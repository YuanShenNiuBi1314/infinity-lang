# scripts/ —— 风险操作的唯一入口

项目所有者要求：**所有涉及风险的操作一律通过脚本执行**，不要逐条直接敲命令。
原因很实际：直接命令会在人的界面上弹确认，打断人；本机有自动 UAC 提权，脚本跑起来没有阻碍。

## 硬规矩（每个脚本都要满足）

| # | 规矩 | 为什么 |
| --- | --- | --- |
| 1 | 默认干跑，只有 `--apply` 才真做 | 出错时人还有拦截的机会 |
| 2 | 幂等，可重复运行 | 定时任务会重试，第二遍不许造成第二次伤害 |
| 3 | 动作与结果追加写 `logs/<脚本名>.log`，失败退出码非 0 | 无人值守也要可审计 |
| 4 | 改本机状态必须是临时的：`trap`/`finally` 恢复 + 事后核验 | 见经验库 `restore-system-changes` |

## 边界（脚本不扩大权限范围）

免确认 ≠ 免判断。仍然**绝对禁止**：

- `git push --force`、改写已推送历史、动 `main` 分支；
- 删除或移动仓库目录与 `target/` 以外的任何用户文件；
- `rm -rf` / `Remove-Item -Recurse` 作用到仓库外路径；
- 关电源、关防火墙、改 hosts、改系统配置这类"顺手"的机器级改动；
- 用脚本包装一个本来会被拒绝的操作，以此绕开策略。

做不到就**不做**，在 `docs/RUNLOG.md` 里写清卡在哪。

## 现有脚本

| 脚本 | 用途 | 干跑 |
| --- | --- | --- |
| `cleanup.ps1` | 清理 `.infy-build/`、`target/scratch*`、一次性测试二进制 | 是（默认） |
| `push-branch.ps1` | 带重试把 HEAD 推到受控分支，并回查远端算欠账（`main`/`master` 硬拒、绝不 force；远端是 `git@` 走 SSH 密钥，https 无凭据时用环境里的 `GITHUB_TOKEN` 现拼一次性 URL，日志中一律脱敏） | 有 `-DryRun`；推送是加法，正常调用即推 |
| `cron-lock.ps1` | **跨任务锁**（BACKLOG §0.0.1）：项目根 `cron.lock`，`acquire`/`check`/`release`/`status`。别人持锁且未过 TTL ⇒ **退出码 10 = 本轮静默放弃**；超 TTL 才允许抢占并记 `STEAL` | 是（`check`/`status` 纯只读；`release` 只放自己名下的锁） |
| `untrack-artifacts.ps1` | 把仍在跟踪的一次性产物**只从索引摘掉**（`git rm --cached`，文件留磁盘），封闭白名单 + 回滚命令进日志 | 是（默认） |
| `canonicalize-tour.ps1` | 把 `examples/tour` 写成 §9.5 的格式化不动点（F6/D-17 的执行入口）：范围只放开 `examples/tour/**/*.infy`，写完立刻回核 `fmt --check`，越界即还原 | 是（默认；`-Apply` 才写盘） |

## 本机调用方式（重要）

这台机**没有** `pwsh`（PowerShell 7），只有 Windows PowerShell 5.1。一律这样调：

    powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/<name>.ps1 [-Apply|-DryRun]

写 `.ps1` 有两条必须遵守的 5.1 坑（本轮踩过，两次都真实红过）：

1. **文件必须存成带 UTF-8 BOM**。5.1 对无 BOM 的文件按 ANSI 解码，中文注释与中文字符串会直接
   语法报错（`UnexpectedToken` / `TerminatorExpectedAtEndOfString`），跟脚本逻辑毫无关系。
2. **`Add-Content` 必须写 `-Encoding utf8`**（F6 轮实测的新坑，登记为 BACKLOG **F46**）：5.1 的缺省编码是
   `Default` = ANSI(GBK)，于是 `logs/*.log` 里的中文行在 bash / GitHub Actions / 任何 UTF-8 审阅器里全是乱码
   （`file logs/push-branch.log` = `ISO-8859 text`）。**审计面写进去却读不出，等于护栏只做一半**。既有三份脚本的
   `Log` 还没改（F46 的活），新脚本一律带上。
3. **不要在原生命令上开着 `$ErrorActionPreference='Stop'`**。PowerShell 5.1 会把原生命令写到
   stderr 的内容当成异常抛出，而 `git push` 的进度信息正是写 stderr —— 于是"推送失败"和
   "推送成功但有进度输出"都变成异常。脚本里的 `Run-Native` 已把这件事封好：临时降到
   `Continue`、拿回真实 `$LASTEXITCODE`、再恢复。新增脚本请复用它，别自己裸调。

另外：中文输出在本机控制台会显示成乱码（代码页问题），不影响判断——**以 `logs/*.log` 与
`gh api` 的回查结果为准**，不要信终端里的字形。

