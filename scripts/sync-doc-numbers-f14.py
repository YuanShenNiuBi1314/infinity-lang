#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Sync the numbers F14 moved, across the outward-facing docs.

F14 fixed constructor-pattern bindings, which moved only the Rust-side test count:

  * cargo test --offline --lib   576 -> 580   (sema 35 -> 37, codegen 33 -> 34, borrow 22 -> 23)

That stage-1 mapping was written while F14 was still the only round in flight. F14 did not land on
top of F13: F58 rebuilt `capabilities/IFBC-CAP-py-3.13.toml` first and added +3 to `btrack`, so the
live totals the documents actually need are **583** (576 + F58's 3 + F14's 4) and the `btrack` row is
**34**, not 31. Stage 2 below moves every live claim to that number; the historical rows keep the
figure of the round that measured them.

UNCHANGED and deliberately not touched: rt harness 32353 (except README's runtime row, which still
claimed the F8-era 32340 while the same file listed 32353 two sections further down), tour artifact
200700 bytes, C11 12512 bytes, build-id d7c4408e...370015, node table 7-8-9-10-16-20, conformance
83 = 77/0/6, IFBC 180/180. The change is front-end + one parser shape; no `infy_rt.c` edit and
`examples/tour` contains no constructor binding, so the emitted C is byte-for-byte the same size.

Why a script: the same number appears in several documents and in *historical* rows that must keep
their old value (a past round's measurement is a record, not a stale claim). Every rule names a
full anchor substring and an expected occurrence count; validation is all-or-nothing, so a
half-updated document set cannot happen. `scripts/check-doc-numbers.py` re-verifies afterwards.

Idempotency: a rule whose `old` is absent is accepted when `new` already appears the expected
number of times, which is how a second run reports "already applied".

Usage:
  python scripts/sync-doc-numbers-f14.py            # dry run: per-rule counts, writes nothing
  python scripts/sync-doc-numbers-f14.py --apply    # write, then append the audit log
Rollback (logged too): git checkout -- docs/status.md docs/coverage.md README.md
"""
import io
import os
import sys
import datetime

try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LOG = os.path.join(REPO, "logs", "sync-doc-numbers-f14.log")
ROLLBACK = "git checkout -- docs/status.md docs/coverage.md README.md"

F14_DELTA = "`sema` 35 → **37** · `codegen` 33 → **34** · `borrow` 22 → **23**"

RULES = [
    # ---------------- docs/coverage.md : the per-module table ----------------
    ("docs/coverage.md", "| `sema` | **35** |", "| `sema` | **37** |", 1),
    ("docs/coverage.md", "| `codegen` | **33** |", "| `codegen` | **34** |", 1),
    ("docs/coverage.md", "| `borrow` | 22 |", "| `borrow` | 23 |", 1),
    # ---------------- docs/coverage.md : P1 and the section intro ----------------
    ("docs/coverage.md",
     "`576 passed; 0 failed; 0 ignored`（**F13 轮基线 576 = F12/F53 轮 570 + 6**，全在 `sema` 29 → **35**",
     "`580 passed; 0 failed; 0 ignored`（**F14 轮基线 580 = F13 轮 576 + 4**，" + F14_DELTA +
     " = 构造器模式的绑定进臂体作用域（`Option`/`Result` 闭集穷尽性 + 裸 `None` 是单元构造器不是新绑定，"
     "裁定 = 偏差 **D-56**）；五条变异探针各自实测能红（`scripts/f14-mutation-probe.ps1`）；"
     "**上一档 F13 轮基线 576 = F12/F53 轮 570 + 6**，全在 `sema` 29 → **35**", 1),
    ("docs/coverage.md",
     "`cargo test --offline --lib` 的 **576** 项按模块分布（**F13 轮实测分组计数** = 上一档 570 + 6，全在 `sema` **29 → 35**",
     "`cargo test --offline --lib` 的 **580** 项按模块分布（**F14 轮实测分组计数** = 上一档 576 + 4：" +
     F14_DELTA.replace("**", "") + "，四组全是构造器模式绑定的锁点，见偏差 **D-56**；上一档 **F13 轮实测分组计数** = 上一档 570 + 6，全在 `sema` **29 → 35**", 1),
    # ---------------- docs/status.md : the live head block ----------------
    ("docs/status.md",
     "test result: ok. 576 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out   # **F13 轮 570 → 576**，`sema` 29→**35**（负字面量",
     "test result: ok. 580 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out   # **F14 轮 576 → 580**，"
     + F14_DELTA + "（构造器模式的绑定进臂体作用域，裁定 = 偏差 **D-56**，探针 `scripts/f14-mutation-probe.ps1`）；"
     "**上一档 F13 轮 570 → 576**，`sema` 29→**35**（负字面量", 1),
    ("docs/status.md",
     "576 项按模块（`cargo test --offline --lib -j 1 -- --list` 逐条分组计数，合计 576；**F13 轮实测**，上一版 570 = F12/F53 轮实测",
     "580 项按模块（`cargo test --offline --lib -j 1 -- --list` 逐条分组计数，合计 580；**F14 轮实测** = 上一版 576 + 4："
     + F14_DELTA + "；上一版 570 = F12/F53 轮实测", 1),
    ("docs/status.md", "`sema` **35**", "`sema` **37**", 1),
    ("docs/status.md", "`codegen` **33**（D3 轮 22 → 26", "`codegen` **34**（D3 轮 22 → 26", 1),
    ("docs/status.md", "`borrow` **22**（**F7 轮 15 → 22**", "`borrow` **23**（**F7 轮 15 → 22**", 1),
    ("docs/status.md",
     "= **576**（逐模块相加：79+58+41+47+35+31+27+**33**+24+**35**+22+22+18+15+22+12+19+9+**14**+7+4+2，"
     "与 `-- --list` 的 576 行一致）",
     "= **580**（逐模块相加：79+58+41+47+35+31+27+**34**+24+**37**+22+22+18+15+23+12+19+9+**14**+7+4+2，"
     "与 `-- --list` 的 580 行一致）", 1),
    # ---------------- README.md ----------------
    ("README.md",
     "`cargo test --offline --lib` 目前 **576 passed / 0 failed**（**F13 轮 570 → 576**：`sema` 29 → **35**",
     "`cargo test --offline --lib` 目前 **580 passed / 0 failed**（**F14 轮 576 → 580**：" + F14_DELTA +
     " = 构造器模式的绑定进臂体作用域，裁定 = 偏差 **D-56**；**上一档 F13 轮 570 → 576**：`sema` 29 → **35**", 1),
]

# ---------------------------------------------------------------- stage 2
# F14's own delta is +4, but it did not land straight on top of F13: F58 (the rebuilt
# `IFBC-CAP-py-3.13.toml` + the `cap_opmap` version contract) added +3 to `btrack` first.
# The stage-1 rules above therefore wrote totals that were already one round stale. Stage 2
# moves every live claim to the number the test binary actually lists (583) and carries the
# `btrack` row from 31 to 34, so `scripts/check-doc-numbers.py` has one truth to compare.
F58_NOTE = "`btrack` 31 → **34**（**F58 轮**：3.13 能力表按 CPython 3.13 自己的发布件重建，" \
           "外加 `cap_opmap::opcode_contract` 这份与表不同源的版本契约，装载期三方向校验报 `E-0713`）"

RULES += [
    # ---------------- docs/coverage.md : P1 + the section intro + the btrack row ----------------
    ("docs/coverage.md",
     "`580 passed; 0 failed; 0 ignored`（**F14 轮基线 580 = F13 轮 576 + 4**，",
     "`583 passed; 0 failed; 0 ignored`（**F14 轮基线 583 = F13 轮 576 + F58 轮 3 + F14 轮 4**，" + F58_NOTE + " · ", 1),
    ("docs/coverage.md",
     "`cargo test --offline --lib` 的 **580** 项按模块分布（**F14 轮实测分组计数** = 上一档 576 + 4：",
     "`cargo test --offline --lib` 的 **583** 项按模块分布（**F14 轮实测分组计数** = 上一档 579 + 4，"
     "其间 " + F58_NOTE + " 把 576 抬到 579：", 1),
    ("docs/coverage.md", "| `btrack` | **31** | §7.7.1 的 [6a] 入口", "| `btrack` | **34** | §7.7.1 的 [6a] 入口", 1),
    ("docs/coverage.md",
     "两成员 stored 归档里每个成员按**自己的** local header 偏移取数据（F2 轮 +1，负对照 = 把 `data_off` 写回 0 即红） |",
     "两成员 stored 归档里每个成员按**自己的** local header 偏移取数据（F2 轮 +1，负对照 = 把 `data_off` 写回 0 即红）。"
     "**F58 轮 +3** = 3.13 表的数值行必须由 3.13 自己的发布件复现、契约与表**不同源**、装载期三个方向都校验"
     "（必有名缺席 / 外来名 / 未知数值键 ⇒ `E-0713`） |", 1),
    # ---------------- docs/status.md : head block, per-module lines and the addition ----------------
    ("docs/status.md",
     "test result: ok. 580 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out   # **F14 轮 576 → 580**，",
     "test result: ok. 583 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out   # "
     "**F14 轮 579 → 583**（其间 " + F58_NOTE + " ⇒ 576 → 579），", 1),
    ("docs/status.md",
     "580 项按模块（`cargo test --offline --lib -j 1 -- --list` 逐条分组计数，合计 580；**F14 轮实测** = 上一版 576 + 4：",
     "583 项按模块（`cargo test --offline --lib -j 1 -- --list` 逐条分组计数，合计 583；**F14 轮实测** = 上一版 579 + 4：", 1),
    ("docs/status.md",
     "`btrack` **31**（D1(1) 前是 0，D1(2) 轮 23 → 30，**F2 轮 +1** = wheel 归档里每个成员按自己的 local header 偏移取数据）",
     "`btrack` **34**（D1(1) 前是 0，D1(2) 轮 23 → 30，**F2 轮 +1** = wheel 归档里每个成员按自己的 local header 偏移取数据，"
     "**F58 轮 +3** = 3.13 能力表的版本契约三方向校验）", 1),
    ("docs/status.md",
     "= **580**（逐模块相加：79+58+41+47+35+31+27+**34**+24+**37**+22+22+18+15+23+12+19+9+**14**+7+4+2，"
     "与 `-- --list` 的 580 行一致）",
     "= **583**（逐模块相加：79+58+41+47+35+**34**+27+**34**+24+**37**+22+22+18+15+23+12+19+9+**14**+7+4+2，"
     "与 `-- --list` 的 583 行一致）", 1),
    # ---------------- README.md ----------------
    ("README.md",
     "`cargo test --offline --lib` 目前 **580 passed / 0 failed**（**F14 轮 576 → 580**：",
     "`cargo test --offline --lib` 目前 **583 passed / 0 failed**（**F14 轮 579 → 583**（其间 " + F58_NOTE + " ⇒ 576 → 579）：", 1),
    # README's runtime row carried the F8-era harness total while the same file already listed the
    # F12 total two sections below it -- the "one number written twice, only one half updated" shape.
    ("README.md",
     "→ **32340 assertions / 0 failures**（0 error / 0 warning，链的是生产 `infy_rt.c`）",
     "→ **32353 assertions / 0 failures**（0 error / 0 warning，链的是生产 `infy_rt.c`；**F12 轮 32340 → 32353** = "
     "`infy_cleanup_cancel` 的四组）", 1),
]


def log(lines):
    stamp = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    d = os.path.dirname(LOG)
    if not os.path.isdir(d):
        os.makedirs(d)
    with io.open(LOG, "a", encoding="utf-8", newline="\n") as fh:
        for ln in lines:
            fh.write("%s  %s\n" % (stamp, ln))


def main():
    apply = "--apply" in sys.argv
    texts = {}
    report = []
    bad = 0
    for i, (rel, old, new, want) in enumerate(RULES):
        if rel not in texts:
            with io.open(os.path.join(REPO, rel), encoding="utf-8", newline="") as fh:
                texts[rel] = fh.read()
        t = texts[rel]
        hits = t.count(old)
        done = t.count(new)
        if hits == 0 and done >= want:
            report.append("rule %d %-18s ALREADY-APPLIED (%d x new)" % (i, rel, done))
            continue
        if hits != want:
            report.append("rule %d %-18s MISMATCH hits=%d want=%d anchor=%s" % (i, rel, hits, want, old[:48]))
            bad += 1
            continue
        report.append("rule %d %-18s ok (%d site%s)" % (i, rel, hits, "" if hits == 1 else "s"))
        texts[rel] = t.replace(old, new)
    for ln in report:
        print(ln)
    if bad:
        print("ABORT: %d rule(s) did not match; nothing written (all-or-nothing)" % bad)
        log(report + ["ABORT mismatches=%d -- nothing written" % bad])
        return 1
    if not apply:
        print("DRY RUN: %d rule(s) validated, nothing written (add --apply)" % len(RULES))
        log(["DRY RUN validated %d rules, nothing written" % len(RULES)])
        return 0
    for rel in sorted(texts):
        with io.open(os.path.join(REPO, rel), "w", encoding="utf-8", newline="\n") as fh:
            fh.write(texts[rel])
    print("APPLIED %d rules across %d file(s); rollback: %s" % (len(RULES), len(texts), ROLLBACK))
    log(report + ["APPLIED %d rules; rollback: %s" % (len(RULES), ROLLBACK)])
    return 0


if __name__ == "__main__":
    sys.exit(main())
