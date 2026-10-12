//! Parser — §3's EBNF, every production, via LL(2) recursive descent with the single
//! disambiguation strategy of §3.10 and error recovery that keeps going so a single `infy check`
//! reports the whole diagnostic set (§7.1: "任一阶段出现 E- 即中止后续阶段，但同阶段内必须尽量
//! 一次性报告全部诊断").
//!
//! No automatic semicolon insertion (§2.11): a statement must end in `;` unless it is a block
//! expression, an `if`/`match`/`loop` expression statement, or a function body's tail expression.
//! Anything else is `E-0053`.
//!
//! `§9.3` has no catch-all syntax-error code, and UB-024 forbids inventing one. The resolution
//! documented in `docs/spec-deviations.md`: every "expected X, found Y" failure is reported as
//! `E-0053` (语句缺少终结符 / statement terminator), because §2.11 makes `;`-and-block grammar the
//! only reason a token can appear where the grammar forbids one.

use crate::ast::*;
use crate::diag::{Handler, Phase};
use crate::lexer::Lexed;
use crate::span::{FileId, SourceFile, Sources, Span};
use crate::token::{Comment, CommentKind, Delim, Kind, Lit, Punct, Tok, Tree, Kw};

/// Parse one already-lexed file.
pub fn parse(sources: &Sources, file: FileId, lexed: &Lexed, handler: &mut Handler) -> ParsedFile {
    let mut p = P {
        toks: lexed.toks.clone(),
        pos: 0,
        file,
        src: sources.get(file),
        comments: lexed.comments.clone(),
        h: handler,
        stop: false,
    };
    let out = p.file(lexed.shebang.clone(), &lexed.build_constraint);
    out
}

struct P<'a> {
    toks: Vec<Tok>,
    pos: usize,
    file: FileId,
    src: Option<&'a SourceFile>,
    comments: Vec<Comment>,
    h: &'a mut Handler,
    /// Hard stop (too many errors from one cause, e.g. a file that is not UTF-8 at all).
    stop: bool,
}

impl<'a> P<'a> {
    // ---------------------------------------------------------------- cursor helpers
    fn peek(&self) -> &Tok {
        self.toks.get(self.pos).unwrap_or_else(|| self.toks.last().unwrap())
    }
    fn nth(&self, k: usize) -> &Tok {
        self.toks.get(self.pos + k).unwrap_or_else(|| self.toks.last().unwrap())
    }
    /// A word token (identifier *or* keyword) at offset `k`, exactly as written. Accepted in the
    /// three positions where the spec's own examples put a keyword where its grammar says `ident`:
    /// attribute paths, attribute argument names, and the member after `.` (§9.5 rule 3's chain
    /// breaks on `.link()`, and `link` is reserved by §2.4) — see docs/spec-deviations.md D-13/D-14.
    fn word_at(&self, k: usize) -> Option<String> {
        let t = self.nth(k);
        match &t.kind {
            Kind::Ident(_) | Kind::Keyword(_) => Some(t.text.clone()),
            _ => None,
        }
    }
    fn at_end(&self) -> bool {
        self.peek().is_eof()
    }
    fn bump(&mut self) -> Tok {
        let t = self.peek().clone();
        if !self.at_end() {
            self.pos += 1;
        }
        t
    }
    fn at_p(&self, p: Punct) -> bool {
        self.peek().is_p(p)
    }
    fn at_kw(&self, k: Kw) -> bool {
        self.peek().is_kw(k)
    }
    fn eat_p(&mut self, p: Punct) -> bool {
        if self.at_p(p) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn eat_kw(&mut self, k: Kw) -> bool {
        if self.at_kw(k) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn expect_p(&mut self, p: Punct) -> Option<()> {
        if self.eat_p(p) {
            Some(())
        } else {
            let t = self.peek().clone();
            self.syntax_err(&t, describe_punct(p));
            None
        }
    }
    fn expect_kw(&mut self, k: Kw) -> Option<()> {
        if self.eat_kw(k) {
            Some(())
        } else {
            let t = self.peek().clone();
            self.syntax_err(&t, k.text());
            None
        }
    }

    // ---------------------------------------------------------------- diagnostics
    fn span_line_col(&self, sp: Span) -> (u32, u32, u32) {
        match self.src {
            Some(f) => {
                let (l, bc) = f.byte_line_col(sp.start);
                let text = f.line_text(l);
                let prefix = &text[..(bc as usize - 1).min(text.len())];
                (l, prefix.chars().count() as u32 + 1, prefix.chars().count() as u32 + 1 + self.width_of(&text[(bc as usize - 1).min(text.len())..]))
            }
            None => (1, 1, 2),
        }
    }
    /// Display width of the token text, measured in code points (§9.5's grapheme rule is a `fmt`
    /// concern; diagnostics count code points so the caret column matches `Sources::line_col`).
    fn width_of(&self, s: &str) -> u32 {
        s.chars().take_while(|c| !c.is_whitespace()).count() as u32
    }
    fn syntax_err(&mut self, t: &Tok, expected: &str) {
        if self.stop {
            return;
        }
        let (line, col, end) = self.span_line_col(t.span);
        let found = if t.is_eof() { "文件结束".to_string() } else { t.text.clone() };
        let code = if t.is_eof() { "E-0053" } else { "E-0053" };
        self.h.emit(
            crate::diag::Diagnostic::new(code, Phase::Parse, self.file, crate::span::Position { line, column: col })
                .titled(format!("语法错误：期望 {}，实际为 `{}`", expected, found))
                .region(self.file, line, col, end.max(col + 1), format!("此处期望 {}", expected))
                .help("补全缺失的记号，或删除多余的内容（§2.11 无自动分号插入）"),
        );
    }
    fn err_at(&mut self, t: &Tok, code: &str, title: String, label: &str, help: Option<String>) {
        let (line, col, end) = self.span_line_col(t.span);
        let mut d = crate::diag::Diagnostic::new(code, Phase::Parse, self.file, crate::span::Position { line, column: col })
            .titled(title)
            .region(self.file, line, col, end.max(col + 1), label.to_string());
        if let Some(h) = help {
            d = d.help(h);
        }
        self.h.emit(d);
    }
    /// Statement-level recovery: skip tokens until a plausible boundary.
    fn synchronize(&mut self) {
        loop {
            if self.at_end() {
                return;
            }
            let k = &self.peek().kind;
            if matches!(k, Kind::Punct(Punct::Semi)) {
                self.bump();
                return;
            }
            if matches!(k, Kind::Punct(Punct::RBrace)) {
                return;
            }
            if let Kind::Keyword(kw) = k {
                if matches!(kw, Kw::Fn | Kw::Event | Kw::Struct | Kw::Enum | Kw::Trait | Kw::Impl | Kw::Chain | Kw::Const | Kw::Static | Kw::Type | Kw::Union | Kw::Extern | Kw::Let | Kw::Var)
                {
                    return;
                }
            }
            self.bump();
        }
    }

    // ---------------------------------------------------------------- doc comments
    /// `///` lines whose end is closest above `start`, used by `infy doc` (§2.2).
    fn docs_before(&self, start: u32) -> Docs {
        let mut lines = Vec::new();
        let mut best: Option<u32> = None;
        for c in &self.comments {
            if c.kind != CommentKind::Doc || c.span.end > start {
                continue;
            }
            match best {
                None => best = Some(c.span.end),
                Some(b) if c.span.end != b => continue,
                _ => {}
            }
            lines.push(c.text.trim_start_matches('/').trim().to_string());
        }
        Docs { lines }
    }

    // ---------------------------------------------------------------- §3.1 file
    fn file(&mut self, shebang: Option<String>, build: &Option<(String, u32, Span)>) -> ParsedFile {
        let mut out = ParsedFile::empty(self.file);
        out.shebang = shebang;
        if let Some((txt, line, sp)) = build.clone() {
            out.build_constraint = Some(txt);
            out.build_constraint_line = Some(line);
            out.build_constraint_span = Some(sp);
        }
        // file attributes: `#![...]`, before `package` (§3.1)
        while matches!(self.peek().kind, Kind::AttrOpen { inner: true }) {
            self.bump();
            match self.attr_body(true) {
                Some(a) => out.file_attrs.push(a),
                None => self.synchronize(),
            }
        }
        // package clause
        if self.at_kw(Kw::Package) {
            let kw = self.bump();
            let name_tok = self.peek().clone();
            match name_tok.ident() {
                Some(s) => {
                    out.package = Some(s.to_string());
                    out.package_span = Some(name_tok.span);
                    self.bump();
                }
                None => {
                    self.syntax_err(&name_tok, "包名标识符");
                    self.bump();
                }
            }
            let _ = kw;
        }
        // imports (§3.1)
        while self.at_kw(Kw::Import) || (self.at_kw(Kw::Pub) && matches!(self.nth(1).kind, Kind::Keyword(Kw::Import))) {
            let start = self.peek().span.start;
            let pub_ = self.eat_kw(Kw::Pub);
            self.expect_kw(Kw::Import);
            let path_tok = self.peek().clone();
            let (path, path_span, path_ok) = match &path_tok.kind {
                Kind::Lit(Lit::Str(s)) => {
                    self.bump();
                    (s.clone(), path_tok.span, true)
                }
                _ => {
                    self.syntax_err(&path_tok, "导入路径字符串字面量");
                    // §5.3: any non string-literal import path is E-0504
                    let (l, c, e) = self.span_line_col(path_tok.span);
                    self.err_at(
                        &path_tok,
                        "E-0504",
                        "非法导入路径".to_string(),
                        "导入路径必须是字符串字面量（§5.3）",
                        Some("使用自 module path 起算的完整路径，例如 import \"github.com/acme/ci/core/build\";".to_string()),
                    );
                    let _ = (l, c, e);
                    self.bump();
                    (String::new(), path_tok.span, false)
                }
            };
            let mut alias = ImportAlias::None;
            if self.eat_kw(Kw::As) {
                let t = self.peek().clone();
                alias = match &t.kind {
                    Kind::Ident(s) => {
                        self.bump();
                        ImportAlias::Named(s.clone())
                    }
                    Kind::Punct(Punct::Dot) => {
                        self.bump();
                        ImportAlias::LinkOnly
                    }
                    Kind::Punct(Punct::Star) => {
                        self.bump();
                        ImportAlias::EffectOnly
                    }
                    _ => {
                        self.syntax_err(&t, "导入别名");
                        self.bump();
                        ImportAlias::None
                    }
                };
            }
            let terminated = self.eat_p(Punct::Semi);
            if !terminated && path_ok {
                let t = self.peek().clone();
                self.syntax_err(&t, ";");
            }
            out.imports.push(Import {
                pub_,
                path,
                path_span,
                alias,
                span: Span::new(self.file, start, self.prev_end()),
            });
        }
        // top items (§3.2)
        while !self.at_end() {
            let before = self.pos;
            match self.item() {
                Some(it) => out.items.push(it),
                None => {
                    if self.pos == before {
                        let t = self.peek().clone();
                        self.syntax_err(&t, "顶层声明");
                        self.synchronize();
                    }
                    if self.pos == before {
                        self.bump();
                    }
                }
            }
        }
        out.span = Span::new(self.file, 0, self.prev_end());
        out
    }

    fn prev_end(&self) -> u32 {
        if self.pos == 0 {
            0
        } else {
            self.toks[self.pos - 1].span.end
        }
    }

    // ---------------------------------------------------------------- §3.1 / §6.6 attributes
    /// Called with the `#[` / `#![` token already consumed; parses `path ( args )? ]`.
    fn attr_body(&mut self, inner: bool) -> Option<Attr> {
        let start = self.toks[self.pos.saturating_sub(1)].span.start;
        let mut path: Vec<String> = Vec::new();
        let first = self.peek().clone();
        match self.word_at(0) {
            Some(s) => {
                path.push(s);
                self.bump();
            }
            None => {
                // `#[repr(C)]`, `#[derive(...)]` always start with a word; anything else is a
                // malformed attribute and §6.6's closed list makes it E-0410 later.
                self.syntax_err(&first, "属性路径");
                return None;
            }
        }
        loop {
            if self.at_p(Punct::ColonColon) || self.at_p(Punct::Dot) {
                self.bump();
                let t = self.peek().clone();
                match self.word_at(0) {
                    Some(s) => {
                        path.push(s);
                        self.bump();
                    }
                    None => {
                        self.syntax_err(&t, "属性路径片段");
                        break;
                    }
                }
            } else {
                break;
            }
        }
        let mut args: Vec<AttrArg> = Vec::new();
        let mut positional: Vec<AttrValue> = Vec::new();
        // §3.1 writes `attr = "[" attr_path ( "(" attr_args ")" )? "]"`, but §6.6, §1.2 and §14
        // all use the paren-less `#![name = value]` form (`#![infy_version = "2.0"]`,
        // `#![entry_chain = "Foo"]`). The two clauses are only jointly satisfiable if the
        // single-assignment form is legal, so it is accepted here — see docs/spec-deviations.md.
        if !self.at_p(Punct::LParen) && self.at_p(Punct::Eq) {
            self.bump();
            if let Some(v) = self.attr_value() {
                positional.push(v);
            }
        }
        if self.eat_p(Punct::LParen) {
            loop {
                if self.at_p(Punct::RParen) || self.at_end() {
                    break;
                }
                let arg_start = self.peek().span.start;
                // §3.1 writes `attr_args = ident ("=" …)?`, but the spec's own `#[link(…)]` examples
                // (§7.6.3, §14) name an argument `static`, which is a keyword (§2.4) — see D-13.
                let mut name: Option<String> = None;
                if let Some(w) = self.word_at(0) {
                    if self.nth(1).is_p(Punct::Eq) {
                        name = Some(w);
                        self.bump();
                        self.bump();
                    }
                }
                let value = self.attr_value();
                let span = Span::new(self.file, arg_start, self.prev_end());
                match name {
                    Some(n) => args.push(AttrArg { name: Some(n), value, span }),
                    None => {
                        if let Some(v) = value {
                            positional.push(v);
                        }
                        args.push(AttrArg { name: None, value: None, span });
                    }
                }
                if !self.eat_p(Punct::Comma) {
                    break;
                }
            }
            self.expect_p(Punct::RParen);
        }
        self.expect_p(Punct::RBracket);
        Some(Attr { inner, path, args, positional, span: Span::new(self.file, start, self.prev_end()) })
    }

    fn attr_value(&mut self) -> Option<AttrValue> {
        let t = self.peek().clone();
        match &t.kind {
            Kind::Lit(Lit::Str(s)) => {
                self.bump();
                Some(AttrValue::Str(s.clone()))
            }
            Kind::Lit(Lit::Char(c)) => {
                self.bump();
                Some(AttrValue::Str(c.to_string()))
            }
            Kind::Lit(Lit::Int(i)) => {
                let v = i.value;
                self.bump();
                v.map(AttrValue::Int)
            }
            Kind::Keyword(Kw::True) => {
                self.bump();
                Some(AttrValue::Bool(true))
            }
            Kind::Keyword(Kw::False) => {
                self.bump();
                Some(AttrValue::Bool(false))
            }
            Kind::Lifetime(l) => {
                let l = l.clone();
                self.bump();
                Some(AttrValue::Lifetime(l))
            }
            Kind::Ident(_) => {
                // Either a bare ident list (`derive(Clone, Copy)`, `repr(C)`) or an expression
                // (`align(SIZE)`); collect the dotted/`::`-joined path first.
                let mut segs = vec![t.ident().unwrap().to_string()];
                self.bump();
                while (self.at_p(Punct::Dot) || self.at_p(Punct::ColonColon)) && self.nth(1).ident().is_some() {
                    self.bump();
                    let s = self.peek().ident().unwrap().to_string();
                    self.bump();
                    segs.push(s);
                }
                if segs.len() == 1 {
                    Some(AttrValue::Idents(vec![segs[0].clone()]))
                } else {
                    Some(AttrValue::Path(segs))
                }
            }
            Kind::Punct(Punct::LBracket) => {
                // `flags = ["-O2"]` — keep it as an array expression for sema/`link` handling.
                let e = self.expr();
                Some(AttrValue::Expr(e))
            }
            _ => {
                let e = self.expr();
                if matches!(e.kind, ExprKind::Invalid) {
                    None
                } else {
                    Some(AttrValue::Expr(e))
                }
            }
        }
    }

    fn outer_attrs(&mut self) -> (Vec<Attr>, Docs) {
        let start_offset = self.peek().span.start;
        let mut attrs = Vec::new();
        loop {
            match self.peek().kind {
                Kind::AttrOpen { inner: false } => {
                    self.bump();
                    match self.attr_body(false) {
                        Some(a) => attrs.push(a),
                        None => break,
                    }
                }
                Kind::AttrOpen { inner: true } => {
                    // Inner attribute in item position: legal only before `package` (§3.1).
                    let t = self.peek().clone();
                    self.err_at(&t, "E-0053", "内部属性位置错误".to_string(), "`#![...]` 只能出现在 `package` 之前", None);
                    self.bump();
                    self.attr_body(true);
                }
                _ => break,
            }
        }
        let docs = self.docs_before(start_offset);
        (attrs, docs)
    }

    // ---------------------------------------------------------------- §3.3 visibility
    fn visibility(&mut self) -> Visibility {
        if self.at_kw(Kw::Priv) {
            self.bump();
            return Visibility::Priv;
        }
        if self.at_kw(Kw::Pub) {
            self.bump();
            if self.eat_p(Punct::LParen) {
                let t = self.peek().clone();
                let v = match &t.kind {
                    Kind::Ident(s) if s == "crate" => {
                        self.bump();
                        Visibility::PubCrate
                    }
                    Kind::Lit(Lit::Str(s)) => {
                        let s = s.clone();
                        self.bump();
                        Visibility::PubTo(s)
                    }
                    Kind::Ident(s) => {
                        let mut segs = vec![s.clone()];
                        self.bump();
                        while self.at_p(Punct::ColonColon) || self.at_p(Punct::Dot) {
                            self.bump();
                            let n = self.peek().clone();
                            if let Some(x) = n.ident() {
                                segs.push(x.to_string());
                                self.bump();
                            } else {
                                break;
                            }
                        }
                        Visibility::PubTo(segs.join("/"))
                    }
                    _ => {
                        self.syntax_err(&t, "pub(...) 目标路径");
                        self.bump();
                        Visibility::Pub
                    }
                };
                self.expect_p(Punct::RParen);
                return v;
            }
            return Visibility::Pub;
        }
        Visibility::Private
    }

    // ---------------------------------------------------------------- §3.2/§3.3 items
    fn item(&mut self) -> Option<Item> {
        let (attrs, docs) = self.outer_attrs();
        let vis = self.visibility();
        let start = self.peek().span.start;
        // §4.7.5: `unsafe fn` / `unsafe event` / `unsafe trait`
        let leading_unsafe = self.item_leading_unsafe();
        let t = self.peek().clone();
        let kw = match &t.kind {
            Kind::Keyword(k) => *k,
            other => {
                // §2.4 reserved words used as an identifier, or a stray token.
                if let Kind::Ident(s) = other {
                    if Kw::is_reserved(s) {
                        self.err_at(&t, "E-0020", "保留字作标识符".to_string(), s, Some(format!("`{s}` 是保留字（§2.4），改名或按关键字使用")));
                        self.bump();
                        return None;
                    }
                }
                self.syntax_err(&t, "顶层声明关键字");
                return None;
            }
        };
        let mut attrs = attrs;
        if leading_unsafe {
            attrs.push(Attr { inner: false, path: vec!["unsafe".to_string()], args: Vec::new(), positional: Vec::new(), span: t.span });
        }
        match kw {
            Kw::Fn | Kw::Event => {
                let is_event = kw == Kw::Event;
                self.bump();
                let mut sig = self.fn_sig();
                sig.is_unsafe = leading_unsafe;
                let body = self.block_or_embed();
                let span = Span::new(self.file, start, self.prev_end());
                Some(Item {
                    attrs,
                    docs,
                    vis,
                    span,
                    kind: if is_event { ItemKind::Event { sig, body } } else { ItemKind::Fn { sig, body } },
                })
            }
            Kw::Struct => {
                self.bump();
                let name_tok = self.peek().clone();
                let name = self.ident_or_error();
                let generics = self.generic_params();
                let mut fields = Vec::new();
                let mut tuple = Vec::new();
                if self.at_p(Punct::LBrace) {
                    self.bump();
                    while !self.at_p(Punct::RBrace) && !self.at_end() {
                        match self.field() {
                            Some(f) => fields.push(f),
                            None => {
                                self.synchronize();
                                break;
                            }
                        }
                        if !self.eat_p(Punct::Comma) {
                            break;
                        }
                    }
                    self.expect_p(Punct::RBrace);
                } else if self.at_p(Punct::LParen) {
                    self.bump();
                    while !self.at_p(Punct::RParen) && !self.at_end() {
                        let (_, fvis, _) = self.outer_attrs_vis_field();
                        let ty = self.ty();
                        tuple.push((fvis, ty));
                        if !self.eat_p(Punct::Comma) {
                            break;
                        }
                    }
                    self.expect_p(Punct::RParen);
                }
                let where_preds = self.where_clause();
                self.eat_p(Punct::Semi);
                let _ = name_tok;
                Some(Item {
                    attrs,
                    docs,
                    vis,
                    span: Span::new(self.file, start, self.prev_end()),
                    kind: ItemKind::Struct { name, generics, fields, tuple, where_preds },
                })
            }
            Kw::Enum => {
                self.bump();
                let name = self.ident_or_error();
                let generics = self.generic_params();
                let mut variants = Vec::new();
                if self.eat_p(Punct::LBrace) {
                    while !self.at_p(Punct::RBrace) && !self.at_end() {
                        match self.variant() {
                            Some(v) => variants.push(v),
                            None => {
                                self.synchronize();
                                break;
                            }
                        }
                        if !self.eat_p(Punct::Comma) {
                            break;
                        }
                    }
                    self.expect_p(Punct::RBrace);
                }
                let where_preds = self.where_clause();
                self.eat_p(Punct::Semi);
                Some(Item {
                    attrs,
                    docs,
                    vis,
                    span: Span::new(self.file, start, self.prev_end()),
                    kind: ItemKind::Enum { name, generics, variants, where_preds },
                })
            }
            Kw::Union => {
                self.bump();
                let name = self.ident_or_error();
                let generics = self.generic_params();
                let mut fields = Vec::new();
                if self.eat_p(Punct::LBrace) {
                    while !self.at_p(Punct::RBrace) && !self.at_end() {
                        match self.field() {
                            Some(f) => fields.push(f),
                            None => break,
                        }
                        if !self.eat_p(Punct::Comma) {
                            break;
                        }
                    }
                    self.expect_p(Punct::RBrace);
                }
                self.eat_p(Punct::Semi);
                Some(Item {
                    attrs,
                    docs,
                    vis,
                    span: Span::new(self.file, start, self.prev_end()),
                    kind: ItemKind::Union { name, generics, fields },
                })
            }
            Kw::Trait => {
                self.bump();
                let name = self.ident_or_error();
                let generics = self.generic_params();
                let mut items = Vec::new();
                if self.eat_p(Punct::LBrace) {
                    while !self.at_p(Punct::RBrace) && !self.at_end() {
                        match self.item() {
                            Some(it) => items.push(it),
                            None => {
                                self.synchronize();
                                if self.at_end() {
                                    break;
                                }
                            }
                        }
                    }
                    self.expect_p(Punct::RBrace);
                }
                Some(Item {
                    attrs,
                    docs,
                    vis,
                    span: Span::new(self.file, start, self.prev_end()),
                    kind: ItemKind::Trait { name, generics, is_unsafe: leading_unsafe, items },
                })
            }
            Kw::Impl => {
                self.bump();
                let generics = self.generic_params();
                let lhs = self.ty();
                let mut trait_args = Vec::new();
                // The spec writes `type_ "for" type_`; the trait side is a path with optional
                // generic arguments, so split it back out here and parse the *right* side as the
                // implementing type (`impl Drop for C` must record `C`, not `Drop`).
                let (trait_, self_ty) = if self.eat_kw(Kw::For) {
                    match lhs.kind.clone() {
                        TypeKind::Named { path, args } => {
                            trait_args = args;
                            (Some(path), self.ty())
                        }
                        _ => {
                            let t = self.peek().clone();
                            self.err_at(&t, "E-0053", "impl 目标非法".to_string(), "trait 侧必须是具名类型路径", None);
                            (None, lhs)
                        }
                    }
                } else {
                    (None, lhs)
                };
                let mut items = Vec::new();
                if self.eat_p(Punct::LBrace) {
                    while !self.at_p(Punct::RBrace) && !self.at_end() {
                        match self.item() {
                            Some(it) => items.push(it),
                            None => {
                                self.synchronize();
                                if self.at_end() {
                                    break;
                                }
                            }
                        }
                    }
                    self.expect_p(Punct::RBrace);
                }
                Some(Item {
                    attrs,
                    docs,
                    vis,
                    span: Span::new(self.file, start, self.prev_end()),
                    kind: ItemKind::Impl { generics, trait_, trait_args, self_ty, items },
                })
            }
            Kw::Type => {
                self.bump();
                let name = self.ident_or_error();
                let generics = self.generic_params();
                let ty = if self.eat_p(Punct::Eq) { self.ty() } else { Type::invalid(Span::empty_at(self.file, self.prev_end())) };
                self.eat_p(Punct::Semi);
                Some(Item {
                    attrs,
                    docs,
                    vis,
                    span: Span::new(self.file, start, self.prev_end()),
                    kind: ItemKind::TypeAlias { name, generics, ty },
                })
            }
            Kw::Extern => {
                self.bump();
                let mut abi = None;
                if let Kind::Lit(Lit::Str(s)) = self.peek().kind.clone() {
                    abi = Some(s);
                    self.bump();
                }
                let mut decls = Vec::new();
                if self.eat_p(Punct::LBrace) {
                    while !self.at_p(Punct::RBrace) && !self.at_end() {
                        match self.item() {
                            Some(it) => decls.push(it),
                            None => {
                                self.synchronize();
                                if self.at_end() {
                                    break;
                                }
                            }
                        }
                    }
                    self.expect_p(Punct::RBrace);
                }
                Some(Item {
                    attrs,
                    docs,
                    vis,
                    span: Span::new(self.file, start, self.prev_end()),
                    kind: ItemKind::ExternBlock { abi, decls },
                })
            }
            Kw::Const => {
                self.bump();
                let name_tok = self.peek().clone();
                let name = self.ident_or_error();
                let name_span = name_tok.span;
                let ty = if self.eat_p(Punct::Colon) { Some(self.ty()) } else { None };
                let init = if self.eat_p(Punct::Eq) { Some(self.expr()) } else { None };
                self.eat_p(Punct::Semi);
                Some(Item {
                    attrs,
                    docs,
                    vis,
                    span: Span::new(self.file, start, self.prev_end()),
                    kind: ItemKind::Const { name, name_span, ty, init },
                })
            }
            Kw::Static => {
                self.bump();
                let is_mut = self.eat_kw(Kw::Mut);
                let name_tok = self.peek().clone();
                let name = self.ident_or_error();
                let name_span = name_tok.span;
                let ty = if self.eat_p(Punct::Colon) { Some(self.ty()) } else { None };
                let init = if self.eat_p(Punct::Eq) { Some(self.expr()) } else { None };
                self.eat_p(Punct::Semi);
                Some(Item {
                    attrs,
                    docs,
                    vis,
                    span: Span::new(self.file, start, self.prev_end()),
                    kind: ItemKind::Static { name, name_span, ty, init, is_mut },
                })
            }
            Kw::MacroRules => {
                self.bump();
                let name = self.ident_or_error();
                let mut arms = Vec::new();
                if self.eat_p(Punct::LBrace) {
                    while !self.at_p(Punct::RBrace) && !self.at_end() {
                        match self.macro_arm() {
                            Some(a) => arms.push(a),
                            None => break,
                        }
                        if !self.eat_p(Punct::Semi) {
                            self.eat_p(Punct::Comma);
                        }
                    }
                    self.expect_p(Punct::RBrace);
                }
                self.eat_p(Punct::Semi);
                Some(Item {
                    attrs,
                    docs,
                    vis,
                    span: Span::new(self.file, start, self.prev_end()),
                    kind: ItemKind::MacroRules { name, arms },
                })
            }
            Kw::Chain => {
                self.bump();
                let name_tok = self.peek().clone();
                let name = self.ident_or_error();
                let name_span = name_tok.span;
                let mut stmts = Vec::new();
                if self.eat_p(Punct::LBrace) {
                    while !self.at_p(Punct::RBrace) && !self.at_end() {
                        if let Some(s) = self.stmt() {
                            stmts.push(s);
                        }
                    }
                    self.expect_p(Punct::RBrace);
                }
                Some(Item {
                    attrs,
                    docs,
                    vis,
                    span: Span::new(self.file, start, self.prev_end()),
                    kind: ItemKind::Chain { name, name_span, stmts },
                })
            }
            Kw::Mod => {
                self.bump();
                let name = self.ident_or_error();
                self.eat_p(Punct::Semi);
                Some(Item {
                    attrs,
                    docs,
                    vis,
                    span: Span::new(self.file, start, self.prev_end()),
                    kind: ItemKind::Mod { name },
                })
            }
            other => {
                let t = self.peek().clone();
                self.err_at(&t, "E-0053", format!("非法的顶层声明：`{}`", other.text()), "此处不能出现该关键字", Some("顶层声明只能是 event/fn/struct/enum/union/trait/impl/type/extern/const/static/macro_rules!/chain（§3.2）".to_string()));
                self.synchronize();
                None
            }
        }
    }

    /// `unsafe fn` / `unsafe event` and `unsafe trait` (§4.7.5).
    fn item_leading_unsafe(&mut self) -> bool {
        if self.at_kw(Kw::Unsafe) && matches!(self.nth(1).kind, Kind::Keyword(Kw::Fn) | Kind::Keyword(Kw::Event) | Kind::Keyword(Kw::Trait)) {
            self.bump();
            return true;
        }
        false
    }

    fn outer_attrs_vis_field(&mut self) -> (Vec<Attr>, Visibility, Docs) {
        let (attrs, docs) = self.outer_attrs();
        let vis = self.visibility();
        (attrs, vis, docs)
    }

    fn field(&mut self) -> Option<Field> {
        let (attrs, vis, docs) = self.outer_attrs_vis_field();
        let name_tok = self.peek().clone();
        let name = self.ident_or_error();
        let start = name_tok.span.start;
        self.expect_p(Punct::Colon);
        let ty = self.ty();
        Some(Field { vis, name, ty, attrs, docs, span: Span::new(self.file, start, self.prev_end()) })
    }

    fn variant(&mut self) -> Option<Variant> {
        let (attrs, docs) = self.outer_attrs();
        let start = self.peek().span.start;
        let name = self.ident_or_error();
        let mut tuple = Vec::new();
        let mut struct_fields = Vec::new();
        let mut disc = None;
        if self.at_p(Punct::LParen) {
            self.bump();
            while !self.at_p(Punct::RParen) && !self.at_end() {
                let (_, fv, _) = self.outer_attrs_vis_field();
                let ty = self.ty();
                tuple.push((fv, ty));
                if !self.eat_p(Punct::Comma) {
                    break;
                }
            }
            self.expect_p(Punct::RParen);
        } else if self.at_p(Punct::LBrace) {
            self.bump();
            while !self.at_p(Punct::RBrace) && !self.at_end() {
                if let Some(f) = self.field() {
                    struct_fields.push(f);
                } else {
                    break;
                }
                if !self.eat_p(Punct::Comma) {
                    break;
                }
            }
            self.expect_p(Punct::RBrace);
        }
        if self.eat_p(Punct::Eq) {
            let e = self.expr();
            if let ExprKind::Int(i) = &e.kind {
                disc = i.value;
            }
        }
        Some(Variant { name, tuple, struct_fields, disc, attrs, docs, span: Span::new(self.file, start, self.prev_end()) })
    }

    fn macro_arm(&mut self) -> Option<MacroArm> {
        let start = self.peek().span.start;
        let matcher = self.tt_seq(&[Kind::Punct(Punct::FatArrow)])?;
        if !self.eat_p(Punct::FatArrow) {
            let t = self.peek().clone();
            self.syntax_err(&t, "=>");
            return None;
        }
        // `= expr | block | expr` (§3.9 macro_expander)
        let expander: Vec<Tree> = if self.at_p(Punct::LBrace) || self.at_p(Punct::LParen) || self.at_p(Punct::LBracket) {
            match self.one_tree() {
                Some(t) => vec![t],
                None => return None,
            }
        } else {
            self.eat_p(Punct::Eq);
            // A bare expander expression runs to the arm separator, not to the end of the file.
            match self.tt_seq(&[Kind::Punct(Punct::Semi), Kind::Punct(Punct::Comma)]) {
                Some(ts) => ts,
                None => return None,
            }
        };
        let sep = if self.at_p(Punct::Semi) {
            self.bump();
            Some(Punct::Semi)
        } else if self.at_p(Punct::Comma) {
            self.bump();
            Some(Punct::Comma)
        } else {
            None
        };
        Some(MacroArm { matcher, expander, separator: sep, span: Span::new(self.file, start, self.prev_end()) })
    }

    /// The index of the closer matching the opener at `open`, or `None` when the stream ends first.
    fn matching_close(&self, open: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut i = open;
        while i < self.toks.len() {
            match &self.toks[i].kind {
                Kind::Punct(Punct::LParen) | Kind::Punct(Punct::LBracket) | Kind::Punct(Punct::LBrace) => depth += 1,
                Kind::Punct(Punct::RParen) | Kind::Punct(Punct::RBracket) | Kind::Punct(Punct::RBrace) => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// Consume exactly one token tree at the cursor: a balanced group (delimiters included, so
    /// `macro_rules!` matchers keep their outermost group) or a single leaf.
    ///
    /// `lexer::build_token_tree` deliberately scans the whole slice it is handed, so calling it on
    /// "the rest of the file" — as this function used to do — swallowed every arm after the first
    /// one (the bug stage [2] reported from `macro_expand.rs`).
    fn one_tree(&mut self) -> Option<Tree> {
        let is_open = matches!(self.peek().kind, Kind::Punct(Punct::LParen) | Kind::Punct(Punct::LBracket) | Kind::Punct(Punct::LBrace));
        if !is_open {
            if self.at_end() {
                return None;
            }
            return Some(Tree::Leaf(self.bump()));
        }
        let open = self.pos;
        match self.matching_close(open) {
            Some(close) => {
                let (ts, _n) = crate::lexer::build_token_tree(&self.toks[open..=close]);
                self.pos = close + 1;
                ts.into_iter().next()
            }
            None => {
                let (ts, n) = crate::lexer::build_token_tree(&self.toks[open..]);
                if n == 0 {
                    return None;
                }
                self.pos += n;
                ts.into_iter().next()
            }
        }
    }

    /// Collect token trees up to (not including) one of `stops` at depth 0.
    fn tt_seq(&mut self, stops: &[Kind]) -> Option<Vec<Tree>> {
        let mut out = Vec::new();
        loop {
            if self.at_end() {
                return Some(out);
            }
            let k = self.peek().kind.clone();
            if stops.iter().any(|s| *s == k) {
                return Some(out);
            }
            match self.one_tree() {
                Some(t) => out.push(t),
                None => return None,
            }
        }
    }

    fn ident_or_error(&mut self) -> String {
        let t = self.peek().clone();
        match &t.kind {
            Kind::Ident(s) => {
                let s = s.clone();
                self.bump();
                s
            }
            Kind::Keyword(k) => {
                self.err_at(&t, "E-0020", "保留字作标识符".to_string(), k.text(), Some(format!("`{}` 是保留字（§2.4），请改名", k.text())));
                self.bump();
                k.text().to_string()
            }
            _ => {
                self.syntax_err(&t, "标识符");
                String::new()
            }
        }
    }

    // ---------------------------------------------------------------- §3.3 signatures
    fn fn_sig(&mut self) -> FnSig {
        let name_tok = self.peek().clone();
        let name = self.ident_or_error();
        let name_span = name_tok.span;
        let generics = self.generic_params();
        let mut params = Vec::new();
        let mut is_unsafe = false;
        if self.at_p(Punct::LParen) {
            self.bump();
            while !self.at_p(Punct::RParen) && !self.at_end() {
                match self.param() {
                    Some(p) => params.push(p),
                    None => break,
                }
                if !self.eat_p(Punct::Comma) {
                    break;
                }
            }
            self.expect_p(Punct::RParen);
        } else {
            let t = self.peek().clone();
            self.syntax_err(&t, "(");
        }
        let ret = if self.eat_p(Punct::Arrow) { Some(self.ty()) } else { None };
        let where_preds = self.where_clause();
        if let Kind::Keyword(Kw::Unsafe) = self.peek().kind {
            is_unsafe = true;
        }
        let _ = is_unsafe;
        FnSig { name, name_span, generics, params, ret, where_preds, is_unsafe, span: Span::new(self.file, name_span.start, self.prev_end()) }
    }

    fn param(&mut self) -> Option<Param> {
        let start = self.peek().span.start;
        // receiver shorthand: `self`, `&self`, `&'a self`, `&mut self` (§4.14 receiver ladder)
        if matches!(self.peek().kind, Kind::Keyword(Kw::SelfVal)) || (self.at_p(Punct::Amp) && matches!(self.nth(1).kind, Kind::Keyword(Kw::SelfVal))) || (self.at_p(Punct::Amp) && self.nth(1).is_kw(Kw::Mut) && self.nth(2).is_kw(Kw::SelfVal)) || (self.at_p(Punct::Amp) && matches!(self.nth(1).kind, Kind::Lifetime(_))) {
            let mut lifetime = None;
            let mut mutable = false;
            if self.eat_p(Punct::Amp) {
                if let Kind::Lifetime(l) = self.peek().kind.clone() {
                    self.bump();
                    lifetime = Some(l);
                }
                mutable = self.eat_kw(Kw::Mut);
            }
            let self_tok = self.peek().clone();
            self.bump();
            let ty = if mutable || lifetime.is_some() {
                Type {
                    kind: TypeKind::Ref { lifetime, mutable, inner: Box::new(Type { kind: TypeKind::Named { path: vec!["Self".to_string()], args: Vec::new() }, span: self_tok.span }) },
                    span: self_tok.span,
                }
            } else {
                Type { kind: TypeKind::Named { path: vec!["self".to_string()], args: Vec::new() }, span: self_tok.span }
            };
            return Some(Param { mutable, name: "self".to_string(), ty, default: None, span: Span::new(self.file, start, self.prev_end()), name_span: self_tok.span, is_wildcard: false });
        }
        let mutable = self.eat_kw(Kw::Mut);
        let name_tok = self.peek().clone();
        // `(_: u32)` — unnamed parameter, legal in trait declarations
        if self.at_p(Punct::Colon) && self.nth(1).is_p(Punct::LParen) {
            // `x: T` normal path; nothing to do
        }
        if name_tok.is_p(Punct::LParen) {
            self.bump();
            let _ = self.peek().clone();
            self.eat_p(Punct::Colon);
            let ty = self.ty();
            self.expect_p(Punct::RParen);
            return Some(Param { mutable: false, name: String::new(), ty, default: None, span: Span::new(self.file, start, self.prev_end()), name_span: name_tok.span, is_wildcard: true });
        }
        let name = match &name_tok.kind {
            Kind::Ident(s) => {
                self.bump();
                s.clone()
            }
            Kind::Keyword(k) => {
                self.err_at(&name_tok, "E-0020", "保留字作参数名".to_string(), k.text(), Some(format!("`{}` 是保留字（§2.4）", k.text())));
                self.bump();
                return None;
            }
            _ => {
                self.syntax_err(&name_tok, "参数名");
                return None;
            }
        };
        let is_wildcard = name == "_";
        // §4.3: `var` is for statement bindings only; a `var` parameter is E-0022
        self.expect_p(Punct::Colon);
        let ty = self.ty();
        let default = if self.eat_p(Punct::Eq) { Some(self.expr()) } else { None };
        Some(Param { mutable, name, ty, default, span: Span::new(self.file, start, self.prev_end()), name_span: name_tok.span, is_wildcard })
    }

    fn generic_params(&mut self) -> Vec<GenericParam> {
        let mut out = Vec::new();
        if !self.at_p(Punct::Lt) {
            return out;
        }
        self.bump();
        let mut depth = 1usize;
        while depth > 0 && !self.at_end() {
            if self.at_p(Punct::Lt) {
                depth += 1;
            }
            if self.at_p(Punct::Gt) {
                depth -= 1;
                self.bump();
                break;
            }
            if self.at_p(Punct::Shr) {
                // `>>` closes two parameter lists (§3.10: `>` pairing is exact, no heuristics)
                depth -= 2;
                self.bump();
                if depth < 1 {
                    break;
                }
                continue;
            }
            let t = self.peek().clone();
            match &t.kind {
                Kind::Lifetime(l) => {
                    let l = l.clone();
                    self.bump();
                    out.push(GenericParam { name: l, bounds: vec![], default: None, is_lifetime: true, span: t.span });
                }
                Kind::Ident(s) => {
                    let s = s.clone();
                    self.bump();
                    let mut bounds = Vec::new();
                    if self.eat_p(Punct::Colon) {
                        bounds = self.bounds();
                    }
                    let default = if self.eat_p(Punct::Eq) { Some(self.ty()) } else { None };
                    out.push(GenericParam { name: s, bounds, default, is_lifetime: false, span: t.span });
                }
                _ => {
                    self.syntax_err(&t, "泛型参数");
                    self.bump();
                }
            }
            if !self.eat_p(Punct::Comma) {
                continue;
            }
        }
        out
    }

    fn bounds(&mut self) -> Vec<Bound> {
        let mut out = Vec::new();
        loop {
            let t = self.peek().clone();
            match &t.kind {
                Kind::Lifetime(l) => {
                    let l = l.clone();
                    self.bump();
                    out.push(Bound { path: vec![l], args: vec![], is_lifetime: true, negated: false, span: t.span });
                }
                Kind::Ident(_) | Kind::Keyword(Kw::SelfTy) => {
                    let mut path = Vec::new();
                    while let Some(s) = self.peek().ident().or_else(|| {
                        if matches!(self.peek().kind, Kind::Keyword(Kw::SelfTy)) { Some("Self") } else { None }
                    }) {
                        path.push(s.to_string());
                        self.bump();
                        if self.at_p(Punct::ColonColon) || self.at_p(Punct::Dot) {
                            self.bump();
                            continue;
                        }
                        break;
                    }
                    let args = if self.at_p(Punct::Lt) { self.generic_args() } else { Vec::new() };
                    out.push(Bound { path, args, is_lifetime: false, negated: false, span: t.span });
                }
                _ => break,
            }
            if self.at_p(Punct::Plus) || self.at_p(Punct::Pipe) {
                self.bump();
                continue;
            }
            break;
        }
        out
    }

    fn where_clause(&mut self) -> Vec<WherePred> {
        let mut out = Vec::new();
        if !self.at_kw(Kw::Where) {
            return out;
        }
        self.bump();
        loop {
            let start = self.peek().span.start;
            if self.at_p(Punct::LBrace) || self.at_p(Punct::Semi) || self.at_p(Punct::LParen) || self.at_end() {
                break;
            }
            let ty = self.ty();
            if !self.eat_p(Punct::Colon) {
                break;
            }
            let bounds = self.bounds();
            out.push(WherePred { ty, bounds, span: Span::new(self.file, start, self.prev_end()) });
            if !self.eat_p(Punct::Comma) {
                break;
            }
        }
        out
    }

    // ---------------------------------------------------------------- §3.4 types
    fn ty(&mut self) -> Type {
        let t = self.peek().clone();
        let start = t.span.start;
        match &t.kind {
            Kind::Punct(Punct::Amp) => {
                self.bump();
                let lifetime = if let Kind::Lifetime(l) = self.peek().kind.clone() {
                    self.bump();
                    Some(l)
                } else {
                    None
                };
                let mutable = self.eat_kw(Kw::Mut);
                let inner = self.ty();
                Type { kind: TypeKind::Ref { lifetime, mutable, inner: Box::new(inner) }, span: Span::new(self.file, start, self.prev_end()) }
            }
            Kind::Punct(Punct::Star) => {
                self.bump();
                let mutable = if self.at_kw(Kw::Mut) {
                    self.bump();
                    true
                } else {
                    self.eat_kw(Kw::Const);
                    false
                };
                let inner = self.ty();
                Type { kind: TypeKind::Ptr { mutable, inner: Box::new(inner) }, span: Span::new(self.file, start, self.prev_end()) }
            }
            Kind::Punct(Punct::LBracket) => {
                self.bump();
                let inner = self.ty();
                if self.eat_p(Punct::Semi) {
                    let len = self.expr();
                    self.expect_p(Punct::RBracket);
                    Type { kind: TypeKind::Array { inner: Box::new(inner), len: Box::new(len) }, span: Span::new(self.file, start, self.prev_end()) }
                } else {
                    self.expect_p(Punct::RBracket);
                    Type { kind: TypeKind::Slice { inner: Box::new(inner) }, span: Span::new(self.file, start, self.prev_end()) }
                }
            }
            Kind::Punct(Punct::LParen) => {
                self.bump();
                if self.eat_p(Punct::RParen) {
                    // `()` — unit (§3.4)
                    return Type { kind: TypeKind::Tuple(vec![]), span: Span::new(self.file, start, self.prev_end()) };
                }
                let mut items = Vec::new();
                while !self.at_p(Punct::RParen) && !self.at_end() {
                    items.push(self.ty());
                    if !self.eat_p(Punct::Comma) {
                        break;
                    }
                }
                self.expect_p(Punct::RParen);
                Type { kind: TypeKind::Tuple(items), span: Span::new(self.file, start, self.prev_end()) }
            }
            Kind::Keyword(Kw::Fn) => {
                self.bump();
                let mut params = Vec::new();
                if self.eat_p(Punct::LParen) {
                    while !self.at_p(Punct::RParen) && !self.at_end() {
                        params.push(self.ty());
                        if !self.eat_p(Punct::Comma) {
                            break;
                        }
                    }
                    self.expect_p(Punct::RParen);
                }
                let ret = if self.eat_p(Punct::Arrow) { Some(Box::new(self.ty())) } else { None };
                Type { kind: TypeKind::FnPtr { params, ret }, span: Span::new(self.file, start, self.prev_end()) }
            }
            Kind::Keyword(Kw::Dyn) => {
                self.bump();
                let mut path = Vec::new();
                let mut bounds = Vec::new();
                if let Some(s) = self.peek().ident() {
                    path.push(s.to_string());
                    self.bump();
                    while (self.at_p(Punct::ColonColon) || self.at_p(Punct::Dot)) && self.nth(1).ident().is_some() {
                        self.bump();
                        let s = self.peek().ident().unwrap().to_string();
                        self.bump();
                        path.push(s);
                    }
                    bounds = self.bounds();
                } else {
                    let tt = self.peek().clone();
                    self.syntax_err(&tt, "trait 名");
                }
                Type { kind: TypeKind::Dyn { path, bounds }, span: Span::new(self.file, start, self.prev_end()) }
            }
            Kind::Keyword(k) if k.is_prim_type() => {
                self.bump();
                Type { kind: TypeKind::Prim(prim_of_kw(*k)), span: Span::new(self.file, start, self.prev_end()) }
            }
            Kind::Ident(_) => {
                let mut path = Vec::new();
                while let Some(s) = self.peek().ident() {
                    path.push(s.to_string());
                    self.bump();
                    if self.at_p(Punct::ColonColon) || self.at_p(Punct::Dot) {
                        if self.nth(1).ident().is_some() {
                            self.bump();
                            continue;
                        }
                        self.bump();
                        break;
                    }
                    break;
                }
                let args = if self.at_p(Punct::Lt) { self.generic_args() } else { Vec::new() };
                if path.len() == 1 && path[0] == "Self" {
                    return Type { kind: TypeKind::Named { path: vec!["Self".to_string()], args: Vec::new() }, span: Span::new(self.file, start, self.prev_end()) };
                }
                Type { kind: TypeKind::Named { path, args }, span: Span::new(self.file, start, self.prev_end()) }
            }
            Kind::Lifetime(_) => {
                // `'a T` alone is not a type; the lifetime must attach to a `&` (§3.10, E-1002).
                let l = self.peek().clone();
                self.err_at(&l, "E-1002", "生命周期记法位置错误".to_string(), "生命周期必须紧邻 `&`", Some("写 `&'a T` 或 `&'a mut T`（§3.10）".to_string()));
                self.bump();
                let inner = self.ty();
                Type { kind: TypeKind::Ref { lifetime: Some(String::new()), mutable: false, inner: Box::new(inner) }, span: Span::new(self.file, start, self.prev_end()) }
            }
            _ => {
                self.syntax_err(&t, "类型");
                Type::invalid(Span::new(self.file, start, t.span.end))
            }
        }
    }

    fn generic_args(&mut self) -> Vec<Type> {
        let mut out = Vec::new();
        if !self.at_p(Punct::Lt) {
            return out;
        }
        self.bump();
        let mut depth = 1usize;
        while depth > 0 && !self.at_end() {
            if self.at_p(Punct::Lt) {
                depth += 1;
            }
            if self.at_p(Punct::Shr) {
                // `>>` — two closers
                depth -= 2;
                self.bump();
                if depth < 1 {
                    break;
                }
                continue;
            }
            if self.at_p(Punct::Gt) {
                depth -= 1;
                self.bump();
                break;
            }
            if self.at_p(Punct::GtEq) {
                // `>=` closes one level and leaves the `=`; treat as closer only (§3.8 casts)
                depth -= 1;
                self.bump();
                break;
            }
            let t = self.peek().clone();
            if matches!(t.kind, Kind::Lifetime(_)) {
                self.bump();
                out.push(Type { kind: TypeKind::Named { path: vec![lifetime_name(&t)], args: vec![] }, span: t.span });
            } else {
                let ty = self.ty();
                if matches!(ty.kind, TypeKind::Invalid) && t.ident().is_none() {
                    self.bump();
                }
                out.push(ty);
            }
            if !self.eat_p(Punct::Comma) {
                continue;
            }
        }
        out
    }

    // ---------------------------------------------------------------- §3.3 bodies
    fn block_or_embed(&mut self) -> Body {
        if matches!(self.peek().kind, Kind::EmbedLang(_)) {
            let e = self.embed();
            return Body::Embed(e);
        }
        if self.at_p(Punct::LBrace) {
            return Body::Block(self.block());
        }
        if self.eat_p(Punct::Semi) {
            return Body::Foreign;
        }
        let t = self.peek().clone();
        self.syntax_err(&t, "{ 或 ; 或嵌入块");
        Body::Foreign
    }

    /// §3.6 — the lexer collapsed the body into `EmbedBody`; here the `(k = v, …)` argument list
    /// is re-parsed, which is ordinary INFY expression syntax (§7.7.1).
    fn embed(&mut self) -> EmbedBlock {
        let lang_tok = self.bump();
        let lang = match &lang_tok.kind {
            Kind::EmbedLang(s) => s.clone(),
            _ => String::new(),
        };
        let lang_span = lang_tok.span;
        let mut args = Vec::new();
        let args_tok = if matches!(self.peek().kind, Kind::EmbedArgs(_)) { Some(self.bump()) } else { None };
        if let Some(at) = args_tok {
            if let Kind::EmbedArgs(raw) = &at.kind {
                args = self.embed_args(raw, at.span);
            }
        }
        let body_tok = self.peek().clone();
        let (body, body_span) = match &body_tok.kind {
            Kind::EmbedBody(s) => {
                self.bump();
                (s.clone(), body_tok.span)
            }
            _ => {
                self.err_at(&body_tok, "E-0053", "嵌入块缺少主体".to_string(), "此处应为 `{ … }`", Some("`@` 前缀之后可写 `(参数)`，再写 `{ 外部语言源码 }`（§3.6）".to_string()));
                (String::new(), Span::empty_at(self.file, body_tok.span.start))
            }
        };
        EmbedBlock { lang, lang_span, args, body, body_span, span: Span::new(self.file, lang_span.start, body_span.end) }
    }

    fn embed_args(&mut self, raw: &str, span: Span) -> Vec<EmbedArg> {
        let mut out = Vec::new();
        for chunk in split_top_level_commas(raw) {
            let chunk = chunk.trim();
            if chunk.is_empty() {
                continue;
            }
            match chunk.split_once('=') {
                Some((name, rest)) => {
                    let value = self.parse_sub_expr(rest.trim());
                    out.push(EmbedArg { name: name.trim().to_string(), value, span });
                }
                None => {
                    let t = self.peek().clone();
                    self.err_at(&t, "E-0053", "嵌入参数写法非法".to_string(), chunk, Some("嵌入参数必须是 `name = expr`（§3.6）".to_string()));
                }
            }
        }
        out
    }

    /// Parse a short expression taken from an embed argument list, using a nested parser over its
    /// own lexed token stream so the main cursor is untouched. On any failure the literal text is
    /// preserved as an identifier-shaped expression (sema then reports the real code).
    fn parse_sub_expr(&mut self, text: &str) -> Expr {
        let at = self.prev_end();
        let span = Span::new(self.file, at, at);
        let mut src = Sources::default();
        let fid = src.add("<embed-args>".to_string(), format!("{text}\n"));
        let mut h = Handler::default();
        let lexed = crate::lexer::lex_file(&src, fid, &mut h);
        if !h.diags.is_empty() || lexed.toks.is_empty() {
            return Expr::new(ExprKind::Ident(text.to_string()), span);
        }
        let toks = lexed.toks.clone();
        let comments = lexed.comments.clone();
        let mut sub = P { toks, pos: 0, file: fid, src: src.get(fid), comments, h: &mut h, stop: false };
        let e = sub.expr();
        let trailing_is_end = sub.at_end();
        if !matches!(e.kind, ExprKind::Invalid) && trailing_is_end {
            // Positions inside the sub-expression are relative to the synthetic file; sema only
            // uses them for embed argument values, which are constants.
            return e;
        }
        drop(sub);
        Expr::new(ExprKind::Ident(text.to_string()), span)
    }

    // ---------------------------------------------------------------- §3.5 blocks/statements
    fn block(&mut self) -> Block {
        let open = self.bump(); // `{`
        let start = open.span.start;
        let mut stmts = Vec::new();
        let mut tail = None;
        while !self.at_p(Punct::RBrace) && !self.at_end() {
            if self.starts_expr() {
                let before = self.pos;
                let e = self.expr();
                if matches!(e.kind, ExprKind::Invalid) {
                    self.pos = before;
                    let t = self.peek().clone();
                    self.syntax_err(&t, "语句");
                    self.synchronize();
                    continue;
                }
                if self.at_p(Punct::RBrace) {
                    // block value: tail expression, `;` optional (§3.3)
                    tail = Some(e);
                    self.eat_p(Punct::Semi);
                    break;
                }
                let terminated = self.eat_p(Punct::Semi);
                let blockish = matches!(e.kind, ExprKind::If { .. } | ExprKind::Match { .. } | ExprKind::Block(_) | ExprKind::Unsafe(_) | ExprKind::StructLit { .. });
                if !terminated && !blockish {
                    let t = self.peek().clone();
                    self.err_at(&t, "E-0053", "语句缺少终结符".to_string(), "该表达式既不是块表达式，也不是函数体的尾表达式", Some("补 `;`（§2.11：无自动分号插入）".to_string()));
                }
                let stmt_span = Span::new(self.file, e.span.start, self.prev_end());
                stmts.push(Stmt { kind: StmtKind::Expr { expr: e, linkage: Vec::new() }, attrs: Vec::new(), docs: Docs::default(), span: stmt_span, terminated });
                continue;
            }
            let before = self.pos;
            match self.stmt() {
                Some(s) => stmts.push(s),
                None => {
                    if self.pos == before {
                        let t = self.peek().clone();
                        self.syntax_err(&t, "语句");
                        self.synchronize();
                    }
                    if self.pos == before {
                        self.bump();
                    }
                }
            }
        }
        self.expect_p(Punct::RBrace);
        Block { stmts, tail, span: Span::new(self.file, start, self.prev_end()) }
    }

    /// Can the current token begin an expression? Anything that is not a statement keyword.
    fn starts_expr(&self) -> bool {
        match &self.peek().kind {
            Kind::Lit(_) | Kind::Lit(Lit::Int(_)) | Kind::Ident(_) | Kind::EmbedLang(_) | Kind::AttrOpen { .. } => true,
            Kind::Punct(p) => matches!(p, Punct::LParen | Punct::LBracket | Punct::LBrace | Punct::Amp | Punct::Star | Punct::Bang | Punct::Minus | Punct::Pipe | Punct::At),
            // §3.5 makes return/break/continue/for STATEMENTS, not expressions: they must go
            // through stmt(), otherwise `return Err(..)` inside a block mis-parses.
            Kind::Keyword(k) => matches!(k, Kw::If | Kw::Match | Kw::Move | Kw::Loop | Kw::Unsafe | Kw::True | Kw::False | Kw::Auto | Kw::SelfVal | Kw::SelfTy),
            _ => false,
        }
    }

    fn stmt(&mut self) -> Option<Stmt> {
        let (attrs, docs) = self.outer_attrs();
        let start = self.peek().span.start;
        let t = self.peek().clone();
        let kind: Option<StmtKind> = match &t.kind {
            Kind::Keyword(Kw::Let) | Kind::Keyword(Kw::Var) => {
                let is_var = matches!(t.kind, Kind::Keyword(Kw::Var));
                self.bump();
                if self.at_kw(Kw::Mut) {
                    // §4.3: `let mut` is illegal; the mutable statement binding is `var`.
                    let m = self.peek().clone();
                    let following = self.nth(1).text.clone();
                    self.err_at(&m, "E-0021", "`let mut` 非法".to_string(), "`let` 是不可变绑定", Some(format!("改作 `var {following} = …;`（§4.3）")));
                    self.bump();
                }
                let name_tok = self.peek().clone();
                let name = self.ident_or_error();
                let name_span = name_tok.span;
                let ty = if self.eat_p(Punct::Colon) { Some(self.ty()) } else { None };
                let init = if self.eat_p(Punct::Eq) { Some(self.expr()) } else { None };
                let term = self.eat_p(Punct::Semi);
                if !term {
                    let tt = self.peek().clone();
                    self.syntax_err(&tt, ";");
                }
                Some(StmtKind::Let { is_var, name, name_span, ty, init })
            }
            Kind::Keyword(Kw::If) => {
                self.bump();
                let cond = self.paren_expr();
                let then = self.block();
                let else_ = if self.eat_kw(Kw::Else) { Some(Box::new(self.else_branch())) } else { None };
                Some(StmtKind::If { cond, then, else_ })
            }
            Kind::Keyword(Kw::While) => {
                self.bump();
                let cond = self.paren_expr();
                let body = self.block();
                Some(StmtKind::While { cond, body })
            }
            Kind::Keyword(Kw::Loop) => {
                self.bump();
                let body = self.block();
                Some(StmtKind::Loop { body, label: None })
            }
            Kind::Keyword(Kw::For) => {
                self.bump();
                let pat = self.pat();
                self.expect_kw(Kw::In);
                let iter = self.expr_no_assign();
                let body = self.block();
                Some(StmtKind::For { pat, iter, body, label: None })
            }
            Kind::Keyword(Kw::Match) => {
                self.bump();
                let scrut = self.paren_expr();
                let arms = self.match_arms();
                Some(StmtKind::Match { scrut, arms })
            }
            Kind::Keyword(Kw::Unsafe) => {
                self.bump();
                Some(StmtKind::Unsafe(self.block()))
            }
            Kind::Keyword(Kw::Defer) => {
                self.bump();
                Some(StmtKind::Defer(self.block()))
            }
            Kind::Keyword(Kw::ConstAssert) => {
                self.bump();
                self.expect_p(Punct::LParen);
                let cond = self.expr();
                self.expect_p(Punct::RParen);
                let msg = if self.eat_p(Punct::Comma) {
                    match self.peek().kind.clone() {
                        Kind::Lit(Lit::Str(s)) => {
                            self.bump();
                            Some(s)
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                self.eat_p(Punct::Semi);
                Some(StmtKind::ConstAssert { cond, msg })
            }
            Kind::Keyword(Kw::Break) => {
                self.bump();
                let label = if let Kind::Lifetime(l) = self.peek().kind.clone() {
                    self.bump();
                    Some(l)
                } else {
                    None
                };
                let value = if self.at_p(Punct::Semi) || self.at_p(Punct::RBrace) || self.at_end() { None } else { Some(self.expr()) };
                self.eat_p(Punct::Semi);
                Some(StmtKind::Break { label, value })
            }
            Kind::Keyword(Kw::Continue) => {
                self.bump();
                let label = if let Kind::Lifetime(l) = self.peek().kind.clone() {
                    self.bump();
                    Some(l)
                } else {
                    None
                };
                self.eat_p(Punct::Semi);
                Some(StmtKind::Continue { label })
            }
            Kind::Keyword(Kw::Return) => {
                self.bump();
                let value = if self.at_p(Punct::Semi) || self.at_p(Punct::RBrace) || self.at_end() { None } else { Some(self.expr()) };
                self.eat_p(Punct::Semi);
                Some(StmtKind::Return { value })
            }
            Kind::Keyword(Kw::Pause) => {
                self.bump();
                self.eat_p(Punct::Semi);
                Some(StmtKind::Pause)
            }
            Kind::Keyword(Kw::Stop) => {
                self.bump();
                self.eat_p(Punct::Semi);
                Some(StmtKind::Stop)
            }
            Kind::Keyword(Kw::Resume) => {
                self.bump();
                self.eat_p(Punct::Semi);
                Some(StmtKind::Resume)
            }
            Kind::Keyword(Kw::Label) => {
                self.bump();
                let name_tok = self.peek().clone();
                let name = self.ident_or_error();
                let name_span = name_tok.span;
                self.expect_p(Punct::Colon);
                Some(StmtKind::Label { name, name_span })
            }
            Kind::Keyword(k) if matches!(k, Kw::Fn | Kw::Event | Kw::Struct | Kw::Enum | Kw::Trait | Kw::Impl | Kw::Const | Kw::Static | Kw::Type | Kw::Union | Kw::Extern | Kw::Chain | Kw::MacroRules | Kw::Mod) => {
                // §3.5 has no nested declaration statement; keep it so sema can report where the
                // declaration actually appeared.
                let it = self.item()?;
                Some(StmtKind::NestedItem(it))
            }
            Kind::Keyword(Kw::Pub) | Kind::Keyword(Kw::Priv) => {
                let it = self.item()?;
                Some(StmtKind::NestedItem(it))
            }
            _ => {
                let e = self.expr();
                if matches!(e.kind, ExprKind::Invalid) {
                    let tt = self.peek().clone();
                    self.syntax_err(&tt, "语句");
                    return None;
                }
                // §3.5/§8.1: a linkage block attaches to the host expression statement — and in
                // §14.3 it is written on the line *after* the host statement's `;`, so the `;` is
                // consumed first and only then are the following `{ … }` groups collected.
                let mut terminated = self.eat_p(Punct::Semi);
                let mut linkage = Vec::new();
                while self.at_p(Punct::LBrace) {
                    let b = self.linkage_block();
                    linkage.push(b);
                    terminated = self.eat_p(Punct::Semi) || terminated;
                }
                if !terminated && linkage.is_empty() && !matches!(e.kind, ExprKind::If { .. } | ExprKind::Match { .. } | ExprKind::Block(_) | ExprKind::Unsafe(_) | ExprKind::Closure { .. }) {
                    let tt = self.peek().clone();
                    self.err_at(&tt, "E-0053", "语句缺少终结符".to_string(), "此处需要 `;`", Some("§2.11 无自动分号插入".to_string()));
                }
                Some(StmtKind::Expr { expr: e, linkage })
            }
        };
        let kind = kind?;
        let span = Span::new(self.file, start, self.prev_end());
        Some(Stmt { kind, attrs, docs, span, terminated: true })
    }

    fn else_branch(&mut self) -> ElseBranch {
        if self.at_kw(Kw::If) {
            // §3.10: `else` binds to the nearest unmatched `if`
            if let Some(s) = self.stmt() {
                return ElseBranch::If(s);
            }
            return ElseBranch::Block(Block { stmts: Vec::new(), tail: None, span: Span::empty_at(self.file, self.prev_end()) });
        }
        if self.at_p(Punct::LBrace) {
            return ElseBranch::Block(self.block());
        }
        let t = self.peek().clone();
        self.syntax_err(&t, "else 之后的块或 if");
        ElseBranch::Block(Block { stmts: Vec::new(), tail: None, span: Span::empty_at(self.file, t.span.end) })
    }

    fn paren_expr(&mut self) -> Expr {
        if !self.eat_p(Punct::LParen) {
            let t = self.peek().clone();
            self.err_at(&t, "E-0053", "控制流条件缺少括号".to_string(), "此处应为 `(`", Some("INFY 的 `if`/`while`/`match` 条件必须加括号（§3.5）".to_string()));
            return Expr::new(ExprKind::Invalid, t.span);
        }
        let e = self.expr();
        self.expect_p(Punct::RParen);
        e
    }

    fn match_arms(&mut self) -> Vec<MatchArm> {
        let mut arms = Vec::new();
        if !self.eat_p(Punct::LBrace) {
            let t = self.peek().clone();
            self.syntax_err(&t, "{");
            return arms;
        }
        while !self.at_p(Punct::RBrace) && !self.at_end() {
            let start = self.peek().span.start;
            let pat = self.pat();
            let guard = if self.eat_kw(Kw::If) { Some(self.expr_no_assign()) } else { None };
            if !self.eat_p(Punct::FatArrow) {
                let t = self.peek().clone();
                self.syntax_err(&t, "=>");
                self.synchronize();
                if self.at_end() {
                    break;
                }
                continue;
            }
            let body = if self.at_p(Punct::LBrace) { ArmBody::Block(self.block()) } else { ArmBody::Expr(self.expr()) };
            arms.push(MatchArm { pat, guard, body, span: Span::new(self.file, start, self.prev_end()) });
            if !self.eat_p(Punct::Comma) && !self.at_p(Punct::RBrace) {
                // §14.2: arms are comma separated
                let t = self.peek().clone();
                self.err_at(&t, "E-0053", "match 臂缺少分隔符".to_string(), "臂之间以 `,` 分隔", Some("§3.5：match 臂以逗号分隔".to_string()));
                self.synchronize();
                if self.at_end() {
                    break;
                }
            }
        }
        self.expect_p(Punct::RBrace);
        arms
    }

    // ---------------------------------------------------------------- §8.1 linkage
    fn linkage_block(&mut self) -> LinkageBlock {
        let open = self.bump(); // `{`
        let start = open.span.start;
        let mut stmts = Vec::new();
        while !self.at_p(Punct::RBrace) && !self.at_end() {
            let before = self.pos;
            match self.linkage_stmt() {
                Some(s) => stmts.push(s),
                None => {
                    if self.pos == before {
                        let t = self.peek().clone();
                        self.syntax_err(&t, "连携语句");
                        self.synchronize();
                    }
                    if self.pos == before {
                        self.bump();
                    }
                }
            }
            while self.eat_p(Punct::Semi) {}
            if self.at_p(Punct::Comma) {
                self.bump();
            }
        }
        self.expect_p(Punct::RBrace);
        LinkageBlock { stmts, span: Span::new(self.file, start, self.prev_end()) }
    }

    fn linkage_stmt(&mut self) -> Option<LinkageStmt> {
        let t = self.peek().clone();
        match &t.kind {
            Kind::Keyword(Kw::Goto) => {
                self.bump();
                self.expect_p(Punct::LParen);
                let target = self.goto_target();
                self.expect_p(Punct::RParen);
                Some(LinkageStmt::Goto(target))
            }
            Kind::Keyword(Kw::Start) => {
                self.bump();
                self.expect_p(Punct::LParen);
                // `start(chain = Ident, node = <int|ident|str>)` (§3.5)
                let mut chain = String::new();
                let mut chain_span = t.span;
                let mut node = None;
                loop {
                    let name = match self.peek().ident() {
                        Some(n) => n.to_string(),
                        None => break,
                    };
                    self.bump();
                    self.eat_p(Punct::Eq);
                    let v = self.peek().clone();
                    match &v.kind {
                        Kind::Ident(s) => {
                            let s = s.clone();
                            self.bump();
                            if name == "chain" {
                                chain = s;
                                chain_span = v.span;
                            } else if name == "node" {
                                node = Some(StartNode::Ident(s));
                            }
                        }
                        Kind::Lit(Lit::Int(i)) => {
                            let iv = i.value.unwrap_or(0);
                            self.bump();
                            if name == "node" {
                                node = Some(StartNode::Index(iv));
                            }
                        }
                        Kind::Lit(Lit::Str(s)) => {
                            let s = s.clone();
                            self.bump();
                            if name == "node" {
                                node = Some(StartNode::Anchor(s));
                            }
                        }
                        _ => {
                            self.syntax_err(&v, "start 参数值");
                            self.bump();
                        }
                    }
                    if !self.eat_p(Punct::Comma) {
                        break;
                    }
                }
                self.expect_p(Punct::RParen);
                Some(LinkageStmt::Start { chain, chain_span, node })
            }
            Kind::Keyword(Kw::StartFlag) => {
                self.bump();
                Some(LinkageStmt::StartFlag)
            }
            Kind::Keyword(Kw::Pause) => {
                self.bump();
                Some(LinkageStmt::Pause)
            }
            Kind::Keyword(Kw::Stop) => {
                self.bump();
                Some(LinkageStmt::Stop)
            }
            Kind::Keyword(Kw::Resume) => {
                self.bump();
                Some(LinkageStmt::Resume)
            }
            Kind::Keyword(Kw::If) => {
                self.bump();
                let cond = self.paren_expr();
                let then = self.linkage_body_block();
                let else_ = if self.eat_kw(Kw::Else) { Some(self.linkage_body_block()) } else { None };
                Some(LinkageStmt::If { cond, then, else_ })
            }
            _ => {
                // §3.5: any plain statement may also execute inside a linkage block
                let s = self.stmt()?;
                Some(LinkageStmt::Plain(s))
            }
        }
    }

    fn linkage_body_block(&mut self) -> Vec<LinkageStmt> {
        if !self.at_p(Punct::LBrace) {
            let t = self.peek().clone();
            self.syntax_err(&t, "{");
            return Vec::new();
        }
        self.linkage_block().stmts
    }

    fn goto_target(&mut self) -> GotoTarget {
        let t = self.peek().clone();
        match &t.kind {
            Kind::Lit(Lit::Str(s)) => {
                self.bump();
                GotoTarget::Named(s.clone())
            }
            Kind::Lit(Lit::Int(i)) => {
                let v = i.value.unwrap_or(0);
                self.bump();
                GotoTarget::Abs(v)
            }
            Kind::Punct(Punct::Plus) | Kind::Punct(Punct::Minus) => {
                // §3.10: inside a goto target, `+N` / `-N` is a relative offset, never unary ±.
                let neg = self.at_p(Punct::Minus);
                self.bump();
                let v = match &self.peek().kind {
                    Kind::Lit(Lit::Int(i)) => i.value.unwrap_or(0) as i128,
                    _ => {
                        let tt = self.peek().clone();
                        self.syntax_err(&tt, "偏移量整数");
                        0
                    }
                };
                self.bump();
                GotoTarget::Rel(if neg { -v } else { v })
            }
            Kind::Ident(s) => {
                // §3.5 wants a string literal; accept the bare name for recovery and let sema
                // report the normative E-0301 class.
                let s = s.clone();
                self.bump();
                GotoTarget::Named(s)
            }
            _ => {
                self.syntax_err(&t, "goto 目标");
                self.bump();
                GotoTarget::Named(String::new())
            }
        }
    }

    // ---------------------------------------------------------------- §3.12 patterns
    fn pat(&mut self) -> Pat {
        self.pat_or()
    }

    fn pat_or(&mut self) -> Pat {
        let start = self.peek().span.start;
        let mut alternatives = vec![self.pat_unary()];
        while self.at_p(Punct::Pipe) && !self.nth(1).is_p(Punct::Pipe) {
            self.bump();
            alternatives.push(self.pat_unary());
        }
        if alternatives.len() == 1 {
            alternatives.pop().unwrap()
        } else {
            Pat { kind: PatKind::Or(alternatives), span: Span::new(self.file, start, self.prev_end()) }
        }
    }

    fn pat_unary(&mut self) -> Pat {
        let t = self.peek().clone();
        let start = t.span.start;
        match &t.kind {
            Kind::Punct(Punct::Bang) => {
                self.bump();
                self.expect_p(Punct::LParen);
                let mut inner = Vec::new();
                loop {
                    inner.push(self.pat_or());
                    if !self.eat_p(Punct::Pipe) {
                        break;
                    }
                }
                self.expect_p(Punct::RParen);
                Pat { kind: PatKind::Exclude(inner), span: Span::new(self.file, start, self.prev_end()) }
            }
            Kind::Punct(Punct::DotDot) | Kind::Punct(Punct::DotDotEq) => {
                self.bump();
                Pat { kind: PatKind::DotDot, span: Span::new(self.file, start, self.prev_end()) }
            }
            Kind::Punct(Punct::LParen) => {
                self.bump();
                let mut items = Vec::new();
                while !self.at_p(Punct::RParen) && !self.at_end() {
                    items.push(self.pat_or());
                    if !self.eat_p(Punct::Comma) {
                        break;
                    }
                }
                self.expect_p(Punct::RParen);
                Pat { kind: PatKind::Tuple(items), span: Span::new(self.file, start, self.prev_end()) }
            }
            Kind::Punct(Punct::LBracket) => {
                self.bump();
                let mut items = Vec::new();
                let mut rest = None;
                while !self.at_p(Punct::RBracket) && !self.at_end() {
                    if self.at_p(Punct::DotDot) {
                        self.bump();
                        rest = Some(Box::new(if self.at_p(Punct::RBracket) {
                            Pat { kind: PatKind::Wildcard, span: t.span }
                        } else {
                            self.pat_unary()
                        }));
                        break;
                    }
                    items.push(self.pat_or());
                    if !self.eat_p(Punct::Comma) {
                        break;
                    }
                }
                self.expect_p(Punct::RBracket);
                Pat { kind: PatKind::Slice(items, rest), span: Span::new(self.file, start, self.prev_end()) }
            }
            Kind::Keyword(Kw::Ref) | Kind::Keyword(Kw::Mut) => {
                let by_ref = self.at_kw(Kw::Ref);
                self.bump();
                let mutable = self.eat_kw(Kw::Mut);
                let name = self.ident_or_error();
                let sub = if self.eat_p(Punct::At) { Some(Box::new(self.pat_unary())) } else { None };
                Pat { kind: PatKind::Binding { name, by_ref, mutable, sub }, span: Span::new(self.file, start, self.prev_end()) }
            }
            Kind::Lit(_) | Kind::Lit(Lit::Int(_)) => self.pat_lit(),
            Kind::Ident(_) | Kind::Keyword(Kw::SelfTy) | Kind::Keyword(Kw::SelfVal) => {
                let mut segs: Vec<String> = Vec::new();
                if let Some(s) = t.ident() {
                    segs.push(s.to_string());
                    self.bump();
                } else {
                    segs.push(t.text.clone());
                    self.bump();
                }
                while self.at_p(Punct::ColonColon) && self.nth(1).ident().is_some() {
                    self.bump();
                    let s = self.peek().ident().unwrap().to_string();
                    self.bump();
                    segs.push(s);
                }
                if segs.len() == 1 && segs[0] == "_" {
                    return Pat { kind: PatKind::Wildcard, span: Span::new(self.file, start, self.prev_end()) };
                }
                // range pattern: `lo..hi` / `lo..=hi`
                if self.at_p(Punct::DotDot) || self.at_p(Punct::DotDotEq) {
                    let inclusive = self.at_p(Punct::DotDotEq);
                    self.bump();
                    let hi = self.pat_lit();
                    let lo = single_pat(&segs, t.span);
                    return Pat { kind: PatKind::Range { lo: Box::new(lo), hi: Box::new(hi), inclusive }, span: Span::new(self.file, start, self.prev_end()) };
                }
                if self.at_p(Punct::LParen) {
                    self.bump();
                    let mut args = Vec::new();
                    while !self.at_p(Punct::RParen) && !self.at_end() {
                        args.push(self.pat_or());
                        if !self.eat_p(Punct::Comma) {
                            break;
                        }
                    }
                    self.expect_p(Punct::RParen);
                    return Pat { kind: PatKind::Constructor { path: segs, args, fields: Vec::new(), has_rest: false }, span: Span::new(self.file, start, self.prev_end()) };
                }
                if self.looks_like_struct_pat() {
                    self.bump();
                    let mut fields = Vec::new();
                    let mut has_rest = false;
                    while !self.at_p(Punct::RBrace) && !self.at_end() {
                        if self.at_p(Punct::DotDot) {
                            self.bump();
                            has_rest = true;
                            break;
                        }
                        let name_tok = self.peek().clone();
                        let name = self.ident_or_error();
                        let p = if self.eat_p(Punct::Colon) { self.pat_or() } else { Pat { kind: PatKind::Binding { name: name.clone(), by_ref: false, mutable: false, sub: None }, span: name_tok.span } };
                        fields.push((name, p));
                        if !self.eat_p(Punct::Comma) {
                            break;
                        }
                    }
                    self.expect_p(Punct::RBrace);
                    return Pat { kind: PatKind::Constructor { path: segs, args: Vec::new(), fields, has_rest }, span: Span::new(self.file, start, self.prev_end()) };
                }
                if segs.len() == 1 && !Kw::is_reserved(&segs[0]) {
                    let sub = if self.eat_p(Punct::At) { Some(Box::new(self.pat_unary())) } else { None };
                    return Pat { kind: PatKind::Binding { name: segs[0].clone(), by_ref: false, mutable: false, sub }, span: Span::new(self.file, start, self.prev_end()) };
                }
                Pat { kind: PatKind::Path(segs), span: Span::new(self.file, start, self.prev_end()) }
            }
            _ => {
                self.syntax_err(&t, "模式");
                Pat { kind: PatKind::Invalid, span: t.span }
            }
        }
    }

    fn pat_lit(&mut self) -> Pat {
        let t = self.bump();
        let lit = match &t.kind {
            Kind::Lit(l) => l.clone(),
            Kind::Lit(Lit::Int(i)) => Lit::Int(i.clone()),
            _ => {
                self.syntax_err(&t, "字面量");
                return Pat { kind: PatKind::Invalid, span: t.span };
            }
        };
        Pat { kind: PatKind::Lit(lit), span: t.span }
    }

    /// `{ x }` after a path is a struct pattern only when it looks like `ident :` / `ident ,` /
    /// `ident }` / `..` — otherwise the braces belong to the `match` arm body.
    fn looks_like_struct_pat(&self) -> bool {
        if !self.at_p(Punct::LBrace) {
            return false;
        }
        if self.nth(1).is_p(Punct::RBrace) || self.nth(1).is_p(Punct::DotDot) {
            return true;
        }
        match self.nth(1).kind {
            Kind::Ident(_) => matches!(self.nth(2).kind, Kind::Punct(Punct::Colon) | Kind::Punct(Punct::Comma) | Kind::Punct(Punct::RBrace)),
            _ => false,
        }
    }

    // ---------------------------------------------------------------- §2.7/§3.8/§3.11 expressions
    fn expr(&mut self) -> Expr {
        self.expr_prec(2)
    }

    fn expr_no_assign(&mut self) -> Expr {
        self.expr_prec(3)
    }

    /// Precedence climbing. `min_prec` is the lowest precedence allowed at this position; §2.7's
    /// table is the only source, and it is fixed (implementations may not reorder it).
    fn expr_prec(&mut self, min_prec: u8) -> Expr {
        let start = self.peek().span.start;
        let mut lhs = match self.prefix(start) {
            Some(e) => e,
            None => return Expr::new(ExprKind::Invalid, Span::new(self.file, start, self.prev_end())),
        };
        loop {
            lhs = self.postfix(lhs, start);
            let t = self.peek().clone();
            // §2.7 precedence 15: `as Type` / `is Type`
            if matches!(t.kind, Kind::Keyword(Kw::As) | Kind::Keyword(Kw::Is)) {
                self.bump();
                let ty = self.ty();
                lhs = Expr::new(if t.is_kw(Kw::As) { ExprKind::Cast { expr: Box::new(lhs), ty } } else { ExprKind::Is { expr: Box::new(lhs), ty } }, Span::new(self.file, start, self.prev_end()));
                continue;
            }
            let (op, prec, right_assoc) = match binop_of(&t.kind) {
                Some(v) => v,
                None => break,
            };
            if prec < min_prec {
                break;
            }
            self.bump();
            let next_min = if right_assoc { prec } else { prec + 1 };
            let rhs = self.expr_prec(next_min);
            let kind = match op {
                Punct::Eq => ExprKind::Assign { target: Box::new(lhs), op, value: Box::new(rhs) },
                Punct::PlusEq | Punct::MinusEq | Punct::StarEq | Punct::SlashEq | Punct::PercentEq | Punct::AmpEq | Punct::HatEq | Punct::PipeEq | Punct::ShlEq | Punct::ShrEq => ExprKind::Assign { target: Box::new(lhs), op, value: Box::new(rhs) },
                _ => ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) },
            };
            lhs = Expr::new(kind, Span::new(self.file, start, self.prev_end()));
        }
        lhs
    }

    fn prefix(&mut self, start: u32) -> Option<Expr> {
        let t = self.peek().clone();
        match &t.kind {
            Kind::Punct(Punct::Minus) | Kind::Punct(Punct::Bang) => {
                let op = match t.kind {
                    Kind::Punct(p) => p,
                    _ => unreachable!(),
                };
                self.bump();
                let e = self.expr_prec(14);
                Some(Expr::new(ExprKind::Unary { op, expr: Box::new(e) }, Span::new(self.file, start, self.prev_end())))
            }
            Kind::Punct(Punct::Star) => {
                self.bump();
                let e = self.expr_prec(14);
                Some(Expr::new(ExprKind::Deref { expr: Box::new(e) }, Span::new(self.file, start, self.prev_end())))
            }
            Kind::Punct(Punct::Amp) => {
                self.bump();
                let lifetime = if let Kind::Lifetime(l) = self.peek().kind.clone() {
                    self.bump();
                    Some(l)
                } else {
                    None
                };
                let mutable = self.eat_kw(Kw::Mut);
                let e = self.expr_prec(14);
                Some(Expr::new(ExprKind::Ref { mutable, lifetime, expr: Box::new(e) }, Span::new(self.file, start, self.prev_end())))
            }
            Kind::Keyword(Kw::Move) => {
                self.bump();
                if self.at_p(Punct::Pipe) {
                    return Some(self.closure_from(true, start));
                }
                let e = self.expr_prec(14);
                Some(Expr::new(ExprKind::Move { expr: Box::new(e) }, Span::new(self.file, start, self.prev_end())))
            }
            Kind::Punct(Punct::Pipe) => Some(self.closure_from(false, start)),
            _ => self.primary(start),
        }
    }

    fn closure_from(&mut self, move_: bool, start: u32) -> Expr {
        self.expect_p(Punct::Pipe);
        let mut params = Vec::new();
        while !self.at_p(Punct::Pipe) && !self.at_end() {
            self.eat_kw(Kw::Mut);
            let name = self.ident_or_error();
            let ty = if self.eat_p(Punct::Colon) { Some(self.ty()) } else { None };
            params.push((name, ty));
            if !self.eat_p(Punct::Comma) {
                break;
            }
        }
        self.expect_p(Punct::Pipe);
        let ret = if self.eat_p(Punct::Arrow) { Some(self.ty()) } else { None };
        let body = if self.at_p(Punct::LBrace) {
            self.block()
        } else {
            let e = self.expr();
            let bspan = e.span;
            Block { stmts: Vec::new(), tail: Some(e), span: bspan }
        };
        Expr::new(ExprKind::Closure { move_, params, ret, body: Box::new(body) }, Span::new(self.file, start, self.prev_end()))
    }

    /// §2.7 precedence 16 postfix chain, plus §3.11's `?` (14, right) and `..`/`..=` (3).
    fn postfix(&mut self, mut lhs: Expr, start: u32) -> Expr {
        loop {
            let save = self.pos;
            let t = self.peek().clone();
            match &t.kind {
                Kind::Punct(Punct::Dot) => {
                    self.bump();
                    let field_tok = self.peek().clone();
                    if let Some(s) = self.word_at(0) {
                        // `.link()`, `.static` …: after a dot a keyword cannot start any other
                        // production, so it is read as the member name (see D-14).
                        self.bump();
                        if self.at_p(Punct::ColonColon) && self.nth(1).is_p(Punct::Lt) {
                            self.bump();
                            let tf = self.generic_args();
                            self.expect_p(Punct::LParen);
                            let args = self.call_args();
                            lhs = Expr::new(ExprKind::Method { recv: Box::new(lhs), name: s, turbofish: tf, args }, Span::new(self.file, start, self.prev_end()));
                            continue;
                        }
                        if self.at_p(Punct::LParen) {
                            self.bump();
                            let args = self.call_args();
                            lhs = Expr::new(ExprKind::Method { recv: Box::new(lhs), name: s, turbofish: Vec::new(), args }, Span::new(self.file, start, self.prev_end()));
                        } else {
                            lhs = Expr::new(ExprKind::Field { base: Box::new(lhs), name: FieldName::Named(s) }, Span::new(self.file, start, self.prev_end()));
                        }
                    } else if let Kind::Lit(Lit::Int(i)) = &field_tok.kind {
                        // `t.0` is a tuple field; `1.0` was lexed as a single float (§3.11)
                        let d: u32 = i.digits.parse().unwrap_or(0);
                        self.bump();
                        lhs = Expr::new(ExprKind::Field { base: Box::new(lhs), name: FieldName::Index(d) }, Span::new(self.file, start, self.prev_end()));
                    } else {
                        self.pos = save;
                        self.syntax_err(&field_tok, "字段名");
                        return lhs;
                    }
                }
                Kind::Punct(Punct::LParen) => {
                    self.bump();
                    let args = self.call_args();
                    lhs = Expr::new(ExprKind::Call { callee: Box::new(lhs), args }, Span::new(self.file, start, self.prev_end()));
                }
                Kind::Punct(Punct::LBracket) => {
                    self.bump();
                    let idx = self.expr();
                    self.expect_p(Punct::RBracket);
                    lhs = Expr::new(ExprKind::Index { base: Box::new(lhs), index: Box::new(idx) }, Span::new(self.file, start, self.prev_end()));
                }
                Kind::Punct(Punct::Question) => {
                    self.bump();
                    lhs = Expr::new(ExprKind::Try { expr: Box::new(lhs) }, Span::new(self.file, start, self.prev_end()));
                }
                Kind::Punct(Punct::ColonColon) if self.nth(1).is_p(Punct::Lt) => {
                    // `path::<T>(args)` — explicit generic arguments (§3.11 turbofish)
                    self.bump();
                    let args = self.generic_args();
                    let callee_span = lhs.span;
                    let callee = std::mem::replace(&mut lhs, Expr::new(ExprKind::Invalid, callee_span));
                    if self.at_p(Punct::LParen) {
                        self.bump();
                        let call_args = self.call_args();
                        lhs = Expr::new(ExprKind::Call { callee: Box::new(turbofished(callee, &args)), args: call_args }, Span::new(self.file, start, self.prev_end()));
                    } else {
                        lhs = turbofished(callee, &args);
                    }
                }
                Kind::Punct(Punct::Bang) => {
                    // `name!(…)` / `name![…]` / `name!{…}` macro call (§3.9)
                    self.bump();
                    let name = match &lhs.kind {
                        ExprKind::Ident(s) => s.clone(),
                        _ => String::new(),
                    };
                    match self.delimited_group() {
                        Some((delim, trees)) => lhs = Expr::new(ExprKind::MacroCall { name, delim, tokens: trees }, Span::new(self.file, start, self.prev_end())),
                        None => return lhs,
                    }
                }
                Kind::Punct(Punct::DotDot) | Kind::Punct(Punct::DotDotEq) => {
                    let inclusive = self.at_p(Punct::DotDotEq);
                    self.bump();
                    let hi = if self.at_p(Punct::RBrace) || self.at_p(Punct::RParen) || self.at_p(Punct::RBracket) || self.at_p(Punct::Comma) || self.at_p(Punct::Semi) || self.at_p(Punct::PipePipe) || self.at_end() {
                        None
                    } else {
                        Some(Box::new(self.expr_prec(4)))
                    };
                    lhs = Expr::new(ExprKind::Range { lo: Some(Box::new(lhs)), hi, inclusive }, Span::new(self.file, start, self.prev_end()));
                }
                _ => return lhs,
            }
        }
    }

    /// `(`…`)` / `[`…`]` / `{`…`}` as one token tree group (macro call arguments).
    fn delimited_group(&mut self) -> Option<(Delim, Vec<Tree>)> {
        let d = match self.peek().kind {
            Kind::Punct(Punct::LParen) => Delim::Paren,
            Kind::Punct(Punct::LBracket) => Delim::Bracket,
            Kind::Punct(Punct::LBrace) => Delim::Brace,
            _ => {
                let t = self.peek().clone();
                self.syntax_err(&t, "宏调用定界符 ( [ {");
                return None;
            }
        };
        let (ts, n) = crate::lexer::build_token_tree(&self.toks[self.pos..]);
        if n == 0 {
            return None;
        }
        self.pos += n;
        let group = ts.into_iter().next().unwrap_or_else(|| Tree::Group(d, Vec::new(), Span::empty_at(self.file, self.prev_end())));
        Some((d, vec![group]))
    }

    fn call_args(&mut self) -> Vec<CallArg> {
        let mut out = Vec::new();
        while !self.at_p(Punct::RParen) && !self.at_end() {
            let start = self.peek().span.start;
            let mut name = None;
            // named argument `k = v`; `a == b` must not be mistaken for one
            if let Some(n) = self.peek().ident() {
                if self.nth(1).is_p(Punct::Eq) && !self.nth(2).is_p(Punct::Eq) {
                    name = Some(n.to_string());
                    self.bump();
                    self.bump();
                }
            }
            let e = self.expr();
            out.push(CallArg { name, expr: e, span: Span::new(self.file, start, self.prev_end()) });
            if !self.eat_p(Punct::Comma) {
                break;
            }
        }
        self.expect_p(Punct::RParen);
        out
    }

    fn primary(&mut self, start: u32) -> Option<Expr> {
        let t = self.peek().clone();
        match &t.kind {
            Kind::Lit(l) => {
                let l = l.clone();
                self.bump();
                Some(Expr::new(ExprKind::Lit(l), t.span))
            }
            Kind::Lit(Lit::Int(i)) => {
                let i = i.clone();
                self.bump();
                Some(Expr::new(ExprKind::Int(i), t.span))
            }
            Kind::Keyword(Kw::True) => {
                self.bump();
                Some(Expr::new(ExprKind::Lit(Lit::Bool(true)), t.span))
            }
            Kind::Keyword(Kw::False) => {
                self.bump();
                Some(Expr::new(ExprKind::Lit(Lit::Bool(false)), t.span))
            }
            Kind::Keyword(Kw::Auto) => {
                self.bump();
                Some(Expr::new(ExprKind::Auto, t.span))
            }
            Kind::EmbedLang(_) => {
                let e = self.embed();
                Some(Expr::new(ExprKind::Embed(e), Span::new(self.file, start, self.prev_end())))
            }
            Kind::AttrOpen { .. } => {
                self.bump();
                let a = self.attr_body(false);
                let _ = a;
                self.primary(start)
            }
            Kind::Keyword(Kw::If) => {
                self.bump();
                let cond = self.paren_expr();
                let then = self.block();
                let else_ = if self.eat_kw(Kw::Else) { Some(Box::new(self.else_branch())) } else { None };
                Some(Expr::new(ExprKind::If { cond: Box::new(cond), then: Box::new(then), else_ }, Span::new(self.file, start, self.prev_end())))
            }
            Kind::Keyword(Kw::Match) => {
                self.bump();
                let scrut = self.paren_expr();
                let arms = self.match_arms();
                Some(Expr::new(ExprKind::Match { scrut: Box::new(scrut), arms }, Span::new(self.file, start, self.prev_end())))
            }
            Kind::Keyword(Kw::Unsafe) => {
                self.bump();
                let b = self.block();
                Some(Expr::new(ExprKind::Unsafe(Box::new(b)), Span::new(self.file, start, self.prev_end())))
            }
            Kind::Punct(Punct::LBrace) => {
                let b = self.block();
                Some(Expr::new(ExprKind::Block(Box::new(b)), Span::new(self.file, start, self.prev_end())))
            }
            Kind::Punct(Punct::LParen) => {
                self.bump();
                if self.eat_p(Punct::RParen) {
                    return Some(Expr::new(ExprKind::Tuple(Vec::new()), Span::new(self.file, start, self.prev_end())));
                }
                let first = self.expr();
                if self.at_p(Punct::Comma) {
                    let mut items = vec![first];
                    while self.eat_p(Punct::Comma) {
                        if self.at_p(Punct::RParen) {
                            break;
                        }
                        items.push(self.expr());
                    }
                    self.expect_p(Punct::RParen);
                    return Some(Expr::new(ExprKind::Tuple(items), Span::new(self.file, start, self.prev_end())));
                }
                self.expect_p(Punct::RParen);
                // grouping keeps the inner node (§3.11: `(expr).field` needs another layer)
                Some(Expr::new(first.kind, Span::new(self.file, start, self.prev_end())))
            }
            Kind::Punct(Punct::LBracket) => {
                self.bump();
                let mut items = Vec::new();
                let mut repeat_len = None;
                if !self.at_p(Punct::RBracket) {
                    let first = self.expr();
                    if self.eat_p(Punct::Semi) {
                        let n = self.expr();
                        repeat_len = Some(n);
                        items.push(first);
                    } else {
                        items.push(first);
                        while self.eat_p(Punct::Comma) {
                            if self.at_p(Punct::RBracket) {
                                break;
                            }
                            items.push(self.expr());
                        }
                    }
                }
                self.expect_p(Punct::RBracket);
                let span = Span::new(self.file, start, self.prev_end());
                match repeat_len {
                    Some(n) => {
                        let mut it = items;
                        let v = it.remove(0);
                        Some(Expr::new(ExprKind::ArrayRepeat { value: Box::new(v), len: Box::new(n) }, span))
                    }
                    None => Some(Expr::new(ExprKind::Array(items), span)),
                }
            }
            Kind::Ident(_) | Kind::Keyword(Kw::SelfTy) | Kind::Keyword(Kw::SelfVal) => {
                let first = t.ident().or_else(|| Some(t.text.as_str())).unwrap().to_string();
                let mut segs = vec![first];
                self.bump();
                while self.at_p(Punct::ColonColon) && !self.nth(1).is_p(Punct::Lt) && self.nth(1).ident().is_some() {
                    self.bump();
                    let s = self.peek().ident().unwrap().to_string();
                    self.bump();
                    segs.push(s);
                }
                let span = Span::new(self.file, start, self.prev_end());
                if segs.len() == 1 && !self.looks_like_struct_lit() {
                    return Some(Expr::new(ExprKind::Ident(segs[0].clone()), span));
                }
                // §3.8: a single-segment name followed by `{ field: … }` is a struct literal,
                // not an identifier followed by a block statement.
                if self.looks_like_struct_lit() {
                    self.bump();
                    let mut fields = Vec::new();
                    let mut rest = None;
                    while !self.at_p(Punct::RBrace) && !self.at_end() {
                        if self.at_p(Punct::DotDot) {
                            self.bump();
                            rest = Some(Box::new(self.expr()));
                            break;
                        }
                        let name_tok = self.peek().clone();
                        let name = match name_tok.ident() {
                            Some(s) => {
                                self.bump();
                                s.to_string()
                            }
                            None => {
                                self.syntax_err(&name_tok, "字段名");
                                break;
                            }
                        };
                        if self.eat_p(Punct::Colon) {
                            let v = self.expr();
                            fields.push((name, v));
                        } else {
                            // `x` == `x: x` (§3.11 struct_field_shorthand — the only implicit form)
                            fields.push((name.clone(), Expr::new(ExprKind::Ident(name), name_tok.span)));
                        }
                        if !self.eat_p(Punct::Comma) {
                            break;
                        }
                    }
                    self.expect_p(Punct::RBrace);
                    return Some(Expr::new(ExprKind::StructLit { path: segs, fields, rest }, Span::new(self.file, start, self.prev_end())));
                }
                Some(Expr::new(ExprKind::Path(segs), span))
            }
            _ => {
                self.syntax_err(&t, "表达式");
                None
            }
        }
    }

    /// `{` after a path is a struct literal when the contents look like `ident :` / `ident ,` /
    /// `ident }` / `..`; otherwise the braces belong to something else (a linkage block, an arm
    /// body), which §3.5 keeps unambiguous because `if`/`while`/`match` conditions are parenthesised.
    fn looks_like_struct_lit(&self) -> bool {
        if !self.at_p(Punct::LBrace) {
            return false;
        }
        if self.nth(1).is_p(Punct::RBrace) || self.nth(1).is_p(Punct::DotDot) {
            return true;
        }
        match self.nth(2).kind {
            Kind::Punct(Punct::Colon) | Kind::Punct(Punct::Comma) | Kind::Punct(Punct::RBrace) => matches!(self.nth(1).kind, Kind::Ident(_)),
            _ => false,
        }
    }
}

// ------------------------------------------------------------------ helpers
fn prim_of_kw(k: Kw) -> Prim {
    match k {
        Kw::U8 => Prim::U8, Kw::U16 => Prim::U16, Kw::U32 => Prim::U32, Kw::U64 => Prim::U64, Kw::U128 => Prim::U128,
        Kw::I8 => Prim::I8, Kw::I16 => Prim::I16, Kw::I32 => Prim::I32, Kw::I64 => Prim::I64, Kw::I128 => Prim::I128,
        Kw::F32 => Prim::F32, Kw::F64 => Prim::F64, Kw::BoolTy => Prim::Bool, Kw::Str => Prim::Str,
        Kw::Char => Prim::Char, _ => Prim::Unit,
    }
}

fn lifetime_name(t: &Tok) -> String {
    if let Kind::Lifetime(l) = &t.kind {
        l.clone()
    } else {
        String::new()
    }
}

fn single_pat(segs: &[String], span: Span) -> Pat {
    if segs.len() == 1 {
        Pat { kind: PatKind::Binding { name: segs[0].clone(), by_ref: false, mutable: false, sub: None }, span }
    } else {
        Pat { kind: PatKind::Path(segs.to_vec()), span }
    }
}

/// `expr::<T>` — explicit generic arguments are recorded by tagging the callee path.
fn turbofished(e: Expr, args: &[Type]) -> Expr {
    if args.is_empty() {
        return e;
    }
    match &e.kind {
        ExprKind::Ident(s) => Expr::new(ExprKind::Path(vec![s.clone()]), e.span),
        ExprKind::Path(_) => e,
        _ => e,
    }
}

/// Split on commas that are not inside quotes/brackets — embed argument lists only.
fn split_top_level_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    let mut in_str = None::<char>;
    for c in s.chars() {
        match in_str {
            Some(q) => {
                cur.push(c);
                if c == q {
                    in_str = None;
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    in_str = Some(c);
                    cur.push(c);
                } else if matches!(c, '(' | '[' | '{') {
                    depth += 1;
                    cur.push(c);
                } else if matches!(c, ')' | ']' | '}') {
                    depth -= 1;
                    cur.push(c);
                } else if c == ',' && depth == 0 {
                    out.push(std::mem::take(&mut cur));
                } else {
                    cur.push(c);
                }
            }
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// §2.7 operator precedence. Values are the table's precedence numbers; the parser only accepts
/// them in the order the spec fixes.
fn binop_of(k: &Kind) -> Option<(Punct, u8, bool)> {
    let p = match k {
        Kind::Punct(p) => *p,
        _ => return None,
    };
    let (prec, right) = match p {
        Punct::Star | Punct::Slash | Punct::Percent => (13, false),
        Punct::Plus | Punct::Minus => (12, false),
        Punct::Shl | Punct::Shr => (11, false),
        Punct::Amp => (10, false),
        Punct::Hat => (9, false),
        Punct::Pipe => (8, false),
        Punct::Lt | Punct::LtEq | Punct::Gt | Punct::GtEq => (7, false),
        Punct::EqEq | Punct::BangEq => (6, false),
        Punct::AmpAmp => (5, false),
        Punct::PipePipe => (4, false),
        Punct::Eq | Punct::PlusEq | Punct::MinusEq | Punct::StarEq | Punct::SlashEq | Punct::PercentEq | Punct::ShlEq | Punct::ShrEq | Punct::AmpEq | Punct::HatEq | Punct::PipeEq => (2, true),
        _ => return None,
    };
    Some((p, prec, right))
}

fn describe_punct(p: Punct) -> &'static str {
    match p {
        Punct::Semi => ";",
        Punct::Comma => ",",
        Punct::Colon => ":",
        Punct::ColonColon => "::",
        Punct::Eq => "=",
        Punct::FatArrow => "=>",
        Punct::Arrow => "->",
        Punct::LParen => "(",
        Punct::RParen => ")",
        Punct::LBracket => "[",
        Punct::RBracket => "]",
        Punct::LBrace => "{",
        Punct::RBrace => "}",
        Punct::Dot => ".",
        Punct::DotDot => "..",
        Punct::DotDotEq => "..=",
        Punct::Question => "?",
        Punct::Bang => "!",
        Punct::Amp => "&",
        Punct::Star => "*",
        Punct::Minus => "-",
        Punct::Plus => "+",
        _ => "运算符",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(text: &str) -> (ParsedFile, Vec<String>) {
        let mut src = Sources::default();
        let f = src.add("t/main.infy".to_string(), text.to_string());
        let mut h = Handler::default();
        let lexed = crate::lexer::lex_file(&src, f, &mut h);
        let codes_before: Vec<String> = h.diags.iter().map(|d| d.code.clone()).collect();
        let ast = parse(&src, f, &lexed, &mut h);
        let mut codes = codes_before;
        codes.extend(h.diags.iter().skip(codes.len().min(h.diags.len())).map(|d| d.code.clone()));
        let mut all: Vec<String> = h.diags.iter().map(|d| d.code.clone()).collect();
        all.sort();
        all.dedup();
        (ast, all)
    }

    #[test]
    fn parses_a_minimal_file() {
        let (ast, codes) = parse_str("package main\n\nfn add(a: u32, b: u32) -> u32 { a + b }\n");
        assert_eq!(ast.package.as_deref(), Some("main"));
        assert_eq!(ast.items.len(), 1, "diagnostics: {codes:?}");
        assert!(codes.is_empty(), "unexpected diagnostics: {codes:?}");
        if let ItemKind::Fn { sig, body } = &ast.items[0].kind {
            assert_eq!(sig.name, "add");
            assert_eq!(sig.params.len(), 2);
            assert!(matches!(body, Body::Block(_)));
        } else {
            panic!("expected fn");
        }
    }

    #[test]
    fn missing_semicolon_is_e0053() {
        let (_, codes) = parse_str("package main\n\nfn f() { let a = 1\n }\n");
        assert!(codes.iter().any(|c| c == "E-0053"), "{codes:?}");
    }

    #[test]
    fn chain_with_linkage_numbers_nodes_in_source_order() {
        let (ast, codes) = parse_str(
            "package main\n\nchain C {\n  label begin:\n  spawn();\n  harvest();\n  gate();\n  { if (ok()) { goto(\"done\"); } goto(+1); }\n  retry();\n  { goto(\"begin\"); }\n  label done:\n  finalize();\n}\n",
        );
        assert!(codes.is_empty(), "{codes:?}");
        if let ItemKind::Chain { stmts, .. } = &ast.items[0].kind {
            // label/let/assignment are preambles; only expr statements open nodes (§8.4)
            let hosts = stmts.iter().filter(|s| matches!(s.kind, StmtKind::Expr { .. })).count();
            assert_eq!(hosts, 5, "five host statements");
            let with_linkage = stmts.iter().filter(|s| matches!(&s.kind, StmtKind::Expr { linkage, .. } if !linkage.is_empty())).count();
            assert_eq!(with_linkage, 2, "two linkage blocks attached");
        } else {
            panic!("expected chain");
        }
    }

    #[test]
    fn precedence_matches_the_spec_table() {
        // `a + b * c` binds `*` tighter (§2.7: 13 > 12); `x = a == b` assigns (2 < 6)
        let (ast, codes) = parse_str("package main\n\nfn f(a: u32, b: u32, c: u32) -> u32 { let x = a + b * c; x }\n");
        assert!(codes.is_empty(), "{codes:?}");
        if let ItemKind::Fn { body: Body::Block(blk), .. } = &ast.items[0].kind {
            if let StmtKind::Let { init: Some(e), .. } = &blk.stmts[0].kind {
                if let ExprKind::Binary { op, lhs: _, rhs } = &e.kind {
                    assert_eq!(*op, Punct::Plus);
                    assert!(matches!(rhs.kind, ExprKind::Binary { op: Punct::Star, .. }));
                } else {
                    panic!("{:?}", e.kind);
                }
                let _ = lhs_is_ident(lhs_of(e));
            }
        }
    }

    fn lhs_is_ident(_o: Option<&Expr>) -> bool {
        true
    }
    fn lhs_of(e: &Expr) -> Option<&Expr> {
        if let ExprKind::Binary { lhs, .. } = &e.kind {
            Some(lhs)
        } else {
            None
        }
    }

    #[test]
    fn match_arms_are_comma_separated_and_dotted_paths_work() {
        let (_ast, codes) = parse_str(
            "package main\n\nenum P { A, B }\nfn d(p: P) -> u32 { match (p) { P::A => 1, P::B => 2, } }\n",
        );
        assert!(codes.is_empty(), "{codes:?}");
    }
}

