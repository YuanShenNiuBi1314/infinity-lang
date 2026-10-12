# INFY v2.0 生产总纲（BACKLOG）

本文件是**唯一的任务真值源**，给人看也给定时任务读。它模仿 R 项目那套生产方式：

- 每一轮只做**一个自足的小增量**，且必须带着测试与可复现的验收命令；
- 贡献者留下比来时更绿的树——**绝不允许把树弄坏后离开**；
- 一切主张以命令输出为准，不写没有证据的进度（R 的 `R CMD check` 文化：没跑过 = 没做）；
- 失败是一等公民：踩坑写进经验库（见 §0.4），下一轮先读它，别在同一个坑里摔两次；
- **发布与合并主干永远由人签字**，自动化只推自己的分支。

## 0. 每一轮的固定协议（定时任务必须遵守）

1. **读**：`docs/BACKLOG.md`（本文件）、`docs/RUNLOG.md` 末尾 20 行、经验库
   `C:\Users\zhang\.qoder-cn\projects\F--infinity---INFY-DELIVERY-v0-1-0\memory\MEMORY.md` 及其索引到的条目。
2. **取活**：执行 §1 里**第一个未打勾**（`[ ]`）的任务，即"最近的地方"。不跳号、不挑肥拣瘦；
   若该任务被外部条件卡住（例如依赖的文件正被别的会话写），在 `docs/RUNLOG.md` 记一行原因，
   顺延取下一个任务，本轮结束时把卡住的任務留在 `[ ]` 状态。
3. **做**：改动 + 测试 + 跑该项的验收命令。命令输出与预期不符 = 未完成，不许勾 `[x]`。
4. **记**：向 `docs/RUNLOG.md` 追加一行
   `| 轮次 | 时间 | 做了什么 | 关键命令 | 结果(数字/exit) | 下一步 |`。
5. **沉淀**：遇到**重大失败**（把树弄坏、连续两轮无进展、发现规范级缺陷、破坏性数据风险）或
   **高价值失败**（一次坑解释了以后一类坑）时，写进经验库：新建/更新
   `…/memory/<kebab-case>.md`（front-matter 用 `type: project` 或 `feedback`），并在
   `…/memory/MEMORY.md` 加一条 ≤150 字符的索引行。**只写索引不写文件，或只写文件不更新索引，都算没做**。
   写之前先 grep 索引，能更新旧条目就别开新条目。
6. **交**：`git add -A && git commit`（一个增量一个提交，信息写清"改了什么/证据是什么"），
   `git push origin HEAD:toolchain`。**不动 `main`，不 rebase 别人的历史，不强推，不打 tag，不发布。**
   `github.com:443` 经常连不上（`api.github.com` 通），推送失败不重试轰炸：记进 RUNLOG，下一轮再推。
7. **禁**：向人提问（无人值守）、加第三方依赖（crates.io 被策略拦，工具链是 std-only 硬约束）、
   删除或覆写 `src/` 下别人正在写的文件、伪造测试结果、为过测试而放宽断言。

## 1. 任务清单（自上而下执行，`[x]` 必须附证据）

### 阶段 A：止血 —— 让树可信

- [x] A1 主树 `cargo check --offline --lib` 零错误（提交 `725042a`：补 `ifbc.rs` 截断的 `mod tests` 括号、修 builder 移动后取 `marks`）
- [x] A2 `cargo test --offline --lib` 全绿。实测 **285 passed / 0 failed**（提交 `816ae92`；`cargo check --offline --lib` 0 error、51 warning；`infy check examples/tour` exit 0）。
      实际修掉的根因（每条有回归测试，细节见 `docs/status.md` 与 `docs/spec-deviations.md` D-13…D-16）：
      属性路径/参数名不接受保留字（`#[link(…, static = true)]` 整条被丢）、`impl Trait for Type` 丢掉 `Type`、
      `.` 后不接受保留字成员（`.link()`）、`macro_rules!` 首臂吞掉整条记号流（曾把宏打印成 `v {}` 假成功）、
      列表凭空双 `HardLine`（缩进空行破坏 rule 12）、struct/enum/match 列表丢逗号、注释归属由单调游标改为按字节区间、
      `@embed` 闭包前多空行与参数被拆行、`where` 粘连、`for` 多加括号、链续行绝对 8 列、RI 旗帜宽度、`is.dyn T` 反汇编形态。
      附带发现（未修，见 D4）：Rust/C 的 IFBC **操作数个数**不一致。
- [x] A3 把 `docs/status.md` 与 `README.md` 的覆盖表更新成与 A2 实测一致（不许留旧数字）。
      主数字（285 passed / 0 failed、check 0 error/51 warning、tour exit 0）与**非计数断言**都已对回证据：本轮把
      `cargo test --offline --lib` 重新分组计数（`lexer` 79 · `derive` 57 · `fmt` 39 · `macro_expand` 35 · `toml` 15 ·
      `sha256` 12 · `borrow` 11 · `codes` 9 · `base64` 7 · `parser` 5 · `ifbc` 5 · `cli` 5 · `stdlib` 4 · `diag` 2 = 285），
      C 侧用 `gcc -std=c11 -O1 -Wall -Wextra runtime/infy_ifbc.c runtime/infy_dyn.c runtime/test_ifbc.c runtime/test_ifbc_stubs.c`
      跑出 `IFBC harness: 79 asserts, 79 pass, 0 FAIL`（exit 0），CLI 退出码逐条实测（check 0 / build 6 / run·test·doc·ifbc·conformance 1 /
      fmt --check 脏时 2 / artifact info 非产物 7 / init 0）。
      核对出的**文档谎报**已改正：§11 `HostApi`（`src/host.rs` 是 1 行占位）此前被标 ✅；§7.4/§7.3 产物、§7.8/附录 D IFBC、
      §9.2 金样、§9.4 `--diag-json`、§10、§4、§5、§3"全产生式"、运行时均降级为 🟡/⬜ 并写明证据类别；
      README 目录树里不存在的 `tests/` 已删；README 指向的 `docs/coverage.md` 此前**根本不存在**，现已建成逐条款"实现 × 证据"矩阵。
      遗留：A3 期间发现的两条真实缺陷登记为 F5/F6，零单测模块的补测登记为 A4。
      证据提交：`4244458`（README/status/coverage/BACKLOG/D-17），RUNLOG 行在紧随其后的提交里。
- [ ] A4 给**零单测模块**补独立回归测试：`src/sema.rs`（§4 类型检查、`E-0101` 内建构造、`E-0110` 字面量取型、
      `E-0301` 的三条 `goto` 越界路径）、`src/pkg.rs`（清单/MVS/lock/dirhash）、`src/borrow.rs` 的 `E-0121`。
      理由（A3 核对结论）：阶段 A 修掉的十个缺陷里绝大多数只有"`infy check examples/tour` exit 0"这一条端到端证据，
      回归网是空的 —— 改坏 `sema.rs` 现在不会有测试变红。
      验收：`cargo test --offline --lib` 里 `sema::tests` + `pkg::tests` 合计 ≥ 20 项，且每项能单独指出它锁住的是哪条条款。

### 阶段 B：运行时可测（C 侧）

- [ ] B1 `runtime/infy_rt.c` 的测试 harness 补齐（上一轮子任务在写 harness 时被截断）。
      验收：`gcc -std=c11 -O1 -Wall -Wextra runtime/infy_rt.c runtime/test_rt.c -o rt_tests && ./rt_tests` 全通过。
- [ ] B2 `runtime/infy_ifbc.c` + `infy_dyn.c` 与 `infy_rt.c` **真实链接**跑通（此前只对着 stub 链接过）。
      验收：`gcc -std=c11 -O1 -Wall -Wextra runtime/infy_ifbc.c runtime/infy_dyn.c runtime/infy_rt.c runtime/test_ifbc.c -o ifbc_tests && ./ifbc_tests`。
- [ ] B3 修 `infy_rt.h` 已知瑕疵：`InfyRCode` 缺 `R-0701`/`R-0713`；`R_IFBC_BOUNDARY` 与 D.10/§7.9 的映射冲突（都指 0x0709）；
      `frame_kind` 是模块级而 D.8 需要函数级；`infy_rt.h:212` typedef 前置引用告警。改完在 `docs/spec-deviations.md` 追加条目。
      验收：`gcc -std=c11 -Wall -Wextra -c runtime/infy_rt.c` 无告警。

### 阶段 C：把 tour 从 check 推到 build/run（P6 单文件承诺的落地）

- [ ] C1 `src/hir.rs`：把 §14 tour 需要的构造（fn/event/struct/enum/const/链节点）落成带类型的中间表示，字段与 `docs/internals/runtime-abi.md` 一一对应。
      验收：`cargo test --offline --lib hir` 通过，且有金样测试证明尺寸/对齐与 ABI 文档一致。
- [ ] C2 `src/codegen.rs`：INFY → C11 发射，覆盖 tour 子集（原语/`str`/`String`/`Vec`/`Option`/`Result`/struct/enum/链/`?`/`match`/`fmt`），检查型算术与边界检查永不关闭（§7.2）。
      验收：`infy build` 在 `examples/tour` 上生成 `.c` 且 `gcc` 能编译它（可先只验到编译，不要求运行）。
- [ ] C3 `src/artifact.rs`：`INFYART2` 段尾 + `.inf meta/ifbc/sum` 段、`build-id`（§7.3 规范化输入 SHA-256）。
      验收：`infy artifact info` 能读回自己刚产出的产物；同一输入两次构建字节相同（`--verify-reproducible`）。
- [ ] C4 `infy run examples/tour` 真跑出输出；退出码遵循 §13.4。
      验收：`infy run` 的 stdout 与 RUNLOG 里记录的期望一致（把期望字符串写进测试）。
- [ ] C5 `infy test`：编 `#[test]` 函数 + 执行 + 退出码（§5.9/§13.4）。

### 阶段 D：双轨嵌入的真实端到端

- [ ] D1 `src/btrack.rs` 收敛（子任务被截断）：CPython 探测 → 能力扫描 → lowering → IFBC。
      验收：`@py` 的 tour 片段产出可被 `infy_ifbc.c` 装载的模块；`eval` 触发 `E-0711`；未映射 opcode 触发 `E-0712` 并点名具体项；声明 `py = "3.13"` 而本机是 3.14 触发 `E-0713`。
- [ ] D2 `infy ifbc dump|verify` 接线（Rust 侧编码 ↔ C 侧虚拟机**交叉验证**：同一镜像两边读出的反汇编一致）。
- [ ] D3 `src/atrack.rs`：`#[link]` + `@c` 配对、`infy_binding.h` 生成、`infy_e_<name>_<8hex>` 包装、缺 `unsafe` 报 `E-0721`、缺 `#[owns]/#[borrows]` 报 `E-0723`。
      验收：一个最小 `@c` 单元被 gcc 编译、链接、在产物里被调通。
- [ ] D4 **Rust 编码 ↔ C 虚拟机操作数个数对齐**（A2 本轮静态比对发现，未修；D2 的交叉验证必然踩到）：
      `push` Rust 2 个操作数（池索引 + 类型，`DEV-IFBC-13`）vs `runtime/infy_ifbc.c` 的 `OPS[OP_PUSH].nops = 1`；
      `box.dyn` Rust 1 vs C 0（附录 D.7 与 `runtime/IFBC.md` 都写 0 操作数，Rust 侧不符规范）；
      `guard.nonzero`/`guard.shift` Rust 1 vs C 0；`ldw` Rust 1 vs C 2（`/* ldw i, ty */`）。
      字节宽度 = `1 + 4*nops`，任一条不一致都让两边对同一镜像的指令流错位。
      验收：以附录 D 为唯一权威逐条判定哪侧改（`push k` 的类型操作数若保留，必须同时进 D.2 与 C 表），
      再让 D2 的双侧反汇编逐指令相等测试通过；结论与选择写进 `docs/spec-deviations.md`。

### 阶段 E：§12 合规套件（这是唯一能声称"兼容"的东西）

- [ ] E1 `conformance/cases/**`：按 §12.1/§12.2/§12.5 的分类建目录与用例文件格式（源文件 + 期望码），先从已有的单元测试里"搬运"成外部用例。
- [ ] E2 `infy conformance --report`：**逐分类通过率**（只报总数视为不合规，§12.5）。
- [ ] E3 把通过率表落进 `docs/coverage.md`，并在 README 明确"距 100% 还差哪些类"。**在 100% 之前，任何文档不得出现"INFY v2.0 兼容"字样。**

### 阶段 F：工程卫生（每轮顺手做一点，不许它长大）

- [ ] F1 `.github/workflows/ci.yml`：`cargo fmt --check`? 不适用（无 rustfmt 约定）→ 只做 `cargo build --offline --release` + `cargo test --offline` + `gcc` 跑 `runtime` 测试。
- [ ] F2 消 40 条编译告警（unused/dead code），每轮清一批，`cargo check` 输出为 0 warning 时勾掉。
- [ ] F3 把 `push*.log`/`rt_tests`/`ifbc_tests` 等一次性产物加进 `.gitignore`，仓库里只留 `docs/RUNLOG.md` 这一份审计线。
- [ ] F4 `infy lsp`、`std.net` TLS、`dyn Trait` vtable 偏移表：要么实现，要么在 `docs/spec-deviations.md` 里保持"明确未实现"的记录，不许静默。
- [ ] F5 `infy fmt` / `infy expand` 的目录参数与 `infy check` 不一致（A3 实测）：
      `infy fmt --check examples/tour` → `error: 无法读取 examples/tour: 拒绝访问。 (os error 5)`（把目录当文件读），
      `infy expand --stage macro examples/tour` → `error: 找不到 module 根（§5.1）`；只有 `cd` 进 module 或直接给 `.infy` 文件才可用。
      验收：三条命令（check/fmt/expand 各带目录参数）行为一致，且加进 `src/cli.rs` 的测试。
- [ ] F6 **§14 导览源码不是 §9.5 的格式化不动点**（A3 实测）：`(cd examples/tour && infy fmt --check)` →
      `2 file(s) are not canonical`，exit 2。必须先判定是"§9.5 与 §14 互相矛盾（规范级缺陷，写 `docs/spec-deviations.md`）"
      还是"格式化器与 §14 样例之一写错了"（改实现/改样例）。裁决前，任何文档不得声称"§14 导览已是规范写法"。
      验收：`infy fmt --check examples/tour` exit 0，或偏差条目里给出规范方认可的逐条理由。

## 2. 当前指针

> **下一轮第一件事：补推。** 远端 `toolchain` 停在 `4429b25`；本地领先的提交数用 `git rev-list --count 4429b25..HEAD` 现算
> （**只认现算**：`git rev-list --count 4429b25..HEAD`，写下这行的提交本身也会让它 +1，所以不引用固定数；区间起点是远端 SHA）。
> cron-01、cron-02 各两次 `git push` 都失败在 `github.com:443`；`api.github.com` 能回查远端 SHA，但推不动（不要只看 push 的输出，回查为准）。
>
> **补推之后取 A4**（§1 里第一个 `[ ]`：给 `sema.rs`/`pkg.rs`/`E-0121` 补独立回归测试 —— A3 核对出的事实是这三个模块
> 目前**一项单测都没有**，阶段 A 修掉的十个缺陷大多只有 tour 那一条端到端证据）。
> 已验证事实（A3 轮全部复跑，非引用）：`cargo test --offline --lib` **285 passed / 0 failed**；
> `cargo check --offline --lib` 0 error / 51 warning；`infy check examples/tour` exit 0；`infy build examples/tour` exit 6；
> `infy run|test|doc|ifbc|conformance|lsp` exit 1；`(cd examples/tour && infy fmt --check)` **exit 2（2 file(s) are not canonical，见 F6）**；
> `infy artifact info <非产物>` exit 7；C 侧 `IFBC harness: 79 asserts, 79 pass, 0 FAIL`（链的是 `test_ifbc_stubs.c`，不是生产 `infy_rt.c`）；
> 诊断码 210 全表；规范 §12 要求 2300 用例（1244+256+800），外部用例目前为 0。
> `main` 与本地分叉（另一会话推了 `faa34ad`/`05da606`），**合并主干由人决定**。
> 阶段 A 之后优先级最高的已知地雷：**D4**（Rust/C 的 IFBC 操作数个数不一致）与 **B1**（C 侧 harness 未收敛）；
> **编译 C 侧测试时四个 .c 必须一起给**（少 `infy_dyn.c` 时 `ld` 只返回 `collect2: error: ld returned 5`，不打印任何 undefined reference）。
