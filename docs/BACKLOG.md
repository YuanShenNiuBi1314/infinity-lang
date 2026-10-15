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
      证据提交：`c63187e`（上述 333/0、82/82、32303/0、`-c` 0 告警、tour exit 0、`fmt --check` exit 2 全部为该提交上的
      实测复跑，非引用；该提交尚未推上去，见 §2 推送欠账与 RUNLOG `cron-06-push` 行）。

### 阶段 C：把 tour 从 check 推到 build/run（P6 单文件承诺的落地）

- [x] C1 `src/hir.rs`：把 §14 tour 需要的构造（fn/event/struct/enum/const/链节点）落成带类型的中间表示，字段与 `docs/internals/runtime-abi.md` 一一对应。
      验收：`cargo test --offline --lib hir` 通过，且有金样测试证明尺寸/对齐与 ABI 文档一致。
      实测：`hir` **20 项全绿**，`cargo test --offline --lib` **354 passed / 0 failed**（基线 333，+20 `hir` +1 `parser`），
      `cargo check --offline --lib` 0 error / 51 warning，`infy check examples/tour` exit 0。
      **金样不是抄文档，是抄 `gcc`**：一次性探针打 `sizeof`/`_Alignof`/`offsetof`（`target/scratch/c1/probe.c`，
      实测数字摘录记在 `docs/RUNLOG.md` cron-07），Rust 侧 `record_layout`/`union_layout` 必须复现它。覆盖的表示：
      §1 逐行（`[u32;4]` 16/4、`Rc/Arc` 16/8、`dyn`/闭包 16/8、`Option` 的指针 niche 只认 `&T`/`Box`/`*const`/fn 指针）、
      `struct`/`enum` 的每字段偏移、链帧（`let`/`var` 声明序 + `node_idx` + 尾部补齐）、§8.4 节点表
      （tour `Pipeline` = **6 个节点**，行 8/9/10/11/16/19，`gate`/`retry_once` 各挂 1 个连携块，锚点 `begin`=0/`done`=5
      —— 这是"连携块与 `label` 都不开新节点"第一次被钉成可复跑断言）、§4.6/§10 `EnvOffsets` 11 个偏移 + 72/8、
      §2 符号 mangle（类型的 package 路径改从 `Program::by_path` 反查，不再是 module 路径）、§1 vtable 前缀 `24 + 8·n`、
      `InfyNodeLoc` 24/8、`InfyChainDesc` 48/8、`InfyFocusId3` 12/4。
      三条 **ABI 文档自身的错处**已改正并写回文档：`Result<u32,Fail>` 实测 **20**、`Result<u32,u64>` 实测 **24**
      （原文的 `sizeof(T)+sizeof(E)+1, aligned` 两处都会算成 16）；§5 的 `ChainFrame` 从伪码改成可算规则；
      §10 补出 `EnvSnapshot` 逐字段偏移表（§4.6 写 `&str`，冻结头存的是**内联 `InfyStr`**，不是指针）。
      规范未定 → 选定实现：偏差 **D-24**（元组 = 声明序 C struct；无 payload 的 enum 省掉 union → `Phase` 4/4；
      判别式只接受整数字面量，其它形态 `E-0053`）与 **D-25**（§4.1"判别式最小容纳" vs ABI 的 `uint32_t tag` →
      裁为恒 `u32`，理由含 `.infib` ABI 稳定性与 `repr(C)` 须与 C 的 `int` 共享布局；推论 = 超宽判别式报 `E-0042`，
      不截断，锁定点 `hir::tests::a_discriminant_wider_than_the_tag_is_e0042`）。
      测试揪出并修掉 **1 个真实缺陷**：`enum Code { A = 3 }` 的显式判别式被解析器**静默丢弃且不打任何诊断**
      （`variant()` 只匹配 `ExprKind::Int`，而 `primary()` 首个 `Kind::Lit(l)` 臂吃掉所有字面量 → 该形状从不产生）。
      锁定点 = `parser::tests::enum_discriminants_reach_the_ast_and_bogus_ones_are_e0053` + `hir` 的两个判别式测试。
      **明确未做**：语句/表达式级 IFIR（函数体仍是 `ast::Block`），`infy build` 也没有接 HIR（仍 exit 6）—— 属 C2 前置。
      同族未修只登记为 **F9/F10/F11**。
      证据提交：`44c7b54`（上述 354/0、20 项 `hir`、0/51、tour exit 0、`-c` 0 告警、rt 32303/0、两条 IFBC 链 82/82
      全部为该提交上的实测复跑，非引用；远端 `toolchain` 回查 = `14654c9`，推送欠账 **0**）。
- [x] C2 `src/codegen.rs`：INFY → C11 发射，覆盖 tour 子集（原语/`str`/`String`/`Vec`/`Option`/`Result`/struct/enum/链/`?`/`match`/`fmt`），检查型算术与边界检查永不关闭（§7.2）。
      验收：`infy build` 在 `examples/tour` 上生成 `.c` 且 `gcc` 能编译它（可先只验到编译，不要求运行）。
      实测：`infy build examples/tour` → `check ok — 2 file(s), 0 error(s)` + `codegen ok — 12473 byte(s) of C11 written to
      examples/tour\.infy-build\x86_64-pc-windows-gnu\ci.c (stages [7]-[8]; 1 chain(s), 6 node(s))`，exit **6**
      （阶段 [9] 链接未实现，属 C3；不再写"codegen 未实现"）。
      验收命令 `gcc -std=c11 -Wall -Wextra -I runtime -c <生成的 .c>` → **exit 0，0 error / 0 warning**，产出 5723 字节对象文件。
      `cargo test --offline --lib` **372 passed / 0 failed**（基线 354，+18 项 `codegen`），`cargo check --offline --lib`
      0 error / **51 warning**（与基线同数：本轮新增的 4 条已清）、`infy check examples/tour` 仍 exit 0、
      `(cd examples/tour && infy fmt --check)` 仍 exit 2（F6 未裁决）。
      **改了冻结头 `runtime/infy_rt.h`**：`infy_chain_register` / `infy_chain_register_table` / `infy_chain_desc` /
      `infy_chain_count` 四个入口在 `infy_rt.c:1893-1906` 早已定义却从未进头（codegen 是头的甲方），补声明后
      **四条 C 链全部复跑**：`gcc -c infy_rt.c` 0 告警 exit 0；rt harness **32303 assertions / 0 failures** exit 0（0 告警）；
      P6 替身链 IFBC **82 / 82** exit 0；P11 生产链 IFBC **82 / 82** exit 0，且与替身链 stdout **逐字节相同**（`diff -q` 无输出）。
      发射面（每项都有单测指名条款，见 `src/codegen.rs` 的 `mod tests`）：ABI §1 全部表示 + **布局自检**
      （生成的 C 自带 `sizeof`/`_Alignof`/`offsetof` 负数组断言，尺寸取自 `HirCrate`，所以 gcc 一编译就等于把
      `Phase` 4/4、`FailWhy` 8/4、`Fail` 12/4、`Job` 12/4、`Result<u32,Fail>` **20**/4、`Pipeline` 帧 4/4 且
      `node_idx@0` 全部复核一遍）；§8.4 节点表 6 行（8/9/10/11/16/19，节点 3/4 各 1 连携块）；
      §8.2 `goto` 编译期解析（`done`→5、`+1`→4、`begin`→0）；§4.9/§7.2 检查型算术走 `infy_add_u32` 而非裸 `+`、
      数组下标走 `R-0004` 检查、`unwrap` 走 `R-0005`；§4.11 `?` = 测 `is_ok` + 搬运 `err` + 取 `ok`；
      §3.12 `match`（标签判定、或模式析取、载荷按 `a0` 绑定、穷尽性之外的 panic）；§4.7.4 push/pop 与
      `infy_drop_String`/`infy_drop_Vec_u8_`（元素大小随容器走，§10.13）；§4.13 规则 2 的无后缀字面量取上下文型；
      §2.8 字面量写成 `UINT32_C/INT64_C/…`；§5.3 调用按**调用方 package 优先**解析（tour 的 `gate` 在两个 package
      同名，按简单名全局匹配会静默选错，已修并有测试）。
      规范未定 → 选定实现并记偏差 **D-26**（单 TU）、**D-27**（§8.4"序言"归属歧义 → 节点末尾执行）、
      **D-28**（`match` 不可达分支发 `R_BAD_BOOL` panic）、**D-29**（含 NUL 的 `str` 字面量拒绝）、
      **D-30**（`overflow=saturate` 与窄位移没有检查入口 → 拒绝）、**D-31**（owned 值的三种移动形式拒绝）。
      **明确未做**（不用假成功掩盖）：阶段 [9] 链接与 §7.4 产物容器（C3）、`infy run`（C4）、`#[test]` 执行（C5）、
      闭包 / `dyn` / `@embed` / A 轨 `extern` 调用 / `event` 调用 / `for`·`defer`·`const_assert` / 128 位算术 /
      `Vec`·`String` 的构造入口 / 字节串字面量 —— 全部是**指名条款的 CgError**；
      `fmt!` 在阶段 [2]/[4] 根本没有 lowering 路径（`sema` 对未展开宏发 `E-0402`），所以 C2 的 `fmt` 覆盖
      被上游阻塞，登记为 **F15**。
      证据提交：**`2b0fcdb`**（`feat(C2)`）—— 上述 372/0、0/51、12473 字节 C11、gcc exit 0（0 error / 0 warning）、
      四条 C 链（32303/0、82/82 两条、`-c infy_rt.c` 0 告警）全部为该提交上的实测复跑，非引用；RUNLOG 的 cron-08 行是同轮的审计行。
      **该提交尚未推上去**：本轮三次推送全败（三种失败形态），见 §2 推送状态与 cron-08-push 行。
- [x] C3 `src/artifact.rs`：`INFYART2` 段尾 + `.inf meta/ifbc/sum` 段、`build-id`（§7.3 规范化输入 SHA-256）。
      验收：`infy artifact info` 能读回自己刚产出的产物；同一输入两次构建字节相同（`--verify-reproducible`）。
      **两条都实测通过**：`infy build examples/tour` → `artifact ok — 199975 byte(s)`（宿主镜像 199173 + 段 blob 738 + 段尾 64，6 段）、
      `build-id 4d8250b58122ca2b76132029955e2d2005156dc0a6b021bf5f4ae761e5fd55ba`、exit **0**（不再是 6）；
      `infy artifact info …ci.exe --nodes` → exit **0**，`.inf meta` 逐字段读回，节点表 6 行 = 行 8/9/10/11/16/19
      （`gate`/`retry_once` 各 1 连携块，锚点 begin=0·done=5）—— **与 C1 轮的 §8.4 金样逐格相同**；
      `infy build examples/tour --verify-reproducible` → `两个独立构建目录产物字节相同（199975 字节）`，exit **0**。
      落地面：`src/artifact.rs`（新建，1176 行）写入器 + 读取器 + `build_id`；`cli.rs` 的 `link_plan`/`stage9`/
      `compute_build_id`/`package_dirhashes`，`--cc`/`$CC`（缺 → `E-0602` + exit 6，实测 `--cc /no/such/cc`），
      `--runtime`，非产物 → `E-0605` + exit **7**（实测 `infy artifact info README.md`）。
      容器布局**逐字节镜像 C 侧**（`infy_art_parse_trailer`/`parse_meta_record`/`meta_resolve_strings`），并有
      `artifact::tests::meta_offsets_and_sizes_match_the_frozen_header` 把 `INFY_META_OFF_*`/段尾大小/两个魔数
      对回 `runtime/infy_rt_internal.h` 与 `infy_rt.h` 的**文本**（改头不改这里 = 测试变红）。
      `cargo test --offline --lib` **391 passed / 0 failed**（基线 372，+18 `artifact` +1 `cli`），
      `cargo check --offline --lib` 0 error / **51 warning**（未新增），`infy check examples/tour` exit 0，
      `gcc -std=c11 -Wall -Wextra -I runtime -c 生成的 ci.c` exit 0（0/0，5723 字节 .o），
      `(cd examples/tour && infy fmt --check)` 仍 exit **2**（F6 未裁决，未变）。
      **Rust 写 ↔ C 读 的常驻锁点**：`runtime/fixtures/artifact_rust_writer.bin`（730 字节，写入器逐字节固定输出，
      只有 `INFY_REGENERATE_FIXTURE=1` 才重写）由 `runtime/test_rt.c` 用**生产**读取器 `infy_art_read_file` 读回并断言
      同一组字段（+27 项）；一次性探针另把**真实 tour 产物**交给同一读取器 → `rc=0 blob_len=738`，
      `infy_art_dump_meta` 每字段与 Rust 侧一致。四条 C 链全复跑：`-c infy_rt.c` **0 warning** exit 0；
      rt harness **32335 assertions / 0 failures**（基线 32303，+32）exit 0；P6 替身链 IFBC **82/82**；
      P11 生产链 IFBC **82/82**，与替身链 stdout 逐字节相同（`cmp` 无输出）。
      规范未定 → 选定实现：偏差 **D-32**（五张表的记录布局、`opt_level`/`lto` 值表、profile 名反推、入口描述与
      `ResourceKeys` 借用 `export` 区/推导规则、`artifact info` 借 `E-0605`/exit 7）。
      **发现并裁掉一条规范级缺陷 D-33**：§7.4/ABI §6 的"段尾缺失/重复即非产物"若按字面整文件扫魔数，会因为
      `.inf rt` 静态链进镜像、而 `infy_rt.c` 自身带 `"INFYART2"` 字面量，**否掉每一个真产物** —— 本轮第一次真接
      阶段 [9] 就撞上（`host image already contains the INFYART2 magic`）。两侧同规则收窄到追加区 `[blob_off, EOF)`，
      写入器另拒绝 blob 自带魔数；顺带修掉 `infy_art_parse_trailer` 的 `if (t) {…}` 后无条件 `t->hdr_ver` NULL 解引用。
      `docs/internals/runtime-abi.md` §6/§7 的草图（`blob_off u64` 等）与实现不符，已按实现改正并补全逐字段表。
      **明确未做**（不用假成功掩盖）：阶段 [10] 的 sumdb 校验/运行时自校验比对（`.inf sum` 只抄 `lock.dirh`，
      tour 无依赖 = 0 条）；`ResourceKeys`/`EnvOffsets` 区写空集（前端不收集 `std.chain.lock` 键名）；
      `.inf ifbc` 恒空（D1）；A 轨 `#[link]` 目标码、§7.2 LTO 编排、`infyl` 独立链接器驱动；
      C 侧 `infy_art_dump_meta` 里 `meta off=0 len=0` 是**写死的显示值**（打印错，不是容器错）—— 本轮未改，见 §2。
      证据提交：**`75bf27a`**（`feat(C3)`）—— 上述 391/0、0/51、199975 字节产物、blob 738、exit 0、
      `--verify-reproducible` 两次相同、32335/0、82/82 两条链、`-c infy_rt.c` 0 告警、tour exit 0、
      `fmt --check` exit 2 全部为该提交上的实测复跑，非引用；RUNLOG 的 cron-09 行是同一轮的做账行。
      **该提交已推上 `toolchain`**（本轮第 6、7 次尝试成功：`a6ac175..ef23a75`、`ef23a75..a0db263`，
      `gh api` 回查远端 = 本地 HEAD；前 5 次全败，见 §2 推送状态与 cron-09-push/-push2/-push3 三行）。
      同轮登记 **F16**（阶段 [10] 无人比对 `.inf sum`）、**F17**（`ResourceKeys`/`EnvOffsets` 在容器里是空集，
      而 `hir` 早就算出 11 个偏移）、**F18**（C 侧 `infy_art_dump_meta` 把 meta 段行写成死值 `off=0 len=0`）。
- [x] C4 `infy run examples/tour` 真跑出输出；退出码遵循 §13.4。
      验收：`infy run` 的 stdout 与 RUNLOG 里记录的期望一致（把期望字符串写进测试）。
      实测：`infy run examples/tour` → **exit 0**，程序 **stdout 0 字节 / stderr 0 字节**（逐字节转发），
      `infy run` 自己的状态行在 stderr（`run ok — 程序退出码 0（stdout 0 字节 / stderr 0 字节，§13.4）`），
      阶段 [1]-[9] 的构建信息仍按 `infy build` 的原样在 stdout（`artifact ok — 199975 byte(s)`、
      build-id `4d8250b5…55ba` 与 C3 轮同一份 —— 生成码变了但 build-id 不含它）。
      期望字符串就写在这三条常量里并被测试断言：`cli::tests::{TOUR_EXPECTED_STDOUT, TOUR_EXPECTED_STDERR, PANIC_EXPECTED_LINE}`。
      **程序 stdout 为空不是"没接上"，是没有任何 INFY 构造能写 stdout**：§14.3 的导览本身不打字，
      §7.7.1 第 4 项又把 B 轨 `print()` 判给日志汇（`infy_log` → stderr）而不是 stdout —— 登记 **F19**，
      所以转发链是用**非空的 stderr** 反向钉住的：合成 module `target/scratch/cli-run-boom`
      （`overflow="panic"` 的 `u32::MAX + 1` 落在链节点里）→ 运行时 panic → **exit 3**（§13.4 的 3），
      §9.2 报告首行 `runtime[R-0001]: 整数溢出（\`overflow=panic\`）` 逐字节转发，另断言
      `infy_add_u32 overflowed` 与 `(node_idx 0)`。`--arg K=V` / `--feature f` → **显式 exit 1**（规范列了
      这两个开关但没有任何消费者，见 D-35(c)/F19），不静默忽略。
      **第一次执行就把一个从未暴露的缺陷打了出来（偏差 D-34）**：§8.2"`goto` 语义（移动焦点，暂停当前
      节点之后）"按"只登记目标、节点跑完、最后一条 `goto` 赢"来读，会让 §14.3 导览的节点 3 先 `goto("done")`
      再被 `goto(+1)` 覆盖 → `retry_once()` → `{ goto("begin"); }` → 回节点 0 **无限循环**。现场表现 =
      产物挂住 120 秒、0 字节输出、被强杀，而 `infy check`/`infy build`/`infy artifact info` 全绿
      （这条路径在此之前**从未被执行过**）。裁决 = `goto` 立即结束所在节点（§8.2 的"移动"与 `start` 的
      "不移动"成对反义、§8.5 抢占点 ①、且读法 2 让规范样例自己死循环）；发射形态
      `infy_goto_checked(...); drain_pops(); return;`，`drain_pops()` 保住 §8.4 规则 4"defer/临时量 drop
      计入该节点的退出动作集"；`infy_goto_checked` 与 `R-0010` 兜底不动，**C 侧零改动**。
      落地面：`src/cli.rs`（`frontend()` 抽出供 check/build/run 共用、`run_module()`+`run_cmd()`+`RunOutcome`、
      临时目录 `infy-run-<pid>-<seq>`、`--verify-reproducible` 在 run 路径上剥掉）、`src/codegen.rs`（goto 转移控制）。
      回归全部复跑：`cargo test --offline --lib` **395 passed / 0 failed**（基线 391，+1 `codegen` +3 `cli`），
      `cargo check --offline --lib` 0 error / **51 warning**（未新增），`infy check examples/tour` exit 0，
      `infy build examples/tour` exit 0（生成的 C11 从 12473 → **12513 字节**，产物仍 199975 字节），
      `--verify-reproducible` exit 0（199975 == 199975），`gcc -std=c11 -Wall -Wextra -I runtime -c 生成的 ci.c`
      exit 0（**0 error / 0 warning**），`(cd examples/tour && infy fmt --check)` 仍 exit **2**（F6 未裁决，未变）；
      C 侧虽未改源码仍按纪律复跑四条链：`gcc -c runtime/infy_rt.c` **0 warning** exit 0、
      rt harness **32335 assertions / 0 failures** exit 0（必须在 `target/scratch` 里跑：夹具按
      `../../runtime/fixtures/` 取，本轮先在 `target/scratch/c4` 跑出了 2 条假失败）、
      替身链 IFBC **82/82**、生产链 IFBC **82/82**、两份 stdout `cmp` 无输出。
      规范未定 → 选定实现并记偏差 **D-34**（goto 立即结束节点）与 **D-35**（`run` 的输出分流 /
      退出码映射：正常退出原样转发、无退出码记 3、启动失败 6、临时目录写不进 4 / `--arg`·`--feature` 显式拒绝）。
      **明确未做**（不用假成功掩盖）：`#[test]` 的执行 = C5；程序侧 stdout 没有任何构造可写 = **F19**；
      运行时按 §9.2 尝试读源行时拿的是 module 相对路径，进程 CWD 不在 module 根就必然打
      `<source unavailable>`（本轮 panic 现场实证）= **F20**；`infy run` 不向程序传 argv（§13.4 未定义该通道）。
      证据提交：**`7f7a4ce`**（`feat(C4)`）—— 上述 395/0、0/51、run exit 0/exit 3、12513 字节、199975 字节、
      `--verify-reproducible` 0、`gcc -c` 0/0、四条 C 链（32335/0、82/82×2、`-c` 0 告警）全部为该提交上的
      实测复跑，非引用。**该提交已推上 `toolchain`**：本轮第 1 次尝试就 exit 0
      （`96f09b0..7f7a4ce`），`gh api …/refs/heads/toolchain` 回查远端 = 本地 HEAD，现算欠账 **0**。
- [x] C5 `infy test`：编 `#[test]` 函数 + 执行 + 退出码（§5.9/§13.4）。
      验收：`infy test` 把 `#[test]` 编进入口并真跑，退出码按 §13.4；期望串写进测试。
      实测（三套 `target/scratch/` 合成 module + `examples/tour`，期望串在
      `cli::tests::{TEST_GREEN_REPORT, TEST_RED_REPORT, TEST_NO_TESTS_REPORT}`）：
      **全绿套件 → exit 0**，程序 stdout 139 字节、逐字节 =
      `test example/ctest/cmd/ctest::calls_work ... ok` / `…::result_ok ... ok` /
      `test result: 2 passed, 0 failed, 2 total`，程序 stderr 0 字节（同 C4：非空即说明容器/策略链断了）；
      **有测试返回 `Err` → exit 1**（§13.4 的一般性失败，报告 `1 passed, 1 failed, 2 total`，
      套件不中断）；**`#[test]` 里 panic（`overflow="panic"` 的 `u32::MAX+1`）→ exit 3**（§13.4 的 3），
      §9.2 首行 `runtime[R-0001]: 整数溢出（\`overflow=panic\`）` 逐字节转发，且**排序在 panic 之后的
      测试不出现在报告里**（断言的是次序，不是存在）；`infy test examples/tour` → **exit 0**，
      程序 stdout 恰为 `test result: 0 passed, 0 failed, 0 total`（**没有 `#[test]` 不是失败**，
      该行同时证明 harness 没调 `infy_runtime_start` ⇒ tour 的 `Pipeline` 一条节点都没跑）。
      `--cov` → **显式 exit 1**（§12.4 的插桩没有任何阶段产出，见 F22）；`--bench` 是唯一让
      `#[bench]` 进运行集的开关，而 §14.7 的基准签名依赖 `std/test` ⇒ 有参数的 bench 报
      `CgError`（exit 6）并点名"§10 的 std 表里没有 `std.test`"（F21），无 `--bench` 时该函数根本不进套件。
      **§5.9 的两句"不进普通构建"此前从未接线**：`pkg` 早就算好 `is_test`/`is_test_pkg`/`constraint`，
      `pkg::constraint_matches` 也存在，但**全仓库零调用点** —— 于是 `infy check|build|run` 把
      `*_test.infy` 一起编了，`E-0012`（`lex_file_with_features` 唯一会发）也从不触发。本轮接上
      （`pkg::BuildConfig` + `pkg::file_selected` + `cli::load_scoped`）并实测同一 module：
      `infy check` = **1 file**、`infy test` = **2 files**；`//infy:build ignore` 的类型不合法的草稿
      在任何构建里都不入选；`//infy:build riscv` → `E-0012`（未知名检查放在选择之前、对每个文件做，
      否则拼错的约束名会把文件变成"从来没存在过"）。偏差 **D-37**（含 §2.2 全集漏了 `ignore`/`test`、
      解析失败借用 `E-0012`、§5.1 R1 为 `package <dir>_test`/`tests/` 的文件主干开例外）；
      harness 本身的四处规范未定记 **D-36**（入口被替换、只认 `()`/`Result` 两种判据、panic 中止整条套件、
      报告走生成的 C 自己 `fputs` 到 stdout）。
      **一次探针揪出两个真实缺陷**（都不是规范歧义，已修并各带回归测试，细节见 `docs/status.md` 的 C5 一节）：
      (1) §5.1 R2 的判据写在**反的地方** —— `names.insert(pkg_name).is_some() → continue` 把
      "**同名**的第二个文件"当作违规，于是 §14.1 自己的 `mod.infy`+`cache.infy` 形态会被**静默丢掉一个
      文件**并假报 `E-0501 内存在两个包名`（实测 `infy check` 在合法 module 上 exit 2）；改为对
      "不同包名的集合"判定，并只允许 §5.9 的 `{<目录>, <目录>_test}` 并存。
      (2) `discover_into` 直接用 `read_dir` 的顺序，而该顺序决定 `FileId` 编号 → 发射的 C 文本 → build-id，
      即 §7.3 的可复现承诺依赖文件系统；现在 `files`/`subdirs` 按名字排序。
      (3) **接线时差点引入的第三处（本轮已挡）**：`cli::package_dirhashes` 的内容取自 `Loaded::files`，
      文件一按 §5.9 过滤，§5.8 第 1 项的"module 的**全部文件**"就跟着命令变，dirhash 与 §7.3 的 build-id
      会一起变成"哪条命令在跑"的函数；改为从 `pkg::discover` 重新取全树并逐文件读字节，成对断言 =
      `cli::tests::dirhash_covers_files_the_build_excludes`（普通构建 1 file / 测试构建 2 file，
      而两种 scope 下逐一哈希相等）。
      落地面：`src/pkg.rs`（`BuildConfig`/`file_selected`/`BASE_CONSTRAINTS` 补 `ignore`+`test`/
      `constraint_matches` 改收配置/排序/R1-R2 例外）、`src/lexer.rs`（`known_constraint_names()` 改为
      返回 `pkg::BASE_CONSTRAINTS`，消掉手抄的第二份清单）、`src/cli.rs`（`Scope`、`load_scoped`、
      `triple_parts`、`frontend`/`build_c` 带 scope、`resolve_entry` 在 test 构建跳 `E-0600`、
      `test_cmd` + `forward_program`、`package_dirhashes` 改走全树、`--cov` 拒绝）、`src/codegen.rs`（`Config.{test_harness,bench}` +
      `emit_test_driver`）。
      回归全部复跑：`cargo test --offline --lib` **410 passed / 0 failed**（基线 395，+5 `pkg` +3 `codegen`
      +7 `cli`），`cargo check --offline --lib` 0 error / **51 warning**（未新增，F2 没变大），
      `infy check examples/tour` exit 0（仍 2 files），`infy build examples/tour` exit 0
      （生成的 C11 仍 **12513 字节**、产物仍 **199975 字节**、`build-id 4d8250b5…55ba` 与 C3/C4 同值 ——
      非 test 路径逐字节未变），`--verify-reproducible` exit 0，`infy run examples/tour` exit 0（0/0 字节），
      `infy artifact info <产物> --nodes` exit 0，`gcc -std=c11 -Wall -Wextra -I runtime -c <ci.c>` exit 0
      （0/0），`(cd examples/tour && infy fmt --check)` 仍 exit **2**（F6 未裁决，未变）；
      **C 侧本轮零改动**（没动 `.c`/`.h`），四条链仍按纪律复跑且改动后重跑一次：`gcc -c runtime/infy_rt.c`
      **0 warning** exit 0、rt harness **32335 assertions / 0 failures** exit 0（在 `target/scratch` 里跑）、
      替身链 IFBC **82/82**、生产链 IFBC **82/82**、两份 stdout `cmp` 无输出。
      **明确未做**（不用假成功掩盖）：`std/test`（§14.7 的 `assert_eq`/`Bench`）在 §10 的 std 签名表里
      一条都没有 ⇒ §14.7 的样例仍过不了 `infy check`，本轮不代它发明签名（**F21**）；`--cov` 的节点表插桩
      （§12.4）没有阶段产出（**F22**）；test 产物的 `.inf meta` 入口字段仍写 §7.4 的 `fn main`/首条链，
      与真正执行的 harness 不符（冻结容器没有"harness"这一档，**F23**）；`W-0003`（约束恒假）需要静态
      不可满足性判定，本轮只对当前配置求值（**F24**）；panic 的**逐测试隔离**要把内部入口 `infy_rt_catch`
      提升进冻结公共头（ABI 变更 + 四条链同批），本轮按 C4 的"能不动 C 侧就不动"如实实现并记录；
      `#[test]` 函数在普通构建里仍被编译成 C 符号（只是不进运行集），§5.9 没有要求剔除。
      证据提交：**`f706c3d`**（`feat(C5)`）—— 上述 410/0、0/51、`infy test` 的 exit 0/1/3 与逐字节报告、
      12513/199975 字节与 `build-id 4d8250b5…55ba` 未变、`--verify-reproducible` 0、`gcc -c` 0/0、
      四条 C 链（32335/0、82/82×2、`-c` 0 告警、两份 stdout 逐字节相同）全部为该提交上的实测复跑，非引用。
      **该提交已推上 `toolchain`**：本轮第 1 次尝试即 exit 0（`1fc8803..f706c3d`），
      `gh api …/refs/heads/toolchain` 回查远端 = 本地 HEAD，现算欠账 **0**（本节的做账行提交后又 = 1，
      下一轮仍只认现算）。

### 阶段 D：双轨嵌入的真实端到端

- [ ] D1 `src/btrack.rs` 收敛（子任务被截断）：CPython 探测 → 能力扫描 → lowering → IFBC。
      验收：`@py` 的 tour 片段产出可被 `infy_ifbc.c` 装载的模块；`eval` 触发 `E-0711`；未映射 opcode 触发 `E-0712` 并点名具体项；声明 `py = "3.13"` 而本机是 3.14 触发 `E-0713`。
- [ ] D2 `infy ifbc dump|verify` 接线（Rust 侧编码 ↔ C 侧虚拟机**交叉验证**：同一镜像两边读出的反汇编一致）。
- [ ] D3 `src/atrack.rs`：`#[link]` + `@c` 配对、`infy_binding.h` 生成、`infy_e_<name>_<8hex>` 包装、缺 `unsafe` 报 `E-0721`、缺 `#[owns]/#[borrows]` 报 `E-0723`。
      验收：一个最小 `@c` 单元被 gcc 编译、链接、在产物里被调通。
- [x] D4 **Rust 编码 ↔ C 虚拟机操作数个数对齐**（A2 本轮静态比对发现，未修；D2 的交叉验证必然踩到）：
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
      **D-38 轮（2026-10-14）状态 = 裁决完毕、实现未动**。全面审计发现范围比上面写的更大（三层：
      容器包装有无 / `type_table`·`import_table`·`func_table` 记录形态 / 指令模型家族 vs 具体字节 ——
      算术块 `0x20..0x3F` 两侧字节语义整体错位，且 `guard` 字节、`field_off` 操作数、区段包装都不同；
      两侧从未交换过一字节，因为 `.inf ifbc` 恒空，所以从未现形）。裁决与新发现规范矛盾已写入
      `docs/spec-deviations.md` **D-38/D-39**（提交 `b422629`），逐项钉死哪侧改什么。
      **D4 余项（按 D-38/D-40 机械执行，无需再审计）**：
      (1) **Rust 侧 —— 已完成（提交 `d7d8991`，2026-10-15 D4(1) 轮）**：删区包装、三表改合并形状、
          `TyKind` 重编号到 `SIG_*`（前 18 行自动置标量行，`N_SCALAR_ROWS`）、guard/box.dyn 去操作数、
          ldw/field_off 补操作数、switch 解禁（`1+4(1+n)` 宽度进边界表与线性扫描、上限 48 同装载器）、
          `ldg/stg` 迁到 0x0E/0x0F、校验面另补三条对齐（高度 vs `max_stack`、`ret/ret.multi` vs
          `ret_arity`、tailcall 帧五元组并按 D-22 报 `R-0707` 而非 `R-0709`）。
          **动手时补裁了 D-38 没点名的 5 处操作数形状 + 5 处实现选择 → 偏差 `D-40`**（含"自动置的标量行
          `size/align` 留 0"这条：仓库唯一的布局真相是 ABI §1 + `hir` 的 gcc 探针，而那里没量过
          `bytes`/`dyn`，写一个没量过的数 = 编造常数）。btrack 连动：`Instance`→`Dyn` 改名 5 处、
          `push_func` 收 `max_stack`（`CodeBuilder` 的运行栈高峰值，保守上界）、guard/box.dyn 操作数清空、
          `Selected::Object` 改为按 `arity()` 构造操作数并在需要更多操作数时显式拒绝。
          证据：`cargo test --offline --lib` **423 passed / 0 failed**（基线 410，`ifbc` 6 → **19**）、
          `cargo check --offline --lib` 0 error / **50 warning**（基线 51，删死字段）、tour check/build/
          `--verify-reproducible`/test 全 exit 0 且 12513/199975/`build-id 4d8250b5…55ba` 未变、
          `fmt --check` 仍 exit 2（F6）、四条 C 链复跑 0 告警 / **32335/0** / **82/82** / **82/82** + `cmp` 无输出。
          Rust 侧的镜像字节表落成文件 `docs/internals/ifbc-encoding-rust.md`（**本轮新建**：
          `src/ifbc.rs` 的模块注释一直指向它，而它此前根本不存在，同 A3 轮 `docs/coverage.md` 那一族）。
      (2) **C 侧 —— 已完成（提交 `e5555dc`，2026-10-15 D4(2) 轮）**：OPS 表按 D.3 家族编号重排
          （`guard.nonzero/shift` → 0x27/0x2F 且 0 操作数，`add..cmp` 一族一字节 + 操作数 0 = 类型行，
          旧 `add.f64..cmpeq.f64` 与 0x3E/0x3F 让位）、`exec` 按 `types[op0].kind` 选 i64/f64 路径
          （`fam_path`；非 i64/f64 装载放行、执行 R-0706）、`push` 1→2、`ldw` 注解按 kind 判定、
          `is.dyn/cast.dyn` 走 `kind_to_dyn_tag` 表、三表读取器合并形状（type 8×u32 含 `payload_a/b`、
          import 尾 `sig_hash u32`、func 48 字节/12×u32）、`new.list/ty,count`·`new.dict kt,vt,count`·
          `new.tuple n` 按 count 弹出、17 条间隙指令全部进 OPS/校验/反汇编（11 条有执行实现，6 条
          装载通过而执行显式 R-0710 = 偏差 **D-41**、登记 **F27**）、switch 上限两侧同为 48
          （C 侧原表达式是 64，D-40 选择 5 的算术写错）、`runtime/IFBC.md` §1.2/§1.3/§1.5/§3/§3.1/§5/§6/§7
          随之改写、`runtime/test_ifbc.c` 同批改：**82 → 146 断言**（0 项放宽）。
          **两侧第一次在字节上相遇**：新夹具 `runtime/fixtures/ifbc_rust_writer.bin`（1031 字节，
          `src/ifbc.rs` 写入器的逐字节固定输出，只有 `INFY_REGENERATE_FIXTURE=1` 才重写）由 C 侧
          `test_rust_writer_fixture` 用**生产装载器**读回、过六项校验、并**执行**其中两个函数
          （`add_one(41) = 42` 走家族字节 + 类型操作数；`pool_str()` 走 `push k,ty`），
          Rust 侧 `ifbc::tests::c_loader_fixture_is_exactly_what_the_writer_produces` 断言
          "重新生成 == 夹具"且"构建期校验器对这些字节零 R-码"。
          证据：替身链 IFBC **146/146** exit 0、生产链 IFBC **146/146** exit 0 且两份 stdout `cmp` 无输出、
          `gcc -c runtime/infy_ifbc.c` 与 `-c runtime/infy_rt.c` 均 **0 告警**、rt harness
          **32335/0**、`cargo test --offline --lib` **424 passed / 0 failed**（基线 423，+1 夹具测试）、
          `cargo check` 0 error / **50 warning**、`infy check|build|test examples/tour` 全 exit 0 且
          12513 字节 C11 / 199975 字节产物 / `build-id 4d8250b5…55ba` 逐字节未变、`fmt --check` 仍 exit 2。
      (3) 交叉验证 —— 同一镜像 Rust 反汇编 == C 反汇编逐指令相等（D2 的第一批断言），再回 D1 的装载验收。
          **已完成（提交 `1f8988d`，2026-10-15 D4(3) 轮）**。做法按 §2 的建议：不动人类可读输出，
          两侧各加一份"机器可比"的规范化文本（C 侧 `infy_ifbc_dump_canonical` /
          `infy_ifbc_verify_canonical`，Rust 侧 `ifbc::dump_canonical` / `ifbc::verify_canonical`），
          行语法 = `H/T/P/F/I/L/X/S`（`runtime/IFBC.md` §6.1），逐位十进制、只含双方都保留的字段。
          **三份夹具、双向锁点**：`.bin` 由 Rust 写入器生成（`INFY_REGENERATE_FIXTURE=1` 才重写），
          `.can.txt`/`.ver.txt` 由 **C 侧**渲染（`INFY_REGENERATE_CANONICAL=1` 才重写），Rust 侧
          `ifbc::tests::canonical_render_equals_the_c_loader_render` 断言 == 同一份文件 ⇒ 谁漂谁红：
          `ifbc_rust_writer.bin`（1031 字节，另断言两侧**阻塞性**校验结论都为空）、
          `ifbc_all_ops.bin`（2083 字节，**附录 D 全部 opcode** 一条一 `I` 行，含 17 条间隙指令、
          `switch` 内联 case 表、类型操作数越界的 `?` 后缀）、
          `ifbc_verify_matrix.bin`（1890 字节，每函数一个缺陷 ⇒ `(code, func, off)` 逐条相等：
          `R-0704 ×2 / R-0705 / R-0706 ×3 / R-0707 ×2 / R-0710 / R-0713`）。
          **第一次跑就抓到三处分叉**（前两条改 C 侧、第三条改本侧；全部记入偏差 **D-42**）：
          ① `coro.resume`/`coro.throw` C 侧 `nops = 0` vs Rust `arity = 1`，而 D.8 明写 `h` ⇒
             指令宽度 `1+4*nops` 错 4 字节，C 侧会把协程指令之后的**整条指令流**读歪（本轮 `I` 行
             立刻分两截）；C 侧改 1，执行仍从操作数栈取值、`h` 是与 `ldw` 注解同类的"执行不读"注解。
          ② D.9 助记符：C 侧与两份文档写 `size_of.idx`/`align_of.idx`/`field_off.idx`，规范是
             `size_of idx`/`align_of idx`/`field_off idx`（`.idx` 是操作数；只有 `typeof.idx` 是名字）
             ⇒ 改 C 侧 + `IFBC.md` §1.2/§3.1 + `ifbc-encoding-rust.md` 表格。
          ③ 本侧 `verify_func` 的**重复报告**缺陷：操作数/邻接检查写在高度走查里，而走查把直线后继
             也 `work.push(next)` 回栈 ⇒ 同一 `(code, off)` 报 2–3 次（C 侧按边界各查一次报 1 次）。
             修：检查搬出走查（每指令一次，且不再漏掉不可达指令）+ 后继不再压栈（分支/跳转目标仍压）。
          **F26 的账**：verify 结论本轮**比过了**（矩阵那 10 条逐 `(code, func, off)` 相等，
          且写入器夹具两侧都为空），但 (a) `call.virt`/`call.embed`/`invoke.dynamic` 本侧仍判
          `R-0705`、(b) `coro.*`/`throw.inf` 的 δ 与 C 的 `sim_delta` 仍不同、(c) 操作数越界本侧在
          `decode` 就拒（集合不相交，进不了"两侧都能读"的比较）、(d) 本侧另要求"函数必须以终止指令
          收口"、(e) `W-0705` 粒度不同 ⇒ 这五条清单已逐条写进 D-42，"两侧装载期校验等价"这句话
          **仍然不许讲**，`docs/coverage.md` 按此措辞；消掉 (a)(b) = F26 的能力表扩容项。
          证据：`cargo test --offline --lib` **426 passed / 0 failed**（基线 424，`ifbc` 20 → **22**）、
          `cargo check --offline --lib` 0 error / **50 warning**（未新增）、IFBC harness
          **146 → 167 断言 / 0 FAIL**（两条链同数、stdout `cmp` 无输出）、`-c infy_rt.c` 与
          `-c infy_ifbc.c` 各 **0 告警**、rt harness **32335/0**、
          `infy check|build|test examples/tour` 全 exit 0 且 12513 字节 C11 / 199975 字节产物 /
          `build-id 4d8250b5…55ba` 逐字节未变、生成 C 的 `gcc -c` 0 error/0 warning、
          `fmt --check` 仍 exit **2**（F6 未裁决）。
          D4 三段全部落地。**证据提交：`1f8988d`**（`feat(D4-3)`）—— 上述 426/0、0/50、167/167 两条链、
          `-c` 各 0 告警、32335/0、tour 四条 exit 0 与 12513/199975/`4d8250b5…55ba` 未变、`gcc -c` 0/0、
          `fmt --check` exit 2 全部为该提交上的实测复跑，非引用；本节的做账行在其后的提交里。
          附带成果：`.gitattributes` 把 `runtime/fixtures` 的夹具钉成 `binary` / `-text` —— 本机
          `core.autocrlf=true`，没有这份属性则检出会把 C 侧写的 LF 结尾文本改成 CRLF，双侧按原始字节
          比较会因此**假红**（不是内容漂移）；动夹具前先确认这条还在（手法：`rm` 掉再 `git checkout --`，
          然后 `cmp`）。

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

- [ ] F9 **`parser::primary()` 的 `Kind::Lit(Lit::Int(i))` 臂永不可达**（C1 轮定位，未改）：
      它排在 `Kind::Lit(l)` 之后，而后者匹配所有字面量，于是 `ExprKind::Int(IntLit)` 这一形状**在全树从不产生**。
      后果是凡只匹配 `ExprKind::Int` 的代码都是死代码 —— `variant()` 那条已由 C1 修掉（`A = 3` 曾静默丢判别式），
      但 `ast::Attr::int_arg()`（`#[link(name = "…", ordinal = 3)]` 一类）、`cli.rs` 与 `sema.rs` 里的
      `ExprKind::Int` 判据仍需逐条审计：要么改成双形状匹配，要么删掉死臂让 `primary` 先匹配 `Lit::Int`。
      验收：`grep -n "ExprKind::Int" src/*.rs` 的每一处都有测试证明它可达（或已删除），`primary` 的臂顺序有回归测试。

- [ ] F10 **`sema` 收集枚举判别式时 `u128 as u64` 截断**（C1 轮定位，未改）：
      `Variant.disc` 是 `Option<u128>`，`sema.rs:616` 直接 `as u64`，所以宽于 `u64` 的字面量**先回绕**才交给 HIR，
      D-25 的 `u32` 容纳检查拦不住已回绕的值。
      验收：宽于 `u64` 的判别式报 `E-0042`（而不是回绕）；成对断言 `A = 4294967296` → `E-0042`（HIR，C1 已有）
      与 `A = 2^64` → `E-0042`（sema，待补）。

- [ ] F11 **`[T; N]` 的 N 不是字面量时被 `unwrap_or(0)` 静默当成长 0**（C1 轮定位，未改）：
      `sema.rs:439` 是 `let n = self.const_eval_usize(len).unwrap_or(0);`，而 `const_eval_usize` 在 §2.12 闭集之外返回 `None`
      —— 于是 `let a: [u32; SOME_CONST]` 的类型变成 `[u32; 0]`，不报 `E-0131`，布局与边界检查全按 0 元素算。
      HIR 侧本轮至少不再猜：`hir::hir_ty_from_ast` 遇到非字面量长度返回 `None` → `UnresolvedType`。
      验收：`[u32; 常量表达式]` 要么按 §2.12 求值成功，要么显式 `E-0131`，不得静默取 0；补 `sema` 的成对测试。

- [ ] F12 **owned 值的移动在 stage [8] 被拒绝，根因是运行时清理队列只支持弹栈顶**（C2 轮定位，实现选择见偏差 **D-31**）：
      `infy_rt.h:115-118` 的清理 API 是 `infy_cleanup_push(fn, &obj, is_defer)` + `infy_cleanup_pop_run()` /
      `infy_cleanup_pop_noop()`，登记的是**地址**且只能弹**栈顶**。于是这三类合法 INFY 写法无法发射：
      (a) `let d: String = s;`（换地址）；(b) `take(s)` 按值传形参（ callee 与 caller 各有一份清理项 → 双 `infy_free`）；
      (c) `return s;`（移动出作用域后仍会跑它的 drop → 悬垂）。
      现状：codegen 对三者一律报 `CgError`（§4.7.1 / §4.7.4），锁定点 = `codegen::tests::moving_an_owned_binding_is_refused`。
      验收：给运行时加"按地址取消/搬迁清理项"的入口（如 `infy_cleanup_relocate(void* old, void* new)` 或
      `infy_cleanup_cancel(fn, obj)`），实现三条移动形式，并把上面那条测试从"必须报错"改成"必须生成 + 成对断言
      旧地址不再被释放"；改 `infy_rt.h` 后**四条 C 链都要复跑**。

- [ ] F13 **`-128` 这类无后缀负字面量被阶段 [4] 判成 `E-0110`**（C2 轮写测试时确认，未改实现）：
      `src/sema.rs` 的一元负号只接受 `is_signed() || is_float()` 的 `Ty::Prim`，而 §4.13 规则 2 让无后缀字面量取
      `Ty::IntLit`（尚未定型），于是 `fn g() -> i32 { -128 }` 报 `E-0110 · 一元负号作用于非数值类型`。
      §2.8 明写"`-128i8` 例外由常量求值折叠"，说明规范认为这类写法合法；C1 的判别式测试（`A = 3`）也说明
      "字面量在类型约束之前就被拒"是本仓库反复出现的一类坑。
      验收：`-<IntLit>` 取上下文类型（或按 §4.13 规则 7 回落 `i32`）后做 §2.8 的范围检查；
      成对测试 `fn g() -> i32 { -128 }` 通过、`fn g() -> u32 { -1 }` 报 `E-0111`。

- [ ] F14 **构造器模式的绑定没有进入作用域**（C2 轮写测试时确认，未改实现）：
      `fn g(q: Q) -> u32 { match (q) { Q::X => 0, Q::Y(n) => n } }` 报 `E-0101 · 未声明的标识符 n`，
      即 §3.12 的绑定形如 `Variant(b)` 在臂体里不可用（同函数里 `Q::X => 0` 正常）。
      后果：codegen 的绑定发射（`pat_bind`）在真实程序上无法被端到端验证，目前只有构造器**条件**部分被 tour 之外的
      合成测试覆盖（`codegen::tests::enums_elide_the_union_when_no_variant_carries_a_payload` 因此改用
      `Q::Y(n) => 1` 的"绑定不使用"形式）。
      验收：`sema` 在检查臂体前把模式绑定压入局部表，令上例通过；补 `E-1001`（移动后使用）与穷尽性的成对测试。

- [ ] F15 **`fmt!` 没有 lowering 路径**（C2 轮确认，属 C2 验收清单里唯一被上游阻塞的项）：
      §10.11 的 `fmt!("fetch #{}", j.id)` 在 §14.2 是规范样例，但 `macro_expand` 不认 `fmt!`（它是编译器内建而非
      `macro_rules!`），`src/sema.rs:1822` 对任何残留 `MacroCall` 发 `E-0402`。因此 `infy check` 就拒绝，
      codegen 只能在"未展开的宏调用"上报 `CgError`（`codegen::tests::uncovered_constructs_are_refused_by_name` 锁的是后者）。
      运行时侧的接口是齐的：`infy_rt.h` 的 `InfyFmtSpec`/`InfyFmtArg`/`infy_fmt_run`，§10.11 也规定格式串在编译期解析。
      验收：阶段 [2] 把 `fmt!` 展开成带 `InfyFmtSpec` 表的构造（或阶段 [4] 直接定型、阶段 [8] 发射 specs 数组 +
      `infy_fmt_run` 调用），并让 §14.2 的 `describe` 样例（含 `String` 返回）通过 `infy check` + `infy build` + gcc。

- [ ] F16 **阶段 [10]（产物签名/校验段写入 → dirhash + lock 比对，§5.8/§7.1）未做**（C3 轮落地 [9] 时确认）：
      `artifact.rs` 只把 `MODULE.infy.lock` 的 `dirh` 三元组抄进 `.inf sum`（12 字节/行 = `package_pool, version_pool, dirhash_pool`），
      **没有任何一侧拿它去比对**，`infy_rt.c` 读 meta 后也不校验 sum 区。tour 无依赖 → 该路径在现有一切实例上是**空集**，
      所以 [9] 的绿不代表 [10] 通。
      验收：`infy artifact verify <产物>`（或 `artifact info` 的一档 `--check-sum`）逐条把 `.inf sum` 与 lock/sumdb 比对，
      不一致 → 用已有的 §5.8 族码（`E-0580`/`E-0583`）并 exit 7；`.inf sum` 为空时明确打"0 条依赖，无可校验"，不假装通过。

- [ ] F17 **容器里的 `ResourceKeys` 与 `EnvOffsets` 是空集**（C3 轮确认）：冻结的 208 字节 `.inf meta` 没有 `EnvOffsets` 槽位，
      `ResourceKeys` 连名字都没被前端收集（`std.chain.lock("db")` 的键名在 `sema`/`hir` 里没有出口），
      所以 `artifact::Content.resource_keys` 恒为 `Vec::new()`，`EnvOffsets` 只能让运行时走默认值。
      而 `src/hir.rs` 其实**已经算出** §4.6/§10 的 11 个偏移并有金样（`hir::tests`），只是没写进产物。
      验收：`EnvTable` 写进 blob（新区或 meta 尾部扩展，须与 C 侧同时改并记偏差），`infy artifact info` 打印 11 个偏移；
      `std.chain.lock` 键名收集进 `ResourceKeys`，并由 `infy_rt.c` 用于锁键校验。

- [ ] F18 **C 侧 `infy_art_dump_meta` 的 `meta` 段行写死 `off=0 len=0`**（C3 轮读真实产物时发现，`infy_rt.c:3119`）：
      真实容器里 meta 是 `off=0 len=208`。这是**打印错误**不是容器错误（Rust 侧 `artifact::dump_sections` 打的是真 208），
      但两份工具的输出因此不可比对。
      验收：改成打真实的 `INFY_META_FIXED_BYTES`，并让 `infy artifact sections` 与 C 侧 dump 在同一夹具上逐行可比。

- [ ] F19 **没有任何 INFY 构造能写 stdout，且 §13.4 的 `--arg` / `--feature` 没有消费者**（C4 轮实现 `infy run` 时确认）：
      (a) §14.3 的导览不打字，`codegen` 的 tour 子集里也没有任何通往 stdout 的入口，而 §7.7.1 第 4 项把 B 轨
      `print()` 判给**日志汇**（`infy_rt.c` 的 `infy_log` → stderr）。后果：`infy run` 的程序 stdout 对**现有一切实例**
      恒为 0 字节 —— 转发链只能靠非空的 stderr（panic 的 §9.2 报告）反向钉住，`cli::tests` 里的
      `TOUR_EXPECTED_STDOUT` 是空串并被注释写明原因，不是漏测。
      (b) §13.4 给 `infy run` 列了 `--arg K=V` 与 `--feature f`，但 `env` 的字段集合按 §4.6 是**封闭**的且不含运行参数，
      `[features]` 的求值（`pkg::constraint_matches` 收 `features`）从来没从命令行接进 `load()`/`Manifest`。
      本轮按硬约束显式拒绝（exit 1 + 指到 D-35(c)），没有静默忽略。
      验收：(a) 定一条 stdout 通道（`std.io` 的写入口 + codegen 发射，或把 F15 的 `fmt!` 产物接到 stdout sink），
      让某条真实程序的 stdout 非空并被测试逐字节断言；(b) 要么把两个开关接进 §2.2/§4.6 的真实消费者，
      要么在 `docs/spec-deviations.md` 里记"§13.4 的这两个开关在本实现无对应语义"。

- [ ] F20 **运行时的 §9.2 报告读不到源行**（C4 轮的 panic 现场实证）：节点表里的 `file` 是 **module 相对**显示路径
      （`cmd/boom/main.infy`），而 `infy_rt.c:238` 的 `read_source_line()` 直接拿它去 `fopen`，进程 CWD 不在 module 根
      就必然失败 → §9.2 的源码行恒为 `<source unavailable>`。规范允许"读不到就打占位"，所以这是**可用性缺口而不是
      合规违规**，但两份工具（编译器诊断 vs 运行时诊断）因此在真实程序上不可比对。
      同处另记：`read_source_line` 与 `infy_fs_*` 不一样，它按**字节**路径打开文件（`std.fs` 已走 `CreateFileW` +
      UTF-8→宽字符），非 ASCII 路径下 ACP 会咬人。
      验收：产物/容器里带上 module 根，运行时把相对显示路径解析成真实路径后读源行；
      成对测试 = 同一条 panic 在 CWD 为 module 根时打出源行、否则打占位且**不报错**。

- [ ] F21 **§14.7 的 `std/test` 在 §10 的 std 签名表里一条都没有**（C5 轮实现 `infy test` 时确认）：
      §14.7 的样例写 `import "std/test" as test;` 并用 `test::assert_eq(…) -> Result<(), test::Fail>` 与
      `test::Bench`+`b.iter()`，但 `src/stdlib.rs` 的 `STD` 表里没有任何 `std.test` 条目（`std.chain`/`math`/
      `mem`/`meta`/`env`/`fs`/`net`/`process`/`sync`/… 都有）。后果：规范的测试样例过不了 `infy check`，
      `#[bench]` 的签名无法表达，`infy test --bench` 只能拒绝（锁定点 =
      `codegen::tests::bench_functions_enter_the_suite_only_under_bench`，拒绝消息已点名本条）。
      本轮没有代规范发明 `assert_eq`/`Bench` 的签名（P5 零隐式 + "实现不得自定义语义"）。
      验收：先裁定 `std/test` 的签名表进 §10（或裁定测试判据改走别的路径），再实现
      `test::assert_eq`/`assert`/`Bench::iter`，让 §14.7 的样例 `infy check`+`infy test` 双绿。

- [ ] F22 **`infy test --cov` 没有消费者**（C5 轮）：§12.4 说覆盖率来自 `-O0` 构建的节点表插桩且
      "不改变可观察行为"，而阶段 [7]-[9] 都不产出插桩、也没有汇总器。当前显式 exit 1
      （`cli::tests::test_refuses_cov_and_runs_zero_tests_where_there_are_none`），不静默忽略、不假报数字。
      验收：节点表插桩 + 覆盖率汇总（或偏差条目里记"`--cov` 在本实现无对应语义"）。

- [ ] F23 **test 产物的 `.inf meta` 入口字段仍写 §7.4 的 `fn main`/首条链**（C5 轮）：冻结容器只有
      `ENTRY_MAIN`/链两档（`src/artifact.rs::ENTRY_MAIN`、C 侧同），没有"harness"这一档，所以
      `infy artifact info <test 产物>` 打出的"入口"与实际执行的 `main`（生成的 harness）不一致。
      容器本身没错（`infy_art_read_file` 读回同一份），是**声称不准确**；本轮既不改冻结布局也不改声称。
      验收：给容器加第三档入口（需与 C 侧同批改并记偏差），或让 `artifact info` 在 test 产物上明打
      "入口 = §5.9 harness（非 §7.4 入口）"。

- [ ] F24 **`W-0003`（构建约束恒 false）未实现**（C5 轮接 §2.2 选择时确认）：§2.2 要求"约束恒假 → `W-0003`"，
      那是对约束表达式做**静态不可满足性**判定（如 `linux && windows`、`!x && x`），而
      `pkg::file_selected` 只对当前 `BuildConfig` 求值 —— 一个"对谁都不入选"的文件会被静默丢掉。
      同族的 `E-0011`/`E-0013`（位置/重复）由 `src/lexer.rs` 负责，已活。
      验收：`pkg` 里对约束做DNF式不可满足判定并打 `W-0003`（strict 下 exit 2），补 `pkg::tests` 成对断言。

- [ ] F25 **`new.set ty` 没有元素个数的落点，而 `effect()` 把类型索引当弹出个数用**（D4(1) 轮补
      `CodeBuilder` 的 `max_stack` 峰值时定位；已在偏差 **D-40** 点名，本轮不改语义）：
      `src/ifbc.rs` 里 `Op::NewSet` 的 `operand_kinds()` 是 `[Type]`，但 `effect()` 与
      `CodeBuilder::note_height()` 都取 `ops[0]` 当作要弹出的元素个数 —— 于是
      `new.set <ty>` 的栈效果 = 弹出 `ty` 个、压入 1 个。`btrack.rs:2945` 发的正是
      `NewSet, &[t]`（`t` 是 `intern_type(List,"set",elem,…)` 的行号），所以 Python `BUILD_SET`
      的 `n` 在这条路上**丢了**：装载期校验按错误的高度判 `R-0705`/通过，运行时按 C 侧后来的实现解释。
      同一族的 `new.list ty,count` / `new.dict kt,vt,count` / `new.tuple n` 都有显式个数
      （D-38 补充裁定把前两个钉成 2/3 操作数），只有 `new.set` 缺。
      验收：把 `new.set` 钉成 `ty,count`（2 操作数，写进 D-40 的表 + `docs/internals/ifbc-encoding-rust.md`
      + C 侧同批实现），或钉成"空容器 + `seq.push`"并让 lowering 走后者；两种都要给 `btrack` 的
      BUILD_SET 路径补一条**带真实元素个数**的 `ifbc` 断言（现在 `btrack` 0 项单测，D1 会顺手做）。

- [ ] F26 **两侧"装载期校验结论"并不等价**（D4(1) 轮做 `call.embed` 的 import-kind 校验时确认；
      已在偏差 **D-40** 点名）：`src/ifbc.rs::effect()` 对 `call.virt`/`call.embed`/`invoke.dynamic`
      一律发 `R-0705`（"被调元数无法静态确定，校验项 2 无法完成"），而 `runtime/infy_ifbc.c` 会
      **校验并执行**它们（`sim_delta` 的 `SP_CALL_HOST`/`SP_INVOKE_DYN`，`call.virt` 记 δ=0）。
      另外本侧 `stack_effect()` 在 `coro.resume`/`coro.throw`/`throw.inf` 上是 `(0,0)`，C 的
      `sim_delta` 是 `-1/-1/-1`；同一镜像两侧的高度分析结论可以不同。
      这不是线格式问题（D4 的 (3) 只交叉验证**反汇编**，不被它挡），但**"两侧校验等价"这句话现在
      不能讲**，`docs/coverage.md` 的"被测对象是生产运行时吗"一行已按此写明。
      验收：要么 Rust 侧按 C 的 δ 表实现这三条 + 三处 dynamic call 的可证明性（能力表扩容，§7.7.2），
      要么在偏差里记"本侧装载期校验比运行时更严/更松"的具体清单，并让 D4(3) 的交叉验证把
      **verify 结论**也比一遍（逐条 (code, off) 相等），不许只比反汇编就声称对齐完成。
      **D4(3) 轮把后半个验收做了**（提交 `1f8988d`）：清单已逐条写进偏差 **D-42** 的 (a)–(e)，
      比较也真做了 —— `ifbc_verify_matrix.bin` 的 `.ver.txt` 两侧 10 条 `(code, func, off)` 逐条相等，
      而写入器夹具两侧**阻塞性**结论皆为空。**仍开着的是前半个**：本侧要按 C 的 δ 表实现那三条、
      并给三处 dynamic call 做出可证明性（能力表扩容，§7.7.2），才能讲"两侧校验等价"；在那之前
      `docs/coverage.md` 与 README 只能引用 D-42 的清单。本轮顺带在本侧揪出一条**重复报告**缺陷
      （同一 `(code, off)` 报 2–3 次，C 侧按边界各查一次报 1 次），已修并由矩阵钉住。

- [ ] F27 **C 侧对象模型缺三种载体，六条 IFBC 指令只能装载不能执行**（D4(2) 轮落地间隙指令时确认；
      实现选择记在偏差 **D-41** 第 2 项）：`runtime/infy_rt.h` 的 `InfyDyn` union 只有
      `i / f / s / list / dict / clo` 六种载荷，**没有属性表、没有迭代器、没有集合**，于是
      `import.attr`(0x69)、`inst.get`(0x8B)、`inst.set`(0x8C)、`seq.iter`(0x87)、`seq.next`(0x86)、
      `new.set`(0x93) 这六条在 C 侧**解码、校验、反汇编都通过，执行显式返回 R-0710 且不出值**。
      这挡住了 D1 的全部动态形状：Python 的 `LOAD_ATTR`/`STORE_ATTR`/`GET_ITER`/`FOR_ITER`/`BUILD_SET`
      都落到这六条上，也就是说 `@py` 片段即使装进 `.inf ifbc` 也只能跑不含属性访问与迭代的那一小部分。
      同族：`new.set` 的操作数形状本身是 **F25**（没有元素个数落点）。
      验收：给 `InfyDyn` 加一种属性表载荷（`DY_INSTANCE` 带 key→value 的 dict + `infy_dyn_attr_get/set`
      两个私有入口，冻结头改 = 四条 C 链同批），迭代器按"容器 + 下标"的显式 dyn 形状实现（须与 Rust 侧
      `seq.iter`/`seq.next` 的 δ (1,1)/(1,2) 对齐），集合用去重的 list 或新增 tag；六条都要有
      **执行成功**的 harness 断言（今天的锁点只断言"装载通过 + 执行被显式拒绝"，见
      `test_ifbc.c::test_model_refusal_is_explicit`）。

## 2. 当前指针

> **下一轮第一件事：D1（`src/btrack.rs` 收敛，§1 阶段 D 的第一个 `[ ]`）**。
> 阶段 D 的线格式债已清完：D4 的 (1) Rust 侧 `d7d8991`、(2) C 侧 `e5555dc`、(3) 双侧文本交叉验证
> `1f8988d` 都落地；裁决在 **D-38**，动手时补裁的在 **D-40**（Rust）/ **D-41**（C）/ **D-42**
> （交叉验证本身，含它当场抓到的三处分叉）。两份形状文档 =
> `docs/internals/ifbc-encoding-rust.md`（Rust，§6 是 D4(3) 新增的"双侧共用文本"）与
> `runtime/IFBC.md`（C，§6.1 = 行语法）。
>
> **D4(3) 之后新增的常驻面（D1 直接要用）**：
> (0a) 双侧共用的规范化文本：C 侧 `infy_ifbc_dump_canonical` / `infy_ifbc_verify_canonical`，
>     Rust 侧 `ifbc::dump_canonical` / `ifbc::verify_canonical`，行语法 `H/T/P/F/I/L/X/S`。
>     **方向分工是设计核心**：`.bin` 只有 `INFY_REGENERATE_FIXTURE=1` 能重写（Rust 写入器是唯一作者），
>     `.can.txt` / `.ver.txt` 只有 **C 侧**的 `INFY_REGENERATE_CANONICAL=1` 能重写，两侧各自断言同一份
>     文件 —— 改线格式 / 表形态 / 助记符 / 操作数个数 = 至少一侧红，没有一侧能单独定义答案。
>     D1 产出的每份 `@py` 镜像都能直接挂上这条锁点，不需要再造比较器。
> (0b) 三份夹具：`ifbc_rust_writer.bin`(1031B) + `.can.txt`、`ifbc_all_ops.bin`(2083B) + `.can.txt`
>     （附录 D 全部 opcode，一条一个 `I` 行，含 17 条间隙指令、`switch` 内联 case 表、类型操作数
>     越界时的 `?` 后缀）、`ifbc_verify_matrix.bin`(1890B) + `.ver.txt`（11 函数 / 10 条
>     `(code, func, off)`）。IFBC harness 因此 **146 → 167 断言**（两条链同数）。
> (0c) `.gitattributes` 把 `runtime/fixtures` 钉成 `binary` / `-text`：本机 `core.autocrlf=true`，
>     没有它检出会把 LF 文本改成 CRLF ⇒ 双侧逐字节比较**假红**（不是内容漂移）。动夹具前先确认这条
>     还在（手法：`rm` 掉再 `git checkout --`，然后 `cmp`）。
> (0d) C 侧 `coro.resume` / `coro.throw` 现在是 **1 操作数**（D.8 的 `h` 在线上；执行仍从操作数栈取
>     被 resume 的值，`h` 与 `ldw` 的类型注解同类：装载与反汇编读、执行不读）。D1 的发侧别再按
>     0 操作数想。D.9 的助记符名也按规范改成 `size_of` / `align_of` / `field_off`（`typeof.idx` 是名字）。
>
> **D1 接手要点（仍然有效的部分，逐条沿用）**：
> (1) **CPython 本机可用：`python` = 3.14.6**（D-38 轮实测）。D1 验收的"声明 `py = "3.13"` 而本机是
>     3.14 → `E-0713`"今天就有真环境可测；`btrack.rs` 的 `find_python` / `run_python_probe` /
>     `CapTable` / `capability_scan` / `lower_named` / `ifbc::verify` 骨架都在文件里，缺的是**接线与
>     测试**。D4(1) 已经把 `btrack.rs` 的连动面改完（`Instance`→`Dyn`、`push_func` 的 `max_stack`、
>     guard/box.dyn 操作数、`Selected::Object` 按 `arity()` 构造），**不要**再按旧形状写。
> (2) **`src/btrack.rs` 不是占位，是 4476 行 / 75 个 `pub fn` / 0 项单测的截断模块**（`atrack.rs`、
>     `host.rs`、`doc.rs`、`conformance.rs` 才是 1 行占位）。截断模块的正确做法是**收敛而不是重写**：
>     先跑一次性探针打印 `@py` 片段进出的中间形态（探测 JSON → `CodeObj` → 能力扫描 → lowering
>     指令流 → `dump_canonical` 文本），再定位缺口。它上次失败撞到 agent 最大轮次（150）⇒
>     要么提高 maxTurns，要么拆成"探测+能力扫描"与"lowering+IFBC"两条增量。
> (3) **两处已知形状缺陷会在 D1 现形，且都是"预期的显式失败"，别当 IFBC 编码 bug 去绕**：
>     `new.set ty` 没有元素个数落点（**F25**）、`Selected::Object` 对多于 1 个操作数的映射是显式拒绝；
>     C 侧对象模型缺属性表 / 迭代器 / 集合载体 ⇒ `import.attr` / `inst.get` / `inst.set` / `seq.iter` /
>     `seq.next` / `new.set` 六条**装载通过而执行返回 R-0710**（**F27**，偏差 D-41 第 2 项）⇒ 带属性
>     访问或迭代的 `@py` 片段只能验到"装载成功"那一半。本侧装载期还拒绝 `call.virt` / `call.embed` /
>     `invoke.dynamic`（**F26**，D-42 的 (a)）。
> (4) **`.inf ifbc` 目前恒为空**（`src/artifact.rs` 写 0 个 B 轨模块）。D1 产出真字节码后 `infy build`
>     的段 blob 会变，C3 轮钉住的 `199975 字节` / `build-id 4d8250b5…55ba` / `blob 738` 三处数字会
>     一起动 —— 那是**预期变化**，要同步改 `docs/status.md`、`docs/coverage.md`、`README.md` 与本节，
>     别留旧声称。
> (5) **C5 留下的可复用面**（`src/cli.rs`）：`frontend(positional, rest, scope)` = 阶段 [1]-[5]，
>     `check` / `build` / `run` / `test` 共用；`load_scoped(root, handler, scope, triple)` =
>     §5.9+§2.2 的文件选择；`run_module(..., scope)` + `forward_program(out, label, note)` =
>     建到临时目录 + 执行 + 逐字节转发；`codegen::Config` 7 个字段（`test_harness` / `bench`），
>     **加开关同时给 `Default`**，`codegen::tests` 有 `emit_with(src, cfg)` 可复用。D2 的
>     `infy ifbc dump|verify` 应当复用 `frontend`，并且现在可以**直接复用**
>     `disassemble`（人类可读）与 `dump_canonical` / `verify_canonical`（可比对那档），别再起一套。
> (6) **"哪些文件参与构建"只有一个入口**：`pkg::BuildConfig` + `pkg::file_selected`。新维度往
>     `BuildConfig` 里加，别在 `cli` 里另写判据；**未知名 `E-0012` 必须在选择之前、对每个 discovered
>     文件做**（否则拼错的约束名把文件变成"从来没存在过"）。并且：任何"过滤 `Loaded::files`"的改动
>     都要顺着查以文件集合为输入的下游 —— §5.8 dirhash、§7.3 build-id、`infy mod tidy` 的 lock 内容
>     都必须与构建范围无关（`cli::package_dirhashes` 已改为从 `pkg::discover` 重取全树）。
> (7) **第一次执行 / 装载必然现形**（C4 的 D-34、C5 的 §5.1 R2 与 `read_dir` 顺序、D4(3) 的 `coro.*`
>     宽度与本侧 `verify` 重复报告，全是这么揪出来的）：`infy check` / `build` / `artifact info` 全绿
>     对"能不能装载 / 跑完"没有预测力，而**两侧各自单测全绿对"两侧读法是否相同"也没有预测力**。
>     D1 的第一条断言应当是"`infy_ifbc.c` 装载成功 + 六项校验过 + 退出码 + 双侧 canonical 文本相同"，
>     不是"反汇编看着对"。断言要写**次序与逐字节**，不要写 `contains` 的存在性（识别法：grep 测试里的
>     `contains(`；控制流 / 时序性质只查存在 = 会绿会骗人）。
> (8) **手法留档**：改了 `infy_ifbc.c` / `infy_rt.c`（哪怕只改 `.h`）都要复跑四条 C 链 ——
>     `-c infy_rt.c`（0 告警）、`-c infy_ifbc.c`（0 告警）、rt harness、替身链 IFBC、生产链 IFBC +
>     `cmp` 两份 stdout；rt / ifbc 的二进制**产在 `target/scratch/` 并从那里跑**（夹具按
>     `../../runtime/fixtures/…` 取，换目录 = 假失败）；编译 C 侧测试**四个 `.c` 一起给**
>     （少 `infy_dyn.c` 时 `ld` 只返回 `collect2: error: ld returned 5`）；跨语言一致性靠**逐字节夹具**
>     （artifact 那份 + IFBC 这三份）。**D4(3) 新增一类更强的手法：跨语言差分文本** —— 两侧各实现一份
>     "只含双方都有的字段"的渲染，比较的对象由**对方**生成并提交，比两侧各自单测强得多：本轮它第一次
>     跑就抓到一条线宽错误（`coro.*`）+ 一族命名错（D.9）+ 本侧一条重复报告缺陷。
> (9) **`infy test` 的 harness 输出在 Windows 上是 CRLF**（C 运行时文本模式流），期望串按
>     CRLF→LF 归一后逐字节比较 —— 与 §9.2 报告的既有手法一致；但 **IFBC 的规范化文本是 LF**
>     （两侧都按原始字节写、按原始字节比），别把两件事混为一谈（`.gitattributes` 那条就是为它加的）。
> (10) **BACKLOG 指针与文档的旧描述会过期**，本轮又实证一次：D.9 的 `.idx` 名同时写在 C 代码、
>     `runtime/IFBC.md` 和**本侧自己的镜像文档** `docs/internals/ifbc-encoding-rust.md` 里（同一个错
>     抄了两遍，而本侧代码本来就是对的）；`runtime/IFBC.md` §7 还声称两个私有入口"declared in
>     `infy_rt_internal.h`" —— `grep ifbc runtime/infy_rt_internal.h` 一个都没有，声明其实在
>     `test_ifbc.c` 的 `extern` 里。接手任何"看起来已实现 / 已记录"的条款前，先 grep 调用点与声明处。
> (11) **本轮新增的一条文档卫生教训（写进经验库）**：`core.autocrlf=true` 这台机器上，用脚本以文本
>     模式整文件重写 docs 会把 LF 变成 CRLF，git 于是把整个文件报成改动（本轮 `docs/BACKLOG.md`
>     一度 804 增 / 760 删）。收尾必须看 `git diff --numstat` 的删除列，并确认行尾；写文件要二进制模式
>     或用编辑工具，别用文本模式整篇重写。
>
> **推送欠账**：只认现算 `git rev-list --count <远端SHA>..HEAD`；远端 SHA 用
> `gh api repos/YuanShenNiuBi1314/infinity-lang/git/refs/heads/toolchain --jq .object.sha` 回查，
> `Everything up-to-date` 不算成功。**D4(3) 轮**推 `1f8988d` + 本轮做账提交，结果见 RUNLOG 的
> `cron-15-push` 行（本轮现算，别引用旧数）。
> C4 / C5 / D4(1) / D4(2) 都是第 1 次就成、C3 轮第 6/7 次、cron-12 三次全败 ⇒ `github.com:443` 仍是
> **间歇**不通而不是坏了：照协议先推一次、失败再试一次即停，不预先加变体、不循环轰炸；若连败，
> 首个候选是 `git -c http.version=HTTP/1.1 push origin HEAD:toolchain`。
>
> 已验证事实（**D4(3) 轮复跑，非引用**）：`cargo test --offline --lib` **426 passed / 0 failed**
> （`lexer` 79 · `derive` 57 · `fmt` 39 · `macro_expand` 35 · `pkg` 24 · `sema` 22 · `ifbc` **22**
> （+2 = 双侧文本锁点）· `codegen` 22 · `hir` 20 · `artifact` 18 · `cli` 16 · `toml` 15 · `borrow` 15 ·
> `sha256` 12 · `codes` 9 · `parser` 8 · `base64` 7 · `stdlib` 4 · `diag` 2 = 426）；
> `cargo check --offline --lib` 0 error / **50 warning**（未新增）；
> `infy check examples/tour` exit 0（仍 2 files）；`infy build examples/tour` exit 0（生成 C11 仍
> **12513 字节**、产物仍 **199975 字节**、`build-id 4d8250b5…55ba` 与 C3/C4/C5/D4(1)/D4(2) 同值
> ⇒ 本轮动的是 C 侧 IFBC 虚拟机与本侧校验器，阶段 [9] 只链 `infy_rt.c` + `infy_dyn.c`，产物字节不变）；
> `--verify-reproducible` exit 0（199975 == 199975）；`infy test examples/tour` exit 0（程序 stdout
> 42 字节 `test result: 0 passed, 0 failed, 0 total`）；`infy artifact info <产物> --nodes` exit 0
> （节点表 6 行 = 行 8/9/10/11/16/19）；`gcc -std=c11 -Wall -Wextra -I runtime -c 生成的 ci.c`
> exit 0（**0 error / 0 warning**）；`(cd examples/tour && infy fmt --check)` 仍 exit **2**
> （F6 未裁决，未变）；**C 侧四条链全部复跑**：`-c runtime/infy_rt.c` **0 告警** exit 0、
> `-c runtime/infy_ifbc.c` **0 告警** exit 0、rt harness **32335 assertions / 0 failures** exit 0
> （必须在 `target/scratch` 里跑）、替身链 IFBC **167/167** exit 0（基线 146，+21 = D4(3) 的三份夹具
> 断言）、生产链 IFBC **167/167** exit 0、两份 stdout `cmp` 无输出。
> **本轮未复跑、仍是 C5 轮实测**（不在本轮主张范围内）：`infy run examples/tour` 的 0/0 字节、
> 三套合成 `infy test` 的 exit 0/1/3 与逐字节报告、`--cov` exit 1、`--bench` exit 6、
> §5.9 / §2.2 的文件选择三条（同一 module 上 `infy check` = 1 file 而 `infy test` = 2 files；
> `//infy:build ignore` 不入选、`//infy:build test` 只在 test 构建入选、`//infy:build riscv` →
> `E-0012`）。C5 轮修掉 **2 个实现缺陷**（§5.1 R2 判据写反；`read_dir` 顺序漏进 `FileId` / 发射文本 /
> build-id），**挡住第 3 处同族风险**（`package_dirhashes` 取过滤后的 `Loaded::files`），并把
> **3 处死代码接活**（`pkg::constraint_matches`、`pkg::unknown_constraint_names`、
> `lexer::lex_file_with_features` 的替代路径）；裁为偏差 **D-37** 与 **D-36**。**D4(3) 轮另修
> 1 处本侧缺陷**：`verify_func` 对同一 `(code, off)` 重复报 2–3 次（操作数 / 邻接检查写在高度走查里，
> 而走查又把直线后继压回工作栈），修法是检查搬出走查 + 后继不再压栈，见 D-42。
> 未修只登记：D-18、D-19/F7、F8、F9/F10/F11、F12…F15、F16/F17/F18、F19/F20、F21/F22/F23/F24、
> F25 / F26（F26 的"把 verify 结论也比一遍"那半已做，剩"本侧按 C 的 δ 表实现那三条 + dynamic call
> 可证明性"那半）、**F27（C 侧对象模型缺属性表 / 迭代器 / 集合载体 ⇒ 六条指令装载通过而执行显式
> R-0710，偏差 D-41）**；`main` 与本地分叉（另一会话推了 `faa34ad` / `05da606`），
> **合并主干由人决定**。
> 优先级最高的已知地雷：**D1**（`src/btrack.rs` 端到端；C 侧现在读得懂 Rust 的字节且宽度已对齐，
> 执行那一半被 **F27** 挡）、**F26 的前半**（本侧装载期校验与 C 的 δ 表不等价 ⇒ "两侧校验等价"仍
> 不许讲，清单 = D-42 的 (a)–(e)）、**F12**（移动语义没有运行时入口，`String` / `Vec` 的真实程序
> 写不出来）、**F15**（`fmt!` 被阶段 [4] 挡住，§14.2 样例不能 `check`）、**F19**（没有任何 INFY
> 构造能写 stdout：`infy run` 的程序输出对现有一切实例恒为 0 字节）、**F21**（`std/test` 在 §10
> 一条没有 ⇒ §14.7 的测试样例过不了 `check`，`#[bench]` 无法表达）、**F5 / F6**（CLI 目录参数不一致、
> §14 源码不是格式化不动点）、**F7**、**F8**。F9 已在 C2 落地时按"只匹配 `Lit::Int`、`ExprKind::Int`
> 分支只作防御"绕过，死臂本身仍未清。
