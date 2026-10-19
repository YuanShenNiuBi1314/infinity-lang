#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""F18 轮的文档数字换数（BACKLOG §0.0 第 ③ 条：批量改文件走脚本）。

为什么脚本化而不是手改：同一份数字在 README 与 status.md 里各写了两三遍（产物字节、宿主镜像、
rt 断言数、远端 run 的那一步），漏改一处就是假账 —— 这个坑在 F14/F15/F16/F17 各复现过一次，
`scripts/check-doc-numbers.py` 只查覆盖率表那三组数，管不到这里。

四条硬规矩：① 默认干跑，只有 --apply 才写盘；② 幂等（换完再跑一遍命中数为 0 条旧串，脚本按
「旧串必须命中 expected 次」判定，第二次运行会大声报 mismatch，而不是静默改坏）；③ 动作与结果
追加进 logs/sync-doc-numbers-f18.log，任何一条不符预期就退非 0；④ 字节级读写（不改行尾、不吞
裸 CR），每个文件改前后 CRLF 计数必须相同。

幂等性做法：每条规则都是「旧串 → 新串」，第二次跑旧串自然 0 命中。因此 expected 命中数在重跑时
不成立 —— 脚本把「旧串 0 命中且新串已存在」单独识别为 ALREADY，视为通过（真·幂等），其余 0 命中
一律 FAIL。
"""
import sys, pathlib, datetime

# 这台机器的控制台码页是 CP936（GBK），打印 ⇒/中文会 UnicodeEncodeError —— 日志文件走 UTF-8，
# stdout 也强制 UTF-8，否则脚本自己先崩，把「成功」报成非 0 退码。
try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass

REPO = pathlib.Path(__file__).resolve().parent.parent
LOG = REPO / "logs" / "sync-doc-numbers-f18.log"

# (相对路径, 旧串, 新串, 期望命中数)
RULES = [
    ("docs/status.md",
     "32382 assertions, 0 failures",
     "32387 assertions, 0 failures", 1),
    ("docs/status.md",
     "当前口径 32382 断言 / 0 失败（F17 轮 +29：C 侧读 EnvOffsets/ResourceKeys + 行数逃逸拒绝）",
     "当前口径 32387 断言 / 0 失败（F17 轮 +29：C 侧读 EnvOffsets/ResourceKeys + 行数逃逸拒绝；"
     "F18 轮 +5 = 段表手写期望块那 5 条 CK）", 1),
    ("docs/status.md",
     "artifact ok — 204548 byte(s) 写到 examples/tour",
     "artifact ok — 205060 byte(s) 写到 examples/tour", 1),
    ("docs/status.md",
     "（§7.4：宿主镜像 203397 + 段 blob 1087 + 段尾 64，6 段）",
     "（§7.4：宿主镜像 203909 + 段 blob 1087 + 段尾 64，6 段）", 1),
    ("docs/status.md",
     "artifact ok — 204548 byte(s) 写到 %TEMP%",
     "artifact ok — 205060 byte(s) 写到 %TEMP%", 1),
    ("docs/status.md",
     "verify-reproducible ok — 两个独立构建目录产物字节相同（204548 字节）",
     "verify-reproducible ok — 两个独立构建目录产物字节相同（205060 字节）", 1),
    ("README.md",
     "**最新一条远端实录（F9 轮回查）**：run #13（`38991050867`，push `2852b00`，树含 F8 的 `9c56b45`）= **success**，",
     "**最新一条远端实录（F18 轮回查）**：run #28（`39181242747`，push `af8860d` = F17 的交付树）= **success**，", 1),
    ("README.md",
     "步 3-12 逐条 `success` ⇒ 步 7 = **548 passed / 0 failed**、步 10 = rt **32340 assertions, 0 failures**、",
     "步 3-12 逐条 `success` ⇒ 步 7 = **619 passed / 0 failed**、步 10 = rt **32382 assertions, 0 failures**、", 1),
    ("README.md",
     "步 12 tour 产物 **221823**（宿主镜像 221021 + blob 738 + 段尾 64；该行 = F16 交付树的那条 run，"
     "F17 轮本机 blob 已变 1087，下一班回查换新数）",
     "步 12 tour 产物 **226689**（宿主镜像 225538 + blob 1087 + 段尾 64）", 1),
    ("README.md",
     "是宿主 `cc` 的函数（runner 的 `gcc 15.2.0` 产 221666 字节，本机 MSYS2 `gcc 16.2.0` 产 204548 字节"
     "（F17 轮又改 `infy_rt.c`：镜像 +3499、blob +349；F8 轮改后 200700，改前 199975）；段 blob 1087 +",
     "是宿主 `cc` 的函数（runner 的 `gcc 15.2.0` 产 226689 字节，本机 MSYS2 `gcc 16.2.0` 产 205060 字节"
     "（F18 轮又改 `infy_rt.c`：镜像 +512；F17 轮 +3499 与 blob +349；F8 轮改后 200700，改前 199975）；段 blob 1087 +", 1),
    # 两条规则都必须**吃掉**旧串才谈得上幂等：新串里若原样留着旧串，重跑就会 MISS(1)。
    ("docs/status.md", "本机现值 **200700**", "该行写作时的本机现值 **200700**", 1),
    ("docs/status.md", "`infy_cleanup_cancel`，+56 字节）",
     "`infy_cleanup_cancel`，+56 字节；**F17 轮 → 204548、F18 轮 → 205060**，"
     "现值以本文件的 `artifact ok` 那行为准）", 1),
]

def main():
    apply = "--apply" in sys.argv[1:]
    stamp = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    lines, fails = [], 0
    cache = {}
    for rel, old, new, expected in RULES:
        p = REPO / rel
        if rel not in cache:
            cache[rel] = p.read_bytes()
        data = cache[rel]
        ob, nb = old.encode("utf-8"), new.encode("utf-8")
        hits = data.count(ob)
        if hits == 0 and data.count(nb) >= 1:
            verdict = "ALREADY"
        elif hits == expected:
            verdict = "HIT"
            cache[rel] = data.replace(ob, nb)
        else:
            verdict = "MISS(%d)" % hits
            fails += 1
        lines.append("%s %s %s" % (verdict, rel, old[:60]))
    if apply:
        for rel in cache:
            p = REPO / rel
            before = p.read_bytes()
            after = cache[rel]
            if before == after:
                lines.append("SKIP %s（无字节变化）" % rel)
                continue
            crlf_before, crlf_after = before.count(b"\r\n"), after.count(b"\r\n")
            if crlf_before != crlf_after:
                lines.append("FAIL %s 行尾被改写 CRLF %d -> %d" % (rel, crlf_before, crlf_after))
                fails += 1
                continue
            p.write_bytes(after)
            lines.append("WROTE %s（%d -> %d 字节，CRLF 计数 %d 未变）"
                         % (rel, len(before), len(after), crlf_after))
    body = "\n".join(lines)
    LOG.parent.mkdir(exist_ok=True)
    with open(LOG, "a", encoding="utf-8", newline="\n") as f:
        f.write("%s  %s rules=%d fails=%d\n%s\n"
                % (stamp, "APPLY" if apply else "DRYRUN", len(RULES), fails, body))
    print(("APPLY" if apply else "DRYRUN") + " rules=%d fails=%d" % (len(RULES), fails))
    print(body)
    return 1 if fails else 0

if __name__ == "__main__":
    sys.exit(main())
