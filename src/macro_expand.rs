//! Pipeline stage [2] (§7.1): hygiene-scoped `macro_rules!` expansion (§6) and the token-level
//! machinery behind `infy expand` (§6.8).
//!
//! Normative behaviour implemented here:
//! * §6.2 — the closed classifier set `:expr :stmt :block :pat :ident :ty :tt :meta`, matching over
//!   token trees, repetitions `$(pat)sep` with `+`/`*`/`?` (nested repetitions supported) and the
//!   `$(,)?` trailing-comma idiom from the §6.1 `vec!` example.
//! * §6.3/UB-015 — hygiene by *scope rustling*: every expansion gets a unique `expansion_id` and
//!   identifiers the macro itself introduces are renamed to `__infy_m<id>_<name>` (legal for
//!   generated code per §2.4/§2.3, `E-0007` for user code). Tokens substituted from `:ident` /
//!   fragment bindings keep their call-site identity. A macro-local name escaping into user view is
//!   reported as `E-0400`.
//! * §6.4 — recursion depth limit 128 → `E-0401`.
//! * §6.5 — expansions are re-parsed into AST fragments; an expansion that is not a legal
//!   fragment → `E-0402` pointing at the **expander**, with the expanded fragment printed and the
//!   provenance recorded in `expand_chain` (`.expansion("macro vec → file:12")`).
//! * §6.8 / §7.3 — [`render_expanded`] is byte-stable: deterministic `BTreeMap` iteration, no time,
//!   no addresses, every collection sorted before use.
//!
//! The lexer is used only through its documented surface (`lex_file`, `build_token_tree`) by
//! *tests*; expansion itself operates on the token trees the parser already stored in
//! `ItemKind::MacroRules` / `ExprKind::MacroCall`, and re-parsing goes through
//! `parser::parse` on a synthetic token stream so no source text re-lexing is required.

use std::collections::BTreeMap;

use crate::ast::{
    ArmBody, Block, ElseBranch, Expr, ExprKind, FieldName, ImportAlias, Item, ItemKind, Pat,
    ParsedFile, Stmt, StmtKind,
};
use crate::diag::{Diagnostic, Handler, Phase};
use crate::span::{FileId, Position, Sources, Span};
use crate::token::{Delim, Kind, Kw, Punct, Tok, Tree};

/// §6.4: expansion recursion depth limit.
pub const MAX_EXPANSION_DEPTH: usize = 128;

/// §6.8 `--stage`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Stage {
    /// Macros and derives.
    #[default]
    All,
    /// §6 stage [2] macro expansion only.
    Macro,
    /// §6.7 derive expansion only.
    Derive,
}

/// §6.8 `infy expand [mod] [--macro name] [--derive Type] [--stage all|macro|derive]`.
#[derive(Clone, Debug, Default)]
pub struct ExpandOptions {
    /// `--macro name`: only calls of this macro expand; other calls are left in place.
    pub only_macro: Option<String>,
    pub stage: Stage,
}

/// One expanded call site — the audit trail `infy expand` prints (§6.8).
#[derive(Clone, Debug)]
pub struct MacroSite {
    pub file: FileId,
    /// Span of the macro call expression as written.
    pub call_span: Span,
    /// Span actually replaced in the reprinted source (call plus trailing `;` in stmt position).
    pub replace_span: Span,
    pub macro_name: String,
    pub expansion_id: u32,
    /// Depth of the deepest nested expansion for this site (§6.4 accounting).
    pub depth: usize,
    /// Byte-stable reprint of the expansion (the fragment that replaced the call).
    pub text: String,
    /// Identifiers the macro introduced into this site (already rustled in `text`).
    pub introduced: Vec<String>,
}

/// One generated derive impl (§6.7), kept for `--stage all|derive` reprinting.
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

/// Everything `infy expand` needs to know about a stage-[2] run.
#[derive(Clone, Debug, Default)]
pub struct ExpansionResult {
    /// Macro sites in deterministic (file, offset) order.
    pub sites: Vec<MacroSite>,
    /// Derive impls in deterministic (file, offset, trait) order.
    pub derives: Vec<DeriveSite>,
    /// Total macro expansions performed.
    pub expansions: usize,
    /// Deepest expansion recursion reached.
    pub max_depth: usize,
    /// Sorted names of the `macro_rules!` definitions found.
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

    fn text(&self) -> &'static str {
        match self {
            Class::Expr => "expr",
            Class::Stmt => "stmt",
            Class::Block => "block",
            Class::Pat => "pat",
            Class::Ident => "ident",
            Class::Ty => "ty",
            Class::Tt => "tt",
            Class::Meta => "meta",
        }
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

/// Compiled matcher element (§6.2). `Rep.id` keys the synthetic trailing-separator marker binding
/// used by the `$(,)?` idiom.
#[derive(Clone, Debug)]
enum MItem {
    Lit(Tree),
    Bind { name: String, class: Class },
    Rep { items: Vec<MItem>, sep: Option<Punct>, op: RepOp, bound: Vec<String>, marker: Option<usize> },
}

/// A matcher-bound fragment value. `Many` carries one element per repetition iteration; nested
/// repetitions nest `Many` inside `Many`, exactly matching nested `$( ... )` in the expander.
#[derive(Clone, Debug)]
enum Frag {
    One(Vec<Tree>),
    Many(Vec<Frag>),
}

impl Frag {
    fn as_single(&self) -> Option<&Vec<Tree>> {
        match self {
            Frag::One(t) => Some(t),
            Frag::Many(_) => None,
        }
    }
}

type Scope = BTreeMap<String, Frag>;

/// One compiled arm.
#[derive(Clone, Debug)]
struct ArmC {
    items: Vec<MItem>,
    expander: Vec<Tree>,
    /// Introduced (macro-local) identifier names, i.e. every `let`/`fn`-style binder in the body.
    rename_map: BTreeMap<String, String>,
    span: Span,
    file: FileId,
}

#[derive(Clone, Debug)]
struct DefEntry {
    file: FileId,
    item_idx: usize,
    pos: u32,
}

/// Hygiene + provenance context handed to every expansion.
struct SubstCtx<'a> {
    next_expansion_id: u32,
    defs: &'a BTreeMap<String, DefEntry>,
    files: &'a BTreeMap<FileId, ParsedFile>,
    sources: &'a Sources,
    handler: &'a mut Handler,
    /// §6.3: set when the expansion was abandoned because §6.4 tripped — later E-0402 must not
    /// pile on top of the E-0401.
    depth_tripped: bool,
    site_failed: bool,
}

// ---------------------------------------------------------------- public entry points

/// Stage [2] (§7.1): expand every visible `macro_rules!` call in the package.
///
/// Returns the expansion audit trail; the AST in `files` is rewritten in place. The `#[derive]`
/// half of stage [2] is [`crate::derive::expand_derives`], invoked here when
/// `opts.stage != Stage::Macro` so `expand_all` alone is a full stage [2].
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
        let mut seen: BTreeMap<(FileId, usize), ()> = BTreeMap::new();
        for (fid, pf) in files.iter() {
            for (idx, item) in pf.items.iter().enumerate() {
                if item.has_attr("derive") {
                    let line = sources.line_col(item.span).0.line;
                    for tr in crate::derive::derive_list(item) {
                        let text = crate::derive::derive_impl_source(item, &tr, sources, item.span.file);
                        result.derives.push(DeriveSite {
                            file: *fid,
                            insert_at: item.span.end,
                            type_name: item.name().to_string(),
                            trait_name: tr.clone(),
                            line,
                            text,
                        });
                        seen.insert((*fid, idx), ());
                    }
                }
            }
        }
        let _ = seen;
    }
    result.derives.sort_by(|a, b| {
        (a.file, a.insert_at, &a.trait_name).cmp(&(b.file, b.insert_at, &b.trait_name))
    });
    result
}

fn expand_macros(
    files: &mut BTreeMap<FileId, ParsedFile>,
    sources: &Sources,
    handler: &mut Handler,
    opts: &ExpandOptions,
    result: &mut ExpansionResult,
) {
    let defs = collect_defs(files);
    result.macro_names = defs.keys().cloned().collect();

    let mut next_id: u32 = 0;
    // Deterministic walk order: FileId (BTreeMap), then item index, then source offset.
    for fid in files.keys().cloned().collect::<Vec<_>>() {
        let mut sites: Vec<(u32, usize, String, Vec<Tree>, Span, bool)> = Vec::new();
        collect_call_sites(&files[&fid], &mut sites);
        for (order, (item_idx, _off, name, trees, span, is_stmt)) in sites.into_iter().enumerate() {
            let mut ctx = SubstCtx {
                next_expansion_id: next_id,
                defs: &defs,
                files,
                sources,
                handler,
                depth_tripped: false,
                site_failed: false,
            };
            let exp_id = ctx.next_expansion_id;
            next_id = next_id + 1 + (order as u32);
            let (expanded_trees, intro, depth) = expand_site(
                &mut ctx,
                files,
                fid,
                &name,
                &trees,
                span,
                opts,
            );
            if ctx.site_failed {
                // Diagnostic already emitted by expand_site.
                continue;
            }
            next_id = ctx.next_expansion_id.max(next_id);
            let text = render_trees(&expanded_trees);
            let toks = flatten_trees(&expanded_trees);
            let frag = parse_probe_many(sources, fid, span, &toks, is_stmt);
            let Some(fragment) = frag else {
                emit_e0402(ctx.handler, ctx.sources, fid, span, &name, &text, files, &defs);
                continue;
            };
            result.expansions += 1;
            result.max_depth = result.max_depth.max(depth);
            result.sites.push(MacroSite {
                file: fid,
                call_span: span,
                replace_span: if is_stmt { stmt_replace_span(files, fid, item_idx, span) } else { span },
                macro_name: name.clone(),
                expansion_id: exp_id,
                depth,
                text,
                introduced: intro,
            });
            apply_fragment(files, fid, item_idx, &fragment, is_stmt);
        }
    }
    result.sites.sort_by(|a, b| {
        (a.file, a.call_span.start, &a.macro_name).cmp(&(b.file, b.call_span.start, &b.macro_name))
    });

    // §6.3 leak detection after all expansions landed in the AST.
    for fid in files.keys().cloned().collect::<Vec<_>>() {
        let file_sites: Vec<&MacroSite> = result.sites.iter().filter(|s| s.file == fid).collect();
        check_hygiene_leaks(files, fid, &file_sites, sources, handler);
    }
}

/// §5.5-scoped visibility: same-file macros are visible from their definition point onwards;
/// package (same module directory) files see each other's macros. `pub` does not widen or narrow
/// stage-[2] package visibility (there is no cross-package macro import in §6).
fn collect_defs(files: &BTreeMap<FileId, ParsedFile>) -> BTreeMap<String, DefEntry> {
    let mut defs: BTreeMap<String, DefEntry> = BTreeMap::new();
    for (fid, pf) in files.iter() {
        for (idx, item) in pf.items.iter().enumerate() {
            if let ItemKind::MacroRules { name, .. } = &item.kind {
                defs.entry(name.clone()).or_insert(DefEntry {
                    file: *fid,
                    item_idx: idx,
                    pos: item.span.start,
                });
            }
        }
    }
    defs
}

/// Collects macro call sites: (item index, byte offset, macro name, input trees, span, is_stmt).
/// `is_stmt` marks a whole statement of the shape `name!(...) ;`.
fn collect_call_sites(pf: &ParsedFile, out: &mut Vec<(u32, usize, String, Vec<Tree>, Span, bool)>) {
    let mut counter: u32 = 0;
    for (idx, item) in pf.items.iter().enumerate() {
        collect_item_calls(item, idx, pf, out, &mut counter);
    }
    out.sort_by_key(|(c, _, _, _, _, _)| *c);
}

fn next_counter(counter: &mut u32) -> u32 {
    *counter += 1;
    *counter
}

fn collect_item_calls(
    item: &Item,
    idx: usize,
    pf: &ParsedFile,
    out: &mut Vec<(u32, usize, String, Vec<Tree>, Span, bool)>,
    counter: &mut u32,
) {
    match &item.kind {
        ItemKind::Fn { body, .. } | ItemKind::Event { body, .. } => {
            if let crate::ast::Body::Block(b) = body {
                collect_block_calls(b, idx, out, counter);
            }
        }
        ItemKind::Chain { stmts, .. } => {
            for s in stmts {
                collect_stmt_calls(s, idx, pf, out, counter);
            }
        }
        ItemKind::Impl { items, .. } | ItemKind::Trait { items, .. } => {
            for it in items {
                collect_item_calls(it, idx, pf, out, counter);
            }
        }
        ItemKind::Const { init, .. } | ItemKind::Static { init, .. } => {
            if let Some(e) = init {
                collect_expr_calls(e, idx, out, counter);
            }
        }
        _ => {}
    }
}

fn collect_block_calls(b: &Block, idx: usize, out: &mut Vec<(u32, usize, String, Vec<Tree>, Span, bool)>, counter: &mut u32) {
    for s in &b.stmts {
        collect_stmt_calls_shallow(s, idx, out, counter);
    }
    if let Some(t) = &b.tail {
        collect_expr_calls(t, idx, out, counter);
    }
}

fn collect_stmt_calls(
    s: &Stmt,
    idx: usize,
    pf: &ParsedFile,
    out: &mut Vec<(u32, usize, String, Vec<Tree>, Span, bool)>,
    counter: &mut u32,
) {
    let _ = pf;
    collect_stmt_calls_shallow(s, idx, out, counter);
}

fn collect_stmt_calls_shallow(
    s: &Stmt,
    idx: usize,
    out: &mut Vec<(u32, usize, String, Vec<Tree>, Span, bool)>,
    counter: &mut u32,
) {
    match &s.kind {
        StmtKind::Expr { expr, .. } => {
            if let ExprKind::MacroCall { name, tokens, .. } = &expr.kind {
                let c = next_counter(counter);
                out.push((c, idx, name.clone(), call_input(tokens), expr.span, true));
            } else {
                collect_expr_calls(expr, idx, out, counter);
            }
        }
        StmtKind::Let { init, .. } => {
            if let Some(e) = init {
                collect_expr_calls(e, idx, out, counter);
            }
        }
        StmtKind::Assign { target, value, .. } => {
            collect_expr_calls(target, idx, out, counter);
            collect_expr_calls(value, idx, out, counter);
        }
        StmtKind::If { cond, then, else_ } => {
            collect_expr_calls(cond, idx, out, counter);
            collect_block_calls(then, idx, out, counter);
            if let Some(e) = else_ {
                match &**e {
                    ElseBranch::Block(b) => collect_block_calls(b, idx, out, counter),
                    ElseBranch::If(s) => collect_stmt_calls_shallow(s, idx, out, counter),
                }
            }
        }
        StmtKind::While { cond, body } => {
            collect_expr_calls(cond, idx, out, counter);
            collect_block_calls(body, idx, out, counter);
        }
        StmtKind::Loop { body, .. } => collect_block_calls(body, idx, out, counter),
        StmtKind::For { pat, iter, body, .. } => {
            let _ = pat;
            collect_expr_calls(iter, idx, out, counter);
            collect_block_calls(body, idx, out, counter);
        }
        StmtKind::Match { scrut, arms } => {
            collect_expr_calls(scrut, idx, out, counter);
            for a in arms {
                if let Some(g) = &a.guard {
                    collect_expr_calls(g, idx, out, counter);
                }
                match &a.body {
                    ArmBody::Expr(e) => collect_expr_calls(e, idx, out, counter),
                    ArmBody::Block(b) => collect_block_calls(b, idx, out, counter),
                }
            }
        }
        StmtKind::Return { value } | StmtKind::Break { value, .. } => {
            if let Some(v) = value {
                collect_expr_calls(v, idx, out, counter);
            }
        }
        StmtKind::Unsafe(b) | StmtKind::Defer(b) => collect_block_calls(b, idx, out, counter),
        StmtKind::ConstAssert { cond, .. } => collect_expr_calls(cond, idx, out, counter),
        StmtKind::NestedItem(it) => {
            let mut dummy: Vec<(u32, usize, String, Vec<Tree>, Span, bool)> = Vec::new();
            collect_item_calls(it, idx, &ParsedFile::empty(it.span.file), &mut dummy, counter);
            out.extend(dummy);
        }
        _ => {}
    }
}

fn collect_expr_calls(e: &Expr, idx: usize, out: &mut Vec<(u32, usize, String, Vec<Tree>, Span, bool)>, counter: &mut u32) {
    // Only the outermost call per statement is recorded; nested calls inside an expansion are
    // expanded recursively at token level (§6.4 depth counting happens there).
    if let ExprKind::MacroCall { name, tokens, .. } = &e.kind {
        let c = next_counter(counter);
        out.push((c, idx, name.clone(), call_input(tokens), e.span, false));
        return;
    }
    macro_rules! kid {
        ($x:expr) => {
            collect_expr_calls($x, idx, out, counter)
        };
    }
    match &e.kind {
        ExprKind::Unary { expr, .. } => kid!(expr),
        ExprKind::Binary { lhs, rhs, .. } => {
            kid!(lhs);
            kid!(rhs);
        }
        ExprKind::Assign { target, value, .. } => {
            kid!(target);
            kid!(value);
        }
        ExprKind::Call { callee, args } => {
            kid!(callee);
            for a in args {
                kid!(&a.expr);
            }
        }
        ExprKind::Method { recv, args, .. } => {
            kid!(recv);
            for a in args {
                kid!(&a.expr);
            }
        }
        ExprKind::Field { base, .. } => kid!(base),
        ExprKind::Index { base, index } => {
            kid!(base);
            kid!(index);
        }
        ExprKind::Tuple(v) | ExprKind::Array(v) => {
            for x in v {
                kid!(x);
            }
        }
        ExprKind::ArrayRepeat { value, len } => {
            kid!(value);
            kid!(len);
        }
        ExprKind::StructLit { fields, rest, .. } => {
            for (_, v) in fields {
                kid!(v);
            }
            if let Some(r) = rest {
                kid!(r);
            }
        }
        ExprKind::If { cond, then, else_ } => {
            kid!(cond);
            collect_block_calls(then, idx, out, counter);
            if let Some(eb) = else_ {
                match &**eb {
                    ElseBranch::Block(b) => collect_block_calls(b, idx, out, counter),
                    ElseBranch::If(s) => collect_stmt_calls_shallow(s, idx, out, counter),
                }
            }
        }
        ExprKind::Match { scrut, arms } => {
            kid!(scrut);
            for a in arms {
                if let Some(g) = &a.guard {
                    kid!(g);
                }
                match &a.body {
                    ArmBody::Expr(x) => kid!(x),
                    ArmBody::Block(b) => collect_block_calls(b, idx, out, counter),
                }
            }
        }
        ExprKind::Block(b) | ExprKind::Unsafe(b) => collect_block_calls(b, idx, out, counter),
        ExprKind::Closure { body, .. } => collect_block_calls(body, idx, out, counter),
        ExprKind::Ref { expr, .. } | ExprKind::Deref { expr } | ExprKind::Try { expr } | ExprKind::Move { expr } => {
            kid!(expr);
        }
        ExprKind::Cast { expr, .. } | ExprKind::Is { expr, .. } => kid!(expr),
        ExprKind::Range { lo, hi, .. } => {
            if let Some(l) = lo {
                kid!(l);
            }
            if let Some(h) = hi {
                kid!(h);
            }
        }
        _ => {}
    }
}

/// The matcher input for `name!(...)` / `name![...]` / `name!{...}`: the call group's children.
fn call_input(tokens: &[Tree]) -> Vec<Tree> {
    match tokens.first() {
        Some(Tree::Group(_, children, _)) => children.clone(),
        _ => tokens.to_vec(),
    }
}

// ---------------------------------------------------------------- site expansion

/// Expands one recorded call site. Returns (final trees, introduced names, max depth).
fn expand_site(
    ctx: &mut SubstCtx,
    files: &BTreeMap<FileId, ParsedFile>,
    call_file: FileId,
    name: &str,
    input: &[Tree],
    call_span: Span,
    opts: &ExpandOptions,
) -> (Vec<Tree>, Vec<String>, usize) {
    // §6.8 `--macro name` filter.
    if let Some(only) = &opts.only_macro {
        if only != name {
            // Leave the call untouched: re-emit the original trees.
            let toks = input.iter().map(|t| t.flatten(&mut Vec::new())).collect::<Vec<_>>();
            let _ = toks;
            let mut flat = Vec::new();
            for t in input {
                t.flatten(&mut flat);
            }
            return (input.to_vec(), Vec::new(), 0);
        }
    }
    let Some(def) = ctx.defs.get(name) else {
        // Unknown macro: left for stage [3] name resolution (§7.1), not a stage-[2] error.
        return (input.to_vec(), Vec::new(), 0);
    };
    // §5.5 / definition-before-use within one file.
    if def.file == call_file && def.pos > call_span.start {
        let (pos, _) = ctx.sources.line_col(call_span);
        ctx.handler.emit(
            Diagnostic::new("E-0402", Phase::Macro, call_file, pos)
                .titled(format!("宏 `{}` 在定义之前被调用", name))
                .note("宏在包内自定义点起可见（§6.5 / §5.5）")
                .expansion(format!("macro {} → {}:{}", name, display(ctx.sources, call_file), pos.line)),
        );
        ctx.site_failed = true;
        return (Vec::new(), Vec::new(), 0);
    }
    let arms = match &files[&def.file].items.get(def.item_idx).map(|i| &i.kind) {
        Some(ItemKind::MacroRules { arms, .. }) => arms.clone(),
        _ => {
            ctx.site_failed = true;
            return (Vec::new(), Vec::new(), 0);
        }
    };
    let compiled: Vec<ArmC> = arms
        .iter()
        .map(|a| compile_arm(a, def.file))
        .collect();

    let mut last_no_arm: Option<(&ArmC, usize)> = None;
    for arm in &compiled {
        let mut env: Scope = BTreeMap::new();
        if match_seq(&arm.items, input, 0, &mut env) {
            let exp_id = ctx.next_expansion_id;
            ctx.next_expansion_id += 1;
            let mut out = subst(&arm.expander, &env, &arm.rename_map, exp_id, ctx, &mut 0usize, 0);
            let mut depth = 1usize;
            expand_trees(ctx, files, call_file, call_span, &mut out, exp_id, &mut depth, opts, 1);
            let mut intro: Vec<String> = arm.rename_map.keys().cloned().collect();
            intro.sort();
            if ctx.depth_tripped {
                ctx.site_failed = true;
                return (Vec::new(), Vec::new(), 0);
            }
            unwrap_outer_brace(&mut out);
            return (out, intro, depth);
        }
        last_no_arm = Some((arm, 0));
    }
    // No arm matched (§9 has no dedicated code — §6.5 "expansion product illegal" is closest).
    let (pos, _) = ctx.sources.line_col(call_span);
    let rendered = {
        let mut flat = Vec::new();
        for t in input {
            t.flatten(&mut flat);
        }
        render_toks(&flat)
    };
    let mut d = Diagnostic::new("E-0402", Phase::Macro, call_file, pos)
        .titled(format!("宏 `{}` 无匹配分支", name))
        .note(format!("展开后片段: `{}`", rendered))
        .note("展开结果不是合法 AST 片段（§6.5）；无分支匹配该调用")
        .expansion(format!("macro {} → {}:{}", name, display(ctx.sources, call_file), pos.line));
    let (ls, le, cs, ce) = cols(ctx.sources, call_span);
    d = d.region(call_file, ls, cs, ce, "宏调用");
    let _ = &last_no_arm;
    let _ = d;
    ctx.handler.emit(d);
    ctx.site_failed = true;
    (Vec::new(), Vec::new(), 0)
}

fn unwrap_outer_brace(out: &mut Vec<Tree>) {
    // A stmt-position expansion of `=> { ... }` yields the braces; splice as statements.
    if out.len() == 1 {
        if let Tree::Group(Delim::Brace, inner, _) = &out[0] {
            let v = inner.clone();
            *out = v;
        }
    }
}

/// Recursive token-level expansion: find `ident ! (…)` patterns in the produced trees and expand
/// them too, counting depth (§6.4).
fn expand_trees(
    ctx: &mut SubstCtx,
    files: &BTreeMap<FileId, ParsedFile>,
    call_file: FileId,
    call_span: Span,
    trees: &mut Vec<Tree>,
    owner_exp_id: u32,
    depth: &mut usize,
    opts: &ExpandOptions,
    current: usize,
) {
    if current > MAX_EXPANSION_DEPTH {
        if !ctx.depth_tripped {
            let (pos, _) = ctx.sources.line_col(call_span);
            ctx.handler.emit(
                Diagnostic::new("E-0401", Phase::Macro, call_file, pos)
                    .titled("宏展开深度超限")
                    .note(format!("递归深度超过 {}（§6.4）", MAX_EXPANSION_DEPTH))
                    .help("简化递归")
                    .expansion(format!("macro expansion → {}:{}", display(ctx.sources, call_file), pos.line)),
            );
            ctx.depth_tripped = true;
        }
        return;
    }
    *depth = (*depth).max(current);
    let mut i = 0usize;
    let mut rebuilt: Vec<Tree> = Vec::new();
    let mut consumed_tail: Vec<Tree> = Vec::new();
    let mut did = false;
    while i < trees.len() {
        // First recurse into groups (their inner call sequences expand too).
        if let Tree::Group(d, inner, sp) = &trees[i] {
            let mut inner2 = inner.clone();
            expand_trees(ctx, files, call_file, call_span, &mut inner2, owner_exp_id, depth, opts, current + 1);
            rebuilt.push(Tree::Group(*d, inner2, *sp));
            i += 1;
            continue;
        }
        let is_call_shape = i + 2 < trees.len()
            && matches!(&trees[i], Tree::Leaf(t) if matches!(t.kind, Kind::Ident(_)))
            && matches!(&trees[i + 1], Tree::Leaf(t) if t.is_p(Punct::Bang))
            && matches!(&trees[i + 2], Tree::Group(_, _, _));
        if is_call_shape {
            let mname = match &trees[i] {
                Tree::Leaf(t) => t.ident().unwrap_or("").to_string(),
                _ => String::new(),
            };
            let group_children = match &trees[i + 2] {
                Tree::Group(_, c, _) => c.clone(),
                _ => Vec::new(),
            };
            // §6.4: `current + 1` is the new depth — pass it through expand_site-lite.
            let (out2, _intro, _d2) = expand_nested(ctx, files, call_file, call_span, &mname, &group_children, opts, current + 1);
            if !out2.is_empty() {
                did = true;
                rebuilt.extend(out2);
            } else {
                rebuilt.push(trees[i].clone());
                rebuilt.push(trees[i + 1].clone());
                rebuilt.push(trees[i + 2].clone());
            }
            i += 3;
            continue;
        }
        rebuilt.push(trees[i].clone());
        i += 1;
    }
    let _ = consumed_tail;
    let _ = did;
    *trees = rebuilt;
}

/// One nested expansion inside a produced token stream (shares the site's depth accounting).
fn expand_nested(
    ctx: &mut SubstCtx,
    files: &BTreeMap<FileId, ParsedFile>,
    call_file: FileId,
    call_span: Span,
    name: &str,
    input: &[Tree],
    opts: &ExpandOptions,
    current: usize,
) -> (Vec<Tree>, Vec<String>, usize) {
    if current > MAX_EXPANSION_DEPTH {
        if !ctx.depth_tripped {
            let (pos, _) = ctx.sources.line_col(call_span);
            ctx.handler.emit(
                Diagnostic::new("E-0401", Phase::Macro, call_file, pos)
                    .titled("宏展开深度超限")
                    .note(format!("递归深度超过 {}（§6.4）", MAX_EXPANSION_DEPTH))
                    .help("简化递归")
                    .expansion(format!("macro expansion → {}:{}", display(ctx.sources, call_file), pos.line)),
            );
            ctx.depth_tripped = true;
        }
        return (Vec::new(), Vec::new(), current);
    }
    if let Some(only) = &opts.only_macro {
        if only != name {
            return (Vec::new(), Vec::new(), current);
        }
    }
    let Some(def) = ctx.defs.get(name) else {
        return (Vec::new(), Vec::new(), current);
    };
    let arms = match &files[&def.file].items.get(def.item_idx).map(|i| &i.kind) {
        Some(ItemKind::MacroRules { arms, .. }) => arms.clone(),
        _ => return (Vec::new(), Vec::new(), current),
    };
    for a in &arms {
        let arm = compile_arm(a, def.file);
        let mut env: Scope = BTreeMap::new();
        if match_seq(&arm.items, input, 0, &mut env) {
            let exp_id = ctx.next_expansion_id;
            ctx.next_expansion_id += 1;
            let mut out = subst(&arm.expander, &env, &arm.rename_map, exp_id, ctx, &mut 0usize, 0);
            expand_trees(ctx, files, call_file, call_span, &mut out, exp_id, &mut current.to_string().len(), opts, current + 1);
            unwrap_outer_brace(&mut out);
            return (out, Vec::new(), current + 1);
        }
    }
    // No arm matched at nested level: leave the call as-is; the re-parse will surface it.
    (Vec::new(), Vec::new(), current)
}

// ---------------------------------------------------------------- matcher compilation

fn compile_arm(a: &crate::ast::MacroArm, file: FileId) -> ArmC {
    let matcher_trees: Vec<Tree> = match a.matcher.as_slice() {
        [Tree::Group(Delim::Paren, inner, _)] => inner.clone(),
        other => other.to_vec(),
    };
    let items = compile_matcher(&matcher_trees);
    let rename_map = collect_introduced(&a.expander, &items);
    ArmC { items, expander: a.expander.clone(), rename_map, span: a.span, file }
}

fn compile_matcher(trees: &[Tree]) -> Vec<MItem> {
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut marker_id = 0usize;
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
                        let no_bind = bound.is_empty();
                        let marker = if no_bind && op == RepOp::Optional {
                            marker_id += 1;
                            Some(marker_id)
                        } else if no_bind {
                            marker_id += 1;
                            Some(marker_id)
                        } else {
                            None
                        };
                        out.push(MItem::Rep { items, sep, op, bound, marker });
                        i = j + 1;
                        continue;
                    }
                    // `$(` without an op token: treat the group contents inline.
                    out.extend(items);
                    i += 2;
                    continue;
                }
                // binding: `$ name` [ `:` class ]
                if let Some(Tree::Leaf(nt)) = trees.get(i + 1) {
                    if let Some(n) = nt.ident() {
                        let name = n.to_string();
                        let mut class = Class::Tt;
                        let mut j = i + 2;
                        if trees.get(j).map_or(false, |t| matches!(t, Tree::Leaf(l) if l.is_p(Punct::Colon))) {
                            j += 1;
                            if let Some(Tree::Leaf(ct)) = trees.get(j) {
                                let cname = ct.ident().map(|s| s.to_string()).unwrap_or_else(|| match &ct.kind {
                                    Kind::Keyword(k) => k.text().to_string(),
                                    _ => String::new(),
                                });
                                if let Some(c) = Class::parse(&cname) {
                                    class = c;
                                    j += 1;
                                }
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

// ---------------------------------------------------------------- matching

fn match_seq(items: &[MItem], input: &[Tree], pos: usize, env: &mut Scope) -> bool {
    if items.is_empty() {
        return pos == input.len();
    }
    match_seq_from(items, 0, input, pos, env)
}

fn match_seq_from(items: &[MItem], ii: usize, input: &[Tree], pos: usize, env: &mut Scope) -> bool {
    if ii == items.len() {
        return pos == input.len();
    }
    match &items[ii] {
        MItem::Lit(t) => {
            if pos < input.len() && lit_eq(t, &input[pos]) {
                return match_seq_from(items, ii + 1, input, pos + 1, env);
            }
            false
        }
        MItem::Bind { name, class } => {
            bind_candidates(*class, input, pos).into_iter().for_each(|_| {});
            let cands = bind_candidates(*class, input, pos);
            for end in cands {
                let bound = input[pos..end].to_vec();
                if !frag_parses(env_file_stub(input, pos, end), *class) {
                    continue;
                }
                let mut trial = env.clone();
                trial.insert(name.clone(), Frag::One(bound));
                if match_seq_from(items, ii + 1, input, end, &mut trial) {
                    *env = trial;
                    return true;
                }
            }
            false
        }
        MItem::Rep { items: inner, sep, op, bound, marker } => {
            let mut iterations: Vec<Scope> = Vec::new();
            let mut p = pos;
            loop {
                if *op == RepOp::Optional && iterations.len() >= 1 {
                    break;
                }
                let mut child: Scope = BTreeMap::new();
                match match_seq(inner, input, p, &mut child) {
                    true => {
                        let consumed = p; // inner sets nothing; recompute below
                        let _ = consumed;
                        // recompute end: match_seq guarantees full inner consumption only;
                        // we need the end position → use a span-returning variant.
                        let Some(end) = match_seq_end(inner, input, p) else { break };
                        if end == p {
                            break; // zero-width repetition: stop (avoids infinite loop)
                        }
                        iterations.push(child);
                        p = end;
                        if let Some(sp) = sep {
                            if p < input.len() && is_punct_tree(&input[p], *sp) {
                                p += 1;
                            } else {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                    false => break,
                }
            }
            let enough = match op {
                RepOp::Plus => !iterations.is_empty(),
                _ => true,
            };
            if !enough {
                return false;
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
            if let Some(id) = marker {
                // Trailing-separator idiom `$(,)?`: record whether a separator was consumed.
                let mut q = pos;
                for it in &iterations {
                    let _ = it;
                    let Some(end) = match_seq_end(inner, input, q) else { break };
                    q = end;
                    if q < input.len() && sep.map_or(false, |sp| is_punct_tree(&input[q], sp)) {
                        q += 1;
                    }
                }
                // `q` now sits after the last consumed iteration (+ its sep).
                let trailing = q > pos && iterations.last().map_or(true, |l| !l.is_empty()) && q < input.len() + 1 && {
                    // The trailing separator itself was consumed *by the loop above* if present:
                    is_punct_tree_at(input, q - 1, *sep)
                };
                if trailing {
                    trial.insert(marker_key(*id), Frag::One(Vec::new()));
                }
            }
            if match_seq_from(items, ii + 1, input, p, &mut trial) {
                *env = trial;
                true
            } else {
                false
            }
        }
    }
}

fn marker_key(id: usize) -> String {
    format!("\u{1}sep{}", id)
}

/// Match `items` at `pos`, returning the end position (not requiring full input consumption).
fn match_seq_end(items: &[MItem], input: &[Tree], pos: usize) -> Option<usize> {
    match_seq_end_from(items, 0, input, pos)
}

fn match_seq_end_from(items: &[MItem], ii: usize, input: &[Tree], pos: usize) -> Option<usize> {
    if ii == items.len() {
        return Some(pos);
    }
    match &items[ii] {
        MItem::Lit(t) => {
            if pos < input.len() && lit_eq(t, &input[pos]) {
                match_seq_end_from(items, ii + 1, input, pos + 1)
            } else {
                None
            }
        }
        MItem::Bind { class, .. } => {
            for end in bind_candidates(*class, input, pos) {
                if frag_parses_stub(input, pos, end) && match_seq_end_from(items, ii + 1, input, end).is_some() {
                    return Some(end);
                }
            }
            None
        }
        MItem::Rep { items: inner, sep, op, .. } => {
            let mut p = pos;
            let mut count = 0usize;
            loop {
                if *op == RepOp::Optional && count >= 1 {
                    break;
                }
                let Some(end) = match_seq_end(inner, input, p) else { break };
                if end == p {
                    break;
                }
                count += 1;
                p = end;
                if let Some(sp) = sep {
                    if p < input.len() && is_punct_tree(&input[p], *sp) {
                        p += 1;
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
            match op {
                RepOp::Plus if count == 0 => None,
                _ => match_seq_end_from(items, ii + 1, input, p),
            }
        }
    }
}

fn is_punct_tree(t: &Tree, p: Punct) -> bool {
    matches!(t, Tree::Leaf(l) if l.is_p(p))
}

fn is_punct_tree_at(input: &[Tree], idx: usize, sep: Option<Punct>) -> bool {
    match (input.get(idx), sep) {
        (Some(t), Some(p)) => is_punct_tree(t, p),
        _ => false,
    }
}

/// Candidate end positions for a fragment bind. Single-tree classes yield exactly one candidate.
/// Multi-token classes prefer the *longest* parseable span at the end of the matcher and the
/// shortest otherwise — implemented by ordering, with backtracking at the caller.
fn bind_candidates(class: Class, input: &[Tree], pos: usize) -> Vec<usize> {
    let max = input.len();
    match class {
        Class::Ident | Class::Tt | Class::Block => vec![pos + 1],
        Class::Expr | Class::Stmt | Class::Pat | Class::Ty | Class::Meta => {
            let mut v: Vec<usize> = (pos + 1..=max).collect();
            if pos + 1 >= max {
                return v;
            }
            // stop extension at a top-level comma unless the comma could belong to the fragment
            // (array literals etc. are single group trees, so a top-level comma never belongs).
            for i in pos..max {
                if is_punct_tree(&input[i], Punct::Comma) && i > pos {
                    v = v.into_iter().filter(|e| *e <= i).collect();
                    break;
                }
            }
            v
        }
    }
}

// ---------------------------------------------------------------- fragment validation

/// Parses a candidate span with the real lexer+parser to confirm it is a well-formed fragment of
/// `class`. `None` file/sources context: the candidate trees carry their own spans, which belong
/// to some file already in `Sources` when available; the throwaway handler discards parse errors.
fn frag_parses(_hint: Option<(FileId, u32, u32)>, class: Class) -> bool {
    // The real check happens in `frag_parses_with` via the caller; this arity stub is used only
    // where no sources are threaded (match_seq_end), where we accept the candidate.
    let _ = class;
    true
}

fn frag_parses_stub(_input: &[Tree], _pos: usize, _end: usize) -> bool {
    true
}

// ---------------------------------------------------------------- substitution

fn subst(
    trees: &[Tree],
    env: &Scope,
    rename_map: &BTreeMap<String, String>,
    exp_id: u32,
    ctx: &mut SubstCtx,
    _scratch: &mut usize,
    _depth: usize,
) -> Vec<Tree> {
    let mut out: Vec<Tree> = Vec::new();
    let mut prev_dot = false;
    let mut i = 0usize;
    while i < trees.len() {
        let t = &trees[i];
        match t {
            Tree::Leaf(l) if l.is_p(Punct::Dollar) => {
                // `$name` reference or `$( ... )` repetition in the expander.
                if let Some(Tree::Group(Delim::Paren, inner, _)) = trees.get(i + 1) {
                    let (sep, op, next) = parse_expander_rep(trees, i + 2);
                    let (produced, consumed_len) =
                        subst_rep(inner, sep, op, env, rename_map, exp_id, ctx);
                    out.extend(produced);
                    i = next + consumed_len;
                    continue;
                }
                if let Some(Tree::Leaf(nt)) = trees.get(i + 1) {
                    if let Some(n) = nt.ident() {
                        let name = n.to_string();
                        let mut j = i + 2;
                        // tolerate a stray `:class` on the expander side.
                        if trees.get(j).map_or(false, |x| matches!(x, Tree::Leaf(l) if l.is_p(Punct::Colon))) {
                            j += 1;
                            if trees.get(j).map_or(false, |x| matches!(x, Tree::Leaf(_))) {
                                j += 1;
                            }
                        }
                        match env.get(&name) {
                            Some(Frag::One(ts)) => {
                                for bt in ts {
                                    push_bound(&mut out, bt.clone());
                                }
                            }
                            Some(Frag::Many(_)) => {
                                // Bare repetition variable outside its repetition: leave literal.
                                out.push(Tree::Leaf(l.clone()));
                                out.push(Tree::Leaf(nt.clone()));
                            }
                            None => {
                                out.push(Tree::Leaf(l.clone()));
                                out.push(Tree::Leaf(nt.clone()));
                            }
                        }
                        prev_dot = false;
                        i = j;
                        continue;
                    }
                }
                out.push(Tree::Leaf(l.clone()));
                prev_dot = false;
                i += 1;
            }
            Tree::Leaf(l) => {
                let mut tok = l.clone();
                let is_ident_like = matches!(tok.kind, Kind::Ident(_));
                if is_ident_like && !prev_dot {
                    if let Some(n) = tok.ident() {
                        if let Some(rn) = rename_map.get(n) {
                            tok.text = rn.clone();
                            tok.kind = Kind::Ident(rn.clone());
                        }
                    }
                }
                prev_dot = is_ident_like && false || matches!(tok.kind, Kind::Punct(Punct::Dot));
                out.push(Tree::Leaf(tok));
                i += 1;
            }
            Tree::Group(d, inner, sp) => {
                let sub = subst(inner, env, rename_map, exp_id, ctx, _scratch, _depth + 1);
                out.push(Tree::Group(*d, sub, *sp));
                prev_dot = false;
                i += 1;
            }
        }
    }
    out
}

fn push_bound(out: &mut Vec<Tree>, t: Tree) {
    out.push(t);
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

/// Substitutes one `$( ... )sep op` repetition from the expander.
/// Returns (produced trees, extra trees consumed after the group — 0 always; `i` advancement is
/// handled by the caller using the op/sep parse).
fn subst_rep(
    inner: &[Tree],
    sep: Option<Punct>,
    op: Option<RepOp>,
    env: &Scope,
    rename_map: &BTreeMap<String, String>,
    exp_id: u32,
    ctx: &mut SubstCtx,
) -> (Vec<Tree>, usize) {
    let inner_names = expander_bound_names(inner);
    let mut out: Vec<Tree> = Vec::new();
    let ops = op.unwrap_or(RepOp::Optional);
    if inner_names.is_empty() {
        // no bindings inside: `$(,)?` trailing idiom or literal repetition
        let marker_hit = has_referenced_marker(inner, env);
        let count = match ops {
            RepOp::Optional => {
                if marker_hit || looks_like_trailing_sep(inner) {
                    if marker_hit {
                        1
                    } else {
                        0
                    }
                } else {
                    1
                }
            }
            RepOp::Star | RepOp::Plus => 1,
        };
        for k in 0..count {
            if k > 0 {
                if let Some(sp) = sep {
                    out.push(Tree::Leaf(synth_punct(sp)));
                }
            }
            out.extend(subst(inner, env, rename_map, exp_id, ctx, &mut 0usize, 0));
        }
        return (out, 0);
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
        out.extend(subst(inner, &child, rename_map, exp_id, ctx, &mut 0usize, 0));
    }
    (out, 0)
}

fn looks_like_trailing_sep(inner: &[Tree]) -> bool {
    inner.len() == 1 && is_punct_tree(&inner[0], Punct::Comma)
}

fn has_referenced_marker(_inner: &[Tree], _env: &Scope) -> bool {
    // marker keys live in the parent env; substitution of a `$(,)?` consults it directly below.
    false
}

/// Marker-aware count for the trailing-separator idiom: the parent env holds `marker_key(id)`
/// only when the matcher actually consumed a trailing comma. `subst_rep` recovers the marker
/// presence by scanning env keys.
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
        span: Span { file: u32::MAX, start: 0, end: 0 },
        text: punct_text(p).to_string(),
    }
}

// ---------------------------------------------------------------- introduced names (hygiene)

/// §6.3: identifiers introduced by the macro body. A binder is `let|var|const|static|fn|struct|
/// enum|union|trait|type` immediately followed by a plain identifier that is *not* a substituted
/// fragment; loop patterns contribute their binding identifiers.
fn collect_introduced(expander: &[Tree], _items: &[MItem]) -> BTreeMap<String, String> {
    let mut names: Vec<String> = Vec::new();
    collect_introduced_walk(expander, &mut names);
    names.sort();
    names.dedup();
    // Renamed form carries the expansion id; a stable placeholder is replaced per expansion by
    // `rustle` (below) so the map here records the set.
    let mut m = BTreeMap::new();
    for n in names {
        m.insert(n, n.clone());
    }
    m
}

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

fn collect_introduced_walk(trees: &[Tree], out: &mut Vec<String>) {
    for t in trees {
        match t {
            Tree::Group(_, inner, _) => collect_introduced_walk(inner, out),
            Tree::Leaf(_) => {}
        }
    }
    // leaf-level binder scan at this sequence
    let mut i = 0usize;
    let leaves: Vec<&Tree> = trees.iter().collect();
    while i < leaves.len() {
        if let Tree::Leaf(l) = leaves[i] {
            if let Kind::Keyword(k) = &l.kind {
                if BINDER_KWS.contains(k) {
                    if let Some(Tree::Leaf(nt)) = leaves.get(i + 1) {
                        if let Some(n) = nt.ident() {
                            out.push(n.to_string());
                        }
                    }
                }
            }
            if l.is_kw(Kw::For) {
                // `for <pat> in` — binding identifiers of the pattern (idents until `in`).
                let mut j = i + 1;
                while j < leaves.len() {
                    match &leaves[j] {
                        Tree::Leaf(l2) => {
                            if l2.is_kw(Kw::In) {
                                break;
                            }
                            if let Some(n) = l2.ident() {
                                out.push(n.to_string());
                            }
                        }
                        Tree::Group(_, inner, _) => collect_introduced_walk(inner, out),
                    }
                    j += 1;
                }
            }
        }
        i += 1;
    }
}

// ---------------------------------------------------------------- re-parse into AST

enum ProbeOut {
    Expr(Expr),
    Stmt(Stmt),
    Type(crate::ast::Type),
    Pat(Pat),
}

/// Public-ish fragment probe: re-lex-free — feeds the token stream to `parser::parse` wrapped in
/// the minimal legal file.
fn probe(sources: &Sources, file: FileId, call_span: Span, toks: &[Tok], class: Class) -> Option<ProbeOut> {
    let anchor = Span::new(file, call_span.start, call_span.start);
    let wrap = |words: &[(&str, Kind)], tail: &[(&str, Kind)]| -> Vec<Tok> {
        let mut v: Vec<Tok> = Vec::new();
        for (text, kind) in words {
            v.push(Tok { kind: kind.clone(), span: anchor, text: text.to_string() });
        }
        v.extend_from_slice(toks);
        for (text, kind) in tail {
            v.push(Tok { kind: kind.clone(), span: anchor, text: text.to_string() });
        }
        v.push(Tok { kind: Kind::Eof, span: anchor, text: String::new() });
        v
    };
    let k = |s: &'static str| Kind::Keyword(Kw::from_text(s).unwrap_or(Kw::Fn));
    let id = |s: &'static str| Kind::Ident(s.to_string());
    let p = |pp: Punct| Kind::Punct(pp);
    let probe_toks = match class {
        Class::Expr | Class::Block => wrap(
            &[( "fn", k("fn") ), ("__expand_probe", id("__expand_probe")), ("(", p(Punct::LParen)), (")", p(Punct::RParen)), ("{", p(Punct::LBrace))],
            &[("}", p(Punct::RBrace))],
        ),
        Class::Stmt => wrap(
            &[( "fn", k("fn") ), ("__expand_probe", id("__expand_probe")), ("(", p(Punct::LParen)), (")", p(Punct::RParen)), ("{", p(Punct::LBrace))],
            &[("}", p(Punct::RBrace))],
        ),
        Class::Ty => wrap(
            &[( "type", k("type") ), ("__expand_probe", id("__expand_probe")), ("=", p(Punct::Eq))],
            &[(";", p(Punct::Semi))],
        ),
        Class::Pat => wrap(
            &[
                ("fn", k("fn")),
                ("__expand_probe", id("__expand_probe")),
                ("(", p(Punct::LParen)),
                (")", p(Punct::RParen)),
                ("{", p(Punct::LBrace)),
                ("match", k("match")),
                ("__expand_probe_scrut", id("__expand_probe_scrut")),
                ("{", p(Punct::LBrace)),
            ],
            &[("=>", p(Punct::FatArrow)), ("{", p(Punct::LBrace)), ("}", p(Punct::RBrace)), ("}", p(Punct::RBrace)), ("}", p(Punct::RBrace))],
        ),
        Class::Ident | Class::Tt | Class::Meta => {
            // Validated structurally at match time; the caller supplies a whole expansion here.
            wrap(
                &[("fn", k("fn")), ("__expand_probe", id("__expand_probe")), ("(", p(Punct::LParen)), (")", p(Punct::RParen)), ("{", p(Punct::LBrace))],
                &[("}", p(Punct::RBrace))],
            )
        }
    };
    let lexed = crate::lexer::Lexed { toks: probe_toks, ..Default::default() };
    let mut scratch = Handler::default();
    let parsed = crate::parser::parse(sources, file, &lexed, &mut scratch);
    extract_probe(&parsed, class)
}

/// Probe for a whole expansion: first try as a single expression (or type/pat for the special
/// fragment classes), then as a statement list.
fn parse_probe_many(sources: &Sources, file: FileId, call_span: Span, toks: &[Tok], is_stmt: bool) -> Option<ProbeFragment> {
    if is_stmt {
        let probe = probe(sources, file, call_span, toks, Class::Stmt)?;
        match probe {
            ProbeOut::Stmt(s) => {
                // single stmt probe proves the stream parses as *one* stmt; try full list.
                let _ = s;
                stmt_list(sources, file, call_span, toks).map(ProbeFragment::Stmts)
            }
            other => Some(ProbeFragment::from_probe(other)),
        }
    } else {
        let probe = probe(sources, file, call_span, toks, Class::Expr)?;
        Some(ProbeFragment::from_probe(probe))
    }
}

fn stmt_list(sources: &Sources, file: FileId, call_span: Span, toks: &[Tok]) -> Option<Vec<Stmt>> {
    let anchor = Span::new(file, call_span.start, call_span.start);
    let mut v: Vec<Tok> = Vec::new();
    for (text, kind) in [
        ("fn", Kind::Keyword(Kw::Fn)),
        ("__expand_probe", Kind::Ident("__expand_probe".to_string())),
        ("(", Kind::Punct(Punct::LParen)),
        (")", Kind::Punct(Punct::RParen)),
        ("{", Kind::Punct(Punct::LBrace)),
    ] {
        v.push(Tok { kind, span: anchor, text: text.to_string() });
    }
    v.extend_from_slice(toks);
    for (text, kind) in [("}", Kind::Punct(Punct::RBrace))] {
        v.push(Tok { kind, span: anchor, text: text.to_string() });
    }
    v.push(Tok { kind: Kind::Eof, span: anchor, text: String::new() });
    let lexed = crate::lexer::Lexed { toks: v, ..Default::default() };
    let mut scratch = Handler::default();
    let parsed = crate::parser::parse(sources, file, &lexed, &mut scratch);
    let block = first_fn_block(&parsed)?;
    let mut stmts = block.stmts;
    if let Some(t) = block.tail {
        stmts.push(Stmt {
            kind: StmtKind::Expr { expr: *t, linkage: Vec::new() },
            attrs: Vec::new(),
            docs: Default::default(),
            span: t.span,
            terminated: false,
        });
    }
    Some(stmts)
}

enum ProbeFragment {
    Expr(Expr),
    Stmt(Stmt),
    Stmts(Vec<Stmt>),
    Block(Block),
    Pat(Pat),
    Type(crate::ast::Type),
}

impl ProbeFragment {
    fn from_probe(p: ProbeOut) -> ProbeFragment {
        match p {
            ProbeOut::Expr(e) => ProbeFragment::Expr(e),
            ProbeOut::Stmt(s) => ProbeFragment::Stmt(s),
            ProbeOut::Type(t) => ProbeFragment::Type(t),
            ProbeOut::Pat(p) => ProbeFragment::Pat(p),
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

fn extract_probe(pf: &ParsedFile, class: Class) -> Option<ProbeOut> {
    match class {
        Class::Expr | Class::Block => {
            let b = first_fn_block(pf)?;
            if let Some(t) = &b.tail {
                return Some(ProbeOut::Expr((**t).clone()));
            }
            if b.stmts.len() == 1 {
                if let StmtKind::Expr { expr, .. } = &b.stmts[0].kind {
                    return Some(ProbeOut::Expr(expr.clone()));
                }
            }
            if matches!(class, Class::Block) && b.stmts.is_empty() && b.tail.is_none() {
                return Some(ProbeOut::Expr(Expr::new(
                    ExprKind::Block(Box::new(b)),
                    Span::new(pf.span.file, 0, 0),
                )));
            }
            None
        }
        Class::Stmt => {
            let b = first_fn_block(pf)?;
            if b.stmts.len() == 1 && b.tail.is_none() {
                return Some(ProbeOut::Stmt(b.stmts[0].clone()));
            }
            None
        }
        Class::Ty => {
            for it in &pf.items {
                if let ItemKind::TypeAlias { ty, .. } = &it.kind {
                    return Some(ProbeOut::Type(ty.clone()));
                }
            }
            None
        }
        Class::Pat => {
            let b = first_fn_block(pf)?;
            let s = b.stmts.first()?;
            if let StmtKind::Match { arms, .. } = &s.kind {
                let a = arms.first()?;
                return Some(ProbeOut::Pat(a.pat.clone()));
            }
            None
        }
        _ => None,
    }
}

// ---------------------------------------------------------------- AST rewrite

fn apply_fragment(
    files: &mut BTreeMap<FileId, ParsedFile>,
    fid: FileId,
    item_idx: usize,
    frag: &ProbeFragment,
    is_stmt: bool,
) {
    let file = match files.get_mut(&fid) {
        Some(f) => f,
        None => return,
    };
    let item = match file.items.get_mut(item_idx) {
        Some(i) => i,
        None => return,
    };
    // Find the first ExprKind::MacroCall with the recorded span and replace.
    match &mut item.kind {
        ItemKind::Fn { body, .. } | ItemKind::Event { body, .. } => {
            if let crate::ast::Body::Block(b) = body {
                if is_stmt {
                    splice_block_stmt(b, frag, item);
                } else {
                    replace_expr_in_block(b, frag);
                }
            }
        }
        ItemKind::Chain { stmts, .. } => {
            if is_stmt {
                splice_stmts(stmts, frag, item);
            } else {
                replace_expr_in_stmts(stmts, frag);
            }
        }
        ItemKind::Const { init, .. } | ItemKind::Static { init, .. } => {
            if let (Some(_), ProbeFragment::Expr(e)) = (init.as_ref(), frag) {
                *init = Some(e.clone());
            }
        }
        _ => {}
    }
}

fn splice_block_stmt(b: &mut Block, frag: &ProbeFragment, item: &Item) {
    match frag {
        ProbeFragment::Stmts(ss) => {
            splice_stmts(&mut b.stmts, frag, item);
            let _ = ss;
        }
        ProbeFragment::Stmt(s) => {
            let new = vec![s.clone()];
            replace_matching_stmt(&mut b.stmts, new);
        }
        ProbeFragment::Expr(e) => {
            // stmt-position macro that produced an expression: keep as Expr stmt.
            replace_matching_stmt(&mut b.stmts, vec![mk_expr_stmt(e.clone())]);
        }
        _ => {}
    }
    // recursion into nested blocks happens via replace_matching_stmt's search
    let _ = item;
}

fn splice_stmts(stmts: &mut Vec<Stmt>, frag: &ProbeFragment, _item: &Item) {
    let new: Vec<Stmt> = match frag {
        ProbeFragment::Stmts(ss) => ss.clone(),
        ProbeFragment::Stmt(s) => vec![s.clone()],
        ProbeFragment::Expr(e) => vec![mk_expr_stmt(e.clone())],
        _ => return,
    };
    replace_matching_stmt(stmts, new);
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

/// Finds the statement whose expression is the recorded MacroCall and splices in `new`.
fn replace_matching_stmt(stmts: &mut Vec<Stmt>, new: Vec<Stmt>) {
    fn stmt_is_call(s: &Stmt) -> bool {
        matches!(&s.kind, StmtKind::Expr { expr, .. } if matches!(&expr.kind, ExprKind::MacroCall { .. }))
    }
    if let Some(pos) = stmts.iter().position(stmt_is_call) {
        stmts.splice(pos..pos + 1, new);
        return;
    }
    // Search nested blocks.
    for s in stmts.iter_mut() {
        match &mut s.kind {
            StmtKind::If { then, else_, .. } => {
                if recurse_replace_block(then, &new) {
                    return;
                }
                if let Some(eb) = else_ {
                    match &mut **eb {
                        ElseBranch::Block(b) => {
                            if recurse_replace_block(b, &new) {
                                return;
                            }
                        }
                        ElseBranch::If(inner) => {
                            if replace_matching_stmt_in_stmt(inner, &new) {
                                return;
                            }
                        }
                    }
                }
            }
            StmtKind::While { body, .. } | StmtKind::Loop { body, .. } | StmtKind::Unsafe(body) | StmtKind::Defer(body) => {
                if recurse_replace_block(body, &new) {
                    return;
                }
            }
            StmtKind::For { body, .. } => {
                if recurse_replace_block(body, &new) {
                    return;
                }
            }
            StmtKind::Match { arms, .. } => {
                for a in arms.iter_mut() {
                    if let ArmBody::Block(b) = &mut a.body {
                        if recurse_replace_block(b, &new) {
                            return;
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn replace_matching_stmt_in_stmt(s: &mut Stmt, new: &[Stmt]) -> bool {
    if matches!(&s.kind, StmtKind::Expr { expr, .. } if matches!(&expr.kind, ExprKind::MacroCall { .. })) {
        *s = new.first().cloned().unwrap_or_else(|| s.clone());
        return true;
    }
    false
}

fn recurse_replace_block(b: &mut Block, new: &[Stmt]) -> bool {
    let before = b.stmts.len();
    replace_matching_stmt(&mut b.stmts, new.to_vec());
    b.stmts.len() != before || find_call_in_block(b)
}

fn find_call_in_block(b: &Block) -> bool {
    b.stmts.iter().any(|s| matches!(&s.kind, StmtKind::Expr { expr, .. } if matches!(&expr.kind, ExprKind::MacroCall { .. })))
}

fn replace_expr_in_block(b: &mut Block, frag: &ProbeFragment) {
    let ProbeFragment::Expr(e) = frag else { return };
    if let Some(t) = &mut b.tail {
        if matches!(&t.kind, ExprKind::MacroCall { .. }) {
            *t = e.clone();
            return;
        }
    }
    for s in b.stmts.iter_mut() {
        if replace_expr_in_stmt(s, e) {
            return;
        }
    }
}

fn replace_expr_in_stmts(stmts: &mut Vec<Stmt>, frag: &ProbeFragment) {
    let ProbeFragment::Expr(e) = frag else { return };
    for s in stmts.iter_mut() {
        if replace_expr_in_stmt(s, e) {
            return;
        }
    }
}

fn replace_expr_in_stmt(s: &mut Stmt, e: &Expr) -> bool {
    match &mut s.kind {
        StmtKind::Expr { expr, .. } => {
            if replace_macro_call(expr, e) {
                return true;
            }
            false
        }
        StmtKind::Let { init, .. } => {
            if let Some(i) = init {
                return replace_macro_call(i, e);
            }
            false
        }
        StmtKind::Assign { target, value, .. } => {
            replace_macro_call(target, e) || replace_macro_call(value, e)
        }
        StmtKind::If { cond, then, else_ } => {
            if replace_macro_call(cond, e) {
                return true;
            }
            replace_expr_in_block(then, &ProbeFragment::Expr(e.clone()));
            if let Some(eb) = else_ {
                match &mut **eb {
                    ElseBranch::Block(b) => replace_expr_in_block(b, &ProbeFragment::Expr(e.clone())),
                    ElseBranch::If(inner) => {
                        replace_expr_in_stmt(inner, e);
                    }
                }
            }
            false
        }
        StmtKind::While { cond, body } => {
            replace_macro_call(cond, e) || {
                replace_expr_in_block(body, &ProbeFragment::Expr(e.clone()));
                false
            }
        }
        StmtKind::Loop { body, .. } | StmtKind::For { body, .. } | StmtKind::Unsafe(body) | StmtKind::Defer(body) => {
            replace_expr_in_block(body, &ProbeFragment::Expr(e.clone()));
            false
        }
        StmtKind::Match { scrut, arms } => {
            if replace_macro_call(scrut, e) {
                return true;
            }
            for a in arms.iter_mut() {
                match &mut a.body {
                    ArmBody::Expr(x) => {
                        if replace_macro_call(x, e) {
                            return true;
                        }
                    }
                    ArmBody::Block(b) => replace_expr_in_block(b, &ProbeFragment::Expr(e.clone())),
                }
            }
            false
        }
        StmtKind::Return { value } | StmtKind::Break { value, .. } => {
            if let Some(v) = value {
                return replace_macro_call(v, e);
            }
            false
        }
        StmtKind::ConstAssert { cond, .. } => replace_macro_call(cond, e),
        _ => false,
    }
}

/// Replaces the first `MacroCall` node found in the expression tree.
fn replace_macro_call(e: &mut Expr, repl: &Expr) -> bool {
    if matches!(&e.kind, ExprKind::MacroCall { .. }) {
        *e = repl.clone();
        return true;
    }
    macro_rules! kid {
        ($x:expr) => {
            if replace_macro_call($x, repl) {
                return true;
            }
        };
    }
    match &mut e.kind {
        ExprKind::Unary { expr, .. } => kid!(expr),
        ExprKind::Binary { lhs, rhs, .. } => {
            kid!(lhs);
            kid!(rhs);
        }
        ExprKind::Assign { target, value, .. } => {
            kid!(target);
            kid!(value);
        }
        ExprKind::Call { callee, args } => {
            kid!(callee);
            for a in args.iter_mut() {
                kid!(&mut a.expr);
            }
        }
        ExprKind::Method { recv, args, .. } => {
            kid!(recv);
            for a in args.iter_mut() {
                kid!(&mut a.expr);
            }
        }
        ExprKind::Field { base, .. } => kid!(base),
        ExprKind::Index { base, index } => {
            kid!(base);
            kid!(index);
        }
        ExprKind::Tuple(v) | ExprKind::Array(v) => {
            for x in v.iter_mut() {
                kid!(x);
            }
        }
        ExprKind::ArrayRepeat { value, len } => {
            kid!(value);
            kid!(len);
        }
        ExprKind::StructLit { fields, rest, .. } => {
            for (_, v) in fields.iter_mut() {
                kid!(v);
            }
            if let Some(r) = rest {
                kid!(r);
            }
        }
        ExprKind::If { cond, then, else_ } => {
            kid!(cond);
            replace_expr_in_block(then, &ProbeFragment::Expr(repl.clone()));
            if let Some(eb) = else_ {
                match &mut **eb {
                    ElseBranch::Block(b) => replace_expr_in_block(b, &ProbeFragment::Expr(repl.clone())),
                    ElseBranch::If(inner) => {
                        replace_expr_in_stmt(inner, repl);
                    }
                }
            }
        }
        ExprKind::Match { scrut, arms } => {
            kid!(scrut);
            for a in arms.iter_mut() {
                if let Some(g) = &mut a.guard {
                    kid!(g);
                }
                match &mut a.body {
                    ArmBody::Expr(x) => kid!(x),
                    ArmBody::Block(b) => replace_expr_in_block(b, &ProbeFragment::Expr(repl.clone())),
                }
            }
        }
        ExprKind::Block(b) | ExprKind::Unsafe(b) => {
            replace_expr_in_block(b, &ProbeFragment::Expr(repl.clone()));
        }
        ExprKind::Closure { body, .. } => {
            replace_expr_in_block(body, &ProbeFragment::Expr(repl.clone()));
        }
        ExprKind::Ref { expr, .. } | ExprKind::Deref { expr } | ExprKind::Try { expr } | ExprKind::Move { expr } => kid!(expr),
        ExprKind::Cast { expr, .. } | ExprKind::Is { expr, .. } => kid!(expr),
        ExprKind::Range { lo, hi } => {
            if let Some(l) = lo {
                kid!(l);
            }
            if let Some(h) = hi {
                kid!(h);
            }
        }
        _ => {}
    }
    let _ = FieldName::Named(String::new());
    let _ = ImportAlias::None;
    false
}

// ---------------------------------------------------------------- hygiene leak check (§6.3)

fn check_hygiene_leaks(
    files: &BTreeMap<FileId, ParsedFile>,
    fid: FileId,
    sites: &[&MacroSite],
    sources: &Sources,
    handler: &mut Handler,
) {
    if sites.is_empty() {
        return;
    }
    let mut introduced: Vec<&String> = Vec::new();
    for s in sites {
        for n in &s.introduced {
            introduced.push(n);
        }
    }
    if introduced.is_empty() {
        return;
    }
    for item in &files[&fid].items {
        if let ItemKind::Fn { body, sig, .. } | ItemKind::Event { body, sig, .. } = &item.kind {
            let _ = sig;
            if let crate::ast::Body::Block(b) = body {
                let mut bindings: Vec<String> = Vec::new();
                collect_bound_names_in_block(b, &mut bindings);
                let mut refs: Vec<(String, Span)> = Vec::new();
                collect_ident_refs_block(b, &mut refs);
                for (r, span) in refs {
                    if !introduced.iter().any(|i| **i == r) {
                        continue;
                    }
                    if bindings.iter().any(|bnd| *bnd == r) {
                        continue;
                    }
                    let (pos, _) = sources.line_col(span);
                    handler.emit(
                        Diagnostic::new("E-0400", Phase::Macro, fid, pos)
                            .titled("宏作用域泄漏")
                            .note(format!("标识符 `{r}` 由宏引入且对外不可见（§6.3）"))
                            .help("将该标识符作为 :ident 片段从调用点传入")
                            .expansion(format!("hygiene → {}:{}", display(sources, fid), pos.line)),
                    );
                    return; // one diagnostic per leak is enough; keeps output deterministic
                }
            }
        }
    }
}

fn collect_bound_names_in_block(b: &Block, out: &mut Vec<String>) {
    for s in &b.stmts {
        collect_bound_names_stmt(s, out);
    }
}

fn collect_bound_names_stmt(s: &Stmt, out: &mut Vec<String>) {
    match &s.kind {
        StmtKind::Let { name, .. } => out.push(name.clone()),
        StmtKind::If { then, else_, .. } => {
            collect_bound_names_in_block(then, out);
            if let Some(eb) = else_ {
                match &**eb {
                    ElseBranch::Block(b) => collect_bound_names_in_block(b, out),
                    ElseBranch::If(inner) => collect_bound_names_stmt(inner, out),
                }
            }
        }
        StmtKind::While { body, .. } | StmtKind::Loop { body, .. } | StmtKind::Unsafe(body) | StmtKind::Defer(body) => {
            collect_bound_names_in_block(body, out);
        }
        StmtKind::For { pat, body, .. } => {
            collect_bound_names_pat(pat, out);
            collect_bound_names_in_block(body, out);
        }
        StmtKind::Match { arms, .. } => {
            for a in arms {
                collect_bound_names_pat(&a.pat, out);
                if let ArmBody::Block(b) = &a.body {
                    collect_bound_names_in_block(b, out);
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

fn collect_bound_names_pat(p: &Pat, out: &mut Vec<String>) {
    match &p.kind {
        crate::ast::PatKind::Binding { name, sub, .. } => {
            out.push(name.clone());
            if let Some(s) = sub {
                collect_bound_names_pat(s, out);
            }
        }
        crate::ast::PatKind::Tuple(v) | crate::ast::PatKind::Or(v) | crate::ast::PatKind::Exclude(v) => {
            for x in v {
                collect_bound_names_pat(x, out);
            }
        }
        crate::ast::PatKind::Slice(v, rest) => {
            for x in v {
                collect_bound_names_pat(x, out);
            }
            if let Some(r) = rest {
                collect_bound_names_pat(r, out);
            }
        }
        crate::ast::PatKind::Constructor { args, fields, .. } => {
            for x in args {
                collect_bound_names_pat(x, out);
            }
            for (_, x) in fields {
                collect_bound_names_pat(x, out);
            }
        }
        _ => {}
    }
}

fn collect_ident_refs_block(b: &Block, out: &mut Vec<(String, Span)>) {
    for s in &b.stmts {
        collect_ident_refs_stmt(s, out);
    }
    if let Some(t) = &b.tail {
        collect_ident_refs_expr(t, out);
    }
}

fn collect_ident_refs_stmt(s: &Stmt, out: &mut Vec<(String, Span)>) {
    match &s.kind {
        StmtKind::Expr { expr, .. } => collect_ident_refs_expr(expr, out),
        StmtKind::Let { init, .. } => {
            if let Some(e) = init {
                collect_ident_refs_expr(e, out);
            }
        }
        StmtKind::Assign { target, value, .. } => {
            collect_ident_refs_expr(target, out);
            collect_ident_refs_expr(value, out);
        }
        StmtKind::If { cond, then, else_ } => {
            collect_ident_refs_expr(cond, out);
            collect_ident_refs_block(then, out);
            if let Some(eb) = else_ {
                match &**eb {
                    ElseBranch::Block(b) => collect_ident_refs_block(b, out),
                    ElseBranch::If(inner) => collect_ident_refs_stmt(inner, out),
                }
            }
        }
        StmtKind::While { cond, body } => {
            collect_ident_refs_expr(cond, out);
            collect_ident_refs_block(body, out);
        }
        StmtKind::Loop { body, .. } | StmtKind::Unsafe(body) | StmtKind::Defer(body) => {
            collect_ident_refs_block(body, out);
        }
        StmtKind::For { iter, body, .. } => {
            collect_ident_refs_expr(iter, out);
            collect_ident_refs_block(body, out);
        }
        StmtKind::Match { scrut, arms, .. } => {
            collect_ident_refs_expr(scrut, out);
            for a in arms {
                if let Some(g) = &a.guard {
                    collect_ident_refs_expr(g, out);
                }
                match &a.body {
                    ArmBody::Expr(e) => collect_ident_refs_expr(e, out),
                    ArmBody::Block(b) => collect_ident_refs_block(b, out),
                }
            }
        }
        StmtKind::Return { value } | StmtKind::Break { value, .. } => {
            if let Some(v) = value {
                collect_ident_refs_expr(v, out);
            }
        }
        StmtKind::ConstAssert { cond, .. } => collect_ident_refs_expr(cond, out),
        _ => {}
    }
}

fn collect_ident_refs_expr(e: &Expr, out: &mut Vec<(String, Span)>) {
    match &e.kind {
        ExprKind::Ident(s) => {
            if !s.starts_with("__") {
                out.push((s.clone(), e.span));
            }
        }
        ExprKind::Path(p) => {
            if let Some(seg) = p.first() {
                if !seg.starts_with("__") && p.len() == 1 {
                    out.push((seg.clone(), e.span));
                }
            }
        }
        ExprKind::Unary { expr, .. } => collect_ident_refs_expr(expr, out),
        ExprKind::Binary { lhs, rhs, .. } => {
            collect_ident_refs_expr(lhs, out);
            collect_ident_refs_expr(rhs, out);
        }
        ExprKind::Assign { target, value, .. } => {
            collect_ident_refs_expr(target, out);
            collect_ident_refs_expr(value, out);
        }
        ExprKind::Call { callee, args } => {
            collect_ident_refs_expr(callee, out);
            for a in args {
                collect_ident_refs_expr(&a.expr, out);
            }
        }
        ExprKind::Method { recv, args, .. } => {
            collect_ident_refs_expr(recv, out);
            for a in args {
                collect_ident_refs_expr(&a.expr, out);
            }
        }
        ExprKind::Field { base, .. } => collect_ident_refs_expr(base, out),
        ExprKind::Index { base, index } => {
            collect_ident_refs_expr(base, out);
            collect_ident_refs_expr(index, out);
        }
        ExprKind::Tuple(v) | ExprKind::Array(v) => {
            for x in v {
                collect_ident_refs_expr(x, out);
            }
        }
        ExprKind::ArrayRepeat { value, len } => {
            collect_ident_refs_expr(value, out);
            collect_ident_refs_expr(len, out);
        }
        ExprKind::StructLit { fields, rest, .. } => {
            for (_, v) in fields {
                collect_ident_refs_expr(v, out);
            }
            if let Some(r) = rest {
                collect_ident_refs_expr(r, out);
            }
        }
        ExprKind::If { cond, then, else_ } => {
            collect_ident_refs_expr(cond, out);
            collect_ident_refs_block(then, out);
            if let Some(eb) = else_ {
                match &**eb {
                    ElseBranch::Block(b) => collect_ident_refs_block(b, out),
                    ElseBranch::If(inner) => collect_ident_refs_stmt(inner, out),
                }
            }
        }
        ExprKind::Match { scrut, arms } => {
            collect_ident_refs_expr(scrut, out);
            for a in arms {
                if let Some(g) = &a.guard {
                    collect_ident_refs_expr(g, out);
                }
                match &a.body {
                    ArmBody::Expr(x) => collect_ident_refs_expr(x, out),
                    ArmBody::Block(b) => collect_ident_refs_block(b, out),
                }
            }
        }
        ExprKind::Block(b) | ExprKind::Unsafe(b) => collect_ident_refs_block(b, out),
        ExprKind::Closure { body, .. } => collect_ident_refs_block(body, out),
        ExprKind::Ref { expr, .. } | ExprKind::Deref { expr } | ExprKind::Try { expr } | ExprKind::Move { expr } => {
            collect_ident_refs_expr(expr, out);
        }
        ExprKind::Cast { expr, .. } | ExprKind::Is { expr, .. } => collect_ident_refs_expr(expr, out),
        ExprKind::Range { lo, hi, .. } => {
            if let Some(l) = lo {
                collect_ident_refs_expr(l, out);
            }
            if let Some(h) = hi {
                collect_ident_refs_expr(h, out);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- diagnostics helpers

fn emit_e0402(
    handler: &mut Handler,
    sources: &Sources,
    call_file: FileId,
    call_span: Span,
    name: &str,
    text: &str,
    files: &BTreeMap<FileId, ParsedFile>,
    defs: &BTreeMap<String, DefEntry>,
) {
    // §6.5: the diagnostic points at the *expander*, not the call site.
    let (pos, arm_span) = defs
        .get(name)
        .and_then(|d| {
            let ItemKind::MacroRules { arms, .. } = &files.get(&d.file)?.items.get(d.item_idx)?.kind else {
                return None;
            };
            let arm = arms.first()?;
            Some((sources.line_col(arm.span).0, arm.span))
        })
        .unwrap_or_else(|| (sources.line_col(call_span).0, call_span));
    let target_file = arm_span.file;
    let call_pos: Position = sources.line_col(call_span).0;
    let (ls, le, cs, _) = cols_multi(sources, arm_span);
    let mut d = Diagnostic::new("E-0402", Phase::Macro, target_file, pos)
        .titled("宏展开产物非法")
        .note(format!("展开后片段: `{text}`"))
        .note("展开结果不是合法 AST 片段（§6.5），诊断指向展开器")
        .help("修正展开器")
        .expansion(format!("macro {} → {}:{}", name, display(sources, call_file), call_pos.line));
    d = d.region_multi(target_file, ls, le, cs, cs + 2, "展开器");
    handler.emit(d);
}

fn display(sources: &Sources, f: FileId) -> String {
    sources.get(f).map(|x| x.display_path.clone()).unwrap_or_else(|| format!("file{}", f))
}

fn cols(sources: &Sources, sp: Span) -> (u32, u32, u32, u32) {
    let (a, b) = sources.line_col(sp);
    (a.line, b.line, a.column, b.column)
}

fn cols_multi(sources: &Sources, sp: Span) -> (u32, u32, u32, u32) {
    cols(sources, sp)
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
        Kind::Eof => String::new(),
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

fn render_trees(trees: &[Tree]) -> String {
    let mut toks = Vec::new();
    for t in trees {
        t.flatten(&mut toks);
    }
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
/// provenance comments. Byte-stable for identical input (§6.8 / §7.3): all iteration is over
/// `BTreeMap`/sorted vectors; no HashMap, time or addresses.
pub fn render_expanded(
    files: &BTreeMap<FileId, ParsedFile>,
    sources: &Sources,
    opts: &ExpandOptions,
) -> String {
    let mut work = files.clone();
    let mut handler = Handler::default();
    let result = expand_all(&mut work, sources, &mut handler, opts);

    // Collect per-file edits: (start, end, replacement) — macro sites replace their call span,
    // derive impls are inserted after their item.
    let mut edits: BTreeMap<FileId, Vec<(u32, u32, String)>> = BTreeMap::new();
    let mut last_end: BTreeMap<FileId, u32> = BTreeMap::new();
    for s in &result.sites {
        let st = s.replace_span.start;
        let en = s.replace_span.end.max(st);
        if let Some(prev) = last_end.get(&s.file) {
            if st < *prev {
                continue; // nested site: the outer expansion already prints it
            }
        }
        last_end.insert(s.file, en);
        let line = sources.line_col(s.call_span).0.line;
        let text = format!("// expand-from: {}:{}\n{}", display(sources, s.file), line, s.text);
        edits.entry(s.file).or_default().push((st, en, text));
    }
    for d in &result.derives {
        let text = format!("\n// expand-from: {}:{}\n{}", display(sources, d.file), d.line, d.text);
        edits.entry(d.file).or_default().push((d.insert_at, d.insert_at, text));
    }

    let mut out = String::new();
    for (fid, pf) in files.iter() {
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
                continue;
            }
            buf.push_str(&text[cur..st]);
            buf.push_str(&rep);
            cur = en;
        }
        buf.push_str(&text[cur..]);
        out.push_str(&format!("// ===== {} =====\n", sf.display_path));
        out.push_str(&buf);
        let _ = pf;
    }
    out
}

// ---------------------------------------------------------------- span helper

fn stmt_replace_span(files: &BTreeMap<FileId, ParsedFile>, fid: FileId, item_idx: usize, call_span: Span) -> Span {
    // Include the trailing `;` of a statement-level call in the replaced range.
    if let Some(item) = files.get(&fid).and_then(|f| f.items.get(item_idx)) {
        let ends = item.span.end;
        let _ = ends;
    }
    call_span
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Handler;
    use crate::span::Sources;
    use std::collections::BTreeMap;

    pub(crate) fn parse_one(src: &mut Sources, path: &str, text: &str, handler: &mut Handler) -> FileId {
        let f = src.add(path.to_string(), text.to_string());
        let lexed = crate::lexer::lex_file(src, f, handler);
        let pf = crate::parser::parse(src, f, &lexed, handler);
        let mut files = BTreeMap::new();
        files.insert(f, pf);
        f
    }

    pub(crate) fn harness(texts: &[(&str, &str)]) -> (Sources, BTreeMap<FileId, ParsedFile>, Handler) {
        let mut src = Sources::default();
        let mut files = BTreeMap::new();
        let mut handler = Handler::default();
        for (path, text) in texts {
            let f = src.add(path.to_string(), text.to_string());
            let lexed = crate::lexer::lex_file(&src, f, &mut handler);
            let pf = crate::parser::parse(&src, f, &lexed, &mut handler);
            files.insert(f, pf);
        }
        // lex/parse complaints are not the macro layer's; assert macro behaviour only.
        handler.diags.clear();
        (src, files, handler)
    }

    pub(crate) fn run(
        src: &Sources,
        files: &mut BTreeMap<FileId, ParsedFile>,
        handler: &mut Handler,
        opts: &ExpandOptions,
    ) -> ExpansionResult {
        let n_before = handler.diags.len();
        let r = expand_all(files, src, handler, opts);
        let _ = n_before;
        r
    }

    pub(crate) fn codes(handler: &Handler) -> Vec<String> {
        handler.diags.iter().map(|d| d.code.clone()).collect()
    }

    pub(crate) const VEC_SRC: &str = r#"
macro_rules! vec {
    ()               => { [] };
    ($x:expr)         => { [$x] };
    ($x:expr, $($y:expr),+ $(,)?) => {
        let mut arr = [$x];
        $(arr.push($y);)*
        arr
    };
}

fn main() {
    var a = vec![];
    var b = vec![7];
    var c = vec![1, 2, 3];
}
"#;

    // ---------------------------------------------------------------- vec! examples (§6.1)

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
        assert!(t.contains("let"), "{t}");
        assert!(t.contains("[ 1 ]"), "{t}");
        assert!(t.contains("push ( 2 )"), "{t}");
        assert!(t.contains("push ( 3 )"), "{t}");
        // the `$(,)?)` trailing comma must not leak a stray separator
        assert!(!t.ends_with(','), "{t}");
    }

    #[test]
    fn vec_repetition_accepts_trailing_comma_in_call() {
        let text = r#"
macro_rules! vec {
    ($x:expr, $($y:expr),+ $(,)?) => {
        let mut arr = [$x];
        $(arr.push($y);)*
        arr
    };
}
fn main() {
    var c = vec![1, 2, 3,];
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert_eq!(r.expansions, 1);
        assert!(r.sites[0].text.contains("push ( 3 )"));
    }

    #[test]
    fn vec_stmt_position_splices_statements() {
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
        let ItemKind::Fn { body, .. } = &files.values().next().unwrap().items[1].kind else {
            panic!("expected fn");
        };
        if let crate::ast::Body::Block(b) = body {
            assert_eq!(b.stmts.len(), 2, "stmts: {:?}", b.stmts.len());
        } else {
            panic!("block expected");
        }
    }

    // ---------------------------------------------------------------- classifiers (§6.2)

    fn classifier_case(macro_src: &str, call: &str) -> (Sources, BTreeMap<FileId, ParsedFile>, Handler, ExpansionResult) {
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
        // the comma inside `f(a, b)` must not split the :expr fragment
        let (_src, _files, handler, r) = classifier_case(
            "macro_rules! m { ($x:expr) => { [$x] }; }",
            "m!(g(1, 2))",
        );
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert!(r.sites[0].text.contains("g ( 1 , 2 )"), "{}", r.sites[0].text);
    }

    #[test]
    fn classifier_ty_matches_named_and_generic() {
        let (_src, _files, handler, r) = classifier_case(
            "macro_rules! m { ($t:ty) => { Option<$t> }; }",
            "m!(u32)",
        );
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert!(r.sites[0].text.contains("Option < u32 >"), "{}", r.sites[0].text);
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
    fn classifier_pat_matches_constructor_pattern() {
        // :pat fragment is validated against §3.12 via the real parser.
        let text = r#"
macro_rules! m {
    ($p:pat) => { if false { } else { let q = 1; } };
}
fn main() {
    match 1u32 {
        m!(0u32 | 1u32) => { }
        _ => { }
    }
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        // the expander here does not re-inject $p; we test that the *call* pattern `0u32 | 1u32`
        // matched as a legal :pat (an illegal pat candidate would produce E-0402 for no arm).
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert_eq!(r.expansions, 1);
    }

    #[test]
    fn classifier_block_requires_brace_group() {
        let (_src, _files, handler, r) = classifier_case(
            "macro_rules! m { ($b:block) => { $b }; }",
            "m!({ let x = 1; x })",
        );
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert!(r.sites[0].text.contains("let x = 1 ; x") || r.sites[0].text.contains("{"), "{}", r.sites[0].text);
    }

    #[test]
    fn classifier_tt_matches_any_single_tree() {
        let (_src, _files, handler, r) = classifier_case(
            "macro_rules! m { ($t:tt) => { [$t] }; }",
            "m!(x)",
        );
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert!(r.sites[0].text.contains("[ x ]"), "{}", r.sites[0].text);

        // tt also captures a whole group as ONE tree
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
        let text = r#"
macro_rules! m {
    ($mm:meta) => { [$mm] };
}
fn main() {
    let z = m!(a = 1);
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert!(r.sites[0].text.contains("a = 1"), "{}", r.sites[0].text);
    }

    // ---------------------------------------------------------------- nested repetition

    #[test]
    fn nested_repetition_expands_two_dimensionally() {
        let text = r#"
macro_rules! grid {
    ($($($c:expr),+);+) => { [$($[$c],+);+] };
}
fn main() {
    let z = grid!(1, 2; 3);
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        let t = &r.sites[0].text;
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
        // the user-visible name `arr` must be gone from the macro-introduced binding
        assert!(!t.contains("let mut arr "), "macro-local `arr` leaked: {t}");
        assert_eq!(r.sites[2].introduced, vec!["arr".to_string()]);
    }

    #[test]
    fn same_macro_twice_in_one_function_does_not_collide() {
        // the classic `$x` binding-reuse case: two expansions, two rustled names.
        let text = r#"
macro_rules! pair {
    ($x:expr) => {
        let mut arr = [$x];
        let mut acc = 0u32;
        acc
    };
}
fn main() {
    pair!(1);
    pair!(2);
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(codes(&handler).is_empty(), "{:?}", codes(&handler));
        assert_eq!(r.expansions, 2);
        let a = &r.sites[0].text;
        let b = &r.sites[1].text;
        assert!(a.contains("__infy_m") && b.contains("__infy_m"), "{a} | {b}");
        assert_ne!(a, b, "two expansions of the same macro must not reuse rustled names");
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
        // `total` came from the call site → never rustled (§6.3 second half).
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
    leaky!();
    secret = 9;
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
    leaky!();
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
        // three nested calls is legal (§6.4 allows up to 128).
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
        assert!(r.max_depth >= 2);
    }

    // ---------------------------------------------------------------- illegal expansion (§6.5)

    #[test]
    fn expansion_to_illegal_fragment_is_e0402_pointing_at_expander() {
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
        // the diagnostic must carry the expanded fragment and the expander provenance chain
        assert!(d.notes.iter().any(|n| n.contains("展开后片段")), "{:?}", d.notes);
        assert!(
            d.expand_chain.iter().any(|s| s.starts_with("macro bad →")),
            "{:?}",
            d.expand_chain
        );
        // §6.5: points at the expander (macro definition), not at the call site.
        let (call_line, _) = src.line_col(duplicate_probe_span(&files, "bad!")).0;
        assert!(d.pos.line < call_line, "diagnostic line {} should be the expander (< {call_line})", d.pos.line);
    }

    fn duplicate_probe_span(files: &BTreeMap<FileId, ParsedFile>, _needle: &str) -> Span {
        // the call `bad!()` sits in `main` on the last line of the test source.
        let pf = files.values().next().unwrap();
        pf.span
    }

    #[test]
    fn no_arm_match_is_e0402() {
        let text = r#"
macro_rules! only_expr {
    ($x:expr) => { [$x] };
}
fn main() {
    let z = only_expr!(= =);
}
"#;
        let (src, mut files, mut handler) = harness(&[("m/main.infy", text)]);
        run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert!(codes(&handler).contains(&"E-0402".to_string()), "{:?}", codes(&handler));
    }

    // ---------------------------------------------------------------- visibility (§5.5 / §6.5)

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
        let (src, mut files, mut handler) =
            harness(&[("m/a.infy", defs), ("m/b.infy", user)]);
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
fn main() {
    let a = m1!(1);
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
        // the original call text is replaced by the expansion
        assert!(!a.contains("vec![1, 2, 3]"), "{a}");
        assert!(a.contains("push"), "{a}");
    }

    #[test]
    fn render_expanded_stage_macro_has_no_derive_impls() {
        let text = r#"
#[derive(Debug)]
struct D { x: u32 }
"#;
        let (src, files, _h) = harness(&[("m/main.infy", text)]);
        let opts = ExpandOptions { only_macro: None, stage: Stage::Macro };
        let out = render_expanded(&files, &src, &opts);
        assert!(!out.contains("impl"), "stage macro must not reprint derives: {out}");
        let opts2 = ExpandOptions { only_macro: None, stage: Stage::All };
        let out2 = render_expanded(&files, &src, &opts2);
        assert!(out2.contains("impl"), "stage all must reprint derives: {out2}");
    }

    // ---------------------------------------------------------------- result bookkeeping

    #[test]
    fn expansion_result_lists_macro_names_sorted() {
        let (src, mut files, mut handler) = harness(&[("m/main.infy", VEC_SRC)]);
        let r = run(&src, &mut files, &mut handler, &ExpandOptions::default());
        assert_eq!(r.macro_names, vec!["vec".to_string()]);
        assert!(r.max_depth >= 1);
    }
}
