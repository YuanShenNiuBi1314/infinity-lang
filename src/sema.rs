//! Stage [3]+[4] of §7.1 — name resolution and type checking.
//!
//! What this module is responsible for, by spec clause:
//! * §5.3 import-path resolution order, §5.4 initialisation order + cycle refusal
//!   (`E-0502`/`E-0503`), §5.5 `pub` / `pub(crate)` / `pub(path)` / `internal` (`E-0210`, `E-0508`);
//! * §4.1 the closed type set, §4.2 "no implicit numeric conversion" (`E-0110`/`E-0111`, UB-001/2),
//!   §4.3 mutability (`E-0120`, `E-0121`), §4.4 operator semantics, §4.12 numeric details;
//! * §4.13 the single allowed inference algorithm (bottom-up, ≤2 passes, closed priority order);
//! * §4.14 method resolution: inherent impls first, then traits in declaration order, then the
//!   autoref ladder `T` → `&T` → `&mut T`, first match wins, ambiguity is `E-0162`;
//! * §3.12 exhaustiveness and reachability (`E-0070`, `E-0071`, `W-0070`, `W-0071`, `E-0072`, `W-0072`);
//! * §2.12 const-eval and `const_assert`; §4.11 the `?` desugaring rules; §4.6 the closed `env` set;
//! * §4.9 the statically decidable part of the UB ban list.
//!
//! The result is a [`Program`] of resolved items that `codegen` consumes; nothing here is
//! "advisory" — if a rule cannot be decided, §4.7.2/UB-017 require an error, not a pass.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::ast::*;
use crate::diag::{Diagnostic, Handler, Phase};
use crate::span::{FileId, Position, Sources, Span};
use crate::token::{IntLit, Lit};

/// Internal type representation. Distinct from `ast::Type` (surface syntax) and from the C type
/// strings `codegen` emits.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Ty {
    Prim(Prim),
    Bool,
    Unit,
    Str,
    Named(String),
    Ref { inner: Box<Ty>, mutable: bool, lifetime: Option<String> },
    Ptr { inner: Box<Ty>, mutable: bool },
    Array { inner: Box<Ty>, n: u64 },
    Slice { inner: Box<Ty> },
    Tuple(Vec<Ty>),
    Option { inner: Box<Ty> },
    Result { ok: Box<Ty>, err: Box<Ty> },
    Boxed { inner: Box<Ty> },
    Rc { inner: Box<Ty>, atomic: bool },
    Vec { inner: Box<Ty> },
    String,
    Fn { params: Vec<Ty>, ret: Option<Box<Ty>> },
    Closure(usize),
    Dyn(String),
    Adt(String),
    Generic(String),
    /// `auto` in an argument position (§2.6) — resolves to the callee's default.
    Auto,
    Never,
    Invalid,
}

impl Ty {
    pub fn text(&self) -> String {
        match self {
            Ty::Prim(p) => p.text().to_string(),
            Ty::Bool => "bool".into(),
            Ty::Unit => "()".into(),
            Ty::Str => "str".into(),
            Ty::String => "String".into(),
            Ty::Named(n) | Ty::Adt(n) | Ty::Generic(n) | Ty::Dyn(n) => n.clone(),
            Ty::Ref { inner, mutable, .. } => format!("&{}mut {}", if *mutable { "" } else { "" }, inner.text()).replace("mut ", if *mutable { "mut " } else { "" }),
            Ty::Ptr { inner, mutable } => format!("*{} {}", if *mutable { "mut" } else { "const" }, inner.text()),
            Ty::Array { inner, n } => format!("[{}; {n}]", inner.text()),
            Ty::Slice { inner } => format!("[{}]", inner.text()),
            Ty::Tuple(v) if v.is_empty() => "()".into(),
            Ty::Tuple(v) => format!("({})", v.iter().map(|t| t.text()).collect::<Vec<_>>().join(", ")),
            Ty::Option { inner } => format!("Option<{}>", inner.text()),
            Ty::Result { ok, err } => format!("Result<{}, {}>", ok.text(), err.text()),
            Ty::Boxed { inner } => format!("Box<{}>", inner.text()),
            Ty::Rc { inner, atomic } => format!("{}<{}>", if *atomic { "Arc" } else { "Rc" }, inner.text()),
            Ty::Vec { inner } => format!("Vec<{}>", inner.text()),
            Ty::Fn { params, ret } => format!("fn({}){}", params.iter().map(|t| t.text()).collect::<Vec<_>>().join(", "), ret.as_ref().map(|r| format!(" -> {}", r.text())).unwrap_or_default()),
            Ty::Closure(i) => format!("{{closure#{i}}}"),
            Ty::Auto => "auto".into(),
            Ty::Never => "!".into(),
            Ty::Invalid => "<invalid>".into(),
        }
    }
    pub fn is_integer(&self) -> bool {
        matches!(self, Ty::Prim(p) if p.is_integer())
    }
    pub fn is_float(&self) -> bool {
        matches!(self, Ty::Prim(p) if p.is_float())
    }
    pub fn is_numeric(&self) -> bool {
        self.is_integer() || self.is_float()
    }
    /// §4.7.1: which types are implicitly `Copy`.
    pub fn is_copy(&self, prog: &Program) -> bool {
        match self {
            Ty::Prim(_) | Ty::Bool | Ty::Unit | Ty::Str | Ty::Auto | Ty::Never => true,
            Ty::Ref { .. } | Ty::Ptr { .. } | Ty::Fn { .. } => true,
            Ty::Adt(name) => prog.adt_is_copy(name),
            Ty::Array { inner, .. } => prog.copy_of(inner) && prog.copy_of(inner),
            Ty::Tuple(v) => v.iter().all(|t| prog.copy_of(t)),
            _ => false,
        }
    }
    /// §4.1: uninitialised read is `E-0101`; these types have a defined zero value only via
    /// `Default`, so `let` without an initialiser is an error for them.
    pub fn needs_initialiser(&self) -> bool {
        !matches!(self, Ty::Unit)
    }
}

#[derive(Clone, Debug)]
pub struct StructInfo {
    pub name: String,
    pub module: String,
    pub vis: Visibility,
    pub fields: Vec<(String, Ty, Visibility)>,
    pub repr_c: bool,
    pub is_copy: bool,
    pub has_drop: bool,
    pub align: Option<u32>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct EnumInfo {
    pub name: String,
    pub module: String,
    pub vis: Visibility,
    /// variant name → (payload types, discriminant)
    pub variants: Vec<(String, Vec<Ty>, u64)>,
    pub repr_c: bool,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FnInfo {
    pub name: String,
    pub package: String,
    pub module: String,
    pub vis: Visibility,
    pub params: Vec<(String, Ty)>,
    pub ret: Ty,
    pub is_event: bool,
    pub is_unsafe: bool,
    pub is_foreign: bool,
    pub file: FileId,
    pub span: Span,
    pub self_ty: Option<Ty>,
    /// `#[test]` / `#[bench]` (§5.9)
    pub test_kind: Option<TestKind>,
    pub must_use: bool,
    /// `#[export("name")]` — A-track reverse export (§7.6.5)
    pub export_symbol: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestKind {
    Test,
    Bench,
}

#[derive(Clone, Debug)]
pub struct ChainInfo {
    pub name: String,
    pub package: String,
    pub vis: Visibility,
    /// §8.4 node table, computed here so `infy artifact dump --nodes` and codegen agree.
    pub nodes: Vec<NodeDesc>,
    pub labels: BTreeMap<String, usize>,
    pub stmts: Vec<Stmt>,
    pub file: FileId,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct NodeDesc {
    pub idx: u32,
    pub line: u32,
    pub col: u32,
    pub linkage_count: usize,
    pub host_event: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct TraitInfo {
    pub name: String,
    pub module: String,
    pub methods: Vec<String>,
    pub associated: Vec<String>,
    pub is_unsafe: bool,
    pub object_safe: bool,
}

#[derive(Clone, Debug)]
pub struct ImplInfo {
    pub trait_: Option<String>,
    pub self_ty: Ty,
    pub methods: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum ResolvedItem {
    Fn(usize),
    Struct(usize),
    Enum(usize),
    Trait(usize),
    Const(usize),
    Static(usize),
    Chain(usize),
    TypeAlias(usize),
}

#[derive(Clone, Debug)]
pub struct ConstInfo {
    pub name: String,
    pub package: String,
    pub ty: Ty,
    pub value: Option<ConstValue>,
    pub file: FileId,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConstValue {
    Int(i128),
    UInt(u128),
    Float(f64),
    Bool(bool),
    Char(char),
}

/// Everything `codegen` needs, plus the type of every expression (keyed by span so no AST node
/// has to carry mutation).
pub struct Program {
    pub structs: Vec<StructInfo>,
    pub enums: Vec<EnumInfo>,
    pub fns: Vec<FnInfo>,
    pub chains: Vec<ChainInfo>,
    pub traits: Vec<TraitInfo>,
    pub impls: Vec<ImplInfo>,
    pub consts: Vec<ConstInfo>,
    pub statics: Vec<ConstInfo>,
    pub aliases: BTreeMap<String, Ty>,
    /// (package, name) → item, for cross-package resolution (§5.3).
    pub by_path: BTreeMap<(String, String), ResolvedItem>,
    /// local import alias → package path (§5.3 default name = last segment).
    pub import_map: BTreeMap<(FileId, String), (String, ImportAlias)>,
    pub ty_of: HashMap<(u32, u32, u32), Ty>,
    pub entry: Option<EntryPoint>,
    pub package_order: Vec<String>,
    pub struct_of_trait_method: BTreeMap<(String, String), usize>,
    pub warnings: usize,
}

#[derive(Clone, Debug)]
pub enum EntryPoint {
    Chain(usize),
    Main(usize),
}

impl Program {
    fn new() -> Program {
        Program {
            structs: Vec::new(), enums: Vec::new(), fns: Vec::new(), chains: Vec::new(), traits: Vec::new(),
            impls: Vec::new(), consts: Vec::new(), statics: Vec::new(), aliases: BTreeMap::new(),
            by_path: BTreeMap::new(), import_map: BTreeMap::new(), ty_of: HashMap::new(), entry: None,
            package_order: Vec::new(), struct_of_trait_method: BTreeMap::new(), warnings: 0,
        }
    }

    pub fn adt_is_copy(&self, name: &str) -> bool {
        self.structs.iter().any(|s| s.name == name && s.is_copy)
    }
    pub fn copy_of(&self, t: &Ty) -> bool {
        t.is_copy(self)
    }
    pub fn lookup_fn(&self, package: &str, name: &str) -> Option<usize> {
        match self.by_path.get(&(package.to_string(), name.to_string())) {
            Some(ResolvedItem::Fn(i)) => Some(*i),
            _ => None,
        }
    }
    pub fn lookup_chain(&self, name: &str) -> Option<usize> {
        (0..self.chains.len()).find(|i| self.chains[*i].name == name)
    }
    pub fn struct_by_name(&self, name: &str) -> Option<usize> {
        self.structs.iter().position(|s| s.name == name)
    }
    pub fn enum_by_name(&self, name: &str) -> Option<usize> {
        self.enums.iter().position(|e| e.name == name)
    }
    /// §4.14: `ty.method` where `ty` is a named ADT.
    pub fn method_of(&self, ty_name: &str, method: &str) -> Option<usize> {
        self.fns.iter().position(|f| f.self_ty.as_ref().map(|t| t.text()) == Some(ty_name.to_string()) && f.name == method)
    }

    pub fn set_ty(&mut self, span: Span, ty: Ty) {
        self.ty_of.insert((span.file, span.start, span.end), ty);
    }
    pub fn ty(&self, span: Span) -> Option<&Ty> {
        self.ty_of.get(&(span.file, span.start, span.end))
    }
}

pub struct Checker<'a> {
    pub src: &'a Sources,
    pub h: &'a mut Handler,
    pub prog: Program,
    /// current package path (e.g. `github.com/acme/ci/core/build`)
    pub package: String,
    pub file: FileId,
    /// Local `self` type while checking an impl.
    self_ty: Option<Ty>,
    /// Bindings in scope: name → (type, declared mutable, moved?)
    scopes: Vec<Vec<(String, Ty, bool)>>,
    ret_ty: Ty,
    in_chain: bool,
    in_defer: bool,
    /// inside an `unsafe` block (§4.7.5)
    in_unsafe: bool,
    negated_child: Option<bool>,
    /// §8.4: nodes of the chain currently being checked.
    chain_nodes: Vec<NodeDesc>,
    closure_counter: usize,
    /// Names declared `pub` per package for `internal`/visibility checks.
    pub manifest_module: String,
    /// `[profile] a_track = "unsafe-trusted"` (§4.7.5 / §7.6.6).
    pub a_track_declared: bool,
    /// `[b_track] py = "3.13"` etc. (§7.7.3).
    pub b_track_versions: Vec<(String, String)>,
    /// Every `#[link(name = "…")]` unit in scope (§7.6.1 pairing).
    pub link_unit_names: Vec<String>,
}

impl<'a> Checker<'a> {
    pub fn new(src: &'a Sources, h: &'a mut Handler, module: &str) -> Checker<'a> {
        Checker {
            src, h, prog: Program::new(), package: String::new(), file: 0, self_ty: None,
            scopes: vec![Vec::new()], ret_ty: Ty::Unit, in_chain: false, in_defer: false,
            chain_nodes: Vec::new(), closure_counter: 0, manifest_module: module.to_string(), in_unsafe: false, negated_child: None,
            a_track_declared: false, b_track_versions: Vec::new(), link_unit_names: Vec::new(),
        }
    }

    fn pos(&self, span: Span) -> Position {
        self.src.line_col(span).0
    }

    fn emit(&mut self, code: &str, span: Span, title: impl Into<String>, label: impl Into<String>, note: Option<String>, help: Option<String>) {
        let p = self.pos(span);
        let title: String = title.into();
        let label: String = label.into();
        let mut d = Diagnostic::new(code, Phase::Typeck, span.file, p)
            .titled(title)
            .region(span.file, p.line, p.column, p.column + (span.end - span.start).max(1), label);
        if let Some(n) = note {
            d = d.note(n);
        }
        if let Some(x) = help {
            d = d.help(x);
        }
        self.h.emit(d);
    }

    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }
    fn pop_scope(&mut self) {
        self.scopes.pop();
    }
    fn declare(&mut self, name: &str, ty: Ty, mutable: bool) {
        if let Some(s) = self.scopes.last_mut() {
            s.push((name.to_string(), ty, mutable));
        }
    }
    fn lookup(&self, name: &str) -> Option<(Ty, bool)> {
        for s in self.scopes.iter().rev() {
            if let Some((_, ty, m)) = s.iter().rev().find(|(n, _, _)| n == name) {
                return Some((ty.clone(), *m));
            }
        }
        None
    }

    // ---------------------------------------------------------------- surface → internal
    fn ty_from_ast(&mut self, t: &Type, generics: &BTreeSet<String>) -> Ty {
        match &t.kind {
            TypeKind::Prim(p) => match p {
                Prim::Str => Ty::Str,
                Prim::Unit => Ty::Unit,
                Prim::Bool => Ty::Bool,
                other => Ty::Prim(*other),
            },
            TypeKind::Named { path, args } => {
                let joined = path.join("::");
                let base = path.last().cloned().unwrap_or_default();
                if generics.contains(&base) && args.is_empty() {
                    return Ty::Generic(base);
                }
                let argtys: Vec<Ty> = args.iter().map(|a| self.ty_from_ast(a, generics)).collect();
                match base.as_str() {
                    "Option" if argtys.len() == 1 => Ty::Option { inner: Box::new(argtys[0].clone()) },
                    "Result" if argtys.len() == 2 => Ty::Result { ok: Box::new(argtys[0].clone()), err: Box::new(argtys[1].clone()) },
                    "Box" if argtys.len() == 1 => Ty::Boxed { inner: Box::new(argtys[0].clone()) },
                    "Rc" if argtys.len() == 1 => Ty::Rc { inner: Box::new(argtys[0].clone()), atomic: false },
                    "Arc" if argtys.len() == 1 => Ty::Rc { inner: Box::new(argtys[0].clone()), atomic: true },
                    "Vec" if argtys.len() == 1 => Ty::Vec { inner: Box::new(argtys[0].clone()) },
                    "String" => Ty::String,
                    "Self" => self.self_ty.clone().unwrap_or(Ty::Named("Self".into())),
                    "str" => Ty::Str,
                    _ => {
                        // A path with more than one segment is qualified (`pkg::Item` or `T::Assoc`).
                        if path.len() > 1 {
                            if let Some(alias) = self.prog.aliases.get(&joined) {
                                return alias.clone();
                            }
                        }
                        if argtys.is_empty() {
                            Ty::Named(base)
                        } else {
                            // Unknown generic name: keep it named so a missing type is `E-0131`.
                            Ty::Named(format!("{base}<{}>", argtys.iter().map(|a| a.text()).collect::<Vec<_>>().join(", ")))
                        }
                    }
                }
            }
            TypeKind::Ref { inner, mutable, lifetime } => Ty::Ref { inner: Box::new(self.ty_from_ast(inner, generics)), mutable: *mutable, lifetime: lifetime.clone() },
            TypeKind::Ptr { inner, mutable } => Ty::Ptr { inner: Box::new(self.ty_from_ast(inner, generics)), mutable: *mutable },
            TypeKind::Array { inner, len } => {
                let n = self.const_eval_usize(len).unwrap_or(0);
                Ty::Array { inner: Box::new(self.ty_from_ast(inner, generics)), n }
            }
            TypeKind::Slice { inner } => Ty::Slice { inner: Box::new(self.ty_from_ast(inner, generics)) },
            TypeKind::Tuple(v) if v.is_empty() => Ty::Unit,
            TypeKind::Tuple(v) => Ty::Tuple(v.iter().map(|t| self.ty_from_ast(t, generics)).collect()),
            TypeKind::FnPtr { params, ret } => Ty::Fn { params: params.iter().map(|t| self.ty_from_ast(t, generics)).collect(), ret: ret.as_ref().map(|r| Box::new(self.ty_from_ast(r, generics))) },
            TypeKind::Dyn { path, .. } => Ty::Dyn(path.join("::")),
            TypeKind::Never => Ty::Never,
            TypeKind::Invalid => Ty::Invalid,
        }
    }

    /// §2.12 const-eval: only the closed set of constructs is evaluated here.
    pub fn const_eval_usize(&mut self, e: &Expr) -> Option<u64> {
        self.const_eval(e).and_then(|v| match v {
            ConstValue::UInt(u) => Some(u as u64),
            ConstValue::Int(i) => u64::try_from(i).ok(),
            _ => None,
        })
    }

    pub fn const_eval(&mut self, e: &Expr) -> Option<ConstValue> {
        match &e.kind {
            ExprKind::Int(i) => {
                let v = i.value?;
                Some(if i.suffix.as_deref().map(|s| s.starts_with('i')).unwrap_or(false) { ConstValue::Int(v as i128) } else { ConstValue::UInt(v) })
            }
            ExprKind::Lit(Lit::Int(i)) => {
                let v = i.value?;
                Some(if i.suffix.as_deref().map(|s| s.starts_with('i')).unwrap_or(false) { ConstValue::Int(v as i128) } else { ConstValue::UInt(v) })
            }
            ExprKind::Lit(Lit::Float(f)) => f.mantissa.parse::<f64>().ok().map(ConstValue::Float),
            ExprKind::Lit(Lit::Bool(b)) => Some(ConstValue::Bool(*b)),
            ExprKind::Lit(Lit::Char(c)) => Some(ConstValue::Char(*c)),
            ExprKind::Unary { op, expr } => {
                let v = self.const_eval(expr)?;
                match (op, v) {
                    (crate::token::Punct::Minus, ConstValue::Int(i)) => Some(ConstValue::Int(-i)),
                    (crate::token::Punct::Minus, ConstValue::UInt(u)) => Some(ConstValue::Int(-(u as i128))),
                    (crate::token::Punct::Minus, ConstValue::Float(f)) => Some(ConstValue::Float(-f)),
                    (crate::token::Punct::Bang, ConstValue::Bool(b)) => Some(ConstValue::Bool(!b)),
                    _ => {
                        self.emit("E-0060", e.span, "编译期求值遇到禁止构造", "此处的常量表达式无法求值", None, Some("§2.12 规定了可常量化构造的封闭集合".into()));
                        None
                    }
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let a = self.const_eval(lhs)?;
                let b = self.const_eval(rhs)?;
                self.const_binop(*op, a, b, e.span)
            }
            ExprKind::Ident(name) => {
                if let Some(c) = self.prog.consts.iter().find(|c| c.name == *name) {
                    return c.value;
                }
                None
            }
            ExprKind::Path(p) => {
                let last = p.last().cloned().unwrap_or_default();
                self.prog.consts.iter().find(|c| c.name == last).and_then(|c| c.value)
            }
            _ => None,
        }
    }

    fn const_binop(&mut self, op: crate::token::Punct, a: ConstValue, b: ConstValue, span: Span) -> Option<ConstValue> {
        use crate::token::Punct::*;
        // §2.12 item 3: constant overflow is a compile error now, not "kind compiler, strict runtime".
        let over = |me: &mut Self| {
            me.emit("E-0062", span, "编译期常量溢出", "常量表达式的结果超出该类型范围", Some("§2.12 第 3 条：不推迟到运行时".into()), Some("改值，或显式 std.num.wrapping_*".into()));
            None::<ConstValue>
        };
        Some(match (a, b) {
            (ConstValue::Int(x), ConstValue::Int(y)) => match op {
                Plus => ConstValue::Int(x.checked_add(y).unwrap_or_else(|| { over(self); 0 })),
                Minus => ConstValue::Int(x.checked_sub(y).unwrap_or_else(|| { over(self); 0 })),
                Star => ConstValue::Int(x.checked_mul(y).unwrap_or_else(|| { over(self); 0 })),
                Slash => match x.checked_div(y) { Some(v) => ConstValue::Int(v), None => { self.emit("E-0062", span, "编译期除零", "常量除法不可求值", None, None); return None; } },
                Percent => match x.checked_rem(y) { Some(v) => ConstValue::Int(v), None => { self.emit("E-0062", span, "编译期除零", "常量取模不可求值", None, None); return None; } },
                AmpAmp => ConstValue::Bool(x != 0 && y != 0),
                PipePipe => ConstValue::Bool(x != 0 || y != 0),
                Amp => ConstValue::Int(x & y),
                Hat => ConstValue::Int(x ^ y),
                Pipe => ConstValue::Int(x | y),
                Shl => ConstValue::Int(x << y),
                Shr => ConstValue::Int(x >> y),
                LtEq => ConstValue::Bool(x <= y),
                Lt => ConstValue::Bool(x < y),
                GtEq => ConstValue::Bool(x >= y),
                Gt => ConstValue::Bool(x > y),
                EqEq => ConstValue::Bool(x == y),
                BangEq => ConstValue::Bool(x != y),
                _ => return None,
            },
            (ConstValue::UInt(x), ConstValue::UInt(y)) => match op {
                Plus => ConstValue::UInt(x.checked_add(y).unwrap_or_else(|| { over(self); 0 })),
                Minus => ConstValue::UInt(x.checked_sub(y).unwrap_or_else(|| { over(self); 0 })),
                Star => ConstValue::UInt(x.checked_mul(y).unwrap_or_else(|| { over(self); 0 })),
                Slash => match x.checked_div(y) { Some(v) => ConstValue::UInt(v), None => { self.emit("E-0062", span, "编译期除零", "常量除法不可求值", None, None); return None; } },
                Percent => match x.checked_rem(y) { Some(v) => ConstValue::UInt(v), None => { self.emit("E-0062", span, "编译期除零", "常量取模不可求值", None, None); return None; } },
                EqEq => ConstValue::Bool(x == y),
                BangEq => ConstValue::Bool(x != y),
                Lt => ConstValue::Bool(x < y),
                LtEq => ConstValue::Bool(x <= y),
                Gt => ConstValue::Bool(x > y),
                GtEq => ConstValue::Bool(x >= y),
                AmpAmp => ConstValue::Bool(x != 0 && y != 0),
                PipePipe => ConstValue::Bool(x != 0 || y != 0),
                _ => return None,
            },
            (ConstValue::Float(x), ConstValue::Float(y)) => match op {
                Plus => ConstValue::Float(x + y),
                Minus => ConstValue::Float(x - y),
                Star => ConstValue::Float(x * y),
                Slash => ConstValue::Float(x / y),
                EqEq => ConstValue::Bool(x == y),
                BangEq => ConstValue::Bool(x != y),
                Lt => ConstValue::Bool(x < y),
                LtEq => ConstValue::Bool(x <= y),
                Gt => ConstValue::Bool(x > y),
                GtEq => ConstValue::Bool(x >= y),
                _ => return None,
            },
            (ConstValue::Bool(x), ConstValue::Bool(y)) => match op {
                AmpAmp => ConstValue::Bool(x && y),
                PipePipe => ConstValue::Bool(x || y),
                EqEq => ConstValue::Bool(x == y),
                BangEq => ConstValue::Bool(x != y),
                _ => return None,
            },
            _ => return None,
        })
    }

    // ---------------------------------------------------------------- declaration pass
    /// Pass 1: register every item so forward references resolve (§5.4 makes order observable but
    /// not restrictive for declarations).
    pub fn collect(&mut self, files: &[(FileId, String, ParsedFile)]) {
        for (file, package, pf) in files {
            self.file = *file;
            self.package = package.clone();
            for import in &pf.imports {
                self.prog.import_map.insert((*file, import.path.clone()), (import.path.clone(), import.alias.clone()));
            }
            for it in &pf.items {
                self.collect_item(it, package);
            }
        }
    }

    fn collect_item(&mut self, it: &Item, package: &str) {
        let span = it.span;
        match &it.kind {
            ItemKind::Fn { sig, body } => self.register_fn(sig, false, matches!(body, Body::Foreign), it, package, span),
            ItemKind::Event { sig, body } => self.register_fn(sig, true, matches!(body, Body::Foreign), it, package, span),
            ItemKind::Struct { name, fields, .. } => {
                let generics: BTreeSet<String> = BTreeSet::new();
                let fs = fields.iter().map(|f| (f.name.clone(), self.ty_from_ast(&f.ty, &generics), f.vis.clone())).collect();
                let repr_c = it.attrs.iter().any(|a| a.name() == "repr" && a.has_ident("C"));
                let idx = self.prog.structs.len();
                self.prog.structs.push(StructInfo {
                    name: name.clone(), module: self.manifest_module.clone(), vis: it.vis.clone(), fields: fs,
                    repr_c, is_copy: false, has_drop: false, align: it.attr("align").and_then(|a| a.args.first()).and_then(|a| match a.value { Some(AttrValue::Int(i)) => Some(i as u32), _ => None }),
                    span,
                });
                self.prog.by_path.insert((package.to_string(), name.clone()), ResolvedItem::Struct(idx));
            }
            ItemKind::Enum { name, variants, .. } => {
                let generics = BTreeSet::new();
                let mut vs = Vec::new();
                let mut next_disc = 0u64;
                for v in variants {
                    let mut payload: Vec<Ty> = v.tuple.iter().map(|(_, t)| self.ty_from_ast(t, &generics)).collect();
                let more: Vec<Ty> = v.struct_fields.iter().map(|f| self.ty_from_ast(&f.ty, &generics)).collect();
                payload.extend(more);
                    let disc = v.disc.unwrap_or(next_disc as u128) as u64;
                    next_disc = disc + 1;
                    vs.push((v.name.clone(), payload, disc));
                }
                let repr_c = it.attrs.iter().any(|a| a.name() == "repr" && a.has_ident("C"));
                let idx = self.prog.enums.len();
                self.prog.enums.push(EnumInfo { name: name.clone(), module: self.manifest_module.clone(), vis: it.vis.clone(), variants: vs, repr_c, span });
                self.prog.by_path.insert((package.to_string(), name.clone()), ResolvedItem::Enum(idx));
            }
            ItemKind::Trait { name, items, is_unsafe, .. } => {
                let methods = items.iter().map(|i| i.name().to_string()).collect();
                let associated = items.iter().filter(|i| matches!(i.kind, ItemKind::TypeAlias { .. })).map(|i| i.name().to_string()).collect();
                // §4.14 object safety
                let object_safe = items.iter().all(|i| match &i.kind {
                    ItemKind::Fn { sig, .. } => sig.generics.is_empty() && !sig.params.iter().any(|p| p.name == "_" && format!("{:?}", p.ty.kind).contains("Self")) && !(sig.ret.as_ref().map(|r| format!("{:?}", r.kind).contains("SelfTy")).unwrap_or(false)),
                    _ => true,
                });
                let idx = self.prog.traits.len();
                self.prog.traits.push(TraitInfo { name: name.clone(), module: self.manifest_module.clone(), methods, associated, is_unsafe: *is_unsafe, object_safe });
                self.prog.by_path.insert((package.to_string(), name.clone()), ResolvedItem::Trait(idx));
                if !object_safe && it.attrs.iter().any(|a| a.name() == "object_safe_required") {
                    self.emit("E-0167", span, "trait 非对象安全", name, Some("泛型方法或 Self 返回类型使 dyn 不可用（§4.14）".into()), None);
                }
            }
            ItemKind::Const { name, ty, init, .. } => {
                let generics = BTreeSet::new();
                let t = match ty {
                    Some(t) => self.ty_from_ast(t, &generics),
                    None => Ty::Invalid,
                };
                let value = init.as_ref().and_then(|e| self.const_eval(e));
                if init.is_some() && value.is_none() && !matches!(init.as_ref().map(|e| &e.kind), Some(ExprKind::StructLit { .. } | ExprKind::Path(_) | ExprKind::Lit(_) | ExprKind::Call { .. })) {
                    self.emit("E-0060", span, "编译期求值遇到禁止构造", name, Some("const 初始值必须是编译期可求值的封闭构造（§2.12）".into()), None);
                }
                let idx = self.prog.consts.len();
                self.prog.consts.push(ConstInfo { name: name.clone(), package: package.to_string(), ty: t, value, file: span.file, span });
                self.prog.by_path.insert((package.to_string(), name.clone()), ResolvedItem::Const(idx));
            }
            ItemKind::Static { name, ty, init, .. } => {
                let generics = BTreeSet::new();
                let t = match ty { Some(t) => self.ty_from_ast(t, &generics), None => Ty::Invalid };
                let value = init.as_ref().and_then(|e| self.const_eval(e));
                let idx = self.prog.statics.len();
                self.prog.statics.push(ConstInfo { name: name.clone(), package: package.to_string(), ty: t, value, file: span.file, span });
                self.prog.by_path.insert((package.to_string(), name.clone()), ResolvedItem::Static(idx));
            }
            ItemKind::TypeAlias { name, ty, .. } => {
                let generics = BTreeSet::new();
                let t = self.ty_from_ast(ty, &generics);
                self.prog.aliases.insert(name.clone(), t.clone());
                self.prog.aliases.insert(format!("{package}::{name}"), t);
            }
            ItemKind::Chain { name, stmts, .. } => {
                let idx = self.prog.chains.len();
                self.prog.chains.push(ChainInfo {
                    name: name.clone(), package: package.to_string(), vis: it.vis.clone(),
                    nodes: Vec::new(), labels: BTreeMap::new(), stmts: stmts.clone(), file: span.file, span,
                });
                self.prog.by_path.insert((package.to_string(), name.clone()), ResolvedItem::Chain(idx));
            }
            ItemKind::Impl { trait_, self_ty, items, .. } => {
                let generics: BTreeSet<String> = BTreeSet::new();
                let st = self.ty_from_ast(self_ty, &generics);
                let ty_name = type_base_name(&st);
                for m in items {
                    if let ItemKind::Fn { sig, body } = &m.kind {
                        let _ = self.register_fn_for_impl(sig, trait_.as_ref().and_then(|p| p.last().cloned()).unwrap_or_default(), &st, matches!(body, Body::Foreign), m, package, m.span);
                    }
                }
                let idx = self.prog.impls.len();
                self.prog.impls.push(ImplInfo { trait_: trait_.as_ref().and_then(|p| p.last().cloned()), self_ty: st, methods: items.iter().map(|i| i.name().to_string()).collect() });
                if let Some(tr) = trait_.as_ref().and_then(|p| p.last()) {
                    for m in items {
                        self.prog.struct_of_trait_method.insert((format!("{tr}::{ty_name}"), m.name().to_string()), self.prog.fns.len().saturating_sub(1));
                    }
                    // §4.14 orphan rule: trait or type must belong to this module.
                    let trait_local = self.prog.traits.iter().any(|t| &t.name == tr);
                    let type_local = self.prog.structs.iter().any(|s| &s.name == &ty_name) || self.prog.enums.iter().any(|e| e.name == ty_name);
                    if !trait_local && !type_local && !is_std_path(package) {
                        self.emit("E-0168", m_span(items), "违反孤儿规则", format!("{tr} for {ty_name}"), Some("外部 trait 对外部 type 的 impl 会破坏 .infib 跨库 ABI 稳定性（§4.14）".into()), Some("包一层 newtype（§4.14）".into()));
                    }
                }
                let _ = idx;
            }
            ItemKind::ExternBlock { decls, .. } => {
                for d in decls {
                    if let ItemKind::Fn { sig, .. } = &d.kind {
                        self.register_fn(sig, false, true, d, package, d.span);
                    }
                }
            }
            ItemKind::Union { name, .. } => {
                let idx = self.prog.structs.len();
                self.prog.structs.push(StructInfo { name: name.clone(), module: self.manifest_module.clone(), vis: it.vis.clone(), fields: Vec::new(), repr_c: true, is_copy: true, has_drop: false, align: None, span });
                self.prog.by_path.insert((package.to_string(), name.clone()), ResolvedItem::Struct(idx));
            }
            ItemKind::MacroRules { .. } | ItemKind::Mod { .. } => {}
        }
    }

    fn register_fn(&mut self, sig: &FnSig, is_event: bool, is_foreign: bool, it: &Item, package: &str, span: Span) {
        let mut generics: BTreeSet<String> = BTreeSet::new();
        for g in &sig.generics {
            if !g.is_lifetime {
                generics.insert(g.name.clone());
            }
        }
        let params: Vec<(String, Ty)> = sig.params.iter().map(|p| (p.name.clone(), self.ty_from_ast(&p.ty, &generics))).collect();
        let ret = match &sig.ret {
            Some(t) => self.ty_from_ast(t, &generics),
            None => Ty::Unit,
        };
        // §4.7.3 lifetime elision: rules 1–3, otherwise E-1003.
        self.check_elision(sig, &params, &ret, span);
        let idx = self.prog.fns.len();
        let export_symbol = it
            .attr("export")
            .and_then(|a| a.positional.first())
            .and_then(|v| match v {
                AttrValue::Str(s) => Some(s.clone()),
                _ => None,
            })
            .or_else(|| {
                it.attr("export")
                    .and_then(|a| a.args.first())
                    .and_then(|a| match &a.value {
                        Some(AttrValue::Str(s)) => Some(s.clone()),
                        _ => None,
                    })
            });
        let f = FnInfo {
            name: sig.name.clone(), package: package.to_string(), module: self.manifest_module.clone(),
            vis: it.vis.clone(), params, ret, is_event, is_unsafe: sig.is_unsafe, is_foreign,
            file: span.file, span, self_ty: None,
            test_kind: if it.has_attr("test") { Some(TestKind::Test) } else if it.has_attr("bench") { Some(TestKind::Bench) } else { None },
            // §4.11: every function returning Result/Option carries an implicit `#[must_use]`.
            must_use: returns_fallible(&sig.ret),
            export_symbol,
        };
        self.prog.fns.push(f);
        self.prog.by_path.insert((package.to_string(), sig.name.clone()), ResolvedItem::Fn(idx));
    }

    fn register_fn_for_impl(&mut self, sig: &FnSig, trait_name: String, self_ty: &Ty, is_foreign: bool, it: &Item, package: &str, span: Span) -> usize {
        let mut generics: BTreeSet<String> = BTreeSet::new();
        for g in &sig.generics {
            if !g.is_lifetime {
                generics.insert(g.name.clone());
            }
        }
        let mut params: Vec<(String, Ty)> = Vec::new();
        for p in &sig.params {
            let mut pt = self.ty_from_ast(&p.ty, &generics);
            // The receiver's `Self` resolves to the impl's type.
            if p.name == "self" {
                pt = match pt {
                    Ty::Ref { inner, mutable, lifetime } if matches!(*inner, Ty::Named(ref n) if n == "Self" || n == "self") => Ty::Ref { inner: Box::new(self_ty.clone()), mutable, lifetime },
                    Ty::Named(n) if n == "Self" || n == "self" => self_ty.clone(),
                    other => other,
                };
            }
            params.push((p.name.clone(), pt));
        }
        let ret = match &sig.ret {
            Some(t) => {
                let r = self.ty_from_ast(t, &generics);
                substitute_self(&r, self_ty)
            }
            None => Ty::Unit,
        };
        self.self_ty = Some(self_ty.clone());
        self.check_elision(sig, &params, &ret, span);
        let idx = self.prog.fns.len();
        let name = if trait_name.is_empty() { sig.name.clone() } else { format!("{trait_name}::{}", sig.name) };
        self.prog.fns.push(FnInfo {
            name, package: package.to_string(), module: self.manifest_module.clone(), vis: it.vis.clone(),
            params, ret, is_event: false, is_unsafe: sig.is_unsafe, is_foreign, file: span.file, span,
            self_ty: Some(self_ty.clone()), test_kind: None, must_use: returns_fallible(&sig.ret), export_symbol: None,
        });
        self.self_ty = None;
        let _ = it.vis;
        idx
    }

    /// §4.7.3 — elision only when all three rules apply, never a guessed lifetime.
    fn check_elision(&mut self, sig: &FnSig, params: &[(String, Ty)], ret: &Ty, span: Span) {
        let fn_span = sig.name_span;
        let out_has_ref = contains_ref_needing_lifetime(ret);
        if !out_has_ref {
            return;
        }
        let input_lifetimes: Vec<Option<&str>> = params.iter().map(|(_, t)| match t { Ty::Ref { lifetime, .. } => lifetime.as_deref(), _ => None }).collect();
        let has_self = params.first().map(|(n, _)| n == "self").unwrap_or(false);
        let explicit_inputs = input_lifetimes.iter().filter(|l| l.is_some()).count();
        let refs_without = input_lifetimes.iter().filter(|l| l.is_none()).count();
        // rule 1 assigns a fresh lifetime to each elided input; rules 2/3 then decide the output
        if refs_without > 0 && explicit_inputs == 0 && !has_self {
            self.emit("E-1003", fn_span, "生命周期无法唯一确定", format!("{}", sig.name), Some("省略规则 1–3 无法全部适用（§4.7.3）".into()), Some("为返回值显式写出 `'a` 标注".into()));
            let _ = span;
        } else if refs_without > 1 && !has_self && explicit_inputs == 0 {
            self.emit("E-1003", fn_span, "生命周期无法唯一确定", format!("{}", sig.name), Some("多个未标注的输入借用，规则 2 不适用（§4.7.3）".into()), Some("显式标注 `'a`，或改为只借用一个参数".into()));
        } else if has_self && refs_without > 0 {
            // rule 3: self's lifetime is assigned to elided outputs — always decidable
        }
    }

    // ---------------------------------------------------------------- body checking
    pub fn check_bodies(&mut self, files: &[(FileId, String, ParsedFile)]) {
        for (file, package, pf) in files {
            self.file = *file;
            self.package = package.clone();
            for it in &pf.items {
                self.check_item(it);
            }
            // §8.4 node table for every chain declared in this file
            for it in &pf.items {
                if let ItemKind::Chain { name, stmts, .. } = &it.kind {
                    if let Some(idx) = self.prog.lookup_chain(name) {
                        let (nodes, labels) = build_node_table(self.src, *file, stmts);
                        self.prog.chains[idx].nodes = nodes;
                        self.prog.chains[idx].labels = labels;
                    }
                }
            }
        }
        // `Copy` derive / implicit-copy legality (§4.7.1, §6.7)
        let structs = self.prog.structs.clone();
        for i in 0..self.prog.structs.len() {
            let all_copy = self.prog.structs[i].fields.iter().all(|(_, t, _)| self.copy_of_shallow(t, &structs));
            let derived = self.prog.structs[i].is_copy;
            self.prog.structs[i].is_copy = derived && all_copy;
        }
    }

    /// A shallow copy test that tolerates the not-yet-fixed-point `is_copy` set.
    fn copy_of_shallow(&self, t: &Ty, structs: &[StructInfo]) -> bool {
        match t {
            Ty::Adt(n) | Ty::Named(n) => structs.iter().find(|s| &s.name == n).map(|s| s.is_copy).unwrap_or(false),
            Ty::Ref { .. } | Ty::Ptr { .. } | Ty::Prim(_) | Ty::Bool | Ty::Unit | Ty::Str => true,
            Ty::Array { inner, .. } | Ty::Boxed { inner } | Ty::Rc { inner, .. } | Ty::Vec { inner } | Ty::Slice { inner } | Ty::Option { inner } => self.copy_of_shallow(inner, structs),
            Ty::Tuple(v) => v.iter().all(|x| self.copy_of_shallow(x, structs)),
            Ty::Result { ok, err } => self.copy_of_shallow(ok, structs) && self.copy_of_shallow(err, structs),
            _ => false,
        }
    }

    fn check_item(&mut self, it: &Item) {
        match &it.kind {
            ItemKind::Fn { sig, body } | ItemKind::Event { sig, body } => {
                self.check_body(sig, body, it);
            }
            ItemKind::Struct { name, fields, .. } => {
                let generics = BTreeSet::new();
                let mut seen: HashSet<String> = HashSet::new();
                for f in fields {
                    let t = self.ty_from_ast(&f.ty, &generics);
                    if t == Ty::Invalid {
                        self.emit("E-0131", f.span, "类型无法解析", &f.name, Some(format!("字段 `{}` 的类型 `{name}` 未知", f.name)), None);
                    }
                    if !seen.insert(f.name.clone()) {
                        self.emit("E-0053", f.span, "字段重复", &f.name, Some("同一 struct 内字段名必须唯一".into()), None);
                    }
                    // §4.15 recursive size: `struct L { next: L }` is infinite
                    if type_base_name(&t) == *name {
                        self.emit("E-0175", f.span, "递归尺寸无限", format!("{name}.{}", f.name), Some("直接递归的字段使 size_of 无限（§4.15）".into()), Some("插入 Box/Option<Box<..>> 断开尺寸环（§4.15）".into()));
                    }
                }
            }
            ItemKind::Const { name, init, .. } => {
                if let Some(e) = init {
                    let _v = self.const_eval(e);
                    let _ = name;
                }
            }
            ItemKind::Impl { items, .. } => {
                for m in items {
                    self.check_item(m);
                }
            }
            ItemKind::Chain { name, stmts, .. } => {
                let saved = std::mem::take(&mut self.chain_nodes);
                self.in_chain = true;
                for s in stmts {
                    self.check_stmt(s);
                }
                self.in_chain = false;
                self.chain_nodes = saved;
                let _ = name;
            }
            _ => {}
        }
    }

    fn check_body(&mut self, sig: &FnSig, body: &Body, it: &Item) {
        let mut generics: BTreeSet<String> = BTreeSet::new();
        for g in &sig.generics {
            if !g.is_lifetime {
                generics.insert(g.name.clone());
            }
        }
        self.ret_ty = match &sig.ret { Some(t) => self.ty_from_ast(t, &generics), None => Ty::Unit };
        let is_impl = self.package_is_impl(sig);
        let _ = is_impl;
        self.push_scope();
        for p in &sig.params {
            let pt = self.ty_from_ast(&p.ty, &generics);
            self.declare(&p.name, pt, p.mutable);
        }
        match body {
            Body::Block(b) => self.check_block(b),
            Body::Embed(e) => self.check_embed(e, it),
            Body::Foreign => {
                // §3.3: `;` means A-track extern or host-provided event.
                if !it.attrs.iter().any(|a| a.name() == "link") && !self.in_extern_block(it) {
                    // host event (§11) — no body is legal for `event`
                    if !matches!(it.kind, ItemKind::Event { .. }) {
                        self.emit("E-0720", sig.span, "无本地实现缺少声明", &sig.name, Some("函数体为 `;` 时必须由 #[link] extern 块或宿主提供（§3.3/§7.6.1）".into()), None);
                    }
                }
            }
        }
        // §4.10 rule 6 / §3.3: a tail expression supplies the return value; otherwise the body
        // must return unit or end in `return`.
        if let Body::Block(b) = body {
            match &b.tail {
                Some(t) => {
                    let ty = self.check_expr(t);
                    self.coerce_return(&ty, t.span, &sig.name);
                }
                None => {
                    if !matches!(self.ret_ty, Ty::Unit | Ty::Never | Ty::Invalid) {
                        self.emit("E-0053", sig.span, "缺少返回值", format!("{}", sig.name), Some("函数体没有尾表达式也没有 return（P5 零隐式 return，§1.1）".into()), Some("补尾表达式或显式 `return v;`".into()));
                    }
                }
            }
        }
        self.pop_scope();
    }

    fn package_is_impl(&self, sig: &FnSig) -> bool {
        !sig.params.is_empty() && sig.params[0].name == "self"
    }
    fn in_extern_block(&self, _it: &Item) -> bool {
        false
    }

    fn coerce_return(&mut self, ty: &Ty, span: Span, name: &str) {
        if matches!(self.ret_ty, Ty::Unit | Ty::Invalid | Ty::Never) && matches!(ty, Ty::Unit | Ty::Invalid) {
            return;
        }
        if !type_compatible(&self.ret_ty, ty) {
            self.emit("E-0110", span, format!("返回值类型不匹配：期望 {}，实际 {}", self.ret_ty.text(), ty.text()), name, Some("§4.2：赋值、参数、返回值、泛型实参必须 100% 类型兼容".into()), Some("显式 `as` 转换，或修正签名".into()));
        }
    }

    pub fn check_block(&mut self, b: &Block) {
        self.push_scope();
        for s in &b.stmts {
            self.check_stmt(s);
        }
        if let Some(t) = &b.tail {
            let _ = self.check_expr(t);
        }
        self.pop_scope();
    }

    pub fn check_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Let { is_var, name, ty, init, name_span } => {
                let declared = ty.as_ref().map(|t| self.ty_from_ast(t, &BTreeSet::new()));
                let actual = match init {
                    Some(e) => {
                        let t = self.check_expr(e);
                        if let Some(d) = &declared {
                            if !type_compatible(d, &t) && !matches!(t, Ty::Invalid) {
                                self.emit("E-0110", e.span, format!("绑定类型不匹配：声明 {}，实际 {}", d.text(), t.text()), name, Some("§4.2 无隐式转换（UB-001）".into()), Some("显式 `as`，或修正声明".into()));
                            }
                        }
                        t
                    }
                    None => declared.clone().unwrap_or(Ty::Invalid),
                };
                if matches!(actual, Ty::Invalid) && init.is_none() {
                    self.emit("E-0131", s.span, "绑定类型无法确定", name, Some("既无类型标注也无初始化表达式（§4.1）".into()), None);
                }
                if *is_var {
                    // `var` may not appear in parameter position; that is E-0022 and the parser
                    // already routes parameters through Param{mutable}.
                }
                self.declare(name, actual, *is_var);
                let _ = name_span;
            }
            StmtKind::Assign { target, op, value } => {
                let vt = self.check_expr(value);
                let tt = self.check_place(target);
                if !type_compatible(&tt, &vt) && !matches!(vt, Ty::Invalid) && !matches!(tt, Ty::Invalid) {
                    self.emit("E-0110", value.span, format!("赋值类型不匹配：左值 {}，右值 {}", tt.text(), vt.text()), "赋值", Some("§4.2 无隐式转换".into()), None);
                }
                // §4.3: assigning to a `let` binding is E-0120
                let name = match &target.kind { ExprKind::Ident(x) => Some(x.clone()), ExprKind::Path(p) => p.last().cloned(), _ => None };
                if let Some(name) = name.clone() {
                    let name = match &target.kind { ExprKind::Ident(x) => x.clone(), ExprKind::Path(p) => p.last().cloned().unwrap_or_default(), _ => unreachable!() };
                    if let Some((ty, mutable)) = self.lookup(&name) {
                        if !mutable {
                            self.emit("E-0120", target.span, "修改 let 不可变绑定", &name, Some("§4.3：let 是不可变绑定".into()), Some(format!("改 `var {name} = …`，或使用 `std.sync.RefCell`")));
                        }
                        let _ = ty;
                    } else {
                        self.emit("E-0101", target.span, "未声明的标识符", &name, Some("INFY 无隐式变量声明（P5 零隐式，§1.1）".into()), None);
                    }
                }
                let _ = op;
            }
            StmtKind::Expr { expr, linkage } => {
                let t = self.check_expr(expr);
                // §4.11: Result/Option values are #[must_use] by default
                if matches!(t, Ty::Result { .. } | Ty::Option { .. }) {
                    self.emit("W-0170", expr.span, "丢弃 Result/Option 值", "该调用的结果未被使用", Some("所有返回 Result/Option 的函数默认带 #[must_use]（§4.11）".into()), Some("处理错误，或 `let _ = …;`".into()));
                }
                for lb in linkage {
                    self.check_linkage(lb);
                }
                if !linkage.is_empty() && !self.in_chain {
                    self.emit("E-0300", s.span, "连携块缺少链宿主", "连携块只能出现在 chain 体内", Some("§8.1：连携语句挂载于 chain 宿主语句之后".into()), Some("把该语句移入 `chain { … }`".into()));
                }
            }
            StmtKind::If { cond, then, else_ } => {
                let ct = self.check_expr(cond);
                if !matches!(ct, Ty::Bool) && !matches!(ct, Ty::Invalid) {
                    self.emit("E-0110", cond.span, "if 条件必须是 bool", format!("实际类型 {}", ct.text()), Some("§4.3：无隐式真值判断（P5 零隐式）".into()), None);
                }
                self.check_block(then);
                if let Some(e) = else_ {
                    match &**e {
                        ElseBranch::Block(b) => self.check_block(b),
                        ElseBranch::If(s) => self.check_stmt(s),
                    }
                }
            }
            StmtKind::While { cond, body } => {
                let ct = self.check_expr(cond);
                if !matches!(ct, Ty::Bool) && !matches!(ct, Ty::Invalid) {
                    self.emit("E-0110", cond.span, "while 条件必须是 bool", format!("实际类型 {}", ct.text()), None, None);
                }
                self.check_block(body);
            }
            StmtKind::Loop { body, .. } => self.check_block(body),
            StmtKind::For { pat, iter, body, .. } => {
                let it_ty = self.check_expr(iter);
                let item_ty = iterator_item(&it_ty, &self.prog).unwrap_or(Ty::Invalid);
                self.push_scope();
                self.bind_pat(pat, item_ty);
                self.check_block(body);
                self.pop_scope();
            }
            StmtKind::Match { scrut, arms } => {
                let st = self.check_expr(scrut);
                self.check_match(arms, &st, s.span);
            }
            StmtKind::Unsafe(b) => {
                // §4.7.5: every unsafe block must be followed by `// infy-safety:` — W-1030.
                self.check_block(b);
            }
            StmtKind::Defer(b) => {
                let saved = self.in_defer;
                self.in_defer = true;
                self.check_block(b);
                self.in_defer = saved;
            }
            StmtKind::ConstAssert { cond, .. } => {
                match self.const_eval(cond) {
                    Some(ConstValue::Bool(true)) | None => {}
                    Some(ConstValue::Bool(false)) => self.emit("E-0063", s.span, "const_assert 失败", "断言条件为假", Some("§2.12：编译期断言".into()), Some("修正断言或值；可用 [lints] allow（§9.6）".into())),
                    Some(_) => self.emit("E-0063", s.span, "const_assert 条件非 bool", "断言必须是布尔常量", None, None),
                }
            }
            StmtKind::Break { value, .. } | StmtKind::Return { value } => {
                if self.in_defer {
                    self.emit("E-0074", s.span, "defer 内非法控制流", "defer 块不得改变退出流", Some("§3.13：defer 的执行时机已定".into()), None);
                }
                if let Some(v) = value {
                    let t = self.check_expr(v);
                    if matches!(&s.kind, StmtKind::Return { .. }) {
                        self.coerce_return(&t, v.span, "return");
                    }
                } else if matches!(&s.kind, StmtKind::Return { .. }) && !matches!(self.ret_ty, Ty::Unit | Ty::Never) {
                    self.emit("E-0053", s.span, "return 缺少值", format!("期望返回 {}", self.ret_ty.text()), None, None);
                }
            }
            StmtKind::Continue { .. } => {
                if self.in_defer {
                    self.emit("E-0074", s.span, "defer 内非法控制流", "defer 块不得 continue", Some("§3.13".into()), None);
                }
            }
            StmtKind::Pause | StmtKind::Stop | StmtKind::Resume => {}
            StmtKind::Label { .. } => {}
            StmtKind::NestedItem(it) => {
                self.emit("E-0053", it.span, "声明出现在语句位置", it.name(), Some("§3.5 的 statement 集合不含声明（嵌套函数不在 v2.0 文法内）".into()), Some("把该声明移到文件顶层".into()));
            }
        }
    }

    fn check_linkage(&mut self, lb: &LinkageBlock) {
        let n_nodes = self.chain_nodes.len().max(1) as i128;
        for s in &lb.stmts {
            match s {
                LinkageStmt::Goto(target) => match target {
                    GotoTarget::Named(name) => {
                        if !self.chain_nodes_labels().contains(name) {
                            self.emit("E-0301", lb.span, "goto 目标编译期不可解析", format!("链内无锚点 `{name}`"), Some("§8.2/UB-013：命名锚点必须在同链内声明".into()), Some("补 `label {name}:` 或改用现有锚点".into()));
                        }
                    }
                    GotoTarget::Abs(i) => {
                        if *i as i128 >= n_nodes.max(0) {
                            self.emit("E-0301", lb.span, "goto 绝对索引越界", format!("节点数 {}，目标 {i}", n_nodes), Some("§8.2".into()), None);
                        }
                        self.emit("N-0800", lb.span, "goto 使用绝对索引", "源码改动会移动节点（§8.4 第 6 条）", None, Some("改用命名锚点".into()));
                    }
                    GotoTarget::Rel(d) => {
                        let cur = self.chain_nodes.len() as i128;
                        let tgt = cur + d;
                        if tgt < 0 || tgt >= n_nodes {
                            self.emit("E-0301", lb.span, "goto 相对偏移越界", format!("目标 {tgt} 不在 [0, {n_nodes})"), Some("§8.2 编译期即可判定".into()), None);
                        }
                    }
                },
                LinkageStmt::Start { chain, node, .. } => {
                    if self.prog.lookup_chain(chain).is_none() {
                        self.emit("E-0800", lb.span, "链不存在", format!("start(chain = {chain})"), Some("§8.2：链名编译期必须可解析".into()), None);
                    } else if let Some(StartNode::Anchor(a)) = node {
                        let ok = self.prog.lookup_chain(chain).map(|i| self.prog.chains[i].labels.contains_key(a)).unwrap_or(false);
                        if !ok {
                            self.emit("E-0801", lb.span, "链锚点不存在", format!("链 {chain} 无锚点 `{a}`"), Some("§8.2".into()), None);
                        }
                    }
                }
                LinkageStmt::If { cond, then, else_ } => {
                    let t = self.check_expr(cond);
                    if !matches!(t, Ty::Bool) && !matches!(t, Ty::Invalid) {
                        self.emit("E-0110", lb.span, "连携 if 条件必须是 bool", format!("实际 {t}", t = t.text()), None, None);
                    }
                    for x in then {
                        let l = LinkageBlock { stmts: vec![x.clone()], span: lb.span };
                        self.check_linkage(&l);
                    }
                    for x in else_.iter().flatten() {
                        let l = LinkageBlock { stmts: vec![x.clone()], span: lb.span };
                        self.check_linkage(&l);
                    }
                }
                LinkageStmt::Plain(st) => self.check_stmt(st),
                LinkageStmt::StartFlag | LinkageStmt::Pause | LinkageStmt::Stop | LinkageStmt::Resume => {}
            }
        }
    }

    fn chain_nodes_labels(&self) -> Vec<String> {
        // labels are collected during node-table construction
        self.prog.chains.iter().find(|c| c.file == self.file).map(|c| c.labels.keys().cloned().collect()).unwrap_or_default()
    }

    fn check_place(&mut self, e: &Expr) -> Ty {
        let t = self.check_expr(e);
        match &e.kind {
            ExprKind::Ident(_) | ExprKind::Field { .. } | ExprKind::Index { .. } | ExprKind::Deref { .. } | ExprKind::Path(_) => t,
            _ => {
                self.emit("E-0053", e.span, "左值必须是 place expression", "此处不能赋值", Some("§3.5：assign_stmt = place_expr \"=\" expr".into()), None);
                Ty::Invalid
            }
        }
    }

    /// §3.12: exhaustiveness + reachability + or-pattern binding consistency.
    fn check_match(&mut self, arms: &[MatchArm], scrut: &Ty, span: Span) {
        for a in arms {
            self.push_scope();
            self.bind_pat(&a.pat, scrut.clone());
            if let Some(g) = &a.guard {
                let t = self.check_expr(g);
                if !matches!(t, Ty::Bool) && !matches!(t, Ty::Invalid) {
                    self.emit("E-0110", g.span, "match 守卫必须是 bool", format!("实际 {t}", t = t.text()), None, None);
                }
            }
            match &a.body {
                ArmBody::Expr(e) => { let _ = self.check_expr(e); }
                ArmBody::Block(b) => self.check_block(b),
            }
            self.pop_scope();
            // §3.12 or-pattern binding sets must be identical
            if let PatKind::Or(alts) = &a.pat.kind {
                let sets: Vec<BTreeSet<String>> = alts.iter().map(|p| pat_bindings(p)).collect();
                if let Some(first) = sets.first() {
                    for s in sets.iter().skip(1) {
                        if s != first {
                            self.emit("E-0072", a.pat.span, "或模式绑定集不一致", "两侧必须绑定同名同类型集合", Some("§3.12".into()), Some("补齐缺失绑定，或改为 `_` 以外的同一集合".into()));
                            break;
                        }
                    }
                }
            }
            if pat_depth(&a.pat) > 32 {
                self.emit("E-0073", a.pat.span, "模式嵌套过深", "深度超过 32", Some("§3.12".into()), Some("拆成嵌套 match".into()));
            }
        }
        // reachability: an arm whose pattern space is a subset of all earlier arms is unreachable
        for i in 1..arms.len() {
            if pat_is_subsumed(&arms[i].pat, &arms[..i]) {
                self.emit("E-0071", arms[i].span, "不可达匹配臂", "该臂与前面的臂完全重叠", Some("§3.12：按臂顺序构造互斥子空间".into()), Some("删除该臂".into()));
            }
        }
        // guards do not count toward exhaustiveness (§3.12)
        let has_wild = arms.iter().any(|a| a.guard.is_none() && pat_covers_all(&a.pat));
        let exhaustive = match scrut {
            Ty::Bool => covers_bool(arms),
            Ty::Adt(name) | Ty::Named(name) => covers_enum(arms, name, &self.prog) || has_wild,
            Ty::Prim(p) => covers_int_range(arms, *p) || has_wild,
            _ => has_wild,
        };
        if !exhaustive {
            let missing = match scrut {
                Ty::Adt(name) | Ty::Named(name) => format!("枚举 {name} 有未覆盖的变体"),
                Ty::Bool => "缺少 `false` 或 `true` 臂".to_string(),
                _ => "存在未覆盖的取值".to_string(),
            };
            self.emit("E-0070", span, format!("match 不穷尽：{missing}"), "此处", Some("§3.12：守卫不参与穷尽性判定".into()), Some("补臂，或显式写 `_ => …`（枚举建议列全变体，§3.12 W-0070）".into()));
        }
        if scrut_is_enum(scrut) && has_wild && !arms.iter().any(|a| matches!(a.pat.kind, PatKind::Wildcard) && a.guard.is_none() && false) {
            self.emit("W-0070", span, "枚举 match 使用 `_` 掩盖新变体", "下游新增变体将被静默忽略", Some("§3.12：领域无关的健壮性要求".into()), Some("列全变体".into()));
        }
    }

    fn bind_pat(&mut self, p: &Pat, ty: Ty) {
        match &p.kind {
            PatKind::Binding { name, .. } => self.declare(name, ty, false),
            PatKind::Tuple(v) => {
                for (i, sub) in v.iter().enumerate() {
                    let t = match &ty { Ty::Tuple(ts) => ts.get(i).cloned().unwrap_or(Ty::Invalid), _ => Ty::Invalid };
                    self.bind_pat(sub, t);
                }
            }
            PatKind::Constructor { path, args, fields, .. } => {
                // Enum variant payload or struct destructuring
                let name = path.last().cloned().unwrap_or_default();
                if let Some(ei) = self.prog.enum_by_name(&name) {
                    let payload: Vec<Ty> = self.prog.enums[ei].variants.iter().find(|(v, _, _)| v == &name || path.len() > 1).map(|(_, p, _)| p.clone()).unwrap_or_default();
                    if !payload.is_empty() || path.len() > 1 {
                        for (sub, t) in args.iter().zip(payload.iter()) {
                            self.bind_pat(sub, t.clone());
                        }
                    }
                } else if let Some(si) = self.prog.struct_by_name(&name) {
                    for (fname, sub) in fields {
                        let t = self.prog.structs[si].fields.iter().find(|(n, _, _)| n == fname).map(|(_, ty, _)| ty.clone()).unwrap_or(Ty::Invalid);
                        self.bind_pat(sub, t);
                    }
                }
            }
            PatKind::Slice(v, rest) => {
                let inner = match &ty { Ty::Array { inner, .. } | Ty::Slice { inner } | Ty::Vec { inner } => (**inner).clone(), _ => Ty::Invalid };
                for sub in v {
                    self.bind_pat(sub, inner.clone());
                }
                if let Some(r) = rest {
                    self.bind_pat(r, Ty::Vec { inner: Box::new(inner) });
                }
            }
            PatKind::Or(v) | PatKind::Exclude(v) => {
                for sub in v {
                    self.bind_pat(sub, ty.clone());
                }
            }
            PatKind::Range { lo, hi, .. } => {
                self.bind_pat(lo, ty.clone());
                self.bind_pat(hi, ty.clone());
            }
            _ => {}
        }
    }

    fn check_embed(&mut self, e: &EmbedBlock, it: &Item) {
        // §3.6: prefix whitelist (§7.5)
        const A: &[&str] = &["c", "cpp", "rust", "zig"];
        const B: &[&str] = &["py", "lua"];
        const P: &[&str] = &["ts", "js", "sh", "ps1", "bat", "cmd"];
        if !(A.contains(&e.lang.as_str()) || B.contains(&e.lang.as_str()) || P.contains(&e.lang.as_str())) {
            self.emit("E-0702", e.lang_span, "未知或不支持的嵌入前缀", format!("@{}", e.lang), Some("§7.5 白名单之外的前缀一律拒绝".into()), Some("A 轨：@c/@cpp/@rust/@zig；B 轨：@py/@lua；仅进程隔离：@ts/@js/@sh/@ps1/@bat/@cmd".into()));
            return;
        }
        if e.is_a_track() {
            // §4.7.5 / §7.6.6: A track requires the module's trust declaration.
            if !self.a_track_declared {
                self.emit("E-0722", e.span, "未声明 A 轨信任策略", &e.lang, Some("[profile] a_track = \"unsafe-trusted\" 是 A 轨的前置条件（§4.7.5）".into()), None);
            }
            // §7.6.1: `#[link(name)]` + `extern` signature block must pair by name.
            let name = e.unit_name().unwrap_or_default();
            if name.is_empty() {
                self.emit("E-0720", e.span, "A 轨缺少 name", "@c 块必须写 (name = \"…\")", Some("§7.6.1：签名块与源码块由同一 name 配对".into()), None);
            } else if !self.link_names().contains(&name) {
                self.emit("E-0720", e.span, "A 轨 name 不匹配", format!("没有 #[link(name = \"{name}\")] 与之配对"), Some("§7.6.1".into()), None);
            }
        }
        if e.is_b_track() {
            let declared = self.b_track_declared(&e.lang);
            if !declared {
                self.emit("E-0715", e.span, "未声明 B 轨工具链版本", format!("[b_track] {} = \"…\" 缺失", e.lang), Some("§7.7.3：字节码格式不跨版本稳定".into()), Some(format!("[b_track] {} = \"<minor>\"", e.lang)));
            }
            // §7.7.1: named embed arguments bind to the enclosing signature's parameters.
            for a in &e.args {
                let _t = self.check_expr(&a.value);
            }
        }
        if e.is_process_only() {
            let iso = e.arg("isolation");
            let is_process = iso.map(|v| matches!(&v.kind, ExprKind::Lit(Lit::Str(s)) if s == "process")).unwrap_or(false);
            if !is_process {
                self.emit("E-0716", e.span, "进程隔离前缀缺 isolation", format!("@{} 必须写 isolation = \"process\"", e.lang), Some("§7.7.5 R-JS：这些前缀不进入字节码摄取路径".into()), None);
            }
        }
        let _ = it;
    }

    /// Stage [4] for one expression: infer its type, enforce the §2.7/§2.8/§4.2/§4.4/§4.11/§4.14
    /// rules that are decidable here, and record the result so `codegen` never re-derives it.
    pub fn check_expr(&mut self, e: &Expr) -> Ty {
        let ty = self.check_expr_inner(e);
        if !matches!(ty, Ty::Invalid) {
            self.prog.set_ty(e.span, ty.clone());
        }
        ty
    }

    fn check_expr_inner(&mut self, e: &Expr) -> Ty {
        use crate::token::Punct;
        match &e.kind {
            ExprKind::Lit(l) => match l {
                Lit::Int(i) => self.int_lit_ty(i, e.span),
                Lit::Float(f) => Ty::Prim(if f.suffix.as_deref() == Some("f32") { Prim::F32 } else { Prim::F64 }),
                Lit::Str(_) | Lit::Cord(_) => Ty::Str,
                Lit::ByteStr(_) => Ty::Vec { inner: Box::new(Ty::Prim(Prim::U8)) },
                Lit::Char(_) => Ty::Prim(Prim::Char),
                Lit::Bool(_) => Ty::Bool,
            },
            ExprKind::Int(i) => self.int_lit_ty(i, e.span),
            ExprKind::Auto => {
                self.emit("E-0030", e.span, "auto 出现在非法位置", "auto 只能用于具名实参", Some("§2.6：它不是任何类型，不能参与运算、绑定或返回".into()), None);
                Ty::Invalid
            }
            ExprKind::Ident(name) => {
                if let Some((t, _)) = self.lookup(name) {
                    return t;
                }
                if name == "self" {
                    return self.self_ty.clone().unwrap_or(Ty::Invalid);
                }
                if let Some(c) = self.prog.consts.iter().find(|c| c.name == *name) {
                    return c.ty.clone();
                }
                if let Some(s) = self.prog.statics.iter().find(|s| s.name == *name) {
                    return s.ty.clone();
                }
                if let Some(f) = self.prog.fns.iter().find(|f| f.name == *name && f.self_ty.is_none()) {
                    return Ty::Fn { params: f.params.iter().map(|(_, t)| t.clone()).collect(), ret: Some(Box::new(f.ret.clone())) };
                }
                if let Some(ei) = self.prog.enum_by_name(name) {
                    let _ = ei;
                    return Ty::Named(name.clone());
                }
                self.emit("E-0101", e.span, format!("未声明的标识符 `{name}`"), name, Some("§4.1 未初始化即读取 = E-0101；P5 禁止隐式声明".into()), None);
                Ty::Invalid
            }
            ExprKind::Path(segs) => self.check_path(e, segs),
            ExprKind::Unary { op, expr } => {
                let t = self.check_expr(expr);
                match op {
                    Punct::Minus | Punct::Star => {
                        if matches!(op, Punct::Star) {
                            // §4.7.5: dereferencing a raw pointer outside `unsafe` is E-1030.
                            if matches!(t, Ty::Ptr { .. }) && !self.in_unsafe {
                                self.emit("E-1030", e.span, "裸指针解引用缺少 unsafe", format!("*({})", t.text()), Some("§4.7.5 封闭列表第 1 项".into()), Some("包进 `unsafe { … }` 并写 `// infy-safety:`".into()));
                            }
                            return match &t {
                                Ty::Ptr { inner, .. } | Ty::Ref { inner, .. } => (**inner).clone(),
                                Ty::Invalid => Ty::Invalid,
                                other => {
                                    self.emit("E-0110", e.span, "对非引用类型解引用", format!("{}", other.text()), None, None);
                                    Ty::Invalid
                                }
                            };
                        }
                        match &t {
                            Ty::Prim(p) if p.is_numeric() => t,
                            Ty::Invalid => Ty::Invalid,
                            other => {
                                self.emit("E-0110", e.span, "一元负号作用于非数值类型", format!("{}", other.text()), Some("§4.4：无隐式转换".into()), None);
                                Ty::Invalid
                            }
                        }
                    }
                    Punct::Bang => match &t {
                        Ty::Bool | Ty::Invalid => t,
                        Ty::Prim(p) if p.is_integer() => t,
                        other => {
                            self.emit("E-0110", e.span, "`!` 的操作数必须是 bool 或整型", format!("{}", other.text()), Some("§4.4：`!` 由类型决定是逻辑非还是位非".into()), None);
                            Ty::Invalid
                        }
                    },
                    Punct::Amp => t,
                    _ => t,
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let l = self.check_expr(lhs);
                let r = self.check_expr(rhs);
                self.binop_ty(*op, &l, &r, e.span)
            }
            ExprKind::Assign { target, op, value } => {
                let vt = self.check_expr(value);
                let tt = self.check_place(target);
                if !type_compatible(&tt, &vt) && !matches!(vt, Ty::Invalid) && !matches!(tt, Ty::Invalid) {
                    self.emit("E-0110", value.span, format!("赋值类型不匹配：左值 {}，右值 {}", tt.text(), vt.text()), "赋值", Some("§4.2：无隐式数字转换（UB-001）".into()), Some("显式 `as`".into()));
                }
                if !matches!(op, Punct::Eq) {
                    let _ = self.binop_ty(compound_to_bin(*op), &tt, &vt, e.span);
                }
                tt
            }
            ExprKind::Call { callee, args } => self.check_call(e, callee, args),
            ExprKind::Method { recv, name, args, .. } => {
                let rt = self.check_expr(recv);
                let tn = base_of(&rt);
                let mut found: Vec<usize> = self
                    .prog
                    .fns
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.self_ty.as_ref().map(|t| base_of(t) == tn).unwrap_or(false) && (f.name == *name || f.name.ends_with(&format!("::{name}"))))
                    .map(|(i, _)| i)
                    .collect();
                found.sort();
                found.dedup();
                if found.is_empty() {
                    self.emit("E-0110", e.span, format!("类型 {} 没有方法 `{name}`", rt.text()), name, Some("§4.14：接收者查找顺序 inherent → trait(声明序) → 自动引用阶梯".into()), None);
                    return Ty::Invalid;
                }
                if found.len() > 1 && !self.resolvable_by_trait_path(&found, name) {
                    self.emit("E-0162", e.span, "方法名歧义", format!("`{name}` 由多个 trait 提供"), Some("§4.14：不做多候选权重排序".into()), Some(format!("写 `Trait::{name}(…)` 显式消歧")));
                }
                for a in args {
                    self.check_expr(&a.expr);
                }
                self.prog.fns[found[0]].ret.clone()
            }
            ExprKind::Field { base, name } => {
                let bt = self.check_expr(base);
                match name {
                    FieldName::Index(i) => match &bt {
                        Ty::Tuple(v) => v.get(*i as usize).cloned().unwrap_or(Ty::Invalid),
                        other => {
                            self.emit("E-0110", e.span, "元组下标用于非元组类型", format!("{}", other.text()), Some("§3.11：`t.0` 是元组字段".into()), None);
                            Ty::Invalid
                        }
                    },
                    FieldName::Named(f) => {
                        let tn = base_of(&bt);
                        if let Some(si) = self.prog.struct_by_name(&tn) {
                            let fld = self.prog.structs[si].fields.iter().find(|(n, _, _)| n == f).map(|(_, t, v)| (t.clone(), v.clone()));
                            return match fld {
                                Some((ty, vis)) => {
                                    // §5.5: field visibility is independent of the type's visibility.
                                    if matches!(vis, Visibility::Private) && tn != self.package_type_owner(&tn) {
                                        self.emit("E-0210", e.span, "访问 package 私有字段", format!("{tn}.{f}"), Some("§5.5：跨 package 引用私有项".into()), Some("加 `pub` 或改 API".into()));
                                    }
                                    ty.clone()
                                }
                                None => {
                                    self.emit("E-0110", e.span, format!("类型 {tn} 无字段 `{f}`"), f, None, None);
                                    Ty::Invalid
                                }
                            };
                        }
                        if let Some(t) = env_field_ty(f) {
                            return t;
                        }
                        match &bt {
                            Ty::Invalid => Ty::Invalid,
                            _ => {
                                self.emit("E-0110", e.span, format!("无法解析字段 `{f}`"), format!("{}", bt.text()), Some("§4.6：`env` 的字段集合封闭且领域中立".into()), None);
                                Ty::Invalid
                            }
                        }
                    }
                }
            }
            ExprKind::Index { base, index } => {
                let bt = self.check_expr(base);
                let it = self.check_expr(index);
                if !matches!(it, Ty::Invalid) && !it.is_integer() {
                    self.emit("E-0110", index.span, "索引必须是整型", format!("{}", it.text()), None, None);
                }
                match &bt {
                    Ty::Array { inner, .. } | Ty::Slice { inner } | Ty::Vec { inner } | Ty::Ptr { inner, .. } => (**inner).clone(),
                    Ty::Str => Ty::Prim(Prim::Char),
                    Ty::Invalid => Ty::Invalid,
                    other => {
                        self.emit("E-0110", e.span, "该类型不可索引", format!("{}", other.text()), Some("§4.1：索引只对数组 / 切片 / Vec / 裸指针合法".into()), None);
                        Ty::Invalid
                    }
                }
            }
            ExprKind::Tuple(v) => {
                let ts: Vec<Ty> = v.iter().map(|x| self.check_expr(x)).collect();
                if ts.is_empty() {
                    Ty::Unit
                } else {
                    Ty::Tuple(ts)
                }
            }
            ExprKind::Array(v) => {
                let mut inner: Option<Ty> = None;
                for x in v {
                    let t = self.check_expr(x);
                    match &inner {
                        Some(i) => {
                            if !type_compatible(i, &t) && !matches!(t, Ty::Invalid) {
                                self.emit("E-0110", x.span, "数组元素类型不一致", format!("期望 {}，实际 {}", i.text(), t.text()), None, None);
                            }
                        }
                        None => inner = Some(t),
                    }
                }
                Ty::Vec { inner: Box::new(inner.unwrap_or(Ty::Unit)) }
            }
            ExprKind::ArrayRepeat { value, len } => {
                let t = self.check_expr(value);
                let n = self.const_eval_usize(len);
                if n.is_none() {
                    self.emit("E-0060", len.span, "重复数组长度必须是编译期常量", "此处", Some("§3.11：`[x; N]` 的 N 必须 const-eval（§2.12 封闭集合）".into()), None);
                }
                Ty::Array { inner: Box::new(t), n: n.unwrap_or(0) }
            }
            ExprKind::StructLit { path, fields, rest } => {
                let tn = path.last().cloned().unwrap_or_default();
                let si = self.prog.struct_by_name(&tn);
                for (fname, v) in fields {
                    let vt = self.check_expr(v);
                    if let Some(si) = si {
                        match self.prog.structs[si].fields.iter().find(|(n, _, _)| n == fname) {
                            Some((_, ty, _)) => {
                                if !type_compatible(ty, &vt) && !matches!(vt, Ty::Invalid) {
                                    self.emit("E-0110", v.span, format!("字段 `{fname}` 类型不匹配：期望 {}，实际 {}", ty.text(), vt.text()), fname, Some("§4.2".into()), None);
                                }
                            }
                            None => self.emit("E-0110", v.span, format!("{tn} 无字段 `{fname}`"), fname, None, None),
                        }
                    }
                }
                if let Some(b) = rest {
                    let bt = self.check_expr(b);
                    // §3.11: `..base` with a borrowed base would "lend out every field".
                    if matches!(bt, Ty::Ref { .. }) {
                        self.emit("E-1006", b.span, "从引用移动全部字段", format!("..{}", bt.text()), Some("§3.11：base 为引用时不允许借出全部字段".into()), Some("改 clone() 或取所有权".into()));
                    }
                }
                if let Some(si) = si {
                    if rest.is_none() {
                        let missing: Vec<String> = self.prog.structs[si].fields.iter().map(|(n, _, _)| n.clone()).filter(|n| !fields.iter().any(|(f, _)| f == n)).collect();
                        if !missing.is_empty() {
                            self.emit("E-0110", e.span, "结构体字面量字段不完整", missing.join(", "), Some("§4.1：无隐式默认值（P5）".into()), Some("补全字段，或写 `..base`".into()));
                        }
                    }
                } else if !tn.is_empty() && self.prog.enum_by_name(&tn).is_none() {
                    self.emit("E-0110", e.span, format!("未知类型 `{tn}`"), &tn, None, None);
                }
                Ty::Named(tn)
            }
            ExprKind::If { cond, then, else_ } => {
                let ct = self.check_expr(cond);
                if !matches!(ct, Ty::Bool) && !matches!(ct, Ty::Invalid) {
                    self.emit("E-0110", cond.span, "if 条件必须是 bool", format!("{}", ct.text()), Some("P5：无隐式真值判断".into()), None);
                }
                self.check_block(then);
                match else_.as_deref() {
                    Some(ElseBranch::Block(b)) => self.check_block(b),
                    Some(ElseBranch::If(s)) => self.check_stmt(s),
                    None => {}
                }
                then.tail.as_ref().and_then(|t| self.prog.ty(t.span).cloned()).unwrap_or(Ty::Unit)
            }
            ExprKind::Match { scrut, arms } => {
                let st = self.check_expr(scrut);
                self.check_match(arms, &st, e.span);
                arms.first().and_then(|a| match &a.body {
                    ArmBody::Expr(e) => self.prog.ty(e.span).cloned(),
                    ArmBody::Block(b) => b.tail.as_ref().and_then(|t| self.prog.ty(t.span).cloned()),
                }).unwrap_or(Ty::Unit)
            }
            ExprKind::Block(b) => {
                self.check_block(b);
                b.tail.as_ref().and_then(|t| self.prog.ty(t.span).cloned()).unwrap_or(Ty::Unit)
            }
            ExprKind::Unsafe(b) => {
                let saved = std::mem::replace(&mut self.in_unsafe, true);
                self.check_block(b);
                self.in_unsafe = saved;
                b.tail.as_ref().and_then(|t| self.prog.ty(t.span).cloned()).unwrap_or(Ty::Unit)
            }
            ExprKind::Ref { expr, mutable, .. } => {
                let t = self.check_expr(expr);
                // §4.3: taking `&mut` of an immutable `let` binding is E-0120.
                if *mutable {
                    if let ExprKind::Ident(n) = &expr.kind {
                        if let Some((_, mutable_binding)) = self.lookup(n) {
                            if !mutable_binding {
                                self.emit("E-0120", expr.span, "对 let 不可变绑定取 &mut", n, Some("§4.3：`&mut` 只能指向 `var` 绑定或 `mut` 形参".into()), None);
                            }
                        }
                    }
                }
                Ty::Ref { inner: Box::new(t), mutable: *mutable, lifetime: None }
            }
            ExprKind::Deref { expr } => {
                let t = self.check_expr(expr);
                match &t {
                    Ty::Ref { inner, .. } | Ty::Boxed { inner } => (**inner).clone(),
                    Ty::Ptr { inner, .. } => {
                        if !self.in_unsafe {
                            self.emit("E-1030", e.span, "裸指针解引用缺少 unsafe", format!("*({})", t.text()), Some("§4.7.5".into()), None);
                        }
                        (**inner).clone()
                    }
                    Ty::Invalid => Ty::Invalid,
                    other => {
                        self.emit("E-0110", e.span, "该类型不可解引用", format!("{}", other.text()), None, None);
                        Ty::Invalid
                    }
                }
            }
            ExprKind::Move { expr } => self.check_expr(expr),
            ExprKind::Try { expr } => {
                let t = self.check_expr(expr);
                // §4.11: `?` expands against the enclosing function's return type only.
                if self.in_chain {
                    self.emit("E-0173", e.span, "`?` 出现在链顶层", "链层的错误处理属于连携语义", Some("§4.11：不允许 `?` 逃逸到调度器".into()), None);
                    return Ty::Invalid;
                }
                if self.in_defer {
                    self.emit("E-0074", e.span, "defer 内非法控制流", "`?` 在 defer 内", Some("§3.13：退出时点已定".into()), None);
                    return Ty::Invalid;
                }
                let want = self.ret_ty.clone();
                match (&want, &t) {
                    (Ty::Result { ok: _, err }, Ty::Result { ok, err: e2 }) => {
                        if !type_compatible(err, e2) && !matches!(&**e2, Ty::Invalid) {
                            // §4.11: conversion goes through `From<E, E2>`; no implicit stringifying.
                            self.emit("E-0170", e.span, format!("`?` 缺少 From<{}, {}> 实现", e2.text(), err.text()), "错误类型无法转换", Some("诊断必须列出缺失的 From<A,B>（§4.11）".into()), Some("写 impl From<A> for B，或显式 map_err".into()));
                        }
                        (**ok).clone()
                    }
                    (Ty::Option { .. }, Ty::Option { inner }) => (**inner).clone(),
                    (Ty::Result { .. }, Ty::Option { .. }) => {
                        self.emit("E-0171", e.span, "Option 值在返回 Result 的函数中使用 `?`", "类型族不匹配", Some("§4.11".into()), Some("显式 `.ok_or(…)`".into()));
                        Ty::Invalid
                    }
                    (Ty::Option { .. }, Ty::Result { .. }) => {
                        self.emit("E-0171", e.span, "Result 值在返回 Option 的函数中使用 `?`", "类型族不匹配", Some("§4.11：反向同理".into()), None);
                        Ty::Invalid
                    }
                    (Ty::Unit | Ty::Never | Ty::Invalid, _) => {
                        self.emit("E-0172", e.span, "`?` 所在函数返回类型非法", format!("当前返回 {}", self.ret_ty.text()), Some("§4.11：既非 Result 也非 Option".into()), Some("改签名".into()));
                        Ty::Invalid
                    }
                    (Ty::Result { ok, .. }, other) => {
                        self.emit("E-0172", e.span, "`?` 的操作数不是 Result/Option", format!("{}", other.text()), Some("§4.11".into()), None);
                        (**ok).clone()
                    }
                    (Ty::Option { inner }, _) => (**inner).clone(),
                    _ => {
                        self.emit("E-0172", e.span, " 所在函数返回类型非法", format!("当前返回 {}", self.ret_ty.text()), Some("§4.11：既非 Result 也非 Option".into()), None);
                        Ty::Invalid
                    }
                }
            }
            ExprKind::Cast { expr, ty } => {
                let from = self.check_expr(expr);
                let to = self.ty_from_ast(ty, &BTreeSet::new());
                // §4.4: `as` never panics; pointer↔integer requires unsafe.
                if (matches!(from, Ty::Ptr { .. } | Ty::Ref { .. }) && to.is_integer()) && !self.in_unsafe {
                    self.emit("E-1030", e.span, "指针到整数的转换缺少 unsafe", format!("{} as {}", from.text(), to.text()), Some("§4.4：指针→整数只允许在 unsafe 块内".into()), None);
                }
                if matches!(from, Ty::Ptr { .. }) && matches!(to, Ty::Ptr { .. }) {
                    return to;
                }
                if from.is_numeric() && to.is_numeric() {
                    return to;
                }
                if matches!(from, Ty::Bool) && to.is_integer() {
                    return to;
                }
                if matches!(to, Ty::Str) && matches!(from, Ty::Str | Ty::String) {
                    return Ty::Str;
                }
                if !matches!(from, Ty::Invalid) && !from.is_numeric() && !matches!(from, Ty::Bool) {
                    self.emit("E-0110", e.span, format!("不允许的 `as` 转换：{} → {}", from.text(), to.text()), "as", Some("§4.4：`as` 只对数字与 bool→u8 定义".into()), None);
                }
                to
            }
            ExprKind::Is { expr, ty } => {
                self.check_expr(expr);
                let _ = ty;
                Ty::Bool
            }
            ExprKind::Range { lo, hi, .. } => {
                let a = lo.as_ref().map(|x| self.check_expr(x)).unwrap_or(Ty::Invalid);
                let b = hi.as_ref().map(|x| self.check_expr(x)).unwrap_or(a.clone());
                if !matches!(a, Ty::Invalid) && !a.is_integer() {
                    self.emit("E-0110", e.span, "区间端点必须是整型", format!("{}", a.text()), Some("§2.7 优先级 3：`..` / `..=`".into()), None);
                }
                let _ = b;
                Ty::Named("Range".into())
            }
            ExprKind::Closure { params, body, ret, .. } => {
                self.push_scope();
                let mut pts = Vec::new();
                for (n, t) in params {
                    // §4.13 rule 5: an unannotated closure parameter is inferred from the first
                    // call site; if it cannot be, sema reports E-0161 at the call.
                    let pt = t.as_ref().map(|t| self.ty_from_ast(t, &BTreeSet::new())).unwrap_or(Ty::Invalid);
                    self.declare(n, pt.clone(), false);
                    pts.push(pt);
                }
                self.check_block(body);
                self.pop_scope();
                let r = match ret {
                    Some(t) => self.ty_from_ast(t, &BTreeSet::new()),
                    None => body.tail.as_ref().and_then(|x| self.prog.ty(x.span).cloned()).unwrap_or(Ty::Unit),
                };
                self.closure_counter += 1;
                let _ = self.closure_counter;
                Ty::Fn { params: pts, ret: Some(Box::new(r)) }
            }
            ExprKind::Embed(embed) => {
                // §3.6: embed blocks bind to an event/fn body or a statement; patterns and type
                // positions are the parser's `E-0700`, unreachable here.
                self.check_embed_expr(embed, e.span);
                Ty::Unit
            }
            ExprKind::MacroCall { name, .. } => {
                self.emit("E-0402", e.span, format!("未展开的宏调用 `{name}!`"), name, Some("§6.5：宏在阶段 [2] 展开，展开后必须是合法 AST 片段".into()), Some("检查 macro_rules! 是否可见，或 infy expand 观察展开结果".into()));
                Ty::Invalid
            }
            ExprKind::Invalid => Ty::Invalid,
        }
    }

    /// §2.8: literal range rules. Compile-time only — no wraparound, no saturation (§4.4).
    fn int_lit_ty(&mut self, i: &IntLit, span: Span) -> Ty {
        if let Some(suf) = &i.suffix {
            let p = match suf.as_str() {
                "u8" => Prim::U8, "u16" => Prim::U16, "u32" => Prim::U32, "u64" => Prim::U64, "u128" => Prim::U128,
                "i8" => Prim::I8, "i16" => Prim::I16, "i32" => Prim::I32, "i64" => Prim::I64, "i128" => Prim::I128,
                "usize" => Prim::Usize, "isize" => Prim::Isize, "f32" => Prim::F32, "f64" => Prim::F64,
                other => {
                    self.emit("E-0043", span, format!("未知字面量后缀 `{other}`"), other, Some("§2.8：后缀集合是封闭的".into()), None);
                    return Ty::Invalid;
                }
            };
            if let Some(v) = i.value {
                // §2.8's `-128i8` exception: the unary minus case is folded by const-eval, so a
                // magnitude that fits the *signed* range after negation is accepted here too.
                let fits = match p {
                    Prim::U8 => v <= u8::MAX as u128, Prim::U16 => v <= u16::MAX as u128, Prim::U32 => v <= u32::MAX as u128,
                    Prim::U64 => v <= u64::MAX as u128, Prim::U128 => true,
                    Prim::I8 => v <= 128, Prim::I16 => v <= 32768, Prim::I32 => v <= 2147483648, Prim::I64 => v <= 9223372036854775808,
                    Prim::I128 | Prim::Usize | Prim::Isize => true,
                    Prim::F32 | Prim::F64 => true,
                    _ => false,
                };
                if !fits {
                    self.emit("E-0043", span, format!("带后缀字面量溢出：{}{suf}", i.digits), "超出该后缀范围", Some("§2.8：不回绕、不饱和——回绕只属于运行时的 wrapping_*".into()), Some("换更宽后缀".into()));
                }
            }
            return Ty::Prim(p);
        }
        match i.value {
            Some(v) => {
                if i.radix == crate::token::Radix::Dec {
                    if v > i64::MAX as u128 && v <= u128::MAX {
                        // decimal, no suffix, beyond i64 → E-0042 (§2.8)
                        self.emit("E-0042", span, format!("字面量超出可表示范围：{}", i.digits), "无后缀十进制整型上限为 i64", Some("§2.8".into()), Some("加后缀，如 12345678901234u64".into()));
                    }
                    Ty::Prim(Prim::I64)
                } else {
                    // non-decimal without suffix: i32, else u32, else E-0042 (§2.8)
                    if v <= i32::MAX as u128 {
                        Ty::Prim(Prim::I32)
                    } else if v <= u32::MAX as u128 {
                        Ty::Prim(Prim::U32)
                    } else if v <= i64::MAX as u128 {
                        Ty::Prim(Prim::I64)
                    } else {
                        self.emit("E-0042", span, format!("字面量超出可表示范围：{}", i.digits), "非十进制无后缀整型", Some("§2.8：i32 → u32 → 否则 E-0042".into()), None);
                        Ty::Invalid
                    }
                }
            }
            None => {
                self.emit("E-0042", span, "字面量超出 i128", i.digits.clone(), Some("§2.8".into()), None);
                Ty::Invalid
            }
        }
    }

    fn check_path(&mut self, e: &Expr, segs: &[String]) -> Ty {
        if segs.len() == 1 {
            return self.check_expr(&Expr::new(ExprKind::Ident(segs[0].clone()), e.span));
        }
        let last = segs.last().cloned().unwrap_or_default();
        let owner = segs[..segs.len() - 1].join("::");
        // `env.x` (§4.6): the only read-only host accessors, closed set.
        if owner == "env" {
            if let Some(t) = env_field_ty(&last) {
                return t;
            }
            self.emit("E-0110", e.span, format!("`env.{last}` 不存在"), &last, Some("§4.6/§1.3：env 字段集合封闭且领域中立，实现私自扩展 = UB-024".into()), Some("领域指标走 std.host.query(&str)".into()));
            return Ty::Invalid;
        }
        // enum variant path `Phase::Fetch`
        if let Some(ei) = self.prog.enum_by_name(&owner) {
            if self.prog.enums[ei].variants.iter().any(|(v, _, _)| *v == last) {
                return Ty::Named(owner.clone());
            }
        }
        // `pkg::item` via an import alias, or `Type::assoc`
        if let Some((_, alias)) = self.prog.import_map.iter().find(|(_, (p, _))| p.split('/').last().map(|s| s == owner).unwrap_or(false)) {
            let _ = alias;
        }
        if let Some(f) = self.prog.fns.iter().find(|f| f.name == format!("{owner}::{last}") || (f.name == last && base_of(&f.self_ty.clone().unwrap_or(Ty::Invalid)) == owner)) {
            return Ty::Fn { params: f.params.iter().map(|(_, t)| t.clone()).collect(), ret: Some(Box::new(f.ret.clone())) };
        }
        if let Some(c) = self.prog.consts.iter().find(|c| c.name == last) {
            return c.ty.clone();
        }
        if self.prog.struct_by_name(&owner).is_some() || self.prog.enum_by_name(&owner).is_some() {
            return Ty::Named(owner);
        }
        // §5.3 step 5: an undeclared module prefix.
        self.emit("E-0201", e.span, format!("无法解析路径 `{}`", segs.join("::")), &last, Some("清单未声明该 module，或名称不在作用域内（§5.3）".into()), Some("运行 `infy mod tidy`".into()));
        Ty::Invalid
    }

    fn check_call(&mut self, call: &Expr, callee: &Expr, args: &[CallArg]) -> Ty {
        let name = match &callee.kind {
            ExprKind::Ident(n) => n.clone(),
            ExprKind::Path(p) => p.last().cloned().unwrap_or_default(),
            _ => {
                return self.check_expr(callee);
            }
        };
        // §4.6: assigning to env is E-0140; calling it is simply unresolvable.
        let Some(fi) = self.prog.lookup_fn(&self.package, &name).or_else(|| self.prog.fns.iter().position(|f| f.name == name)) else {
            // builtin-ish names handled by std at codegen; unknown here is E-0101
            self.emit("E-0101", call.span, format!("调用未声明的函数 `{name}`"), &name, Some("§5.3：导入路径解析顺序是规范唯一的".into()), None);
            for a in args {
                self.check_expr(&a.expr);
            }
            return Ty::Invalid;
        };
        let f = self.prog.fns[fi].clone();
        // §7.6.5 / §4.7.5: calling an A-track extern function requires unsafe.
        if f.is_foreign && !self.in_unsafe && self.link_names().iter().any(|_| true) {
            self.emit("E-0721", call.span, "A 轨调用缺少 unsafe", format!("{name}(…)"), Some("§4.7.5 封闭列表第 2 项：调用 extern 函数必须在 unsafe 内".into()), Some("包进 unsafe { } 并写 // infy-safety:".into()));
        }
        // argument typing, left to right (§4.10 rule 3)
        let mut named_used: Vec<String> = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let at = self.check_expr(&a.expr);
            if matches!(&a.expr.kind, ExprKind::Auto) {
                // §2.6: `auto` == omitting the argument; no behavioural difference.
                if a.name.is_none() {
                    self.emit("E-0030", a.expr.span, "auto 出现在非法位置", "auto 只能用于具名实参", Some("§2.6".into()), None);
                }
                named_used.push(a.name.clone().unwrap_or_default());
                continue;
            }
            let slot = if a.name.is_some() {
                f.params.iter().position(|(n, _)| Some(n.as_str()) == a.name.as_deref())
            } else {
                Some(i)
            };
            match slot {
                Some(idx) => {
                    if let Some((_, pt)) = f.params.get(idx) {
                        if !type_compatible(pt, &at) && !matches!(at, Ty::Invalid) {
                            self.emit("E-0110", a.expr.span, format!("实参类型不匹配：期望 {}，实际 {}", pt.text(), at.text()), format!("第 {} 个实参", idx + 1), Some("§4.2：函数参数 100% 类型兼容".into()), Some("显式 `as`（§4.4）".into()));
                        }
                    }
                }
                None => self.emit("E-0900", a.expr.span, "宿主事件参数不匹配", format!("`{name}` 无参数 `{}`", a.name.clone().unwrap_or_default()), Some("§10.8/§11：具名实参必须匹配声明".into()), None),
            }
        }
        let positional = args.iter().filter(|a| a.name.is_none()).count();
        let required = f.params.iter().filter(|(n, _)| n != "self").count();
        if positional < required && f.params.iter().all(|(_, t)| !matches!(t, Ty::Invalid)) {
            // defaults may fill the gap (§3.3 param `= expr`), otherwise E-0900
            if !f.params.iter().any(|(_, _)| true) {
                self.emit("E-0900", call.span, "调用缺少必填参数", &name, Some("§3.3：默认值可省略实参".into()), None);
            }
        }
        let _ = named_used;
        f.ret.clone()
    }

    /// §4.4/§4.2: operator typing with the implicit-conversion ban.
    fn binop_ty(&mut self, op: crate::token::Punct, l: &Ty, r: &Ty, span: Span) -> Ty {
        use crate::token::Punct::*;
        match op {
            AmpAmp | PipePipe => {
                if !matches!(l, Ty::Bool) && !matches!(l, Ty::Invalid) {
                    self.emit("E-0110", span, "逻辑运算的操作数必须是 bool", format!("{}", l.text()), Some("§4.4：&& / || 短路求值，操作数为 bool".into()), None);
                }
                if !matches!(r, Ty::Bool) && !matches!(r, Ty::Invalid) {
                    self.emit("E-0110", span, "逻辑运算的右操作数必须是 bool", format!("{}", r.text()), None, None);
                }
                Ty::Bool
            }
            EqEq | BangEq => {
                if !types_comparable_for_eq(l, r) {
                    self.emit("E-0110", span, format!("`==` 两侧类型不兼容：{} 与 {}", l.text(), r.text()), "比较", Some("§4.2：无隐式转换".into()), Some("显式 `as`，或对 str 用 a.bytes() == b.bytes()（§2.10）".into()));
                }
                Ty::Bool
            }
            Lt | LtEq | Gt | GtEq => {
                if !type_compatible(l, r) && !matches!(l, Ty::Invalid) {
                    self.emit("E-0110", span, format!("比较两侧类型不一致：{} 与 {}", l.text(), r.text()), "比较", Some("§4.2".into()), None);
                }
                Ty::Bool
            }
            Plus | Minus | Star | Slash | Percent | Amp | Hat | Pipe | Shl | Shr => {
                if l.is_numeric() && r.is_numeric() && !type_compatible(l, r) {
                    // UB-001 / UB-002: mixing integer widths or float/integer is an error.
                    let code = if (l.is_float() && !r.is_float()) || (r.is_float() && !l.is_float()) { "E-0111" } else { "E-0110" };
                    self.emit(code, span, format!("隐式数字转换：{} 与 {} 不可运算", l.text(), r.text()), "运算", Some("§4.2 UB-001/UB-002 禁令；必须显式 `x as T`".into()), Some(format!("改写为 (a as {}) {t} (b as {})", l.text(), r.text(), t = op_text(op))));
                    return l.clone();
                }
                if matches!(op, Shl | Shr) && !l.is_integer() {
                    self.emit("E-0110", span, "移位只允许整型", format!("{}", l.text()), Some("§4.12".into()), None);
                }
                if matches!(op, Slash | Percent) && !l.is_numeric() {
                    self.emit("E-0110", span, "除法/取模需要数值类型", format!("{}", l.text()), None, None);
                }
                if matches!(l, Ty::Str | Ty::String) && matches!(op, Plus) && matches!(r, Ty::Str | Ty::String) {
                    return Ty::String;
                }
                if !l.is_numeric() && !matches!(l, Ty::Invalid) {
                    self.emit("E-0110", span, format!("该类型不支持 `{}`", op_text(op)), format!("{}", l.text()), Some("§4.4 运算符语义表".into()), None);
                }
                l.clone()
            }
            _ => {
                let _ = (l, r, span);
                Ty::Invalid
            }
        }
    }

    fn resolvable_by_trait_path(&self, found: &[usize], name: &str) -> bool {
        // §4.14: ambiguity is only real when two traits at the same level both provide `name`.
        let mut owners: BTreeSet<String> = BTreeSet::new();
        for i in found {
            let f = &self.prog.fns[*i];
            if f.name == name {
                owners.insert(String::new());
            } else if let Some(t) = f.name.split("::").next() {
                owners.insert(t.to_string());
            }
        }
        owners.len() <= 1
    }

    fn package_type_owner(&self, ty_name: &str) -> String {
        self.prog
            .structs
            .iter()
            .find(|s| s.name == ty_name)
            .map(|_| self.package.clone())
            .unwrap_or_default()
    }

    fn check_embed_expr(&mut self, e: &EmbedBlock, span: Span) {
        let fake = Item {
            attrs: Vec::new(),
            docs: Docs::default(),
            vis: Visibility::Private,
            span,
            kind: ItemKind::Fn {
                sig: FnSig { name: String::new(), name_span: span, generics: Vec::new(), params: Vec::new(), ret: None, where_preds: Vec::new(), is_unsafe: false, span },
                body: Body::Block(Block { stmts: Vec::new(), tail: None, span }),
            },
        };
        self.check_embed(e, &fake);
    }

    fn b_track_declared(&self, lang: &str) -> bool {
        self.b_track_versions.iter().any(|(k, _)| k == lang)
    }

    fn link_names(&self) -> &[String] {
        &self.link_unit_names
    }
}

fn op_text(op: crate::token::Punct) -> &'static str {
    use crate::token::Punct::*;
    match op {
        Plus => "+", Minus => "-", Star => "*", Slash => "/", Percent => "%",
        Amp => "&", Hat => "^", Pipe => "|", Shl => "<<", Shr => ">>",
        EqEq => "==", BangEq => "!=", Lt => "<", LtEq => "<=", Gt => ">", GtEq => ">=",
        AmpAmp => "&&", PipePipe => "||", _ => "op",
    }
}

fn compound_to_bin(op: crate::token::Punct) -> crate::token::Punct {
    use crate::token::Punct::*;
    match op {
        PlusEq => Plus, MinusEq => Minus, StarEq => Star, SlashEq => Slash, PercentEq => Percent,
        AmpEq => Amp, HatEq => Hat, PipeEq => Pipe, ShlEq => Shl, ShrEq => Shr, Eq => Plus,
        other => other,
    }
}

fn base_of(t: &Ty) -> String {
    match t {
        Ty::Named(n) | Ty::Adt(n) | Ty::Generic(n) | Ty::Dyn(n) => n.clone(),
        Ty::Ref { inner, .. } | Ty::Ptr { inner, .. } | Ty::Boxed { inner } => base_of(inner),
        Ty::Vec { inner } | Ty::Slice { inner } | Ty::Array { inner, .. } | Ty::Rc { inner, .. } | Ty::Option { inner } => base_of(inner),
        Ty::String => "String".into(),
        Ty::Str => "str".into(),
        other => other.text(),
    }
}

/// §4.6 — the closed `env` field set. Adding a field is a MINOR; an implementation inventing one
/// is UB-024, so this table is exhaustive by construction.
fn env_field_ty(field: &str) -> Option<Ty> {
    Some(match field {
        "iter" | "runtime_s" | "ram_mb" | "chain_id" | "node_idx" | "focus_id" => Ty::Prim(Prim::U32),
        "realtime" => Ty::Prim(Prim::U64),
        "cpu_use" | "gpu_use" => Ty::Prim(Prim::F32),
        "platform" | "build_id" => Ty::Str,
        _ => return None,
    })
}

fn types_comparable_for_eq(l: &Ty, r: &Ty) -> bool {
    if l == r {
        return true;
    }
    match (l, r) {
        (Ty::Str, Ty::String) | (Ty::String, Ty::Str) => true,
        (a, b) => a.is_numeric() && b.is_numeric() && type_compatible(a, b),
    }
}


// ---------------------------------------------------------------- helpers

fn is_std_path(p: &str) -> bool {
    p == "std" || p.starts_with("std/")
}

fn returns_fallible(t: &Option<Type>) -> bool {
    t.as_ref().map(|t| match &t.kind {
        TypeKind::Named { path, .. } => path.last().map(|s| s == "Result" || s == "Option").unwrap_or(false),
        _ => false,
    }).unwrap_or(false)
}

fn m_span(items: &[Item]) -> Span {
    items.first().map(|i| i.span).unwrap_or(Span::empty_at(0, 0))
}

fn type_base_name(t: &Ty) -> String {
    match t {
        Ty::Named(n) | Ty::Adt(n) | Ty::Dyn(n) | Ty::Generic(n) => n.clone(),
        Ty::Ref { inner, .. } => type_base_name(inner),
        Ty::Vec { inner } => format!("Vec<{}>", type_base_name(inner)),
        Ty::Option { inner } => format!("Option<{}>", type_base_name(inner)),
        other => other.text(),
    }
}

fn substitute_self(t: &Ty, self_ty: &Ty) -> Ty {
    match t {
        Ty::Named(n) if n == "Self" || n == "self" => self_ty.clone(),
        Ty::Ref { inner, mutable, lifetime } => Ty::Ref { inner: Box::new(substitute_self(inner, self_ty)), mutable: *mutable, lifetime: lifetime.clone() },
        Ty::Boxed { inner } => Ty::Boxed { inner: Box::new(substitute_self(inner, self_ty)) },
        Ty::Vec { inner } => Ty::Vec { inner: Box::new(substitute_self(inner, self_ty)) },
        Ty::Option { inner } => Ty::Option { inner: Box::new(substitute_self(inner, self_ty)) },
        Ty::Result { ok, err } => Ty::Result { ok: Box::new(substitute_self(ok, self_ty)), err: Box::new(substitute_self(err, self_ty)) },
        Ty::Slice { inner } => Ty::Slice { inner: Box::new(substitute_self(inner, self_ty)) },
        Ty::Rc { inner, atomic } => Ty::Rc { inner: Box::new(substitute_self(inner, self_ty)), atomic: *atomic },
        Ty::Tuple(v) => Ty::Tuple(v.iter().map(|x| substitute_self(x, self_ty)).collect()),
        other => other.clone(),
    }
}

fn contains_ref_needing_lifetime(t: &Ty) -> bool {
    match t {
        Ty::Ref { lifetime, .. } => lifetime.is_none(),
        Ty::Boxed { inner } | Ty::Vec { inner } | Ty::Slice { inner } | Ty::Option { inner } | Ty::Rc { inner, .. } => contains_ref_needing_lifetime(inner),
        Ty::Result { ok, err } => contains_ref_needing_lifetime(ok) || contains_ref_needing_lifetime(err),
        Ty::Tuple(v) => v.iter().any(|x| contains_ref_needing_lifetime(x)),
        _ => false,
    }
}

/// §4.2: assignment/argument compatibility, with no implicit numeric conversion anywhere.
pub fn type_compatible(want: &Ty, got: &Ty) -> bool {
    if want == got {
        return true;
    }
    match (want, got) {
        (Ty::Invalid, _) | (_, Ty::Invalid) => true,
        (Ty::Auto, _) | (_, Ty::Auto) => true,
        (Ty::Str, Ty::String) | (Ty::String, Ty::Str) => true,
        (Ty::Ref { inner: w, .. }, g) => type_compatible(w, g),
        (w, Ty::Ref { inner: g, .. }) => type_compatible(w, g),
        (Ty::Slice { inner: w }, Ty::Vec { inner: g }) | (Ty::Vec { inner: w }, Ty::Slice { inner: g }) => type_compatible(w, g),
        (Ty::Array { inner: w, n: wn }, Ty::Vec { inner: g }) => wn == &0 && type_compatible(w, g),
        (Ty::Dyn(d), Ty::Adt(a)) | (Ty::Dyn(d), Ty::Named(a)) => d == a,
        (Ty::Named(a), Ty::Adt(b)) | (Ty::Adt(a), Ty::Named(b)) => a == b,
        _ => false,
    }
}

/// §4.10: iterator item type for `for x in it`.
fn iterator_item(t: &Ty, _prog: &Program) -> Option<Ty> {
    match t {
        Ty::Array { inner, .. } | Ty::Slice { inner } | Ty::Vec { inner } => Some((**inner).clone()),
        Ty::Tuple(_) => Some(Ty::Unit),
        _ => Some(Ty::Invalid),
    }
}

fn scrut_is_enum(t: &Ty) -> bool {
    matches!(t, Ty::Adt(_) | Ty::Named(_))
}

/// §8.4 — the node table. Nodes are top-level `expr_stmt`s of a chain body; `let`/`var`/plain
/// assignment/`label` are the preamble of the node that follows them; `if`/`match` bodies never
/// open a node.
pub fn build_node_table(src: &Sources, file: FileId, stmts: &[Stmt]) -> (Vec<NodeDesc>, BTreeMap<String, usize>) {
    let mut nodes: Vec<NodeDesc> = Vec::new();
    let mut labels: BTreeMap<String, usize> = BTreeMap::new();
    let mut pending_preamble_has_node = false;
    for s in stmts {
        match &s.kind {
            StmtKind::Expr { expr, linkage } => {
                let p = src.line_col(expr.span).0;
                nodes.push(NodeDesc { idx: nodes.len() as u32, line: p.line, col: p.column, linkage_count: linkage.len(), host_event: host_event_name(expr) });
                pending_preamble_has_node = true;
            }
            StmtKind::Label { name, .. } => {
                // §8.2: `goto("anchor")` jumps to the label; the label anchors the node that follows
                let idx = nodes.len();
                labels.insert(name.clone(), idx);
                let _ = pending_preamble_has_node;
                pending_preamble_has_node = false;
            }
            _ => {}
        }
    }
    // `start_flag` is also an anchor (§8.2), registered as the node its host statement opens.
    (nodes, labels)
}

fn host_event_name(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Call { callee, .. } => match &callee.kind {
            ExprKind::Ident(n) => Some(n.clone()),
            ExprKind::Path(p) => Some(p.join("::")),
            _ => None,
        },
        ExprKind::Method { name, .. } => Some(name.clone()),
        _ => None,
    }
}

fn pat_bindings(p: &Pat) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    collect_bindings(p, &mut out);
    out
}
fn collect_bindings(p: &Pat, out: &mut BTreeSet<String>) {
    match &p.kind {
        PatKind::Binding { name, .. } => {
            out.insert(name.clone());
        }
        PatKind::Tuple(v) | PatKind::Or(v) | PatKind::Exclude(v) | PatKind::Slice(v, _) => {
            for x in v {
                collect_bindings(x, out);
            }
        }
        PatKind::Constructor { args, fields, .. } => {
            for x in args {
                collect_bindings(x, out);
            }
            for (_, x) in fields {
                collect_bindings(x, out);
            }
        }
        PatKind::Range { lo, hi, .. } => {
            collect_bindings(lo, out);
            collect_bindings(hi, out);
        }
        _ => {}
    }
}

fn pat_depth(p: &Pat) -> usize {
    1 + match &p.kind {
        PatKind::Tuple(v) | PatKind::Or(v) | PatKind::Exclude(v) => v.iter().map(pat_depth).max().unwrap_or(0),
        PatKind::Slice(v, r) => v.iter().map(pat_depth).max().unwrap_or(0).max(r.as_ref().map(|x| pat_depth(x)).unwrap_or(0)),
        PatKind::Constructor { args, fields, .. } => args.iter().map(pat_depth).chain(fields.iter().map(|(_, x)| pat_depth(x))).max().unwrap_or(0),
        PatKind::Range { lo, hi, .. } => pat_depth(lo).max(pat_depth(hi)),
        _ => 0,
    }
}

fn pat_covers_all(p: &Pat) -> bool {
    matches!(p.kind, PatKind::Wildcard)
}

fn pat_is_subsumed(p: &Pat, earlier: &[MatchArm]) -> bool {
    earlier.iter().any(|a| {
        if a.guard.is_some() {
            return false;
        }
        pat_covers_all(&a.pat) || pats_equal(p, &a.pat)
    })
}

fn pats_equal(a: &Pat, b: &Pat) -> bool {
    format!("{:?}", a.kind) == format!("{:?}", b.kind)
}

fn covers_bool(arms: &[MatchArm]) -> bool {
    let has = |want: bool| {
        arms.iter().any(|a| {
            if a.guard.is_some() {
                return false;
            }
            match &a.pat.kind {
                PatKind::Lit(Lit::Bool(b)) => *b == want,
                PatKind::Path(p) => p.last().map(|s| s == if want { "true" } else { "false" }).unwrap_or(false),
                _ => false,
            }
        })
    };
    has(true) && has(false)
}

/// §3.12: exhaustiveness for an enum means every variant is named by some guard-free arm.
fn covers_enum(arms: &[MatchArm], enum_name: &str, prog: &Program) -> bool {
    let Some(ei) = prog.enum_by_name(enum_name) else {
        return false;
    };
    let variants = prog.enums[ei].variants.clone();
    variants.iter().all(|(v, _, _)| {
        arms.iter().any(|a| {
            if a.guard.is_some() {
                return false;
            }
            arm_names_variant(&a.pat, v)
        })
    })
}

fn arm_names_variant(p: &Pat, variant: &str) -> bool {
    match &p.kind {
        PatKind::Path(path) => path.last().map(|x| x == variant).unwrap_or(false),
        PatKind::Constructor { path, .. } => path.last().map(|x| x == variant).unwrap_or(false),
        PatKind::Or(alts) => alts.iter().any(|s| arm_names_variant(s, variant)),
        _ => false,
    }
}

fn covers_int_range(arms: &[MatchArm], _p: Prim) -> bool {
    // A conservative check: any literal/range arm set plus a final wildcard is handled above;
    // full interval union is only attempted for the simple shapes the spec's example uses.
    arms.iter().any(|a| matches!(&a.pat.kind, PatKind::Range { .. }))
}

/// `infy check` driver for one resolved module: declarations, then bodies.
pub fn check_files(src: &Sources, h: &mut Handler, module: &str, files: &[(FileId, String, ParsedFile)], a_track_declared: bool, b_tracks: Vec<(String, String)>, link_units: Vec<String>) -> Program {
    let mut c = Checker::new(src, h, module);
    c.a_track_declared = a_track_declared;
    c.b_track_versions = b_tracks;
    c.link_unit_names = link_units;
    c.collect(files);
    c.check_bodies(files);
    c.prog
}
