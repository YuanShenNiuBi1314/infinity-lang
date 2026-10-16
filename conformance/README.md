# §12 合规用例集（conformance corpus）

这个目录是规范 §12 要求的合规套件。它现在是**首批搬运结果**，不是 §12 的完成量：
§12.1 + §12.2 + §12.5 的表合计 **2300** 个用例，而这里的用例数与逐分类缺口由
`infy conformance --report` 打印（**E2 轮已接线**：20 行逐分类 + 三档小计 + 合计，退出码裁定 = 偏差 **D-48**），
并落进 `docs/coverage.md` §6 与根 README（**E3 轮已落**：那份 20 行 × 11 列的表由
`conformance::tests::the_pass_rate_table_in_coverage_md_matches_the_corpus` 逐格对回 `conformance::tally`，
用例集动了而文档没动 ⇒ 测试变红；本文件下面那张"已有 vs §12 要求"只留数量差额，**不复制通过率**）。
**在用例集覆盖到 100% 之前，任何文档不得出现「INFY v2.0 兼容」字样**（§12.4）—— 这条现在是机检：
`conformance::tests::no_markdown_doc_claims_the_compatibility_name` 扫全仓 `.md`（除 `spec/`），
任何一行出现那句话而没有否定标记就红；`the_front_facing_docs_restate_the_refusal` 反过来要求本文件与
README / `docs/coverage.md` / `docs/status.md` 各自都还写着这句红线。

## 目录结构

```
conformance/cases/<分类目录>/<用例编号>/case.toml      # 期望（诊断码 + 退出码）
conformance/cases/<分类目录>/<用例编号>/main.infy      # INFY 源码，逐字节
conformance/cases/<分类目录>/CATEGORY.toml             # 该分类的 §12 行（由 src/conformance.rs 渲染）
```

分类目录 = §12.1/§12.2/§12.5 三张表的一行，共 20 个，名字由 `src/conformance.rs` 的
`CATEGORIES` 决定（那里是**唯一真值源**，`CATEGORY.toml` 是它的渲染产物；两者不一致时
用例集直接加载失败）。加载器同时拒绝**未知分类目录**与**缺失分类目录**：分类名拼错会让
整目录的用例静默消失，而 §12.5 要的正是"逐分类通过率"，静默丢分类 = 谎报。

## 用例格式

规范写了分类、数量与"检查"列，但从头到尾**没有定义一个用例长什么样**（不存在文件名约定、
不存在期望的表示法、不存在编号规则）。这一处的确定选择记在
`docs/spec-deviations.md` **D-47**，本目录按它执行：

一个用例是**一个目录**，不是一个"源码 + 注释里的期望"的单文件。理由是 §12.1 的分类里就有
"注释"这一项，而 §12.5 的 §9.5 分类检查的是"注释保留"——把期望写进被测源码的注释里，
这两个分类会互相吞掉对方的语义。

`case.toml` 的字段：

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `id` | ✅ | 用例编号，必须**等于目录名**。§12 要求"已有用例的编号与期望不得改变" ⇒ 编号只增不复用 |
| `category` | ✅ | 分类 slug，必须等于所在目录 |
| `clause` | ✅ | 钉住的规范条款，形如 `§4.13 规则 7`；必须以 `§` 开头 |
| `title` | ✅ | 一行说明 |
| `kind` | ✅ | `check` / `fmt-idempotent` / `ifbc-verify`（见下） |
| `expect` | ✅ | 期望的诊断码数组。**正例也要写 `expect = []`** —— 空是声称，不是漏 |
| `exit` | ✅ | 期望退出码（§13.4 的 0–7） |
| `file` | `fmt-idempotent` 必填 | 相对用例目录的 `.infy` 路径 |
| `image` | `ifbc-verify` 必填 | 相对**仓库根**的 IFBC 镜像路径（与 `runtime/fixtures` 共用一份字节，不复制） |
| `requires` | ❌ | 构建期工具名单（`cc` / `python` / `lua`）。缺工具 ⇒ 用例记为**阻塞**，既不记通过也不记失败 |
| `defect` | ❌ | 已登记但未修的 BACKLOG 项（如 `F11`）。挂着标记的用例**永不计为通过**：不达期望记为阻塞，一旦通过则测试变红并要求删标记 |

诊断码按**排序后的多重集**比较，不比较位置与文本——位置和文本是 §9.2 渲染层的事，
另有 `diag`/`fmt` 的单测钉住；这里是"哪条禁令被识别"这一层。

## 三种 kind 各自跑什么

- `check`：把用例目录落成 `target/scratch/conformance/<id>` 下的一个 module（§5.1 的目录树；
  用例不带 `MODULE.infy.toml` 时由运行器写最小清单，见 `conformance::DEFAULT_MANIFEST`），
  然后走 `infy check` **同一条函数链** `cli::check_verdict` = 阶段 [1]-[5] +（[1]-[5] 干净时的）
  [6a]。运行器刻意不复写一条前端管道：D1(2) 与 D2 两轮都实证过"上游全绿对命令/路径是否存在
  没有预测力"。
- `fmt-idempotent`：§9.5 的幂等性 —— 格式化一次、再格式化一次，两次输出必须逐字节相同，
  且诊断集合与期望一致。
- `ifbc-verify`：§7.8.3 的六项校验，取 `ifbc::verify(..., all_errors = true)` 的码；
  退出码归属沿用 `infy ifbc verify` 的裁定（D-45(c)：有阻断 = 2）。

## 怎么跑

```sh
cargo test --offline --lib conformance          # 加载全量用例 + 逐条执行 + 缺口自检
infy conformance --report                       # 逐分类通过率（E2 轮已接线；今天 83 条 ⇒ exit 1，见 D-48(a)）
infy conformance --category 7.8-ifbc             # 只跑一个分类：其余 19 个分类打 `-`/「未运行」，退出码恒不为 0
```

## 现在有多少条（E1 首批，2026-10-16 实测）

**83 条用例，17 个分类有内容**：77 条 Pass、6 条 Blocked（挂着 `defect` 标记的已知未修项）、**0 条 Fail**。
逐分类数量与 §12 要求的差额由运行器算（`conformance::tally`），**E2 轮已把它打到命令行**（实测输出 = `docs/coverage.md` P29），
**E3 轮起完整的逐分类通过率表在 `docs/coverage.md` §6**（Pass/Fail/Blocked/通过率/缺口/非满分的原因，逐格有测试回核）；
下面这张表刻意**只留数量差额**——通过率在两个地方各算一份，报告和文档就会在同一个缺口上给出两个答案（D-47(h)）。

| 分类 | 已有 | §12 要求 | 分类 | 已有 | §12 要求 |
| --- | --- | --- | --- | --- | --- |
| `2-lexical` | 13 | 96 | `7.6-a-track` | 4 | 96 |
| `2.8-literals` | 12 | 96 | `7.7-b-track` | 6 | 96 |
| `3-grammar` | 5 | 288 | `7.8-ifbc` | 2 | 64 |
| `3.11-expr-pat-stmt` | 3 | 128 | `8-chains` | 6 | 128 |
| `4-types-ops` | 6 | 120 | `8.4-node-focus` | 2 | 96 |
| `4.7-ownership` | 7 | 200 | `9.5-fmt` | 2 | 120 |
| `4.9-ub` | 3 | 120 | `1.3-domain-neutral` | **0** | 52 |
| `4.10-eval-order` | 3 | 144 | `10.9-std` | **0** | 84 |
| `5-modules` | 4 | 144 | `11.1-host-protocol` | **0** | 60 |
| `6-macros` | 2 | 96 | `6.7-derive` | 3 | 72 |

6 条 Blocked 挂的标记与对应 BACKLOG 项：`F34`（§10 的 std 表里没有 `Vec::clone` ⇒ §4.7 的"clone 后再用"
正例不可表达）、`F35`（derive 生成的体调 `std::fmt::format_struct` 等 §10 表里没有的符号，
任何 `#[derive(Debug)]` 都过不了 `infy check`）、`F36`（同一根因被两个阶段各报一次：`E-0020`、`E-0410`）、
`F37`（未知 `@` 前缀报 `E-0702` 之后级联 8 条 `E-0053`）。
这六条的期望写的都是**规范的期望**，不是当前实现的输出。

三个空分类的原因（保持目录与 `CATEGORY.toml`，不靠删目录让表好看）：

- `§1.3 领域中立` 需要 `std/domain_terms.txt` 黑名单在场并被扫描 —— `pkg::domain_neutrality_violations`
  是纯函数且有单测，但**全仓库没有任何 CLI 调用点**，而那份黑名单文件本身不在仓库里（**F38**）。
- `§10.9–§10.14 std 补充` 的四项检查（无偏 shuffle、`Map` 有序迭代、`Rng` 种子可复现、
  `ParseIntError.idx`）都是**执行面**断言，而 `infy run` 的程序 stdout 对现有一切实例恒为 0 字节
  （**F19**），`kind = check` 表达不了。
- `§11.1–§11.4 宿主协议` 需要 `HostApi`（`src/host.rs` 仍是 1 行占位；三步握手、串行化锁与 FIFO、
  镜像表 ID 一致性都没有发点）。

