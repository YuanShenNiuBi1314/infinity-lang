#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Sync the numbers F12 + F53 moved, across the outward-facing docs.

F12 landed a new public runtime entry (`infy_cleanup_cancel`) and turned the three §4.7.1 move
forms from "refused" into "emitted", so the live measurements moved:

  * cargo test --offline --lib   561   -> 570     (codegen 27 -> 33, parser 11 -> 14)
  * rt harness assertions       32340  -> 32353   (+13 = the four infy_cleanup_cancel groups)
  * tour artifact bytes         200644 -> 200700  (host image 199842 -> 199898)

UNCHANGED and deliberately not touched: C11 12512, build-id d7c4408e...370015, node table
7-8-9-10-16-20, conformance 83 = 77/0/6, IFBC 180/180. The build-id input set has no host
image, so a pure `infy_rt.c` change moves the artifact size and nothing else.

Why a script: the same number appears in several documents and in *historical* rows that must
keep their old value (a past round's measurement is a record, not a stale claim). Every rule
therefore names a full anchor substring and an expected occurrence count; validation is
all-or-nothing, so a half-updated document set cannot happen.

Idempotency: a rule whose `old` is absent is accepted when `new` already appears the expected
number of times, which is how a second run reports "already applied".

Usage:
  python scripts/sync-doc-numbers-f12.py            # dry run: per-rule counts, writes nothing
  python scripts/sync-doc-numbers-f12.py --apply    # write, then append the audit log
Rollback (logged too): git checkout -- docs/status.md docs/coverage.md README.md
"""
import io
import os
import sys
import datetime

# The console codepage on this host is GBK and every anchor below is Chinese: without this the
# report itself raises UnicodeEncodeError before it can say anything (measured on the dry run).
try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LOG = os.path.join(REPO, "logs", "sync-doc-numbers-f12.log")
ROLLBACK = "git checkout -- docs/status.md docs/coverage.md README.md"

# (file, old, new, expected)  -- old/new are full literal substrings
RULES = [
    # ---------------- docs/coverage.md : the P-table and the matrix rows ----------------
    ("docs/coverage.md",
     "`artifact ok — 200644 byte(s) 写到 …\\ci.exe（§7.4：宿主镜像 199842 + 段 blob 738 + 段尾 64，6 段）`",
     "`artifact ok — 200700 byte(s) 写到 …\\ci.exe（§7.4：宿主镜像 199898 + 段 blob 738 + 段尾 64，6 段；**F12 轮 200644 → 200700** = `infy_rt.c` 多了 `infy_cleanup_cancel`，而 C11/build-id/节点表三组一字未动）`",
     1),
    ("docs/coverage.md",
     "`verify-reproducible ok — 两个独立构建目录产物字节相同（200644 字节）`（§7.3 第 3 项",
     "`verify-reproducible ok — 两个独立构建目录产物字节相同（200700 字节）`（§7.3 第 3 项",
     1),
    ("docs/coverage.md",
     "`32340 assertions, 0 failures`，0 error / 0 warning；诊断渲染单独再跑一次",
     "`32353 assertions, 0 failures`，0 error / 0 warning（**F12 轮 32340 → 32353** = `infy_cleanup_cancel` 的四组）；诊断渲染单独再跑一次",
     1),
    ("docs/coverage.md",
     "P9 链 `infy_rt.c`（rt **32340** 项，C3 轮起）",
     "P9 链 `infy_rt.c`（rt **32353** 项，C3 轮起口径随 `infy_rt.c` 变动，F12 轮 32340 → 32353）",
     1),
    ("docs/coverage.md",
     "P9：`32340 assertions, 0 failures`",
     "P9：`32353 assertions, 0 failures`",
     1),
    ("docs/coverage.md",
     "12512/200644/`d7c4408e…370015` 逐字节不变",
     "12512/`d7c4408e…370015` 逐字节不变（产物字节随 `infy_rt.c` 动：F8 轮 200644、**F12 轮 200700**）",
     1),
    ("docs/coverage.md",
     "`--verify-reproducible`（**200644** 字节 = F8 轮起本机口径，重放它的那一轮记的是当时的 199975）",
     "`--verify-reproducible`（**200700** 字节 = F12 轮起本机口径；P30 那轮重放记的是当时的 199975，F8 轮到 200644）",
     1),
    ("docs/coverage.md",
     "产物仍 199975 字节 / blob 738（F6 轮口径；F8 轮改了 `infy_rt.c` ⇒ 本机现在是 200644 / blob 738 未变）",
     "产物仍 199975 字节 / blob 738（F6 轮口径；F8 轮改了 `infy_rt.c` ⇒ 200644，**F12 轮同因再 +56 ⇒ 200700** / blob 738 未变）",
     1),
    ("docs/coverage.md",
     "端到端 = P13（两个独立构建目录 200644 字节逐字节相同）",
     "端到端 = P13（两个独立构建目录 200700 字节逐字节相同）",
     1),
    ("docs/coverage.md",
     "`infy build examples/tour` 真产出 200644 字节的 §7.4 容器",
     "`infy build examples/tour` 真产出 200700 字节的 §7.4 容器",
     1),
    ("docs/coverage.md",
     "cargo test --offline --lib` 的 **561** 项按模块分布（**F11 轮实测分组计数** = 上一档 555 + 6",
     "cargo test --offline --lib` 的 **570** 项按模块分布（**F12/F53 轮实测分组计数** = 上一档 561 + 9：`codegen` 27 → **33** · `parser` 11 → **14**；再上一档 555 = F10/F51 轮 + 6",
     1),
    # ---------------- README.md ----------------
    ("README.md",
     "本机 MSYS2 `gcc 16.2.0` 产 200644",
     "本机 MSYS2 `gcc 16.2.0` 产 200700",
     1),
    ("README.md",
     "`cargo test --offline --lib` 目前 **561 passed / 0 failed**（**F11 轮 555 → 561**",
     "`cargo test --offline --lib` 目前 **570 passed / 0 failed**（**F12/F53 轮 561 → 570**：`codegen` 27 → **33** · `parser` 11 → **14**；上一档 **F11 轮 555 → 561**",
     1),
    ("README.md",
     "**32340 assertions / 0 failures**（B1 轮收敛，`-Wall -Wextra` 零告警",
     "**32353 assertions / 0 failures**（B1 轮收敛，`-Wall -Wextra` 零告警；**F12 轮 32340 → 32353** = `infy_cleanup_cancel` 的四组",
     1),
]


def read(path):
    with io.open(path, "r", encoding="utf-8", newline="") as fh:
        return fh.read()


def main():
    apply_changes = "--apply" in sys.argv[1:]
    lines = []
    ok = True
    cache = {}
    for rel, old, new, expected in RULES:
        path = os.path.join(REPO, rel.replace("/", os.sep))
        if path not in cache:
            cache[path] = read(path)
        text = cache[path]
        n_old = text.count(old)
        n_new = text.count(new)
        if n_old == expected:
            verdict = "APPLY x%d" % expected
            cache[path] = text.replace(old, new)
        elif n_old == 0 and n_new >= expected:
            verdict = "ALREADY x%d" % n_new
        else:
            verdict = "MISS old=%d want=%d new=%d" % (n_old, expected, n_new)
            ok = False
        lines.append("%-18s %s\n    old: %s\n    new: %s" % (rel, verdict, old[:88], new[:88]))
    stamp = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    mode = "APPLY" if apply_changes else "DRYRUN"
    print("[%s] %s — %d rule(s)" % (stamp, mode, len(RULES)))
    for ln in lines:
        print(ln)
    if not ok:
        print("RESULT: validation failed, nothing written")
        _log(stamp, mode, "FAILED", lines)
        return 1
    if apply_changes:
        for rel in {r[0] for r in RULES}:
            path = os.path.join(REPO, rel.replace("/", os.sep))
            if path in cache and cache[path] != read(path):
                with io.open(path, "w", encoding="utf-8", newline="") as fh:
                    fh.write(cache[path])
                print("wrote %s" % rel)
        print("RESULT: applied. rollback = %s" % ROLLBACK)
        _log(stamp, mode, "OK", lines + ["rollback: " + ROLLBACK])
    else:
        print("RESULT: dry run, nothing written")
    return 0


def _log(stamp, mode, result, lines):
    d = os.path.dirname(LOG)
    if not os.path.isdir(d):
        os.makedirs(d)
    with io.open(LOG, "a", encoding="utf-8", newline="") as fh:
        fh.write("%s | %s | %s | %d rules\n" % (stamp, mode, result, len(RULES)))
        for ln in lines:
            fh.write("    " + ln.replace("\n", " / ") + "\n")


if __name__ == "__main__":
    sys.exit(main())
