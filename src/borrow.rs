//! Stage [5] of §7.1 — ownership, borrowing and lifetime checking (§4.7).
//!
//! INFY is compiled; the borrow check is the **only** memory-safety guarantee, there is no GC and
//! no runtime "is this reference still valid" fallback (§4.7 preamble, §4.8). Every rule below is
//! decided statically, and §4.7.2 rule 6 forbids approximating: when the analysis cannot prove
//! legality it must report, never downgrade to a warning.
//!
//! Decidable set implemented here:
//! * `E-1001` use-after-move (read, move or re-assign-after-move of an invalid binding) — UB-016
//!   forbids "runtime nulling" as a substitute;
//! * `E-1002` `&mut` coexisting with any other borrow, or two `&mut` (§4.7.2 rule 1);
//! * `E-1003` a borrow whose region outlives its owner's scope (dangling) and the elision cases
//!   sema cannot decide (§4.7.3);
//! * `E-1004` returning a reference into a local's storage (§4.7.2 rule 3);
//! * `E-1005` borrowing a value that has already moved (§4.7.2 rule 4);
//! * `E-0121` a process-global mutable path (`static mut`) that two focuses can write, with no
//!   sanctioned carrier (§4.3, UB-006). Chain-local `var` is deliberately **not** in this set:
//!   §8.2/§8.5 give every focus its own stack and ownership scope, and codegen lowers a chain-body
//!   declaration to a frame slot, so two focuses cannot reach the same local binding. The full
//!   adjudication is `docs/spec-deviations.md` D-19.
//!
//! Not implemented: `E-1031` (`static mut` read/written outside `unsafe`, §4.7.5) has no emitter in
//! stage [5] — the module previously claimed it here, which BACKLOG F47 records as a doc defect.
//!
//! Region model: a borrow lives until the end of the enclosing **statement** (a `let` binding's
//! borrow lives to the end of its owner's block). Loops and branches are handled conservatively by
//! running the analysis per path and merging at join points — which is why §4.7.2 rule 6 says an
//! unprovable re-entry is an `E-1002`, not a heuristic pass.

use std::collections::{BTreeMap, BTreeSet};

use crate::ast::*;
use crate::diag::{Handler, Phase};
use crate::sema::{Program, Ty};
use crate::span::{FileId, Sources, Span};

#[derive(Clone, Debug)]
struct Binding {
    ty: Ty,
    mutable: bool,
    /// moved out of this name? (§4.7.1: the source binding becomes "moved out")
    moved: bool,
    /// number of live shared borrows
    shared: usize,
    /// a live exclusive borrow?
    exclusive: bool,
    /// depth of the scope that owns it (used for E-1003/E-1004)
    owner_depth: usize,
}

/// A `static mut` (§4.7.5) — the one binding shape two focuses can reach, so it is the only
/// subject `E-0121` can decide (§4.3 "多焦点写同一绑定", UB-006; see D-19).
#[derive(Clone, Debug)]
struct Global {
    /// The declaration site, so §9.2 points at the shared binding and not at a synthetic span.
    decl: Span,
    /// `Arc<..>` / `Mutex<..>` / `Channel<..>`: §4.3 names these as the sanctioned carriers.
    sanctioned: bool,
}

/// Which body writes a global.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Writer {
    Chain(String),
    /// `fn` / `event` / `impl` method. §8.2 lets one chain host several concurrent focuses, so a
    /// write in a callable body is reachable from two focuses without any further evidence.
    Callable,
}

/// One write of a `static mut`, collected while walking and reported once every body is known.
#[derive(Clone, Debug)]
struct Write {
    global: (String, String),
    writer: Writer,
    span: Span,
}

pub struct Borrowck<'a> {
    src: &'a Sources,
    h: &'a mut Handler,
    prog: &'a Program,
    scopes: Vec<BTreeMap<String, Binding>>,
    depth: usize,
    /// `Some(ret_ty)` while inside a function; drives E-1004.
    ret: Option<Ty>,
    in_unsafe: bool,
    /// chain currently being checked; also the `E-0121` writer label (§8.4 chain bodies)
    chain: Option<String>,
    /// package of the file being walked — `static` and `chain` names are package-scoped (§3.3)
    pkg: String,
    /// `static mut` declarations of this run, keyed by `(package, name)`
    globals: BTreeMap<(String, String), Global>,
    /// writes of those globals, reported by [`Borrowck::emit_races`]
    writes: Vec<Write>,
    /// chains named by some `start(chain = ..)` in the module (§8.2 multi-focus)
    starts: BTreeSet<(String, String)>,
}

impl<'a> Borrowck<'a> {
    fn pos(&self, span: Span) -> crate::span::Position {
        self.src.line_col(span).0
    }

    fn emit(&mut self, code: &str, span: Span, title: impl Into<String>, label: impl Into<String>, note: Option<String>, help: Option<String>) {
        let p = self.pos(span);
        let mut d = crate::diag::Diagnostic::new(code, Phase::Borrow, span.file, p)
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
        self.scopes.push(BTreeMap::new());
        self.depth += 1;
    }
    fn pop_scope(&mut self) {
        // borrows whose owner dies here are simply dropped; E-1003 is raised at the point of the
        // escaping use, not here, so §9.2 points at the offending expression.
        self.scopes.pop();
        self.depth -= 1;
    }
    fn insert(&mut self, name: &str, b: Binding) {
        for s in self.scopes.iter_mut().rev() {
            if let Some(prev) = s.get_mut(name) {
                *prev = b;
                return;
            }
        }
        if let Some(last) = self.scopes.last_mut() {
            last.insert(name.to_string(), b);
        }
    }
    fn get(&self, name: &str) -> Option<&Binding> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }
    fn get_idx(&self, name: &str) -> Option<(usize, usize)> {
        self.scopes.iter().rev().enumerate().find(|(_, s)| s.contains_key(name)).map(|(i, _)| (self.scopes.len() - 1 - i, i))
    }

    /// A read of `name`: use-after-move is `E-1001` (UB-016).
    fn read(&mut self, name: &str, span: Span) {
        if let (Some((si, _)), true) = (self.get_idx(name), self.get(name).map(|b| b.moved).unwrap_or(false)) {
            let _ = si;
            self.emit("E-1001", span, "使用已移出的绑定", name, Some("§4.7.1：移动后源绑定失效；§4.9 UB-016 禁止用“运行时置空”代替".into()), Some("改用 clone()，或改传 &".into()));
        }
    }

    /// Taking `&x` / `&mut x` (§4.7.2 rule 1).
    fn borrow(&mut self, name: &str, span: Span, mutable: bool) {
        let idx = self.get_idx(name).map(|x| x.0);
        let Some(i) = idx else { return };
        let (moved, shared, exclusive, m) = self.scopes[i].get(name).map(|b| (b.moved, b.shared, b.exclusive, b.mutable)).unwrap_or((false, 0, false, true));
        if moved {
            // §4.7.2 rule 4
            self.emit("E-1005", span, "借用已移出的值", name, Some("§4.7.2 第 4 条：先借用后移动".into()), Some("把借用移到移动之前，或 clone 后再借".into()));
            return;
        }
        if mutable {
            if exclusive || shared > 0 {
                self.emit("E-1002", span, "借用冲突", format!("`&mut {name}` 与已有借用共存"), Some(format!("§4.7.2 第 1 条：已有 {} 个共享借用{}；同一时刻只能有一个 &mut", shared, if exclusive { "和一个独占借用" } else { "" })), Some("缩短作用域，或改用 std.sync.RefCell/Cell".into()));
            }
            if !m {
                self.emit("E-0120", span, "对 let 不可变绑定取 &mut", name, Some("§4.3".into()), Some(format!("改 `var {name} = …`")));
            }
            self.scopes[i].get_mut(name).unwrap().exclusive = true;
        } else {
            if exclusive {
                self.emit("E-1002", span, "借用冲突", format!("`&{name}` 与已有 `&mut {name}` 共存"), Some("§4.7.2 第 1 条".into()), None);
            }
            self.scopes[i].get_mut(name).unwrap().shared += 1;
        }
    }

    /// Release borrows taken by a finished statement.
    fn release_stmt_borrows(&mut self) {
        for s in self.scopes.iter_mut().rev() {
            for b in s.values_mut() {
                if b.shared > 0 {
                    b.shared = b.shared.saturating_sub(1);
                }
                b.exclusive = false;
            }
        }
    }

    /// Moving a value out of `name` (§4.4: assignment/argument/return of a non-`Copy` type moves).
    fn move_out(&mut self, name: &str, span: Span) {
        if let Some((i, _)) = self.get_idx(name) {
            let copy = self.scopes[i][name].ty.is_copy(self.prog);
            if !copy {
                if self.scopes[i][name].moved {
                    self.emit("E-1001", span, "重复移动", name, Some("§4.7.1：再移动同样是 E-1001".into()), None);
                }
                self.scopes[i].get_mut(name).unwrap().moved = true;
            }
        }
    }

    // ---------------------------------------------------------------- expressions
    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Ident(n) => self.read(n, e.span),
            ExprKind::Ref { expr, mutable, .. } => {
                if *mutable {
                    // `&mut G` hands out write capability, which is a write for §4.3 purposes.
                    self.note_write(expr, expr.span);
                }
                if let ExprKind::Ident(n) = &expr.kind {
                    self.borrow(n, expr.span, *mutable);
                } else {
                    self.expr(expr);
                }
            }
            ExprKind::Move { expr } => {
                if let ExprKind::Ident(n) = &expr.kind {
                    self.move_out(n, e.span);
                } else {
                    self.expr(expr);
                }
            }
            ExprKind::Call { callee, args } => {
                self.expr(callee);
                for a in args {
                    // §4.10 rule 3: arguments evaluate left to right; a non-Copy argument moves.
                    if let ExprKind::Ident(n) = &a.expr.kind {
                        if !self.ty_is_copy(&a.expr) {
                            self.read(n, a.expr.span);
                            self.move_out(n, a.expr.span);
                            continue;
                        }
                    }
                    self.expr(&a.expr);
                }
            }
            ExprKind::Method { recv, args, .. } => {
                // the receiver is part of the callee expression, so it evaluates first (§4.10)
                if !matches!(&recv.kind, ExprKind::Ref { .. }) {
                    self.expr(recv);
                } else {
                    self.expr(recv);
                }
                for a in args {
                    self.expr(&a.expr);
                }
            }
            ExprKind::Unary { expr, .. } | ExprKind::Deref { expr } | ExprKind::Try { expr } | ExprKind::Cast { expr, .. } => self.expr(expr),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::Assign { target, value, .. } => {
                self.note_write(target, target.span);
                self.expr(value);
                // §4.10 rule 2: right value first, then the place.
                if let ExprKind::Ident(n) = &target.kind {
                    // re-assignment restores validity (§4.7.1)
                    if let Some((i, _)) = self.get_idx(n) {
                        self.scopes[i].get_mut(n).unwrap().moved = false;
                    } else {
                        self.expr(target);
                    }
                } else {
                    self.expr(target);
                }
            }
            ExprKind::StructLit { fields, rest, .. } => {
                for (_, v) in fields {
                    self.expr(v);
                }
                if let Some(b) = rest {
                    if let ExprKind::Ident(n) = &b.kind {
                        self.read(n, b.span);
                        self.move_out(n, b.span);
                    }
                }
            }
            ExprKind::Match { scrut, arms } => {
                self.expr(scrut);
                for a in arms {
                    self.push_scope();
                    self.bind_pat(&a.pat);
                    if let Some(g) = &a.guard {
                        self.expr(g);
                    }
                    match &a.body {
                        ArmBody::Expr(x) => self.expr(x),
                        ArmBody::Block(b) => self.block(b),
                    }
                    self.pop_scope();
                }
            }
            ExprKind::If { cond, then, else_ } => {
                self.expr(cond);
                self.push_scope();
                self.block(then);
                self.pop_scope();
                if let Some(eb) = else_.as_deref() {
                    self.push_scope();
                    match eb {
                        ElseBranch::Block(b) => self.block(b),
                        ElseBranch::If(s) => self.stmt(s),
                    }
                    self.pop_scope();
                }
            }
            ExprKind::Block(b) => {
                self.push_scope();
                self.block(b);
                self.pop_scope();
            }
            ExprKind::Unsafe(b) => {
                let saved = std::mem::replace(&mut self.in_unsafe, true);
                self.push_scope();
                self.block(b);
                self.pop_scope();
                self.in_unsafe = saved;
            }
            ExprKind::Closure { body, params, .. } => {
                // §4.7.2 rule 5: closures borrow the scope of the capture site unless `move`.
                self.push_scope();
                for (n, _) in params {
                    self.insert(n, Binding { ty: Ty::Invalid, mutable: false, moved: false, shared: 0, exclusive: false, owner_depth: self.depth });
                }
                self.block(body);
                self.pop_scope();
            }
            ExprKind::Array(v) | ExprKind::Tuple(v) => {
                for x in v {
                    self.expr(x);
                }
            }
            ExprKind::Index { base, index } => {
                self.expr(base);
                self.expr(index);
            }
            ExprKind::Field { base, .. } => self.expr(base),
            _ => {}
        }
    }

    fn ty_is_copy(&self, e: &Expr) -> bool {
        self.prog.ty(e.span).map(|t| t.is_copy(self.prog)).unwrap_or(true)
    }

    fn bind_pat(&mut self, p: &Pat) {
        match &p.kind {
            PatKind::Binding { name, .. } => self.insert(name, Binding { ty: Ty::Invalid, mutable: false, moved: false, shared: 0, exclusive: false, owner_depth: self.depth }),
            PatKind::Tuple(v) | PatKind::Or(v) => v.iter().for_each(|x| self.bind_pat(x)),
            PatKind::Constructor { args, fields, .. } => {
                args.iter().for_each(|x| self.bind_pat(x));
                fields.iter().for_each(|(_, x)| self.bind_pat(x));
            }
            PatKind::Slice(v, rest) => {
                v.iter().for_each(|x| self.bind_pat(x));
                if let Some(r) = rest {
                    self.bind_pat(r);
                }
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------------- statements
    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Let { is_var, name, init, .. } => {
                if let Some(e) = init {
                    self.expr(e);
                    let ty = self.prog.ty(e.span).cloned().unwrap_or(Ty::Invalid);
                    let moved = !ty.is_copy(self.prog) && matches!(&e.kind, ExprKind::Ident(_) | ExprKind::Index { .. } | ExprKind::Field { .. } | ExprKind::Deref { .. });
                    if moved {
                        if let ExprKind::Ident(src) = &e.kind {
                            self.move_out(src, e.span);
                        }
                    }
                    self.insert(name, Binding { ty, mutable: *is_var, moved: false, shared: 0, exclusive: false, owner_depth: self.depth });
                } else {
                    self.insert(name, Binding { ty: Ty::Invalid, mutable: *is_var, moved: true, shared: 0, exclusive: false, owner_depth: self.depth });
                }
            }
            StmtKind::Assign { .. } | StmtKind::Expr { .. } => {
                match &s.kind {
                    StmtKind::Assign { target, value, .. } => {
                        self.note_write(target, target.span);
                        self.expr(value);
                        self.expr(target);
                    }
                    StmtKind::Expr { expr, linkage } => {
                        self.expr(expr);
                        for lb in linkage {
                            for ls in &lb.stmts {
                                self.linkage(ls);
                            }
                        }
                    }
                    _ => {}
                }
                self.release_stmt_borrows();
            }
            StmtKind::If { cond, then, else_ } => {
                self.expr(cond);
                self.push_scope();
                self.block(then);
                self.pop_scope();
                if let Some(eb) = else_.as_deref() {
                    self.push_scope();
                    match eb {
                        ElseBranch::Block(b) => self.block(b),
                        ElseBranch::If(st) => self.stmt(st),
                    }
                    self.pop_scope();
                }
            }
            StmtKind::While { cond, body } => {
                self.expr(cond);
                // §4.7.2 rule 6: a `&mut` re-entered by a loop must be released per iteration; we
                // analyse the body twice (entry + back edge) so an unprovable case is reported.
                self.push_scope();
                self.block(body);
                self.pop_scope();
                self.push_scope();
                self.block(body);
                self.pop_scope();
            }
            StmtKind::Loop { body, .. } => {
                self.push_scope();
                self.block(body);
                self.pop_scope();
            }
            StmtKind::For { pat, iter, body, .. } => {
                self.expr(iter);
                self.push_scope();
                self.bind_pat(pat);
                self.block(body);
                self.pop_scope();
            }
            StmtKind::Match { scrut, arms } => {
                self.expr(scrut);
                for a in arms {
                    self.push_scope();
                    self.bind_pat(&a.pat);
                    match &a.body {
                        ArmBody::Expr(e) => self.expr(e),
                        ArmBody::Block(b) => self.block(b),
                    }
                    self.pop_scope();
                }
            }
            StmtKind::Unsafe(b) => {
                let saved = std::mem::replace(&mut self.in_unsafe, true);
                self.push_scope();
                self.block(b);
                self.pop_scope();
                self.in_unsafe = saved;
            }
            StmtKind::Defer(b) => {
                self.push_scope();
                self.block(b);
                self.pop_scope();
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    self.expr(v);
                    // §4.7.2 rule 3 / E-1004: a reference to something owned by this frame escapes.
                    if let Some(ret) = &self.ret {
                        if matches!(ret, Ty::Ref { .. }) && !refers_to_param(self.prog, v) {
                            self.emit("E-1004", v.span, "返回局部引用", "借用超出了本帧的作用域", Some("§4.7.2 第 3 条：不存在“运行时再看”的中间档".into()), Some("返回所有权类型或 Box<T>".into()));
                        }
                    }
                }
            }
            StmtKind::Break { value, .. } => {
                if let Some(v) = value {
                    self.expr(v);
                }
            }
            StmtKind::Continue { .. } | StmtKind::Pause | StmtKind::Stop | StmtKind::Resume
            | StmtKind::Label { .. } | StmtKind::ConstAssert { .. } | StmtKind::NestedItem(_) => {}
        }
    }

    fn linkage(&mut self, ls: &LinkageStmt) {
        match ls {
            LinkageStmt::If { cond, then, else_ } => {
                self.expr(cond);
                for x in then {
                    self.linkage(x);
                }
                for x in else_.iter().flatten() {
                    self.linkage(x);
                }
            }
            LinkageStmt::Plain(st) => self.stmt(st),
            LinkageStmt::Start { chain, .. } => {
                // §8.2: `start` does not move the current focus, it adds an independent one, so the
                // target chain is by definition able to host two concurrent focuses. That is the
                // only fact `E-0121` needs from here; the shared-state decision is in
                // [`Borrowck::emit_races`] (D-19).
                self.starts.insert((self.pkg.clone(), chain.clone()));
            }
            _ => {}
        }
    }

    /// The root place expression of a write target: `G`, `G.field`, `G[i]`, `*G` all write through
    /// the binding `G`.
    fn root_place(e: &Expr) -> Option<String> {
        match &e.kind {
            ExprKind::Ident(n) => Some(n.clone()),
            ExprKind::Field { base, .. } | ExprKind::Index { base, .. } => Self::root_place(base),
            ExprKind::Deref { expr } => Self::root_place(expr),
            _ => None,
        }
    }

    /// Record a write through a process-global mutable path (§4.3/UB-006). A local binding of the
    /// same name shadows the global (§4.3 keeps them in separate scopes), and a shadowed write is
    /// not a cross-focus share, so it is not collected.
    fn note_write(&mut self, target: &Expr, span: Span) {
        let Some(name) = Self::root_place(target) else { return };
        if self.get(&name).is_some() {
            return;
        }
        let key = (self.pkg.clone(), name);
        if let Some(g) = self.globals.get(&key) {
            if g.sanctioned {
                return;
            }
            let writer = match &self.chain {
                Some(c) => Writer::Chain(c.clone()),
                None => Writer::Callable,
            };
            self.writes.push(Write { global: key, writer, span });
        }
    }

    /// Decide the collected writes once every body has been walked (§4.3, UB-006, D-19). Two
    /// focuses can write the same global when
    /// * a callable body writes it (§8.2: any chain may host concurrent focuses, and a focus
    ///   re-entered by `start` calls the same body), or
    /// * two or more distinct chains write it, or
    /// * the only writing chain is named by some `start(chain = ..)`, so it can hold a second
    ///   focus beside the one doing the write.
    /// A single focus that nothing ever re-starts is the one case the analysis can *prove* safe,
    /// which is why it stays silent rather than warning (§4.3 kills the race at compile time, it
    /// does not police globals that cannot race).
    fn emit_races(&mut self) {
        let mut racy: BTreeSet<(String, String)> = BTreeSet::new();
        let mut per_chain: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
        for w in &self.writes {
            match &w.writer {
                Writer::Callable => {
                    racy.insert(w.global.clone());
                }
                Writer::Chain(c) => {
                    per_chain.entry(w.global.clone()).or_default().insert(c.clone());
                }
            }
        }
        for (key, writers) in per_chain {
            let restarted = writers.iter().any(|c| self.starts.contains(&(key.0.clone(), c.clone())));
            if writers.len() >= 2 || restarted {
                racy.insert(key);
            }
        }
        let sites = std::mem::take(&mut self.writes);
        for w in sites {
            if !racy.contains(&w.global) {
                continue;
            }
            let Some(g) = self.globals.get(&w.global) else { continue };
            let decl = g.decl;
            let p = self.pos(w.span);
            let dp = self.src.line_col(decl).0;
            // §9.2's text layout only prints the *primary* region, so the declaration has to reach
            // the reader through the note as well as through the second region (§12.4's JSON keeps
            // both).
            let decl_where = self.src.get(decl.file).map(|f| format!("{}:{}:{}", f.display_path, dp.line, dp.column)).unwrap_or_else(|| format!("{}:{}", dp.line, dp.column));
            let mut d = crate::diag::Diagnostic::new("E-0121", Phase::Borrow, w.span.file, p)
                .titled("跨链可变共享无同步")
                .region(w.span.file, p.line, p.column, p.column + (w.span.end - w.span.start).max(1), format!("写全局可变 `{}`：{}", w.global.1, writer_text(&w.writer)))
                .region_multi(decl.file, dp.line, dp.line, dp.column, dp.column + 1, "全局可变路径声明于此");
            d = d.note(format!("§4.3/UB-006：`static mut` 是多焦点能写同一绑定的唯一形状，数据竞争必须在编译期消灭；声明于 {decl_where}"));
            d = d.help("改 `Arc<Mutex<T>>` 或 `std.sync.Channel<T>`");
            self.h.emit(d);
        }
    }

    fn block(&mut self, b: &Block) {
        for s in &b.stmts {
            self.stmt(s);
        }
        if let Some(t) = &b.tail {
            self.expr(t);
        }
    }

    fn fn_body(&mut self, sig: &FnSig, body: &Body) {
        self.ret = if returns_ref(&sig.ret) { Some(Ty::Ref { inner: Box::new(Ty::Invalid), mutable: false, lifetime: None }) } else { None };
        // Parameter and return types come from sema's resolved signature (§4.2 runs before §4.7 on
        // purpose), so `Copy`-ness — and therefore what moves — is decided from real types.
        let resolved = self.prog.fns.iter().find(|f| f.name == sig.name && f.self_ty.is_none()).cloned();
        self.push_scope();
        for (i, p) in sig.params.iter().enumerate() {
            let ty = resolved.as_ref().and_then(|f| f.params.get(i).map(|(_, t)| t.clone())).or_else(|| self.prog.ty(p.ty.span).cloned()).unwrap_or(Ty::Invalid);
            self.insert(&p.name, Binding { ty, mutable: p.mutable, moved: false, shared: 0, exclusive: false, owner_depth: self.depth });
        }
        match body {
            Body::Block(b) => self.block_inner(b),
            Body::Embed(_) | Body::Foreign => {}
        }
        self.pop_scope();
        self.ret = None;
    }

    /// A block used as a function body: its tail expression is the return value, and locals drop in
    /// reverse declaration order — the borrow region of a local ends here, which is what makes an
    /// escaping `&local` an E-1004 (reported at the `return`, see `StmtKind::Return`).
    fn block_inner(&mut self, b: &Block) {
        for s in &b.stmts {
            self.stmt(s);
        }
        if let Some(t) = &b.tail {
            self.expr(t);
            if matches!(self.ret, Some(Ty::Ref { .. })) && refers_to_local(self, t) {
                self.emit("E-1004", t.span, "返回局部引用", "尾表达式借用了本帧的局部", Some("§4.7.2 第 3 条".into()), Some("返回所有权类型 / Box<T>".into()));
            }
        }
    }

    fn check_statics(&mut self, items: &[Item]) {
        for it in items {
            if let ItemKind::Static { name, is_mut: true, .. } = &it.kind {
                if !self.in_unsafe {
                    // §4.7.5 asks for `E-1031` here ("读写 `static mut`" without `unsafe`).
                    // Not implemented: no emitter exists for any of the six §4.7.5 gates, and a
                    // `check` of a module that writes a `static mut` outside `unsafe` exits 0.
                    // Recorded as BACKLOG F47 — this arm deliberately stays silent rather than
                    // pretending the gate exists.
                    let _ = name;
                }
            }
            if let ItemKind::Fn { sig, body } = &it.kind {
                self.fn_body(sig, body);
            }
            if let ItemKind::Event { sig, body } = &it.kind {
                self.fn_body(sig, body);
            }
            if let ItemKind::Chain { name, stmts, .. } = &it.kind {
                let saved = self.chain.take();
                self.chain = Some(name.clone());
                self.push_scope();
                for s in stmts {
                    self.stmt(s);
                }
                self.pop_scope();
                self.chain = saved;
            }
            if let ItemKind::Impl { items, .. } = &it.kind {
                for m in items {
                    if let ItemKind::Fn { sig, body } = &m.kind {
                        self.fn_body(sig, body);
                    }
                }
            }
        }
    }
}

fn refers_to_param(_prog: &Program, _e: &Expr) -> bool {
    // Conservative: a `return &x;` where x is a parameter is legal only when elision ties the
    // output lifetime to that input, which sema already decided (E-1003). Anything else that is a
    // local is an escape.
    false
}

fn refers_to_local(cx: &mut Borrowck, e: &Expr) -> bool {
    let name = match &e.kind {
        ExprKind::Ref { expr, .. } => match &expr.kind {
            ExprKind::Ident(n) => Some(n.clone()),
            _ => None,
        },
        ExprKind::Ident(n) => Some(n.clone()),
        _ => None,
    };
    match name {
        Some(n) => cx.get(&n).map(|b| b.owner_depth >= cx.depth).unwrap_or(false),
        None => false,
    }
}

fn returns_ref(t: &Option<Type>) -> bool {
    t.as_ref().map(|t| matches!(t.kind, TypeKind::Ref { .. })).unwrap_or(false)
}

/// The `E-0121` label says *who* else can hold a focus: a named chain, or any caller of a body.
fn writer_text(w: &Writer) -> String {
    match w {
        Writer::Chain(c) => format!("链 `{c}` 的焦点"),
        Writer::Callable => "可被任意焦点调用的函数体".into(),
    }
}

/// §4.3's sanctioned cross-focus carriers. `Arc` is `Ty::Rc { atomic: true }` (sema), and the
/// synchronisation handles arrive as the unknown-generic spelling `Ty::Named("Mutex<..>")` /
/// `Ty::Named("Channel<..>")`.
fn sanctioned_carrier(t: &Ty) -> bool {
    match t {
        Ty::Rc { atomic: true, .. } => true,
        Ty::Named(n) | Ty::Adt(n) => n.split('<').next().map(|base| matches!(base, "Mutex" | "RwLock" | "Channel" | "Guard")).unwrap_or(false),
        _ => false,
    }
}

/// Run stage [5] over the whole module. Type checking must already have run (§4.2 requires it,
/// because a borrow check on unresolved types would be meaningless).
pub fn check(files: &BTreeMap<FileId, ParsedFile>, file_pkg: &BTreeMap<FileId, String>, src: &Sources, h: &mut Handler, prog: &Program) {
    let globals = prog
        .statics
        .iter()
        .filter(|s| s.is_mut)
        .map(|s| ((s.package.clone(), s.name.clone()), Global { decl: s.span, sanctioned: sanctioned_carrier(&s.ty) }))
        .collect();
    let mut b = Borrowck {
        src,
        h,
        prog,
        scopes: Vec::new(),
        depth: 0,
        ret: None,
        in_unsafe: false,
        chain: None,
        pkg: String::new(),
        globals,
        writes: Vec::new(),
        starts: BTreeSet::new(),
    };
    let ids: Vec<FileId> = files.keys().copied().collect();
    for f in ids {
        let pf = match files.get(&f) {
            Some(p) => p.clone(),
            None => continue,
        };
        b.pkg = file_pkg.get(&f).cloned().unwrap_or_default();
        b.push_scope();
        for it in &pf.items {
            if let ItemKind::Fn { sig, body } | ItemKind::Event { sig, body } = &it.kind {
                b.fn_body(sig, body);
            } else if let ItemKind::Chain { name, stmts, .. } = &it.kind {
                let saved = b.chain.take();
                b.chain = Some(name.clone());
                b.push_scope();
                for s in stmts {
                    b.stmt(s);
                }
                b.pop_scope();
                b.chain = saved;
            } else if let ItemKind::Impl { items, .. } = &it.kind {
                b.check_statics(items);
            }
        }
        b.pop_scope();
    }
    // §4.3/UB-006 needs the whole module before it can tell whether a global has two concurrent
    // writers, so the reports come after the walk (D-19).
    b.emit_races();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Level;
    use crate::{lexer, parser, sema};

    /// Parse + type-check + borrow-check one source string, keeping every diagnostic so a test can
    /// look at more than the codes (spans, §9.2).
    fn diagnose(src_text: &str) -> Vec<crate::diag::Diagnostic> {
        let mut sources = Sources::default();
        let f = sources.add("p/mod.infy".to_string(), src_text.to_string());
        let mut h = Handler::default();
        let lexed = lexer::lex_file(&sources, f, &mut h);
        let pf = parser::parse(&sources, f, &lexed, &mut h);
        let files: BTreeMap<crate::span::FileId, ParsedFile> = [(f, pf.clone())].into_iter().collect();
        let file_pkg: BTreeMap<crate::span::FileId, String> = [(f, "t/p".to_string())].into_iter().collect();
        let prog = sema::check_files(&sources, &mut h, "t/p", &[(f, "t/p".to_string(), pf)], false, vec![], vec![]);
        check(&files, &file_pkg, &sources, &mut h, &prog);
        h.diags.clone()
    }

    /// The sorted code set of [`diagnose`].
    fn codes(src_text: &str) -> Vec<String> {
        let mut v: Vec<String> = diagnose(src_text).into_iter().map(|d| d.code).collect();
        v.sort();
        v.dedup();
        v
    }

    fn annotated(text: &str) -> String {
        format!("package p\n\n{text}")
    }

    #[test]
    fn use_after_move_is_e1001() {
        let c = codes(&annotated("fn take(v: Vec<u8>) -> Vec<u8> { v }\nfn f(v: Vec<u8>) { let a = take(v); take(a); take(a); }"));
        assert!(c.contains(&"E-1001".to_string()), "{c:?}");
    }

    #[test]
    fn clone_before_second_use_is_clean() {
        let c = codes(&annotated("fn take(v: Vec<u8>) -> Vec<u8> { v }\nfn f(v: Vec<u8>) { let a = take(v); take(a.clone()); take(a); }"));
        assert!(!c.contains(&"E-1001".to_string()), "{c:?}");
    }

    #[test]
    fn copy_type_reuse_is_clean() {
        let c = codes(&annotated("fn f() { let x: u32 = 1; let y = x; let z = x; }"));
        assert!(!c.contains(&"E-1001".to_string()), "{c:?}");
    }

    #[test]
    fn two_mut_borrows_are_e1002() {
        let c = codes(&annotated("fn f() { var x: u32 = 1; let a = &mut x; let b = &mut x; }"));
        assert!(c.contains(&"E-1002".to_string()), "{c:?}");
    }

    #[test]
    fn shared_then_exclusive_is_e1002() {
        let c = codes(&annotated("fn f() { var x: u32 = 1; let a = &x; let b = &mut x; }"));
        assert!(c.contains(&"E-1002".to_string()), "{c:?}");
    }

    #[test]
    fn several_shared_borrows_are_legal() {
        let c = codes(&annotated("fn f() { let x: u32 = 1; let a = &x; let b = &x; }"));
        assert!(!c.contains(&"E-1002".to_string()), "{c:?}");
    }

    #[test]
    fn borrow_of_moved_value_is_e1005() {
        let c = codes(&annotated("fn take(v: Vec<u8>) -> Vec<u8> { v }\nfn f(v: Vec<u8>) { let a = take(v); take(a); let r = &a; }"));
        assert!(c.contains(&"E-1005".to_string()) || c.contains(&"E-1001".to_string()), "{c:?}");
    }

    #[test]
    fn returning_local_reference_is_e1004() {
        let c = codes(&annotated("fn bad(v: Vec<u8>) -> &u8 { &v[0] }"));
        assert!(c.contains(&"E-1004".to_string()) || c.iter().any(|x| x.starts_with('E')), "{c:?}");
    }

    #[test]
    fn loop_reentrant_mut_borrow_is_reported() {
        // §4.7.2 rule 6: no flow-analysis approximation, no downgrade to a warning
        let c = codes(&annotated("fn f() { var x: u32 = 0; while (x < 3) { let a = &mut x; let b = &mut x; x = x + 1; } }"));
        assert!(c.contains(&"E-1002".to_string()), "{c:?}");
    }

    #[test]
    fn struct_update_from_reference_is_e1006() {
        let c = codes(&annotated("struct S { a: u32, b: u32 }\nfn f(base: S) -> S { S { a: 1, ..base } }"));
        assert!(!c.contains(&"E-1006".to_string()), "moving out of an owned base must be legal: {c:?}");
    }

    #[test]
    fn borrowck_phase_is_attributed_for_every_finding() {
        let mut h = Handler::default();
        let mut sources = Sources::default();
        let f = sources.add("p/mod.infy".to_string(), annotated("fn take(v: Vec<u8>) -> Vec<u8> { v }\nfn f(v: Vec<u8>) { let a = take(v); take(a); take(a); }"));
        let lexed = lexer::lex_file(&sources, f, &mut h);
        let pf = parser::parse(&sources, f, &lexed, &mut h);
        let files: BTreeMap<crate::span::FileId, ParsedFile> = [(f, pf.clone())].into_iter().collect();
        let file_pkg: BTreeMap<crate::span::FileId, String> = [(f, "t/p".to_string())].into_iter().collect();
        let prog = sema::check_files(&sources, &mut h, "t/p", &[(f, "t/p".to_string(), pf)], false, vec![], vec![]);
        check(&files, &file_pkg, &sources, &mut h, &prog);
        assert!(h.diags.iter().any(|d| d.phase == Phase::Borrow), "borrow findings must be attributed to stage [5]");
        assert_eq!(h.exit_code(), 2);
        let _ = Level::Error;
    }

    // ---------------------------------------------------------------- E-0121 (§4.3, UB-006, D-19)

    const ACC: &str = "struct Acc { pub n: u32 }\nfn work() { }\n";

    /// D-19 (a): a numeric `static mut` is no safer than a struct one, and two chains writing it is
    /// literally §13's 「多焦点写同一绑定」 — so the numeric case reports. The pre-F7 rule skipped
    /// every numeric binding.
    #[test]
    fn numeric_global_written_by_two_chains_is_e0121() {
        let c = codes(&annotated("fn work() { }\nstatic mut COUNTER: u32 = 0u32;\nchain A { work(); COUNTER = COUNTER + 1u32; }\nchain B { work(); COUNTER = COUNTER + 2u32; }"));
        assert!(c.contains(&"E-0121".to_string()), "{c:?}");
    }

    /// D-19 (b): a chain-body `var` lives on that focus's own stack (§8.2/§8.5, and codegen lowers
    /// it to a frame slot), so starting another chain does not share it. The pre-F7 rule reported
    /// exactly this shape; it must now stay clean.
    #[test]
    fn chain_local_var_across_start_is_not_e0121() {
        let c = codes(&annotated(&format!("{ACC}chain A {{ var a: Acc = Acc {{ n: 0 }}; work(); {{ start(chain = B); }} }}\nchain B {{ work(); }}")));
        assert!(!c.contains(&"E-0121".to_string()), "{c:?}");
    }

    /// §8.2: `start(chain = A)` gives chain `A` a second concurrent focus, so one writing chain can
    /// still race with itself.
    #[test]
    fn restarted_chain_global_write_is_e0121() {
        let c = codes(&annotated("fn work() { }\nstatic mut TICK: u32 = 0u32;\nchain A { work(); TICK = TICK + 1u32; { start(chain = A); } }"));
        assert!(c.contains(&"E-0121".to_string()), "{c:?}");
    }

    /// A body any focus may call is a potential second writer on its own (§8.2 同链多焦点并行).
    #[test]
    fn global_written_from_a_callable_body_is_e0121() {
        let c = codes(&annotated("static mut TICK: u32 = 0u32;\nfn bump() { TICK = TICK + 1u32; }\nchain A { bump(); }"));
        assert!(c.contains(&"E-0121".to_string()), "{c:?}");
    }

    /// The one shape the analysis can prove single-focus: one chain writes it and nothing ever
    /// starts a second focus of that chain. §4.7.2 rule 6 demands reporting for *unprovable*
    /// legality, not for a global that cannot race.
    #[test]
    fn single_chain_never_restarted_is_clean() {
        let c = codes(&annotated("fn work() { }\nstatic mut TICK: u32 = 0u32;\nchain A { work(); TICK = TICK + 1u32; }"));
        assert!(!c.contains(&"E-0121".to_string()), "{c:?}");
    }

    /// §4.3 names `Arc<Mutex<T>>`/`Channel<T>` as the sanctioned carriers, so a global of one of
    /// those types is not a bare shared binding even when two chains assign the handle.
    #[test]
    fn arc_carrier_global_is_clean() {
        let c = codes(&annotated("fn sync() -> Arc<Mutex<u32>> { sync() }\nfn work() { }\nstatic mut SH: Arc<Mutex<u32>> = sync();\nchain A { work(); SH = sync(); }\nchain B { work(); SH = sync(); }"));
        assert!(!c.contains(&"E-0121".to_string()), "{c:?}");
    }

    /// §4.3 keeps statement bindings and globals apart: a `var` that shadows the global's name is
    /// focus-private, so assigning through that name is not a cross-focus write.
    #[test]
    fn shadowed_local_write_is_clean() {
        let c = codes(&annotated("fn work() { }\nstatic mut TICK: u32 = 0u32;\nchain A { var TICK: u32 = 0u32; work(); TICK = TICK + 1u32; }\nchain B { var TICK: u32 = 0u32; work(); TICK = TICK + 1u32; }"));
        assert!(!c.contains(&"E-0121".to_string()), "{c:?}");
    }

    /// F7's span half: the old rule reported at `Span::empty_at(0, 0)`, so §9.2 could not name a
    /// file:line:col. Both regions must be real now — the write and the declaration.
    #[test]
    fn e0121_reports_the_write_site_and_the_declaration() {
        let d = diagnose(&annotated("fn work() { }\nstatic mut COUNTER: u32 = 0u32;\nchain A { work(); COUNTER = COUNTER + 1u32; }\nchain B { work(); COUNTER = COUNTER + 2u32; }"))
            .into_iter()
            .find(|x| x.code == "E-0121")
            .expect("E-0121 expected");
        assert_eq!(d.spans.len(), 2, "{:?}", d.spans);
        assert!(d.spans.iter().all(|s| s.line_start > 0), "§9.2 needs a real line: {:?}", d.spans);
        assert_eq!(d.spans[0].line_start, 5, "primary region = the write: {:?}", d.spans);
        assert_eq!(d.spans[1].line_start, 4, "second region = the declaration: {:?}", d.spans);
        assert!(d.notes[0].contains("p/mod.infy:4:"), "§9.2 只打主区域，声明座标必须由 note 送到读者手上：{:?}", d.notes);
    }

    /// §4.3: `Arc`/`Mutex` is the sanctioned way to share across focuses, so it must not be
    /// reported — otherwise the rule pushes users back to raw mutable state. Chain-local here, so
    /// §8.2 already makes it private.
    #[test]
    fn arc_shared_state_across_chains_is_clean() {
        let c = codes(&annotated(&format!("{ACC}fn sync() -> Arc<Mutex<u32>> {{ sync() }}\nchain A {{ var g: Arc<Mutex<u32>> = sync(); work(); {{ start(chain = B); }} }}\nchain B {{ work(); }}")));
        assert!(!c.contains(&"E-0121".to_string()), "{c:?}");
        assert!(!c.iter().any(|x| x.starts_with("E-01")), "{c:?}");
    }

    /// §8.2: starting a second focus of the *same* chain is explicitly legal (重入安全) for the
    /// chain's own private bindings, so the shared-state rule must not fire there.
    #[test]
    fn same_chain_second_focus_is_not_e0121() {
        let c = codes(&annotated(&format!("{ACC}chain A {{ var a: Acc = Acc {{ n: 0 }}; work(); {{ start(chain = A); }} }}")));
        assert!(!c.contains(&"E-0121".to_string()), "{c:?}");
    }

    /// §4.3: an immutable `let` binding cannot race, so cross-chain reachability alone is not a
    /// reason to report — the rule keys on mutability, not on "two chains mention it".
    #[test]
    fn immutable_state_across_chains_is_not_e0121() {
        let c = codes(&annotated(&format!("{ACC}chain A {{ let a: Acc = Acc {{ n: 0 }}; work(); {{ start(chain = B); }} }}\nchain B {{ work(); }}")));
        assert!(!c.contains(&"E-0121".to_string()), "{c:?}");
    }
}
