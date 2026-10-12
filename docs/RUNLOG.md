# 运行日志（自动化与人工的贡献线）

协议见 `docs/BACKLOG.md` §0。每一轮追加一行；数字必须来自命令输出，不许估算。

| 轮次 | 时间 | 做了什么 | 关键命令 | 结果 | 下一步 |
| --- | --- | --- | --- | --- | --- |
| pre-cron | 2026-10-12 00:4x | A1 完成：补 `ifbc.rs` 被截断的 `mod tests` 闭合括号；修 `finish_into` 消耗 builder 后仍读 `marks` 的 E0382；`fmt` 的 `Doc::Blank` 改为"终止当前行 + 恰有空行 + 吞掉紧随其后的硬换行" | `cargo test --offline --lib` | 274 passed / 11 failed（fmt 8、derive 1、ifbc 1、cli 1）；`infy check examples/tour` exit 0 | A2 |
| cron-01 | 2026-10-13 01:40 | A2 完成：`cargo test --offline --lib` 全绿。前端 4 个根因（属性路径/参数名不接受保留字 → `#[link(…, static = true)]` 整条被丢；`impl Trait for Type` 只留 trait 侧 → `E-0421` 永不触发；`.` 后不接受保留字成员 → `.link()` 连锁 5 个 `E-0053`；`macro_rules!` 首臂吞掉整条记号流 → 宏被格式化器打印成 `v {}` 的假成功）。格式化器 8 处（列表双 `HardLine` 造出缩进空行破坏 rule 12；struct/enum/match/变体字段列表丢逗号 → `E-0092`；注释归属改按字节区间，`unsafe` 上方的 `// infy-safety:` 不再被扫进块内触发 `W-1030`；`@embed` 闭包前多空行 + 参数被拆行；`where` 粘连；`for` 每遍多一对括号；链续行缩进改相对一层；RI 旗帜宽度 2 列）。IFBC 反汇编按附录 D.7 打 `is.dyn T`。顺带发现 Rust/C 的 IFBC 操作数个数不一致，登记为 BACKLOG D4（未修）。测试语料里 `use(x)` 改为 `sink(x)`：`use` 按 §2.4 是保留字，是输入不合法而非实现错误。偏差新增 D-13…D-16 | `cargo check --offline --lib` / `cargo test --offline --lib` / `cargo run --offline --bin infy -- check examples/tour` | check：0 error、51 warning；test：`285 passed; 0 failed`（基线 274 passed/11 failed）；tour：`check ok — 2 file(s), 0 error(s)`，`TOUR_EXIT=0`；提交 `816ae92` | A3（逐条核对 status/README 覆盖表的每个数字，主数字本轮已同步） |
