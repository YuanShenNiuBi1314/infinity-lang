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

// --------------------------------------------------------------------------------------------
// Comment attachment (§2.11 · §9.5 rules 8/9).
// --------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Cmt {
    start: u32,
    end: u32,
    line: u32,
    kind: CommentKind,
    /// Text after the `//`, `///` or `/*` marker, exactly as written (§9.5 rule 9).
    text: String,
    /// Nothing but whitespace precedes the comment on its source line.
    own_line: bool,
    is_directive: bool,
    is_safety: bool,
}

impl Cmt {
    /// The canonical bytes of this comment (§9.5 rule 8).
    fn render(&self) -> String {
        match self.kind {
            CommentKind::Doc => {
                // `///` content is never touched, only re-indented (rule 8).
                format!("///{}", self.text)
            }
            CommentKind::Line => {
                if self.is_directive {
                    // `//infy:build …` keeps §2.2's space-free spelling (module header note 3).
                    format!("//{}", self.text)
                } else if self.text.is_empty() {
                    String::from("//")
                } else {
                    let first = self.text.chars().next().unwrap_or(' ');
                    if first == ' ' || !first.is_alphanumeric() {
                        format!("//{}", self.text)
                    } else {
                        format!("// {}", self.text)
                    }
                }
            }
            CommentKind::Block => format!("/*{}*/", self.text),
        }
    }
}

/// Comments not yet emitted, in source order. Emission walks the tree in source order, so the
/// cursor only ever moves forward and every comment is emitted exactly once (§2.11 hard rule).
struct Pool {
    items: Vec<Cmt>,
    cursor: usize,
    /// Ranges whose bytes are reprinted verbatim (macro arms, `@embed` bodies): comments inside
    /// them are already on their way out and must not be emitted a second time.
    consumed: Vec<(u32, u32)>,
}

impl Pool {
    fn new(comments: &[Comment], src: &SourceFile) -> Pool {
        let mut items: Vec<Cmt> = Vec::with_capacity(comments.len());
        for c in comments {
            let line = src.byte_line_col(c.span.start).0;
            let own = own_line(src, c.span.start);
            let is_directive = c.kind == CommentKind::Line
                && (c.text.trim_start().starts_with("infy:") || c.text.starts_with("**infy:"));
            let is_safety = c.kind != CommentKind::Block && c.text.contains("infy-safety");
            items.push(Cmt {
                start: c.span.start,
                end: c.span.end,
                line,
                kind: c.kind,
                text: c.text.clone(),
                own_line: own,
                is_directive,
                is_safety,
            });
        }
        items.sort_by_key(|c| (c.start, c.end));
        Pool { items, cursor: 0, consumed: Vec::new() }
    }

    fn mark_consumed(&mut self, from: u32, to: u32) {
        self.consumed.push((from.min(to), from.max(to)));
    }

    fn in_consumed(&self, off: u32) -> bool {
        self.consumed.iter().any(|(a, b)| off >= *a && off <= *b)
    }

    /// The index of the next un-consumed comment, if it starts before `off`.
    fn take_before(&mut self, off: u32) -> Vec<usize> {
        let mut out = Vec::new();
        while self.cursor < self.items.len() {
            let c = &self.items[self.cursor];
            if c.start >= off {
                break;
            }
            let skip = self.in_consumed(c.start);
            if !skip {
                out.push(self.cursor);
            }
            self.cursor += 1;
        }
        out
    }

    fn take_rest(&mut self) -> Vec<usize> {
        let mut out = Vec::new();
        while self.cursor < self.items.len() {
            let c = &self.items[self.cursor];
            if !self.in_consumed(c.start) {
                out.push(self.cursor);
            }
            self.cursor += 1;
        }
        out
    }

    /// The byte offset where the next comment (that still has to be emitted) begins.
    fn next_start(&self) -> Option<u32> {
        let mut i = self.cursor;
        while i < self.items.len() && self.in_consumed(self.items[i].start) {
            i += 1;
        }
        self.items.get(i).map(|c| c.start)
    }
}

/// True when only spaces/tabs precede `off` on its line.
fn own_line(src: &SourceFile, off: u32) -> bool {
    let text = &src.text;
    let mut i = (off as usize).min(text.len());
    while i > 0 {
        let Some(c) = text.get(i - 1..i) else { break };
        match c {
            " " | "\t" => i -= 1,
            "\n" | "\r" => return true,
            _ => return false,
        }
    }
    true
}

/// A node of a list that can own comments: its code span and its rendered doc.
struct NodeSpec {
    start: u32,
    end: u32,
    doc: Doc,
}

/// What a mark id refers to, for §9.5 rule 8's `infy-safety` adjacency check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MarkKind {
    SafetyComment,
    UnsafeStart,
    UnsafeEnd,
}

// --------------------------------------------------------------------------------------------
// The printer.
// --------------------------------------------------------------------------------------------

pub struct Printer<'a> {
    src: &'a SourceFile,
    opts: &'a FmtOptions,
    pool: Pool,
    host: String,
    marks: Vec<(u32, MarkKind)>,
    next_mark: u32,
    /// Output offsets, filled by [`Printer::print_file`].
    offsets: Vec<(u32, usize)>,
    unsafe_blocks: Vec<Span>,
    comment_lines: Vec<(u32, u32, u32)>,
}

impl<'a> Printer<'a> {
    pub fn new(
        src: &'a SourceFile,
        comments: &'a [Comment],
        opts: &'a FmtOptions,
        host_prefix: &str,
    ) -> Printer<'a> {
        Printer {
            src,
            opts,
            pool: Pool::new(comments, src),
            host: host_prefix.to_string(),
            marks: Vec::new(),
            next_mark: 1,
            offsets: Vec::new(),
            unsafe_blocks: Vec::new(),
            comment_lines: Vec::new(),
        }
    }

    fn alloc_mark(&mut self, kind: MarkKind) -> u32 {
        let id = self.next_mark;
        self.next_mark += 1;
        self.marks.push((id, kind));
        id
    }

    // ------------------------------------------------------------------ source helpers

    fn span_text(&self, sp: Span) -> Option<&str> {
        if sp.file != self.src.id {
            return None;
        }
        let a = (sp.start as usize).min(self.src.text.len());
        let b = (sp.end as usize).min(self.src.text.len());
        if b < a {
            return None;
        }
        self.src.text.get(a..b)
    }

    /// §9.5 rule 9: literals, identifiers and paths are reprinted from their source bytes, never
    /// regenerated from the AST value (`1_000` must not collapse to `1000`, escapes and raw strings
    /// must survive untouched).
    fn verbatim(&self, sp: Span, fallback: &str) -> String {
        match self.span_text(sp) {
            Some(s) => {
                let t = s.trim_matches(|c: char| c == ' ' || c == '\t' || c == '\n' || c == '\r');
                if t.is_empty() { fallback.to_string() } else { t.to_string() }
            }
            None => fallback.to_string(),
        }
    }

    fn blank_between(&self, from: u32, to: u32) -> bool {
        let a = (from as usize).min(self.src.text.len());
        let b = (to as usize).min(self.src.text.len());
        if b <= a {
            return false;
        }
        let s = match self.src.text.get(a..b) {
            Some(s) => s,
            None => return false,
        };
        let mut seen_nl = false;
        let mut run = String::new();
        for c in s.chars() {
            if c == '\n' {
                if seen_nl && run.chars().all(|x| x == ' ' || x == '\t') {
                    return true;
                }
                seen_nl = true;
                run.clear();
            } else {
                run.push(c);
            }
        }
        false
    }

    fn line_of_offset(&self, off: usize) -> u32 {
        self.src.line_starts().iter().filter(|&&s| (s as usize) <= off).count() as u32
    }

    // ------------------------------------------------------------------ §9.5 rule 6: list layout

    /// What a gap between two nodes contributes to.
    ///
    /// * `Items` — exactly one blank line between every pair (§9.5 rule 6: declarations);
    /// * `Statements` — keep a source blank line, folded to one (§9.5 rule 6);
    /// * `Fields` — no blank lines (fields and variants collapse onto one line when they fit).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum ListMode {
        Items,
        Statements,
        Fields,
    }

    /// Lay out a list of nodes: leading comments above the node, a comment that shares the previous
    /// node's last line stays behind it, at most one blank line in between (§9.5 rule 6).
    ///
    /// `list_end` is the offset that closes the list; comments up to it trail the last node.
    fn list_doc(&mut self, nodes: Vec<NodeSpec>, list_end: u32, mode: ListMode) -> Doc {
        let mut out: Vec<Doc> = Vec::new();
        let mut prev_end: Option<u32> = None;
        for (i, n) in nodes.into_iter().enumerate() {
            let hole = self.pool.take_before(n.start);
            let (trail, lead, detached) = self.split_hole(&hole, prev_end, n.start);
            let blank = match mode {
                ListMode::Items => true,
                ListMode::Statements => {
                    let base = prev_end.unwrap_or(n.start);
                    let target = lead.first().map(|c| self.pool.items[*c].start).unwrap_or(n.start);
                    self.blank_between(base, target)
                }
                ListMode::Fields => false,
            };
            if i > 0 {
                for c in trail {
                    out.push(Doc::Text(" ".to_string()));
                    let idx = c;
                    self.push_comment_doc(&mut out, idx, true);
                }
                out.push(Doc::HardLine);
                if detached {
                    // The group is separated from the node below it by a blank line: emit it first,
                    // then the blank, then the node. Stable under re-parse (rule 12).
                    for (j, &idx) in lead.iter().enumerate() {
                        if j > 0 {
                            out.push(Doc::HardLine);
                        }
                        self.push_comment_doc(&mut out, idx, false);
                    }
                    if !lead.is_empty() {
                        out.push(Doc::Blank);
                    }
                    out.push(Doc::HardLine);
                } else {
                    if blank {
                        out.push(Doc::Blank);
                    }
                    out.push(Doc::HardLine);
                    for (j, &idx) in lead.iter().enumerate() {
                        if j > 0 {
                            out.push(Doc::HardLine);
                        }
                        self.push_comment_doc(&mut out, idx, false);
                    }
                    if !lead.is_empty() {
                        out.push(Doc::HardLine);
                    }
                }
            } else {
                for c in trail {
                    out.push(Doc::Text(" ".to_string()));
                    self.push_comment_doc(&mut out, c, true);
                    out.push(Doc::HardLine);
                }
                for (j, &idx) in lead.iter().enumerate() {
                    if j > 0 {
                        out.push(Doc::HardLine);
                    }
                    self.push_comment_doc(&mut out, idx, false);
                }
                if !lead.is_empty() {
                    out.push(Doc::HardLine);
                }
            }
            out.push(n.doc);
            prev_end = Some(n.end);
        }
        let tail = self.pool.take_before(list_end);
        if !tail.is_empty() {
            let first_own = tail
                .first()
                .map(|&idx| self.pool.items[idx].own_line)
                .unwrap_or(false);
            if first_own {
                for (j, &idx) in tail.iter().enumerate() {
                    if j > 0 {
                        out.push(Doc::HardLine);
                    }
                    self.push_comment_doc(&mut out, idx, false);
                }
            } else {
                for &idx in &tail {
                    if self.pool.items[idx].own_line {
                        out.push(Doc::HardLine);
                        self.push_comment_doc(&mut out, idx, false);
                    } else {
                        out.push(Doc::Text(" ".to_string()));
                        self.push_comment_doc(&mut out, idx, true);
                    }
                }
            }
        }
        Doc::Cat(out)
    }

    /// Split a gap's comments into (trailing of the previous node, leading of the next node,
    /// detached). A comment is a trailing comment only when it shares the previous node's last
    /// source line; `detached` is §9.5 rule 6's blank line between a comment group and the node it
    /// visually precedes.
    fn split_hole(
        &self,
        hole: &[usize],
        prev_end: Option<u32>,
        next_start: u32,
    ) -> (Vec<usize>, Vec<usize>, bool) {
        let mut trail: Vec<usize> = Vec::new();
        let mut lead: Vec<usize> = Vec::new();
        let prev_line = prev_end.map(|e| self.line_of_offset(e as usize));
        let mut i = 0usize;
        if let Some(pl) = prev_line {
            while i < hole.len() {
                let c = &self.pool.items[hole[i]];
                if c.line == pl && !c.own_line {
                    trail.push(hole[i]);
                    i += 1;
                } else {
                    break;
                }
            }
        }
        for &idx in &hole[i..] {
            lead.push(idx);
        }
        let detached = match lead.last() {
            Some(&c) => self.blank_between(self.pool.items[c].end, next_start),
            None => false,
        };
        (trail, lead, detached)
    }

    fn push_comment_doc(&mut self, out: &mut Vec<Doc>, idx: usize, inline: bool) {
        let c = &self.pool.items[idx];
        let rendered = c.render();
        let safety = c.is_safety;
        let start = c.start;
        if safety {
            let id = self.alloc_mark(MarkKind::SafetyComment);
            out.push(Doc::Mark(id));
            self.comment_lines.push((id, start, inline as u32));
        }
        if rendered.contains('\n') && !inline {
            // §9.5 rule 8: a multi-line block comment keeps its position and its inner bytes; the
            // lines after the first are verbatim, so they must not pick up the list's indentation.
            let parts: Vec<&str> = rendered.split('\n').collect();
            for (k, p) in parts.iter().enumerate() {
                if k > 0 {
                    out.push(Doc::Text("\n".to_string()));
                }
                out.push(Doc::Text(p.to_string()));
            }
            return;
        }
        out.push(Doc::Text(rendered));
    }

    // ------------------------------------------------------------------ §9.5 rule 7 + rule 6: the file

    /// Format a whole parsed file (§3.1 order: shebang · `//infy:build` · file attrs · `package` ·
    /// imports · items).
    pub fn print_file(&mut self, f: &ParsedFile) -> String {
        let mut v: Vec<Doc> = Vec::new();

        // shebang, then the build constraint pinned to the first content line (rules 8/11).
        if let Some(sb) = &f.shebang {
            v.push(Doc::Text(sb.trim_end_matches(['\r', '\n']).to_string()));
            v.push(Doc::HardLine);
        }
        if let Some(bc) = &f.build_constraint {
            v.push(Doc::Text(format!("//infy:build {bc}")));
            v.push(Doc::HardLine);
            // Rule 8 also deletes the blank line that used to follow it — the layout below never
            // emits one, and the comment itself must not be re-emitted from the pool.
            if let Some(sp) = f.build_constraint_span {
                self.pool.mark_consumed(sp.start, sp.end);
            }
        }

        // Everything above the first real construct, including comments that precede it.
        let first_off = [
            f.file_attrs.first().map(|a| a.span.start),
            f.package_span,
            f.imports.first().map(|i| i.span.start),
            f.items.first().map(|i| i.span.start),
        ]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(u32::MAX);
        let pre = self.pool.take_before(first_off);
        for (j, &idx) in pre.iter().enumerate() {
            if j > 0 {
                v.push(Doc::HardLine);
            }
            self.push_comment_doc(&mut v, idx, false);
        }
        if !pre.is_empty() {
            v.push(Doc::HardLine);
        }

        for a in &f.file_attrs {
            // §9.5 rule 10: every attribute sits on its own line.
            v.push(self.attr_doc(a));
            v.push(Doc::HardLine);
        }
        if let Some(p) = &f.package {
            v.push(Doc::Text(format!("package {p}")));
        }
        if !f.imports.is_empty() {
            if f.package.is_some() {
                // §9.5 rule 6: exactly one blank line between `package` and the import block.
                v.push(Doc::Blank);
            } else {
                v.push(Doc::HardLine);
            }
            let imp = self.imports_doc(&f.imports);
            v.push(imp);
        }
        let end_off = self
            .src
            .text
            .len()
            .try_into()
            .unwrap_or(u32::MAX);
        let nodes = self.item_nodes(&f.items);
        let items = self.list_doc(nodes, end_off, ListMode::Items);
        if !is_empty_doc(&items) {
            if f.package.is_some() || !f.imports.is_empty() || !f.file_attrs.is_empty() {
                // One blank line between the header block and the first declaration.
                v.push(Doc::Blank);
            }
            v.push(items);
        }
        let doc = Doc::Cat(v);
        let (mut out, offsets) = render(&doc, self.opts.width());
        self.offsets = offsets;
        while out.ends_with('\n') && out.len() > 1 {
            out.pop();
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        // §9.5 rule 2: the formatter itself never emits a tab.
        self.strip_leading_tabs(&mut out);
        out
    }

    fn strip_leading_tabs(&self, out: &mut String) {
        // Only indentation is touched: `@embed` bodies and literals are opaque (rules 2/9), so a tab
        // inside them survives; the ones the formatter *could* have produced are already spaces.
        let mut res = String::with_capacity(out.len());
        let mut in_embed = false;
        for line in out.split_inclusive('\n') {
            let indent: usize = line.chars().take_while(|c| *c == ' ').count();
            let rest = &line[indent.min(line.len())..];
            if in_embed && rest.starts_with('@') {
                in_embed = false;
            }
            if !in_embed && rest.starts_with('\t') {
                let mut l = " ".repeat(indent);
                l.push_str(&rest.replace('\t', "    "));
                res.push_str(&l);
            } else {
                res.push_str(line);
            }
        }
        *out = res;
    }

    /// §9.5 rule 7: `"std/…"` · same-prefix-as-host-module · everything else; bytewise order inside
    /// a group; one blank line between groups; a repeated path keeps the LAST `as` alias.
    fn imports_doc(&mut self, imports: &[Import]) -> Doc {
        let mut merged: Vec<Import> = Vec::new();
        for im in imports {
            match merged.iter().position(|m| m.path == im.path && m.pub_ == im.pub_) {
                // Duplicate path: the later alias wins (rule 7). The whole statement is replaced so
                // `pub import` and `import` of one path never mix.
                Some(pos) => merged[pos] = im.clone(),
                None => merged.push(im.clone()),
            }
        }
        let host = if self.host.is_empty() {
            None
        } else {
            Some(self.host.clone())
        };
        let group_of = |p: &str| -> usize {
            if p == "std" || p.starts_with("std/") || p.starts_with("std::") {
                0
            } else if let Some(h) = &host {
                if p == h || p.starts_with(&format!("{h}/")) {
                    1
                } else {
                    2
                }
            } else {
                2
            }
        };
        let mut out: Vec<Doc> = Vec::new();
        let mut wrote_any = false;
        for g in 0..3 {
            let mut group: Vec<&Import> = merged.iter().filter(|i| group_of(&i.path) == g).collect();
            if group.is_empty() {
                continue;
            }
            group.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
            if wrote_any {
                out.push(Doc::Blank);
            }
            for (i, im) in group.iter().enumerate() {
                if i > 0 {
                    out.push(Doc::HardLine);
                }
                out.push(self.import_doc(im));
            }
            wrote_any = true;
        }
        if out.is_empty() {
            return Doc::Nil;
        }
        Doc::Cat(out)
    }

    fn import_doc(&mut self, im: &Import) -> Doc {
        // Comments that sat between import statements are hoisted to the block's head by the pool;
        // the statement itself is reprinted from its path literal plus the resolved alias (§9.5
        // rule 9 keeps the literal bytes).
        let path = self.verbatim(im.path_span, &format!("\"{}\"", im.path));
        let mut s = String::new();
        if im.pub_ {
            s.push_str("pub ");
        }
        s.push_str("import ");
        s.push_str(&path);
        match &im.alias {
            ImportAlias::None => s.push(';'),
            ImportAlias::Named(a) => {
                s.push_str(" as ");
                s.push_str(a);
                s.push(';');
            }
            ImportAlias::LinkOnly => s.push_str(" as .;"),
            ImportAlias::EffectOnly => s.push_str(" as _;"),
        }
        Doc::Text(s)
    }

    // ------------------------------------------------------------------ §9.5 rule 10: attrs & visibility

    fn attr_doc(&mut self, a: &Attr) -> Doc {
        // An `unsafe` marker the parser synthesised from `unsafe fn` / `unsafe trait` is not an
        // attribute in the source; it is printed in front of the keyword instead (§9.5 rule 10).
        if a.path.len() == 1 && a.path[0] == "unsafe" {
            return Doc::Nil;
        }
        if let Some(s) = self.span_text(a.span) {
            let t = s.trim_matches(|c: char| c.is_whitespace());
            if t.starts_with("#[") || t.starts_with("#![") {
                self.pool.mark_consumed(a.span.start, a.span.end);
                return Doc::Text(t.to_string());
            }
        }
        let mut parts: Vec<String> = Vec::new();
        for arg in &a.args {
            if let Some(n) = &arg.name {
                let v = arg
                    .value
                    .as_ref()
                    .map(|v| self.attr_value_text(v))
                    .unwrap_or_default();
                parts.push(format!("{n} = {v}"));
            }
        }
        for p in &a.positional {
            parts.push(self.attr_value_text(p));
        }
        let open = if a.inner { "#![" } else { "#[" };
        let body = if parts.is_empty() {
            format!("{}{}", open, a.path.join("::"))
        } else {
            format!("{}{}({})", open, a.path.join("::"), parts.join(", "))
        };
        Doc::Text(format!("{body}]"))
    }

    fn attr_value_text(&self, v: &AttrValue) -> String {
        match v {
            AttrValue::Str(s) => format!("\"{}\"", escape_minimal(s)),
            AttrValue::Int(i) => i.to_string(),
            AttrValue::Bool(b) => b.to_string(),
            AttrValue::Idents(xs) => xs.join(", "),
            AttrValue::Path(xs) => xs.join("::"),
            AttrValue::Lifetime(l) => format!("'{l}"),
            AttrValue::Expr(e) => flatten_doc(&self.expr_doc(e, 2)),
        }
    }

    fn vis_doc(&self, v: &Visibility) -> Option<Doc> {
        let s = match v {
            Visibility::Private => return None,
            Visibility::Pub => "pub",
            Visibility::PubCrate => "pub(crate)",
            Visibility::PubTo(p) => {
                return Some(Doc::Cat(vec![Doc::Text(format!("pub({p})")), Doc::HardLine]))
            }
            Visibility::Priv => "priv",
        };
        Some(Doc::Cat(vec![Doc::Text(s.to_string()), Doc::HardLine]))
    }

    // ------------------------------------------------------------------ §3.2 items

    fn item_nodes(&mut self, items: &[Item]) -> Vec<NodeSpec> {
        let mut out = Vec::with_capacity(items.len());
        for it in items {
            let doc = self.item_doc(it);
            out.push(NodeSpec { start: it.span.start, end: it.span.end, doc });
        }
        out
    }

    fn item_doc(&mut self, it: &Item) -> Doc {
        let mut v: Vec<Doc> = Vec::new();
        for a in &it.attrs {
            let d = self.attr_doc(a);
            if !is_empty_doc(&d) {
                v.push(d);
                v.push(Doc::HardLine);
            }
        }
        if let Some(vis) = self.vis_doc(&it.vis) {
            v.push(vis);
        }
        v.push(self.item_kind_doc(&it.kind, &it.attrs));
        Doc::Cat(v)
    }

    fn item_kind_doc(&mut self, k: &ItemKind, attrs: &[Attr]) -> Doc {
        match k {
            ItemKind::Event { sig, body } => self.fn_like_doc("event", sig, body, attrs),
            ItemKind::Fn { sig, body } => self.fn_like_doc("fn", sig, body, attrs),
            ItemKind::Struct { name, generics, fields, tuple, where_preds } => {
                let mut v = vec![Doc::Text(format!("struct {name}")), self.generics_doc(generics)];
                if !tuple.is_empty() && fields.is_empty() {
                    let items: Vec<Doc> = tuple
                        .iter()
                        .map(|(vis, ty)| {
                            let pre = vis_text(vis);
                            Doc::Cat(vec![Doc::Text(pre), self.ty_doc(ty)])
                        })
                        .collect();
                    v.push(delimited("(", items, ")", INDENT, false));
                } else {
                    let last = fields.last().map(|f| f.span.end).unwrap_or(0);
                    let nodes = self.field_nodes(fields);
                    let list = self.list_doc(nodes, self.close_brace_after(last), ListMode::Fields);
                    v.push(if fields.is_empty() {
                        Doc::Text(" {}".to_string())
                    } else {
                        Doc::Cat(vec![Doc::Text(" ".to_string()), braces(list)])
                    });
                }
                v.push(self.where_doc(where_preds));
                Doc::Cat(v)
            }
            ItemKind::Enum { name, generics, variants, where_preds } => {
                let mut v = vec![Doc::Text(format!("enum {name}")), self.generics_doc(generics)];
                let mut nodes: Vec<NodeSpec> = Vec::new();
                for var in variants {
                    let doc = self.variant_doc(var);
                    nodes.push(NodeSpec { start: var.span.start, end: var.span.end, doc });
                }
                let last = variants.last().map(|x| x.span.end).unwrap_or(0);
                let list = self.list_doc(nodes, self.close_brace_after(last), ListMode::Fields);
                v.push(if variants.is_empty() {
                    Doc::Text(" {}".to_string())
                } else {
                    Doc::Cat(vec![Doc::Text(" ".to_string()), braces(list)])
                });
                v.push(self.where_doc(where_preds));
                Doc::Cat(v)
            }
            ItemKind::Union { name, generics, fields } => {
                let mut v = vec![Doc::Text(format!("union {name}")), self.generics_doc(generics)];
                let last = fields.last().map(|f| f.span.end).unwrap_or(0);
                let nodes = self.field_nodes(fields);
                let list = self.list_doc(nodes, self.close_brace_after(last), ListMode::Fields);
                v.push(if fields.is_empty() {
                    Doc::Text(" {}".to_string())
                } else {
                    Doc::Cat(vec![Doc::Text(" ".to_string()), braces(list)])
                });
                Doc::Cat(v)
            }
            ItemKind::Trait { name, generics, is_unsafe, items } => {
                let head = if *is_unsafe { "unsafe trait" } else { "trait" };
                let mut v = vec![Doc::Text(format!("{head} {name}")), self.generics_doc(generics)];
                v.push(self.item_list_doc(items));
                Doc::Cat(v)
            }
            ItemKind::Impl { generics, trait_, trait_args, self_ty, items } => {
                let mut v = vec![Doc::Text("impl".to_string()), self.generics_doc(generics)];
                if let Some(p) = trait_ {
                    v.push(Doc::Text(p.join("::")));
                    if !trait_args.is_empty() {
                        let items: Vec<Doc> = trait_args.iter().map(|t| self.ty_doc(t)).collect();
                        v.push(self.angle_doc(items));
                    }
                    v.push(Doc::Text(" for ".to_string()));
                } else {
                    v.push(Doc::Text(" ".to_string()));
                }
                v.push(self.ty_doc(self_ty));
                v.push(self.item_list_doc(items));
                Doc::Cat(v)
            }
            ItemKind::TypeAlias { name, generics, ty } => {
                let mut v = vec![Doc::Text(format!("type {name}")), self.generics_doc(generics)];
                v.push(Doc::Text(" = ".to_string()));
                v.push(self.ty_doc(ty));
                v.push(Doc::Text(";".to_string()));
                Doc::Cat(v)
            }
            ItemKind::ExternBlock { abi, decls } => {
                let mut v = vec![Doc::Text("extern".to_string())];
                if let Some(a) = abi {
                    v.push(Doc::Text(format!(" \"{a}\"")));
                }
                v.push(Doc::Text(" ".to_string()));
                v.push(self.item_list_doc(decls));
                Doc::Cat(v)
            }
            ItemKind::Const { name, ty, init, .. } => self.const_like_doc("const", name, ty, init, false),
            ItemKind::Static { name, ty, init, is_mut } => {
                self.const_like_doc(if *is_mut { "static mut" } else { "static" }, name, ty, init, true)
            }
            ItemKind::MacroRules { name, arms } => {
                let mut v = vec![Doc::Text(format!("macro_rules! {name} "))];
                let mut inner: Vec<Doc> = Vec::new();
                for (i, a) in arms.iter().enumerate() {
                    if i > 0 {
                        inner.push(Doc::HardLine);
                    }
                    inner.push(self.macro_arm_doc(a));
                }
                if inner.is_empty() {
                    v.push(Doc::Text("{}".to_string()));
                } else {
                    v.push(braces(Doc::Cat(vec![
                        Doc::HardLine,
                        Doc::Cat(inner),
                        Doc::HardLine,
                    ])));
                }
                Doc::Cat(v)
            }
            ItemKind::Chain { name, stmts } => {
                let nodes = self.stmt_nodes(stmts);
                let last = stmts.last().map(|s| s.span.end).unwrap_or(0);
                let list = self.list_doc(nodes, self.close_brace_after(last), ListMode::Statements);
                if stmts.is_empty() {
                    Doc::Text(format!("chain {name} {{}}"))
                } else {
                    Doc::Cat(vec![
                        Doc::Text(format!("chain {name} ")),
                        braces(Doc::Cat(vec![Doc::HardLine, list, Doc::HardLine])),
                    ])
                }
            }
            ItemKind::Mod { name } => Doc::Text(format!("mod {name};")),
        }
    }

    /// Items inside `trait` / `impl` / `extern` bodies: every declaration on its own line with
    /// exactly one blank line between them (§9.5 rule 6).
    fn item_list_doc(&mut self, items: &[Item]) -> Doc {
        if items.is_empty() {
            return Doc::Text("{}".to_string());
        }
        let last = items.last().map(|i| i.span.end).unwrap_or(0);
        let nodes = self.item_nodes(items);
        let list = self.list_doc(nodes, self.close_brace_after(last), ListMode::Items);
        braces(Doc::Cat(vec![Doc::HardLine, list, Doc::HardLine]))
    }

    /// The offset just past the `}` that closes a list beginning after `from`.
    fn close_brace_after(&self, from: u32) -> u32 {
        let a = (from as usize).min(self.src.text.len());
        match self.src.text[a..].find('}') {
            Some(i) => (a + i + 1) as u32,
            None => self.src.text.len() as u32,
        }
    }

    // ------------------------------------------------------------------ §3.5 blocks & statements

    fn block_doc(&mut self, b: &Block) -> Doc {
        if b.stmts.is_empty() && b.tail.is_none() {
            // §9.5 rule 6: no blank line at the head or tail of a block.
            return Doc::Text("{}".to_string());
        }
        let mut v: Vec<Doc> = Vec::new();
        if !b.stmts.is_empty() {
            let nodes = self.stmt_nodes(&b.stmts);
            v.push(Doc::HardLine);
            v.push(self.list_doc(nodes, b.span.end, ListMode::Statements));
        }
        if let Some(t) = &b.tail {
            v.push(Doc::HardLine);
            let d = self.expr_doc(t, 2);
            v.push(d);
            // A comment between the tail expression and the closing brace still has to be emitted.
            let tail = self.pool.take_before(b.span.end);
            for &idx in &tail {
                v.push(Doc::HardLine);
                self.push_comment_doc(&mut v, idx, false);
            }
        }
        if v.is_empty() {
            return Doc::Text("{}".to_string());
        }
        v.push(Doc::HardLine);
        braces(Doc::Cat(v))
    }


    fn stmt_nodes(&mut self, stmts: &[Stmt]) -> Vec<NodeSpec> {
        let mut out = Vec::with_capacity(stmts.len());
        for s in stmts {
            let doc = self.stmt_doc(s);
            out.push(NodeSpec { start: s.span.start, end: s.span.end, doc });
        }
        out
    }

    fn stmt_doc(&mut self, s: &Stmt) -> Doc {
        let mut v: Vec<Doc> = Vec::new();
        for a in &s.attrs {
            let d = self.attr_doc(a);
            if !is_empty_doc(&d) {
                v.push(d);
                v.push(Doc::HardLine);
            }
        }
        let body: Doc = match &s.kind {
            StmtKind::Let { is_var, name, ty, init, .. } => {
                let kw = if *is_var { "var" } else { "let" };
                let mut x = vec![Doc::Text(format!("{kw} {name}"))];
                if let Some(t) = ty {
                    x.push(Doc::Text(": ".to_string()));
                    x.push(self.ty_doc(t));
                }
                if let Some(i) = init {
                    x.push(Doc::Text(" =".to_string()));
                    x.push(self.value_doc(i, 2));
                }
                x.push(Doc::Text(";".to_string()));
                Doc::Cat(x)
            }
            StmtKind::Assign { target, op, value } => {
                let mut x = vec![self.expr_doc(target, 3)];
                x.push(Doc::Text(format!(" {} ", punct_text(*op))));
                x.push(self.value_doc(value, 2));
                x.push(Doc::Text(";".to_string()));
                Doc::Cat(x)
            }
            StmtKind::Expr { expr, linkage } => {
                let mut x = vec![self.expr_doc(expr, 2)];
                for lb in linkage {
                    x.push(Doc::Text(" ".to_string()));
                    x.push(self.linkage_doc(lb));
                }
                if s.terminated && !blockish(expr) {
                    x.push(Doc::Text(";".to_string()));
                } else if s.terminated && blockish(expr) && needs_semi_after(expr) {
                    x.push(Doc::Text(";".to_string()));
                }
                Doc::Cat(x)
            }
            StmtKind::If { cond, then, else_ } => self.if_doc(cond, then, else_.as_deref()),
            StmtKind::While { cond, body } => Doc::Cat(vec![
                Doc::Text("while (".to_string()),
                self.expr_doc(cond, 2),
                Doc::Text(") ".to_string()),
                self.block_doc(body),
            ]),
            StmtKind::Loop { body, label } => {
                let mut x: Vec<Doc> = Vec::new();
                if let Some(l) = label {
                    x.push(Doc::Text(format!("'{l}: ")));
                }
                x.push(Doc::Text("loop ".to_string()));
                x.push(self.block_doc(body));
                Doc::Cat(x)
            }
            StmtKind::For { pat, iter, body, label } => {
                let mut x: Vec<Doc> = Vec::new();
                if let Some(l) = label {
                    x.push(Doc::Text(format!("'{l}: ")));
                }
                x.push(Doc::Text("for ".to_string()));
                x.push(self.pat_doc(pat));
                x.push(Doc::Text(" in (".to_string()));
                x.push(self.expr_doc(iter, 2));
                x.push(Doc::Text(") ".to_string()));
                x.push(self.block_doc(body));
                Doc::Cat(x)
            }
            StmtKind::Match { scrut, arms } => self.match_doc(scrut, arms),
            StmtKind::Break { label, value } => {
                let mut x = vec![Doc::Text("break".to_string())];
                if let Some(l) = label {
                    x.push(Doc::Text(format!(" '{l}")));
                }
                if let Some(v) = value {
                    x.push(Doc::Text(" ".to_string()));
                    x.push(self.expr_doc(v, 2));
                }
                x.push(Doc::Text(";".to_string()));
                Doc::Cat(x)
            }
            StmtKind::Continue { label } => {
                let mut x = vec![Doc::Text("continue".to_string())];
                if let Some(l) = label {
                    x.push(Doc::Text(format!(" '{l}")));
                }
                x.push(Doc::Text(";".to_string()));
                Doc::Cat(x)
            }
            StmtKind::Return { value } => {
                let mut x = vec![Doc::Text("return".to_string())];
                if let Some(v) = value {
                    x.push(Doc::Text(" ".to_string()));
                    x.push(self.value_doc(v, 2));
                }
                x.push(Doc::Text(";".to_string()));
                Doc::Cat(x)
            }
            StmtKind::Unsafe(b) => self.unsafe_doc(b),
            StmtKind::Defer(b) => {
                let mut x = vec![Doc::Text("defer ".to_string())];
                x.push(self.block_doc(b));
                Doc::Cat(x)
            }
            StmtKind::ConstAssert { cond, msg } => {
                let mut x = vec![Doc::Text("const_assert(".to_string()), self.expr_doc(cond, 2)];
                if let Some(m) = msg {
                    x.push(Doc::Text(format!(", \"{}\"", escape_minimal(m))));
                }
                x.push(Doc::Text(")".to_string()));
                Doc::Cat(x)
            }
            StmtKind::Label { name, .. } => Doc::Text(format!("label {name}:")),
            StmtKind::Pause => Doc::Text("pause;".to_string()),
            StmtKind::Stop => Doc::Text("stop;".to_string()),
            StmtKind::Resume => Doc::Text("resume;".to_string()),
            StmtKind::NestedItem(it) => self.item_doc(it),
        };
        v.push(body);
        Doc::Cat(v)
    }

    fn if_doc(&mut self, cond: &Expr, then: &Block, else_: Option<&ElseBranch>) -> Doc {
        let mut v = vec![
            Doc::Text("if (".to_string()),
            self.expr_doc(cond, 2),
            Doc::Text(") ".to_string()),
            self.block_doc(then),
        ];
        if let Some(e) = else_ {
            match e {
                ElseBranch::Block(b) => {
                    v.push(Doc::Text(" else ".to_string()));
                    v.push(self.block_doc(b));
                }
                ElseBranch::If(s) => {
                    v.push(Doc::Text(" else ".to_string()));
                    v.push(self.stmt_doc(s));
                }
            }
        }
        Doc::Cat(v)
    }

    fn match_doc(&mut self, scrut: &Expr, arms: &[MatchArm]) -> Doc {
        let mut v = vec![
            Doc::Text("match (".to_string()),
            self.expr_doc(scrut, 2),
            Doc::Text(") ".to_string()),
        ];
        if arms.is_empty() {
            v.push(Doc::Text("{}".to_string()));
            return Doc::Cat(v);
        }
        let last = arms.last().map(|a| a.span.end).unwrap_or(0);
        let end = self.close_brace_after(last);
        let mut nodes: Vec<NodeSpec> = Vec::new();
        for a in arms {
            nodes.push(NodeSpec { start: a.span.start, end: a.span.end, doc: self.arm_doc(a) });
        }
        let list = self.list_doc(nodes, end, ListMode::Fields);
        v.push(braces(Doc::Cat(vec![Doc::HardLine, list, Doc::HardLine])));
        Doc::Cat(v)
    }

    /// `match` arms: one per line only when the whole `match` does not fit (§9.5 rule 4), with the
    /// trailing comma that §9.5 rule 5 demands for the broken form.
    fn arm_doc(&mut self, a: &MatchArm) -> Doc {
        let mut v = vec![self.pat_doc(&a.pat)];
        if let Some(g) = &a.guard {
            v.push(Doc::Text(" if ".to_string()));
            v.push(self.expr_doc(g, 3));
        }
        v.push(Doc::Text(" => ".to_string()));
        match &a.body {
            ArmBody::Expr(e) => v.push(self.expr_doc(e, 2)),
            ArmBody::Block(b) => v.push(self.block_doc(b)),
        }
        Doc::Cat(v)
    }

    fn unsafe_doc(&mut self, b: &Block) -> Doc {
        let start = self.alloc_mark(MarkKind::UnsafeStart);
        let inner = self.block_doc(b);
        let end = self.alloc_mark(MarkKind::UnsafeEnd);
        Doc::Cat(vec![
            Doc::Mark(start),
            Doc::Text("unsafe ".to_string()),
            inner,
            Doc::Mark(end),
        ])
    }

    fn linkage_doc(&mut self, lb: &LinkageBlock) -> Doc {
        if lb.stmts.is_empty() {
            return Doc::Text("{}".to_string());
        }
        let mut v: Vec<Doc> = Vec::new();
        for (i, s) in lb.stmts.iter().enumerate() {
            if i > 0 {
                v.push(Doc::HardLine);
            }
            v.push(self.linkage_stmt_doc(s));
        }
        braces(Doc::Cat(vec![Doc::HardLine, Doc::Cat(v), Doc::HardLine]))
    }

    fn linkage_stmt_doc(&mut self, s: &LinkageStmt) -> Doc {
        let body: Doc = match s {
            LinkageStmt::Goto(g) => {
                let arg = match g {
                    GotoTarget::Named(n) => format!("\"{}\"", escape_minimal(n)),
                    GotoTarget::Abs(i) => i.to_string(),
                    GotoTarget::Rel(r) => {
                        if *r >= 0 {
                            format!("+{r}")
                        } else {
                            r.to_string()
                        }
                    }
                };
                Doc::Text(format!("goto({arg});"))
            }
            LinkageStmt::Start { chain, node, .. } => {
                let mut s = format!("start(chain = {chain}");
                match node {
                    Some(StartNode::Index(i)) => s.push_str(&format!(", node = {i}")),
                    Some(StartNode::Anchor(a)) => {
                        s.push_str(&format!(", node = \"{}\"", escape_minimal(a)))
                    }
                    Some(StartNode::Ident(i)) => s.push_str(&format!(", node = {i}")),
                    None => {}
                }
                s.push_str(");");
                Doc::Text(s)
            }
            LinkageStmt::StartFlag => Doc::Text("start_flag;".to_string()),
            LinkageStmt::Pause => Doc::Text("pause;".to_string()),
            LinkageStmt::Stop => Doc::Text("stop;".to_string()),
            LinkageStmt::Resume => Doc::Text("resume;".to_string()),
            LinkageStmt::If { cond, then, else_ } => {
                let mut v = vec![
                    Doc::Text("if (".to_string()),
                    self.expr_doc(cond, 2),
                    Doc::Text(") ".to_string()),
                    self.linkage_flat(then),
                ];
                if let Some(e) = else_ {
                    v.push(Doc::Text(" else ".to_string()));
                    v.push(self.linkage_flat(e));
                }
                Doc::Cat(v)
            }
            LinkageStmt::Plain(st) => self.stmt_doc(st),
        };
        body
    }

    fn linkage_flat(&mut self, stmts: &[LinkageStmt]) -> Doc {
        if stmts.is_empty() {
            return Doc::Text("{}".to_string());
        }
        let mut v: Vec<Doc> = Vec::new();
        for (i, s) in stmts.iter().enumerate() {
            if i > 0 {
                v.push(Doc::HardLine);
            }
            v.push(self.linkage_stmt_doc(s));
        }
        braces(Doc::Cat(vec![Doc::HardLine, Doc::Cat(v), Doc::HardLine]))
    }

    // ------------------------------------------------------------------ §3.4 types

    fn ty_doc(&mut self, t: &Type) -> Doc {
        match &t.kind {
            TypeKind::Prim(p) => Doc::Text(p.text().to_string()),
            TypeKind::Named { path, args } => {
                let mut v = vec![Doc::Text(path.join("::"))];
                if !args.is_empty() {
                    let items: Vec<Doc> = args.iter().map(|a| self.ty_doc(a)).collect();
                    v.push(self.angle_doc(items));
                }
                Doc::Cat(v)
            }
            TypeKind::Ref { lifetime, mutable, inner } => {
                let mut v = vec![Doc::Text("&".to_string())];
                if let Some(l) = lifetime {
                    v.push(Doc::Text(format!("{l} ")));
                }
                if *mutable {
                    v.push(Doc::Text("mut ".to_string()));
                }
                v.push(self.ty_doc(inner));
                Doc::Cat(v)
            }
            TypeKind::Ptr { mutable, inner } => {
                let mut v = vec![Doc::Text("* ".to_string())];
                v.push(Doc::Text(if *mutable { "mut " } else { "const " }.to_string()));
                v.push(self.ty_doc(inner));
                Doc::Cat(v)
            }
            TypeKind::Array { inner, len } => {
                let mut v = vec![Doc::Text("[".to_string()), self.ty_doc(inner)];
                v.push(Doc::Text("; ".to_string()));
                v.push(self.expr_doc(len, 2));
                v.push(Doc::Text("]".to_string()));
                Doc::Cat(v)
            }
            TypeKind::Slice { inner } => Doc::Cat(vec![Doc::Text("[".to_string()), self.ty_doc(inner), Doc::Text("]".to_string())]),
            TypeKind::Dyn { path, bounds } => {
                let mut v = vec![Doc::Text(format!("dyn {}", path.join("::")))];
                if !bounds.is_empty() {
                    v.push(Doc::Text(" + ".to_string()));
                    v.push(self.bounds_doc(bounds));
                }
                Doc::Cat(v)
            }
            TypeKind::FnPtr { params, ret } => {
                let items: Vec<Doc> = params.iter().map(|p| self.ty_doc(p)).collect();
                let mut v = vec![Doc::Text("fn".to_string()), delimited("(", items, ")", INDENT, false)];
                if let Some(r) = ret {
                    v.push(Doc::Text(" -> ".to_string()));
                    v.push(self.ty_doc(r));
                }
                Doc::Cat(v)
            }
            TypeKind::Tuple(ts) => {
                if ts.is_empty() {
                    return Doc::Text("()".to_string());
                }
                let items: Vec<Doc> = ts.iter().map(|t| self.ty_doc(t)).collect();
                if ts.len() == 1 {
                    // A one-element tuple needs its trailing comma to stay a tuple (§3.4).
                    return Doc::Cat(vec![
                        Doc::Text("(".to_string()),
                        items.into_iter().next().unwrap_or(Doc::Nil),
                        Doc::Text(",)".to_string()),
                    ]);
                }
                delimited("(", items, ")", INDENT, false)
            }
            TypeKind::Never => Doc::Text("!".to_string()),
            TypeKind::Invalid => Doc::Text(self.verbatim(t.span, "!")),
        }
    }

    // ------------------------------------------------------------------ §3.12 patterns

    fn pat_doc(&mut self, p: &Pat) -> Doc {
        match &p.kind {
            PatKind::Wildcard => Doc::Text("_".to_string()),
            PatKind::DotDot => Doc::Text("..".to_string()),
            PatKind::Lit(_) => Doc::Text(self.verbatim(p.span, "?")),
            PatKind::Binding { name, by_ref, mutable, sub } => {
                let mut s = String::new();
                if *by_ref {
                    s.push_str("ref ");
                }
                if *mutable {
                    s.push_str("mut ");
                }
                s.push_str(name);
                let mut v = vec![Doc::Text(s)];
                if let Some(sub) = sub {
                    v.push(Doc::Text(" @ ".to_string()));
                    v.push(self.pat_doc(sub));
                }
                Doc::Cat(v)
            }
            PatKind::Path(path) => Doc::Text(path.join("::")),
            PatKind::Constructor { path, args, fields, has_rest } => {
                let head = path.join("::");
                if !fields.is_empty() || *has_rest {
                    let mut items: Vec<Doc> = Vec::new();
                    for (n, p) in fields {
                        let mut v = vec![Doc::Text(format!("{n}: "))];
                        v.push(self.pat_doc(p));
                        items.push(Doc::Cat(v));
                    }
                    if *has_rest {
                        items.push(Doc::Text("..".to_string()));
                    }
                    Doc::Cat(vec![Doc::Text(head), Doc::Text(" ".to_string()), delimited("{", items, "}", INDENT, true)])
                } else if !args.is_empty() {
                    let items: Vec<Doc> = args.iter().map(|a| self.pat_doc(a)).collect();
                    Doc::Cat(vec![Doc::Text(head), delimited("(", items, ")", INDENT, false)])
                } else {
                    Doc::Text(head)
                }
            }
            PatKind::Tuple(ps) => {
                let items: Vec<Doc> = ps.iter().map(|p| self.pat_doc(p)).collect();
                delimited("(", items, ")", INDENT, false)
            }
            PatKind::Slice(ps, rest) => {
                let mut items: Vec<Doc> = ps.iter().map(|p| self.pat_doc(p)).collect();
                if let Some(r) = rest {
                    if matches!(r.kind, PatKind::Wildcard) {
                        items.push(Doc::Text("..".to_string()));
                    } else {
                        let mut v = vec![Doc::Text("..".to_string())];
                        v.push(self.pat_doc(r));
                        items.push(Doc::Cat(v));
                    }
                }
                delimited("[", items, "]", INDENT, false)
            }
            PatKind::Or(ps) => {
                let mut v: Vec<Doc> = Vec::new();
                for (i, p) in ps.iter().enumerate() {
                    if i > 0 {
                        v.push(Doc::Text(" | ".to_string()));
                    }
                    v.push(self.pat_doc(p));
                }
                Doc::Cat(v)
            }
            PatKind::Range { lo, hi, inclusive } => {
                let mut v = vec![self.pat_doc(lo)];
                v.push(Doc::Text(if *inclusive { "..= ".to_string() } else { ".. ".to_string() }));
                v.push(self.pat_doc(hi));
                Doc::Cat(v)
            }
            PatKind::Exclude(ps) => {
                let mut v = vec![Doc::Text("!(".to_string())];
                for (i, p) in ps.iter().enumerate() {
                    if i > 0 {
                        v.push(Doc::Text(" | ".to_string()));
                    }
                    v.push(self.pat_doc(p));
                }
                v.push(Doc::Text(")".to_string()));
                Doc::Cat(v)
            }
            PatKind::Invalid => Doc::Text(self.verbatim(p.span, "_")),
        }
    }

    // ------------------------------------------------------------------ §3.11 / §4.4 expressions

    /// The value side of `let` / assignment / `return`. §9.5 rule 3 gives every construct its own
    /// break points; nothing is force-broken after `=`.
    fn value_doc(&mut self, e: &Expr, min: u8) -> Doc {
        self.expr_doc(e, min)
    }

    fn expr_doc(&mut self, e: &Expr, min: u8) -> Doc {
        let inner = self.expr_inner(e);
        if kind_prec(&e.kind) < min {
            Doc::Cat(vec![Doc::Text("(".to_string()), inner, Doc::Text(")".to_string())])
        } else {
            inner
        }
    }

    fn expr_inner(&mut self, e: &Expr) -> Doc {
        match &e.kind {
            ExprKind::Invalid => Doc::Text(self.verbatim(e.span, "!")),
            ExprKind::Lit(_) | ExprKind::Int(_) => Doc::Text(self.verbatim(e.span, "?")),
            ExprKind::Auto => Doc::Text("auto".to_string()),
            ExprKind::Ident(name) => {
                // The AST cannot express every spelling (`x::<T>` collapses to a path, and a failed
                // embed-argument parse stores the raw text as an identifier), so the source bytes
                // win whenever they differ (§9.5 rule 9).
                let fb = name.clone();
                Doc::Text(self.verbatim(e.span, &fb))
            }
            ExprKind::Path(path) => {
                let fb = path.join("::");
                Doc::Text(self.verbatim(e.span, &fb))
            }
            ExprKind::Unary { op, expr } => {
                let mut v = vec![Doc::Text(punct_text(*op).to_string())];
                v.push(self.expr_doc(expr, 14));
                Doc::Cat(v)
            }
            ExprKind::Binary { op, lhs, rhs } => self.binary_doc(e, *op, lhs, rhs),
            ExprKind::Assign { target, op, value } => {
                let mut v = vec![self.expr_doc(target, 3)];
                v.push(Doc::Text(format!(" {}", punct_text(*op))));
                v.push(Doc::Line);
                v.push(nest(INDENT, self.expr_doc(value, 2)));
                group(Doc::Cat(v))
            }
            ExprKind::Ref { mutable, lifetime, expr } => {
                let mut v = vec![Doc::Text("&".to_string())];
                if let Some(l) = lifetime {
                    v.push(Doc::Text(format!("{l} ")));
                }
                if *mutable {
                    v.push(Doc::Text("mut ".to_string()));
                }
                v.push(self.expr_doc(expr, 14));
                Doc::Cat(v)
            }
            ExprKind::Deref { expr } => {
                Doc::Cat(vec![Doc::Text("*".to_string()), self.expr_doc(expr, 14)])
            }
            ExprKind::Move { expr } => {
                Doc::Cat(vec![Doc::Text("move ".to_string()), self.expr_doc(expr, 14)])
            }
            ExprKind::Try { expr } => {
                Doc::Cat(vec![self.chain_doc(expr, true), Doc::Text("?".to_string())])
            }
            ExprKind::Cast { expr, ty } => {
                let mut v = vec![self.expr_doc(expr, 2), Doc::Text(" as ".to_string())];
                v.push(self.ty_doc(ty));
                Doc::Cat(v)
            }
            ExprKind::Is { expr, ty } => {
                let mut v = vec![self.expr_doc(expr, 2), Doc::Text(" is ".to_string())];
                v.push(self.ty_doc(ty));
                Doc::Cat(v)
            }
            ExprKind::Call { callee, args } => self.call_doc(e, callee, args),
            ExprKind::Method { recv, name, turbofish, args } => {
                let mut head = format!(".{name}");
                if !turbofish.is_empty() {
                    head.push_str("::");
                }
                let mut v = vec![Doc::Text(head)];
                if !turbofish.is_empty() {
                    let items: Vec<Doc> = turbofish.iter().map(|t| self.ty_doc(t)).collect();
                    v.push(Doc::Text("<".to_string()));
                    let mut inner: Vec<Doc> = Vec::new();
                    for (i, d) in items.into_iter().enumerate() {
                        if i > 0 {
                            inner.push(Doc::Text(", ".to_string()));
                        }
                        inner.push(d);
                    }
                    v.push(Doc::Cat(inner));
                    v.push(Doc::Text(">".to_string()));
                }
                v.push(self.args_doc(args));
                let _ = recv;
                Doc::Cat(v)
            }
            ExprKind::Field { base, name } => {
                let f = match name {
                    FieldName::Named(n) => format!(".{n}"),
                    FieldName::Index(i) => format!(".{i}"),
                };
                Doc::Cat(vec![self.expr_doc(base, 16), Doc::Text(f)])
            }
            ExprKind::Index { base, index } => {
                let mut v = vec![self.expr_doc(base, 16), Doc::Text("[".to_string())];
                v.push(self.expr_doc(index, 2));
                v.push(Doc::Text("]".to_string()));
                Doc::Cat(v)
            }
            ExprKind::Tuple(items) => {
                if items.is_empty() {
                    return Doc::Text("()".to_string());
                }
                let ds: Vec<Doc> = items.iter().map(|i| self.expr_doc(i, 2)).collect();
                if ds.len() == 1 {
                    let one = ds.into_iter().next().unwrap_or(Doc::Nil);
                    return Doc::Cat(vec![Doc::Text("(".to_string()), one, Doc::Text(",)".to_string())]);
                }
                delimited("(", ds, ")", INDENT, false)
            }
            ExprKind::Array(items) => {
                let ds: Vec<Doc> = items.iter().map(|i| self.expr_doc(i, 2)).collect();
                delimited("[", ds, "]", INDENT, false)
            }
            ExprKind::ArrayRepeat { value, len } => {
                let mut v = vec![Doc::Text("[".to_string()), self.expr_doc(value, 2)];
                v.push(Doc::Text("; ".to_string()));
                v.push(self.expr_doc(len, 2));
                v.push(Doc::Text("]".to_string()));
                Doc::Cat(v)
            }
            ExprKind::StructLit { path, fields, rest } => {
                let mut ds: Vec<Doc> = Vec::new();
                for (n, v) in fields {
                    let mut x = vec![Doc::Text(format!("{n}: "))];
                    x.push(self.expr_doc(v, 2));
                    ds.push(Doc::Cat(x));
                }
                if let Some(r) = rest {
                    let mut x = vec![Doc::Text("..".to_string())];
                    x.push(self.expr_doc(r, 2));
                    ds.push(Doc::Cat(x));
                }
                let head = path.join("::");
                if ds.is_empty() {
                    Doc::Text(format!("{head} {{}}"))
                } else {
                    Doc::Cat(vec![Doc::Text(head), Doc::Text(" ".to_string()), delimited("{", ds, "}", INDENT, true)])
                }
            }
            ExprKind::If { cond, then, else_ } => {
                let mut v = vec![
                    Doc::Text("if (".to_string()),
                    self.expr_doc(cond, 2),
                    Doc::Text(") ".to_string()),
                    self.block_doc(then),
                ];
                if let Some(e) = else_ {
                    match e.as_ref() {
                        ElseBranch::Block(b) => {
                            v.push(Doc::Text(" else ".to_string()));
                            v.push(self.block_doc(b));
                        }
                        ElseBranch::If(s) => {
                            v.push(Doc::Text(" else ".to_string()));
                            v.push(self.stmt_doc(s));
                        }
                    }
                }
                Doc::Cat(v)
            }
            ExprKind::Match { scrut, arms } => self.match_doc(scrut, arms),
            ExprKind::Block(b) => self.block_doc(b),
            ExprKind::Unsafe(b) => self.unsafe_doc(b),
            ExprKind::Closure { move_, params, ret, body } => {
                let mut v: Vec<Doc> = Vec::new();
                let mut head = String::new();
                if *move_ {
                    head.push_str("move ");
                }
                head.push('|');
                for (i, (n, t)) in params.iter().enumerate() {
                    if i > 0 {
                        head.push_str(", ");
                    }
                    head.push_str(n);
                    if let Some(ty) = t {
                        head.push_str(": ");
                        head.push_str(&flatten_doc(&self.ty_doc(ty)));
                    }
                }
                head.push('|');
                if let Some(r) = ret {
                    head.push_str(" -> ");
                    head.push_str(&flatten_doc(&self.ty_doc(r)));
                }
                v.push(Doc::Text(head));
                v.push(Doc::Text(" ".to_string()));
                v.push(self.block_doc(body));
                Doc::Cat(v)
            }
            ExprKind::Range { lo, hi, inclusive } => {
                let mut v: Vec<Doc> = Vec::new();
                if let Some(l) = lo {
                    v.push(self.expr_doc(l, 16));
                }
                v.push(Doc::Text(if *inclusive { "..=" } else { ".." }.to_string()));
                if let Some(h) = hi {
                    v.push(self.expr_doc(h, 4));
                }
                Doc::Cat(v)
            }
            ExprKind::Embed(eb) => self.embed_doc(eb),
            ExprKind::MacroCall { name, delim, tokens } => {
                let (open, close) = match delim {
                    Delim::Paren => ("(", ")"),
                    Delim::Bracket => ("[", "]"),
                    Delim::Brace => ("{", "}"),
                };
                let inner = tokens_text(tokens);
                Doc::Text(format!("{name}!{open}{inner}{close}"))
            }
        }
    }

    /// §9.5 rule 3: a chain that does not fit breaks **before every `.`**, 8 columns in; one that
    /// fits stays on a single line (§9.5 rule 4's collapse requirement).
    fn chain_doc(&mut self, e: &Expr, stop_at_try: bool) -> Doc {
        let mut segs: Vec<Seg<'_>> = Vec::new();
        let base = flatten_chain(e, &mut segs, stop_at_try);
        if segs.len() < 2 {
            let _ = base;
            return self.postfix_doc(e);
        }
        let base_start = base.span;
        let base_doc = self.atom_chain_base(base);
        let mut parts: Vec<Doc> = vec![base_doc];
        let mut group_items: Vec<Doc> = Vec::new();
        for s in segs {
            match s {
                Seg::Glue(t) => {
                    if let Some(last) = group_items.last_mut() {
                        *last = Doc::Cat(vec![last.clone(), Doc::Text(t)]);
                    } else {
                        parts.push(Doc::Text(t));
                    }
                }
                other => {
                    let d = self.seg_doc(other);
                    group_items.push(Doc::Cat(vec![Doc::SoftLine, d]));
                }
            }
        }
        parts.push(nest(CHAIN_CONT, group(Doc::Cat(group_items))));
        // A parenthesised base needs its own parens; `atom_chain_base` already handled that.
        let _ = base_start;
        Doc::Cat(parts)
    }

    fn atom_chain_base(&mut self, base: &Expr) -> Doc {
        self.expr_doc(base, 16)
    }

    fn seg_doc(&mut self, s: Seg<'_>) -> Doc {
        match s {
            Seg::Method(name, fish, args) => {
                let mut v = vec![Doc::Text(format!(".{name}"))];
                if !fish.is_empty() {
                    v.push(Doc::Text("::<".to_string()));
                    let mut inner: Vec<Doc> = Vec::new();
                    for (i, t) in fish.iter().enumerate() {
                        if i > 0 {
                            inner.push(Doc::Text(", ".to_string()));
                        }
                        inner.push(self.ty_doc(t));
                    }
                    v.push(Doc::Cat(inner));
                    v.push(Doc::Text(">".to_string()));
                }
                v.push(self.args_doc(&args));
                Doc::Cat(v)
            }
            Seg::Call(args) => self.args_doc(&args),
            Seg::Field(n) => match n {
                FieldName::Named(x) => Doc::Text(format!(".{x}")),
                FieldName::Index(i) => Doc::Text(format!(".{i}")),
            },
            Seg::Index(ix) => {
                let mut v = vec![Doc::Text("[".to_string())];
                v.push(self.expr_doc(&ix, 2));
                v.push(Doc::Text("]".to_string()));
                Doc::Cat(v)
            }
            Seg::Try => Doc::Text("?".to_string()),
        }
    }

    /// `f(a, b)` / `f.k = v` argument list (§9.5 rule 4: one per line only when it does not fit).
    fn args_doc(&mut self, args: &[CallArg]) -> Doc {
        let ds: Vec<Doc> = args
            .iter()
            .map(|a| match &a.name {
                Some(n) => {
                    let mut v = vec![Doc::Text(format!("{n} = "))];
                    v.push(self.expr_doc(&a.expr, 2));
                    Doc::Cat(v)
                }
                None => self.expr_doc(&a.expr, 2),
            })
            .collect();
        delimited("(", ds, ")", INDENT, false)
    }

    fn call_doc(&mut self, e: &Expr, callee: &Expr, args: &[CallArg]) -> Doc {
        let _ = e;
        if matches!(callee.kind, ExprKind::Method { .. } | ExprKind::Call { .. } | ExprKind::Field { .. } | ExprKind::Index { .. }) {
            return self.chain_doc(callee, false);
        }
        let mut v: Vec<Doc> = Vec::new();
        let head = self.expr_doc(callee, 16);
        // `path::<T>(args)`: the parser drops the turbofish arguments from the AST, so their source
        // bytes are re-emitted verbatim (they would otherwise silently disappear, §9.5 rule 9).
        let tail = self.turbofish_between(callee.span);
        v.push(head);
        if !tail.is_empty() {
            v.push(Doc::Text(tail));
        }
        v.push(self.args_doc(args));
        Doc::Cat(v)
    }

    fn turbofish_between(&self, callee_span: Span) -> String {
        let a = (callee_span.end as usize).min(self.src.text.len());
        let b = (a + 512).min(self.src.text.len());
        let region = match self.src.text.get(a..b) {
            Some(r) => r,
            None => return String::new(),
        };
        let upto = match region.find('(') {
            Some(i) => i,
            None => region.len(),
        };
        let t = region[..upto].trim();
        if t.starts_with("::") {
            t.to_string()
        } else {
            String::new()
        }
    }

    /// Non-chain postfix printing (single-segment chains and simple field access).
    fn postfix_doc(&mut self, e: &Expr) -> Doc {
        match &e.kind {
            ExprKind::Method { recv, name, turbofish, args } => {
                let mut v = vec![self.expr_doc(recv, 16)];
                v.push(Doc::Text(format!(".{name}")));
                if !turbofish.is_empty() {
                    v.push(Doc::Text("::<".to_string()));
                    let mut inner: Vec<Doc> = Vec::new();
                    for (i, t) in turbofish.iter().enumerate() {
                        if i > 0 {
                            inner.push(Doc::Text(", ".to_string()));
                        }
                        inner.push(self.ty_doc(t));
                    }
                    v.push(Doc::Cat(inner));
                    v.push(Doc::Text(">".to_string()));
                }
                v.push(self.args_doc(args));
                Doc::Cat(v)
            }
            ExprKind::Call { callee, args } => {
                let mut v = vec![self.expr_doc(callee, 16)];
                let tail = self.turbofish_between(callee.span);
                if !tail.is_empty() {
                    v.push(Doc::Text(tail));
                }
                v.push(self.args_doc(args));
                Doc::Cat(v)
            }
            ExprKind::Field { base, name } => {
                let f = match name {
                    FieldName::Named(n) => format!(".{n}"),
                    FieldName::Index(i) => format!(".{i}"),
                };
                Doc::Cat(vec![self.expr_doc(base, 16), Doc::Text(f)])
            }
            ExprKind::Index { base, index } => {
                let mut v = vec![self.expr_doc(base, 16), Doc::Text("[".to_string())];
                v.push(self.expr_doc(index, 2));
                v.push(Doc::Text("]".to_string()));
                Doc::Cat(v)
            }
            other => self.expr_inner_shallow(other),
        }
    }

    fn expr_inner_shallow(&mut self, k: &ExprKind) -> Doc {
        // Only reached from `postfix_doc` for shapes it does not handle; render defensively from the
        // source bytes so nothing is ever lost.
        match k {
            _ => Doc::Nil,
        }
    }

    /// §9.5 rules 3/4: same-precedence operators collapse into one breakable run, always with the
    /// operator at the end of the line (§2.11's trailing-operator normalisation).
    fn binary_doc(&mut self, e: &Expr, op: Punct, lhs: &Expr, rhs: &Expr) -> Doc {
        let (prec, _right) = binop_prec(op).unwrap_or((12, false));
        let mut operands: Vec<&Expr> = Vec::new();
        collect_binary(lhs, op, prec, &mut operands);
        operands.push(rhs);
        collect_binary_rhs(rhs, op, prec, &mut operands);
        let mut items: Vec<Doc> = Vec::new();
        for (i, o) in operands.iter().enumerate() {
            if i > 0 {
                items.push(Doc::Text(format!(" {}", punct_text(op))));
                items.push(Doc::Line);
            }
            let min = if i + 1 == operands.len() { prec + 1 } else { prec };
            items.push(self.expr_doc(o, min));
        }
        group(nest(INDENT, Doc::Cat(items)))
    }

    // ------------------------------------------------------------------ §9.5 rule 8: `infy-safety`

    /// `// infy-safety: …` must sit on the `unsafe` block's line, the line above it, or the line
    /// below it (the module header's note 4 reconciles §9.5 rule 8 with §4.7.5, which asks for the
    /// comment to *follow* the block). Anything else is `W-1030`.
    pub fn safety_warnings(&self, sources: &Sources, file: FileId, handler: &mut Handler) {
        if self.comment_lines.is_empty() {
            return;
        }
        let mut unsafe_lines: Vec<(u32, u32)> = Vec::new();
        for (id, kind) in &self.marks {
            if *kind != MarkKind::UnsafeStart {
                continue;
            }
            if let Some((_, off)) = self.offsets.iter().find(|(m, _)| m == id) {
                let line = line_of_byte(&self.src.text, *off);
                unsafe_lines.push((*id, line));
            }
        }
        let mut unsafe_ends: Vec<u32> = Vec::new();
        for (id, kind) in &self.marks {
            if *kind == MarkKind::UnsafeEnd {
                if let Some((_, off)) = self.offsets.iter().find(|(m, _)| m == id) {
                    unsafe_ends.push(line_of_byte(&self.src.text, *off));
                }
            }
        }
        for (id, src_start, _inline) in &self.comment_lines {
            let out_line = match self.offsets.iter().find(|(m, _)| m == id) {
                Some((_, off)) => line_of_byte(&self.src.text, *off),
                None => continue,
            };
            let adjacent = unsafe_lines
                .iter()
                .any(|(_, l)| *l == out_line || *l == out_line + 1)
                || unsafe_ends.iter().any(|l| *l + 1 == out_line);
            if !adjacent {
                let pos = sources
                    .get(file)
                    .map(|f| {
                        let (line, col) = f.byte_line_col(*src_start);
                        crate::span::Position { line, column: col }
                    })
                    .unwrap_or(crate::span::Position { line: 1, column: 1 });
                let d = Diagnostic::new("W-1030", Phase::Fmt, file, pos)
                    .note("§9.5 第 8 条：`// infy-safety:` 必须留在其 `unsafe` 块同一行或紧邻上一行")
                    .help("把注释移到对应 `unsafe` 块旁");
                handler.emit(d);
            }
        }
    }
}

fn line_of_byte(text: &str, off: usize) -> u32 {
    1 + text[..off.min(text.len())].matches('\n').count() as u32
}

/// One link of a postfix chain.
enum Seg<'x> {
    Method(String, Vec<Type>, Vec<CallArg>),
    Call(Vec<CallArg>),
    Field(FieldName),
    Index(Box<Expr>),
    /// `?` — glued to the segment it belongs to (§9.5 rule 3).
    Try,
    Glue(String),
    /// A link that must not start a line on its own; it is appended to the previous segment.
    #[allow(dead_code)]
    Leaf(&'x Expr),
}

/// Walk a postfix chain bottom-up, returning the non-chain base expression.
fn flatten_chain<'x>(e: &'x Expr, segs: &mut Vec<Seg<'x>>, stop_at_try: bool) -> &'x Expr {
    match &e.kind {
        ExprKind::Method { recv, name, turbofish, args } => {
            let b = flatten_chain(recv, segs, stop_at_try);
            segs.push(Seg::Method(name.clone(), turbofish.clone(), args.clone()));
            b
        }
        ExprKind::Call { callee, args } => {
            let b = flatten_chain(callee, segs, stop_at_try);
            segs.push(Seg::Call(args.clone()));
            b
        }
        ExprKind::Field { base, name } => {
            let b = flatten_chain(base, segs, stop_at_try);
            segs.push(Seg::Field(name.clone()));
            b
        }
        ExprKind::Index { base, index } => {
            let b = flatten_chain(base, segs, stop_at_try);
            segs.push(Seg::Index(index.clone()));
            b
        }
        ExprKind::Try { expr } => {
            let b = flatten_chain(expr, segs, stop_at_try);
            segs.push(Seg::Glue("?".to_string()));
            b
        }
        _ => {
            if stop_at_try {
                let _ = e;
            }
            e
        }
    }
}

// --------------------------------------------------------------------------------------------
// Precedence (§2.7's table, mirrored from `parser::binop_of`), and the paren decision it drives.
// §4.10 fixes evaluation order, so a re-print may never reassociate: a child whose precedence is
// below the position's minimum is parenthesised.
// --------------------------------------------------------------------------------------------

fn binop_prec(p: Punct) -> Option<(u8, bool)> {
    Some(match p {
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
        Punct::Eq
        | Punct::PlusEq
        | Punct::MinusEq
        | Punct::StarEq
        | Punct::SlashEq
        | Punct::PercentEq
        | Punct::AmpEq
        | Punct::HatEq
        | Punct::PipeEq
        | Punct::ShlEq
        | Punct::ShrEq => (2, true),
        _ => return None,
    })
}

fn kind_prec(k: &ExprKind) -> u8 {
    match k {
        ExprKind::Binary { op, .. } | ExprKind::Assign { op, .. } => binop_prec(*op).unwrap_or((3, false)).0,
        ExprKind::Range { .. } => 3,
        ExprKind::Unary { .. } | ExprKind::Ref { .. } | ExprKind::Deref { .. } | ExprKind::Move { .. } => 14,
        ExprKind::Cast { .. } | ExprKind::Is { .. } => 15,
        ExprKind::Call { .. }
        | ExprKind::Method { .. }
        | ExprKind::Field { .. }
        | ExprKind::Index { .. }
        | ExprKind::Try { .. }
        | ExprKind::MacroCall { .. } => 16,
        _ => 20,
    }
}

fn collect_binary<'x>(e: &'x Expr, op: Punct, prec: u8, out: &mut Vec<&'x Expr>) {
    if let ExprKind::Binary { op: o, lhs, rhs } = &e.kind {
        if *o == op && binop_prec(*o).unwrap_or((0, false)).0 == prec {
            collect_binary(lhs, op, prec, out);
            out.push(rhs);
            return;
        }
    }
    out.push(e);
}

fn collect_binary_rhs<'x>(e: &'x Expr, op: Punct, prec: u8, out: &mut Vec<&'x Expr>) {
    if let ExprKind::Binary { op: o, lhs, rhs } = &e.kind {
        if *o == op && binop_prec(*o).unwrap_or((0, false)).0 == prec {
            out.push(lhs);
            collect_binary_rhs(rhs, op, prec, out);
        }
    }
}

// --------------------------------------------------------------------------------------------
// Small textual helpers.
// --------------------------------------------------------------------------------------------

fn vis_text(v: &Visibility) -> String {
    match v {
        Visibility::Private => String::new(),
        Visibility::Pub => "pub ".to_string(),
        Visibility::PubCrate => "pub(crate) ".to_string(),
        Visibility::PubTo(p) => format!("pub({p}) "),
        Visibility::Priv => "priv ".to_string(),
    }
}

fn is_empty_doc(d: &Doc) -> bool {
    match d {
        Doc::Nil => true,
        Doc::Text(s) => s.is_empty(),
        Doc::Cat(v) => v.iter().all(is_empty_doc),
        _ => false,
    }
}

fn braces(inner: Doc) -> Doc {
    Doc::Cat(vec![Doc::Text("{".to_string()), inner, Doc::Text("}".to_string())])
}

/// Render a doc with no line breaks at all — used for the one-line positions of attribute values
/// and closure signatures.
fn flatten_doc(d: &Doc) -> String {
    let mut out = String::new();
    flatten_into(d, &mut out);
    out
}

fn flatten_into(d: &Doc, out: &mut String) {
    match d {
        Doc::Nil | Doc::Mark(_) => {}
        Doc::Text(t) => out.push_str(t),
        Doc::Cat(v) => {
            for x in v {
                flatten_into(x, out);
            }
        }
        Doc::Nest(_, b) => flatten_into(b, out),
        Doc::Line => out.push(' '),
        Doc::SoftLine | Doc::HardLine | Doc::Blank => {}
        Doc::Group(b, _) => flatten_into(b, out),
        Doc::IfBreak(_, f) => out.push_str(f),
    }
}

fn punct_text(p: Punct) -> &'static str {
    match p {
        Punct::Dot => ".",
        Punct::DotDot => "..",
        Punct::DotDotEq => "..=",
        Punct::Comma => ",",
        Punct::Semi => ";",
        Punct::Colon => ":",
        Punct::ColonColon => "::",
        Punct::LParen => "(",
        Punct::RParen => ")",
        Punct::LBracket => "[",
        Punct::RBracket => "]",
        Punct::LBrace => "{",
        Punct::RBrace => "}",
        Punct::Bang => "!",
        Punct::Eq => "=",
        Punct::FatArrow => "=>",
        Punct::Arrow => "->",
        Punct::Question => "?",
        Punct::Amp => "&",
        Punct::AmpAmp => "&&",
        Punct::Pipe => "|",
        Punct::PipePipe => "||",
        Punct::Hat => "^",
        Punct::Star => "*",
        Punct::Percent => "%",
        Punct::Plus => "+",
        Punct::Minus => "-",
        Punct::Lt => "<",
        Punct::LtEq => "<=",
        Punct::Gt => ">",
        Punct::GtEq => ">=",
        Punct::EqEq => "==",
        Punct::BangEq => "!=",
        Punct::Shl => "<<",
        Punct::Shr => ">>",
        Punct::PlusEq => "+=",
        Punct::MinusEq => "-=",
        Punct::StarEq => "*=",
        Punct::PercentEq => "%=",
        Punct::AmpEq => "&=",
        Punct::HatEq => "^=",
        Punct::PipeEq => "|=",
        Punct::ShlEq => "<<=",
        Punct::ShrEq => ">>=",
        Punct::Slash => "/",
        Punct::SlashEq => "/=",
        Punct::At => "@",
        Punct::Hash => "#",
        Punct::Dollar => "$",
        Punct::Tilde => "~",
    }
}

/// Reprint a macro token tree from the exact token bytes (§9.5 rule 9), with a single space between
// two leaves that were separated by whitespace in the source.
fn trees_text(ts: &[Tree]) -> String {
    let mut out = String::new();
    for (i, t) in ts.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        tree_into(t, &mut out);
    }
    out
}

fn tree_into(t: &Tree, out: &mut String) {
    match t {
        Tree::Leaf(tk) => out.push_str(&leaf_text(tk)),
        Tree::Group(d, inner, _) => {
            let (o, c) = match d {
                Delim::Paren => ("(", ")"),
                Delim::Bracket => ("[", "]"),
                Delim::Brace => ("{", "}"),
            };
            out.push_str(o);
            for (i, x) in inner.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                tree_into(x, out);
            }
            out.push_str(c);
        }
    }
}

fn leaf_text(tk: &Tok) -> String {
    if !tk.text.is_empty() {
        return tk.text.clone();
    }
    match &tk.kind {
        Kind::Ident(s) => s.clone(),
        Kind::Keyword(k) => k.text().to_string(),
        Kind::Punct(p) => punct_text(*p).to_string(),
        Kind::Lifetime(l) => format!("'{l}"),
        Kind::Lit(l) => lit_text(l),
        _ => String::new(),
    }
}

fn lit_text(l: &Lit) -> String {
    match l {
        Lit::Int(i) => {
            let pre = match i.radix {
                crate::token::Radix::Hex => "0x",
                crate::token::Radix::Oct => "0o",
                crate::token::Radix::Bin => "0b",
                crate::token::Radix::Dec => "",
            };
            format!("{pre}{}{}", i.digits, i.suffix.clone().unwrap_or_default())
        }
        Lit::Float(f) => format!(
            "{}{}{}",
            f.mantissa,
            f.exp.clone().map(|e| format!("e{e}")).unwrap_or_default(),
            f.suffix.clone().unwrap_or_default()
        ),
        Lit::Str(s) => format!("\"{}\"", escape_minimal(s)),
        Lit::ByteStr(bs) => format!(
            "b\"{}\"",
            bs.iter().map(|b| b.escape_ascii().collect::<String>()).collect::<String>()
        ),
        Lit::Cord(s) => format!("c\"{}\"", escape_minimal(s)),
        Lit::Char(c) => format!("'{}'", c.escape_debug()),
        Lit::Bool(b) => b.to_string(),
    }
}

/// Re-escape a string *value* only when the source bytes are unavailable; §9.5 rule 9 forbids
/// rewriting a literal that can be taken from the source, so this path is a fallback.
fn escape_minimal(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{{{:X}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Collapse whitespace runs outside literals, keeping comments and literal bytes intact. Used for
/// macro arms, whose interior is token soup rather than structured AST (§9.5 rules 4/9).
fn normalise_ws_outside_literals(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let mut in_line = false;
    while let Some(c) = chars.next() {
        if in_line {
            out.push(c);
            if c == '\n' {
                in_line = false;
            }
            continue;
        }
        match c {
            '/' if chars.peek() == Some(&'/') => {
                in_line = true;
                out.push('/');
                out.push('/');
            }
            '"' | '\'' => {
                out.push(c);
                let quote = c;
                let mut esc = false;
                while let Some(x) = chars.next() {
                    out.push(x);
                    if esc {
                        esc = false;
                        continue;
                    }
                    if x == '\\' {
                        esc = true;
                        continue;
                    }
                    if x == quote {
                        break;
                    }
                }
            }
            c if c.is_whitespace() => {
                while chars.peek().map(|x| x.is_whitespace() && *x != '\n').unwrap_or(false) {
                    chars.next();
                }
                out.push(if c == '\n' { ' ' } else { c });
            }
            c => out.push(c),
        }
    }
    out
}

// --------------------------------------------------------------------------------------------
// §9.5 rule 13: the AST fingerprint. Comment-position metadata is excluded by construction — the
// digest reads only node shapes and the values the parser stored.
// --------------------------------------------------------------------------------------------

pub fn ast_fingerprint(f: &ParsedFile) -> String {
    let mut s = String::new();
    s.push_str("FILE");
    s.push_str(&format!("shebang={}", f.shebang.is_some()));
    s.push_str(&format!("build={}", f.build_constraint.clone().unwrap_or_default()));
    s.push_str(&format!("pkg={}", f.package.clone().unwrap_or_default()));
    s.push_str(&format!("fattr={}", f.file_attrs.len()));
    for a in &f.file_attrs {
        fp_attr(a, &mut s);
    }
    s.push_str(&format!("imports{}", f.imports.len()));
    for i in &f.imports {
        s.push_str(&format!("[pub={}path={}alias={}]", i.pub_, i.path, fp_alias(&i.alias)));
    }
    s.push_str("items");
    for it in &f.items {
        fp_item(it, &mut s);
    }
    s
}

fn fp_alias(a: &ImportAlias) -> String {
    match a {
        ImportAlias::None => "-".to_string(),
        ImportAlias::Named(x) => format!("as{x}"),
        ImportAlias::LinkOnly => "as.".to_string(),
        ImportAlias::EffectOnly => "as_".to_string(),
    }
}

fn fp_attr(a: &Attr, s: &mut String) {
    s.push_str("<attr(");
    s.push_str(if a.inner { "!" } else { "" });
    s.push_str(&a.path.join("::"));
    for arg in &a.args {
        s.push_str(&format!(
            " {}={}",
            arg.name.clone().unwrap_or_default(),
            arg.value.as_ref().map(|v| fp_value(v)).unwrap_or_default()
        ));
    }
    for p in &a.positional {
        s.push_str(&format!(" ?{}", fp_value(p)));
    }
    s.push_str(")>");
}

fn fp_value(v: &AttrValue) -> String {
    match v {
        AttrValue::Str(x) => format!("s{x}"),
        AttrValue::Int(x) => format!("i{x}"),
        AttrValue::Bool(x) => format!("b{x}"),
        AttrValue::Expr(e) => {
            let mut s = String::new();
            fp_expr(e, &mut s);
            s
        }
        AttrValue::Idents(xs) => format!("ids{}", xs.join("+")),
        AttrValue::Path(xs) => format!("p{}", xs.join("::")),
        AttrValue::Lifetime(l) => format!("l{l}"),
    }
}

fn fp_vis(v: &Visibility, s: &mut String) {
    s.push_str(match v {
        Visibility::Private => "v-",
        Visibility::Pub => "vP",
        Visibility::PubCrate => "vC",
        Visibility::PubTo(p) => return {
            s.push_str("vT");
            s.push_str(p);
            s.push(';');
        },
        Visibility::Priv => "vp",
    });
}

fn fp_bound(b: &Bound, s: &mut String) {
    s.push_str("B[");
    if b.negated {
        s.push('!');
    }
    if b.is_lifetime {
        s.push('\'');
    }
    s.push_str(&b.path.join("::"));
    for a in &b.args {
        fp_type(a, s);
    }
    s.push(']');
}

fn fp_generic(g: &GenericParam, s: &mut String) {
    s.push_str("G[");
    if g.is_lifetime {
        s.push('\'');
    }
    s.push_str(&g.name);
    for b in &g.bounds {
        fp_bound(b, s);
    }
    if let Some(d) = &g.default {
        fp_type(d, s);
    }
    s.push(']');
}

fn fp_type(t: &Type, s: &mut String) {
    match &t.kind {
        TypeKind::Prim(p) => {
            s.push_str("T:");
            s.push_str(p.text());
        }
        TypeKind::Named { path, args } => {
            s.push_str("TN[");
            s.push_str(&path.join("::"));
            for a in args {
                fp_type(a, s);
            }
            s.push(']');
        }
        TypeKind::Ref { lifetime, mutable, inner } => {
            s.push_str(&format!("TR{}{}[", if *mutable { "m" } else { "" }, lifetime.clone().unwrap_or_default()));
            fp_type(inner, s);
            s.push(']');
        }
        TypeKind::Ptr { mutable, inner } => {
            s.push_str(if *mutable { "TPm[" } else { "TPc[" });
            fp_type(inner, s);
            s.push(']');
        }
        TypeKind::Array { inner, len } => {
            s.push_str("TA[");
            fp_type(inner, s);
            fp_expr(len, s);
            s.push(']');
        }
        TypeKind::Slice { inner } => {
            s.push_str("Ts[");
            fp_type(inner, s);
            s.push(']');
        }
        TypeKind::Dyn { path, bounds } => {
            s.push_str("TD[");
            s.push_str(&path.join("::"));
            for b in bounds {
                fp_bound(b, s);
            }
            s.push(']');
        }
        TypeKind::FnPtr { params, ret } => {
            s.push_str("TF[");
            for p in params {
                fp_type(p, s);
            }
            if let Some(r) = ret {
                fp_type(r, s);
            }
            s.push(']');
        }
        TypeKind::Tuple(ts) => {
            s.push_str("TT[");
            for t in ts {
                fp_type(t, s);
            }
            s.push(']');
        }
        TypeKind::Never => s.push_str("Tn"),
        TypeKind::Invalid => s.push_str("T!"),
    }
}

fn fp_lit(l: &Lit, s: &mut String) {
    match l {
        Lit::Int(i) => s.push_str(&format!("Li{}/{}/{:?}", i.digits, i.suffix.clone().unwrap_or_default(), i.value)),
        Lit::Float(f) => s.push_str(&format!(
            "Lf{}/{}/{}",
            f.mantissa,
            f.exp.clone().unwrap_or_default(),
            f.suffix.clone().unwrap_or_default()
        )),
        Lit::Str(x) => s.push_str(&format!("Ls{x}")),
        Lit::ByteStr(b) => s.push_str(&format!("Lb{:?}", b)),
        Lit::Cord(x) => s.push_str(&format!("Lc{x}")),
        Lit::Char(c) => s.push_str(&format!("LC{c}")),
        Lit::Bool(b) => s.push_str(&format!("LB{b}")),
    }
}

fn fp_expr(e: &Expr, s: &mut String) {
    s.push('{');
    match &e.kind {
        ExprKind::Invalid => s.push_str("invalid"),
        ExprKind::Lit(l) => {
            s.push_str("lit");
            fp_lit(l, s);
        }
        ExprKind::Int(i) => s.push_str(&format!("int{}/{}/{:?}", i.digits, i.suffix.clone().unwrap_or_default(), i.value)),
        ExprKind::Auto => s.push_str("auto"),
        // `Ident("x")` and `Path(["x"])` are the same expression: the parser rewrites one into the
        // other for turbofish calls, so the digest must not treat them as different shapes.
        ExprKind::Ident(x) => s.push_str(&format!("id{x}")),
        ExprKind::Path(p) => s.push_str(&format!("id{}", p.join("::"))),
        ExprKind::Unary { op, expr } => {
            s.push_str(&format!("un{}", punct_text(*op)));
            fp_expr(expr, s);
        }
        ExprKind::Binary { op, lhs, rhs } => {
            s.push_str(&format!("bin{}", punct_text(*op)));
            fp_expr(lhs, s);
            fp_expr(rhs, s);
        }
        ExprKind::Assign { op, target, value } => {
            s.push_str(&format!("asg{}", punct_text(*op)));
            fp_expr(target, s);
            fp_expr(value, s);
        }
        ExprKind::Call { callee, args } => {
            s.push_str("call[");
            fp_expr(callee, s);
            for a in args {
                s.push_str(&format!("n{}", a.name.clone().unwrap_or_default()));
                fp_expr(&a.expr, s);
            }
            s.push(']');
        }
        ExprKind::Method { recv, name, turbofish, args } => {
            s.push_str(&format!("meth{name}["));
            for t in turbofish {
                fp_type(t, s);
            }
            fp_expr(recv, s);
            for a in args {
                s.push_str(&format!("n{}", a.name.clone().unwrap_or_default()));
                fp_expr(&a.expr, s);
            }
            s.push(']');
        }
        ExprKind::Field { base, name } => {
            s.push_str("field[");
            fp_expr(base, s);
            match name {
                FieldName::Named(n) => s.push_str(&format!("#{n}")),
                FieldName::Index(i) => s.push_str(&format!("#{i}")),
            }
            s.push(']');
        }
        ExprKind::Index { base, index } => {
            s.push_str("idx[");
            fp_expr(base, s);
            fp_expr(index, s);
            s.push(']');
        }
        ExprKind::Tuple(items) => {
            s.push_str("tuple[");
            for i in items {
                fp_expr(i, s);
            }
            s.push(']');
        }
        ExprKind::Array(items) => {
            s.push_str("array[");
            for i in items {
                fp_expr(i, s);
            }
            s.push(']');
        }
        ExprKind::ArrayRepeat { value, len } => {
            s.push_str("rep[");
            fp_expr(value, s);
            fp_expr(len, s);
            s.push(']');
        }
        ExprKind::StructLit { path, fields, rest } => {
            s.push_str(&format!("slit[{}", path.join("::")));
            for (n, v) in fields {
                s.push_str(&format!("#{n}"));
                fp_expr(v, s);
            }
            if let Some(r) = rest {
                s.push_str("..");
                fp_expr(r, s);
            }
            s.push(']');
        }
        ExprKind::If { cond, then, else_ } => {
            s.push_str("if[");
            fp_expr(cond, s);
            fp_block(then, s);
            if let Some(e) = else_ {
                match e.as_ref() {
                    ElseBranch::Block(b) => {
                        s.push_str("eb[");
                        fp_block(b, s);
                        s.push(']');
                    }
                    ElseBranch::If(st) => {
                        s.push_str("ei[");
                        fp_stmt(st, s);
                        s.push(']');
                    }
                }
            }
            s.push(']');
        }
        ExprKind::Match { scrut, arms } => {
            s.push_str("match[");
            fp_expr(scrut, s);
            for a in arms {
                fp_pat(&a.pat, s);
                if let Some(g) = &a.guard {
                    fp_expr(g, s);
                }
                match &a.body {
                    ArmBody::Expr(x) => fp_expr(x, s),
                    ArmBody::Block(b) => fp_block(b, s),
                }
            }
            s.push(']');
        }
        ExprKind::Block(b) => {
            s.push_str("block");
            fp_block(b, s);
        }
        ExprKind::Unsafe(b) => {
            s.push_str("unsafe");
            fp_block(b, s);
        }
        ExprKind::Closure { move_, params, ret, body } => {
            s.push_str(&format!("closure{move_}["));
            for (n, t) in params {
                s.push_str(&format!("|{n}"));
                if let Some(ty) = t {
                    fp_type(ty, s);
                }
            }
            if let Some(r) = ret {
                fp_type(r, s);
            }
            fp_block(body, s);
            s.push(']');
        }
        ExprKind::Ref { mutable, lifetime, expr } => {
            s.push_str(&format!("ref{}{}[", if *mutable { "m" } else { "" }, lifetime.clone().unwrap_or_default()));
            fp_expr(expr, s);
            s.push(']');
        }
        ExprKind::Deref { expr } => {
            s.push_str("deref[");
            fp_expr(expr, s);
            s.push(']');
        }
        ExprKind::Try { expr } => {
            s.push_str("try[");
            fp_expr(expr, s);
            s.push(']');
        }
        ExprKind::Cast { expr, ty } => {
            s.push_str("cast[");
            fp_expr(expr, s);
            fp_type(ty, s);
            s.push(']');
        }
        ExprKind::Is { expr, ty } => {
            s.push_str("is[");
            fp_expr(expr, s);
            fp_type(ty, s);
            s.push(']');
        }
        ExprKind::Move { expr } => {
            s.push_str("move[");
            fp_expr(expr, s);
            s.push(']');
        }
        ExprKind::Range { lo, hi, inclusive } => {
            s.push_str(&format!("range{inclusive}["));
            if let Some(l) = lo {
                fp_expr(l, s);
            }
            if let Some(h) = hi {
                fp_expr(h, s);
            }
            s.push(']');
        }
        ExprKind::Embed(eb) => fp_embed(eb, s),
        ExprKind::MacroCall { name, delim, tokens } => {
            s.push_str(&format!("mcall{name}{}[", delim_letter(*delim)));
            fp_trees(tokens, s);
            s.push(']');
        }
    }
    s.push('}');
}

fn delim_letter(d: Delim) -> char {
    match d {
        Delim::Paren => '(',
        Delim::Bracket => '[',
        Delim::Brace => '{',
    }
}

fn fp_trees(ts: &[Tree], s: &mut String) {
    for t in ts {
        match t {
            Tree::Leaf(tk) => s.push_str(&format!("L{}", leaf_text(tk))),
            Tree::Group(d, inner, _) => {
                s.push_str(&format!("{}[", delim_letter(*d)));
                fp_trees(inner, s);
                s.push(']');
            }
        }
    }
}

fn fp_embed(eb: &EmbedBlock, s: &mut String) {
    s.push_str(&format!("embed[{};", eb.lang));
    s.push_str(&format!("body={};", eb.body));
    for a in &eb.args {
        s.push_str(&format!("{}=", a.name));
        fp_expr(&a.value, s);
    }
    s.push(']');
}

fn fp_pat(p: &Pat, s: &mut String) {
    s.push('[');
    match &p.kind {
        PatKind::Wildcard => s.push('_'),
        PatKind::DotDot => s.push_str(".."),
        PatKind::Lit(l) => fp_lit(l, s),
        PatKind::Binding { name, by_ref, mutable, sub } => {
            s.push_str(&format!("bi{by_ref}{mutable}{name}"));
            if let Some(x) = sub {
                fp_pat(x, s);
            }
        }
        PatKind::Path(path) => s.push_str(&format!("p{}", path.join("::"))),
        PatKind::Constructor { path, args, fields, has_rest } => {
            s.push_str(&format!("c{}", path.join("::")));
            if *has_rest {
                s.push_str("..");
            }
            for a in args {
                fp_pat(a, s);
            }
            for (n, x) in fields {
                s.push_str(&format!("#{n}"));
                fp_pat(x, s);
            }
        }
        PatKind::Tuple(ps) => {
            s.push_str("tup");
            for x in ps {
                fp_pat(x, s);
            }
        }
        PatKind::Slice(ps, rest) => {
            s.push_str("sli");
            for x in ps {
                fp_pat(x, s);
            }
            if let Some(r) = rest {
                s.push_str("..");
                fp_pat(r, s);
            }
        }
        PatKind::Or(ps) => {
            s.push_str("or");
            for x in ps {
                fp_pat(x, s);
            }
        }
        PatKind::Range { lo, hi, inclusive } => {
            s.push_str(&format!("rng{inclusive}"));
            fp_pat(lo, s);
            fp_pat(hi, s);
        }
        PatKind::Exclude(ps) => {
            s.push_str("exc");
            for x in ps {
                fp_pat(x, s);
            }
        }
        PatKind::Invalid => s.push_str("invalid"),
    }
    s.push(']');
}

fn fp_param(p: &Param, s: &mut String) {
    s.push_str(&format!("P{}{}{}{}", if p.mutable { "m" } else { "" }, p.name, if p.is_wildcard { "*" } else { "" }, if p.default.is_some() { "=" } else { "" }));
    fp_type(&p.ty, s);
    if let Some(d) = &p.default {
        fp_expr(d, s);
    }
}

fn fp_sig(sig: &FnSig, s: &mut String) {
    s.push_str("S[");
    s.push_str(&sig.name);
    for g in &sig.generics {
        fp_generic(g, s);
    }
    for p in &sig.params {
        fp_param(p, s);
    }
    if let Some(r) = &sig.ret {
        fp_type(r, s);
    }
    for w in &sig.where_preds {
        fp_type(&w.ty, s);
        for b in &w.bounds {
            fp_bound(b, s);
        }
    }
    s.push_str(&format!("u{}", sig.is_unsafe));
    s.push(']');
}

fn fp_body(b: &Body, s: &mut String) {
    match b {
        Body::Block(bl) => {
            s.push_str("body-block");
            fp_block(bl, s);
        }
        Body::Embed(e) => fp_embed(e, s),
        Body::Foreign => s.push_str("body-foreign"),
    }
}

fn fp_block(b: &Block, s: &mut String) {
    s.push_str("b{");
    for st in &b.stmts {
        fp_stmt(st, s);
    }
    if let Some(t) = &b.tail {
        s.push_str("tail");
        fp_expr(t, s);
    }
    s.push('}');
}

fn fp_stmt(st: &Stmt) -> (String, String);




    fn field_nodes(&mut self, fields: &[Field]) -> Vec<NodeSpec> {
        let mut out = Vec::with_capacity(fields.len());
        for f in fields {
            let mut v: Vec<Doc> = Vec::new();
            for a in &f.attrs {
                let d = self.attr_doc(a);
                if !is_empty_doc(&d) {
                    v.push(d);
                    v.push(Doc::HardLine);
                }
            }
            let pre = vis_text(&f.vis);
            v.push(Doc::Cat(vec![
                Doc::Text(format!("{pre}{}: ", f.name)),
                self.ty_doc(&f.ty),
            ]));
            out.push(NodeSpec { start: f.span.start, end: f.span.end, doc: Doc::Cat(v) });
        }
        out
    }

    fn variant_doc(&mut self, var: &Variant) -> Doc {
        let mut v: Vec<Doc> = Vec::new();
        for a in &var.attrs {
            let d = self.attr_doc(a);
            if !is_empty_doc(&d) {
                v.push(d);
                v.push(Doc::HardLine);
            }
        }
        v.push(Doc::Text(var.name.clone()));
        if !var.tuple.is_empty() {
            let items: Vec<Doc> = var
                .tuple
                .iter()
                .map(|(vis, ty)| {
                    let pre = vis_text(vis);
                    Doc::Cat(vec![Doc::Text(pre), self.ty_doc(ty)])
                })
                .collect();
            v.push(delimited("(", items, ")", INDENT, false));
        } else if !var.struct_fields.is_empty() {
            let nodes = self.field_nodes(&var.struct_fields);
            let end = var.struct_fields.last().map(|f| f.span.end).unwrap_or(var.span.end);
            let list = self.list_doc(nodes, end, ListMode::Fields);
            v.push(Doc::Cat(vec![Doc::Text(" ".to_string()), braces(list)]));
        }
        if let Some(d) = var.disc {
            v.push(Doc::Text(format!(" = {d}")));
        }
        Doc::Cat(v)
    }

    fn const_like_doc(&mut self, kw: &str, name: &str, ty: &Option<Type>, init: &Option<Expr>, _semi: bool) -> Doc {
        let mut v = vec![Doc::Text(format!("{kw} {name}"))];
        if let Some(t) = ty {
            v.push(Doc::Text(": ".to_string()));
            v.push(self.ty_doc(t));
        }
        if let Some(i) = init {
            v.push(Doc::Text(" =".to_string()));
            v.push(self.value_doc(i, 2));
        }
        v.push(Doc::Text(";".to_string()));
        Doc::Cat(v)
    }

    fn fn_like_doc(&mut self, kw: &str, sig: &FnSig, body: &Body, attrs: &[Attr]) -> Doc {
        let leading_unsafe = sig.is_unsafe
            || attrs.iter().any(|a| a.path.len() == 1 && a.path[0] == "unsafe");
        let head = if leading_unsafe { format!("unsafe {kw}") } else { kw.to_string() };
        let mut v = vec![Doc::Text(format!("{head} {}", sig.name)), self.generics_doc(&sig.generics)];
        let params: Vec<Doc> = sig.params.iter().map(|p| self.param_doc(p)).collect();
        v.push(delimited("(", params, ")", INDENT, false));
        if let Some(r) = &sig.ret {
            v.push(Doc::Text(" -> ".to_string()));
            v.push(self.ty_doc(r));
        }
        v.push(self.where_doc(&sig.where_preds));
        v.push(Doc::Text(" ".to_string()));
        match body {
            Body::Block(b) => {
                let _ = b;
                v.push(self.block_doc_ref(b));
            }
            Body::Embed(e) => v.push(self.embed_doc(e)),
            Body::Foreign => v.push(Doc::Text(";".to_string())),
        }
        Doc::Cat(v)
    }

    fn param_doc(&mut self, p: &Param) -> Doc {
        let mut s = String::new();
        if p.mutable {
            s.push_str("mut ");
        }
        if p.name == "self" {
            // Receiver shorthand (§4.14): the parser rebuilt it as a `Self` reference type.
            return self.receiver_doc(p);
        }
        s.push_str(&p.name);
        let mut v = vec![Doc::Text(s)];
        v.push(Doc::Text(": ".to_string()));
        v.push(self.ty_doc(&p.ty));
        if let Some(d) = &p.default {
            v.push(Doc::Text(" =".to_string()));
            v.push(self.value_doc(d, 2));
        }
        Doc::Cat(v)
    }

    fn receiver_doc(&mut self, p: &Param) -> Doc {
        if let TypeKind::Ref { lifetime, mutable, inner } = &p.ty.kind {
            let is_self = matches!(&inner.kind, TypeKind::Named { path, .. } if path.len() == 1 && (path[0] == "Self" || path[0] == "self"));
            if is_self {
                let lt = lifetime.as_ref().map(|l| format!(" {l}")).unwrap_or_default();
                let m = if *mutable { "mut " } else { "" };
                return Doc::Text(format!("&{lt}{m}self"));
            }
        }
        if matches!(&p.ty.kind, TypeKind::Named { path, .. } if path.len() == 1 && path[0] == "self") {
            return Doc::Text("self".to_string());
        }
        Doc::Text("self".to_string())
    }

    fn generics_doc(&mut self, gs: &[GenericParam]) -> Doc {
        if gs.is_empty() {
            return Doc::Nil;
        }
        let items: Vec<Doc> = gs
            .iter()
            .map(|g| {
                let mut s = if g.is_lifetime {
                    format!("'{}", g.name)
                } else {
                    g.name.clone()
                };
                let _ = &mut s;
                let mut v: Vec<Doc> = Vec::new();
                if g.is_lifetime {
                    v.push(Doc::Text(format!("'{}", g.name)));
                } else {
                    v.push(Doc::Text(g.name.clone()));
                }
                if !g.bounds.is_empty() {
                    v.push(Doc::Text(": ".to_string()));
                    v.push(self.bounds_doc(&g.bounds));
                }
                if let Some(d) = &g.default {
                    v.push(Doc::Text(" = ".to_string()));
                    v.push(self.ty_doc(d));
                }
                Doc::Cat(v)
            })
            .collect();
        self.angle_doc(items)
    }

    fn angle_doc(&mut self, items: Vec<Doc>) -> Doc {
        if items.is_empty() {
            return Doc::Nil;
        }
        delimited("<", items, ">", INDENT, false)
    }

    fn bounds_doc(&mut self, bs: &[Bound]) -> Doc {
        let mut v: Vec<Doc> = Vec::new();
        for (i, b) in bs.iter().enumerate() {
            if i > 0 {
                v.push(Doc::Text(" + ".to_string()));
            }
            if b.negated {
                v.push(Doc::Text("!".to_string()));
            }
            if b.is_lifetime {
                v.push(Doc::Text(format!("'{}", b.path.join("'"))));
                continue;
            }
            v.push(Doc::Text(b.path.join("::")));
            if !b.args.is_empty() {
                let items: Vec<Doc> = b.args.iter().map(|t| self.ty_doc(t)).collect();
                v.push(self.angle_doc(items));
            }
        }
        Doc::Cat(v)
    }

    /// §9.5 rule 4: a `where` clause goes one-per-line only when the signature does not fit.
    fn where_doc(&mut self, preds: &[WherePred]) -> Doc {
        if preds.is_empty() {
            return Doc::Nil;
        }
        let mut inner: Vec<Doc> = Vec::new();
        for (i, p) in preds.iter().enumerate() {
            if i > 0 {
                inner.push(Doc::Text(",".to_string()));
                inner.push(Doc::Line);
            }
            inner.push(self.ty_doc(&p.ty));
            inner.push(Doc::Text(": ".to_string()));
            inner.push(self.bounds_doc(&p.bounds));
        }
        inner.push(Doc::IfBreak(",".to_string(), String::new()));
        group(Doc::Cat(vec![
            Doc::Text(" where".to_string()),
            nest(INDENT, Doc::Cat(vec![Doc::SoftLine, Doc::Cat(inner)])),
        ]))
    }

    // ------------------------------------------------------------------ §3.6 embed blocks

    fn embed_doc(&mut self, e: &EmbedBlock) -> Doc {
        let mut v: Vec<Doc> = Vec::new();
        v.push(Doc::Text(format!("@{}", e.lang)));
        if !e.args.is_empty() {
            let items: Vec<Doc> = e
                .args
                .iter()
                .map(|a| {
                    Doc::Cat(vec![
                        Doc::Text(format!("{} = ", a.name)),
                        self.expr_doc(&a.value, 2),
                    ])
                })
                .collect();
            v.push(delimited("(", items, ")", INDENT, false));
        }
        // §2.11 + §9.5 rule 2: the body is lexically opaque. Only the block's own first line is
        // placed at the current indentation; the interior bytes are copied verbatim.
        let lines: Vec<&str> = e.body.split('\n').collect();
        let mut start = 0usize;
        while start < lines.len() && lines[start].trim().is_empty() {
            start += 1;
        }
        let mut end = lines.len();
        while end > start && lines[end - 1].trim().is_empty() {
            end -= 1;
        }
        if start >= end {
            v.push(Doc::Text(" {}".to_string()));
            return Doc::Cat(v);
        }
        // Everything from the `{` to the last body line is opaque, so the whole run is consumed and
        // can never be re-emitted as a comment.
        self.pool.mark_consumed(e.body_span.start.saturating_sub(1), e.body_span.end + 1);
        let mut body: Vec<Doc> = Vec::new();
        for l in &lines[start..end] {
            body.push(Doc::Text("\n".to_string()));
            body.push(Doc::Text(l.to_string()));
        }
        v.push(Doc::Text(" {".to_string()));
        v.push(Doc::Cat(body));
        v.push(Doc::Text("\n".to_string()));
        v.push(Doc::HardLine);
        v.push(Doc::Text("}".to_string()));
        Doc::Cat(v)
    }

    // ------------------------------------------------------------------ §3.9 macro arms (verbatim)

    fn macro_arm_doc(&mut self, a: &MacroArm) -> Doc {
        if let Some(s) = self.span_text(a.span) {
            let t = s.trim_matches(|c: char| c.is_whitespace());
            if !t.is_empty() {
                self.pool.mark_consumed(a.span.start, a.span.end);
                return Doc::Text(normalise_ws_outside_literals(t));
            }
        }
        let mut v: Vec<Doc> = Vec::new();
        v.push(Doc::Text(trees_text(&a.matcher)));
        v.push(Doc::Text(" => ".to_string()));
        v.push(Doc::Text(trees_text(&a.expander)));
        if let Some(sep) = a.separator {
            v.push(Doc::Text(punct_text(sep).to_string()));
        }
        Doc::Cat(v)
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

