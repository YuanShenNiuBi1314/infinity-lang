# 规范偏差记录

规范是实现必须遵守的唯一权威；下列各点是规范**未定**或**自相矛盾**之处，本实现做出的唯一确定选择。
每条都标注条款来源，便于审签时否决。

## D-1 §9.3 没有通用「语法错误」码
§9.1 的区间表把 `030x` 给「语句与连携」，但 §3 的任意产生式失败（"期望 `)`，实际 `;`"）没有专属码，
而 UB-024 禁止实现私自新增 `E-` 码。
**选择**：所有"期望 X 实际 Y"的解析失败一律报 `E-0053`（§2.11 语句缺少终结符），因为按 §3.5 的文法，
一个记号出现在文法禁止的位置，唯一合法的修复动作就是补/删终结符或块边界。
新增码需要走 `X-infyc-<n>` 命名空间（§9.1），本实现未使用。

## D-2 §9.2 示例的两处内部不一致
1. 标题行 `cmd/host/main.infy:12:25` 的列 25 不是 caret 区起点（`ticks * 1.5` 起于第 21 列）。
2. 建议行前有 2 个额外空格，其下的 `+` 标记行却按**未加缩进**的几何对齐；且 `) as u32` 的插入
   被标记成连续 6 个 `+`，与逐字符标记规则不符。
**选择**：实现按 §9.2 模板的几何规则确定为——`gw = digits(最宽行号)+1`，正文行 `gutter + ' ' + 源码`，
caret 行 `spaces(gw-1) + │ + spaces(col_start) + '^'*width + "  " + label`；
建议正文行多 2 空格，标记行与 caret 行对齐，插入的**非空白**字符打 `+`（插入的空白打空格）。
标题列取诊断的「主位置」，由各阶段自行指定（不与 caret 区间绑定）。
`src/diag.rs` 的金样测试逐字符比对该示例的前 6 行；标记行以本规则为准。

## D-3 §9.1 区间表与实际码不匹配
14 个码不在区间表任一区间内：`E-0200/0201/0202/0210`（`020x`–`021x` 未列）、`W-1010/1011/1012`、
`R-1010`、`E-1030`–`E-1034`、`W-1030`（`103x` 未列）。区间 `105x`（"IFBC 装载校验"）下没有任何码
（IFBC 实际用 `070x`/`073x`）。另有主题错配：`R-0001…R-0012` 落在"词法基础"区间，`E-0053` 落在
"字面量/字符串"区间，`E-0093` 落在"`infy fmt` 自身故障"区间。
**选择**：保留码值不变（码即接口），`group` 字段按语义就近归类，区间表偏差本身作为规范缺陷上报。

## D-4 §9.3 的 `W-` 列表与 `R-` 表缺少列
`W-` 只有一行罗列（无触发场景/修复建议列），`R-` 表只有"含义"列，共 39 个码没有规范性 note/help。
**选择**：由定义条款推导 note/help 文本，并在 `src/codes.rs` 头部注明哪些是推导的。
`E-0111`/`E-0503` 的触发场景在规范中写作"同上"，实现按上下文补全。

## D-5 附录 E 的伪 TOML
E.2/E.3/E.4 用 `;` 在同一行分隔键值（`lang = "cpython" ; format = "CPY-313"`），并出现 `[net] ; proxy = …`；
标准 TOML 不允许。§5.2 示例还使用 `(* … *)` 注释。
**选择**：`src/toml.rs` 把 `;` 接受为**语句分隔符**、把 `(* *)` 接受为注释，作为规范符合性扩展，
并在文档中标注；其余不合子集的构造返回类型化错误，由调用方报 `E-0506`。

## D-6 E-0701 / R-0702
§9.3 声明二者废止且编号永久保留。表中仍保留条目（标题加`（废止）`），任何实现不得赋予新语义。

## D-7 附录 E-0093（`[lints]` 降级 `E-`）
§9.6 允许配置文件中出现对 `E-` 的降级尝试并要求以 `E-0093` 拒绝；该码只出现在 §9.6 正文，
不在 §9.3/§9.7 表中，实现按正文语义收录。

## D-8 焦点栈的实现形态
§8.5 规定焦点是"用户态协程栈挂在 OS 线程池上"。可移植 C 无标准协程原语，本实现以线程栈承载焦点，
但保留全部可观察语义：推进位置、独立所有权作用域、封闭的 5 类抢占点、公平轮转、`focus` 终止半径、
`R-0802` ID 回绕拒绝。见 `docs/internals/runtime-abi.md` §5。

## D-9 §9.5 规则 6 的空行次序
"`import` 块与 `package` 之间 1 个" 与 "`package` 前 0 个" 在 §3.1 的 `file_attr* package import*` 次序下
不可同时成立。实现取"`package` 前 0 空行，`package` 与 `import` 块之间 1 空行"。

## D-11 §3.1 的属性文法与 §6.6/§1.2/§14 的写法互斥
§3.1 写 `attr = "[" attr_path ( "(" attr_args ")" )? "]"`，即赋值只允许出现在括号内；
但 §6.6、§1.2 与 §14 全部使用无括号的 `#![infy_version = "2.0"]` / `#![entry_chain = "Foo"]`。
两者不可能同时成立。
**选择**：解析器额外接受 `attr_path "=" value`（无括号单赋值），因为它在规范正文中被大量使用且
语义无歧义；括号形式保持原样。审签时可把 §3.1 改为
`attr = "[" attr_path ( "(" attr_args ")" | "=" attr_value )? "]"` 以消除偏差。

## D-12 §3.5 的连携块与 `;` 的相对位置
§3.5 说 `linkage_block` "仅允许附加在 expr_stmt 后"，而 §14.3 的写法是宿主语句带 `;`、
连携块另起一行：
```
gate();
{ if (build::all_green()) { goto("done"); } goto(+1); }
```
若严格按"表达式语句后可直接跟 `{`"实现，这个官方样例会被解析成一条独立的块语句（且 §3.5 的
statement 集合里没有裸块语句）。
**选择**：解析器先吃 `;`，再把紧随其后的 `{ … }` 归给刚才那条 `expr_stmt` 的 `linkage` 列表；
`f() { … }`（无 `;`）同样接受。二者在 AST 上等价。

## D-10 码数
§9.1 声称 210 个码；§9.3+§9.7 表格实际给出 209 个，加上只出现在 §9.6 正文的 `E-0093` 恰为 210。
本实现收录 210 个，测试以"≥200"为下界而非硬编码 210，以免把规范计数当成事实。

## D-13 §3.1 的属性参数名文法与 §7.6.3/§14 的 `#[link(…)]` 互斥
§3.1 写 `attr_args = ident ("=" (expr | str_lit))? …`，但规范自己的链接属性样例是
`#[link(name = "decoder", static = true, flags = ["-O2"], toolchain = "cc")]`（§3.7 注释、§7.6.3、§14），
其中 `static` 按 §2.4 是**保留字**；同理 `attr_path = ident` 与 `#[link(...)]` 的 `link`（§2.4 保留字）冲突。
按文面实现会让官方样例在词法/语法期直接失败（实测：整条 `#[link]` 被丢弃，A 轨配对检查 `E-0720` 永不触发）。
**选择**：属性路径片段与属性参数名位置接受"词记号"（`Ident` 或 `Keyword`，取记号原文），
即 `#[`、`(` 之后的 `词 =` 形式一律视为命名参数。这是 D-11 的同一条冲突在参数侧的延伸。
建议规范把文法改为 `attr_path = word ("." word)*`、`attr_args = word ("=" attr_value)?`。

## D-14 `.` 之后的成员名可以是保留字
§7.0.1 第 4 条把字段/索引链定义为"从左到右逐段求值"，§3.11 写 `expr "." ident`；
而 §9.5 规则 3 的链式断行示例（本仓库 `fmt` 语料即取自该风格）含 `.link()`，`link` 按 §2.4 是保留字。
**选择**：`.` 之后接受词记号（`Ident` 或 `Keyword`）作字段名/方法名。理由：`.` 之后的记号不可能是
语句起始，按上下文取为成员名无歧义；拒绝会使规范自己的格式化规则无法验收。
不扩大范围：仅在 `.` 之后生效，其余标识符位置仍按 §2.4 报 `E-0020`。

## D-15 §9.5 规则 3 的"续行缩进 8 空格"取相对一层
规则 3 说链式调用超软上限时"在每个 `.` 前断行，续行缩进 8 空格"。字面的 8 列绝对缩进会让
嵌套在 `if`/`loop` 内（语句本身已在第 8 列）的续行比语句更靠左，任何格式化器都不产出这种东西；
而规范示例都在第 1 层（语句 4 列 → 续行 8 列），两种读法在该例上不可区分。
**选择**：续行缩进 = 语句缩进 + 一个 `INDENT`（4 空格），第 1 层的规范示例正好得到 8 列
（`CHAIN_CONT = INDENT`，`src/fmt.rs`）。规则 2 的"4 空格一级"是唯一与 §9.5 其余各条自洽的解释。

## D-16 `@embed` 参数列表恒为单行
§2.11 说 `@embed` 的**块体**是词法不透明区，§9.5 规则 2 说只规范化块的首行缩进；对
`@lang( args )` 这段普通代码是否参与软上限断行没有规定。实现里块体的逐字节原文含换行，
按 Doc 模型会使包含它的组强制 break，于是 `@c(name = "d")` 被拆成三行，且第二遍格式化继续变化
（违反规则 12 的不动点要求）。
**选择**：`@lang(…)` 的参数列表以单行形态发射（与属性值表达式同一手法 `flatten_doc`），
不参与断行。代价：超长的嵌入参数行可能超过 100 列软上限（规则 1 的软上限不是硬约束，硬上限 120
仍由 `--check` 之外的路径保持可解析）。

## D-17 §14 导览源码不是 §9.5 的不动点 —— **已裁决（F6 轮，2026-10-17）：§9.5 是唯一定义源，样例侧改写**
A3 轮实测：`(cd examples/tour && infy fmt --check)` → `2 file(s) are not canonical`，exit 2。
§9.5 声称格式化输出是字节唯一的、且 `fmt` 的输出是其自身不动点（规则 12）；§14 的导览是规范自带的示例源码。
两者互相矛盾，且规范没有写明哪一侧让步。

**裁决 = §9.5 胜，样例改写到规范形态。** 依据三条：① §9.5 开头自己声明了权威等级 ——
「以下即规则本身；实现不得偏离（UB-014）」，而 UB-014 的判据是「对同一 AST 必须输出字节级唯一」，
即规则就是"官方风格"的**定义**；② §14 的标题是「语言导览（完整可编译示例）」，它约束的是**可编译**，
不是排版 —— 本仓库的 `examples/tour` 本来也不是它的逐字节副本（§14.3 带 `import` 块与逐行节点注释，
导览里都没有），所以"改了样例就违背规范"这个前提不成立；③ 反向裁决（改格式化器去迁就 §14 的排版）
等于让实现偏离一条写死的规则，而规范没有任何一条规则授权实现去"照顾样例的观感"。
**反方证据如实登记，供审签否决**：全规范 0 处把 `pub` 单独成行、5 处写的是 `pub enum` / `pub struct` /
`pub trait` 同行（`spec:1796/2034/2037/2045/2047`）—— 规则 10 与规范的**所有**样例都冲突，这是规范级缺陷的
形态之一；本条按规则文本裁决，因此 §14 的围栏代码块从今天起**不能**再当作格式化金样引用。

### 逐条判据（`infy fmt examples/tour` 实际改写的六种形状，一条不落）
1. **`#![entry_chain = "Pipeline"]` 与 `package` 之间的空行删除** —— §9.5 第 6 条「`package` 前 0 个」，
   次序矛盾已在 **D-9** 裁过（取"`package` 前 0 空行、`package` 与 `import` 块之间 1 空行"）。样例侧错。
2. **`pub` 前置成独立一行**（`pub enum Phase {…}` → `pub` + `enum Phase {…}`）—— §9.5 第 10 条
   「`#[..]` 与 `pub` **各占一行前置**」的字面。实现侧原样保留（`fmt::vis_doc` 发 `HardLine`），
   已由 `fmt::tests::attrs_and_pub_each_get_their_own_line` / `repr_c_stays_on_the_struct` 钉住。
   样例侧错。**只作用于 item 级可见性**：结构体字段的 `pub job: u32` 仍在同一行（第 4 条要求能压回单行的
   列表必须压回），实测输出逐字如此，没有第二条解释空间。
3. **连续的 `fn` 声明之间补恰好 1 个空行**（导览里 7 个 `fn` 挤在一起）—— §9.5 第 6 条
   「函数/事件/类型声明之间恰好 1 个空行」。样例侧错。
4. **空体 `{ }` → `{}`** —— 第 6 条「块首尾的空行删除」，且空块没有可逐项一行的元素。样例侧错。
5. **单行块一律拆行**（`if (n == 0) { return Err(…); }`、`fn all_green() -> bool { true }`）——
   §9.5 **未定**：第 4 条的"未超限时禁止逐项一行"只点名实参列表/数组·元组字面量/`match` 臂/`where` 子句，
   不含语句块；第 3 条只规定**超限**才必须换行。**本实现选定**：块体恒逐行（`{` 与 `}` 各占一行、体内语句
   各占一行），空块走第 4 条压成 `{}`。这是一处实现选择，不是规范的命令。
6. **宿主语句与其连携块合并成 `gate() {…};`**（源码写的是 `gate();` 后跟一个 `{…}`）—— §9.5 **未定**。
   **本实现选定**：按 AST 的既有形状发射（`StmtKind::Expr{expr, linkage}` 里连携块本就属于该宿主语句，
   所以打印成"宿主 + 尾随块"）。**AST 不变性不是靠自证**：`fmt` 内的第 12/13 条自检（`E-0091`/`E-0092`）
   全程沉默，且对规范化前后各跑一次阶段 [7]-[8]，**生成的 C11 逐行 diff 只差一处** ——
   `Pipeline_locs[6]` 里那六个行号常数（8/9/10/11/16/19 → 7/8/9/10/16/20），其余 12512 字节逐字节相同；
   节点数、`linkage_count`、`goto` 的编译期目标、`InfyChainDesc` 行全未变 ⇒ 折叠是**排版**，不是语义。

### 裁决的后果（本轮已一并落，不留半截）
- `examples/tour` 的两个 `.infy` 由 **`scripts/canonicalize-tour.ps1`**（默认干跑 / `-Apply` 才写 /
  写完回核 / 越出 `examples/tour/**/*.infy` 即还原并退非 0）规范化，`infy fmt --check examples/tour`
  实测 exit **0**（debug 与 release 两个二进制各一次）。
- 本机随 tour 字节变化的三组常数**已同步**：生成 C11 **12513 → 12512** 字节、`build-id 4d8250b5…55ba →
  d7c4408e84614ac24fe0475c31e1af24c9ab27c2790c605d9e371b4fac370015`、节点表 6 行 **8/9/10/11/16/19 →
  7/8/9/10/16/20**（`docs/status.md` / `docs/coverage.md` / `README.md` 的**现状**段；各轮历史记录行保留当时实测值）。
  产物字节 **199975** 与段 blob **738** 未变。
- 锁点两处：`fmt::tests::the_tour_sources_are_the_9_5_fixpoint`（把两个文件读回来逐字节比，
  负对照实测：把 `pub\nenum Phase` 拼回一行该测试即红）+ `.github/workflows/ci.yml` 步 12 新增
  `"$INFY" fmt --check examples/tour`（曾以"D-17 未裁决"刻意排除，现已请进门禁；
  `conformance::tests::the_ci_workflow_covers_the_build_the_tests_and_both_c_harnesses` 的必需清单加了这一条，
  负对照实测：yaml 里删掉该行该测试即红）。
- 措辞解禁：本条裁决后，文档**可以**说"§14 导览的仓库副本是 §9.5 的规范写法"，但仍**不得**说规范 §14 的
  围栏代码块本身是规范写法 —— 那是本条登记的反方证据所在。

## D-18 未知字面量后缀由语法层报 `E-0053`，`E-0043` 的"后缀不在集合内"分支不可达
§2.8 规定后缀集合封闭，`src/sema.rs::int_lit_ty` 为此写了 `other => E-0043` 分支；但词法器只在
认得该后缀时才把它并入整型字面量，`1u99` 实际被切成 `1` + 标识符 `u99`，于是报错来自语法层：
实测 `fn f() { let x = 1u99; }` → `E-0053 语法错误：期望 ;，实际为 'u99'` + `E-0101`。
**选择**：不改词法器（把任意 `u\w+` 吞进字面量会让 `1u` 后接标识符的输入更难恢复），保留"未知后缀必被拒绝"这一
§2.8 承诺，由 `E-0053` 承担；`sema` 的 `E-0043` 分支只对词法器承认的后缀形态生效（溢出判定仍走 `E-0043`，已测）。
回归测试 = `sema::tests::unknown_literal_suffix_is_rejected`（断言"必须被拒绝"，不锁死具体码）。

## D-19 `E-0121` 的判据挂到「全局可变路径」：数值也报、链内局部 `var` 不报、span 指向写入处 —— **已裁决（F7，交付提交 `13ab2cf`）**
**规范给的是三处，不是一处**：§4.3（spec:563）「跨链裸 `&mut T` / 直接写同一 `var` → `E-0121`」、
UB-006（spec:675）「跨链无锁可变共享 ⇒ 必须 `E-0121` 编译错误，不是运行时竞争检测」、
§13 码表（spec:1421）触发场景写死「**多焦点写同一绑定**」。而 §8.2（spec:1297）与 §8.5（spec:1319）规定
每条焦点拥有**独立栈与独立所有权作用域**、跨焦点共享只能经 `Arc`/`Channel`；`src/codegen.rs:1639` 把链体内的
`let`/`var` 落成**帧槽**（frame slot）。于是 F7 之前那条判据（`src/borrow.rs:460`：`start(chain=B)` 时扫存活绑定，
条件 `mutable && !ty.is_numeric() && !matches!(ty, Ty::Rc{..})`）两头都错：

- **(a) 数值不排除**（判「实现错」）。数值绑定不比结构体绑定安全，排除它没有任何规范依据。
  新判据里类型形状只用于**豁免**，不用于入选。
- **(b) 链内局部 `var` 不报**（判「实现错」，方向偏严）。§8.2 的独立栈使「两条链写同一个链内 `var`」
  在语言模型里**不可构造**；旧判据报的正是这个不可达形状。反方证据照实登记：§4.3 的字面措辞
  「直接写同一 `var`」在 §8.2/§8.5 下没有对应形状，属规范措辞缺口 —— 本实现按 §13 那句
  「多焦点写同一绑定」取**可达**读法：能同时属于两个焦点的绑定只有进程级全局路径。
- **span 判「实现错」**。`b_span_default()` = `Span::empty_at(0,0)` ⇒ §9.2 打不出 `file:line:col`。
  现主区域 = 写入处，第二区域 = 声明处；因 `diag::render_one` 只渲染 `primary` 那一区（`src/diag.rs:478`），
  声明座标同时写进 note，否则第二区域只有 JSON 出口看得见。

**新判据（`src/borrow.rs` 的 `note_write` + `emit_races`）**：把 `static mut` 当作候选全局可变路径，
按 `(package, name)` 键（`sema::ConstInfo` 为此新增 `is_mut`；`borrow::check` 为此新增 `file_pkg` 参数），
收集"写入形状"= 赋值目标 / 表达式型赋值 / `&mut G` 的根标识符（`G`、`G.f`、`G[i]`、`*G` 都算穿过去写），
同名的语句级绑定会**遮蔽**全局（§4.3 把两者分开），被遮蔽的写不入选。走完整个 module 后才判定，因为
"有没有第二个焦点"要看全部链体：可调用体（`fn`/`event`/`impl` 方法）写 ⇒ 报；≥2 条不同链写 ⇒ 报；
唯一写链被任一 `start(chain = C)` 点名（含自启，§8.2 重入安全=第二条焦点）⇒ 报；
**只有这一条链、且模块里没人 start 它** ⇒ 这是分析能**证明**单焦点的形状，保持沉默（§4.7.2 第 6 条要求的是
"证明不了就报"，不是"看见全局就报"）。豁免只给 §4.3 点名的载体：`Arc<..>`（sema 里 `Ty::Rc{atomic:true}`）
与 `Mutex`/`RwLock`/`Channel`/`Guard`；非原子 `Rc` **不豁免**（引用计数本身就是竞争对象）。

**明确未做、不许当成已做**（登记为 BACKLOG F47 与本段的剩余面）：
① 读侧与写侧的竞争（§13 只写「写同一绑定」，实现只做写）；② 经 `Mutex`/`Channel` 句柄的**方法调用**
跨链访问不做语义分析（只按类型豁免）；③ 闭包捕获跨焦点、跨链裸 `&mut T` 这两条 §4.3 危险面在当前前端
**不可表达**（`start(chain=…)` 不带实参，闭包没有跨焦点通道），因此对应判据不存在发射点 —— 这不是
"已实现并通过"，是"没有通往它的路径"；④ `E-1031`（§4.7.5 的 `static mut` 必须 `unsafe`）全链路无发射点，
`src/borrow.rs` 的模块文档此前把它列为已实现，是假声称，现已改为显式"未实现"并注明 F47。
**锁定点**：`borrow::tests` 的 `numeric_global_written_by_two_chains_is_e0121`、
`restarted_chain_global_write_is_e0121`、`global_written_from_a_callable_body_is_e0121`、
`chain_local_var_across_start_is_not_e0121`、`single_chain_never_restarted_is_clean`、
`arc_carrier_global_is_clean`、`shadowed_local_write_is_clean`、`e0121_reports_the_write_site_and_the_declaration`
（+ 原有 3 条不误报的边界）；外部用例 `conformance/cases/4.7-ownership/4.7-ownership-0006` 的源已换成
两条链写同一 `static mut` 的形状（`expect = ["E-0121","E-0121"]`，仍 Pass）。
**若审签否决 (b)**（即认为链内 `var` 也该报），要改的是 `emit_races` 的入选集与 0006 用例的源，
而不是回滚 (a) 与 span 两条。

## D-20 `{:.N}` 对 `str` 数的是**字节**（回退到码点边界），`{:#X}` 前缀取小写 `0x`
§10.11 只写了格式选项全集 `{:>10} {:<10} {:^10} {:0>10} {:.3} {:e} {:?} {:#?}`，既没说 `{:.N}` 的 `N`
数码点还是字节，也没说 trait `UpperHex` 在 `#` 下的前缀大小写。两处都属"规范未定"，实现必须选一个确定的。
**选择 1（精度）**：`infy_rt.c` 的 `utf8_truncate` 按**字节**取前 `N` 个字节，若会把一个 UTF-8 序列切断就整段回退，
因此输出永远是合法 UTF-8，但可见结果与参考实现家族（Rust，`N` 数码点）在非 ASCII 上不同：
`fmt("{:.4}", "日本語x")` 在 INFY 得 `"日"`（3 字节），Rust 得 `"日本語x"`（4 码点）。ASCII 上两者一致（`{:.3}` → `"hel"`）。
**选择 2（前缀）**：`{:#X}` 打 `0x` + 大写字母（`0xFF`），与 `rustc` 实测一致；此前实现打 `0XFF`。
锁定点 = `runtime/test_rt.c` 的 `test_fmt`（`CK_STR(one(s, A_STR("日本語x")), "日")`、`CK_STR(one(s, A_UINT(255)), "0xFF")`）。
若 §12 的 fmt 金样以后规定按码点计数，以规范为准改实现，本条改判。

## D-21 焦点 id 从 1 起分配，0 保留为"无焦点"哨兵
§8.5 规定焦点 id 单调分配、不复用、到 `u32` 回绕点直接 `R-0802` 拒绝新焦点，但没有规定起点。
而本运行时有两处用 0 表达"没有焦点"：`infy_current_focus()` 在无焦点上下文返回 0，`infy_focus_spawn()`
的三条拒绝/失败路径（链不在镜像表、节点越界、id 空间耗尽）也返回 0。若第一个焦点也叫 id 0，
调用方无法区分"拿到的是 0 号焦点"和"没拿到"。
**选择**：`infy_sched_init` 把 `next_focus_id` 置 1，0 永久保留为哨兵（回绕判定仍是 `>= 0xFFFFFFFF`，
可分配 id 为 `1 … 0xFFFFFFFE`）。锁定点 = `runtime/test_rt.c` 的 §11.4 节流用例（`CK(tf != 0u)`）。

## D-22 `R-0709` 被规范自身双重占用：附录 D 的 IFBC"边界"校验 vs §9.7 的嵌入内存配额 —— **已裁决（BACKLOG B3）**
B2 轮逐条回查规范后确认这是**规范级自相矛盾**，不是实现错误：
- §9.7 运行时码表（spec:1502）：`R-0709 = 嵌入单元超出 max_mem_bytes`，与 spec:1267（`@embed(max_mem_bytes=…)` → 独立 arena，超限 `R-0709`）一致；
- 附录 D 装载校验映射（spec:2441）："任一失败即整体拒绝该模块，错误码经首失败项映射：线性 0x→`R-0704`、高度→`R-0705`、类型→`R-0706`、帧→`R-0707`、host 签名→`R-0708`、**边界→`R-0709`**"；
- spec:2417 又把"tailcall 帧尺寸不兼容"称为装载期 `R-0709`。

裁决前仓库的现状：`infy_rt.h` 把 `R_IFBC_MEMBUDGET` 定为 `0x0709`、`R_IFBC_BOUNDARY` 顺延为 `0x070A`（**无人使用**），
而 `R_IFBC_MEMBUDGET` 在 `infy_ifbc.c` 里同时背三种语义——装载期 tailcall 帧尺寸不兼容、`closure.new` 的 `code_idx`
越出 `func_table`、执行期 `live_bytes > max_mem_bytes`（只有最后一条是 §9.7 的原意），`test_ifbc.c` 还把它当
"R-0709 tail-call frame size"断言。另一条取证：D.10 的"边界"在 §7.8.3 的 1→6 里**不存在**（第 6 项是"行映射完备"→
`W-0705`，不阻断），"边界"来自 §7.8.2 的指令分类表（`ret`/`ret.multi` 与声明返回类型逐元一致），而 spec:1256 已把
这一条归进第 4 项"帧合法"（`R-0707`）。也就是说 D.10 的末项是把六个格子顺号编下来的结果，不是语义选择。

**裁决（B3 轮已落地）**：§9.7 是码表权威，`R-0709` 只表示"嵌入单元超出 `max_mem_bytes`"。
- 装载期"边界"类发现改报 `R_IFBC_FRAME`（`R-0707`）：`infy_ifbc.c` 的 `check_boundary()` 两处（tailcall 帧尺寸不兼容、
  `closure.new` 目标越出 `func_table`）；后者与同函数里早已存在的"call/tailcall 目标越出 `func_table` → `R-0707`"同类；
  `spec:2417` 所称"装载期 `R-0709`"因此一并改判，内存配额号不再被装载期借用。
- `R_IFBC_BOUNDARY = 0x070A` 从枚举与 `infy_rt.c` 的诊断码表里删除：`0x070A` 不见于规范任何码表，而该枚举自称闭集
  （`infy_rt.h` 注释 "closed set, no implementation extensions"），删除前它也确实没有任何使用点。
- **没有**把冲突项挪到 `R-0702`/`E-0701`：spec:1508 明文这两个编号"永久保留、不得赋予新语义"。
锁定点 = `runtime/test_ifbc.c` 的校验映射表（逐行断言确切码，`SC_TAILCALL_BAD` 与新增的 `SC_CLOSURE_BAD` 现断言
`R_IFBC_FRAME`；断言只增不减：79 → **82**）。同批把 `InfyRCode` 补齐为规范闭集：新增 `R_EMBED_TIMEOUT = 0x0701`
（§9.7 spec:1495，`tickloop` 预算/`timeout_ms` 越线时触发，不杀进程）与 `R_FIELD_OFF_REPR = 0x0713`
（spec:1627/D.9，此前 `infy_ifbc.c` 用文件内 `#define` 的裸整数返回），这两项是**补齐规范已有的码**，不构成偏差。
`docs/coverage.md` 的 §7.8/附录 D 一行因此可以声称"码表与 §9.7 对齐"；仍不改判 🟡 的原因是 **D4**（Rust↔C 的
IFBC 操作数个数与 `func_table` 记录布局不一致，双侧交叉验证尚不可行）。

## D-23 `frame_kind` 的两级取值表：模块级只有 `0`，函数级 `1 = coroutine` 由实现选定
§7.8.1（spec:1232 起的容器头定义）在头部放了 `frame_kind:u8 (= 0: 单地址栈式 + 局部窗口)`，那是**值模型**；
附录 D.8（spec:2434）要判的却是"`yield` 出现在 `frame_kind != coroutine` 的**函数**"→ `R-0707`。
规范既没把这两级分开写，也没给 D.8 的数值表（只出现 `coroutine` 这个名字），所以 `coroutine` 等于几必须由实现定死。
**选择**：两级分开，且两级的取值集合不同——
- 模块级（容器头第 46 字节）只允许 `0`，其它值视为容器非法：C 侧 `infy_ifbc_load` 返回 `R_IFBC_INVALID`（`R-0703`），
  Rust 侧 `src/ifbc.rs::decode` 返回 `IfbcDecodeErr::BadFrameKind`。Rust 侧此前允许头部写 `1`（判据是 `> 1` 才拒），
  与 C 侧的"必须为 0"不一致，本轮收紧成同一条规则；
- 函数级（`func_table` 每条记录）取 `0 = 栈式帧` / `1 = 协程帧`，`> 1` 装载失败（同上两个错误）。常量已在冻结公共头里
  命名：`infy_rt.h` 的 `INFY_FRAME_STACK` / `INFY_FRAME_COROUTINE`（Rust 侧对应 `FRAME_KIND_STACK` /
  `FRAME_KIND_COROUTINE`），此前 C 侧是裸 `== 1` / `!= 1`。
锁定点 = `src/ifbc.rs` 的 `frame_kind_header_and_function_value_sets`（头部 `1` 被拒、函数级 `1` 可装载、函数级 `2` 被拒）。
文档同批更新：`runtime/IFBC.md` §1.1/§1.5/§7、`docs/internals/runtime-abi.md` §9。
若 §12 合规套件以后给出 D.8 的正式数值表，以规范为准改实现，本条改判。

## D-24 stage [7] 三项规范未定的表示：元组 = 声明序 C struct、无 payload 的 enum 省掉 union、判别式只接受整数字面量
C1 轮落 `src/hir.rs` 时撞上三条"规范没写、但 `.infib` 一定能观察到"的表示问题，按协议 §3(c) 选一个确定实现：

1. **元组**。§3 的 EBNF 有 `type_list`（括号即元组/unit）、§4.12 之外多处使用元组（`ret.multi` 要"与 INFY 侧
   元组返回类型逐元一致"，spec:1249），但 §4.1 的表示表**没有元组一行**，`docs/internals/runtime-abi.md` §1 原来也没有。
   **选择**：元组 = C struct，元素按书写顺序，字段名 `a0`/`a1`/…；空元组即 `()` = `InfyUnit`（1/1）。
   实测 `(u32, bool)` = 8/4、`(u64, (u32, bool))` = 16/8（`a1`@8）。
2. **没有任何变体带 payload 的 enum**。ABI §1 写的是 `{ uint32_t tag; union { ... } v; }`，但 union 里一个成员都没有时
   C11 不允许空 union。**选择**：此时省掉 union，枚举就是它的 tag → `Phase` = **4/4**（带 payload 的仍是 tag+union，
   `FailWhy` = 8/4）。
3. **判别式的语法**。`Variant.disc` 是 `Option<u128>`（**该形状自 F10/F51 轮起为 `Option<ast::Discriminant>`，
   见 D-52(a)**），即只能装整数字面量；规范 §3 的 EBNF 没有 `enum_decl` 的产生式体，
   没写 `A = expr` 是否允许常量路径或负数。**选择**：只接受非负整数字面量，其它形态报 `E-0053`（解析器的语法码），
   字面量宽于 `u128` 时留给 §2.8 的范围检查（`E-0042`/`E-0043`），不重复报同一个记号。
   **F9 轮实测更正（2026-10-17）**：第 3 点的"留给 §2.8"今天**落空** —— `variant()` 只把 `i.value` 存进
   `disc`、字面量表达式本体被丢掉，所以宽于 `u128` 的 `A = 2^128` 与"没写 `= …`"是同一个 AST 形状，
   `infy check` 与 `infy build` 实测 **0 诊断 / exit 0**。已登记 BACKLOG **F51**（与 **F10** 的 `u128 as u64`
   回绕同族、不同层）；本条**选择**保持不变（不接受常量路径与负数），但"超宽必须报 `E-0042`"这一半在
   F51 修掉之前不许声称已实现。
   **F10/F51 轮更正（2026-10-17）**：那一半**已落地**，上面那句禁令撤销。`ast::Variant::disc` 不再是
   `Option<u128>`（本条第 3 点开头那个形状已过期），而是 `Option<ast::Discriminant>` —— 保存**写下来的
   字面量本身**加它的 `Span`，于是"没写 `= …`"与"写了但放不下 `u128`"重新是两个形状，§2.8 的范围检查
   才有东西可读。分层判据、收集宽度与格式化打印三件都定成 **D-52**。实测（`target/scratch/f10`）：
   `enum Wide { A = 2^128, B, C }` 与 `enum Fits { A = 2^64, B }` 现在各打一条 `E-0042`（指得住字面量本身，
   `core/embed/mod.infy:5:17` 起 39 列），`infy check` 与 `infy build` 都 **exit 2** —— 修前两条命令全 **exit 0 / 0 诊断**。

**顺带修掉的真实缺陷**（就是这条语法选择暴露的）：`src/parser.rs::variant()` 原来的判据是
`if let ExprKind::Int(i) = &e.kind`，而 `primary()` 的第一个臂 `Kind::Lit(l)` 已把所有字面量构造为
`ExprKind::Lit(Lit::Int(..))` —— 它下面那个 `Kind::Lit(Lit::Int(i))` 臂**永不可达**，所以 `ExprKind::Int` 从不产生。
后果：`enum Code { A = 3, B, C = 10 }` 的三个判别式全部静默变 `None`，`A = 3` 被当成 `A`，**且不打任何诊断**
（§4.1 的显式判别式在整条前端链上消失）。这是"码写了却在无产生式/不可达分支里"这一类缺陷的第 4 次出现
（见 `docs/RUNLOG.md` cron-07 与经验库 infy-unreachable-implementation-branch）。修法：`variant()` 同时接受两种形态，
非字面量报 `E-0053`。锁定点 = `parser::tests::enum_discriminants_reach_the_ast_and_bogus_ones_are_e0053`
（AST 侧 `[Some(3), None, Some(10)]`）与 `hir::tests::fieldless_enums_are_exactly_their_tag_and_discriminants_survive`
（HIR 侧 `(3, 4, 10)`：`B` 由前一个显式值续号）。
`primary()` 的死臂本身**未改**（改它会让 `ExprKind::Int` 突然在全树产生，风险跨模块），登记为 BACKLOG **F10**。
**F9 轮（2026-10-17）更正**：那一档"两个形状并存"已经不存在 —— 交付提交 `83e7dec` 删掉的是 `ExprKind::Int`
**变体本身**（唯一产地 `derive::lit_u` 改产 `ExprKind::Lit(Lit::Int(..))`），`variant()` 的双形状匹配因此收成一支。
上面那句"同时接受两种形态"保留为**当时的修法**，读法是"只接受 `ExprKind::Lit(Lit::Int(..))`"。

## D-25 `enum` 的标签恒为 `uint32_t`，不按 §4.1"判别式最小容纳"逐类型收窄
§4.1 的表示表（spec:538）给 `enum` 写的存储是"**判别式最小容纳** + payload（niches 由编译器选取，但**不得**被源码依赖，
除非显式 `#[repr(C)]`）"，而 `docs/internals/runtime-abi.md` §1 自 IFABI-2 起把标签固定成 `uint32_t tag`。
按字面读"最小容纳"，`Phase`（4 个变体）应该是 `u8` 1 字节；按 ABI 文档读是 4 字节。二者不能同时成立，必须裁决。

**裁决（C1 轮）**：标签恒为 `uint32_t`，理由四条——
1. §4.1 同一行就把表示交给编译器（"niches 由编译器选取"）并**禁止源码依赖**；只有 `#[repr(C)]` 才把布局升格为源码事实，
   而 §4.8 又规定 `offset_of` 只对 `repr(C)` 可用 —— 说明非 `repr(C)` 枚举的宽度本来就不是可依赖的语义；
2. `#[repr(C)]` 的枚举要与 A 轨 C 代码共享同一布局，而 C 的可执行枚举底层类型在参考目标上就是 `int`（4 字节）；
   逐类型收窄会让"同一个类型在 C 和 INFY 里宽度不同"成为常态；
3. 宽度随变体个数变化 = 加一个变体就改变 ABI。§4.15/§7.6 的 `.infib` 跨版本签名稳定性（`E-0605`）依赖"未修改的类型
   布局不变"，最小容纳直接违背它；
4. spec:1153 明令"未 `repr(C)` 的 `enum` 判别式"禁止跨边界传递 —— 与"布局由 IFABI 固定、源码不得依赖"一致。

**推论（已实现）**：判别式必须能装进 `u32`，否则是 §2.8 的字面量超范围 → `E-0042`，**不是**截断。
锁定点 = `hir::tests::a_discriminant_wider_than_the_tag_is_e0042`（`Hi = 4294967296` 报错且不产出该枚举）；
`hir::tests::fieldless_enums_are_exactly_their_tag_and_discriminants_survive` 锁 4/4。
注意（**F10/F51 轮更正，2026-10-17**）：这里原先写"`sema` 收集判别式时做的是 `v.disc as u64`（u128 → u64
截断），所以宽于 `u64` 的判别式在到 HIR 之前就已经回绕，HIR 的 `u32` 检查拦不住它 —— 登记为 BACKLOG **F11**"。
两件事都要更正：① 编号笔误，那条缺陷是 **F10**（F11 是 `[T; N]` 非字面量长度那一族）；② 它**已修** ——
阶段 [4] 现在对"装不进收集宽度 `u64`"与"连 `u128` 都放不下"两种判别式各打一条 `E-0042`，然后按隐式续号
继续收集，谁报 `E-0042` 的分层由 **D-52(b)** 定。本条的 `u32` 判据与 `hir` 那两条锁点未动。
若 §12 合规套件将来给出"最小容纳"的正式判据，以规范为准改实现，本条改判。

## D-26 stage [8] 每个 crate 发射**单个** C 翻译单元，不按 package 分文件

§5.10 把增量编译定为"以 package 为粒度"，§7.4 又把 `.inf text` 描述成"INFY 本体 + A 轨目标码按 §7.4 顺序链接"，
读起来像后端应当每个 package 产一个对象文件。本轮实现选择：**整个 crate 一个 `.c`**，写在
`<module>/.infy-build/<target-triple>/<module 末段>.c`。

**为什么这样选**：分文件需要为每个 package 产出头文件并把跨 package 的类型引用改成前置声明，
而 `sema` 的类型表（`Ty::Named` 只带简单名）**无法区分两个 package 里的同名类型**；先把"单一布局权威 +
单一发射单元"钉死，比先拆文件更稳。§7.2 明写"增量结果是优化，不得改变诊断集合"，而发射文本的划分
不是可观察语义（产物仍是单文件，§7.4）。

**代价（如实记录）**：`.infy-build` 里目前没有 per-package 对象，因此 §5.10 的缓存 key 与 §7.2 的
`--target` 交叉编译只做 triple 校验，没做增量。**锁定点** = `codegen::tests::generated_text_is_byte_stable`
（两次发射逐字节相同）。

## D-27 §8.4 规则 1 的"序言"归属歧义：节点间的声明在**前一个节点末尾**执行

§8.4 规则 1 说 `let`/`var`/纯赋值/`label`"**归属前一个节点，作为其"序言"**"—— 术语"序言"指在前，
归属却记为前一个节点，两种读法都会让 `goto` 跳过或执行同一条声明二选一。规范没有第三处澄清（全文
"序言"仅这一处，grep 验证）。

**选定实现**：节点 `k` 的函数体 = `k` 的宿主语句 → `k` 的连携块 → 到 `k+1` 的宿主语句之间的普通声明/赋值
（§4.10 源码顺序即执行顺序，且 §8.1 说连携块挂在宿主语句之后）；**第一条宿主语句之前**的声明进节点 0 的开头。
若一条链只有声明、没有任何 `expr_stmt`，后端**报错**而不是丢掉这些声明（§7.1 不许静默降级）。

**锁定点** = `codegen::tests::tour_chain_node_table_rows_are_the_c1_golden_lines`（行 8/9/10/11/16/19、
节点 3/4 各 1 个连携块）与 `goto_targets_are_resolved_at_compile_time`。

## D-28 `match` 的"不可能落到"分支发射 `infy_panic(R_BAD_BOOL, …)`

§3.12 把穷尽性定为阶段 [4] 的静态性质（不穷尽 → `E-0092`），因此运行期"一条臂都没中"按规范不可能发生；
但后端若发射空 `else`，带值 `match` 的结果临时量会**未初始化**（§4.9 UB 清单里的未初始化读）。
规范没有为"无臂命中"分配 `R-` 码（§9.3/§9.7 全表核对）。

**选定实现**：`else { infy_panic(R_BAD_BOOL, "codegen: match 未命中任何臂（…）", loc); }` ——
`infy_panic` 永不返回，于是每条路径都有定义值；消息明写这是后端与阶段 [4] 不一致的实现 bug，而不是伪装成
一种语言语义。**锁定点** = `codegen::tests::match_arms_become_tag_tests_with_a_non_returning_fallthrough`。

## D-29 含 U+0000 的 `str` 字面量被**拒绝**，而不是静默截断

`infy_rt.h` 的 `infy_str_of(const char*)` 以 `strlen` 计量（`infy_rt.c:756`），而 §2.9 允许 `str` 值内含
NUL（只有跨 A 轨边界才是 `R-0003`/UB-007）。若后端照发 `infy_str_of("a\0b")`，长度会算成 1 —— 假成功。

**选定实现**：codegen 遇到含 NUL 的字符串字面量直接报错（消息点名 `infy_str_of` 与 `strlen`），
等运行时补一个"值+长度"的字面量入口（属 §7.4 `.inf rodata` 字符串池，BACKLOG C3）再放开。
**锁定点** = `codegen::tests::str_literals_and_comparison_use_the_runtime_helpers`（正常路径）。

## D-30 `overflow = "saturate"` 与窄位宽移位：没有检查入口就**不生成**

§5.2 允许 `[profile] overflow = "saturate"`，§4.4 也说溢出策略可饱和；但 `infy_rt.h` 的饱和入口只有
`infy_saturating_add_u64/i64` 两个，没有 u8/u16/u32/i8/i16/i32 的饱和版本。同理 §4.12 要求移位量
≥ 类型宽度 → `R-0007`，而头里只有 `infy_shl_u64`/`infy_shr_u64`/`infy_shr_i64`，对 32 位类型会**漏判**。

**选定实现**：这两种情形报 `CgError` 并指名条款，绝不用"先按 panic 语义发出去"或"用 u64 助手凑"。
**锁定点** = 见 `docs/BACKLOG.md` F2 之外的覆盖表行；`codegen::tests::integer_addition_is_a_checked_runtime_call`
锁住"整数运算必须是 `infy_*` 调用"这一半。

## D-31 §4.7.1 的移动（owned 值换地址）：**F12 轮起改为发射 `infy_cleanup_cancel(&源)`**（此前是拒绝）

§4.7.4 + ABI §3 的释放靠 `infy_cleanup_push(fn, &obj, is_defer)` 登记**地址**、在作用域末尾
`infy_cleanup_pop_run()` 弹**栈顶**实现（`infy_rt.h:115-118`）。`let d = s;`、把 owned 值按值传给形参、
`return s;` 三种移动都会让已登记的地址失效：要么留下双弹（同一缓冲两次 `infy_free` = 段错误），
要么对已经搬走的值执行释放（悬垂）。

**（历史）C2 轮的选定实现**：三种移动形式一律 `CgError`，并在 `docs/BACKLOG.md` 登记为 **F12**
（建议给运行时加 `infy_cleanup_relocate(old, new)` 或按地址移除的 API）。

**（现行）F12 轮的选定实现** —— 规范 §4.7.1/§4.7.4 只说"移动后由**新所有者**在它的作用域里释放"，
没有规定运行时必须能"搬迁"，所以选**最小那一个**：

* **按地址摘除，不搬迁**：C 侧新增 `uint8_t infy_cleanup_cancel(void* obj)`（`infy_rt.h`），从内到外取
  **第一条 `obj` 相等且 `is_scope == 0`** 的清理项，摘掉记录本身、**不跑 Drop**，返回是否命中。
  没有做 `relocate(old, new)`：代码里从来不需要"把项搬到新地址"，只需要"让源那条不再作答"，
  而目的地由 codegen 另 `push` 一条 —— 少一个入口就少一处会错的地方。
* **`obj == 0` 直接返回 0**（不匹配任何项）：codegen 只会登记 `&local`，允许 0 等于让一次调用取消掉
  任意一条以 0 当载荷的**手写**项（`infy_cleanup_push(fn, 0, 1)` 的 defer 就是这个形状）。
* **跳过 scope 哨兵**：`infy_scope_mark()` 塞进同一个 deque 的哨兵不是清理项，被摘掉会让 §3.13 的括号失衡。
* **`#[must_drop]` 仍报 `R-0730`**：`cancel` 与 `pop_noop` 都是"移除而不跑"，所以移动一个 `#[must_drop]`
  值必须和 `#[no_drop]` 那条路一样响（同形语义，不是新判据）。
* **成对不变量在 Rust 侧**：`Binding` 带 `pop: Option<(usize, usize)>` = 它欠的那条 `pop_run` 的槽位，
  `pops` 是 `Vec<Vec<Option<String>>>`；取消 = 摘运行时的项 **+ 清自己的槽位**同一动作。
  于是"一条 push 恰好一条答案"（`pop_run` / `pop_noop` / `cancel`）这个 `close_scope`/`drain_pops`
  依赖的不变量在三种形式下都成立，可机检（`codegen::tests::cleanup_ledger`）。
* **`return` 的判据是"按值提到"**：`mentions_by_value` 做标识符边界匹配，**左边是 `&` 就跳过** ——
  `&s` 是借用，所有权不动，取消它反而**漏**。两个方向的代价不对称（多取消 = 漏、少取消 = 双释放），
  所以两条都有锁：`a_return_that_carries_the_binding_in_a_literal_cancels_its_entry` 与
  `a_borrow_leaves_the_cleanup_entry_in_place`。
* **两条"拒绝"实测不可达，删掉**：① 把 owned 按值交给 A 轨 `extern` —— 阶段 [4] 就 `E-0723`
  （§7.6.4 行 3/末条：`String`/`Vec` 的裸内存不得跨边界），codegen 那条分支永远走不到；
  ② 把 owned 移进链帧槽位 —— `emit_chain` 在**铺帧之前**就对任何带析构器的帧局部报
  「帧由调度器释放（`infy_free(f->frame)`），释放时机未实现」。两者都不是本轮的缺口，
  且②那条真拒绝此前**没有测试**，F12 轮补上 `a_chain_frame_local_with_a_destructor_is_refused_by_name`。

**锁定点**：`codegen::tests` 的 `a_let_move_cancels_the_source_entry_and_keeps_the_destination_pop` /
`an_owned_byvalue_argument_cancels_the_callers_entry` / `a_returned_owned_binding_cancels_its_entry` /
`a_return_that_carries_the_binding_in_a_literal_cancels_its_entry` /
`a_borrow_leaves_the_cleanup_entry_in_place` / `moving_a_value_that_never_registered_an_entry_emits_no_cancel`
（六条共用 `cleanup_ledger`）+ `owned_values_register_a_cleanup_entry_at_their_scope`（正常路径的
push/shim/pop 与元素大小）；C 侧 = `runtime/test_rt.c` 的 `test_drop_order` 四组 **+13 断言**
（非栈顶摘除只免跑一次 Drop / 按地址命中而非按值 / 不许摘 scope 哨兵 / `#[must_drop]` 仍 `R-0730`）。
**验收本体**另见 `docs/coverage.md` 的 **P36**（release 二进制实跑三份最小 module + 逐行读生成的 C）。
被替换掉的旧锁点 `moving_an_owned_binding_is_refused` 锁的正是本轮关掉的缺口，随之下线（**不是放宽断言**：
同一份源码现在必须**生成**，且旧地址的 `pop_run` 必须**消失**）。
不移动的 owned 值（形参、`let` 绑定到新建值）仍按声明序 push、作用域末尾 pop，LIFO 即 §4.7.4 的逆序。

## D-32 §7.4 的 `.inf meta` 只有 208 字节固定记录，规范点名的五张表没有槽位/没有字节表

§7.4 与 `docs/internals/runtime-abi.md` §6 要求 `.inf meta` 携带
`EventTable`/`ChainTable`/`NodeTable`/`ResourceKeys`/`EnvOffsets` + 入口描述 + 依赖闭包，但：

* 冻结头 `runtime/infy_rt_internal.h` 的 `INFY_META_OFF_*` 只给了 208 字节记录的字段表，**没有**
  这些表的**记录布局**（§8.4 第 5 条只列了字段名 `chain_id, node_idx, file:line:col, linkage_count,
  host_event_id?`）；
* `ResourceKeys` 与入口描述**没有偏移槽位**；
* `opt_level`/`lto`/`fast_math` 是无值表的裸整数；
* §7.2 的 profile 只有字段（`opt_level`/`lto`/…），清单里不保存"选中的段名"，而 ABI §7 要求
  profile 名进 `build-id` 输入集。

**选定实现**（全部落进 `src/artifact.rs`，两侧读者共用）：

| 项 | 字节表（小端，blob 相对偏移） |
| --- | --- |
| blob 布局 | `meta(208) \| 字符串池 pair 表 \| chains \| events \| nodes \| ifbc \| sum \| export \| resource_keys \| rodata` |
| 字符串池 | `(u32 blob_off, u32 len)` × n；intern 顺序 = 固定字段序（target, build-id, platform, lang, toolchain, 入口名, chain…, event…, node…, sum…, 资源键…），去重走 `BTreeMap` 而非 `HashMap`（§7.3 禁止系统熵） |
| `NodeTable` | 32 字节/行 = `chain_id, node_idx, name_pool, file_pool, line, col, linkage_count, host_event_id`；`host_event_id` 缺省写 `0xFFFFFFFF` |
| `ChainTable` | 20 字节/行 = `name_pool, package_pool, node_count, frame_bytes, reserved=0`（`chain_id` = 行号） |
| `EventTable` | 12 字节/行 = `name_pool, arity, reserved=0`（`host_event_id` = 行号，同 §11） |
| `.inf sum` | 12 字节/行 = `package_pool, version_pool, dirhash_pool`（§5.8 的依赖 dirhash 清单） |
| `ResourceKeys` | 4 字节/键 = `key_pool`；**偏移由规则推出** = `export.off + export.len`（无槽位） |
| 入口描述 | 写在 `export` 区首记录 = `(u32 kind, u32 name_pool)`，`kind` 0 = `entry_chain`、1 = `fn main`（§7.4 优先级表的两个来源；§7.4 的"全无 → `E-0600`"在阶段 [7] 已由 `hir` 报出） |
| `opt_level` | §7.2 书写序 `0/1/2/3/z` → `0..4` |
| `lto` | §7.2 书写序 `off/thin/fat` → `0/1/2` |
| profile 名 | 由 opt 层反推：`-O0 → debug`、`-Oz → release-small`、其余 → `release`（§7.2 的三档定义就是这两参数的组合，所以反推是单射的） |
| `infy artifact info` 读非产物/损坏容器 | 借 `E-0605`（06xx = 构建/链接/工具链/可复现，§9.7 没有"容器损坏"专号），exit **7**（§13.4 校验和不一致） |

**锁定点** = `artifact::tests::meta_offsets_and_sizes_match_the_frozen_header`（逐条把
`INFY_META_OFF_*` 与 `INFY_ART_TRAILER_SIZE`/`INFY_ART_HDR_VER`/两个魔数对回**头文件文本**）、
`written_container_reads_back_every_field`、`node_table_rows_survive_the_round_trip`、
`entry_description_round_trips`（含"入口占 8 字节后资源键表不串位"）、
`cli::tests::artifact_cmd_indexes_positionals_and_refuses_non_artifacts`（`E-0605`/7 与非产物路径）。
跨语言侧：`runtime/fixtures/artifact_rust_writer.bin` 是 Rust 写入器的逐字节固定输出，
`runtime/test_rt.c` 用生产读取器 `infy_art_read_file` 读回同一组字段 —— Rust 写 ↔ C 读。

## D-33 §7.4 的"重复段尾"检查若按整文件扫描会否掉**每一个**真产物（规范级缺陷）

`docs/internals/runtime-abi.md` §6 写"运行时从文件尾找 `INFYART2` 段尾并校验 `trailer_crc`；
**缺失/重复即表示该文件不是 INFY 产物**"。C 侧原实现（`infy_rt.c` 的 `infy_art_parse_trailer`）
照字面把整文件扫一遍数魔数出现次数。

问题在于 P6 的单文件承诺：`.inf rt` 是**静态链接进镜像**的（§7.4），而 `infy_rt.c` 自己就带
`INFY_ART_MAGIC "INFYART2"` 这个字符串常量（正是这条比较用的字面量）。于是任何用本运行时链出来的
宿主镜像天然含有一份魔数 → 整文件计数 ≥ 2 → **runtime 拒绝自己的产物**、`infy artifact info` 也拒绝
自家 `infy build` 的输出。C3 轮第一次把 `infy build` 真接到阶段 [9] 时立刻撞上：
`error: 阶段 [9] 失败：host image already contains the INFYART2 magic`。

**选定实现**：重复性检查的范围收窄到**追加区** `[blob_off, file_len)`，段尾自身占 1 次，因此
"blob 里出现第 2 次魔数"仍然被拒（正是 §7.4 想拦的"被追加了两次"），而镜像里的那份字面量不再误伤。
两侧同规则：C 侧 `infy_art_parse_trailer` 与 Rust 侧 `artifact::parse_trailer` 都是
"先验 blob 范围 → 再从 `blob_off` 起数"；写入器另加一条自检：blob 自身含魔数直接拒
（`Bad::BlobCarriesMagic`），避免产出自己读不回的文件。
顺带修掉同一函数的一处 NULL 解引用：原先 `if (t) { …填字段… }` 之后无条件 `t->hdr_ver` 参与校验。

**锁定点** = `artifact::tests::duplicate_trailer_scan_is_scoped_to_the_appended_area`（镜像含魔数 →
读回成功；段尾再附一次 → `DuplicateMagic`；节点名叫 `INFYART2` → 写入端 `BlobCarriesMagic`）+
`runtime/test_rt.c` 的 `build_artifact(..., mode=2)` 那条（宿主镜像含魔数的容器 `infy_art_read_file`
返回 0，且 `n_nodes`/`opt_level` 仍对）。

## D-34 §8.2 的"`goto` 语义（移动焦点，暂停当前节点之后）"括号有歧义：裁为**第一个 `goto` 立即结束该节点**

§8.2 只给了七个字的括号说明"（移动焦点，暂停当前节点之后）"。它有两种读法：

1. **移动焦点 = 转移控制**：`goto` 之后的同节点语句不再执行，节点在 `goto` 处结束，"暂停当前节点
   之后"说的是*推进*这个动作发生在节点边界（§8.5 抢占点 ①"节点边界（`node_idx += 1` 前）"）。
2. **只登记目标、节点照常跑完**：一个节点里多条 `goto` 以**最后一条**为准。

读法 2 让 §14.3 的导览**永不终止**：节点 3 的连携块是
`if (build::all_green()) { goto("done"); } goto(+1);`，两条都会执行，`pending_node` 先写 5 再被
`+1` 覆盖成 4 → `retry_once()` → `{ goto("begin"); }` → 回到节点 0 无限循环。C4 轮第一次真执行
`examples/tour` 的产物时现场撞上（进程挂住 120 s 无输出、被强杀），而 `infy check`/`infy build`/
`infy artifact info` 全都是绿的 —— 这条路径此前从未被执行过。

**选定实现 = 读法 1**。依据：§8.2 的措辞是"**移动**焦点"（与 `start` 的"不移动焦点"成对反义），
§8.4 规则 3 说"焦点推进只看节点编号"，而 §14.3 把 `goto(+1)` 写在条件 `goto` 之后正是"否则顺延一个
节点"的惯用写法 —— 按读法 2 这个规范样例自己就是死循环。

发射形态：`infy_goto_checked(chain, target, loc);` 之后 `drain_pops(); return;`，即节点函数在此离开。
先 `drain_pops()` 是为了保住 §8.4 规则 4"defer、临时量 drop 计入该节点的**退出动作集**"——提前离开
节点不等于跳过该节点的 drop。`infy_goto_checked` 自身不变（它只写 `pending_node`），因此运行时侧
无需改动，也保持了 §4.9 UB-013 的 `R-0010` 兜底路径。

**锁定点** = `codegen::tests::the_first_goto_in_a_node_ends_it`（断言 `goto("done")` → `return;` →
`goto(+1)` 的**先后次序**，而不是只断言三条 goto 的目标常量存在）与
`cli::tests::run_executes_the_tour_and_exits_zero`（真执行、真退出码）。

## D-35 §13.4 的 `infy run` 有三处规范没写：自身输出与程序输出怎么分、退出码怎么映射、`--arg`/`--feature` 指向什么

§13.4 只给了一行 `infy run [mod|src.infy] [--arg K=V] [--feature f]  开发态：build 到临时目录 + 立即执行`。

**(a) 输出分流（规范未定）**：`infy build` 的状态行一直打在自己的 stdout 上，而"立即执行"必然让
被跑程序的 stdout 也进同一个流。选定实现：**阶段 [1]-[9] 的构建信息留在 stdout（与 `infy build`
逐字相同，不改已有声称），`run` 自己的状态行（`run ok — 程序退出码 0（stdout N 字节 / stderr M 字节）`）
打 stderr，程序的 stdout/stderr 用 `write_all` **逐字节转发**、不加前缀不改行尾**。这样程序输出仍然是
可 pipe 的原始字节，而构建噪声与 run 结论都在 stderr 上可分离。

**(b) 退出码映射（§13.4 只定义了 `infy` 自己的 7 档）**：
- 子进程正常退出 → **原样转发**它的码（INFY 产物的 `main` 只会给 0 或 §13.4 的 3，见 `infy_runtime_start`
  → `infy_sched_run_until_idle()` 的 `n_failed ? 3 : 0`）；
- 子进程**没有**退出码（被信号/异常终止）→ 记 **3**。规范没有"异常终止"这一档，3 = 运行时失败是
  最贴近的既有码，且不新造数字；
- 产物存在但**启动失败** → **6**（§13.4 的工具链/链接一类）；
- 临时构建目录创建失败 → **4**（§13.4 的权限/缓存写入错误）。

**(c) `--arg K=V` / `--feature f`：没有任何消费者，显式拒绝（exit 1）而不是静默忽略**。
`env` 的字段集合在 §4.6 是**封闭**的且不含运行参数（明令"不得新增 `env` 字段"），所以 `--arg` 无处可放；
`[features]` 虽然有 §2.2 的约束求值（`pkg::constraint_matches` 收 `features` 形参），但 CLI 从来没把
命令行的 feature 接进 `load()`/`Manifest`，`infy build`/`check` 也不收这两个开关 —— 在 `run` 上单独
发明一条 feature 通道会造成三命令不一致（同类问题已登记 F5）。按硬约束"不实现的功能必须显式报错"
处理，登记为 BACKLOG **F19**。

**锁定点** = `cli::tests::run_executes_the_tour_and_exits_zero`（退出码 0 + 程序流逐字节）、
`cli::tests::run_forwards_the_program_streams_and_a_runtime_panic_maps_to_exit_3`（§13.4 的 3 与
§9.2 报告首行 `runtime[R-0001]: 整数溢出（\`overflow=panic\`）`）、
`cli::tests::run_refuses_flags_it_cannot_honour`。

## D-36 §5.9 的 `infy test` 有四处规范没写：入口怎么被替换、"通过"怎么判、panic 把套件带到哪、harness 往哪打

§5.9 只给了三行（测试文件不进普通构建、`package foo_test` 只能经 `infy test` 编译、
"`#[test]`/`#[bench]` 标记 + `infy test` = build + 执行，退出码沿用 §13.4"），§13.4 只给了
`infy test [pkg] [--bench] [--cov]` 一行。以下四点规范都没定义，选定实现如下。

**(a) 入口被 harness 替换，§7.4 的入口规则在 test 构建里不适用**。`codegen::Config.test_harness`
为真时 `emit_driver()` 直接走 `emit_test_driver()`，`main` 依次做：注册链表（若该 module 有链）→
`infy_run_package_inits(0, 0u)` → 逐个调用 harness 的 `#[test]` 函数 → 打总结行 → 返回。
**不调 `infy_runtime_start`**：调度器不参与，所以 `#![entry_chain]` 指向的链在 `infy test` 里一行节点都不跑。
`cli::resolve_entry` 的 `E-0600`（"产物无入口"）在 test 构建里跳过 —— 它的入口是 harness，
不是链也不是 `fn main`。锁点 = `cli::tests::test_refuses_cov_and_runs_zero_tests_where_there_are_none`
（对 `examples/tour` 跑 `infy test`：程序 stdout 恰为 `test result: 0 passed, 0 failed, 0 total\n`，
即"链没被启动"本身也被逐字节钉住，而不是靠"没有输出"反推）。

**(b) "通过"的判据只认两种签名**：`fn` 无返回 = 调用返回即通过；返回 `Result<…>` = 取 `.is_ok`
（§14.7 的样例 `-> Result<(), test::Fail>` 用的就是这一族，配合 `?`）。其它返回类型
（`-> u32`、`-> bool`）**报 `CgError` 而不是猜一个判据**；带参数的 `#[test]` 同样拒绝
（§5.9 没有参数注入机制，`env` 的字段集按 §4.6 是封闭的）。`#[bench]` 只有 `--bench` 才进运行集，
而 §14.7 的基准签名 `fn(b: &mut test::Bench)` 依赖 `std/test`，§10 的 std 签名表里**没有任何
`std.test` 条目** —— 登记为 **F21**，拒绝消息里点名这一条。
锁点 = `codegen::tests::harness_refuses_tests_it_cannot_call_or_judge`、
`codegen::tests::bench_functions_enter_the_suite_only_under_bench`。

**(c) panic 中止整条套件，退出码 §13.4 的 3**。`#[test]` 是从 `main` 里直接调用的普通函数，
不在任何焦点内，§4.16 的 catch 点（`infy_rt.c` 的 `infy_tls_try`）因此不存在：
`infy_panic` 走完 `infy_report` → `infy_cleanup_drain` → `process_radius_exit()` → `exit(3)`。
**没有改成"每个测试套一层 catch"**：那要把内部入口 `infy_rt_catch`（`infy_rt_internal.h:146`）
提升进冻结公共头，属于 ABI 变更、要四条 C 链同批改，本轮按 C4 的同一原则（能不动 C 侧就不动）
先把行为如实实现并钉住：panic 之后的测试**不运行**、总结行**不打**，§9.2 报告逐字节转发在 stderr。
锁点 = `cli::tests::a_panic_inside_a_test_is_exit_3_and_stops_the_suite`（断言 `a_first ... ok` 在、
`z_after` 不在，即"次序"而不是"存在"）。

**(d) harness 的报告打在自己的 stdout，用 C 的 `fputs`**。§7.7.1 第 4 项把 **INFY 的 `print()`** 判给
日志汇（stderr），而本实现里没有任何 INFY 构造能写 stdout（**F19**）；harness 不是 INFY 程序构造，
是编译器生成的 C，所以它自己开一条通道，不冒充 `print()` 的语义。名字走 `fputs("test <pkg>::<fn> ... ", stdout)`
的字面量而非格式串（§2.3 允许非 ASCII 标识符，不能让标识符落到 `printf` 的格式参数位上）。
CLI 侧沿用 D-35：构建信息在 stdout、`test ok/FAILED` 状态行在 stderr、程序两条流逐字节转发。
C 运行时是文本模式流，所以报告的行尾在 Windows 上是 `\r\n` —— 测试按 `\r\n`→`\n` 归一后**逐字节**比较
（与 C4 的 §9.2 比较同一手法）。

**(e) `--cov` 显式拒绝（exit 1）**：§12.4 要求覆盖率来自 `-O0` 构建的节点表插桩，阶段 [7]-[9] 都不产出
插桩，`--cov` 没有消费者。不静默忽略、更不假报一个覆盖率 —— 登记为 **F22**。
锁点 = `cli::tests::test_refuses_cov_and_runs_zero_tests_where_there_are_none`。

**(f) test 产物的 `.inf meta` 入口字段仍写 §7.4 的 `fn main`/首条链**（冻结容器只有 `ENTRY_MAIN`/链两档，
没有"harness"这一档），所以 `infy artifact info` 对 test 产物打出的"入口"与实际执行的 harness 不符 ——
登记为 **F23**，不改布局也不改声称。

**锁定点**（全部为本轮实测）：`cli` 的 4 条新测试 + `codegen::tests::{harness_runs_each_test_in_sorted_order_and_judges_result_by_is_ok,
harness_refuses_tests_it_cannot_call_or_judge, bench_functions_enter_the_suite_only_under_bench}`。

## D-37 §5.9 的"测试文件不进普通构建"从未接线（`pkg` 里算好的 `is_test`/约束求值是死代码），接线时顺带定了三处

**现状核对**：`pkg::PackageFile.is_test`、`Package.is_test_pkg`、`PackageFile.constraint` 三个字段
早就由 `discover()` 填好，`pkg::constraint_matches()` 也早就存在，但**全仓库没有一个调用点**
（`grep -rn "constraint_matches\|unknown_constraint_names" src/*.rs` 只命中定义处与本轮新增处）
—— 也就是说 §2.2/§5.9 的文件选择在 `infy check|build|run` 上根本没生效：任何 `*_test.infy`
都进了普通构建。`src/lexer.rs:2187` 的 `lex_file_with_features()`（唯一会打 `E-0012` 的词法入口）
同样是死代码，所以 §2.2 的"未知约束名 → `E-0012`"在 CLI 上从不触发。

**接线的选定实现**（`pkg::BuildConfig` + `pkg::file_selected`，由 `cli::load_scoped` 调用）：
- §5.9 的门：`f.is_test || pkg.is_test_pkg` ⇒ 只在 `BuildConfig.test`（= `infy test`）为真时入选；
- §2.2 的门：文件自己的 `//infy:build <表达式>` 对 `BuildConfig` 求值。两道门都要过。
- `E-0012` 的**未知名检查放在选择之前、对每个文件做**（不是只对被编译的文件）：否则
  `//infy:build riscv` 会让文件被静默排除，把拼错的约束名变成"这个文件从来没存在过"。
  代价是该路径的诊断落在 `file:1:1`（module 层，同 `E-0501`/`E-0090` 的既有约定），
  而 `lex_file_with_features` 那条带 span 的路径本轮**不再调用**，避免同一文件打两遍 `E-0012`。
  锁点 = `cli::tests::build_constraints_gate_the_file_set_from_the_cli`。

**(a) §2.2 的约束名全集漏了自己用到的两个名字**：line 96 的封闭集里没有 `ignore`，也没有 `test`，
而 §2.2 的样例写 `//infy:build ignore`、§14.7 写 `//infy:build test`。选定实现把两者收进
`pkg::BASE_CONSTRAINTS`，并让 `lexer::known_constraint_names()` 直接返回该常量（此前是两份手抄清单，
同族问题见 `docs/spec-deviations.md` D-32 的"第二份真相"）。求值规则：`ignore` 恒假（任何构建都排除）、
`test` 取 `BuildConfig.test`。锁点 = `pkg::tests::constraint_expression_evaluates_the_build_config`。

**(b) 约束表达式解析失败没有对应码**：§2.2 只发了 `E-0011`（位置）/`E-0012`（未知名）/`E-0013`（重复）/
`W-0003`（恒假）。`//infy:build linux &&` 这类不完整表达式不属于其中任何一条。选定实现**借用 `E-0012`**
（同一族的"这条约束写错了"）并把解析器的原文放进 note，**且绝不因为解析失败就把文件收进来**。
`W-0003`（"约束恒 false"）本轮**没有实现**：它要求对约束做静态不可满足性判定，而实现只对当前
`BuildConfig` 求值 —— 登记为 **F24**。

**(c) §5.1 R1"包名必须等于目录末段名"与 §5.9/§14.1 冲突，按后两条开例外**：§5.9 明写"目录内可存在
`package foo` 与 `package foo_test` 两种测试文件"，§14.1/§14.7 又是 `tests/build_test.infy` =
`package build_test`（目录末段名是 `tests`）。R1 按字面执行会把规范自己的两种形态都判死。
选定实现：`_test.infy` 文件额外允许 `package <目录>_test` 与 `package <文件名主干>` 两种包名；
目录级 R2 检查改为看**不同包名的集合**，只允许 `{<目录>, <目录>_test}` 这一对并存（`tests/` 里两个
不同主干名仍是违规）。锁点 = `pkg::tests::discover_accepts_the_package_shapes_the_spec_shows`、
`pkg::tests::discover_reports_genuinely_two_package_directories`。

**顺带修掉的两个实现缺陷（不是规范歧义，记在 `docs/status.md` 的 C5 一节）**：
1. R2 的判据此前写在反的地方 —— `if names.insert(pkg_name).is_some() { continue; }` 把
   **同名第二个文件**当作违规：`core/pack/mod.infy` + `core/pack/second.infy`（`package pack` × 2，
   即 §14.1 的 `mod.infy`+`cache.infy` 形态）会被**静默丢掉一个文件**并打
   `E-0501 内存在两个包名`。本轮用一次性探针实测确认（`infy check` 在合法 module 上 exit 2）。
2. `discover_into` 直接用 `read_dir` 的顺序，而该顺序决定 `FileId` 编号、进而决定发射的 C 文本与
   build-id —— §7.3 的可复现承诺因此依赖文件系统。现在 `files`/`subdirs` 都按名字排序。
   锁点 = `pkg::tests::discover_orders_files_by_name`。

## D-38 D4 裁决：Rust↔C 的 IFBC 在**三层**都不一致，容器/表形态以 C 侧文档为基准、指令模型以附录 D 的家族编号为基准

**现象（D4 轮的全面审计，范围大于 BACKLOG 原描述的四条操作数差异）**：Rust（`src/ifbc.rs`）与 C
（`runtime/infy_ifbc.c` + `runtime/IFBC.md`）各自实现了同一句 §7.8.1 容器式，但两边**从未交换过一字节**
（`.inf ifbc` 恒空 = D1 未落地的直接后果），因此差异直到静态审计才现形，且不止 BACKLOG 列的那几条：

1. **容器层**：Rust 给每个区加 `byte_len:u32 || body` 包装（`src/ifbc.rs:16-21` 自称"规范洞"）；
   C 侧按 `IFBC.md` §1"各区自定界（前导 `u32 count` / `code` 前导 `u32 byte_len`）"顺序扫描，
   **没有也不需要一个外层 len**（`code` 的长度就是它自己的前导 u32，Rust 注释里"不加包就分不出 code 与
   line_map"的理由不成立）。`docs/internals/runtime-abi.md` §9 抄的是 C 侧形态。
2. **表形态层**：
   - `type_table`：Rust 每行 `{kind u8, name u32, a u32, b u32}`（kind 用**自家 `TyKind` 0..29**），
     C 每行 `{name_str,kind,flags,size,align,nfields}×u32 + nfields×{u32×3}`（kind 是 D.9 布局语义）；
   - `import_table`：Rust `{name,kind(ifbc 自有枚举),ifabi u16,ret_ty,params:Vec<u32>,sig_hash}`，
     C `{name_str,kind u8,nparams u8,ret_sig u8,flags u8,psigs:nparams×u8}`；
   - `func_table`：Rust 36 字节/条（8×u32 + frame/flags + u16），C 40 字节/条（10×u32），
     且 C 在 `n_funcs` 后多一个 `u64 max_mem_bytes`（§7.9 的单元级配额，规范有据，Rust 没有）；
   - 区段顺序：Rust `… func_table | code | line_map | exception_table …`，C `… import | func | code |
     line_map | exception …` —— 规范 §7.8.1 的式子是 `header|type|import|func|code|line|exc|pool`，
     两边其实都合规，差异只在包装与记录形态。
3. **指令层**：D.3 给每个**家族**编号（`0x20 add`…），并要求类型后缀是"独立 opcode"；32 槽装不下
   家族×宽度全集，两侧从同一规范洞出发选了相反的路 —— Rust 走"家族字节 + 操作数 0 = 类型索引"
   （`DEV-IFBC-1`），C 走"具体字节表（i64/f64 各占一字节）+ 0 操作数"。连带四条 BACKLOG 点名的差异
   （`push` 2vs1、`box.dyn` 1vs0、`guard.*` 1vs0、`ldw` 1vs2），以及 Rust 未察觉的更大冲突：
   **算术块 0x20..0x3F 的字节语义整体错位**、`guard.nonzero/shift` 字节（Rust 0x27/0x2F vs C 0x0C/0x0D）、
   `field_off idx`（D.9 记 1 操作数，C 实现 2 = `ty,field`，Rust 1）。

**规范级新发现（登记为 D-39）**：§7.8.1 line 1235 把"**固定宽度指令**"列为规范性要求，而 D.4 的
`0x43 switch n, [targets]` 与"每个基本块入口栈高度唯一"（line 1254）的语义**必需**内联变长目标表
（C 按 `1+4(1+n)` 解码，`IFBC.md` §3 称其"长度静态可算"）；line 1233 自己写的 `0..3 个 u32 operand`
同样与 D.4 冲突。以 D.4 为准（具体表随附录 D 发布，line 32/2300；§7.8.2 只是主干），"固定宽度"读作
"**长度静态可算**"。锁定点随实现落地（Rust 侧 switch 解禁 + C 侧既有解码器的边界表）。

**裁决（唯一线格式 = 合并形态，两侧各自向它对齐；本节是文档裁决，两侧实现是 D4 余项）**：
- (a) **容器与表形态以 C 侧文档形状为基准**（`runtime/IFBC.md` §1 + `runtime-abi.md` §9 + 冻结头的
  既有约定），Rust 删区包装、表记录改为下述合并形状：
  - `type_table`：`n u32` + 每行 `{name_str u32, kind u32, flags u32, size u32, align u32,
    nfields u32, payload_a u32, payload_b u32}` + `nfields×{name_str,off,ty}×u32`。
    `kind` 的取值空间统一到 **IFABI §2 标量码**（`infy_ifbc.c:54` 的 `SIG_*`：void0 unit1 bool2
    char3 i8 4 i16 5 i32 6 i64 7 u8 8 u16 9 u32 10 u64 11 f32 12 f64 13 str14 bytes15 ptr16 dyn17），
    Rust 的 `TyKind` 重编号对齐，扩展 kind 从 18 起
    （list/tuple/dict/func/coro/instance/ref/box/rc/arc/option/result/never/usize/isize）；
    `payload_a/b` 收 Rust 的 list/dict 元素与泛型实参（C 侧忽略，非新语义），`flags` bit0 = `#[repr(C)]`
    （D.9 的 `field_off` 判据，有据）。前 **18** 行由构建器自动置为标量行，令"类型索引 0..17 == 标量码"
    恒成立 —— 这是让 C 的 `psigs:u8` 与 Rust 的 `params:Vec<u32>` 能引用同一空间的前提。
  - `import_table`：C 形状 + 尾部 `sig_hash u32`，即每行 `{name_str u32, kind u8, nparams u8,
    ret_sig u8, flags u8, sig_hash u32, psigs:nparams×u8}`。`kind` 取值统一为 **host=0 embed=1 event=2**
    （D.5 的 `call.host idx`/`call.embed idx` 与 §7.6 的 event 中转都需要它；Rust 的
    `ImportKind::{PyModule,Infib,Entry}` 折叠掉 —— 源语言在 header `lang_tag` 里，`entry` 由
    `FuncEntry.flags` 的 `F_ENTRY` 表达，二者都不需要 import 行）。`ret_sig/psigs` 是标量码（= 前 18 行
    的类型索引），`sig_hash` 是校验项 5 的快速比对值，文本仍可由 `name_str`+标量码重建。
  - `func_table`：**48 字节/条** = `{code_off, code_len, locals, max_stack, nargs, ret_arity, ret_sig,
    name_str, sig_str, sig_hash, flags u32, frame_kind u32}`；`n_funcs u32` 之后接
    `max_mem_bytes u64`（C 已有，Rust 新增模块字段）。`sig_str` = 签名文本的池索引（`infy doc` 与
    诊断需要，文本是真相、`sig_hash` 是快路径），`ret_sig` = 标量码。C 读取自身形状的 10 个字段的
    **子集**，无需改动语义；Rust 的 `arity/ret_ty/sig` 分别并入 `nargs/ret_sig/sig_str`。
  - `line_map`/`exception_table`/`string_pool`：两侧已经逐字节一致，未合并。
- (b) **指令模型以附录 D 的家族编号为基准**（D.3 的字节就是家族首字节），C 侧向 Rust 侧对齐：
  `0x20 add / 0x21 sub / 0x22 mul / 0x23 div / 0x24 rem / 0x25 neg / 0x26 abs / 0x27 guard.nonzero /
  0x28 and / 0x29 or / 0x2a xor / 0x2b not / 0x2c shl / 0x2d shr / 0x2e shr.a / 0x2f guard.shift /
  0x30 cmplt / 0x31 cmple / 0x32 cmpgt / 0x33 cmpge / 0x34 cmpeq / 0x35 cmpne / 0x36 cmp`，
  **操作数 0 = 类型索引**，C 的 `exec` 按 `types[ty].kind` 选 i64/f64 路径、其余 kind → `R-0706`
  （原"具体字节表"的能力限制原样迁移为"家族 + kind 检查"，表达力不减）。C 的
  `add.f64..cmpeq.f64`（0x35..0x3D）与 `0x3E/0x3F` 让位给家族编号。读法依据：D.3 的"无后缀版本非法 /
  运行时不看类型"禁止的是**运行期按值的类型重新解释语义**（无类型化模式），家族字节 + 编译期类型索引仍是
  静态定型的指令，校验项 3 逐条静态查 kind —— 这正是 D.3 校验栏（"div/rem 必须紧邻 guard.nonzero"、
  "shl/shr 必须紧邻 guard.shift"）在字节面上唯一可校验的形态。
- (c) `guard.nonzero`/`guard.shift` = **0 操作数**、字节 0x27/0x2F（D.3 的邻接校验只看前一字节，类型由
  被守护指令自己的操作数 0 携带）。Rust 去掉 `ty` 操作数，C 的 0x0C/0x0D 迁入。
- (d) `push k,ty` = **2 操作数**（D.2 的 `push k` 只推 `string_pool[k]` 内联常量；B 轨常量带类型，
  `IFBC.md` 的 `push`（DY_STR）单操作数装不下 int/float/bytes 常量。类型索引不新增字节面，与 (b) 同理。
  C 增操作数，Rust 保持）。
- (e) `ldw i,ty` = **2 操作数**（D.2 的校验栏要求类型注解可校验，注解必须在字节面上；C 的执行本来就不读
  它，其语义钉死为"静态类型注解，不参与执行"。Rust 补第二操作数，字节沿用 C 的 0x03）。
- (f) `box.dyn` = **0 操作数**（D.7 的装箱是恒等 —— B 轨值本来就是 `InfyDyn`，类型不需要操作数；
  Rust 去掉 `ty`，与 C 一致）。`unbox.dyn` 两侧本已一致。
- (g) `field_off idx` = **2 操作数**（`ty,field`，D.9 只给了 `idx` 一个名字的表漏了字段索引这个必然操作数，
  C 的校验要用 `types[ty].nfields` 与 `fields[f].off`；Rust 补第 2 操作数，规范漏项随 D-39 一并记录）。
- (h) `switch` 按 D.4 为**变长内联表**：`switch n` 后随 `n×u32` 目标，长度 `1+4(1+n)` 静态可算；
  Rust 的 `SwitchNotSupported` 拒绝解禁（校验器与线性扫描按此宽度处理），落点 = Rust 侧新增边界跳过。
- (i) 间隙操作（Rust 已有、C 未实现，字节沿用 Rust 的 D 间隙分配：`0x0E ldg/0x0F stg` —— 注意 Rust
  现用 0x0C/0x0D，随 (c) 一并迁走、`0x68 import/0x69 import.attr`、`0x84..0x87 seq.*`、
  `0x8B..0x8F inst.get/inst.set/map.insert/str.concat/seq.concat`、`0x92 new.tuple/0x93 new.set`、
  `0xA5 dyn.truthy/0xA6 dyn.identity`）**由 C 侧补实现**：§7.7.6 明言"B 轨的代价在对象模型"，
  而 §7.7.2 的能力扫描（`IMPORT_NAME` 等）必需这些指令；B 轨没有它们就只能把一切动态形状拒之门外。
  Rust 的 `DEV-IFBC-6/7/12` 字节分配由本裁决转正为唯一表。
**未裁决事项（不许被当已完成）**：两侧实现均未动 —— 本条只是把"哪侧改什么"钉死。D4 余项 =
按 (a) 改 Rust（容器+三表编解码+`TyKind` 重编号+lowering 调用点）、按 (b)–(i) 改 C
（OPS 表/exec/校验/dumper/82 断言 harness 同批改）、双侧交叉验证（同一镜像 Rust 反汇编 == C 反汇编，
逐指令）与 D1 的装载验收。**`.inf ifbc` 恒空（D1）期间两侧永不在同一字节上相遇，本条不引入回归面。**

## D-39 规范自身的两处指令面矛盾（D-38 审计时发现）：`switch` 的变长表 vs "固定宽度"规范性句；D.9 `field_off idx` 漏操作数

**(a) "固定宽度指令"与 D.4 冲突**：§7.8.1 line 1233 写指令编码为"1 字节 opcode + **0..3 个 u32
operand**（固定宽度…）"，line 1235 更把"**固定宽度指令**"列为规范性要求（理由：校验器 O(n) 单遍、
不回溯）；但 D.4 的 `0x43 switch n, [targets]` 必需一个**内联的 n 项目标表** —— 若硬塞进 0..3 操作数，
`switch` 最多 2 个 case，规范样例级别的 `match` 都编不出来；若目标表放指令流外，D.4 的写法
（targets 紧跟 `n`）与"跳转目标必须落在指令边界"（D.1 校验项 1）就互相拆台。
裁决 = 以 D.4 为准（line 32/2300 承诺"完整表随附录 D 发布"，附录 D 是指令面的权威；§7.8.1 的"固定宽度"
读作"**长度静态可算**"，switch 占 `1+4(1+n)` 字节，仍是单遍前向扫描，规范性理由不被破坏）。
C 侧已如此实现（`insn_boundaries`/`OPS[OP_SWITCH].special = SP_SWITCH`），Rust 侧本轮解禁（此前
`CodeBuilder::emit` 对 `Switch` 直接报 `SwitchNotSupported`，是"以拒绝冒充合规"的形态）。
同族的 D.9 `0xe3 field_off idx`：字段索引是这个指令的**必然操作数**（校验与执行都要 `types[ty].nfields`
和 `fields[f].off`），D.9 的助记符列漏了它 —— 以 C 的 2 操作数形态（`ty,field`）为准，Rust 补齐。

## D-40 D4 实现段 (1)（Rust 侧）落地时的补充裁定：D-38 未覆盖的 5 处操作数形状 + 4 处实现选择

D-38 把"哪侧改什么"钉到 (a)–(i)，但审计是按**它列出的那些差异**做的；动手时又遇到 5 处
两侧操作数形状不一致、D-38 没点名的指令。按协议（"发现的形状缺口若 D-38 未覆盖，先补 D-38
再动代码，不许两侧各自即兴"）在这里补齐，判据与 D-38 同一条：**附录 D 的家族/助记符列本身
就有漏项**（D-39 已两次实证：`switch` 的目标表、`field_off` 的字段索引），所以操作数以
"校验与执行是否必需"定，不靠助记符列的字面个数。

| 指令 | Rust（D-38 前） | C（`infy_ifbc.c`） | 裁定 | 依据 |
|---|---|---|---|---|
| `new.list` | 2（`ty,count`） | 1（`ty`，push 空容器） | **2**：`elem_ty,count`，装载时弹 `count` 个；`count==0` 就是 C 现在的"空容器" | 栈形态必须可判定：Python 的 `BUILD_LIST` 把元素先压在栈上，1 操作数形态无法表达，只能把 lowering 改成"先建空容器再 `seq.push`"（那是把 B 轨 lowering 拖进容器协议细节，且 `seq.push` 在 C 侧还不存在） |
| `new.dict` | 3（`kt,vt,count`） | 2（`kt,vt`，push 空） | **3**：同上，弹 `2*count` | 同 `new.list` |
| `new.frame` | 0 | 1（`f`，func 索引） | **1**：`f` | 协程帧必须指名是哪个函数，0 操作数的 `new.frame` 无法执行；C 的形态可执行，Rust 补齐（D.6 的助记符列没写操作数） |
| `typeof.idx` | 0 | 1（`T`，类型索引，且装载期做越界校验） | **1**：`T` | D.9 同一行的 `size_of idx`/`align_of idx` 都带 `idx`，`typeof.idx` 单独不带是抄漏；C 的 `check_type` 已把它当类型索引用 |
| `call.virt` | 1（`imm`） | 1（`idx < n_types`） | 个数不变，**kind 改为类型索引** | 两侧个数本就相同，是"这个 u32 是什么"的分歧；C 按 `n_types` 越界校验，Rust 跟同一判据（影响反汇编可读性与校验路径） |

实现选择（规范未定 → 选一个确定实现）：

1. **自动置的标量行 `size/align = 0`**。D-38 只说"前 18 行由构建器自动置为标量行"，没给这些行的
   布局数。仓库里唯一有据的布局来源是 `docs/internals/runtime-abi.md` §1 + `src/hir.rs` 的
   gcc 校准探针，而那份表没有 `bytes`/`dyn` 的 `sizeof`（`InfyDyn` 是引用计数装箱，其尺寸在冻结头
   里、没被任何探针量过）。写一个没量过的数进产物 = 本仓库明令禁止的"编造常数"（B1 轮抓到过两个），
   所以留 0 并写明：**要用 `size_of.idx`/`align_of.idx`/`field_off` 的生产者必须自己填真布局**。
2. **`ImportEntry::ifabi` 从线上删除**，签名域校验改用模块头 `ifabi` 与 `HostSigTable::ifabi`
   相等（D-38 (a) 给的合并行形状里根本没有 `ifabi`；头里那一处已是唯一真相）。原先那条
   "每行 `ifabi` 必须等于工具链的"检查没有失效，只是换了它本来就该看的量。
3. **签名文本/摘要按折叠后的标量码算**（`(dyn, dyn) -> dyn` 而不是 `(list, list) -> list`）。
   `sig_hash` 是校验项 5 的快路径，必须覆盖线上真有的字节；`List`/`Dict`/… 在 IFABI 线上就是
   装箱 `dyn`（§2 的封闭集 `>17` 不可表达，`R-0708`）。
4. **`max_stack` 由 `CodeBuilder` 的运行栈高峰值给出**，`call.*` 只按 opcode 隐含效果计入
   （构建器不知道被调元数）⇒ 这是**保守上界**，永不低于装载器自己算出的高度，因此不会造成
   C 侧假失败；代价是可能多分配几个栈槽（C 的 `stack_cap = max_stack`）。
5. `switch` 的目标数上限取装载器的 `OPS_MAX * 16 = 48`（`MAX_SWITCH_CASES`），发射期与解码期都拒
   超出者：解码不照抄这条，两侧对同一镜像的"能不能线性扫描"结论就不同。

登记为后续项、**本轮不动语义**：`new.set ty` 没有元素个数的落点，而 `effect()` 把它的类型索引
当弹出个数用（BACKLOG **F25**）；本侧装载期校验拒绝 `call.virt`/`call.embed`/`invoke.dynamic`
而 C 侧校验并执行，且 `coro.resume`/`coro.throw`/`throw.inf` 的栈效果与 C 的 `sim_delta` 不同
（BACKLOG **F26**）。D4 的第 (3) 步只交叉验证反汇编，这两条不挡 D4，但挡"两侧校验结论等价"这种
更强的声称，所以写在这里不许被当成已完成。

## D-41 D4 实现段 (2)（C 侧）落地时的实现选择：`ldg/stg` 落在帧的局部窗口、六条指令装载通过而执行显式拒绝

D-38 (b)–(i) 与 D-40 把"哪侧改什么"钉死了，但没有（也无法）规定 C 侧**在没有载体时怎么办**。本轮把
`runtime/infy_ifbc.c` 对齐到唯一线格式时遇到六处规范未定，按"未实现必须显式、不许假成功"选定：

1. **`ldg i` / `stg i`（0x0E/0x0F）寻址帧的局部窗口**。两侧对这个操作数的判据是同一条
   （`operand_kinds` → `OperandKind::Slot` → `i < func.locals`，`check_type` 同），而这个 VM 没有
   模块级存储 —— 那个数除局部窗口外没有别的东西可以指。执行形态与 `ldl`/`stl` 逐指令相同
   （`case OP_LDL: case OP_LDG:` 共用一段），所以两侧对同一镜像的"能不能装载 + 高度"结论一致。
   代价：**跨调用可写的模块级全局在这份实现里不存在**；B 轨若真需要它，要加的是运行时的新入口
   （`InfyIfbcModule` 级的槽数组）+ 新的判据，而不是把这两个字节另作解释。
2. **`import.attr` / `inst.get` / `inst.set` / `seq.iter` / `seq.next` / `new.set` 装载通过、执行显式
   拒绝（R-0710）**。`InfyDyn`（`infy_rt.h` 的冻结 union）没有属性表、没有迭代器、没有集合载体：
   DY_INSTANCE 只带标签。D-38 (i) 要的是这些字节**能被解码、能被 §7.7.2 的能力扫描数出来**，
   所以 OPS/校验/反汇编三处都补全了，执行返回 `R_DYN_TAG` 并**不产出值**
   （锁点 = `test_ifbc.c::test_model_refusal_is_explicit`，它同时断言 `infy_ifbc_load` 仍返回 0，
   即"装载通过"与"执行拒绝"是分开的两件事）。用别的码不行：R-0706 是"操作数类型与后缀不符"，
   而这里操作数合法；R-0710 在本仓库的既有语义就是"dyn 值不是这条指令能承载的形状"
   （`as_int` 失败也报它）。登记为 BACKLOG **F27**（对象模型缺属性表/迭代器/集合载体）。
3. **`import idx`（0x68）执行 = no-op**。它是装载期的能力声明（δ (0,0) 就是共享表给的值），
   没有运行期效果；越界仍在校验项 3 里报 R-0706。
4. **`seq.len` / `dyn.truthy` 是 peek 语义**（δ (0,1)：容器/值留在栈上，长度/真值压在上面）。
   这不是"顺手写成弹出"的理由而是**必须如此**：高度分析与解释器要对同一条指令给出同一个栈形态，
   否则装载期算出的高度与实际执行不一致（R-0705 与假通过都会来）。
5. **`dyn.identity` 比的是指针同一性**，不是 `cmpeq` 的值相等（§4.4 明确区分 `==` 与 `is`）。
6. **家族字节的可执行宽度仍是 i64/f64**。装载期按 Rust 的 `is_numeric` 判据放行所有数值 kind
   （`bool` 只给 `not`），执行期 `fam_path()` 对其它 kind 报 R-0706 —— 这是 D-38 (b)
   "能力限制原样迁移"的字面执行：旧表不给 `add.u8` 分配字节，新表不执行 `add.u8`。
   特别地 **`f32` 被拒绝而不是按 double 算**：静默改变精度比显式失败更坏。

顺带纠正 D-40 选择 5 的一处算术：它写"上限取装载器的 `OPS_MAX * 16 = 48`"，而 C 侧那个表达式实际是
**64**（`OPS_MAX = 4`）。本轮把 C 侧改成显式的 `#define MAX_SWITCH_CASES 48u`，两侧这才同为 48
（锁点 = `test_ifbc.c` 的 `SC_SWITCH_HUGE`：49 个 case 的镜像 C 侧报 R-0704，Rust 侧发射期就拒）。

## D-42 D4 实现段 (3)（双侧交叉验证）：两侧第一次用同一份文本比对，当场抓到三处分叉；同时把"现在还不可比的校验结论"逐条登记

**为什么需要这一条**：D4(1)/(2) 各自把两侧对齐到 D-38/D-40/D-41 裁定的形状，两侧的单测都是绿的，
但**没有任何一条命令把同一份字节交给两个 dumper 比过一次**。本轮加的就是这个：两侧各实现一份
"只含双方都保留的字段、逐位十进制"的规范化文本，写进仓库同一份文件，谁漂谁红。
语法表见 `runtime/IFBC.md` §6.1，两侧入口的分工见 `docs/internals/ifbc-encoding-rust.md` §6。

**锁点**（`.bin` 由 Rust 写入器生成，只有 `INFY_REGENERATE_FIXTURE=1` 重写；`.can.txt`/`.ver.txt`
由 **C 侧**渲染，只有 `INFY_REGENERATE_CANONICAL=1` 重写；Rust 侧 `ifbc::tests::canonical_render_equals_the_c_loader_render`
断言自己读同一份字节的文本 == C 侧那份）：

| 夹具 | 字节 | 钉住的东西 |
| --- | --- | --- |
| `runtime/fixtures/ifbc_rust_writer.bin` + `.can.txt` | 1031 | D4(2) 那份能被执行的镜像，两侧文本相同；且两侧**阻塞性**校验结论都为空 |
| `runtime/fixtures/ifbc_all_ops.bin` + `.can.txt` | 2083 | 附录 D 的**全部** opcode（含 17 条间隙指令、`switch` 的内联 case 表、类型操作数越界时的 `?` 后缀）逐条文本相同 |
| `runtime/fixtures/ifbc_verify_matrix.bin` + `.ver.txt` | 1890 | 每函数一个缺陷、两侧都在同一 `(code, func, off)` 报出来的 10 条：`R-0704 ×2 / R-0705 / R-0706 ×3 / R-0707 ×2 / R-0710 / R-0713` |

**第一次跑就抓到的三处分叉**（前两条改 C 侧，第三条改本侧；都是"规范是唯一权威"的机械执行）：

1. **`coro.resume` / `coro.throw` 的操作数个数**：C 的 `OPS` 表写 0，本侧 `Op::arity()` 写 1。
   D.8 明写 `0xc4 coro.resume h` / `0xc5 coro.throw h` ⇒ 那个 `h` 在线上，**C 侧改为 1**。
   严重性不是排版：指令宽度是 `1 + 4*nops`，C 侧会把 resume 之后的 4 字节操作数当指令流读，
   于是一条协程指令让后面**所有**指令错位（本轮 `ifbc_all_ops.bin` 的 `I` 行立刻分成两截）。
   执行语义不变：被 resume 的值仍从操作数栈取，`h` 是执行不读的注解，与 D-38 (e) 给 `ldw`
   的类型注解同一个处理；装载期与反汇编都按 1 操作数。
2. **D.9 的助记符名**：C 侧与两份文档写 `size_of.idx` / `align_of.idx` / `field_off.idx`，
   规范那行是 `0xe1 size_of idx` `0xe2 align_of idx` `0xe3 field_off idx` —— `.idx` 是**操作数**
   而不是名字的一部分；只有 `typeof.idx` 是一个名字（`0xe0 typeof.idx`）。本侧代码本来就是
   `size_of`/`align_of`/`field_off`/`typeof.idx`，**改 C 侧与文档**。（本侧的镜像文档
   `docs/internals/ifbc-encoding-rust.md` 的表格此前抄的是 C 的错名，一并改正。）
3. **本侧 `verify_func` 的重复报告**：操作数合法性与 D.3/D.7 邻接检查写在"抽象执行"走查里，
   而那条走查把**直线后继**也 `work.push(next)` 回工作栈，于是同一条指令被走 2–3 次、
   同一个 `(code, off)` 报 2–3 次（矩阵里 `div` 缺守卫那条报了 3 次；C 侧按边界各查一次只报 1 次）。
   修法两处：① 操作数/邻接检查搬出走查，按 `instrs` 每指令一次（顺带与本侧原先"只检查可达指令"
   相比更接近 C 侧——不可达指令的非法操作数现在也报）；② 走查不再把直线后继压回工作栈
   （高度仍记入 `height`，分支/跳转目标仍由 `push_block` 压）。
   这**不是**线格式问题，但同样是"两侧对同一镜像的结论不同"，所以放在这里而不是 RUNLOG 就完。

**现在还不可比的校验结论（F26 要的那份具体清单；讲"两侧校验等价"之前必须逐条消掉）**：

* (a) `call.virt` / `call.embed` / `invoke.dynamic`：本侧装载期直接判 `R-0705`（被调元数不可静态
  确定，校验项 2 无法完成），C 侧会校验并执行（`sim_delta` 的 `SP_CALL_HOST` / `SP_INVOKE_DYN`，
  `call.virt` 记 δ=0）⇒ 同一镜像本侧拒绝、C 侧接受。
* (b) `coro.resume` / `coro.throw` / `throw.inf` 的栈效果：本侧 `(0,0)`，C 的 `sim_delta` 是 `-1`
  系列；同一份高度分析两侧结论可以不同。
* (c) **操作数越界的层级不同**：本侧 `decode` 把"引用不存在的行"当**容器**错误
  （`IfbcDecodeErr`，`validate_refs`），C 侧 `parse_sections` 通过、由 `check_type` 报 `R-0706`。
  后果是这类镜像根本进不了"两侧都能读"的集合，所以矩阵里没有它（不是漏测，是集合不相交）。
* (d) 本侧 §7.8.3 第 4 项额外要求"函数最后一条指令必须是 `ret`/`ret.multi`/`throw.inf`"
  （否则 `R-0707`），C 侧没有这条判据。
* (e) `W-0705`（第 6 项，非阻塞）：本侧逐指令报，C 侧每函数报一次 —— 报告粒度不同，
  所以规范化文本只渲染**阻塞性**结论。

因此 D4(3) 的声称边界是：**反汇编逐指令相等（覆盖附录 D 全部 opcode）** + **矩阵里那 6 个 R-码类
的 `(code, func, off)` 逐条相等**；(a)–(e) 之内仍不许讲"两侧装载期校验等价"，`docs/coverage.md`
的"被测对象是生产运行时吗"一行按此写。消掉 (a)(b) 需要本侧的能力表扩容（§7.7.2），登记在
BACKLOG **F26**；(c)(d) 属"本侧更严"，若要合并需与 C 侧同批改并在这一条下追加裁定。

## D-43 D1(1) 轮（B 轨探针 + 能力扫描落地时）：附录 E.3 的三处未写明之处，选定确定实现

**为什么需要这一条**：`btrack.rs` 之前 0 项单测，本轮补到 23 项后一次跑出 10 红，其中三条不是"测试写错"，
而是规范把形态写在**文档注释**里、实现按字面读，两边在字节上没有相遇过。附录 E.3 是能力表的规范式，
§7.7.2 是扫描的规范条款，两者都留了口子。

**(a) `arg:` 派发项的取值域**。E.3 头部（`src/btrack.rs` 的 `CapTable::mapping` 文档与三份表的注释块）
把行写成两层：

```
value := "-" | "ignore" | mnemonic | "arg:" case ("," case)* ("," "else=" value)?
case  := key "->" mnemonic
```

`case` 那行只写了 `mnemonic`，而 shipped 三份表的实际写法是 `arg:+->add._,…,//->-,%->-,**->-,@->-,else->-`
—— 分支值就是 `-`。实现（`parse_single`）照 `case` 那行的字面，把分支值一律送进 `check_mnemonic`，
于是 `else->-` **让整行解析失败**；扫描器把"行解析失败"报成"该 opcode 未映射"，于是
`BINARY_OP`/`CALL`/`COMPARE_OP`/`CONTAINS_OP`/`IS_OP`/`LOAD_ATTR`/`STORE_ATTR` 这七个家族**无论子操作是否被覆盖**
都拒绝构建，`lower_named` 永远进不到发射（py-3.14 表 154 行中 7 行、py-3.13 表 14 行受影响）。
**裁定 = `-` 与 `ignore` 是"值"，在任何位置都合法**（顶层、派发分支、`a|b` 候选表），因为
`value := "-" | …` 已经把 `-` 定义为值，而 `case` 行里的 `mnemonic` 是对值的一句话简称而不是第二套文法。
落点：`parse_single` 先认 `-`/`ignore` 再走 `check_mnemonic`；锁点 =
`btrack::tests::the_mapping_grammar_covers_every_form_the_tables_use`（含 `else->-` 与 `//->-` 两种写法）
与 `every_row_of_a_shipped_table_parses_under_the_grammar`（三份表的 `unparseable` 必须为空）。

**(b) `co_names` 索引的低 1 位在 3.12+ 是标志位**。§7.7.2 扫描项 2 只写 "`co_names` ∩ `LOAD_GLOBAL`"，
没写 CPython 3.12 起 `LOAD_GLOBAL`/`LOAD_ATTR`/`STORE_ATTR` 的 `oparg` 打包了标志位
（`LOAD_GLOBAL`：`oparg = namei*2 + do_call`；`LOAD_ATTR`：`oparg = namei*2 + method`）。
本机 CPython 3.14.6 实测：`input()` → `LOAD_GLOBAL arg=1` 且 `names=['input']`；`x.foo` → `arg=0`、
`x.bar` → `arg=2`、`x.bar()` → `arg=1`。实现按原值 `name_at(arg)` 取 ⇒ 单元里**只有一个**全局名时索引越界、
扫描静默 `continue`（`E-0711` 与 `E-0712 builtin` 两个分支从不触发），有**两个**名字时解析成**邻居**
（`input` 被当成 `eval`，方向反了）。**裁定 = 统一走 `CodeObj::operand_name()`，对这五个 opname 取 `arg >> 1`**，
扫描项 2 与 lowering 的 `LOAD_GLOBAL`/`LOAD_ATTR`/`STORE_ATTR` 共用同一入口（不许两侧各自解包）。
锁点 = `btrack::tests::eval_is_e0711_and_not_a_capability_gap`、
`gaps_inside_nested_code_objects_are_reported_with_their_own_name`（`<lambda>` 里的 `eval` 仍报）、
`a_builtin_the_table_refuses_is_e0712_naming_it`（`input` 不被读成邻居）。

**(c) `IMPORT_NAME`/`IMPORT_FROM` 的参数是 `co_names` 索引，不是 `co_consts` 索引**。
`PyInstr::const_name` 的文档注释自称"服务 `IMPORT_NAME`/`LOAD_ATTR`"，实现却读 `co.const_at(arg)`，
而这两个 opname 的参数都索引 `co_names`（本机实测 `import numpy` → `IMPORT_NAME arg=0`、
`names=['numpy']`、`co_consts=[0, None]`）。后果 = §7.7.2 扫描项 3 从不触发：`import numpy`（deny 段里明写的
原生扩展）的 violations 是**空**；lowering 还把模块名 intern 成空串。**裁定 = 删除 `const_name`，
`IMPORT_NAME`/`IMPORT_FROM` 与属性形式共用 `operand_name`**（`IMPORT_*` 无标志位，取原值）。
锁点 = `btrack::tests::a_denied_import_is_e0712_naming_the_module`（真 dump 的 `import(numpy)` 必须被拒）。

**顺带登记的规范性缺口（不是本条的裁定，见 BACKLOG F28）**：E.3 规定 `[opcode]` 行以数值键 `0xNN` 为
规范式并附 `OPNAME` 别名，但随版本发布的三份表里只有 `IFBC-CAP-py-3.14.toml` 有数值键（154 行），
`IFBC-CAP-py-3.13.toml` 与 `IFBC-CAP-lua-5.4.toml` **一个都没有** ⇒ `check_alias_agreement` 对这两张表永不触发。
本机没有 3.13 / lua 解释器，数值不能凭记忆填（§12.2 的可复核要求，B1 轮"编造常数"那一族的教训），
所以只登记不补数；`the_name_keyed_tables_still_map_every_row` 里留了一条**补上数值键就会变红**的断言，
强迫改动者把表移进数值键测试并消掉 F28。

## D-44 D1(2) 轮（阶段 [6a] 接进 CLI）落地时的八处规范未定，选定确定实现

**为什么需要这一条**：§7.7 把 B 轨的**构建期**行为写成条款，但"接进 `infy check`"之后每一件事都需要一个
确定的错误码、一个确定的取值来源、或一个确定的名字规则，而规范三处都没写。`btrack` 的上半段（探测 +
扫描 + lowering）在 D1(1) 轮已经有 23 项单测，本轮第一次从 CLI 走到它 —— 当场现形的两条实现缺陷
（`unit_code()` 的名称错配、`sema` 把 embed 表达式判成 `()`）写在 (d)(e) 里，因为它们改变了"什么算合法程序"。

**(a) 构建期解释器不在场 → `E-0713`**。§7.1 的阶段注（spec:1051）明写"[6a] 在 `infy check` 下**需要**源语言
工具链在场才能做能力扫描（缺失 → `E-0713`）"，而 §9.4 的 `E-0713` 标题是"字节码版本不匹配"。码表是闭集
（§9.4 共 210 码，且 spec:1508 禁止挪用废止号），所以同一个码承担"版本不符"与"根本不在场"两种语义。
裁定 = 照 §7.1 的字面用 `E-0713`，标题写"未找到 py 工具链（清单声明 3.14）"、help 指 `infy toolchain add py`
（§7.6.2）；落点 = `IngestErr::ToolchainMissing::code()`，`cli::ingest_b_track` 在没有解释器时逐单元发射。

**(b) 前缀未实现 / 参数表不可表达 → `E-0712`**。§9.4 没有"B 轨前缀本实现不降低"或"INFY 侧类型没有 IFABI
标量码"的码。`E-0712`（字节码能力缺失，§7.7.2）是唯一同族的：它的语义就是"这条路要走能力表扩容"。
裁定 = 两处都报 `E-0712` 并在标题里点名具体项（`@lua 的字节码摄取未实现` / `` `widen` 的形参表无法构造`），
诊断里写明 §7.7.1 规则 1 与 `runtime/IFBC.md` §2 的封闭签名集。锁点 =
`cli::tests::a_lua_unit_is_refused_by_name_and_blocks_the_build`、
`cli::tests::a_parameter_with_no_ifabi_row_stops_the_ingest`。
**没有**借用的号：`E-0716`（限进程隔离前缀）、`E-0702`（限白名单之外的前缀，`@lua` 在白名单内）。

**(c) 参数表的取值来源 = 阶段 [4] 推断出的实参表达式类型**。§7.7.1 规则 1 只说"编译器读 INFY 侧声明类型，
生成 IFBC 形参表"，而具名实参的值按 §2.6 可以是任意表达式（`@py(scale=scale*1.0)`）。裁定 =
取 `sema::Program::ty(实参表达式 span)`，即 [4] 给该表达式的类型；返回类型取所在 item 的声明返回类型。
元数据实参名闭集 = `{name, timeout_ms, isolation}`（规范未给 B 轨保留名清单；`name` 是 §7.6.1 的 A 轨配对键、
`timeout_ms` 是规则 3、`isolation` 是 §7.7.5）。单元默认名 = 所在 item 名（规范只写 `name = "…"` 可指定）。
不可映射（`u128`/`i128`/`auto`/未定型）一律拒绝，**不回落 `()`** —— 回落会静默改变 §7.8.3 校验项 5 比较的摘要。

**(d) 实现缺陷（修）：探针被问的单元名与 `wrap_embed_unit` 写入的名字不一致**。
`build_embed_unit` 调 `run_python_probe(python, &src)`，而那个入口固定用 `unit = "__infy_unit__"`；
`wrap_embed_unit` 写的却是 `def <unit_name>(…)`。`CodeObjDump::unit_code()` 在名字匹配不上时
**回落到模块 code object**（它的文档注释把这叫"source is not a wrapper"），于是摄取拿到的是模块包装器的
`MAKE_FUNCTION` 流：单元体里的 opcode / builtin / import **一个都没被扫描**，产出的镜像也不是那个函数。
表现是"任何具名单元都得到一张看似正常的镜像"，而不是报错。
裁定/修法 = `build_embed_unit` 改调 `run_python_probe_named(python, &src, unit_name)`。
锁点 = `btrack::tests::the_declared_table_reaches_the_function_record`（断言 `m.unit == "normalize"` 且函数记录
只有一条 —— 回落路径下这两条都成立不了）与 `cli::tests::check_ingests_a_py_unit_and_keeps_the_declared_table`。

**(e) 实现缺陷（修）：`sema` 把 embed 表达式的类型判成 `()`，令 §7.7.1 自己的样例过不了 `infy check`**。
`check_expr_inner` 的 `ExprKind::Embed` 臂无条件返回 `Ty::Unit`，而 §7.7.1 规则 2 写"单元的 `return` 映射到
IFBC 返回指令"⇒ 该表达式的值就是所在 item 的返回值。于是规范样例
`event normalize(…) -> Vec<f32> { @py(…) { … } }` 在阶段 [4] 就吃 `E-0110 返回值类型不匹配：期望 Vec<f32>，实际 ()`，
阶段 [6a] 从 CLI **永远到不了**（这条挡住的是整条 D1，与 D1(1) 轮那三条挡死扫描的缺陷同族）。
修法 = 该臂返回 `self.ret_ty.clone()`（`check_body` 在检查体之前已按声明设好）。
锁点 = `cli::tests::check_ingests_a_py_unit_and_keeps_the_declared_table`（不带这条修复就是 exit 2）。

**(f) 补丁级声明查不到能力表（修）**。§7.7.3 写"精确到 minor；**patch 不敏感**"，但归一只做在版本比较里：
`py = "3.14.6"` 时 `check_probe_version` 放行、`CapTable::load("py", "3.14.6")` 却去找
`IFBC-CAP-py-3.14.6.toml` → `E-0712 能力表无法读取`。裁定 = 表名按**探针报出的 minor** 取
（`dump.py_minor()`，即真正编译了该单元的那个 minor；§7.7.3 已保证它与声明一致）。
锁点 = `btrack::tests::a_patch_level_declaration_still_finds_the_minor_capability_table`。

**(g) `infy check` 的 stdout 增加一行 `ingest ok —`**。§13.4 只规定 `check` 的退出码，没规定输出格式；
本仓库的既有约定是每个已落地阶段打一行状态（`codegen ok —`、`artifact ok —`）。裁定 = 阶段 [6a] 每份镜像
打一行 `ingest ok — 阶段 [6a] \`<unit>\`：IFBC 镜像 N 字节 / M 函数（CPython 3.14.6，§7.7.1）`，
无 B 轨单元的模块一行不打 ⇒ `examples/tour` 的 stdout 逐字节未变。

**(h) B 轨夹具的环境依赖**。`runtime/fixtures/ifbc_py_unit.bin` 与 D4(2)/D4(3) 那三份夹具方向分工相同
（`.bin` 只有 `INFY_REGENERATE_FIXTURE=1` 能重写、`.can.txt` 只有 **C 侧** `INFY_REGENERATE_CANONICAL=1` 能重写），
差别在它是**探针产物**而不是手写形状 ⇒ 复跑 `btrack::tests::py_unit_fixture_is_exactly_what_the_ingest_produces`
需要本机有发布时那台机器的同一个 CPython（本轮 = 3.14.6）。§7.7.3 本来就把源语言工具链定为**构建期**依赖
（"构建期依赖 ≠ 运行期依赖，与 A 轨需要 C 编译器完全同构"），所以这里按 §12.2 记录而不假装可移植。

## D-45 §7.8.4/§13.4 的 `infy ifbc dump|verify` 有四处规范没写：输入形态、`--canonical` 是哪一份文本、拒绝走哪个退出码、一个区里多个模块怎么分帧

规范给的只有三句：§13.4 列 `infy ifbc dump|verify <artifact>`；§7.8.4"提供 … 用于诊断与合规测试（§12.3）"；
D.10"`infy ifbc verify --all-errors` 可一次列出全部违规（默认只报首个，保持与运行时一致）"。
§12.3 又把 `infy ifbc dump` 列为黄金文件之一。剩下的四处都没定义，本实现的确定选择如下（提交 `D2` 那批）。

**(a) 输入形态：产物与裸镜像都收，`.inf ifbc` 只接受"恰好一个模块"**。
`classify_ifbc` 先按 §7.4 容器读（`artifact::read` 只看文件尾的段尾，D-33 ⇒ 裸镜像绝不会被误判成容器），
容器走 `.inf ifbc` 区；读不出容器再按 IFBC 镜像读（`IFB1` 魔数）。
区非空而 `meta.n_ifbc_modules != 1` ⇒ **点名拒绝**（exit 1）：§7.4 没有为一个区里多个模块定义任何分帧
（无计数前缀、无长度表、无分隔符），两侧读取器也都没实现，本命令不猜一种分隔符去给"区的前缀"发合格证。
F29 落地时必须先裁这条（或由 `n_ifbc_modules` + 逐模块长度表把分帧钉进 §7.4），届时这里从拒绝变成支持。
区为空 ⇒ exit 0 并把"0 个 B 轨模块 / 区域 0 字节"打在输出里，`ifbc_empty_region_text` 另点名 F29 与
"不声称校验过产物里的 B 轨字节码" —— 空区是"没有东西可查"，不是"查过了没问题"。
锁点 = `cli::tests::an_artifact_with_an_empty_ifbc_region_is_reported_as_empty`、
`a_one_module_artifact_is_read_from_the_ifbc_region`（容器路径真读回写入的那一份字节）、
`several_modules_in_one_region_are_refused_by_name`。

**(b) `--canonical` 是 D4(3) 那份双侧共用文本，且此时 stdout 是纯的**。
默认输出 = 人类可读反汇编（`IfbcModule::disassemble`）—— 它**没有**跨语言逐字节锁点，C 侧
`infy_ifbc_dump` 是另一套行格式（D-42），所以只锁"同一份字节两次渲染相同"（§12.3 黄金文件的最小要求）。
`--canonical` = 规范化文本：dump 走 `ifbc::dump_canonical` ↔ C `infy_ifbc_dump_canonical`，
verify 走 `ifbc::verify_canonical` ↔ `infy_ifbc_verify_canonical`（行语法 `H/T/P/F/I/L/X/S` 与 `V`，
见 `runtime/IFBC.md` §6.1 与 `docs/internals/ifbc-encoding-rust.md` §6）。裁定 = canonical 模式下
**不打**表头、状态行与 note（状态行改走 stderr），因为那一份文本的用途就是与 **C 侧**渲染的
`runtime/fixtures/*.can.txt` / `*.ver.txt` 按原始字节 `cmp`。
D4(3) 把这个锁点放在 C harness 与 `ifbc::tests` 里；本轮第一次把它抬到命令行那一层
（`cli::tests::cli_canonical_dump_equals_the_text_the_c_loader_rendered`、
`cli_canonical_verify_equals_the_verdict_text_the_c_loader_rendered`）。
`W-0705` 在 canonical 模式下不渲染（D-42(e)：本侧按指令、C 侧按函数，粒度不同，不是可比文本）。

**(c) 退出码：§7.8.3 的阻断结论记 2，读不出的文件记 7，命令用法错记 1**。
§13.4 的 2 写的是"诊断阻断（E，或 strict 下 W）"，而 §7.8.3/D.10 的结论是 `R-07xx`，规范**没有**给
装载期拒绝分配退出码。选 2 的理由是按角色而非按字面：2 是唯一表达"某项检查说了不通过"的号；
1 与"命令跑不下去"（读不到文件、不认识的子命令、缺参数）不可区分，7 被 §13.4 限定给
§5.8/§7.3 的校验和/可复现性。文件两侧都读不出（既无段尾也无 `IFB1`）⇒ 沿用 `artifact info` 的借用
（`E-0605` + exit 7，D-32），并把两条拒绝理由都打出来。**没有**新增任何 `E-`/`R-` 码（UB-024）。
锁点 = `cli::tests::verify_reports_the_first_finding_by_default_and_all_of_them_with_the_flag`、
`ifbc_refuses_a_file_that_is_neither_and_an_unusable_invocation`。

**(d) host 签名表用 `HostSigTable::std_default()`，逐 build 的表不可得**。
§7.8.3 第 5 项要求 `call.host` 的签名"逐元匹配 `IFABI`"，而 `src/host.rs`（§11 `HostApi`）仍是 1 行占位、
D3 的 A 轨绑定表也还没生成 ⇒ 本命令拿的是 §10 的 std 签名表（与 `ifbc::tests` 和 C harness 同一张）。
后果要说清：`infy ifbc verify` 今天校验的是"镜像对 std 表"，不是"镜像对该产物实际注册的 host 表"。
另按此裁定，`verify` 的结论**只在 `.ver.txt` 那一份矩阵上与 C 侧比过**；对
`runtime/fixtures/ifbc_all_ops.bin` 本侧报 26 条阻断（`call.virt`/`call.embed`/`invoke.dynamic` 一类，
F26 前半），C 侧不报同样的集合 ⇒ 两侧装载期校验等价的这句话仍然不许讲（清单 = D-42 (a)–(e)）。

**(e) `--all-errors` 的粒度**：D.10 说"一次列出全部违规（默认只报首个）"，没定义"首个"是首个**函数**
还是首条**结论**。裁定 = 首个失败函数返回时手上已有的全部结论（`ifbc::verify(.., false)` 的既有语义，
与运行时"首失败项映射 R-码后整体拒绝该模块"一致：模块级拒绝不需要也不应拿到别的函数的结论）。
实测 = 矩阵镜像默认报 1 条（`R-0704 func #0 @ 0x0005`），`--all-errors` 报 10 条。

## D-46 D3 轮（A 轨绑定落地）的六处规范未定/自相矛盾，选定确定实现

**为什么需要这一条**：§7.6 把 A 轨的胶水全部判给编译器（§7.6.3“用户不得手写以下任何一项”），但它只写了
**做什么**（生成头、生成包装、生成 thunk、下链接指令），没写任何一处**怎么做才算确定**。本轮第一次从
`infy build` 走到链接，六条都必须当场定下来；其中 (f) 是本轮撞上的规范自己的矛盾（样例过不了自己的文法判据）。

**(a) `<8hex>` 的规范化算法**。§7.6.3 项 2 与 `docs/internals/runtime-abi.md` §2 只说“`<8hex>` = 签名块规范化
哈希 / the first 4 bytes of `SHA-256` of the normalised `extern` signature block”，没定义“规范化”。裁定 =
`infy-a-track-v1 <单元名>\n` + 成员行 `<name><签名>` 按 `(package, name)` **字典序**、`\n` 连接，取 SHA-256
前 4 字节的小写十六进制。理由：同一个签名块的声明可以写在相邻两行，也可以拆进 `//infy:build` 门后的两个文件
（§5.1 允许一个 package 跨文件），而**声明序**会随“哪个文件先被读到”变化 ⇒ §7.3 的可复现承诺会依赖文件系统。
落点 = `atrack::normalized_block`；锁点 = `the_normalised_block_is_independent_of_declaration_order`
（成对断言：换声明序同哈希、多一个成员必换哈希 —— 后者正是 §7.6.3 项 2 要防的“同名不同签名撞车”）。

**(b) 哈希是块级还是函数级**。规范写“签名块规范化哈希”（块级），而 `src/hir.rs` 早就准备了**逐函数**的
`HirFn::signature_text()`（§11 / ABI §2 那一列，且此前**零调用点**）。裁定 = 哈希按块（规范原文优先），
逐函数签名作为块规范化的成员行，`infy_e_<name>_<8hex>` 的 `<name>` 是函数名 ⇒ **同一块内所有函数共享后缀**。
这条让 `signature_text` 第一次变成活的（`the_normalised_block_uses_the_hir_signature_column`）。

**(c) UB-010 的移交判据落在哪个方向**。§8 的属性表（spec:995）写 `#[owns]/#[borrows('a)]` 适用于
“`extern_decl` **返回/参数**”，而 §7.6.1 的规范样例只在返回裸指针的那条声明上写属性：
`dec_open(path: *const u8, len: u64, err: *mut i32)` 与 `dec_close(h: *mut DecHandle)` 的**形参**都是裸指针而
没有属性。若“参数也算”，规范自己的样例就自犯 `E-0723`。裁定 = 判据只落**返回方向**（含 `Option<指针>`），
参数位置的借出按 §7.6.4 行 2 的“借用仅在调用期间有效”不需要属性；行 2 那句“外部**保存**该指针必须
`#[borrows]` 显式声明生命周期”读的也是这个方向。同时：`#[owns]` 与 `#[borrows]` 并存 = `E-0723`
（规范写的是二选一，并存等于没写）；`#[borrows]` 不带生命周期 = `E-0723`。
锁点 = `a_pointer_returning_declaration_without_ownership_is_e0723` +
`an_explicit_ownership_annotation_is_accepted`（放行的一半，形参裸指针不报）+
`a_void_declaration_needs_no_ownership_attribute`（`dec_close` 的形状）+
`both_ownership_annotations_together_are_still_e0723` + `borrows_without_a_lifetime_is_e0723`。

**(d) 长度的 C 拼写**。§7.6.4 行 2/行 3 给切片与 `&str` 的长度写 `uint64_t`，而 INFY 侧 `usize` 在
`codegen::prim_c` 里已经是 `size_t`。裁定 = 绑定头继续写 `size_t`（§7.2 的每个内建 target 上两者是同一类型；
写 `uint64_t` 会让同一值在 ci.c 与头文件里出现两个类型名，换回来的是 `-Wall` 下的不兼容声明）。
`bool`/`char` 则相反：规范写的 `uint8_t`/32 位枚举与 `infy_rt.h:41-42` 的 `InfyBool`/`InfyChar` 本就是同一类型，
所以照规范写而不是照 typedef 写。锁点 = `the_scalar_rows_map_to_their_c11_spelling`。

**(e) §7.6.4 的表里“规范禁止”与“本实现没做”必须分道**。这一族的码表只有 `E-0723`（跨边界指针移交未声明），
但表里既有**禁止传递**的行（`Rc<T>`、`[T]` 的容量、任何 INFY 分配器的裸内存 → 程序不合规），也有
**允许但还没有发射形态**的行（`&[u8]`/`&str` 的 ptr+len 展开、`Option<&T>` 的 null→`None` thunk、
`Result<T,i32>` 的出参 thunk、`dyn Fn` 回调 ctx、`#[repr(C)]` struct 按值）。裁定：
禁止项在阶段 [4] 发 `E-0723`（`atrack::decl_gaps` 只对 `Boundary::Forbidden` 出码），
未实现的行**不占任何诊断码**，在阶段 [8] 以条款号 `§7.6.4` 指名 `CgError`（与 F15/F19/F29 那一族同形）。
理由：拿“程序不合规”的码去报一条规范允许的签名，是假称不合规 —— 比拒绝更坏的失败，而且它会写进
§12 的合规通过率里。锁点 = `a_legal_signature_without_emission_shape_is_not_an_e0723`（阶段 [4] 必须干净）
+ `codegen::tests::an_unmappable_but_legal_a_track_signature_is_refused_at_build_by_name`（阶段 [8] 必须红）
+ `the_boundary_table_is_enforced_at_stage_four`（`Vec<u8>` 形参 = 禁止项，必须在 [4] 红）。

**(f) `@c` 作为语句要不要 `;`：§3.6 与 §7.6.1 打架，实现此前站在错的一半**。§3.6 明写“一个嵌入块**必须**绑定到
一个 `event`/`fn` 声明体，或作为 `expr_stmt` 出现”，而 §7.6.1（spec:1113）与 §14（spec:2143）的 A 轨样例把
`@c(name = "decoder") { … }` 写成一条**不带 `;`** 的语句。`parser.rs` 的两处“块状表达式”判定
（`block()` 的 1638 与 `stmt()` 的 1843）列了 `If`/`Match`/`Block`/`Unsafe`/`StructLit`/`Closure` 而**没有
`Embed`** ⇒ 规范的样例自己报 `E-0053 语句缺少终结符`，且 span 落在**下一条**语句的开头（本轮第一次写
A 轨端到端测试时撞上，报在 `unsafe { … }` 那一行，看起来像是 `unsafe` 的问题）。裁定 = `Embed` 是花括号
开头的块状表达式，属于 §2.11“不需要终结符”那一档，两处判定同时补上；`;` 形式继续合法。
锁点 = `parser::tests::an_embed_block_statement_needs_no_semicolon`（无 `;` 与有 `;` 两种都断言，并断言 AST
形状：一条 `StmtKind::Expr`、`ExprKind::Embed`、`body` 逐字节原文）。

**同轮登记**：`docs/BACKLOG.md` **F31**（`#[repr(C)]` struct 与 ADT 的不透明指针跨边界：绑定头只能携带标量与
标量指针，于是 §7.6.1 样例自己的 `-> *mut DecHandle` 过不了 —— 规范样例不能构建，与 F30 同族）、
**F32**（§7.6.2 的工具链发现/注册、`recipe.toml` 审计、版本核对 `E-0604`/`--allow-toolchain-drift` 全未接；
A 轨现在只驱动 `$CC`/`--cc` 那一个 `cc`）、**F33**（§7.6.3 项 3 的边界 thunk 只有标量转发、项 5 的外部行号→
INFY 诊断位置未做、§7.6.1 的 `W-0720`（源码导出而签名未声明）与“签名声明而源码未导出 → 链接期 `E-0604`”
没有映射、§7.6.5 反向导出与 `E-0724`、§7.6.6 的 `isolation = "process"` 全未实现）。

## D-47 §12 给了分类、数量与"检查"列，却从未定义**一个用例长什么样**：目录结构、期望的表示法、编号规则、缺工具时算什么，全部由此条确定

§12 全章（spec:1845–1899）只写了三件事：分类表（§12.1/§12.2/§12.5，合计 2300）、§12.3 的产物与构建
硬校验、§12.4 的等价性矩阵，以及"`infy conformance --report` 必须输出**逐分类**通过率"。它没有说
用例是什么形态（单文件还是目录）、期望码怎么写、编号规则（只说了"已有用例的编号与期望不得改变"）、
也没有说**构建期工具不在场时**一条 §7.7 的用例算通过还是算失败——而 §7.7.3 自己把"解释器在场"列为
构建期前提。下列八项是本实现选定的唯一确定形态。

1. **(a) 一个用例 = 一个目录**：`conformance/cases/<分类>/<编号>/`，内含 `case.toml`（期望）与
   源文件。不采用"把期望写进被测源码的注释里"（Rust `compiletest` / CPython 那一流派），因为 §12.1
   的分类里有"注释"（§2.5），§12.5 的 §9.5 分类里明写要检查"注释保留"——期望若以注释承载，
   这两个分类的用例会把自己的被测面吃掉一半。
2. **(b) 分类的真值源是代码不是目录**：`src/conformance.rs::CATEGORIES` 逐字抄三张表
   （`slug`/`table`/`title`/`check`/`required`，其中 `check` 列去掉 markdown 强调符号），
   `CATEGORY.toml` 是它的**渲染产物**（`INFY_REGENERATE_CATEGORIES=1` 才重写，方向分工与 IFBC
   夹具同一族）。加载器两个方向都拦：表里有的分类没有目录 → 报错（空目录靠 `CATEGORY.toml` 存活，
   git 存不了空目录）；目录不在表里 → 报错（分类名拼错会让整目录用例静默消失，正是 §12.5 要防的）。
3. **(c) 期望只比较诊断码的排序多重集 + §13.4 退出码**，不比较位置、不比较 §9.2 的文本。
   位置与文本另有 `diag`/`fmt` 单测钉住；把它们放进用例 = 一处渲染改动红一批无关用例。
   正例必须显式写 `expect = []` —— 空是**声称**，缺字段是**漏**，加载器拒后者。
4. **(d) 编号 = 目录名，只增不复用**，且 `case.toml` 的 `id` 必须等于目录名（不一致 = 加载错误）。
   §12 的"编号不得改变"落在这三条上：移动一个用例=改编号=违规，改名会被拦。
5. **(e) 构建期工具不在场 = 第三种结论"阻塞"**，既不记通过也不记失败（`requires = ["cc"|"python"|"lua"]`）。
   §7.7.3/§7.6.2 把工具在场列为前提，而 §12.5 要求逐分类通过率——把缺解释器的 B 轨用例记成通过是
   谎报，记成失败是把环境说成实现。
6. **(f) `defect` 标记把"已知未修"与"通过"分开**：挂着 `defect = "F11"` 这类标记的用例，其期望仍是
   **规范**给的期望（绝不按当前实现写期望），不达期望记为阻塞；一旦它通过了，加载它的那条测试直接变红
   并要求删标记。这一条是为了禁止"用 用例 记录 缺陷 当 通过"的第二种谎报。
7. **(g) `kind` 三档**：`check`（阶段 [1]-[5] + 干净时的 [6a]）、`fmt-idempotent`（§9.5 二次格式化
   逐字节相同）、`ifbc-verify`（§7.8.3 六项校验，退出码沿用 D-45(c) 的归属：有阻断 = 2）。
   §12.3 的黄金文件/可复现构建与 §12.4 的等价性矩阵**不进 `cases/**`**：它们是命令行为的断言
   （现有锁点 = `infy build --verify-reproducible`、`infy ifbc dump --canonical` 与 C 侧 `.can.txt` 的
   `cmp`），塞进用例格式只会造出第二套声称。
8. **(h) 运行器必须走 `infy check` 自己那条链**：`cli::check_verdict`（`load_scoped` + `analyse` +
   干净时 `ingest_b_track`）是唯一入口，运行器不复写前端。D1(2)/D2 两轮都实证"模块自己全绿对
   CLI 能不能走到它没有预测力"，用例集若跑在私有管道上，`infy check` 坏了照样全绿。

锁点 = `conformance::tests::the_category_table_adds_up_to_the_specs_own_totals`（1244/256/800/2300
与 20 行，直接抄 §12 的合计句）、`category_slugs_and_directories_are_unique_and_complete`、
`the_corpus_loads_without_a_single_format_error`、`every_case_in_the_corpus_meets_its_expectation_or_is_marked_blocked`、
`a_case_that_misses_its_expectation_fails_loudly_instead_of_being_skipped`（码表外的期望码由**加载器**拒、
码表内但不可能产生的期望码由**运行器**判失败——证明这套东西能变红）、
`an_unknown_category_directory_is_a_load_error_not_a_silent_drop`、
`a_case_id_that_does_not_match_its_directory_is_a_load_error`、
`tally_reports_every_declared_category_including_the_empty_ones`、
`the_default_manifest_is_the_one_the_runner_writes`。

## D-48 §12.5 要求 `infy conformance --report` 输出逐分类通过率，§13.4 给了它的开关却从未给它任何退出码：通过率的分母、空分类的渲染、`--category` 过滤运行的身份、`--update-golden` 到底更新什么，全部由此条确定

E2 轮（命令行层）落地时定。数据面（分类表、用例格式、运行器、`tally`）是 E1 轮的 D-47，本条只裁命令层。

1. **(a) 退出码归属**（§13.4 的 0–7 里没有一个分配给 `conformance`）：
   - `0` = 20 个分类**都**达到 §12 该行 `required` 的用例数，且 `Fail = 0`、`Blocked = 0`；
   - `2` = 有用例 `Fail`（实现给出的诊断与规范期望不符 —— 就是 §13.4 的"诊断阻断"那一档的语义）；
   - `1` = 其余：有 `Blocked`、或用例数没达到规范目标、或用例集加载失败/用法错。
   **为什么 `0` 还要求"套件是完整的"**：§12 把套件定义为 2300 个用例；在 83 条上打 `0` 会被 CI 读成
   "整套过了"。覆盖率不是可以省略的分母，`--report` 的合计行因此同时打通过率与"覆盖 83 / 2300"。
2. **(b) `Fail` 优先于 `Blocked`**：两者同轮出现 ⇒ `2`。`Blocked` 不是失败，但"实现与规范不符"必须比
   "没判完"更响。
3. **(c) 通过率 = `Pass / 用例数`，`Blocked` 进分母、绝不进分子**；`用例数 == 0` 打 `n/a`，不打 `0.0%`
   也不打 `100.0%`。若分母只算"判完的"（`Pass + Fail`），1 条通过 + 5 条阻塞就是 100%，正是 §12.5 要防的
   形状；空分类若打 100% 则把"这一行 §12 完全没被检查"读成"这一行全过了"（同 `infy ifbc` 空区那一族的
   "空最容易被读成查过了没问题"）。
4. **(d) `--category X` 的过滤运行不构成任何结论**：未运行的分类在表里打 `-`（用例数列）与"未运行"
   （通过率列），**不打 0**（0 会被读成"查过了，这个分类没有用例"），退出码恒不为 `0`；`--category` 指向
   一个 0 条用例的分类时命令**报错**，而不是打一份空表冒充跑过。
5. **(e) `--update-golden` 两个分支都拒绝**。无 `--i-know-this-is-breaking` ⇒ §12.4 的授权错误；
   **有授权也拒绝**，因为本实现在 `conformance/cases/**` 里没有可更新的黄金文件：§12 明令"已有用例的编号与
   期望不得改变"（按当前行为重生成期望 = 把实现写成权威），而 §12.3 的黄金文件（`.inf meta` + 反汇编 +
   `infy ifbc dump`）是 `runtime/fixtures` 那两份两侧各写一半的文本（`.bin` = Rust `INFY_REGENERATE_FIXTURE=1`，
   `.can.txt`/`.ver.txt` = C `INFY_REGENERATE_CANONICAL=1`），命令行取黄金文本走 `infy ifbc dump --canonical`
   （D-45(b)）。真要把产物里的 `.inf ifbc` 当黄金还需要阶段 [8] 的执行通道（BACKLOG **F29**）。
6. **(f) §13.4 未列的开关一律拒绝**（`--json`、`--verbose` 等），不静默忽略 —— 同 D-35(c)/F19(b) 那一族。
7. **(g) 默认用例根 = 仓库内 `conformance/cases`**（按 `CARGO_MANIFEST_DIR` 解析，与进程 CWD 无关）；
   §13.4 的 `[tests/]` 只是可覆盖它的位置参数。`--report` 接受但不改变行为：这条命令除了跑套件打逐分类
   通过率没有别的动作，规范里也没有第二种形态。

锁点 = `cli::tests::{the_conformance_report_prints_every_declared_category_and_keeps_blocked_out_of_the_numerator`（20 行一行都不能少 + 空分类是 `n/a` + `defect` 编号打出来 + 有 Blocked 时不为 0,
`conformance_exit_refuses_zero_for_anything_short_of_the_whole_suite`（把 (a) 的规则与用例集状态分开钉，
"全通过但覆盖不足"这一档必须显式钉住，否则将来用例涨到 2300 时没人记得这条）,
`a_filtered_conformance_run_marks_the_unrun_categories_and_offers_no_verdict`,
`conformance_refuses_misuse_and_golden_updates_by_name`, `conformance_is_no_longer_a_refused_command}`（拒绝名单与 `usage()` 两处都要红，因为拒绝臂和已接线的命令可以共享同一个退出码）。


## D-49 §9.6 给了 `[lints]` 的封闭集与 `E-0093`，却没说这条配置在哪个阶段进入诊断、也没说 `E-0093` 该指向何处：本轮把两件事都定成可复现的行为

规范原文只到"只有 `W-`/`N-` 可调（`E-0063` 是唯一例外）；试图降级 `E-`/`R-` ⇒ `E-0093`"。两处空白：

1. **(a) 生效阶段**。`[lints]` 来自 `MODULE.infy.toml`，而清单是阶段 [1] 读的，诊断从 [2] 起产生。
   裁成 **在 `load_scoped()` 里、清单解析完就写进 `Handler::lint`** —— 于是 `[lints]` 也管得住 [1] 自己
   （词法阶段的 `W-0001`/`W-0002` 是本轮实测的那条路径）。若留到渲染前才应用，"过滤"与"计数/退出码"
   会读到两份不同的配置（`Handler::visible()`、`count()`、`strict` 三处都读 `self.lint`）。
   **改前的事实**：`Loaded::lint()` 会把 `[lints]` 翻成 `LintConfig`，但全仓零调用点，而 `load_scoped()` 里
   活着的那三行只赋 `strict` ⇒ 清单里的 `[lints]` **被静默忽略**（写 `allow` 不生效、写降级也不报错 = 假成功）。
2. **(b) `E-0093` 的落点**。它说的是清单里的一行，而清单不是 `Sources` 里的文件、没有 span，规范也没给
   "配置诊断该打哪个 file/行/列"。裁成 **`Phase::Lex` + file `0` + `1:1`**，与同一处早已存在的
   `E-0570`（清单读不出来）用同一套坐标；被拒的具体原因进 `note`（`LintConfig::set` 的返回串点名是哪条码）。
   没有为它新增 `Phase` 变体（§9.2 的渲染表按现有 11 个阶段分列，凭空加一档会让渲染器与 `--diag-json` 都缺一行）。
3. **(c) 只作用在主 module**。§9.6 的 help 明写"调整只在主 module 生效，不传递给依赖"，因此应用点取
   `load_scoped()` 读到的**那一份**清单，不去合并依赖清单里的 `[lints]`。
4. **(d) 降级请求不改变被降级的那条诊断**。`E-0063` 之外的 `E-`/`R-` 条目**只报 `E-0093`**，原意图不生效
   —— 实现不接受"降级了但没人知道"的中间态，也不因为报了就顺手把该码删掉。

锁点 = `cli::tests::manifest_lints_reach_check_and_a_downgrade_is_refused`（同一份源码三份清单的成对断言：
无 `[lints]` ⇒ 诊断集恰为 `["W-0002"]` 且 exit `2`；`W-0002 = "allow"` ⇒ 诊断集为空且 exit `0`；
`E-0101 = "allow"` ⇒ 含 `E-0093` 且 exit `2`）。实测语料（`examples/`、`conformance/cases/`、`runtime/`）
里没有任何一份清单写过 `[lints]` ⇒ 这条接线不会改动既有用例的期望码。

## D-50 F4 的三件「明确未实现」：`infy lsp`、`std.net` 的 TLS 后端、`dyn Trait` 的 vtable —— 逐条给出**实测的**拒绝形态，并把规范没写的偏移表定成有主、可测的数

F4 的验收是"要么实现，要么在本文档保持明确记录，不许静默"。三件**今天都是显式拒绝**（下面的数字都是
本轮实测，不是推断），所以本轮不写实现，而做三件事：(1) 把拒绝的**形态**钉成回归锁点；(2) 把规范唯一
没给的东西 —— `dyn Trait` 的偏移表（附录 C-7「仍缺 ②」）—— 定成本实现冻结的数，并让**文档成为被测的那一侧**；
(3) 把实测出来的"拒绝的理由是错的"那两条挪进 `docs/BACKLOG.md` F40/F41，不让它们藏在一条正确的拒绝里。

1. **(a) `infy lsp` 保持拒绝，且不降级成空 server**。规范面只有 §13.1 的 `infy lsp  Language Server（可选）`
   一行，而附录 C-7「仍缺 ④」明写"增量诊断与 §9.4 的逐字节一致性"从未定义：局部修改后 LSP 缓存的阶段
   [1]-[5] 如何与全量 `infy check` 同码同文没有判据，而 §9.4 恰恰把"同码同文"当合规要求。⇒ 实现之前先有规范，
   否则只能造一个**看起来在服务**的东西，那是假成功。实现面 `cli.rs:33` 的 `NOT_IMPLEMENTED` 含 `lsp`。
   实测：`infy lsp` 与 `infy lsp examples/tour` 都打印 `notice: 'infy lsp' is not implemented in this build …`
   并 **exit 1**（§13.4 的 generic 档）。**注意档位不够**：拒绝臂与"未知子命令"臂同为 exit 1，
   所以锁点必须是名单本身 + `usage()` 那一行（D2/conformance 的同一条教训）。
2. **(b) `std.net` 的三条 + §10.4 的 TLS 后端：本实现的约束下不可实现，记成明确未实现**。
   §10.4 要求"TLS 后端由产物 `.inf rt` 段内置（不依赖系统解释器）"，而 §1.3 与 BACKLOG §0.7 的硬约束是
   只依赖 std / libc、crates.io 被策略拦截 ⇒ 没有可依赖的密码学实现，内置 TLS 属于要 vendoring 才有的一档，
   本轮**不做**。实测拒绝形态：`import "std/net" as net;` 之后 `net.http_get(…)` ⇒ `E-0101 未声明的标识符 'net'`
   + `E-0110 类型 <invalid> 没有方法 'http_get'`；换全限定 `std.net.http_get(…)` 同样是 `E-0101`（`未声明的标识符 'std'`）
   + `E-0110`；`infy check` **exit 2**。`runtime/infy_rt.c` 里 grep `TLS` 只命中 thread-local 的 `INFY_TLS`
   宏（**同一个三字首两种含义**，写记录时别看错），没有任何 socket / 握手代码。
   **必须一并记下的**：这条拒绝**理由目前是错的** —— 它不是"net 未实现"，而是"任何 `import \"std/*\"` 都不产生绑定"，
   连有 native 入口的 `std.fs.read_to_str` 也吃同一对码（`stdlib::std_packages()` / `all_symbol_paths()` 全仓零调用点）。
   ⇒ 拆出去登记 **F40**；`stdlib.rs` 模块头那句"STD 表驱动 name resolution / `infy doc` / codegen"在 F40 修掉之前是**假声称**，
   本轮先把该注释改成"表尚未接进解析"（见锁点段）。
3. **(c) `dyn Trait` 的 vtable 偏移表：本实现定死如下，且文档是被测方**。
   §4.1 只承诺"胖指针（数据 ptr + vtable ptr）；vtable 布局由 `IFABI` 固定"，附录 C-7「仍缺 ②」确认偏移表从未写出。
   裁定 = `docs/internals/runtime-abi.md` §1 新增的那三行（`drop@0:8 size@8:8 align@16:8`、第 `i` 个方法指针 `24+8i`、
   `total = 24+8n`；胖指针 16/8），数的**唯一真值**是 `hir::Layouter::vtable_record`，测试从文档里把那三行
   **读出来**再比 —— 与经验库 doc-claims 那条一致：差分文本由对方生成，不许两侧各自算。改动这张表 = `IFABI` 变更 = MAJOR（§1.2）。
   表"定了但没发射"：阶段 [8] 的 `Ty::Dyn` 臂与 `ExprKind::Is` 臂都是显式 `CgError`，`.inf meta`（§7.4 的冻结容器）
   没有装 vtable 的槽位 ⇒ **跨 `.infib` 的 `dyn` 链接不是本实现现在能做的声称**。
   实测：`fn f(x: dyn Shape) { }` 配入口链 —— 阶段 [1]-[5] **过得去**（`infy check` exit 0），拒绝发生在 [8]：
   `error: codegen: 'dyn Trait' 的 vtable 发射未实现 …`、`infy build` **exit 6**。
   而 `let s: dyn Shape = Sq{ side: 1.0 };` 在阶段 [4] 就吃 `E-0110 绑定类型不匹配：声明 Shape，实际 Sq`
   —— 缺的是**上转（强制转换）**，不是 vtable 发射；这一条拆出去登记 **F41**，不许把它误读成"[8] 的臂不可达所以偏移表白定"。
   另记：C 侧的 `InfyDebugVt`（`runtime/infy_rt.h`，只有 `write` 一个指针）是 §6.7 derive 的 Debug 派发，
   与 `dyn` 表**不是同一个对象**，两份文档不许混称 "vtable"。

锁点 = 四条，各测一层：`cli::tests::lsp_is_refused_out_loud_and_never_as_a_working_server`（名单 + 两种参数形态的
exit 1 + `usage()` 措辞）、`cli::tests::a_std_package_call_is_refused_and_never_faked`（`net`/`fs` 两条调用的码集
恰为 `["E-0101","E-0110","E-0101","E-0110"]` 且 exit 2 —— 修 F40 时必须连同 (b) 一起改这条期望，不许只放宽）、
`codegen::tests::a_dyn_type_is_refused_at_build_by_name`（合法 `dyn` 形参走到 [8] 以条款号被拒，且错误串里不含 `E-0`
⇒ 证明拒绝不在 [4]）、`hir::tests::the_vtable_offsets_published_in_the_abi_doc_are_the_ones_the_layouter_produces`
（**负对照实测**：把文档里 `1@32:8` 改成 `1@33:8` ⇒ 该测试红并报"文档里的方法 #1 槽 ≠ 布局器"，改回即绿）。

## D-51 F5 的四处规范没写或自相矛盾：`infy fmt` 根本没有路径参数、目录算一个 module 还是算一批文件、空集与不在 module 内怎么报、`infy expand --derive` 取值却没有入口 —— 全部定成确定实现

F5 的验收要求"三条命令（`check`/`fmt`/`expand`）各带目录参数行为一致"，但一动手就撞上规范面：§13.4 的
`infy fmt [-w] [--check]` **一个路径参数都没列**（同段的 `check [mod]`、`run [mod|src.infy]`、`ifbc dump|verify <artifact>` 都列了），
而 §6.8 的 `infy expand [mod] [--macro name] [--derive Type] [--stage all|macro|derive]` 三个开关**全都取值**。
⇒ "一致"没有可抄的定义，只能把实现选择定死并留锁点。四条裁定如下（(a)–(d) 各带实测命令行，全部是本轮命令输出）：

1. **(a) `infy fmt` 接受 `[mod|src.infy…]`，作为 §13.4 的超集，而不是另造一套参数语法。**
   理由：§9.5 的规则全是**逐文件**的（第 11 条甚至专门规定"`.infy` 之外的文件不处理"），而 §13.4 只给了
   `[-w] [--check]` 两个开关，没说这些规则作用在哪些文件上；本实现早就接受文件路径（`fmt --check <file>`），
   本轮补的是目录，两者共用同一条"路径 → 待格式化文件"的规则才不会出现第三种形状。`usage()` 那一行同步写成
   `fmt [-w] [--check] [mod|src.infy…]`。**不声称**§13.4 写了这个参数：它没写。
2. **(b) 目录参数 = 该目录下的每个 `.infy`（沿 §5.1 的目录跳过规则），不是"整个 module"。**
   这是三条命令唯一允许不一致的地方，且理由是硬的：`check`/`expand` 的单位必须是 module —— 名字解析（§4.1/§5.3）
   与 `[lints]`（§9.6）都以 `MODULE.infy.toml` 为准，所以 `check examples/tour/core` 必须解析到
   `examples/tour` 这个根；而 `fmt -w` 是把字节**写回磁盘**的动作，把它的作用域悄悄扩到用户没指的两个目录之外
   （`cmd/runner/`…）是破坏性的，不是"一致"。实测同一棵树两种写法各自稳定：`infy fmt --check examples/tour/core`
   → `1 file(s) are not canonical` + exit 2（只有 `core/build/mod.infy`），`infy fmt --check examples/tour`
   → `2 file(s) are not canonical` + exit 2（加上 `cmd/runner/main.infy`），后者与
   `(cd examples/tour && infy fmt --check)` 逐字同结论 ⇒ "指向 module 根"这一档三条命令完全一致。
3. **(c) 空集与不在 module 内 = 显式 exit 1，绝不许是 `0 file(s) formatted` + exit 0。**
   修前两条都是静默的：无参数且当前目录不在 module 内时 `fmt` 打印 `0 file(s) formatted` 并 **exit 0**
   （`default_infy_files()` 拿 `find_module_root(None)` 的 `None` 当"零个文件"继续跑），目录里一个 `.infy` 也没有时同样。
   ⇒ 硬约束 (d)（未实现/无事可做必须显式）在这一支上的落点。裁定：无参数 → 报与 `check` 同一句话
   （`找不到 MODULE.infy.toml——先 infy init <module-path>…（§5.1）`，exit 1）；显式目录无源文件 →
   报 `… 下没有 .infy 文件（§9.5 第 11 条只处理 .infy）`，exit **1**（§13.4：1 = 一般性失败；2 是"诊断阻断"，
   而这里一条诊断都没产生）。
4. **(d) `infy expand --derive Type`：取值但本实现没有按类型筛选的入口 ⇒ 显式拒绝，exit 1。**
   §6.8 把 `--derive` 和 `--macro`/`--stage` 并列为取值开关，而 `macro_expand::ExpandOptions` 只有
   `only_macro` + `stage` 两个字段。三种可选形状里（静默忽略 / 当 module 路径 / 拒绝）前两种都是假成功：
   静默忽略等于声称"按类型筛过了"，当路径用就是 F5 的那个根因（`--stage macro examples/tour` 当年报
   `找不到 module 根`）。⇒ 本轮定成拒绝 + 文案指名缺的是"按类型筛选的入口"，并提示 `--stage derive` 是现有的全量档。
   这条**不是**功能实现，`--derive Type` 仍属未实现（不勾任何 BACKLOG 项）。

锁点（`src/cli.rs`，共 5 条，全部实测过负对照）：`an_option_value_is_never_read_as_the_module_path`（(d) 与 §13.4
取值面的两头：11 项逐个跳值 + 布尔开关/`--x=y` 形态不吃 + 名单内容逐条相等）、
`every_option_whose_value_the_cli_reads_is_declared_in_value_opts`（把 `src/cli.rs` 读回来扫每个
`flag_value`/`opt_value` 调用点，实测扫到的 9 个名字逐条相等 ⇒ 新增取值点或把某处改成读变量都会红）、
`a_directory_argument_reaches_check_fmt_and_expand_the_same_way`（(a)(b) 的验收本体，八条命令行全 exit 0）、
`fmt_refuses_an_empty_directory_and_a_missing_module`（(c) 的两半）、
`expand_refuses_the_derive_filter_it_does_not_implement`（(d)）。
**没被这条偏差覆盖、仍开着的三件**：F43（未知开关被静默忽略）、F44（不带 `-w` 也原地改写文件）、
F45（§9.5 第 11/14 条的两个入口 `fmt::check_file` / `InfyfmtConfig::parse` 在 `src/` 里零调用点）。
D-51(b)(c) 只规定"目录→哪些文件"和"空集怎么报"，**没有**规定"`-w` 才写盘"和"非 `.infy` 跳过"，那两条要等 F44/F45 裁决。

## D-52 §4.1 的显式判别式在 AST 里存什么、超宽之后由哪一层报 `E-0042`、格式化器打印什么，规范都没写：F10/F51 轮的四处定死

§2.8 只说"无后缀整型超出 `i128` → `E-0042`"，D-25 只推论出"判别式装不进 `uint32_t` 标签就是 `E-0042` 而不是截断"。
两者都没写这四件事，而 F10（阶段 [4] `u128 as u64` 回绕）与 F51（解析器把宽于 `u128` 的判别式静默降级成"没写"）
恰恰是同一处信息在两层各自丢失。按协议 §0.3(c) 逐个定死：

**(a) AST 形状：判别式保存"写下来的字面量"，不保存它的值。**
`ast::Variant::disc` 从 `Option<u128>` 改成 `Option<ast::Discriminant>`，`Discriminant { lit: token::IntLit, span: Span }`。
"有没有写判别式"从此只由 `disc.is_some()` 回答，而 `lit.value: Option<u128>` 的 `None` 保留它本来的意思
（词法器"放不下 `u128`"，`token.rs:160`）。修前两个构造共用一个形状：`A = 2^128` 存的 `value` 是 `None`，
而 `None` 正是"没写"，于是 `enum Wide { A = 2^128, B, C }` 在阶段 [7] 只见 `disc = None` ⇒ `A = 0, B = 1, C = 2` 静默成立。
这与 F9 那一族（两个形状表示同一构造 ⇒ 判据必漂）是同一课的反方向：**同一形状表示两个构造 ⇒ 判据同样必漂**，
而且这次漂在唯一读取口上。读取口因此只有一个：`Discriminant::value()`，`sema`/`derive`/`fmt` 三处都走它。

**(b) 谁报 `E-0042`：按层分配，两半各自可测。**
阶段 [4] `sema` 管两种"装不进收集宽度 `u64`"：连 `u128` 都放不下（标题"字面量超出 i128"）与
放得进 `u128` 但放不下 `u64`（标题"字面量超出可表示范围"）—— 各打一条，然后该变体按**隐式续号**继续收集。
"继续"不是宽容：这条诊断是阻断的（`infy check` 从此 exit 2），继续收集只为让同一文件里其余声明照常被检查，
而不是让一个超宽判别式把后面的语义检查全吞掉。阶段 [7] `hir` 继续管"装不进 `uint32_t` 标签"（D-25 原判据与原锁点）。
成对断言因此跨两层，且**每一层都能单独变红**：`A = 4294967296` 在 [4] 干净、在 [7] 报；
`A = 2^64` 与 `A = 2^128` 都在 [4] 报。锁点 = `sema::tests::over_wide_enum_discriminants_are_e0042_and_never_a_wrap`
（含"只报 A 那一条，B/C 的续号不得级联"的集合相等断言）与 `cli::tests::over_wide_discriminants_stop_check_instead_of_wrapping`
（端到端：`check` 与 `build` 都 exit 2，渲染里看得见写下的数字本身）。

**(c) 收集时的续号用 `saturating_add(1)`，不再 `disc + 1`。**
`enum Max { A = 18446744073709551615, B }`（`u64::MAX`，阶段 [4] 合法、阶段 [7] 才会拒）在修前会让
`disc + 1` 在 debug 构建里**算术溢出 panic** —— 编译器前端因为一份源码而 panic 从来不是可交付行为。
饱和的代价是 `B` 与 `A` 同值，实测表现是阶段 [7] 对两个变体各报一条 `E-0042`（`infy build` exit 6）：
`B` 的真值 `2^64` 本来就不可表示，而这条枚举无论如何都会被显式拒掉 ⇒ 这里没有"静默"可言，
但它**不是**一条被证明等价的语义，只是当前不可达，故登记为本条而不是伪装成规范判据。

**(d) 格式化器打印什么：有值打十进制规范形，没值打写下来的形态，永不整段删除。**
能放进 `u128` 的判别式照旧打十进制（`0x1F` → `31`，与 F6 轮之前的行为逐字节相同，不动既有锁点）；
放不进的没有值可打，改打 `0x`/`0o`/`0b` 前缀 + 去下划线的数字 + 后缀，即 `fmt::int_lit_text`。
修前它打的是"什么都没有"—— ` = …` 从输出里整段消失，那不是格式化而是**改写源码**（并且与"本来没写"不可区分）。
锁点 = `fmt::tests::a_discriminant_wider_than_u128_is_reprinted_not_dropped`（两支各钉：超宽的留在输出里、
`2^128-1` 仍打十进制）与 corpus 新增的 `enum-disc` 行，后者同时过 §9.5 规则 12（幂等）与规则 13（AST 指纹不变）。

**(e) `derive` 那一层的 `as u32` 折位保持不变，但记明它不是已验证的等价。**
`derive.rs` 给 `#[derive(Hash)]` 生成 `hash_feed_disc(disc)` 时仍写 `… .unwrap_or(vi as u128) as u32`。
不变的理由是可达性而不是正确性：判别式只要超宽就在 [4]（`u64`/`u128` 那一半）或 [7]（`u32` 标签那一半）被阻断，
派生体因此没有通往 [8] 的路；而今天所有 derive 连 `check` 都过不了（**F35**）。**措辞红线**：
在 F35 修掉之前不许声称"派生体的判别式宽度已按 (b) 校验过"，那半句要靠 F35 的锁点补。

**变异探针（两条都实测过，不是推理）**：M1 = 把 `sema` 的 `u64::try_from(x).ok()` 换回 `x as u64` ⇒
`sema` 与 `cli` 两条同时红（`left: ["E-0042"] right: ["E-0042", "E-0042"]`，即 `2^64` 那条又消失）。
M2 = 把 `parser` 的 `Some(Discriminant{..})` 换回 `i.value.map(..)`（复刻 F51 的形状合并）⇒
`parser`/`fmt`/`sema`/`cli` **四条同时红**，覆盖四个层。两处变异都已复原，复原后
`grep -rn "i\.value\.map" src/*.rs` 只剩 `hir.rs` 那处既有的数组长度读取（属 F11 那一族，本轮未动）——
**F11 轮之后这一处也改成 `and_then(u64::try_from)`，同一条 grep 现在返回 0 行**，见 **D-53(d)**。

## D-53 `[T; N]` 的长度：文法写 `int_lit`、§2.12 说封闭集合"用于数组长度"、§4.5 说"N 必须编译期常量"，而求值失败该报哪条码规范没写 —— F11 轮的四处定死

三个落点各说一半：spec:303 的文法是 `"[" type_ ";" int_lit "]"`（只写字面量）、spec:226 的 §2.12
把"数组长度"列为 const-eval 封闭集合的**用途**（⇒ 长度可以是常量表达式）、spec:535 的 §4.5 表写
"`sizeof(T) * N`，N 必须编译期常量"。而失败侧只给了两句通则：spec:590「参数为类型或常量时必须在编译期
求值完成；无法常量求值 → `E-0131`」与 spec:227「违反（§2.12 的禁止构造）→ `E-0060`」。
`sema.rs:448` 的 `const_eval_usize(len).unwrap_or(0)` 就是在这道缝里把不可求值的长度做成 `0`，
布局与边界检查照 0 元素算，`infy check` 全绿。按协议 §0.3(c) 逐条定死：

**(a) 类型位置按 §2.12 的封闭集合求值，不按文法的 `int_lit` 收窄。**
文法那行不足以当唯一判据：若长度只能是一个字面量，§2.12 把"数组长度"列为用途就没有意义，
而 §4.5 说的是"编译期常量"而不是"字面量"。所以 `const` 读取、算术、`as`、§10 里标注 `const fn` 的
`std.math`/`std.num` 调用都算合法长度。实测（`target/scratch/f11`）：`[u32; 4]` → 4、`[u32; 2 + 3]` → 5、
`const N: u32 = 4;` 之后的 `[u32; N]` → 4，三条都 `check ok`、exit 0。

**(b) 失败码 = `E-0131`，但只在类型位置；表达式位置的 `[x; N]` 保留它已有的 `E-0060`。**
`E-0131` 在 §9 码表里的样例是内置函数（`std.mem.size_of<dyn Unknown>()`），本条把它的使用面按 spec:590
的原文扩到"类型参数与常量参数无法求值"；`E-0060` 的既有发射点（`[x; N]` 那一支，本轮之前的 `sema.rs:1663`）
语义一字未动 ⇒ **同一份根因在两个句法位置有两个码**不是即兴，是"类型位置由 spec:590 管、
表达式位置的 §3.11 判据早已落在 `E-0060`"。实测：`[u32; NOPE]` 一条 `E-0131`（exit 2），
`[0u32; NOPE]` 一条 `E-0060`（exit 2）。

**(c) 形状：求值失败 ⇒ `Ty::Invalid`，绝不返回 `Array { n: 0 }`。**
`0` 是一个**写得出来的合法长度**（`[u32; 0]` 今天仍然干净通过，且 `§4.5` 给它 `sizeof = 0`），
"求不出长度"与"长度是 0"共用一个形状 = D-52(a) 那一课（同一形状表示两个构造 ⇒ 判据必漂）；
而 `type_compatible` 的第一条臂就放行 `Ty::Invalid`，所以换形状不制造级联（F36/F37 那一族）。
实测：`struct S { pub a: [u32; NOPE] }` 的字段类型文本从 `[u32; 0]` 变成 `<invalid>`，
`infy build` 不再能对着一份 `check` 绿的源码发 `uint32_t a[0]`。

**(d) 长度读取口两处都不折位。**
`sema::const_eval_usize` 的 `ConstValue::UInt(u) => Some(u as u64)` 换成 `u64::try_from(u).ok()` ——
`u128` 放得下而长度放不下就是"没有长度"，不是取低半（`[u32; 18446744073709551616]` 修前静默得 `0`，
本轮实测 = `check` 从 exit 0 变 exit 2）。`hir::literal_len` 的 `i.value.map(|v| v as u64)` 同批改
`and_then(|v| u64::try_from(v).ok())`：这条今天已被 (b) 挡在阶段 [4] 之前，改它的理由是**第二个读取口
不该留着同一族折位**（F10 的"把截断换成检查时顺手扫相邻算术"），并留一条直接打 `hir_ty_from_ast` 的锁点
`hir::tests::an_over_wide_array_length_has_no_layout_at_stage_7`。

**(e) 一个根因只报一条，代价是把"字段类型被解析两遍"量了出来。**
同一份 `f.ty` 在注册通道（`sema.rs:607`）与 `check_item`（`923`）各解析一次；照 (b) 直接发诊断，
`struct S { pub a: [u32; NOPE] }` 会在同一坐标打两条 `E-0131`（实测确实如此，加判据前 2 条）。
实现改成：长度按 `Span` 只上报一次，并把该 `Type` 自己的 span 登记为"已解释"；字段与绑定那两条
通用兜底只在没人说过的时候开口（绑定兜底的条件同时改成它自己 note 写的那个形状 —— "既无类型标注
也无初始化表达式"，有标注而标注自己报过的情形不再重复报告）。**残留两条登记为 F52，本轮不修**：
① 双通道解析本身还在，"const-eval 自己已经报过"的长度今天仍打两遍 —— 实测 `[u32; 1 / 0]` 的
`E-0062` 在**新旧两个二进制里都是 2 条**，不是本轮引入；② `prog.consts` 按 item 顺序填充，
所以 `struct S { a: [u32; N] }` 写在 `const N: u32 = 4;` **之前**时读不到值 ⇒ 本轮之后它显式失败
（修前是 `uint32_t a[0]` + `sizeof(S) == 0` 且 `check` exit 0，实测于本轮保存的旧二进制），
要让 §2.12 的"const 读取"与声明顺序无关，需要常量前置 pass 或定点迭代。

**变异探针（五条全实测，跑在副本树里，仓库一字不动；脚本 = `scripts/f11-mutation-probe.ps1`）**：
control（无变异）= `561 passed / 0 failed`；M1 = 两处长度求值换回 `Some(self.const_eval_usize(len).unwrap_or(0))`
⇒ **4 条红**（`a_const_read_is_the_array_length` · `a_repeat_expression_length_reports_once` ·
`an_over_wide_array_length_does_not_wrap` · `an_unevaluable_array_length_is_e0131`）；M2 = `ConstValue::UInt(u)`
换回 `Some(u as u64)` ⇒ `an_over_wide_array_length_does_not_wrap` 红；M3 = 撤掉按 `Span` 的一次性上报 ⇒
两条"恰一条"的断言红；M4 = 字段兜底不看"已解释"判据 ⇒ **2 红**；M5 = 绑定兜底换回
`matches!(actual, Ty::Invalid) && init.is_none()` ⇒ **1 红**。脚本对每个变异都要求"它自己那条断言变红"，
所以 (b)(c)(d)(e) 四条裁定各有牙齿，不是只有实现跑绿。
**探针第一次跑失败的形态值得记下来**：副本目录里残留一个**名叫 `src` 的文件**（上一版脚本把 `src\*`
复制到还不存在的目标目录，PowerShell 把其中一个文件改名成了 `src`），于是 `cargo` 在副本里读到的树是坏的、
`test result` 行为空 —— 这会被后面的判据读成"变异没红"。现在脚本先清掉自己的 work 目录，再在复制完成后
断言 `Cargo.toml`/`src/lib.rs`/`src/sema.rs`/`conformance/cases`/`docs/coverage.md` 五项齐备，缺一即 exit 2 ⇒
**"没测"永远不许被读成"没红"**。

## D-54 连携语句写在 chain 顶层：规范给了码（`E-0300`）却没给"报错之后怎么办"，而实现这里是**非终止** —— F53 轮的四处定死

§8.1 的分类表把 `goto` / `start` / `start_flag` / `resume` 列为**连携语句**，"只能出现在某语句尾部的
`linkage_block = { … }` 内"；§9.6 的码表 `E-0300` 标题就是「连携语句独立出现（UB-011）」，note 的样例
正是 `goto("x");` 写在 chain 顶层。规范**没说**的是：报了 `E-0300` 之后解析器怎么继续（恢复策略）。
实现这里更糟：`parser.rs` 的链体循环 `while !at(RBrace) && !at_end()` 只写
`if let Some(s) = self.stmt() { … }` —— **没有前进守卫**。`stmt()` 对这三个关键字返回 `None` 且不消费记号
（它没有这三条臂，落到的表达式那支也无法起始），于是每圈攒一条诊断、`pos` 不动 ⇒ 诊断 Vec 翻倍到
**33 GB** 把进程打死（实测 `cargo test` 的临时探针 `memory allocation of 33285996544 bytes failed` /
`0xc0000409`；`infy check` 与 `infy fmt --check` 对九个最小形状**全部超时 rc=124**）。
`block()` 与 `linkage_block()` 两处都有这个守卫，**只有 `chain` 那一处漏了** —— 所以同一个 `goto` 写在
`fn` 体里只是 `E-0053`（实测 exit 2、不挂），而**修前既有测试一条都没红**：规范的连携块形状走的是有守卫的
那条路（**判据写在哪一层 ≠ 覆盖到哪一层**，见经验库 unreachable-implementation-branch）。

**定死的四处**：

* **(a) 码用 `E-0300`，标题逐字取码表**「连携语句独立出现（UB-011）」—— 不是 `E-0053`（那会把 UB-011
  这一类语法外的语义错误降级成"缺记号"），也不是新造码（§9.6 是封闭表）。`help` 用码表的
  「包进宿主语句的 `{ … }` 连携块」。
* **(b) 恢复 = 消费整条构造**：命中这三个关键字时调 `linkage_stmt()` 把 `goto(…)` / `start(…)` /
  `start_flag` **整条吃掉**再报，而不是 `bump()` 一个记号。只前进一格会把 `(+0)` 留在流里 ⇒ 级联出
  一串 `E-0053`（本仓库反复出现的那类"一个错爆 N 条"，见 F36/F37）。消费整条 = 一个构造恰好一条诊断。
* **(c) 通用守卫照 `linkage_block` 同形补齐**：`stmt()` 返回 `None` 且 `pos` 未动时
  `syntax_err` + `synchronize()`，仍不动再 `bump()`。这一条**不是**为 `goto` 服务的，是为"任何无法起始
  语句的记号"服务的（锁点 = `junk_at_chain_top_level_reports_once_and_advances`，语料是一个裸 `)`）。
  (b) 与 (c) 是两层：(b) 给规范要的码与干净恢复，(c) 保证这一族**任何**成员都不再挂死。
* **(d) `resume` 这一半没跟着改**：`stmt()` 有 `Kind::Keyword(Kw::Resume)` 臂（`parser.rs:1835`，产
  `StmtKind::Resume`），所以 `chain C { resume(); }` 按 §8.1 也该是 `E-0300`，实测却是 `infy check`
  exit **0** / 0 诊断。**没顺手改** —— 它已有 HIR 与发射路径，把它挪进连携族要连动阶段 [7]/[8]，
  超出"止血非终止"这一格。登记为 **F54**（保持 `[ ]`，写清实测行为与改动面）。

**成对锁点三条**（`parser::tests`）：`a_linkage_statement_at_chain_top_level_is_e0300_and_terminates`
（三个关键字各一份语料）、`junk_at_chain_top_level_reports_once_and_advances`（通用守卫）、
`a_goto_mounted_on_a_host_statement_is_not_e0300`（合法形状**不许**被新诊断波及 —— 这条防的是把修复
做成"一律拒绝"）。**验收本体 = 修前/修后同一份语料对照**（九个形状 rc 124 → 2），记在
`docs/coverage.md` 的 **P37**；临时探针（`zz_probe_f12_text`）按规矩删掉，不留进测试树。

## D-55 F13 的负字面量：规范给了"无后缀字面量取上下文类型"（§4.13）和"整体落在目标类型范围"（§2.8）两句话，但没给**码**，也没说这条范围规则要落在哪些位置 —— 七处定死

**现场**（`target/scratch/f13/` 十九个最小 module，release 二进制逐条实跑）：修前
`fn g() -> i32 { -128 }`、`let x: i8 = -128;`、`let x = -1;`、`take(-128)`、`-0x10` **全部**
`check FAILED`（`E-0110 · 一元负号作用于非数值类型 <int-literal>`）—— 也就是这个写法在 INFY 里根本
不能出现；与此同时范围的另一半是**空**的：`fn g() -> u8 { 300 }`、`take(300)`、`P { a: 300 }`、
`[1u8, 300]`、`x + 300`、`var x: u8 = 0; x = 300;`、`const K: u8 = -1;` 修前全部 `check ok`，其中六条
在阶段 [8] 显式拒绝（`codegen: 字面量 300 放不进 UINT8_C` + exit 6），而 **`const K: u8 = -1;` 连
check 带 build 全绿、值直接进产物**。一条规则两半都漏：该报的没处可报，该拒的没人拒。

* **(a) 未定型字面量取负之后仍是未定型字面量。** `Ty::IntLit` 的含义就是"类型由上下文给"（§4.13
  规则 2），一元负号不产生类型，所以它不该是 `E-0110` 的判据；范围检查挪到**上下文已知**的那些点
  去做（(d)）。`expr.kind` 带着字面量的数字与后缀，所以值不需要塞进 `Ty` —— **没有新增第二种形状**
  （F9/F10 那一课的正面执行：把两个事实塞进一个 `Ty` 变体才是漂移的起点）。
* **(b) 范围失败用 `E-0042`；BACKLOG F13 验收里那句"成对测试报 `E-0111`"作废。** 理由：`E-0111` 的
  标题与 §4.2 的 UB-002 钉死为「隐式浮点/整数互转」（spec:554、spec:1419、§9.2 金样那行也是
  `期望 u32，实际 f32`），拿它报"负值放不进无符号类型"等于给一个码两种语义；`E-0043` 的标题钉死
  「**带后缀**字面量溢出」（spec:1553），挪用给无后缀形状同样不行。剩下的码里标题正好是这件事的
  只有 `E-0042`「字面量超出可表示范围」（spec:1552），而且 §2.8 非十进制那一支的超限本来就报它 ⇒
  **同一个事实（值放不进类型）不分两套码，判别式是"这个类型是谁给的"**：上下文给的 → `E-0042`，
  后缀给的 → `E-0043`。
* **(c) 带后缀字面量取负、负值超出后缀自己的范围 → `E-0043`**（`-1u32`、`-0xFFu8`）。这正是 §2.8
  负数条线的字面推论："若一元负号的运算数在加宽后可精确表示且**整体落在目标类型范围**"才允许，
  `-128i8` 因此合法、`-129i8` 因此 `E-0043`（后半 `int_lit_ty` 早就在做，缺的是无符号那一半）。
  判据只在**字面量**上生效：`-x`（`x: u32` 是变量）没有幅度可查，仍由阶段 [8] 指名拒绝 —— 先量
  拒绝落在哪一层，再决定这一层要不要搬上来。
* **(d) 上下文站点穷举**（`sema::check_int_in_ctx` 的调用点，少一个就是"护栏只做一半"）：标注过的
  `let`/`var` 绑定、缺省成 `i32` 的绑定（§4.13 规则 2 末档）、尾表达式的返回值、`return v;`、
  语句式赋值与表达式式赋值两条、函数实参、结构体字面量字段、数组元素、二元运算的两侧、`const` 与
  `static` 的初值。**两类位置按规范明文排除**：`<<`/`>>` 的两侧都不查 —— §4.12 写"移位量 ≥ 类型
  宽度 → 运行时 `R-0007`"，所以 `x << 200`（`x: u8`）是合法程序而不是字面量超限；索引、`as`、match
  臂体与构造器实参不是"上下文类型已定"的位置，见本条末尾的登记。
  `Ty::Ref`/`Boxed` 会剥一层再查（`let r: &u8 = &300;` 的上下文是 `u8`）。
* **(e) 标注优先于初值的字面量**：`(Ty::IntLit, Some(declared))` ⇒ 绑定类型取 `declared`。修前这条
  优先级没实现，`var x: u8 = 0;` 把 `x` 绑成了 `Ty::IntLit`（标注被丢掉），后果不是"少一条诊断"
  而是**判据失去对象**：`x = 300;` 与 `let y: u32 = x;` 都因为两侧都是 `IntLit` 而 `type_compatible`
  恒真（实测修前两条都 `check ok`，本轮实测分别报 `E-0042` 与 `E-0110`）。这是 (b)(c) 的范围规则
  能够落到赋值站点的前提，不是附带清理。
* **(f) 超出 `i64` 的十进制无后缀字面量：`int_lit_ty` 报完 `E-0042` 之后返回 `Ty::Invalid`，不再
  返回 `Ty::IntLit`。** 修前它照样报错但把类型留在"待上下文决定"的形状上，于是新增的上下文规则对
  **同一个根因**再报一次 `E-0042`（外部用例 `2.8-literals-0011` 从 `["E-0042"]` 变成两条，被运行器的
  多重集比对逮住）。非十进制那一支早就是 `Ty::Invalid` ⇒ 两支现在同形（一条根因恰好一条诊断，
  F36 那一族）。
* **(g) 阶段 [8] 的字面量文本改成带符号值**：`numeric_text(v: i128, …)`，于是 `-128i8` 发
  `INT8_C(-128)` 而不是 `(-(INT8_C(128)))`，负 `const` 同理。这不是审美：`128` 这个**幅度**放不进
  `i8`，旧代码在 `numeric_text` 的边界上必然拒绝 §2.8 明文允许的 `-128i8`（实测
  `codegen: 字面量 128 放不进 INT8_C` + exit 6，而 check 是绿的）。同时删掉 `const_text` 里
  "取幅度、再在外层补一个负号"的第二套表示。`literals_carry_their_resolved_width` 跟着改认新形，
  并加了一条反向断言"旧形不许再出现"。**这不是放宽断言**：同一个构造的规范文本没变，变的是实现
  此前发不出来的那一档。

**登记（本轮不做，实测在 (d) 之后仍然成立）**：**F55** = 任何以 `return v;` 收尾、没有尾表达式的
函数体都吃 `E-0053 · 缺少返回值`，而 `note` 自己写着"函数体没有尾表达式**也没有 return**"（实测
`fn g() -> i64 { return 5; }` = exit 2，用 `target/scratch/f11/infy-old.exe` 复跑证明早于本轮）——
本条的返回站点因此只能测 `E-0042` 那一半。
**F56** = `const`/`static` 初值除字面量范围外**不做类型检查**（`const K: u8 = 1i32;` check/build 全绿），
`Option`/`Result` 构造器的实参与 match 模式的字面量不在 (d) 的清单里（`Some(300)` 在 `Option<u8>`
返回位置 = check 绿、build 6），§2.8 的加宽特例对**无后缀**的 `i64::MIN` 那一档仍然先被字面量自身的
上限判据拒掉（`fn g() -> i64 { -9223372036854775808 }` = `E-0042`）。
**F57** = 宽度小于 64 的移位没有 codegen 入口（`x << 40`，`x: u8` = build 6，消息自己点名缺
`R-0007` 检查入口），而 §4.12 承诺的是运行时事件。

---

## D-57 F58 的能力表版本忠实性：规范只说"数值键 = 该 minor 的 `dis.opmap` 值"，没说什么算该 minor 的
opmap、没给"表描述错了版本"这一类错误的码、也没说契约该由谁来持有 —— 七处定死

**现场**：`capabilities/IFBC-CAP-py-3.13.toml` 自 2026-10 起就是一份**改了标题的 3.14 表**（自己的头注释
承认 "CPython 3.13 is NOT installed here… deliberately has NO numbered (0xNN) rows"）。后果方向与 §7.7.2
的 P3 相反：不是漏放行，而是**假红** —— 真 3.13 主机上 `BINARY_SUBSCR`（3.13 的 0x05）在表里**一行都不存在**，
扫描器打 "该 opcode 不在能力表内" + `E-0712`，于是 `x[i]` 这种合法 Python 被说成 INFY 不支持。
实测两份表的数值行数：3.13 = **0**，3.14 = **154**；3.13 表里却出现 `BUILD_TEMPLATE`、
`ANNOTATIONS_PLACEHOLDER`、`LOAD_FAST_BORROW`、`LOAD_SMALL_INT` 四个只有 3.14 才有的名字。

* **(a) 缺解释器不等于不许有数值**。§7.7.3 只说能力表随版本发布，E.3 只说数值键是"该 minor 的
  `dis.opmap` 值"。本轮把它定成：**取该 minor 自己发布的 `Include/opcode_ids.h`**（CPython 的发布件，
  不是记忆），并在生成前用**本机装得上的那个 minor** 校准筛选规则 —— 规则复不出 3.14.6 的
  `dis.opmap`（154 项逐名逐值相同）就**拒绝生成**（`scripts/gen-cap-table-py313.py` 退码 4）。
  F28 当年写"凭记忆填等于编造常数"是对的，但它给的唯一出路是"等有 3.13 的机器"；本条把另一条出路定死。
* **(b) opmap 的构造规则定死**：`opcode_ids.h` 的 `#define` 名集 **减去** 任何
  `family(...) = { … }` 块的成员；**块里被 `//` 注释掉的名字仍算成员**。最后半句不是细节洁癖：
  `BINARY_OP_INPLACE_ADD_UNICODE`（3.13 的 0x03）在 `bytecodes.c` 里就是被注释掉的家族成员，
  而它同时又被 `analyzer.py` 硬写进 `instmap` —— 按"注释即排除"读，3.14 上复现 `dis.opmap` 精确到
  每一个名与数；按"注释不算"读，3.14 多出一个名字，校准当场红。生成的 3.13 表因此**没有 0x03 行**，
  这是定死的口径，下一轮不许"顺手补上"。
* **(c) "表描述错了版本"用 `E-0713`，不许复用 `E-0712`**。`E-0712` 的标题被 §7.7.2 钉成
  "字节码能力缺失"（这条指令不支持 ⇒ 出路是扩表或改写该处），`E-0713` 的标题是"字节码版本不匹配"
  （clause §7.7.3 ⇒ 出路是换版本声明或重生成表）。两者**修法相反**，混用会把"重生成这张表"读成
  "等能力扩容"。§9.3 的 210 码表里没有独立的"能力表谎报版本"码，故落 `E-0713`
  （新增 `IngestErr::CapVersionMismatch`，`code()` 打 `E-0713`）。
* **(d) 契约由工具链持有，不由表持有**。哪些名字必须出现、哪些不许出现，如果写进 TOML 就是**自我声明**
  （正是阶段 G 的 G32 批评的那一类：可信度靠头注释声称）。定成
  `src/cap_opmap.rs::opcode_contract(lang, minor)`（与表**不同源**，由同一生成器从 CPython 发布件写出），
  `CapTable::load()` 装载时校验三个方向：该 minor 必有名**缺席**、别个 minor 的名**出现**、本 minor
  从未分配过的**数值键**。
* **(e) 数值键的规范式与两侧一致**。E.3 的例子写 `0x71`/`0xb8`（两位十六进制），两份表也这么写；
  实现此前按 `format!("0x{opcode:x}")` 计算 ⇒ **0x10 以下的每一行，数值键都查不到自己**，
  `check_alias_agreement` 对那 15 个 opcode **永不触发**（F28 说"永不触发"的那条，根因在这里）。
  定死：`numeric_key` = `0x{opcode:02x}`，装载时把任何 `0xHEX` 键归一到该规范式（`0x5` 与 `0x05` 同一行），
  而同一 opcode 两种拼法给出**两个不同值**则在解析期拒绝。
* **(f) 11 个只有 3.13 才有的 opcode，映射值由本轮定**（规范不给，实现也未定）：
  `BINARY_SUBSCR = "idx.get"`（与 3.14 表 `BINARY_OP` 的 `[]` 臂同目标，这就是 F58 点名的那条假红）；
  `RETURN_CONST`、`BUILD_CONST_KEY_MAP`、`LOAD_ASSERTION_ERROR`、`BEFORE_WITH`、`BEFORE_ASYNC_WITH`、
  `LOAD_METHOD`、`LOAD_SUPER_METHOD`、`LOAD_ZERO_SUPER_ATTR`、`LOAD_ZERO_SUPER_METHOD`、
  `INSTRUMENTED_RETURN_CONST` 一律 `"-"`（**显式声明未映射**，硬约束 (d)）。理由逐条写进表头注释。
  本轮**不引入新的 IFBC 助记符**：`RETURN_CONST` 需要 push+ret 融合、`BUILD_CONST_KEY_MAP` 需要
  "键元组 + N 值"的融合构造，那是 §7.8 的 ISA 变更，登记为 **F60**，不是靠往表里填一个不存在的
  助记符来"修好"（`check_mnemonic` 也会当场拒）。
* **(g) `BINARY_OP` 的 `[]` 臂在 3.13 表里删掉**。3.13 的下标是 `BINARY_SUBSCR`，`BINARY_OP` 的参数域
  里根本没有 `[]` 这一档；留着一臂 = 声称一个永不成立的用例，与 F58 的病同族（表说的和该 minor 的
  事实不一致），只是方向不伤放行。

**登记（本轮不做，实测成立）**：**F59** = 表里 `[builtin]`/`[import]`/`[exception]` 三段是从旧表
**逐字搬过来**的，也就是 3.14 的名字集，同一类版本忠实性问题仍在（本轮的契约只覆盖 `[opcode]`；
生成器已在段内写明这条，段内注释 = 本轮唯一被授权的"声称"，因为它指向 F59 而不是自称正确）。


