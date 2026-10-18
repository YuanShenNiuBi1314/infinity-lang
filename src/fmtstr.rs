//! §10.11 `std.fmt`: the format string is parsed **at compile time**.
//!
//! This module is the only place that knows the placeholder grammar. Stage [4] (`sema`) calls
//! [`plan`] to validate a `fmt(…)` call against its arguments (`E-0180` / `E-0181`), and stage [8]
//! (`codegen`) calls the same function with the same two inputs — the literal text and the argument
//! names — to build the `InfyFmtSpec` table `infy_fmt_run` reads. One implementation on both sides
//! is what keeps "validated" and "emitted" from meaning different things.
//!
//! The accepted grammar is §10.11 plus the closed option set it lists
//! (`{:>10} {:<10} {:^10} {:0>10} {:.3} {:e} {:?} {:#?}`); anything else is refused, never ignored.
//! The choices §10.11 leaves open are recorded as deviation D-58 in `docs/spec-deviations.md`.

/// The rendering a placeholder asks for. **The enumerator names are an ABI contract**: they are
/// printed into generated C, and [`tests::the_which_names_match_the_c_header`] re-reads
/// `runtime/infy_rt.h` to prove the list matches `InfyFmtWhich`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Which {
    Auto,
    Display,
    Debug,
    DebugPretty,
    LowerHex,
    UpperHex,
    Binary,
    Octal,
    Pointer,
    Exp,
}

impl Which {
    pub const ALL: [Which; 10] = [
        Which::Auto,
        Which::Display,
        Which::Debug,
        Which::DebugPretty,
        Which::LowerHex,
        Which::UpperHex,
        Which::Binary,
        Which::Octal,
        Which::Pointer,
        Which::Exp,
    ];

    /// The enumerator name in `runtime/infy_rt.h`.
    pub fn c_enumerator(self) -> &'static str {
        match self {
            Which::Auto => "FY_AUTO",
            Which::Display => "FY_DISPLAY",
            Which::Debug => "FY_DEBUG",
            Which::DebugPretty => "FY_DEBUG_PRETTY",
            Which::LowerHex => "FY_LOWER_HEX",
            Which::UpperHex => "FY_UPPER_HEX",
            Which::Binary => "FY_BINARY",
            Which::Octal => "FY_OCTAL",
            Which::Pointer => "FY_POINTER",
            Which::Exp => "FY_EXP",
        }
    }
}

/// `INFY_FMT_TAIL_ONLY` in `runtime/infy_rt.h`: a row carrying only the literal run that follows
/// the last placeholder.
pub const TAIL_ONLY: u32 = 0xFFFF_FFFF;

/// One `InfyFmtSpec` row. `arg == None` marks the tail-only row; `width`/`precision` are `-1` when
/// unset, matching the header's `int32_t` fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub lit: String,
    pub arg: Option<usize>,
    pub which: Which,
    pub width: i32,
    pub precision: i32,
    /// Byte of the fill character, `0` = space. A non-ASCII fill is stored as `0` because
    /// `InfyFmtSpec::fill` is a `uint8_t` and cannot hold it (§10.11 does not say what to do).
    pub fill: u8,
    /// `0` = default, else the ASCII of `<`, `>` or `^`.
    pub align: u8,
    /// `{:#?}` / `{:#x}`.
    pub alt: bool,
    /// `{:0>10}` / `{:010}`.
    pub zero_pad: bool,
}

impl Spec {
    /// The C designated initialiser for one row, in the header's field names.
    pub fn c_initializer(&self) -> String {
        let arg = match self.arg {
            Some(i) => i as u32,
            None => TAIL_ONLY,
        };
        format!(
            "{{ .lit = {}, .lit_len = {}u, .arg_index = {}u, .which = {}, .width = {}, .precision = {}, .fill = {}u, .align = {}u, .alt = {}u, .zero_pad = {}u }}",
            c_string_literal(&self.lit),
            self.lit.len(),
            arg,
            self.which.c_enumerator(),
            self.width,
            self.precision,
            self.fill,
            self.align,
            self.alt as u32,
            self.zero_pad as u32,
        )
    }
}

/// Why a format string cannot be used. Every variant is `E-0180` (§10.11); the non-literal format
/// string is `E-0181`, which `sema` says out loud before it ever calls [`plan`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bad {
    /// An opening `{` with no closing `}`.
    Unterminated { ph: String },
    /// A placeholder or format option outside what §10.11 accepts.
    BadOption { ph: String, reason: String },
    /// `{}` and `{0}` in one string: §10.11 lists both but says nothing about mixing them.
    MixedIndexing { ph: String },
    /// §10.11: 位置索引越界.
    IndexOutOfRange { ph: String, index: usize, positional: usize },
    /// §10.11: `{name}` 只接受具名实参.
    NamedMissing { ph: String, name: String },
    /// §10.11: 实参未用尽.
    UnusedArg { position: usize, name: Option<String> },
}

impl Bad {
    pub fn message(&self) -> String {
        match self {
            Bad::Unterminated { ph } => format!("格式串占位符未闭合：`{ph}`"),
            Bad::BadOption { ph, reason } => format!("格式串 `{ph}` 非法：{reason}"),
            Bad::MixedIndexing { ph } => {
                format!("`{ph}` 与另一种索引形态混用（§10.11 的 `{{}}` 与 `{{0}}` 不得同串混用）")
            }
            Bad::IndexOutOfRange { ph, index, positional } => format!(
                "格式串位置索引越界：`{ph}` 要求索引 {index}，位置实参只有 {positional} 个"
            ),
            Bad::NamedMissing { ph, name } => {
                format!("具名占位符 `{ph}` 没有对应的具名实参 `{name}=`")
            }
            Bad::UnusedArg { position, name } => match name {
                Some(n) => format!("具名实参 `{n}=`（第 {position} 个）未被格式串使用（§10.11：实参未用尽）"),
                None => format!("第 {position} 个位置实参未被格式串使用（§10.11：实参未用尽）"),
            },
        }
    }
}

/// Where a placeholder points.
#[derive(Clone, Debug)]
enum Slot {
    /// `{}` — the next positional argument.
    Auto,
    /// `{0}` — that positional argument.
    Index(usize),
    /// `{name}` — the argument named `name`.
    Name(String),
}

/// A parsed placeholder, before names have been resolved to argument positions.
#[derive(Clone, Debug)]
struct Raw {
    lit: String,
    slot: Slot,
    which: Which,
    width: i32,
    precision: i32,
    fill: u8,
    align: u8,
    alt: bool,
    zero_pad: bool,
    /// The placeholder exactly as it appears in the string, braces included (for messages).
    text: String,
}

/// Parses `fmt_str` and resolves every placeholder against `arg_names` — one entry per **value**
/// argument in source order, `None` for a positional one. The format string itself is not in that
/// list. The returned rows are what stage [8] emits, in order, tail row included.
pub fn plan(fmt_str: &str, arg_names: &[Option<String>]) -> Result<Vec<Spec>, Bad> {
    let (raws, tail) = parse(fmt_str)?;
    let positional: Vec<usize> = (0..arg_names.len())
        .filter(|k| arg_names[*k].is_none())
        .collect();
    let mut used = vec![false; arg_names.len()];
    let mut auto_next = 0usize;
    let mut saw_auto = false;
    let mut saw_index = false;
    let mut specs: Vec<Spec> = Vec::new();
    for r in &raws {
        let arg = match r.slot {
            Slot::Auto => {
                saw_auto = true;
                if saw_index {
                    return Err(Bad::MixedIndexing { ph: r.text.clone() });
                }
                let i = auto_next;
                auto_next += 1;
                slot_or_range(&positional, i, &r.text)?
            }
            Slot::Index(i) => {
                saw_index = true;
                if saw_auto {
                    return Err(Bad::MixedIndexing { ph: r.text.clone() });
                }
                slot_or_range(&positional, i, &r.text)?
            }
            Slot::Name(ref name) => match arg_names.iter().position(|n| n.as_deref() == Some(name.as_str())) {
                Some(k) => Some(k),
                None => {
                    return Err(Bad::NamedMissing { ph: r.text.clone(), name: name.clone() })
                }
            },
        };
        if let Some(k) = arg {
            used[k] = true;
        }
        specs.push(Spec {
            lit: r.lit.clone(),
            arg,
            which: r.which,
            width: r.width,
            precision: r.precision,
            fill: r.fill,
            align: r.align,
            alt: r.alt,
            zero_pad: r.zero_pad,
        });
    }
    // One row is always emitted: `infy_fmt_run` takes (specs, n_specs) and an empty table with a
    // non-empty literal would drop the text.
    if !tail.is_empty() || specs.is_empty() {
        specs.push(Spec {
            lit: tail,
            arg: None,
            which: Which::Auto,
            width: -1,
            precision: -1,
            fill: 0,
            align: 0,
            alt: false,
            zero_pad: false,
        });
    }
    for (k, u) in used.iter().enumerate() {
        if !u {
            return Err(Bad::UnusedArg { position: k + 1, name: arg_names[k].clone() });
        }
    }
    Ok(specs)
}

fn slot_or_range(positional: &[usize], i: usize, ph: &str) -> Result<Option<usize>, Bad> {
    match positional.get(i) {
        Some(k) => Ok(Some(*k)),
        None => Err(Bad::IndexOutOfRange { ph: ph.to_string(), index: i, positional: positional.len() }),
    }
}

/// The C string literal for a `lit` run. `lit_len` is the byte length, so an embedded NUL stays
/// inside the run instead of truncating it.
fn c_string_literal(s: &str) -> String {
    let mut out: Vec<u8> = Vec::with_capacity(s.len() + 2);
    out.push(b'"');
    for b in s.as_bytes() {
        match *b {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            0 => out.extend_from_slice(b"\\0"),
            c if c < 0x20 || c == 0x7f => out.extend_from_slice(format!("\\x{c:02x}").as_bytes()),
            c => out.push(c),
        }
    }
    out.push(b'"');
    String::from_utf8(out).expect("every escape written above is valid UTF-8")
}

/// Scans the string left to right, returning the placeholders and the literal run after the last
/// one.
fn parse(fmt_str: &str) -> Result<(Vec<Raw>, String), Bad> {
    let chars: Vec<char> = fmt_str.chars().collect();
    let mut out: Vec<Raw> = Vec::new();
    let mut lit = String::new();
    let mut i = 0usize;
    while i < chars.len() {
        match chars[i] {
            '{' => {
                if chars.get(i + 1) == Some(&'{') {
                    lit.push('{');
                    i += 2;
                    continue;
                }
                let start = i;
                i += 1;
                let mut head = String::new();
                let mut body = String::new();
                let mut has_colon = false;
                while i < chars.len() && chars[i] != '}' {
                    if chars[i] == ':' && !has_colon {
                        has_colon = true;
                        i += 1;
                        continue;
                    }
                    if has_colon {
                        body.push(chars[i]);
                    } else {
                        head.push(chars[i]);
                    }
                    i += 1;
                }
                if i >= chars.len() {
                    return Err(Bad::Unterminated {
                        ph: chars[start..].iter().collect(),
                    });
                }
                i += 1;
                let text: String = chars[start..i].iter().collect();
                let slot = if head.is_empty() {
                    Slot::Auto
                } else if let Ok(ix) = head.parse::<usize>() {
                    Slot::Index(ix)
                } else if is_ident(&head) {
                    Slot::Name(head.clone())
                } else {
                    return Err(Bad::BadOption {
                        ph: text,
                        reason: format!("`{head}` 既不是位置索引也不是标识符"),
                    });
                };
                let (which, width, precision, fill, align, alt, zero_pad) =
                    parse_options(&body, &text)?;
                out.push(Raw {
                    lit: std::mem::take(&mut lit),
                    slot,
                    which,
                    width,
                    precision,
                    fill,
                    align,
                    alt,
                    zero_pad,
                    text,
                });
            }
            '}' => {
                if chars.get(i + 1) == Some(&'}') {
                    lit.push('}');
                    i += 2;
                } else {
                    return Err(Bad::BadOption {
                        ph: "}".to_string(),
                        reason: "孤立的 `}` 不是转义（`}}` 才是）".to_string(),
                    });
                }
            }
            c => {
                lit.push(c);
                i += 1;
            }
        }
    }
    Ok((out, lit))
}

/// `[[fill]align]['0'][width]['.'precision]['#'[type]]` over the part between `:` and `}`.
fn parse_options(body: &str, text: &str) -> Result<(Which, i32, i32, u8, u8, bool, bool), Bad> {
    let cs: Vec<char> = body.chars().collect();
    let bad = |reason: String| Bad::BadOption { ph: text.to_string(), reason };
    let mut k = 0usize;
    let mut fill = 0u8;
    let mut align = 0u8;
    let mut zero_pad = false;
    let is_align = |c: char| matches!(c, '<' | '>' | '^');
    if k < cs.len() && is_align(cs[k]) {
        align = cs[k] as u8;
        k += 1;
    } else if k + 1 < cs.len() && is_align(cs[k + 1]) && !matches!(cs[k], '.' | '#' | ':') {
        fill = if cs[k].is_ascii() { cs[k] as u8 } else { 0 };
        zero_pad = cs[k] == '0';
        align = cs[k + 1] as u8;
        k += 2;
    }
    if k < cs.len() && cs[k] == '0' && cs.get(k + 1).map_or(false, |c| c.is_ascii_digit()) {
        zero_pad = true;
        k += 1;
    }
    let mut digits = String::new();
    while k < cs.len() && cs[k].is_ascii_digit() {
        digits.push(cs[k]);
        k += 1;
    }
    let width = if digits.is_empty() {
        -1
    } else {
        digits.parse::<i32>().map_err(|_| bad("宽度超出 i32".to_string()))?
    };
    let mut precision = -1i32;
    if k < cs.len() && cs[k] == '.' {
        k += 1;
        if k >= cs.len() || !cs[k].is_ascii_digit() {
            return Err(bad("`{.` 之后必须紧跟精度数字（动态宽度 `*` 未实现）".to_string()));
        }
        let mut pd = String::new();
        while k < cs.len() && cs[k].is_ascii_digit() {
            pd.push(cs[k]);
            k += 1;
        }
        precision = pd.parse::<i32>().map_err(|_| bad("精度超出 i32".to_string()))?;
    }
    let mut alt = false;
    if k < cs.len() && cs[k] == '#' {
        alt = true;
        k += 1;
    }
    let which = match cs.get(k) {
        None => Which::Auto,
        Some('?') => {
            k += 1;
            if alt { Which::DebugPretty } else { Which::Debug }
        }
        Some('x') => {
            k += 1;
            Which::LowerHex
        }
        Some('X') => {
            k += 1;
            Which::UpperHex
        }
        Some('b') => {
            k += 1;
            Which::Binary
        }
        Some('o') => {
            k += 1;
            Which::Octal
        }
        Some('p') => {
            k += 1;
            Which::Pointer
        }
        Some('e') => {
            k += 1;
            Which::Exp
        }
        Some(c) => return Err(bad(format!("`{c}` 不在 §10.11 的格式选项集合里"))),
    };
    if k != cs.len() {
        return Err(bad("格式选项尾部有多余内容".to_string()));
    }
    if alt
        && !matches!(
            which,
            Which::DebugPretty | Which::LowerHex | Which::UpperHex | Which::Binary | Which::Octal
        )
    {
        return Err(bad("`#` 只能与 `?`/`x`/`X`/`b`/`o` 组合".to_string()));
    }
    Ok((which, width, precision, fill, align, alt, zero_pad))
}

fn is_ident(s: &str) -> bool {
    let mut it = s.chars();
    match it.next() {
        Some(c) if c.is_alphabetic() || c == '_' => it.all(|c| c.is_alphanumeric() || c == '_'),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `""` marks a positional argument; anything else is that argument's name.
    fn names(v: &[&str]) -> Vec<Option<String>> {
        v.iter()
            .map(|s| if s.is_empty() { None } else { Some((*s).to_string()) })
            .collect()
    }

    fn specs(f: &str, v: &[&str]) -> Vec<Spec> {
        plan(f, &names(v)).unwrap_or_else(|e| panic!("`{f}`: {}", e.message()))
    }

    fn err(f: &str, v: &[&str]) -> String {
        match plan(f, &names(v)) {
            Ok(p) => panic!("expected `{f}` to be rejected, got {p:?}"),
            Err(b) => b.message(),
        }
    }

    #[test]
    fn a_placeholder_carries_the_literal_run_before_it() {
        let s = specs("fetch #{}", &[""]);
        assert_eq!(s.len(), 1, "{s:?}");
        assert_eq!(s[0].lit, "fetch #");
        assert_eq!(s[0].arg, Some(0));
        assert_eq!(s[0].which, Which::Auto);
        assert_eq!((s[0].width, s[0].precision), (-1, -1));
        assert_eq!(s[0].lit.len(), 7);
    }

    #[test]
    fn a_trailing_run_becomes_one_tail_only_row() {
        let s = specs("{}!", &[""]);
        assert_eq!(s.len(), 2, "{s:?}");
        assert_eq!(s[0].arg, Some(0));
        assert_eq!(s[1].arg, None);
        assert_eq!(s[1].lit, "!");
        // a literal-only format string still gets a row, so n_specs never describes an empty table
        let s = specs("plain", &[]);
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].arg.clone(), s[0].lit.as_str()), (None, "plain"));
        // …and so does an empty one
        assert_eq!(specs("", &[]).len(), 1);
    }

    #[test]
    fn braces_escape_but_a_lone_brace_is_a_rejection() {
        let s = specs("{}{{}}", &[""]);
        assert_eq!(s.len(), 2, "{s:?}");
        assert_eq!(s[0].arg, Some(0));
        assert_eq!(s[1].lit, "{}");
        assert_eq!(specs("a}}b", &[])[0].lit, "a}b");
        assert!(err("a}b", &[]).contains("孤立的 `}`"), "{}", err("a}b", &[]));
    }

    #[test]
    fn explicit_indexes_count_positional_arguments_only() {
        let s = specs("{1}-{0}", &["", ""]);
        assert_eq!(vec![s[0].arg, s[1].arg], vec![Some(1), Some(0)]);
        // a named argument is not a positional slot: `{0}` is the first *positional* one
        let s = specs("{0}-{w}", &["w", ""]);
        assert_eq!(vec![s[0].arg, s[1].arg], vec![Some(1), Some(0)], "{s:?}");
        assert!(err("{1}", &[""]).contains("越界"), "{}", err("{1}", &[""]));
        assert!(err("{}", &[]).contains("越界"), "{}", err("{}", &[]));
        assert!(err("{}{0}", &["", ""]).contains("混用"), "{}", err("{}{0}", &["", ""]));
        assert!(err("{0}{}", &["", ""]).contains("混用"), "{}", err("{0}{}", &["", ""]));
    }

    #[test]
    fn named_placeholders_resolve_to_named_arguments() {
        let s = specs("{w}x{h}", &["w", "h"]);
        assert_eq!(vec![s[0].arg, s[1].arg], vec![Some(0), Some(1)]);
        assert_eq!((s[0].lit.as_str(), s[1].lit.as_str()), ("", "x"));
        assert!(err("{w}", &[""]).contains("具名实参"), "{}", err("{w}", &[""]));
        assert!(err("{w}", &["h"]).contains("`w=`"), "{}", err("{w}", &["h"]));
    }

    #[test]
    fn an_unused_argument_is_a_rejection() {
        assert!(err("{}", &["", ""]).contains("第 2 个"), "{}", err("{}", &["", ""]));
        assert!(err("{}", &["", "w"]).contains("`w=`"), "{}", err("{}", &["", "w"]));
    }

    #[test]
    fn the_closed_option_set_parses() {
        let one = |f: &str| {
            let s = specs(f, &[""]);
            assert_eq!(s.len(), 1, "{f}: {s:?}");
            s[0].clone()
        };
        let sp = one("{:>10}");
        assert_eq!((sp.align, sp.width, sp.fill), (b'>', 10, 0));
        assert_eq!((one("{:<3}").align, one("{:<3}").width), (b'<', 3));
        let sp = one("{:*^7}");
        assert_eq!((sp.align, sp.fill, sp.width, sp.zero_pad), (b'^', b'*', 7, false));
        let sp = one("{:0>10}");
        assert_eq!((sp.zero_pad, sp.align, sp.width, sp.fill), (true, b'>', 10, b'0'));
        let sp = one("{:010}");
        assert_eq!((sp.zero_pad, sp.width, sp.align), (true, 10, 0));
        let sp = one("{:.3}");
        assert_eq!((sp.precision, sp.width), (3, -1));
        assert_eq!(one("{:e}").which, Which::Exp);
        assert_eq!((one("{:?}").which, one("{:?}").alt), (Which::Debug, false));
        assert_eq!((one("{:#?}").which, one("{:#?}").alt), (Which::DebugPretty, true));
        for (f, w) in [
            ("{:x}", Which::LowerHex),
            ("{:X}", Which::UpperHex),
            ("{:b}", Which::Binary),
            ("{:o}", Which::Octal),
            ("{:p}", Which::Pointer),
            ("{:#x}", Which::LowerHex),
        ] {
            assert_eq!(one(f).which, w, "{f}");
        }
        // a multi-byte fill cannot live in `uint8_t fill`: it is stored as 0 = space (D-58(e))
        let sp = one("{:—^5}");
        assert_eq!((sp.fill, sp.align, sp.width), (0, b'^', 5));
        assert_eq!(one("{:e>4}").fill, b'e');
        assert!(err("{—^5}", &[]).contains("既不是位置索引"), "{}", err("{—^5}", &[]));
    }

    #[test]
    fn anything_outside_the_closed_set_is_refused_not_ignored() {
        for f in [
            "{:d}", "{:s}", "{:q}", "{:%}", "{:/}", "{:.}", "{: }", "{:x }", "{:#}",
            "{:#e}", "{:#p}", "{:10000000000}", "{:.10000000000}", "{:..3}", "{:x#}",
        ] {
            let e = plan(f, &names(&[""])).err().unwrap_or_else(|| panic!("`{f}` was accepted"));
            assert!(!e.message().is_empty(), "{f}");
        }
        // an empty format spec is the same placeholder as `{}` (§10.11 lists neither as an error)
        assert_eq!(specs("{:}", &[""])[0].arg, Some(0));
        assert!(plan("{", &names(&[])).err().unwrap().message().contains("未闭合"));
        assert!(plan("{:x", &names(&[])).err().unwrap().message().contains("未闭合"));
    }

    #[test]
    fn the_c_initializer_names_the_header_fields_and_the_tail_sentinel() {
        let s = specs("!{}", &[""]).pop().unwrap();
        let t = s.c_initializer();
        assert_eq!(
            t,
            "{ .lit = \"!\", .lit_len = 1u, .arg_index = 0u, .which = FY_AUTO, .width = -1, .precision = -1, .fill = 0u, .align = 0u, .alt = 0u, .zero_pad = 0u }"
        );
        let tail = Spec {
            lit: "z".into(),
            arg: None,
            which: Which::Auto,
            width: -1,
            precision: -1,
            fill: 0,
            align: 0,
            alt: false,
            zero_pad: false,
        };
        assert!(tail.c_initializer().contains(".arg_index = 4294967295u"), "{}", tail.c_initializer());
        assert_eq!(c_string_literal("a\u{1}b"), "\"a\\x01b\"");
        assert_eq!(c_string_literal("q\"\\n"), "\"q\\\"\\\\n\"");
        // a non-ASCII run is copied as its own UTF-8 bytes, so `lit_len` matches what C sees
        assert_eq!(c_string_literal("a\u{e6}"), "\"a\u{e6}\"");
        assert_eq!(c_string_literal("a\u{e6}").len() - 2, "a\u{e6}".len());
    }

    /// The ABI contract: rows are printed by enumerator name, so a reordered `InfyFmtWhich` must go
    /// red here rather than silently changing a generated artifact.
    #[test]
    fn the_which_names_match_the_c_header() {
        let h = include_str!("../runtime/infy_rt.h");
        let end = h.find("} InfyFmtWhich").expect("InfyFmtWhich end");
        let start = h[..end].rfind("typedef enum {").expect("InfyFmtWhich block");
        let block = &h[start..end];
        let flat = block.replace('\n', " ");
        let seen: Vec<&str> = flat
            .split('{')
            .nth(1)
            .unwrap()
            .split(',')
            .map(|p| p.trim().split('=').next().unwrap().trim())
            .filter(|t| t.starts_with("FY_"))
            .collect();
        let mine: Vec<&str> = Which::ALL.iter().map(|w| w.c_enumerator()).collect();
        assert_eq!(seen, mine, "runtime/infy_rt.h 的 InfyFmtWhich 与 fmtstr::Which 不同源");
        assert!(h.contains("#define INFY_FMT_TAIL_ONLY 0xFFFFFFFFu"), "tail sentinel moved");
        assert_eq!(TAIL_ONLY, u32::MAX);
    }
}
