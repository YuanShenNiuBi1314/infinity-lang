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

## D-17 §14 导览源码不是 §9.5 的不动点 —— **未裁决（OPEN）**
A3 轮实测：`(cd examples/tour && infy fmt --check)` → `2 file(s) are not canonical`，exit 2。
§9.5 声称格式化输出是字节唯一的、且 `fmt` 的输出是其自身不动点（规则 12）；§14 的导览是规范自带的示例源码。
两者目前互相矛盾：无法判断是 §14 示例未按 §9.5 书写（样例应重写），还是 §9.5 的某条规则与本仓库实现的理解不一致（实现应改）。
**本轮不选择、不改动实现**，只登记：裁决前，`README.md`/`docs/status.md`/`docs/coverage.md` 均不得声称"§14 导览已是规范写法"；
追踪项 = BACKLOG F6（验收 = 该命令 exit 0，或本条改判为规范缺陷并由规范方认可）。

## D-18 未知字面量后缀由语法层报 `E-0053`，`E-0043` 的"后缀不在集合内"分支不可达
§2.8 规定后缀集合封闭，`src/sema.rs::int_lit_ty` 为此写了 `other => E-0043` 分支；但词法器只在
认得该后缀时才把它并入整型字面量，`1u99` 实际被切成 `1` + 标识符 `u99`，于是报错来自语法层：
实测 `fn f() { let x = 1u99; }` → `E-0053 语法错误：期望 ;，实际为 'u99'` + `E-0101`。
**选择**：不改词法器（把任意 `u\w+` 吞进字面量会让 `1u` 后接标识符的输入更难恢复），保留"未知后缀必被拒绝"这一
§2.8 承诺，由 `E-0053` 承担；`sema` 的 `E-0043` 分支只对词法器承认的后缀形态生效（溢出判定仍走 `E-0043`，已测）。
回归测试 = `sema::tests::unknown_literal_suffix_is_rejected`（断言"必须被拒绝"，不锁死具体码）。

## D-19 `E-0121` 目前低估：数值可变绑定不报 —— **未收敛（OPEN，BACKLOG F7）**
§4.3/UB-006 要求"跨链裸共享可变状态"一律编译期报错。`src/borrow.rs` 的实现是保守近似：在链 `A` 内出现
`start(chain = B)`（B ≠ A）时，检查当前作用域的存活绑定，条件为 `mutable && !ty.is_numeric() && !matches!(ty, Ty::Rc{..})`。
后果：`var a: Acc = …` 会报 `E-0121`（已测），而 `var n: u32 = 0` 同样跨链可达却**静默通过**——数值绑定并不比
结构体绑定更安全，排除它没有规范依据。同时该判定把"链的局部 `var`"当成跨链共享，方向上偏严（局部绑定按 §8.2
属于各焦点私有栈，真正危险的是捕获/全局路径）。
**本轮不改动**：判据本身需要与 §8.5 焦点模型对齐后才谈收窄或放宽，属规范解释问题；A4 只补了它现在**确实**报告的
那一条（`borrow::tests::mutable_state_shared_across_chains_is_e0121`）与三条不该误报的边界（`Arc`/同链重入/`let`）。
F7 验收 = 逐条判定数值绑定是否必须报、以及局部 `var` 是否应豁免，并把结论写回本条。

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
3. **判别式的语法**。`Variant.disc` 是 `Option<u128>`，即只能装整数字面量；规范 §3 的 EBNF 没有 `enum_decl` 的产生式体，
   没写 `A = expr` 是否允许常量路径或负数。**选择**：只接受非负整数字面量，其它形态报 `E-0053`（解析器的语法码），
   字面量宽于 `u128` 时留给 §2.8 的范围检查（`E-0042`/`E-0043`），不重复报同一个记号。

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
注意 `sema` 收集判别式时做的是 `v.disc as u64`（u128 → u64 截断），所以**宽于 `u64` 的判别式在到 HIR 之前就已经回绕**，
HIR 的 `u32` 检查拦不住它 —— 登记为 BACKLOG **F11**。
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

## D-31 §4.7.1 的移动（owned 值换地址）在 stage [8] 被**拒绝**

§4.7.4 + ABI §3 的释放是靠 `infy_cleanup_push(fn, &obj, is_defer)` 登记**地址**、在作用域末尾
`infy_cleanup_pop_run()` 弹**栈顶**实现的（`infy_rt.h:115-118`）。`let d = s;`、把 owned 值按值传给形参、
`return s;` 三种移动都会让已登记的地址失效：要么留下双弹（同一缓冲两次 `infy_free` = 段错误），
要么对已经搬走的值执行释放（悬垂）。运行时没有"按地址取消/搬迁清理项"的入口。

**选定实现**：这三种移动形式一律 `CgError`，消息指名 §4.7.1/§4.7.4 与"双层释放/悬垂"，
并在 `docs/BACKLOG.md` 登记为 **F12**（给运行时加 `infy_cleanup_relocate(old, new)` 或按地址移除的 API）。
不移动的 owned 值（形参、`let` 绑定到新建值）按声明序 push、作用域末尾 pop，LIFO 即 §4.7.4 的逆序。
**锁定点** = `codegen::tests::moving_an_owned_binding_is_refused`（三种形式成对）+
`owned_values_register_a_cleanup_entry_at_their_scope`（正常路径的 push/shim/pop 与元素大小）。

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
