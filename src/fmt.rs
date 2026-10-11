//! §9.5 `infy fmt` — the official formatter. The style is **byte-level unique** for a given AST
//! (UB-014): the 14 numbered rules of §9.5 are the specification, this module is the implementation,
//! and no rule is configurable.
//!
//! ## Pipeline
//!
//! [`format_file`] normalises the file bytes (§9.5 rule 11), lexes and parses it with the very same
//! stage [1] the compiler uses (`crate::lexer::lex_file` + `crate::parser::parse`), prints the AST
//! through a Wadler-style group engine ([`Doc`]), then runs the two self-checks the spec mandates:
//!
//! * rule 12 — the output is formatted a **second** time; if the two prints differ, the formatter is
//!   not a fixpoint → `E-0091` and nothing is written;
//! * rule 13 — the AST is fingerprinted before and after ([`ast_fingerprint`], comment-position
//!   metadata excluded) and the two fingerprints are compared; a difference is `E-0092` and the
//!   original file is left untouched.
//!
//! ## Layout engine
//!
//! Every decision is a function of (AST, column budget) only — never of the input's line breaks —
//! which is what makes rule 4's *second half* ("未超限时禁止逐项一行", collapse back to one line)
//! hold. Input line breaks only ever influence blank lines between statements (rule 6), which are
//! folded to at most one and are therefore stable under a second pass.
//!
//! ## Comment model (§2.11 · rules 8/9 — a comment may never be deleted)
//!
//! The lexer's [`crate::token::Comment`] list is attached to AST nodes **by source offset**: a
//! comment belongs to the gap that precedes the next node whose code starts after it; if it shares
//! its line with the preceding node it becomes that node's trailing comment instead. Emission walks
//! the tree in source order, so the same comment always lands in the same gap, and the attachment is
//! reproduced verbatim when the output is re-parsed — that is what keeps the pass idempotent.
//! Doc comments are printed from the comment list, never from `ast::Docs.lines`, because the parser
//! trims them while rule 8 forbids touching their content.
//!
//! ## Documented deviations / spec ambiguities (see also the report in the module tests)
//!
//! 1. **rule 6 vs §3.1 ordering.** §3.1 puts `package` *before* `import`s; rule 6 says "`package`
//!    前 0 个" *and* "`import` 块与 `package` 之间 1 个". Both cannot be about the same pair in the
//!    spec's own order, so rule 6 is read as "`package` 前 0 个空行" (nothing blank above `package`)
//!    and "exactly 1 blank line between `package` and the `import` block", which satisfies the
//!    sentence under either reading of which side "之间" names.
//! 2. **rule 1 hard limit.** A construct with no break opportunity (a long string literal, an
//!    identifier) may exceed 120 columns; the hard limit is a *shall-break-if-breakable* bound, not
//!    a permission to rewrite tokens (rule 9).
//! 3. **`//infy:build` keeps its spelling.** Rule 8's `// ` leading-space normalisation is *not*
//!    applied to `//infy:` directive comments, because §2.2's normative examples spell them without
//!    the space and the space-free form is what `lexer::maybe_build_constraint` matches.
//! 4. **§4.7.5 vs rule 8 for `// infy-safety:`.** §4.7.5 line 654 demands the comment *follow* the
//!    `unsafe` block, rule 8 demands the same line or the line *above*. Rule 8 is enforced for
//!    comments that are adjacent in either sense (so the spec's own §14.4 example is canonical);
//!    a safety comment that is adjacent to no `unsafe` block at all gets `W-1030`.
//! 5. **no diagnostic code exists for a bad `.infyfmt`** (only `E-0093`, which §9.6 reserves for
//!    `[lints]`). UB-024 forbids inventing one, so [`InfyfmtConfig::parse`] and
//!    [`FmtOptions::validate`] return the message instead of emitting it.
//! 6. **rule 7's host prefix** needs the module path, which is not reachable from a [`Sources`]
//!    entry; [`Printer::host_prefix_for`] falls back to the longest prefix shared by at least two
//!    non-`std` imports (deterministic, and exact when the caller supplies the prefix through
//!    [`format_file_at_host`]).

#![allow(clippy::needless_return)]

use crate::ast::*;
use crate::diag::{Diagnostic, Handler, Phase};
use crate::lexer::{self, Lexed};
use crate::parser;
use crate::span::{FileId, SourceFile, Sources, Span};
use crate::token::{Comment, CommentKind, Delim, Kind, Punct, Tok, Tree, Kw};

// --------------------------------------------------------------------------------------------
// §9.5 rule 1 + rule 14: options. There is no other configuration surface.
// --------------------------------------------------------------------------------------------

/// The only three line widths §9.5 rule 14 admits; §12.5 keeps a golden set per band.
pub const LEGAL_WIDTHS: [u32; 3] = [80, 100, 120];
/// §9.5 rule 1: the default soft limit.
pub const SOFT_WIDTH: u32 = 100;
/// §9.5 rule 1: the hard limit.
pub const HARD_WIDTH: u32 = 120;
/// §9.5 rule 2: 4 spaces, never a tab.
pub const INDENT: usize = 4;
/// §9.5 rule 3: continuation indent of a broken call chain.
pub const CHAIN_CONT: usize = 8;

#[derive(Clone, Debug)]
pub struct FmtOptions {
    /// 80 | 100 | 120 — anything else is a configuration error, never a silent default.
    pub max_width: u32,
    /// `-w`: rewrite the file in place (the CLI owns the write, [`format_file`] only returns bytes).
    pub in_place: bool,
    /// `--check`: report canonicality instead of writing (§13.4).
    pub check: bool,
    /// `.infyfmt` `exclude = [...]` globs (§9.5 rule 14).
    pub exclude: Vec<String>,
}

impl Default for FmtOptions {
    fn default() -> FmtOptions {
        FmtOptions {
            max_width: SOFT_WIDTH,
            in_place: false,
            check: false,
            exclude: Vec::new(),
        }
    }
}

impl FmtOptions {
    /// The printed soft budget in columns.
    pub fn width(&self) -> usize {
        self.max_width as usize
    }

    /// §9.5 rule 14: `max_width` must be one of the three bands. No fallback is applied: an illegal
    /// value is reported to the caller (and see the module header for why no code is emitted).
    pub fn validate(&self) -> Result<(), String> {
        if LEGAL_WIDTHS.contains(&self.max_width) {
            Ok(())
        } else {
            Err(format!(
                "`infy fmt` 行宽配置非法：`max_width = {}`，§9.5 第 14 条只允许 80/100/120 三档",
                self.max_width
            ))
        }
    }

    pub fn with_width(w: u32) -> FmtOptions {
        FmtOptions { max_width: w, ..FmtOptions::default() }
    }

    /// §9.5 rule 11/14: only `.infy` files are handled, and only when no exclude glob matches.
    pub fn skips(&self, display_path: &str) -> bool {
        !is_infy_path(display_path) || self.exclude.iter().any(|g| glob_matches(g, display_path))
    }
}

/// `fmt --check` result (§13.4 `infy fmt [-w] [--check]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FmtStatus {
    /// Bytes already canonical, including rule 5's trailing-comma contract.
    Canonical,
    /// Would change; `--check` reports this and exits non-zero.
    NeedsFormatting,
    /// `.infyfmt` `exclude` hit (§9.5 rule 14).
    Excluded,
    /// Not a `.infy` file (§9.5 rule 11).
    NotInfy,
    /// `E-0090` / `E-0091` / `E-0092` was emitted; nothing may be written.
    Failed,
}

impl FmtStatus {
    pub fn is_clean(self) -> bool {
        matches!(self, FmtStatus::Canonical | FmtStatus::Excluded | FmtStatus::NotInfy)
    }
    pub fn label(self) -> &'static str {
        match self {
            FmtStatus::Canonical => "canonical",
            FmtStatus::NeedsFormatting => "needs-formatting",
            FmtStatus::Excluded => "excluded",
            FmtStatus::NotInfy => "not-infy",
            FmtStatus::Failed => "failed",
        }
    }
}

// --------------------------------------------------------------------------------------------
// .infyfmt (§9.5 rule 14): `exclude` and `max_width`, nothing else.
// --------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InfyfmtConfig {
    pub exclude: Vec<String>,
    pub max_width: Option<u32>,
}

impl InfyfmtConfig {
    /// Parse `.infyfmt`. The file admits exactly two keys; every other key, every other value form
    /// and every other style knob is a configuration error (§9.5 rule 14: "不提供风格配置项").
    pub fn parse(text: &str) -> Result<InfyfmtConfig, Vec<String>> {
        let mut errs = Vec::new();
        let mut out = InfyfmtConfig::default();
        for raw in logical_lines(text) {
            let (key, value) = match raw.split_once('=') {
                Some(kv) => kv,
                None => {
                    errs.push(format!(".infyfmt：无法解析的行 `{raw}`（只允许 `key = value`）"));
                    continue;
                }
            };
            let key = key.trim();
            let value = value.trim();
            match key {
                "exclude" => {
                    let items = parse_string_array(value)
                        .unwrap_or_else(|| {
                            errs.push(format!(
                                ".infyfmt：`exclude` 必须是字符串数组，实际为 `{value}`"
                            ));
                            Vec::new()
                        });
                    out.exclude = items;
                }
                "max_width" => {
                    let n = value.parse::<u32>();
                    match n {
                        Ok(n) if LEGAL_WIDTHS.contains(&n) => out.max_width = Some(n),
                        Ok(n) => errs.push(format!(
                            ".infyfmt：`max_width = {n}`，§9.5 第 14 条只允许 80/100/120"
                        )),
                        Err(_) => errs.push(format!(
                            ".infyfmt：`max_width` 必须是 80/100/120 之一，实际为 `{value}`"
                        )),
                    }
                }
                other => errs.push(format!(
                    ".infyfmt：`{other}` 不是配置项；官方风格不可配置（§9.5 第 14 条，UB-014）"
                )),
            }
        }
        if errs.is_empty() {
            Ok(out)
        } else {
            Err(errs)
        }
    }

    pub fn apply_to(&self, opts: &mut FmtOptions) {
        if let Some(w) = self.max_width {
            opts.max_width = w;
        }
        if !self.exclude.is_empty() {
            opts.exclude = self.exclude.clone();
        }
    }
}

/// Split a TOML-ish file into `key = value` logical lines, honouring `{}`/`[]` continuations and
/// skipping comments. Deliberately tiny: `.infyfmt` has two keys (§9.5 rule 14), so the general
/// TOML reader in `crate::toml` is not needed here and must not be depended on while it is edited.
fn logical_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut depth = 0i32;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('#') && depth == 0 {
            continue;
        }
        buf.push(' ');
        buf.push_str(trimmed);
        depth += trimmed.matches('[').count() as i32 + trimmed.matches('{').count() as i32;
        depth -= trimmed.matches(']').count() as i32 + trimmed.matches('}').count() as i32;
        if depth <= 0 {
            depth = 0;
            let s = buf.trim().to_string();
            if !s.is_empty() {
                out.push(s);
            }
            buf.clear();
        }
    }
    let tail = buf.trim().to_string();
    if !tail.is_empty() {
        out.push(tail);
    }
    out
}

fn parse_string_array(value: &str) -> Option<Vec<String>> {
    let v = value.trim();
    let inner = v.strip_prefix('[')?.strip_suffix(']')?;
    let mut out = Vec::new();
    for part in split_top_commas(inner) {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        let s = p
            .strip_prefix('"')
            .and_then(|x| x.strip_suffix('"'))
            .or_else(|| p.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')))?;
        out.push(s.to_string());
    }
    Some(out)
}

fn split_top_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote = None::<char>;
    let mut depth = 0i32;
    let mut prev_escape = false;
    for c in s.chars() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q && !prev_escape {
                    quote = None;
                }
                prev_escape = c == '\\' && !prev_escape;
            }
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    cur.push(c);
                    prev_escape = false;
                }
                '[' | '{' => {
                    depth += 1;
                    cur.push(c);
                }
                ']' | '}' => {
                    depth -= 1;
                    cur.push(c);
                }
                ',' if depth == 0 => out.push(std::mem::take(&mut cur)),
                _ => cur.push(c),
            },
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

// --------------------------------------------------------------------------------------------
// §9.5 rule 1: columns are grapheme clusters, East-Asian Wide/Fullwidth count as 2.
// Tables per crate::VERSION.unicode_data ("Unicode 16.0.0"), std-only, no crates.
// --------------------------------------------------------------------------------------------

/// `EastAsianWidth.txt` W ∪ F for the ranges the spec names (§9.5 rule 1). Ambiguous (A) counts
/// as 1, which is the terminal convention the spec's `EastAsianWidth` reference implies.
pub fn is_wide_or_fullwidth(c: char) -> bool {
    let x = c as u32;
    matches!(
        x,
        // Hangul Jamo init. consonants
        0x1100..=0x115F
        // CJK punctuation (U+3000..U+303E) + Hangul Compatibility Jamo + Kanbun + Bopomofo
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        // CJK Unified Ideographs Extension A + Ideographs (rule 1 names U+3400..U+4DBF, U+4E00..U+9FFF)
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        // Yijing hexagram symbols
        | 0xA000..=0xA4CF
        // Hangul Jamo extended-A
        | 0xA960..=0xA97F
        // Hangul syllables
        | 0xAC00..=0xD7A3
        // CJK compatibility ideographs
        | 0xF900..=0xFAFF
        // Vertical forms
        | 0xFE10..=0xFE19
        // CJK compatibility forms
        | 0xFE30..=0xFE6F
        // Fullwidth forms (U+FF00..U+FF60) + halfwidth katakana excluded
        | 0xFF00..=0xFF60
        // Fullwidth signs
        | 0xFFE0..=0xFFE6
        // CJK Unified Ideographs Extension B.. and beyond
        | 0x20000..=0x2FFFD
        | 0x30000..=0x3FFFD
        // Emoji (EastAsianWidth W since Unicode 9) + pictographs
        | 0x1F300..=0x1F64F
        | 0x1F680..=0x1F6FF
        | 0x1F900..=0x1F9FF
        | 0x1FA70..=0x1FAFF
    ) || matches!(x, 0x2329 | 0x232A)
}

/// Zero-width: combining marks, joiners, variation selectors, controls.
pub fn is_zero_width(c: char) -> bool {
    let x = c as u32;
    if x == 0x200D || x == 0xFEFF {
        return true;
    }
    if (c as u32) < 0x20 || (x == 0x7F) || (0x80..=0x9F).contains(&x) {
        return true;
    }
    matches!(
        x,
        0x0300..=0x036F
            | 0x0483..=0x0489
            | 0x0591..=0x05BD
            | 0x0610..=0x061A
            | 0x064B..=0x065F
            | 0x0670
            | 0x06D6..=0x06DC
            | 0x06DF..=0x06E4
            | 0x06E7..=0x06E8
            | 0x06EA..=0x06ED
            | 0x0711
            | 0x0730..=0x074A
            | 0x07A6..=0x07B0
            | 0x0816..=0x0819
            | 0x08D3..=0x08E1
            | 0x08E3..=0x0902
            | 0x093A
            | 0x093C
            | 0x0941..=0x0948
            | 0x0951..=0x0957
            | 0x0962..=0x0963
            | 0x0A01..=0x0A02
            | 0x0A3C
            | 0x0A41..=0x0A42
            | 0x0A47..=0x0A48
            | 0x0A70..=0x0A71
            | 0x0A81..=0x0A82
            | 0x0ABC
            | 0x0AC1..=0x0AC5
            | 0x0AC7..=0x0AC8
            | 0x0B01
            | 0x0B3C
            | 0x0B3F..=0x0B44
            | 0x0B55..=0x0B56
            | 0x0BC0
            | 0x0C3E..=0x0C40
            | 0x0CBC
            | 0x0CBF
            | 0x0CC6
            | 0x0CCC..=0x0CCD
            | 0x0D41..=0x0D44
            | 0x0D62..=0x0D63
            | 0x0DCA
            | 0x0DD4..=0x0DD6
            | 0x0E31
            | 0x0E34..=0x0E3A
            | 0x0E47..=0x0E4E
            | 0x0EB1
            | 0x0EB4..=0x0EBC
            | 0x0EC8..=0x0ECE
            | 0x0F18..=0x0F19
            | 0x0F35
            | 0x0F37
            | 0x0F39
            | 0x0F71..=0x0F7E
            | 0x0F80..=0x0F84
            | 0x0F86..=0x0F87
            | 0x0F8D..=0x0FBC
            | 0x102B..=0x102E
            | 0x103D..=0x1048
            | 0x135D..=0x135F
            | 0x1712..=0x1714
            | 0x1732..=0x1733
            | 0x1752..=0x1753
            | 0x1772..=0x1773
            | 0x17B4..=0x17D3
            | 0x17DD
            | 0x180B..=0x180D
            | 0x1885..=0x1886
            | 0x18A9
            | 0x1920..=0x1922
            | 0x1A17..=0x1A18
            | 0x1AB0..=0x1AFF
            | 0x1B00..=0x1B03
            | 0x1DC0..=0x1DFF
            | 0x20D0..=0x20F0
            | 0x2CEF..=0x2CF1
            | 0x2DE0..=0x2DFF
            | 0x302A..=0x302D
            | 0x3099..=0x309A
            | 0xA66B..=0xA672
            | 0xA674..=0xA67D
            | 0xA69E..=0xA69F
            | 0xA6F0..=0xA6F1
            | 0xA802
            | 0xA806
            | 0xA80B
            | 0xA825..=0xA826
            | 0xA8C4..=0xA8C5
            | 0xA8E0..=0xA8F1
            | 0xA926..=0xA92D
            | 0xA947..=0xA951
            | 0xA980..=0xA982
            | 0xA9B3
            | 0xA9B6..=0xA9B9
            | 0xA9BC
            | 0xA9E5
            | 0xAA29..=0xAA2E
            | 0xAA31..=0xAA32
            | 0xAA35..=0xAA36
            | 0xAA43
            | 0xAA4C
            | 0xAA7C
            | 0xAAB0
            | 0xAAB2..=0xAAB4
            | 0xAAB7..=0xAAB8
            | 0xAABE..=0xAABF
            | 0xAAC1
            | 0xAAEC..=0xAAED
            | 0xAAF6
            | 0xABE5
            | 0xABE8
            | 0xABED
            | 0xFB1E
            | 0xFE00..=0xFE0F
            | 0xFE20..=0xFE2F
            | 0xE0100..=0xE01EF
            | 0xE0001..=0xE007F
    )
}

/// Skin-tone modifiers and ZWJ glue, i.e. `ExtendedPictographic`-ish code points that join a
/// cluster (§9.5 rule 1 counts a cluster as one column).
fn is_extended_pictographic(c: char) -> bool {
    let x = c as u32;
    matches!(
        x,
        0x1F004
            | 0x1F0CF
            | 0x1F18E
            | 0x1F191..=0x1F19A
            | 0x1F200..=0x1F2FF
            | 0x1F300..=0x1F5FF
            | 0x1F680..=0x1F6FF
            | 0x1F7E0..=0x1F7EB
            | 0x1F900..=0x1F9FF
            | 0x1FA70..=0x1FAFF
            | 0x2600..=0x26FF
            | 0x2700..=0x27BF
            | 0x1F3FB..=0x1F3FF
    )
}

fn is_regional_indicator(c: char) -> bool {
    (c as u32) >= 0x1F1E6 && (c as u32) <= 0x1F1FF
}

fn is_hangul_l(c: char) -> bool {
    (c as u32) >= 0x1100 && (c as u32) <= 0x115F
}
fn is_hangul_v(c: char) -> bool {
    (c as u32) >= 0x1160 && (c as u32) <= 0x11A7
}
fn is_hangul_t(c: char) -> bool {
    (c as u32) >= 0x11A8 && (c as u32) <= 0x11FF
}

/// Width of one cluster's base scalar (§9.5 rule 1).
fn base_columns(c: char) -> usize {
    if is_zero_width(c) {
        0
    } else if is_wide_or_fullwidth(c) {
        2
    } else {
        1
    }
}

/// Number of terminal columns `s` occupies: grapheme clusters (§2.10 counts in clusters), each
/// cluster's width taken from its first non-zero-width scalar.
pub fn display_width(s: &str) -> usize {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    let mut cols = 0usize;
    while i < chars.len() {
        let first = chars[i];
        if first == '\r' {
            // §2.1: a bare CR is illegal source; count it as zero rather than panicking.
            i += if chars.get(i + 1) == Some(&'\n') { 2 } else { 1 };
            continue;
        }
        if first == '\n' {
            i += 1;
            continue;
        }
        let mut w = base_columns(first);
        i += 1;
        let mut ri = usize::from(is_regional_indicator(first));
        let mut hangul = if is_hangul_l(first) {
            1
        } else if is_hangul_v(first) {
            2
        } else {
            0
        };
        loop {
            let Some(&c) = chars.get(i) else { break };
            if c == '\n' || c == '\r' {
                break;
            }
            if is_regional_indicator(c) {
                // GB12/GB13: RI pairs are a single cluster.
                if ri == 1 {
                    ri = 0;
                    i += 1;
                    continue;
                }
                break;
            }
            if ri == 0 && is_regional_indicator(first) {
                break;
            }
            if is_zero_width(c) {
                // GB9: Extend / ZWJ / variation selectors never start a cluster.
                if c == '\u{200D}' {
                    i += 1;
                    // GB11: ZWJ + Extended_Pictographic stays in the same cluster.
                    if let Some(&n) = chars.get(i) {
                        if is_extended_pictographic(n) {
                            i += 1;
                            continue;
                        }
                    }
                    continue;
                }
                i += 1;
                continue;
            }
            if hangul == 1 && (is_hangul_v(c) || is_hangul_t(c)) {
                hangul = if is_hangul_v(c) { 2 } else { 3 };
                i += 1;
                continue;
            }
            if hangul == 2 && is_hangul_t(c) {
                hangul = 3;
                i += 1;
                continue;
            }
            break;
        }
        cols += w;
    }
    cols
}

/// §9.5 rule 2: tabs are never emitted; for width arithmetic an input tab advances to the next
/// multiple of 4 (only relevant while measuring the *input*).
fn expand_tabs_for_width(s: &str, mut col: usize) -> (usize, String) {
    let mut out = String::new();
    for c in s.chars() {
        if c == '\t' {
            let pad = 4 - (col % 4);
            col += pad;
            for _ in 0..pad {
                out.push(' ');
            }
        } else {
            col += base_columns(c);
            out.push(c);
        }
    }
    (col, out)
}

// --------------------------------------------------------------------------------------------
// §9.5 rules 3/4: the group engine. Layout depends on (AST, budget) only, never on input breaks.
// --------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Flat,
    Break,
}

#[derive(Clone, Debug)]
pub enum Doc {
    Nil,
    Text(String),
    Cat(Vec<Doc>),
    /// Lines inside get `indent + n` columns (§9.5 rules 2/3).
    Nest(usize, Box<Doc>),
    /// Flat: one space. Break: newline + indent.
    Line,
    /// Flat: nothing. Break: newline + indent.
    SoftLine,
    /// Always a newline — also breaks every enclosing group.
    HardLine,
    /// A folded-away blank line (§9.5 rule 6): one extra `\n`, no indentation written.
    Blank,
    /// `try flat first, else break`; the `bool` caches "contains a forced line break".
    Group(Box<Doc>, bool),
    /// Different text depending on the enclosing group's mode (§9.5 rule 5's trailing comma).
    IfBreak(String, String),
    /// Zero-width position marker, used for the §9.5 rule 8 `infy-safety` adjacency check.
    Mark(u32),
}

pub fn text(s: &str) -> Doc {
    Doc::Text(s.to_string())
}
pub fn cat(v: Vec<Doc>) -> Doc {
    Doc::Cat(v)
}
pub fn nest(n: usize, d: Doc) -> Doc {
    Doc::Nest(n, Box::new(d))
}
pub fn group(d: Doc) -> Doc {
    let hard = contains_hard(&d);
    Doc::Group(Box::new(d), hard)
}

fn contains_hard(d: &Doc) -> bool {
    match d {
        Doc::Nil | Doc::Mark(_) | Doc::Line | Doc::SoftLine => false,
        Doc::Text(t) => t.contains('\n'),
        Doc::HardLine | Doc::Blank => true,
        Doc::Cat(v) => v.iter().any(contains_hard),
        Doc::Nest(_, b) => contains_hard(b),
        Doc::Group(b, hard) => *hard || contains_hard(b),
        Doc::IfBreak(a, b) => a.contains('\n') || b.contains('\n'),
    }
}

fn fits(mut w: i64, doc: &Doc, stack: &[(usize, Mode, &Doc)]) -> bool {
    if w < 0 {
        return false;
    }
    let mut queue: std::collections::VecDeque<(usize, Mode, &Doc)> = std::collections::VecDeque::new();
    queue.push_back((0, Mode::Flat, doc));
    // The stack is LIFO: printing order is the reverse of its Vec order.
    for e in stack.iter().rev() {
        queue.push_back(*e);
    }
    while let Some((ind, mode, d)) = queue.pop_front() {
        let _ = ind;
        match d {
            Doc::Nil | Doc::Mark(_) => {}
            Doc::Text(t) => w -= display_width(t) as i64,
            Doc::Cat(v) => {
                for x in v.iter().rev() {
                    queue.push_front((0, mode, x));
                }
            }
            Doc::Nest(_, b) => queue.push_front((0, mode, b)),
            Doc::Line => {
                if mode == Mode::Break {
                    return true;
                }
                w -= 1;
            }
            Doc::SoftLine => {
                if mode == Mode::Break {
                    return true;
                }
            }
            Doc::HardLine | Doc::Blank => return true,
            Doc::IfBreak(_, f) => {
                if mode == Mode::Break {
                    return true;
                }
                w -= display_width(f) as i64;
            }
            Doc::Group(b, hard) => {
                let m = if *hard { Mode::Break } else { Mode::Flat };
                queue.push_front((0, m, b));
            }
        }
        if w < 0 {
            return false;
        }
    }
    true
}

/// Render `doc` into at most `width` columns. Returns the text and the recorded mark offsets.
pub fn render(doc: &Doc, width: usize) -> (String, Vec<(u32, usize)>) {
    let mut out = String::new();
    let mut marks: Vec<(u32, usize)> = Vec::new();
    let mut col = 0usize;
    let mut stack: Vec<(usize, Mode, &Doc)> = vec![(0, Mode::Break, doc)];
    while let Some((ind, mode, d)) = stack.pop() {
        match d {
            Doc::Nil => {}
            Doc::Mark(id) => marks.push((*id, out.len())),
            Doc::Text(t) => {
                if t.contains('\n') {
                    let last = t.rsplit('\n').next().unwrap_or("");
                    col = col + display_width(last);
                } else {
                    col += display_width(t);
                }
                out.push_str(t);
            }
            Doc::Cat(v) => {
                for x in v.iter().rev() {
                    stack.push((ind, mode, x));
                }
            }
            Doc::Nest(k, b) => stack.push((ind + k, mode, b)),
            Doc::Line => {
                if mode == Mode::Flat {
                    out.push(' ');
                    col += 1;
                } else {
                    newline(&mut out, &mut col, ind);
                }
            }
            Doc::SoftLine => {
                if mode == Mode::Break {
                    newline(&mut out, &mut col, ind);
                }
            }
            Doc::HardLine => newline(&mut out, &mut col, ind),
            Doc::Blank => {
                out.push('\n');
                col = 0;
            }
            Doc::IfBreak(b, f) => {
                let pick = if mode == Mode::Break { b } else { f };
                if !pick.is_empty() {
                    col += display_width(pick);
                    out.push_str(pick);
                }
            }
            Doc::Group(b, hard) => {
                if mode == Mode::Flat {
                    stack.push((ind, Mode::Flat, b));
                } else if *hard {
                    stack.push((ind, Mode::Break, b));
                } else {
                    let budget = width as i64 - col as i64;
                    let m = if fits(budget, b, &stack) { Mode::Flat } else { Mode::Break };
                    stack.push((ind, m, b));
                }
            }
        }
    }
    (out, marks)
}

fn newline(out: &mut String, col: &mut usize, indent: usize) {
    out.push('\n');
    for _ in 0..indent {
        out.push(' ');
    }
    *col = indent;
}

/// The one canonical "may break, one item per line when it does" shape:
/// `open` · items separated by `,` · optional trailing comma (`§9.5` rule 5) · `close`.
fn delimited(open: &str, items: Vec<Doc>, close: &str, indent: usize, pad: bool) -> Doc {
    if items.is_empty() {
        return text(&format!("{open}{close}"));
    }
    let pad_doc = if pad { Doc::Line } else { Doc::SoftLine };
    let sep = if pad { Doc::Line } else { Doc::SoftLine };
    let mut inner = vec![pad_doc.clone()];
    for (i, it) in items.into_iter().enumerate() {
        if i > 0 {
            inner.push(text(","));
            inner.push(sep.clone());
        }
        inner.push(it);
    }
    inner.push(Doc::IfBreak(",".to_string(), String::new()));
    let body = cat(vec![
        text(open),
        nest(indent, cat(inner)),
        pad_doc,
        text(close),
    ]);
    group(body)
}

// --------------------------------------------------------------------------------------------
// §9.5 rule 11 + rule 14: file selection, byte normalisation, entry points.
// --------------------------------------------------------------------------------------------

/// §9.5 rule 11: files that are not `.infy` are not processed.
pub fn is_infy_path(display_path: &str) -> bool {
    let p = display_path.rsplit('/').next().unwrap_or(display_path);
    p.len() > ".infy".len() && p.to_ascii_lowercase().ends_with(".infy")
}

/// `.infyfmt` globs: `**` crosses directories, `*`/`?` do not.
pub fn glob_matches(pattern: &str, path: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let s: Vec<char> = path.replace('\\', "/").chars().collect();
    glob_from(&p, 0, &s, 0)
}

fn glob_from(p: &[char], mut pi: usize, s: &[char], mut si: usize) -> bool {
    loop {
        if pi >= p.len() {
            return si >= s.len();
        }
        if p[pi] == '*' {
            if p.get(pi + 1) == Some(&'*') {
                // `**` — any number of characters, including `/`.
                let mut k = si;
                loop {
                    if glob_from(p, pi + 2, s, k) {
                        return true;
                    }
                    if k >= s.len() {
                        return false;
                    }
                    k += 1;
                }
            }
            // `*` — up to the next `/`.
            let mut k = si;
            loop {
                if glob_from(p, pi + 1, s, k) {
                    return true;
                }
                if k >= s.len() || s[k] == '/' {
                    return false;
                }
                k += 1;
            }
        }
        if si >= s.len() {
            return false;
        }
        if p[pi] == '?' || p[pi] == s[si] {
            pi += 1;
            si += 1;
            continue;
        }
        return false;
    }
}

/// §2.1 + §9.5 rule 11: the byte-level fixes `infy fmt` owns. Returns the normalised text plus the
/// warnings the normalisation justifies (`W-0001` CRLF, `W-0002` missing trailing LF).
pub fn normalise_bytes(text: &str) -> (String, bool, bool) {
    let mut body = text;
    let mut had_bom = false;
    if let Some(stripped) = body.strip_prefix('\u{FEFF}') {
        body = stripped;
        had_bom = true;
    }
    let crlf = body.contains("\r\n");
    let mut out = String::with_capacity(body.len());
    if crlf || had_bom {
        out.extend(body.chars().filter(|c| *c != '\r'));
        // A bare CR (§2.1 `E-0003`) is *not* rewritten: the lexer must report it.
        if body.contains('\r') {
            out = String::new();
            let bytes: Vec<char> = body.chars().collect();
            let mut i = 0usize;
            while i < bytes.len() {
                if bytes[i] == '\r' {
                    if bytes.get(i + 1) == Some(&'\n') {
                        out.push('\n');
                        i += 2;
                        continue;
                    }
                    out.push('\r');
                    i += 1;
                    continue;
                }
                out.push(bytes[i]);
                i += 1;
            }
        }
    } else {
        out.push_str(body);
    }
    let missing_tail_lf = !out.is_empty() && !out.ends_with('\n');
    if missing_tail_lf {
        out.push('\n');
    }
    (out, crlf, missing_tail_lf)
}

/// §9.5 rule 11: an empty source file is not formatable (`E-0090`).
fn is_empty_source(text: &str) -> bool {
    text.chars().all(|c| c.is_whitespace())
}

fn emit_fmt_diag(
    handler: &mut Handler,
    src: &Sources,
    file: FileId,
    span: Span,
    code: &str,
    note: &str,
) {
    let (start, _) = src.line_col(span);
    let d = Diagnostic::new(code, Phase::Fmt, file, start)
        .note(note)
        .help("这是编译器缺陷，请上报；本次不写出生成结果");
    handler.emit(d);
}

/// §9.5 rule 12 + 13 pipeline for one file: lex · parse · print · re-print · re-parse · compare.
/// `diagnose` is false for the internal fixpoint pass, whose diagnostics must not be surfaced.
fn format_pass(
    sources: &Sources,
    file: FileId,
    opts: &FmtOptions,
    handler: &mut Handler,
    diagnose: bool,
    host_prefix: &str,
) -> Option<(String, String)> {
    let src = sources.get(file)?;
    let mut lexed = lexer::lex_file(sources, file, handler);
    let parsed = parser::parse(sources, file, &lexed, handler);
    let before = ast_fingerprint(&parsed);
    let comments = std::mem::take(&mut lexed.comments);
    let mut p = Printer::new(src, &comments, opts, host_prefix);
    let output = p.print_file(&parsed);
    if diagnose {
        p.safety_warnings(sources, file, handler);
    }
    let _ = src;
    Some((output, before))
}

/// §9.5 rule 12/13 entry point.
pub fn format_file_with_host(
    sources: &Sources,
    file: FileId,
    handler: &mut Handler,
    opts: &FmtOptions,
    host_prefix: &str,
) -> Option<String> {
    let display_path = sources.get(file).map(|f| f.display_path.clone())?;
    if opts.skips(&display_path) {
        return None;
    }
    if !is_infy_path(&display_path) {
        return None;
    }
    let raw = sources.get(file).map(|f| f.text.clone())?;
    if is_empty_source(&raw) {
        // §9.5 rule 11: empty file → E-0090.
        let d = Diagnostic::new("E-0090", Phase::Fmt, file, crate::span::Position { line: 1, column: 1 })
            .note("§9.5 第 11 条：空源码文件视为格式非法")
            .help("删除该文件或写入 `package`");
        handler.emit(d);
        return None;
    }
    let (norm, crlf, no_lf) = normalise_bytes(&raw);
    if crlf {
        let d = Diagnostic::new("W-0001", Phase::Fmt, file, crate::span::Position { line: 1, column: 1 })
            .note("§2.1：CRLF 换行；`infy fmt` 已改写为 LF（§9.5 第 11 条）")
            .help("无需处理，输出即 LF");
        handler.emit(d);
    }
    if no_lf {
        let d = Diagnostic::new("W-0002", Phase::Fmt, file, crate::span::Position { line: 1, column: 1 })
            .note("§2.1：文件结尾缺少一个 trailing LF；`infy fmt` 已补齐（§9.5 第 11 条）")
            .help("无需处理");
        handler.emit(d);
    }
    let mut inner = Sources::default();
    let fid = inner.add(display_path.clone(), norm.clone());
    let (output, before_fp) = format_pass(&inner, fid, opts, handler, true, host_prefix)?;

    // §9.5 rule 13: AST invariance. Re-parse the output and compare fingerprints.
    let mut src2 = Sources::default();
    let fid2 = src2.add(display_path.clone(), output.clone());
    let mut h2 = Handler::default();
    let lexed2 = lexer::lex_file(&src2, fid2, &mut h2);
    let parsed2 = parser::parse(&src2, fid2, &lexed2, &mut h2);
    let after_fp = ast_fingerprint(&parsed2);
    if after_fp != before_fp {
        let span = sources
            .get(file)
            .map(|f| Span::new(file, 0, f.text.len() as u32))
            .unwrap_or_else(|| Span::empty_at(file, 0));
        emit_fmt_diag(
            handler,
            sources,
            file,
            span,
            "E-0092",
            "§9.5 第 13 条：格式化前后 AST 不同构，原文件保持不变",
        );
        return None;
    }

    // §9.5 rule 12: idempotence self-check — format the output a second time and compare bytes.
    let mut h3 = Handler::default();
    let lexed3 = lexer::lex_file(&src2, fid2, &mut h3);
    let parsed3 = parser::parse(&src2, fid2, &lexed3, &mut h3);
    let mut p3 = Printer::new(src2.get(fid2)?, &lexed3.comments, opts, host_prefix);
    let second = p3.print_file(&parsed3);
    if second != output {
        let span = sources
            .get(file)
            .map(|f| Span::new(file, 0, f.text.len() as u32))
            .unwrap_or_else(|| Span::empty_at(file, 0));
        emit_fmt_diag(
            handler,
            sources,
            file,
            span,
            "E-0091",
            "§9.5 第 12 条：fmt(fmt(x)) != fmt(x)，拒绝写出",
        );
        return None;
    }
    Some(output)
}

/// §9.5 deliverable entry point. The host module prefix is derived from the import set (§9.5 rule
/// 7, module header note 6).
pub fn format_file(
    sources: &Sources,
    file: FileId,
    handler: &mut Handler,
    opts: &FmtOptions,
) -> Option<String> {
    let host = match sources.get(file) {
        Some(f) => {
            let mut inner = Sources::default();
            let fid = inner.add(f.display_path.clone(), f.text.clone());
            let mut h = Handler::default();
            let lexed = lexer::lex_file(&inner, fid, &mut h);
            let parsed = parser::parse(&inner, fid, &lexed, &mut h);
            derive_host_prefix(&parsed)
        }
        None => String::new(),
    };
    format_file_with_host(sources, file, handler, opts, &host)
}

/// §13.4 `infy fmt --check`: report canonicality. Rule 5's trailing-comma contract is part of the
/// comparison — a multi-line list without its trailing comma is not canonical.
pub fn check_file(
    sources: &Sources,
    file: FileId,
    handler: &mut Handler,
    opts: &FmtOptions,
) -> FmtStatus {
    let display_path = match sources.get(file) {
        Some(f) => f.display_path.clone(),
        None => return FmtStatus::Failed,
    };
    if !is_infy_path(&display_path) {
        return FmtStatus::NotInfy;
    }
    if opts.exclude.iter().any(|g| glob_matches(g, &display_path)) {
        return FmtStatus::Excluded;
    }
    if opts.check || opts.in_place {
        // fall through: both modes need the canonical bytes
    }
    let raw = match sources.get(file) {
        Some(f) => f.text.clone(),
        None => return FmtStatus::Failed,
    };
    if is_empty_source(&raw) {
        let d = Diagnostic::new("E-0090", Phase::Fmt, file, crate::span::Position { line: 1, column: 1 })
            .note("§9.5 第 11 条：空源码文件视为格式非法")
            .help("删除该文件或写入 `package`");
        handler.emit(d);
        return FmtStatus::Failed;
    }
    let (norm, _, _) = normalise_bytes(&raw);
    let host = derive_host_prefix_from_text(&norm, &display_path);
    let before_opts = FmtOptions { check: false, in_place: false, ..opts.clone() };
    let mut inner = Handler::default();
    let out = format_file_with_host(sources, file, &mut inner, &before_opts, &host);
    for d in inner.diags {
        handler.emit(d);
    }
    match out {
        None => FmtStatus::Failed,
        Some(o) => {
            if o == norm {
                FmtStatus::Canonical
            } else {
                FmtStatus::NeedsFormatting
            }
        }
    }
}

fn derive_host_prefix_from_text(text: &str, display_path: &str) -> String {
    let mut inner = Sources::default();
    let fid = inner.add(display_path.to_string(), text.to_string());
    let mut h = Handler::default();
    let lexed = lexer::lex_file(&inner, fid, &mut h);
    let parsed = parser::parse(&inner, fid, &lexed, &mut h);
    derive_host_prefix(&parsed)
}

/// §9.5 rule 7 group 2 is "the host module's own prefix", which is manifest data. Without it the
/// deterministic fallback is the longest path prefix shared by at least two non-`std` imports.
pub fn derive_host_prefix(file: &ParsedFile) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for im in &file.imports {
        if im.path == "std" || im.path.starts_with("std/") || im.path.starts_with("std::") {
            continue;
        }
        let mut segs = String::new();
        for seg in im.path.split('/') {
            if seg.is_empty() {
                continue;
            }
            if segs.is_empty() {
                segs.push_str(seg);
            } else {
                segs.push('/');
                segs.push_str(seg);
            }
            counts.push((segs.clone(), 1));
        }
    }
    let mut best = String::new();
    let mut best_hits = 0usize;
    for (p, _) in counts.clone() {
        let hits = counts.iter().filter(|(q, _)| *q == p).count();
        if p.is_empty() {
            continue;
        }
        let seg_n = p.split('/').count();
        if hits >= 2 && (hits > best_hits || (hits == best_hits && seg_n > best.split('/').count()))
        {
            best = p;
            best_hits = hits;
        }
    }
    let _ = best_hits;
    best
}

