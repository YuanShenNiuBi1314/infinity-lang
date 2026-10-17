//! Stage [7] of §7.1 ("IFIR + optimise"): the typed intermediate representation that is the
//! **only interface** between the front end ([`sema`](crate::sema)) and [`codegen`](crate::codegen).
//!
//! Field names, sizes, alignments and offsets here are fixed by
//! `docs/internals/runtime-abi.md` (IFABI-2), **not** by [`ast`](crate::ast): the AST is
//! surface-shaped on purpose (§4.11, §6.5), while the HIR must already be representational.
//! Every number is cross-checked against `gcc` measurements of the frozen
//! [`runtime/infy_rt.h`](../../runtime/infy_rt.h) structures (see the `abi_rows` tests and
//! `docs/RUNLOG.md` cron-07).
//!
//! ## What this stage covers today, and what it deliberately does not
//!
//! * covered: `fn` / `event` / `struct` / `enum` / `const` / `static` / `chain` (incl. the §8.4
//!   node table and the chain frame layout), the §4.6 `env` offset table, the §1 vtable prefix,
//!   and the §2 symbol mangling — i.e. everything §14's tour declares;
//! * **not** covered: statement/expression lowering. Function bodies are still the AST's
//!   [`Block`](crate::ast::Block); the back end that consumes them is C2, and until it exists no
//!   `infy build` path may claim a HIR body exists. This is recorded in `docs/coverage.md`.
//!
//! Layout failures are errors, never defaults: an infinite-size by-value cycle reports **E-0175**
//! (§4.15), a residual type parameter reports `MonomorphRequired`, and `!` has no layout.

use std::collections::BTreeMap;

use crate::ast::{Prim, StmtKind, Type, TypeKind, Visibility};
use crate::sema::{ConstValue, Program, ResolvedItem, TestKind, Ty};
use crate::span::FileId;

// --------------------------------------------------------------------------- targets

/// The target whose numbers `docs/internals/runtime-abi.md` fixes. §1 makes pointer width and
/// `__int128`'s alignment target-probed ("the compiler emits the probed value"), so they are
/// fields rather than constants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    /// `sizeof(void*)` — also the width of `usize`/`isize` (§2.8, recorded in `.inf meta`).
    pub pointer_width: u64,
    /// `_Alignof(__int128)`: 8 on the ABI reference target, 16 on GCC ≥ 13.
    pub u128_align: u64,
}

impl Target {
    /// The reference target of IFABI-2: 64-bit little-endian.
    pub const REFERENCE: Target = Target { pointer_width: 8, u128_align: 8 };
    /// 32-bit target: pointers shrink, but `uint64_t` fields in `InfyString`/`InfyVec` do not.
    pub const WASM32: Target = Target { pointer_width: 4, u128_align: 8 };
    /// What `gcc` on this machine's ucrt64 toolchain probed (cron-07): `__int128` at align 16.
    pub const X86_64_GCC13: Target = Target { pointer_width: 8, u128_align: 16 };

    fn ptr(self) -> Layout {
        Layout::new(self.pointer_width, self.pointer_width)
    }
    /// `u64` fields are fixed-width and independent of the target.
    fn u64f(self) -> Layout {
        Layout::new(8, 8)
    }
    fn u32f(self) -> Layout {
        Layout::new(4, 4)
    }
    /// §1: `str`/`[T]` are `(ptr, len)`; `String`/`Vec<T>` are `(ptr, len, cap)`.
    fn str_like(self) -> Layout {
        record_layout(&[self.ptr(), self.u64f()]).layout
    }
    fn string_like(self) -> Layout {
        record_layout(&[self.ptr(), self.u64f(), self.u64f()]).layout
    }
    /// §1: `Rc<T>`/`Arc<T>` = `{ data; strong }`, `closure` = `{ fn; env }`, `dyn` = `{ data; vt }`.
    fn two_ptrs(self) -> Layout {
        record_layout(&[self.ptr(), self.ptr()]).layout
    }
}

impl Default for Target {
    /// §1's table is the "@64" reference target, so that is also the default.
    fn default() -> Target {
        Target::REFERENCE
    }
}

// --------------------------------------------------------------------------- layout

/// A size/alignment pair — the thing §1's table and every `.infib` observable reduces to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub size: u64,
    pub align: u64,
}

impl Layout {
    pub const fn new(size: u64, align: u64) -> Layout {
        Layout { size, align }
    }
    /// §1 `()`: zero-sized types are `InfyUnit` (1/1), never "nothing", so `&T` on a ZST stays
    /// well-formed.
    pub const UNIT: Layout = Layout::new(1, 1);
}

pub fn align_up(value: u64, align: u64) -> u64 {
    if align <= 1 {
        value
    } else {
        (value + align - 1) / align * align
    }
}

/// A laid-out record: one offset per field plus the record's own layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub layouts: Vec<Layout>,
    pub offsets: Vec<u64>,
    pub layout: Layout,
}

/// C struct rules (§1: fields in declaration order, `#[repr(Rust)]` keeps the same order):
/// each field starts at `align_up(offset, field.align)`, the record's alignment is the maximum,
/// and the size is padded to that alignment. An empty record is 0/1 — the caller decides whether
/// that means `()`.
pub fn record_layout(fields: &[Layout]) -> Record {
    let mut offsets = Vec::with_capacity(fields.len());
    let mut cursor = 0u64;
    let mut align = 1u64;
    for f in fields {
        align = align.max(f.align);
        cursor = align_up(cursor, f.align.max(1));
        offsets.push(cursor);
        cursor += f.size;
    }
    Record { layouts: fields.to_vec(), offsets, layout: Layout::new(align_up(cursor, align), align) }
}

/// C union rules (§1 `enum`'s payload): every member is at offset 0, size/align are the maxima.
pub fn union_layout(variants: &[Layout]) -> Layout {
    let mut out = Layout::new(0, 1);
    for v in variants {
        out.size = out.size.max(v.size);
        out.align = out.align.max(v.align);
    }
    out.size = align_up(out.size, out.align);
    out
}

/// `#[align(N)]` raises the alignment (and therefore the tail padding) of a record.
fn apply_align_attr(base: Layout, align_attr: Option<u64>) -> Layout {
    match align_attr {
        Some(a) if a > base.align => Layout { size: align_up(base.size, a), align: a },
        _ => base,
    }
}

// --------------------------------------------------------------------------- errors

/// Why a layout could not be computed. These are hard errors: stage [7] never substitutes a
/// guess for a size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HirError {
    /// §4.15 / spec:757 — a by-value cycle (`struct L { next: L }`) has no finite size.
    InfiniteSize { type_name: String, cycle: String },
    /// A name that never resolved to a `struct`/`enum`/alias. `sema` reports the missing
    /// declaration itself (`E-0101`/`E-0131`); this is stage [7]'s refusal to continue.
    UnresolvedType { type_name: String },
    /// §4.15 monomorphisation happens before this stage; a residual type parameter has no size.
    MonomorphRequired { type_name: String },
    /// `!`, `auto` and parse-error placeholders are not storable types.
    Unrepresentable { type_text: String },
    /// `[T; N]` where `N` was not a plain integer literal (§2.12's closed const-eval set).
    UnsizedArrayLength { type_text: String },
    /// §7.4 — `#![entry_chain]` named a chain that does not exist (cli reports the same code).
    UnknownEntryChain { name: String },
    /// D-25: the tag is `uint32_t` for every enum, so a discriminant that does not fit is out of
    /// range for its type (§2.8's rule, §9.3's code) rather than silently truncated.
    DiscriminantOutOfRange { type_name: String, variant: String, value: u64 },
}

impl HirError {
    /// The §9.3 code the compiler must print, or `None` for an implementation-stage invariant.
    /// `None` still fails the build — it is never downgraded to a warning.
    pub fn spec_code(&self) -> Option<&'static str> {
        match self {
            HirError::InfiniteSize { .. } => Some("E-0175"),
            HirError::UnknownEntryChain { .. } => Some("E-0800"),
            HirError::DiscriminantOutOfRange { .. } => Some("E-0042"),
            _ => None,
        }
    }

    pub fn message(&self) -> String {
        match self {
            HirError::InfiniteSize { type_name, cycle } => {
                format!("类型 `{type_name}` 的尺寸无限（按值自引用）：{cycle}（§4.15：插入 `Box`）")
            }
            HirError::UnresolvedType { type_name } => {
                format!("类型 `{type_name}` 未解析到 struct/enum/别名，无法确定布局")
            }
            HirError::MonomorphRequired { type_name } => {
                format!("泛型参数 `{type_name}` 未单态化（§4.15），无法确定布局")
            }
            HirError::Unrepresentable { type_text } => {
                format!("`{type_text}` 不是可存储类型，没有布局")
            }
            HirError::UnsizedArrayLength { type_text } => {
                format!("`{type_text}` 的数组长度不是字面量，stage [7] 无法求值（§2.12）")
            }
            HirError::UnknownEntryChain { name } => format!("入口链不存在：`{name}`（§8.2）"),
            HirError::DiscriminantOutOfRange { type_name, variant, value } => format!(
                "`{type_name}::{variant}` 的判别式 {value} 超出标签类型 u32 的范围（D-25：标签恒为 `uint32_t`）"
            ),
        }
    }
}

// --------------------------------------------------------------------------- layouter

/// Computes §1 layouts for the types `sema` produced. Holds the ADT names it is inside of so a
/// by-value cycle is caught instead of recursing forever.
pub struct Layouter<'a> {
    pub target: Target,
    prog: &'a Program,
    stack: Vec<String>,
}

impl<'a> Layouter<'a> {
    pub fn new(target: Target, prog: &'a Program) -> Layouter<'a> {
        Layouter { target, prog, stack: Vec::new() }
    }

    pub fn layout(&mut self, ty: &Ty) -> Result<Layout, HirError> {
        Ok(match ty {
            Ty::Prim(p) => self.prim(*p)?,
            Ty::Bool => Layout::new(1, 1),
            Ty::Unit => Layout::UNIT,
            Ty::Str => self.target.str_like(),
            Ty::String => self.target.string_like(),
            // §1: `&T`, `&mut T`, `*const T`, `*mut T`, `Box<T>` and `fn(..)` are all pointers.
            Ty::Ref { .. } | Ty::Ptr { .. } | Ty::Boxed { .. } | Ty::Fn { .. } => self.target.ptr(),
            Ty::Array { inner, n } => {
                let e = self.layout(inner)?;
                Layout::new(e.size * *n, e.align)
            }
            Ty::Slice { .. } => self.target.str_like(),
            Ty::Vec { .. } => self.target.string_like(),
            Ty::Rc { .. } => self.target.two_ptrs(),
            Ty::Tuple(items) if items.is_empty() => Layout::UNIT,
            // Deviation D-24: the spec fixes no tuple representation; this backend lays a tuple
            // out as a C struct in element order.
            Ty::Tuple(items) => self.record(items)?.layout,
            Ty::Option { inner } => {
                // §1 niche: for pointer-shaped `T` the `Option` *is* the pointer (NULL == None).
                if is_niche_pointer(inner) {
                    self.target.ptr()
                } else {
                    let i = self.layout(inner)?;
                    record_layout(&[i, Layout::new(1, 1)]).layout
                }
            }
            // §1: `{ T ok; E err; uint8_t is_ok; }` — C padding rules, so the shorthand
            // "sizeof(T)+sizeof(E)+1, aligned" only holds when both operands share an alignment.
            Ty::Result { ok, err } => {
                let a = self.layout(ok)?;
                let b = self.layout(err)?;
                record_layout(&[a, b, Layout::new(1, 1)]).layout
            }
            Ty::Closure(_) => self.target.two_ptrs(),
            Ty::Dyn(_) => self.target.two_ptrs(),
            Ty::Named(name) | Ty::Adt(name) => self.adt(name)?,
            Ty::Generic(name) => return Err(HirError::MonomorphRequired { type_name: name.clone() }),
            Ty::IntLit | Ty::Auto | Ty::Never | Ty::Invalid => {
                return Err(HirError::Unrepresentable { type_text: ty.text() })
            }
        })
    }

    /// Layout a field list, returning per-field offsets (§4.8's `offset_of`, §1's struct rows).
    pub fn record(&mut self, fields: &[Ty]) -> Result<Record, HirError> {
        let mut layouts = Vec::with_capacity(fields.len());
        for f in fields {
            layouts.push(self.layout(f)?);
        }
        Ok(record_layout(&layouts))
    }

    fn prim(&self, p: Prim) -> Result<Layout, HirError> {
        Ok(match p {
            Prim::U8 | Prim::I8 => Layout::new(1, 1),
            Prim::U16 | Prim::I16 => Layout::new(2, 2),
            Prim::U32 | Prim::I32 => Layout::new(4, 4),
            Prim::U64 | Prim::I64 => Layout::new(8, 8),
            Prim::U128 | Prim::I128 => Layout::new(16, self.target.u128_align),
            Prim::F32 => Layout::new(4, 4),
            Prim::F64 => Layout::new(8, 8),
            Prim::Bool => Layout::new(1, 1),
            Prim::Char => Layout::new(4, 4),
            Prim::Unit => Layout::UNIT,
            Prim::Str => self.target.str_like(),
            Prim::Usize | Prim::Isize => self.target.ptr(),
        })
    }

    fn adt(&mut self, name: &str) -> Result<Layout, HirError> {
        if let Some(alias) = self.prog.aliases.get(name) {
            let alias = alias.clone();
            return self.layout(&alias);
        }
        if self.stack.iter().any(|s| s == name) {
            let mut cycle = self.stack.clone();
            cycle.push(name.to_string());
            return Err(HirError::InfiniteSize { type_name: name.to_string(), cycle: cycle.join(" → ") });
        }
        if let Some(i) = self.prog.struct_by_name(name) {
            let (fields, align_attr) = {
                let s = &self.prog.structs[i];
                (s.fields.iter().map(|(_, t, _)| t.clone()).collect::<Vec<Ty>>(), s.align.map(|a| a as u64))
            };
            if fields.is_empty() {
                // §1: a fieldless struct is a zero-sized type, and ZSTs are `InfyUnit` (1/1).
                return Ok(apply_align_attr(Layout::UNIT, align_attr));
            }
            self.stack.push(name.to_string());
            let r = self.record(&fields);
            self.stack.pop();
            return Ok(apply_align_attr(r?.layout, align_attr));
        }
        if let Some(i) = self.prog.enum_by_name(name) {
            self.stack.push(name.to_string());
            let r = self.enum_layout_of(i);
            self.stack.pop();
            return r;
        }
        Err(HirError::UnresolvedType { type_name: name.to_string() })
    }

    /// §1 `enum`: `{ uint32_t tag; union { ... } v; }`. Deviation D-24: when **no** variant
    /// carries a payload the union is elided, so a fieldless enum is exactly its `u32` tag (4/4).
    fn enum_layout_of(&mut self, i: usize) -> Result<Layout, HirError> {
        let tag = self.target.u32f();
        let recs = self.enum_payload_records(i)?;
        let payloads: Vec<Layout> = recs.iter().filter(|r| !r.offsets.is_empty()).map(|r| r.layout).collect();
        if payloads.is_empty() {
            return Ok(tag);
        }
        let u = union_layout(&payloads);
        Ok(record_layout(&[tag, u]).layout)
    }

    /// The same computation, but keeping each variant's own payload record (for [`HirEnum`]).
    /// A variant with no payload yields an empty record (0/1) and takes no union slot.
    fn enum_payload_records(&mut self, i: usize) -> Result<Vec<Record>, HirError> {
        let variants = self.prog.enums[i].variants.clone();
        let mut out = Vec::with_capacity(variants.len());
        for (_, tys, _) in variants.iter() {
            out.push(self.record(tys)?);
        }
        Ok(out)
    }
}

/// §1: the `Option` niche applies to `&T`, `Box<U>`, `*const U` and function pointers.
fn is_niche_pointer(ty: &Ty) -> bool {
    matches!(ty, Ty::Ref { .. } | Ty::Ptr { .. } | Ty::Boxed { .. } | Ty::Fn { .. })
}

// --------------------------------------------------------------------------- items

/// §4.8: `#[repr(C)]` is what makes a layout contract with the source; `repr(Rust)` may be
/// reordered by a future back end (this one does not reorder, but the source still may not
/// depend on it — and `offset_of` is only legal for `repr(C)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repr {
    Rust,
    C,
}

#[derive(Clone, Debug)]
pub struct HirField {
    pub name: String,
    pub ty: Ty,
    pub layout: Layout,
    pub offset: u64,
    /// §5.5: `pub`/`pub(crate)`/`pub(path)` fields are visible across package boundaries.
    pub exported: bool,
}

#[derive(Clone, Debug)]
pub struct HirStruct {
    pub name: String,
    /// Declaring package/module path, used by the §2 symbol rule.
    pub path: String,
    pub symbol: String,
    pub repr: Repr,
    pub is_copy: bool,
    pub has_drop: bool,
    pub fields: Vec<HirField>,
    pub layout: Layout,
}

impl HirStruct {
    /// §4.8: `std.mem.offset_of::<T, f>()` is only available for `#[repr(C)]` types.
    pub fn offset_of(&self, field: &str) -> Option<u64> {
        if self.repr != Repr::C {
            return None;
        }
        self.fields.iter().find(|f| f.name == field).map(|f| f.offset)
    }
}

#[derive(Clone, Debug)]
pub struct HirVariant {
    pub name: String,
    /// §4.1 explicit discriminant (`Fetch = 3`), otherwise declaration order.
    pub discriminant: u64,
    /// Payload fields at offsets **relative to the payload start**.
    pub payload: Vec<HirField>,
    pub payload_layout: Layout,
    /// Offset of this variant's payload inside the enum value (the union's offset; 0 when the
    /// enum has no payload at all).
    pub payload_offset: u64,
}

#[derive(Clone, Debug)]
pub struct HirEnum {
    pub name: String,
    pub path: String,
    pub symbol: String,
    pub repr: Repr,
    pub tag: Layout,
    pub variants: Vec<HirVariant>,
    pub layout: Layout,
}

impl HirEnum {
    pub fn variant(&self, name: &str) -> Option<&HirVariant> {
        self.variants.iter().find(|v| v.name == name)
    }
}

#[derive(Clone, Debug)]
pub struct HirParam {
    pub name: String,
    pub ty: Ty,
    pub layout: Layout,
}

/// `None` means the function returns `()` (§1: an `InfyUnit`, 1/1, never "nothing"). A `!` return
/// carries no value at all, so `ret` is `None` and [`HirFn::diverges`] is set.
#[derive(Clone, Debug)]
pub struct HirFn {
    pub name: String,
    pub package: String,
    pub symbol: String,
    /// §3.3 `event` vs `fn`.
    pub is_event: bool,
    /// `;`-bodied: A-track `extern` or a host-provided event (§3.3, §7.6).
    pub is_foreign: bool,
    pub is_unsafe: bool,
    pub must_use: bool,
    /// §5.9 `#[test]` / `#[bench]`.
    pub test_kind: Option<TestKind>,
    /// §7.6.5 `#[export("name")]`.
    pub export_symbol: Option<String>,
    /// §7.6.1: the `#[link(name = "…")]` unit this A-track declaration belongs to (`None` for
    /// everything else). `atrack` groups a wrapper's signature block by it.
    pub link_unit: Option<String>,
    /// Set for `impl` methods; the receiver is passed first.
    pub self_ty: Option<(Ty, Layout)>,
    pub params: Vec<HirParam>,
    pub ret: Option<(Ty, Layout)>,
    pub diverges: bool,
}

impl HirFn {
    /// §11 / ABI §2: the signature the A-track wrapper `infy_e_<name>_<8hex>` hashes.
    pub fn signature_text(&self) -> String {
        let mut s = String::new();
        s.push('(');
        for (i, p) in self.params.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&p.ty.text());
        }
        s.push(')');
        s.push_str("->");
        s.push_str(&match &self.ret {
            Some((t, _)) => t.text(),
            None if self.diverges => "!".to_string(),
            None => "()".to_string(),
        });
        s
    }
}

#[derive(Clone, Debug)]
pub struct HirConst {
    pub name: String,
    pub package: String,
    pub symbol: String,
    pub ty: Ty,
    pub layout: Layout,
    /// `None` = §2.12's closed const-eval set did not cover the initialiser; the back end must
    /// emit an initialisation path, it may not substitute a default.
    pub value: Option<ConstValue>,
    /// `static` (shared, possibly `mut`) rather than `const` (inlined per use, §4.5).
    pub is_static: bool,
}

/// One chain-local slot (§8.4: `let`/`var` at chain top level are a node's preamble, but they
/// live in the frame, not in the node).
#[derive(Clone, Debug)]
pub struct HirLocal {
    pub name: String,
    pub mutable: bool,
    pub ty: Ty,
    pub layout: Layout,
    pub offset: u64,
}

/// ABI §5's `ChainFrame_<name>`: the locals record followed by `node_idx`. The runtime only ever
/// sees `frame_size`/`frame_align` — it `infy_alloc`s that block and `memset`s it to 0
/// (`runtime/infy_rt.c:2057`), so offsets here are the whole contract.
#[derive(Clone, Debug)]
pub struct HirFrame {
    pub locals: Vec<HirLocal>,
    pub node_idx_offset: u64,
    pub size: u64,
    pub align: u64,
}

/// §8.4 rule 5's node table row: `chain_id, node_idx, file:line:col, linkage_count, host_event_id?`.
#[derive(Clone, Debug)]
pub struct HirNode {
    pub idx: u32,
    pub file: FileId,
    pub line: u32,
    pub col: u32,
    pub linkage_count: u32,
    /// Callee name of the node's host statement, as resolved by `sema`.
    pub host_event: Option<String>,
    /// Set only when that callee **is** a declared `event` (§11 EventTable index).
    pub event_id: Option<u32>,
    pub anchor: Option<String>,
}

#[derive(Clone, Debug)]
pub struct HirChain {
    pub name: String,
    pub package: String,
    pub symbol: String,
    pub frame: HirFrame,
    pub nodes: Vec<HirNode>,
    /// Named anchors (`goto("anchor")`, §8.2) → node index.
    pub labels: BTreeMap<String, u32>,
}

#[derive(Clone, Debug)]
pub enum HirItem {
    Struct(HirStruct),
    Enum(HirEnum),
    Fn(HirFn),
    Const(HirConst),
    Chain(HirChain),
}

impl HirItem {
    pub fn symbol(&self) -> &str {
        match self {
            HirItem::Struct(s) => &s.symbol,
            HirItem::Enum(e) => &e.symbol,
            HirItem::Fn(f) => &f.symbol,
            HirItem::Const(c) => &c.symbol,
            HirItem::Chain(c) => &c.symbol,
        }
    }
    pub fn name(&self) -> &str {
        match self {
            HirItem::Struct(s) => &s.name,
            HirItem::Enum(e) => &e.name,
            HirItem::Fn(f) => &f.name,
            HirItem::Const(c) => &c.name,
            HirItem::Chain(c) => &c.name,
        }
    }
    pub fn layout(&self) -> Option<Layout> {
        match self {
            HirItem::Struct(s) => Some(s.layout),
            HirItem::Enum(e) => Some(e.layout),
            HirItem::Const(c) => Some(c.layout),
            _ => None,
        }
    }
}

/// §7.4 entry rules: `#![entry_chain]` > `fn main` > first chain of the main package.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HirEntry {
    Chain(usize),
    Main(usize),
}

/// Input the front end must supply beyond `sema::Program`: the §7.4 entry declaration lives in a
/// file attribute, which `Program` does not carry.
#[derive(Clone, Debug, Default)]
pub struct HirConfig {
    pub target: Target,
    /// `#![entry_chain = "…"]`.
    pub entry_chain: Option<String>,
}

#[derive(Clone, Debug)]
pub struct HirCrate {
    pub target: Target,
    /// Deterministic order (types, then values, then functions, then chains), so the emitted C
    /// text and the §7.3 build-id are byte-stable for identical input.
    pub items: Vec<HirItem>,
    /// §11 EventTable: indices into `items` holding `event`s, in declaration order. The position
    /// in this vector **is** the `host_event_id`.
    pub events: Vec<usize>,
    pub env: EnvTable,
    pub entry: Option<HirEntry>,
    pub errors: Vec<HirError>,
}

impl HirCrate {
    pub fn item(&self, i: usize) -> Option<&HirItem> {
        self.items.get(i)
    }

    pub fn struct_by_name(&self, name: &str) -> Option<&HirStruct> {
        self.items.iter().find_map(|it| match it {
            HirItem::Struct(s) if s.name == name => Some(s),
            _ => None,
        })
    }
    pub fn enum_by_name(&self, name: &str) -> Option<&HirEnum> {
        self.items.iter().find_map(|it| match it {
            HirItem::Enum(e) if e.name == name => Some(e),
            _ => None,
        })
    }
    pub fn chain_by_name(&self, name: &str) -> Option<&HirChain> {
        self.items.iter().find_map(|it| match it {
            HirItem::Chain(c) if c.name == name => Some(c),
            _ => None,
        })
    }
    pub fn fn_by_name(&self, name: &str) -> Option<&HirFn> {
        self.items.iter().find_map(|it| match it {
            HirItem::Fn(f) if f.name == name => Some(f),
            _ => None,
        })
    }
    pub fn event_id(&self, name: &str) -> Option<u32> {
        self.events.iter().position(|i| self.items[*i].name() == name).map(|p| p as u32)
    }

    /// The same table as a record, so the per-slot offsets are reachable: `docs/internals/runtime-abi.md`
    /// §1 publishes them and a test reads that block back against here (D-50(c)).
    pub fn vtable_record(&self, n_methods: u64) -> Record {
        let mut fields = vec![self.target.ptr(), self.target.u64f(), self.target.u64f()];
        fields.extend((0..n_methods).map(|_| self.target.ptr()));
        record_layout(&fields)
    }

    /// ABI §1 vtable prefix: `drop`, `size`, `align`, then the method pointers in declaration
    /// order (§4.1: the vtable layout is fixed by IFABI).
    pub fn vtable_layout(&self, n_methods: u64) -> Layout {
        self.vtable_record(n_methods).layout
    }
}

/// §2: the *package* an item resolved into. `StructInfo`/`EnumInfo` only carry the manifest's
/// module path, so the package comes from `Program::by_path`; the module path is the fallback for
/// anything the resolution table does not list.
fn package_of(prog: &Program, want: &ResolvedItem, fallback: &str) -> String {
    prog.by_path
        .iter()
        .find(|(_, v)| **v == *want)
        .map(|((package, _), _)| package.clone())
        .unwrap_or_else(|| fallback.to_string())
}

/// §2: `infy_<mangled package path>_<item>`, mangled = non-alphanumerics replaced by `_`.
pub fn mangle(path: &str, item: &str) -> String {
    let mut out = String::from("infy_");
    for c in path.chars() {
        out.push(if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' });
    }
    out.push('_');
    out.push_str(item);
    out
}

// --------------------------------------------------------------------------- env (§4.6)

/// One `env.*` accessor: the §4.6 name, its type, and the compile-time offset codegen loads from
/// (§10: "compiles to a load at a compile-time offset recorded in `.inf meta`'s `EnvOffsets`").
#[derive(Clone, Debug)]
pub struct EnvField {
    pub name: &'static str,
    pub ty: Ty,
    pub layout: Layout,
    pub offset: u64,
}

/// The closed `env` field set (§1.3 rule 2 / §4.6): adding a field is a spec change, and an
/// implementation may not extend it (UB-024). Order and types mirror
/// `runtime/infy_rt.h`'s `InfyEnvSnapshot`, which is the frozen layout the host must agree with
/// (`R-0905` on mismatch, §11.2).
#[derive(Clone, Debug)]
pub struct EnvTable {
    pub fields: Vec<EnvField>,
    pub layout: Layout,
}

impl EnvTable {
    pub fn offset_of(&self, name: &str) -> Option<u64> {
        self.fields.iter().find(|f| f.name == name).map(|f| f.offset)
    }
    pub fn names(&self) -> Vec<&'static str> {
        self.fields.iter().map(|f| f.name).collect()
    }
}

/// Build `EnvOffsets` for a target. `env.platform`/`env.build_id` are `&str` in §4.6's table; the
/// snapshot stores the `InfyStr` **inline** (the host publishes the whole block, so a pointer
/// into host memory would not survive the node boundary).
pub fn env_table(target: Target) -> EnvTable {
    let u32f = target.u32f();
    let u64f = target.u64f();
    let f32f = Layout::new(4, 4);
    let strf = target.str_like();
    let spec: [(&'static str, Ty, Layout); 11] = [
        ("iter", Ty::Prim(Prim::U32), u32f),
        ("runtime_s", Ty::Prim(Prim::U32), u32f),
        ("realtime", Ty::Prim(Prim::U64), u64f),
        ("ram_mb", Ty::Prim(Prim::U32), u32f),
        ("cpu_use", Ty::Prim(Prim::F32), f32f),
        ("gpu_use", Ty::Prim(Prim::F32), f32f),
        ("chain_id", Ty::Prim(Prim::U32), u32f),
        ("node_idx", Ty::Prim(Prim::U32), u32f),
        ("focus_id", Ty::Prim(Prim::U32), u32f),
        ("platform", Ty::Str, strf),
        ("build_id", Ty::Str, strf),
    ];
    let layouts: Vec<Layout> = spec.iter().map(|(_, _, l)| *l).collect();
    let rec = record_layout(&layouts);
    EnvTable {
        fields: spec
            .iter()
            .enumerate()
            .map(|(i, (name, ty, l))| EnvField { name, ty: ty.clone(), layout: *l, offset: rec.offsets[i] })
            .collect(),
        layout: rec.layout,
    }
}

// --------------------------------------------------------------------------- ABI table rows

/// One field of a fixed ABI record, with its measured offset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowField {
    pub name: &'static str,
    pub layout: Layout,
    pub offset: u64,
}

fn build_row(fields: &[(&'static str, Layout)]) -> (Vec<RowField>, Layout) {
    let layouts: Vec<Layout> = fields.iter().map(|(_, l)| *l).collect();
    let rec = record_layout(&layouts);
    let row = fields
        .iter()
        .enumerate()
        .map(|(i, (name, l))| RowField { name, layout: *l, offset: rec.offsets[i] })
        .collect();
    (row, rec.layout)
}

/// §8.4 rule 5 / ABI §5: `InfyNodeLoc` — the row the node table and every §9.2 diagnostic use.
pub fn node_loc_row(target: Target) -> (Vec<RowField>, Layout) {
    build_row(&[
        ("file", target.ptr()),
        ("line", target.u32f()),
        ("col", target.u32f()),
        ("node_idx", target.u32f()),
        ("linkage_count", target.u32f()),
    ])
}

/// ABI §5: `InfyChainDesc` — what codegen emits per chain, and what `.inf meta`'s `ChainTable`
/// describes (§7.4).
pub fn chain_desc_row(target: Target) -> (Vec<RowField>, Layout) {
    build_row(&[
        ("name", target.ptr()),
        ("n_nodes", target.u32f()),
        ("entry_node", target.u32f()),
        ("frame_size", target.u32f()),
        ("frame_align", target.u32f()),
        ("nodes", target.ptr()),
        ("locs", target.ptr()),
        ("linkage_counts", target.ptr()),
    ])
}

/// §10: `InfyFocusId3`, the scheduler's identity triple.
pub fn focus_id_row(target: Target) -> (Vec<RowField>, Layout) {
    build_row(&[("chain_id", target.u32f()), ("node_idx", target.u32f()), ("focus_id", target.u32f())])
}

// --------------------------------------------------------------------------- lowering

/// Lower a checked program into the declaration-level HIR. Items that fail to lay out are
/// **omitted** and recorded in `errors` — the caller must treat a non-empty `errors` as a build
/// failure (§7.2: no silent degradation).
pub fn lower(prog: &Program, cfg: &HirConfig) -> HirCrate {
    let target = cfg.target;
    let mut items: Vec<HirItem> = Vec::new();
    let mut errors: Vec<HirError> = Vec::new();

    for (n, s) in prog.structs.iter().enumerate() {
        let mut l = Layouter::new(target, prog);
        let fields: Vec<Ty> = s.fields.iter().map(|(_, t, _)| t.clone()).collect();
        match l.record(&fields) {
            Ok(rec) => {
                // §1: a fieldless struct is a ZST and ZSTs are `InfyUnit` (1/1), never nothing.
                let base = if rec.offsets.is_empty() { Layout::UNIT } else { rec.layout };
                let layout = apply_align_attr(base, s.align.map(|a| a as u64));
                let fields = s
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(i, (name, ty, vis))| HirField {
                        name: name.clone(),
                        ty: ty.clone(),
                        layout: rec.layouts[i],
                        offset: rec.offsets[i],
                        exported: !matches!(vis, Visibility::Private | Visibility::Priv),
                    })
                    .collect();
                let package = package_of(prog, &ResolvedItem::Struct(n), &s.module);
                items.push(HirItem::Struct(HirStruct {
                    name: s.name.clone(),
                    path: package.clone(),
                    symbol: mangle(&package, &s.name),
                    repr: if s.repr_c { Repr::C } else { Repr::Rust },
                    is_copy: s.is_copy,
                    has_drop: s.has_drop,
                    fields,
                    layout,
                }));
            }
            Err(e) => errors.push(e),
        }
    }

    for (n, e) in prog.enums.iter().enumerate() {
        let mut l = Layouter::new(target, prog);
        match l.enum_payload_records(n) {
            Err(err) => errors.push(err),
            Ok(recs) => {
                let tag = target.u32f();
                let payloads: Vec<Layout> = recs.iter().filter(|r| !r.offsets.is_empty()).map(|r| r.layout).collect();
                let (layout, payload_offset) = if payloads.is_empty() {
                    (tag, 0u64)
                } else {
                    let rec = record_layout(&[tag, union_layout(&payloads)]);
                    (rec.layout, rec.offsets[1])
                };
                // D-25: the tag is `uint32_t` for every enum, so a discriminant that does not fit
                // is an out-of-range literal (§2.8), never a silent truncation.
                let mut fits = true;
                for (vname, _, disc) in e.variants.iter() {
                    if *disc > u32::MAX as u64 {
                        fits = false;
                        errors.push(HirError::DiscriminantOutOfRange {
                            type_name: e.name.clone(),
                            variant: vname.clone(),
                            value: *disc,
                        });
                    }
                }
                let variants = e
                    .variants
                    .iter()
                    .zip(recs.iter())
                    .map(|((vname, tys, disc), rec)| HirVariant {
                        name: vname.clone(),
                        discriminant: *disc,
                        // §1: tuple-variant payloads are addressed by position (`a0`, `a1`, …);
                        // struct-variant payloads were flattened by `sema` in declaration order.
                        payload: tys
                            .iter()
                            .enumerate()
                            .map(|(i, t)| HirField {
                                name: format!("a{i}"),
                                ty: t.clone(),
                                layout: rec.layouts[i],
                                offset: rec.offsets[i],
                                exported: true,
                            })
                            .collect(),
                        payload_layout: rec.layout,
                        payload_offset,
                    })
                    .collect();
                let package = package_of(prog, &ResolvedItem::Enum(n), &e.module);
                if !fits {
                    continue;
                }
                items.push(HirItem::Enum(HirEnum {
                    name: e.name.clone(),
                    path: package.clone(),
                    symbol: mangle(&package, &e.name),
                    repr: if e.repr_c { Repr::C } else { Repr::Rust },
                    tag,
                    variants,
                    layout,
                }));
            }
        }
    }

    for (c, is_static) in prog.consts.iter().map(|c| (c, false)).chain(prog.statics.iter().map(|c| (c, true))) {
        let mut l = Layouter::new(target, prog);
        match l.layout(&c.ty) {
            Ok(layout) => items.push(HirItem::Const(HirConst {
                name: c.name.clone(),
                package: c.package.clone(),
                symbol: mangle(&c.package, &c.name),
                ty: c.ty.clone(),
                layout,
                value: c.value,
                is_static,
            })),
            Err(e) => errors.push(e),
        }
    }

    for f in &prog.fns {
        let mut l = Layouter::new(target, prog);
        let mut params = Vec::new();
        let mut failed: Option<HirError> = None;
        let self_layout = match &f.self_ty {
            Some(t) => match l.layout(t) {
                Ok(x) => Some((t.clone(), x)),
                Err(e) => {
                    failed = Some(e);
                    None
                }
            },
            None => None,
        };
        if failed.is_none() {
            for (name, ty) in &f.params {
                match l.layout(ty) {
                    Ok(layout) => params.push(HirParam { name: name.clone(), ty: ty.clone(), layout }),
                    Err(e) => {
                        failed = Some(e);
                        break;
                    }
                }
            }
        }
        let mut ret = None;
        let mut diverges = false;
        if failed.is_none() {
            match &f.ret {
                ty @ (Ty::Never | Ty::Invalid | Ty::Unit) => {
                    diverges = matches!(ty, Ty::Never);
                }
                ty => match l.layout(ty) {
                    Ok(layout) => ret = Some((ty.clone(), layout)),
                    Err(e) => failed = Some(e),
                },
            }
        }
        match failed {
            Some(e) => errors.push(e),
            None => items.push(HirItem::Fn(HirFn {
                name: f.name.clone(),
                package: f.package.clone(),
                symbol: mangle(&f.package, &f.name),
                is_event: f.is_event,
                is_foreign: f.is_foreign,
                is_unsafe: f.is_unsafe,
                must_use: f.must_use,
                test_kind: f.test_kind,
                export_symbol: f.export_symbol.clone(),
                link_unit: f.link_unit.clone(),
                self_ty: self_layout,
                params,
                ret,
                diverges,
            })),
        }
    }

    for c in &prog.chains {
        let mut l = Layouter::new(target, prog);
        // §8.4 rule 1: chain-level `let`/`var` are a node's preamble but live in the frame.
        let mut local_tys: Vec<(String, bool, Ty)> = Vec::new();
        let mut err: Option<HirError> = None;
        for s in &c.stmts {
            if let StmtKind::Let { is_var, name, ty, init, .. } = &s.kind {
                let resolved = match (ty, init) {
                    (Some(t), _) => hir_ty_from_ast(t),
                    (None, Some(e)) => prog.ty(e.span).cloned(),
                    (None, None) => None,
                };
                match resolved {
                    Some(t) => local_tys.push((name.clone(), *is_var, t)),
                    None => {
                        err = Some(HirError::UnresolvedType { type_name: name.clone() });
                        break;
                    }
                }
            }
        }
        let tys: Vec<Ty> = local_tys.iter().map(|(_, _, t)| t.clone()).collect();
        let rec = match l.record(&tys) {
            Ok(r) => r,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        if let Some(e) = err {
            errors.push(e);
            continue;
        }
        let node_idx_off = align_up(rec.layout.size, 4);
        let frame = HirFrame {
            locals: local_tys
                .iter()
                .enumerate()
                .map(|(i, (name, mutable, ty))| HirLocal {
                    name: name.clone(),
                    mutable: *mutable,
                    ty: ty.clone(),
                    layout: rec.layouts[i],
                    offset: rec.offsets[i],
                })
                .collect(),
            node_idx_offset: node_idx_off,
            size: align_up(node_idx_off + 4, rec.layout.align.max(4)),
            align: rec.layout.align.max(4),
        };
        let nodes = c
            .nodes
            .iter()
            .map(|n| HirNode {
                idx: n.idx,
                file: c.file,
                line: n.line,
                col: n.col,
                linkage_count: n.linkage_count as u32,
                host_event: n.host_event.clone(),
                event_id: event_id_of(prog, &c.package, n.host_event.as_deref()),
                anchor: anchor_for(&c.labels, n.idx as usize),
            })
            .collect();
        items.push(HirItem::Chain(HirChain {
            name: c.name.clone(),
            package: c.package.clone(),
            symbol: mangle(&c.package, &c.name),
            frame,
            nodes,
            labels: c.labels.iter().map(|(k, v)| (k.clone(), *v as u32)).collect(),
        }));
    }

    let events: Vec<usize> = items.iter().enumerate().filter_map(|(i, it)| match it {
        HirItem::Fn(f) if f.is_event => Some(i),
        _ => None,
    }).collect();

    let entry = resolve_entry(prog, cfg, &items, &mut errors);
    HirCrate { target, items, events, env: env_table(target), entry, errors }
}

/// §7.4: `#![entry_chain]` beats `fn main`, which beats the first chain of the main package.
fn resolve_entry(prog: &Program, cfg: &HirConfig, items: &[HirItem], errors: &mut Vec<HirError>) -> Option<HirEntry> {
    if let Some(name) = &cfg.entry_chain {
        for (i, it) in items.iter().enumerate() {
            if let HirItem::Chain(c) = it {
                if c.name == *name {
                    return Some(HirEntry::Chain(i));
                }
            }
        }
        errors.push(HirError::UnknownEntryChain { name: name.clone() });
        return None;
    }
    for (i, it) in items.iter().enumerate() {
        if let HirItem::Fn(f) = it {
            if f.name == "main" && f.self_ty.is_none() && !f.is_event {
                return Some(HirEntry::Main(i));
            }
        }
    }
    let main_pkg = prog.package_order.first().cloned().unwrap_or_default();
    for (i, it) in items.iter().enumerate() {
        if let HirItem::Chain(c) = it {
            if c.package == main_pkg {
                return Some(HirEntry::Chain(i));
            }
        }
    }
    items.iter().enumerate().find_map(|(i, it)| match it {
        HirItem::Chain(_) => Some(HirEntry::Chain(i)),
        _ => None,
    })
}

/// A node's host statement names a callee; only a callee that *is* a declared `event` gets an id.
fn event_id_of(prog: &Program, package: &str, callee: Option<&str>) -> Option<u32> {
    let name = callee?;
    let fi = prog.lookup_fn(package, name).or_else(|| {
        (0..prog.fns.len()).find(|i| prog.fns[*i].name == name)
    })?;
    if prog.fns[fi].is_event {
        // The id is the position among events in *declaration order*, which is the order `lower`
        // walks `prog.fns`; recompute identically so the two never diverge.
        (0..=fi).filter(|i| prog.fns[*i].is_event).count().checked_sub(1).map(|x| x as u32)
    } else {
        None
    }
}

fn anchor_for(labels: &BTreeMap<String, usize>, idx: usize) -> Option<String> {
    labels.iter().find(|(_, v)| **v == idx).map(|(k, _)| k.clone())
}

/// Structural `ast::Type` → [`Ty`] for the places stage [7] needs a type but `sema` only recorded
/// one for an expression: chain-local declarations. Names are left as [`Ty::Named`] and resolved
/// by [`Layouter`] against `Program`, so this never duplicates `sema`'s alias/generics logic.
pub fn hir_ty_from_ast(t: &Type) -> Option<Ty> {
    Some(match &t.kind {
        TypeKind::Prim(p) => match p {
            Prim::Str => Ty::Str,
            Prim::Unit => Ty::Unit,
            Prim::Bool => Ty::Bool,
            other => Ty::Prim(*other),
        },
        TypeKind::Named { path, args } => {
            let base = path.last().cloned().unwrap_or_default();
            let inner: Vec<Ty> = match args.iter().map(hir_ty_from_ast).collect::<Option<Vec<_>>>() {
                Some(v) => v,
                None => return None,
            };
            match (base.as_str(), inner.len()) {
                ("Option", 1) => Ty::Option { inner: Box::new(inner[0].clone()) },
                ("Result", 2) => Ty::Result { ok: Box::new(inner[0].clone()), err: Box::new(inner[1].clone()) },
                ("Box", 1) => Ty::Boxed { inner: Box::new(inner[0].clone()) },
                ("Rc", 1) => Ty::Rc { inner: Box::new(inner[0].clone()), atomic: false },
                ("Arc", 1) => Ty::Rc { inner: Box::new(inner[0].clone()), atomic: true },
                ("Vec", 1) => Ty::Vec { inner: Box::new(inner[0].clone()) },
                ("String", 0) => Ty::String,
                ("str", 0) => Ty::Str,
                _ if inner.is_empty() => Ty::Named(base),
                _ => Ty::Named(format!("{base}<{}>", inner.iter().map(|t| t.text()).collect::<Vec<_>>().join(", "))),
            }
        }
        TypeKind::Ref { inner, mutable, lifetime } => {
            Ty::Ref { inner: Box::new(hir_ty_from_ast(inner)?), mutable: *mutable, lifetime: lifetime.clone() }
        }
        TypeKind::Ptr { inner, mutable } => {
            Ty::Ptr { inner: Box::new(hir_ty_from_ast(inner)?), mutable: *mutable }
        }
        TypeKind::Array { inner, len } => {
            let n = literal_len(len)?;
            Ty::Array { inner: Box::new(hir_ty_from_ast(inner)?), n }
        }
        TypeKind::Slice { inner } => Ty::Slice { inner: Box::new(hir_ty_from_ast(inner)?) },
        TypeKind::Tuple(v) if v.is_empty() => Ty::Unit,
        TypeKind::Tuple(v) => Ty::Tuple(match v.iter().map(hir_ty_from_ast).collect::<Option<Vec<_>>>() {
            Some(t) => t,
            None => return None,
        }),
        TypeKind::FnPtr { params, ret } => {
            let ps = match params.iter().map(hir_ty_from_ast).collect::<Option<Vec<_>>>() {
                Some(p) => p,
                None => return None,
            };
            let r = match ret.as_deref() {
                Some(t) => Some(Box::new(hir_ty_from_ast(t)?)),
                None => None,
            };
            Ty::Fn { params: ps, ret: r }
        }
        TypeKind::Dyn { path, .. } => Ty::Dyn(path.join("::")),
        TypeKind::Never => Ty::Never,
        TypeKind::Invalid => return None,
    })
}

/// §2.12: only a plain integer literal is const-evaluable at this stage (a computed length is
/// `sema`'s job and would have been an error there).
fn literal_len(e: &crate::ast::Expr) -> Option<u64> {
    match &e.kind {
        crate::ast::ExprKind::Lit(crate::token::Lit::Int(i)) => i.value.map(|v| v as u64),
        _ => None,
    }
}

// --------------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Handler;
    use crate::span::Sources;
    use crate::{lexer, parser};

    /// Stages [1]–[4] over in-memory files: `(display path, package path, source)`.
    fn program(files: &[(&str, &str, &str)]) -> Program {
        let mut sources = Sources::default();
        let mut h = Handler::default();
        let mut tuple = Vec::new();
        for (path, package, code) in files {
            let f = sources.add((*path).to_string(), (*code).to_string());
            let lexed = lexer::lex_file(&sources, f, &mut h);
            let pf = parser::parse(&sources, f, &lexed, &mut h);
            tuple.push((f, (*package).to_string(), pf));
        }
        crate::sema::check_files(&sources, &mut h, "tour", &tuple, false, vec![], vec![])
    }

    /// A program with no declarations, for the rows that need no name resolution.
    fn empty() -> Program {
        program(&[("e/mod.infy", "tour/e", "package e\n")])
    }

    fn lay(t: Ty) -> Layout {
        lay_on(Target::REFERENCE, t)
    }
    fn lay_on(target: Target, t: Ty) -> Layout {
        let prog = empty();
        let mut l = Layouter::new(target, &prog);
        match l.layout(&t) {
            Ok(x) => x,
            Err(e) => panic!("{}: {}", t.text(), e.message()),
        }
    }
    fn try_lay_on(target: Target, t: Ty, prog: &Program) -> Result<Layout, HirError> {
        Layouter::new(target, prog).layout(&t)
    }

    fn lower_program(prog: &Program, entry: Option<&str>) -> HirCrate {
        lower(prog, &HirConfig { target: Target::REFERENCE, entry_chain: entry.map(|s| s.to_string()) })
    }

    /// §14's tour, read from the repository so the golden can never drift from the sample.
    fn tour() -> Program {
        program(&[
            ("core/build/mod.infy", "tour/core/build", include_str!("../examples/tour/core/build/mod.infy")),
            ("cmd/runner/main.infy", "tour/cmd/runner", include_str!("../examples/tour/cmd/runner/main.infy")),
        ])
    }

    fn u32t() -> Ty {
        Ty::Prim(Prim::U32)
    }

    // -------------------------------------------------------------- §1 scalar rows

    /// §1's table, row by row. Every pair is what `gcc -std=c11` measured on the reference target
    /// (`docs/RUNLOG.md` cron-07, probe `sizeof`/`_Alignof`).
    #[test]
    fn scalar_rows_of_abi_section_1() {
        let cases: Vec<(&str, Ty, u64, u64)> = vec![
            ("bool", Ty::Bool, 1, 1),
            ("char", Ty::Prim(Prim::Char), 4, 4),
            ("()", Ty::Unit, 1, 1),
            ("u8", Ty::Prim(Prim::U8), 1, 1),
            ("i8", Ty::Prim(Prim::I8), 1, 1),
            ("u16", Ty::Prim(Prim::U16), 2, 2),
            ("u32", u32t(), 4, 4),
            ("i64", Ty::Prim(Prim::I64), 8, 8),
            ("u64", Ty::Prim(Prim::U64), 8, 8),
            ("usize", Ty::Prim(Prim::Usize), 8, 8),
            ("isize", Ty::Prim(Prim::Isize), 8, 8),
            ("f32", Ty::Prim(Prim::F32), 4, 4),
            ("f64", Ty::Prim(Prim::F64), 8, 8),
            ("u128", Ty::Prim(Prim::U128), 16, 8),
            ("i128", Ty::Prim(Prim::I128), 16, 8),
            ("str", Ty::Str, 16, 8),
            ("String", Ty::String, 24, 8),
            ("[u32]", Ty::Slice { inner: Box::new(u32t()) }, 16, 8),
            ("Vec<u32>", Ty::Vec { inner: Box::new(u32t()) }, 24, 8),
            ("Box<u32>", Ty::Boxed { inner: Box::new(u32t()) }, 8, 8),
            ("Rc<u32>", Ty::Rc { inner: Box::new(u32t()), atomic: false }, 16, 8),
            ("Arc<u32>", Ty::Rc { inner: Box::new(u32t()), atomic: true }, 16, 8),
            ("[u32; 4]", Ty::Array { inner: Box::new(u32t()), n: 4 }, 16, 4),
            ("[u8; 4]", Ty::Array { inner: Box::new(Ty::Prim(Prim::U8)), n: 4 }, 4, 1),
            ("&u32", Ty::Ref { inner: Box::new(u32t()), mutable: false, lifetime: None }, 8, 8),
            ("&mut u32", Ty::Ref { inner: Box::new(u32t()), mutable: true, lifetime: None }, 8, 8),
            ("*const u8", Ty::Ptr { inner: Box::new(Ty::Prim(Prim::U8)), mutable: false }, 8, 8),
            ("fn() -> u32", Ty::Fn { params: vec![], ret: Some(Box::new(u32t())) }, 8, 8),
            ("closure", Ty::Closure(0), 16, 8),
            ("dyn Display", Ty::Dyn("Display".into()), 16, 8),
        ];
        for (text, ty, size, align) in cases {
            let got = lay(ty.clone());
            assert_eq!(got, Layout::new(size, align), "{text}");
        }
    }

    /// §2.8: `usize`/`isize` take the target width; the fixed-width `u64` fields inside
    /// `String`/`Vec` do not shrink with it.
    #[test]
    fn pointer_width_is_a_target_property_not_a_constant() {
        assert_eq!(lay_on(Target::WASM32, Ty::Prim(Prim::Usize)), Layout::new(4, 4));
        assert_eq!(lay_on(Target::WASM32, Ty::Str), Layout::new(16, 8));
        assert_eq!(lay_on(Target::WASM32, Ty::String), Layout::new(24, 8));
        assert_eq!(lay_on(Target::WASM32, Ty::Vec { inner: Box::new(u32t()) }), Layout::new(24, 8));
    }

    /// §1 gives `__int128` as "16/8 (16/16 on GCC >= 13; the compiler emits the probed value)".
    /// The probe on this machine's ucrt64 gcc measured align 16, so a struct containing a `u128`
    /// really does change size — the ABI number is a probe result, never an assumption.
    #[test]
    fn u128_alignment_is_probed_and_changes_dependent_layouts() {
        let pair = Ty::Tuple(vec![u32t(), Ty::Prim(Prim::U128)]);
        assert_eq!(lay_on(Target::REFERENCE, pair.clone()), Layout::new(24, 8));
        assert_eq!(lay_on(Target::X86_64_GCC13, pair), Layout::new(32, 16));
    }

    // ------------------------------------------------------- §1 Option/Result rows

    #[test]
    fn option_and_result_rows_of_abi_section_1() {
        // `Option<T>` for a non-pointer T: { T val; uint8_t some; }.
        assert_eq!(lay(Ty::Option { inner: Box::new(u32t()) }), Layout::new(8, 4));
        assert_eq!(lay(Ty::Option { inner: Box::new(Ty::Str) }), Layout::new(24, 8));
        assert_eq!(lay(Ty::Option { inner: Box::new(Ty::Prim(Prim::U8)) }), Layout::new(2, 1));
        assert_eq!(
            lay(Ty::Option { inner: Box::new(Ty::Rc { inner: Box::new(u32t()), atomic: true }) }),
            Layout::new(24, 8),
            "Rc is not on the niche list, so it keeps its own tag"
        );
        // The niche list is exactly `&T`, `Box<U>`, `*const U`, fn ptr — nothing else gets it.
        for inner in [
            Ty::Ref { inner: Box::new(u32t()), mutable: false, lifetime: None },
            Ty::Boxed { inner: Box::new(u32t()) },
            Ty::Ptr { inner: Box::new(Ty::Prim(Prim::U8)), mutable: false },
            Ty::Fn { params: vec![], ret: None },
        ] {
            assert_eq!(lay(Ty::Option { inner: Box::new(inner) }), Layout::new(8, 8));
        }
        // §1's shorthand "sizeof(T)+sizeof(E)+1, aligned" is only right when T and E agree on
        // alignment; the rule actually implemented is C padding, measured for `Result<u32, Fail>`
        // at 20/4 (ok@0 err@4 is_ok@16) rather than the shorthand's 16.
        let prog = tour();
        let r = Ty::Result { ok: Box::new(u32t()), err: Box::new(Ty::Named("Fail".into())) };
        assert_eq!(try_lay_on(Target::REFERENCE, r, &prog).unwrap(), Layout::new(20, 4));
        // A `u64` payload after a `u32` pushes `err` to offset 8 and the tag to 16, so the C rule
        // gives 24 — the doc's arithmetic shorthand would have said 16.
        let same_align = Ty::Result { ok: Box::new(u32t()), err: Box::new(Ty::Prim(Prim::U64)) };
        assert_eq!(lay(same_align), Layout::new(24, 8));
    }

    // ------------------------------------------------------ §1 fieldless enum (D-24)

    #[test]
    fn fieldless_enums_are_exactly_their_tag_and_discriminants_survive() {
        let prog = program(&[(
            "m/mod.infy",
            "tour/m",
            "package m\n\npub enum Phase { Fetch, Compile, Link, Publish }\npub enum Code { A = 3, B, C = 10 }\npub enum Mixed { Zero, One(u64) }\n",
        )]);
        let h = lower_program(&prog, None);
        assert!(h.errors.is_empty(), "{:?}", h.errors.iter().map(|e| e.message()).collect::<Vec<_>>());
        assert_eq!(h.enum_by_name("Phase").unwrap().layout, Layout::new(4, 4));
        let code = h.enum_by_name("Code").unwrap();
        assert_eq!(
            code.variants.iter().map(|v| v.discriminant).collect::<Vec<_>>(),
            vec![3, 4, 10],
            "§4.1: an explicit discriminant is the tag value, and the unnumbered one continues from it"
        );
        let mixed = h.enum_by_name("Mixed").unwrap();
        assert_eq!(mixed.layout, Layout::new(16, 8), "tag 4 + pad 4 + union(8)");
        assert_eq!(mixed.variant("One").unwrap().payload_offset, 8);
        assert_eq!(mixed.variant("Zero").unwrap().payload.len(), 0);
    }

    // ------------------------------------------------------------ tour declarations

    /// §14's tour lowered through stage [7]: every size and offset equals the `gcc` measurement.
    #[test]
    fn tour_declarations_carry_the_measured_layouts() {
        let prog = tour();
        let h = lower_program(&prog, Some("Pipeline"));
        let errs: Vec<String> = h.errors.iter().map(|e| e.message()).collect();
        assert!(errs.is_empty(), "tour failed to lower: {errs:?}");

        let phase = h.enum_by_name("Phase").unwrap();
        assert_eq!(phase.layout, Layout::new(4, 4));

        let why = h.enum_by_name("FailWhy").unwrap();
        assert_eq!(why.layout, Layout::new(8, 4));
        let native = why.variant("Native").unwrap();
        assert_eq!(native.payload[0].layout, Layout::new(4, 4));
        assert_eq!(native.payload[0].offset, 0, "payload offsets are relative to the payload start");
        assert_eq!(native.payload_offset, 4);
        assert_eq!(why.variant("Missing").unwrap().payload_offset, 4);

        let fail = h.struct_by_name("Fail").unwrap();
        assert_eq!(fail.layout, Layout::new(12, 4));
        assert_eq!(fail.fields[0].offset, 0);
        assert_eq!(fail.fields[1].offset, 4);

        let job = h.struct_by_name("Job").unwrap();
        assert_eq!(job.layout, Layout::new(12, 4));
        assert_eq!(job.fields.iter().map(|f| f.offset).collect::<Vec<_>>(), vec![0, 4, 8]);
        assert_eq!(job.fields[1].layout, Layout::new(4, 4), "a fieldless-enum field is 4 bytes");

        let describe = h.fn_by_name("describe").unwrap();
        assert_eq!(describe.params[0].layout, Layout::new(8, 8), "`&Job` is a thin pointer");
        assert_eq!(describe.ret.as_ref().unwrap().1, Layout::new(16, 8), "`str` is (ptr, len)");
        assert_eq!(h.fn_by_name("gate").unwrap().ret.as_ref().unwrap().1, Layout::new(20, 4));
        assert_eq!(h.fn_by_name("twice").unwrap().ret.as_ref().unwrap().1, Layout::new(20, 4));
        assert_eq!(h.fn_by_name("bump").unwrap().ret.as_ref().unwrap().1, Layout::new(12, 4));
        assert_eq!(h.fn_by_name("all_green").unwrap().ret.as_ref().unwrap().1, Layout::new(1, 1));
        assert!(h.fn_by_name("spawn").unwrap().ret.is_none(), "a `()` return carries no value");
    }

    /// §4.8: `offset_of` is a `repr(C)` privilege. The backend lays `repr(Rust)` out in
    /// declaration order too, but the source may not depend on it.
    #[test]
    fn offset_of_field_is_only_available_for_repr_c() {
        let prog = program(&[(
            "m/mod.infy",
            "tour/m",
            "package m\n\npub struct Rust { pub a: u8, pub b: u32 }\n#[repr(C)] pub struct Raw { pub a: u8, pub b: u32 }\n",
        )]);
        let h = lower_program(&prog, None);
        let rust = h.struct_by_name("Rust").unwrap();
        let raw = h.struct_by_name("Raw").unwrap();
        assert_eq!(rust.repr, Repr::Rust);
        assert_eq!(raw.repr, Repr::C);
        assert_eq!(rust.layout, Layout::new(8, 4));
        assert_eq!(raw.layout, Layout::new(8, 4));
        assert_eq!(rust.offset_of("b"), None, "§4.8: repr(Rust) offsets are not a source-level fact");
        assert_eq!(raw.offset_of("b"), Some(4));
        assert_eq!(raw.offset_of("missing"), None);
    }

    // ------------------------------------------------------- §4.15 infinite sizes

    #[test]
    fn by_value_recursion_is_e0175_and_indirection_fixes_it() {
        let prog = program(&[(
            "m/mod.infy",
            "tour/m",
            "package m\n\npub struct Loop { pub n: u32, pub next: Loop }\n",
        )]);
        let h = lower_program(&prog, None);
        assert_eq!(h.errors.len(), 1, "{:?}", h.errors.iter().map(|e| e.message()).collect::<Vec<_>>());
        assert_eq!(h.errors[0].spec_code(), Some("E-0175"));
        assert!(h.items.iter().all(|it| it.name() != "Loop"), "a type with no layout must not appear in the HIR");

        let boxed = program(&[(
            "m/mod.infy",
            "tour/m",
            "package m\n\npub struct Node { pub v: u32, pub next: Option<Box<Node>> }\n",
        )]);
        let h = lower_program(&boxed, None);
        assert!(h.errors.is_empty(), "{:?}", h.errors.iter().map(|e| e.message()).collect::<Vec<_>>());
        // `Option<Box<T>>` is the pointer niche (8/8), so the cell is u32@0 + ptr@8 = 16/8.
        assert_eq!(h.struct_by_name("Node").unwrap().layout, Layout::new(16, 8));
    }

    /// D-25: the tag is `uint32_t` for every enum, so an oversized discriminant is §2.8's
    /// out-of-range literal error rather than a truncation.
    #[test]
    fn a_discriminant_wider_than_the_tag_is_e0042() {
        let prog = program(&[(
            "m/mod.infy",
            "tour/m",
            "package m

pub enum Wide { Lo = 1, Hi = 4294967296 }
",
        )]);
        let h = lower_program(&prog, None);
        let codes: Vec<&'static str> = h.errors.iter().filter_map(|e| e.spec_code()).collect();
        assert_eq!(codes, vec!["E-0042"], "{:?}", h.errors.iter().map(|e| e.message()).collect::<Vec<_>>());
        assert!(h.enum_by_name("Wide").is_none(), "an enum with no representable tag must not be emitted");
    }

    #[test]
    fn types_with_no_layout_are_refused_not_defaulted() {
        let prog = empty();
        for (ty, want) in [
            (Ty::Generic("T".into()), "MonomorphRequired"),
            (Ty::Never, "Unrepresentable"),
            (Ty::IntLit, "Unrepresentable"),
            (Ty::Auto, "Unrepresentable"),
            (Ty::Named("Nope".into()), "UnresolvedType"),
        ] {
            let e = Layouter::new(Target::REFERENCE, &prog)
                .layout(&ty)
                .err()
                .unwrap_or_else(|| panic!("{} was accepted", ty.text()));
            assert!(format!("{e:?}").starts_with(want), "{:?} -> {:?}", ty, e);
            assert_eq!(e.spec_code(), None, "an implementation-stage invariant is not a spec code");
        }
    }

    // ------------------------------------------------------------ §8.4 / §5 chains

    /// ABI §5 + §8.4 rule 1: only top-level host statements open a node; `label` and the
    /// `{ … }` linkage blocks belong to the node they follow. Printed for the golden in RUNLOG.
    #[test]
    fn tour_pipeline_node_table_matches_the_source() {
        let prog = tour();
        let h = lower_program(&prog, Some("Pipeline"));
        let c = h.chain_by_name("Pipeline").unwrap();
        let rows: Vec<(u32, u32, u32, u32, Option<String>)> = c
            .nodes
            .iter()
            .map(|n| (n.idx, n.line, n.col, n.linkage_count, n.host_event.clone()))
            .collect();
        // Measured against examples/tour/cmd/runner/main.infy as `infy fmt` writes it (偏差 D-17):
        // `gate()` owns lines 10-15 and `retry_once()` lines 16-18, each carrying its `{ … }`
        // linkage block, so they add a linkage count but no node — and `label` lines 6/19 are
        // preambles, which is why `finalize` is node 5.
        assert_eq!(
            rows,
            vec![
                (0, 7, 5, 0, Some("spawn".into())),
                (1, 8, 5, 0, Some("harvest".into())),
                (2, 9, 5, 0, Some("report".into())),
                (3, 10, 5, 1, Some("gate".into())),
                (4, 16, 5, 1, Some("retry_once".into())),
                (5, 20, 5, 0, Some("finalize".into())),
            ]
        );
        let labels: Vec<(String, u32)> = c.labels.iter().map(|(k, v)| (k.clone(), *v)).collect();
        assert_eq!(labels, vec![("begin".to_string(), 0), ("done".to_string(), 5)]);
        assert_eq!(
            c.nodes.iter().map(|n| n.anchor.clone()).collect::<Vec<_>>(),
            vec![
                Some("begin".to_string()),
                None,
                None,
                None,
                None,
                Some("done".to_string())
            ],
            "§8.2: an anchor names the node that follows it"
        );
        assert!(c.nodes.iter().all(|n| n.event_id.is_none()), "the tour calls plain fns, not events");
        // No chain-level bindings in the tour, so the frame is just `node_idx`.
        assert!(c.frame.locals.is_empty());
        assert_eq!((c.frame.node_idx_offset, c.frame.size, c.frame.align), (0, 4, 4));
    }

    #[test]
    fn chain_frame_is_locals_then_node_idx() {
        let prog = program(&[(
            "m/mod.infy",
            "tour/m",
            "package m\n\nchain C {\n    let a: u32 = 0;\n    var b: u64 = 1;\n    ping();\n}\nfn ping() { }\n",
        )]);
        let h = lower_program(&prog, Some("C"));
        assert!(h.errors.is_empty(), "{:?}", h.errors.iter().map(|e| e.message()).collect::<Vec<_>>());
        let c = h.chain_by_name("C").unwrap();
        let names: Vec<(&str, u64, bool)> =
            c.frame.locals.iter().map(|l| (l.name.as_str(), l.offset, l.mutable)).collect();
        assert_eq!(names, vec![("a", 0, false), ("b", 8, true)], "§8.4: chain-level let/var are a node's preamble but live in the frame");
        assert_eq!(c.frame.node_idx_offset, 16);
        assert_eq!((c.frame.size, c.frame.align), (24, 8), "tail padding to the frame alignment");
        assert_eq!(c.nodes.len(), 1);
    }

    // ------------------------------------------------------------------ §7.4 entry

    #[test]
    fn entry_rule_prefers_entry_chain_then_main_then_first_chain() {
        let h = lower_program(&tour(), Some("Pipeline"));
        match h.entry {
            Some(HirEntry::Chain(i)) => assert_eq!(h.items[i].name(), "Pipeline"),
            other => panic!("expected the declared entry chain, got {other:?}"),
        }

        let with_main = program(&[(
            "m/mod.infy",
            "tour/m",
            "package m\n\nchain C {\n    go();\n}\nfn go() { }\nfn main() { }\n",
        )]);
        let h = lower_program(&with_main, None);
        match h.entry {
            Some(HirEntry::Main(i)) => assert_eq!(h.items[i].name(), "main"),
            other => panic!("§7.4: fn main beats a chain when no entry_chain is declared, got {other:?}"),
        }

        let only_chain = program(&[(
            "m/mod.infy",
            "tour/m",
            "package m\n\nchain C {\n    go();\n}\nfn go() { }\n",
        )]);
        let h = lower_program(&only_chain, None);
        assert!(matches!(h.entry, Some(HirEntry::Chain(_))));

        let bad = lower_program(&tour(), Some("Nope"));
        assert_eq!(bad.errors.iter().filter_map(|e| e.spec_code()).collect::<Vec<_>>(), vec!["E-0800"]);
    }

    // --------------------------------------------------------- §10 / §4.6 env table

    /// The `EnvOffsets` table must agree with the frozen `runtime/infy_rt.h` — the host checks the
    /// same layout at startup and answers `R-0905` (§11.2) when it does not.
    #[test]
    fn env_offset_table_matches_the_frozen_header() {
        let t = env_table(Target::REFERENCE);
        let names: Vec<&str> = t.fields.iter().map(|f| f.name).collect();
        assert_eq!(
            names,
            vec![
                "iter", "runtime_s", "realtime", "ram_mb", "cpu_use", "gpu_use", "chain_id",
                "node_idx", "focus_id", "platform", "build_id"
            ],
            "§4.6's field set, in order — an implementation may not extend it (UB-024)"
        );
        let measured: Vec<(&str, u64)> = vec![
            ("iter", 0),
            ("runtime_s", 4),
            ("realtime", 8),
            ("ram_mb", 16),
            ("cpu_use", 20),
            ("gpu_use", 24),
            ("chain_id", 28),
            ("node_idx", 32),
            ("focus_id", 36),
            ("platform", 40),
            ("build_id", 56),
        ];
        for (name, off) in measured {
            assert_eq!(t.offset_of(name), Some(off), "offsetof(InfyEnvSnapshot, {name})");
        }
        assert_eq!(t.layout, Layout::new(72, 8));
        assert_eq!(t.offset_of("queue_len"), None, "§1.3 rule 2: domain-specific fields go through std.host.query");
    }

    // ------------------------------------------------------------ §5 / §8.4 ABI rows

    /// `InfyNodeLoc`, `InfyChainDesc` and `InfyFocusId3` are the records `.inf meta` describes and
    /// codegen emits; their offsets are ABI, so they are golden-tested against `gcc`.
    #[test]
    fn abi_table_rows_match_infy_rt_h() {
        let (loc, l) = node_loc_row(Target::REFERENCE);
        let offs: Vec<u64> = loc.iter().map(|f| f.offset).collect();
        assert_eq!(offs, vec![0, 8, 12, 16, 20]);
        assert_eq!(l, Layout::new(24, 8));
        assert_eq!(
            loc.iter().map(|f| f.name).collect::<Vec<_>>(),
            vec!["file", "line", "col", "node_idx", "linkage_count"]
        );

        let (desc, d) = chain_desc_row(Target::REFERENCE);
        assert_eq!(
            desc.iter().map(|f| f.offset).collect::<Vec<_>>(),
            vec![0, 8, 12, 16, 20, 24, 32, 40]
        );
        assert_eq!(d, Layout::new(48, 8));

        let (fid, f) = focus_id_row(Target::REFERENCE);
        assert_eq!(fid.iter().map(|x| x.offset).collect::<Vec<_>>(), vec![0, 4, 8]);
        assert_eq!(f, Layout::new(12, 4));
    }

    /// §1: `InfyVtable_T` is `drop`, `size`, `align`, then the methods in declaration order.
    #[test]
    fn vtable_prefix_is_drop_size_align_then_methods() {
        let h = lower_program(&tour(), Some("Pipeline"));
        assert_eq!(h.vtable_layout(0), Layout::new(24, 8));
        assert_eq!(h.vtable_layout(2), Layout::new(40, 8), "measured: drop@0 size@8 align@16 m0@24 m1@32");
        for n in 0..6u64 {
            assert_eq!(h.vtable_layout(n).size, 24 + 8 * n);
        }
    }

    /// D-50(c): 规范只说"vtable 布局由 `IFABI` 固定"、附录 C-7「仍缺 ②」确认偏移表从未写出，
    /// 所以偏移是本实现定的。**数只有一份真值 = `vtable_record`**，`docs/internals/runtime-abi.md`
    /// §1 那个代码块是**被读出来比对的**那一侧（经验库 checkout-eol / doc-claims 的口径：
    /// 差分文本由对方生成，不许两侧各自算）。改偏移就得同时改文档，否则这里红。
    #[test]
    fn the_vtable_offsets_published_in_the_abi_doc_are_the_ones_the_layouter_produces() {
        let doc = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs").join("internals").join("runtime-abi.md"),
        )
        .expect("docs/internals/runtime-abi.md");
        let slot = |t: &str| -> (String, u64, u64) {
            // `drop@0:8` / `0@24:8`
            let (name, rest) = t.split_once('@').unwrap_or_else(|| panic!("不是一个槽位：{t}"));
            let (off, w) = rest.split_once(':').unwrap_or_else(|| panic!("缺宽度：{t}"));
            (name.to_string(), off.parse().expect("偏移是十进制"), w.parse().expect("宽度是十进制"))
        };
        let mut prefix: Vec<(String, u64, u64)> = Vec::new();
        let mut methods: Vec<(u64, u64, u64)> = Vec::new();
        let mut totals: Vec<(u64, u64)> = Vec::new();
        for line in doc.lines().filter(|l| l.starts_with("prefix ") || l.starts_with("method ") || l.starts_with("total ")) {
            let body = line.split("//").next().expect("注释切得开");
            let mut toks = body.split_whitespace();
            match toks.next() {
                Some("prefix") => prefix = toks.map(slot).collect(),
                Some("method") => methods = toks.map(|t| { let (i, o, w) = slot(t); (i.parse().expect("方法是数字下标"), o, w) }).collect(),
                Some("total") => {
                    totals = toks
                        .map(|t| {
                            let v = t.split('=').nth(1).unwrap_or_else(|| panic!("total 写成 n=<k>:<size>：{t}"));
                            let (k, s) = v.split_once(':').unwrap_or_else(|| panic!("total 写成 n=<k>:<size>：{t}"));
                            (k.parse().expect("n 是十进制"), s.parse().expect("size 是十进制"))
                        })
                        .collect()
                }
                _ => {}
            }
        }
        assert_eq!(prefix.len(), 3, "`prefix` 行必须列出 drop/size/align 三个槽：{prefix:?}");
        assert!(!methods.is_empty() && !totals.is_empty(), "文档里必须给出方法槽与 total 的实数");
        let h = lower_program(&tour(), Some("Pipeline"));
        let wide = *methods.iter().map(|(i, _, _)| i).max().expect("有方法下标") + 1;
        let r = h.vtable_record(wide);
        for (i, (name, off, w)) in prefix.iter().enumerate() {
            assert_eq!((off, w), (&r.offsets[i], &r.layouts[i].size), "文档里的 `{name}` 槽 ≠ 布局器：{name}@{off}:{w}");
        }
        assert_eq!(
            prefix.iter().map(|(n, _, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["drop", "size", "align"],
            "前缀顺序就是 D-50(c) 定的顺序"
        );
        for (i, off, w) in &methods {
            let slot_i = (*i as usize) + 3;
            assert_eq!((off, w), (&r.offsets[slot_i], &r.layouts[slot_i].size), "文档里的方法 #{i} 槽 ≠ 布局器");
            assert_eq!(*off, 24 + 8 * i, "方法 i 落在 24 + 8·i（{off}）");
        }
        for (n, size) in &totals {
            assert_eq!(*size, h.vtable_layout(*n).size, "文档里的 total(n={n}) ≠ 布局器");
        }
    }

    // ----------------------------------------------------------------- §2 mangling

    #[test]
    fn symbols_follow_the_abi_section_2_rule() {
        assert_eq!(mangle("tour/core/build", "describe"), "infy_tour_core_build_describe");
        assert_eq!(mangle("a.b-c", "f"), "infy_a_b_c_f");
        assert_eq!(mangle("p", "x"), "infy_p_x");
        let h = lower_program(&tour(), Some("Pipeline"));
        assert_eq!(h.fn_by_name("describe").unwrap().symbol, "infy_tour_core_build_describe");
        assert_eq!(h.chain_by_name("Pipeline").unwrap().symbol, "infy_tour_cmd_runner_Pipeline");
        assert_eq!(h.enum_by_name("Phase").unwrap().symbol, "infy_tour_core_build_Phase");
    }

    /// §11: a chain node whose host statement is an `event` gets the EventTable id; a plain `fn`
    /// does not, and the HIR says so instead of inventing one.
    #[test]
    fn only_event_callees_get_a_host_event_id() {
        let prog = program(&[(
            "m/mod.infy",
            "tour/m",
            "package m\n\nevent ping();\nevent pong();\nchain C {\n    ping();\n    pong();\n    helper();\n}\nfn helper() { }\n",
        )]);
        let h = lower_program(&prog, Some("C"));
        assert!(h.errors.is_empty(), "{:?}", h.errors.iter().map(|e| e.message()).collect::<Vec<_>>());
        let ids: Vec<Option<u32>> = h.chain_by_name("C").unwrap().nodes.iter().map(|n| n.event_id).collect();
        assert_eq!(ids, vec![Some(0), Some(1), None]);
        assert_eq!(h.events.len(), 2);
        assert_eq!(h.event_id("ping"), Some(0));
        assert_eq!(h.event_id("pong"), Some(1));
        assert_eq!(h.event_id("helper"), None);
    }

    /// §3.3/§7.6: an `event` with a `;` body is host-provided — the HIR keeps that distinction so
    /// codegen never emits a C function for it.
    #[test]
    fn foreign_events_and_a_track_externs_keep_their_linkage() {
        let prog = program(&[(
            "m/mod.infy",
            "tour/m",
            "package m\n\nevent host_only();\npub fn local() -> u32 { 1 }\n",
        )]);
        let h = lower_program(&prog, None);
        let e = h.fn_by_name("host_only").unwrap();
        assert!(e.is_event && e.is_foreign);
        let l = h.fn_by_name("local").unwrap();
        assert!(!l.is_event && !l.is_foreign);
        assert_eq!(l.ret.as_ref().unwrap().1, Layout::new(4, 4));
    }

    /// Determinism is a §7.3 / §12.3 requirement: the same source must produce the same item order
    /// and the same numbers, twice.
    #[test]
    fn lowering_is_deterministic() {
        let a = lower_program(&tour(), Some("Pipeline"));
        let b = lower_program(&tour(), Some("Pipeline"));
        let syms = |h: &HirCrate| h.items.iter().map(|it| it.symbol().to_string()).collect::<Vec<_>>();
        assert_eq!(syms(&a), syms(&b));
        let lays = |h: &HirCrate| h.items.iter().filter_map(|it| it.layout()).collect::<Vec<_>>();
        assert_eq!(lays(&a), lays(&b));
        assert!(!syms(&a).is_empty());
    }
}
