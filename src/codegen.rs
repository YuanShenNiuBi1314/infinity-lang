//! Stage [8] codegen (§7.1): INFY → C11.
//!
//! The declaration-level HIR of stage [7] is the only upstream interface: sizes, alignments,
//! offsets, symbol names and the node table are **never recomputed here** ([`crate::hir`] is the
//! single layout authority). The generated file re-asserts those numbers with negative-size-array
//! checks, so a codegen/layout drift fails the C compile instead of shipping a second layout
//! (§4.8 — the layout is a contract with the source).
//!
//! Every boundary check is a call into `runtime/infy_rt.h`: checked arithmetic, index bounds,
//! `unwrap` on `None`, shift width, division by zero (§4.9, §7.2 — checks are never compiled out at
//! any `-O` level, and this back end has no "release skips checks" path).
//!
//! Coverage is the §14 tour subset: primitives, `str`, `String`, `Vec`, `Option`, `Result`,
//! `struct`, `enum`, chains, `?` (§4.11), `match` (§3.12). Anything else is refused with a named
//! reason rather than a plausible-looking substitute (§7.1). Choices and refusals are recorded in
//! `docs/spec-deviations.md` (D-26…D-30) and tracked in `docs/BACKLOG.md`.

use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{
    ArmBody, Block, Body, ElseBranch, Expr, ExprKind, FieldName, Item, ItemKind, LinkageBlock,
    LinkageStmt, ParsedFile, Pat, PatKind, Prim, StartNode, Stmt, StmtKind,
};
use crate::hir::{HirCrate, HirEntry, HirItem, Layout, Target};
use crate::sema::{ChainInfo, ConstValue, Program, TestKind, Ty};
use crate::span::{FileId, Sources};
use crate::token::{Lit, Punct};

/// §7.2's built-in target set; anything else must be registered in `TOOLCHAIN.infy.toml`, else
/// `E-0601`.
pub const BUILTIN_TARGETS: [&str; 5] = [
    "x86_64-pc-windows-msvc",
    "x86_64-pc-windows-gnu",
    "x86_64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "aarch64-unknown-linux-gnu",
];

/// Resolve a §7.2 triple to the layout parameters codegen must assert. `None` = unregistered
/// triple (`E-0601`).
pub fn target_from_triple(triple: &str) -> Option<(String, Target)> {
    if !BUILTIN_TARGETS.contains(&triple) {
        return None;
    }
    let width: u64 = match triple.split('-').next().unwrap_or("") {
        "x86_64" | "aarch64" => 8,
        _ => return None,
    };
    // `__int128` is 16-aligned on GCC >= 13 (docs/internals/runtime-abi.md §1); the MSVC triple has
    // no `__int128`, so the software pair's 8 is what the layout must assume there.
    let u128_align = if triple.contains("msvc") { 8 } else { 16 };
    Some((triple.to_string(), Target { pointer_width: width, u128_align }))
}

/// A codegen refusal. `spec` names the clause the missing feature belongs to, so the message says
/// what is missing instead of a bare "unsupported".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CgError {
    pub message: String,
    pub spec: &'static str,
}

impl CgError {
    fn new(spec: &'static str, message: impl Into<String>) -> CgError {
        CgError { message: message.into(), spec }
    }
    pub fn render(&self) -> String {
        format!("error: {} ({})", self.message, self.spec)
    }
}

/// Codegen inputs: the artifact's target, and the path the emitted `#include` prints.
#[derive(Clone, Debug)]
pub struct Config {
    pub target_triple: String,
    pub target: Target,
    pub header_include: String,
    /// `[profile] overflow` (§5.2/§4.4). Only `panic` is implemented; `saturate` is refused (D-30).
    pub overflow_panic: bool,
    pub module: String,
    /// §5.9: `infy test` — the generated `main` becomes the `#[test]` harness instead of calling
    /// the §7.4 entry.
    pub test_harness: bool,
    /// §13.4's `infy test --bench`: put `#[bench]` functions in the harness's run set as well.
    pub bench: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            target_triple: "x86_64-pc-windows-gnu".to_string(),
            target: Target::REFERENCE,
            header_include: "infy_rt.h".to_string(),
            overflow_panic: true,
            module: String::new(),
            test_harness: false,
            bench: false,
        }
    }
}

/// Run stage [8]: returns the generated C11 text, or every refusal hit in this stage (§7.1 wants
/// as much of the error set as possible per stage, not just the first).
pub fn emit<'x>(
    prog: &'x Program,
    hir: &'x HirCrate,
    files: &'x [(FileId, String, ParsedFile)],
    sources: &'x Sources,
    cfg: &'x Config,
) -> Result<String, Vec<CgError>> {
    let mut cg = Cg::new(prog, hir, files, sources, cfg);
    cg.run();
    if !cg.errors.is_empty() {
        return Err(cg.errors);
    }
    Ok(cg.assemble())
}

/// C keywords and reserved identifiers that §2.4 does **not** reserve, so an INFY name may legally
/// collide with one. Every AST-derived identifier passes through this.
///
/// `atrack` shares the table: a wrapper's C parameter names come out of the same spelling rule, so a
/// second list would be a second truth (`docs/spec-deviations.md` D-46(e)).
pub(crate) const C_KEYWORDS: &[&str] = &[
    "auto", "break", "case", "char", "const", "continue", "default", "do", "double", "else",
    "enum", "extern", "float", "for", "goto", "if", "inline", "int", "long", "register",
    "restrict", "return", "short", "signed", "sizeof", "static", "struct", "switch", "typedef",
    "union", "unsigned", "void", "volatile", "while", "_Alignas", "_Alignof", "_Atomic", "_Bool",
    "_Complex", "_Generic", "_Noreturn", "_Static_assert",
];

pub(crate) fn c_ident(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 2);
    for c in name.chars() {
        out.push(if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' });
    }
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    if C_KEYWORDS.contains(&out.as_str()) || out.starts_with("infy_") || out.starts_with('_') {
        out.push('x');
    }
    out
}

/// Non-alphanumerics collapse so a synthesized type name is a valid C identifier.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect()
}

/// A C string literal for an INFY `str` (§2.9). NUL is refused rather than silently truncated,
/// because `infy_str_of` measures with `strlen` (D-29).
fn c_str_literal(s: &str) -> Result<String, CgError> {
    if s.bytes().any(|b| b == 0) {
        return Err(CgError::new(
            "§2.9 / §4.9 UB-007",
            "字符串字面量含 U+0000：`infy_str_of` 以 `strlen` 计量，会静默截断，已拒绝而非生成截断值",
        ));
    }
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\{:03o}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    Ok(out)
}

/// `T[N]` is not a standalone type name in C: declarations must place the declarator after the
/// variable. This splits a trailing array suffix off so `{base} {name}{suffix}` is emitted.
fn declarator(cty: &str, name: &str) -> String {
    if let Some(open) = cty.rfind('[') {
        if cty.ends_with(']') {
            let base = cty[..open].trim_end();
            let suffix = &cty[open..];
            return format!("{base} {name}{suffix}");
        }
    }
    format!("{cty} {name}")
}

/// Does the emitted C text read `name` **by value**? An identifier-boundary match whose left
/// neighbour is `&` is the address of that binding — a borrow, which leaves ownership in place.
fn mentions_by_value(text: &str, name: &str) -> bool {
    let bytes = text.as_bytes();
    let is_cont = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut from = 0usize;
    while let Some(off) = text[from..].find(name) {
        let start = from + off;
        let end = start + name.len();
        from = end;
        let left_free = start == 0 || !is_cont(bytes[start - 1]);
        let right_free = end >= bytes.len() || !is_cont(bytes[end]);
        if !(left_free && right_free) {
            continue;
        }
        if text[..start].trim_end().ends_with('&') {
            continue;
        }
        return true;
    }
    false
}

struct TypeNode {
    /// Definition text; `None` while still being computed.
    def: Option<String>,
    /// Type names this definition contains **by value**, so they must be defined before it.
    deps: BTreeSet<String>,
}

#[derive(Clone, Debug)]
struct Binding {
    infy: String,
    cvar: String,
    ty: Ty,
    /// A cleanup entry was pushed for this binding (§4.7.4), so it owes one `pop_run` at scope end.
    /// `pop` is that owed line's slot: a §4.7.1 move cancels the entry by address and clears the
    /// slot in the same step, which is what keeps the deque and the emitted pops one-for-one.
    cleanup: bool,
    pop: Option<(usize, usize)>,
}

/// Where `goto` / `start` targets resolve inside the chain currently being emitted.
#[derive(Clone)]
struct ChainCtx {
    id: u32,
    n_nodes: u32,
    labels: BTreeMap<String, u32>,
}

/// A lowered value: the C type it carries plus the C text that reads it.
struct Val {
    ty: Ty,
    text: String,
}

impl Val {
    fn of(ty: Ty, text: String) -> Val {
        Val { ty, text }
    }
}

struct Cg<'a> {
    prog: &'a Program,
    hir: &'a HirCrate,
    sources: &'a Sources,
    cfg: &'a Config,
    /// `prog.fns` index → AST body, paired by item span (`hir::lower` walks `prog.fns` in order,
    /// so HIR function `k` is `prog.fns[k]` once no layout error occurred).
    bodies: BTreeMap<usize, &'a Block>,
    /// INFY type name → (C typedef name, index into `hir.items`).
    adt: BTreeMap<String, (String, usize)>,
    types: BTreeMap<String, TypeNode>,
    /// Definition order for the type region, computed by `order_types`.
    type_order: Vec<String>,
    /// ADT names whose definition is being computed (a by-value cycle is infinite size).
    defining: BTreeSet<String>,
    fptr_typedefs: BTreeSet<String>,
    /// Drop shims, keyed by shim symbol so each type gets exactly one (§4.7.4).
    shims: BTreeMap<String, String>,
    protos: BTreeMap<String, String>,
    /// §7.6.3 项 2: A-track wrapper keyed by the HIR symbol of the declaration it wraps. Built from
    /// `atrack` so the binding header and the call site cannot disagree about the symbol.
    atrack: BTreeMap<String, crate::atrack::Wrapper>,
    /// Same key, the §7.6.4 refusal text for a wrapper that could not be built.
    atrack_refused: BTreeMap<String, String>,
    fn_defs: Vec<String>,
    chain_defs: Vec<String>,
    /// `const`/`static` definitions, keyed by symbol so redeclaration is impossible.
    data_defs: BTreeMap<String, String>,
    /// The `main`/entry region, filled by `emit_driver`.
    driver: String,
    /// Runtime chain ids, in `hir::items` order — the same order `infy_chain_register_table` uses.
    chain_ids: BTreeMap<String, u32>,
    chain_item_idx: BTreeMap<String, usize>,
    chain_ctx: Option<ChainCtx>,
    /// The node number being emitted, so `goto(+n)` can resolve relative targets at compile time.
    cur_node_idx: Option<u32>,
    scopes: Vec<Vec<Binding>>,
    /// Cleanup pops that each currently open scope owes before it is left (§4.7.4). A slot goes to
    /// `None` when its binding is moved out, so "one entry pushed = one pop emitted" survives
    /// §4.7.1's three move forms.
    pops: Vec<Vec<Option<String>>>,
    errors: Vec<CgError>,
    tmp: u32,
    ret_ty: Option<Ty>,
    diverging: bool,
    /// The package whose item is being emitted, for §5.3's caller-first name resolution.
    cur_package: String,
    buf: String,
    depth: usize,
}

impl<'a> Cg<'a> {
    fn new(
        prog: &'a Program,
        hir: &'a HirCrate,
        files: &'a [(FileId, String, ParsedFile)],
        sources: &'a Sources,
        cfg: &'a Config,
    ) -> Cg<'a> {
        let mut spans: BTreeMap<FileId, Vec<(u32, &Item)>> = BTreeMap::new();
        for (id, _, pf) in files {
            let v = spans.entry(*id).or_default();
            for it in &pf.items {
                v.push((it.span.start, it));
            }
        }
        let mut bodies: BTreeMap<usize, &'a Block> = BTreeMap::new();
        for (i, f) in prog.fns.iter().enumerate() {
            if let Some((_, it)) = spans.get(&f.file).and_then(|v| {
                v.iter().find(|(start, it)| {
                    *start == f.span.start
                        && match &it.kind {
                            ItemKind::Fn { .. } | ItemKind::Event { .. } => true,
                            _ => false,
                        }
                })
            }) {
                if let ItemKind::Fn { body, .. } | ItemKind::Event { body, .. } = &it.kind {
                    if let Body::Block(b) = body {
                        bodies.insert(i, b);
                    }
                }
            }
        }

        let mut adt: BTreeMap<String, (String, usize)> = BTreeMap::new();
        let mut dup: Vec<String> = Vec::new();
        for (i, it) in hir.items.iter().enumerate() {
            let (name, symbol) = match it {
                HirItem::Struct(s) => (s.name.clone(), s.symbol.clone()),
                HirItem::Enum(e) => (e.name.clone(), e.symbol.clone()),
                _ => continue,
            };
            match adt.get(&name) {
                Some((prev, _)) if *prev != symbol => dup.push(format!("{name} ({prev} vs {symbol})")),
                _ => {
                    adt.insert(name, (symbol, i));
                }
            }
        }
        let mut errors: Vec<CgError> = Vec::new();
        if !dup.is_empty() {
            // `sema` types ADTs by simple name (`Ty::Named`), so two packages exporting the same
            // name cannot be distinguished downstream. Refuse instead of picking one.
            errors.push(CgError::new(
                "§4.1 / §5.5",
                format!("codegen: 跨 package 重名的 ADT 无法消歧：{}（后端按简单名索引类型）", dup.join(", ")),
            ));
        }
        // §7.6.3 项 2/3: build the A-track wrapper table once, from `atrack` — the same call `cli`
        // makes when it writes `infy_binding.h`, so the prototype in the generated C and the one in
        // the header cannot drift apart (a wrapper symbol mismatch is a *link* error, invisible to
        // every unit test on either side).
        let blocks = crate::atrack::signature_blocks_of(files);
        let mut atrack: BTreeMap<String, crate::atrack::Wrapper> = BTreeMap::new();
        let mut atrack_refused: BTreeMap<String, String> = BTreeMap::new();
        for (unit, (ws, refusals)) in crate::atrack::wrapper_plan(&hir.items, &blocks) {
            let decls = crate::atrack::unit_decls(&hir.items, &unit);
            for w in ws {
                if let Some(f) = decls.iter().find(|f| f.name == w.extern_symbol) {
                    atrack.insert(f.symbol.clone(), w.clone());
                }
            }
            for (name, text) in refusals {
                for f in decls.iter().filter(|f| f.name == name) {
                    atrack_refused.insert(f.symbol.clone(), text.clone());
                }
            }
        }
        Cg {
            prog,
            hir,
            sources,
            cfg,
            bodies,
            adt,
            types: BTreeMap::new(),
            type_order: Vec::new(),
            defining: BTreeSet::new(),
            fptr_typedefs: BTreeSet::new(),
            shims: BTreeMap::new(),
            protos: BTreeMap::new(),
            atrack,
            atrack_refused,
            fn_defs: Vec::new(),
            chain_defs: Vec::new(),
            data_defs: BTreeMap::new(),
            driver: String::new(),
            chain_ids: BTreeMap::new(),
            chain_item_idx: BTreeMap::new(),
            chain_ctx: None,
            cur_node_idx: None,
            scopes: Vec::new(),
            pops: Vec::new(),
            errors,
            tmp: 0,
            ret_ty: None,
            diverging: false,
            cur_package: String::new(),
            buf: String::new(),
            depth: 0,
        }
    }

    // ------------------------------------------------------------------ plumbing

    fn b(&mut self, line: &str) {
        self.buf.push_str(&"    ".repeat(self.depth));
        self.buf.push_str(line);
        self.buf.push('\n');
    }

    fn fresh(&mut self, stem: &str) -> String {
        self.tmp += 1;
        format!("{}_{}", c_ident(stem), self.tmp)
    }

    fn fail(&mut self, e: CgError) {
        if !self.errors.contains(&e) {
            self.errors.push(e);
        }
    }

    fn lookup(&self, name: &str) -> Option<&Binding> {
        self.scopes.iter().rev().find_map(|s| s.iter().rev().find(|b| b.infy == name))
    }

    /// `infy_current_node_loc()` is the §12.4 mandatory focus/chain/node triple source; the
    /// runtime tolerates a null loc outside a focus (`infy_rt.c:440`).
    fn loc(&mut self) -> String {
        "infy_current_node_loc()".to_string()
    }

    /// Declare a C variable for `ty`, honouring array declarators, and register the definition
    /// need for every container type involved.
    fn decl_stmt(&mut self, ty: &Ty, name: &str, init: Option<&str>) -> Result<(), CgError> {
        let mut deps = BTreeSet::new();
        let cty = self.c_ty(ty, false, &mut deps)?;
        let line = match init {
            Some(v) => format!("{} = {};", declarator(&cty, name), v),
            None => format!("{};", declarator(&cty, name)),
        };
        self.b(&line);
        Ok(())
    }

    /// Emit the drop-cleanup registration for a binding whose type has a destructor (§4.7.4:
    /// the order is emitted, not inferred; the deque is LIFO so scope exit pops in reverse).
    fn register_drop(&mut self, cvar: &str, ty: &Ty) -> Result<(), CgError> {
        if !self.needs_drop(ty)? {
            return Ok(());
        }
        let shim = self.drop_shim(ty)?;
        let slot = match self.pops.len().checked_sub(1) {
            Some(i) => {
                let pi = self.pops[i].len();
                self.pops[i].push(Some("infy_cleanup_pop_run();".to_string()));
                Some((i, pi))
            }
            None => None,
        };
        for sc in self.scopes.iter_mut().rev() {
            if let Some(b) = sc.iter_mut().find(|b| b.cvar == cvar) {
                b.cleanup = true;
                b.pop = slot;
                break;
            }
        }
        self.b(&format!("infy_cleanup_push({shim}, &{cvar}, 0);"));
        Ok(())
    }

    fn needs_drop(&mut self, ty: &Ty) -> Result<bool, CgError> {
        Ok(match ty {
            Ty::String | Ty::Vec { .. } | Ty::Boxed { .. } => true,
            Ty::Rc { .. } => {
                return Err(CgError::new(
                    "§4.7.4 / §10.13",
                    "codegen: `Rc`/`Arc` 的引用计数释放没有运行时入口（`infy_rt.h` 未提供），拒绝生成假 drop",
                ))
            }
            Ty::Array { inner, .. } | Ty::Slice { inner } => self.needs_drop(inner)?,
            Ty::Tuple(v) => {
                let mut any = false;
                for t in v {
                    any |= self.needs_drop(t)?;
                }
                any
            }
            Ty::Option { inner } => self.needs_drop(inner)?,
            Ty::Result { ok, err } => self.needs_drop(ok)? || self.needs_drop(err)?,
            Ty::Named(n) | Ty::Adt(n) => match self.adt.get(n) {
                Some((_, i)) => match &self.hir.items[*i] {
                    HirItem::Struct(s) => {
                        let mut any = s.has_drop;
                        for f in &s.fields {
                            any |= self.needs_drop(&f.ty)?;
                        }
                        any
                    }
                    HirItem::Enum(e) => {
                        let mut any = false;
                        for v in &e.variants {
                            for f in &v.payload {
                                any |= self.needs_drop(&f.ty)?;
                            }
                        }
                        any
                    }
                    _ => false,
                },
                None => false,
            },
            _ => false,
        })
    }

    /// The `InfyDropFn` shim for a drop-bearing type, emitted once per type in the helpers region.
    /// §4.7.4's order — body first, then fields in declaration order — is what the struct/enum
    /// shims below reproduce.
    fn drop_shim(&mut self, ty: &Ty) -> Result<String, CgError> {
        let name = format!("infy_drop_{}", sanitize(&ty.text()));
        if self.shims.contains_key(&name) {
            return Ok(name);
        }
        self.shims.insert(name.clone(), String::new());
        let mut deps = BTreeSet::new();
        let cty = self.c_ty(ty, false, &mut deps)?;
        let body = match ty {
            Ty::String => format!("infy_string_drop(({cty}*)p);"),
            Ty::Vec { inner } => {
                let esz = self.size_of(inner)?;
                let esh = if self.needs_drop(inner)? {
                    format!(", {}", self.drop_shim(inner)?)
                } else {
                    ", (InfyDropFn)0".to_string()
                };
                format!("infy_vec_drop(({cty}*)p, {esz}u{esh});")
            }
            Ty::Boxed { inner } => {
                if self.needs_drop(inner)? {
                    let sh = self.drop_shim(inner)?;
                    format!("{sh}(p); infy_free(p);")
                } else {
                    "infy_free(p);".to_string()
                }
            }
            Ty::Named(n) | Ty::Adt(n) => {
                let i = self.adt_get(n)?.1;
                let mut out = String::from("infy_t* p = (infy_t*)obj; (void)p;");
                out = out.replace("infy_t", &cty);
                match &self.hir.items[i] {
                    HirItem::Struct(s) => {
                        for f in &s.fields {
                            if self.needs_drop(&f.ty)? {
                                let sh = self.drop_shim(&f.ty)?;
                                out.push_str(&format!(" {sh}(&p->{});", c_ident(&f.name)));
                            }
                        }
                    }
                    HirItem::Enum(e) => {
                        for v in &e.variants {
                            let mut inner_body = String::new();
                            for (k, f) in v.payload.iter().enumerate() {
                                if self.needs_drop(&f.ty)? {
                                    let sh = self.drop_shim(&f.ty)?;
                                    inner_body.push_str(&format!(" {sh}(&p->v.{}.a{k});", c_ident(&v.name)));
                                }
                            }
                            if !inner_body.is_empty() {
                                out.push_str(&format!(" if (p->tag == {}u) {{ {inner_body} }}", v.discriminant));
                            }
                        }
                    }
                    _ => {}
                }
                out
            }
            other => {
                return Err(CgError::new(
                    "§4.7.4",
                    format!("codegen: {} 的析构发射未实现", other.text()),
                ))
            }
        };
        self.shims.insert(name.clone(), format!("static void {name}(void* obj) {{ {cty}* p = ({cty}*)obj; {body} }}"));
        Ok(name)
    }

    fn size_of(&mut self, ty: &Ty) -> Result<u64, CgError> {
        let mut deps = BTreeSet::new();
        let _ = self.c_ty(ty, false, &mut deps)?;
        let mut l = crate::hir::Layouter::new(self.cfg.target, self.prog);
        l.layout(ty)
            .map(|x| x.size)
            .map_err(|e| CgError::new("§4.8", format!("codegen: 布局失败：{}", e.message())))
    }

    fn align_of(&mut self, ty: &Ty) -> Result<u64, CgError> {
        let mut l = crate::hir::Layouter::new(self.cfg.target, self.prog);
        l.layout(ty)
            .map(|x| x.align)
            .map_err(|e| CgError::new("§4.8", format!("codegen: 布局失败：{}", e.message())))
    }

    fn layout(&mut self, ty: &Ty) -> Result<Layout, CgError> {
        let mut l = crate::hir::Layouter::new(self.cfg.target, self.prog);
        l.layout(ty)
            .map_err(|e| CgError::new("§4.8", format!("codegen: 布局失败：{}", e.message())))
    }

    // ------------------------------------------------------------------ type mapping

    /// The C type name for `ty`. `indirect` marks a use through a pointer, where only the forward
    /// typedef is needed and no definition-order dependency is recorded.
    fn c_ty(&mut self, ty: &Ty, indirect: bool, deps: &mut BTreeSet<String>) -> Result<String, CgError> {
        Ok(match ty {
            Ty::Prim(p) => self.prim_c(*p).to_string(),
            Ty::Bool => "InfyBool".to_string(),
            Ty::Unit => "InfyUnit".to_string(),
            Ty::Str => "InfyStr".to_string(),
            Ty::String => "InfyString".to_string(),
            Ty::Named(n) | Ty::Adt(n) => {
                let (sym, idx) = self.adt_get(n)?;
                if self.types.get(&sym).map(|t| t.def.is_none()).unwrap_or(true) {
                    self.intern_adt(idx)?;
                }
                if !indirect {
                    deps.insert(sym.clone());
                }
                sym
            }
            Ty::Generic(n) => {
                return Err(CgError::new(
                    "§4.15",
                    format!("codegen: 类型参数 `{n}` 未被单态化替换（§4.15 的单态化未实现）"),
                ))
            }
            Ty::Ref { inner, mutable, .. } | Ty::Ptr { inner, mutable } => {
                let t = self.c_ty(inner, true, deps)?;
                format!("{}{}*", if *mutable { "" } else { "const " }, t)
            }
            Ty::Boxed { inner } => {
                let t = self.c_ty(inner, true, deps)?;
                format!("{t}*")
            }
            Ty::Rc { inner, atomic } => {
                let name = self.intern_rc(inner, *atomic)?;
                if !indirect {
                    deps.insert(name.clone());
                }
                name
            }
            // The frozen header's `InfyVec`/`InfySlice` are untyped; element size and alignment
            // travel with each call (docs/internals/runtime-abi.md §1 synced by C2).
            Ty::Vec { .. } => "InfyVec".to_string(),
            Ty::Slice { .. } => "InfySlice".to_string(),
            Ty::Array { inner, n } => {
                let t = self.c_ty(inner, false, deps)?;
                format!("{t}[{n}]")
            }
            Ty::Tuple(v) => {
                let name = self.intern_tuple(v)?;
                if !indirect {
                    deps.insert(name.clone());
                }
                name
            }
            Ty::Option { inner } => {
                if Cg::niche(inner) {
                    self.c_ty(inner, indirect, deps)?
                } else {
                    let name = self.intern_option(inner)?;
                    if !indirect {
                        deps.insert(name.clone());
                    }
                    name
                }
            }
            Ty::Result { ok, err } => {
                let name = self.intern_result(ok, err)?;
                if !indirect {
                    deps.insert(name.clone());
                }
                name
            }
            Ty::Fn { params, ret } => self.intern_fnptr(params, ret.as_deref())?,
            Ty::Dyn(_) => {
                return Err(CgError::new(
                    "§4.1 / §7.7.7",
                    "codegen: `dyn Trait` 的 vtable 发射未实现（偏移表定在 `docs/internals/runtime-abi.md` §1 与 \
                     `docs/spec-deviations.md` D-50(c)，缺的是发射点和 `.inf meta` 的槽位，见 docs/BACKLOG.md F4）",
                ))
            }
            Ty::Closure(_) => {
                return Err(CgError::new(
                    "§4.1 / §3.8",
                    "codegen: 闭包未实现（捕获布局与 `InfyCloseret` 箱属未做项，见 docs/BACKLOG.md）",
                ))
            }
            Ty::Auto => {
                return Err(CgError::new("§2.6", "codegen: `auto` 不是类型，只能出现在具名实参位置"))
            }
            Ty::Never => "void".to_string(),
            // §4.13 rule 7: an unsuffixed decimal literal that no context constrained defaults to
            // `i32`, so this is the one place the default type is resolved.
            Ty::IntLit => "int32_t".to_string(),
            Ty::Invalid => {
                return Err(CgError::new(
                    "§4.2",
                    "codegen: 遇到未解析类型；阶段 [4] 应已阻断，拒绝生成猜测的表示",
                ))
            }
        })
    }

    fn adt_get(&self, n: &str) -> Result<(String, usize), CgError> {
        self.adt
            .get(n)
            .map(|(s, i)| (s.clone(), *i))
            .ok_or_else(|| CgError::new("§4.1", format!("codegen: 类型 `{n}` 不在 HIR 的项表里（阶段 [7] 未落成表示）")))
    }

    fn prim_c(&self, p: Prim) -> &'static str {
        match p {
            Prim::U8 => "uint8_t",
            Prim::U16 => "uint16_t",
            Prim::U32 => "uint32_t",
            Prim::U64 => "uint64_t",
            Prim::U128 => "InfyU128",
            Prim::I8 => "int8_t",
            Prim::I16 => "int16_t",
            Prim::I32 => "int32_t",
            Prim::I64 => "int64_t",
            Prim::I128 => "InfyI128",
            Prim::F32 => "float",
            Prim::F64 => "double",
            Prim::Bool => "InfyBool",
            Prim::Str => "InfyStr",
            Prim::Char => "InfyChar",
            Prim::Unit => "InfyUnit",
            Prim::Usize => "size_t",
            Prim::Isize => "ptrdiff_t",
        }
    }

    /// §4.1 / ABI §1: the `Option` niche applies to `&T`, `Box<U>`, `*const U`, fn pointers.
    fn niche(ty: &Ty) -> bool {
        matches!(ty, Ty::Ref { .. } | Ty::Ptr { .. } | Ty::Boxed { .. } | Ty::Fn { .. })
    }

    fn reserve(&mut self, name: &str) {
        self.types
            .entry(name.to_string())
            .or_insert_with(|| TypeNode { def: None, deps: BTreeSet::new() });
    }

    fn defined(&self, name: &str) -> bool {
        self.types.get(name).map(|t| t.def.is_some()).unwrap_or(false)
    }

    fn intern_adt(&mut self, idx: usize) -> Result<String, CgError> {
        match &self.hir.items[idx] {
            HirItem::Struct(s) => self.intern_struct(idx, s.name.clone(), s.symbol.clone()),
            HirItem::Enum(e) => self.intern_enum(idx, e.name.clone(), e.symbol.clone()),
            _ => Err(CgError::new("§4.1", "codegen: 内部错误：ADT 索引指向非 ADT 项")),
        }
    }

    fn intern_struct(&mut self, idx: usize, name: String, symbol: String) -> Result<String, CgError> {
        if self.defined(&symbol) {
            return Ok(symbol);
        }
        if self.defining.contains(&symbol) {
            return Err(CgError::new(
                "§4.8",
                format!("codegen: `{name}` 通过值递归包含自身，没有有限布局（阶段 [7] 也应已拒绝）"),
            ));
        }
        self.reserve(&symbol);
        self.defining.insert(symbol.clone());
        let hir = &self.hir.items[idx];
        let HirItem::Struct(s) = hir else {
            self.defining.remove(&symbol);
            return Err(CgError::new("§4.1", "codegen: 内部错误：非 struct 项"));
        };
        let (ty, layout) = (Ty::Adt(name.clone()), s.layout);
        let mut deps = BTreeSet::new();
        let mut fields = String::new();
        let mut asserts = String::new();
        for f in &s.fields {
            let ft = self.c_ty(&f.ty, false, &mut deps)?;
            fields.push_str(&format!(" {};", declarator(&ft, &c_ident(&f.name))));
            asserts.push_str(&format!(
                "\ntypedef char infy_off_{symbol}_{f}[(offsetof(struct infy_s_{symbol}, {f}) == {o}u) ? 1 : -1];",
                f = c_ident(&f.name),
                o = f.offset
            ));
        }
        if s.fields.is_empty() {
            // §1 / ABI §1: a fieldless struct is the ZST `InfyUnit`, never "nothing".
            fields.push_str(" uint8_t _0;");
        }
        let def = format!(
            "struct infy_s_{symbol} {{{fields}}};\ntypedef struct infy_s_{symbol} {symbol};{layout_assert}{offsets}",
            layout_assert = self.layout_assert_text(&symbol, layout),
            offsets = asserts
        );
        self.defining.remove(&symbol);
        self.types.insert(symbol.clone(), TypeNode { def: Some(def), deps });
        let _ = ty;
        Ok(symbol)
    }

    fn intern_enum(&mut self, idx: usize, name: String, symbol: String) -> Result<String, CgError> {
        if self.defined(&symbol) {
            return Ok(symbol);
        }
        if self.defining.contains(&symbol) {
            return Err(CgError::new(
                "§4.8",
                format!("codegen: `{name}` 通过值递归包含自身，没有有限布局"),
            ));
        }
        self.reserve(&symbol);
        self.defining.insert(symbol.clone());
        let hir = &self.hir.items[idx];
        let HirItem::Enum(e) = hir else {
            self.defining.remove(&symbol);
            return Err(CgError::new("§4.1", "codegen: 内部错误：非 enum 项"));
        };
        let (layout, has_payload) = (e.layout, e.variants.iter().any(|v| !v.payload.is_empty()));
        let mut deps = BTreeSet::new();
        let mut members = String::new();
        for v in &e.variants {
            if v.payload.is_empty() {
                continue;
            }
            let pt = self.payload_type(&symbol, &v.name, &v.payload, &mut deps)?;
            members.push_str(&format!(" {} {};", pt, c_ident(&v.name)));
        }
        let body = if has_payload {
            // ABI §1: `{ uint32_t tag; union { … } v; }`, the union elided when no variant carries
            // a payload (D-24), which is why `Phase` is exactly 4/4.
            format!(" uint32_t tag; union infy_u_{symbol} {{ {members} }} v;")
        } else {
            " uint32_t tag;".to_string()
        };
        let def = format!(
            "struct infy_s_{symbol} {{{body}}};\ntypedef struct infy_s_{symbol} {symbol};{assert}",
            assert = self.layout_assert_text(&symbol, layout)
        );
        self.defining.remove(&symbol);
        self.types.insert(symbol.clone(), TypeNode { def: Some(def), deps });
        let _ = name;
        Ok(symbol)
    }

    /// The payload record of one variant (D-24: tuple variants are a C record with fields `a0`…).
    fn payload_type(
        &mut self,
        enum_symbol: &str,
        variant: &str,
        payload: &[crate::hir::HirField],
        outer: &mut BTreeSet<String>,
    ) -> Result<String, CgError> {
        let name = format!("{enum_symbol}_{variant}_payload");
        let name = name.replace("::", "_");
        if !self.defined(&name) {
            self.reserve(&name);
            let mut deps = BTreeSet::new();
            let mut fields = String::new();
            for (i, f) in payload.iter().enumerate() {
                let ft = self.c_ty(&f.ty, false, &mut deps)?;
                fields.push_str(&format!(" {};", declarator(&ft, &format!("a{i}"))));
            }
            let lay = crate::hir::record_layout(&payload.iter().map(|f| f.layout).collect::<Vec<_>>()).layout;
            let def = format!(
                "struct infy_s_{name} {{{fields}}};\ntypedef struct infy_s_{name} {name};{assert}",
                assert = self.layout_assert_text(&name, lay)
            );
            self.types.insert(name.clone(), TypeNode { def: Some(def), deps });
        }
        outer.insert(name.clone());
        Ok(name)
    }

    fn layout_assert_text(&mut self, c_name: &str, l: Layout) -> String {
        format!(
            "\ntypedef char infy_layout_{c_name}[(sizeof({c_name}) == {}u && _Alignof({c_name}) == {}u) ? 1 : -1];",
            l.size, l.align
        )
    }

    fn intern_result(&mut self, ok: &Ty, err: &Ty) -> Result<String, CgError> {
        let ty = Ty::Result { ok: Box::new(ok.clone()), err: Box::new(err.clone()) };
        let name = self.synth_name("Result", &ty);
        if !self.defined(&name) {
            self.reserve(&name);
            let mut deps = BTreeSet::new();
            let kt = self.c_ty(ok, false, &mut deps)?;
            let et = self.c_ty(err, false, &mut deps)?;
            let lay = self.layout(&ty)?;
            // ABI §1 field order — `ok`, `err`, `is_ok` — is exactly what `hir::record_layout` folds.
            let def = format!(
                "struct infy_s_{name} {{ {kt} ok; {et} err; uint8_t is_ok; }};\ntypedef struct infy_s_{name} {name};{assert}",
                assert = self.layout_assert_text(&name, lay)
            );
            self.types.insert(name.clone(), TypeNode { def: Some(def), deps });
        }
        Ok(name)
    }

    fn intern_option(&mut self, inner: &Ty) -> Result<String, CgError> {
        let ty = Ty::Option { inner: Box::new(inner.clone()) };
        let name = self.synth_name("Option", &ty);
        if !self.defined(&name) {
            self.reserve(&name);
            let mut deps = BTreeSet::new();
            let it = self.c_ty(inner, false, &mut deps)?;
            let lay = self.layout(&ty)?;
            let def = format!(
                "struct infy_s_{name} {{ {it} val; uint8_t some; }};\ntypedef struct infy_s_{name} {name};{assert}",
                assert = self.layout_assert_text(&name, lay)
            );
            self.types.insert(name.clone(), TypeNode { def: Some(def), deps });
        }
        Ok(name)
    }

    fn intern_tuple(&mut self, fields: &[Ty]) -> Result<String, CgError> {
        let ty = Ty::Tuple(fields.to_vec());
        let name = self.synth_name("Tuple", &ty);
        if !self.defined(&name) {
            self.reserve(&name);
            let mut deps = BTreeSet::new();
            let mut decls = String::new();
            for (i, f) in fields.iter().enumerate() {
                let ct = self.c_ty(f, false, &mut deps)?;
                decls.push_str(&format!(" {};", declarator(&ct, &format!("a{i}"))));
            }
            let lay = self.layout(&ty)?;
            let def = format!(
                "struct infy_s_{name} {{{decls} }};\ntypedef struct infy_s_{name} {name};{assert}",
                assert = self.layout_assert_text(&name, lay)
            );
            self.types.insert(name.clone(), TypeNode { def: Some(def), deps });
        }
        Ok(name)
    }

    fn intern_rc(&mut self, inner: &Ty, atomic: bool) -> Result<String, CgError> {
        let ty = Ty::Rc { inner: Box::new(inner.clone()), atomic };
        let name = self.synth_name(if atomic { "Arc" } else { "Rc" }, &ty);
        if !self.defined(&name) {
            self.reserve(&name);
            let mut deps = BTreeSet::new();
            let it = self.c_ty(inner, true, &mut deps)?;
            let count = if atomic { "_Atomic uint64_t*" } else { "uint64_t*" };
            let lay = self.layout(&ty)?;
            let def = format!(
                "struct infy_s_{name} {{ {it} data; {count} strong; }};\ntypedef struct infy_s_{name} {name};{assert}",
                assert = self.layout_assert_text(&name, lay)
            );
            self.types.insert(name.clone(), TypeNode { def: Some(def), deps });
        }
        Ok(name)
    }

    fn intern_fnptr(&mut self, params: &[Ty], ret: Option<&Ty>) -> Result<String, CgError> {
        let ty = Ty::Fn { params: params.to_vec(), ret: ret.cloned().map(Box::new) };
        let name = self.synth_name("Fn", &ty);
        let mut deps = BTreeSet::new();
        let rt = match ret {
            Some(t) => self.c_ty(t, false, &mut deps)?,
            None => "void".to_string(),
        };
        let mut ps = Vec::new();
        for p in params {
            ps.push(self.c_ty(p, false, &mut deps)?);
        }
        // Function-pointer typedefs live in their own region (after the forwards they mention), not
        // in the struct-definition region.
        self.fptr_typedefs.insert(format!(
            "typedef {rt} (*{name})({});",
            if ps.is_empty() { "void".to_string() } else { ps.join(", ") }
        ));
        Ok(name)
    }

    /// `Ty::text()` is already kind-qualified (`Result<u32, Fail>`), so the sanitized text alone is
    /// a unique, deterministic C name for the instantiation (§7.3 byte-stable output).
    fn synth_name(&mut self, _prefix: &str, ty: &Ty) -> String {
        format!("Infy{}", sanitize(&ty.text()))
    }

    // ------------------------------------------------------------------ driver

    fn run(&mut self) {
        if !self.hir.errors.is_empty() {
            for e in self.hir.errors.iter() {
                self.fail(CgError::new(
                    "§4.8 / 阶段 [7]",
                    format!("codegen: 拒绝在布局失败的 HIR 上生成：{}", e.message()),
                ));
            }
            return;
        }
        // Assign chain ids first: `goto`/`start` need them while node bodies are emitted, and the
        // runtime numbers chains by registration order (`infy_chain_register`, `infy_rt.c:1893`).
        let mut cid = 0u32;
        for (i, it) in self.hir.items.iter().enumerate() {
            if let HirItem::Chain(c) = it {
                self.chain_ids.insert(c.name.clone(), cid);
                self.chain_item_idx.insert(c.name.clone(), i);
                cid += 1;
            }
        }
        for (i, it) in self.hir.items.iter().enumerate() {
            match it {
                HirItem::Struct(_) | HirItem::Enum(_) => {
                    if let Err(e) = self.intern_adt(i) {
                        self.fail(e);
                    }
                }
                HirItem::Const(c) => {
                    let c = c.clone();
                    if let Err(e) = self.emit_const(&c) {
                        self.fail(e);
                    }
                }
                _ => {}
            }
        }
        // Functions: HIR pushed them in `prog.fns` order, so the k-th `Fn` item is `prog.fns[k]`
        // (guaranteed because `hir.errors` is empty above).
        let mut fn_items = self
            .hir
            .items
            .iter()
            .enumerate()
            .filter(|(_, it)| matches!(it, HirItem::Fn(_)))
            .collect::<Vec<_>>();
        if fn_items.len() != self.prog.fns.len() {
            self.fail(CgError::new(
                "§7.1 [7]",
                format!(
                    "codegen: HIR 只给了 {} 个函数，阶段 [4] 有 {} 个，无法配对函数体",
                    fn_items.len(),
                    self.prog.fns.len()
                ),
            ));
            return;
        }
        for ((prog_idx, _info), (hir_idx, _)) in self.prog.fns.iter().enumerate().zip(fn_items.drain(..)) {
            let item = self.hir.items[hir_idx].clone();
            if let HirItem::Fn(f) = item {
                let body = self.bodies.get(&prog_idx).copied();
                let sig_name = format!("{}::{}", f.package, f.name);
                match body {
                    Some(b) => {
                        if let Err(e) = self.emit_fn(&f, b) {
                            let _ = &sig_name;
                            self.fail(e);
                        }
                    }
                    None => {
                        if !f.is_foreign {
                            self.fail(CgError::new(
                                "§7.1 [8]",
                                format!("codegen: 找不到 `{sig_name}` 的函数体（阶段 [8] 按 item span 配对）"),
                            ));
                        } else if let Err(e) = self.emit_foreign_decl(&f) {
                            self.fail(e);
                        }
                    }
                }
            }
        }
        let mut chain_idx = 0usize;
        for (i, it) in self.hir.items.iter().enumerate() {
            if let HirItem::Chain(c) = it {
                let Some(chain) = self.prog.chains.get(chain_idx).cloned() else {
                    self.fail(CgError::new(
                        "§8.4",
                        format!("codegen: 链 `{}` 在阶段 [4] 的记录里不存在", c.name),
                    ));
                    continue;
                };
                chain_idx += 1;
                let _ = i;
                let c = c.clone();
                if let Err(e) = self.emit_chain(&c, &chain) {
                    self.fail(e);
                }
            }
        }
        if chain_idx != self.prog.chains.len() {
            self.fail(CgError::new(
                "§8.4",
                format!(
                    "codegen: HIR 给了 {chain_idx} 条链，阶段 [4] 有 {} 条",
                    self.prog.chains.len()
                ),
            ));
        }
        if self.errors.is_empty() {
            if let Err(e) = self.emit_driver() {
                self.fail(e);
            }
        }
        self.order_types();
    }

    /// Definition order for the type region: a type that contains another **by value** must be
    /// defined first, so `deps` becomes the edge set. Ties break by name, which keeps the emitted
    /// C byte-stable for identical input (§7.3).
    fn order_types(&mut self) {
        let mut remaining: BTreeSet<String> = self.types.keys().cloned().collect();
        let mut done: BTreeSet<String> = BTreeSet::new();
        while !remaining.is_empty() {
            let ready = remaining
                .iter()
                .find(|n| {
                    self.types
                        .get(*n)
                        .map(|t| t.deps.iter().all(|d| done.contains(d) || !remaining.contains(d)))
                        .unwrap_or(false)
                })
                .cloned();
            match ready {
                Some(n) => {
                    self.type_order.push(n.clone());
                    remaining.remove(&n);
                    done.insert(n);
                }
                None => {
                    let stuck = remaining.iter().cloned().collect::<Vec<_>>().join(", ");
                    self.fail(CgError::new(
                        "§4.8",
                        format!("codegen: 类型定义顺序出现环（通过值互相包含，没有有限布局）：{stuck}"),
                    ));
                    return;
                }
            }
        }
    }

    fn assemble(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "/* Generated by `infy build` stage [8] (INFY v2.0, IFABI-{} , target {}). */\n\
             /* Source module: {} */\n\
             /* Do not edit. Rebuilding the same inputs reproduces this text byte for byte (§7.3). */\n\
             #include <stddef.h>\n\
             #include \"{}\"\n\n",
            "2",
            self.cfg.target_triple,
            self.cfg.module,
            self.cfg.header_include
        ));
        out.push_str("/* ---- forward typedefs (ABI §2: one symbol per INFY item) ---- */\n");
        for name in &self.type_order {
            out.push_str(&format!("struct infy_s_{name};\ntypedef struct infy_s_{name} {name};\n"));
        }
        out.push('\n');
        for t in &self.fptr_typedefs {
            out.push_str(t);
            out.push('\n');
        }
        out.push_str("\n/* ---- type definitions, in dependency order ---- */\n");
        for name in &self.type_order {
            if let Some(Some(def)) = self.types.get(name).map(|t| t.def.clone()) {
                out.push_str(&def);
                out.push('\n');
            }
        }
        if !self.shims.is_empty() {
            out.push_str("\n/* ---- drop shims (§4.7.4: the order is emitted, not inferred) ---- */\n");
            for (_, s) in &self.shims {
                if !s.is_empty() {
                    out.push_str(s);
                    out.push('\n');
                }
            }
        }
        if !self.data_defs.is_empty() {
            out.push_str("\n/* ---- consts and statics (§4.5, §2.12) ---- */\n");
            for (_, d) in &self.data_defs {
                out.push_str(d);
                out.push('\n');
            }
        }
        out.push_str("\n/* ---- function prototypes ---- */\n");
        for (_, p) in &self.protos {
            out.push_str(p);
            out.push('\n');
        }
        out.push_str("\n/* ---- functions ---- */\n");
        for d in &self.fn_defs {
            out.push_str(d);
        }
        if !self.chain_defs.is_empty() {
            out.push_str("\n/* ---- chains (§8.4 node tables) ---- */\n");
            for d in &self.chain_defs {
                out.push_str(d);
            }
        }
        out.push_str(&self.driver);
        out
    }

    // ------------------------------------------------------------------ consts

    fn emit_const(&mut self, c: &crate::hir::HirConst) -> Result<(), CgError> {
        let mut deps = BTreeSet::new();
        let cty = self.c_ty(&c.ty, false, &mut deps)?;
        let sym = c.symbol.clone();
        match c.value {
            Some(v) => {
                let init = self.const_text(v, &c.ty)?;
                let text = if c.is_static {
                    format!("static {cty} {sym} = {init};")
                } else {
                    format!("static const {cty} {sym} = {init};")
                };
                self.data_defs.insert(sym, text);
                Ok(())
            }
            None => Err(CgError::new(
                "§2.12 / §4.5",
                format!(
                    "codegen: `{}` 的初始值不在 §2.12 的闭集里，后端不得用默认值替换（阶段 [7] 已如实记为 None）",
                    c.name
                ),
            )),
        }
    }

    fn const_text(&mut self, v: ConstValue, ty: &Ty) -> Result<String, CgError> {
        Ok(match v {
            ConstValue::Bool(b) => if b { "1u".to_string() } else { "0u".to_string() },
            ConstValue::Char(c) => format!("UINT32_C({})", c as u32),
            ConstValue::Float(f) => {
                if matches!(ty, Ty::Prim(Prim::F32)) {
                    format!("{f}_f")
                } else {
                    format!("{f}")
                }
            }
            ConstValue::Int(i) => {
                let u: u128 = if i < 0 {
                    (-(i as i128)) as u128
                } else {
                    i as u128
                };
                let text = self.numeric_text(u, ty)?;
                if i < 0 {
                    format!("(-{text})")
                } else {
                    text
                }
            }
            ConstValue::UInt(u) => self.numeric_text(u, ty)?,
        })
    }

    // ------------------------------------------------------------------ functions

    /// A-track `extern` declarations (§7.6.3 项 2/3): the signature check belongs to stage [4], the
    /// wrapper is emitted by `atrack` into `<build>/a_track/<name>.c`, and here the generated TU only
    /// needs the `extern` prototype of that wrapper. A declaration whose signature §7.6.4 cannot map
    /// is refused *when it is called* (see `user_fn_call`), because an uncalled declaration costs
    /// nothing and refusing it would break a module that merely declares it behind a `//infy:build`.
    fn emit_foreign_decl(&mut self, f: &crate::hir::HirFn) -> Result<(), CgError> {
        let w = match self.atrack.get(&f.symbol) {
            Some(w) => w,
            None => {
                // Two different reasons for having no wrapper, and the message must name the right
                // one: §7.6.4's table has rows this back end cannot emit yet (a legal program that
                // stops here), versus §7.6.1's "a declaration no signature block claims".
                if let Some(text) = self.atrack_refused.get(&f.symbol) {
                    return Err(CgError::new("§7.6.4", text.clone()));
                }
                return Err(CgError::new(
                    "§7.6.1",
                    format!(
                        "codegen: A 轨声明 `{}` 不在任何 `#[link(name = \"…\")]` 签名块里，没有包装符号可发射（§7.6.1 要求签名块与源码块成对）",
                        f.name
                    ),
                ));
            }
        };
        // The key is the wrapper symbol, not `f.symbol`: the INFY-side symbol is never defined by
        // anybody, and emitting it would produce an unresolved reference at link time.
        self.protos
            .insert(w.symbol.clone(), format!("extern {};", w.prototype()));
        Ok(())
    }

    fn emit_fn(&mut self, f: &crate::hir::HirFn, body: &Block) -> Result<(), CgError> {
        if f.is_event {
            return Err(CgError::new(
                "§3.3 / §11",
                format!(
                    "codegen: `event` `{}` 由宿主实现，调用需走 `infy_host_call_event` 与 `InfyArgFrame` 打包（`src/host.rs` 仍是占位）",
                    f.name
                ),
            ));
        }
        let mut deps = BTreeSet::new();
        let ret_cty = match &f.ret {
            Some((t, _)) => self.c_ty(t, false, &mut deps)?,
            None if f.diverges => "void".to_string(),
            None => "InfyUnit".to_string(),
        };
        // §2 of the ABI doc: locals keep numbered SSA-ish names. Parameters get their numbers
        // first, so the text is a deterministic function of the declaration order.
        self.scopes.push(Vec::new());
        self.pops.push(Vec::new());
        let mut ps = Vec::new();
        let mut owned = Vec::new();
        for p in &f.params {
            let pt = self.c_ty(&p.ty, false, &mut deps)?;
            let name = self.fresh(&p.name);
            ps.push(declarator(&pt, &name));
            self.scopes[0].push(Binding {
                infy: p.name.clone(),
                cvar: name.clone(),
                ty: p.ty.clone(),
                cleanup: false,
                pop: None,
            });
            // The cleanup entry belongs to the function's own scope, so it is pushed *inside* the
            // body — registering it before the signature would put it in the previous item's buffer.
            if self.needs_drop(&p.ty)? {
                owned.push((name, p.ty.clone()));
            }
        }
        let sig = format!(
            "{ret_cty} {}({})",
            f.symbol,
            if ps.is_empty() { "void".to_string() } else { ps.join(", ") }
        );
        self.protos
            .insert(f.symbol.clone(), format!("{sig};"));
        let text = {
            self.buf.clear();
            self.depth = 0;
            self.b(&format!("{sig} {{"));
            self.depth = 1;
            for (cvar, ty) in &owned {
                self.register_drop(cvar, ty)?;
            }
            self.ret_ty = f.ret.as_ref().map(|(t, _)| t.clone());
            self.diverging = f.diverges;
            self.cur_package = f.package.clone();
            self.block_body(body, f.ret.as_ref().map(|(t, _)| t.clone()).as_ref(), true)?;
            self.depth = 0;
            self.b("}");
            std::mem::take(&mut self.buf)
        };
        self.scopes.clear();
        self.pops.clear();
        self.fn_defs.push(text);
        Ok(())
    }

    /// Emit a block's statements and its tail expression into the current buffer.
    ///
    /// `fn_body` marks the function's own block: only there does the tail become `return`, and only
    /// there is a missing tail for a value-returning function an error. A nested block (`if`,
    /// `while`, an arm body) discards its tail — emitting `return (InfyUnit){0}` there would put a
    /// wrong-typed return inside a function that returns something else.
    fn block_body(&mut self, body: &Block, want: Option<&Ty>, fn_body: bool) -> Result<(), CgError> {
        let want_owned = want.cloned();
        for s in &body.stmts {
            self.stmt(s)?;
        }
        match (&body.tail, want_owned.as_ref()) {
            (Some(t), Some(_)) if fn_body => {
                let v = self.expr(t, want_owned.as_ref())?;
                self.cancel_moved_return(&v)?;
                self.drain_pops();
                self.b(&format!("return {};", v.text));
                Ok(())
            }
            (Some(t), _) => {
                let v = self.expr(t, if fn_body { want_owned.as_ref() } else { None })?;
                if fn_body {
                    self.cancel_moved_return(&v)?;
                    self.drain_pops();
                    self.b(&format!("return {};", v.text));
                } else {
                    self.b(&format!("(void){};", v.text));
                }
                Ok(())
            }
            (None, Some(_)) if fn_body => Err(CgError::new(
                "§3.5 / §4.2",
                "codegen: 有返回类型的函数体缺少尾表达式（阶段 [4] 应已报 E-0110）",
            )),
            (None, None) if fn_body => {
                if !self.diverging {
                    self.drain_pops();
                    self.b("return (InfyUnit){0};");
                }
                Ok(())
            }
            (None, _) => Ok(()),
        }
    }

    /// The binding a §4.7.1 move names: `s`, or the explicit `move s`. A field, a call result or a
    /// literal produces a value rather than transferring an owned binding.
    fn moved_binding(e: &Expr) -> Option<&str> {
        match &e.kind {
            ExprKind::Ident(n) => Some(n.as_str()),
            ExprKind::Move { expr } => match &expr.kind {
                ExprKind::Ident(n) => Some(n.as_str()),
                _ => None,
            },
            _ => None,
        }
    }

    /// §4.7.1 + §4.7.4: hand the buffer to the destination. `infy_cleanup_cancel` removes the entry
    /// registered at the **source** address without running it, and the `pop_run` line this back end
    /// owed for that entry is cleared in the same step — one entry per pop is the invariant
    /// `close_scope` / `drain_pops` rely on, so the two halves must always move together.
    fn cancel_cleanup_of(&mut self, cvar: &str, name: &str) {
        let slot = self
            .scopes
            .iter()
            .rev()
            .find_map(|s| s.iter().rev().find(|b| b.cvar == cvar && b.cleanup))
            .and_then(|b| b.pop);
        let Some((si, pi)) = slot else { return };
        for sc in self.scopes.iter_mut().rev() {
            if let Some(b) = sc.iter_mut().find(|b| b.cvar == cvar && b.cleanup) {
                b.cleanup = false;
                b.pop = None;
                break;
            }
        }
        if let Some(v) = self.pops.get_mut(si) {
            if pi < v.len() {
                v[pi] = None;
            }
        }
        self.b(&format!(
            "/* §4.7.1 移动：`{name}` 交出所有权，清理项按地址取消，由新所有者在其作用域释放（§4.7.4）*/"
        ));
        self.b(&format!("infy_cleanup_cancel(&{cvar});"));
    }

    /// §4.7.1: `let d: String = s;` and an owned by-value argument both re-seat the buffer, so the
    /// source's entry must leave the deque. A binding that never registered one has nothing to
    /// cancel, and a chain frame slot cannot hold a drop-bearing type (`emit_chain` refuses it).
    fn cancel_owned_move(&mut self, e: &Expr, ty: &Ty) -> Result<(), CgError> {
        if !self.needs_drop(ty)? {
            return Ok(());
        }
        let Some(name) = Self::moved_binding(e) else { return Ok(()) };
        let hit = self
            .scopes
            .iter()
            .rev()
            .find_map(|s| s.iter().rev().find(|b| b.infy == name && b.cleanup))
            .map(|b| b.cvar.clone());
        if let Some(cvar) = hit {
            self.cancel_cleanup_of(&cvar, name);
        }
        Ok(())
    }

    /// §4.7.4: a value that leaves through `return` is dropped by its new owner, so every cleanup
    /// entry its C text still carries by value must be cancelled — **before** `drain_pops()` emits
    /// what the deque owes. `&s` is a borrow, not a move, so `mentions_by_value` skips it:
    /// cancelling there would leak the buffer instead of double-freeing it.
    fn cancel_moved_return(&mut self, v: &Val) -> Result<(), CgError> {
        if !self.needs_drop(&v.ty)? {
            return Ok(());
        }
        let hits: Vec<(String, String)> = self
            .scopes
            .iter()
            .rev()
            .flatten()
            .filter(|b| b.cleanup && mentions_by_value(&v.text, &b.cvar))
            .map(|b| (b.cvar.clone(), b.infy.clone()))
            .collect();
        for (cvar, name) in hits {
            self.cancel_cleanup_of(&cvar, &name);
        }
        Ok(())
    }

    fn open_scope(&mut self) {
        self.scopes.push(Vec::new());
        self.pops.push(Vec::new());
    }

    /// Close the innermost scope: its cleanup pops run here, which is LIFO over declaration order
    /// (§4.7.4) because each pop takes the current top of the focus's deque.
    fn close_scope(&mut self) {
        if let Some(p) = self.pops.last_mut() {
            let entries = std::mem::take(p);
            for s in entries.into_iter().flatten() {
                self.b(&s);
            }
        }
        self.scopes.pop();
        self.pops.pop();
    }

    /// A `return` exits every open scope first, innermost first (§4.7.4). The scope stack keeps its
    /// shape so dead code after the `return` still compiles.
    fn drain_pops(&mut self) {
        let all = std::mem::take(&mut self.pops);
        let mut kept = Vec::with_capacity(all.len());
        for mut p in all.into_iter().rev() {
            let entries = std::mem::take(&mut p);
            kept.push(p);
            for s in entries.into_iter().flatten() {
                self.b(&s);
            }
        }
        kept.reverse();
        self.pops = kept;
    }

    // ------------------------------------------------------------------ statements

    fn stmt(&mut self, s: &Stmt) -> Result<(), CgError> {
        match &s.kind {
            StmtKind::Let { is_var, name, ty, init, .. } => self.stmt_let(*is_var, name, ty.as_ref(), init.as_ref()),
            StmtKind::Assign { target, op, value } => self.stmt_assign(target, *op, value),
            StmtKind::Expr { expr, linkage } => {
                // §7.6.1/§7.6.3: an A-track `@c` block in statement position *is* the external
                // source. `cli` writes it verbatim into `<build>/a_track/<name>.c`, so the generated
                // C must not also try to evaluate it — it contributes a comment, no statement.
                if let ExprKind::Embed(em) = &expr.kind {
                    if em.is_a_track() {
                        self.b(&format!(
                            "/* §7.6.1 A 轨源码块 `{}`（{}）在 a_track/ 里，不在此处发射 */",
                            em.unit_name().unwrap_or_default(),
                            em.lang
                        ));
                        for l in linkage {
                            self.linkage(l)?;
                        }
                        return Ok(());
                    }
                }
                let v = self.expr(expr, None)?;
                self.b(&format!("(void){};", v.text));
                for l in linkage {
                    self.linkage(l)?;
                }
                Ok(())
            }
            StmtKind::If { cond, then, else_ } => {
                let c = self.expr(cond, Some(&Ty::Bool))?;
                self.b(&format!("if ({}) {{", c.text));
                self.depth += 1;
                self.open_scope();
                self.block_body(then, None, false)?;
                self.close_scope();
                self.depth -= 1;
                match else_.as_deref() {
                    Some(ElseBranch::Block(b)) => {
                        self.b("} else {");
                        self.depth += 1;
                        self.open_scope();
                        self.block_body(b, None, false)?;
                        self.close_scope();
                        self.depth -= 1;
                        self.b("}");
                    }
                    Some(ElseBranch::If(inner)) => {
                        self.b("} else ");
                        self.stmt(inner)?;
                    }
                    None => self.b("}"),
                }
                Ok(())
            }
            StmtKind::While { cond, body } => {
                let c = self.expr(cond, Some(&Ty::Bool))?;
                self.b(&format!("while ({}) {{", c.text));
                self.depth += 1;
                self.open_scope();
                self.block_body(body, None, false)?;
                self.close_scope();
                self.depth -= 1;
                self.b("}");
                Ok(())
            }
            StmtKind::Loop { body, label } => {
                if label.is_some() {
                    return Err(CgError::new("§3.5", "codegen: 带标签的 `loop`/`break`/`continue` 未实现"));
                }
                self.b("for (;;) {");
                self.depth += 1;
                self.open_scope();
                self.block_body(body, None, false)?;
                self.close_scope();
                self.depth -= 1;
                self.b("}");
                Ok(())
            }
            StmtKind::Break { label, value } => {
                if label.is_some() || value.is_some() {
                    return Err(CgError::new(
                        "§3.5",
                        "codegen: 带标签或带值的 `break` 未实现（`break` 的值需要循环返回类型）",
                    ));
                }
                self.b("break;");
                Ok(())
            }
            StmtKind::Continue { label } => {
                if label.is_some() {
                    return Err(CgError::new("§3.5", "codegen: 带标签的 `continue` 未实现"));
                }
                self.b("continue;");
                Ok(())
            }
            StmtKind::Match { scrut, arms } => {
                self.match_into(scrut, arms, None)?;
                Ok(())
            }
            StmtKind::Return { value } => {
                let want = self.ret_ty.clone();
                match value {
                    Some(v) => {
                        let val = self.expr(v, want.as_ref())?;
                        self.cancel_moved_return(&val)?;
                        self.drain_pops();
                        self.b(&format!("return {};", val.text));
                    }
                    None if want.is_none() => {
                        self.drain_pops();
                        self.b("return;");
                    }
                    None => {
                        self.drain_pops();
                        self.b("return (InfyUnit){0};");
                    }
                }
                Ok(())
            }
            StmtKind::Unsafe(b) => {
                self.b("{");
                self.depth += 1;
                self.open_scope();
                self.block_body(b, None, false)?;
                self.close_scope();
                self.depth -= 1;
                self.b("}");
                Ok(())
            }
            StmtKind::Label { name, .. } => {
                // §8.4 rule 1: a label is an anchor, not a node and not a C label.
                self.b(&format!("/* anchor: {name} */"));
                Ok(())
            }
            StmtKind::Pause => Ok(self.b("infy_focus_pause(infy_current_focus());")),
            StmtKind::Stop => Ok(self.b("infy_request_stop();")),
            StmtKind::Resume => Ok(self.b("infy_focus_resume(infy_current_focus());")),
            StmtKind::For { .. } => Err(CgError::new(
                "§3.5 / §10.10",
                "codegen: `for` 需要 §10.10 的迭代器协议（`InfyIter` 与 guard 步进），未实现",
            )),
            StmtKind::Defer(_) => Err(CgError::new(
                "§3.13",
                "codegen: `defer` 块需要为每个位点生成闭包函数，未实现",
            )),
            StmtKind::ConstAssert { .. } => Err(CgError::new(
                "§3.13",
                "codegen: `const_assert!` 属阶段 [4] 的编译期求值，后端不重复判定，未实现",
            )),
            StmtKind::NestedItem(_) => Err(CgError::new(
                "§3.5",
                "codegen: 语句位置里的声明不会被生成（阶段 [4] 应已报 E-03xx）",
            )),
        }
    }

    /// `let`/`var`. Inside a chain the declaration names a **frame slot** (§8.4 rule 1 puts it in
    /// the frame, `docs/internals/runtime-abi.md` §5), so it becomes an assignment.
    fn stmt_let(
        &mut self,
        _is_var: bool,
        name: &str,
        ty: Option<&crate::ast::Type>,
        init: Option<&Expr>,
    ) -> Result<(), CgError> {
        let from_frame = self
            .scopes
            .first()
            .map(|s| s.iter().any(|b| b.infy == name))
            .unwrap_or(false);
        let declared = ty.and_then(crate::hir::hir_ty_from_ast);
        let init_ty = init.and_then(|e| self.prog.ty(e.span).cloned());
        let want: Option<Ty> = declared.clone().or(init_ty);
        let v = match init {
            Some(e) => Some(self.expr(e, want.as_ref())?),
            None => None,
        };
        let ty = match (v.as_ref().map(|x| x.ty.clone()), declared) {
            (Some(t), _) => t,
            (None, Some(t)) => t,
            (None, None) => {
                return Err(CgError::new(
                    "§4.13",
                    format!("codegen: `let {name}` 既无标注也无初始值，类型无从推断"),
                ))
            }
        };
        if from_frame {
            // §8.4 rule 1 + ABI §5: this `let` assigns a chain frame slot. A slot can never hold a
            // drop-bearing type — `emit_chain` refuses the whole chain before emitting its body —
            // so there is no cleanup entry here to move or cancel.
            let slot = self.lookup(name).map(|b| b.cvar.clone()).unwrap_or_default();
            match &v {
                Some(x) => self.b(&format!("{slot} = {};", x.text)),
                None => self.b(&format!("{slot} = {{0}};")),
            }
            return Ok(());
        }
        let cvar = self.fresh(name);
        let mut deps = BTreeSet::new();
        let cty = self.c_ty(&ty, false, &mut deps)?;
        match &v {
            Some(x) => self.b(&format!("{} = {};", declarator(&cty, &cvar), x.text)),
            None => self.b(&format!("{} = {{0}};", declarator(&cty, &cvar))),
        }
        if let Some(top) = self.scopes.last_mut() {
            top.push(Binding { infy: name.to_string(), cvar: cvar.clone(), ty: ty.clone(), cleanup: false, pop: None });
        }
        self.register_drop(&cvar, &ty)?;
        // §4.7.1: cancel the source only after the destination holds the buffer *and* owns an entry
        // of its own, so no point of the emitted sequence leaves the value unowned.
        if let Some(init) = init {
            self.cancel_owned_move(init, &ty)?;
        }
        Ok(())
    }

    fn stmt_assign(&mut self, target: &Expr, op: Punct, value: &Expr) -> Result<(), CgError> {
        let (place, pty) = self.place(target)?;
        let v = self.expr(value, Some(&pty))?;
        let text = match op {
            Punct::Eq => format!("{place} = {};", v.text),
            op => {
                let op_name = match op {
                    Punct::PlusEq => "add",
                    Punct::MinusEq => "sub",
                    Punct::StarEq => "mul",
                    Punct::SlashEq => "div",
                    Punct::PercentEq => "rem",
                    other => {
                        return Err(CgError::new(
                            "§2.7",
                            format!("codegen: 复合赋值 `{}` 未实现", punct_text(other)),
                        ))
                    }
                };
                let checked = self.checked_arith(op_name, &pty, &place, &v.text)?;
                format!("{place} = {checked};")
            }
        };
        self.b(&text);
        Ok(())
    }

    /// A place expression: an lvalue plus its type. §3.5 keeps assignment targets in place form.
    fn place(&mut self, e: &Expr) -> Result<(String, Ty), CgError> {
        match &e.kind {
            ExprKind::Ident(n) => match self.lookup(n) {
                Some(b) => Ok((b.cvar.clone(), b.ty.clone())),
                None => Err(CgError::new("§4.1", format!("codegen: 未绑定的标识符 `{n}`"))),
            },
            ExprKind::Field { base, name } => {
                let b = self.expr(base, None)?;
                let (field, fty) = self.field_of(&b.ty, name)?;
                let text = self.field_text(&b, &field);
                Ok((text, fty))
            }
            ExprKind::Index { base, index } => {
                let b = self.expr(base, None)?;
                let i = self.expr(index, Some(&Ty::Prim(Prim::Usize)))?;
                let elem = self.elem_ty(&b.ty)?;
                let text = self.index_text(&b, &i)?;
                Ok((text, elem))
            }
            ExprKind::Deref { expr } => {
                let b = self.expr(expr, None)?;
                match &b.ty {
                    Ty::Ref { inner, .. } | Ty::Ptr { inner, .. } | Ty::Boxed { inner } => {
                        Ok((format!("(*{})", b.text), (**inner).clone()))
                    }
                    other => Err(CgError::new(
                        "§4.7.5",
                        format!("codegen: `*` 只能作用于引用/指针/Box，实际是 {}", other.text()),
                    )),
                }
            }
            other => Err(CgError::new(
                "§3.5",
                format!("codegen: 赋值目标必须是 place 表达式，实际是 {}", expr_kind_name(other)),
            )),
        }
    }

    // ------------------------------------------------------------------ chains (§8)

    /// One `chain`: a frame record, one C function per §8.4 node, and the three tables the
    /// scheduler reads (`InfyChainDesc`'s `nodes`/`locs`/`linkage_counts`).
    fn emit_chain(&mut self, c: &crate::hir::HirChain, info: &ChainInfo) -> Result<(), CgError> {
        let sym = c.symbol.clone();
        // --- frame record (ABI §5: locals in declaration order, then `node_idx`, then tail pad)
        let mut fields = String::new();
        let mut asserts = String::new();
        let mut frame_deps = BTreeSet::new();
        let frame_name = format!("{sym}_frame");
        for l in &c.frame.locals {
            if self.needs_drop(&l.ty)? {
                return Err(CgError::new(
                    "§4.7.4 / §8.4",
                    format!(
                        "codegen: 链 `{}` 的帧局部 `{}` 带析构器，而帧由调度器释放（`infy_rt.c` 的 `infy_free(f->frame)`），释放时机未实现",
                        c.name, l.name
                    ),
                ));
            }
            let mut deps = BTreeSet::new();
            let ft = self.c_ty(&l.ty, false, &mut deps)?;
            fields.push_str(&format!(" {};", declarator(&ft, &c_ident(&l.name))));
            frame_deps.extend(deps);
            asserts.push_str(&format!(
                "\ntypedef char infy_off_{f}_{n}[(offsetof(struct infy_s_{f}, {n}) == {o}u) ? 1 : -1];",
                f = frame_name,
                n = c_ident(&l.name),
                o = l.offset
            ));
        }
        // ABI §5: the locals record, then `node_idx`, then tail padding to the frame alignment.
        fields.push_str(" uint32_t node_idx;");
        let frame_def = format!(
            "struct infy_s_{frame_name} {{{fields}}};\ntypedef struct infy_s_{frame_name} {frame_name};\n\
             typedef char infy_layout_{frame_name}[(sizeof({frame_name}) == {}u && _Alignof({frame_name}) == {}u) ? 1 : -1];\n\
             typedef char infy_off_{frame_name}_node_idx[(offsetof(struct infy_s_{frame_name}, node_idx) == {}u) ? 1 : -1];{asserts}",
            c.frame.size,
            c.frame.align,
            c.frame.node_idx_offset,
            asserts = asserts
        );
        self.reserve(&frame_name);
        self.types.insert(frame_name.clone(), TypeNode { def: Some(frame_def), deps: frame_deps.clone() });

        // --- node grouping (§8.4 rule 1: only a top-level `expr_stmt` opens a node; `let`/`var`/
        // plain assignment/`label` belong to the *preceding* node and run after its linkage).
        let mut groups: Vec<Vec<&Stmt>> = Vec::new();
        let mut leading: Vec<&Stmt> = Vec::new();
        for s in &info.stmts {
            if matches!(s.kind, StmtKind::Expr { .. }) {
                let mut v = std::mem::take(&mut leading);
                v.push(s);
                groups.push(v);
            } else if let Some(g) = groups.last_mut() {
                g.push(s);
            } else {
                leading.push(s);
            }
        }
        if !leading.is_empty() && groups.is_empty() {
            return Err(CgError::new(
                "§8.4",
                format!(
                    "codegen: 链 `{}` 只有 `let`/`var`/`label`，没有 `expr_stmt` 承载它们（§8.4 的节点只由宿主语句开出）",
                    c.name
                ),
            ));
        }
        if groups.len() != c.nodes.len() {
            return Err(CgError::new(
                "§8.4",
                format!(
                    "codegen: 链 `{}` 的节点分组得到 {} 个，阶段 [7] 的节点表有 {} 个（编号规则必须逐行一致）",
                    c.name,
                    groups.len(),
                    c.nodes.len()
                ),
            ));
        }

        let id = *self.chain_ids.get(&c.name).ok_or_else(|| {
            CgError::new("§8.2", format!("codegen: 链 `{}` 没有注册 id", c.name))
        })?;
        self.chain_ctx = Some(ChainCtx {
            id,
            n_nodes: c.nodes.len() as u32,
            labels: c.labels.clone(),
        });

        // --- node functions
        let mut node_texts: Vec<String> = Vec::new();
        for (k, stmts) in groups.iter().enumerate() {
            self.buf.clear();
            self.depth = 0;
            let fname = format!("{sym}_node_{k}");
            self.b(&format!("static void {fname}(void* frame_) {{"));
            self.depth = 1;
            self.b(&format!("{frame_name}* f = ({frame_name}*)frame_;"));
            self.b(&format!("f->node_idx = {k}u;"));
            self.scopes = vec![Vec::new()];
            self.pops = vec![Vec::new()];
            for l in &c.frame.locals {
                self.scopes[0].push(Binding {
                    infy: l.name.clone(),
                    cvar: format!("f->{}", c_ident(&l.name)),
                    ty: l.ty.clone(),
                    cleanup: false,
                    pop: None,
                });
            }
            self.ret_ty = None;
            self.diverging = false;
            self.cur_package = c.package.clone();
            self.cur_node_idx = Some(k as u32);
            for s in stmts {
                self.stmt(s)?;
            }
            while self.scopes.len() > 1 {
                self.close_scope();
            }
            // The function scope's own pops run at the node boundary (§4.7.4).
            self.close_scope();
            self.depth = 0;
            self.b("}");
            node_texts.push(std::mem::take(&mut self.buf));
        }
        self.chain_ctx = None;
        self.scopes.clear();
        self.pops.clear();

        // --- tables
        let n = c.nodes.len();
        let nodes_arr = format!(
            "static const InfyNodeFn {sym}_nodes[{n}] = {{ {} }};",
            (0..n).map(|k| format!("{sym}_node_{k}")).collect::<Vec<_>>().join(", ")
        );
        let mut loc_rows = Vec::new();
        let mut linkage_rows = Vec::new();
        for node in &c.nodes {
            let file = self
                .sources
                .get(node.file)
                .map(|f| f.display_path.clone())
                .unwrap_or_default();
            loc_rows.push(format!(
                "{{ {}, {}u, {}u, {}u, {}u }}",
                c_str_literal(&file)?,
                node.line,
                node.col,
                node.idx,
                node.linkage_count
            ));
            linkage_rows.push(format!("{}u", node.linkage_count));
        }
        let locs_arr = format!("static const InfyNodeLoc {sym}_locs[{n}] = {{ {} }};", loc_rows.join(", "));
        let link_arr = format!("static const uint32_t {sym}_linkage[{n}] = {{ {} }};", linkage_rows.join(", "));
        let desc = format!(
            "static const InfyChainDesc {sym}_desc = {{ {}, {n}u, 0u, {}u, {}u, {sym}_nodes, {sym}_locs, {sym}_linkage }};",
            c_str_literal(&c.name)?,
            c.frame.size,
            c.frame.align
        );
        let mut text = String::new();
        for t in &node_texts {
            text.push_str(t);
        }
        text.push_str(&format!("{nodes_arr}\n{locs_arr}\n{link_arr}\n{desc}\n\n"));
        self.chain_defs.push(text);
        Ok(())
    }

    /// §5.9/§13.4: the `infy test` entry. The harness is generated C, so it owns a report channel
    /// of its own — per-test lines on stdout (§7.7.1 item 4 assigns INFY's own `print()` to the log
    /// sink, and no INFY construct can write stdout yet, BACKLOG F19). Verdict per §13.4: `0` when
    /// every test passed, `1` when any returned `Err`. A panic inside a test leaves the process with
    /// §13.4's `3` and stops the suite, because §4.16 gives no catch point outside a focus
    /// (docs/spec-deviations.md D-36).
    fn emit_test_driver(&mut self) -> Result<(), CgError> {
        let mut tests: Vec<crate::hir::HirFn> = self
            .hir
            .items
            .iter()
            .filter_map(|it| match it {
                HirItem::Fn(f) => match f.test_kind {
                    Some(TestKind::Test) => Some(f.clone()),
                    Some(TestKind::Bench) if self.cfg.bench => Some(f.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        // §7.3: the report is a function of the declared set, not of the directory walk order.
        tests.sort_by(|a, b| a.package.cmp(&b.package).then_with(|| a.name.cmp(&b.name)));
        let mut s = String::from("\n/* ---- §5.9 test harness (`infy test`) ---- */\nint main(void) {\n");
        let chains: Vec<&crate::hir::HirChain> = self
            .hir
            .items
            .iter()
            .filter_map(|it| match it {
                HirItem::Chain(c) => Some(c),
                _ => None,
            })
            .collect();
        if !chains.is_empty() {
            s.push_str(&format!(
                "    static const InfyChainDesc* const infy_chain_table[{}] = {{ {} }};\n\
                 \x20   infy_chain_register_table(infy_chain_table, {}u);\n",
                chains.len(),
                chains.iter().map(|c| format!("&{}_desc", c.symbol)).collect::<Vec<_>>().join(", "),
                chains.len()
            ));
        }
        s.push_str("    infy_run_package_inits(0, 0u);\n");
        s.push_str("    uint32_t infy_t_n = 0u, infy_t_bad = 0u;\n");
        for f in &tests {
            let label = format!("{}::{}", f.package, f.name);
            let what = if matches!(f.test_kind, Some(TestKind::Bench)) { "`#[bench]`" } else { "`#[test]`" };
            if f.is_event || f.is_foreign {
                return Err(CgError::new(
                    "§5.9",
                    format!("codegen: {what} `{label}` 没有可执行的本体（`event`/`extern` 由宿主提供，§3.3/§7.6）"),
                ));
            }
            if f.self_ty.is_some() {
                return Err(CgError::new(
                    "§5.9",
                    format!("codegen: {what} `{label}` 是方法，harness 没有一个接收者可传（§5.9 只定义自由函数）"),
                ));
            }
            if !f.params.is_empty() {
                // §14.7's `#[bench] fn(b: &mut test::Bench)` needs `std/test`, which has no entry in
                // §10's std table at all (BACKLOG F21) — so the honest message names that, not a
                // generic "bad signature".
                let why = if matches!(f.test_kind, Some(TestKind::Bench)) {
                    "§14.7 的基准签名是 `fn(b: &mut test::Bench)`，而 §10 的 std 签名表里没有任何 \
                     `std.test` 条目（含 `Bench`/`iter`），`b.iter()` 在阶段 [4] 就不可解析 —— \
                     登记为 BACKLOG F21"
                        .to_string()
                } else {
                    "§5.9 没有给测试函数注入参数的机制（`env` 的字段集合按 §4.6 是封闭的）".to_string()
                };
                return Err(CgError::new(
                    "§5.9",
                    format!("codegen: {what} `{label}` 有 {} 个参数，harness 无法调用它：{why}", f.params.len()),
                ));
            }
            s.push_str(&format!("    /* {label} (§5.9) */\n    infy_t_n += 1u;\n"));
            // `fputs` with the name baked into the literal, so no user identifier is ever a format
            // string (§2.3 allows non-ASCII identifiers).
            s.push_str(&format!("    fputs(\"test {label} ... \", stdout);\n    fflush(stdout);\n"));
            match &f.ret {
                None => s.push_str(&format!("    (void){}();\n    fputs(\"ok\\n\", stdout);\n", f.symbol)),
                Some((ret, _)) => {
                    if !matches!(ret, Ty::Result { .. }) {
                        return Err(CgError::new(
                            "§5.9",
                            format!(
                                "codegen: {what} `{label}` 的返回类型不是 `()` 也不是 `Result<…>`，\
                                 harness 对它没有可判定的\u{201c}通过\u{201d}条件（§5.9/§14.7 只给出这两种形态）"
                            ),
                        ));
                    }
                    let ty = self.c_ty_isolated(ret)?;
                    s.push_str(&format!(
                        "    {{ {ty} infy_t_r = {}(); if (infy_t_r.is_ok) {{ fputs(\"ok\\n\", stdout); }} \
                         else {{ fputs(\"FAILED\\n\", stdout); infy_t_bad += 1u; }} }}\n",
                        f.symbol
                    ));
                }
            }
        }
        s.push_str("    printf(\"test result: %u passed, %u failed, %u total\\n\", infy_t_n - infy_t_bad, infy_t_bad, infy_t_n);\n");
        s.push_str("    return infy_t_bad == 0u ? 0 : 1;\n}\n");
        self.driver = s;
        Ok(())
    }

    fn emit_driver(&mut self) -> Result<(), CgError> {
        if self.cfg.test_harness {
            return self.emit_test_driver();
        }
        let mut s = String::from("\n/* ---- entry (§7.4) ---- */\nint main(void) {\n");
        let chains: Vec<&crate::hir::HirChain> = self
            .hir
            .items
            .iter()
            .filter_map(|it| match it {
                HirItem::Chain(c) => Some(c),
                _ => None,
            })
            .collect();
        if chains.is_empty() {
            s.push_str("    return 1;\n}\n");
            self.driver = s;
            return Ok(());
        }
        s.push_str(&format!(
            "    static const InfyChainDesc* const infy_chain_table[{}] = {{ {} }};\n\
             \x20   infy_chain_register_table(infy_chain_table, {}u);\n",
            chains.len(),
            chains.iter().map(|c| format!("&{}_desc", c.symbol)).collect::<Vec<_>>().join(", "),
            chains.len()
        ));
        s.push_str("    infy_run_package_inits(0, 0u);\n");
        match self.hir.entry {
            Some(HirEntry::Chain(item_idx)) => {
                let c = match &self.hir.items[item_idx] {
                    HirItem::Chain(c) => c,
                    _ => {
                        return Err(CgError::new("§7.4", "codegen: 入口索引不指向链"));
                    }
                };
                let id = self.chain_ids.get(&c.name).cloned().unwrap_or(0);
                s.push_str(&format!("    return infy_runtime_start({id}u, 0u);\n}}\n"));
            }
            Some(HirEntry::Main(item_idx)) => {
                let sym = match &self.hir.items[item_idx] {
                    HirItem::Fn(f) => f.symbol.clone(),
                    _ => return Err(CgError::new("§7.4", "codegen: `fn main` 入口索引不指向函数")),
                };
                s.push_str(&format!("    {sym}();\n    return 0;\n}}\n"));
            }
            None => {
                return Err(CgError::new(
                    "§7.4",
                    "codegen: 阶段 [7] 没有解析出入口（`E-0600` 应已在更早阶段报出）",
                ))
            }
        }
        self.driver = s;
        Ok(())
    }

    /// Linkage statements (§8.1/§8.2): `goto` targets are resolved at compile time (`E-0301`), so
    /// only the checked runtime fallback is emitted (§4.9 UB-013).
    fn linkage(&mut self, block: &LinkageBlock) -> Result<(), CgError> {
        for st in &block.stmts {
            self.linkage_stmt(st)?;
        }
        Ok(())
    }

    fn linkage_stmt(&mut self, st: &LinkageStmt) -> Result<(), CgError> {
        let ctx = self.chain_ctx.clone().ok_or_else(|| {
            CgError::new("§8.1", "codegen: 连携块只能出现在 chain 的宿主语句之后")
        })?;
        match st {
            LinkageStmt::Goto(t) => {
                let target = match t {
                    crate::ast::GotoTarget::Named(s) => *ctx.labels.get(s).ok_or_else(|| {
                        CgError::new("§8.2", format!("codegen: 锚点 `{s}` 不存在（E-0301 应已在 [4] 报出）"))
                    })?,
                    crate::ast::GotoTarget::Abs(n) => {
                        let n = *n as i128;
                        if n < 0 || n >= ctx.n_nodes as i128 {
                            return Err(CgError::new("§8.4", format!("codegen: `goto({n})` 越出节点表")));
                        }
                        n as u32
                    }
                    crate::ast::GotoTarget::Rel(d) => {
                        let cur = self.cur_node_idx.ok_or_else(|| {
                            CgError::new("§8.4", "codegen: `goto(+n)` 需要在节点上下文里求当前编号")
                        })?;
                        let t = cur as i128 + *d;
                        if t < 0 || t >= ctx.n_nodes as i128 {
                            return Err(CgError::new(
                                "§8.4",
                                format!("codegen: `goto({:+})` 从节点 {cur} 越出 {} 个节点的表", d, ctx.n_nodes),
                            ));
                        }
                        t as u32
                    }
                };
                let at = self.loc();
                self.b(&format!(
                    "infy_goto_checked({}u, {}u, {});",
                    ctx.id,
                    target,
                    at
                ));
                // §8.2: `goto` *moves* the focus, so it ends the node — the statements after it in
                // the same linkage block do not run. Setting the pending node and falling through
                // lets a later `goto` overwrite it, and `if (all_green()) { goto("done"); }
                // goto(+1);` from the §14 tour then always took `+1`: the chain never ended.
                // The node's exit action set (§8.4 rule 4) still runs, hence `drain_pops`.
                self.drain_pops();
                self.b("return;");
                Ok(())
            }
            LinkageStmt::Start { chain, node, .. } => {
                let id = self.chain_ids.get(chain).cloned().ok_or_else(|| {
                    CgError::new("§8.2", format!("codegen: `start(chain = {chain})` 指向不存在的链"))
                })?;
                let start = match node {
                    None => 0u32,
                    Some(StartNode::Index(i)) => *i as u32,
                    Some(StartNode::Anchor(a)) => {
                        let c = self.hir.chain_by_name(chain).ok_or_else(|| {
                            CgError::new("§8.2", format!("codegen: 链 `{chain}` 不在 HIR 里"))
                        })?;
                        *c.labels.get(a).ok_or_else(|| {
                            CgError::new("§8.2", format!("codegen: 链 `{chain}` 没有锚点 `{a}`"))
                        })?
                    }
                    Some(StartNode::Ident(i)) => {
                        let c = self.hir.chain_by_name(chain).ok_or_else(|| {
                            CgError::new("§8.2", format!("codegen: 链 `{chain}` 不在 HIR 里"))
                        })?;
                        *c.labels.get(i).ok_or_else(|| {
                            CgError::new("§8.2", format!("codegen: 链 `{chain}` 没有锚点 `{i}`"))
                        })?
                    }
                };
                let at = self.loc();
                self.b(&format!(
                    "infy_focus_spawn({id}u, {start}u, {at});"
                ));
                Ok(())
            }
            LinkageStmt::StartFlag => Err(CgError::new(
                "§8.2",
                "codegen: 裸 `start`（取声明序下一条链）需要链声明序索引，未实现",
            )),
            LinkageStmt::Pause => Ok(self.b("infy_focus_pause(infy_current_focus());")),
            LinkageStmt::Stop => Ok(self.b("infy_request_stop();")),
            LinkageStmt::Resume => Ok(self.b("infy_focus_resume(infy_current_focus());")),
            LinkageStmt::If { cond, then, else_ } => {
                let c = self.expr(cond, Some(&Ty::Bool))?;
                self.b(&format!("if ({}) {{", c.text));
                self.depth += 1;
                for s in then {
                    self.linkage_stmt(s)?;
                }
                self.depth -= 1;
                match else_ {
                    Some(e) => {
                        self.b("} else {");
                        self.depth += 1;
                        for s in e {
                            self.linkage_stmt(s)?;
                        }
                        self.depth -= 1;
                        self.b("}");
                    }
                    None => self.b("}"),
                }
                Ok(())
            }
            LinkageStmt::Plain(s) => self.stmt(s),
        }
    }

    // ------------------------------------------------------------------ expressions

    fn expr(&mut self, e: &Expr, want: Option<&Ty>) -> Result<Val, CgError> {
        match &e.kind {
            ExprKind::Lit(l) => self.literal(l, want, e),
            ExprKind::Ident(n) => self.ident(n, e),
            ExprKind::Path(segs) => self.path_expr(segs, e, want),
            ExprKind::Unary { op, expr } => self.unary(*op, expr, want),
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, e),
            ExprKind::Assign { target, op, value } => {
                let (place, pty) = self.place(target)?;
                let v = self.expr(value, want.or(Some(&pty)))?;
                self.b(&format!("{place} = {};", v.text));
                let _ = op;
                Ok(Val::of(pty, format!("({place})")))
            }
            ExprKind::Call { callee, args } => self.call(callee, args, want),
            ExprKind::Method { recv, name, args, .. } => self.method(recv, name, args, want),
            ExprKind::Field { base, name } => {
                let b = self.expr(base, None)?;
                let (field, fty) = self.field_of(&b.ty, name)?;
                Ok(Val::of(fty, self.field_text(&b, &field)))
            }
            ExprKind::Index { base, index } => {
                let b = self.expr(base, None)?;
                let i = self.expr(index, Some(&Ty::Prim(Prim::Usize)))?;
                let text = self.index_text(&b, &i)?;
                let elem = self.elem_ty(&b.ty)?;
                Ok(Val::of(elem, text))
            }
            ExprKind::StructLit { path, fields, rest } => self.struct_lit(path, fields, rest.as_deref(), want),
            ExprKind::Tuple(v) => self.tuple_lit(v, want),
            ExprKind::Array(items) => {
                let ty = want.cloned().ok_or_else(|| {
                    CgError::new("§4.1", "codegen: 数组字面量没有上下文类型，长度与元素类型无从确定")
                })?;
                let elem = match &ty {
                    Ty::Array { inner, .. } | Ty::Slice { inner } => (**inner).clone(),
                    other => return Err(CgError::new("§4.1", format!("codegen: 数组字面量的上下文类型应是 [T; N]，实际是 {}", other.text()))),
                };
                let mut parts = Vec::new();
                for it in items {
                    let v = self.expr(it, Some(&elem))?;
                    parts.push(v.text);
                }
                let mut deps = BTreeSet::new();
                let cty = self.c_ty(&ty, false, &mut deps)?;
                Ok(Val::of(ty, format!("({}){{ {} }}", cty, parts.join(", "))))
            }
            ExprKind::ArrayRepeat { value, len } => {
                let v = self.expr(value, want)?;
                let n = self.expr(len, Some(&Ty::Prim(Prim::Usize)))?.text;
                let ty = match want.cloned() {
                    Some(t @ Ty::Array { .. }) => t,
                    _ => {
                        return Err(CgError::new(
                            "§4.1",
                            "codegen: `[x; N]` 需要上下文类型才能确定元素类型",
                        ))
                    }
                };
                let mut deps = BTreeSet::new();
                let cty = self.c_ty(&ty, false, &mut deps)?;
                let base = cty.trim_end_matches(|c| c != '[').trim_end().to_string();
                let tmp = self.fresh("arr");
                self.b(&format!("{} {tmp}[{n}];", base));
                self.b(&format!(
                    "for (size_t k_{tmp} = 0; k_{tmp} < (size_t)({n}); k_{tmp}++) {{ {tmp}[k_{tmp}] = {}; }}",
                    v.text
                ));
                Ok(Val::of(ty, tmp))
            }
            ExprKind::If { cond, then, else_ } => {
                let c = self.expr(cond, Some(&Ty::Bool))?;
                let ty = want.cloned().or_else(|| self.prog.ty(e.span).cloned());
                let res = match &ty {
                    Some(t) if !matches!(t, Ty::Unit) => {
                        let name = self.fresh("ifres");
                        self.decl_stmt(t, &name, Some("{0}"))?;
                        Some((name, t.clone()))
                    }
                    _ => None,
                };
                self.b(&format!("if ({}) {{", c.text));
                self.depth += 1;
                self.open_scope();
                self.block_into(then, res.as_ref())?;
                self.close_scope();
                self.depth -= 1;
                match else_.as_deref() {
                    Some(ElseBranch::Block(b)) => {
                        self.b("} else {");
                        self.depth += 1;
                        self.open_scope();
                        self.block_into(b, res.as_ref())?;
                        self.close_scope();
                        self.depth -= 1;
                        self.b("}");
                    }
                    Some(ElseBranch::If(inner)) => {
                        self.b("} else ");
                        self.stmt(inner)?;
                    }
                    None => self.b("}"),
                }
                Ok(match res {
                    Some((n, t)) => Val::of(t, n),
                    None => Val::of(Ty::Unit, "(InfyUnit){0}".to_string()),
                })
            }
            ExprKind::Match { scrut, arms } => Ok(self.match_into(scrut, arms, want)?
                .unwrap_or_else(|| Val::of(Ty::Unit, "(InfyUnit){0}".to_string()))),
            ExprKind::Block(b) | ExprKind::Unsafe(b) => {
                let ty = want.cloned().or_else(|| self.prog.ty(e.span).cloned());
                // The result temp belongs to the **enclosing** scope: a block used as an expression
                // hands its value back to the caller, and a `bres_N` declared inside the braces is
                // out of sight when the caller emits `return bres_N;`. `ExprKind::If` right above
                // already declares before opening its branches; this arm had it the other way round.
                let res = match &ty {
                    Some(t) if !matches!(t, Ty::Unit) => {
                        let name = self.fresh("bres");
                        self.decl_stmt(t, &name, Some("{0}"))?;
                        Some((name, t.clone()))
                    }
                    _ => None,
                };
                self.b("{");
                self.depth += 1;
                self.open_scope();
                self.block_into(b, res.as_ref())?;
                self.close_scope();
                self.depth -= 1;
                self.b("}");
                Ok(match res {
                    Some((n, t)) => Val::of(t, n),
                    None => Val::of(Ty::Unit, "(InfyUnit){0}".to_string()),
                })
            }
            ExprKind::Ref { mutable, expr, .. } => {
                let v = self.expr(expr, None)?;
                let mut deps = BTreeSet::new();
                let ct = self.c_ty(&Ty::Ref { inner: Box::new(v.ty.clone()), mutable: *mutable, lifetime: None }, true, &mut deps)?;
                Ok(Val::of(Ty::Ref { inner: Box::new(v.ty.clone()), mutable: *mutable, lifetime: None }, format!("(({ct})&({}))", v.text)))
            }
            ExprKind::Deref { expr } => {
                let v = self.expr(expr, None)?;
                match &v.ty {
                    Ty::Ref { inner, .. } | Ty::Ptr { inner, .. } | Ty::Boxed { inner } => {
                        Ok(Val::of((**inner).clone(), format!("(*{})", v.text)))
                    }
                    other => Err(CgError::new(
                        "§4.7.5",
                        format!("codegen: `*` 只能解引用引用/指针/Box，实际是 {}", other.text()),
                    )),
                }
            }
            ExprKind::Try { expr } => self.try_expr(expr),
            ExprKind::Cast { expr, ty } => {
                let v = self.expr(expr, None)?;
                let t = crate::hir::hir_ty_from_ast(ty).ok_or_else(|| {
                    CgError::new("§4.4", "codegen: `as` 的目标类型无法映射到 C 表示")
                })?;
                let mut deps = BTreeSet::new();
                let ct = self.c_ty(&t, false, &mut deps)?;
                Ok(Val::of(t, format!("(({ct})({}))", v.text)))
            }
            ExprKind::Move { expr } => self.expr(expr, want),
            ExprKind::Closure { .. } => Err(CgError::new(
                "§4.1 / §3.8",
                "codegen: 闭包未实现（捕获箱与 `InfyCloseret` 表示属未做项）",
            )),
            ExprKind::Is { .. } => Err(CgError::new(
                "§4.14 / §7.7.7",
                "codegen: `is` 需要 `dyn` vtable，未实现（表已定在 `docs/internals/runtime-abi.md` §1 / D-50(c)，F4）",
            )),
            ExprKind::Range { .. } => Err(CgError::new(
                "§3.8 / §10.10",
                "codegen: 范围表达式需要 §10.10 的迭代器协议，未实现",
            )),
            ExprKind::Embed(e) => Err(CgError::new(
                "§7.6.1 / §7.7.1",
                format!(
                    "codegen: `@{}` 块出现在需要值的位置：源码块不产生值（A 轨要取返回值就调它的 `extern` 函数，§7.6.1），B 轨通往 IFBC VM 的调用通道还没有（docs/BACKLOG.md F29）",
                    e.lang
                ),
            )),
            ExprKind::MacroCall { name, .. } => Err(CgError::new(
                "§6.5 / §10.11",
                format!("codegen: 未展开的宏调用 `{name}!`（`fmt!` 在阶段 [2]/[4] 就没有 lowering 路径，见 docs/BACKLOG.md）"),
            )),
            ExprKind::Auto => Err(CgError::new("§2.6", "codegen: `auto` 只能出现在具名实参位置")),
            ExprKind::Invalid => Err(CgError::new(
                "§3.10",
                "codegen: 遇到解析错误占位，拒绝生成",
            )),
        }
    }

    /// `expr?` desugars exactly as §4.11 fixes it: `match expr { Ok(v) => v, Err(e) => return
    /// Err(e.into()) }`, and the `into()` is only legal when the error types already agree.
    fn try_expr(&mut self, inner: &Expr) -> Result<Val, CgError> {
        let v = self.expr(inner, None)?;
        let ret = self.ret_ty.clone().ok_or_else(|| CgError::new(
            "§4.11",
            "codegen: `?` 出现在没有返回边界的上下文（链顶层属 E-0173）",
        ))?;
        match (&v.ty, &ret) {
            (Ty::Result { ok, err }, Ty::Result { err: rerr, .. }) => {
                if err.text() != rerr.text() {
                    return Err(CgError::new(
                        "§4.11",
                        format!("codegen: `?` 需要 `From<{}, {}>` 的调用发射，未实现（E-0170 由阶段 [4] 判定）", err.text(), rerr.text()),
                    ));
                }
                let tmp = self.fresh("try");
                self.decl_stmt(&v.ty, &tmp, Some(&v.text))?;
                let eres = self.fresh("err");
                let rty_text = {
                    let mut deps = BTreeSet::new();
                    self.c_ty(&ret, false, &mut deps)?
                };
                self.b(&format!("if (!{tmp}.is_ok) {{"));
                self.depth += 1;
                self.b(&format!("{rty_text} {eres} = ({rty_text}){{ .is_ok = 0u, .err = {tmp}.err }};"));
                self.drain_pops();
                self.b(&format!("return {eres};"));
                self.depth -= 1;
                self.b("}");
                Ok(Val::of((**ok).clone(), format!("{tmp}.ok")))
            }
            (Ty::Option { inner: oi }, Ty::Option { .. }) => {
                let tmp = self.fresh("try");
                self.decl_stmt(&v.ty, &tmp, Some(&v.text))?;
                let rty_text = {
                    let mut deps = BTreeSet::new();
                    self.c_ty(&ret, false, &mut deps)?
                };
                let none = self.fresh("none");
                let test = if Cg::niche(oi) {
                    format!("{tmp} == 0")
                } else {
                    format!("!{tmp}.some")
                };
                self.b(&format!("if ({test}) {{"));
                self.depth += 1;
                self.b(&format!("{rty_text} {none} = ({rty_text}){{0}};"));
                self.drain_pops();
                self.b(&format!("return {none};"));
                self.depth -= 1;
                self.b("}");
                let val = if Cg::niche(oi) { tmp } else { format!("{tmp}.val") };
                Ok(Val::of((**oi).clone(), val))
            }
            (a, b) => Err(CgError::new(
                "§4.11",
                format!("codegen: `?` 的组合 {} → {} 不合法（E-0171/E-0172 应已由阶段 [4] 报出）", a.text(), b.text()),
            )),
        }
    }

    // ------------------------------------------------------------------ leaf exprs

    fn literal(&mut self, l: &Lit, want: Option<&Ty>, e: &Expr) -> Result<Val, CgError> {
        match l {
            Lit::Str(s) => Ok(Val::of(Ty::Str, format!("infy_str_of({})", c_str_literal(s)?))),
            Lit::Cord(s) => Ok(Val::of(Ty::Str, format!("infy_str_of({})", c_str_literal(s)?))),
            Lit::Char(c) => Ok(Val::of(Ty::Prim(Prim::Char), format!("UINT32_C({})", *c as u32))),
            // §1: `bool` **is** `uint8_t` with only 0x00/0x01 legal, and `<stdbool.h>` is not part
            // of the runtime header, so the literals are the integers they stand for.
            Lit::Bool(b) => Ok(Val::of(Ty::Bool, if *b { "1u".to_string() } else { "0u".to_string() })),
            Lit::ByteStr(bytes) => Err(CgError::new(
                "§2.9 / §10.13",
                format!(
                    "codegen: 字节串字面量（{} 字节）需要 `infy_rt.h` 的 `Vec<u8>` 构造入口，头里没有，已拒绝而非伪造长度",
                    bytes.len()
                ),
            )),
            Lit::Float(f) => {
                let text = match &f.exp {
                    Some(e) => format!("{}e{}", f.mantissa, e.trim_start_matches('e')),
                    None => f.mantissa.clone(),
                };
                let ty = match f.suffix.as_deref() {
                    Some("f32") => Ty::Prim(Prim::F32),
                    _ => Ty::Prim(Prim::F64),
                };
                let text = if matches!(ty, Ty::Prim(Prim::F32)) {
                    format!("{text}f")
                } else {
                    text
                };
                Ok(Val::of(ty, text))
            }
            Lit::Int(i) => {
                let ty = match want {
                    Some(t) if t.is_integer() => t.clone(),
                    _ => self.prog.ty(e.span).cloned().unwrap_or(Ty::Prim(Prim::I32)),
                };
                let ty = if matches!(ty, Ty::IntLit) || ty.text() == "<int-literal>" {
                    Ty::Prim(Prim::I32)
                } else {
                    ty
                };
                let v = i.value.ok_or_else(|| CgError::new("§2.8", "codegen: 整型字面量超出 i128"))?;
                Ok(Val::of(ty.clone(), self.numeric_text(v, &ty)?))
            }
        }
    }

    /// §2.8: the literal's value must fit the resolved type. `sema` checks this, so a failure here
    /// means the two stages disagree — refuse loudly instead of emitting a wrapped constant.
    fn numeric_text(&mut self, v: u128, ty: &Ty) -> Result<String, CgError> {
        let p = match ty {
            Ty::Prim(p) => *p,
            Ty::Bool => return Ok(if v == 0 { "0".to_string() } else { "1".to_string() }),
            other => {
                return Err(CgError::new("§2.8", format!("codegen: 整型字面量落在了 {}", other.text())))
            }
        };
        macro_rules! fit {
            ($t:ty, $c:expr) => {{
                let x = <$t>::try_from(v).map_err(|_| {
                    CgError::new("§2.8 / §4.4", format!("codegen: 字面量 {v} 放不进 {}", $c))
                })?;
                Ok(format!("{}({})", $c, x))
            }};
        }
        match p {
            Prim::U8 => fit!(u8, "UINT8_C"),
            Prim::U16 => fit!(u16, "UINT16_C"),
            Prim::U32 => fit!(u32, "UINT32_C"),
            Prim::U64 | Prim::Usize => fit!(u64, "UINT64_C"),
            Prim::U128 => Err(CgError::new("§2.8 / §4.12", "codegen: u128 字面量没有 `stdint.h` 宏可写，未实现")),
            Prim::I8 => fit!(i8, "INT8_C"),
            Prim::I16 => fit!(i16, "INT16_C"),
            Prim::I32 => fit!(i32, "INT32_C"),
            Prim::I64 | Prim::Isize => fit!(i64, "INT64_C"),
            Prim::I128 => Err(CgError::new("§2.8 / §4.12", "codegen: i128 字面量没有 `stdint.h` 宏可写，未实现")),
            Prim::F32 => Ok(format!("{v}_f")),
            Prim::F64 => Ok(format!("{v}")),
            Prim::Bool => Ok(if v == 0 { "0".to_string() } else { "1".to_string() }),
            Prim::Char => Ok(format!("UINT32_C({v})")),
            Prim::Str | Prim::Unit => Err(CgError::new(
                "§2.8",
                format!("codegen: 整型字面量不可能取型 {}", p.text()),
            )),
        }
    }

    fn ident(&mut self, n: &str, e: &Expr) -> Result<Val, CgError> {
        if let Some(b) = self.lookup(n) {
            return Ok(Val::of(b.ty.clone(), b.cvar.clone()));
        }
        // A package-level `const`/`static` is a named C object (§2: `infy_<package>_<item>`).
        if let Some((idx, is_static)) = self
            .prog
            .consts
            .iter()
            .position(|c| c.name == n)
            .map(|i| (i, false))
            .or_else(|| self.prog.statics.iter().position(|c| c.name == n).map(|i| (i, true)))
        {
            let _ = idx;
            let item = self.hir.items.iter().enumerate().find_map(|(i, it)| match it {
                HirItem::Const(c) if c.name == n && (c.is_static == is_static) => Some(i),
                _ => None,
            });
            if let Some(i) = item {
                let (ty, sym) = match &self.hir.items[i] {
                    HirItem::Const(c) => (c.ty.clone(), c.symbol.clone()),
                    _ => return Err(CgError::new("§4.5", "codegen: 内部错误")),
                };
                let _ = e;
                return Ok(Val::of(ty, sym));
            }
        }
        match n {
            "true" => return Ok(Val::of(Ty::Bool, "1".to_string())),
            "false" => return Ok(Val::of(Ty::Bool, "0".to_string())),
            _ => {}
        }
        Err(CgError::new("§4.1", format!("codegen: 标识符 `{n}` 没有对应的绑定或数据项")))
    }

    /// `Phase::Fetch`, `build::gate`, `env.platform` (§4.6) and const paths.
    fn path_expr(&mut self, segs: &[String], e: &Expr, want: Option<&Ty>) -> Result<Val, CgError> {
        if segs.len() == 1 {
            return self.ident(&segs[0], e);
        }
        if segs.len() == 2 && segs[0] == "env" {
            let off = self.hir.env.offset_of(&segs[1]).ok_or_else(|| {
                CgError::new("§4.6", format!("codegen: `env.{}` 不在 §4.6 的封闭字段集里", segs[1]))
            })?;
            let f = self
                .hir
                .env
                .fields
                .iter()
                .find(|x| x.name == segs[1])
                .ok_or_else(|| CgError::new("§4.6", "codegen: 内部错误：env 字段表不一致"))?;
            let mut deps = BTreeSet::new();
            let ct = self.c_ty(&f.ty, false, &mut deps)?;
            // ABI §10: `env.x` is a load at a compile-time offset out of the host snapshot.
            return Ok(Val::of(
                f.ty.clone(),
                format!("(*((const {ct}*)((const uint8_t*)infy_env() + {off}u)))"),
            ));
        }
        // enum variant path
        let owner = &segs[segs.len() - 2];
        let variant = &segs[segs.len() - 1];
        if let Some((sym, idx)) = self.adt.get(owner).cloned() {
            if let HirItem::Enum(en) = &self.hir.items[idx] {
                if let Some(v) = en.variant(variant) {
                    if !v.payload.is_empty() {
                        return Err(CgError::new(
                            "§4.1",
                            format!("codegen: 变体 `{owner}::{variant}` 带载荷，必须写成调用形式"),
                        ));
                    }
                    let mut deps = BTreeSet::new();
                    let ct = self.c_ty(&Ty::Adt(owner.clone()), false, &mut deps)?;
                    let _ = sym;
                    return Ok(Val::of(
                        Ty::Adt(owner.clone()),
                        format!("({ct}){{ .tag = {}u }}", v.discriminant),
                    ));
                }
            }
        }
        // `pkg::item` — resolve as a function or a data item in another package (§5.3).
        let last = segs.last().cloned().unwrap_or_default();
        if let Some(fi) = self.prog.fns.iter().position(|f| f.name == last || f.name == format!("{owner}::{last}")) {
            let _ = fi;
            return Err(CgError::new(
                "§3.8",
                format!("codegen: 函数 `{}` 作为值使用需要函数指针，未实现（§4.1 只允许非捕获函数）", segs.join("::")),
            ));
        }
        if let Some(it) = self.hir.items.iter().find_map(|it| match it {
            HirItem::Const(c) if c.name == last => Some(c.clone()),
            _ => None,
        }) {
            return Ok(Val::of(it.ty, it.symbol));
        }
        let _ = want;
        Err(CgError::new(
            "§5.3",
            format!("codegen: 路径 `{}` 解析不到可生成的项", segs.join("::")),
        ))
    }

    // ------------------------------------------------------------------ calls

    /// §4.1/§4.11: `Ok`/`Err`/`Some`/`None` are constructors of the built-in Option/Result, so they
    /// resolve before the function table; their inner type comes from the **context**, because
    /// `sema::ctor_ty` can only record `Invalid` for the side the constructor does not carry.
    fn call(&mut self, callee: &Expr, args: &[crate::ast::CallArg], want: Option<&Ty>) -> Result<Val, CgError> {
        let (name, owner): (String, Option<String>) = match &callee.kind {
            ExprKind::Ident(n) => (n.clone(), None),
            ExprKind::Path(p) => (
                p.last().cloned().unwrap_or_default(),
                if p.len() >= 2 { Some(p[p.len() - 2].clone()) } else { None },
            ),
            other => {
                return Err(CgError::new(
                    "§3.8",
                    format!("codegen: 间接调用需要函数指针，实际是 {}", expr_kind_name(other)),
                ))
            }
        };
        if let Some(o) = &owner {
            if let Some((sym, idx)) = self.adt.get(o).cloned() {
                if let HirItem::Enum(en) = &self.hir.items[idx] {
                    if let Some(v) = en.variant(&name) {
                        return self.variant_ctor(&sym, &Ty::Adt(o.clone()), v, args);
                    }
                }
            }
        }
        if owner.is_none() {
            if let Some(v) = self.ctor(&name, args, want)? {
                return Ok(v);
            }
        }
        self.user_fn_call(&name, owner.as_deref(), args)
    }

    fn ctor(&mut self, name: &str, args: &[crate::ast::CallArg], want: Option<&Ty>) -> Result<Option<Val>, CgError> {
        if !matches!(name, "Ok" | "Err" | "Some" | "None") {
            return Ok(None);
        }
        let ty = want.cloned().ok_or_else(|| {
            CgError::new(
                "§4.1 / §4.13",
                format!("codegen: 构造器 `{name}(…)` 没有上下文类型，Option/Result 的内层类型无从确定"),
            )
        })?;
        let mut deps = BTreeSet::new();
        let arg0 = || args.first().map(|a| &a.expr);
        match (name, &ty) {
            ("Ok", Ty::Result { ok, .. }) => {
                let ct = self.c_ty(&ty, false, &mut deps)?;
                let v = match arg0() {
                    Some(e) => self.expr(e, Some(ok))?.text,
                    None => "{0}".to_string(),
                };
                Ok(Some(Val::of(ty, format!("({ct}){{ .ok = {v}, .is_ok = 1u }}"))))
            }
            ("Err", Ty::Result { err, .. }) => {
                let ct = self.c_ty(&ty, false, &mut deps)?;
                let v = match arg0() {
                    Some(e) => self.expr(e, Some(err))?.text,
                    None => "{0}".to_string(),
                };
                Ok(Some(Val::of(ty, format!("({ct}){{ .err = {v}, .is_ok = 0u }}"))))
            }
            ("Some", Ty::Option { inner }) => {
                let v = match arg0() {
                    Some(e) => self.expr(e, Some(inner))?.text,
                    None => "{0}".to_string(),
                };
                if Cg::niche(inner) {
                    Ok(Some(Val::of(ty, v)))
                } else {
                    let ct = self.c_ty(&ty, false, &mut deps)?;
                    Ok(Some(Val::of(ty, format!("({ct}){{ .val = {v}, .some = 1u }}"))))
                }
            }
            ("None", Ty::Option { inner }) => {
                if Cg::niche(inner) {
                    Ok(Some(Val::of(ty, "0".to_string())))
                } else {
                    let ct = self.c_ty(&ty, false, &mut deps)?;
                    Ok(Some(Val::of(ty, format!("({ct}){{0}}"))))
                }
            }
            (n, other) => Err(CgError::new(
                "§4.11",
                format!("codegen: `{n}(…)` 用在了 {} 上（E-0171/E-0172 应已由阶段 [4] 报出）", other.text()),
            )),
        }
    }

    fn variant_ctor(
        &mut self,
        sym: &str,
        ty: &Ty,
        v: &crate::hir::HirVariant,
        args: &[crate::ast::CallArg],
    ) -> Result<Val, CgError> {
        if v.payload.is_empty() && !args.is_empty() {
            return Err(CgError::new(
                "§4.1",
                format!("codegen: 无载荷变体不接受实参（给了 {} 个）", args.len()),
            ));
        }
        let _ = sym;
        let mut deps = BTreeSet::new();
        let ct = self.c_ty(ty, false, &mut deps)?;
        let mut inits = Vec::new();
        for (i, p) in v.payload.iter().enumerate() {
            let a = args.get(i).map(|x| &x.expr).ok_or_else(|| {
                CgError::new("§4.1", format!("codegen: 变体构造缺少第 {} 个载荷实参", i + 1))
            })?;
            let av = self.expr(a, Some(&p.ty))?;
            inits.push(format!(".a{i} = {}", av.text));
        }
        let member = c_ident(&v.name);
        let payload = if inits.is_empty() {
            String::new()
        } else {
            format!(", .v = {{ .{member} = {{ {} }} }}", inits.join(", "))
        };
        Ok(Val::of(
            ty.clone(),
            format!("({ct}){{ .tag = {}u{payload} }}", v.discriminant),
        ))
    }

    /// An ordinary call. §4.10 rule 3 fixes left-to-right argument evaluation, so every argument is
    /// lowered in order before the call text is built (C leaves argument order unsequenced).
    fn user_fn_call(
        &mut self,
        name: &str,
        owner: Option<&str>,
        args: &[crate::ast::CallArg],
    ) -> Result<Val, CgError> {
        let f = self.resolve_fn(name, owner)?;
        // §7.6.3 项 2: an A-track call goes to the wrapper symbol, never to the bare C name — that
        // is exactly what keeps two same-named declarations with different signatures apart.
        let wrapper = if f.is_foreign {
            match self.atrack.get(&f.symbol) {
                Some(w) => Some(w.clone()),
                None => {
                    return Err(CgError::new(
                        "§7.6.4",
                        format!(
                            "codegen: 调用 A 轨 `extern` 函数 `{name}` 被 §7.6.4 的封闭表拒绝：{}",
                            self.atrack_refused.get(&f.symbol).cloned().unwrap_or_else(|| "该声明不在任何 `#[link(name = \"…\")]` 签名块里（§7.6.1）".to_string())
                        ),
                    ))
                }
            }
        } else {
            None
        };
        if f.is_event {
            return Err(CgError::new(
                "§11",
                format!("codegen: 调用 `event` `{name}` 需走 `infy_host_call_event`（`src/host.rs` 是占位）"),
            ));
        }
        let mut slots: Vec<Option<&Expr>> = vec![None; f.params.len()];
        for (i, a) in args.iter().enumerate() {
            let idx = match &a.name {
                Some(n) => f.params.iter().position(|p| &p.name == n).ok_or_else(|| {
                    CgError::new("§2.6", format!("codegen: `{name}` 没有名为 `{n}` 的参数（E-0900 属阶段 [4]）"))
                })?,
                None => i,
            };
            if idx >= slots.len() {
                return Err(CgError::new(
                    "§3.3",
                    format!("codegen: `{name}` 的实参多于形参（第 {} 个）", idx + 1),
                ));
            }
            slots[idx] = Some(&a.expr);
        }
        let mut argv = Vec::new();
        for (idx, (p, slot)) in f.params.iter().zip(slots.iter()).enumerate() {
            let e = slot.ok_or_else(|| {
                CgError::new(
                    "§3.3",
                    format!("codegen: `{name}` 缺少第 {} 个实参 `{}`（默认参数发射未实现）", idx + 1, p.name),
                )
            })?;
            // An A-track callee needs no special case: §7.6.4's table has no row that carries an
            // owned value across the boundary (`String`/`Vec` are `Forbidden` → `E-0723` at stage
            // [4], measured with `infy check`), so a move never reaches this call site.
            self.cancel_owned_move(e, &p.ty)?;
            let v = self.expr(e, Some(&p.ty))?;
            argv.push(v.text);
        }
        let ret = match &f.ret {
            Some((t, _)) => t.clone(),
            None if f.diverges => Ty::Never,
            None => Ty::Unit,
        };
        let call = match &wrapper {
            Some(w) => {
                let text = w.call(&argv);
                // A `()`-returning declaration maps to a `void` wrapper (§7.6.4 gives no row for
                // `()`), and `InfyUnit tmp = void_expr;` is not C. The comma operator keeps the call
                // an expression without inventing a runtime helper.
                if w.c_ret.is_none() {
                    format!("((void){text}, (InfyUnit){{0}})")
                } else {
                    text
                }
            }
            None => format!("{}({})", f.symbol, argv.join(", ")),
        };
        let tmp = self.fresh(name);
        self.decl_stmt(&ret, &tmp, Some(&call))?;
        Ok(Val::of(ret, tmp))
    }

    fn resolve_fn(&self, name: &str, owner: Option<&str>) -> Result<crate::hir::HirFn, CgError> {
        // §5.3 + §4.14: a bare call resolves in the **caller's** package first; only an owner prefix
        // (`build::gate`) selects another package. Matching by simple name across the whole crate
        // silently picked the wrong `gate` when two packages export one (found on examples/tour).
        let mut cands: Vec<crate::hir::HirFn> = self
            .hir
            .items
            .iter()
            .filter_map(|it| match it {
                HirItem::Fn(f) if f.name == name || f.name.ends_with(&format!("::{name}")) => Some(f.clone()),
                _ => None,
            })
            .collect();
        if let Some(o) = owner {
            cands.retain(|f| f.package.rsplit('/').next().unwrap_or("") == o || f.package.ends_with(o));
        } else {
            let local: Vec<crate::hir::HirFn> =
                cands.iter().filter(|f| f.package == self.cur_package).cloned().collect();
            if local.len() == 1 {
                return Ok(local.into_iter().next().expect("checked"));
            }
            if local.len() > 1 {
                cands = local;
            }
        }
        if cands.is_empty() {
            return Err(CgError::new(
                "§5.3",
                format!(
                    "codegen: 找不到函数 `{}`",
                    owner.map(|o| format!("{o}::{name}")).unwrap_or_else(|| name.to_string())
                ),
            ));
        }
        if cands.len() > 1 {
            let who = cands
                .iter()
                .map(|f| format!("{}::{}", f.package, f.name))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(CgError::new(
                "§4.14",
                format!("codegen: 函数名 `{name}` 有多个候选（{who}），无法确定调用目标"),
            ));
        }
        Ok(cands.into_iter().next().expect("checked non-empty"))
    }

    /// Container methods with a real entry point in `infy_rt.h`. Anything outside this closed set
    /// is refused by name — §4.14's automatic reference ladder is not reproduced here.
    fn method(
        &mut self,
        recv: &Expr,
        name: &str,
        args: &[crate::ast::CallArg],
        want: Option<&Ty>,
    ) -> Result<Val, CgError> {
        let r = self.expr(recv, None)?;
        let base = self.base_ty(&r.ty);
        match name {
            "len" => match &base {
                Ty::Vec { .. } | Ty::String | Ty::Str | Ty::Slice { .. } => {
                    Ok(Val::of(Ty::Prim(Prim::Usize), format!("({}.len)", r.text)))
                }
                Ty::Array { n, .. } => Ok(Val::of(Ty::Prim(Prim::Usize), format!("{n}u"))),
                other => Err(CgError::new(
                    "§10.13",
                    format!("codegen: `.len()` 不适用于 {}", other.text()),
                )),
            },
            "is_some" | "is_none" => {
                let inner = match &base {
                    Ty::Option { inner } => (**inner).clone(),
                    other => {
                        return Err(CgError::new(
                            "§4.1",
                            format!("codegen: `.{name}()` 只适用于 Option，实际是 {}", other.text()),
                        ))
                    }
                };
                let cond = if Cg::niche(&inner) {
                    format!("({} != 0)", r.text)
                } else {
                    format!("({}.some != 0u)", r.text)
                };
                let _ = want;
                Ok(Val::of(Ty::Bool, if name == "is_some" { cond } else { format!("!{cond}") }))
            }
            "is_ok" | "is_err" => {
                if !matches!(base, Ty::Result { .. }) {
                    return Err(CgError::new(
                        "§4.1",
                        format!("codegen: `.{name}()` 只适用于 Result，实际是 {}", base.text()),
                    ));
                }
                let cond = format!("({}.is_ok != 0u)", r.text);
                Ok(Val::of(Ty::Bool, if name == "is_ok" { cond } else { format!("!{cond}") }))
            }
            "unwrap" | "unwrap_or" => {
                let (pty, niche, access, is_result) = match &base {
                    Ty::Option { inner } => ((**inner).clone(), Cg::niche(inner), format!("{}.val", r.text), false),
                    Ty::Result { ok, .. } => ((**ok).clone(), false, format!("{}.ok", r.text), true),
                    other => {
                        return Err(CgError::new(
                            "§4.1 / §10.9",
                            format!("codegen: `.unwrap()` 只适用于 Option/Result，实际是 {}", other.text()),
                        ))
                    }
                };
                let tmp = self.fresh("unw");
                self.decl_stmt(&base, &tmp, Some(&r.text))?;
                let ok = if niche {
                    format!("{tmp} != 0")
                } else if is_result {
                    format!("{tmp}.is_ok != 0u")
                } else {
                    format!("{tmp}.some != 0u")
                };
                self.b(&format!("if (!({ok})) {{"));
                self.depth += 1;
                let at = self.loc();
                self.b(&format!(
                    "infy_panic(R_UNWRAP_NONE, \"{}\", {at});",
                    if is_result { "unwrap on Err" } else { "unwrap on None" }
                ));
                self.depth -= 1;
                self.b("}");
                let access = if niche { tmp.clone() } else { access };
                if name == "unwrap_or" {
                    let d = args
                        .first()
                        .map(|a| &a.expr)
                        .ok_or_else(|| CgError::new("§4.1", "codegen: `unwrap_or` 缺少默认值实参"))?;
                    let dv = self.expr(d, Some(&pty))?;
                    let out = self.fresh("uo");
                    self.decl_stmt(&pty, &out, Some(&format!("({ok}) ? ({access}) : ({})", dv.text)))?;
                    return Ok(Val::of(pty, out));
                }
                Ok(Val::of(pty, access))
            }
            "push_str" => {
                if !matches!(base, Ty::String) {
                    return Err(CgError::new(
                        "§10.11",
                        format!("codegen: `.push_str()` 只适用于 String，实际是 {}", base.text()),
                    ));
                }
                let a = args
                    .first()
                    .map(|x| &x.expr)
                    .ok_or_else(|| CgError::new("§10.11", "codegen: `.push_str()` 缺少实参"))?;
                let av = self.expr(a, Some(&Ty::Str))?;
                self.b(&format!("infy_string_push_str(&{}, {});", r.text, av.text));
                Ok(Val::of(Ty::Unit, "(InfyUnit){0}".to_string()))
            }
            "push" => {
                let elem = match &base {
                    Ty::Vec { inner } => (**inner).clone(),
                    other => {
                        return Err(CgError::new(
                            "§10.13",
                            format!("codegen: `.push()` 只适用于 Vec，实际是 {}", other.text()),
                        ))
                    }
                };
                let a = args
                    .first()
                    .map(|x| &x.expr)
                    .ok_or_else(|| CgError::new("§10.13", "codegen: `.push()` 缺少实参"))?;
                let av = self.expr(a, Some(&elem))?.text;
                let esz = self.size_of(&elem)?;
                let eal = self.align_of(&elem)?;
                let cty = self.c_ty_isolated(&elem)?;
                let recv = r.text.clone();
                self.b(&format!(
                    "{{ {cty} a_ = {av}; infy_vec_push(&{recv}, &a_, {esz}u, {eal}u); }}"
                ));
                Ok(Val::of(Ty::Unit, "(InfyUnit){0}".to_string()))
            }
            "as_str" => {
                if !matches!(base, Ty::String) {
                    return Err(CgError::new(
                        "§4.1",
                        format!("codegen: `.as_str()` 只适用于 String，实际是 {}", base.text()),
                    ));
                }
                Ok(Val::of(Ty::Str, format!("infy_string_as_str(&{})", r.text)))
            }
            other => self.adt_method(&r, other, args),
        }
    }

    /// §4.14: an inherent or trait method on a named ADT. The receiver is passed first, by address
    /// when the declaration takes a reference.
    fn adt_method(
        &mut self,
        r: &Val,
        name: &str,
        args: &[crate::ast::CallArg],
    ) -> Result<Val, CgError> {
        let ty_name = base_name_of(&r.ty);
        let cands: Vec<crate::hir::HirFn> = self
            .hir
            .items
            .iter()
            .filter_map(|it| match it {
                HirItem::Fn(f)
                    if (f.name == name || f.name.ends_with(&format!("::{name}")))
                        && f.self_ty.as_ref().map(|(st, _)| base_name_of(st) == ty_name).unwrap_or(false) =>
                {
                    Some(f.clone())
                }
                _ => None,
            })
            .collect();
        if cands.len() != 1 {
            return Err(CgError::new(
                "§4.14",
                format!(
                    "codegen: 方法 `{}.{name}()` 解析到 {} 个候选（后端不做自动引用阶梯与 trait 权重排序）",
                    ty_name,
                    cands.len()
                ),
            ));
        }
        let f = cands.into_iter().next().expect("length checked");
        let mut argv = Vec::new();
        match f.params.first().map(|p| p.ty.clone()) {
            Some(Ty::Ref { .. }) => argv.push(format!("(&{})", r.text)),
            Some(Ty::Ptr { .. }) => argv.push(format!("(&{})", r.text)),
            _ => argv.push(r.text.clone()),
        }
        for (i, a) in args.iter().enumerate() {
            let pt = f.params.get(i + 1).map(|p| &p.ty);
            let v = self.expr(&a.expr, pt)?;
            argv.push(v.text);
        }
        let ret = match &f.ret {
            Some((t, _)) => t.clone(),
            None if f.diverges => Ty::Never,
            None => Ty::Unit,
        };
        let call = format!("{}({})", f.symbol, argv.join(", "));
        let tmp = self.fresh(name);
        self.decl_stmt(&ret, &tmp, Some(&call))?;
        Ok(Val::of(ret, tmp))
    }

    // ------------------------------------------------------------------ operators

    fn unary(&mut self, op: Punct, e: &Expr, want: Option<&Ty>) -> Result<Val, CgError> {
        match op {
            Punct::Bang => {
                let v = self.expr(e, Some(&Ty::Bool))?;
                Ok(Val::of(Ty::Bool, format!("(!({}))", v.text)))
            }
            Punct::Minus => {
                let v = self.expr(e, want)?;
                match &v.ty {
                    Ty::Prim(p) if p.is_signed() || p.is_float() => Ok(Val::of(v.ty.clone(), format!("(-({}))", v.text))),
                    Ty::Prim(p) => Err(CgError::new(
                        "§4.4",
                        format!("codegen: 无符号类型 {} 不能取负（E-0111 属阶段 [4]）", p.text()),
                    )),
                    other => Err(CgError::new("§4.4", format!("codegen: `-` 不适用于 {}", other.text()))),
                }
            }
            Punct::Star => {
                let v = self.expr(e, want)?;
                match &v.ty {
                    Ty::Ref { inner, .. } | Ty::Ptr { inner, .. } | Ty::Boxed { inner } => {
                        Ok(Val::of((**inner).clone(), format!("(*{})", v.text)))
                    }
                    other => Err(CgError::new(
                        "§4.7.5",
                        format!("codegen: `*` 只能解引用引用/指针/Box，实际是 {}", other.text()),
                    )),
                }
            }
            other => Err(CgError::new(
                "§2.7",
                format!("codegen: 一元 `{}` 没有对应的 C11 发射", punct_text(other)),
            )),
        }
    }

    fn binary(&mut self, op: Punct, lhs: &Expr, rhs: &Expr, site: &Expr) -> Result<Val, CgError> {
        if matches!(op, Punct::AmpAmp) {
            let a = self.expr(lhs, Some(&Ty::Bool))?;
            let b = self.expr(rhs, Some(&Ty::Bool))?;
            return Ok(Val::of(Ty::Bool, format!("(({}) && ({}))", a.text, b.text)));
        }
        if matches!(op, Punct::PipePipe) {
            let a = self.expr(lhs, Some(&Ty::Bool))?;
            let b = self.expr(rhs, Some(&Ty::Bool))?;
            return Ok(Val::of(Ty::Bool, format!("(({}) || ({}))", a.text, b.text)));
        }
        let (lt, rt) = self.binary_operand_types(lhs, rhs, site)?;
        let a = self.expr(lhs, Some(&lt))?;
        let b = self.expr(rhs, Some(&rt))?;
        match op {
            Punct::EqEq => self.eq_text(&a, &b, true).map(|t| Val::of(Ty::Bool, t)),
            Punct::BangEq => self.eq_text(&a, &b, false).map(|t| Val::of(Ty::Bool, t)),
            Punct::Lt | Punct::LtEq | Punct::Gt | Punct::GtEq => {
                let o = match op {
                    Punct::Lt => "<",
                    Punct::LtEq => "<=",
                    Punct::Gt => ">",
                    _ => ">=",
                };
                Ok(Val::of(Ty::Bool, format!("(({}) {} ({}))", a.text, o, b.text)))
            }
            Punct::Plus | Punct::Minus | Punct::Star | Punct::Slash | Punct::Percent => {
                let name = match op {
                    Punct::Plus => "add",
                    Punct::Minus => "sub",
                    Punct::Star => "mul",
                    Punct::Slash => "div",
                    _ => "rem",
                };
                let text = self.checked_arith(name, &lt, &a.text, &b.text)?;
                Ok(Val::of(lt.clone(), text))
            }
            Punct::Shl | Punct::Shr => {
                let width = match &lt {
                    Ty::Prim(p) => p.bits(),
                    other => {
                        return Err(CgError::new(
                            "§4.12",
                            format!("codegen: 移位只支持整型，实际是 {}", other.text()),
                        ))
                    }
                };
                if width != 64 {
                    return Err(CgError::new(
                        "§4.12",
                        format!(
                            "codegen: `infy_rt.h` 只提供 `infy_shl_u64`/`infy_shr_u64`/`infy_shr_i64`，{} 位宽的移位没有对应的 R-0007 检查入口，拒绝生成（见 docs/BACKLOG.md）",
                            width
                        ),
                    ));
                }
                let signed = matches!(lt, Ty::Prim(Prim::I64 | Prim::Isize));
                let call = match op {
                    Punct::Shl => format!("infy_shl_u64((uint64_t)({}), (uint32_t)({}), {})", a.text, b.text, self.loc()),
                    _ if signed => format!("infy_shr_i64((int64_t)({}), (uint32_t)({}), {})", a.text, b.text, self.loc()),
                    _ => format!("infy_shr_u64((uint64_t)({}), (uint32_t)({}), {})", a.text, b.text, self.loc()),
                };
                Ok(Val::of(lt.clone(), call))
            }
            Punct::Amp | Punct::Pipe | Punct::Hat => {
                let o = match op {
                    Punct::Amp => "&",
                    Punct::Pipe => "|",
                    _ => "^",
                };
                Ok(Val::of(lt.clone(), format!("(({}) {} ({}))", a.text, o, b.text)))
            }
            other => Err(CgError::new(
                "§2.7",
                format!("codegen: 二元 `{}` 没有对应的 C11 发射", punct_text(other)),
            )),
        }
    }

    /// §4.13 rule 2: an unsuffixed literal takes the type of the other side, so `u32 + 1` lowers to
    /// `infy_add_u32` rather than a widened `i32` add.
    fn binary_operand_types(&mut self, lhs: &Expr, rhs: &Expr, site: &Expr) -> Result<(Ty, Ty), CgError> {
        let mut l = self.prog.ty(lhs.span).cloned().or_else(|| self.prog.ty(site.span).cloned());
        let mut r = self.prog.ty(rhs.span).cloned();
        let widen_int_lit = |x: &mut Option<Ty>, other: &Option<Ty>| {
            let needs = x.is_none() || matches!(x.as_ref(), Some(Ty::IntLit));
            if needs {
                if let Some(o) = other {
                    if o.is_integer() {
                        *x = Some(o.clone());
                    }
                }
            }
        };
        widen_int_lit(&mut l, &r);
        widen_int_lit(&mut r, &l);
        let mut l = l.unwrap_or(Ty::Prim(Prim::I32));
        if matches!(l, Ty::IntLit) {
            l = Ty::Prim(Prim::I32);
        }
        let mut r = r.unwrap_or_else(|| l.clone());
        if matches!(r, Ty::IntLit) {
            r = l.clone();
        }
        if let (Ty::Prim(a), Ty::Prim(b)) = (&l, &r) {
            if a != b && !(a.is_numeric() && b.is_numeric()) {
                return Err(CgError::new(
                    "§4.4",
                    format!("codegen: 二元两侧类型不一致（{} vs {}），语言层没有隐式转换", a.text(), b.text()),
                ));
            }
        } else if l.text() != r.text() && !(l.is_numeric() && r.is_numeric()) {
            return Err(CgError::new(
                "§4.4",
                format!("codegen: 二元两侧类型不一致（{} vs {}），语言层没有隐式转换", l.text(), r.text()),
            ));
        }
        Ok((l, r))
    }

    /// The checked helper for `op` over `ty` (§4.9, §7.2: never optional, never inlined as a bare
    /// C operator for integers).
    fn checked_arith(&mut self, op: &str, ty: &Ty, a: &str, b: &str) -> Result<String, CgError> {
        if !self.cfg.overflow_panic {
            return Err(CgError::new(
                "§5.2 / §4.4",
                "codegen: `[profile] overflow = \"saturate\"` 未实现；不静默按 panic 语义生成",
            ));
        }
        if matches!(ty, Ty::Prim(Prim::U128 | Prim::I128)) {
            return Err(CgError::new("§4.12", "codegen: 128 位算术没有运行时助手，未实现"));
        }
        let suffix = match ty {
            Ty::Prim(Prim::U8) => "u8",
            Ty::Prim(Prim::U16) => "u16",
            Ty::Prim(Prim::U32) => "u32",
            Ty::Prim(Prim::U64) | Ty::Prim(Prim::Usize) => "u64",
            Ty::Prim(Prim::I8) => "i8",
            Ty::Prim(Prim::I16) => "i16",
            Ty::Prim(Prim::I32) => "i32",
            Ty::Prim(Prim::I64) | Ty::Prim(Prim::Isize) => "i64",
            Ty::Prim(Prim::F32) => "f32",
            Ty::Prim(Prim::F64) => "f64",
            Ty::Prim(p) => {
                return Err(CgError::new(
                    "§4.4",
                    format!("codegen: `{}` 不参与算术（E-0111 属阶段 [4]）", p.text()),
                ))
            }
            other => {
                return Err(CgError::new(
                    "§4.4",
                    format!("codegen: 算术只支持原语数值类型，实际是 {}", other.text()),
                ))
            }
        };
        if matches!(ty, Ty::Prim(Prim::F32 | Prim::F64)) {
            // §4.4: float add/sub/mul/div are plain IEEE ops (`-ffp-contract=off` is the toolchain's
            // job); only remainder has explicit failure modes R-0002/R-0008 (§4.12).
            if op != "rem" {
                let symbol = match op {
                    "add" => "+",
                    "sub" => "-",
                    "mul" => "*",
                    _ => "/",
                };
                return Ok(format!("(({a}) {symbol} ({b}))"));
            }
            let helper = if suffix == "f32" { "infy_remf32" } else { "infy_remf64" };
            return Ok(format!("{helper}({a}, {b}, {})", self.loc()));
        }
        Ok(format!("infy_{op}_{suffix}({a}, {b}, {})", self.loc()))
    }

    /// §4.4: `str` comparison is NFC content, not pointer equality.
    fn eq_text(&mut self, a: &Val, b: &Val, same: bool) -> Result<String, CgError> {
        let is_str = matches!(a.ty, Ty::Str) || matches!(b.ty, Ty::Str);
        let cmp = if is_str {
            let x = if matches!(a.ty, Ty::Str) { a.text.clone() } else { self.str_of(&b.ty, &b.text)? };
            let y = if matches!(b.ty, Ty::Str) { b.text.clone() } else { self.str_of(&a.ty, &a.text)? };
            format!("(infy_str_eq_nfc({x}, {y}) != 0)")
        } else {
            format!("(({}) == ({}))", a.text, b.text)
        };
        Ok(if same { cmp } else { format!("!{cmp}") })
    }

    fn str_of(&mut self, ty: &Ty, text: &str) -> Result<String, CgError> {
        match ty {
            Ty::String => Ok(format!("infy_string_as_str(&{text})")),
            Ty::Ref { inner, .. } if matches!(&**inner, Ty::Str) => Ok(format!("(*{text})")),
            other => Err(CgError::new(
                "§4.4",
                format!("codegen: 与 `str` 比较需要一侧可转成 `InfyStr`，实际是 {}", other.text()),
            )),
        }
    }

    // ------------------------------------------------------------------ structure

    fn base_ty(&self, ty: &Ty) -> Ty {
        match ty {
            Ty::Ref { inner, .. } | Ty::Ptr { inner, .. } | Ty::Boxed { inner } => self.base_ty(inner),
            other => other.clone(),
        }
    }

    fn elem_ty(&mut self, ty: &Ty) -> Result<Ty, CgError> {
        match ty {
            Ty::Array { inner, .. } | Ty::Slice { inner } | Ty::Vec { inner } => Ok((**inner).clone()),
            other => Err(CgError::new(
                "§4.1",
                format!("codegen: `[]` 只适用于 [T; N]、[T]、Vec<T>，实际是 {}", other.text()),
            )),
        }
    }

    /// `->` behind a reference or pointer (§1: `&T` **is** `T*`), `.` otherwise.
    fn field_text(&mut self, b: &Val, field: &str) -> String {
        match &b.ty {
            Ty::Ref { .. } | Ty::Ptr { .. } | Ty::Boxed { .. } => format!("{}->{field}", b.text),
            _ => format!("{}.{field}", b.text),
        }
    }

    fn field_of(&mut self, ty: &Ty, name: &FieldName) -> Result<(String, Ty), CgError> {
        let head = self.base_ty(ty);
        match (&head, name) {
            (Ty::Named(n) | Ty::Adt(n), FieldName::Named(f)) => {
                let (_, idx) = self.adt_get(n)?;
                match &self.hir.items[idx] {
                    HirItem::Struct(s) => {
                        let fld = s.fields.iter().find(|x| &x.name == f).ok_or_else(|| {
                            CgError::new("§4.1", format!("codegen: `{n}` 没有字段 `{f}`"))
                        })?;
                        Ok((c_ident(&fld.name), fld.ty.clone()))
                    }
                    HirItem::Enum(_) => Err(CgError::new(
                        "§4.1",
                        format!("codegen: 枚举 `{n}` 的载荷经模式匹配访问，没有具名字段"),
                    )),
                    _ => Err(CgError::new("§4.1", "codegen: 内部错误：ADT 索引指向非 ADT")),
                }
            }
            (Ty::Tuple(v), FieldName::Index(i)) => {
                let f = v.get(*i as usize).ok_or_else(|| {
                    CgError::new("§3.11", format!("codegen: 元组没有第 {i} 个元素（长度 {}）", v.len()))
                })?;
                Ok((format!("a{i}"), f.clone()))
            }
            (other, FieldName::Index(i)) => Err(CgError::new(
                "§3.11",
                format!("codegen: `.{i}` 只适用于元组，实际是 {}", other.text()),
            )),
            (other, FieldName::Named(f)) => Err(CgError::new(
                "§4.1",
                format!("codegen: `.{f}` 不适用于 {}", other.text()),
            )),
        }
    }

    /// An index expression. The bounds check is its own statement so R-0004 stays in the emitted
    /// code at every `-O` level (§4.9).
    fn index_text(&mut self, b: &Val, i: &Val) -> Result<String, CgError> {
        match &b.ty {
            Ty::Array { n, .. } => {
                self.b(&format!("if (!((uint64_t)({i}) < {n}u)) {{", i = i.text, n = n));
                self.depth += 1;
                let at = self.loc();
                self.b(&format!(
                    "infy_panic(R_INDEX_OOB, \"array index out of bounds (§4.9 R-0004)\", {at});"
                ));
                self.depth -= 1;
                self.b("}");
                Ok(format!("{}[{}]", b.text, i.text))
            }
            Ty::Vec { inner } => {
                let esz = self.size_of(inner)?;
                let mut deps = BTreeSet::new();
                let ct = self.c_ty(inner, false, &mut deps)?;
                Ok(format!(
                    "*({ct}*)infy_vec_index(&{}, (uint64_t)({}), {esz}u, {})",
                    b.text,
                    i.text,
                    self.loc()
                ))
            }
            Ty::Slice { inner } => {
                let esz = self.size_of(inner)?;
                let mut deps = BTreeSet::new();
                let ct = self.c_ty(inner, false, &mut deps)?;
                Ok(format!(
                    "*(const {ct}*)infy_slice_index(&{}, (uint64_t)({}), {esz}u, {})",
                    b.text,
                    i.text,
                    self.loc()
                ))
            }
            other => Err(CgError::new(
                "§4.1",
                format!("codegen: `[]` 只适用于 [T; N]、[T]、Vec<T>，实际是 {}", other.text()),
            )),
        }
    }

    /// A struct literal. `..base` (functional update, §3.11) is emitted as a copy plus field writes,
    /// which is what the record layout implies.
    fn struct_lit(
        &mut self,
        path: &[String],
        fields: &[(String, Expr)],
        rest: Option<&Expr>,
        want: Option<&Ty>,
    ) -> Result<Val, CgError> {
        let name = path.last().cloned().unwrap_or_default();
        let (ty, idx) = match want.cloned() {
            Some(t @ (Ty::Named(_) | Ty::Adt(_))) => {
                let n = match &t {
                    Ty::Named(x) | Ty::Adt(x) => x.clone(),
                    _ => unreachable!(),
                };
                let (_, i) = self.adt_get(&n)?;
                (t, i)
            }
            _ => {
                let (_, i) = self.adt_get(&name)?;
                (Ty::Adt(name.clone()), i)
            }
        };
        let s = match &self.hir.items[idx] {
            HirItem::Struct(s) => s.clone(),
            other => {
                return Err(CgError::new(
                    "§4.1",
                    format!("codegen: 结构体字面量 `{}` 解析到的不是 struct（{}）", name, item_kind_name(other)),
                ))
            }
        };
        let mut deps = BTreeSet::new();
        let ct = self.c_ty(&ty, false, &mut deps)?;
        let mut inits = Vec::new();
        for (fname, fe) in fields {
            let f = s.fields.iter().find(|x| &x.name == fname).ok_or_else(|| {
                CgError::new("§4.1", format!("codegen: `{}` 没有字段 `{fname}`", s.name))
            })?;
            let v = self.expr(fe, Some(&f.ty))?;
            inits.push((c_ident(&f.name), v.text));
        }
        let tmp = self.fresh(&s.name);
        if let Some(base) = rest {
            let bv = self.expr(base, Some(&ty))?;
            self.b(&format!("{} {tmp} = {};", declarator(&ct, &tmp), bv.text));
            for (n, v) in &inits {
                self.b(&format!("{tmp}.{n} = {v};"));
            }
            return Ok(Val::of(ty, tmp));
        }
        if inits.len() != s.fields.len() {
            let given: BTreeSet<String> = fields.iter().map(|(n, _)| n.clone()).collect();
            let missing: Vec<String> = s
                .fields
                .iter()
                .filter(|f| !given.contains(&f.name))
                .map(|f| f.name.clone())
                .collect();
            return Err(CgError::new(
                "§3.11",
                format!("codegen: 结构体字面量 `{}` 缺字段 {missing:?}", s.name),
            ));
        }
        let parts = inits.iter().map(|(n, v)| format!(".{n} = {v}")).collect::<Vec<_>>().join(", ");
        if s.fields.is_empty() {
            return Ok(Val::of(ty, format!("({ct}){{ ._0 = 0 }}")));
        }
        Ok(Val::of(ty, format!("({ct}){{ {parts} }}")))
    }

    fn tuple_lit(&mut self, items: &[Expr], want: Option<&Ty>) -> Result<Val, CgError> {
        let tys: Vec<Ty> = match want.cloned() {
            Some(Ty::Tuple(v)) => v,
            _ => items
                .iter()
                .map(|e| self.prog.ty(e.span).cloned().unwrap_or(Ty::Invalid))
                .collect(),
        };
        if tys.len() != items.len() {
            return Err(CgError::new(
                "§3.11",
                format!("codegen: 元组字面量有 {} 个元素，上下文类型有 {} 个", items.len(), tys.len()),
            ));
        }
        let ty = Ty::Tuple(tys.clone());
        let mut deps = BTreeSet::new();
        let ct = self.c_ty(&ty, false, &mut deps)?;
        let mut inits = Vec::new();
        for (i, (e, t)) in items.iter().zip(tys.iter()).enumerate() {
            let v = self.expr(e, Some(t))?;
            inits.push(format!(".a{i} = {}", v.text));
        }
        Ok(Val::of(ty, format!("({ct}){{ {} }}", inits.join(", "))))
    }

    // ------------------------------------------------------------------ match (§3.12)

    /// Compile a `match` into an `if`/`else if` chain over the scrutinee. With `want` the arm values
    /// land in a result temporary, so `match` is usable as an expression (§3.11).
    fn match_into(
        &mut self,
        scrut: &Expr,
        arms: &[crate::ast::MatchArm],
        want: Option<&Ty>,
    ) -> Result<Option<Val>, CgError> {
        let sv = self.expr(scrut, None)?;
        let res = match want.cloned() {
            Some(t) if !matches!(t, Ty::Unit) => {
                let name = self.fresh("mres");
                self.decl_stmt(&t, &name, Some("{0}"))?;
                Some((name, t))
            }
            _ => None,
        };
        for (i, arm) in arms.iter().enumerate() {
            if arm.guard.is_some() {
                return Err(CgError::new(
                    "§3.12",
                    "codegen: match 臂的 guard 未实现（guard 必须在绑定生效之后求值）",
                ));
            }
            let cond = self.pat_cond(&arm.pat, &sv)?;
            self.b(&format!("{} ({}) {{", if i == 0 { "if" } else { "else if" }, cond));
            self.depth += 1;
            self.open_scope();
            self.pat_bind(&arm.pat, &sv)?;
            match (&arm.body, &res) {
                (ArmBody::Expr(e), Some((name, t))) => {
                    let v = self.expr(e, Some(t))?;
                    self.b(&format!("{name} = {};", v.text));
                }
                (ArmBody::Expr(e), None) => {
                    let v = self.expr(e, None)?;
                    self.b(&format!("(void){};", v.text));
                }
                (ArmBody::Block(b), Some((name, t))) => {
                    self.block_into(b, Some(&(name.clone(), t.clone())))?;
                }
                (ArmBody::Block(b), None) => {
                    self.block_body(b, None, false)?;
                }
            }
            self.close_scope();
            self.depth -= 1;
            self.b("}");
        }
        // §3.12 makes exhaustiveness a stage-[4] property, so this branch is unreachable by
        // construction — but it cannot be empty either, or a result temp would stay uninitialised.
        // `infy_panic` never returns, which keeps every path defined (D-30).
        let at = self.loc();
        self.b(&format!(
            "else {{ infy_panic(R_BAD_BOOL, \"codegen: match 未命中任何臂（§3.12 穷尽性由阶段 [4] 判定；走到这里说明后端与阶段 [4] 不一致）\", {at}); }}"
        ));
        Ok(res.map(|(n, t)| Val::of(t, n)))
    }

    fn pat_cond(&mut self, pat: &Pat, sv: &Val) -> Result<String, CgError> {
        Ok(match &pat.kind {
            PatKind::Wildcard | PatKind::Binding { .. } => "1".to_string(),
            PatKind::Lit(l) => {
                let holder = Expr::new(ExprKind::Lit(l.clone()), pat.span);
                let v = self.literal(l, Some(&sv.ty), &holder)?;
                self.eq_text(sv, &v, true)?
            }
            PatKind::Path(segs) => {
                if segs.len() == 1 {
                    match segs[0].as_str() {
                        "None" => {
                            let inner = match &sv.ty {
                                Ty::Option { inner } => (**inner).clone(),
                                other => {
                                    return Err(CgError::new(
                                        "§4.1",
                                        format!("codegen: `None` 模式用在了 {}", other.text()),
                                    ))
                                }
                            };
                            if Cg::niche(&inner) {
                                format!("({} == 0)", sv.text)
                            } else {
                                format!("({}.some == 0u)", sv.text)
                            }
                        }
                        "Some" | "Ok" | "Err" => {
                            return Err(CgError::new(
                                "§3.12",
                                format!("codegen: 构造器 `{}` 的模式必须带子模式", segs[0]),
                            ))
                        }
                        n => {
                            let c = self
                                .prog
                                .consts
                                .iter()
                                .find(|c| c.name == n)
                                .cloned()
                                .or_else(|| self.prog.statics.iter().find(|c| c.name == n).cloned())
                                .ok_or_else(|| {
                                    CgError::new(
                                        "§3.12",
                                        format!("codegen: 单段路径模式 `{n}` 既不是 `None` 也不是常量"),
                                    )
                                })?;
                            let v = match c.value {
                                Some(cv) => self.const_text(cv, &c.ty)?,
                                None => crate::hir::mangle(&c.package, &c.name),
                            };
                            format!("(({}) == ({}))", sv.text, v)
                        }
                    }
                } else {
                    let owner = &segs[segs.len() - 2];
                    let variant = &segs[segs.len() - 1];
                    let tag = self.variant_tag(owner, variant)?;
                    format!("({}.tag == {tag}u)", sv.text)
                }
            }
            PatKind::Constructor { path, args, fields, has_rest } => {
                let mut conds = Vec::new();
                if path.len() >= 2 {
                    let owner = &path[path.len() - 2];
                    let variant = &path[path.len() - 1];
                    let tag = self.variant_tag(owner, variant)?;
                    conds.push(format!("({sv}.tag == {tag}u)", sv = sv.text));
                } else {
                    match (path.first().map(|s| s.as_str()), &sv.ty) {
                        (Some("Some"), Ty::Option { inner }) => conds.push(if Cg::niche(inner) {
                            format!("({} != 0)", sv.text)
                        } else {
                            format!("({}.some != 0u)", sv.text)
                        }),
                        (Some("Ok"), Ty::Result { .. }) => conds.push(format!("({}.is_ok != 0u)", sv.text)),
                        (Some("Err"), Ty::Result { .. }) => conds.push(format!("({}.is_ok == 0u)", sv.text)),
                        (Some(n), t) => {
                            return Err(CgError::new(
                                "§3.12",
                                format!("codegen: 构造器模式 `{n}` 用在了 {}", t.text()),
                            ))
                        }
                        (None, _) => return Err(CgError::new("§3.12", "codegen: 空构造器路径")),
                    }
                }
                if !fields.is_empty() && *has_rest {
                    return Err(CgError::new(
                        "§3.12",
                        "codegen: `Foo { a, .. }` 的 `..` 与具名字段子模式混用未实现",
                    ));
                }
                for (i, sub) in args.iter().enumerate() {
                    let subv = self.payload_subvalue(sv, path, i)?;
                    conds.push(self.pat_cond(sub, &subv)?);
                }
                for (fname, sub) in fields {
                    let slot = self.named_payload_slot(sv, path, fname)?;
                    conds.push(self.pat_cond(sub, &slot)?);
                }
                conds.join(" && ")
            }
            PatKind::Or(alts) => {
                let mut parts = Vec::new();
                for a in alts {
                    parts.push(format!("({})", self.pat_cond(a, sv)?));
                }
                parts.join(" || ")
            }
            PatKind::Range { lo, hi, inclusive } => {
                let l = self.pat_cond(lo, sv)?;
                let r = self.pat_cond(hi, sv)?;
                if *inclusive {
                    format!("(({l}) && ({r}))")
                } else {
                    format!("(({l}) && !({r}))")
                }
            }
            PatKind::Tuple(items) => {
                let mut parts = Vec::new();
                for (i, sub) in items.iter().enumerate() {
                    let (fty, text) = self.tuple_subvalue(sv, i)?;
                    parts.push(self.pat_cond(sub, &Val::of(fty, text))?);
                }
                parts.join(" && ")
            }
            PatKind::Invalid | PatKind::DotDot | PatKind::Slice(..) | PatKind::Exclude(_) => {
                return Err(CgError::new(
                    "§3.12",
                    "codegen: 切片模式、`..`、`!(p)` 没有编译期判定发射，未实现",
                ))
            }
        })
    }

    fn variant_tag(&mut self, owner: &str, variant: &str) -> Result<u64, CgError> {
        let (_, idx) = self.adt_get(owner)?;
        match &self.hir.items[idx] {
            HirItem::Enum(en) => en
                .variant(variant)
                .map(|v| v.discriminant)
                .ok_or_else(|| CgError::new("§3.12", format!("codegen: 枚举 `{owner}` 没有变体 `{variant}`"))),
            _ => Err(CgError::new(
                "§3.12",
                format!("codegen: 模式的所有者 `{owner}` 不是枚举"),
            )),
        }
    }

    /// Declare the bindings a pattern introduces, inside the arm's own scope.
    fn pat_bind(&mut self, pat: &Pat, sv: &Val) -> Result<(), CgError> {
        match &pat.kind {
            PatKind::Binding { name, by_ref, mutable, sub } => {
                let _ = mutable;
                let ty = if *by_ref {
                    Ty::Ref { inner: Box::new(sv.ty.clone()), mutable: false, lifetime: None }
                } else {
                    sv.ty.clone()
                };
                let cvar = self.fresh(name);
                let mut deps = BTreeSet::new();
                let ct = self.c_ty(&ty, false, &mut deps)?;
                let init = if *by_ref { format!("(&{})", sv.text) } else { sv.text.clone() };
                self.b(&format!("{} = {};", declarator(&ct, &cvar), init));
                if let Some(top) = self.scopes.last_mut() {
                    top.push(Binding { infy: name.clone(), cvar, ty, cleanup: false, pop: None });
                }
                if let Some(s) = sub {
                    self.pat_bind(s, sv)?;
                }
                Ok(())
            }
            PatKind::Constructor { path, args, fields, .. } => {
                for (i, sub) in args.iter().enumerate() {
                    let subv = self.payload_subvalue(sv, path, i)?;
                    self.pat_bind(sub, &subv)?;
                }
                for (fname, sub) in fields {
                    let slot = self.named_payload_slot(sv, path, fname)?;
                    self.pat_bind(sub, &slot)?;
                }
                Ok(())
            }
            PatKind::Or(alts) => {
                // §3.12 requires every alternative to bind the same set, so the first one names them.
                if let Some(a) = alts.first() {
                    self.pat_bind(a, sv)?;
                }
                Ok(())
            }
            PatKind::Tuple(items) => {
                for (i, sub) in items.iter().enumerate() {
                    let (fty, text) = self.tuple_subvalue(sv, i)?;
                    self.pat_bind(sub, &Val::of(fty, text))?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// One payload slot of a constructor pattern. Tuple variants are records with fields `a0`…
    /// (D-24); `Option`/`Result` slots are `.val`/`.ok`/`.err`.
    fn payload_subvalue(&mut self, sv: &Val, path: &[String], idx: usize) -> Result<Val, CgError> {
        if path.len() < 2 {
            return match (path.first().map(|s| s.as_str()), &sv.ty) {
                (Some("Some"), Ty::Option { inner }) => Ok(Val::of(
                    (**inner).clone(),
                    if Cg::niche(inner) { sv.text.clone() } else { format!("{}.val", sv.text) },
                )),
                (Some("Ok"), Ty::Result { ok, .. }) => Ok(Val::of((**ok).clone(), format!("{}.ok", sv.text))),
                (Some("Err"), Ty::Result { err, .. }) => Ok(Val::of((**err).clone(), format!("{}.err", sv.text))),
                _ => Err(CgError::new("§3.12", "codegen: 构造器子模式无法定位载荷槽")),
            };
        }
        let ename = &path[path.len() - 2];
        let variant = &path[path.len() - 1];
        let (_, hi) = self.adt_get(ename)?;
        let en = match &self.hir.items[hi] {
            HirItem::Enum(e) => e.clone(),
            _ => return Err(CgError::new("§3.12", "codegen: 构造器模式的所有者不是枚举")),
        };
        let v = en.variant(variant).ok_or_else(|| {
            CgError::new("§3.12", format!("codegen: 枚举 `{ename}` 没有变体 `{variant}`"))
        })?;
        let f = v.payload.get(idx).ok_or_else(|| {
            CgError::new(
                "§3.12",
                format!("codegen: 变体 `{variant}` 只有 {} 个载荷槽", v.payload.len()),
            )
        })?;
        Ok(Val::of(f.ty.clone(), format!("{}.v.{}.a{idx}", sv.text, c_ident(variant))))
    }

    fn named_payload_slot(&mut self, sv: &Val, path: &[String], field: &str) -> Result<Val, CgError> {
        if path.len() < 2 {
            return Err(CgError::new(
                "§3.12",
                "codegen: `Option`/`Result` 的具名字段子模式没有对应字段",
            ));
        }
        let ename = &path[path.len() - 2];
        let variant = &path[path.len() - 1];
        let (_, hi) = self.adt_get(ename)?;
        let en = match &self.hir.items[hi] {
            HirItem::Enum(e) => e.clone(),
            _ => return Err(CgError::new("§3.12", "codegen: 模式的所有者不是枚举")),
        };
        let v = en.variant(variant).ok_or_else(|| {
            CgError::new("§3.12", format!("codegen: 枚举 `{ename}` 没有变体 `{variant}`"))
        })?;
        // `sema` flattens struct-variant payloads into declaration order (D-24), so a named field
        // is addressed by its position among them.
        let name = self.prog.enums.iter().find(|e| e.name == *ename).and_then(|e| {
            e.variants.iter().find(|(vn, _, _)| vn == variant).map(|(_, tys, _)| tys.clone())
        });
        let _ = name;
        let pos = v
            .payload
            .iter()
            .position(|p| p.name == field)
            .or_else(|| if v.payload.iter().any(|p| p.name.starts_with('a')) { field.parse::<usize>().ok() } else { None })
            .ok_or_else(|| {
                CgError::new(
                    "§3.12",
                    format!("codegen: 变体 `{variant}` 没有可定位的字段 `{field}`（载荷字段按声明序扁平化为 a0…）"),
                )
            })?;
        Ok(Val::of(v.payload[pos].ty.clone(), format!("{}.v.{}.a{pos}", sv.text, c_ident(variant))))
    }

    fn tuple_subvalue(&mut self, sv: &Val, idx: usize) -> Result<(Ty, String), CgError> {
        let tys = match &sv.ty {
            Ty::Tuple(v) => v.clone(),
            other => return Err(CgError::new("§3.11", format!("codegen: 元组模式用在了 {}", other.text()))),
        };
        let t = tys.get(idx).cloned().ok_or_else(|| {
            CgError::new("§3.11", format!("codegen: 元组没有第 {idx} 个元素（长度 {}）", tys.len()))
        })?;
        Ok((t, format!("{}.a{idx}", sv.text)))
    }

    /// Emit a block whose tail value must land in `out` — the value-producing form of `if`,
    /// `match` and plain blocks.
    fn block_into(&mut self, b: &Block, out: Option<&(String, Ty)>) -> Result<(), CgError> {
        for s in &b.stmts {
            self.stmt(s)?;
        }
        match (&b.tail, out) {
            (Some(e), Some((name, t))) => {
                let v = self.expr(e, Some(t))?;
                self.b(&format!("{name} = {};", v.text));
            }
            (Some(e), None) => {
                let v = self.expr(e, None)?;
                self.b(&format!("(void){};", v.text));
            }
            (None, Some((name, _))) => {
                self.b(&format!("{name} = {{0}};"));
            }
            (None, None) => {}
        }
        Ok(())
    }

    fn c_ty_isolated(&mut self, ty: &Ty) -> Result<String, CgError> {
        let mut deps = BTreeSet::new();
        self.c_ty(ty, false, &mut deps)
    }
}

// --------------------------------------------------------------------------- free helpers

fn base_name_of(ty: &Ty) -> String {
    match ty {
        Ty::Named(n) | Ty::Adt(n) | Ty::Generic(n) | Ty::Dyn(n) => n.clone(),
        Ty::Ref { inner, .. } | Ty::Ptr { inner, .. } | Ty::Boxed { inner } => base_name_of(inner),
        other => other.text(),
    }
}

fn item_kind_name(it: &HirItem) -> &'static str {
    match it {
        HirItem::Struct(_) => "struct",
        HirItem::Enum(_) => "enum",
        HirItem::Fn(_) => "fn",
        HirItem::Const(_) => "const",
        HirItem::Chain(_) => "chain",
    }
}

fn punct_text(p: Punct) -> &'static str {
    match p {
        Punct::Plus => "+",
        Punct::Minus => "-",
        Punct::Star => "*",
        Punct::Slash => "/",
        Punct::Percent => "%",
        Punct::Hat => "^",
        Punct::Amp => "&",
        Punct::Pipe => "|",
        Punct::Shl => "<<",
        Punct::Shr => ">>",
        Punct::Eq => "=",
        Punct::EqEq => "==",
        Punct::BangEq => "!=",
        Punct::Lt => "<",
        Punct::LtEq => "<=",
        Punct::Gt => ">",
        Punct::GtEq => ">=",
        Punct::AmpAmp => "&&",
        Punct::PipePipe => "||",
        Punct::Bang => "!",
        Punct::PlusEq => "+=",
        Punct::MinusEq => "-=",
        Punct::StarEq => "*=",
        Punct::SlashEq => "/=",
        Punct::PercentEq => "%=",
        Punct::AmpEq => "&=",
        Punct::HatEq => "^=",
        Punct::PipeEq => "|=",
        Punct::ShlEq => "<<=",
        Punct::ShrEq => ">>=",
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
        Punct::FatArrow => "=>",
        Punct::Arrow => "->",
        Punct::Question => "?",
        Punct::At => "@",
        Punct::Hash => "#",
        Punct::Dollar => "$",
        Punct::Tilde => "~",
    }
}

fn expr_kind_name(e: &ExprKind) -> &'static str {
    match e {
        ExprKind::Lit(_) => "字面量",
        ExprKind::Ident(_) => "标识符",
        ExprKind::Path(_) => "路径",
        ExprKind::Call { .. } => "调用",
        ExprKind::Method { .. } => "方法调用",
        ExprKind::Field { .. } => "字段访问",
        other => match other {
            ExprKind::Unary { .. } => "一元运算",
            ExprKind::Binary { .. } => "二元运算",
            _ => "表达式",
        },
    }
}

// __TESTS__

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{hir, lexer, parser, sema};
    use crate::diag::Handler;
    use crate::span::Sources;

    /// Stages [1]-[4] over a one-package module, then [7] and [8]. Returns the generated text.
    /// Any front-end `E-` fails the test first: a codegen assertion made against a program the
    /// checker already rejected would prove nothing.
    fn emit_of(src: &str) -> String {
        match emit_try(src) {
            Ok(t) => t,
            Err(e) => panic!("codegen refused: {e:?}\n--- source ---\n{src}"),
        }
    }

    fn emit_try(src: &str) -> Result<String, Vec<String>> {
        emit_with(src, Config::default())
    }

    /// §5.9: the same pipeline with `infy test`'s configuration, so the harness driver is emitted.
    fn harness_try(src: &str, bench: bool) -> Result<String, Vec<String>> {
        emit_with(src, Config { test_harness: true, bench, ..Config::default() })
    }

    fn harness_of(src: &str) -> String {
        match harness_try(src, false) {
            Ok(t) => t,
            Err(e) => panic!("test harness refused: {e:?}\n--- source ---\n{src}"),
        }
    }

    fn emit_with(src: &str, cfg: Config) -> Result<String, Vec<String>> {
        let mut sources = Sources::default();
        let f = sources.add("p/mod.infy".to_string(), format!("package p\n\n{src}\n"));
        let mut h = Handler::default();
        let lexed = lexer::lex_file(&sources, f, &mut h);
        let pf = parser::parse(&sources, f, &lexed, &mut h);
        let files = vec![(f, "p".to_string(), pf)];
        let prog = sema::check_files(&sources, &mut h, "t/p", &files, false, vec![], vec![]);
        let blocked: Vec<String> = h
            .diags
            .iter()
            .filter(|d| d.code.starts_with('E'))
            .map(|d| format!("{} · {}", d.code, d.title))
            .collect();
        if !blocked.is_empty() {
            return Err(blocked);
        }
        let hc = hir::lower(&prog, &hir::HirConfig::default());
        if !hc.errors.is_empty() {
            return Err(hc.errors.iter().map(|e| format!("hir: {}", e.message())).collect());
        }
        emit(&prog, &hc, &files, &sources, &cfg).map_err(|es| es.iter().map(|e| e.render()).collect())
    }

    /// The whole §14 tour through the real CLI pipeline (stages [1]-[5]), so the golden numbers are
    /// the same ones `docs/internals/runtime-abi.md` and the `gcc` probes measured.
    fn tour_text() -> String {
        let mut handler = Handler::default();
        let mut l = crate::cli::load(std::path::Path::new("examples/tour"), &mut handler)
            .expect("examples/tour must load");
        let prog = crate::cli::analyse(&mut l);
        assert_eq!(handler.exit_code(), 0, "tour must check clean before codegen");
        let files: Vec<(FileId, String, ParsedFile)> = l
            .files
            .iter()
            .map(|(id, pf)| (*id, l.file_pkg.get(id).cloned().unwrap_or_default(), pf.clone()))
            .collect();
        let hc = hir::lower(
            &prog,
            &hir::HirConfig { target: Target::REFERENCE, entry_chain: Some("Pipeline".to_string()) },
        );
        assert!(hc.errors.is_empty(), "hir must lay the tour out: {:?}", hc.errors.iter().map(|e| e.message()).collect::<Vec<_>>());
        let cfg = Config {
            target_triple: "x86_64-pc-windows-gnu".to_string(),
            target: Target::REFERENCE,
            module: l.manifest.module_path.clone(),
            ..Config::default()
        };
        match emit(&prog, &hc, &files, &l.sources, &cfg) {
            Ok(t) => t,
            Err(es) => panic!("tour codegen must succeed: {:?}", es.iter().map(|e| e.render()).collect::<Vec<_>>()),
        }
    }

    // ------------------------------------------------------------ §4.8 layout is asserted, not assumed

    /// §4.8 + ABI §1: every record the back end emits carries a compile-time check against stage
    /// [7]'s numbers, so codegen and the layout engine cannot drift apart silently. The four
    /// numbers here are the ones `gcc` measured in `docs/RUNLOG.md` cron-07.
    #[test]
    fn generated_c_asserts_the_measured_tour_layouts() {
        let t = tour_text();
        for probe in [
            "sizeof(infy_github_com_acme_ci_core_build_Phase) == 4u",
            "sizeof(infy_github_com_acme_ci_core_build_FailWhy) == 8u",
            "sizeof(infy_github_com_acme_ci_core_build_Fail) == 12u",
            "sizeof(infy_github_com_acme_ci_core_build_Job) == 12u",
            "sizeof(InfyResult_u32__Fail_) == 20u",
            "offsetof(struct infy_s_infy_github_com_acme_ci_core_build_Job, phase) == 4u",
            "sizeof(infy_github_com_acme_ci_cmd_runner_Pipeline_frame) == 4u",
            "offsetof(struct infy_s_infy_github_com_acme_ci_cmd_runner_Pipeline_frame, node_idx) == 0u",
        ] {
            assert!(t.contains(probe), "missing layout assertion: {probe}");
        }
    }

    /// §8.4: the node table is `chain_id, node_idx, file:line:col, linkage_count`, and only a
    /// top-level `expr_stmt` opens a node. `Pipeline` has six nodes on lines 7/8/9/10/16/20 — the
    /// §9.5-canonical layout of `examples/tour` (偏差 D-17), the same table stage [7] was pinned
    /// to in C1, now produced by stage [8].
    #[test]
    fn tour_chain_node_table_rows_are_the_c1_golden_lines() {
        let t = tour_text();
        let locs = t
            .lines()
            .find(|l| l.contains("Pipeline_locs[6]"))
            .expect("six-node table");
        for line in ["7u", "8u", "9u", "10u", "16u", "20u"] {
            let probe = format!("{{ \"cmd/runner/main.infy\", {line}, 5u");
            assert!(locs.contains(&probe), "node row missing: {probe} in {locs}");
        }
        assert!(locs.contains(" 1u }, { \"cmd/runner/main.infy\", 16u"), "nodes 3 and 4 each carry one linkage block: {locs}");
        assert!(t.contains("Pipeline_desc = { \"Pipeline\", 6u, 0u, 4u, 4u,"), "InfyChainDesc header row");
    }

    /// §8.2/§4.9 UB-013: `goto` is resolved at compile time. `done` is node 5, `begin` is node 0,
    /// and `goto(+1)` from node 3 is node 4 — all constants in the emitted C.
    #[test]
    fn goto_targets_are_resolved_at_compile_time() {
        let t = tour_text();
        assert!(t.contains("infy_goto_checked(0u, 5u,"), "goto(\"done\")");
        assert!(t.contains("infy_goto_checked(0u, 4u,"), "goto(+1) from node 3");
        assert!(t.contains("infy_goto_checked(0u, 0u,"), "goto(\"begin\")");
        assert!(!t.contains("goto(+1)"), "no source text may survive into the C");
    }

    /// §8.2 `goto` 语义（**移动**焦点）: the first `goto` of a node ends that node, so a later
    /// `goto` in the same linkage block cannot overwrite the pending target. `if (all_green()) {
    /// goto("done"); } goto(+1);` is the §14 tour's own shape, and with fall-through the `+1`
    /// always won: `Pipeline` re-ran node 0 forever and `infy run` never returned (first execution
    /// of a built artifact, 2026-10-13 — see docs/RUNLOG.md cron-10).
    #[test]
    fn the_first_goto_in_a_node_ends_it() {
        let t = tour_text();
        let body = t
            .split("Pipeline_node_3(void* frame_) {")
            .nth(1)
            .and_then(|s| s.split("Pipeline_node_4(").next())
            .expect("node 3 body");
        let goto_done = body.find("infy_goto_checked(0u, 5u,").expect("goto(\"done\") is emitted");
        let transferred = body.find("return;").expect("the goto must transfer control");
        let goto_rel = body.find("infy_goto_checked(0u, 4u,").expect("goto(+1) is still emitted");
        assert!(
            goto_done < transferred && transferred < goto_rel,
            "node 3 has to leave at the first goto, got: {body}"
        );
    }

    // ------------------------------------------------------------ §4.9 / §7.2 checks are never off

    /// §4.9 + §7.2: an integer `+` is a call into the runtime, never a bare C operator (which would
    /// wrap silently at `-O2`). Paired assertions: the call is there *and* the bare form is not.
    #[test]
    fn integer_addition_is_a_checked_runtime_call() {
        let t = emit_of("fn f(a: u32, b: u32) -> u32 { a + b }");
        assert!(t.contains("infy_add_u32(a_1, b_2, infy_current_node_loc())"), "{t}");
        assert!(!t.contains("(a_1) + (b_2)"), "a bare C + would wrap (§4.4)");
        // §4.12: floats are plain IEEE for add/sub/mul/div; only `rem` needs a helper.
        let t = emit_of("fn g(a: f64, b: f64) -> f64 { a * b }");
        assert!(t.contains("((a_1) * (b_2))"), "{t}");
    }

    /// §4.13 rule 2/7: an unsuffixed literal takes the type of the other side, so `tries + 1` on a
    /// `u32` field stays a `u32` check instead of widening to `i32`.
    #[test]
    fn unsuffixed_literal_takes_the_context_type() {
        let t = emit_of("struct J { pub tries: u32 }\nfn bump(j: J) -> J { J { tries: j.tries + 1 } }");
        assert!(t.contains("infy_add_u32(j_1.tries, UINT32_C(1)"), "{t}");
    }

    /// §2.8: a literal is written with the `stdint.h` macro for its resolved type, so the C type
    /// cannot drift from the HIR type on a target with a different `int` width.
    #[test]
    fn literals_carry_their_resolved_width() {
        let t = emit_of("fn f() -> u64 { 4294967296 }\nfn g() -> i32 { -128i32 }\nfn h() -> bool { true }");
        assert!(t.contains("UINT64_C(4294967296)"), "{t}");
        assert!(t.contains("(-(INT32_C(128)))"), "{t}");
        // §1: `bool` is `uint8_t` with only 0x00/0x01 legal, and `<stdbool.h>` is not in the contract.
        assert!(t.contains("return 1u;"), "{t}");
        assert!(!t.contains("return true;"), "{t}");
    }

    // ------------------------------------------------------------ §4.1 representations

    /// ABI §1: `Option` of a scalar is a record, `Option` of a pointer is the pointer itself (the
    /// §4.1 niche), and `Result<u32, u64>` is 24/8 — the number the ABI doc had wrong before C1.
    #[test]
    fn option_and_result_use_the_abi_section_one_shapes() {
        let t = emit_of("fn f(a: Option<u32>, b: Option<&u32>, c: Result<u32, u64>) { }");
        assert!(t.contains("InfyOption_u32_ { uint32_t val; uint8_t some; }"), "{t}");
        assert!(t.contains("sizeof(InfyOption_u32_) == 8u"), "{t}");
        assert!(t.contains("const uint32_t* b_2"), "niche Option<&u32> is the pointer itself: {t}");
        assert!(t.contains("InfyResult_u32__u64_ { uint32_t ok; uint64_t err; uint8_t is_ok; }"), "{t}");
        assert!(t.contains("sizeof(InfyResult_u32__u64_) == 24u"), "{t}");
    }

    /// §4.1: `str` is a `(ptr, len)` pair that is **not** NUL-terminated, so a literal goes through
    /// `infy_str_of`, and §4.4 compares two `str` by NFC content, never by pointer.
    #[test]
    fn str_literals_and_comparison_use_the_runtime_helpers() {
        let t = emit_of("fn f(j: u32) -> str { match (j) { 0 => \"fetch\", _ => \"late\" } }");
        assert!(t.contains("infy_str_of(\"fetch\")"), "{t}");
        let t = emit_of("fn eq(a: str, b: str) -> bool { a == b }");
        assert!(t.contains("infy_str_eq_nfc(a_1, b_2)"), "{t}");
        assert!(!t.contains("(a_1) == (b_2)"), "pointer equality on `str` would be a false negative: {t}");
    }

    /// §4.1 / D-24: a fieldless enum is exactly its tag (the union is elided), and a tuple variant
    /// addresses its payload by position (`a0`), which is what `hir::HirVariant::payload` promises.
    #[test]
    fn enums_elide_the_union_when_no_variant_carries_a_payload() {
        let t = emit_of("enum P { A, B }\nenum Q { X, Y(i32) }\nfn f() -> P { P::B }\nfn g(q: Q) -> u32 { match (q) { Q::X => 0, Q::Y(n) => 1 } }");
        assert!(t.contains("struct infy_s_infy_p_P { uint32_t tag;}"), "fieldless enum is exactly its tag: {t}");
        assert!(!t.contains("union infy_u_infy_p_P"), "no payload, no union: {t}");
        assert!(t.contains("union infy_u_infy_p_Q"), "{t}");
        assert!(t.contains(".tag = 1u }"), "P::B is discriminant 1");
        assert!(t.contains("q_1.v.Y.a0"), "tuple payload is addressed as a0 (D-24): {t}");
    }

    // ------------------------------------------------------------ §4.11 `?`

    /// §4.11: `expr?` is exactly `match expr { Ok(v) => v, Err(e) => return Err(e.into()) }`. The
    /// generated C must test the tag, propagate the *same* error value, and yield `.ok`.
    #[test]
    fn try_operator_propagates_the_error_and_yields_the_ok_payload() {
        let t = emit_of(
            "struct E { pub c: u32 }\nfn inner() -> Result<u32, E> { Ok(1) }\nfn outer() -> Result<u32, E> { let v = inner()?; Ok(v) }",
        );
        assert!(t.contains("if (!try_"), "{t}");
        assert!(t.contains(".is_ok = 0u, .err = try_"), "{t}");
        assert!(t.contains("return err_"), "{t}");
        assert!(t.contains("= try_"), "the value of `?` is the ok payload: {t}");
    }

    /// §4.11 last bullet: `?` needs a return boundary. In a chain's top level there is none, and the
    /// back end must refuse rather than emit a `return` outside a function.
    #[test]
    fn try_outside_a_function_is_refused() {
        let e = emit_try("fn q() -> u32 { 1 }\nchain C {\n    let x = q()?;\n}").unwrap_err();
        assert!(
            e.iter().any(|s| s.contains("E-0173") || s.contains("§4.11") || s.contains("阶段 [4]")),
            "{e:?}"
        );
    }

    // ------------------------------------------------------------ §3.12 match

    /// §3.12: arms compile to a tag test chain with the arm body binding its payload; the fallthrough
    /// branch cannot be empty, because a result temporary would stay uninitialised.
    #[test]
    fn match_arms_become_tag_tests_with_a_non_returning_fallthrough() {
        let t = emit_of("enum P { A, B }\nfn f(p: P) -> u32 { match (p) { P::A => 1, P::B => 2 } }");
        assert!(t.contains("p_1.tag == 0u"), "{t}");
        assert!(t.contains("else if ((p_1.tag == 1u))"), "{t}");
        assert!(t.contains("infy_panic(R_BAD_BOOL"), "fallthrough must not leave the result unset: {t}");
    }

    /// §3.12: an or-pattern is a disjunction of its alternatives' conditions.
    #[test]
    fn or_patterns_disjoin_the_alternatives() {
        let t = emit_of("enum P { A, B, C }\nfn f(p: P) -> u32 { match (p) { P::A | P::B => 1, _ => 2 } }");
        assert!(t.contains("||"), "or-pattern must disjoin: {t}");
        assert!(t.contains("((p_1.tag == 0u)) || ((p_1.tag == 1u))"), "{t}");
    }

    // ------------------------------------------------------------ §4.7.4 drop

    /// §4.7.4 + ABI §3: the drop order is *emitted*. An owned `String` pushes a cleanup entry where
    /// it comes into scope and pops it at scope end; the runtime drains the same deque on panic.
    #[test]
    fn owned_values_register_a_cleanup_entry_at_their_scope() {
        let t = emit_of("fn f(s: String, v: Vec<u8>) { }");
        assert!(t.contains("infy_cleanup_push(infy_drop_String, &s_1, 0);"), "{t}");
        assert!(t.contains("infy_cleanup_push(infy_drop_Vec_u8_, &v_2, 0);"), "{t}");
        assert!(t.contains("static void infy_drop_String(void* obj) { InfyString* p = (InfyString*)obj; infy_string_drop((InfyString*)p); }"), "{t}");
        assert!(t.contains("infy_vec_drop((InfyVec*)p, 1u, (InfyDropFn)0);"), "element size travels with the container: {t}");
        assert!(t.contains("infy_cleanup_pop_run();"), "{t}");
    }

    /// §4.7.1 + §4.7.4: the invariant that makes a move safe. Every cleanup entry a function pushes
    /// must be answered by exactly one `pop_run` / `pop_noop` / `cancel` — a second answer is a
    /// double free, a missing one leaks the buffer. Counting both sides is what a textual
    /// `contains` assertion cannot do.
    fn cleanup_ledger(text: &str) -> (usize, usize) {
        let pushed = text.matches("infy_cleanup_push(").count();
        let answered = text.matches("infy_cleanup_pop_run();").count()
            + text.matches("infy_cleanup_pop_noop();").count()
            + text.matches("infy_cleanup_cancel(").count();
        (pushed, answered)
    }

    /// §4.7.1 `let d: String = s;`: the entry registered at the **source** address leaves the deque
    /// without running, and the destination answers the buffer at scope end (F12).
    #[test]
    fn a_let_move_cancels_the_source_entry_and_keeps_the_destination_pop() {
        let t = emit_of("fn f(s: String) { let d: String = s; }");
        assert!(t.contains("infy_cleanup_cancel(&s_"), "{t}");
        assert!(t.contains("infy_cleanup_push(infy_drop_String, &d_"), "{t}");
        assert!(!t.contains("infy_cleanup_cancel(&d_"), "the destination owns the buffer: {t}");
        assert_eq!(t.matches("infy_cleanup_pop_run();").count(), 1, "only the destination pops: {t}");
        assert_eq!(cleanup_ledger(&t), (2, 2), "{t}");
    }

    /// §4.7.1 by-value argument: the callee registers its own entry for the same buffer, so the
    /// caller's entry must be the one cancelled.
    #[test]
    fn an_owned_byvalue_argument_cancels_the_callers_entry() {
        let t = emit_of("fn take(s: String) { }\nfn f(s: String) { take(s); }");
        assert!(t.contains("infy_cleanup_cancel(&s_"), "{t}");
        assert_eq!(t.matches("infy_cleanup_pop_run();").count(), 1, "the callee pops its own entry: {t}");
        assert_eq!(cleanup_ledger(&t), (2, 2), "{t}");
    }

    /// §4.7.4: a value that leaves through `return` is dropped by its new owner, so nothing pops it
    /// here — the entry is cancelled and the function ends with zero `pop_run`.
    #[test]
    fn a_returned_owned_binding_cancels_its_entry() {
        let t = emit_of("fn f(s: String) -> String { s }");
        assert!(t.contains("infy_cleanup_cancel(&s_"), "{t}");
        assert_eq!(t.matches("infy_cleanup_pop_run();").count(), 0, "the caller owns it now: {t}");
        assert_eq!(cleanup_ledger(&t), (1, 1), "{t}");
    }

    /// §4.7.1: `return P { s }` moves the binding too — the cancellation keys on the binding being
    /// read **by value** in the emitted text, not on the tail expression's shape.
    #[test]
    fn a_return_that_carries_the_binding_in_a_literal_cancels_its_entry() {
        let t = emit_of("struct P { pub s: String }\nfn f(s: String) -> P { P { s } }");
        assert!(t.contains("infy_cleanup_cancel(&s_"), "{t}");
        assert!(t.contains(".s = s_"), "{t}");
        assert_eq!(cleanup_ledger(&t), (1, 1), "{t}");
    }

    /// §4.7.1 + §3.9: `&s` is a borrow, ownership stays with the callee's caller, so the entry must
    /// still be popped. Cancelling on a borrow would leak the buffer — the failure mode is the
    /// opposite of the one `cancel_moved_return` exists to prevent.
    #[test]
    fn a_borrow_leaves_the_cleanup_entry_in_place() {
        let t = emit_of("fn take(r: &String) { }\nfn f(s: String) -> u32 { take(&s); 1u32 }");
        assert!(!t.contains("infy_cleanup_cancel("), "a borrow is not a move: {t}");
        assert!(t.contains("infy_cleanup_pop_run();"), "{t}");
        assert_eq!(cleanup_ledger(&t), (1, 1), "{t}");
    }

    /// Negative control: a value that never registered an entry has nothing to cancel, so the move
    /// paths must not emit a `cancel` that matches nothing.
    #[test]
    fn moving_a_value_that_never_registered_an_entry_emits_no_cancel() {
        let t = emit_of("fn f(n: u32) -> u32 { n }");
        assert!(!t.contains("infy_cleanup_cancel("), "{t}");
        assert_eq!(cleanup_ledger(&t), (0, 0), "{t}");
    }

    /// §4.7.4 + ABI §5: a chain frame is released by the scheduler (`infy_free(f->frame)`), so a
    /// drop-bearing frame local is refused where the frame is laid out. That refusal — not a
    /// move-specific one — is what keeps owned values out of frames (measured while landing F12).
    #[test]
    fn a_chain_frame_local_with_a_destructor_is_refused_by_name() {
        let e = emit_try("chain C {\n    let a: String = \"abc\";\n}").unwrap_err();
        assert!(
            e.iter().any(|x| x.contains("帧局部") && x.contains("析构器")),
            "the frame-local refusal must be named, got {e:?}"
        );
    }

    // ------------------------------------------------------------ refusals must be named

    /// §7.1: an uncovered construct is an error that names the clause, never a plausible substitute
    /// and never silence.
    #[test]
    fn uncovered_constructs_are_refused_by_name() {
        // §3.13 `defer` parses and type-checks; only stage [8] has no emission for it.
        let e = emit_try("fn f() { let x: u32 = 1; defer { } }").unwrap_err();
        let joined = e.join("\n");
        assert!(joined.contains("§3.13") && joined.contains("defer"), "refusal must name the clause: {joined}");
        // §3.5 `for` needs the §10.10 iterator protocol.
        let j2 = emit_try("fn f(v: Vec<u8>) { for x in v { } }").unwrap_err().join("\n");
        assert!(
            j2.contains("§10.10") || j2.contains("§3.5") || j2.contains("E-"),
            "`for` must be refused or rejected upstream: {j2}"
        );
    }

    /// §7.6.3 项 2/3: an A-track call is emitted as a call **through the wrapper symbol**, and the
    /// TU declares that wrapper `extern`. The generated C never mentions the bare C name — that
    /// indirection is exactly what §7.6.3 item 2 buys (two same-named declarations with different
    /// signatures must not collide at link time).
    #[test]
    fn an_a_track_call_is_emitted_through_its_wrapper_symbol() {
        let t = emit_of("#[link(name = \"dec\", static = true)]\nextern \"C\" {\n    fn decode(p: *const u8, n: u32) -> i32;\n}\nfn f(x: *const u8) -> i32 { unsafe { decode(x, 1u32) } }");
        let (_, after) = t.split_once("extern ").expect("an extern wrapper declaration");
        let (proto, _) = after.split_once(";\n").expect("prototype ends at the newline");
        assert!(proto.starts_with("int32_t infy_e_decode_"), "{proto}");
        let sym = proto.split('(').next().expect("paren").split_whitespace().last().expect("name").to_string();
        let hash = sym.strip_prefix("infy_e_decode_").unwrap_or_else(|| panic!("unexpected symbol {sym}"));
        assert_eq!(hash.len(), 8, "ABI §2 wants four hash bytes: {hash}");
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()), "{hash}");
        assert!(t.contains(&format!("{sym}(x_1, UINT32_C(1))")), "the call site must use the wrapper:\n{t}");
        assert!(!t.contains(" decode("), "the bare C name must never be referenced:\n{t}");
    }

    /// 块状表达式（`{}` / `unsafe {}`）作尾值时的结果临时量必须落在**外层**作用域：
    /// 先前它被声明在花括号里面，于是 `return bres_1;` 引用了一个已经出视线的变量（gcc：
    /// `'bres_1' undeclared`）。`ExprKind::If` 一直是对的，这一臂把顺序写反了。
    #[test]
    fn a_block_tail_expression_declares_its_result_in_the_function_scope() {
        let t = emit_of("fn g() -> u32 { 7u32 }\nfn f() -> u32 { unsafe { g() } }");
        let body = t.split_once("uint32_t infy_p_f(void) {").expect("f's body").1;
        let body = body.split_once("/* ---- ").map(|(b, _)| b).unwrap_or(body);
        let decl_at = body.find("uint32_t bres_").expect("the result temp must be declared:\n{body}");
        let brace_at = body.find('{').expect("the block opens after it");
        assert!(decl_at < brace_at, "the temp must be outside the braces:\n{body}");
        assert!(body.contains("return bres_"), "{body}");
    }
    /// §7.6.4 行 2 的 `&[u8]` 是规范允许的跨边界类型，本实现只是还没把实参摊成 `ptr, len`。
    /// 合法程序 + 未实现 = 阶段 [8] 以条款号指名拒绝（`docs/spec-deviations.md` D-46(e)），
    /// 而**不是**在阶段 [4] 发 `E-0723` 假装这个程序不合规。
    #[test]
    fn an_unmappable_but_legal_a_track_signature_is_refused_at_build_by_name() {
        let e = emit_try("#[link(name = \"dec\")]\nextern \"C\" {\n    fn dec_read(b: &[u8]) -> u32;\n}\nfn f() { }\n")
            .unwrap_err()
            .join("\n");
        assert!(e.contains("§7.6.4") && e.contains("dec_read"), "{e}");
        assert!(!e.contains("E-0723"), "an unimplemented row must not be reported as a compliance error: {e}");
    }

    /// D-50(c)：`dyn Trait` 出现在**合法**位置（形参）时，阶段 [1]-[5] 是过的，拒绝发生在 [8] 且
    /// 以条款号指名 —— 锁的是"那一臂还活着"。偏移表已定（`docs/internals/runtime-abi.md` §1），
    /// 缺的是发射点；既不许多生成一份近似代码，也不许把未实现报成 `E-`（假合规失败）。
    #[test]
    fn a_dyn_type_is_refused_at_build_by_name() {
        let e = emit_try("trait Shape {\n    fn area(&self) -> f32;\n}\nfn f(x: dyn Shape) { }\n")
            .unwrap_err()
            .join("\n");
        assert!(e.contains("§4.1") && e.contains("vtable"), "拒绝要指名条款与缺的东西：{e}");
        assert!(!e.contains("E-0"), "未实现的发射不许伪装成语义诊断：{e}");
    }

    /// §7.6.1: a `;`-bodied declaration that no `#[link]` block claims has no wrapper, so calling it
    /// is refused by name at stage [8] instead of being emitted as an unresolved reference.
    #[test]
    fn an_extern_declaration_outside_a_link_block_is_refused_by_name() {
        let e = emit_try("extern \"C\" {\n    fn decode(p: *const u8) -> i32;\n}\nfn f(x: *const u8) -> i32 { unsafe { decode(x) } }")
            .unwrap_err()
            .join("\n");
        assert!(e.contains("§7.6.1") && e.contains("decode"), "{e}");
    }

    /// A `()`-returning declaration maps to a `void` wrapper (§7.6.4 gives no row for `()`), so the
    /// call must not be assigned directly — `InfyUnit t = void_call();` is not C. The comma
    /// operator keeps it an expression without inventing a runtime helper.
    #[test]
    fn a_void_a_track_call_keeps_the_call_an_expression() {
        let t = emit_of("#[link(name = \"dec\")]\nextern \"C\" {\n    fn close(h: *mut u8);\n}\nfn f(x: *mut u8) { unsafe { close(x) } }");
        assert!(t.contains("= ((void)infy_e_close_"), "{t}");
        assert!(t.contains("(InfyUnit){0})"), "{t}");
        assert!(t.contains("void infy_e_close_"), "the wrapper itself must be declared void:\n{t}");
    }

    // ------------------------------------------------------------ §5.9 test harness

    /// §5.9/§13.4: under `infy test` the generated `main` is the harness, not the §7.4 entry. The
    /// run set is exactly the `#[test]` functions, in (package, name) order — order is asserted
    /// because the C4 round learned that existence-only assertions pass on a wrong control flow —
    /// and a `Result`-returning test is judged by `.is_ok`, not by the call having happened.
    #[test]
    fn harness_runs_each_test_in_sorted_order_and_judges_result_by_is_ok() {
        let t = harness_of("#[test]\nfn zeta() -> Result<u32, u32> { Ok(1u32) }\n\n#[test]\nfn alpha() { }\n\nfn not_a_test() { }\n");
        let a = t.find("p::alpha").expect("alpha is in the suite");
        let z = t.find("p::zeta").expect("zeta is in the suite");
        assert!(a < z, "§7.3: the report is sorted by name, not by declaration order\n{t}");
        assert!(!t.contains("p::not_a_test"), "an un-annotated fn is not a test");
        assert_eq!(t.matches("infy_t_n += 1u;").count(), 2, "one counted call per test");
        assert!(t.contains("infy_t_r.is_ok"), "a `Result` test must be judged: {t}");
        assert!(t.contains("FAILED") && t.contains("infy_t_bad += 1u;"), "the failing branch counts");
        assert!(t.contains("return infy_t_bad == 0u ? 0 : 1;"), "§13.4: 1 = a test failed");
        assert!(t.contains("fputs(\"test p::alpha ... \", stdout);"), "report on stdout (D-36)");
        assert!(!t.contains("entry (§7.4)"), "the harness replaces the entry driver");
    }

    /// §5.9: the harness only has a definition of "passed" for `()` and `Result<…>`. Anything else
    /// is refused by name instead of guessing (e.g. treating a returned `bool` as a verdict).
    #[test]
    fn harness_refuses_tests_it_cannot_call_or_judge() {
        let e = harness_try("#[test]\nfn needs_arg(x: u32) { }\n", false).unwrap_err().join("\n");
        assert!(e.contains("§5.9") && e.contains("needs_arg"), "{e}");
        assert!(e.contains("§4.6"), "the refusal must say why there is no injection: {e}");
        let e2 = harness_try("#[test]\nfn returns_value() -> u32 { 1u32 }\n", false).unwrap_err().join("\n");
        assert!(e2.contains("§5.9") && e2.contains("returns_value"), "{e2}");
    }

    /// §13.4's `--bench` is the only way a `#[bench]` function enters the run set; and §14.7's
    /// benchmark signature (`fn(b: &mut test::Bench)`) has no `std/test` in §10's std table, so a
    /// bench with the shape §5.9 *could* run is refused naming that gap (BACKLOG F21) rather than
    /// silently timing nothing.
    #[test]
    fn bench_functions_enter_the_suite_only_under_bench() {
        let t = harness_of("#[bench]\nfn bench_zero() { }\n");
        assert!(!t.contains("p::bench_zero"), "no `--bench`, no bench in the suite");
        assert_eq!(t.matches("infy_t_n += 1u;").count(), 0, "the suite is empty, not faked");
        assert!(t.contains("infy_t_n - infy_t_bad"), "still reports 0 total (§13.4: success)");
        let e = harness_try("#[bench]\nfn bench_one(n: u32) { }\n", true).unwrap_err().join("\n");
        assert!(e.contains("§5.9") && e.contains("bench_one"), "{e}");
        assert!(e.contains("std.test"), "the refusal names the missing §10 entry (F21): {e}");
    }

    // ------------------------------------------------------------ §7.3 byte stability

    /// §7.3: the emitted text is a pure function of the inputs — same bytes for the same module, in
    /// the same order, so a build-id over it is meaningful.
    #[test]
    fn generated_text_is_byte_stable() {
        let a = tour_text();
        let b = tour_text();
        assert_eq!(a, b, "two runs over examples/tour must agree byte for byte");
        assert!(a.starts_with("/* Generated by `infy build` stage [8]"), "banner first");
        // §2 of the ABI doc: one symbol per item, package path mangled.
        assert!(a.contains("infy_github_com_acme_ci_core_build_describe"), "{a}");
        assert!(!a.contains("::"), "C text must not carry INFY path separators");
    }
}
