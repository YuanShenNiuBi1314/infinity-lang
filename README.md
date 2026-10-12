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
infy build            # 跑完前端后停在阶段 [8]：notice + exit 6
infy run              # 未实现：notice + exit 1
infy test             # 未实现：notice + exit 1
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
| §5 | module/package/file 三层、`MODULE.infy.toml`、§5.3 导入解析次序、`internal` 可见性、§5.7 MVS、§5.8 dirhash、lock 读写、§2.2 约束表达式 | 🟡 **19 项独立单测**（A4 轮补：清单逐段/`E-0506`/`E-0507`/`W-0500`、§5.3 次序、`^` 区间、MVS 最小选择与冲突、dirhash、lock 往返与 `E-0200`）；`discover()` 目录发现仍只有 tour 证据 |
| §4 | 类型全集、字面量范围、无隐式数字转换、运算符语义、`?` 展开、生命周期 elision、§4.12 数字细节、穷尽性、§2.12 const-eval、`env` 封闭字段集、`unsafe` 六禁令 | 🟡 **22 项独立单测**（A4 轮补：内建构造、未声明名、字面量取型与后缀范围、`type_compatible` 逐臂、`E-0120`、`env`/`E-0140`、`goto`/`start`）；`?` 展开、elision、穷尽性、const-eval、`unsafe` 六禁令仍只有 tour 证据 |
| §4.7 | 借用检查 | ✅ 15 项：`E-1001`/`E-1002`/`E-1004`/`E-1005`/`E-1006` 正反例 + `E-0121` 4 项（A4 轮补；其判据低估数值绑定，见偏差 D-19 / BACKLOG F7） |
| §5.8 | SHA-256 / base64 / Merkle dirhash 自研实现（NIST 与 RFC 4648 向量） | ✅ 12 + 7 项 |
| §5.2/E.2–E.4 | 无依赖 TOML 子集解析 + 稳定输出（`parse→emit→parse` 幂等） | ✅ 15 项 |
| §6.1–§6.5 / §6.7 | `macro_rules!` 展开与 derive 生成 | ✅ 35 + 57 项 |
| §9.5 | 格式化十四条规则 | ✅ 39 项 |
| §10 | `std` 签名表 | 🟡 表有 4 项测试；其消费者 `infy doc`、codegen 映射未实现 |
| §1.3 | 领域中立判据（`NFKCCaseFold` 匹配 `domain_terms` 黑名单） | ✅ 4 项 |
| §8.4 | 节点表编号 | 🟡 7 项（`parser` 编号 1 项 + `sema` 的 `goto` 三条 `E-0301` 越界路径、`start` 链名/锚点、序言不移动编号） |
| §7.4 / §7.3 | 产物容器：宿主可执行文件 + `INFYART2` 段尾 + `.inf meta/rodata 索引/ifbc/sum`；`build-id` | 🟡 `infy artifact info` 只识别段尾魔数；段写入器与 `.inf meta` 解析都不存在（BACKLOG C3） |
| §7.8 / 附录 D | IFBC 编码、反汇编、装载期六项校验（`R-0704`…`R-0708`）、UB-019 | 🟡 Rust 侧 5 项 + C 侧 79 断言**各自**通过；两侧操作数个数不一致（BACKLOG D4），双侧交叉验证目前必然错位 |
| §11 | `HostApi` C ABI、三步握手（`R-0903/0904/0905`）、`EnvSnapshot` 固定偏移 | ⬜ `src/host.rs` 是占位文件 —— **未实现** |
| 运行时 | 检查型算术、drop/defer 序、Vec/String 增长因子、NFC 比较、Map/VecMap/Deque 序、`xxHash64`、`std.fmt` 字节精确、SplitMix64+PCG32、无偏 Fisher–Yates、ResourceLock FIFO、焦点调度与 `focus`/`chain` 半径、§9.2 渲染、产物容器、host 三步握手、`std.fs` | ✅ 对 harness 而言：`gcc -std=c11 -O1 -Wall -Wextra runtime/infy_rt.c runtime/test_rt.c` → **32303 assertions / 0 failures**（0 error / 0 warning，链的是生产 `infy_rt.c`）。仍不等于 §12 合规；IFBC 那 79 断言链的仍是 `test_ifbc_stubs.c` 替身（B2） |

`cargo test --offline --lib` 目前 **332 passed / 0 failed**（`cargo check --offline --lib` 0 error / 51 warning）。
"绿灯"只表示已写测试的模块自洽：A4 轮之后 `sema`(22) 与 `pkg`(19) 已不再是零单测，
但 `hir`、`codegen`、`host`、`btrack`、`atrack`、`conformance`、`doc` **一项单测都没有**，仍在进行中或未开始。
C 侧另有两份独立 harness：`runtime/test_rt.c` 直接链**生产** `runtime/infy_rt.c` →
**32303 assertions / 0 failures**（B1 轮收敛，`-Wall -Wextra` 零告警）；`runtime/test_ifbc.c` →
`79 asserts, 79 pass, 0 FAIL`，但它链的是 `test_ifbc_stubs.c` 替身（真实链接 = B2）。
以上都只说明"已写用例自洽"，**不等于 §12 合规**（外部用例仍为 0）。

明确未实现（不使用即报错，不做静默降级）：`src/codegen.rs`（§7.5 C11 发射）、`src/hir.rs`、`src/atrack.rs`（A 轨绑定）、
`src/host.rs`（§11 `HostApi`）、`src/doc.rs`（`infy doc`）、`src/conformance.rs`（§12 用例集）——这六个文件目前是 1 行占位；
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
