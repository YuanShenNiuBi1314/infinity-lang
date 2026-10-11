# 构建状态（如实记录）

`main` 当前 **`cargo check --offline --lib` 失败**，原因是两个模块仍在被并发写入、语法尚未闭合：

- `src/fmt.rs`（§9.5 十四条规则）
- `src/ifbc.rs` + `src/btrack.rs`（§7.8 / §7.7）

失败点只出现在这两个模块内部，其余模块无诊断。

## 已验证为绿色的部分

把上述两个模块换成空桩后（隔离副本，不动在写的文件），整条前端流水线可编译并通过测试：

```
cp -r src target/verify/src            # 隔离副本
printf '//! stub\n' > target/verify/src/fmt.rs
printf '//! stub\n' > target/verify/src/ifbc.rs
printf '//! stub\n' > target/verify/src/btrack.rs
printf '//! stub\n' > target/verify/src/macro_expand.rs
printf '//! stub\n' > target/verify/src/derive.rs
cargo test --offline --lib             # 诊断金样 + 词法 + 语法 + 类型 + 借用
```

在该状态下实测通过：

| 阶段 | 内容 | 结果 |
| --- | --- | --- |
| §9.2 | 诊断版式金样（gutter / caret / note / help / 建议行） | 通过 |
| §2 | 词法（含 `@embed` 不透明区、字面量封闭表、错误码逐条） | 通过 |
| §3 | 语法（全产生式 + §3.10 消歧 + 连携块 + 节点表） | 通过 |
| §4 | 类型检查（无隐式转换、字面量范围、`?` 展开、elision、穷尽性、const-eval、`env` 封闭集） | 通过 |
| §4.7 | 借用检查 `E-1001`/`E-1002`/`E-1004`/`E-1005`/`E-0121` + 正反例 | 11/11 通过 |
| §5 | 清单/包/导入/MVS/lock/dirhash（SHA-256 + Merkle + `h1:`） | 通过 |

## 尚未实现（不使用即报错，不静默降级）

`infy build` 的 C11 发射（`codegen.rs`）、产物段写入（`artifact`）、CLI 主体（`cli.rs`）、
A 轨绑定生成（`atrack.rs`）、§12 合规用例集，以及 `runtime/*.c` 的收尾。
因此 §14 语言导览样例目前还不能 `infy check → build → run` 端到端跑通。

**本实现不声称「INFY v2.0 兼容」**（§12 要求 2300 用例 100% 通过）。
