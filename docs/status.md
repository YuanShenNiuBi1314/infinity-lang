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
test result: ok. 285 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
$ cargo check --offline --lib
Finished `dev` profile  (51 warnings，全部为 unused/dead code，见 BACKLOG F2)
$ infy check examples/tour
check ok — 2 file(s), 0 error(s), 0 warning(s), 0 notice(s)   # exit 0
$ infy build examples/tour
（同一行 check ok，再）notice: `infy build` stops here in this build — stages [8] codegen and [9] link are not implemented yet   # exit 6
$ gcc -std=c11 -O1 -Wall -Wextra runtime/infy_ifbc.c runtime/infy_dyn.c \
      runtime/test_ifbc.c runtime/test_ifbc_stubs.c -o ifbc_tests && ./ifbc_tests
IFBC harness: 79 asserts, 79 pass, 0 FAIL        # exit 0；编译期 4 条 infy_rt.h:212 告警（BACKLOG B3）
$ (cd examples/tour && infy fmt --check)
2 file(s) are not canonical                        # exit 2，见下文"导览源码不是格式化不动点"
```

285 项按模块（`cargo test --offline --lib -- --list` 分组计数，合计 285）：
`lexer` 79 · `derive` 57 · `fmt` 39 · `macro_expand` 35 · `toml` 15 · `sha256` 12 · `borrow` 11 ·
`codes` 9 · `base64` 7 · `parser` 5 · `ifbc` 5 · `cli` 5 · `stdlib` 4 · `diag` 2。

- 词法、语法、`fmt`(§9.5 十四条规则)、`macro_expand`(§6.1–§6.5)、`derive`(§6.7)、诊断码表、
  借用检查（`E-1001`/`E-1002`/`E-1004`/`E-1005`/`E-1006` 正反例）、哈希与 base64 与 TOML 往返、
  `infy` CLI 自身、IFBC Rust 侧编码/解码/反汇编：都有上表的测试。
- **没有一项独立单测**：`sema.rs`（§4 类型检查）、`pkg.rs`（§5 清单/MVS/lock/dirhash）、
  `btrack.rs`（§7.7 B 轨）。它们的证据目前只有"对 `examples/tour` 跑完 §7.1 阶段 [1]–[5] 且 exit 0"这一条端到端。
- 先前记为"§9.2 字节级金样"实际是 1 条规范样例逐字节 + 1 条 caret 行规则（`diag` 2 项）；
  "§8.4 节点编号 ✅"实际只有 1 项（`parser::chain_with_linkage_numbers_nodes_in_source_order`），
  `goto` 的三条 `E-0301` 越界路径无测试；`E-0121` 实现了但无测试。

**绿灯的含义要如实限定**：`cargo test --lib` 全绿只说明"已写的模块自洽"，不等于 §12 合规。
`infy build`/`run`/`test`/`doc`/`lib pack`/`ifbc dump`/`conformance` 仍显式报"未实现"（build 是 exit 6，其余 exit 1）；
B 轨 lowering(`src/btrack.rs`) 与 C 侧 harness(`runtime/test_rt.c`) 尚未收敛；
`src/codegen.rs`、`src/hir.rs`、`src/host.rs`、`src/atrack.rs`、`src/doc.rs`、`src/conformance.rs`
是 1 行占位文件（**§11 `HostApi` 之前被 README 标成 ✅，是文档错误，本轮已改为未实现**）；
A 轨绑定、`std.net` TLS、`infy lsp` 未实现。
79 项 IFBC 断言链的是 `runtime/test_ifbc_stubs.c`（harness 替身），**不是**生产 `infy_rt.c`，所以它不能算运行时已验证（B2）。

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
`use`/`mod`/`union`/`linkage` 这组"保留但本版文法未用"的字）。**仍未收敛**：`runtime/test_rt.c`（B1）、
`src/btrack.rs` 的端到端 lowering（D1）。已完成的 C 侧里程碑不变：`ifbc-vm`(`runtime/test_ifbc.c`) 79 项断言通过，
含六项装载校验、逐字节截断/单字节变异/800 轮随机 fuzz；`codes`、`infra`(哈希/TOML) 已交付。
（A3 轮补充限定：这 79 项链的是 `runtime/test_ifbc_stubs.c` 替身，不是生产 `infy_rt.c`。）

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
| "通过率见 `docs/coverage.md`（随构建生成）" | 该文件此前**不存在**，而 `infy conformance` 未实现，不可能"随构建生成"；5 个占位模块的注释也指向它 | 新建 `docs/coverage.md`：逐条款"实现 × 证据"矩阵，并写明外部用例为 0、通过率不可测（E1/E2/E3 待做） |

顺带发现两个真实缺陷（**未修**，已进 BACKLOG F5/F6）：

1. `infy fmt <目录>` / `infy expand <目录>` 的 positional 语义与 `check` 不一致：
   `error: 无法读取 examples/tour: 拒绝访问。 (os error 5)`、`error: 找不到 module 根（§5.1）`；
   只有先 `cd` 进 module 或直接给 `.infy` 文件才可用。
2. `(cd examples/tour && infy fmt --check)` → `2 file(s) are not canonical`，exit 2：
   **§14 导览源码不是本格式化器的不动点**。要么 §9.5 与 §14 互相矛盾（规范级），要么导览样例需按 §9.5 重写。
   未裁决前，任何文档不得声称"§14 导览已是规范写法"。

## 仍未实现

`infy build` 的 C11 发射（阶段 [8]）与产物段写入（阶段 [9]）→ 因此 `build`（exit 6）、
`run`/`test`/`doc`/`lib pack`/`ifbc dump`/`conformance`/`lsp`（exit 1）目前明确返回"未实现"而不是假成功；
A 轨绑定生成（`src/atrack.rs`）、§11 `HostApi`（`src/host.rs`）、§12 合规用例集（`src/conformance.rs`）、
`infy doc`（`src/doc.rs`）、HIR（`src/hir.rs`）同样未实现 —— 这六个文件当前是 1 行占位。

**本实现不声称「INFY v2.0 兼容」**：§12 的 2300 个用例（§12.1 的 1244 + §12.2 的 256 + §12.5 的 800）
目前外部用例为 **0**，逐分类通过率不可测。
