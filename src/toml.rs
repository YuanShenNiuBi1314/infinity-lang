//! TOML reader/writer for the toolchain's data files.
//!
//! Normative inputs this must read, all of them TOML-shaped:
//!
//! * §5.2 `MODULE.infy.toml` — `[module]`, `[dependencies]`, `[features]`, `[profile]`,
//!   `[b_track]`, `[[bin]]`, `[[lib]]`;
//! * §9.6 `[lints]` — keys shaped `W-0070 = "deny"`;
//! * appendix E.2 `INFIB-META/manifest.toml` — `[lib]`, `[lib.profiles]`, `[[code]]`, `[[deps]]`;
//! * appendix E.3 `IFBC-CAP-<lang>-<minor>.toml` — `[cap]`, `[opcode]` with `0x71`-style bare
//!   keys, `[builtin]`, `[import]`, `[exception]`;
//! * appendix E.4 `TOOLCHAIN.infy.toml` — repeated `[[toolchain]]` plus `[net]`.
//!
//! A failure here is `E-0506` (清单语法错误, §5.2) at the caller; [`TomlError::line`] and
//! [`TomlError::column`] are 1-based and code-point based, the same rule `span` uses for §9.2.
//!
//! # Deliberate subset
//!
//! This is **not** a full TOML 1.0 implementation: extra surface inside the trust boundary is extra
//! code nobody audits. Unsupported constructs return [`TomlError::Unsupported`] rather than being
//! silently accepted.
//!
//! | accepted | rejected, with a typed error |
//! | --- | --- |
//! | `[a]`, `[a.b]` headers; dotted keys `a.b.c = …`; `[[bin]]` arrays of tables | duplicate key, duplicate `[table]`, value used as a table → [`TomlError::Redefinition`] |
//! | | `[t.u]` / `[[t.u]]` headers *inside* a `[[t]]` element: TOML resolves them against the newest element, so this subset asks for them inline |
//! | inline tables `{ k = v }`; arrays, multi-line and nested | trailing comma in an array; newline inside an inline table |
//! | basic strings with `\b\t\n\f\r\"\\` and `\uXXXX`/`\UXXXXXXXX`; literal `'…'`; quoted keys `"a.b"` / `'a.b'` | multi-line basic/literal strings (`"""`, `'''`) |
//! | integers (decimal, `0x`/`0o`/`0b`, `_` separators), floats with fraction/exponent, `inf`, `nan`, `-inf`, `nan` | `+inf`, `+nan` (no spec file writes them); leading-zero decimals (`07`) |
//! | `true`/`false`; `#` comments | dates and RFC 3339 datetimes |
//!
//! # Two documented spec-conformance extensions
//!
//! 1. **`;` separates key/value pairs on one line.** Appendices E.2–E.4 write pseudo-TOML:
//!    `lang = "cpython" ; format = "CPY-313" ; cap_version = 1`, `[[deps]]` followed by
//!    `path = "…" ; version = "…" ; dirhash = "…"`, and E.4 even writes `[net] ; proxy = "…"` with
//!    the first pair on the header line. Real TOML 1.0 forbids `;` as a separator, so the spec's own
//!    normative examples would not load. `;` is therefore accepted as a *statement separator*: a
//!    `;` ends the current statement and the next statement begins on the same line. The extension
//!    is strictly additive — every document that real TOML accepts (inside the subset above) parses
//!    identically here, so a hand-written manifest gains nothing and loses nothing.
//! 2. **`(* … *)` block comments.** §5.2's own `MODULE.infy.toml` sample annotates values with
//!    `(* 必填… *)`, which is the §2 source-language comment form, not TOML. A manifest copied
//!    verbatim out of the spec must load, so `(* … *)` is read as a comment; nesting is not
//!    supported and an unterminated one is `E-0506`.
//!
//! # Determinism (§7.3)
//!
//! Tables are `BTreeMap`, so iteration is key-sorted and [`to_toml`] is byte-stable across hosts: a
//! manifest's own hash feeds the §5.10 cache key and §7.3 `build-id`, and §7.3 forbids any
//! host-dependent ordering. [`to_toml`] output re-parses to an equal [`Value`].

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// A parsed TOML value. `BTreeMap` — not `HashMap` — is required by §7.3: emitted manifests and the
/// digests taken over them must be byte-identical on every host.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// A table: `[a]`, `[a.b]`, `{ k = v }`.
    Table(BTreeMap<String, Value>),
    /// An array: `targets = [...]`. A `[[bin]]` header produces an array whose elements are tables.
    Array(Vec<Value>),
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    /// "The key exists but carries no value" — a placeholder for callers (a feature name with no
    /// payload, a missing capability row). Never produced by [`parse`]. [`to_toml`] renders it as
    /// `{}`, so `Empty` itself is the one variant that does not round-trip.
    Empty,
}

impl Value {
    /// Field of a table; `None` for non-tables and for missing keys.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Table(m) => m.get(key),
            _ => None,
        }
    }

    /// Dotted lookup, e.g. `get_path("module.path")` (§5.2) or
    /// `get_path("lib.profiles.fast_math")` (E.2). Segments split on `.`; array elements are not
    /// addressable, so `deps.0.path` resolves to `None` (use [`Value::as_array`] for those).
    pub fn get_path(&self, dotted: &str) -> Option<&Value> {
        let mut node = self;
        for seg in dotted.split('.') {
            node = node.get(seg)?;
        }
        Some(node)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Array view: both `k = [..]` and the elements collected by `[[t]]`.
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_table(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Table(m) => Some(m),
            _ => None,
        }
    }

    pub fn as_table_mut(&mut self) -> Option<&mut BTreeMap<String, Value>> {
        match self {
            Value::Table(m) => Some(m),
            _ => None,
        }
    }

    /// Required-key checks for callers implementing `E-0554` (缺 `ifabi` / `abi_hash` /
    /// `deps.dirhash`) and §5.2's `W-0500` unknown-key warning.
    pub fn is_empty_table(&self) -> bool {
        matches!(self, Value::Table(m) if m.is_empty())
    }

    pub fn new_table() -> Value {
        Value::Table(BTreeMap::new())
    }

    fn kind_name(&self) -> &'static str {
        match self {
            Value::Table(_) => "table",
            Value::Array(_) => "array",
            Value::Str(_) => "string",
            Value::Int(_) => "integer",
            Value::Float(_) => "float",
            Value::Bool(_) => "boolean",
            Value::Empty => "empty",
        }
    }
}

/// Parse failure; the caller renders it as `E-0506` (§5.2) with the §9.2 position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TomlError {
    /// Malformed syntax at this position.
    Syntax { line: usize, column: usize, message: String },
    /// Syntactically TOML, but outside this module's subset: multi-line strings, datetimes, `+inf`.
    Unsupported { line: usize, column: usize, message: String },
    /// Duplicate key, duplicate `[table]`, or a non-table used as a table.
    Redefinition { line: usize, column: usize, message: String },
    /// Input ended where a token was required.
    UnexpectedEof { line: usize, column: usize, message: String },
}

impl TomlError {
    /// 1-based line number (§9.2).
    pub fn line(&self) -> usize {
        match self {
            TomlError::Syntax { line, .. }
            | TomlError::Unsupported { line, .. }
            | TomlError::Redefinition { line, .. }
            | TomlError::UnexpectedEof { line, .. } => *line,
        }
    }

    /// 1-based, code-point-based column (§9.2).
    pub fn column(&self) -> usize {
        match self {
            TomlError::Syntax { column, .. }
            | TomlError::Unsupported { column, .. }
            | TomlError::Redefinition { column, .. }
            | TomlError::UnexpectedEof { column, .. } => *column,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            TomlError::Syntax { message, .. }
            | TomlError::Unsupported { message, .. }
            | TomlError::Redefinition { message, .. }
            | TomlError::UnexpectedEof { message, .. } => message,
        }
    }

    /// Short kind tag for the diagnostic headline.
    pub fn kind(&self) -> &'static str {
        match self {
            TomlError::Syntax { .. } => "syntax",
            TomlError::Unsupported { .. } => "unsupported construct",
            TomlError::Redefinition { .. } => "duplicate definition",
            TomlError::UnexpectedEof { .. } => "unexpected end of input",
        }
    }

    fn syntax(loc: Loc, message: impl Into<String>) -> TomlError {
        TomlError::Syntax { line: loc.line, column: loc.col, message: message.into() }
    }
    fn unsupported(loc: Loc, message: impl Into<String>) -> TomlError {
        TomlError::Unsupported { line: loc.line, column: loc.col, message: message.into() }
    }
    fn redefinition(loc: Loc, message: impl Into<String>) -> TomlError {
        TomlError::Redefinition { line: loc.line, column: loc.col, message: message.into() }
    }
    fn eof(loc: Loc, message: impl Into<String>) -> TomlError {
        TomlError::UnexpectedEof { line: loc.line, column: loc.col, message: message.into() }
    }
}

impl fmt::Display for TomlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at {}:{}: {}",
            self.kind(),
            self.line(),
            self.column(),
            self.message()
        )
    }
}

impl std::error::Error for TomlError {}

/// Parse a manifest, capability table or lock-adjacent TOML file (§5.2, E.2–E.4).
pub fn parse(text: &str) -> Result<Value, TomlError> {
    let stripped;
    let body = match text.strip_prefix('\u{FEFF}') {
        Some(rest) => {
            stripped = rest.to_string();
            &stripped
        }
        None => text,
    };
    // CRLF normalised so `\n` is the only statement terminator. Columns are unaffected: `\r` only
    // ever sits at a line end, after every code point of the line.
    let normalized;
    let src = if body.contains('\r') {
        normalized = body.replace("\r\n", "\n");
        &normalized
    } else {
        body
    };
    let mut p = Parser::new(src);
    let stmts = p.statements()?;
    assemble(&stmts).map(Value::Table)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Loc {
    line: usize,
    col: usize,
}

/// One parsed statement, kept apart from the document tree so that the tree is assembled in a
/// second pass (the ledger rules then need no interaction with the lexer's borrow graph).
enum Stmt {
    /// `[a.b]`
    Table { path: Vec<String>, loc: Loc },
    /// `[[a.b]]`
    ArrayOfTables { path: Vec<String>, loc: Loc },
    /// `a.b = <value>`; `inline` marks an inline-table value, which TOML makes immutable.
    Assign { path: Vec<String>, value: Value, inline: bool, loc: Loc },
}

/// Where plain `key = value` statements land.
#[derive(Clone, Debug)]
enum Scope {
    Root,
    Table(Vec<String>),
    /// The newest element of a `[[t]]` array.
    ArrayTable(Vec<String>),
}

/// One step of a path inside the tree. `Element` means "last element of the array at this key".
#[derive(Clone, Debug)]
enum Step {
    Key(String),
    Element,
}

/// Leading sign of a number literal; `Plus` exists so that `+inf`/`+nan` can be refused with a
/// precise message while `+1`/`+1.5` stay legal TOML.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sign {
    None,
    Plus,
    Minus,
}

fn scope_steps(scope: &Scope) -> Vec<Step> {
    match scope {
        Scope::Root => Vec::new(),
        Scope::Table(p) => p.iter().map(|k| Step::Key(k.clone())).collect(),
        Scope::ArrayTable(p) => {
            let mut v: Vec<Step> = p.iter().map(|k| Step::Key(k.clone())).collect();
            v.push(Step::Element);
            v
        }
    }
}

struct Parser {
    src: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
}

impl Parser {
    fn new(text: &str) -> Parser {
        Parser { src: text.chars().collect(), pos: 0, line: 1, col: 1 }
    }

    fn loc(&self) -> Loc {
        Loc { line: self.line, col: self.col }
    }

    fn cur(&self) -> Option<char> {
        self.src.get(self.pos).copied()
    }

    fn nth(&self, k: usize) -> Option<char> {
        self.src.get(self.pos + k).copied()
    }

    fn char_at(&self, k: usize) -> char {
        self.src.get(self.pos + k).copied().unwrap_or('\n')
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.cur()?;
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn starts_with(&self, s: &str) -> bool {
        s.chars().enumerate().all(|(i, c)| self.char_at(i) == c)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.cur() == Some(c) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// Spaces and tabs only — a newline terminates a statement.
    fn skip_spaces(&mut self) {
        while matches!(self.cur(), Some(' ') | Some('\t')) {
            self.bump();
        }
    }

    fn skip_line_comment(&mut self) {
        while let Some(c) = self.cur() {
            if c == '\n' {
                break;
            }
            self.bump();
        }
    }

    /// Extension 2: §5.2's `(* … *)` comment form. Not nestable.
    fn skip_block_comment(&mut self) -> Result<(), TomlError> {
        let start = self.loc();
        self.bump();
        self.bump();
        loop {
            match self.cur() {
                None => return Err(TomlError::eof(start, "unterminated `(*` comment")),
                Some('*') if self.nth(1) == Some(')') => {
                    self.bump();
                    self.bump();
                    return Ok(());
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    /// Everything allowed *between* statements.
    fn skip_trivia(&mut self) -> Result<(), TomlError> {
        loop {
            match self.cur() {
                Some(' ') | Some('\t') | Some('\n') => {
                    self.bump();
                }
                Some('#') => {
                    self.bump();
                    self.skip_line_comment();
                }
                Some('(') if self.starts_with("(*") => self.skip_block_comment()?,
                _ => return Ok(()),
            }
        }
    }

    fn statements(&mut self) -> Result<Vec<Stmt>, TomlError> {
        let mut out = Vec::new();
        loop {
            self.skip_trivia()?;
            let start = self.loc();
            match self.cur() {
                None => return Ok(out),
                // Extension 1: a stray `;` between statements.
                Some(';') => {
                    self.bump();
                }
                Some('[') => {
                    self.bump();
                    let is_array = self.eat('[');
                    let path = self.key_path()?;
                    if path.is_empty() {
                        return Err(TomlError::syntax(start, "empty table header `[]`"));
                    }
                    self.skip_spaces();
                    if is_array {
                        if !(self.eat(']') && self.eat(']')) {
                            return Err(TomlError::syntax(
                                self.loc(),
                                "array-of-tables header must close with `]]`",
                            ));
                        }
                        out.push(Stmt::ArrayOfTables { path, loc: start });
                    } else {
                        if !self.eat(']') {
                            return Err(TomlError::syntax(
                                self.loc(),
                                "table header must close with `]`",
                            ));
                        }
                        out.push(Stmt::Table { path, loc: start });
                    }
                }
                _ => {
                    let path = self.key_path()?;
                    if path.is_empty() {
                        return Err(TomlError::syntax(start, "expected a key"));
                    }
                    self.skip_spaces();
                    if !self.eat('=') {
                        return Err(TomlError::syntax(
                            self.loc(),
                            format!("expected `=` after key `{}`", path.join(".")),
                        ));
                    }
                    self.skip_spaces();
                    let inline = matches!(self.cur(), Some('{'));
                    let value = self.value()?;
                    out.push(Stmt::Assign { path, value, inline, loc: start });
                }
            }
            self.end_of_statement()?;
        }
    }

    /// After a statement only spaces, a comment (either form), a newline, EOF or `;` may follow.
    fn end_of_statement(&mut self) -> Result<(), TomlError> {
        let start = self.loc();
        self.skip_spaces();
        match self.cur() {
            None | Some('\n') => Ok(()),
            Some('#') => {
                self.bump();
                self.skip_line_comment();
                Ok(())
            }
            Some('(') if self.starts_with("(*") => self.skip_block_comment(),
            Some(';') => {
                self.bump();
                Ok(())
            }
            Some(c) => Err(TomlError::syntax(
                start,
                format!(
                    "unexpected '{}' after a statement; a statement ends at a newline, a comment \
                     or `;` (the separator appendices E.2–E.4 use)",
                    c
                ),
            )),
        }
    }

    /// `a`, `a.b.c`, `"quoted.key"`, `'literal key'`.
    fn key_path(&mut self) -> Result<Vec<String>, TomlError> {
        let mut out = Vec::new();
        loop {
            let start = self.loc();
            out.push(self.key_segment(start)?);
            self.skip_spaces();
            if self.cur() == Some('.') {
                self.bump();
                self.skip_spaces();
                continue;
            }
            return Ok(out);
        }
    }

    fn key_segment(&mut self, start: Loc) -> Result<String, TomlError> {
        match self.cur() {
            Some('"') | Some('\'') => match self.string()? {
                Value::Str(s) => Ok(s),
                _ => unreachable!("string() only returns Str"),
            },
            Some(c) if is_bare_key_char(c) => {
                let mut s = String::new();
                while let Some(c) = self.cur() {
                    if !is_bare_key_char(c) {
                        break;
                    }
                    s.push(c);
                    self.bump();
                }
                Ok(s)
            }
            Some(c) => Err(TomlError::syntax(
                start,
                format!(
                    "invalid key character '{}'; a key is bare ([A-Za-z0-9_-]+) or quoted",
                    c
                ),
            )),
            None => Err(TomlError::eof(start, "expected a key")),
        }
    }

    fn value(&mut self) -> Result<Value, TomlError> {
        let start = self.loc();
        match self.cur() {
            None => Err(TomlError::eof(start, "expected a value")),
            Some('"') | Some('\'') => self.string(),
            Some('[') => self.array(),
            Some('{') => self.inline_table(),
            Some('t') if self.starts_with("true") => {
                self.keyword("true")?;
                Ok(Value::Bool(true))
            }
            Some('f') if self.starts_with("false") => {
                self.keyword("false")?;
                Ok(Value::Bool(false))
            }
            Some('i') if self.starts_with("inf") => {
                self.keyword("inf")?;
                Ok(Value::Float(f64::INFINITY))
            }
            Some('n') if self.starts_with("nan") => {
                self.keyword("nan")?;
                Ok(Value::Float(f64::NAN))
            }
            Some('0') if matches!(self.nth(1), Some('x') | Some('o') | Some('b')) => {
                self.prefixed_integer()
            }
            Some(c) if c.is_ascii_digit() || c == '-' || c == '+' => self.number(),
            Some(c) => Err(TomlError::syntax(
                start,
                format!(
                    "'{}' does not start a value (strings, numbers, booleans, arrays and inline \
                     tables only; datetimes are out of scope)",
                    c
                ),
            )),
        }
    }

    fn keyword(&mut self, word: &str) -> Result<(), TomlError> {
        for _ in 0..word.len() {
            self.bump();
        }
        self.expect_value_end()
    }

    /// `0x` / `0o` / `0b` integers, e.g. E.3's `cap_version`-adjacent hex rows.
    fn prefixed_integer(&mut self) -> Result<Value, TomlError> {
        let start = self.loc();
        self.bump();
        let radix_char = self.bump().expect("checked by caller");
        let radix: u32 = match radix_char {
            'x' => 16,
            'o' => 8,
            'b' => 2,
            _ => unreachable!(),
        };
        let mut digits = String::new();
        let mut prev_digit = false;
        while let Some(c) = self.cur() {
            if c == '_' {
                if !prev_digit {
                    return Err(TomlError::syntax(
                        self.loc(),
                        "`_` must separate digits",
                    ));
                }
                prev_digit = false;
                self.bump();
                continue;
            }
            if c.is_digit(radix) {
                digits.push(c);
                prev_digit = true;
                self.bump();
            } else {
                break;
            }
        }
        if digits.is_empty() || !prev_digit {
            return Err(TomlError::syntax(
                start,
                format!("`0{}` needs at least one digit and must not end in `_`", radix_char),
            ));
        }
        let v = i64::from_str_radix(&digits, radix).map_err(|_| {
            TomlError::syntax(
                start,
                format!("integer 0{}{} is outside i64", radix_char, digits),
            )
        })?;
        self.expect_value_end()?;
        Ok(Value::Int(v))
    }

    fn number(&mut self) -> Result<Value, TomlError> {
        let start = self.loc();
        let sign = match self.cur() {
            Some('-') => {
                self.bump();
                Sign::Minus
            }
            Some('+') => {
                self.bump();
                Sign::Plus
            }
            _ => Sign::None,
        };
        // `-inf` / `nan`; `+inf`, `+nan` rejected on purpose (module docs).
        if self.starts_with("inf") || self.starts_with("nan") {
            let word = if self.starts_with("inf") { "inf" } else { "nan" };
            if matches!(sign, Sign::Plus) {
                return Err(TomlError::unsupported(
                    start,
                    format!("`+{}` is not accepted: no file defined by §5.2/E.2–E.4 needs it", word),
                ));
            }
            for _ in 0..word.len() {
                self.bump();
            }
            self.expect_value_end()?;
            return Ok(Value::Float(match (word, sign) {
                ("inf", Sign::Minus) => f64::NEG_INFINITY,
                ("inf", _) => f64::INFINITY,
                (_, _) => f64::NAN,
            }));
        }
        if matches!(sign, Sign::Plus)
            && self.cur() == Some('0')
            && matches!(self.nth(1), Some('x') | Some('o') | Some('b'))
        {
            return Err(TomlError::syntax(
                start,
                "`+` is not allowed on a prefixed integer",
            ));
        }
        let mut digits = String::new();
        let mut prev_digit = false;
        while let Some(c) = self.cur() {
            if c == '_' {
                if !prev_digit {
                    return Err(TomlError::syntax(self.loc(), "`_` must separate digits"));
                }
                prev_digit = false;
                self.bump();
                continue;
            }
            if c.is_ascii_digit() {
                digits.push(c);
                prev_digit = true;
                self.bump();
            } else {
                break;
            }
        }
        if digits.is_empty() || !prev_digit {
            return Err(TomlError::syntax(start, "expected a number"));
        }
        // TOML 1.0 forbids leading zeros in a decimal integer part, which is what keeps `07` from
        // being silently read as octal (`0o7`) by a different parser.
        if digits.len() > 1 && digits.starts_with('0') {
            return Err(TomlError::syntax(
                start,
                "decimal numbers must not have a leading zero (`0o755`, not `0755`)",
            ));
        }
        // A `-` + digits right after the integer part is an RFC 3339 date, not a negative number.
        if self.cur() == Some('-') && self.nth(1).map_or(false, |c| c.is_ascii_digit()) {
            let mut raw = digits.clone();
            while let Some(c) = self.cur() {
                if matches!(c, ' ' | '\t' | '\n') {
                    break;
                }
                raw.push(c);
                self.bump();
            }
            return Err(TomlError::unsupported(
                start,
                format!("`{}` looks like a date or datetime; TOML datetimes are out of scope here", raw),
            ));
        }
        let mut text = digits.clone();
        let mut is_float = false;
        if self.cur() == Some('.') && self.nth(1).map_or(false, |c| c.is_ascii_digit()) {
            is_float = true;
            text.push('.');
            self.bump();
            loop {
                match self.cur() {
                    Some('_') => {
                        self.bump();
                    }
                    Some(c) if c.is_ascii_digit() => {
                        text.push(c);
                        self.bump();
                    }
                    _ => break,
                }
            }
        }
        if matches!(self.cur(), Some('e') | Some('E')) {
            is_float = true;
            text.push('e');
            self.bump();
            if let Some(s @ ('+' | '-')) = self.cur() {
                text.push(s);
                self.bump();
            }
            let mut saw = false;
            loop {
                match self.cur() {
                    Some('_') => {
                        self.bump();
                    }
                    Some(c) if c.is_ascii_digit() => {
                        text.push(c);
                        self.bump();
                        saw = true;
                    }
                    _ => break,
                }
            }
            if !saw {
                return Err(TomlError::syntax(self.loc(), "exponent has no digits"));
            }
        }
        self.expect_value_end()?;
        let negative = matches!(sign, Sign::Minus);
        if is_float {
            let f: f64 = text.parse().map_err(|_| {
                TomlError::syntax(start, format!("`{}` is not a valid float", text))
            })?;
            Ok(Value::Float(if negative { -f } else { f }))
        } else {
            // i128 first so that `-9223372036854775808` (i64::MIN) is representable.
            let magnitude: i128 = text.parse().map_err(|_| {
                TomlError::syntax(start, format!("integer `{}` is outside i64", text))
            })?;
            let v: i64 = if negative {
                i64::try_from(-magnitude).map_err(|_| {
                    TomlError::syntax(start, format!("integer `-{}` is outside i64", text))
                })?
            } else {
                i64::try_from(magnitude).map_err(|_| {
                    TomlError::syntax(start, format!("integer `{}` is outside i64", text))
                })?
            };
            Ok(Value::Int(v))
        }
    }

    /// A value must be followed by a delimiter, never by more word characters — which is what
    /// catches `1.2.3`, `0x71g`, `truex` and `1979-05-27`.
    fn expect_value_end(&self) -> Result<(), TomlError> {
        match self.cur() {
            None | Some('\n') | Some(' ') | Some('\t') | Some(',') | Some(']') | Some('}')
            | Some('#') | Some(';') => Ok(()),
            Some('(') if self.starts_with("(*") => Ok(()),
            Some(c) => Err(TomlError::syntax(
                self.loc(),
                format!(
                    "unexpected '{}' right after a value (near `{}`); trailing characters are not \
                     allowed — datetimes and multi-line strings are also unsupported",
                    c,
                    self.tail_snippet()
                ),
            )),
        }
    }

    fn tail_snippet(&self) -> String {
        self.src
            .iter()
            .skip(self.pos.saturating_sub(8))
            .take(20)
            .collect::<String>()
    }

    /// Basic (`"…"`, with escapes) and literal (`'…'`, no escapes) strings.
    fn string(&mut self) -> Result<Value, TomlError> {
        let start = self.loc();
        let quote = self.cur().expect("caller checked");
        if self.nth(1) == Some(quote) && self.nth(2) == Some(quote) {
            return Err(TomlError::unsupported(
                start,
                format!(
                    "multi-line strings (`{0}{0}{0}`) are not supported; nothing in §5.2/E.2–E.4 uses one",
                    quote
                ),
            ));
        }
        self.bump();
        let mut out = String::new();
        loop {
            let loc = self.loc();
            match self.cur() {
                None => {
                    return Err(TomlError::eof(
                        start,
                        format!("unterminated string: missing closing `{}`", quote),
                    ))
                }
                Some('\n') => {
                    return Err(TomlError::syntax(
                        loc,
                        "a string cannot contain a raw newline; write `\\n` (multi-line strings are \
                         unsupported)",
                    ))
                }
                Some(c) if c == quote => {
                    self.bump();
                    return Ok(Value::Str(out));
                }
                Some('\\') if quote == '"' => {
                    self.bump();
                    let esc = self.loc();
                    match self.cur() {
                        None => {
                            return Err(TomlError::eof(esc, "unterminated escape sequence"))
                        }
                        Some('b') => {
                            self.bump();
                            out.push('\u{0008}');
                        }
                        Some('t') => {
                            self.bump();
                            out.push('\t');
                        }
                        Some('n') => {
                            self.bump();
                            out.push('\n');
                        }
                        Some('f') => {
                            self.bump();
                            out.push('\u{000C}');
                        }
                        Some('r') => {
                            self.bump();
                            out.push('\r');
                        }
                        Some('"') => {
                            self.bump();
                            out.push('"');
                        }
                        Some('\\') => {
                            self.bump();
                            out.push('\\');
                        }
                        Some('u') => {
                            self.bump();
                            out.push(self.unicode_escape(4, esc)?);
                        }
                        Some('U') => {
                            self.bump();
                            out.push(self.unicode_escape(8, esc)?);
                        }
                        Some(other) => {
                            return Err(TomlError::syntax(
                                esc,
                                format!("`\\{}` is not a TOML escape sequence", other),
                            ))
                        }
                    }
                }
                Some(c) => {
                    if (c as u32) < 0x20 && c != '\t' {
                        return Err(TomlError::syntax(
                            loc,
                            format!(
                                "control character U+{:04X} cannot appear raw in a string",
                                c as u32
                            ),
                        ));
                    }
                    out.push(c);
                    self.bump();
                }
            }
        }
    }

    fn unicode_escape(&mut self, width: usize, start: Loc) -> Result<char, TomlError> {
        let mut acc: u32 = 0;
        for _ in 0..width {
            let loc = self.loc();
            match self.cur() {
                Some(c) if c.is_ascii_hexdigit() => {
                    acc = acc * 16 + c.to_digit(16).expect("hexdigit");
                    self.bump();
                }
                None => return Err(TomlError::eof(loc, "unterminated `\\u` escape")),
                Some(c) => {
                    return Err(TomlError::syntax(
                        start,
                        format!("`\\u` escape needs {} hex digits, found '{}'", width, c),
                    ))
                }
            }
        }
        char::from_u32(acc)
            .ok_or_else(|| TomlError::syntax(start, format!("U+{:X} is not a Unicode scalar", acc)))
    }

    /// Arrays may span lines; comments are allowed inside; a trailing comma is not (TOML 1.0).
    fn array(&mut self) -> Result<Value, TomlError> {
        self.bump(); // '['
        let mut items = Vec::new();
        loop {
            self.skip_trivia()?;
            let loc = self.loc();
            match self.cur() {
                None => return Err(TomlError::eof(loc, "unterminated array: missing `]`")),
                Some(']') => {
                    self.bump();
                    return Ok(Value::Array(items));
                }
                _ => {}
            }
            items.push(self.value()?);
            self.skip_trivia()?;
            let loc = self.loc();
            match self.cur() {
                Some(',') => {
                    self.bump();
                    self.skip_trivia()?;
                    if self.cur() == Some(']') {
                        return Err(TomlError::syntax(
                            loc,
                            "trailing comma is not allowed in a TOML 1.0 array",
                        ));
                    }
                }
                Some(']') => {
                    self.bump();
                    return Ok(Value::Array(items));
                }
                Some(c) => {
                    return Err(TomlError::syntax(
                        loc,
                        format!("expected ',' or ']' in an array, found '{}'", c),
                    ))
                }
                None => return Err(TomlError::eof(loc, "unterminated array: missing `]`")),
            }
        }
    }

    fn inline_table(&mut self) -> Result<Value, TomlError> {
        let start = self.loc();
        self.bump(); // '{'
        let mut map: BTreeMap<String, Value> = BTreeMap::new();
        self.skip_spaces();
        if self.cur() == Some('}') {
            self.bump();
            return Ok(Value::Table(map));
        }
        loop {
            let kloc = self.loc();
            if kloc.line != start.line || self.cur() == Some('\n') {
                return Err(TomlError::syntax(
                    kloc,
                    "an inline table must stay on one line (TOML 1.0)",
                ));
            }
            let path = self.key_path()?;
            self.skip_spaces();
            let loc = self.loc();
            if !self.eat('=') {
                return Err(TomlError::syntax(loc, "expected `=` inside an inline table"));
            }
            self.skip_spaces();
            let value = self.value()?;
            insert_literal(&mut map, &path, value, kloc)?;
            self.skip_spaces();
            match self.cur() {
                Some(',') => {
                    self.bump();
                    self.skip_spaces();
                }
                Some('}') => {
                    self.bump();
                    return Ok(Value::Table(map));
                }
                None => {
                    return Err(TomlError::eof(self.loc(), "unterminated inline table: missing `}`"))
                }
                Some('\n') => {
                    return Err(TomlError::syntax(
                        self.loc(),
                        "an inline table must stay on one line (TOML 1.0): newline before `}`",
                    ))
                }
                Some(c) => {
                    return Err(TomlError::syntax(
                        self.loc(),
                        format!("expected ',' or '}}' in an inline table, found '{}'", c),
                    ))
                }
            }
        }
    }
}

/// Insert into a value literal (inline table / array element), honouring "define once" inside the
/// literal itself. Independent of the document ledger, which only governs top-level statements.
fn insert_literal(
    map: &mut BTreeMap<String, Value>,
    path: &[String],
    value: Value,
    loc: Loc,
) -> Result<(), TomlError> {
    let mut node = map;
    for seg in &path[..path.len() - 1] {
        let entry = node
            .entry(seg.clone())
            .or_insert_with(|| Value::Table(BTreeMap::new()));
        node = match entry {
            Value::Table(m) => m,
            other => {
                return Err(TomlError::redefinition(
                    loc,
                    format!("`{}` is not a table ({})", seg, other.kind_name()),
                ))
            }
        };
    }
    let last = path.last().expect("non-empty key path").clone();
    if node.contains_key(&last) {
        return Err(TomlError::redefinition(
            loc,
            format!("duplicate key `{}` in an inline table", path.join(".")),
        ));
    }
    node.insert(last, value);
    Ok(())
}

#[derive(Clone, Debug)]
struct AssembleError {
    loc: Loc,
    kind: Kind,
    message: String,
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Redefinition,
    Syntax,
}

impl AssembleError {
    fn into_toml_error(self) -> TomlError {
        match self.kind {
            Kind::Redefinition => TomlError::redefinition(self.loc, self.message),
            Kind::Syntax => TomlError::syntax(self.loc, self.message),
        }
    }
}

impl From<AssembleError> for TomlError {
    fn from(e: AssembleError) -> TomlError {
        e.into_toml_error()
    }
}

/// Second pass: fold the statement list into the tree, applying TOML's "define once" rules.
fn assemble(stmts: &[Stmt]) -> Result<BTreeMap<String, Value>, TomlError> {
    let mut root = Value::new_table();
    let mut ledger = Ledger::new();
    let mut scope = Scope::Root;
    for stmt in stmts {
        match stmt {
            Stmt::Table { path, loc } => {
                open_table(&mut root, &mut ledger, path, *loc)?;
                scope = Scope::Table(path.clone());
            }
            Stmt::ArrayOfTables { path, loc } => {
                open_array_of_tables(&mut root, &mut ledger, path, *loc)?;
                scope = Scope::ArrayTable(path.clone());
            }
            Stmt::Assign { path, value, inline, loc } => {
                let base = scope_steps(&scope);
                assign(&mut root, &mut ledger, &base, path, value.clone(), *inline, *loc)?;
            }
        }
    }
    match root {
        Value::Table(m) => Ok(m),
        _ => unreachable!("the root is always a table"),
    }
}

struct Ledger {
    /// Opened by a `[header]`.
    defined: BTreeSet<Vec<String>>,
    /// Created implicitly by a dotted key or by walking a header prefix.
    implicit: BTreeSet<Vec<String>>,
    /// Bound to a `[[header]]`.
    arrays: BTreeSet<Vec<String>>,
    /// Inline tables are immutable, so nothing may be added to them afterwards.
    frozen: BTreeSet<Vec<String>>,
}

impl Ledger {
    fn new() -> Ledger {
        Ledger {
            defined: BTreeSet::new(),
            implicit: BTreeSet::new(),
            arrays: BTreeSet::new(),
            frozen: BTreeSet::new(),
        }
    }
}

/// Walk `steps` from `node`, creating missing intermediate tables (recorded in `created`, all of
/// them implicit). Returns the node at the end of the walk together with its **canonical path**:
/// a `Step::Element` contributes a `#<index>` segment, so two keys that live in different
/// `[[t]]` elements get different paths and never shadow each other in the ledger.
fn descend<'a>(
    node: &'a mut Value,
    steps: &[Step],
    prefix: Vec<String>,
    created: &mut Vec<Vec<String>>,
    ledger: &Ledger,
    loc: Loc,
) -> Result<(&'a mut Value, Vec<String>), AssembleError> {
    let (step, rest) = match steps.split_first() {
        None => return Ok((node, prefix)),
        Some(x) => x,
    };
    match step {
        Step::Key(k) => {
            let mut child_prefix = prefix.clone();
            child_prefix.push(k.clone());
            let map = as_table(node, &prefix, loc)?;
            let child = match map.entry(k.clone()) {
                std::collections::btree_map::Entry::Occupied(occ) => {
                    if ledger.frozen.contains(&child_prefix) {
                        return Err(err(
                            loc,
                            Kind::Redefinition,
                            format!("cannot extend inline table `{}`", child_prefix.join(".")),
                        ));
                    }
                    occ.into_mut()
                }
                std::collections::btree_map::Entry::Vacant(vac) => {
                    created.push(child_prefix.clone());
                    vac.insert(Value::Table(BTreeMap::new()))
                }
            };
            descend(child, rest, child_prefix, created, ledger, loc)
        }
        Step::Element => {
            let items = match node {
                Value::Array(a) => a,
                other => {
                    return Err(err(
                        loc,
                        Kind::Redefinition,
                        format!("`{}` is not an array of tables ({})", prefix.join("."), other.kind_name()),
                    ))
                }
            };
            let mut child_prefix = prefix.clone();
            child_prefix.push(format!("#{}", items.len().saturating_sub(1)));
            let last = items
                .last_mut()
                .ok_or_else(|| err(loc, Kind::Syntax, format!("array `{}` is empty", prefix.join("."))))?;
            descend(last, rest, child_prefix, created, ledger, loc)
        }
    }
}

fn as_table<'a>(
    node: &'a mut Value,
    prefix: &[String],
    loc: Loc,
) -> Result<&'a mut BTreeMap<String, Value>, AssembleError> {
    match node {
        Value::Table(m) => Ok(m),
        Value::Array(_) => Err(err(
            loc,
            Kind::Redefinition,
            format!(
                "`{}` is an array of tables, not a table: sub-tables of a [[t]] element must be \
                 written inline (`cfg = {{ … }}`), never as a `[t.cfg]` header",
                prefix.join(".")
            ),
        )),
        other => Err(err(
            loc,
            Kind::Redefinition,
            format!("`{}` is not a table ({})", prefix.join("."), other.kind_name()),
        )),
    }
}

fn err(loc: Loc, kind: Kind, message: impl Into<String>) -> AssembleError {
    AssembleError { loc, kind, message: message.into() }
}

fn assign(
    root: &mut Value,
    ledger: &mut Ledger,
    base: &[Step],
    path: &[String],
    value: Value,
    inline: bool,
    loc: Loc,
) -> Result<(), TomlError> {
    if path.is_empty() {
        return Err(TomlError::syntax(loc, "expected a key"));
    }
    let mut steps: Vec<Step> = base.to_vec();
    steps.extend(path.iter().map(|k| Step::Key(k.clone())));
    let (head, last) = steps.split_at(steps.len() - 1);
    let last_key = match &last[0] {
        Step::Key(k) => k.clone(),
        Step::Element => unreachable!("an assignment always ends on a key"),
    };
    let mut created = Vec::new();
    let (target, prefix) = descend(root, head, Vec::new(), &mut created, ledger, loc)?;
    let mut full = prefix;
    full.push(last_key.clone());
    let map = as_table(target, &full[..full.len() - 1], loc)?;
    if map.contains_key(&last_key)
        || ledger.defined.contains(&full)
        || ledger.arrays.contains(&full)
    {
        return Err(TomlError::redefinition(
            loc,
            format!("duplicate key `{}`", full.join(".")),
        ));
    }
    ledger.implicit.extend(created);
    if inline {
        ledger.frozen.insert(full);
    }
    map.insert(last_key, value);
    Ok(())
}

fn open_table(
    root: &mut Value,
    ledger: &mut Ledger,
    path: &[String],
    loc: Loc,
) -> Result<(), TomlError> {
    let steps: Vec<Step> = path.iter().map(|k| Step::Key(k.clone())).collect();
    let (head, last) = steps.split_at(steps.len() - 1);
    let last_key = match &last[0] {
        Step::Key(k) => k.clone(),
        Step::Element => unreachable!("a header never ends on an element step"),
    };
    let mut created = Vec::new();
    let (parent, _) = descend(root, head, Vec::new(), &mut created, ledger, loc)?;
    let map = as_table(parent, &path[..path.len() - 1], loc)?;
    ledger.implicit.extend(created);
    // `Option<kind_name>` instead of a borrow of `map`, so the branches below may still mutate it.
    match map.get(&last_key).map(Value::kind_name) {
        None => {
            map.insert(last_key, Value::Table(BTreeMap::new()));
            ledger.defined.insert(path.to_vec());
        }
        Some("table") => {
            let full = path.to_vec();
            if ledger.frozen.contains(&full) {
                return Err(TomlError::redefinition(
                    loc,
                    format!("`{}` is an inline table and cannot be re-opened", path.join(".")),
                ));
            }
            if ledger.arrays.contains(&full) {
                return Err(TomlError::redefinition(
                    loc,
                    format!("`{}` is a [[{}]] array of tables", path.join("."), path.join(".")),
                ));
            }
            if !ledger.implicit.remove(&full) {
                return Err(TomlError::redefinition(
                    loc,
                    format!("table [{}] is defined twice", path.join(".")),
                ));
            }
            ledger.defined.insert(full);
        }
        Some(kind) => {
            return Err(TomlError::redefinition(
                loc,
                format!("`{}` already holds a {}", path.join("."), kind),
            ))
        }
    }
    Ok(())
}

fn open_array_of_tables(
    root: &mut Value,
    ledger: &mut Ledger,
    path: &[String],
    loc: Loc,
) -> Result<(), TomlError> {
    let steps: Vec<Step> = path.iter().map(|k| Step::Key(k.clone())).collect();
    let (head, last) = steps.split_at(steps.len() - 1);
    let last_key = match &last[0] {
        Step::Key(k) => k.clone(),
        Step::Element => unreachable!("a header never ends on an element step"),
    };
    let mut created = Vec::new();
    let (parent, _) = descend(root, head, Vec::new(), &mut created, ledger, loc)?;
    let map = as_table(parent, &path[..path.len() - 1], loc)?;
    ledger.implicit.extend(created);
    match map.get(&last_key).map(Value::kind_name) {
        None => {
            map.insert(last_key, Value::Array(vec![Value::Table(BTreeMap::new())]));
            ledger.arrays.insert(path.to_vec());
        }
        Some("array") => {
            if !ledger.arrays.contains(path) {
                return Err(TomlError::redefinition(
                    loc,
                    format!(
                        "`{}` is a static array; [[{}]] may only extend an array of tables",
                        path.join("."),
                        path.join(".")
                    ),
                ));
            }
            if let Some(Value::Array(items)) = map.get_mut(&last_key) {
                items.push(Value::Table(BTreeMap::new()));
            } else {
                unreachable!("kind_name said array");
            }
        }
        Some(kind) => {
            return Err(TomlError::redefinition(
                loc,
                format!("`{}` already holds a {}", path.join("."), kind),
            ))
        }
    }
    Ok(())
}

fn is_bare_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Emit a stable, re-parsable document (§7.3: byte-identical output for a byte-identical [`Value`]).
///
/// Layout per table: plain keys first, then `[sub]` sections, then `[[array]]` sections, each group
/// in [`BTreeMap`] key order. [`to_toml(&parse(text))`] re-parses to an equal value for every
/// document in the accepted subset except one: an `Empty` placeholder, which has no TOML spelling
/// (see [`Value::Empty`]).
///
/// A root that is not a table is emitted as its inline value, which is not a document — callers
/// write manifests, not bare scalars.
pub fn to_toml(value: &Value) -> String {
    let mut out = String::new();
    match value {
        Value::Table(map) => emit_body(&[], map, &mut out, true),
        other => {
            emit_inline(other, &mut out);
            out.push('\n');
        }
    }
    out
}

/// `sections` says whether sub-tables may be written as `[path.key]` headers. Inside a `[[t]]`
/// element they may not: TOML resolves `[t.u]` against *the newest* element of `t`, so a section
/// following element *n* would silently attach to the wrong element (and `cfg2 = 3` after it would
/// land inside `cfg`). Nested tables of an element are therefore written as inline values, which
/// re-parse to the same [`Value`] regardless of element position.
fn emit_body(path: &[String], map: &BTreeMap<String, Value>, out: &mut String, sections: bool) {
    for (k, v) in map {
        match v {
            Value::Table(_) if sections => {}
            Value::Array(items) if sections && is_array_of_tables(items) => {}
            _ => {
                out.push_str(&format_key(k));
                out.push_str(" = ");
                emit_inline(v, out);
                out.push('\n');
            }
        }
    }
    if !sections {
        return;
    }
    for (k, v) in map {
        if let Value::Table(sub) = v {
            push_blank(out);
            out.push_str(&format!("[{}]\n", join_path(path, k)));
            emit_body(&child(path, k), sub, out, true);
        }
    }
    for (k, v) in map {
        if let Value::Array(items) = v {
            if is_array_of_tables(items) {
                for item in items {
                    push_blank(out);
                    out.push_str(&format!("[[{}]]\n", join_path(path, k)));
                    if let Value::Table(sub) = item {
                        // Element bodies stay flat: see the doc comment above.
                        emit_body(&child(path, k), sub, out, false);
                    } else {
                        unreachable!("is_array_of_tables");
                    }
                }
            }
        }
    }
}

fn child(path: &[String], key: &str) -> Vec<String> {
    let mut v = path.to_vec();
    v.push(key.to_string());
    v
}

fn join_path(path: &[String], key: &str) -> String {
    let mut s = String::new();
    for p in path {
        s.push_str(&format_key(p));
        s.push('.');
    }
    s.push_str(&format_key(key));
    s
}

fn push_blank(out: &mut String) {
    if !out.is_empty() && !out.ends_with("\n\n") {
        out.push('\n');
    }
}

fn is_array_of_tables(items: &[Value]) -> bool {
    !items.is_empty() && items.iter().all(|v| matches!(v, Value::Table(_)))
}

fn emit_inline(value: &Value, out: &mut String) {
    match value {
        Value::Str(s) => emit_basic_string(s, out),
        Value::Int(i) => out.push_str(&i.to_string()),
        Value::Float(f) => out.push_str(&format_float(*f)),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Empty => out.push_str("{}"),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                emit_inline(item, out);
            }
            out.push(']');
        }
        Value::Table(map) => {
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format_key(k));
                out.push_str(" = ");
                emit_inline(v, out);
            }
            out.push('}');
        }
    }
}

/// Floats must keep their `.` or `e` so that they re-parse as `Float` and not `Int`, and the
/// non-finite spellings are the lowercase ones this module accepts.
fn format_float(f: f64) -> String {
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf".to_string() } else { "-inf".to_string() };
    }
    let rendered = format!("{}", f);
    if rendered.contains('.') || rendered.contains('e') || rendered.contains('E') {
        rendered
    } else {
        format!("{}.0", rendered)
    }
}

/// Bare key when possible (`W-0070`, `0x71`, `a_track_c`), quoted otherwise (`"github.com/x"`).
fn format_key(k: &str) -> String {
    if !k.is_empty() && k.chars().all(is_bare_key_char) {
        k.to_string()
    } else {
        let mut s = String::new();
        emit_basic_string(k, &mut s);
        s
    }
}

fn emit_basic_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{000C}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || c == '\u{007F}' => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §5.2's normative `MODULE.infy.toml` sample, copied verbatim — including its `(* … *)`
    /// annotations, which is exactly why extension 2 exists.
    const MODULE_5_2: &str = r##"[module]
path    = "github.com/user/infinity"   (* 必填。必须可解析：https 直取 / 重定向元标签 / git *)
version = "1.2.3"                      (* 发布时必填；开发期可为 "0.0.0-dev" *)
infy    = "2.0"                        (* 语言版本，决定 §1.2 兼容判定 *)
authors = ["Alice <a@b.com>"]

[dependencies]
"std"                        = "1.0"   (* 内置标准库 = 强制依赖，不可撤销 *)
"github.com/user/mathx"      = "1.5.0" (* 精确版本或 ^1.5 语义区间 *)
"github.com/user/decoder"    = "2.0.0"

[features]
gpu = []                               (* 可选特性，参与 §2.2 build 约束 *)
a_track_c = ["gpu"]                    (* 特性可传递依赖另一特性 *)

[profile]
overflow         = "panic"             (* panic | saturate（无 wrap，§4.4）*)
diagnostic_level = "strict"            (* strict | lenient *)
opt_level        = 2                   (* 0..3 | "z"，§7.2 *)
stack_bytes      = 8388608             (* 每链栈大小，§4.8 *)
a_track          = "unsafe-trusted"    (* 声明后才允许 @c/@cpp/@rust/@zig，§4.7.5 *)
reproducible     = true                (* 强制可复现构建，§7.3 *)

[b_track]                              (* B 轨字节码摄取所需的源语言工具链版本（§7.7.3）*)
py  = "3.13"                           (* 精确到 minor；使用 @py 而未声明 → E-0715 *)
lua = "5.4"

[[bin]]                                (* 可执行目标；缺省则本 module 为纯库 *)
name  = "zhukong"                      (* 产物文件名；可含非 ASCII，不受 R3 约束 *)
path  = "cmd/host"                     (* 必须是小写 ASCII 目录（R3）*)

[[lib]]                                (* 库目标：产出的 .infib 的 edition 在此声明 *)
path     = "core/schedule"
edition  = ["source", "wheel"]         (* 同时提供源码版与 wheel 版 *)
targets  = ["x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"]
"##;

    /// Appendix E.2 `INFIB-META/manifest.toml`, verbatim (`;` pairs in `[[deps]]`).
    const MANIFEST_E2: &str = r##"[lib]
path       = "github.com/user/mathx"
version    = "1.2.3"
edition    = "wheel"                    # source | wheel
ifabi      = 3
ifbc_caps  = ["CPY-313:1", "LUA-54:1"]  # 含 B 轨段时必填
[lib.profiles]                          # 该库编译时策略（链接期一致性检查依据）
overflow   = "panic"
repr_default = "rust"
fast_math  = false
[[code]]                                # 仅 wheel 版
target  = "x86_64-pc-windows-msvc"
compiler = "infyc 2.0.1 (llvm 18.1)"
abi_hash = "sha256:…"
[[deps]]
path = "github.com/user/base" ; version = "1.0" ; dirhash = "h1:…"
"##;

    /// Appendix E.3 capability table, verbatim.
    const CAP_E3: &str = r##"[cap]
lang = "cpython" ; format = "CPY-313" ; cap_version = 1 ; table_hash = "sha256:…"
[opcode]                       # §7.7.2 扫描项 1
0x71 = "add.i64"              # 已映射
0xb8 = "-"                     # 未映射 → E-0712
[builtin]                      # 扫描项 2
len = "std.iter.len" ; print = "std.io.log" ; eval = "-"
[import]                       # 扫描项 3
allow = ["math", "json", "itertools"]     # 纯字节码且已声明
deny  = ["numpy", "opencv", "pillow"]      # 原生扩展 → E-0712 + 出路提示
[exception]                    # 扫描项 5
entry = ["push", "setup", "with"]         # co_exceptiontable 支持条目类型
"##;

    /// Appendix E.4 `TOOLCHAIN.infy.toml`, verbatim, including `[net] ; proxy = "…"`.
    const TOOLCHAIN_E4: &str = r##"[[toolchain]]
kind = "cc" ; name = "msvc-cl" ; path = "…/cl.exe" ; version_str = "19.40" ; targets = ["x86_64-pc-windows-msvc"]
[[toolchain]]
kind = "py" ; name = "cpython" ; path = "…/python.exe" ; version_str = "3.13.1" ; bytecode = "CPY-313"
[[toolchain]]
kind = "deno" ; name = "deno" ; path = "…" ; version_str = "1.40"     # 仅 isolation="process"
[net] ; proxy = "…"; timeout_ms = 15000
"##;

    /// §9.6 `[lints]`, verbatim.
    const LINTS_9_6: &str = r##"[lints]
W-0070 = "deny"      # 枚举遗漏变体 → 升级为错误
W-0801 = "allow"     # 长循环无抢占点 → 关闭
E-0063 = "allow"     # 谨慎：允许 const_assert 失败降级（默认禁止此降级）
"##;

    fn doc(text: &str) -> Value {
        match parse(text) {
            Ok(v) => v,
            Err(e) => panic!("parse failed: {}", e),
        }
    }

    #[test]
    fn parses_module_manifest_from_5_2() {
        let m = doc(MODULE_5_2);
        assert_eq!(m.get_path("module.path").and_then(Value::as_str), Some("github.com/user/infinity"));
        assert_eq!(m.get_path("module.version").and_then(Value::as_str), Some("1.2.3"));
        assert_eq!(m.get_path("module.infy").and_then(Value::as_str), Some("2.0"));
        assert_eq!(
            m.get_path("module.authors").and_then(Value::as_array).map(|a| a.len()),
            Some(1)
        );
        // Quoted keys keep their dots inside one key, they are not dotted paths (§5.3).
        assert_eq!(m.get("dependencies").and_then(|d| d.get("github.com/user/mathx")).and_then(Value::as_str), Some("1.5.0"));
        assert_eq!(m.get_path("features.gpu").and_then(Value::as_array).map(|a| a.len()), Some(0));
        assert_eq!(
            m.get_path("features.a_track_c").and_then(Value::as_array).map(|a| a[0].as_str()),
            Some(Some("gpu"))
        );
        assert_eq!(m.get_path("profile.opt_level").and_then(Value::as_int), Some(2));
        assert_eq!(m.get_path("profile.stack_bytes").and_then(Value::as_int), Some(8_388_608));
        assert_eq!(m.get_path("profile.reproducible").and_then(Value::as_bool), Some(true));
        assert_eq!(m.get_path("profile.overflow").and_then(Value::as_str), Some("panic"));
        assert_eq!(m.get_path("b_track.py").and_then(Value::as_str), Some("3.13"));
        assert_eq!(m.get_path("b_track.lua").and_then(Value::as_str), Some("5.4"));
        let bins = m.get("bin").and_then(Value::as_array).expect("[[bin]] array");
        assert_eq!(bins.len(), 1);
        assert_eq!(bins[0].get("name").and_then(Value::as_str), Some("zhukong"));
        assert_eq!(bins[0].get("path").and_then(Value::as_str), Some("cmd/host"));
        let libs = m.get("lib").and_then(Value::as_array).expect("[[lib]] array");
        assert_eq!(
            libs[0].get("edition").and_then(Value::as_array).map(|a| a.len()),
            Some(2)
        );
        assert_eq!(
            libs[0].get("targets").and_then(Value::as_array).map(|a| a.len()),
            Some(2)
        );
    }

    #[test]
    fn parses_infib_manifest_from_e2() {
        let m = doc(MANIFEST_E2);
        assert_eq!(m.get_path("lib.ifabi").and_then(Value::as_int), Some(3));
        assert_eq!(m.get_path("lib.edition").and_then(Value::as_str), Some("wheel"));
        assert_eq!(
            m.get_path("lib.ifbc_caps").and_then(Value::as_array).map(|a| a.len()),
            Some(2)
        );
        assert_eq!(m.get_path("lib.profiles.fast_math").and_then(Value::as_bool), Some(false));
        assert_eq!(m.get_path("lib.profiles.overflow").and_then(Value::as_str), Some("panic"));
        let code = m.get("code").and_then(Value::as_array).expect("[[code]]");
        assert_eq!(code[0].get("target").and_then(Value::as_str), Some("x86_64-pc-windows-msvc"));
        assert_eq!(code[0].get("compiler").and_then(Value::as_str), Some("infyc 2.0.1 (llvm 18.1)"));
        // E.2 requires `ifabi` / `abi_hash` / `deps.dirhash` — the `E-0554` check reads these.
        assert!(code[0].get("abi_hash").and_then(Value::as_str).map(|s| s.starts_with("sha256:")) == Some(true));
        let deps = m.get("deps").and_then(Value::as_array).expect("[[deps]] via `;`");
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].get("path").and_then(Value::as_str), Some("github.com/user/base"));
        assert_eq!(deps[0].get("version").and_then(Value::as_str), Some("1.0"));
        assert!(deps[0].get("dirhash").and_then(Value::as_str).map(|s| s.starts_with("h1:")) == Some(true));
    }

    #[test]
    fn parses_capability_table_from_e3() {
        let c = doc(CAP_E3);
        assert_eq!(c.get_path("cap.lang").and_then(Value::as_str), Some("cpython"));
        assert_eq!(c.get_path("cap.format").and_then(Value::as_str), Some("CPY-313"));
        assert_eq!(c.get_path("cap.cap_version").and_then(Value::as_int), Some(1));
        // Bare keys that *look* like hex integers stay keys (§7.7.2 扫描项 1).
        assert_eq!(c.get("opcode").and_then(|o| o.get("0x71")).and_then(Value::as_str), Some("add.i64"));
        assert_eq!(c.get("opcode").and_then(|o| o.get("0xb8")).and_then(Value::as_str), Some("-"));
        assert_eq!(c.get_path("builtin.eval").and_then(Value::as_str), Some("-"));
        assert_eq!(
            c.get_path("import.allow").and_then(Value::as_array).map(|a| a.len()),
            Some(3)
        );
        assert_eq!(
            c.get_path("import.deny").and_then(Value::as_array).map(|a| a.len()),
            Some(3)
        );
        assert_eq!(
            c.get_path("exception.entry").and_then(Value::as_array).map(|a| a.len()),
            Some(3)
        );
    }

    #[test]
    fn parses_toolchain_registry_from_e4() {
        let t = doc(TOOLCHAIN_E4);
        let tools = t.get("toolchain").and_then(Value::as_array).expect("[[toolchain]]");
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].get("kind").and_then(Value::as_str), Some("cc"));
        assert_eq!(
            tools[0].get("targets").and_then(Value::as_array).map(|a| a.len()),
            Some(1)
        );
        assert_eq!(tools[1].get("bytecode").and_then(Value::as_str), Some("CPY-313"));
        // The third row declares no `targets` and no `bytecode`.
        assert!(tools[2].get("targets").is_none());
        assert!(tools[2].get("bytecode").is_none());
        // `[net] ; proxy = "…"` — a `;` right after a header (extension 1).
        assert_eq!(t.get_path("net.timeout_ms").and_then(Value::as_int), Some(15000));
        assert_eq!(t.get_path("net.proxy").and_then(Value::as_str), Some("…"));
    }

    #[test]
    fn parses_lints_from_9_6() {
        let l = doc(LINTS_9_6);
        assert_eq!(l.get("lints").and_then(|x| x.get("W-0070")).and_then(Value::as_str), Some("deny"));
        assert_eq!(l.get("lints").and_then(|x| x.get("W-0801")).and_then(Value::as_str), Some("allow"));
        assert_eq!(l.get("lints").and_then(|x| x.get("E-0063")).and_then(Value::as_str), Some("allow"));
    }

    /// §7.3: emit → re-parse must be value-identical, and the emitter must be a fixed point.
    #[test]
    fn every_spec_fixture_round_trips() {
        for (name, text) in [
            ("§5.2 MODULE.infy.toml", MODULE_5_2),
            ("E.2 manifest.toml", MANIFEST_E2),
            ("E.3 capability table", CAP_E3),
            ("E.4 TOOLCHAIN.infy.toml", TOOLCHAIN_E4),
            ("§9.6 [lints]", LINTS_9_6),
        ] {
            let v = parse(text).unwrap_or_else(|e| panic!("{}: {}", name, e));
            let emitted = to_toml(&v);
            let reparsed = parse(&emitted).unwrap_or_else(|e| panic!("{}: re-parse of emitted failed: {}\n{}", name, e, emitted));
            assert_eq!(v, reparsed, "{} did not round-trip\n--- emitted ---\n{}", name, emitted);
            assert_eq!(emitted, to_toml(&reparsed), "{} emitter is not stable", name);
        }
    }

    #[test]
    fn emitter_output_is_deterministic() {
        // Two documents that differ only in key order must emit identically (BTreeMap, §7.3).
        let a = doc("[p]\nz = 1\na = 2\n[t]\nk = \"v\"");
        let b = doc("[t]\nk = \"v\"\n[p]\na = 2\nz = 1");
        assert_eq!(to_toml(&a), to_toml(&b));
        assert_eq!(to_toml(&a), "[p]\na = 2\nz = 1\n\n[t]\nk = \"v\"\n");
    }

    #[test]
    fn handles_the_rest_of_the_subset() {
        let v = doc(concat!(
            "dotted.inner = 1\n",
            "hex = 0xFF\n",
            "octal = 0o755\n",
            "binary = 0b1010\n",
            "under = 1_000_000\n",
            "neg = -42\n",
            "float = 3.14\n",
            "exp = 1e3\n",
            "negfloat = -0.5\n",
            "infp = inf\n",
            "neginf = -inf\n",
            "notanumber = nan\n",
            "literal = 'C:\\tmp\\a'\n",
            "escaped = \"tab\\there\\nu=\\u4e2d\"\n",
            "multiline_array = [\n",
            "  1, # first\n",
            "  2 # second\n",
            "]\n",
            "nested = [[1, 2], [3]]\n",
            "inline = { a = 1, b.c = 2 }\n",
            "'quoted key' = 7\n",
            "\"dotted.quoted\" = 8\n",
            "empty_table = {}\n",
            "semi_a = 1 ; semi_b = 2\n",
            "block_a = 3 (* trailing block *)\n",
            "[deep.deeper]\n",
            "x = true\n",
        ));
        assert_eq!(v.get_path("dotted.inner").and_then(Value::as_int), Some(1));
        assert_eq!(v.get_path("hex").and_then(Value::as_int), Some(255));
        assert_eq!(v.get_path("octal").and_then(Value::as_int), Some(0o755));
        assert_eq!(v.get_path("binary").and_then(Value::as_int), Some(0b1010));
        assert_eq!(v.get_path("under").and_then(Value::as_int), Some(1_000_000));
        assert_eq!(v.get_path("neg").and_then(Value::as_int), Some(-42));
        assert_eq!(v.get_path("float").and_then(Value::as_float), Some(3.14));
        assert_eq!(v.get_path("exp").and_then(Value::as_float), Some(1e3));
        assert_eq!(v.get_path("negfloat").and_then(Value::as_float), Some(-0.5));
        assert_eq!(v.get_path("infp").and_then(Value::as_float), Some(f64::INFINITY));
        assert_eq!(v.get_path("neginf").and_then(Value::as_float), Some(f64::NEG_INFINITY));
        assert!(v.get_path("notanumber").and_then(Value::as_float).map(|f| f.is_nan()) == Some(true));
        assert_eq!(v.get_path("literal").and_then(Value::as_str), Some("C:\\tmp\\a"));
        assert_eq!(v.get_path("escaped").and_then(Value::as_str), Some("tab\there\nu=\u{4e2d}"));
        assert_eq!(v.get_path("multiline_array").and_then(Value::as_array).map(|a| a.len()), Some(2));
        assert_eq!(
            v.get_path("nested").and_then(Value::as_array).and_then(|a| a[0].as_array()).map(|a| a.len()),
            Some(2)
        );
        assert_eq!(v.get_path("inline.b.c").and_then(Value::as_int), Some(2));
        assert_eq!(v.get("quoted key").and_then(Value::as_int), Some(7));
        assert_eq!(v.get("\"dotted.quoted\"").and_then(Value::as_int), None);
        assert_eq!(v.get("dotted.quoted").and_then(Value::as_int), Some(8));
        assert!(v.get_path("empty_table").map(Value::is_empty_table) == Some(true));
        assert_eq!(v.get_path("deep.deeper.x").and_then(Value::as_bool), Some(true));
        assert_eq!(v.get_path("semi_b").and_then(Value::as_int), Some(2));
        assert_eq!(v.get_path("block_a").and_then(Value::as_int), Some(3));
        // i64 extremes.
        assert_eq!(parse("a = 9223372036854775807").unwrap().get_path("a").and_then(Value::as_int), Some(i64::MAX));
        assert_eq!(parse("a = -9223372036854775808").unwrap().get_path("a").and_then(Value::as_int), Some(i64::MIN));
    }

    /// Floats that happen to be integral must stay floats when emitted.
    #[test]
    fn emitter_keeps_floats_floats() {
        let v = doc("a = 1.0\nb = 2e3\nc = -0.0\nd = 7\n");
        let s = to_toml(&v);
        assert!(s.contains("a = 1.0"), "{}", s);
        assert!(s.contains("b = 2000.0"), "{}", s);
        assert!(s.contains("c = -0.0"), "{}", s);
        assert!(s.contains("d = 7\n"), "{}", s);
        assert_eq!(parse(&s).unwrap(), v);
        let inf = doc("a = inf\nb = -inf\nc = nan");
        let rendered = to_toml(&inf);
        assert!(rendered.contains("a = inf") && rendered.contains("b = -inf") && rendered.contains("c = nan"), "{}", rendered);
        // `+inf` is what a naive emitter would write, and what this module refuses to parse.
        assert!(parse("a = +inf").is_err());
    }

    #[test]
    fn rejects_out_of_subset_constructs_with_typed_errors() {
        let cases: &[(&str, &str)] = &[
            ("x = \"\"\"abc\"\"\"", "multi-line"),
            ("x = '''abc'''", "multi-line"),
            ("x = 1979-05-27T07:32:00Z", "date"),
            ("x = 1979-05-27", "date"),
            ("x = +inf", "+inf"),
            ("x = +nan", "+nan"),
            ("x = [1, 2,]", "trailing comma"),
            ("x = { a = 1,\n b = 2 }", "one line"),
            ("x = 1\nx = 2", "duplicate key"),
            ("[a]\n[a]", "defined twice"),
            ("x = [1]\n[[x]]", "static array"),
            ("a = 1\na.b = 2", "not a table"),
            ("a = { b = 1 }\na.c = 2", "inline table"),
            ("a = { b = 1, b = 2 }", "duplicate key"),
            ("x = 'unterminated", "unterminated string"),
            ("x = \"\\q\"", "not a TOML escape"),
            ("x = 1.2.3", "right after a value"),
            ("x = 07", "leading zero"),
            ("x = 0x", "at least one digit"),
            ("x = 1__2", "separate digits"),
            ("[a", "must close"),
            ("x &y = 1", "expected `=`"),
            ("x = { a = 1\n b = 2 }", "one line"),
            ("x = { a = 1", "unterminated inline table"),
            ("[[bin]]\n[bin.cfg]\nx = 1", "array of tables"),
            ("x = 9223372036854775808", "outside i64"),
            ("(* unterminated", "unterminated `(*`"),
            ("x = truex", "right after a value"),
        ];
        for (text, expect) in cases {
            let e = parse(text).unwrap_err();
            assert!(
                e.message().contains(expect),
                "input {:?}: expected message to contain {:?}, got {:?}",
                text,
                expect,
                e
            );
            assert!(e.line() >= 1 && e.column() >= 1, "input {:?}: {:?}", text, e);
        }
    }

    /// §9.2 positions: 1-based line, 1-based code-point column.
    #[test]
    fn errors_carry_line_and_column() {
        let e = parse("a = 1\nb = 2\n  c x = 3\n").unwrap_err();
        assert_eq!((e.line(), e.column()), (3, 5), "{}", e);
        let e = parse("# 注释\n[lints]\nW-0070 = deny\n").unwrap_err();
        assert_eq!((e.line(), e.column()), (3, 10), "{}", e);
        let e = parse("x = \"abc\n").unwrap_err();
        assert_eq!(e.line(), 1, "{}", e);
        // A CRLF file reports the same columns as the LF form.
        let e_lf = parse("x = y\n").unwrap_err();
        let e_crlf = parse("x = y\r\n").unwrap_err();
        assert_eq!((e_lf.line(), e_lf.column()), (e_crlf.line(), e_crlf.column()));
        assert!(format!("{}", e_lf).starts_with("syntax at 1:5"), "{}", e_lf);
    }

    #[test]
    fn empty_and_bom_inputs() {
        assert!(parse("").unwrap().is_empty_table());
        assert!(parse("# only a comment\n\n").unwrap().is_empty_table());
        assert_eq!(parse("\u{FEFF}[a]\nb = 1\n").unwrap(), parse("[a]\nb = 1\n").unwrap());
        // A document that starts with key/values before any header is legal (§5.2 root keys).
        assert_eq!(parse("x = 1").unwrap().get_path("x").and_then(Value::as_int), Some(1));
    }

    /// Two `[[t]]` elements must not shadow each other's inline tables (the ledger keys elements
    /// by index, see `descend`).
    #[test]
    fn array_of_tables_elements_are_independent() {
        let v = parse(concat!(
            "[[bin]]\n",
            "cfg = { a = 1 }\n",
            "[[bin]]\n",
            "cfg = { b = 2 }\n",
            "cfg2 = 3\n",
        ))
        .unwrap();
        let bins = v.get("bin").and_then(Value::as_array).unwrap();
        assert_eq!(bins.len(), 2);
        assert_eq!(bins[0].get_path("cfg.a").and_then(Value::as_int), Some(1));
        assert_eq!(bins[1].get_path("cfg.b").and_then(Value::as_int), Some(2));
        assert_eq!(bins[1].get_path("cfg2").and_then(Value::as_int), Some(3));
        assert_eq!(parse(&to_toml(&v)).unwrap(), v);
    }

    /// Deterministic corpus sweep (§7.3 — a seeded xorshift, never system entropy): every generated
    /// document either parses or yields a typed error, and every parsed document survives
    /// `to_toml` → `parse` unchanged. This is the guard on the `unwrap()`s inside the ledger code.
    #[test]
    fn fuzz_never_panics_and_round_trips() {
        const TOKENS: &[&str] = &[
            "a", "b", "1", "-2", "0", "3.5", "1e3", "0x1f", "true", "false", "\"s\"", "'l'", "[", "]",
            "{", "}", "=", ".", ",", ";", " ", "\t", "\n", "#c\n", "(*c*)", "(", ")", "\\", "\"", "'",
            "[[", "]]", " \n[x]\n", "y.z",
            // Whole statements, so the accept path (and the ledger) gets real coverage.
            "k = 1\n", "[t]\n", "[[u]]\n", "j = { x = 1 }\n", "m = [1, 2]\n", "p = \"s\" ; q = 2\n",
            "[t.r]\n", "s.t = 3\n", "z = {}\n",
        ];
        let mut state = 0x243F_6A88_85A3_08D3u64;
        let mut next = move || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state = state.wrapping_mul(0x94D0_49BB_1331_11EB);
            state
        };
        let mut parsed_ok = 0usize;
        for case in 0..30_000usize {
            let n = 1 + (next() % 14) as usize;
            let mut text = String::new();
            for _ in 0..n {
                let idx = (next() % TOKENS.len() as u64) as usize;
                text.push_str(TOKENS[idx]);
            }
            let v = match parse(&text) {
                Ok(v) => v,
                Err(e) => {
                    // An error must always be renderable with a usable position (§9.2).
                    assert!(e.line() >= 1 && e.column() >= 1, "{}", e);
                    let _ = format!("{}", e);
                    continue;
                }
            };
            parsed_ok += 1;
            let emitted = to_toml(&v);
            let again = parse(&emitted)
                .unwrap_or_else(|e| panic!("case {}: {:?} -> {:?} -> re-parse {} failed: {}", case, text, v, emitted, e));
            assert_eq!(v, again, "case {} text {:?} emitted {:?}", case, text, emitted);
            assert_eq!(emitted, to_toml(&again), "case {}: emitter unstable", case);
        }
        // The corpus has to actually reach the accept path, or the test proves nothing. With this
        // token set roughly 2 % of the draws are valid documents (565/30000 as of writing).
        assert!(parsed_ok > 200, "only {} accepted documents", parsed_ok);
    }

    /// `Empty` is a caller-side placeholder; the emitter has to spell it as `{}` and that is the
    /// one documented non-round-trip (see [`Value::Empty`]).
    #[test]
    fn empty_placeholder_emits_as_empty_inline_table() {
        let mut map = BTreeMap::new();
        map.insert("k".to_string(), Value::Empty);
        let s = to_toml(&Value::Table(map));
        assert_eq!(s, "k = {}\n");
        assert_eq!(parse(&s).unwrap().get_path("k").map(Value::is_empty_table), Some(true));
    }
}
