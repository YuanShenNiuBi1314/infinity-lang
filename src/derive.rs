//! §6.7 — `#[derive]` code generation, and §6.6 — CLOSED attribute-list validation.
//!
//! Derive products' semantics are fixed by the spec table, row by row:
//! * field order = declaration order (permanent, golden-file normative — §6.7 `Debug` row);
//! * `Debug` prints `Name { f1: v1, f2: v2 }`, the tuple form `Name(f1, f2)` for `#[repr(C)]`,
//!   `<redacted>` for `#[redact]` fields;
//! * `PartialEq` short-circuits in declaration order; `#[compare(skip)]` fields are excluded;
//!   float fields may derive `PartialEq`, but `Eq`/`Ord` need an explicit `#[allow_nan_equality]`
//!   (`E-0424`);
//! * `Eq` adds no code — marker check only (`E-0425`);
//! * `PartialOrd`/`Ord` are strict lexicographic by declaration order, `#[order(asc)]` /
//!   `#[order(desc)]` per field;
//! * `Hash` feeds `field_discriminant_i32` then every field through **xxHash64** — fixed for
//!   v2.0; changing the algorithm would be a MAJOR release event (§6.7 `Hash` row);
//! * `Default`: numeric `0`, `bool false`, `char '\0'`, `str ""`, `[T;N]` element-wise,
//!   `Option → None`, `Result → Err(E::default())`, enum → first variant (`E-0422` when its
//!   fields lack `Default`) unless a `#[default]` variant is chosen (multiple → `E-0423`);
//! * `#[skip_clone]` fields must be `Default` (`E-0420`);
//! * `Copy` is rejected for `Drop` types / `dyn` fields / non-`Copy` fields (`E-0421`);
//! * `HasField` emits the reflection table — field names, type-tag strings, offsets only for
//!   `#[repr(C)]`;
//! * generated code that would have to touch raw pointers fails with `E-0427` instead of
//!   silently inserting `unsafe`.
//!
//! Products are real AST `ItemKind::Impl` nodes (§6.8: derive output must itself pass
//! `infy check`), and the `infy expand` reprint is produced by the deterministic printer in this
//! module (`p_impl` & friends), so reprint and AST cannot diverge.
//!
//! `expand_derives` cannot know display line numbers (its signature carries no `Sources`); those
//! diagnostics fall back to `1:1` positioning — code, title and phase stay normative.
//! `validate_attrs` receives `&Sources` and points precisely.

use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{
    Attr, ArmBody, Block, Bound, Docs, ElseBranch, Expr, ExprKind, Field, FieldName, GenericParam,
    Item, ItemKind, MatchArm, Param, Pat, ParsedFile, Prim, Stmt, StmtKind, Type, TypeKind,
    Visibility,
};
use crate::diag::{Diagnostic, Handler, Phase};
use crate::span::{FileId, Sources, Span};
use crate::token::{IntLit, Lit, Punct, Radix};

/// §6.7: the closed derive set.
pub const CLOSED_DERIVES: [&str; 10] = [
    "Clone", "Copy", "Default", "Debug", "PartialEq", "Eq", "PartialOrd", "Ord", "Hash", "HasField",
];

/// The CLOSED attribute list implemented as the **union** of §6.6's table (lines 987–1002) and
/// §6.7's prose names — the two sections disagree about which names are legal; the union is the
/// conservative implementation (reported upstream). `#[cfg]` stays illegal per §F.1.
pub const CLOSED_ATTRS: &[&str] = &[
    // §6.6 table
    "infy_version", "entry_chain", "test", "bench", "repr", "align", "link", "owns", "borrows",
    "export", "fast_math", "no_drop", "assume_typed", "deprecated", "doc",
    // §6.7 derive machinery
    "derive", "default", "redact", "order", "compare", "skip_clone", "allow_nan_equality",
    // §6.7 prose effect/ownership names
    "panicable", "effectful", "must_use", "must_drop", "owned",
];

/// Attributes legal on fields (§6.7) and variants (`#[default]`).
const FIELD_ALLOWED: &[&str] = &[
    "redact", "default", "order", "compare", "skip_clone", "doc", "deprecated", "must_use",
    "must_drop",
];

// ---------------------------------------------------------------- xxHash64 (§6.7 Hash row)

const XXH64_P1: u64 = 0x9E37_79B1_85EB_CA87;
const XXH64_P2: u64 = 0xC2B2_AE3D_27D4_EB4F;
const XXH64_P3: u64 = 0x1656_67B1_9E37_79F9;
const XXH64_P4: u64 = 0x85EB_CA77_C2B2_AE63;
const XXH64_P5: u64 = 0x27D4_EB2F_1656_67C5;

/// Reference xxHash64, implemented exactly. **Normative (v2.0)**: the `Hash` derive feeds through
/// this function; any algorithm change is a MAJOR release event (§6.7 `Hash` row).
pub fn xxhash64(data: &[u8]) -> u64 {
    xxhash64_with_seed(data, 0)
}

/// xxHash64 with an explicit seed.
pub fn xxhash64_with_seed(data: &[u8], seed: u64) -> u64 {
    fn read64(b: &[u8], i: usize) -> u64 {
        let mut b8 = [0u8; 8];
        b8.copy_from_slice(&b[i..i + 8]);
        u64::from_le_bytes(b8)
    }
    fn read32(b: &[u8], i: usize) -> u32 {
        let mut b4 = [0u8; 4];
        b4.copy_from_slice(&b[i..i + 4]);
        u32::from_le_bytes(b4)
    }
    fn round(acc: u64, lane: u64) -> u64 {
        acc.wrapping_add(lane.wrapping_mul(XXH64_P2))
            .rotate_left(31)
            .wrapping_mul(XXH64_P1)
    }
    fn merge(acc: u64, lane: u64) -> u64 {
        (acc ^ round(0, lane)).wrapping_mul(XXH64_P1).wrapping_add(XXH64_P4)
    }
    let n = data.len();
    let mut h: u64;
    let mut i = 0usize;
    if n >= 32 {
        let mut v1 = seed.wrapping_add(XXH64_P1).wrapping_add(XXH64_P2);
        let mut v2 = seed.wrapping_add(XXH64_P2);
        let mut v3 = seed;
        let mut v4 = seed.wrapping_sub(XXH64_P1);
        while n - i >= 32 {
            v1 = round(v1, read64(data, i));
            v2 = round(v2, read64(data, i + 8));
            v3 = round(v3, read64(data, i + 16));
            v4 = round(v4, read64(data, i + 24));
            i += 32;
        }
        h = v1
            .rotate_left(1)
            .wrapping_add(v2.rotate_left(7))
            .wrapping_add(v3.rotate_left(12))
            .wrapping_add(v4.rotate_left(18));
        h = merge(h, v1);
        h = merge(h, v2);
        h = merge(h, v3);
        h = merge(h, v4);
    } else {
        h = seed.wrapping_add(XXH64_P5);
    }
    h = h.wrapping_add(n as u64);
    // XXH64_finalize: last ≤31 bytes, 8-lanes via k1, then one 4-lane, then single bytes.
    while n - i >= 8 {
        let k1 = round(0, read64(data, i));
        i += 8;
        h ^= k1;
        h = h.rotate_left(27).wrapping_mul(XXH64_P1).wrapping_add(XXH64_P4);
    }
    if n - i >= 4 {
        h ^= (read32(data, i) as u64).wrapping_mul(XXH64_P1);
        h = h.rotate_left(23).wrapping_mul(XXH64_P2).wrapping_add(XXH64_P3);
        i += 4;
    }
    while i < n {
        h ^= (data[i] as u64).wrapping_mul(XXH64_P5);
        h = h.rotate_left(11).wrapping_mul(XXH64_P1);
        i += 1;
    }
    h ^= h >> 33;
    h = h.wrapping_mul(XXH64_P2);
    h ^= h >> 29;
    h = h.wrapping_mul(XXH64_P3);
    h ^= h >> 32;
    h
}

/// §6.7 `Hash` row: the discriminant fed before the fields; fixed i32 semantics.
pub fn field_discriminant_i32(index: u32) -> i32 {
    index as i32
}

/// The normative Hash fold: discriminant first (declaration order), then every field hash through
/// the fixed mix. Changing any constant here is a MAJOR event.
pub fn derive_hash_u64(discriminant: u32, field_hashes: &[u64]) -> u64 {
    let mut h = xxhash64(&discriminant.to_le_bytes());
    for fh in field_hashes {
        h ^= *fh;
        h = h.rotate_left(23).wrapping_mul(XXH64_P2).wrapping_add(XXH64_P3);
    }
    h ^= h >> 31;
    h = h.wrapping_mul(XXH64_P4);
    h ^= h >> 27;
    h
}

/// Hash one field exactly the way generated code does (bytes → xxHash64).
pub fn hash_field_bytes(bytes: &[u8]) -> u64 {
    xxhash64(bytes)
}

// ---------------------------------------------------------------- derive lists

/// Ordered trait names of `#[derive(...)]` on `item` (written order; duplicates preserved —
/// duplicates are the `E-0426` condition detected by [`expand_derives`]).
pub fn derive_list(item: &Item) -> Vec<String> {
    let mut out = Vec::new();
    for a in &item.attrs {
        if a.path.last().map(|s| s.as_str()) == Some("derive") {
            for n in a.idents() {
                out.push(n);
            }
        }
    }
    out
}

// ---------------------------------------------------------------- AST constructors

fn ident(n: &str, span: Span) -> Expr {
    Expr::new(ExprKind::Ident(n.to_string()), span)
}

fn path_expr(segs: &[&str], span: Span) -> Expr {
    Expr::new(ExprKind::Path(segs.iter().map(|x| x.to_string()).collect()), span)
}

fn field_expr(base: Expr, name: FieldName, span: Span) -> Expr {
    Expr::new(ExprKind::Field { base: Box::new(base), name }, span)
}

fn self_expr(span: Span) -> Expr {
    path_expr(&["self"], span)
}

fn self_named(f: &str, span: Span) -> Expr {
    field_expr(self_expr(span), FieldName::Named(f.to_string()), span)
}

fn self_tuple(i: u32, span: Span) -> Expr {
    field_expr(self_expr(span), FieldName::Index(i), span)
}

fn other_named(f: &str, span: Span) -> Expr {
    field_expr(ident("other", span), FieldName::Named(f.to_string()), span)
}

fn other_tuple(i: u32, span: Span) -> Expr {
    field_expr(ident("other", span), FieldName::Index(i), span)
}

fn lit_str(s: &str, span: Span) -> Expr {
    Expr::new(ExprKind::Lit(Lit::Str(s.to_string())), span)
}

fn lit_u(v: u128, span: Span) -> Expr {
    Expr::new(
        ExprKind::Lit(Lit::Int(IntLit { digits: v.to_string(), radix: Radix::Dec, suffix: None, value: Some(v) })),
        span,
    )
}

fn lit_i(v: i64, span: Span) -> Expr {
    if v < 0 {
        Expr::new(ExprKind::Unary { op: Punct::Minus, expr: Box::new(lit_u((-v) as u128, span)) }, span)
    } else {
        lit_u(v as u128, span)
    }
}

fn lit_bool(b: bool, span: Span) -> Expr {
    Expr::new(ExprKind::Lit(Lit::Bool(b)), span)
}

fn call(callee: Expr, args: Vec<Expr>, span: Span) -> Expr {
    let argv = args.into_iter().map(|e| crate::ast::CallArg { name: None, expr: e, span }).collect();
    Expr::new(ExprKind::Call { callee: Box::new(callee), args: argv }, span)
}

fn call_path(segs: &[&str], args: Vec<Expr>, span: Span) -> Expr {
    call(path_expr(segs, span), args, span)
}

fn bin(op: Punct, l: Expr, r: Expr, span: Span) -> Expr {
    Expr::new(ExprKind::Binary { op, lhs: Box::new(l), rhs: Box::new(r) }, span)
}

fn block(stmts: Vec<Stmt>, span: Span) -> Block {
    Block { stmts, tail: None, span }
}

fn let_stmt(name: &str, init: Expr, span: Span) -> Stmt {
    Stmt {
        kind: StmtKind::Let { is_var: false, name: name.to_string(), name_span: span, ty: None, init: Some(init) },
        attrs: Vec::new(),
        docs: Docs::default(),
        span,
        terminated: true,
    }
}

fn ret_stmt(v: Option<Expr>, span: Span) -> Stmt {
    Stmt { kind: StmtKind::Return { value: v }, attrs: Vec::new(), docs: Docs::default(), span, terminated: true }
}

fn expr_stmt(e: Expr, span: Span) -> Stmt {
    Stmt {
        kind: StmtKind::Expr { expr: e, linkage: Vec::new() },
        attrs: Vec::new(),
        docs: Docs::default(),
        span,
        terminated: true,
    }
}

fn if_stmt(cond: Expr, then: Vec<Stmt>, span: Span) -> Stmt {
    Stmt {
        kind: StmtKind::If { cond, then: block(then, span), else_: None },
        attrs: Vec::new(),
        docs: Docs::default(),
        span,
        terminated: false,
    }
}

fn param(name: &str, ty: Type, span: Span) -> Param {
    Param { mutable: false, name: name.to_string(), ty, default: None, span, name_span: span, is_wildcard: false }
}

fn ty_named(segs: &[&str], span: Span) -> Type {
    Type::named(segs.iter().map(|x| x.to_string()).collect(), Vec::new(), span)
}

fn fn_item(name: &str, params: Vec<Param>, ret: Option<Type>, stmts: Vec<Stmt>, span: Span) -> Item {
    Item {
        attrs: Vec::new(),
        docs: Docs::default(),
        vis: Visibility::Private,
        span,
        kind: ItemKind::Fn {
            sig: crate::ast::FnSig {
                name: name.to_string(),
                name_span: span,
                generics: Vec::new(),
                params,
                ret,
                where_preds: Vec::new(),
                is_unsafe: false,
                span,
            },
            body: crate::ast::Body::Block(block(stmts, span)),
        },
    }
}

fn binding_pat(name: &str, span: Span) -> Pat {
    Pat { kind: crate::ast::PatKind::Binding { name: name.to_string(), by_ref: false, mutable: false, sub: None }, span }
}

fn match_scrut_self_other(span: Span) -> Expr {
    Expr::new(ExprKind::Tuple(vec![self_expr(span), ident("other", span)]), span)
}

// ---------------------------------------------------------------- deterministic printer
//
// The printer covers exactly the node shapes generated here. It never panics on other shapes and
// is byte-stable (§6.8/§7.3).

fn p_ty(t: &Type) -> String {
    match &t.kind {
        TypeKind::Prim(p) => p.text().to_string(),
        TypeKind::Named { path, args } => {
            let base = path.join("::");
            if args.is_empty() {
                base
            } else {
                let a: Vec<String> = args.iter().map(p_ty).collect();
                format!("{}<{}>", base, a.join(", "))
            }
        }
        TypeKind::Ref { lifetime, mutable, inner } => {
            let lt = lifetime.as_ref().map(|l| format!("'{} ", l)).unwrap_or_default();
            format!("&{}{}{}", lt, if *mutable { "mut " } else { "" }, p_ty(inner))
        }
        TypeKind::Ptr { mutable, inner } => format!("*{}{}", if *mutable { "mut " } else { "const " }, p_ty(inner)),
        TypeKind::Array { inner, len } => format!("[{}; {}]", p_ty(inner), p_expr(len)),
        TypeKind::Slice { inner } => format!("[{}]", p_ty(inner)),
        TypeKind::Dyn { path, .. } => format!("dyn {}", path.join("::")),
        TypeKind::FnPtr { params, ret } => {
            let ps: Vec<String> = params.iter().map(p_ty).collect();
            let r = ret.as_ref().map(|t| format!(" -> {}", p_ty(t))).unwrap_or_default();
            format!("fn({}){}", ps.join(", "), r)
        }
        TypeKind::Tuple(v) => {
            let xs: Vec<String> = v.iter().map(p_ty).collect();
            format!("({})", xs.join(", "))
        }
        TypeKind::Never => "!".to_string(),
        TypeKind::Invalid => "?".to_string(),
    }
}

fn p_lit_char(c: char) -> String {
    match c {
        '\0' => "'\\0'".to_string(),
        '\'' => "'\\''".to_string(),
        '\\' => "'\\\\'".to_string(),
        '\n' => "'\\n'".to_string(),
        c => format!("'{}'", c),
    }
}

pub(crate) fn p_expr(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Lit(l) => match l {
            Lit::Str(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
            Lit::Char(c) => p_lit_char(*c),
            Lit::Bool(b) => (if *b { "true" } else { "false" }).to_string(),
            Lit::Int(i) => {
                let s = format!("{}{}", i.digits, i.suffix.clone().unwrap_or_default());
                s
            }
            Lit::Float(f) => format!(
                "{}{}",
                f.mantissa,
                f.exp.as_ref().map(|e| format!("e{}", e)).unwrap_or_default()
            ),
            Lit::ByteStr(_) => "b\"…\"".to_string(),
            Lit::Cord(_) => "cord…".to_string(),
        },
        ExprKind::Ident(s) => s.clone(),
        ExprKind::Path(p) => p.join("::"),
        ExprKind::Unary { op, expr } => format!("{}{}", crate::macro_expand::punct_text(*op), p_expr(expr)),
        ExprKind::Binary { op, lhs, rhs } => format!("({} {} {})", p_expr(lhs), crate::macro_expand::punct_text(*op), p_expr(rhs)),
        ExprKind::Call { callee, args } => {
            let a: Vec<String> = args.iter().map(|c| p_expr(&c.expr)).collect();
            format!("{}({})", p_expr(callee), a.join(", "))
        }
        ExprKind::Method { recv, name, args, .. } => {
            let a: Vec<String> = args.iter().map(|c| p_expr(&c.expr)).collect();
            format!("{}.{}({})", p_expr(recv), name, a.join(", "))
        }
        ExprKind::Field { base, name } => {
            let n = match name {
                FieldName::Named(s) => s.clone(),
                FieldName::Index(i) => i.to_string(),
            };
            format!("{}.{}", p_expr(base), n)
        }
        ExprKind::Index { base, index } => format!("{}[{}]", p_expr(base), p_expr(index)),
        ExprKind::Tuple(v) => {
            let xs: Vec<String> = v.iter().map(p_expr).collect();
            format!("({})", xs.join(", "))
        }
        ExprKind::Array(v) => {
            let xs: Vec<String> = v.iter().map(p_expr).collect();
            format!("[{}]", xs.join(", "))
        }
        ExprKind::ArrayRepeat { value, len } => format!("[{}; {}]", p_expr(value), p_expr(len)),
        ExprKind::StructLit { path, fields, rest } => {
            let fs: Vec<String> = fields.iter().map(|(n, e)| format!("{}: {}", n, p_expr(e))).collect();
            let mut parts = fs;
            if let Some(r) = rest {
                parts.push(format!("..{}", p_expr(r)));
            }
            format!("{} {{ {} }}", path.join("::"), parts.join(", "))
        }
        ExprKind::Match { scrut, arms } => {
            let as_: Vec<String> = arms.iter().map(|a| {
                let body = match &a.body {
                    ArmBody::Expr(e) => p_expr(e),
                    ArmBody::Block(b) => p_block(b),
                };
                format!("{} => {}", p_pat(&a.pat), body)
            }).collect();
            format!("match ({}) {{ {} }}", p_expr(scrut), as_.join(", "))
        }
        ExprKind::Block(b) | ExprKind::Unsafe(b) => p_block(b),
        ExprKind::If { cond, then, else_ } => {
            let mut t = format!("if {} {}", p_expr(cond), p_block(then));
            if let Some(eb) = else_ {
                match &**eb {
                    ElseBranch::Block(b) => t = format!("{} else {}", t, p_block(b)),
                    ElseBranch::If(s) => t = format!("{} else {}", t, p_stmt(s)),
                }
            }
            t
        }
        ExprKind::Ref { mutable, expr, .. } => format!("&{}{}", if *mutable { "mut " } else { "" }, p_expr(expr)),
        _ => "?".to_string(),
    }
}

fn p_pat(p: &Pat) -> String {
    match &p.kind {
        crate::ast::PatKind::Wildcard => "_".to_string(),
        crate::ast::PatKind::DotDot => "..".to_string(),
        crate::ast::PatKind::Lit(l) => p_expr(&Expr::new(ExprKind::Lit(l.clone()), p.span)),
        crate::ast::PatKind::Binding { name, .. } => name.clone(),
        crate::ast::PatKind::Path(path) => path.join("::"),
        crate::ast::PatKind::Constructor { path, args, fields, has_rest } => {
            if !fields.is_empty() {
                let fs: Vec<String> = fields.iter().map(|(n, x)| format!("{}: {}", n, p_pat(x))).collect();
                let mut all = fs;
                if *has_rest {
                    all.push("..".to_string());
                }
                format!("{} {{ {} }}", path.join("::"), all.join(", "))
            } else if !args.is_empty() {
                let as_: Vec<String> = args.iter().map(p_pat).collect();
                format!("{}({})", path.join("::"), as_.join(", "))
            } else {
                path.join("::")
            }
        }
        crate::ast::PatKind::Tuple(v) => {
            let xs: Vec<String> = v.iter().map(p_pat).collect();
            format!("({})", xs.join(", "))
        }
        crate::ast::PatKind::Slice(v, rest) => {
            let xs: Vec<String> = v.iter().map(p_pat).collect();
            let r = rest.as_ref().map(|x| format!(", ..{}", p_pat(x))).unwrap_or_default();
            format!("[{}{}]", xs.join(", "), r)
        }
        crate::ast::PatKind::Or(v) => {
            let xs: Vec<String> = v.iter().map(p_pat).collect();
            xs.join(" | ")
        }
        crate::ast::PatKind::Range { lo, hi, inclusive } => format!("{}..{}{}", p_pat(lo), if *inclusive { "=" } else { "" }, p_pat(hi)),
        crate::ast::PatKind::Exclude(v) => {
            let xs: Vec<String> = v.iter().map(p_pat).collect();
            format!("!({})", xs.join(" | "))
        }
        crate::ast::PatKind::Invalid => "?".to_string(),
    }
}

pub(crate) fn p_stmt(s: &Stmt) -> String {
    match &s.kind {
        StmtKind::Let { is_var, name, init, .. } => {
            let kw = if *is_var { "var" } else { "let" };
            let i = init.as_ref().map(p_expr).unwrap_or_default();
            format!("{} {} = {};", kw, name, i)
        }
        StmtKind::Assign { target, op, value } => format!("{} {}= {};", p_expr(target), crate::macro_expand::punct_text(*op), p_expr(value)),
        StmtKind::Expr { expr, .. } => format!("{};", p_expr(expr)),
        StmtKind::If { cond, then, else_ } => {
            let mut t = format!("if {} {}", p_expr(cond), p_block(then));
            if let Some(eb) = else_ {
                match &**eb {
                    ElseBranch::Block(b) => t = format!("{} else {}", t, p_block(b)),
                    ElseBranch::If(s) => t = format!("{} else {}", t, p_stmt(s)),
                }
            }
            t
        }
        StmtKind::Return { value } => match value {
            Some(v) => format!("return {};", p_expr(v)),
            None => "return;".to_string(),
        },
        StmtKind::While { cond, body } => format!("while {} {}", p_expr(cond), p_block(body)),
        StmtKind::Loop { body, .. } => format!("loop {}", p_block(body)),
        StmtKind::Match { scrut, arms } => {
            let as_: Vec<String> = arms.iter().map(|a| {
                let body = match &a.body {
                    ArmBody::Expr(e) => p_expr(e),
                    ArmBody::Block(b) => p_block(b),
                };
                format!("{} => {}", p_pat(&a.pat), body)
            }).collect();
            format!("match ({}) {{ {} }}", p_expr(scrut), as_.join(", "))
        }
        StmtKind::Unsafe(b) => format!("unsafe {}", p_block(b)),
        StmtKind::Defer(b) => format!("defer {}", p_block(b)),
        _ => ";".to_string(),
    }
}

fn p_block(b: &Block) -> String {
    let mut parts: Vec<String> = b.stmts.iter().map(p_stmt).collect();
    if let Some(t) = &b.tail {
        parts.push(p_expr(t));
    }
    format!("{{ {} }}", parts.join(" "))
}

fn p_fn_item(i: &Item) -> String {
    if let ItemKind::Fn { sig, body } = &i.kind {
        let ps: Vec<String> = sig.params.iter().map(|p| format!("{}: {}", p.name, p_ty(&p.ty))).collect();
        let ret = sig.ret.as_ref().map(|t| format!(" -> {}", p_ty(t))).unwrap_or_default();
        let b = match body {
            crate::ast::Body::Block(bl) => p_block(bl),
            _ => "{ }".to_string(),
        };
        return format!("fn {}({}){} {}", sig.name, ps.join(", "), ret, b);
    }
    String::new()
}

/// Print a generated impl item deterministically (the `infy expand --stage derive` product text).
pub fn p_impl(i: &Item) -> String {
    if let ItemKind::Impl { generics, trait_, self_ty, items, .. } = &i.kind {
        let g: Vec<String> = generics
            .iter()
            .map(|gp| {
                if gp.bounds.is_empty() {
                    gp.name.clone()
                } else {
                    let b: Vec<String> = gp.bounds.iter().map(|bd| bd.path.last().cloned().unwrap_or_default()).collect();
                    format!("{}: {}", gp.name, b.join(" + "))
                }
            })
            .collect();
        let gtext = if g.is_empty() { String::new() } else { format!("<{}>", g.join(", ")) };
        let tr = trait_.as_ref().map(|p| p.join("::")).unwrap_or_default();
        let body: Vec<String> = items.iter().map(p_fn_item).collect();
        let btext = if body.is_empty() { "{ }".to_string() } else { format!("{{ {} }}", body.join(" ")) };
        let head = if gtext.is_empty() { format!("impl {}", tr) } else { format!("impl {} {}", gtext, tr) };
        return format!("{} for {} {}", head, p_ty(self_ty), btext);
    }
    String::new()
}

// ---------------------------------------------------------------- package context

/// The per-package facts §6.7's legality checks need (declared types, their derives, `Drop`
/// impls). Built deterministically from the module AST (`BTreeMap`/`BTreeSet` only).
pub struct PackageCtx {
    decls: BTreeMap<String, DeclInfo>,
    drop_impls: BTreeSet<String>,
}

struct DeclInfo {
    derives: Vec<String>,
}

impl PackageCtx {
    pub fn new(files: &BTreeMap<FileId, ParsedFile>) -> PackageCtx {
        let mut decls: BTreeMap<String, DeclInfo> = BTreeMap::new();
        let mut drop_impls: BTreeSet<String> = BTreeSet::new();
        for pf in files.values() {
            for item in &pf.items {
                match &item.kind {
                    ItemKind::Struct { name, .. } | ItemKind::Enum { name, .. } => {
                        decls.entry(name.clone()).or_insert(DeclInfo { derives: derive_list(item) });
                    }
                    ItemKind::Impl { trait_: Some(tr), self_ty, .. }
                        if tr.last().map(|s| s.as_str()) == Some("Drop") =>
                    {
                        if let TypeKind::Named { path, .. } = &self_ty.kind {
                            if let Some(n) = path.last() {
                                drop_impls.insert(n.clone());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        PackageCtx { decls, drop_impls }
    }

    pub fn empty() -> PackageCtx {
        PackageCtx { decls: BTreeMap::new(), drop_impls: BTreeSet::new() }
    }

    fn derives_of(&self, name: &str) -> Vec<String> {
        self.decls.get(name).map(|d| d.derives.clone()).unwrap_or_default()
    }
    fn is_declared(&self, name: &str) -> bool {
        self.decls.contains_key(name)
    }
    fn has_drop(&self, name: &str) -> bool {
        self.drop_impls.contains(name)
    }
}

// ---------------------------------------------------------------- aggregate model

struct Agg<'a> {
    name: &'a str,
    span: Span,
    generics: &'a [GenericParam],
    kind: AggKind<'a>,
    repr_c: bool,
    allow_nan: bool,
}

enum AggKind<'a> {
    Named(&'a [Field]),
    Tuple(&'a [(Visibility, Type)]),
    Unit,
    Enum(&'a [crate::ast::Variant]),
}

impl<'a> Agg<'a> {
    fn from_item(item: &'a Item) -> Option<Agg<'a>> {
        let (name, generics, kind) = match &item.kind {
            ItemKind::Struct { name, generics, fields, tuple, .. } => {
                let k = if !fields.is_empty() {
                    AggKind::Named(fields)
                } else if !tuple.is_empty() {
                    AggKind::Tuple(tuple)
                } else {
                    AggKind::Unit
                };
                (name, generics, k)
            }
            ItemKind::Enum { name, generics, variants, .. } => (name, generics, AggKind::Enum(variants)),
            _ => return None,
        };
        Some(Agg {
            name,
            span: item.span,
            generics,
            kind,
            repr_c: item
                .attrs
                .iter()
                .any(|a| a.path.last().map(|s| s.as_str()) == Some("repr") && a.has_ident("C")),
            allow_nan: item.has_attr("allow_nan_equality"),
        })
    }

    /// (display name, type, field attrs) in declaration order. Enums flatten to `Variant.field`
    /// rows for the aggregate-level legality checks.
    fn field_rows(&self) -> Vec<(String, &'a Type, &'a [Attr])> {
        match self.kind {
            AggKind::Named(fs) => {
                fs.iter().map(|f| (f.name.clone(), &f.ty, &f.attrs as &[Attr])).collect()
            }
            AggKind::Tuple(ts) => ts
                .iter()
                .enumerate()
                .map(|(i, (_, t))| (i.to_string(), t, &[] as &[Attr]))
                .collect(),
            AggKind::Unit => Vec::new(),
            AggKind::Enum(vs) => {
                let mut out = Vec::new();
                for v in vs {
                    for (i, (_, t)) in v.tuple.iter().enumerate() {
                        out.push((format!("{}.{}", v.name, i), t, &[] as &[Attr]));
                    }
                    for f in &v.struct_fields {
                        out.push((format!("{}.{}", v.name, f.name), &f.ty, &f.attrs as &[Attr]));
                    }
                }
                out
            }
        }
    }
}

// ---------------------------------------------------------------- legality helpers

fn ty_has_ptr(t: &Type) -> bool {
    match &t.kind {
        TypeKind::Ptr { .. } => true,
        TypeKind::Ref { inner, .. } | TypeKind::Array { inner, .. } | TypeKind::Slice { inner } => ty_has_ptr(inner),
        TypeKind::Named { args, .. } => args.iter().any(ty_has_ptr),
        TypeKind::Tuple(v) => v.iter().any(ty_has_ptr),
        _ => false,
    }
}

fn ty_has_dyn(t: &Type) -> bool {
    match &t.kind {
        TypeKind::Dyn { .. } => true,
        TypeKind::Ref { inner, .. } | TypeKind::Array { inner, .. } | TypeKind::Slice { inner } => ty_has_dyn(inner),
        TypeKind::Named { args, .. } => args.iter().any(ty_has_dyn),
        TypeKind::Tuple(v) => v.iter().any(ty_has_dyn),
        _ => false,
    }
}

fn ty_has_float(t: &Type) -> bool {
    match &t.kind {
        TypeKind::Prim(p) => p.is_float(),
        TypeKind::Ref { inner, .. } | TypeKind::Array { inner, .. } | TypeKind::Slice { inner } => ty_has_float(inner),
        TypeKind::Named { args, .. } => args.iter().any(ty_has_float),
        TypeKind::Tuple(v) => v.iter().any(ty_has_float),
        _ => false,
    }
}

/// §6.7 `Copy` row. Unknown named types are conservatively non-`Copy` (stage [4] has not run —
/// §6.5 forbids reading types at stage [2]).
fn type_is_copy(t: &Type, ctx: &PackageCtx, depth: u32) -> bool {
    if depth > 4 {
        return false;
    }
    match &t.kind {
        TypeKind::Prim(p) => !matches!(p, Prim::Str),
        TypeKind::Ref { .. } | TypeKind::Ptr { .. } => true,
        TypeKind::Array { inner, .. } => type_is_copy(inner, ctx, depth + 1),
        TypeKind::Tuple(v) => v.iter().all(|x| type_is_copy(x, ctx, depth + 1)),
        TypeKind::Named { path, args } => {
            let last = path.last().map(|s| s.as_str()).unwrap_or("");
            match last {
                "Option" => args.first().map_or(true, |a| type_is_copy(a, ctx, depth + 1)),
                "Result" => args.iter().all(|a| type_is_copy(a, ctx, depth + 1)),
                _ => {
                    if path.len() == 1 && ctx.is_declared(last) {
                        ctx.derives_of(last).iter().any(|d| d == "Copy")
                    } else {
                        false
                    }
                }
            }
        }
        _ => false,
    }
}

/// §6.7 `Default` row support (E-0420 / E-0422 checks).
fn type_has_default(t: &Type, ctx: &PackageCtx, depth: u32) -> bool {
    if depth > 4 {
        return false;
    }
    default_expr_opt(t, ctx, Span::empty_at(0, 0), depth).is_some()
}

fn type_is_eq(t: &Type, ctx: &PackageCtx, allow_nan: bool, depth: u32) -> bool {
    if depth > 4 {
        return false;
    }
    match &t.kind {
        TypeKind::Prim(p) => !p.is_float() || allow_nan,
        TypeKind::Array { inner, .. } => type_is_eq(inner, ctx, allow_nan, depth + 1),
        TypeKind::Tuple(v) => v.iter().all(|x| type_is_eq(x, ctx, allow_nan, depth + 1)),
        TypeKind::Named { path, args } => {
            let last = path.last().map(|s| s.as_str()).unwrap_or("");
            match last {
                "Option" => args.first().map_or(true, |a| type_is_eq(a, ctx, allow_nan, depth + 1)),
                "Result" => args.iter().all(|a| type_is_eq(a, ctx, allow_nan, depth + 1)),
                _ => {
                    if path.len() == 1 && ctx.is_declared(last) {
                        let d = ctx.derives_of(last);
                        d.iter().any(|x| x == "Eq") || d.iter().any(|x| x == "PartialEq")
                    } else {
                        false
                    }
                }
            }
        }
        _ => false,
    }
}

fn type_is_ord(t: &Type, ctx: &PackageCtx, allow_nan: bool, depth: u32) -> bool {
    if depth > 4 {
        return false;
    }
    match &t.kind {
        TypeKind::Prim(p) => !p.is_float() || allow_nan,
        TypeKind::Array { inner, .. } => type_is_ord(inner, ctx, allow_nan, depth + 1),
        TypeKind::Tuple(v) => v.iter().all(|x| type_is_ord(x, ctx, allow_nan, depth + 1)),
        TypeKind::Named { path, args } => {
            let last = path.last().map(|s| s.as_str()).unwrap_or("");
            match last {
                "Option" => args.first().map_or(true, |a| type_is_ord(a, ctx, allow_nan, depth + 1)),
                "Result" => args.iter().all(|a| type_is_ord(a, ctx, allow_nan, depth + 1)),
                _ => {
                    if path.len() == 1 && ctx.is_declared(last) {
                        ctx.derives_of(last).iter().any(|x| x == "Ord")
                    } else {
                        false
                    }
                }
            }
        }
        _ => false,
    }
}

// ---------------------------------------------------------------- default values (§6.7 row)

/// The `Default` value for a type, or `None` when stage [2] cannot know it.
fn default_expr(t: &Type, ctx: &PackageCtx, span: Span) -> Expr {
    default_expr_opt(t, ctx, span, 0).unwrap_or_else(|| call_path(&["Default", "default"], Vec::new(), span))
}

fn default_expr_opt(t: &Type, ctx: &PackageCtx, span: Span, depth: u32) -> Option<Expr> {
    if depth > 4 {
        return None;
    }
    match &t.kind {
        TypeKind::Prim(p) => Some(match p {
            Prim::Bool => lit_bool(false, span),
            Prim::Char => Expr::new(ExprKind::Lit(Lit::Char('\0')), span),
            Prim::Str => lit_str("", span),
            Prim::Unit => Expr::new(ExprKind::Tuple(Vec::new()), span),
            _ => lit_u(0, span),
        }),
        TypeKind::Array { inner, len } => {
            let d = default_expr_opt(inner, ctx, span, depth + 1)?;
            Some(Expr::new(ExprKind::ArrayRepeat { value: Box::new(d), len: len.clone() }, span))
        }
        TypeKind::Tuple(v) => {
            let mut xs = Vec::new();
            for x in v {
                xs.push(default_expr_opt(x, ctx, span, depth + 1)?);
            }
            Some(Expr::new(ExprKind::Tuple(xs), span))
        }
        TypeKind::Named { path, args } => {
            let last = path.last().map(|s| s.as_str()).unwrap_or("");
            match last {
                "Option" => Some(path_expr(&["None"], span)),
                "Result" => {
                    let e = match args.get(1) {
                        Some(err_ty) => default_expr_opt(err_ty, ctx, span, depth + 1)
                            .unwrap_or_else(|| call_path(&["Default", "default"], Vec::new(), span)),
                        None => call_path(&["Default", "default"], Vec::new(), span),
                    };
                    Some(call(path_expr(&["Err"], span), vec![e], span))
                }
                _ => {
                    if path.len() == 1 && ctx.is_declared(last) {
                        // declared type: default only if it itself derives Default (§6.7 closure)
                        if ctx.derives_of(last).iter().any(|d| d == "Default") {
                            Some(call_path(&[last, "default"], Vec::new(), span))
                        } else {
                            None
                        }
                    } else if path.len() == 1 {
                        // generic parameter: `<T>::default()` under the emitted `T: Default` bound
                        Some(call_path(&[path[0].as_str(), "default"], Vec::new(), span))
                    } else {
                        None
                    }
                }
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------- generation

/// Generate (impl AST item, reprinted text) for one trait, or `None` when unknown/inapplicable.
fn generate<'a>(agg: &Agg<'a>, ctx: &PackageCtx, tr: &str) -> Option<(Item, String)> {
    let span = agg.span;
    let mut fns: Vec<Item> = Vec::new();
    match tr {
        "Clone" => fns.push(gen_clone(agg, ctx)?),
        "Copy" | "Eq" => {} // markers: no new code (§6.7 Eq row; Copy is a marker trait)
        "Default" => fns.push(gen_default(agg, ctx)?),
        "Debug" => fns.push(gen_debug(agg)?),
        "PartialEq" => fns.push(gen_partial_eq(agg)?),
        "PartialOrd" | "Ord" => fns.push(gen_ord(agg)?),
        "Hash" => fns.push(gen_hash(agg)?),
        "HasField" => fns.extend(gen_hasfield(agg)),
        _ => return None,
    }
    let bound: Option<&str> = match tr {
        "Clone" => Some("Clone"),
        "Debug" => Some("Debug"),
        "PartialEq" => Some("PartialEq"),
        "PartialOrd" | "Ord" => Some("Ord"),
        "Hash" => Some("Hash"),
        "Default" => Some("Default"),
        _ => None,
    };
    let (gen_params, arg_types) = impl_generics(agg, bound, span);
    let self_ty = Type::named(vec![agg.name.to_string()], arg_types, span);
    let item = Item {
        attrs: Vec::new(),
        docs: Docs::default(),
        vis: Visibility::Private,
        span,
        kind: ItemKind::Impl {
            generics: gen_params,
            trait_: Some(vec![tr.to_string()]),
            trait_args: Vec::new(),
            self_ty,
            items: fns,
        },
    };
    let text = p_impl(&item);
    Some((item, text))
}

fn impl_generics(agg: &Agg, bound: Option<&str>, span: Span) -> (Vec<GenericParam>, Vec<Type>) {
    let mut params = Vec::new();
    let mut args = Vec::new();
    for gp in agg.generics {
        let mut g = gp.clone();
        if !gp.is_lifetime {
            if let Some(b) = bound {
                g.bounds.push(Bound { path: vec![b.to_string()], args: Vec::new(), is_lifetime: false, negated: false, span });
            }
        }
        args.push(Type::named(vec![gp.name.clone()], Vec::new(), span));
        params.push(g);
    }
    (params, args)
}

/// Render (or decline) the derive impl source for one (item, trait) — used by `infy expand`
/// reprinting; legality is validated by [`expand_derives`].
pub fn derive_impl_source(item: &Item, trait_name: &str) -> Option<String> {
    let agg = Agg::from_item(item)?;
    let ctx = PackageCtx::empty();
    let (_ast, text) = generate(&agg, &ctx, trait_name)?;
    Some(text)
}

// ----- Clone -----

fn gen_clone(agg: &Agg, ctx: &PackageCtx) -> Option<Item> {
    let span = agg.span;
    let body = match agg.kind {
        AggKind::Named(fs) => {
            let mut fields: Vec<(String, Expr)> = Vec::new();
            for f in fs {
                let v = if field_skip_clone(f) {
                    default_expr(&f.ty, ctx, span)
                } else {
                    call_path(&["Clone", "clone"], vec![self_named(&f.name, span)], span)
                };
                fields.push((f.name.clone(), v));
            }
            Expr::new(ExprKind::StructLit { path: vec![agg.name.to_string()], fields, rest: None }, span)
        }
        AggKind::Tuple(ts) => {
            let mut args: Vec<Expr> = Vec::new();
            for i in 0..ts.len() {
                args.push(call_path(&["Clone", "clone"], vec![self_tuple(i as u32, span)], span));
            }
            call(path_expr(&[agg.name], span), args, span)
        }
        AggKind::Unit => path_expr(&[agg.name], span),
        AggKind::Enum(vs) => {
            let mut arms: Vec<MatchArm> = Vec::new();
            for v in vs {
                let (pat, binds) = variant_bind_pattern(agg.name, v, span);
                let rebuilt = if binds.is_empty() {
                    path_expr(&[agg.name, v.name.as_str()], span)
                } else if !v.struct_fields.is_empty() {
                    let fields: Vec<(String, Expr)> = v
                        .struct_fields
                        .iter()
                        .zip(binds.iter())
                        .map(|(f, b)| (f.name.clone(), call_path(&["Clone", "clone"], vec![ident(b, span)], span)))
                        .collect();
                    Expr::new(
                        ExprKind::StructLit { path: vec![agg.name.to_string(), v.name.clone()], fields, rest: None },
                        span,
                    )
                } else {
                    let args: Vec<Expr> = binds
                        .iter()
                        .map(|b| call_path(&["Clone", "clone"], vec![ident(b, span)], span))
                        .collect();
                    call(path_expr(&[agg.name, v.name.as_str()], span), args, span)
                };
                arms.push(MatchArm { pat, guard: None, body: ArmBody::Expr(rebuilt), span });
            }
            Expr::new(ExprKind::Match { scrut: Box::new(self_expr(span)), arms }, span)
        }
    };
    Some(fn_item(
        "clone",
        vec![param("self", ty_named(&["Self"], span), span)],
        Some(ty_named(&["Self"], span)),
        vec![ret_stmt(Some(body), span)],
        span,
    ))
}

/// `Name::V(__c_0, …)` / `Name::V { f: __c_f }` binding pattern + the binding names, in
/// declaration order (§6.7: field order = declaration order).
fn variant_bind_pattern(ename: &str, v: &crate::ast::Variant, span: Span) -> (Pat, Vec<String>) {
    let path = vec![ename.to_string(), v.name.clone()];
    if !v.struct_fields.is_empty() {
        let fields: Vec<(String, Pat)> = v
            .struct_fields
            .iter()
            .map(|f| (f.name.clone(), binding_pat(&format!("__c_{}", f.name), span)))
            .collect();
        let binds: Vec<String> = v.struct_fields.iter().map(|f| format!("__c_{}", f.name)).collect();
        (
            Pat { kind: crate::ast::PatKind::Constructor { path, args: Vec::new(), fields, has_rest: false }, span },
            binds,
        )
    } else if !v.tuple.is_empty() {
        let args: Vec<Pat> = (0..v.tuple.len()).map(|i| binding_pat(&format!("__c_{}", i), span)).collect();
        let binds: Vec<String> = (0..v.tuple.len()).map(|i| format!("__c_{}", i)).collect();
        (Pat { kind: crate::ast::PatKind::Constructor { path, args, fields: Vec::new(), has_rest: false }, span }, binds)
    } else {
        (Pat { kind: crate::ast::PatKind::Path(path), span }, Vec::new())
    }
}

/// `(Name::V(__a_0, …), Name::V(__b_0, …))` paired pattern for `eq`/`cmp` over enums.
fn variant_pair_pattern(ename: &str, v: &crate::ast::Variant, span: Span) -> (Pat, Vec<(String, String)>) {
    let path = vec![ename.to_string(), v.name.clone()];
    let n = v.tuple.len().max(v.struct_fields.len());
    let mut pairs = Vec::new();
    let mut a_args = Vec::new();
    let mut b_args = Vec::new();
    if !v.struct_fields.is_empty() {
        let mut fa = Vec::new();
        let mut fb = Vec::new();
        for f in &v.struct_fields {
            let an = format!("__a_{}", f.name);
            let bn = format!("__b_{}", f.name);
            fa.push((f.name.clone(), binding_pat(&an, span)));
            fb.push((f.name.clone(), binding_pat(&bn, span)));
            pairs.push((an, bn));
        }
        a_args = fa;
        b_args = fb;
    } else {
        for i in 0..n {
            let an = format!("__a_{}", i);
            let bn = format!("__b_{}", i);
            a_args.push((String::new(), binding_pat(&an, span)));
            b_args.push((String::new(), binding_pat(&bn, span)));
            pairs.push((an, bn));
        }
    }
    let _ = path.clone();
    let mk = |args_named: Vec<(String, Pat)>| -> Pat {
        if args_named.iter().all(|(n, _)| n.is_empty()) {
            let args: Vec<Pat> = args_named.into_iter().map(|(_, p)| p).collect();
            Pat {
                kind: crate::ast::PatKind::Constructor {
                    path: path.clone(),
                    args,
                    fields: Vec::new(),
                    has_rest: false,
                },
                span,
            }
        } else {
            Pat {
                kind: crate::ast::PatKind::Constructor {
                    path: path.clone(),
                    args: Vec::new(),
                    fields: args_named,
                    has_rest: false,
                },
                span,
            }
        }
    };
    if n == 0 {
        (
            Pat {
                kind: crate::ast::PatKind::Tuple(vec![
                    Pat { kind: crate::ast::PatKind::Path(path.clone()), span },
                    Pat { kind: crate::ast::PatKind::Path(path), span },
                ]),
                span,
            },
            pairs,
        )
    } else {
        let t = Pat {
            kind: crate::ast::PatKind::Tuple(vec![mk(a_args), mk(b_args)]),
            span,
        };
        (t, pairs)
    }
}

// ----- Debug -----

fn gen_debug(agg: &Agg) -> Option<Item> {
    let span = agg.span;
    let body = match agg.kind {
        AggKind::Named(fs) => {
            let mut parts: Vec<Expr> = Vec::new();
            for f in fs {
                if agg.repr_c {
                    // §6.7: #[repr(C)] Debug prints the tuple form Name(f1, f2)
                    parts.push(debug_of(self_named(&f.name, span), f));
                } else {
                    parts.push(lit_str(&f.name, span));
                    parts.push(debug_of(self_named(&f.name, span), f));
                }
            }
            let segs: &[&str] = if agg.repr_c { &["std", "fmt", "format_tuple"] } else { &["std", "fmt", "format_struct"] };
            call_path(segs, vec![lit_str(agg.name, span), Expr::new(ExprKind::Array(parts), span)], span)
        }
        AggKind::Tuple(ts) => {
            let mut parts: Vec<Expr> = Vec::new();
            for (i, (_, _t)) in ts.iter().enumerate() {
                let _ = _t;
                parts.push(call_path(&["Debug", "format"], vec![self_tuple(i as u32, span)], span));
            }
            call_path(&["std", "fmt", "format_tuple"], vec![lit_str(agg.name, span), Expr::new(ExprKind::Array(parts), span)], span)
        }
        AggKind::Unit => call_path(&["std", "fmt", "format_unit"], vec![lit_str(agg.name, span)], span),
        AggKind::Enum(vs) => {
            let mut arms: Vec<MatchArm> = Vec::new();
            for v in vs {
                let (pat, binds) = variant_bind_pattern(agg.name, v, span);
                let label = format!("{}::{}", agg.name, v.name);
                let call = if binds.is_empty() {
                    call_path(&["std", "fmt", "format_unit"], vec![lit_str(&label, span)], span)
                } else {
                    let parts: Vec<Expr> = binds.iter().map(|b| call_path(&["Debug", "format"], vec![ident(b, span)], span)).collect();
                    call_path(&["std", "fmt", "format_variant"], vec![lit_str(&label, span), Expr::new(ExprKind::Array(parts), span)], span)
                };
                arms.push(MatchArm { pat, guard: None, body: ArmBody::Expr(call), span });
            }
            Expr::new(ExprKind::Match { scrut: Box::new(self_expr(span)), arms }, span)
        }
    };
    Some(fn_item(
        "format",
        vec![param("self", ty_named(&["Self"], span), span)],
        Some(ty_named(&["str"], span)),
        vec![ret_stmt(Some(body), span)],
        span,
    ))
}

/// Debug value for a field: `<redacted>` when `#[redact]`, else `Debug::format(self.f)`.
fn debug_of(acc: Expr, f: &Field) -> Expr {
    let span = acc.span;
    if field_redact(f) {
        lit_str("<redacted>", span)
    } else {
        call_path(&["Debug", "format"], vec![acc], span)
    }
}

// ----- PartialEq -----

fn gen_partial_eq(agg: &Agg) -> Option<Item> {
    let span = agg.span;
    let body = match agg.kind {
        AggKind::Named(fs) => {
            let mut conds: Vec<Expr> = Vec::new();
            for f in fs {
                if field_eq_skip(f) {
                    continue; // §6.7: #[compare(skip)] excluded
                }
                conds.push(bin(Punct::EqEq, self_named(&f.name, span), other_named(&f.name, span), span));
            }
            fold_and(conds, span)
        }
        AggKind::Tuple(ts) => {
            let mut conds: Vec<Expr> = Vec::new();
            for i in 0..ts.len() {
                conds.push(bin(Punct::EqEq, self_tuple(i as u32, span), other_tuple(i as u32, span), span));
            }
            fold_and(conds, span)
        }
        AggKind::Unit => lit_bool(true, span),
        AggKind::Enum(vs) => {
            let mut arms: Vec<MatchArm> = Vec::new();
            for v in vs {
                let (pat, pairs) = variant_pair_pattern(agg.name, v, span);
                let conds: Vec<Expr> = pairs
                    .iter()
                    .map(|(a, b)| bin(Punct::EqEq, ident(a, span), ident(b, span), span))
                    .collect();
                let cond = fold_and(conds, span);
                arms.push(MatchArm { pat, guard: None, body: ArmBody::Expr(cond), span });
            }
            arms.push(MatchArm {
                pat: Pat { kind: crate::ast::PatKind::Wildcard, span },
                guard: None,
                body: ArmBody::Expr(lit_bool(false, span)),
                span,
            });
            Expr::new(ExprKind::Match { scrut: Box::new(match_scrut_self_other(span)), arms }, span)
        }
    };
    Some(fn_item(
        "eq",
        vec![
            param("self", ty_named(&["Self"], span), span),
            param("other", ty_named(&["Self"], span), span),
        ],
        Some(ty_named(&["bool"], span)),
        vec![ret_stmt(Some(body), span)],
        span,
    ))
}

fn fold_and(conds: Vec<Expr>, span: Span) -> Expr {
    if conds.is_empty() {
        return lit_bool(true, span);
    }
    let mut it = conds.into_iter();
    let mut acc = it.next().unwrap_or_else(|| lit_bool(true, span));
    for c in it {
        acc = bin(Punct::AmpAmp, acc, c, span);
    }
    acc
}

// ----- PartialOrd / Ord -----

fn gen_ord(agg: &Agg) -> Option<Item> {
    let span = agg.span;
    let mut stmts: Vec<Stmt> = Vec::new();
    match agg.kind {
        AggKind::Named(fs) => {
            for f in fs {
                ord_field(&mut stmts, self_named(&f.name, span), other_named(&f.name, span), field_order(f), span);
            }
        }
        AggKind::Tuple(ts) => {
            for i in 0..ts.len() {
                ord_field(&mut stmts, self_tuple(i as u32, span), other_tuple(i as u32, span), OrderDir::Asc, span);
            }
        }
        AggKind::Unit => {}
        AggKind::Enum(vs) => {
            // §6.7: lexicographic order — variant discriminant first, then fields per variant.
            stmts.push(let_stmt("__d_self", call_path(&["std", "cmp", "discriminant"], vec![self_expr(span)], span), span));
            stmts.push(let_stmt("__d_other", call_path(&["std", "cmp", "discriminant"], vec![ident("other", span)], span), span));
            ord_field(
                &mut stmts,
                ident("__d_self", span),
                ident("__d_other", span),
                OrderDir::Asc,
                span,
            );
            let mut arms: Vec<MatchArm> = Vec::new();
            for v in vs {
                let (pat, pairs) = variant_pair_pattern(agg.name, v, span);
                let mut inner: Vec<Stmt> = Vec::new();
                for (a, b) in &pairs {
                    ord_field(&mut inner, ident(a, span), ident(b, span), OrderDir::Asc, span);
                }
                inner.push(ret_stmt(Some(lit_i(0, span)), span));
                arms.push(MatchArm { pat, guard: None, body: ArmBody::Block(block(inner, span)), span });
            }
            arms.push(MatchArm {
                pat: Pat { kind: crate::ast::PatKind::Wildcard, span },
                guard: None,
                body: ArmBody::Expr(lit_i(0, span)),
                span,
            });
            stmts.push(Stmt {
                kind: StmtKind::Expr {
                    expr: Expr::new(ExprKind::Match { scrut: Box::new(match_scrut_self_other(span)), arms }, span),
                    linkage: Vec::new(),
                },
                attrs: Vec::new(),
                docs: Docs::default(),
                span,
                terminated: false,
            });
        }
    }
    stmts.push(ret_stmt(Some(lit_i(0, span)), span));
    Some(fn_item(
        "cmp",
        vec![
            param("self", ty_named(&["Self"], span), span),
            param("other", ty_named(&["Self"], span), span),
        ],
        Some(ty_named(&["i32"], span)),
        stmts,
        span,
    ))
}

/// One lexicographic field step: `if a < b { return s; } if a > b { return b; }` with
/// `asc/desc` deciding the sign order (§6.7 `#[order(asc)]` / `#[order(desc)]`).
fn ord_field(stmts: &mut Vec<Stmt>, a: Expr, b: Expr, dir: OrderDir, span: Span) {
    let (small, big) = match dir {
        OrderDir::Asc => (lit_i(-1, span), lit_i(1, span)),
        OrderDir::Desc => (lit_i(1, span), lit_i(-1, span)),
    };
    stmts.push(if_stmt(bin(Punct::Lt, a.clone(), b.clone(), span), vec![ret_stmt(Some(small), span)], span));
    stmts.push(if_stmt(bin(Punct::Gt, a, b, span), vec![ret_stmt(Some(big), span)], span));
}

// ----- Hash -----

fn gen_hash(agg: &Agg) -> Option<Item> {
    let span = agg.span;
    let mut stmts: Vec<Stmt> = Vec::new();
    match agg.kind {
        AggKind::Named(fs) => {
            stmts.push(hash_feed_disc(0, span));
            for f in fs {
                stmts.push(hash_feed_field(self_named(&f.name, span), span));
            }
        }
        AggKind::Tuple(ts) => {
            stmts.push(hash_feed_disc(0, span));
            for i in 0..ts.len() {
                stmts.push(hash_feed_field(self_tuple(i as u32, span), span));
            }
        }
        AggKind::Unit => {
            stmts.push(hash_feed_disc(0, span));
        }
        AggKind::Enum(vs) => {
            let mut arms: Vec<MatchArm> = Vec::new();
            for (vi, v) in vs.iter().enumerate() {
                let disc = v.disc.unwrap_or(vi as u128) as u32;
                let (pat, binds) = variant_bind_pattern(agg.name, v, span);
                let mut inner: Vec<Stmt> = Vec::new();
                inner.push(hash_feed_disc(disc, span));
                for b in &binds {
                    inner.push(hash_feed_field(ident(b, span), span));
                }
                arms.push(MatchArm { pat, guard: None, body: ArmBody::Block(block(inner, span)), span });
            }
            stmts.push(Stmt {
                kind: StmtKind::Expr {
                    expr: Expr::new(ExprKind::Match { scrut: Box::new(self_expr(span)), arms }, span),
                    linkage: Vec::new(),
                },
                attrs: Vec::new(),
                docs: Docs::default(),
                span,
                terminated: false,
            });
        }
    }
    Some(fn_item(
        "hash",
        vec![
            param("self", ty_named(&["Self"], span), span),
            param("state", ty_named(&["Hasher"], span), span),
        ],
        None,
        stmts,
        span,
    ))
}

/// §6.7 Hash row: `std::hash::feed_i32(state, field_discriminant_i32(d))`.
fn hash_feed_disc(disc: u32, span: Span) -> Stmt {
    expr_stmt(
        call_path(
            &["std", "hash", "feed_i32"],
            vec![ident("state", span), call_path(&["field_discriminant_i32"], vec![lit_u(disc as u128, span)], span)],
            span,
        ),
        span,
    )
}

/// §6.7 Hash row: the field fed through xxHash64 (fixed algorithm).
fn hash_feed_field(acc: Expr, span: Span) -> Stmt {
    expr_stmt(
        call_path(
            &["std", "hash", "feed_u64"],
            vec![ident("state", span), call_path(&["std", "hash", "xxhash64_field"], vec![acc], span)],
            span,
        ),
        span,
    )
}

// ----- Default -----

fn gen_default(agg: &Agg, ctx: &PackageCtx) -> Option<Item> {
    let span = agg.span;
    let body = match agg.kind {
        AggKind::Named(fs) => {
            let mut fields: Vec<(String, Expr)> = Vec::new();
            for f in fs {
                fields.push((f.name.clone(), default_expr(&f.ty, ctx, span)));
            }
            Expr::new(ExprKind::StructLit { path: vec![agg.name.to_string()], fields, rest: None }, span)
        }
        AggKind::Tuple(ts) => {
            let mut args: Vec<Expr> = Vec::new();
            for (_, t) in ts {
                args.push(default_expr(t, ctx, span));
            }
            call(path_expr(&[agg.name], span), args, span)
        }
        AggKind::Unit => path_expr(&[agg.name], span),
        AggKind::Enum(vs) => {
            let chosen = default_variant(vs)?;
            let v = &vs[chosen];
            let mut args: Vec<Expr> = Vec::new();
            for (_, t) in &v.tuple {
                args.push(default_expr(t, ctx, span));
            }
            for f in &v.struct_fields {
                args.push(default_expr(&f.ty, ctx, span));
            }
            if args.is_empty() {
                path_expr(&[agg.name, v.name.as_str()], span)
            } else if v.struct_fields.is_empty() {
                call(path_expr(&[agg.name, v.name.as_str()], span), args, span)
            } else {
                let fields: Vec<(String, Expr)> = v
                    .struct_fields
                    .iter()
                    .zip(args.iter())
                    .map(|(f, a)| (f.name.clone(), a.clone()))
                    .collect();
                Expr::new(
                    ExprKind::StructLit { path: vec![agg.name.to_string(), v.name.clone()], fields, rest: None },
                    span,
                )
            }
        }
    };
    Some(fn_item("default", Vec::new(), Some(ty_named(&["Self"], span)), vec![ret_stmt(Some(body), span)], span))
}

/// §6.7: first variant, or the `#[default]`-selected one (multiple → `E-0423`, detected in
/// `expand_derives`; deterministically the first here).
fn default_variant(vs: &[crate::ast::Variant]) -> Option<usize> {
    for (i, v) in vs.iter().enumerate() {
        if v.attrs.iter().any(|a| attr_is(a, "default")) {
            return Some(i);
        }
    }
    if vs.is_empty() {
        None
    } else {
        Some(0)
    }
}

// ----- HasField -----

/// §6.7 HasField row: compile-time reflection table — field name, type tag string, and offsets
/// only when `#[repr(C)]`.
fn gen_hasfield(agg: &Agg) -> Vec<Item> {
    let span = agg.span;
    let mut rows: Vec<(String, String)> = Vec::new();
    match agg.kind {
        AggKind::Named(fs) => {
            for f in fs {
                rows.push((f.name.clone(), p_ty(&f.ty)));
            }
        }
        AggKind::Tuple(ts) => {
            for (i, (_, t)) in ts.iter().enumerate() {
                rows.push((i.to_string(), p_ty(t)));
            }
        }
        AggKind::Unit => {}
        AggKind::Enum(vs) => {
            for v in vs {
                rows.push((v.name.clone(), "variant".to_string()));
            }
        }
    }
    let mut out = Vec::new();
    out.push(fn_item(
        "__hasfield_field_count",
        Vec::new(),
        Some(ty_named(&["u32"], span)),
        vec![ret_stmt(Some(lit_u(rows.len() as u128, span)), span)],
        span,
    ));
    out.push(table_fn("__hasfield_field_name", rows.iter().map(|r| r.0.clone()).collect(), span));
    out.push(table_fn("__hasfield_field_type", rows.iter().map(|r| r.1.clone()).collect(), span));
    if agg.repr_c {
        let mut arms: Vec<MatchArm> = Vec::new();
        for (i, r) in rows.iter().enumerate() {
            let off = match agg.kind {
                AggKind::Named(_) => call_path(&["std", "mem", "offset_of"], vec![lit_str(&r.0, span)], span),
                AggKind::Tuple(_) => call_path(&["std", "mem", "offset_of"], vec![lit_str(&i.to_string(), span)], span),
                _ => lit_u(0, span),
            };
            arms.push(MatchArm { pat: int_pat(i as u32, span), guard: None, body: ArmBody::Expr(off), span });
        }
        arms.push(MatchArm {
            pat: Pat { kind: crate::ast::PatKind::Wildcard, span },
            guard: None,
            body: ArmBody::Expr(lit_u(0, span)),
            span,
        });
        out.push(fn_item(
            "__hasfield_field_offset",
            vec![param("i", ty_named(&["u32"], span), span)],
            Some(ty_named(&["u64"], span)),
            vec![ret_stmt(Some(Expr::new(ExprKind::Match { scrut: Box::new(ident("i", span)), arms }, span)), span)],
            span,
        ));
    }
    out
}

fn table_fn(name: &'static str, cells: Vec<String>, span: Span) -> Item {
    let mut arms: Vec<MatchArm> = Vec::new();
    for (i, c) in cells.iter().enumerate() {
        arms.push(MatchArm {
            pat: int_pat(i as u32, span),
            guard: None,
            body: ArmBody::Expr(lit_str(c, span)),
            span,
        });
    }
    arms.push(MatchArm {
        pat: Pat { kind: crate::ast::PatKind::Wildcard, span },
        guard: None,
        body: ArmBody::Expr(lit_str("", span)),
        span,
    });
    fn_item(
        name,
        vec![param("i", ty_named(&["u32"], span), span)],
        Some(ty_named(&["str"], span)),
        vec![ret_stmt(Some(Expr::new(ExprKind::Match { scrut: Box::new(ident("i", span)), arms }, span)), span)],
        span,
    )
}

fn int_pat(i: u32, span: Span) -> Pat {
    Pat {
        kind: crate::ast::PatKind::Lit(Lit::Int(IntLit { digits: i.to_string(), radix: Radix::Dec, suffix: None, value: Some(i as u128) })),
        span,
    }
}

// ---------------------------------------------------------------- field attributes

fn attr_is(a: &Attr, name: &str) -> bool {
    a.path.last().map(|s| s.as_str()) == Some(name)
}

fn field_redact(f: &Field) -> bool {
    f.attrs.iter().any(|a| attr_is(a, "redact"))
}

fn field_eq_skip(f: &Field) -> bool {
    f.attrs.iter().any(|a| attr_is(a, "compare") && a.has_ident("skip"))
}

fn field_skip_clone(f: &Field) -> bool {
    f.attrs.iter().any(|a| attr_is(a, "skip_clone"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OrderDir {
    Asc,
    Desc,
}

fn field_order(f: &Field) -> OrderDir {
    for a in &f.attrs {
        if attr_is(a, "order") {
            if a.has_ident("desc") {
                return OrderDir::Desc;
            }
            if a.has_ident("asc") {
                return OrderDir::Asc;
            }
        }
    }
    OrderDir::Asc
}

// ---------------------------------------------------------------- expand_derives (§6.7)

/// Stage [2] derive expansion: validates every `#[derive]` against the §6.7 rules, generates the
/// impl items, and returns the number of generated impls.
///
/// §6.7 derive-order: multiple derives on one item expand in **written order**; generated impls
/// are inserted after the item in that order.
pub fn expand_derives(files: &mut BTreeMap<FileId, ParsedFile>, handler: &mut Handler) -> usize {
    let ctx = PackageCtx::new(files);
    let mut total = 0usize;
    let fids: Vec<FileId> = files.keys().cloned().collect();
    for fid in fids {
        let mut inserts: Vec<(usize, Vec<Item>)> = Vec::new();
        let snapshot: Vec<Item> = files[&fid].items.clone();
        for (idx, item) in snapshot.iter().enumerate() {
            if !item.has_attr("derive") {
                continue;
            }
            if !matches!(item.kind, ItemKind::Struct { .. } | ItemKind::Enum { .. }) {
                continue; // placement is reported by validate_attrs (E-0410)
            }
            let mut generated: Vec<Item> = Vec::new();
            let mut seen: BTreeSet<String> = BTreeSet::new();
            for tr in derive_list(item) {
                if !CLOSED_DERIVES.contains(&tr.as_str()) {
                    emit(handler, item.span, "E-0410", &format!("未知 derive trait `{}`", tr));
                    continue;
                }
                if !seen.insert(tr.clone()) {
                    emit(handler, item.span, "E-0426", "重复 derive");
                    continue;
                }
                if let Some((ast, _text)) = check_and_generate(item, &ctx, &tr, handler) {
                    generated.push(ast);
                    total += 1;
                }
            }
            if !generated.is_empty() {
                inserts.push((idx, generated));
            }
        }
        // Insert back-to-front so earlier indices stay valid; written order preserved within item.
        for (idx, items) in inserts.into_iter().rev() {
            let pos = idx + 1;
            if let Some(f) = files.get_mut(&fid) {
                f.items.splice(pos..pos, items);
            }
        }
    }
    total
}

/// §6.7 legality checks + generation for one (item, trait). Emits `E-042x` diagnostics and
/// returns `None` when the derive must fail.
fn check_and_generate(item: &Item, ctx: &PackageCtx, tr: &str, handler: &mut Handler) -> Option<(Item, String)> {
    let agg = Agg::from_item(item)?;
    let span = item.span;
    let rows = agg.field_rows();
    // E-0427: a derive whose generated code would touch a raw pointer fails outright (§6.7).
    if rows.iter().any(|(_, t, _)| ty_has_ptr(t)) {
        emit(handler, span, "E-0427", "derive 需要 `unsafe`");
        return None;
    }
    match tr {
        "Copy" => {
            if ctx.has_drop(agg.name) {
                emit(handler, span, "E-0421", "`Copy` derive 非法");
                return None;
            }
            for (_, t, _) in &rows {
                if ty_has_dyn(t) || !type_is_copy(t, ctx, 0) {
                    emit(handler, span, "E-0421", "`Copy` derive 非法");
                    return None;
                }
            }
        }
        "Eq" | "Ord" => {
            let has_float = rows.iter().any(|(_, t, _)| ty_has_float(t));
            if has_float && !agg.allow_nan {
                emit(handler, span, "E-0424", "含浮点字段却 derive `Eq`");
                return None;
            }
            let need_ord = tr == "Ord";
            for (_, t, _) in &rows {
                let ok = if need_ord {
                    type_is_ord(t, ctx, agg.allow_nan, 0)
                } else {
                    type_is_eq(t, ctx, agg.allow_nan, 0)
                };
                if !ok {
                    emit(handler, span, "E-0425", "derive `Eq` 但字段非 `Eq`");
                    return None;
                }
            }
        }
        "Clone" => {
            if let AggKind::Named(fs) = agg.kind {
                for f in fs {
                    if field_skip_clone(f) && !type_has_default(&f.ty, ctx, 0) {
                        emit(handler, f.span, "E-0420", "`#[skip_clone]` 字段无 `Default`");
                        return None;
                    }
                }
            }
        }
        "Default" => {
            if let AggKind::Enum(vs) = agg.kind {
                let dcount = vs.iter().filter(|v| v.attrs.iter().any(|a| attr_is(a, "default"))).count();
                if dcount > 1 {
                    emit(handler, span, "E-0423", "多个 `#[default]` 变体");
                    return None;
                }
                if let Some(i) = default_variant(vs) {
                    let v = &vs[i];
                    for (_, t) in &v.tuple {
                        if !type_has_default(t, ctx, 0) {
                            emit(handler, span, "E-0422", "`Default` derive 于 enum 无零值变体");
                            return None;
                        }
                    }
                    for f in &v.struct_fields {
                        if !type_has_default(&f.ty, ctx, 0) {
                            emit(handler, span, "E-0422", "`Default` derive 于 enum 无零值变体");
                            return None;
                        }
                    }
                }
            }
        }
        _ => {}
    }
    generate(&agg, ctx, tr)
}

fn emit(handler: &mut Handler, span: Span, code: &str, title: &str) {
    // `expand_derives` has no `Sources` in its signature: positions fall back to 1:1 while the
    // code, title and phase stay normative.
    handler.emit(Diagnostic::new(code, Phase::Macro, span.file, crate::span::Position { line: 1, column: 1 }).titled(title));
}

// ---------------------------------------------------------------- validate_attrs (§6.6)

/// §6.6 CLOSED attribute list validation. Unknown attributes → `E-0410`; `#[cfg]` is rejected
/// with the §F.1 pointer to `//infy:build`; `#[allow]` is not on the list (§9.6 adjusts levels
/// through `[lints]`, not attributes). Misplaced attributes are also `E-0410` — the closed list
/// leaves no finer code. `#[deprecated]` *uses* produce `W-0400`. Returns the number of
/// diagnostics emitted.
pub fn validate_attrs(files: &BTreeMap<FileId, ParsedFile>, sources: &Sources, handler: &mut Handler) -> usize {
    let before = handler.diags.len();
    for (fid, pf) in files.iter() {
        let fid: FileId = *fid;
        for a in &pf.file_attrs {
            check_attr(fid, a, Some("file"), sources, handler);
        }
        for item in &pf.items {
            for a in &item.attrs {
                check_attr(fid, a, Some(item_kind_name(&item.kind)), sources, handler);
            }
            check_nested_attrs(fid, item, sources, handler);
        }
    }
    // use-side: W-0400 for references to `#[deprecated]` pub items (§6.6 last-but-one row)
    let mut deprecated: BTreeMap<String, String> = BTreeMap::new();
    for pf in files.values() {
        for item in &pf.items {
            if item.vis == Visibility::Pub {
                if let Some(a) = item.attrs.iter().find(|a| attr_is(a, "deprecated")) {
                    let note = a
                        .positional
                        .first()
                        .and_then(|v| match v {
                            crate::ast::AttrValue::Str(s) => Some(s.clone()),
                            _ => None,
                        })
                        .unwrap_or_default();
                    deprecated.insert(item.name().to_string(), note);
                }
            }
        }
    }
    if !deprecated.is_empty() {
        for (fid, pf) in files.iter() {
            for item in &pf.items {
                let mut refs: Vec<(String, Span)> = Vec::new();
                collect_name_refs_item(item, &mut refs);
                for (n, sp) in refs {
                    if n == item.name() {
                        continue; // shadowed by local definitions: conservative no-op
                    }
                    if let Some(note) = deprecated.get(&n) {
                        let pos = sources.line_col(sp).0;
                        let mut d = Diagnostic::new("W-0400", Phase::Macro, *fid, pos)
                            .titled(format!("使用了已弃用项 `{}`", n));
                        if !note.is_empty() {
                            d = d.note(format!("弃用说明: {}", note));
                        }
                        handler.emit(d);
                    }
                }
            }
        }
    }
    handler.diags.len() - before
}

fn item_kind_name(kind: &ItemKind) -> &'static str {
    match kind {
        ItemKind::Fn { .. } => "fn",
        ItemKind::Event { .. } => "event",
        ItemKind::Struct { .. } => "struct",
        ItemKind::Enum { .. } => "enum",
        ItemKind::Union { .. } => "union",
        ItemKind::Trait { .. } => "trait",
        ItemKind::Impl { .. } => "impl",
        ItemKind::ExternBlock { .. } => "extern",
        ItemKind::Const { .. } | ItemKind::Static { .. } => "const",
        ItemKind::MacroRules { .. } => "macro",
        ItemKind::Chain { .. } => "chain",
        ItemKind::TypeAlias { .. } => "type",
        ItemKind::Mod { .. } => "mod",
    }
}

fn check_attr(
    fid: FileId,
    a: &Attr,
    place: Option<&'static str>,
    sources: &Sources,
    handler: &mut Handler,
) {
    let name = match a.path.last() {
        Some(n) => n.as_str(),
        None => return,
    };
    let pos = sources.line_col(a.span).0;
    if name == "cfg" {
        handler.emit(
            Diagnostic::new("E-0410", Phase::Macro, fid, pos)
                .titled(format!("未知属性 `#[{}]`", a.full()))
                .help("语言层无 `#[cfg]`（§F.1）：条件编译用 `//infy:build` 构建约束注释（§2.2）"),
        );
        return;
    }
    if !CLOSED_ATTRS.contains(&name) {
        handler.emit(
            Diagnostic::new("E-0410", Phase::Macro, fid, pos)
                .titled(format!("未知属性 `#[{}]`", a.full()))
                .help("用 §6.6 封闭列表内属性"),
        );
        return;
    }
    match name {
        "infy_version" | "entry_chain" => {
            if place != Some("file") || !a.inner {
                handler.emit(
                    Diagnostic::new("E-0410", Phase::Macro, fid, pos)
                        .titled(format!("属性 `#[{}]` 位置错误", name))
                        .help("`#![infy_version]` / `#![entry_chain]` 只允许出现在文件作用域（§6.6/§3.1）"),
                );
            }
        }
        "test" | "bench" => {
            if place != Some("fn") && place != Some("event") {
                handler.emit(
                    Diagnostic::new("E-0410", Phase::Macro, fid, pos)
                        .titled(format!("属性 `#[{}]` 位置错误", name))
                        .help("只能作用于 fn/event（§6.6）"),
                );
            }
        }
        "derive" => {
            if place != Some("struct") && place != Some("enum") {
                handler.emit(
                    Diagnostic::new("E-0410", Phase::Macro, fid, pos)
                        .titled("属性 `#[derive]` 位置错误")
                        .help("derive 仅适用于 struct/enum（§6.7）"),
                );
            }
            for t in a.idents() {
                if !CLOSED_DERIVES.contains(&t.as_str()) {
                    handler.emit(
                        Diagnostic::new("E-0410", Phase::Macro, fid, pos)
                            .titled(format!("未知 derive trait `{}`", t))
                            .help("可用 derive 全集见 §6.7"),
                    );
                }
            }
        }
        "repr" => {
            if !a.has_ident("C") {
                handler.emit(
                    Diagnostic::new("E-0410", Phase::Macro, fid, pos)
                        .titled("未知属性参数 `#[repr(…)]`")
                        .help("§6.6 只列出了 `#[repr(C)]`"),
                );
            }
            if place != Some("struct") && place != Some("enum") && place != Some("union") {
                handler.emit(
                    Diagnostic::new("E-0410", Phase::Macro, fid, pos)
                        .titled("属性 `#[repr]` 位置错误")
                        .help("作用于 struct/enum/union（§6.6）"),
                );
            }
        }
        "align" => {
            let n = a.positional.first().and_then(|v| match v {
                // `parser::attr_value` routes an integer token to `AttrValue::Int`, so an
                // `AttrValue::Expr` here is always a non-literal spelling (`SIZE`, `-1`) and is
                // reported, not guessed.
                crate::ast::AttrValue::Int(i) => Some(*i),
                _ => None,
            });
            match n {
                Some(v) if v != 0 && (v & (v - 1)) == 0 => {}
                _ => {
                    handler.emit(
                        Diagnostic::new("E-0410", Phase::Macro, fid, pos)
                            .titled("属性 `#[align(N)]` 参数非法")
                            .help("N 必须是 2 的幂（§4.8）"),
                    );
                }
            }
        }
        "doc" => {
            let hidden = a.has_ident("hidden")
                || a.args.iter().any(|ar| ar.name.as_deref() == Some("hidden"));
            if !hidden {
                handler.emit(
                    Diagnostic::new("E-0410", Phase::Macro, fid, pos)
                        .titled("属性 `#[doc]` 参数非法")
                        .help("§6.6 只定义了 `#[doc(hidden)]`"),
                );
            }
        }
        _ => {}
    }
}

fn check_nested_attrs(
    fid: FileId,
    item: &Item,
    sources: &Sources,
    handler: &mut Handler,
) {
    match &item.kind {
        ItemKind::Struct { fields, .. } => {
            for f in fields {
                check_field_attrs(fid, &f.attrs, f.span, sources, handler);
            }
        }
        ItemKind::Enum { variants, .. } => {
            for v in variants {
                for a in &v.attrs {
                    let name = a.path.last().cloned().unwrap_or_default();
                    if !(name == "default" || FIELD_ALLOWED.contains(&name.as_str())) {
                        let pos = sources.line_col(v.span).0;
                        handler.emit(
                            Diagnostic::new("E-0410", Phase::Macro, fid, pos)
                                .titled(format!("未知属性 `#[{}]`（变体位置）", a.full())),
                        );
                    }
                }
                for f in &v.struct_fields {
                    check_field_attrs(fid, &f.attrs, f.span, sources, handler);
                }
            }
        }
        ItemKind::Impl { items, .. } | ItemKind::Trait { items, .. } => {
            for it in items {
                for a in &it.attrs {
                    check_attr(fid, a, Some(item_kind_name(&it.kind)), sources, handler);
                }
                check_nested_attrs(fid, it, sources, handler);
            }
        }
        ItemKind::ExternBlock { decls, .. } => {
            for d in decls {
                for a in &d.attrs {
                    check_attr(fid, a, Some(item_kind_name(&d.kind)), sources, handler);
                }
            }
        }
        _ => {}
    }
}

fn check_field_attrs(fid: FileId, attrs: &[Attr], span: Span, sources: &Sources, handler: &mut Handler) {
    for a in attrs {
        let name = match a.path.last() {
            Some(n) => n.clone(),
            None => continue,
        };
        let pos = sources.line_col(span).0;
        if !FIELD_ALLOWED.contains(&name.as_str()) {
            handler.emit(
                Diagnostic::new("E-0410", Phase::Macro, fid, pos)
                    .titled(format!("未知属性 `#[{}]`（字段位置）", a.full())),
            );
            continue;
        }
        if name == "order" && !(a.has_ident("asc") || a.has_ident("desc")) {
            handler.emit(
                Diagnostic::new("E-0410", Phase::Macro, fid, pos)
                    .titled("属性 `#[order]` 参数非法")
                    .help("只接受 `#[order(asc)]` / `#[order(desc)]`（§6.7）"),
            );
        }
        if name == "compare" && !a.has_ident("skip") {
            handler.emit(
                Diagnostic::new("E-0410", Phase::Macro, fid, pos)
                    .titled("属性 `#[compare]` 参数非法")
                    .help("只接受 `#[compare(skip)]`（§6.7）"),
            );
        }
    }
}

// ---------------------------------------------------------------- deprecated use scan

fn collect_name_refs_item(item: &Item, out: &mut Vec<(String, Span)>) {
    match &item.kind {
        ItemKind::Fn { body, .. } | ItemKind::Event { body, .. } => {
            if let crate::ast::Body::Block(b) = body {
                refs_block(b, out);
            }
        }
        ItemKind::Const { init, .. } | ItemKind::Static { init, .. } => {
            if let Some(e) = init {
                refs_expr(e, out);
            }
        }
        ItemKind::Chain { stmts, .. } => {
            for st in stmts {
                refs_stmt(st, out);
            }
        }
        ItemKind::Impl { items, .. } | ItemKind::Trait { items, .. } => {
            for it in items {
                collect_name_refs_item(it, out);
            }
        }
        _ => {}
    }
}

fn refs_block(b: &Block, out: &mut Vec<(String, Span)>) {
    for st in &b.stmts {
        refs_stmt(st, out);
    }
    if let Some(t) = &b.tail {
        refs_expr(t, out);
    }
}

fn refs_stmt(st: &Stmt, out: &mut Vec<(String, Span)>) {
    match &st.kind {
        StmtKind::Expr { expr, .. } => refs_expr(expr, out),
        StmtKind::Let { init, .. } => {
            if let Some(e) = init {
                refs_expr(e, out);
            }
        }
        StmtKind::Assign { target, value, .. } => {
            refs_expr(target, out);
            refs_expr(value, out);
        }
        StmtKind::If { cond, then, else_ } => {
            refs_expr(cond, out);
            refs_block(then, out);
            if let Some(eb) = else_ {
                match &**eb {
                    ElseBranch::Block(b) => refs_block(b, out),
                    ElseBranch::If(s) => refs_stmt(s, out),
                }
            }
        }
        StmtKind::While { cond, body } => {
            refs_expr(cond, out);
            refs_block(body, out);
        }
        StmtKind::Loop { body, .. } | StmtKind::Unsafe(body) | StmtKind::Defer(body) => refs_block(body, out),
        StmtKind::For { iter, body, .. } => {
            refs_expr(iter, out);
            refs_block(body, out);
        }
        StmtKind::Match { scrut, arms } => {
            refs_expr(scrut, out);
            for a in arms {
                if let Some(g) = &a.guard {
                    refs_expr(g, out);
                }
                match &a.body {
                    ArmBody::Expr(e) => refs_expr(e, out),
                    ArmBody::Block(b) => refs_block(b, out),
                }
            }
        }
        StmtKind::Return { value } | StmtKind::Break { value, .. } => {
            if let Some(v) = value {
                refs_expr(v, out);
            }
        }
        StmtKind::ConstAssert { cond, .. } => refs_expr(cond, out),
        _ => {}
    }
}

fn refs_expr(e: &Expr, out: &mut Vec<(String, Span)>) {
    match &e.kind {
        ExprKind::Ident(s) => out.push((s.clone(), e.span)),
        ExprKind::Path(p) => {
            if let Some(f) = p.first() {
                out.push((f.clone(), e.span));
            }
        }
        ExprKind::StructLit { path, fields, rest } => {
            if let Some(f) = path.first() {
                out.push((f.clone(), e.span));
            }
            for (_, v) in fields {
                refs_expr(v, out);
            }
            if let Some(r) = rest {
                refs_expr(r, out);
            }
        }
        ExprKind::Unary { expr, .. } => refs_expr(expr, out),
        ExprKind::Binary { lhs, rhs, .. } => {
            refs_expr(lhs, out);
            refs_expr(rhs, out);
        }
        ExprKind::Assign { target, value, .. } => {
            refs_expr(target, out);
            refs_expr(value, out);
        }
        ExprKind::Call { callee, args } => {
            refs_expr(callee, out);
            for a in args {
                refs_expr(&a.expr, out);
            }
        }
        ExprKind::Method { recv, args, .. } => {
            refs_expr(recv, out);
            for a in args {
                refs_expr(&a.expr, out);
            }
        }
        ExprKind::Field { base, .. } => refs_expr(base, out),
        ExprKind::Index { base, index } => {
            refs_expr(base, out);
            refs_expr(index, out);
        }
        ExprKind::Tuple(v) | ExprKind::Array(v) => {
            for x in v {
                refs_expr(x, out);
            }
        }
        ExprKind::ArrayRepeat { value, len } => {
            refs_expr(value, out);
            refs_expr(len, out);
        }
        ExprKind::If { cond, then, else_ } => {
            refs_expr(cond, out);
            refs_block(then, out);
            if let Some(eb) = else_ {
                match &**eb {
                    ElseBranch::Block(b) => refs_block(b, out),
                    ElseBranch::If(s) => refs_stmt(s, out),
                }
            }
        }
        ExprKind::Match { scrut, arms } => {
            refs_expr(scrut, out);
            for a in arms {
                if let Some(g) = &a.guard {
                    refs_expr(g, out);
                }
                match &a.body {
                    ArmBody::Expr(x) => refs_expr(x, out),
                    ArmBody::Block(b) => refs_block(b, out),
                }
            }
        }
        ExprKind::Block(b) | ExprKind::Unsafe(b) => refs_block(b, out),
        ExprKind::Closure { body, .. } => refs_block(body, out),
        ExprKind::Ref { expr, .. } | ExprKind::Deref { expr } | ExprKind::Try { expr } | ExprKind::Move { expr } => {
            refs_expr(expr, out)
        }
        ExprKind::Cast { expr, .. } | ExprKind::Is { expr, .. } => refs_expr(expr, out),
        ExprKind::Range { lo, hi, .. } => {
            if let Some(l) = lo {
                refs_expr(l, out);
            }
            if let Some(h) = hi {
                refs_expr(h, out);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Sources;

    fn harness(texts: &[(&str, &str)]) -> (Sources, BTreeMap<FileId, ParsedFile>, Handler) {
        let mut src = Sources::default();
        let mut files = BTreeMap::new();
        let mut handler = Handler::default();
        for (path, text) in texts {
            let f = src.add(path.to_string(), text.to_string());
            let lexed = crate::lexer::lex_file(&src, f, &mut handler);
            let pf = crate::parser::parse(&src, f, &lexed, &mut handler);
            files.insert(f, pf);
        }
        handler.diags.clear();
        (src, files, handler)
    }

    fn codes(h: &Handler) -> Vec<String> {
        h.diags.iter().map(|d| d.code.clone()).collect()
    }

    fn impl_texts(files: &BTreeMap<FileId, ParsedFile>) -> Vec<String> {
        let mut out = Vec::new();
        for pf in files.values() {
            for it in &pf.items {
                if let ItemKind::Impl { .. } = &it.kind {
                    out.push(p_impl(it));
                }
            }
        }
        out
    }

    fn one(files: &BTreeMap<FileId, ParsedFile>, needle: &str) -> String {
        impl_texts(files)
            .into_iter()
            .find(|t| t.contains(needle))
            .unwrap_or_else(|| panic!("no impl containing {needle} in {:?}", impl_texts(files)))
    }

    // ---------------------------------------------------------------- xxHash64 (§6.7 fixed algorithm)

    #[test]
    fn xxhash64_empty_is_the_reference_digest() {
        assert_eq!(xxhash64(b""), 0xEF46_DB37_51D8_E999);
    }

    #[test]
    fn xxhash64_single_byte_is_the_reference_digest() {
        assert_eq!(xxhash64(b"a"), 0xD24E_C4F1_A98C_6E5B);
    }

    #[test]
    fn xxhash64_abc_is_the_reference_digest() {
        assert_eq!(xxhash64(b"abc"), 0x44BC_2CF5_AD77_0999);
    }

    #[test]
    fn xxhash64_xxhash_word_is_the_reference_digest() {
        // 6-byte input: exercises the 4-lane + single-byte tail path.
        assert_eq!(xxhash64(b"xxhash"), 0x32DD_3895_2C4B_C720);
    }

    #[test]
    fn xxhash64_hello_is_the_reference_digest() {
        assert_eq!(xxhash64(b"hello"), 0x26C7_827D_889F_6DA3);
    }

    #[test]
    fn xxhash64_handles_long_input_with_strip_loop() {
        // ≥ 32 bytes exercises the four-accumulator path; must be stable and seed-sensitive.
        let data: Vec<u8> = (0..64u8).collect();
        let h1 = xxhash64(&data);
        let h2 = xxhash64(&data);
        assert_eq!(h1, h2, "xxHash64 must be deterministic (§7.3)");
        assert_ne!(h1, xxhash64_with_seed(&data, 1), "seed must change the digest");
    }

    #[test]
    fn hash_fold_equal_values_equal_order_stable() {
        let a = derive_hash_u64(0, &[xxhash64(b"x"), xxhash64(b"y")]);
        let b = derive_hash_u64(0, &[xxhash64(b"x"), xxhash64(b"y")]);
        assert_eq!(a, b, "equal values must hash equal");
        let c = derive_hash_u64(0, &[xxhash64(b"y"), xxhash64(b"x")]);
        assert_ne!(a, c, "declaration order is normative (§6.7)");
        let d = derive_hash_u64(1, &[xxhash64(b"x"), xxhash64(b"y")]);
        assert_ne!(a, d, "discriminant feeds first (§6.7 Hash row)");
    }

    #[test]
    fn field_discriminant_is_i32_semantics() {
        assert_eq!(field_discriminant_i32(0), 0);
        assert_eq!(field_discriminant_i32(3), 3);
    }

    // ---------------------------------------------------------------- Clone

    #[test]
    fn clone_generates_per_field_in_declaration_order() {
        let (_src, mut files, mut h) = harness(&[("m/main.infy", "\n#[derive(Clone)]\nstruct Point { x: u32, y: i8 }\n")]);
        let n = expand_derives(&mut files, &mut h);
        assert_eq!(n, 1);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let t = one(&files, "impl Clone for Point");
        assert!(t.contains("Clone::clone(self.x)"), "{t}");
        assert!(t.contains("Clone::clone(self.y)"), "{t}");
        let ix = t.find("self.x").unwrap();
        let iy = t.find("self.y").unwrap();
        assert!(ix < iy, "field order = declaration order (§6.7): {t}");
    }

    #[test]
    fn clone_tuple_and_unit_forms() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Clone)]\nstruct Tup(u32, bool);\n#[derive(Clone)]\nstruct Uni;\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let t = one(&files, "impl Clone for Tup");
        assert!(t.contains("Tup(Clone::clone(self.0), Clone::clone(self.1))"), "{t}");
        let u = one(&files, "impl Clone for Uni");
        assert!(u.contains("return Uni;"), "{u}");
    }

    #[test]
    fn skip_clone_uses_default_when_available() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Clone)]\nstruct S { a: u32, #[skip_clone] b: str }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let t = one(&files, "impl Clone for S");
        assert!(t.contains("b: \"\""), "skip_clone field takes Default: {t}");
    }

    #[test]
    fn skip_clone_without_default_is_e0420() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Clone)]\nstruct S { a: u32, #[skip_clone] b: weird::Weird }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).contains(&"E-0420".to_string()), "{:?}", codes(&h));
        assert!(impl_texts(&files).is_empty(), "failed derive must not generate code");
    }

    // ---------------------------------------------------------------- Copy

    #[test]
    fn copy_ok_is_a_marker_impl() {
        let (_src, mut files, mut h) = harness(&[("m/main.infy", "\n#[derive(Copy)]\nstruct C { a: u32, b: bool }\n")]);
        let n = expand_derives(&mut files, &mut h);
        assert_eq!(n, 1);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let t = one(&files, "impl Copy for C");
        assert!(t.contains("{ }"), "Copy adds no code (§6.7): {t}");
    }

    #[test]
    fn copy_rejected_when_drop_impl_exists() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Copy)]\nstruct C { a: u32 }\nimpl Drop for C { fn drop(self: Self) { } }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).contains(&"E-0421".to_string()), "{:?}", codes(&h));
    }

    #[test]
    fn copy_rejected_with_dyn_field() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Copy)]\nstruct C { a: dyn std::fmt::Debug }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).contains(&"E-0421".to_string()), "{:?}", codes(&h));
    }

    #[test]
    fn copy_rejected_with_noncopy_field() {
        let (_src, mut files, mut h) = harness(&[("m/main.infy", "\n#[derive(Copy)]\nstruct C { a: str }\n")]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).contains(&"E-0421".to_string()), "str is not Copy (§6.7): {:?}", codes(&h));
    }

    // ---------------------------------------------------------------- Default

    #[test]
    fn default_primitives_per_spec_row() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Default)]\nstruct D { n: u32, b: bool, c: char, s: str, o: Option<u8> }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let t = one(&files, "impl Default for D");
        assert!(t.contains("n: 0"), "{t}");
        assert!(t.contains("b: false"), "{t}");
        assert!(t.contains("c: '\\0'"), "{t}");
        assert!(t.contains("s: \"\""), "{t}");
        assert!(t.contains("o: None"), "{t}");
    }

    #[test]
    fn default_result_is_err_of_default_error() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Default)]\nstruct D { r: Result<u32, u32> }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let t = one(&files, "impl Default for D");
        assert!(t.contains("r: Err(0)"), "Result → Err(E::default()): {t}");
    }

    #[test]
    fn default_enum_first_variant() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Default)]\nenum E { Green, Fetch(u32) }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let t = one(&files, "impl Default for E");
        assert!(t.contains("return E::Green;"), "enum Default = first variant (§6.7): {t}");
    }

    #[test]
    fn default_enum_picks_explicit_default_variant() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Default)]\nenum E { Green, #[default] Amber(u32), Red }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let t = one(&files, "impl Default for E");
        assert!(t.contains("E::Amber(0)"), "#[default] overrides first variant (§6.7): {t}");
    }

    #[test]
    fn multiple_default_variants_is_e0423() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Default)]\nenum E { #[default] A, #[default] B }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).contains(&"E-0423".to_string()), "{:?}", codes(&h));
    }

    #[test]
    fn enum_default_without_defaultable_first_variant_is_e0422() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Default)]\nenum E { V(weird::Weird), W }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).contains(&"E-0422".to_string()), "{:?}", codes(&h));
    }

    // ---------------------------------------------------------------- Debug

    #[test]
    fn debug_struct_named_form_with_declaration_order() {
        let (_src, mut files, mut h) = harness(&[("m/main.infy", "\n#[derive(Debug)]\nstruct Point { x: u32, y: i8 }\n")]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let t = one(&files, "impl Debug for Point");
        assert!(t.contains("std::fmt::format_struct(\"Point\""), "{t}");
        assert!(t.contains("\"x\", Debug::format(self.x)"), "{t}");
        assert!(t.contains("\"y\", Debug::format(self.y)"), "{t}");
        let ix = t.find("\"x\"").unwrap();
        let iy = t.find("\"y\"").unwrap();
        assert!(ix < iy, "Debug field order is golden-file normative (§6.7)");
    }

    #[test]
    fn debug_repr_c_uses_tuple_form() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Debug)]\n#[repr(C)]\nstruct R { a: u32 }\n",
        )]);
        expand_derives(&mut files, &mut h);
        let t = one(&files, "impl Debug for R");
        assert!(t.contains("std::fmt::format_tuple(\"R\""), "#[repr(C)] → Name(f1, f2) (§6.7): {t}");
        assert!(!t.contains("\"a\""), "tuple form has no field names: {t}");
    }

    #[test]
    fn debug_redact_field_prints_placeholder() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Debug)]\nstruct S { pub_key: str, #[redact] token: str }\n",
        )]);
        expand_derives(&mut files, &mut h);
        let t = one(&files, "impl Debug for S");
        assert!(t.contains("\"<redacted>\""), "#[redact] → <redacted> (§6.7): {t}");
        assert!(!t.contains("Debug::format(self.token)"), "{t}");
    }

    // ---------------------------------------------------------------- PartialEq / Eq

    #[test]
    fn partial_eq_chains_and_short_circuits_in_order() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(PartialEq)]\nstruct P { a: u32, b: u32 }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let t = one(&files, "impl PartialEq for P");
        assert!(t.contains("((self.a == other.a) && (self.b == other.b))"), "&& short-circuits (§6.7): {t}");
    }

    #[test]
    fn compare_skip_excludes_field_from_eq() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(PartialEq)]\nstruct P { a: u32, #[compare(skip)] ts: u64 }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let t = one(&files, "impl PartialEq for P");
        assert!(t.contains("self.a == other.a"), "{t}");
        assert!(!t.contains("ts"), "#[compare(skip)] field excluded (§6.7): {t}");
    }

    #[test]
    fn eq_generates_no_code() {
        let (_src, mut files, mut h) = harness(&[("m/main.infy", "\n#[derive(Eq)]\nstruct E { a: u32 }\n")]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let t = one(&files, "impl Eq for E");
        assert!(t.contains("{ }"), "Eq = marker check, no new code (§6.7): {t}");
    }

    #[test]
    fn eq_with_float_needs_allow_nan_equality_e0424() {
        let (_src, mut files, mut h) = harness(&[("m/main.infy", "\n#[derive(Eq)]\nstruct F { v: f64 }\n")]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).contains(&"E-0424".to_string()), "{:?}", codes(&h));
    }

    #[test]
    fn eq_with_float_and_allow_nan_passes() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Eq)]\n#[allow_nan_equality]\nstruct F { v: f64 }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(!codes(&h).contains(&"E-0424".to_string()), "{:?}", codes(&h));
        assert!(impl_texts(&files).iter().any(|t| t.contains("impl Eq for F")));
    }

    #[test]
    fn eq_with_non_eq_field_is_e0425() {
        let (_src, mut files, mut h) = harness(&[("m/main.infy", "\n#[derive(Eq)]\nstruct E { a: weird::Weird }\n")]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).contains(&"E-0425".to_string()), "{:?}", codes(&h));
    }

    // ---------------------------------------------------------------- PartialOrd / Ord

    #[test]
    fn ord_is_strict_lexicographic_over_three_fields() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Ord)]\nstruct T { a: u32, b: u32, c: u32 }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let t = one(&files, "impl Ord for T");
        // field priority = declaration order (§6.7 normative)
        let ia = t.find("self.a").unwrap();
        let ib = t.find("self.b").unwrap();
        let ic = t.find("self.c").unwrap();
        assert!(ia < ib && ib < ic, "lexicographic field order: {t}");
        assert!(t.contains("if (self.a < other.a) { return -1; }"), "{t}");
        assert!(t.contains("if (self.a > other.a) { return 1; }"), "{t}");
        assert!(t.contains("return 0; }"), "final equal case: {t}");
    }

    #[test]
    fn order_desc_flips_the_field_signs() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Ord)]\nstruct D { a: u32, #[order(desc)] b: u32 }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let t = one(&files, "impl Ord for D");
        assert!(t.contains("if (self.b < other.b) { return 1; }"), "#[order(desc)] flips (§6.7): {t}");
        assert!(t.contains("if (self.a < other.a) { return -1; }"), "asc default kept: {t}");
    }

    #[test]
    fn order_on_enum_compares_discriminant_first() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Ord)]\nenum E { A, B(u32) }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let t = one(&files, "impl Ord for E");
        assert!(t.contains("std::cmp::discriminant(self)"), "{t}");
        assert!(t.contains("std::cmp::discriminant(other)"), "{t}");
    }

    // ---------------------------------------------------------------- Hash

    #[test]
    fn hash_feeds_discriminant_then_fields_in_order() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Hash)]\nstruct H { a: u32, b: bool }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let t = one(&files, "impl Hash for H");
        let idisc = t.find("field_discriminant_i32(0)").unwrap();
        let ia = t.find("xxhash64_field(self.a)").unwrap();
        let ib = t.find("xxhash64_field(self.b)").unwrap();
        assert!(idisc < ia && ia < ib, "discriminant, then fields in declaration order (§6.7): {t}");
        assert!(t.contains("std::hash::feed_i32(state"), "{t}");
        assert!(t.contains("std::hash::feed_u64(state"), "{t}");
    }

    #[test]
    fn hash_enum_uses_variant_discriminants() {
        // §6.7: each variant feeds its discriminant (explicit `= 5` discriminants are honored when
        // the AST carries them; the current WIP parser delivers declaration indices, and the
        // generator's fallback `disc.unwrap_or(index)` covers both).
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Hash)]\nenum E { A, B, C(u32) }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let t = one(&files, "impl Hash for E");
        assert!(t.contains("field_discriminant_i32(0)"), "{t}");
        assert!(t.contains("field_discriminant_i32(1)"), "second variant discriminant: {t}");
        assert!(t.contains("field_discriminant_i32(2)"), "third variant discriminant: {t}");
        assert!(t.contains("xxhash64_field(__c_0)"), "payload field fed after its discriminant: {t}");
    }

    // ---------------------------------------------------------------- HasField

    #[test]
    fn hasfield_table_has_names_and_type_tags() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(HasField)]\nstruct S { a: u32, b: Option<bool> }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let t = one(&files, "impl HasField for S");
        assert!(t.contains("__hasfield_field_count() -> u32 { return 2; }"), "{t}");
        assert!(t.contains("__hasfield_field_name"), "{t}");
        assert!(t.contains("\"a\""), "{t}");
        assert!(t.contains("\"b\""), "{t}");
        assert!(t.contains("Option<bool>"), "type tag string: {t}");
        assert!(!t.contains("offset_of"), "offsets only for #[repr(C)] (§6.7 HasField): {t}");
    }

    #[test]
    fn hasfield_offsets_only_under_repr_c() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(HasField)]\n#[repr(C)]\nstruct S { a: u32, b: bool }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let t = one(&files, "impl HasField for S");
        assert!(t.contains("__hasfield_field_offset"), "{t}");
        assert!(t.contains("std::mem::offset_of(\"a\")"), "{t}");
    }

    /// F9: `derive` was the only producer of a second integer shape, so what it generates has to be
    /// the shape `parser::primary` produces — otherwise stages [4]/[7]/[8] match one form while
    /// `derive` builds another. The text test above cannot tell those two apart (both reprint the
    /// same digits), so this one reads the AST node itself.
    #[test]
    fn generated_integer_literals_use_the_single_source_literal_shape() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(HasField)]\nstruct S { a: u32, b: bool }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let mut found: Option<&Expr> = None;
        for pf in files.values() {
            for it in &pf.items {
                let ItemKind::Impl { items, .. } = &it.kind else { continue };
                for m in items {
                    let ItemKind::Fn { sig, body, .. } = &m.kind else { continue };
                    if sig.name != "__hasfield_field_count" {
                        continue;
                    }
                    let crate::ast::Body::Block(b) = body else { panic!("block body expected") };
                    let StmtKind::Return { value: Some(e) } = &b.stmts[0].kind else {
                        panic!("generated field count must be a `return <literal>`")
                    };
                    found = Some(e);
                }
            }
        }
        let e = found.expect("`__hasfield_field_count` must exist in the generated impl");
        match &e.kind {
            ExprKind::Lit(Lit::Int(i)) => assert_eq!(
                (i.digits.as_str(), i.radix, i.suffix.as_deref(), i.value),
                ("2", Radix::Dec, None, Some(2)),
                "the derived literal must carry the parser's fields"
            ),
            other => panic!("derived literals must use the parser's shape, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------- derive ordering / E-0426

    #[test]
    fn multiple_derives_expand_in_written_order() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Debug, Clone)]\nstruct O { a: u32 }\n",
        )]);
        let n = expand_derives(&mut files, &mut h);
        assert_eq!(n, 2);
        let t = impl_texts(&files);
        assert_eq!(t.len(), 2, "{t:?}");
        assert!(t[0].contains("impl Debug"), "written order first (§6.7): {t:?}");
        assert!(t[1].contains("impl Clone"), "written order second (§6.7): {t:?}");
    }

    #[test]
    fn duplicate_derive_is_e0426_and_expands_once() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Clone, Clone)]\nstruct S { a: u32 }\n",
        )]);
        let n = expand_derives(&mut files, &mut h);
        assert!(codes(&h).contains(&"E-0426".to_string()), "{:?}", codes(&h));
        assert_eq!(n, 1, "duplicate derive must not double-generate");
    }

    #[test]
    fn unknown_derive_trait_is_e0410() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Serializable)]\nstruct S { a: u32 }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).contains(&"E-0410".to_string()), "closed derive set (§6.7): {:?}", codes(&h));
    }

    #[test]
    fn derive_needing_unsafe_pointer_is_e0427() {
        let (_src, mut files, mut h) = harness(&[(
            "m/main.infy",
            "\n#[derive(Clone)]\nstruct S { p: *const u32 }\n",
        )]);
        expand_derives(&mut files, &mut h);
        assert!(codes(&h).contains(&"E-0427".to_string()), "{:?}", codes(&h));
        assert!(impl_texts(&files).is_empty(), "no silent unsafe insertion (§6.7)");
    }

    // ---------------------------------------------------------------- validate_attrs (§6.6)

    fn vattrs(text: &str) -> (Sources, BTreeMap<FileId, ParsedFile>, Handler) {
        let (src, files, mut h) = harness(&[("m/main.infy", text)]);
        validate_attrs(&files, &src, &mut h);
        (src, files, h)
    }

    #[test]
    fn closed_list_accepts_every_documented_attr() {
        let text = r#"
#![infy_version = "2.0"]
#[test]
fn t() { }
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, HasField)]
struct S { a: u32 }
impl S { }
"#;
        let (_s, _f, h) = vattrs(text);
        assert!(codes(&h).is_empty(), "§6.6/§6.7 union must accept all of these: {:?}", codes(&h));
    }

    #[test]
    fn prose_effect_attrs_are_accepted() {
        let text = "\n#[panicable]\n#[effectful]\n#[must_use]\n#[must_drop]\n#[owned]\nfn f() { }\n";
        let (_s, _f, h) = vattrs(text);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
    }

    #[test]
    fn unknown_attr_is_e0410() {
        let text = "\n#[myattr]\nfn f() { }\n";
        let (_s, _f, h) = vattrs(text);
        assert!(codes(&h).contains(&"E-0410".to_string()), "{:?}", codes(&h));
    }

    #[test]
    fn cfg_attr_is_e0410_with_f1_pointer() {
        let text = "\n#[cfg(windows)]\nfn f() { }\n";
        let (_s, _f, h) = vattrs(text);
        let d = h.diags.iter().find(|d| d.code == "E-0410").unwrap();
        assert!(d.helps.iter().any(|s| s.contains("infy:build")), "§F.1: no #[cfg] — see //infy:build: {:?}", d.helps);
    }

    #[test]
    fn allow_is_not_an_attribute_in_infy() {
        // §6.6's closed list has no `#[allow]`; §9.6 adjusts levels via `[lints]`.
        let text = "\n#[allow(unused)]\nfn f() { }\n";
        let (_s, _f, h) = vattrs(text);
        assert!(codes(&h).contains(&"E-0410".to_string()), "{:?}", codes(&h));
    }

    #[test]
    fn align_requires_power_of_two() {
        let ok = "\n#[align(8)]\nstruct A { a: u32 }\n";
        let (_s, _f, h) = vattrs(ok);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let bad = "\n#[align(7)]\nstruct B { a: u32 }\n";
        let (_s, _f, h) = vattrs(bad);
        assert!(codes(&h).contains(&"E-0410".to_string()), "{:?}", codes(&h));
    }

    #[test]
    fn doc_only_supports_hidden() {
        let ok = "\n#[doc(hidden)]\nfn f() { }\n";
        let (_s, _f, h) = vattrs(ok);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
        let bad = "\n#[doc(not_a_real_option)]\nfn g() { }\n";
        let (_s, _f, h) = vattrs(bad);
        assert!(codes(&h).contains(&"E-0410".to_string()), "{:?}", codes(&h));
    }

    #[test]
    fn repr_only_supports_c() {
        let bad = "\n#[repr(transparent)]\nenum E { A }\n";
        let (_s, _f, h) = vattrs(bad);
        assert!(codes(&h).contains(&"E-0410".to_string()), "§6.6 lists only #[repr(C)]: {:?}", codes(&h));
        let ok = "\n#[repr(C)]\nenum E { A }\n";
        let (_s, _f, h) = vattrs(ok);
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
    }

    #[test]
    fn misplaced_attr_is_e0410() {
        let text = "\n#[test]\nstruct S { a: u32 }\n";
        let (_s, _f, h) = vattrs(text);
        assert!(codes(&h).contains(&"E-0410".to_string()), "#[test] on struct is misplaced: {:?}", codes(&h));
    }

    #[test]
    fn inner_attr_on_item_is_e0410() {
        let text = "#![entry_chain = \"Foo\"]\nfn f() { }\n";
        let (_s, _f, h) = vattrs(text);
        assert!(codes(&h).is_empty(), "file-scope inner attr before package/items is legal (§3.1): {:?}", codes(&h));
        let bad = "\n#[derive(Clone)]\nfn f() { }\n";
        let (_s, _f, h) = vattrs(bad);
        assert!(codes(&h).contains(&"E-0410".to_string()), "derive on fn is misplaced (§6.7): {:?}", codes(&h));
    }

    #[test]
    fn field_attrs_are_checked() {
        let text = "\nstruct S { #[weird_field_attr] a: u32 }\n";
        let (_s, _f, h) = vattrs(text);
        assert!(codes(&h).contains(&"E-0410".to_string()), "{:?}", codes(&h));
        let text2 = "\n#[derive(Ord)]\nstruct T { #[order(bogus)] a: u32 }\n";
        let (_s, _f, h) = vattrs(text2);
        assert!(codes(&h).contains(&"E-0410".to_string()), "{:?}", codes(&h));
    }

    #[test]
    fn deprecated_use_reports_w0400() {
        let text = r#"
#[deprecated("use new_thing")]
pub fn old_thing() { }
fn caller() {
    old_thing();
}
"#;
        let (_s, _f, h) = vattrs(text);
        let d = h.diags.iter().find(|d| d.code == "W-0400");
        assert!(d.is_some(), "usage of deprecated item → W-0400 (§6.6): {:?}", codes(&h));
        assert!(d.unwrap().notes.iter().any(|n| n.contains("use new_thing")));
    }

    #[test]
    fn deprecated_definition_itself_is_not_a_use() {
        let text = "\n#[deprecated(\"x\")]\npub fn only_def() { }\n";
        let (_s, _f, h) = vattrs(text);
        assert!(!codes(&h).contains(&"W-0400".to_string()), "{:?}", codes(&h));
    }

    // ---------------------------------------------------------------- expand_derives returns count

    #[test]
    fn expand_derives_counts_generated_impls_across_files() {
        let (_src, mut files, mut h) = harness(&[
            ("m/a.infy", "\n#[derive(Clone)]\nstruct A { a: u32 }\n"),
            ("m/b.infy", "\n#[derive(Debug)]\nstruct B { b: bool }\n"),
        ]);
        let n = expand_derives(&mut files, &mut h);
        assert_eq!(n, 2);
        assert_eq!(impl_texts(&files).len(), 2);
    }

    #[test]
    fn generated_products_are_real_ast_impls() {
        let (_src, mut files, mut h) = harness(&[("m/main.infy", "\n#[derive(PartialEq)]\nstruct P { a: u32 }\n")]);
        expand_derives(&mut files, &mut h);
        let found = files
            .values()
            .flat_map(|f| f.items.iter())
            .find(|i| matches!(&i.kind, ItemKind::Impl { trait_: Some(t), .. } if t.last().map(|x| x.as_str()) == Some("PartialEq")));
        assert!(found.is_some(), "derive must emit real ItemKind::Impl nodes (§6.8)");
    }
}
