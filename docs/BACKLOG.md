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
- [x] A4 给**零单测模块**补独立回归测试：`src/sema.rs`（§4 类型检查、`E-0101` 内建构造、`E-0110` 字面量取型、
      `E-0301` 的三条 `goto` 越界路径）、`src/pkg.rs`（清单/MVS/lock/dirhash）、`src/borrow.rs` 的 `E-0121`。
      理由（A3 核对结论）：阶段 A 修掉的十个缺陷里绝大多数只有"`infy check examples/tour` exit 0"这一条端到端证据，
      回归网是空的 —— 改坏 `sema.rs` 现在不会有测试变红。
      验收：`cargo test --offline --lib` 里 `sema::tests` + `pkg::tests` 合计 ≥ 20 项，且每项能单独指出它锁住的是哪条条款。
      实测：`sema` **22** + `pkg` **19** = **41 项**（≥20 ✔），另补 `borrow` 的 `E-0121` **4 项**与配套的 `parser` **2 项**；
      `cargo test --offline --lib` **332 passed / 0 failed**（基线 285/0），`cargo check --offline --lib` 0 error / 51 warning，
      `infy check examples/tour` 仍 exit 0，`(cd examples/tour && infy fmt --check)` 仍 exit 2，C 侧 `79 asserts, 79 pass, 0 FAIL`。
      每项的条款写在测试的 doc comment 里（`§4.1`/`§4.2`/`§4.3`/`§4.6`/`§2.8`/`§4.13 规则 7`/`§8.2`/`§8.4`/`§5.2`/`§5.3`/`§5.7`/`§5.8`/`§4.3+UB-006`）。
      新测试一上手就红的 4 条不是测试写错，而是真实缺陷，已修并各自带回归测试（细节见 `docs/status.md` 的 A4 一节）：
      (1) `start(chain = C, node = "a")` 整条解析失败（参数名位置只接受 `Ident`，`chain` 是 §2.4 保留字）→ `E-0800`/`E-0801` 永不触发；
      (2) `E-0120` 永不触发（§3.5 把 `x = 2` 解析成赋值**表达式**，检查却写在 `parser.rs` 无产生式的 `StmtKind::Assign` 分支里）；
      (3) §4.6 `env.x` 报假 `E-0101`、写 `env` 的 `E-0140` 永不触发（判定只写在 `check_path` 的 `::` 形式里）；
      (4) 嵌套泛型 `Arc<Mutex<u32>>` 令解析器 **panic（`usize` 减法下溢）** —— `generic_args` 的 `depth -= 2` 与递归内层冲突，改为 `closer_debts` 记账。
      两条未修只登记：`E-0043` 的未知后缀分支不可达（实际报 `E-0053`，偏差 D-18）、`E-0121` 判据低估数值绑定（偏差 D-19 / 新任务 F7）。

### 阶段 B：运行时可测（C 侧）

- [x] B1 `runtime/infy_rt.c` 的测试 harness 补齐（上一轮子任务在写 harness 时被截断）。
      验收：`gcc -std=c11 -O1 -Wall -Wextra runtime/infy_rt.c runtime/test_rt.c -o rt_tests && ./rt_tests` 全通过。
      实测：收敛后 **32303 assertions, 0 failures**，exit 0，编译 **0 error / 0 warning**（输出二进制写在 `target/scratch/`）。
      回归基线全部复跑未退化：`cargo test --offline --lib` **332 passed / 0 failed**、`cargo check --offline --lib`
      0 error / 51 warning、`infy check examples/tour` exit 0、`(cd examples/tour && infy fmt --check)` exit 2（F6 未裁决）、
      IFBC 侧 `79 asserts, 79 pass, 0 FAIL` exit 0。
      harness 自身从未编译过，修掉的测试错误：`&LOC(...)` 对已返回指针的函数再取地址（83 处）、
      把无 NUL 结尾的 `InfyString` 当 C 串比较（7 处，新增 `cstr()` helper）、§4.12 恒等式那条断言写反（56 项假失败）、
      `test_map` 提前 drop 后又迭代同一张表（use-after-free + double free = 段错误）、
      `std.fs` 用例往 mode `"w"` 的 1 字节缓冲里写 18 字节（堆越界）、VecMap 槽位断言取错偏移、
      §9.2 同一串同时被断言"存在"与"不存在"、`golden_perm`/`xxh64("")` 两个**无推导的编造常数**
      （改用独立推导值：`0xEF46DB3751D8E999` 与 `{7,9,10,1,6,3,11,8,2,5,0,4}`，同一推导复现了 PyPI 公布的两条
      `xxh64("xxhash")` 向量，且旧常数与任何 Fisher–Yates 方向都对不上）。
      测试揪出的 7 个实现缺陷已修（细节与条款见 `docs/status.md` 的 B1 一节）：cleanup drain 把 LIFO 又反一次（§3.13/§4.7.4）、
      chain 半径漏标崩溃焦点自身（§4.16）、被标 FAILED 的兄弟焦点仍留在 ready FIFO（段错误；新增 `ready_remove_locked`，
      顺带修 `infy_focus_pause` 的 `ready_tail` 截断）、`InfyMetaFixed.iter_guard_steps` 用 `uint32_t` 装不下 §10.10 的 2^32
      默认值、`InfyString` 缺结构标签导致 `infy_rt.h:212` 前置引用（= B3 第 4 项，实现里为此存在的 cast 绕过已删）、
      `{:#X}` 前缀 `0X`→`0x`、焦点 id 起点 0→1（0 保留为哨兵）。后三条属"规范未定→选一个确定实现"，已记偏差 **D-20/D-21**。
      一条未修只登记：host 调用的嵌套计数在 panic 跨越边界时不回滚 → 新任务 **F8**。
      证据提交：`b1b7ff8`（rt harness 32303/0、cargo 332/0、check 0/51、tour exit 0、IFBC 79/79 全部为该提交上的实测复跑，非引用）。
- [x] B2 `runtime/infy_ifbc.c` + `infy_dyn.c` 与 `infy_rt.c` **真实链接**跑通（此前只对着 stub 链接过）。
      验收：`gcc -std=c11 -O1 -Wall -Wextra runtime/infy_ifbc.c runtime/infy_dyn.c runtime/infy_rt.c runtime/test_ifbc.c
      -o target/scratch/ifbc_real.exe && (cd target/scratch && ./ifbc_real.exe)`
      → **0 error / 0 warning，`IFBC harness: 79 asserts, 79 pass, 0 FAIL`，exit 0**。
      根因不是工具链：把静默的 `collect2: error: ld returned 5` 用逐对象 `gcc -c` + `nm --defined-only` / `nm -u` 求差，
      真缺口只有**一个**符号——`test_ifbc.c` 拿 stub 专有的 `infy_stub_live_objs()` 当泄漏探针（116 个未决符号里其余全是
      CRT/kernel32 正常导入，且四个对象之间**无**同名重定义，§2 之前担心的那一条不成立）。
      改法：探针走冻结公共 API `infy_mem_stats().live_objs`（`infy_rt.h`；替身与生产都在这个字段里报同一份
      `infy_alloc`/`infy_free` 余额），18 处调用点等价替换，**断言一条没减、没放宽**。
      两条防"空跑"证据：一次性探针实测生产计数器是活的（`objs 0 → 1 → 0`、`allocs 0 → 1`，exit 0）；
      `nm` 显示产物含 **230 个 `T infy_` 生产符号**、stub 专有符号 **0** 个。生产 `infy_string_push` 与替身只在
      `cp == 0`（生产走 `R-0003` panic）上不同，harness 不触及该路径，所以两侧 stdout **逐字节相同**是真实一致，不是未测。
      `test_ifbc_stubs.c` 与 P6 替身链**保留**作对照，未退役。
      回归基线全部复跑未退化：`cargo test --offline --lib` **332 passed / 0 failed**、`cargo check --offline --lib`
      0 error / 51 warning、`infy check examples/tour` exit 0、`(cd examples/tour && infy fmt --check)` exit 2（F6 未裁决）、
      rt harness **32303 assertions / 0 failures**、替身链 IFBC **79 asserts / 79 pass**、`gcc -std=c11 -Wall -Wextra -c runtime/infy_rt.c` 0 warning。
      文档同步（不留旧声称）：`docs/coverage.md` 的 P11 由"探测失败 exit 1"改为实测 exit 0 并写入 `nm` 确诊手法、
      §4"被测对象是生产运行时吗"❌→✅；`docs/status.md` 与 `README.md` 里"79 断言链的是替身"的限定已解除。
      **B2 不改变** §7.8/附录 D 的 🟡：Rust↔C 双侧交叉验证仍被 **D4**（操作数个数不一致）阻塞。
      证据提交：`466e43e`（上述全部数字为该提交上的实测复跑，非引用；远端 `toolchain` 回查 = `466e43e`，推送欠账 0）。
- [x] B3 修 `infy_rt.h` 已知瑕疵：`InfyRCode` 缺 `R-0701`/`R-0713`；`R_IFBC_BOUNDARY` 与 D.10/§7.9 的映射冲突（都指 0x0709）；
      `frame_kind` 是模块级而 D.8 需要函数级；`infy_rt.h:212` typedef 前置引用告警。改完在 `docs/spec-deviations.md` 追加条目。
      验收：`gcc -std=c11 -Wall -Wextra -c runtime/infy_rt.c` 无告警。
      **B1 轮进度**：第 4 项已修——`InfyString` 补了结构标签，`typedef struct InfyString {…}`，`infy_rt.h` 的前置引用告警消失，
      `gcc -std=c11 -Wall -Wextra -c runtime/infy_rt.c` 现在 **0 warning（exit 0）**，IFBC 那条编译也从 4 告警降到 0；
      `infy_rt.c` 里为此存在的 `InfyDebugWriteFn` 绕过 cast 与它的"不能改头文件"注释一并删除。顺带修了同头的另一处布局错：
      `InfyMetaFixed.iter_guard_steps` 由 `uint32_t` 改 `uint64_t`（磁盘字段是 8 字节、§10.10 默认 2^32，读回来被截成 0 后
      `infy_art_apply_meta` 会静默换成默认值）。那三项语义留到本轮。
      **B3 轮落地的三项**（验收命令 B1 轮就绿，所以勾的是语义而不是那条命令）：
      (1) `InfyRCode` 补 `R_EMBED_TIMEOUT = 0x0701`（§9.7 spec:1495；`tickloop` 预算越线与 `timeout_ms` 越 deadline 时由
          `infy_ifbc.c` 返回，规范明言不杀进程）与 `R_FIELD_OFF_REPR = 0x0713`（spec:1627 / 附录 D.9）。这两个码此前只是
          `infy_ifbc.c` 文件内 `#define` 的裸整数，而该枚举注释自称闭集；`test_ifbc.c` 里两处写死的 `0x0701`/`0x0713`
          改成断言枚举常量（强度不变）。
      (2) **D-22 裁决**：§9.7 是码表权威 → `R-0709` 只表示"嵌入单元超出 `max_mem_bytes`"；装载期"边界"两处发现
          （tailcall 帧尺寸不兼容、`closure.new` 的 `code_idx` 越出 `func_table`，都在 `check_boundary()`）改报
          `R_IFBC_FRAME`（`R-0707` = §7.8.3 第 4 项"帧合法"，与同函数里早已存在的"call/tailcall 目标越界 → `R-0707`"同类）；
          不在规范任何码表里的 `R_IFBC_BOUNDARY = 0x070A` 从枚举与 `infy_rt.c` 诊断码表删除；**没有**借用废止号
          `R-0702`/`E-0701`（spec:1508 明令永久保留）。取证补充：D.10 的"边界"在 §7.8.3 的 1→6 里根本不存在
          （第 6 项是"行映射完备"→ `W-0705` 不阻断），而它对应的 `ret`/`ret.multi` 那条 spec:1256 已归入第 4 项，
          可见 D.10 末项是顺号编出来的、不是语义选择。`test_ifbc.c` 新增一行 `SC_CLOSURE_BAD`，IFBC harness **79 → 82**。
      (3) `frame_kind` 两级分开，并**纠正 BACKLOG 的旧描述**：函数级字段实现里早就有（`InfyIfbcFunc.frame_kind`），
          缺的是取值表与命名常量，所以没有"补字段"。现在 `infy_rt.h` 出 `INFY_FRAME_STACK` / `INFY_FRAME_COROUTINE`，
          C 侧两处裸 `== 1`/`!= 1` 换掉，函数记录 `> 1` 装载失败 `R-0703`，模块级只允许 `0`（其它值同样 `R-0703`）。
          顺带修掉一处 Rust↔C 不一致：`src/ifbc.rs::decode` 原判据是 `frame_kind > 1` 才拒，即**允许容器头写 1**，
          而 C 侧要求必须为 0 → 两侧收成同一规则，新增测试 `frame_kind_header_and_function_value_sets`
          （头 `1` 被拒 / 函数级 `1` 可装载 / 函数级 `2` 被拒）。规范只写了 D.8 的取值名字、没写数值，记偏差 **D-23**。
      回归（四条链共用一个头，全部复跑）：`gcc -std=c11 -Wall -Wextra -c runtime/infy_rt.c` **0 warning** exit 0；
      rt harness **32303 assertions / 0 failures** exit 0；替身链 IFBC **82 asserts / 82 pass** exit 0；
      **生产链** IFBC **82 / 82** exit 0、0 error / 0 warning、两份 stdout 逐字节相同；
      `cargo test --offline --lib` **333 passed / 0 failed**（基线 332，+1）；`cargo check --offline --lib` 0 error / 51 warning；
      `infy check examples/tour` exit 0；`(cd examples/tour && infy fmt --check)` 仍 exit 2（F6 未裁决，未变）。
      文档同步（不留旧声称）：`runtime/IFBC.md` §1.1/§1.5/§5/§7、`docs/internals/runtime-abi.md` §4/§9、
      `docs/spec-deviations.md`（D-22 改"已裁决"＋新增 D-23）、`docs/coverage.md`（P1/P6/P10/P11、§7.8 行、
      `infy_rt.h` 已知瑕疵行）、`docs/status.md` 与 `README.md` 的 333/82 与 B3 一节。
      **§7.8/附录 D 仍保持 🟡**：码表已对齐，但 D4 未裁；本轮另核实 D4 范围比原描述更大（`func_table` 记录布局
      Rust 36 字节/条 vs C 40 字节/条，字段集也不同），已写进 D4。头文件仍缺"模块卸载/host thunk 绑定"公共入口，
      已在 `runtime/IFBC.md` §7 明确记录，不假实现。

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
      **B3 轮另核实一处同族缺口，范围比"操作数个数"更大**：`func_table` 的记录布局两侧也不同 —— Rust `FuncEntry`
      是 36 字节/条（`name code_off code_len locals arity ret_ty sig sig_hash` 八个 u32 + `frame` u8 + `flags` u8 + u16 填充），
      C 侧按 40 字节/条读（`code_off code_len locals max_stack nargs ret_arity ret_sig name_str flags frame_kind` 十个 u32）。
      裁决 D4 时要把这两层（指令流 + 函数表）一起定，否则 D2 的双侧读法仍然对不上。
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
- [ ] F7 **`E-0121` 的判据与 §4.3/UB-006 不一致**（A4 轮补测时确认，已写 `docs/spec-deviations.md` D-19，未改实现）：
      `src/borrow.rs:460` 的条件是 `mutable && !ty.is_numeric() && !matches!(ty, Ty::Rc{..})`。
      于是 (a) `var n: u32 = 0` 跨链可达时**静默通过**（数值绑定并不比结构体绑定安全，排除它没有规范依据）；
      (b) 反过来把链内局部 `var` 当成跨链共享（§8.2 说各焦点私有栈，真正危险的是捕获与全局路径），方向偏严。
      同一函数的诊断 span 用 `b_span_default()`（`Span::empty_at(0,0)`），所以 §9.2 的 `file:line:col` 指向不到绑定处。
      验收：逐条裁决 (a)(b) 与 span，改实现或改判据，并把 `borrow::tests` 里 `E-0121` 的用例补成"数值绑定也报 / 局部 var 不报"的成对断言；结论写回 D-19。
        现状测试：`mutable_state_shared_across_chains_is_e0121`（报）+ `arc_shared_state_across_chains_is_clean` /
        `same_chain_second_focus_is_not_e0121` / `immutable_state_across_chains_is_not_e0121`（不误报）。

- [ ] F8 **host 调用的嵌套计数在 panic 跨越边界时不回滚**（B1 轮补 `runtime/test_rt.c` 时确认，未改实现）：
      `runtime/infy_rt.c:2654` 的 `static int32_t g_host_nesting` 是普通全局，`infy_host_call_event` 在
      `if (g_host_nesting >= 8) infy_panic(R_HOST_NESTING…)` 之后才 `++`、返回前 `--`；被 `§4.16` 的
      `focus`/`chain` 半径 longjmp 出去时，`--` 被跳过，于是**该焦点之后所有焦点的 host 调用都会假报 `R-0902`**。
      harness 目前只能在 catch 之后手工 `infy_test_set_host_nesting(0)` 兜住（`test_rt.c` 的 `e_call_deep` 已注明"尾部复位是死代码"）。
      验收：把深度存进 `InfyTry`（`infy_rt_internal.h`）并在两条 catch 路径（`infy_rt_catch`、`dispatch_node` 的 `setjmp`）上恢复，
      再给 `test_rt.c` 加一对断言：深度 8 的调用 panic 被捕获后，下一次正常 `infy_host_call_event` 必须成功返回而不是 `R-0902`。
      同类风险请一并核：`infy_tls_unwinding`（`drain_scope`/`scope_end_common` 里 1→0 的赋值若被 panic 打断同样悬挂）。

## 2. 当前指针

> **下一轮第一件事：取 C1**（§1 里第一个 `[ ]`：`src/hir.rs` 把 §14 tour 需要的构造落成带类型的中间表示）。
> B3 轮已把接活需要的取证做齐，直接从这里接：
> (1) `src/hir.rs` 目前是 1 行占位，`src/lib.rs:40` 已挂 `pub mod hir`，`src/lib.rs:15` 的阶段表把 §7.1 阶段 [7]
>     "IFIR + optimise"指给它 —— 也就是说 HIR 是**前端与 codegen 之间唯一的接口**，字段命名要和
>     `docs/internals/runtime-abi.md` 对齐而不是和 `src/ast.rs` 对齐：§1 标量/聚合表示、§5 链-节点-焦点、
>     §10 `env` 封闭字段集（`env.*` 写操作 = `E-0140`，`sema` 已有测试锁住）；验收要求的"尺寸/对齐金样"就抄这三节。
> (2) 可复用的现成件（别重造）：`src/sema.rs` 的 `Ty`（§4 类型）、`runtime/infy_rt.h` 的 `InfyMetaFixed`
>     （§7.4 磁盘 meta 布局，B1 轮刚把 `iter_guard_steps` 修成 64 位）、`InfyChainDesc` / `InfyNodeLoc`
>     （§8 链表与诊断定位）。tour 的两个文件是 `examples/tour/core/build/mod.infy` 与 `examples/tour/cmd/runner/main.infy`，
>     需要的构造 = fn/event/struct/enum/const/链节点，`infy check` 已经在它们上面 exit 0，所以 AST 侧不缺信息。
> (3) **不要碰 D4 的两侧布局**：C1 只落 Rust 侧表示。B3 轮核实 D4 比原描述大 —— 除了操作数个数，
>     `func_table` 记录 Rust 36 字节/条 vs C 40 字节/条也不同；那两条一起留给 D2/D4 裁决（`infy run` 之前不必动）。
> 输出二进制一律写 `target/scratch/`（写 `/tmp` 会让 `ld` 直接返回 5）；改 `infy_rt.h` 后**四条链**都要复跑
> （P6 替身 IFBC、P11 生产 IFBC、P9 rt、`-c infy_rt.c`），因为它们共用一个头。
> **B3 手法留档**：码表冲突的裁决顺序 = §9.7 码表 > 附录 D 的顺号表；附录 D.10 的"边界"在 §7.8.3 的 1→6 里并不存在
> （第 6 项是行映射完备 → `W-0705`），这类"表末项是顺号编出来的"痕迹可以直接作为让路的依据；而废止号（`R-0702`/`E-0701`，
> spec:1508）不能当逃生舱用。改枚举时记得 `infy_rt.c` 的 `r_table` 是第二份真相（诊断标题表），删码要同步删行。
>
> **B2 已完成，手法留档**：静默的 `collect2: error: ld returned 5` 用逐对象 `gcc -c` + `nm --defined-only`/`nm -u` +
> `comm -23` 一减就露出真缺口（本轮唯一缺口是 harness 私用的 `infy_stub_live_objs`）。`nm -u -P` 在本机只输出单列 `U`，别用它解析。
>
> **推送欠账**：只认现算 `git rev-list --count <远端SHA>..HEAD`；远端 SHA 用
> `gh api repos/YuanShenNiuBi1314/infinity-lang/git/refs/heads/toolchain --jq .object.sha` 回查，`Everything up-to-date` 不算成功。
>
> 已验证事实（B3 轮全部复跑，非引用）：`cargo test --offline --lib` **333 passed / 0 failed**
> （`lexer` 79 · `derive` 57 · `fmt` 39 · `macro_expand` 35 · `sema` 22 · `pkg` 19 · `toml` 15 · `borrow` 15 · `sha256` 12 ·
> `codes` 9 · `base64` 7 · `parser` 7 · `ifbc` 6 · `cli` 5 · `stdlib` 4 · `diag` 2 = 333）；
> `cargo check --offline --lib` 0 error / 51 warning；`infy check examples/tour` exit 0；
> `(cd examples/tour && infy fmt --check)` **exit 2（2 file(s) are not canonical，见 F6）**；
> **rt harness `gcc -std=c11 -O1 -Wall -Wextra runtime/infy_rt.c runtime/test_rt.c` → 0 error / 0 warning，
> `32303 assertions, 0 failures` exit 0**；
> `gcc -std=c11 -Wall -Wextra -c runtime/infy_rt.c` → **0 warning**（B3 已勾，三项语义同轮落地）；
> **IFBC 两条链都绿且 stdout 逐字节相同**：替身链 `gcc … infy_ifbc.c infy_dyn.c test_ifbc.c test_ifbc_stubs.c` →
> `82 asserts, 82 pass, 0 FAIL` exit 0；**生产链** `gcc … infy_ifbc.c infy_dyn.c infy_rt.c test_ifbc.c` → 同为 82/82、0 告警。
> A4 修掉的 4 个前端缺陷与 B1 修掉的 7 个运行时缺陷都有回归测试；未修只登记：D-18、D-19/F7、F8（host 嵌套计数跨 panic 不回滚）；
> D-22 与 `frame_kind` 取值表（D-23）已于 B3 裁决完毕。
> `main` 与本地分叉（另一会话推了 `faa34ad`/`05da606`），**合并主干由人决定**。
> 优先级最高的已知地雷：**D4**（Rust/C 的 IFBC 操作数个数 + `func_table` 记录布局都不一致，D2 之前必须先裁）、
> **D1**（`src/btrack.rs` 端到端）、**F5/F6**（CLI 目录参数不一致、§14 源码不是格式化不动点）、**F7**、**F8**。
> **编译 C 侧测试时四个 .c 必须一起给**（少 `infy_dyn.c` 时 `ld` 只返回 `collect2: error: ld returned 5`，不打印任何 undefined reference）。
