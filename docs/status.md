# 构建状态（如实记录）

## 已跑通的端到端验收

`examples/tour` 是按 §14（语言导览，module 布局 + 类型/所有权 + 链/节点/连携）写的真实 module，
用本仓库构建出的 `infy` 可执行文件检查：

```
$ infy check
check ok — 2 file(s), 0 error(s), 0 warning(s), 0 notice(s)
$ echo $?
0
```

即阶段 [1] 词法/语法 → [2] 宏与 derive → [3] 名称解析与包图 → [4] 类型检查 → [5] 借用检查
→ §9.2 诊断渲染 → §13.4 退出码 这条链在多 package module 上真的跑起来了。

为达成它修掉的真实缺陷。**证据类别要写清**：这批修复落在 `sema.rs`/`parser.rs` 的类型与链语义上，
而 `sema.rs` 没有独立单测，所以除下表点名到的两条（词法 `AttrOpen`、parser 源码序编号）之外，
它们的回归证据是"tour 跑完阶段 [1]–[5] 且 exit 0"这一条端到端，不是逐条断言（补齐计划见 BACKLOG A4）：

| 缺陷 | 条款 |
| --- | --- |
| `#![entry_chain = "X"]` 这类**无括号带赋值**的属性被文法拒绝 | §3.1 与 §6.6/§1.2/§14 的写法互斥，实现取并集（见 spec-deviations D-11） |
| `Foo { a: 1 }` 单元名结构体字面量被解析成"标识符 + 块语句" | §3.8 `struct_literal` |
| 连携块写在宿主语句 `;` **之后**另起一行时被当成独立块语句 | §3.5 `linkage_block` / §14.3 |
| `return`/`break`/`continue`/`for` 被当作表达式起始记号，导致 `return Err(..)` 解析失败 | §3.5 它们是语句 |
| `Ok(v)`/`Err(e)`/`Some(v)`/`None` 被当普通函数查表 → 假 `E-0101` | §4.1 内建类型 |
| 无后缀整型字面量被定成 `i64`，于是 `job: 0` 报假 `E-0110` | §2.6/§4.13 上下文取型、兜底 `i32` |
| `Result<_,_>`/`Option<_>`/`Vec<_>` 等同构造类型不做逐参数兼容判定 | §4.1/§4.2 |
| `let v = f()?;` 之后块尾表达式看不到 `v`（尾表达式被求值两次，第二次作用域已弹出） | §3.3 block 的尾表达式属于本块 |
| `goto(+1)` 用"节点总数"而不是"当前节点索引"判越界 → 假 `E-0301` | §8.2/§8.4 |
| `goto("anchor")` 在同文件第二条链上查第一条链的锚点 | §8.2 |

## 测试计数（主树实测，非隔离副本；逐条状态见 `docs/coverage.md`）

```
$ cargo test --offline --lib
test result: ok. 423 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
$ cargo check --offline --lib
Finished `dev` profile  (50 warnings，全部为 unused/dead code，见 BACKLOG F2；D4(1) 清掉一处死字段)
$ infy check examples/tour
check ok — 2 file(s), 0 error(s), 0 warning(s), 0 notice(s)   # exit 0
$ infy build examples/tour
check ok — 2 file(s), 0 error(s), 0 warning(s), 0 notice(s)
codegen ok — 12513 byte(s) of C11 written to examples/tour/.infy-build/x86_64-pc-windows-gnu/ci.c (stages [7]-[8]; 1 chain(s), 6 node(s))
artifact ok — 199975 byte(s) 写到 examples/tour/.infy-build/x86_64-pc-windows-gnu/ci.exe（§7.4：宿主镜像 199173 + 段 blob 738 + 段尾 64，6 段）
  build-id 4d8250b58122ca2b76132029955e2d2005156dc0a6b021bf5f4ae761e5fd55ba   # exit 0（C3 轮起阶段 [9] 已落地）
$ infy run examples/tour                                        # C4 轮起：[7]-[9] 建到临时目录 + 立即执行
check ok — 2 file(s), 0 error(s), 0 warning(s), 0 notice(s)
codegen ok — 12513 byte(s) of C11 written to %TEMP%\infy-run-<pid>-<n>\ci.c (stages [7]-[8]; 1 chain(s), 6 node(s))
artifact ok — 199975 byte(s) 写到 %TEMP%\infy-run-<pid>-<n>\ci.exe（…）       # 程序的 stdout/stderr 逐字节转发
run ok — 程序退出码 0（stdout 0 字节 / stderr 0 字节，§13.4）                  # 这行在 stderr
$ echo $?
0                                                                # 程序 stdout 为空的原因见 BACKLOG F19：没有任何 INFY 构造能写 stdout
$ infy run <overflow="panic" 的 u32::MAX+1 module>               # 同一命令，程序里发生 panic
runtime[R-0001]: 整数溢出（`overflow=panic`）                     # §9.2 报告逐字节转发在 stderr
  = note: infy_add_u32 overflowed (§4.4: overflow="panic", §4.9 UB-003)
run FAILED — 程序退出码 3（§13.4：3 = 运行时 panic）
$ echo $?
3                                                                # §13.4 的 3；两条都由 cli::tests 断言
$ infy test examples/tour                                        # C5 轮起：测试构建 + 跑 #[test] harness
check ok — 2 file(s), 0 error(s), 0 warning(s), 0 notice(s)
codegen ok — 12681 byte(s) … (stages [7]-[8]; 1 chain(s), 6 node(s))   # 测试构建的 C11 比普通构建多 168 字节
artifact ok — … 写到 %TEMP%\infy-run-<pid>-<n>\ci.exe（…）
test result: 0 passed, 0 failed, 0 total                         # 程序的 stdout（tour 里没有 #[test]）
test ok — 程序退出码 0（stdout 42 字节 / stderr 0 字节，§13.4）    # 这行在 stderr；exit 0
$ infy test <全绿的合成 module：void 测试 + 返回 Ok 的 Result 测试>   # 见 P18
test example/ctest/cmd/ctest::calls_work ... ok
test example/ctest/cmd/ctest::result_ok ... ok
test result: 2 passed, 0 failed, 2 total                          # 程序 stdout 139 字节逐字节（\r\n 归一后）
$ echo $?
0
$ infy test <有测试返回 Err 的 module>                             # 见 P19
test example/ctest/cmd/ctest::a_ok ... ok
test example/ctest/cmd/ctest::b_err ... FAILED
test result: 1 passed, 1 failed, 2 total
test FAILED — 程序退出码 1（§13.4：1 = 一般性失败 = 有测试返回 Err）
$ echo $?
1
$ infy test <#[test] 里 u32::MAX+1 的 module>                      # panic 在焦点之外没有 catch 点 ⇒ 套件中止
test example/ctest/cmd/ctest::a_first ... ok
test example/ctest/cmd/ctest::m_boom ... runtime[R-0001]: 整数溢出（`overflow=panic`）
  = note: infy_add_u32 overflowed (§4.4: overflow="panic", §4.9 UB-003)
test FAILED — 程序退出码 3（§13.4：3 = 运行时 panic（某个 `#[test]` 里 panic…整条套件中止））
$ echo $?
3                                                                # z_after 不在报告里、总结行也没打 —— 断言的是次序
$ infy test --cov <mod>                                          # §12.4 的插桩没有阶段产出（F22）
error: `infy test --cov` 未实现：…                                # exit 1，不静默忽略也不假报数字
$ infy check target/scratch/cli-test-green; infy test target/scratch/cli-test-green | head -1
check ok — 1 file(s), 0 error(s), 0 warning(s), 0 notice(s)       # §5.9：mod_test.infy 不进普通构建
check ok — 2 file(s), 0 error(s), 0 warning(s), 0 notice(s)       # 同一 module，infy test 看得见它
$ infy build examples/tour --verify-reproducible
verify-reproducible ok — 两个独立构建目录产物字节相同（199975 字节）            # exit 0，§7.3 第 3 项
$ infy build examples/tour --cc /no/such/cc
error: E-0602 · … 无法启动 …（§7.6.2：A 轨工具链缺失或未注册，绝不静默跳过链接）  # exit 6
$ infy artifact info examples/tour/.infy-build/x86_64-pc-windows-gnu/ci.exe --nodes
INFY artifact (.inf meta) … n_nodes 6 … entry chain `Pipeline` (kind 0)
chain_id  node_idx  file                   line  col  linkage  host_event_id  name
   0         0      cmd/runner/main.infy     8     5      0         -          begin     # exit 0（8/9/10/11/16/19，与 C1 金样一致）
$ gcc -std=c11 -Wall -Wextra -I runtime -c examples/tour/.infy-build/x86_64-pc-windows-gnu/ci.c -o ci.o
（无输出）                                        # exit 0，0 error / 0 warning；这是 BACKLOG C2 的验收命令（BACKLOG §1 C2）
$ gcc -std=c11 -O1 -Wall -Wextra runtime/infy_ifbc.c runtime/infy_dyn.c \
      runtime/test_ifbc.c runtime/test_ifbc_stubs.c -o ifbc_tests && ./ifbc_tests
IFBC harness: 167 asserts, 167 pass, 0 FAIL      # exit 0，0 编译告警；82 → 146 = D4(2) 合并形状 + 间隙指令 + 跨语言字节夹具；146 → 167 = D4(3) 的三份双侧共用文本夹具
$ gcc -std=c11 -O1 -Wall -Wextra runtime/infy_ifbc.c runtime/infy_dyn.c \
      runtime/infy_rt.c runtime/test_ifbc.c -o ifbc_real && ./ifbc_real
IFBC harness: 167 asserts, 167 pass, 0 FAIL      # exit 0，0 error / 0 warning；B2 轮真实链生产 infy_rt.c，stdout 与上一行逐字节相同
$ gcc -std=c11 -O1 -Wall -Wextra runtime/infy_rt.c runtime/test_rt.c -o rt_tests && (cd target/scratch && ./rt_tests)
32335 assertions, 0 failures                     # exit 0，0 error / 0 warning；链的是生产 infy_rt.c；C3 轮 +32（Rust 写 ↔ C 读的夹具锁 + D-33）
$ gcc -std=c11 -Wall -Wextra -c runtime/infy_rt.c
（无输出）                                        # exit 0，0 warning（B3 的验收命令；三项语义已于 B3 轮落地，见下文 B3 一节）
$ (cd examples/tour && infy fmt --check)
2 file(s) are not canonical                        # exit 2，见下文"导览源码不是格式化不动点"
```

423 项按模块（`cargo test --offline --lib` 逐条分组计数，合计 423）：
`lexer` 79 · `derive` 57 · `fmt` 39 · `macro_expand` 35 · `pkg` 24 · `sema` 22 · `codegen` 22 ·
`hir` 20 · `ifbc` **19**（D4(1) 前是 6） · `artifact` 18 · `cli` 16 · `toml` 15 · `borrow` 15 ·
`sha256` 12 · `codes` 9 · `parser` 8 · `base64` 7 · `stdlib` 4 · `diag` 2。

- 词法、语法、`fmt`(§9.5 十四条规则)、`macro_expand`(§6.1–§6.5)、`derive`(§6.7)、诊断码表、
  类型检查（§4.1/§4.2/§4.3/§4.6/§2.8/§4.13/§8.2）、包与依赖（§5.2/§5.3/§5.7/§5.8）、
  借用检查（`E-1001`/`E-1002`/`E-1004`/`E-1005`/`E-1006`/`E-0121` 正反例）、哈希与 base64 与 TOML 往返、
  `infy` CLI 自身、IFBC Rust 侧编码/解码/反汇编：都有上表的测试。
- **仍没有一项独立单测**：`btrack.rs`（§7.7 B 轨）。它的证据目前只有"对 `examples/tour` 跑完 §7.1 阶段 [1]–[5] 且 exit 0"这一条端到端。
  `codegen.rs`（§7.1 阶段 [8]）在 C2 轮从 1 行占位变成独立单测模块（C4 轮 **19** 项 = `goto` 必须转移控制那一条；C5 轮 **22** 项 = §5.9 测试 harness 的 3 条），`hir.rs` 的 20 项锁阶段 [7]，
  `artifact.rs`（§7.1 阶段 [9] / §7.3 / §7.4）在 C3 轮新建并直接带 18 项单测 + 一份被 C 侧读取器消费的逐字节夹具，
  `cli.rs` 从 C4 轮的 9 项到 C5 轮的 **16** 项（`infy test` 的三条端到端 + §5.9/§2.2 的文件选择两条 + §5.8 dirhash 不随构建范围变的一条 + `--cov` 拒绝）。
- 先前记为"§9.2 字节级金样"实际是 1 条规范样例逐字节 + 1 条 caret 行规则（`diag` 2 项）；
  `goto` 的三条 `E-0301` 越界路径与 `E-0121` 在 A4 轮之前无测试（**A4 轮已补**，见下）。

**绿灯的含义要如实限定**：`cargo test --lib` 全绿只说明"已写的模块自洽"，不等于 §12 合规。
`infy build` **自 C3 轮起跑完 §7.1 阶段 [1]–[9]**：前端 → HIR → C11 → 用 A 轨 `cc` 链成宿主镜像 →
追加 §7.4 段容器与 64 字节段尾，`infy artifact info` 读得回自己刚产出的文件（P4/P13/P14），故 exit **0**。
阶段 [10]（sumdb 校验与运行时自校验比对）仍**未做**，`.inf sum` 只是把 `MODULE.infy.lock` 的 dirhash 清单抄进容器；
`doc`/`lib pack`/`ifbc dump`/`conformance`/`bench` 仍显式报"未实现"（exit 1）；**`run` 自 C4 轮起真执行**（§13.4 的
开发态 = [7]-[9] 建到临时目录 + 立即执行，退出码按 §13.4，见下文 C4 一节）；**`test` 自 C5 轮起真执行**（§5.9 的
测试构建 + 生成的 `#[test]` harness，退出码 0/1/3 按 §13.4，见下文 C5 一节；`bench` 留在未实现里是因为 §13.4
根本没有这个子命令，它是 `infy test --bench`）；
B 轨 lowering(`src/btrack.rs`) 尚未收敛；C 侧 harness(`runtime/test_rt.c`) **B1 轮已收敛**（C3 轮复跑 **32335 断言 / 0 失败**，直接链生产 `infy_rt.c`）；
`src/host.rs`、`src/atrack.rs`、`src/doc.rs`、`src/conformance.rs` 是 1 行占位文件（`src/artifact.rs` 已于 C3 轮从"不存在"落成 §7.4 容器写入器 + 读取器 + §7.3 build-id，18 项独立单测）（`src/hir.rs` 已于 C1 轮落成声明层 IR，不再是占位）（**§11 `HostApi` 之前被 README 标成 ✅，是文档错误，本轮已改为未实现**）；
A 轨绑定、`std.net` TLS、`infy lsp` 未实现。
IFBC 断言（B3 轮 79 → 82，D4(2) 轮 **82 → 146**：合并线格式 + 17 条间隙指令 + Rust 写↔C 读执行的夹具）
**B2 轮起既链 `runtime/test_ifbc_stubs.c`（harness 替身）也链生产 `infy_rt.c`**，两侧同为 146/146、
stdout 逐字节相同。仍未验证的是：`infy conformance`（E1/E2）与 Rust↔C 的**逐指令反汇编相等**
（D4 余项 (3)；"两侧校验结论等价"另被 F26 挡着）。

## A2 本轮修掉的真实缺陷（每条都有回归测试）

| 缺陷 | 条款 |
| --- | --- |
| `#[link(name = "z", static = true)]` 整条属性被丢弃：属性路径与参数名只接受 `Ident`，而 `link`/`static` 是保留字 | §3.1 vs §2.4/§7.6.3（偏差 D-13） |
| `impl Drop for C` 的 `C` 被解析器丢掉（只解析一个类型，`self_ty` 停在 `Drop`），因此 `E-0421` 永不触发，所有 `impl Trait for Type` 的 AST 都是错的 | §3.2 `impl_block` |
| `j.fetch().link()` 之类链式成员名在 `.` 后被拒（`link` 是保留字），连锁产生 5 个 `E-0053` | §3.11/§7.0.1（偏差 D-14） |
| `macro_rules!` 的第一条 arm 吞掉整条剩余记号流，臂列表为空 → `infy fmt` 把 `macro_rules! v { … }` 打印成 `macro_rules! v {}`（假成功，产物不可解析） | §3.9/§6.2 |
| 语句/字段列表在**无注释无空行**时被插入两个 `HardLine`，凭空产生"只含缩进的空行"，第二遍格式化把它折叠 → `E-0091` | §9.5 规则 6/12 |
| `struct`/`union` 字段、`enum` 变体与 `C { … }` 变体字段、`match` 臂列表用换行分隔且**丢掉逗号**，输出无法重新解析 → `E-0092` | §3.7 `match_stmt`、§14 `pub struct Job { … }` |
| 写在 `unsafe` 块**上一行**的 `// infy-safety:` 被块的尾表达式扫尾收进块内并挪到末尾，第二遍报 `W-1030`（注释归属按字节区间，不再是单调游标） | §9.5 规则 8/12 |
| `@embed` 块闭包前多一个空行，破坏"块体逐字节原样"→ `E-0092`；`@c(name = "d")` 参数列表被拆成三行 | §2.11、§9.5 规则 2（偏差 D-16） |
| `where` 与首个约束被粘成 `whereT: Default`（用了 `SoftLine`） | §3.3 `where_clause` |
| `for x in items` 打印成 `for x in (items)`，每格式化一层就多一对括号 | §3.5 `for_stmt` 无括号 |
| 链式断行续行缩进取 8 的绝对列（应为语句缩进 + 一层） | §9.5 规则 3（偏差 D-15） |
| 区域指示符对（旗帜 emoji）宽度算成 1 列，应为 2 列 | §9.5 规则 1 |
| IFBC 反汇编把 `is.dyn T` 打成 D.3 风格的 `is.dyn.i64`，与附录 D.7 的写法不符 | 附录 D.7 |

## 子任务状态

`lexer`、`formatter`、`rt-core`(C 侧 harness)、`ifbc-rust`(B 轨摄取) 四个并行子任务曾撞到 agent 轮次上限被截断。
A2 轮收敛了 `formatter` 与词法/语法侧的缺口；`lexer` 的 E-0020 保留字集经核与 §2.4 一致（含
`use`/`mod`/`union`/`linkage` 这组"保留但本版文法未用"的字）。**仍未收敛**：
`src/btrack.rs` 的端到端 lowering（D1）。`runtime/test_rt.c`（B1）已于 B1 轮收敛。已完成的 C 侧里程碑不变：`ifbc-vm`(`runtime/test_ifbc.c`) **146** 项断言通过（B3 轮 79 → 82，D4(2) 轮 → 146），
含六项装载校验、逐字节截断、单字节变异、800 轮 fuzz；`codes`、`infra`(哈希/TOML) 已交付。
（A3 轮当时的限定"这些项链的是替身不是生产运行时"已于 **B2 轮解除**：同一 harness 现在也链生产
`runtime/infy_rt.c` 跑通 146/146，见上文 P11。）

新发现（未修，已记入 BACKLOG 阶段 D）：Rust 侧 IFBC 编码与 C 侧虚拟机的**操作数个数**不一致 —
`push`（Rust 2 个：池索引 + 类型；C 表 `nops=1`）、`box.dyn`（Rust 1；C `nops=0`，附录 D.7 与 `runtime/IFBC.md` 也是 0）、
`guard.nonzero`/`guard.shift`（Rust 1；C `nops=0`）、`ldw`（Rust 1；C `nops=2`）。
任一条成立都会让同一镜像两边读出的指令流错位，D2 的交叉验证必然失败。

## A3 本轮：把文档里的声称逐条对回证据

改动只涉及文档（`README.md`、`docs/status.md`、新增 `docs/coverage.md`），**代码一行未动**，
所以 285/0、51 warning、tour exit 0、79 断言四个数字本轮被复跑而不是被引用。核对结果：

| 文档原来的声称 | 事实（本轮复跑/查源码得到） | 处理 |
| --- | --- | --- |
| §11 `HostApi`、三步握手、`EnvSnapshot` = ✅ | `src/host.rs` 是**1 行占位文件**，没有任何实现 | 改为 ⬜ 未实现，并列入"明确未实现" |
| §7.4/§7.3 产物容器 = ✅ 设计与运行时实现 | 只有 `artifact info` 认 64 字节 `INFYART2` 段尾；`.inf meta` 段表不解析（自己打的 note 就写着要等 codegen）；写入器不存在，也没有 `src/artifact.rs` | 改为 🟡，指向 BACKLOG C3 |
| §7.8/附录 D IFBC = ✅ | Rust 侧 5 项 + C 侧 79 断言**各自**通过，但两侧操作数个数不一致（D4），交叉验证必然错位 | 改为 🟡，注明 D4 |
| §4/§5 = ✅ `src/sema.rs` / `src/pkg.rs` | 两模块**零独立单测**（sema 2396 行、pkg 1127 行），证据只有 tour 的 `check` exit 0 | 改为 🟡 并写明证据类别 |
| §3 = "EBNF **全产生式**递归下降" ✅ | 只有 5 项 parser 测试；没有任何"产生式→AST"覆盖审计 | 去掉"全产生式"，改 🟡 |
| §9.2 = 金样测试逐字符对齐 ✅ | 只有 2 项：§9.2 那**一条**样例逐字节 + caret 行规则 | 改为 🟡 并写清只有 1 条样例 |
| §9.4/E.5 `--diag-json` = ✅ | 能写文件（本轮手工跑出的是 `{"version":1,"tool":{...},"diagnostics":[]}`），无结构金样 | 改为 🟡 |
| §10 = ✅（驱动名称解析、领域扫描、`infy doc`、codegen 映射） | 表 + 4 项测试在；`infy doc` 与 codegen 两个消费者是占位文件 | 改为 🟡 |
| 运行时 = ✅ | `infy_rt.c` 有代码，`test_rt.c` harness 未收敛（B1），且 79 断言链的是替身（B2） | 改为 🟡 |
| "通过的 285 项覆盖 … 类型检查、包与依赖、`E-0121` 正反例、IFBC 校验" | 分模块计数如上：`sema`/`pkg` 无测试；`E-0121` 无测试（实际被测的是 `E-1006`）；Rust 侧 IFBC 无装载校验测试 | 重写成分模块计数 + 明示无测试模块 |
| README 目录树里的 `tests/ 集成测试` | 仓库没有顶层 `tests/`，测试全在 `src/*.rs` 的 `mod tests` 里 | 删掉该行 |
| "通过率见 `docs/coverage.md`（随构建生成）" | 该文件此前**不存在**，而 `infy conformance` 未实现，不可能"随构建生成"；6 个占位模块的注释也指向它（`atrack`/`codegen`/`conformance`/`doc`/`hir`/`host`，`grep -l docs/coverage.md src/*.rs` 可数） | 新建 `docs/coverage.md`：逐条款"实现 × 证据"矩阵，并写明外部用例为 0、通过率不可测（E1/E2/E3 待做） |

顺带发现两个真实缺陷（**未修**，已进 BACKLOG F5/F6）：

1. `infy fmt <目录>` / `infy expand <目录>` 的 positional 语义与 `check` 不一致：
   `error: 无法读取 examples/tour: 拒绝访问。 (os error 5)`、`error: 找不到 module 根（§5.1）`；
   只有先 `cd` 进 module 或直接给 `.infy` 文件才可用。
2. `(cd examples/tour && infy fmt --check)` → `2 file(s) are not canonical`，exit 2：
   **§14 导览源码不是本格式化器的不动点**。要么 §9.5 与 §14 互相矛盾（规范级），要么导览样例需按 §9.5 重写。
   未裁决前，任何文档不得声称"§14 导览已是规范写法"。

## A4 本轮：给 `sema`/`pkg`/`E-0121` 补独立回归测试（并修掉测试暴露的 4 个前端缺陷）

补测 **47 项**（`sema` 22 · `pkg` 19 · `borrow` 的 `E-0121` 4 · `parser` 2 项配套回归），285 → **332 passed / 0 failed**；
`cargo check --offline --lib` 仍 0 error / **51 warning**；`infy check examples/tour` 仍 exit 0；
`(cd examples/tour && infy fmt --check)` 仍 exit 2（F6 未裁决）；C 侧 `79 asserts, 79 pass, 0 FAIL` exit 0。

新写的测试里有 4 条一上手就红，根因都不在测试里，逐条修掉（每条现在都有上表的测试锁住）：

| 缺陷 | 条款 | 症状 |
| --- | --- | --- |
| `start(chain = B, node = "a")` 整个连携语句解析失败：参数名位置只接受 `Ident`，而 `chain` 按 §2.4 是保留字 | §3.5/§8.2 | 报 `E-0053 期望 )，实际为 'chain'`，`E-0800`/`E-0801` **永不触发**（与 D-13/D-14 同一类坑的第三处） |
| `E-0120`（改 `let` 不可变绑定）不触发：§3.5 把 `x = 2` 解析成**赋值表达式**，而检查只写在 `StmtKind::Assign` 分支里——该分支在 `parser.rs` 里根本没有产生式 | §4.3/§3.5 | `let x: u32 = 1; x = 2;` 一个诊断都不发（假成功） |
| `env.<字段>` 走的是字段访问路径，而 `env` 的判定只写在 `check_path`（`::` 形式）里；`env` 又被当普通标识符 | §4.6/UB-024 | 任何 `env.iter` 读都报假 `E-0101 未声明的标识符 'env'`；写 `env.iter = 1` 该有的 `E-0140` **永不触发** |
| 嵌套泛型 `Arc<Mutex<u32>>` 让解析器 **panic（`usize` 减法下溢）**：`generic_args` 的 `depth -= 2` 假设 `>>` 一定闭合两层，但内层 `<…>` 是递归解析的，`depth` 那时已经是 1 | §3.6 | debug 构建直接崩（`attempt to subtract with overflow`），release 构建回绕成巨数后继续吞 token。改为"内层欠外层的闭合记在 `closer_debts` 上"，并加 2 项回归测试（含 `Vec<Map<u32, str>>`、`Result<Option<u32>, Vec<u8>>`、`Arc<Mutex<u32> >`） |

两条**未修，只登记**：未知字面量后缀实际报 `E-0053`（偏差 D-18）、`E-0121` 的判据漏掉数值可变绑定且把链内局部 `var`
当跨链共享（偏差 D-19 / BACKLOG F7）。

## B1 本轮：收敛 C 侧运行时 harness（`runtime/test_rt.c`），并修掉它揪出的 7 个运行时缺陷

被截断的半成品 `test_rt.c` 从没编译过。收敛后：**32303 assertions / 0 failures**，exit 0，
`gcc -std=c11 -O1 -Wall -Wextra runtime/infy_rt.c runtime/test_rt.c` **0 error / 0 warning**，
链的是生产 `infy_rt.c`（不是 `test_ifbc_stubs.c` 那种替身）。同时 `cargo test --offline --lib` 仍
**332 passed / 0 failed**、`cargo check --offline --lib` 仍 0 error / 51 warning、`infy check examples/tour` 仍 exit 0、
`(cd examples/tour && infy fmt --check)` 仍 exit 2（F6 未裁决）、IFBC 侧仍 `79 asserts, 79 pass, 0 FAIL`。

**harness 自己的错**（改的是测试，不是断言强度）：`&LOC(...)` 取一个已经返回指针的函数的地址（83 处，整文件因此从未通过编译）；
`InfyString` 是 `(ptr,len)` 无 NUL 结尾却被当 C 串比较（7 处）；§4.12 恒等式那条写成了 `(b>0)==1 || r<=0`，
对 `a>0,b<0` 必然假报（56 项）；`test_map` 提前 `infy_map_drop` 后又迭代同一张表（use-after-free + double free = 段错误）；
`std.fs` 用例往 mode `"w"` 给的 1 字节缓冲里 `memcpy` 18 字节（堆越界，`-Warray-bounds` 抓到）；
VecMap 槽位断言把值的低字节当"键应为 0"；§9.2 那条同时断言同一串"存在"和"不存在"。

**两个"金样"是编造的**（这是本轮最值钱的发现）：`xxh64("")` 常数写成 `0xEF46DB3751D8F993`，而按 §6.7 钉死的
xxHash64 独立算一遍是 `0xEF46DB3751D8E999`（同一实现复现 PyPI `xxhash` 公布的两条向量 `32dd38952c4bc720` /
`b559b98d844e0635`，故算法可信）；`golden_perm` 写死 `{5,10,7,2,0,3,1,9,11,4,8,6}`，而 §10.12 钉的
SplitMix64→PCG-XSH-RR→Lemire 无偏→从尾到头 Fisher–Yates 推出来是 `{7,9,10,1,6,3,11,8,2,5,0,4}`——
**换任何方向（头到尾、带偏的取模）也都得不出那个常数**，所以它没有推导。两处都换成推导值，不是换成运行时输出。

**实现的缺陷**（每条现在都被 harness 锁住）：

| 缺陷 | 条款 | 现象 |
| --- | --- | --- |
| `infy_rt_cleanup_drain_scope` 把已经是 LIFO 的 deque 在分区时**又反了一次** | §3.13/§4.7.4 | unwind drain 按声明**正序**跑 drop 与 defer；`infy_scope_end` 反而是对的，所以只有半径路径错 |
| chain 半径只标记兄弟焦点，**漏了崩溃焦点自身** | §4.16（`chain` = 同 `focus` + 额外全链标记） | `n_failed`/`n_alive` 少计，该焦点状态停在 RUNNING |
| 被标记 FAILED 的兄弟焦点仍挂在 ready FIFO 上 | §4.16/§8.2 | 调度器随后 dispatch 一个 `frame` 已释放的焦点 → **段错误**。新增 `ready_remove_locked()`；顺带修 `infy_focus_pause` 内联手写 unlink 时把 `ready_tail` 清 0 的错（删中间/末尾节点会截断队列） |
| `InfyMetaFixed.iter_guard_steps` 是 `uint32_t`，而磁盘字段是 8 字节、§10.10 默认值是 `2^32` | §7.4/§10.10 | 读回来截成 **0**，`infy_art_apply_meta` 又把 0 当"未设置"替换成默认值 → 自定义守卫值静默失效 |
| `InfyString` 没有结构标签，`InfyDebugVt.write` 里的 `struct InfyString*` 是另一个不完整类型 | §6.7 | `infy_rt.h:212` 前置引用告警 × 4；`infy_rt.c` 为此自带一个"不改头文件只能 cast"的绕过。补标签后删掉该 cast（B3 第 4 项） |
| `{:#X}` 打印 `0XFF` | §10.11（规范未钉前缀大小写） | 与参考实现 Rust 的 `0xFF` 不一致（`rustc` 实测）；改为 `0x` 前缀 + 大写字母，记偏差 D-20 |
| 焦点 id 从 0 开始分配 | §8.5（规范未定起点） | 与"`spawn` 拒绝时返回 0"、"`infy_current_focus()` 无焦点时返回 0"三重歧义 → 0 保留为"无焦点"，分配从 1 起，记偏差 D-21 |

未修只登记：`g_host_nesting` 在 host 调用 panic 跨出边界时不回滚（`infy_rt.c:2654` 起的全局计数，`++`/`--`
被 longjmp 跳过），harness 只能在 catch 之后手工复位——已开 BACKLOG **F8**。

## B3 本轮：`infy_rt.h` 的三项语义瑕疵落地 + D-22 裁决

B3 的验收命令（`gcc -std=c11 -Wall -Wextra -c runtime/infy_rt.c` 无告警）在 B1 轮就已绿，所以本轮勾的是
**三项语义**，不是那条命令。

1. **`InfyRCode` 补齐为规范的闭集**：新增 `R_EMBED_TIMEOUT = 0x0701`（§9.7 spec:1495；`tickloop` 预算越线
   与 `timeout_ms` 越 deadline 时由 `infy_ifbc.c` 返回，规范明言"不杀进程"）与 `R_FIELD_OFF_REPR = 0x0713`
   （spec:1627 / 附录 D.9，`field_off` 作用于非 `repr(C)` 类型）。这两个码此前只以 `infy_ifbc.c` 文件内的
   `#define` 裸整数存在，`infy_rt.h` 的注释却自称闭集（"closed set, no implementation extensions"）；
   `test_ifbc.c` 的两处断言原来写死 `0x0701`/`0x0713`，现改断言枚举常量（断言强度不变）。
2. **D-22 裁决**：`0x0709` 被规范自己用了三种语义（附录 D.10 的"边界"行、spec:2417 的 tailcall 帧尺寸、
   §9.7/§7.9 的 `max_mem_bytes`）。§9.7 是码表权威，故 `R_IFBC_MEMBUDGET` 只保留内存配额一条语义；
   装载期"边界"类两处发现（tailcall 帧尺寸不兼容、`closure.new` 的 `code_idx` 越出 `func_table`）改报
   `R_IFBC_FRAME`（`R-0707`，§7.8.3 第 4 项"帧合法"，与同一函数里早已存在的"call/tailcall 目标越界 → `R-0707`"同类）。
   未使用的 `R_IFBC_BOUNDARY = 0x070A` 从枚举与 `infy_rt.c` 诊断码表删除（`0x070A` 不见于规范任何码表）。
   没有借用 `R-0702`/`E-0701`：spec:1508 明令那两个编号永久废止。取证补充：D.10 的"边界"在 §7.8.3 的 1→6 里
   并不存在（第 6 项是行映射完备 → `W-0705` 不阻断），说明该末项是顺号编出来的而非语义选择。
   `test_ifbc.c` 的映射表新增一行 `SC_CLOSURE_BAD`，断言只增不减：**79 → 82**。
3. **`frame_kind` 两级分开**：模块级（容器头第 46 字节）只允许 `0` = 值模型，其它值装载失败 `R-0703`；
   函数级（`func_table` 每条记录）取 `0 = 栈式帧` / `1 = 协程帧`，`> 1` 装载失败。C 侧原来用裸 `== 1`/`!= 1`，
   现用冻结公共头里的命名常量 `INFY_FRAME_STACK` / `INFY_FRAME_COROUTINE`（`infy_rt.h`）。
   **本轮取证纠正了 BACKLOG 的旧描述**：`frame_kind` 的函数级字段实现里早就有（`InfyIfbcFunc.frame_kind`），
   缺的是头文件侧的命名常量与取值来源，所以没有"补字段"，改为补常量 + 范围校验 + 文档。
   顺带发现并修掉一处 Rust↔C 不一致：`src/ifbc.rs::decode` 原本判据是 `frame_kind > 1` 才拒，即允许模块头写 `1`，
   而 C 侧要求必须为 `0` —— 现两侧同规则，并加 `frame_kind_header_and_function_value_sets` 测试锁住
   （头 `1` 被拒 / 函数级 `1` 可装载 / 函数级 `2` 被拒）。取值表本身规范未钉数字，记偏差 **D-23**。

回归：四条 C 链全部复跑（`-c infy_rt.c` 0 warning、rt harness 32303/0、替身链 82/82、生产链 82/82 且两份
stdout 逐字节相同）；`cargo test --offline --lib` **333 passed / 0 failed**（基线 332，+1 为新增的 `ifbc` 测试）；
`cargo check --offline --lib` 0 error / 51 warning；`infy check examples/tour` exit 0；`(cd examples/tour && infy fmt --check)`
仍 exit 2（F6 未裁决）。**未做**：`docs/coverage.md` 的 §7.8/附录 D 仍保持 🟡 —— 码表现已与 §9.7 对齐，但双侧
交叉验证仍被 D4 阻塞；本轮另外核实到 D4 的范围比原描述更大（`func_table` 记录布局两侧也不同：Rust 36 字节/条
含 `arity/ret_ty/sig/sig_hash`，C 40 字节/条含 `max_stack/ret_arity/ret_sig/flags/frame_kind`），已写进 BACKLOG D4。

## C1 本轮：`src/hir.rs` 落成声明层 IR，布局与 `gcc` 实测逐一对回（IFABI-2 金样）

阶段 [7] 的模块原先是 1 行占位。本轮把它落成**前端与 codegen 之间唯一的接口**，字段与偏移一律对
`docs/internals/runtime-abi.md`（不是对 `src/ast.rs`）：

- **布局引擎**：`Target`（指针宽度、`__int128` 对齐 = 探测值）、`Layout`、`record_layout`/`union_layout`
  （C 规则：字段 `align_up` 入位、对齐取最大、尾部补齐）、`Layouter`（带 ADT 递归栈）。
  §1 表示表**逐行**落成测试，且每个数字都有独立的 C 来源：`gcc -std=c11` 打印
  `sizeof`/`_Alignof`/`offsetof`（探针 `target/scratch/c1/probe.c`，实测数字摘录记在 `docs/RUNLOG.md` cron-07）。
  对照出的两处 ABI 文档**算术错误**已改正并写回文档：`Result<u32, Fail>` 实测 **20/4**（ok@0 err@4 is_ok@16），
  `Result<u32, u64>` 实测 **24/8** —— 原文的 `sizeof(T)+sizeof(E)+1, aligned` 两条都会算成 16。
- **条目 lowers**：`struct`/`enum`/`fn`/`event`/`const`/`static`/`chain`，含每个字段的偏移、每个变体的
  判别式与 payload 偏移、每个函数签名/返回的布局、`#[repr(C)]` 与 `repr(Rust)` 的区分（§4.8：
  `offset_of` 只对 `repr(C)` 可用，测试 `offset_of_field_is_only_available_for_repr_c` 锁住）、
  `event` 与 A 轨 `extern` 的 foreign 链接性、§5.9 `#[test]`/`#[bench]`、§7.6.5 `#[export]`。
- **链（§8.4 + ABI §5）**：节点表（`idx/line/col/linkage_count/host_event/event_id/anchor`）与链帧
  （`let`/`var` 按声明序入帧 + `node_idx` + 尾部补齐）。tour 的 `Pipeline` 金样 = **6 个节点**
  （行 8/9/10/11/16/19，`gate` 与 `retry_once` 各挂 1 个连携块，锚点 `begin`=0、`done`=5），
  它把 §8.4 规则 1"连携块与 `label` 都不开新节点"第一次钉成了可复跑断言。
  `ABI` §5/§8.4 的三种定长行也做了金样：`InfyNodeLoc` 24/8（0/8/12/16/20）、`InfyChainDesc` 48/8
  （0/8/12/16/20/24/32/40）、`InfyFocusId3` 12/4，全部与 `gcc` 的 `offsetof` 一致。
- **`env`（§4.6 + ABI §10）**：11 个字段的封闭集 + `EnvOffsets`，与冻结头 `InfyEnvSnapshot` 的
  `offsetof` 逐个对上（`iter`@0 … `platform`@40、`build_id`@56，**72/8**）。§4.6 写 `&str` 而头文件存
  的是**内联 `InfyStr`**，这条已写进 ABI 文档 §10 的表里（宿主发布整块快照，指针跨不过节点边界）。
- **符号（ABI §2）**：`infy_<mangled package path>_<item>`，非字母数字换 `_`。类型原先只有 module 路径，
  本轮改从 `Program::by_path` 反查真正的 package 路径，使 tour 的类型符号是
  `infy_tour_core_build_Phase` 而不是 `infy_tour_Phase`。
- **vtable 前缀（§1）**：`drop`/`size`/`align` + 方法指针，`24 + 8·n`（n=2 实测 40/8）。

规范未定的三项表示按协议选定并记 **D-24**（元组 = 声明序 C struct；无 payload 的 enum 省掉 union → `Phase`
= 4/4；判别式只接受整数字面量，其它形态 `E-0053`）；§4.1"判别式最小容纳"与 ABI 的 `uint32_t tag` 冲突记
**D-25**（裁决：标签恒为 `u32`，理由含 `.infib` ABI 稳定性与 `repr(C)` 要和 C 的 `int` 共享布局），
推论是超宽判别式报 **`E-0042`** 而不是截断。

**测试揪出的真实缺陷（已修）**：`enum Code { A = 3 }` 的显式判别式在解析器里被**静默丢弃且不打诊断**。
根因是 `parser.rs::variant()` 只匹配 `ExprKind::Int(..)`，而 `primary()` 的第一个臂 `Kind::Lit(l)` 已吃掉所有
字面量，它下面的 `Kind::Lit(Lit::Int(i))` 臂永不可达 —— 即 `ExprKind::Int` 从不被构造（同一类"码写在不可达
分支里"的第 4 次出现）。修法：`variant()` 同时接受两种形态并在非字面量时报 `E-0053`；
锁定点 `parser::tests::enum_discriminants_reach_the_ast_and_bogus_ones_are_e0053`。
`primary()` 的死臂本身**没动**（动它会让 `ExprKind::Int` 全树突然出现），连同 `sema` 收集判别式时的
`u128 as u64` 截断、`[T; N]` 的 N 非字面量时 `unwrap_or(0)` 静默变成长 0 三件事一起登记为 **F9/F10/F11**。

**明确未做**（不假成功）：语句/表达式级 IFIR 没落，函数体仍是 AST 的 `Block`；`infy build` 没有接 HIR
（仍打 `notice: stages [8] codegen and [9] link are not implemented yet`，exit 6）。这是 C2 的前置。

回归（全部为本轮实测，非引用）：`cargo test --offline --lib` **354 passed / 0 failed**（基线 333，+20 `hir` +1 `parser`）；
`cargo check --offline --lib` 0 error / **51 warning**（与基线同）；`infy check examples/tour` exit **0**；
`(cd examples/tour && infy fmt --check)` 仍 exit **2**（F6 未裁决）；四条 C 链未改代码但全部复跑：
`-c infy_rt.c` **0 warning** exit 0、rt harness **32303 assertions / 0 failures** exit 0、
替身链 IFBC **82/82** exit 0、生产链 IFBC **82/82** exit 0 且与替身链 stdout 逐字节相同。

## C2 · `src/codegen.rs`：阶段 [8] 的 INFY → C11 发射（本轮）

原先是 1 行占位。现在它是 §7.1 阶段 [8] 的实现，输入只有阶段 [7] 的 `HirCrate` + `sema::Program` +
解析后的 AST（函数体不在 HIR 里，C1 的遗留），输出**一个 C11 翻译单元**（偏差 D-26）。

**布局自检是这一轮的结构性决定**：尺寸/对齐/偏移一律从 `HirCrate` 取，codegen 绝不重算；同时生成的 C
自带 `sizeof`/`_Alignof`/`offsetof` 的负数组 typedef 断言。于是 **gcc 编译生成的 .c = 把 HIR 的表复核一遍**，
`Result<u32, Fail>` 的 20 字节、`Phase` 的 4/4、`Pipeline` 帧的 `node_idx@0` 都不再是"文档里写着"而是"编译器拒收"。
这比 C1 的一次性探针更强：探针要人手重跑，断言每次编译都在。

**发射面（18 项单测，每项 doc comment 指名条款）**：§4.1 全部标量与聚合表示（`Option` 的指针 niche 走
`NULL == None`，§1 的 `bool` 只发 `1u/0u` 因为头里没有 `<stdbool.h>`）、§4.8/ABI §1 记录与 union 布局、
§8.4 节点表（6 行、连携计数、显示路径）、§8.2 `goto` 编译期解析成常量、§4.9/§7.2 检查型算术与下标/`unwrap`
/移位全部走 `infy_rt.h` 入口（**没有任何裸的 C 整数 `+`**）、§4.11 `?` 的两种情形、§3.12 `match`
（含或模式与载荷绑定）、§4.7.4 清理项的 push/pop 与 `infy_drop_*` shim（`Vec` 带元素大小，§10.13）、
§4.13 规则 2/7 的字面量取型、§2.8 的 `UINT*_C/INT*_C` 字面量、§4.6 `env.x` 的编译期偏移装载、
§5.3 的调用解析、§7.4 入口与 `main` 驱动。

**本轮在自己代码里揪出并修掉 2 个真 bug**（都是测试逼出来的，不是重构）：
(1) 形参的清理项被写进了**上一个 item 的缓冲区**（`emit_fn` 在 `buf.clear()` 之前调 `register_drop`），
    于是 owned 形参根本不释放 —— 泄漏。现在清理登记发生在函数体 `{` 之后。
(2) `return s;`（s 是 owned 形参）会先跑 pop 再返回同一份缓冲 → 悬垂。现在与 `let d = s`、
    `take(s)` 三种移动形式一起显式拒绝（偏差 D-31 / BACKLOG **F12**），因为 `infy_cleanup_*` 只能弹栈顶。

**顺带修的一处解析歧义**：tour 的 `gate` 在 `cmd/runner` 与 `core/build` 各有一个，原实现按简单名全局匹配 →
静默选错。现在按 §5.3/§4.14 以**调用方 package 优先**解析，多候选才报错（有测试）。

**测试反过来暴露的 3 个上游缺口**（都不是 codegen 能修的，登记为 F13/F14/F15）：
`-128` 这类无后缀负字面量在阶段 [4] 就 `E-0110`（F13）；`Q::Y(n)` 的绑定不进作用域（F14）；
`fmt!` 在阶段 [2]/[4] 没有 lowering 路径，`sema` 见残留 `MacroCall` 就发 `E-0402`（F15）——
所以 C2 验收清单里的 `fmt` 覆盖被上游阻塞，本轮**没有**为了让它过而在后端假展开。

**改了冻结头**：`infy_chain_register` / `_table` / `infy_chain_desc` / `infy_chain_count` 四个入口
`infy_rt.c:1893-1906` 早就定义了，但从未进 `infy_rt.h`（而这个头自称"codegen 与运行时之间的契约"）；
补声明后四条 C 链全部复跑（数字见上表与下一段）。

回归（全部为本轮实测，非引用）：`cargo test --offline --lib` **372 passed / 0 failed**（基线 354，+18 `codegen`）；
`cargo check --offline --lib` 0 error / **51 warning**（本轮新增的 4 条已清掉，没让 F2 变大）；
`infy check examples/tour` exit **0**；`infy build examples/tour` → 12473 字节 C11、exit **6**（[9] 未做）；
`gcc -std=c11 -Wall -Wextra -I runtime -c <生成的 .c>` exit **0**、0 error / 0 warning；
`(cd examples/tour && infy fmt --check)` 仍 exit **2**（F6 未裁决）；四条 C 链：
`-c infy_rt.c` **0 warning** exit 0、rt harness **32303 assertions / 0 failures** exit 0、
替身链 IFBC **82/82** exit 0、生产链 IFBC **82/82** exit 0 且与替身链 stdout 逐字节相同。

## C3 · `src/artifact.rs`：§7.4 段容器 + §7.3 build-id + 阶段 [9]（本轮）

`src/artifact.rs` 从零落成 **1176 行**：容器写入器（`.inf meta` 208 字节固定记录 + 字符串池 +
`ChainTable`/`EventTable`/`NodeTable`/`.inf sum`/入口记录 + 64 字节段尾含 `trailer_crc` 与 build-id 低 4 字节镜像）、
读取器（逐条对齐 C 侧 `infy_art_parse_trailer`/`parse_meta_record`/`meta_resolve_strings` 的七个拒绝理由）、
以及 §7.3 的 `build_id`。`cli.rs` 接上阶段 [9]：`link_plan`（`--cc`/`$CC`，默认 `gcc`，缺 → `E-0602`）→
`cc -std=c11 <opt> -I runtime ci.c infy_rt.c infy_dyn.c -Wl,--no-insert-timestamp -o ci.exe` → 把 blob+段尾追加到镜像尾部；
`infy artifact info|sections [--nodes]` 换成真解析；`--verify-reproducible` 在两个独立构建目录重跑并比字节。

验收（本轮全部实测，非引用）：`infy build examples/tour` → **exit 0**，`artifact ok — 199975 byte(s)`
（宿主镜像 199173 + 段 blob 738 + 段尾 64，6 段），`build-id 4d8250b58122ca2b76132029955e2d2005156dc0a6b021bf5f4ae761e5fd55ba`；
`--verify-reproducible` → `两个独立构建目录产物字节相同（199975 字节）`；
`infy artifact info …ci.exe --nodes` → **exit 0**，读回 `ifabi 2 / usize_bits 64 / iter_guard_steps 4294967296 / n_nodes 6 / entry chain 'Pipeline'`，
节点表 6 行 = 行 8/9/10/11/16/19（`gate`/`retry_once` 各 1 个连携块）—— **与 C1 轮的 §8.4 金样逐格相同**；
`infy artifact info README.md`（非产物）→ `E-0605` + **exit 7**；`--cc /no/such/cc` → `E-0602` + **exit 6**。
交叉验证：一次性探针把**真实 tour 产物**交给 C 侧生产读取器 `infy_art_read_file` →
`rc=0 blob_len=738`，`infy_art_dump_meta` 的每个字段与 Rust 侧一致（含同一串 build-id）。

**Rust 写 ↔ C 读 的常驻锁点**：`runtime/fixtures/artifact_rust_writer.bin`（730 字节）是写入器的逐字节固定输出，
`runtime/test_rt.c` 用生产读取器读回它并断言同一组字段（+27 项），`artifact::tests::c_reader_fixture_is_exactly_what_the_writer_produces`
锁住"写入器不再改变夹具"（只有 `INFY_REGENERATE_FIXTURE=1` 才重写）。rt harness 复跑 **32335 assertions / 0 failures**。

规范未定 → 选定实现并记偏差 **D-32**（五张表的记录布局、`opt_level`/`lto` 值表、profile 名反推、
入口描述与 `ResourceKeys` 借用 `export` 区与推导规则、`artifact info` 借 `E-0605`/exit 7）。
**另发现并裁掉一条规范级缺陷 D-33**：§7.4/ABI §6 的"段尾缺失/重复即非产物"若按字面**整文件**扫魔数，
会因为 `.inf rt` 静态链接进镜像、而 `infy_rt.c` 自身带 `"INFYART2"` 字面量而**否掉每一个真产物**
（本轮第一次真接阶段 [9] 就撞上：`host image already contains the INFYART2 magic`）。
两侧同规则收窄到追加区 `[blob_off, EOF)`，写入器另拒绝 blob 自带魔数；顺带修掉 `infy_art_parse_trailer`
里 `if (t) {…}` 之后无条件 `t->hdr_ver` 的 NULL 解引用。
`docs/internals/runtime-abi.md` §6/§7 的布局草图原本写 `blob_off u64` 等 —— 两侧实现从来是 `u32` 对，
已按实现改正文档并把段尾逐字段表、blob 布局、`--no-insert-timestamp` 的必要性写进去。

**明确未做**（不用假成功掩盖）：阶段 [10] 的 sumdb 校验段与运行时自校验比对（`.inf sum` 只是 `lock.dirh` 的抄写，
tour 无依赖 → 实例上是 0 条）；`ResourceKeys`/`EnvOffsets` 区写空集（前端不收集 `std.chain.lock` 键名）；
`.inf ifbc` 恒为空（D1）；A 轨 `#[link]` 目标码与 §7.2 的 LTO 编排、`infyl` 独立链接器驱动；
`infy artifact dump --nodes`/`diff` 的其余形态；C 侧 `infy_art_dump_meta` 那行 `meta off=0 len=0` 是**写死的显示值**
（不是容器内容错，是打印错），本轮未改，登记在 RUNLOG 下一步。

## C4 · `infy run`：第一次真执行，并修掉一条只在执行时才现形的控制流缺陷（本轮）

`cli.rs` 的 `frontend()`（阶段 [1]-[5]）现在被 `check`/`build`/`run` 三条命令共用；
`run_module()` 把 [7]-[9] 建进临时目录（`%TEMP%/infy-run-<pid>-<n>/`，跑完即删）后执行产物，
`RunOutcome{code, exe, program_stdout, program_stderr}` 是被测试直接断言的那层（`run_cmd()` 只负责把
程序的两条流用 `write_all` **逐字节**转发到自己的 stdout/stderr，`run ok`/`run FAILED` 状态行在 stderr，
构建信息仍按 `infy build` 的原样在 stdout —— §13.4 没规定这个分流，选定实现记 **D-35(a)**）。

**退出码（§13.4）**：程序正常退出 → 原样转发（INFY 产物的 `main` 只会给 0 或 `infy_runtime_start` 的 3）；
无退出码（信号/异常终止）→ 记 3；产物启动失败 → 6；临时目录创建失败 → 4。规范没有"异常终止"这一档，
不新造数字，见 **D-35(b)**。

**修掉的真实缺陷（D-34）**：`examples/tour` 的产物**第一次执行就挂住**（120 秒无输出、被强杀），
而 `infy check`/`infy build`/`infy artifact info` 全部是绿的 —— 这条路径此前从未被执行过。根因：
§8.2"`goto` 语义（移动焦点，暂停当前节点之后）"被实现成"只登记 `pending_node`、节点照常跑完"，
于是 §14.3 导览节点 3 的连携块 `if (all_green()) { goto("done"); } goto(+1);` 里两条 `goto` 都执行，
`pending_node` 先写 5 再被 `+1` 覆盖成 4 → `retry_once()` → `{ goto("begin"); }` → 回节点 0 **无限循环**。
裁决 = 第一个 `goto` **立即结束该节点**：发射 `infy_goto_checked(...); drain_pops(); return;`，
`drain_pops()` 保住 §8.4 规则 4"defer/临时量 drop 计入该节点的退出动作集"。运行时 `infy_goto_checked`
与 §4.9 UB-013 的 `R-0010` 兜底都不动，**C 侧零改动**（四条链仍按纪律复跑：`-c infy_rt.c` 0 告警、
rt 32335/0、两条 IFBC 82/82 且 stdout `cmp` 相同）。生成的 C11 因此从 12473 → **12513 字节**，
产物仍是 199975 字节、build-id 不变（build-id 的输入集合不含生成的 C，§7.3）。

**取证手法**：`goto` 的三条目标常量原本已有断言（`goto_targets_are_resolved_at_compile_time`），
绿而程序死循环 —— 缺的是**次序**断言。新测试 `codegen::tests::the_first_goto_in_a_node_ends_it`
在节点 3 的函数体里要求 `goto("done")` < `return;` < `goto(+1)`；`cli::tests::run_executes_the_tour_and_exits_zero`
真执行、真退出码；`cli::tests::run_forwards_the_program_streams_and_a_runtime_panic_maps_to_exit_3`
用 `overflow="panic"` 的 `u32::MAX + 1` 造运行时 panic，逐字节断言 §9.2 首行 `runtime[R-0001]: 整数溢出（…）`
并断言退出码 3。

**明确未做**（不用假成功掩盖）：程序的 **stdout 对现有一切实例恒为 0 字节** —— codegen 的子集里没有任何构造能写
stdout，§7.7.1 第 4 项又把 B 轨 `print()` 判给日志汇（`infy_log` → stderr），登记 **F19**；
`--arg K=V` / `--feature f` 没有任何消费者 → **显式 exit 1**（`cli::tests::run_refuses_flags_it_cannot_honour`，
见 D-35(c) 与 F19(b)），没有静默忽略；`infy run` 不向程序传 argv（§13.4 未定义该通道）；
运行时按 §9.2 尝试读源行时用的是 module 相对路径，CWD 不在 module 根就恒打 `<source unavailable>` → **F20**；
`#[test]` 的执行 = C5（**已于 C5 轮落地**，见下文 C5 一节）。

## C5 · `infy test`：§5.9 的测试构建 + 生成的 `#[test]` harness（本轮）

**做完了什么**：`infy test [pkg] [--bench]` = 阶段 [1]-[5]（**测试构建**：`*_test.infy` 与 `tests/…` 只在这里入选）
→ [7] HIR → [8] 发射**以 harness 为 `main`** 的 C11 → [9] 链接 + §7.4 容器 → 建到临时目录 → 执行 → 程序的
stdout/stderr 逐字节转发 → 退出码按 §13.4。三条实测：全绿套件 **exit 0**（程序 stdout 139 字节逐字节 =
两行 `… ok` + `test result: 2 passed, 0 failed, 2 total`）、有测试返回 `Err` **exit 1**（`1 passed, 1 failed, 2 total`，
**套件不中断**）、`#[test]` 里 `u32::MAX+1`（`overflow="panic"`）**exit 3**（§9.2 首行逐字节转发在 stderr）。
`infy test examples/tour` → **exit 0**，程序 stdout 恰为 `test result: 0 passed, 0 failed, 0 total`
（42 字节）：没有 `#[test]` 不是失败，而这一行同时**反证 harness 没调 `infy_runtime_start`** —— tour 的
`Pipeline` 一个节点都没跑（否则会多出程序的输出且退出码来自调度器）。测试构建的生成 C11 = **12681 字节**
（比普通构建的 12513 多 168 字节，差值就是 harness driver）。

**§5.9 那两句"不进普通构建"此前从未接线**（本轮最大的发现）：`pkg::PackageFile.is_test`、
`Package.is_test_pkg`、`PackageFile.constraint` 三个字段早就由 `discover()` 填好，
`pkg::constraint_matches()` 也早就存在，但**全仓库零调用点** —— 也就是说 `infy check|build|run` 一直在把
`*_test.infy` 编进普通构建，而 §2.2 的 `//infy:build` 完全没生效；`lexer::lex_file_with_features()`
（唯一会发 `E-0012` 的词法入口）同样是死代码，"未知约束名"在 CLI 上从不触发。本轮接成
`pkg::BuildConfig` + `pkg::file_selected`（两道门：§5.9 的测试范围 + §2.2 的约束求值，都要过），
并**把未知名检查放在选择之前、对每个 discovered 文件做** —— 否则 `//infy:build riscv` 会让文件被静默排除，
把拼错的约束名变成"这个文件从来没存在过"。实测：同一 module 上 `infy check` = 1 file / `infy test` = 2 files；
`//infy:build ignore` 的类型不合法草稿在任何构建都不入选（所以它躺在树里也不破坏绿灯）；
`//infy:build riscv` → `E-0012`。

**顺带修掉两个真实缺陷**（都是一次性探针/新能力揪出来的，各带回归测试）：
1. **§5.1 R2 的判据写在反的地方**：`if names.insert(pkg_name).is_some() { continue; }` 把"**同名**的第二个文件"
   当作违规 —— 于是 §14.1 自己的 `mod.infy` + `cache.infy`（同一 package 两个文件）会被**静默丢掉一个文件**，
   并假报 `E-0501 内存在两个包名`。探针实测：合法 module 上 `infy check` **exit 2**。改为对"不同包名的集合"判定，
   并只允许 §5.9 明写的 `{<目录>, <目录>_test}` 并存（`pkg::tests::discover_accepts_the_package_shapes_the_spec_shows`）。
2. **`read_dir` 的顺序漏进了产物**：`discover_into` 按 `read_dir` 给的顺序建 `Package`/`PackageFile`，
   而该顺序在 `cli::load` 里决定 `FileId` 编号，进而决定发射的 C 文本与 §7.3 的 build-id —— 可复现承诺依赖文件系统。
   现在 `files`/`subdirs` 按名字排序（`pkg::tests::discover_orders_files_by_name`）。
3. **接线时差点引入的第三处（已挡）**：`cli::package_dirhashes` 的内容来自 `Loaded::files`，而 `Loaded::files`
   一旦按 §5.9 过滤，§5.8 第 1 项的"module 的**全部文件**"就跟着命令变 —— 同一个 module 在
   `infy build` 与 `infy test` 下会算出不同的 dirhash，并顺着 §7.3 把 build-id 也变成构建范围的函数。
   改为**从 `pkg::discover` 重新取全树并逐文件读字节**（`cli::tests::dirhash_covers_files_the_build_excludes`
   成对断言：普通构建 1 file / 测试构建 2 file，而两者的 dirhash 逐一相等）。

**规范未定 → 选定实现**：偏差 **D-36**（harness 替换 §7.4 入口、`E-0600` 在 test 构建跳过、只认 `()` 与
`Result<…>` 两种判据且其它签名**拒绝而不是猜**、panic 因 §4.16 焦点之外无 catch 点而**中止整条套件**、
报告由生成的 C `fputs` 到自己的 **stdout**（§7.7.1 第 4 项管的是 INFY 的 `print()`，不是编译器生成的 harness）、
`--cov` 显式拒绝、test 产物的 `.inf meta` 入口字段仍写 §7.4 那一档）与 **D-37**（§2.2 的封闭集漏了它自己用到的
`ignore`/`test` 两个名字、约束表达式解析失败借用 `E-0012` 且绝不因解析失败而收录文件、§5.1 R1 为
`package <dir>_test` 与 `tests/` 的文件主干开例外）。
`lexer::known_constraint_names()` 改为直接返回 `pkg::BASE_CONSTRAINTS`（此前是两份手抄清单，同族问题见 D-32 的"第二份真相"）。

**回归全部复跑**（C 侧本轮零改动：没动 `.c`/`.h`，四条链仍按纪律复跑、并在改动后重跑一次）：
`cargo test --offline --lib` **410 passed / 0 failed**（基线 395，+5 `pkg` +3 `codegen` +7 `cli`）、
`cargo check --offline --lib` 0 error / **51 warning**（未新增）、`infy check examples/tour` exit 0、
`infy build examples/tour` exit 0（**生成的 C11 仍 12513 字节、产物仍 199975 字节、`build-id 4d8250b5…55ba`**
⇒ 非 test 路径逐字节未变）、`--verify-reproducible` exit 0、`infy run examples/tour` exit 0（0/0 字节）、
`infy artifact info --nodes` exit 0、`gcc -std=c11 -Wall -Wextra -I runtime -c <ci.c>` exit 0（0/0）、
`(cd examples/tour && infy fmt --check)` 仍 exit **2**（F6 未裁决）；`gcc -c runtime/infy_rt.c` **0 warning**、
rt harness **32335 / 0**（在 `target/scratch` 里跑）、替身链 IFBC **82/82**、生产链 IFBC **82/82** 且两份 stdout `cmp` 无输出。

**明确未做**（不用假成功掩盖）：`std/test`（§14.7 的 `test::assert_eq`/`Bench`/`b.iter()`）在 §10 的 std 签名表里
**一条都没有** ⇒ 规范自己的测试样例仍过不了 `infy check`，本轮不代规范发明签名 → **F21**（`--bench` 对有参数的
`#[bench]` 报 exit 6 并在消息里点名本条；不带 `--bench` 时该函数不进运行集）；`--cov` 的 §12.4 节点表插桩
没有阶段产出 → **F22**；test 产物的容器入口字段与实际执行的 harness 不符（冻结容器只有 `ENTRY_MAIN`/链两档）→ **F23**；
`W-0003`（约束恒假）要的是静态不可满足性判定，本轮只对当前配置求值 → **F24**；panic 的**逐测试隔离**要把
`infy_rt_internal.h` 的 `infy_rt_catch` 提升进冻结公共头（ABI 变更 + 四条链同批），本轮不动 C 侧；
`#[test]` 函数在普通构建里仍被编译成 C 符号（只是不进运行集），§5.9 没要求剔除。

## D4(1) · Rust 侧 IFBC 线格式对齐 D-38（本轮，代码段第一半）

D-38（上一轮，纯文档）把"哪侧改什么"钉死成 (a)–(i)；本轮执行 **D4 余项 (1)**：`src/ifbc.rs`
向合并后的唯一线格式对齐。C 侧本轮**零改动**（(2) 是下一轮），两侧仍不在同一字节上相遇
（`.inf ifbc` 恒空 = D1 未落地）。

**容器（D-38 (a)）**：删掉每区的 `byte_len:u32 || body` 包装（当年注释说"不包就分不出 `code` 与
`line_map`"——该理由不成立，`code` 自己的前导 `u32` 就是它的长度），现在 header(47) 之后逐区
自定界，正是 `infy_ifbc.c::parse_sections` 的走法。落点 =
`ifbc::tests::regions_are_self_delimiting_exactly_where_the_loader_reads_them`
（各区起点是**算术推出来的**：`47 → 4+18·32 → 4+13 → 4+8+48 → 4+code_len → …`，不是抄一次输出）。

**三张表（D-38 (a)）**：
`type_table` 每行 8×`u32`（`name_str,kind,flags,size,align,nfields,payload_a,payload_b`）+
`nfields×{name_str,off,ty}`；`kind` 空间统一到 IFABI 标量码，`TyKind` **重编号**到
`SIG_*`（`void0 unit1 bool2 char3 i8..i64 7 u8..u64 11 f32 12 f64 13 str14 bytes15 ptr16 dyn17`），
扩展类别从 18 起（`list/tuple/dict/func/coro/instance/ref/box/rc/arc/option/result/never/usize/isize`），
构建器自动置前 18 行使"类型索引 == 标量码"恒成立（`N_SCALAR_ROWS`）；重编号后 `InfyDynTag`
不再是数值相等，改由显式表 `dyn_tag()` 给出。
`import_table` = C 形状 + 尾部 `sig_hash u32`，`kind` 折叠为 `host0/embed1/event2`
（`PyModule/Infib/Entry` 删除：源语言在 header 的 `lang_tag`，入口在 `F_ENTRY`），
`ret_sig/psigs` 是标量码，容器的签名折叠成 `dyn`；每行的 `ifabi u16` 从线上删除（header 那份是唯一真相）。
`func_table` = `n_funcs u32` + `max_mem_bytes u64`（§7.9，Rust 侧新增模块字段）+ **48 字节/条**
（`code_off,code_len,locals,max_stack,nargs,ret_arity,ret_sig,name_str,sig_str,sig_hash,flags,frame_kind`）。
`line_map`/`exception_table`/`string_pool` 本就逐字节一致，未动。

**指令面（D-38 (b)–(h) + D-39/D-40）**：`guard.nonzero/shift` 去操作数（字节停在 D.3 的
0x27/0x2F，C 的 0x0C/0x0D 由 (2) 迁入）、`ldg/stg` 迁到 0x0E/0x0F、`ldw` 补第 2 操作数并按
`i64(7)/u64(11)/f64(13)` 校验注解、`box.dyn` 去操作数（恒等）、`field_off` 补第 2 操作数并按
`repr(C)`→`R-0713`、字段越界→`R-0706` 校验；**`switch` 解禁**（`CodeBuilder::switch`，长度
`1+4(1+n)`，上限 `MAX_SWITCH_CASES = 48` 与装载器 `insn_len` 同判据；`SwitchNotSupported` 这一
"以拒绝冒充合规"的错误变体已删）。操作数形状补充裁定（D-38 未覆盖）：`new.list ty,count`、
`new.dict kt,vt,count`、`new.frame f`、`typeof.idx T`、`call.virt` 的 kind 改类型索引。
校验面顺带对齐三条：项 2 现在按 `max_stack` 拦高度、项 4 现在按 `ret_arity` 拦 `ret/ret.multi`、
`tailcall.stat` 的帧尺寸比较改成 C 的五元组（locals/max_stack/nargs/ret_arity/frame_kind），
并且按 **D-22** 报 `R-0707` 而不是 `R-0709`（这条本轮才落到 Rust 侧，C 侧 B3 轮已改）。

**`src/btrack.rs` 连动**：`TyKind::Instance`（旧 11）改名 `TyKind::Dyn`（5 处）、
`push_func` 多收 `max_stack`（新）与 `flags: u32`、两处 guard 与一处 `box.dyn` 的操作数清空、
`Selected::Object` 的发射改成按 `op.arity()` 构造操作数并在需要多于一个操作数时**显式拒绝**
（原来非 1 操作数一律发 0 个，D-38 之后会静默发错宽度）。

**新增 13 项 `ifbc` 单测**（模块 6 → **19**）：区段自定界的逐字节偏移、标量行自动置、
签名码折叠与 `sig_hash` 覆盖线上文本、opcode 字节/操作数个数对照表（含家族块 21 行逐个断言）、
`switch` 的变长往返 + 边界检查（正/反例）、发射期拒绝超上限、48 字节记录与 `max_mem_bytes`
往返、`max_stack` 高度正反例、`ldw` 注解域正反例、`field_off` 三态（合法 / 非 `repr(C)` /
字段越界）、`tailcall` 帧不兼容报 `R-0707` 且**不**报 `R-0709`、返回形状与 `ret_arity` 正反例。

**证据（本轮实测）**：`cargo test --offline --lib` **423 passed / 0 failed**（基线 410）、
`cargo check --offline --lib` 0 error / **50 warning**（基线 51，删掉 `CodeBuilder` 的死字段
`bounds`）；`infy check examples/tour` exit 0、`infy build examples/tour` exit 0
（C11 仍 **12513 字节**、产物仍 **199975 字节**、`build-id 4d8250b5…55ba` 未变 ⇒
IFBC 不在非 B 轨路径上，逐字节未受影响）、`--verify-reproducible` exit 0、`infy test examples/tour`
exit 0（`0 passed, 0 failed, 0 total`）、`(cd examples/tour && infy fmt --check)` 仍 exit **2**（F6）。
C 侧零改动仍复跑四条链：`gcc -std=c11 -Wall -Wextra -c runtime/infy_rt.c` **0 告警** exit 0、
rt harness **32335 assertions / 0 failures** exit 0、替身链 IFBC **82/82** exit 0、
生产链 IFBC **82/82** exit 0，两份 stdout `cmp` 无输出。

**文档同步（不留旧声称）**：新建 `docs/internals/ifbc-encoding-rust.md`（`src/ifbc.rs` 的模块注释
一直指向这份"Rust 侧字节表"，**而它此前根本不存在**—— opcode 表由一次性探针从
`Op::from_byte/arity/operand_kinds/stack_effect` 打印生成，不是手抄）；
`runtime/IFBC.md` §2 的标量码表按 `infy_ifbc.c:54` 的 `SIG_*` 改正（原表漏了 `u16/u32`，
把 `u64` 记成 9、`f64` 记成 11，与装载器实际校验值不符；`ldw` 注解说明同错同改）、并在 §1 加
"§1.2/§1.3/§1.5 与 §3.1 是 D4(2) 待改，不是已合规"的状态块；`docs/spec-deviations.md` 追加
**D-40**（D-38 未覆盖的 5 处操作数形状 + 5 处实现选择，含"标量行 `size/align` 留 0，不编造
未量过的常数"）；新登记 BACKLOG **F25**（`new.set` 没有元素个数落点，`effect()` 把类型索引当
弹出个数用）与 **F26**（本侧校验拒绝 `call.virt/call.embed/invoke.dynamic`、
`coro.resume/throw/throw.inf` 的栈效果与 C 的 `sim_delta` 不同 ⇒ 两侧"校验结论等价"不能声称）。

**D4 余项**：(2) C 侧**已于 D4(2) 轮落地**（提交 `e5555dc`：OPS 表按家族重排、exec 按 kind 分派、
push/ldw/field_off 操作数、17 条间隙指令补装载+校验+执行+dumper、三表读取器对齐合并形状、
harness 同批改 82 → 146）→ 剩 (3) 同一镜像 Rust 反汇编 == C 反汇编逐指令相等，然后回 D1 的装载验收。

## D4(2) 轮（2026-10-15）：C 侧对齐唯一线格式，两侧第一次在字节上相遇

改动只在 `runtime/`（`infy_ifbc.c` + `test_ifbc.c` + 新夹具）与 `src/ifbc.rs` 的**测试**；
Rust 侧的编码器、`btrack.rs`、codegen/artifact 一行未动。

- **线格式**：`OPS[]` 换成 D.3 的家族编号（`guard.nonzero`/`guard.shift` 归位 0x27/0x2F 且 0 操作数，
  `add..cmp` 一族一字节 + 操作数 0 = `type_table` 行，旧 `add.f64..cmpeq.f64` 与 0x3E/0x3F 让位）；
  三张表读成合并形状（type 8×u32、import 尾 `sig_hash u32`、func **48 字节**/12×u32）；
  `push` 1→2 操作数、`ldw` 注解与 `is.dyn`/`cast.dyn` 改按行的 **kind** 判定（后者经
  `kind_to_dyn_tag` 表，D-38 重编号后 kind 与 `InfyDynTag` 不再数值相等）。
- **执行**：`fam_path()` 按 kind 选 i64/f64 路径；可执行宽度仍是 i64/f64（其余数值 kind 装载放行、
  执行 R-0706 = 旧表能力限制的原样迁移，`f32` **拒绝而不是按 double 悄悄算**）。
- **间隙指令 17 条**全部进 OPS/校验/反汇编；`ldg/stg/import/seq.push/seq.len/map.insert/str.concat/
  seq.concat/new.tuple/dyn.truthy/dyn.identity` 有执行实现，`import.attr/inst.get/inst.set/seq.iter/
  seq.next/new.set` **装载通过而执行显式返回 R-0710** —— `InfyDyn` 没有属性表/迭代器/集合载体。
  实现选择记在偏差 **D-41**，缺口登记为 **F27**。
- **两侧第一次真相遇**：`runtime/fixtures/ifbc_rust_writer.bin`（1031 字节，Rust 写入器的逐字节固定
  输出，只有 `INFY_REGENERATE_FIXTURE=1` 才重写）由 `test_ifbc.c::test_rust_writer_fixture` 用生产
  装载器读回、过六项校验、并**执行**两个函数（`add_one(41) = 42` 走家族字节 + 类型操作数、
  `pool_str()` 走 `push k,ty`）；Rust 侧 `ifbc::tests::c_loader_fixture_is_exactly_what_the_writer_produces`
  钉"重新生成 == 夹具"与"构建期校验器对这些字节零 R-码"。这是 C3 轮那套跨语言夹具手法的第二份锁点。
- **顺手挡住的两个真实错位**（都是"两侧各自演算同一个表达式"的形态）：
  (1) `MAX_SWITCH_CASES` —— D-40 选择 5 写"取装载器的 `OPS_MAX * 16 = 48`"，而 C 侧那个表达式实际是
  **64**；Rust 发射上限 48 ⇒ 一个 56 case 的 `switch` 两边对"能不能线性扫描"结论相反。C 侧改成显式
  `48u`，锁点 = `SC_SWITCH_HUGE`（49 个 case → C 报 R-0704）。
  (2) `test_ifbc.c` 的 `FDesc` 若用位置初始化把新增的两个 u32 补成 0，那么"读取顺序错一格"照样能装载
  —— 改为 `FD()` 宏在 `sig_hash` 里打**非零哨兵**（`0x5A5A5A5A`），字段顺序一漂移 flags/frame_kind 就脏。
- **另补两条 Rust 已有而 C 缺的结构校验**：`type_table.kind > 32`、`import.kind > 2` 都判 R-0703
  （`decode_types`/`decode_imports` 同判据）。
- 回归（全为本轮实测）：`cargo test --offline --lib` **424 / 0**（`ifbc` 19 → 20）、`cargo check`
  0 error / **50 warning**、`infy check|build|test examples/tour` 全 exit 0 且 12513 字节 C11 /
  199975 字节产物 / `build-id 4d8250b5…55ba` **逐字节未变**、`--verify-reproducible` exit 0、
  `fmt --check` 仍 exit 2（F6）、四条 C 链：`-c infy_rt.c` 与 `-c infy_ifbc.c` 各 **0 告警**、
  rt harness **32335/0**、替身链 IFBC **146/146**、生产链 **146/146** 且两份 stdout `cmp` 无输出。

## D4(3) 轮（2026-10-15）：同一镜像两侧读成**同一份文本**，当场抓到三处分叉

- **做法**：不把人类可读的 dumper 改成人家不能读的形态，而是两侧各加一份"只含双方都保留的字段、
  逐位十进制"的规范化文本（C 侧 `infy_ifbc_dump_canonical` / `infy_ifbc_verify_canonical`，
  Rust 侧 `ifbc::dump_canonical` / `ifbc::verify_canonical`），行语法 `H/T/P/F/I/L/X/S`
  写进 `runtime/IFBC.md` §6.1。**方向分工是关键**：`.bin` 由 Rust 写入器生成
  （`INFY_REGENERATE_FIXTURE=1` 才重写），`.can.txt`/`.ver.txt` 由 **C 侧**渲染
  （`INFY_REGENERATE_CANONICAL=1` 才重写），两侧各自断言同一份文件 ⇒ 任一侧改渲染或改读法都会红，
  没有一侧能单独定义答案。
- **三份夹具**：`ifbc_rust_writer.bin`（1031 字节，D4(2) 那份，另加"两侧**阻塞性**校验结论都为空"）、
  `ifbc_all_ops.bin`（2083 字节，**附录 D 的全部 opcode** 一条一个 `I` 行 —— 17 条间隙指令、`switch`
  的内联 case 表、类型操作数越界时的 `?` 后缀都在内）、`ifbc_verify_matrix.bin`（1890 字节，
  11 个函数、每函数一个缺陷、10 条 `(code, func, off)`：`R-0704 ×2 / R-0705 / R-0706 ×3 /
  R-0707 ×2 / R-0710 / R-0713`，第 11 个函数是 `tailcall` 的目标且刻意干净）。
- **分叉一（改 C 侧，是线宽错误）**：`coro.resume`/`coro.throw` 的 C 侧 `OPS[].nops = 0`，
  本侧 `Op::arity() = 1`，而 D.8 明写 `0xc4 coro.resume h` ⇒ 指令宽度 `1 + 4*nops` 差 4 字节，
  C 侧会把一条协程指令之后的**整条指令流**读歪（`I` 行立刻分两截、把填充字节读成 `nop`/`pop`）。
  C 侧改为 1；执行仍从操作数栈取被 resume 的值，`h` 是与 `ldw` 类型注解（D-38 (e)）同类的
  "装载与反汇编都读、执行不读"的注解。登记在偏差 **D-42**。
- **分叉二（改 C 侧与两份文档，是命名）**：D.9 的 `0xe1/0xe2/0xe3` 规范写 `size_of idx` /
  `align_of idx` / `field_off idx` —— `.idx` 是**操作数**，只有 `0xe0 typeof.idx` 是一个名字。
  C 侧与 `runtime/IFBC.md`、本侧镜像文档 `docs/internals/ifbc-encoding-rust.md` 的表格都在抄
  `.idx`（本侧代码本来就是 `size_of`/`align_of`/`field_off`）⇒ 三处一起改正。
- **分叉三（改本侧，是重复报告）**：`verify_func` 把操作数合法性与 D.3/D.7 邻接检查写在"抽象执行"
  走查里，而那条走查把**直线后继**也 `work.push(next)` 回工作栈 ⇒ 同一条指令被走 2–3 次、
  同一个 `(code, off)` 报 2–3 次（矩阵里 `div` 缺守卫报 3 次，C 侧按边界各查一次只报 1 次）。
  修 = ① 两项检查搬出走查、按 `instrs` 每指令一次（顺带不再漏掉不可达指令，与 C 侧同形）；
  ② 走查不再把直线后继压回栈（高度仍记入 `height`，分支/跳转目标仍由 `push_block` 压）。
  这不是线格式问题，但同样是"两侧对同一镜像的结论不同"。
- **本轮没有让 F26 变成可讲的话**：比较覆盖了两侧**都能表达**的校验类，另有五类结论现在还不可比
  （(a) 动态调用本侧判 `R-0705`、(b) `coro.*`/`throw.inf` 的 δ、(c) 操作数越界本侧在 `decode` 就拒、
  (d) 本侧另要求"函数必须以终止指令收口"、(e) `W-0705` 粒度），逐条写进 D-42；
  `docs/coverage.md` 的"被测对象是生产运行时吗"一行按此措辞。
- 回归（全为本轮实测）：`cargo test --offline --lib` **426 / 0**（基线 424，`ifbc` 20 → **22**）、
  `cargo check --offline --lib` 0 error / **50 warning**（未新增）、`infy check|build|test examples/tour`
  全 exit 0 且 12513 字节 C11 / 199975 字节产物 / `build-id 4d8250b5…55ba` **逐字节未变**、
  `--verify-reproducible` exit 0、生成 C 的 `gcc -std=c11 -Wall -Wextra -I runtime -c` exit 0（0/0）、
  `fmt --check` 仍 exit **2**（F6 未裁决）；四条 C 链：`-c infy_rt.c` 与 `-c infy_ifbc.c` 各
  **0 告警**、rt harness **32335/0**、替身链 IFBC **167/167**、生产链 **167/167** 且两份 stdout
  `cmp` 无输出。

## 仍未实现

`doc`/`lib pack`/`ifbc dump`/`conformance`/`lsp`/`bench`（exit 1）目前明确返回"未实现"而不是假成功
（`infy build` 自 C3 轮起 exit **0**：阶段 [1]–[9] 全通，产物容器可被 `infy artifact info` 与 C 侧运行时读回；
**`infy run` 自 C4 轮起 exit 0/3 真执行**，见上文 C4 一节；**`infy test` 自 C5 轮起 exit 0/1/3 真执行**，
见上文 C5 一节。`bench` 留在这一列是因为 §13.4 根本没有 `infy bench`，它是 `infy test --bench`；
`infy test --cov` 同样显式 exit 1 —— §12.4 的 `-O0` 节点表插桩没有任何阶段产出，F22）；
A 轨绑定生成（`src/atrack.rs`）、§11 `HostApi`（`src/host.rs`）、§12 合规用例集（`src/conformance.rs`）、
`infy doc`（`src/doc.rs`）同样未实现 —— 这四个文件当前是 1 行占位。
HIR（`src/hir.rs`）于 C1 轮落成**声明层**；**语句/表达式级 IFIR 仍没有独立表示**（C2 是 AST→C 直接发射），
因此后端与阶段 [7] 之间没有可优化的中间层。
`codegen` 明确拒绝、没有假成功的构造：闭包、`dyn`、`@embed`（B 轨 D1 / A 轨 D3）、A 轨 `extern` 调用、
`event` 调用、`for`/`defer`/`const_assert`、128 位算术与字面量、窄位宽移位、`overflow = "saturate"`、
含 NUL 的 `str` 字面量、字节串字面量、owned 值的移动（F12）。逐条见 `src/codegen.rs` 的 `CgError` 与偏差 D-26…D-31。

**本实现不声称「INFY v2.0 兼容」**：§12 的 2300 个用例（§12.1 的 1244 + §12.2 的 256 + §12.5 的 800）
目前外部用例为 **0**，逐分类通过率不可测。
