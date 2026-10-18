//! §2 lexical conventions — the complete lexer.
//!
//! ## Pipeline contract (two entry points, one E-0002)
//!
//! * [`check_utf8`] inspects **raw bytes**. The CLI (`infy build`, §13.4) reads a file as
//!   `Vec<u8>` and calls it *before* decoding: a `String` cannot hold invalid UTF-8, so once the
//!   bytes have been decoded the E-0002 condition is unrecoverable. When it returns `Some((off,
//!   len))` the CLI emits E-0002 (see [`lex_bytes`, which does exactly that]) and must not lex.
//! * [`lex_file`] re-runs the same check defensively on `Sources::get(file).text`; for a `String`
//!   it is a no-op, but a caller that stored re-encoded/lossy text still gets a well-formed
//!   diagnostic instead of silently mis-lexing.
//!
//! The token stream is always returned (even after errors) so `infy fmt` and the parser can
//! recover; §9.4 requires `check` and `build` to agree on a file's diagnostic set.
//!
//! ## What the lexer does NOT decide
//!
//! * `E-0042`/`E-0043` (integer range) — context dependent: §2.8's `-128i8` exception means a
//!   unary minus can make an otherwise overflowing literal legal, so the lexer only fills
//!   `IntLit { digits, radix, suffix, value }` (`value: None` == does not fit `u128`).
//! * `E-0008` (ambiguous unsuffixed integer) — inference, §4.13.
//! * `E-0006` / the *pairwise* half of `E-0050` (two identifiers colliding) — needs a scope. The
//!   lexer exposes [`skeleton`] (confusable skeleton) and [`nfkc_approx`] so sema can do it.
//!   The *single-identifier* half (mixed script inside one identifier) IS lexical and is checked
//!   here.
//! * `W-1010` (unused lifetime), `E-0053` (missing `;`), `E-0011` vs `W-0003` evaluation, and the
//!   whole constraint *expression* parse — parser / sema / `pkg`.
//! * `E-0012` (unknown build-constraint name) is only emitted by [`lex_file_with_features`]:
//!   `features` live in the module manifest (§5.2), invisible to [`lex_file`]. `pkg` either calls
//!   the features-aware entry point or runs [`check_constraint_names`] itself.

use std::collections::HashMap;

use crate::diag::{Diagnostic, Handler, Level, Phase};
use crate::span::{FileId, SourceFile, Sources, Span, Position};
use crate::token::{
    Comment, CommentKind, Delim, FloatLit, IntLit, Kind, Kw, Lit, Punct, Radix, Tok, Tree,
};

#[derive(Clone, Debug, Default)]
pub struct Lexed {
    pub toks: Vec<Tok>,
    pub comments: Vec<Comment>,
    /// `#!` line, if the file starts with a shebang (§13.1: the compiler ignores it).
    pub shebang: Option<String>,
    /// `//infy:build <constraint>` (§2.2): the raw constraint text, its 1-based line, its span.
    /// The *expression* is parsed by `pkg` against the closed constraint set; the lexer owns the
    /// placement (`E-0011`), duplication (`E-0013`) and unknown-name (`E-0012`) checks.
    pub build_constraint: Option<(String, u32, Span)>,
    /// Diagnostics already emitted while lexing (the caller keeps `handler` authoritative; this is
    /// the count, used by conformance cases that assert a file is lex-clean).
    pub diagnostics_emitted: usize,
}

/// §2.1: validate raw bytes. `Some((byte_offset_of_first_invalid_byte, length_of_invalid_seq))`.
/// `len` is the remaining byte count when the sequence is "incomplete but not yet wrong"
/// (a truncated multi-byte sequence at EOF).
pub fn check_utf8(bytes: &[u8]) -> Option<(usize, usize)> {
    match std::str::from_utf8(bytes) {
        Ok(_) => None,
        Err(e) => Some((
            e.valid_up_to(),
            e.error_len().unwrap_or(bytes.len() - e.valid_up_to()),
        )),
    }
}

/// Byte offsets are `u32` in `Span`; clamp instead of truncating.
fn u32o(off: usize) -> u32 {
    off.min(u32::MAX as usize) as u32
}

/// §2.2 closed constraint set. `ignore` (§2.2's own example) and `test` (§14.7's
/// `//infy:build test`) are used by the spec but missing from its list on line 96; they are
/// admitted by `pkg::BASE_CONSTRAINTS`, which is the single truth both `E-0012` checks read.
pub fn known_constraint_names() -> &'static [&'static str] {
    crate::pkg::BASE_CONSTRAINTS
}

/// §2.2: extract the identifiers of a constraint expression (`ident | "!" constraint |
/// "(" constraint ")" | constraint ("&&"|",") constraint | constraint "||" constraint`) and
/// return the names that are neither in [`known_constraint_names`] nor in `features`.
///
/// Order is first-appearance and duplicates are collapsed, so `pkg`'s E-0012 rendering is
/// deterministic (§7.3). Garbage between names is ignored — the *expression* parser is `pkg`'s.
pub fn check_constraint_names(text: &str, features: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        if !(c == '_' || is_xid_start(c)) {
            i += 1;
            continue;
        }
        let mut w = String::new();
        while i < bytes.len() && (bytes[i] == '_' || is_xid_continue(bytes[i])) {
            w.push(bytes[i]);
            i += 1;
        }
        if known_constraint_names().contains(&w.as_str()) || features.contains(&w.as_str()) {
            continue;
        }
        if !out.iter().any(|s| s == &w) {
            out.push(w);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Unicode predicates (§2.3 / §2.10). std only: XID is approximated with `char` predicates plus
// explicit tables for the scripts the spec names (Han / Hiragana / Katakana / Hangul).
// ---------------------------------------------------------------------------------------------

/// §2.3 `XID_Start`: Unicode category L (Han, Hiragana, Katakana and Hangul are `Lo`, and
/// `char::is_alphabetic()` reports true for them), letter numerals, plus the `_` §2.3 makes legal.
/// Combining marks are excluded **explicitly**: `char::is_alphabetic()` answers `true` for several
/// `Mn` characters (U+093B, U+0E31 …) that XID_Start does not admit.
pub fn is_xid_start(c: char) -> bool {
    if c == '_' {
        return true;
    }
    if is_combining(c) {
        return false;
    }
    if c.is_alphabetic() {
        return true;
    }
    // Explicit backstop for the §2.10 scripts, independent of what `is_alphabetic` may cover.
    matches!(c as u32,
        0x3005..=0x3007 | 0x3021..=0x3029 | 0x3038..=0x303C
        | 0x3041..=0x3096 | 0x309D..=0x309F | 0x30A1..=0x30FA | 0x30FC..=0x30FF
        | 0x3105..=0x312F | 0x3131..=0x318E | 0x31A0..=0x31BF
        | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xA000..=0xA48C
        | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0x20000..=0x2FA1F)
}

/// §2.3 `XID_Continue` = `XID_Start` + marks + Nd.
pub fn is_xid_continue(c: char) -> bool {
    is_xid_start(c) || c.is_numeric() || is_combining(c) || matches!(c as u32, 0x00B7 | 0x0387)
}

fn is_ascii_digit(c: char) -> bool {
    matches!(c, '0'..='9')
}

/// §2.10: zero-width and directional-control characters that may not appear outside string
/// literals: U+200B..U+200F, U+202A..U+202E, U+FEFF (the leading BOM excepted, §2.1).
fn is_invisible(c: char) -> bool {
    matches!(c as u32, 0x200B..=0x200F | 0x202A..=0x202E | 0xFEFF)
}

/// Approximate "mark" test (Mn/Me/Mc + default-ignorable), used by [`skeleton`] and by the
/// grapheme-cluster check of §2.9.
fn is_combining(c: char) -> bool {
    let x = c as u32;
    x == 0x200D
        || matches!(x,
            0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x05BF
            | 0x05C1..=0x05C2 | 0x05C4..=0x05C5 | 0x05C7 | 0x0610..=0x061A
            | 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06DC | 0x06DF..=0x06E4
            | 0x06E7..=0x06E8 | 0x06EA..=0x06ED | 0x0711 | 0x0730..=0x074A
            | 0x07A6..=0x07B0 | 0x0900..=0x0903 | 0x093A..=0x094F | 0x0951..=0x0957
            | 0x0962..=0x0963 | 0x0A01..=0x0A03 | 0x0A70..=0x0A71 | 0x0B01..=0x0B03
            | 0x0C00..=0x0C03 | 0x0D01..=0x0D03 | 0x0E31 | 0x0E34..=0x0E3A
            | 0x0E47..=0x0E4E | 0x0EB1 | 0x0EB4..=0x0EBC | 0x0EC8..=0x0ECD
            | 0x102B..=0x103E | 0x1056..=0x1059
            | 0x135D..=0x135F | 0x1712..=0x1714 | 0x17B4..=0x17D3 | 0x180B..=0x180D
            | 0x18A9 | 0x1920..=0x193B | 0x1A17..=0x1A1B | 0x1B00..=0x1B04
            | 0x1DC0..=0x1DF9 | 0x20D0..=0x20F0 | 0x2CEF..=0x2CF1 | 0x2D7F
            | 0x302A..=0x302E | 0x3099..=0x309C | 0xA66F..=0xA67F | 0xFB1E
            | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F)
}

/// Script classification for §2.10's mixed-script rule. Anything not tabulated is `Neutral`, so
/// an unknown script never produces a false accusation — only the named collisions do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    Neutral,
    Latin,
    Cyrillic,
    Greek,
    Han,
    Hiragana,
    Katakana,
    Hangul,
    Other(u32),
}

pub fn script_of(c: char) -> Script {
    let x = c as u32;
    if is_combining(c) || c.is_numeric() || c == '_' || c == '\'' {
        return Script::Neutral;
    }
    match x {
        0x0041..=0x005A | 0x0061..=0x007A | 0x00AA | 0x00BA | 0x00C0..=0x00D6
        | 0x00D8..=0x00F6 | 0x00F8..=0x02AF | 0x1E00..=0x1EFF | 0x2C60..=0x2C7F | 0xA720..=0xA7FF => {
            Script::Latin
        }
        0x0250..=0x02FF => Script::Latin,
        0x0400..=0x04FF | 0x0500..=0x052F | 0x2DE0..=0x2DFF | 0xA650..=0xA69F => Script::Cyrillic,
        0x0370..=0x03FF | 0x1F00..=0x1FFF => Script::Greek,
        0x3005..=0x3007 | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
        | 0x20000..=0x2A6DF | 0x2A700..=0x2EBEF | 0x2F800..=0x2FA1F | 0x30000..=0x3134A => {
            Script::Han
        }
        0x3041..=0x3096 | 0x309D..=0x309F => Script::Hiragana,
        0x30A1..=0x30FA | 0x30FC..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9D => Script::Katakana,
        0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7A3 | 0xD7B0..=0xD7FF => {
            Script::Hangul
        }
        0x0590..=0x05FF => Script::Other(0x0590),
        0x0600..=0x06FF | 0x0750..=0x077F => Script::Other(0x0600),
        0x0900..=0x097F => Script::Other(0x0900),
        0x0530..=0x058F => Script::Other(0x0530),
        0x10A0..=0x10FF => Script::Other(0x10A0),
        0x0E00..=0x0E7F => Script::Other(0x0E00),
        _ if c.is_alphabetic() => Script::Other(x & 0xFF00),
        _ => Script::Neutral,
    }
}

/// Han + kana may mix (§2.10: the Japanese case is legal); everything else must not.
fn scripts_compatible(set: &[Script]) -> bool {
    use Script::*;
    if set.len() <= 1 {
        return true;
    }
    let in_group = |s: Script| matches!(s, Han | Hiragana | Katakana);
    set.iter().all(|s| in_group(*s))
}

fn scripts_are_confusable_family(set: &[Script]) -> bool {
    use Script::*;
    set.iter()
        .all(|s| matches!(s, Latin | Cyrillic | Greek))
}

/// Compatibility decomposition subset that std can supply without Unicode data: fullwidth
/// forms, halfwidth Hangul jamo, a few ligatures and spaces. **Documented limitation**: std has
/// no normalization API, so this is not a conforming NFKC; it is deterministic and covers the
/// collisions the spec's examples exercise (`ＥＦＦ`, `ﬁ`, NBSP).
pub fn nfkc_approx(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let x = c as u32;
        match x {
            0xFF01..=0xFF5E => out.push(char::from_u32(x - 0xFEE0).unwrap_or(c)),
            0xFF61..=0xFFDC => out.push(c), // halfwidth kana: kept (script table maps to Katakana)
            0x00A0 | 0x2007 | 0x202F | 0x205F | 0x3000 => out.push(' '),
            0xFB00 => out.push_str("ff"),
            0xFB01 => out.push_str("fi"),
            0xFB02 => out.push_str("fl"),
            0xFB03 => out.push_str("ffi"),
            0xFB04 => out.push_str("ffl"),
            0xFB05 | 0xFB06 => out.push_str("st"),
            0x2026 => out.push_str("..."),
            0x2126 => out.push('\u{3A9}'),
            0x212A => out.push('K'),
            0x2100 => out.push_str("a/c"),
            0x2101 => out.push_str("a/s"),
            0x2105 => out.push_str("c/o"),
            _ => out.push(c),
        }
    }
    out
}

/// Common ASCII homoglyph folds so `skeleton` actually catches the classic look-alikes.
fn fold_confusable(c: char) -> char {
    match c {
        // Cyrillic
        '\u{0410}' => 'a',
        '\u{0430}' => 'a',
        '\u{0415}' => 'e',
        '\u{0435}' => 'e',
        '\u{0401}' => 'e',
        '\u{0451}' => 'e',
        '\u{041E}' => 'o',
        '\u{043E}' => 'o',
        '\u{0420}' => 'p',
        '\u{0440}' => 'p',
        '\u{0421}' => 'c',
        '\u{0441}' => 'c',
        '\u{0423}' => 'y',
        '\u{0443}' => 'y',
        '\u{0425}' => 'x',
        '\u{0445}' => 'x',
        '\u{0426}' => 'u',
        '\u{043A}' => 'k',
        '\u{041C}' => 'm',
        '\u{043C}' => 'm',
        '\u{041B}' => 'n',
        '\u{043B}' => 'n',
        '\u{0406}' => 'i',
        '\u{0456}' => 'i',
        '\u{0407}' => 'j',
        '\u{0457}' => 'j',
        '\u{04BB}' => 'n',
        '\u{051D}' => 'l',
        // Greek
        '\u{0391}' => 'a',
        '\u{03B1}' => 'a',
        '\u{0395}' => 'e',
        '\u{039F}' => 'o',
        '\u{03BF}' => 'o',
        '\u{03A1}' => 'p',
        '\u{03C1}' => 'p',
        '\u{0392}' => 'b',
        '\u{0396}' => 'z',
        '\u{03B9}' => 'i',
        '\u{03C5}' => 'u',
        '\u{03BA}' => 'k',
        '\u{03AE}' => 'h',
        '\u{03A5}' => 'Y',
        // Latin lookalikes / modifier letters
        '\u{0131}' => 'i',
        '\u{01C0}' => 'l',
        '\u{0269}' => 'i',
        '\u{026B}' => 'l',
        '\u{0271}' => 'q',
        '\u{1D0A}' => 'j',
        '\u{1D0F}' => 'o',
        '\u{1D1C}' => 'u',
        '\u{1E9F}' => 'd',
        '\u{2C62}' => 'r',
        '\u{A722}' => 'A',
        _ => c,
    }
}

/// §2.10 confusable skeleton: NFKC (approximated by [`nfkc_approx`]) → case fold → strip
/// combining marks and default-ignorables → fold known homoglyphs.
///
/// Two identifiers in the same scope whose skeletons are equal are confusables and `sema`
/// reports `E-0050`; the lexer cannot see a scope, so it only exposes this function (and checks
/// the *single-identifier* mixed-script half of §2.10 directly).
pub fn skeleton(ident: &str) -> String {
    let folded = nfkc_approx(ident);
    let lowered: String = folded.chars().flat_map(|c| c.to_lowercase()).collect();
    let mut out = String::with_capacity(lowered.len());
    for c in lowered.chars() {
        if is_combining(c) || is_invisible(c) {
            continue;
        }
        out.push(fold_confusable(c));
    }
    out
}

/// §2.10 / §2.9 `cord_lit` normalisation: NFC-normalise the value for `==` comparison. std
/// exposes no normalization, so this is [`nfkc_approx`] (stronger than NFC, and documented as an
/// approximation of it).
pub fn normalize_cord(s: &str) -> String {
    nfkc_approx(s)
}

// ---------------------------------------------------------------------------------------------
// The lexer proper
// ---------------------------------------------------------------------------------------------

/// Per-code diagnostic cap: a pathological file must not print thousands of identical lexemes.
const MAX_PER_CODE: usize = 16;
/// Global cap so a runaway file still terminates quickly.
const MAX_TOTAL: usize = 256;
/// §2.2 nesting limit for block comments.
const MAX_COMMENT_DEPTH: usize = 255;

struct Lexer<'a> {
    sources: &'a Sources,
    src: &'a SourceFile,
    text: &'a str,
    file: FileId,
    pos: usize,
    /// Byte offset where the "first non-blank line" search for `//infy:build` starts (§2.2): after
    /// an optional BOM and an optional shebang line.
    region_start: usize,
    toks: Vec<Tok>,
    comments: Vec<Comment>,
    shebang: Option<String>,
    build_constraint: Option<(String, u32, Span)>,
    features: &'a [&'a str],
    check_e0012: bool,
    emitted: usize,
    per_code: HashMap<String, usize>,
}

impl<'a> Lexer<'a> {
    fn new(
        sources: &'a Sources,
        file: FileId,
        features: &'a [&'a str],
        check_e0012: bool,
    ) -> Option<Lexer<'a>> {
        let src = sources.get(file)?;
        let text = src.text.as_str();
        let mut lx = Lexer {
            sources,
            src,
            text,
            file,
            pos: 0,
            region_start: 0,
            toks: Vec::new(),
            comments: Vec::new(),
            shebang: None,
            build_constraint: None,
            features,
            check_e0012,
            emitted: 0,
            per_code: HashMap::new(),
        };
        // §2.1 optional BOM: exactly one leading U+FEFF.
        if text.starts_with('\u{FEFF}') {
            lx.pos = 3;
            lx.region_start = 3;
        }
        Some(lx)
    }

    // -- tiny cursor helpers (never panic on a non-boundary index) -----------------------------

    fn at_eof(&self) -> bool {
        self.pos >= self.text.len()
    }
    fn cur(&self) -> Option<char> {
        self.text[self.pos.min(self.text.len())..].chars().next()
    }
    fn peek_at(&self, off: usize) -> Option<char> {
        self.text.get(off..)?.chars().next()
    }
    fn starts(&self, s: &str) -> bool {
        self.text[self.pos.min(self.text.len())..].starts_with(s)
    }
    fn advance(&mut self, n: usize) {
        self.pos = (self.pos + n).min(self.text.len());
    }
    fn bump_char(&mut self) {
        if let Some(c) = self.cur() {
            self.advance(c.len_utf8());
        }
    }
    fn slice(&self, a: usize, b: usize) -> String {
        let lo = self.floor_boundary(a.min(self.text.len()));
        let hi = self.ceil_boundary(b.min(self.text.len()).max(lo));
        self.text[lo..hi].to_string()
    }
    fn floor_boundary(&self, i: usize) -> usize {
        let mut j = i.min(self.text.len());
        while j > 0 && !self.text.is_char_boundary(j) {
            j -= 1;
        }
        j
    }
    fn ceil_boundary(&self, i: usize) -> usize {
        let mut j = i.min(self.text.len());
        while j < self.text.len() && !self.text.is_char_boundary(j) {
            j += 1;
        }
        j
    }

    // -- diagnostics ---------------------------------------------------------------------------

    fn mk(&self, code: &str, start: usize, end: usize, title: &str) -> Diagnostic {
        let s = Span::new(
            self.file,
            u32o(self.floor_boundary(start)),
            u32o(self.ceil_boundary(end.max(start))),
        );
        let (p0, p1) = self.sources.line_col(s);
        let (cs, ce) = self.sources.columns(s);
        let label: String = title
            .split(['：', '（', '('])
            .next()
            .unwrap_or(title)
            .chars()
            .take(28)
            .collect();
        Diagnostic::new(code, Phase::Lex, self.file, p0)
            .titled(title.to_string())
            .region_multi(self.file, p0.line, p1.line, cs, ce, label)
    }

    fn emit(&mut self, h: &mut Handler, d: Diagnostic) {
        if self.emitted >= MAX_TOTAL {
            return;
        }
        let code = d.code.clone();
        let n = self.per_code.get(&code).copied().unwrap_or(0) + 1;
        self.per_code.insert(code.clone(), n);
        self.emitted += 1;
        if n == MAX_PER_CODE {
            h.emit(
                d.clone()
                    .note(format!("`{code}` 诊断已达单文件上限 {MAX_PER_CODE}，后续同类不再输出")),
            );
        } else if n < MAX_PER_CODE {
            h.emit(d);
        }
    }

    /// §2.10: an invisible/directional character, or any character outside INFY's lexical
    /// charset, met outside a string literal. There is no dedicated code for "stray character" in
    /// §9.3/§9.7 and inventing one is forbidden, so E-0052 — the code that owns "this character
    /// may not appear in source" — carries both, with distinct titles.
    fn illegal_char(&mut self, h: &mut Handler, what: &str, help: &str) {
        let start = self.pos;
        let c = self.cur().unwrap_or(' ');
        self.bump_char();
        let title = if is_invisible(c) {
            format!("零宽/方向控制字符 U+{:04X} 出现在源码中", c as u32)
        } else {
            format!("源码中出现非法字符 `{c}`（{what}）")
        };
        let d = self
            .mk("E-0052", start, self.pos, &title)
            .note("词法不透明区（字符串/字节串/原字符串/注释正文/`@embed` 体）之外，该字符不合法（§2.10）")
            .help(help);
        self.emit(h, d);
    }

    fn scan_invisibles_in(&mut self, h: &mut Handler, a: usize, b: usize) {
        let mut i = a;
        while i < b {
            let Some(c) = self.peek_at(i) else { break };
            if is_invisible(c) {
                let title = format!("零宽/方向控制字符 U+{:04X} 出现在注释中", c as u32);
                let d = self.mk("E-0052", i, i + c.len_utf8(), &title).help(
                    "删除它；确需该字符请放进字符串字面量（§2.10，Trojan Source 防护）",
                );
                self.emit(h, d);
            }
            i += c.len_utf8();
        }
    }

    // -- §2.1 file shape -----------------------------------------------------------------------

    fn file_shape_checks(&mut self, h: &mut Handler) {
        let body = &self.text[self.region_start..];
        let base = self.region_start;
        // §2.2: whitespace-only / empty file is E-0090 ("0 字节的 .infy"); a file whose only
        // content is blank lines has no lexical content either, so the same code reports it.
        if body.is_empty() || body.chars().all(|c| c.is_whitespace()) {
            let d = self
                .mk("E-0090", base, base + body.len(), "空源码文件（0 个有效字节）")
                .note("§9.7：0 字节的 `.infy` 必须删除或写入 `package`")
                .help("写入 `package <name>`，或删除该文件");
            self.emit(h, d);
            return;
        }
        let mut crlf: Vec<usize> = Vec::new();
        let bytes = body.as_bytes();
        for (i, b) in bytes.iter().enumerate() {
            if *b != b'\r' {
                continue;
            }
            if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                crlf.push(base + i);
            } else {
                let off = base + i;
                let d = self
                    .mk("E-0003", off, off + 1, "CR 换行（Mac 老式）不合法，强制 LF")
                    .note("§2.1：源码必须使用 LF(`\\n`) 行结束符")
                    .help("改为 LF（`infy fmt` 自动修复）");
                self.emit(h, d);
            }
        }
        if !crlf.is_empty() {
            // One warning per file, at the first CRLF, with the total in the title (§9.2 renders it).
            let off = crlf[0];
            let mut d = self
                .mk("W-0001", off, off + 2, &format!("CRLF 行结束符：应为 LF（共 {} 处）", crlf.len()))
                .note("§2.1：强制 LF；CRLF 为警告，`infy fmt` 自动修复")
                .help("运行 `infy fmt`，或把行结束符改为 LF");
            d.code = "W-0001".to_string();
            d.level = Level::Warn;
            self.emit(h, d);
        }
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            let off = self.text.len().saturating_sub(1);
            let d = self
                .mk("W-0002", off, off + 1, "文件结尾缺少一个 trailing LF")
                .note("§2.1：文件结尾需一个 trailing LF（缺失为警告）")
                .help("在文件末尾补一个换行（`infy fmt` 自动补）");
            let mut d = d;
            d.level = Level::Warn;
            self.emit(h, d);
        }
    }

    fn take_shebang(&mut self) {
        // §13.1: a first line `#!...` is ignored. `#![` is an inner attribute, not a shebang.
        if !self.starts("#!") || self.starts("#![") {
            return;
        }
        let start = self.pos;
        let rest = &self.text[start..];
        let end = start + rest.find('\n').unwrap_or(rest.len());
        self.shebang = Some(self.slice(start, end));
        self.pos = end;
        self.region_start = end;
    }

    // -- §2.2 comments -------------------------------------------------------------------------

    fn lex_line_comment(&mut self, h: &mut Handler) {
        let start = self.pos;
        let mut kind = CommentKind::Line;
        let mut marker_end = start + 2;
        if self.peek_at(start + 2) == Some('/') && self.peek_at(start + 3) != Some('/') {
            kind = CommentKind::Doc;
            marker_end = start + 3;
        }
        let rest = &self.text[marker_end.min(self.text.len())..];
        let line_end = marker_end + rest.find('\n').unwrap_or(rest.len());
        let text = self.slice(marker_end, line_end);
        self.pos = line_end;
        self.scan_invisibles_in(h, marker_end, line_end);
        if kind == CommentKind::Line {
            self.maybe_build_constraint(h, start, line_end, &text);
        }
        self.comments.push(Comment {
            kind,
            span: Span::new(self.file, u32o(start), u32o(line_end)),
            text,
        });
    }

    fn lex_block_comment(&mut self, h: &mut Handler) {
        let start = self.pos;
        self.pos += 2;
        let body_start = self.pos;
        let mut depth: usize = 1;
        let mut deep_reported = false;
        let mut closed = false;
        let mut end = self.text.len();
        while !self.at_eof() {
            if self.starts("/*") {
                depth += 1;
                if depth > MAX_COMMENT_DEPTH && !deep_reported {
                    let d = self
                        .mk("E-0005", self.pos, self.pos + 2, &format!(
                            "注释嵌套过深（第 {} 层 > 上限 {MAX_COMMENT_DEPTH}）",
                            depth
                        ))
                        .note("§2.2：`/* */` 可嵌套，但深度上限 255，超限即 E-0005")
                        .help("简化注释");
                    self.emit(h, d);
                    deep_reported = true;
                }
                self.pos += 2;
                continue;
            }
            if self.starts("*/") {
                depth -= 1;
                self.pos += 2;
                if depth == 0 {
                    closed = true;
                    end = self.pos;
                    break;
                }
                continue;
            }
            self.bump_char();
        }
        if !closed {
            let d = self
                .mk("E-0004", start, self.text.len(), "未闭合块注释：`/*` 无匹配 `*/`")
                .note("§2.2：未闭合块注释 → E-0004")
                .help("补全 `*/`");
            self.emit(h, d);
            end = self.text.len();
        }
        let text_from = body_start;
        let text_to = if closed {
            end.saturating_sub(2).max(text_from)
        } else {
            end
        };
        let text = self.slice(text_from, text_to);
        self.scan_invisibles_in(h, text_from, text_to);
        self.comments.push(Comment {
            kind: CommentKind::Block,
            span: Span::new(self.file, u32o(start), u32o(end)),
            text,
        });
    }

    fn maybe_build_constraint(&mut self, h: &mut Handler, start: usize, end: usize, text: &str) {
        let t = text.trim_start();
        if !(t == "infy:build" || t.starts_with("infy:build ") || t.starts_with("infy:build\t")) {
            return;
        }
        let raw = t["infy:build".len()..].trim().to_string();
        // §2.2: must be the file's first non-blank line, right after an optional shebang.
        let before = &self.text[self.region_start..start.min(self.text.len())];
        let first_line = before.chars().all(|c| c == ' ' || c == '\t' || c == '\n' || c == '\r');
        if self.build_constraint.is_some() {
            let d = self
                .mk("E-0013", start, end, "重复构建约束注释：同一文件出现两条 `//infy:build`")
                .note("§2.2：同一文件多条约束注释 → E-0013")
                .help("合并为一条布尔表达式");
            self.emit(h, d);
            return;
        }
        if !first_line {
            let d = self
                .mk("E-0011", start, end, "构建约束位置错误：`//infy:build` 不在首个非空行")
                .note("§2.2：`//infy:build <约束>` 必须是文件的第一个非空行，且紧跟 shebang（若有）")
                .help("上移到首行");
            self.emit(h, d);
        }
        let line = self.src.byte_line_col(u32o(start)).0;
        self.build_constraint = Some((raw.clone(), line, Span::new(self.file, u32o(start), u32o(end))));
        if self.check_e0012 {
            let unknown = check_constraint_names(&raw, self.features);
            if !unknown.is_empty() {
                let d = self
                    .mk("E-0012", start, end, &format!(
                        "未知构建约束名：{}",
                        unknown.iter().map(|s| format!("`{s}`")).collect::<Vec<_>>().join("、")
                    ))
                    .note("§2.2：约束名全集 = 11 个内置名 + module 清单 `features` 声明的自定义名")
                    .help("用 §2.2 封闭集或在 `[features]` 声明");
                self.emit(h, d);
            }
        }
    }

    // -- trivia --------------------------------------------------------------------------------

    fn skip_trivia(&mut self, h: &mut Handler) {
        loop {
            loop {
                let Some(c) = self.cur() else { break };
                if matches!(c, ' ' | '\t' | '\n' | '\r') {
                    self.bump_char();
                    continue;
                }
                if is_invisible(c) {
                    self.illegal_char(h, "不可见字符", "删除；确需则放字符串字面量");
                    continue;
                }
                if c.is_whitespace() || c.is_control() {
                    self.illegal_char(h, "非 ASCII 空白或控制字符", "改用 ASCII 空格/制表符（§2.11）");
                    continue;
                }
                break;
            }
            if self.starts("//") {
                self.lex_line_comment(h);
                continue;
            }
            if self.starts("/*") {
                self.lex_block_comment(h);
                continue;
            }
            break;
        }
    }

    // -- tokens --------------------------------------------------------------------------------

    fn push_tok(&mut self, kind: Kind, start: usize, end: usize) {
        let text = self.slice(start, end);
        self.toks.push(Tok {
            kind,
            span: Span::new(self.file, u32o(start), u32o(end)),
            text,
        });
    }

    /// §2.9 literal prefixes: `r"`, `r#"…`, `b"`, `c"` — and never when the letter merely continues
    /// an identifier (`red`, `byte`, `cord`).
    fn ident_prefix_is_literal(&self, c0: char) -> bool {
        let nxt = self.peek_at(self.pos + c0.len_utf8());
        match c0 {
            'r' => nxt == Some('#') || nxt == Some('"'),
            'b' | 'c' => nxt == Some('"'),
            _ => false,
        }
    }

    fn lex_ident_or_literal(&mut self, h: &mut Handler) {
        let start = self.pos;
        let c0 = match self.cur() {
            Some(c) => c,
            None => return,
        };
        // §2.9 prefixes: `r".."`, `r#".."#`, `b".."`, `c".."` — only when the letter is NOT part of
        // a longer identifier (`red"x"` stays `red` + a string token).
        if matches!(c0, 'r' | 'b' | 'c') && self.ident_prefix_is_literal(c0) {
            match c0 {
                'r' => {
                    let mut j = start + 1;
                    while self.peek_at(j) == Some('#') {
                        j += 1;
                    }
                    if self.peek_at(j) == Some('"') {
                        self.lex_raw_string(h);
                        return;
                    }
                    // `r#foo` is not a raw string: fall through, `r` is an ordinary identifier.
                }
                'b' => {
                    self.lex_byte_string(h);
                    return;
                }
                _ => {
                    self.lex_quoted(h, QuoteKind::Cord);
                    return;
                }
            }
        }
        // maximal XID run
        self.bump_char();
        while let Some(c) = self.cur() {
            if c == '_' || is_xid_continue(c) {
                self.bump_char();
            } else {
                break;
            }
        }
        let run = self.slice(start, self.pos);

        // §2.4 `macro_rules!` is ONE token (`!` belongs to the keyword).
        if run == "macro_rules" && self.cur() == Some('!') {
            self.bump_char();
            self.push_tok(Kind::Keyword(Kw::MacroRules), start, self.pos);
            return;
        }

        if run.starts_with("__") {
            let d = self
                .mk("E-0007", start, self.pos, &format!("保留前缀标识符 `{run}`：`__` 前缀保留给编译器与生成代码"))
                .note("§2.3：`__` 双下划线前缀保留给用户定义之外的用途")
                .help("删除前置 `__`");
            self.emit(h, d);
        }
        if matches!(run.as_str(), "use" | "mod" | "union" | "linkage") {
            let d = self
                .mk("E-0020", start, self.pos, &format!("保留字 `{run}` 不可作标识符"))
                .note("§2.4：`use`/`mod`/`union`/`linkage` 在本版文法中未被使用，但仍是保留字（占用命名空间）")
                .help("改名（§2.4 保留集）");
            self.emit(h, d);
        }
        self.check_script_mixing(h, &run, start, self.pos);

        let kind = match Kw::from_text(&run) {
            Some(k) => Kind::Keyword(k),
            None => Kind::Ident(run),
        };
        self.push_tok(kind, start, self.pos);
    }

    /// §2.10 mixed-script check inside ONE identifier (the pairwise confusable check is sema's).
    fn check_script_mixing(&mut self, h: &mut Handler, run: &str, start: usize, end: usize) {
        let mut seen: Vec<Script> = Vec::new();
        for c in run.chars() {
            let s = script_of(c);
            if s == Script::Neutral {
                continue;
            }
            if !seen.contains(&s) {
                seen.push(s);
            }
        }
        if seen.len() <= 1 || scripts_compatible(&seen) {
            return;
        }
        let names = seen
            .iter()
            .map(|s| format!("{s:?}"))
            .collect::<Vec<_>>()
            .join("+");
        let (code, title) = if scripts_are_confusable_family(&seen) {
            (
                "E-0050",
                format!("同一标识符 `{run}` 内混用脚本 {names}（混淆字符，§2.10）"),
            )
        } else {
            (
                "E-0051",
                format!("标识符 `{run}` 脚本混用非法：{names}（§2.10）"),
            )
        };
        let d = self
            .mk(code, start, end, &title)
            .note("§2.10：一个标识符内的字母必须同属一个脚本，或属允许的中性类（数字、`_`、`'`）；Han 与假名可混用，Han 与谚文不可")
            .help("拆分命名 / 更换字符");
        self.emit(h, d);
    }

    fn lex_quote(&mut self, h: &mut Handler) {
        let start = self.pos;
        // char literal vs lifetime (§2.5 / §2.9)
        match self.peek_at(start + 1) {
            None => {
                self.illegal_char(
                    h,
                    "孤立的 `'`",
                    "生命周期写作 `'a`/`'static`（§2.5）；字符写作 `'x'`（§2.9）",
                );
                return;
            }
            Some('\\') => {
                self.pos = start + 1;
                self.lex_char_lit_with_escape(h, start);
                return;
            }
            _ => {}
        }
        // base code point, then any combining marks / ZWJ, then the closing `'` → char_lit (§2.9)
        let mut j = start + 1;
        let base = match self.peek_at(j) {
            Some(c) if c != '\'' && c != '\n' => c,
            _ => {
                // `''` — empty char literal: not exactly one code point.
                self.pos = (j + 1).min(self.text.len());
                let d = self
                    .mk("E-0049", start, self.pos, "`char_lit` 必须恰好含一个 Unicode 代码点（此处为空）")
                    .note("§2.9：`char_lit` 必须是单个代码点")
                    .help("补一个字符，或改用 `str`");
                self.emit(h, d);
                return;
            }
        };
        j += base.len_utf8();
        let mut marks = 0usize;
        while let Some(c) = self.peek_at(j) {
            if !is_combining(c) && c != '\u{200D}' {
                break;
            }
            marks += 1;
            j += c.len_utf8();
        }
        if self.peek_at(j) == Some('\'') {
            let end = j + 1;
            if marks > 0 {
                let d = self
                    .mk("E-0049", start, end, &format!(
                        "`char_lit` 含图符簇（1 个基字符 + {marks} 个组合标记）"
                    ))
                    .note("§2.9：图符簇（如 `'e\\u{301}'`）→ E-0049，应写进 `str`")
                    .help("改用 `str` 字面量，或写成 `'\\u{...}'`");
                self.emit(h, d);
                self.pos = end;
                return;
            }
            self.pos = end;
            self.push_tok(Kind::Lit(Lit::Char(base)), start, end);
            return;
        }
        // not a char literal → lifetime (§2.5)
        let mut k = start + 1;
        let mut first = true;
        while let Some(c) = self.peek_at(k) {
            let ok = if first { c == '_' || is_xid_start(c) } else { c == '_' || is_xid_continue(c) };
            if !ok {
                break;
            }
            first = false;
            k += c.len_utf8();
        }
        if k > start + 1 {
            // `'ab'` — quoted but multi code point: that is a malformed char_lit, not a lifetime.
            if self.peek_at(k) == Some('\'') {
                let end = k + 1;
                let d = self
                    .mk("E-0049", start, end, "`char_lit` 含多个代码点")
                    .note("§2.9：`char_lit` 必须是单个 Unicode 代码点")
                    .help("改用 `str` 字面量，或写成 `'\\u{...}'`");
                self.emit(h, d);
                self.pos = end;
                return;
            }
            let name = self.slice(start + 1, k);
            self.check_script_mixing(h, &name, start, k);
            self.pos = k;
            self.push_tok(Kind::Lifetime(name), start, k);
            return;
        }
        self.pos = start;
        self.illegal_char(h, "既不是生命周期也不是字符字面量的 `'`", "生命周期写作 `'a`/`'static`（§2.5）；字符写作 `'x'`（§2.9）");
    }

    fn lex_char_lit_with_escape(&mut self, h: &mut Handler, start: usize) {
        // `'\\n'`, `'\\u{..'}`, `'\\x41'` (§2.9 allows \\x in char_lit)
        let esc_start = self.pos; // at `\`
        match self.lex_escape(h, esc_start, /*is_byte=*/ false, /*in_str_lit=*/ false) {
            Some(Esc::Ch(c)) => {
                if self.cur() == Some('\'') {
                    let end = self.pos + 1;
                    self.pos = end;
                    self.push_tok(Kind::Lit(Lit::Char(c)), start, end);
                    return;
                }
                let d = self
                    .mk("E-0049", start, self.pos + 1, "`char_lit` 转义后缺少闭合 `'`")
                    .note("§2.9：`char_lit` 必须是单个代码点加一对 `'`")
                    .help("补全闭合 `'`");
                self.emit(h, d);
                let end = (self.pos + 1).min(self.text.len());
                self.push_tok(Kind::Lit(Lit::Char(c)), start, end);
            }
            Some(Esc::Byte(b)) => {
                if self.cur() == Some('\'') {
                    let end = self.pos + 1;
                    self.pos = end;
                    self.push_tok(Kind::Lit(Lit::Char(b as char)), start, end);
                    return;
                }
                let d = self
                    .mk("E-0049", start, self.pos + 1, "`char_lit` 转义后缺少闭合 `'`")
                    .help("补全闭合 `'`");
                self.emit(h, d);
            }
            None => {
                // lex_escape already diagnosed; resync past the escape attempt
                if self.pos == esc_start {
                    self.pos = (esc_start + 2).min(self.text.len());
                }
                if self.cur() == Some('\'') {
                    self.bump_char();
                }
            }
        }
    }

    fn lex_number(&mut self, h: &mut Handler) {
        let start = self.pos;
        let (radix, digits_start) = if self.starts("0x") {
            (Radix::Hex, start + 2)
        } else if self.starts("0o") {
            (Radix::Oct, start + 2)
        } else if self.starts("0b") {
            (Radix::Bin, start + 2)
        } else {
            (Radix::Dec, start)
        };
        if radix != Radix::Dec {
            self.pos = digits_start;
        }
        let digits = self.scan_digit_run(h, radix);
        if digits.is_empty() {
            // `"0x"` with no hex digit is not a `hex_lit` at all (§2.8 grammar); the conforming
            // longest match is the decimal `0`, then an identifier.
            self.pos = start + 1;
            let end = self.pos;
            self.push_tok(
                Kind::Lit(Lit::Int(IntLit {
                    digits: "0".to_string(),
                    radix: Radix::Dec,
                    suffix: None,
                    value: Some(0),
                })),
                start,
                end,
            );
            return;
        }
        if radix == Radix::Dec {
            // fractional part
            let mut is_float = false;
            let mut mantissa = digits.clone();
            if self.cur() == Some('.') {
                let dot = self.pos;
                let after = self.peek_at(dot + 1);
                if after == Some('.') {
                    // `1..` / `1..=` is a range (§2.7 priority 3), never a fraction.
                } else if after == Some('_') {
                    // `1._5`: E-0041, then carry on scanning the fraction.
                    self.underscore_adjacent(h, dot + 1, '.');
                    is_float = true;
                    self.pos = dot + 2;
                    let frac = self.scan_digit_run(h, Radix::Dec);
                    mantissa.push('.');
                    mantissa.push_str(&frac);
                } else if after.map(is_ascii_digit).unwrap_or(false) {
                    is_float = true;
                    self.pos = dot + 1;
                    let frac = self.scan_digit_run(h, Radix::Dec);
                    mantissa.push('.');
                    mantissa.push_str(&frac);
                } else if after.map(is_xid_start).unwrap_or(false) {
                    // `1.min(2)` / `x.0` (§3.11): integer, then `.` then an identifier. The dot
                    // belongs to the punctuator, so the literal stops here and no diagnostic is due.
                } else {
                    // `1.` at EOF / before whitespace / before punctuation → E-0044.
                    self.float_trailing_dot(h, start, dot);
                    is_float = true;
                    self.pos = dot + 1;
                }
            }
            // exponent
            let mut exp: Option<String> = None;
            if matches!(self.cur(), Some('e') | Some('E')) {
                let epos = self.pos;
                let mut j = epos + 1;
                let sign = if matches!(self.peek_at(j), Some('+') | Some('-')) {
                    let s = self.slice(j, j + 1);
                    j += 1;
                    s
                } else {
                    String::new()
                };
                if self.peek_at(j) == Some('_') {
                    self.underscore_adjacent(h, j, 'e');
                    j += 1;
                }
                if self.peek_at(j).map(is_ascii_digit).unwrap_or(false) {
                    self.pos = j;
                    let d = self.scan_digit_run(h, Radix::Dec);
                    is_float = true;
                    exp = Some(sign + &d);
                }
            }
            let suffix = self.take_suffix();
            if is_float {
                self.check_float_overflow(h, start, self.pos, &mantissa, exp.as_deref(), suffix.as_deref());
                self.push_tok(
                    Kind::Lit(Lit::Float(FloatLit {
                        mantissa,
                        exp,
                        suffix,
                    })),
                    start,
                    self.pos,
                );
                return;
            }
            self.push_int(h, start, self.pos, Radix::Dec, &digits, suffix);
            return;
        }

        // non-decimal radix: no frac/exp possible (§2.8 `float_lit = dec_lit (frac|exp)`)
        let suffix = self.take_suffix();
        self.push_int(h, start, self.pos, radix, &digits, suffix);
    }

    fn float_trailing_dot(&mut self, h: &mut Handler, start: usize, dot: usize) {
        let d = self
            .mk("E-0044", start, dot + 1, &format!(
                "浮点字面量写法非法：`{}` 必须写成 `{}`",
                self.slice(start, dot + 1),
                self.slice(start, dot + 1) + "0"
            ))
            .note("§2.8：仅 `frac` 或仅 `exp` 均合法，但 `1.` 非法（必须写 `1.0`）")
            .help("补上小数位");
        let (p, _) = self.sources.line_col(Span::new(self.file, u32o(dot), u32o(dot + 1)));
        let d = d.insert(self.file, p.line, p.column + 1, "0");
        self.emit(h, d);
    }

    /// `1._5`, `1_e5`, `1.5e_3`: `_` on either side of `.` or `e` → E-0041.
    fn underscore_adjacent(&mut self, h: &mut Handler, at: usize, neighbour: char) {
        let d = self
            .mk("E-0041", at, at + 1, &format!("分隔符位置非法：`_` 贴近 `{neighbour}`"))
            .note("§2.8：`_` 出现在 `.` 或 `e` 两侧 → E-0041")
            .help("移到数字之间");
        self.emit(h, d);
    }

    /// Scan a digit run for `radix`, enforcing §2.8's separator rules.
    /// Returns the digits with `_` stripped.
    fn scan_digit_run(&mut self, h: &mut Handler, radix: Radix) -> String {
        let mut out = String::new();
        let mut last_us = false;
        // Position of a `_` whose adjacency to `.`/`e` was already reported, so `1_e5` yields one
        // E-0041 rather than two (once from the run, once from the trailing check).
        let mut reported_at: Option<usize> = None;
        while let Some(c) = self.cur() {
            let is_d = match radix {
                Radix::Hex => c.is_ascii_hexdigit(),
                Radix::Oct => matches!(c, '0'..='7'),
                Radix::Bin => matches!(c, '0'..='1'),
                Radix::Dec => is_ascii_digit(c),
            };
            if is_d {
                out.push(c);
                last_us = false;
                self.bump_char();
                continue;
            }
            if c == '_' {
                let at = self.pos;
                if last_us {
                    let d = self
                        .mk("E-0040", at, at + 1, "非法数字分隔符：连续的 `_`")
                        .note("§2.8：连续两个 `_` / 尾 `_` → E-0040。注意 §2.6/§2.8 又明文规定 `0x_FFu8` 与 \
                              `0xFFu8` 等价，故紧跟数制前缀的首个 `_` 被放行（两处规范的矛盾点，以前缀等价条款为准）；\
                              十进制字面量不可能以 `_` 开头，因为 `_123` 本身就是标识符")
                        .help("移除多余分隔符");
                    self.emit(h, d);
                }
                let nxt = self.peek_at(at + 1);
                if radix == Radix::Dec && matches!(nxt, Some('.') | Some('e') | Some('E')) {
                    self.underscore_adjacent(h, at, nxt.unwrap_or('.'));
                    reported_at = Some(at);
                }
                last_us = true;
                self.bump_char();
                continue;
            }
            break;
        }
        if last_us {
            let at = self.pos.saturating_sub(1);
            let nxt = self.cur();
            if reported_at == Some(at) {
                // already diagnosed as "`_` next to `.`/`e`" while scanning
            } else if radix == Radix::Dec && matches!(nxt, Some('.') | Some('e') | Some('E')) {
                self.underscore_adjacent(h, at, nxt.unwrap_or('.'));
            } else {
                let d = self
                    .mk("E-0040", at, at + 1, "非法数字分隔符：尾 `_`")
                    .note("§2.8：尾 `_` → E-0040")
                    .help("移除多余分隔符");
                self.emit(h, d);
            }
        }
        out
    }

    /// A literal suffix from §2.8's closed set, taken only when the whole following XID run is a
    /// legal suffix (`123abc` therefore lexes as `123` + `abc`, not as a mangled literal).
    fn take_suffix(&mut self) -> Option<String> {
        let start = self.pos;
        let mut end = start;
        while let Some(c) = self.peek_at(end) {
            if c == '_' || is_xid_continue(c) {
                end += c.len_utf8();
            } else {
                break;
            }
        }
        if end == start {
            return None;
        }
        let cand = self.slice(start, end);
        if is_valid_suffix(&cand) {
            self.pos = end;
            Some(cand)
        } else {
            None
        }
    }

    fn push_int(
        &mut self,
        _h: &mut Handler,
        start: usize,
        end: usize,
        radix: Radix,
        digits: &str,
        suffix: Option<String>,
    ) {
        // E-0042/E-0043 are RANGE checks that depend on context (§2.8's `-128i8` exception), so
        // the lexer only records whether the value fits a u128.
        let r = match radix {
            Radix::Dec => 10,
            Radix::Hex => 16,
            Radix::Oct => 8,
            Radix::Bin => 2,
        };
        let value = u128::from_str_radix(digits, r).ok();
        self.push_tok(
            Kind::Lit(Lit::Int(IntLit {
                digits: digits.to_string(),
                radix,
                suffix,
                value,
            })),
            start,
            end,
        );
    }

    /// §2.8 `1.0e999f32` → E-0045 (a literal that overflows to `inf` is forbidden).
    fn check_float_overflow(
        &mut self,
        h: &mut Handler,
        start: usize,
        end: usize,
        mantissa: &str,
        exp: Option<&str>,
        suffix: Option<&str>,
    ) {
        let mut norm = mantissa.trim_end_matches('.').to_string();
        if norm.is_empty() {
            norm = "0".to_string();
        }
        match exp {
            Some(e) => {
                norm.push('e');
                norm.push_str(e);
            }
            None => {}
        }
        let as_f64 = norm.parse::<f64>();
        let inf64 = as_f64.map(|v| !v.is_finite()).unwrap_or(false);
        let inf32 = if suffix.map(|s| s == "f32").unwrap_or(false) {
            norm.parse::<f32>().map(|v| !v.is_finite()).unwrap_or(false)
        } else {
            false
        };
        if inf64 || inf32 {
            let d = self
                .mk("E-0045", start, end, &format!(
                    "浮点字面量 `{}` 溢出为 inf（{}）",
                    self.slice(start, end),
                    suffix.unwrap_or("默认 f64")
                ))
                .note("§2.8：`1.0e999f32` → E-0045，字面量溢出为 `inf` 被禁止")
                .help("用 `std.f32.INF` / `std.f64.INF` 显式表达");
            self.emit(h, d);
        }
    }

    // -- strings -------------------------------------------------------------------------------

    fn lex_quoted(&mut self, h: &mut Handler, kind: QuoteKind) {
        let start = match kind {
            QuoteKind::Str => self.pos,
            QuoteKind::Cord => self.pos,
        };
        let prefix_len = match kind {
            QuoteKind::Cord => 1,
            _ => 0,
        };
        let open = start + prefix_len;
        if self.peek_at(open) != Some('"') {
            self.pos = open;
            return;
        }
        self.pos = open + 1;
        let mut val = String::new();
        let mut terminated = false;
        loop {
            let Some(c) = self.cur() else { break };
            if c == '"' {
                self.bump_char();
                terminated = true;
                break;
            }
            if c == '\n' {
                let d = self
                    .mk("E-0009", start, self.pos, "未闭合字符串：`str_lit` 内出现裸换行")
                    .note("§2.9：`str_lit` 内不得出现裸换行 → E-0009；跨行文本用 `raw_str_lit` 或多段拼接")
                    .help("补全闭合 `\"`，或改用 `r\"...\"`");
                self.emit(h, d);
                break;
            }
            if c == '\\' {
                let esc_start = self.pos;
                match self.lex_escape(h, esc_start, false, true) {
                    Some(Esc::Ch(ch)) => val.push(ch),
                    Some(Esc::Byte(b)) => {
                        if let Some(ch) = char::from_u32(b as u32) {
                            val.push(ch);
                        }
                    }
                    None => {
                        if self.pos == esc_start {
                            self.pos = (esc_start + 1).min(self.text.len());
                        }
                    }
                }
                continue;
            }
            val.push(c);
            self.bump_char();
        }
        if !terminated {
            let d = self
                .mk("E-0009", start, self.pos, "未闭合字符串：缺少 `\"`")
                .note("§2.6：未闭合 → E-0009")
                .help("补全 `\"`");
            self.emit(h, d);
        }
        let end = self.pos;
        match kind {
            QuoteKind::Str => self.push_tok(Kind::Lit(Lit::Str(val)), start, end),
            QuoteKind::Cord => {
                let norm = normalize_cord(&val);
                self.push_tok(Kind::Lit(Lit::Cord(norm)), start, end)
            }
        }
    }

    fn lex_byte_string(&mut self, h: &mut Handler) {
        let start = self.pos;
        if self.cur() == Some('b') {
            self.bump_char();
        }
        if self.cur() != Some('"') {
            return;
        }
        self.bump_char();
        let mut bytes: Vec<u8> = Vec::new();
        let mut terminated = false;
        loop {
            let Some(c) = self.cur() else { break };
            if c == '"' {
                self.bump_char();
                terminated = true;
                break;
            }
            if c == '\n' {
                let d = self
                    .mk("E-0009", start, self.pos, "未闭合字节串：`b\"..\"` 内出现裸换行")
                    .note("§2.9：`byte_str_lit` 为单行字面量")
                    .help("补全 `\"`");
                self.emit(h, d);
                break;
            }
            if c == '\\' {
                let esc_start = self.pos;
                match self.lex_escape(h, esc_start, true, false) {
                    Some(Esc::Byte(b)) => bytes.push(b),
                    Some(Esc::Ch(ch)) => {
                        if ch.is_ascii() {
                            bytes.push(ch as u8);
                        } else {
                            let d = self
                                .mk("E-0047", esc_start, self.pos, &format!(
                                    "byte_str_lit 含非 ASCII 字节：`\\u{{{:X}`}}` = U+{:04X}",
                                    ch as u32, ch as u32
                                ))
                                .note("§2.9：`byte_str_lit` 仅允许 ASCII 字节（0..=0x7F）")
                                .help("改用 `str_lit`，或换用 ASCII 转义");
                            self.emit(h, d);
                        }
                    }
                    None => {
                        if self.pos == esc_start {
                            self.pos = (esc_start + 1).min(self.text.len());
                        }
                    }
                }
                continue;
            }
            if !c.is_ascii() {
                let at = self.pos;
                self.bump_char();
                let d = self
                    .mk("E-0047", at, self.pos, &format!(
                        "byte_str_lit 含非 ASCII 字符 `{c}`（仅 ASCII 字节）"))
                    .note("§2.9：`byte_str_lit = \"b\" '\"' (仅 ASCII 字节) '\"'`")
                    .help("改用 `str_lit`，或用 `\\xNN` 写出该字节");
                self.emit(h, d);
                continue;
            }
            bytes.push(c as u8);
            self.bump_char();
        }
        if !terminated {
            let d = self
                .mk("E-0009", start, self.pos, "未闭合字节串：缺少 `\"`")
                .note("§2.6：未闭合 → E-0009")
                .help("补全 `\"`");
            self.emit(h, d);
        }
        let end = self.pos;
        self.push_tok(Kind::Lit(Lit::ByteStr(bytes)), start, end);
    }

    fn lex_raw_string(&mut self, h: &mut Handler) {
        let start = self.pos;
        if self.cur() == Some('r') {
            self.bump_char();
        }
        let mut hashes = 0usize;
        while self.cur() == Some('#') {
            hashes += 1;
            self.bump_char();
        }
        if self.cur() != Some('"') {
            // Not a raw string (`r#foo`): `r` is an ordinary identifier and the `#`s follow as
            // punctuation. No recursion — this is the recovery path, it must terminate.
            self.pos = (start + 1).min(self.text.len());
            let end = self.pos;
            self.push_tok(Kind::Ident("r".to_string()), start, end);
            return;
        }
        self.bump_char();
        let body_start = self.pos;
        let mut terminated = false;
        let mut body_end = self.text.len();
        loop {
            let Some(c) = self.cur() else { break };
            if c == '"' {
                let mut j = self.pos + 1;
                let mut k = 0usize;
                while k < hashes && self.peek_at(j) == Some('#') {
                    j += 1;
                    k += 1;
                }
                if k == hashes {
                    body_end = self.pos;
                    self.pos = j;
                    terminated = true;
                    break;
                }
                self.bump_char();
                continue;
            }
            self.bump_char();
        }
        if !terminated {
            let d = self
                .mk("E-0010", start, self.text.len(), &format!(
                    "未闭合原字符串：`r{}\"` 需要成对的 `\"{}``",
                    "#".repeat(hashes),
                    "#".repeat(hashes)
                ))
                .note("§2.6：`r#\" .. \"#` 支持定界符数量递增，不可闭合 → E-0010")
                .help("增加/补全 `#` 与闭合引号");
            self.emit(h, d);
            body_end = self.text.len();
        }
        let end = self.pos;
        let raw = self.slice(body_start, body_end);
        let value = self.dedent_raw(body_start, &raw, h);
        self.push_tok(Kind::Lit(Lit::Str(value)), start, end);
    }

    /// §2.9: a multi-line raw string whose first character is a newline has its **maximum common
    /// leading whitespace** stripped at lex time; tabs count as one unit and must not mix with
    /// spaces inside the common prefix → E-0048.
    fn dedent_raw(&mut self, body_start: usize, raw: &str, h: &mut Handler) -> String {
        if !raw.starts_with('\n') {
            return raw.to_string();
        }
        let after_first = &raw[1..];
        let lines: Vec<&str> = after_first.split('\n').collect();
        let mut min: Option<usize> = None;
        let mut kinds: Vec<(usize, bool, bool)> = Vec::new(); // (prefix_len, has_tab, has_space)
        for line in &lines {
            let mut n = 0usize;
            let mut tab = false;
            let mut sp = false;
            for c in line.chars() {
                match c {
                    '\t' => {
                        tab = true;
                        n += 1;
                    }
                    ' ' => {
                        sp = true;
                        n += 1;
                    }
                    _ => break,
                }
            }
            let blank = line.chars().all(|c| c == '\t' || c == ' ');
            kinds.push((n, tab, sp));
            if blank {
                continue;
            }
            min = Some(match min {
                Some(m) => m.min(n),
                None => n,
            });
        }
        let min = match min {
            Some(m) => m,
            None => return raw[1..].to_string(),
        };
        if min == 0 {
            return raw[1..].to_string();
        }
        // E-0048: tabs and spaces mixed inside the common prefix.
        for (idx, (n, tab, sp)) in kinds.iter().enumerate() {
            let (tab_in_prefix, sp_in_prefix) = {
                let line = lines[idx];
                let pre: String = line.chars().take(min).collect();
                (pre.contains('\t'), pre.contains(' '))
            };
            let _ = (n, tab, sp);
            if tab_in_prefix && sp_in_prefix {
                let line_no = self.src.byte_line_col(u32o(body_start)).0 + idx as u32 + 1;
                let d = self
                    .mk("E-0048", body_start, body_start, &format!(
                        "多行 raw 缩进混用制表符与空格（第 {line_no} 行的公共前导空白）"))
                    .note("§2.9：缩进只算空格与制表符，制表符按 1 个空白单元处理且不得与空格混用于同一公共前缀 → E-0048")
                    .help("统一缩进字符（`infy fmt` 使用 4 空格）");
                self.emit(h, d);
                break;
            }
        }
        let mut out = String::new();
        for (idx, line) in lines.iter().enumerate() {
            let trimmed: String = line.chars().skip(min).collect();
            if idx > 0 {
                out.push('\n');
            }
            out.push_str(&trimmed);
        }
        out
    }

    // -- escapes -------------------------------------------------------------------------------

    /// Parse the escape starting at `esc_start` (which must be a `\`).
    /// `is_byte` = inside `b".."`; `in_str_lit` = inside a `str_lit`/`char_lit` text context.
    fn lex_escape(&mut self, h: &mut Handler, esc_start: usize, is_byte: bool, in_str_lit: bool) -> Option<Esc> {
        let at = esc_start + 1;
        let Some(c) = self.peek_at(at) else {
            return None;
        };
        let simple = |ch: char| -> Option<char> {
            Some(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '0' => '\0',
                '\\' => '\\',
                '"' => '"',
                '\'' => '\'',
                'e' => '\u{1B}',
                'a' => '\u{07}',
                'b' => '\u{08}',
                'f' => '\u{0C}',
                'v' => '\u{0B}',
                _ => return None,
            })
        };
        if let Some(ch) = simple(c) {
            self.pos = at + c.len_utf8();
            return Some(if is_byte {
                Esc::Byte(ch as u8)
            } else {
                Esc::Ch(ch)
            });
        }
        if c == '\n' {
            // Not in §2.9's closed table. Reported as E-0046 and consumed as a line continuation
            // so the second diagnostic (E-0009) is not stacked on the same byte.
            let d = self
                .mk("E-0046", esc_start, at + 1, "未知转义序列 `\\` + 换行：§2.9 转义表封闭")
                .note("§2.9：未在该表中的 `\\X` → E-0046；`str_lit` 内也不得出现裸换行")
                .help("删除反斜杠，或用 `r\"...\"` 原字符串");
            self.emit(h, d);
            self.pos = at + 1;
            return None;
        }
        if c == 'x' {
            let end_at = at + 1;
            let val = if self.peek_at(end_at) == Some('{') {
                self.scan_braced_hex(end_at + 1).map(|(v, e)| (v, e))
            } else {
                let mut v: u32 = 0;
                let mut j = end_at;
                let mut n = 0;
                while n < 2 {
                    match self.peek_at(j).and_then(|ch| ch.to_digit(16)) {
                        Some(dg) => {
                            v = v * 16 + dg;
                            j += 1;
                            n += 1;
                        }
                        None => break,
                    }
                }
                if n == 2 {
                    Some((v, j))
                } else {
                    None
                }
            };
            match val {
                Some((v, e)) => {
                    self.pos = e;
                    if !is_byte && in_str_lit {
                        let d = self
                            .mk("E-0047", esc_start, e, &format!(
                                "`str_lit` 内使用字节转义 `\\x{}`",
                                self.slice(at + 1, e)
                            ))
                            .note("§2.9：`\\x41` / `\\x{..}` 仅在 `b\"..\"` 与 `char_lit` 允许；在 `str_lit` 中 → E-0047（防止写出非法 UTF-8）")
                            .help("改 `b\"\\x41\"` 或 `\\u{41}`");
                        self.emit(h, d);
                    }
                    if v > 0xFF {
                        let d = self
                            .mk("E-0046", esc_start, e, &format!("字节转义超界：`\\x{}` > 0xFF", self.slice(at + 1, e)))
                            .note("§2.9：`\\x` 转义只编码一个字节")
                            .help("改用 `\\u{...}`");
                        self.emit(h, d);
                        return Some(Esc::Ch('\u{0}'));
                    }
                    let b = v as u8;
                    return Some(if is_byte { Esc::Byte(b) } else { Esc::Ch(b as char) });
                }
                None => {
                    let d = self
                        .mk("E-0046", esc_start, (at + 3).min(self.text.len()), "未知转义序列：`\\x` 需要两位十六进制数字或 `\\x{..}`")
                        .note("§2.9 转义表封闭")
                        .help("写成 `\\xNN` 或 `\\x{...}`");
                    self.emit(h, d);
                    self.pos = (at + 1).min(self.text.len());
                    return None;
                }
            }
        }
        if c == 'u' {
            let braced = at + 1;
            if self.peek_at(braced) != Some('{') {
                let d = self
                    .mk("E-0046", esc_start, (braced + 1).min(self.text.len()), "未知转义序列：`\\u` 必须写作 `\\u{...}`")
                    .note("§2.9：`\\u{1F600}` Unicode 代码点，1..6 位十六进制")
                    .help("补上花括号");
                self.emit(h, d);
                self.pos = (braced).min(self.text.len());
                return None;
            }
            match self.scan_braced_hex(braced + 1) {
                Some((v, e)) => {
                    self.pos = e;
                    if v > 0x10FFFF || (0xD800..0xE000).contains(&v) {
                        let d = self
                            .mk("E-0046", esc_start, e, &format!("`\\u{{{:X}}}` 不是合法的 Unicode 代码点", v))
                            .note("§2.9：代码点必须 ≤ 0x10FFFF 且不得落在代理区 D800..DFFF")
                            .help("改用一个合法代码点");
                        self.emit(h, d);
                        return Some(Esc::Ch('\u{0}'));
                    }
                    if is_byte && v > 0xFF {
                        let d = self
                            .mk("E-0047", esc_start, e, &format!("byte_str_lit 内 `\\u{{{:X}}}` 超出单字节范围", v))
                            .note("§2.9：`b\"..\"` 只编码 ASCII/单字节")
                            .help("改用 `\\xNN` 或 `str_lit`");
                        self.emit(h, d);
                        return Some(Esc::Byte(0));
                    }
                    let ch = char::from_u32(v).unwrap_or('\u{0}');
                    return Some(if is_byte { Esc::Byte(ch as u8) } else { Esc::Ch(ch) });
                }
                None => {
                    let d = self
                        .mk("E-0046", esc_start, (braced + 2).min(self.text.len()), "未知转义序列：`\\u{..}` 需要 1..6 位十六进制数字")
                        .note("§2.9：`\\u{XXXX}` Unicode 转义，1 至 6 位十六进制")
                        .help("检查十六进制位数");
                    self.emit(h, d);
                    self.pos = (braced + 1).min(self.text.len());
                    return None;
                }
            }
        }
        // unknown escape
        let d = self
            .mk("E-0046", esc_start, (at + c.len_utf8()).min(self.text.len()), &format!("未知转义序列 `\\{c}`"))
            .note("§2.9：转义全集是封闭表；未在该表中的 `\\X` → E-0046")
            .help("查 §2.9 转义表");
        self.emit(h, d);
        self.pos = (at + c.len_utf8()).min(self.text.len());
        Some(if is_byte {
            Esc::Byte(if c.is_ascii() { c as u8 } else { 0 })
        } else {
            Esc::Ch(c)
        })
    }

    /// `scan_braced_hex`: at the first hex digit of `{...}`; returns (value, index after `}`).
    fn scan_braced_hex(&self, from: usize) -> Option<(u32, usize)> {
        let mut v: u32 = 0;
        let mut j = from;
        let mut n = 0usize;
        while let Some(c) = self.peek_at(j) {
            if c == '}' {
                if n == 0 || n > 6 {
                    return None;
                }
                return Some((v, j + 1));
            }
            match c.to_digit(16) {
                Some(dg) => {
                    v = v * 16 + dg;
                    j += 1;
                    n += 1;
                }
                None => return None,
            }
        }
        None
    }

    // -- @embed (BOTH forms) -------------------------------------------------------------------

    fn lex_embed(&mut self, h: &mut Handler, lang: String, start: usize, end: usize) {
        self.push_tok(Kind::EmbedLang(lang.clone()), start, end);
        self.skip_trivia(h);
        if self.cur() == Some('(') {
            let pstart = self.pos;
            match self.scan_balanced_parens(h) {
                Some((inner, pend)) => {
                    self.push_tok(Kind::EmbedArgs(inner), pstart, pend);
                }
                None => return,
            }
            self.skip_trivia(h);
        }
        if self.cur() != Some('{') {
            // No body at all: the parser reports that shape (§3.6, its E-0053 "嵌入块缺少主体"), so
            // the lexer stays quiet here and only owns the case the parser cannot see — a `{` that
            // was opened but never closes (E-0010, see `scan_embed_body`).
            return;
        }
        let bstart = self.pos;
        match self.scan_embed_body(h, &lang, bstart) {
            Some((body, bend)) => {
                self.push_tok(Kind::EmbedBody(body), bstart, bend);
            }
            None => {}
        }
    }

    fn scan_balanced_parens(&mut self, h: &mut Handler) -> Option<(String, usize)> {
        let start = self.pos;
        let mut depth = 0usize;
        loop {
            let Some(c) = self.cur() else { break };
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        let inner_end = self.pos;
                        let end = self.pos + 1;
                        self.pos = end;
                        let inner = self.slice(start + 1, inner_end);
                        return Some((inner, end));
                    }
                }
                '"' => {
                    // Args are INFY expressions: `"` starts a string whose parens must not count.
                    self.bump_char();
                    while let Some(ch) = self.cur() {
                        if ch == '\\' {
                            self.pos = (self.pos + 2).min(self.text.len());
                            continue;
                        }
                        if ch == '"' {
                            self.bump_char();
                            break;
                        }
                        if ch == '\n' {
                            break;
                        }
                        self.pos += ch.len_utf8();
                    }
                    continue;
                }
                '\n' => {}
                _ => {}
            }
            self.bump_char();
        }
        let d = self
            .mk("E-0010", start, self.text.len(), "嵌入块参数列表 `(` 未闭合")
            .note("§3.6 `embed_attrs = \"(\" embed_arg ... \")\"`；未闭合的定界复用 E-0010")
            .help("补全 `)`");
        self.emit(h, d);
        None
    }

    /// §2.11: the embed body is a lexically OPAQUE region — only `{`/`}` depth counting, no
    /// escape/comment interpretation.
    fn scan_embed_body(&mut self, h: &mut Handler, lang: &str, start: usize) -> Option<(String, usize)> {
        let mut depth = 0usize;
        self.bump_char(); // opening `{`
        let body_start = self.pos;
        loop {
            let Some(c) = self.cur() else { break };
            if c == '{' {
                depth += 1;
                self.bump_char();
                continue;
            }
            if c == '}' {
                if depth == 0 {
                    let body_end = self.pos;
                    let end = self.pos + 1;
                    self.pos = end;
                    return Some((self.slice(body_start, body_end), end));
                }
                depth -= 1;
                self.bump_char();
                continue;
            }
            self.bump_char();
        }
        let d = self
            .mk("E-0010", start, self.text.len(), &format!(
                "`@{lang}` 嵌入块的 `{{` 未闭合（词法不透明区只按 `{{`/`}}` 计数，§2.11）"
            ))
            .note("§9.3/§9.7 未给未闭合嵌入块分配编号，且禁止新增 E- 编号；这里选 E-0010 \
                   （未闭合原字符串）而非 E-0700（嵌入块位置非法）：两者都是\"不透明定界区未闭合\"，\
                   与位置无关；§2.11 保证嵌入内容本身永不触发 INFY 语法错误，故此诊断只针对定界符")
            .help("补全闭合 `}`");
        self.emit(h, d);
        None
    }

    // -- dispatcher ----------------------------------------------------------------------------

    fn lex_at_token(&mut self, h: &mut Handler) {
        let start = self.pos;
        let c = match self.cur() {
            Some(c) => c,
            None => return,
        };
        match c {
            '0'..='9' => self.lex_number(h),
            '"' => self.lex_quoted(h, QuoteKind::Str),
            '\'' => self.lex_quote(h),
            'r' | 'b' | 'c' | '_' => self.lex_ident_or_literal(h),
            '@' => self.lex_at_prefix(h, start),
            '#' => self.lex_hash(h),
            '/' => {
                if self.starts("/=") {
                    self.advance(2);
                    self.push_tok(Kind::Punct(Punct::SlashEq), start, self.pos);
                } else {
                    self.advance(1);
                    self.push_tok(Kind::Punct(Punct::Slash), start, self.pos);
                }
            }
            '.' => self.lex_punct_chain(start, &["..=", "..", "."], h),
            ':' => self.lex_punct_chain(start, &["::", ":"], h),
            '=' => self.lex_punct_chain(start, &["==", "=>", "="], h),
            '!' => self.lex_punct_chain(start, &["!=", "!"], h),
            '<' => self.lex_punct_chain(start, &["<<=", "<<", "<=", "<"], h),
            '>' => self.lex_punct_chain(start, &[">>=", ">>", ">=", ">"], h),
            '+' => self.lex_punct_chain(start, &["+=", "+"], h),
            '-' => self.lex_punct_chain(start, &["->", "-=", "-"], h),
            '*' => self.lex_punct_chain(start, &["*=", "*"], h),
            '%' => self.lex_punct_chain(start, &["%=", "%"], h),
            '&' => self.lex_punct_chain(start, &["&&", "&=", "&"], h),
            '|' => self.lex_punct_chain(start, &["||", "|=", "|"], h),
            '^' => self.lex_punct_chain(start, &["^=", "^"], h),
            '?' => {
                self.advance(1);
                self.push_tok(Kind::Punct(Punct::Question), start, self.pos);
            }
            '~' => {
                self.advance(1);
                self.push_tok(Kind::Punct(Punct::Tilde), start, self.pos);
            }
            ',' => {
                self.advance(1);
                self.push_tok(Kind::Punct(Punct::Comma), start, self.pos);
            }
            ';' => {
                self.advance(1);
                self.push_tok(Kind::Punct(Punct::Semi), start, self.pos);
            }
            '(' => {
                self.advance(1);
                self.push_tok(Kind::Punct(Punct::LParen), start, self.pos);
            }
            ')' => {
                self.advance(1);
                self.push_tok(Kind::Punct(Punct::RParen), start, self.pos);
            }
            '[' => {
                self.advance(1);
                self.push_tok(Kind::Punct(Punct::LBracket), start, self.pos);
            }
            ']' => {
                self.advance(1);
                self.push_tok(Kind::Punct(Punct::RBracket), start, self.pos);
            }
            '{' => {
                self.advance(1);
                self.push_tok(Kind::Punct(Punct::LBrace), start, self.pos);
            }
            '}' => {
                self.advance(1);
                self.push_tok(Kind::Punct(Punct::RBrace), start, self.pos);
            }
            '$' => {
                self.advance(1);
                self.push_tok(Kind::Punct(Punct::Dollar), start, self.pos);
            }
            _ => {
                if is_xid_start(c) {
                    self.lex_ident_or_literal(h);
                    return;
                }
                // `illegal_char` consumes the character itself.
                self.illegal_char(h, "该字符不属于 INFY 词法字符集", "删除它，或放进字符串字面量（§2.10）");
            }
        }
    }

    /// Longest-first punctuator match; the candidate list MUST be ordered longest-first (§2.7).
    fn lex_punct_chain(&mut self, start: usize, cands: &[&str], _h: &mut Handler) {
        for s in cands {
            if self.starts(s) {
                self.pos = (start + s.len()).min(self.text.len());
                let p = punct_of(s);
                self.push_tok(Kind::Punct(p), start, self.pos);
                return;
            }
        }
        // unreachable: every chain ends with a 1-char candidate
        self.bump_char();
    }

    fn lex_hash(&mut self, h: &mut Handler) {
        let start = self.pos;
        // §2.11/§3.1: `#[` and `#![` are ONE token each (AttrOpen), the `#` and `[` never split.
        if self.starts("#![") {
            self.advance(3);
            self.push_tok(Kind::AttrOpen { inner: true }, start, self.pos);
            return;
        }
        if self.starts("#[") {
            self.advance(2);
            self.push_tok(Kind::AttrOpen { inner: false }, start, self.pos);
            return;
        }
        self.advance(1);
        self.push_tok(Kind::Punct(Punct::Hash), start, self.pos);
        let _ = h;
    }

    fn lex_at_prefix(&mut self, h: &mut Handler, start: usize) {
        // `@py` … must be adjacent (no space) — `@ py` is a punct + ident.
        let mut j = start + 1;
        while let Some(c) = self.peek_at(j) {
            if is_xid_continue(c) {
                j += c.len_utf8();
            } else {
                break;
            }
        }
        if j == start + 1 {
            self.advance(1);
            self.push_tok(Kind::Punct(Punct::At), start, self.pos);
            return;
        }
        let name = self.slice(start + 1, j);
        if let Some(lang) = embed_lang_of(&name) {
            let end = j;
            self.pos = end;
            self.lex_embed(h, lang.to_string(), start, end);
            return;
        }
        // Not in §7.5's whitelist. Emitted as `@` + ident so the parser can still recover;
        // E-0702 is the lexer's to report because the whitelist is a *lexical* closed set.
        // Only at a token-start position: `x @ _` (§3.12 sub-pattern binding) is legal and the
        // `@` there follows an expression, so it must not be accused.
        if !self.prev_is_value_end() {
            let d = self
                .mk("E-0702", start, j, &format!("未知或不支持的嵌入前缀 `@{name}`"))
                .note("§7.5：不在任一白名单的 `@xxx` → E-0702（A 轨 c/cpp/rust/zig；B 轨 py/lua/ts/js/sh/ps1/bat/cmd）")
                .help("用 §7.5 白名单内前缀");
            self.emit(h, d);
        }
        self.advance(1);
        self.push_tok(Kind::Punct(Punct::At), start, start + 1);
    }

    fn prev_is_value_end(&self) -> bool {
        match self.toks.last() {
            Some(t) => matches!(
                t.kind,
                Kind::Ident(_)
                    | Kind::Lit(_)
                    | Kind::Punct(Punct::RParen | Punct::RBracket | Punct::RBrace)
                    | Kind::Lifetime(_)
            ),
            None => false,
        }
    }
}

enum QuoteKind {
    Str,
    Cord,
}

enum Esc {
    Ch(char),
    Byte(u8),
}

fn is_valid_suffix(s: &str) -> bool {
    matches!(
        s,
        "u8" | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "f32"
            | "f64"
    )
}

/// §2.4/§7.5: the closed set of embed prefixes (`@` + name).
pub fn embed_lang_names() -> &'static [&'static str] {
    &["py", "lua", "ts", "js", "sh", "ps1", "bat", "cmd", "c", "cpp", "rust", "zig"]
}

fn embed_lang_of(name: &str) -> Option<&'static str> {
    embed_lang_names().iter().copied().find(|s| *s == name)
}

fn punct_of(s: &str) -> Punct {
    match s {
        "..=" => Punct::DotDotEq,
        ".." => Punct::DotDot,
        "." => Punct::Dot,
        "::" => Punct::ColonColon,
        ":" => Punct::Colon,
        "==" => Punct::EqEq,
        "=>" => Punct::FatArrow,
        "=" => Punct::Eq,
        "!=" => Punct::BangEq,
        "!" => Punct::Bang,
        "<<=" => Punct::ShlEq,
        "<<" => Punct::Shl,
        "<=" => Punct::LtEq,
        "<" => Punct::Lt,
        ">>=" => Punct::ShrEq,
        ">>" => Punct::Shr,
        ">=" => Punct::GtEq,
        ">" => Punct::Gt,
        "+=" => Punct::PlusEq,
        "+" => Punct::Plus,
        "->" => Punct::Arrow,
        "-=" => Punct::MinusEq,
        "-" => Punct::Minus,
        "*=" => Punct::StarEq,
        "*" => Punct::Star,
        "%=" => Punct::PercentEq,
        "%" => Punct::Percent,
        "/=" => Punct::SlashEq,
        "/" => Punct::Slash,
        "&&" => Punct::AmpAmp,
        "&=" => Punct::AmpEq,
        "&" => Punct::Amp,
        "||" => Punct::PipePipe,
        "|=" => Punct::PipeEq,
        "|" => Punct::Pipe,
        "^=" => Punct::HatEq,
        "^" => Punct::Hat,
        _ => Punct::Tilde,
    }
}

// ---------------------------------------------------------------------------------------------
// Public entry points (frozen API)
// ---------------------------------------------------------------------------------------------

/// Lex one file, whose text lives in `sources.get(file)`.
///
/// Diagnostics go to `handler`; the token stream is returned even for files that contain errors so
/// `infy fmt` and the parser can recover (§9.4 requires `check` and `build` to agree on the
/// diagnostic set for a file).
///
/// E-0012 is *not* emitted here: the constraint name set is only complete once the manifest's
/// `features` (§5.2) are known, and inventing a false positive would break §9.4's agreement
/// guarantee. Use [`lex_file_with_features`] (or [`check_constraint_names`] from `pkg`).
pub fn lex_file(sources: &Sources, file: FileId, handler: &mut Handler) -> Lexed {
    lex_file_inner(sources, file, handler, &[], false)
}

/// §2.2: `pkg`-facing entry point. Identical to [`lex_file`] but the closed-set check for
/// constraint names runs with the module's declared `features`, so unknown names report E-0012
/// from the lex phase (one diagnostic per offending comment).
pub fn lex_file_with_features(
    sources: &Sources,
    file: FileId,
    handler: &mut Handler,
    features: &[&str],
) -> Lexed {
    lex_file_inner(sources, file, handler, features, true)
}

fn lex_file_inner(
    sources: &Sources,
    file: FileId,
    handler: &mut Handler,
    features: &[&str],
    check_e0012: bool,
) -> Lexed {
    // §2.1 E-0002, defensively: unreachable for a real `String` (see the module header).
    // `lex_bytes` is the path that can actually report it.
    let raw: &[u8] = sources.get(file).map(|f| f.text.as_bytes()).unwrap_or(&[]);
    if let Some((off, len)) = check_utf8(raw) {
        let line = sources
            .get(file)
            .map(|f| f.byte_line_col(u32o(off)).0)
            .unwrap_or(1);
        let line_start = sources
            .get(file)
            .and_then(|f| f.line_starts().get((line as usize).saturating_sub(1)).copied())
            .unwrap_or(0) as usize;
        let column = off.saturating_sub(line_start) + 1;
        let d = Diagnostic::new("E-0002", Phase::Lex, file, Position { line, column: column as u32 })
            .titled(format!(
                "源码非 UTF-8：第 {line} 行第 {column} 列（字节偏移 {off}）起有长度 {len} 的非法字节序列"
            ))
            .note("§2.1：强制 UTF-8，非 UTF-8 源码 → E-0002；该文件未被词法化")
            .help("转为 UTF-8（推荐无 BOM）");
        handler.emit(d);
        return Lexed {
            toks: vec![Tok {
                kind: Kind::Eof,
                span: Span::new(file, 0, 0),
                text: String::new(),
            }],
            diagnostics_emitted: 1,
            ..Lexed::default()
        };
    }
    let Some(mut lx) = Lexer::new(sources, file, features, check_e0012) else {
        return Lexed::default();
    };
    lx.take_shebang();
    lx.file_shape_checks(handler);
    loop {
        lx.skip_trivia(handler);
        if lx.at_eof() {
            break;
        }
        let before = lx.pos;
        lx.lex_at_token(handler);
        // Safety net: a token function must consume at least one byte, so a malformed file can
        // never spin the loop.
        if lx.pos <= before {
            lx.advance(1);
        }
    }
    let eof_at = lx.text.len();
    lx.push_tok(Kind::Eof, eof_at, eof_at);
    Lexed {
        toks: lx.toks,
        comments: lx.comments,
        shebang: lx.shebang,
        build_constraint: lx.build_constraint,
        diagnostics_emitted: lx.emitted.min(MAX_TOTAL),
    }
}

/// The CLI's real entry point (§13.1/§13.4): validate the raw bytes, report E-0002 *before*
/// decoding, and otherwise lex. `display_path` is added to `sources` (with a lossy copy of the
/// text, so the diagnostic can still print the offending line) when it is not already there.
///
/// Returns `(FileId, Lexed)`; on E-0002 the token stream is just `Eof`.
pub fn lex_bytes(
    sources: &mut Sources,
    display_path: &str,
    bytes: &[u8],
    handler: &mut Handler,
) -> (FileId, Lexed) {
    if let Some((off, len)) = check_utf8(bytes) {
        let lossy = String::from_utf8_lossy(bytes).into_owned();
        let file = if let Some(id) = sources.id_of_path(display_path) {
            id
        } else {
            sources.add(display_path.to_string(), lossy)
        };
        let line = bytes[..off].iter().filter(|b| **b == b'\n').count() as u32 + 1;
        let line_start = bytes[..off].iter().rposition(|b| *b == b'\n').map(|p| p + 1).unwrap_or(0);
        let column = String::from_utf8_lossy(&bytes[line_start..off]).chars().count() as u32 + 1;
        let d = Diagnostic::new("E-0002", Phase::Lex, file, Position { line, column })
            .titled(format!(
                "源码非 UTF-8：第 {line} 行第 {column} 列起有长度 {len} 的非法字节序列（字节偏移 {off}）"
            ))
            .note("§2.1：强制 UTF-8，非 UTF-8 源码 → E-0002；文件未被词法化")
            .help("转为 UTF-8（无 BOM）后重试");
        handler.emit(d);
        return (
            file,
            Lexed {
                toks: vec![Tok {
                    kind: Kind::Eof,
                    span: Span::new(file, 0, 0),
                    text: String::new(),
                }],
                diagnostics_emitted: 1,
                ..Lexed::default()
            },
        );
    }
    let text = String::from_utf8_lossy(bytes).into_owned();
    if sources.id_of_path(display_path).is_none() {
        sources.add(display_path.to_string(), text);
    }
    let file = sources.id_of_path(display_path).unwrap_or(0);
    (file, lex_file(sources, file, handler))
}

/// §2.6: adjacent literal concatenation (`"a" "b"`) is folded by the PARSER, not here — this is
/// the only hook it needs to recognise the run.
pub fn is_concatenatable_lit(t: &Tok) -> bool {
    matches!(t.kind, Kind::Lit(Lit::Str(_)))
}

// ---------------------------------------------------------------------------------------------
// Token trees (§3.9 / §6.2)
// ---------------------------------------------------------------------------------------------

fn open_delim(k: &Kind) -> Option<(Delim, Punct)> {
    match k {
        Kind::Punct(Punct::LParen) => Some((Delim::Paren, Punct::RParen)),
        Kind::Punct(Punct::LBracket) => Some((Delim::Bracket, Punct::RBracket)),
        Kind::Punct(Punct::LBrace) => Some((Delim::Brace, Punct::RBrace)),
        _ => None,
    }
}

fn close_delim(k: &Kind) -> Option<Delim> {
    match k {
        Kind::Punct(Punct::RParen) => Some(Delim::Paren),
        Kind::Punct(Punct::RBracket) => Some(Delim::Bracket),
        Kind::Punct(Punct::RBrace) => Some(Delim::Brace),
        _ => None,
    }
}

/// Group flat tokens into token trees (§3.9/§6.2), stopping after the first unmatched closing
/// delimiter. Returns the trees and the number of input tokens consumed.
///
/// Never panics: an unmatched closer (or EOF with open groups) closes nothing — the still-open
/// frames' children are flattened into the enclosing level, which is the same shape the macro
/// matcher sees for a truncated invocation.
pub fn build_token_tree(toks: &[Tok]) -> (Vec<Tree>, usize) {
    #[derive(Clone)]
    struct Frame {
        delim: Delim,
        children: Vec<Tree>,
        open: Span,
    }
    let mut frames: Vec<Frame> = Vec::new();
    let mut top: Vec<Tree> = Vec::new();
    let mut consumed = 0usize;

    for (idx, t) in toks.iter().enumerate() {
        consumed = idx + 1;
        if let Some((d, _)) = open_delim(&t.kind) {
            frames.push(Frame {
                delim: d,
                children: Vec::new(),
                open: t.span,
            });
            continue;
        }
        if let Some(d) = close_delim(&t.kind) {
            let matched = frames.last().map(|f| f.delim == d).unwrap_or(false);
            if !matched {
                // unmatched / mismatched closer: stop cleanly before it
                consumed = idx;
                break;
            }
            let frame = match frames.pop() {
                Some(f) => f,
                None => {
                    consumed = idx;
                    break;
                }
            };
            let span = frame.open.merge(t.span);
            let group = Tree::Group(frame.delim, frame.children, span);
            match frames.last_mut() {
                Some(f) => f.children.push(group),
                None => top.push(group),
            }
            continue;
        }
        let leaf = Tree::Leaf(t.clone());
        match frames.last_mut() {
            Some(f) => f.children.push(leaf),
            None => top.push(leaf),
        }
    }
    // Unclosed frames: splice their leaves up so no token is silently dropped.
    while let Some(frame) = frames.pop() {
        for child in frame.children {
            match frames.last_mut() {
                Some(f) => f.children.push(child),
                None => top.push(child),
            }
        }
    }
    (top, consumed)
}

/// The **single** delimited group starting at `toks[0]`, plus how many tokens it took.
///
/// [`build_token_tree`] is written for `macro_rules!` bodies, so it keeps scanning past a closed
/// top-level group until the next unmatched closer. A call site must stop at its own closer: the
/// parser used to take only the first tree while consuming everything in between, which silently
/// deleted the rest of a `match` arm list after `name!(…)` (BACKLOG F63).
pub fn build_call_group(toks: &[Tok]) -> Option<(Delim, Vec<Tree>, usize)> {
    let (d, _) = open_delim(&toks.first()?.kind)?;
    let mut depth = 0usize;
    for (idx, t) in toks.iter().enumerate() {
        if open_delim(&t.kind).is_some() {
            depth += 1;
            continue;
        }
        if close_delim(&t.kind).is_some() {
            if depth == 0 {
                return None; // a closer that does not match any opener: the group never closed
            }
            depth -= 1;
            if depth == 0 {
                let (tree, n) = build_token_tree(&toks[..idx + 1]);
                if n == 0 {
                    return None;
                }
                return Some((d, tree, n));
            }
        }
    }
    None
}

// =============================================================================================
// Conformance seed for §12.1's "词法 96 例": one test per diagnostic the lexer can emit, plus a
// positive case for every literal form of §2.6/§2.8/§2.9, the whole §2.7 operator table, the §2.4
// reserved set, `@embed` (§2.11/§3.6), nested comments (§2.2), token trees (§3.9) and the §14 tour.
// Tests never read `codes`' table titles — only codes, kinds and values.
// =============================================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::{CommentKind, Kind, Kw, Lit, Punct, Radix};

    /// Lex `text`, appending the trailing LF §2.1 asks for so `W-0002` does not pollute the
    /// positive cases. Returns the stream, the diagnostic codes in emission order, and the sources.
    fn lex(text: &str) -> (Lexed, Vec<String>) {
        let mut t = text.to_string();
        if !t.ends_with('\n') {
            t.push('\n');
        }
        let mut src = Sources::default();
        let f = src.add("t.infy".to_string(), t);
        let mut h = Handler::default();
        let lx = lex_file(&src, f, &mut h);
        (lx, h.diags.iter().map(|d| d.code.clone()).collect())
    }

    /// Lex exactly as written (no appended LF) — for the §2.1 line-ending cases.
    fn lex_raw(text: &str) -> (Lexed, Vec<String>) {
        let mut src = Sources::default();
        let f = src.add("t.infy".to_string(), text.to_string());
        let mut h = Handler::default();
        let lx = lex_file(&src, f, &mut h);
        (lx, h.diags.iter().map(|d| d.code.clone()).collect())
    }

    fn codes(text: &str) -> Vec<String> {
        lex(text).1
    }

    fn kinds(text: &str) -> Vec<Kind> {
        toks(text).iter().map(|t| t.kind.clone()).collect()
    }

    /// The token stream without the trailing `Eof`.
    fn toks(text: &str) -> Vec<Tok> {
        let mut v: Vec<Tok> = lex(text).0.toks;
        assert!(v.last().map(|t| t.is_eof()).unwrap_or(false), "stream must end with Eof");
        v.pop();
        v
    }

    fn clean(text: &str) -> Lexed {
        let (lx, c) = lex(text);
        assert!(c.is_empty(), "expected a lex-clean file, got {c:?}\n--- source ---\n{text}");
        lx
    }

    fn has(text: &str, code: &str) {
        let c = codes(text);
        assert!(c.iter().any(|x| x == code), "expected `{code}` among {c:?}\n--- source ---\n{text}");
    }

    fn lacks(text: &str, code: &str) {
        let c = codes(text);
        assert!(!c.iter().any(|x| x == code), "did not expect `{code}`, got {c:?}\n{text}");
    }

    fn id(s: &str) -> Kind {
        Kind::Ident(s.to_string())
    }
    fn kw(k: Kw) -> Kind {
        Kind::Keyword(k)
    }
    fn pu(p: Punct) -> Kind {
        Kind::Punct(p)
    }

    fn ints(text: &str) -> Vec<(String, Radix, Option<String>, Option<u128>)> {
        lex(text)
            .0
            .toks
            .iter()
            .filter_map(|t| match &t.kind {
                Kind::Lit(Lit::Int(i)) => {
                    Some((i.digits.clone(), i.radix, i.suffix.clone(), i.value))
                }
                _ => None,
            })
            .collect()
    }

    fn floats(text: &str) -> Vec<(String, Option<String>, Option<String>)> {
        lex(text)
            .0
            .toks
            .iter()
            .filter_map(|t| match &t.kind {
                Kind::Lit(Lit::Float(f)) => {
                    Some((f.mantissa.clone(), f.exp.clone(), f.suffix.clone()))
                }
                _ => None,
            })
            .collect()
    }

    fn strs(text: &str) -> Vec<String> {
        lex(text)
            .0
            .toks
            .iter()
            .filter_map(|t| match &t.kind {
                Kind::Lit(Lit::Str(s)) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    // -- §2.1 encoding, BOM, line endings ------------------------------------------------------

    #[test]
    fn check_utf8_finds_invalid_sequences() {
        assert_eq!(check_utf8(b"let x = 1;\n"), None);
        assert_eq!(check_utf8(&[0xFF, 0xFE]), Some((0, 1)));
        // A truncated multi-byte sequence at EOF reports the remaining length.
        assert_eq!(check_utf8(&[b'a', 0xE4, 0xB8]), Some((1, 2)));
        assert_eq!(check_utf8("漢字".as_bytes()), None);
    }

    #[test]
    fn lex_bytes_reports_e0002_and_refuses_to_lex() {
        let mut src = Sources::default();
        let mut h = Handler::default();
        let (_f, lx) = lex_bytes(&mut src, "bad.infy", &[b'l', b'e', b't', 0xC3, 0x28], &mut h);
        assert_eq!(h.diags.len(), 1, "{:?}", h.diags.iter().map(|d| &d.code).collect::<Vec<_>>());
        assert_eq!(h.diags[0].code, "E-0002");
        assert_eq!(lx.toks.len(), 1);
        assert!(lx.toks[0].is_eof());
        assert_eq!(lx.diagnostics_emitted, 1);
    }

    #[test]
    fn lex_bytes_accepts_valid_utf8() {
        let mut src = Sources::default();
        let mut h = Handler::default();
        let (f, lx) = lex_bytes(&mut src, "ok.infy", "package ok\n".as_bytes(), &mut h);
        assert!(h.diags.is_empty());
        assert!(matches!(lx.toks[0].kind, Kind::Keyword(Kw::Package)));
        assert_eq!(src.get(f).map(|x| x.display_path.as_str()), Some("ok.infy"));
    }

    #[test]
    fn bom_is_skipped_once() {
        let (lx, c) = lex_raw("\u{FEFF}package a\n");
        assert!(c.is_empty(), "{c:?}");
        assert!(matches!(lx.toks[0].kind, Kind::Keyword(Kw::Package)));
    }

    #[test]
    fn crlf_warns_w0001() {
        let (_lx, c) = lex_raw("package a\r\nfn f() {}\r\n");
        assert_eq!(c.iter().filter(|x| x.as_str() == "W-0001").count(), 1, "{c:?}");
        assert!(!c.iter().any(|x| x.as_str() == "E-0003"), "{c:?}");
    }

    #[test]
    fn lone_cr_is_e0003() {
        let (_lx, c) = lex_raw("package a\rfn f() {}\n");
        assert_eq!(c.iter().filter(|x| x.as_str() == "E-0003").count(), 1, "{c:?}");
        assert!(!c.iter().any(|x| x.as_str() == "W-0001"), "{c:?}");
    }

    #[test]
    fn missing_trailing_lf_is_w0002() {
        let (_lx, c) = lex_raw("package a");
        assert!(c.iter().any(|x| x.as_str() == "W-0002"), "{c:?}");
        assert!(!codes("package a\n").iter().any(|x| x.as_str() == "W-0002"));
    }

    #[test]
    fn empty_file_is_e0090() {
        has("", "E-0090");
        has("\u{FEFF}", "E-0090");
    }

    // -- §2.2 comments -------------------------------------------------------------------------

    #[test]
    fn line_and_doc_comments_keep_verbatim_text() {
        let lx = clean("// plain body\n///  doc body\n/* block */\nfn f() {}\n");
        assert_eq!(lx.comments.len(), 3);
        assert_eq!(lx.comments[0].kind, CommentKind::Line);
        assert_eq!(lx.comments[0].text, " plain body");
        assert_eq!(lx.comments[1].kind, CommentKind::Doc);
        assert_eq!(lx.comments[1].text, "  doc body");
        assert_eq!(lx.comments[2].kind, CommentKind::Block);
        assert_eq!(lx.comments[2].text, " block ");
        // comments never appear in the token stream
        assert_eq!(kinds("/// doc\nfn f() {}\n"), vec![kw(Kw::Fn), id("f"), pu(Punct::LParen), pu(Punct::RParen), pu(Punct::LBrace), pu(Punct::RBrace)]);
    }

    #[test]
    fn four_slashes_is_an_ordinary_comment() {
        let lx = clean("//// not a doc comment\nfn f() {}\n");
        assert_eq!(lx.comments[0].kind, CommentKind::Line);
        assert_eq!(lx.comments[0].text, "// not a doc comment");
    }

    #[test]
    fn block_comments_nest() {
        let lx = clean("/* a /* b /* c */ d */ e */\nfn f() {}\n");
        assert_eq!(lx.comments.len(), 1);
        assert_eq!(lx.comments[0].text, " a /* b /* c */ d */ e ");
    }

    #[test]
    fn unterminated_block_comment_is_e0004() {
        has("/* open /* inner\nfn f() {}\n", "E-0004");
    }

    #[test]
    fn block_comment_depth_limit_is_e0005() {
        let deep = format!("{}{}\n", "/*".repeat(256), "*/".repeat(256));
        has(&deep, "E-0005");
        let ok = format!("{}{}\n", "/*".repeat(255), "*/".repeat(255));
        lacks(&ok, "E-0005");
    }

    // -- §2.2 build constraint -----------------------------------------------------------------

    #[test]
    fn build_constraint_on_first_line() {
        let lx = clean("//infy:build linux\nfn f() {}\n");
        let (raw, line, _) = lx.build_constraint.clone().expect("constraint captured");
        assert_eq!(raw, "linux");
        assert_eq!(line, 1);
    }

    #[test]
    fn build_constraint_after_shebang_and_blanks() {
        let lx = clean("#!/usr/bin/env -S infy run\n\n//infy:build windows && x86_64\nfn f() {}\n");
        assert_eq!(lx.shebang.as_deref(), Some("#!/usr/bin/env -S infy run"));
        let (raw, line, _) = lx.build_constraint.clone().expect("constraint captured");
        assert_eq!(raw, "windows && x86_64");
        assert_eq!(line, 3);
    }

    #[test]
    fn misplaced_build_constraint_is_e0011() {
        has("fn f() {}\n//infy:build linux\n", "E-0011");
        has("/// doc\n//infy:build linux\nfn f() {}\n", "E-0011");
    }

    #[test]
    fn duplicated_build_constraint_is_e0013() {
        let c = codes("//infy:build linux\n//infy:build windows\nfn f() {}\n");
        assert!(c.iter().any(|x| x.as_str() == "E-0013"), "{c:?}");
        assert_eq!(c.iter().filter(|x| x.as_str() == "E-0013").count(), 1);
    }

    #[test]
    fn constraint_expression_without_features_is_not_accused_by_lex_file() {
        // `lex_file` cannot see the manifest's `features`, so it must not emit E-0012.
        lacks("//infy:build riscv\nfn f() {}\n", "E-0012");
    }

    #[test]
    fn unknown_constraint_name_is_e0012_when_features_are_known() {
        let mut src = Sources::default();
        let f = src.add("t.infy".to_string(), "//infy:build riscv && gpu\nfn f() {}\n".to_string());
        let mut h = Handler::default();
        let lx = lex_file_with_features(&src, f, &mut h, &["gpu"]);
        let c: Vec<String> = h.diags.iter().map(|d| d.code.clone()).collect();
        assert!(c.iter().any(|x| x.as_str() == "E-0012"), "{c:?}");
        assert_eq!(lx.build_constraint.as_ref().unwrap().0, "riscv && gpu");
        // With the real feature set declared, nothing is unknown.
        let mut h2 = Handler::default();
        lex_file_with_features(&src, f, &mut h2, &["gpu", "riscv"]);
        assert!(!h2.diags.iter().any(|d| d.code == "E-0012"), "{:?}", h2.diags.iter().map(|d| &d.code).collect::<Vec<_>>());
    }

    #[test]
    fn constraint_names_helper_returns_unknown_only() {
        assert!(check_constraint_names("windows && x86_64 || !a_track", &[]).is_empty());
        assert_eq!(check_constraint_names("riscv", &[]), vec!["riscv".to_string()]);
        assert_eq!(check_constraint_names("riscv && gpu", &["gpu"]), vec!["riscv".to_string()]);
        assert_eq!(
            check_constraint_names("a && b && a", &[]),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(known_constraint_names().contains(&"ignore"));
    }

    // -- §2.3 / §2.4 identifiers and keywords ---------------------------------------------------

    #[test]
    fn identifiers_are_unicode_xid() {
        assert!(is_xid_start('汉') && is_xid_start('あ') && is_xid_start('ア') && is_xid_start('한'));
        assert!(is_xid_start('_') && is_xid_start('a') && !is_xid_start('1'));
        assert!(!is_xid_start('\u{300}'), "a combining mark may not start an identifier");
        assert!(is_xid_continue('\u{301}') && is_xid_continue('9') && is_xid_continue('_'));
        assert_eq!(kinds("let 变量_1 = 漢字;\n")[1], id("变量_1"));
        assert_eq!(kinds("let x = _y;\n")[3], id("_y"));
        assert_eq!(kinds("let _ = 1;\n")[1], id("_"));
        assert_eq!(kinds("let a = Foo; let b = foo;\n")[3], id("Foo"));
    }

    #[test]
    fn double_underscore_prefix_is_e0007() {
        has("let __reserved = 1;\n", "E-0007");
        lacks("let _single = 1;\n", "E-0007");
        lacks("let _ = 1;\n", "E-0007");
    }

    #[test]
    fn keywords_are_reserved_words() {
        let table: &[(&str, Kw)] = &[
            ("module", Kw::Module), ("package", Kw::Package), ("import", Kw::Import),
            ("pub", Kw::Pub), ("priv", Kw::Priv), ("mod", Kw::Mod), ("event", Kw::Event),
            ("fn", Kw::Fn), ("struct", Kw::Struct), ("enum", Kw::Enum), ("union", Kw::Union),
            ("trait", Kw::Trait), ("impl", Kw::Impl), ("type", Kw::Type), ("const", Kw::Const),
            ("static", Kw::Static), ("let", Kw::Let), ("var", Kw::Var), ("extern", Kw::Extern),
            ("unsafe", Kw::Unsafe), ("link", Kw::Link), ("owns", Kw::Owns),
            ("borrows", Kw::Borrows), ("label", Kw::Label), ("u8", Kw::U8), ("u16", Kw::U16),
            ("u32", Kw::U32), ("u64", Kw::U64), ("u128", Kw::U128), ("i8", Kw::I8),
            ("i16", Kw::I16), ("i32", Kw::I32), ("i64", Kw::I64), ("i128", Kw::I128),
            ("f32", Kw::F32), ("f64", Kw::F64), ("bool", Kw::BoolTy), ("str", Kw::Str),
            ("char", Kw::Char), ("dyn", Kw::Dyn), ("Self", Kw::SelfTy), ("self", Kw::SelfVal),
            ("true", Kw::True), ("false", Kw::False), ("if", Kw::If), ("else", Kw::Else),
            ("while", Kw::While), ("loop", Kw::Loop), ("for", Kw::For), ("in", Kw::In),
            ("break", Kw::Break), ("continue", Kw::Continue), ("return", Kw::Return),
            ("match", Kw::Match), ("chain", Kw::Chain), ("start_flag", Kw::StartFlag),
            ("goto", Kw::Goto), ("start", Kw::Start), ("stop", Kw::Stop), ("pause", Kw::Pause),
            ("resume", Kw::Resume), ("linkage", Kw::Linkage), ("as", Kw::As), ("is", Kw::Is),
            ("where", Kw::Where), ("move", Kw::Move), ("auto", Kw::Auto), ("mut", Kw::Mut),
            ("ref", Kw::Ref), ("defer", Kw::Defer), ("const_assert", Kw::ConstAssert),
            ("use", Kw::Use),
        ];
        for (text, k) in table {
            let src = format!("{text}\n");
            let ks = kinds(&src);
            assert_eq!(ks.len(), 1, "{text} produced {ks:?}");
            assert_eq!(ks[0], kw(*k), "{text} must lex as {:?} and round-trip its text", k);
            assert_eq!(k.text(), *text);
        }
    }

    #[test]
    fn reserved_but_unused_words_are_e0020() {
        for w in ["use", "mod", "union", "linkage"] {
            has(&format!("let {w} = 1;\n"), "E-0020");
        }
        lacks("let model = 1;\nlet unified = 1;\n", "E-0020");
    }

    #[test]
    fn macro_rules_bang_is_one_token() {
        let ks = kinds("macro_rules! m { () => {}; }\n");
        assert_eq!(ks[0], kw(Kw::MacroRules));
        assert_eq!(clean("macro_rules! m {}\n").toks[0].text, "macro_rules!");
        // The `!` is part of the keyword: no separate Bang punct is emitted.
        assert!(!ks.iter().any(|k| *k == pu(Punct::Bang)));
        // plain `macro_rules` (no bang) is just an identifier
        assert_eq!(kinds("let macro_rules = 1;\n")[1], id("macro_rules"));
    }

    #[test]
    fn keywords_are_not_identifiers_and_are_case_sensitive() {
        assert_eq!(kinds("let IF = 1;\n")[1], id("IF"));
        assert_eq!(kinds("let Str = 1;\n")[1], id("Str"));
    }

    // -- §2.10 script / confusable checks ------------------------------------------------------

    #[test]
    fn mixed_script_inside_one_identifier() {
        has("let аdmin = 1;\n", "E-0050"); // Cyrillic а + Latin d (§2.10's example)
        has("let 汉한 = 1;\n", "E-0051"); // Han + Hangul (§2.10's example)
        lacks("let 漢字ひらがな = 1;\n", "E-0051"); // Han + kana is legal (§2.10)
        lacks("let 漢字カタカナ = 1;\n", "E-0050");
        lacks("let normal_1 = 1;\n", "E-0050");
        lacks("let normal_1 = 1;\n", "E-0051");
    }

    #[test]
    fn skeleton_folds_confusables_and_marks() {
        assert_eq!(skeleton("аdmin"), skeleton("admin"));
        assert_eq!(skeleton("ＰＹ"), skeleton("py"));
        assert_eq!(skeleton("e\u{301}"), skeleton("e"));
        assert_ne!(skeleton("abc"), skeleton("abd"));
        assert_eq!(skeleton("A\u{200b}B"), skeleton("ab"));
    }

    #[test]
    fn invisible_characters_outside_strings_are_e0052() {
        has("let a\u{200b}b = 1;\n", "E-0052");
        has("let x = 1;\u{202e}\n", "E-0052");
        has("// trailing \u{202d} direction mark\n", "E-0052");
        has("/* \u{200f} inside a block comment */\n", "E-0052");
        lacks("let s = \"a\u{200b}b\";\n", "E-0052");
        lacks("let r = r\"a\u{200f}b\";\n", "E-0052");
        lacks("@py(a = 1) { \u{202e} }\n", "E-0052");
    }

    #[test]
    fn stray_characters_use_the_source_charset_code() {
        has("let x = `quote`;\n", "E-0052");
        has("let nbsp = 1;\u{a0}\n", "E-0052");
    }

    // -- §2.5 lifetimes ------------------------------------------------------------------------

    #[test]
    fn lifetimes_are_their_own_kind() {
        let ks = kinds("fn f<'a, 'static>(x: &'a u8) -> u8 { 0 }\n");
        assert_eq!(ks[2], pu(Punct::Lt));
        assert_eq!(ks[3], Kind::Lifetime("a".to_string()));
        assert_eq!(ks[5], Kind::Lifetime("static".to_string()));
        assert_eq!(ks.iter().filter(|k| matches!(k, Kind::Lifetime(_))).count(), 3);
        let ks = kinds("#[borrows('static)]\nfn f() {}\n");
        assert_eq!(ks[0], Kind::AttrOpen { inner: false });
        assert_eq!(ks[3], Kind::Lifetime("static".to_string()));
        assert_eq!(clean("'a\n").toks[0].text, "'a");
    }

    // -- §2.6 / §2.8 numeric literals ----------------------------------------------------------

    #[test]
    fn integer_forms_from_the_spec() {
        assert_eq!(
            ints("let a = 123u32;\n"),
            vec![("123".to_string(), Radix::Dec, Some("u32".to_string()), Some(123))]
        );
        assert_eq!(
            ints("let a = 0x_FFu8;\n"),
            vec![("FF".to_string(), Radix::Hex, Some("u8".to_string()), Some(255))]
        );
        assert_eq!(ints("let a = 0o777;\n"), vec![("777".to_string(), Radix::Oct, None, Some(511))]);
        assert_eq!(
            ints("let a = 0b1010i64;\n"),
            vec![("1010".to_string(), Radix::Bin, Some("i64".to_string()), Some(10))]
        );
        assert_eq!(ints("let a = 1_000_000;\n")[0].3, Some(1_000_000));
        assert_eq!(ints("let a = 12usize;\n")[0].2.as_deref(), Some("usize"));
        assert_eq!(ints("let a = 13isize;\n")[0].2.as_deref(), Some("isize"));
        assert_eq!(ints("let a = 18446744073709551615u64;\n")[0].3, Some(u64::MAX as u128));
        assert_eq!(clean("0x_FFu8 == 0xFFu8\n").toks.len(), 4);
    }

    #[test]
    fn range_errors_are_left_to_sema() {
        // 300u8 / 129i8 are SUFFIXED range errors (E-0043); a value above u128 clears `value`
        // (E-0042) — the lexer must not decide either.
        let lx = clean("let a = 300u8; let b = 129i8; let c = -128i8;\n");
        assert!(lx.diagnostics_emitted == 0);
        let got = ints("let c = 340282366920938463463374607431768211456;\n");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].3, None, "overflowing u128 must leave value: None for sema");
        lacks("let a = 300u8;\n", "E-0042");
        lacks("let a = 300u8;\n", "E-0043");
        lacks("let a = 123;\n", "E-0008");
    }

    #[test]
    fn float_forms() {
        assert_eq!(
            floats("let a = 3.14f64;\n"),
            vec![("3.14".to_string(), None, Some("f64".to_string()))]
        );
        assert_eq!(
            floats("let a = 1.0e-5f32;\n"),
            vec![("1.0".to_string(), Some("-5".to_string()), Some("f32".to_string()))]
        );
        assert_eq!(
            floats("let a = 1.5e-3f32;\n"),
            vec![("1.5".to_string(), Some("-3".to_string()), Some("f32".to_string()))]
        );
        assert_eq!(floats("let a = 1e5;\n"), vec![("1".to_string(), Some("5".to_string()), None)]);
        assert_eq!(floats("let a = 1.5e+3;\n")[0].1.as_deref(), Some("+3"));
        assert_eq!(floats("let a = 1_0.2_5e1_0;\n")[0].0, "10.25");
    }

    #[test]
    fn dot_disambiguation() {
        // `x.0` is a tuple field access (§3.11); `1.0` is always one float.
        assert_eq!(
            kinds("x.0;\n"),
            vec![id("x"), pu(Punct::Dot), Kind::Lit(Lit::Int(IntLit {
                digits: "0".to_string(),
                radix: Radix::Dec,
                suffix: None,
                value: Some(0),
            })), pu(Punct::Semi)]
        );
        assert_eq!(kinds("1.0;\n")[0], Kind::Lit(Lit::Float(FloatLit {
            mantissa: "1.0".to_string(),
            exp: None,
            suffix: None,
        })));
        assert_eq!(kinds("x.0.abs();\n")[3], pu(Punct::Dot));
        // `1..2` and `1..=2` are ranges, never broken floats.
        assert_eq!(kinds("a = 1..2;\n")[3], pu(Punct::DotDot));
        assert_eq!(kinds("a = 1..=2;\n")[3], pu(Punct::DotDotEq));
        assert_eq!(kinds("1.5..2.5\n")[1], pu(Punct::DotDot));
        // a method call on an integer literal keeps the dot as a punct
        assert_eq!(kinds("1.min(2)\n")[1], pu(Punct::Dot));
        lacks("let a = 1.min(2);\n", "E-0044");
    }

    #[test]
    fn underscore_rules() {
        has("let a = 1__000;\n", "E-0040");
        has("let a = 1000_;\n", "E-0040");
        has("let a = 0x_FF_;\n", "E-0040");
        has("let a = 1._5;\n", "E-0041");
        has("let a = 1_e5;\n", "E-0041");
        has("let a = 1.5e_3;\n", "E-0041");
        lacks("let a = 1_000;\n", "E-0040");
        lacks("let a = 0x_FF_FF;\n", "E-0040");
        // `_123` is an identifier, so a decimal literal can never start with `_`
        assert_eq!(kinds("let a = _123;\n")[3], id("_123"));
        assert_eq!(codes("let a = 1_e5;\n").iter().filter(|c| *c == "E-0041").count(), 1);
    }

    #[test]
    fn trailing_dot_float_is_e0044() {
        has("let a = 1.;\n", "E-0044");
        has("let a = 1.\n", "E-0044");
        lacks("let a = 1.0;\n", "E-0044");
    }

    #[test]
    fn float_overflow_to_infinity_is_e0045() {
        has("let a = 1.0e999f32;\n", "E-0045");
        has("let a = 1e400;\n", "E-0045");
        has("let a = 1e39f32;\n", "E-0045");
        lacks("let a = 1e38f32;\n", "E-0045");
        lacks("let a = 1e308;\n", "E-0045");
        lacks("let a = 1e-400;\n", "E-0045");
    }

    #[test]
    fn literal_longest_match_and_adjacent_ident() {
        // `0b1010i64` must not lose its suffix; `123abc` is a literal plus an identifier.
        let ks = kinds("0b1010i64 123abc\n");
        assert!(matches!(ks[0], Kind::Lit(Lit::Int(_))));
        assert_eq!(ks[2], id("abc"));
        assert_eq!(ints("123abc\n")[0].0, "123");
        assert_eq!(ints("123abc\n")[0].2, None);
    }

    // -- §2.9 strings, byte strings, chars, cords ----------------------------------------------

    #[test]
    fn closed_escape_table_resolves() {
        let v = strs("let s = \"\\n\\r\\t\\0\\\\\\\"\\'\\e\\a\\b\\f\\v\";\n");
        assert_eq!(v[0], "\n\r\t\0\\\"\'\u{1b}\u{7}\u{8}\u{c}\u{b}");
        assert_eq!(strs("let s = \"\\u{1F600}\";\n")[0], "\u{1F600}");
        assert_eq!(strs("let s = \"a\\u{41}b\";\n")[0], "aAb");
    }

    #[test]
    fn unknown_escape_is_e0046() {
        has("let s = \"a\\qb\";\n", "E-0046");
        has("let s = \"\\u\";\n", "E-0046");
        has("let s = \"\\u7F62\";\n", "E-0046");
        has("let s = \"\\x4\";\n", "E-0046");
        has("let c = '\\u{110000}';\n", "E-0046");
        has("let c = '\\uD800';\n", "E-0046");
        lacks("let s = \"\\n\";\n", "E-0046");
        // a `\` before a newline is not in the closed table either (§2.9): E-0046
        has("let s = \"abc\\\n", "E-0046");
    }

    #[test]
    fn byte_escape_forbidden_in_str_lit() {
        has("let s = \"\\x41\";\n", "E-0047");
        has("let s = \"\\x{0041}\";\n", "E-0047");
        // allowed in char_lit and b".."
        assert_eq!(clean("let c = '\\x41';\n").toks[3].kind, Kind::Lit(Lit::Char('A')));
        // ... but a byte escape may only encode one byte
        has("let c = '\\x{1F600}';\n", "E-0046");
        let lx = clean("let b = b\"A\\x41B\";\n");
        assert_eq!(lx.toks[3].kind, Kind::Lit(Lit::ByteStr(vec![b'A', b'A', b'B'])));
    }

    #[test]
    fn unterminated_string_and_bare_newline_are_e0009() {
        has("let s = \"abc;\n", "E-0009");
        has("let s = \"a\nb\";\n", "E-0009");
        lacks("let s = \"ab\";\n", "E-0009");
        lacks("let s = \"a\\\"b\";\n", "E-0009");
    }

    #[test]
    fn raw_strings_count_delimiters() {
        assert_eq!(strs("let s = r\"C:\\Users\\me\";\n")[0], "C:\\Users\\me");
        // `r#".."#` needs a `"##` to close, so a single `#` stays inside the body
        assert_eq!(strs("let s = r##\"a\"#b\"##;\n")[0], "a\"#b");
        assert_eq!(strs("let s = r#\"a\"#;\n")[0], "a");
        has("let s = r#\"abc;\n", "E-0010");
        has("let s = r\"abc;\n", "E-0010");
        lacks("let s = r\"abc\";\n", "E-0010");
    }

    #[test]
    fn multiline_raw_dedents_at_lex_time() {
        let v = strs("let s = r\"\n    line1\n    line2\n\";\n");
        assert_eq!(v[0], "line1\nline2\n");
        // a single-line raw string is untouched
        assert_eq!(strs("let s = r\" keep  spaces \";\n")[0], " keep  spaces ");
        // the common prefix is the maximum one
        assert_eq!(strs("let s = r\"\n\t\ta\n\t\t\tb\n\";\n")[0], "a\n\tb\n");
        // blank lines do not lower the common prefix
        assert_eq!(strs("let s = r\"\n    a\n\n    b\n\";\n")[0], "a\n\nb\n");
    }

    #[test]
    fn mixed_indent_in_raw_prefix_is_e0048() {
        has("let s = r\"\n \ta\n \tb\n\";\n", "E-0048");
        lacks("let s = r\"\n\t\ta\n\t\tb\n\";\n", "E-0048");
        lacks("let s = r\"\n    a\n        b\n\";\n", "E-0048");
    }

    #[test]
    fn byte_strings_are_ascii_only() {
        assert_eq!(
            clean("let b = b\"plain\";\n").toks[3].kind,
            Kind::Lit(Lit::ByteStr(b"plain".to_vec()))
        );
        has("let b = b\"汉\";\n", "E-0047");
        has("let b = b\"\\u{4E00}\";\n", "E-0047");
        has("let b = b\"abc;\n", "E-0009");
    }

    #[test]
    fn char_lit_is_exactly_one_code_point() {
        assert_eq!(clean("let c = 'a';\n").toks[3].kind, Kind::Lit(Lit::Char('a')));
        assert_eq!(clean("let c = '汉';\n").toks[3].kind, Kind::Lit(Lit::Char('汉')));
        assert_eq!(clean("let c = '\\n';\n").toks[3].kind, Kind::Lit(Lit::Char('\n')));
        assert_eq!(
            clean("let c = '\\u{1F600}';\n").toks[3].kind,
            Kind::Lit(Lit::Char('\u{1F600}'))
        );
        assert_eq!(clean("let c = '+';\n").toks[3].kind, Kind::Lit(Lit::Char('+')));
        assert_eq!(clean("let c = '\\'';\n").toks[3].kind, Kind::Lit(Lit::Char('\'')));
        has("let c = 'e\u{301}';\n", "E-0049");
        has("let c = 'ab';\n", "E-0049");
        has("let c = '';\n", "E-0049");
    }

    #[test]
    fn cord_lit_normalises_at_lex_time() {
        let lx = clean("let c = c\"café\";\n");
        assert!(matches!(&lx.toks[3].kind, Kind::Lit(Lit::Cord(s)) if s == "café"));
        let lx2 = clean("let c = c\"ＰＹ\";\n");
        assert!(matches!(&lx2.toks[3].kind, Kind::Lit(Lit::Cord(s)) if s == "PY"));
        has("let c = c\"abc;\n", "E-0009");
    }

    #[test]
    fn adjacent_literals_stay_separate_tokens() {
        // §3.11: `"a" "b"` is folded by the parser, so the lexer must emit both.
        let ks = kinds("let s = \"a\" \"b\";\n");
        assert_eq!(
            ks[3..5],
            [Kind::Lit(Lit::Str("a".to_string())), Kind::Lit(Lit::Str("b".to_string()))]
        );
        assert!(is_concatenatable_lit(&clean("let s = \"a\";\n").toks[3]));
        assert!(!is_concatenatable_lit(&clean("let s = b\"a\";\n").toks[3]));
    }

    #[test]
    fn no_interpolation_in_strings() {
        // §2.9: `"${x}"` is not a thing; `{}`/`#` inside a literal are plain characters.
        let lx = clean("let s = \"fetch #{} ${x}\";\n");
        assert!(matches!(&lx.toks[3].kind, Kind::Lit(Lit::Str(v)) if v == "fetch #{} ${x}"));
    }

    // -- §2.7 operators ------------------------------------------------------------------------

    #[test]
    fn operator_table_longest_first() {
        let table: &[(&str, Punct)] = &[
            (".", Punct::Dot), ("..", Punct::DotDot), ("..=", Punct::DotDotEq),
            (",", Punct::Comma), (";", Punct::Semi), (":", Punct::Colon),
            ("::", Punct::ColonColon), ("(", Punct::LParen), (")", Punct::RParen),
            ("[", Punct::LBracket), ("]", Punct::RBracket), ("{", Punct::LBrace),
            ("}", Punct::RBrace), ("!", Punct::Bang), ("=", Punct::Eq),
            ("=>", Punct::FatArrow), ("->", Punct::Arrow), ("?", Punct::Question),
            ("&", Punct::Amp), ("&&", Punct::AmpAmp), ("|", Punct::Pipe),
            ("||", Punct::PipePipe), ("^", Punct::Hat), ("*", Punct::Star),
            ("%", Punct::Percent), ("+", Punct::Plus), ("-", Punct::Minus),
            ("<", Punct::Lt), ("<<=", Punct::ShlEq), ("<<", Punct::Shl),
            ("<=", Punct::LtEq), (">", Punct::Gt), (">>=", Punct::ShrEq),
            (">>", Punct::Shr), (">=", Punct::GtEq), ("==", Punct::EqEq),
            ("!=", Punct::BangEq), ("+=", Punct::PlusEq), ("-=", Punct::MinusEq),
            ("*=", Punct::StarEq), ("/", Punct::Slash), ("/=", Punct::SlashEq),
            ("%=", Punct::PercentEq), ("&=", Punct::AmpEq), ("^=", Punct::HatEq),
            ("|=", Punct::PipeEq), ("~", Punct::Tilde), ("#", Punct::Hash),
            ("@", Punct::At), ("$", Punct::Dollar),
        ];
        for (text, p) in table {
            let ks = kinds(&format!(" {text} \n"));
            assert_eq!(ks, vec![pu(*p)], "`{text}` must lex as exactly one {p:?}");
        }
    }

    #[test]
    fn precedence_spine_lexes_greedily() {
        let ks = kinds("a <<= b >>= c .. d ..= e && f || g & h | i ^ j\n");
        assert_eq!(
            ks,
            vec![
                id("a"), pu(Punct::ShlEq), id("b"), pu(Punct::ShrEq), id("c"),
                pu(Punct::DotDot), id("d"), pu(Punct::DotDotEq), id("e"),
                pu(Punct::AmpAmp), id("f"), pu(Punct::PipePipe), id("g"),
                pu(Punct::Amp), id("h"), pu(Punct::Pipe), id("i"), pu(Punct::Hat), id("j"),
            ]
        );
    }

    // -- §2.11 / §3.1 attribute openers, `@` prefixes, embed blocks -----------------------------

    #[test]
    fn attribute_openers_are_single_tokens() {
        assert_eq!(kinds("#[derive(Clone)]\n")[0], Kind::AttrOpen { inner: false });
        assert_eq!(kinds("#![entry_chain = \"P\"]\n")[0], Kind::AttrOpen { inner: true });
        // a lone `#` stays a punct
        assert_eq!(kinds("# x\n"), vec![pu(Punct::Hash), id("x")]);
    }

    #[test]
    fn embed_prefixes_are_keyword_like_tokens() {
        for lang in embed_lang_names() {
            let src = format!("@{lang}(a = 1) {{ x }}\n");
            let ks = kinds(&src);
            assert_eq!(ks[0], Kind::EmbedLang(lang.to_string()), "@{lang}");
            assert!(matches!(ks[1], Kind::EmbedArgs(_)));
            assert!(matches!(ks[2], Kind::EmbedBody(_)));
        }
        assert_eq!(clean("@py(a = 1) {{}}\n").toks[0].text, "@py");
        // bare `@` (no adjacent name) stays punct: §3.12 sub-pattern binding
        let ks = kinds("case x @ _ => 1\n");
        assert_eq!(ks[2], pu(Punct::At));
        lacks("case x @ _ => 1\n", "E-0702");
        // `@ py` with a space is not an embed prefix either
        assert_eq!(kinds("@ py {{}}\n")[0], pu(Punct::At));
    }

    #[test]
    fn unknown_embed_prefix_is_e0702() {
        has("@forth(a = 1) {{ x }}\n", "E-0702");
        has("@java {{ x }}\n", "E-0702");
        // §3.12's sub-pattern binding `x @ y` must never be accused: the `@` follows a value.
        lacks("case x @ y => 1\n", "E-0702");
        assert_eq!(kinds("case x @ y => 1\n")[2], pu(Punct::At));
    }

    #[test]
    fn embed_block_is_lexically_opaque() {
        // §2.11: no escape, comment or string interpretation inside the body — only `{`/`}`.
        let src = "@py(a = 1) {\n    s = \"{ } in a string\"\n    t = \"/* not a comment */\"\n    u = back\\\\slash\\n\n    if x { y }\n}\n";
        let lx = clean(src);
        assert!(lx.comments.is_empty(), "the body must not produce comments");
        let body = match &lx.toks[2].kind {
            Kind::EmbedBody(b) => b.clone(),
            k => panic!("expected EmbedBody, got {k:?}"),
        };
        assert!(body.contains("\"{ } in a string\""), "{body}");
        assert!(body.contains("/* not a comment */"));
        assert!(body.contains("back\\\\slash"));
        let args = match &lx.toks[1].kind {
            Kind::EmbedArgs(a) => a.clone(),
            k => panic!("expected EmbedArgs, got {k:?}"),
        };
        assert_eq!(args, "a = 1");
        // tokens after the block keep flowing
        assert_eq!(lx.toks[3].kind, Kind::Eof);
    }

    #[test]
    fn embed_args_skip_parens_inside_strings() {
        let lx = clean("@js(isolation = \"process)\", name = \"x\") { y }\n");
        let args = match &lx.toks[1].kind {
            Kind::EmbedArgs(a) => a.clone(),
            k => panic!("{k:?}"),
        };
        assert_eq!(args, "isolation = \"process)\", name = \"x\"");
        assert_eq!(lx.toks[2].kind, Kind::EmbedBody(" y ".to_string()));
    }

    #[test]
    fn embed_body_without_open_brace_is_left_to_the_parser() {
        // The parser owns "missing body" (§3.6/§9.7 report E-0053 there); the lexer stays silent so
        // the source is not diagnosed twice.
        let (lx, c) = lex("@py(a = 1) fn f() {}\n");
        assert!(c.is_empty(), "{c:?}");
        assert!(matches!(lx.toks[0].kind, Kind::EmbedLang(_)));
        assert!(matches!(lx.toks[1].kind, Kind::EmbedArgs(_)));
        assert!(matches!(lx.toks[2].kind, Kind::Keyword(Kw::Fn)));
    }

    #[test]
    fn unclosed_embed_body_is_e0010() {
        has("@py(a = 1) { if x { y\n", "E-0010");
        has("@c { \n", "E-0010");
        lacks("@c { if x { y } }\n", "E-0010");
    }

    #[test]
    fn embed_block_after_another_token_still_lexes() {
        let ks = kinds("let v = @lua(n = 2) { return n }\n");
        assert_eq!(ks[3], Kind::EmbedLang("lua".to_string()));
        assert!(matches!(ks[4], Kind::EmbedArgs(_)));
        assert!(matches!(ks[5], Kind::EmbedBody(_)));
    }

    // -- §3.9 / §6.2 token trees ---------------------------------------------------------------

    #[test]
    fn token_trees_group_all_three_delimiters() {
        let ks = toks("a (b [c] {d}) e\n");
        let (ts, n) = build_token_tree(&ks);
        assert_eq!(n, ks.len());
        assert_eq!(ts.len(), 3);
        assert!(matches!(&ts[0], Tree::Leaf(t) if t.is_ident("a")));
        let group = match &ts[1] {
            Tree::Group(Delim::Paren, kids, _) => kids,
            other => panic!("expected a paren group, got {other:?}"),
        };
        assert_eq!(group.len(), 3, "the paren group holds b, [c] and {{d}}");
        assert!(matches!(&group[0], Tree::Leaf(t) if t.is_ident("b")));
        assert!(matches!(&group[1], Tree::Group(Delim::Bracket, inner, _) if inner.len() == 1));
        assert!(matches!(&group[2], Tree::Group(Delim::Brace, inner, _) if inner.len() == 1));
        assert!(matches!(&ts[2], Tree::Leaf(t) if t.is_ident("e")));
        // flatten materialises the delimiters again (§6.8 byte-stable reprinting)
        let flat = ts[1].leaves();
        assert_eq!(flat.len(), 9);
        assert!(flat[0].is_p(Punct::LParen) && flat[8].is_p(Punct::RParen));
    }

    #[test]
    fn token_trees_stop_at_an_unmatched_closer() {
        let ks = toks("a (b) ) c\n");
        let (ts, n) = build_token_tree(&ks);
        assert_eq!(n, 4, "must consume up to but not including the stray closer");
        assert_eq!(ts.len(), 2);
        assert!(matches!(&ts[1], Tree::Group(Delim::Paren, _, _)));
    }

    #[test]
    fn token_trees_stop_at_a_mismatched_closer() {
        let ks = toks("(a]\n");
        let (ts, n) = build_token_tree(&ks);
        assert_eq!(n, 2, "the `]` does not close the `(`, so it is not consumed");
        assert_eq!(ts.len(), 1);
        assert!(ts.iter().all(|t| matches!(t, Tree::Leaf(_))));
    }

    #[test]
    fn token_trees_never_panic_on_malformed_input() {
        for src in [")", ")))((( ", "[(]", "{[)", "(]", "[)", "(", "{", "}"] {
            let ks = toks(src);
            let (_ts, _n) = build_token_tree(&ks);
        }
        let (_ts, _n) = build_token_tree(&[]);
        // an unterminated group splices its children up rather than losing tokens
        let (ts, n) = build_token_tree(&toks("a (b c\n"));
        assert_eq!(n, 4);
        assert_eq!(ts.len(), 3);
    }

    #[test]
    fn token_tree_spans_merge_the_delimiters() {
        let ks = toks("(a)\n");
        let (ts, _) = build_token_tree(&ks);
        match &ts[0] {
            Tree::Group(Delim::Paren, _, s) => {
                assert_eq!(s.start, 0);
                assert_eq!(s.end, 3);
            }
            other => panic!("{other:?}"),
        }
    }

    // -- stream shape / recovery ---------------------------------------------------------------

    #[test]
    fn never_panics_on_arbitrary_fragments() {
        // Every unterminated / half-written construct must degrade into diagnostics, not a panic.
        let fragments = [
            "/*", "\"", "r#", "r##\"", "b\"", "c\"", "'", "'\\", "\\", "@", "@py(", "@py{",
            "@forth", "..", "<<=", "\u{200b}", "\u{feff}", "漢", "$", "#![", "](", "0x", "0b_",
            "1.", "1._", "1e", "1e_", "123abc", "1__0", "\"\\", "\"\\u{", "\"\\x", "\'", "r\"",
            "///", "//infy:build", "\u{a0}", "`", "\u{202e}/*",
        ];
        for a in fragments {
            for b in fragments {
                let src = format!("{a}{b}\n");
                let (lx, _c) = lex_raw(&src);
                assert!(lx.toks.last().map(|t| t.is_eof()).unwrap_or(false), "{src:?}");
                let (ts, n) = build_token_tree(&lx.toks);
                assert!(n <= lx.toks.len(), "{src:?}: {n} > {}", lx.toks.len());
                let _ = ts;
            }
        }
    }

    #[test]
    fn hash_bang_without_bracket_is_not_an_attribute() {
        // only `#![` opens an inner attribute; a shebang elsewhere is `#` + `!`
        let ks = kinds("fn f() {}\n#!/bin/sh\n");
        assert_eq!(ks[6], pu(Punct::Hash));
        assert_eq!(ks[7], pu(Punct::Bang));
    }

    #[test]
    fn crlf_at_eof_still_warns_about_both() {
        let (_lx, c) = lex_raw("package a\r\n");
        assert!(c.iter().any(|x| x.as_str() == "W-0001"), "{c:?}");
        assert!(!c.iter().any(|x| x.as_str() == "W-0002"), "{c:?}");
    }

    #[test]
    fn stream_always_ends_with_eof_and_recovers_after_errors() {
        let lx = lex("fn f( { }\n").0;
        assert!(lx.toks.last().map(|t| t.is_eof()).unwrap_or(false));
        assert!(lx.toks.len() > 4, "error recovery must keep lexing: {} toks", lx.toks.len());
        assert_eq!(lx.diagnostics_emitted, lex("fn f( { }\n").1.len());
    }

    #[test]
    fn spans_and_text_reproduce_the_source() {
        let text = "let x = 123u32;\n";
        let (lx, _c) = lex(text);
        for t in &lx.toks {
            let lo = t.span.start as usize;
            let hi = (t.span.end as usize).min(text.len());
            assert_eq!(&text[lo..hi], t.text, "{:?}", t.kind);
        }
        assert_eq!(lx.toks[3].text, "123u32");
        assert_eq!(lx.toks[3].span.start, 8);
        assert_eq!(lx.toks[3].span.end, 14);
    }

    #[test]
    fn pathological_input_terminates_and_is_capped() {
        // 200 unterminated strings: the per-code cap must bound the diagnostic count.
        let src = "\"abc\n".repeat(200);
        let (_lx, c) = lex_raw(&src);
        assert!(
            c.iter().filter(|x| x.as_str() == "E-0009").count() <= MAX_PER_CODE,
            "cap breached: {}",
            c.iter().filter(|x| x.as_str() == "E-0009").count()
        );
        assert!(_lx.diagnostics_emitted <= MAX_TOTAL);
        assert!(_lx.toks.last().map(|t| t.is_eof()).unwrap_or(false));
    }

    // -- §14 tour: every snippet must lex with zero diagnostics ---------------------------------

    const TOUR_14_2: &str = r##"
#![infy_version = "2.0"]

package build

import "github.com/acme/ci/core/test/report" as report;
import "std/fs" as fs;
import "std/fmt" as fmt;

#[derive(Debug, Clone, Copy)]
pub enum Phase { Fetch, Compile, Link, Publish }

#[derive(Debug)]
pub struct Job {
    pub id: u32,
    pub phase: Phase,
    pub src: String,
    pub out: Option<Box<Artifact>>,        // 无 null；未产出即 None
}

#[derive(Debug)]
pub struct Fail { pub job: u32, pub why: FailWhy }

pub trait Run {
    fn run(&mut self) -> Result<u32, Fail>;
    fn name(&self) -> &str;
}

impl Run for Job {
    fn run(&mut self) -> Result<u32, Fail> {
        let bytes = self.pull()?;                     // §4.11：From<Fail> for Fail 已具备
        self.phase = Phase.Compile;
        let n = self.compile(&bytes)?;                 // &bytes 借用止于本语句
        Ok(n)
    }
    fn name(&self) -> &str { "build" }
}

fn pull_from(j: &mut Job) -> Result<Vec<u8>, Fail> {
    let p = j.out.as_mut().ok_or(Fail { job: j.id, why: FailWhy::Missing })?;
    // infy-safety: 无 unsafe；此处仅借用 p 的一个字段，作用域内有效
    Ok(p.slab.take(&mut j.src))                       // 显式移动，之后 j.src 失效
}

fn describe(j: &Job) -> String {
    match (j.phase) {                                  // §3.12：臂以逗号分隔
        Phase::Fetch   => fmt!("fetch #{}", j.id),
        Phase::Compile => fmt!("compile #{}", j.id),
        Phase::Link | Phase::Publish => fmt!("late #{}", j.id),  // 或模式，绑定集相同
    }
}
"##;

    const TOUR_14_2_NEG: &str = r##"
fn bad(j: Job) -> String {
    let a = describe(&j);
    let b = describe(&j);
    drop(j);
    describe(&j)                                       // E-1001：使用已移出的绑定
}
fn bad2(x: u32, y: f64) -> u32 { x + y }               // E-0111：隐式浮点/整数互转
fn bad3(v: Vec<u8>) -> &u8 { &v[0] }                   // E-1004：返回局部引用
}
"##;

    const TOUR_14_3: &str = r##"
#![entry_chain = "Pipeline"]

package runner

import "github.com/acme/ci/core/build" as build;
import "std/chain" as chain;
import "std/sync" as sync;

chain Pipeline {
    label begin:
    spawn_jobs();                                       // 节点 0（宿主语句）
    harvest();                                          // 节点 1
    report::emit();                                      // 节点 2

    gate();                                              // 节点 3
    {                                                    // §8.1：连携块挂在宿主语句之后
        if (build::all_green()) { goto("done"); }        // 普通语句，编译期解析锚点
        goto(+1);
    }
    retry_once();                                        // 节点 4
    { goto("begin"); }                                   // 节点 5 的连携
    label done:
    finalize();                                          // 节点 6
}

fn gate() -> Result<(), build::Fail> {
    let _g = chain::lock("runner:exclusive");            // Guard drop 即释放
    Ok(())
}
"##;

    const TOUR_14_4: &str = r##"
#[link(name = "decoder", static = true, flags = ["-O2"], toolchain = "cc")]
extern "C" {
    #[borrows('static)]
    fn dec_open(path: *const u8, len: u64, err: *mut i32) -> *mut DecHandle;
    fn dec_read(h: &mut DecHandle, buf: &mut [u8]) -> usize;
    fn dec_close(h: *mut DecHandle);
}

@c(name = "decoder") {
    #include <stdint.h>
    #include "infy_binding.h"          /* 编译器生成，§7.6.3 项 1 */
    DecHandle* dec_open(const uint8_t* p, uint64_t n, int32_t* err) { /* … */ }
    /* … */
}

fn open(path: &str) -> Result<Box<DecHandle>, build::Fail> {
    let mut e: i32 = 0;
    let raw = unsafe { dec_open(path.as_ptr(), path.len() as u64, &mut e) };
    // infy-safety: dec_open 当且仅当返回非空时不写 err（见 decoder.c 头部约定）
    if raw.is_null() { return Err(build::Fail { job: 0, why: build::FailWhy::Native(e) }); }
    Ok(unsafe { Box::from_raw_safe(raw) })
}
"##;

    const TOUR_14_5: &str = r##"
fn normalize(batch: &[f32], scale: f32) -> Result<Vec<f32>, build::Fail> {
    @py(timeout_ms = 5_000, batch = batch, scale = scale) {
        # 构建期：本块写入 <build>/b_track/normalize.py
        # 由清单声明的 CPython 3.13 compile() 产出 code object，再 lowering 为 IFBC
        if scale == 0.0:
            raise ZeroDivisionError("scale")
        out = [ (x / scale) for x in batch ]
        return out
    }
}
"##;

    const TOUR_14_6: &str = r##"
fanout(n: u32) {
    let i = 0;
    while (i < n) {
        start(chain = Worker);                 // 每调用创建一个新焦点（§8.5）
        i += 1;
    }
    { resume; }
}

chain Worker {
    claim();                                   // 内部用 std.chain.lock 争抢资源
    build::run_one();
    release();
    { goto(+1); }                              // 无更多任务 → 焦点自然结束
}
"##;

    const TOUR_14_7: &str = r##"
package build_test

import "github.com/acme/ci/core/build" as build;
import "std/test" as test;

#[test]
fn describe_covers_all_phases() -> Result<(), test::Fail> {
    let j = build::Job { id: 1, phase: build::Phase::Fetch, src: String::new(), out: None };
    test::assert_eq(build::describe(&j), "fetch #1")?;
    Ok(())
}

#[bench]
fn bench_open(b: &mut test::Bench) {
    for _ in b.iter() { let _h = build::open("x.bin"); }
}
"##;

    #[test]
    fn tour_snippets_lex_with_zero_errors() {
        for (name, snippet) in [
            ("14.2", TOUR_14_2),
            ("14.2 counter-examples", TOUR_14_2_NEG),
            ("14.3", TOUR_14_3),
            ("14.4", TOUR_14_4),
            ("14.5", TOUR_14_5),
            ("14.6", TOUR_14_6),
            ("14.7", TOUR_14_7),
        ] {
            let (_lx, c) = lex(snippet);
            assert!(c.is_empty(), "{name} must lex clean, got {c:?}");
        }
    }

    #[test]
    fn tour_embed_blocks_are_captured_opaquely() {
        let lx = clean(TOUR_14_4);
        let langs: Vec<String> = lx
            .toks
            .iter()
            .filter_map(|t| match &t.kind {
                Kind::EmbedLang(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(langs, vec!["c".to_string()]);
        let body = lx
            .toks
            .iter()
            .find_map(|t| match &t.kind {
                Kind::EmbedBody(b) => Some(b.clone()),
                _ => None,
            })
            .expect("an embed body");
        assert!(body.contains("#include <stdint.h>"), "{body}");
        assert!(body.contains("\"infy_binding.h\""));
        assert!(body.contains("/* … */"));
        // the body's own balanced `{ … }` did not close the block early
        assert!(body.contains("dec_open(const uint8_t*"));
    }

    #[test]
    fn tour_py_embed_body_keeps_python_indentation_and_text() {
        let lx = clean(TOUR_14_5);
        let args = lx
            .toks
            .iter()
            .find_map(|t| match &t.kind {
                Kind::EmbedArgs(a) => Some(a.clone()),
                _ => None,
            })
            .expect("embed args");
        assert_eq!(args, "timeout_ms = 5_000, batch = batch, scale = scale");
        let body = lx
            .toks
            .iter()
            .find_map(|t| match &t.kind {
                Kind::EmbedBody(b) => Some(b.clone()),
                _ => None,
            })
            .expect("embed body");
        assert!(body.contains("raise ZeroDivisionError(\"scale\")"));
        assert!(body.contains("out = [ (x / scale) for x in batch ]"));
    }

    #[test]
    fn tour_token_shapes() {
        let ks = kinds(TOUR_14_2);
        assert_eq!(ks[0], Kind::AttrOpen { inner: true });
        // `#![infy_version = "2.0"]` is AttrOpen(inner) + the attribute's own token run
        assert_eq!(ks[1], id("infy_version"));
        assert_eq!(ks[2], pu(Punct::Eq));
        assert!(matches!(ks[3], Kind::Lit(Lit::Str(ref s)) if s == "2.0"));
        assert_eq!(ks[4], pu(Punct::RBracket));
        let pkg = ks
            .iter()
            .position(|k| *k == kw(Kw::Package))
            .expect("`package` keyword");
        assert_eq!(ks[pkg + 1], id("build"));
        // `fmt!(..)` is ident + bang + args, never a keyword
        let bang_after_fmt = ks
            .windows(2)
            .any(|w| w[0] == id("fmt") && w[1] == pu(Punct::Bang));
        assert!(bang_after_fmt, "`fmt!` must lex as Ident + Bang");
        let lx = clean(TOUR_14_2);
        assert_eq!(
            lx.toks.iter().filter(|t| t.kind == Kind::AttrOpen { inner: false }).count(),
            3
        );
    }

    #[test]
    fn tour_borrows_static_lifetime_and_attributes() {
        let ks = kinds(TOUR_14_4);
        let lt = ks.iter().find(|k| matches!(k, Kind::Lifetime(_))).expect("'static");
        assert_eq!(*lt, Kind::Lifetime("static".to_string()));
        assert!(ks.iter().any(|k| *k == kw(Kw::Unsafe)));
        assert!(ks.iter().any(|k| *k == kw(Kw::Extern)));
        assert!(ks.iter().any(|k| *k == kw(Kw::Borrows)));
        assert!(ks.iter().any(|k| *k == id("usize")));
    }

    // -- API surface the parallel parser depends on --------------------------------------------

    #[test]
    fn lexed_public_fields_are_populated() {
        let lx = clean("#!/usr/bin/env infy\n//infy:build a_track\nfn f() {}\n");
        assert_eq!(lx.shebang.as_deref(), Some("#!/usr/bin/env infy"));
        let (raw, line, span) = lx.build_constraint.clone().unwrap();
        assert_eq!(raw, "a_track");
        assert_eq!(line, 2);
        assert_eq!(span.file, 0);
        assert_eq!(lx.diagnostics_emitted, 0);
    }

    #[test]
    fn missing_file_yields_empty_lexed_not_a_panic() {
        let src = Sources::default();
        let mut h = Handler::default();
        let lx = lex_file(&src, 42, &mut h);
        assert!(lx.toks.is_empty());
        assert!(h.diags.is_empty());
    }
}
