# Infinity Language Specification · INFY Language Spec v1.0

> **文档状态**：PROPOSAL（待用户审签生效）
> **代号**：INFY / 因菲
> **文件后缀**：`.ifny`（源码）、`.ifnyny`（打包分发）、`.infypkg`（包发布）
> **冲突处理**：本规范是 INFY 语言的**唯一权威定义**。任何实现若与本规范冲突，以本规范为准；实现不得自行新增未定义行为（UB）。
> **合规要求**：一个实现若要称为「INFY v1.0 兼容」，必须 100% 通过本规范 §10 合规性测试套件（Conformance Suite）。

---

## 目录
1. 设计哲学与目标（Design Philosophy）
2. 基础约定（Lexical Conventions：编码、注释、标识符、字面量、关键字）
3. EBNF 形式文法（Formal Grammar）
4. 类型系统与语义（Type System & Semantics，**含 UB 禁令 §4.8**）
5. 模块与包系统（Modules & Packages：import / use / package / URL import）
6. 宏系统（Macro System：卫生 macro_rules!）
7. 多语言嵌入（Foreign Embedding：@py / @ps1 / @bat / @ts …）
8. 链与控制流（Chains & Control Flow：普通语句 vs 连携语句，**if=普通**）
9. 诊断引擎（Diagnostics Engine：错误码表 + 输出格式 + 修复建议规范）
10. 标准库（Standard Library API：`std.*` + `ai_evolve.std`）
11. 宿主交互协议（Host ↔ INFY Runtime：env 变量、ResourceLock FIFO）
12. 合规性测试套件（Conformance Suite）
13. 文件格式与 CLI（.ifny 编码 / infy 子命令规范）

---

## 1. 设计哲学与目标

### 1.1 五原则
| # | 原则 | 说明 |
| --- | --- | --- |
| P1 | **大气、通用** | 不只服务于 AI 进化项目；可作 MLOps、CI/CD、ETL、分布式任务、运维流水线的通用解释型工作流语言。 |
| P2 | **少 UB** | **禁止实现自行定义语义**。所有边缘情况在本规范中必有唯一定义（panic / saturate / error）。**UB = 合规性失败**。 |
| P3 | **错误先于运行** | 尽可能把错误推到 `infy check` 阶段（静态）。错误信息必须人类可读 + 机器可解析。 |
| P4 | **多语言即一等公民** | 任意**解释型语言**的代码块可直接嵌入为事件体。编译型语言（Rust/C/C++/Go/Java…）**强制禁用**。 |
| P5 | **零隐式** | 无隐式类型转换、无隐式变量声明、无隐式 return、无隐式 this/self。所有行为必须在源码中显式写出。 |

### 1.2 语言版本策略
- 语义化版本 `MAJOR.MINOR.PATCH`。
- MAJOR：破坏向后兼容的文法/语义变更。
- MINOR：新增标准库、新增可选语法（不破坏旧代码）。
- PATCH：修复规范 bug 与诊断文本。
- 所有 INFY 源码第一行**可选**声明 `#![infy_version = "1.0"]`；未声明视为"最新兼容"。解释器若遇到不兼容版本，必须输出 `E-0001`。

---

## 2. 基础约定（Lexical Conventions）

### 2.1 编码与换行
- **强制 UTF-8**，BOM 可选（推荐无 BOM）。非 UTF-8 源码 → `E-0002`。
- **强制 LF (`\n`)** 换行。CRLF → `W-0001`（警告，`infy fmt` 自动修复）。CR(Mac 老式) → `E-0003`。
- 文件结尾需一个 trailing LF。缺失 → `W-0002`。

### 2.2 注释
```
// line comment      行注释，至行尾
/* block comment */  块注释，可嵌套。未闭合块注释 → E-0004
/// doc comment      文档注释（包 / 模块 / event / fn / struct），生成文档
```
嵌套深度上限 255。超限 → `E-0005`。

### 2.3 标识符（Identifiers）
```ebnf
ident = XID_Start XID_Continue* ;
```
- 采用 Unicode XID（Annex #31）。**禁止** ASCII 之外的混淆字符（homoglyph 检测）。若 `NFKC` 归一化后与另一标识符冲突 → `E-0006`。
- 下划线 `_` 开头的标识符 = "内部使用约定"，但仍合法；`__` 双下划线前缀**保留**给解释器，用户定义双下划线 → `E-0007`。
- 大小写敏感。`Foo` ≠ `foo`。

### 2.4 关键字（Keywords）—— 保留，不可作标识符

| 类别 | 关键字 |
| --- | --- |
| 声明 | `package`, `import`, `use`, `mod`, `event`, `fn`, `struct`, `enum`, `union`, `const`, `static`, `let`, `var`, `macro_rules!` |
| 类型 | `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`, `f32`, `f64`, `bool`, `str`, `char`, `Self`, `self`, `true`, `false` |
| 控制流（普通） | `if`, `else`, `while`, `loop`, `for`, `in`, `break`, `continue`, `return`, `match` |
| 链/连携 | `chain`, `start_flag`, `goto`, `start`, `stop`, `pause`, `resume`, `linkage`（保留字） |
| 操作语义 | `as`, `in`, `is`, `where`, `pub`, `priv`, `unsafe`（保留） |
| 特殊前缀 | `@py`, `@ps1`, `@bat`, `@cmd`, `@ts`, `@js`, `@sh`, `@lua`, `@ruby`, `@php`（§7 白名单，不可重定义） |

### 2.5 字面量
- 整数：`123u32`, `0x_FFu8`, `0o777`, `0b1010i64`。未带后缀的十进制字面量 → 优先匹配参数类型；无法推断 → `E-0008`（歧义整数）。
- 浮点：`3.14f64`, `1.0e-5f32`。默认为 `f64`。
- 字符串：`"hello\nworld"`（支持 `\n \r \t \\ \' \" \0 \u{XXXX}` Unicode 转义）。未闭合 → `E-0009`。
- 原字符串：`r"C:\Users\me"`（不解析转义）。`r#" ... "#` 支持定界符数量递增，不可闭合 → `E-0010`。
- 布尔：`true`, `false`。
- `auto`：字面量标记（仅用于具名参数，语义由宿主事件定义，如 `generate_samples(count=auto)`）。

### 2.6 操作符（优先级从高到低，规范唯一确定）

| 优先级 | 操作符 | 结合性 |
| --- | --- | --- |
| 16 | `.` `()` `[]`（调用/索引/字段） | 左 |
| 15 | `as` `is` | 左 |
| 14 | 一元 `-` `!` `*` `&` | 右 |
| 13 | `*` `/` `%` | 左 |
| 12 | `+` `-` | 左 |
| 11 | `<<` `>>` | 左 |
| 10 | `&` | 左 |
| 9  | `^` | 左 |
| 8  | `\|` | 左 |
| 7  | `<` `<=` `>` `>=` | 左 |
| 6  | `==` `!=` | 左 |
| 5  | `&&` | 左 |
| 4  | `\|\|` | 左 |
| 3  | `..` `..=` | 左 |
| 2  | `=` `+=` `-=` `*=` `/=` `%=` `<<=` `>>=` `&=` `^=` `\|=` | **右** |
| 1  | 语句级 `,` `;` | — |

**实现不得调整优先级或结合性**。调整 = 不合规。

---

## 3. EBNF 形式文法

> 本规范采用 EBNF，每条规则有唯一编号。解析歧义若不能由 LL(2) / LR(1) 判定，必须在 §3.5 中指定唯一消歧策略。

```ebnf
(* 3.1 顶层：一个 .ifny 源码文件 = 一个模块 *)
module = package_header? import_item* top_item* ;

package_header = "package" ident ( "." ident )* ("version" "=" str_lit)? ";" ;

import_item = "import" import_path ( "as" ident )? ( "{" ident ( "," ident )* ","? "}" )? ";" ;
import_path = import_std | import_rel | import_url ;
import_std = "std" ( "." ident )* ;
import_rel = ( "./" | "../" )+ ( ident ( "/" ident )* ( ".ifny" )? ) ;
import_url = str_lit (* 必须以 http:// / https:// / git:// 开头，允许 @version *) ;

(* 3.2 顶层项 *)
top_item = event_decl
         | fn_decl
         | struct_decl
         | enum_decl
         | const_decl
         | static_decl
         | macro_decl
         | chain_decl ;

(* 3.3 事件 / 函数声明 *)
event_decl = "pub"? "event" ident "(" param_list? ")" ( "->" type_ )? block_or_embed ;
fn_decl    = "pub"? "fn"    ident "(" param_list? ")" ( "->" type_ )? block_or_embed ;
param_list = param ( "," param )* ","? ;
param      = ident ":" type_ ( "=" expr )? ;   (* = 默认值 *)

block_or_embed = block | embed_block | ";" ;   (* ; = 外部实现，由宿主提供 *)
block = "{" statement* ( expr ";"? )? "}" ;

(* 3.4 类型 *)
type_ = prim_type
      | ident                                 (* 用户 struct / enum / 类型别名 *)
      | "&" type_                             (* 引用，禁止空悬 *)
      | "[" type_ ";" int_lit "]"             (* 定长数组 [u32; 16] *)
      | "[" type_ "]"                         (* 动态数组 [u32] *)
      | "Option" "<" type_ ">"
      | "Result" "<" type_ "," type_ ">"
      | "fn" "(" type_list? ")" ( "->" type_ )?
      | "(" type_list? ")"                    (* 元组 / unit *)
      ;
prim_type = "u8" | "u16" | "u32" | "u64"
          | "i8" | "i16" | "i32" | "i64" | "i128" | "u128"
          | "f32" | "f64" | "bool" | "str" | "char" | "()" | "Self" ;
type_list = type_ ( "," type_ )* ","? ;

(* 3.5 语句：分普通语句与连携语句（见 §8） *)
statement = stmt_common ";"? ;
stmt_common = let_stmt
            | var_stmt
            | assign_stmt
            | expr_stmt                           (* 函数/事件/嵌入调用 *)
            | if_stmt
            | while_stmt
            | loop_stmt
            | for_stmt
            | match_stmt
            | break_stmt | continue_stmt | return_stmt
            | pause_stmt | stop_stmt             (* 可作普通语句 *)
            | linkage_block                       (* 连携挂载块：仅允许附加在 expr_stmt 后 *)
            ;

linkage_block = "{" linkage_stmt ( ";" linkage_stmt )* ";"? "}" ;
linkage_stmt = goto_stmt | start_stmt | start_flag_stmt
             | pause_stmt | stop_stmt | resume_stmt
             | if_stmt                           (* if 可嵌套于连携内，本身仍是普通语义 *)
             | expr_stmt ;                       (* 任意普通语句均允许在连携内执行 *)

let_stmt    = "let"  ident ( ":" type_ )? "=" expr ;
var_stmt    = "var"  ident ( ":" type_ )? "=" expr ;   (* 可变 *)
assign_stmt = expr "=" expr ;

if_stmt    = "if" "(" expr ")" block ( "else" ( block | if_stmt ) )? ;
while_stmt = "while" "(" expr ")" block ;
loop_stmt  = "loop" block ;
for_stmt   = "for" ident "in" expr block ;
match_stmt = "match" "(" expr ")" "{" match_arm ( "," match_arm )* ","? "}" ;
match_arm  = pat "=>" ( block | expr ) ;
break_stmt    = "break" ( expr )? ;
continue_stmt = "continue" ;
return_stmt   = "return" ( expr )? ;
pause_stmt    = "pause" ;
stop_stmt     = "stop" ;
resume_stmt   = "resume" ;
goto_stmt     = "goto" "(" goto_target ")" ;
goto_target   = expr                              (* 数字：绝对节点索引 / 字符串：命名锚点 *)
              | "+" int_lit | "-" int_lit ;       (* 相对偏移 *)
start_stmt    = "start" "(" "chain" "=" ident ( "," ("node" "=" ( int_lit | ident )) )? ")" ;
start_flag_stmt = "start_flag" ;

chain_decl = "pub"? "chain" ident block ;

(* 3.6 多语言嵌入块 §7 *)
embed_block = "@" lang_name ( "(" embed_arg ( "," embed_arg )* ","? ")" )? "{" embed_source "}" ;
lang_name = "py" | "ps1" | "bat" | "cmd" | "ts" | "js" | "sh" | "lua" | "ruby" | "php" ;   (* 白名单，编译型禁入 *)
embed_arg = ident "=" expr ;
embed_source = (* 原文语言源码，只按配对大括号扫描，不作语义解析 *) ;

(* 3.7 宏（简化 Rust 风格卫生宏）*)
macro_decl = "macro_rules!" ident "{" macro_arm ( ";" macro_arm )* ";"? "}" ;
macro_arm  = "(" macro_pat ")" "=>" block ;
macro_pat  = (* 见 §6 *) ;

(* 3.8 表达式（优先级见 §2.6）*)
expr = literal
     | ident
     | expr "." ident                     (* 字段：env.gen *)
     | expr "(" expr_list? ")"            (* 调用 *)
     | expr "[" expr "]"                  (* 索引 *)
     | unary_op expr
     | expr binary_op expr
     | expr "as" type_
     | "(" expr_list? ")"                 (* 分组 / 元组 *)
     | "[" expr_list? "]"                 (* 数组字面量 *)
     | struct_literal
     | if_expr
     | block ;
expr_list = expr ( "," expr )* ","? ;

literal = int_lit | float_lit | str_lit | char_lit | bool_lit | "auto" ;
struct_literal = ident "{" struct_field ( "," struct_field )* ","? "}" ;
struct_field = ident ":" expr ;
```

### 3.5 消歧策略（唯一）
- `goto(+1)` 的 `+1` / `-1` 必须**优先识别为 goto 相对偏移**，而不是一元 + int_lit。
- `if (a) { b } else { c }` 中的 `else` 始终**绑定到最近的未匹配 `if`**（C 经典消歧）。
- `foo.bar = baz` 中 `.` 优先级高于 `=`，等价于 `(foo.bar) = baz`。

---

## 4. 类型系统与语义

### 4.1 类型全集（规范定义，实现不得自定义）

| 类型 | 位宽 | 初值（`var` 未初始化 = 编译错误 E-0101） |
| --- | --- | --- |
| `u8/u16/u32/u64/u128` | 8/16/32/64/128 unsigned | —（声明必须赋值） |
| `i8/i16/i32/i64/i128` | 8/16/32/64/128 signed | — |
| `f32/f64` | IEEE 754-2008 binary32/binary64 | — |
| `bool` | 1（逻辑） | — |
| `str` | 动态 UTF-8 字符串（不可变；可变用 `String` stdlib） | — |
| `()` | 0（unit） | `()` |
| `[T; N]` | `sizeof(T) * N` | —（必须数组字面量初始化） |
| `[T]` | 动态（胖指针：ptr + len） | — |
| `struct { ... }` | 字段顺序对齐（`#[repr(C)]` 仅在显式标注时允许） | — |
| `enum` | 最小容纳判别式的整数 + payload | — |
| `Option<T>` | `Some(T) | None`（`null` 不存在于 INFY） | — |
| `Result<T,E>` | `Ok(T) | Err(E)`（异常无 throw，所有错误必须返回 Result） | — |
| `fn(A,B) -> R` | 函数指针（捕获需闭包，保留字 `closure` 供 v1.1） | — |

### 4.2 静态类型检查（infy check 必须通过）
**所有类型检查静态完成**，运行时不做额外类型验证（除了 `@embed` 跨语言边界）。
- 检查规则：赋值、函数参数、返回值 100% 类型兼容。
- **UB-001 禁令**：**禁止任何隐式数字转换**。`u32` + `u8` → `E-0110`，必须显式 `x as u32`。
- **UB-002 禁令**：`f32` ↔ `u32` 隐式互转 → `E-0111`。
- 数字字面量后缀匹配（§2.5）。无后缀字面量可匹配单义上下文；多义上下文 → `E-0008`。

### 4.3 可变性
- `let` = 不可变（绑定），修改 → `E-0120`。
- `var` = 可变，类型必须完整声明或从初始化完整推断。
- 跨链共享变量：默认 `&T` 不可变引用；可变跨链共享必须通过 **`std.sync.Mutex<T>`** 显式加锁，直接访问未加锁的跨链可变共享 → `E-0121`。

### 4.4 运算符语义
| 操作 | 语义（规范唯一） |
| --- | --- |
| `+ - * / %`（整数） | **UB-003 禁令**：溢出/除 0 **不得 UB**。按 `PACKAGE.infy.toml`：`overflow = "panic"`（默认）→ 运行时 panic（诊断码 `R-0001`）；`overflow = "saturate"` → 饱和。 |
| `+ - * / %`（浮点） | IEEE 754 定义（含 `inf`/`nan`，但 `nan == nan` → `false` 严格）。 |
| `&& / ||` | **短路求值**。右侧有无副作用均按短路执行（实现不得优化执行）。 |
| `==` 对 `str` | 按 Unicode NFC 逐字节比较（非 `==` 指针）。 |
| `as` 转换 | 数字→数字：按 Rust `as` 语义（截断/位级重解释 规范固定）。失败不 panic（除了 `bool as u8` 定义为 0/1，其余 `as` 定义为 bit-cast/truncate）。 |

### 4.5 内置函数（纯函数，可编译期求值）

```
std.math.abs(x: T) -> T              整数/浮点绝对值
std.math.roundN(x: f64, N: u32) -> u64   按 N 取整（N≥10；否则 E-0130）。也写作 round10/round50 ...
std.math.min(a: T, b: T) -> T
std.math.max(a: T, b: T) -> T
std.math.clamp(v: T, lo: T, hi: T) -> T
std.mem.size_of<T>() -> u32
std.mem.align_of<T>() -> u32
std.meta.type_name<T>() -> &str
std.env.tick() -> u64                 单调时钟（毫秒，启动=0）
```

### 4.6 `env` 全局只读宿主变量（不可修改）
规范全集：

| 变量路径 | 类型 | 语义（宿主必须实时注入） |
| --- | --- | --- |
| `env.gen` | u32 | 当前 AI 世代号（无 AI 场景 = 主循环次数） |
| `env.runtime_s` | u32 | INFY VM 已运行秒数 |
| `env.realtime` | u64 | Unix 时间戳（秒） |
| `env.ram_mb` | u32 | 宿主 Runtime 常驻集内存（MB） |
| `env.cpu_use` | f32 | 整机 CPU 使用率 [0.0, 1.0]（宿主采样） |
| `env.gpu_use` | f32 | 整机 GPU 使用率 [0.0, 1.0]（无 GPU = 0.0） |
| `env.sample_unconsumed` | u32 | 样本库未消耗数量（无样本库 = 0） |
| `env.chain_id` | u32 | 当前执行链 ID（宿主分配，入口链 = 0） |
| `env.node_idx` | u32 | 当前执行节点在链内索引（从 0 开始） |
| `env.branch_id` | &str | 当前分支 ID（read.txt 3.7） |
| `env.platform` | &str | `"win10" / "win11" / "linux" / "macos"` |

对 `env.xxx` 赋值 → `E-0140`（只读）。

### 4.7 内存模型
- **禁止空悬引用**：`&T` 的生命周期必须静态覆盖或由编译器报错（INFY v1.0 不支持生命周期标注，故只允许**栈内 &T**。堆引用强制走 `Box<T>` / `Rc<T>` stdlib）。
- **UB-004 禁令**：实现不得 GC 时随意移动对象（栈对象地址稳定）。

### 4.8 **UB 禁令清单**（任何触发 = 合规性失败）

| UB 编号 | 禁止行为 | 规范要求（必须的确定性结果） |
| --- | --- | --- |
| UB-001 | 隐式类型转换 | 必须 `E-0110` 编译错误 |
| UB-002 | f32↔u32 隐式互转 | 必须 `E-0111` 编译错误 |
| UB-003 | 整数溢出未定义 | 必须 panic 或 saturate（package 配置），不能"wrap"除非显式 `wrapping_add()` |
| UB-004 | 除 0 静默忽略 | 必须运行时 `R-0002` panic |
| UB-005 | 未初始化变量读取 | 必须 `E-0101` 编译错误 |
| UB-006 | 跨链无锁可变共享 | 必须 `E-0121` 编译错误 |
| UB-007 | NUL 注入 `\0` 出现在 str 中间 | 构造时必须 `E-0150`；运行时检查须 `R-0003` |
| UB-008 | 数组越界访问静默 | 必须运行时 `R-0004` panic |
| UB-009 | 空指针解引用 | INFY 无 `null`；`Option::unwrap()` 在 None 时必须 `R-0005` |
| UB-010 | 编译型语言嵌入（Rust/C/C++/Go 等） | 必须 `E-0701`（不在白名单） |
| UB-011 | 连携语句独立出现于 chain body（非挂载于宿主） | 必须 `E-0300` 见 §8 |
| UB-012 | 诊断输出实现不一致 | 必须严格符合 §9（错误码、模板、修复建议、高亮位置） |
| UB-013 | goto 跳到不存在的节点静默 | 必须运行时 `R-0010` panic |
| UB-014 | `infy fmt` 官方风格不唯一 | 对同一 AST 必须输出字节级唯一格式化结果 |
| UB-015 | 宏展开泄漏作用域（非卫生） | 必须在展开后类型检查阶段失败或重新命名 |

---

## 5. 模块与包系统

### 5.1 Package（最小分发单元）
一个包目录：
```
my_pkg/
  PACKAGE.infy.toml      (* 必选。UTF-8，TOML v1.0 *)
  lib.ifny               (* 可选。默认入口模块。若无，import 需指定文件。 *)
  submod/
    a.ifny
    b.ifny
```
`PACKAGE.infy.toml` 规范：
```toml
[package]
name = "my_pkg"                    (* 合法包名 = ident，ASCII 小写 + 下划线 + 数字，首字母不能数字 *)
version = "1.2.3"                  (* semver *)
authors = ["Alice <a@b.com>"]
edition = "infy-1.0"               (* 语言版 *)
overflow = "panic" | "saturate"    (* §4.4 整数溢出策略 *)
[dependencies]
"std" = "1.0"                      (* 内置标准库 = 强制依赖 *)
"github.com/user/pkg" = "1.5"
[profile]
diagnostic_level = "strict"        (* strict / lenient。strict：W-xxxx 不得视为无错 *)
```

### 5.2 解析顺序（规范唯一，不可自定义）
`import foo.bar.baz` 解析：
1. 若是 `std.*` → 内置标准库虚拟包（不得被本地包同名覆盖）。
2. 若是相对路径 `"./xxx"` / `"../xxx"` → 相对当前 `.ifny` 所在目录查找。
3. 若是 URL `"https://..."` → 首次下载到 `%LOCALAPPDATA%/Infinity/infy_pkg_cache/`（校验内容哈希），后续命中缓存。URL 内容哈希变动 → 校验失败 → `E-0200`。
4. 查找顺序：`dependencies` 表中声明的包。未声明的导入 → `E-0201`。
5. 都找不到 → `E-0202`。

### 5.3 可见性（`pub` 关键字）
未标注 `pub` 的 `event / fn / struct / enum / const / chain` = **模块私有**。其他模块引用私有项 → `E-0210`。

---

## 6. 宏系统（卫生 macro_rules!）

### 6.1 语法（复用 §3.7 EBNF）
```infy
macro_rules! vec {
    ()               => { [] };
    ($x:expr)         => { [$x] };
    ($x:expr, $($y:expr),+ $(,)?) => {
        let mut arr = [$x];
        $(arr.push($y);)*
        arr
    };
}
```

### 6.2 片段分类器（Fragment Specifiers）
| 分类 | 匹配 |
| --- | --- |
| `:expr` | 表达式 |
| `:stmt` | 语句 |
| `:block` | 块 |
| `:pat` | 模式 |
| `:ident` | 标识符 |
| `:ty` | 类型 |
| `:tt` | 单 token tree |
| `:meta` | 元属性（`#[...]` 内部） |

### 6.3 卫生性（Hygiene）
- 宏内部定义的标识符，外部不可见；外部传入的标识符保留其原作用域。
- 违规（泄漏）→ `E-0400`。

### 6.4 递归深度
- 宏展开递归深度上限 128。超过 → `E-0401`。

---

## 7. 多语言嵌入（Foreign Embedding · 白名单）

> **核心规则**：仅解释型语言可嵌入。编译型语言（Rust/C/C++/Go/Java/Swift/Kotlin/C#/Zig/Odin…）**绝对禁止**嵌入，即 §4.8 UB-010，触发即 `E-0701`。

### 7.1 嵌入白名单（v1.0）
`@py` / `@ps1` / `@bat` / `@cmd` / `@ts` / `@js` / `@sh` / `@lua` / `@ruby` / `@php`。

不在白名单中的 `@xxx` → `E-0702`。

### 7.2 嵌入通用执行协议（所有语言一致）
```infy
event add(a: u32, b: u32) -> u32 {
    @py(a=a, b=b) {
        # ← 这里是 Python 原语法，INFY 解释器不解析
        # 通过 env 注入上下文：输入参数 = 全局变量；返回通过 env.return(x)
        result = int(a) + int(b)
        env.return(result)
    }
}
```

对所有嵌入语言：
1. **沙箱执行**：独立子进程（而非同进程加载 DLL）。INFY Runtime 通过 `stdio + JSON-RPC 2.0` 与子进程通信。
2. **参数传递**：INFY `ident=expr` 实参 → JSON 值 → 注入为子脚本全局变量。
3. **返回值**：子脚本必须通过 `env.return(value)` 写入 stdout JSON-RPC。多返回值 = `env.return(tuple)` → INFY 元组类型。
4. **超时**：可配置 `@py(timeout_ms=60_000, ...) { ... }`；超时 → 杀子进程 → 运行时 `R-0701`。
5. **stderr**：嵌入脚本 stderr → 映射为 INFY `WARN` 级日志（附行号）。

### 7.3 每种嵌入的 Runtime 需求
| 嵌入 | 宿主必须预装（首次启动由主控自动拉取） |
| --- | --- |
| `@py` | CPython ≥ 3.11（embed 版，与 PyTorch 版本锁定） |
| `@ps1` | Windows PowerShell ≥ 5.1 / pwsh ≥ 7（Win10/11 自带） |
| `@bat/@cmd` | cmd.exe（Win 自带） |
| `@ts/@js` | Deno ≥ 1.40 或 Bun ≥ 1.1（主控可选拉取） |
| `@sh` | Git Bash / WSL bash（主控可选拉取 Git for Windows） |
| `@lua` | Lua ≥ 5.4 |
| `@ruby` | Ruby ≥ 3.2 |
| `@php` | PHP ≥ 8.2 |

运行时缺失 → `R-0702`（友好提示：可通过 `infy setup @lua` 自动下载）。

---

## 8. 链与控制流

### 8.1 分类（严格按用户最新决策：`if` = 普通语句）
| 类别 | 允许出现位置 | 包含 |
| --- | --- | --- |
| **普通语句（Statement）** | `chain { ... }` 顶层 / 块内 / 连携块内 | 赋值、`if-else`、`while`、`loop`、`for`、`break`、`continue`、`return`、`var/let`、`@embed`、事件/fn 调用、`pause`、`stop` |
| **连携语句（Linkage Statement）** | **只能**出现在某语句尾部的 `linkage_block = { ... }` 内 | `goto(target)`、`start(chain=?, node=?)`、`start_flag`、`resume`；以及嵌套在里面的任意普通语句（if 可嵌套） |

> **用户关键指令**：`if` 是普通语句。它可以独立成为节点，也可以嵌套在连携块中（仍然是普通语义，只是执行时机变成宿主完成后）。

### 8.2 链（Chain）生命周期
- 链在 `chain Foo { ... }` 声明时**注册**，不自动执行。
- **入口链**：`package` 所在源码文件中**第一个声明的 chain**（或通过 `#![entry_chain = "MonitorChain"]` 属性）。
- **自动滑进**：链当前节点的**最后一个普通语句**及其所有连携块执行完毕后，焦点 `node_idx += 1`（若越界 = 链结束）。
- **`goto` 语义**（移动焦点，暂停当前节点之后）：
  - `goto(5)` = 跳到本链索引 5。
  - `goto("anchor_name")` = 跳到本链 `start_flag` 或命名标签（命名标签保留字 v1.1 `label xxx:`）。
  - `goto(+3)` / `goto(-1)` = 相对当前 node_idx。
  - 目标不存在或超出范围 → §4.8 UB-013 → `R-0010` panic。
- **`start` 语义**（不移动焦点，并行新增）：
  - `start(chain = MonitorChain)` = 启动 `MonitorChain` 的焦点从其入口节点执行；当前链焦点继续原位置推进。
  - `start(chain = C, node = 5)` 或 `node = "anchor"` 指定起始节点。
  - 对已执行中链再次 start：**重入安全**——启动第二条独立焦点，互不干扰（同链多焦点并行）。
- **`start_flag`**：无操作 NOP，仅作为锚点。
- **`stop` 语义**：**普通语句即可使用**，也可作连携。执行后所有链**在完成当前节点的全部普通+连携动作后**优雅停止（非强杀）。
- **`pause` / `resume`**：暂停 = 完成当前节点后挂起，不释放模型/资源；resume 从焦点位置继续。

### 8.3 ResourceLock FIFO 调度（全局）
- 资源名：任意字符串（例 `"replica:3:training"`、`"sample_pool::gomoku"`、`"classroom2::compute"`）。
- 通过 `std.chain.lock(resource: &str)` 函数在进入事件前排队，退出释放。
- 严格按提交时间戳 FIFO 互斥。同资源并发 → 后提交者挂起直到前持有者释放。公平性：排队不得饥饿（无等待者跳过）。

---

## 9. 诊断引擎

### 9.1 错误分类与编号范围
| 前缀 | 级别 | 含义 | 范围 |
| --- | --- | --- | --- |
| `E-` | 错误 | **必须阻断运行**。检查不通过不可执行。 | 0000–4999 |
| `W-` | 警告 | 默认阻断（strict）；lenient 允许。 | 0000–1999 |
| `R-` | 运行时 panic | 执行期确定性终止（不是 UB，是规范 panic）。 | 0000–2999 |
| `N-` | 通知 | 仅信息（fix-it 成功 / `infy fmt` 改动统计）。 | 0000–999 |

### 9.2 输出格式（规范唯一，实现必须字节级一致）

```
<level>[<code>]: <title>
  ┌─ <file>:<line>:<col>
  │
<L>│     <source line>
  │               ^^^^^^^ <pointer_label>
  │
  = note: <note>
  = help: <help_text>
  │
<S>│     <suggestion source line with +++++ insertion>
  │     <+> 表示新增 / <-> 表示删除
```

**示例**（§5.10 myplan.md 中相同，必须一致）：
```
error[E-0314]: 类型不匹配：期望 u32，实际 f32
  ┌─ foo.ifny:12:18
  │
12│     var a: u32 = gen * 1.5;
  │                  ^^^^^^^^^  表达式类型为 f32
  │
  = note: `gen` 类型为 u32，乘以 f32 字面量后类型提升为 f32
  = help: 使用 round10() 或显式 as 转换
  12│     var a: u32 = round10(gen * 1.5) as u32;
  │                     ++++++++           ++++++
```

### 9.3 常用错误码表（v1.0 全集）

| 码 | 标题 | 触发场景 | 修复建议 |
| --- | --- | --- | --- |
| E-0001 | 语言版本不兼容 | `infy_version = "2.0"` 在 INFY v1.0 解释器上 | 升级解释器或降级源码版本声明 |
| E-0002 | 源码非 UTF-8 | 文件存在非法字节序列 | 转为 UTF-8 |
| E-0003 | CR 换行 | Mac 老式 CR 换行 | 改为 LF（`infy fmt` 自动修复） |
| E-0004 | 未闭合块注释 | `/*` 无匹配 `*/` | 补全闭合 |
| E-0005 | 注释嵌套过深 | >255 层 | 简化注释 |
| E-0006 | 混淆标识符 | NFKC 归一冲突 | 更换标识符 |
| E-0007 | 保留前缀标识符 | `__foo` 用户定义 | 删除前置 `__` |
| E-0008 | 歧义整数字面量 | 未加后缀且无法推断 | 加后缀 `123u32` |
| E-0009 | 未闭合字符串 | 缺少 `"` | 补全 |
| E-0010 | 未闭合原字符串 | `r#"` 定界不匹配 | 增加 `#` 数量 |
| E-0101 | 未初始化变量读取 | `var x: u32;` 读取 | 初始化 |
| E-0110 | 隐式数字转换 | `u32 + u8` | 显式 `as` |
| E-0111 | 隐式浮点/整数互转 | 同上 | 显式 roundN / as |
| E-0120 | 修改 let 不可变绑定 | `let x = 1; x = 2;` | 改 `var` |
| E-0121 | 跨链无锁可变共享 | 多链写同一 `var` | 用 `std.sync.Mutex<T>` |
| E-0130 | roundN N<10 | `round3(x)` | 用 ≥10 的 N |
| E-0140 | 写入 env 只读变量 | `env.gen = 5` | 禁止写入 |
| E-0150 | 字符串含 NUL 注入 | `"a\0b"` 在跨边界前未剥离 | `std.str.strip_nul` |
| E-0200 | URL 包哈希变更 | 远端内容变化 | 显式升级版本号 |
| E-0201 | 未声明的依赖导入 | import 了 dependencies 未列的包 | 在 PACKAGE.infy.toml 加依赖 |
| E-0202 | 导入目标不存在 | 路径/URL 404 | 修正路径 |
| E-0210 | 访问私有项 | 其他模块非 pub 项 | 加 pub 或改 API |
| E-0300 | 连携语句独立出现（UB-011） | `goto(5);` 直接写在 chain 顶层而未挂载 `{}` | 包在宿主 `stmt { goto(5); };` 中 |
| E-0400 | 宏作用域泄漏 | 卫生宏展开冲突 | 重命名宏内标识符 |
| E-0401 | 宏展开深度超限 | >128 | 简化递归 |
| E-0701 | 编译型语言嵌入（UB-010） | `@rust` / `@c` | 改用解释型；或外部预编译 + `@bat` 调 EXE |
| E-0702 | 未知嵌入语言 | `@forth` | 使用白名单内嵌入 |
| E-0800 | 链不存在 | `start(chain = NoSuch)` | 修正链名 / 先声明 |
| E-0900 | 自定义事件参数不匹配 | 调用时缺必填参数 | 补全参数或加默认值 |

**运行时 panic（R-xxxx）**：
| 码 | 含义 |
| --- | --- |
| R-0001 | 整数溢出（`overflow=panic`） |
| R-0002 | 除 0 |
| R-0003 | str NUL 运行时检测到 |
| R-0004 | 数组越界 |
| R-0005 | `Option::unwrap()` / `Result::unwrap()` 在 None / Err |
| R-0010 | goto 目标不存在 |
| R-0701 | 嵌入执行超时 |
| R-0702 | 嵌入 runtime 缺失 |

### 9.4 `infy check` 命令
- 语义：**只检查不执行**。输入 `.ifny` + 所有 transitive 依赖。
- 退出码：`0`（零 E/W-strict）、`1`（有 E）、`2`（strict 模式下有 W）。
- 所有错误按规范 §9.2 输出到 stderr（JSON 结构化副本输出到 `--diag-json foo.json`，CI 用）。

---

## 10. 标准库 API（v1.0）

### 10.1 std.chain
```
std.chain.id() -> u32                 当前 env.chain_id
std.chain.node_idx() -> u32           当前 env.node_idx
std.chain.lock(resource: &str) -> Guard   获取 FIFO 锁（scope 结束释放）
std.chain.yield_now()                 让出当前调度片（公平性）
goto(target) / start(chain=?, node=?)   作为关键字，不在库中
stop / pause / resume / start_flag     关键字
```

### 10.2 std.math
`abs / min / max / clamp / roundN / sqrt / pow / sin / cos / tan / log / ln` 等。

### 10.3 std.fs
`read_to_str(path) -> Result<&str, IoErr>`、`write_file(path, content)`、`exists(path)`、`mkdir_p(path)`。

### 10.4 std.net
`http_get(url)`、`http_post(url, body_json)`、`websocket_connect(url)`。

### 10.5 std.process
`exec(cmd: &str, args: [&str], timeout_ms: u32) -> ExecResult { stdout, stderr, code }`。

### 10.6 std.sync
`Mutex<T>::new(value)`、`.lock()`、`.unlock()`；`Channel<T>` 多链消息队列。

### 10.7 ai_evolve.std（本项目专用标准库，随 Infinity 分发）
```infy
event variant_tournament(variant_id: u32, loops: u32 = 1, copies_each: u32 = 5) -> TournamentResult;
event inter_variant_championship(loops: u32 = 1, copies_each: u32 = 5) -> CrossVariantResult;
event generate_samples(count: u32 | auto, game_type: &str = "auto") -> GeneratedReport;
event train(rounds: u32, lr: f32 = 0.001, batch_size: u32 = 64) -> TrainReport;
event var(name: &str, initial: u32) -> ();   // 兼容旧语法；INFY 原生 var 更好
event variant2_lock_component(cid: &str) -> LockReport;
event variant2_retrospect_check() -> RetrospectReport;
```

---

## 11. 宿主 ↔ INFY Runtime 交互协议
- 宿主（Rust infinity_runtime / Python embed server）与 INFY VM 通过 `HostApi` Trait 通信：
  ```rust
  pub trait HostApi {
      fn sample_env(&self) -> EnvSnapshot;  // §4.6 注入
      fn call_native_event(&mut self, name: &str, args: &[Value]) -> Result<Value, HostErr>;
      fn acquire_resource(&mut self, res: &str, ts: u64);
      fn release_resource(&mut self, res: &str);
      fn spawn_chain(&mut self, chain_id: u32, start_node: i64);
      fn request_pause(&mut self);
      fn request_stop(&mut self);
  }
  ```
- 调用 Native 事件返回错误必须携带**可映射的 INFY 诊断码**（`E-0900`-系列）。

---

## 12. 合规性测试套件（Conformance Suite）

`infy conformance` 子命令运行规范附带的 **1024 个测试用例**（分类如下）：
| 分类 | 数量 | 检查 |
| --- | --- | --- |
| §2 Lexical | 80 | 编码、标识符、字面量、注释 |
| §3 Grammar | 256 | 每条 EBNF 规则的正反例 |
| §4 Type & UB | 200 | 每条 UB 禁令必须被识别 |
| §5 Modules | 80 | 导入、可见性、缓存 |
| §6 Macros | 100 | 卫生性、深度、片段分类 |
| §7 Embedding | 64 | 每类嵌入的 stdout/stderr/timeout/返回值 |
| §8 Chains | 128 | goto/start/stop/pause、FIFO |
| §9 Diagnostics | 64 | 错误码/格式字节级对比 |
| §10 Stdlib | 48 | 每个 API 的边界用例 |
| 合计 | 1020+ |  |

合规率 100% = 才可声称 INFY v1.0 兼容。98% 以下 = 不合规。

---

## 13. 文件格式与 CLI 规范

### 13.1 `.ifny` 源码
- UTF-8，LF，trailing LF。
- 首行 `#! /usr/bin/env infy` 允许（shebang 被解释器忽略）。

### 13.2 `.ifnyny` 打包分发
- 格式：ZIP（store，不压缩单个文件，整体 zstd 压缩层）。
- 结构：`META.infy.toml` + `modules/<hash>.ifny` + `embedded/*`。
- `infy pack foo.ifny -o app.ifnyny`；`infy unpack app.ifnyny -d out/`。

### 13.3 `infy` CLI 全集（规范定义）
```
infy --version / -V               版本号（必须 INFY-v1.0.x 格式）
infy run <src.ifny> [--arg K=V]   执行
infy check <src.ifny> [--strict]  静态检查 + 诊断
infy fmt <src.ifny> [--write|-w]  官方格式化（字节级唯一结果）
infy pack <src.ifny> -o a.ifnyny  打包
infy unpack <a.ifnyny> -d <dir>   解包
infy setup @<lang>                安装嵌入语言 runtime
infy conformance [tests/]         运行合规套件
infy lsp                         启动 Language Server（IDE 用，可选）
infy doc <pkg> -o out/            生成 HTML 文档
```

退出码：
- 0 成功；1 一般性失败；2 检查失败；3 运行时 panic；4 权限错误；5 网络/嵌入下载失败。

---

## 附录 A：设计取舍的原理说明（非规范，解释性）
A1. **为什么要做独立语言而不是 YAML/JSON/TOML 配置？**  
YAML 能描述 DAG，但难以表达 if/else/循环/宏/模块化/跨语言嵌入。INFY 是一个解释型语言，而不是配置格式——它能在调试时单步执行断点、动态检查类型、给出修复建议，这是纯配置做不到的。

A2. **为什么 if 是普通语句而不是连携语句？**  
用户最新决策。它让链节点本身就可以是条件节点，而不必"先空跑一个 NOP 再在连携里 if"。语义更直观、调试更清晰。

A3. **为什么禁用编译型嵌入？**  
单文件 EXE 主控无法保证任意平台的 Rust/C++ 编译器可用，且编译耗时阻塞工作流。若需要调用编译型产物，推荐用 `std.process.exec("cl.exe / rustc.exe")` 外部调 EXE，而不是 INFY 直接嵌入。

A4. **为什么强制 FIFO ResourceLock？**  
多节点操作同一对象需保证结果可复现，调度不能"随机挑一个"；否则训练不可回溯，违反 read.txt 的可追溯性。

---

> **待审签**：本规范 v1.0 经用户审签后生效，后续修改走版本号迭代（v1.1 / v2.0），不覆盖历史版本。
