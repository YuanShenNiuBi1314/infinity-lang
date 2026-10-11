# Infinity Language (INFY) — v2.0

**A zero-dependency Rust toolchain for a statically-typed, compiled language.**

`infy` is the reference implementation of the [INFY Language Spec v2.0](spec/infinity_lang_spec.md) — lexer / parser / macros / type & ownership checker / C11 backend / single-file self-contained artifacts / IFBC bytecode with dual-track embedding / `infy` CLI.

- Language `2.0` · toolchain `2.0.0` · **IFABI-2** · IFBC v1 · std 1.0
- Implemented in Rust with **zero third-party dependencies** (`.cargo/config.toml` sets `offline = true`; the compiler is a trust boundary — the supply-chain model of §5.8 applies only to user code)
- AOT path: INFY → C11 → host `cc` → single executable + appended `.inf` segment (§7.4)
- License: MIT

[English](#english) · [中文](#中文)

## English

### Why INFY v2.0

Most orchestration DSLs are either weakly checked or drag in a heavy runtime. INFY v2.0 is designed as a **statically-typed, compiled language** with a toolchain that is itself auditable: no crates, no lockfile, no runtime dependency — `cargo build --release --offline` is all you need.

### Quick start

```
cargo build --release --offline
target/release/infy -V
infy init github.com/me/hello
infy check            # §7.1 phases [1]–[5]
infy build --reproducible
infy run
infy fmt --check
infy expand --stage macro
infy conformance --report
```

### Status (honest)

Implemented and covered by unit/golden tests: the **210-code diagnostics engine** (§9.1–§9.3, byte-exact rendering), full lexer (§2), recursive-descent EBNF parser (§3), module/package/file with SHA-256 Merkle dirhash (§5), type & ownership checking (§4), the std signature table (§10), the artifact container with `INFYART2` trailer (§7.4/§7.3), IFBC bytecode with six load-time checks (§7.8/appendix D), and the `HostApi` C ABI with the three-step handshake (§11).

Still converging (`cargo check --offline --lib` is the trunk gate; some `cargo test` modules are not settled yet): macro expansion, derive, `fmt`, borrow checking module, IFBC B-track ingestion, the C11 emitter, and the conformance suite.

Explicitly not implemented (errors out, no silent degradation): the TLS backend of `std.net`, `infy lsp`, and a few spec-gap byte formats (fixed locally, documented in `runtime/IFBC.md`).

**This implementation does not claim "INFY v2.0 conformance"**: spec §12 requires 100% of 2300 cases; current case count and pass rate live in `docs/coverage.md`.

### Layout

```
src/             toolchain (Rust, zero dependencies)
runtime/         embedded runtime & IFBC VM (C11) + ABI docs
capabilities/    §7.7.2 capability tables (IFBC-CAP-<lang>-<minor>.toml)
spec/            INFY Language Spec v2.0 (full text)
docs/            design docs, coverage matrix, deviation log
examples/        compilable §14 language tour
tests/           integration tests
```

### Spec deviations

Every choice made where the spec is silent is recorded in `docs/spec-deviations.md` (including two internal inconsistencies in §9.2 examples, 14 codes outside the §9.1 range table, and the appendix-E pseudo-TOML `;` syntax).

### License

MIT — see [LICENSE](LICENSE).

---

## 中文

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
infy check            # §7.1 阶段 [1]–[5]
infy build --reproducible
infy run
infy fmt --check
infy expand --stage macro
infy conformance --report
```

## 规范覆盖状态（如实记录）

已实现并通过单元/金样测试：

| 条款 | 内容 | 状态 |
| --- | --- | --- |
| §9.1–§9.3, §9.7 | **210 个诊断码**全表（E/W/R/N，含 §9.3 保留不复用的 `E-0701`/`R-0702`） | ✅ `src/codes.rs` |
| §9.2 | 诊断渲染的字节级版式（gutter、caret 行、`note`/`help`、建议行） | ✅ 金样测试逐字符对齐 |
| §9.4 / E.5 | `--diag-json` 结构（`phase` / `origin` / `expand_chain` / `suggestions` / `runtime_ctx`） | ✅ |
| §2 | 词法：编码与换行、嵌套注释、`//infy:build` 约束、XID 标识符与保留字、生命周期记法、数值/字符串/原字符串/字节串/`char`/`cord` 的完整文法与转义封闭表、`@embed` 词法不透明区 | ✅ `src/lexer.rs` |
| §3 | EBNF 全产生式递归下降（含 §3.6 嵌入、§3.9 宏、§3.11/3.12/3.13 补全），§3.10 消歧，错误恢复 | ✅ `src/parser.rs` |
| §5 | module/package/file 三层、`MODULE.infy.toml`、§5.3 导入解析次序、`internal` 可见性、§5.7 MVS、§5.8 dirhash（SHA-256 Merkle + `h1:` base64）、`MODULE.infy.lock` 读写、§2.2 约束表达式求值 | ✅ `src/pkg.rs` |
| §4 | 类型全集、字面量范围规则、无隐式数字转换（`E-0110`/`E-0111`）、运算符语义、`?` 展开规则、生命周期 elision 三规则、§4.12 数字细节、§3.12 穷尽性与可达性、§2.12 const-eval 与 `const_assert`、`env` 封闭字段集、`unsafe` 六项禁令 | ✅ `src/sema.rs` |
| §5.8 | SHA-256 / base64 / Merkle dirhash 自研实现（NIST 与 RFC 4648 向量） | ✅ |
| §5.2/E.2–E.4 | 无依赖 TOML 子集解析 + 稳定输出（`parse→emit→parse` 幂等） | ✅ |
| §10 | `std` 签名表（驱动名称解析、领域中立扫描、`infy doc`、codegen 映射） | ✅ `src/stdlib.rs` |
| §1.3 | 领域中立判据落成可执行检查（`NFKCCaseFold` 匹配 `domain_terms` 黑名单） | ✅ |
| §8.4 | 节点表编号（只有顶层 `expr_stmt` 开节点；`let`/`var`/赋值/`label` 归为前序节点的序言） | ✅ |
| §7.4 / §7.3 | 产物容器：宿主可执行文件 + `INFYART2` 段尾 + `.inf meta/rodata 索引/ifbc/sum`；`build-id` = 规范化输入的 SHA-256 | ✅ 设计与运行时实现 |
| §7.8 / 附录 D | IFBC 编码、反汇编、装载期六项校验（`R-0704`…`R-0708`），失败即整模块拒绝（UB-019） | ✅ |
| §11 | `HostApi` C ABI、三步握手（`R-0903/0904/0905`）、`EnvSnapshot` 固定偏移 | ✅ |
| 运行时 | 检查型算术、Vec/String 增长因子（ABI 可观察）、Cleanup deque 实现的确定性 drop 顺序与 `focus` 终止半径、焦点调度器与 ResourceLock FIFO、`xxHash64`、SplitMix64+PCG32 与无偏 Fisher–Yates | ✅ |

进行中（`cargo test` 可能因这些模块尚未收敛而失败；`cargo check --offline --lib` 为主干门禁）：

`src/macro_expand.rs` · `src/derive.rs`（§6）、`src/fmt.rs`（§9.5 十四条规则）、`src/btrack.rs` + `src/ifbc.rs`（§7.7 B 轨摄取与 lowering）、`runtime/*.c`（C 侧 VM 与运行时）、`src/codegen.rs`（C11 发射）、`src/artifact.rs`、`src/cli.rs`、`src/borrow.rs`（§4.7 借用检查独立模块）、`src/conformance.rs`（§12 用例集）。

明确未实现（不使用即报错，不做静默降级）：`std.net` 的 TLS 后端（§10.4）、`infy lsp`（§13.4 标注为可选）、`dyn Trait` 的跨实现 vtable 偏移表（规范缺口 C-7 ②）、`line_map`/`exception_table` 的规范字节格式（缺口 C-7 ①，本实现自行固定并写入 `runtime/IFBC.md`）。

**因此本实现不声称「INFY v2.0 兼容」**：§12 合规套件要求 100% 通过 2300 用例，当前用例集与通过率见 `docs/coverage.md`（随构建生成）。

## 目录

```
src/            工具链（Rust，零依赖）
runtime/        内嵌运行时与 IFBC 虚拟机（C11）+ ABI 文档
capabilities/   §7.7.2 能力表（IFBC-CAP-<lang>-<minor>.toml）
spec/           INFY Language Spec v2.0 全文
docs/           设计文档、覆盖矩阵、规范偏差记录
examples/       §14 语言导览的可编译样例
tests/          集成测试
```

## 规范偏差

实现必须对规范未定处做出选择，全部逐条记录在 `docs/spec-deviations.md`（含 §9.2 示例的两处内部不一致、§9.3 缺少通用语法错误码、`E/W/R/N` 中 14 个码落在 §9.1 区间表之外、附录 E 的伪 TOML `;` 分隔语法等）。

## 许可

MIT
