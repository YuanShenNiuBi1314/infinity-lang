# 覆盖矩阵（条款 → 实现位置 → **可复跑的证据**）

本文件回答一个问题：**"某条声称完成了"到底是被什么证明的？**
规则只有一条：标 `✅` 必须能在 §1 的命令里数出对应的测试/断言；数不出来的一律写 `🟡`（实现了但证据不足）
或 `⬜`（未实现，运行时显式报错）。`src/host.rs`、`src/hir.rs`、`src/codegen.rs`、`src/atrack.rs`、
`src/conformance.rs`、`src/doc.rs` 六个模块的占位注释也指向这里。

## 1. 证据命令（2026-10-13 本轮实测，主树非隔离副本）

| # | 命令 | 输出里的数字 | exit |
| --- | --- | --- | --- |
| P1 | `cargo test --offline --lib` | `285 passed; 0 failed; 0 ignored` | 0 |
| P2 | `cargo check --offline --lib` | 0 error、`51 warnings`（全部 unused/dead code，BACKLOG F2） | 0 |
| P3 | `infy check examples/tour` | `check ok — 2 file(s), 0 error(s), 0 warning(s), 0 notice(s)` | 0 |
| P4 | `infy build examples/tour` | 先打 P3 那行，再打 `notice: ... stages [8] codegen and [9] link are not implemented yet` | 6 |
| P5 | `infy run` / `test` / `doc` / `ifbc` / `conformance` / `lsp` / `lib` | `notice: infy <cmd> is not implemented in this build` | 1 |
| P6 | `gcc -std=c11 -O1 -Wall -Wextra runtime/infy_ifbc.c runtime/infy_dyn.c runtime/test_ifbc.c runtime/test_ifbc_stubs.c -o ifbc_tests && ./ifbc_tests` | `IFBC harness: 79 asserts, 79 pass, 0 FAIL`；编译期 4 条 `infy_rt.h:212` 告警（= BACKLOG B3） | 0 |
| P7 | `(cd examples/tour && infy fmt --check)` | `2 file(s) are not canonical` — §14 的导览源码不是本格式化器的不动点（见 §5） | 2 |
| P8 | `infy artifact info <任意非产物文件>` | `error[E-0605]: ... 找不到 INFYART2 段尾` | 7 |

P6 必须**四个 .c 一起编**：漏掉 `infy_dyn.c` 时 `ld` 只返回 `collect2: error: ld returned 5`，
不打印任何 undefined reference，看起来像工具链坏了。

## 2. 前端与语义（Rust 侧）

`cargo test --offline --lib` 的 285 项按模块分布（`cargo test -- --list` 计数）：

| 模块 | 项数 | 条款 |
| --- | --- | --- |
| `lexer` | 79 | §2 词法全集（含 §2.4 保留字/`E-0020`、`@embed` 不透明区、字素簇宽度） |
| `derive` | 57 | §6.7 derive（含 3 项 `E-0421`） |
| `fmt` | 39 | §9.5 十四条规则（幂等、注释不丢、三种宽度带金样） |
| `macro_expand` | 35 | §6.1–§6.5 |
| `toml` | 15 | §5.2 / 附录 E.2–E.4 |
| `sha256` | 12 | §5.8 |
| `borrow` | 11 | §4.7 借用检查 |
| `codes` | 9 | §9.1–§9.3/§9.7，`count() == 210` |
| `base64` | 7 | §5.8 |
| `parser` | 5 | §3 文法 + §8.4 节点编号（1 项） |
| `ifbc` | 5 | §7.8 Rust 侧编码/反汇编 |
| `cli` | 5 | §13.4 子命令与退出码 |
| `stdlib` | 4 | §10 签名表 + §1.3 领域中立 |
| `diag` | 2 | §9.2 版式（规范样例逐字节 1 项 + caret 行规则 1 项） |

**没有一项独立单测的模块**：`sema.rs`（§4 类型检查，2396 行）、`pkg.rs`（§5 清单/MVS/lock/dirhash，1127 行）、
`btrack.rs`（§7.7 B 轨，4476 行）、`span.rs`/`token.rs`/`ast.rs`（结构体，由上述模块间接使用）。
它们目前的证据只有 P3（`infy check examples/tour` 跑完 §7.1 阶段 [1]–[5] 且 exit 0），属于"端到端跑通一次"，
不等于分支覆盖。

## 3. 逐条状态

| 条款 | 实现位置 | 状态 | 证据 / 缺口 |
| --- | --- | --- | --- |
| §2 词法 | `src/lexer.rs` | ✅ | 79 项（P1） |
| §3 语法 | `src/parser.rs` | 🟡 | 5 项 + P3 全量解析；没有"EBNF 全产生式是否都落到 AST"的审计，**"全产生式"这句在 README 里已降级** |
| §4 类型检查 | `src/sema.rs` | 🟡 | 0 项独立单测；仅 P3。`Ok/Some/None` 内建、上下文取型、`?` 展开、elision、穷尽性、const-eval、`unsafe` 六禁令都只有 tour 走到的那部分被真实执行过 |
| §4.7 借用检查 | `src/borrow.rs` | ✅ | 11 项：`E-1001`/`E-1002`/`E-1004`/`E-1005`/`E-1006` 正反例。**`E-0121` 实现了但没有测试** |
| §5 包/依赖 | `src/pkg.rs` | 🟡 | 0 项独立单测；仅 P3 装载 `examples/tour` 的 2 文件多 package 布局 |
| §5.8 哈希/base64 | `sha256.rs` `base64.rs` | ✅ | 12 + 7 项（NIST、RFC 4648 向量、流式百万字节） |
| §5.2/E.2–E.4 TOML | `toml.rs` | ✅ | 15 项（规范 fixture 往返 + fuzz 不 panic） |
| §6.1–6.5 宏 | `macro_expand.rs` | ✅ | 35 项 |
| §6.7 derive | `derive.rs` | ✅ | 57 项 |
| §8.4 节点编号 | `sema.rs` + `parser.rs` | 🟡 | 1 项（源码序编号）。`goto("anchor")`/绝对/相对越界三条 `E-0301` 路径**无测试** |
| §9.1–9.3/9.7 诊断码 | `codes.rs` | ✅ | 9 项 + `count()==210`（`E-0093` 来自 §9.6 正文，见 `codes.rs:2052`） |
| §9.2 诊断版式 | `diag.rs` | 🟡 | 2 项，其中逐字节金样只有 §9.2 那一条样例 |
| §9.4/E.5 `--diag-json` | `cli.rs:416` | 🟡 | 能写出 `{"version":1,"tool":{...},"diagnostics":[...]}`，但结构无金样测试 |
| §9.5 格式化 | `fmt.rs` | ✅ | 39 项 |
| §1.3 领域中立 | `stdlib.rs` | ✅ | 4 项（NFKCCaseFold 全角/大小写绕过） |
| §10 `std` 签名表 | `stdlib.rs` | 🟡 | 表本身 4 项；其**消费者** `infy doc`（`doc.rs` 占位）与 codegen 映射（`codegen.rs` 占位）未实现 |
| §7.3/§7.4 产物容器 | `cli.rs:758` `artifact_cmd` | 🟡 | 只识别 64 字节 `INFYART2` 段尾；`.inf meta`/`rodata 索引`/`ifbc`/`sum` 段的写入器与解析器都不存在（BACKLOG C3，P8） |
| §7.8/附录 D IFBC | `src/ifbc.rs` + `runtime/infy_ifbc.c` | 🟡 | Rust 侧 5 项、C 侧 79 断言各自通过，但**两侧操作数个数不一致（BACKLOG D4）**，同一镜像的双侧反汇编目前必然错位，故不能记 ✅ |
| §7.7 B 轨 | `btrack.rs` | 🟡 | 4476 行、0 测试，端到端未收敛（D1） |
| §7.6/§7.7 A 轨绑定 | `atrack.rs` | ⬜ | 占位（D3） |
| §7.5 C11 发射 | `codegen.rs` | ⬜ | 占位（C2），P4 显式报错 |
| §11 `HostApi`/三步握手 | `host.rs` | ⬜ | **占位**。（本轮之前的 README 把它写成 ✅，是文档错误，非实现事实） |
| §12 合规套件 | `conformance.rs` | ⬜ | 0 个外部用例；规范要求 2300（§12.1 1244 + §12.2 256 + §12.5 800），**通过率目前不可测**（E1/E2/E3） |
| §13.4 CLI | `cli.rs` | ✅ | 5 项 + P3/P4/P5/P7/P8 的 exit 码逐条对上 |

## 4. C 侧运行时

| 项 | 状态 | 证据 |
| --- | --- | --- |
| `infy_ifbc.c` 装载/校验/解释 | ✅ 对 harness 而言 | P6：79 asserts / 79 pass，含六项装载校验、逐字节截断、单字节变异、800 轮 fuzz |
| 被测对象是**生产运行时**吗 | ❌ 不是 | P6 链的是 `test_ifbc_stubs.c`（harness 专用替身），不是 `infy_rt.c` → BACKLOG B2 |
| `infy_rt.c`（检查型算术、Vec/String 增长因子、Cleanup deque drop 序、焦点调度、ResourceLock FIFO、xxHash64、SplitMix64+PCG32、无偏 Fisher–Yates） | 🟡 代码在、测试未收敛 | `runtime/test_rt.c` harness 补齐 = B1；`infy_rt.h` 的 `R-0701`/`R-0713` 缺失与 `R_IFBC_BOUNDARY` 映射冲突 = B3（P6 里那 4 条 `infy_rt.h:212` 告警即其一项） |

## 5. 本轮核对时新发现的两个真实缺陷（未修，已进 BACKLOG）

1. `infy fmt <目录>` 与 `infy expand <目录>` 把目录参数当文件读：`error: 无法读取 examples/tour: 拒绝访问。 (os error 5)`、
   `error: 找不到 module 根（§5.1）`；必须先 `cd` 进 module 或传具体 `.infy` 文件。`check` 却接受目录 —— 同一 CLI 内 positional 语义不一致。
2. P7：`examples/tour` 的两个源文件按 §9.5 **不是格式化不动点**（`2 file(s) are not canonical`）。
   要么格式化器与规范样例互相矛盾（规范级），要么导览源码需要按 §9.5 重写 —— 未裁决前，任何文档不得声称"§14 导览已是规范写法"。
