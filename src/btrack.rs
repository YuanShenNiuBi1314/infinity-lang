//! §7.7 B 轨：解释型语言 → 该语言自己的原生字节码 → 能力扫描 → 机械 lowering → IFBC。
//!
//! The whole point of v2.0's B-track is stated in §7.7.1: **INFY writes no parser for another
//! language**. Everything lexical, syntactic and semantic about Python is done by CPython itself;
//! this module only ever consumes the *instruction stream and metadata* that CPython's own compiler
//! produced (`co_code`, `co_consts`, `co_names`, `co_varnames`, `co_linetable`,
//! `co_exceptiontable`), compares it against a machine-readable capability table (§7.7.2) and maps
//! it mechanically onto IFBC (§7.8, appendix D).
//!
//! Consequences implemented here:
//!
//! * [`run_python_probe`] is the **single** place the external toolchain is invoked (§7.7.3). Its
//!   output is a JSON description of the code objects; nothing else in the crate talks to Python.
//! * [`capability_scan`] answers "can this be lowered?" before anything is emitted, and every
//!   rejection names the concrete unmapped item (§7.7.2: 按诊断列出的具体项等待能力表扩容).
//! * Unsupported constructs fail the **build**, never surprise the runtime (P3): `eval`/`exec` and
//!   friends are `E-0711`, unmapped opcodes/builtins/imports are `E-0712`, a version disagreement is
//!   `E-0713`/`E-0715`, a binary wheel is `E-0714`, `@js` without `isolation="process"` is `E-0716`
//!   and a prefix outside §7.5's whitelist is `E-0702`.
//! * §7.7.6 is honest about the cost: the front end is free, the **object model** is not. Where this
//!   implementation cannot supply Python semantics without that object model, the capability table
//!   says `"-"` and the build stops.
//!
//! Determinism (§7.3): tables are `BTreeMap`s, the string pool interns by first appearance, and
//! `module_hash` covers the normalised contents, so two runs over the same code-object dump produce
//! byte-identical images.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::ast::EmbedBlock;
use crate::codes;
use crate::diag::{Diagnostic, Handler, Origin, Phase};
use crate::span::Position;
use crate::ifbc::{
    self, CodeBuilder, ExcEntry, ExcKind, HostSig, HostSigTable, IfbcModule, ImportKind, Instr,
    Op, TyKind, F_ENTRY, NONE32,
};
use crate::pkg::Manifest;
use crate::sha256;
use crate::span::{FileId, Sources};

// ------------------------------------------------------------------ a minimal JSON reader

/// The probe speaks JSON (§7.7.1: the code objects are CPython's, the *description* is ours), and
/// the toolchain is std-only (§7.3), so a small recursive-descent reader lives here rather than in a
/// dependency. It is only ever fed output from our own probe program, but nothing is `unwrap`ed:
/// malformed input becomes [`IngestErr::ProbeOutput`].
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Int(i64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

impl Json {
    pub fn get(&self, k: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.get(k),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Int(i) => Some(*i),
            Json::Num(f) => Some(*f as i64),
            _ => None,
        }
    }
    pub fn as_u32(&self) -> Option<u32> {
        self.as_i64().and_then(|v| u32::try_from(v).ok())
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(f) => Some(*f),
            Json::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }
}

fn json_parse(text: &str) -> Result<Json, String> {
    let b = text.as_bytes();
    let mut p = 0usize;
    fn ws(b: &[u8], p: &mut usize) {
        while *p < b.len() && matches!(b[*p], b' ' | b'\t' | b'\n' | b'\r') {
            *p += 1;
        }
    }
    fn value(b: &[u8], p: &mut usize) -> Result<Json, String> {
        ws(b, p);
        if *p >= b.len() {
            return Err("JSON 提前结束".to_string());
        }
        match b[*p] {
            b'{' => {
                *p += 1;
                let mut out: BTreeMap<String, Json> = BTreeMap::new();
                loop {
                    ws(b, p);
                    if *p >= b.len() {
                        return Err("对象未闭合".to_string());
                    }
                    if b[*p] == b'}' {
                        *p += 1;
                        return Ok(Json::Obj(out));
                    }
                    if b[*p] == b',' {
                        *p += 1;
                        continue;
                    }
                    let key = string(b, p)?;
                    ws(b, p);
                    if *p >= b.len() || b[*p] != b':' {
                        return Err("对象缺少 ':'".to_string());
                    }
                    *p += 1;
                    let v = value(b, p)?;
                    out.insert(key, v);
                }
            }
            b'[' => {
                *p += 1;
                let mut out = Vec::new();
                loop {
                    ws(b, p);
                    if *p >= b.len() {
                        return Err("数组未闭合".to_string());
                    }
                    if b[*p] == b']' {
                        *p += 1;
                        return Ok(Json::Arr(out));
                    }
                    if b[*p] == b',' {
                        *p += 1;
                        continue;
                    }
                    out.push(value(b, p)?);
                }
            }
            b'"' => Ok(Json::Str(string(b, p)?)),
            b't' => lit(b, p, "true", Json::Bool(true)),
            b'f' => lit(b, p, "false", Json::Bool(false)),
            b'n' => lit(b, p, "null", Json::Null),
            c if c == b'-' || c.is_ascii_digit() => {
                let start = *p;
                if b[*p] == b'-' {
                    *p += 1;
                }
                let mut is_float = false;
                while *p < b.len() && (b[*p].is_ascii_digit() || b"-+.eE".contains(&b[*p])) {
                    if matches!(b[*p], b'.' | b'e' | b'E') {
                        is_float = true;
                    }
                    *p += 1;
                }
                let s = std::str::from_utf8(&b[start..*p]).map_err(|e| e.to_string())?;
                if is_float {
                    s.parse::<f64>().map(Json::Num).map_err(|e| e.to_string())
                } else {
                    match s.parse::<i64>() {
                        Ok(i) => Ok(Json::Int(i)),
                        Err(_) => s.parse::<f64>().map(Json::Num).map_err(|e| e.to_string()),
                    }
                }
            }
            other => Err(format!("JSON 意外字节 {other:#04x}")),
        }
    }
    fn lit(b: &[u8], p: &mut usize, word: &str, v: Json) -> Result<Json, String> {
        if b.len() >= *p + word.len() && &b[*p..*p + word.len()] == word.as_bytes() {
            *p += word.len();
            return Ok(v);
        }
        Err(format!("JSON 字面量解析失败于 {word}"))
    }
    fn hex4(b: &[u8], p: usize) -> Result<u32, String> {
        if b.len() < p + 4 {
            return Err("\\u 截断".to_string());
        }
        let s = std::str::from_utf8(&b[p..p + 4]).map_err(|e| e.to_string())?;
        u32::from_str_radix(s, 16).map_err(|e| e.to_string())
    }
    fn string(b: &[u8], p: &mut usize) -> Result<String, String> {
        if b[*p] != b'"' {
            return Err("期望字符串".to_string());
        }
        *p += 1;
        let mut out = String::new();
        while *p < b.len() {
            let c = b[*p];
            if c == b'"' {
                *p += 1;
                return Ok(out);
            }
            if c == b'\\' {
                *p += 1;
                if *p >= b.len() {
                    return Err("转义提前结束".to_string());
                }
                match b[*p] {
                    b'n' => out.push('\n'),
                    b't' => out.push('\t'),
                    b'r' => out.push('\r'),
                    b'b' => out.push('\u{8}'),
                    b'f' => out.push('\u{c}'),
                    b'/' => out.push('/'),
                    b'\\' => out.push('\\'),
                    b'"' => out.push('"'),
                    b'u' => {
                        *p += 1;
                        let hi = hex4(b, *p)?;
                        *p += 4;
                        let cp = if (0xD800..0xDC00).contains(&hi) && b.len() >= *p + 6 && b[*p] == b'\\'
                        {
                            *p += 1;
                            if b[*p] != b'u' {
                                return Err("代理对损坏".to_string());
                            }
                            *p += 1;
                            let lo = hex4(b, *p)?;
                            *p += 4;
                            0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                        } else {
                            *p += 4;
                            hi
                        };
                        out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                        continue;
                    }
                    other => return Err(format!("未知转义 \\{other}")),
                }
                *p += 1;
                continue;
            }
            // Copy whole UTF-8 sequences verbatim; the probe emits UTF-8.
            let start = *p;
            while *p < b.len() && b[*p] != b'"' && b[*p] != b'\\' {
                *p += 1;
            }
            out.push_str(std::str::from_utf8(&b[start..*p]).map_err(|e| e.to_string())?);
        }
        Err("字符串未闭合".to_string())
    }
    let v = value(b, &mut p)?;
    ws(b, &mut p);
    if p != b.len() {
        return Err(format!("JSON 尾部有 {} 个多余字节", b.len() - p));
    }
    Ok(v)
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 2);
    for i in (0..b.len()).step_by(2) {
        let hi = (b[i] as char).to_digit(16)?;
        let lo = (b[i + 1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

// ------------------------------------------------------------------ code object model

/// One Python constant, as reported by the probe. `BigInt` is a Python `int` outside the `i64`
/// window that IFBC's `Int` kind cannot hold (§7.7.6: 任意精度 int 属于对象模型成本).
#[derive(Clone, Debug, PartialEq)]
pub enum PyConst {
    Int(i64),
    BigInt(String),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    Bool(bool),
    None,
    Ellipsis,
    Tuple(Vec<PyConst>),
    Frozenset(Vec<PyConst>),
    Code(Box<CodeObj>),
    Unsupported(String),
}

/// One instruction of `co_code`, exactly as `dis.get_instructions` reports it. INFY never re-derives
/// any of this — §7.7.1 forbids writing a parser for another language.
#[derive(Clone, Debug, PartialEq)]
pub struct PyInstr {
    pub opname: String,
    pub opcode: u32,
    pub arg: Option<u32>,
    pub argrepr: String,
    /// `argval` when it is an integer (jump targets, small ints), else `None`.
    pub argval_int: Option<i64>,
    pub offset: u32,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub col: Option<u32>,
    pub end_col: Option<u32>,
    pub is_jump_target: bool,
}

/// `co_exceptiontable` entry, decoded **by CPython's own `dis` module** (§7.7.2 scan item 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PyExcEntry {
    pub start: u32,
    pub end: u32,
    pub target: u32,
    pub depth: u32,
    pub lasti: bool,
}

/// A byte range of `co_code` with its source line (`co_lines()`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineSpan {
    pub start: u32,
    pub end: u32,
    pub line: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CodeObj {
    pub name: String,
    pub argcount: u32,
    pub posonlyargcount: u32,
    pub kwonlyargcount: u32,
    pub nlocals: u32,
    pub stacksize: u32,
    pub flags: u32,
    pub firstlineno: u32,
    pub code: Vec<u8>,
    pub varnames: Vec<String>,
    pub names: Vec<String>,
    pub freevars: Vec<String>,
    pub cellvars: Vec<String>,
    pub consts: Vec<PyConst>,
    pub instrs: Vec<PyInstr>,
    pub lines: Vec<LineSpan>,
    pub exc: Vec<PyExcEntry>,
}

/// The whole probe result: the interpreter identity (§7.7.3) plus the code-object tree.
#[derive(Clone, Debug)]
pub struct CodeObjDump {
    pub python_version: String,
    pub version_info: (u32, u32, u32),
    /// 2-byte little-endian `.pyc` magic (`importlib.util.MAGIC_NUMBER[:2]`).
    pub pyc_magic: u32,
    /// Full 4-byte magic as printed by the probe, e.g. `2b0e0d0a`.
    pub pyc_magic_hex: String,
    pub unit: String,
    pub module: CodeObj,
    pub raw_json: String,
}

/// Flag bits of `co_flags` that change the frame model, hence the lowering.
pub const CO_GENERATOR: u32 = 0x20;
pub const CO_COROUTINE: u32 = 0x100;
pub const CO_ASYNC_GEN: u32 = 0x200;

impl CodeObj {
    /// `co_stacksize` is a hint; the IFBC local window is `co_nlocals`.
    pub fn local_slot(&self, name: &str) -> Option<u32> {
        self.varnames.iter().position(|v| v == name).map(|i| i as u32)
    }
    pub fn const_at(&self, idx: u32) -> Option<&PyConst> {
        self.consts.get(idx as usize)
    }
    pub fn name_at(&self, idx: u32) -> Option<&str> {
        self.names.get(idx as usize).map(|s| s.as_str())
    }
    /// The `co_names` entry an instruction's argument selects. Since CPython 3.12 the global and
    /// attribute forms pack a flag into bit 0 of that argument (`LOAD_GLOBAL` = "also call it",
    /// `LOAD_ATTR` = "bound method"), so the name index is `arg >> 1` there — measured on this host
    /// (`x.foo` → 0, `x.bar` → 2, `x.bar()` → 1). Using the packed value as an index resolves to the
    /// *neighbouring* name when there is one and to nothing otherwise, which silently drops a
    /// builtin out of scan item 2.
    pub fn operand_name(&self, i: &PyInstr) -> Option<&str> {
        let arg = i.arg?;
        let idx = match i.opname.as_str() {
            "LOAD_GLOBAL" | "LOAD_ATTR" | "STORE_ATTR" | "DELETE_ATTR" | "LOAD_METHOD" => arg >> 1,
            _ => arg,
        };
        self.name_at(idx)
    }
    pub fn instr_at(&self, offset: u32) -> Option<&PyInstr> {
        self.instrs.iter().find(|i| i.offset == offset)
    }
    /// Nested code objects, in `co_consts` order (deterministic, §7.3).
    pub fn nested(&self) -> Vec<&CodeObj> {
        let mut out = Vec::new();
        for c in &self.consts {
            if let PyConst::Code(co) = c {
                out.push(co.as_ref());
            }
        }
        out
    }
    /// Self plus every nested code object, depth-first in const order — the set §7.7.2 scan item 4
    /// demands be scanned recursively (comprehensions are inlined since PEP 709, so this is
    /// `def`/`lambda`/generator bodies and anything a decorator expanded into).
    pub fn tree(&self) -> Vec<&CodeObj> {
        let mut out = vec![self];
        for n in self.nested() {
            out.extend(n.tree());
        }
        out
    }
    /// The original-language line covering a `co_code` offset (§7.4 line mapping).
    pub fn line_at(&self, offset: u32) -> Option<u32> {
        let from_instr = self
            .instrs
            .iter()
            .find(|i| i.offset == offset)
            .and_then(|i| i.start_line);
        if from_instr.is_some() {
            return from_instr;
        }
        self.lines
            .iter()
            .find(|l| l.start <= offset && offset < l.end)
            .and_then(|l| l.line)
    }
    pub fn is_coroutine_frame(&self) -> bool {
        self.flags & (CO_GENERATOR | CO_COROUTINE | CO_ASYNC_GEN) != 0
    }
    /// Classify a `co_exceptiontable` entry the way §7.7.2 scan item 5 and appendix E.3's
    /// `[exception] entry` list do: `push` (unwind cleanup, no exception object), `setup` (handler),
    /// `with` (its handler runs `__exit__`), `bare` (anything else).
    pub fn exc_entry_kind(&self, e: &PyExcEntry) -> ExcKind {
        let body: Vec<&str> = self
            .instrs
            .iter()
            .filter(|i| i.offset >= e.target && i.offset < self.code.len() as u32)
            .map(|i| i.opname.as_str())
            .take(12)
            .collect();
        if body.contains(&"WITH_EXCEPT_START") {
            return ExcKind::With;
        }
        if e.lasti {
            return ExcKind::Handler;
        }
        if body.contains(&"RERAISE") || body.contains(&"END_FINALLY") || body.contains(&"POP_EXCEPT")
        {
            if body.contains(&"PUSH_EXC_INFO") {
                return ExcKind::Handler;
            }
            return ExcKind::Cleanup;
        }
        ExcKind::Unsupported
    }
}

impl CodeObjDump {
    /// The unit body: the wrapper function this toolchain compiled the `@py` block into (§7.7.1
    /// rule 1 turns the block into a *calling convention*, not into injected globals). When the
    /// source is not a wrapper, the module code object is the unit.
    pub fn unit_code(&self) -> &CodeObj {
        if self.module.name == "<module>" {
            for n in self.module.nested() {
                if n.name == self.unit {
                    return n;
                }
            }
        }
        &self.module
    }

    /// The declared `.pyc` magic numbers this toolchain knows, by minor version — the values in
    /// §7.7.3's version lock. `3.14` is **observed on this machine** (`python --version` →
    /// `Python 3.14.6`, `importlib.util.MAGIC_NUMBER` = `2b0e0d0a` = 3627); `3.13` is the value
    /// CPython's `importlib._bootstrap_external` table publishes for that series (`f30d0d0a` = 3571)
    /// and is *not* observed here, which is why declaring `py = "3.13"` against this interpreter is
    /// `E-0713`.
    pub fn known_magics() -> BTreeMap<&'static str, (u32, &'static str)> {
        let mut m = BTreeMap::new();
        m.insert("3.12", (3531u32, "cb0d0d0a"));
        m.insert("3.13", (3571u32, "f30d0d0a"));
        m.insert("3.14", (3627u32, "2b0e0d0a"));
        m
    }

    pub fn py_minor(&self) -> String {
        format!("{}.{}", self.version_info.0, self.version_info.1)
    }
}

// ------------------------------------------------------------------ the probe: CPython's own compiler

/// The probe program, run as `python -c <this> <unit-name>` with the embed source on stdin.
///
/// This is the one and only call into the external toolchain (§7.7.1). CPython does the lexing,
/// parsing, semantic analysis and code generation; `dis` does the decode of `co_code` and
/// `co_exceptiontable`. INFY consumes only the result, which is what keeps the promise "INFY 不实现
/// 任何外部语言的解析器" (§7.7).
pub const PROBE_PY: &str = r##"
import sys, json, dis, types, importlib.util

def positions(i):
    p = getattr(i, 'positions', None)
    if p is None:
        return {}
    return {'start_line': p.lineno, 'end_line': p.end_lineno,
            'col': p.col_offset, 'end_col': p.end_col_offset}

def code_obj(co):
    instrs = []
    for i in dis.get_instructions(co):
        d = {'opname': i.opname, 'opcode': i.opcode, 'arg': i.arg,
             'argrepr': i.argrepr, 'offset': i.offset,
             'is_jump_target': bool(getattr(i, 'is_jump_target', False))}
        av = i.argval
        d['argval_int'] = av if isinstance(av, int) and not isinstance(av, bool) else None
        d.update(positions(i))
        instrs.append(d)
    exc = []
    parser = getattr(dis, 'extract_exception_entries', None) or getattr(dis, '_parse_exception_table', None)
    if parser is not None:
        try:
            for e in parser(co):
                exc.append({'start': e[0], 'end': e[1], 'target': e[2],
                            'depth': e[3], 'lasti': bool(e[4])})
        except Exception as ex:
            exc = []
    return {
        'name': co.co_name, 'argcount': co.co_argcount,
        'posonlyargcount': co.co_posonlyargcount, 'kwonlyargcount': co.co_kwonlyargcount,
        'nlocals': co.co_nlocals, 'stacksize': co.co_stacksize, 'flags': co.co_flags,
        'firstlineno': co.co_firstlineno, 'code_hex': co.co_code.hex(),
        'varnames': list(co.co_varnames), 'names': list(co.co_names),
        'freevars': list(co.co_freevars), 'cellvars': list(co.co_cellvars),
        'consts': [const_item(c) for c in co.co_consts],
        'instructions': instrs,
        'lines': [{'start': a, 'end': b, 'line': l} for (a, b, l) in co.co_lines()],
        'exception_table': exc,
    }

def const_item(c):
    if isinstance(c, types.CodeType):
        return {'k': 'code', 'code': code_obj(c)}
    if c is None:
        return {'k': 'none'}
    if c is Ellipsis:
        return {'k': 'ellipsis'}
    if isinstance(c, bool):
        return {'k': 'bool', 'v': bool(c)}
    if isinstance(c, int):
        return {'k': 'int', 'v': c, 'fits64': -(1 << 63) <= c < (1 << 63)}
    if isinstance(c, float):
        return {'k': 'float', 'v': c, 'hex': float.hex(c)}
    if isinstance(c, str):
        return {'k': 'str', 'v': c}
    if isinstance(c, bytes):
        return {'k': 'bytes', 'v': c.hex()}
    if isinstance(c, tuple):
        return {'k': 'tuple', 'items': [const_item(x) for x in c]}
    if isinstance(c, frozenset):
        return {'k': 'frozenset', 'items': [const_item(x) for x in sorted(c, key=repr)]}
    return {'k': 'unsupported', 'type': str(type(c)), 'repr': repr(c)}

def fail(kind, msg, line):
    json.dump({'error': {'kind': kind, 'message': msg, 'line': line}}, sys.stdout)
    sys.stdout.flush()
    sys.exit(4)

name = sys.argv[1] if len(sys.argv) > 1 else '<embed>'
try:
    src = sys.stdin.buffer.read().decode('utf-8')
except Exception as ex:
    fail('decode', repr(ex), None)
try:
    co = compile(src, name, 'exec')
except SyntaxError as ex:
    fail('SyntaxError', str(ex), ex.lineno)
except ValueError as ex:
    fail('ValueError', str(ex), None)
except Exception as ex:
    fail(type(ex).__name__, repr(ex), None)
out = {
    'python_version': sys.version.split()[0],
    'version_info': list(sys.version_info[:3]),
    'hexversion': sys.hexversion,
    'pyc_magic': importlib.util.MAGIC_NUMBER.hex(),
    'pyc_magic_int': int.from_bytes(importlib.util.MAGIC_NUMBER[:2], 'little'),
    'unit': name,
    'code': code_obj(co),
}
sys.stdout.buffer.write(json.dumps(out).encode('utf-8'))
"##;

/// Errors of the ingest pipeline. Every variant is a *build-time* failure: nothing here can be
/// deferred to the runtime (§7.7.2, P3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IngestErr {
    /// No interpreter registered for the declared language (§7.7.3, §7.6.2).
    ToolchainMissing { lang: String, declared: String },
    /// The probe could not be started, or died without a report.
    Probe { message: String },
    /// CPython's own compiler rejected the source (`compile()` raised).
    SourceRejected { kind: String, message: String, line: Option<u32> },
    /// The probe answered, but the answer is not the shape we expect.
    ProbeOutput(String),
    /// A capability table file is missing or malformed (appendix E.3).
    CapTable(String),
    /// Capability scan failed (§7.7.2). Carries every violation, each naming the concrete item.
    Scan(Vec<ScanViolation>),
    /// `E-0713`: interpreter or `.pyc` magic disagrees with `[b_track]` (§7.7.3).
    VersionMismatch { declared: String, observed: String, magic_declared: u32, magic_observed: u32 },
    /// `E-0715`: `[b_track]` has no entry for the language in use (§7.7.3).
    VersionUndeclared { lang: String },
    /// `E-0714`: a wheel is not pure Python (§7.7.4).
    BinaryWheel { tag: String, offending: Vec<String> },
    /// The wheel archive itself is unreadable — including a deflate stream this reader cannot handle.
    Zip(String),
    /// Writing the `.infib` failed.
    Io(String),
}

impl IngestErr {
    /// The §9.3 code this failure reports, if it has one.
    pub fn code(&self) -> Option<&'static str> {
        Some(match self {
            // §7.1's stage note: `[6a]` under `infy check` **needs** the source-language toolchain
            // present to run the capability scan, and its absence is the §7.7.3 version code.
            IngestErr::ToolchainMissing { .. } => "E-0713",
            IngestErr::VersionMismatch { .. } => "E-0713",
            IngestErr::VersionUndeclared { .. } => "E-0715",
            IngestErr::BinaryWheel { .. } => "E-0714",
            IngestErr::Scan(v) => return v.first().map(|x| x.code),
            _ => return None,
        })
    }

    pub fn title(&self) -> String {
        match self {
            IngestErr::ToolchainMissing { lang, declared } => {
                format!("未找到 {lang} 工具链（清单声明 {declared}），无法在构建期取得原生字节码")
            }
            IngestErr::Probe { message } => format!("字节码摄取探针失败：{message}"),
            IngestErr::SourceRejected { kind, message, .. } => {
                format!("源语言编译器拒绝该嵌入体：{kind}: {message}")
            }
            IngestErr::ProbeOutput(m) => format!("字节码探针输出无法解析：{m}"),
            IngestErr::CapTable(m) => format!("能力表无法读取：{m}"),
            IngestErr::Scan(v) => match codes::lookup("E-0712") {
                Some(i) => format!("{}：{} 项未覆盖", i.title, v.len()),
                None => format!("字节码能力缺失：{} 项", v.len()),
            },
            IngestErr::VersionMismatch { declared, observed, .. } => format!(
                "字节码版本不匹配：清单声明 py = \"{declared}\"，实际解释器为 {observed}"
            ),
            IngestErr::VersionUndeclared { lang } => {
                format!("使用 @{lang} 但清单没有 [{lang} 的] b_track 版本声明")
            }
            IngestErr::BinaryWheel { tag, .. } => {
                format!("wheel 含原生二进制（tag = {tag}），不属于纯字节码生态")
            }
            IngestErr::Zip(m) => format!("wheel 归档读取失败：{m}"),
            IngestErr::Io(m) => format!("产物写入失败：{m}"),
        }
    }

    pub fn note(&self) -> String {
        match self {
            IngestErr::SourceRejected { line, .. } => match line {
                Some(l) => format!("CPython 在第 {l} 行报告语法错误；INFY 不重复解析该语言（§7.7.1）"),
                None => "错误由源语言自身的编译器给出（§7.7.2：指令语义由其定型）".to_string(),
            },
            IngestErr::Scan(v) => v
                .iter()
                .map(|x| format!("{}：{}", x.code, x.item))
                .collect::<Vec<_>>()
                .join(" · "),
            IngestErr::BinaryWheel { offending, .. } => {
                format!("二进制成员：{}", offending.join(", "))
            }
            IngestErr::VersionMismatch { magic_declared, magic_observed, .. } => format!(
                ".pyc magic 期望 {magic_declared}，实际 {magic_observed}（附录 §7.7.3：magic 必须等于声明版本的期望值）"
            ),
            _ => String::new(),
        }
    }

    pub fn help(&self) -> String {
        match self {
            IngestErr::ToolchainMissing { .. } => {
                "infy toolchain add py --path <python.exe> --version <声明版本>（§7.6.2）".to_string()
            }
            IngestErr::Scan(_) => "按诊断列出的具体项等待能力表扩容，或改写该处（§7.7.2）".to_string(),
            IngestErr::VersionMismatch { declared, .. } => format!(
                "infy toolchain add py --version {declared}.x，或把清单 [b_track] 改为实际版本"
            ),
            IngestErr::VersionUndeclared { lang } => {
                format!("在 MODULE.infy.toml 写 [b_track] {lang} = \"<minor>\"（§7.7.3）")
            }
            IngestErr::BinaryWheel { .. } => {
                "改走 A 轨 @c 链接其 C API，或 @py(isolation=\"process\") 逃生舱（§7.7.4）".to_string()
            }
            IngestErr::SourceRejected { .. } => {
                "改写嵌入体；INFY 只接受源语言工具链能编译的构造".to_string()
            }
            _ => String::new(),
        }
    }

    pub fn diagnostic(&self, file: FileId, pos: Position) -> Diagnostic {
        let code = self.code().unwrap_or("E-0712");
        let d = Diagnostic::new(code, Phase::Embed, file, pos)
            .titled(self.title())
            .with_origin(Origin::EmbedPy);
        let d = if self.note().is_empty() { d } else { d.note(self.note()) };
        let d = if self.help().is_empty() { d } else { d.help(self.help()) };
        if let IngestErr::Scan(v) = self {
            return v.iter().skip(1).fold(d, |acc, x| {
                acc.child(x.diagnostic(file, pos))
            });
        }
        d
    }
}

impl std::fmt::Display for IngestErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.title())
    }
}

impl std::error::Error for IngestErr {}

// ------------------------------------------------------------------ probe plumbing

/// Locate the interpreter. §7.7.3 keeps a registry (`infy toolchain add py --path …`); the
/// environment gives it here so CI without Python can be skipped explicitly rather than silently.
pub fn find_python() -> Option<PathBuf> {
    if let Some(p) = crate::env_str("INFY_PYTHON") {
        let pb = PathBuf::from(&p);
        if pb.exists() {
            return Some(pb);
        }
    }
    for cand in ["python", "python3"] {
        if probe_version(Path::new(cand)).is_some() {
            return Some(PathBuf::from(cand));
        }
    }
    None
}

pub fn probe_version(python: &Path) -> Option<String> {
    let out = Command::new(python)
        .arg("-E")
        .arg("-c")
        .arg("import sys;sys.stdout.write(sys.version.split()[0])")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        return None;
    }
    Some(s)
}

/// Turn an `@py` block body into a compilation unit.
///
/// §7.7.1 rule 1 says the bindings are done at compile time and the calling convention is the
/// unit's parameter table — so the body is compiled as the *body of a function*, which is also what
/// makes Python's bare `return` legal. Nothing here parses Python: the text is indented verbatim and
/// `compile()` decides what it means.
pub fn wrap_embed_unit(body: &str, params: &[&str], unit_name: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out = String::from("def ");
    out.push_str(unit_name);
    out.push('(');
    out.push_str(&params.join(", "));
    out.push_str("):\n");
    for l in &lines {
        if l.trim().is_empty() {
            out.push('\n');
        } else {
            let cut = indent.min(l.len() - l.trim_start().len());
            out.push_str("    ");
            out.push_str(&l[cut..]);
            out.push('\n');
        }
    }
    out
}

/// §7.7.1: invoke the installed CPython, let **its** compiler produce the code objects, and read
/// back their description. This is the only external-toolchain call site in the crate.
pub fn run_python_probe(python: &Path, src: &str) -> Result<CodeObjDump, IngestErr> {
    run_python_probe_named(python, src, "__infy_unit__")
}

pub fn run_python_probe_named(python: &Path, src: &str, unit: &str) -> Result<CodeObjDump, IngestErr> {
    let mut child = Command::new(python)
        .arg("-E")
        .arg("-c")
        .arg(PROBE_PY)
        .arg(unit)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| IngestErr::Probe {
            message: format!("无法启动 {}：{e}（§7.7.3 要求构建期存在该工具链）", python.display()),
        })?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| IngestErr::Probe { message: "探针无 stdin".to_string() })?;
        stdin
            .write_all(src.as_bytes())
            .map_err(|e| IngestErr::Probe { message: format!("写入源码失败：{e}") })?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| IngestErr::Probe { message: format!("等待探针失败：{e}") })?;
    let body = String::from_utf8_lossy(&out.stdout).to_string();
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if out.status.code() == Some(4) {
            return Err(probe_error(&body, &err));
        }
        return Err(IngestErr::Probe {
            message: format!("退出码 {:?}：{}", out.status.code(), if err.is_empty() { body } else { err }),
        });
    }
    parse_dump(&body)
}

fn probe_error(body: &str, stderr: &str) -> IngestErr {
    if let Ok(Json::Obj(m)) = json_parse(body) {
        if let Some(e) = m.get("error") {
            let kind = e.get("kind").and_then(|v| v.as_str()).unwrap_or("Error").to_string();
            let message = e.get("message").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let line = e.get("line").and_then(|v| v.as_u32());
            return IngestErr::SourceRejected { kind, message, line };
        }
    }
    IngestErr::Probe {
        message: if stderr.is_empty() { body } else { &stderr }.to_string(),
    }
}

fn parse_dump(body: &str) -> Result<CodeObjDump, IngestErr> {
    let j = json_parse(body).map_err(|e| IngestErr::ProbeOutput(e))?;
    let version_info = j
        .get("version_info")
        .and_then(|v| v.as_arr())
        .map(|a| {
            (
                a.first().and_then(|x| x.as_u32()).unwrap_or(0),
                a.get(1).and_then(|x| x.as_u32()).unwrap_or(0),
                a.get(2).and_then(|x| x.as_u32()).unwrap_or(0),
            )
        })
        .ok_or_else(|| IngestErr::ProbeOutput("缺 version_info".to_string()))?;
    let module = j
        .get("code")
        .map(parse_code_obj)
        .transpose()?
        .ok_or_else(|| IngestErr::ProbeOutput("缺 code".to_string()))?;
    Ok(CodeObjDump {
        python_version: j.get("python_version").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        version_info,
        pyc_magic: j.get("pyc_magic_int").and_then(|v| v.as_u32()).unwrap_or(0),
        pyc_magic_hex: j.get("pyc_magic").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        unit: j.get("unit").and_then(|v| v.as_str()).unwrap_or("__infy_unit__").to_string(),
        module,
        raw_json: body.to_string(),
    })
}

fn parse_code_obj(j: &Json) -> Result<CodeObj, IngestErr> {
    let g = |k: &str| j.get(k);
    let num = |k: &str| g(k).and_then(|v| v.as_u32()).unwrap_or(0);
    let strs = |k: &str| -> Vec<String> {
        g(k)
            .and_then(|v| v.as_arr())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default()
    };
    let code_hex = g("code_hex").and_then(|v| v.as_str()).unwrap_or("");
    let code = unhex(code_hex).ok_or_else(|| IngestErr::ProbeOutput("code_hex 非法".to_string()))?;
    let mut consts = Vec::new();
    if let Some(items) = g("consts").and_then(|v| v.as_arr()) {
        for c in items {
            consts.push(parse_const(c)?);
        }
    }
    let mut instrs = Vec::new();
    if let Some(items) = g("instructions").and_then(|v| v.as_arr()) {
        for i in items {
            instrs.push(PyInstr {
                opname: i.get("opname").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                opcode: i.get("opcode").and_then(|v| v.as_u32()).unwrap_or(0),
                arg: i.get("arg").and_then(|v| v.as_u32()),
                argrepr: i.get("argrepr").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                argval_int: i.get("argval_int").and_then(|v| v.as_i64()),
                offset: i.get("offset").and_then(|v| v.as_u32()).unwrap_or(0),
                start_line: i.get("start_line").and_then(|v| v.as_u32()),
                end_line: i.get("end_line").and_then(|v| v.as_u32()),
                col: i.get("col").and_then(|v| v.as_u32()),
                end_col: i.get("end_col").and_then(|v| v.as_u32()),
                is_jump_target: i.get("is_jump_target").and_then(|v| v.as_bool()).unwrap_or(false),
            });
        }
    }
    let mut lines = Vec::new();
    if let Some(items) = g("lines").and_then(|v| v.as_arr()) {
        for l in items {
            lines.push(LineSpan {
                start: l.get("start").and_then(|v| v.as_u32()).unwrap_or(0),
                end: l.get("end").and_then(|v| v.as_u32()).unwrap_or(0),
                line: l.get("line").and_then(|v| v.as_u32()),
            });
        }
    }
    let mut exc = Vec::new();
    if let Some(items) = g("exception_table").and_then(|v| v.as_arr()) {
        for e in items {
            if let (Some(start), Some(end), Some(target)) = (
                e.get("start").and_then(|v| v.as_u32()),
                e.get("end").and_then(|v| v.as_u32()),
                e.get("target").and_then(|v| v.as_u32()),
            ) {
                exc.push(PyExcEntry {
                    start,
                    end,
                    target,
                    depth: e.get("depth").and_then(|v| v.as_u32()).unwrap_or(0),
                    lasti: e.get("lasti").and_then(|v| v.as_bool()).unwrap_or(false),
                });
            }
        }
    }
    Ok(CodeObj {
        name: g("name").and_then(|v| v.as_str()).unwrap_or("<unnamed>").to_string(),
        argcount: num("argcount"),
        posonlyargcount: num("posonlyargcount"),
        kwonlyargcount: num("kwonlyargcount"),
        nlocals: num("nlocals"),
        stacksize: num("stacksize"),
        flags: num("flags"),
        firstlineno: num("firstlineno"),
        code,
        varnames: strs("varnames"),
        names: strs("names"),
        freevars: strs("freevars"),
        cellvars: strs("cellvars"),
        consts,
        instrs,
        lines,
        exc,
    })
}

fn parse_const(j: &Json) -> Result<PyConst, IngestErr> {
    let k = j.get("k").and_then(|v| v.as_str()).unwrap_or("unsupported");
    Ok(match k {
        "none" => PyConst::None,
        "ellipsis" => PyConst::Ellipsis,
        "bool" => PyConst::Bool(j.get("v").and_then(|v| v.as_bool()).unwrap_or(false)),
        "int" => {
            let raw = j.get("v").and_then(|v| v.as_i64());
            match raw {
                Some(i) if j.get("fits64").and_then(|v| v.as_bool()).unwrap_or(true) => PyConst::Int(i),
                _ => PyConst::BigInt(
                    j.get("v")
                        .and_then(|v| match v {
                            Json::Int(i) => Some(i.to_string()),
                            Json::Str(s) => Some(s.clone()),
                            _ => None,
                        })
                        .unwrap_or_default(),
                ),
            }
        }
        "float" => PyConst::Float(j.get("v").and_then(|v| v.as_f64()).unwrap_or(0.0)),
        "str" => PyConst::Str(j.get("v").and_then(|v| v.as_str()).unwrap_or("").to_string()),
        "bytes" => PyConst::Bytes(unhex(j.get("v").and_then(|v| v.as_str()).unwrap_or("")).unwrap_or_default()),
        "tuple" => PyConst::Tuple(parse_const_items(j)?),
        "frozenset" => PyConst::Frozenset(parse_const_items(j)?),
        "code" => {
            let inner = j
                .get("code")
                .ok_or_else(|| IngestErr::ProbeOutput("嵌套 code 缺字段".to_string()))?;
            PyConst::Code(Box::new(parse_code_obj(inner)?))
        }
        other => PyConst::Unsupported(format!(
            "{}:{}",
            other,
            j.get("repr").and_then(|v| v.as_str()).unwrap_or("")
        )),
    })
}

fn parse_const_items(j: &Json) -> Result<Vec<PyConst>, IngestErr> {
    let mut out = Vec::new();
    if let Some(items) = j.get("items").and_then(|v| v.as_arr()) {
        for x in items {
            out.push(parse_const(x)?);
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ capability table (§7.7.2)

/// A resolved `[opcode]` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mapped {
    /// The instruction has no IFBC counterpart and is dropped by design (`RESUME`, `NOP`,
    /// `NOT_TAKEN`, `CACHE`).
    Ignore,
    /// An IFBC mnemonic, optionally with `_` where the inferred type suffix goes (`add._`).
    Mnem(String),
    /// Deliberately unmapped: the build stops with `E-0712` (§7.7.2, P3).
    Reject,
    /// A `call.host` target, written `host:<std path>` in the table: the instruction becomes a host
    /// call and the `import_table` row is created on demand (§7.7.1 item 4, §7.8.3 item 5).
    Host(String),
    /// Alternatives for one Python sub-operation, tried in order: `cmpeq._|host:std.embed.eq` says
    /// "static compare when the operand kinds prove numeric, the host comparator otherwise". Python's
    /// `==` is the rich-comparison protocol, i.e. object model (§7.7.6), so the fallback is a host call.
    Alt(Vec<Mapped>),
}

impl Mapped {
    pub fn mnemonic(&self) -> Option<&str> {
        match self {
            Mapped::Mnem(s) => Some(s.as_str()),
            Mapped::Alt(v) => v.iter().find_map(|x| x.mnemonic()),
            _ => None,
        }
    }
    /// The candidates, in declaration order (deterministic: the table's own order).
    pub fn alternatives(&self) -> Vec<&Mapped> {
        match self {
            Mapped::Alt(v) => v.iter().collect(),
            other => vec![other],
        }
    }
}

/// Either a single mapping or a mapping dispatched on the instruction's own argument — which is how
/// one Python opcode such as `BINARY_OP` names several IFBC instructions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mapping {
    Direct(Mapped),
    Dispatch { cases: BTreeMap<String, Mapped>, otherwise: Option<Mapped> },
}

impl Mapping {
    pub fn resolve(&self, key: &str) -> Mapped {
        match self {
            Mapping::Direct(m) => m.clone(),
            Mapping::Dispatch { cases, otherwise } => cases
                .get(key)
                .cloned()
                .or_else(|| otherwise.clone())
                .unwrap_or(Mapped::Reject),
        }
    }
    /// Every case key this row knows about — used by `infy ingest probe` to print the gaps.
    pub fn cases(&self) -> Vec<String> {
        match self {
            Mapping::Direct(_) => Vec::new(),
            Mapping::Dispatch { cases, .. } => cases.keys().cloned().collect(),
        }
    }
}

/// `IFBC-CAP-<lang>-<minor>.toml`, appendix E.3: the machine-readable data that **replaced** the old
/// hand-written subset list. §12.2 feeds these files straight into conformance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapTable {
    pub lang: String,
    pub format: String,
    pub cap_version: u32,
    /// `sha256:…` over the table body, §12.2 reproducibility.
    pub table_hash: String,
    /// Scan item 1. Keys are `0xNN` opcode numbers (E.3's normative form) **and** their `OPNAME`
    /// aliases; see [`CapTable::check_aliases`].
    pub opcode_map: BTreeMap<String, String>,
    /// Scan item 2: `co_names` ∩ `LOAD_GLOBAL`. `"-"` = not implemented, `host:<path>` = a
    /// `call.host` target, `_builtins` is the interpreter's own name table.
    pub builtins: BTreeMap<String, String>,
    /// Scan item 3.
    pub allow_imports: Vec<String>,
    pub deny_imports: Vec<String>,
    /// Scan item 5: the accepted `co_exceptiontable` entry classes (`push` / `setup` / `with`).
    pub exception_entries: Vec<String>,
}

/// Names whose very existence means code is built at run time, which static scanning cannot cover:
/// permanently rejected since v2.0 (§7.7.2).
pub const DYNAMIC_CODE_NAMES: &[&str] = &[
    "eval",
    "exec",
    "compile",
    "__import__",
    "getattr",
    "setattr",
    "delattr",
    "globals",
    "locals",
    "vars",
    "breakpoint",
];

impl CapTable {
    /// Parse appendix E.3's format. `toml::parse` gives the `Value`; every required field is checked
    /// and a missing one is a `CapTable` error rather than a default (§7.7.2: the table is normative).
    pub fn parse(text: &str, origin: &str) -> Result<CapTable, IngestErr> {
        let v = crate::toml::parse(text)
            .map_err(|e| IngestErr::CapTable(format!("{origin}: {e}")))?;
        let bad = |m: String| IngestErr::CapTable(format!("{origin}: {m}"));
        let cap = v.get("cap").ok_or_else(|| bad("缺 [cap] 段".to_string()))?;
        let lang = cap
            .get("lang")
            .and_then(|x| x.as_str())
            .ok_or_else(|| bad("[cap] lang 缺失".to_string()))?
            .to_string();
        let format = cap
            .get("format")
            .and_then(|x| x.as_str())
            .ok_or_else(|| bad("[cap] format 缺失".to_string()))?
            .to_string();
        let cap_version = cap
            .get("cap_version")
            .and_then(|x| x.as_int())
            .ok_or_else(|| bad("[cap] cap_version 缺失".to_string()))? as u32;
        let table_hash = cap
            .get("table_hash")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let mut opcode_map = BTreeMap::new();
        if let Some(t) = v.get("opcode").and_then(|x| x.as_table()) {
            for (k, val) in t {
                let s = val
                    .as_str()
                    .ok_or_else(|| bad(format!("[opcode] {k} 的值必须是字符串")))?
                    .to_string();
                opcode_map.insert(k.clone(), s);
            }
        } else {
            return Err(bad("缺 [opcode] 段".to_string()));
        }
        let mut builtins = BTreeMap::new();
        if let Some(t) = v.get("builtin").and_then(|x| x.as_table()) {
            for (k, val) in t {
                let s = val
                    .as_str()
                    .ok_or_else(|| bad(format!("[builtin] {k} 的值必须是字符串")))?
                    .to_string();
                builtins.insert(k.clone(), s);
            }
        }
        let str_list = |key: &str, sub: &str| -> Vec<String> {
            v.get(key)
                .and_then(|t| t.get(sub))
                .and_then(|x| x.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default()
        };
        let allow_imports = str_list("import", "allow");
        let deny_imports = str_list("import", "deny");
        let exception_entries = str_list("exception", "entry");
        Ok(CapTable {
            lang,
            format,
            cap_version,
            table_hash,
            opcode_map,
            builtins,
            allow_imports,
            deny_imports,
            exception_entries,
        })
    }

    /// The tables this toolchain ships, embedded so an offline build never looks for a resource file
    /// (§7.7.3: capability tables are released with the version).
    pub fn shipped(lang: &str, minor: &str) -> Option<(&'static str, &'static str)> {
        match (lang, minor) {
            ("py", "3.13") => Some(("IFBC-CAP-py-3.13.toml", include_str!("../capabilities/IFBC-CAP-py-3.13.toml"))),
            ("py", "3.14") => Some(("IFBC-CAP-py-3.14.toml", include_str!("../capabilities/IFBC-CAP-py-3.14.toml"))),
            ("lua", "5.4") => Some(("IFBC-CAP-lua-5.4.toml", include_str!("../capabilities/IFBC-CAP-lua-5.4.toml"))),
            _ => None,
        }
    }

    /// Load the shipped table for a declared `[b_track]` minor version.
    pub fn load(lang: &str, minor: &str) -> Result<CapTable, IngestErr> {
        let (name, text) = CapTable::shipped(lang, minor).ok_or_else(|| {
            IngestErr::CapTable(format!(
                "本工具链没有 IFBC-CAP-{lang}-{minor}.toml；该 minor 的字节码形态未随版本发布（§7.7.3）"
            ))
        })?;
        let mut t = CapTable::parse(text, name)?;
        // E.3's `table_hash` covers the table body, and the hash row itself is excluded — the same
        // canonical rule the generator that wrote the file used, on both sides.
        let canon: String = text
            .lines()
            .filter(|l| !l.trim_start().starts_with("table_hash"))
            .collect::<Vec<_>>()
            .join("\n");
        let computed = format!("sha256:{}", sha256::hex(canon.as_bytes()));
        if !t.table_hash.is_empty() && t.table_hash != computed {
            return Err(IngestErr::CapTable(format!(
                "{name}: table_hash 与实际内容不符（声明 {}，计算 {computed}）—— 能力表本身必须可复核（§12.2）",
                t.table_hash
            )));
        }
        t.table_hash = computed;
        Ok(t)
    }

    /// E.3 keys rows by **number** (`0x71`). These tables also carry the `OPNAME` spelling of the
    /// same row for auditability; the two must agree, because a table that says one thing by number
    /// and another by name is a table nobody can trust. Checked wherever a number and a name are both
    /// known, i.e. during the scan.
    pub fn check_alias_agreement(&self, opcode: u32, opname: &str) -> Result<(), IngestErr> {
        let num = self.opcode_map.get(&CapTable::numeric_key(opcode));
        let name = self.opcode_map.get(opname);
        if let (Some(a), Some(b)) = (num, name) {
            if a != b {
                return Err(IngestErr::CapTable(format!(
                    "能力表自相矛盾：{} = {a} 与 {opname} = {b}（§7.7.2 要求扫描器穷举一致）",
                    CapTable::numeric_key(opcode)
                )));
            }
        }
        Ok(())
    }

    fn numeric_key(opcode: u32) -> String {
        format!("0x{opcode:x}")
    }

    /// The row for an opcode: numeric key first (E.3's normative form), then the `OPNAME` alias.
    pub fn opcode_row(&self, opcode: u32, opname: &str) -> Option<&String> {
        self.opcode_map
            .get(&CapTable::numeric_key(opcode))
            .or_else(|| self.opcode_map.get(opname))
    }

    /// Parse a row into a [`Mapping`]. Grammar (documented in `docs/internals/ifbc-encoding-rust.md`
    /// and at the top of each shipped table):
    ///
    /// ```text
    /// value := "-" | "ignore" | mnemonic | "arg:" case ("," case)* | "arg:" case ("," case)* "," "else=" value
    /// case  := key "=" mnemonic
    /// ```
    pub fn mapping(&self, opcode: u32, opname: &str) -> Result<Option<Mapping>, IngestErr> {
        let raw = match self.opcode_row(opcode, opname) {
            None => return Ok(None),
            Some(s) => s.clone(),
        };
        Ok(Some(parse_mapping(&raw, opname)?))
    }

    /// `[builtin]` lookup; `None` means the name is not a known builtin at all.
    pub fn builtin(&self, name: &str) -> Option<&str> {
        self.builtins.get(name).map(|s| s.as_str())
    }

    pub fn is_rejected_builtin(&self, name: &str) -> bool {
        matches!(self.builtin(name), Some("-"))
    }

    /// Scan item 3's verdict for an imported module name.
    pub fn import_verdict(&self, name: &str) -> ImportVerdict {
        let root = name.split('.').next().unwrap_or(name);
        for p in &self.deny_imports {
            if root == p || name.starts_with(&format!("{p}.")) {
                return ImportVerdict::Denied(p.clone());
            }
        }
        for p in &self.allow_imports {
            if root == p || name.starts_with(&format!("{p}.")) {
                return ImportVerdict::Allowed(p.clone());
            }
        }
        ImportVerdict::Undeclared
    }

    /// Scan item 5: is this `co_exceptiontable` entry class covered?
    pub fn exception_allowed(&self, kind: ExcKind) -> bool {
        self.exception_entries.iter().any(|e| e == kind.word())
    }

    /// How many opcodes this table maps, and how many it explicitly refuses — the numbers §7.7.2
    /// wants to be able to compare against `infy ingest probe` output.
    pub fn coverage(&self) -> CapCoverage {
        let mut c = CapCoverage::default();
        for (k, v) in &self.opcode_map {
            let parsed = parse_mapping(v, k).ok();
            let nameish = !k.starts_with("0x");
            match parsed {
                Some(Mapping::Direct(Mapped::Reject)) => {
                    if !nameish {
                        c.rejected += 1;
                    }
                }
                Some(_) => {
                    if !nameish {
                        c.mapped += 1;
                    }
                }
                None => {
                    if !nameish {
                        c.unparseable.push(k.clone());
                    }
                }
            }
        }
        for (k, v) in &self.builtins {
            if v == "-" {
                c.rejected_builtins.push(k.clone());
            } else {
                c.builtins += 1;
            }
        }
        c
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CapCoverage {
    pub mapped: usize,
    pub rejected: usize,
    pub builtins: usize,
    pub rejected_builtins: Vec<String>,
    pub unparseable: Vec<String>,
}

/// Scan item 3's verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportVerdict {
    /// Present in `[import] allow`: a declared pure-bytecode package or `.infib` (§7.7.4).
    Allowed(String),
    /// Present in `[import] deny`: a native extension — `E-0712` plus the §7.7.4 escape routes.
    Denied(String),
    /// Neither: undeclared, therefore `E-0712` ("必须是已声明的 INFY module / `.infib` / 能力表内的
    /// 纯字节码包").
    Undeclared,
}

/// The §7.7.2 dispatch key of an instruction: which sub-mapping of a family row applies. The
/// vocabulary is documented in each shipped table's header.
pub fn dispatch_key(instr: &PyInstr) -> String {
    match instr.opname.as_str() {
        "BINARY_OP" | "COMPARE_OP" | "IS_OP" | "CONTAINS_OP" | "CALL_INTRINSIC_1"
        | "LOAD_COMMON_CONSTANT" | "UNPACK_EX" | "SEND" | "MAKE_FUNCTION" => {
            let k = instr.argrepr.clone();
            if k.is_empty() {
                match instr.opname.as_str() {
                    "MAKE_FUNCTION" => match instr.arg {
                        Some(0) | None => "plain".to_string(),
                        Some(_) => "flags".to_string(),
                        _ => "flags".to_string(),
                    },
                    _ => k,
                }
            } else {
                k
            }
        }
        // LOAD_ATTR/STORE_ATTR carry a method-call flag in bit 0 since 3.12; bound methods need the
        // Python object model, so the tables say which of the two forms is covered.
        "LOAD_ATTR" | "STORE_ATTR" => match instr.arg {
            Some(a) if a & 1 == 1 => "method".to_string(),
            _ => "attr".to_string(),
        },
        "CALL" | "CALL_KW" => match instr.arg {
            Some(_) => "stack".to_string(),
            None => "stack".to_string(),
        },
        _ => String::new(),
    }
}

// ------------------------------------------------------------------ scan violations (§7.7.2)

/// One concrete gap. The spec's demand is that the diagnostic *names the item* — "按诊断列出的具体项
/// 等待能力表扩容" — so `item` always carries the opcode/builtin/import/entry that is missing, never
/// just "unsupported".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanViolation {
    /// `E-0711` (dynamic code generation) or `E-0712` (capability gap).
    pub code: &'static str,
    /// Which of §7.7.2's five scan items produced it.
    pub scan_item: &'static str,
    /// The named item, e.g. `BINARY_OP(**)`, `builtin(input)`, `import(numpy)`.
    pub item: String,
    /// Human-readable detail, including the escape route when the spec gives one.
    pub detail: String,
    /// The code object the item was found in (`__infy_unit__`, `<lambda>`, …) — scan item 4 requires
    /// the recursion to be visible in the diagnosis.
    pub code_name: String,
    /// Offset inside `co_code`, and the original-language line when the code object carries one.
    pub py_offset: u32,
    pub py_line: Option<u32>,
}

impl ScanViolation {
    pub fn new(
        code: &'static str,
        scan_item: &'static str,
        item: impl Into<String>,
        detail: impl Into<String>,
        code_name: impl Into<String>,
        py_offset: u32,
        py_line: Option<u32>,
    ) -> ScanViolation {
        ScanViolation {
            code,
            scan_item,
            item: item.into(),
            detail: detail.into(),
            code_name: code_name.into(),
            py_offset,
            py_line,
        }
    }

    pub fn render(&self) -> String {
        let line = self
            .py_line
            .map(|l| format!(" line {l}"))
            .unwrap_or_default();
        format!(
            "{} [{}] {} @ co_code+{:#x}{} (code object {}): {}",
            self.code,
            self.scan_item,
            self.item,
            self.py_offset,
            line,
            self.code_name,
            self.detail
        )
    }

    /// The §9 diagnostic for one violation, positioned at the embed unit.
    pub fn diagnostic(&self, file: FileId, pos: Position) -> Diagnostic {
        let d = Diagnostic::new(self.code, Phase::Embed, file, pos)
            .with_origin(Origin::EmbedPy)
            .titled(match self.code {
                "E-0711" => format!("动态代码生成：{}", self.item),
                _ => format!("字节码能力缺失：{}", self.item),
            })
            .note(format!(
                "扫描项 {} · code object `{}` · co_code+{:#x}{}",
                self.scan_item,
                self.code_name,
                self.py_offset,
                self.py_line.map(|l| format!(" · 原始语言行 {l}")).unwrap_or_default()
            ))
            .note(&self.detail);
        match self.code {
            "E-0711" => d.help("静态展开，或改 A 轨 / isolation=\"process\"（§7.7.2）"),
            _ => d.help("按诊断列出的具体项等待能力表扩容，或改写该处（§7.7.2）"),
        }
    }
}

/// §7.7.2's five scan items, over the whole code-object tree, plus `E-0711` for dynamic code
/// generation. Pure: it reports every gap rather than the first one, because capability expansion is
/// planned from that list (§12.2 feeds it into conformance).
pub fn capability_scan(dump: &CodeObjDump, cap: &CapTable) -> Vec<ScanViolation> {
    let mut out = Vec::new();
    for co in dump.unit_code().tree() {
        scan_code_obj(co, cap, &mut out);
    }
    out.sort_by(|a, b| {
        (&a.code_name, a.py_offset, &a.item).cmp(&(&b.code_name, b.py_offset, &b.item))
    });
    out
}

fn scan_code_obj(co: &CodeObj, cap: &CapTable, out: &mut Vec<ScanViolation>) {
    let name = co.name.as_str();
    let line_of = |off: u32| co.line_at(off);

    // Item 1 — the opcode set must be entirely inside the table.
    for i in &co.instrs {
        if let Err(e) = cap.check_alias_agreement(i.opcode, &i.opname) {
            out.push(ScanViolation::new(
                "E-0712",
                "opcode",
                format!("能力表行 {} / {}", CapTable::numeric_key(i.opcode), i.opname),
                e.title(),
                name,
                i.offset,
                line_of(i.offset),
            ));
            continue;
        }
        match cap.mapping(i.opcode, &i.opname) {
            Ok(None) => out.push(ScanViolation::new(
                "E-0712",
                "opcode",
                format!("{} ({})", i.opname, CapTable::numeric_key(i.opcode)),
                "该 opcode 不在能力表内：本 minor 的该指令从未被映射（§7.8.2 映射表）".to_string(),
                name,
                i.offset,
                line_of(i.offset),
            )),
            Ok(Some(m)) => {
                let key = dispatch_key(i);
                if let Mapped::Reject = m.resolve(&key) {
                    let shown = if key.is_empty() {
                        i.opname.clone()
                    } else {
                        format!("{}({})", i.opname, key)
                    };
                    out.push(ScanViolation::new(
                        "E-0712",
                        "opcode",
                        shown,
                        "能力表明确标记为未映射（值为 \"-\"）：构造依赖 INFY 尚未实现的对象模型语义（§7.7.6）".to_string(),
                        name,
                        i.offset,
                        line_of(i.offset),
                    ));
                }
            }
            Err(e) => out.push(ScanViolation::new(
                "E-0712",
                "opcode",
                i.opname.clone(),
                e.title(),
                name,
                i.offset,
                line_of(i.offset),
            )),
        }
    }

    // Item 2 — builtins referenced through LOAD_GLOBAL, plus E-0711 for dynamic code generation.
    let stored_globals: Vec<String> = co
        .instrs
        .iter()
        .filter(|i| matches!(i.opname.as_str(), "STORE_GLOBAL" | "STORE_NAME" | "IMPORT_NAME"))
        .filter_map(|i| co.name_at(i.arg.unwrap_or(NONE32)).map(|s| s.to_string()))
        .collect();
    for i in &co.instrs {
        if i.opname != "LOAD_GLOBAL" {
            continue;
        }
        let Some(nm) = co.operand_name(i) else { continue };
        let nm = nm.to_string();
        if DYNAMIC_CODE_NAMES.contains(&nm.as_str()) {
            out.push(ScanViolation::new(
                "E-0711",
                "builtin",
                format!("builtin({nm})"),
                "动态代码生成无法被静态扫描覆盖，自 v2.0 起永久拒绝（§7.7.2）。getattr 形式的动态派发同属此类。"
                    .to_string(),
                name,
                i.offset,
                line_of(i.offset),
            ));
            continue;
        }
        match cap.builtin(&nm) {
            Some("-") => out.push(ScanViolation::new(
                "E-0712",
                "builtin",
                format!("builtin({nm})"),
                "该内置名在本语言的运行时能力表内未实现；诊断已列出具体名字，等待能力表扩容（§7.7.2）。"
                    .to_string(),
                name,
                i.offset,
                line_of(i.offset),
            )),
            Some(_) => {}
            None => {
                let local_ok = co.varnames.iter().any(|v| *v == nm)
                    || stored_globals.iter().any(|g| *g == nm)
                    || co.cellvars.iter().any(|v| *v == nm)
                    || co.freevars.iter().any(|v| *v == nm);
                if !local_ok {
                    out.push(ScanViolation::new(
                        "E-0712",
                        "builtin",
                        format!("global({nm})"),
                        "既不在能力表的 builtin 段，也不是本单元自身定义的名字：外部全局名必须由已声明的 \
                         INFY module / `.infib` 提供（§7.7.2 扫描项 2/3）。"
                            .to_string(),
                        name,
                        i.offset,
                        line_of(i.offset),
                    ));
                }
            }
        }
    }

    // Item 3 — import targets (`IMPORT_NAME`'s constant name).
    for i in &co.instrs {
        if i.opname != "IMPORT_NAME" {
            continue;
        }
        let Some(nm) = co.operand_name(i) else { continue };
        let nm = nm.to_string();
        if DYNAMIC_CODE_NAMES.contains(&nm.as_str()) || nm == "importlib" {
            out.push(ScanViolation::new(
                "E-0711",
                "import",
                format!("import({nm})"),
                "运行时导入/动态代码生成无法静态扫描（§7.7.2）。".to_string(),
                name,
                i.offset,
                line_of(i.offset),
            ));
            continue;
        }
        match cap.import_verdict(&nm) {
            ImportVerdict::Allowed(_) => {}
            ImportVerdict::Denied(p) => out.push(ScanViolation::new(
                "E-0712",
                "import",
                format!("import({nm})"),
                format!(
                    "`{p}` 在能力表的 deny 段：它带原生扩展，IFBC 无法重实现厂商的机器码算子。出路（§7.7.4）：\
                     (a) A 轨 @c 链接其 C API；(b) @py(isolation=\"process\")；(c) 宿主 HostApi 事件。"
                ),
                name,
                i.offset,
                line_of(i.offset),
            )),
            ImportVerdict::Undeclared => out.push(ScanViolation::new(
                "E-0712",
                "import",
                format!("import({nm})"),
                "导入目标未在清单/能力表中声明为纯字节码包（§7.7.2 扫描项 3）。".to_string(),
                name,
                i.offset,
                line_of(i.offset),
            )),
        }
    }

    // Item 4 — nested code objects. The recursion happens by walking `tree()`; a unit whose closure
    // machinery is still present cannot be lowered without the cell/object model (§7.7.6).
    if !co.freevars.is_empty() || !co.cellvars.is_empty() {
        out.push(ScanViolation::new(
            "E-0712",
            "嵌套 code object",
            format!(
                "closure 变量 [{}] / cell [{}]",
                co.freevars.join(", "),
                co.cellvars.join(", ")
            ),
            "嵌套作用域需要 cell 对象模型；能力表 v1 未覆盖（§7.7.2 扫描项 4 要求递归穷举，此处即穷举结果）。"
                .to_string(),
            name,
            0,
            Some(co.firstlineno),
        ));
    }

    // Item 5 — exception table shape.
    for e in &co.exc {
        let kind = co.exc_entry_kind(e);
        if !cap.exception_allowed(kind) {
            out.push(ScanViolation::new(
                "E-0712",
                "异常表",
                format!("entry({}) @ [{:#x},{:#x})", kind.word(), e.start, e.end),
                format!(
                    "该 co_exceptiontable 条目类型不可映射到 IFBC exception_table（handler {:#x}, depth {}）；\
                     能力表 entry 段只覆盖 {:?}（§7.7.2 扫描项 5）。",
                    e.target,
                    e.depth,
                    cap.exception_entries
                ),
                name,
                e.start,
                line_of(e.start),
            ));
        }
    }

    // Constants that do not fit the IFBC value model are named, not silently approximated.
    for (idx, c) in co.consts.iter().enumerate() {
        match c {
            PyConst::BigInt(v) => out.push(ScanViolation::new(
                "E-0712",
                "常量",
                format!("int 常量 {v}"),
                "超出 IFBC `Int`(i64) 宽度；Python 的任意精度整数属于对象模型成本（§7.7.6）。".to_string(),
                name,
                idx as u32,
                Some(co.firstlineno),
            )),
            PyConst::Unsupported(u) => out.push(ScanViolation::new(
                "E-0712",
                "常量",
                format!("常量 {u}"),
                "该常量类型没有 IFBC 对应形态。".to_string(),
                name,
                idx as u32,
                Some(co.firstlineno),
            )),
            _ => {}
        }
    }
}

impl PyInstr {
    /// The name this instruction's argument selects, unpacking CPython's bit-0 flag (§7.7.2 scan
    /// items 2/3 — see [`CodeObj::operand_name`]).
    pub fn operand_name<'a>(&self, co: &'a CodeObj) -> Option<&'a str> {
        co.operand_name(self)
    }
}

fn parse_mapping(raw: &str, context: &str) -> Result<Mapping, IngestErr> {
    let raw = raw.trim();
    if raw == "-" {
        return Ok(Mapping::Direct(Mapped::Reject));
    }
    if raw == "ignore" {
        return Ok(Mapping::Direct(Mapped::Ignore));
    }
    if let Some(body) = raw.strip_prefix("arg:") {
        let mut cases = BTreeMap::new();
        let mut otherwise = None;
        for part in split_top(body) {
            // `k->v`, not `k=v`: Python's own dispatch keys include `=`, `<=` and `==`.
            let (k, v) = part
                .split_once("->")
                .ok_or_else(|| IngestErr::CapTable(format!("{context}: 派发项 `{part}` 缺 '->'")))?;
            let m = parse_mapped(v.trim(), context)?;
            if k.trim() == "else" {
                otherwise = Some(m);
            } else {
                cases.insert(k.trim().to_string(), m);
            }
        }
        return Ok(Mapping::Dispatch { cases, otherwise });
    }
    Ok(Mapping::Direct(parse_mapped(raw, context)?))
}

/// One mapping value: `-`, `ignore`, a mnemonic, or `a|b|c` alternatives.
fn parse_mapped(raw: &str, context: &str) -> Result<Mapped, IngestErr> {
    let raw = raw.trim();
    if raw.contains('|') {
        let mut parts = Vec::new();
        for p in split_top(raw.replace('|', ",").as_str()) {
            parts.push(parse_single(p.trim(), context)?);
        }
        if parts.len() == 1 {
            return Ok(parts.pop().unwrap());
        }
        return Ok(Mapped::Alt(parts));
    }
    parse_single(raw, context)
}

fn parse_single(raw: &str, context: &str) -> Result<Mapped, IngestErr> {
    let raw = raw.trim();
    // E.3's grammar makes `-` and `ignore` *values*, so they are legal in a dispatch branch too —
    // `arg:+->add._,…,else->-` is how a table says "this one sub-operation is covered, the rest are
    // gaps". Treating the branch value as a mnemonic made every such row a table-parse error, which
    // the scan then reported as `E-0712` on the whole opcode instead of the one missing branch.
    if raw == "-" {
        return Ok(Mapped::Reject);
    }
    if raw == "ignore" {
        return Ok(Mapped::Ignore);
    }
    if let Some(host) = raw.strip_prefix("host:") {
        if host.is_empty() {
            return Err(IngestErr::CapTable(format!("{context}: host: 后必须给出 std 路径")));
        }
        return Ok(Mapped::Host(host.to_string()));
    }
    check_mnemonic(raw, context)?;
    Ok(Mapped::Mnem(raw.to_string()))
}

/// Reject a mnemonic the encoder does not know *at table-parse time*: an unreadable capability table
/// must not be able to smuggle an undefined instruction into a build (§7.8.2).
/// Split on commas that are not inside parentheses or brackets, so a dispatch key containing a
/// comma (CPython's fused `argrepr`, e.g. `x, x`) survives.
fn split_top(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '(' | '[' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur = String::new();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Reject a mnemonic the encoder does not know *at table-parse time*: an unreadable capability table
/// must not be able to smuggle an undefined instruction into a build (§7.8.2).
fn check_mnemonic(text: &str, context: &str) -> Result<(), IngestErr> {
    // `add._` is the type-placeholder form; drop the placeholder before resolving the base name.
    let probe = text.trim().strip_suffix("._").unwrap_or(text.trim());
    if ifbc::parse_mnemonic(probe).is_some() {
        return Ok(());
    }
    Err(IngestErr::CapTable(format!(
        "{context}: 目标助记符 `{text}` 不是附录 D 定义的 IFBC 指令"
    )))
}


// ------------------------------------------------------------------ lowering (§7.7.1)

/// A value of the *model* operand stack. IFBC is a stack machine too, so most Python instructions
/// map 1:1; a value may additionally be parked in a local-window slot, which is what lets loop
/// headers and fused instructions keep one verifiable stack height (§7.8.3 item 2).
#[derive(Clone, Debug, PartialEq)]
enum Val {
    /// Physically on the IFBC operand stack.
    Stack { ty: u32 },
    /// In the frame's local window (parameter, Python local, or compiler scratch).
    Slot { slot: u32, ty: u32 },
    /// Model-only entry: a builtin loaded by `LOAD_GLOBAL` whose call is finished by `CALL`.
    PendingHost { name: String, arity: usize },
}

impl Val {
    fn ty(&self) -> u32 {
        match self {
            Val::Stack { ty } | Val::Slot { ty, .. } => *ty,
            Val::PendingHost { .. } => NONE32,
        }
    }
}

/// What [`Lowerer::choose`] settled on for one Python instruction.
#[derive(Clone, Debug)]
enum Selected {
    /// A D.3 family instruction: needs its type suffix, its guard and the box/unbox pair.
    Family(Op, u32),
    /// An object-level instruction: takes the `dyn` boxes as they are (D.6/D.7).
    Object(Op, u32),
    /// A `call.host` target from the row's `host:` form.
    Host(String),
    None,
}

/// `tickloop`'s quota operand: §8.5 preemption point ④ fixes the count at 65536.
pub const TICK_QUOTA: u32 = 65536;

/// §7.7.1 rule 1: the *INFY-side* declared types give the unit's parameter table. `HostSigTable`
/// carries it under the unit's own name (§7.7.1 turns the `@py` bindings into a calling convention,
/// not into injected globals).
pub fn unit_signature(co: &CodeObj, hosts: &HostSigTable) -> HostSig {
    if let Some(s) = hosts.get(&co.name) {
        return s.clone();
    }
    if let Some(s) = hosts.get("__infy_unit__") {
        return s.clone();
    }
    HostSig::new(&co.name, &vec![TyKind::Dyn; co.argcount as usize], TyKind::Dyn)
}

fn intern_kind(m: &mut IfbcModule, k: TyKind) -> u32 {
    m.intern_type(k, k.suffix(), NONE32, NONE32)
}

fn intern_list(m: &mut IfbcModule, elem: u32) -> u32 {
    m.intern_type(TyKind::List, "list", elem, NONE32)
}

/// §7.7.1's mechanical mapping: code object tree → one IFBC module. The capability scan of §7.7.2
/// runs first, so this is only ever reached by input whose opcodes, builtins, imports, nested code
/// objects and exception entries are all covered — that ordering is what makes "never a runtime
/// surprise" a build-time guarantee (P3). Every nested code object becomes its own `func_table`
/// entry, which is what scan item 4's recursion has to mean.
pub fn lower(
    dump: &CodeObjDump,
    cap: &CapTable,
    hosts: &HostSigTable,
) -> Result<IfbcModule, Vec<ScanViolation>> {
    lower_named(dump, cap, hosts, &dump.unit)
}

pub fn lower_named(
    dump: &CodeObjDump,
    cap: &CapTable,
    hosts: &HostSigTable,
    source_name: &str,
) -> Result<IfbcModule, Vec<ScanViolation>> {
    let mut viol = capability_scan(dump, cap);
    if !viol.is_empty() {
        return Err(viol);
    }
    let unit = dump.unit_code();
    let mut m = IfbcModule::new_unit(&cap.format, cap.cap_version, &dump.unit, source_name)
        .ok_or_else(|| {
            vec![ScanViolation::new(
                "E-0713",
                "能力表",
                format!("lang_tag({})", cap.format),
                format!(
                    "能力表的 format 不是本工具链认识的 lang_tag（§7.8.1）；已知：{}",
                    crate::ifbc::FORMATS.iter().map(|(_, n)| *n).collect::<Vec<_>>().join(", ")
                ),
                "<cap>",
                0,
                None,
            )]
        })?;
    // Pool positions 0/1 are documented as unit / source_name (`decode` reads them back).
    m.intern(&dump.unit);
    m.intern(source_name);

    let tree = unit.tree();
    for (i, co) in tree.iter().enumerate() {
        let is_unit = i == 0;
        let sig = if is_unit {
            unit_signature(co, hosts)
        } else {
            // A nested function's parameters stay `dyn` until the object model can type them.
            HostSig::new(&co.name, &vec![TyKind::Dyn; co.argcount as usize], TyKind::Dyn)
        };
        let base = m.code.len() as u32;
        let lr = Lowerer::new(cap, hosts, &mut m, co, sig, base).run();
        let Lowered { builder, lines, exc, viol: vv, param_kinds, ret_kind, nlocals, scratch, offsets, .. } = lr;
        viol.extend(vv);
        if !viol.is_empty() {
            continue;
        }
        // D-38: `func_table` carries a declared operand-stack ceiling; the builder tracked the peak
        // of the emitted stream, and `finish_into` consumes the builder, so take it first.
        let max_stack = builder.max_stack();
        let (code_off, code_len) = builder.finish_into(&mut m);
        let _ = code_off;
        m.lines.extend(lines.iter().map(|(o, l)| ifbc::LineEntry { code_off: *o, line: *l }));
        m.exceptions.extend(exc.iter().copied());
        let locals = nlocals + scratch;
        m.push_func(
            &co.name,
            code_off,
            code_len,
            locals,
            max_stack,
            &param_kinds,
            ret_kind,
            if co.is_coroutine_frame() {
                ifbc::FRAME_KIND_COROUTINE
            } else {
                ifbc::FRAME_KIND_STACK
            },
            if is_unit { F_ENTRY } else { 0 },
        );
        let _ = offsets;
    }
    if !viol.is_empty() {
        viol.sort_by(|a, b| {
            (&a.code_name, a.py_offset, &a.item).cmp(&(&b.code_name, b.py_offset, &b.item))
        });
        return Err(viol);
    }
    // §7.8.3 items 1-6 as a build-time self-check: an image this toolchain cannot verify must never
    // ship, because the runtime refuses it wholesale (R-0703, UB-019).
    let bad = ifbc::verify(&m, hosts, true)
        .into_iter()
        .filter(|v| v.code.starts_with('R'))
        .collect::<Vec<_>>();
    if !bad.is_empty() {
        return Err(vec![ScanViolation::new(
            "E-0712",
            "lowering 自检",
            format!("装载校验失败 {} 项", bad.len()),
            bad.iter().map(|v| v.render()).collect::<Vec<_>>().join(" · "),
            dump.unit.clone(),
            0,
            None,
        )]);
    }
    m.seal();
    Ok(m)
}

/// What one code object contributed to the module.
struct Lowered {
    builder: CodeBuilder,
    lines: Vec<(u32, u32)>,
    exc: Vec<ExcEntry>,
    viol: Vec<ScanViolation>,
    param_kinds: Vec<TyKind>,
    ret_kind: TyKind,
    nlocals: u32,
    scratch: u32,
    offsets: BTreeMap<u32, u32>,
}

struct Lowerer<'a> {
    cap: &'a CapTable,
    hosts: &'a HostSigTable,
    m: &'a mut IfbcModule,
    co: &'a CodeObj,
    b: CodeBuilder,
    stack: Vec<Val>,
    local_ty: Vec<u32>,
    nlocals: u32,
    scratch: u32,
    off_map: BTreeMap<u32, u32>,
    fixups: Vec<(u32, u32)>,
    pending_entry: BTreeMap<u32, Vec<(u32, u32)>>,
    saved: Vec<(u32, u32)>,
    viol: Vec<ScanViolation>,
    lines: Vec<(u32, u32)>,
    exc: Vec<ExcEntry>,
    param_kinds: Vec<TyKind>,
    ret_kind: TyKind,
    /// Set once a `ret`/`throw.inf` closed the frame; `finish` adds the implicit `return None` only
    /// when the Python body fell off the end.
    terminated: bool,
    ty_dyn: u32,
    ty_unit: u32,
    ty_int: u32,
    ty_float: u32,
    ty_str: u32,
    ty_bool: u32,
}

impl<'a> Lowerer<'a> {
    fn new(
        cap: &'a CapTable,
        hosts: &'a HostSigTable,
        m: &'a mut IfbcModule,
        co: &'a CodeObj,
        sig: HostSig,
        base: u32,
    ) -> Lowerer<'a> {
        let nlocals = co.nlocals.max(co.varnames.len() as u32);
        let ty_dyn = m.intern_type(TyKind::Dyn, "dyn", NONE32, NONE32);
        let ty_unit = m.intern_type(TyKind::Unit, "unit", NONE32, NONE32);
        let ty_int = m.intern_type(TyKind::Int, "i64", NONE32, NONE32);
        let ty_float = m.intern_type(TyKind::Float, "f64", NONE32, NONE32);
        let ty_str = m.intern_type(TyKind::Str, "str", NONE32, NONE32);
        let ty_bool = m.intern_type(TyKind::Bool, "bool", NONE32, NONE32);
        let mut local_ty = vec![ty_dyn; nlocals.max(1) as usize];
        for (i, k) in sig.params.iter().enumerate() {
            if i < local_ty.len() {
                local_ty[i] = intern_kind(m, *k);
            }
        }
        Lowerer {
            cap,
            hosts,
            m,
            co,
            b: CodeBuilder::new(base),
            stack: Vec::new(),
            local_ty,
            nlocals,
            scratch: 0,
            off_map: BTreeMap::new(),
            fixups: Vec::new(),
            pending_entry: BTreeMap::new(),
            saved: Vec::new(),
            viol: Vec::new(),
            lines: Vec::new(),
            exc: Vec::new(),
            param_kinds: sig.params.clone(),
            ret_kind: sig.ret,
            terminated: false,
            ty_dyn,
            ty_unit,
            ty_int,
            ty_float,
            ty_str,
            ty_bool,
        }
    }

    // ---- plumbing ---------------------------------------------------------------

    fn kind(&self, ty: u32) -> Option<TyKind> {
        self.m.ty_kind(ty)
    }
    fn elem_of(&self, ty: u32) -> Option<u32> {
        self.m
            .types
            .get(ty as usize)
            .and_then(|t| if t.a == NONE32 { None } else { Some(t.a) })
    }
    fn alloc_scratch(&mut self) -> u32 {
        let s = self.nlocals + self.scratch;
        self.scratch += 1;
        s
    }
    fn err(&mut self, item: impl Into<String>, detail: impl Into<String>, at: &PyInstr) {
        let l = self.line(at);
        self.viol.push(ScanViolation::new(
            "E-0712",
            "lowering",
            item,
            detail,
            self.co.name.clone(),
            at.offset,
            l,
        ));
    }
    fn line(&self, at: &PyInstr) -> Option<u32> {
        at.start_line.or(Some(self.co.firstlineno))
    }
    /// Emit one instruction and record its `line_map` mark (§7.4: lowering 必须从 code object 带上行映射).
    fn emit(&mut self, op: Op, ops: &[u32], at: &PyInstr) -> Result<(), ()> {
        let l = self.line(at);
        match self.b.emit(op, ops, l) {
            Ok(off) => {
                if let Some(ll) = l {
                    self.lines.push((off, ll));
                }
                Ok(())
            }
            Err(e) => {
                self.err(format!("{} 编码失败", op.base()), e.to_string(), at);
                Err(())
            }
        }
    }
    /// A slot-held value is re-pushed with `ldl`; one already on the stack is a no-op.
    fn push_val(&mut self, v: &Val, at: &PyInstr) -> Result<u32, ()> {
        match v {
            Val::Stack { ty } => Ok(*ty),
            Val::Slot { slot, ty } => {
                self.emit(Op::Ldl, &[*slot], at)?;
                Ok(*ty)
            }
            Val::PendingHost { .. } => {
                self.err("把函数值当作数据使用", "callee 不是可装箱值（能力表 v1 未覆盖）", at);
                Err(())
            }
        }
    }
    fn to_slot(&mut self, v: Val, at: &PyInstr) -> Result<Val, ()> {
        match v {
            Val::Slot { slot, ty } => Ok(Val::Slot { slot, ty }),
            Val::Stack { ty } => {
                let s = self.alloc_scratch();
                self.emit(Op::Stl, &[s], at)?;
                Ok(Val::Slot { slot: s, ty })
            }
            Val::PendingHost { .. } => {
                self.err("把函数值当作数据使用", "callee 不是可装箱值", at);
                Err(())
            }
        }
    }
    fn pop(&mut self, at: &PyInstr) -> Result<Val, ()> {
        match self.stack.pop() {
            Some(v) => Ok(v),
            None => {
                self.err("栈模型下溢", "code object 的栈形状与设计不符（§7.7.1 机械映射无法继续）", at);
                Err(())
            }
        }
    }
    /// Park every stack value in a slot: the transformation that keeps a block entry at operand-stack
    /// height 0 (§7.8.3 item 2), applied at loop headers and before forward branches.
    fn park_stack(&mut self, at: &PyInstr) -> Result<(), ()> {
        let n = self.stack.len();
        for i in (0..n).rev() {
            if matches!(self.stack[i], Val::Stack { .. }) {
                let v = self.stack.remove(i);
                let s = self.to_slot(v, at)?;
                self.stack.insert(i, s);
            }
        }
        Ok(())
    }
    fn ifbc_height(&self) -> usize {
        self.stack.iter().filter(|v| matches!(v, Val::Stack { .. })).count()
    }
    fn mapping_for(&self, i: &PyInstr) -> Option<Mapped> {
        let row = self.cap.mapping(i.opcode, &i.opname).ok().flatten()?;
        Some(row.resolve(&dispatch_key(i)))
    }
    /// Patch every jump whose Python target is `py_off` with the IFBC offset we just recorded.
    fn bind(&mut self, py_off: u32) {
        if let Some(&t) = self.off_map.get(&py_off) {
            let mine: Vec<(u32, u32)> = self
                .fixups
                .iter()
                .filter(|(_, p)| *p == py_off)
                .copied()
                .collect();
            for (operand_at, _) in mine {
                let _ = self.b.patch(operand_at, t);
                self.fixups.retain(|(a, _)| *a != operand_at);
            }
        }
    }
    fn branch(&mut self, op: Op, target_py: u32, at: &PyInstr) -> Result<(), ()> {
        // Blocks are entered at operand-stack height 0 (item 2), so a branch carrying live stack
        // values must park them first; the target then picks the same slots back up.
        self.park_stack(at)?;
        if self.ifbc_height() != 0 {
            self.err(
                format!("{} 处栈高度非 0", op.base()),
                "回边/分支入口的 IFBC 操作数栈必须为空，否则校验项 2 无法成立".to_string(),
                at,
            );
            return Err(());
        }
        let rec: Vec<(u32, u32)> = self
            .stack
            .iter()
            .filter_map(|v| match v {
                Val::Slot { slot, ty } => Some((*slot, *ty)),
                _ => None,
            })
            .collect();
        self.pending_entry.insert(target_py, rec);
        let operand_at = self
            .b
            .emit_with_fixup(op, &[0], self.line(at))
            .map_err(|e| {
                self.err(format!("{} 编码失败", op.base()), e.to_string(), at);
            })?;
        self.fixups.push((operand_at, target_py));
        Ok(())
    }
    /// The one linear pass over `co_code`. §7.7.1: this is a *mechanical* mapping of an instruction
    /// stream that CPython already validated — no Python grammar is consulted anywhere.
    fn run(mut self) -> Lowered {
        let instrs = self.co.instrs.clone();
        // Every target <= its own offset is a back edge; §7.7.1 item 3 says `tickloop` goes in front
        // of each of them, which is also where §8.5 preemption point ④ and R-0701's timeout live.
        let back_edges: std::collections::BTreeSet<u32> = instrs
            .iter()
            .filter(|i| is_jump(i) && matches!(i.argval_int, Some(t) if (t as u32) <= i.offset))
            .filter_map(|i| i.argval_int.map(|t| t as u32))
            .collect();
        let reach = reachable(&instrs);
        let mut i = 0usize;
        while i < instrs.len() {
            let at = instrs[i].clone();
            if !reach.contains(&at.offset) {
                // Unreachable-from-entry code: in CPython 3.12+ that is the implicit unwind/cleanup
                // block an inlined comprehension leaves behind. It is not dead weight here — it is
                // what the `co_exceptiontable` entry names, and §7.7.2 scan item 5 has already
                // checked that entry.
                i += 1;
                continue;
            }
            self.bind(at.offset);
            if let Some(rec) = self.pending_entry.remove(&at.offset) {
                self.stack = rec.into_iter().map(|(slot, ty)| Val::Slot { slot, ty }).collect();
            }
            if matches!(self.mapping_for(&at), Some(Mapped::Ignore)) {
                self.off_map.insert(at.offset, self.b.offset());
                i += 1;
                continue;
            }
            self.off_map.insert(at.offset, self.b.offset());
            let step = self.instr(&at, i, &instrs, &back_edges);
            match step {
                Ok(n) => i += n,
                Err(()) => {
                    // A violation already names the concrete item; keep walking so the report is the
                    // whole gap list rather than only its first element (§7.7.2).
                    i += 1;
                }
            }
        }
        self.map_exception_table();
        for (operand_at, py_target) in self.fixups.clone() {
            if let Some(&t) = self.off_map.get(&py_target) {
                let _ = self.b.patch(operand_at, t);
            } else {
                self.viol.push(ScanViolation::new(
                    "E-0712",
                    "lowering",
                    format!("跳转目标 {py_target:#x} 不在指令边界上"),
                    "co_code 的跳转目标无法解析——code object 与 dis 的形态不一致".to_string(),
                    self.co.name.clone(),
                    operand_at,
                    None,
                ));
            }
        }
        self.finish();
        Lowered {
            builder: self.b,
            lines: self.lines,
            exc: self.exc,
            viol: self.viol,
            param_kinds: self.param_kinds,
            ret_kind: self.ret_kind,
            nlocals: self.nlocals,
            scratch: self.scratch,
            offsets: self.off_map,
        }
    }

    /// The next instruction offset that was actually emitted at or after `py` — the fallback that
    /// lets a CPython cleanup block, whose own code is not lowered, still be *named* by an IFBC
    /// exception entry instead of silently disappearing.
    fn map_off(&self, py: u32) -> Option<u32> {
        if let Some(&o) = self.off_map.get(&py) {
            return Some(o);
        }
        self.off_map.range(py..).next().map(|(_, &v)| v)
    }

    /// §7.7.2 scan item 5 in the other direction: the code object's own `co_exceptiontable` becomes
    /// IFBC's `exception_table`, region-translated through the offset map. A `push`-class entry whose
    /// handler block this lowering folded away is recorded against the entry's own end offset,
    /// because the IFBC loop's normal exit *is* its unwind path (documented in
    /// `docs/internals/ifbc-encoding-rust.md`).
    fn map_exception_table(&mut self) {
        let entries = self.co.exc.clone();
        for e in entries {
            let kind = self.co.exc_entry_kind(&e);
            let start = self.map_off(e.start);
            let end = self.map_off(e.end);
            let handler = self.map_off(e.target).or(end);
            match (start, end, handler) {
                (Some(st), Some(en), Some(h)) if en >= st => {
                    self.exc.push(ExcEntry {
                        start: st,
                        end: en,
                        handler: h,
                        depth: e.depth.min(u16::MAX as u32) as u16,
                        kind,
                    });
                }
                _ => self.viol.push(ScanViolation::new(
                    "E-0712",
                    "异常表",
                    format!("entry({}) [{:#x},{:#x})", kind.word(), e.start, e.end),
                    "co_exceptiontable 条目的区间无法翻译到已发射的 IFBC 代码".to_string(),
                    self.co.name.clone(),
                    e.start,
                    self.co.line_at(e.start),
                )),
            }
        }
    }

    /// The frame's terminator: whatever is still parked must be returned or dropped, and every
    /// function must end on `ret`/`ret.multi`/`throw.inf` (§7.8.3 item 4).
    fn finish(&mut self) {
        let dummy = PyInstr {
            opname: "RETURN_VALUE".to_string(),
            opcode: 0x64,
            arg: None,
            argrepr: String::new(),
            argval_int: None,
            offset: self.co.code.len() as u32,
            start_line: None,
            end_line: None,
            col: None,
            end_col: None,
            is_jump_target: false,
        };
        if self.stack.iter().any(|v| matches!(v, Val::Stack { .. })) {
            self.err(
                "函数结束时栈上仍有值",
                "Python 的返回约定要求恰好一个值；多余的栈项说明该构造未被覆盖（§7.7.6）".to_string(),
                &dummy,
            );
            return;
        }
        let ret_ty = if self.ret_kind.dyn_tag().is_some() {
            self.ret_kind
        } else {
            TyKind::Dyn
        };
        let want = match ret_ty {
            TyKind::Unit => self.ty_unit,
            TyKind::Int => self.ty_int,
            TyKind::Float => self.ty_float,
            TyKind::Str => self.ty_str,
            TyKind::Bool => self.ty_bool,
            _ => self.ty_dyn,
        };
        let _ = want;
        if self.terminated {
            return;
        }
        // Python returns `None` when the body falls off the end; `ret` still needs exactly one value
        // (§7.8.3 item 4), so materialise it rather than emitting a bare `ret`.
        let _ = self.emit(Op::Nop, &[], &dummy);
        let pool = self.m.intern("None");
        let _ = self.b.emit(Op::Push, &[pool, self.ty_unit], Some(self.co.firstlineno));
        let _ = self.b.emit(Op::Ret, &[], Some(self.co.firstlineno));
        self.terminated = true;
    }

    fn host_import(&mut self, name: &str, at: &PyInstr) -> Result<u32, ()> {
        if self.hosts.get(name).is_none() {
            self.err(
                format!("host({name})"),
                "能力表指向的 host 入口不在 IFABI 签名表内（§7.8.3 校验项 5 会拒绝该模块）".to_string(),
                at,
            );
            return Err(());
        }
        self.m
            .intern_host(self.hosts, name)
            .ok_or_else(|| self.err(format!("host({name})"), "import_table 行创建失败".to_string(), at))
    }

    /// Emit the D.7 tag checkpoint plus the unbox: `is.dyn T` before every `unbox.dyn` is UB-020's
    /// requirement and validation item 3's precondition.
    fn unbox_to(&mut self, v: &Val, want: u32, at: &PyInstr) -> Result<(), ()> {
        self.push_val(v, at)?;
        self.emit(Op::IsDyn, &[want], at)?;
        self.emit(Op::UnboxDyn, &[], at)
    }

    /// D.7 / D-38 (f): `box.dyn` is the identity on an already-boxed value, so it takes no operand;
    /// the type only steers this side's symbolic stack.
    fn box_as(&mut self, ty: u32, at: &PyInstr) -> Result<(), ()> {
        let _ = ty;
        self.emit(Op::BoxDyn, &[], at)
    }

    /// A host call: args already on the stack in Python order; the result is re-boxed as a `dyn` so
    /// the Python-value invariant of the rest of the frame holds (§7.7.7).
    fn host_call(&mut self, name: &str, nargs: usize, at: &PyInstr) -> Result<u32, ()> {
        let idx = self.host_import(name, at)?;
        let sig = self.hosts.get(name).cloned().ok_or(())?;
        if sig.params.len() != nargs {
            self.err(
                format!("host({name}) arity"),
                format!("能力表要求 {nargs} 个实参，IFABI 签名是 {}", sig.params.len()),
                at,
            );
            return Err(());
        }
        self.emit(Op::CallHost, &[idx], at)?;
        let k = if sig.ret.dyn_tag().is_some() {
            // Already a `dyn` box on the wire — nothing to re-wrap.
            return Ok(intern_dyn_for(self.m, sig.ret, sig.ret_elem));
        } else if sig.ret == TyKind::Unit {
            // Python's CALL always leaves exactly one value; `print(x)` returns None.
            let pool = self.m.intern("None");
            self.b.emit(Op::Push, &[pool, self.ty_unit], self.line(at)).map_err(|e| {
                self.err("push 编码失败", e.to_string(), at);
            })?;
            return Ok(self.ty_unit);
        } else {
            match sig.ret {
                TyKind::I8 | TyKind::I16 | TyKind::I32 | TyKind::U8 | TyKind::U16 | TyKind::U32
                | TyKind::U64 | TyKind::Usize | TyKind::Isize | TyKind::Int => TyKind::Int,
                TyKind::F32 | TyKind::Float => TyKind::Float,
                _ => sig.ret,
            }
        };
        let ty = intern_kind(self.m, k);
        self.box_as(ty, at)?;
        Ok(ty)
    }

    /// Pick the first applicable candidate of an `a|b` mapping row. It emits nothing: the caller
    /// feeds the operands, which keeps the D.3 guard rule and the D.7 tag-checkpoint rule in one
    /// place instead of scattered over instruction handlers.
    fn choose(&mut self, at: &PyInstr, args: &[u32]) -> Result<Selected, ()> {
        let mapped = self.mapping_for(at).ok_or(())?;
        for cand in mapped.alternatives() {
            match cand {
                Mapped::Mnem(text) => {
                    let wants_type = text.ends_with("._");
                    let base = text.strip_suffix("._").unwrap_or(text);
                    if let Some((op, suffix)) = ifbc::parse_mnemonic(base) {
                        if op.is_typed_family() || wants_type {
                            if let Some(ty) = self.common_numeric(args) {
                                return Ok(Selected::Family(op, ty));
                            }
                            continue;
                        }
                        let _ = suffix;
                        let ty = args.first().copied().unwrap_or(self.ty_dyn);
                        return Ok(Selected::Object(op, ty));
                    }
                }
                Mapped::Host(name) => {
                    if let Some(sig) = self.hosts.get(name).cloned() {
                        if host_accepts(&sig, args, self.m) {
                            return Ok(Selected::Host(name.clone()));
                        }
                    }
                }
                _ => {}
            }
        }
        self.err(
            format!("{}({})", at.opname, dispatch_key(at)),
            format!(
                "操作数类型 [{}] 不满足能力表给出的任何映射目标 [{}]；数值族指令要求两侧同为数值类型（D.3）",
                args.iter()
                    .map(|t| self.kind(*t).map(|k| k.suffix()).unwrap_or("?"))
                    .collect::<Vec<_>>()
                    .join(", "),
                mapped.mnemonic().unwrap_or("-")
            ),
            at,
        );
        Err(())
    }

    /// Emit a host call for a `host:` candidate: operands as `dyn` boxes (unboxed where the IFABI
    /// signature asks for a raw number), result re-boxed.
    fn emit_host(&mut self, name: &str, args: &[&Val], at: &PyInstr) -> Result<u32, ()> {
        let sig = self.hosts.get(name).cloned().ok_or(())?;
        for (i, a) in args.iter().enumerate() {
            let ty = a.ty();
            self.push_val(a, at)?;
            if sig.params.get(i).copied().unwrap_or(TyKind::Dyn).is_numeric() {
                self.emit(Op::IsDyn, &[ty], at)?;
                self.emit(Op::UnboxDyn, &[], at)?;
            }
        }
        self.host_call(name, args.len(), at)
    }

    /// The single numeric type shared by all these operands. Mixed `i64`/`f64` is **not** widened:
    /// appendix D has no conversion opcode, and a silent widening is exactly the behaviour drift
    /// §7.7 refuses to allow, so an `a|b` row falls through to its host alternative instead.
    fn common_numeric(&mut self, args: &[u32]) -> Option<u32> {
        if args.is_empty() {
            return Some(self.ty_int);
        }
        let first = self.kind(args[0])?;
        if !first.is_numeric() {
            return None;
        }
        for a in &args[1..] {
            if self.kind(*a)? != first {
                return None;
            }
        }
        Some(args[0])
    }

    /// `argrepr` of the fused two-variable forms (`LOAD_FAST_LOAD_FAST`, `STORE_FAST_LOAD_FAST`,
    /// …): CPython names both slots, which is the only honest way to read them (§7.7.1: consume what
    /// `dis` reports, never re-derive an encoding).
    fn fused_names(&self, at: &PyInstr) -> Option<(u32, u32)> {
        let mut it = at.argrepr.split(',');
        let a = it.next()?.trim();
        let b = it.next()?.trim();
        if it.next().is_some() {
            return None;
        }
        Some((self.co.local_slot(a)?, self.co.local_slot(b)?))
    }

    /// The type and pool text of a Python constant. Any constant whose value model IFBC cannot hold
    /// is refused here *and* by the scan, naming it.
    fn const_pair(&mut self, c: &PyConst, at: &PyInstr) -> Option<(String, u32)> {
        let (text, ty) = match c {
            PyConst::None => ("None".to_string(), self.ty_unit),
            PyConst::Bool(b) => (b.to_string(), self.ty_bool),
            PyConst::Int(i) => (i.to_string(), self.ty_int),
            PyConst::Float(f) => (format_f64(*f), self.ty_float),
            PyConst::Str(s) => (s.clone(), self.ty_str),
            PyConst::Bytes(_) => {
                self.err("bytes 常量", "IFBC 的 Bytes 值模型在本能力表版本未接入", at);
                return None;
            }
            PyConst::BigInt(v) => {
                self.err(
                    format!("int 常量 {v}"),
                    "超出 i64 窗口；任意精度整数属于对象模型成本（§7.7.6）".to_string(),
                    at,
                );
                return None;
            }
            PyConst::Tuple(items) => {
                // A tuple constant becomes a `new.tuple` of its boxed elements.
                let mut tys = Vec::new();
                for it in items {
                    let (t, ty) = self.const_pair(it, at)?;
                    let pool = self.m.intern(&t);
                    self.emit(Op::Push, &[pool, ty], at).ok()?;
                    tys.push(ty);
                }
                let elem = if tys.len() == 1 { tys[0] } else { self.ty_dyn };
                let _ = elem;
                let ty = self.m.intern_type(TyKind::Tuple, "tuple", tys.first().copied().unwrap_or(self.ty_dyn), NONE32);
                if self.emit(Op::NewTuple, &[items.len() as u32], at).is_err() {
                    return None;
                }
                self.stack.push(Val::Stack { ty });
                return None; // already emitted end-to-end
            }
            PyConst::Frozenset(_) | PyConst::Ellipsis => {
                self.err(
                    format!("常量 {}", c.repr_short()),
                    "该常量类型没有 IFBC 对应形态（§7.7.2 扫描项 4/常量）".to_string(),
                    at,
                );
                return None;
            }
            PyConst::Unsupported(u) => {
                self.err(format!("常量 {u}"), "能力表未覆盖该常量类型".to_string(), at);
                return None;
            }
            PyConst::Code(inner) => {
                // A nested code object as a constant: it is lowered into `func_table` (scan item 4),
                // but binding it to a name needs MAKE_FUNCTION, which the table refuses.
                self.err(
                    format!("code object({})", inner.name),
                    "嵌套函数体已作为独立 IFBC 函数产出，但闭包/函数对象的绑定不在能力表内（§7.7.6）"
                        .to_string(),
                    at,
                );
                return None;
            }
        };
        Some((text, ty))
    }

    /// Load a constant onto the model stack.
    fn load_const(&mut self, idx: u32, at: &PyInstr) -> Result<(), ()> {
        let c = self
            .co
            .const_at(idx)
            .cloned()
            .ok_or_else(|| self.err(format!("LOAD_CONST {idx}"), "常量索引越界".to_string(), at))?;
        if let PyConst::Code(_) = c {
            let _ = self.const_pair(&c, at);
            return Err(());
        }
        let (text, ty) = self.const_pair(&c, at).ok_or(())?;
        if matches!(c, PyConst::Tuple(_)) {
            return Ok(());
        }
        let pool = self.m.intern(&text);
        self.emit(Op::Push, &[pool, ty], at)?;
        self.stack.push(Val::Stack { ty });
        Ok(())
    }

    /// Map one Python instruction. Returns how many Python instructions it consumed (>1 when a
    /// documented multi-instruction pattern was recognised, e.g. the PEP 709 comprehension prologue).
    fn instr(
        &mut self,
        at: &PyInstr,
        idx: usize,
        all: &[PyInstr],
        back: &std::collections::BTreeSet<u32>,
    ) -> Result<usize, ()> {
        let name = at.opname.as_str();
        let arg = at.arg.unwrap_or(NONE32);
        match name {
            // ---- constants -------------------------------------------------------
            "LOAD_CONST" => {
                self.load_const(arg, at)?;
                Ok(1)
            }
            "LOAD_SMALL_INT" => {
                let ty = self.ty_int;
                self.emit(Op::PushB, &[arg], at)?;
                self.stack.push(Val::Stack { ty });
                Ok(1)
            }
            "LOAD_COMMON_CONSTANT" => {
                self.err(
                    format!("LOAD_COMMON_CONSTANT({})", at.argrepr),
                    "None/NotImplemented 单例常量需经 std.embed 取值；能力表 v1 未覆盖".to_string(),
                    at,
                );
                Err(())
            }
            // ---- locals ----------------------------------------------------------
            "LOAD_FAST" | "LOAD_FAST_BORROW" => {
                let ty = self.local_slot_ty(arg);
                self.emit(Op::Ldl, &[arg], at)?;
                self.stack.push(Val::Stack { ty });
                Ok(1)
            }
            "LOAD_FAST_LOAD_FAST" | "LOAD_FAST_BORROW_LOAD_FAST_BORROW" | "LOAD_FAST_LOAD_FAST_BORROW"
            | "LOAD_FAST_BORROW_LOAD_FAST" => {
                let (a, b) = self.fused_names(at).ok_or_else(|| {
                    self.err(format!("{name}"), "融合指令的名字参数无法从 argrepr 还原".to_string(), at)
                })?;
                let ta = self.local_slot_ty(a);
                self.emit(Op::Ldl, &[a], at)?;
                self.stack.push(Val::Stack { ty: ta });
                let tb = self.local_slot_ty(b);
                self.emit(Op::Ldl, &[b], at)?;
                self.stack.push(Val::Stack { ty: tb });
                Ok(1)
            }
            "STORE_FAST" => {
                let v = self.pop(at)?;
                let ty = v.ty();
                match v {
                    Val::Stack { .. } => self.emit(Op::Stl, &[arg], at)?,
                    Val::Slot { slot, .. } => {
                        self.emit(Op::Ldl, &[slot], at)?;
                        self.emit(Op::Stl, &[arg], at)?;
                    }
                    Val::PendingHost { .. } => {
                        self.err("把函数值当作数据存储", "callee 不是可装箱值".to_string(), at);
                        return Err(());
                    }
                }
                self.set_local_ty(arg, ty);
                Ok(1)
            }
            "STORE_FAST_LOAD_FAST" | "STORE_FAST_STORE_FAST" => {
                let (a, b) = self.fused_names(at).ok_or_else(|| {
                    self.err(format!("{name}"), "融合指令的名字参数无法从 argrepr 还原".to_string(), at)
                })?;
                let v = self.pop(at)?;
                let ty = v.ty();
                self.push_val(&v, at)?;
                self.emit(Op::Stl, &[a], at)?;
                self.set_local_ty(a, ty);
                if name == "STORE_FAST_LOAD_FAST" {
                    let tb = self.local_slot_ty(b);
                    self.emit(Op::Ldl, &[b], at)?;
                    self.stack.push(Val::Stack { ty: tb });
                } else {
                    self.push_val(&Val::Stack { ty }, at)?;
                    self.emit(Op::Stl, &[b], at)?;
                    self.set_local_ty(b, ty);
                }
                Ok(1)
            }
            "DELETE_FAST" => {
                // The local window starts life all-`none` (§7.8.1 `frame_kind 0`), so "the name holds
                // None" is the same write, and no IFBC instruction ever sees an uninitialised slot.
                let pool = self.m.intern("None");
                self.emit(Op::Push, &[pool, self.ty_unit], at)?;
                self.emit(Op::Stl, &[arg], at)?;
                self.set_local_ty(arg, self.ty_unit);
                Ok(1)
            }
            "LOAD_FAST_AND_CLEAR" => {
                // PEP 709's inlined-comprehension preamble: push the current value, and let the
                // epilogue's `STORE_FAST` restore it. Clearing would need an outer scope this frame
                // does not have, so the pair LOAD_FAST_AND_CLEAR / STORE_FAST maps to push and store.
                let ty = self.local_slot_ty(arg);
                self.emit(Op::Ldl, &[arg], at)?;
                self.stack.push(Val::Stack { ty });
                Ok(1)
            }
            "STORE_NAME" | "STORE_GLOBAL" | "LOAD_NAME" => {
                self.err(
                    format!("{name}({})", at.argrepr),
                    "模块级全局写读需要 module 对象模型；能力表 v1 只覆盖单元的参数/局部窗口（§7.7.1 规则 1)"
                        .to_string(),
                    at,
                );
                Err(())
            }
            "LOAD_DEREF" | "STORE_DEREF" | "DELETE_DEREF" | "FREE_VALUE" | "MAKE_CELL"
            | "COPY_FREE_VARS" | "LOAD_CLOSURE" => {
                self.err(
                    format!("{name}({})", at.argrepr),
                    "闭包 cell 模型不在能力表 v1 内（§7.7.2 扫描项 4）".to_string(),
                    at,
                );
                Err(())
            }
            // ---- stack shape -----------------------------------------------------
            "POP_TOP" => {
                let v = self.pop(at)?;
                if matches!(v, Val::Stack { .. }) {
                    self.emit(Op::Pop, &[], at)?;
                }
                Ok(1)
            }
            "SWAP" => {
                if arg != 2 {
                    self.err(
                        format!("SWAP {arg}"),
                        "IFBC 提供 swap/dup2（D.2），更深的换序需要多槽搬迁；能力表 v1 未覆盖该形态".to_string(),
                        at,
                    );
                    return Err(());
                }
                let b = self.pop(at)?;
                let a = self.pop(at)?;
                let ta = a.ty();
                let tb = b.ty();
                self.push_val(&a, at)?;
                self.push_val(&b, at)?;
                self.emit(Op::Swap, &[], at)?;
                self.stack.push(Val::Stack { ty: tb });
                self.stack.push(Val::Stack { ty: ta });
                Ok(1)
            }
            "COPY" => {
                if arg != 1 {
                    self.err(format!("COPY {arg}"), "仅 COPY 1（等价 dup）在能力表内".to_string(), at);
                    return Err(());
                }
                let top = self.stack.last().cloned().ok_or_else(|| {
                    self.err("COPY 于空栈", "code object 栈形状与设计不符".to_string(), at)
                })?;
                self.push_val(&top, at)?;
                let ty = top.ty();
                self.stack.push(Val::Stack { ty });
                Ok(1)
            }
            // ---- unary -----------------------------------------------------------
            "UNARY_NEGATIVE" => {
                let v = self.pop(at)?;
                let ty = self.common_numeric(&[v.ty()]).ok_or_else(|| {
                    self.err("UNARY_NEGATIVE", "操作数不是数值类型（D.3 后缀必须静态可定）".to_string(), at)
                })?;
                self.unbox_to(&v, ty, at)?;
                self.emit(Op::Neg, &[ty], at)?;
                self.box_as(ty, at)?;
                self.stack.push(Val::Stack { ty });
                Ok(1)
            }
            "UNARY_NOT" => {
                let v = self.pop(at)?;
                self.push_val(&v, at)?;
                self.emit(Op::DynTruthy, &[], at)?;
                self.emit(Op::Not, &[self.ty_bool], at)?;
                self.box_as(self.ty_bool, at)?;
                self.stack.push(Val::Stack { ty: self.ty_bool });
                Ok(1)
            }
            // ---- binary ----------------------------------------------------------
            "BINARY_OP" => {
                let r = self.pop(at)?;
                let l = self.pop(at)?;
                let (ls, rs) = (self.to_slot(l, at)?, self.to_slot(r, at)?);
                self.binop(&ls, &rs, at)?;
                Ok(1)
            }
            "COMPARE_OP" => {
                let r = self.pop(at)?;
                let l = self.pop(at)?;
                let (ls, rs) = (self.to_slot(l, at)?, self.to_slot(r, at)?);
                self.binop(&ls, &rs, at)?;
                Ok(1)
            }
            "IS_OP" | "CONTAINS_OP" => {
                let r = self.pop(at)?;
                let l = self.pop(at)?;
                let ls = self.to_slot(l, at)?;
                let rs = self.to_slot(r, at)?;
                self.push_val(&ls, at)?;
                self.push_val(&rs, at)?;
                let negated = at.argrepr.contains("not");
                let ty = if name == "IS_OP" {
                    self.emit(Op::DynIdentity, &[], at)?;
                    self.ty_bool
                } else {
                    self.host_call("std.iter.contains", 2, at)?
                };
                if negated {
                    self.emit(Op::Not, &[self.ty_bool], at)?;
                }
                self.box_as(ty, at)?;
                self.stack.push(Val::Stack { ty });
                Ok(1)
            }
            "TO_BOOL" => {
                let v = self.pop(at)?;
                self.push_val(&v, at)?;
                self.emit(Op::DynTruthy, &[], at)?;
                self.box_as(self.ty_bool, at)?;
                self.stack.push(Val::Stack { ty: self.ty_bool });
                Ok(1)
            }
            // ---- collections -----------------------------------------------------
            "BUILD_LIST" => {
                let n = arg;
                let mut elem = self.ty_dyn;
                for _ in 0..n {
                    let v = self.pop(at)?;
                    elem = v.ty();
                }
                let list_ty = self.m.intern_type(TyKind::List, "list", elem, NONE32);
                self.emit(Op::NewList, &[list_ty, n], at)?;
                self.stack.push(Val::Stack { ty: list_ty });
                Ok(1)
            }
            "BUILD_TUPLE" => {
                let n = arg;
                let mut elem = self.ty_dyn;
                for _ in 0..n {
                    let v = self.pop(at)?;
                    elem = v.ty();
                }
                let t = self.m.intern_type(TyKind::Tuple, "tuple", elem, NONE32);
                self.emit(Op::NewTuple, &[n], at)?;
                self.stack.push(Val::Stack { ty: t });
                Ok(1)
            }
            "BUILD_MAP" | "BUILD_SET" => {
                let n = arg;
                if name == "BUILD_SET" {
                    for _ in 0..n {
                        let v = self.pop(at)?;
                        let _ = v;
                    }
                    let elem = self.ty_dyn;
                    let t = self.m.intern_type(TyKind::List, "set", elem, NONE32);
                    self.emit(Op::NewSet, &[t], at)?;
                    self.stack.push(Val::Stack { ty: t });
                    return Ok(1);
                }
                let mut val_ty = self.ty_dyn;
                let mut key_ty = self.ty_dyn;
                for _ in 0..n {
                    let v = self.pop(at)?;
                    val_ty = v.ty();
                    let k = self.pop(at)?;
                    key_ty = k.ty();
                }
                let dict_ty = self.m.intern_type(TyKind::Dict, "dict", key_ty, val_ty);
                self.emit(Op::NewDict, &[key_ty, val_ty, n], at)?;
                self.stack.push(Val::Stack { ty: dict_ty });
                Ok(1)
            }
            "BINARY_SUBSCR" => {
                let ix = self.pop(at)?;
                let obj = self.pop(at)?;
                let obj_ty = obj.ty();
                let elem = self.elem_of(obj_ty).unwrap_or(self.ty_dyn);
                self.push_val(&obj, at)?;
                self.push_val(&ix, at)?;
                self.emit(Op::IdxGet, &[obj_ty], at)?;
                self.stack.push(Val::Stack { ty: elem });
                Ok(1)
            }
            "STORE_SUBSCR" => {
                let ix = self.pop(at)?;
                let obj = self.pop(at)?;
                let val = self.pop(at)?;
                let obj_ty = obj.ty();
                self.push_val(&obj, at)?;
                self.push_val(&ix, at)?;
                self.push_val(&val, at)?;
                self.emit(Op::IdxSet, &[obj_ty], at)?;
                Ok(1)
            }
            "UNPACK_SEQUENCE" => {
                let n = arg;
                let cv = self.pop(at)?;
                let cont = self.to_slot(cv, at)?;
                let cti = cont.ty();
                let elem = self.elem_of(cti).unwrap_or(self.ty_dyn);
                // Element order matches CPython: item 0 is pushed first, so the last `STORE_FAST`
                // gets the first element, exactly as Python unpacking assigns.
                for i in 0..n {
                    self.push_val(&cont, at)?;
                    self.emit(Op::PushB, &[i], at)?;
                    self.emit(Op::IdxGet, &[cti], at)?;
                    self.stack.push(Val::Stack { ty: elem });
                }
                Ok(1)
            }
            // ---- attributes / calls ---------------------------------------------
            "LOAD_ATTR" | "STORE_ATTR" => {
                let key = dispatch_key(at);
                if key == "method" {
                    self.err(
                        format!("LOAD_ATTR(method)({})", at.argrepr),
                        "绑定方法需要 Python 的 method/描述符协议（对象模型，§7.7.6）；能力表 v1 未覆盖"
                            .to_string(),
                        at,
                    );
                    return Err(());
                }
                let interned = self
                    .co
                    .operand_name(at)
                    .unwrap_or(at.argrepr.as_str())
                    .to_string();
                let nm = self.m.intern(&interned);
                if name == "LOAD_ATTR" {
                    let v = self.pop(at)?;
                    self.push_val(&v, at)?;
                    self.emit(Op::InstGet, &[nm], at)?;
                    let ty = self.ty_dyn;
                    self.stack.push(Val::Stack { ty });
                } else {
                    let val = self.pop(at)?;
                    let obj = self.pop(at)?;
                    self.push_val(&obj, at)?;
                    self.push_val(&val, at)?;
                    self.emit(Op::InstSet, &[nm], at)?;
                }
                Ok(1)
            }
            "LOAD_GLOBAL" => {
                let nm = self
                    .co
                    .operand_name(at)
                    .ok_or_else(|| self.err("LOAD_GLOBAL", "co_names 索引越界".to_string(), at))?
                    .to_string();
                match self.cap.builtin(&nm) {
                    Some(v) if v.starts_with("host:") => {
                        let host = v[5..].to_string();
                        let arity = self
                            .hosts
                            .get(&host)
                            .map(|s| s.params.len())
                            .unwrap_or(0);
                        self.stack.push(Val::PendingHost { name: host, arity });
                        Ok(1)
                    }
                    Some("-") | None => {
                        self.err(
                            format!("builtin({nm})"),
                            "该全局名既不在能力表的 host 映射内，也不是本单元的局部（§7.7.2 扫描项 2）"
                                .to_string(),
                            at,
                        );
                        Err(())
                    }
                    Some(other) => {
                        self.err(
                            format!("builtin({nm})"),
                            format!("能力表给的值 `{other}` 不是 host 路径；内置名一律映射到 call.host"),
                            at,
                        );
                        Err(())
                    }
                }
            }
            "CALL" => {
                let nargs = arg as usize;
                let mut args: Vec<Val> = Vec::new();
                for _ in 0..nargs {
                    args.insert(0, self.pop(at)?);
                }
                let callee = self.pop(at)?;
                let name = match callee {
                    Val::PendingHost { name, arity } => {
                        if arity != nargs {
                            self.err(
                                format!("host({name}) 调用元数"),
                                format!("该处写了 {nargs} 个实参，IFABI 签名是 {arity} 个"),
                                at,
                            );
                            return Err(());
                        }
                        name
                    }
                    _ => {
                        self.err(
                            "CALL(非 host 目标)",
                            "用户函数/绑定方法的调用需要函数对象模型；能力表 v1 未覆盖（§7.7.6）".to_string(),
                            at,
                        );
                        return Err(());
                    }
                };
                let sig = self.hosts.get(&name).cloned().ok_or(())?;
                let tys: Vec<u32> = args.iter().map(|a| a.ty()).collect();
                if !host_accepts(&sig, &tys, self.m) {
                    self.err(
                        format!("host({name}) 实参类型"),
                        format!(
                            "[{}] 不满足 IFABI 签名 {}（§7.8.3 校验项 5 → R-0708）",
                            tys.iter()
                                .map(|t| self.m.type_name(*t).unwrap_or_default())
                                .collect::<Vec<_>>()
                                .join(", "),
                            sig.text()
                        ),
                        at,
                    );
                    return Err(());
                }
                for (i, a) in args.iter().enumerate() {
                    self.push_val(a, at)?;
                    // A numeric IFABI parameter wants the raw value, so the `dyn` gets its D.7 tag
                    // checkpoint and unbox first (UB-020).
                    if sig.params[i].is_numeric() {
                        self.emit(Op::IsDyn, &[a.ty()], at)?;
                        self.emit(Op::UnboxDyn, &[], at)?;
                    }
                }
                let ty = self.host_call(&name, nargs, at)?;
                self.stack.push(Val::Stack { ty });
                Ok(1)
            }
            "IMPORT_NAME" => {
                let modname = at
                    .operand_name(self.co)
                    .unwrap_or("")
                    .to_string();
                let idx = self.m.intern_module_import(&modname);
                self.emit(Op::Import, &[idx], at)?;
                self.stack.push(Val::Stack { ty: self.ty_dyn });
                Ok(1)
            }
            "IMPORT_FROM" => {
                let interned = self
                    .co
                    .operand_name(at)
                    .unwrap_or(at.argrepr.as_str())
                    .to_string();
                let nm = self.m.intern(&interned);
                let top = match self.stack.last().cloned() {
                    Some(v) => v,
                    None => {
                        self.err("IMPORT_FROM 于空栈", "code object 栈形状与设计不符".to_string(), at);
                        return Err(());
                    }
                };
                let cont = self.to_slot(top, at)?;
                self.push_val(&cont, at)?;
                self.emit(Op::ImportAttr, &[nm], at)?;
                let ty = self.ty_dyn;
                self.stack.push(Val::Stack { ty });
                Ok(1)
            }
            // ---- control flow ----------------------------------------------------
            "RETURN_VALUE" => {
                match self.stack.last().cloned() {
                    Some(val) => {
                        self.push_val(&val, at)?;
                        self.stack.pop();
                    }
                    None => {
                        // A bare `return` still hands back exactly one value in Python.
                        let pool = self.m.intern("None");
                        self.emit(Op::Push, &[pool, self.ty_unit], at)?;
                    }
                }
                self.emit(Op::Ret, &[], at)?;
                self.terminated = true;
                Ok(1)
            }
            "RETURN_CONST" => {
                self.load_const(arg, at)?;
                if !self.stack.is_empty() {
                    let v = self.pop(at)?;
                    self.push_val(&v, at)?;
                }
                self.emit(Op::Ret, &[], at)?;
                self.terminated = true;
                Ok(1)
            }
            "JUMP" | "JUMP_FORWARD" | "JUMP_NO_INTERRUPT" | "JUMP_BACKWARD"
            | "JUMP_BACKWARD_NO_INTERRUPT" => {
                let target = self.jump_target(at)?;
                if target <= at.offset || back.contains(&target) {
                    // §7.7.1 item 3: every back edge gets a `tickloop` in front of it. §8.5 ④ and
                    // R-0701's timeout hang off that count.
                    self.emit(Op::TickLoop, &[TICK_QUOTA], at)?;
                }
                self.branch(Op::Jmp, target, at)?;
                Ok(1)
            }
            "POP_JUMP_IF_TRUE" | "POP_JUMP_IF_FALSE" | "POP_JUMP_IF_NONE"
            | "POP_JUMP_IF_NOT_NONE" => {
                let target = self.jump_target(at)?;
                match name {
                    "POP_JUMP_IF_TRUE" => {
                        let v = self.pop(at)?;
                        self.push_val(&v, at)?;
                        self.branch(Op::Br, target, at)?;
                    }
                    "POP_JUMP_IF_FALSE" => {
                        let v = self.pop(at)?;
                        self.push_val(&v, at)?;
                        self.branch(Op::Brf, target, at)?;
                    }
                    _ => {
                        // `is None` / `is not None` become a tag compare, which is exactly the D.7
                        // tag checkpoint the verifier wants before any unbox.
                        let v = self.pop(at)?;
                        self.push_val(&v, at)?;
                        self.emit(Op::TagGet, &[], at)?;
                        let zero = self.ty_int;
                        self.emit(Op::PushB, &[0], at)?;
                        self.emit(Op::CmpEq, &[zero], at)?;
                        if name == "POP_JUMP_IF_NONE" {
                            self.branch(Op::Br, target, at)?;
                        } else {
                            self.branch(Op::Brf, target, at)?;
                        }
                    }
                }
                Ok(1)
            }
            "FOR_ITER" | "SEND" => self.for_iter(at, all, back),
            "END_FOR" | "POP_ITER" => {
                // The iterator lives in a slot in this lowering, so there is nothing to pop; the pair
                // also closes a comprehension's loop, whose epilogue restore ran in `for_iter`.
                Ok(1)
            }
            "GET_ITER" => {
                let v = self.pop(at)?;
                let src = v.ty();
                let elem = self.elem_of(src).unwrap_or(self.ty_dyn);
                self.push_val(&v, at)?;
                let iter_ty = self.m.intern_type(TyKind::Coro, "iter", elem, NONE32);
                self.emit(Op::SeqIter, &[iter_ty], at)?;
                self.stack.push(Val::Stack { ty: iter_ty });
                Ok(1)
            }
            "LIST_APPEND" | "SET_ADD" | "MAP_ADD" => self.collection_append(at, name, all),
            "GET_YIELD_FROM_ITER" | "YIELD_VALUE" | "RETURN_GENERATOR" | "BEFORE_WITH"
            | "LOAD_SPECIAL" | "WITH_EXCEPT_START" | "PUSH_EXC_INFO" | "POP_EXCEPT" | "RERAISE"
            | "CHECK_EXC_MATCH" | "RAISE_VARARGS" | "SETUP_FINALLY" | "MAKE_FUNCTION"
            | "CALL_INTRINSIC_1" | "CALL_KW" | "CALL_FUNCTION_EX" | "FORMAT_VALUE"
            | "FORMAT_SIMPLE" | "BUILD_STRING" | "LIST_EXTEND" | "DICT_UPDATE" | "LIST_TO_TUPLE"
            | "UNPACK_EX" | "DELETE_SUBSCR" | "STORE_SLICE" | "BINARY_SLICE"
            | "MATCH_CLASS" | "MATCH_MAPPING" | "MATCH_SEQUENCE" | "MATCH_KEYS" => {
                self.err(
                    format!("{}({})", at.opname, at.argrepr),
                    "该构造依赖尚未纳入能力表的对象模型语义（§7.7.6）；构建期即红，不留到运行时（P3）"
                        .to_string(),
                    at,
                );
                Err(())
            }
            _ => {
                self.err(
                    format!("{}({})", at.opname, at.argrepr),
                    "该指令没有 lowering 规则；能力表虽覆盖其映射，本实现的机械映射尚未实现其形态 \
                     （§7.7.2：请按诊断列出的具体项扩容）"
                        .to_string(),
                    at,
                );
                Err(())
            }
        }
    }

    /// `LIST_APPEND`/`SET_ADD`/`MAP_ADD`: CPython keeps the container *under* the value, while in this
    /// lowering the container lives in a slot; the mapping therefore parks the value, re-pushes
    /// `container, value` and writes the container back to its slot. Mechanical, and it keeps the
    /// operand-stack height at zero inside a loop body (§7.8.3 item 2).
    fn collection_append(
        &mut self,
        at: &PyInstr,
        name: &str,
        _all: &[PyInstr],
    ) -> Result<usize, ()> {
        let depth = at.arg.unwrap_or(2) as usize;
        let val = self.pop(at)?;
        let vals = self.to_slot(val.clone(), at)?;
        if self.stack.len() < depth {
            self.err(format!("{name} {depth}"), "容器不在预期的栈深度".to_string(), at);
            return Err(());
        }
        let cont_idx = self.stack.len() - depth;
        let cont = self.stack.remove(cont_idx);
        let cont = self.to_slot(cont.clone(), at)?;
        let cty = cont.ty();
        match name {
            "MAP_ADD" => {
                // MAP_ADD pushes (map, key, value): the key is the value below `val`.
                let key = self.stack.remove(self.stack.len() - 2);
                let key = self.to_slot(key.clone(), at)?;
                self.push_val(&cont, at)?;
                self.push_val(&key, at)?;
                self.push_val(&val, at)?;
                self.emit(Op::MapInsert, &[cty], at)?;
                self.pop(at)?;
            }
            _ => {
                self.push_val(&cont, at)?;
                self.push_val(&vals, at)?;
                let elem = self.elem_of(cty).unwrap_or(self.ty_dyn);
                self.emit(Op::SeqPush, &[elem], at)?;
            }
        }
        self.stack.insert(cont_idx, cont);
        Ok(1)
    }

    /// The loop head: `FOR_ITER` becomes a `tickloop`-checked `seq.next` test whose exhausted branch
    /// leaves the operand stack empty, which is what makes the back edge verifiable.
    fn for_iter(
        &mut self,
        at: &PyInstr,
        _all: &[PyInstr],
        _back: &std::collections::BTreeSet<u32>,
    ) -> Result<usize, ()> {
        let target = self.jump_target(at)?;
        // The iterator is the value Python keeps on TOS here; everything on the stack becomes a slot.
        self.park_stack(at)?;
        let it = match self.stack.pop() {
            Some(Val::Slot { slot, ty }) => (slot, ty),
            _ => {
                self.err("FOR_ITER", "迭代器不在模型栈顶（栈形状与设计不符）".to_string(), at);
                return Err(());
            }
        };
        // §7.7.1 item 3: a loop head is a back edge target, so it carries the timeout checkpoint.
        self.emit(Op::TickLoop, &[TICK_QUOTA], at)?;
        // Back edges must land on the test, not on the checkpoint: rebind this offset accordingly.
        self.off_map.insert(at.offset, self.b.offset());
        let elem = self.elem_of(it.1).unwrap_or(self.ty_dyn);
        self.emit(Op::Ldl, &[it.0], at)?;
        self.emit(Op::SeqNext, &[elem], at)?;
        let val_slot = self.alloc_scratch();
        self.emit(Op::Stl, &[val_slot], at)?;
        self.branch_cond(Op::Brf, target, at)?;
        self.stack.push(Val::Slot { slot: val_slot, ty: elem });
        Ok(1)
    }

    fn jump_target(&mut self, at: &PyInstr) -> Result<u32, ()> {
        if let Some(t) = at.argval_int.filter(|v| *v >= 0) {
            return Ok(t as u32);
        }
        self.err(
            format!("{} 的跳转目标无法解析", at.opname),
            "dis 未把 argval 报成绝对偏移：code object 形态与本实现预期不符".to_string(),
            at,
        );
        Err(())
    }

    /// A conditional branch: everything below the tested value is parked, the tested value stays on
    /// the stack because `br`/`brf` pop it themselves (D.4).
    fn branch_cond(&mut self, op: Op, target_py: u32, at: &PyInstr) -> Result<(), ()> {
        let keep = self.stack.pop();
        self.park_stack(at)?;
        if let Some(k) = keep {
            self.push_val(&k, at)?;
        }
        self.branch(op, target_py, at)
    }

    /// One binary Python operator. The capability table's row for the operator names the candidate
    /// IFBC instructions; the operand types decide which candidate applies, and a `div`/`rem`/`shl`/
    /// `shr` result always gets its D.3 guard immediately before it (the verifier refuses the module
    /// without one — UB-003/UB-004's bytecode-side counterpart).
    fn binop(&mut self, l: &Val, r: &Val, at: &PyInstr) -> Result<(), ()> {
        let sel = self.choose(at, &[l.ty(), r.ty()])?;
        match sel {
            Selected::Family(op, ty) => {
                // D.7 / UB-020: every `unbox.dyn` is preceded by its `is.dyn` checkpoint.
                self.push_val(l, at)?;
                self.emit(Op::IsDyn, &[ty], at)?;
                self.emit(Op::UnboxDyn, &[], at)?;
                self.push_val(r, at)?;
                self.emit(Op::IsDyn, &[ty], at)?;
                self.emit(Op::UnboxDyn, &[], at)?;
                // D.3: the boundary checks must be adjacent and may not be dropped (UB-003/UB-004).
                // D-38 (c): the guards themselves carry no operand — the type rides on `op` below.
                match op {
                    Op::Div | Op::Rem => self.emit(Op::GuardNonzero, &[], at)?,
                    Op::Shl | Op::Shr | Op::ShrA => self.emit(Op::GuardShift, &[], at)?,
                    _ => {}
                }
                self.emit(op, &[ty], at)?;
                let res = if op.is_cmp() { self.ty_bool } else { ty };
                self.box_as(res, at)?;
                self.stack.push(Val::Stack { ty: res });
            }
            Selected::Object(op, ty) => {
                self.push_val(l, at)?;
                self.push_val(r, at)?;
                // D-38 moved operand counts (guards/`box.dyn` to 0, `ldw`/`field_off` to 2), so the
                // width comes from the table rather than a hand-written slice. A mapping row that
                // needs more than the one type operand (the container forms `new.list`/`new.dict`,
                // which also want an element count) is refused here instead of emitting a
                // mis-sized instruction.
                let ops: Vec<u32> = match op.arity() {
                    0 => Vec::new(),
                    1 => vec![ty],
                    n => {
                        self.err(
                            format!("{}({})", at.opname, dispatch_key(at)),
                            format!(
                                "能力表映射到 {}，它有 {n} 个操作数，而二元运算的 lowering 只能给出类型操作数；需要一个带元素计数的构造形态",
                                op.base()
                            ),
                            at,
                        );
                        return Err(());
                    }
                };
                self.emit(op, &ops, at)?;
                let res = self.object_result_ty(op, ty);
                self.stack.push(Val::Stack { ty: res });
            }
            Selected::Host(name) => {
                let refs: Vec<&Val> = vec![l, r];
                let ty = self.emit_host(&name, &refs, at)?;
                self.stack.push(Val::Stack { ty });
            }
            Selected::None => {
                self.err(
                    format!("{}({})", at.opname, dispatch_key(at)),
                    "能力表给出的候选映射都不适用于这里的操作数类型（§7.7.2：按诊断列出的具体项扩容）"
                        .to_string(),
                    at,
                );
                return Err(());
            }
        }
        Ok(())
    }

    /// Result type of an object-level instruction (D.6/D.7).
    fn object_result_ty(&mut self, op: Op, container: u32) -> u32 {
        let elem = self.elem_of(container).unwrap_or(self.ty_dyn);
        match op {
            Op::IdxGet | Op::SeqNext | Op::SeqIter => elem,
            Op::SeqLen => self.ty_int,
            Op::DynTruthy | Op::DynIdentity => self.ty_bool,
            _ => container,
        }
    }

    fn local_slot_ty(&self, slot: u32) -> u32 {
        self.local_ty.get(slot as usize).copied().unwrap_or(self.ty_dyn)
    }
    fn set_local_ty(&mut self, slot: u32, ty: u32) {
        if let Some(s) = self.local_ty.get_mut(slot as usize) {
            *s = ty;
        }
    }
}

// ------------------------------------------------------------------ helpers

/// Does this opcode name jump?
pub fn is_jump(i: &PyInstr) -> bool {
    matches!(
        i.opname.as_str(),
        "FOR_ITER"
            | "JUMP"
            | "JUMP_FORWARD"
            | "JUMP_BACKWARD"
            | "JUMP_NO_INTERRUPT"
            | "JUMP_BACKWARD_NO_INTERRUPT"
            | "POP_JUMP_IF_TRUE"
            | "POP_JUMP_IF_FALSE"
            | "POP_JUMP_IF_NONE"
            | "POP_JUMP_IF_NOT_NONE"
            | "SEND"
    )
}

/// `f64` → the decimal text stored in the pool. `{:?}` keeps the round-trip exact (Rust prints the
/// shortest representation that re-parses to the same bits), which is what §7.3 needs.
fn format_f64(f: f64) -> String {
    if f.is_nan() {
        "nan".to_string()
    } else if f.is_infinite() {
        if f > 0.0 { "inf".to_string() } else { "-inf".to_string() }
    } else if f == f.trunc() && f.abs() < 1e15 {
        format!("{f:.1}")
    } else {
        format!("{f:?}")
    }
}

impl PyConst {
    pub fn repr_short(&self) -> String {
        match self {
            PyConst::Int(i) => i.to_string(),
            PyConst::Float(f) => format_f64(*f),
            PyConst::Str(s) => format!("{:?}", if s.len() > 24 { &s[..24] } else { s.as_str() }),
            PyConst::Bool(b) => b.to_string(),
            PyConst::None => "None".to_string(),
            PyConst::Code(c) => format!("code object({})", c.name),
            PyConst::Tuple(_) => "tuple".to_string(),
            PyConst::Bytes(_) => "bytes".to_string(),
            PyConst::BigInt(v) => v.clone(),
            PyConst::Frozenset(_) => "frozenset".to_string(),
            PyConst::Ellipsis => "Ellipsis".to_string(),
            PyConst::Unsupported(u) => u.clone(),
        }
    }
}

fn intern_dyn_for(m: &mut IfbcModule, k: TyKind, elem: Option<TyKind>) -> u32 {
    let e = match elem {
        Some(ek) => intern_kind(m, ek),
        None => NONE32,
    };
    m.intern_type(k, k.suffix(), e, NONE32)
}

/// Whether a host signature may take these argument types: `dyn`-visible parameters take anything,
/// numeric parameters take exactly that numeric kind (§7.7.7's tag boundary).
fn host_accepts(sig: &HostSig, args: &[u32], m: &IfbcModule) -> bool {
    args.len() == sig.params.len()
        && args.iter().enumerate().all(|(i, a)| host_takes(sig, *a, i, m))
}

fn host_takes(sig: &HostSig, arg: u32, i: usize, m: &IfbcModule) -> bool {
    let need = match sig.params.get(i) {
        Some(k) => *k,
        None => return false,
    };
    if need.dyn_tag().is_some() || matches!(need, TyKind::Unit) {
        return true;
    }
    match m.ty_kind(arg) {
        Some(got) => got == need || (matches!(need, TyKind::Int) && matches!(got, TyKind::Int)),
        None => false,
    }
}

/// Reachability over the Python control-flow graph: fallthrough until a terminator, plus jump
/// targets. §7.8.3 item 1 needs IFBC code that is reachable from the frame entry only; the blocks
/// CPython leaves behind for implicit unwinding are handled through the exception table instead.
pub fn reachable(instrs: &[PyInstr]) -> std::collections::BTreeSet<u32> {
    let mut out = std::collections::BTreeSet::new();
    let mut work: Vec<u32> = Vec::new();
    if let Some(f) = instrs.first() {
        work.push(f.offset);
    }
    let index: BTreeMap<u32, usize> = instrs.iter().enumerate().map(|(i, x)| (x.offset, i)).collect();
    while let Some(off) = work.pop() {
        if out.contains(&off) {
            continue;
        }
        let mut cur = off;
        loop {
            if out.contains(&cur) {
                break;
            }
            let Some(&i) = index.get(&cur) else { break };
            let ins = &instrs[i];
            out.insert(cur);
            if matches!(
                ins.opname.as_str(),
                "RETURN_VALUE" | "RETURN_CONST" | "RAISE_VARARGS" | "RERAISE"
            ) {
                break;
            }
            if is_jump(ins) {
                if let Some(t) = ins.argval_int.filter(|v| *v >= 0) {
                    work.push(t as u32);
                }
                let unconditional = matches!(
                    ins.opname.as_str(),
                    "JUMP" | "JUMP_FORWARD" | "JUMP_BACKWARD" | "JUMP_NO_INTERRUPT"
                        | "JUMP_BACKWARD_NO_INTERRUPT"
                );
                if unconditional {
                    break;
                }
            }
            cur = match instrs.get(i + 1) {
                Some(n) => n.offset,
                None => break,
            };
        }
    }
    out
}


// ------------------------------------------------------------------ version lock (§7.7.3)

/// §7.7.3's byte-code version lock, checked before a single instruction is lowered.
///
/// * `manifest.b_track` has no `py` entry while the unit is `@py` → `E-0715`.
/// * the declared minor disagrees with the interpreter, or the `.pyc` magic disagrees with the
///   declared minor's normative value → `E-0713`.
///
/// `py_minor` is the declaration (`"3.13"`), `magic` the value the probe reported. The numbers in
/// [`CodeObjDump::known_magics`] are the normative `.pyc` magics; `3627` (`2b0e0d0a`) is the one
/// observed on this machine with CPython 3.14.6.
pub fn check_declared_version(
    manifest: &Manifest,
    py_minor: &str,
    magic: u32,
) -> Result<(), IngestErr> {
    let declared = manifest
        .b_track
        .get("py")
        .cloned()
        .or_else(|| manifest.b_track.get("cpython").cloned())
        .ok_or_else(|| IngestErr::VersionUndeclared { lang: "py".to_string() })?;
    let observed_magic = CodeObjDump::known_magics()
        .get(py_minor)
        .map(|(m, _)| *m)
        .unwrap_or(0);
    let norm = |v: &str| {
        let parts: Vec<&str> = v.split('.').collect();
        if parts.len() < 2 {
            return String::new();
        }
        format!("{}.{}", parts[0], parts[1])
    };
    if norm(&declared) != norm(py_minor) {
        return Err(IngestErr::VersionMismatch {
            declared,
            observed: py_minor.to_string(),
            magic_declared: observed_magic,
            magic_observed: magic,
        });
    }
    if observed_magic != 0 && magic != observed_magic {
        return Err(IngestErr::VersionMismatch {
            declared,
            observed: py_minor.to_string(),
            magic_declared: observed_magic,
            magic_observed: magic,
        });
    }
    Ok(())
}

/// The same check on a live probe result, which is what `ingest` actually calls.
pub fn check_probe_version(manifest: &Manifest, dump: &CodeObjDump) -> Result<(), IngestErr> {
    check_declared_version(manifest, &dump.py_minor(), dump.pyc_magic)
}

/// §7.7.5's R-JS resolution and §7.5's prefix whitelist.
///
/// `@js`/`@ts`/`@sh`/`@ps1`/`@bat`/`@cmd` are syntactically legal but never enter the byte-code
/// ingest path: they must say `isolation = "process"`, or the build stops with `E-0716`. Prefixes
/// outside the whitelist (`@forth`, `@ruby`, `@php`, `@java` in v2.0) are `E-0702`.
pub fn js_process_only(block: &EmbedBlock, handler: &mut Handler) -> bool {
    let file = block.lang_span.file;
    let pos = Position { line: 1, column: 1 };
    js_process_only_at(block, handler, file, pos)
}

/// Same rule, with the geometry a phase that holds [`Sources`] can supply (§9.2 wants a real
/// caret line; `js_process_only` keeps the signature §7.7.5 asks for and falls back to the unit).
pub fn js_process_only_at(
    block: &EmbedBlock,
    handler: &mut Handler,
    file: FileId,
    pos: Position,
) -> bool {
    let lang = block.lang.as_str();
    match lang {
        // §7.5: the ingest prefixes.
        "py" | "lua" => true,
        // §7.6: the A-track prefixes.
        "c" | "cpp" | "rust" | "zig" => true,
        // §7.7.5 R-JS: process isolation is mandatory, not a default.
        "js" | "ts" | "sh" | "ps1" | "bat" | "cmd" => {
            if has_process_isolation(block) {
                return true;
            }
            handler.emit(
                Diagnostic::new("E-0716", Phase::Embed, file, pos)
                    .with_origin(Origin::EmbedPy)
                    .titled(format!("@{lang} 省略了 isolation = \"process\""))
                    .note(
                        "R-JS：JS/TS 没有任何对外承诺稳定的字节码形态，v2.0 不进字节码摄取路径；\
                         脚本/命令前缀同样与宿主环境深度耦合（§7.7.5）"
                    )
                    .help("补 isolation = \"process\"，或改用 @py/@lua（§7.7.5 R-JS）"),
            );
            false
        }
        other => {
            handler.emit(
                Diagnostic::new("E-0702", Phase::Embed, file, pos)
                    .with_origin(Origin::EmbedPy)
                    .titled(format!("未知或不支持的嵌入前缀 @{other}"))
                    .note("§7.5 的白名单是封闭集合；@java 为 v2.1 候选，@ruby/@php/@forth 明确不支持")
                    .help("用 §7.5 白名单内前缀"),
            );
            false
        }
    }
}

/// `isolation = "process"` as written in the `@embed(...)` argument list.
pub fn has_process_isolation(block: &EmbedBlock) -> bool {
    match block.arg("isolation") {
        Some(e) => match &e.kind {
            crate::ast::ExprKind::Lit(crate::token::Lit::Str(s)) => s.as_str() == "process",
            _ => false,
        },
        None => false,
    }
}

/// §7.9's ban on implicit cross-track calls, kept here so the B-track phase can report `E-0703`
/// with the same wording as the A-track driver.
pub fn cross_track(block: &EmbedBlock, other_track: &str, handler: &mut Handler) {
    let file = block.lang_span.file;
    handler.emit(
        Diagnostic::new("E-0703", Phase::Embed, file, Position { line: 1, column: 1 })
            .with_origin(Origin::EmbedPy)
            .titled("跨轨直接调用")
            .note(format!("@{} 单元直调 {other_track} 轨单元", block.lang))
            .help("经 INFY 侧 event 中转（§7.9）"),
    );
}

// ------------------------------------------------------------------ ZIP (§7.7.4, std only)

/// A ZIP member as read from the **central directory** — the only part of the format that gives
/// names, sizes and offsets without walking local headers (PKZIP APPNOTE 8.6.16).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZipEntry {
    pub name: String,
    /// 0 = stored, 8 = deflate. Anything else is refused.
    pub method: u16,
    pub crc32: u32,
    pub comp_size: u64,
    pub size: u64,
    pub data_off: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZipArchive {
    pub entries: Vec<ZipEntry>,
}

/// Local file header signature, central directory signature, end-of-central-directory signature.
const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_EOCD: u32 = 0x0605_4b50;
/// Zip64 markers we refuse rather than guess (§7.7.4 wants a wheel, not an archive experiment).
const SIG_ZIP64_EOCD: u32 = 0x0606_4b50;

fn le16(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn le32(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

impl ZipArchive {
    /// Parse the central directory. Every length is bounds-checked; a truncated or forged archive
    /// becomes [`IngestErr::Zip`], never a panic (§9's "no unwrap on data").
    pub fn open(bytes: &[u8]) -> Result<ZipArchive, IngestErr> {
        let n = bytes.len();
        if n < 22 {
            return Err(IngestErr::Zip("文件比 EOCD 还短，不是 ZIP 归档".to_string()));
        }
        // EOCD is at the tail, possibly with a comment; scan back at most 64 KiB + 22.
        let floor = n.saturating_sub(22 + 65535);
        let mut at = None;
        let mut i = n - 22;
        loop {
            if le32(bytes, i) == SIG_EOCD {
                at = Some(i);
                break;
            }
            if i <= floor {
                break;
            }
            i -= 1;
        }
        let eocd = at.ok_or_else(|| IngestErr::Zip("找不到 EOCD（结束中央目录记录）".to_string()))?;
        let total_entries = le16(bytes, eocd + 10) as usize;
        let cd_size = le32(bytes, eocd + 12) as usize;
        let cd_off = le32(bytes, eocd + 16) as usize;
        if le32(bytes, eocd.saturating_sub(4)) == SIG_ZIP64_EOCD || total_entries == 0xffff
            || cd_off == 0xffffffff || cd_size == 0xffffffff
        {
            return Err(IngestErr::Zip(
                "zip64 归档不在 §7.7.4 的摄取范围内；wheel 不应需要它".to_string(),
            ));
        }
        if cd_off + cd_size > n {
            return Err(IngestErr::Zip(format!(
                "中央目录越界：起 {cd_off} 长 {cd_size}，文件只有 {n} 字节"
            )));
        }
        let mut entries = Vec::new();
        let mut p = cd_off;
        for _ in 0..total_entries {
            if p + 46 > n || le32(bytes, p) != SIG_CENTRAL {
                return Err(IngestErr::Zip(format!("中央目录第 {} 项头损坏", entries.len())));
            }
            let method = le16(bytes, p + 10);
            let crc = le32(bytes, p + 16);
            let comp = le32(bytes, p + 20) as u64;
            let size = le32(bytes, p + 24) as u64;
            let name_len = le16(bytes, p + 28) as usize;
            let extra_len = le16(bytes, p + 30) as usize;
            let comment_len = le16(bytes, p + 32) as usize;
            let lho = le32(bytes, p + 42) as u64;
            let name_end = p + 46 + name_len;
            if name_end > n {
                return Err(IngestErr::Zip("中央目录成员名越界".to_string()));
            }
            let name = std::str::from_utf8(&bytes[p + 46..name_end])
                .map_err(|_| IngestErr::Zip("成员名不是 UTF-8".to_string()))?
                .to_string();
            entries.push(ZipEntry {
                name,
                method,
                crc32: crc,
                comp_size: comp,
                size,
                data_off: 0,
            });
            p = name_end + extra_len + comment_len;
        }
        // Resolve each member's data start from its local header (variable-length name/extra).
        for e in entries.iter_mut() {
            let o = e.data_off as usize;
            let off = (e.data_off as u64) as usize;
            let _ = o;
            if off + 30 > n || le32(bytes, off) != SIG_LOCAL {
                return Err(IngestErr::Zip(format!("{} 的本地文件头损坏", e.name)));
            }
            let nl = le16(bytes, off + 26) as usize;
            let el = le16(bytes, off + 28) as usize;
            let start = off + 30 + nl + el;
            if start + e.comp_size as usize > n {
                return Err(IngestErr::Zip(format!("{} 的数据区越过文件尾", e.name)));
            }
            e.data_off = start as u64;
        }
        Ok(ZipArchive { entries })
    }

    /// Decompress one member and verify its CRC-32.
    pub fn read(&self, bytes: &[u8], idx: usize) -> Result<Vec<u8>, IngestErr> {
        let e = self
            .entries
            .get(idx)
            .ok_or_else(|| IngestErr::Zip(format!("归档无第 {idx} 项")))?;
        let raw = &bytes[e.data_off as usize..(e.data_off + e.comp_size) as usize];
        let out = match e.method {
            0 => raw.to_vec(),
            8 => inflate(raw, e.size as usize)?,
            m => {
                return Err(IngestErr::Zip(format!(
                    "{} 的压缩方法 {m} 不受支持（§7.7.4 只需 stored/deflate）",
                    e.name
                )))
            }
        };
        if out.len() as u64 != e.size {
            return Err(IngestErr::Zip(format!(
                "{} 解压得 {} 字节，声明 {} 字节",
                e.name,
                out.len(),
                e.size
            )));
        }
        let got = crc32(&out);
        if got != e.crc32 {
            return Err(IngestErr::Zip(format!(
                "{} 的 CRC-32 不符（存档 {:#010x}，实得 {:#010x}）",
                e.name, e.crc32, got
            )));
        }
        Ok(out)
    }
}

/// CRC-32/ISO-HDLC, the polynomial ZIP uses. Table-free so nothing has to be initialised lazily in a
/// way that could differ between runs (§7.3).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for b in data {
        crc ^= *b as u32;
        for _ in 0..8 {
            let mask = (crc & 1) as u32;
            crc >>= 1;
            crc ^= if mask != 0 { 0xedb8_8320 } else { 0 };
        }
    }
    !crc
}

// ------------------------------------------------------------------ inflate (RFC 1951)

/// Deflate is needed because wheels are compressed: §7.7.4 takes a `pip download` artifact. It is
/// implemented here (std only) rather than refused, so `infy ingest py-wheel` works offline.
struct BitReader<'a> {
    b: &'a [u8],
    byte: usize,
    bit: u32,
}

impl<'a> BitReader<'a> {
    fn new(b: &'a [u8]) -> BitReader<'a> {
        BitReader { b, byte: 0, bit: 0 }
    }
    fn need_bytes(&self, n: usize) -> Result<(), IngestErr> {
        if self.byte + n <= self.b.len() {
            Ok(())
        } else {
            Err(IngestErr::Zip("deflate 流要求的字节数超出成员长度".to_string()))
        }
    }
    /// LSB-first, as RFC 1951 requires.
    fn bits(&mut self, n: u32) -> Result<u32, IngestErr> {
        if n == 0 {
            return Ok(0);
        }
        self.need_bytes(((n + self.bit + 7) / 8) as usize)?;
        let mut v = 0u32;
        let mut left = n;
        let mut filled = 0u32;
        while left > 0 {
            let take = left.min(8 - self.bit);
            let mut chunk = 0u32;
            for i in 0..take {
                let bitpos = self.bit + i;
                if (self.b[self.byte] >> bitpos) & 1 == 1 {
                    chunk |= 1 << i;
                }
            }
            v |= chunk << filled;
            filled += take;
            left -= take;
            self.bit += take;
            if self.bit == 8 {
                self.bit = 0;
                self.byte += 1;
            }
        }
        Ok(v)
    }
    fn align_byte(&mut self) {
        if self.bit != 0 {
            self.bit = 0;
            self.byte += 1;
        }
    }
    fn u8(&mut self) -> Result<u8, IngestErr> {
        self.need_bytes(1)?;
        let v = self.b[self.byte];
        self.byte += 1;
        Ok(v)
    }
}

/// Canonical Huffman decoder in RFC 1951's counts/offsets form: 15 levels, no tree nodes, so both
/// the table build and the decode are deterministic.
#[derive(Clone, Default)]
struct Huffman {
    counts: [u32; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn from_lengths(lens: &[u8]) -> Result<Huffman, IngestErr> {
        let mut h = Huffman { counts: [0; 16], symbols: vec![0u16; lens.len().max(1)] };
        for l in lens {
            if *l > 15 {
                return Err(IngestErr::Zip(format!("deflate 码长 {} 超过 15", *l)));
            }
            h.counts[*l as usize] += 1;
        }
        if h.counts[0] == lens.len() as u32 {
            return Ok(h);
        }
        let mut offs = [0u32; 16];
        for i in 1..16 {
            offs[i] = offs[i - 1] + h.counts[i - 1];
        }
        for (sym, l) in lens.iter().enumerate() {
            if *l != 0 {
                h.symbols[offs[*l as usize] as usize] = sym as u16;
                offs[*l as usize] += 1;
            }
        }
        Ok(h)
    }

    fn decode(&self, br: &mut BitReader) -> Result<u16, IngestErr> {
        let mut code = 0i32;
        let mut first = 0i32;
        let mut index = 0i32;
        for len in 1..16 {
            index += first;
            first += self.counts[len] as i32;
            let bit = br.bits(1)? as i32;
            code = (code << 1) | bit;
            if code - first < 0 {
                let off = (code - first + index) as usize;
                return self
                    .symbols
                    .get(off)
                    .copied()
                    .ok_or_else(|| IngestErr::Zip("deflate 霍夫曼表越界".to_string()));
            }
        }
        Err(IngestErr::Zip("deflate 码字超出 15 层上限".to_string()))
    }
}

const LEN_BASE: [u16; 30] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258, 258,
];
const LEN_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// Inflate one raw deflate stream. `expect` is the central directory's uncompressed size, which also
/// bounds the work, so a compressed bomb cannot make the build run away (§7.7.4 is a build path).
pub fn inflate(raw: &[u8], expect: usize) -> Result<Vec<u8>, IngestErr> {
    let mut br = BitReader::new(raw);
    let mut out: Vec<u8> = Vec::new();
    let mut last;
    loop {
        last = br.bits(1)?;
        let typ = br.bits(2)?;
        match typ {
            0 => {
                br.align_byte();
                let len = br.bits(16)? as usize;
                let _nlen = br.bits(16)?;
                br.need_bytes(len)?;
                for _ in 0..len {
                    out.push(br.u8()?);
                }
            }
            1 | 2 => {
                let (lit, dist) = if typ == 1 {
                    let mut ll = [0u8; 288];
                    for (i, l) in ll.iter_mut().enumerate() {
                        *l = if i < 144 {
                            8
                        } else if i < 256 {
                            9
                        } else if i < 280 {
                            7
                        } else {
                            8
                        };
                    }
                    let dl = [5u8; 30];
                    (Huffman::from_lengths(&ll)?, Huffman::from_lengths(&dl)?)
                } else {
                    self_tables(&mut br)?
                };
                loop {
                    let s = lit.decode(&mut br)?;
                    if s < 256 {
                        if out.len() >= expect {
                            return Err(IngestErr::Zip(format!(
                                "deflate 输出超过声明长度 {expect}"
                            )));
                        }
                        out.push(s as u8);
                        continue;
                    }
                    if s == 256 {
                        break;
                    }
                    let idx = (s as usize - 257).min(29);
                    let len = LEN_BASE[idx] as usize + br.bits(LEN_EXTRA[idx] as u32)? as usize;
                    let didx = dist.decode(&mut br)? as usize;
                    let didx = didx.min(29);
                    let d = DIST_BASE[didx] as usize + br.bits(DIST_EXTRA[didx] as u32)? as usize;
                    if d == 0 || d > out.len() {
                        return Err(IngestErr::Zip(format!(
                            "deflate 回引用越界：距离 {d}，已产出 {} 字节",
                            out.len()
                        )));
                    }
                    for _ in 0..len {
                        if out.len() >= expect {
                            return Err(IngestErr::Zip(format!(
                                "deflate 输出超过声明长度 {expect}"
                            )));
                        }
                        let b = out[out.len() - d];
                        out.push(b);
                    }
                }
            }
            other => return Err(IngestErr::Zip(format!("deflate 块类型 {other} 非法"))),
        }
        if last != 0 {
            break;
        }
    }
    Ok(out)
}

/// Dynamic-block code-length tables (RFC 1951 3.2.7).
fn self_tables(br: &mut BitReader) -> Result<(Huffman, Huffman), IngestErr> {
    let hlit = br.bits(5)? as usize + 257;
    let hdist = br.bits(5)? as usize + 1;
    let hclen = br.bits(4)? as usize + 4;
    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let mut clen = [0u8; 19];
    for i in 0..hclen {
        clen[ORDER[i]] = br.bits(3)? as u8;
    }
    let order = Huffman::from_lengths(&clen)?;
    let mut lens = vec![0u8; hlit + hdist];
    let mut n = 0usize;
    while n < lens.len() {
        let s = order.decode(br)? as usize;
        match s {
            0..=15 => {
                lens[n] = s as u8;
                n += 1;
            }
            16 => {
                if n == 0 {
                    return Err(IngestErr::Zip("deflate 重复码出现在码长表开头".to_string()));
                }
                let prev = lens[n - 1];
                let c = 3 + br.bits(2)? as usize;
                for _ in 0..c {
                    if n >= lens.len() {
                        return Err(IngestErr::Zip("deflate 码长表溢出".to_string()));
                    }
                    lens[n] = prev;
                    n += 1;
                }
            }
            17 | 18 => {
                let c = if s == 17 {
                    3 + br.bits(3)? as usize
                } else {
                    11 + br.bits(7)? as usize
                };
                for _ in 0..c {
                    if n >= lens.len() {
                        return Err(IngestErr::Zip("deflate 码长表溢出".to_string()));
                    }
                    lens[n] = 0;
                    n += 1;
                }
            }
            other => return Err(IngestErr::Zip(format!("deflate 码长符号 {other} 非法"))),
        }
    }
    Ok((
        Huffman::from_lengths(&lens[..hlit])?,
        Huffman::from_lengths(&lens[hlit..])?,
    ))
}

// ------------------------------------------------------------------ ingest pipeline

/// What one ingest run produced. `gaps` is what `infy ingest probe` exists to report: the scan's
/// answer *before* the build is asked to fail (§7.7.2 CI early warning).
#[derive(Clone, Debug, Default)]
pub struct IngestReport {
    pub wheel_tag: String,
    pub module_path: String,
    pub version: String,
    pub py_minor: String,
    /// §7.7.3: this string is an input to §7.3's build-id, so changing interpreter changes build-id.
    pub source_toolchain: String,
    pub cap_format: String,
    pub cap_version: u32,
    pub modules: Vec<ModuleReport>,
    pub gaps: Vec<ScanViolation>,
    /// §5.8 item 1 over the emitted tree, so an ingested `.infib` is addressable by dirhash exactly
    /// like a hand-written module@version (§7.7.4's last bullet).
    pub dirhash: String,
    pub out_dir: Option<PathBuf>,
    pub files_written: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleReport {
    pub name: String,
    pub funcs: usize,
    pub code_len: usize,
    pub line_marks: usize,
    pub exceptions: usize,
    pub module_hash: String,
    pub py_file: String,
}

/// §7.7.4: `infy ingest py-wheel <pkg-1.2.3-py3-none-any.whl> --as <path> [--py 3.13]`.
pub fn ingest_py_wheel(
    whl: &Path,
    as_path: &str,
    py: &str,
    out_dir: &Path,
) -> Result<IngestReport, IngestErr> {
    ingest_inner(whl, as_path, py, Some(out_dir))
}

/// `infy ingest probe`: the same scan, no output, gaps reported instead of fatal (CI early warning).
pub fn ingest_probe(whl: &Path, as_path: &str, py: &str) -> Result<IngestReport, IngestErr> {
    ingest_inner(whl, as_path, py, None)
}

/// Pure-Python wheel tags accepted by §7.7.4. `py3-none-any` (and the `py2.py3-none-any` form some
/// projects still publish) means "no interpreter ABI, no platform": every member is `.py`.
pub fn is_pure_python_tag(tag: &str) -> bool {
    let parts: Vec<&str> = tag.split('-').collect();
    if parts.len() != 3 {
        return false;
    }
    let (pyabi, abi, plat) = (parts[0], parts[1], parts[2]);
    plat == "any" && abi == "none" && {
        pyabi.split('.').all(|p| p == "py3" || p == "py2.py3" || p == "py2")
            && !pyabi.is_empty()
    }
}

/// Binary members that make a wheel an A-track problem instead (§7.7.4 → `E-0714`).
pub fn is_binary_member(name: &str) -> bool {
    const EXT: &[&str] = &[
        ".pyd", ".so", ".dll", ".dylib", ".exe", ".pyd.gz", ".o", ".a", ".lib", ".pyd?",
    ];
    let lower = name.to_lowercase();
    EXT.iter().any(|e| lower.ends_with(e)) || lower.ends_with(".cp313-win_amd64")
}

fn wheel_name_parts(whl: &Path) -> Result<(String, String, String), IngestErr> {
    let stem = whl
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| IngestErr::Zip("wheel 路径无法解名".to_string()))?
        .strip_suffix(".whl")
        .ok_or_else(|| IngestErr::Zip("文件名不是 .whl".to_string()))?
        .to_string();
    let mut parts: Vec<String> = stem.split('-').map(|s| s.to_string()).collect();
    if parts.len() < 5 {
        return Err(IngestErr::Zip(format!(
            "wheel 文件名 `{stem}` 不符合 {{name}}-{{ver}}-{{py}}-{{abi}}-{{plat}}.whl"
        )));
    }
    if parts.len() == 6 {
        parts.remove(0); // build tag is not part of the compatibility tag triple
    }
    let tag = format!("{}-{}-{}", parts[2], parts[3], parts[4]);
    if !is_pure_python_tag(&tag) {
        let binary: Vec<String> = parts[2..]
            .iter()
            .filter(|p| p.starts_with('c') || p.contains("linux") || p.contains("win"))
            .cloned()
            .collect();
        return Err(IngestErr::BinaryWheel {
            tag,
            offending: if binary.is_empty() {
                vec![parts[2].clone()]
            } else {
                binary
            },
        });
    }
    Ok((parts[0].clone(), parts[1].clone(), tag))
}

fn ingest_inner(
    whl: &Path,
    as_path: &str,
    py: &str,
    out: Option<&Path>,
) -> Result<IngestReport, IngestErr> {
    let (name, version, tag) = wheel_name_parts(whl)?;
    let bytes = std::fs::read(whl).map_err(|e| IngestErr::Io(format!("读取 wheel 失败：{e}")))?;
    let zip = ZipArchive::open(&bytes)?;
    let binary: Vec<String> = zip
        .entries
        .iter()
        .filter(|e| is_binary_member(&e.name))
        .map(|e| e.name.clone())
        .collect();
    if !binary.is_empty() {
        return Err(IngestErr::BinaryWheel { tag, offending: binary });
    }
    let mut members: Vec<(String, usize)> = zip
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.name.ends_with('/') && e.name.ends_with(".py"))
        .map(|(i, e)| (e.name.clone(), i))
        .collect();
    members.sort();
    if members.is_empty() {
        return Err(IngestErr::Zip("wheel 里没有 .py 成员".to_string()));
    }
    let python = find_python().ok_or_else(|| IngestErr::ToolchainMissing {
        lang: "py".to_string(),
        declared: py.to_string(),
    })?;
    let cap = CapTable::load("py", py)?;
    let hosts = HostSigTable::std_default();
    let mut report = IngestReport {
        wheel_tag: tag.clone(),
        module_path: if as_path.is_empty() {
            name.clone()
        } else {
            as_path.to_string()
        },
        version: version.clone(),
        py_minor: py.to_string(),
        source_toolchain: format!("CPython {}.0", py),
        cap_format: cap.format.clone(),
        cap_version: cap.cap_version,
        ..Default::default()
    };
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut gaps: Vec<ScanViolation> = Vec::new();
    for (mname, idx) in &members {
        let src = String::from_utf8(zip.read(&bytes, *idx)?)
            .map_err(|_| IngestErr::Zip(format!("{mname} 不是 UTF-8")))?;
        let dump = run_python_probe_named(&python, &src, mname)?;
        report.source_toolchain = format!("CPython {}", dump.python_version);
        if dump.pyc_magic != CodeObjDump::known_magics().get(py).map(|(m, _)| *m).unwrap_or(dump.pyc_magic)
        {
            return Err(IngestErr::VersionMismatch {
                declared: py.to_string(),
                observed: dump.py_minor(),
                magic_declared: CodeObjDump::known_magics()
                    .get(py)
                    .map(|(m, _)| *m)
                    .unwrap_or(0),
                magic_observed: dump.pyc_magic,
            });
        }
        let v = capability_scan(&dump, &cap);
        if out.is_none() || !v.is_empty() {
            gaps.extend(v);
            if out.is_none() {
                continue;
            }
            return Err(IngestErr::Scan(gaps));
        }
        let m = lower_named(&dump, &cap, &hosts, mname).map_err(IngestErr::Scan)?;
        let image = m.encode();
        let rel = format!("ifbc/{}", mname.replace('/', "_"));
        let rel_src = format!("source/{mname}");
        report.modules.push(ModuleReport {
            name: mname.clone(),
            funcs: m.funcs.len(),
            code_len: m.code.len(),
            line_marks: m.lines.len(),
            exceptions: m.exceptions.len(),
            module_hash: crate::sha256::hex(&m.normalised()),
            py_file: mname.clone(),
        });
        files.push((rel_src, src.into_bytes()));
        files.push((rel, image));
    }
    report.gaps = gaps;
    if out.is_none() {
        return Ok(report);
    }
    if !report.gaps.is_empty() {
        return Err(IngestErr::Scan(report.gaps.clone()));
    }
    let out_dir = match out {
        Some(d) => d,
        None => return Ok(report),
    };
    report.dirhash = infib_dirhash(&mut files);
    write_infib(out_dir, &report, &mut files)?;
    report.files_written = files.iter().map(|(p, _)| p.clone()).collect();
    Ok(report)
}

/// The `.infib` container of an ingested wheel: appendix E.2's `INFIB-META/manifest.toml`, plus the
/// two editions §5.6 defines — `source/` (the wheel's `.py`, which is the *source* edition's input)
/// and `ifbc/` (the lowered images, the *wheel* edition). §5.8's dirhash is taken over the unpacked
/// logical tree, so the container form never changes the digest.
pub fn write_infib(
    out_dir: &Path,
    report: &IngestReport,
    files: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), IngestErr> {
    let manifest = infib_manifest(report);
    files.push(("INFIB-META/manifest.toml".to_string(), manifest.into_bytes()));
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let root = out_dir.join(format!("{}-{}.infib.d", report.module_path.replace('/', "_"), report.version));
    for (rel, body) in files.iter() {
        let p = root.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| IngestErr::Io(format!("{dir:?}: {e}")))?;
        }
        std::fs::write(&p, body).map_err(|e| IngestErr::Io(format!("{p:?}: {e}")))?;
    }
    Ok(())
}

/// appendix E.2, in the field order the toolchain prints (sorted maps, no timestamps — §7.3).
pub fn infib_manifest(report: &IngestReport) -> String {
    let mut o = String::new();
    o.push_str("# INFIB-META/manifest.toml — appendix E.2\n");
    o.push_str("[lib]\n");
    o.push_str(&format!("path = \"{}\"\n", report.module_path));
    o.push_str(&format!("version = \"{}\"\n", report.version));
    o.push_str("edition = \"wheel\"\n");
    o.push_str(&format!("ifabi = {}\n", crate::VERSION.ifabi));
    o.push_str(&format!(
        "ifbc_caps = [\"{}:{}\"]\n",
        report.cap_format, report.cap_version
    ));
    o.push_str("[lib.profiles]\n");
    o.push_str("overflow = \"panic\"\n");
    o.push_str("repr_default = \"rust\"\n");
    o.push_str("fast_math = false\n");
    for m in &report.modules {
        o.push_str("[[code]]\n");
        o.push_str(&format!(
            "target = \"{}\"\n",
            std::env::consts::ARCH
        ));
        o.push_str(&format!(
            "compiler = \"infyc {} (b_track ingest, {})\"\n",
            crate::VERSION.version, report.source_toolchain
        ));
        o.push_str(&format!("abi_hash = \"sha256:{}\"\n", m.module_hash));
        o.push_str(&format!("source = \"{}\"\n", m.py_file));
    }
    o
}

/// §5.8 item 1 over the ingest's own file tree, so `infy ingest` output is hashable exactly like a
/// hand-written library's (that is the clause making "上游内容变更同样会被 E-0560 拦住" true).
pub fn infib_dirhash(files: &mut Vec<(String, Vec<u8>)>) -> String {
    crate::pkg::dirhash(files)
}

/// The glue stage [6a] calls for one `@py` unit: version lock, scan, lowering. Every failure is a
/// `§9` diagnostic; nothing here can produce a runtime surprise (§7.7.2, P3).
pub fn build_embed_unit(
    body: &str,
    params: &[&str],
    unit_name: &str,
    manifest: &Manifest,
    python: &Path,
    hosts: &HostSigTable,
    file: FileId,
    pos: Position,
    handler: &mut Handler,
) -> Result<IfbcModule, IngestErr> {
    // §7.7.3: an `@py` unit needs `[b_track] py = "<minor>"` in the manifest — undeclared is a
    // refusal, never a guessed version. The minor itself is taken from the probe afterwards,
    // because that is what actually compiled the unit.
    if !manifest.b_track.contains_key("py") {
        let d = Diagnostic::new("E-0715", Phase::Embed, file, pos)
            .with_origin(Origin::EmbedPy)
            .titled("使用 @py 但清单无 [b_track] 条目")
            .note("§7.7.3 要求精确到 minor 的版本声明；未声明即拒绝构建")
            .help("在 MODULE.infy.toml 写 [b_track] py = \"3.13\"（§7.7.3）");
        handler.emit(d);
        return Err(IngestErr::VersionUndeclared { lang: "py".to_string() });
    }
    let src = wrap_embed_unit(body, params, unit_name);
    // The probe selects the unit by *name* (`unit_code()` looks for the wrapper the block was
    // compiled into), so it has to be told the name `wrap_embed_unit` just wrote. Asking it for
    // `__infy_unit__` instead silently returns the module code object, which is the wrapper's
    // `MAKE_FUNCTION` stream — not the unit body — and then no opcode, builtin or import inside the
    // block is ever scanned.
    let dump = run_python_probe_named(python, &src, unit_name).map_err(|e| {
        handler.emit(e.diagnostic(file, pos));
        e
    })?;
    check_probe_version(manifest, &dump).map_err(|e| {
        handler.emit(e.diagnostic(file, pos));
        e.clone()
    })?;
    // §7.7.3 makes the declaration **patch-insensitive** (`check_probe_version` normalises it), so
    // the capability table is looked up by the minor that actually compiled the unit — `3.14.6`
    // declaring a `py = "3.14.6"` module used to ask for `IFBC-CAP-py-3.14.6.toml` and die with
    // `E-0712 能力表无法读取` while the version check itself had passed.
    let cap = CapTable::load("py", &dump.py_minor()).map_err(|e| {
        handler.emit(e.diagnostic(file, pos));
        e
    })?;
    match lower_named(&dump, &cap, &hosts, &format!("b_track/{unit_name}.py")) {
        Ok(m) => Ok(m),
        Err(v) => {
            for x in &v {
                handler.emit(x.diagnostic(file, pos));
            }
            Err(IngestErr::Scan(v))
        }
    }
}

/// Line round-trip: the original-language line behind an IFBC code offset (§7.4, §12.2's "行映射回
/// 原语言"). `None` is the `W-0705` case, never an error.
pub fn original_line(m: &IfbcModule, code_off: u32) -> Option<u32> {
    m.line_of_code_off(code_off)
}

// ------------------------------------------------------------------ stage [6a]: the front end's entry

use crate::ast::Prim;
use crate::sema::Ty;

/// §7.1 stage [6a]: one B-track embed unit **as the INFY side declares it**. §7.7.1 rule 1 is the
/// reason this type exists: the IFBC parameter table comes from the INFY-side types, never from
/// anything read back out of the code object, so the front end hands over names plus `TyKind`s and
/// this module never has to guess one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbedSite {
    /// `py` or `lua` — §7.5's whitelist, B-track half (§7.7.1).
    pub lang: String,
    /// The wrapper name the probe compiles: `name = "…"` when written, else the enclosing item.
    pub unit_name: String,
    /// The block body, verbatim (§2.11: lexically opaque, braces excluded).
    pub body: String,
    /// `(python parameter name, INFY-side kind)` in the order `@py(…)` wrote them.
    pub params: Vec<(String, TyKind)>,
    /// The enclosing item's declared return type, or `Unit` when it declares none (§7.7.1 rule 2).
    pub ret: TyKind,
    pub file: FileId,
    pub pos: Position,
}

impl EmbedSite {
    /// §7.7.1 rule 1 as the `HostSigTable` row `unit_signature` looks up by unit name.
    pub fn host_sig(&self) -> HostSig {
        let params: Vec<TyKind> = self.params.iter().map(|(_, k)| *k).collect();
        HostSig::new(&self.unit_name, &params, self.ret)
    }

    /// The wrapper's positional parameter names, in table order.
    pub fn param_names(&self) -> Vec<&str> {
        self.params.iter().map(|(n, _)| n.as_str()).collect()
    }
}

/// What stage [6a] produces for one unit: the image bytes, and the §7.7.3 `source_toolchain`
/// string that §7.3 requires inside the `build-id` input set ("换 Python 版本必须换 build-id").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngestedImage {
    pub unit: String,
    pub image: Vec<u8>,
    pub source_toolchain: String,
    /// Function records in the image (the unit plus every nested code object that lowered).
    pub n_funcs: usize,
}

/// §7.7.1 rule 1: map an INFY-side type onto IFBC's static `TyKind`. `None` means **not
/// expressible** — §2 of `runtime/IFBC.md` is a closed 18-row set with no 128-bit width, and
/// `auto` / unresolved types carry no information. The caller refuses those; boxing a guess would
/// be exactly the "假成功" P3 forbids.
pub fn embed_ty_kind(t: &Ty) -> Option<TyKind> {
    Some(match t {
        Ty::Prim(p) => match p {
            Prim::I8 => TyKind::I8,
            Prim::I16 => TyKind::I16,
            Prim::I32 => TyKind::I32,
            Prim::I64 => TyKind::Int,
            Prim::U8 => TyKind::U8,
            Prim::U16 => TyKind::U16,
            Prim::U32 => TyKind::U32,
            Prim::U64 => TyKind::U64,
            Prim::Usize => TyKind::Usize,
            Prim::Isize => TyKind::Isize,
            Prim::F32 => TyKind::F32,
            Prim::F64 => TyKind::Float,
            Prim::Bool => TyKind::Bool,
            Prim::Char => TyKind::Char,
            Prim::Str => TyKind::Str,
            Prim::Unit => TyKind::Unit,
            Prim::U128 | Prim::I128 => return None,
        },
        Ty::Bool => TyKind::Bool,
        Ty::Unit => TyKind::Unit,
        Ty::Str => TyKind::Str,
        // Owned `String` and borrowed `str` are the same dyn-visible text at the boundary (`DY_STR`,
        // §7.7.7); the parameter table only needs the value class.
        Ty::String => TyKind::Str,
        // A shared borrow is the same value view — §4.2's `&T` has no separate IFABI row.
        Ty::Ref { inner, .. } => embed_ty_kind(inner)?,
        Ty::Ptr { .. } => TyKind::Ptr,
        Ty::Slice { .. } | Ty::Array { .. } | Ty::Vec { .. } => TyKind::List,
        Ty::Tuple(..) => TyKind::Tuple,
        Ty::Option { .. } => TyKind::Option,
        Ty::Result { .. } => TyKind::Result,
        Ty::Boxed { .. } => TyKind::Box,
        Ty::Rc { atomic, .. } => {
            if *atomic {
                TyKind::Arc
            } else {
                TyKind::Rc
            }
        }
        Ty::Fn { .. } | Ty::Closure(_) => TyKind::Func,
        Ty::Dyn(_) => TyKind::Dyn,
        Ty::Adt(_) | Ty::Named(_) => TyKind::Instance,
        Ty::Never => TyKind::Never,
        Ty::Auto | Ty::IntLit | Ty::Generic(_) | Ty::Invalid => return None,
    })
}

/// §7.1 [6a] for one site: put the declared table into the host signature table, let CPython's own
/// compiler take the wrapped body, run §7.7.2's scan, lower, and encode. Every failure is already a
/// §9 diagnostic on `handler`; `Err` means "the build stops here", never a partial image.
pub fn ingest_embed_site(
    site: &EmbedSite,
    manifest: &Manifest,
    python: &Path,
    handler: &mut Handler,
) -> Result<IngestedImage, IngestErr> {
    let hosts = HostSigTable::std_default().with_unit_sig(&site.unit_name, site.host_sig());
    let names = site.param_names();
    let m = build_embed_unit(
        &site.body,
        &names,
        &site.unit_name,
        manifest,
        python,
        &hosts,
        site.file,
        site.pos,
        handler,
    )?;
    let n_funcs = m.funcs.len();
    Ok(IngestedImage {
        unit: site.unit_name.clone(),
        image: m.encode(),
        source_toolchain: format!(
            "CPython {}",
            probe_version(python).unwrap_or_else(|| manifest.b_track.get("py").cloned().unwrap_or_default())
        ),
        n_funcs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    /// §7.7.3: stage [6a] is a *build-time* ingest, so these tests need a real interpreter — the same
    /// environment dependency the C harnesses have on `gcc`. A missing interpreter is a red test,
    /// never a silently skipped one.
    fn python() -> PathBuf {
        find_python().expect(
            "阶段 [6a] 需要构建期 CPython（§7.7.3）：PATH 上没有 python/python3，也没有 INFY_PYTHON",
        )
    }

    fn cache() -> &'static Mutex<BTreeMap<String, CodeObjDump>> {
        static CACHE: OnceLock<Mutex<BTreeMap<String, CodeObjDump>>> = OnceLock::new();
        CACHE.get_or_init(Default::default)
    }

    /// One interpreter round-trip per distinct unit source, shared by the tests below.
    fn probe(body: &str, params: &[&str]) -> CodeObjDump {
        let src = wrap_embed_unit(body, params, "__infy_unit__");
        let key = format!("{}\u{0}{src}", params.join(","));
        if let Some(d) = cache().lock().unwrap().get(&key) {
            return d.clone();
        }
        let d = run_python_probe(&python(), &src)
            .unwrap_or_else(|e| panic!("探针失败（§7.7.1）：{e}\n送入的源：{src}"));
        cache().lock().unwrap().insert(key, d.clone());
        d
    }

    fn cap(minor: &str) -> CapTable {
        CapTable::load("py", minor).unwrap_or_else(|e| panic!("能力表 IFBC-CAP-py-{minor} 读不回：{e}"))
    }

    /// §7.7.3: an `@py` unit needs `[b_track] py = "<minor>"` in the manifest.
    fn manifest_py(decl: Option<&str>) -> Manifest {
        let text = match decl {
            Some(d) => format!("[module]\npath = \"example/btrack\"\n[b_track]\npy = \"{d}\"\n"),
            None => "[module]\npath = \"example/btrack\"\n".to_string(),
        };
        let mut h = crate::diag::Handler::default();
        crate::pkg::parse_manifest(&text, crate::pkg::MANIFEST_NAME, &mut h)
            .unwrap_or_else(|| panic!("清单夹具应能解析：{text}"))
    }

    fn code_obj(name: &str) -> CodeObj {
        CodeObj {
            name: name.to_string(),
            argcount: 0,
            posonlyargcount: 0,
            kwonlyargcount: 0,
            nlocals: 0,
            stacksize: 0,
            flags: 0,
            firstlineno: 1,
            code: Vec::new(),
            varnames: Vec::new(),
            names: Vec::new(),
            freevars: Vec::new(),
            cellvars: Vec::new(),
            consts: Vec::new(),
            instrs: Vec::new(),
            lines: Vec::new(),
            exc: Vec::new(),
        }
    }

    /// A hand-built dump, for the rules that do not need an interpreter.
    fn dump_of(co: CodeObj) -> CodeObjDump {
        CodeObjDump {
            python_version: "3.14.6".to_string(),
            version_info: (3, 14, 6),
            pyc_magic: 3627,
            pyc_magic_hex: "2b0e0d0a".to_string(),
            unit: "__infy_unit__".to_string(),
            module: co,
            raw_json: String::new(),
        }
    }

    fn instr(opname: &str, opcode: u32, arg: Option<u32>, argrepr: &str, offset: u32) -> PyInstr {
        PyInstr {
            opname: opname.to_string(),
            opcode,
            arg,
            argrepr: argrepr.to_string(),
            argval_int: None,
            offset,
            start_line: Some(2),
            end_line: Some(2),
            col: Some(1),
            end_col: Some(9),
            is_jump_target: false,
        }
    }

    fn items(vs: &[ScanViolation]) -> Vec<(String, String, String)> {
        vs.iter()
            .map(|v| (v.code.to_string(), v.item.clone(), v.code_name.clone()))
            .collect()
    }

    /// §7.7.1 rule 1 + §7.7.2's preamble: the code objects come from **CPython's own** compiler, and
    /// the unit is a function so the declared bindings are its calling convention.
    #[test]
    fn the_probe_returns_cpythons_own_code_object_for_the_unit() {
        let d = probe("return x + 1", &["x"]);
        assert_eq!(d.unit, "__infy_unit__");
        assert_eq!(d.version_info.0, 3, "§7.7.3 只摄取 CPython 3.x");
        let co = d.unit_code();
        assert_eq!(co.name, "__infy_unit__");
        assert_eq!(co.argcount, 1, "绑定是单元的调用约定，不是注入的全局名（§7.7.1）");
        assert_eq!(co.varnames, vec!["x".to_string()]);
        assert!(co.consts.contains(&PyConst::Int(1)), "常量由源语言编译器定型：{:?}", co.consts);
        let last = co.instrs.last().expect("指令流非空");
        assert!(last.opname.starts_with("RETURN"), "单元以 return 收口，实得 {}", last.opname);
    }

    /// §7.7.1: `unit_code()` must hand back the **nested wrapper**, not the `<module>` body — the
    /// latter would carry no parameters and the whole calling convention would silently vanish.
    #[test]
    fn the_unit_is_the_nested_wrapper_not_the_module_body() {
        let d = probe("return x + 1", &["x"]);
        assert_eq!(d.module.name, "<module>");
        assert_eq!(d.module.nested().len(), 1);
        assert_eq!(d.unit_code().name, "__infy_unit__");
        assert_eq!(d.unit_code().argcount, 1, "取错对象的话这里是 0");
    }

    /// §7.4: every instruction the scan can point at must resolve back to an original-language line,
    /// otherwise `W-0705` would fire on every unit and the lowered image could not carry a line map.
    #[test]
    fn instructions_other_than_the_interpreters_own_padding_carry_a_line() {
        let d = probe("total = 0\nfor k in range(4):\n    total = total + k\nreturn total", &[]);
        let co = d.unit_code();
        let missing: Vec<String> = co
            .instrs
            .iter()
            .filter(|i| co.line_at(i.offset).is_none())
            .map(|i| i.opname.clone())
            .collect();
        for name in &missing {
            assert!(
                matches!(name.as_str(), "RESUME" | "CACHE" | "END_FOR" | "PUSH_NULL" | "RETURN_GENERATOR"),
                "非填充指令 {name} 没有行映射（§7.4）：{missing:?}"
            );
        }
        assert!(
            co.instrs.iter().filter(|i| co.line_at(i.offset).is_some()).count() >= co.instrs.len() / 2,
            "行映射几乎不存在：{missing:?}"
        );
    }

    /// §7.7.1 rule 1: the block is re-indented under the wrapper (relative indent kept) and the
    /// declared bindings become the parameter list.
    #[test]
    fn wrap_embed_unit_dedents_the_block_under_the_wrapper() {
        let body = "        if ok:\n            return 1\n        return 0\n";
        let got = wrap_embed_unit(body, &["x", "env"], "unit7");
        assert_eq!(got, "def unit7(x, env):\n    if ok:\n        return 1\n    return 0\n");
        assert_eq!(wrap_embed_unit("    return 1", &[], "u"), "def u():\n    return 1\n");
    }

    /// §7.7.3, the acceptance case "declare `py = \"3.13\"` while the host runs 3.14": the minor that
    /// actually compiled the unit must equal the declaration.
    #[test]
    fn a_declared_minor_that_is_not_the_interpreter_is_e0713() {
        let d = probe("return 1", &[]);
        let observed = d.py_minor();
        let other = if observed == "3.13" { "3.12" } else { "3.13" };
        let e = check_probe_version(&manifest_py(Some(other)), &d)
            .expect_err(&format!("声明 {other} 而解释器是 {observed}，必须 E-0713（§7.7.3）"));
        assert_eq!(e.code(), Some("E-0713"));
        match &e {
            IngestErr::VersionMismatch { declared, observed: o, .. } => {
                assert_eq!(declared.as_str(), other);
                assert_eq!(o, &observed);
            }
            bad => panic!("应为 VersionMismatch，实得 {bad:?}"),
        }
        assert!(
            check_probe_version(&manifest_py(Some(&observed)), &d).is_ok(),
            "声明与实际一致必须放行"
        );
    }

    /// §7.7.3: the `.pyc` magic is compared even when the minor agrees, a patch-level declaration is
    /// normalised, and an undeclared `@py` is `E-0715` rather than a guess.
    #[test]
    fn version_lock_covers_the_pyc_magic_and_an_undeclared_language() {
        let m313 = manifest_py(Some("3.13"));
        let magic313 = CodeObjDump::known_magics()["3.13"].0;
        assert!(check_declared_version(&m313, "3.13", magic313).is_ok());
        assert!(check_declared_version(&m313, "3.13.4", magic313).is_ok(), "补丁级声明应归一到 minor");
        let bad = check_declared_version(&m313, "3.13", magic313 + 1).expect_err("magic 不符必须拦");
        assert_eq!(bad.code(), Some("E-0713"));
        let none = check_declared_version(&manifest_py(None), "3.13", magic313).expect_err("§7.7.3 无声明即拒绝");
        assert_eq!(none.code(), Some("E-0715"));
    }

    /// §7.7.2 + [`DYNAMIC_CODE_NAMES`]: `eval` is refused as dynamic code generation (`E-0711`), and
    /// the table's own `eval = "-"` row must not downgrade it into a capability gap.
    #[test]
    fn eval_is_e0711_and_not_a_capability_gap() {
        let d = probe("return eval(\"1 + 1\")", &[]);
        let v = capability_scan(&d, &cap(&d.py_minor()));
        let hit = v
            .iter()
            .find(|x| x.item == "builtin(eval)")
            .unwrap_or_else(|| panic!("实得 {:?}", items(&v)));
        assert_eq!(hit.code, "E-0711");
        assert_eq!(hit.scan_item, "builtin");
        assert_eq!(hit.diagnostic(0, Position { line: 1, column: 1 }).code, "E-0711");
    }

    /// §7.7.2 item 1, the acceptance case "an unmapped opcode triggers `E-0712` and names the exact
    /// item": a **sub-operation** of a dispatched family row is refused by name, with the line it
    /// came from.
    #[test]
    fn a_denied_sub_operation_is_e0712_naming_it() {
        let d = probe("return x ** 2", &["x"]);
        let v = capability_scan(&d, &cap(&d.py_minor()));
        let hit = v
            .iter()
            .find(|x| x.item == "BINARY_OP(**)")
            .unwrap_or_else(|| panic!("应点名 BINARY_OP(**)，实得 {:?}", items(&v)));
        assert_eq!(hit.code, "E-0712");
        assert_eq!(hit.scan_item, "opcode");
        assert!(hit.py_line.is_some(), "诊断必须能指回原始语言行（§7.4）");
        assert!(hit.detail.contains("未映射"), "拒绝的理由要写进诊断：{}", hit.detail);
        assert!(hit.render().contains("BINARY_OP(**)"), "诊断必须点名具体子操作：{}", hit.render());
    }

    /// §7.7.2 item 1: an opcode with **no row at all** (rather than a `-` row) is also `E-0712`, and
    /// names both the OPNAME and the number E.3 keys it by.
    #[test]
    fn an_opcode_with_no_row_at_all_is_e0712_naming_number_and_opname() {
        let d = probe("return x + 1", &["x"]);
        let mut c = cap(&d.py_minor());
        let bin = d
            .unit_code()
            .instrs
            .iter()
            .find(|i| i.opname == "BINARY_OP")
            .expect("x + 1 必须编出 BINARY_OP");
        c.opcode_map.remove(&format!("0x{:x}", bin.opcode));
        c.opcode_map.remove("BINARY_OP");
        let v = capability_scan(&d, &c);
        let want = format!("BINARY_OP (0x{:x})", bin.opcode);
        let hit = v.iter().find(|x| x.item == want).unwrap_or_else(|| panic!("实得 {:?}", items(&v)));
        assert_eq!(hit.code, "E-0712");
        assert_eq!(hit.py_offset, bin.offset);
    }

    /// §7.7.2 item 2: a builtin the table explicitly refuses stops the build **by name**.
    #[test]
    fn a_builtin_the_table_refuses_is_e0712_naming_it() {
        let d = probe("tmp = input()\nreturn 1", &[]);
        let v = capability_scan(&d, &cap(&d.py_minor()));
        assert!(
            v.iter().any(|x| x.code == "E-0712" && x.item == "builtin(input)"),
            "实得 {:?}",
            items(&v)
        );
    }

    /// §7.7.2 scan item 4 demands the recursion be **exhaustive**: a gap inside a nested code object
    /// must still be reported, and the diagnosis must say which code object it came from.
    #[test]
    fn gaps_inside_nested_code_objects_are_reported_with_their_own_name() {
        let d = probe("f = lambda: eval(\"1\")\nreturn f()", &[]);
        assert_eq!(d.unit_code().nested().len(), 1, "lambda 是嵌套 code object");
        let v = capability_scan(&d, &cap(&d.py_minor()));
        let hit = v
            .iter()
            .find(|x| x.item == "builtin(eval)")
            .unwrap_or_else(|| panic!("实得 {:?}", items(&v)));
        assert_eq!(hit.code, "E-0711");
        assert_eq!(hit.code_name, "<lambda>", "诊断必须指明所在的 code object");
    }

    /// §7.7.2 is a *report*, not a first-failure abort: several gaps in one unit are all listed, in a
    /// deterministic order (§7.3 reproducibility).
    #[test]
    fn the_scan_lists_every_gap_in_deterministic_order() {
        let d = probe("tmp = input()\nreturn eval(tmp)", &[]);
        let v = capability_scan(&d, &cap(&d.py_minor()));
        assert!(v.len() >= 2, "两个缺口都要列出，实得 {:?}", items(&v));
        let keys: Vec<(String, u32, String)> =
            v.iter().map(|x| (x.code_name.clone(), x.py_offset, x.item.clone())).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "扫描结果必须有序");
    }

    /// §7.7.2 item 3 / §7.7.4: allow / deny / undeclared, and a deny entry covers its submodules
    /// without catching a sibling name by the nose.
    #[test]
    fn import_verdicts_are_allow_deny_or_undeclared() {
        let c = cap("3.14");
        assert!(matches!(c.import_verdict("math"), ImportVerdict::Allowed(_)));
        assert!(matches!(c.import_verdict("math.random"), ImportVerdict::Allowed(_)));
        assert!(matches!(c.import_verdict("numpy"), ImportVerdict::Denied(p) if p == "numpy"));
        assert!(matches!(c.import_verdict("numpy.linalg"), ImportVerdict::Denied(_)));
        assert!(matches!(c.import_verdict("numpy2"), ImportVerdict::Undeclared));
        assert!(matches!(c.import_verdict("requests"), ImportVerdict::Undeclared));
    }

    /// §7.7.2 scan item 3 through a real dump: `IMPORT_NAME`'s argument indexes **`co_names`**, so a
    /// denied import has to be found there — reading it as a constant index never fires.
    #[test]
    fn a_denied_import_is_e0712_naming_the_module() {
        let d = probe("import numpy\nreturn 1", &[]);
        assert!(
            d.unit_code()
                .instrs
                .iter()
                .any(|i| i.opname == "IMPORT_NAME"),
            "import 语句应编出 IMPORT_NAME"
        );
        let v = capability_scan(&d, &cap(&d.py_minor()));
        assert!(
            v.iter().any(|x| x.code == "E-0712" && x.item == "import(numpy)"),
            "实得 {:?}",
            items(&v)
        );
    }

    /// §7.7.2 scan item 5: the `co_exceptiontable` entry classes are a pure function of CPython's own
    /// decode — `push` / `setup` / `with` / `bare`, and the shipped tables cover the first three.
    #[test]
    fn exception_entry_shapes_are_classified_push_setup_with_bare() {
        let mut co = code_obj("__infy_unit__");
        co.code = vec![0u8; 12];
        co.instrs = vec![
            instr("PUSH_EXC_INFO", 0x20, None, "", 0),
            instr("CHECK_EXC_MATCH", 0x06, Some(0), "ValueError", 2),
            instr("POP_EXCEPT", 0x07, None, "", 4),
            instr("RERAISE", 0x69, Some(0), "4", 6),
        ];
        let e = |target: u32, lasti: bool| PyExcEntry { start: 0, end: 8, target, depth: 1, lasti };
        assert_eq!(co.exc_entry_kind(&e(0, true)), ExcKind::Handler, "lasti 的条目收到异常对象");
        assert_eq!(co.exc_entry_kind(&e(6, false)), ExcKind::Cleanup, "只有 RERAISE 的是清理块");
        co.instrs.push(instr("WITH_EXCEPT_START", 0x0b, None, "", 8));
        assert_eq!(co.exc_entry_kind(&e(8, false)), ExcKind::With, "with 协议另有一类");
        let mut bare = code_obj("__infy_unit__");
        bare.instrs = vec![instr("RETURN_CONST", 0x36, Some(0), "None", 0)];
        assert_eq!(bare.exc_entry_kind(&PyExcEntry { start: 0, end: 2, target: 0, depth: 0, lasti: false }), ExcKind::Unsupported);
        let c = cap("3.14");
        for k in [ExcKind::Cleanup, ExcKind::Handler, ExcKind::With] {
            assert!(c.exception_allowed(k), "{} 应在 entry 段", k.word());
        }
        assert!(!c.exception_allowed(ExcKind::Unsupported), "bare 不在 entry 段：必须由扫描拒绝");
    }

    /// E.3's value grammar, using the forms the shipped tables actually write.
    #[test]
    fn the_mapping_grammar_covers_every_form_the_tables_use() {
        assert_eq!(parse_mapping("seq.iter", "X").unwrap(), Mapping::Direct(Mapped::Mnem("seq.iter".to_string())));
        assert_eq!(parse_mapping("-", "X").unwrap(), Mapping::Direct(Mapped::Reject));
        assert_eq!(parse_mapping("ignore", "X").unwrap(), Mapping::Direct(Mapped::Ignore));
        assert_eq!(
            parse_mapping("host:std.io.log", "X").unwrap(),
            Mapping::Direct(Mapped::Host("std.io.log".to_string()))
        );
        let disp = parse_mapping("arg:+->add._|host:std.str.concat,-->sub._,else->-", "X").unwrap();
        assert_eq!(disp.resolve("+").mnemonic(), Some("add._"), "候选表按声明序取第一个可用的");
        assert_eq!(disp.resolve("+").alternatives().len(), 2);
        assert_eq!(disp.resolve("-").mnemonic(), Some("sub._"), "键 `-` 自身不是分隔符");
        assert_eq!(disp.resolve("%"), Mapped::Reject, "else->- 即能力缺口");
        let no_else = parse_mapping("arg:0->add._", "X").unwrap();
        assert_eq!(no_else.resolve("9"), Mapped::Reject, "没有 else 的派发表，未知键必须拒绝");
        assert!(parse_mapping("", "X").is_err(), "空值不是合法映射");
        assert!(parse_mapping("not_an_ifbc_mnemonic", "X").is_err(), "附录 D 之外的助记符不得混进构建（§7.8.2）");
    }

    /// §7.7.2's dispatch vocabulary: which sub-operation a family row splits on.
    #[test]
    fn dispatch_key_names_the_sub_operation_a_family_row_splits_on() {
        assert_eq!(dispatch_key(&instr("BINARY_OP", 0x2c, Some(0), "+", 0)), "+");
        assert_eq!(dispatch_key(&instr("COMPARE_OP", 0x38, Some(0), "<=", 0)), "<=");
        assert_eq!(dispatch_key(&instr("LOAD_ATTR", 0x24, Some(0), "x", 0)), "attr");
        assert_eq!(dispatch_key(&instr("LOAD_ATTR", 0x24, Some(1), "x", 0)), "method", "3.12 起 bit0 = 绑定方法");
        assert_eq!(dispatch_key(&instr("MAKE_FUNCTION", 0x73, Some(0), "", 0)), "plain");
        assert_eq!(dispatch_key(&instr("MAKE_FUNCTION", 0x73, Some(3), "", 0)), "flags");
        assert_eq!(dispatch_key(&instr("RETURN_VALUE", 0x37, None, "", 0)), "");
    }

    /// appendix E.3 keys `[opcode]` rows by **number** and carries the `OPNAME` alias beside it; the
    /// two must agree, or the table cannot be audited against the interpreter's own `dis.opmap`
    /// (§7.7.2). Only `IFBC-CAP-py-3.14.toml` satisfies the numeric half today — it was generated from
    /// the interpreter installed on this host. The 3.13 and lua tables are name-keyed only, recorded
    /// as BACKLOG **F28**: those numbers cannot be filled in here without inventing them (§12.2), and
    /// while they are missing `check_alias_agreement` can never fire for those two tables.
    #[test]
    fn the_number_keyed_table_agrees_between_its_number_and_name_rows() {
        let (lang, minor) = ("py", "3.14");
        let (name, text) = CapTable::shipped(lang, minor).expect("shipped table");
        let c = CapTable::load(lang, minor).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut numeric = 0usize;
        for line in text.lines() {
            let t = line.trim();
            let Some(rest) = t.strip_prefix("0x") else { continue };
            let (key, val) = rest.split_once(" = ").unwrap_or_else(|| panic!("{name}: 行格式 {t}"));
            let (value, alias) = match val.split_once("   # ") {
                Some((v, cm)) => (v.trim().trim_matches('"').to_string(), cm.trim().to_string()),
                None => panic!("{name}: {key} 缺 OPNAME 别名注释"),
            };
            let key = format!("0x{key}");
            numeric += 1;
            assert_eq!(
                c.opcode_map.get(&alias),
                Some(&value),
                "{name}: {key} = {value} 但 {alias} = {:?}",
                c.opcode_map.get(&alias)
            );
            let n = u32::from_str_radix(&key[2..], 16).unwrap();
            assert!(
                c.check_alias_agreement(n, &alias).is_ok(),
                "{name}: {key}/{alias} 自相矛盾"
            );
        }
        assert!(numeric > 100, "{name}: [opcode] 段只有 {numeric} 行");
    }

    /// The two name-keyed tables must still resolve **every** row: `check_alias_agreement` cannot
    /// help there (F28), so the only thing standing between a typo and a silently unmapped opcode is
    /// that each row parses (§7.7.2).
    #[test]
    fn the_name_keyed_tables_still_map_every_row() {
        for (lang, minor) in [("py", "3.13"), ("lua", "5.4")] {
            let (name, text) = CapTable::shipped(lang, minor).expect("shipped table");
            assert!(
                !text.lines().any(|l| l.trim().starts_with("0x")),
                "{name}: 已补上 E.3 的数值键就把它并进 the_number_keyed_table_…，并消掉 BACKLOG F28"
            );
            let c = CapTable::load(lang, minor).unwrap();
            let mut mapped = 0usize;
            let mut rejected = 0usize;
            for (k, v) in &c.opcode_map {
                let m = parse_mapping(v, k).unwrap_or_else(|e| panic!("{name}: {k} => {}", e.title()));
                match m.resolve("") {
                    Mapped::Reject => rejected += 1,
                    _ => mapped += 1,
                }
            }
            assert!(
                mapped > 20 && rejected > 20,
                "{name}: {mapped} 行覆盖 / {rejected} 行拒绝，共 {} 行",
                c.opcode_map.len()
            );
        }
    }

    /// §12.2: each table carries a SHA-256 over the lines that are not the hash row. The canonical
    /// form is built inside `CapTable::load`, so this is the only thing that catches a drift there
    /// (line endings included — this machine checks out with `core.autocrlf=true`).
    #[test]
    fn each_table_hash_matches_the_body_it_claims_to_cover() {
        for (lang, minor) in [("py", "3.13"), ("py", "3.14"), ("lua", "5.4")] {
            let (name, text) = CapTable::shipped(lang, minor).expect("shipped table");
            let declared = text
                .lines()
                .find(|l| l.trim_start().starts_with("table_hash"))
                .and_then(|l| l.split('"').nth(1))
                .unwrap_or_else(|| panic!("{name}: 没有 table_hash 行"))
                .to_string();
            let loaded = CapTable::load(lang, minor).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(loaded.table_hash, declared, "{name}: 声明的 hash 与按 E.3 规则重算的不一致");
        }
    }

    /// §7.7.2: `infy ingest probe` compares these counts against a real dump, so a row that does not
    /// parse must never degrade into "not mapped" silently.
    #[test]
    fn every_row_of_a_shipped_table_parses_under_the_grammar() {
        for (lang, minor) in [("py", "3.13"), ("py", "3.14"), ("lua", "5.4")] {
            let c = CapTable::load(lang, minor).expect("shipped table");
            let cov = c.coverage();
            assert!(cov.unparseable.is_empty(), "{lang}-{minor}: 这些行无法解析：{:?}", cov.unparseable);
            assert!(cov.builtins > 0, "{lang}-{minor}: [builtin] 段一行可用都没有：{cov:?}");
            if minor == "3.14" {
                // `coverage()` counts E.3's numeric rows, so a name-keyed table reports 0 by design.
                assert!(cov.mapped > 0 && cov.rejected > 0, "{lang}-{minor}: 覆盖统计 {cov:?}");
            }
        }
    }

    /// §7.7.1 rule 1: the units parameter table comes from the INFY-side declaration when the host
    /// table carries it; without that it is `argcount` x `dyn`, never an empty signature.
    #[test]
    fn unit_signature_prefers_the_declared_host_entry() {
        let mut co = code_obj("__infy_unit__");
        co.argcount = 2;
        let mut hosts = HostSigTable::new();
        assert_eq!(unit_signature(&co, &hosts).params, vec![TyKind::Dyn, TyKind::Dyn]);
        hosts.register(HostSig::new("__infy_unit__", &[TyKind::U32, TyKind::Str], TyKind::Bool));
        let s = unit_signature(&co, &hosts);
        assert_eq!(s.params, vec![TyKind::U32, TyKind::Str]);
        assert_eq!(s.ret, TyKind::Bool);
        co.name = "unit9".to_string();
        hosts.register(HostSig::new("unit9", &[TyKind::Int], TyKind::Unit));
        assert_eq!(unit_signature(&co, &hosts).params, vec![TyKind::Int], "按单元名索引（§7.7.1）");
    }

    /// §7.7.1/§7.8.2: a unit whose every opcode is covered lowers to an image that passes §7.8.3's
    /// checks and survives the encoder/decoder round-trip.
    #[test]
    fn a_covered_unit_lowers_to_an_image_that_passes_the_load_checks() {
        let d = probe("return x + 1", &["x"]);
        let c = cap(&d.py_minor());
        let hosts = HostSigTable::std_default();
        let m = lower_named(&d, &c, &hosts, "b_track/__infy_unit__.py")
            .unwrap_or_else(|v| panic!("lowering 失败：{:?}", items(&v)));
        let text = ifbc::dump_canonical(&m.encode()).expect("双侧共用的规范化文本必须能渲染这张镜像");
        assert!(text.lines().any(|l| l.starts_with('F')), "至少一个函数记录：{text}");
        assert!(text.lines().any(|l| l.starts_with('I')), "至少一条指令：{text}");
    }

    /// §7.7.1 rule 1: the parameter table is read off the **INFY-side** types. The `None` half of
    /// each pair is §2 of `runtime/IFBC.md` being a closed 18-row set (no 128-bit width) or a type
    /// that carries no information (`auto`, unresolved) — refusing is the honest answer, boxing a
    /// guess is not.
    #[test]
    fn embed_ty_kind_reads_the_declared_type_or_refuses() {
        let slice = Ty::Slice { inner: Box::new(Ty::Prim(Prim::F32)) };
        let cases: Vec<(Ty, Option<TyKind>)> = vec![
            (Ty::Prim(Prim::U32), Some(TyKind::U32)),
            (Ty::Prim(Prim::I64), Some(TyKind::Int)),
            (Ty::Prim(Prim::F64), Some(TyKind::Float)),
            (Ty::Prim(Prim::F32), Some(TyKind::F32)),
            (Ty::Prim(Prim::Char), Some(TyKind::Char)),
            (Ty::Str, Some(TyKind::Str)),
            (Ty::String, Some(TyKind::Str), ),
            (Ty::Bool, Some(TyKind::Bool)),
            (Ty::Unit, Some(TyKind::Unit)),
            // `&[f32]`（§7.7.1 样例的 `batch`）：共享引用透传到被指向的容器类
            (Ty::Ref { inner: Box::new(slice.clone()), mutable: false, lifetime: None }, Some(TyKind::List)),
            (Ty::Vec { inner: Box::new(Ty::Prim(Prim::F32)) }, Some(TyKind::List)),
            (Ty::Tuple(vec![Ty::Prim(Prim::U8)]), Some(TyKind::Tuple)),
            (Ty::Adt("Fail".into()), Some(TyKind::Instance)),
            (Ty::Prim(Prim::U128), None),
            (Ty::Prim(Prim::I128), None),
            (Ty::Auto, None),
            (Ty::IntLit, None),
            (Ty::Invalid, None),
        ];
        for (t, want) in cases {
            assert_eq!(embed_ty_kind(&t), want, "{}", t.text());
        }
        // 容器类在 IFABI 线上就是 `dyn`（§2 的标量码域），签名文本与摘要按同一规则算。
        assert_eq!(TyKind::List.sig_code(), TyKind::Dyn as u8);
        assert_eq!(embed_ty_kind(&Ty::String).map(TyKind::sig_code), Some(TyKind::Str as u8));
    }

    /// The §7.7.1 sample's calling convention, one unit, through the front end's entry point: the
    /// declared `f64`/`u32` table and the declared return must land in the image's own function
    /// record (`nargs`, `ret_sig`, and the §7.8.3 item-5 digest), not be re-derived from the dump.
    #[test]
    fn the_declared_table_reaches_the_function_record() {
        let mut h = Handler::default();
        let img = ingest_embed_site(
            &unit_site("normalize", "return scale * 2.0", &[("scale", TyKind::Float), ("n", TyKind::U32)], TyKind::Float),
            &manifest_py(Some("3.14")),
            &python(),
            &mut h,
        )
        .unwrap_or_else(|e| panic!("阶段 [6a] 摄取失败：{e}"));
        assert_eq!(h.exit_code(), 0, "摄取不应再发诊断：{:?}", h.summary());
        assert_eq!(img.unit, "normalize");
        let m = ifbc::decode(&img.image).expect("阶段 [6a] 的镜像必须能被本侧解码器读回");
        assert_eq!(m.unit, "normalize", "探针应按写入的单元名取函数体，而不是模块包装器");
        assert_eq!(m.funcs.len(), 1, "只有单元自己那一条函数记录：{:?}", m.funcs);
        let f = &m.funcs[0];
        assert_eq!(f.nargs, 2, "§7.7.1 规则 1：形参个数来自 INFY 侧声明");
        assert_eq!(f.ret_sig, u32::from(TyKind::Float.sig_code()), "返回类型同样来自声明（规则 2）");
        assert!(img.source_toolchain.starts_with("CPython 3.14"), "§7.7.3：进 build-id 的源工具链串：{}", img.source_toolchain);
    }

    /// D1's acceptance, this side: the image stage [6a] produced passes §7.8.3's six checks here.
    /// `runtime/test_ifbc.c::test_py_unit_fixture` runs the same checks with the **production**
    /// loader over the committed bytes.
    #[test]
    fn a_b_track_image_passes_the_load_checks_on_this_side() {
        let m = ifbc::decode(&py_unit_fixture_bytes()).expect("夹具应能被本侧解码");
        let bad: Vec<String> = ifbc::verify(&m, &HostSigTable::std_default(), true)
            .into_iter()
            .filter(|v| v.code.starts_with('R'))
            .map(|v| v.render())
            .collect();
        assert!(bad.is_empty(), "装载期校验不该报 R-码：{bad:?}");
        // §7.4 的行映射：镜像里的行号是**原始语言**的行号（这里单元体只有两行有效代码）。
        assert!(!m.lines.is_empty(), "B 轨镜像必须带原语言行映射");
    }

    /// §7.7.3 through the CLI entry: a declared minor that is not the interpreter's is `E-0713`,
    /// and the ingest produces no image.
    #[test]
    fn a_version_disagreement_stops_the_ingest_and_reports_e0713() {
        let observed = probe_version(&python()).expect("探针版本应可读");
        let declared = if observed.starts_with("3.14") { "3.13" } else { "3.14" };
        let mut h = Handler::default();
        let r = ingest_embed_site(
            &unit_site("normalize", "return scale * 2.0", &[("scale", TyKind::Float)], TyKind::Float),
            &manifest_py(Some(declared)),
            &python(),
            &mut h,
        );
        let e = r.expect_err(&format!("声明 {declared} 而解释器是 {observed}，必须拒绝"));
        assert_eq!(e.code(), Some("E-0713"), "{e}");
        assert_eq!(h.exit_code(), 2, "诊断必须让构建变红：{:?}", h.summary());
    }

    /// §7.7.1 的样例形状在**本构建**里过不了扫描（`RERAISE` 在能力表里是 `-`），这是显式失败而不是
    /// 静默跳过；诊断必须点名 opcode 并带原始语言行（登记为 BACKLOG F29）。
    #[test]
    fn the_list_comprehension_sample_is_refused_by_name_not_silently() {
        let mut h = Handler::default();
        let e = ingest_embed_site(
            &unit_site(
                "normalize",
                "out = [ (x / scale) for x in batch ]\nreturn out",
                &[("batch", TyKind::List), ("scale", TyKind::Float)],
                TyKind::List,
            ),
            &manifest_py(Some("3.14")),
            &python(),
            &mut h,
        )
        .expect_err("§7.7.1 的推导式样例含未映射 opcode，应显式拒绝");
        assert_eq!(e.code(), Some("E-0712"), "{e}");
        match &e {
            IngestErr::Scan(v) => {
                let named: Vec<String> = v.iter().map(|x| x.item.clone()).collect();
                assert!(
                    named.iter().any(|x| x.contains("RERAISE")),
                    "诊断必须点名具体 opcode（§7.7.2）：{named:?}"
                );
                assert!(
                    v.iter().any(|x| x.py_line.is_some()),
                    "§7.4：拒绝必须带原始语言行，否则改写时无从定位：{named:?}"
                );
            }
            other => panic!("应是能力扫描失败，实得 {other:?}"),
        }
    }

    /// The committed `runtime/fixtures/ifbc_py_unit.bin`: a **real stage-[6a] image** (not a
    /// hand-built one), which `runtime/test_ifbc.c` loads with the production VM. Only
    /// `INFY_REGENERATE_FIXTURE=1` rewrites it; the paired `.can.txt` is rendered by the **C** side
    /// (`INFY_REGENERATE_CANONICAL=1`), so neither side defines the shared text alone — the same
    /// division of direction D4(3) set up. Regenerating requires the CPython this image was
    /// ingested from (§7.7.3 makes stage [6a] a build-time dependency of B-track, of `gcc` for C).
    #[test]
    fn py_unit_fixture_is_exactly_what_the_ingest_produces() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/fixtures/ifbc_py_unit.bin");
        let bytes = py_unit_fixture_bytes();
        if std::env::var("INFY_REGENERATE_FIXTURE").is_ok() {
            std::fs::write(path, &bytes).expect("explicit fixture regeneration");
        }
        let on_disk = std::fs::read(path).unwrap_or_else(|e| {
            panic!("{path} 不在仓库里（{e}）—— 用 INFY_REGENERATE_FIXTURE=1 生成一次")
        });
        assert_eq!(on_disk, bytes, "阶段 [6a] 的产物与已提交夹具不再逐字节相同");
        // 双侧共用的文本由 C 侧渲染；这一侧读出的镜像必须与夹具是同一份内容。
        let m = ifbc::decode(&bytes).expect("夹具可解码");
        assert_eq!(m.unit, "normalize");
        assert_eq!(m.funcs.len(), 1);
    }

    /// The B-track unit behind `runtime/fixtures/ifbc_py_unit.bin`. §7.7.1 rule 1's declared table,
    /// one body that the capability table covers end to end (no attribute access, no iteration —
    /// those are F27's load-but-cannot-execute half).
    fn py_unit_fixture_bytes() -> Vec<u8> {
        let mut h = Handler::default();
        ingest_embed_site(
            &unit_site("normalize", "return scale * 2.0", &[("scale", TyKind::Float), ("n", TyKind::U32)], TyKind::Float),
            &manifest_py(Some("3.14")),
            &python(),
            &mut h,
        )
        .unwrap_or_else(|e| panic!("夹具单元的摄取失败：{e}"))
        .image
    }

    /// §7.7.3's "patch 不敏感" has to reach the capability **table** too: declaring the full
    /// interpreter version (`py = "3.14.6"`) must resolve `IFBC-CAP-py-3.14.toml`, not ask for a
    /// nonexistent `3.14.6` table and die with `E-0712 能力表无法读取` after the version check
    /// already passed. The minor the probe reports is what compiled the unit, so that is the key.
    #[test]
    fn a_patch_level_declaration_still_finds_the_minor_capability_table() {
        let observed = probe_version(&python()).expect("探针应报出解释器版本");
        let mut h = Handler::default();
        let img = ingest_embed_site(
            &unit_site("normalize", "return scale * 2.0", &[("scale", TyKind::Float)], TyKind::Float),
            &manifest_py(Some(&observed)),
            &python(),
            &mut h,
        )
        .unwrap_or_else(|e| panic!("声明补丁级版本 {observed} 也应摄取成功：{e}"));
        assert!(!img.image.is_empty(), "补丁级声明不该产出空镜像");
        assert_eq!(h.exit_code(), 0, "{observed} 不该引出诊断：{:?}", h.summary());
    }

    /// A site as stage [6a] receives it: `@py(scale=scale, n=n)` over a one-line body, §7.7.1's form.
    fn unit_site(        unit: &str,
        body: &str,
        params: &[(&str, TyKind)],
        ret: TyKind,
    ) -> EmbedSite {
        EmbedSite {
            lang: "py".to_string(),
            unit_name: unit.to_string(),
            body: body.to_string(),
            params: params.iter().map(|(n, k)| (n.to_string(), *k)).collect(),
            ret,
            file: 0,
            pos: Position { line: 1, column: 1 },
        }
    }
}
