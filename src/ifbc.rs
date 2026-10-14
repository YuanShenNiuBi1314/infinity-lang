//! IFBC — INFY ByteCode container, assembler, decoder, disassembler, loader verifier.
//!
//! §7.8 + appendix D + `docs/internals/runtime-abi.md` §9. This module is the only place on the
//! Rust side that knows the IFBC byte format; it is pure data (no I/O, no diagnostics, no front-end
//! dependency) so that both the B-track ingest (`crate::btrack`) and `infy ifbc dump|verify`
//! (§7.8.4) sit on one implementation.
//!
//! Container, as fixed by deviation **D-38** (the one wire format both sides now align to):
//!
//! ```text
//! header | type_table | import_table | func_table | code | line_map | exception_table | string_pool
//! header: magic "IFB1" | ifbc_version u16 | ifabi u16 | lang_tag u16 | module_hash [u8;32]
//!         | func_count u32 | frame_kind u8                        (= 47 bytes)
//! ```
//!
//! Every region after the header is **self-delimiting**: the tables and the trailing regions open
//! with their own `u32` count and `code` with its own `u32` byte length (`runtime/IFBC.md` §1).
//! This module used to wrap each region in an extra `byte_len:u32 || body` and called the wrapper a
//! spec hole; D-38 removed it, because `code`'s own leading length already separates it from
//! `line_map`, which is what `runtime/infy_ifbc.c` has always walked. All integers are big-endian
//! (D.1), an instruction is `opcode:u8` + its opcode-implied `u32` operands, and function entry
//! offsets and the end of the code region are 4-byte aligned.
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
/// §7.8.1 header `frame_kind:u8`: the module-wide value model. 0 = single-address stack machine +
/// local window, and 0 is the **only** legal header value (D-23). The frame kind of an individual
/// function is `FuncEntry::frame`, below.
pub const FRAME_KIND_STACK: u8 = 0;
/// Appendix D.8's per-function coroutine frame. The spec names the value but never numbers it, so
/// `1` is this implementation's fixed choice, recorded as deviation D-23.
pub const FRAME_KIND_COROUTINE: u8 = 1;
/// Sentinel for "no type / no name" in a u32 field.
pub const NONE32: u32 = u32::MAX;
/// Maximum operands per instruction (D.1: 0..3).
pub const MAX_OPS: usize = 3;
/// D-38: the row caps both sides share. `runtime/infy_ifbc.c` rejects an absurd count with the
/// umbrella load failure rather than trusting a `u32`, so the build-time decoder caps the same way.
pub const MAX_TABLE_ROWS: u32 = 1 << 20;
/// The cap for the three count-prefixed regions at the tail of the container.
pub const MAX_LONG_ROWS: u32 = 1 << 24;
/// `switch n` case-table cap: C's `insn_len` treats `n > OPS_MAX * 16` as hostile input.
pub const MAX_SWITCH_CASES: u32 = 48;
/// D-38: the first `type_table` rows are the IFABI scalar codes (§2 of `runtime/IFBC.md`), so a
/// type index in `0..N_SCALAR_ROWS` **is** a scalar code. `intern_type` seeds them on first use.
pub const N_SCALAR_ROWS: usize = 18;

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
        .map(|((l, f), _)| ((*l as u16) << 8) | *f as u16)
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

/// Type-class kinds. **D-38 renumbered** this enum into the IFABI scalar-code space: `0..=17` are
/// exactly `SIG_*` in `runtime/infy_ifbc.c` (`void, unit, bool, char, i8, i16, i32, i64, u8, u16,
/// u32, u64, f32, f64, str, bytes, ptr, dyn`), which is what lets an `import_table` row's `u8`
/// signature codes and a `type_table` index name the *same* space. Kinds `18..` are the extended
/// static classes D-38 lists after the scalar block.
///
/// The old numbering copied `InfyDynTag`'s order for `0..=11`; that identity is gone, so
/// [`TyKind::dyn_tag`] is an explicit table now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum TyKind {
    // ---- 0..=17: the IFABI scalar codes, the closed signature domain (§2 of `IFBC.md`)
    Void = 0,
    Unit = 1,
    Bool = 2,
    Char = 3,
    I8 = 4,
    I16 = 5,
    I32 = 6,
    /// D.3's `i64` width; B-track's Python int.
    Int = 7,
    U8 = 8,
    U16 = 9,
    U32 = 10,
    U64 = 11,
    F32 = 12,
    /// D.3's `f64` width.
    Float = 13,
    Str = 14,
    Bytes = 15,
    Ptr = 16,
    Dyn = 17,
    // ---- 18..: extended static classes, not expressible as an IFABI signature code
    List = 18,
    Tuple = 19,
    Dict = 20,
    Func = 21,
    Coro = 22,
    Instance = 23,
    Ref = 24,
    Box = 25,
    Rc = 26,
    Arc = 27,
    Option = 28,
    Result = 29,
    Never = 30,
    Usize = 31,
    Isize = 32,
}

impl TyKind {
    pub fn as_u8(self) -> u8 {
        self as u8
    }
    /// The `type_table.kind` field is a `u32` (D-38 merged shape).
    pub fn as_u32(self) -> u32 {
        self as u32
    }
    pub fn from_u8(b: u8) -> Option<TyKind> {
        Self::from_u32(b as u32)
    }
    pub fn from_u32(b: u32) -> Option<TyKind> {
        Some(match b {
            0 => TyKind::Void,
            1 => TyKind::Unit,
            2 => TyKind::Bool,
            3 => TyKind::Char,
            4 => TyKind::I8,
            5 => TyKind::I16,
            6 => TyKind::I32,
            7 => TyKind::Int,
            8 => TyKind::U8,
            9 => TyKind::U16,
            10 => TyKind::U32,
            11 => TyKind::U64,
            12 => TyKind::F32,
            13 => TyKind::Float,
            14 => TyKind::Str,
            15 => TyKind::Bytes,
            16 => TyKind::Ptr,
            17 => TyKind::Dyn,
            18 => TyKind::List,
            19 => TyKind::Tuple,
            20 => TyKind::Dict,
            21 => TyKind::Func,
            22 => TyKind::Coro,
            23 => TyKind::Instance,
            24 => TyKind::Ref,
            25 => TyKind::Box,
            26 => TyKind::Rc,
            27 => TyKind::Arc,
            28 => TyKind::Option,
            29 => TyKind::Result,
            30 => TyKind::Never,
            31 => TyKind::Usize,
            32 => TyKind::Isize,
            _ => return None,
        })
    }
    /// Inside the closed IFABI signature domain (`<= SIG_DYN`), so it can appear as an
    /// `import_table` `ret_sig`/`param_sig` byte.
    pub fn is_scalar_code(self) -> bool {
        (self as u32) <= TyKind::Dyn as u32
    }
    /// The signature code an `import_table` row may carry for this kind: anything outside the
    /// closed set is a boxed dynamic value on the IFABI wire (`SIG_DYN = 17`), which is what
    /// §7.7.7's B-track representation says a container or instance *is*.
    pub fn sig_code(self) -> u8 {
        if self.is_scalar_code() {
            self as u8
        } else {
            TyKind::Dyn as u8
        }
    }
    /// The `InfyDynTag` of the dyn-visible kinds (§7.7.7), explicit since D-38's renumbering.
    pub fn dyn_tag(self) -> Option<u8> {
        match self {
            TyKind::Unit => Some(0),     // DY_NONE
            TyKind::Int => Some(1),      // DY_INT
            TyKind::Float => Some(2),    // DY_FLOAT
            TyKind::Str => Some(3),      // DY_STR
            TyKind::Bytes => Some(4),    // DY_BYTES
            TyKind::List => Some(5),     // DY_LIST
            TyKind::Tuple => Some(6),    // DY_TUPLE
            TyKind::Dict => Some(7),     // DY_DICT
            TyKind::Bool => Some(8),     // DY_BOOL
            TyKind::Func => Some(9),     // DY_FUNC
            TyKind::Coro => Some(10),    // DY_CORO
            TyKind::Dyn => Some(11),     // DY_INSTANCE
            _ => None,
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
    /// `i64`, `f32`, … — the suffix text of a mnemonic such as `add.i64`, and the pool name of the
    /// scalar row the kind seeds.
    pub fn suffix(self) -> &'static str {
        match self {
            TyKind::Void => "void",
            TyKind::Unit => "unit",
            TyKind::Bool => "bool",
            TyKind::Char => "char",
            TyKind::I8 => "i8",
            TyKind::I16 => "i16",
            TyKind::I32 => "i32",
            TyKind::Int => "i64",
            TyKind::U8 => "u8",
            TyKind::U16 => "u16",
            TyKind::U32 => "u32",
            TyKind::U64 => "u64",
            TyKind::F32 => "f32",
            TyKind::Float => "f64",
            TyKind::Str => "str",
            TyKind::Bytes => "bytes",
            TyKind::Ptr => "ptr",
            TyKind::Dyn => "dyn",
            TyKind::List => "list",
            TyKind::Tuple => "tuple",
            TyKind::Dict => "dict",
            TyKind::Func => "func",
            TyKind::Coro => "coro",
            TyKind::Instance => "instance",
            TyKind::Ref => "ref",
            TyKind::Box => "box",
            TyKind::Rc => "rc",
            TyKind::Arc => "arc",
            TyKind::Option => "option",
            TyKind::Result => "result",
            TyKind::Never => "never",
            TyKind::Usize => "usize",
            TyKind::Isize => "isize",
        }
    }
}

/// `type_table.flags` bit 0 = `#[repr(C)]`, the precondition of `field_off ty,field` (D.9).
pub const TF_REPR_C: u32 = 1 << 0;

/// One `type_table` row in D-38's merged shape: the six fields `runtime/infy_ifbc.c` reads, plus
/// the two payloads this side carries for container elements and generic arguments (the C side
/// ignores them, so they are not new semantics).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeEntry {
    pub kind: u32,
    pub name: u32,
    pub flags: u32,
    pub size: u32,
    pub align: u32,
    /// `nfields` on the wire, then `nfields × {name_str, off, ty}`.
    pub fields: Vec<TypeField>,
    /// `payload_a` / `payload_b`: for `list`/`tuple` `a` is the element type index, for `dict`
    /// `a`/`b` are key/value; otherwise `NONE32`.
    pub a: u32,
    pub b: u32,
}

/// One field record of a `type_table` row; `ty` is a `type_table` index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeField {
    pub name: u32,
    pub off: u32,
    pub ty: u32,
}

impl TypeEntry {
    /// The row D-38 auto-seeds for a scalar code: name only, no layout claim, no payload. The
    /// producer that needs `size_of.idx`/`align_of.idx`/`field_off` on a row fills those in.
    fn scalar_row(name: u32, kind: u32) -> TypeEntry {
        TypeEntry {
            kind,
            name,
            flags: 0,
            size: 0,
            align: 0,
            fields: Vec::new(),
            a: NONE32,
            b: NONE32,
        }
    }
}

/// What an `import_table` row refers to. D-38 folded this side's `PyModule`/`Infib`/`Entry` classes
/// into the three the loader knows: the source language is the header's `lang_tag`, and an entry
/// point is `FuncEntry::flags`' `F_ENTRY`, so neither needs an import row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ImportKind {
    Host = 0,
    Embed = 1,
    Event = 2,
}

/// `import_table.flags` bit 0 = "IFABI-signed": the compiler stamps it after checking the row
/// against the IFABI thunk table (`IFBC.md` §5.5, validation item 5).
pub const IMP_SIGNED: u8 = 1 << 0;

impl ImportKind {
    pub fn as_u8(self) -> u8 {
        self as u8
    }
    pub fn from_u8(b: u8) -> Option<ImportKind> {
        Some(match b {
            0 => ImportKind::Host,
            1 => ImportKind::Embed,
            2 => ImportKind::Event,
            _ => return None,
        })
    }
    pub fn word(self) -> &'static str {
        match self {
            ImportKind::Host => "host",
            ImportKind::Embed => "embed",
            ImportKind::Event => "event",
        }
    }
}

/// One `import_table` row: an external name plus the signature it is checked against. `ret_sig`
/// and `params` are **IFABI scalar codes** (§2 of `IFBC.md`), i.e. the first `N_SCALAR_ROWS`
/// `type_table` indices, so a `u8` here and a `u32` type index name one space (D-38).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportEntry {
    pub name: u32,
    pub kind: ImportKind,
    pub flags: u8,
    pub ret_sig: u8,
    pub params: Vec<u8>,
    /// Validation item 5's fast comparison value; the text stays rebuildable from the codes.
    pub sig_hash: u32,
}

impl ImportEntry {
    pub fn sig_text(&self) -> String {
        let ps: Vec<String> = self.params.iter().map(|c| sig_name(*c).to_string()).collect();
        format!("({}) -> {}", ps.join(", "), sig_name(self.ret_sig))
    }
}

/// The scalar-code name space: `ret_sig`/`param_sig` bytes rendered for `dump`.
pub fn sig_kind(code: u8) -> Option<TyKind> {
    TyKind::from_u32(code as u32).filter(|k| k.is_scalar_code())
}

pub fn sig_name(code: u8) -> &'static str {
    match sig_kind(code) {
        Some(k) => k.suffix(),
        None => "?",
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
            match TyKind::from_u32(t.kind) {
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
pub const F_ENTRY: u32 = 1 << 0;
pub const F_COROUTINE: u32 = 1 << 1;
pub const F_BOUNDARY: u32 = 1 << 2;
/// `func_table.flags` bit 0 on the C side is "has_return" (`IFBC.md` §1.5); D-38 keeps both names
/// for the same bit set so neither side has to guess.
pub const F_HAS_RETURN: u32 = 1 << 0;

/// One `func_table` row: D-38's 48-byte merged record, twelve `u32` in the loader's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuncEntry {
    pub code_off: u32,
    pub code_len: u32,
    /// Local-window slot count: parameters + locals + compiler scratch (§7.7.1 rule 1).
    pub locals: u32,
    /// Declared operand-stack ceiling; the height pass rejects a height above it (R-0705).
    pub max_stack: u32,
    pub nargs: u32,
    /// 0 = unit, 1 = scalar, n = tuple — `ret`/`ret.multi` must match it (item 4).
    pub ret_arity: u32,
    /// The return type as a scalar code (§2 domain), diagnostic; `ret_arity` carries the shape.
    pub ret_sig: u32,
    pub name: u32,
    /// String-pool index of the canonical signature text, for `infy doc` and diagnostics.
    pub sig_str: u32,
    /// `HostSigTable::sig_hash` of that signature: validation item 5 compares bytes, not names.
    pub sig_hash: u32,
    pub flags: u32,
    /// 0 = stack + local window (§7.8.1), 1 = coroutine frame (D.8, deviation D-23).
    pub frame_kind: u32,
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
    /// D-38: `0x0C`/`0x0D` are the C loader's old `guard.nonzero`/`guard.shift` bytes, which move
    /// back to D.3's own block (`0x27`/`0x2F`); the global-window pair takes the next free gap.
    Ldg = 0x0E,
    Stg = 0x0F,

    // ---- D.3 arithmetic and comparison, 0x20..0x3F (operand 0 = type index)
    Add = 0x20,
    Sub = 0x21,
    Mul = 0x22,
    Div = 0x23,
    Rem = 0x24,
    Neg = 0x25,
    Abs = 0x26,
    /// D.3 mandates `guard.nonzero` immediately before `div`/`rem` but assigns it no byte; 0x27 is
    /// D.3's own gap, and D-38 (c) pins it there with **0 operands** (the C side's 0x0C/0x0D pair
    /// migrates here, and the type travels on the guarded instruction's own operand 0).
    GuardNonzero = 0x27,
    And = 0x28,
    Or = 0x29,
    Xor = 0x2A,
    Not = 0x2B,
    Shl = 0x2C,
    Shr = 0x2D,
    ShrA = 0x2E,
    /// D.3's `guard.shift`, same reasoning as `GuardNonzero`; 0x2F is the gap before `cmplt`, and
    /// D-38 (c) gives it **0 operands** as well.
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
    /// D.4's `switch n, [targets]`: one operand (the case count) plus an **inline** `n × u32`
    /// target table, so its length is `1 + 4*(1+n)` — statically computable, still a single
    /// forward pass. D-38/D-39: this is unbanned (the earlier `SwitchNotSupported` refusal was
    /// "以拒绝冒充合规"); `MAX_SWITCH_CASES` mirrors the loader's hostile-count cap.
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
    /// D.6 gap 0x92/0x93: tuple and set construction (`DEV-IFBC-7`), needed because §7.7.1 maps a
    /// Python multi-return onto a tuple and `BUILD_TUPLE` is a plain Python instruction.
    NewTuple = 0x92,
    NewSet = 0x93,
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

/// Every appendix-D row that groups more than one mnemonic or leaves an operand count open, with
/// the byte/width assignment this implementation makes. Printed by `disassemble`'s legend and
/// mirrored in `docs/internals/ifbc-encoding-rust.md`, and since **D-38** it is the table both
/// sides share (the `DEV-IFBC-*` ids below were "this side's choice" until D-38 made them law).
pub const GROUPING: [(&str, &str); 14] = [
    ("D.2 0x06", "dup / dup2 / swap -> 0x06 dup, 0x0A dup2, 0x0B swap"),
    ("D.2 0x0A..0x1F", "unassigned gap -> 0x0E ldg, 0x0F stg (global window); 0x0C/0x0D are free now that guard.* sit at their D.3 bytes"),
    ("D.2 0x03", "ldw i,ty -> 2 operands; operand 1 is a static type annotation the executor never reads: D-38 (e)"),
    ("D.2 0x04", "push k,ty -> 2 operands: a pool constant must carry its type: D-38 (d) / DEV-IFBC-13"),
    ("D.3 all rows", "family byte + operand 0 = type_table index, not one byte per (op,type): D-38 (b) / DEV-IFBC-1"),
    ("D.3 0x27, 0x2F", "guard.nonzero, guard.shift: 0 operands, D.3's adjacency only ever inspects the previous byte: D-38 (c)"),
    ("D.4 0x43", "switch n + n inline u32 targets, length 1+4*(1+n): D-38 (h) / D-39 (the old fixed-width reading)")  ,
    ("D.5 0x68, 0x69", "import, import.attr: DEV-IFBC-6, confirmed by D-38 (i)"),
    ("D.6 0x81, 0x82", "new.list elem_ty,count / new.dict k_ty,v_ty,count: D.6 names the type operands only; the count is what makes the stack form decidable (D-38 addendum)"),
    ("D.6 0x83", "new.frame f: D.6 lists no operand, but a frame record must name its coroutine function (D-38 addendum)"),
    ("D.6 0x84..0x87", "seq.push, seq.len, seq.next, seq.iter: DEV-IFBC-7"),
    ("D.6 0x8B..0x8F", "inst.get, inst.set, map.insert, str.concat, seq.concat: DEV-IFBC-7"),
    ("D.6 0x92, 0x93", "new.tuple, new.set: DEV-IFBC-7"),
    ("D.7 0xA3", "box.dyn: 0 operands, B-track values are already InfyDyn: D-38 (f). D.7 0xA5/0xA6 dyn.truthy, dyn.identity: DEV-IFBC-7. D.9 0xE0 typeof.idx T and 0xe3 field_off idx,fld: D.9's mnemonic column drops operands the check needs (D-38 addendum / D-39)"),
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
            0x0E => Op::Ldg,
            0x0F => Op::Stg,
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
            0x92 => Op::NewTuple,
            0x93 => Op::NewSet,
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
            Op::NewTuple => "new.tuple",
            Op::NewSet => "new.set",
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
    /// `switch` is the one exception: its count *value* is operand 0 and its target table follows
    /// inline, so `arity()` is 1 and [`Op::switch_extra`] counts the rest (D-38 (h), D-39).
    pub fn arity(self) -> usize {
        match self {
            // zero operands
            Op::Nop
            | Op::Dup
            | Op::Dup2
            | Op::Swap
            | Op::Pop
            | Op::LdNull
            | Op::LdFrame
            | Op::Ret
            | Op::Leave
            | Op::FrameEnter
            | Op::FrameExit
            | Op::TagGet
            | Op::UnboxDyn
            | Op::BoxDyn
            | Op::DynTruthy
            | Op::DynIdentity
            | Op::Yield
            | Op::YieldFrom
            | Op::RefInc
            | Op::RefDec
            // D-38 (c): the guards carry no type operand; the guarded instruction has its own.
            | Op::GuardNonzero
            | Op::GuardShift => 0,
            // one operand
            Op::Ldl
            | Op::Stl
            | Op::Ldg
            | Op::Stg
            | Op::PushB
            | Op::Jmp
            | Op::Br
            | Op::Brf
            | Op::TickLoop
            | Op::ThrowInf
            | Op::CallStat
            | Op::CallVirt
            | Op::CallHost
            | Op::TailCallStat
            | Op::Import
            | Op::ImportAttr
            | Op::NewStr
            | Op::NewFrame
            | Op::IdxGet
            | Op::IdxSet
            | Op::Slice
            | Op::SeqLen
            | Op::SeqIter
            | Op::SeqNext
            | Op::SeqPush
            | Op::MapInsert
            | Op::InstGet
            | Op::InstSet
            | Op::StrConcat
            | Op::SeqConcat
            | Op::MoveOut
            | Op::CastDyn
            | Op::IsDyn
            | Op::CallEmbed
            | Op::InvokeDynamic
            | Op::CoroResume
            | Op::CoroThrow
            | Op::SizeOf
            | Op::AlignOf
            | Op::TypeOfIdx
            | Op::RetMulti
            // `switch n`: operand 0 is the case count, the table is inline (D-38 (h)).
            | Op::Switch
            // D.3 families: operand 0 is the type index (DEV-IFBC-1, pinned by D-38 (b))
            | Op::Add
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
            | Op::Cmp => 1,
            // two operands — `push k,ty` (D-38 (d)) and `ldw i,ty` (D-38 (e)), plus `field_off
            // ty,field` (D-38 (g)); `new.list elem_ty,count` / `closure.new` / `enter.try`
            Op::NewList | Op::ClosureNew | Op::EnterTry | Op::Push | Op::Ldw | Op::FieldOff => 2,
            // one operand (added by this implementation, see the D.6 gap note)
            Op::NewTuple | Op::NewSet => 1,
            // two type operands plus this implementation's element count (D-38 addendum)
            Op::NewDict => 3,
        }
    }

    /// `switch` is the only opcode whose length depends on an operand value: the `n` case targets
    /// sit inside the instruction, so the linear pass still needs one forward scan (D-39).
    pub fn is_variable_width(self) -> bool {
        matches!(self, Op::Switch)
    }

    /// Operand forms, positionally. A D.3 family's operand 0 is always `Type`.
    pub fn operand_kinds(self) -> Vec<OperandKind> {
        use OperandKind::*;
        match self {
            Op::Ldl | Op::Stl | Op::Ldg | Op::Stg | Op::MoveOut => vec![Slot],
            // D-38 (e): `ldw i,ty` — the annotation is operand 1 and the executor never reads it.
            Op::Ldw => vec![Slot, Type],
            Op::ThrowInf | Op::NewStr | Op::ImportAttr | Op::InstGet | Op::InstSet => vec![Pool],
            Op::Push => vec![Pool, Type],
            Op::PushB | Op::TickLoop | Op::RetMulti | Op::CoroResume | Op::CoroThrow => vec![Imm],
            // D-38 addendum: D.9's mnemonic column drops the index these checks need, so the
            // mirror of `runtime/infy_ifbc.c`'s `check_type` is that they are `type_table` indices.
            Op::TypeOfIdx | Op::SizeOf | Op::AlignOf => vec![Type],
            // D-38 (g): `field_off ty,field`.
            Op::FieldOff => vec![Type, Imm],
            Op::Jmp | Op::Br | Op::Brf => vec![CodeOff],
            Op::CallStat | Op::TailCallStat | Op::NewFrame => vec![Func],
            Op::CallHost | Op::Import => vec![Import],
            // `call.virt idx` is the vtable slot's type index (same check as C's).
            Op::CallVirt => vec![Type],
            // `call.embed idx` addresses an import row like `call.host` (§5.5 checks the row's kind
            // against the call site); `invoke.dynamic f` is a `func_table` index this side rejects
            // with R-0712, so its operand stays an immediate (see `verify_func`'s item-5 pass).
            Op::CallEmbed => vec![Import],
            Op::InvokeDynamic => vec![Imm],
            Op::ClosureNew => vec![Func, Imm],
            Op::IdxGet | Op::IdxSet | Op::Slice | Op::SeqLen | Op::SeqIter | Op::SeqNext
            | Op::SeqPush | Op::CastDyn | Op::IsDyn | Op::StrConcat | Op::SeqConcat
            | Op::MapInsert => vec![Type],
            Op::NewList => vec![Type, Imm],
            Op::NewTuple => vec![Imm],
            Op::NewSet => vec![Type],
            Op::NewDict => vec![Type, Type, Imm],
            Op::EnterTry => vec![CodeOff, CodeOff],
            // operand 0 is the case count; the targets are inline (verified as code offsets by
            // the switch branch of item 1, not by the positional kinds).
            Op::Switch => vec![Imm],
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

    /// The six two-operand comparisons of D.3: they push a `bool`, not the operand type.
    pub fn is_cmp(self) -> bool {
        matches!(
            self,
            Op::CmpLt | Op::CmpLe | Op::CmpGt | Op::CmpGe | Op::CmpEq | Op::CmpNe
        )
    }

    /// Binary arithmetic/comparison families: two operands on the stack.
    pub fn is_family(self) -> bool {
        self.is_typed_family() || matches!(self, Op::GuardNonzero | Op::GuardShift)
    }

    /// `(pops, pushes)` for the stack-height validation (§7.8.3 item 2), for the instructions whose
    /// effect is opcode-implied. Table-dependent instructions (`call.*`, `new.list`, `new.dict`,
    /// `closure.new`) are resolved by the verifier from the tables — see `effect`.
    pub fn stack_effect(self) -> (usize, usize) {
        use Op::*;
        match self {
            Nop | LdFrame | GuardNonzero | GuardShift | TickLoop | EnterTry | Leave | FrameEnter
            | FrameExit | Jmp | Br | Brf | ThrowInf | Ret | RetMulti | Yield | YieldFrom
            | CoroResume | CoroThrow | RefDec | Import | RefInc => (0, 0),
            // D-38 (h): `switch n` pops the case index and pushes nothing (the successor is the
            // selected target or the byte after the inline table) — C's `delta = -1`.
            Switch => (1, 0),
            Ldl | Ldw | Push | PushB | LdNull | Ldg | SeqLen | NewStr | NewFrame | TypeOfIdx
            | SizeOf | AlignOf | FieldOff | DynTruthy => (0, 1),
            Stl | Stg | Pop | MoveOut | NewTuple | NewSet => (1, 0),
            Dup => (1, 2),
            Dup2 => (2, 4),
            Swap => (2, 2),
            Neg | Abs | Not | TagGet | IsDyn | BoxDyn | CastDyn | UnboxDyn | InstGet | ImportAttr
            | SeqIter | InvokeDynamic | CallVirt | CallEmbed => (1, 1),
            Add | Sub | Mul | Div | Rem | And | Or | Xor | Shl | Shr | ShrA | CmpLt | CmpLe
            | CmpGt | CmpGe | CmpEq | CmpNe | Cmp | DynIdentity | StrConcat | SeqConcat
            | IdxGet => (2, 1),
            SeqNext => (1, 2),
            IdxSet => (3, 0),
            InstSet => (2, 0),
            Slice => (3, 1),
            SeqPush => (2, 1),
            MapInsert => (3, 1),
            NewList | NewDict | NewTuple | NewSet | ClosureNew | CallStat | CallHost
            | TailCallStat => (0, 0),
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
    /// `switch` with more cases than the loader will scan (`MAX_SWITCH_CASES`): refusing to emit it
    /// is what keeps an image this side writes decodable by `runtime/infy_ifbc.c`.
    SwitchCaseCountTooLarge { given: usize },
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
            IfbcEncodeErr::SwitchCaseCountTooLarge { given } => write!(
                f,
                "switch 的目标表有 {given} 项，超过装载器上限 {MAX_SWITCH_CASES}（D-38 (h)）"
            ),
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
    /// `switch n` with `n > MAX_SWITCH_CASES`: C's `insn_len` treats an absurd count as hostile and
    /// fails the linear pass, so the build-time decoder refuses it the same way (D-38 (h)).
    SwitchCasesTooLarge { off: u32, n: u32 },
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
            IfbcDecodeErr::SwitchCasesTooLarge { off, n } => write!(
                f,
                "switch@{off:#06x} 的目标表有 {n} 项，超过装载器上限 {MAX_SWITCH_CASES}（敌意输入）"
            ),
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
    /// §7.9's per-unit arena budget, the `u64` the loader reads right after `n_funcs`
    /// (`runtime/IFBC.md` §1.5). 0 = no limit; over budget at run time is `R-0709`.
    pub max_mem_bytes: u64,
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
            max_mem_bytes: 0,
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
fn put_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_be_bytes());
}
fn be_u16(b: &[u8]) -> Option<u16> {
    b.get(..2).map(|x| u16::from_be_bytes([x[0], x[1]]))
}
fn be_u32(b: &[u8]) -> Option<u32> {
    b.get(..4)
        .map(|x| u32::from_be_bytes([x[0], x[1], x[2], x[3]]))
}
fn be_u64(b: &[u8]) -> Option<u64> {
    b.get(..8)
        .map(|x| u64::from_be_bytes([x[0], x[1], x[2], x[3], x[4], x[5], x[6], x[7]]))
}

impl IfbcModule {
    /// A module that can only come from the B-track ingest of `format` (`CPY-313`, `LUA-54`).
    /// `unit` and `source_name` are interned first so they occupy pool slots 0 and 1 — `decode`
    /// recovers them from those positions, which is what keeps `dump` able to name the origin of
    /// every line (§7.4).
    pub fn new_unit(format: &str, cap_version: u32, unit: &str, source_name: &str) -> Option<IfbcModule> {
        let tag = lang_tag_of(format)?;
        let mut m = IfbcModule {
            lang: LangTag { tag, cap_version },
            unit: unit.to_string(),
            source_name: source_name.to_string(),
            ..IfbcModule::default()
        };
        m.strings.intern(unit);
        m.strings.intern(source_name);
        m.seed_scalar_rows();
        Some(m)
    }

    /// Intern a string into the pool (first-appearance order, therefore deterministic).
    pub fn intern(&mut self, s: &str) -> u32 {
        self.strings.intern(s)
    }

    /// D-38: write the `N_SCALAR_ROWS` scalar rows so **type index `0..=17` *is* the IFABI scalar
    /// code**. That identity is what lets an `import_table` `u8` signature code and a `type_table`
    /// `u32` index address one space. No-op once any row exists (including a decoded image, whose
    /// producer owns the invariant). `size`/`align`/`flags` are the producer's payload: nothing is
    /// claimed here, because a wrong layout number is worse than an empty one.
    pub fn seed_scalar_rows(&mut self) {
        if !self.types.is_empty() {
            return;
        }
        for k in 0..N_SCALAR_ROWS {
            let kind = TyKind::from_u32(k as u32).expect("the scalar block is 0..=17");
            let name = self.strings.intern(kind.suffix());
            self.types.push(TypeEntry::scalar_row(name, kind.as_u32()));
        }
    }

    /// Add (or reuse) a type-table row; identity is `(kind, name, a, b)` so interning is stable.
    /// A scalar kind resolves to **its own code index**, which is what keeps the seeded block
    /// from growing a second `i64` row.
    pub fn intern_type(&mut self, kind: TyKind, name: &str, a: u32, b: u32) -> u32 {
        self.seed_scalar_rows();
        if kind.is_scalar_code() && self.types[kind as usize].kind == kind.as_u32() {
            return kind as u32;
        }
        let name_idx = self.strings.intern(name);
        for (i, t) in self.types.iter().enumerate() {
            if t.kind == kind.as_u32() && t.name == name_idx && t.a == a && t.b == b {
                return i as u32;
            }
        }
        self.types.push(TypeEntry {
            kind: kind.as_u32(),
            name: name_idx,
            flags: 0,
            size: 0,
            align: 0,
            fields: Vec::new(),
            a,
            b,
        });
        (self.types.len() - 1) as u32
    }

    /// Attach a field to a `type_table` row (the D.9 `field_off ty,field` target set).
    pub fn add_type_field(&mut self, ty: u32, field_name: &str, off: u32, field_ty: u32) {
        let name = self.strings.intern(field_name);
        if let Some(t) = self.types.get_mut(ty as usize) {
            t.fields.push(TypeField { name, off, ty: field_ty });
        }
    }

    /// Stamp `#[repr(C)]` (`flags` bit 0), the precondition `field_off` is checked against.
    pub fn set_type_repr_c(&mut self, ty: u32) {
        if let Some(t) = self.types.get_mut(ty as usize) {
            t.flags |= TF_REPR_C;
        }
    }

    pub fn ty_kind(&self, idx: u32) -> Option<TyKind> {
        self.types.get(idx as usize).and_then(|t| TyKind::from_u32(t.kind))
    }

    pub fn type_name(&self, idx: u32) -> Option<String> {
        let t = self.types.get(idx as usize)?;
        Some(self.strings.get(t.name)?.clone())
    }

    /// Register an `import_table` row for a host function, returning its index. The signature is
    /// taken from `hosts`, which is what makes validation item 5 checkable. The row is stamped
    /// `IMP_SIGNED` because it only exists once the name has been checked against the IFABI table.
    pub fn intern_host(&mut self, hosts: &HostSigTable, name: &str) -> Option<u32> {
        let sig = hosts.get(name)?;
        // `nparams` is a `u8` on the wire (the loader's row shape); a signature that cannot be
        // written must not be written partly.
        if sig.params.len() > u8::MAX as usize {
            return None;
        }
        self.seed_scalar_rows();
        if let Some(i) = self.imports.iter().position(|e| {
            e.kind == ImportKind::Host && self.strings.get(e.name).map(|s| s.as_str()) == Some(name)
        }) {
            return Some(i as u32);
        }
        let name_idx = self.strings.intern(name);
        // The signature codes address the seeded scalar block; intern the rows too so a
        // `dump`/`verify` of the module can name them.
        for k in sig.params.iter().chain(std::iter::once(&sig.ret)) {
            let _ = self.intern_tykind(*k);
        }
        let params: Vec<u8> = sig.params.iter().map(|k| k.sig_code()).collect();
        let ret_sig = sig.ret.sig_code();
        self.imports.push(ImportEntry {
            name: name_idx,
            kind: ImportKind::Host,
            flags: IMP_SIGNED,
            ret_sig,
            params,
            sig_hash: sig.sig_hash(),
        });
        Some((self.imports.len() - 1) as u32)
    }

    /// Register an `import_table` row for an ingested module (scan item 3). D-38 folds this side's
    /// `PyModule`/`Infib` classes into `embed`: the source language is the header's `lang_tag`, and
    /// an imported module is exactly "a unit the embed track reaches".
    pub fn intern_module_import(&mut self, name: &str) -> u32 {
        self.seed_scalar_rows();
        if let Some(i) = self.imports.iter().position(|e| {
            e.kind == ImportKind::Embed && self.strings.get(e.name).map(|s| s.as_str()) == Some(name)
        }) {
            return i as u32;
        }
        let name_idx = self.strings.intern(name);
        let _ = self.intern_tykind(TyKind::Dyn);
        self.imports.push(ImportEntry {
            name: name_idx,
            kind: ImportKind::Embed,
            flags: 0,
            ret_sig: TyKind::Dyn.sig_code(),
            params: Vec::new(),
            sig_hash: 0,
        });
        (self.imports.len() - 1) as u32
    }

    fn intern_tykind(&mut self, k: TyKind) -> u32 {
        self.intern_type(k, k.suffix(), NONE32, NONE32)
    }

    /// Append one function to `func_table`, returning its index. `code` must already have been
    /// appended (by [`CodeBuilder::finish_into`]) so `code_off`/`code_len` are known, and
    /// `max_stack` is the producer's declared operand-stack ceiling (the height pass checks it).
    pub fn push_func(
        &mut self,
        name: &str,
        code_off: u32,
        code_len: u32,
        locals: u32,
        max_stack: u32,
        params: &[TyKind],
        ret: TyKind,
        frame: u8,
        flags: u32,
    ) -> u32 {
        self.seed_scalar_rows();
        let name_idx = self.strings.intern(name);
        let param_tys: Vec<u32> = params.iter().map(|k| self.intern_tykind(*k)).collect();
        let ret_idx = self.intern_tykind(ret);
        let sig_text = HostSig::canonical_text(&param_tys, &self.types, ret_idx);
        let sig_hash = HostSig::hash_text(&sig_text);
        let sig_str = self.strings.intern(&sig_text);
        // §7.8.3 item 4's `ret`/`ret.multi` match: a unit-shaped return pops nothing.
        let ret_arity = if matches!(ret, TyKind::Unit | TyKind::Void | TyKind::Never) {
            0
        } else {
            1
        };
        self.funcs.push(FuncEntry {
            code_off,
            code_len,
            locals,
            max_stack,
            nargs: params.len() as u32,
            ret_arity,
            ret_sig: ret.sig_code() as u32,
            name: name_idx,
            sig_str,
            sig_hash,
            flags,
            frame_kind: frame as u32,
        });
        (self.funcs.len() - 1) as u32
    }

    // ------------------------------------------------------------------ hash + encode

    /// The normalised module contents that `module_hash` is taken over: the seven regions after the
    /// header, in container order, **excluding** the header (and therefore excluding the hash field
    /// itself). `unit` and `source_name` are interned into the pool before encoding, so they are
    /// covered too.
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

    /// Full container bytes: the [`HEADER_LEN`]-byte header followed by the self-delimiting regions.
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

    /// D-38: no region wrapper. Each region opens with its own count (`code` with its byte length),
    /// exactly the walk `runtime/infy_ifbc.c::parse_sections` does.
    fn write_body_into(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.encode_types());
        out.extend_from_slice(&self.encode_imports());
        out.extend_from_slice(&self.encode_funcs());
        let mut code = self.code.clone();
        while code.len() % 4 != 0 {
            code.push(0);
        }
        put_u32(out, code.len() as u32);
        out.extend_from_slice(&code);
        out.extend_from_slice(&self.encode_lines());
        out.extend_from_slice(&self.encode_exceptions());
        out.extend_from_slice(&self.encode_pool());
    }

    /// `n u32` then `n × {name_str, kind, flags, size, align, nfields, payload_a, payload_b}` and
    /// that row's `nfields × {name_str, off, ty}` (D-38's merged shape).
    fn encode_types(&self) -> Vec<u8> {
        let mut b = Vec::new();
        put_u32(&mut b, self.types.len() as u32);
        for t in &self.types {
            put_u32(&mut b, t.name);
            put_u32(&mut b, t.kind);
            put_u32(&mut b, t.flags);
            put_u32(&mut b, t.size);
            put_u32(&mut b, t.align);
            put_u32(&mut b, t.fields.len() as u32);
            put_u32(&mut b, t.a);
            put_u32(&mut b, t.b);
            for f in &t.fields {
                put_u32(&mut b, f.name);
                put_u32(&mut b, f.off);
                put_u32(&mut b, f.ty);
            }
        }
        b
    }

    /// `n u32` then `n × {name_str u32, kind u8, nparams u8, ret_sig u8, flags u8, sig_hash u32,
    /// psigs: nparams × u8}` — the C row plus D-38's trailing signature digest.
    fn encode_imports(&self) -> Vec<u8> {
        let mut b = Vec::new();
        put_u32(&mut b, self.imports.len() as u32);
        for e in &self.imports {
            put_u32(&mut b, e.name);
            b.push(e.kind.as_u8());
            b.push(e.params.len() as u8);
            b.push(e.ret_sig);
            b.push(e.flags);
            put_u32(&mut b, e.sig_hash);
            for p in &e.params {
                b.push(*p);
            }
        }
        b
    }

    /// `n_funcs u32`, `max_mem_bytes u64`, then 48-byte records in the loader's field order.
    fn encode_funcs(&self) -> Vec<u8> {
        let mut b = Vec::new();
        put_u32(&mut b, self.funcs.len() as u32);
        put_u64(&mut b, self.max_mem_bytes);
        for f in &self.funcs {
            put_u32(&mut b, f.code_off);
            put_u32(&mut b, f.code_len);
            put_u32(&mut b, f.locals);
            put_u32(&mut b, f.max_stack);
            put_u32(&mut b, f.nargs);
            put_u32(&mut b, f.ret_arity);
            put_u32(&mut b, f.ret_sig);
            put_u32(&mut b, f.name);
            put_u32(&mut b, f.sig_str);
            put_u32(&mut b, f.sig_hash);
            put_u32(&mut b, f.flags);
            put_u32(&mut b, f.frame_kind);
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
        let mut b: Vec<u8> = Vec::new();
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

// ------------------------------------------------------------------------ host signatures

/// One host (INFY-native) entry point a B-track frame may reach with `call.host`, or the signature
/// of the embed unit itself (§7.7.1 rule 1: the *INFY-side* declared types fix the parameter table).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostSig {
    pub name: String,
    pub params: Vec<TyKind>,
    pub ret: TyKind,
    /// Element kind when `ret` is a container (`std.iter.range -> list` of `Int`). This is a
    /// *lowering hint*, not part of the signature digest: IFABI fixes the value representation, the
    /// element kind is what lets `seq.next` produce a statically typed item (§7.7.2).
    pub ret_elem: Option<TyKind>,
    pub ifabi: u16,
}

impl HostSig {
    pub fn new(name: &str, params: &[TyKind], ret: TyKind) -> HostSig {
        HostSig {
            name: name.to_string(),
            params: params.to_vec(),
            ret,
            ret_elem: None,
            ifabi: IFABI_V2,
        }
    }
    /// Same, with the container element kind.
    pub fn with_elem(mut self, elem: TyKind) -> HostSig {
        self.ret_elem = Some(elem);
        self
    }

    /// Canonical signature text `(i64, f64) -> dyn` — the byte form whose digest validation item 5
    /// compares (§7.8.3). The names are the **scalar codes** the `import_table` row encodes
    /// ([`TyKind::sig_code`]), not the static kinds: a container or instance *is* a boxed `dyn` on
    /// the IFABI wire, so the text and the digest describe the same bytes the loader checks.
    pub fn canonical_text(params: &[u32], types: &[TypeEntry], ret: u32) -> String {
        let name_of = |idx: u32| -> &'static str {
            types
                .get(idx as usize)
                .and_then(|e| TyKind::from_u32(e.kind))
                .map(|k| sig_name(k.sig_code()))
                .unwrap_or("?")
        };
        let ps: Vec<&str> = params.iter().map(|t| name_of(*t)).collect();
        format!("({}) -> {}", ps.join(", "), name_of(ret))
    }

    /// `sha256(canonical text)` truncated to a big-endian `u32`, compared byte-wise at load.
    pub fn hash_text(text: &str) -> u32 {
        let d = sha256::sha256(text.as_bytes());
        u32::from_be_bytes([d[0], d[1], d[2], d[3]])
    }

    /// Signature digest computed from the declared kinds (no module tables needed); the text is the
    /// scalar-code form [`HostSig::canonical_text`] produces, so an `import_table` row's digest and
    /// its `sig_text` render the same thing.
    pub fn sig_hash(&self) -> u32 {
        HostSig::hash_text(&self.text())
    }

    pub fn text(&self) -> String {
        let ps: Vec<&str> = self.params.iter().map(|k| sig_name(k.sig_code())).collect();
        format!("({}) -> {}", ps.join(", "), sig_name(self.ret.sig_code()))
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
        let dyn_ = Dyn;
        for (name, params, ret) in [
            ("std.io.log", vec![dyn_], Unit),
            ("std.io.warn", vec![dyn_], Unit),
            ("std.iter.len", vec![dyn_], Usize),
            ("std.iter.range", vec![Usize], List),
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
            ("std.embed.eq", vec![dyn_, dyn_], Bool),
            ("std.embed.ne", vec![dyn_, dyn_], Bool),
            ("std.math.truediv", vec![dyn_, dyn_], Float),
        ] {
            let sig = match name {
                "std.iter.range" => HostSig::new(name, &params, ret).with_elem(Int),
                _ => HostSig::new(name, &params, ret),
            };
            t.register(sig);
        }
        t
    }

    /// Add the entry-point signature of one embed unit; `lower` uses it as the parameter table
    /// (§7.7.1 rule 1) and `verify` checks `ret` against it.
    pub fn with_unit_sig(mut self, unit_name: &str, sig: HostSig) -> HostSigTable {
        self.entries.insert(unit_name.to_string(), sig);
        self
    }

    /// Validation item 5 for one `import_table` row: it must be a host row, the module's header
    /// IFABI must be the table's, the row must carry the IFABI-signed stamp, every signature code
    /// must be inside §2's closed set, and the digest must equal the table's for the same name
    /// (`IFBC.md` §5.5 — all of it is `R-0708`).
    pub fn check_import(&self, m: &IfbcModule, idx: usize) -> Result<(), String> {
        let e = m
            .imports
            .get(idx)
            .ok_or_else(|| format!("import_table 无第 {idx} 项（校验项 5 → R-0708）"))?;
        let name = m.strings.get(e.name).cloned().unwrap_or_default();
        if e.kind != ImportKind::Host {
            return Err(format!(
                "call.host 目标 `{name}` 不是 host 行（kind = {}）",
                e.kind.word()
            ));
        }
        // D-38: no per-row `ifabi` field survives the merge — the header's is the module's version.
        if m.ifabi != self.ifabi {
            return Err(format!("模块头 IFABI-{} 与工具链 IFABI-{} 不符", m.ifabi, self.ifabi));
        }
        if e.flags & IMP_SIGNED == 0 {
            return Err(format!("host `{name}` 的 import 行缺 IFABI-signed 标记（IFBC.md §5.5）"));
        }
        for c in e.params.iter().chain(std::iter::once(&e.ret_sig)) {
            if sig_kind(*c).is_none() {
                return Err(format!("host `{name}` 的签名码 {c} 不在 §2 的封闭集内（>17 不可表达）"));
            }
        }
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

/// Emits `opcode:u8` plus its `u32` operands big-endian (D.1) into one function's code, tracking the
/// offsets needed for back-patching jumps, the `line_map`, and the operand-stack high-water mark
/// that becomes `func_table.max_stack`.
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
    locals: u32,
    scratch_next: u32,
    /// Running operand-stack height, and the peak so far (the declared ceiling D-38's `func_table`
    /// record carries).
    height: i64,
    max_stack: u32,
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
            locals: 0,
            scratch_next: 0,
            height: 0,
            max_stack: 0,
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

    /// Declared operand-stack ceiling for the function built so far: the peak of the running
    /// height. `call.*` contribute their opcode-implied effect (no callee arity), so the number is
    /// a **conservative upper bound** — never below what the loader's own height pass measures.
    pub fn max_stack(&self) -> u32 {
        self.max_stack
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
        let off = self.offset();
        self.bytes.push(op.byte());
        for o in operands {
            put_u32(&mut self.bytes, *o);
        }
        self.note_height(op, operands);
        if let Some(l) = line {
            self.marks.push((off, l));
        }
        Ok(off)
    }

    /// `switch n, [targets]` (D.4, pinned by D-38 (h)): the case count and its inline target table
    /// are one instruction, so the linear pass stays single-and-forward.
    pub fn switch(&mut self, targets: &[u32], line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        if targets.len() > MAX_SWITCH_CASES as usize {
            return Err(IfbcEncodeErr::SwitchCaseCountTooLarge { given: targets.len() });
        }
        let off = self.offset();
        self.bytes.push(Op::Switch.byte());
        put_u32(&mut self.bytes, targets.len() as u32);
        for t in targets {
            put_u32(&mut self.bytes, *t);
        }
        self.note_height(Op::Switch, &[targets.len() as u32]);
        if let Some(l) = line {
            self.marks.push((off, l));
        }
        Ok(off)
    }

    fn note_height(&mut self, op: Op, operands: &[u32]) {
        // The container forms size their own pop count from an operand, exactly like `effect`.
        let (pops, pushes) = match op {
            Op::NewList | Op::NewTuple | Op::NewSet => (operands[operands.len() - 1] as usize, 1),
            Op::NewDict => (2 * operands[2] as usize, 1),
            Op::ClosureNew => (operands[1] as usize, 1),
            _ => op.stack_effect(),
        };
        self.height = self.height - pops as i64 + pushes as i64;
        if self.height < 0 {
            // Unbalanced emission: the verifier reports it, the ceiling only ever needs the peak.
            self.height = 0;
        }
        if self.height > self.max_stack as i64 {
            self.max_stack = self.height as u32;
        }
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
    /// `push k,ty` — D-38 (d): a pool constant must carry its type, IFBC has no implicit literals.
    pub fn push_pool(&mut self, pool: u32, ty: u32, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::Push, &[pool, ty], line)
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
    /// D.3: mandatory immediately before `div`/`rem`. D-38 (c): **no operands** — the type travels
    /// on the guarded instruction's own operand 0, and D.3's adjacency check only inspects bytes.
    pub fn guard_nonzero(&mut self, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::GuardNonzero, &[], line)
    }
    /// D.3: mandatory immediately before `shl`/`shr`/`shr.a`; 0 operands for the same reason.
    pub fn guard_shift(&mut self, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::GuardShift, &[], line)
    }
    /// D.7 / UB-020: mandatory immediately before every `unbox.dyn`.
    pub fn is_dyn(&mut self, ty: u32, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::IsDyn, &[ty], line)
    }
    pub fn unbox_dyn(&mut self, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::UnboxDyn, &[], line)
    }
    /// D.7 / D-38 (f): `box.dyn` is the identity — a B-track value already *is* an `InfyDyn` — so it
    /// takes no type operand.
    pub fn box_dyn(&mut self, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::BoxDyn, &[], line)
    }
    /// `ldw i,ty` — D-38 (e): the annotation is operand 1 and must be `i64`/`u64`/`f64`.
    pub fn ldw(&mut self, slot: u32, ty: u32, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::Ldw, &[slot, ty], line)
    }
    /// `field_off ty,field` — D-38 (g).
    pub fn field_off(&mut self, ty: u32, field: u32, line: Option<u32>) -> Result<u32, IfbcEncodeErr> {
        self.emit(Op::FieldOff, &[ty, field], line)
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
}

// ------------------------------------------------------------------------ decoding

/// Cursor over a big-endian image. Nothing here trusts the bytes: every read is bounds-checked and
/// yields a [`IfbcDecodeErr`] rather than panicking, because a malformed `.infib` must produce a
/// diagnostic (§7.8.3, UB-019).
struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Reader<'a> {
        Reader { b, p: 0 }
    }
    fn rest(&self) -> usize {
        self.b.len() - self.p
    }
    fn take(&mut self, n: usize, region: &'static str) -> Result<&'a [u8], IfbcDecodeErr> {
        if self.rest() < n {
            return Err(IfbcDecodeErr::RegionTruncated { region, need: n, got: self.rest() });
        }
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn u8(&mut self, region: &'static str) -> Result<u8, IfbcDecodeErr> {
        Ok(self.take(1, region)?[0])
    }
    fn u16(&mut self, region: &'static str) -> Result<u16, IfbcDecodeErr> {
        Ok(be_u16(self.take(2, region)?).unwrap_or(0))
    }
    fn u32(&mut self, region: &'static str) -> Result<u32, IfbcDecodeErr> {
        Ok(be_u32(self.take(4, region)?).unwrap_or(0))
    }
    fn hash(&mut self, region: &'static str) -> Result<[u8; 32], IfbcDecodeErr> {
        let s = self.take(32, region)?;
        let mut h = [0u8; 32];
        h.copy_from_slice(s);
        Ok(h)
    }
}

/// Decode a container image: the structural half of §7.8.3 (regions, indices, arity, linearity of
/// the code stream). The semantic half — stack heights, host signatures — is [`IfbcModule::verify`].
pub fn decode(bytes: &[u8]) -> Result<IfbcModule, IfbcDecodeErr> {
    if bytes.len() < HEADER_LEN {
        return Err(IfbcDecodeErr::TooShort { need: HEADER_LEN, got: bytes.len() });
    }
    if &bytes[0..4] != MAGIC {
        let mut m = [0u8; 4];
        m.copy_from_slice(&bytes[0..4]);
        return Err(IfbcDecodeErr::BadMagic(m));
    }
    let mut r = Reader::new(bytes);
    r.take(4, REGIONS[0])?;
    let ifbc_version = r.u16(REGIONS[0])?;
    if ifbc_version != IFBC_VERSION_V1 {
        return Err(IfbcDecodeErr::UnsupportedIfbc(ifbc_version));
    }
    let ifabi = r.u16(REGIONS[0])?;
    if ifabi != IFABI_V2 {
        return Err(IfbcDecodeErr::UnsupportedIfabi(ifabi));
    }
    let tag = r.u16(REGIONS[0])?;
    if lang_tag_name(tag).is_none() {
        return Err(IfbcDecodeErr::UnknownLangTag(tag));
    }
    let module_hash = r.hash(REGIONS[0])?;
    let func_count = r.u32(REGIONS[0])?;
    let frame_kind = r.u8(REGIONS[0])?;
    // §7.8.1 defines exactly one module-wide value model (`0`); the coroutine flag is per function
    // in func_table (appendix D.8), so accepting 1 here would let a module claim a value model the
    // C loader rejects. C: `infy_ifbc.c` returns R-0703 for any header frame_kind != 0.
    if frame_kind != FRAME_KIND_STACK {
        return Err(IfbcDecodeErr::BadFrameKind(frame_kind));
    }

    let mut m = IfbcModule {
        ifbc_version,
        ifabi,
        lang: LangTag { tag, cap_version: 0 },
        frame_kind,
        module_hash,
        ..IfbcModule::default()
    };
    m.types = decode_types(&mut r)?;
    m.imports = decode_imports(&mut r)?;
    let (funcs, max_mem_bytes) = decode_funcs(&mut r, func_count)?;
    m.funcs = funcs;
    m.max_mem_bytes = max_mem_bytes;
    m.code = decode_code(&mut r, &m)?;
    m.lines = decode_lines(&mut r)?;
    m.exceptions = decode_exceptions(&mut r, &m)?;
    let entries = decode_pool(&mut r)?;
    m.strings = StringPool::from_entries(entries);
    if let Some(u) = m.strings.get(0) {
        m.unit = u.clone();
    }
    if let Some(s) = m.strings.get(1) {
        m.source_name = s.clone();
    }
    if r.rest() != 0 {
        return Err(IfbcDecodeErr::TrailingBytes(r.rest()));
    }
    validate_refs(&m)?;
    // §7.3: the image must hash to exactly what its own contents say.
    let computed = m.compute_module_hash();
    if computed != m.module_hash {
        return Err(IfbcDecodeErr::HashMismatch { stored: m.module_hash, computed });
    }
    Ok(m)
}

/// Cross-table reference checks that only become possible once the pool is decoded: every `name`,
/// signature, type, import, func and slot index must be inside its table (§7.8.3 items 1, 3 and 5).
fn validate_refs(m: &IfbcModule) -> Result<(), IfbcDecodeErr> {
    let plen = m.strings.len() as u32;
    let tlen = m.types.len() as u32;
    let ilen = m.imports.len() as u32;
    let flen = m.funcs.len() as u32;
    let in_bounds = |v: u32, len: u32| v == NONE32 || v < len;
    for t in &m.types {
        if !in_bounds(t.name, plen) {
            return Err(IfbcDecodeErr::PoolIndexOutOfRange { pool: t.name, len: plen });
        }
        for k in [t.a, t.b] {
            if !in_bounds(k, tlen) {
                return Err(IfbcDecodeErr::TypeIndexOutOfRange { ty: k, len: tlen });
            }
        }
        for f in &t.fields {
            if !in_bounds(f.name, plen) {
                return Err(IfbcDecodeErr::PoolIndexOutOfRange { pool: f.name, len: plen });
            }
            if !in_bounds(f.ty, tlen) {
                return Err(IfbcDecodeErr::TypeIndexOutOfRange { ty: f.ty, len: tlen });
            }
        }
    }
    for e in &m.imports {
        if !in_bounds(e.name, plen) {
            return Err(IfbcDecodeErr::PoolIndexOutOfRange { pool: e.name, len: plen });
        }
        // D-38: a signature byte addresses the scalar block, so its range is §2's closed set
        // (`> 17` is not expressible in IFABI), not the type-table length.
        if sig_kind(e.ret_sig).is_none() {
            return Err(IfbcDecodeErr::TypeIndexOutOfRange { ty: e.ret_sig as u32, len: 18 });
        }
        for p in &e.params {
            if sig_kind(*p).is_none() {
                return Err(IfbcDecodeErr::TypeIndexOutOfRange { ty: *p as u32, len: 18 });
            }
        }
    }
    for f in &m.funcs {
        if !in_bounds(f.name, plen) {
            return Err(IfbcDecodeErr::PoolIndexOutOfRange { pool: f.name, len: plen });
        }
        if !in_bounds(f.sig_str, plen) {
            return Err(IfbcDecodeErr::PoolIndexOutOfRange { pool: f.sig_str, len: plen });
        }
        if sig_kind(f.ret_sig as u8).is_none() {
            return Err(IfbcDecodeErr::TypeIndexOutOfRange { ty: f.ret_sig, len: 18 });
        }
    }
    for (i, f) in m.funcs.iter().enumerate() {
        for (off, ins) in instructions_of(m, i)? {
            for (k, kind) in ins.op.operand_kinds().iter().enumerate() {
                let v = ins.ops[k];
                let bad = |e: &str| match e {
                    "pool" => IfbcDecodeErr::PoolIndexOutOfRange { pool: v, len: plen },
                    "type" => IfbcDecodeErr::TypeIndexOutOfRange { ty: v, len: tlen },
                    "import" => IfbcDecodeErr::ImportIndexOutOfRange { imp: v, len: ilen },
                    "func" => IfbcDecodeErr::FuncIndexOutOfRange { func: v, len: flen },
                    _ => IfbcDecodeErr::InstructionRunsPastFunc { op: ins.op.base(), off },
                };
                match kind {
                    OperandKind::Pool if v != NONE32 && v >= plen => return Err(bad("pool")),
                    OperandKind::Type if v != NONE32 && v >= tlen => return Err(bad("type")),
                    OperandKind::Import if v >= ilen => return Err(bad("import")),
                    OperandKind::Func if v >= flen => return Err(bad("func")),
                    OperandKind::Slot if v >= f.locals => return Err(bad("slot")),
                    OperandKind::CodeOff => {
                        if v as usize > m.code.len() {
                            return Err(IfbcDecodeErr::CodeRangeOutOfRange {
                                func: i as u32,
                                off,
                                len: v,
                                code_len: m.code.len() as u32,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

/// D-38's merged row: the six `u32` the loader reads, then `payload_a`/`payload_b`, then the
/// `nfields × {name_str, off, ty}` triples. Self-delimiting: the leading count is the region's.
fn decode_types(r: &mut Reader) -> Result<Vec<TypeEntry>, IfbcDecodeErr> {
    let region = REGIONS[1];
    let n = r.u32(region)?;
    if n > MAX_TABLE_ROWS {
        return Err(IfbcDecodeErr::RegionCount { region, max: n });
    }
    let mut out = Vec::new();
    for _ in 0..n {
        let name = r.u32(region)?;
        let kind = r.u32(region)?;
        if TyKind::from_u32(kind).is_none() {
            return Err(IfbcDecodeErr::TypeIndexOutOfRange { ty: kind, len: 33 });
        }
        let flags = r.u32(region)?;
        let size = r.u32(region)?;
        let align = r.u32(region)?;
        let nfields = r.u32(region)?;
        let a = r.u32(region)?;
        let b = r.u32(region)?;
        if nfields > MAX_TABLE_ROWS {
            return Err(IfbcDecodeErr::RegionCount { region, max: nfields });
        }
        let mut fields = Vec::new();
        for _ in 0..nfields {
            let fname = r.u32(region)?;
            let off = r.u32(region)?;
            let fty = r.u32(region)?;
            fields.push(TypeField { name: fname, off, ty: fty });
        }
        out.push(TypeEntry { kind, name, flags, size, align, fields, a, b });
    }
    Ok(out)
}

/// D-38's merged row: `name_str u32, kind u8, nparams u8, ret_sig u8, flags u8, sig_hash u32`,
/// then `nparams` signature-code bytes. No per-row `ifabi` — the header owns the version.
fn decode_imports(r: &mut Reader) -> Result<Vec<ImportEntry>, IfbcDecodeErr> {
    let region = REGIONS[2];
    let n = r.u32(region)?;
    if n > MAX_TABLE_ROWS {
        return Err(IfbcDecodeErr::RegionCount { region, max: n });
    }
    let mut out = Vec::new();
    for _ in 0..n {
        let name = r.u32(region)?;
        let kind = ImportKind::from_u8(r.u8(region)?).ok_or(IfbcDecodeErr::RegionTruncated {
            region,
            need: 1,
            got: 0,
        })?;
        let nparams = r.u8(region)?;
        let ret_sig = r.u8(region)?;
        let flags = r.u8(region)?;
        let sig_hash = r.u32(region)?;
        let mut params = Vec::with_capacity(nparams as usize);
        for _ in 0..nparams {
            params.push(r.u8(region)?);
        }
        out.push(ImportEntry { name, kind, flags, ret_sig, params, sig_hash });
    }
    Ok(out)
}

/// `n_funcs u32`, `max_mem_bytes u64`, then the 48-byte records (D-38's merge of C's ten fields and
/// this side's signature text/digest). Returns the rows plus the §7.9 unit budget.
fn decode_funcs(r: &mut Reader, func_count: u32) -> Result<(Vec<FuncEntry>, u64), IfbcDecodeErr> {
    let region = REGIONS[3];
    let n = r.u32(region)?;
    if n != func_count {
        return Err(IfbcDecodeErr::FuncCountMismatch { header: func_count, table: n });
    }
    if n > MAX_TABLE_ROWS {
        return Err(IfbcDecodeErr::RegionCount { region, max: n });
    }
    let max_mem_bytes = {
        let bytes = r.take(8, region)?;
        be_u64(bytes).unwrap_or(0)
    };
    let mut out = Vec::new();
    for _ in 0..n {
        let code_off = r.u32(region)?;
        let code_len = r.u32(region)?;
        let locals = r.u32(region)?;
        let max_stack = r.u32(region)?;
        let nargs = r.u32(region)?;
        let ret_arity = r.u32(region)?;
        let ret_sig = r.u32(region)?;
        let name = r.u32(region)?;
        let sig_str = r.u32(region)?;
        let sig_hash = r.u32(region)?;
        let flags = r.u32(region)?;
        let frame_kind = r.u32(region)?;
        // D.8 names the values "0" and "coroutine" without a numeric table; this implementation
        // fixes 0 = stack + local window / 1 = coroutine and rejects anything else (deviation
        // D-23). The C loader applies the same range check.
        if frame_kind > FRAME_KIND_COROUTINE as u32 {
            return Err(IfbcDecodeErr::BadFrameKind(frame_kind as u8));
        }
        out.push(FuncEntry {
            code_off,
            code_len,
            locals,
            max_stack,
            nargs,
            ret_arity,
            ret_sig,
            name,
            sig_str,
            sig_hash,
            flags,
            frame_kind,
        });
    }
    Ok((out, max_mem_bytes))
}

fn decode_code(r: &mut Reader, m: &IfbcModule) -> Result<Vec<u8>, IfbcDecodeErr> {
    let region = REGIONS[4];
    let len = r.u32(region)? as usize;
    let body = r.take(len, region)?.to_vec();
    if body.len() % 4 != 0 {
        return Err(IfbcDecodeErr::MisalignedRegion { region, len: body.len() as u32 });
    }
    for (i, f) in m.funcs.iter().enumerate() {
        let end = (f.code_off as usize) + (f.code_len as usize);
        if (f.code_off as usize) % 4 != 0 {
            return Err(IfbcDecodeErr::MisalignedFunc { func: i as u32, off: f.code_off });
        }
        if end > body.len() {
            return Err(IfbcDecodeErr::CodeRangeOutOfRange {
                func: i as u32,
                off: f.code_off,
                len: f.code_len,
                code_len: body.len() as u32,
            });
        }
    }
    Ok(body)
}

fn decode_lines(r: &mut Reader) -> Result<Vec<LineEntry>, IfbcDecodeErr> {
    let region = REGIONS[5];
    let n = r.u32(region)?;
    if n > MAX_LONG_ROWS {
        return Err(IfbcDecodeErr::RegionCount { region, max: n });
    }
    let mut out = Vec::new();
    let mut last = 0u32;
    for i in 0..n {
        let code_off = r.u32(region)?;
        let line = r.u32(region)?;
        if i > 0 && code_off < last {
            return Err(IfbcDecodeErr::LineMapUnsorted { at: code_off });
        }
        last = code_off;
        out.push(LineEntry { code_off, line });
    }
    Ok(out)
}

fn decode_exceptions(r: &mut Reader, m: &IfbcModule) -> Result<Vec<ExcEntry>, IfbcDecodeErr> {
    let region = REGIONS[6];
    let n = r.u32(region)?;
    if n > MAX_LONG_ROWS {
        return Err(IfbcDecodeErr::RegionCount { region, max: n });
    }
    let mut out = Vec::new();
    for i in 0..n {
        let start = r.u32(region)?;
        let end = r.u32(region)?;
        let handler = r.u32(region)?;
        let depth = r.u16(region)?;
        let kind = ExcKind::from_u16(r.u16(region)?).ok_or(IfbcDecodeErr::ExcRangeInvalid {
            at: i as usize,
        })?;
        if end < start || (end as usize) > m.code.len() || (handler as usize) > m.code.len() {
            return Err(IfbcDecodeErr::ExcRangeInvalid { at: i as usize });
        }
        out.push(ExcEntry { start, end, handler, depth, kind });
    }
    Ok(out)
}

fn decode_pool(r: &mut Reader) -> Result<Vec<String>, IfbcDecodeErr> {
    let region = REGIONS[7];
    let n = r.u32(region)?;
    if n > MAX_LONG_ROWS {
        return Err(IfbcDecodeErr::RegionCount { region, max: n });
    }
    let mut out = Vec::new();
    for i in 0..n {
        let el = r.u32(region)? as usize;
        let raw = r.take(el, region)?;
        let s = std::str::from_utf8(raw)
            .map_err(|_| IfbcDecodeErr::BadUtf8InPool { pool: i })?
            .to_string();
        out.push(s);
    }
    Ok(out)
}

/// All decoded instructions of one function, in code order, each with its absolute code offset.
pub fn instructions_of(m: &IfbcModule, func: usize) -> Result<Vec<(u32, Instr)>, IfbcDecodeErr> {
    let f = m
        .funcs
        .get(func)
        .ok_or(IfbcDecodeErr::FuncIndexOutOfRange { func: func as u32, len: m.funcs.len() as u32 })?;
    let end = (f.code_off as usize) + (f.code_len as usize);
    if end > m.code.len() {
        return Err(IfbcDecodeErr::CodeRangeOutOfRange {
            func: func as u32,
            off: f.code_off,
            len: f.code_len,
            code_len: m.code.len() as u32,
        });
    }
    let mut out = Vec::new();
    let mut p = f.code_off as usize;
    while p < end {
        let byte = m.code[p];
        let op = Op::from_byte(byte).ok_or(IfbcDecodeErr::UnknownOpcode { byte, off: p as u32 })?;
        let fixed = 1 + 4 * op.arity();
        if p + fixed > end {
            return Err(IfbcDecodeErr::InstructionRunsPastFunc { op: op.base(), off: p as u32 });
        }
        // D-38 (h): `switch n` is the only opcode whose length depends on an operand value. The
        // count is capped exactly like the loader's `insn_len`, so a hostile table cannot make this
        // walk run away, and the inline targets are consumed with the instruction (never mistaken
        // for executable code by the linear pass).
        let mut extra = 0u32;
        let mut width = fixed;
        if op.is_variable_width() {
            let n = be_u32(&m.code[p + 1..p + 5]).unwrap_or(0);
            if n > MAX_SWITCH_CASES {
                return Err(IfbcDecodeErr::SwitchCasesTooLarge { off: p as u32, n });
            }
            extra = n;
            width = fixed + 4 * n as usize;
        }
        if p + width > end {
            return Err(IfbcDecodeErr::InstructionRunsPastFunc { op: op.base(), off: p as u32 });
        }
        let mut instr = Instr::new(op);
        instr.extra = extra;
        for i in 0..op.arity() {
            let q = p + 1 + i * 4;
            instr.ops[i] = be_u32(&m.code[q..q + 4]).unwrap_or(0);
        }
        out.push((p as u32, instr));
        p += width;
    }
    Ok(out)
}

/// The inline case table of the `switch` at `off` (empty for every other opcode). The targets live
/// inside the instruction, so they are read from the code region rather than from `Instr::ops`.
pub fn switch_targets(m: &IfbcModule, off: u32, ins: &Instr) -> Vec<u32> {
    if ins.op != Op::Switch || ins.extra == 0 {
        return Vec::new();
    }
    let base = off as usize + 1 + 4;
    let end = base + 4 * ins.extra as usize;
    if end > m.code.len() {
        return Vec::new();
    }
    (0..ins.extra as usize)
        .filter_map(|k| be_u32(&m.code[base + 4 * k..base + 4 * k + 4]))
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Instr {
    pub op: Op,
    pub ops: [u32; MAX_OPS],
    /// Inline operand words that follow the opcode-implied ones — only `switch` uses this
    /// (the case count), see [`switch_targets`].
    pub extra: u32,
}

impl Instr {
    pub fn new(op: Op) -> Instr {
        Instr { op, ops: [0; MAX_OPS], extra: 0 }
    }
    pub fn with(op: Op, a: u32) -> Instr {
        Instr { op, ops: [a, 0, 0], extra: 0 }
    }
    pub fn with2(op: Op, a: u32, b: u32) -> Instr {
        Instr { op, ops: [a, b, 0], extra: 0 }
    }
    pub fn operands(&self) -> &[u32] {
        &self.ops[..self.op.arity()]
    }
    /// Encoded width in bytes: `1 + 4 * (arity + extra)` (D.1, plus D-38 (h) for `switch`).
    pub fn width(&self) -> usize {
        1 + 4 * (self.op.arity() + self.extra as usize)
    }
    /// Render `add.i64 3` given the module's tables.
    pub fn render(&self, m: &IfbcModule) -> String {
        let mut s = String::new();
        if self.op.is_typed_family() {
            // D.3: the type suffix *is* operand 0, shown inline (`add.i64`).
            let t = m.ty_kind(self.ops[0]).map(|k| k.suffix()).unwrap_or("?");
            s.push_str(self.op.base());
            s.push('.');
            s.push_str(t);
        } else {
            // D.7 writes `is.dyn T` / `cast.dyn T` with the type as an ordinary operand, so those
            // render as `is.dyn i64`; only D.3's families get a dotted suffix.
            s.push_str(self.op.base());
        }
        let kinds = self.op.operand_kinds();
        for (i, o) in self.operands().iter().enumerate() {
            s.push(' ');
            let txt = match kinds.get(i) {
                Some(OperandKind::Pool) => {
                    format!("{:?}@{o}", m.strings.get(*o).cloned().unwrap_or_default())
                }
                Some(OperandKind::Type) => ty_text(&m.types, &m.strings, *o),
                Some(OperandKind::Func) => format!(
                    "#{o} {:?}",
                    m.funcs
                        .get(*o as usize)
                        .and_then(|f| m.strings.get(f.name))
                        .cloned()
                        .unwrap_or_default()
                ),
                Some(OperandKind::Import) => format!(
                    "#{o} {:?}",
                    m.imports
                        .get(*o as usize)
                        .and_then(|e| m.strings.get(e.name))
                        .cloned()
                        .unwrap_or_default()
                ),
                Some(OperandKind::CodeOff) => format!("@{o:#06x}"),
                _ => format!("{o}"),
            };
            s.push_str(&txt);
        }
        // The `switch` case table lives inside the instruction and needs its code offset to read,
        // so it is printed by the disassembler (`[0x…, 0x…]` after the count), not here.
        s
    }
}

// ------------------------------------------------------------------------ disassembly

/// Byte-stable text rendering of a container image — the body of `infy ifbc dump` (§7.8.4, and the
/// golden-file target of §12.3). No table iteration order is left to a hash map, so two runs over
/// the same bytes print the same bytes.
pub fn disassemble(bytes: &[u8]) -> String {
    match decode(bytes) {
        Ok(m) => m.disassemble(),
        Err(e) => format!("IFBC 镜像无法解码: {e}\n"),
    }
}

impl IfbcModule {
    /// See [`disassemble`]; this is the same renderer on an already-decoded (or just-encoded)
    /// module, so callers can print a module they assembled without a round trip.
    pub fn disassemble(&self) -> String {
        let mut o = String::new();
        let tag = self.lang.name().unwrap_or("UNKNOWN");
        o.push_str(&format!(
            "IFBC module  magic={}  ifbc_version={}  ifabi={}  lang_tag={}({:#06x})  cap={}  \
             frame_kind={}  funcs={}  max_mem_bytes={}  image_len={}\n",
            std::str::from_utf8(MAGIC).unwrap_or("?"),
            self.ifbc_version,
            self.ifabi,
            tag,
            self.lang.tag,
            self.lang.cap_version,
            self.frame_kind,
            self.funcs.len(),
            self.max_mem_bytes,
            self.encode().len()
        ));
        o.push_str(&format!("unit          = {:?}\n", self.unit));
        o.push_str(&format!("source_name   = {:?}\n", self.source_name));
        o.push_str(&format!(
            "module_hash   = sha256:{}\n",
            hex32(&self.module_hash)
        ));
        o.push_str(&format!(
            "regions       = {}  (各区自定界, D-38)\n",
            REGIONS.join(" | ")
        ));

        o.push_str(&format!("\ntype_table ({}):\n", self.types.len()));
        for (i, t) in self.types.iter().enumerate() {
            let k = TyKind::from_u32(t.kind).map(|k| k.suffix()).unwrap_or("?");
            let name = self.strings.get(t.name).cloned().unwrap_or_default();
            let kids: Vec<String> = [t.a, t.b]
                .iter()
                .filter(|x| **x != NONE32)
                .map(|x| format!("ty#{x}"))
                .collect();
            o.push_str(&format!(
                "  #{i:<3} {k:<8} name={name:<10} kind={:<4} size={} align={} repr_c={} \
                 nfields={} {}\n",
                t.kind,
                t.size,
                t.align,
                (t.flags & TF_REPR_C) != 0,
                t.fields.len(),
                kids.join(" ")
            ));
            if let Some(dt) = TyKind::from_u32(t.kind).and_then(|k| k.dyn_tag()) {
                o.push_str(&format!("      InfyDynTag={dt}\n"));
            }
            for (j, fld) in t.fields.iter().enumerate() {
                let fname = self.strings.get(fld.name).cloned().unwrap_or_default();
                o.push_str(&format!("      field #{j} {fname:?} off={} ty=#{}\n", fld.off, fld.ty));
            }
        }

        o.push_str(&format!("\nimport_table ({}):\n", self.imports.len()));
        for (i, e) in self.imports.iter().enumerate() {
            o.push_str(&format!(
                "  #{i:<3} {:<6} signed={} {:?}  {}  sig_hash={:#010x}\n",
                e.kind.word(),
                (e.flags & IMP_SIGNED) != 0,
                self.strings.get(e.name).cloned().unwrap_or_default(),
                e.sig_text(),
                e.sig_hash
            ));
        }

        for (i, f) in self.funcs.iter().enumerate() {
            let frame = if f.frame_kind == FRAME_KIND_COROUTINE as u32 { "coroutine" } else { "stack" };
            o.push_str(&format!(
                "\nfunc #{i} {:?}  code=[{:#06x},{:#06x})  locals={}  max_stack={}  nargs={}  \
                 ret_arity={}  ret_sig={}  frame={frame}  flags={:#04x}  sig={:?}  \
                 sig_hash={:#010x}\n",
                self.strings.get(f.name).cloned().unwrap_or_default(),
                f.code_off,
                f.code_off + f.code_len,
                f.locals,
                f.max_stack,
                f.nargs,
                f.ret_arity,
                sig_name(f.ret_sig as u8),
                f.flags,
                self.strings.get(f.sig_str).cloned().unwrap_or_default(),
                f.sig_hash
            ));
            let per = self.line_per_code_off();
            match instructions_of(self, i) {
                Ok(instrs) => {
                    for (off, ins) in instrs {
                        let line = per
                            .iter()
                            .rev()
                            .find(|(o, _)| *o <= off)
                            .map(|(_, l)| *l)
                            .unwrap_or(0);
                        let text = match switch_targets(self, off, &ins).is_empty() {
                            false => {
                                let t: Vec<String> = switch_targets(self, off, &ins)
                                    .iter()
                                    .map(|x| format!("{x:#06x}"))
                                    .collect();
                                format!("{} [{}]", ins.render(self), t.join(", "))
                            }
                            true => ins.render(self),
                        };
                        o.push_str(&format!(
                            "  {off:#06x}  {:02x}  {:<22} ; line {line}\n",
                            ins.op.byte(),
                            text,
                        ));
                    }
                }
                Err(e) => o.push_str(&format!("  ; 指令流解码失败: {e}\n")),
            }
        }

        o.push_str(&format!("\nline_map ({}): code_off -> 原始语言行\n", self.lines.len()));
        for l in &self.lines {
            o.push_str(&format!("  {:#06x} -> line {}\n", l.code_off, l.line));
        }
        o.push_str(&format!("\nexception_table ({}):\n", self.exceptions.len()));
        for e in &self.exceptions {
            o.push_str(&format!(
                "  [{:#06x},{:#06x}) -> handler {:#06x}  depth={}  kind={}({})\n",
                e.start,
                e.end,
                e.handler,
                e.depth,
                e.kind.as_u16(),
                e.kind.word()
            ));
        }
        o.push_str(&format!("\nstring_pool ({}):\n", self.strings.len()));
        for (i, s) in self.strings.entries().iter().enumerate() {
            o.push_str(&format!("  #{i:<4} {s:?}\n"));
        }
        o.push_str("\n; appendix-D grouping decisions (docs/internals/ifbc-encoding-rust.md):\n");
        for (row, split) in GROUPING.iter() {
            o.push_str(&format!("; {row}: {split}\n"));
        }
        o
    }

    /// See [`dump_canonical`]: the canonical text of a module this side assembled or decoded.
    /// Fails only where the instruction stream is not linearly decodable, which is the same reason
    /// `infy_ifbc.c`'s canonical dumper stops walking — an image either side refuses has no shared
    /// text to compare, and that is reported rather than rendered as a partial match.
    pub fn dump_canonical(&self) -> Result<String, IfbcDecodeErr> {
        let mut o = String::new();
        o.push_str(&format!(
            "H {} {} {} {} {} {} {} {} {} {} {} {}\n",
            self.ifbc_version,
            self.ifabi,
            self.lang.tag,
            self.frame_kind,
            self.funcs.len(),
            self.types.len(),
            self.imports.len(),
            self.lines.len(),
            self.exceptions.len(),
            self.strings.len(),
            self.code.len(),
            self.max_mem_bytes
        ));
        for (i, t) in self.types.iter().enumerate() {
            o.push_str(&format!(
                "T {} {} {} {} {} {} {}\n",
                i,
                t.kind,
                t.name,
                t.flags,
                t.size,
                t.align,
                t.fields.len()
            ));
        }
        for (i, e) in self.imports.iter().enumerate() {
            o.push_str(&format!(
                "P {} {} {} {} {} {} {}",
                i,
                e.kind as u32,
                e.flags as u32,
                e.name,
                e.ret_sig as u32,
                e.params.len(),
                e.sig_hash
            ));
            for p in &e.params {
                o.push_str(&format!(" {p}"));
            }
            o.push('\n');
        }
        for (i, f) in self.funcs.iter().enumerate() {
            o.push_str(&format!(
                "F {} {} {} {} {} {} {} {} {} {} {} {} {}\n",
                i,
                f.name,
                f.code_off,
                f.code_len,
                f.locals,
                f.max_stack,
                f.nargs,
                f.ret_arity,
                f.ret_sig,
                f.frame_kind,
                f.flags,
                f.sig_str,
                f.sig_hash
            ));
        }
        for i in 0..self.funcs.len() {
            for (off, ins) in instructions_of(self, i)? {
                o.push_str(&canonical_insn(i, off, &ins, self));
                o.push('\n');
            }
        }
        for (i, l) in self.lines.iter().enumerate() {
            o.push_str(&format!("L {} {} {}\n", i, l.code_off, l.line));
        }
        for (i, e) in self.exceptions.iter().enumerate() {
            o.push_str(&format!(
                "X {} {} {} {} {} {}\n",
                i,
                e.start,
                e.end,
                e.handler,
                e.depth,
                e.kind.as_u16()
            ));
        }
        for (i, s) in self.strings.entries().iter().enumerate() {
            o.push_str(&format!("S {} {} ", i, s.len()));
            for b in s.as_bytes() {
                o.push_str(&format!("{b:02x}"));
            }
            o.push('\n');
        }
        Ok(o)
    }

    /// `line_map` as pairs, for the disassembler and for the line round-trip tests.
    pub fn line_per_code_off(&self) -> Vec<(u32, u32)> {
        self.lines.iter().map(|l| (l.code_off, l.line)).collect()
    }

    /// The original-language line an instruction offset came from (§7.4). `None` when the code
    /// region has no mark at or before `off` (which is `W-0705`, not an error).
    pub fn line_of_code_off(&self, off: u32) -> Option<u32> {
        let mut best = None;
        for l in &self.lines {
            if l.code_off <= off {
                best = Some(l.line);
            } else {
                break;
            }
        }
        best
    }

    /// The exception-table entries protecting a code offset.
    pub fn handlers_for(&self, off: u32) -> Vec<&ExcEntry> {
        self.exceptions
            .iter()
            .filter(|e| e.start <= off && off < e.end)
            .collect()
    }
}

// --------------------------------------------------- D4(3): canonical, machine-comparable render

/// The line grammar of the cross-language lock (deviation D-42, `runtime/IFBC.md` §6): one line per
/// header, table row, instruction, line mark, exception row and pool entry — every field positional
/// and decimal, and nothing resolved beyond a D.3 family's kind suffix, which both sides read out of
/// `type_table`. `runtime/infy_ifbc.c`'s `infy_ifbc_dump_canonical` produces the same text from the
/// same bytes; the two texts are compared as committed fixtures (`runtime/fixtures/*.can.txt`), so
/// "both sides read a container identically" is a diff, not a claim.
pub fn dump_canonical(bytes: &[u8]) -> Result<String, IfbcDecodeErr> {
    decode(bytes)?.dump_canonical()
}

/// D4(3)'s other half: one image's §7.8.3 conclusions as sorted `V <code> f=<func> off=<off>` lines
/// — the mirror of `infy_ifbc_verify_canonical`. Only blocking findings are rendered: §7.8.3 item 6's
/// `W-0705` fires per instruction here and once per function there, so it is not comparable text
/// (recorded in D-42 rather than papered over).
pub fn verify_canonical(m: &IfbcModule, hosts: &HostSigTable) -> String {
    let mut rows: Vec<(String, u32, u32)> = verify(m, hosts, true)
        .into_iter()
        .filter(|v| !v.code.starts_with('W'))
        .map(|v| (v.code.to_string(), v.func, v.off))
        .collect();
    rows.sort();
    let mut o = String::new();
    for (code, f, off) in rows {
        o.push_str(&format!("V {code} f={f} off={off}\n"));
    }
    o
}

/// `I`-line rendering of one instruction: `<func> <off> <opcode byte> <mnemonic> <arity>
/// <operands…> <switch case targets…>`.
fn canonical_insn(f: usize, off: u32, ins: &Instr, m: &IfbcModule) -> String {
    let mn = if ins.op.is_typed_family() {
        format!(
            "{}.{}",
            ins.op.base(),
            m.ty_kind(ins.ops[0]).map(|k| k.suffix()).unwrap_or("?")
        )
    } else {
        ins.op.base().to_string()
    };
    let mut o = format!("I {f} {off} {} {mn} {}", ins.op.byte(), ins.op.arity());
    for x in ins.operands() {
        o.push_str(&format!(" {x}"));
    }
    for t in switch_targets(m, off, ins) {
        o.push_str(&format!(" {t}"));
    }
    o
}

fn hex32(h: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in h {
        s.push(std::char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
        s.push(std::char::from_digit((b & 0x0f) as u32, 16).unwrap_or('0'));
    }
    s
}

// ------------------------------------------------------------------------ mnemonics

/// Every opcode defined by appendix D, in byte order (the order the tables are walked, so
/// `parse_mnemonic` is deterministic).
pub fn all_ops() -> Vec<Op> {
    (0u32..=255).filter_map(|b| Op::from_byte(b as u8)).collect()
}

impl TyKind {
    /// Inverse of [`TyKind::suffix`]; `object`/`instance-as-dyn` stay accepted as the B-track
    /// boxed value, whose name in the §2 scalar table is `dyn`.
    pub fn from_suffix(s: &str) -> Option<TyKind> {
        let k = match s {
            "void" => TyKind::Void,
            "unit" => TyKind::Unit,
            "bool" => TyKind::Bool,
            "char" => TyKind::Char,
            "i8" => TyKind::I8,
            "i16" => TyKind::I16,
            "i32" => TyKind::I32,
            "i64" | "int" => TyKind::Int,
            "u8" => TyKind::U8,
            "u16" => TyKind::U16,
            "u32" => TyKind::U32,
            "u64" => TyKind::U64,
            "f32" => TyKind::F32,
            "f64" | "float" => TyKind::Float,
            "str" => TyKind::Str,
            "bytes" => TyKind::Bytes,
            "ptr" => TyKind::Ptr,
            "dyn" | "object" => TyKind::Dyn,
            "list" => TyKind::List,
            "tuple" => TyKind::Tuple,
            "dict" => TyKind::Dict,
            "func" => TyKind::Func,
            "coro" => TyKind::Coro,
            "instance" => TyKind::Instance,
            "ref" => TyKind::Ref,
            "box" => TyKind::Box,
            "rc" => TyKind::Rc,
            "arc" => TyKind::Arc,
            "option" => TyKind::Option,
            "result" => TyKind::Result,
            "never" => TyKind::Never,
            "usize" => TyKind::Usize,
            "isize" => TyKind::Isize,
            _ => return None,
        };
        Some(k)
    }
}

/// `add` + `Some(Int)` = `add.i64`; the form written into a capability table (appendix E.3).
pub fn mnemonic(op: Op, ty: Option<TyKind>) -> String {
    match ty {
        Some(t) => format!("{}.{}", op.base(), t.suffix()),
        None => op.base().to_string(),
    }
}

/// Parse a capability-table mnemonic into (opcode, optional type suffix). Dotted names that are part
/// of the base (`push.b`, `call.host`, `guard.nonzero`, …) are matched longest-first over
/// [`all_ops`], so `push.b` is never mistaken for `push` with suffix `b`.
pub fn parse_mnemonic(text: &str) -> Option<(Op, Option<TyKind>)> {
    let text = text.trim();
    let mut best: Option<Op> = None;
    for op in all_ops() {
        let b = op.base();
        if text == b {
            return Some((op, None));
        }
        if let Some(rest) = text.strip_prefix(b) {
            if rest.starts_with('.') {
                let cur_len = best.map(|o| o.base().len()).unwrap_or(0);
                if b.len() > cur_len {
                    best = Some(op);
                }
            }
        }
    }
    let op = best?;
    let suffix = text[op.base().len() + 1..].trim();
    if suffix.is_empty() {
        return Some((op, None));
    }
    Some((op, TyKind::from_suffix(suffix)))
}

// ------------------------------------------------------------------------ §7.8.3 verifier

/// One loader-time violation. `code` is the `R-07xx` the runtime reports (§7.8.3, appendix D.10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyViolation {
    pub code: &'static str,
    pub func: u32,
    pub off: u32,
    pub item: String,
}

impl VerifyViolation {
    pub fn new(code: &'static str, func: u32, off: u32, item: impl Into<String>) -> VerifyViolation {
        VerifyViolation { code, func, off, item: item.into() }
    }
    pub fn render(&self) -> String {
        format!("{}: func #{} @ {:#06x}: {}", self.code, self.func, self.off, self.item)
    }
}

impl fmt::Display for VerifyViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

/// §7.8.3's six validations, per function in `func_table` order, plus the two structural mandates of
/// D.3 (guards) and D.7 (`is.dyn` before every `unbox.dyn`). `all_errors == false` stops at the
/// first violation, exactly like the runtime (D.10); `true` is `infy ifbc verify --all-errors`.
pub fn verify(m: &IfbcModule, hosts: &HostSigTable, all_errors: bool) -> Vec<VerifyViolation> {
    let mut out = Vec::new();
    for i in 0..m.funcs.len() {
        verify_func(m, hosts, i, &mut out);
        if !all_errors && !out.is_empty() {
            return out;
        }
    }
    // item 6 — line-map completeness is a warning, never a rejection (§7.8.3).
    for i in 0..m.funcs.len() {
        if let Ok(instrs) = instructions_of(m, i) {
            for (off, ins) in instrs {
                if m.line_of_code_off(off).is_none() {
                    out.push(VerifyViolation::new("W-0705", i as u32, off, format!("{} 无行映射", ins.op.base())));
                }
            }
        }
    }
    out
}

fn push(out: &mut Vec<VerifyViolation>, v: VerifyViolation) {
    out.push(v);
}

fn verify_func(m: &IfbcModule, hosts: &HostSigTable, fi: usize, out: &mut Vec<VerifyViolation>) {
    let f = match m.funcs.get(fi) {
        Some(f) => f,
        None => return,
    };
    let instrs = match instructions_of(m, fi) {
        Ok(v) => v,
        Err(e) => {
            // Item 1 (linearity / unknown opcode) — the image is not a well-formed instruction stream.
            push(out, VerifyViolation::new("R-0704", fi as u32, f.code_off, e.to_string()));
            return;
        }
    };
    let ends: Vec<u32> = instrs.iter().map(|(o, i)| *o + i.width() as u32).collect();
    let boundaries: std::collections::BTreeSet<u32> =
        instrs.iter().map(|(o, _)| *o).chain(ends.iter().copied()).collect();
    let f_end = f.code_off + f.code_len;

    // ---- item 1: linearity — jump targets must land on instruction boundaries, inside this frame
    for (off, ins) in &instrs {
        for (k, kind) in ins.op.operand_kinds().iter().enumerate() {
            if *kind != OperandKind::CodeOff {
                continue;
            }
            let t = ins.ops[k];
            if !boundaries.contains(&t) || t < f.code_off || t >= f_end {
                push(
                    out,
                    VerifyViolation::new(
                        "R-0704",
                        fi as u32,
                        *off,
                        format!("{} 的目标 {t:#06x} 不是本函数内的指令边界", ins.op.base()),
                    ),
                );
            }
        }
        // D-38 (h): `switch` is legal, so its inline table is checked like any other target set —
        // every case must land on an instruction boundary of this function.
        if matches!(ins.op, Op::Switch) {
            for t in switch_targets(m, *off, ins) {
                if !boundaries.contains(&t) || t < f.code_off || t >= f_end {
                    push(
                        out,
                        VerifyViolation::new(
                            "R-0704",
                            fi as u32,
                            *off,
                            format!("switch 的某个 case 目标 {t:#06x} 不是本函数内的指令边界"),
                        ),
                    );
                }
            }
        }
        if matches!(ins.op, Op::LdNull) {
            push(out, VerifyViolation::new("R-0706", fi as u32, *off, "ldnull 被禁用：INFY 无 null，须用 Option".to_string()));
        }
    }

    // ---- items 2/3/4/5: one abstract execution per block
    // Items 3 and 5 first, exactly once per instruction: operand legality and D.3/D.7 adjacency do
    // not depend on the stack height, and running them inside the walk below re-reported one defect
    // once per path that reached it (the loader's `check_type` walks each boundary once, and D4(3)
    // holds both sides to one text — see deviation D-42).
    for (idx, (off, ins)) in instrs.iter().enumerate() {
        check_operands(m, ins, f, *off, fi, out);
        check_adjacency(m, &instrs, idx, *off, fi, out);
    }
    let mut height: BTreeMap<u32, i64> = BTreeMap::new();
    let mut work: Vec<u32> = vec![f.code_off];
    height.insert(f.code_off, 0);
    let index: BTreeMap<u32, usize> = instrs.iter().enumerate().map(|(i, (o, _))| (*o, i)).collect();
    while let Some(start) = work.pop() {
        let mut h = height.get(&start).copied().unwrap_or(0);
        let mut idx = match index.get(&start) {
            Some(i) => *i,
            None => continue,
        };
        loop {
            let (off, ins) = match instrs.get(idx) {
                Some(x) => *x,
                None => break,
            };
            let (pops, pushes) = effect(m, fi, &ins, off, hosts, out);            if h < pops as i64 {
                push(
                    out,
                    VerifyViolation::new(
                        "R-0705",
                        fi as u32,
                        off,
                        format!("{} 需要 {pops} 个栈槽，块入口高度只有 {h}", ins.op.base()),
                    ),
                );
                break;
            }
            h = h - pops as i64 + pushes as i64;
            // D-38: `max_stack` is on the wire now, so item 2 checks the height against the declared
            // ceiling exactly like the loader does (`R-0705`).
            if h > f.max_stack as i64 {
                push(
                    out,
                    VerifyViolation::new(
                        "R-0705",
                        fi as u32,
                        off,
                        format!("{} 之后栈高 {h} 超过声明上限 max_stack={}", ins.op.base(), f.max_stack),
                    ),
                );
            }

            match ins.op {
                Op::Jmp => {
                    push_block(&mut height, &mut work, ins.ops[0], h, out, fi, off, "jmp");
                    break;
                }
                Op::Br | Op::Brf => {
                    push_block(&mut height, &mut work, ins.ops[0], h, out, fi, off, ins.op.base());
                }
                // every case target is a block entry, and so is the byte after the table (D-38 (h)).
                Op::Switch => {
                    for t in switch_targets(m, off, &ins) {
                        push_block(&mut height, &mut work, t, h, out, fi, off, "switch case");
                    }
                }
                Op::Ret | Op::ThrowInf => {
                    if matches!(ins.op, Op::Ret) && h < 1 {
                        push(out, VerifyViolation::new("R-0707", fi as u32, off, "ret 时操作数栈为空".to_string()));
                    }
                    break;
                }
                Op::RetMulti => {
                    let n = ins.ops[0] as i64;
                    if h < n {
                        push(
                            out,
                            VerifyViolation::new("R-0707", fi as u32, off, format!("ret.multi {n} 需要 {n} 个值，栈高 {h}")),
                        );
                    }
                    break;
                }
                _ => {}
            }
            idx += 1;
            if idx >= instrs.len() {
                break;
            }
            let next = instrs[idx].0;
            // A block is entered with a single height; a second arrival with a different height is
            // R-0705 (item 2).
            match height.get(&next) {
                Some(&prev) if prev != h => {
                    push(
                        out,
                        VerifyViolation::new(
                            "R-0705",
                            fi as u32,
                            next,
                            format!("基本块入口高度不唯一：{prev} 与 {h}"),
                        ),
                    );
                    break;
                }
                Some(_) => {}
                None => {
                    // Record the entry height and keep walking into `next` right here: pushing it as
                    // a work item too re-entered the same block from its own successor, which made
                    // `effect` report a table-dependent finding once per path (D4(3) / D-42).
                    height.insert(next, h);
                }
            }
        }
    }

    // ---- item 4: frame legality — the last instruction of a function must terminate it
    if let Some((off, last)) = instrs.last() {
        if !matches!(last.op, Op::Ret | Op::RetMulti | Op::ThrowInf) {
            push(
                out,
                VerifyViolation::new(
                    "R-0707",
                    fi as u32,
                    *off,
                    format!("函数以 {} 结束，未收口于 ret/ret.multi/throw.inf", last.op.base()),
                ),
            );
        }
    } else {
        push(out, VerifyViolation::new("R-0707", fi as u32, f.code_off, "函数无代码".to_string()));
    }
    // ---- item 4 (the mirror of `infy_ifbc.c::check_frame`): coroutine-only instructions need a
    // coroutine frame, and the return shape the code actually takes must agree with the declared
    // `ret_arity` D-38 put in `func_table`.
    let mut ret_arity_seen: i64 = -1;
    for (off, ins) in &instrs {
        if matches!(ins.op, Op::Yield | Op::YieldFrom | Op::FrameEnter | Op::FrameExit)
            && f.frame_kind != FRAME_KIND_COROUTINE as u32
        {
            push(
                out,
                VerifyViolation::new(
                    "R-0707",
                    fi as u32,
                    *off,
                    format!("{} 出现在非协程帧（frame_kind={}）", ins.op.base(), f.frame_kind),
                ),
            );
        }
        match ins.op {
            Op::Ret => {
                if ret_arity_seen < 0 {
                    ret_arity_seen = 1;
                } else if ret_arity_seen != 1 {
                    push(
                        out,
                        VerifyViolation::new("R-0707", fi as u32, *off, "各返回路径的返回元数不一致".to_string()),
                    );
                }
            }
            Op::RetMulti => {
                let n = ins.ops[0] as i64;
                if ret_arity_seen < 0 {
                    ret_arity_seen = n;
                } else if ret_arity_seen != n {
                    push(
                        out,
                        VerifyViolation::new(
                            "R-0707",
                            fi as u32,
                            *off,
                            format!("ret.multi 的 {n} 与先前返回路径的 {ret_arity_seen} 不一致"),
                        ),
                    );
                }
                if n as u32 != f.ret_arity {
                    push(
                        out,
                        VerifyViolation::new(
                            "R-0707",
                            fi as u32,
                            *off,
                            format!("ret.multi {n} != 声明的 ret_arity={}", f.ret_arity),
                        ),
                    );
                }
            }
            _ => {}
        }
    }
    if f.ret_arity > 1 && ret_arity_seen == 1 {
        push(
            out,
            VerifyViolation::new(
                "R-0707",
                fi as u32,
                f.code_off,
                format!("声明返回元数 {} 但代码只返回单值", f.ret_arity),
            ),
        );
    }
    if f.ret_arity == 1 && ret_arity_seen > 1 {
        push(
            out,
            VerifyViolation::new(
                "R-0707",
                fi as u32,
                f.code_off,
                format!("声明标量返回但代码返回 {} 元组", ret_arity_seen),
            ),
        );
    }
    // `ret_sig` is a scalar code (§2), not a type index, since D-38 merged the record.
    if sig_kind(f.ret_sig as u8).is_none() {
        push(
            out,
            VerifyViolation::new(
                "R-0706",
                fi as u32,
                f.code_off,
                format!("返回签名码 {} 不在 §2 的封闭集内", f.ret_sig),
            ),
        );
    }

    // ---- item 5: every host call target must exist and match IFABI
    for (off, ins) in &instrs {
        if ins.op == Op::CallEmbed {
            // §5.5: the loader compares the row's kind against the call site (R-0708). `call.embed`
            // itself stays rejected by `effect` (this side cannot prove its height), but the row it
            // names is still checked, so a wrong-kind row is reported here rather than silently
            // reaching the runtime.
            let idx = ins.ops[0] as usize;
            match m.imports.get(idx) {
                None => push(
                    out,
                    VerifyViolation::new(
                        "R-0708",
                        fi as u32,
                        *off,
                        format!("call.embed 目标 #{idx} 不在 import_table 内"),
                    ),
                ),
                Some(e) if e.kind != ImportKind::Embed => push(
                    out,
                    VerifyViolation::new(
                        "R-0708",
                        fi as u32,
                        *off,
                        format!(
                            "call.embed 目标 {:?} 不是 embed 行（kind = {}）",
                            m.strings.get(e.name).cloned().unwrap_or_default(),
                            e.kind.word()
                        ),
                    ),
                ),
                Some(_) => {}
            }
        }
        if ins.op == Op::CallHost {
            let idx = ins.ops[0] as usize;
            if let Err(msg) = hosts.check_import(m, idx) {
                push(out, VerifyViolation::new("R-0708", fi as u32, *off, msg));
            }
        }
        if ins.op == Op::InvokeDynamic {
            let t = ins.ops[0];
            if t as usize >= m.funcs.len() {
                push(
                    out,
                    VerifyViolation::new("R-0712", fi as u32, *off, format!("invoke.dynamic 目标 #{t} 不在本模块 func_table 内")),
                );
            }
        }
    }
}

fn push_block(
    height: &mut BTreeMap<u32, i64>,
    work: &mut Vec<u32>,
    target: u32,
    h: i64,
    out: &mut Vec<VerifyViolation>,
    fi: usize,
    off: u32,
    what: &str,
) {
    if let Some(&prev) = height.get(&target) {
        if prev != h {
            push(
                out,
                VerifyViolation::new(
                    "R-0705",
                    fi as u32,
                    off,
                    format!("{what} 的目标 {target:#06x} 入口高度不唯一：{prev} 与 {h}"),
                ),
            );
        }
        return;
    }
    height.insert(target, h);
    work.push(target);
}

/// Stack effect, resolving the table-dependent instructions. Errors are recorded as violations.
fn effect(
    m: &IfbcModule,
    fi: usize,
    ins: &Instr,
    off: u32,
    hosts: &HostSigTable,
    out: &mut Vec<VerifyViolation>,
) -> (usize, usize) {
    match ins.op {
        // `new.list elem_ty, count` pops `count` elements.
        Op::NewList => (ins.ops[1] as usize, 1),
        Op::NewTuple => (ins.ops[0] as usize, 1),
        Op::NewSet => (ins.ops[0] as usize, 1),
        // `new.dict k_ty, v_ty, count` pops `2 * count` (the verifier caps the literal size).
        Op::NewDict => {
            let n = ins.ops[2] as usize;
            (2 * n, 1)
        }
        Op::ClosureNew => (ins.ops[1] as usize, 1),
        Op::CallStat => match m.funcs.get(ins.ops[0] as usize) {
            Some(t) => (t.nargs as usize, if t.ret_arity == 0 { 0 } else { 1 }),
            None => {
                push(
                    out,
                    VerifyViolation::new(
                        "R-0707",
                        fi as u32,
                        off,
                        format!("call.stat 目标 #{} 不存在（校验项 4）", ins.ops[0]),
                    ),
                );
                (0, 1)
            }
        },
        Op::TailCallStat => match m.funcs.get(ins.ops[0] as usize) {
            Some(t) => {
                // D-22: a load-time "boundary" finding is R-0707, and R-0709 belongs only to the
                // run-time arena check. The comparison set is the C loader's (`check_boundary`).
                let cur = m.funcs.get(fi);
                let incompatible = cur
                    .map(|c| {
                        c.locals != t.locals
                            || c.max_stack != t.max_stack
                            || c.nargs != t.nargs
                            || c.ret_arity != t.ret_arity
                            || c.frame_kind != t.frame_kind
                    })
                    .unwrap_or(false);
                if incompatible {
                    push(
                        out,
                        VerifyViolation::new(
                            "R-0707",
                            fi as u32,
                            off,
                            format!(
                                "tailcall.stat 到 #{} 的帧尺寸不兼容（locals {}/{}, max_stack {}/{}, nargs {}/{}, ret_arity {}/{}, frame {}/{}）",
                                ins.ops[0],
                                m.funcs.get(fi).map(|c| c.locals).unwrap_or(0), t.locals,
                                m.funcs.get(fi).map(|c| c.max_stack).unwrap_or(0), t.max_stack,
                                m.funcs.get(fi).map(|c| c.nargs).unwrap_or(0), t.nargs,
                                m.funcs.get(fi).map(|c| c.ret_arity).unwrap_or(0), t.ret_arity,
                                m.funcs.get(fi).map(|c| c.frame_kind).unwrap_or(0), t.frame_kind,
                            ),
                        ),
                    );
                }
                (t.nargs as usize, 0)
            }
            None => (0, 0),
        },
        Op::CallHost => match m.imports.get(ins.ops[0] as usize) {
            Some(x) => {
                if hosts.check_import(m, ins.ops[0] as usize).is_err() {
                    // Reported once, in the item-5 pass, to keep `verify` output readable.
                }
                (
                    x.params.len(),
                    if matches!(sig_kind(x.ret_sig), Some(TyKind::Unit) | Some(TyKind::Void)) {
                        0
                    } else {
                        1
                    },
                )
            }
            None => {
                push(
                    out,
                    VerifyViolation::new(
                        "R-0708",
                        fi as u32,
                        off,
                        format!("call.host 目标 #{} 不在 import_table 内", ins.ops[0]),
                    ),
                );
                (0, 1)
            }
        },
        // D.5/D.7 dynamic dispatch: the callee's arity is not static, so validation item 2 cannot
        // be completed. v2.0's capability tables therefore leave these unimplemented.
        Op::CallVirt | Op::CallEmbed | Op::InvokeDynamic => {
            push(
                out,
                VerifyViolation::new(
                    "R-0705",
                    fi as u32,
                    off,
                    format!("{} 的被调元数无法静态确定，校验项 2 无法完成（能力表未覆盖）", ins.op.base()),
                ),
            );
            (0, 0)
        }
        _ => ins.op.stack_effect(),
    }
}

/// Item 3 (operand type vs suffix) plus the D.2 index bounds.
fn check_operands(m: &IfbcModule, ins: &Instr, f: &FuncEntry, off: u32, fi: usize, out: &mut Vec<VerifyViolation>) {
    let kinds = ins.op.operand_kinds();
    for (k, kind) in kinds.iter().enumerate() {
        let v = ins.ops[k];
        match kind {
            OperandKind::Slot => {
                if v >= f.locals {
                    push(
                        out,
                        VerifyViolation::new(
                            "R-0706",
                            fi as u32,
                            off,
                            format!("{} 的局部槽 {v} 超出帧窗口 {}", ins.op.base(), f.locals),
                        ),
                    );
                }
            }
            OperandKind::Type | OperandKind::Pool | OperandKind::Func | OperandKind::Import => {
                let (len, what) = match kind {
                    OperandKind::Type => (m.types.len(), "type_table"),
                    OperandKind::Pool => (m.strings.len(), "string_pool"),
                    OperandKind::Func => (m.funcs.len(), "func_table"),
                    _ => (m.imports.len(), "import_table"),
                };
                if v != NONE32 && v as usize >= len {
                    push(
                        out,
                        VerifyViolation::new(
                            "R-0706",
                            fi as u32,
                            off,
                            format!("{} 的 {what} 索引 {v} 越界（{len}）", ins.op.base()),
                        ),
                    );
                }
            }
            _ => {}
        }
    }
    // D.3's families carry their type in operand 0. Since D-38 (c) the two guards have no operand
    // at all — D.3's adjacency rule only ever inspects the previous byte.
    if ins.op.is_typed_family() {
        match m.ty_kind(ins.ops[0]) {
            // `not` is D.3's boolean family member; every other family needs a numeric suffix.
            Some(TyKind::Bool) if ins.op == Op::Not => {}
            Some(k) if k.is_numeric() => {}
            Some(k) => push(
                out,
                VerifyViolation::new(
                    "R-0706",
                    fi as u32,
                    off,
                    format!("{} 的类型后缀 {} 不是数值类型（D.3）", ins.op.base(), k.suffix()),
                ),
            ),
            None => push(
                out,
                VerifyViolation::new("R-0706", fi as u32, off, format!("{} 缺少类型后缀操作数", ins.op.base())),
            ),
        }
    }
    // D-38 (e): `ldw i,ty`'s annotation must be one of the three wide scalars the loader accepts —
    // `i64`(7) / `u64`(11) / `f64`(13) in the §2 code space.
    if ins.op == Op::Ldw {
        match m.ty_kind(ins.ops[1]) {
            Some(k) if matches!(k, TyKind::Int | TyKind::U64 | TyKind::Float) => {}
            Some(k) => push(
                out,
                VerifyViolation::new(
                    "R-0706",
                    fi as u32,
                    off,
                    format!("ldw 的宽类型注解 {} 只能是 i64/u64/f64（§2 的码表）", k.suffix()),
                ),
            ),
            None => push(
                out,
                VerifyViolation::new("R-0706", fi as u32, off, "ldw 的类型注解索引非法".to_string()),
            ),
        }
    }
    if matches!(ins.op, Op::IsDyn | Op::CastDyn) {
        match m.ty_kind(ins.ops[0]) {
            Some(k) if k.dyn_tag().is_some() => {}
            Some(k) => push(
                out,
                VerifyViolation::new(
                    "R-0710",
                    fi as u32,
                    off,
                    format!("{} 的类型 {} 不是 dyn 可见类型（UB-020）", ins.op.base(), k.suffix()),
                ),
            ),
            None => push(out, VerifyViolation::new("R-0710", fi as u32, off, "dyn 指令的类型索引非法".to_string())),
        }
    }
    // D-38 (g) + `IFBC.md` §5's extras: `field_off ty,field` needs a `repr(C)` row and an in-range
    // field index. Non-`repr(C)` is `R-0713`, the range failure is `R-0706`.
    if ins.op == Op::FieldOff {
        match m.types.get(ins.ops[0] as usize) {
            None => push(
                out,
                VerifyViolation::new("R-0706", fi as u32, off, "field_off 的类型索引越界".to_string()),
            ),
            Some(t) => {
                if t.flags & TF_REPR_C == 0 {
                    push(
                        out,
                        VerifyViolation::new(
                            "R-0713",
                            fi as u32,
                            off,
                            "field_off 作用于非 #[repr(C)] 的类型（D.9 / R_FIELD_OFF_REPR）".to_string(),
                        ),
                    );
                }
                if ins.ops[1] as usize >= t.fields.len() {
                    push(
                        out,
                        VerifyViolation::new(
                            "R-0706",
                            fi as u32,
                            off,
                            format!("field_off 的字段索引 {} 超出该行的 {nfields} 个字段", ins.ops[1], nfields = t.fields.len()),
                        ),
                    );
                }
            }
        }
    }
}

/// D.3 (`guard.nonzero` before `div`/`rem`, `guard.shift` before `shl`/`shr`/`shr.a`) and D.7
/// (`is.dyn` before every `unbox.dyn`). Adjacency is checked in the *instruction stream*, which is
/// what the two clauses mean; the byte values are D.3's own gaps (`DEV-IFBC-4`).
fn check_adjacency(
    m: &IfbcModule,
    instrs: &[(u32, Instr)],
    idx: usize,
    off: u32,
    fi: usize,
    out: &mut Vec<VerifyViolation>,
) {
    let ins = instrs[idx].1;
    let prev = if idx > 0 { Some(instrs[idx - 1].1) } else { None };
    let mut need = |what: &str, code: &'static str, ok: bool| {
        if !ok {
            push(
                out,
                VerifyViolation::new(
                    code,
                    fi as u32,
                    off,
                    format!("{} 前必须紧邻 {what}（检查不可删除，UB-003/UB-004 的字节码侧对应）", ins.op.base()),
                ),
            );
        }
    };
    match ins.op {
        // D-38 (c): the guards carry no operand, so adjacency is the byte test the loader does.
        Op::Div | Op::Rem => need(
            "guard.nonzero",
            "R-0706",
            prev.map(|p| p.op == Op::GuardNonzero).unwrap_or(false),
        ),
        Op::Shl | Op::Shr | Op::ShrA => need(
            "guard.shift",
            "R-0706",
            prev.map(|p| p.op == Op::GuardShift).unwrap_or(false),
        ),
        Op::UnboxDyn => need(
            "is.dyn",
            "R-0710",
            prev.map(|p| p.op == Op::IsDyn).unwrap_or(false),
        ),
        _ => {}
    }
    let _ = m;
}

// ------------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;

    const TICKLOOP_TEST: u32 = 65536;

    /// A module built by hand: `f(a) = (a / 2) + 1`, exercising the pool, the type table, the D.3
    /// guards, the D.7 tag checkpoints, the line map and the exception table.
    fn sample() -> IfbcModule {
        let mut m = IfbcModule::new_unit("CPY-313", 1, "__infy_unit__", "b_track/demo.py").unwrap();
        // D-38: index 0..=17 *is* the scalar code, so `i64` is 7 without interning anything.
        let ty_i = TyKind::Int as u32;
        let k2 = m.intern("2");
        let k1 = m.intern("1");
        let hosts = HostSigTable::std_default();
        let mut b = CodeBuilder::new(0);
        b.set_locals(2);
        b.emit(Op::Ldl, &[0], Some(1)).unwrap();
        b.emit(Op::IsDyn, &[ty_i], Some(1)).unwrap();
        b.emit(Op::UnboxDyn, &[], Some(1)).unwrap();
        b.emit(Op::Push, &[k2, ty_i], Some(1)).unwrap();
        b.emit(Op::IsDyn, &[ty_i], Some(1)).unwrap();
        b.emit(Op::UnboxDyn, &[], Some(1)).unwrap();
        // D-38 (c): the guards carry no operand; D-38 (f): neither does `box.dyn`.
        b.emit(Op::GuardNonzero, &[], Some(1)).unwrap();
        b.emit(Op::Div, &[ty_i], Some(1)).unwrap();
        b.emit(Op::BoxDyn, &[], Some(1)).unwrap();
        b.emit(Op::Push, &[k1, ty_i], Some(1)).unwrap();
        b.emit(Op::TickLoop, &[TICKLOOP_TEST], Some(2)).unwrap();
        b.emit(Op::Jmp, &[0], Some(2)).unwrap();
        b.emit(Op::Ret, &[], Some(3)).unwrap();
        // the marks belong to the builder, and `finish_into` consumes it, so snapshot them first
        let max_stack = b.max_stack();
        let lines: Vec<LineEntry> = b.marks().iter().map(|(o, l)| LineEntry { code_off: *o, line: *l }).collect();
        let (off, len) = b.finish_into(&mut m);
        m.lines = lines;
        m.exceptions.push(ExcEntry {
            start: off,
            end: off + len,
            handler: off,
            depth: 1,
            kind: ExcKind::Cleanup,
        });
        let _ = m.intern_host(&hosts, "std.io.log");
        m.push_func(
            "__infy_unit__",
            off,
            len,
            2,
            max_stack,
            &[TyKind::Dyn],
            TyKind::Dyn,
            0,
            F_ENTRY,
        );
        m.seal();
        m
    }

    /// Emit one function's body into `m` and return `(code_off, code_len, max_stack)`. A `Switch`
    /// entry passes its **case targets** (the count is written from the list length), which is the
    /// only variable-width form (D-38 (h)).
    fn build_into(m: &mut IfbcModule, locals: u32, body: &[(Op, Vec<u32>)]) -> (u32, u32, u32) {
        let mut b = CodeBuilder::new(m.code.len() as u32);
        b.set_locals(locals);
        for (op, ops) in body {
            if *op == Op::Switch && ops.len() != 1 {
                b.switch(ops, Some(1)).expect("switch");
            } else {
                b.emit(*op, ops, Some(1)).expect("emit");
            }
        }
        let max_stack = b.max_stack();
        let marks: Vec<(u32, u32)> = b.marks().to_vec();
        let (off, len) = b.finish_into(m);
        for (o, l) in marks {
            m.lines.push(LineEntry { code_off: o, line: l });
        }
        (off, len, max_stack)
    }

    /// A one-function module whose body is `body`, returning `ret`.
    fn one_func(locals: u32, body: &[(Op, Vec<u32>)], ret: TyKind) -> IfbcModule {
        let mut m = IfbcModule::new_unit("CPY-313", 1, "u", "s.py").unwrap();
        let (off, len, ms) = build_into(&mut m, locals, body);
        m.push_func("u", off, len, locals, ms, &[], ret, 0, F_ENTRY);
        m.seal();
        m
    }

    /// Every `R-07xx` a module draws from the build-time verifier (warnings dropped: W-0705 is a
    /// notice by §7.8.3 and would fire on every hand-written body).
    fn r_codes(m: &IfbcModule) -> Vec<&'static str> {
        verify(m, &HostSigTable::std_default(), true)
            .into_iter()
            .filter(|v| v.code.starts_with('R'))
            .map(|v| v.code)
            .collect()
    }

    fn r_reports(m: &IfbcModule) -> Vec<String> {
        verify(m, &HostSigTable::std_default(), true)
            .into_iter()
            .filter(|v| v.code.starts_with('R'))
            .map(|v| v.render())
            .collect()
    }

    /// D4(2)'s cross-language lock: the module whose bytes are committed as
    /// `runtime/fixtures/ifbc_rust_writer.bin`. It exercises exactly the shapes D-38 merged — a
    /// family byte with its type operand, a two-operand `push k,ty`, the 48-byte func record and
    /// the 18 seeded scalar rows — and `runtime/test_ifbc.c` loads *and executes* those bytes with
    /// the production loader/interpreter (the same手法 as C3's artifact fixture).
    fn c_loader_fixture_module() -> IfbcModule {
        let mut m = IfbcModule::new_unit("CPY-313", 1, "u", "s.py").unwrap();
        let k_one = m.intern("1");
        let (off0, len0, ms0) = build_into(
            &mut m,
            1,
            &[(Op::Ldl, vec![0]), (Op::PushB, vec![1]), (Op::Add, vec![7]), (Op::Ret, vec![])],
        );
        m.push_func("add_one", off0, len0, 1, ms0, &[TyKind::Int], TyKind::Int, 0, F_ENTRY);
        let (off1, len1, ms1) =
            build_into(&mut m, 0, &[(Op::Push, vec![k_one, 14]), (Op::Ret, vec![])]);
        m.push_func("pool_str", off1, len1, 0, ms1, &[], TyKind::Str, 0, 0);
        m.seal();
        m
    }

    /// Two sides, one image: this writes the bytes, `runtime/test_ifbc.c::test_rust_writer_fixture`
    /// hands the same file to `infy_ifbc_load` + `infy_ifbc_invoke`. Only
    /// `INFY_REGENERATE_FIXTURE=1` rewrites the file, so a wire-format drift fails one side or the
    /// other instead of silently moving the goalposts.
    #[test]
    fn c_loader_fixture_is_exactly_what_the_writer_produces() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/fixtures/ifbc_rust_writer.bin");
        let m = c_loader_fixture_module();
        let bytes = m.encode();
        if std::env::var("INFY_REGENERATE_FIXTURE").is_ok() {
            std::fs::write(path, &bytes).expect("explicit fixture regeneration");
        }
        let on_disk = std::fs::read(path)
            .unwrap_or_else(|e| panic!("{} 不在仓库里（{e}）—— 用 INFY_REGENERATE_FIXTURE=1 生成一次", path));
        assert_eq!(on_disk, bytes, "IFBC 写入器输出与已提交夹具不再逐字节相同（D-38 线格式漂移）");
        // The C loader rejects the whole module on any finding, so the fixture has to be clean here
        // too — build-time verifier and runtime loader must reach the same verdict on these bytes.
        assert!(r_codes(&m).is_empty(), "the fixture draws a load-time violation: {:?}", r_reports(&m));
        assert_eq!(m.types.len(), N_SCALAR_ROWS);
        assert_eq!(m.funcs.len(), 2);
    }

    /// D4(3)'s second image: **every** opcode appendix D defines, in byte order, one instruction
    /// each — the 17 gap ops and `switch`'s inline case table included — plus one family byte whose
    /// type operand names no row, which is the `?` suffix path both sides share. Every operand is
    /// chosen inside its own table because this side's `decode` treats a dangling reference as a
    /// *container* error (`validate_refs`) while the loader treats it as `R-0706`; an image either
    /// side refuses has no shared text. Committed as `runtime/fixtures/ifbc_all_ops.bin`.
    fn all_ops_fixture_module() -> IfbcModule {
        let mut m = IfbcModule::new_unit("CPY-313", 1, "u", "all_ops.py").unwrap();
        // one import row, so `import` / `call.host` / `call.embed` have a row to name.
        assert_eq!(m.intern_host(&HostSigTable::std_default(), "std.io.log"), Some(0));
        let body: Vec<(Op, Vec<u32>)> = all_ops()
            .into_iter()
            .map(|op| {
                if op == Op::Switch {
                    // the inline case table is part of the instruction (D-38 (h))
                    return (op, vec![0u32, 7]);
                }
                let mut kinds = op.operand_kinds();
                // D.3's families put their type in operand 0 but `operand_kinds` is the verifier's
                // table, where the families are handled by `is_typed_family` instead — pad to arity.
                while kinds.len() < op.arity() {
                    kinds.push(OperandKind::Type);
                }
                let ops = kinds
                    .into_iter()
                    .map(|k| match k {
                        OperandKind::Func | OperandKind::Import => 0,
                        _ if op == Op::Sub => NONE32,
                        _ => 7,
                    })
                    .collect();
                (op, ops)
            })
            .collect();
        let (off, len, ms) = build_into(&mut m, 8, &body);
        m.push_func("all_ops", off, len, 8, ms, &[], TyKind::Int, 0, F_ENTRY);
        m.seal();
        m
    }

    /// D4(3)'s third image: one function per load-time check **both** verifiers implement, each
    /// carrying exactly one defect, so §7.8.3's conclusions are comparable as
    /// `(code, func, off)` triples. The classes where the two sides provably reach different
    /// findings — out-of-range operands (this side rejects them while decoding, the loader as
    /// `R-0706`), the dynamic-call rejections, the "a function must end in a terminator" rule, and
    /// `W-0705`'s granularity — are listed in deviation **D-42** instead of being rendered here, and
    /// this side must not claim an equivalence it cannot show.
    fn verify_matrix_module() -> IfbcModule {
        let mut m = IfbcModule::new_unit("CPY-313", 1, "u", "verify_matrix.py").unwrap();
        // `field_off` needs a legal field index so only the layout rule fires: a non-`repr(C)` row
        // with three fields (D.9 / R-0713).
        let inst = m.intern_type(TyKind::Instance, "P", NONE32, NONE32);
        for (i, n) in ["a", "b", "c"].iter().enumerate() {
            m.add_type_field(inst, n, (i * 8) as u32, TyKind::Int as u32);
        }
        let ty_i = TyKind::Int as u32;
        // f0 comes first on purpose: `build_into` starts it at code offset 0, which is the one
        // target its `switch` can name and still be legal (a case must land on a boundary *of its
        // own function*, so any other function's entry offset would be a second finding).
        let cases: Vec<(u32, Vec<(Op, Vec<u32>)>)> = vec![
            // f0 switch case target that is not a boundary -> R-0704 (D-38 (h)), one bad target
            (1, vec![(Op::PushB, vec![5]), (Op::Switch, vec![0, 7]), (Op::Ldl, vec![0]), (Op::Ret, vec![])]),
            // f1 ldnull anywhere -> R-0706 (both sides reject it at the instruction)
            (1, vec![(Op::LdNull, vec![]), (Op::Ret, vec![])]),
            // f2 div without its guard -> R-0706 (D.3, UB-003)
            (1, vec![(Op::PushB, vec![1]), (Op::PushB, vec![1]), (Op::Div, vec![ty_i]), (Op::Ret, vec![])]),
            // f3 shl without its guard -> R-0706 (D.3, UB-004)
            (1, vec![(Op::PushB, vec![1]), (Op::PushB, vec![1]), (Op::Shl, vec![ty_i]), (Op::Ret, vec![])]),
            // f4 unbox.dyn with no tag checkpoint -> R-0710 (UB-020)
            (1, vec![(Op::PushB, vec![1]), (Op::UnboxDyn, vec![]), (Op::Ret, vec![])]),
            // f5 jump into the middle of an instruction -> R-0704 (item 1)
            (1, vec![(Op::Jmp, vec![2]), (Op::Ldl, vec![0]), (Op::Ret, vec![])]),
            // f6 pop with an empty operand stack -> R-0705 (item 2)
            (1, vec![(Op::Pop, vec![]), (Op::Ret, vec![])]),
            // f7 field_off on a non-repr(C) row -> R-0713
            (1, vec![(Op::FieldOff, vec![inst, 1]), (Op::Ret, vec![])]),
            // f8 yield inside a stack frame -> R-0707 (item 4, D.8)
            (1, vec![(Op::PushB, vec![1]), (Op::Yield, vec![]), (Op::Ret, vec![])]),
            // f9 tailcall into a frame of a different shape -> R-0707 (D-22: not R-0709)
            (1, vec![(Op::PushB, vec![1]), (Op::TailCallStat, vec![10]), (Op::Ret, vec![])]),
            // f10 the tailcall target: deliberately clean (its locals differ from f9's)
            (2, vec![(Op::PushB, vec![1]), (Op::Ret, vec![])]),
        ];
        for (i, (locals, body)) in cases.into_iter().enumerate() {
            let (off, len, ms) = build_into(&mut m, locals, &body);
            m.push_func(&format!("c{i}"), off, len, locals, ms, &[], TyKind::Int, 0, 0);
        }
        m.seal();
        m
    }

    fn fixture_bytes(dir: &str, name: &str) -> Vec<u8> {
        let path = format!("{dir}{name}");
        std::fs::read(&path).unwrap_or_else(|e| {
            panic!("{path} 不在仓库里（{e}）—— .bin 用 INFY_REGENERATE_FIXTURE=1 生成，文本由 C 侧的 INFY_REGENERATE_CANONICAL=1 生成")
        })
    }

    /// The two `.bin` images D4(3) added are the Rust writer's own output, pinned the same way
    /// D4(2) pinned `ifbc_rust_writer.bin`: only `INFY_REGENERATE_FIXTURE=1` rewrites them.
    #[test]
    fn canonical_cross_check_images_are_the_writers_output() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/fixtures/");
        for (name, m) in [
            ("ifbc_all_ops.bin", all_ops_fixture_module()),
            ("ifbc_verify_matrix.bin", verify_matrix_module()),
        ] {
            let bytes = m.encode();
            if std::env::var("INFY_REGENERATE_FIXTURE").is_ok() {
                std::fs::write(format!("{dir}{name}"), &bytes).expect("explicit fixture regeneration");
            }
            let on_disk = fixture_bytes(dir, name);
            assert_eq!(on_disk, bytes, "{name} 与 IFBC 写入器的输出不再逐字节相同（D-38 线格式漂移）");
            assert!(
                decode(&bytes).is_ok(),
                "{name} 必须能被本侧解码（规范化渲染没有可比文本）：{:?}",
                decode(&bytes).err()
            );
        }
    }

    /// D4(3) proper: this side's canonical text has to equal the text the **C loader** rendered into
    /// the committed `.can.txt` / `.ver.txt`. Neither side defines the answer alone — the texts only
    /// move when `runtime/test_ifbc.c` is run with `INFY_REGENERATE_CANONICAL=1`, so a drift in
    /// either renderer, or in either reader's idea of a record, fails here and in the C harness.
    #[test]
    fn canonical_render_equals_the_c_loader_render() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/fixtures/");
        let hosts = HostSigTable::std_default();
        for (bin, txt) in [
            ("ifbc_rust_writer.bin", "ifbc_rust_writer.can.txt"),
            ("ifbc_all_ops.bin", "ifbc_all_ops.can.txt"),
        ] {
            let bytes = fixture_bytes(dir, bin);
            let want = String::from_utf8(fixture_bytes(dir, txt)).expect("canonical text is UTF-8");
            let got = dump_canonical(&bytes).unwrap_or_else(|e| panic!("{bin} 本侧无法规范化渲染: {e}"));
            assert_eq!(want, got, "{txt} 不是本侧对 {bin} 的渲染结果 —— 两侧的反汇编又开始分叉");
            if bin == "ifbc_all_ops.bin" {
                let ins = got.lines().filter(|l| l.starts_with("I ")).count();
                assert_eq!(ins, all_ops().len(), "{bin} 必须覆盖附录 D 的全部 opcode");
                assert!(got.contains("sub.?"), "{bin} 必须覆盖类型操作数越界的 `?` 后缀");
                assert!(got.lines().any(|l| l.starts_with("I ") && l.split(' ').nth(3) == Some("67")), "switch 的家族字节必须在场");
                assert!(got.contains("new.set") && got.contains("seq.iter") && got.contains("inst.get"), "间隙指令必须在场");
            }
        }
        // §7.8.3's conclusions, compared as sorted (code, func, off) triples.
        let bytes = fixture_bytes(dir, "ifbc_verify_matrix.bin");
        let m = decode(&bytes).expect("the verify matrix must decode");
        let got = verify_canonical(&m, &hosts);
        let want = String::from_utf8(fixture_bytes(dir, "ifbc_verify_matrix.ver.txt")).expect("UTF-8");
        assert_eq!(want, got, "两侧的装载期校验结论不再逐条相同");
        let codes: Vec<&str> = got.lines().filter_map(|l| l.split(' ').nth(1)).collect();
        for c in ["R-0704", "R-0705", "R-0706", "R-0707", "R-0710", "R-0713"] {
            assert!(codes.iter().any(|x| *x == c), "校验矩阵没有覆盖 {c}");
        }
        // and the clean image is clean on both sides (no blocking finding at all).
        let clean = fixture_bytes(dir, "ifbc_rust_writer.bin");
        assert_eq!(
            verify_canonical(&decode(&clean).expect("decodes"), &hosts),
            "",
            "夹具镜像在本侧引出了阻塞性校验结论，而 C 侧接受它"
        );
    }

    #[test]
    fn encode_decode_encode_is_idempotent() {        let m = sample();
        let a = m.encode();
        let d = decode(&a).expect("decode");
        let b = d.encode();
        assert_eq!(a, b, "encode -> decode -> encode must be byte-identical");
        assert_eq!(&a[0..4], MAGIC);
        assert_eq!(d.lang.tag, m.lang.tag);
        assert_eq!(d.funcs.len(), 1);
        assert_eq!(d.strings.entries(), m.strings.entries());
        assert_eq!(d.lines, m.lines);
        assert_eq!(d.exceptions, m.exceptions);
    }

    /// D-38 (a): the regions are self-delimiting — nothing between the header and `type_table`'s own
    /// `u32 n_types`, and every region's extent follows from its record shape. This is the byte
    /// layout `runtime/infy_ifbc.c::parse_sections` walks, so the offsets are arithmetic, not a
    /// captured dump: type = 8 u32/row, import = 13 + nparams, func = 4 + 8 + 48/row.
    #[test]
    fn regions_are_self_delimiting_exactly_where_the_loader_reads_them() {
        let m = sample();
        let img = m.encode();
        // 18 seeded scalar rows, no extras: `i64`/`dyn`/`unit` all resolve inside the block.
        assert_eq!(m.types.len(), N_SCALAR_ROWS);
        let n_types = u32::from_be_bytes([img[47], img[48], img[49], img[50]]);
        assert_eq!(n_types, 18, "type_table opens with its own count at the header end");
        let type_len = 4 + 18 * 32;
        let import_at = HEADER_LEN + type_len;
        assert_eq!(
            u32::from_be_bytes([img[import_at], img[import_at + 1], img[import_at + 2], img[import_at + 3]]),
            1,
            "import_table follows immediately (no byte_len wrapper)"
        );
        let import_len = 4 + (4 + 1 + 1 + 1 + 1 + 4 + 1); // one host row, IFABI std.io.log -> (dyn) -> unit
        let func_at = import_at + import_len;
        assert_eq!(u32::from_be_bytes([img[func_at], img[func_at + 1], img[func_at + 2], img[func_at + 3]]), 1);
        assert_eq!(
            u64::from_be_bytes([
                img[func_at + 4],
                img[func_at + 5],
                img[func_at + 6],
                img[func_at + 7],
                img[func_at + 8],
                img[func_at + 9],
                img[func_at + 10],
                img[func_at + 11]
            ]),
            0,
            "n_funcs is followed by max_mem_bytes u64 (IFBC.md §1.5)"
        );
        let code_at = func_at + 4 + 8 + 48;
        let code_len =
            u32::from_be_bytes([img[code_at], img[code_at + 1], img[code_at + 2], img[code_at + 3]]) as usize;
        // the region holds the stream padded up to a 4-byte boundary (D.1), which is what `encode`
        // writes and what the loader reads as `code_len`
        let padded = (m.code.len() + 3) / 4 * 4;
        assert_eq!(code_len, padded, "code opens with its own byte length");
        assert_eq!(code_len % 4, 0);
        let lines_at = code_at + 4 + code_len;
        assert_eq!(
            u32::from_be_bytes([img[lines_at], img[lines_at + 1], img[lines_at + 2], img[lines_at + 3]]) as usize,
            m.lines.len()
        );
        // the whole image is accounted for: nothing is left over, which is what `TrailingBytes` guards
        let rest = img.len() - lines_at;
        assert_eq!(
            rest,
            4 + 8 * m.lines.len() + 4 + 16 + 4 + m.strings.entries().iter().map(|s| 4 + s.len()).sum::<usize>()
        );
        assert_eq!(rest + lines_at, img.len());
    }

    /// D-38 (a): the first 18 rows are the scalar codes, so a `u8` signature code and a `u32` type
    /// index address one space, and re-interning a scalar kind never appends a row.
    #[test]
    fn scalar_rows_are_seeded_and_type_indices_equal_scalar_codes() {
        let mut m = IfbcModule::new_unit("CPY-313", 1, "u", "s").unwrap();
        assert_eq!(m.types.len(), N_SCALAR_ROWS);
        for k in 0..N_SCALAR_ROWS {
            assert_eq!(m.types[k].kind, k as u32, "row #{k} must *be* scalar code {k}");
            assert_eq!(m.type_name(k as u32).unwrap(), sig_kind(k as u8).unwrap().suffix());
        }
        assert_eq!(m.intern_type(TyKind::Int, "i64", NONE32, NONE32), TyKind::Int as u32);
        assert_eq!(m.intern_type(TyKind::Dyn, "dyn", NONE32, NONE32), TyKind::Dyn as u32);
        assert_eq!(m.types.len(), N_SCALAR_ROWS, "a scalar kind never grows the table");
        // an extended kind gets its own row past the block, and folds to `dyn` as a signature code
        let elem = TyKind::Int as u32;
        let lst = m.intern_type(TyKind::List, "list", elem, NONE32);
        assert_eq!(lst as usize, N_SCALAR_ROWS);
        assert_eq!(TyKind::List.sig_code(), TyKind::Dyn as u8);
        assert_eq!(TyKind::Int.sig_code(), 7, "i64 is SIG_I64 = 7 in infy_ifbc.c's SIG_* enum");
        assert_eq!(TyKind::U64.sig_code(), 11);
        assert_eq!(TyKind::Float.sig_code(), 13, "f64 is SIG_F64 = 13");
        assert_eq!(TyKind::Dyn.dyn_tag(), Some(11), "DY_INSTANCE: the boxed dynamic value");
        assert_eq!(TyKind::Int.dyn_tag(), Some(1), "DY_INT survived the renumbering");
    }

    #[test]
    fn import_rows_store_scalar_codes_and_the_sig_digest() {
        let mut m = IfbcModule::new_unit("CPY-313", 1, "u", "s").unwrap();
        let mut hosts = HostSigTable::new();
        hosts.register(HostSig::new("x.range", &[TyKind::Usize, TyKind::List], TyKind::List));
        let idx = m.intern_host(&hosts, "x.range").expect("row");
        let e = &m.imports[idx as usize];
        // usize(31) and list(18) are outside §2's closed set: on the IFABI wire both are a boxed dyn.
        assert_eq!(e.params, vec![TyKind::Dyn.sig_code(), TyKind::Dyn.sig_code()]);
        assert_eq!(e.ret_sig, TyKind::Dyn.sig_code());
        assert_eq!(e.kind, ImportKind::Host);
        assert_eq!(e.flags & IMP_SIGNED, IMP_SIGNED, "the row only exists after the IFABI check");
        assert_eq!(e.sig_text(), "(dyn, dyn) -> dyn");
        assert_eq!(e.sig_hash, HostSig::hash_text("(dyn, dyn) -> dyn"), "digest covers the wire form");
    }

    /// D-38 (b)/(c)/(d)/(e)/(f)/(g)/(h): one table the two sides can diff line by line.
    #[test]
    fn opcode_bytes_and_operand_counts_are_the_d38_table() {
        assert_eq!(Op::GuardNonzero.byte(), 0x27);
        assert_eq!(Op::GuardShift.byte(), 0x2F);
        assert_eq!(Op::GuardNonzero.arity(), 0, "D-38 (c): guards carry no type operand");
        assert_eq!(Op::GuardShift.arity(), 0);
        assert_eq!(Op::Ldg.byte(), 0x0E, "D-38 (i): 0x0C/0x0D are freed for the guards' old bytes");
        assert_eq!(Op::Stg.byte(), 0x0F);
        assert_eq!(Op::Push.arity(), 2, "D-38 (d): push k,ty");
        assert_eq!(Op::Ldw.arity(), 2, "D-38 (e): ldw i,ty");
        assert_eq!(Op::BoxDyn.arity(), 0, "D-38 (f): boxing is the identity");
        assert_eq!(Op::FieldOff.arity(), 2, "D-38 (g): field_off ty,field");
        assert_eq!(Op::TypeOfIdx.arity(), 1, "D-38 addendum: D.9's column drops the index it needs");
        assert_eq!(Op::NewFrame.arity(), 1);
        assert_eq!(Op::Switch.arity(), 1);
        assert!(Op::Switch.is_variable_width(), "D-38 (h): the case table is inline");
        // the D.3 family block: 0x20..0x36 with operand 0 = type index
        for (op, byte) in [
            (Op::Add, 0x20u8),
            (Op::Sub, 0x21),
            (Op::Mul, 0x22),
            (Op::Div, 0x23),
            (Op::Rem, 0x24),
            (Op::Neg, 0x25),
            (Op::Abs, 0x26),
            (Op::And, 0x28),
            (Op::Or, 0x29),
            (Op::Xor, 0x2A),
            (Op::Not, 0x2B),
            (Op::Shl, 0x2C),
            (Op::Shr, 0x2D),
            (Op::ShrA, 0x2E),
            (Op::CmpLt, 0x30),
            (Op::CmpLe, 0x31),
            (Op::CmpGt, 0x32),
            (Op::CmpGe, 0x33),
            (Op::CmpEq, 0x34),
            (Op::CmpNe, 0x35),
            (Op::Cmp, 0x36),
        ] {
            assert_eq!(op.byte(), byte, "{} keeps D.3's family byte", op.base());
            assert_eq!(op.arity(), 1, "{} carries its type in operand 0", op.base());
        }
        // every defined byte round-trips through the table
        for b in 0..=255u8 {
            if let Some(op) = Op::from_byte(b) {
                assert_eq!(op.byte(), b);
            }
        }
        // the sample's guard really sits at 0x27 in the emitted stream
        let m = sample();
        assert_eq!(m.code[find_instr(&m, Op::GuardNonzero)], 0x27);
    }

    /// D-38 (h) + D-39: `switch n, [targets]` encodes as `1 + 4*(1+n)` bytes, decodes as ONE
    /// instruction (the cases are never mistaken for code), and its targets take part in the
    /// linearity check like any other jump.
    ///
    /// Layout (three cases): `0 push.b` · `5 ldl` · `10 switch[3]` · `27 nop` · `28 ret`, so the
    /// boundaries are `{0, 5, 10, 27, 28}` and every case lands on one of them with the same height.
    #[test]
    fn switch_is_variable_width_inline_and_linear() {
        let mut m = IfbcModule::new_unit("CPY-313", 1, "u", "s.py").unwrap();
        let (off, len, ms) = build_into(
            &mut m,
            1,
            &[
                (Op::PushB, vec![1]),
                (Op::Ldl, vec![0]),
                (Op::Switch, vec![27, 27, 28]),
                (Op::Nop, vec![]),
                (Op::Ret, vec![]),
            ],
        );
        m.push_func("u", off, len, 1, ms, &[], TyKind::Int, 0, F_ENTRY);
        m.seal();
        let d = decode(&m.encode()).expect("the switch image decodes");
        let instrs = instructions_of(&d, 0).unwrap();
        let widths: Vec<usize> = instrs.iter().map(|(_, i)| i.width()).collect();
        assert_eq!(
            widths,
            vec![5, 5, 1 + 4 * 4, 1, 1],
            "switch with 3 cases is 17 bytes: opcode + count + inline table"
        );
        let sw = instrs.iter().find(|(_, i)| i.op == Op::Switch).expect("switch");
        assert_eq!(switch_targets(&d, sw.0, &sw.1), vec![27, 27, 28]);
        assert_eq!(len, 29, "5 + 5 + 17 + 1 + 1");
        assert_eq!(d.code.len(), 32, "the code region is zero-padded to a multiple of 4 (D.1)");
        assert!(r_codes(&d).is_empty(), "a legal switch verifies clean: {:?}", r_reports(&d));
        // and the disassembly prints the table, so a reviewer can read the jump targets
        let hx = |v: u32| format!("{v:#06x}");
        let want = format!("switch 3 [{}, {}, {}]", hx(27), hx(27), hx(28));
        assert!(d.disassemble().contains(&want), "{want} missing from\n{}", d.disassemble());
    }

    #[test]
    fn a_switch_target_off_a_boundary_is_r0704() {
        // `push.b`@0(5) · `ldl`@5(5) · `switch[2]`@10(13) · `nop`@23(1) · `ret`@24(1); the case 14
        // sits inside the switch's own table, so it is not an instruction boundary.
        let m = one_func(
            1,
            &[
                (Op::PushB, vec![1]),
                (Op::Ldl, vec![0]),
                (Op::Switch, vec![14, 23]),
                (Op::Nop, vec![]),
                (Op::Ret, vec![]),
            ],
            TyKind::Int,
        );
        assert!(r_codes(&m).contains(&"R-0704"), "{:?}", r_reports(&m));
    }

    #[test]
    fn an_absurd_switch_count_is_refused_at_emit_time() {
        let mut b = CodeBuilder::new(0);
        b.set_locals(1);
        let targets = vec![0u32; (MAX_SWITCH_CASES + 1) as usize];
        let err = b.switch(&targets, None).expect_err("beyond the loader's cap must be refused");
        match err {
            IfbcEncodeErr::SwitchCaseCountTooLarge { given } => {
                assert_eq!(given, MAX_SWITCH_CASES as usize + 1)
            }
            other => panic!("wrong refusal: {other}"),
        }
        // exactly at the cap it is emitted, and the table is part of the instruction
        let at_cap = vec![0u32; MAX_SWITCH_CASES as usize];
        let off = b.switch(&at_cap, None).expect("at the cap");
        assert_eq!(b.offset() - off, 1 + 4 + 4 * MAX_SWITCH_CASES as u32);
    }

    /// D-38 (a): `func_table` is 48 bytes/record and the unit budget round-trips.
    #[test]
    fn func_records_are_48_bytes_and_max_mem_bytes_round_trips() {
        let mut m = one_func(1, &[(Op::PushB, vec![1]), (Op::Ret, vec![])], TyKind::Int);
        m.max_mem_bytes = 1 << 40;
        m.seal();
        let img = m.encode();
        // 47 header + (4 + 18*32) types + (4 + 0) imports -> func_table count
        let func_at = HEADER_LEN + 4 + N_SCALAR_ROWS * 32 + 4;
        assert_eq!(u64::from_be_bytes(img[func_at + 4..func_at + 12].try_into().unwrap()), 1 << 40);
        let d = decode(&img).expect("decodes");
        assert_eq!(d.max_mem_bytes, 1 << 40);
        let f = &d.funcs[0];
        assert_eq!(f.nargs, 0);
        assert_eq!(f.ret_sig, TyKind::Int.sig_code() as u32, "ret_sig is a scalar code, not a row index");
        assert_eq!(f.ret_arity, 1);
        assert_eq!(f.flags, F_ENTRY);
        assert_eq!(f.frame_kind, 0);
        assert!(f.max_stack >= 1, "the builder tracked the pushed value: {}", f.max_stack);
        // the record really is 48 bytes: the code region starts right after count + budget + 1 row
        let code_at = func_at + 4 + 8 + 48;
        assert_eq!(
            u32::from_be_bytes(img[code_at..code_at + 4].try_into().unwrap()) as usize,
            d.code.len()
        );
    }

    /// D-38 (a) put `max_stack` on the wire; item 2 then means the same thing it does in C: a height
    /// above the declared ceiling is R-0705, and the pair (too small / declared enough) is the test.
    #[test]
    fn a_height_above_the_declared_max_stack_is_r0705() {
        let body = [
            (Op::PushB, vec![1u32]),
            (Op::PushB, vec![2]),
            (Op::Add, vec![TyKind::Int as u32]),
            (Op::Ret, vec![]),
        ];
        let legal = one_func(1, &body, TyKind::Int);
        assert!(r_codes(&legal).is_empty(), "{:?}", r_reports(&legal));
        assert_eq!(legal.funcs[0].max_stack, 2, "two pushes before the binary op peaks at 2");

        let mut tight = one_func(1, &body, TyKind::Int);
        tight.funcs[0].max_stack = 1;
        tight.seal();
        assert!(r_codes(&tight).contains(&"R-0705"), "{:?}", r_reports(&tight));
    }

    /// D-38 (e): the `ldw` annotation is a *static* one, and only the three wide scalars exist.
    /// The pair is legal `i64`(7) vs illegal `bool`(2), which also proves the operand is read.
    #[test]
    fn ldw_annotation_must_be_i64_u64_or_f64() {
        let legal = one_func(
            1,
            &[(Op::Ldw, vec![0, TyKind::Int as u32]), (Op::Ret, vec![])],
            TyKind::Int,
        );
        assert!(r_codes(&legal).is_empty(), "{:?}", r_reports(&legal));
        let wrong = one_func(
            1,
            &[(Op::Ldw, vec![0, TyKind::Bool as u32]), (Op::Ret, vec![])],
            TyKind::Int,
        );
        assert!(r_codes(&wrong).contains(&"R-0706"), "{:?}", r_reports(&wrong));
    }

    /// D-38 (g) + §5's extras: `field_off ty,field` needs a `#[repr(C)]` row (else R-0713) and an
    /// in-range field index (else R-0706). The three-way pair pins both.
    #[test]
    fn field_off_needs_repr_c_and_an_in_range_field_index() {
        let mk = |repr_c: bool, field: u32| -> IfbcModule {
            let mut m = IfbcModule::new_unit("CPY-313", 1, "u", "s.py").unwrap();
            let st = m.intern_type(TyKind::Instance, "S", NONE32, NONE32);
            if repr_c {
                m.set_type_repr_c(st);
            }
            m.add_type_field(st, "a", 0, TyKind::Int as u32);
            m.add_type_field(st, "b", 8, TyKind::Int as u32);
            let (off, len, ms) =
                build_into(&mut m, 1, &[(Op::FieldOff, vec![st, field]), (Op::Ret, vec![])]);
            m.push_func("u", off, len, 1, ms, &[], TyKind::Int, 0, F_ENTRY);
            m.seal();
            m
        };
        let ok = mk(true, 1);
        assert!(r_codes(&ok).is_empty(), "{:?}", r_reports(&ok));
        let not_c = mk(false, 1);
        assert!(r_codes(&not_c).contains(&"R-0713"), "{:?}", r_reports(&not_c));
        let too_far = mk(true, 2);
        assert!(r_codes(&too_far).contains(&"R-0706"), "{:?}", r_reports(&too_far));
    }

    /// D-22's adjudication reaches this side too: a load-time "boundary" finding is R-0707, and
    /// R-0709 belongs only to the run-time arena check. The tailcall frame comparison is the C
    /// loader's set (locals / max_stack / nargs / ret_arity / frame_kind).
    #[test]
    fn a_tailcall_frame_mismatch_is_r0707_never_r0709() {
        let mut m = IfbcModule::new_unit("CPY-313", 1, "u", "s.py").unwrap();
        let (o1, l1, s1) = build_into(&mut m, 1, &[(Op::PushB, vec![1]), (Op::Ret, vec![])]);
        m.push_func("callee", o1, l1, 1, s1, &[], TyKind::Int, 0, 0);
        // the caller declares a different frame size (3 locals against the callee's 1), so the
        // tailcall is not frame-compatible
        let (o2, l2, s2) = build_into(&mut m, 3, &[(Op::PushB, vec![1]), (Op::TailCallStat, vec![0])]);
        m.push_func("caller", o2, l2, 3, s2, &[], TyKind::Int, 0, F_ENTRY);
        m.seal();
        let reports = r_reports(&m);
        assert!(
            reports.iter().any(|s| s.contains("帧尺寸不兼容")),
            "the frame comparison itself must fire: {reports:?}"
        );
        let codes = r_codes(&m);
        assert!(codes.contains(&"R-0707"), "{codes:?}");
        assert!(!codes.contains(&"R-0709"), "R-0709 is the arena code only (D-22): {:?}", codes);
    }

    /// D-38 (a) put `ret_arity` in the record, so `ret`/`ret.multi` are checked against it here
    /// exactly as `infy_ifbc.c::check_frame` does. The pair is a 2-tuple returned under a scalar
    /// declaration against the same code with the declaration fixed.
    #[test]
    fn return_shape_must_match_the_declared_ret_arity() {
        let body = [(Op::PushB, vec![1]), (Op::PushB, vec![2]), (Op::RetMulti, vec![2])];
        let m = one_func(1, &body, TyKind::Int);
        assert_eq!(m.funcs[0].ret_arity, 1, "a scalar return declaration");
        assert!(r_codes(&m).contains(&"R-0707"), "{:?}", r_reports(&m));

        let mut ok = one_func(1, &body, TyKind::Int);
        ok.funcs[0].ret_arity = 2;
        ok.seal();
        assert!(!r_codes(&ok).contains(&"R-0707"), "{:?}", r_reports(&ok));
    }

    fn find_instr(m: &IfbcModule, op: Op) -> usize {
        for i in 0..m.funcs.len() {
            for (off, ins) in instructions_of(m, i).unwrap() {
                if ins.op == op {
                    return off as usize;
                }
            }
        }
        panic!("no {op:?} in the sample")
    }

    /// §7.8.1 gives the header `frame_kind` one legal value (0 = single-address stack + local
    /// window) while D.8 needs a *per-function* coroutine kind, so the two levels have different
    /// value sets. `runtime/infy_ifbc.c` rejects a header value != 0 with R-0703 and a function
    /// value > 1 the same way; deviation D-23 fixes `1 = coroutine`.
    #[test]
    fn frame_kind_header_and_function_value_sets() {
        let m = sample();
        let mut img = m.encode();
        assert_eq!(img[HEADER_LEN - 1], FRAME_KIND_STACK, "header defaults to the 0 value model");
        img[HEADER_LEN - 1] = FRAME_KIND_COROUTINE;
        assert!(
            matches!(decode(&img), Err(IfbcDecodeErr::BadFrameKind(1))),
            "a module header may not claim the coroutine value model"
        );

        let mut ok = sample();
        ok.funcs[0].frame_kind = FRAME_KIND_COROUTINE as u32;
        ok.seal();
        assert!(matches!(decode(&ok.encode()), Ok(_)), "1 = coroutine is legal per function (D.8)");

        let mut bad = sample();
        bad.funcs[0].frame_kind = (FRAME_KIND_COROUTINE + 1) as u32;
        bad.seal();
        assert!(
            matches!(decode(&bad.encode()), Err(IfbcDecodeErr::BadFrameKind(2))),
            "the per-function value set is closed at {{0, 1}}"
        );
    }

    #[test]
    fn module_hash_covers_normalised_contents() {
        let mut m = sample();
        let h = m.module_hash;
        assert_eq!(h, m.compute_module_hash());
        m.intern("one byte more into the pool");
        assert_ne!(h, m.compute_module_hash(), "§7.3 reproducibility + tamper evidence");
    }

    #[test]
    fn disassembly_is_byte_stable() {
        let a = sample().encode();
        let d1 = disassemble(&a);
        let d2 = disassemble(&a);
        assert_eq!(d1, d2, "infy ifbc dump must be byte-stable (§12.3 golden)");
        for want in ["div.i64", "guard.nonzero", "is.dyn i64", "tickloop", "lang_tag=CPY-313"] {
            assert!(d1.contains(want), "missing {want} in\n{d1}");
        }
        assert!(d1.contains("InfyDynTag=1"), "the Int row must show it *is* DY_INT:\n{d1}");
        // D-38's merged shapes print too, so a reviewer can compare against the C dumper
        for want in ["nfields=0", "signed=true", "max_stack=", "ret_sig=dyn", "(各区自定界, D-38)"] {
            assert!(d1.contains(want), "missing {want} in\n{d1}");
        }
    }

    #[test]
    fn lang_tag_scheme_matches_7_8_1() {
        assert_eq!(lang_tag_of("CPY-313"), Some(0x010d));
        assert_eq!(lang_tag_of("LUA-54"), Some(0x0236));
        assert_eq!(lang_tag_name(0x010d), Some("CPY-313"));
        assert_eq!(lang_tag_name(0x0236), Some("LUA-54"));
        assert_eq!(lang_tag_of("CPY-999"), None);
        assert_eq!(
            LangTag { tag: lang_tag_of("CPY-313").unwrap(), cap_version: 1 }.cap_string(),
            "CPY-313:1"
        );
    }

    #[test]
    fn pool_interns_by_first_appearance() {
        let mut m = IfbcModule::new_unit("CPY-313", 1, "u", "s").unwrap();
        let a = m.intern("second");
        let b = m.intern("first");
        assert_eq!(m.intern("second"), a, "re-interning never moves an entry (§7.3)");
        assert!(b > a);
        // slots 0/1 are unit/source, then the 18 seeded scalar names, then these two
        assert_eq!(m.strings.len(), 2 + N_SCALAR_ROWS + 2);
    }

}
