# INFY v2.0 生产总纲（BACKLOG）

本文件是**唯一的任务真值源**，给人看也给定时任务读。它模仿 R 项目那套生产方式：

- 每一轮只做**一个自足的小增量**，且必须带着测试与可复现的验收命令；
- 贡献者留下比来时更绿的树——**绝不允许把树弄坏后离开**；
- 一切主张以命令输出为准，不写没有证据的进度（R 的 `R CMD check` 文化：没跑过 = 没做）；
- 失败是一等公民：踩坑写进经验库（见 §0.4），下一轮先读它，别在同一个坑里摔两次；
- **发布与合并主干永远由人签字**，自动化只推自己的分支。

## 0. 每一轮的固定协议（定时任务必须遵守）

> **§0.0 风险操作一律走脚本**（项目所有者的明确要求：直接敲命令会频繁弹人工确认，脚本不会）。
> 凡涉及删除/移动文件、清理缓存、`git` 历史相关操作、改本机状态、批量改文件，**不得逐条直接执行命令**，
> 必须先在 `scripts/` 下写成一个脚本再运行，脚本四条硬规矩：
> ① **默认干跑**（不带 `--apply` 只打印将做什么）；② 幂等，可重复运行；③ 把动作与结果追加到
> `logs/<name>.log`，失败时退出码非 0；④ 改本机状态的是临时的——必须 `trap`/`finally` 恢复并核验
> （见经验库 restore-system-changes）。
> **脚本只是免掉确认弹窗，不是扩大权限范围**：仍然禁止强推、禁止动 `main`、禁止删除仓库外的用户文件、
> 禁止 `rm -rf` 仓库与 `target/` 以外的路径。用脚本做这些事属于违规，不做比做错好。

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

- [x] D1 `src/btrack.rs` 收敛（子任务被截断）：CPython 探测 → 能力扫描 → lowering → IFBC。
      验收：`@py` 的 tour 片段产出可被 `infy_ifbc.c` 装载的模块；`eval` 触发 `E-0711`；未映射 opcode 触发 `E-0712` 并点名具体项；声明 `py = "3.13"` 而本机是 3.14 触发 `E-0713`。
      **D1(1) 轮（2026-10-15）= 两段增量的第一段（探测 + 能力扫描）已落地，D1 保持 `[ ]`**。
      按 §2 的建议没有重写，做法是**先补测试**：`btrack` **0 → 23 项**（每项 doc comment 指名条款）。
      验收四项里 **后三项已有实测锁点**（都是真 dump 上的扫描结果，不是手搓 code object）：
      `eval` ⇒ `E-0711 builtin(eval)`（`eval_is_e0711_and_not_a_capability_gap`，含"表的 `eval = "-"` 不把它降级成
      `E-0712`"这半条）；未映射 opcode ⇒ `E-0712` **点名到子操作** `BINARY_OP(**)` 与带号形式 `BINARY_OP (0x2c)`
      （两条测试分别走"表里是 `-`"和"表里根本没有这一行"）；声明 `py = "3.13"` 而本机 3.14.6 ⇒ `E-0713`
      （`a_declared_minor_that_is_not_the_interpreter_is_e0713`，成对断言"声明实际版本 ⇒ 放行"）。
      **测试一上手就红的 10 项里，三条是实现根因**（全部有回归锁点，裁定记偏差 **D-43**）：
      (1) `arg:` 派发行的分支值 `-` 被 `check_mnemonic` 判成非法助记符 ⇒ `BINARY_OP`/`CALL`/`COMPARE_OP`/
      `CONTAINS_OP`/`IS_OP`/`LOAD_ATTR`/`STORE_ATTR` **整行**解析失败、扫描按"整个 opcode 未映射"报 `E-0712`
      —— **任何含算术/比较/属性/调用的 `@py` 单元都过不了 §7.7.2，D1 被这一条挡死**（py-3.14 表 154 行里 7 行、
      py-3.13 表 14 行受影响）；(2) CPython 3.12+ 把标志位打包进 `co_names` 索引（`arg = namei*2 + flag`，
      本机实测 `input()` → `arg=1`、`x.foo`→0 / `x.bar`→2 / `x.bar()`→1），按原值取名字 ⇒ 只有一个全局名时
      **越界→扫描静默跳过**（`E-0711` 分支从不触发）、有两个名字时**解析成邻居**（`input` 被当 `eval`），
      修 = 新增 `CodeObj::operand_name()` 统一 `>> 1`，扫描项 2 与 lowering 四处共用；
      (3) `IMPORT_NAME` 的参数是 `co_names` 索引而 `const_name()` 从 `co_consts` 取 ⇒ **扫描项 3 从不触发**
      （`import numpy` 的 violations 是**空**，deny 段形同不存在），修 = 删 `const_name` 统一走 `operand_name`。
      顺带把 `CapTable::load` 里重算 `table_hash` 的裸换行字面量改成 `"\n"` 转义（这台机 `core.autocrlf=true`，
      裸换行会随检出变成 `\r\n` 让 hash 对不上），`each_table_hash_matches_the_body_it_claims_to_cover` 钉住。
      **新登记 F28**：三份 shipped 能力表里只有 `IFBC-CAP-py-3.14.toml` 有 E.3 的数值键，3.13 与 lua 表一个都没有
      ⇒ `check_alias_agreement` 对这两张表永不触发；本机无 3.13/lua 解释器，数值不凭记忆填。
      证据：`cargo test --offline --lib` **449 passed / 0 failed**（基线 426，+23 全在 `btrack`）、
      `cargo check --offline --lib` 0 error / **50 warning**（未新增）、`infy check|build|test examples/tour` 全
      exit 0 且 `--verify-reproducible` 0（199975 == 199975）、生成 C11 仍 12513 字节、`fmt --check` 仍 exit 2（F6）、
      **C 侧零改动仍复跑四条链**：`-c infy_rt.c` / `-c infy_ifbc.c` 各 0 告警、rt **32335/0**、
      替身链与生产链 IFBC 各 **167/167** 且两份 stdout `cmp` 无差异。
      **余项（下一轮的起点，两条都不需要重新审计）**：
      (a) **阶段 [6a] 没有任何 CLI 调用点** —— 全仓对 `btrack` 的引用只有 `artifact.rs` 的 `crc32` 与本模块的测试，
      `frontend()`/`build_c()` 从不跑摄取，`@py` 单元到阶段 [8] 仍是 `codegen.rs:2334` 的显式拒绝。
      接线点在 `cli.rs::frontend` 之后、`hir::lower` 之前（§7.1 的 [6a] 就在那儿），入口已备好 =
      `btrack::build_embed_unit(body, params, unit_name, manifest, python, hosts, file, pos, handler)`；
      要现造的是 `HostSigTable` 里该单元那一行（§7.7.1 规则 1：参数表来自 **INFY 侧**声明的类型 ⇒ 需要
      `sema::Ty` → `ifbc::TyKind` 的映射，`hir::hir_ty_from_ast` 是最接近的现成件）。
      (b) **产出的镜像还没被 C 侧 `infy_ifbc.c` 装载过** —— 做法 = 新增 `runtime/fixtures/ifbc_py_unit.bin`
      挂进 D4(3) 的双向锁点（`.bin` 由 Rust 写入器生成、`.can.txt` 由 C 侧 `INFY_REGENERATE_CANONICAL=1` 渲染，
      两侧各断言同一份文件），并让 `test_ifbc.c` 用生产装载器读它 + 过六项校验；带属性访问/迭代的片段
      会被 **F27** 挡在执行那一半（装载通过而执行显式 `R-0710`），那是**预期的显式失败**，不要去绕。
      (c) `.inf ifbc` 恒空（`artifact.rs` 写 0 个 B 轨模块）；真接上之后 C3 轮钉的 199975 字节 /
      `build-id 4d8250b5…55ba` / `blob 738` 三处数字会一起动 —— 预期变化，须同步改 status/coverage/README 与本节。
      **D1(2) 轮（2026-10-15）= 余项 (a)(b) 落地，D1 验收四项全部有实测锁点 ⇒ 勾 `[x]`；(c) 登记为 F29**。
      (a) **阶段 [6a] 接进 CLI**：`cli.rs::frontend` 在 `analyse()` 之后、渲染诊断之前调 `ingest_b_track`
      （§7.1 的阶段注要求 `infy check` 也跑 [6a]，而 §7.1 又规定"任一阶段出现 `E-` 即中止后续阶段" ⇒
      只在 [1]-[5] 干净时才摄取）；新增 `btrack::EmbedSite` / `ingest_embed_site` / `embed_ty_kind`
      （§7.7.1 规则 1 的 `sema::Ty` → `ifbc::TyKind` 映射，容器类折到 `dyn`，`u128`/`auto` **拒绝而不回落**）、
      `cli.rs` 的 AST 采集（§3.6 的两个合法位置：整体 body 与语句表达式，含 Block/If/While/For/Match/Unsafe/
      Defer 的嵌套），镜像存在 `Loaded::embed_images`，`infy check` 每份打一行 `ingest ok —`（§13.4 未规定
      `check` 的输出格式，按本仓库"每个已落地阶段一行状态"的既有约定，见偏差 **D-44(g)**）。
      裁定与保留名集合等七处规范未定 = **D-44**；`E-0713` 同时承担"解释器不在场"（spec:1051 的字面）。
      **接线当场揪出两条实现缺陷**（都不是规范歧义，各带回归锁点）：
      (1) `build_embed_unit` 问探针要 `__infy_unit__`，而 `wrap_embed_unit` 写的是 `def <unit_name>` ⇒
      `unit_code()` 名字匹配不上就**静默回落到模块 code object**，扫的是包装器的 `MAKE_FUNCTION` 流，
      单元体里的 opcode/builtin/import 一个都不进扫描，产出的镜像也不是那个函数（D-44(d)，改 `run_python_probe_named`）；
      (2) `sema` 的 `ExprKind::Embed` 臂无条件返回 `Ty::Unit`，令 §7.7.1 自己的样例在阶段 [4] 吃
      `E-0110 返回值类型不匹配：期望 Vec<f32>，实际 ()` ⇒ **阶段 [6a] 从 CLI 永远到不了**（D-44(e)，
      改判为所在 item 的声明返回类型，§7.7.1 规则 2 的推论）。顺带 (3) 补丁级声明 `py = "3.14.6"` 时
      版本比较放行而能力表按原串去找 `IFBC-CAP-py-3.14.6.toml` → 假 `E-0712`（D-44(f)，改按 `dump.py_minor()`）。
      (b) **C 侧真装载了一份 `@py` 镜像**：新夹具 `runtime/fixtures/ifbc_py_unit.bin`（1062 字节，
      由 `ingest_embed_site` 从 §7.7.1 的 `normalize` 单元产出，非手写形状）+ `.can.txt`（1144 字节，
      **C 侧** `INFY_REGENERATE_CANONICAL=1` 渲染），`test_ifbc.c::test_py_unit_fixture` 用**生产装载器**读它并过
      §7.8.3 六项校验（`rc == 0`），再从规范化文本里 `sscanf` 回函数记录断言 **arity 2 / ret_sig 13(f64) /
      ret_arity 1** —— §7.7.1 规则 1 的声明表穿过探针、lowering、编码器，在虚拟机这一侧读回来还是那一张；
      另断言 `L` 行在场（§7.4 的原始语言行映射，本例 13 行全部映射到 Python 第 2 行）。
      Rust 侧 `ifbc::tests::canonical_render_equals_the_c_loader_render` 把这份夹具加进同一份文本的比对列表，
      `btrack::tests::py_unit_fixture_is_exactly_what_the_ingest_produces` 断言"重新生成 == 夹具"。
      **不声称执行**：B 轨帧的执行通道在阶段 [8]（F29），且 `import.attr`/`seq.iter` 那一半还被 **F27** 挡着。
      验收四项的第 1 项用的是 §7.7.1 单元**去掉推导式**的那一份体（`return scale * 2.0`）——
      原样推导式被能力表红着拒绝（`RERAISE` 在表里是 `-`，另有 `f32` 与无后缀字面量的混宽算术被 lowering
      自检拒），这是 §7.7.2 期望的行为而不是本条的失败，规范样例本身过不了扫描登记为 **F30**。
      证据：`cargo test --offline --lib` **460 passed / 0 failed**（基线 449，+7 全在 `btrack` +4 在 `cli` = 11）、
      `cargo check --offline --lib` 0 error / **50 warning**（未新增）、
      `infy check examples/tour` 仍 exit 0 且 stdout 逐字节未变（无 B 轨单元 ⇒ [6a] 一行不打，
      `cli::tests::a_module_without_embeds_produces_no_images` 钉住 P6）、`infy build` 仍 12513 字节 C11 /
      199975 字节产物 / `blob 738` / `build-id 4d8250b5…55ba`（阶段 [6a] 不参与 [9] 的输入，(c) 未做）、
      `--verify-reproducible` exit 0、`infy artifact info … --nodes` exit 0、`infy test examples/tour` exit 0、
      `(cd examples/tour && infy fmt --check)` 仍 exit **2**（F6 未裁决，未变）；
      **C 侧只改 `test_ifbc.c`**（`infy_ifbc.c`/`infy_rt.c`/头文件零改动）仍复跑四条链：`-c infy_rt.c` 与
      `-c infy_ifbc.c` 各 **0 告警**、rt harness **32335/0**、替身链与生产链 IFBC 各 **180/180**
      （基线 167，+13 = 新夹具装载那组 8 项 + 第四份双侧文本 5 项）且两份 stdout `cmp` **无差异**。
- [x] D2 `infy ifbc dump|verify` 接线（Rust 侧编码 ↔ C 侧虚拟机**交叉验证**：同一镜像两边读出的反汇编一致）。
      验收：命令层产出同一份文本 ⇒ `infy ifbc dump --canonical runtime/fixtures/ifbc_{rust_writer,all_ops,py_unit}.bin`
      三份输出与 **C 侧**渲染的 `.can.txt`（983/3978/1144 字节）按原始字节 `cmp` **无差异**，exit 0；
      `infy ifbc verify --canonical …ifbc_verify_matrix.bin` 的 10 行 `V …` 与 C 侧 `.ver.txt` 同样逐字节相同。
      D4(3) 轮把这份相等锁在 C harness 与 `ifbc::tests` 里，本轮第一次让**命令行**成为比对的那一层
      （P22/P23/P24，`docs/coverage.md` §1）。落地面只有 `src/cli.rs`：`classify_ifbc`（先按 §7.4 容器读、
      再按裸镜像读）+ `ifbc_dump_text` / `ifbc_verify_image` / `ifbc_verify_canonical_text` /
      `ifbc_empty_region_text` / `ifbc_cmd` + 派发与 `usage()`，`ifbc` 从"未实现"臂里摘掉；
      没有再造第二套渲染器（`disassemble` / `dump_canonical` / `verify_canonical` / `verify` 全是现成件）。
      实测码：`--all-errors` 报 **10** 条阻断（`R-0704 ×2 / R-0705 / R-0706 ×3 / R-0707 ×2 / R-0710 / R-0713`）、
      默认报 **1** 条（D.10"只报首个，与运行时一致"）⇒ 两者都 exit **2**；`ifbc_rust_writer.bin` 与
      `ifbc_py_unit.bin`（[6a] 真产物）各 **0** 条 ⇒ exit 0；`README.md`（既无段尾又无 `IFB1`）→ `E-0605` + exit **7**
      且两条拒绝理由都打；不认识的子命令/缺文件/读不到 → exit **1**。
      规范未定 → 选定实现并记偏差 **D-45 (a)–(e)**：输入形态（`.inf ifbc` 只接受恰好 1 个模块，
      `n_ifbc_modules != 1` **点名拒绝**，不猜一种 §7.4 没定义的分帧）、`--canonical` = D4(3 那份文本且
      此时 stdout 必须纯（表头/状态行走 stderr）、拒绝的退出码归属、host 表取 `HostSigTable::std_default()`
      （`src/host.rs` 是 1 行占位 ⇒ 校验的是"镜像对 std 表"，不是"镜像对该产物注册的 host 表"）、
      "首个"裁成首个**失败函数**。
      **两条开工前就接受的前提，没有绕**：① 真实产物的 `.inf ifbc` 恒空（**F29**）⇒ `infy ifbc dump|verify
      examples/tour/.infy-build/…/ci.exe` 明打 `0 个 B 轨模块 / 区域 0 字节` + 点名 F29 与"不声称校验过
      产物里的 B 轨字节码"（这段**文字**由 `cli::tests::an_artifact_with_an_empty_ifbc_region_is_reported_as_empty`
      钉住，因为空区最容易被读成"查过了没问题"）；容器路径本身仍被钉可用 =
      `a_one_module_artifact_is_read_from_the_ifbc_region`（测试用 `artifact::write` 造一个 `.inf ifbc` 恰好 1 个模块的
      容器，读回字节 == 写入字节、`--canonical` == 同一份 `.can.txt`），多模块 → `several_modules_in_one_region_are_refused_by_name`。
      ② 两侧装载期校验**不等价**（**F26 前半**，清单 = D-42 (a)–(e)）⇒ 命令的输出里直接把这句话打成 note，
      并且实测就是如此：`ifbc_all_ops.bin` 本侧报 **26** 条而 C 侧不报同一集合（该夹具双侧只比过反汇编）。
      证据：`cargo test --offline --lib` **468 passed / 0 failed**（基线 460，+8 全在 `cli`：20 → **28**）、
      `cargo check --offline --lib` 0 error / **50 warning**（未新增）、`infy check|build|run|test examples/tour` 全
      exit 0 且 `--verify-reproducible` 0、生成 C11 仍 **12513** 字节 / 产物仍 **199975** 字节 /
      `blob 738` / `build-id 4d8250b5…55ba`（本轮没碰 [7]-[9]）、`infy artifact info … --nodes` 0、
      生成 C 的 `gcc -c` 0 error / 0 warning、`fmt --check` 仍 exit **2**（F6 未裁决）；
      **C 侧零改动**（`.c`/`.h` 一行没动）仍复跑四条链：`-c infy_rt.c` 与 `-c infy_ifbc.c` 各 **0 告警**、
      rt harness **32335/0**、替身链与生产链 IFBC 各 **180/180** 且两份 stdout `cmp` **无差异**。
      证据提交：**`d6c26a1`**（`feat(D2)`）—— 上述 468/0、0/50、三份 `.can.txt` 与一份 `.ver.txt` 的 CLI 级
      `cmp`、1/10 条阻断与 exit 2、12513/199975/`4d8250b5…55ba` 未变、四条 C 链（32335/0、180/180×2、
      `-c` 各 0 告警、两份 stdout `cmp`）全部为该提交上的实测复跑，非引用；本节的做账行与 RUNLOG 的
      cron-18 行在其后的提交里。
- [x] D3 `src/atrack.rs`：`#[link]` + `@c` 配对、`infy_binding.h` 生成、`infy_e_<name>_<8hex>` 包装、缺 `unsafe` 报 `E-0721`、缺 `#[owns]/#[borrows]` 报 `E-0723`。
      验收：一个最小 `@c` 单元被 gcc 编译、链接、在产物里被调通。
      **D3 轮（2026-10-16）落地并勾 `[x]`**（`atrack.rs` 从 1 行占位新建 = 绑定的唯一真值源，**27 项**单测）。
      (1) **配对 §7.6.1 两个方向**：`atrack::signature_blocks` / `atrack_sources` / `pair` —— `sema` 一直只报
          "`@c` 找不到签名块"那一半，本轮补上 "`#[link]` 落空" 与 ABI/前缀支持面（`extern "C"` 之外的
          `"system"`/`"infy-unwind"` 点名拒绝；`@cpp`/`@rust`/`@zig` 因 §7.6.2 没有消费者而拒绝，见 **F32**）。
      (2) **`E-0723` 第一次有发点**（开工前 `grep -rn "E-0723" src/ | grep -v codes.rs` 为空）：UB-010 的移交判据
          四态（缺属性 / `#[owns]`+`#[borrows]` 并存 / `#[borrows]` 无生命周期 / `()` 不要求）+ §7.6.4 末条的
          禁止传递（`Rc<T>`、`Vec`/`String` 的容量与裸内存）。**判据只落返回方向** —— 规范样例的
          `dec_open(path: *const u8, …)` 与 `dec_close(h: *mut DecHandle)` 形参都是裸指针而没有属性，
          若"参数也算"则规范自犯 `E-0723`（偏差 **D-46(c)**）。
      (3) **§7.6.3 项 1/2/4 的产物**：`infy_binding.h`（反向生成的用户符号声明 + 包装原型）与
          `a_track/<name>.c`（用户源码逐字节 + `infy_e_<name>_<8hex>` 本体）由 `cli::a_track_units` 写进
          **每个**构建目录（`--verify-reproducible` 跑两个目录 ⇒ 写文件必须在 `stage9` 里面），
          `stage9` 的 `cc` 命令加 `-I <work>/a_track` 与逐单元 `.c`，顺序按单元名字典序（§7.4 + §7.3）。
          哈希 = 规范化签名块 SHA-256 前 4 字节；规范化裁定 = 块名 + 成员按 `(package, name)` 字典序
          （声明序会随文件系统变 ⇒ §7.3 的承诺断掉），**D-46(a)/(b)**；`hir::HirFn::signature_text`
          此前**零调用点**，现在是它的输入。
      (4) **发射面接上**：`codegen::emit_foreign_decl` 发 `extern <原型> <wrapper>;`（键是包装符号，不是那个
          从没人定义的 INFY 符号），`user_fn_call` 改调 `wrapper.call(argv)`，`()` 返回走
          `((void)w(a), (InfyUnit){0})`；`StmtKind::Expr` 里的 A 轨 `@c` 语句不发射任何东西（C 原文活在
          `a_track/` 里）。§7.6.4 里**规范允许但还没发射形态**的五类（ptr+len 展开 / null→`Option` /
          `Result` 出参 / 回调 ctx / struct 按值）以条款号 `§7.6.4` 指名 `CgError` 而**不发码** ——
          拿"程序不合规"的码去报一条规范允许的签名 = 假称不合规，且会写进 §12 的通过率（**D-46(e)**）。
      **端到端实测**：最小 `@c` 单元（`#[link(name = "decoder", static = true, toolchain = "cc")]` +
      `extern "C" { fn dec_mark(v: u32) -> u32; }` + `@c(name = "decoder")` 的 C 定义 + 链节点里
      `unsafe { dec_mark(41u32) }`）→ `infy run` **exit 0**，程序 stdout 逐字节 = `dec_mark(41) -> 42`
      （观察通道是 C 侧 `printf`：INFY 没有任何构造能写 stdout = F19，非空输出必须来自 A 轨的 C）；
      `infy build` 多打 `a_track ok — 阶段 [8] 绑定：1 个单元（decoder）、1 个包装符号、475 字节写入 …\a_track`。
      **头 / 包装本体 / `ci.c` 三处符号逐字符一致**被 `cli::tests::the_build_writes_the_binding_header_and_the_wrapper_unit`
      钉住（不一致只会以链接错误现形，两侧各自的单测都看不见）；`atrack::tests::the_generated_header_and_wrapper_compile`
      用 `gcc -std=c11 -Wall -Wextra -c` 真编译生成的头 + 假实现 ⇒ **0 error / 0 warning**。
      **接线当场揪出三条真实缺陷**（都不是规范歧义，各带回归锁点，细节见 `docs/status.md` 的 D3 一节）：
      (a) `parser` 的两处"块状表达式"判定漏了 `Embed` ⇒ **规范自己的 A 轨样例**（§7.6.1/§14 不带 `;`）报
          `E-0053`，span 还落在下一条语句上（`an_embed_block_statement_needs_no_semicolon`，偏差 **D-46(f)**）；
      (b) `codegen` 把块状尾表达式的结果临时量声明在**内层**作用域 ⇒ `return bres_1;` 引用已出视线的变量，
          真链接时 gcc 报 `'bres_1' undeclared`（旁边的 `ExprKind::If` 分支一直是对的，这一臂顺序写反；
          `a_block_tail_expression_declares_its_result_in_the_function_scope`）；
      (c) `E-0721` 的原判据是 `link_names().iter().any(|_| true)` 的占位 —— 与"被调者是不是 A 轨"无关，
          改成被调者自己的 `is_foreign`（`an_unsafe_less_a_track_call_is_e0721`）。
      同时删掉恒假的 `sema::in_extern_block`（它只在"顶层 `;` 体"这条路上被读，答案本来就是 `false`；
      留一个常量假判据是下一轮误判的来源）。
      回归：`cargo test --offline --lib` **505 passed / 0 failed**（基线 468，+37 = `atrack` 27 + `cli` +5 +
      `codegen` +4 + `parser` +1），`cargo check --offline --lib` 0 error / **50 warning**（改动前后告警**集合**
      逐条 diff 相同，且同一棵树 fresh 检出同为 50 ⇒ 未新增），`infy check|build|run|test examples/tour` 全
      exit 0 且 `--verify-reproducible` 0、生成 C11 仍 **12513** 字节 / 产物仍 **199975** 字节 / `blob 738` /
      `build-id 4d8250b5…55ba` **逐字节不变**（tour 无 A 轨源 ⇒ `a_track/` 不创建、链接输入一字不变，
      正是开工前定的判据）、`infy artifact info … --nodes` 0、生成 C 的 `gcc -c` 0/0、
      `(cd examples/tour && infy fmt --check)` 仍 exit **2**（F6 未裁决）；**C 侧零改动**仍复跑四条链：
      `-c infy_rt.c` 与 `-c infy_ifbc.c` 各 **0 告警**、rt **32335/0**、替身链与生产链 IFBC 各 **180/180**
      且两份 stdout `cmp` **无差异**。
      **明确未做**（登记，不假成功）：**F31**（§7.6.4 行 7 的 `#[repr(C)]` struct / ADT 不透明指针没有跨边界落点
      ⇒ §7.6.1 样例自己的 `-> *mut DecHandle` 过不了，规范样例不能构建，与 F30 同族）、
      **F32**（§7.6.2 工具链发现/注册、`recipe.toml` 审计、版本核对 `E-0604`/`--allow-toolchain-drift` 全未接）、
      **F33**（§7.6.3 项 3 的非标量 thunk 与项 5 的调试映射、§7.6.1 的 `W-0720` 与"声明了但未导出 →
      链接期 `E-0604`"映射、§7.6.5 反向导出与 `E-0724`、§7.6.6 `isolation = "process"`）。
      证据提交：**`e669834`**（`feat(D3)`）—— 上述 505/0、0/50、端到端 exit 0 + `dec_mark(41) -> 42`、
      12513/199975/`4d8250b5…55ba` 未变、四条 C 链（32335/0、180/180×2、`-c` 各 0 告警、stdout `cmp`）
      全部为该提交上的实测复跑，非引用；本节的做账行与 RUNLOG 的 cron-19 行在其后的提交里。
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

- [x] E1 `conformance/cases/**`：按 §12.1/§12.2/§12.5 的分类建目录与用例文件格式（源文件 + 期望码），先从已有的单元测试里"搬运"成外部用例。
      **E1 轮（2026-10-16）落地并勾 `[x]`**：`src/conformance.rs` 从 1 行占位落成 §12 的**分类表 + 用例格式 + 运行器**
      （**11 项**单测），`conformance/cases/**` 落成 **83 条外部用例 / 17 个分类**（77 Pass、6 Blocked、**0 Fail**）。
      (1) **分类表 = 规范三张表逐字抄**（20 行：§12.1 的 9 行 1244 + §12.2 的 3 行 256 + §12.5 的 8 行 800），
          `the_category_table_adds_up_to_the_specs_own_totals` 钉住 1244/256/800/2300 与行数；
          `CATEGORY.toml` 是这张表的**渲染产物**（`INFY_REGENERATE_CATEGORIES=1` 才重写），
          `verify_category_file` 锁另一方向；加载器同时拒绝**未知分类目录**与**缺失分类目录**
          （分类名拼错会让整目录用例静默消失 = §12.5 要防的那种谎报）。
      (2) **用例格式**（规范只给了分类/数量/检查列，从未定义一个用例长什么样 ⇒ 偏差 **D-47 (a)–(h)**）：
          一个用例 = 一个目录 = `case.toml`（`id`/`category`/`clause`/`title`/`kind`/`expect`/`exit` + 可选
          `file`/`image`/`requires`/`defect`）+ 逐字节的源文件。**不**采用"期望写进被测源码注释里"那一流派
          （§12.1 有"注释"分类、§12.5 的 §9.5 分类检查"注释保留"，期望若以注释承载就自指）。
          `kind` 三档 = `check` / `fmt-idempotent` / `ifbc-verify`；§12.3/§12.4 的硬校验不进 `cases/**`（命令行为）。
      (3) **运行器走 `infy check` 自己那条链**：新落成的 `cli::check_verdict`（`load_scoped` + `analyse` +
          干净时的 `ingest_b_track`）是唯一入口，比较面 = 诊断码的排序多重集 + §13.4 退出码。
          码表外的期望码由**加载器**拒（`codes::lookup`）；缺构建期工具 = **Blocked**（§7.7.3 的前提，
          记通过是谎报、记失败是把环境说成实现）；`defect = "Fxx"` 的用例挂的仍是**规范**期望，
          不达 → Blocked，**一旦通过 → 测试变红并要求删标记**（禁止"把缺陷当通过"）。
          B 轨用例的清单写 `@PY_MINOR@`，由运行器换成**本机探针的 minor** —— 否则同一份提交的用例在测机器。
      (4) **搬运来源** = 已有的 505 项单测的源码片段与 doc comment 条款（`lexer`/`parser`/`sema`/`borrow`/`pkg`/
          `derive`/`cli`/`atrack`），落成外部用例后比较面变成"诊断码全集"，于是揪出下面五条
          （原单测多是 `contains` 式断言，多出来的码以前没人看）。
      **当场揪出并登记的缺口**：**F34**（§10 的 std 表里没有 `Vec::clone` ⇒ §4.7 的"clone 后再用"正例不可表达，
      实测 `E-0110 类型 Vec<u8> 没有方法 clone` ×3）、**F35**（**任何 `#[derive(...)]` 都过不了 `infy check`**：
      生成体调 `std::fmt::format_struct`/`format_unit`/`format_tuple`（`derive.rs:1165/1176`）而 §10 的 std 表里没有，
      实测 `E-0053 缺少返回值` + `E-0101 调用未声明的函数` + `E-0110 无法解析字段`；改 package 目录 / 加
      `import "std/fmt"` / 补 `[[bin]]`/`[[lib]]` 都不变。`derive.rs` 的 57 项断言的全是生成物**文本**，
      生成体从未被阶段 [2]-[4] 回读，而 `examples/tour` 根本没有 `derive` ⇒ "derive 已通"从来没有端到端证据）、
      **F36**（同一根因两阶段各报一次：`E-0020` ×2 且标题不同、`E-0410` ×2）、**F37**（未知 `@` 前缀报
      `E-0702` 之后级联 8 条 `E-0053`）、**F38**（§1.3 领域中立的 `pkg::domain_neutrality_violations` 有单测而
      **零 CLI 调用点**，规范点名的 `std/domain_terms.txt` 不在仓库 ⇒ 该分类结构性无用例）。
      三个空分类逐条记名而不是删目录：`1.3-domain-neutral`（F38）、`10.9-std`（断言在运行期而程序没有
      stdout 通道 = F19）、`11.1-host-protocol`（`src/host.rs` 占位）。
      **一处假红提前挡掉**：`.gitattributes` 新增 `conformance/cases/** -text` —— 这台机 `core.autocrlf=true`，
      检出会把源文件变成 CRLF，而 §2.1 的 `W-0001` **就把行尾当被测面** ⇒ 每条用例多一条诊断、
      故意写 CRLF 的正反例整个失效（与 D4(3) 轮 `runtime/fixtures` 那条同族）。
      回归全部复跑：`cargo test --offline --lib` **516 passed / 0 failed**（基线 505，+11 全在 `conformance`）、
      `cargo check --offline --lib` 0 error / **50 warning**（未新增）、`infy check|build|run|test examples/tour` 全
      exit 0 且 `--verify-reproducible` 0、生成 C11 仍 **12513** 字节 / 产物仍 **199975** 字节 / `blob 738` /
      `build-id 4d8250b5…55ba` **逐字节不变**（本轮没碰 [7]-[9]）、`infy artifact info … --nodes` 0、
      `fmt --check` 仍 exit **2**（F6 未裁决）；**C 侧零改动**仍复跑四条链：`-c infy_rt.c` 与 `-c infy_ifbc.c`
      各 **0 告警**、rt **32335/0**、替身链与生产链 IFBC 各 **180/180** 且两份 stdout `cmp` **无差异**。
      证据提交：**`75e9fc1`**（`feat(E1)`，代码 + 用例集 + 做账同一次提交）—— 上述 516/0、0/50、
      83 条用例的 77/6/0、四条 C 链（32335/0、180/180×2、`-c` 各 0 告警、stdout `cmp`）与 tour 的
      12513/199975/`4d8250b5…55ba` 全部为该提交上的实测复跑，非引用；RUNLOG 的 cron-20 行在同一提交里。
      **推送**：本轮开头第 1 次即清掉 cron-19 欠的 3 个提交（`1bc2206..ea76ab9`），`75e9fc1` 本身也第 1 次
      推上（`ea76ab9..75e9fc1`），`gh api` 回查远端 = 本地 HEAD ⇒ 现算欠账 **0**（本节的做账行提交后又 = 1，
      下一轮仍只认现算）。
- [x] E2 `infy conformance --report`：**逐分类通过率**（只报总数视为不合规，§12.5）。
      **E2 轮（2026-10-16）落地并勾 `[x]`**：`src/cli.rs` 新增 `conformance_cmd` / `conformance_report_text` /
      `conformance_rate` / `conformance_exit` / `conformance_update_golden_refused`（**+5 项 `cli` 测试**：33 → **38**），
      `conformance` 从"未实现"臂摘出。**数据面一行没动**（E1 的 `conformance::{CATEGORIES, load_cases, run_case, tally}`
      就是全部输入，运行器仍只有 `cli::check_verdict` 一条路径 = D-47(h)），所以命令行打的三档数字与 `conformance::tests`
      在测试里跑的**逐位相同** —— 这不是巧合而是要求（两处若各算一份，报告与测试就会在同一个缺口上给出两个答案）。
      **验收实测**：`infy conformance --report` → 20 行逐分类（§12.1 的 9 + §12.2 的 3 + §12.5 的 8）+ 三档小计 + 合计 =
      `83 条用例 · Pass 77 · Fail 0 · Blocked 6 · 通过率 92.8% · 规范目标 2300（覆盖 83，缺口 2217）`，exit **1**。
      三条防谎报的形状：(1) **空分类打 `n/a`** —— `1.3-domain-neutral`（缺 52）/ `10.9-std`（84）/ `11.1-host-protocol`（60）
      一条用例都没有，0/0 若渲染成 100%（或 0%）就把"这一行 §12 完全没被检查"读成"这一行有结论"（同 D2 轮
      `ifbc_empty_region_text` 那一族：措辞本身就是断言对象，所以 `conformance_report_text` 做成函数并被逐子串断言）；
      (2) **`Blocked` 进分母、不进分子**，通过率 = `Pass / 用例数`；若分母只算"判完的"，1 条通过 + 5 条阻塞 = 100%，
      正是 §12.5 要防的形状的另一半；6 条 Blocked 逐条打 `defect=F34 / F35 / F36 / F37` 与「期望 … / 实得 …」首行；
      (3) **`0` 不发给不完整的套件**：§13.4 从未给 `conformance` 分配退出码，裁 `0` 要求 20 个分类都达到 §12 的
      `required` 且 `Fail=0`、`Blocked=0`；有 `Fail` → `2`；其余 → `1`。这一条用**合成数字**单独钉
      （`conformance_exit_refuses_zero_for_anything_short_of_the_whole_suite`），否则用例涨到 2300 时没人记得"0 曾经有条件"。
      规范未定 → 选定实现 = **偏差 D-48 (a)–(g)**：(a) 退出码归属、(b) `Fail` 优先于 `Blocked`、(c) 分母与 `n/a`、
      (d) `--category` 过滤运行不构成结论（未运行的分类打 `-`/「未运行」而不是 0；指向 0 条用例的分类直接**报错**而不是打空表）、
      (e) `--update-golden` **两个分支都拒绝**（无授权 = §12.4 的授权错误；有授权 = 本实现 `cases/**` 里没有黄金文件可更新，
      §12 又明令已有用例的期望不得改变，而 §12.3 的黄金文本由 `runtime/fixtures` 两侧写入器生成 = D-45(b)，真接还需要 F29）、
      (f) §13.4 未列的开关（`--json`）拒绝而不静默忽略（同 D-35(c)/F19(b)）、(g) 默认用例根按 `CARGO_MANIFEST_DIR` 解析、
      `[tests/]` 可覆盖，`--report` 接受但不改变行为。
      顺带把拒绝臂的名单抽成 `cli::NOT_IMPLEMENTED` 常量：拒绝臂与已接线的命令**可以共享 exit 1**，只有名单本身能让
      "命令又躺回拒绝臂"变红（D1(2)/D2 那两条"上游全绿对 CLI 能不能走到它没有预测力"的第三次应用）。
      实测退出码逐条：`--report` = **1**（6 条 Blocked）、`--category 7.8-ifbc` = **1**（只跑 2 条、19 个分类未运行）、
      `--category 2-lexicali` / `--category`（缺值）/ `--category --report` / `--json` / 两个位置参数 / 不存在的用例根 /
      `--update-golden`（± `--i-know-this-is-breaking`）= **1** 且每条点名拒绝理由。
      回归全部复跑：`cargo test --offline --lib` **521 passed / 0 failed**（基线 516，+5 全在 `cli`）、
      `cargo check --offline --lib` 0 error / **50 warning**（未新增：改动前后的告警**集合**逐条 diff 相同，且 HEAD 用
      `git archive` 解到 `target/scratch/basecheck` 全量重检同为 50）、`infy check|run|test examples/tour` 全 exit 0、
      `infy build examples/tour` exit 0 且生成 C11 仍 **12513** 字节 / 产物仍 **199975** 字节 / `blob 738` /
      `build-id 4d8250b58122ca2b76132029955e2d2005156dc0a6b021bf5f4ae761e5fd55ba` **逐字节不变**（本轮没碰 [7]-[9]）、
      `--verify-reproducible` exit 0、`infy artifact info … --nodes` exit 0、生成 C 的 `gcc -std=c11 -Wall -Wextra -I runtime -c`
      exit 0（**0 error / 0 warning**）、`(cd examples/tour && infy fmt --check)` 仍 exit **2**（F6 未裁决，未变）；
      **C 侧零改动**（`.c`/`.h` 一行没动）仍复跑四条链：`gcc -c runtime/infy_rt.c` 与 `-c runtime/infy_ifbc.c` 各 **0 告警**
      exit 0、rt harness **32335 assertions / 0 failures** rc **0**（在 `target/scratch` 里跑）、替身链与生产链 IFBC 各
      **180 asserts / 180 pass / 0 FAIL** rc **0** 且两份 stdout `cmp` **无差异**。
      **明确未做**（不用假成功掩盖）：§12 的**对外通过率表**（落进 `docs/coverage.md` 与 README + 显式列"距 100% 还差哪些类"）
      = **E3**；§12.3 的黄金文件更新（`--update-golden` 现两分支都拒绝）；§12.4 的等价性矩阵（wheel ≡ 源码、`-O0` ≡ `-O3`、
      `lto=false` ≡ `fat`、A 轨同进程 ≡ `isolation="process"`）一档都跑不了，因为 F12/F19/F29/F31/F32 仍开着；
      三个空分类的成因仍是 F38 / F19 / `src/host.rs` 占位；6 条 Blocked 的期望挂的是**规范**期望而不达 = F34/F35/F36/F37，
      **没有**为了让报告变绿改过任何一条期望。**本轮没有修掉任何实现缺陷**（`conformance::*`、前端、C 侧行为一字未动）。
      证据提交：**`d3f5a80`**（`feat(E2)`）—— 上述 521/0、0/50、83 条的 77/0/6、exit 1、`--category` 与九种误用的退出码、
      12513/199975/`4d8250b5…55ba` 未变、`gcc -c` 0/0、四条 C 链（32335/0、180/180×2、`-c` 各 0 告警、stdout `cmp`）
      全部为该提交上的实测复跑，非引用；本节的做账行与 RUNLOG 的 cron-21 行在其后的提交里。
- [x] E3 把通过率表落进 `docs/coverage.md`，并在 README 明确"距 100% 还差哪些类"。**在 100% 之前，任何文档不得出现"INFY v2.0 兼容"字样。**
      **E3 轮（2026-10-16）落地并勾 `[x]`**：表格落进 `docs/coverage.md` 新增的 **§6**（20 行 × 11 列 =
      slug / §12 表 / 分类名原文 / 要求 / 已有 / Pass / Fail / Blocked / 通过率 / 缺口 / 非满分的原因，
      + 三档小计 + 合计 + 缺口三分类那一行 + §6.1 逐类读法 + §6.2 用例格式表达不了的那一面 + §6.3 红线），
      README 新增「**距 §12 的 100% 还差哪些类**」一节，`conformance/README.md` 改为指向 §6 并说明它自己那张表
      **刻意不复制通过率**（同一份数字在两处各算一遍 = 同一个缺口两个答案，D-47(h) 的同一族）。
      (1) **数字全部从命令来**：`infy conformance --report` 是数据源（P29），本轮**没加、没改任何一条用例或期望**，
          合计行仍是 `83 条用例 · Pass 77 · Fail 0 · Blocked 6 · 通过率 92.8% · 规范目标 2300（覆盖 83，缺口 2217）` ⇒ exit **1**。
      (2) **副本配了逐格回核**（`conformance` **11 → 14**）：`the_pass_rate_table_in_coverage_md_matches_the_corpus`
          要求行的存在/数量/顺序等于 `CATEGORIES`、前 10 列等于 `conformance::tally`、通过率用 `cli::conformance_rate`
          这**一个**格式化函数（为此改 `pub(crate)`）、"非满分的原因"列点名的 `Fxx` 必须与该分类实际挂着的 `defect`
          集合**相等**（少写、多写、写错都红）、空分类行以"结构性空分类"开头、小计/合计/缺口行按整行字符串比对。
      (3) **"距 100% 还差哪些类"分三类点名**（把 Blocked 写成待补用例、把空分类写成不适用都是谎报）：
          覆盖缺口 = **20/20 行** `short` > 0、合计 2217 条；结构性空分类 = 3 行 / 196 条
          （`1.3-domain-neutral` = **F38**、`10.9-std` = **F19**、`11.1-host-protocol` = `src/host.rs` 占位）；
          Blocked = 6 条（**F34** ×1、**F35** ×2、**F36** ×2、**F37** ×1）。另列第四类：§12.3 黄金文件
          （`--update-golden` 两分支仍拒 = D-48(e)）与 §12.4 等价性矩阵四档（一档都跑不了 = F12/F19/F29/F31/F32）。
          **先划读法线：通过率 ≠ 合规率** —— 92.8% 是"已写的 83 条过了 77 条"，§12.4 的 100% 是"2300 条全过 + 黄金 + 矩阵"，
          这就是 D-48(a) 把 `0` 裁成"20 个分类都达到 `required` 且 Fail=0、Blocked=0"的理由。
      (4) **红线从约定变成机检**：`no_markdown_doc_claims_the_compatibility_name` 扫全仓 `.md`（排除 `spec/` 那份规范原文），
          任何一行出现那句声称而**没有否定标记** ⇒ 红；出现在禁令的引用里是允许的（BACKLOG 的任务行、RUNLOG 的历史行、
          各文档的红线句都可搜索，要挡的是肯定句）。`the_front_facing_docs_restate_the_refusal` 反过来要求
          README / `docs/coverage.md` / `conformance/README.md` / `docs/status.md` **各自**都还写着那句禁令 ——
          否则"把红线删干净"也能通过上一条。**三条测试都验过能变红**（改 `9.5-fmt` 行的数字 / 加一句肯定式声称 /
          删掉红线句 ⇒ 三条各自 FAILED，随后逐字恢复）。
      (5) **顺带修掉两处过期声称**（同族认法 = 经验库 `infy-doc-claims-vs-evidence`）：`docs/status.md` 的"仍未实现"
          挂了三轮的"目前外部用例为 **0**，逐分类通过率不可测"（E1 就有 83 条）改成 83 条 / 3.6% 并写明 E1/E2/E3 各自落了什么；
          本节旧指针里"覆盖缺口 = 20 行里 17 行 > 0"**是错的** —— 17 是"有用例的分类数"，实测 20/20 行都欠
          （最小的一行是 `7.8-ifbc` 的 62 条），`conformance_exit` 的 `categories_short` 就是 20。
      (6) **本轮自己造的测试隔离缺陷（不是实现缺陷，已修）**：`run_case` 把用例落成 `target/scratch/conformance/<用例 id>`，
          目录名只有 id ⇒ 同进程两条测试跑同一套用例时，后一条的 `remove_dir_all` 把前一条正在读的 module 删在半路，
          现场症状是 `2-lexical-0001` 假报 `E-0600 产物无入口`（一个语义码，完全不像文件竞争），单跑各自绿、并跑红。
          修 = `CORPUS_SERIAL` 静态锁，跑用例集的三条测试（含 `INFY_CONFORMANCE_PROBE=1` 那条作者工具）都先取锁。
      回归全部复跑：`cargo test --offline --lib` **524 passed / 0 failed**（基线 521，+3 全在 `conformance`；分组计数见
      `docs/status.md` 的 E3 一节）、`cargo check --offline --lib` 0 error / **50 warning**（未新增：HEAD 用 `git archive`
      解到 `target/scratch/basecheck` 全量重检，与工作树的告警**集合**逐条 `diff` 相同，各 50 条）、
      `infy check|build|run|test examples/tour` 全 exit 0、生成 C11 仍 **12513** 字节 / 产物仍 **199975** 字节 / `blob 738` /
      `build-id 4d8250b58122ca2b76132029955e2d2005156dc0a6b021bf5f4ae761e5fd55ba` **逐字节不变**（没碰 [7]-[9]）、
      `--verify-reproducible` exit 0、`infy artifact info … --nodes` exit 0 且节点表 6 行 = 8/9/10/11/16/19、
      生成 C 的 `gcc -std=c11 -Wall -Wextra -I runtime -c` exit 0（**0/0**，5723 字节 .o）、`fmt --check` 仍 exit **2**（F6 未裁决）；
      **C 侧零改动**仍复跑四条链：`-c infy_rt.c` 与 `-c infy_ifbc.c` 各 **0 告警**、rt **32335/0**（**CWD 必须是 `target/scratch`**：
      本轮先在 `target/scratch/e3/` 跑出 2 条假失败 + exit 139，换回去就绿）、替身链与生产链 IFBC 各 **180/180** 且 stdout `cmp` 无差异。
      **明确未做**：§12 的**数量**（还差 2217 条，E3 一条没填）、§12.3 黄金更新、§12.4 矩阵四档、三个空分类的实现
      （F38 / F19 / `host.rs`）、6 条 Blocked 背后的 F34/F35/F36/F37；"两侧装载期校验等价"仍不许讲（F26 前半 / D-42 (a)–(e)）。
      **E3 落的是缺口表，不是达标声明**：100% 之前那句声称仍然不许出现在任何文档里（现在有测试挡）。
      证据提交：**`008bd93`**（`feat(E3)`）—— 上述 524/0、0/50（告警集合逐条 diff）、83 条的 77/0/6 与 exit 1、
      12513/199975/`blob 738`/`4d8250b5…55ba` 未变、`--verify-reproducible` 0、`gcc -c` 0/0、四条 C 链（32335/0、
      180/180×2、`-c` 各 0 告警、stdout `cmp`）全部为该提交上的实测复跑，非引用；本节的做账行与 RUNLOG 的 cron-22 行在其后的提交里。

### 阶段 F：工程卫生（每轮顺手做一点，不许它长大）

- [x] F1 `.github/workflows/ci.yml`：`cargo fmt --check`? 不适用（无 rustfmt 约定）→ 只做 `cargo build --offline --release` + `cargo test --offline` + `gcc` 跑 `runtime` 测试。
      **cron-26 轮收尾（2026-10-16）= 本条勾 `[x]`，凭据是含修复的那一轮**：**run `38912466381`（run_number 4 / head_sha `c7312eb`
      = 上面那条 EOL 次序修复的提交）= conclusion `success`，步 3-12 逐条 `success`**（步 3 正是 run #3 红的那一步，
      远端打印 `core.autocrlf = true` ⇒ 守卫是在真条件下跑的，不是被绕开）。远端实数取自 `gh run view …--log`
      （957 行，`FAILED`/`panicked` 命中 **0**）：步 7 `cargo test --offline` = **531 passed / 0 failed**、步 8 `cargo check --offline --lib`
      告警数 = **0**、步 9 `runtime/infy_rt.c：0 告警` + `runtime/infy_ifbc.c：0 告警`、步 10 rt **32335 assertions / 0 failures**、
      步 11 两条 IFBC 链各 **180/180** 且 `cmp` 过、步 12 tour 四条 + `--verify-reproducible`（两目录产物字节相同 = **221666**）+
      `artifact info --nodes` 全 exit 0。**验收条件的读法在这里定死，免得下一轮拿"做账提交也须绿"无限自指**：
      勾 `[x]` 认的是**包含该轮交付的那个 commit 触发的 run**（= run #4）；此后的纯文档提交触发的 run 只是确认，
      不是新增的前置条件 —— 但**任何一次 run 变红就把本条重新打开**并按逐条根因处置，这条不变。
      本条**不声称**：§12 兼容（覆盖 83/2300，`conformance --report` 仍 exit **1** = D-48(a)）、§14 导览已是规范写法
      （`fmt --check` 仍 exit **2** = F6）、两侧装载期校验等价（F26 前半 / D-42 (a)–(e)）。
      **cron-26 轮后半（2026-10-16）：本条一度从 `[x]` 回到 `[ ]`** —— 勾上之后**那笔做账提交自己触发了 run `38910330820`（run_number 3）= `failure`**，
      红在**步 3「检出没改写工作树」**，红的正好一个文件：`conformance/cases/2-lexical/2-lexical-0002/main.infy`。
      **根因是 F1 自己的修法有缺陷**：`.gitattributes` **后匹配优先**，为修 run #1 假红而加的全局 `*.infy text eol=lf` 写在
      `conformance/cases/** -text` **之后** ⇒ 全局规则盖掉语料例外 ⇒ 那条**故意带 6 个 CRLF 对**的正反例被 clean 过滤器抹成 LF
      ⇒ 归一化结果 ≠ blob ⇒ 在 `core.autocrlf=true` 的检出里**永久 dirty**。本机 `git clone` 到 `target/scratch/` 当场复现
      （`porcelain` 恰这一条、`check-attr` 给 `text: set / eol: lf`）；**对照组** = `2-lexical-0001` 只有 **1 个裸 CR、0 个 CRLF 对**
      ⇒ clean 无事可做 ⇒ 它不脏（与远端只报一个文件逐字相符）。**字节其实没被 smudge 改写**（工作树仍是 6 个 CR，实测）
      ⇒ 这是「守卫红而用例语义还活着」那一族；也因此 **run #2 的步 3 对同一条规则没红** —— **观察到的事实是**：同一棵树、同一份属性表，
      run #2 步 3 绿而 run #3 步 3 红，且隔离探针里 `git status` 的读数会**随时间翻转**（刚 clone 后 0 行、几分钟后 1 行）
      ⇒ 守卫的结论不是由树的内容单独决定的。**归因（机制推断，不是本轮直接量到 git 内部）**：`git status` 在 stat 缓存
      可信时不比内容（racy-clean 那一族）。**可执行的那句结论**：`git status --porcelain` 当行尾/改写守卫不可靠，两个方向都不可靠
      —— **「守卫绿」从来不等价于「树干净」**（要不要让守卫连内容一起查，留给下一轮，本轮不扩大改动面）。
      **修法已落地并双向验证**：`.gitattributes` 把 `conformance/cases/**`（含 `**.infy` / `**.infy.toml`）的 `-text`
      **重新声明在全局规则之后**；新测试 `the_corpus_eol_exception_is_declared_after_the_global_infy_rule` 两头钉 ——
      ① 次序（`rposition` 比较，反了就红并指名两行行号）② 工作树里那两条正反例仍是它声称的形状（CRLF 对 ≥ 6 / 裸 CR 且 0 个 CRLF 对）。
      **两条负对照都实测变红**：次序翻回去 → 红；把 0002 的 CR 抹掉 → 红并报「只剩 0 个 CRLF 对」；随后 `cmp` 验证两文件字节复原、
      `git status` 里语料文件不再出现。基线换为 `cargo test --offline --lib` **531 passed / 0 failed**（+1 = `conformance` 18 → **19**）、
      `cargo check --offline --lib` **0 error / 0 warning**。
      **勾 `[x]` 的条件仍是「那一轮真 run 绿」**：run #2 绿（步 7-12 的远端实数已拿到，见下一段），run #3 红（本条缺陷），
      修复后的 **run #4** 才是本条的最终凭据 ⇒ **下一轮开头回查 `gh api …/actions/runs?branch=toolchain` 里 run #4 的逐步结论**，
      绿了就勾 `[x]`、红了照实记，**不许**为了让它绿而放宽步 3 的守卫。
      **cron-26 轮前半（2026-10-16）= run #2 十二步逐条 `success`**（当时据此勾了 `[x]`，随后 run #3 暴露出上面那条缺陷 ⇒ 已回退）：本轮开头 `scripts/push-branch.ps1`
      第 **1** 次即 exit 0（`1896c2a..6efb926`，`gh api …/git/refs/heads/toolchain` 回查 = 本地 HEAD ⇒ 现算欠账 0，
      cron-24/25 欠的 **7** 个提交全部清掉），推送当场触发的 **run `38908208089`（run_number 2, event push, head_sha `6efb926`）
      conclusion = `success`**，且 **12 步逐条 `success`**（`gh api …/actions/runs/38908208089/jobs` + `gh run view --log` 两份都取了，
      日志 956 行、全仓 `FAILED`/`panicked` 命中 **0**）。**run #1 从没跑过的那 5 步（步 8-12）这轮第一次在远端跑，全部实到数字**：
      步 7 `cargo test --offline` = `running 530 tests` → **530 passed / 0 failed**（另有 bin 的 0 项、doc-test 的 0 项）
      ⇒ **EOL 修复在远端生效**（步 3「porcelain 为空 + `grep -rlU` 查 CR 字节」两道守卫都过，run #1 那三条 `W-0001` 假红消失）；
      步 8 告警数 = **0**；步 9 两份 `.c` 各打「0 告警」（stderr 空才走得完）；步 10 rt **32335 assertions / 0 failures**
      （`cd target/scratch` 那条前提在 runner 上同样成立）；步 11 替身链与生产链 IFBC 各 **180 asserts / 180 pass / 0 FAIL** 且 `cmp` 通过；
      步 12 tour 的 `check|build|run|test` + `--verify-reproducible`（两个独立构建目录产物字节相同）+ `artifact info --nodes`
      全部 exit 0，**节点表 6 行 = 8/9/10/11/16/19 与 §8.4 金样逐行相同**、生成 C11 **12513** 字节与本机逐字相符、
      `unreproducible false`。**两条只认远端的供给事实**：runner 的 `gcc` 在 PATH（步 5 success）、`setup-python@v7` 装的是
      **CPython 3.14.8**（本机 3.14.6）⇒ `btrack::tests::py_unit_fixture_is_exactly_what_the_ingest_produces` 在 3.14.8 上仍 `ok`，
      「B 轨夹具回核只对本机那个 patch 成立」这句不成立。**一条新的基线口径教训**（照实记，不当已解决）：远端 tour 产物是
      **221666 字节 / build-id `e39398bc…a5be`**，本机一直记的是 **199975 / `4d8250b5…55ba`** ⇒ 这两个数**不是**跨机常数，
      而是宿主 `cc` 的函数（远端 `.inf meta` 自己写着 `source_toolchain gcc 15.2.0`，本机 MSYS2 是 16.2.0；
      差的只有镜像 220864 vs 199173，**段 blob 738 + 段尾 64 两侧逐字节同**）。CI 因此**只打印不门禁**这些数是对的；
      谁把它们当断言，就是把 CI 永久钉红。勾 `[x]` 只声称「那一次真 run 的 conclusion = success + 步 8-12 的远端数字已知」，
      不声称「§12 兼容」（覆盖仍 83/2300）、不声称「`fmt --check` 过」（F6 仍 exit 2）、不声称「两侧装载期校验等价」（F26 前半 / D-42 (a)–(e)）。
      **F2 轮（2026-10-16）把下面那段「远端还没有 workflow」的旧声称作废并给出真结论**：推送第 1 次即成（`6fceaa5..1896c2a`），
      `gh api …/actions/workflows` 由 **0 → 1**、`…/actions/runs?branch=toolchain` 出现 **run 38892797672（run_number 1, event push）**
      ⇒ **分支上首条 workflow 不需要人工批准**（过去两轮悬着的那个问题就此有答案）。该 run **conclusion = failure**，逐步结论：
      步 1-6 全 `success`（含 **Provision CPython 3.14** 与 **工具链自检** ⇒ `windows-latest` 上 `gcc` 在 PATH、`setup-python@v7` 有 CPython 3.14，
      「runner 供给」那两条疑问**都不成立**、不需要改 yaml 的供给面）；步 7 `cargo test --offline` **failed**（`526 passed; 3 failed`），步 8-12 因 `set -e` 之后全部 `skipped`。
      **3 条红的都是同一根因，而且是本仓库第三次踩行尾**：`cli::tests::a_module_without_embeds_produces_no_images` /
      `run_executes_the_tour_and_exits_zero` / `test_refuses_cov_and_runs_zero_tests_where_there_are_none`，日志里是
      `warning[W-0001]: CRLF 行结束符（共 28 处 / 共 38 处）` 打在 `cmd/runner/main.infy` 与 `core/build/mod.infy` 上 ⇒ 断言 `left: 2, right: 0`。
      根因 = runner 的 `core.autocrlf=true` 把**没有 EOL 属性**的 `examples/**.infy` 检出成 CRLF，而 §2.1 的 `W-0001` 把行尾本身当被测面；
      本地永远是 LF ⇒ 本地 529 项全绿看不见它。**更要紧的是那条守卫本身是错的判据**：`git status --porcelain` 为空**证明不了**字节没被改写
      （clean 过滤器把 CRLF 换回 LF 再比较），所以「检出没改写工作树」这一步在 run #1 里是 `success` 而工作树其实是 CRLF。
      **修法两条同批**（不是放宽断言）：`.gitattributes` 加 `*.infy text eol=lf` + `*.infy.toml text eol=lf`；
      CI 的守卫步加一条直接查输入面字节 —— `LC_ALL=C grep -rlU CR --include=*.infy --include=*.infy.toml examples` 命中即红并打名单
      （`conformance/cases` **不查**，那里故意有写 CRLF 字节的正反例）。本地负对照实测：LF 树命中 **0** 条，把 `examples/tour` 的两个 `.infy`
      改成 CRLF 后命中 **2** 条并指名这两个文件；`infy check` 对同一份 CRLF 副本 exit **2** 并打 2 条 `W-0001`（与远端逐字相符）。
      新断言也钉进测试（`the_ci_workflow_is_windows_only_and_keeps_the_three_anti_false_red_guards` 多两条：`grep -rlU` 必须在、
      `.gitattributes` 里 `*.infy` 必须在）。**本条仍 `[ ]`**：勾它的条件是那**一次真 run** 的 conclusion = success，而 run #1 是红的；
      修复已推送 ⇒ **下一轮 = `gh api …/actions/runs?branch=toolchain` 取 run #2 的逐步结论**（还可能在步 8-12 红，那 5 步在 run #1 里是 skipped，从没在远端跑过一次）。
      **F1 轮（2026-10-16）= 实现与文档全部落地，但本条保持 `[ ]`**：验收条件按 §2 的 F1(4) 只认 `gh api …/actions/workflows`（及 runs）的**回查**，而本轮 `git push origin HEAD:toolchain` **三次全败**（3×`Recv failure: Connection was reset`，同一时刻 `gh api` 正常 ⇒ 仍是 `github.com:443` 抖动，不是 token/配置），远端 `toolchain` 仍 = `6fceaa5`、`…/actions/workflows` = **0**、`…/actions/runs` = **0** ⇒ 远端**还没有这条 workflow**，「CI 绿」与「CI 跑过」两句都不许写。**卡在哪**：推送。**下一轮从哪继续**：开头先补推 `ae2c58e` + 做账提交，再 `gh api …/actions/runs?branch=toolchain` 回查 run 是否被创建（首条 workflow 常需一次手动批准）；红了就按「runner 供给」（`gcc` 在 PATH 吗、`setup-python` 的缓存里有 3.14 吗）逐条判定，那两条在 yaml 里都是「缺了就红并打 PATH」而不是「缺了就跳」。**本地已实测的部分不重做**（下列数字均为 `ae2c58e` 所在工作树的复跑，非引用）。
      **F1 轮（2026-10-16）落地（本条仍 `[ ]`，见上）**：新建 `.github/workflows/ci.yml`（单 job、11 步 = 2 个 `uses` + 9 个 `run`）+
      `.gitattributes` 一条把 workflow 自己钉成 `eol=lf` + README 的目录表与顶部 CI 行 + `docs/coverage.md` **P30**。
      **门禁内容**（逐条与 §1 的证据命令同源，本地按 GitHub 的执行方式 `bash --noprofile --norc -eo pipefail` 重放）：
      `cargo build --offline --release` exit **0**（23.8s）→ `cargo test --offline` exit **0**（**527 passed** + bin 的 0 项 + doc-test 的 0 项）→
      `gcc -std=c11 -Wall -Wextra -c runtime/infy_rt.c` 与 `-c runtime/infy_ifbc.c` 各 **0 告警**（stderr 0 字节才算过）→
      rt harness **32335 assertions / 0 failures** → IFBC 替身链与生产链各 **180/180** 且两份 stdout `cmp` 无差异 →
      tour 的 `check|build|run|test` + `--verify-reproducible`（199975 == 199975）+ `artifact info --nodes`（节点表 6 行 = 8/9/10/11/16/19）全 exit **0**。
      `cargo check --offline --lib` 的告警数（**50**）在 CI 里**只报不门禁** —— F2 未清，门禁等于假装它已清。
      **`cargo fmt --check` 确实不适用**：本仓库没有 rustfmt 约定，`src/` 全文不是 rustfmt 形状（跑一次会产生大面积 diff ⇒ 等于再造一套风格真值源）。
      **三条防假红由测试钉住而不是注释**（`conformance` **14 → 17**）：`the_ci_workflow_covers_the_build_the_tests_and_both_c_harnesses`
      （命令面少一条 = 那一环从此没人跑）、`the_ci_workflow_never_sets_the_regeneration_switches`（四个重写夹具的开关只许出现在注释里，
      赋值即红 —— 否则「双侧逐字节比较」变成「两侧各写一遍再比一遍自己」）、
      `the_ci_workflow_is_windows_only_and_keeps_the_three_anti_false_red_guards`（每一行 `runs-on` 都须含 `windows` + 工作树守卫
      `git status --porcelain` 必须在 + rt harness 的 `cd target/scratch` 必须在 + `.gitattributes` 那条 `eol=lf` 必须在）。
      **只有 Windows**（阶段 [9] 产 PE + `INFYART2` 追加段、`std.fs` 走 `CreateFileW` ⇒ 别的平台跑的不是这 527 项测过的形状，
      没加 matrix 假装双平台）；**CPython 用官方 `actions/setup-python@v7` 装 3.14**（`py_unit_fixture_bytes()` 声明 `py = "3.14"`，
      别的 minor ⇒ `E-0713` ⇒ §6 表回核红 —— 按旧指针的裁定：补解释器而不是放宽断言）；`cc` 先查 PATH 再查四个常见目录，
      都没有就带着 `PATH=` 显式失败，**没有「缺工具就 skip」的分支**。
      **两条刻意不进 CI**（写进 yaml 注释，防下一轮顺手补上）：`infy conformance --report` 今天 exit **1**（D-48(a)：`0` 要求 20 个分类
      都达到 `required` 且 Fail=0、Blocked=0，而覆盖只有 83/2300）、`infy fmt --check examples/tour` exit **2**（F6 未裁决）——
      把它们当门禁就是把缺口读成回归。
      **本轮另修掉一处 E3 遗留的测试隔离缺陷**（不是实现缺陷）：`CORPUS_SERIAL` 定义在 `conformance::tests` 里，而 `cli::tests` 的两条
      测试直接调**公开的** `conformance::run_case` 跑同一套用例 ⇒ 同进程两个用例集运行器照旧互删 module；本轮加 3 项只读测试改变了
      线程调度后现形：`7.7-b-track-0003` 假报「系统找不到指定的路径 (os error 3)」、`cli` 那条读到 `Fail = 2`，而**单跑各自全绿**。
      修 = 锁挪到共享路径的被碰点（`run_case` 自己持模块级 `SCRATCH_SERIAL`），`mod tests` 里那把锁与三处取锁行删除。
      回归：`cargo test --offline --lib` **527 passed / 0 failed**（基线 524，+3 全在 `conformance`）、`cargo check --offline --lib`
      0 error / **50 warning**（未新增）、`infy check|build|run|test examples/tour` 全 exit 0、`--verify-reproducible` exit 0、
      生成 C11 仍 **12513** 字节 / 产物仍 **199975** 字节 / `build-id 4d8250b5…55ba` 逐字节不变（没碰 [7]-[9]）、
      生成 C 的 `gcc -c` 0 error / 0 warning（5723 字节 .o）、`infy conformance --report` 仍 exit **1**（83 = 77/0/6 · 92.8% 一字未变）、
      `fmt --check` 仍 exit **2**；**C 侧零改动**仍复跑四条链（两份 `-c` 各 0 告警、rt **32335/0**、两条 IFBC **180/180** + `cmp` 无差异）。
      **验收只能验到「命令 + 文件 + 远端是否登记」**：远端那次 run 的实际结论只认 `gh api …/actions/runs` 的回查，写在紧随其后的
      RUNLOG `cron-23-push` 行里；本机量不到 runner 镜像装了什么（`github/runner-images` 在本机 API 返 404），
      所以 `gcc` 与 Python 3.14 两条都做成「缺了就红并打 PATH」而不是「缺了就跳」。
- [x] F2 消 40 条编译告警（unused/dead code），每轮清一批，`cargo check` 输出为 0 warning 时勾掉。
      **F2 轮（2026-10-16）一次清零**：口径 `cargo check --offline --lib --message-format=short`（先 `touch src/lib.rs`）
      实测 **50 → 0**、0 error；**没有用 `#[allow(...)]` 消数**，也没新增。形状分解与逐条去处见 `docs/status.md` 的 F2 一节、
      对外行 = `docs/coverage.md` **P2**。两处判成「没接上」的都接上了：§9.6 的 `[lints]`（裁定 = 偏差 **D-49**，
      成对断言的测试 1 条）与 wheel 归档的成员偏移（`lho` → `ZipEntry.data_off`，测试 1 条 + **负对照实测**：
      把 `data_off` 改回写死 0 该测试即红）。基线换为 `cargo test --offline --lib` **529 passed / 0 failed**
      （+2 = `cli` 38→**39**、`btrack` 30→**31**）、`cargo test --offline` 全目标 exit 0（529 / bin 0 / doc 0）、
      `cargo build --offline --release` exit 0（22.5s）、`infy check|build|run|test examples/tour` 全 exit 0、
      `--verify-reproducible` exit 0（199975 == 199975）、C11 仍 12513 字节 / 产物仍 199975 字节 /
      `build-id 4d8250b5…55ba` 逐字节不变、C 侧四条链复跑未变（两份 `-c` 各 0 告警、rt 32335/0、IFBC 两条链 180/180 + `cmp` 无差异）、
      `infy conformance --report` 仍 exit 1（83 = 77/0/6 · 92.8%）、`fmt --check` 仍 exit 2（F6）。
      **F2 的口径只覆盖 `--lib`**：release 构建与 bin 目标的告警未逐条清理（本轮改动不涉及它们的源代码形状）。
- [x] F3 把 `push*.log`/`rt_tests`/`ifbc_tests` 等一次性产物加进 `.gitignore`，仓库里只留 `docs/RUNLOG.md` 这一份审计线。
      **F2 轮顺手做掉的一项**：删掉被跟踪的 `src/fmt.rs.tmp`（3984 行的旧 `fmt.rs` 快照，`418a34c` 起入库、
      cargo 不编译它、无人引用，与 `src/fmt.rs` 已差 1382 行）。
      **F3 轮（2026-10-16）做完剩下那一族并勾 `[x]`**：`.gitignore` 从"记具体名字"（原来只有 `push.log` 一条，
      所以 `push2.log`/`push3.log` 一路入库）改成"记**产物形状**" = `*.log` + `logs/` + `*.tmp` + `*.o` +
      无扩展名那一族（`/rt_tests`、`/ifbc_tests`、`/ifbc_real`、`/a.out` —— `cleanup.ps1` 的白名单里就有它们，
      只写 `*.exe` 的话同一命令少打个后缀就又是一条脏文件）+ `/chunk.txt`（某次截断会话留下的 Rust 碎片，
      全仓库零引用：`grep -rn 'chunk\.txt'` 除自身为空）。
      **仍在跟踪的 5 项只从索引里摘掉**（新脚本 `scripts/untrack-artifacts.ps1`，`git rm --cached` ⇒ 工作树
      一字节未动、文件全部留在磁盘上；那些 log 是历史轮次的现场记录，删掉等于销毁证据，F3 要的是"仓库里
      只有一份审计线"而不是本机不留痕）。§0.0 四条硬规矩齐：默认干跑、幂等、动作逐条进 `logs/untrack-artifacts.log`
      （含回滚命令 `git add -- <路径>`）、白名单**封闭**（`docs/`/`spec/`/`src/`/`runtime/`/`conformance/`/
      `scripts/`/`.github/`/`.gitignore`/`.gitattributes` 命中即拒，解析到仓库外即拒）。
      实测：干跑命中 **5**、拒绝 **0**；`-Apply` exit **0**、`VERIFY mode=APPLY 命中=5 失败=0 剩余跟踪=0`；
      复跑 = `NOOP` exit **0**（幂等）；`git ls-files | grep -Ei '\.(log|tmp|o)$|^logs/|chunk\.txt'` = **0** 行；
      `git check-ignore -v` 逐条命中 `.gitignore:15:*.log` / `:16:logs/` / `:17:*.tmp` / `:18:*.o` / `:3:*.exe`；
      5 个文件 `ls` 全部在场。细节见 `docs/status.md` 的 F3 一节与 `docs/coverage.md` **P31**。
      **两条不是"顺手"的判断**：① 反向红线进了测试 —— 名单宽到 `*.bin`/`*.md`/`*.rs` 或某个整目录会让真值源
      （跨语言逐字节夹具、逐条款文档）**悄悄脱离跟踪而测试照样全绿**，所以新测试两头都查（六条必需模式按
      **整行相等**比对，`contains` 会被 `*.logx` 糊过去；十四种禁止形状一个不许出现）；② 「`.gitignore` 自己会不会
      也被 `core.autocrlf` 改坏」没有猜：隔离 `git init` 仓库里写一份 **CRLF** 的 `.gitignore`，两条模式照常命中
      ⇒ **不**给它补 `text eol=lf`（行尾这一族 F1 的 run #1 已经红过一次，值得每次实测而不是背规则）。
      回归：基线 `cargo test --offline --lib` **530 passed / 0 failed**（+1 全在 `conformance`：**17 → 18**）、
      `cargo check --offline --lib` **0 error / 0 warning**、`cargo build --offline --release` exit 0、
      tour 的 `check|build|run|test` + `--verify-reproducible` 全 exit **0**（两份 `ci.exe` 各 199173 字节且 `cmp` 相同）、
      C11 仍 **12513** 字节 / 产物仍 **199975** 字节 / `build-id 4d8250b5…55ba` 逐字节不变（`src/` 生产代码一字未动）、
      `infy conformance --report` 仍 exit **1**（83 = 77/0/6 · 92.8%）、`fmt --check` 仍 exit **2**（F6）；
      **C 侧零改动**仍复跑四条链（两份 `-c` 各 **0 告警**、rt **32335/0**、两条 IFBC **180/180** + `cmp` 无差异）。
- [x] F4 `infy lsp`、`std.net` TLS、`dyn Trait` vtable 偏移表：要么实现，要么在 `docs/spec-deviations.md` 里保持"明确未实现"的记录，不许静默。
      **F4 轮（2026-10-16）走"记录"这条验收路** ⇒ 裁定 = 偏差 **D-50(a)–(c)**（提交号由紧随的做账提交补记，同 F1 的做法；
      本轮的 4 条锁点与全部数字在 `docs/status.md` 的 F4 一节）。为什么不走实现：三件里两件被**规范本身**挡住 ——
      附录 C-7「仍缺 ④」明写 LSP 的增量诊断与 §9.4 的逐字节一致性从未定义（实现了也只能造一个看起来在服务的空 server = 假成功），
      §10.4 又要求"TLS 后端由 `.inf rt` 段内置"而 §1.3 的零第三方依赖约束下没有可依赖的密码学实现（`runtime/infy_rt.c` 里
      grep `TLS` 只命中 thread-local 的 `INFY_TLS` 宏 —— **同一三字首两种含义**）。第三件（vtable）是唯一有真实落点的：
      偏移表**由本实现定死**并写进 `docs/internals/runtime-abi.md` §1（`drop@0:8 size@8:8 align@16:8`、方法 `i` = `24+8i`、
      `total = 24+8n`、胖指针 16/8），`hir::Layouter::vtable_record`（新增，`vtable_layout` 改为委托它）是唯一真值，
      测试**把文档那三行读回来**再比 —— 负对照实测：`1@32:8` 改成 `1@33:8` ⇒ 该测试红并报"文档里的方法 #1 槽 ≠ 布局器"，改回即绿。
      实测的拒绝形态（每条都是命令输出）：`infy lsp` 与 `infy lsp examples/tour` = `notice: … not implemented …` + exit **1**；
      `net.http_get(…)` 与全限定 `std.net.http_get(…)` = `E-0101` + `E-0110`、`infy check` exit **2**；
      合法 `dyn` 形参（`fn f(x: dyn Shape) { }`）阶段 [1]-[5] **过得去**（check exit 0），拒绝在 [8]：
      `error: codegen: 'dyn Trait' 的 vtable 发射未实现 …`、`infy build` exit **6**。
      **顺带纠两处旧记录**：`codegen` 的 dyn 文案原本指 `docs/coverage.md §11`（那一行讲的是 `HostApi`，指错）⇒ 改指
      `runtime-abi.md §1` / D-50(c) / F4；`stdlib.rs` 模块头声称 STD 表"驱动 name resolution / `infy doc` / codegen"是**假声称**
      ⇒ 本轮改成实话。三件仍**不声称**已实现：`.inf meta` 没有 vtable 槽位 ⇒ 跨 `.infib` 的 `dyn` 链接不做声称；
      本轮实测出的三条真缺陷按下方 **F40/F41/F42** 登记（拒绝的形态是对的，但 F40/F41 的**理由**是错的，不能混在一条正确的拒绝里）。
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

- [ ] F9 **`ExprKind::Int(IntLit)` 这一形状从不由 parser 产生**（C1 轮定位；**死臂本身已由 F2 轮删除**）：
      `parser::primary()` 原先在 `Kind::Lit(l)`（匹配所有字面量）之后还挂着一条 `Kind::Lit(Lit::Int(i))` 臂，永不可达
      ⇒ 整数永远以 `ExprKind::Lit(Lit::Int(..))` 进 AST。F2 轮删掉了那条死臂并在原地留一行注释
      （`ExprKind::Int` 的唯一产地是 `derive`：`derive.rs:235`/`1595` 现造整数节点），同族的
      `parser.rs:1675/2179/2256` 三处重复臂一并清掉。**仍未做**：凡**只**匹配 `ExprKind::Int` 的判据现在只靠
      `derive` 才可达，而 `derive` 不产的场景（例如 `#[link(name = "…", ordinal = 3)]` 那类属性参数走的是
      `Attr::int_arg`）是否可达仍需逐条审 —— `ast::Attr::int_arg()`、`cli.rs` 与 `sema.rs` 里的 `ExprKind::Int`
      判据在名单内。验收：`grep -n "ExprKind::Int" src/*.rs` 的每一处都有测试证明它可达（或已删除），
      且 `primary` 的臂顺序有回归测试（F2 轮**没有**新增"整数字面量 = `ExprKind::Lit`"这条锁，仍欠）。

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

- [ ] F28 **随发布的三份能力表里，只有 `IFBC-CAP-py-3.14.toml` 满足附录 E.3 的数值键规范式**（D1(1) 轮补
      `btrack` 单测时确认）：E.3 明写 `[opcode]` 行以 `0xNN`（该 minor 的 `dis.opmap` 值）为规范式、
      `OPNAME` 只是别名，且 `CapTable::check_alias_agreement` 要求两式同值；实测
      `IFBC-CAP-py-3.13.toml` 与 `IFBC-CAP-lua-5.4.toml` 的 `[opcode]` 段**一行数值键都没有**（只有别名），
      而扫描器的查找顺序是"先数值、后别名"，所以对这两张表 `check_alias_agreement` **永不触发**，
      也没有任何一条断言能把表与那版本的 `dis.opmap` 对账（E.3 的可审计要求落空）。
      py-3.14 表是合规的（154 行数值键 + 别名，`the_number_keyed_table_agrees_between_its_number_and_name_rows`
      逐行钉两式同值）。
      **为什么不直接补数**：本机只有 CPython 3.14.6 与无 lua —— 3.13/lua 的 opcode 数值必须来自**那版本的
      解释器**，凭记忆填等于编造常数（B1 轮 `xxh64`/`golden_perm` 那一族的教训，§12.2 要求能力表可复核）。
      验收：在有 CPython 3.13（或取其 `dis.opmap` 的权威发布件）与 lua 5.4 的机器上跑一次表生成，
      把两份表重写成"数值键 + 别名"双式；然后把 `btrack::tests::the_name_keyed_tables_still_map_every_row`
      里那条"补上数值键就变红"的断言指向本条，把两张表移进 `the_number_keyed_table_agrees_…` 的列表，
      并确认 `check_alias_agreement` 对它们真的会触发（加一条人为造矛盾的测试）。

- [ ] F29 **阶段 [8] 没有通往 IFBC VM 的调用通道，`.inf ifbc` 因此恒空**（D1(2) 轮接线时确认）：
      阶段 [6a] 现在能从 CLI 产出真镜像（`Loaded::embed_images`），但 `codegen` 对 embed 体仍报
      `CgError`（`src/codegen.rs:2334`，"A 轨单元调用未实现（D3）"同族），于是含 `@py` 的 module 在
      `infy build` 阶段 [8] 就 exit 6，永远走不到 [9] ⇒ `artifact.rs` 那两行 `.inf ifbc` 写入对本构建
      是**不可达路径**。规范侧的对应条款：§7.7.1 规则 1"不再是注入全局变量，而是函数调用约定"、
      §7.4 `.inf ifbc` 非空时装载期先跑 §7.8.3、§12.2 要求行映射回原语言。
      验收：给 codegen 一条 `infy_ifbc_invoke`/模块装载入口的发射形态（冻结头现在**没有**"从产物里取
      B 轨模块"的公共入口，见 `runtime/IFBC.md` §7 的记录 ⇒ 先补头并同批复跑四条 C 链），把 `@py` 单元
      接进调用约定，然后：同一个 module `infy build` exit 0、`infy artifact info` 打出
      `n_ifbc_modules >= 1`、`.inf ifbc` 的长度等于镜像字节数、`infy run` 里该单元真被调用；
      届时 199975 / `4d8250b5…55ba` / `blob 738` 三处数字会动（预期变化，同步改 status/coverage/README）。

- [ ] F30 **§7.7.1 的 `@py` 样例本体过不了能力扫描**（D1(2) 轮接线时实测）：
      `event normalize(batch: &[f32], scale: f32) -> Vec<f32> { @py(batch=batch, scale=scale) { out =
      [ (x / scale) for x in batch ]; return out } }` 在 `infy check` 上报
      `E-0712 字节码能力缺失：RERAISE`（扫描项 1 · code object `normalize` · `co_code+0x3c` · 原始语言行 3），
      另有一条同族：`f32` 形参与无后缀 `2.0` 混宽 ⇒ `操作数类型 [f32, f64] 不满足能力表给出的任何映射目标`。
      这不是接线缺陷，是 §7.7.2 设计的样子（"没实现的东西在构建期就红着退出"），但**规范的样例就是不能构建**
      属于文档级矛盾：§14 的导览、§7.7.1 的样例、§12.2 的合规用例（"能力扫描正反例（每类 `E-0712`）"）
      都默认这段能过。锁点 = `btrack::tests::the_list_comprehension_sample_is_refused_by_name_not_silently`
      （断言点名 `RERAISE` 且带原始语言行）。
      验收：要么扩能力表/`lowering` 让 `RERAISE` 与 `GET_ITER`/`FOR_ITER`/`END_FOR` 的收尾形态可映射
      （F27 的迭代器载体是前置），要么把 §7.7.1/§14.2 的样例改成可摄取形态并请维护者在规范里勘误；
      裁定写进 `docs/spec-deviations.md`，在此之前任何文档不得声称"§7.7.1 样例可构建"。

- [ ] F31 **§7.6.4 行 7 的 struct / ADT 不透明指针没有跨边界落点，于是 §7.6.1 的样例自己过不了**（D3 轮确认）：
      绑定头只携带**标量与标量指针**（`uint32_t`/`double`/`const uint8_t*`…），因为按值跨边界的
      `#[repr(C)]` struct 需要类型定义同时出现在头与生成码里，而生成码的 ADT typedef
      （`infy_<pkg>_<Name>`）是 ci.c 私有的 —— 两侧各写一份就是 `-Wall` 下的不兼容声明。
      后果实测：§7.6.1 的 `fn dec_open(...) -> *mut DecHandle` 与 `dec_close(h: *mut DecHandle)` 在阶段 [8]
      被 `§7.6.4` 指名拒绝（D-46(e) 的第二半），所以那一条 A 轨样例**不能构建** —— 与 F30 同族。
      验收：`DecHandle` 这类 ADT 的指针能跨边界（头里给 `struct DecHandle;` + 同名 `typedef`，生成码用同一个
      tag，或按 §4.8 把 `#[repr(C)]` 的布局表导出成头），并让 §7.6.1 的样例通过
      `infy check` + `infy build` + gcc；在此之前任何文档不得声称"§7.6.1 样例可构建"。

- [ ] F32 **§7.6.2 的工具链发现/注册与审计面全未接**（D3 轮确认）：`TOOLCHAIN.infy.toml`（module 根或
      `%LOCALAPPDATA%/Infinity/toolchain/`）、`infy toolchain probe` / `infy toolchain add <kind> --path <exe>`、
      构建期**版本核对**（实际编译器版本必须与 `.infib` 记录的 major/minor 一致，否则 `E-0604`；
      `--allow-toolchain-drift` 降级为 `W-0604` 并把产物标 `unreproducible`）、
      `<build>/a_track/<name>/recipe.toml`（编译命令行 / 环境变量 / 输入哈希，供复放与审计）都没有实现。
      现状：A 轨只驱动 `link_plan` 的 `--cc`/`$CC`（缺 → `E-0602` + exit 6，P15 已钉），而
      `#[link(toolchain = "…", flags = [...], isolation = "…")]` 的三个参数**被读出来却没有任何消费者**。
      验收：要么接上（`recipe.toml` 落文件 + 版本核对走 `E-0604`/`W-0604` + `flags` 进命令行），
      要么在偏差里记"这三个参数在本实现无对应语义"，不许静默忽略。

- [ ] F33 **§7.6.3 项 3/5 与 §7.6.1 的 `W-0720`、§7.6.5 反向导出、§7.6.6 进程隔离未做**（D3 轮确认）：
      项 3 的边界 thunk 现在只有"标量原样转发"一种形态，规范列的检查（长度 / 对齐 / null→`Option` /
      调用期借用有效性 `R-0720` / 错误码→`NativeErr` / 外部忘写 `err` → `R-0722` / 回调计数不为 0 时卸载 →
      `R-0723`）一条都没有落点；项 5 的调试映射（外部行号 → INFY 诊断位置，使 `R-` panic 能报
      `decoder.c:41`）未做；§7.6.1 的 `W-0720`（源码导出而签名未声明且无人引用）没有 C 侧符号扫描，
      而"签名声明了但源码未导出 → **链接期** `E-0604`"也没有映射 —— 今天由 `cc` 的 undefined reference
      原文冒出来（退出码 6，不是 `E-0604`）；§7.6.5 的 `#[export("name")]` 反向导出表进 `.inf meta` 与
      `E-0724`；§7.6.6 的 `#[link(isolation = "process")]`（独立子进程 + 长度前缀 IPC）全未实现。
      验收：逐条落地，或逐条在 `docs/spec-deviations.md` 记"无对应语义"，禁止假成功。

- [ ] F34 **§10 的 std 签名表里没有 `Vec::clone`（也没有别的 clone 入口），于是 §4.7 的"clone 后再各自使用"这一类正例不可表达**（E1 轮搬运 `borrow::tests::clone_before_second_use_is_clean` 时确认）：
      `fn take(v: Vec<u8>) -> Vec<u8> { v }` + `fn f(v: Vec<u8>) { let a = take(v); take(a.clone()); take(a); }`
      实测报 **3 条 `E-0110 · 类型 Vec<u8> 没有方法 clone`**。原单测只断言"结果里不含 `E-1001`"（`contains` 式），
      所以这三条一直在，只是从没被看见。后果不止用例：§4.7 的"正反 20 例"里 clone 那一档、
      §6.7 `#[derive(Clone)]` 的语义、以及 F12（owned 移动没有运行时入口）都缺这一条路。
      验收：把 `clone` 接进 §10 的 std 表（或裁定 §4.7 的解除形状走别的路径），令
      `conformance/cases/4.7-ownership/4.7-ownership-0002` 从 Blocked 转 Pass 并删掉它的 `defect` 标记。

- [ ] F35 **`#[derive(...)]` 生成的体调 §10 的 std 表里不存在的符号 ⇒ 任何 derive 都过不了 `infy check`**（E1 轮确认）：
      `derive.rs:1165` 发 `std::fmt::format_struct`（`repr(C)` 时 `format_tuple`）、`:1176/1183` 发
      `std::fmt::format_unit`，而 §10 的 std 签名表里**一条 `std.fmt.format_*` 都没有**（同 F21 的 `std.test` 那一族）。
      实测最小形状 `#[derive(Debug)] pub struct Job { pub id: u32, }` → `E-0053 缺少返回值` +
      `E-0101 调用未声明的函数 format/format_struct` ×3 + `E-0110 无法解析字段 id` ×4；enum 那一档再多出
      `E-0070 match 不穷尽` ×2。换 package 名、加 `import "std/fmt" as fmt;`、补 `[[bin]]`/`[[lib]]` 都不变
      （本轮逐条试过）。**为什么此前没人知道**：`derive.rs` 的 57 项单测断言的全是生成物的**文本**
      （`expand_derives` 的产物从不被阶段 [2]-[4] 回读），而 `examples/tour` 的 `Phase`/`Job`/`Fail`
      **根本没写 `#[derive]`** ⇒ "derive 已通"这一族声称从来没有端到端证据
      （认法 = 经验库 `infy-unreachable-implementation-branch`）。
      验收：给 §10 补 `std.fmt.format_struct`/`format_tuple`/`format_unit` 的签名（或裁定 derive 发别的形式），
      令 `conformance/cases/6.7-derive/6.7-derive-0001` 与 `-0003` 从 Blocked 转 Pass 并删标记；
      并给 derive 加一条"生成体再过一次 `analyse`"的常驻测试（否则下一族同形状还会重现）。

- [ ] F36 **同一根因被两个阶段各报一次诊断**（E1 轮确认）：`let union = 1u32;` 报 **2 条 `E-0020`**
      且标题不同（一条"保留字 `union` 不可作标识符"、一条"保留字作标识符"）；`#[derive(Nope)]` 报
      **2 条 `E-0410`**。§9.2 没有"同一违规报两次"的规定，而 §12 的用例比较的是**诊断码全集** ⇒ 重复报告
      直接改变通过率，也让 `--diag-json` 的消费者看到幻影条目。与 D4(3) 轮修掉的 `verify_func` 重复报告同族
      （那次根因 = 检查写在走查里 + 后继被重复压栈）。
      验收：每个形状只报一条（后者降级为 note 或合并），令
      `conformance/cases/2-lexical/2-lexical-0010` 与 `conformance/cases/6.7-derive/6.7-derive-0002` 转 Pass、删标记。

- [ ] F37 **未知 `@` 嵌入前缀（`E-0702`）之后级联 8 条 `E-0053`**（E1 轮确认）：
      `fn main() { let v = @forth(a = 1) { 1u32 }; }` 实测 `E-0702` ×1 + `E-0053` ×**8** —— 一条词法错误
      把整条语句乃至整个文件炸成"期望顶层声明"，而 §9.6 只给了每码上限（`MAX_PER_CODE`），没有相邻去重
      也没有恢复点。影响面：§12.1 要求正反例成对，级联会让每条反例的期望里混进无关码
      （本仓库这条只能挂 `defect` 记成 Blocked）。
      验收：`E-0702` 之后把该嵌入表达式作为一个整体恢复（跳到 `;` 或块尾），令
      `conformance/cases/2-lexical/2-lexical-0013` 转 Pass；顺带审 `lexer.rs` 其它"一个错误吞一行"的恢复点。

- [ ] F38 **§1.3 领域中立的扫描有函数、没有调用点，而黑名单文件不在仓库里**（E1 轮确认）：
      `pkg::domain_neutrality_violations`（`pkg.rs:1171`）实现并单测了 §2.10 的 NFKCCaseFold 匹配，但
      **全仓库没有任何 CLI 调用点**（`grep -rn domain_neutrality src/` 只有定义与该函数自己的测试），
      而规范 §12.1 点名的 `std/domain_terms.txt` 根本不存在（`find . -name domain_terms*` 空）。
      ⇒ §1.3 要求的"静态扫描 `std.*` 与语言层导出符号表，命中黑名单即判不合规"在 `infy check`/`infy build`/
      `infy conformance` 三条路上都不发生，`conformance/cases/1.3-domain-neutral/` 因此是空分类（目录留着、缺口打印）。
      验收：把黑名单落成仓库内文件（并在 §10 的 std 表 + `stdlib.rs` 的导出符号上真跑一次），
      加 `infy conformance` 的一档或 `infy check` 的一条诊断，再为该分类补正反用例。

- [ ] F39 **§9.5 第 2 条的「输入 tab 参与宽度测量」这一半没接**（F2 轮删死码时确认，未实现）：
      `src/fmt.rs` 里那个 `expand_tabs_for_width()`（输入 tab 进位到 4 的倍数、只为测量而存在）**从未被调用**，
      F2 轮把它删了而不是接通 —— 因为接通它要先回答一个规范没答的问题：§9.5 的 100/120/40 三档宽度是
      **输出**列数，而同一条规则的前半禁止输出里出现 tab，于是「含 tab 的注释按几列参与断行」没有金样可依。
      现状 = 宽度一律按 `base_columns()` 逐码点算，tab 按 1 列计。
      认法：`grep -rn "	" examples/ conformance/cases/` 为空 ⇒ 这条今天不可测，**别**为它编造期望。
      验收：先决定「含 tab 的注释按几列」并把裁定写成本文件的偏差条目，再让测量走那条规则并配正反例。
- [ ] F40 **任何 `import "std/…"` 都不产生绑定 ⇒ 整个 §10 的 std 面从 INFY 源码够不到**（F4 轮实测）：
      阶段 [3] 的 `cli.rs:330` 是 `pkg::ImportSource::Std => {}` 空臂，`stdlib::std_packages()` / `lookup()` / `all_symbol_paths()`
      在 `stdlib.rs` 之外**零消费者**（只有 `nfkc_case_fold` 被 `pkg.rs:1182` 用）。后果：`import "std/fs" as fs;` 之后
      `fs.read_to_str("x.txt")` 吃 `E-0101 未声明的标识符 'fs'` + `E-0110 类型 <invalid> 没有方法 'read_to_str'`，
      与没有 native 的 `std.net.http_get` **一模一样**、`infy check` exit **2**；全限定 `std.fs.read_to_str(…)` 同样 `E-0101`（`'std'`）。
      ⇒ `native: Some("infy_fs_read_all")` 那些映射**从源码路径永远走不到**，`std` 只在运行时内部被用（`infy_rt.c` 自己调 `infy_fs_read_all`）。
      这也是 **F19(a)**"没有任何 INFY 构造能写 stdout"的上游根因，且它使 `stdlib.rs` 无法被 §12.1 的扫描消费（= **F38** 的另一半）。
      验收：`import "std/fs" as fs;` + 一次 `fs.read_to_str(…)` 的模块在 `infy check` 里 exit **0**（`std` 是 §5.2 的强制依赖、§5.3 第 2 步的虚拟 module），
      成对锁点 = 有 native 的项解析成功、`std.net` 的三条仍按 D-50(b) 显式拒绝（**不许**因为接了绑定就把 net 变成假成功）；
      改动时把 `cli::tests::a_std_package_call_is_refused_and_never_faked` 的期望码集与 D-50(b) **一起**改掉，不许只放宽测试。
- [ ] F41 **`dyn` 上转（强制转换）在阶段 [4] 缺失 ⇒ `dyn Trait` 唯一的构造入口不通**（F4 轮实测）：
      `let s: dyn Shape = Sq{ side: 1.0 };` 报 `E-0110 绑定类型不匹配：声明 Shape，实际 Sq`（诊断还把类型打成 `Shape` 而不是 `dyn Shape`），
      而 §4.1 明写 `dyn Trait` 是胖指针、§4.14 给了对象安全判据（`E-0167`）—— 规范要的是"具体类型 → `dyn T`"的隐式上转，
      本实现的 `sema::type_compatible` 只有 `(Ty::Dyn(d), Ty::Adt(a)) | (Ty::Dyn(d), Ty::Named(a)) => d == a`（`sema.rs:2290`）这一条**同名**判据，
      没有任何装箱/上转臂。⇒ 与 D-50(c) 是两件事：**偏移表已定，缺的是"造出一个 dyn 值"的入口**，
      不许把 F41 当成"vtable 发射未实现"的重复登记。
      验收：`let s: dyn Shape = Sq{ side: 1.0f32 };` 在 `infy check` exit 0 且 `E-0167` 的对象安全检查照旧生效（负例 = 泛型方法的 trait 仍被拒），
      随后 `infy build` 仍按 D-50(c) 以条款号拒绝发射（= `codegen::tests::a_dyn_type_is_refused_at_build_by_name` 的形状）。
- [ ] F42 **`impl` 方法体里 `self.field` 恒 `E-0110` ⇒ 方法在源码里不可写**（F4 轮实测）：
      `struct Pt { pub x: u32 }` + `impl Pt { fn get(&self) -> u32 { self.x } }` ⇒ `error[E-0110]: 无法解析字段 'x'`，
      `&self` / `&mut self` / `self` 三种接收者**全中**（`mut self` 另吃 `E-0020 保留字作参数名`）；诊断的 note 还错指到 §4.6"env 字段集合封闭"。
      根因（读码定位，非猜测）：`check_body` 用 `self.ty_from_ast(&p.ty, …)` 直接声明形参（`sema.rs:960-962`），
      接收者类型因此是没替换过的 `Ty::Named("self")` / `Ref{Named("self")}`；`Self`→impl 类型的替换只在
      `register_fn_for_impl` 里做（`sema.rs:796-802`），而 `self.self_ty` 在 `812` 设、`822` 清，`ExprKind::Ident("self")`
      的回退（`sema.rs:1418`）在体检查时已经是 `None` ⇒ `Ty::Invalid`。
      ⇒ 这条挡住的是**一切带字段的 impl**（§14 导览没有 impl 方法体，所以从没走到）。验收：上面那段最小复例 `infy check` exit **0**，
      并补一条 `trait` 版（`impl Shape for Sq { fn area(&self) -> f32 { self.side * self.side } }`，即 §14/§7.7.1 样例里那种写法）成对锁点。

## 2. 当前指针

> **F4 轮收尾（2026-10-16）：F4 勾 `[x]`，本段取代下一段** —— 走的是验收允许的**记录**路（裁定 = 偏差 **D-50(a)–(c)**，
> 4 条锁点 + 1 次实测负对照）。**为什么不走实现路**：附录 C-7「仍缺 ④」把 LSP 的增量诊断/§9.4 逐字节一致性定义为空白，
> §10.4 又要求 TLS 后端内置在 `.inf rt` 段而 §1.3 禁止第三方依赖 ⇒ 这两件在规范/约束改口之前只能记录；
> vtable 那件本轮**定死了偏移表**（`docs/internals/runtime-abi.md` §1，唯一真值 = `hir::Layouter::vtable_record`，
> 文档是被读回来比对的那一侧）。
> **下一轮 = §1 第一个 `[ ]` = F5**（`infy fmt` / `infy expand` 的目录参数与 `infy check` 不一致）。起手现成件（本轮已定位，不必重审）：
> 失败点 = `cli.rs:1616 fmt_cmd`（`1641` 那句 `error: 无法读取 …` 就是把目录当文件 `read` 的现场）与 `cli.rs:1711 expand_cmd`；
> `check` 那条走的是 `load_scoped()` 的 module 根发现（`MODULE.infy.toml`），三条命令要一致就得让 fmt/expand 也接同一套发现，
> 而不是各写一份。验收按 F5 原文：三条命令各带目录参数行为一致 + 进 `src/cli.rs` 的测试。
> **本轮实测出三条真缺陷，已按顺序登记在 F39 之后：F40 / F41 / F42**（F40 = 任何 `import "std/…"` 都不产生绑定 ⇒
> 整个 §10 面从源码够不到，`cli.rs:330` 的 `ImportSource::Std => {}` 空臂是正主，也是 F19(a) 的上游；
> F41 = `dyn` 上转缺失；F42 = `impl` 方法体里 `self.field` 恒 `E-0110`，根因已读到行）。**别跳指针去抓它们**，
> 但 F40 与 F5 都要动 `load`/阶段 [3]，接 F5 时如果顺手看清了绑定面，就把 F40 的复例贴在 RUNLOG 里，不要半修。
> **一条口径给下一轮（本轮自伤一次记下的）**：`rt_tests` / 两份 IFBC 二进制**必须在 `target/scratch` 下运行** ——
> `test_rt.c:1501` 的夹具路径写死 `../../runtime/fixtures/…`，在仓库根跑会 segfault + 一串假 FAIL（看着像回归，其实是 CWD）。
> **基线（本轮收尾复跑）**：`cargo test --offline --lib` **535 passed / 0 failed**（= 531 + 4）、
> `cargo check --offline --lib` **0 error / 0 warning**、`cargo build --offline --release` exit 0、
> tour 四条 + `--verify-reproducible` 全 exit **0**（本机产物仍 **199975** / C11 **12513**）、rt **32335/0**、
> 两条 IFBC **180/180** + `cmp` 逐字节相同、`infy conformance --report` 仍 exit **1**（83 = 77/0/6 · 92.8%）、`fmt --check` 仍 exit **2**（F6）。
> 措辞红线不变：「两侧装载期校验等价」仍不许讲（F26 前半 / D-42 (a)–(e)）；覆盖 83/2300 ⇒ 任何文档
> 不得出现「INFY v2.0 兼容」的肯定式声称（逐物理行判，否定标记必须与声称同一行）。
> **F4 勾 `[x]` 不声称的三件**：`infy lsp` 能用、`std.net` 能调用、跨 `.infib` 的 `dyn` 链接成立 —— 三条都是
> **明确未实现**（D-50），本轮只把"拒绝的形态"钉住。
> **（以下为 F4 轮之前各轮的指针，已被上面一段取代，保留作历史）**

> **cron-26 轮收尾（2026-10-16）：F1 已勾 `[x]`，本段取代下一段（同轮的「F1 回到 `[ ]`」那段）** ——
> 含 EOL 次序修复的那个 commit（`c7312eb`）触发的 **run `38912466381`（run_number 4）= `success`，步 3-12 逐条 `success`**，
> 远端打印 `core.autocrlf = true`（守卫在真条件下跑）；远端实数：步 7 **531 passed / 0 failed**、步 8 告警 **0**、
> 步 9 两份 `.c` 各 **0 告警**、步 10 rt **32335/0**、步 11 两条 IFBC **180/180** + `cmp`、步 12 tour 四条 +
> `--verify-reproducible`（**221666** 两目录相同）+ `artifact info --nodes` 全 exit 0；日志 957 行、`FAILED`/`panicked` **0** 处。
> **验收读法定死（防下一轮拿"做账提交也要绿"无限自指）**：勾 `[x]` 认的是**包含该轮交付的那个 commit 触发的 run**；
> 之后的纯文档 run 只是确认。**任何一次 run 变红就重新打开本条。**
> **下一轮 = §1 第一个 `[ ]` = F4**（`infy lsp` / `std.net` TLS / `dyn Trait` vtable 偏移表：要么实现，要么在
> `docs/spec-deviations.md` 保持"明确未实现"记录）。起手现成件见下一段末尾（本轮 grep 过，不必重审）。
> 一条**已量过、留给下一轮判断**的待办：CI 步 3 的判据现在是 `git status --porcelain`，本轮实测它在两个方向上都不可靠
> （run #1 型：改写了字节却"干净"；本轮 run #2 型：该报的会因 racy-clean 漏报）。要不要换成内容级判据由下一轮定，
> 但**别**为了让它绿而放宽（经验库 [[checkout-eol-is-an-executed-surface]] 第 5 形态）。
> 措辞红线不变：「两侧装载期校验等价」仍不许讲（F26 前半 / D-42 (a)–(e)）；覆盖 83/2300 ⇒ 任何文档
> 不得出现「INFY v2.0 兼容」的肯定式声称（该测试**逐物理行**判，否定标记必须与声称同一行）。
> **（以下为 cron-26 轮前半/中段与 cron-25 轮的指针，已被上面一段取代，保留作历史）**
> **cron-26 轮后半（2026-10-16）：F1 回到 `[ ]`，本段取代下一段（同轮前半那段「F1 勾 `[x]`」）** ——
> 因为**做账提交自己触发的 run #3（`38910330820`）= `failure`**，红在 CI 步 3「检出没改写工作树」，
> 而根因是 **F1 的修法本身有缺陷**：`.gitattributes` 后匹配优先，全局 `*.infy text eol=lf` 写在
> `conformance/cases/** -text` 之后 ⇒ 盖掉语料例外 ⇒ 故意带 **6 个 CRLF 对**的 `2-lexical-0002/main.infy`
> 被 clean 过滤器抹成 LF ⇒ 归一化 ≠ blob ⇒ **在 autocrlf=true 的检出里永久 dirty**。
> 本机 `git clone` 到 `target/scratch/eolprobe` 复现（`porcelain` 恰这一条）；对照组 `2-lexical-0001` =
> **1 个裸 CR / 0 个 CRLF 对** ⇒ 不脏（远端也只报一个文件）。**字节没被 smudge 改写**（工作树仍是 6 个 CR）
> ⇒ 用例语义还活着 ⇒ **run #2 步 3 没红 = 运气**（**观察**：同一棵树、同一份属性表，run #2 绿而 run #3 红，且隔离探针里
> `git status` 的读数随时间翻转 ⇒ 守卫的结论不由内容单独决定；**机制归因 = 推断**，stat 缓存可信时 `git status` 不比内容
> （racy-clean 那一族），本轮没直接量到 git 内部）⇒ **「守卫绿」≠「树干净」**，这句是给下一轮的判据、不是给 CI 加冕。
> **修法**：语料例外的 `-text` 重新声明在全局规则**之后**（三行：`conformance/cases/**`、`**.infy`、`**.infy.toml`）；
> 新测试 `the_corpus_eol_exception_is_declared_after_the_global_infy_rule` 两头钉（次序 + 工作树里两条正反例的形状），
> **两条负对照都实测变红**（翻次序 → 红并报行号；抹 0002 的 CR → 红并报「0 个 CRLF 对」），`cmp` 验证复原。
> **新基线**：`cargo test --offline --lib` **531 passed / 0 failed**（+1 = `conformance` 18 → **19**）、
> `cargo check --offline --lib` **0 error / 0 warning**、`cargo build --offline --release` exit 0、
> tour 四条 + `--verify-reproducible` 全 exit 0（本机产物 **199975**）、C 四链复跑（两份 `-c` 各 stderr **0 字节**、
> rt **32335/0**、两条 IFBC **180/180** + `cmp` 无差异）、`conformance --report` 仍 exit **1**（83 = 77/0/6 · 92.8%）、
> `fmt --check` 仍 exit **2**（F6）。
> **下一轮第一件事 = 回查 run #4**（本轮修复推上去之后由推送触发；`gh api …/actions/runs?branch=toolchain`，
> 逐步结论用 `…/runs/<id>/jobs`，日志用 `gh run view <id> --job <jobId> --log`）：**绿了才把 F1 勾 `[x]`**，
> 红了按逐条根因处置、**不许**为了让守卫变绿而放宽步 3（也不许把 199975 那种本机常数写成远端断言，见下一段那条口径纠正）。
> F1 之后 §1 第一个 `[ ]` = **F4**（`infy lsp`、`std.net` TLS、`dyn Trait` vtable 偏移表：要么实现，要么在
> `docs/spec-deviations.md` 保持"明确未实现"记录）。起手现成件（本轮 grep 过，不必重审）：`infy lsp` 已在
> `cli::NOT_IMPLEMENTED` 名单里（显式拒绝，不是假成功）；`stdlib.rs:64-66` 的 `std.net.*` 三条 `native: None`；
> `hir::vtable_layout` **已实现且有测试**（`drop`/`size`/`align` + 方法指针 = `24 + 8n`，`hir.rs:656`），
> 缺的是 `codegen.rs:670` 与 `:2395` 的发射（两处都已是显式 `CgError`，不是静默）⇒ vtable 是三件里唯一有真实落点的。
> 一条待办留给下一轮**判断**（本轮不扩大改动面）：步 3 的守卫要不要从 `git status --porcelain` 换成
> 「按 `git diff --no-textconv --binary` 或直接查字节」这种**内容级**判据 —— 本轮实测已经证明 porcelain 会漏报
> （racy-clean），这与经验库 [[checkout-eol-is-an-executed-surface]] 的「porcelain 空≠字节没改写」是同一件事的两面。
> 措辞红线不变：「两侧装载期校验等价」仍不许讲（F26 前半 / D-42 (a)–(e)）；覆盖 83/2300 ⇒ 任何文档
> 不得出现「INFY v2.0 兼容」的肯定式声称（这条是 `no_markdown_doc_claims_the_compatibility_name` 的判据：
> **逐物理行**看，所以否定标记必须和那句声称**同一行**，换行就红）。
> **（以下为 cron-26 轮前半与 cron-25 轮的指针，已被上面一段取代，保留作历史）**
> **cron-26 轮（2026-10-16）= F1 勾 `[x]`，本段取代下一段的「下一轮第一件事：还是先补推 `toolchain`」**：
> 开头 `scripts/push-branch.ps1` 第 **1** 次即 exit 0 ⇒ 远端 `toolchain` = `6efb926` = 本地 HEAD、现算欠账 **0**
> （cron-24/25 两批全败欠的 **7** 个提交全清；**没**动 git 配置、**没**换凭据，`github.com:443` 这轮不抖）。
> 推送当场触发的 **run `38908208089`（run_number 2 / event push / head_sha `6efb926`）= conclusion `success`，12 步逐条 `success`**，
> **run #1 里 skipped 的步 8-12 第一次在远端跑且全部拿到实数**：check 告警 **0** · 两份 `.c` 各 0 告警 · rt **32335/0** ·
> IFBC 两条链各 **180/180** + `cmp` 过 · tour 四条 + `--verify-reproducible` + `artifact info --nodes` 全 exit 0 且
> **节点表 8/9/10/11/16/19 与 §8.4 金样逐行相同**、C11 **12513** 字节与本机相符；步 7 `cargo test --offline` =
> **530 passed / 0 failed** ⇒ run #1 那三条 `W-0001` 假红消失（EOL 锁在远端实测生效）。日志已取回本机
> （`gh run view --log` → 956 行，`FAILED`/`panicked` 命中 **0** 处），下列数字全部取自该日志而非引用上一轮。
> **两条只认远端的供给事实**：runner 上 `gcc` 在 PATH（步 5 success）、`setup-python@v7` 给的是 **CPython 3.14.8**（本机 3.14.6）
> 而 `btrack::tests::py_unit_fixture_is_exactly_what_the_ingest_produces` 在它上面仍 `ok` ⇒「B 轨夹具只对本机那个 patch 成立」不成立。
> **一条要传给下一轮的口径纠正**：远端 tour 产物 **221666 字节 / build-id `e39398bc…a5be`** vs 本机一直记的
> **199975 / `4d8250b5…55ba`** ⇒ **产物字节与 build-id 是宿主 `cc` 的函数，不是跨机常数**（远端 `.inf meta` 自带
> `source_toolchain gcc 15.2.0`，本机 MSYS2 是 16.2.0；镜像 220864 vs 199173，而**段 blob 738 + 段尾 64 两侧逐字节相同**）。
> CI 对这些数**只打印不门禁**是正确设计，别"顺手补上"；把 199975 当远端断言 = 把 CI 永久钉红。
> **下一轮 = §1 第一个 `[ ]` = F4**（`infy lsp`、`std.net` TLS、`dyn Trait` vtable 偏移表：要么实现，要么在
> `docs/spec-deviations.md` 里保持"明确未实现"的记录）。起手现成件（本轮 grep 过，不必重审）：`infy lsp` 已在
> `cli::NOT_IMPLEMENTED` 名单里（显式拒绝，不是假成功）；`stdlib.rs` 的 `std.net.*` 三条 `native: None`；
> `hir::vtable_layout` **已实现且有测试**（`drop`/`size`/`align` + 方法指针 = `24 + 8n`），缺的是 `codegen.rs:670/2395`
> 的发射（两处都已是显式 `CgError`，不是静默）。措辞红线不变：「两侧装载期校验等价」仍不许讲（F26 前半 / D-42 (a)–(e)）；
> 覆盖 83/2300 ⇒ 任何文档不得出现「INFY v2.0 兼容」的肯定式声称。
> **（以下为 cron-25 轮的指针，已被上面一段取代，保留作历史）**
> **下一轮第一件事：还是先补推 `toolchain`**（F3 轮开头走 `scripts/push-branch.ps1`，两次全败 = 第 1 次
> `Recv failure: Connection was reset`、第 2 次 `Failed to connect to github.com:443 after 21071 ms`，而同一时刻
> `gh api` 正常 ⇒ 仍是 `github.com` 侧抖动，**不动 git 配置、不换凭据**；远端 `toolchain` 回查仍 = `1896c2a`
> ⇒ **EOL 修复还没上远端、run #2 还没被触发**。欠账每次**现算**（`gh api …/git/refs/heads/toolchain --jq .object.sha`
> + `git rev-list --count <远端SHA>..HEAD`），别抄固定数），再用 `gh api …/actions/runs?branch=toolchain` 取 **run #2**
> 的逐步结论 ⇒ 做到了才把 F1 勾 `[x]`。**「runner 供给」那两条疑问 run #1 已经答过了**（步 1-6 全 success ⇒
> `windows-latest` 有 `gcc`、`setup-python@v7` 装得上 CPython 3.14），run #2 真正要看的是**步 8-12**：它们在 run #1
> 里因 `set -e` 全 `skipped`，**在远端从没跑过一次**（`cargo check` 告警数、两份 `-Wall -Wextra -c`、rt harness 的
> `cd target/scratch`、IFBC 两条链 + `cmp`、tour 四条 + `--verify-reproducible` + `artifact info --nodes`）。
> 红了按逐条根因处置、照实记，**不许**放宽断言。之后就做 §1 的第一个 `[ ]` = **F4**
> （`infy lsp`、`std.net` TLS、`dyn Trait` vtable 偏移表：要么实现，要么在 `docs/spec-deviations.md` 里保持"明确未实现"的记录）。
> **F3 轮（2026-10-16）已勾 `[x]`**：`.gitignore` 改成按**产物形状**列（`*.log` + `logs/` + `*.tmp` + `*.o` +
> `/rt_tests` `/ifbc_tests` `/ifbc_real` `/a.out` `/chunk.txt`），5 项仍在跟踪的一次性产物由新脚本
> `scripts/untrack-artifacts.ps1` **只摘索引**（`git rm --cached` ⇒ 文件留磁盘、逐条写回滚命令；干跑命中 5 拒绝 0、
> `-Apply` exit 0、复跑 `NOOP` exit 0、`git ls-files` 产物形状 = 0 行），新测试把名单**两头**钉住
> （六条必需整行相等 + 十四种禁止形状不许出现 + 每个 `.ps1` 的 log 必须在 `logs/`）= `conformance` **17 → 18**、
> 基线换为 **530 passed / 0 failed**、check **0 error / 0 warning**（口径未变）。证据 = `docs/coverage.md` **P31**
> 与 `docs/status.md` 的 F3 一节。**一条实测掉的假设**：CRLF 版 `.gitignore` 在隔离仓库里模式照常命中 ⇒ 没给它补
> `text eol=lf`。`src/` 生产代码一字未动 ⇒ 12513 / 199975 / `4d8250b5…55ba` 逐字节不变是**预期**，四条 C 链仍复跑未变。
> **F1 轮（2026-10-16）= 落地但仍未勾 `[ ]`**（远端回查没做到）：`.github/workflows/ci.yml` 落成 = `cargo build --offline --release` + `cargo test --offline` + `-Wall -Wextra` 的 C 侧编译自检（stderr 非空即红）+ rt/IFBC 两份 harness（IFBC 两条链 + `cmp`）+ tour 端到端 9 个 run 段，全部在本地按 GitHub 的执行方式逐步 exit 0；`conformance` **14 → 17** 把 yaml 的命令覆盖面、四个重写夹具的开关、Windows-only 与两条防假红守卫钉住。基线 **527 passed / 0 failed**、check **0 error / 50 warning**（只报不门禁）、`infy conformance --report` 仍 83 = Pass 77 / Fail 0 / Blocked 6 ⇒ **exit 1**、`fmt --check` 仍 **exit 2**（F6 未裁决）。证据 = `docs/coverage.md` **P30** 与 `docs/status.md` 的 F1 一节；远端 run 的结论只认 RUNLOG `cron-23-push` 行的 `gh api` 回查。
> **E3 轮（2026-10-16）已勾 `[x]`**：§12 逐分类通过率落进 `docs/coverage.md` **§6**（20 行 × 11 列 + 三档小计 + 合计 + 缺口三分类）与 README 的「距 §12 的 100% 还差哪些类」，副本配了逐格回核 + 红线机检 ⇒ `conformance` **11 → 14**、基线 **524 passed / 0 failed**、`cargo check` **0 error / 50 warning**（HEAD 全量重检的告警**集合**与工作树逐条 `diff` 相同）、`infy conformance --report` 的数字**一字未变**（83 = Pass 77 / Fail 0 / Blocked 6 · 92.8% · 覆盖 83/2300 ⇒ exit 1）。证据提交 `008bd93`。
> **F2 轮（2026-10-16）已勾 `[x]`**：`cargo check --offline --lib` 的告警 **50 → 0**（0 error，未用 `#[allow(...)]` 消数、未新增），基线换为 **529 passed / 0 failed**（+2 = `cli` 39、`btrack` 31）。逐条去处 = `docs/status.md` 的 F2 一节，对外行 = `docs/coverage.md` **P1/P2/P30**；两处判成「没接上」的已接线 = 偏差 **D-49**（§9.6 的 `[lints]`）与 `btrack` 的 ZIP 成员偏移（`lho` → `data_off`，带**负对照实测**）。
> F2 轮留下的**新欠账**（四条都量过，不用重审）：
> (1) **F2 的口径只覆盖 `--lib`** —— `cargo build --offline --release` 与 bin 目标里若另有告警，本轮没逐条清（release 构建本身 exit 0）。
> (2) **`primary` 的臂顺序仍无回归锁** —— 死臂删了，但「整数字面量 = `ExprKind::Lit(Lit::Int)`」这句话今天没有测试钉住（= F9 验收的第二半）。
> (3) **§9.5 第 2 条的 tab 测量半边** = 新登记 **F39**（删掉的 `expand_tabs_for_width` 是它的残骸，不是它的实现）。
> (4) 一处入口顺序问题仍未动：`codegen::emit_driver` 在 `chains.is_empty()` 时直接 `return 1;`，而 `HirEntry::Main` 那条分支在它**后面**（D3 轮记的）。
> **E3 轮的现成件（别再造）**：`docs/coverage.md` §6 = 逐分类数字的对外副本（§1 的证据命令表仍按 P 编号引用）；`conformance::tests` 的三条文档回核/红线测试 = CI 该跑的红线看门狗；E2 的 `cli::NOT_IMPLEMENTED` / `conformance_report_text` / `conformance_exit` 不变；跑用例集的三条测试共用 `CORPUS_SERIAL` 锁（`run_case` 的 scratch 目录只按用例 id 命名，同进程并发会互删 module，现场症状是**语义码** `E-0600` 而不是文件锁错误）。
> **E2 轮（2026-10-16）已勾 `[x]`**：`infy conformance --report` 接进 `cli.rs`，实测 20 行逐分类 + 三档小计 + 合计 ⇒ **exit 1**（裁定 = 偏差 **D-48 (a)–(g)**）。
> **E2 轮新增的常驻面（别再造）**：`cli::NOT_IMPLEMENTED`（拒绝臂名单常量，`conformance_is_no_longer_a_refused_command` 钉它
> —— 拒绝臂与已接线命令共享 exit 1，只有名单能让"命令又躺回拒绝臂"变红）；`cli::conformance_report_text(ran, selected)`
> 是唯一渲染入口（空分类的 `n/a`、`defect=`、"不得声称兼容"这些**措辞本身**被测试逐子串断言）；
> `cli::conformance_exit(fail, blocked, short_rows)` 把退出码规则与用例集状态**分开**钉（规则变了必须同时改 D-48(a)）。
> **E2 轮的实测事实**：命令打的三档数字与 `conformance::tests` 在测试里算的逐位相同 = 同源要求（D-47(h)：运行器只有
> `cli::check_verdict` 一条路径），不是巧合；本轮**零实现缺陷修复**（`conformance::*`、前端、C 侧一字未动）。
> **E1 轮留下的现成件（别再造）**：`cli::check_verdict(root, scope) -> Option<Verdict{codes, exit, rendered}>`
> = `infy check` 的同一条链（`load_scoped` + `analyse` + 干净时 `ingest_b_track`），E3 也不要写第二套前端；
> `CATEGORIES`（20 行）与 `CATEGORY.toml` 的双向锁（改表 → `INFY_REGENERATE_CATEGORIES=1` 重渲染；
> 不改而漂 → 加载失败）；`conformance::tests::probe_corpus_dump_verdicts` 是**作者工具** =
> `INFY_CONFORMANCE_PROBE=1 cargo test --offline --lib -- --nocapture conformance::tests::probe_corpus`
> 每行打 `id / 实得码 / exit / 结论`，新用例的期望先在这里量、再对照它声称的条款**审**，不许盲抄。
> **E1 轮新登记的缺口 F34–F38**（E3 只是把它们列进缺口表，别当已完成）：F34 `Vec::clone` 不在 §10 表里；
> **F35 任何 derive 都过不了 `infy check`**（生成体调 `std.fmt.format_*` 而 std 表没有 ⇒ derive 的 57 项单测
> 全是文本断言、生成体从未被 [2]-[4] 回读，`examples/tour` 也没写 derive）；F36 同一根因两阶段各报一次；
> F37 未知 `@` 前缀级联 8 条 `E-0053`；F38 §1.3 领域扫描零调用点 + `std/domain_terms.txt` 不在仓库。
> **用例格式的坑**：`.gitattributes` 的 `conformance/cases/** -text` 别删（`core.autocrlf=true` 把行尾变成
> CRLF，而 §2.1 的 `W-0001` 就把行尾当被测面 ⇒ 每条用例多一条诊断）；B 轨用例的清单写 `@PY_MINOR@`
> 而不是硬编码 minor（否则用例集在测机器而不是测实现）；`expect = []` 必须显式写（空是声称、缺是漏）；
> 编号 = 目录名且**只增不复用**（§12 明令"已有用例的编号与期望不得改变"）。
> **D3 轮留下的 A 轨余项（E1 用不到，但别当已完成）**：**F31**（`#[repr(C)]` struct / ADT 不透明指针没有
> 跨边界落点 ⇒ §7.6.1 样例自己的 `-> *mut DecHandle` 过不了 = 规范样例不能构建）、
> **F32**（§7.6.2 工具链发现/注册、`recipe.toml` 审计、版本核对 `E-0604` 与 `--allow-toolchain-drift` 全未接，
> A 轨只驱动 `--cc`/`$CC` 那一个 `cc`；`#[link(toolchain=…, flags=[…], isolation=…)]` 三个参数**读出来但没有消费者**）、
> **F33**（§7.6.3 项 3 非标量 thunk、项 5 外部行号→INFY 位置、§7.6.1 的 `W-0720` 与"声明了但未导出 →
> 链接期 `E-0604`"映射、§7.6.5 `E-0724`、§7.6.6 `isolation = "process"`）。
> **D3 轮的现成件（别再造）**：A 轨的命名 / 类型表 / 产物文本一律从 `atrack::*` 取（`wrapper_symbol`、
> `normalized_block`、`boundary_c`、`decl_gaps`、`binding_header`、`unit_c_source`、`pair`、`wrapper_plan`）——
> `codegen` 与 `cli` 从同一批函数取，所以头里的原型与 `ci.c` 的调用点不可能不一致；
> `sema::FnInfo::link_unit` 与 `hir::HirFn::link_unit` 是"这条声明属于哪个 `#[link]` 单元"的唯一来源。
> 加 A 轨源会**动**链接输入 ⇒ 产物字节与 build-id 会变（预期变化，须同步 status/coverage/README/本节）；
> 不加 A 轨源则 12513 / 199975 / `4d8250b5…55ba` 必须逐字节不变，D3 轮已按这条验过。
> **仍有效的 D3 手法（写 A 轨相关的任何东西前先看）**：判据"写死 / 恒假 / 占位"那一族本轮清了三个 ——
> `sema::in_extern_block`（恒 `false`，且它只在顶层 `;` 体那条路上被读，答案本来就是 `false` ⇒ 删除而不是
> "修好"）、`E-0721` 的 `link_names().iter().any(|_| true)`（与"被调者是不是 A 轨"无关 ⇒ 换成 `is_foreign`）、
> `hir::HirFn::signature_text`（零调用点 ⇒ 现在是包装哈希的输入）。认法见经验库
> `infy-unreachable-implementation-branch`。缺 `cc` 时先查 PATH（`gcc` 在 `/c/msys64/ucrt64/bin`）。
> **D2 轮的现成件（别再造）**：`infy ifbc dump|verify` 已可用（P22/P23/P24），§12.3 的黄金文件现在能直接从
> 命令行取；`--canonical` 是双侧共用文本且此时 stdout 是纯的 ⇒ **E2/E3 的期望文件可以直接用命令生成**，
> 不必再进 C 目录跑 harness；要与 C 侧对表形状时，`docs/internals/ifbc-encoding-rust.md` 与
> `runtime/IFBC.md` 那两份镜像表是参照。
> **仍然不许讲的话**："两侧装载期校验等价"（**F26 前半**，清单 = D-42 (a)–(e)）—— D2 的实现把这句 note
> 直接打进命令输出了，措辞照 `docs/coverage.md` 的"被测对象是生产运行时吗"那一行。
>
> **D2 轮实测（本轮复跑，非引用）= 新基线**：`cargo test --offline --lib` **468 passed / 0 failed**
> （基线 460，+8 全在 `cli`：20 → **28**；全分布 `lexer` 79 · `derive` 57 · `fmt` 39 · `macro_expand` 35 ·
> `btrack` 30 · `cli` **28** · `pkg` 24 · `sema` 22 · `ifbc` 22 · `codegen` 22 · `hir` 20 · `artifact` 18 ·
> `toml` 15 · `borrow` 15 · `sha256` 12 · `codes` 9 · `parser` 8 · `base64` 7 · `stdlib` 4 · `diag` 2 = 468）；
> `cargo check --offline --lib` 0 error / **50 warning**（未新增）；`infy check|build|run|test examples/tour`
> 全 exit 0、`--verify-reproducible` exit 0、`infy artifact info … --nodes` exit 0、生成 C11 仍 **12513** 字节 /
> 产物仍 **199975** 字节 / `blob 738` / `build-id 4d8250b5…55ba` 逐字节未变（本轮没碰 [7]-[9]）、
> 生成 C 的 `gcc -std=c11 -Wall -Wextra -I runtime -c` exit 0（0 error / 0 warning）、
> `(cd examples/tour && infy fmt --check)` 仍 exit **2**（F6 未裁决）；**C 侧零改动**（`.c`/`.h` 一行没动）
> 仍复跑四条链：`-c infy_rt.c` 与 `-c infy_ifbc.c` 各 **0 告警** exit 0、rt harness **32335/0**
> （必须在 `target/scratch` 里跑）、替身链与生产链 IFBC 各 **180/180** 且两份 stdout `cmp` **无差异**。
>
> **D2 轮新增的常驻面**：
> (1) `cli.rs` 的 §7.8.4 检视层：`IfbcInput{images, container}` / `IfbcOpenErr::{Neither, Framing}` /
>     `classify_ifbc`（先容器后裸镜像；容器只接受"恰好 1 个模块"）/ `ifbc_dump_text(image, canonical)` /
>     `ifbc_verify_image(image, all_errors)` / `ifbc_verify_canonical_text(images)` /
>     `ifbc_empty_region_text(sub, n)`（空区那两行做成**函数**，因为措辞本身就是断言对象）/ `ifbc_cmd`。
> (2) 测试件：`cli::tests::{fixture_bytes, fixture_path, container_with_ifbc, scratch_file}` ——
>     `container_with_ifbc` 用 `artifact::write` 现造带非空 `.inf ifbc` 的容器（今天没有任何真实构建会写它），
>     这是命令的"产物路径"唯一可测形态；F29 落地后这些测试不必改。
> (3) 裁定 = 偏差 **D-45** 的 (a)–(e)：输入形态与多模块点名拒绝 / `--canonical` 是哪份文本且 stdout 必须纯 /
>     退出码归属（§7.8.3 阻断 → 2、读不出的文件 → 7 且两条理由都打、用法错 → 1）/ host 表取
>     `HostSigTable::std_default()`（`src/host.rs` 是占位 ⇒ 校验的是"镜像对 std 表"）/ "首个"裁成首个**失败函数**。
> (4) 本轮**没有**修掉任何实现缺陷（`ifbc::*` 与 `artifact::*` 行为一字未动，只加命令层）；记一条实测事实：
>     `infy ifbc verify runtime/fixtures/ifbc_all_ops.bin` 本侧报 **26** 条阻断而 C 侧对同一镜像不报同一集合
>     （该夹具双侧只比过反汇编）⇒ 这就是 F26 前半的具体体量，别把它当测试写错、也不要去放宽本侧判据。
>
> **仍然有效的接手要点（逐条沿用，不要再审计）**：
> (旧 1) CPython **3.14.6** 在位；`INFY_PYTHON` 覆盖解释器路径（`find_python`）。本机无 3.13 / lua。
> (旧 3) **预期的显式失败，不要去绕**：`RERAISE` 未映射 + `f32` 与无后缀字面量混宽算术被自检拒 ⇒
>     §7.7.1 的推导式样例过不了扫描（**F30**）；`import.attr`/`inst.get`/`inst.set`/`seq.iter`/`seq.next`/
>     `new.set` 六条装载通过而执行 `R-0710`（**F27**）；`new.set` 没有元素个数落点（**F25**）；
>     `call.virt`/`call.embed`/`invoke.dynamic` 本侧判 `R-0705`（**F26**）。
> (旧 5) `cli.rs` 可复用面：`frontend(positional, rest, scope)`、`load_scoped`、`run_module`、
>     `forward_program`、`codegen::Config`（加开关同时给 `Default`）、`codegen::tests::emit_with(src, cfg)`。
> (旧 6) "哪些文件参与构建"只有一个入口 = `pkg::BuildConfig` + `pkg::file_selected`；未知名 `E-0012`
>     在选择之前对每个 discovered 文件做；过滤 `Loaded::files` 后要顺着查 dirhash / build-id / lock。
> (旧 7) **第一次执行 / 装载 / 接线必然现形**：D1(2) 与 D2 各再一次实证 —— `btrack` 30 项单测全绿对
>     "CLI 能不能走到它"没有预测力，`ifbc` 22 项 + C 侧 180 断言全绿也对"`infy ifbc` 这条命令存不存在"
>     没有预测力（§13.4 列了它、§7.8.4 承诺了它、§12.3 把它当黄金文件，而它一直躺在"未实现"臂里）。
>     认法与手法已并进经验库 `infy-unreachable-implementation-branch`（D2 轮又更新了一次）。
> (旧 8) 手法：改 `infy_ifbc.c`/`infy_rt.c`（哪怕只改 `.h`）就复跑四条 C 链；二进制产在 `target/scratch/`
>     并从那里跑；编 C 测试**四个 `.c` 一起给**；跨语言一致性靠逐字节夹具 + 双侧差分文本。
> (旧 9) `infy test` 的 harness 输出在 Windows 上是 CRLF（归一后逐字节比），而 **IFBC 规范化文本是 LF**
>     （按原始字节比）；`.gitattributes` 把 `runtime/fixtures` 钉成 `binary`/`-text`，别删。
> (旧 11) `core.autocrlf=true` 这台机上别用文本模式脚本整篇重写文件；收尾看 `git diff --numstat` 的删除列。
> (旧 12) **D1(2) 轮自己犯的错**：向大文件插入整段时把"锚点行"当成边界 ⇒ 函数体被拼到同一行
> （`fn f() {        let mut …`）、以及同一段测试被插入两次。修法 = 按行号切片 + 二进制写回，
> 改完立刻 `grep -c` 数函数名并跑全量 `cargo test`；已写进经验库 `feedback-edit-insert-deletes-lines`
> （user 域）与 BACKLOG §0 的纪律之外，不新增规则。
> (新) **D2 轮自己犯的错（同一族的两个新形态）**：① 把 Edit 的 `old_string`/`new_string` 内容弄反
> （拿"要插入的新代码"当锚点）⇒ 报 0 occurrences，白跑一次；② 长 `println!` 多行字符串字面量
> 当锚点时把续行 `\` 的数量抄错 ⇒ 同样 0 occurrences。**D2 轮改用"短锚点 + 行号切片"两种手法**：
> 插入大段用 python 按行号 splice（`docs/BACKLOG.md` §2 的 60→54 行就是这么换的），改完立即
> `git diff --numstat` 看删除列并逐条核对被删的行（本轮 cli.rs 9 / status.md 11 / BACKLOG 全部对得上）。
>
> **推送欠账**：只认现算 `git rev-list --count <远端SHA>..HEAD`；远端 SHA 用
> `gh api repos/YuanShenNiuBi1314/infinity-lang/git/refs/heads/toolchain --jq .object.sha` 回查，
> `Everything up-to-date` 不算成功。**cron-20 轮开头第 1 次推送即成功**（`1bc2206..ea76ab9`），`gh api` 回查
> 远端 = 本地 HEAD ⇒ cron-19 欠的 **3** 个提交（`e669834` + `b785421` + `ea76ab9`）已清，
> 本轮的提交推完再现算。
> `github.com:443` 仍是**间歇**不通而不是坏：C4/C5/D4(1)/(2)/(3)/cron-17/D2
> 都第 1 次即成、C3 第 6/7 次、cron-12/cron-16/cron-19 全败 ⇒ 先推一次、失败再试一次即停，连败时首个候选
> `git -c http.version=HTTP/1.1 push origin HEAD:toolchain`（cron-19 两条都试过，都败）。
>
> 未修只登记：D-18、D-19/F7、F8、F9/F10/F11、F12…F15、F16/F17/F18、F19/F20、F21/F22/F23/F24、F25、
> F26（"把 verify 结论也比一遍"那半已做）、F27、F28、**F29**（阶段 [8] 没有通往 IFBC VM 的调用通道 ⇒
> `.inf ifbc` 恒空）、**F30**（§7.7.1 的推导式样例过不了能力扫描，规范样例不能构建）、
> **F31/F32/F33**（D3 轮的 A 轨余项：`#[repr(C)]` struct 跨边界 / §7.6.2 工具链注册与审计 /
> §7.6.3 项 3·5 与 §7.6.5/§7.6.6）；
> `main` 与本地分叉（另一会话推了 `faa34ad` / `05da606`），**合并主干由人决定**。
> 优先级最高的已知地雷：**F1**（§1 唯一的下一个 `[ ]` = `.github/workflows/ci.yml`；只能跑 Windows，且**验收验不到"CI 绿"**，见上面 F1 的 (4)），
> 紧随其后的 **F2**（50 条告警）、**F3**（一次性产物的 `.gitignore`：本轮 E3 就在仓库根落过一个 `tmp_rt.out`，靠手工删）、
> A 轨余项 **F31/F32/F33**，
> **F26 的前半**（"两侧校验等价"仍不许讲）、
> **F12**（移动语义没有运行时入口，`String`/`Vec` 的真实程序写不出来）、**F15**（`fmt!` 被阶段 [4] 挡住，
> §14.2 样例不能 `check`）、**F19**（没有任何 INFY 构造能写 stdout ⇒ `infy run` 的程序输出对现有一切实例
> 恒为 0 字节）、**F21**（`std/test` 在 §10 一条没有 ⇒ §14.7 样例过不了 `check`）、**F29/F30**（B 轨的
> 产物半边与规范样例）、**F5 / F6**、**F7**、**F8**。F9 已在 C2 落地时按"只匹配 `Lit::Int`、
> `ExprKind::Int` 分支只作防御"绕过，死臂本身仍未清。
