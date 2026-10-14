# 覆盖矩阵（条款 → 实现位置 → **可复跑的证据**）

本文件回答一个问题：**"某条声称完成了"到底是被什么证明的？**
规则只有一条：标 `✅` 必须能在 §1 的命令里数出对应的测试/断言；数不出来的一律写 `🟡`（实现了但证据不足）
或 `⬜`（未实现，运行时显式报错）。`src/host.rs`、`src/atrack.rs`、`src/conformance.rs`、`src/doc.rs`
四个模块的占位注释也指向这里（`src/hir.rs` 于 C1 轮从占位落成声明层 IR、`src/codegen.rs` 于 C2 轮落成
阶段 [8] 的 C11 发射，证据分别是 §3 的 stage [7] 与 stage [8] 两行）。

## 1. 证据命令（2026-10-13 B1 轮起逐轮复跑，主树非隔离副本；P11 于 B2 轮改判，P1/P2/P4/P9/P10/P11/P12 于 C3 轮逐条复跑，C4 轮复跑 P1-P4/P9-P15/P16 并新增 P16/P17，**C5 轮复跑 P1-P4/P9-P17 并新增 P18-P21**）

| # | 命令 | 输出里的数字 | exit |
| --- | --- | --- | --- |
| P1 | `cargo test --offline --lib` | `423 passed; 0 failed; 0 ignored`（D4(1) 轮 `ifbc` 6 → **19**；基线 410） | 0 |
| P2 | `cargo check --offline --lib` | 0 error、`51 warnings`（全部 unused/dead code，BACKLOG F2） | 0 |
| P3 | `infy check examples/tour` | `check ok — 2 file(s), 0 error(s), 0 warning(s), 0 notice(s)` | 0 |
| P4 | `infy build examples/tour` | 先打 P3 那行，再打 `codegen ok — 12513 byte(s) of C11 written to examples/tour/.infy-build/x86_64-pc-windows-gnu/ci.c (stages [7]-[8]; 1 chain(s), 6 node(s))`（C4 轮起从 12473 变来：`goto` 要 `return` 才能转移控制，偏差 D-34），再打 `artifact ok — 199975 byte(s) 写到 …\ci.exe（§7.4：宿主镜像 199173 + 段 blob 738 + 段尾 64，6 段）` + `build-id 4d8250b5…55ba`。**C3 轮起阶段 [9] 已落地，不再有"尚未实现"的 notice** | 0 |
| P13 | `infy build examples/tour --verify-reproducible` | `verify-reproducible ok — 两个独立构建目录产物字节相同（199975 字节）`（§7.3 第 3 项；不一致时打 `E-0603 · 首个差异在偏移 N（…段名…）` 并 exit 7） | 0 |
| P14 | `infy artifact info examples/tour/.infy-build/x86_64-pc-windows-gnu/ci.exe --nodes` | `.inf meta` 全字段读回：`ifabi 2 / opt_level 2 / usize_bits 64 / iter_guard_steps 4294967296 / target x86_64-pc-windows-gnu / source_toolchain gcc 16.2.0 / n_nodes 6 / entry chain 'Pipeline'`，节点表 6 行 = `cmd/runner/main.infy` 的 8/9/10/11/16/19（gate 与 retry_once 各 1 个连携块，锚点 begin=0·done=5，与 C1 金样一致） | 0 |
| P15 | `infy build examples/tour --cc /no/such/cc` | `error: E-0602 · … 无法启动 …（§7.6.2：A 轨工具链缺失或未注册，绝不静默跳过链接）`——缺编译器是**报错**不是"跳过链接后假装成功" | 6 |
| P12 | `gcc -std=c11 -Wall -Wextra -I runtime -c examples/tour/.infy-build/x86_64-pc-windows-gnu/ci.c -o ci.o` | **0 error / 0 warning**（无输出），产出 5723 字节 .o；这就是 BACKLOG C2 的验收命令。生成的 C 自带 `sizeof`/`_Alignof`/`offsetof` 断言，所以这条命令同时复核了阶段 [7] 的布局表 | 0 |
| P5 | `infy doc` / `ifbc` / `conformance` / `lsp` / `lib` / `bench` | `notice: infy <cmd> is not implemented in this build`（`run` 自 C4 轮起不在此列，见 P16/P17；`test` 自 C5 轮起不在此列，见 P18-P21。`bench` 留着是因为 §13.4 根本没有这个子命令 —— 它是 `infy test --bench`） | 1 |
| P16 | `infy run examples/tour` | 构建信息按 P4 原样在 stdout，然后**执行**产物并逐字节转发其流：程序 stdout **0 字节** / stderr **0 字节**，`run ok — 程序退出码 0（stdout 0 字节 / stderr 0 字节，§13.4）` 在 stderr。**链真的结束了**（D-34 之前它会无限循环）；stderr 为空同时钉住"容器被 `infy_art_self` 读到、策略取自 `.inf meta`"（读不到会打 `artifact rejected: …` 或 `runtime: no INFY artifact container …`）。程序 stdout 为空是因为**没有任何 INFY 构造能写 stdout**（F19），不是转发没接上 | 0 |
| P18 | `infy test <全绿的合成 module：一个 void `#[test]` + 一个返回 `Ok` 的 `Result` 测试>` | 构建信息按 P4 原样在 stdout，然后**执行** test 产物：程序 stdout **139 字节**逐字节 = `test example/ctest/cmd/ctest::calls_work ... ok` / `…::result_ok ... ok` / `test result: 2 passed, 0 failed, 2 total`（C 运行时文本模式 ⇒ `\r\n`，比较时归一），程序 stderr **0 字节**（同 P16：非空即说明容器/策略链断了）；`test ok — 程序退出码 0（stdout 139 字节 / stderr 0 字节，§13.4）` 在 stderr。锁定点 = `cli::tests::test_build_runs_the_harness_and_exits_zero`，期望串常量 `cli::tests::TEST_GREEN_REPORT` | 0 |
| P19 | `infy test <有测试返回 `Err` 的 module>` / `infy test <`#[test]` 里 `u32::MAX+1`（`overflow="panic"`）的 module>` | 两条不同的 §13.4 码：`Err` → **1**（一般性失败），报告 `…::a_ok ... ok` + `…::b_err ... FAILED` + `1 passed, 1 failed, 2 total`，**套件不中断**；panic → **3**（运行时 panic），§9.2 首行 `runtime[R-0001]: 整数溢出（\`overflow=panic\`）` 逐字节转发在 stderr，且**排序在 panic 之后的测试不出现在报告里、总结行不打**（§4.16 在焦点之外没有 catch 点 ⇒ `process_radius_exit()`；偏差 D-36(c)）。锁定点 = `cli::tests::{a_failing_test_is_exit_1_not_a_panic, a_panic_inside_a_test_is_exit_3_and_stops_the_suite}` | 1 / 3 |
| P20 | `infy test examples/tour` | 程序 stdout 恰为 `test result: 0 passed, 0 failed, 0 total`，**exit 0** —— 没有 `#[test]` 函数不是失败（§13.4：0 = 成功）；这一行同时反证 harness **没有调 `infy_runtime_start`**，`#![entry_chain]` 指向的 `Pipeline` 一个节点都没跑（否则 stdout 里会多出程序自己的输出且退出码来自调度器）。锁定点 = `cli::tests::test_refuses_cov_and_runs_zero_tests_where_there_are_none` | 0 |
| P21 | `infy test --cov <mod>` / `infy test --bench <有参数的 #[bench]>` / 同一 module 上的 `infy check` vs `infy test` | `--cov` → **1** 并指名 §12.4 的插桩没有阶段产出（F22，不静默忽略、不假报数字）；`--bench` 对一个 `fn bench_open(n: u32)` → 阶段 [8] 拒绝、**exit 6**，消息点名"§10 的 std 签名表里没有任何 `std.test` 条目"（F21），而不带 `--bench` 时同一函数根本不进运行集（`test result: 0 passed, 0 failed, 0 total`）；文件选择实测 `infy check` = **1 file(s)** 而 `infy test` = **2 files**（§5.9 的 `*_test.infy` 只进测试构建），`//infy:build ignore` 的类型不合法草稿在任何构建都不入选、`//infy:build test` 只在测试构建入选、`//infy:build riscv` → `E-0012`（§2.2，未知名检查在**选择之前**、对每个 discovered 文件做）。锁定点 = `cli::tests::{test_files_are_out_of_the_ordinary_build, build_constraints_gate_the_file_set_from_the_cli, dirhash_covers_files_the_build_excludes}` + `codegen::tests::bench_functions_enter_the_suite_only_under_bench`。**第三处同族风险已挡**：§5.8 第 1 项的 dirhash 覆盖 module 的**全部文件**，而 `cli::package_dirhashes` 当时取的是 `Loaded::files` —— 文件一按 §5.9 过滤，dirhash（进而 §7.3 的 build-id）就成了"哪条命令在跑"的函数；现在它从 `pkg::discover` 重新取全树，两种 scope 下哈希逐一相等。 | 1 / 6 / 0 |
| P6 | `gcc -std=c11 -O1 -Wall -Wextra runtime/infy_ifbc.c runtime/infy_dyn.c runtime/test_ifbc.c runtime/test_ifbc_stubs.c -o ifbc_tests && ./ifbc_tests` | `IFBC harness: 146 asserts, 146 pass, 0 FAIL`；**0 编译告警**（B1 轮给 `InfyString` 补了结构标签，原先 4 条 `infy_rt.h:212` 前置引用告警随之消失 = B3 的第 4 项；79 → 82 是 B3 轮为 D-22 裁决新增的一行 `closure.new` 拒绝断言，3 项；82 → **146** 是 D4(2) 轮的合并形状 + 17 条间隙指令 + Rust 字节夹具，见 `docs/status.md` 的 D4(2) 一节。夹具那组断言要在 `target/scratch` 里跑，路径按 `../../runtime/fixtures/…` 取） | 0 |
| P9 | `gcc -std=c11 -O1 -Wall -Wextra runtime/infy_rt.c runtime/test_rt.c -o rt_tests && ./rt_tests` | `32335 assertions, 0 failures`，0 error / 0 warning；诊断渲染单独再跑一次为 `rt_render.log` 逐字节比对。**链的是生产 `runtime/infy_rt.c`**，不是替身。C3 轮 +32（基线 32303）：D-33 的 `mode=2`"宿主镜像自带 `INFYART2` 字面量仍须可读"5 项，
加用生产读取器 `infy_art_read_file` 读回 **Rust 写入器**逐字节夹具
`runtime/fixtures/artifact_rust_writer.bin` 的 27 项字段/池断言 = Rust 写 ↔ C 读的常驻锁点。
**跑之前必须 `cd target/scratch`**（夹具按 `../../runtime/fixtures/…` 取，换目录跑会红而不是空跑） | 0 |
| P10 | `gcc -std=c11 -Wall -Wextra -c runtime/infy_rt.c` | 0 warning。**B3 已勾**：这条命令 B1 轮就绿，B3 轮真正落地的是另外三项语义——`InfyRCode` 补 `R_EMBED_TIMEOUT=0x0701`/`R_FIELD_OFF_REPR=0x0713`、删除规范里不存在的 `R_IFBC_BOUNDARY=0x070A` 并按 D-22 把装载期"边界"改报 `R-0707`、`frame_kind` 两级取值表进冻结头（`INFY_FRAME_STACK`/`INFY_FRAME_COROUTINE`） | 0 |
| P11 | `gcc -std=c11 -O1 -Wall -Wextra runtime/infy_ifbc.c runtime/infy_dyn.c runtime/infy_rt.c runtime/test_ifbc.c -o ifbc_real && ./ifbc_real` | **B2 轮：真实链接跑通**。0 error / **0 warning**，`IFBC harness: 146 asserts, 146 pass, 0 FAIL`（D4(2) 轮起），exit 0，且 stdout 与 P6（替身链）**逐字节相同**。链接失败的根因不是工具链：`test_ifbc.c` 用 stub 专有的 `infy_stub_live_objs()` 当泄漏探针，改走公共 API `infy_mem_stats().live_objs`（`runtime/infy_rt.h`）后两侧皆可链接 | 0 |
| P7 | `(cd examples/tour && infy fmt --check)` | `2 file(s) are not canonical` — §14 的导览源码不是本格式化器的不动点（见 §5） | 2 |
| P8 | `infy artifact info README.md`（任意非产物文件） | `error[E-0605]: README.md 不是可读的 INFY 产物：no INFYART2 trailer at end of file (not an INFY artifact)`；损坏的 crc / 重复段尾 / 越界区段 / `INFYMETA` 魔数不符各有一条同族消息与单测 | 7 |

P6 必须**四个 .c 一起编**：漏掉 `infy_dyn.c` 时 `ld` 只返回 `collect2: error: ld returned 5`，
不打印任何 undefined reference，看起来像工具链坏了。P11 同理（把 `test_ifbc_stubs.c` 换成 `infy_rt.c`）。
遇到这种"静默 ld 5"时的确诊手法（B2 轮用过一次就定位）：逐个 `gcc -c` 出 `.o`，
`nm --defined-only` 求并集、`nm -u` 求未决并集，`comm -23` 一减就是真缺口——本轮减出来只有一个
`infy_stub_live_objs`，其余全是 CRT/kernel32 正常导入。注意 `nm -u -P` 在本机会输出单列 `U`，解析要直接用 `nm -u`。

## 2. 前端与语义（Rust 侧）

`cargo test --offline --lib` 的 **423** 项按模块分布（D4(1) 轮实测分组计数；此前这里写 354 项、且表里漏了
`codegen`/`hir`/`artifact` 三行 —— 那是 C1 轮之前的数字，已过期）：

| 模块 | 项数 | 条款 |
| --- | --- | --- |
| `lexer` | 79 | §2 词法全集（含 §2.4 保留字/`E-0020`、`@embed` 不透明区、字素簇宽度） |
| `derive` | 57 | §6.7 derive（含 3 项 `E-0421`） |
| `fmt` | 39 | §9.5 十四条规则（幂等、注释不丢、三种宽度带金样） |
| `macro_expand` | 35 | §6.1–§6.5 |
| `sema` | 22 | §4.1 内建构造/未声明名、§4.2 无隐式转换（含 `type_compatible` 逐臂）、§4.3、§4.6 `env`、§2.8 字面量取型、§4.13 规则 7、§8.2/§8.4 `goto`/`start` |
| `pkg` | 24 | §5.2 清单、§5.3 导入解析顺序、§5.7 semver/MVS、§5.8 dirhash 与 lock、§2.2 约束表达式求值、§5.9 测试文件选择、`discover()` 的 §5.1 R1/R2 形状与排序（C5 轮 +5） |
| `codegen` | 22 | §7.1 阶段 [8]（C2 轮 18 项 + C4 轮 `goto` 转移控制 1 项 + C5 轮 §5.9 harness 3 项）：布局自检、检查型算术、`?`、`match`、清理序、节点表、调用解析、测试 harness、字节稳定 |
| `hir` | 20 | §7.1 阶段 [7] 声明层：ABI §1 表示表逐行（对回 `gcc` 探针）、§4.6/§10 `EnvOffsets`、§5 链帧、§8.4 节点表金样、§2 符号 mangle、§7.4 入口次序、D-24/D-25 |
| `artifact` | 18 | §7.4 容器写+读、§7.3 九类 build-id 输入逐个绑定、`INFY_META_OFF_*` 钉回冻结头文本、七个拒绝理由各一条 |
| `toml` | 15 | §5.2 / 附录 E.2–E.4 |
| `borrow` | 15 | §4.7 借用检查 + §4.3/UB-006 `E-0121`（4 项，A4 轮补） |
| `sha256` | 12 | §5.8 |
| `codes` | 9 | §9.1–§9.3/§9.7，`count() == 210` |
| `base64` | 7 | §5.8 |
| `parser` | 8 | §3 文法 + §8.4 节点编号（1 项）+ §3.5 `start(chain=…)` + §3.6 嵌套泛型闭合 + C1 轮 `enum` 显式判别式到达 AST 且假判别式 `E-0053` |
| `ifbc` | **19** | §7.8 Rust 侧编码/反汇编/装载校验；D-23 的两级 `frame_kind` 取值表；**D4(1) 轮新增 13 项**锁 D-38 的合并线格式（区段自定界偏移、`TyKind` 重编号 + 前 18 行 == 标量码、import/func 记录形状与 `max_mem_bytes`、opcode 字节与操作数个数对照表、`switch` 变长往返与边界、`ldw` 注解域、`field_off` 三态、`max_stack` 高度、`tailcall` 帧兼容报 `R-0707` 不报 `R-0709`、`ret_arity` 正反例） |
| `cli` | 16 | §13.4 子命令与退出码（C4 轮 `infy run` +3、C5 轮 `infy test`/§5.9 文件选择/§5.8 dirhash 不随构建范围变/`--cov` 拒绝 +7） |
| `stdlib` | 4 | §10 签名表 + §1.3 领域中立 |
| `diag` | 2 | §9.2 版式（规范样例逐字节 1 项 + caret 行规则 1 项） |

**没有一项独立单测的模块**：`btrack.rs`（§7.7 B 轨，4476 行）、`span.rs`/`token.rs`/`ast.rs`（结构体，由上述模块间接使用）。
`sema.rs`/`pkg.rs` 的零单测状态在 A4 轮结束（41 项，见上表），它们剩下的证据仍是 P3（`infy check examples/tour`
跑完 §7.1 阶段 [1]–[5] 且 exit 0）——**新测试锁的是判据与码，不等于分支覆盖**。

## 3. 逐条状态

| 条款 | 实现位置 | 状态 | 证据 / 缺口 |
| --- | --- | --- | --- |
| §2 词法 | `src/lexer.rs` | ✅ | 79 项（P1） |
| §3 语法 | `src/parser.rs` | 🟡 | 8 项 + P3 全量解析；（C1 轮补的第 8 项锁住 §4.1 枚举判别式，见偏差 D-24 —— 它此前被静默丢弃）没有"EBNF 全产生式是否都落到 AST"的审计，**"全产生式"这句在 README 里已降级** |
| §7.1 [7] IFIR（**声明层**） | `src/hir.rs` | 🟡 | C1 轮 **20 项**：§1 表示表逐行（`sizeof`/`_Alignof`/`offsetof` 由 `gcc` 实测对回）、§4.6/§10 `EnvOffsets` 11 个偏移 + 72/8、§5 链帧与 `InfyNodeLoc`/`InfyChainDesc`/`InfyFocusId3` 行、§8.4 节点表、§2 符号 mangle、§4.8 `offset_of` 只对 `repr(C)`、§4.15 `E-0175`、D-24/D-25。**缺口**：语句/表达式层没落（C2 的 `codegen` 直接从 AST 发射），这 20 项锁的是表示与判据而非端到端；C3 轮起 `.inf meta` 的 `ChainTable`/`NodeTable` 就是从这张表写出去的（P14 读回的对上一致） |
| §4 类型检查 | `src/sema.rs` | 🟡 | A4 轮补 **22 项**（内建构造、未声明名、字面量取型/后缀/范围、无隐式转换含 `type_compatible` 逐臂、`E-0120`、`env` 封闭集与 `E-0140`、`goto`/`start`）。缺口：`?` 展开、生命周期 elision、穷尽性 `E-0070/0071/0072`、const-eval、`unsafe` 六禁令仍只有 P3 |
| §4.7 借用检查 | `src/borrow.rs` | ✅ | 15 项：`E-1001`/`E-1002`/`E-1004`/`E-1005`/`E-1006` 正反例 + `E-0121` 4 项（A4 轮补）。`E-0121` 的判据低估数值绑定，见偏差 D-19 / BACKLOG F7 |
| §5 包/依赖 | `src/pkg.rs` | 🟡 | A4 轮补 **19 项**（清单逐段与默认值、`E-0506`/`E-0507`/`W-0500`、§5.3 解析顺序、`^` 区间、MVS 最小选择与冲突、dirhash 顺序无关与路径绑定、lock 往返/排序/`E-0200`）；C5 轮再补 **5 项**：`//infy:build` 表达式对 `BuildConfig` 逐名求值（含 `ignore`/`test`/feature/半截表达式）、§5.9 的 `file_selected` 矩阵（`_test.infy`、`tests/…`、`!test`、两道门同时过）、`discover()` 接受 §14.1/§5.9 的三种包名形状、R2 仍拒真正 unrelated 的两包名、`read_dir` 顺序不再漏进产物。**C5 之前的实情**：`discover()` 只有 tour 那一种布局作证据，而 `constraint_matches`/`unknown_constraint_names`/`is_test` 三处**零调用点** |
| §5.8 哈希/base64 | `sha256.rs` `base64.rs` | ✅ | 12 + 7 项（NIST、RFC 4648 向量、流式百万字节） |
| §5.2/E.2–E.4 TOML | `toml.rs` | ✅ | 15 项（规范 fixture 往返 + fuzz 不 panic） |
| §6.1–6.5 宏 | `macro_expand.rs` | ✅ | 35 项 |
| §6.7 derive | `derive.rs` | ✅ | 57 项 |
| §8.4 节点编号 | `sema.rs` + `parser.rs` + `hir.rs` | 🟡 | 编号 1 项（`parser`）+ A4 轮 6 项（`sema`）+ C1 轮把 §14 tour `Pipeline` 的 6 节点表逐行钉成金样（`hir::tests::tour_pipeline_node_table_matches_the_source`）：命名锚点缺失、绝对索引越界（并 `N-0800`）、相对偏移前/后越界、序言语句不移动编号、`start` 的链名/锚点。缺口：`N-0800` 只在 strict 之外的展示层，`--deadlock`/`W-0800` 无测试 |
| §9.1–9.3/9.7 诊断码 | `codes.rs` | ✅ | 9 项 + `count()==210`（`E-0093` 来自 §9.6 正文，见 `codes.rs:2052`） |
| §9.2 诊断版式 | `diag.rs` | 🟡 | 2 项，其中逐字节金样只有 §9.2 那一条样例 |
| §9.4/E.5 `--diag-json` | `cli.rs:416` | 🟡 | 能写出 `{"version":1,"tool":{...},"diagnostics":[...]}`，但结构无金样测试 |
| §9.5 格式化 | `fmt.rs` | ✅ | 39 项 |
| §1.3 领域中立 | `stdlib.rs` | ✅ | 4 项（NFKCCaseFold 全角/大小写绕过） |
| §10 `std` 签名表 | `stdlib.rs` | 🟡 | 表本身 4 项；消费者 `infy doc`（`doc.rs` 占位）未实现。codegen 侧自 C2 轮起**有映射**，但只覆盖 `infy_rt.h` 里有入口的部分：`Vec`/`String` 的 `push`/`len`/`as_str`、`Option`/`Result` 的 `unwrap` 族、`std.fmt` 的 `infy_fmt_run`（但 `fmt!` 宏本身没有 lowering 路径 → F15），其余 std 调用一律显式拒绝 |
| §7.3 可复现构建 / build-id | `artifact.rs::build_id` + `cli.rs::compute_build_id`/`--verify-reproducible` | 🟡 | **18 项 `artifact` 单测**里的 4 条专锁 build-id：`build_id_is_deterministic`、`every_build_id_input_class_moves_the_digest`（§7.3 第 1 项列的九类输入**逐个单独**改动都必须让摘要变化，防"某一类其实没进哈希"）、`build_id_binds_order_and_field_boundaries`（§5.4 顺序 + 长度前缀边界）、`writer_output_is_byte_identical_for_the_same_content`。端到端 = P13（两个独立构建目录 199975 字节逐字节相同）。记 🟡 的理由：① 第 4 项"A 轨工具链输出的逐字节重放校验（`[6b]` 记录版本/命令行/输入哈希）"未做；② 时间戳中和只覆盖了 GNU ld 的 `--no-insert-timestamp`，别的链接器等价物未测；③ §12 的外部合规用例仍为 0 |
| §7.4 产物容器 | `artifact.rs`（写 + 读）+ `cli.rs::stage9` | 🟡 | C3 轮落成：**18 项 `artifact` 单测** + P14（`infy artifact info` 读回自家产物，节点表 6 行对回 C1 金样）+ P9 的 Rust 写 ↔ C 读常驻夹具 + 一次性探针（生产 `infy_art_read_file` 读真实 tour 产物：`rc=0 blob_len=738`，meta 与 Rust 侧逐字段一致）。记 🟡 的理由：① `.inf text`/`.inf rt` 靠"宿主镜像 = gcc 直接链出的可执行文件"，尚未做 §7.2 的 LTO/多目标文件编排；② `.inf ifbc` 恒为空（D1 未做）；③ `ResourceKeys`/`EnvOffsets` 区写的是空集（前端不收集 `std.chain.lock` 键名）；④ 规范级缺陷 D-33（重复段尾扫描范围）已裁，但要等项目所有者认可 |
| §7.8/附录 D IFBC | `src/ifbc.rs` + `runtime/infy_ifbc.c` | 🟡 | Rust 侧 **20** 项、C 侧 **146** 断言；码表与 §9.7 对齐（D-22/D-23）。**D4(1)+D4(2) 已把两侧改成 D-38 裁定的同一份线格式**：区段自定界（删掉 `byte_len‖body` 包装）、三表合并形状（`type_table` 8×u32+字段、`import_table` 尾带 `sig_hash`、`func_table` 48 字节 + `max_mem_bytes u64`）、`TyKind` 重编号到 `SIG_*` 且前 18 行自动置标量行、guard/ldw/box.dyn/field_off 操作数面与 `switch` 变长解禁、C 侧 `exec` 按 `types[op0].kind` 选 i64/f64 路径、17 条间隙指令在 C 侧补齐解码/校验/反汇编。两侧**第一次在字节上相遇** = `runtime/fixtures/ifbc_rust_writer.bin`（Rust 写 → C 生产装载器读 + 执行）。镜像表 = `docs/internals/ifbc-encoding-rust.md`（Rust）与 `runtime/IFBC.md`（C，D4(2) 轮按落地字节重写）。**仍 🟡**：同一镜像的**反汇编文本逐指令相等**没做（余项 (3)）、C 侧六条间隙指令装载通过而执行显式 R-0710（F27 / 偏差 D-41）、两侧装载期校验结论不等价（F26）、`.inf ifbc` 仍恒空（D1） |
| §7.7 B 轨 | `btrack.rs` | 🟡 | 4476 行、0 测试，端到端未收敛（D1） |
| §7.6/§7.7 A 轨绑定 | `atrack.rs` | ⬜ | 占位（D3） |
| §7.1 阶段 [8] / §7.2 检查永不关 | `codegen.rs` | 🟡 | C2 轮落成 **18 项** + C4 轮 1 项（`goto` 立即结束节点）+ C5 轮 3 项（§5.9 harness）= **22 项** + P12（gcc 编译生成的 C，0 告警）。记 🟡 而不是 ✅ 的理由：① §12 外部用例仍为 0；② tour 之外的构造多数是**显式拒绝**（闭包、`dyn`、`@embed`、A 轨调用、`for`/`defer`、128 位、窄位移、`saturate`、移动 owned 值、有参数的 `#[bench]`）；③ 没有独立的语句/表达式级 IR，阶段 [7] 与 [8] 之间不可优化；④ 单 TU 发射（偏差 D-26），§5.10 的按 package 增量未做 |
| §7.1 阶段 [9] 链接成单文件产物 | `cli.rs::stage9`（`cc` 驱动）+ `artifact.rs::write` | 🟡 | C3 轮落地：`infy build examples/tour` 真产出 199975 字节的 §7.4 容器并 **exit 0**（P4），`--cc` 缺失/不可启动 → `E-0602`/exit 6（P15，不假装成功）。记 🟡 的理由：A 轨只走"把生成的 C 与 `infy_rt.c`/`infy_dyn.c` 一起交给 cc"这一条最简直路径，§7.6 的 `#[link]` 目标码、§7.2 的 LTO、`infyl` 独立链接器驱动都没有；`.infib` 依赖的链接（§13.2）也没有 |
| §7.1 阶段 [10] 签名/校验段写入与 lock 比对 | `.inf sum` 由 `lock.dirh` 生成（`cli.rs::stage9`） | ⬜ | 只做了"把依赖 dirhash 清单写进容器"这一半（tour 无依赖 → 0 条，实例上是空集）；§5.8 的 **sumdb 记录/inclusion proof 与运行时自校验比对**没有，`infy artifact verify` 一类命令也不存在。BACKLOG 未单列该项，记在这里以免被 [9] 的绿误读成"产物校验已通" |
| §11 `HostApi`/三步握手 | `host.rs` | ⬜ | **占位**。（`EnvSnapshot` 的偏移表已由 C1 轮在 `hir.rs::env_table` 落成并有金样，握手本身仍未实现）（本轮之前的 README 把它写成 ✅，是文档错误，非实现事实） |
| §12 合规套件 | `conformance.rs` | ⬜ | 0 个外部用例；规范要求 2300（§12.1 1244 + §12.2 256 + §12.5 800），**通过率目前不可测**（E1/E2/E3） |
| §13.4 CLI | `cli.rs` | ✅ | **16 项** + P3/P4/P5/P7/P8 的 exit 码逐条对上 + P16/P17（`infy run`）+ P18-P21（`infy test`）。§13.4 列出的命令里仍显式报未实现的：`doc`/`lib`/`ifbc`/`ingest`/`conformance`/`lsp`/`pack`/`unpack`/`install`/`sumdb`/`database`/`bench` |
| §5.9 测试与基准包 | `pkg.rs::file_selected` + `cli.rs::test_cmd` + `codegen.rs::emit_test_driver` | 🟡 | C5 轮落地：`infy test` = 测试构建（`*_test.infy`/`tests/…`/`//infy:build test` 只在这里入选）+ 生成的 `#[test]` harness 作入口 + §13.4 的 0/1/3。P18-P21 + `pkg::tests::test_files_enter_only_the_test_build` + `codegen::tests::{harness_runs_each_test_in_sorted_order_and_judges_result_by_is_ok, harness_refuses_tests_it_cannot_call_or_judge, bench_functions_enter_the_suite_only_under_bench}`。记 🟡 的理由：① `std/test`（§14.7 的 `assert_eq`/`Bench`）在 §10 的 std 表里一条都没有 ⇒ 规范自己的测试样例过不了 `check`，`#[bench]` 有参数即拒（**F21**）；② panic 不逐测试隔离，整条套件中止（要动冻结头，偏差 D-36(c)）；③ `--cov` 显式拒（**F22**）；④ test 产物的 `.inf meta` 入口字段仍写 §7.4 那一档（**F23**）|

## 4. C 侧运行时

| 项 | 状态 | 证据 |
| --- | --- | --- |
| `infy_ifbc.c` 装载/校验/解释 | ✅ 对 harness 而言 | P6：146 asserts / 146 pass，含六项装载校验、逐字节截断、单字节变异、800 轮 fuzz、间隙指令的执行/显式拒绝，以及一份 **Rust 写入器字节** 的装载+执行夹具。P11 链生产 `infy_rt.c` 再跑一次 = 同一 146 项、输出逐字节相同 |
| 被测对象是**生产运行时**吗 | ✅ 两个 harness 都是（B2 轮） | P9 链 `infy_rt.c`（rt **32335** 项，C3 轮起）；P11 也链 `infy_rt.c`（IFBC **146** 项）。P6 的替身链保留作为对照，`test_ifbc_stubs.c` 仍未从构建里退役。**这不等于 §12 合规**：`infy conformance` 与外部用例仍是 E1/E2；§7.8/附录 D 的线格式两侧已同源（D4(1)+D4(2)，并有 `ifbc_rust_writer.bin` 作为"Rust 写 ↔ C 读+执行"的常驻锁点），还差 D4(3) 的**逐指令反汇编相等**；且本侧装载校验对 `call.virt/call.embed/invoke.dynamic` 是**拒绝**而 C 侧执行（F26），所以"两侧校验结论等价"这句话现在不能讲 |
| `infy_rt.c`（检查型算术、drop/defer 序、Vec/String 增长因子、NFC 比较、Map/VecMap/Deque 序、xxHash64、std.fmt 字节精确、SplitMix64+PCG32+无偏 Fisher–Yates、ResourceLock FIFO、焦点调度与 `focus`/`chain` 半径、§9.2 渲染、产物容器、host 三步握手、std.fs、mem stats/leak check/包初始化） | ✅ 对 harness 而言（B1 轮收敛） | P9：`32335 assertions, 0 failures`，直接链生产 `infy_rt.c`（无替身）。**这不等于 §12 合规**：外部用例目录与 `infy conformance` 仍是 E1/E2 |
| `infy_rt.h` 已知瑕疵 | ✅ B3 轮清完（四项） | B1 轮已修：`InfyString` 无结构标签造成的前置引用（顺带删掉 `infy_rt.c` 里为此写的函数指针绕过 cast）、`InfyMetaFixed.iter_guard_steps` 是 `uint32_t`（磁盘格式与 `infy_profile` 都是 64 位，§10.10 默认 2^32 会被截成 0）。B3 轮已修：`InfyRCode` 补 `R-0701`/`R-0713`；`R_IFBC_BOUNDARY=0x070A` 因不在规范任何码表里而删除，装载期"边界"按 D-22 改报 `R-0707`，`R-0709` 只表示 §9.7 的内存配额；`frame_kind` 的两级取值表以命名常量 `INFY_FRAME_STACK`/`INFY_FRAME_COROUTINE` 进冻结头（D-23，C 与 Rust 两侧同一判据）。**仍开且已明确记录不假实现**：头文件没有 IFBC 模块卸载入口与 host thunk 绑定入口（`runtime/IFBC.md` §7 记为 group-(b) 私有入口） |

## 5. 本轮核对时新发现的两个真实缺陷（未修，已进 BACKLOG）

1. `infy fmt <目录>` 与 `infy expand <目录>` 把目录参数当文件读：`error: 无法读取 examples/tour: 拒绝访问。 (os error 5)`、
   `error: 找不到 module 根（§5.1）`；必须先 `cd` 进 module 或传具体 `.infy` 文件。`check` 却接受目录 —— 同一 CLI 内 positional 语义不一致。
2. P7：`examples/tour` 的两个源文件按 §9.5 **不是格式化不动点**（`2 file(s) are not canonical`）。
   要么格式化器与规范样例互相矛盾（规范级），要么导览源码需要按 §9.5 重写 —— 未裁决前，任何文档不得声称"§14 导览已是规范写法"。
