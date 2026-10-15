//! §7.6 A 轨（编译型语言嵌入）的自动连接 —— 绑定生成的**唯一真值源**。
//!
//! 规范把 A 轨的胶水全部判给编译器（§7.6.3“用户不得手写以下任何一项”），于是本模块负责三件事：
//!
//! 1. **配对**（§7.6.1）：`#[link(name = "…")] extern "C" { … }` 签名块 ↔ `@c(name = "…")` 源码块，
//!    两边由同一 `name` 绑定；任一方向落空 → `E-0720`。
//! 2. **边界映射**（§7.6.4 的封闭表 + UB-010）：把 INFY 侧签名翻成 C11 类型写法，并判出规范明令禁止
//!    的跨边界类型（`Rc<T>`、`Vec`/`String` 的容量与裸内存）与未声明的移交方式 → `E-0723`。
//! 3. **命名与产物文本**（§7.6.3 项 1/2）：`infy_binding.h` 与 `infy_e_<name>_<8hex>` 包装。
//!    `<8hex>` = 规范化签名块 SHA-256 的前 4 字节（`docs/internals/runtime-abi.md` §2）。
//!
//! 分工：`sema` 用判据发诊断，`codegen` 与 `cli` 从同一批函数取符号与文本 —— 头文件里的原型和
//! 生成码调用的符号必须来自一次计算，否则两侧各算一份哈希会在链接期才现形
//! （`docs/spec-deviations.md` D-46(a)）。
//! `E-0722`（清单缺 `[profile] a_track`）与 `E-0721`（调用缺 `unsafe`）留在阶段 [4]，
//! 因为它们是“程序能不能编译”的判据。

use crate::ast::{Attr, AttrValue, Body, EmbedBlock, Expr, ExprKind, Item, ItemKind, ParsedFile, Prim};
use crate::hir::{HirFn, HirItem};
use crate::sema::Ty;
use crate::span::{FileId, Span};
use crate::token::Lit;
use std::collections::BTreeMap;

/// §7.6.3 项 2 / ABI §2 的包装前缀。
pub const WRAPPER_PREFIX: &str = "infy_e_";

/// §7.6.1：`extern "C"` 是 A 轨**唯一**的稳定 ABI。
pub const SUPPORTED_ABIS: &[&str] = &["C"];

/// 只有这一半是“规范禁止”，另一半是“规范允许但还没发射” —— 两者都必须红，但消息要点名是哪一种。
#[derive(Clone, Debug, PartialEq)]
pub enum Boundary {
    /// §7.6.4 末条“禁止跨边界传递” → `E-0723`（不是待实现）。
    Forbidden(String, &'static str),
    /// §7.6.4 有这一行，本实现还没有可发射的形态 → 显式拒绝，不降级、不近似。
    Unsupported(String, &'static str),
}

impl Boundary {
    /// 禁止项落在 `E-0723`（§9.6 给 A 轨签名问题的唯一码）；还没发射的行没有码可挂 ——
    /// 它是"合法程序 + 未实现"，用条款号指名（`codegen::CgError` 的 `code` 就是这种字符串）。
    pub fn code(&self) -> &'static str {
        match self {
            Boundary::Forbidden(..) => "E-0723",
            Boundary::Unsupported(..) => "§7.6.4",
        }
    }
    pub fn text(&self) -> String {
        match self {
            Boundary::Forbidden(t, why) => format!("`{t}` 不得跨 A 轨边界：{why}"),
            Boundary::Unsupported(t, why) => format!("`{t}` 在 §7.6.4 里有对应行，但本实现还没有发射形态：{why}"),
        }
    }
}

/// 一条 A 轨判据（阶段 [4] 用它发诊断）。
#[derive(Clone, Debug)]
pub struct Gap {
    pub code: &'static str,
    pub title: String,
    pub label: String,
    pub note: Option<String>,
    pub help: Option<String>,
    pub span: Span,
}

impl Gap {
    fn new(code: &'static str, span: Span, title: impl Into<String>, label: impl Into<String>) -> Gap {
        Gap { code, title: title.into(), label: label.into(), note: None, help: None, span }
    }
    fn note(mut self, n: impl Into<String>) -> Gap {
        self.note = Some(n.into());
        self
    }
    fn help(mut self, h: impl Into<String>) -> Gap {
        self.help = Some(h.into());
        self
    }

    /// 发射侧（`codegen::CgError` / `cli`）只要一行文本，没有 span 可用。
    pub fn summary(&self) -> String {
        match &self.note {
            Some(n) => format!("{} · {}：{n}", self.code, self.label),
            None => format!("{} · {}", self.code, self.label),
        }
    }
}

// ------------------------------------------------------------------ §7.6.4 边界类型表

/// §7.6.4 行 1“同宽整型 / `double` / `uint8_t` / 32 位枚举”的值拷贝子集。
///
/// `bool` 写成 `uint8_t`、`char` 写成 `uint32_t` 是规范原文，而两者与 `runtime/infy_rt.h:41-42` 的
/// `InfyBool`（`uint8_t`）/ `InfyChar`（`uint32_t`）本就是同一类型，所以调用点不会出现类型不符。
/// `usize`/`isize` 保持 `codegen::prim_c` 对同一类型的写法（`size_t`/`ptrdiff_t`）—— §7.6.4 给切片
/// 长度写的是 `uint64_t`，而 §7.2 每个内建 target 上 `size_t` 都是 64 位无符号；两处拼写只留一份，
/// 见 `docs/spec-deviations.md` D-46(d)。
pub fn scalar_c(p: Prim) -> Option<&'static str> {
    Some(match p {
        Prim::U8 => "uint8_t",
        Prim::U16 => "uint16_t",
        Prim::U32 => "uint32_t",
        Prim::U64 => "uint64_t",
        Prim::I8 => "int8_t",
        Prim::I16 => "int16_t",
        Prim::I32 => "int32_t",
        Prim::I64 => "int64_t",
        Prim::F32 => "float",
        Prim::F64 => "double",
        Prim::Bool => "uint8_t",
        Prim::Char => "uint32_t",
        Prim::Usize => "size_t",
        Prim::Isize => "ptrdiff_t",
        // 128 位整数没有可移植的 C11 同宽类型；`str` 与 `()` 走下面的行。
        Prim::U128 | Prim::I128 | Prim::Str | Prim::Unit => return None,
    })
}

/// 指针的指涉对象。标量之外一律拒绝：`#[repr(C)]` struct 的不透明指针要等 `infy_binding.h` 能携带
/// 共享定义（§7.6.4 行 7），登记为 BACKLOG F31。
fn pointee_c(ty: &Ty) -> Result<String, Boundary> {
    match ty {
        Ty::Prim(p) => scalar_c(*p).map(|s| s.to_string()).ok_or_else(|| {
            Boundary::Unsupported(ty.text(), "指涉对象没有 C11 同宽类型")
        }),
        Ty::Bool => Ok("uint8_t".to_string()),
        other => Err(Boundary::Unsupported(
            other.text(),
            "指涉对象必须是标量；ADT 的不透明指针要先把 `#[repr(C)]` 的共享定义写进绑定头（行 7）",
        )),
    }
}

/// 一个 INFY 类型在跨边界处的 C 写法。
///
/// 一个 INFY 形参只产出一个 C 形参：`&[u8]`/`&str` 的 ptr+len 展开会占**两个**，阶段 [8] 还没有
/// 拆分实参的发射，所以按 `Unsupported` 拒绝而不是悄悄丢掉长度（丢长度 = 外部拿到越界指针）。
pub fn boundary_c(ty: &Ty) -> Result<String, Boundary> {
    match ty {
        Ty::Prim(p) => scalar_c(*p).map(|s| s.to_string()).ok_or_else(|| {
            Boundary::Unsupported(
                ty.text(),
                "行 1 只覆盖同宽整型 / `double` / `uint8_t` / 32 位枚举",
            )
        }),
        Ty::Bool => Ok("uint8_t".to_string()),
        Ty::Unit => Ok("void".to_string()),
        // `&[u8]` / `&str` 是行 2/3 的 ptr+len 展开，不是"指向切片的指针"：消息必须点正确的行，
        // 否则读者会以为只要给切片加个 `#[repr(C)]` 定义就能过。
        Ty::Ref { inner, .. } if matches!(**inner, Ty::Slice { .. }) => Err(Boundary::Unsupported(
            ty.text(),
            "行 2 要摊成 `ptr, len` 两个 C 形参，阶段 [8] 还没有切片实参的拆分发射",
        )),
        Ty::Ref { inner, .. } if matches!(**inner, Ty::Str) => Err(Boundary::Unsupported(
            ty.text(),
            "行 3 是 `const char*, uint64_t`（非 NUL 结尾）双形参，同上",
        )),
        Ty::Ref { inner, mutable, .. } | Ty::Ptr { inner, mutable } => {
            let t = pointee_c(inner)?;
            Ok(format!("{}{}*", if *mutable { "" } else { "const " }, t))
        }
        Ty::Boxed { .. } => Err(Boundary::Unsupported(
            ty.text(),
            "`Box<[u8]>` 移交是行 2 给的另一条路，但移交方向的释放责任表（`#[owns]`）还没接进代码生成",
        )),
        Ty::Slice { .. } => Err(Boundary::Unsupported(
            ty.text(),
            "行 2 要摊成 `ptr, len` 两个 C 形参，阶段 [8] 还没有切片实参的拆分发射",
        )),
        Ty::Str => Err(Boundary::Unsupported(
            ty.text(),
            "行 3 是 `const char*, uint64_t`（非 NUL 结尾）双形参，同上",
        )),
        Ty::String => Err(Boundary::Forbidden(
            ty.text(),
            "行 3/末条：那是 INFY 分配器的裸内存，跨边界必须显式 `.to_cstr()` 或改 ptr+len",
        )),
        Ty::Vec { .. } => Err(Boundary::Forbidden(
            ty.text(),
            "末条：`[T]` 的容量不得跨边界，只传 ptr+len",
        )),
        Ty::Rc { .. } => Err(Boundary::Forbidden(
            ty.text(),
            "末条：`Rc<T>` 的引用计数是 INFY 侧状态，跨边界必然失配",
        )),
        Ty::Option { inner } => Err(Boundary::Unsupported(
            ty.text(),
            if matches!(**inner, Ty::Ptr { .. } | Ty::Ref { .. } | Ty::Boxed { .. }) {
                "行 5 的 null→`None` thunk 还没有发射形态"
            } else {
                "非指针的 `Option<T>` 不在 §7.6.4 的表里"
            },
        )),
        Ty::Result { .. } => Err(Boundary::Unsupported(
            ty.text(),
            "行 6 要把返回改写成 `T ret, int32_t* err` 出参 thunk",
        )),
        Ty::Fn { .. } | Ty::Closure(_) | Ty::Dyn(_) => Err(Boundary::Unsupported(
            ty.text(),
            "行 8 的回调要 `void (*)(void* ctx, …)` + ctx 装箱，并经 `#[export]` 的 free 入口释放",
        )),
        Ty::Tuple(_) => Err(Boundary::Unsupported(ty.text(), "表里没有元组的跨边界行")),
        Ty::Array { .. } => Err(Boundary::Unsupported(
            ty.text(),
            "数组按值跨边界要先解决 §7.6.4 行 7 的共享定义",
        )),
        Ty::Named(n) | Ty::Adt(n) => Err(Boundary::Unsupported(
            n.clone(),
            "行 7：`#[repr(C)]` struct 按值跨边界要绑定侧的类型定义，而 `infy_binding.h` 目前只写标量与指针",
        )),
        Ty::Generic(n) => Err(Boundary::Unsupported(
            format!("{n}"),
            "类型参数未被单态化替换（§4.15）",
        )),
        Ty::Auto | Ty::IntLit | Ty::Invalid | Ty::Never => {
            Err(Boundary::Unsupported(ty.text(), "该形态不是可跨边界的类型"))
        }
    }
}

/// 拒绝消息的统一形状，阶段 [4] 与阶段 [8] 共用措辞。
fn boundary_gap(b: &Boundary, span: Span, decl: &str, param: Option<&str>, ty: &str) -> Gap {
    let where_ = match param {
        Some(p) => format!("`{decl}` 的形参 `{p}: {ty}`"),
        None => format!("`{decl}` 的返回类型 `{ty}`"),
    };
    Gap::new(b.code(), span, "A 轨跨边界类型不可映射", where_).note(b.text())
}

// ------------------------------------------------------------------ UB-010 移交方式判据

/// §7.6.1/§7.6.4/UB-010：跨边界指针的移交方式必须显式声明，没有默认值（P5 零隐式）。
#[derive(Clone, Debug, PartialEq)]
pub enum Ownership {
    /// 签名没有需要移交的方向 ⇒ 不要求属性。§7.6.1 的 `dec_close(h: *mut DecHandle)` 就没写。
    NotNeeded,
    Declared,
    Missing,
    /// `#[owns]` 与 `#[borrows(…)]` 同时出现：规范写的是**二选一**。
    Both,
    /// `#[borrows]` 不带生命周期：行 2 要求“显式声明生命周期”。
    BorrowsWithoutLifetime,
}

/// 返回值是否把跨边界内存交给 INFY 之外（指针形状 ⇒ 需要声明移交方式）。
///
/// 形参不算：§7.6.1 的样例里 `dec_open(path: *const u8, len: u64, err: *mut i32)` 与
/// `dec_close(h: *mut DecHandle)` 都带裸指针形参而**没有** `#[owns]`/`#[borrows]`，而它是规范的
/// 样例；行 2 的话也说“外部**保存**该指针必须 `#[borrows]` 显式声明”。所以 UB-010 的移交判在
/// 返回方向，见 `docs/spec-deviations.md` D-46(c)。
pub fn return_needs_ownership(ret: &Ty) -> bool {
    match ret {
        Ty::Ptr { .. } | Ty::Ref { .. } | Ty::Boxed { .. } => true,
        Ty::Option { inner } => matches!(**inner, Ty::Ptr { .. } | Ty::Ref { .. } | Ty::Boxed { .. }),
        _ => false,
    }
}

pub fn ownership_of(decl: &Item, ret: &Ty) -> Ownership {
    let owns = decl.has_attr("owns");
    let borrows = decl.attr("borrows");
    if owns && borrows.is_some() {
        return Ownership::Both;
    }
    if owns {
        return Ownership::Declared;
    }
    match borrows {
        Some(a) => {
            let has_lifetime = a.positional.iter().any(|v| matches!(v, AttrValue::Lifetime(_)))
                || a.args.iter().any(|x| matches!(x.value, Some(AttrValue::Lifetime(_))));
            if has_lifetime {
                Ownership::Declared
            } else {
                Ownership::BorrowsWithoutLifetime
            }
        }
        None if return_needs_ownership(ret) => Ownership::Missing,
        None => Ownership::NotNeeded,
    }
}

/// 一个 `extern` 声明的阶段 [4] 判据：**只有规范的禁止项在这里红**。
///
/// 分工（`docs/spec-deviations.md` D-46(e)）：§7.6.4 末条的禁止传递（`Rc<T>`、`[T]` 的容量、INFY
/// 分配器的裸内存）与 UB-010 的移交未声明是“这不是合法程序”，用 `E-0723`；而表里有对应行、本实现
/// 还没有发射形态的那几行（ptr+len 展开、`Option<&T>` 的 null thunk、`Result` 出参、回调 ctx、
/// `#[repr(C)]` struct 按值）是**合法程序 + 未实现**，规范没给它们分配任何码，于是留给阶段 [8] 的
/// 指名 `CgError`（与 F15/F19/F29 那一族同形）。在阶段 [4] 拿 `E-0723` 去砸一条规范允许的签名，
/// 等于假称程序不合规 —— 比拒绝更坏的失败。
pub fn decl_gaps(decl: &Item, params: &[(String, Ty)], ret: &Ty, span: Span) -> Vec<Gap> {
    let mut out = Vec::new();
    match ownership_of(decl, ret) {
        Ownership::Missing => out.push(
            Gap::new(
                "E-0723",
                span,
                "跨边界指针移交未声明",
                format!("`{}` 的返回类型 `{}` 把跨边界内存交给外部", decl.name(), ret.text()),
            )
            .note("UB-010 / §7.6.1：`#[owns]` 与 `#[borrows('a)]` 二选一，无默认值（P5 零隐式）")
            .help("在声明上写 `#[owns]`（接管释放）或 `#[borrows('static)]`（借出且生命周期显式）"),
        ),
        Ownership::Both => out.push(Gap::new(
            "E-0723",
            span,
            "移交方式同时写了两种",
            format!("`{}` 同时带 `#[owns]` 与 `#[borrows]`", decl.name()),
        )
        .note("§7.6.1 的表写的是二选一，两者同时出现等于没有声明")),
        Ownership::BorrowsWithoutLifetime => out.push(
            Gap::new(
                "E-0723",
                span,
                "`#[borrows]` 缺生命周期",
                format!("`{}` 的 `#[borrows]` 没有生命周期参数", decl.name()),
            )
            .help("写 `#[borrows('a)]` 或 `#[borrows('static)]`（§7.6.4 行 2：外部保存该指针必须显式声明生命周期）"),
        ),
        Ownership::Declared | Ownership::NotNeeded => {}
    }
    for (name, ty) in params {
        if let Err(b) = boundary_c(ty) {
            if matches!(b, Boundary::Forbidden(..)) {
                out.push(boundary_gap(&b, span, &decl.name(), Some(name), &ty.text()));
            }
        }
    }
    if !matches!(ret, Ty::Unit) {
        if let Err(b) = boundary_c(ret) {
            if matches!(b, Boundary::Forbidden(..)) {
                out.push(boundary_gap(&b, span, &decl.name(), None, &ret.text()));
            }
        }
    }
    out
}

// ------------------------------------------------------------------ §7.6.3 项 2：符号包装

/// ABI §2：`<8hex>` = 规范化签名块 SHA-256 的前 4 字节。
pub fn sig_hash(normalized: &str) -> String {
    let d = crate::sha256::sha256(normalized.as_bytes());
    let mut s = String::with_capacity(8);
    for b in &d[..4] {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 一个 C 标识符的拼写 —— 直接借用 `codegen::c_ident`，不在这里再抄一份关键字表。
/// 绑定头里的形参名与包装本体必须同拼写，而本体在同一个 TU 里被 gcc 编译，两套规则迟早撞车。
fn c_ident(raw: &str) -> String {
    crate::codegen::c_ident(raw)
}

/// §7.6.3 项 2 / ABI §2 的包装符号：`infy_e_<name>_<8hex>`。
pub fn wrapper_symbol(name: &str, normalized_block: &str) -> String {
    format!("{WRAPPER_PREFIX}{}_{}", c_ident(name), sig_hash(normalized_block))
}

/// 签名块的规范化文本（哈希的输入）：块名 + 成员按 `(package, name)` 字典序的 `<name><签名>`。
///
/// **不是**声明序：同一个块的两个声明可以写在相邻两行，也可以拆进 `//infy:build` 门后的两个文件，
/// 而声明序会随“哪个文件先被读到”变化；§7.3 要求同一输入的产物字节相同，所以哈希输入必须与文件
/// 顺序无关（`docs/spec-deviations.md` D-46(b)）。签名本体来自 `hir::HirFn::signature_text`。
pub fn normalized_block(unit: &str, decls: &[&HirFn]) -> String {
    let mut lines: Vec<String> = decls
        .iter()
        .map(|f| format!("{}{}", c_ident(&f.name), f.signature_text()))
        .collect();
    lines.sort();
    let mut out = format!("infy-a-track-v1 {unit}\n");
    out.push_str(&lines.join("\n"));
    out
}

/// 一个包装：C 侧签名 + 它转发的外部符号名。
#[derive(Clone, Debug, PartialEq)]
pub struct Wrapper {
    /// 单元名（`#[link(name = "…")]`）。
    pub unit: String,
    /// 用户源码必须定义的符号 = `extern` 声明的名字（§7.6.1 的 `dec_open`）。
    pub extern_symbol: String,
    /// `infy_e_<name>_<8hex>`，生成码实际调用的符号。
    pub symbol: String,
    /// 逐个形参的 C 类型写法（与 `boundary_c` 同源）。
    pub c_params: Vec<String>,
    /// `None` = `void`（INFY 侧的 `()`）。
    pub c_ret: Option<String>,
    /// 形参名，用于头文件与包装本体的可读性。
    pub param_names: Vec<String>,
}

impl Wrapper {
    fn param_list(&self, with_names: bool) -> String {
        if self.c_params.is_empty() {
            return "void".to_string();
        }
        self.c_params
            .iter()
            .zip(self.param_names.iter())
            .map(|(t, n)| if with_names { format!("{t} {}", c_ident(n)) } else { t.clone() })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// 头文件与 `extern` 声明共用的原型文本（不含结尾分号）。
    pub fn prototype(&self) -> String {
        format!(
            "{} {}({})",
            self.c_ret.as_deref().unwrap_or("void"),
            self.symbol,
            self.param_list(true)
        )
    }

    /// 被声明方（`ci.c`）只需要类型，不带名字 —— 避免与包装本体里的形参重名混淆。
    pub fn extern_symbol_prototype(&self) -> String {
        format!(
            "{} {}({})",
            self.c_ret.as_deref().unwrap_or("void"),
            c_ident(&self.extern_symbol),
            self.param_list(true)
        )
    }

    /// §7.6.3 项 3 的边界 thunk 本体。标量/指针子集没有可检查的边界，于是原样转发；
    /// `()` 返回时不写 `return`，否则 C 侧是 `void` 表达式。
    pub fn definition(&self) -> String {
        let args: Vec<String> = self.param_names.iter().map(|n| c_ident(n)).collect();
        let call = format!("{}({})", c_ident(&self.extern_symbol), args.join(", "));
        match &self.c_ret {
            Some(_) => format!("{} {{ return {}; }}", self.prototype(), call),
            None => format!("{} {{ {}; }}", self.prototype(), call),
        }
    }

    /// 调用点文本（codegen 拼好实参值后交给它）。
    pub fn call(&self, argv: &[String]) -> String {
        format!("{}({})", self.symbol, argv.join(", "))
    }
}

/// 一个单元的包装表。返回 `(wrappers, refusals)`，其中 `refusals` 是 `(声明名, 点名条款的一行)` ——
/// 逐成员给结论，调用方才能把拒绝归到正确的那条声明上（`codegen` 按 HIR 符号索引）。
pub fn wrappers(unit: &str, decls: &[&HirFn]) -> (Vec<Wrapper>, Vec<(String, String)>) {
    let norm = normalized_block(unit, decls);
    let mut out = Vec::new();
    let mut refusals = Vec::new();
    for f in decls {
        let mut c_params = Vec::new();
        let mut param_names = Vec::new();
        let mut mine: Vec<String> = Vec::new();
        for p in &f.params {
            match boundary_c(&p.ty) {
                Ok(t) => {
                    c_params.push(t);
                    param_names.push(c_ident(&p.name));
                }
                Err(b) => mine.push(boundary_gap(&b, Span::empty_at(0, 0), &f.name, Some(&p.name), &p.ty.text()).summary()),
            }
        }
        let c_ret = match &f.ret {
            Some((t, _)) => match boundary_c(t) {
                Ok(s) => {
                    if s == "void" {
                        None
                    } else {
                        Some(s)
                    }
                }
                Err(b) => {
                    mine.push(boundary_gap(&b, Span::empty_at(0, 0), &f.name, None, &t.text()).summary());
                    None
                }
            },
            None => None,
        };
        if mine.is_empty() {
            out.push(Wrapper {
                unit: unit.to_string(),
                extern_symbol: f.name.clone(),
                symbol: wrapper_symbol(&f.name, &norm),
                c_params,
                c_ret,
                param_names,
            });
        } else {
            refusals.push((f.name.clone(), mine.join(" / ")));
        }
    }
    (out, refusals)
}

// ------------------------------------------------------------------ §7.6.1 配对

/// 一个 `#[link(name = "…")] extern { … }` 签名块（AST 侧的读取结果）。
#[derive(Clone, Debug)]
pub struct SignatureBlock {
    pub unit: String,
    /// `extern "C"` 的 `"C"`；裸 `extern { }` 为 `None`。
    pub abi: Option<String>,
    pub is_static: bool,
    pub toolchain: Option<String>,
    pub flags: Vec<String>,
    pub isolation: Option<String>,
    /// 块内声明的名字（声明序）。
    pub decls: Vec<String>,
    pub span: Span,
}

/// 一个 A 轨源码块（`@c` / `@cpp` / `@rust` / `@zig`）。
#[derive(Clone, Debug)]
pub struct AtrackSource {
    pub unit: String,
    pub lang: String,
    /// 原样文本，不含外层花括号（§2.11 词法不透明）。
    pub body: String,
    pub span: Span,
}

/// 配对成功、可以编译的 A 轨单元。
#[derive(Clone, Debug)]
pub struct Unit {
    pub name: String,
    pub is_static: bool,
    pub toolchain: Option<String>,
    pub flags: Vec<String>,
    pub decls: Vec<String>,
    /// §7.6.1“编译器整段抽出”的用户源码原文。
    pub source: String,
    pub span: Span,
}

fn strings_in(e: &Expr, out: &mut Vec<String>) {
    match &e.kind {
        ExprKind::Lit(Lit::Str(s)) => out.push(s.clone()),
        ExprKind::Array(v) | ExprKind::Tuple(v) => {
            for x in v {
                strings_in(x, out);
            }
        }
        _ => {}
    }
}

fn flag_list(a: &Attr, name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for arg in &a.args {
        if arg.name.as_deref() == Some(name) {
            if let Some(AttrValue::Expr(e)) = &arg.value {
                strings_in(e, &mut out);
            }
        }
    }
    out
}

/// 收集所有 `#[link(name = "…")] extern { … }` 签名块，按 `(文件, 偏移)` 定序（§7.3 的可复现要求）。
pub fn signature_blocks(files: &BTreeMap<FileId, ParsedFile>) -> Vec<SignatureBlock> {
    signature_blocks_from(&files.iter().collect::<Vec<_>>())
}

/// [`signature_blocks`] 的另一个入口：`codegen` 手上是 `(FileId, package, ParsedFile)` 元组切片。
/// 两个入口共用同一份遍历，免得出现两套“什么算签名块”。
pub fn signature_blocks_of(files: &[(FileId, String, ParsedFile)]) -> Vec<SignatureBlock> {
    signature_blocks_from(&files.iter().map(|(id, _, pf)| (id, pf)).collect::<Vec<_>>())
}

fn signature_blocks_from(files: &[(&FileId, &ParsedFile)]) -> Vec<SignatureBlock> {
    let mut out = Vec::new();
    for (id, pf) in files {
        for it in &pf.items {
            collect_blocks(**id, it, &mut out);
        }
    }
    out.sort_by(|a, b| (a.span.file, a.span.start).cmp(&(b.span.file, b.span.start)));
    out
}

fn collect_blocks(id: FileId, it: &Item, out: &mut Vec<SignatureBlock>) {
    if let ItemKind::ExternBlock { abi, decls } = &it.kind {
        if let Some(a) = it.attr("link") {
            out.push(SignatureBlock {
                unit: a.str_arg("name").unwrap_or_default(),
                abi: abi.clone(),
                // §7.6.1 的样例写了 `static = true`；缺省按静态处理，因为 §7.4 的段顺序只有静态一档。
                is_static: a.bool_arg("static").unwrap_or(true),
                toolchain: a.str_arg("toolchain"),
                flags: flag_list(a, "flags"),
                isolation: a.str_arg("isolation"),
                decls: decls.iter().map(|d| d.name().to_string()).collect(),
                // 空 name 保留给 `pair()`，让它报 E-0720 而不是在这里静默丢掉整块。
                span: Span::new(id, it.span.start, it.span.end),
            });
        }
    }
    match &it.kind {
        ItemKind::Impl { items, .. } | ItemKind::Trait { items, .. } => {
            for inner in items {
                collect_blocks(id, inner, out);
            }
        }
        _ => {}
    }
}

/// 收集所有 A 轨源码块，按 `(文件, 偏移)` 定序。
pub fn atrack_sources(files: &BTreeMap<FileId, ParsedFile>) -> Vec<AtrackSource> {
    atrack_sources_from(&files.iter().collect::<Vec<_>>())
}

/// [`atrack_sources`] 的元组切片入口（同 [`signature_blocks_of`]）。
pub fn atrack_sources_of(files: &[(FileId, String, ParsedFile)]) -> Vec<AtrackSource> {
    atrack_sources_from(&files.iter().map(|(id, _, pf)| (id, pf)).collect::<Vec<_>>())
}

fn atrack_sources_from(files: &[(&FileId, &ParsedFile)]) -> Vec<AtrackSource> {
    let mut out = Vec::new();
    for (id, pf) in files {
        for it in &pf.items {
            let mut blocks = Vec::new();
            embeds_of_item(it, &mut blocks);
            for e in &blocks {
                if e.is_a_track() {
                    out.push(AtrackSource {
                        unit: e.unit_name().unwrap_or_default(),
                        lang: e.lang.clone(),
                        body: e.body.clone(),
                        span: Span::new(**id, e.span.start, e.span.end),
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| (a.span.file, a.span.start).cmp(&(b.span.file, b.span.start)));
    out
}

/// item 树里的每一个 `@…` 块都要走到：`@c` 可以写在函数体、`impl` 的方法体，也可以写在块内声明里
/// （§3.1 允许块内 `fn`），漏掉哪一层就是漏掉一个能编译的单元。
fn embeds_of_item(it: &Item, out: &mut Vec<EmbedBlock>) {
    match &it.kind {
        ItemKind::Fn { body, .. } | ItemKind::Event { body, .. } => embeds_of_body(body, out),
        ItemKind::Impl { items, .. } | ItemKind::Trait { items, .. } | ItemKind::ExternBlock { decls: items, .. } => {
            for inner in items {
                embeds_of_item(inner, out);
            }
        }
        _ => {}
    }
}

fn embeds_of_body(body: &Body, out: &mut Vec<EmbedBlock>) {
    match body {
        Body::Embed(e) => out.push(e.clone()),
        Body::Block(b) => embeds_of_block(b, out),
        Body::Foreign => {}
    }
}

fn embeds_of_expr(e: &Expr, out: &mut Vec<EmbedBlock>) {
    use crate::ast::ElseBranch;
    match &e.kind {
        ExprKind::Embed(b) => out.push(b.clone()),
        ExprKind::Block(b) | ExprKind::Unsafe(b) => embeds_of_block(b, out),
        ExprKind::If { cond, then, else_ } => {
            embeds_of_expr(cond, out);
            embeds_of_block(then, out);
            if let Some(br) = else_ {
                match &**br {
                    ElseBranch::Block(b) => embeds_of_block(b, out),
                    ElseBranch::If(s) => embeds_of_stmt(s, out),
                }
            }
        }
        ExprKind::Match { scrut, arms } => {
            embeds_of_expr(scrut, out);
            for a in arms {
                match &a.body {
                    crate::ast::ArmBody::Block(b) => embeds_of_block(b, out),
                    crate::ast::ArmBody::Expr(x) => embeds_of_expr(x, out),
                }
            }
        }
        ExprKind::Unary { expr, .. } | ExprKind::Deref { expr } | ExprKind::Try { expr } | ExprKind::Move { expr } => {
            embeds_of_expr(expr, out);
        }
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::Assign { target: lhs, value: rhs, .. } => {
            embeds_of_expr(lhs, out);
            embeds_of_expr(rhs, out);
        }
        ExprKind::Call { callee, args } => {
            embeds_of_expr(callee, out);
            for a in args {
                embeds_of_expr(&a.expr, out);
            }
        }
        ExprKind::Method { recv, args, .. } => {
            embeds_of_expr(recv, out);
            for a in args {
                embeds_of_expr(&a.expr, out);
            }
        }
        ExprKind::Array(v) | ExprKind::Tuple(v) => {
            for x in v {
                embeds_of_expr(x, out);
            }
        }
        ExprKind::Closure { body, .. } => embeds_of_block(body, out),
        _ => {}
    }
}

fn embeds_of_block(b: &crate::ast::Block, out: &mut Vec<EmbedBlock>) {
    for s in &b.stmts {
        embeds_of_stmt(s, out);
    }
    if let Some(t) = &b.tail {
        embeds_of_expr(t, out);
    }
}

fn embeds_of_stmt(s: &crate::ast::Stmt, out: &mut Vec<EmbedBlock>) {
    use crate::ast::{ElseBranch, StmtKind};
    match &s.kind {
        StmtKind::Expr { expr, .. } => embeds_of_expr(expr, out),
        StmtKind::Let { init, .. } => {
            if let Some(e) = init {
                embeds_of_expr(e, out);
            }
        }
        StmtKind::Assign { target, value, .. } => {
            embeds_of_expr(target, out);
            embeds_of_expr(value, out);
        }
        StmtKind::If { cond, then, else_ } => {
            embeds_of_expr(cond, out);
            embeds_of_block(then, out);
            if let Some(br) = else_ {
                match &**br {
                    ElseBranch::Block(b) => embeds_of_block(b, out),
                    ElseBranch::If(inner) => embeds_of_stmt(inner, out),
                }
            }
        }
        StmtKind::While { cond, body } => {
            embeds_of_expr(cond, out);
            embeds_of_block(body, out);
        }
        StmtKind::For { iter, body, .. } => {
            embeds_of_expr(iter, out);
            embeds_of_block(body, out);
        }
        StmtKind::Loop { body, .. } => embeds_of_block(body, out),
        StmtKind::Match { scrut, arms } => {
            embeds_of_expr(scrut, out);
            for a in arms {
                match &a.body {
                    crate::ast::ArmBody::Block(b) => embeds_of_block(b, out),
                    crate::ast::ArmBody::Expr(x) => embeds_of_expr(x, out),
                }
            }
        }
        StmtKind::Return { value } | StmtKind::Break { value, .. } => {
            if let Some(e) = value {
                embeds_of_expr(e, out);
            }
        }
        StmtKind::Unsafe(b) | StmtKind::Defer(b) => embeds_of_block(b, out),
        StmtKind::ConstAssert { cond, .. } => embeds_of_expr(cond, out),
        // 块内声明（§3.1 只允许 `fn`/`const`）里也可能有 `@c`，所以一起走。
        StmtKind::NestedItem(inner) => embeds_of_item(inner, out),
        _ => {}
    }
}

/// §7.6.1 的配对。**双向**都报 `E-0720`：
/// `@c` 落空由 `sema::check_embed` 报（它手上就是那个源码块的 span），这里补“签名块落空”那一半
/// 以及 ABI/前缀的支持面。返回的 `Vec<Unit>` 已按单元名定序。
pub fn pair(blocks: &[SignatureBlock], sources: &[AtrackSource]) -> (Vec<Unit>, Vec<Gap>) {
    let mut units = Vec::new();
    let mut gaps = Vec::new();
    for b in blocks {
        if b.unit.is_empty() {
            gaps.push(
                Gap::new("E-0720", b.span, "A 轨缺少 name", "`#[link]` 必须写 `name = \"…\"`")
                    .note("§7.6.1：签名块与源码块由同一个 `name` 绑定"),
            );
            continue;
        }
        if let Some(abi) = &b.abi {
            if !SUPPORTED_ABIS.contains(&abi.as_str()) {
                gaps.push(
                    Gap::new(
                        "E-0720",
                        b.span,
                        "A 轨 ABI 不受支持",
                        format!("`extern \"{abi}\"`（单元 `{}`）", b.unit),
                    )
                    .note("§7.6.1：`extern \"C\"` 是 A 轨唯一的稳定 ABI，`\"system\"` 与 `\"infy-unwind\"` 还没有调用约定/跨界展开的实现")
                    .help("改成 `extern \"C\"`"),
                );
                continue;
            }
        }
        let mine: Vec<&AtrackSource> = sources.iter().filter(|s| s.unit == b.unit).collect();
        let foreign: Vec<&AtrackSource> = mine.iter().copied().filter(|s| s.lang != "c").collect();
        if !foreign.is_empty() {
            gaps.push(
                Gap::new(
                    "E-0720",
                    b.span,
                    "A 轨源码块的前缀未注册",
                    format!(
                        "单元 `{}` 的源码块前缀是 `{}`",
                        b.unit,
                        foreign.iter().map(|s| format!("@{}", s.lang)).collect::<Vec<_>>().join(", ")
                    ),
                )
                .note("§7.6.2 的工具链注册（`TOOLCHAIN.infy.toml` / `infy toolchain probe`）还没有消费者 ⇒ 本实现只驱动 `cc`，只有 `@c` 可编译"),
            );
            continue;
        }
        if mine.is_empty() {
            gaps.push(
                Gap::new(
                    "E-0720",
                    b.span,
                    "A 轨 name 不匹配",
                    format!("`#[link(name = \"{}\")]]` 没有配对的 `@c(name = \"{}\")`", b.unit, b.unit),
                )
                .note("§7.6.1：A 轨必须成对出现 —— 一个签名块 + 一个源码块"),
            );
            continue;
        }
        // 同一单元的多个源码块按（文件, 偏移）序拼接，与 `atrack_sources` 的定序一致。
        units.push(Unit {
            name: b.unit.clone(),
            is_static: b.is_static,
            toolchain: b.toolchain.clone(),
            flags: b.flags.clone(),
            decls: b.decls.clone(),
            source: mine.iter().map(|s| s.body.as_str()).collect::<Vec<_>>().join("\n"),
            span: b.span,
        });
    }
    units.sort_by(|a, c| a.name.cmp(&c.name));
    (units, gaps)
}

/// 取一个单元在 HIR 里的 A 轨声明（成员可能分布在多个文件），按 `(package, name)` 定序。
pub fn unit_decls<'a>(items: &'a [HirItem], unit: &str) -> Vec<&'a HirFn> {
    let mut out: Vec<&HirFn> = items
        .iter()
        .filter_map(|it| match it {
            HirItem::Fn(f) if f.is_foreign && f.link_unit.as_deref() == Some(unit) => Some(f),
            _ => None,
        })
        .collect();
    out.sort_by(|a, b| (a.package.clone(), a.name.clone()).cmp(&(b.package.clone(), b.name.clone())));
    out
}

/// 全部单元的包装表：`unit -> (wrappers, refusals)`。`codegen` 与 `cli` 都调它，
/// 于是绑定头里的原型和生成码调用的符号永远来自同一次计算。
pub fn wrapper_plan(items: &[HirItem], blocks: &[SignatureBlock]) -> BTreeMap<String, (Vec<Wrapper>, Vec<(String, String)>)> {
    let mut out = BTreeMap::new();
    for b in blocks {
        if b.unit.is_empty() || out.contains_key(&b.unit) {
            continue;
        }
        let decls = unit_decls(items, &b.unit);
        let (ws, refusals) = wrappers(&b.unit, &decls);
        out.insert(b.unit.clone(), (ws, refusals));
    }
    out
}

// ------------------------------------------------------------------ §7.6.3 项 1/2：产物文本

fn include_guard(unit: &str) -> String {
    let mut s = String::from("INFY_BINDING_");
    for c in unit.chars() {
        s.push(if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' });
    }
    s.push_str("_H");
    s
}

/// 单元名要变成目录里的文件名。`#[link(name = "…")]` 的 name 是用户给的字符串，
/// 不消毒就等于让一条声明决定写到构建目录**外面**去哪里（`../..`、绝对路径、盘符都进得来）。
pub fn unit_file_stem(unit: &str) -> String {
    let mut s = String::new();
    for c in unit.chars() {
        s.push(if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' });
    }
    if s.is_empty() || s.chars().all(|c| c == '_') {
        s.push_str("unit");
    }
    s
}

/// §7.6.3 项 1：`infy_binding.h` —— 由 `extern` 签名块**反向**生成的 C 头。
/// 一个构建只有一份头文件，所以 `unit_names` 是这次构建参与的全部单元。
pub fn binding_header(unit_names: &[String], ws: &[Wrapper]) -> String {
    let guard = include_guard(if unit_names.len() == 1 { unit_names[0].as_str() } else { "all" });
    let mut s = format!(
        "/* generated by `infy`（§7.6.3 项 1）— A 轨单元 [{}] 的绑定头，勿手改 */\n",
        unit_names.join(", ")
    );
    s.push_str(&format!("#ifndef {guard}\n#define {guard}\n"));
    s.push_str("#include <stdint.h>\n#include <stddef.h>\n\n");
    s.push_str("/* 用户源码必须定义的符号（§7.6.1：签名块与源码块由 name 配对） */\n");
    for w in ws {
        s.push_str(&format!("{};\n", w.extern_symbol_prototype()));
    }
    s.push_str("\n/* §7.6.3 项 2 的包装符号：生成码调用的就是它们 */\n");
    for w in ws {
        s.push_str(&format!("{};\n", w.prototype()));
    }
    s.push_str(&format!("\n#endif /* {guard} */\n"));
    s
}

/// `<build>/a_track/<name>.c`：绑定头 + 用户源码原样 + 包装本体。
pub fn unit_c_source(unit: &str, user_c: &str, ws: &[Wrapper]) -> String {
    let mut s = format!("/* generated by `infy`（§7.6.3 项 2/3）— A 轨单元 `{unit}` 的翻译单元 */\n");
    s.push_str("#include \"infy_binding.h\"\n\n");
    s.push_str("/* ---- 用户源码，逐字节原样（§2.11 词法不透明） ---- */\n");
    s.push_str(user_c);
    if !user_c.ends_with('\n') {
        s.push('\n');
    }
    s.push_str("\n/* ---- 包装 ");
    s.push_str(WRAPPER_PREFIX);
    s.push_str("<name>_<8hex> ---- */\n");
    for w in ws {
        s.push_str(&w.definition());
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Handler;
    use crate::hir::{HirConfig, Target};
    use crate::span::Sources;
    use crate::{lexer, parser, sema};

    /// 阶段 [1]-[4] 的一个文件；`package p`，清单声明了 A 轨。
    fn parsed(src: &str) -> (Sources, BTreeMap<FileId, ParsedFile>) {
        let mut sources = Sources::default();
        let f = sources.add("p/mod.infy".to_string(), format!("package p\n\n{src}\n"));
        let mut h = Handler::default();
        let lexed = lexer::lex_file(&sources, f, &mut h);
        let pf = parser::parse(&sources, f, &lexed, &mut h);
        let mut files = BTreeMap::new();
        files.insert(f, pf);
        (sources, files)
    }

    fn blocks(src: &str) -> Vec<SignatureBlock> {
        let (s, files) = parsed(src);
        let _ = s;
        signature_blocks(&files)
    }

    fn sources_of(src: &str) -> Vec<AtrackSource> {
        let (_, files) = parsed(src);
        atrack_sources(&files)
    }

    /// 阶段 [1]-[7]；`a_track_declared = true` 且 `link_units` 由源码里的 `#[link]` 提供。
    fn lowered(src: &str) -> (Sources, Vec<HirItem>) {
        let mut sources = Sources::default();
        let f = sources.add("p/mod.infy".to_string(), format!("package p\n\n{src}\n"));
        let mut h = Handler::default();
        let lexed = lexer::lex_file(&sources, f, &mut h);
        let pf = parser::parse(&sources, f, &lexed, &mut h);
        let tuple = vec![(f, "t/p".to_string(), pf)];
        let units: Vec<String> = vec!["dec".to_string(), "pair".to_string()];
        let prog = sema::check_files(&sources, &mut h, "t/p", &tuple, true, vec![], units);
        let hc = crate::hir::lower(&prog, &HirConfig { target: Target::REFERENCE, entry_chain: None });
        (sources, hc.items)
    }

    const UNIT: &str = "#[link(name = \"dec\", static = true, toolchain = \"cc\")]\nextern \"C\" {\n    fn decode(p: *const u8, n: u32) -> i32;\n}\n";

    // ------------------------------------------------------------ §7.6.1 配对

    /// §7.6.1：`#[link]` 签名块与 `@c` 源码块由同一 `name` 绑定；这里钉的是读取面。
    #[test]
    fn a_signature_block_is_read_with_its_attr_and_decls() {
        let bs = blocks(UNIT);
        assert_eq!(bs.len(), 1, "{bs:?}");
        assert_eq!(bs[0].unit, "dec");
        assert_eq!(bs[0].abi.as_deref(), Some("C"), "extern \"C\" 的 abi 必须读出来");
        assert!(bs[0].is_static, "`static = true`");
        assert_eq!(bs[0].toolchain.as_deref(), Some("cc"));
        assert_eq!(bs[0].decls, vec!["decode".to_string()]);
    }

    /// §7.6.1 的源码块是词法不透明的整段文本（§2.11），且必须带得上 `name`。
    #[test]
    fn a_c_block_is_collected_with_its_raw_body() {
        let src = format!("{UNIT}\nfn go(p: *const u8) -> i32 {{ @c(name = \"dec\") {{\n    int decode(const unsigned char* p, unsigned n) {{ return 0; }}\n}}\n    0\n}}");
        let ss = sources_of(&src);
        assert_eq!(ss.len(), 1, "{ss:?}");
        assert_eq!(ss[0].unit, "dec");
        assert_eq!(ss[0].lang, "c");
        assert!(ss[0].body.contains("int decode(const unsigned char* p"), "body must be verbatim:\n{}", ss[0].body);
    }

    /// §7.6.1：签名块找不到源码块也是 `E-0720`（`sema` 只报“源码块找不到签名块”那一半）。
    #[test]
    fn a_signature_block_without_source_is_e0720() {
        let bs = blocks(UNIT);
        let (units, gaps) = pair(&bs, &[]);
        assert!(units.is_empty());
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        assert_eq!(gaps[0].code, "E-0720");
        assert!(gaps[0].label.contains("dec") && gaps[0].label.contains("@c"), "{}", gaps[0].label);
    }

    /// 配对成功时不产生判据，且单元按名字定序。
    #[test]
    fn a_paired_unit_carries_its_source() {
        let src = format!("{UNIT}\nfn go() {{ @c(name = \"dec\") {{ /*body*/ }} }}");
        let bs = blocks(&src);
        let ss = sources_of(&src);
        let (units, gaps) = pair(&bs, &ss);
        assert!(gaps.is_empty(), "{:?}", gaps.iter().map(|g| g.summary()).collect::<Vec<_>>());
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].name, "dec");
        assert!(units[0].source.contains("/*body*/"));
    }

    /// §7.6.1：`extern "C"` 是唯一稳定 ABI —— `"system"`/`"infy-unwind"` 必须点名拒绝。
    #[test]
    fn an_unsupported_abi_is_refused_by_name() {
        for abi in ["system", "infy-unwind"] {
            let src = format!("#[link(name = \"dec\")]\nextern \"{abi}\" {{\n    fn decode(p: *const u8) -> i32;\n}}\n");
            let bs = blocks(&src);
            let (units, gaps) = pair(&bs, &[AtrackSource { unit: "dec".into(), lang: "c".into(), body: String::new(), span: Span::empty_at(0, 0) }]);
            assert!(units.is_empty(), "{abi} must not be accepted");
            assert_eq!(gaps.len(), 1, "{abi}: {gaps:?}");
            assert!(gaps[0].label.contains(abi), "{}", gaps[0].label);
        }
    }

    /// §7.6.2 的工具链注册还没有消费者 ⇒ `@cpp`/`@rust`/`@zig` 点名拒绝而不是当成 C 编译。
    #[test]
    fn a_non_c_a_track_prefix_is_refused_by_name() {
        let bs = blocks(UNIT);
        for lang in ["cpp", "rust", "zig"] {
            let s = AtrackSource { unit: "dec".into(), lang: lang.to_string(), body: String::new(), span: Span::empty_at(0, 0) };
            let (units, gaps) = pair(&bs, &[s]);
            assert!(units.is_empty(), "@{lang} must be refused");
            assert_eq!(gaps.len(), 1, "@{lang}");
            assert!(gaps[0].label.contains(&format!("@{lang}")), "{}", gaps[0].label);
            assert!(gaps[0].note.as_deref().unwrap_or("").contains("§7.6.2"), "{:?}", gaps[0].note);
        }
    }

    /// `#[link]` 缺 `name` 时整块不能静默消失（`E-0720` 的“缺少 name”分支）。
    #[test]
    fn a_link_block_without_name_is_reported_not_dropped() {
        let src = "#[link(static = true)]\nextern \"C\" {\n    fn decode(p: *const u8) -> i32;\n}\n";
        let bs = blocks(src);
        assert_eq!(bs.len(), 1, "the block must still be read");
        assert!(bs[0].unit.is_empty());
        let (units, gaps) = pair(&bs, &[]);
        assert!(units.is_empty());
        assert_eq!(gaps[0].code, "E-0720", "{:?}", gaps.iter().map(|g| g.summary()).collect::<Vec<_>>());
    }

    // ------------------------------------------------------------ §7.6.3 项 2：包装符号

    /// ABI §2 / §7.6.3 项 2：`<8hex>` = 规范化签名块 SHA-256 的前 4 字节。
    #[test]
    fn the_wrapper_symbol_is_the_prefix_name_and_four_hash_bytes() {
        let (s, items) = lowered(UNIT);
        let _ = s;
        let bs = blocks(UNIT);
        let plan = wrapper_plan(&items, &bs);
        let (ws, refusals) = plan.get("dec").unwrap_or_else(|| panic!("no plan: {plan:?}"));
        assert!(refusals.is_empty(), "{refusals:?}");
        assert_eq!(ws.len(), 1);
        assert_eq!(ws[0].extern_symbol, "decode");
        assert!(ws[0].symbol.starts_with("infy_e_decode_"), "{}", ws[0].symbol);
        let suffix = ws[0].symbol.strip_prefix("infy_e_decode_").expect("prefix");
        assert_eq!(suffix.len(), 8, "{suffix}");
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{suffix}");
        let norm = normalized_block("dec", &unit_decls(&items, "dec"));
        assert_eq!(suffix, sig_hash(&norm));
    }

    /// 规范化用名字序而不是声明序：同一个块的两个声明换个写法顺序必须得到同一个哈希
    /// （否则 §7.3 的可复现承诺会依赖“哪个文件先被读到”）。
    #[test]
    fn the_normalised_block_is_independent_of_declaration_order() {
        let ab = "#[link(name = \"pair\")]\nextern \"C\" {\n    fn a(x: u32) -> u32;\n    fn b(x: u64) -> u64;\n}\n";
        let ba = "#[link(name = \"pair\")]\nextern \"C\" {\n    fn b(x: u64) -> u64;\n    fn a(x: u32) -> u32;\n}\n";
        let (_, items1) = lowered(ab);
        let (_, items2) = lowered(ba);
        let d1 = unit_decls(&items1, "pair");
        let d2 = unit_decls(&items2, "pair");
        assert_eq!(normalized_block("pair", &d1), normalized_block("pair", &d2));
        assert_eq!(wrapper_symbol("a", &normalized_block("pair", &d1)), wrapper_symbol("a", &normalized_block("pair", &d2)));
        // 反过来：多一个成员必须换哈希（否则同名不同签名会撞车，正是 §7.6.3 项 2 要防的）。
        let (_, items3) = lowered("#[link(name = \"pair\")]\nextern \"C\" {\n    fn a(x: u32) -> u32;\n    fn b(x: u64) -> u64;\n    fn c(x: u8) -> u8;\n}\n");
        assert_ne!(normalized_block("pair", &d1), normalized_block("pair", &unit_decls(&items3, "pair")));
    }

    /// `hir::HirFn::signature_text` 是哈希输入的一部分；它此前**没有任何调用点**，
    /// 本测试把它钉成活的。
    #[test]
    fn the_normalised_block_uses_the_hir_signature_column() {
        let (s, items) = lowered(UNIT);
        let _ = s;
        let d = unit_decls(&items, "dec");
        assert_eq!(d.len(), 1);
        assert!(normalized_block("dec", &d).contains(&d[0].signature_text()), "{}", normalized_block("dec", &d));
        assert!(d[0].signature_text().contains("(*const u8,u32)->i32"), "{}", d[0].signature_text());
    }

    // ------------------------------------------------------------ §7.6.4 边界类型表

    /// §7.6.4 行 1 的值拷贝子集（同宽整型 / `double` / `uint8_t` / 32 位枚举）。
    #[test]
    fn the_scalar_rows_map_to_their_c11_spelling() {
        for (ty, want) in [
            (Ty::Prim(Prim::U8), "uint8_t"),
            (Ty::Prim(Prim::U32), "uint32_t"),
            (Ty::Prim(Prim::I64), "int64_t"),
            (Ty::Prim(Prim::F64), "double"),
            (Ty::Bool, "uint8_t"),
            (Ty::Unit, "void"),
        ] {
            assert_eq!(boundary_c(&ty), Ok(want.to_string()), "{}", ty.text());
        }
        // `bool`/`char` 的写法与 infy_rt.h 的 typedef 同宽，所以调用点不会出现类型不符。
        assert_eq!(scalar_c(Prim::Bool), Some("uint8_t"));
        assert_eq!(scalar_c(Prim::Char), Some("uint32_t"));
    }

    /// §7.6.1 样例的两个裸指针形参：`*const u8` 与 `*mut i32` 的拼写必须与 `codegen::c_ty` 一致。
    #[test]
    fn pointer_rows_keep_the_const_qualifier_and_pointee() {
        assert_eq!(boundary_c(&Ty::Ptr { inner: Box::new(Ty::Prim(Prim::U8)), mutable: false }), Ok("const uint8_t*".to_string()));
        assert_eq!(boundary_c(&Ty::Ptr { inner: Box::new(Ty::Prim(Prim::I32)), mutable: true }), Ok("int32_t*".to_string()));
        assert_eq!(boundary_c(&Ty::Ref { inner: Box::new(Ty::Prim(Prim::U8)), mutable: false, lifetime: Some("'a".into()) }), Ok("const uint8_t*".to_string()));
    }

    /// §7.6.4 末条：`Rc<T>`、`Vec`/`String` 的容量与裸内存是**禁止**，不是“还没做”。
    #[test]
    fn the_forbidden_rows_are_named_as_forbidden() {
        for ty in [Ty::Rc { inner: Box::new(Ty::Prim(Prim::U8)), atomic: false }, Ty::Vec { inner: Box::new(Ty::Prim(Prim::U8)) }, Ty::String] {
            let b = boundary_c(&ty).expect_err("must be refused");
            assert!(matches!(b, Boundary::Forbidden(..)), "{:?} => {b:?}", ty.text());
            assert_eq!(b.code(), "E-0723");
        }
    }

    /// 表里有但还没发射的行（切片、`&str`、`Result` 出参、回调…）必须点名是哪一行 —— 且不被
    /// 当成“禁止”而让规范看起来错了。
    #[test]
    fn rows_without_an_emission_shape_are_refused_by_name_not_silently() {
        let ty = Ty::Slice { inner: Box::new(Ty::Prim(Prim::U8)) };
        match boundary_c(&ty) {
            Err(Boundary::Unsupported(_, why)) => assert!(why.contains("ptr"), "{why}"),
            other => panic!("expected the ptr+len refusal, got {other:?}"),
        }
        for t in [
            Ty::Str,
            Ty::Result { ok: Box::new(Ty::Prim(Prim::U32)), err: Box::new(Ty::Prim(Prim::I32)) },
            Ty::Option { inner: Box::new(Ty::Prim(Prim::U8)) },
            Ty::Adt("DecHandle".into()),
        ] {
            assert!(boundary_c(&t).is_err(), "{} must be refused", t.text());
        }
    }

    // ------------------------------------------------------------ UB-010 移交方式

    /// §7.6.1 / UB-010：返回裸指针而没写 `#[owns]`/`#[borrows]` → `E-0723`（无默认值）。
    #[test]
    fn a_pointer_returning_declaration_without_ownership_is_e0723() {
        let src = "#[link(name = \"dec\")]\nextern \"C\" {\n    fn dec_open(p: *const u8) -> *mut u8;\n}\n";
        let gaps = gaps_of(src);
        assert_eq!(gaps.len(), 1, "{:?}", gaps.iter().map(|g| g.summary()).collect::<Vec<_>>());
        assert_eq!(gaps[0].code, "E-0723");
        assert!(gaps[0].label.contains("dec_open"), "{}", gaps[0].label);
    }

    /// 成对断言的另一半：写了 `#[borrows('static)]` 就放行，且**形参**的裸指针不需要属性
    /// （§7.6.1 的样例就是这么写的）。
    #[test]
    fn an_explicit_ownership_annotation_is_accepted() {
        let src = "#[link(name = \"dec\")]\nextern \"C\" {\n    #[borrows('static)]\n    fn dec_open(p: *const u8, e: *mut i32) -> *mut u8;\n}\n";
        assert!(gaps_of(src).is_empty(), "{:?}", gaps_of(src).iter().map(|g| g.summary()).collect::<Vec<_>>());
        let owned = "#[link(name = \"dec\")]\nextern \"C\" {\n    #[owns]\n    fn dec_open(p: *const u8) -> *mut u8;\n}\n";
        assert!(gaps_of(owned).is_empty());
    }

    /// `#[owns]` 与 `#[borrows]` 同时出现 = 没有声明（规范写的是二选一）。
    #[test]
    fn both_ownership_annotations_together_are_still_e0723() {
        let src = "#[link(name = \"dec\")]\nextern \"C\" {\n    #[owns]\n    #[borrows('static)]\n    fn dec_open(p: *const u8) -> *mut u8;\n}\n";
        let g = gaps_of(src);
        assert_eq!(g.len(), 1);
        assert!(g[0].title.contains("同时"), "{:?}", g[0].title);
    }

    /// §7.6.4 行 2：`#[borrows]` 必须带上生命周期。
    #[test]
    fn borrows_without_a_lifetime_is_e0723() {
        let src = "#[link(name = \"dec\")]\nextern \"C\" {\n    #[borrows]\n    fn dec_open(p: *const u8) -> *mut u8;\n}\n";
        let g = gaps_of(src);
        assert_eq!(g.len(), 1, "{:?}", g.iter().map(|x| x.summary()).collect::<Vec<_>>());
        assert!(g[0].title.contains("生命周期"), "{}", g[0].title);
    }

    /// `()` 返回的声明没有移交方向 ⇒ 不要求属性（§7.6.1 的 `dec_close`）。
    #[test]
    fn a_void_declaration_needs_no_ownership_attribute() {
        let src = "#[link(name = \"dec\")]\nextern \"C\" {\n    fn dec_close(h: *mut u8);\n}\n";
        assert!(gaps_of(src).is_empty(), "{:?}", gaps_of(src).iter().map(|g| g.summary()).collect::<Vec<_>>());
    }

    /// 直接对 AST 跑判据：单测钉的是 `atrack` 的表，不是 `sema` 的接线（接线另有成条测试）。
    fn gaps_of(src: &str) -> Vec<Gap> {
        let (_, files) = parsed(src);
        let mut out = Vec::new();
        for pf in files.values() {
            for it in &pf.items {
                if let ItemKind::ExternBlock { decls, .. } = &it.kind {
                    for d in decls {
                        if let ItemKind::Fn { sig, .. } = &d.kind {
                            let params: Vec<(String, Ty)> = sig
                                .params
                                .iter()
                                .map(|p| (p.name.clone(), crate::hir::hir_ty_from_ast(&p.ty).unwrap_or(Ty::Invalid)))
                                .collect();
                            let ret = sig
                                .ret
                                .as_ref()
                                .and_then(crate::hir::hir_ty_from_ast)
                                .unwrap_or(Ty::Unit);
                            out.extend(decl_gaps(d, &params, &ret, d.span));
                        }
                    }
                }
            }
        }
        out
    }

    /// §7.6.4 的表在阶段 [4] 就要生效：一个 `Vec<u8>` 形参的 A 轨声明必须在 `infy check` 阶段就红，
    /// 而不是拖到阶段 [8] 的发射。
    #[test]
    fn the_boundary_table_is_enforced_at_stage_four() {
        let mut sources = Sources::default();
        let f = sources.add("p/mod.infy".to_string(), format!("package p\n\n{}\n", "#[link(name = \"dec\")]\nextern \"C\" {\n    fn dec_read(b: Vec<u8>) -> u32;\n}\n"));
        let mut h = Handler::default();
        let lexed = lexer::lex_file(&sources, f, &mut h);
        let pf = parser::parse(&sources, f, &lexed, &mut h);
        let tuple = vec![(f, "t/p".to_string(), pf)];
        sema::check_files(&sources, &mut h, "t/p", &tuple, true, vec![], vec!["dec".to_string()]);
        let codes: Vec<String> = h.diags.iter().map(|d| d.code.clone()).collect();
        assert!(codes.iter().any(|c| c == "E-0723"), "E-0723 must fire at stage [4], got {codes:?}");
    }

    /// 发射侧用的是同一张表：`wrappers()` 对同一签名给出拒绝，而不是生成一个丢参数的原型。
    #[test]
    fn the_emission_side_refuses_the_same_signatures() {
        let src = "#[link(name = \"dec\")]\nextern \"C\" {\n    fn dec_read(b: Vec<u8>) -> u32;\n}\n";
        let mut sources = Sources::default();
        let f = sources.add("p/mod.infy".to_string(), format!("package p\n\n{src}\n"));
        let mut h = Handler::default();
        let lexed = lexer::lex_file(&sources, f, &mut h);
        let pf = parser::parse(&sources, f, &lexed, &mut h);
        let tuple = vec![(f, "t/p".to_string(), pf)];
        let prog = sema::check_files(&sources, &mut h, "t/p", &tuple, true, vec![], vec!["dec".to_string()]);
        let hc = crate::hir::lower(&prog, &HirConfig { target: Target::REFERENCE, entry_chain: None });
        let (ws, refusals) = wrappers("dec", &unit_decls(&hc.items, "dec"));
        assert!(ws.is_empty(), "a refused signature must not produce a wrapper: {ws:?}");
        assert_eq!(refusals.len(), 1, "{refusals:?}");
        assert_eq!(refusals[0].0, "dec_read", "the refusal must name the declaration");
        assert!(refusals[0].1.contains("Vec<u8>") && refusals[0].1.contains("E-0723"), "{refusals:?}");
    }

    /// §7.6.4 行 2 的 `&[u8]` 是**规范允许**的跨边界类型，本实现只是还没有把实参摊成 `ptr, len` 的
    /// 发射形态。它不许在阶段 [4] 变成 `E-0723`（那是"这个程序不合规"的假称，见 D-46(e)），
    /// 只能在阶段 [8] 以条款号指名拒绝 —— 同一条签名在两处各测一半。
    #[test]
    fn a_legal_signature_without_emission_shape_is_not_an_e0723() {
        let src = "#[link(name = \"dec\")]\nextern \"C\" {\n    fn dec_read(b: &[u8]) -> u32;\n}\n";
        assert!(gaps_of(src).is_empty(), "stage [4] must stay clean: {:?}", gaps_of(src).iter().map(|g| g.summary()).collect::<Vec<_>>());
        let (s, items) = lowered(src);
        let _ = s;
        let (ws, refusals) = wrappers("dec", &unit_decls(&items, "dec"));
        assert!(ws.is_empty(), "no wrapper may be built from an unmapped row");
        assert_eq!(refusals.len(), 1, "{refusals:?}");
        assert!(refusals[0].1.contains("§7.6.4"), "the refusal must name the clause: {}", refusals[0].1);
        assert!(refusals[0].1.contains("ptr"), "and say which shape is missing: {}", refusals[0].1);
    }

    // ------------------------------------------------------------ §7.6.3 项 1：绑定头与包装

    /// 头文件里两处原型：用户必须定义的符号 + 包装符号；两者类型来自同一次 `boundary_c`。
    #[test]
    fn the_binding_header_declares_both_halves_of_the_pair() {
        let (s, items) = lowered(UNIT);
        let _ = s;
        let decls = unit_decls(&items, "dec");
        let (ws, refusals) = wrappers("dec", &decls);
        assert!(refusals.is_empty(), "{refusals:?}");
        let h = binding_header(&["dec".to_string()], &ws);
        assert!(h.contains("int32_t decode(const int32_t*") || h.contains("int32_t decode(const uint8_t* p, uint32_t n);"), "{h}");
        assert!(h.contains(&ws[0].symbol), "the wrapper symbol must be declared:\n{h}");
        assert!(h.contains("#include <stdint.h>"), "{h}");
        assert!(h.contains("#ifndef INFY_BINDING_DEC_H") && h.contains("#endif"), "guard pair");
        // 包装原型与本体必须同一份签名（本体转发的是用户符号）。
        let def = ws[0].definition();
        assert!(def.starts_with(&ws[0].prototype()), "{def}\n{}", ws[0].prototype());
        assert!(def.contains("decode("), "{def}");
        assert!(ws[0].symbol.len() > "infy_e_decode_".len());
    }

    /// `()` 返回的包装不能写 `return`（C 侧是 `void` 表达式）。
    #[test]
    fn a_unit_returning_void_forwards_without_return() {
        let src = "#[link(name = \"dec\")]\nextern \"C\" {\n    fn dec_close(h: *mut u8);\n}\n";
        let (s, items) = lowered(src);
        let _ = s;
        let (ws, refusals) = wrappers("dec", &unit_decls(&items, "dec"));
        assert!(refusals.is_empty(), "{refusals:?}");
        assert_eq!(ws.len(), 1);
        assert_eq!(ws[0].c_ret, None);
        let d = ws[0].definition();
        assert!(!d.contains("return"), "{d}");
        assert!(d.starts_with("void "), "{d}");
    }

    /// §7.6.1：编译器“整段抽出”源码原文 —— 包装 TU 里必须能看见用户写的字节。
    #[test]
    fn the_unit_translation_unit_keeps_the_user_source_verbatim() {
        let (s, items) = lowered(UNIT);
        let _ = s;
        let (ws, _) = wrappers("dec", &unit_decls(&items, "dec"));
        let tu = unit_c_source("dec", "static int g;\n", &ws);
        assert!(tu.contains("#include \"infy_binding.h\""), "{tu}");
        assert!(tu.contains("static int g;\n"), "user bytes must survive verbatim:\n{tu}");
        assert!(tu.contains(&ws[0].definition()), "{tu}");
    }

    /// C11 自检：生成的头 + 一段假的用户实现必须能被 `gcc -std=c11 -Wall -Wextra` 编过
    /// （`runtime/../target/scratch` 里跑；不可用时跳过并说明，不假报通过）。
    #[test]
    fn the_generated_header_and_wrapper_compile() {
        let (s, items) = lowered(UNIT);
        let _ = s;
        let (ws, refusals) = wrappers("dec", &unit_decls(&items, "dec"));
        assert!(refusals.is_empty(), "{refusals:?}");
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/scratch/atrack");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        std::fs::write(dir.join("infy_binding.h"), binding_header(&["dec".to_string()], &ws)).expect("header");
        let user = "int32_t decode(const uint8_t* p, uint32_t n) { return (int32_t)(n + (p != 0)); }\n";
        std::fs::write(dir.join("dec.c"), unit_c_source("dec", user, &ws)).expect("unit");
        let probe = std::process::Command::new("gcc")
            .args(["-std=c11", "-Wall", "-Wextra", "-c"])
            .arg(dir.join("dec.c"))
            .arg("-o")
            .arg(dir.join("dec.o"))
            .output();
        match probe {
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                assert!(o.status.success(), "gcc must accept the generated unit:\n{err}\n--- dec.c ---\n{}\n--- header ---\n{}", std::fs::read_to_string(dir.join("dec.c")).expect("read"), std::fs::read_to_string(dir.join("infy_binding.h")).expect("read"));
                assert_eq!(err.trim(), "", "the generated C must compile warning-free");
            }
            Err(e) => panic!("gcc is required for D3's acceptance (PATH: /c/msys64/ucrt64/bin): {e}"),
        }
    }

    /// 形参名进 C 原型与包装本体，必须过 `codegen` 那套标识符拼写（`register` 是 C 关键字）。
    #[test]
    fn parameter_names_use_the_back_ends_identifier_rule() {
        assert_eq!(c_ident("register"), "registerx", "a C keyword cannot be used as spelled");
        assert_eq!(c_ident("decode"), "decode");
        let src = "#[link(name = \"dec\")]\nextern \"C\" {\n    fn decode(a: u32, b: u8) -> u32;\n}\n";
        let (s, items) = lowered(src);
        let _ = s;
        let (ws, refusals) = wrappers("dec", &unit_decls(&items, "dec"));
        assert!(refusals.is_empty(), "{refusals:?}");
        assert_eq!(ws[0].param_names, vec!["a", "b"], "{:?}", ws[0].param_names);
        assert!(ws[0].definition().contains("decode(a, b)"), "{}", ws[0].definition());
    }
}
