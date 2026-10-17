//! AST for the whole of §3 (EBNF), including §3.11/§3.12/§3.13 completions, §3.6 embed blocks and
//! §3.9 macro declarations. Every node carries a `Span`: §9.2 diagnostics must point at a column
//! and §12.3 goldens compare source positions.
//!
//! The AST is deliberately *surface-shaped*, not desugared: §4.11 defines the `?` expansion, §6.5
//! puts macro expansion before name resolution, and §6.8 requires `infy expand` to reprint
//! expandable source. Desugaring therefore happens in `sema`/`codegen`, never in the parser.

use crate::span::Span;
use crate::token::{Lit, Punct, Tree};

#[derive(Clone, Debug, Default)]
pub struct Docs {
    /// `///` lines attached to the following item (§2.2), in source order, marker stripped.
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Visibility {
    /// Package private (§5.5).
    Private,
    Pub,
    PubCrate,
    /// `pub(<path>)` — directed export.
    PubTo(String),
    Priv,
}

#[derive(Clone, Debug)]
pub struct Import {
    pub pub_: bool,
    /// The literal's *value*, e.g. `github.com/acme/ci/core/build`. §5.3 step 1 allows only a
    /// string literal, so the parser rejects every other form with `E-0504`.
    pub path: String,
    pub path_span: Span,
    pub alias: ImportAlias,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportAlias {
    /// Default: local name = last path segment (§5.3).
    None,
    Named(String),
    /// `as .` — link symbols only, bind no name.
    LinkOnly,
    /// `as _` — run package initialisers only (Go blank import).
    EffectOnly,
}

#[derive(Clone, Debug)]
pub struct AttrArg {
    pub name: Option<String>,
    pub value: Option<AttrValue>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum AttrValue {
    Str(String),
    Int(u128),
    Bool(bool),
    Expr(Expr),
    /// `derive(Clone, Copy)` → `Idents(["Clone", "Copy"])`.
    Idents(Vec<String>),
    Path(Vec<String>),
    /// `#[borrows('a)]` / `#[repr(C)]`.
    Lifetime(String),
}

#[derive(Clone, Debug)]
pub struct Attr {
    /// `#![...]` (file scope) vs `#[...]` (item scope).
    pub inner: bool,
    /// `attr_path`, e.g. `["link"]`, `["infy_version"]`.
    pub path: Vec<String>,
    pub args: Vec<AttrArg>,
    /// Arguments written without `name =`, in source order.
    pub positional: Vec<AttrValue>,
    pub span: Span,
}

impl Attr {
    pub fn name(&self) -> &str {
        self.path.last().map(|s| s.as_str()).unwrap_or("")
    }
    pub fn full(&self) -> String {
        self.path.join("::")
    }
    pub fn str_arg(&self, name: &str) -> Option<String> {
        self.args.iter().find(|a| a.name.as_deref() == Some(name)).and_then(|a| match &a.value {
            Some(AttrValue::Str(s)) => Some(s.clone()),
            _ => None,
        })
    }
    pub fn bool_arg(&self, name: &str) -> Option<bool> {
        for a in &self.args {
            if a.name.as_deref() != Some(name) {
                continue;
            }
            match &a.value {
                Some(AttrValue::Bool(b)) => return Some(*b),
                Some(AttrValue::Expr(e)) => match &e.kind {
                    ExprKind::Lit(Lit::Bool(b)) => return Some(*b),
                    ExprKind::Ident(s) if s == "true" || s == "false" => return Some(s == "true"),
                    _ => {}
                },
                _ => {}
            }
        }
        None
    }
    /// `#[repr(C)]`, `#[derive(Clone)]` style positional names.
    pub fn has_ident(&self, name: &str) -> bool {
        self.positional.iter().any(|v| match v {
            AttrValue::Idents(xs) => xs.iter().any(|x| x == name),
            AttrValue::Path(p) => p.last().map(|s| s == name).unwrap_or(false),
            _ => false,
        }) || self.args.iter().any(|a| a.name.as_deref() == Some(name))
    }
    pub fn idents(&self) -> Vec<String> {
        let mut v = Vec::new();
        for p in &self.positional {
            match p {
                AttrValue::Idents(xs) => v.extend(xs.iter().cloned()),
                AttrValue::Path(xs) => v.extend(xs.iter().cloned()),
                _ => {}
            }
        }
        for a in &self.args {
            if let Some(AttrValue::Idents(xs)) = &a.value {
                v.extend(xs.iter().cloned());
            }
        }
        v
    }
}

#[derive(Clone, Debug)]
pub struct GenericParam {
    pub name: String,
    pub bounds: Vec<Bound>,
    pub default: Option<Type>,
    /// `true` for `'a` (§2.5), `false` for a type parameter.
    pub is_lifetime: bool,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Bound {
    /// `Trait`, `Trait<A, B>`, `std::fmt::Display`, or a lifetime name.
    pub path: Vec<String>,
    pub args: Vec<Type>,
    pub is_lifetime: bool,
    pub negated: bool,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct WherePred {
    pub ty: Type,
    pub bounds: Vec<Bound>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub mutable: bool,
    pub name: String,
    pub ty: Type,
    pub default: Option<Expr>,
    pub span: Span,
    pub name_span: Span,
    /// `_` / omitted names, legal in trait declarations.
    pub is_wildcard: bool,
}

#[derive(Clone, Debug)]
pub enum Body {
    Block(Block),
    /// `@track { ... }` as the entire body (§3.6 / §7.6.1).
    Embed(EmbedBlock),
    /// `;` — no local implementation: A-track `extern` or host-provided event (§3.3).
    Foreign,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FnSig {
    pub name: String,
    pub name_span: Span,
    pub generics: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub ret: Option<Type>,
    pub where_preds: Vec<WherePred>,
    /// `unsafe fn` (§4.7.5).
    pub is_unsafe: bool,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub vis: Visibility,
    pub name: String,
    pub ty: Type,
    pub attrs: Vec<Attr>,
    pub docs: Docs,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Variant {
    pub name: String,
    /// `Fetch(u32, str)`.
    pub tuple: Vec<(Visibility, Type)>,
    /// `Fetch { code: u32 }`.
    pub struct_fields: Vec<Field>,
    /// Explicit discriminant, `Fetch = 3`.
    pub disc: Option<u128>,
    pub attrs: Vec<Attr>,
    pub docs: Docs,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ItemKind {
    Event { sig: FnSig, body: Body },
    Fn { sig: FnSig, body: Body },
    Struct { name: String, generics: Vec<GenericParam>, fields: Vec<Field>, tuple: Vec<(Visibility, Type)>, where_preds: Vec<WherePred> },
    Enum { name: String, generics: Vec<GenericParam>, variants: Vec<Variant>, where_preds: Vec<WherePred> },
    Union { name: String, generics: Vec<GenericParam>, fields: Vec<Field> },
    Trait { name: String, generics: Vec<GenericParam>, is_unsafe: bool, items: Vec<Item> },
    Impl { generics: Vec<GenericParam>, trait_: Option<Vec<String>>, trait_args: Vec<Type>, self_ty: Type, items: Vec<Item> },
    TypeAlias { name: String, generics: Vec<GenericParam>, ty: Type },
    ExternBlock { abi: Option<String>, decls: Vec<Item> },
    Const { name: String, name_span: Span, ty: Option<Type>, init: Option<Expr> },
    Static { name: String, name_span: Span, ty: Option<Type>, init: Option<Expr>, is_mut: bool },
    MacroRules { name: String, arms: Vec<MacroArm> },
    Chain { name: String, name_span: Span, stmts: Vec<Stmt> },
    Mod { name: String },
}

#[derive(Clone, Debug)]
pub struct Item {
    pub attrs: Vec<Attr>,
    pub docs: Docs,
    pub vis: Visibility,
    pub span: Span,
    pub kind: ItemKind,
}

impl Item {
    pub fn name(&self) -> &str {
        match &self.kind {
            ItemKind::Event { sig, .. } | ItemKind::Fn { sig, .. } => &sig.name,
            ItemKind::Struct { name, .. } | ItemKind::Enum { name, .. } | ItemKind::Union { name, .. }
            | ItemKind::Trait { name, .. } | ItemKind::TypeAlias { name, .. }
            | ItemKind::Const { name, .. } | ItemKind::Static { name, .. }
            | ItemKind::MacroRules { name, .. } | ItemKind::Chain { name, .. }
            | ItemKind::Mod { name, .. } => name,
            ItemKind::Impl { .. } | ItemKind::ExternBlock { .. } => "",
        }
    }
    pub fn has_attr(&self, name: &str) -> bool {
        self.attrs.iter().any(|a| a.path.last().map(|s| s == name).unwrap_or(false))
    }
    pub fn attr(&self, name: &str) -> Option<&Attr> {
        self.attrs.iter().find(|a| a.path.last().map(|s| s == name).unwrap_or(false))
    }
    pub fn is_foreign_fn(&self) -> bool {
        matches!(&self.kind, ItemKind::Fn { body: Body::Foreign, .. } | ItemKind::Event { body: Body::Foreign, .. })
    }
}

#[derive(Clone, Debug)]
pub struct MacroArm {
    /// Raw matcher token trees; `$name:classifier` and repetitions stay uninterpreted so the
    /// expander can match literally (§6.2).
    pub matcher: Vec<Tree>,
    pub expander: Vec<Tree>,
    pub separator: Option<Punct>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct EmbedBlock {
    /// `py` `lua` `c` `cpp` `rust` `zig` `ts` `js` `sh` `ps1` `bat` `cmd`.
    pub lang: String,
    pub lang_span: Span,
    pub args: Vec<EmbedArg>,
    /// Raw body source, braces excluded — lexically opaque (§2.11).
    pub body: String,
    pub body_span: Span,
    pub span: Span,
}

impl EmbedBlock {
    pub fn is_a_track(&self) -> bool {
        matches!(self.lang.as_str(), "c" | "cpp" | "rust" | "zig")
    }
    pub fn is_b_track(&self) -> bool {
        matches!(self.lang.as_str(), "py" | "lua")
    }
    pub fn is_process_only(&self) -> bool {
        matches!(self.lang.as_str(), "ts" | "js" | "sh" | "ps1" | "bat" | "cmd")
    }
    pub fn arg(&self, name: &str) -> Option<&Expr> {
        self.args.iter().find(|a| a.name == name).map(|a| &a.value)
    }
    pub fn unit_name(&self) -> Option<String> {
        self.arg("name").and_then(|e| match &e.kind {
            ExprKind::Lit(Lit::Str(s)) => Some(s.clone()),
            _ => None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct EmbedArg {
    pub name: String,
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Type {
    pub kind: TypeKind,
    pub span: Span,
}

impl Type {
    pub fn invalid(span: Span) -> Type {
        Type { kind: TypeKind::Invalid, span }
    }
    pub fn prim(p: Prim, span: Span) -> Type {
        Type { kind: TypeKind::Prim(p), span }
    }
    pub fn named(path: Vec<String>, args: Vec<Type>, span: Span) -> Type {
        Type { kind: TypeKind::Named { path, args }, span }
    }
}

#[derive(Clone, Debug)]
pub enum TypeKind {
    Prim(Prim),
    /// `Foo`, `foo::Bar`, `Option<u8>`, `Vec<T>` — sema decides what it means.
    Named { path: Vec<String>, args: Vec<Type> },
    Ref { lifetime: Option<String>, mutable: bool, inner: Box<Type> },
    Ptr { mutable: bool, inner: Box<Type> },
    Array { inner: Box<Type>, len: Box<Expr> },
    Slice { inner: Box<Type> },
    Dyn { path: Vec<String>, bounds: Vec<Bound> },
    FnPtr { params: Vec<Type>, ret: Option<Box<Type>> },
    Tuple(Vec<Type>),
    Never,
    /// Parse-error placeholder: never type-checks, never codegens.
    Invalid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Prim {
    U8, U16, U32, U64, U128, I8, I16, I32, I64, I128, F32, F64, Bool, Str, Char, Unit, Usize, Isize,
}

impl Prim {
    pub fn text(&self) -> &'static str {
        match self {
            Prim::U8 => "u8", Prim::U16 => "u16", Prim::U32 => "u32", Prim::U64 => "u64",
            Prim::U128 => "u128", Prim::I8 => "i8", Prim::I16 => "i16", Prim::I32 => "i32",
            Prim::I64 => "i64", Prim::I128 => "i128", Prim::F32 => "f32", Prim::F64 => "f64",
            Prim::Bool => "bool", Prim::Str => "str", Prim::Char => "char", Prim::Unit => "()",
            Prim::Usize => "usize", Prim::Isize => "isize",
        }
    }
    pub fn from_text(s: &str) -> Option<Prim> {
        Some(match s {
            "u8" => Prim::U8, "u16" => Prim::U16, "u32" => Prim::U32, "u64" => Prim::U64,
            "u128" => Prim::U128, "i8" => Prim::I8, "i16" => Prim::I16, "i32" => Prim::I32,
            "i64" => Prim::I64, "i128" => Prim::I128, "f32" => Prim::F32, "f64" => Prim::F64,
            "bool" => Prim::Bool, "str" => Prim::Str, "char" => Prim::Char, "()" => Prim::Unit,
            "usize" => Prim::Usize, "isize" => Prim::Isize,
            _ => return None,
        })
    }
    pub fn is_integer(&self) -> bool {
        !matches!(self, Prim::F32 | Prim::F64 | Prim::Bool | Prim::Str | Prim::Char | Prim::Unit)
    }
    pub fn is_float(&self) -> bool {
        matches!(self, Prim::F32 | Prim::F64)
    }
    pub fn is_numeric(&self) -> bool { self.is_integer() || self.is_float() }
    pub fn is_signed(&self) -> bool {
        matches!(self, Prim::I8 | Prim::I16 | Prim::I32 | Prim::I64 | Prim::I128 | Prim::Isize)
    }
    pub fn bits(&self) -> u32 {
        match self {
            Prim::U8 | Prim::I8 => 8, Prim::U16 | Prim::I16 => 16,
            Prim::U32 | Prim::I32 | Prim::F32 => 32, Prim::U64 | Prim::I64 | Prim::F64 => 64,
            Prim::U128 | Prim::I128 => 128, Prim::Usize | Prim::Isize => 64,
            Prim::Bool | Prim::Char => 32, Prim::Str | Prim::Unit => 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

impl Expr {
    pub fn new(kind: ExprKind, span: Span) -> Expr {
        Expr { kind, span }
    }
}

#[derive(Clone, Debug)]
pub enum FieldName {
    Named(String),
    /// `t.0` (§3.11: `.0` after an expression is a tuple field; `1.0` is always a float).
    Index(u32),
}

#[derive(Clone, Debug)]
pub struct CallArg {
    /// `f(retries=auto)` — named argument (§2.6 / §10.11).
    pub name: Option<String>,
    pub expr: Expr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    /// Integer literals carry `digits`/`suffix`/`value` (§2.8 range checks happen in stage [4]),
    /// and **both** producers — `parser::primary` and `derive`'s generated bodies — build this one
    /// shape. A second int shape is what BACKLOG F9 removed: predicates that matched only the other
    /// one silently dropped literals, and the two lowerings of the same literal had already drifted.
    Lit(Lit),
    /// `auto`, legal only in named-argument position (§2.6, `E-0030`).
    Auto,
    Ident(String),
    Path(Vec<String>),
    Unary { op: Punct, expr: Box<Expr> },
    Binary { op: Punct, lhs: Box<Expr>, rhs: Box<Expr> },
    Assign { target: Box<Expr>, op: Punct, value: Box<Expr> },
    Call { callee: Box<Expr>, args: Vec<CallArg> },
    Method { recv: Box<Expr>, name: String, turbofish: Vec<Type>, args: Vec<CallArg> },
    Field { base: Box<Expr>, name: FieldName },
    Index { base: Box<Expr>, index: Box<Expr> },
    Tuple(Vec<Expr>),
    Array(Vec<Expr>),
    ArrayRepeat { value: Box<Expr>, len: Box<Expr> },
    StructLit { path: Vec<String>, fields: Vec<(String, Expr)>, rest: Option<Box<Expr>> },
    If { cond: Box<Expr>, then: Box<Block>, else_: Option<Box<ElseBranch>> },
    Match { scrut: Box<Expr>, arms: Vec<MatchArm> },
    Block(Box<Block>),
    Unsafe(Box<Block>),
    Closure { move_: bool, params: Vec<(String, Option<Type>)>, ret: Option<Type>, body: Box<Block> },
    Ref { mutable: bool, lifetime: Option<String>, expr: Box<Expr> },
    Deref { expr: Box<Expr> },
    Try { expr: Box<Expr> },
    Cast { expr: Box<Expr>, ty: Type },
    Is { expr: Box<Expr>, ty: Type },
    Move { expr: Box<Expr> },
    Range { lo: Option<Box<Expr>>, hi: Option<Box<Expr>>, inclusive: bool },
    Embed(EmbedBlock),
    MacroCall { name: String, delim: crate::token::Delim, tokens: Vec<Tree> },
    Invalid,
}

#[derive(Clone, Debug)]
pub enum ElseBranch {
    Block(Block),
    If(Stmt),
}

impl ElseBranch {
    pub fn as_block(&self) -> Option<&Block> {
        match self {
            ElseBranch::Block(b) => Some(b),
            ElseBranch::If(_) => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MatchArm {
    pub pat: Pat,
    pub guard: Option<Expr>,
    pub body: ArmBody,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ArmBody {
    Expr(Expr),
    Block(Block),
}

#[derive(Clone, Debug)]
pub struct Pat {
    pub kind: PatKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum PatKind {
    Wildcard,
    /// `..` inside slice patterns.
    DotDot,
    Lit(Lit),
    /// Binding; `ref`/`mut`/`@ subpat` per §3.12.
    Binding { name: String, by_ref: bool, mutable: bool, sub: Option<Box<Pat>> },
    /// `Phase::Fetch`, `None`, `Vec`, or a const path `Const::NAME`.
    Path(Vec<String>),
    /// `Variant(p, p)` / `Variant { field: p }` / `Foo::Bar { .. }`.
    Constructor { path: Vec<String>, args: Vec<Pat>, fields: Vec<(String, Pat)>, has_rest: bool },
    Tuple(Vec<Pat>),
    /// `[a, b]`, `[head, ..rest]`.
    Slice(Vec<Pat>, Option<Box<Pat>>),
    Or(Vec<Pat>),
    Range { lo: Box<Pat>, hi: Box<Pat>, inclusive: bool },
    /// `!(a | b)` (§3.12), finite integer/enum domains only.
    Exclude(Vec<Pat>),
    Invalid,
}

#[derive(Clone, Debug)]
pub enum StmtKind {
    /// `let` (immutable) and `var` (mutable) — §4.3 keeps them distinct (`E-0021`/`E-0022`).
    Let { is_var: bool, name: String, name_span: Span, ty: Option<Type>, init: Option<Expr> },
    /// `a[i] = v`, `x += 1`; the target must be a place expression (§3.5).
    Assign { target: Expr, op: Punct, value: Expr },
    /// A host statement, optionally followed by linkage blocks (§8.1; a block with no host
    /// statement is `E-0300`).
    Expr { expr: Expr, linkage: Vec<LinkageBlock> },
    If { cond: Expr, then: Block, else_: Option<Box<ElseBranch>> },
    While { cond: Expr, body: Block },
    Loop { body: Block, label: Option<String> },
    For { pat: Pat, iter: Expr, body: Block, label: Option<String> },
    Match { scrut: Expr, arms: Vec<MatchArm> },
    Break { label: Option<String>, value: Option<Expr> },
    Continue { label: Option<String> },
    Return { value: Option<Expr> },
    Unsafe(Block),
    Defer(Block),
    ConstAssert { cond: Expr, msg: Option<String> },
    Label { name: String, name_span: Span },
    /// `pause` / `stop` / `resume` as plain statements (§8.1).
    Pause,
    Stop,
    Resume,
    /// A declaration where only statements are legal — kept for error reporting.
    NestedItem(Item),
}

#[derive(Clone, Debug)]
pub struct Stmt {
    pub kind: StmtKind,
    pub attrs: Vec<Attr>,
    pub docs: Docs,
    pub span: Span,
    /// Whether a trailing `;` was present in source (§2.11 `E-0053` depends on this).
    pub terminated: bool,
}

#[derive(Clone, Debug)]
pub struct LinkageBlock {
    pub stmts: Vec<LinkageStmt>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum LinkageStmt {
    Goto(GotoTarget),
    Start { chain: String, chain_span: Span, node: Option<StartNode> },
    StartFlag,
    Pause,
    Stop,
    Resume,
    If { cond: Expr, then: Vec<LinkageStmt>, else_: Option<Vec<LinkageStmt>> },
    /// Any plain statement may also run inside a linkage block (§3.5).
    Plain(Stmt),
}

#[derive(Clone, Debug)]
pub enum GotoTarget {
    /// `goto("anchor")` — resolved at compile time (`E-0301`).
    Named(String),
    /// `goto(5)` absolute node index (always annotated `N-0800`).
    Abs(u128),
    /// `goto(+3)` / `goto(-1)` (§3.10 disambiguation).
    Rel(i128),
}

#[derive(Clone, Debug)]
pub enum StartNode {
    Index(u128),
    Anchor(String),
    Ident(String),
}

/// One parsed `.infy` file (§3.1).
#[derive(Clone, Debug)]
pub struct ParsedFile {
    pub shebang: Option<String>,
    /// Raw text of `//infy:build <constraint>` (§2.2).
    pub build_constraint: Option<String>,
    pub build_constraint_line: Option<u32>,
    pub build_constraint_span: Option<Span>,
    pub file_attrs: Vec<Attr>,
    pub package: Option<String>,
    pub package_span: Option<Span>,
    pub imports: Vec<Import>,
    pub items: Vec<Item>,
    pub span: Span,
}

impl ParsedFile {
    pub fn empty(file: crate::span::FileId) -> ParsedFile {
        ParsedFile {
            shebang: None,
            build_constraint: None,
            build_constraint_line: None,
            build_constraint_span: None,
            file_attrs: Vec::new(),
            package: None,
            package_span: None,
            imports: Vec::new(),
            items: Vec::new(),
            span: Span::empty_at(file, 0),
        }
    }

    pub fn file_attr(&self, name: &str) -> Option<&Attr> {
        self.file_attrs.iter().find(|a| a.path.last().map(|s| s == name).unwrap_or(false))
    }
}
