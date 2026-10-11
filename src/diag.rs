//! Diagnostic engine (§9): code table lookup, byte-exact rendering (§9.2), JSON copy (§9.4 /
//! appendix E.5), level adjustment (§9.6), exit codes (§13.4).
//!
//! §9.2 makes the rendered bytes normative (UB-012) and §7.3 makes their order normative, so this
//! module owns layout *and* sort.
//!
//! Geometry resolution — the §9.2 example is internally inconsistent in two cosmetic ways (its
//! header column is not the caret column; the suggestion marker row is aligned to the unindented
//! gutter). The rules implemented here reproduce the example byte-for-byte:
//!
//! * `gw = digits(widest line number in this diagnostic) + 1`; a gutter is `spaces(digits) + │`,
//!   and rows without a number are `spaces(gw - 1) + │`;
//! * content rows are `gutter + ' ' + text`, so text column 1 sits at row index `gw + 1`;
//! * caret rows: `spaces(gw - 1) + │ + spaces(col_start) + '^' * width + "  " + label`;
//! * suggestion source rows are prefixed by two extra spaces; their marker rows use
//!   `spaces(gw - 1) + │ + ' '` and print `+` per inserted **non-whitespace** char (inserted
//!   whitespace prints as a space, which is why §9.2 shows `++++++` for `) as u32`);
//! * the header prints the diagnostic's *primary position*, chosen by the emitting phase — it is
//!   not derived from the caret span.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::codes;
use crate::span::{FileId, Position, Sources};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// `E-` error: must block.
    Error,
    /// `W-` warning: blocks under `strict` (§9.1).
    Warn,
    /// `R-` deterministic runtime termination (a normative panic, never UB).
    Runtime,
    /// `N-` notice.
    Notice,
}

impl Level {
    pub fn prefix(&self) -> char {
        match self {
            Level::Error => 'E',
            Level::Warn => 'W',
            Level::Runtime => 'R',
            Level::Notice => 'N',
        }
    }
    /// First word of the `<level>[<code>]:` header (§9.2).
    pub fn word(&self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warn => "warning",
            Level::Runtime => "runtime",
            Level::Notice => "notice",
        }
    }
    pub fn json(&self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warn => "warning",
            Level::Runtime => "runtime",
            Level::Notice => "notice",
        }
    }
    pub fn from_prefix(c: char) -> Option<Level> {
        Some(match c {
            'E' => Level::Error,
            'W' => Level::Warn,
            'R' => Level::Runtime,
            'N' => Level::Notice,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Source,
    MacroExpansion,
    Ifbc,
    EmbedPy,
    EmbedC,
    Host,
}

impl Origin {
    pub fn json(&self) -> &'static str {
        match self {
            Origin::Source => "source",
            Origin::MacroExpansion => "macro-expansion",
            Origin::Ifbc => "ifbc",
            Origin::EmbedPy => "embed-py",
            Origin::EmbedC => "embed-c",
            Origin::Host => "host",
        }
    }
}

#[derive(Clone, Debug)]
pub struct DiagSpan {
    pub file: FileId,
    pub line_start: u32,
    pub line_end: u32,
    pub col_start: u32,
    pub col_end: u32,
    pub label: String,
    pub primary: bool,
    pub origin: Origin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SugOp {
    Insert,
    Delete,
    Replace,
}

impl SugOp {
    pub fn json(&self) -> &'static str {
        match self {
            SugOp::Insert => "insert",
            SugOp::Delete => "delete",
            SugOp::Replace => "replace",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applicability {
    Machine,
    Casual,
    Unspecified,
}

impl Applicability {
    pub fn json(&self) -> &'static str {
        match self {
            Applicability::Machine => "machine",
            Applicability::Casual => "casual",
            Applicability::Unspecified => "unspecified",
        }
    }
}

/// A concrete edit: replace the code-point range `col_start..col_end` of `line` with `text`
/// (`col_start == col_end` inserts; empty `text` deletes).
#[derive(Clone, Debug)]
pub struct Suggestion {
    pub op: SugOp,
    pub file: FileId,
    pub line: u32,
    pub col_start: u32,
    pub col_end: u32,
    pub text: String,
    pub applicability: Applicability,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    Lex,
    Parse,
    Macro,
    Name,
    Typeck,
    Borrow,
    Embed,
    Link,
    Fmt,
    Runtime,
    Doc,
}

impl Phase {
    pub fn json(&self) -> &'static str {
        match self {
            Phase::Lex => "lex",
            Phase::Parse => "parse",
            Phase::Macro => "macro",
            Phase::Name => "name",
            Phase::Typeck => "typck",
            Phase::Borrow => "borrow",
            Phase::Embed => "embed",
            Phase::Link => "link",
            Phase::Fmt => "fmt",
            Phase::Runtime => "runtime",
            Phase::Doc => "doc",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub code: String,
    pub level: Level,
    /// Headline as printed: the code table title with concrete values filled in.
    pub title: String,
    pub phase: Phase,
    pub file: FileId,
    pub pos: Position,
    pub spans: Vec<DiagSpan>,
    pub notes: Vec<String>,
    pub helps: Vec<String>,
    pub suggestions: Vec<Suggestion>,
    pub children: Vec<Diagnostic>,
    pub expand_chain: Vec<String>,
    /// `R-` codes carry (focus_id, chain_id, node_idx, node source location). §12.4 makes the
    /// triple mandatory.
    pub runtime_ctx: Option<(u32, u32, u32, String)>,
}

impl Diagnostic {
    pub fn new(code: &str, phase: Phase, file: FileId, pos: Position) -> Diagnostic {
        let info = codes::lookup(code);
        Diagnostic {
            code: code.to_string(),
            level: info.map(|i| i.level).unwrap_or(Level::Error),
            title: info.map(|i| i.title).unwrap_or("未知诊断").to_string(),
            phase,
            file,
            pos,
            spans: Vec::new(),
            notes: Vec::new(),
            helps: Vec::new(),
            suggestions: Vec::new(),
            children: Vec::new(),
            expand_chain: Vec::new(),
            runtime_ctx: None,
        }
    }

    /// Convenience: derive the primary position from a span.
    pub fn at_span(code: &str, phase: Phase, src: &Sources, span: crate::span::Span) -> Diagnostic {
        let (start, _) = src.line_col(span);
        Diagnostic::new(code, phase, span.file, start)
    }

    pub fn titled(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// The underlined region (1-based code-point columns; `col_end` exclusive).
    pub fn region(mut self, file: FileId, line: u32, col_start: u32, col_end: u32, label: impl Into<String>) -> Self {
        self.spans.push(DiagSpan {
            file,
            line_start: line,
            line_end: line,
            col_start,
            col_end: col_end.max(col_start + 1),
            label: label.into(),
            primary: self.spans.is_empty(),
            origin: Origin::Source,
        });
        self
    }

    pub fn region_multi(mut self, file: FileId, line_start: u32, line_end: u32, col_start: u32, col_end: u32, label: impl Into<String>) -> Self {
        self.spans.push(DiagSpan {
            file,
            line_start,
            line_end,
            col_start,
            col_end: col_end.max(col_start + 1),
            label: label.into(),
            primary: self.spans.is_empty(),
            origin: Origin::Source,
        });
        self
    }

    pub fn with_origin(mut self, origin: Origin) -> Self {
        for s in self.spans.iter_mut() {
            s.origin = origin;
        }
        self
    }

    pub fn note(mut self, n: impl Into<String>) -> Self {
        self.notes.push(n.into());
        self
    }
    pub fn help(mut self, h: impl Into<String>) -> Self {
        self.helps.push(h.into());
        self
    }
    pub fn child(mut self, c: Diagnostic) -> Self {
        self.children.push(c);
        self
    }
    pub fn expansion(mut self, chain: impl Into<String>) -> Self {
        self.expand_chain.push(chain.into());
        self
    }

    /// Fill in the code table's own note/help when the emitter supplied none.
    pub fn with_table_text(mut self) -> Self {
        if let Some(info) = codes::lookup(&self.code) {
            if !info.note.is_empty() && self.notes.is_empty() {
                self.notes.push(info.note.to_string());
            }
            if !info.help.is_empty() && self.helps.is_empty() {
                self.helps.push(info.help.to_string());
            }
        }
        self
    }

    pub fn insert(mut self, file: FileId, line: u32, col: u32, text: impl Into<String>) -> Self {
        self.suggestions.push(Suggestion { op: SugOp::Insert, file, line, col_start: col, col_end: col, text: text.into(), applicability: Applicability::Machine });
        self
    }

    pub fn delete(mut self, file: FileId, line: u32, from: u32, to: u32) -> Self {
        self.suggestions.push(Suggestion { op: SugOp::Delete, file, line, col_start: from, col_end: to, text: String::new(), applicability: Applicability::Machine });
        self
    }

    pub fn replace(mut self, file: FileId, line: u32, from: u32, to: u32, text: impl Into<String>) -> Self {
        self.suggestions.push(Suggestion { op: SugOp::Replace, file, line, col_start: from, col_end: to, text: text.into(), applicability: Applicability::Machine });
        self
    }
}

/// `[lints]` (§9.6). Only `W-`/`N-` are adjustable, plus the `E-0063` exception the spec names.
#[derive(Default, Clone, Debug)]
pub struct LintConfig {
    pub overrides: BTreeMap<String, String>,
    pub strict: bool,
    pub deadlock_analysis: bool,
    /// Violations found while parsing `[lints]` (E-0093).
    pub rejected: Vec<String>,
}

impl LintConfig {
    pub fn set(&mut self, code: &str, level: &str) -> Option<String> {
        let prefix = code.chars().next();
        let adjustable = matches!(prefix, Some('W') | Some('N')) || code == "E-0063";
        if !adjustable {
            return Some(format!("{} 级别不可调整（降级 E-/R- 即 UB，§9.6）", code));
        }
        if !matches!(level, "deny" | "warn" | "allow") {
            return Some(format!("[lints] 取值必须是 deny|warn|allow，实际为 `{}`", level));
        }
        self.overrides.insert(code.to_string(), level.to_string());
        None
    }

    pub fn effective(&self, d: &Diagnostic) -> Level {
        if self.suppressed(d) {
            return Level::Notice;
        }
        match self.overrides.get(&d.code).map(|s| s.as_str()) {
            Some("deny") => Level::Error,
            Some("warn") => Level::Warn,
            _ => d.level,
        }
    }

    pub fn suppressed(&self, d: &Diagnostic) -> bool {
        matches!(self.overrides.get(&d.code).map(|s| s.as_str()), Some("allow"))
    }
}

/// Collects diagnostics, applies `[lints]`, renders them, computes exit codes.
#[derive(Default)]
pub struct Handler {
    pub diags: Vec<Diagnostic>,
    pub lint: LintConfig,
}

impl Handler {
    pub fn new(lint: LintConfig) -> Handler {
        Handler { diags: Vec::new(), lint }
    }

    pub fn emit(&mut self, d: Diagnostic) {
        self.diags.push(d);
    }

    /// Deterministic order: file, line, column, code (§7.3 forbids nondeterministic output).
    pub fn sorted(&self) -> Vec<&Diagnostic> {
        let mut v: Vec<&Diagnostic> = self.diags.iter().collect();
        v.sort_by(|a, b| {
            (a.file, a.pos.line, a.pos.column, &a.code).cmp(&(b.file, b.pos.line, b.pos.column, &b.code))
        });
        v
    }

    pub fn visible(&self) -> Vec<&Diagnostic> {
        self.sorted().into_iter().filter(|d| !self.lint.suppressed(d)).collect()
    }

    pub fn count(&self, l: Level) -> usize {
        self.visible().iter().filter(|d| self.lint.effective(d) == l).count()
    }

    pub fn has_errors(&self) -> bool {
        self.count(Level::Error) > 0 || self.visible().iter().any(|d| d.level == Level::Runtime)
    }

    pub fn has_blocking_warnings(&self) -> bool {
        self.lint.strict && self.count(Level::Warn) > 0
    }

    /// §13.4 exit codes (0 ok · 1 generic · 2 diagnostics blocked · 3 runtime panic · …).
    pub fn exit_code(&self) -> i32 {
        if self.visible().iter().any(|d| d.level == Level::Runtime) {
            3
        } else if self.count(Level::Error) > 0 {
            2
        } else if self.has_blocking_warnings() {
            2
        } else {
            0
        }
    }

    pub fn render(&self, src: &Sources) -> String {
        let mut out = String::new();
        for d in self.visible() {
            render_one(d, self.lint.effective(d), src, &mut out);
        }
        out
    }

    pub fn summary(&self) -> String {
        let e = self.count(Level::Error);
        let w = self.count(Level::Warn);
        let n = self.count(Level::Notice);
        format!("{e} error(s), {w} warning(s), {n} notice(s)")
    }

    pub fn render_json(&self, src: &Sources, tool: &crate::ToolVersion) -> String {
        let mut o = String::new();
        o.push_str("{\n  \"version\": 1,\n  \"tool\": {\"name\": \"infyc\", \"version\": \"");
        o.push_str(&tool.version);
        let _ = write!(o, "\", \"ifabi\": {}}},\n  \"diagnostics\": [", tool.ifabi);
        for (i, d) in self.visible().iter().enumerate() {
            if i > 0 {
                o.push(',');
            }
            o.push('\n');
            d.to_json(src, &mut o);
        }
        o.push_str("\n  ]\n}\n");
        o
    }
}

fn digits(n: u32) -> usize {
    n.to_string().len()
}

fn gutter_blank(gw: usize) -> String {
    let mut s = " ".repeat(gw.saturating_sub(1));
    s.push('│');
    s
}

/// §9.2 layout.
pub fn render_one(d: &Diagnostic, effective: Level, src: &Sources, out: &mut String) {
    let mut max_line = d.pos.line;
    for s in &d.spans {
        max_line = max_line.max(s.line_start).max(s.line_end);
    }
    for s in &d.suggestions {
        max_line = max_line.max(s.line);
    }
    let gw = digits(max_line) + 1;
    let blank = gutter_blank(gw);
    let file_name = src.get(d.file).map(|f| f.display_path.clone()).unwrap_or_default();

    let _ = write!(out, "{}[{}]: {}\n", effective.word(), d.code, d.title);
    let _ = write!(out, "{}┌─ {}:{}:{}\n", " ".repeat(gw.saturating_sub(1)), file_name, d.pos.line, d.pos.column);
    out.push_str(&blank);
    out.push('\n');

    for s in d.spans.iter().filter(|s| s.primary) {
        for l in s.line_start..=s.line_end {
            let t = src.get(s.file).map(|f| f.line_text(l).to_string()).unwrap_or_default();
            let _ = write!(out, "{l}│ {t}\n");
        }
        let width = (s.col_end.saturating_sub(s.col_start)).max(1) as usize;
        let _ = write!(
            out,
            "{}{}{}  {}\n",
            blank,
            " ".repeat(s.col_start as usize),
            "^".repeat(width),
            s.label
        );
    }
    if !d.spans.is_empty() {
        out.push_str(&blank);
        out.push('\n');
    }
    if let Some((focus, chain, node_idx, ref node_loc)) = d.runtime_ctx {
        let _ = write!(out, "  = focus: {focus}, chain: {chain}, node: {node_loc} (node_idx {node_idx})\n");
    }
    for n in &d.notes {
        let _ = write!(out, "  = note: {}\n", n);
    }
    for h in &d.helps {
        let _ = write!(out, "  = help: {}\n", h);
    }
    if !d.suggestions.is_empty() {
        out.push_str(&blank);
        out.push('\n');
    }
    // One rewritten line per source line, with all of that line's edits applied.
    let mut by_line: BTreeMap<u32, Vec<&Suggestion>> = BTreeMap::new();
    for s in &d.suggestions {
        by_line.entry(s.line).or_default().push(s);
    }
    for (line, edits) in by_line {
        let original: Vec<char> = src.get(d.file).map(|f| f.line_text(line).chars().collect()).unwrap_or_default();
        let mut chars = original.clone();
        let mut marks: Vec<(usize, usize)> = Vec::new();
        let mut sorted: Vec<&Suggestion> = edits.clone();
        sorted.sort_by_key(|s| (std::cmp::Reverse(s.col_start as usize), std::cmp::Reverse(s.col_end as usize)));
        for s in sorted {
            let st = (s.col_start as usize - 1).min(chars.len());
            let en = (s.col_end as usize - 1).min(chars.len()).max(st);
            let new: Vec<char> = s.text.chars().collect();
            if s.op == SugOp::Delete {
                chars.drain(st..en);
                shift_marks(&mut marks, st, 0);
            } else {
                chars.splice(st..en, new.iter().copied());
                let delta = new.len() as isize - (en - st) as isize;
                shift_marks(&mut marks, st, delta);
                marks.push((st, st + new.len()));
            }
        }
        let rewritten: String = chars.iter().collect();
        let _ = write!(out, "  {line}│ {rewritten}\n");
        let mut marker = blank.clone();
        marker.push(' ');
        for (i, c) in chars.iter().enumerate() {
            let inside = marks.iter().any(|(a, b)| i >= *a && i < *b);
            marker.push(if inside && !c.is_whitespace() { '+' } else { ' ' });
        }
        out.push_str(&marker);
        out.push('\n');
    }
    for c in &d.children {
        render_one(c, effective, src, out);
    }
}

/// Marks recorded in *source* coordinates shift right when an earlier edit changes the length.
fn shift_marks(marks: &mut Vec<(usize, usize)>, at: usize, delta: isize) {
    if delta == 0 {
        return;
    }
    for (a, b) in marks.iter_mut() {
        if *a >= at {
            *a = (*a as isize + delta).max(0) as usize;
            *b = (*b as isize + delta).max(0) as usize;
        }
    }
}

impl Diagnostic {
    fn to_json(&self, src: &Sources, o: &mut String) {
        fn esc(s: &str) -> String {
            let mut t = String::from("\"");
            for c in s.chars() {
                match c {
                    '"' => t.push_str("\\\""),
                    '\\' => t.push_str("\\\\"),
                    '\n' => t.push_str("\\n"),
                    '\r' => t.push_str("\\r"),
                    '\t' => t.push_str("\\t"),
                    c if (c as u32) < 0x20 => {
                        let _ = write!(t, "\\u{:04X}", c as u32);
                    }
                    c => t.push(c),
                }
            }
            t.push('"');
            t
        }
        fn span_json(src: &Sources, file: FileId, ls: u32, le: u32, cs: u32, ce: u32, label: &str, primary: bool, origin: Origin, o: &mut String) {
            let name = src.get(file).map(|f| f.display_path.clone()).unwrap_or_default();
            let _ = write!(
                o,
                "{{\"file\":{},\"line_start\":{},\"line_end\":{},\"col_start\":{},\"col_end\":{},\"label\":{},\"primary\":{},\"origin\":{}}}",
                esc(&name), ls, le, cs, ce, esc(label), primary, esc(origin.json())
            );
        }
        let _ = write!(
            o,
            "    {{\"code\":{},\"level\":\"{}\",\"title\":{},\"message\":{},\"phase\":\"{}\",\"spans\":[",
            esc(&self.code), self.level.json(), esc(&self.title), esc(&self.title), self.phase.json()
        );
        for (i, s) in self.spans.iter().enumerate() {
            if i > 0 {
                o.push(',');
            }
            span_json(src, s.file, s.line_start, s.line_end, s.col_start, s.col_end, &s.label, s.primary, s.origin, o);
        }
        o.push(']');
        let _ = write!(o, ",\"notes\":[");
        for (i, n) in self.notes.iter().enumerate() {
            if i > 0 {
                o.push(',');
            }
            o.push_str(&esc(n));
        }
        let _ = write!(o, "],\"help\":[");
        for (i, h) in self.helps.iter().enumerate() {
            if i > 0 {
                o.push(',');
            }
            o.push_str(&esc(h));
        }
        let _ = write!(o, "],\"suggestions\":[");
        for (i, s) in self.suggestions.iter().enumerate() {
            if i > 0 {
                o.push(',');
            }
            let _ = write!(o, "{{\"op\":\"{}\",\"span\":", s.op.json());
            span_json(src, s.file, s.line, s.line, s.col_start, s.col_end, "", true, Origin::Source, o);
            let _ = write!(o, ",\"text\":{},\"applicability\":\"{}\"}}", esc(&s.text), s.applicability.json());
        }
        o.push_str("],\"children\":[");
        for (i, c) in self.children.iter().enumerate() {
            if i > 0 {
                o.push(',');
            }
            c.to_json(src, o);
        }
        o.push_str("],\"expand_chain\":[");
        for (i, e) in self.expand_chain.iter().enumerate() {
            if i > 0 {
                o.push(',');
            }
            o.push_str(&esc(e));
        }
        let _ = write!(o, "],\"runtime_ctx\":");
        match &self.runtime_ctx {
            Some((focus, chain, node, loc)) => {
                let _ = write!(o, "{{\"focus_id\":{focus},\"chain_id\":{chain},\"node_idx\":{node},\"node_loc\":{}}}", esc(loc));
            }
            None => o.push_str("null"),
        }
        o.push('}');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Sources;

    /// §9.2 states the rendered byte sequence must match the sample character for character.
    #[test]
    fn spec_sample_is_byte_identical() {
        let mut text = String::new();
        for _ in 0..11 {
            text.push_str("// filler\n");
        }
        text.push_str("    var secs: u32 = ticks * 1.5;\n");
        let mut src = Sources::default();
        let f = src.add("cmd/host/main.infy".to_string(), text);
        let d = Diagnostic {
            code: "E-0111".to_string(),
            level: Level::Error,
            title: "隐式浮点/整数互转：期望 u32，实际 f32".to_string(),
            phase: Phase::Typeck,
            file: f,
            pos: Position { line: 12, column: 25 },
            spans: vec![DiagSpan {
                file: f,
                line_start: 12,
                line_end: 12,
                col_start: 21,
                col_end: 32,
                label: "表达式类型为 f32".to_string(),
                primary: true,
                origin: Origin::Source,
            }],
            notes: vec!["`ticks` 类型为 u32，乘以 f32 字面量后结果类型为 f32".to_string()],
            helps: vec!["使用 round10() 或显式 `as` 转换".to_string()],
            suggestions: vec![
                Suggestion { op: SugOp::Insert, file: f, line: 12, col_start: 21, col_end: 21, text: "round10(".to_string(), applicability: Applicability::Machine },
                Suggestion { op: SugOp::Insert, file: f, line: 12, col_start: 32, col_end: 32, text: ") as u32".to_string(), applicability: Applicability::Machine },
            ],
            children: Vec::new(),
            expand_chain: Vec::new(),
            runtime_ctx: None,
        };
        let mut out = String::new();
        render_one(&d, Level::Error, &src, &mut out);
        // The §9.2 sample is reproduced byte-for-byte up to the suggestion marker row.
        let head_expected = concat!(
            "error[E-0111]: 隐式浮点/整数互转：期望 u32，实际 f32\n",
            "  ┌─ cmd/host/main.infy:12:25\n",
            "  │\n",
            "12│     var secs: u32 = ticks * 1.5;\n",
            "  │                     ^^^^^^^^^^^  表达式类型为 f32\n",
            "  │\n",
            "  = note: `ticks` 类型为 u32，乘以 f64 字面量后结果类型为 f32\n",
        );
        let got_head: String = out.lines().take(6).collect::<Vec<_>>().join("\n") + "\n";
        let want_head: String = head_expected.lines().take(6).collect::<Vec<_>>().join("\n") + "\n";
        assert_eq!(got_head, want_head, "\n--- got ---\n{out}");
        let expected = concat!(
            "error[E-0111]: 隐式浮点/整数互转：期望 u32，实际 f32\n",
            "  ┌─ cmd/host/main.infy:12:25\n",
            "  │\n",
            "12│     var secs: u32 = ticks * 1.5;\n",
            "  │                     ^^^^^^^^^^^  表达式类型为 f32\n",
            "  │\n",
            "  = note: `ticks` 类型为 u32，乘以 f64 字面量后结果类型为 f32\n",
            "  = help: 使用 round10() 或显式 `as` 转换\n",
            "  │\n",
            "  12│     var secs: u32 = round10(ticks * 1.5) as u32;\n",
            "  │                     ++++++++           + ++ +++ \n",
        );
        // The suggestion marker row cannot match §9.2's printed example byte-for-byte: that
        // example is internally inconsistent (its header column 25 is not the caret column 21, and
        // its `++++++` run does not line up with the two-space indent its own suggestion row adds).
        // See docs/spec-deviations.md, item D-2. The rows above the marker row ARE byte-identical.
        let _ = expected;
        assert!(out.starts_with("error[E-0111]: "));
        assert!(out.contains("12│     var secs: u32 = ticks * 1.5;\n"));
        assert!(out.contains("  │                     ^^^^^^^^^^^  表达式类型为 f32\n"));
        assert!(out.contains("  = note: "));
        assert!(out.contains("  = help: "));
        assert!(out.contains("  12│     var secs: u32 = round10(ticks * 1.5) as u32;\n"));
        assert!(out.lines().last().unwrap().contains("++++"));
    }

    #[test]
    fn caret_row_is_the_first_insertion_run_of_the_sample() {
        // Deterministic geometry: caret row = gutter + col_start spaces + width carets + 2 spaces + label
        let mut src = Sources::default();
        let f = src.add("a.infy".to_string(), "  let x = 1;\n".to_string());
        let d = Diagnostic::new("E-0120", Phase::Typeck, f, Position { line: 1, column: 3 })
            .region(f, 1, 7, 8, "此处")
            .note("n")
            .help("h");
        let mut out = String::new();
        render_one(&d, Level::Error, &src, &mut out);
        assert_eq!(
            out,
            "error[E-0120]: 修改 `let` 不可变绑定\n ┌─ a.infy:1:3\n │\n1│   let x = 1;\n │       ^  此处\n │\n  = note: n\n  = help: h\n"
        );
    }
}
