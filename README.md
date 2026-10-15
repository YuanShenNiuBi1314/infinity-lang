# Infinity Language (INFY) — v2.0 工具链

`infy` 是 [INFY Language Spec v2.0](spec/infinity_lang_spec.md) 的参考实现：**静态类型、编译型**语言的工具链 ——
词法 / 语法 / 宏 / 类型与所有权检查 / C 后端 / 单文件自包含产物 / IFBC 字节码与双轨嵌入 / `infy` CLI。

- 语言版本 `2.0` · 工具链 `2.0.0` · **IFABI-2** · IFBC v1 · std 1.0
- 实现语言 Rust（规范指定的参考实现语言），**零第三方依赖**（`.cargo/config.toml` 里 `offline = true`；编译器是信任边界，供应链模型只适用于用户代码）
- AOT 路径：INFY → C11 → 宿主 `cc` → 单个可执行文件 + 追加的 `.inf` 段（§7.4）
- License: MIT

## 快速开始

```
cargo build --release --offline
target/release/infy -V
infy init github.com/me/hello
infy check            # §7.1 阶段 [1]–[5]，可用；exit 0
infy fmt --check      # §9.5，可用；文件不是规范写法时 exit 2（需 cd 进 module 或传 .infy 文件）
infy expand --stage macro   # §6.8，可用（与 fmt 一样：positional 目前只吃文件，目录参数会报"找不到 module 根"，BACKLOG F5）
infy build            # 阶段 [1]-[5] + [7] hir + [8] codegen + [9] 链接与 §7.4 段容器：exit 0，产出单文件产物
infy artifact info <产物> [--nodes]   # §7.4：读回 .inf meta 与节点表；非产物 → E-0605 + exit 7
infy run              # §13.4 开发态：[1]-[9] 建到临时目录 + 立即执行，程序的 stdout/stderr 逐字节转发；exit 0（tour），运行时 panic 时 exit 3
infy test             # §5.9：测试构建（`*_test.infy` 与 `tests/…` 只在这里入选）+ 跑生成的 `#[test]` harness；全过 exit 0、有 Err exit 1、panic exit 3（§13.4）
infy doc              # 未实现：notice + exit 1
infy ifbc dump|verify # 未实现：notice + exit 1
infy conformance --report   # 未实现：notice + exit 1
```

## 规范覆盖状态（如实记录）

下表只说"实现 + 证据"，**逐条款、逐模块的完整矩阵在 `docs/coverage.md`**（含每条声称对应的命令与数字）。

| 条款 | 内容 | 状态 |
| --- | --- | --- |
| §9.1–§9.3, §9.7 | **210 个诊断码**全表（E/W/R/N，含 §9.3 保留不复用的 `E-0701`/`R-0702`） | ✅ 9 项测试断言 `count()==210` |
| §9.2 | 诊断渲染的字节级版式（gutter、caret 行、`note`/`help`、建议行） | 🟡 仅 §9.2 那条规范样例逐字节对齐（2 项测试） |
| §9.4 / E.5 | `--diag-json` 结构（`phase` / `origin` / `expand_chain` / `suggestions` / `runtime_ctx`） | 🟡 能输出，无金样测试 |
| §2 | 词法：编码与换行、嵌套注释、`//infy:build` 约束、XID 标识符与保留字、生命周期记法、数值/字符串/原字符串/字节串/`char`/`cord` 的完整文法与转义封闭表、`@embed` 词法不透明区 | ✅ 79 项测试 |
| §3 | EBNF 递归下降（含 §3.6 嵌入、§3.9 宏、§3.11/3.12/3.13 补全），§3.10 消歧，错误恢复 | 🟡 7 项测试 + `examples/tour` 全量解析（A4 轮补 `start(chain = …)` 与嵌套泛型闭合 2 项）；没有"全产生式都落到 AST"的审计 |
| §5 | module/package/file 三层、`MODULE.infy.toml`、§5.3 导入解析次序、`internal` 可见性、§5.7 MVS、§5.8 dirhash、lock 读写、§2.2 约束表达式 | 🟡 **24 项独立单测**（A4 轮补：清单逐段/`E-0506`/`E-0507`/`W-0500`、§5.3 次序、`^` 区间、MVS 最小选择与冲突、dirhash、lock 往返与 `E-0200`；C5 轮补：`//infy:build` 表达式对 `BuildConfig` 求值、§5.9 测试文件只进测试构建、`discover()` 的 §5.1 R1/R2 形状与 `read_dir` 排序）。C5 轮之前的实情：`constraint_matches`/`unknown_constraint_names`/`is_test` **存在但零调用点**，§2.2/§5.9 的文件选择从未生效，`E-0012` 在 CLI 上从不触发 |
| §5.9 / §13.4 `infy test` | 测试构建的文件选择 + `#[test]` harness（入口替换、`()`/`Result` 两种判据、退出码 0/1/3）+ `--bench` 开关 | 🟡 C5 轮落地：`codegen::emit_test_driver` 按 (package, name) 序逐个调用，`Result` 取 `.is_ok`，报告由生成的 C `fputs` 到 stdout；`cli::test_cmd` = 建到临时目录 + 执行 + 逐字节转发（同一 module 上 `infy check` 1 file / `infy test` 2 files 已实测）。**仍 🟡**：`std/test` 在 §10 无任何条目 ⇒ §14.7 的样例（`test::assert_eq`、`Bench`）过不了 `check`，`#[bench]` 有参数即拒（F21）；`--cov` 显式拒（F22）；panic 不逐测试隔离（要动冻结头，见偏差 D-36）；test 产物的 `.inf meta` 入口字段仍写 §7.4 那一档（F23）|
| §4 | 类型全集、字面量范围、无隐式数字转换、运算符语义、`?` 展开、生命周期 elision、§4.12 数字细节、穷尽性、§2.12 const-eval、`env` 封闭字段集、`unsafe` 六禁令 | 🟡 **22 项独立单测**（A4 轮补：内建构造、未声明名、字面量取型与后缀范围、`type_compatible` 逐臂、`E-0120`、`env`/`E-0140`、`goto`/`start`）；`?` 展开、elision、穷尽性、const-eval、`unsafe` 六禁令仍只有 tour 证据 |
| §4.7 | 借用检查 | ✅ 15 项：`E-1001`/`E-1002`/`E-1004`/`E-1005`/`E-1006` 正反例 + `E-0121` 4 项（A4 轮补；其判据低估数值绑定，见偏差 D-19 / BACKLOG F7） |
| §5.8 | SHA-256 / base64 / Merkle dirhash 自研实现（NIST 与 RFC 4648 向量） | ✅ 12 + 7 项 |
| §5.2/E.2–E.4 | 无依赖 TOML 子集解析 + 稳定输出（`parse→emit→parse` 幂等） | ✅ 15 项 |
| §6.1–§6.5 / §6.7 | `macro_rules!` 展开与 derive 生成 | ✅ 35 + 57 项 |
| §9.5 | 格式化十四条规则 | ✅ 39 项 |
| §10 | `std` 签名表 | 🟡 表有 4 项测试；消费者 `infy doc` 未实现，codegen 只发射 §10 里有 `infy_rt.h` 入口的那部分（`Vec`/`String`/`fmt` 的构造入口缺 → 见下） |
| §1.3 | 领域中立判据（`NFKCCaseFold` 匹配 `domain_terms` 黑名单） | ✅ 4 项 |
| §8.4 | 节点表编号 | 🟡 7 项（`parser` 编号 1 项 + `sema` 的 `goto` 三条 `E-0301` 越界路径、`start` 链名/锚点、序言不移动编号） |
| §7.4 / §7.3 | 产物容器：宿主可执行文件 + `INFYART2` 段尾 + `.inf meta/rodata 索引/ifbc/sum`；`build-id` | 🟡 C3 轮落地：`src/artifact.rs` 写 + 读，`infy build` 阶段 [9] 产出 199975 字节容器并 exit 0，`infy artifact info --nodes` 读回自家产物（节点表 6 行 = §8.4 金样），`--verify-reproducible` 两个独立目录字节相同，`build-id` 绑满 §7.3 的九类输入。**仍 🟡**：阶段 [10] 的 sumdb 校验/运行时自校验未做（`.inf sum` 只是 `lock.dirh` 的抄写，tour 无依赖 = 空集）、`.inf ifbc` 恒空（阶段 [6a] 已产出镜像，缺阶段 [8] 的调用通道 = F29）、`ResourceKeys`/`EnvOffsets` 写空集、A 轨 `#[link]` 目标码与 LTO 编排没有；另裁掉一条规范级缺陷 **D-33**（重复段尾扫描范围会否掉每个真产物），偏差表见 **D-32** |
| §7.8 / 附录 D | IFBC 编码、反汇编、装载期六项校验（`R-0704`…`R-0708`）、UB-019 | 🟡 **D4(1)+D4(2)+D4(3) 已把两侧改成 D-38 裁定的同一份线格式，并且读成同一份文本**：`runtime/fixtures/ifbc_rust_writer.bin` 是 Rust 写入器的逐字节输出，C 侧用生产装载器读回并**执行**其中函数；D4(3) 另加两份规范化文本入口（C 侧 `infy_ifbc_dump_canonical`/`infy_ifbc_verify_canonical`，Rust 侧 `ifbc::dump_canonical`/`ifbc::verify_canonical`）与三份共用夹具 —— **附录 D 全部 opcode 的逐指令反汇编相等**、**校验矩阵 10 条 `(code, func, off)` 逐条相等**（Rust 侧 22 项单测 / C 侧 180 断言）。当场抓到 `coro.resume`/`coro.throw` 的 C 侧操作数个数（= 指令流错位）与 D.9 助记符命名，外加本侧 `verify_func` 的重复报告缺陷，全部记入偏差 **D-42**。**仍 🟡**：两侧装载期校验结论仍有 D-42 (a)–(e) 五类不可比（F26 未消）、六条间隙指令在 C 侧装载通过而执行显式拒绝（F27 / 偏差 D-41）、`.inf ifbc` 也仍恒空（阶段 [6a] 已产出镜像，缺阶段 [8] 的调用通道 = F29） |
| §11 | `HostApi` C ABI、三步握手（`R-0903/0904/0905`）、`EnvSnapshot` 固定偏移 | ⬜ `src/host.rs` 是占位文件 —— **未实现**（偏移表本身已由 `src/hir.rs::env_table` 落成并有金样） |
| 运行时 | 检查型算术、drop/defer 序、Vec/String 增长因子、NFC 比较、Map/VecMap/Deque 序、`xxHash64`、`std.fmt` 字节精确、SplitMix64+PCG32、无偏 Fisher–Yates、ResourceLock FIFO、焦点调度与 `focus`/`chain` 半径、§9.2 渲染、产物容器、host 三步握手、`std.fs` | ✅ 对 harness 而言：`gcc -std=c11 -O1 -Wall -Wextra runtime/infy_rt.c runtime/test_rt.c` → **32335 assertions / 0 failures**（0 error / 0 warning，链的是生产 `infy_rt.c`）。IFBC 那 180 断言 B2 轮起也链生产 `infy_rt.c`（同为 180/180，输出逐字节相同），且 D4(3) 轮起两侧读**同一份规范化文本**。仍不等于 §12 合规：外部用例为 0 |

`cargo test --offline --lib` 目前 **460 passed / 0 failed**（`cargo check --offline --lib` 0 error / 50 warning；
D4(1) 轮把 `ifbc` 从 6 项抬到 **19** 项，并删掉 `CodeBuilder` 的一个死字段，告警 −1；D4(2) 轮 +1 = 钉
"Rust 写入器的字节 == 已提交夹具"；D4(3) 轮 +2 = `ifbc` **22** 项，钉"本侧渲染的规范化文本 == C 侧渲染并
提交的那份"；D1(1) 轮 +23 = `btrack` 从 **0** 到 **23**、D1(2) 轮到 **30** 项，钉 §7.7.1 探针、§7.7.2 五个扫描项、§7.7.3 版本锁
与附录 E.3 的表语法/表体 hash —— 它们当场揪出三个把 D1 整条路挡死的缺陷，见 `docs/status.md` 的 D1(1) 一节）。
"绿灯"只表示已写测试的模块自洽：A4 轮之后 `sema`(22) 与 `pkg`(24，C5 轮 +5) 已不再是零单测，C2 轮之后 `codegen` 也不再是
（**22** 项 = 19 + C5 轮的 3 项 harness 断言，锁 §4.1/§4.8 布局自检、§4.9/§7.2 检查永不关、§4.11 `?`、§3.12 `match`、
§4.7.4 清理序、§8.4 节点表、§8.2 `goto` 转移控制、§5.3 调用解析、§5.9 测试 harness、§7.3 字节稳定），
C3 轮新建的 `artifact` 直接带 **18** 项（§7.4 容器逐字段读回、七个拒绝理由各一条、§7.3 九类输入逐个绑定、
`INFY_META_OFF_*` 钉回冻结头文本），C4/C5 轮把 `cli` 抬到 **16** 项（`infy run`/`infy test` 的端到端退出码与逐字节转发、
§5.9/§2.2 的文件选择、§5.8 的 dirhash 不随构建范围变、`--cov`/`--arg`/`--feature` 的显式拒绝），仍在进行中或未开始的只剩
`host`、`atrack`、`conformance`、`doc`（`btrack` 于 D1(1) 轮从 0 抬到 **23** 项，但它锁的是**库内**的
探针/扫描/降级路径 —— 阶段 [6a] 至今没有 CLI 调用点；`hir` 于 C1 轮补了 20 项声明层金样，语句/表达式级 IFIR 仍未落 ——
阶段 [8] 是 AST→C 直接发射）。
C 侧另有两份独立 harness，**两份都直接链生产 `runtime/infy_rt.c`**：`runtime/test_rt.c` →
**32335 assertions / 0 failures**（B1 轮收敛，`-Wall -Wextra` 零告警；C3 轮起其中 27 项是把 **Rust 写入器**的
逐字节夹具 `runtime/fixtures/artifact_rust_writer.bin` 交给生产读取器 `infy_art_read_file` = Rust 写 ↔ C 读）；
`runtime/test_ifbc.c` →
**180 asserts, 180 pass, 0 FAIL**（B2 轮真实链接、D4(2) 轮对齐 D-38 的唯一线格式后 82 → 146，D4(3) 轮的
双侧文本夹具再 +21 = **180**，零告警；同一 harness 链 `test_ifbc_stubs.c` 替身也同为 180/180，两份输出逐字节
相同。两份锁点并存：字节夹具 `runtime/fixtures/ifbc_rust_writer.bin` 交给**生产装载器**读回并执行
（Rust 写 ↔ C 读+跑），而 `ifbc_all_ops.bin` / `ifbc_verify_matrix.bin` 的 `.can.txt` / `.ver.txt` 由 **C 侧**
渲染、Rust 侧断言同一份文件（两侧读同一文本，谁漂谁红）。
以上都只说明"已写用例自洽"，**不等于 §12 合规**（外部用例仍为 0）。

明确未实现（不使用即报错，不做静默降级）：`src/atrack.rs`（A 轨绑定）、
`src/host.rs`（§11 `HostApi`）、`src/doc.rs`（`infy doc`）、`src/conformance.rs`（§12 用例集）——这四个文件目前是 1 行占位；
`infy build` 的阶段 [10]（产物签名/sumdb 校验段与运行时自校验比对，§5.8）**未做** —— 容器里写了依赖 dirhash 清单，但没人拿它去比对；
`src/hir.rs` 与 `src/codegen.rs` 不再是占位文件：前者只落**声明层**（类型布局、链节点表、`env` 偏移、符号名），
后者发射 C11 但**没有独立的语句/表达式级 IR**；
`src/codegen.rs` 明确拒绝的构造（逐条指名条款，见偏差 D-26…D-31 与 BACKLOG F12…F15）：闭包、`dyn`、`@embed`、
A 轨 `extern` 调用、`event` 调用、`for`/`defer`/`const_assert`、128 位算术与字面量、窄位宽移位、
`overflow = "saturate"`、含 NUL 的 `str` 字面量、字节串字面量、owned 值的三种移动形式；
`infy test --cov`（§12.4 的 `-O0` 节点表插桩没有阶段产出，显式 exit 1，BACKLOG F22）、
`infy test --bench` 依赖的 `std/test`（§14.7 的 `test::Bench`/`b.iter()` 与 `test::assert_eq` 在 §10 的 std
签名表里**一条都没有**，故有参数的 `#[bench]` 一律拒绝并点名本条，BACKLOG F21）、
`std.net` 的 TLS 后端（§10.4）、`infy lsp`（§13.4 标注为可选）、`dyn Trait` 的跨实现 vtable 偏移表（规范缺口 C-7 ②）、
`line_map`/`exception_table` 的规范字节格式（缺口 C-7 ①，本实现自行固定并写入 `runtime/IFBC.md`）。

**因此本实现不声称「INFY v2.0 兼容」**：§12 要求 `infy conformance --report` 输出**逐分类**通过率、用例集共 2300 项
（§12.1 的 1244 + §12.2 的 256 + §12.5 的 800）。当前外部用例为 **0**，`infy conformance` 显式报未实现，
所以通过率**不可测**；`docs/coverage.md` 现在是逐条款"实现 × 证据"矩阵，合规通过率待 E1/E2 落地后追加。

## 目录

```
src/            工具链（Rust，零依赖）。测试是各模块内的 `mod tests`，没有顶层 tests/ 目录
runtime/        内嵌运行时与 IFBC 虚拟机（C11）+ ABI 文档
capabilities/   §7.7.2 能力表（IFBC-CAP-<lang>-<minor>.toml）
spec/           INFY Language Spec v2.0 全文
docs/           覆盖矩阵（coverage.md）、构建状态（status.md）、规范偏差、运行日志
examples/       §14 语言导览的可编译样例
```

## 规范偏差

实现必须对规范未定处做出选择，全部逐条记录在 `docs/spec-deviations.md`（含 §9.2 示例的两处内部不一致、§9.3 缺少通用语法错误码、`E/W/R/N` 中 14 个码落在 §9.1 区间表之外、附录 E 的伪 TOML `;` 分隔语法等）。

## 许可

MIT
