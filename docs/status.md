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
test result: ok. 530 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
$ cargo check --offline --lib
Finished `dev` profile  (0 warnings —— F2 轮把 50 条一次清零：20 条 `unreachable pattern`、6 条 unused import、6 条 unused variable、3 条 `depth` 赋值不读、2 条多余 `mut`、2 条 never-constructed 变体、11 条 never-used fn/method/field；**没有用 `#[allow(...)]` 消数**，其中两处是「没接上」而非「没用」，见下文 F2 一节。口径 = `cargo check --offline --lib --message-format=short` 先 `touch src/lib.rs` 再计数；上一档 50 = F1 轮，51 → 50 = D4(1) 轮删掉 `CodeBuilder.bounds`)
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
IFBC harness: 180 asserts, 180 pass, 0 FAIL      # exit 0，0 编译告警；82 → 146 = D4(2) 合并形状 + 间隙指令 + 跨语言字节夹具；146 → 167 = D4(3) 的三份双侧共用文本夹具；167 → 180 = D1(2) 的第四份夹具 `ifbc_py_unit.bin`（阶段 [6a] 真产物的装载 + 双侧文本）
$ gcc -std=c11 -O1 -Wall -Wextra runtime/infy_ifbc.c runtime/infy_dyn.c \
      runtime/infy_rt.c runtime/test_ifbc.c -o ifbc_real && ./ifbc_real
IFBC harness: 180 asserts, 180 pass, 0 FAIL      # exit 0，0 error / 0 warning；B2 轮真实链生产 infy_rt.c，stdout 与上一行逐字节相同
$ gcc -std=c11 -O1 -Wall -Wextra runtime/infy_rt.c runtime/test_rt.c -o rt_tests && (cd target/scratch && ./rt_tests)
32335 assertions, 0 failures                     # exit 0，0 error / 0 warning；链的是生产 infy_rt.c；C3 轮 +32（Rust 写 ↔ C 读的夹具锁 + D-33）
$ gcc -std=c11 -Wall -Wextra -c runtime/infy_rt.c
（无输出）                                        # exit 0，0 warning（B3 的验收命令；三项语义已于 B3 轮落地，见下文 B3 一节）
$ (cd examples/tour && infy fmt --check)
2 file(s) are not canonical                        # exit 2，见下文"导览源码不是格式化不动点"
$ infy conformance --report                        # E2 轮起：§12.5 要求的逐分类通过率（下例是真实输出的节选）
  table    slug                           req cases  pass  fail block    rate short   分类名（§12 原文）
  §12.1    2-lexical                       96    13    11     0     2    84.6%     83   §2 词法（含 build 约束）  ← 非通过 2 条
  §12.1    1.3-domain-neutral              52     0     0     0     0      n/a     52   §1.3 领域中立  ← 空分类：整行都是缺口，空不等于通过
  §12.5    6.7-derive                      72     3     0     0     3     0.0%     69   §6.7–§6.8 derive  ← 非通过 3 条
  小计 §12.1（9 个分类 / 声明 9 个，其中 0 个未运行）：规范目标 1244 · 用例 46 · Pass 43 · Fail 0 · Blocked 3 · 缺口 1198
  小计 §12.2（3 个分类 / 声明 3 个，其中 0 个未运行）：规范目标 256 · 用例 12 · Pass 12 · Fail 0 · Blocked 0 · 缺口 244
  小计 §12.5（8 个分类 / 声明 8 个，其中 0 个未运行）：规范目标 800 · 用例 25 · Pass 22 · Fail 0 · Blocked 3 · 缺口 775
  合计：用例 83 · Pass 77 · Fail 0 · Blocked 6 · 通过率 92.8% · 规范目标 2300（覆盖 83，缺口 2217）
  空分类（一条用例都没有，所以 §12 这一行完全没有被检查）：1.3-domain-neutral（缺口 52） · 10.9-std（缺口 84） · 11.1-host-protocol（缺口 60）
  非通过用例（6 条 · Blocked ≠ Pass ≠ Fail）：… 逐条打 defect=F34 / F35 / F36 / F37 与「期望 … / 实得 …」首行
conformance report — 83 条用例：Pass 77 · Fail 0 · Blocked 6 ⇒ exit 1
$ echo $?
1                                                  # 退出码裁定 = 偏差 D-48(a)：有 Blocked 或覆盖不足 ⇒ 不是 0
$ infy conformance --category 7.8-ifbc             # 过滤运行只跑 2 条，其余 19 个分类打 `-` / 「未运行」
conformance report — 2 条用例：Pass 2 · Fail 0 · Blocked 0 ⇒ exit 1        # 过滤运行永远不给 0（D-48(d)）
$ infy conformance --update-golden                 # §12.4 要求显式授权；带 --i-know-this-is-breaking 仍拒绝（D-48(e)）
error: `infy conformance --update-golden` 被拒绝：§12.4 要求显式授权（`--i-know-this-is-breaking`），因为改动黄金文件等于改语义。
```

530 项按模块（`cargo test --offline --lib` 逐条分组计数，合计 530；F3 轮实测）：
`lexer` 79 · `derive` 57 · `fmt` 39 · `cli` **39**（C5 轮 16 → D1(2) 轮 20 → D2 轮 28 → D3 轮 33 → E2 轮 38 → **F2 轮 39** = §9.6 `[lints]` 的成对断言） ·
`macro_expand` 35 · `btrack` **31**（D1(1) 前是 0，D1(2) 轮 23 → 30，**F2 轮 +1** = wheel 归档里每个成员按自己的 local header 偏移取数据） ·
`atrack` **27**（D3 轮新建，此前是 1 行占位） · `codegen` **26**（D3 轮 22 → 26） · `pkg` 24 · `sema` 22 ·
`ifbc` **22**（D4(3) 轮 +2 = 双侧文本锁点） · `hir` 20 · `artifact` 18 ·
`toml` 15 · `borrow` 15 · `sha256` 12 · `conformance` **18**（E1 轮新建 0 → 11，**E3 轮 +3** = 文档表的逐格回核 + §12.4 红线的机检 + 四份文档各须重申红线，**F1 轮 +3** = `.github/workflows/ci.yml` 的命令覆盖面 / 四个重写夹具的开关绝不设 / Windows-only 与两条防假红守卫，**F3 轮 +1** = `.gitignore` 的一次性产物名单形状（按"谁产生它"列的六条一条不能少）+ 反向红线（不得宽到吞掉 `*.bin`/`*.md`/`*.rs` 或整个目录）+ 每个 `scripts/*.ps1` 的审计日志必须落在 `logs/`） · `codes` 9 ·
`parser` **9**（D3 轮 +1 = §3.6 的 `@c` 语句形态） · `base64` 7 · `stdlib` 4 · `diag` 2 = 530。

- 词法、语法、`fmt`(§9.5 十四条规则)、`macro_expand`(§6.1–§6.5)、`derive`(§6.7)、诊断码表、
  类型检查（§4.1/§4.2/§4.3/§4.6/§2.8/§4.13/§8.2）、包与依赖（§5.2/§5.3/§5.7/§5.8）、
  借用检查（`E-1001`/`E-1002`/`E-1004`/`E-1005`/`E-1006`/`E-0121` 正反例）、哈希与 base64 与 TOML 往返、
  `infy` CLI 自身、IFBC Rust 侧编码/解码/反汇编：都有上表的测试。
- **仍没有一项独立单测**：仍是 1 行占位的 `host.rs`/`doc.rs`（`atrack.rs` 的占位状态在 D3 轮结束，27 项；`conformance.rs` 的在 E1 轮结束，F1 轮到 **17** 项 + 83 条外部用例）。
  `btrack.rs`（§7.7 B 轨）的零单测状态在 **D1(1) 轮**结束（23 项，D1(2) 轮 → **30** 项，锁 §7.7.1 探针、§7.7.2 五个扫描项、
  §7.7.3 版本锁、附录 E.3 表语法与表体 hash、§7.8.3 的降级自检），它揪出并修掉的三个缺陷见下文 D1(1) 一节。
  `codegen.rs`（§7.1 阶段 [8]）在 C2 轮从 1 行占位变成独立单测模块（C4 轮 **19** 项 = `goto` 必须转移控制那一条；C5 轮 **22** 项 = §5.9 测试 harness 的 3 条；D3 轮 **26** 项 = A 轨调用走包装符号、没有 `#[link]` 配对的声明被点名拒绝、`()` 返回的 `void` 转发、块状尾表达式的结果临时量作用域），`hir.rs` 的 20 项锁阶段 [7]，
  `artifact.rs`（§7.1 阶段 [9] / §7.3 / §7.4）在 C3 轮新建并直接带 18 项单测 + 一份被 C 侧读取器消费的逐字节夹具，
  `cli.rs` 从 C4 轮的 9 项到 C5 轮的 16 项、D1(2) 轮的 **20** 项、D2 轮的 28 项、D3 轮的 33 项、**E2 轮的 38 项**（`infy test` 的三条端到端 + §5.9/§2.2 的文件选择两条 + §5.8 dirhash 不随构建范围变的一条 + `--cov` 拒绝 + [6a] 经命令行的摄取四条 + `infy ifbc dump|verify` 八条 + A 轨的端到端/绑定文件/三条判据五条 + §12 逐分类报告五条）。
  `atrack.rs`（§7.6 A 轨）在 **D3 轮**从 1 行占位新建并带 **27** 项：配对两个方向、ABI/前缀支持面、`infy_e_<name>_<8hex>` 的哈希规范化、§7.6.4 封闭表逐行、UB-010 移交四态、绑定头与包装 TU（含一次真 `gcc -std=c11 -Wall -Wextra` 编译）。
- 先前记为"§9.2 字节级金样"实际是 1 条规范样例逐字节 + 1 条 caret 行规则（`diag` 2 项）；
  `goto` 的三条 `E-0301` 越界路径与 `E-0121` 在 A4 轮之前无测试（**A4 轮已补**，见下）。

**绿灯的含义要如实限定**：`cargo test --lib` 全绿只说明"已写的模块自洽"，不等于 §12 合规。
`infy build` **自 C3 轮起跑完 §7.1 阶段 [1]–[9]**：前端 → HIR → C11 → 用 A 轨 `cc` 链成宿主镜像 →
追加 §7.4 段容器与 64 字节段尾，`infy artifact info` 读得回自己刚产出的文件（P4/P13/P14），故 exit **0**。
阶段 [10]（sumdb 校验与运行时自校验比对）仍**未做**，`.inf sum` 只是把 `MODULE.infy.lock` 的 dirhash 清单抄进容器；
`doc`/`lib pack`/`bench` 仍显式报"未实现"（exit 1）；**`infy conformance --report` 自 E2 轮起输出 §12.5 要求的逐分类通过率**（83 条用例 = 77 Pass / 6 Blocked / 0 Fail、覆盖 83/2300 ⇒ 仍不是合规声称，见下文 E2 一节与 P29）；**`infy ifbc dump|verify` 自 D2 轮起可用**（§7.8.4，见下文 D2 一节；今天真产物的 `.inf ifbc` 恒空 = F29，命令对此明打为空而不假称校验过）；**`run` 自 C4 轮起真执行**（§13.4 的
开发态 = [7]-[9] 建到临时目录 + 立即执行，退出码按 §13.4，见下文 C4 一节）；**`test` 自 C5 轮起真执行**（§5.9 的
测试构建 + 生成的 `#[test]` harness，退出码 0/1/3 按 §13.4，见下文 C5 一节；`bench` 留在未实现里是因为 §13.4
根本没有这个子命令，它是 `infy test --bench`）；
B 轨 `src/btrack.rs` 的**库内**链路（探针→扫描→降级→本侧装载自检）已于 **D1(1) 轮**跑通并带 23 项单测，**D1(2) 轮**接进 CLI（阶段 [6a]）并把镜像交给 C 侧装载（30 项单测 + `ifbc_py_unit.bin` 夹具）⇒ D1 已勾；
**D2 轮**再把 §7.8.4 的 `infy ifbc dump|verify` 命令接上（`cli` 20 → **28**，双侧文本第一次在命令层比对）。
B 轨剩下的缺口在**产物那半边**：阶段 [8] 没有通往 IFBC VM 的调用通道 ⇒ `.inf ifbc` 恒空、含 `@py` 的 module 走不到 [9]（**F29**），§7.7.1 样例本体也被能力表红着拒（**F30**）；
C 侧 harness(`runtime/test_rt.c`) **B1 轮已收敛**（C3 轮复跑 **32335 断言 / 0 失败**，直接链生产 `infy_rt.c`）；
`src/host.rs`、`src/doc.rs` 是 1 行占位文件（`src/conformance.rs` 的占位状态在 E1 轮结束，见下文 E1 一节）（`src/artifact.rs` 已于 C3 轮从"不存在"落成 §7.4 容器写入器 + 读取器 + §7.3 build-id，18 项独立单测）（`src/hir.rs` 已于 C1 轮落成声明层 IR，不再是占位）（**§11 `HostApi` 之前被 README 标成 ✅，是文档错误，本轮已改为未实现**）；
A 轨只落了 `@c` + `cc` + 标量/指针那一半（§7.6.2 工具链注册、§7.6.5 反向导出、§7.6.6 进程隔离未实现 = F32/F33）、`std.net` TLS、`infy lsp` 未实现。
IFBC 断言（B3 轮 79 → 82，D4(2) 轮 **82 → 146**：合并线格式 + 17 条间隙指令 + Rust 写↔C 读执行的夹具；
D4(3) 轮 **146 → 167**：三份双侧共用文本夹具，D1(2) 轮 **167 → 180**：第四份 = 阶段 [6a] 的真产物）
**B2 轮起既链 `runtime/test_ifbc_stubs.c`（harness 替身）也链生产 `infy_rt.c`**，两侧同为 **180/180**（D1(2) 轮复跑）、
stdout 逐字节相同。**E3 轮已落**的是 §12 逐分类通过率的**对外表格**（`docs/coverage.md` §6 + 根 README 的「距 §12 的 100% 还差哪些类」，逐格由 `conformance::tests` 回核）。仍未验证的是：§12.3 的黄金文件更新（`--update-golden` 两分支都拒 = D-48(e)）与 §12.4 的等价性矩阵（四档一档都跑不了），以及 B 轨的**产物半边**（D1/D2 已把摄取与检视命令接上，
缺的是阶段 [8] 通往 IFBC VM 的调用通道 ⇒ `.inf ifbc` 恒空 = **F29**）。Rust↔C 的**逐指令反汇编相等**已于 D4(3) 轮落地（偏差 D-42），D2 轮起这份相等也在 CLI 层比对；"两侧校验结论等价"另被 F26 挡着。

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
`src/btrack.rs` 的端到端 lowering（D1 —— D1(1) 轮已把探针/扫描/降级测到位，剩 CLI 接线与 C 侧装载夹具）。
`runtime/test_rt.c`（B1）已于 B1 轮收敛。已完成的 C 侧里程碑不变：`ifbc-vm`(`runtime/test_ifbc.c`) **180** 项断言通过（B3 轮 79 → 82，D4(2) 轮 → 146，D4(3) 轮 → 167，D1(2) 轮 → 180），
含六项装载校验、逐字节截断、单字节变异、800 轮 fuzz；`codes`、`infra`(哈希/TOML) 已交付。
（A3 轮当时的限定"这些项链的是替身不是生产运行时"已于 **B2 轮解除**：同一 harness 现在也链生产
`runtime/infy_rt.c` 跑通 180/180，见上文 P11。）

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
`.inf ifbc` 恒为空（D1）；§7.2 的 LTO 编排与 `infyl` 独立链接器驱动（A 轨 `@c` 的 `.c` 自 D3 轮起已进链接命令）；
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

## D1(1) 轮（2026-10-15）：B 轨「探针 + 能力扫描」段收敛 —— `btrack` 0 → 23 项单测，修掉三个将 D1 整条路挡死的缺陷

本轮取 BACKLOG §1 第一个 `[ ]` = **D1**，按 §2 指针的建议拆成两段中的第一段（探测 + 能力扫描）；
`src/btrack.rs` 是 4476 行 / 75 个 `pub fn` / **0 项单测**的截断模块，做法是**收敛而不是重写**：
先补测试，再用一次性探针把 CPython 自己的 `dis` 形态打出来对照。

**测试一上手就红的 10 项，根因是实现不是测试**。三条根因（全部有回归锁点）：

1. **`arg:` 派发行的分支值 `-` 被判成非法助记符**（`parse_single` 只在顶层认 `-`/`ignore`）。
   后果是 `BINARY_OP`/`CALL`/`COMPARE_OP`/`CONTAINS_OP`/`IS_OP`/`LOAD_ATTR`/`STORE_ATTR` **整行**
   解析失败 → 扫描按"该 opcode 未映射"报 `E-0712`，即任何含算术/比较/属性/调用的 `@py` 单元
   都过不了 §7.7.2，`lower_named` 永远进不到发射 —— **D1 被这一条挡死**（py-3.14 表 154 行里 7 行、
   py-3.13 表 14 行、lua 表若干行受影响）。修 = `-`/`ignore` 在任何位置都是**值**（E.3 头部的
   `value := "-" | "ignore" | mnemonic | …` 本来就是这意），记偏差 **D-43**(a)。
2. **CPython 3.12+ 把标志位打包进 `co_names` 索引**：`LOAD_GLOBAL` 的 `arg = namei*2 + do_call`、
   `LOAD_ATTR` 的 `arg = namei*2 + method`。代码按原值取 `name_at(arg)` ⇒ 单元里只有一个全局名时
   **越界**（`scan` 静默 `continue`，`E-0711`/`E-0712 builtin` 分支从不触发），有两个名字时**解析成邻居**
   （`input` 被当成 `eval`）。本机实测：`input()` → `LOAD_GLOBAL arg=1 names=['input']`；
   `x.foo`→0、`x.bar`→2、`x.bar()`→1。修 = 新增 `CodeObj::operand_name()` 统一解包（`>> 1`），
   扫描项 2 与 lowering 的 `LOAD_GLOBAL`/`LOAD_ATTR`/`STORE_ATTR`/`IMPORT_FROM` 四处共用它，记 **D-43**(b)。
3. **`IMPORT_NAME` 的参数是 `co_names` 索引，代码从 `co_consts` 取**（`PyInstr::const_name`，其文档注释
   自称服务 `IMPORT_NAME`/`LOAD_ATTR`，两者其实都是 names 索引）⇒ 扫描项 3 **从不触发**：
   `import numpy` 的 violations 是**空**（deny 段形同不存在），lowering 还会把模块名 intern 成空串。
   修 = 删掉 `const_name`，统一走 `operand_name`，记 **D-43**(c)。

顺带把 `CapTable::load` 里"按 E.3 规则重算 `table_hash`"的换行分隔符从**跨行字符串字面量**改成
`"\n"` 转义 —— 这台机 `core.autocrlf=true`，字面量里的裸换行会随检出变成 `\r\n`，让 hash 对不上；
`each_table_hash_matches_the_body_it_claims_to_cover` 钉住三份表的 hash 不变（本机实测仍 == 表内声明值）。

**新登记的规范符合性缺口 F28**：附录 E.3 规定 `[opcode]` 行以**数值键**（`0xNN`）为规范式并附 `OPNAME`
别名，而 shipped 的三份表里只有 `IFBC-CAP-py-3.14.toml` 有数值键（154 行 + 别名，逐行两式同值由
`the_number_keyed_table_agrees_between_its_number_and_name_rows` 钉死）；`IFBC-CAP-py-3.13.toml` 与
`IFBC-CAP-lua-5.4.toml` **0 行数值键** ⇒ `check_alias_agreement` 对这两张表永远不会触发，也无法与
`dis.opmap` 对账。本机没有 3.13/lua 解释器，**数值不能凭记忆填**（§12.2 禁编造常数），故只登记不补数。
`the_name_keyed_tables_still_map_every_row` 里有一条**会变红的断言**：一旦有人补上数值键，它要求把该表
移进数值键那条测试并消掉 F28。

**23 项测试**（每项的条款写在 doc comment 里）覆盖：§7.7.1 探针往返 / `unit_code()` 取的是嵌套 wrapper
而不是 `<module>`、§7.4 行映射、§7.7.1 规则 1 的 `wrap_embed_unit` 重缩进与 `unit_signature` 优先取声明、
§7.7.2 五个扫描项（含 `BINARY_OP(**)` 这种**子操作**级点名、`E-0711 builtin(eval)` 不被表的 `eval = "-"`
降级、`<lambda>` 里的缺口仍报且 `code_name` 指明所在 code object、异常表 `push/setup/with/bare` 四分类、
扫描列出**全部**缺口且有序）、§7.7.3 版本锁双向（声明 3.13 而本机 3.14 ⇒ `E-0713`，声明实际版本 ⇒ 放行；
`.pyc` magic 与补丁级归一；无声明 ⇒ `E-0715`）、§7.7.4 import 三判 + 真 dump 的 `import(numpy)` 拒绝、
附录 E.3 语法与表体 hash、以及 §7.8.2/§7.8.3 的"一个已覆盖单元 lowering 成镜像、过本侧装载自检、
`encode` ↔ `dump_canonical` 往返"。

**明确未做**（不用假成功掩盖，D1 保持 `[ ]`）：阶段 [6a] 仍**没有任何 CLI 调用点**（`btrack` 只被测试与
`artifact::crc32` 引用；`infy check/build` 不跑摄取，`@py` 单元到阶段 [8] 仍是 codegen 的显式拒绝）；
产出的镜像还没交给 C 侧 `runtime/infy_ifbc.c` **装载并执行**（那是 D1 验收第 1 项，做法 = 新增一份
`@py` 夹具挂进 D4(3) 的双侧文本锁点）；`.inf ifbc` 仍恒空。下一轮从这三条继续。

**回归（全部为本轮实测，非引用）**：`cargo test --offline --lib` **449 passed / 0 failed**（基线 426，+23 全在 `btrack`）、
`cargo check --offline --lib` 0 error / **50 warning**（未新增）、`infy check examples/tour` exit **0**（仍 2 files）、
`infy build examples/tour` exit **0**、`--verify-reproducible` exit **0**（199975 == 199975）、
`infy test examples/tour` exit **0**（程序 stdout 42 字节 `test result: 0 passed, 0 failed, 0 total`）、
`(cd examples/tour && infy fmt --check)` 仍 exit **2**（F6 未裁决，未变）。**C 侧本轮零改动**（没动 `.c`/`.h`/夹具），
四条链仍按纪律复跑：`gcc -c runtime/infy_rt.c` 与 `-c runtime/infy_ifbc.c` 各 **0 告警** exit 0、
rt harness **32335 assertions / 0 failures** exit 0（在 `target/scratch` 里跑）、替身链 IFBC **167/167**、
生产链 IFBC **167/167**、两份 stdout `cmp` **无差异**。

## 仍未实现

`doc`/`lib pack`/`lsp`/`bench`/`ingest`（exit 1）目前明确返回"未实现"而不是假成功
（`infy ifbc dump|verify` **自 D2 轮起可用**：§7.8.4 的检视命令，输入 = §7.4 产物或裸 IFBC 镜像，
输出 = 反汇编 / `--canonical` 的双侧共用文本 / §7.8.3 的校验结论，见下文 D2 一节。它今天能检视的镜像来自
`runtime/fixtures`，而**真产物的 `.inf ifbc` 恒空**（F29），命令对此明打"0 个 B 轨模块"而不假称校验过；
`infy build` 自 C3 轮起 exit **0**：阶段 [1]–[9] 全通，产物容器可被 `infy artifact info` 与 C 侧运行时读回；
**`infy run` 自 C4 轮起 exit 0/3 真执行**，见上文 C4 一节；**`infy test` 自 C5 轮起 exit 0/1/3 真执行**，
见上文 C5 一节。**`infy conformance --report` 自 E2 轮起输出 §12.5 要求的逐分类通过率**（退出码裁定 = 偏差 D-48，见下文 E2 一节；83 条用例只覆盖 §12 目标的 3.6%，所以那份通过率不是合规声称）。`bench` 留在这一列是因为 §13.4 根本没有 `infy bench`，它是 `infy test --bench`；
`infy test --cov` 同样显式 exit 1 —— §12.4 的 `-O0` 节点表插桩没有任何阶段产出，F22）；
A 轨的 §7.6.2 工具链发现/注册与 `recipe.toml` 审计（F32）、§7.6.5 反向导出与 §7.6.6 进程隔离（F33）、§11 `HostApi`（`src/host.rs`）、
`infy doc`（`src/doc.rs`）同样未实现 —— 这两个文件当前是 1 行占位（`src/atrack.rs` 的占位状态在 D3 轮结束、`src/conformance.rs` 的在 E1 轮结束，`infy conformance` 的命令行在 E2 轮接上）。
§12 还缺的不是表格而是**量与实现**：逐分类通过率已落成 `docs/coverage.md` §6 与 README 的缺口表（**E3 轮**），
而 §12 要求的 2300 条里这里只有 83 条，三个分类结构性空着（F38 / F19 / `host.rs`），§12.4 的等价性矩阵一档跑不了。
HIR（`src/hir.rs`）于 C1 轮落成**声明层**；**语句/表达式级 IFIR 仍没有独立表示**（C2 是 AST→C 直接发射），
因此后端与阶段 [7] 之间没有可优化的中间层。
`codegen` 明确拒绝、没有假成功的构造：闭包、`dyn`、`@embed` 的 B 轨半边（F29）、A 轨 §7.6.4 未发射的那五类跨边界类型（F31 / 偏差 D-46(e)）、A 轨 `extern` 调用、
`event` 调用、`for`/`defer`/`const_assert`、128 位算术与字面量、窄位宽移位、`overflow = "saturate"`、
含 NUL 的 `str` 字面量、字节串字面量、owned 值的移动（F12）。逐条见 `src/codegen.rs` 的 `CgError` 与偏差 D-26…D-31。

**本实现不声称「INFY v2.0 兼容」**：§12 的 2300 个用例（§12.1 的 1244 + §12.2 的 256 + §12.5 的 800）
这里只有 **83 条**（E1 轮搬运，E2 轮起 `infy conformance --report` 打逐分类通过率，E3 轮落进 `docs/coverage.md` §6），
覆盖 83/2300 ≈ 3.6% ⇒ 不声称。**这条在本文件里挂了三轮的旧话（"外部用例为 0、通过率不可测"）由 E3 轮改正** ——
它是 A3 那轮写下的、E1/E2 之后就没再对回证据的文档谎报，同族认法见经验库 `infy-doc-claims-vs-evidence`。

## D1(2) 轮（2026-10-15）：阶段 [6a] 接进 CLI，C 侧第一次装载一份 `@py` 的真镜像

**这一轮做的事**：D1 的余项 (a)(b) —— 不是重写 `btrack`，是把已经存在的那 4476 行**第一次从命令行走进去**，
再把它产出的字节交给对面那台虚拟机读。D1 因此勾掉（验收四项全有实测锁点）；余项 (c)（`.inf ifbc` 恒空）
登记为 **F29**，接线时新发现的"规范样例本身不能构建"登记为 **F30**。

**(a) 阶段 [6a] 的接线面**（`src/cli.rs`）：`frontend()` 在 `analyse()` 之后、诊断渲染之前调 `ingest_b_track`，
且**只在 [1]-[5] 没有阻断诊断时**运行（§7.1 的"任一阶段出现 `E-` 即中止后续阶段"与 spec:1051 的
"`infy check` 也要跑 [6a]"两条一起决定的位置）。采集器 `embed_sites` / `sites_of_item` / `embeds_in_body|block|stmt`
走 §3.6 的两个合法位置（整体 body 与语句表达式），块嵌套全覆盖（`if`/`while`/`loop`/`for`/`match`/`unsafe`/`defer`
与 `else if` 链）。形参表按 §7.7.1 规则 1 从 **INFY 侧**类型构造：
`btrack::embed_ty_kind(&sema::Ty) -> Option<ifbc::TyKind>`（`i64→Int(7)`、`f64→Float(13)`、
`String` 与 `str` 同为 `Str(14)`、共享引用透传到被指向的类、容器类折成 `List`/`Tuple`/`Option`/…，
而 `u128`/`i128`/`auto`/未定型 **返回 `None` ⇒ 拒绝**，不折成 `dyn` 也不回落 `()` ——
折或回落都会静默改变 §7.8.3 校验项 5 比较的那份摘要）。镜像存在 `Loaded::embed_images`
（`{unit, image, source_toolchain, n_funcs}`），`infy check` 每份打一行 `ingest ok —`。
元数据实参名闭集 `{name, timeout_ms, isolation}`、单元默认名 = 所在 item 名、解释器不在场报 `E-0713`、
前缀未实现与类型无标量码报 `E-0712` ⇒ 全部写在偏差 **D-44** 的 (a)–(h)。

**接线当场现形的三条实现缺陷**（各带回归锁点；三条都是"代码在、但从没被走到"那一族）：
| # | 缺陷 | 后果 | 修法与锁点 |
| --- | --- | --- | --- |
| (1) | `build_embed_unit` 调 `run_python_probe`，那里固定问探针要 `__infy_unit__`，而 `wrap_embed_unit` 写的是 `def <unit_name>` | `unit_code()` 名字匹配不上就**静默回落到模块 code object** ⇒ 扫的是包装器的 `MAKE_FUNCTION` 流，单元体里的 opcode / builtin / import **一个都没进扫描**，产出的镜像也不是那个函数。表现不是报错，是"一张看似正常的错镜像" | 改调 `run_python_probe_named(python, &src, unit_name)`；`btrack::tests::the_declared_table_reaches_the_function_record` 断言 `m.unit` 与"只有一条函数记录" |
| (2) | `sema` 的 `ExprKind::Embed` 臂无条件返回 `Ty::Unit` | §7.7.1 自己的样例 `event normalize(…) -> Vec<f32> { @py(…) { … } }` 在阶段 [4] 就吃 `E-0110 返回值类型不匹配：期望 Vec<f32>，实际 ()` ⇒ **阶段 [6a] 从 CLI 永远到不了**（整条 D1 被这一条挡在门外） | 该臂返回 `self.ret_ty.clone()`（§7.7.1 规则 2 的推论：单元的 `return` 就是所在 item 的返回值）；`cli::tests::check_ingests_a_py_unit_and_keeps_the_declared_table` 是它的端到端锁点 |
| (3) | §7.7.3 的"patch 不敏感"只做在版本比较里：`py = "3.14.6"` 时 `check_probe_version` 放行，`CapTable::load("py", "3.14.6")` 却去找 `IFBC-CAP-py-3.14.6.toml` | 假 `E-0712 能力表无法读取` —— 声明合法、表名带补丁号就查不到 | 表名按 `dump.py_minor()`（真正编译了该单元的那个 minor）取；`btrack::tests::a_patch_level_declaration_still_finds_the_minor_capability_table` |

**(b) C 侧装载**（`runtime/test_ifbc.c`，+13 断言 = **167 → 180**）：新夹具
`runtime/fixtures/ifbc_py_unit.bin`（**1062 字节**，方向分工不变：`.bin` 只有 Rust 侧
`INFY_REGENERATE_FIXTURE=1` 能重写、`.can.txt`（1144 字节）只有 **C 侧** `INFY_REGENERATE_CANONICAL=1` 能重写）。
它和前三份夹具的区别是**它不是手写形状**：字节来自 `ingest_embed_site` 把 §7.7.1 的 `normalize` 单元
（声明表 `f64, u32 -> f64`，体 `return scale * 2.0`）经探针 → §7.7.2 扫描 → lowering → `ifbc::encode` 得来。
`test_py_unit_fixture` 用**生产装载器** `infy_ifbc_load(..., verbose=1, rep)` 读它 ⇒ `rc == 0` 就是
§7.8.3 六项校验全过；再从本侧渲染的规范化文本里 `sscanf` 回 `F` 行，断言 **arity 2 / ret_sig 13(f64) /
ret_arity 1** —— 声明表穿过 探针 → 扫描 → lowering → 编码 → 虚拟机 五段没变形；另断言 `L` 行在场
（§7.4 的原始语言行映射，本例 13 条全指向 Python 第 2 行）与"装载不泄漏"（`live_objs()` 回原值）。
**不声称执行**：B 轨帧的执行通道属阶段 [8]（**F29**），属性访问/迭代那半还被 **F27** 挡着。
Rust 侧 `btrack::tests::py_unit_fixture_is_exactly_what_the_ingest_produces` 断言"重新生成 == 夹具"，
`ifbc::tests::canonical_render_equals_the_c_loader_render` 把这份夹具加进同一份文本的比对列表。

**F30（规范样例本身不能构建，只登记不代规范改）**：§7.7.1 的样例体是推导式
`out = [ (x / scale) for x in batch ]`，本机实测 `infy check` 报
`E-0712 字节码能力缺失：RERAISE`（扫描项 1 · `co_code+0x3c` · 原始语言行 3），
另有一条同族：形参 `f32` 与无后缀 `2.0` 混宽 ⇒ `操作数类型 [f32, f64] 不满足能力表给出的任何映射目标`。
这正是 §7.7.2 设计的行为（"没实现的东西在构建期就红着退出"），所以**没有**为了让样例过而放宽扫描；
但 §14 的导览与 §12.2 的合规用例都默认这段能过，属文档级矛盾，留给能力表扩容（`RERAISE` /
`GET_ITER`·`FOR_ITER`·`END_FOR` 的收尾形态，前置是 F27 的迭代器载体）。
锁点 = `btrack::tests::the_list_comprehension_sample_is_refused_by_name_not_silently`（点名 `RERAISE` 且带行号）。

**回归（全部本轮实测，非引用）**：`cargo test --offline --lib` **460 passed / 0 failed**
（基线 449，+7 `btrack` = 23 → **30**、+4 `cli` = 16 → **20**）、`cargo check --offline --lib`
0 error / **50 warning**（未新增）、`infy check examples/tour` exit 0 且 **stdout 逐字节未变**
（无 B 轨单元 ⇒ [6a] 一行不打，`cli::tests::a_module_without_embeds_produces_no_images` 钉住 P6）、
`infy build examples/tour` exit 0（生成 C11 仍 **12513 字节**、产物仍 **199975 字节**、`blob 738`、
`build-id 4d8250b5…55ba` —— 阶段 [6a] 不参与 [9] 的输入，(c) 未做）、`--verify-reproducible` exit 0、
`infy artifact info … --nodes` exit 0、`infy test examples/tour` exit 0、
`(cd examples/tour && infy fmt --check)` 仍 exit **2**（F6 未裁决，未变）；
**C 侧只动 `test_ifbc.c`**（`infy_ifbc.c` / `infy_rt.c` / 冻结头零改动）仍按纪律复跑四条链：
`gcc -std=c11 -Wall -Wextra -c runtime/infy_rt.c` 与 `-c runtime/infy_ifbc.c` 各 **0 告警** exit 0、
rt harness **32335 assertions / 0 failures** exit 0（在 `target/scratch` 里跑）、
替身链 IFBC **180/180**、生产链 IFBC **180/180**、两份 stdout `cmp` **无差异**。

## D2 轮（2026-10-16）：`infy ifbc dump|verify` 接进 CLI，双侧文本第一次在**命令层**比对

§13.4 列了 `infy ifbc dump|verify <artifact>`、§7.8.4 承诺"提供 … 用于诊断与合规测试（§12.3）"、§12.3 又把
`infy ifbc dump` 当黄金文件 —— 而这条命令此前落在 `cli.rs` 的"未实现"臂里（exit 1 的 notice）。
实质件全在库里（`ifbc::disassemble` / `dump_canonical` / `verify_canonical` / `verify` ↔ C 侧同名件），
本轮只补命令这一层，没有再造第二套渲染器。落地面只有 `src/cli.rs`：`IfbcInput`/`IfbcOpenErr`/`classify_ifbc`
（先按 §7.4 容器读、再按裸镜像读）/`ifbc_dump_text`/`ifbc_verify_image`/`ifbc_verify_canonical_text`/
`ifbc_empty_region_text`/`ifbc_cmd`，加进 `run()` 的派发与 `usage()`，并从"未实现"臂里摘掉 `ifbc`。

**实测（P22/P23/P24，全部是本轮真实命令输出）**
- `infy ifbc dump --canonical runtime/fixtures/ifbc_{rust_writer,all_ops,py_unit}.bin` 三份输出与 **C 侧**渲染的
  `.can.txt` 按原始字节 `cmp` 无差异，exit 0；不带 `--canonical` 的人类可读形态只锁"同一份字节两次渲染相同"
  （C 侧 dumper 是另一套行格式 = D-42，没有跨语言字节锁）。
- `infy ifbc verify runtime/fixtures/ifbc_verify_matrix.bin` 默认报 **1** 条（`R-0704: func #0 @ 0x0005: switch 的
  某个 case 目标 0x0007 不是本函数内的指令边界`）—— D.10 的"默认只报首个，保持与运行时一致"；
  `--all-errors` 报 **10** 条（`R-0704 ×2 / R-0705 / R-0706 ×3 / R-0707 ×2 / R-0710 / R-0713`）；
  `--canonical` 的 10 行 `V …` 与 C 侧 `.ver.txt` 逐字节 `cmp` 无差异。三条都是 exit **2**。
- `ifbc_rust_writer.bin` 与 `ifbc_py_unit.bin`（= [6a] 的真产物）各 **0** 条阻断 ⇒ exit 0。
  **不据此讲"两侧校验等价"**：`ifbc_all_ops.bin` 本侧报 **26** 条而 C 侧不报同一集合（F26 前半 / D-42 (a)–(e)），
  命令的输出里直接把这句话打成 note。
- 真实产物 `examples/tour/.infy-build/x86_64-pc-windows-gnu/ci.exe`：`dump` 与 `verify` 都 exit 0 并明打
  `ifbc … — .inf ifbc 为空（0 个 B 轨模块 / 区域 0 字节）：没有可反汇编/校验的字节码` + 点名 F29 与
  "不声称校验过产物里的 B 轨字节码"（F29 是 D2 开工前就接受的前提，不绕）。
- 既无 `INFYART2` 段尾又无 `IFB1` 魔数的文件（`README.md`）→ `E-0605` + exit **7**，且**两条**拒绝理由都打；
  不认识的子命令 / 缺文件 / 读不到 → exit **1**。

**规范未定 → 选定实现，记偏差 D-45 (a)–(e)**：输入形态（容器或裸镜像；`.inf ifbc` 只接受恰好 1 个模块，
`n_ifbc_modules != 1` **点名拒绝**而不去猜一种 §7.4 没定义的分帧）、`--canonical` = D4(3) 那份双侧文本且
此时 stdout 必须纯（表头/状态行走 stderr，否则 `cmp` 无从谈起）、拒绝的退出码（§7.8.3 的阻断结论 → 2、
读不出的文件 → 7、用法错 → 1）、host 表用 `HostSigTable::std_default()`（`src/host.rs` 是 1 行占位 ⇒
今天校验的是"镜像对 std 表"，不是"镜像对该产物注册的 host 表"）、`--all-errors` 的"首个"裁成首个**失败函数**。

**测试**：`cli` **20 → 28**（八项：三份 `.can.txt` + 一份 `.ver.txt` 的 CLI 级锁、默认 vs `--all-errors` vs
`--canonical`、空区那两行**文字**、1 模块容器的区读回 == 写入字节、多模块点名拒绝、既非产物又非镜像）。
容器那两条是本轮唯一需要现造的形状 —— 今天没有任何真实构建会写出非空 `.inf ifbc`，所以测试用
`artifact::write` 造容器（`container_with_ifbc`），把命令的产物路径钉住，等 F29 落地时不必再补这一层。

**回归（全部本轮实测，非引用）**：`cargo test --offline --lib` **468 passed / 0 failed**（基线 460，+8 全在 `cli`）、
`cargo check --offline --lib` 0 error / **50 warning**（未新增）；`infy check examples/tour` exit 0、
`infy build` exit 0（生成 C11 仍 **12513** 字节、产物仍 **199975** 字节、`build-id 4d8250b5…55ba` 逐字节未变 ——
本轮没碰 [7]-[9]）、`--verify-reproducible` exit 0、`infy run`/`infy test examples/tour` exit 0、
`infy artifact info … --nodes` exit 0、生成 C 的 `gcc -std=c11 -Wall -Wextra -I runtime -c` exit 0（0/0）、
`(cd examples/tour && infy fmt --check)` 仍 exit **2**（F6 未裁决，未变）；
**C 侧零改动**（`.c`/`.h` 一行没动）仍复跑四条链：`gcc -c runtime/infy_rt.c` 与 `-c runtime/infy_ifbc.c` 各
**0 告警** exit 0、rt harness **32335 assertions / 0 failures** exit 0（在 `target/scratch` 里跑）、
替身链 IFBC **180/180**、生产链 IFBC **180/180**、两份 stdout `cmp` **无差异**。

## D3 轮（2026-10-16）：`src/atrack.rs` 落成 A 轨绑定的唯一真值源，最小 `@c` 单元第一次被编译、链接、在产物里调通

**这一轮补的是 §1 阶段 D 里最后一块"从零写真东西"的地**：`atrack.rs` 是 1 行占位（不像 `btrack` 那样有
半成品可收敛），而验收命令要求**真链接** —— `E-0723` 在全仓库**零发点**（只有 `codes.rs` 的码表行），
`sema.rs:974` 的 `in_extern_block()` 是恒 `false` 的占位，`sema.rs:1988` 的 `E-0721` 判据写的是
`self.link_names().iter().any(|_| true)`（等价于"模块里存在过任何一个 `#[link]`"，不是判据），
`codegen.rs:1226` 对 A 轨调用发指名 `CgError` = 本轮的发射落点。

**落地面**

- `src/atrack.rs`（新建，**27 项单测**，每项 doc comment 指名条款）：
  §7.6.1 的读取与配对（`signature_blocks` / `atrack_sources` / `pair`，两个方向都出 `E-0720`；
  `extern "C"` 之外的 ABI 与 `@cpp`/`@rust`/`@zig` 点名拒绝）；§7.6.4 的封闭表（`boundary_c`，
  把"规范禁止"与"规范允许但没发射形态"分成 `Forbidden`/`Unsupported` 两半，见偏差 D-46(e)）；
  UB-010 的移交四态（`ownership_of`）；§7.6.3 项 2 的符号（`normalized_block` + `sig_hash` +
  `wrapper_symbol`）；项 1 的 `binding_header` 与 `unit_c_source`。
- `src/cli.rs`：`analyse()` 里接 `atrack::pair` 的另一半方向（`emit_atrack_gaps`）；
  `a_track_units()` 把 `infy_binding.h` 与 `a_track/<name>.c` 写进**每个**构建目录（`--verify-reproducible`
  跑两个目录，所以写文件的动作必须在 `stage9` 里面而不是外面）；`stage9` 的 `cc` 命令加
  `-I <work>/a_track` 与逐单元的 `.c`，顺序按单元名字典序（§7.4 段顺序 + §7.3 可复现）。
- `src/sema.rs`：`FnInfo::link_unit`（§7.6.1 的 `#[link]` 挂在块上，块内声明继承它）；
  `register_fn` 改为返回索引，`extern` 块注册时对每条声明跑 `atrack::decl_gaps` ⇒ **`E-0723` 第一次有发点**；
  `E-0721` 的判据换成被调者自己的 `is_foreign`；删掉恒假的 `in_extern_block`（它此前只在"顶层 `;` 体"这条
  路径上被读，那条路径的答案本来就是 `false`，留着一个常量假判据是下一轮误判的来源）。
- `src/hir.rs`：`HirFn::link_unit` 透传；`signature_text()`（§11 / ABI §2 那一列）**此前零调用点**，
  现在是包装哈希的输入 ⇒ 由 `the_normalised_block_uses_the_hir_signature_column` 钉成活的。
- `src/codegen.rs`：`emit_foreign_decl` 从"指名拒绝"变成发射 `extern <原型> <wrapper>;`（键是**包装符号**，
  不是那个从没人定义的 INFY 符号）；`user_fn_call` 对 A 轨被调者改用 `wrapper.call(argv)`，
  `()` 返回走 `((void)w(a), (InfyUnit){0})`；`StmtKind::Expr` 里 A 轨 `@c` 语句**不发射任何东西**
  （它的 C 原文活在 `a_track/<name>.c` 里），B 轨半边与 §7.6.4 未发射的行继续以条款号拒绝。
- `src/parser.rs`：两处"块状表达式"判定补 `ExprKind::Embed`（偏差 D-46(f)，见下"本轮修掉的真实缺陷"）。

**本轮修掉的三条真实缺陷（都不是规范歧义，各有回归锁点）**

1. **`@c` 作语句时解析失败**（`parser.rs`）。§3.6 明写嵌入块"或作为 `expr_stmt` 出现"，§7.6.1（spec:1113）
   与 §14（spec:2143）的样例把它写成**不带 `;`** 的一条语句；而 `block()` 与 `stmt()` 的"块状表达式"清单
   （列 `If`/`Match`/`Block`/`Unsafe`/`StructLit`/`Closure`）漏了 `Embed` ⇒ 规范自己的样例报
   `E-0053 语句缺少终结符`，且 span 落在**下一条**语句的开头（本轮第一次写 A 轨端到端测试时被报在
   `unsafe { … }` 那一行，第一眼像是 `unsafe` 的问题）。修 = 两处补 `Embed`，`E-0722`/配对才轮到显形。
   锁点 = `parser::tests::an_embed_block_statement_needs_no_semicolon`（无 `;` 与有 `;` 两种都断言 + AST 形状）。
2. **块状尾表达式的结果临时量声明在内层作用域**（`codegen.rs`）。`{}`/`unsafe {}` 作函数尾值时，
   `bres_N` 被声明在 `self.b("{")` **之后**，于是 `return bres_N;;` 引用一个已经出视线的变量 ——
   gcc 报 `'bres_1' undeclared`，产物根本链不出来。旁边的 `ExprKind::If` 分支一直是对的（先声明再开分支），
   这一臂把顺序写反。这条缺陷存在了很久：`infy check`/`infy build` 到阶段 [9] 才需要真编译，而 tour 里
   没有"块作尾值"的写法，所以**从未被执行面暴露**。修 = 结果临时量移到开括号之前。
   锁点 = `codegen::tests::a_block_tail_expression_declares_its_result_in_the_function_scope`。
3. **`E-0721` 的判据是占位**（`sema.rs`）。原判据 `self.link_names().iter().any(|_| true)` 与"被调者是不是
   A 轨 extern"无关：模块里只要出现过任何一条 `#[link]`，所有 `is_foreign` 调用都被判要 `unsafe`；
   一条 `#[link]` 都没有时，真正的 A 轨调用反而**不报**。修 = 判被调者自己的 `is_foreign`，
   并把"是不是 A 轨单元成员"写进诊断的 note（`link_unit`）。
   锁点 = `cli::tests::an_unsafe_less_a_track_call_is_e0721`。

**端到端（D3 的验收命令本身）**：`target/scratch/cli-atrack` 那份最小 module（`#[link(name = "decoder",
static = true, toolchain = "cc")] extern "C" { fn dec_mark(v: u32) -> u32; }` + 一个 `@c(name = "decoder")`
的 C 定义 + 链节点里 `unsafe { dec_mark(41u32) }`）→ `infy run` **exit 0**，程序 stdout 逐字节 =
`dec_mark(41) -> 42`（观察通道是 C 侧的 `printf`：INFY 侧没有任何构造能写 stdout = F19，所以非空输出必须
由 A 轨的 C 代码提供）。`infy build` 同一 module 多打一行 `a_track ok — 阶段 [8] 绑定：1 个单元（decoder）、
1 个包装符号、475 字节写入 …\a_track（§7.6.3）`，目录里是 `infy_binding.h` 与 `decoder.c`。
**三处符号逐字符一致**被 `the_build_writes_the_binding_header_and_the_wrapper_unit` 钉住（头 / 包装本体 /
`ci.c` 的 `extern` 与调用点）——不一致只会以链接错误现形，两侧各自的单测都看不见，所以比较的是符号本身
而不是"两边都能编译"。`atrack::tests::the_generated_header_and_wrapper_compile` 另用
`gcc -std=c11 -Wall -Wextra -c` 直接编译生成的头 + 假用户实现，断言 **0 error / 0 warning**。

**规范未定 → 选定实现**：偏差 **D-46 (a)–(f)**（`<8hex>` 的规范化算法 / 哈希是块级而 `signature_text` 是函数级 /
UB-010 只判返回方向 / `usize` 写 `size_t` 而不是规范文中的 `uint64_t` / 禁止项与未发射项分道 /
`@c` 语句不需要 `;`）。

**回归（全部本轮实测，非引用）**：`cargo test --offline --lib` **505 passed / 0 failed**（基线 468，+37：
`atrack` 0 → 27、`cli` 28 → 33、`codegen` 22 → 26、`parser` 8 → 9）；`cargo check --offline --lib` 0 error /
**50 warning**（未新增 —— 本轮把改动前后的告警**集合**逐条 `diff` 过，同一棵树 fresh 检出也是 50，
`BACKLOG` 里那句"50"与本文件的"50"是同一口径）；`infy check examples/tour` exit **0**、
`infy build examples/tour --verify-reproducible` exit **0**（生成 C11 仍 **12513** 字节 / 产物仍 **199975** 字节 /
`blob 738` / `build-id 4d8250b5…55ba` 逐字节不变：tour 没有 A 轨源 ⇒ `a_track/` 不创建、链接输入一字不变，
这正是 BACKLOG §2 事先约定的判据）、`infy run`/`infy test examples/tour` exit **0**、
`infy artifact info … --nodes` exit **0**、生成 C 的 `gcc -std=c11 -Wall -Wextra -I runtime -c` exit **0**
（0 error / 0 warning）、`(cd examples/tour && infy fmt --check)` 仍 exit **2**（F6 未裁决，未变）；
**C 侧本轮零改动**（`.c`/`.h` 一行没动）仍复跑四条链：`-c infy_rt.c` 与 `-c infy_ifbc.c` 各 **0 告警** exit 0、
rt harness **32335 assertions / 0 failures** exit 0、替身链与生产链 IFBC 各 **180/180 pass / 0 FAIL**
exit 0 且两份 stdout `cmp` **无差异**。

**明确未做**（不用假成功掩盖，全部登记）：**F31** = §7.6.4 行 7 的 `#[repr(C)]` struct 与 ADT 的不透明指针
跨边界没有落点（绑定头只携带标量与标量指针），于是 §7.6.1 样例自己的 `-> *mut DecHandle` 过不了 ——
与 F30 同族：**规范的样例不能构建**；**F32** = §7.6.2 的工具链发现/注册（`TOOLCHAIN.infy.toml`、
`infy toolchain probe/add`）、`<build>/a_track/<name>/recipe.toml` 审计、版本核对 `E-0604` 与
`--allow-toolchain-drift`/`W-0604`/`unreproducible` 标记全未接，A 轨现在只驱动 `--cc`/`$CC` 那一个 `cc`；
**F33** = §7.6.3 项 3 的非标量边界 thunk（长度/对齐/null→`Option`、`R-0720` 借用有效性、`R-0722`/`R-0723`）
只有标量转发、项 5 的外部行号→INFY 诊断位置（`decoder.c:41`）未做、§7.6.1 的 `W-0720`
（源码导出了而签名未声明）与"签名声明了但源码未导出 → 链接期 `E-0604`"没有映射（今天由 `cc` 的
undefined reference 原文冒出来）、§7.6.5 反向导出与 `E-0724`、§7.6.6 的 `isolation = "process"` 全未实现。


## E1 轮（2026-10-16）：§12 的用例格式、分类表与运行器落成，首批 83 条外部用例 / 17 个分类

`src/conformance.rs` 从 1 行占位落成三件东西（此前 `conformance/cases/**` 也不存在）：

1. **§12 的三张分类表**（`CATEGORIES`，20 行：§12.1 的 9 行 1244 + §12.2 的 3 行 256 + §12.5 的 8 行 800），
   `title`/`check`/`required` 逐字抄规范。`the_category_table_adds_up_to_the_specs_own_totals` 直接把
   1244/256/800/2300 与 `CATEGORIES.len() == 20` 断言住 ⇒ 表里错一个数、多一行或少一行都会红。
   `conformance/cases/<分类>/CATEGORY.toml` 是这张表的**渲染产物**
   （`INFY_REGENERATE_CATEGORIES=1` 才重写，方向分工与 IFBC 夹具同一族），`verify_category_file` 锁另一方向。
2. **用例格式**（规范从未定义，裁定记偏差 **D-47** (a)–(h)）：一个用例 = 一个目录 =
   `case.toml`（`id`/`category`/`clause`/`title`/`kind`/`expect`/`exit` + 可选 `file`/`image`/`requires`/`defect`）
   加逐字节的源文件。不采用"期望写进被测源码的注释里"（§12.1 有"注释"分类、§12.5 的 §9.5 分类检查
   "注释保留"，两者会自指）。`kind` 三档：`check` / `fmt-idempotent` / `ifbc-verify`。
3. **运行器**：`run_case` 走 `cli::check_verdict`（新落成的公开入口 = `load_scoped` + `analyse` +
   干净时的 `ingest_b_track`），也就是 `infy check` 自己的那条链，**不是第二套前端**。
   比较面 = 诊断码的排序多重集 + §13.4 退出码（§9.2 的位置与文本由 `diag`/`fmt` 单测钉，不在这里）。
   `case.toml` 里出现码表之外的期望码 = **加载错误**（`codes::lookup`），不是"永不匹配"。
   缺构建期工具 = **Blocked**（§7.7.3 把工具在场列为前提；记通过是谎报、记失败是把环境说成实现）。
   `defect = "Fxx"` 的用例挂的仍是**规范**期望：不达 → Blocked，一旦通过 → 测试直接红并要求删标记。

### 用例集实测（`cargo test --offline --lib` 里跑，P28）

**83 条 / 17 个分类**：77 Pass、6 Blocked、**0 Fail**。逐分类：`2-lexical` 13 · `2.8-literals` 12 ·
`4.7-ownership` 7 · `4-types-ops` 6 · `7.7-b-track` 6 · `8-chains` 6 · `3-grammar` 5 · `5-modules` 4 ·
`7.6-a-track` 4 · `4.9-ub` 3 · `4.10-eval-order` 3 · `6.7-derive` 3 · `3.11-expr-pat-stmt` 3 ·
`6-macros` 2 · `7.8-ifbc` 2 · `8.4-node-focus` 2 · `9.5-fmt` 2。空分类 3 个（`1.3-domain-neutral` = F38、
`10.9-std` = 断言在运行期而程序没有 stdout 通道 = F19、`11.1-host-protocol` = `src/host.rs` 占位），
目录与 `CATEGORY.toml` 都留着，缺口打印出来而不是删掉。
B 轨用例声明 `requires = ["python"]`，清单里写 `@PY_MINOR@` 由运行器换成**本机探针报的 minor**
（否则同一份提交的用例在 3.13 机器上会因 `E-0713` 红 ⇒ 用例集就在测机器而不是测实现）。

### 搬运当场揪出的四条缺口（各挂着 Blocked 用例，不假装通过）

- **F34** = §10 的 std 签名表里没有 `Vec::clone`：`take(a.clone())` 报 `E-0110 · 类型 Vec<u8> 没有方法 clone`
  ×3 ⇒ §4.7 的"clone 之后再各自使用"这一类**正例**在本实现不可表达。`borrow.rs` 的原测试只断言
  "不含 E-1001"（`contains` 式断言），所以这三条 E-0110 一直在，只是从没被看见。
- **F35** = **任何 `#[derive(...)]` 都过不了 `infy check`**：derive 生成的体调
  `std::fmt::format_struct` / `format_unit` / `format_tuple`（`derive.rs:1165/1176`），而 §10 的 std 表里
  没有这些符号 ⇒ 阶段 [4] 报 `E-0053 缺少返回值` + `E-0101 调用未声明的函数` + `E-0110 无法解析字段`；
  加 `import "std/fmt"`、改 package 目录、补 `[[bin]]`/`[[lib]]` 都不变。`derive.rs` 的 57 项单测断言的
  全是生成物的**文本**，生成体从未被阶段 [2]-[4] 回读过；而 `examples/tour` 里根本没有 `derive` ——
  所以"derive 已通"这一族声称从来没有端到端证据（同族认法 = 经验库 `infy-unreachable-implementation-branch`）。
- **F36** = 同一根因被两个阶段各报一次：`let union = 1u32;` 出两条 `E-0020`（标题还不同：
  "保留字 `union` 不可作标识符" + "保留字作标识符"），`#[derive(Nope)]` 出两条 `E-0410`。
  §9.2 没有"同一违规报两次"的规定，而用例比较的是全集 ⇒ 重复报告会直接改变通过率。
- **F37** = 未知 `@` 前缀（`E-0702`）之后级联 8 条 `E-0053`：一条词法错误把一个语句炸成整文件的
  顶层声明错误，违反 §9.6 的诊断可读性方向（`lexer.rs` 自己有 `MAX_PER_CODE`，但没有"相邻去重"）。
- **F38**（新登记的分类阻塞）= §1.3 领域中立的扫描函数 `pkg::domain_neutrality_violations` 存在且有单测，
  但**全仓库没有 CLI 调用点**，而规范点名的黑名单文件 `std/domain_terms.txt` 也不在仓库里
  ⇒ `1.3-domain-neutral` 分类结构性地无法有用例。

### 顺手挡掉的一处假红（`.gitattributes`）

这台机 `core.autocrlf=true`，检出会把 `conformance/cases/**/*.infy` 改写成 CRLF；而 §2.1 的 `W-0001`
**就把行尾本身当被测面** ⇒ 每条用例都会多出一条诊断，故意写 CRLF 的那两条正反例整个失效。
新增 `conformance/cases/** -text`（与 D4(3) 轮给 `runtime/fixtures` 那条同族），别删。

### 回归（全部本轮实测，非引用）

`cargo test --offline --lib` **516 passed / 0 failed**（基线 505，+11 全在 `conformance`）、
`cargo check --offline --lib` 0 error / **50 warning**（未新增）、`infy check|build|run|test examples/tour`
全 exit 0 且 `--verify-reproducible` 0、生成 C11 仍 **12513** 字节 / 产物仍 **199975** 字节 /
`blob 738` / `build-id 4d8250b5…55ba` **逐字节不变**（本轮没碰 [7]-[9]）、`infy artifact info … --nodes` 0、
`(cd examples/tour && infy fmt --check)` 仍 exit **2**（F6 未裁决）；**C 侧零改动**仍复跑四条链：
`-c infy_rt.c` 与 `-c infy_ifbc.c` 各 **0 告警**、rt harness **32335/0**、替身链与生产链 IFBC 各 **180/180**
且两份 stdout `cmp` **无差异**。

**明确未做**（不用假成功掩盖）：`infy conformance --report` 与逐分类通过率的**命令行** = **E2**
（E1 轮时仍走"未实现"臂 exit 1，`tally`/`run_corpus` 已把数据面备好；**E2 轮已落地**，见下文 E2 一节）；通过率落进 `docs/coverage.md`
与 README 的"距 100% 还差哪些类" = **E3**；§12.3 的黄金文件（codegen golden / `infy ifbc dump` 逐指令）
与 §12.4 的等价性矩阵不进 `cases/**`（D-47(g)），它们的现成锁点是 `--verify-reproducible`（C3）与
`infy ifbc dump --canonical` 的双侧 `cmp`（D2）。

## E2 轮（2026-10-16）：`infy conformance --report` 接进命令行，§12.5 的逐分类通过率第一次由命令输出

数据面一行没动（E1 轮的 `conformance::{CATEGORIES, load_cases, run_case, tally}` 就是全部输入），
本轮只落 `src/cli.rs` 的**命令层**：`conformance_cmd`（§13.4 的 `[cases/] [--category X] [--update-golden]`
+ §12.5 的 `--report`）/ `conformance_report_text` / `conformance_rate` / `conformance_exit` /
`conformance_update_golden_refused`，并把 `conformance` 从"未实现"臂摘出来。顺带把那张臂的名单抽成
`cli::NOT_IMPLEMENTED` 常量 —— 拒绝臂和已接线的命令**可以共享同一个退出码 1**，只有名单本身能让
"命令又躺回拒绝臂"变红（D2/D1(2) 那两条"上游全绿对 CLI 能不能走到它没有预测力"的第三次应用）。

**验收（逐分类通过率，实测输出见上文"已跑通的端到端验收"的 `infy conformance --report` 段）**：
20 行逐分类（§12.1 的 9 + §12.2 的 3 + §12.5 的 8）+ 三档小计 + 合计，
**83 条用例 = Pass 77 · Fail 0 · Blocked 6 · 通过率 92.8% · 规范目标 2300（覆盖 83，缺口 2217）**。
命令层的这三档数字与 E1 轮 `conformance::tests` 在测试里跑出的**完全相同** —— 这不是巧合而是要求：
运行器只有 `cli::check_verdict` 一条路径（D-47(h)），所以"命令打的分"与"测试算的分"必须同源。

三条防谎报的形状（都是 D-48 的内容，不是实现细节）：
1. **空分类打 `n/a`**：`1.3-domain-neutral`（缺 52）/ `10.9-std`（84）/ `11.1-host-protocol`（60）三行
   一条用例都没有，0/0 若渲染成 100%（或 0%）就把"这一行 §12 完全没被检查"读成"这一行有结论"。
   同一族的 `infy ifbc` 空区措辞（D2 轮把 `ifbc_empty_region_text` 做成函数因为措辞本身就是断言对象），
   这里 `conformance_report_text` 同样做成函数并被测试逐子串断言。
2. **Blocked 进分母、不进分子**：通过率 = `Pass / 用例数`。若分母只算"判完的"（`Pass + Fail`），
   1 条通过 + 5 条阻塞 = 100% —— 正是 §12.5「只报总数视为不合规」要防的形状的另一半。6 条 Blocked 逐条打
   `defect=F34 / F35 / F36 / F37` 与「期望 … / 实得 …」首行，所以外部读者不需要读 `case.toml` 就知道缺口挂在谁名下。
3. **`0` 不发给不完整的套件**：§13.4 从未给 `conformance` 分配退出码，本实现裁 `0` 要求
   **20 个分类都达到 §12 的 `required`** 且 `Fail=0`、`Blocked=0`；有 `Fail` → `2`；其余 → `1`。
   今天实得 `1`。这一条单独钉（`conformance_exit_refuses_zero_for_anything_short_of_the_whole_suite`
   用**合成数字**而不是用例集），否则将来用例涨到 2300 时没人记得"0 曾经是有条件的"。

规范未定 → 选定实现 = **偏差 D-48 (a)–(g)**：(a) 退出码归属、(b) `Fail` 优先于 `Blocked`、
(c) 通过率的分母与空分类的 `n/a`、(d) `--category` 过滤运行不构成结论（未运行的分类打 `-`/「未运行」
而不是 0，指向 0 条用例的分类直接报错而不是打空表）、(e) `--update-golden` **两个分支都拒绝**
（无授权 = §12.4 的授权错误；有授权 = 本实现的 `cases/**` 里没有黄金文件可更新，§12 又明令已有用例的期望
不得改变，§12.3 的黄金文本由 `runtime/fixtures` 的两侧写入器生成 = D-45(b)，真接还需要 F29 的执行通道）、
(f) §13.4 未列的开关拒绝而不静默忽略（同 D-35(c)/F19(b) 那一族）、(g) 默认用例根按 `CARGO_MANIFEST_DIR`
解析而 `[tests/]` 位置参数可覆盖它，`--report` 接受但不改变行为（这条命令只有"跑套件 + 打逐分类表"一种动作）。

**本轮没有修掉任何实现缺陷**（`conformance::*` 与前端行为一字未动，只加命令层）。新增 5 项 `cli` 测试
（33 → **38**），其中两条钉的是"措辞本身"而不是数字：空分类的 `n/a` 与 `defect=` 必须出现在输出里。

回归全部复跑：`cargo test --offline --lib` **521 passed / 0 failed**（基线 516，+5 全在 `cli`）、
`cargo check --offline --lib` 0 error / **50 warning**（未新增：改动前后的告警**集合**逐条 `diff` 相同，
HEAD 用 `git archive` 解到 `target/scratch/basecheck` 全量重检同为 50）、`infy check|run|test examples/tour`
全 exit 0、`infy build examples/tour` exit 0 且生成 C11 仍 **12513** 字节 / 产物仍 **199975** 字节 /
`blob 738` / `build-id 4d8250b58122ca2b76132029955e2d2005156dc0a6b021bf5f4ae761e5fd55ba` **逐字节不变**
（本轮没碰 [7]-[9]）、`--verify-reproducible` exit 0（两个独立构建目录 199975 == 199975）、
`infy artifact info … --nodes` exit 0、生成 C 的 `gcc -std=c11 -Wall -Wextra -I runtime -c` exit 0
（**0 error / 0 warning**）、`(cd examples/tour && infy fmt --check)` 仍 exit **2**（F6 未裁决，未变）；
**C 侧零改动**（`.c`/`.h` 一行没动）仍复跑四条链：`gcc -c runtime/infy_rt.c` 与 `-c runtime/infy_ifbc.c`
各 **0 告警** exit 0、rt harness **32335 assertions / 0 failures** rc **0**（在 `target/scratch` 里跑）、
替身链与生产链 IFBC 各 **180 asserts / 180 pass / 0 FAIL** rc **0** 且两份 stdout `cmp` **无差异**。

**明确未做**（不用假成功掩盖）：§12 的**对外通过率表**（落进 `docs/coverage.md` 与 README + 显式列
「距 100% 还差哪些类」）= **E3**；§12.3 的黄金文件更新（`--update-golden` 现两分支都拒绝 = D-48(e)）；
§12.4 的等价性矩阵（wheel ≡ 源码、`-O0` ≡ `-O3`、`lto=false` ≡ `fat`、A 轨同进程 ≡ `isolation="process"`）
没有任何一档能真跑，因为 F12/F19/F29/F31/F32 都还开着；三个空分类的成因仍是 **F38**（§1.3 扫描零调用点 +
黑名单文件不在仓库）、**F19**（程序没有 stdout 通道 ⇒ §10.9 的运行期断言不可表达）、`src/host.rs` 占位；
6 条 Blocked 用例的期望挂的是**规范**期望而不达 = **F34/F35/F36/F37**（一条都没为了让报告变绿而改动期望，
§12 明令编号与期望不得改变）。**措辞红线**：E3 之前任何文档不得出现「INFY v2.0 兼容」，
命令报告里那句"不得声称兼容"是它自己的限定语，不是声称。

## E3 轮（2026-10-16）：§12 逐分类通过率落成对外表格，并把「不得声称兼容」从约定变成机检

**这一轮做的事**：E3 = §1 的第一个 `[ ]`。通过率数字 E2 轮已经在命令里了，缺的是"读者看得到的那张表"
与"那条红线不只是写着"。落了四处：

1. **`docs/coverage.md` 新增 §6**（本文档之外的第二个视角：§1-§5 是"条款 × 实现 × 证据"，§6 是"分类 × 数字 × 缺口性质"）：
   20 行 × 11 列（slug / §12 表 / 分类名原文 / 要求 / 已有 / Pass / Fail / Blocked / 通过率 / 缺口 / 非满分的原因）
   + 三档小计 + 合计 + **缺口分三类**那一行 + §6.1 逐类点名的读法 + §6.2 "用例格式表达不了的 §12 面"
   （§12.3 黄金、§12.4 等价性矩阵四档、比较面只有【诊断码 + 退出码】）+ §6.3 红线与"通过率 ≠ 合规率"。
2. **根 README 新增「距 §12 的 100% 还差哪些类」**：三类缺口逐条点名（覆盖缺口 2217 条 / 结构性空分类 3 行 196 条 /
   Blocked 6 条）+ 第四类（§12.3/§12.4 不进 `cases/**`），并先划"通过率 92.8% ≠ 合规率"这条读法线 ——
   这正是 D-48(a) 为什么把 `0` 裁成"20 个分类都达到 `required` 且 Fail=0、Blocked=0"而不是"没有失败"。
   §12 行与测试计数行同步改成 524 / E3。
3. **`conformance/README.md`**：指出通过率表在 `docs/coverage.md` §6，并说明本目录那张"已有 vs §12 要求"
   **刻意不复制通过率**（同一份数字在两处各算一遍，就会在同一个缺口上给出两个答案 = D-47(h) 的同一族）。
4. **表是副本，所以给它配了回核**（`conformance` 11 → **14** 项）：
   - `the_pass_rate_table_in_coverage_md_matches_the_corpus` —— 行的**存在、数量、顺序**必须等于 `CATEGORIES`；
     每行前 10 列逐格对回 `conformance::tally`（通过率用 `cli::conformance_rate` 那**一个**格式化函数，
     为此把它改成 `pub(crate)`；两处各写一份四舍五入 = 报告与文档能在同一个缺口上给出两个数）；
     "非满分的原因"列里点名的 `Fxx` 必须与该分类实际挂着的 `defect` 集合**相等**（少写、多写、写错都红），
     0 用例的行必须以"结构性空分类"开头、有 Blocked 的行必须写 Blocked、其余以"覆盖缺口"开头；
     三档小计、合计、缺口三分类那五行按整行字符串比对。
   - `no_markdown_doc_claims_the_compatibility_name` —— §12.4 的红线机检：扫全仓 `.md`（排除 `spec/` 那份规范原文），
     任何一行出现那句声称而没有否定标记 ⇒ 红。**允许出现在禁令的引用里**（BACKLOG 的任务行、RUNLOG 的历史行、
     各文档的红线句），因为那正是它能被搜索的原因；要被挡的是肯定句。
   - `the_front_facing_docs_restate_the_refusal` —— 反空转：上面那条把四份文档的红线句全删掉也能"通过"，
     所以要求 README / `docs/coverage.md` / `conformance/README.md` / `docs/status.md` **各自**都还写着这句禁令。
   - 三条都做过**能变红**验证（临时把 `9.5-fmt` 行改成 3/3/117、往 status.md 加一句肯定式声称、
     删掉 conformance/README 的红线句 ⇒ 三条各自 FAILED，随后逐字恢复）。

**顺手修掉一处文档谎报（挂了三轮）**：`docs/status.md` 的"仍未实现"一节此前写着"目前外部用例为 **0**，
逐分类通过率不可测" —— 那是 A3 轮写下的、E1（83 条）与 E2（命令行）之后从未对回证据的旧话。本轮改成
83 条 / 3.6% 并点名它属于经验库 `infy-doc-claims-vs-evidence` 那一族。**BACKLOG §2 的 E2 指针里那句
"覆盖缺口 = 20 行里 17 行 > 0"同样过期**：17 是"有用例的分类数"，实测 20/20 行 `short` 全都 > 0
（最小的一行是 `7.8-ifbc` 的 62 条），`conformance_exit` 拿到的 `categories_short` 就是 20。

**本轮自己制造并修掉的一个测试隔离缺陷（不是实现缺陷）**：新表测试要跑全量用例集，而 `run_case` 把用例落成
`target/scratch/conformance/<用例 id>` —— **目录名只有用例 id**。同进程里两条测试跑同一套用例时，后一条的
`remove_dir_all` 会把前一条正在读的 module 删在半路上，现场症状是 `2-lexical-0001` 假报
`E-0600 产物无入口`（一个**语义**错误码，完全不像文件被抢），单独跑两条各自全绿、一起跑就红。
修 = `CORPUS_SERIAL` 静态 Mutex，三条会跑用例集的测试（含 `INFY_CONFORMANCE_PROBE=1` 那条作者工具）都先取锁。
记法与教训写进经验库 `infy-parallel-tests-share-scratch`。

**另一条按既有教训躲过的假失败**：rt harness 第一次在 `target/scratch/e3/` 里跑出
`FAIL test_rt.c:1520 fm.n_resource_keys` + `:1521 target_triple.len` + **exit 139（段错误）** ——
夹具按 `../../runtime/fixtures/…` 相对 CWD 取，换目录就是这种红（C4 / D4(1) 两轮现场记录过）。
**二进制可以留在子目录，但必须 `cd target/scratch` 再跑**；换过去之后 32335 / 0、exit 0。

**回归（全部本轮实测，非引用）**：`cargo test --offline --lib` **524 passed / 0 failed**（基线 521，+3 全在
`conformance`：**11 → 14**；分组计数 `lexer` 79 · `derive` 57 · `fmt` 39 · `cli` 38 · `macro_expand` 35 ·
`btrack` 30 · `atrack` 27 · `codegen` 26 · `pkg` 24 · `sema` 22 · `ifbc` 22 · `hir` 20 · `artifact` 18 ·
`toml` 15 · `borrow` 15 · `sha256` 12 · `conformance` **14** · `parser` 9 · `codes` 9 · `base64` 7 ·
`stdlib` 4 · `diag` 2 = 524）、`cargo check --offline --lib` 0 error / **50 warning**（未新增：HEAD 用
`git archive` 解到 `target/scratch/basecheck` 全量重检，与工作树的告警**集合**逐条 `diff` 相同，各 50 条）、
`infy check|build|run|test examples/tour` 全 exit **0**、生成 C11 仍 **12513** 字节 / 产物仍 **199975** 字节 /
`blob 738` / `build-id 4d8250b58122ca2b76132029955e2d2005156dc0a6b021bf5f4ae761e5fd55ba` **逐字节不变**
（E3 没碰 [7]-[9]）、`--verify-reproducible` exit 0（两目录 199975 == 199975）、
`infy artifact info … --nodes` exit 0 且节点表 6 行 = 行 8/9/10/11/16/19（`gate`/`retry_once` 各 1 连携块）、
生成 C 的 `gcc -std=c11 -Wall -Wextra -I runtime -c` exit 0（**0 error / 0 warning**，5723 字节 .o）、
`infy conformance --report` exit **1** 且合计行仍是 `83 · Pass 77 · Fail 0 · Blocked 6 · 92.8% · 2300（覆盖 83，缺口 2217）`
（本轮**没动过用例集**：83 条一个没加、一个期望没改）、`(cd examples/tour && infy fmt --check)` 仍 exit **2**（F6 未裁决，未变）；
**C 侧零改动**（`.c`/`.h` 一行没动）仍复跑四条链：`gcc -c runtime/infy_rt.c` 与 `-c runtime/infy_ifbc.c` 各 **0 告警**
exit 0、rt harness **32335 assertions / 0 failures** rc **0**、替身链与生产链 IFBC 各 **180/180 pass / 0 FAIL**
rc **0** 且两份 stdout `cmp` **无差异**。

**明确未做**（不用假成功掩盖）：§12 的**数量**本身（还差 2217 条，E3 只把缺口逐行点名，没有填一条）；
§12.3 黄金文件更新（`--update-golden` 两分支仍拒 = D-48(e)）；§12.4 等价性矩阵四档（F12/F19/F29/F31/F32 挡着）；
三个空分类要的是实现（**F38** 黑名单文件与调用点 / **F19** stdout 通道 / `src/host.rs` 占位）；
6 条 Blocked 要的是修 **F34/F35/F36/F37**；"两侧装载期校验等价"仍不许讲（**F26** 前半，清单 = **D-42** (a)–(e)）。
**红线本身不变**：§12.4 的 100% 之前任何文档不得那样声称，E3 落的是缺口表而**不是**达标声明；这条从今天起由测试守。


## F1 轮（2026-10-16）：`.github/workflows/ci.yml` 落成，CI 的每个 run 段先在本地按 GitHub 的执行方式重放

**做了什么**：新建 `.github/workflows/ci.yml`（11 步 = 2 个 `uses` + 9 个 `run`，单 job、单 runner），
`.gitattributes` 加一条把 workflow 自己钉成 `eol=lf`，`.github/` 落进 README 的目录表；
`src/conformance.rs` 加 **3 项**回核（14 → **17**），并修掉一处 E3 遗留的测试隔离缺陷（见本节末）。
`cargo fmt --check` 按 BACKLOG F1 的原话**不适用**（本仓库没有 rustfmt 约定，`src/` 全文不是 rustfmt 形状）。

**命令面 = §1 那四条链 + tour**，逐条与文档里的证据命令同源：

| CI 步骤 | 对应证据行 | 本地重放结果 |
| --- | --- | --- |
| `cargo build --offline --release` | 新（F1 第一条） | exit **0**，23.8s，`target/release/infy.exe` 3060224 字节 |
| `cargo test --offline` | P1（`--lib` 的那 527 项 + bin 的 0 项 + doc-test 的 0 项） | exit **0**，`527 passed; 0 failed` |
| `cargo check --offline --lib` 告警数 | P2 | **50**（只报不门禁：F2 未清，门禁 = 假装 F2 已清） |
| `gcc -std=c11 -Wall -Wextra -c` 两份 `.c` | P10 + `-c infy_ifbc.c` | 各 **0 告警**（stderr 0 字节，非空即红） |
| rt harness | P9 | **32335 assertions / 0 failures** exit 0 |
| IFBC 替身链 + 生产链 + `cmp` | P6 + P11 | 各 **180/180** exit 0，两份 stdout 逐字节相同 |
| tour 的 `check\|build\|run\|test` + `--verify-reproducible` + `artifact info --nodes` | P3/P12/P13/P14/P16/P17 | 全 exit **0**；12513 字节 C11 / 199975 字节产物 / 节点表 6 行 = 8/9/10/11/16/19 |

**为什么只有 Windows**（`runs-on: windows-latest`，没有 matrix）：阶段 [9] 的产物是 PE + 追加的
`INFYART2` 段，`infy_rt.c` 的 `std.fs` 走 `CreateFileW` + UTF-8→宽字符 ⇒ Linux/macOS 上跑的**不是**
这 527 项测过的形状，加一个 `ubuntu-latest` 就是假装双平台。

**三条防假红写进 yaml，并由测试钉住**（不是注释里的约定）：
1. **检出改写工作树 ⇒ 红**。`.gitattributes` 的 `-text`/`binary` 是双侧逐字节比较与 §2.1 `W-0001`
   （行尾本身就是被测面）的前提；守卫做成 `git status --porcelain` 必须为空 —— 它同时抓行尾转换和
   任何 smudge 过滤，比"打印一下 autocrlf 的值"强。
2. **四个重写夹具的开关绝不设**（`INFY_REGENERATE_FIXTURE`/`_CANONICAL`/`_CATEGORIES`/
   `INFY_CONFORMANCE_PROBE`）。CI 一旦设了它们，"双侧逐字节比较"就变成"两侧各写一遍再比一遍自己"。
   注释里点名允许（那正是"为什么不设"），赋值不允许 ⇒ 测试按 `NAME=`/`NAME:`/`export NAME` 三种形态
   扫非注释行。
3. **rt harness 的 CWD 锁在 `target/scratch`**：它按 `../../runtime/fixtures/…` 取夹具，换目录 =
   两条假失败 + **exit 139**（这条坑已在 D4(1)、C4、E3 三次现场复现，本轮第四次：先在 `target/scratch`
   之外跑，拿到 `2 failures` 才换回去）。
   另有一条本轮新坑：**workflow 文件自己的行尾**。这台机 `core.autocrlf=true`，未标属性的 `.yml` 检出成
   CRLF 时，多行 `run` 段里的 `do`/`fi` 是 bash 语法错误（用 CRLF 副本验过 PyYAML 会把行尾归一化、
   run 段里 `` 数为 0，但 GitHub 用的是另一个解析器 ⇒ 不赌，`.gitattributes` 显式钉 `eol=lf`）。

**两条刻意不进 CI**（写进 yaml 注释，防止下一轮"顺手补上"）：`infy conformance --report` 今天 exit **1**
（D-48(a)：`0` 要求 20 个分类都达到 §12 的 `required` 且 Fail=0、Blocked=0，而覆盖只有 83/2300）；
`(cd examples/tour && infy fmt --check)` exit **2**（F6 未裁决）。把它们当门禁 = 把"缺口/未裁决"读成"回归"。

**CPython 与 `cc` 都是前提，缺了就红**：`btrack::tests::py_unit_fixture_bytes()` 重新摄取 §7.7.1 的
`normalize` 单元时声明 `py = "3.14"`，别的 minor ⇒ `E-0713`、别的字节码 ⇒ 夹具回核不等 ⇒
`the_pass_rate_table_in_coverage_md_matches_the_corpus` 会红。这是设计如此（BACKLOG §2 的 F1(2)：
remedy 是备解释器，不是放宽断言）⇒ yaml 用官方 `actions/setup-python@v7` 装 **3.14**，
`cc` 则先查 PATH、再查四个常见目录（`/c/msys64/ucrt64/bin`、`/c/msys64/mingw64/bin`、`/c/mingw64/bin`、
`/c/Strawberry/c/bin`）、都没有就带 `PATH=` 显式失败。**没有**"缺工具就 skip"的分支。

**顺带修掉的测试隔离缺陷（E3 那把锁的跨模块形态）**：E3 轮加的 `CORPUS_SERIAL` 定义在
`conformance::tests` 里，而 `cli::tests` 的 `the_conformance_report_…` 与 `a_filtered_conformance_run_…`
直接调**公开的** `conformance::run_case` 跑同一套用例 ⇒ 同进程两个用例集运行器照旧互删 module。
本轮加 3 项只读测试改变了线程调度，把它现形了：`7.7-b-track-0003` 假报「系统找不到指定的路径
(os error 3)」、`cli` 那条读到 `Fail = 2`，而**单跑各自全绿**。修 = 把锁挪到共享路径的被碰点
（`run_case` 自己持模块级 `SCRATCH_SERIAL`），调用方再也忘不掉；`mod tests` 里那把锁与三处取锁行删除。
**认法**：一把放在测试模块里的锁，只保护同一个模块里的邻居（经验库 `infy-parallel-tests-share-scratch`）。

**回归（全部本轮实测，非引用）**：`cargo test --offline --lib` **527 passed / 0 failed**（基线 524，
+3 全在 `conformance`）、`cargo check --offline --lib` 0 error / **50 warning**（未新增）、
`cargo test --offline` 全目标 exit 0（lib 527 / bin 0 / doc 0）、`cargo build --offline --release` exit 0、
`infy check|build|run|test examples/tour` 全 exit 0、`--verify-reproducible` exit 0（199975 == 199975）、
`infy artifact info … --nodes` exit 0、生成 C11 仍 **12513** 字节、生成 C 的 `gcc -c` **0 error / 0 warning**
（5723 字节 .o）、`infy conformance --report` 仍 **exit 1** 且数字一字未变（83 = Pass 77 / Fail 0 / Blocked 6 ·
92.8% · 覆盖 83/2300）、`(cd examples/tour && infy fmt --check)` 仍 **exit 2**（F6 未裁决）；
**C 侧零改动**仍复跑四条链：`-c infy_rt.c` 与 `-c infy_ifbc.c` 各 **0 告警**、rt **32335/0**、
替身链与生产链 IFBC 各 **180/180** 且 stdout `cmp` **无差异**。

**明确未做 / 不能声称**：F1 的验收只能验到"命令 + 文件 + 远端是否登记"，**验不到"CI 绿"** ——
远端那一次 run 的实际结论只认 `gh api …/actions/runs` 的回查，逐条写在 `docs/RUNLOG.md` 的 F1 push 行里；
runner 镜像里到底有没有 `gcc`、`setup-python` 的缓存里有没有 3.14，都是**本轮无法在本机测量的事实**
（`windows-latest` 映射到哪张镜像、镜像装了什么的仓库 `github/runner-images` 在本机 API 返回 404），
yaml 因此把两者都做成"缺了就红并打 PATH"而不是"缺了就跳"。

## F2 轮（2026-10-16）：`cargo check --offline --lib` 的 50 条告警一次清零，其中两条是「没接上」

**口径**：`touch src/lib.rs` 后 `cargo check --offline --lib --message-format=short`，计数 = 冒号 warning 的行数
（`grep -c warning` 会把"generated 50 warnings"那句总结也数进去）。开工 50 → 收尾 **0**，0 error。
**没有用 `#[allow(...)]` 消数**（仓库里既有的两处 `#[allow(dead_code)]` 未动，也不新增）。

**按形状分掉的 50 条**：20 条 `unreachable pattern`（`borrow.rs:436` 一行里 11 条重复臂，437 又重复
`NestedItem`；`parser.rs` 4 条 = 1675/2179/2256 的 `Kind::Lit(Lit::Int(_))` 重复 + **2540 就是 F9 那条死臂**；
`ifbc.rs:1166`×2、`btrack.rs:1406`、`fmt.rs:528`、`pkg.rs:837`）· 6 条 unused import · 6 条 unused variable ·
3 条 `depth` 赋值不读（`parser.rs:1219/1487/1493`，三处都是 `depth -= 1; bump(); break;`， break 之后不再读）·
2 条多余 `mut` · 2 条 never-constructed 变体（`btrack::Selected::None`、`fmt::Seg::Try`）·
11 条 never-used fn/method/field（`intern_list`、`expand_tabs_for_width`、`call_doc`、`take_rest`/`next_start`、
`cli::lint`、字段 `saved`/`is_enum`/`unsafe_blocks`/`expect`/`span`+`file`/`negated_child`）。

**两条判成「没接上」而不是「没用」的，本轮接上了**：

1. **§9.6 的 `[lints]` 此前被静默忽略**。`pkg.rs` 把 `[lints]` 解析进 `Manifest::lints`，`Loaded::lint()`
   会把它翻成 `LintConfig`，但**全仓零调用点**，而 `load_scoped()` 里那段"活的"代码只赋了 `strict` 一个字段。
   ⇒ 清单里写 `W-0002 = "allow"` 完全不生效，写 `E-0101 = "allow"`（§9.6 明令拒绝的降级）也不报错 = 假成功。
   **改法**：`Loaded::lint()` → `cli::lint_config(base, &manifest)`，在 `load_scoped()` 里真调用；
   `LintConfig::set` 退回的理由进 `rejected`，每条发一条 `E-0093`（`Phase::Lex`、file 0、1:1 —— §9.6 没说这条
   诊断该指哪，裁定记偏差 **D-49**）。测试 `cli::tests::manifest_lints_reach_check_and_a_downgrade_is_refused`
   是**同一份源码三份清单的成对断言**：无 `[lints]` ⇒ 诊断集恰为 `["W-0002"]` 且 exit 2；
   `W-0002 = "allow"` ⇒ 诊断集为空且 **exit 0**；`E-0101 = "allow"` ⇒ 含 `E-0093` 且 exit 2。
   实测语料三份目录（`examples/`、`conformance/cases/`、`runtime/`）**没有任何一份清单写过 `[lints]`** ⇒
   接线不会改动既有用例，527 项旧测试一条没转红。
2. **wheel 归档的 ZIP 成员偏移**。`btrack.rs` 解析中央目录时算出了 `lho`（APPNOTE 8.6.16 的
   "relative offset of local header"，偏移 42）**却丢掉**，`ZipEntry.data_off` 一律先写 `0`，随后的
   "Resolve each member's data start from its local header" 循环就从 `data_off`（= 恒 0）取本地头 ——
   ⇒ **每个成员都按第 0 个成员的本地头算数据起点**。两成员归档里第二条要么 CRC/长度不符显式报错，
   要么静默读到别人的字节。`loop` 头里那三行 `let o` / `(e.data_off as u64) as usize` / `let _ = o` 是同一次
   改动的残留。**改法**：`data_off: lho`，循环头收成 `let off = e.data_off as usize;`。
   回归测试 `btrack::tests::each_wheel_member_reads_through_its_own_local_header` 现造一份两成员 stored 归档
   （stored、CRC 由 `btrack::crc32` 现场算，不用编造常数），断言 `read(0)` 得 `AAA`、`read(1)` 得 `BBBBBB`；
   **负对照实测**：把 `data_off` 改回写死 `0` ⇒ 该测试在 `zip.read(&bytes, 1)` 处 panic（`解压 3 字节，但要 6 字节`），
   改回后 `data_off: lho` 在场、测试绿。这条就是 F2 指针里那句「删之前先分清『没用』与『没接上』」的兑现。

**判成「没用」而删掉的，都留了去处或替代锁点**：`parser.rs:2540` 的死臂删掉后，`ExprKind::Int` 的唯一产地是
`derive`（`primary` 的 `Kind::Lit(l)` 臂匹配所有字面量，F9 的剩余待审计面因此缩小到"只匹配 `ExprKind::Int`
的判据"，`primary` 那条已不可能再被写回来）；`Seg::Try` 删掉是因为 `flatten_chain` 早就用 `Seg::Glue("?")`
发 `?`，输出不变；`call_doc` 与 `ExprKind::Call` 那条活臂逐行同形（`turbofish_between` + `args_doc`），
`expand_tabs_for_width` 没有替代实现 ⇒ 登记 **F39**（§9.5 第 2 条"输入 tab 参与宽度测量"这一半未接）而不是假装做过。

**回归面**：`cargo test --offline --lib` **529 passed / 0 failed**（基线 527，+2 = `cli` 38→**39**、
`btrack` 30→**31**）、`cargo test --offline` 全目标 exit 0（lib 529 / bin 0 / doc 0）、
`cargo build --offline --release` exit 0（22.5s，`infy.exe` 3061248 字节）、
`infy check|build|run|test examples/tour` 全 exit 0、`--verify-reproducible` exit 0（199975 == 199975）、
`infy artifact info … --nodes` exit 0（6 行 = 8/9/10/11/16/19）、生成 C11 仍 **12513** 字节、
产物仍 **199975** 字节、`build-id 4d8250b5…55ba` **逐字节不变**（本轮没碰阶段 [7]-[9] 的发射与容器写入）、
`infy conformance --report` 仍 **exit 1**（83 = Pass 77 / Fail 0 / Blocked 6 · 92.8% · 覆盖 83/2300）、
`(cd examples/tour && infy fmt --check)` 仍 **exit 2**（F6 未裁决）。
**C 侧零改动**仍复跑四条链：`-c infy_rt.c` 与 `-c infy_ifbc.c` 各 **0 告警**（stderr 0 字节）、
rt **32335 assertions / 0 failures**、替身链与生产链 IFBC 各 **180/180** 且 stdout `cmp` **无差异**。

**顺手清掉的仓库卫生**：`src/fmt.rs.tmp`（3984 行的旧 `fmt.rs` 快照，`418a34c` 起被跟踪、无人引用、
cargo 也不会编译它）删除 —— 这是 BACKLOG F3 早已点名的一项，本轮动手时它已经和 `src/fmt.rs` 差 1382 行。

**明确未做 / 不能声称**：F2 勾掉只覆盖 `cargo check --offline --lib` 这一个口径；
`cargo build --offline --release` 与 bin 目标的告警本轮未逐条清理（release 构建 exit 0、无新告警来源改动）。
"两侧装载期校验等价"仍不许讲（F26 前半，清单 = D-42 (a)–(e)）；`[lints]` 的接线只证明**配置进入了诊断**，
不证明 §9.6 的其余表面（`.infyfmt`、`--diag-json` 的 lint 字段）已实现。


## F1 的 run #1 结论（本轮推送后回查，与 F2 同批落地）

推送第 **1** 次即成功（`6fceaa5..1896c2a`）⇒ `…/actions/workflows` 由 **0 → 1**、`…/actions/runs?branch=toolchain`
出现 **run 38892797672（run_number 1, event push）** ⇒ **分支上首条 workflow 不需要人工批准**。
该 run **conclusion = failure**，逐步：步 **1-6 success**（`Provision CPython 3.14` 与「工具链自检」都过 ⇒
`windows-latest` 上 `gcc` 在 PATH、`setup-python@v7` 有 CPython 3.14 ⇒ 过去两轮悬着的「runner 供给」两条疑问**都不成立**）；
步 **7 `cargo test --offline` failed** = `526 passed; 3 failed`；步 **8-12 skipped**（`set -e` 之后没跑到，
这 5 步在远端从未执行过一次 ⇒ 「IFBC 交叉验证在 CI 里过了」这类话今天仍然不许讲）。

**3 条红项是同一个根因，且是本仓库第三次踩行尾**：`cli::tests::a_module_without_embeds_produces_no_images`、
`run_executes_the_tour_and_exits_zero`、`test_refuses_cov_and_runs_zero_tests_where_there_are_none`，
远端日志打的是 `warning[W-0001]: CRLF 行结束符：应为 LF（共 28 处）`（`cmd/runner/main.infy`）与
`（共 38 处）`（`core/build/mod.infy`），断言形态是 `left: 2, right: 0`。根因 = runner 的 `core.autocrlf=true`
把**没有 EOL 属性**的 `examples/**.infy` 检出成 CRLF，而 §2.1 把行尾本身当被测面；本地是 LF ⇒ 529 项全绿也看不见。

**另有一条更贵的发现**：那条「检出没改写工作树」的守卫**判据本身是错的** —— `git status --porcelain` 为空
证明不了字节没被改写（clean 过滤器把 CRLF 换回 LF 再比较），所以 run #1 里这一步是 `success` 而工作树其实是 CRLF。
**修法（同批，不放宽任何断言）**：① `.gitattributes` 加 `*.infy text eol=lf` 与 `*.infy.toml text eol=lf`；
② CI 守卫步加一条直接查输入面字节：`LC_ALL=C grep -rlU CR --include=*.infy --include=*.infy.toml examples`
命中即红并打名单（`conformance/cases` **不查** —— 那里故意有写 CRLF 字节的正反例）；
③ 两条新断言钉进 `conformance::tests::the_ci_workflow_is_windows_only_and_keeps_the_three_anti_false_red_guards`
（`grep -rlU` 必须在 + `.gitattributes` 里 `*.infy` 必须在）。**本地负对照实测**：LF 树命中 **0** 条；
把 `examples/tour` 的两个 `.infy` 换成 CRLF 后命中 **2** 条且指名这两个文件，`infy check` 对同一份 CRLF 副本
exit **2** 并打 **2** 条 `W-0001` ⇒ 与远端逐字相符。**F1 仍 `[ ]`**：勾它的条件是那一次 run 的 conclusion = success，
run #1 是红的；修复已推送 ⇒ 下一轮取 run #2 的逐步结论（步 8-12 从没在远端跑过，仍可能另有红）。

## F3 轮（2026-10-16）：一次性产物脱离跟踪，仓库里只剩 `docs/RUNLOG.md` 这一份审计线

**改了什么**：① `.gitignore` 从"记具体名字"改成"记产物形状"（原来只有 `push.log` 一条，所以
`push2.log`/`push3.log` 一路被跟踪）；② 新脚本 `scripts/untrack-artifacts.ps1` 把仍在跟踪的 5 项
**只从索引里摘掉**（`git rm --cached`），文件留在磁盘；③ `conformance` 加 1 条回核，把这份名单与
"脚本的审计日志必须落在 `logs/`"钉成测试。

**为什么是 `--cached` 而不是删除**：那些 log 是历史轮次的现场记录，删掉等于销毁证据；F3 要求的是
**仓库里只有一份审计线**，不是本机不留痕。脚本逐条写了回滚命令（`git add -- <路径>`），完全可逆。

**实测**：干跑命中 **5** 项（`chunk.txt`、`logs/cleanup.log`、`logs/push-branch.log`、`push2.log`、
`push3.log`）、拒绝 **0** 项；`-Apply` exit **0**，`VERIFY mode=APPLY 命中=5 失败=0 剩余跟踪=0`；
复跑一次 = `NOOP 没有仍在跟踪的一次性产物` exit **0**（幂等）；5 个文件仍全部在磁盘上；
`git ls-files | grep -Ei '\.(log|tmp|o)$|^logs/|chunk\.txt'` = **0** 行。
`git check-ignore -v` 逐条命中：`push2.log` ← `.gitignore:15:*.log`、`logs/push-branch.log` ← `:16:logs/`、
`ifbc_tests.exe` ← `:3:*.exe`、`foo.tmp` ← `:17:*.tmp`、`bar.o` ← `:18:*.o`。

**顺带量掉的一条假设**：`.gitignore` 自己会不会也踩行尾（这台机 `core.autocrlf=true`，F1 的 run #1 就是被
这个坑红过一次）？在 `target/scratch/` 里 `git init` 一个隔离仓库、写入 **CRLF** 的 `.gitignore`
（`push2.log\r\n` + `*.tmp\r\n`），`git status --porcelain` 只剩 `?? .gitignore` ⇒ 两条模式**照常命中**，
git 的 ignore 解析器吃 CRLF ⇒ 不需要给 `.gitignore` 补 `text eol=lf`。**没有为这条没需要的规则去改文件。**

**反向红线（这条比名单本身更要紧）**：名单一旦宽到 `*.bin` / `*.md` / `*.rs` / 某个整目录，真值源会
**悄悄脱离跟踪** —— 跨语言逐字节夹具（`runtime/fixtures/*.bin`）不再入库、逐条款文档不再入库，而所有
测试仍然全绿。新测试因此两头都查：六条必需模式按**整行相等**比对（不是 `contains`，`*.log` 的
`contains` 会被 `*.logx` 糊过去），十四种禁止形状一个都不许出现。

**回归面**：`cargo test --offline --lib` **530 passed / 0 failed**（基线 529，+1 全在 `conformance`：
**17 → 18**）、`cargo check --offline --lib` **0 error / 0 warning**（口径未变）、
`cargo build --offline --release` exit 0、`infy check|build|run|test examples/tour` 全 exit **0**、
`--verify-reproducible` exit **0**（`verify-a/ci.exe` 与 `verify-b/ci.exe` 各 199173 字节且 `cmp` 相同）、
`infy artifact info … --nodes` exit **0**、生成 C11 仍 **12513** 字节、产物仍 **199975** 字节、
`build-id 4d8250b5…55ba` **逐字节不变**（本轮没碰 `src/` 的生产代码，只加了一条测试）、
`infy conformance --report` 仍 **exit 1**（83 = Pass 77 / Fail 0 / Blocked 6 · 92.8%）、
`(cd examples/tour && infy fmt --check)` 仍 **exit 2**（F6 未裁决）；
**C 侧零改动**仍复跑四条链：`-c infy_rt.c` 与 `-c infy_ifbc.c` 各 **0 告警**（stderr 0 字节）、
rt **32335 assertions / 0 failures**、替身链与生产链 IFBC 各 **180/180** 且两份 stdout `cmp` **无差异**。
