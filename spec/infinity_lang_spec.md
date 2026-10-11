# Infinity Language Specification · INFY Language Spec v2.0

> **文档状态**：PROPOSAL（待审签生效）
> **代号**：INFY / 因菲
> **本版定位**：**编译型重置版**。取代并废止 v1.0 提案（v1.0 从未审签生效，故本版为 MAJOR 递增，见 §1.2）。
> **语言类别**：**静态类型、编译型**（INFY 本体 AOT 到原生码；嵌入脚本在构建期**摄取源语言自身的字节码**并 lowering 为 IFBC，由产物内嵌运行时动态执行）。
> **参考实现**：Rust。内存安全语义对标 Rust（§4.7）。
> **文件后缀**：`.infy`（源码）、`.infib`（库文件：源码版 / wheel 版，见 §5.6）、`.infyny`（打包分发产物）、`.infypkg`（包发布）
> **冲突处理**：本规范是 INFY 语言的**唯一权威定义**。任何实现若与本规范冲突，以本规范为准；实现不得自行新增未定义行为（UB）。
> **合规要求**：一个实现若要称为「INFY v2.0 兼容」，必须 100% 通过本规范 §12 合规性测试套件（Conformance Suite），且其 codegen 必须通过 §12.3 黄金文件与可复现构建校验。

---

## 目录
1. 设计哲学与目标（Design Philosophy：**含 §1.3 领域中立原则**）
2. 基础约定（Lexical Conventions：编码、注释、标识符、生命周期记法、字面量、关键字）
3. EBNF 形式文法（Formal Grammar，**含嵌入/链接/宏文法 §3.6–§3.9**）
4. 类型系统、所有权与内存安全（Type System & Ownership，**含 UB 禁令 §4.9**）
5. 模块、包与库文件（Modules / Packages / `.infib`：Go 式分包、internal、依赖解析、校验与签名）
6. 宏与属性系统（Macros & Attributes）
7. 编译管线与多语言嵌入（Compilation Pipeline & Embedding：自动连接 A 轨 / 字节码摄取 B 轨 / IFBC）
8. 链与控制流（Chains & Control Flow：普通语句 vs 连携语句，**if=普通**）
9. 诊断引擎（Diagnostics Engine：错误码表 + 输出格式 + 修复建议规范）
10. 标准库（Standard Library API：`std.*`）
11. 宿主交互协议（Host ↔ INFY Runtime：进程内 HostApi、env 快照、ResourceLock FIFO）
12. 合规性测试套件（Conformance Suite）
13. 产物格式与 CLI（artifact 段布局 / `.infib` 容器 / `infy` 子命令规范）
14. 语言导览（完整可编译示例：module 布局 / 所有权 / 链与连携 / 双轨嵌入 / 测试）
- 附录 A 设计取舍的原理说明
- 附录 B v2.0 相对 v1.0 提案的变更清单（增删对照）
- 附录 C 未决项、风险与待决策取舍
- 附录 D IFBC 完整指令集
- 附录 E 数据文件格式（lock / manifest / 能力表 / 工具链 / diag-json）
- 附录 F 明确不支持清单、术语表与索引

---

## 1. 设计哲学与目标

### 1.1 六原则
| # | 原则 | 说明 |
| --- | --- | --- |
| P1 | **大气、通用，且编译型** | 不服务于任何单一领域或单一项目。CI/CD、ETL、构建编排、分布式任务、运维流水线、桌面自动化、嵌入式脚本宿主——都是它的目标场景。源码经 `infy build` 产出原生码，不在用户机器上解析源码。**语言层不含任何领域专属构造**（§1.3）。 |
| P2 | **少 UB** | **禁止实现自行定义语义**。所有边缘情况在本规范中必有唯一定义（panic / saturate / error）。**UB = 合规性失败**。 |
| P3 | **错误先于运行** | 尽可能把错误推到编译阶段（`infy check` / `infy build`）。**新增**：嵌入语言的类型不匹配、外部工具链缺失、`.infib` ABI 漂移、链接符号冲突，全部必须在编译期终止，不得留到运行时（§7）。错误信息必须人类可读 + 机器可解析。 |
| P4 | **多语言即一等公民（双轨）** | 嵌入不再是"起个子进程喂 JSON"。**B 轨**：解释型语言（Python/Lua/…）在构建期由**其自身工具链**编译成原生字节码，INFY 做一次 **lowering 转成 IFBC** 内嵌进产物，运行时动态执行——INFY 侧不写任何外部语言的解析器。**A 轨**：编译型语言（C/C++/Rust/Zig）由编译器**自动完成绑定生成与链接**，产物仍是单文件。两条轨道都由 §7 的编译器统一处理，用户不写胶水代码。 |
| P5 | **零隐式** | 无隐式类型转换、无隐式变量声明、无隐式 return、无隐式 this/self、无隐式跨语言指针移交（§7.6.4）。所有行为必须在源码中显式写出。 |
| P6 | **单文件自包含交付** | 一次 `infy build` 产出一个可直接执行的文件：原生码 + IFBC 字节码段 + 精简运行时 + 元数据 + 校验和清单，全部装进同一个容器（§7.4）。用户不需要安装 Python、Node、Lua，也不需要 `pip install`。这是本版改编译型最主要的收益，也是 A3 记录的关键决策。 |

### 1.2 语言版本策略
- 语义化版本 `MAJOR.MINOR.PATCH`。
- MAJOR：破坏向后兼容的文法/语义/**产物 ABI** 变更。
- MINOR：新增标准库、新增可选语法（不破坏旧代码）、新增嵌入语言（不改动已有嵌入语义）。
- PATCH：修复规范 bug 与诊断文本。
- **v1.0 与 v2.0 的关系**：v1.0 处于 PROPOSAL 状态、未经审签，因此 v2.0 不承担对 v1.0 的兼容义务。本文档全文即 v2.0 权威定义；v1.0 提案文本存档于 `infinity_lang_spec.v1-proposal.bak.md`，仅作决策史参考，不得作为实现依据。
- 所有 INFY 源码的**首个文件级属性行**（位于 shebang 与 `//infy:build` 之后、`package` 之前，见 §3.1）**可选**声明 `#![infy_version = "2.0"]`；未声明视为"最新兼容"。编译器若遇到不兼容版本声明，必须输出 `E-0001`。同一 module 内多个文件声明不一致 → `E-0001`。
- **产物 ABI 版本**独立于语言版本，记为 `IFABI-<n>`（写入产物 `.inf meta` 段，见 §7.4）。语言 MINOR 升级不得改变 `IFABI`；`IFABI` 变更 = MAJOR 变更。

### 1.3 领域中立原则（规范性）
INFY 是**通用语言**，不是任何单一用途的 DSL。因此本规范对"能出现在语言层的东西"设一条硬边界：

1. **语言层与 `std.*` 只含通用构造**：类型系统、所有权、链与连携、模块与库、嵌入、诊断、IO/进程/网络/并发原语。
2. **禁止出现领域专属标识符**：`std` 的任何路径下不得出现指向特定应用领域（数值计算、图形、交易、游戏、企业内部系统等）的类型名、函数名、`env` 字段或关键字。领域词汇只能出现在**用户 module 的名字里**。违反 = 实现不合规（UB-024 的组成部分）。
3. **领域能力只有三条合法出口**：
   - 普通 `.infib` 库（第三方或自有 module，经 §5 正常导入）；
   - 宿主以 `HostApi::call_native_event`（§11）注册的事件；
   - `@embed` 外部代码（§7.6/§7.7）。
   三者都在语言规范之外，规范不为任何一方预留命名空间，也不承诺它们存在。
4. **`event` / `chain` 是通用编排构造，不专属任何领域**——它们描述"可挂接的工作单元"与"工作单元的推进拓扑"，用于 CI、ETL、自动化同样成立。
5. 判据（写死，避免争议）：**把某构造从规范删除后，若只影响一个应用领域，则该构造就是领域专属的，不得进语言层。**

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
/// doc comment      文档注释（module / package / event / fn / struct），生成文档
```
嵌套深度上限 255。超限 → `E-0005`。

**构建约束注释（Go 式 build tag）**：`//infy:build <约束>` 必须是文件的**第一个非空行**且紧跟 shebang（若有），用于把文件排除出某个构建配置：
```
//infy:build linux                 仅 linux 目标参与编译
//infy:build windows && x86_64     与运算
//infy:build !a_track              取反（A 轨工具链不可用时仍可编译）
//infy:build ignore                永不参与构建（示例/草稿）
```
- 约束名全集：`windows` `linux` `macos` `x86_64` `aarch64` `msvc` `gnu` `a_track` `b_track` `debug` `release` + module 清单中 `features` 声明的自定义名。
- 表达式文法：`constraint = ident | "!" constraint | "(" constraint ")" | constraint "&&" constraint | constraint "," constraint`，`,` 与 `&&` 同义，`||` 表示或。
- 位置不在首行 → `E-0011`；未知约束名 → `E-0012`；同一文件多条约束注释 → `E-0013`；约束恒 false（文件必然被排除）→ `W-0003`。

### 2.3 标识符（Identifiers）
```ebnf
ident = XID_Start XID_Continue* ;
```
- 采用 Unicode XID（Annex #31）。**禁止** ASCII 之外的混淆字符（homoglyph 检测）。若 `NFKC` 归一化后与另一标识符冲突 → `E-0006`。
- 下划线 `_` 开头的标识符 = "内部使用约定"，但仍合法；`__` 双下划线前缀**保留**给编译器与生成的绑定代码使用，用户定义双下划线 → `E-0007`。
- 大小写敏感。`Foo` ≠ `foo`。**注意**：INFY 采纳 Go 式分包，但**不**采纳 Go 式"首字母大写即导出"；导出仍由显式 `pub` 决定（§5.5）。首字母大写仅为社区命名惯例，无语义。

### 2.4 关键字（Keywords）—— 保留，不可作标识符

| 类别 | 关键字 |
| --- | --- |
| 声明 | `module`, `package`, `import`, `pub`, `priv`, `mod`, `event`, `fn`, `struct`, `enum`, `union`, `trait`, `impl`, `type`, `const`, `static`, `let`, `var`, `macro_rules!` |
| 链接/外部 | `extern`, `unsafe`, `link`, `owns`, `borrows`, `label` |
| 类型 | `u8`, `u16`, `u32`, `u64`, `u128`, `i8`, `i16`, `i32`, `i64`, `i128`, `f32`, `f64`, `bool`, `str`, `char`, `dyn`, `Self`, `self`, `true`, `false` |
| 控制流（普通） | `if`, `else`, `while`, `loop`, `for`, `in`, `break`, `continue`, `return`, `match` |
| 链/连携 | `chain`, `start_flag`, `goto`, `start`, `stop`, `pause`, `resume`, `linkage`（保留字） |
| 操作语义 | `as`, `is`, `where`, `move`（显式移动，见 §4.4） |
| 嵌入前缀（B 轨·字节码摄取） | `@py`, `@lua`（§7.7.1 有稳定字节码形态者，不可重定义） |
| 嵌入前缀（B 轨·仅进程隔离） | `@ts`, `@js`, `@sh`, `@ps1`, `@bat`, `@cmd`（语法合法，但强制 `isolation="process"`，见 §7.7.5 R-JS） |
| 嵌入前缀（A 轨·自动连接） | `@c`, `@cpp`, `@rust`, `@zig`（§7.6 白名单，不可重定义） |

`dyn` 是类型构造关键字，只在 B 轨边界与 `std.embed` 中出现（§7.7.7）；`unsafe` 在 v2.0 由 v1.0 的"保留"转为**正式启用**（§4.7.5）。
`use`、`mod`、`union`、`linkage` 在本版文法中**未被使用**，但仍是保留字——占用命名空间以避免未来复用时静默破坏已有源码（`E-0007` 之外，用它们作标识符 → `E-0020`）。

### 2.5 生命周期记法（Lifetime Notation）
```ebnf
lifetime = "'" ( "static" | ident ) ;
```
- 形如 `'a`、`'src`、`'static`。与普通标识符同一 XID 规则，前缀 `'` 是其唯一区别。
- `'static` 为保留名，表示"覆盖整个进程生命周期"（指向只读数据段或常驻分配）。
- 生命周期名**不得**与任何类型/变量同名混淆：`'Foo` 合法但无意义，`infy fmt` 不重命名生命周期，但 `infy check` 对**未使用**的生命周期参数报 `W-1010`。
- 函数签名可省略生命周期标注，按 §4.7.3 的 elision 规则唯一确定；无法唯一确定 → `E-1003`（**不做**任何"编译器猜一个"的降级）。

### 2.6 字面量
- 整数：`123u32`, `0x_FFu8`, `0o777`, `0b1010i64`。未带后缀的十进制字面量 → 优先匹配参数类型；无法推断 → `E-0008`（歧义整数）。
- 浮点：`3.14f64`, `1.0e-5f32`。默认为 `f64`。
- 字符串：`"hello\nworld"`（支持 `\n \r \t \\ \' \" \0 \u{XXXX}` Unicode 转义）。未闭合 → `E-0009`。
- 原字符串：`r"C:\Users\me"`（不解析转义）。`r#" ... "#` 支持定界符数量递增，不可闭合 → `E-0010`。
- 布尔：`true`, `false`。
- `auto`：字面量标记，仅可用于**具名参数**位置（`fetch(retries=auto)`），表示"由被调用方或宿主决定的默认策略"。它不是任何类型，不能参与运算，不能出现在返回值/绑定位置（`E-0030`）。语义由被调用方的文档定义；语言层只规定：`auto` 与省略该实参**完全等价**，二者不得有行为差异。

### 2.7 操作符（优先级从高到低，规范唯一确定）

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

### 2.8 数值字面量（完整文法）
```ebnf
int_lit   = dec_lit | hex_lit | oct_lit | bin_lit , suffix? ;
dec_lit   = digit ( [ "_" ] digit_or_underscore )* ;
hex_lit   = "0x" xdigit ( [ "_" ] xdigit )* ;
oct_lit   = "0o" oct_digit ( [ "_" ] oct_digit )* ;
bin_lit   = "0b" bin_digit ( [ "_" ] bin_digit )* ;
float_lit = dec_lit ( frac | exp ) ;
frac      = "." dec_lit ;
exp       = ( "e" | "E" ) [ sign ] dec_lit ;
sign      = "+" | "-" ;
suffix    = "u8"|"u16"|"u32"|"u64"|"u128"|"usize"
          | "i8"|"i16"|"i32"|"i64"|"i128"|"isize"|"f32"|"f64" ;
```
规则（全部编译期强制）：
- **分隔符**：`_` 只是可读性分隔，预处理阶段剥离。连续两个 `_`、首 `_`、尾 `_` → `E-0040`；`_` 出现在 `.` 或 `e` 两侧 → `E-0041`。
- **十进制无后缀整型**：值超出 `i64` 且无后缀 → `E-0042`；超出 `i128` → `E-0042`。带后缀且超出该后缀范围 → `E-0043`（**不做**回绕，也不做饱和——回绕只属于运行时的 `wrapping_*`）。
- **非十进制**：允许无后缀；此时类型推导见 §4.13，默认按 `i32`（值超出 `i32` 时按 `u32`，仍超出则 `E-0042`）。`0x_FFu8` 与 `0xFFu8` 等价。
- **浮点**：`float_lit` 无后缀默认 `f64`。仅 `frac` 或仅 `exp` 均合法（`1.` 非法，必须写 `1.0` → `E-0044`）。`1.0e999f32` → `E-0045`（字面量溢出为 `inf` 被禁止，需显式 `std.f64.INF`）。
- **负数**：`-5` 是**一元负号 + 字面量 5**，不是负字面量；因此 `-128i8` 合法（负号作用于 `128i8`，而 `128i8` 本身溢出 → 特例规则：若一元负号的运算数在加宽后可精确表示且整体落在目标类型范围，编译期常量折叠允许）→ 其他形式（如 `-129i8`）→ `E-0043`。
- **`usize`/`isize`**：宽度由**产物 target** 决定（32 或 64 位），写入 `.inf meta`；跨 target 共享的 `.infib` 不得在 `pub` API 签名里使用 `usize`（→ `E-0605`），只允许在实现内部使用。这是 P6 单文件多 target 承诺的前提。

### 2.9 字符串与字节串（完整文法）
```ebnf
str_lit      = '"'  (* UTF-8 文本，允许下方转义 *) '"' ;
raw_str_lit  = "r" ( "#" )* '"'  (* 原文，无转义 *) '"' ( "#" )* ;
byte_str_lit = "b" '"'  (* 仅 ASCII 字节 *) '"' ;
char_lit     = "'"  ( 非'非\ 单字符 | escape ) "'" ;
cord_lit     = "c" '"' ( UTF-8 代码点序列 ) '"' ;   (* 供 §2.10 NFC 比较的字面量 *)
```
**转义全集**（`str_lit`/`char_lit` 通用；未在此表中的 `\X` → `E-0046`）：

| 转义 | 含义 | 转义 | 含义 |
| --- | --- | --- | --- |
| `\n` `\r` `\t` `\0` | 0x0A / 0x0D / 0x09 / 0x00 | `\\` `\"` `\'` | 反斜杠、双引号、单引号 |
| `\e` | ESC 0x1B | `\a` `\b` `\f` `\v` | BEL / BS / FF / VT |
| `\x41` | 单字节 0x41，**仅** `b".."` 与 `char_lit` 允许；在 `str_lit` 中 → `E-0047`（防止写出非法 UTF-8） | `\u{1F600}` | Unicode 代码点 1..6 位十六进制 |
| `\x{00FF}` | 显式花括号形式，同上限制 | — | — |

- **换行规则**：`str_lit` 内**不得**出现裸换行 → `E-0009`；跨行文本必须用 `raw_str_lit` 或多段拼接（`"a" "b"` 相邻字面量在编译期连接，这是规范定义的唯一隐式行为，且仅对字面量）。
- **多行 raw 的缩进剥离**：`r"..."` 若首字符为换行，则编译期按**最大公共前导空白**对所有后续行剥离缩进（Rust 生态惯例，但此处为强制定义，实现不得自由选择"保留原样"）。缩进只算空格与制表符，制表符按 1 个空白单元处理且不得与空格混用于同一公共前缀 → `E-0048`。
- `\0` 在 `str_lit` 中合法，但**跨 A 轨边界前**必须显式 `to_cstr()`（§7.6.4），构造期检查见 `E-0150`。
- **无插值语法**：`"${x}"` 明确**不**支持（P5 零隐式：插值会隐式调用格式化与隐式取所有权）。格式化一律显式：`fmt("{}, {}", a, b)`（§10.11）。`"{}` 形式出现在字符串中不做特殊处理。
- `char_lit` 必须是**单个 Unicode 代码点**；图符簇（如 `"e\u{301}"`）→ `E-0049`，应写进 `str`。

### 2.10 Unicode 规范化与标识符安全
- **规范化判定**：源码解析前，编译器对**标识符**做 `NFKC` 归一比对（不改写源码本身，只用于冲突检测）。两个标识符若 NFKC 后相同而原文不同 → `E-0006`。
- **字符串比较**：`==` 对 `str` 按 §4.4 的 NFC 比较；需要字节比较必须显式 `a.bytes() == b.bytes()`。
- **混淆字符（confusables）**：编译器维护 Unicode `Confusables.txt` 的**已声明版本**（写入 `.inf meta`）。规则：同一作用域内的两个标识符若互为混淆（skeleton 相同）→ `E-0050`。
- **同脚本混用**：一个标识符内的字母必须属于同一脚本，或属于允许的中性类（数字、`_`、`'`）。西里尔 `а` 与拉丁 `a` 混用 → `E-0050`。中文（Han）与假名混用允许（日语场景合法），但 Han 与谚文混用 → `E-0051`。
- **零宽与控制字符**：任何标识符、`str_lit` 之外的位置出现 `\u{200B}`..`\u{200F}`、`\u{202A}`..`\u{202E}`、`\u{FEFF}`（BOM 除外，见 §2.1）→ `E-0052`。
- **大小写**：标识符大小写敏感且**不做**折叠；`infy fmt` 永不重命名标识符。领域中立检查（§12.1）用 `NFKCCaseFold` 匹配领域词表，避免 `Numpy`/`numpy` 绕过。

### 2.11 空白、换行与语句终结
- **没有自动分号插入。** 语句必须以 `;` 结尾，除非它是：块表达式（`{...}`）、`if`/`match`/`loop` 形式的表达式语句、或函数体的尾表达式（可省略 `;` 作为返回值，见 §4.10 第 6 条）。任何"看起来像表达式却没有 `;` 又不在上述三种位置"→ `E-0053`。
- 行连接：`expr` 可在二元运算符处折行，**运算符必须在行首**（`+ foo`）或行尾（`foo +`）之一，两者都合法但必须一致——同一表达式内混用风格 → `infy fmt` 统一为行尾形式（§9.5）。
- 注释可出现在任何 token 之间，`infy fmt` 不得删除注释（§9.5 的硬约束）。
- `@embed` 块的内容是**词法不透明区**：除配对的 `{}` 定界扫描外不解释任何内容（转义、注释、换行均属外部语言），因此 `.infy` 的语法错误不可能由嵌入内容触发。

### 2.12 编译期常量求值（const-eval）
允许在编译期求值的构造（封闭集合，用于数组长度、`const` 初值、`#[align(N)]`、`timeout_ms` 等嵌入参数）：
- 字面量、`const`/`static` 读取、算术与比较、`as`、`&&`/`||`、`!`、元组/数组字面量、`struct` 字面量、枚举变体构造、`std.math`/`std.num` 中标注 `const fn` 的函数、`std.mem.size_of`/`align_of`。
- **禁止**：函数调用非 `const fn`、循环（只允许 `const` 数组推导 `[x; N]` 与编译期可展开的宏）、任何 IO、任何 `env.*`、任何跨 A 轨调用、堆分配、`unsafe`。违反 → `E-0060`。
- 求值深度上限 512 层嵌套调用；超出 → `E-0061`。整数溢出按该 module 的 `overflow` 策略在编译期即报错（`E-0062`），**不**推迟到运行时。
- `#[effectful]` 标注的函数永不可 `const`，即使体内无副作用（保守规则，避免实现做副作用分析）。

---

## 3. EBNF 形式文法

> 本规范采用 EBNF，每条规则有唯一编号。解析歧义若不能由 LL(2) / LR(1) 判定，必须在 §3.10 中指定唯一消歧策略。
> **本语法描述的是单个源码文件的结构**；文件→包→模块的聚合规则见 §5（一个包 = 一个目录内的全部 `.infy`，共享同一命名空间）。

### 3.1 文件结构与导入
```ebnf
(* 3.1 顶层：一个 .infy = 一个 package 内的一个文件 *)
module_file = file_attr* package_clause import_item* top_item* ;

(* 文件级属性：#![..]，必须位于 package 之前（shebang 与 //infy:build 注释更早，见 §2.1/§2.2）*)
file_attr = "#!" attr ;

(* 包声明：必选（除 cmd/* 入口文件可省略而默认取目录名）；包名必须等于所在目录名（§5.1 R1），否则 E-0501 *)
package_clause = "package" ident ;

attr = "[" attr_path ( "(" attr_args ")" )? "]" ;
attr_path = ident ( "::" ident | "." ident )* ;
attr_args = ( ident ("=" (expr | str_lit))? ("," ident ("=" (expr | str_lit))?)* ","? ) ;

(* 导入：路径为 module path + 子目录（§5.3）。无 "as" 时本地名 = 路径末段目录名 *)
import_item = "import" ( "pub" )? import_path ( "as" import_alias )? ";" ;
import_alias = ident | "." | "_";        (* "." = 仅链接符号；"_" = 只执行包级初始化副作用（Go blank import）*)
import_path = str_lit ;                  (* 必为规范化的 module path，见 §5.3；其他形式一律 E-0504 *)
```

### 3.2 顶层项
```ebnf
top_item = event_decl
         | fn_decl
         | struct_decl
         | enum_decl
         | trait_decl
         | impl_block
         | type_alias
         | extern_block
         | union_decl
         | const_decl
         | static_decl
         | macro_decl
         | chain_decl ;
```

### 3.3 事件 / 函数声明
```ebnf
visibility   = "pub" ( "(" ( "crate" | path | str_lit ) ")" )? | "priv" ;   (* pub(<path>) = 定向导出，见 §5.5 *)
path = ident ( "/" ident )* | ident ( "::" ident )* ;
trait_path = ident generic_args? ( "+" ident generic_args? )* ;
bound_list = bound ( "+" bound )* ;
event_decl = visibility? "event" ident generic_param_list? "(" param_list? ")" ( "->" type_ )? where_clause? block_or_embed ;
fn_decl    = visibility? "fn"    ident generic_param_list? "(" param_list? ")" ( "->" type_ )? where_clause? block_or_embed ;
param_list = param ( "," param )* ","? ;
param      = ( "mut" )? ident ":" type_ ( "=" expr )? ;   (* = 默认值；mut 形参 = 所有权移入且可改 *)
where_clause = "where" where_pred ( "," where_pred )* ","? ;
where_pred = type_ ":" bound_list ;
generic_param_list = "<" generic_param ( "," generic_param )* ","? ">" ;
generic_param = lifetime | ident ( ":" bound_list )? ( "=" type_ )? ;
bound_list = bound ( "+" bound )* ;
bound = lifetime | trait_path ( "<" type_list? ">" )? ;

block_or_embed = block | embed_block | ";" ;   (* ; = 无本地实现：A 轨 extern 或宿主提供，见 §7.6 *)
block = "{" statement* ( expr ";"? )? "}" ;
```

### 3.4 类型
```ebnf
type_ = prim_type
      | ident generic_args?                 (* 用户 struct / enum / trait 对象 / 类型别名 *)
      | lifetime? "&" "mut"? type_          (* 引用；借用检查见 §4.7 *)
      | "*" ("mut"|"const") type_           (* 裸指针；仅 unsafe 内可解引用，见 §4.7.5 *)
      | "[" type_ ";" int_lit "]"           (* 定长数组 [u32; 16] *)
      | "[" type_ "]"                       (* 动态数组切片 [u32]，胖指针 ptr+len *)
      | "Option" "<" type_ ">"
      | "Result" "<" type_ "," type_ ">"
      | "Box" "<" type_ ">" | "Rc" "<" type_ ">" | "Arc" "<" type_ ">"
      | "dyn" trait_path                    (* B 轨动态值与 trait 对象，见 §7.7.7 *)
      | "fn" "(" type_list? ")" ( "->" type_ )?
      | "(" type_list? ")"                  (* 元组 / unit *)
      ;
prim_type = "u8" | "u16" | "u32" | "u64" | "u128"
          | "i8" | "i16" | "i32" | "i64" | "i128"
          | "f32" | "f64" | "bool" | "str" | "char" | "()" | "Self" ;
type_list = type_ ( "," type_ )* ","? ;
generic_args = "<" type_ ( "," type_ | lifetime )* ","? ">" ;
```

### 3.5 语句（分普通语句与连携语句，见 §8）
```ebnf
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
            | unsafe_block
            | break_stmt | continue_stmt | return_stmt
            | pause_stmt | stop_stmt             (* 可作普通语句 *)
            | label_decl
            | linkage_block                       (* 连携挂载块：仅允许附加在 expr_stmt 后 *)
            ;

linkage_block = "{" linkage_stmt ( ";" linkage_stmt )* ";"? "}" ;
linkage_stmt = goto_stmt | start_stmt | start_flag_stmt
             | pause_stmt | stop_stmt | resume_stmt
             | if_stmt                           (* if 可嵌套于连携内，本身仍是普通语义 *)
             | expr_stmt ;                       (* 任意普通语句均允许在连携内执行 *)

let_stmt    = "let"  ident ( ":" type_ )? "=" expr ;      (* 默认 move 语义，§4.4 *)
var_stmt    = "var"  ident ( ":" type_ )? "=" expr ;      (* 可变绑定 *)
assign_stmt = place_expr "=" expr ;                       (* 左值必须是 place expression *)

if_stmt    = "if" "(" expr ")" block ( "else" ( block | if_stmt ) )? ;
while_stmt = "while" "(" expr ")" block ;
loop_stmt  = "loop" block ;
for_stmt   = "for" ident "in" expr block ;
match_stmt = "match" "(" expr ")" "{" match_arm ( "," match_arm )* ","? "}" ;
match_arm  = pat "=>" ( block | expr ) ;
pat = "_" | literal | ident | ".." | pat "|" pat
    | ident "{" pat_field ( "," pat_field )* ","? "}"          (* 解构 struct *)
    | trait_path "(" pat ( "," pat )* ")"                      (* enum/结构体变体 *)
    | "[" pat ( "," pat )* ","? "]" | "[" pat ";" int_lit "]" ;
pat_field = ident ":" pat | ident ;                            (* ident = 绑定 *)
unsafe_block = "unsafe" block ;                            (* §4.7.5：仅 A 轨边界与内联指针运算需要 *)
break_stmt    = "break" ( expr )? ;
continue_stmt = "continue" ;
return_stmt   = "return" ( expr )? ;
pause_stmt    = "pause" ;
stop_stmt     = "stop" ;
resume_stmt   = "resume" ;
label_decl    = "label" ident ":" ;                       (* 静态锚点，编译期编号，见 §8.2 *)
goto_stmt     = "goto" "(" goto_target ")" ;
goto_target   = str_lit                                   (* 命名锚点：编译期必须可解析，否则 E-0301 *)
              | int_lit                                   (* 绝对节点索引：仅当节点数编译期已知时合法 *)
              | "+" int_lit | "-" int_lit ;               (* 相对偏移 *)
start_stmt    = "start" "(" "chain" "=" ident ( "," ("node" "=" ( int_lit | ident )) )? ")" ;
start_flag_stmt = "start_flag" ;

chain_decl = visibility? "chain" ident block ;
```

### 3.6 嵌入块（双轨，语义见 §7）
```ebnf
embed_block = "@" track_lang embed_attrs? "{" embed_source "}" ;
embed_attrs = "(" embed_arg ( "," embed_arg )* ","? ")" ;
embed_arg   = ident "=" expr ;
embed_source = (* 原文语言源码。B 轨：交给该语言自身工具链编译为原生字节码后摄取；A 轨：整段抽出交给外部工具链编译 *) ;

track_lang = lang_b | lang_p | lang_a ;
lang_b = "py" | "lua" ;                                            (* B 轨·摄取：有稳定字节码形态 → IFBC *)
lang_p = "ts" | "js" | "sh" | "ps1" | "bat" | "cmd" ;                (* B 轨·仅进程隔离，不产 IFBC，§7.7.5 R-JS *)
lang_a = "c" | "cpp" | "rust" | "zig" ;                                                 (* A 轨：自动编译 + 自动链接 *)
```
- 一个嵌入块**必须**绑定到一个 `event`/`fn` 声明体，或作为 `expr_stmt` 出现；`embed_block` 出现在 `pat` / 类型位置 → `E-0700`。
- B 轨的 `embed_source` 在编译期交给**源语言自身工具链**编译为原生字节码，再由 §7.7.2 的能力扫描判定可否 lowering 为 IFBC；不可映射的 opcode / builtin / import → `E-0712`，动态代码生成 → `E-0711`。
- A 轨的 `embed_source` 不做 INFY 侧语义解析，但其中的**导出符号必须**在配对的 `extern_block` 中逐一声明签名（§3.9）；未声明符号被引用 → `E-0720`。

### 3.7 外部接口与链接声明（A 轨）
```ebnf
trait_decl = visibility? "trait" ident generic_param_list? "{" trait_item* "}" ;
trait_item = "fn" ident generic_param_list? "(" param_list? ")" ( "->" type_ )? ( block | ";" ) ;
impl_block = "impl" generic_param_list? type_ "for" type_ "{" impl_item* "}"
           | "impl" generic_param_list? type_ "{" impl_item* "}" ;   (* inherent impl *)
impl_item  = "fn" ident generic_param_list? "(" param_list? ")" ( "->" type_ )? block | "type" ident "=" type_ ;

type_alias = visibility? "type" ident generic_param_list? "=" type_ ;

(* 外部实现声明：无 block，仅签名 + 链接属性 *)
extern_block = "extern" abi? "{" extern_decl* "}" ;
extern_decl  = visibility? "fn" ident "(" extern_param_list ")" ( "->" type_ )? ";" ;
abi = str_lit ;   (* "\"C\""（唯一稳定 ABI）| "\"system\"" | "\"infy-unwind\"" *)

(* 链接属性：#[link] 描述一个 A 轨嵌入单元的编译与链接方式 *)
(* #[link(name="decoder", static=true, flags=["-O2"], toolchain="cc")] *)
(* #[owns] / #[borrows(<lifetime>)] 描述跨边界指针移交，见 §7.6.4 *)
```

### 3.8 表达式（优先级见 §2.7）
```ebnf
expr = literal
     | ident
     | expr "." ident                     (* 字段/路径：env.iter、cfg.max_retry *)
     | expr "::" ident                    (* 关联项 / trait 项：Option::Some *)
     | expr "(" expr_list? ")"            (* 调用 *)
     | expr "[" expr "]"                  (* 索引 *)
     | unary_op expr
     | expr binary_op expr
     | expr "as" type_
     | "move" expr                        (* 显式移动；对 Copy 类型为无操作，见 §4.4 *)
     | "&" lifetime? expr
     | "&" lifetime? "mut" expr
     | "(" expr_list? ")"                 (* 分组 / 元组 *)
     | "[" expr_list? "]"                 (* 数组字面量 *)
     | struct_literal
     | if_expr
     | unsafe_block
     | block ;
place_expr = ident | expr "." ident | expr "[" expr "]" ;
expr_list = expr ( "," expr )* ","? ;

literal = int_lit | float_lit | str_lit | char_lit | bool_lit | "auto" ;
struct_literal = ident "{" struct_field ( "," struct_field )* ","? "}" ;
struct_field = ident ":" expr ;
```

### 3.9 宏声明文法（语义与卫生规则见 §6）
```ebnf
macro_decl = "macro_rules!" ident "{" macro_arm ( ";" macro_arm )* ";"? "}" ;
macro_arm  = macro_matcher "=>" macro_expander macro_separator? ;
macro_matcher = "$(" macro_pat ")" ;
macro_expander = ( "=" | block | expr ) ;
macro_separator = ";" | "," ;
macro_pat = (* token tree 序列：$name:classifier、$(pat)sep repetition、字面 tt；完整定义见 §6.2 *) ;
macro_call = ident "!" ( "(" token_tree* ")" | "[" token_tree* "]" | "{" token_tree* "}" ) ;
```

### 3.10 消歧策略（唯一）
- `goto(+1)` 的 `+1` / `-1` 必须**优先识别为 goto 相对偏移**，而不是一元 + int_lit。
- `if (a) { b } else { c }` 中的 `else` 始终**绑定到最近的未匹配 `if`**（C 经典消歧）。
- `foo.bar = baz` 中 `.` 优先级高于 `=`，等价于 `(foo.bar) = baz`。
- `&'a 'b` 之类连续两个生命周期记法不合法：`&'a mut T` 中 `'a` 必须紧邻 `&`，`mut` 必须紧邻被引用类型；顺序错乱 → `E-1002`。
- 表达式起始位置的 `<` 一律按**小于**解析（INFY 无泛型调用 turbofish 歧义源：泛型实参必须显式写 `foo::<T>(..)`）。
- `impl<T> Foo for Bar {}` 中 `<T>` 与 `for` 之间的边界由 `>` 回溯配对确定，实现不得采用"贪婪到 `for`"的启发式。

### 3.11 表达式补全
```ebnf
expr = ...                                     (* §3.8 的全部产生式，外加： *)
     | expr "?"                                (* 错误传播，语义见 §4.11 *)
     | "!" expr                                (* 逻辑非；位非也是 "!"，见 §4.4 的类型驱动 *)
     | expr "::" "<" type_list ">"             (* turbofish：显式泛型实参，必须用 "::" *)
     | expr "." "await"? ident call_args       (* 方法调用；无 await（语言层无 async）*)
     | closure                                 (* 闭包，见下 *)
     | struct_update
     | "[" expr ";" expr "]"                   (* 重复数组字面量 [0u8; n]，n 必须 const-eval *)
     | "(" expr ")" "." ident                  (* 括号后接字段访问需再包一层，避免歧义 *)
     ;
call_args = "(" expr_list? ")" ;
closure = "move"? "|" closure_param ( "," closure_param )* ","? "|" ( "->" type_ )? block ;
closure_param = ( "mut" )? ident ( ":" type_ )? ;     (* 无标注时按使用点推断，见 §4.13 *)
struct_update = ident "{" ( struct_field ( "," struct_field )* )? ( "," ".." expr )? "}" ;
struct_field_shorthand = ident ;                       (* `x` 等价 `x: x`，仅此一种隐式 *)
field_access = expr "." ( ident | int_lit ) ;          (* 元组字段用序号：t.0 —— 注意 ".0" 不是浮点 *)
```
- **方法调用的自动引用/解引用**（§4.14 算法）：`a.f()` 中 `f` 的接收者若是 `&self` 而 `a: T` 不可移动，编译器按 `a`、`&a`、`&mut a`、以及 `*a` 的传递闭包依次尝试；**尝试顺序封闭固定**，实现不得改变，否则同一份源码在不同实现上会解析到不同方法（不合规）。
- `?` 是后缀运算符，优先级与一元 `!` 同级（14），结合性右。`a? + b?` 解析为 `(a?) + (b?)`。
- `t.0` 与 `1.0` 的区分：`.` 后紧跟数字且 `.` 前是**表达式而非数字字面量**时按元组字段解析；`x.0.abs()` 合法，`1.0` 永远是浮点。歧义现场（如 `x.0.y`）一律按字段链左结合。
- 元组字段访问**不能**跨 `&`：`(&t).0` 必须写成 `t.0`（自动解引用）或 `(*r).0`（`unsafe` 外不允许裸指针）。
- `struct_update` 的 `..base` 要求 `base` 与目标同类型且**非 `Copy`** 时其未列出的字段被移动；被移动的字段在 `base` 中失效（§4.7.1 的 use-after-move 检查覆盖此情形，`E-1001`）。`base` 为引用时 → `E-1006`（不允许从引用"借出全部字段"）。
- 相邻字符串字面量连接（§2.9）是**唯一**允许的隐式运算；`"a" "b"` → `"ab"`，在词法后立即折叠。

### 3.12 模式（完整文法与穷尽性）
```ebnf
pat = "_"
    | literal                                   (* 整数/字符/字符串/布尔字面量按 §4.4 比较 *)
    | ident                                     (* 绑定；若与已知单元变体同名，变体优先（见下消歧）*)
    | "ref"? "mut"? ident ( "@" pat )?          (* @ = 子模式绑定，Rust 语义 *)
    | "(" pat_list ")"                          (* 元组模式 *)
    | "[" pat_list "]" | "[" pat ( "," pat )* "," ".." ( pat )? "]"  (* 切片，.. 可命名 *)
    | ident "{" pat_field ( "," pat_field )* ","? "}"      (* 结构体，字段可乱序 *)
    | trait_path "::" ident ( "(" pat_list? ")" | "{" pat_field... "}" )?   (* 枚举变体 *)
    | pat "|" pat                               (* 或模式：两侧绑定集必须完全相同 *)
    | ".." | "..=" pat | range                  (* range = pat ".." pat | pat "..=" pat *)
    | "!(" pat ( "|" pat )* ")"                 (* 排除模式，仅对有限整数/枚举允许 *)
    ;
match_arm = pat ( "if" expr )? "=>" ( block | expr ) ;
```
穷尽性与可达性（全部编译期判定）：
- **穷尽检查**：`match` 必须覆盖所有可能值。`bool`/小整数/枚举按变体与 range 的并集做覆盖判定；整数 range 覆盖判定必须考虑已声明的类型宽度（`match x: u8 { 0..=251 => .., 252..=255 => .. }` 穷尽 ✓；漏掉 `255` → `E-0070`）。`_` 使任何 `match` 穷尽，但对枚举使用 `_` 而**遗漏新增变体**会被 `W-0070` 提示（strict 阻断）——这是领域无关的健壮性要求。
- **可达性**：`pat1 | pat2` 之后同一 `match` 内与 `pat1` 重叠的臂 → `W-0071`（永不匹配）；`E-0071` 留给"完全不可达的臂"（strict 视为错误）。判定算法：按臂顺序构造互斥子空间，任何臂的差集为空即判不可达。
- **`pat` 与绑定的命名冲突**：`ident` 出现在模式位置时，若该名字在当前作用域已绑定且**不是单元变体**，按新绑定处理并遮蔽 → `W-0072`（提示"你想要 `x @ _` 还是常量比较？"）；若要与常量比较必须写 `Const::NAME` 全路径（P5 零隐式）。
- **或模式绑定一致性**：`A(x) | B(x)` 中两侧必须绑定**同名同类型**集合，否则 `E-0072`。`A(x) | B(_)` → `E-0072`。
- 嵌套深度上限 32；超出 → `E-0073`。
- 守卫表达式（`if`）**不参与**穷尽性判定——有守卫的臂被视为"可能失败"，因此其后的臂仍必须覆盖同一空间；若穷尽仅靠守卫成立 → `E-0070`。

### 3.13 语句补全
```ebnf
stmt_common = ... | defer_stmt | const_assert_stmt | asm_free? ;
defer_stmt        = "defer" block ;              (* 作用域退出时执行；顺序见 §4.7.4 *)
const_assert_stmt = "const_assert" "(" expr ")" ( "," str_lit )? ;   (* 编译期断言，失败 E-0063 *)
```
- **`defer` 的执行时机**：在所在块的变量 drop **之后**、按 `defer` 声明的**逆序**执行。`defer` 内不得 `return`/`break`/`continue`/`goto`（→ `E-0074`），不得捕获 `&mut` 超出其宿主作用域的引用（借用检查按块出口时点处理）。
- 语言层**没有** `async`/`await`/`spawn`。并发一律通过链与焦点（§8）显式表达；这是 P5 与"无隐式调度"的直接后果，列入附录 F 不支持清单。



---

## 4. 类型系统、所有权与内存安全

### 4.1 类型全集（规范定义，实现不得自定义）

| 类型 | 位宽 / 表示 | 初值（未初始化即读取 = `E-0101`） |
| --- | --- | --- |
| `u8/u16/u32/u64/u128` | 8/16/32/64/128 unsigned | —（声明必须初始化，或由编译期确定的分支全覆盖） |
| `i8/i16/i32/i64/i128` | 8/16/32/64/128 signed，二进制补码 | — |
| `f32/f64` | IEEE 754-2008 binary32/binary64 | — |
| `bool` | 1 字节存储，取值仅 `0x00`/`0x01`（其他位模式为非法，越界即 `R-0006`） | — |
| `str` | 不可变 UTF-8 切片视图（ptr + len）；可变序列用 `String` | — |
| `()` | 0（unit，ZST） | `()` |
| `[T; N]` | `sizeof(T) * N`，N 必须编译期常量 | —（必须数组字面量初始化） |
| `[T]` | 动态切片（胖指针：ptr + len，16 字节 @64 位） | — |
| `struct { ... }` | 字段顺序对齐（默认 `repr(Rust)` 重排；`#[repr(C)]` 仅在 A 轨边界显式标注） | — |
| `enum` | 判别式最小容纳 + payload（niches 由编译器选取，但**不得**被源码依赖，除非显式 `#[repr(C)]`） | — |
| `Option<T>` | `Some(T) \| None`（**`null` 不存在于 INFY**；对指针类 T 走 niche 优化，尺寸 = 裸指针） | — |
| `Result<T,E>` | `Ok(T) \| Err(E)`（无 `throw`/无 panic-based 错误传递，所有可失败操作必须返回 `Result`） | — |
| `Box<T>` | 8 字节独占堆指针，析构自动 `drop` | — |
| `Rc<T>` / `Arc<T>` | 8/16 字节计数指针；`Rc` 非线程安全（链内），`Arc` 原子计数（跨链） | — |
| `&'a T` / `&'a mut T` | 8 字节窄指针（切片引用为胖指针）；生命周期由 §4.7 借用检查保证 | — |
| `dyn Trait` | 胖指针（数据 ptr + vtable ptr）；vtable 布局由 `IFABI` 固定 | — |
| `dyn` | B 轨边界专用动态装箱值（引用计数 + 类型标签），见 §7.7.7。仅允许出现在 `@embed` 的形参/返回类型上 | — |
| `fn(A,B) -> R` | 8 字节函数指针（非捕获）；捕获需闭包，闭包类型不可命名，只能靠推断或 `dyn` | — |

### 4.2 静态类型检查（`infy check` / `infy build` 阶段必须全部通过）
**所有类型检查在编译期完成**；产物运行时不做通用类型验证，仅有两处例外的边界校验：
1. `@embed` B 轨字节码的动态值边界（§7.7.7，校验失败 → `R-0710`）；
2. A 轨跨语言指针的调用期有效性（§7.6.4，校验失败 → `R-0720`）。
- 检查规则：赋值、函数参数、返回值、泛型实参 100% 类型兼容。
- **UB-001 禁令**：**禁止任何隐式数字转换**。`u32 + u8` → `E-0110`，必须显式 `x as u32`。
- **UB-002 禁令**：`f32` ↔ `u32` 隐式互转 → `E-0111`。
- 数字字面量后缀匹配（§2.6）。无后缀字面量可匹配单义上下文；多义上下文 → `E-0008`。
- 类型检查**必须早于**借用检查执行；两者共享一次 AST/MIR 遍历，诊断可同批输出，但借用检查不得在类型未定的表达式上运行（否则实现违规）。

### 4.3 可变性
- `let` = 不可变绑定（默认），重新赋值 → `E-0120`。
- `var` = 可变绑定，类型必须完整声明或从初始化完整推断。
- **`mut` 与 `var` 的分工**：`var` 只用于**语句级**绑定；`mut` 只用于**形参**、**闭包捕获**与 `impl` 的 `self`（`fn f(mut x: T)`、`move |x| {..}`）。二者不可互换：`let mut x = ..` → `E-0021`，`var x: T` 作为形参声明 → `E-0022`。理由：可变性在绑定处（语句）与在接口处（签名）是两个不同的可读性关注点。
- `&T` = 共享引用，`&mut T` = 独占引用。`&mut` 通过 `let` 绑定时，**绑定本身不可变，所指内容可变**（Rust 语义）；`var` 与 `mut` 形参的区别见 §4.7.1。
- 跨链共享可变数据：必须显式 `Arc<Mutex<T>>` 或 `std.sync.Channel<T>`；跨链裸 `&mut T` / 直接写同一 `var` → `E-0121`（数据竞争在编译期消灭，见 UB-006）。


### 4.4 运算符语义
| 操作 | 语义（规范唯一） |
| --- | --- |
| `+ - * / %`（整数） | **UB-003 禁令**：溢出/除 0 **不得 UB**。按 module 清单 `[profile] overflow = "panic"`（默认）→ 运行时 panic（`R-0001`）；`overflow = "saturate"` → 饱和；`overflow = "wrap"` **不允许**，仅可通过 `std.num.wrapping_add()` 显式表达。该策略为**编译期 codegen 选择**，写入产物元数据；链接期所有依赖的 `overflow` 策略必须一致，否则 `E-0606`（UB-021）。 |
| `+ - * / %`（浮点） | IEEE 754 定义（含 `inf`/`nan`，但 `nan == nan` → `false` 严格）。**禁止**编译器做 FMA 融合、重结合律改写等改变可观察结果的优化，除非源码显式 `#[fast_math]`；开启后必须在产物元数据中记录，且合规套件的浮点黄金用例随之切换（§12.3）。 |
| `&& / \|\|` | **短路求值**。右侧有无副作用均按短路执行（实现不得"优化执行"）。 |
| `==` 对 `str` | 按 Unicode NFC 内容比较（非指针比较）。 |
| `as` 转换 | 数字→数字：按 Rust `as` 语义（截断/位级重解释 规范固定）。不 panic（`bool as u8` 定义为 0/1，其余 `as` 定义为 bit-cast/truncate）。指针→整数**只允许**在 `unsafe` 块内，否则 `E-1030`。 |
| 赋值 `=`（含 `let`/传参/返回） | 非 Copy 类型 = **移动**（源绑定失效，§4.7.1）；`Copy`/`Clone` 类型 = 按位复制或显式 `.clone()`。禁止隐式深拷贝。 |
| `move expr` | 强制移动，即使该类型是 `Copy`（用于诊断意图明确化）。 |

### 4.5 内置函数（纯函数，可编译期求值）

```
std.math.abs(x: T) -> T              整数/浮点绝对值
std.math.roundN(x: f64, N: u32) -> u64   按 N 取整（N≥10；否则 E-0130）。也写作 round10/round50 ...
std.math.min(a: T, b: T) -> T
std.math.max(a: T, b: T) -> T
std.math.clamp(v: T, lo: T, hi: T) -> T
std.mem.size_of<T>() -> u32           编译期常量折叠（非运行时调用）
std.mem.align_of<T>() -> u32          编译期常量折叠
std.meta.type_name<T>() -> &str       产物镜像表查表，编译期解析为静态字符串
std.env.tick() -> u64                 单调时钟（毫秒，进程启动=0）
```
以上均为**纯函数**，参数为类型或常量时必须在编译期求值完成；无法常量求值 → `E-0131`。

### 4.6 `env` 全局只读宿主变量（不可修改）
v2.0 中 `env` 不再是"运行时注入的 JSON 快照"，而是**编译期绑定的只读访问器**：编译器把 `env.xxx` 降级为对运行时 `EnvSnapshot` 结构体固定偏移的读指令（§11）。宿主在每次链节点调度间隙更新该快照。规范全集：

| 变量路径 | 类型 | 语义（宿主必须实时更新） |
| --- | --- | --- |
| `env.iter` | u32 | 当前主循环迭代序号（由宿主推进；无宿主场景 = 入口链被启动的第几次） |
| `env.runtime_s` | u32 | INFY 运行时已运行秒数 |
| `env.realtime` | u64 | Unix 时间戳（秒） |
| `env.ram_mb` | u32 | 宿主进程常驻集内存（MB） |
| `env.cpu_use` | f32 | 整机 CPU 使用率 [0.0, 1.0]（宿主采样） |
| `env.gpu_use` | f32 | 整机 GPU 使用率 [0.0, 1.0]（无 GPU = 0.0） |
| `env.chain_id` | u32 | 当前执行链 ID（运行时分配，入口链 = 0） |
| `env.node_idx` | u32 | 当前执行节点在链内索引（从 0 开始，**编译期静态编号**，见 §8.2） |
| `env.focus_id` | u32 | 当前焦点 ID（同链多焦点并行时区分，§8.2） |
| `env.platform` | &str | `"win10" / "win11" / "linux" / "macos"`（由产物 target triple 决定，非查询所得） |
| `env.build_id` | &str | 本产物的可复现构建标识（§7.3 第 1 项），编译期常量字符串 |

`env` 的字段集合**封闭且领域中立**（§1.3 第 2 条）：任何指向特定应用领域的统计量（队列长度、待处理任务数、资源池余量等）必须由宿主实现 `HostApi` 并通过 `std.host.query(&str) -> Option<i64>` 读取，**不得**新增 `env` 字段。新增通用字段 = MINOR，删除/改类型 = MAJOR，实现私自扩展 = 不合规（UB-024）。

对 `env.xxx` 赋值 → `E-0140`（只读）。

### 4.7 所有权、借用与生命周期（对标 Rust）
INFY 是编译型语言，内存安全的**唯一**保证机制是编译期借用检查。不存在 GC，不存在运行时"引用有效性"兜底。

#### 4.7.1 移动语义与 `Copy`
- 每个类型要么隐式 `Copy`（`u*`/`i*`/`f*`/`bool`/`char`/`()`/`&T`/全字段 `Copy` 的 `struct`/定长数组），要么**移动**。
- 移动后源绑定进入"已移出"状态：再读取 → `E-1001`；再移动 → `E-1001`；重新赋值后恢复有效。
- 非 `Copy` 类型需要复制必须显式 `.clone()`；`#[derive(Clone)]` 不隐含 `Copy`。
- 传参、返回、`let`、结构体字面量、数组字面量、`match` 绑定，全部遵循同一套移动规则；不允许"编译器自动插一次 clone"。

#### 4.7.2 借用规则（唯一、无例外）
1. 同一时刻：任意多个 `&'a T`，**或**恰好一个 `&'a mut T`，二者不可共存。违反 → `E-1002`。
2. 引用的生命周期必须严格包含其使用区间。悬垂 → `E-1003`。
3. 返回局部变量的引用 → `E-1004`。
4. 借用一个"已移出"的值 → `E-1005`。
5. 闭包捕获遵循同一规则；按引用捕获会借用捕获处的作用域，需要移交所有权必须写 `move` 闭包（`|x| {..}` 前置 `move` 关键字）。
6. 循环/分支内对同一 `&mut` 的多次重入借用必须依次失效，不做流分析近似；分析不出来就报 `E-1002`，**不得**降级为警告放行。

#### 4.7.3 生命周期省略规则（elision，规范唯一）
函数/事件签名中省略生命周期时，按下列顺序套用，且**只有三条规则全部适用**才允许省略：
- 规则 1：每个省略的输入生命周期各自成为一个独立参数。
- 规则 2：若只有**一个**输入生命周期，它赋给所有省略的输出位置。
- 规则 3：若签名含 `self`（含 `&self`/`&mut self`/`Box<Self>`），`self` 的生命周期赋给所有省略的输出位置。
- 三条规则均不适用而仍有省略的输出生命周期 → `E-1003`，必须显式标注。`'static` 必须显式写出，永不推断。

#### 4.7.4 Drop 与 RAII
- 每个 `struct` 的字段按**声明顺序**逐个 drop；`impl Drop` 自定义逻辑在字段 drop **之前**执行。
- 局部变量按作用域内**声明逆序** drop；`let` 绑定的临时量在**所在语句结尾**drop。
- 移动到函数外部的值由其新所有者在其作用域 drop；不产生隐式"跨作用域延迟释放"。
- Drop 顺序属于可观察行为：实现不得因优化调整 drop 时机（UB-018）。释放必须由 `#[no_drop]` 显式声明才可抑制。
- 跨链持有：`Arc` 计数减到 0 的那条链执行 drop；drop 期间再被跨链访问 → `R-1010`（而非 UB）。

#### 4.7.5 `unsafe` 与信任边界
`unsafe` 从 v1.0 的保留字转为**正式启用**。允许出现在 `unsafe` 块内的操作全集（封闭列表）：
| 操作 | 未经 `unsafe` 时的诊断码 |
| --- | --- |
| 解引用裸指针 `*const T` / `*mut T` | `E-1030` |
| 调用 A 轨 `extern` 函数 | `E-0721` |
| 读写 `static mut` | `E-1031` |
| 实现 `unsafe trait` / 调用 `unsafe fn` | `E-1032` |
| 访问 `union` 的非活跃变体 | `E-1033` |
| 类型级 `transmute`（`std.mem.transmute::<A,B>`） | `E-1034` |
- 每个 `unsafe` 块必须紧随一条 `// infy-safety: <一句话前提条件>` 注释，缺失 → `W-1030`（strict 下阻断）。
- `unsafe` 不解除类型检查，只解除上述六项。边界之外的内存安全仍由 §4.7.2 负责。
- **诚实边界**：A 轨嵌入（C/C++/Zig/Rust 外部代码）内部的 UB 不在 INFY 保证范围内——INFY 只保证**边界处**的类型/长度/所有权移交正确。因此 A 轨函数一律 `unsafe` 调用，且模块清单必须声明 `[profile] a_track = "unsafe-trusted"`；未声明而使用 A 轨 → `E-0722`。该取舍的影响见附录 C 未决项 C-3。

### 4.8 内存模型与数据布局
- **无 GC**：v2.0 明确不设垃圾回收器。内存由所有权系统（栈/`Box`/`Rc`/`Arc`）与 arena 分配器管理。任何实现引入"移动/压缩式 GC" = 直接违反 UB-004。
- **禁止空悬引用**：`&T`/`&mut T` 的有效性由 §4.7.2 借用检查在编译期保证；不存在"运行时再看"的中间档。
- **地址稳定**：栈对象、`Box` 堆对象、`static`/`const` 数据、IFBC 装箱值在任何优化级别下地址均稳定；优化不得改变指针可比较性（同对象指针相等、不同对象不相等）。
- **数据布局**：默认 `repr(Rust)`（编译器可重排字段以省 padding），源码**不得**依赖默认布局；跨边界或跨 `.infib` 共享的类型必须 `#[repr(C)]`，否则链接期 `E-0605`。字段偏移查询走 `std.mem.offset_of::<T,f>()`，仅对 `#[repr(C)]` 类型可用。
- **对齐**：`#[align(N)]` 只允许增大对齐；缩小到小于自然对齐 → `E-1040`。
- **栈与递归**：每链独立栈，默认 8 MiB，可由 module 清单 `[profile] stack_bytes` 调整；溢出必须终止该链并报 `R-1040`（不得静默破坏相邻链）。

### 4.9 **UB 禁令清单**（任何触发 = 合规性失败）

| UB 编号 | 禁止行为 | 规范要求（必须的确定性结果） |
| --- | --- | --- |
| UB-001 | 隐式类型转换 | 必须 `E-0110` 编译错误 |
| UB-002 | f32↔u32 隐式互转 | 必须 `E-0111` 编译错误 |
| UB-003 | 整数溢出未定义 | 必须 panic（`R-0001`）或 saturate（module 配置），不能 wrap 除非显式 `wrapping_add()` |
| UB-004 | 除 0 静默忽略 / 移动对象的 GC | 除 0 必须 `R-0002` panic；运行时不得移动或压缩任何已分配对象 |
| UB-005 | 未初始化变量读取 | 必须 `E-0101` 编译错误 |
| UB-006 | 跨链无锁可变共享（数据竞争） | 必须 `E-0121` 编译错误（不是运行时竞争检测） |
| UB-007 | NUL 注入：`\0` 出现在 `str` 中间 | 构造时必须 `E-0150`；跨 A 轨边界运行时检查须 `R-0003` |
| UB-008 | 数组越界访问静默 | 必须运行时 `R-0004` panic |
| UB-009 | 空指针解引用 | INFY 无 `null`；`Option::unwrap()` 在 None 时必须 `R-0005`；A 轨返回裸指针为 0 且未声明 `Option` → `R-0720` |
| UB-010 | A 轨跨边界指针的移交方式未显式声明 | 必须 `E-0723` 编译错误（`#[owns]` / `#[borrows('a)]` 二选一，无默认值，体现 P5 零隐式） |
| UB-011 | 连携语句独立出现于 chain body（非挂载于宿主） | 必须 `E-0300` 见 §8 |
| UB-012 | 诊断输出实现不一致 | 必须严格符合 §9（错误码、模板、修复建议、高亮位置） |
| UB-013 | `goto` 跳到不存在的节点 | **优先编译期**：命名锚点/常量偏移不可解析 → `E-0301`；仅当节点集依赖构建配置而无法静态确定时才允许运行时 `R-0010` |
| UB-014 | `infy fmt` 官方风格不唯一 | 对同一 AST 必须输出字节级唯一格式化结果 |
| UB-015 | 宏展开泄漏作用域（非卫生） | 必须在展开后类型检查阶段失败或重新命名（`E-0400`） |
| UB-016 | use-after-move 放行 | 必须 `E-1001` 编译错误，不得以"运行时置空"代替 |
| UB-017 | 双重 drop / 泄漏由借用检查漏判 | 借用检查必须覆盖全部控制流；分析不出来 → 报错，不得默认放行 |
| UB-018 | drop 时机随优化级别变化 | drop 顺序必须是 §4.7.4 定义的确定性顺序，与 `-O` 级别无关 |
| UB-019 | IFBC 字节码未校验即执行 | 装载期必须通过 §7.8.3 校验器；校验失败 → `R-0703`，禁止"部分执行" |
| UB-020 | B 轨动态值越界后按静态类型解释 | 必须 `R-0710`；动态标签不得被优化删除（除 `#[assume_typed]` 显式声明且 `-O3` 下仍保留检查） |
| UB-021 | 依赖间编译策略不一致静默混用 | `overflow` / `repr` / `IFABI` / `fast_math` 策略冲突必须 `E-0606`，不得各自生效 |
| UB-022 | 链接顺序影响可观察结果 | 同一 module 清单 + 同一依赖闭包 + 同一 target，链接进产物的段顺序必须确定（`infy build --reproducible`，§7.3） |
| UB-023 | 工具链版本漂移未被发现 | A 轨预编译码的实际生成编译器版本与 wheel 清单记录不一致 → `E-0604`，不得"大概能用" |
| UB-024 | 编译器私自扩展 `env` / 内置类型 / 诊断码 | 封闭集合；越界即不合规。自定义码必须走 `X-` 前缀命名空间（§9.1） |

### 4.10 求值顺序（全序，规范封闭）
编译型语言最容易被实现随意化的就是求值顺序。INFY 全部定死，任何偏离 = 不合规（UB-012 的组成部分）。

1. **二元运算**：先左操作数，后右操作数，再运算。`f() + g()` 中 `f` 必定先于 `g`。
2. **赋值**：先求 **右值**，再求 **左值的 place**（地址），最后写入。`a[i] = i` 中右侧 `i` 与左侧下标 `i` 使用**同一个**值——顺序颠倒会给出不同结果的实现被禁止。复合赋值 `a[i] += f()` 等价于 `let t = a[i]; let s = f(); a[i] = t + s`，其中 `t` 在读后立即被借用检查约束。
3. **函数调用**：先求被调用表达式（callee），再按**从左到右**求每个实参，然后进入。方法调用的接收者算 callee 的一部分，因此**先于**实参。
4. **字段/索引链**：从左到右逐段求值；`a.f()[i]` = 先 `a`、再 `.f`、再 `()`、再 `[i]`。
5. **`&&` / `||`**：左操作数求值后按结果决定是否求右操作数（§4.4）。`?` 求值后立即判定。
6. **块表达式**：语句按书写顺序；尾表达式最后。块的值在尾表达式求值后才可用。
7. **`match`**：先求 scrutinee 一次（绑定到临时量），再按**臂的书写顺序**逐个尝试模式；同一臂内守卫在模式匹配成功后才求值。
8. **数组与元组字面量**按书写顺序（元素序）求值；**结构体字面量按字段声明序**求值，而不是花括号内的书写序。这样求值顺序与 drop 顺序（§4.7.4 按声明序）方向一致：`S { b: g(), a: f() }` 即使 `b` 写在前，`a` 的初值表达式 `f()` 也先执行。
9. **`defer` 与 drop**：见 §4.7.4 / §3.13。
10. **序列点**：每个语句结束、每个函数调用返回、每个 `?` 判定后都是序列点；两个副作用之间若无序列点且有重叠写入 → `E-0080`（顺序未定义被禁止表达，而不是"实现自定"）。
11. **A 轨/B 轨边界**：跨越边界前，所有 INFY 侧实参求值必须已完成（顺序按第 3 条）；边界内外部语言的求值顺序由该语言自身规范决定，INFY 不作承诺（§7.6.6）。

### 4.11 错误传播 `?`
`expr?` 是**语法糖**，其展开由函数返回类型唯一决定，不留实现自由度：
```infy
// 情形一：当前函数返回 Result<T2, E2>
let x: T = f()?;        ⇒   let x: T = match f() { Ok(v) => v, Err(e) => return Err(e.into()) };
// 情形二：当前函数返回 Option<T2>，且 expr: Option<T>
let x: T = f()?;        ⇒   let x: T = match f() { Some(v) => v, None => return None };
```
- 转换必须经 `From<E, E2>`（写成 `.into()`）；**没有** `From` 实现 → `E-0170`，诊断必须打印缺失的 `From<A, B>` 与已存在的候选 impl 列表。不做任何隐式字符串化或"包一层 `Box<dyn>`"。
- `Option` 值在返回 `Result` 的函数中使用 `?` → `E-0171`（必须显式 `.ok_or(...)`）。反向同理。
- 所在函数返回类型既非 `Result` 也非 `Option` → `E-0172`；出现在 `chain` 顶层语句（无函数返回边界）→ `E-0173`（链层的错误处理属于连携语义，不允许 `?` 逃逸到调度器）。
- `?` 在 `defer` 块内 → `E-0074`（退出时点已定，无法再改变返回值）。
- **`?` 不做控制流跨链传播**：它只作用于当前焦点的当前函数帧。
- `#[must_use]`：所有返回 `Result`/`Option` 的函数默认带此属性，丢弃其值作为语句 → `W-0170`（strict 阻断）。

### 4.12 数字语义细节
| 事项 | 规范定义 |
| --- | --- |
| 整数除法 `/` | 向**零**截断（C/Rust 语义），`(-7)/2 == -3` |
| 取模 `%` | 符号跟随**被除数**，`(-7) % 2 == -1`；恒等式 `(a/b)*b + a%b == a` 在无溢出前提下成立 |
| 移位 `<<` `>>` | 移位量 ≥ 类型宽度 → 运行时 `R-0007`（**不是** UB，不是 no-op）。`>>` 对有符号数为**算术右移**（符号扩展），对无符号数为逻辑右移 |
| `isize`/`usize` | 宽度取自产物 target，写入 `.inf meta`；`usize` 参与跨 target `.infib` 的 `pub` API → `E-0605`（§2.8） |
| 浮点比较 | `nan` 与任何值比较：`<` `<=` `>` `>=` 全为 `false`，`!=` 为 `true`，`==` 为 `false` |
| 浮点取模 | `f % 0.0` → `R-0002`；`inf % x` → `R-0008`（非 IEEE 默认值，规范选择显式失败而非静默 `nan`） |
| `roundN` | `round10(x)` = 四舍五入到最近的 10 的倍数（`.5` 远离零），返回同类型；溢出按 §4.4 策略。`N < 10` → `E-0130` |
| 字面量折叠 | 编译期常量求值遵循同一套溢出规则（§2.12 第 3 条），不做"编译器宽容、运行时严格"的双标 |

### 4.13 类型推断算法（唯一）
自底向上、单向、至多两遍；**不引入 SMT 式的完整约束求解**，以保证诊断可解释、错误定位可读：
1. 每个字面量/表达式先取"类型变量"，收集约束：`=T`、`>:T`（trait bound）、`同 arg 位`。
2. **优先级消解顺序**（封闭）：显式标注 > 函数签名 > 结构体字段声明类型 > 后缀字面量 > 唯一候选（`Option<T>`/`Result<T,E>` 内部）> 迭代器 `Item` 关联类型 > 默认整型 `i32` / 默认浮点 `f64`。
3. 走完仍有多个候选 → `E-0008`（歧义）；无候选 → `E-0009`。诊断必须列出**每个候选及其来源行**。
4. **不做强连通推断**：`let x = a ^ b;` 若 `a`/`b` 类型互推依赖循环 → `E-0010`（要求至少一侧显式）。循环推断永不完全禁止，但只允许在"函数体内、无跨函数依赖"的有界深度（≤ 8 层）内展开；超深 → `E-0160`。
5. 闭包形参类型从**首个调用点**推断（§3.11）；若闭包被存储后调用（跨逃逸点）→ `E-0161`（必须显式标注 `|x: u32|`）。
6. `dyn` 不参与推断，永不作为推断结果被选中；只来自显式标注或 B 轨边界。

### 4.14 方法解析与 trait 求解
- **接收者查找顺序**：`T` 的 inherent `impl` → `T` 实现的 trait（按 `impl` 声明顺序）→ `*T`、`&T`、`&mut T` 各自重复上述三步（配合 §3.11 的自动引用阶梯）。找到第一个即停，**不做**多候选权重排序；出现同层同名歧义（如两个 trait 都提供 `f` 且接收者相同）→ `E-0162`，必须用 `Trait::f(x)` 或 `x.trait_method::<Disambig>()` 显式消歧。
- **关联类型**：`impl` 必须提供全部非 `default` 的关联类型；缺失 → `E-0163`。同一 `impl` 内重复提供 → `E-0164`。
- **trait bound 求解**：编译期按 `impl` 表做有限搜索，深度上限 64；超出 → `E-0165`。禁止循环求解（`impl<T: Foo<T>> Bar for T` 这类自我依赖）→ `E-0166`，诊断打印推导环。
- **对象安全（object safety）**：`dyn Trait` 要求 Trait 无泛型方法、无 `Self` 返回类型、无 `where Self: Sized` 之外的非分发方法；违反 → `E-0167` 并指出具体方法。
- **孤儿规则**：`impl Trait for Type` 要求 Trait 或 Type 至少一个属于本 module（Rust 式，Go 没有这一层但编译型语言必须有）；违反 → `E-0168`。此规则是 `.infib` 跨库ABI稳定性的前提：第三方库无法事后修改你的类型行为。
- **重叠检查（coherence）**：同一 crate graph 内任意两个 `impl` 的 trait+self type 必须可判定不相交；不可判定 → `E-0169`。跨 `.infib` 的重叠由链接期符号唯一性兜底（`E-0603`）。

### 4.15 单态化与代码体积控制
- 泛型默认单态化。实例化上限：同一泛型函数在本 module 内的实例化次数 > 512 → `W-0160`（提示改 `dyn`）；全局 > 65536 → `E-0174`（拒绝构建，避免静默产出巨型产物）。
- `#[inline(never)]` / `#[inline(always)]` 是**建议**，实现可拒绝，但拒绝不得改变语义；`-O0` 与 `-O3` 的实例集合差异属实现自由（不属于可观察行为）。
- 递归泛型（`struct Node<T> { next: Option<Box<Node<T>>> }` 合法；`struct Loop<T> { t: T, next: Loop<T> }` 因尺寸无限 → `E-0175`）。

### 4.16 panic 的终止半径
规范定义三种半径，由 `[profile] panic_radius` 选择，写入 `.inf meta`：
| 取值 | 行为 | 用途 |
| --- | --- | --- |
| `focus`（默认） | 当前**焦点**沿 unwind 表逐帧 drop 后终止；该链的其他焦点与进程不受影响；`R-xxxx` 诊断上报宿主 | 编排类长驻程序：一处崩不应带走整条流水线 |
| `chain` | 同 `focus`，且额外把该链所有焦点标记 `Failed`，触发链级连携 `on_fail`（若声明） | 需要链内一致回滚的任务 |
| `process` | 立即执行全部 `defer`/drop 后以退出码 `3` 结束进程 | 一次性脚本、CI step |
- `focus`/`chain` 半径依赖 unwind 表（`§7.4` 的 `.inf meta` 内含 `unwind_table`）；`-O3` 可裁剪无 drop 函数的表项，但**不得**改变半径语义。
- Drop 中 panic → 直接进入 `process` 半径（双 panic 不可表达），并报 `R-0011`。
- A 轨边界处外部语言的异常/信号（SIGSEGV 除外）：`isolation="process"` 下按 `R-0721`；同进程下**规范不承诺可恢复**，这是 §7.6.6 的代价之一。


---

## 5. 模块、包与库文件（Go 式分包）

> v2.0 的分层直接借鉴 Go 的三级模型：**module（版本化分发单位）> package（目录，编译单位）> file**。
> 与 Go 的差异只有一处：导出仍用显式 `pub`，不采用"首字母大写即导出"（§2.3）。

### 5.1 三层定义
| 层 | 边界 | 唯一标识 | 版本 |
| --- | --- | --- | --- |
| **module** | 一个含 `MODULE.infy.toml` 的目录树根 | module path（如 `"github.com/user/infinity"`） | semver，随依赖解析确定 |
| **package** | **一个目录 = 一个 package**（不跨越目录边界，无"同名分包"） | module path + 目录相对路径 | 跟随所属 module |
| **file** | 目录内一个 `.infy` | package 内文件路径 | — |

目录示例：
```
infinity/                             <- module 根
  MODULE.infy.toml                    <- 必选，等价 go.mod
  MODULE.infy.lock                    <- 由工具生成，等价 go.sum + 锁定版本
  cmd/
    host/                             <- package "…/cmd/host"，包名 = 目录末段
      main.infy                       package host；声明 entry_chain / fn main
  core/
    schedule/                         <- package "…/core/schedule"
      chain.infy                      package schedule；与 dispatch.infy 同一命名空间
      dispatch.infy
    worker/                           <- package "…/core/worker"
      internal/                       <- 仅 worker 子树可见（§5.5）
        pool.infy
  tests/
    schedule_test.infy                <- 后缀 _test.infy 的特殊 package（§5.9）
```
规则（规范性，全部编译期强制）：
- R1 目录内每个 `.infy` 的 `package X` 必须等于目录末段名，且不与其他包目录同名嵌套。违反 → `E-0501`。
- R2 一个目录内**不得**同时存在两个包名。没有"同目录分包"的余地。
- R3 目录名即导入路径的一段，因此**目录名必须是小写 ASCII ident**（`[a-z0-9_]+`，首字符非数字）；需要中文标识符请放在符号名而非路径。违反 → `E-0505`。
- R4 以 `.` 或 `_` 开头的目录、名为 `testdata` 的目录，被构建系统忽略（Go 同规则）。
- R5 module 根之上的目录不可导入；`import` 必须从 module path 起算，**没有相对导入**（区别于 v1.0 的 `"./xxx"`）。相对路径导入 → `E-0504`。

### 5.2 `MODULE.infy.toml`（唯一清单，取代 v1.0 的 `PACKAGE.infy.toml`）
```toml
[module]
path    = "github.com/user/infinity"   (* 必填。必须可解析：https 直取 / 重定向元标签 / git *)
version = "1.2.3"                      (* 发布时必填；开发期可为 "0.0.0-dev" *)
infy    = "2.0"                        (* 语言版本，决定 §1.2 兼容判定 *)
authors = ["Alice <a@b.com>"]

[dependencies]
"std"                        = "1.0"   (* 内置标准库 = 强制依赖，不可撤销 *)
"github.com/user/mathx"      = "1.5.0" (* 精确版本或 ^1.5 语义区间 *)
"github.com/user/decoder"    = "2.0.0"

[features]
gpu = []                               (* 可选特性，参与 §2.2 build 约束 *)
a_track_c = ["gpu"]                    (* 特性可传递依赖另一特性 *)

[profile]
overflow         = "panic"             (* panic | saturate（无 wrap，§4.4）*)
diagnostic_level = "strict"            (* strict | lenient *)
opt_level        = 2                   (* 0..3 | "z"，§7.2 *)
stack_bytes      = 8388608             (* 每链栈大小，§4.8 *)
a_track          = "unsafe-trusted"    (* 声明后才允许 @c/@cpp/@rust/@zig，§4.7.5 *)
reproducible     = true                (* 强制可复现构建，§7.3 *)

[b_track]                              (* B 轨字节码摄取所需的源语言工具链版本（§7.7.3）*)
py  = "3.13"                           (* 精确到 minor；使用 @py 而未声明 → E-0715 *)
lua = "5.4"

[[bin]]                                (* 可执行目标；缺省则本 module 为纯库 *)
name  = "zhukong"                      (* 产物文件名；可含非 ASCII，不受 R3 约束 *)
path  = "cmd/host"                     (* 必须是小写 ASCII 目录（R3）*)

[[lib]]                                (* 库目标：产出的 .infib 的 edition 在此声明 *)
path     = "core/schedule"
edition  = ["source", "wheel"]         (* 同时提供源码版与 wheel 版 *)
targets  = ["x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"]
```
- `overflow` / `a_track` / `opt_level` 是 **module 级**设置，被依赖方不受上游覆盖（各自生效）；但链接进同一产物时必须一致，否则 `E-0606`（UB-021）。
- 清单语法错误 → `E-0506`；出现未知键 → `W-0500`（防拼写失效的静默忽略）。

### 5.3 导入路径解析顺序（规范唯一，不可自定义）
`import "github.com/user/infinity/core/schedule/dispatch" as dispatch;`
1. 路径必须是**字符串字面量**，以某 module path 为前缀；剩余部分为 package 目录序列。任何 `.`/`..`/通配/大小写模糊 → `E-0504`。
2. `std*` 前缀 → 编译器内置标准库虚拟 module（不得被本地 module 同名覆盖；本地 module path 以 `std` 开头 → `E-0507`）。
3. 命中当前构建的 module 根 → 本地 package，直接进入编译队列。
4. 命中 `require`/`[dependencies]` 已声明的 module → 按 §5.7 的解析顺序（vendor → 本地缓存 → 校验数据库 → 直连下载）取物。
5. 未声明的 module 前缀 → `E-0201`（提示：`infy mod tidy` 自动补全）。
6. 声明了但取不到物 → `E-0202`。
7. 取到但校验和不匹配 `MODULE.infy.lock` / 校验数据库 → `E-0560`（§5.8）。
- 导入名默认 = 路径末段目录名（`dispatch`）；`as` 可改名；`as .` 表示只链接符号不启用名字；`as _` 表示只执行其包级初始化副作用（Go 的 blank import 语义）。

### 5.4 包初始化次序（编译期推导，运行期执行）
- 每个 package 有一个隐式初始化函数，按**文件名的字典序**串联其 `package` 级 `const`/`static` 初始表达式。
- package 之间的初始化顺序 = 依赖图的**逆拓扑序**，同层按导入路径字典序。该顺序可观察，实现不得改变（UB-022 的组成部分）。
- **循环导入禁止**：包级环 → `E-0502`（诊断必须打印完整环路径）；module 级环同样禁止 → `E-0503`。没有"惰性初始化绕过循环"的降级。

### 5.5 可见性：`pub` + Go 式 `internal`
| 标记 | 含义 |
| --- | --- |
| （无） | **package 私有**：同目录内所有文件可见，跨目录不可见 |
| `pub` | 对任意导入者可见 |
| `pub(<path>)` | 定向导出：仅 `<path>` 及其子树可见（用于"实现细节只对 parent 开放"） |
| `pub(crate)` | 仅本 module 可见 |
| 目录名 `internal` | **Go 式内部包**：`a/b/internal/c` 只能被 `a/b/...` 子树导入 → 其他方 `E-0508` |
- 未标注 `pub` 的 `event / fn / struct / enum / trait / const / static / chain` = package 私有。跨 package 引用私有项 → `E-0210`。
- 字段可见性独立于类型：`pub struct S { a: u32, b: &str }` 中 `b` 私有。
- `internal` 判定只看**路径上紧邻 internal 目录的父目录**，不看 module 边界（与 Go 一致）；`internal` 目录内不能再嵌 `internal`（`E-0509`）。

### 5.6 `.infib` 库文件（**两种 edition**）
`.infib` = INFY + lib。库文件的共同特征：**不可直接运行**，只能被导入/链接。
```
infy run foo.infib        ->  E-0551「库文件不可作为入口执行」
infy build foo.infib      ->  E-0551（库文件没有独立入口，必须由调用方 module 触发）
```
v2.0 定两种 edition：源码版保住"库可读、可改、可断点"，wheel 版免编译直接链接：

| | **源码版 `edition="source"`** | **wheel 版 `edition="wheel"`** |
| --- | --- | --- |
| 内容 | `.infy` 源码 + `INFIB-META/manifest.toml` + 文档注释 + 校验段 | 可移植 IFIR + 各 target 的已编译目标码 + ABI 签名表 + `manifest.toml` + 校验段 |
| 体积 | 小（源码压缩率高） | 大（每个 target 一份码） |
| 首次使用 | 需本地编译（进 §5.10 构建缓存，之后命中即免编） | **直接链接**，不编译 |
| 可编辑/可审计 | 是（可直接读、`infy doc` 完整、断点映射到源码行） | 否（只有调试用的符号表与反编译提示） |
| 平台耦合 | 无 | 有（`targets` 列表未覆盖本机 target → 回落源码版，取不到则 `E-0607`） |
| 生成方式 | `infy lib pack --edition source` | `infy lib pack --edition wheel --target <triple>`（由 Infinity CLI 工具链在同一编译器版本下产出） |

- **同版本同 module path 的两种 edition 必须给出字节级一致的可观察行为**，否则不合规（§12.4）。工具链在解析依赖时优先 wheel（快），源码版作为回落与审计通道；`--no-wheel` 可强制只用源码版复现问题。
- ABI 签名表由 `IFABI-<n>` + 全部 `pub` 项的规范化类型字符串 + `#[repr]` 决策 + `overflow` 策略哈希构成。链接期任何一项与调用方推断不一致 → `E-0605`（不是"运行时崩了再说"）。
- 库文件**不含** `entry_chain`、`[[bin]]`、`fn main`；出现 → `E-0552`。
- 文件名规范（仅为约定，真值在 `manifest.toml`）：`<pkg>-v<semver>-src.infib` / `<pkg>-v<semver>-whl-<target>.infib`。
- wheel 版**禁止**包含未声明的 `[[link]]` 动态库路径；出现 → `E-0553`（防库文件夹带可加载二进制）。

### 5.7 依赖解析：最小版本选择（MVS）
- 采用 Go 的 **最小版本选择**：对每个 module 取"所有需求中最大的那个下界"，不做回溯搜索、不做 SAT 求解。规则封闭：
  1. 收集依赖闭包中每个 module path 的全部版本需求；
  2. 取满足全部需求的最小版本（`^` 区间按 semver 解释，`>=x <y`）；
  3. 无法同时满足 → `E-0510`，诊断必须给出冲突路径链；
  4. 结果写入 `MODULE.infy.lock`，锁定后构建只读 lock，不再查询上游。
- 只有主 module 的清单可写 `[dependencies]`；依赖的清单只读（Go 的"主模块特权"）。
- 升级必须显式：`infy mod upgrade [--to <path>@<version>]`；未在清单声明的版本即使已缓存也不可用（`E-0201`）。
- `infy mod verify` 重新计算全部校验和；`infy mod why <path>` 打印引入链。

### 5.8 完整性校验与签名：**照搬 Go 的模型**
按选定方案，INFY 采用 Go 的 `go.sum` + `sumdb` 双层机制，**不做** PKI 证书链、**不做**作者私钥签名。

1. **目录哈希（dirhash）**：对 module@version 的全部文件按"路径字节序排序 → 逐文件 `SHA-256(path + "\0" + content + "\0")` → 归约为 Merkle 树"，得到 `h1:<base64>` 形式的一个哈希。`.infib` 容器按其**解包后的逻辑文件树**计算，因此容器压缩方式不影响哈希值（对标 `go mod download -x` 的可比性）。
2. **`MODULE.infy.lock` 中的 `[[checksum]]` 段**（等价 `go.sum`）：记录 `module version dirhash` 与 `module version/go.mod dirhash` 两行。首次解析写入，后续每次比对。不匹配 → `E-0560`（硬阻断，无 `--force`，只有人工编辑 lock 文件这一条路，且必须 `infy mod accept-changes` 二次确认）。
3. **校验数据库（等价 `sum.golang.org`）**：`infy database` 提供 **Merkle tree + 一致性证明（STH）** 的只读服务。客户端内置公开密钥（无密钥轮换，因此不需要在线信任建立），流程：
   - 本地 lock 无该记录 → 向数据库请求该坐标的 inclusion proof → 校验通过才写入 lock 并落缓存；
   - 数据库不可达 → `E-0561`（默认阻断），可用 `INFY_NOSUMDB=<路径前缀>` 或 `[net] nosumdb` 精确豁免（离线场景）；
   - 数据库给出的哈希与 lock 不同 → `E-0562`，并附"该版本已被公开日志改写"提示。
4. **私有/内网**：`INFY_PRIVATE=<glob>` 或清单 `[net] private = ["git.internal/*"]` 同时关闭数据库查询与直连认证要求；此类 module 只走本地 lock 校验，`infy mod verify` 会显式标注 `unaudited`。
5. **明确记录的代价**（写进规范，避免误以为已经"验签身份"）：此模型保证**不可篡改**与**全网一致**，但**不证明作者身份**——任何持有合法 dirhash 的镜像都能被接受。身份认证若要引入，走 v2.1 的 Ed25519 附加签名段（附录 C 未决项 C-5，属 MINOR，不破坏现有 lock）。
6. `@embed` 的 A 轨外部源码、wheel 版的每个 target 目标码，全部作为 dirhash 的普通文件参与计算——不允许"二进制段跳过校验"。

### 5.9 测试与基准包
- `xxx_test.infy` 文件不进入普通构建（受 §2.2 build 约束 `test` 控制）。
- 目录内可存在 `package foo` 与 `package foo_test` 两种测试文件，后者只能经 `infy test` 编译（Go 同规则），用于强制"只走公开 API"。
- 测试函数标记 `#[test]` / `#[bench]`；`infy test` = build + 执行，退出码沿用 §13.4。

### 5.10 缓存与 vendor 目录布局
```
%LOCALAPPDATA%/Infinity/          (Win)   |  ${XDG_CACHE_HOME}/infinity/  (Unix)
  module/<path>@<version>/        解包后的 module 内容（只读，按 dirhash 命名硬链接）
  infib/<path>@<version>-<edition>/       .infib 解包
  build/<key>/                    编译缓存：key = dirhash + 清单策略哈希 + 编译器版本 + target
  sumdb/                          校验数据库公钥与已验证 STH 缓存
  toolchain/                      A 轨外部工具链注册表（§7.6.2）
./vendor/                         `infy mod vendor` 产出；存在则优先于一切网络解析
```
- `build/` 缓存命中要求 key 全等，**不得**用时间戳或 mtime 判定（违反 §7.3 可复现构建）。
- 缓存目录写失败 → `E-0570`（无缓存则直接编译，不阻断）。


---

## 6. 宏与属性系统（卫生 macro_rules! + `#[attr]`）

### 6.1 语法（复用 §3.9 EBNF）
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

### 6.5 展开时机与编译期求值
- 宏在 §7.1 管线的**阶段 [2]**（语法后、名称解析前）展开，展开结果必须是合法 AST 片段；展开成非法片段 → `E-0402`（诊断指向展开器而非调用点，并打印展开后的片段）。
- 宏**不得**读取类型信息、不得条件化于类型；需要按类型分派请写泛型 + trait，而不是宏（P5 零隐式的延伸）。
- 展开后的代码参与借用检查与 codegen，不存在"宏体豁免优化级别"。

### 6.6 属性系统（`#[...]` / `#![...]`）
属性不是宏，由编译器内建实现。**封闭列表**，未知属性 → `E-0410`：
| 属性 | 作用位置 | 语义 |
| --- | --- | --- |
| `#![infy_version = "2.0"]` | 文件 | §1.2 版本声明 |
| `#![entry_chain = "Foo"]` | 文件 | §8.2 指定入口链 |
| `#[test]` / `#[bench]` | `fn` | §5.9 |
| `#[repr(C)]` | `struct`/`enum`/`union` | 固定布局；跨 `.infib`/A 轨边界必需（§4.8） |
| `#[align(N)]` | `struct` | 增大对齐（§4.8） |
| `#[link(name, static, flags, toolchain)]` | `extern_block`/`event` | A 轨编译与链接指令（§7.6.3） |
| `#[owns]` / `#[borrows('a)]` | `extern_decl` 返回/参数 | 跨边界指针移交方式（UB-010，无默认值） |
| `#[export("symbol_name")]` | `fn`/`event` | 反向导出给 A 轨代码回调（§7.6.5） |
| `#[fast_math]` | `fn`/`impl` | 允许浮点重结合（§4.4） |
| `#[no_drop]` | `struct` | 抑制自动 drop（仅 `unsafe` 类型可用） |
| `#[assume_typed(T)]` | B 轨 `dyn` 位置 | 声明"编译期已知类型"，仍保留装载期校验（UB-020） |
| `#[deprecated("note")]` | 任何 `pub` 项 | 使用点报 `W-0400` |
| `#[doc(hidden)]` | 任何项 | 不进入 `infy doc` |

### 6.7 `#[derive]` 自动生成规范
derive 不是宏展开的黑盒，其产物语义**逐条规范固定**——因为 `==`、`size_of`、文档都依赖它。可用 derive 全集：`Clone`, `Copy`, `Default`, `Debug`, `PartialEq`, `Eq`, `PartialOrd`, `Ord`, `Hash`, `HasField`(反射)。

| trait | 生成规则（规范） |
| --- | --- |
| `Clone` | 逐字段 `<T as Clone>::clone`；`Copy` 类型走按位复制。数组 `[T; N]` 逐元素。跳过 `#[skip_clone]` 字段 → 该字段必须实现 `Default`，否则 `E-0420` |
| `Copy` | 仅当**全部**字段 `Copy` 且类型无 `Drop` impl、无 `dyn` 字段；违反 → `E-0421`。`Copy` 不可用户自定义 |
| `Default` | 数值→`0`、`bool`→`false`、`char`→`'\0'`、`str`→`""`、`[T; N]`→逐元素 Default、`Option<T>`→`None`、`Result`→`Err(E::default())`、`enum`→**首个变体**（须全字段 Default，否则 `E-0422`）。可 `#[default]` 显式指定 enum 变体，指定多个 → `E-0423` |
| `Debug` | 输出 `Name { f1: v1, f2: v2 }`（`#[repr(C)]` 输出 `Name(f1, f2)` 元组形式）。字段顺序 = 声明顺序，永久稳定（进 §12.3 黄金文件比对，因日志常被下游解析）。`#[redact]` 字段输出 `<redacted>` |
| `PartialEq` | 逐字段 `==`，**短路**按声明顺序；`#[compare(skip)]` 字段排除。浮点字段允许 derive 但 `Eq` 必须显式 `#[allow_nan_equality]` 否则 `E-0424` |
| `Eq` | 无新代码，仅做标记检查：所有字段 `Eq` 才可 derive，否则 `E-0425` |
| `PartialOrd`/`Ord` | **按字段字典序**比较（声明顺序即优先级），短路；`#[order(asc)]`/`#[order(desc)]` 按字段翻转。`Ord` 要求全字段 `Ord`。规范固定字典序而非"任何实现自选"——排序结果进可观察行为 |
| `Hash` | 按声明顺序喂入 `field_discriminant_i32` 再喂字段哈希；类型名不喂入（保证跨 module 的 `HashMap` key 可迁移）。哈希算法固定为 **xxHash64**（v2.0），换算法 = MAJOR |
| `HasField` | 生成编译期反射表：字段名、偏移（仅 `#[repr(C)]` 可用）、类型标签字符串。`std.meta.fields::<T>() -> &[FieldInfo]` 的底层。跨 `.infib` 的反射表版本由 `IFABI` 校验 |
- derive 顺序：同一项上多个 derive 按**书写顺序**展开生成（影响 `Debug` 的属性叠加错误定位）；重复 derive 同一 trait → `E-0426`。
- derive 生成代码**不参与**用户代码的 `unsafe` 检查放宽；若某字段类型使生成的 `Drop`/`Clone` 需要 `unsafe`，derive 直接失败并给出原因（`E-0427`），不允许"静默插入 unsafe 块"。

### 6.8 `infy expand`（宏与 derive 的可审计输出）
- `infy expand [mod] [--macro name] [--derive Type] [--stage all|macro|derive]` 打印展开后的 `.infy` 源码（带 `// expand-from: <file:line>` 溯源注释），供人工核对"生成代码是否引入隐式行为"。
- 打印格式必须**字节级稳定**（同一输入同一版本 → 同一输出），否则 §7.3 的可复现审计失去意义。
- `--stage derive` 的产物必须能被 `infy check` 接受（可编译性要求），这是"derive 不是黑盒"的机制保证。
- 任何实现不得提供"展开后语义与原码不同"的优化路径。

---

## 7. 编译管线与多语言嵌入

> 本章是 v2.0 的核心。语言从"解释执行 + 子进程喂 JSON"整体翻转为"**编译器一次性产出单文件产物**"。
> 嵌入分两条轨道：**A 轨**（编译型语言 → 编译器自动完成绑定生成与链接）与 **B 轨**（解释型语言 → 构建期取其**自身字节码**并 lowering 为 IFBC，运行时动态执行）。

### 7.1 编译管线（`infy build`，阶段编号规范固定）
```
输入：本 module 源码 + 依赖闭包（.infib / module 源） + 全部 @embed 块
 [1] 词法/语法            → AST                       （§2 §3；错误 E-00xx / E-03xx）
 [2] 宏展开               → eAST                      （§6；E-04xx）
 [3] 名称解析 + 依赖图     → 拓扑序 package 列表        （§5.1/§5.4；E-05xx，循环即终止）
 [4] 类型检查             → 定型 HIR                   （§4.1/§4.2；E-01xx）
 [5] 所有权/借用检查 borrowck → 带 region 的 HIR         （§4.7；E-10xx，与 [4] 同批输出）
 [6] 嵌入分派
     [6a] B 轨字节码摄取     调用源语言工具链 → 能力扫描 → lowering → IFBC（§7.7）（E-071x）
     [6b] A 轨工具链驱动    → 目标码对象文件（§7.6）     （E-072xx / E-06xx）
 [7] IFIR 生成 + 优化       可移植中间表示，语言内部稳定但不对第三方承诺
 [8] 后端 codegen          → 目标码（target triple）    （§7.2）
 [9] 链接 `infyl`          → 单文件产物（§7.4）         （E-06xx）
 [10] 产物签名/校验段写入   → dirhash + lock 比对（§5.8）
```
- 阶段顺序**不得**乱序或并行化到可观察差异（例如 [5] 之前必须 [4] 完成）。
- 任一阶段出现 `E-` 即中止后续阶段，但**同阶段内**必须尽量一次性报告全部诊断（错误恢复是质量要求，非可选）。
- `infy check` 只跑 [1]–[5]，不产出码；[6a] 在 `infy check` 下**需要**源语言工具链在场才能做能力扫描（缺失 → `E-0713`），[6b] 只做**签名核对**不做编译，因此无 C 编译器也能检查纯 INFY 代码。

### 7.2 目标、优化与剖面
- **target triple**：`<arch>-<vendor>-<os>-<abi>`，内建支持集：`x86_64-pc-windows-msvc`、`x86_64-pc-windows-gnu`、`x86_64-unknown-linux-gnu`、`aarch64-apple-darwin`、`aarch64-unknown-linux-gnu`。自定义 triple → 必须在 `TOOLCHAIN.infy.toml` 注册，未注册 → `E-0601`。
- **opt_level**：`0`（无优化，保留完整调试信息）、`1`、`2`、`3`、`z`（体积优先）。默认取清单值，命令行 `-O<级别>` 覆盖主 module，**不覆盖依赖**（依赖用其 wheel/清单声明的级别，避免策略漂移 → UB-021）。
- **profile**：`debug`（默认 opt 0 + debug info + `R-` 边界检查全开）、`release`（opt 2 + 边界检查仍全开）、`release-small`（opt z）。
- **边界检查永不随 profile 关闭**：数组越界、溢出、`Option::unwrap`、IFBC 校验必须保留（§4.9 是语义而非"调试辅助"）。性能优化必须通过 `unsafe` + 显式 `#[assume_...]` 来表达，不提供"整库关掉安全检查"的开关。
- **LTO**：`lto = "thin" | "fat" | false`，默认 `fat`（release）。LTO 改变可观察结果 = 不合规（黄金文件按 lto 设置分别比对）。
- **交叉编译**：`--target <triple>` + 对应 A 轨工具链；缺工具链 → `E-0602`（编译期，而非链接后期）。
- **增量编译**：以 package 为粒度，缓存 key = `dirhash + 清单策略哈希 + 编译器版本 + target`（§5.10）。增量结果是优化，不得改变诊断集合：同一输入在增量与全量下必须输出**相同**诊断序列。

### 7.3 可复现构建（Reproducible Build）
1. 产物 `build-id` = `SHA-256(规范化的全部输入)`，输入含：源码 dirhash、依赖 lock、清单、target、opt_level、编译器版本字符串。**不含**时间戳、绝对路径、用户名。
2. 因此同一输入 + 同一工具链版本 ⇒ **字节级相同**产物。任何随机种子（哈希表序、符号名后缀）必须以 `build-id` 派生而非系统熵，链接段顺序按 §5.4/§7.4 固定。
3. `infy build --verify-reproducible`：在两个独立构建目录重跑并比对；不一致 → `E-0603`（并列出首个差异段）。这是 §12 合规套件的必查项（UB-022）。
4. A 轨外部工具链的输出也必须纳入比对：`[6b]` 记录编译器版本、命令行、输入哈希，重放时逐字节校验。

### 7.4 产物容器（单文件自包含，落实 P6）
产物是一个可直接执行的宿主格式文件（PE / ELF / Mach-O），在标准段之外追加 INFY 段：
```
.inf meta   INFY-ART v2.0 清单：IFABI、target、build-id、依赖闭包(path@version+dirhash)、
            overflow/opt/stack_bytes 策略、entry 描述（entry_chain 或 fn main）
.inf text   原生码（INFY 本体 + A 轨链接进来的目标码）
.inf rodata 常量、字符串字面量、类型镜像表（供 std.meta / 诊断 / infy doc 使用）
.inf ifbc   B 轨摄取结果：按语言分节的 IFBC 模块（§7.8），装载期校验
.inf rt     内嵌精简运行时：IFBC 虚拟机 + 链调度器 + arena 分配器 + ResourceLock
.inf sum    依赖 dirhash 清单 + 校验数据库 inclusion proof（§5.8），运行时自校验用
```
- 运行时**只依赖宿主系统 API**（线程、时钟、mmap、socket、进程），不依赖任何外部语言解释器。这是"用户不需要装 Python"的技术根据。
- `.inf ifbc` 段允许为空（纯 INFY + A 轨产物）；不为空时，装载期先跑 §7.8.3 校验器再执行，失败 → `R-0703` 并在诊断中定位到**原始语言源码行**（lowering 必须从 code object 带上行映射）。
- `.infyny`（§13.2）= 多产物 + 资源的分发容器，不是产物本身。
- 入口规则：产物必须有且仅有一个入口。优先级 `#![entry_chain]` > `[[bin]]` 中的 `fn main` > module 主 package 内**第一个声明的 chain**。全无 → `E-0600`；多个显式声明冲突 → `E-0601`。


### 7.5 嵌入总则：一条 `@` 前缀，两条轨道
| | **A 轨（编译型）** | **B 轨（解释型）** |
| --- | --- | --- |
| 前缀 | `@c` `@cpp` `@rust` `@zig` | 摄取得 IFBC：`@py` `@lua`；仅进程隔离：`@ts` `@js` `@sh` `@ps1` `@bat` `@cmd`（§7.7.5）|
| 编译期做什么 | 抽源码 → 生成绑定 → 调用外部工具链编译 → 链接进 `.inf text` | 调用该语言**自身的字节码编译器** → 能力扫描 → lowering 为 IFBC → 放进 `.inf ifbc` |
| 运行时是什么 | 原生机器码，直接执行 | IFBC，由内嵌虚拟机动态执行 |
| 类型检查 | 边界签名 INFY 侧检查（`extern_decl`）+ 外部工具链自查 | 指令语义由源语言编译器定型；INFY 侧只做能力扫描与 `dyn` 边界校验（装载期 + 调用期） |
| 谁保证内存安全 | INFY 只保证**边界**；块内由外部语言负责（§4.7.5） | IFBC 校验器保证块内（§7.8.3）；指令语义由源语言编译器定型，INFY 不重新解释它 |
| 需要外部依赖 | 需要（编译器） | **不需要**（不再需要 CPython/Deno/Lua…） |
不在任一白名单的 `@xxx` → `E-0702`。A 轨前缀但工具链未注册 → `E-0602`（编译期，绝不拖到运行时）。

### 7.6 A 轨：编译型语言嵌入的自动连接
> 这是本版对 v1.0 的核心翻转：编译型语言从"绝对禁止"变成"编译器负责把活干完"。用户只写源码和签名，不写胶水、不写构建脚本、不手动 `dlopen`。

#### 7.6.1 声明形态
A 轨**必须**成对出现：一个 `#[link] extern "C" { ... }` 签名块 + 一个 `@c { ... }` 源码块。二者由同一个 `name` 绑定。
```infy
package host                                   // 本文件位于 cmd/host/，module path 在清单中声明

#[link(name = "decoder", static = true, flags = ["-O2"], toolchain = "cc")]
extern "C" {
    #[borrows('static)]                        // 返回指针的所有权归属，必须显式（UB-010）
    fn dec_open(path: *const u8, len: u64, err: *mut i32) -> *mut DecHandle;
    fn dec_read(h: &mut DecHandle, buf: &mut [u8]) -> usize;
    fn dec_close(h: *mut DecHandle);           // 接管释放：调用后 h 失效，由编译器插 move 保证
}

event open_decoder(path: &str) -> Result<Box<DecHandle>, NativeErr> {
    @c(name = "decoder") {
        /* C 原文。编译器整段抽出，写入 <build>/a_track/decoder.c */
        #include "infy_binding.h"
        DecHandle* dec_open(const uint8_t* p, uint64_t n, int32_t* err) { /* ... */ }
        /* ... */
    }
    let raw = unsafe { dec_open(path.as_ptr(), path.len() as u64, &mut e) };   // E-0721: unsafe 必需
    // infy-safety: dec_open 返回值非 null 当且仅当 err == 0，见下方封装
    if raw.is_null() { return Err(NativeErr::from(e)); }
    Ok(Box::from_raw_safe(raw))
}
```
- `extern "C"` 是 A 轨**唯一**的稳定 ABI。`"system"` 表示宿主平台默认调用约定；`"infy-unwind"` 表示允许跨边界展开（默认禁止，跨边界展开 → `R-0721`）。
- 签名块与源码块 `name` 不匹配 → `E-0720`。签名声明了但源码未导出 → **链接期** `E-0604`；源码导出了但签名未声明且无人引用 → `W-0720`。

#### 7.6.2 工具链发现与注册
- `TOOLCHAIN.infy.toml`（module 根，或全局 `%LOCALAPPDATA%/Infinity/toolchain/`）登记：`[cc] kind="msvc" | "gcc" | "clang" | "zig" | "rustc"`, `path`, `version_str`, `target`。
- `infy toolchain probe` 扫描 PATH/注册表/vcpkg/`vswhere`，`infy toolchain add <kind> --path <exe>` 手工登记。
- 构建时执行**版本核对**：实际编译器版本必须与 wheel 版 `.infib` 中记录的版本字符串一致（同 major/minor），否则 `E-0604`（UB-023）。允许 `--allow-toolchain-drift` 降级为 `W-0604`，但该产物不再满足 §7.3 可复现承诺，`.inf meta` 会打 `unreproducible` 标记。
- 编译命令行、环境变量、输入哈希全部落 `<build>/a_track/<name>/recipe.toml`，供复放与审计。

#### 7.6.3 绑定自动生成（编译器职责清单）
编译器对每个 A 轨单元自动生成，用户不得手写以下任何一项：
1. `infy_binding.h`：由 `extern_decl` 反向生成的 C 头文件（类型映射见 §7.6.4）。
2. 符号包装：`infy_e_<name>_<8hex>`，`<8hex>` = 签名块规范化哈希；避免同名不同签名冲突。
3. 边界 thunk：长度/对齐/null→`Option` 检查、调用期借用有效性检查（`R-0720`）、错误码→`NativeErr` 转换。
4. 链接指令：静态走 `libtool/link.exe`/`ar`/`lld`；产物段顺序按 §7.4 固定顺序（依赖拓扑逆序 + 名称字典序）。
5. 调试映射：外部行号 → INFY 诊断位置（使 `R-` panic 能报出 `decoder.c:41`）。

#### 7.6.4 跨边界类型与所有权映射（封闭表）
| INFY 类型 | C 侧 | 方向与约束 |
| --- | --- | --- |
| `u8/i32/f64/bool/char` | 同宽整型 / `double` / `uint8_t` / 32 位枚举 | 值拷贝 |
| `&[u8]` / `&mut [u8]` | `const uint8_t*, uint64_t` / `uint8_t*, uint64_t` | 借用**仅在调用期间有效**；外部保存该指针必须 `#[borrows]` 显式声明生命周期或改 `Box<[u8]>` 移交 |
| `&str` | `const char*, uint64_t`（**非 NUL 结尾**）；需 C 字符串必须显式 `.to_cstr()`（分配 ` CString`） | 内含 `\0` → `E-0150`/`R-0003`（UB-007） |
| `*const T` / `*mut T` | 裸指针 | 仅 `unsafe` 内可解引用 |
| `Option<&T>` | 可为 0 的指针 | null → `None`，不 panic |
| `Result<T, i32>` | `T ret, int32_t* err` 出参形式 | 自动生成 thunk；外部忘写 `err` → `R-0722` |
| `struct #[repr(C)]` | 同布局 struct | 无 `repr(C)` 而跨边界 → `E-0605`（§4.8） |
| `dyn Fn(...) -> ...`（回调） | `void (*)(void* ctx, ...)` + ctx | 闭包被装箱进 `Arc`，外部释放必须经 `#[export]` 的 `free` 入口，否则**必然泄漏**并检出为 `R-0723`（计数不为 0 时卸载） |
- **禁止**跨边界传递：`Rc<T>`、未 `repr(C)` 的 `enum` 判别式、`[T]` 的**容量**（只传 ptr+len，不传 `Vec` 内部布局）、任何 INFY 分配器的裸内存。违反 → `E-0723`。

#### 7.6.5 反向导出（外部代码回调 INFY）
`#[export("on_sample")] event on_sample(...)` 生成 `extern "C" fn` 符号并进入导出表；导出表在 `.inf meta` 中登记，`dlopen` 自身产物或宿主 C 代码可查找。回调**必须**在 INFY 侧被包成 `unsafe` thunk：跨边界传入的指针按 §7.6.4 校验后才升级为 `&T`；未声明即解引用 → `E-0724`。

#### 7.6.6 安全代价与可选进程隔离（诚实记录）
同进程链接 C 代码意味着 **C 侧的 UB 会污染整个产物进程**，INFY 的借用检查无法穿透。因此：
- 默认策略 `a_track = "unsafe-trusted"`：边界外不管，边界内全权交给外部语言。未声明该策略而使用 A 轨 → `E-0722`。
- 可选 `#[link(isolation = "process")]`：编译器把该单元降级为**独立子进程 + 长度前缀 IPC**（v1.0 的 JSON-RPC 思路在这里作为**隔离手段**保留，而非默认执行模型）。跨进程模式下 `&mut` 借用不成立，参数/返回一律按"序列化 + 拷贝"，签名必须为 `#[owned]`。模式不同 ⇒ 生成的 thunk 不同，但**可观察语义必须一致**，否则不合规（§12.4）。

### 7.7 B 轨：解释型语言 → 原生字节码摄取 → IFBC
> v1.0 的 B 轨是"把源码原样丢给子进程解释器"。v2.0 改为**在构建期调用源语言自己的工具链取得原生字节码，机械映射（lowering）为 IFBC，运行时由产物自带的精简 VM 执行**。用户机器上不再需要 Python/Node/Lua。**INFY 不实现任何外部语言的解析器。**

#### 7.7.1 摄取路径
B 轨的输入是**源语言官方编译器已经产出的字节码**，于是"转译"退化为一次指令流的机械映射：
```
@py     → CPython compileall / py_compile / compile() → .pyc 内 code object → lowering → IFBC
@lua    → luac -o                                     → Lua 字节码 proto     → lowering → IFBC
@js/@ts → tsc / esbuild 只产 JS 文本，无稳定字节码形态  → 仍需 parser（B 轨唯一的重活）
@java   → javac → .class（JVMS 规范格式，跨版本稳定）   → lowering → IFBC（v2.1 候选，§7.7.5）
```
为什么不把 `.pyc` 原样塞进产物、交给一个"Python 执行器"跑，而要坚持 lowering 到 IFBC：
1. **单一校验器**。§7.8.3 的六项装载校验是内存安全在字节码侧的落点；若产物里并存 N 种外来字节码格式，就要对 N 种格式各做一套等价审计。
2. **单一边界与调度**。链、焦点、ResourceLock、超时、取消全部建立在 IFBC 帧上；外来字节码不受这套调度。
3. **单一分配器**。§4.8 的地址稳定与 arena 规则要求所有堆对象经同一分配器出口。
4. **元数据免费**。code object 自带 `co_linetable`、`co_exceptiontable`、`co_consts`、`co_names`、`co_varnames`，lowering 时直接搬进 IFBC 的 `line_map` / `exception_table` / `string_pool` / `import_table` / 局部窗口——这正是"工程量不大"的实质来源：**前端和调试信息都是白拿的**。
```infy
event normalize(batch: &[f32], scale: f32) -> Vec<f32> {
    @py(batch=batch, scale=scale) {
        # 构建期：本块写入 <build>/b_track/normalize.py，由清单声明的 CPython 编译
        out = [ (x / scale) for x in batch ]
        return out
    }
}
```
**调用与返回的映射规则**：
1. `batch`/`scale` 的绑定在**编译期**完成：编译器读 INFY 侧声明类型，生成 IFBC 形参表与进入前的装箱转换。不再是"注入全局变量"，而是**函数调用约定**。
2. `return` 映射到 IFBC 返回指令；`env.return(x)` 作为 v1.0 兼容写法保留，等价实现期 `return`，报 `W-0700`。多返回值 → 元组。
3. 超时：`@py(timeout_ms = 60_000)` → lowering 时在**每条回边**前插 `tickloop`，超限由运行时挂起该帧并触发 `R-0701`（不杀进程；取消语义见 §7.9）。
4. 外部进程的 stdout/stderr 概念消失。B 轨内 `print()` 映射到 `std.io.log`（级别 `INFO`），脚本错误 → INFY `WARN`/`ERROR` 并附**原始语言行号**（来自 code object 的行映射）。

#### 7.7.2 能力表取代手写子集清单
既然输入是编译器产物，就不需要人为规定"哪些语法允许"。**规范改为对 code object 做静态能力扫描**，逐项与能力表比对：
| 扫描项 | 来源 | 判定 |
| --- | --- | --- |
| opcode 集合 | `co_code` | 必须全部落在 §7.8.2 的映射表内；出现未映射 opcode → `E-0712` |
| 引用的 builtin | `co_names` ∩ `LOAD_GLOBAL` | 必须在该语言运行时能力表内；否则 `E-0712`，并列出尚未实现的内置名 |
| 导入目标 | `IMPORT_NAME` 的常量名 | 必须是已声明的 INFY module / `.infib` / 能力表内的纯字节码包；否则 `E-0712` |
| 嵌套 code object | `co_consts` 递归 | 逐个递归扫描——推导式、lambda、生成器体、装饰器展开后的帧全都在内 |
| 异常表形态 | `co_exceptiontable` | 必须可映射到 IFBC `exception_table`；不支持的条目类型 → `E-0712` |
- 能力表是**机器可读、随版本发布**的规范性数据（`IFBC-CAP-<lang>-<minor>.toml`），直接进 §12.2 合规用例。它比手写语法清单可验证得多：**清单靠人想，能力表靠扫描器穷举**。
- 扩能力 = MINOR；收能力 = MAJOR。没实现的东西在构建期就红着退出，不留到运行时行为漂移（P3）。
- 动态代码生成（`eval` / `exec` / 运行时构造 code object）无法被静态扫描覆盖，因此**永久拒绝** → `E-0711`（该码的语义自 v2.0 起收窄到这一类）。
- 原 `INFY-py` 手写子集表（允许/拒绝构造清单）自本版**废止**，被上表取代。历史条目里属于"动态代码生成"的仍然拒绝，其余一律改由扫描判定。

#### 7.7.3 字节码版本锁定（规范性）
外来字节码格式**不跨版本稳定**：CPython 3.11 引入自适应特化 opcode，3.12 重写 f-string 的 code object 结构，3.13 变更异常表编码。因此：
- module 清单必须声明 `[b_track] py = "3.13"`（精确到 minor；patch 不敏感）。未声明而使用 `@py` → `E-0715`。
- 工具链与 A 轨共用同一张注册表：`infy toolchain add py --path <python.exe> --version 3.13.1`（§7.6.2）。构建期实际调用的解释器版本与声明不符 → `E-0713`。
- `.pyc` 的 `magic number` 必须等于声明版本的期望值 → 否则 `E-0713`。
- 产物 `.inf meta` 记录 `source_toolchain = "CPython 3.13.1"`，该字符串纳入 §7.3 `build-id` 的输入集合——**换 Python 版本必须换 build-id**，否则可复现性不成立（UB-022）。
- 运行时**不**检查宿主是否装有 Python：产物里没有 `.pyc`，只有 IFBC。构建期依赖 ≠ 运行期依赖，这与 A 轨需要 C 编译器完全同构。

#### 7.7.4 第三方包的摄取路径（纯字节码生态直接可达）
`pip download` 取到的**纯 Python wheel**，其 `.py` 可被 `compileall` 变成 code object，因此无需人工写绑定就能进 INFY：
```
infy ingest py-wheel <pkg-1.2.3-py3-none-any.whl> --as "thirdparty.example.com/pkg" [--py 3.13]
  → 解包 → compileall → 能力扫描（§7.7.2）→ 逐模块 lowering → 产出 .infib（源码版 + wheel 版）
```
- 只接受 `py3-none-any` 一类**纯 Python** wheel；带二进制的（`cp313-win_amd64`）→ `E-0714`，并提示改走 A 轨 `@c` 链接其 C API 或 `isolation="process"`。
- 能力扫描是整条路径的守门人：**只要那个包不碰未实现的对象语义，它就能进来**；碰了就当场红并精确指出是哪个模块的哪个 opcode / 哪个 builtin。这比原来"拒绝第三方库"的一句话强得多——拒绝的粒度从"生态"降到了"单个构造"。
- 记住这层对应关系：Python 的 wheel 与 INFY 的 wheel 版 `.infib`（§5.6）本质是同一类东西（预编译、免编译直接链接），`infy ingest` 就是两者之间的转换器。
- 经 `infy ingest` 进入的包，其 dirhash 计算与依赖声明完全遵循 §5.8，与手写 INFY 库无差别；上游内容变更同样会被 `E-0560` 拦住。

### 7.8 IFBC（INFY ByteCode）规范
#### 7.8.1 容器与编码
```
IFBC Module = header | type_table | import_table | func_table | code | line_map | exception_table | string_pool
header: magic "IFB1" | ifbc_version:u16 | ifabi:u16 | lang_tag:u16 | module_hash:[u8;32]
        | func_count:u32 | frame_kind:u8 (= 0: 单地址栈式 + 局部窗口)
指令编码: 1 字节 opcode + 0..3 个 u32 operand（固定宽度，不做变长前缀；便于校验器线性扫描）
```
- **固定宽度指令**是规范性要求：校验器可在 O(n) 单遍扫描内完成栈深度模拟，不需要回溯。
- `lang_tag` 记录源语言、其字节码格式版本（如 `CPY-313`）与所用能力表版本，用于诊断回溯、`infy doc` 标注与 §7.7.3 的版本校验。

#### 7.8.2 指令分类（主干，完整表随附录 D 发布）
| 类 | 助记符样例 | 说明 |
| --- | --- | --- |
| 局部/操作数 | `ldl`, `stl`, `ldw`, `push` | 局部窗口寻址 |
| 算术/比较 | `add.i32`, `div.f64`, `cmpl`, `neg` | **带类型后缀**；无后缀版本非法 |
| 控制 | `jmp`, `br`, `blt`, `switch`（表跳） | 回边计数前插 `tickloop`（承载超时） |
| 调用 | `call.stat`, `call.virt`, `call.host` | `call.host` = 进入 INFY 原生码（B 轨调 `std.*`） |
| 对象 | `new.str`, `new.list`, `new.dict`, `idx.get`, `idx.set` | 全部走 INFY 分配器与 §4.8 地址稳定规则 |
| 动态 | `tag.get`, `cast.dyn`, `is.dyn` | 仅 `dyn` 允许；校验器要求前置标签检查 |
| 协程/帧 | `frame.enter`, `yield`, `yield.from`, `frame.exit` | 生成器/装饰器展开用 |
| 异常 | `throw.inf`, `leave`, `finally.enter` | 与 `exception_table` 配对；跨边界展开受 §7.6 限制 |
| 边界 | `ret`, `ret.multi` | `ret.multi` 必须与 INFY 侧元组返回类型逐元一致 |

#### 7.8.3 装载期校验器（内存安全在字节码侧的落点）
执行任何 IFBC 前，运行时必须逐函数完成：
1. **线性性**：每条指令边界可判定，不存在落入指令中部的跳转（`R-0704`）。
2. **栈深度一致**：每个基本块入口栈高度唯一可推（否则 `R-0705`）。
3. **类型一致**：每个操作数类型与指令后缀匹配（`R-0706`）。
4. **帧合法**：`ret` 的类型与 `func_table` 声明一致；`yield` 只出现在协程帧（`R-0707`）。
5. **host 调用签名**：`call.host` 目标必须在 `import_table` 且签名逐元匹配 `IFABI`（`R-0708`）。
6. **行映射完备**：每条可执行指令有 `line_map` 覆盖（缺失 → `W-0705`，不阻断）。
校验通过前**不得**执行任何指令（UB-019）。校验是装载期一次性开销，不做"边执行边校验"。

#### 7.8.4 稳定性
- IFBC 版本随 `IFABI` 演进；v2.0 的 IFBC **不对第三方编译器工具链承诺稳定**（不鼓励手写 IFBC），但对 `.infyny` 内的段承诺：同一 `IFABI` 内向后兼容装载。
- 提供 `infy ifbc dump <artifact>` 与 `infy ifbc verify <artifact>`，用于诊断与合规测试（§12.3）。

### 7.9 嵌入通用运行时约束
- **取消与超时**：B 轨靠 `tickloop` 计数；A 轨靠 `isolation="process"` 时的进程终止或 `unsafe` 侧自行检查点——**规范不保证**同进程 A 轨代码可被抢占，`timeout_ms` 对其只在该单元回到 INFY 边界时生效（`R-0701`）。这是 A 轨的已知代价，必须在诊断 help 文本中说明。
- **资源配额**：`@embed(max_mem_bytes=…)` → 该单元独立 arena，超限 → `R-0709`。
- **副作用可见性**：B 轨函数若声明为纯（`fn f(x) -> y` 无 `#[effectful]`），编译器必须从 code object 的 `IMPORT_NAME`/`co_names` 证明其不触达文件、网络、进程与 `os`/`sys` 类内建，否则拒绝内联优化并提示 `W-0706`。
- **禁止隐式跨轨**：A 轨单元与 B 轨单元之间不得直接互相调用；必须经 INFY 侧 `event`/`fn` 中转（`E-0703`）。



---

## 8. 链与控制流

### 8.1 分类（`if` = 普通语句）
| 类别 | 允许出现位置 | 包含 |
| --- | --- | --- |
| **普通语句（Statement）** | `chain { ... }` 顶层 / 块内 / 连携块内 | 赋值、`if-else`、`while`、`loop`、`for`、`break`、`continue`、`return`、`var/let`、`@embed`、事件/fn 调用、`pause`、`stop`、`label` |
| **连携语句（Linkage Statement）** | **只能**出现在某语句尾部的 `linkage_block = { ... }` 内 | `goto(target)`、`start(chain=?, node=?)`、`start_flag`、`resume`；以及嵌套在里面的任意普通语句（if 可嵌套） |

> **设计要点**：`if` 是普通语句。它可以独立成为节点，也可以嵌套在连携块中（仍然是普通语义，只是执行时机变成宿主动作完成之后）。

### 8.2 链（Chain）生命周期
- 链在 `chain Foo { ... }` 声明时进入产物的**链镜像表**，不自动执行。
- **入口链**：见 §7.4 入口规则（`#![entry_chain]` > `fn main` > 主 package 内第一个 `chain`）。
- **节点静态编号**（编译型带来的变更）：编译器在 [7] IFIR 阶段为每条链生成**节点表**，`node_idx` 是编译期常量而非运行时计数。一个"节点"= 一条 `expr_stmt` 宿主语句 + 其连携块（与 v1.0 语义一致），编号按声明顺序连续分配，跨构建必须稳定（UB-022）。
- **自动滑进**：链当前节点的**最后一个普通语句**及其所有连携块执行完毕后，焦点 `node_idx += 1`（若越界 = 链结束）。
- **`goto` 语义**（移动焦点，暂停当前节点之后）：
  - `goto("anchor_name")` = 跳到本链 `label anchor_name:` 或 `start_flag` 锚点。**编译期解析**：同链内无该锚点 → `E-0301`。
  - `goto(+3)` / `goto(-1)` = 相对当前 node_idx。目标越出 [0, 节点数) → `E-0301`（常量偏移在编译期即可判定）。
  - `goto(5)` 绝对索引：仅允许出现在节点数受构建配置影响而无法静态校验的场景；否则推荐用命名锚点。运行时目标不存在 → §4.9 UB-013 → `R-0010`。
- **`start` 语义**（不移动焦点，并行新增）：
  - `start(chain = MonitorChain)` = 启动 `MonitorChain` 的焦点从其入口节点执行；当前链焦点继续原位置推进。链名在编译期必须可解析 → 否则 `E-0800`。
  - `start(chain = C, node = 5)` 或 `node = "anchor"` 指定起始节点；`"anchor"` 必须位于链 C 的节点表内 → 否则 `E-0801`。
  - 对已执行中链再次 start：**重入安全**——启动第二条独立焦点，互不干扰（同链多焦点并行）。每条焦点拥有独立栈与独立所有权作用域（§4.8），跨焦点共享只能经 `Arc`/`Channel`（§4.3）。
- **`start_flag`**：无操作 NOP，仅作为锚点。
- **`stop` 语义**：**普通语句即可使用**，也可作连携。执行后所有链**在完成当前节点的全部普通+连携动作后**优雅停止（非强杀）。停止时按 §4.7.4 drop 顺序释放各焦点，泄漏检测（`R-0723`）在此时点执行。
- **`pause` / `resume`**：暂停 = 完成当前节点后挂起，不释放已持有的资源（模型、句柄、连接、锁）；resume 从焦点位置继续。

### 8.3 ResourceLock FIFO 调度（全局）
- 资源名：任意字符串（例 `"db:orders:writer"`、`"runner:ci-7:gpu"`、`"queue:ingest::shard-3"`）。语言层不定义资源名命名空间，只定义其唯一性与 FIFO 语义。
- 通过 `std.chain.lock(resource: &str) -> Guard` 在进入事件前排队，`Guard` drop 时释放（RAII，§4.7.4；不再依赖"退出事件"这一隐式时点）。
- 严格按提交时间戳 FIFO 互斥。同资源并发 → 后提交者挂起直到前持有者释放。公平性：排队不得饥饿（无等待者跳过）。
- **死锁不在规范承诺范围内**：FIFO 只保证同资源互斥顺序，不提供锁序检查。`infy check --deadlock` 提供可选的静态锁序图分析，发现环 → `W-0800`（strict 阻断）。

### 8.4 节点表的编号算法（规范唯一）
编译型意味着 `node_idx` 必须在编译期定死，而编号规则若因实现不同而变化，日志、`goto`、`.infib` 里的锚点全部会错位。规则如下：
1. 一条 `chain` 的节点 = 其块内**顶层语句**序列中的每一个 `expr_stmt`（含 `@embed` 调用），**不含** `let`/`var`/纯赋值/`label`（它们归属前一个节点，作为其"序言"）。
2. 编号从 0 起，按源码书写顺序连续递增；`if`/`match` 的分支体**不产生新节点**（它们是节点内部的控制流，见 §8.1 的"`if` 是普通语句"）。
3. 一个节点内可挂多个 `linkage_block`（分别属于不同的宿主语句），它们共享该节点编号；焦点推进只看节点编号。
4. `defer`、临时量 drop 不改变编号，但计入该节点的退出动作集。
5. 编号写入产物 `.inf meta` 的 **节点表**（`chain_id, node_idx, file:line:col, linkage_count, host_event_id?`）。`infy artifact dump --nodes` 可导出。
6. **稳定性要求**：同一 module 源码在未修改的情况下，任何 `-O` 级别、任何 target、任何实现产出的节点表必须逐行一致（§12.3 黄金文件项）。修改源码中间的某一行会**移动其后所有节点**，因此 `goto(绝对索引)` 被视为脆弱写法，诊断对 `goto(<纯数字>)` 一律附加 `N-0800` 提示改用命名锚点。
7. 跨版本发布时用 `infy artifact diff a.infyny b.infyny --nodes` 检查节点表漂移，供日志回溯。

### 8.5 焦点（Focus）的运行时模型
- **定义**：焦点 = 一条链上一个独立的推进位置（`chain_id` + `node_idx` + 该焦点的栈与所有权作用域）。`start(chain=X)` 每次创建一个新焦点；同链可有 N 个并发焦点（§8.2）。
- **承载**：每个焦点是一条**用户态协程栈**（默认 256 KiB，可 `[profile] focus_stack_bytes`），挂在 OS 线程池上；OS 线程数默认 = `min(逻辑核数, 8)`，可由 `INFY_THREADS` 覆盖。焦点切换不进入内核。
- **抢占点（封闭）**：① 节点边界（`node_idx += 1` 前）；② 每个 A/B 轨边界返回后；③ 每个 `std.chain.yield_now()`；④ 每个 `tickloop` 计数达到 65536；⑤ 每个 `Mutex`/`Channel` 阻塞点。**普通 INFY 代码内无抢占点**——长纯计算循环不会被中断，这既是性能选择也是取消语义的边界，规范在 `W-0801` 提示"超过 N 次迭代的无边界循环可能导致停止迟滞"。
- **公平性**：调度器采用**有界轮转 + 每链配额**（`[profile] focus_quota`，默认每节点 1 个调度片），保证任意已就绪焦点在 `O(焦点总数)` 片内被服务；禁止实现用"最新焦点优先"或哈希序遍历（不可复现）。
- **失败半径**：见 §4.16。默认 `focus`——一个焦点 panic 不波及其余焦点。
- **取消传播矩阵**：
| 触发 | 影响的焦点 | 时机 |
| --- | --- | --- |
| `stop` | 全部焦点 | 当前节点完成（含 defer/drop）后不再推进 |
| `pause` | 当前焦点 | 当前节点完成后挂起，保留栈与堆 |
| 焦点 panic | 仅该焦点（或按 `panic_radius`） | unwind 立即 |
| `R-0701` 超时 | 发起调用的那个嵌入帧所属焦点 | 到下一个抢占点 |
| 宿主 `request_stop` | 全部 | 同 `stop` |
| ResourceLock 持有者失败 | 等待该资源的焦点继续挂起，但锁在 drop 时释放（RAII 保证，§8.3） | unwind 完成时 |
- 焦点 ID (`env.focus_id`) 单调分配、复用需跨过 `u32` 回绕点；回绕行为：不回收、直接 `R-0802` 拒绝新焦点（避免 ID 混淆导致日志错乱，宁停不混）。


---

## 9. 诊断引擎

### 9.1 诊断分类与编号范围
| 前缀 | 级别 | 含义 | 范围 |
| --- | --- | --- | --- |
| `E-` | 错误 | **必须阻断构建/运行**。检查不通过不可产出代码。 | 0000–1999 |
| `W-` | 警告 | 默认阻断（strict）；lenient 允许。 | 0000–1999 |
| `R-` | 运行时 panic | 执行期确定性终止（不是 UB，是规范 panic）。 | 0000–1999 |
| `N-` | 通知 | 仅信息（fix-it 成功 / `infy fmt` 改动统计）。 | 0000–999 |
| `X-` | 实现扩展 | 允许实现自定义的**唯一**命名空间，必须 `X-<impl-id>-<n>`；不得与 E/W/R/N 数值区间重叠 | — |

**区间分配表**（规范性；新增码必须落入对应区间，越界 = UB-024）：
| 区间 | 主题 | 区间 | 主题 |
| --- | --- | --- | --- |
| `000x`–`003x` | 词法基础：编码、注释、标识符、关键字（§2.1–§2.5） | `06xx` | 构建/链接/工具链/可复现（§7.1–§7.3） |
| `004x`–`005x` | 字面量、字符串、Unicode 安全（§2.8–§2.11） | `07xx` | 嵌入：通用 070x，B 轨 071x，A 轨 072x，IFBC 073x |
| `006x` | 编译期常量求值 `const-eval`（§2.12） | `08xx` | 链、节点表、焦点调度（§8） |
| `007x` | 模式与穷尽性（§3.12） | `09xx` | 宿主事件、`HostApi`、握手（§11） |
| `008x` | 求值顺序（§4.10） | `016x` | 类型推断与 trait 求解（§4.13–§4.14） |
| `009x` | `infy fmt` 自身故障（§9.5） | `017x` | 错误传播 `?`、单态化、`defer`（§4.11/§4.15） |
| `010x`–`011x` | 类型与运算符（§4.1–§4.4） | `018x` | 格式化与迭代器（§10.10–§10.11） |
| `012x` | 可变性（§4.3） | `042x` | derive 生成失败（§6.7） |
| `013x`–`015x` | 内置函数 / env / 字符串（§4.5–§4.6） | `05xx` | 模块/包/依赖/`.infib`/校验（§5） |
| `030x` | 语句与连携（§3.5/§8.1） | `100x` | 所有权/借用/生命周期/drop（§4.7） |
| `040x`–`041x` | 宏与属性（§6.1–§6.6） | `104x` | 内存布局与栈（§4.8） |
| `105x` | IFBC 装载校验（§7.8.3，多为 `R-`） | 码数合计 | **210**（§9.3 主表 + §9.7 补全表） |

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

**示例**（实现输出的字节序列必须与此模板逐字符一致，含空白）：
```
error[E-0111]: 隐式浮点/整数互转：期望 u32，实际 f32
  ┌─ cmd/host/main.infy:12:25
  │
12│     var secs: u32 = ticks * 1.5;
  │                     ^^^^^^^^^^^  表达式类型为 f32
  │
  = note: `ticks` 类型为 u32，乘以 f32 字面量后结果类型为 f32
  = help: 使用 round10() 或显式 `as` 转换
  12│     var secs: u32 = round10(ticks * 1.5) as u32;
  │                     ++++++++                ++++++
```

### 9.3 诊断码全集（v2.0）

**编译期错误 `E-`**：
| 码 | 标题 | 触发场景 | 修复建议 |
| --- | --- | --- | --- |
| E-0001 | 语言版本不兼容 | `#![infy_version = "1.0"]` 在 v2.0 编译器上 | 升级源码声明或删除该行 |
| E-0002 | 源码非 UTF-8 | 非法字节序列 | 转为 UTF-8 |
| E-0003 | CR 换行 | Mac 老式 CR | 改为 LF（`infy fmt` 自动修复） |
| E-0004 | 未闭合块注释 | `/*` 无匹配 `*/` | 补全闭合 |
| E-0005 | 注释嵌套过深 | >255 层 | 简化注释 |
| E-0006 | 混淆标识符 | NFKC 归一冲突 | 更换标识符 |
| E-0007 | 保留前缀标识符 | 用户定义 `__foo` | 删除前置 `__` |
| E-0008 | 歧义整数字面量 | 未加后缀且无法推断 | 加后缀 `123u32` |
| E-0009 | 未闭合字符串 | 缺少 `"` | 补全 |
| E-0010 | 未闭合原字符串 | `r#"` 定界不匹配 | 增加 `#` 数量 |
| E-0011 | 构建约束位置错误 | `//infy:build` 不在首个非空行 | 上移到首行 |
| E-0012 | 未知构建约束名 | `//infy:build riscv` | 用 §2.2 封闭集或在 `[features]` 声明 |
| E-0013 | 重复构建约束注释 | 同一文件出现两条 `//infy:build` | 合并为一条布尔表达式 |
| E-0020 | 保留字作标识符 | `let use = 1;`、`fn mod() {}` | 改名（§2.4 保留集） |
| E-0021 | `let mut` 非法 | `let mut x = 1;` | 改 `var x = 1;`（§4.3） |
| E-0022 | `var` 用于形参 | `fn f(var x: u32)` | 改 `fn f(mut x: u32)`（§4.3） |
| E-0030 | `auto` 出现在非法位置 | `let x = auto;`、`return auto;` | 只用于具名实参（§2.6） |
| E-0101 | 未初始化变量读取 | 读取未定型路径上的绑定 | 初始化 |
| E-0110 | 隐式数字转换 | `u32 + u8` | 显式 `as` |
| E-0111 | 隐式浮点/整数互转 | 同上 | 显式 `roundN` / `as` |
| E-0120 | 修改 `let` 不可变绑定 | `let x = 1; x = 2;` | 改 `var` |
| E-0121 | 跨链可变共享无同步 | 多焦点写同一绑定 | `Arc<Mutex<T>>` 或 `Channel<T>` |
| E-0130 | `roundN` 的 N<10 | `round3(x)` | 用 ≥10 的 N |
| E-0131 | 内置函数无法常量求值 | `std.mem.size_of<dyn Unknown>()` | 给定具体类型 |
| E-0140 | 写入 `env` 只读变量 | `env.gen = 5` | 禁止写入 |
| E-0150 | 字符串含 NUL 注入 | `"a\0b"` 跨 A 轨边界 | `std.str.strip_nul` / `to_cstr()` |
| E-0200 | lock 与清单不同步 | `MODULE.infy.lock` 含清单已不要求的依赖，或缺当前所需项 | `infy mod tidy` 重建 lock |
| E-0201 | 导入未声明的 module | 清单无该 `[dependencies]` 项 | `infy mod tidy` |
| E-0202 | 导入目标不存在 | 路径 404 / 缓存缺失且离线 | 修正路径或 `infy mod download` |
| E-0210 | 访问 package 私有项 | 跨目录引用非 `pub` 项 | 加 `pub` 或改 API |
| E-0300 | 连携语句独立出现（UB-011） | `goto("x");` 直接写在 chain 顶层 | 包进宿主语句的 `{ ... }` 连携块 |
| E-0301 | `goto` 目标编译期不可解析 | 锚点不存在 / 常量偏移越界 | 检查 `label` 拼写与节点范围（UB-013） |
| E-0400 | 宏作用域泄漏 | 卫生宏展开命名冲突 | 重命名宏内标识符 |
| E-0401 | 宏展开深度超限 | >128 | 简化递归 |
| E-0402 | 宏展开产物非法 | 展开后不是合法 AST 片段 | 修正展开器（诊断指向展开器） |
| E-0410 | 未知属性 | `#[myattr]` | 用 §6.6 封闭列表内属性 |
| E-0501 | 包名与目录不符 | `package a` 在 `core/b/` 下 | 改为 `package b` |
| E-0502 | 循环包依赖 | `a→b→a` | 提取第三包或反转依赖（诊断打印完整环） |
| E-0503 | 循环 module 依赖 | module 级环 | 同上，跨 module 层 |
| E-0504 | 非法导入路径 | 相对路径 / 非字符串 / 通配 | 用 module path 起算的完整路径 |
| E-0505 | 目录名不可导入 | 大写 / 中文 / 首字符数字目录名 | 目录名改小写 ASCII（§5.1 R3） |
| E-0506 | 清单语法错误 | `MODULE.infy.toml` 非法 TOML | 按 §5.2 修正 |
| E-0507 | module path 与 `std` 冲突 | `path = "std_x"` 前缀违规 | 改用自有域名前缀 |
| E-0508 | 越界访问 `internal` | `a/x/internal/c` 被 `a/y` 导入 | 提升为公开 API 或移入同子树 |
| E-0509 | `internal` 嵌套 `internal` | `internal/internal/` | 删除内层 |
| E-0510 | 版本需求不可满足 | MVS 无解 | 放宽 `[dependencies]` 区间 |
| E-0551 | 库文件不可执行 | `infy run foo.infib` | 在调用方 module 内 `import` 它 |
| E-0552 | 库文件含入口 | `.infib` 内有 `fn main`/`entry_chain`/`[[bin]]` | 拆成 `[[bin]]` module |
| E-0553 | 库文件夹带未声明动态库 | wheel 版出现游离 `[[link]]` 路径 | 在清单显式声明 target 与库 |
| E-0560 | 校验和不匹配 | dirhash ≠ `MODULE.infy.lock` | 回滚或 `accept-changes`（§5.8 第 2 项） |
| E-0561 | 校验数据库不可达 | 首次解析该版本且离线 | 配 `INFY_NOSUMDB` 或先 `infy mod vendor` |
| E-0562 | 校验数据库与本地记录冲突 | 公开日志已改写该版本 | 立即冻结该依赖，视为供应链事件 |
| E-0570 | 缓存目录不可写 | 权限/磁盘满 | 设 `INFY_CACHE`（不阻断，仅无缓存） |
| E-0600 | 产物无入口 | 既无 `entry_chain` 也无 `fn main` | 声明 `#![entry_chain]` |
| E-0601 | 入口声明冲突 | 多个 `entry_chain`/`[[bin]]` 争抢 | 只留一个或分 `[[bin]]` |
| E-0602 | A 轨工具链缺失/未注册 | `@c` 但无 `cc` 记录 | `infy toolchain probe` / `add` |
| E-0603 | 可复现构建不一致 | `--verify-reproducible` 两次产物不同 | 查 §7.3 差异段并上报实现 bug |
| E-0604 | 工具链版本漂移 / 符号未导出 | wheel 记录版本 ≠ 实际；或链接缺符号 | 对齐编译器版本 / 补源码导出（UB-023） |
| E-0605 | ABI 或布局不一致 | `IFABI` 冲突、跨边界类型缺 `#[repr(C)]` | 统一 `IFABI` 或标注布局（UB-021） |
| E-0606 | 依赖间编译策略冲突 | `overflow`/`fast_math` 混用 | 统一到主 module 策略 |
| E-0607 | wheel 未覆盖本机 target 且无源码版 | `targets` 列表不含本机 triple | 发布含本机 target 的 wheel 或附源码版 |
| E-0700 | 嵌入块位置非法 | `@py` 出现在模式/类型位置 | 移到 `event`/`fn` 体或语句位 |
| E-0702 | 未知或不支持的嵌入前缀 | `@forth`、`@ruby`、`@php`、`@java`(v2.0) | 用 §7.5 白名单内前缀 |
| E-0716 | 进程隔离前缀缺 `isolation` | `@js { ... }` 未写 `isolation="process"` | 补参数，或改用 `@py`/`@lua`（§7.7.5 R-JS） |
| E-0703 | 跨轨直接调用 | A 轨单元直调 B 轨单元 | 经 INFY 侧 `event` 中转 |
| E-0711 | 动态代码生成 | `eval` / `exec` / 运行时构造 code object | 静态展开，或改 A 轨 / `isolation="process"`（§7.7.2） |
| E-0712 | 字节码能力缺失 | 摄取到的 code object 含未映射 opcode / 未实现 builtin / 未声明 import | 按诊断列出的具体项等待能力表扩容，或改写该处 |
| E-0713 | 字节码版本不匹配 | `.pyc` magic 或实际解释器版本 ≠ 清单 `[b_track] py` 声明 | `infy toolchain add py --version <声明版本>` 或改声明 |
| E-0714 | wheel 含原生二进制 | `infy ingest py-wheel` 输入非 `py3-none-any` | 走 A 轨链接其 C API，或 `isolation="process"` |
| E-0715 | 未声明 B 轨工具链版本 | 使用 `@py` / `@lua` 但清单无 `[b_track]` 条目 | 声明 `[b_track] py = "3.13"`（§7.7.3） |
| E-0720 | A 轨 `name` 不匹配 | `#[link(name="a")]` 对不上 `@c(name="b")` | 统一 name |
| E-0721 | A 轨调用缺 `unsafe` | 直接调 `extern` 函数 | 包 `unsafe { }` + `// infy-safety:` |
| E-0722 | 未声明 A 轨信任策略 | 用了 `@c` 但清单无 `a_track` | `[profile] a_track = "unsafe-trusted"` |
| E-0723 | 跨边界指针移交未声明 | `extern` 返回裸指针无 `#[owns]`/`#[borrows]` | 二选一显式写出（UB-010，无默认） |
| E-0724 | 回调指针未校验即解引用 | `#[export]` 侧直接信任外部指针 | 走 §7.6.5 thunk |
| E-0800 | 链不存在 | `start(chain = NoSuch)` | 修正链名 / 先声明 |
| E-0801 | 链锚点不存在 | `start(chain=C, node="anchor")` 不在 C 节点表 | 检查锚点属于该链 |
| E-0900 | 宿主事件参数不匹配 | 调用 native `event` 缺必填参数 | 补参数或给默认值 |
| E-1001 | 使用已移出的绑定 | `let a = v; use(v);` | 用 `clone()` 或改传 `&v` |
| E-1002 | 借用冲突 | `&mut` 与 `&` 共存 / 双 `&mut` | 缩短作用域或改 `RefCell` |
| E-1003 | 生命周期无法唯一确定 | elision 三规则均不适用 | 显式 `'a` 标注（§4.7.3） |
| E-1004 | 返回局部引用 | `&x` 逃逸 | 返回所有权类型 / `Box<T>` |
| E-1005 | 借用已移出的值 | `let r = &moved` | 先借用后移动 |
| E-1030 | 裸指针解引用缺 `unsafe` | `*p` | 包 `unsafe` 并写前提 |
| E-1031 | `static mut` 访问缺 `unsafe` | 读写全局可变 | 改 `Mutex<..>`/`Arc<..>` |
| E-1032 | `unsafe` 项未包 `unsafe` | 调 `unsafe fn` | 包 `unsafe` |
| E-1033 | `union` 非活跃变体访问 | 读未写过的变体 | 记录活跃变体或用 `enum` |
| E-1034 | `transmute` 尺寸/对齐不匹配 | `transmute::<u32,u64>` | 显式 `as` 或包结构体 |
| E-1040 | 非法对齐 | `#[align(1)]` 于 8 对齐类型 | 只允许增大对齐 |

**警告 `W-`**：`W-0001` CRLF、`W-0002` 缺 trailing LF、`W-0003` 构建约束恒 false、`W-0400` 使用已弃用项、`W-0500` 清单未知键、`W-0604` 允许工具链漂移（产物标 `unreproducible`）、`W-0700` 使用 `env.return` 旧写法、`W-0705` IFBC 行映射缺失、`W-0706` 声明纯函数但存在宿主副作用、`W-0720` A 轨导出符号未被任何签名声明使用、`W-0800` 静态锁序环、`W-1010` 未使用的生命周期、`W-1011` `dyn` 出现在 `pub` API、`W-1012` `std.host.event` 出现在 `pub` API、`W-1030` `unsafe` 块缺 `infy-safety` 注释。

**运行时 panic `R-`**：
| 码 | 含义 | 码 | 含义 |
| --- | --- | --- | --- |
| R-0001 | 整数溢出（`overflow=panic`） | R-0701 | 嵌入执行超时（B 轨回边计数 / A 轨边界点） |
| R-0002 | 除 0 | R-0703 | IFBC 装载校验失败（总类） |
| R-0003 | `str` 运行时检出 NUL | R-0704 | IFBC 非线性指令流 |
| R-0004 | 数组越界 | R-0705 | IFBC 栈高度不一致 |
| R-0005 | `Option/Result::unwrap()` 于 None/Err | R-0706 | IFBC 操作数类型与后缀不符 |
| R-0006 | `bool` 非法位模式 | R-0707 | 协程指令出现在非协程帧 |
| R-0010 | `goto` 运行时目标不存在 | R-0708 | `call.host` 签名与 `IFABI` 不符 |
| R-0709 | 嵌入单元超出 `max_mem_bytes` | R-0710 | `dyn` 标签与声明类型不符（UB-020） |
| R-0720 | A 轨跨边界指针调用期无效 | R-0721 | 异常跨 `"C"` ABI 边界展开 |
| R-0722 | A 轨 `Result` 出参未写 | R-0723 | 回调 `Arc` 卸载时计数非 0（泄漏） |
| R-1010 | drop 期间被跨链访问 | R-1040 | 链栈溢出 |
| R-0900 | 宿主事件返回未映射错误 | R-0901 | `std.host.query` 键类型与宿主定义不符 |

**废止但不复用的编号**：`E-0701`（v1.0 的"编译型语言嵌入禁令"）与 `R-0702`（v1.0 的"嵌入 runtime 缺失"）在 v2.0 废止。编号**永久保留、不得赋予新语义**——历史诊断日志与 §12 回归用例仍可能引用它们。A 轨的对应新码见 `E-0602`/`E-0722`。

### 9.4 `infy check` 与 `infy build` 的分工
- `infy check`：只跑 §7.1 的阶段 [1]–[5]（含 B 轨能力扫描，若源语言工具链在场）。**不产出代码、不需要 C 编译器**。退出码 `0`（零 E 且零 W-strict）、`1`（有 E）、`2`（strict 下有 W）。
- `infy build`：跑完整 [1]–[10]。退出码沿用 §13.4，另加 `6`（工具链/链接失败）。
- 两者诊断**集合必须一致**（`build` 只会追加 [6b]–[9] 的新码，不得改变已有码的位置与文本）。
- 全部诊断输出到 stderr，并支持 `--diag-json out.json` 结构化副本（CI 用）。`--diag-json` 的 schema 是规范的一部分：`{code, level, message, notes[], help[], spans[{file,line_start,line_end,col_start,col_end,label}], suggestion[{op:"insert"|"delete"|"replace", span, text}]}`。

### 9.5 `infy fmt` 风格规范（必须唯一，故逐条写死）
"字节级唯一"若只作口号而没有规则，就仍然不确定。以下即规则本身；实现不得偏离（UB-014）。

1. **宽度**：软上限 100 列，硬上限 120 列。列宽按**字素簇**数计（CJK 全角字符计 2 列，按 `EastAsianWidth.txt` 已声明版本；该表版本写入工具版本而非产物）。
2. **缩进**：4 空格，永不使用制表符。`@embed` 块**内部**内容原样保留（§2.11 词法不透明），只规范化块的首行缩进层级。
3. **必须换行**：任一 `let`/`var`/赋值/表达式语句超过软上限即换行；链式调用超过软上限时在**每个 `.` 前**断行，续行缩进 8 空格，且 `?` 跟随其依附的表达式不换行。
4. **允许换行**：实参列表、数组/元组字面量、`match` 臂、`where` 子句在超限时逐项一行；**未超限时禁止**逐项一行（格式化必须把可单行的压回单行）。
5. **尾随逗号**：多行列表**必须**带尾随逗号；单行列表禁止。二者都属规范化结果，`fmt --check` 据此判定。
6. **空行**：连续空行折叠为 1；块首尾的空行删除；函数/事件/类型声明之间恰好 1 个空行（文件内首个声明前 0 个）；`import` 块与 `package` 之间 1 个；`package` 前 0 个。
7. **`import` 分组与排序**：按前缀分三组——`"std..."`、`"宿主 module 同名前缀"`、`其他全路径`；组内按**字节序**升序；组间 1 个空行；同路径重复导入合并（保留最后一个 `as` 别名）。
8. **注释**：`//` 前置空格（`//foo` → `// foo`）；`///` 文档注释内容**一字不改**，仅对齐缩进；`/* */` 保持原位与内部内容；`//infy:build` 固定在首行且删除其后紧邻空行；`// infy-safety:` 必须留在其 `unsafe` 块同一行或紧邻上一行，否则 `W-1030`。
9. **禁止改写的内容**：字符串/字节串内部（含 §2.9 的多行 raw 缩进剥离已在词法期完成，fmt 不再动）、标识符、注释正文、`@embed` 内部、`auto` 等字面量拼写、`_` 分隔符位置（`1_000` 不被折叠成 `1000`）。
10. **属性与可见性**：`#[..]` 与 `pub` 各占一行前置，多个 `#[..]` 按书写顺序保留；`#[repr(C)]` 不得被移到 `impl` 上。
11. **文件级**：LF、无 BOM、恰好 1 个结尾 LF（缺失补 `W-0002` 并自动修）；空文件视为格式非法（`E-0090`）；`.infy` 之外的文件不处理。
12. **幂等与不动点**：`fmt(fmt(x)) == fmt(x)` 必须对**任意**输入成立（含畸形但可解析输入）。实现必须内置自检：`infy fmt` 内部对输出再格式化一次并比对，不等 → `E-0091`（编译器 bug，拒绝写出）。
13. **AST 不变性**：`fmt` 前后 AST（除注释位置元数据）必须同构；违反 → `E-0092` 并保持原文件不动。
14. **`.infyfmt`**：**不提供**风格配置项。唯一允许的配置文件是排除列表（`exclude = ["gen/**"]`）与行宽覆盖（`max_width`，仅允许 80/100/120 三档之一，且仍计入 §12.3 黄金用例分档）。理由：风格可配置会摧毁"官方风格唯一"与跨团队 diff 稳定性。

### 9.6 严重级别调整（`[lints]`）
```toml
[lints]
W-0070 = "deny"      # 枚举遗漏变体 → 升级为错误
W-0801 = "allow"     # 长循环无抢占点 → 关闭
E-0063 = "allow"     # 谨慎：允许 const_assert 失败降级（默认禁止此降级）
```
- 只有 `W-` 与 `N-` 可被调整级别；`E-` 与 `R-` **不可**降级（降级即 UB，实现若接受该配置 → `E-0093`）。`E-0063` 例外由规范明示：`const_assert` 属用户自设断言，允许 `allow`，但允许后仍不得跳过求值。
- 调整只在**主 module** 生效，不传递给依赖（依赖按自己的清单，链接期不一致 → `E-0606`）。


### 9.7 诊断码补全（新增章节对应项）

**词法与编译期求值（§2.8–§2.12）**
| 码 | 标题 | 触发场景 | 修复建议 |
| --- | --- | --- | --- |
| E-0040 | 非法数字分隔符 | `_` 连用 / 首尾 `_` | 移除多余分隔符 |
| E-0041 | 分隔符位置非法 | `_` 贴近 `.` 或 `e` | 移到数字之间 |
| E-0042 | 字面量超出可表示范围 | 无后缀整型超 `i128` | 加后缀或改常量 |
| E-0043 | 带后缀字面量溢出 | `129i8`、`300u8` | 换更宽后缀（不回绕、不饱和） |
| E-0044 | 浮点字面量写法非法 | `1.` | 写 `1.0` |
| E-0045 | 浮点字面量为无穷 | `1.0e999f32` | 用 `std.f32.INF` 显式表达 |
| E-0046 | 未知转义序列 | `\\q` | 查 §2.9 封闭表 |
| E-0047 | `str_lit` 内使用字节转义 | `"\\x41"` | 改 `b"\\x41"` 或 `\\u{41}` |
| E-0048 | 多行 raw 缩进混用制表符与空格 | 公共前缀含两类 | 统一缩进字符 |
| E-0049 | `char_lit` 含图符簇 | `'e\\u{301}'` | 改用 `str` |
| E-0050 | 混淆标识符（confusables） | 西里尔 `а` 混拉丁 `a` | 更换字符 |
| E-0051 | 脚本混用非法 | Han 与谚文同标识符 | 拆分命名 |
| E-0052 | 零宽/方向控制字符 | `\\u{200B}` 出现在源码 | 删除；确需则放字符串字面量 |
| E-0053 | 语句缺少终结符 | 非块/非尾表达式却无 `;` | 补 `;`（无自动插入，§2.11） |
| E-0060 | 编译期求值遇禁止构造 | const 内 IO / `env.*` / 堆分配 | 移出 const 上下文（§2.12） |
| E-0061 | const-eval 深度超限 | >512 层调用 | 拆分常量表达式 |
| E-0062 | 编译期常量溢出 | `const` 算术越界 | 显式 `wrapping_*` 或改值 |
| E-0063 | `const_assert` 失败 | 条件为假 | 修正断言或值；可 `[lints] allow` |
| E-0090 | 空源码文件 | 0 字节的 `.infy` | 删除该文件或写入 `package` |
| E-0091 | `infy fmt` 不幂等 | 内部二次格式化不一致 | 编译器 bug，拒绝写出 |
| E-0092 | `infy fmt` 改变 AST | 格式化前后不同构 | 编译器 bug，保持原文件 |

**文法与模式（§3.11–§3.13）**
| 码 | 标题 | 触发场景 | 修复建议 |
| --- | --- | --- | --- |
| E-0070 | `match` 不穷尽 | 存在未覆盖值 | 补臂或用 `_`（枚举建议列全） |
| E-0071 | 不可达匹配臂 | 与前面各臂差集为空 | 删除该臂 |
| E-0072 | 或模式绑定集不一致 | `A(x) \\| B(_)` | 两侧同名同类型 |
| E-0073 | 模式嵌套过深 | >32 层 | 拆分为嵌套 `match` |
| E-0074 | `defer` 内非法控制流 | `defer { return; }`、`defer` 内 `?` | 移到普通语句 |

**类型、推断与内存（§4.10–§4.16）**
| 码 | 标题 | 触发场景 | 修复建议 |
| --- | --- | --- | --- |
| E-0080 | 求值顺序无法确定 | 两个副作用无序列点且写重叠 | 拆成语句（§4.10 第 10 条） |
| E-0160 | 推断展开深度超限 | 类型变量互推 >8 层 | 显式标注其一 |
| E-0161 | 闭包形参不可推断 | 存储后再调用 | 写 `\|x: u32\|` |
| E-0162 | 方法名歧义 | 同层两个 trait 提供同名方法 | `Trait::f(x)` 消歧 |
| E-0163 | 关联类型缺失 | `impl` 未给非 default 项 | 补 `type X = ..` |
| E-0164 | 关联类型重复提供 | 同一 `impl` 内两次 | 删其一 |
| E-0165 | trait bound 求解过深 | 搜索 >64 层 | 加显式 bound 或拆 trait |
| E-0166 | bound 推导成环 | `impl<T: Foo<T>> Bar for T` | 打破自依赖 |
| E-0167 | trait 非对象安全 | 泛型方法 / `Self` 返回 | 改泛型参数或拆 trait |
| E-0168 | 违反孤儿规则 | 外部 trait 对 外部 type 做 impl | 包 newtype |
| E-0169 | impl 重叠不可判定 | 两个 impl 交集无法判空 | 收窄其中一侧 |
| E-0170 | `?` 缺 `From` 实现 | 错误类型无法转换 | 写 `impl From<A> for B` 或显式 `map_err` |
| E-0171 | `Option` 在返回 `Result` 处用 `?` | 类型族不匹配 | `.ok_or(..)` |
| E-0172 | `?` 所在函数返回类型非法 | 非 `Result`/`Option` | 改签名 |
| E-0173 | `?` 出现在链顶层 | 无函数返回边界 | 改连携处理（§8.1） |
| E-0174 | 泛型实例化总数超限 | >65536 | 改 `dyn` 或收窄单态化 |
| E-0175 | 泛型递归尺寸无限 | `struct L<T>{ t:T, next:L<T> }` | 插入 `Box` |
| E-1006 | 从引用移动全部字段 | `S { ..&base }` | 改 `clone()` 或取所有权 |
| R-0007 | 移位量 ≥ 类型宽度 | `1u8 << 8` | 掩码或加宽 |
| R-0008 | `inf % x` | 浮点无穷取模 | 先判 `is_finite` |
| R-0009 | `ilog2/ilog10` 于 0 | 定义域 | 判零后调用 |
| R-0011 | drop 中 panic（双 panic） | `Drop` 内 `unwrap` 失败 | drop 不得失败 |
| R-0012 | 运行时 radix 越界 | `from_str_radix(.., 37)` 非常量 | 编译期常量传参 |
| W-0160 | 实例化次数偏高 | 单函数 >512 实例 | 改 `dyn`/`Box` 参数 |
| W-0170 | 丢弃 `Result`/`Option` | `f();` 未消费 | `let _ = f();` 或处理 |
| W-0185 | 依赖 `HashMap` 迭代顺序 | 顺序影响输出 | 改 `Map`（有序） |

**宏与 derive（§6.7–§6.8）**
| 码 | 标题 | 触发场景 | 修复建议 |
| --- | --- | --- | --- |
| E-0420 | `#[skip_clone]` 字段无 `Default` | derive Clone 无法填值 | 实现 Default 或去掉 skip |
| E-0421 | `Copy` derive 非法 | 含 `Drop`/`dyn`/非 Copy 字段 | 只 derive Clone |
| E-0422 | `Default` derive 于 enum 无零值变体 | 首变体字段缺 Default | 加 `#[default]` |
| E-0423 | 多个 `#[default]` 变体 | enum 上重复 | 保留一个 |
| E-0424 | 含浮点字段却 derive `Eq` | `NaN != NaN` | 加 `#[allow_nan_equality]` |
| E-0425 | derive `Eq` 但字段非 `Eq` | 混合类型 | 先满足约束 |
| E-0426 | 重复 derive | `#[derive(Clone, Clone)]` | 合并 |
| E-0427 | derive 需要 `unsafe` | 生成代码触及裸指针 | 手写该 impl 并写 safety 注释 |

**运行时 / IFBC / 宿主（§7.8、§8.5、§10、§11）**
| 码 | 标题 | 触发场景 | 修复建议 |
| --- | --- | --- | --- |
| R-0712 | 动态调用目标越界 | `invoke.dynamic` 指向模块外函数 | 改走 `call.host` |
| R-0713 | 非 `repr(C)` 求字段偏移 | `field_off` 于默认布局类型 | 标注 `#[repr(C)]` |
| R-0730 | 丢弃必释放值 | `pop` 掉 `#[must_drop]` 类型 | 显式 `drop(..)` |
| R-0731 | 读取已移出的局部 | `ldl i` 于 `move.out` 之后 | 重新赋值后再读 |
| R-0802 | 焦点 ID 回绕 | `focus_id` 超过 `u32` | 回收长驻焦点 / 重启进程 |
| R-0803 | 无限迭代器消费 | `collect` 非 `ExactSizeIterator` 超 `iter_guard_steps` | 改 `take(n)` |
| R-0902 | 宿主回调嵌套过深 | >8 层互递归 | 改为事件队列 |
| R-0903 | `IFABI` 不匹配 | 产物与宿主 ABI 版本不同 | 同步重建 |
| R-0904 | 宿主事件缺失/签名不符 | `EventTable` 项在宿主不存在 | 补注册或重编 |
| R-0905 | `EnvSnapshot` 布局不符 | 字段偏移表不一致 | 升级宿主绑定 |
| E-0554 | `.infib` 清单缺必需字段 | 无 `ifabi`/`abi_hash`/`deps.dirhash` | 用 `infy lib pack` 重产 |
| N-0800 | `goto` 用绝对索引 | 源码改动会移动节点 | 改命名锚点（§8.4） |
| N-0801 | 焦点创建被压力限流 | 宿主 `set_pressure` 生效 | 非错误；用于定位迟滞 |
| W-0070 | 枚举 `_` 掩盖新变体 | 下游可能漏处理 | 列全变体 |
| W-0071 | 匹配臂部分不可达 | 与前臂重叠 | 合并或删除 |
| W-0072 | 模式绑定遮蔽已知名 | `case x` 想比常量？ | 用 `Const::X` 或 `x @ _` |
| W-0801 | 长循环无抢占点 | 纯计算循环超配额 | 插入 `std.chain.yield_now()` |
| E-0180 | 格式串实参不匹配 | `fmt("{}",)` 数量/具名不符 | 对齐占位符与实参（§10.11） |
| E-0181 | 格式串非常量 | `fmt(var_str, ..)` | 改字面量，或显式 `fmt_dynamic`（放弃编译期校验） |

---

## 10. 标准库 API（v2.0）

> `std` 是编译器内置虚拟 module（§5.3 步骤 2），以 `.infib` **wheel 版**随工具链分发，源码版随 SDK 一同安装（保证可审计与断点映射）。

### 10.1 std.chain
```
std.chain.id() -> u32                     当前 env.chain_id
std.chain.node_idx() -> u32               当前 env.node_idx（编译期常量表查询）
std.chain.lock(resource: &str) -> Guard   获取 FIFO 锁；Guard drop 即释放（RAII）
std.chain.yield_now()                     让出当前调度片（公平性）
std.chain.focus_count(target: &str) -> u32 该链当前并发焦点数
-- 以下为关键字，不在库中：
goto / start / stop / pause / resume / start_flag / label
```

### 10.2 std.math
`abs / min / max / clamp / roundN / sqrt / pow / sin / cos / tan / log / ln`。全部为 `const fn`：实参为字面量时编译期求值。

### 10.3 std.fs
`read_to_str(path) -> Result<String, IoErr>`、`write_file(path, content: &[u8])`、`exists(path)`、`mkdir_p(path)`、`open(path, mode) -> File`（`File` 实现 `Drop`，句柄由 §4.7.4 保证释放）。
**变更点**：`read_to_str` 返回**所有权类型** `String` 而非 `Result<&str, _>`——v1.0 该签名在有借用检查后不可能成立（返回局部缓冲区的引用）。

### 10.4 std.net
`http_get(url) -> Result<Response, NetErr>`、`http_post(url, body: &[u8])`、`websocket_connect(url) -> Ws`。TLS 后端由产物 `.inf rt` 段内置（不依赖系统解释器）。

### 10.5 std.process
`exec(cmd: &str, args: &[&str], timeout_ms: u32) -> Result<ExecResult, IoErr>`，`ExecResult { stdout: Vec<u8>, stderr: Vec<u8>, code: i32 }`。
`@sh` / `@ps1` / `@bat` / `@cmd` 在 v2.0 即为该 API 的语法糖（§7.7.5），不进入字节码摄取路径。

### 10.6 std.sync
```
Mutex<T>::new(v) -> Arc<Mutex<T>>      guard 型锁：MutexGuard<T> 实现 Deref/DerefMut + Drop
RwLock<T>                              多读单写；写者优先，防饥饿
Channel<T>::new(cap: u32) -> (Sender<T>, Receiver<T>)   多链/多焦点消息队列
Arc<T> / Rc<T> / RefCell<T> / Cell<T>  计数与内部可变性
```
**变更点**：v1.0 的 `Mutex<T>::lock()`/`unlock()` 手工配对被移除——`unlock()` 可被漏调，违反 RAII 与 §4.7.4；一律返回 `MutexGuard`。

### 10.7 std.embed（B 轨边界）
`Value`（= `dyn` 的具名形式）、`Value::as_i64() -> Result<i64, TagErr>`、`Value::to_json()`、`EmbedFrame::locals()`（仅调试构建可用）。`#[assume_typed]` 的运行时比对逻辑在此模块。

### 10.8 std.host（宿主查询与领域扩展的唯一入口）
按 §1.3，任何应用领域的数据都不得成为语言层构造；宿主能力经此模块访问：
```
std.host.query(key: &str) -> Option<i64>       宿主自定义数值指标（键由宿主文档定义，语言层不枚举）
std.host.query_str(key: &str) -> Option<String> 同上，字符串值
std.host.set(key: &str, value: &str) -> Result<(), HostErr>
std.host.event(name: &str, args: &ArgFrame) -> Result<Value, HostErr>   // 动态派发逃生舱，优先用静态 event 声明
std.host.is_available(key: &str) -> bool        用于构建期能力探测（配合 §2.2 build 约束）
```
- **静态优先原则**：已知签名的宿主事件应在 INFY 侧声明为无体 `event f(...) -> T;`（编译期参数检查，`E-0900`）；`std.host.event` 只用于宿主事件动态增删的场景，出现在 `pub` API → `W-1012`。
- 键空间由宿主自行命名（例 `"queue/depth"`、`"pool/idle"`、`"ci/attempt"`），语言层**不**注册任何具体键名，也不校验其存在性——返回 `None` 即为"宿主未提供"。
- 领域库（编排某个具体业务的那一层）应作为独立 `.infib` 发布，经 §5.3 正常导入；`std` 与语言层永不依赖它。


### 10.9 std.num
```
usize/u32/... 的显式转换一律 `as`（§4.4）；本模块只放**非平凡**运算：
checked_add/sub/mul/div/rem(a, b) -> Option<T>     溢出或除 0 → None（不 panic）
wrapping_add/sub/mul/shl/shr(a, b) -> T             唯一允许回绕的入口（§4.4）
saturating_add/sub/mul(a, b) -> T                   饱和
overflowing_add(a, b) -> (T, bool)                  值 + 是否溢出
powi(base, e: u32) -> T    /    powf(base, e: f64) -> f64
gcd/lcm(a, b) -> T                                   仅整型；两者皆 0 → R-0002
ilog2/ilog10(a) -> u32                               a == 0 → R-0009
from_str_radix(s: &str, radix: u32) -> Result<T, ParseIntError>   radix ∈ [2,36]，否则 E-0131（const）/R-0012（运行时）
```
- `ParseIntError { kind: Overflow|Empty|InvalidChar{idx: u32}|BadRadix }`——**必须**带出错字节下标，供诊断定位（这是错误信息规范的一部分，不是可选字段）。

### 10.10 std.iter
```
trait Iterator { type Item; fn next(&mut self) -> Option<Self::Item> }
map/filter/take/skip/step_by/chain/zip/enumerate/flatten/inspect/scan/chunks_windows
collect<C: FromIter>(...) -> C        必须显式目标集合；`collect()` 无目标 → E-0010
fold(init, acc) / reduce(f) -> Option<T> / try_fold(...) -> Result<R, E>
count/last/nth/min/max/min_by/max_by/clamp 之和：sum/product 需 Self: Sum
ExactSizeIterator / DoubleEndedIterator / TrustedLen(unsafe) 三个附加 trait
```
- **无限迭代器规则**：`sum`/`collect`/`count` 对非 `ExactSizeIterator` 的无限源（`iter::repeat`、`(a..)`）在 `-O0` 下必须触发 `R-0803`（迭代次数超过 `focus_quota` 派生上限）而不是永久挂起。上限取 `[profile] iter_guard_steps`（默认 2^32）。这是"取消迟滞"（§8.5）在迭代器层的兜底。
- 消费式：`next` 接收 `&mut self`；迭代器不可 `Copy`，重复使用同一迭代器变量 → `E-1001`（已被移动/消耗）。

### 10.11 std.fmt
```
fmt(fmt_str: &str, args: &[Args]) -> String      位置索引 "{}" / "{0}" / "{name}"
write_fmt(w: &mut Writer, ...) -> Result<(), IoErr>
trait Display / Debug / LowerHex / UpperHex / Binary / Octal / Pointer
格式选项：{:>10} {:<10} {:^10} {:0>10} {:.3} {:e} {:?} {:#?}
```
- `{name}` 只接受**具名实参**（`fmt("{w}x{h}", w=8, h=2)`），无隐式变量捕获——源码里存在同名变量不构成理由（P5）。位置索引越界 / 实参未用尽 → `E-0180`（编译期解析格式串字面量！非字面量格式串 → `E-0181`，必须用 `fmt_dynamic` 且放弃编译期校验）。
- `{:#?}` 的 `Debug` 多行缩进 4 空格、逐字段一行，规则与 §6.7 `Debug` 一致。

### 10.12 std.time / std.rand
```
std.time.now() -> UnixTs        /  monotonic() -> u64 (纳秒)  /  sleep(d: Duration)
Duration::from_millis(u64) ...   比较与算术按 u64 纳秒，溢出走 §4.4 策略
std.rand.Rng（显式对象，非线程局部全局）: next_u64 / range(lo,hi) / shuffle(&mut [T]) / choice(&[T])
```
- **没有全局随机源**：`rand()` 不接受无参全局调用，因为可复现性（§7.3、A8）要求每次随机都必须能追溯到显式种子。`Rng::seeded(u64)` 或 `Rng::from_host()`；算法固定 **SplitMix64 + PCG32**（v2.0），换算法 = MAJOR。
- `shuffle` 必须实现 **Fiore-Keane/Latin 无偏 Fisher–Yates**（从尾到头、`range(0..=i)`），实现不得改方向：两侧同种子同序列是 §12.4 等价性矩阵的测试项。

### 10.13 std.coll
`Vec<T>` / `VecMap<K,V>`（保留插入序）/ `Map<K,V>`（按 `Ord`，序稳定）/ `Set<T>` / `Deque<T>` / `SmallVec<T, N>`。
- **序语义**：`Map` 迭代按 key 的 `Ord` 顺序（不是哈希序），因为哈希序不可复现且会污染输出；`Set` 同理。需要哈希性能用 `HashMap`，但其**迭代顺序在规范中声明为 unspecified**——`W-0185` 警告"依赖 HashMap 迭代顺序的遍历将不可复现"（strict 阻断，与 A8 一致）。
- 容量与增长因子固定：`Vec` 从 4 起，×2 至 1 KiB 容量后 ×1.5；超过 1 MiB 容量后 ×1.25。因子写入 `IFABI`，改动 = ABI 变更（因它可被 `capacity()` 观察到）。

### 10.14 std.rt
```
std.rt.panic_hook(#[panicable] cb)      仅 panic_radius != process 时有意义
std.rt.focus_id() -> u32  /  chain_id() -> u32  /  node_idx() -> u32
std.rt.mem_stats() -> MemStats          arena 用量，供宿主做背压决策
std.rt.build_id() -> &str               等价 env.build_id
std.rt.abort()                          立即 process 半径终止
```


#### 7.7.5 各前缀状态
| 前缀 | v2.0 状态 | 依据 |
| --- | --- | --- |
| `@lua` | **完整可用（建议先做）** | `luac` 字节码格式最小最稳，能力表近乎全覆盖；适合打通管线与合规基准 |
| `@py` | **可用**，能力表覆盖 §7.7.2 扫描到的构造 | 构建期需 CPython；纯 Python 生态经 `infy ingest py-wheel` 可达 |
| `@js` / `@ts` | **语法合法，但强制 `isolation="process"`**；不进入字节码摄取路径 | 见下方决议 R-JS |
| `@java` | **v2.1 候选，v2.0 不支持**（`E-0702`） | `.class` 是规范化且跨版本稳定的字节码，lowering 成本低；代价是运行时必须携带一套 JVM 对象模型，属能力表扩张 |
| `@sh` `@ps1` `@bat` `@cmd` | **不转译**，强制 `isolation="process"` | 无字节码形态，语义与宿主环境深度耦合；`@sh` 保留为 `std.process.exec` 的参数化糖 |
| `@ruby` `@php` | **不支持**（`E-0702`） | YARV / PHP opcode 均未对外承诺稳定，收益不抵成本 |

> **R-JS：`@js`/`@ts` 的 v2.0 决议。**
> JS/TS 生态没有任何**对外承诺稳定**的字节码形态——`tsc` 与 `esbuild` 只产 JS 文本，V8 的 bytecode、QuickJS 的 `.bc` 都随版本任意变动且无规范。于是若要让 `@js` 走 B 轨，INFY 必须自己写一个 JS/TS parser 加语义分析，这会直接击穿 §7.7.1 的立身前提（**不实现任何外部语言的解析器**）；而 JS 的对象模型（原型链、动态 `this`、getter/setter、`Number` 一律双精度、`==` 的隐式转换矩阵）比 Python 的更庞大。**判定：v2.0 不值得。**
> 具体形态：前缀合法、语法保留，但**必须**显式写成 `@js(isolation = "process")`，由宿主 Node / Deno / Bun 执行；省略该参数 → `E-0716`。它享受与 A 轨 `isolation="process"` 完全相同的规则（§7.6.6）：进程边界、序列化参数、`#[owned]` 签名、构建期签名核对。副作用是这一条**不在** P6"运行期零依赖"的承诺范围内——宿主必须有 Node 系运行时，或经 `infy toolchain add deno` 拉取。
> 撤销条件（满足任一即升入 v2.1 正式 B 轨）：① 出现被 ECMA 或主流实现共同承诺稳定的 JS 字节码格式；② `@java` 落地后证明"复用宿主级对象模型"的成本低于预估，可借 JVM 上的 Rhino/GraalPy 一类方案间接支持。在此之前，规范**不**把 `@js`/`@ts` 写进任何"编译期产 IFBC"的承诺。

#### 7.7.6 工程量真正的落点（诚实记录）
省掉的是**前端**——词法、语法、语义分析、类型推断，以及行号表与异常表这类调试信息的生成。这部分确实如判断：调用源语言自己的工具链就一次性做完了。
省不下的是**运行时对象模型**：`.pyc` 只是指令流，而指令操作的是 `type`/`dict` 布局、`int` 的任意精度语义、字符串 interning、迭代器协议、异常与 traceback 构造、引用计数与循环引用回收。Jython 与 IronPython 多年工作量的主体正在这层——它们同样"只"做了 lowering。
所以结论修正为：**B 轨的成本从"每种语言写一个编译器前端"移到了"一套可分批扩张的对象模型"**。这个位移是有价值的，因为后者可以按能力表逐 MINOR 承诺、且未覆盖处在构建期就红；而前者一旦开工就是整块不可分割的投入。详见附录 C 未决项 C-2。
#### 7.7.7 `dyn` 与动态边界
- `dyn` **只能**出现在 B 轨 `@embed` 的形参/返回/局部，以及 `std.embed.Value`；出现在 `pub` API 签名 → `W-1011`（污染下游静态性）。
- 装载期校验器要求每条 `dyn` 操作都有标签检查点（UB-020）；不符 → `R-0710`。
- `#[assume_typed(T)]` 允许声明"已知类型"，运行时**仍**保留标签比对，仅省去做装箱转换的路径。

---


## 11. 宿主 ↔ INFY 运行时交互协议

v2.0 的运行时**编进产物**（`.inf rt`，§7.4），宿主与 INFY 不再隔进程通信：

- 宿主 = 以 Rust 库形式链接 INFY 运行时的可执行程序（`infinity_runtime`）。通信是**进程内 trait 直调 + 静态分发**，无 JSON-RPC、无 stdio。
  ```rust
  pub trait HostApi {
      fn env_snapshot(&self) -> &EnvSnapshot;                 // §4.6：编译期绑定到固定偏移的读
      fn call_native_event(&mut self, id: EventId, args: &ArgFrame) -> Result<OutVal, HostErr>;
      fn acquire_resource(&mut self, res: ResourceId, ts: u64) -> GuardId;   // §8.3 FIFO
      fn release_resource(&mut self, guard: GuardId);
      fn spawn_focus(&mut self, chain: ChainId, start_node: NodeIdx) -> FocusId;
      fn request_pause(&mut self, focus: FocusId);
      fn request_stop(&mut self);
  }
  ```
- `EventId` / `ChainId` / `NodeId` 均为**编译期分配的 u32 常量**，写在 `.inf meta` 的镜像表里。字符串名查找只在 `infy` 工具侧发生，运行时路径零字符串比较。
- `ArgFrame` 是类型化的紧凑参数帧（由签名生成布局），取代 v1.0 的 `&[Value]` 泛型装箱；这使 §7.6.4 的边界检查能在编译期确定偏移。
- 宿主返回错误必须携带可映射的 INFY 诊断码（`E-0900` 系），否则运行时按 `R-0900` 归类并在诊断中打印 `unmapped host error`。
- `isolation="process"` 的 A 轨单元与 `@ts/@js` 糖（§7.6.6/§7.7.5 R-JS）仍走子进程，但那属于**运行时的普通进程调用**，不再是 INFY 的执行模型本身。

### 11.1 `HostApi` 的绑定与生命周期
- 宿主必须在**任何链启动之前**完成 `HostApi` 注册：`infy_runtime::Builder::host(impl).entry(ChainId).build()?` → 得到 `Runtime`。注册之后 trait 对象不可替换（避免运行中事件突然消失导致 `E-0900` 语义不可复现）。
- `Runtime` 不是 `Send`，但其上 `spawn_focus` 是线程安全的；宿主回调 `call_native_event` **可能来自多个 OS 线程**，因此 `&mut self` 由运行时用一个 `Mutex<Host>` 串行化——即宿主实现**不需要**自己加锁，但也**不得**假设同焦点串行以外的顺序。
- `acquire_resource`/`release_resource` 在串行锁**之外**调用（否则 FIFO 会与宿主锁死锁），实现必须 `Send + Sync`。
- 回调进入 INFY 侧（`#[export]`，§7.6.5）与宿主回调 INFY 事件不得嵌套超过 8 层 → `R-0902`（防互递归爆栈）。

### 11.2 ID 分配表与镜像格式
`.inf meta` 的镜像表（`infy artifact dump --mirror` 可导出）：
| 表 | 键 | 值 | 分配时机 |
| --- | --- | --- | --- |
| `EventTable` | `EventId: u32` | 名（`module/package::event`）、签名摘要、是否宿主实现 | 编译期，按依赖拓扑逆序 + 名称字节序 |
| `ChainTable` | `ChainId: u32` | 名、节点表偏移、入口节点 | 同上 |
| `NodeTable` | `(ChainId, u32)` | 源位置 `file:line:col`、`linkage_count`、宿主事件引用 | §8.4 算法 |
| `ResourceKeys` | — | 字符串字面量资源名的 intern 索引 | 编译期 intern |
| `EnvOffsets` | `env.*` 名 | `EnvSnapshot` 字段偏移与类型标签 | `IFABI` 固定 |
- **ID 不跨构建稳定也不要求稳定**，但**同一 build-id 内**必须一致；日志回溯以 `NodeTable` 的源位置为准，绝不以数字 ID 为准（§8.4 第 6 条要求 `infy artifact diff` 能翻译两侧）。
- 宿主按 `EventId` 查表分发；`R-0900` 出现在宿主不认识该 ID 时（版本错配的典型症状）。

### 11.3 版本握手与不匹配处理
启动时执行三步握手，任一步失败即拒绝运行（不是降级运行）：
1. 产物 `IFABI` == 宿主编译时使用的 `IFABI` → 否则 `R-0903`。
2. `EventTable` 中所有 `host=true` 的事件在宿主注册表中存在且签名摘要一致 → 缺失 → `R-0904`（列出缺失事件与签名差异）。
3. `EnvSnapshot` 布局（字段数 + 偏移表）与宿主一致 → 否则 `R-0905`。
- 握手在设计上是**编译期**就该由链接保证的；这三条运行时期检只在 `.infib` 被动态装载（`static = false`）时才可能触发，属于兜底。
- `infy check --host-signature <f.json>` 允许在构建期用宿主导出的签名文件做同样校验，把 `R-0903/0904/0905` 提前到编译期（P3 的延伸）。

### 11.4 背压与资源上报
- 宿主可通过 `Runtime::set_pressure(f32)` 声明系统压力；运行时据此在 §8.5 的配额上**降低新焦点创建速率**（不改变已有焦点顺序，避免破坏 FIFO 公平性）。
- `std.rt.mem_stats()` 与 `env.ram_mb` 的差异：前者是本进程 arena 明细，后者是宿主采样的整机量。规范禁止用后者做分配决策（噪声大）。
- 焦点创建被压力限流时，`start(chain=X)` **不失败**，只是排队；排队时长计入诊断 `N-0801`（便于定位"为什么流水线慢"）。


---

## 12. 合规性测试套件（Conformance Suite）

`infy conformance` 运行规范附带的 **2300 个测试用例**（§12.1 的 1244 + §12.2 的 256 + §12.5 的 800）。编译型语言的合规不只是"行为对"，还包括"**代码对**"和"**构建可复现**"，因此新增 §12.3 两类硬校验。用例总数随 MINOR 递增，但**已有用例的编号与期望不得改变**。

### 12.1 前端与语义
| 分类 | 数量 | 检查 |
| --- | --- | --- |
| §2 词法（含 build 约束） | 96 | 编码、标识符、生命周期记法、字面量、注释、约束表达式 |
| §3 文法 | 288 | 每条 EBNF 规则正反例，含 §3.10 消歧 |
| §4.1–§4.4 类型与运算 | 120 | 隐式转换禁令、溢出策略、`as` 位级语义 |
| §4.7 所有权/借用/生命周期 | 200 | **E-1001..E-1005 每类正反 20 例**；elision 三规则；drop 顺序 |
| §4.9 UB 清单 | 120 | 24 条禁令逐条必须被识别为编译错误或规范 panic |
| §5 模块/包/`.infib`/校验 | 144 | 目录=包、internal、MVS、循环依赖、dirhash、wheel/源码版等价性 |
| §6 宏与属性 | 96 | 卫生性、深度、片段分类、属性封闭表 |
| §8 链与控制流 | 128 | `goto` 编译期解析、节点静态编号、FIFO、多焦点 |
| §1.3 领域中立 | 52 | 静态扫描 `std.*` 与语言层导出符号表，命中领域词表黑名单（`domain_terms.txt`）即判不合规；同时校验 `env` 字段集合封闭 |

### 12.2 嵌入
| 分类 | 数量 | 检查 |
| --- | --- | --- |
| §7.6 A 轨 | 96 | 绑定生成、符号命名、所有权映射、缺失工具链必须编译期失败、`isolation="process"` 语义等价 |
| §7.7 B 轨 | 96 | 能力扫描正反例（每类 `E-0712`）、`E-0711` 动态代码生成拒绝、`.pyc` magic 校验（`E-0713`）、行映射回原语言、`dyn` 边界、`infy ingest py-wheel` 端到端 |
| §7.8 IFBC | 64 | 校验器六项各自的正反例（必须拒绝畸形字节码，不得"部分执行"） |

### 12.3 产物与构建（编译型专属，硬性）
- **黄金文件（codegen golden）**：每个用例在**基准实现**上产出 `.inf meta` + 反汇编 + `infy ifbc dump`，其他实现必须逐字节或逐指令匹配**可观察语义**（允许寄存器分配差异，禁止控制流与边界检查点差异）。
- **可复现构建**：同一输入两次独立 `infy build --reproducible`，产物必须字节级相同；不一致即 §7.3 违规（UB-022）。
- **链接确定性**：依赖拓扑顺序固定；`--verify-reproducible` 必须通过（`E-0603`）。
- **ABI 一致性**：同一 `IFABI` 下，不同 opt 级别产物对同一 `.infib` 的链接必须成功且行为一致。

### 12.4 等价性矩阵
以下组合必须产生**相同可观察行为**（差异 = 不合规）：
`wheel 版 .infib` ≡ `源码版 .infib`；`-O0` ≡ `-O3`（除性能）；`lto=false` ≡ `lto=fat`；A 轨同进程 ≡ A 轨 `isolation="process"`；`debug` ≡ `release`（边界检查不可被关闭）。

**合规率必须 100% 才能声称「INFY v2.0 兼容」。** 允许 `X-` 扩展码存在，但不得改变任何规范码。

- **构建期**：`infy check` 与 `infy build` 必须输出**同一文件同一行同一码同一文本**；`infy conformance --update-golden` 需要显式授权（写 `--i-know-this-is-breaking`），否则拒绝，因为改动黄金文件等于改语义。
- **运行期**：诊断必须包含焦点 ID / 链 ID / 节点源位置三元组，缺一即不合规（`X-` 扩展码不得覆盖此要求）。

### 12.5 合规性测试的分类扩展（新增章节对应项）
| 分类 | 数量 | 检查 |
| --- | --- | --- |
| §2.8–§2.12 字面量/字符串/const-eval | 96 | 分隔符、进制、字面量溢出、多行 raw 缩进剥离、转义封闭表、confusables |
| §3.11–§3.13 表达式/模式/语句 | 128 | 自动引用阶梯顺序、结构体更新移动、穷尽性与可达性、`defer` 顺序 |
| §4.10–§4.16 求值顺序与推断 | 144 | 每条求值顺序规则一个可观察副作用用例、`?` 展开式、`[lints]` 不可降级 `E-` |
| §6.7–§6.8 derive | 72 | 每个 derive 的生成语义（含 `Ord` 字典序、`Hash` 字段判别式前缀）+ `infy expand` 字节稳定 |
| §8.4–§8.5 节点表与焦点 | 96 | 节点编号跨实现/跨优化一致、取消传播矩阵、抢占点封闭性 |
| §9.5 `infy fmt` | 120 | 幂等、AST 不变、注释保留、`@embed` 不透明、三档行宽各自黄金 |
| §10.9–§10.14 std 补充 | 84 | 无偏 shuffle 序列、`Map` 有序迭代、`Rng` 种子可复现、`ParseIntError.idx` |
| §11.1–§11.4 宿主协议 | 60 | 三步握手失败码、串行化锁与 FIFO 不互锁、镜像表 ID 一致性 |

**合计：§12.1 的 1244 + §12.2 的 256 + 本节 800 = 2300 个用例**（本表不与 §12.1/§12.2 重叠：那两类只测规范既有从句，本节测新增章节）。任何实现必须在 `infy conformance --report` 中输出**逐分类通过率**，只报总数视为不合规。

---

---

## 13. 产物格式与 CLI 规范

### 13.1 `.infy` 源码
- UTF-8，LF，trailing LF。
- 首行 `#!/usr/bin/env -S infy run` 允许（shebang 由编译器忽略，且**仅**在 `infy run` 的开发态快捷路径生效；发布产物不依赖 shebang）。

### 13.2 `.infib`（库文件容器）
- 容器：ZIP（store）+ 整体 zstd 层；`INFIB-META/manifest.toml` 必选。
- 结构：
  ```
  INFIB-META/manifest.toml      [lib] edition / path / version / ifabi / targets / deps / dirhash
  INFIB-META/abi.json           全部 pub 项的规范化类型签名（链接期比对用）
  INFIB-META/sum                dirhash + 校验数据库记录（§5.8）
  src/**.infy                   仅源码版
  ifir/**.ifir                  仅 wheel 版：可移植中间表示
  code/<target-triple>/**.o     仅 wheel 版：各目标预编译码
  ifbc/**.ifbc                  B 轨摄取产物（若该库含 @embed）
  doc/**                        文档注释渲染源（两版都带，体积小）
  ```
- `infy lib pack --edition source|wheel [--target <triple>]`、`infy lib verify <f.infib>`、`infy lib dump <f.infib>`、`infy lib diff <a> <b>`（比对 ABI 是否破坏兼容）。

### 13.3 `.infyny` / `.infypkg`
- `.infyny` = 多 target 产物 + 资源的**发布容器**（ZIP + zstd）：`META.infy.toml` + `artifacts/<triple>/*` + `assets/*` + `sum`。用户仍需用 `infy install` 解出对应平台的原生可执行文件——**它是打包格式，不是产物本身**（对齐 P6）。
- `.infypkg` = module 源码发布包（等价 `go mod` 的可下载形态），供 `import` 解析使用。
- 命令：`infy pack`、`infy unpack`、`infy install <artifact|infyny>`。

### 13.4 `infy` CLI 全集（规范定义）
```
infy -V / --version              版本（INFY-v2.0.x，同时打印 IFABI 与 ifbc 版本）
--- 构建 ---
infy init <module-path>          生成 module 骨架 + MODULE.infy.toml
infy build [mod] [--target T] [-O0..-O3|-Oz] [--profile debug|release|release-small]
                   [--reproducible] [--out f]     → 单文件自包含产物（§7.4）
infy check [mod] [--strict] [--deadlock] [--diag-json f]   前端全检查，不产出码、不需工具链
infy run [mod|src.infy] [--arg K=V] [--feature f]  开发态：build 到临时目录 + 立即执行
infy clean [--cache]             清构建输出 / 追加清 §5.10 缓存
infy fmt [-w] [--check]          官方格式化（字节级唯一，UB-014）
--- 依赖与库 ---
infy mod tidy|why|verify|upgrade|download|vendor|accept-changes
infy lib build|pack|verify|dump|diff          .infib 产出与审计（§13.2）
infy ingest py-wheel <whl> --as <path> [--py 3.13]   纯 Python wheel → 字节码摄取 → 产 .infib（§7.7.4）
infy ingest probe <whl|src>                  只跑能力扫描并列缺口，不产出（CI 预警用）
infy sumdb <addr> | --off                     校验数据库配置（§5.8 第 3 项）
--- 工具链 ---
infy toolchain probe|add|list|remove          A 轨编译器 + B 轨字节码工具链注册（§7.6.2/§7.7.3）
infy target list|add <triple>
--- 检视与文档 ---
infy ifbc dump|verify <artifact>              字节码段检视（§12.3）
infy artifact info|sections <artifact>        .inf meta 与段布局
infy doc [mod] [-o out/] [--private]          生成 HTML
infy lsp                                     Language Server（可选）
--- 质量 ---
infy test [pkg] [--bench] [--cov]
infy conformance [tests/] [--category X] [--update-golden]
```

**退出码**：`0` 成功；`1` 一般性失败；`2` 诊断阻断（E，或 strict 下 W）；`3` 运行时 panic；`4` 权限/缓存写入错误；`5` 网络或依赖获取失败；`6` 工具链或链接失败；`7` 校验和/可复现性不一致（§5.8、§7.3）。

**环境变量**：`INFY_CACHE`、`INFY_PRIVATE`、`INFY_NOSUMDB`、`INFY_TOOLCHAIN`、`INFY_TARGET_DIR`、`INFY_DIAG_COLOR=auto|never`。

---

## 14. 语言导览（完整可编译示例）

> 本章不是教程性闲谈，而是**规范的对照实现样本**：下面每个片段都必须能被 `infy check` 接受（除明确标注为反例的），并对应到具体条款。合规套件（§12）把本章逐段作为用例来源。

### 14.1 module 布局

```
acme-ci/
  MODULE.infy.toml
  MODULE.infy.lock
  cmd/
    runner/                    package runner — 入口
      main.infy
  core/
    build/                     package build
      mod.infy  cache.infy     同目录 = 同一命名空间
    test/                      package test
      report.infy
      internal/                package internal — 仅 core/test 子树可见
        parser.infy
    assets/
      decoder.c                A 轨源码，由 #[link] 绑定
      pyhelpers/normalize.py   B 轨源码，由 compileall 摄取
  tests/
    build_test.infy            package build_test（§5.9）
```

`MODULE.infy.toml`：

```toml
[module]
path    = "github.com/acme/ci"
version = "0.4.1"
infy    = "2.0"

[dependencies]
"std" = "1.0"
"github.com/acme/logx" = "1.2"

[b_track]
py = "3.13"

[profile]
overflow = "panic"
opt_level = 2
a_track = "unsafe-trusted"
reproducible = true

[[bin]]
name = "runner"
path = "cmd/runner"

[[lib]]
path = "core/build"
edition = ["source", "wheel"]
targets = ["x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"]
```

### 14.2 类型、所有权与 `?`（`core/build/mod.infy`）

```infy
#![infy_version = "2.0"]

package build

import "github.com/acme/ci/core/test/report" as report;
import "std/fs" as fs;
import "std/fmt" as fmt;

#[derive(Debug, Clone, Copy)]
pub enum Phase { Fetch, Compile, Link, Publish }

#[derive(Debug)]
pub struct Job {
    pub id: u32,
    pub phase: Phase,
    pub src: String,
    pub out: Option<Box<Artifact>>,        // 无 null；未产出即 None
}

#[derive(Debug)]
pub struct Fail { pub job: u32, pub why: FailWhy }

pub trait Run {
    fn run(&mut self) -> Result<u32, Fail>;
    fn name(&self) -> &str;
}

impl Run for Job {
    fn run(&mut self) -> Result<u32, Fail> {
        let bytes = self.pull()?;                     // §4.11：From<Fail> for Fail 已具备
        self.phase = Phase.Compile;
        let n = self.compile(&bytes)?;                 // &bytes 借用止于本语句
        Ok(n)
    }
    fn name(&self) -> &str { "build" }
}

fn pull_from(j: &mut Job) -> Result<Vec<u8>, Fail> {
    let p = j.out.as_mut().ok_or(Fail { job: j.id, why: FailWhy::Missing })?;
    // infy-safety: 无 unsafe；此处仅借用 p 的一个字段，作用域内有效
    Ok(p.slab.take(&mut j.src))                       // 显式移动，之后 j.src 失效
}

fn describe(j: &Job) -> String {
    match (j.phase) {                                  // §3.12：臂以逗号分隔
        Phase::Fetch   => fmt!("fetch #{}", j.id),
        Phase::Compile => fmt!("compile #{}", j.id),
        Phase::Link | Phase::Publish => fmt!("late #{}", j.id),  // 或模式，绑定集相同
    }
}
```

反例（必须报错，且诊断须精确到列）：

```infy
fn bad(j: Job) -> String {
    let a = describe(&j);
    let b = describe(&j);
    drop(j);
    describe(&j)                                       // E-1001：使用已移出的绑定
}
fn bad2(x: u32, y: f64) -> u32 { x + y }               // E-0111：隐式浮点/整数互转
fn bad3(v: Vec<u8>) -> &u8 { &v[0] }                   // E-1004：返回局部引用
```

### 14.3 链、节点与连携（`cmd/runner/main.infy`）

```infy
#![entry_chain = "Pipeline"]

package runner

import "github.com/acme/ci/core/build" as build;
import "std/chain" as chain;
import "std/sync" as sync;

chain Pipeline {
    label begin:
    spawn_jobs();                                       // 节点 0（宿主语句）
    harvest();                                          // 节点 1
    report::emit();                                      // 节点 2

    gate();                                              // 节点 3
    {                                                    // §8.1：连携块挂在宿主语句之后
        if (build::all_green()) { goto("done"); }        // 普通语句，编译期解析锚点
        goto(+1);
    }
    retry_once();                                        // 节点 4
    { goto("begin"); }                                   // 节点 5 的连携
    label done:
    finalize();                                          // 节点 6
}
```

- 节点编号严格按 §8.4：`let`/`var`/纯赋值/`label` 不产生节点；`if` 在连携块内仍是普通语句（§8.1）。
- `goto(+1)` 相对偏移按 §3.10 消歧优先识别为偏移，而非一元加。
- `gate()` 内的锁用 RAII 释放（§8.3）：

```infy
fn gate() -> Result<(), build::Fail> {
    let _g = chain::lock("runner:exclusive");            // Guard drop 即释放
    Ok(())
}
```

### 14.4 A 轨：自动连接原生解码器（同文件续）

```infy
#[link(name = "decoder", static = true, flags = ["-O2"], toolchain = "cc")]
extern "C" {
    #[borrows('static)]
    fn dec_open(path: *const u8, len: u64, err: *mut i32) -> *mut DecHandle;
    fn dec_read(h: &mut DecHandle, buf: &mut [u8]) -> usize;
    fn dec_close(h: *mut DecHandle);
}

@c(name = "decoder") {
    #include <stdint.h>
    #include "infy_binding.h"          /* 编译器生成，§7.6.3 项 1 */
    DecHandle* dec_open(const uint8_t* p, uint64_t n, int32_t* err) { /* … */ }
    /* … */
}

fn open(path: &str) -> Result<Box<DecHandle>, build::Fail> {
    let mut e: i32 = 0;
    let raw = unsafe { dec_open(path.as_ptr(), path.len() as u64, &mut e) };
    // infy-safety: dec_open 当且仅当返回非空时不写 err（见 decoder.c 头部约定）
    if raw.is_null() { return Err(build::Fail { job: 0, why: build::FailWhy::Native(e) }); }
    Ok(unsafe { Box::from_raw_safe(raw) })
}
```

- `@c` 块与 `extern` 块由 `name = "decoder"` 配对；不配对 → `E-0720`。
- `unsafe` 必需（`E-0721`），且必须紧跟 `// infy-safety:`（`W-1030`）。
- `#[borrows('static)]` 缺失 → `E-0723`（UB-010，无默认值）。

### 14.5 B 轨：摄取 Python 归一化脚本（同文件续）

```infy
fn normalize(batch: &[f32], scale: f32) -> Result<Vec<f32>, build::Fail> {
    @py(timeout_ms = 5_000, batch = batch, scale = scale) {
        # 构建期：本块写入 <build>/b_track/normalize.py
        # 由清单声明的 CPython 3.13 compile() 产出 code object，再 lowering 为 IFBC
        if scale == 0.0:
            raise ZeroDivisionError("scale")
        out = [ (x / scale) for x in batch ]
        return out
    }
}
```

- 参数按 §7.7.1 的调用约定绑定（不再是"注入全局变量"）；`return` 直接映射 IFBC 返回指令。
- 若脚本出现 `eval(...)` → `E-0711`；引用未实现内置名 → `E-0712`；宿主无声明的 `py = "3.13"` → `E-0715`。
- 超时由 `tickloop` 计数在下一个抢占点触发 `R-0701`，**不**杀进程（§7.9）。

### 14.6 焦点与并行（多 job 并发跑）

```infy
fanout(n: u32) {
    let i = 0;
    while (i < n) {
        start(chain = Worker);                 // 每调用创建一个新焦点（§8.5）
        i += 1;
    }
    { resume; }
}

chain Worker {
    claim();                                   // 内部用 std.chain.lock 争抢资源
    build::run_one();
    release();
    { goto(+1); }                              // 无更多任务 → 焦点自然结束
}
```

- 一个焦点 panic 只终止该焦点（默认 `panic_radius = focus`，§4.16）。
- 抢占点封闭（§8.5）：`while` 回边即 `tickloop`，因此长循环不会阻塞调度。

### 14.7 测试（`tests/build_test.infy`）

```infy
package build_test

import "github.com/acme/ci/core/build" as build;
import "std/test" as test;

#[test]
fn describe_covers_all_phases() -> Result<(), test::Fail> {
    let j = build::Job { id: 1, phase: build::Phase::Fetch, src: String::new(), out: None };
    test::assert_eq(build::describe(&j), "fetch #1")?;
    Ok(())
}

#[bench]
fn bench_open(b: &mut test::Bench) {
    for _ in b.iter() { let _h = build::open("x.bin"); }
}
```

`infy test` = 编译（受 `//infy:build test` 约束，§2.2）+ 执行；覆盖率来自 `-O0` 构建的节点表插桩，**不**改变可观察行为（§12.4）。

### 14.8 一次完整构建（读者可自行核对的命令行序列）

```
infy mod tidy                     # 写 MODULE.infy.lock（§5.7 MVS）
infy toolchain probe              # 注册 cc / python 3.13（§7.6.2/§7.7.3）
infy check                        # [1]–[5] + B 轨能力扫描；无 C 编译器也可跑
infy build --target x86_64-unknown-linux-gnu -O2 --reproducible
infy artifact info runner          # 看 IFABI / build-id / 依赖闭包 / 节点表偏移
infy ifbc verify runner            # 校验 .inf ifbc 段（§7.8.3 六项）
./runner                           # 零外部依赖运行（P6）
```

---

## 附录 A：设计取舍的原理说明（非规范，解释性）

A1. **为什么做独立语言而不是 YAML/JSON/TOML 配置？**
配置格式能描述 DAG，但难以表达 if/else/循环/宏/模块化/跨语言嵌入。INFY 是一门真正的语言——它要能断点调试、给出人类可读的修复建议、把错误挡在编译期。

A2. **为什么 `if` 是普通语句而不是连携语句？**
链节点本身就可以是条件节点，不必"先空跑一个 NOP 再在连携里 if"。语义更直观，调试更清晰，且编译期可静态推导节点表。连携块保留给"宿主动作完成之后要跳去哪"这类**编排**关注点，与 `if` 这类**计算**关注点分离。

A3. **为什么从解释型翻转成编译型？**
v1.0 选解释型是为了绕开"单文件分发的宿主程序无法保证目标机上有编译器"。但代价是执行模型只能靠子进程 + JSON-RPC，于是每次跨语言调用都付一次进程与序列化开销，且**类型边界完全失控**（`env.return(x)` 里塞什么都合法）。改成编译型之后：边界检查前移到编译期（P3）、产物单文件自包含（P6）、用户不再需要装 Python/Lua（B 轨在构建期就吃掉了它自己的字节码）。这是本版最大的一笔交换：**用"编译期需要工具链"换"运行期零依赖 + 类型可控"**。

A4. **为什么编译型语言嵌入从"绝对禁止"改为 A 轨自动连接？**
禁止的理由（A3 旧版）是"无法保证任意平台有 Rust/C++ 编译器，且编译耗时阻塞工作流"。这两点在编译型架构下都不再成立：既然用户已经必须有 `infy build` 才能开发，那么由编译器统一驱动外部工具链（发现 → 生成绑定 → 编译 → 链接）就是自然延伸，而不是额外负担。同时链接期错误远好于运行时 `env.return` 语义黑洞。**代价被如实写进 §4.7.5 与 §7.6.6**：外部代码的 UB 不在 INFY 保证范围内，故 A 轨强制 `unsafe` + 清单声明信任策略。

A5. **为什么 B 轨要摄取原生字节码，而不是继续用子进程、也不是自己写转译器？**
取材方式也定了：**不写解析器，摄取源语言自己的字节码**。于是 INFY 与宿主语言之间的界面从「文本 + 进程」变成「指令流 + 同一进程内的调用约定」。字节码化后，跨语言边界变成**同一进程内的函数调用约定**，参数与返回可以按声明类型生成校验点（§7.8.2），超时可以用回边计数实现（不需要杀进程），行号可以映射回原语言。同时运行时依赖被彻底移除。子进程模式作为 `isolation="process"` **保留为隔离手段**（§7.6.6）而不是默认执行模型——因为带原生 C 扩展的第三方库只能靠它（或 A 轨）进来。

A6. **为什么库分隔学 Go，但导出不学 Go 的大写规则？**
Go 的分包之所以清爽，是因为"目录 = 包、导入路径 = 位置、internal 边界、MVS、无循环"这几条**结构性**规则，它们与命名风格无关，能直接消灭 v1.0 里"相对导入 + URL 导入 + `use {a, b}` 选择性导入"三套并存导致的解析歧义。而"首字母大写即导出"是**词法级**规则：它与 Unicode XID 标识符相遇会产出难以判定的边角（`Élaboré` 算不算导出？全角字母呢？），而且导出状态应该出现在 API diff 里，`pub` 更显式、更可审计。所以只借结构，不借词法。

A7. **为什么签名照搬 Go 的 `go.sum` + sumdb，而不上 PKI？**
供应链攻击的主要形态是"某个版本的内容被替换"。dirhash + 公开一致性日志能在**不需要信任建立**的前提下彻底封掉这条路（换内容 → 哈希变 → 与 lock 冲突；换镜像 → 与公开日志不一致 → `E-0562`）。PKI 需要的是一整套密钥签发、轮换、吊销与人工审查——对个人项目和一个尚未存在包生态的语言来说，运营成本远超收益。**代价**：不认证作者身份（§5.8 第 5 项）。Ed25519 附加签名段作为 v2.1 MINOR 留口，不破坏现有 lock。

A8. **为什么强制 FIFO ResourceLock？**
多节点操作同一对象必须保证结果可复现，调度不能"随机挑一个"；否则长流水线作业无法回放、无法定位是哪一次交互产出了坏结果。同时规范明确不承诺死锁自由（§8.3），把它作为可选静态分析而非语义保证，避免给运行时加锁序开销。

---

## 附录 B：v2.0 相对 v1.0 提案的变更清单

### B1. 新增
| 位置 | 新增内容 |
| --- | --- |
| §1.1 | P6 单文件自包含交付；P4 改为双轨嵌入；P3 扩展到工具链/ABI/链接 |
| §1.3 | **领域中立原则**（规范性）：语言层与 `std` 禁止出现领域专属标识符；领域能力只能经 `.infib` / `HostApi` / `@embed` 三条出口 |
| §1.2 | `IFABI` 产物 ABI 版本，独立于语言版本演进 |
| §2.2 | `//infy:build` 构建约束注释（Go build tag） |
| §2.5 | 生命周期记法 `'a` / `'static` |
| §3.7 | `extern_block` / `trait_decl` / `impl_block` / `type_alias` 文法 |
| §3.9 | 宏文法独立成节；新增 `macro_call` |
| §4.7 | **所有权、借用、生命周期、Drop/RAII、`unsafe` 封闭清单**（对标 Rust 的整块内容，v1.0 完全没有） |
| §4.8 | 无 GC、地址稳定、`repr(C)` 强制边界、每链栈大小与 `R-1040` |
| §4.9 | UB-016..UB-024（use-after-move、双重 drop、drop 顺序、字节码校验、`dyn` 标签、策略漂移、链接非确定性、工具链版本漂移、实现私自扩展） |
| §5 | module/package/file 三层、`MODULE.infy.toml`、`MODULE.infy.lock`、`internal/`、MVS、`.infib` 双 edition、dirhash + sumdb、vendor、测试包、缓存布局 |
| §6.5–§6.6 | 宏展开时机；**属性封闭表** |
| §7 | 整章重写：10 段管线、target/opt/profile/LTO、可复现构建与 `build-id`、产物段布局、A 轨全流程、**B 轨字节码摄取 + 能力扫描 + wheel 摄取**、IFBC 编码与 6 项装载校验 |
| §9.1 | `X-` 扩展码命名空间；区间分配表 |
| §10.7 | `std.embed` |
| §12.3–§12.4 | codegen 黄金文件、可复现构建、链接确定性、ABI 一致性、等价性矩阵 |
| §13.2 | `.infib` 容器结构（含 `abi.json`） |
| 附录 C | 未决项与风险清单 |
| §2.8–§2.12 | 字面量完整文法、转义封闭表、多行 raw 缩进剥离算法、Unicode confusables/脚本混用、无自动分号、const-eval 允许集 |
| §3.11–§3.13 | 表达式补全（`?`、闭包、turbofish、结构体更新、自动引用阶梯顺序）、模式完整文法与穷尽/可达判定算法、`defer`、`const_assert` |
| §4.10–§4.16 | 求值顺序全序 11 条、`?` 展开式、数字边界语义表、类型推断消解优先级、方法解析与孤儿/重叠规则、单态化上限、**panic 终止半径** |
| §6.7–§6.8 | derive 逐 trait 生成规范（`Ord` 字典序、`Hash` 字段判别式、`Debug` 稳定性）、`infy expand` 可审计输出 |
| §8.4–§8.5 | 节点表编号算法（含跨实现一致性要求）、焦点模型 / 抢占点封闭集 / 取消传播矩阵 |
| §9.5–§9.7 | `infy fmt` 14 条风格规则（含幂等自检与 AST 不变性）、`[lints]` 升降级边界、**诊断码补全表（新增 80 码，合计 210）** |
| §10.9–§10.14 | `std.num` / `std.iter`（无限迭代器兜底）/ `std.fmt` / `std.time` / `std.rand`（无全局随机源）/ `std.coll`（序语义）/ `std.rt` |
| §11.1–§11.4 | 宿主绑定与串行化、镜像表与 ID 分配、**三步版本握手**、背压与限流 |
| §14 | 语言导览：完整 module 布局、可编译示例、反例组、一次完整构建的命令序列 |
| 附录 D | IFBC 完整指令集（10 段编码分区 + 校验失败映射） |
| 附录 E | lock / manifest / 能力表 / 工具链 / diag-json 的规范性语法 |
| 附录 F | 明确不支持清单（14 项，各给理由）+ 术语表（20 条）+ 高频条款索引 |
| §7.7.1–§7.7.7 | **B 轨重写为字节码摄取**：能力扫描取代手写子集、`.pyc` 版本锁定、`infy ingest py-wheel` 纯 Python 生态通路 |

### B2. 删除
| 位置 | 删除内容 | 原因 |
| --- | --- | --- |
| §1.1 P4 | "编译型语言强制禁用" | 被 A 轨取代 |
| §4.8 UB-010 | "编译型语言嵌入 = UB，触发 `E-0701`" | 同上；UB-010 编号**复用**为"A 轨指针移交未声明"，这是唯一的编号语义变更，已在表中显式标注 |
| §5.1 | `PACKAGE.infy.toml` | 被 `MODULE.infy.toml` 取代（v1.0 未生效，无需兼容层） |
| §5.2 | 相对导入 `"./x"`、`"../x"`；URL 直接 import；`use {a, b}` 选择性导入 | Go 式单一 module path 导入；`E-0504` 拦截 |
| §7.2 | 子进程 + `stdio + JSON-RPC 2.0` 作为**默认**执行模型 | B 轨字节码摄取、A 轨链接；进程隔离降为可选属性 |
| §7.3 | CPython/Deno/Lua/Ruby/PHP 宿主预装需求表 | 产物自带运行时，用户无需安装 |
| 旧 §7.7.3 | `INFY-<lang>-Subset` **手写子集清单**（允许/拒绝构造两张表） | 被 §7.7.2 的**能力扫描**取代：输入已是编译器产物，无需人为划定语法边界 |
| 旧 §7.7.x | `@ruby`、`@php` 支持 | 其字节码格式未对外承诺稳定，收益不抵成本（`E-0702`） |
| §10.6 | `Mutex::lock()/unlock()` 手工配对 | 违反 RAII |
| 旧 §10.7 | `ai_evolve.std` 整节（领域专用事件集）、`event var(...)`、`count: u32 \| auto` | **v2.0 定位为通用语言**：领域事件一律走宿主 `HostApi` + `std.host`（新 §10.8）或独立 `.infib`，语言层不固化任何单一领域 API（§1.3）；`var` 事件与关键字冲突、`u32 \| auto` 非法类型语法 |
| §13.3 | `infy setup @<lang>` | 改为 `infy toolchain add`（安装的是**编译器**，不是解释器） |
| §7.7.3（旧） | `INFY-<lang>-Subset` 手写允许/拒绝清单 | 被 §7.7.2 能力扫描取代；`E-0711` 语义收窄至动态代码生成 |
| §7.7.5 | `@js` / `@ts` 的 B 轨字节码承诺 | 决议 R-JS：无稳定字节码形态，强制 `isolation=process`，缺省即 `E-0716`；v2.1 复议 |
| 全文 | 源码扩展名 `.ifny`、容器 `.ifnyny` | 更正为 `.infy` 与 `.infyny`（v1.0 拼写遗留；扩展名属 MAJOR，但 v1.0 未生效故无迁移成本） |
| — | "解释器 / INFY VM 执行源码"的一切措辞 | 全章统一为"编译器 / 运行时" |

### B3. 语义变更（同一条款，行为不同）
| 条款 | v1.0 | v2.0 |
| --- | --- | --- |
| 类型检查时机 | `infy check` 阶段（解释器启动前） | 编译管线阶段 [4]/[5]，产物运行时不复查 |
| `&T` 可用范围 | 只允许栈内引用，无生命周期标注 | 完整生命周期系统 + elision 三规则 |
| 跨链可变共享 | `std.sync.Mutex<T>` 手工 `unlock` | `Arc<Mutex<T>>` + `MutexGuard`（RAII），竞争仍在编译期消灭 |
| `goto` | 运行时解析，失败 `R-0010` | **编译期**解析（`E-0301`），运行时失败仅作为不可静态判定时的兜底 |
| 节点索引 | 运行时计数 | 编译期节点表常量，跨构建稳定 |
| `env.*` | 宿主实时注入快照 | 编译期绑定偏移 + 运行时更新快照结构；新增 `env.build_id` |
| 整数溢出策略 | 写在 `PACKAGE.infy.toml` | 写在 module `[profile]`，链接期要求全依赖一致（`E-0606`） |
| 库文件 | 无此概念 | `.infib`（源码版 / wheel 版），**不可直接运行**（`E-0551`） |
| 版本兼容承诺 | 1024 用例行为一致 | 1400 用例 + 代码黄金 + 字节级可复现构建 |

---

## 附录 C：未决项、风险与待决策取舍

> 这几条不是"待补充"，而是会决定 v2.0 能不能落地的结构性风险。按严重度排序。

**C-1（边界级，需决策）：带原生扩展的第三方库无法走 B 轨。**
IFBC 不可能重实现厂商的机器码算子，因此 §7.7.2 的能力扫描会把这类导入判为 `E-0712`。三条出路都已写进规范：(a) A 轨 `@c` 链接该库的 C API（推荐，保持 P6 单文件交付）；(b) `@py(isolation="process")` 逃生舱（承认宿主解释器存在，P6 对该单元失效）；(c) 宿主 `HostApi` 事件（§11）。**需要定的是主路径**：若以 (a) 为主，`infy` 必须提供 C API 绑定生成的脚手架（目前 §13.4 无对应子命令）；若以 (b) 为主，P6 的措辞要降级为"大多数场景无需安装解释器"。当前规范按 (a) 写、(b) 保留，未做取舍标注，需审签时定调。

**C-2（规模级，已重估）：省掉的是前端，省不下的是对象模型。**
原判断（要自己写 Python 解析器与语义分析、因此是最大工程量）**已被推翻**：§7.7.1 改为在构建期调用 CPython 的 `compileall` / `compile()` 取得 code object，INFY 只做 opcode 级 lowering——词法、语法、语义分析、行号表、异常表全部白拿。
**剩下的真实成本**是运行时对象模型：`type` / `dict` 布局、`int` 的任意精度语义、字符串 interning、迭代器协议、异常与 traceback 构造、引用计数与循环引用回收。这正是 Jython / IronPython 多年工作量的主体，它们同样"只"做了 lowering。
风险控制方式随之换掉：不再手写子集清单，而是 §7.7.2 的**能力表 + 构建期扫描**，未覆盖项当场 `E-0712` 红出，可按 MINOR 逐批扩容。
实现顺序不变但理由更新：`@lua`（字节码格式最小、对象模型最薄，先打通管线与合规基准）→ `@py`（对象模型按能力表分批扩）→ `@js/@ts`（唯一还要 parser 的）→ `@java`（`.class` 格式最稳，但要背一整套 JVM 对象模型，故列 v2.1）。

**C-3（安全模型级）：A 轨同进程链接削弱了"对标 Rust"的说法。**
Rust 的 `unsafe` 边界内也是没有保证的，INFY 照此办理是自洽的；但**默认允许任意 C 代码进进程**与"内存安全对标 Rust"读起来是矛盾的。目前 §4.7.5 的处理方式是：边界外不管、边界内全管、强制清单声明、并提供 `isolation="process"`。若要更硬的口径，可把 A 轨默认改为进程隔离（牺牲性能换保证），届时 §7.6.6 的默认值需同步改写。

**C-4（一致性级）：`wheel 版 ≡ 源码版` 的等价性很难验证。**
§12.4 要求两条路径可观察行为完全一致，但源码版本地编译可能用不同编译器版本（`E-0604` 只挡 major/minor）。需要一个明确的降级策略：建议"target 或编译器版本不匹配时**拒绝**使用 wheel，强制回落源码版"，即把 `E-0604` 从硬错误改为一次透明回落 + `W-0604`。待定。

**C-5（签名强度）：Go 模型不认证身份。**
§5.8 第 5 项已写明。Ed25519 附加签名段留作 v2.1 MINOR。是否现在就把 manifest 结构里的 `sig` 字段占位（即使为空），以免将来破坏 `.infib` 容器格式？建议占位——**这一条已在 §13.2 的 `INFIB-META/sum` 里预留，不改容器版本号**。

**C-6（领域中立原则的落地检查）：**
§1.3 第 5 条的判据（"删除后是否只影响单一领域"）已在 §12.1 落成一条合规检查，但它依赖一份**规范维护的 `domain_terms.txt` 黑名单**，该文件目前不存在。需要定两件事：(1) 黑名单的粒度——只拦 `std.*` 还是同时拦语言关键字与 `env` 字段（当前 §12.1 写的是"`std.*` 与语言层导出符号表"）；(2) 谁有权增删词条——建议随 MINOR 发布、需一次公开评审，否则该文件会变成事实上的语言层扩展通道。

**C-7（规范洞状态）**——本轮扩展补了两处主干，剩余洞逐条列明：
- 已补：IFBC 指令集主干（附录 D）、derive 逐 trait 生成规范（§6.7）、lock / manifest / 能力表 / 工具链 / diag-json 语法（附录 E）。
- 仍缺 ①：`line_map` 的压缩编码与 `exception_table` 的字节级布局。附录 D 给了语义与校验映射，未给字节格式，第三方工具因此无法自行产出可装载的 IFBC。
- 仍缺 ②：`dyn Trait` 的 vtable 逐字段布局。§4.1 只承诺由 `IFABI` 固定，未写出偏移表，等于把跨实现兼容留给默契。
- 仍缺 ③：`.infyny` 多 target 的 `infy install` 选择逻辑（谁决定装哪个、能否并列保留两个 target 的产物）。
- 仍缺 ④：`infy lsp` 的增量诊断与 §9.4 的逐字节一致性——局部修改后 LSP 缓存的阶段 [1]–[5] 结果如何与全量 `infy check` 同码同文。
- 仍缺 ⑤：`std.host` 键的分隔符与命名空间反转规则。§10.8 明写语言层不注册键名，但不定推荐形态，生态会 `/`、`:`、`_` 三套并存。

---

> **待审签**：本规范 v2.0（14 章 + 附录 A–F、210 个诊断码、2300 个合规用例）经审签后生效。附录 C 的七条**必须在审签前逐条给结论**（尤其 C-1、C-4、C-6），一旦生效即进入 MINOR 迭代，不再做结构性翻转。v1.0 提案文本存档于 `infinity_lang_spec.v1-proposal.bak.md`，仅作决策史参考。

---

## 附录 D：IFBC 完整指令集

### D.1 编码约定
- 指令定宽头：`opcode: u8` + 0..3 个 `operand: u32`（操作数个数由 opcode 隐含，校验器无需变长解码）。
- 大端存储（便于跨 target 字节序一致与 diff 可读）。
- `code` 区以 4 字节对齐填充；跳转目标必须落在指令边界（校验项 1）。
- 值栈模型：**单地址栈式 + 局部窗口**。每个帧有 `locals: u32` 个槽（`ldl`/`stl` 寻址）与一个操作数栈。

### D.2 常量与局部（`0x00`–`0x1F`）
| 码 | 助记 | 效果 | 校验 |
| --- | --- | --- | --- |
| 0x00 | `nop` | — | — |
| 0x01 | `ldl i` | push `locals[i]` | `i < frame.locals` |
| 0x02 | `stl i` | pop → `locals[i]` | 同上；栈不空 |
| 0x03 | `ldw i` | push 宽值（2 槽合并） | 类型注解必为 `u64/i64/f64` |
| 0x04 | `push k` | push `string_pool[k]` 内联常量 | `k` 在界内 |
| 0x05 | `push.b k` | push 1 字节常量 | — |
| 0x06 | `dup` / `dup2` / `swap` | 栈形变 | 高度检查 |
| 0x07 | `pop` | 丢弃栈顶 | 若类型为 `#[must_drop]` → `R-0730` |
| 0x08 | `ldnull` | **禁用**（无 null；须 `Option`） | 出现即 `R-0706` |
| 0x09 | `ld.frame` | push 当前帧指针 | 仅调试构建 |

### D.3 算术与比较（`0x20`–`0x3F`）
后缀 `.i32/.i64/.u8…/.f32/.f64` 全部为**独立 opcode**（无"运行时看类型"版本）：
`0x20 add` `0x21 sub` `0x22 mul` `0x23 div` `0x24 rem` `0x25 neg` `0x26 abs`
`0x28 and` `0x29 or` `0x2a xor` `0x2b not` `0x2c shl` `0x2d shr` `0x2e shr.a`（算术）
`0x30 cmplt` `0x31 cmple` `0x32 cmpgt` `0x33 cmpge` `0x34 cmpeq` `0x35 cmpne` `0x36 cmp`（三态，写 `i32` 判别值）
校验：`div/rem` 必须紧邻一条 `guard.nonzero`（除非源为编译期常量非零）；`shl/shr` 必须紧邻 `guard.shift`；违反 → `R-0706`（不能删检查，UB-003/UB-004 的字节码侧对应）。

### D.4 控制流（`0x40`–`0x5F`）
`0x40 jmp off` `0x41 br off`（弹栈顶 bool）`0x42 brf off` `0x43 switch n, [targets]`
`0x44 tickloop n`（回边计数，承载 `timeout_ms`）`0x45 leave`（出 `finally`）
`0x46 throw.inf k`（抛 INFY 规范化错误对象，见 §4.11）
`0x47 enter.try h_end, c_end`（登记异常表项）
跳转不得落入 `try` 区中间（校验项 4）；`0x40` 目标为下一指令时校验器警告 `W-0705` 但不阻断。

### D.5 调用（`0x60`–`0x7F`）
`0x60 call.stat f` `0x61 call.virt idx` `0x62 call.host idx` `0x63 call.embed idx`
`0x64 ret` `0x65 ret.multi n` `0x66 closure.new code_idx, cap_n`（装箱捕获，`Rc` 引用计数）
`0x67 tailcall.stat f`（仅在帧尺寸兼容时合法，否则装载期 `R-0709`）
`call.host` 的目标必须存在于 `import_table` 且签名字节匹配 `IFABI`（校验项 5 → `R-0708`）；跨 A 轨一律走 `call.host` 到运行时的 thunk，**不**允许直接跳外部地址（否则 §7.8.3 的线性性失去意义）。

### D.6 对象与内存（`0x80`–`0x9F`）
`0x80 new.str k` `0x81 new.list elem_ty` `0x82 new.dict k_ty,v_ty` `0x83 new.frame`（协程帧）
`0x88 idx.get ty` `0x89 idx.set ty` `0x8a slice.ty`（胖指针构造）
`0x90 ref.inc` `0x91 ref.dec`（`Rc/Arc`；`dec` 到 0 时按 §6.7 drop 规则释放）
`0x94 move.out i`（局部失效，对应 §4.7.1；后续 `ldl i` 未重新赋值 → `R-0731`）
所有分配走 runtime arena，指令流中**没有**裸地址常量（校验项 1 的强化）。

### D.7 动态值（`0xA0`–`0xBF`，仅 B 轨）
`0xa0 tag.get` `0xa1 cast.dyn T` `0xa2 is.dyn T` `0xa3 box.dyn` `0xa4 unbox.dyn`
`0xa8 invoke.dynamic idx`（B 轨内部高阶调用；目标必须在本模块 `func_table` 内 → 否则 `R-0712`）
每条 `unbox.dyn` 必须紧邻 `is.dyn` 或前置标签检查点（UB-020；违反 `R-0710`）。

### D.8 协程与迭代（`0xC0`–`0xDF`）
`0xc0 frame.enter` `0xc1 yield` `0xc2 yield.from` `0xc3 frame.exit` `0xc4 coro.resume h` `0xc5 coro.throw h`
`frame.enter/exit` 配对（校验项 4）；`yield` 出现在 `frame_kind != coroutine` 的函数 → `R-0707`。

### D.9 类型镜像内联（`0xE0`–`0xEF`）
`0xe0 typeof.idx` `0xe1 size_of idx` `0xe2 align_of idx` `0xe3 field_off idx`
`field_off` 仅对 `#[repr(C)]` 类型合法（§4.8），否则装载期 `R-0713`。

### D.10 校验顺序与失败映射
装载期按 `func_table` 顺序逐函数执行 §7.8.3 的 1→6；任一失败即整体拒绝该**模块**（不是拒绝单函数，避免"半可执行"状态），错误码经首失败项映射：线性 0x→`R-0704`、高度→`R-0705`、类型→`R-0706`、帧→`R-0707`、host 签名→`R-0708`、边界→`R-0709`。`infy ifbc verify --all-errors` 可一次列出全部违规（默认只报首个，保持与运行时一致）。

---

## 附录 E：数据文件格式（规范性语法）

### E.1 `MODULE.infy.lock`
```
# 行式语法；'#' 起为注释；键值以空白分隔；顺序由工具决定但必须稳定输出（§7.3）
req    <module-path> <version> [resolved-from <range>]
dirh   <module-path> <version> h1:<base64-sha256>
dirh-m <module-path> <version> h1:<base64>              # 仅清单（等价 go.mod 行）
sumdb  <module-path> <version> tree_head=<hex> inset=<n> proof=<base64,..>
priv   <glob-pattern>
nosum  <glob-pattern>
```
- 任一 `dirh` 与实际不符 → `E-0560`；`sumdb` 行的 `tree_head` 与数据库当前 STH 的一致性由 `infy mod verify` 检查 → `E-0562`。
- 该文件必须可被 `git diff` 友好比较：只允许**排序后**追加，禁止重排他人条目。

### E.2 `INFIB-META/manifest.toml`
```toml
[lib]
path       = "github.com/user/mathx"
version    = "1.2.3"
edition    = "wheel"                    # source | wheel
ifabi      = 3
ifbc_caps  = ["CPY-313:1", "LUA-54:1"]  # 含 B 轨段时必填
[lib.profiles]                          # 该库编译时策略（链接期一致性检查依据）
overflow   = "panic"
repr_default = "rust"
fast_math  = false
[[code]]                                # 仅 wheel 版
target  = "x86_64-pc-windows-msvc"
compiler = "infyc 2.0.1 (llvm 18.1)"
abi_hash = "sha256:…"
[[deps]]
path = "github.com/user/base" ; version = "1.0" ; dirhash = "h1:…"
```
缺 `ifabi` / `abi_hash` / `deps.dirhash` 任一 → `E-0554`。

### E.3 `IFBC-CAP-<lang>-<minor>.toml`（能力表）
```toml
[cap]
lang = "cpython" ; format = "CPY-313" ; cap_version = 1 ; table_hash = "sha256:…"
[opcode]                       # §7.7.2 扫描项 1
0x71 = "add.i64"              # 已映射
0xb8 = "-"                     # 未映射 → E-0712
[builtin]                      # 扫描项 2
len = "std.iter.len" ; print = "std.io.log" ; eval = "-"
[import]                       # 扫描项 3
allow = ["math", "json", "itertools"]     # 纯字节码且已声明
deny  = ["numpy", "opencv", "pillow"]      # 原生扩展 → E-0712 + 出路提示
[exception]                    # 扫描项 5
entry = ["push", "setup", "with"]         # co_exceptiontable 支持条目类型
```

### E.4 `TOOLCHAIN.infy.toml`
```toml
[[toolchain]]
kind = "cc" ; name = "msvc-cl" ; path = "…/cl.exe" ; version_str = "19.40" ; targets = ["x86_64-pc-windows-msvc"]
[[toolchain]]
kind = "py" ; name = "cpython" ; path = "…/python.exe" ; version_str = "3.13.1" ; bytecode = "CPY-313"
[[toolchain]]
kind = "deno" ; name = "deno" ; path = "…" ; version_str = "1.40"     # 仅 isolation="process"
[net] ; proxy = "…"; timeout_ms = 15000
```

### E.5 `--diag-json` schema（完整）
```json
{ "version": 1, "tool": {"name":"infyc","version":"2.0.1","ifabi":3},
  "diagnostics": [ {
    "code": "E-0111", "level": "error", "title": "…", "message": "…",
    "phase": "typck|borrow|link|embed|fmt|runtime",
    "spans": [ { "file":"…","line_start":12,"line_end":12,"col_start":25,"col_end":35,
                 "label":"…","primary":true,"origin":"source|macro-expansion|ifbc|embed-py" } ],
    "notes": ["…"], "help": ["…"],
    "suggestions": [ { "op":"insert|delete|replace", "span":{…}, "text":"…", "applicability":"machine|casual|unspecified" } ],
    "children": [ { "code":"E-…", "spans":[…] } ],
    "expand_chain": ["macro vec → file:12"]
  } ] }
```
`origin` 与 `expand_chain` 是编译型语言的必需字段（否则 LSP 无法在宏/派生/IFBC 行映射下正确高亮）；`phase` 使 CI 可按阶段过滤；缺字段即 §9.4 不合规。

---

## 附录 F：明确不支持清单与术语表

### F.1 语言层明确**不做**的事（封闭清单）
每一项都给出理由，避免被当成"待补充功能"。加回任何一项 = MAJOR 变更。

| 不做 | 理由 | 替代路径 |
| --- | --- | --- |
| `async` / `await` / `spawn` | 会引入第二套调度器，与链/焦点（§8.5）语义冲突，且抢占点不再封闭（§8.5） | 链 + `start` + `Channel<T>` |
| GC / 跟踪式回收 | 破坏地址稳定与可预测延迟（§4.8、UB-004） | `Box`/`Rc`/`Arc` + arena |
| 全局随机源 | 可复现性要求每次随机可追溯到显式种子（§7.3、A8） | `Rng::seeded` / `from_host` |
| 字符串插值 `"{x}"` | 隐式取变量所有权 + 隐式格式化，违反 P5 | `fmt("{x}", x=x)` 显式具名 |
| 自动分号插入 | 换行语义二义，`fmt` 无法证明唯一性（§2.11、§9.5） | 显式 `;` |
| 隐式类型转换（含数字） | UB-001/UB-002 | `as` / `roundN` |
| 运行时反射调用未知方法 | 破坏 `IFABI` 校验与链接期符号唯一性 | `HasField`（§6.7）+ `dyn Trait` |
| 第三方过程宏（编译期执行任意代码） | 宏即编译器输入，会破坏可复现构建与沙箱边界 | 卫生 `macro_rules!` + derive（§6） |
| `null` / 空指针值 | UB-009；裸指针仅 `unsafe` | `Option<T>`、`*const T` 在 `unsafe` 内 |
| 异常跨 `"C"` 边界展开 | 外部 ABI 无 unwind 表 | `Result` + 出参（§7.6.4） |
| 热更新 / 运行时代码生成（含 `eval`） | 无法静态校验，违反 P3 | 重新构建（B 轨 `E-0711`） |
| 领域专用标准库（§1.3） | 语言须领域中立 | 自有 `.infib` + `std.host` |
| 多重继承 / trait 默认值覆盖优先级仲裁 | 方法解析必须唯一可判（§4.14 第 1 条） | 组合 + 显式消歧 |
| `#[cfg]` 式的源码级条件编译 | 已有 `//infy:build`（§2.2），两套并存会破坏 AST 唯一性 | 构建约束注释 + `features` |

### F.2 术语表（规范性定义，全文用词以本表为准）
| 术语 | 定义 | 首次定义处 |
| --- | --- | --- |
| **module** | 带版本的分发单位，由 `MODULE.infy.toml` 定义，含若干 package | §5.1 |
| **package** | 一个目录内的全部 `.infy`，共享一个命名空间；包名 = 目录末段名 | §5.1 |
| **产物（artifact）** | `infy build` 的单一自包含可执行文件，含 `.inf *` 段 | §7.4 |
| **A 轨** | 编译型语言嵌入：编译器自动完成绑定生成与链接 | §7.5 |
| **B 轨** | 解释型语言嵌入：构建期摄取其原生字节码并 lowering 为 IFBC | §7.5 |
| **IFBC** | INFY ByteCode，唯一执行用字节码格式；由装载校验器验证 | §7.8 |
| **能力表** | 每种 B 轨语言的"可映射 opcode / builtin / import"封闭集合 | §7.7.2 |
| **lowering** | 原生字节码 → IFBC 的机械指令映射，不做语义分析 | §7.7.1 |
| **`.infib`** | INFY 库文件；edition 为 `source` 或 `wheel`；不可直接执行 | §5.6 |
| **chain（链）** | 静态声明的工作单元拓扑，含编译期确定的节点表 | §8.2 |
| **node（节点）** | 一条宿主语句 + 其连携块；编号由 §8.4 算法唯一确定 | §8.4 |
| **focus（焦点）** | 一条链上的一个独立推进位置，拥有独立栈与所有权作用域 | §8.5 |
| **linkage（连携）** | 挂载在宿主语句之后的跳转/启动语句块 | §8.1 |
| **宿主（host）** | 链接 INFY 运行时并实现 `HostApi` 的程序 | §11 |
| **抢占点** | 调度器可介入的封闭位置集合（5 类） | §8.5 |
| **终止半径** | panic 影响的范围：`focus` / `chain` / `process` | §4.16 |
| **可观察行为** | 诊断文本与码、节点表、drop 顺序、`Debug`/`Ord`/`Hash` 输出、迭代顺序 | §12.3 |
| **IFABI** | 产物 ABI 版本，独立于语言版本演进 | §1.2 |
| **build-id** | 全部输入规范化后的 SHA-256，标识可复现构建 | §7.3 |
| **dirhash** | module@version 的逻辑文件树 Merkle 哈希（`h1:` 前缀） | §5.8 |

### F.3 索引（高频条款速查）
| 想查 | 去 |
| --- | --- |
| 为什么某处报 `E-0712` | §7.7.2 能力扫描表 + 附录 E.3 |
| 能不能把 Python 库直接拿来用 | §7.7.4（`infy ingest py-wheel`）+ §7.7.3 版本锁定 |
| 链节点编号会不会因我改一行代码而全变 | §8.4 第 6 条（会，故 `goto(绝对索引)` 有 `N-0800`） |
| `&mut` 与 `&` 共存为什么被拒 | §4.7.2 规则 1（`E-1002`） |
| 依赖被换包怎么发现 | §5.8 第 2/3 项（`E-0560` / `E-0562`） |
| 格式化后代码变了语义吗 | §9.5 第 13 条（`E-0092`：不会，且自检拒写） |
| C 代码崩了会怎样 | §4.16 + §7.6.6（同进程不承诺可恢复） |
| 想加一个业务专属 API | §1.3 三条出口 + §10.8 `std.host` |
