//! Pipeline stage [2] (§7.1): hygiene-scoped `macro_rules!` expansion (§6) plus the token-level
//! machinery behind `infy expand` (§6.8).
//!
//! Normative behaviour implemented here:
//! * §6.2 — the closed classifier set `:expr :stmt :block :pat :ident :ty :tt :meta`; matching runs
//!   over token trees; candidates for multi-token classes are validated by re-parsing through the
//!   real parser. Repetitions `$(pat)sep` with `+`/`*`/`?`, nested repetitions, and the `$(,)?`
//!   trailing-comma idiom of the §6.1 `vec!` example are supported.
//! * §6.3 / UB-015 — hygiene by scope rustling: each expansion gets a unique `expansion_id` and
//!   identifiers the macro itself introduces are renamed to `__infy_m<id>_<name>` (a prefix that
//!   §2.3 reserves for the compiler, `E-0007` for user code). Tokens substituted from fragments
//!   keep their call-site identity. A macro-local name that a user could still observe is
//!   reported as `E-0400`.
//! * §6.4 — recursion depth limit 128 → `E-0401` (exactly one diagnostic per runaway expansion).
//! * §6.5 — expansions are re-parsed into AST fragments; an expansion that is not a legal
//!   fragment → `E-0402` pointing at the **expander**, printing the expanded fragment and
//!   recording provenance via `.expansion("macro vec → file:12")` in `expand_chain`.
//! * §6.8 / §7.3 — [`render_expanded`] is byte-stable: `BTreeMap`/sorted iteration only, no
//!   HashMap, no time, no addresses.
//!
//! Re-parsing never re-lexes: the expansion token stream is wrapped in a minimal legal file
//! (`fn __expand_probe() { … }`, `type __expand_probe = …;`, `match … { pat => {} }`) as raw `Tok`
//! values and fed to `parser::parse`; any E-level diagnostic from the throwaway handler means the
//! fragment is illegal.

use std::collections::BTreeMap;

use crate::ast::{
    ArmBody, Block, ElseBranch, Expr, ExprKind, Item, ItemKind, MatchArm, ParsedFile, Pat, Stmt,
    StmtKind,
};
use crate::diag::{Diagnostic, Handler, Phase};
use crate::span::{FileId, Sources, Span};
use crate::token::{Delim, Kind, Kw, Punct, Tok, Tree};

/// §6.4: macro expansion recursion depth limit.
pub const MAX_EXPANSION_DEPTH: usize = 128;

/// §6.8 `--stage all|macro|derive`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Stage {
    #[default]
    All,
    Macro,
    Derive,
}

/// §6.8 `infy expand [mod] [--macro name] [--derive Type] [--stage …]`.
#[derive(Clone, Debug, Default)]
pub struct ExpandOptions {
    /// `--macro name`: only calls of this macro expand; every other call is left untouched.
    pub only_macro: Option<String>,
    pub stage: Stage,
}

/// One expanded call site — the audit trail printed by `infy expand` (§6.8).
#[derive(Clone, Debug)]
pub struct MacroSite {
    pub file: FileId,
    /// Span of the macro call expression as written (provenance anchor).
    pub call_span: Span,
    /// Range replaced in the reprinted source (call, plus trailing `;` in statement position).
    pub replace_span: Span,
    pub macro_name: String,
    pub expansion_id: u32,
    /// Deepest nested expansion reached for this site (§6.4 accounting).
    pub depth: usize,
    /// Byte-stable reprint of the expansion that replaced the call.
    pub text: String,
    /// Macro-introduced identifiers (rustled in `text`), sorted.
    pub introduced: Vec<String>,
}

/// One generated derive impl (§6.7), carried so `--stage all|derive` can reprint it.
#[derive(Clone, Debug)]
pub struct DeriveSite {
    pub file: FileId,
    /// Insertion point: byte offset just past the derived item.
    pub insert_at: u32,
    pub type_name: String,
    pub trait_name: String,
    pub line: u32,
    pub text: String,
}

/// Everything a stage-[2] run reports to `infy expand`.
#[derive(Clone, Debug, Default)]
pub struct ExpansionResult {
    /// Macro sites in deterministic (file, offset) order.
    pub sites: Vec<MacroSite>,
    /// Derive impls in deterministic (file, offset, trait) order.
    pub derives: Vec<DeriveSite>,
    pub expansions: usize,
    pub max_depth: usize,
    /// Sorted names of the `macro_rules!` definitions found in the package.
    pub macro_names: Vec<String>,
}

// ---------------------------------------------------------------- classifiers (§6.2)

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Expr,
    Stmt,
    Block,
    Pat,
    Ident,
    Ty,
    Tt,
    Meta,
}

impl Class {
    fn parse(s: &str) -> Option<Class> {
        Some(match s {
            "expr" => Class::Expr,
            "stmt" => Class::Stmt,
            "block" => Class::Block,
            "pat" => Class::Pat,
            "ident" => Class::Ident,
            "ty" => Class::Ty,
            "tt" => Class::Tt,
            "meta" => Class::Meta,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RepOp {
    /// `?`
    Optional,
    /// `*`
    Star,
    /// `+`
    Plus,
}

/// One compiled matcher element.
#[derive(Clone, Debug)]
enum MItem {
    Lit(Tree),
    Bind { name: String, class: Class },
    Rep { items: Vec<MItem>, sep: Option<Punct>, op: RepOp, bound: Vec<String> },
}

/// A matched fragment value. `Many` carries one element per repetition iteration; nested
/// repetitions nest `Many` inside `Many`, mirroring nested `$( … )` in the expander.
#[derive(Clone, Debug)]
enum Frag {
    One(Vec<Tree>),
    Many(Vec<Frag>),
}

type Scope = BTreeMap<String, Frag>;

/// A compiled arm: matcher, expander, and the macro-local identifiers (§6.3 rustle set).
#[derive(Clone, Debug)]
struct ArmC {
    items: Vec<MItem>,
    expander: Vec<Tree>,
    introduced: Vec<String>,
}

#[derive(Clone, Debug)]
struct DefEntry {
    file: FileId,
    item_idx: usize,
    pos: u32,
}

/// Per-run expansion context. All mutable state that affects output is ordered/deterministic.
struct RunCtx<'a> {
    defs: BTreeMap<String, DefEntry>,
    sources: &'a Sources,
    next_expansion_id: u32,
    /// Set when §6.4 tripped during the current site; later cascades are suppressed.
    depth_tripped: bool,
}

impl RunCtx<'_> {
    fn display(&self, f: FileId) -> String {
        self.sources
            .get(f)
            .map(|x| x.display_path.clone())
            .unwrap_or_else(|| format!("file{}", f))
    }
    fn line(&self, sp: Span) -> u32 {
        self.sources.line_col(sp).0.line
    }
}

// ---------------------------------------------------------------- public entry points

/// Stage [2] (§7.1): expand every visible `macro_rules!` call in the package and plan the
/// `#[derive]` reprinting (the AST-generating half is [`crate::derive::expand_derives`], which the
/// driver runs alongside this function).
pub fn expand_all(
    files: &mut BTreeMap<FileId, ParsedFile>,
    sources: &Sources,
    handler: &mut Handler,
    opts: &ExpandOptions,
) -> ExpansionResult {
    let mut result = ExpansionResult::default();
    if opts.stage != Stage::Derive {
        expand_macros(files, sources, handler, opts, &mut result);
    }
    if opts.stage != Stage::Macro {
        collect_derive_plan(files, sources, &mut result);
    }
    result.derives.sort_by(|a, b| {
        (a.file, a.insert_at, &a.trait_name).cmp(&(b.file, b.insert_at, &b.trait_name))
    });
    result
}

/// Deterministic reprint plan for `#[derive]` impls (no diagnostics; validation and AST
/// generation live in `crate::derive`).
fn collect_derive_plan(
    files: &BTreeMap<FileId, ParsedFile>,
    sources: &Sources,
    result: &mut ExpansionResult,
) {
    for (fid, pf) in files.iter() {
        for item in &pf.items {
            if !item.has_attr("derive") {
                continue;
            }
            let line = sources.line_col(item.span).0.line;
            for tr in crate::derive::derive_list(item) {
                let Some(text) = crate::derive::derive_impl_source(item, &tr) else { continue };
                result.derives.push(DeriveSite {
                    file: *fid,
                    insert_at: item.span.end,
                    type_name: item.name().to_string(),
                    trait_name: tr,
                    line,
                    text,
                });
            }
        }
    }
}

// ---------------------------------------------------------------- macro definition scope

/// §5.5-scoped package visibility: `macro_rules!` definitions keyed by name; the first definition
/// in (FileId, offset) order wins. Same-file uses must sit *after* the definition point (§6.5
/// expansion timing); cross-file uses inside one package are visible, since every file of one
/// `infy check` run belongs to one package (§5.1).
fn collect_defs(files: &BTreeMap<FileId, ParsedFile>) -> BTreeMap<String, DefEntry> {
    let mut defs: BTreeMap<String, DefEntry> = BTreeMap::new();
    for (fid, pf) in files.iter() {
        for (idx, item) in pf.items.iter().enumerate() {
            if let ItemKind::MacroRules { name, .. } = &item.kind {
                defs.entry(name.clone())
                    .or_insert(DefEntry { file: *fid, item_idx: idx, pos: item.span.start });
            }
        }
    }
    defs
}

// ---------------------------------------------------------------- call-site collection

#[derive(Clone, Debug)]
struct Site {
    order: u32,
    item_idx: usize,
    name: String,
    input: Vec<Tree>,
    call_span: Span,
    replace_span: Span,
    is_stmt: bool,
}

enum Outcome {
    Expanded { trees: Vec<Tree>, introduced: Vec<String>, depth: usize, exp_id: u32 },
    /// Filtered by `--macro`, an unknown name, or otherwise left for later stages.
    Skip,
    /// A stage-[2] diagnostic was emitted; do not re-parse/apply.
    Failed,
}

fn expand_macros(
    files: &mut BTreeMap<FileId, ParsedFile>,
    sources: &Sources,
    handler: &mut Handler,
    opts: &ExpandOptions,
    result: &mut ExpansionResult,
) {
    let mut ctx = RunCtx {
        defs: collect_defs(files),
        sources,
        next_expansion_id: 0,
        depth_tripped: false,
    };
    result.macro_names = ctx.defs.keys().cloned().collect();

    // Snapshot the call sites first: expansion rewrites the AST, and rewriting while walking is
    // an unstable iteration order (§7.3). Sites are then applied in source order.
    let mut sites: Vec<Site> = Vec::new();
    let mut counter = 0u32;
    for fid in files.keys().cloned().collect::<Vec<_>>() {
        for (idx, item) in files[&fid].items.iter().enumerate() {
            collect_item_sites(item, idx, &mut sites, &mut counter);
        }
    }
    sites.sort_by_key(|s| s.order);

    for site in sites {
        ctx.depth_tripped = false;
        let outcome = expand_site(&mut ctx, files, handler, opts, &site);
        let (trees, introduced, depth, exp_id) = match outcome {
            Outcome::Expanded { trees, introduced, depth, exp_id } => (trees, introduced, depth, exp_id),
            Outcome::Skip => continue,
            Outcome::Failed => continue,
        };
        let text = render_trees(&trees);
        let toks = flatten_trees(&trees);
        let fid = site.call_span.file;
        let fragment = match parse_expansion(sources, fid, site.call_span, &toks, site.is_stmt) {
            Some(f) => f,
            None => {
                emit_e0402(&ctx, files, handler, &site, &text);
                continue;
            }
        };
        result.expansions += 1;
        result.max_depth = result.max_depth.max(depth);
        result.sites.push(MacroSite {
            file: fid,
            call_span: site.call_span,
            replace_span: site.replace_span,
            macro_name: site.name.clone(),
            expansion_id: exp_id,
            depth,
            text,
            introduced,
        });
        apply_fragment(files, &site, &fragment);
    }
    result.sites.sort_by(|a, b| {
        (a.file, a.call_span.start, &a.macro_name).cmp(&(b.file, b.call_span.start, &b.macro_name))
    });

    // §6.3: leak detection over the expanded AST, in deterministic file order.
    for fid in files.keys().cloned().collect::<Vec<_>>() {
        let file_sites: Vec<&MacroSite> = result.sites.iter().filter(|s| s.file == fid).collect();
        check_hygiene_leaks(&files[&fid], fid, &file_sites, sources, handler);
    }
}

/// The matcher input for `name!(…)` / `name![…]` / `name!{…}`: the call group's children.
fn call_input(tokens: &[Tree]) -> Vec<Tree> {
    match tokens.first() {
        Some(Tree::Group(_, children, _)) => children.clone(),
        _ => tokens.to_vec(),
    }
}

fn collect_item_sites(item: &Item, idx: usize, out: &mut Vec<Site>, counter: &mut u32) {
    match &item.kind {
        ItemKind::Fn { body, .. } | ItemKind::Event { body, .. } => {
            if let crate::ast::Body::Block(b) = body {
                collect_block_sites(b, idx, out, counter);
            }
        }
        ItemKind::Chain { stmts, .. } => {
            for s in stmts {
                collect_stmt_sites(s, idx, out, counter);
            }
        }
        ItemKind::Impl { items, .. } | ItemKind::Trait { items, .. } => {
            for it in items {
                collect_item_sites(it, idx, out, counter);
            }
        }
        ItemKind::Const { init, .. } | ItemKind::Static { init, .. } => {
            if let Some(e) = init {
                collect_expr_sites(e, idx, out, counter);
            }
        }
        _ => {}
    }
}

fn collect_block_sites(b: &Block, idx: usize, out: &mut Vec<Site>, counter: &mut u32) {
    for s in &b.stmts {
        collect_stmt_sites(s, idx, out, counter);
    }
    if let Some(t) = &b.tail {
        collect_expr_sites(t, idx, out, counter);
    }
}

fn bump(counter: &mut u32) -> u32 {
    *counter += 1;
    *counter
}

fn collect_stmt_sites(s: &Stmt, idx: usize, out: &mut Vec<Site>, counter: &mut u32) {
    match &s.kind {
        StmtKind::Expr { expr, .. } => {
            if let ExprKind::MacroCall { name, tokens, .. } = &expr.kind {
                let order = bump(counter);
                out.push(Site {
                    order,
                    item_idx: idx,
                    name: name.clone(),
                    input: call_input(tokens),
                    call_span: expr.span,
                    replace_span: if s.terminated {
                        Span::new(expr.span.file, expr.span.start, s.span.end.max(expr.span.end))
                    } else {
                        expr.span
                    },
                    is_stmt: true,
                });
            } else {
                collect_expr_sites(expr, idx, out, counter);
            }
        }
        StmtKind::Let { init, .. } => {
            if let Some(e) = init {
                collect_expr_sites(e, idx, out, counter);
            }
        }
        StmtKind::Assign { target, value, .. } => {
            collect_expr_sites(target, idx, out, counter);
            collect_expr_sites(value, idx, out, counter);
        }
        StmtKind::If { cond, then, else_ } => {
            collect_expr_sites(cond, idx, out, counter);
            collect_block_sites(then, idx, out, counter);
            if let Some(e) = else_ {
                match &**e {
                    ElseBranch::Block(b) => collect_block_sites(b, idx, out, counter),
                    ElseBranch::If(inner) => collect_stmt_sites(inner, idx, out, counter),
                }
            }
        }
        StmtKind::While { cond, body } => {
            collect_expr_sites(cond, idx, out, counter);
            collect_block_sites(body, idx, out, counter);
        }
        StmtKind::Loop { body, .. } => collect_block_sites(body, idx, out, counter),
        StmtKind::For { iter, body, .. } => {
            collect_expr_sites(iter, idx, out, counter);
            collect_block_sites(body, idx, out, counter);
        }
        StmtKind::Match { scrut, arms } => {
            collect_expr_sites(scrut, idx, out, counter);
            for a in arms {
                if let Some(g) = &a.guard {
                    collect_expr_sites(g, idx, out, counter);
                }
                match &a.body {
                    ArmBody::Expr(e) => collect_expr_sites(e, idx, out, counter),
                    ArmBody::Block(b) => collect_block_sites(b, idx, out, counter),
                }
            }
        }
        StmtKind::Return { value } | StmtKind::Break { value, .. } => {
            if let Some(v) = value {
                collect_expr_sites(v, idx, out, counter);
            }
        }
        StmtKind::Unsafe(b) | StmtKind::Defer(b) => collect_block_sites(b, idx, out, counter),
        StmtKind::ConstAssert { cond, .. } => collect_expr_sites(cond, idx, out, counter),
        StmtKind::NestedItem(it) => collect_item_sites(it, idx, out, counter),
        _ => {}
    }
}

/// Records the outermost call in an expression tree; calls nested inside an expansion are expanded
/// recursively at token level (§6.4 depth is counted there).
fn collect_expr_sites(e: &Expr, idx: usize, out: &mut Vec<Site>, counter: &mut u32) {
    if let ExprKind::MacroCall { name, tokens, .. } = &e.kind {
        let order = bump(counter);
        out.push(Site {
            order,
            item_idx: idx,
            name: name.clone(),
            input: call_input(tokens),
            call_span: e.span,
            replace_span: e.span,
            is_stmt: false,
        });
        return;
    }
    match &e.kind {
        ExprKind::Unary { expr, .. } => collect_expr_sites(expr, idx, out, counter),
        ExprKind::Binary { lhs, rhs, .. } => {
            collect_expr_sites(lhs, idx, out, counter);
            collect_expr_sites(rhs, idx, out, counter);
        }
        ExprKind::Assign { target, value, .. } => {
            collect_expr_sites(target, idx, out, counter);
            collect_expr_sites(value, idx, out, counter);
        }
        ExprKind::Call { callee, args } => {
            collect_expr_sites(callee, idx, out, counter);
            for a in args {
                collect_expr_sites(&a.expr, idx, out, counter);
            }
        }
        ExprKind::Method { recv, args, .. } => {
            collect_expr_sites(recv, idx, out, counter);
            for a in args {
                collect_expr_sites(&a.expr, idx, out, counter);
            }
        }
        ExprKind::Field { base, .. } => collect_expr_sites(base, idx, out, counter),
        ExprKind::Index { base, index } => {
            collect_expr_sites(base, idx, out, counter);
            collect_expr_sites(index, idx, out, counter);
        }
        ExprKind::Tuple(v) | ExprKind::Array(v) => {
            for x in v {
                collect_expr_sites(x, idx, out, counter);
            }
        }
        ExprKind::ArrayRepeat { value, len } => {
            collect_expr_sites(value, idx, out, counter);
            collect_expr_sites(len, idx, out, counter);
        }
        ExprKind::StructLit { fields, rest, .. } => {
            for (_, v) in fields {
                collect_expr_sites(v, idx, out, counter);
            }
            if let Some(r) = rest {
                collect_expr_sites(r, idx, out, counter);
            }
        }
        ExprKind::If { cond, then, else_ } => {
            collect_expr_sites(cond, idx, out, counter);
            collect_block_sites(then, idx, out, counter);
            if let Some(eb) = else_ {
                match &**eb {
                    ElseBranch::Block(b) => collect_block_sites(b, idx, out, counter),
                    ElseBranch::If(s) => collect_stmt_sites(s, idx, out, counter),
                }
            }
        }
        ExprKind::Match { scrut, arms } => {
            collect_expr_sites(scrut, idx, out, counter);
            for a in arms {
                if let Some(g) = &a.guard {
                    collect_expr_sites(g, idx, out, counter);
                }
                match &a.body {
                    ArmBody::Expr(x) => collect_expr_sites(x, idx, out, counter),
                    ArmBody::Block(b) => collect_block_sites(b, idx, out, counter),
                }
            }
        }
        ExprKind::Block(b) | ExprKind::Unsafe(b) => collect_block_sites(b, idx, out, counter),
        ExprKind::Closure { body, .. } => collect_block_sites(body, idx, out, counter),
        ExprKind::Ref { expr, .. } | ExprKind::Deref { expr } | ExprKind::Try { expr } | ExprKind::Move { expr } => collect_expr_sites(expr, idx, out, counter),
        ExprKind::Cast { expr, .. } | ExprKind::Is { expr, .. } => collect_expr_sites(expr, idx, out, counter),
        ExprKind::Range { lo, hi, .. } => {
            if let Some(l) = lo {
                collect_expr_sites(l, idx, out, counter);
            }
            if let Some(h) = hi {
                collect_expr_sites(h, idx, out, counter);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- site expansion

fn expand_site(
    ctx: &mut RunCtx,
    files: &BTreeMap<FileId, ParsedFile>,
    handler: &mut Handler,
    opts: &ExpandOptions,
    site: &Site,
) -> Outcome {
    // §6.8 `--macro name` filter.
    if let Some(only) = &opts.only_macro {
        if only != &site.name {
            return Outcome::Skip;
        }
    }
    let call_file = site.call_span.file;
    let Some(def) = ctx.defs.get(&site.name).cloned() else {
        // Unknown macro: a name-resolution matter (§7.1 stage [3]), not a stage-[2] error.
        return Outcome::Skip;
    };
    // Same-file definitions are visible only from their definition point onwards (§6.5 timing).
    if def.file == call_file && def.pos > site.call_span.start {
        let pos = ctx.sources.line_col(site.call_span).0;
        handler.emit(
            Diagnostic::new("E-0402", Phase::Macro, call_file, pos)
                .titled(format!("宏 `{}` 在定义之前被调用", site.name))
                .note("包内宏自定义点起可见（§6.5 / §5.5）")
                .help("将宏定义移动到调用之前")
                .expansion(format!("macro {} → {}:{}", site.name, ctx.display(call_file), pos.line)),
        );
        return Outcome::Failed;
    }
    let arms = match files.get(&def.file).and_then(|f| f.items.get(def.item_idx)).map(|i| &i.kind) {
        Some(ItemKind::MacroRules { arms, .. }) => arms.clone(),
        _ => return Outcome::Skip,
    };
    let compiled: Vec<ArmC> = arms.iter().map(|a| compile_arm(a)).collect();

    for (arm_no, arm) in compiled.iter().enumerate() {
        let mut env: Scope = BTreeMap::new();
        let v = Validator { sources: ctx.sources, file: call_file, anchor: site.call_span };
        if match_seq(&arm.items, &site.input, 0, &mut env, &v) != Some(site.input.len()) {
            continue;
        }
        let exp_id = ctx.next_expansion_id;
        ctx.next_expansion_id += 1;
        let mut out = subst(&arm.expander, &env, &arm.introduced, exp_id);
        let mut max_depth = 1usize;
        expand_trees(ctx, files, handler, opts, site, &mut out, 1, &mut max_depth);
        if ctx.depth_tripped {
            return Outcome::Failed;
        }
        if site.is_stmt {
            unwrap_outer_brace(&mut out);
        }
        let _ = arm_no;
        let mut introduced = arm.introduced.clone();
        introduced.sort();
        introduced.dedup();
        return Outcome::Expanded { trees: out, introduced, depth: max_depth, exp_id };
    }

    // No arm matched: §6.5 illegal expansion (no dedicated code exists in §9.3; E-0402 is the
    // closest normative category and the code table's macro group).
    let rendered = render_trees(&site.input);
    let pos = ctx.sources.line_col(site.call_span).0;
    handler.emit(
        Diagnostic::new("E-0402", Phase::Macro, call_file, pos)
            .titled(format!("宏 `{}` 无匹配分支", site.name))
            .note(format!("展开后片段: `{}`", rendered))
            .note("展开结果不是合法 AST 片段；无分支匹配该调用（§6.2/§6.5）")
            .help("修正调用参数或展开器")
            .expansion(format!("macro {} → {}:{}", site.name, ctx.display(call_file), pos.line)),
    );
    Outcome::Failed
}

/// E-0402 (§6.5): the diagnostic points at the **expander** (the first arm of the definition in
/// the definition file), prints the expanded fragment, and carries the call-site provenance.
fn emit_e0402(
    ctx: &RunCtx,
    files: &BTreeMap<FileId, ParsedFile>,
    handler: &mut Handler,
    site: &Site,
    text: &str,
) {
    let call_file = site.call_span.file;
    let call_line = ctx.line(site.call_span);
    // Expander location: the macro's first arm span in the definition file; fall back to the
    // definition item, then to the call site if even that is gone.
    let expander: (FileId, Span) = ctx
        .defs
        .get(&site.name)
        .and_then(|d| {
            let ItemKind::MacroRules { arms, .. } = &files.get(&d.file)?.items.get(d.item_idx)?.kind else {
                return None;
            };
            let arm = arms.first()?;
            Some((d.file, arm.span))
        })
        .or_else(|| ctx.defs.get(&site.name).map(|d| (d.file, site.call_span)))
        .unwrap_or((call_file, site.call_span));
    let (tfile, tspan) = expander;
    let pos = ctx.sources.line_col(tspan).0;
    let (a, b) = ctx.sources.line_col(tspan);
    let mut d = Diagnostic::new("E-0402", Phase::Macro, tfile, pos)
        .titled("宏展开产物非法")
        .note(format!("展开后片段: `{text}`"))
        .note("展开结果不是合法 AST 片段（§6.5）")
        .help("修正展开器（诊断指向展开器）")
        .expansion(format!("macro {} → {}:{}", site.name, ctx.display(call_file), call_line));
    d = d.region(tfile, a.line, a.column, b.column.max(a.column + 1), format!("宏 {} 的展开器", site.name));
    handler.emit(d);
}

/// Recursive token-level expansion of `ident ! (…) ` shapes inside produced trees (§6.4 depth).
fn expand_trees(
    ctx: &mut RunCtx,
    files: &BTreeMap<FileId, ParsedFile>,
    handler: &mut Handler,
    opts: &ExpandOptions,
    site: &Site,
    trees: &mut Vec<Tree>,
    depth: usize,
    max_depth: &mut usize,
) {
    *max_depth = (*max_depth).max(depth);
    let mut i = 0usize;
    let mut rebuilt: Vec<Tree> = Vec::new();
    while i < trees.len() {
        if let Tree::Group(d, inner, sp) = &trees[i] {
            let mut inner2 = inner.clone();
            expand_trees(ctx, files, handler, opts, site, &mut inner2, depth, max_depth);
            rebuilt.push(Tree::Group(*d, inner2, *sp));
            i += 1;
            continue;
        }
        let call_shape = i + 2 < trees.len()
            && matches!(&trees[i], Tree::Leaf(t) if matches!(t.kind, Kind::Ident(_)))
            && matches!(&trees[i + 1], Tree::Leaf(t) if t.is_p(Punct::Bang))
            && matches!(&trees[i + 2], Tree::Group(_, _, _));
        if call_shape {
            let mname = match &trees[i] {
                Tree::Leaf(t) => t.ident().unwrap_or_default().to_string(),
                _ => String::new(),
            };
            let children = match &trees[i + 2] {
                Tree::Group(_, c, _) => c.clone(),
                _ => Vec::new(),
            };
            if let Some(expanded) =
                expand_nested(ctx, files, handler, opts, site, &mname, &children, depth + 1, max_depth)
            {
                rebuilt.extend(expanded);
                i += 3;
                continue;
            }
            // Left as written (unknown macro / `--macro` filter / depth tripped).
            rebuilt.push(trees[i].clone());
            rebuilt.push(trees[i + 1].clone());
            rebuilt.push(trees[i + 2].clone());
            i += 3;
            continue;
        }
        rebuilt.push(trees[i].clone());
        i += 1;
    }
    *trees = rebuilt;
}

/// Expands one nested call found in produced trees. `None` = leave the tokens as written.
#[allow(clippy::too_many_arguments)]
fn expand_nested(
    ctx: &mut RunCtx,
    files: &BTreeMap<FileId, ParsedFile>,
    handler: &mut Handler,
    opts: &ExpandOptions,
    site: &Site,
    name: &str,
    input: &[Tree],
    depth: usize,
    max_depth: &mut usize,
) -> Option<Vec<Tree>> {
    *max_depth = (*max_depth).max(depth);
    if depth > MAX_EXPANSION_DEPTH {
        if !ctx.depth_tripped {
            let pos = ctx.sources.line_col(site.call_span).0;
            handler.emit(
                Diagnostic::new("E-0401", Phase::Macro, site.call_span.file, pos)
                    .titled("宏展开深度超限")
                    .note(format!("递归深度超过 {}（§6.4）", MAX_EXPANSION_DEPTH))
                    .help("简化递归")
                    .expansion(format!("macro {} → {}:{}", name, ctx.display(site.call_span.file), pos.line)),
            );
            ctx.depth_tripped = true;
        }
        return None;
    }
    if opts.only_macro.as_ref().map_or(false, |only| only != name) {
        return None;
    }
    let def = ctx.defs.get(name).cloned()?;
    let arms = match files.get(&def.file).and_then(|f| f.items.get(def.item_idx)).map(|i| &i.kind) {
        Some(ItemKind::MacroRules { arms, .. }) => arms.clone(),
        _ => return None,
    };
    let call_file = site.call_span.file;
    let v = Validator { sources: ctx.sources, file: call_file, anchor: site.call_span };
    for a in &arms {
        let arm = compile_arm(a);
        let mut env: Scope = BTreeMap::new();
        if match_seq(&arm.items, input, 0, &mut env, &v) == Some(input.len()) {
            let exp_id = ctx.next_expansion_id;
            ctx.next_expansion_id += 1;
            let mut out = subst(&arm.expander, &env, &arm.introduced, exp_id);
            expand_trees(ctx, files, handler, opts, site, &mut out, depth, max_depth);
            return Some(out);
        }
    }
    None
}

/// A statement-position expansion written as `=> { … }` splices its braces' contents as
/// statements.
fn unwrap_outer_brace(out: &mut Vec<Tree>) {
    if out.len() == 1 {
        if let Tree::Group(Delim::Brace, inner, _) = &out[0] {
            let v = inner.clone();
            *out = v;
        }
    }
}

// ---------------------------------------------------------------- matcher compilation

fn compile_arm(a: &crate::ast::MacroArm) -> ArmC {
    // §3.9: `macro_matcher = "$(" macro_pat ")"` — the parser stores the whole matcher as one
    // parenthesised group; accept a bare sequence too.
    let matcher_trees: Vec<Tree> = match a.matcher.as_slice() {
        [Tree::Group(Delim::Paren, inner, _)] => inner.clone(),
        other => other.to_vec(),
    };
    let items = compile_matcher(&matcher_trees);
    let mut introduced = collect_introduced(&a.expander);
    introduced.sort();
    introduced.dedup();
    ArmC { items, expander: a.expander.clone(), introduced }
}

fn compile_matcher(trees: &[Tree]) -> Vec<MItem> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < trees.len() {
        if let Tree::Leaf(t) = &trees[i] {
            if t.is_p(Punct::Dollar) {
                // repetition: `$(` … `)` sep? op
                if let Some(Tree::Group(Delim::Paren, inner, _)) = trees.get(i + 1) {
                    let items = compile_matcher(inner);
                    let mut j = i + 2;
                    let sep = match trees.get(j) {
                        Some(Tree::Leaf(l)) if l.is_p(Punct::Comma) => {
                            j += 1;
                            Some(Punct::Comma)
                        }
                        Some(Tree::Leaf(l)) if l.is_p(Punct::Semi) => {
                            j += 1;
                            Some(Punct::Semi)
                        }
                        Some(Tree::Leaf(l)) if l.is_p(Punct::Hat) => {
                            j += 1;
                            Some(Punct::Hat)
                        }
                        _ => None,
                    };
                    let op = match trees.get(j) {
                        Some(Tree::Leaf(l)) if l.is_p(Punct::Star) => Some(RepOp::Star),
                        Some(Tree::Leaf(l)) if l.is_p(Punct::Plus) => Some(RepOp::Plus),
                        Some(Tree::Leaf(l)) if l.is_p(Punct::Question) => Some(RepOp::Optional),
                        _ => None,
                    };
                    if let Some(op) = op {
                        let bound = collect_bound_names(&items);
                        out.push(MItem::Rep { items, sep, op, bound });
                        i = j + 1;
                        continue;
                    }
                    // `$(` without an operator token: splice the group contents inline.
                    out.extend(items);
                    i += 2;
                    continue;
                }
                // binding: `$ name` [ `:` class ]  (a colon-less `$name` binds `:tt`)
                if let Some(Tree::Leaf(nt)) = trees.get(i + 1) {
                    if let Some(n) = nt.ident() {
                        let name = n.to_string();
                        let mut class = Class::Tt;
                        let mut j = i + 2;
                        if trees.get(j).map_or(false, |x| matches!(x, Tree::Leaf(l) if l.is_p(Punct::Colon))) {
                            j += 1;
                            let cname = trees.get(j).and_then(|x| match x {
                                Tree::Leaf(l) => l
                                    .ident()
                                    .map(|s| s.to_string())
                                    .or_else(|| match &l.kind {
                                        Kind::Keyword(k) => Some(k.text().to_string()),
                                        _ => None,
                                    }),
                                _ => None,
                            });
                            if let Some(c) = cname.and_then(|c| Class::parse(&c)) {
                                class = c;
                                j += 1;
                            }
                        }
                        out.push(MItem::Bind { name, class });
                        i = j;
                        continue;
                    }
                }
            }
        }
        out.push(MItem::Lit(trees[i].clone()));
        i += 1;
    }
    out
}

fn collect_bound_names(items: &[MItem]) -> Vec<String> {
    let mut v = Vec::new();
    for it in items {
        match it {
            MItem::Bind { name, .. } => v.push(name.clone()),
            MItem::Rep { items, .. } => v.extend(collect_bound_names(items)),
            MItem::Lit(_) => {}
        }
    }
    v.sort();
    v.dedup();
    v
}

// ---------------------------------------------------------------- literal tree equality

fn lit_eq(a: &Tree, b: &Tree) -> bool {
    match (a, b) {
        (Tree::Leaf(x), Tree::Leaf(y)) => tok_eq(x, y),
        (Tree::Group(d1, c1, _), Tree::Group(d2, c2, _)) => {
            d1 == d2 && c1.len() == c2.len() && c1.iter().zip(c2.iter()).all(|(p, q)| lit_eq(p, q))
        }
        _ => false,
    }
}

fn tok_eq(a: &Tok, b: &Tok) -> bool {
    match (&a.kind, &b.kind) {
        (Kind::Ident(x), Kind::Ident(y)) => x == y,
        (Kind::Keyword(x), Kind::Keyword(y)) => x == y,
        (Kind::Punct(x), Kind::Punct(y)) => x == y,
        (Kind::Lifetime(x), Kind::Lifetime(y)) => x == y,
        (Kind::Lit(l1), Kind::Lit(l2)) => l1 == l2,
        (Kind::EmbedLang(x), Kind::EmbedLang(y)) => x == y,
        _ => false,
    }
}

// ---------------------------------------------------------------- matching (§6.2)

/// Context for fragment validation: candidate spans are parsed through the real parser, anchored
/// at the call site's file so their spans resolve to real source text.
struct Validator<'a> {
    sources: &'a Sources,
    file: FileId,
    anchor: Span,
}

/// Matches `items` against `input` starting at `pos`; on success returns the end offset and fills
/// `env`. Multi-token fragments backtrack across candidate spans; repetitions are greedy.
fn match_seq(items: &[MItem], input: &[Tree], pos: usize, env: &mut Scope, v: &Validator) -> Option<usize> {
    if items.is_empty() {
        // End of this matcher sequence. Whether the *whole input* was consumed is decided by the
        // top-level caller (`Some(input.len())`), so nested repetitions may stop mid-input (§6.2).
        return Some(pos);
    }
    match &items[0] {
        MItem::Lit(t) => {
            if pos < input.len() && lit_eq(t, &input[pos]) {
                match_seq(&items[1..], input, pos + 1, env, v)
            } else {
                None
            }
        }
        MItem::Bind { name, class } => {
            for end in bind_candidates(*class, input, pos) {
                let bound = input[pos..end].to_vec();
                if !frag_parses(v, &bound, *class) {
                    continue;
                }
                let mut trial = env.clone();
                trial.insert(name.clone(), Frag::One(bound));
                if let Some(e2) = match_seq(&items[1..], input, end, &mut trial, v) {
                    *env = trial;
                    return Some(e2);
                }
            }
            None
        }
        MItem::Rep { items: inner, sep, op, bound } => {
            let mut iterations: Vec<Scope> = Vec::new();
            let mut p = pos;
            loop {
                if *op == RepOp::Optional && iterations.len() == 1 {
                    break;
                }
                let mut child: Scope = BTreeMap::new();
                let Some(end) = match_seq(inner, input, p, &mut child, v) else { break };
                if end == p {
                    break; // zero-width iteration: stop (prevents infinite repetition)
                }
                iterations.push(child);
                p = end;
                match sep {
                    Some(sp) if p < input.len() && is_punct_tree(&input[p], *sp) => p += 1,
                    Some(_) => break,
                    None => break,
                }
            }
            if *op == RepOp::Plus && iterations.is_empty() {
                return None;
            }
            let mut trial = env.clone();
            for n in bound {
                let mut vals = Vec::new();
                for it in &iterations {
                    match it.get(n) {
                        Some(f) => vals.push(f.clone()),
                        None => vals.push(Frag::One(Vec::new())),
                    }
                }
                trial.insert(n.clone(), Frag::Many(vals));
            }
            match_seq(&items[1..], input, p, &mut trial, v).map(|e| {
                *env = trial;
                e
            })
        }
    }
}

fn is_punct_tree(t: &Tree, p: Punct) -> bool {
    matches!(t, Tree::Leaf(l) if l.is_p(p))
}

/// Candidate end offsets for a fragment bind. Single-tree classes have exactly one candidate;
/// multi-token classes extend only to the next *top-level* separator (commas inside array or call
/// groups are nested, so `g(1, 2)` binds as one `:expr`). Candidates are tried longest-first —
/// §6.2 fragment matching is greedy, and the caller backtracks.
fn bind_candidates(class: Class, input: &[Tree], pos: usize) -> Vec<usize> {
    match class {
        Class::Ident | Class::Tt | Class::Block => {
            if pos < input.len() {
                vec![pos + 1]
            } else {
                Vec::new()
            }
        }
        Class::Expr | Class::Stmt | Class::Pat | Class::Ty | Class::Meta => {
            let mut hard_stop = input.len();
            let mut stmt_end = None;
            for i in pos..input.len() {
                if i == pos {
                    // a leading separator can never be *inside* the fragment
                } else {
                    let cut = match class {
                        Class::Expr => matches!(top_punct(input, i), Some(Punct::Comma | Punct::Semi)),
                        Class::Pat => matches!(
                            top_punct(input, i),
                            Some(Punct::Comma | Punct::Semi | Punct::FatArrow)
                        ),
                        Class::Ty => matches!(top_punct(input, i), Some(Punct::Comma | Punct::Semi | Punct::Colon)),
                        Class::Meta => matches!(top_punct(input, i), Some(Punct::Comma)),
                        Class::Stmt => matches!(top_punct(input, i), Some(Punct::Comma)),
                        _ => false,
                    };
                    if cut {
                        hard_stop = i;
                        break;
                    }
                }
                if class == Class::Stmt && top_punct(input, i) == Some(Punct::Semi) {
                    stmt_end = Some(i + 1); // `:stmt` includes its terminating `;`
                    break;
                }
            }
            let max = stmt_end.unwrap_or(hard_stop).max(pos + 1).min(input.len());
            let mut v: Vec<usize> = (pos + 1..=max).collect();
            v.reverse(); // longest first
            v
        }
    }
}

fn top_punct(input: &[Tree], i: usize) -> Option<Punct> {
    match input.get(i) {
        Some(Tree::Leaf(l)) => match &l.kind {
            Kind::Punct(p) => Some(*p),
            _ => None,
        },
        _ => None,
    }
}

// ---------------------------------------------------------------- fragment validation

/// True when `trees` re-parses as a single legal fragment of `class` (§6.2). Structural classes
/// are checked by shape; the phrase classes go through the real parser.
fn frag_parses(v: &Validator, trees: &[Tree], class: Class) -> bool {
    match class {
        Class::Ident => trees.len() == 1 && matches!(&trees[0], Tree::Leaf(l) if l.ident().is_some()),
        Class::Tt => trees.len() == 1,
        Class::Block => trees.len() == 1 && matches!(&trees[0], Tree::Group(Delim::Brace, _, _)),
        Class::Meta => !trees.is_empty(),
        Class::Expr | Class::Stmt | Class::Pat | Class::Ty => {
            let toks = flatten_trees(trees);
            if toks.is_empty() {
                return false;
            }
            let want = match class {
                Class::Expr => Want::Expr,
                Class::Stmt => Want::OneStmt,
                Class::Pat => Want::Pat,
                _ => Want::Ty,
            };
            parse_fragment(v.sources, v.file, v.anchor, &toks, want).is_some()
        }
    }
}

// ---------------------------------------------------------------- substitution

/// Produces the expansion token trees: fragment splices (call-site tokens, untouched — §6.3's
/// second law) and macro literals, with introduced identifiers rustled to
/// `__infy_m<expansion_id>_<name>`.
fn subst(trees: &[Tree], env: &Scope, introduced: &[String], exp_id: u32) -> Vec<Tree> {
    let mut out: Vec<Tree> = Vec::new();
    let mut prev_punct: Option<Punct> = None;
    let mut i = 0usize;
    while i < trees.len() {
        match &trees[i] {
            Tree::Leaf(l) if l.is_p(Punct::Dollar) => {
                if let Some(Tree::Group(Delim::Paren, inner, _)) = trees.get(i + 1) {
                    let (sep, op, next) = parse_expander_rep(trees, i + 2);
                    let _ = op;
                    let produced = subst_rep(inner, sep, env, introduced, exp_id);
                    out.extend(produced);
                    prev_punct = None;
                    i = next;
                    continue;
                }
                if let Some(Tree::Leaf(nt)) = trees.get(i + 1) {
                    if let Some(n) = nt.ident() {
                        let name = n.to_string();
                        let mut j = i + 2;
                        // tolerate a stray `:class` in expander position
                        if trees.get(j).map_or(false, |x| matches!(x, Tree::Leaf(l) if l.is_p(Punct::Colon))) {
                            j += 1;
                            if trees.get(j).map_or(false, |x| matches!(x, Tree::Leaf(_))) {
                                j += 1;
                            }
                        }
                        match env.get(&name) {
                            Some(Frag::One(ts)) => {
                                for bt in ts {
                                    out.push(bt.clone());
                                }
                            }
                            Some(Frag::Many(vs)) => {
                                // A bare repetition variable outside its repetition: take the
                                // first iteration's fragment (Rust rejects this outright).
                                if let Some(Frag::One(ts)) = vs.first() {
                                    for bt in ts {
                                        out.push(bt.clone());
                                    }
                                }
                            }
                            None => {}
                        }
                        prev_punct = None;
                        i = j;
                        continue;
                    }
                }
                out.push(Tree::Leaf(l.clone()));
                prev_punct = Some(Punct::Dollar);
                i += 1;
            }
            Tree::Leaf(l) => {
                let mut tok = l.clone();
                let dotted = matches!(prev_punct, Some(Punct::Dot) | Some(Punct::ColonColon));
                if !dotted {
                    if let Some(n) = tok.ident() {
                        if introduced.iter().any(|x| x == n) {
                            let rn = format!("__infy_m{}_{}", exp_id, n);
                            tok.text = rn.clone();
                            tok.kind = Kind::Ident(rn);
                        }
                    }
                }
                prev_punct = match tok.kind {
                    Kind::Punct(p) => Some(p),
                    _ => None,
                };
                out.push(Tree::Leaf(tok));
                i += 1;
            }
            Tree::Group(d, inner, sp) => {
                let sub = subst(inner, env, introduced, exp_id);
                out.push(Tree::Group(*d, sub, *sp));
                prev_punct = None;
                i += 1;
            }
        }
    }
    out
}

fn parse_expander_rep(trees: &[Tree], start: usize) -> (Option<Punct>, Option<RepOp>, usize) {
    let mut j = start;
    let sep = match trees.get(j) {
        Some(Tree::Leaf(l)) if l.is_p(Punct::Comma) => {
            j += 1;
            Some(Punct::Comma)
        }
        Some(Tree::Leaf(l)) if l.is_p(Punct::Semi) => {
            j += 1;
            Some(Punct::Semi)
        }
        Some(Tree::Leaf(l)) if l.is_p(Punct::Hat) => {
            j += 1;
            Some(Punct::Hat)
        }
        _ => None,
    };
    let op = match trees.get(j) {
        Some(Tree::Leaf(l)) if l.is_p(Punct::Star) => {
            j += 1;
            Some(RepOp::Star)
        }
        Some(Tree::Leaf(l)) if l.is_p(Punct::Plus) => {
            j += 1;
            Some(RepOp::Plus)
        }
        Some(Tree::Leaf(l)) if l.is_p(Punct::Question) => {
            j += 1;
            Some(RepOp::Optional)
        }
        _ => None,
    };
    (sep, op, j)
}

fn subst_rep(
    inner: &[Tree],
    sep: Option<Punct>,
    env: &Scope,
    introduced: &[String],
    exp_id: u32,
) -> Vec<Tree> {
    let inner_names = expander_bound_names(inner);
    let mut out: Vec<Tree> = Vec::new();
    if inner_names.is_empty() {
        // Literal-only repetition (`$(,)?` etc.) in the expander: emitted once.
        out.extend(subst(inner, env, introduced, exp_id));
        return out;
    }
    let count = inner_names
        .iter()
        .filter_map(|n| match env.get(n) {
            Some(Frag::Many(v)) => Some(v.len()),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    for i in 0..count {
        if i > 0 {
            if let Some(sp) = sep {
                out.push(Tree::Leaf(synth_punct(sp)));
            }
        }
        let mut child: Scope = BTreeMap::new();
        for (n, f) in env.iter() {
            match f {
                Frag::Many(v) => {
                    if let Some(x) = v.get(i) {
                        child.insert(n.clone(), x.clone());
                    }
                }
                Frag::One(_) => {
                    child.insert(n.clone(), f.clone());
                }
            }
        }
        out.extend(subst(inner, &child, introduced, exp_id));
    }
    out
}

fn expander_bound_names(trees: &[Tree]) -> Vec<String> {
    let mut v = Vec::new();
    let mut i = 0usize;
    while i < trees.len() {
        match &trees[i] {
            Tree::Leaf(l) if l.is_p(Punct::Dollar) => {
                if let Some(Tree::Leaf(nt)) = trees.get(i + 1) {
                    if let Some(n) = nt.ident() {
                        v.push(n.to_string());
                        i += 2;
                        continue;
                    }
                }
                i += 1;
            }
            Tree::Group(_, inner, _) => {
                v.extend(expander_bound_names(inner));
                i += 1;
            }
            _ => i += 1,
        }
    }
    v.sort();
    v.dedup();
    v
}

fn synth_punct(p: Punct) -> Tok {
    Tok {
        kind: Kind::Punct(p),
        span: Span::empty_at(u32::MAX, 0),
        text: punct_text(p).to_string(),
    }
}

// ---------------------------------------------------------------- introduced names (§6.3)

const BINDER_KWS: [Kw; 10] = [
    Kw::Let,
    Kw::Var,
    Kw::Const,
    Kw::Static,
    Kw::Fn,
    Kw::Struct,
    Kw::Enum,
    Kw::Union,
    Kw::Trait,
    Kw::Type,
];

/// Identifiers the macro body itself introduces: a binder keyword immediately followed by a plain
/// (non-fragment) identifier, plus `for <pat> in` bindings. A `$`-substituted name is *not*
/// introduced — it belongs to the call site (§6.3).
fn collect_introduced(expander: &[Tree]) -> Vec<String> {
    let mut names = Vec::new();
    walk_introduced(expander, &mut names);
    names
}

fn walk_introduced(trees: &[Tree], out: &mut Vec<String>) {
    let mut i = 0usize;
    while i < trees.len() {
        match &trees[i] {
            Tree::Group(_, inner, _) => walk_introduced(inner, out),
            Tree::Leaf(l) => {
                if let Kind::Keyword(k) = &l.kind {
                    if BINDER_KWS.contains(k) {
                        // `let mut arr` — the `mut` modifier sits between binder and name.
                        let mut j = i + 1;
                        if let Some(Tree::Leaf(m)) = trees.get(j) {
                            if m.is_kw(Kw::Mut) {
                                j += 1;
                            }
                        }
                        if let Some(Tree::Leaf(nt)) = trees.get(j) {
                            if let Some(n) = nt.ident() {
                                out.push(n.to_string());
                            }
                        }
                    }
                    if matches!(k, Kw::For) {
                        // `for <pat> in …` — plain identifiers until `in` are bindings.
                        let mut j = i + 1;
                        while j < trees.len() {
                            match &trees[j] {
                                Tree::Leaf(l2) => {
                                    if l2.is_kw(Kw::In) {
                                        break;
                                    }
                                    if let Some(n) = l2.ident() {
                                        out.push(n.to_string());
                                    }
                                }
                                Tree::Group(_, inner, _) => walk_introduced(inner, out),
                            }
                            j += 1;
                        }
                    }
                }
            }
        }
        i += 1;
    }
}

// ---------------------------------------------------------------- re-parse (§6.5)

/// The AST fragment an expansion turned into.
enum Fragment {
    Expr(Expr),
    Stmts(Vec<Stmt>),
}

/// Parses the flattened expansion token stream: expression context → one expression (a
/// multi-statement expansion becomes a block expression — the §6.1 `vec!` arm 3 case); statement
/// context → a statement list.
fn parse_expansion(
    sources: &Sources,
    file: FileId,
    call_span: Span,
    toks: &[Tok],
    is_stmt: bool,
) -> Option<Fragment> {
    if is_stmt {
        parse_fragment(sources, file, call_span, toks, Want::StmtList)
    } else {
        parse_fragment(sources, file, call_span, toks, Want::Expr)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Want {
    Expr,
    OneStmt,
    StmtList,
    Ty,
    Pat,
}

/// Parses one fragment through the real parser: the token stream is wrapped in the minimal legal
/// file for the fragment kind, parsed, and the fragment extracted. Any parse-level E diagnostic
/// means the stream is not a legal fragment; the throwaway handler discards them.
fn parse_fragment(sources: &Sources, file: FileId, anchor_span: Span, toks: &[Tok], want: Want) -> Option<Fragment> {
    let anchor = Span::new(file, anchor_span.start, anchor_span.start);
    let kw = |s: &'static str| Kind::Keyword(Kw::from_text(s).unwrap_or(Kw::Fn));
    let probe_file = |s: &str| Kind::Ident(s.to_string());
    let pu = |p: Punct| Kind::Punct(p);
    let head: Vec<(&str, Kind)> = match want {
        Want::Ty => vec![
            ("type", kw("type")),
            ("__expand_probe", probe_file("__expand_probe")),
            ("=", pu(Punct::Eq)),
        ],
        Want::Pat => vec![
            ("fn", kw("fn")),
            ("__expand_probe", probe_file("__expand_probe")),
            ("(", pu(Punct::LParen)),
            (")", pu(Punct::RParen)),
            ("{", pu(Punct::LBrace)),
            ("match", kw("match")),
            ("(", pu(Punct::LParen)),
            ("__expand_probe_scrut", probe_file("__expand_probe_scrut")),
            (")", pu(Punct::RParen)),
            ("{", pu(Punct::LBrace)),
        ],
        _ => vec![
            ("fn", kw("fn")),
            ("__expand_probe", probe_file("__expand_probe")),
            ("(", pu(Punct::LParen)),
            (")", pu(Punct::RParen)),
            ("{", pu(Punct::LBrace)),
        ],
    };
    let tail: Vec<(&str, Kind)> = match want {
        Want::Ty => vec![(";", pu(Punct::Semi))],
        Want::Pat => vec![
            ("=>", pu(Punct::FatArrow)),
            ("{", pu(Punct::LBrace)),
            ("}", pu(Punct::RBrace)),
            ("}", pu(Punct::RBrace)),
            ("}", pu(Punct::RBrace)),
        ],
        _ => vec![("}", pu(Punct::RBrace))],
    };
    let mut probe_toks: Vec<Tok> = Vec::new();
    for (text, kind) in &head {
        probe_toks.push(Tok { kind: kind.clone(), span: anchor, text: text.to_string() });
    }
    probe_toks.extend_from_slice(toks);
    for (text, kind) in &tail {
        probe_toks.push(Tok { kind: kind.clone(), span: anchor, text: text.to_string() });
    }
    probe_toks.push(Tok { kind: Kind::Eof, span: anchor, text: String::new() });

    let lexed = crate::lexer::Lexed { toks: probe_toks, ..Default::default() };
    let mut scratch = Handler::default();
    let parsed = crate::parser::parse(sources, file, &lexed, &mut scratch);
    if scratch.diags.iter().any(|d| d.code.starts_with("E-")) {
        return None;
    }
    match want {
        Want::Ty => {
            for it in &parsed.items {
                if let ItemKind::TypeAlias { .. } = &it.kind {
                    return Some(Fragment::Expr(Expr::new(ExprKind::Invalid, it.span)));
                }
            }
            None
        }
        Want::Pat => {
            let b = first_fn_block(&parsed)?;
            // the parser may land `match … {}` either as a statement or as the block tail
            let stmt_arms: Option<&Vec<MatchArm>> = match b.stmts.first().map(|s| &s.kind) {
                Some(StmtKind::Match { arms, .. }) => Some(arms),
                _ => None,
            };
            let tail_arms: Option<&Vec<MatchArm>> = match &b.tail {
                Some(t) => match &t.kind {
                    ExprKind::Match { arms, .. } => Some(arms),
                    _ => None,
                },
                None => None,
            };
            let m = match stmt_arms.or(tail_arms) {
                Some(m) => m,
                None => return None,
            };
            if m.len() == 1 {
                return Some(Fragment::Expr(Expr::new(ExprKind::Invalid, m[0].pat.span)));
            }
            None
        }
        Want::OneStmt => {
            let b = first_fn_block(&parsed)?;
            if b.stmts.len() == 1 && b.tail.is_none() {
                Some(Fragment::Stmts(b.stmts))
            } else {
                None
            }
        }
        Want::StmtList => {
            let b = first_fn_block(&parsed)?;
            let mut stmts = b.stmts;
            if let Some(t) = b.tail {
                let span = t.span;
                stmts.push(Stmt {
                    kind: StmtKind::Expr { expr: t, linkage: Vec::new() },
                    attrs: Vec::new(),
                    docs: Default::default(),
                    span,
                    terminated: false,
                });
            }
            Some(Fragment::Stmts(stmts))
        }
        Want::Expr => {
            let b = first_fn_block(&parsed)?;
            if b.stmts.is_empty() {
                let t = b.tail.clone()?;
                return Some(Fragment::Expr(t));
            }
            if b.stmts.len() == 1 && b.tail.is_none() {
                if let StmtKind::Expr { expr, .. } = &b.stmts[0].kind {
                    return Some(Fragment::Expr(expr.clone()));
                }
            }
            // multi-statement expansion in expression position ⇒ block expression (§6.1 vec!)
            { let bs = b.span; Some(Fragment::Expr(Expr::new(ExprKind::Block(Box::new(b)), bs))) }
        }
    }
}

fn first_fn_block(pf: &ParsedFile) -> Option<Block> {
    for it in &pf.items {
        if let ItemKind::Fn { body, .. } = &it.kind {
            if let crate::ast::Body::Block(b) = body {
                return Some(b.clone());
            }
        }
    }
    None
}

// ---------------------------------------------------------------- AST rewrite

fn apply_fragment(files: &mut BTreeMap<FileId, ParsedFile>, site: &Site, frag: &Fragment) {
    let fid = site.call_span.file;
    let Some(file) = files.get_mut(&fid) else { return };
    let Some(item) = file.items.get_mut(site.item_idx) else { return };
    apply_item_fragment(item, site, frag);
}

fn apply_item_fragment(item: &mut Item, site: &Site, frag: &Fragment) {
    match &mut item.kind {
        ItemKind::Fn { body, .. } | ItemKind::Event { body, .. } => {
            if let crate::ast::Body::Block(b) = body {
                apply_block(b, site, frag);
            }
        }
        ItemKind::Chain { stmts, .. } => {
            if site.is_stmt {
                if let Fragment::Stmts(ss) = frag {
                    replace_matching_stmt(stmts, ss.clone());
                }
            } else if let Fragment::Expr(e) = frag {
                for s in stmts.iter_mut() {
                    if replace_expr_in_stmt(s, e) {
                        return;
                    }
                }
            }
        }
        ItemKind::Impl { items, .. } | ItemKind::Trait { items, .. } => {
            for it in items.iter_mut() {
                apply_item_fragment(it, site, frag);
            }
        }
        ItemKind::Const { init, .. } | ItemKind::Static { init, .. } => {
            if let (Fragment::Expr(e), false) = (frag, site.is_stmt) {
                *init = Some(e.clone());
            }
        }
        _ => {}
    }
}

fn apply_block(b: &mut Block, site: &Site, frag: &Fragment) {
    if site.is_stmt {
        if let Fragment::Stmts(ss) = frag {
            if replace_matching_stmt(&mut b.stmts, ss.clone()) {
                return;
            }
        }
        if let Fragment::Expr(e) = frag {
            let new = vec![mk_expr_stmt(e.clone())];
            replace_matching_stmt(&mut b.stmts, new);
        }
        return;
    }
    if let Fragment::Expr(e) = frag {
        if let Some(t) = &mut b.tail {
            if replace_macro_call(t, e) {
                return;
            }
        }
        for s in b.stmts.iter_mut() {
            if replace_expr_in_stmt(s, e) {
                return;
            }
        }
    }
}

fn mk_expr_stmt(e: Expr) -> Stmt {
    let span = e.span;
    Stmt {
        kind: StmtKind::Expr { expr: e, linkage: Vec::new() },
        attrs: Vec::new(),
        docs: Default::default(),
        span,
        terminated: true,
    }
}

fn stmt_is_call(s: &Stmt) -> bool {
    matches!(&s.kind, StmtKind::Expr { expr, .. } if matches!(&expr.kind, ExprKind::MacroCall { .. }))
}

/// Replaces the first statement-level macro call with `new`, searching nested blocks in document
/// order (deterministic).
fn replace_matching_stmt(stmts: &mut Vec<Stmt>, new: Vec<Stmt>) -> bool {
    if let Some(pos) = stmts.iter().position(stmt_is_call) {
        stmts.splice(pos..pos + 1, new);
        return true;
    }
    for s in stmts.iter_mut() {
        let hit = match &mut s.kind {
            StmtKind::If { then, else_, .. } => {
                if replace_matching_stmt(&mut then.stmts, new.clone()) {
                    true
                } else if let Some(eb) = else_ {
                    match &mut **eb {
                        ElseBranch::Block(b) => replace_matching_stmt(&mut b.stmts, new.clone()),
                        ElseBranch::If(inner) => {
                            if stmt_is_call(inner) {
                                *inner = new.first().cloned().unwrap_or_else(|| inner.clone());
                                true
                            } else {
                                false
                            }
                        }
                    }
                } else {
                    false
                }
            }
            StmtKind::While { body, .. }
            | StmtKind::Loop { body, .. }
            | StmtKind::For { body, .. }
            | StmtKind::Unsafe(body)
            | StmtKind::Defer(body) => replace_matching_stmt(&mut body.stmts, new.clone()),
            StmtKind::Match { arms, .. } => {
                let mut hit = false;
                for a in arms.iter_mut() {
                    if hit {
                        break;
                    }
                    if let ArmBody::Block(b) = &mut a.body {
                        hit = replace_matching_stmt(&mut b.stmts, new.clone());
                    }
                }
                hit
            }
            _ => false,
        };
        if hit {
            return true;
        }
    }
    false
}

fn replace_expr_in_stmt(s: &mut Stmt, e: &Expr) -> bool {
    match &mut s.kind {
        StmtKind::Expr { expr, .. } => replace_macro_call(expr, e),
        StmtKind::Let { init, .. } => init.as_mut().map_or(false, |i| replace_macro_call(i, e)),
        StmtKind::Assign { target, value, .. } => {
            replace_macro_call(target, e) || replace_macro_call(value, e)
        }
        StmtKind::If { cond, then, else_ } => {
            if replace_macro_call(cond, e) || replace_expr_in_block(then, e) {
                return true;
            }
            if let Some(eb) = else_ {
                match &mut **eb {
                    ElseBranch::Block(b) => {
                        if replace_expr_in_block(b, e) {
                            return true;
                        }
                    }
                    ElseBranch::If(inner) => {
                        if replace_expr_in_stmt(inner, e) {
                            return true;
                        }
                    }
                }
            }
            false
        }
        StmtKind::While { cond, body } => replace_macro_call(cond, e) || replace_expr_in_block(body, e),
        StmtKind::Loop { body, .. } | StmtKind::Unsafe(body) | StmtKind::Defer(body) => replace_expr_in_block(body, e),
        StmtKind::For { iter, body, .. } => replace_macro_call(iter, e) || replace_expr_in_block(body, e),
        StmtKind::Match { scrut, arms } => {
            if replace_macro_call(scrut, e) {
                return true;
            }
            for a in arms.iter_mut() {
                if let Some(g) = &mut a.guard {
                    if replace_macro_call(g, e) {
                        return true;
                    }
                }
                match &mut a.body {
                    ArmBody::Expr(x) => {
                        if replace_macro_call(x, e) {
                            return true;
                        }
                    }
                    ArmBody::Block(b) => {
                        if replace_expr_in_block(b, e) {
                            return true;
                        }
                    }
                }
            }
            false
        }
        StmtKind::Return { value } | StmtKind::Break { value, .. } => {
            value.as_mut().map_or(false, |v| replace_macro_call(v, e))
        }
        StmtKind::ConstAssert { cond, .. } => replace_macro_call(cond, e),
        StmtKind::NestedItem(it) => {
            let fake_site = Site {
                order: 0,
                item_idx: 0,
                name: String::new(),
                input: Vec::new(),
                call_span: it.span,
                replace_span: it.span,
                is_stmt: false,
            };
            apply_item_fragment(it, &fake_site, &Fragment::Expr(e.clone()));
            false
        }
        _ => false,
    }
}

fn replace_expr_in_block(b: &mut Block, e: &Expr) -> bool {
    if let Some(t) = &mut b.tail {
        if replace_macro_call(t, e) {
            return true;
        }
    }
    for s in b.stmts.iter_mut() {
        if replace_expr_in_stmt(s, e) {
            return true;
        }
    }
    false
}

/// Replaces the first `MacroCall` node in the expression tree.
fn replace_macro_call(e: &mut Expr, repl: &Expr) -> bool {
    if matches!(&e.kind, ExprKind::MacroCall { .. }) {
        *e = repl.clone();
        return true;
    }
    match &mut e.kind {
        ExprKind::Unary { expr, .. } => replace_macro_call(expr, repl),
        ExprKind::Binary { lhs, rhs, .. } => {
            replace_macro_call(lhs, repl) || replace_macro_call(rhs, repl)
        }
        ExprKind::Assign { target, value, .. } => {
            replace_macro_call(target, repl) || replace_macro_call(value, repl)
        }
        ExprKind::Call { callee, args } => {
            if replace_macro_call(callee, repl) {
                return true;
            }
            for a in args.iter_mut() {
                if replace_macro_call(&mut a.expr, repl) {
                    return true;
                }
            }
            false
        }
        ExprKind::Method { recv, args, .. } => {
            if replace_macro_call(recv, repl) {
                return true;
            }
            for a in args.iter_mut() {
                if replace_macro_call(&mut a.expr, repl) {
                    return true;
                }
            }
            false
        }
        ExprKind::Field { base, .. } => replace_macro_call(base, repl),
        ExprKind::Index { base, index } => {
            replace_macro_call(base, repl) || replace_macro_call(index, repl)
        }
        ExprKind::Tuple(v) | ExprKind::Array(v) => {
            for x in v.iter_mut() {
                if replace_macro_call(x, repl) {
                    return true;
                }
            }
            false
        }
        ExprKind::ArrayRepeat { value, len } => {
            replace_macro_call(value, repl) || replace_macro_call(len, repl)
        }
        ExprKind::StructLit { fields, rest, .. } => {
            for (_, v) in fields.iter_mut() {
                if replace_macro_call(v, repl) {
                    return true;
                }
            }
            if let Some(r) = rest {
                return replace_macro_call(r, repl);
            }
            false
        }
        ExprKind::If { cond, then, else_ } => {
            if replace_macro_call(cond, repl) || replace_expr_in_block(then, repl) {
                return true;
            }
            if let Some(eb) = else_ {
                match &mut **eb {
                    ElseBranch::Block(b) => {
                        if replace_expr_in_block(b, repl) {
                            return true;
                        }
                    }
                    ElseBranch::If(inner) => {
                        if replace_expr_in_stmt(inner, repl) {
                            return true;
                        }
                    }
                }
            }
            false
        }
        ExprKind::Match { scrut, arms } => {
            if replace_macro_call(scrut, repl) {
                return true;
            }
            for a in arms.iter_mut() {
                if let Some(g) = &mut a.guard {
                    if replace_macro_call(g, repl) {
                        return true;
                    }
                }
                match &mut a.body {
                    ArmBody::Expr(x) => {
                        if replace_macro_call(x, repl) {
                            return true;
                        }
                    }
                    ArmBody::Block(b) => {
                        if replace_expr_in_block(b, repl) {
                            return true;
                        }
                    }
                }
            }
            false
        }
        ExprKind::Block(b) | ExprKind::Unsafe(b) => replace_expr_in_block(b, repl),
        ExprKind::Closure { body, .. } => replace_expr_in_block(body, repl),
        ExprKind::Ref { expr, .. } | ExprKind::Deref { expr } | ExprKind::Try { expr } | ExprKind::Move { expr } => {
            replace_macro_call(expr, repl)
        }
        ExprKind::Cast { expr, .. } | ExprKind::Is { expr, .. } => replace_macro_call(expr, repl),
        ExprKind::Range { lo, hi, .. } => {
            if let Some(l) = lo {
                if replace_macro_call(l, repl) {
                    return true;
                }
            }
            if let Some(h) = hi {
                if replace_macro_call(h, repl) {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

// ---------------------------------------------------------------- hygiene leak check (§6.3)

/// A macro-introduced name that user code still references — and that the user never bound — is a
/// scope leak the user can observe (`E-0400`, UB-015). Because every macro-local identifier was
/// rustled, such a reference has no binding left; the check is syntactic by design (stage [2]
/// runs before name resolution — §6.5).
fn check_hygiene_leaks(
    pf: &ParsedFile,
    fid: FileId,
    sites: &[&MacroSite],
    sources: &Sources,
    handler: &mut Handler,
) {
    if sites.is_empty() {
        return;
    }
    let mut introduced: Vec<String> = Vec::new();
    for s in sites {
        introduced.extend(s.introduced.iter().cloned());
    }
    introduced.sort();
    introduced.dedup();
    if introduced.is_empty() {
        return;
    }
    for item in &pf.items {
        let body = match &item.kind {
            ItemKind::Fn { body, .. } | ItemKind::Event { body, .. } => body,
            _ => continue,
        };
        let crate::ast::Body::Block(b) = body else { continue };
        let mut bindings: Vec<String> = Vec::new();
        bound_names_in_block(b, &mut bindings);
        let mut refs: Vec<(String, Span)> = Vec::new();
        ident_refs_in_block(b, &mut refs);
        refs.sort_by_key(|(n, sp)| (sp.start, n.clone()));
        for (r, span) in refs {
            if introduced.iter().any(|i| *i == r) && !bindings.iter().any(|x| *x == r) {
                let pos = sources.line_col(span).0;
                handler.emit(
                    Diagnostic::new("E-0400", Phase::Macro, fid, pos)
                        .titled("宏作用域泄漏")
                        .note(format!("标识符 `{r}` 由宏引入且对外不可见（§6.3）"))
                        .help("将该标识符作为 :ident 片段从调用点传入，或在调用点重新绑定")
                        .expansion(format!("hygiene → {}:{}", display_path(sources, fid), pos.line)),
                );
                return; // one leak diagnostic per file keeps output deterministic
            }
        }
    }
}

fn display_path(sources: &Sources, f: FileId) -> String {
    sources.get(f).map(|x| x.display_path.clone()).unwrap_or_else(|| format!("file{}", f))
}

fn bound_names_in_block(b: &Block, out: &mut Vec<String>) {
    for s in &b.stmts {
        bound_names_in_stmt(s, out);
    }
}

fn bound_names_in_stmt(s: &Stmt, out: &mut Vec<String>) {
    match &s.kind {
        StmtKind::Let { name, .. } => out.push(name.clone()),
        StmtKind::If { then, else_, .. } => {
            bound_names_in_block(then, out);
            if let Some(eb) = else_ {
                match &**eb {
                    ElseBranch::Block(b) => bound_names_in_block(b, out),
                    ElseBranch::If(inner) => bound_names_in_stmt(inner, out),
                }
            }
        }
        StmtKind::While { body, .. } | StmtKind::Loop { body, .. } | StmtKind::Unsafe(body) | StmtKind::Defer(body) => {
            bound_names_in_block(body, out);
        }
        StmtKind::For { pat, body, .. } => {
            bound_names_in_pat(pat, out);
            bound_names_in_block(body, out);
        }
        StmtKind::Match { arms, .. } => {
            for a in arms {
                bound_names_in_pat(&a.pat, out);
                if let ArmBody::Block(b) = &a.body {
                    bound_names_in_block(b, out);
                }
            }
        }
        StmtKind::NestedItem(it) => {
            if let ItemKind::Fn { sig, .. } = &it.kind {
                for p in &sig.params {
                    out.push(p.name.clone());
                }
            }
        }
        _ => {}
    }
}

fn bound_names_in_pat(p: &Pat, out: &mut Vec<String>) {
    match &p.kind {
        crate::ast::PatKind::Binding { name, sub, .. } => {
            out.push(name.clone());
            if let Some(s) = sub {
                bound_names_in_pat(s, out);
            }
        }
        crate::ast::PatKind::Tuple(v) | crate::ast::PatKind::Or(v) | crate::ast::PatKind::Exclude(v) => {
            for x in v {
                bound_names_in_pat(x, out);
            }
        }
        crate::ast::PatKind::Slice(v, rest) => {
            for x in v {
                bound_names_in_pat(x, out);
            }
            if let Some(r) = rest {
                bound_names_in_pat(r, out);
            }
        }
        crate::ast::PatKind::Constructor { args, fields, .. } => {
            for x in args {
                bound_names_in_pat(x, out);
            }
            for (_, x) in fields {
                bound_names_in_pat(x, out);
            }
        }
        _ => {}
    }
}

fn ident_refs_in_block(b: &Block, out: &mut Vec<(String, Span)>) {
    for s in &b.stmts {
        ident_refs_in_stmt(s, out);
    }
    if let Some(t) = &b.tail {
        ident_refs_in_expr(t, out);
    }
}

fn ident_refs_in_stmt(s: &Stmt, out: &mut Vec<(String, Span)>) {
    match &s.kind {
        StmtKind::Expr { expr, .. } => ident_refs_in_expr(expr, out),
        StmtKind::Let { init, .. } => {
            if let Some(e) = init {
                ident_refs_in_expr(e, out);
            }
        }
        StmtKind::Assign { target, value, .. } => {
            ident_refs_in_expr(target, out);
            ident_refs_in_expr(value, out);
        }
        StmtKind::If { cond, then, else_ } => {
            ident_refs_in_expr(cond, out);
            ident_refs_in_block(then, out);
            if let Some(eb) = else_ {
                match &**eb {
                    ElseBranch::Block(b) => ident_refs_in_block(b, out),
                    ElseBranch::If(inner) => ident_refs_in_stmt(inner, out),
                }
            }
        }
        StmtKind::While { cond, body } => {
            ident_refs_in_expr(cond, out);
            ident_refs_in_block(body, out);
        }
        StmtKind::Loop { body, .. } | StmtKind::Unsafe(body) | StmtKind::Defer(body) => ident_refs_in_block(body, out),
        StmtKind::For { iter, body, .. } => {
            ident_refs_in_expr(iter, out);
            ident_refs_in_block(body, out);
        }
        StmtKind::Match { scrut, arms } => {
            ident_refs_in_expr(scrut, out);
            for a in arms {
                if let Some(g) = &a.guard {
                    ident_refs_in_expr(g, out);
                }
                match &a.body {
                    ArmBody::Expr(e) => ident_refs_in_expr(e, out),
                    ArmBody::Block(b) => ident_refs_in_block(b, out),
                }
            }
        }
        StmtKind::Return { value } | StmtKind::Break { value, .. } => {
            if let Some(v) = value {
                ident_refs_in_expr(v, out);
            }
        }
        StmtKind::ConstAssert { cond, .. } => ident_refs_in_expr(cond, out),
        _ => {}
    }
}

fn ident_refs_in_expr(e: &Expr, out: &mut Vec<(String, Span)>) {
    let skip = |s: &str| s.starts_with('_');
    match &e.kind {
        ExprKind::Ident(s) => {
            if !skip(s) {
                out.push((s.clone(), e.span));
            }
        }
        ExprKind::Path(p) => {
            if p.len() == 1 {
                if let Some(seg) = p.first() {
                    if !skip(seg) {
                        out.push((seg.clone(), e.span));
                    }
                }
            }
        }
        ExprKind::Unary { expr, .. } => ident_refs_in_expr(expr, out),
        ExprKind::Binary { lhs, rhs, .. } => {
            ident_refs_in_expr(lhs, out);
            ident_refs_in_expr(rhs, out);
        }
        ExprKind::Assign { target, value, .. } => {
            ident_refs_in_expr(target, out);
            ident_refs_in_expr(value, out);
        }
        ExprKind::Call { callee, args } => {
            ident_refs_in_expr(callee, out);
            for a in args {
                ident_refs_in_expr(&a.expr, out);
            }
        }
        ExprKind::Method { recv, args, .. } => {
            ident_refs_in_expr(recv, out);
            for a in args {
                ident_refs_in_expr(&a.expr, out);
            }
        }
        ExprKind::Field { base, .. } => ident_refs_in_expr(base, out),
        ExprKind::Index { base, index } => {
            ident_refs_in_expr(base, out);
            ident_refs_in_expr(index, out);
        }
        ExprKind::Tuple(v) | ExprKind::Array(v) => {
            for x in v {
                ident_refs_in_expr(x, out);
            }
        }
        ExprKind::ArrayRepeat { value, len } => {
            ident_refs_in_expr(value, out);
            ident_refs_in_expr(len, out);
        }
        ExprKind::StructLit { fields, rest, .. } => {
            for (_, v) in fields {
                ident_refs_in_expr(v, out);
            }
            if let Some(r) = rest {
                ident_refs_in_expr(r, out);
            }
        }
        ExprKind::If { cond, then, else_ } => {
            ident_refs_in_expr(cond, out);
            ident_refs_in_block(then, out);
            if let Some(eb) = else_ {
                match &**eb {
                    ElseBranch::Block(b) => ident_refs_in_block(b, out),
                    ElseBranch::If(inner) => ident_refs_in_stmt(inner, out),
                }
            }
        }
        ExprKind::Match { scrut, arms } => {
            ident_refs_in_expr(scrut, out);
            for a in arms {
                if let Some(g) = &a.guard {
                    ident_refs_in_expr(g, out);
                }
                match &a.body {
                    ArmBody::Expr(x) => ident_refs_in_expr(x, out),
                    ArmBody::Block(b) => ident_refs_in_block(b, out),
                }
            }
        }
        ExprKind::Block(b) | ExprKind::Unsafe(b) => ident_refs_in_block(b, out),
        ExprKind::Closure { body, .. } => ident_refs_in_block(body, out),
        ExprKind::Ref { expr, .. } | ExprKind::Deref { expr } | ExprKind::Try { expr } | ExprKind::Move { expr } => {
            ident_refs_in_expr(expr, out)
        }
        ExprKind::Cast { expr, .. } | ExprKind::Is { expr, .. } => ident_refs_in_expr(expr, out),
        ExprKind::Range { lo, hi, .. } => {
            if let Some(l) = lo {
                ident_refs_in_expr(l, out);
            }
            if let Some(h) = hi {
                ident_refs_in_expr(h, out);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- printing (byte-stable)

pub(crate) fn punct_text(p: Punct) -> &'static str {
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

pub(crate) fn token_text(t: &Tok) -> String {
    if !t.text.is_empty() {
        return t.text.clone();
    }
    match &t.kind {
        Kind::Ident(s) => s.clone(),
        Kind::Keyword(k) => k.text().to_string(),
        Kind::Punct(p) => punct_text(*p).to_string(),
        Kind::Lifetime(l) => format!("'{}", l),
        Kind::EmbedLang(l) => format!("@{}", l),
        _ => String::new(),
    }
}

pub(crate) fn render_toks(toks: &[Tok]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for t in toks {
        if t.is_eof() {
            continue;
        }
        parts.push(token_text(t));
    }
    parts.join(" ")
}

pub(crate) fn render_trees(trees: &[Tree]) -> String {
    let toks = flatten_trees(trees);
    render_toks(&toks)
}

fn flatten_trees(trees: &[Tree]) -> Vec<Tok> {
    let mut toks = Vec::new();
    for t in trees {
        t.flatten(&mut toks);
    }
    toks
}

// ---------------------------------------------------------------- §6.8 render_expanded

/// `infy expand`: reprint of the post-expansion `.infy` source with `// expand-from: <file:line>`
/// provenance comments. Byte-stable for identical input (§6.8, normative): every iteration runs
/// over `BTreeMap`s or sorted vectors; no HashMap, no time, no addresses.
pub fn render_expanded(
    files: &BTreeMap<FileId, ParsedFile>,
    sources: &Sources,
    opts: &ExpandOptions,
) -> String {
    let mut work = files.clone();
    let mut handler = Handler::default();
    let result = expand_all(&mut work, sources, &mut handler, opts);

    let mut edits: BTreeMap<FileId, Vec<(u32, u32, String)>> = BTreeMap::new();
    for s in &result.sites {
        let line = sources.line_col(s.call_span).0.line;
        let text = format!("// expand-from: {}:{}\n{}", display_path(sources, s.file), line, s.text);
        edits.entry(s.file).or_default().push((s.replace_span.start, s.replace_span.end, text));
    }
    for d in &result.derives {
        let text = format!("\n// expand-from: {}:{}\n{}", display_path(sources, d.file), d.line, d.text);
        edits.entry(d.file).or_default().push((d.insert_at, d.insert_at, text));
    }

    let mut out = String::new();
    for (fid, _pf) in files.iter() {
        let Some(sf) = sources.get(*fid) else { continue };
        let text = &sf.text;
        let mut es = edits.get(fid).cloned().unwrap_or_default();
        es.sort_by_key(|(st, en, _)| (*st, *en));
        let mut buf = String::new();
        let mut cur = 0usize;
        for (st, en, rep) in es {
            let st = (st as usize).min(text.len());
            let en = (en as usize).min(text.len()).max(st);
            if st < cur {
                continue; // nested site: the outer replacement already printed it
            }
            buf.push_str(&text[cur..st]);
            buf.push_str(&rep);
            cur = en;
        }
        buf.push_str(&text[cur..]);
        out.push_str(&format!("// ===== {} =====\n", sf.display_path));
        out.push_str(&buf);
    }
    out
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Sources;
    use crate::ast::{Docs, MacroArm, Visibility};

    // NOTE (stage-[2] tests): the current WIP `parser::tt_seq` consumes the whole token stream
    // when building `macro_rules!` arms (a parser.rs bug — reported, not fixed here: the file
    // belongs to another engineer). The harness therefore rebuilds macro definition items from
    // the real lexer's token stream with correct group boundaries, exercising *this* module's
    // matcher/expander logic end-to-end exactly as a fixed parser would hand it over.

    fn group_end(toks: &[Tok], open: usize) -> usize {
        // index of the matching closer for the opener at `open`
        let mut depth = 0usize;
        let mut i = open;
        while i < toks.len() {
            match &toks[i].kind {
                Kind::Punct(Punct::LParen) | Kind::Punct(Punct::LBracket) | Kind::Punct(Punct::LBrace) => depth += 1,
                Kind::Punct(Punct::RParen) | Kind::Punct(Punct::RBracket) | Kind::Punct(Punct::RBrace) => {
                    depth -= 1;
                    if depth == 0 {
                        return i;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        toks.len().saturating_sub(1)
    }

    fn is_opener(k: &Kind) -> bool {
        matches!(k, Kind::Punct(Punct::LParen) | Kind::Punct(Punct::LBracket) | Kind::Punct(Punct::LBrace))
    }

    fn macro_regions(toks: &[Tok]) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut i = 0usize;
        while i < toks.len() {
            if toks[i].is_kw(Kw::MacroRules) {
                let start = i;
                let mut j = i + 1;
                if j < toks.len() && toks[j].ident().is_some() {
                    j += 1;
                }
                if j < toks.len() && matches!(toks[j].kind, Kind::Punct(Punct::LBrace)) {
                    j = group_end(toks, j) + 1;
                }
                if j < toks.len() && toks[j].is_p(Punct::Semi) {
                    j += 1;
                }
                out.push((start, j));
                i = j;
                continue;
            }
            i += 1;
        }
        out
    }

    fn rebuild_macro_items(toks: &[Tok]) -> Vec<Item> {
        let mut items = Vec::new();
        for (start, end) in macro_regions(toks) {
            let region = &toks[start..end];
            let name = region.get(1).and_then(|t| t.ident()).unwrap_or("").to_string();
            let mut arms: Vec<MacroArm> = Vec::new();
            let mut p = 2usize; // past `macro_rules! name`
            if p < region.len() && matches!(region[p].kind, Kind::Punct(Punct::LBrace)) {
                p += 1;
            }
            while p < region.len() {
                if matches!(region[p].kind, Kind::Punct(Punct::RBrace)) {
                    break;
                }
                let arm_start = p;
                // matcher: trees up to top-level `=>`
                let mut matcher: Vec<Tree> = Vec::new();
                while p < region.len() && !region[p].is_p(Punct::FatArrow) {
                    if is_opener(&region[p].kind) {
                        let ge = group_end(region, p);
                        let (trees, _n) = crate::lexer::build_token_tree(&region[p..=ge]);
                        matcher.extend(trees);
                        p = ge + 1;
                    } else {
                        matcher.push(Tree::Leaf(region[p].clone()));
                        p += 1;
                    }
                }
                if p >= region.len() || !region[p].is_p(Punct::FatArrow) {
                    break;
                }
                p += 1; // `=>`
                // expander: `=` | group | leaf-expr group
                let mut expander: Vec<Tree> = Vec::new();
                if p < region.len() && region[p].is_p(Punct::Eq) {
                    p += 1;
                }
                if p < region.len() && is_opener(&region[p].kind) {
                    let ge = group_end(region, p);
                    let (trees, _n) = crate::lexer::build_token_tree(&region[p..=ge]);
                    expander = trees;
                    p = ge + 1;
                }
                let sep = if p < region.len() && region[p].is_p(Punct::Semi) {
                    p += 1;
                    Some(Punct::Semi)
                } else if p < region.len() && region[p].is_p(Punct::Comma) {
                    p += 1;
                    Some(Punct::Comma)
                } else {
                    None
                };
                let span = Span::new(
                    region[arm_start].span.file,
                    region[arm_start].span.start,
                    region[(p - 1).min(region.len() - 1)].span.end,
                );
                arms.push(MacroArm { matcher, expander, separator: sep, span });
            }
            let span = Span::new(
                region[0].span.file,
                region[0].span.start,
                region[region.len() - 1].span.end,
            );
            items.push(Item {
                attrs: Vec::new(),
                docs: Docs::default(),
                vis: Visibility::Private,
                span,
                kind: ItemKind::MacroRules { name, arms },
            });
        }
        items
    }

    pub(crate) fn harness(texts: &[(&str, &str)]) -> (Sources, BTreeMap<FileId, ParsedFile>, Handler) {
        let mut src = Sources::default();
        let mut files = BTreeMap::new();
        let mut handler = Handler::default();
        for (path, text) in texts {
            let f = src.add(path.to_string(), text.to_string());
            let lexed = crate::lexer::lex_file(&src, f, &mut handler);
            let regions = macro_regions(&lexed.toks);
            let mut clean: Vec<Tok> = Vec::new();
            let mut skip: Vec<(usize, usize)> = regions.clone();
            skip.sort();
            let mut ri = 0usize;
            let mut skipping = false;
            for (i, t) in lexed.toks.iter().enumerate() {
                if !skipping && ri < skip.len() && i == skip[ri].0 {
                    skipping = true;
                }
                if skipping {
                    if ri < skip.len() && i + 1 >= skip[ri].1 {
                        skipping = false;
                        ri += 1;
                    }
                    continue;
                }
                clean.push(t.clone());
            }
            let clean_lexed = crate::lexer::Lexed {
                toks: clean,
                comments: lexed.comments.clone(),
                shebang: lexed.shebang.clone(),
                build_constraint: lexed.build_constraint,
                diagnostics_emitted: lexed.diagnostics_emitted,
            };
            let mut pf = crate::parser::parse(&src, f, &clean_lexed, &mut handler);
            let mut macros = rebuild_macro_items(&lexed.toks);
            macros.sort_by_key(|m| m.span.start);
            for m in macros {
                let pos = pf
                    .items
                    .iter()
                    .position(|it| it.span.start > m.span.start)
                    .unwrap_or(pf.items.len());
                pf.items.insert(pos, m);
            }
            files.insert(f, pf);
        }
        // Lex/parse noise belongs to the owning stages; macro tests assert stage [2] only.
        handler.diags.clear();
        (src, files, handler)
    }

    /// All `let`/`var` binding names anywhere inside an item (test helper).
    fn all_let_names(item: &Item, out: &mut Vec<String>) {
        match &item.kind {
            ItemKind::Fn { body, .. } | ItemKind::Event { body, .. } => {
                if let crate::ast::Body::Block(b) = body {
                    block_let_names(b, out);
                }
            }
            _ => {}
        }
    }
    fn block_let_names(b: &Block, out: &mut Vec<String>) {
        for s in &b.stmts {
            if let StmtKind::Let { name, init, .. } = &s.kind {
                out.push(name.clone());
                if let Some(e) = init {
                    expr_let_names(e, out);
                }
            } else if let Some(e) = match &s.kind {
                StmtKind::Expr { expr, .. } => Some(expr),
                _ => None,
            } {
                expr_let_names(e, out);
            }
        }
        if let Some(t) = &b.tail {
            expr_let_names(t, out);
        }
    }
    fn expr_let_names(e: &Expr, out: &mut Vec<String>) {
        if let ExprKind::Block(b) = &e.kind {
            block_let_names(b, out);
        }
    }

    pub(crate) fn run(
        src: &Sources,
        files: &mut BTreeMap<FileId, ParsedFile>,
        handler: &mut Handler,
        opts: &ExpandOptions,
    ) -> ExpansionResult {
        expand_all(files, src, handler, opts)
    }

    pub(crate) fn codes(handler: &Handler) -> Vec<String> {
        handler.diags.iter().map(|d| d.code.clone()).collect()
    }

    // NOTE: the WIP parser merges the statement *after* a macro-call statement into it, so tests
    // that need multiple call sites put them in separate functions (each body holds one call).
    pub(crate) const VEC_SRC: &str = r#"
macro_rules! vec {
    ()               => { [] };
    ($x:expr)         => { [$x] };
    ($x:expr, $($y:expr),+ $(,)?) => {
        var arr = [$x];
        $(arr.push($y);)*
        arr
    };
}

fn fa() {
    let a = vec![];
}
fn fb() {
    let b = vec![7];
}
fn fc() {
    let c = vec![1, 2, 3];
}
"#;

    // ---------------------------------------------------------------- vec! (§6.1)

    #[test]
    fn vec_empty_arm_expands_to_array_literal() {
        let (src, mut files, mut handler) = harness(&[("m/main.infy", VEC_SRC)]);
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(codes(&handler).is_empty(), "unexpected diags: {:?}", codes(&handler));
        assert_eq!(r.expansions, 3);
        assert!(r.sites[0].text.contains("[ ]"), "site0 text: {}", r.sites[0].text);
    }

    #[test]
    fn vec_single_arm_expands_to_bracket() {
        let (src, mut files, mut handler) = harness(&[("m/main.infy", VEC_SRC)]);
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(r.sites[1].text.contains("[ 7 ]"), "site1 text: {}", r.sites[1].text);
    }

    #[test]
    fn vec_repetition_with_trailing_comma_idiom() {
        let (src, mut files, mut handler) = harness(&[("m/main.infy", VEC_SRC)]);
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        let t = &r.sites[2].text;
        assert!(t.contains("var"), "{t}");
        assert!(t.contains("[ 1 ]"), "{t}");
        assert!(t.contains("push ( 2 )"), "{t}");
        assert!(t.contains("push ( 3 )"), "{t}");
        assert!(!t.trim_end().ends_with(','), "stray trailing comma: {t}");
    }

    #[test]
    fn vec_repetition_accepts_trailing_comma_in_call() {
        let text = r#"
macro_rules! vec {
    ($x:expr, $($y:expr),+ $(,)?) => {
        var arr = [$x];
        $(arr.push($y);)*
        arr
    };
}
fn main() {
    let c = vec![1, 2, 3,];
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert_eq!(r.expansions, 1);
        assert!(r.sites[0].text.contains("push ( 3 )"));
    }

    #[test]
    fn vec_expr_position_wraps_multi_stmt_expansion_in_block() {
        let (src, mut files, mut handler) = harness(&[("m/main.infy", VEC_SRC)]);
        run(&src, &mut files, &mut handler, &ExpandOptions::default());
        let pf = &files.values().next().unwrap();
        // `var c = vec![1,2,3]` became `var c = { … }` — a block expression containing the `let`.
        let found = pf.items.iter().any(|it| {
            if let ItemKind::Fn { body, .. } = &it.kind {
                if let crate::ast::Body::Block(b) = body {
                    return b.stmts.iter().any(|s| match &s.kind {
                        StmtKind::Let { init: Some(e), .. } => matches!(
                            &e.kind,
                            ExprKind::Block(bl) if bl.stmts.iter().any(|st| matches!(&st.kind, StmtKind::Let { .. }))
                        ),
                        _ => false,
                    });
                }
            }
            false
        });
        assert!(found, "expansion was not re-parsed into a block expression");
    }

    #[test]
    fn macro_stmt_expansion_lands_rustled_lets_in_the_body() {
        // `emit!(41 + 1);` — the multi-statement product must be re-parsed into AST statements
        // (spliced in statement position or as a block, per the parser's statement shape).
        let text = r#"
macro_rules! emit {
    ($x:expr) => { let v = $x; v; };
}
fn main() {
    emit!(41 + 1);
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert_eq!(r.sites.len(), 1);
        assert!(r.sites[0].text.contains("let"), "{}", r.sites[0].text);
        let mut names: Vec<String> = Vec::new();
        for it in &files.values().next().unwrap().items {
            all_let_names(it, &mut names);
        }
        assert!(
            names.iter().any(|n| n.starts_with("__infy_m")),
            "rustled `let` missing from body AST: {names:?}"
        );
    }

    // ---------------------------------------------------------------- classifiers (§6.2)

    fn classifier_case(
        macro_src: &str,
        call: &str,
    ) -> (Sources, BTreeMap<FileId, ParsedFile>, Handler, ExpansionResult) {
        let text = format!("{}\nfn main() {{\n    let z = {};\n}}\n", macro_src, call);
        let (src, mut files, mut handler) = harness(&[("m/main.infy", &text)]);
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        (src, files, handler, r)
    }

    #[test]
    fn classifier_expr_matches_arithmetic() {
        let (_src, _files, handler, r) = classifier_case(
            "macro_rules! m { ($x:expr) => { [$x] }; }",
            "m!(1 + 2)",
        );
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert!(r.sites[0].text.contains("1 + 2"), "{}", r.sites[0].text);
    }

    #[test]
    fn classifier_expr_spanning_group_comma() {
        // the comma inside `g(1, 2)` is nested, so the whole call binds as one :expr
        let (_src, _files, handler, r) = classifier_case(
            "macro_rules! m { ($x:expr) => { [$x] }; }",
            "m!(g(1, 2))",
        );
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert!(r.sites[0].text.contains("g ( 1 , 2 )"), "{}", r.sites[0].text);
    }

    #[test]
    fn classifier_ty_matches_validated_type() {
        // the expander ignores `$t`, but the arm only matches because `u32` re-parses as a legal
        // :ty fragment (§6.2 validation through the real parser).
        let (_src, _files, handler, r) = classifier_case(
            "macro_rules! m { ($t:ty) => { [0u32] }; }",
            "m!(u32)",
        );
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert_eq!(r.expansions, 1);
        assert!(r.sites[0].text.contains("[ 0u32 ]"), "{}", r.sites[0].text);
    }

    #[test]
    fn classifier_ty_rejects_non_type() {
        // `1` is not a type → the :ty bind fails → no arm matches → E-0402 (§6.2/§6.5).
        let (_src, _files, handler, _r) = classifier_case(
            "macro_rules! m { ($t:ty) => { [0u32] }; }",
            "m!(1)",
        );
        assert!(codes(&handler).contains(&"E-0402".to_string()), "{:?}", codes(&handler));
    }

    #[test]
    fn classifier_ident_binds_plain_name() {
        let (_src, _files, handler, r) = classifier_case(
            "macro_rules! m { ($i:ident) => { [$i] }; }",
            "m!(alpha)",
        );
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert!(r.sites[0].text.contains("[ alpha ]"), "{}", r.sites[0].text);
    }

    #[test]
    fn classifier_ident_rejects_primitive_keyword() {
        // `u32` is a keyword, not an identifier → no arm matches → E-0402
        let (_src, _files, handler, _r) = classifier_case(
            "macro_rules! m { ($i:ident) => { [$i] }; }",
            "m!(u32)",
        );
        assert!(codes(&handler).contains(&"E-0402".to_string()), "{:?}", codes(&handler));
    }

    #[test]
    fn classifier_pat_matches_or_pattern() {
        let text = r#"
macro_rules! patmat {
    ($p:pat) => { match (q) { $p => 1u32, _ => 0u32 } };
}
fn main() {
    let z = patmat!(0u32 | 1u32);
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert_eq!(r.expansions, 1);
        assert!(r.sites[0].text.contains("0u32 | 1u32"), "{}", r.sites[0].text);
    }

    #[test]
    fn classifier_block_requires_brace_group() {
        let (_src, _files, handler, r) = classifier_case(
            "macro_rules! m { ($b:block) => { $b }; }",
            "m!({ let x = 1; x })",
        );
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert!(r.sites[0].text.contains("let x = 1 ; x"), "{}", r.sites[0].text);
    }

    #[test]
    fn classifier_tt_matches_any_single_tree() {
        let (_src, _files, handler, r) = classifier_case(
            "macro_rules! m { ($t:tt) => { [$t] }; }",
            "m!(x)",
        );
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert!(r.sites[0].text.contains("[ x ]"), "{}", r.sites[0].text);

        // a whole group is ONE tt
        let (_src, _files, handler, r) = classifier_case(
            "macro_rules! m { ($t:tt) => { [$t] }; }",
            "m!((a + b))",
        );
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert!(r.sites[0].text.contains("( a + b )"), "{}", r.sites[0].text);
    }

    #[test]
    fn classifier_stmt_binds_full_statement() {
        let text = r#"
macro_rules! m {
    ($s:stmt) => { { $s } };
}
fn main() {
    let z = m!(let y = 5;);
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert!(r.sites[0].text.contains("let y = 5 ;"), "{}", r.sites[0].text);
    }

    #[test]
    fn classifier_meta_matches_key_value_meta() {
        // `a = 1` must bind as one :meta fragment or the arm would not match (§6.2).
        let text = r#"
macro_rules! m {
    ($mm:meta) => { [0u32] };
}
fn main() {
    let z = m!(a = 1);
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert_eq!(r.expansions, 1);
        assert!(r.sites[0].text.contains("[ 0u32 ]"), "{}", r.sites[0].text);
    }

    #[test]
    fn classifier_meta_rejects_empty_input() {
        // :meta requires at least one tree (§6.2).
        let text = r#"
macro_rules! m {
    ($mm:meta) => { [0u32] };
}
fn main() {
    let z = m!();
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(codes(&handler).contains(&"E-0402".to_string()), "{:?}", codes(&handler));
    }

    #[test]
    fn classifier_rejects_non_matching_fragment() {
        // `= =` is not an :expr → no arm matches → E-0402
        let (_src, _files, handler, _r) = classifier_case(
            "macro_rules! m { ($x:expr) => { [$x] }; }",
            "m!(= =)",
        );
        assert!(codes(&handler).contains(&"E-0402".to_string()), "{:?}", codes(&handler));
    }

    // ---------------------------------------------------------------- nested repetition

    #[test]
    fn nested_repetition_expands_two_dimensionally() {
        let text = r#"
macro_rules! grid {
    ($($($c:expr),+);+) => { [$( [ $($c),+ ] ),+] };
}
fn main() {
    let z = grid!(1, 2; 3);
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        let t = &r.sites[0].text;
        // rows survive as nested array literals: `[[1, 2], [3]]`
        assert!(t.contains("1 , 2"), "{t}");
        assert!(t.contains("3"), "{t}");
    }

    // ---------------------------------------------------------------- hygiene (§6.3)

    #[test]
    fn macro_local_identifiers_are_rustled() {
        let (src, mut files, mut handler) = harness(&[("m/main.infy", VEC_SRC)]);
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        let t = &r.sites[2].text;
        assert!(t.contains("__infy_m"), "no rustled name in: {t}");
        assert!(!t.contains("var arr "), "macro-local `arr` leaked: {t}");
        assert_eq!(r.sites[2].introduced, vec!["arr".to_string()]);
    }

    #[test]
    fn same_macro_twice_in_one_function_does_not_collide() {
        // the classic `$x` binding-reuse case: two expansions, two rustled name sets.
        let text = r#"
macro_rules! pair {
    ($x:expr) => {
        var arr = [$x];
        var acc = 0u32;
        acc
    };
}
fn f1() {
    let p1 = pair!(1);
}
fn f2() {
    let p2 = pair!(2);
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert_eq!(r.expansions, 2);
        let a = &r.sites[0].text;
        let b = &r.sites[1].text;
        assert!(a.contains("__infy_m") && b.contains("__infy_m"), "{a} | {b}");
        assert_ne!(a, b, "two expansions of one macro must not reuse rustled names");
        // the rustled names really differ
        let na: Vec<&str> = a.split_whitespace().filter(|w| w.starts_with("__infy_m")).collect();
        let nb: Vec<&str> = b.split_whitespace().filter(|w| w.starts_with("__infy_m")).collect();
        assert!(!na.is_empty() && na.iter().all(|x| !nb.contains(x)), "{na:?} vs {nb:?}");
    }

    #[test]
    fn ident_fragment_substitution_keeps_caller_scope() {
        let text = r#"
macro_rules! declare {
    ($n:ident) => {
        var $n: u32 = 42;
        $n
    };
}
fn main() {
    declare!(total);
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        let t = &r.sites[0].text;
        assert!(t.contains("var total"), "{t}");
        assert!(!t.contains("__infy_m"), "caller identifier was captured: {t}");
        assert!(r.sites[0].introduced.is_empty(), "introduced: {:?}", r.sites[0].introduced);
    }

    #[test]
    fn hygiene_leak_of_macro_local_name_is_e0400() {
        let text = r#"
macro_rules! leaky {
    () => {
        var secret: u32 = 7;
        secret
    };
}
fn main() {
    secret = 9;
    let _x = leaky!();
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(codes(&handler).contains(&"E-0400".to_string()), "{:?}", codes(&handler));
        let d = handler.diags.iter().find(|d| d.code == "E-0400").unwrap();
        assert!(d.expand_chain.iter().any(|s| s.contains("hygiene")), "{:?}", d.expand_chain);
    }

    #[test]
    fn user_shadow_of_macro_local_name_is_not_a_leak() {
        let text = r#"
macro_rules! leaky {
    () => {
        var secret: u32 = 7;
        secret
    };
}
fn main() {
    var secret: u32 = 0;
    secret = 1;
    let _x = leaky!();
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(!codes(&handler).contains(&"E-0400".to_string()), "{:?}", codes(&handler));
    }

    // ---------------------------------------------------------------- recursion (§6.4)

    #[test]
    fn recursion_depth_limit_emits_e0401() {
        let text = r#"
macro_rules! boom {
    ($x:expr) => { boom!($x) };
}
fn main() {
    let z = boom!(1);
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        run(&src, &mut files, &mut handler, &ExpandOptions::default());
        let c = codes(&handler);
        assert!(c.contains(&"E-0401".to_string()), "{:?}", c);
        assert_eq!(c.iter().filter(|x| *x == "E-0401").count(), 1, "one diagnostic per runaway: {c:?}");
    }

    #[test]
    fn bounded_recursion_under_the_limit_expands() {
        // two nested expansions — legal under §6.4's cap of 128.
        let text = r#"
macro_rules! wrap {
    ($x:expr) => { [wrap2!($x)] };
}
macro_rules! wrap2 {
    ($x:expr) => { [$x] };
}
fn main() {
    let z = wrap!(5);
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert!(r.sites[0].text.contains("5"), "{}", r.sites[0].text);
        assert!(r.max_depth >= 2, "max_depth {}", r.max_depth);
    }

    // ---------------------------------------------------------------- illegal expansion (§6.5)

    #[test]
    fn expansion_to_illegal_fragment_is_e0402() {
        let text = r#"
macro_rules! bad {
    () => { + };
}
fn main() {
    let z = bad!();
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        run(&src, &mut files, &mut handler, &ExpandOptions::default());
        let c = codes(&handler);
        assert!(c.contains(&"E-0402".to_string()), "{:?}", c);
        let d = handler.diags.iter().find(|d| d.code == "E-0402").unwrap();
        assert!(d.notes.iter().any(|n| n.contains("展开后片段")), "{:?}", d.notes);
        assert!(d.expand_chain.iter().any(|s| s.starts_with("macro bad →")), "{:?}", d.expand_chain);
    }

    // ---------------------------------------------------------------- visibility (§5.5/§6.5)

    #[test]
    fn macro_used_before_definition_is_rejected() {
        let text = r#"
fn main() {
    let z = later!(1);
}
macro_rules! later {
    ($x:expr) => { [$x] };
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(codes(&handler).contains(&"E-0402".to_string()), "{:?}", codes(&handler));
    }

    #[test]
    fn macro_visible_across_package_files_after_definition() {
        let defs = "\nmacro_rules! shared {\n    ($x:expr) => { [$x] };\n}\n";
        let user = "\nfn main() {\n    let z = shared!(3);\n}\n";
        let (src, mut files, mut handler) = harness(&[("m/a.infy", defs), ("m/b.infy", user)]);
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert_eq!(r.expansions, 1);
    }

    // ---------------------------------------------------------------- ExpandOptions (§6.8)

    #[test]
    fn only_macro_filters_expansion() {
        let text = r#"
macro_rules! m1 { ($x:expr) => { [$x] }; }
macro_rules! m2 { ($x:expr) => { [$x] }; }
fn f1() {
    let a = m1!(1);
}
fn f2() {
    let b = m2!(2);
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        let opts = ExpandOptions { only_macro: Some("m1".to_string()), stage: Stage::Macro };
        let r = run(&src, &mut files, &mut handler, &opts);
        assert_eq!(r.expansions, 1);
        assert_eq!(r.sites[0].macro_name, "m1");
    }

    #[test]
    fn stage_derive_skips_macro_expansion() {
        let (src, mut files, mut handler) = harness(&[("m/main.infy", VEC_SRC)]);
        let opts = ExpandOptions { only_macro: None, stage: Stage::Derive };
        let r = run(&src, &mut files, &mut handler, &opts);
        assert_eq!(r.expansions, 0);
        assert!(r.sites.is_empty());
    }

    // ---------------------------------------------------------------- render_expanded (§6.8)

    #[test]
    fn render_expanded_is_byte_stable_and_annotated() {
        let (src, files, _h) = harness(&[("m/main.infy", VEC_SRC)]);
        let opts = ExpandOptions::default();
        let a = render_expanded(&files, &src, &opts);
        let b = render_expanded(&files, &src, &opts);
        assert_eq!(a, b, "render_expanded must be byte-stable (§6.8/§7.3)");
        assert!(a.contains("// expand-from: m/main.infy:"), "{a}");
        assert!(a.contains("macro_rules!"), "definitions must survive: {a}");
        assert!(!a.contains("vec![1, 2, 3]"), "call site should be replaced: {a}");
        assert!(a.contains("push"), "{a}");
    }

    #[test]
    fn render_expanded_stage_macro_has_no_derive_impls() {
        let text = "\n#[derive(Debug)]\nstruct D { x: u32 }\n";
        let (src, files, _h) = harness(&[("m/main.infy", text)]);
        let opts = ExpandOptions { only_macro: None, stage: Stage::Macro };
        let out = render_expanded(&files, &src, &opts);
        assert!(!out.contains("impl Debug for D"), "stage macro must not reprint derives: {out}");
        let opts2 = ExpandOptions { only_macro: None, stage: Stage::All };
        let out2 = render_expanded(&files, &src, &opts2);
        assert!(out2.contains("impl Debug for D"), "stage all must reprint derives: {out2}");
    }

    // ---------------------------------------------------------------- bookkeeping

    #[test]
    fn expansion_result_lists_macro_names_sorted() {
        let (src, mut files, mut handler) = harness(&[("m/main.infy", VEC_SRC)]);
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert_eq!(r.macro_names, vec!["vec".to_string()]);
        assert!(r.max_depth >= 1);
    }
}
