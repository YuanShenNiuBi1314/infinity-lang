//! IFBC — INFY ByteCode container, assembler, decoder, disassembler, loader verifier.
//!
//! §7.8 + appendix D + `docs/internals/runtime-abi.md` §9. This module is the only place on the
//! Rust side that knows the IFBC byte format; it is pure data (no I/O, no diagnostics, no front-end
//! dependency) so that both the B-track ingest (`crate::btrack`) and `infy ifbc dump|verify`
//! (§7.8.4) sit on one implementation.
//!
//! Container, as fixed by `docs/internals/runtime-abi.md` §9:
//!
//! ```text
//! header | type_table | import_table | func_table | code | line_map | exception_table | string_pool
//! header: magic "IFB1" | ifbc_version u16 | ifabi u16 | lang_tag u16 | module_hash [u8;32]
//!         | func_count u32 | frame_kind u8                        (= 49 bytes)
//! ```
//!
//! Every region after the header is wrapped as `byte_len:u32 || byte_len bytes`. That wrapper is a
//! decision of this implementation (spec hole, recorded in `docs/internals/ifbc-encoding-rust.md`):
//! the normative header carries no region offset table, so without a length prefix `code` — whose
//! size is only implied by `func_table` plus padding to 4 bytes — could not be told apart from
//! `line_map`. All integers are big-endian (D.1), every instruction is `opcode:u8` + 0..3 `u32`
//! operands (D.1), and function entry offsets and the end of the code region are 4-byte aligned.
//!
//! Determinism (§7.3): the string pool interns by first appearance, every map the encoder walks is a
//! `BTreeMap`, and `module_hash` is SHA-256 over the normalised region body, so two builds of one
//! input produce byte-identical images. Opcode bytes, every appendix-D grouping decision and the
//! deviations taken here are listed in `docs/internals/ifbc-encoding-rust.md`.

use std::collections::BTreeMap;
use std::fmt;

use crate::sha256;

/// D.1 / §7.8.1 container magic; identical to `INFY_IFBC_MAGIC` in `runtime/infy_rt.h`.
pub const MAGIC: &[u8; 4] = b"IFB1";
/// v2.0 IFBC version (`crate::VERSION.ifbc`).
pub const IFBC_VERSION_V1: u16 = 1;
/// IFABI-2 (`crate::VERSION.ifabi`, `INFY_IFABI` in `runtime/infy_rt.h`).
pub const IFABI_V2: u16 = 2;
/// `magic | ifbc_version | ifabi | lang_tag | module_hash | func_count | frame_kind`.
pub const HEADER_LEN: usize = 4 + 2 + 2 + 2 + 32 + 4 + 1;
/// §7.8.1 `frame_kind:u8`, 0 = single-address stack machine + local window.
pub const FRAME_KIND_STACK: u8 = 0;
/// Documented addition: 1 = coroutine frame (D.8).
pub const FRAME_KIND_COROUTINE: u8 = 1;
/// Sentinel for "no type / no name" in a u32 field.
pub const NONE32: u32 = u32::MAX;
/// Maximum operands per instruction (D.1: 0..3).
pub const MAX_OPS: usize = 3;

/// Region names in the normative container order (§7.8.1).
pub const REGIONS: [&str; 8] = [
    "header",
    "type_table",
    "import_table",
    "func_table",
    "code",
    "line_map",
    "exception_table",
    "string_pool",
];

/// §7.8.1: `lang_tag` records the source language and its bytecode format version (`CPY-313`); the
/// capability-table version travels beside it. The u16 is `(lang_id << 8) | format_id`, both halves
/// from these two closed tables, so a module whose origin cannot be named never loads (§7.8.3).
pub const LANGS: [(u8, &str); 6] = [
    (0, "infy"),
    (1, "cpython"),
    (2, "lua"),
    (3, "jvm"),
    (4, "deno"),
    (5, "node"),
];

/// Format ids: `CPY-313` is CPython 3.13 (format id = the minor version), `LUA-54` is Lua 5.4.
pub const FORMATS: [((u8, u8), &str); 7] = [
    ((0, 0), "IFBY-1"),
    ((1, 12), "CPY-312"),
    ((1, 13), "CPY-313"),
    ((1, 14), "CPY-314"),
    ((2, 54), "LUA-54"),
    ((4, 1), "DENO-1"),
    ((5, 1), "NODE-1"),
];

pub fn lang_tag_of(name: &str) -> Option<u16> {
    FORMATS
        .iter()
        .find(|(_, n)| *n == name)
        .map(|((l, f), _)| ((l as u16) << 8) | *f as u16)
}

pub fn lang_tag_name(tag: u16) -> Option<&'static str> {
    let (l, f) = ((tag >> 8) as u8, (tag & 0xff) as u8);
    FORMATS
        .iter()
        .find(|((al, af), _)| *al == l && *af == f)
        .map(|(_, n)| *n)
}

/// `lang_tag` plus the capability-table version, for diagnostics and for `ifbc_caps` rows (E.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LangTag {
    pub tag: u16,
    pub cap_version: u32,
}

impl LangTag {
    pub fn name(&self) -> Option<&'static str> {
        lang_tag_name(self.tag)
    }
    /// `CPY-313:1` — the appendix E.2 `ifbc_caps` row form.
    pub fn cap_string(&self) -> String {
        format!("{}:{}", self.name().unwrap_or("UNKNOWN"), self.cap_version)
    }
}

/// Type-class kinds. 0..=11 are exactly `InfyDynTag` in `runtime/infy_rt.h` (`DY_NONE, DY_INT, …,
/// DY_INSTANCE`), so `is.dyn T`/`cast.dyn T` need no translation at run time. That alignment is a
/// deliberate reconciliation with the one part of the C side that already exists (§7.7.7). Kinds
/// 12.. are static-only INFY types with no `dyn` tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TyKind {
    Unit = 0,
    Int = 1,
    Float = 2,
    Str = 3,
    Bytes = 4,
    List = 5,
    Tuple = 6,
    Dict = 7,
    Bool = 8,
    Func = 9,
    Coro = 10,
    Instance = 11,
    I8 = 12,
    I16 = 13,
    I32 = 14,
    U8 = 15,
    U16 = 16,
    U32 = 17,
    U64 = 18,
    F32 = 19,
    Char = 20,
    Usize = 21,
    Isize = 22,
    Ref = 23,
    Box = 24,
    Rc = 25,
    Arc = 26,
    Option = 27,
    Result = 28,
    Never = 29,
}

impl TyKind {
    pub fn as_u8(self) -> u8 {
        self as u8
    }
    pub fn from_u8(b: u8) -> Option<TyKind> {
        const TABLE: [TyKind; 30] = [
            TyKind::Unit,
            TyKind::Int,
            TyKind::Float,
            TyKind::Str,
            TyKind::Bytes,
            TyKind::List,
            TyKind::Tuple,
            TyKind::Dict,
            TyKind::Bool,
            TyKind::Func,
            TyKind::Coro,
            TyKind::Instance,
            TyKind::I8,
            TyKind::I16,
            TyKind::I32,
            TyKind::U8,
            TyKind::U16,
            TyKind::U32,
            TyKind::U64,
            TyKind::F32,
            TyKind::Char,
            TyKind::Usize,
            TyKind::Isize,
            TyKind::Ref,
            TyKind::Box,
            TyKind::Rc,
            TyKind::Arc,
            TyKind::Option,
            TyKind::Result,
            TyKind::Never,
        ];
        if (b as usize) >= TABLE.len() {
            return None;
        }
        Some(TABLE[b as usize])
    }
    /// The `InfyDynTag` value for dyn-visible kinds (§7.7.7).
    pub fn dyn_tag(self) -> Option<u8> {
        let v = self as u8;
        if v <= TyKind::Instance as u8 {
            Some(v)
        } else {
            None
        }
    }
    /// Kinds that may carry an arithmetic/comparison type suffix (D.3).
    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            TyKind::Int
                | TyKind::Float
                | TyKind::I8
                | TyKind::I16
                | TyKind::I32
                | TyKind::U8
                | TyKind::U16
                | TyKind::U32
                | TyKind::U64
                | TyKind::F32
                | TyKind::Usize
                | TyKind::Isize
        )
    }
    /// `i64`, `f32`, … — the suffix text of a mnemonic such as `add.i64`.
    pub fn suffix(self) -> &'static str {
        match self {
            TyKind::Unit => "unit",
            TyKind::Int => "i64",
            TyKind::Float => "f64",
            TyKind::Str => "str",
            TyKind::Bytes => "bytes",
            TyKind::List => "list",
            TyKind::Tuple => "tuple",
            TyKind::Dict => "dict",
            TyKind::Bool => "bool",
            TyKind::Func => "func",
            TyKind::Coro => "coro",
            TyKind::Instance => "object",
            TyKind::I8 => "i8",
            TyKind::I16 => "i16",
            TyKind::I32 => "i32",
            TyKind::U8 => "u8",
            TyKind::U16 => "u16",
            TyKind::U32 => "u32",
            TyKind::U64 => "u64",
            TyKind::F32 => "f32",
            TyKind::Char => "char",
            TyKind::Usize => "usize",
            TyKind::Isize => "isize",
            TyKind::Ref => "ref",
            TyKind::Box => "box",
            TyKind::Rc => "rc",
            TyKind::Arc => "arc",
            TyKind::Option => "option",
            TyKind::Result => "result",
            TyKind::Never => "never",
        }
    }
}

/// One `type_table` row. `a`/`b` are payloads: for `list`/`tuple` `a` is the element type index, for
/// `dict` `a`/`b` are key/value; otherwise `NONE32`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeEntry {
    pub kind: u8,
    pub name: u32,
    pub a: u32,
    pub b: u32,
}

/// What an `import_table` row refers to. Validation item 5 (§7.8.3) only accepts `Host` rows for
/// `call.host` and requires them to match `IFABI` element by element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ImportKind {
    Host = 0,
    PyModule = 1,
    Infib = 2,
    Entry = 3,
}

impl ImportKind {
    pub fn as_u8(self) -> u8 {
        self as u8
    }
    pub fn from_u8(b: u8) -> Option<ImportKind> {
        Some(match b {
            0 => ImportKind::Host,
            1 => ImportKind::PyModule,
            2 => ImportKind::Infib,
            3 => ImportKind::Entry,
            _ => return None,
        })
    }
    pub fn word(self) -> &'static str {
        match self {
            ImportKind::Host => "host",
            ImportKind::PyModule => "pymod",
            ImportKind::Infib => "infib",
            ImportKind::Entry => "entry",
        }
    }
}

/// One `import_table` row: an external name plus the signature it is checked against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportEntry {
    pub name: u32,
    pub kind: ImportKind,
    pub ifabi: u16,
    pub ret_ty: u32,
    pub params: Vec<u32>,
    pub sig_hash: u32,
}

impl ImportEntry {
    pub fn sig_text(&self, pool: &StringPool, types: &[TypeEntry]) -> String {
        let ps: Vec<String> = self.params.iter().map(|t| ty_text(types, pool, *t)).collect();
        format!("({}) -> {}", ps.join(", "), ty_text(types, pool, self.ret_ty))
    }
}

/// Rendering of a type index for `infy ifbc dump` / `disassemble`.
pub fn ty_text(types: &[TypeEntry], pool: &StringPool, idx: u32) -> String {
    if idx == NONE32 {
        return "()".to_string();
    }
    match types.get(idx as usize) {
        Some(t) => {
            let base = pool.get(t.name).cloned().unwrap_or_else(|| "?".to_string());
            match TyKind::from_u8(t.kind) {
                Some(k) if matches!(k, TyKind::List | TyKind::Dict | TyKind::Tuple) => {
                    if t.a == NONE32 {
                        base
                    } else {
                        format!("{}<{}>", base, ty_text(types, pool, t.a))
                    }
                }
                _ => base,
            }
        }
        None => format!("ty#{idx}"),
    }
}

/// Function flags (bits of `FuncEntry::flags`).
pub const F_ENTRY: u8 = 1 << 0;
pub const F_COROUTINE: u8 = 1 << 1;
pub const F_BOUNDARY: u8 = 1 << 2;

/// One `func_table` row (36 bytes on the wire).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuncEntry {
    pub name: u32,
    pub code_off: u32,
    pub code_len: u32,
    /// Local-window slot count: parameters + locals + compiler scratch (§7.7.1 rule 1).
    pub locals: u32,
    pub arity: u32,
    pub ret_ty: u32,
    /// String-pool index of the canonical signature text, for `infy doc` and diagnostics.
    pub sig: u32,
    /// `HostSigTable::sig_hash` of that signature: validation item 5 compares bytes, not names.
    pub sig_hash: u32,
    /// 0 = stack + local window (§7.8.1), 1 = coroutine frame (D.8).
    pub frame: u8,
    pub flags: u8,
}

/// `line_map`: `count u32` then `count × { code_off u32, line u32 }` sorted by `code_off`
/// (`runtime-abi.md` §9). §7.4 requires the mapping to point at the **original** language line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineEntry {
    pub code_off: u32,
    pub line: u32,
}

/// `exception_table`: `count u32` then
/// `count × { start u32, end u32, handler u32, depth u16, kind u16 }` (`runtime-abi.md` §9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExcEntry {
    pub start: u32,
    pub end: u32,
    pub handler: u32,
    pub depth: u16,
    pub kind: ExcKind,
}

/// The `kind` discriminant. Appendix D leaves this open, so `runtime-abi.md` §9 fixes it; the words
/// are the `co_exceptiontable` entry classes of §7.7.2 scan item 5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum ExcKind {
    /// Unwind cleanup block only (CPython `push` entry, `lasti == false`).
    Cleanup = 0,
    /// Handler that receives the exception object (CPython `setup`).
    Handler = 1,
    /// `with`-protocol entry (handler region contains `WITH_EXCEPT_START`).
    With = 2,
    /// Anything else; must be rejected by the capability table.
    Unsupported = 3,
}

impl ExcKind {
    pub fn as_u16(self) -> u16 {
        self as u16
    }
    pub fn from_u16(v: u16) -> Option<ExcKind> {
        Some(match v {
            0 => ExcKind::Cleanup,
            1 => ExcKind::Handler,
            2 => ExcKind::With,
            3 => ExcKind::Unsupported,
            _ => return None,
        })
    }
    pub fn word(self) -> &'static str {
        match self {
            ExcKind::Cleanup => "push",
            ExcKind::Handler => "setup",
            ExcKind::With => "with",
            ExcKind::Unsupported => "bare",
        }
    }
}

/// The instruction set of appendix D: one variant per **byte**, numbered exactly as D numbers them.
/// Where D groups several mnemonics into one row the split is documented on the variant, in
/// `docs/internals/ifbc-encoding-rust.md` and in `GROUPING` below.
///
/// D.3 states the type suffixes are independent opcodes, but the `0x20..0x3F` block cannot hold 15
/// families x 22 kinds of bytes. The resolution taken here (deviation `DEV-IFBC-1`) is a *static*
/// two-part encoding: the byte selects the family and **operand 0 is the type index**, a
/// compile-time constant. Nothing inspects a run-time type to choose semantics — which is what
/// D.3's "无后缀版本非法" forbids — and validation item 3 still checks the suffix statically, so a
/// family instruction with a non-numeric type operand is `R-0706`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Op {
    // ---- D.2 constants and locals, 0x00..0x1F
    Nop = 0x00,
    Ldl = 0x01,
    Stl = 0x02,
    Ldw = 0x03,
    Push = 0x04,
    PushB = 0x05,
    /// D.2 groups `dup / dup2 / swap` in one row at 0x06; split as 0x06, 0x0A, 0x0B (both free).
    Dup = 0x06,
    Pop = 0x07,
    /// D.2: **forbidden** — INFY has no null, only `Option`. Emitting it is `R-0706`.
    LdNull = 0x08,
    /// D.2: debug builds only.
    LdFrame = 0x09,
    Dup2 = 0x0A,
    Swap = 0x0B,
    /// Module-global window access, placed in D.2's unassigned 0x0C..0x1F gap (`DEV-IFBC-12`).
    Ldg = 0x0C,
    Stg = 0x0D,

    // ---- D.3 arithmetic and comparison, 0x20..0x3F (operand 0 = type index)
    Add = 0x20,
    Sub = 0x21,
    Mul = 0x22,
    Div = 0x23,
    Rem = 0x24,
    Neg = 0x25,
    Abs = 0x26,
    /// D.3 mandates `guard.nonzero` immediately before `div`/`rem` but assigns it no byte; 0x27 is
    /// D.3's own gap (`DEV-IFBC-4`).
    GuardNonzero = 0x27,
    And = 0x28,
    Or = 0x29,
    Xor = 0x2A,
    Not = 0x2B,
    Shl = 0x2C,
    Shr = 0x2D,
    ShrA = 0x2E,
    /// D.3's `guard.shift`, same reasoning as `GuardNonzero`; 0x2F is the gap before `cmplt`.
    GuardShift = 0x2F,
    CmpLt = 0x30,
    CmpLe = 0x31,
    CmpGt = 0x32,
    CmpGe = 0x33,
    CmpEq = 0x34,
    CmpNe = 0x35,
    /// Three-way compare writing an `i32` discriminator (D.3).
    Cmp = 0x36,

    // ---- D.4 control flow, 0x40..0x5F (jump operands are absolute code-region offsets)
    Jmp = 0x40,
    Br = 0x41,
    Brf = 0x42,
    /// D.4's `switch n, [targets]` is variable-length, contradicting D.1's fixed width. Reserved:
    /// `CodeBuilder::switch` refuses to emit it and the verifier rejects the byte (`DEV-IFBC-5`).
    Switch = 0x43,
    TickLoop = 0x44,
    Leave = 0x45,
    ThrowInf = 0x46,
    EnterTry = 0x47,

    // ---- D.5 calls, 0x60..0x7F
    CallStat = 0x60,
    CallVirt = 0x61,
    CallHost = 0x62,
    CallEmbed = 0x63,
    Ret = 0x64,
    RetMulti = 0x65,
    ClosureNew = 0x66,
    TailCallStat = 0x67,
    /// §7.7.2 scan item 3 needs imports to be instructions and D has none, so they sit in D.5's
    /// 0x68..0x7F gap (`DEV-IFBC-6`).
    Import = 0x68,
    ImportAttr = 0x69,

    // ---- D.6 objects and memory, 0x80..0x9F
    NewStr = 0x80,
    NewList = 0x81,
    NewDict = 0x82,
    NewFrame = 0x83,
    /// Sequence protocol in D.6's 0x84..0x87 gap (§7.7.6: the object model is where B-track's cost
    /// actually is) (`DEV-IFBC-7`).
    SeqPush = 0x84,
    SeqLen = 0x85,
    SeqNext = 0x86,
    SeqIter = 0x87,
    IdxGet = 0x88,
    IdxSet = 0x89,
    Slice = 0x8A,
    /// D.6 gap 0x8B..0x8F: attribute and mapping writes, string/sequence concatenation.
    InstGet = 0x8B,
    InstSet = 0x8C,
    MapInsert = 0x8D,
    StrConcat = 0x8E,
    SeqConcat = 0x8F,
    RefInc = 0x90,
    RefDec = 0x91,
    MoveOut = 0x94,

    // ---- D.7 dynamic values, 0xA0..0xBF (B-track only)
    TagGet = 0xA0,
    CastDyn = 0xA1,
    IsDyn = 0xA2,
    BoxDyn = 0xA3,
    UnboxDyn = 0xA4,
    /// D.7 gap 0xA5..0xA7: truthiness and identity, needed by every Python `if` and `is`.
    DynTruthy = 0xA5,
    DynIdentity = 0xA6,
    InvokeDynamic = 0xA8,

    // ---- D.8 coroutines and iteration, 0xC0..0xDF
    FrameEnter = 0xC0,
    Yield = 0xC1,
    YieldFrom = 0xC2,
    FrameExit = 0xC3,
    CoroResume = 0xC4,
    CoroThrow = 0xC5,

    // ---- D.9 type mirror inline, 0xE0..0xEF
    TypeOfIdx = 0xE0,
    SizeOf = 0xE1,
    AlignOf = 0xE2,
    FieldOff = 0xE3,
}

/// Every appendix-D row that groups more than one mnemonic, with the byte assignment this
/// implementation makes. Printed by `disassemble`'s legend and mirrored in
/// `docs/internals/ifbc-encoding-rust.md` so the C side can cross-check one table.
pub const GROUPING: [(&str, &str); 9] = [
    ("D.2 0x06", "dup / dup2 / swap -> 0x06 dup, 0x0A dup2, 0x0B swap"),
    ("D.2 0x0A..0x1F", "unassigned gap -> 0x0C ldg, 0x0D stg (global window)"),
    ("D.3 all rows", "type suffix = operand 0 (a type_table index), not a separate byte: DEV-IFBC-1"),
    ("D.3 0x27, 0x2F", "guard.nonzero, guard.shift (mandated by D.3, never numbered by D)"),
    ("D.4 0x43", "switch: reserved, not emitted; verifier rejects it: DEV-IFBC-5"),
    ("D.5 0x68, 0x69", "import, import.attr: DEV-IFBC-6"),
    ("D.6 0x84..0x87", "seq.push, seq.len, seq.next, seq.iter: DEV-IFBC-7"),
    ("D.6 0x8B..0x8F", "inst.get, inst.set, map.insert, str.concat, seq.concat: DEV-IFBC-7"),
    ("D.7 0xA5, 0xA6", "dyn.truthy, dyn.identity: DEV-IFBC-7"),
];

/// Operand forms, so the verifier and the disassembler label the same u32 correctly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperandKind {
    None,
    /// Local-window slot.
    Slot,
    /// String-pool index.
    Pool,
    /// `type_table` index (the type suffix of D.3 families, or the `T` of `is.dyn T`).
    Type,
    /// Absolute offset into the module `code` region.
    CodeOff,
    /// `func_table` index.
    Func,
    /// `import_table` index.
    Import,
    /// Immediate integer.
    Imm,
    /// `InfyDynTag`-compatible immediate (bool).
    Flag,
}

impl Op {
    pub fn byte(self) -> u8 {
        self as u8
    }

    pub fn from_byte(b: u8) -> Option<Op> {
        Some(match b {
            0x00 => Op::Nop,
            0x01 => Op::Ldl,
            0x02 => Op::Stl,
            0x03 => Op::Ldw,
            0x04 => Op::Push,
            0x05 => Op::PushB,
            0x06 => Op::Dup,
            0x07 => Op::Pop,
            0x08 => Op::LdNull,
            0x09 => Op::LdFrame,
            0x0A => Op::Dup2,
            0x0B => Op::Swap,
            0x0C => Op::Ldg,
            0x0D => Op::Stg,
            0x20 => Op::Add,
            0x21 => Op::Sub,
            0x22 => Op::Mul,
            0x23 => Op::Div,
            0x24 => Op::Rem,
            0x25 => Op::Neg,
            0x26 => Op::Abs,
            0x27 => Op::GuardNonzero,
            0x28 => Op::And,
            0x29 => Op::Or,
            0x2A => Op::Xor,
            0x2B => Op::Not,
            0x2C => Op::Shl,
            0x2D => Op::Shr,
            0x2E => Op::ShrA,
            0x2F => Op::GuardShift,
            0x30 => Op::CmpLt,
            0x31 => Op::CmpLe,
            0x32 => Op::CmpGt,
            0x33 => Op::CmpGe,
            0x34 => Op::CmpEq,
            0x35 => Op::CmpNe,
            0x36 => Op::Cmp,
            0x40 => Op::Jmp,
            0x41 => Op::Br,
            0x42 => Op::Brf,
            0x43 => Op::Switch,
            0x44 => Op::TickLoop,
            0x45 => Op::Leave,
            0x46 => Op::ThrowInf,
            0x47 => Op::EnterTry,
            0x60 => Op::CallStat,
            0x61 => Op::CallVirt,
            0x62 => Op::CallHost,
            0x63 => Op::CallEmbed,
            0x64 => Op::Ret,
            0x65 => Op::RetMulti,
            0x66 => Op::ClosureNew,
            0x67 => Op::TailCallStat,
            0x68 => Op::Import,
            0x69 => Op::ImportAttr,
            0x80 => Op::NewStr,
            0x81 => Op::NewList,
            0x82 => Op::NewDict,
            0x83 => Op::NewFrame,
            0x84 => Op::SeqPush,
            0x85 => Op::SeqLen,
            0x86 => Op::SeqNext,
            0x87 => Op::SeqIter,
            0x88 => Op::IdxGet,
            0x89 => Op::IdxSet,
            0x8A => Op::Slice,
            0x8B => Op::InstGet,
            0x8C => Op::InstSet,
            0x8D => Op::MapInsert,
            0x8E => Op::StrConcat,
            0x8F => Op::SeqConcat,
            0x90 => Op::RefInc,
            0x91 => Op::RefDec,
            0x94 => Op::MoveOut,
            0xA0 => Op::TagGet,
            0xA1 => Op::CastDyn,
            0xA2 => Op::IsDyn,
            0xA3 => Op::BoxDyn,
            0xA4 => Op::UnboxDyn,
            0xA5 => Op::DynTruthy,
            0xA6 => Op::DynIdentity,
            0xA8 => Op::InvokeDynamic,
            0xC0 => Op::FrameEnter,
            0xC1 => Op::Yield,
            0xC2 => Op::YieldFrom,
            0xC3 => Op::FrameExit,
            0xC4 => Op::CoroResume,
            0xC5 => Op::CoroThrow,
            0xE0 => Op::TypeOfIdx,
            0xE1 => Op::SizeOf,
            0xE2 => Op::AlignOf,
            0xE3 => Op::FieldOff,
            _ => return None,
        })
    }

    /// Base mnemonic without any type suffix, exactly as appendix D prints it.
    pub fn base(self) -> &'static str {
        match self {
            Op::Nop => "nop",
            Op::Ldl => "ldl",
            Op::Stl => "stl",
            Op::Ldw => "ldw",
            Op::Push => "push",
            Op::PushB => "push.b",
            Op::Dup => "dup",
            Op::Pop => "pop",
            Op::LdNull => "ldnull",
            Op::LdFrame => "ld.frame",
            Op::Dup2 => "dup2",
            Op::Swap => "swap",
            Op::Ldg => "ldg",
            Op::Stg => "stg",
            Op::Add => "add",
            Op::Sub => "sub",
            Op::Mul => "mul",
            Op::Div => "div",
            Op::Rem => "rem",
            Op::Neg => "neg",
            Op::Abs => "abs",
            Op::GuardNonzero => "guard.nonzero",
            Op::And => "and",
            Op::Or => "or",
            Op::Xor => "xor",
            Op::Not => "not",
            Op::Shl => "shl",
            Op::Shr => "shr",
            Op::ShrA => "shr.a",
            Op::GuardShift => "guard.shift",
            Op::CmpLt => "cmplt",
            Op::CmpLe => "cmple",
            Op::CmpGt => "cmpgt",
            Op::CmpGe => "cmpge",
            Op::CmpEq => "cmpeq",
            Op::CmpNe => "cmpne",
            Op::Cmp => "cmp",
            Op::Jmp => "jmp",
            Op::Br => "br",
            Op::Brf => "brf",
            Op::Switch => "switch",
            Op::TickLoop => "tickloop",
            Op::Leave => "leave",
            Op::ThrowInf => "throw.inf",
            Op::EnterTry => "enter.try",
            Op::CallStat => "call.stat",
            Op::CallVirt => "call.virt",
            Op::CallHost => "call.host",
            Op::CallEmbed => "call.embed",
            Op::Ret => "ret",
            Op::RetMulti => "ret.multi",
            Op::ClosureNew => "closure.new",
            Op::TailCallStat => "tailcall.stat",
            Op::Import => "import",
            Op::ImportAttr => "import.attr",
            Op::NewStr => "new.str",
            Op::NewList => "new.list",
            Op::NewDict => "new.dict",
            Op::NewFrame => "new.frame",
            Op::SeqPush => "seq.push",
            Op::SeqLen => "seq.len",
            Op::SeqNext => "seq.next",
            Op::SeqIter => "seq.iter",
            Op::IdxGet => "idx.get",
            Op::IdxSet => "idx.set",
            Op::Slice => "slice",
            Op::InstGet => "inst.get",
            Op::InstSet => "inst.set",
            Op::MapInsert => "map.insert",
            Op::StrConcat => "str.concat",
            Op::SeqConcat => "seq.concat",
            Op::RefInc => "ref.inc",
            Op::RefDec => "ref.dec",
            Op::MoveOut => "move.out",
            Op::TagGet => "tag.get",
            Op::CastDyn => "cast.dyn",
            Op::IsDyn => "is.dyn",
            Op::BoxDyn => "box.dyn",
            Op::UnboxDyn => "unbox.dyn",
            Op::DynTruthy => "dyn.truthy",
            Op::DynIdentity => "dyn.identity",
            Op::InvokeDynamic => "invoke.dynamic",
            Op::FrameEnter => "frame.enter",
            Op::Yield => "yield",
            Op::YieldFrom => "yield.from",
            Op::FrameExit => "frame.exit",
            Op::CoroResume => "coro.resume",
            Op::CoroThrow => "coro.throw",
            Op::TypeOfIdx => "typeof.idx",
            Op::SizeOf => "size_of",
            Op::AlignOf => "align_of",
            Op::FieldOff => "field_off",
        }
    }

    /// Number of `u32` operands implied by the opcode (D.1: the count is implied, never encoded).
    pub fn arity(self) -> usize {
        match self {
            // zero operands
            Op::Nop | Op::Dup | Op::Dup2 | Op::Swap | Op::Pop | Op::LdNull | Op::LdFrame
            | Op::Ldw | Op::Ret | Op::Leave | Op::FrameEnter | Op::FrameExit | Op::TagGet
            | Op::UnboxDyn | Op::DynTruthy | Op::DynIdentity | Op::Yield | Op::YieldFrom
            | Op::RefInc | Op::RefDec | Op::TypeOfIdx => 0,
            // one operand
            Op::Ldl | Op::Stl | Op::Ldg | Op::Stg | Op::Push | Op::PushB | Op::Jmp | Op::Br
            | Op::Brf | Op::TickLoop | Op::ThrowInf | Op::CallStat | Op::CallVirt | Op::CallHost
            | Op::CallEmbed | Op::TailCallStat | Op::Import | Op::ImportAttr | Op::NewStr
            | Op::IdxGet | Op::IdxSet | Op::Slice | Op::SeqLen | Op::SeqIter | Op::SeqNext
            | Op::SeqPush | Op::InstGet | Op::InstSet | Op::StrConcat | Op::SeqConcat
            | Op::MoveOut | Op::CastDyn | Op::IsDyn | Op::BoxDyn | Op::InvokeDynamic
            | Op::CoroResume | Op::CoroThrow | Op::SizeOf | Op::AlignOf | Op::FieldOff
            | Op::RetMulti | Op::NewFrame
            // D.3 families: operand 0 is the type index (DEV-IFBC-1)
            | Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Rem | Op::Neg | Op::Abs | Op::And
            | Op::Or | Op::Xor | Op::Not | Op::Shl | Op::Shr | Op::ShrA | Op::CmpLt | Op::CmpLe
            | Op::CmpGt | Op::CmpGe | Op::CmpEq | Op::CmpNe | Op::Cmp
            | Op::GuardNonzero | Op::GuardShift => 1,
            // two operands
            Op::NewList | Op::NewDict | Op::ClosureNew | Op::EnterTry | Op::MapInsert
            | Op::Switch => 2,
        }
    }

    /// Operand forms, positionally. A D.3 family's operand 0 is always `Type`.
    pub fn operand_kinds(self) -> Vec<OperandKind> {
        use OperandKind::*;
        match self {
            Op::Ldl | Op::Stl | Op::Ldg | Op::Stg | Op::MoveOut => vec![Slot],
            Op::Push | Op::ThrowInf | Op::NewStr | Op::ImportAttr => vec![Pool],
            Op::PushB | Op::TickLoop | Op::SizeOf | Op::AlignOf | Op::FieldOff | Op::RetMulti
            | Op::CoroResume | Op::CoroThrow => vec![Imm],
            Op::Jmp | Op::Br | Op::Brf => vec![CodeOff],
            Op::CallStat | Op::TailCallStat => vec![Func],
            Op::CallHost | Op::Import => vec![Import],
            Op::CallVirt | Op::CallEmbed | Op::InvokeDynamic | Op::ClosureNew => vec![Imm, Imm],
            Op::NewList | Op::SeqLen | Op::SeqIter | Op::SeqNext | Op::SeqPush | Op::IdxGet
            | Op::IdxSet | Op::Slice | Op::InstGet | Op::InstSet | Op::StrConcat | Op::SeqConcat
            | Op::CastDyn | Op::IsDyn | Op::BoxDyn | Op::NewFrame => vec![Type],
            Op::NewDict => vec![Type, Type],
            Op::MapInsert => vec![Type, Imm],
            Op::EnterTry => vec![CodeOff, CodeOff],
            Op::Switch => vec![Imm, CodeOff],
            _ => Vec::new(),
        }
    }

    /// D.3 families and the `dyn` instructions whose operand 0 must be a `type_table` index.
    pub fn is_typed_family(self) -> bool {
        matches!(
            self,
            Op::Add
                | Op::Sub
                | Op::Mul
                | Op::Div
                | Op::Rem
                | Op::Neg
                | Op::Abs
                | Op::And
                | Op::Or
                | Op::Xor
                | Op::Not
                | Op::Shl
                | Op::Shr
                | Op::ShrA
                | Op::CmpLt
                | Op::CmpLe
                | Op::CmpGt
                | Op::CmpGe
                | Op::CmpEq
                | Op::CmpNe
                | Op::Cmp
        )
    }

    /// Binary arithmetic/comparison families: two operands on the stack.
    pub fn is_family(self) -> bool {
        self.is_typed_family() || matches!(self, Op::GuardNonzero | Op::GuardShift)
    }

    /// `(pops, pushes)` for the stack-height validation (§7.8.3 item 2), for the instructions whose
    /// effect is opcode-implied. Call instructions are resolved by the verifier from the tables.
    pub fn stack_effect(self) -> (usize, usize) {
        use Op::*;
        match self {
            Nop | LdFrame | GuardNonzero | GuardShift | TickLoop | EnterTry | Leave | FrameEnter
            | FrameExit | Jmp | Br | Brf | Switch | ThrowInf | Ret | RetMulti | Yield
            | YieldFrom | CoroResume | CoroThrow | RefDec | Import | RefInc => (0, 0),
            Ldl | Ldw | Push | PushB | LdNull | Ldg | SeqLen | NewStr | NewFrame | TypeOfIdx
            | SizeOf | AlignOf | FieldOff | DynTruthy => (0, 1),
            Stl | Stg | Pop | IdxSet | MoveOut => (1, 0),
            Dup => (1, 2),
            Dup2 => (2, 4),
            Swap => (2, 2),
            Neg | Abs | Not | InvokeDynamic | CallVirt | CallEmbed | ClosureNew => (1, 1),
            Add | Sub | Mul | Div | Rem | And | Or | Xor | Shl | Shr | ShrA | CmpLt | CmpLe
            | CmpGt | CmpGe | CmpEq | CmpNe | Cmp | DynIdentity | StrConcat | SeqConcat => (2, 1),
            IdxGet | Slice | InstGet | SeqNext | SeqIter | ImportAttr | CastDyn | UnboxDyn
            | TagGet | IsDyn | BoxDyn | MapInsert => (1, 1),
            NewList => (0, 1), // pops `count` operands, resolved per-operand by the verifier
            NewDict => (0, 1),
            SeqPush => (2, 1),
            CallStat | CallHost | TailCallStat => (0, 1), // resolved from the tables
        }
    }
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.base())
    }
}

/// Deterministic string pool: interned by **first appearance**, so the pool bytes are a pure
/// function of the instruction stream order (§7.3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StringPool {
    entries: Vec<String>,
    index: BTreeMap<String, u32>,
}

impl StringPool {
    pub fn new() -> StringPool {
        StringPool::default()
    }
    pub fn intern(&mut self, s: &str) -> u32 {
        if let Some(i) = self.index.get(s) {
            return *i;
        }
        let i = self.entries.len() as u32;
        self.entries.push(s.to_string());
        self.index.insert(s.to_string(), i);
        i
    }
    pub fn get(&self, idx: u32) -> Option<&String> {
        self.entries.get(idx as usize)
    }
    pub fn find(&self, s: &str) -> Option<u32> {
        self.index.get(s).copied()
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn entries(&self) -> &[String] {
        &self.entries
    }
    /// Rebuild a pool from decoded bytes; interning order is the pool order.
    pub fn from_entries(entries: Vec<String>) -> StringPool {
        let mut p = StringPool::new();
        for e in entries {
            p.intern(&e);
        }
        p
    }
}

/// Encode/assemble failures. Data coming from a `.pyc`, a wheel or a hand-written `.infib` is
/// never trusted, so nothing here panics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IfbcEncodeErr {
    WrongArity { op: &'static str, expected: usize, given: usize },
    UnknownMnemonic(String),
    SwitchNotSupported,
    SlotOutOfRange { slot: u32, locals: u32 },
    CodeRegionOverflow,
}

impl fmt::Display for IfbcEncodeErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IfbcEncodeErr::WrongArity { op, expected, given } => {
                write!(f, "{op} 需要 {expected} 个操作数，实际 {given} 个")
            }
            IfbcEncodeErr::UnknownMnemonic(m) => write!(f, "未知 IFBC 助记符 `{m}`"),
            IfbcEncodeErr::SwitchNotSupported => {
                write!(f, "switch 的变长目标表与 D.1 的定宽指令冲突，本实现不予发射")
            }
            IfbcEncodeErr::SlotOutOfRange { slot, locals } => {
                write!(f, "局部槽 {slot} 超出帧窗口 {locals}")
            }
            IfbcEncodeErr::CodeRegionOverflow => write!(f, "code 区超过 u32 长度"),
        }
    }
}

impl std::error::Error for IfbcEncodeErr {}

/// Decode failures; every one of them is the build-time/static counterpart of an `R-07xx`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IfbcDecodeErr {
    TooShort { need: usize, got: usize },
    BadMagic([u8; 4]),
    UnsupportedIfbc(u16),
    UnsupportedIfabi(u16),
    UnknownLangTag(u16),
    BadFrameKind(u8),
    FuncCountMismatch { header: u32, table: u32 },
    RegionTruncated { region: &'static str, need: usize, got: usize },
    RegionCount { region: &'static str, max: u32 },
    TrailingBytes(usize),
    PoolIndexOutOfRange { pool: u32, len: u32 },
    TypeIndexOutOfRange { ty: u32, len: u32 },
    ImportIndexOutOfRange { imp: u32, len: u32 },
    FuncIndexOutOfRange { func: u32, len: u32 },
    CodeRangeOutOfRange { func: u32, off: u32, len: u32, code_len: u32 },
    MisalignedFunc { func: u32, off: u32 },
    MisalignedRegion { region: &'static str, len: u32 },
    UnknownOpcode { byte: u8, off: u32 },
    InstructionRunsPastFunc { op: &'static str, off: u32 },
    BadUtf8InPool { pool: u32 },
    HashMismatch { stored: [u8; 32], computed: [u8; 32] },
    LineMapUnsorted { at: u32 },
    ExcRangeInvalid { at: usize },
}

impl fmt::Display for IfbcDecodeErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IfbcDecodeErr::TooShort { need, got } => {
                write!(f, "IFBC 镜像过短：至少需要 {need} 字节，实际 {got}")
            }
            IfbcDecodeErr::BadMagic(m) => write!(
                f,
                "IFBC magic 不符：期望 \"IFB1\"，实际 {:?}（不是 IFBC 镜像）",
                std::str::from_utf8(m).unwrap_or("?")
            ),
            IfbcDecodeErr::UnsupportedIfbc(v) => write!(f, "IFBC 版本 {v} 不受本工具链支持"),
            IfbcDecodeErr::UnsupportedIfabi(v) => write!(f, "IFABI-{v} 与 IFABI-2 不符"),
            IfbcDecodeErr::UnknownLangTag(t) => {
                write!(f, "未知 lang_tag 0x{t:04X}：源语言与字节码形态无法判定（§7.8.1）")
            }
            IfbcDecodeErr::BadFrameKind(k) => write!(f, "未知 frame_kind {k}"),
            IfbcDecodeErr::FuncCountMismatch { header, table } => write!(
                f,
                "header.func_count = {header} 与 func_table 行数 {table} 不一致"
            ),
            IfbcDecodeErr::RegionTruncated { region, need, got } => write!(
                f,
                "{region} 区被截断：声明 {need} 字节，只剩 {got}"
            ),
            IfbcDecodeErr::RegionCount { region, max } => {
                write!(f, "{region} 区行数 {max} 超过镜像可容纳上限")
            }
            IfbcDecodeErr::TrailingBytes(n) => write!(f, "IFBC 镜像尾部有 {n} 个多余字节"),
            IfbcDecodeErr::PoolIndexOutOfRange { pool, len } => {
                write!(f, "string_pool 索引 {pool} 越界（池大小 {len}）")
            }
            IfbcDecodeErr::TypeIndexOutOfRange { ty, len } => {
                write!(f, "type_table 索引 {ty} 越界（表长 {len}）")
            }
            IfbcDecodeErr::ImportIndexOutOfRange { imp, len } => {
                write!(f, "import_table 索引 {imp} 越界（表长 {len}）— 校验项 5")
            }
            IfbcDecodeErr::FuncIndexOutOfRange { func, len } => {
                write!(f, "func_table 索引 {func} 越界（表长 {len}）")
            }
            IfbcDecodeErr::CodeRangeOutOfRange { func, off, len, code_len } => write!(
                f,
                "函数 #{func} 的代码区间 [{off}, {off}+{len}) 超出 code 区 {code_len}"
            ),
            IfbcDecodeErr::MisalignedFunc { func, off } => {
                write!(f, "函数 #{func} 入口偏移 {off} 未按 4 字节对齐（D.1）")
            }
            IfbcDecodeErr::MisalignedRegion { region, len } => {
                write!(f, "{region} 区长度 {len} 不是 4 的倍数（D.1 对齐要求）")
            }
            IfbcDecodeErr::UnknownOpcode { byte, off } => {
                write!(f, "code 偏移 {off:#06x} 处是未定义 opcode 0x{byte:02X}（附录 D）")
            }
            IfbcDecodeErr::InstructionRunsPastFunc { op, off } => {
                write!(f, "{op}@{off:#06x} 的操作数越过函数代码区间")
            }
            IfbcDecodeErr::BadUtf8InPool { pool } => {
                write!(f, "string_pool[{pool}] 不是合法 UTF-8")
            }
            IfbcDecodeErr::HashMismatch { .. } => {
                write!(f, "module_hash 与规范化模块内容不符（§7.3 可复现性）")
            }
            IfbcDecodeErr::LineMapUnsorted { at } => {
                write!(f, "line_map 在 code_off {at:#06x} 处未按升序排列")
            }
            IfbcDecodeErr::ExcRangeInvalid { at } => {
                write!(f, "exception_table 第 {at} 项区间非法")
            }
        }
    }
}

impl std::error::Error for IfbcDecodeErr {}

// ------------------------------------------------------------------------ module

/// An IFBC module: the eight regions of §7.8.1 in Rust form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IfbcModule {
    pub ifbc_version: u16,
    pub ifabi: u16,
    /// `CPY-313` etc. (`lang_tag` on the wire is `tag`.)
    pub lang: LangTag,
    /// §7.8.1: 0 = single-address stack machine + local window.
    pub frame_kind: u8,
    /// Diagnostic label of the ingested unit (also the first `string_pool` entry).
    pub unit: String,
    /// Module-relative name of the source, e.g. `b_track/normalize.py` — appears in diagnostics so
    /// a runtime failure can quote the original-language file and line (§7.4).
    pub source_name: String,
    pub types: Vec<TypeEntry>,
    pub imports: Vec<ImportEntry>,
    pub funcs: Vec<FuncEntry>,
    /// Instruction stream, big-endian, 4-byte aligned (§7.8.1 / D.1).
    pub code: Vec<u8>,
    pub lines: Vec<LineEntry>,
    pub exceptions: Vec<ExcEntry>,
    pub strings: StringPool,
    /// SHA-256 over [`IfbcModule::normalised`].
    pub module_hash: [u8; 32],
}

impl Default for IfbcModule {
    fn default() -> Self {
        IfbcModule {
            ifbc_version: IFBC_VERSION_V1,
            ifabi: IFABI_V2,
            lang: LangTag { tag: lang_tag_of("IFBY-1").unwrap_or(0), cap_version: 0 },
            frame_kind: FRAME_KIND_STACK,
            unit: String::new(),
            source_name: String::new(),
            types: Vec::new(),
            imports: Vec::new(),
            funcs: Vec::new(),
            code: Vec::new(),
            lines: Vec::new(),
            exceptions: Vec::new(),
            strings: StringPool::new(),
            module_hash: [0u8; 32],
        }
    }
}

/// Big-endian writers — the only byte-order code in this module (D.1: 大端存储).
fn put_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_be_bytes());
}
fn put_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_be_bytes());
}
fn be_u16(b: &[u8]) -> Option<u16> {
    b.get(..2).map(|x| u16::from_be_bytes([x[0], x[1]]))
}
fn be_u32(b: &[u8]) -> Option<u32> {
    b.get(..4)
        .map(|x| u32::from_be_bytes([x[0], x[1], x[2], x[3]]))
}

impl IfbcModule {
    /// A module that can only come from the B-track ingest of `format` (`CPY-313`, `LUA-54`).
    pub fn new_unit(format: &str, cap_version: u32, unit: &str, source_name: &str) -> Option<IfbcModule> {
        let tag = lang_tag_of(format)?;
        Some(IfbcModule {
            lang: LangTag { tag, cap_version },
            unit: unit.to_string(),
            source_name: source_name.to_string(),
            ..IfbcModule::default()
        })
    }

    /// Intern a string into the pool (first-appearance order, therefore deterministic).
    pub fn intern(&mut self, s: &str) -> u32 {
        self.strings.intern(s)
    }

    /// Add (or reuse) a type-table row; identity is `(kind, name, a, b)` so interning is stable.
    pub fn intern_type(&mut self, kind: TyKind, name: &str, a: u32, b: u32) -> u32 {
        let name_idx = self.strings.intern(name);
        for (i, t) in self.types.iter().enumerate() {
            if t.kind == kind.as_u8() && t.name == name_idx && t.a == a && t.b == b {
                return i as u32;
            }
        }
        self.types.push(TypeEntry { kind: kind.as_u8(), name: name_idx, a, b });
        (self.types.len() - 1) as u32
    }

    pub fn ty_kind(&self, idx: u32) -> Option<TyKind> {
        self.types.get(idx as usize).and_then(|t| TyKind::from_u8(t.kind))
    }

    pub fn type_name(&self, idx: u32) -> Option<String> {
        let t = self.types.get(idx as usize)?;
        Some(self.strings.get(t.name)?.clone())
    }

    /// Register an `import_table` row for a host function, returning its index. The signature is
    /// taken from `hosts`, which is what makes validation item 5 checkable.
    pub fn intern_host(&mut self, hosts: &HostSigTable, name: &str) -> Option<u32> {
        let sig = hosts.get(name)?;
        for (i, e) in self.imports.iter().enumerate() {
            if e.kind == ImportKind::Host && self.strings.get(e.name).map(|s| s.as_str()) == Some(name)
            {
                return Some(i as u32);
            }
        }
        let name_idx = self.strings.intern(name);
        let params: Vec<u32> = sig
            .params
            .iter()
            .map(|tk| self.intern_tykind(*tk))
            .collect();
        let ret_ty = self.intern_tykind(sig.ret);
        self.imports.push(ImportEntry {
            name: name_idx,
            kind: ImportKind::Host,
            ifabi: self.ifabi,
            ret_ty,
            params,
            sig_hash: sig.sig_hash(),
        });
        Some((self.imports.len() - 1) as u32)
    }

    /// Register an `import_table` row for an ingested Python module (scan item 3).
    pub fn intern_module_import(&mut self, name: &str) -> u32 {
        if let Some(i) = self
            .imports
            .iter()
            .position(|e| e.kind == ImportKind::PyModule && self.strings.get(e.name).map(|s| s.as_str()) == Some(name))
        {
            return i as u32;
        }
        let name_idx = self.strings.intern(name);
        let ty = self.intern_type(TyKind::Instance, "dyn", NONE32, NONE32);
        self.imports.push(ImportEntry {
            name: name_idx,
            kind: ImportKind::PyModule,
            ifabi: self.ifabi,
            ret_ty: ty,
            params: Vec::new(),
            sig_hash: 0,
        });
        (self.imports.len() - 1) as u32
    }

    fn intern_tykind(&mut self, k: TyKind) -> u32 {
        self.intern_type(k, k.suffix(), NONE32, NONE32)
    }

    /// Append one function to `func_table`, returning its index. `code` must already have been
    /// appended (by [`CodeBuilder::finish_into`]) so `code_off`/`code_len` are known.
    pub fn push_func(
        &mut self,
        name: &str,
        code_off: u32,
        code_len: u32,
        locals: u32,
        params: &[TyKind],
        ret: TyKind,
        frame: u8,
        flags: u8,
    ) -> u32 {
        let name_idx = self.strings.intern(name);
        let param_tys: Vec<u32> = params.iter().map(|k| self.intern_tykind(*k)).collect();
        let ret_idx = self.intern_tykind(ret);
        let sig_text = HostSig::canonical_text(&param_tys, &self.types, ret_idx);
        let sig_hash = HostSig::hash_text(&sig_text);
        let sig_idx = self.strings.intern(&sig_text);
        self.funcs.push(FuncEntry {
            name: name_idx,
            code_off,
            code_len,
            locals,
            arity: params.len() as u32,
            ret_ty: ret_idx,
            sig: sig_idx,
            sig_hash,
            frame,
            flags,
        });
        (self.funcs.len() - 1) as u32
    }

    // ------------------------------------------------------------------ hash + encode

    /// The normalised module contents that `module_hash` is taken over: the seven regions with
    /// their length prefixes, in container order, **excluding** the header (and therefore excluding
    /// the hash field itself). `unit` and `source_name` are interned into the pool before encoding,
    /// so they are covered too.
    pub fn normalised(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write_body_into(&mut out);
        out
    }

    /// SHA-256 over [`IfbcModule::normalised`], the §7.3 reproducibility anchor for this module.
    pub fn compute_module_hash(&self) -> [u8; 32] {
        sha256::sha256(&self.normalised())
    }

    /// Stamp [`IfbcModule::compute_module_hash`] into `module_hash`; `encode` never re-derives it,
    /// so an unsealed module is detectable and `decode` refuses it.
    pub fn seal(&mut self) {
        self.module_hash = self.compute_module_hash();
    }

    /// Full container bytes: 49-byte header followed by the length-prefixed regions.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + 256);
        out.extend_from_slice(MAGIC);
        put_u16(&mut out, self.ifbc_version);
        put_u16(&mut out, self.ifabi);
        put_u16(&mut out, self.lang.tag);
        out.extend_from_slice(&self.module_hash);
        put_u32(&mut out, self.funcs.len() as u32);
        out.push(self.frame_kind);
        self.write_body_into(&mut out);
        out
    }

    fn write_body_into(&self, out: &mut Vec<u8>) {
        let start = out.len();
        write_region(out, REGIONS[1], &self.encode_types());
        write_region(out, REGIONS[2], &self.encode_imports());
        write_region(out, REGIONS[3], &self.encode_funcs());
        let mut code = self.code.clone();
        while code.len() % 4 != 0 {
            code.push(0);
        }
        write_region(out, REGIONS[4], &code);
        write_region(out, REGIONS[5], &self.encode_lines());
        write_region(out, REGIONS[6], &self.encode_exceptions());
        write_region(out, REGIONS[7], &self.encode_pool());
        let _ = start;
    }

    fn encode_types(&self) -> Vec<u8> {
        let mut b = Vec::new();
        put_u32(&mut b, self.types.len() as u32);
        for t in &self.types {
            b.push(t.kind);
            put_u32(&mut b, t.name);
            put_u32(&mut b, t.a);
            put_u32(&mut b, t.b);
        }
        b
    }

    fn encode_imports(&self) -> Vec<u8> {
        let mut b = Vec::new();
        put_u32(&mut b, self.imports.len() as u32);
        for e in &self.imports {
            put_u32(&mut b, e.name);
            put_u16(&mut b, e.ifabi);
            b.push(e.kind.as_u8());
            b.push(e.params.len() as u8);
            put_u32(&mut b, e.ret_ty);
            put_u32(&mut b, e.sig_hash);
            for p in &e.params {
                put_u32(&mut b, *p);
            }
        }
        b
    }

    fn encode_funcs(&self) -> Vec<u8> {
        let mut b = Vec::new();
        put_u32(&mut b, self.funcs.len() as u32);
        for f in &self.funcs {
            put_u32(&mut b, f.name);
            put_u32(&mut b, f.code_off);
            put_u32(&mut b, f.code_len);
            put_u32(&mut b, f.locals);
            put_u32(&mut b, f.arity);
            put_u32(&mut b, f.ret_ty);
            put_u32(&mut b, f.sig);
            put_u32(&mut b, f.sig_hash);
            b.push(f.frame);
            b.push(f.flags);
            put_u16(&mut b, 0);
        }
        b
    }

    fn encode_lines(&self) -> Vec<u8> {
        let mut b = Vec::new();
        put_u32(&mut b, self.lines.len() as u32);
        for l in &self.lines {
            put_u32(&mut b, l.code_off);
            put_u32(&mut b, l.line);
        }
        b
    }

    fn encode_exceptions(&self) -> Vec<u8> {
        let mut b = Vec<u8>::new();
        put_u32(&mut b, self.exceptions.len() as u32);
        for e in &self.exceptions {
            put_u32(&mut b, e.start);
            put_u32(&mut b, e.end);
            put_u32(&mut b, e.handler);
            put_u16(&mut b, e.depth);
            put_u16(&mut b, e.kind.as_u16());
        }
        b
    }

    fn encode_pool(&self) -> Vec<u8> {
        let mut b = Vec::new();
        put_u32(&mut b, self.strings.len() as u32);
        for s in self.strings.entries() {
            let bytes = s.as_bytes();
            put_u32(&mut b, bytes.len() as u32);
            b.extend_from_slice(bytes);
        }
        b
    }
}

/// `byte_len:u32 || bytes` — the region wrapper described at the top of this module.
fn write_region(out: &mut Vec<u8>, name: &'static str, body: &[u8]) {
    let _ = name;
    put_u32(out, body.len() as u32);
    out.extend_from_slice(body);
}

// ------------------------------------------------------------------------ host signatures

/// One host (INFY-native) entry point a B-track frame may reach with `call.host`, or the signature
/// of the embed unit itself (§7.7.1 rule 1: the *INFY-side* declared types fix the parameter table).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostSig {
    pub name: String,
    pub params: Vec<TyKind>,
    pub ret: TyKind,
    pub ifabi: u16,
}

impl HostSig {
    pub fn new(name: &str, params: &[TyKind], ret: TyKind) -> HostSig {
        HostSig { name: name.to_string(), params: params.to_vec(), ret, ifabi: IFABI_V2 }
    }

    /// Canonical signature text `(i64, f64) -> dyn` — the byte form whose digest is compared
    /// element-wise by validation item 5 (§7.8.3).
    pub fn canonical_text(params: &[u32], types: &[TypeEntry], ret: u32) -> String {
        let pool = StringPool::new();
        let ps: Vec<String> = params
            .iter()
            .map(|t| types.get(*t as usize).and_then(|e| TyKind::from_u8(e.kind)).map(|k| k.suffix()).unwrap_or("?").to_string())
            .collect();
        let _ = pool;
        let r = types
            .get(ret as usize)
            .and_then(|e| TyKind::from_u8(e.kind))
            .map(|k| k.suffix())
            .unwrap_or("?");
        format!("({}) -> {}", ps.join(", "), r)
    }

    /// `sha256(canonical text)` truncated to a big-endian `u32`, compared byte-wise at load.
    pub fn hash_text(text: &str) -> u32 {
        let d = sha256::sha256(text.as_bytes());
        u32::from_be_bytes([d[0], d[1], d[2], d[3]])
    }

    /// Signature digest computed from the declared kinds (no module tables needed).
    pub fn sig_hash(&self) -> u32 {
        let ps: Vec<String> = self.params.iter().map(|k| k.suffix().to_string()).collect();
        let text = format!("({}) -> {}", ps.join(", "), self.ret.suffix());
        HostSig::hash_text(&text)
    }

    pub fn text(&self) -> String {
        let ps: Vec<String> = self.params.iter().map(|k| k.suffix().to_string()).collect();
        format!("({}) -> {}", ps.join(", "), self.ret.suffix())
    }

    /// The same signature under a different name — how `print` and `log` share one host entry.
    pub fn renamed(&self, name: &str) -> HostSig {
        let mut s = self.clone();
        s.name = name.to_string();
        s
    }
}

/// The table `call.host` targets are checked against: `import_table` x `IFABI` x signature
/// (validation item 5, `R-0708`). `entries` is a `BTreeMap` so iteration order is bytewise and the
/// emitted `import_table` never depends on hash order (§7.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostSigTable {
    pub ifabi: u16,
    pub entries: BTreeMap<String, HostSig>,
}

impl Default for HostSigTable {
    fn default() -> Self {
        HostSigTable::new()
    }
}

impl HostSigTable {
    pub fn new() -> HostSigTable {
        HostSigTable { ifabi: IFABI_V2, entries: BTreeMap::new() }
    }

    pub fn register(&mut self, sig: HostSig) {
        self.entries.insert(sig.name.clone(), sig);
    }

    pub fn get(&self, name: &str) -> Option<&HostSig> {
        self.entries.get(name)
    }

    /// §10 / §7.7.1 item 4: the host entries a B-track frame may reach under IFABI-2. Every name in
    /// a capability table's `[builtin]` map must exist here, or the build stops (`E-0712`-adjacent
    /// `IngestErr::UnknownHost`).
    pub fn std_default() -> HostSigTable {
        use TyKind::*;
        let mut t = HostSigTable::new();
        let dyn_ = Instance;
        for (name, params, ret) in [
            ("std.io.log", vec![dyn_], Unit),
            ("std.io.warn", vec![dyn_], Unit),
            ("std.iter.len", vec![dyn_], Usize),
            ("std.iter.range", vec![Usize], Instance),
            ("std.iter.contains", vec![dyn_, dyn_], Bool),
            ("std.iter.collect", vec![dyn_], List),
            ("std.math.abs", vec![Int], Int),
            ("std.math.absf", vec![Float], Float),
            ("std.str.concat", vec![Str, Str], Str),
            ("std.str.from_dyn", vec![dyn_], Str),
            ("std.embed.to_int", vec![dyn_], Int),
            ("std.embed.to_float", vec![dyn_], Float),
            ("std.embed.to_bool", vec![dyn_], Bool),
            ("std.embed.to_str", vec![dyn_], Str),
            ("std.embed.to_list", vec![dyn_], List),
        ] {
            t.register(HostSig::new(name, &params, ret));
        }
        t
    }

    /// Add the entry-point signature of one embed unit; `lower` uses it as the parameter table
    /// (§7.7.1 rule 1) and `verify` checks `ret` against it.
    pub fn with_unit_sig(mut self, unit_name: &str, sig: HostSig) -> HostSigTable {
        self.entries.insert(unit_name.to_string(), sig);
        self
    }

    /// Validation item 5 for one `import_table` row: it must be a host row, its `ifabi` must be the
    /// module's, and its signature digest must equal the table's for the same name.
    pub fn check_import(&self, m: &IfbcModule, idx: usize) -> Result<(), String> {
        let e = m
            .imports
            .get(idx)
            .ok_or_else(|| format!("import_table 无第 {idx} 项（校验项 5 → R-0708）"))?;
        if e.kind != ImportKind::Host {
            return Err(format!(
                "call.host 目标 `{:?}` 不是 host 行（kind = {}）",
                m.strings.get(e.name).cloned().unwrap_or_default(),
                e.kind.word()
            ));
        }
        if e.ifabi != self.ifabi {
            return Err(format!(
                "host `{:?}` 的 IFABI-{} 与工具链 IFABI-{} 不符",
                m.strings.get(e.name).cloned().unwrap_or_default(),
                e.ifabi,
                self.ifabi
            ));
        }
        let name = m.strings.get(e.name).cloned().unwrap_or_default();
        let sig = self
            .entries
            .get(&name)
            .ok_or_else(|| format!("host `{name}` 不在 IFABI 签名表内（R-0708）"))?;
        if sig.sig_hash() != e.sig_hash {
            return Err(format!(
                "host `{name}` 签名不匹配：字节码记 {:#010x}，IFABI 记 {}",
                e.sig_hash,
                sig.text()
            ));
        }
        if sig.params.len() != e.params.len() {
            return Err(format!("host `{name}` 元数不符：{} 对 {}", e.params.len(), sig.params.len()));
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------ code builder

/// Emits `opcode:u8 + 0..3 u32` big-endian instructions (D.1) into one function's code, tracking the
/// offsets needed for back-patching jumps and for the `line_map`.
///
/// Offsets handed out and patched are **absolute offsets into the module `code` region**, because
/// that is what a jump operand holds; `base` is where this function's code will land.
#[derive(Clone, Debug)]
pub struct CodeBuilder {
    bytes: Vec<u8>,
    base: u32,
    /// (code_off, line) pairs collected as instructions are emitted; §7.4 requires the mapping back
    /// to the original language line.
    marks: Vec<(u32, u32)>,
    /// Instruction boundaries discovered while emitting, for the linearity self-check.
    bounds: Vec<u32>,
    locals: u32,
    scratch_next: u32,
}

/// A pending jump, patched once the target is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fixup {
    /// Absolute offset of the operand word inside the code region.
    pub at: u32,
    /// Where the jump must land.
    pub target_slot: usize,
}

impl CodeBuilder {
    pub fn new(base: u32) -> CodeBuilder {
        CodeBuilder {
            bytes: Vec::new(),
            base,
            marks: Vec::new(),
            bounds: Vec::new(),
            locals: 0,
            scratch_next: 0,
        }
    }

    /// Reserve `n` extra local-window slots for compiler scratch (temporaries, boxed intermediates).
    pub fn reserve_scratch(&mut self, n: u32) {
        self.scratch_next = n;
    }
    pub fn scratch_slots(&self) -> u32 {
        self.scratch_next
    }
    pub fn set_locals(&mut self, locals: u32) {
        self.locals = locals;
    }
    pub fn locals(&self) -> u32 {
        self.locals
    }

    /// Current absolute offset = the offset a jump target here would take.
    pub fn offset(&self) -> u32 {
        self.base + self.bytes.len() as u32
    }
    pub fn len(&self) -> u32 {
        self.bytes.len() as u32
    }
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
    /// Recorded line marks, ascending by `code_off` as emitted.
    pub fn marks(&self) -> &[(u32, u32)] {
        &self.marks
    }

    /// Emit an instruction. `line` is the original-language line the emitted code came from and is
    /// what makes `R-0703` diagnostics point at Python/Lua source (§7.4).
    pub fn emit(&mut self, op: Op, operands: &[u32], line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        if operands.len() != op.arity() {
            return Err(IfbcEncodeErr::WrongArity {
                op: op.base(),
                expected: op.arity(),
                given: operands.len(),
            });
        }
        if matches!(op, Op::Switch) {
            return Err(IfbcEncodeErr::SwitchNotSupported);
        }
        let off = self.offset();
        self.bounds.push(off);
        self.bytes.push(op.byte());
        for o in operands {
            put_u32(&mut self.bytes, *o);
        }
        if let Some(l) = line {
            self.marks.push((off, l));
        }
        Ok(off)
    }

    /// Emit and return the absolute offset of the operand slot `i`, so the caller can back-patch.
    pub fn emit_with_fixup(&mut self, op: Op, operands: &[u32], line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        let off = self.emit(op, operands, line)?;
        Ok(off + 1)
    }

    pub fn patch(&mut self, operand_off: u32, target: u32) -> Result<(), IfbcEncodeErr> {
        let rel = operand_off
            .checked_sub(self.base)
            .ok_or(IfbcEncodeErr::CodeRegionOverflow)? as usize;
        if rel + 4 > self.bytes.len() {
            return Err(IfbcEncodeErr::CodeRegionOverflow);
        }
        self.bytes[rel..rel + 4].copy_from_slice(&target.to_be_bytes());
        Ok(())
    }

    /// Append the built bytes into a module's `code` region, padding the *function start* to 4 bytes
    /// as required by D.1, and return `(code_off, code_len)`.
    pub fn finish_into(self, m: &mut IfbcModule) -> (u32, u32) {
        while m.code.len() % 4 != 0 {
            m.code.push(0);
        }
        let off = m.code.len() as u32;
        m.code.extend_from_slice(&self.bytes);
        (off, self.bytes.len() as u32)
    }

    // ---- typed helpers used by the lowering -------------------------------------------------

    pub fn ldl(&mut self, slot: u32, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::Ldl, &[slot], line)
    }
    pub fn stl(&mut self, slot: u32, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::Stl, &[slot], line)
    }
    pub fn push_pool(&mut self, pool: u32, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::Push, &[pool], line)
    }
    pub fn push_b(&mut self, v: u32, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::PushB, &[v], line)
    }
    pub fn pop(&mut self, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::Pop, &[], line)
    }
    pub fn ret(&mut self, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::Ret, &[], line)
    }
    pub fn nop(&mut self, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::Nop, &[], line)
    }
    /// `tickloop quota` — the timeout checkpoint of §7.7.1 item 3, inserted before every back edge.
    pub fn tickloop(&mut self, quota: u32, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::TickLoop, &[quota], line)
    }
    /// D.3: mandatory immediately before `div`/`rem`.
    pub fn guard_nonzero(&mut self, ty: u32, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::GuardNonzero, &[ty], line)
    }
    /// D.3: mandatory immediately before `shl`/`shr`/`shr.a`.
    pub fn guard_shift(&mut self, ty: u32, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::GuardShift, &[ty], line)
    }
    /// D.7 / UB-020: mandatory immediately before every `unbox.dyn`.
    pub fn is_dyn(&mut self, ty: u32, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::IsDyn, &[ty], line)
    }
    pub fn unbox_dyn(&mut self, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::UnboxDyn, &[], line)
    }
    pub fn box_dyn(&mut self, ty: u32, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::BoxDyn, &[ty], line)
    }
    /// A D.3 family instruction: `opcode` + operand 0 = type index (DEV-IFBC-1).
    pub fn family(&mut self, op: Op, ty: u32, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(op, &[ty], line)
    }
    /// A forward jump whose target is not known yet; returns the operand offset to patch.
    pub fn jump_placeholder(&mut self, op: Op, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit_with_fixup(op, &[0], line)
    }
    pub fn jmp(&mut self, target: u32, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::Jmp, &[target], line)
    }
    pub fn call_host(&mut self, import_idx: u32, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::CallHost, &[import_idx], line)
    }
    pub fn call_stat(&mut self, func_idx: u32, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::CallStat, &[func_idx], line)
    }
    pub fn enter_try(&mut self, handler: u32, c_end: u32, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::EnterTry, &[handler, c_end], line)
    }
    pub fn leave(&mut self, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::Leave, &[], line)
    }

/// One decoded instruction: opcode plus its fixed-width operands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Instr {
    pub op: Op,
    pub ops: [u32; MAX_OPS],
}

impl Instr {
    pub fn new(op: Op) -> Instr {
        Instr { op, ops: [0; MAX_OPS] }
    }
    pub fn with(op: Op, a: u32) -> Instr {
        Instr { op, ops: [a, 0, 0] }
    }
    pub fn with2(op: Op, a: u32, b: u32) -> Instr {
        Instr { op, ops: [a, b, 0] }
    }
    pub fn operands(&self) -> &[u32] {
        &self.ops[..self.op.arity()]
    }
    /// Encoded width in bytes: `1 + 4 * arity` (D.1).
    pub fn width(&self) -> usize {
        1 + 4 * self.op.arity()
    }
    /// Render `add.i64 3` given the module's tables.
    pub fn render(&self, m: &IfbcModule) -> String {
        let mut s = String::new();
        let base = self.op.base().to_string();
        if self.op.is_typed_family() {
            let t = m.ty_kind(self.ops[0]).map(|k| k.suffix()).unwrap_or("?");
            s.push_str(&base);
            s.push('.');
            s.push_str(t);
        } else if matches!(self.op, Op::IsDyn | Op::CastDyn | Op::BoxDyn) {
            s.push_str(&base);
            s.push('.');
            s.push_str(m.ty_kind(self.ops[0]).map(|k| k.suffix()).unwrap_or("?"));
        } else {
            s.push_str(&base);
        }
        let kinds = self.op.operand_kinds();
        for (i, o) in self.operands().iter().enumerate() {
            s.push(' ');
            let txt = match kinds.get(i) {
                Some(OperandKind::Pool) | Some(OperandKind::Slot) if false => String::new(),
                Some(OperandKind::Pool) => format!("{:?}@{o}", m.strings.get(*o).cloned().unwrap_or_default()),
                Some(OperandKind::Type) => ty_text(&m.types, &m.strings, *o),
                Some(OperandKind::Func) => format!("#{o} {}", m.funcs.get(*o as usize).map(|f| m.strings.get(f.name).cloned().unwrap_or_default()).unwrap_or_default()),
                Some(OperandKind::Import) => format!("#{o} {}", m.imports.get(*o as usize).map(|e| m.strings.get(e.name).cloned().unwrap_or_default()).unwrap_or_default()),
                Some(OperandKind::CodeOff) => format!("@{o:#06x}"),
                _ => format!("{o}"),
            };
            s.push_str(&txt);
        }
        s
    }
}
