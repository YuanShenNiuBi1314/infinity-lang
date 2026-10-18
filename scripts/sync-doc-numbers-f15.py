#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Sync the numbers F15 moved, across the outward-facing docs.

F15 gave `fmt!` a lowering path (stage [2] rewrite -> stage [4] compile-time validation -> stage [8]
`InfyFmtSpec` table + `infy_fmt_run`), which moved the Rust-side test count and nothing else:

  * cargo test --offline --lib -j 1   583 -> 606
    (`fmtstr` 0 -> 10 new module, `macro_expand` 35 -> 40, `sema` 37 -> 40,
     `codegen` 34 -> 37, `parser` 14 -> 15, `cli` 47 -> 48)

It also closed two defects that F15's own acceptance sample (§14.2's `describe`) stumbled over:
**F63** (stage [1] `delimited_group` swallowed every match arm after a macro-call body, with no
diagnostic) and **F64** (stage [4] compared a *type name* against a *package path*, so every private
field access inside the declaring package was an `E-0210`).

UNCHANGED and deliberately not touched: rt harness 32353, IFBC 180/180, tour artifact 200700 bytes,
C11 12512 bytes, build-id d7c4408e...370015, node table 7-8-9-10-16-20, conformance 83 = 77/0/6. No
`runtime/*.c` edit and `examples/tour` contains no `fmt!`, so the emitted C is byte-for-byte the same.
Historical rows keep the figure of the round that measured them -- which is why this is a rule table
instead of a sed over "583".

Rule semantics (each entry: file, old, new, expected count, then two optional flags):
  * `skip_marker` -- if this substring is already present, the rule is a no-op. Insert-type rules
    need it: their `old` anchor (the row they insert *before*) survives the insertion, and without a
    marker a second run would insert the row again. That is exactly how this script, on 2026-10-19,
    wrote the `P40` row and the `fmtstr` row three times each; the two `tolerant` dedupe rules below
    collapsed them back to one and now keep the whole thing re-runnable.
  * `tolerant` -- `hits == 0` is accepted as a no-op instead of a mismatch.
Validation is all-or-nothing: any non-tolerant mismatch writes nothing at all.
`scripts/check-doc-numbers.py` re-verifies afterwards.

Line endings: docs/coverage.md is CRLF in this worktree (`core.autocrlf=true`, and `*.md` has no eol
rule in .gitattributes), so anchors and insertions take the file's own ending instead of mixing two
shapes into one file. The committed blob is LF either way.

Usage:
  python scripts/sync-doc-numbers-f15.py            # dry run: per-rule counts, writes nothing
  python scripts/sync-doc-numbers-f15.py --apply    # write, then append the audit log
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
LOG = os.path.join(REPO, "logs", "sync-doc-numbers-f15.log")
ROLLBACK = "git checkout -- docs/status.md docs/coverage.md README.md"

F15_DELTA = (
    "新模块 `fmtstr` 0 → **10** · `macro_expand` 35 → **40** · `sema` 37 → **40** · "
    "`codegen` 34 → **37** · `parser` 14 → **15** · `cli` 47 → **48**"
)
F15_WHY = (
    "= §10.11 的 `fmt!` 三段都有锁点了：阶段 [2] 把无定义的 `fmt!(…)` 改写成 §2.5 的 `fmt(…)` 调用形态、"
    "阶段 [4] 在编译期解析格式串（`E-0180`/`E-0181` 由此第一次有发射点）、阶段 [8] 发 `InfyFmtSpec` 表 + "
    "`infy_fmt_run` 调用（裁定 = 偏差 **D-58(a)–(h)**，实测见 `docs/coverage.md` 的 **P40**）"
)

P6_ANCHOR = "| P6 | `gcc -std=c11 -O1 -Wall -Wextra runtime/infy_ifbc.c"
PKG_ANCHOR = "| `pkg` | 24 |"

P40_ROW = (
    "| P40 | `cargo test --offline --lib -j 1` + `cargo check --offline --lib -j 1` + "
    "`cargo build --offline --release -j 1` + release 二进制逐条实跑 `target/scratch/f15/` 的 "
    "`p1|p2|p3|p9|p12|p14|p18` 七份最小 module + 四条 C 链复跑 + tour 七连 + `conformance --report` | "
    "**F15 = §10.11 的 `fmt!` 有 lowering 路径了**（裁定 = 偏差 **D-58(a)–(h)**，顺带关掉 **F63**"
    "（阶段 [1] 的宏调用吞掉它后面的臂）与 **F64**（`E-0210` 拿类型名比包路径））。实现面四段 = "
    "`src/fmtstr.rs`（新，格式串的唯一解析者）+ `macro_expand::lower_builtin_fmt`（阶段 [2] 改写）+ "
    "`sema::check_builtin_fmt`（阶段 [4] 校验并取 `String`）+ `codegen::builtin_fmt`/`fmt_arg`"
    "（阶段 [8] 发 `static const InfyFmtSpec …[]` 与 `(const InfyFmtArg[n]){…}` 再调 `infy_fmt_run`）"
    "。**实测**：`cargo test --offline --lib -j 1` **583 → 606 passed / 0 failed**（+23 = "
    "`fmtstr` **10**（新模块）· `macro_expand` 35→**40** · `sema` 37→**40** · `codegen` 34→**37** · "
    "`parser` 14→**15** · `cli` 47→**48**）、`cargo check --offline --lib -j 1` **0 error / 0 warning**、"
    "`cargo build --offline --release -j 1` exit **0**。验收本体 = §14.2 的 `describe` 样例（`fmt!` 在三条臂里、"
    "返回 `String`）`infy check` **exit 0**（私有字段不再 `E-0210`、穷尽性不再假红）与 `infy build` "
    "**exit 0**（宿主 `cc` 编过，产物 197849 字节），生成的 C 里 **3 张 `InfyFmtSpec` 表 + 3 处 "
    "`infy_fmt_run(infy_fmt_spec…)`**，行形状 `{ .lit = \"fetch #\", .lit_len = 7u, .arg_index = 0u, "
    ".which = FY_AUTO, … }`；反例两条也是实测的：`fmt!(\"x\", 1u32)`（实参未用尽）**exit 2 + `E-0180`**、"
    "`fmt(s, x)`（格式串非常量）**exit 2 + `E-0181`**。C 侧零改动仍复跑：两份 `-c` 各 stderr **0 字节**、"
    "rt **32353/0**、两条 IFBC **180/180** + `cmp` 无差异；tour 七连全 exit **0** 且 C11 **12512** / 产物 "
    "**200700** / build-id **`d7c4408e…370015`** / 节点表 **7-8-9-10-16-20** 一字未动；"
    "`infy conformance --report` 仍 **83 = 77/0/6 ⇒ exit 1**（92.8%）。**剩余面**（不许读成「`fmt` 全部打通」）："
    "**F65** = §10.11 的 `write_fmt` 与 `fmt_dynamic` 没有任何落点，`E-0181` 的 help 指向的 `fmt_dynamic` "
    "今天不存在；ADT/`Option`/`Result`/容器与 `u128`/`i128` 实参在阶段 [8] 被点名拒绝（前者要 `InfyDebugVt` "
    "= **F35** 的另一半），故 §10.11 的 `Display`/`Debug`/`LowerHex` trait 面今天只对**标量**成立 | 0 |"
)

FMTSTR_ROW = (
    "| `fmtstr` | **10** | **F15 轮新建** = §10.11 格式串的编译期解析，阶段 [4] 与阶段 [8] 共用这**一份**"
    "实现（否则「校验过的」与「发出来的」可以各说各话）：占位符带它**前面**的字面量段、尾段发一行 "
    "`INFY_FMT_TAIL_ONLY`、`{{`/`}}` 是转义而孤立 `}` 拒绝、`{0}` 只数**位置**实参、具名占位符按名归位、"
    "实参未用尽拒绝、封闭的格式选项集（`{:d}`/`{:#p}`/`{:..3}`/动态宽度 `*` 逐条红而不是回落成 `FY_AUTO`）、"
    "`InfyFmtWhich` 的枚举名必须与 `runtime/infy_rt.h` 同源（`include_str!` 读回头文件比序列）、发射行的字段名"
    "与尾哨兵逐字符锁 |"
)

# (file, old, new, want, skip_marker, tolerant)
RULES = [
    # ---------------- docs/coverage.md : the per-module table ----------------
    ("docs/coverage.md", "| `macro_expand` | 35 | §6.1–§6.5 |",
     "| `macro_expand` | **40** | §6.1–§6.5；**F15 轮 +5** = 内建 `fmt!` 在阶段 [2] 的 lowering："
     "改写成 `fmt(…)` 调用形态、具名实参 `w=8` 落成 `CallArg::name`、语句位与尾表达式位都改、"
     "用户 `macro_rules! fmt` 优先于内建、`fmt!` 实参里嵌套的用户宏仍被展开（改写发生在取调用点**之前**） |",
     1, None, False),
    ("docs/coverage.md", "| `sema` | **37** |", "| `sema` | **40** |", 1, None, False),
    ("docs/coverage.md", "| `codegen` | **34** |", "| `codegen` | **37** |", 1, None, False),
    ("docs/coverage.md", "| `parser` | **14** |", "| `parser` | **15** |", 1, None, False),
    ("docs/coverage.md", "| `cli` | **47** |", "| `cli` | **48** |", 1, None, False),
    ("docs/coverage.md", PKG_ANCHOR, FMTSTR_ROW + "\n" + PKG_ANCHOR, 1, "| `fmtstr` |", False),
    # ---------------- docs/coverage.md : P1, the section intro, the new P40 ----------------
    ("docs/coverage.md",
     "`583 passed; 0 failed; 0 ignored`（**F14 轮基线 583 = F13 轮 576 + F58 轮 3 + F14 轮 4**，",
     "`606 passed; 0 failed; 0 ignored`（**F15 轮基线 606 = F14 轮 583 + 23**：" + F15_DELTA + " " + F15_WHY +
     "；**上一档 F14 轮基线 583 = F13 轮 576 + F58 轮 3 + F14 轮 4**，", 1, None, False),
    ("docs/coverage.md",
     "的 **583** 项按模块分布（**F14 轮实测分组计数** = 上一档 579 + 4，",
     "的 **606** 项按模块分布（**F15 轮实测分组计数** = 上一档 583 + 23：" + F15_DELTA +
     "，六组全是 §10.11 的 `fmt!`（见 **P40** 与偏差 **D-58**）；**上一档 F14 轮实测分组计数** = 上一档 579 + 4，",
     1, None, False),
    ("docs/coverage.md", P6_ANCHOR, P40_ROW + "\n" + P6_ANCHOR, 1, "| P40 |", False),
    # the two rows an earlier run of this script wrote three times
    ("docs/coverage.md", (P40_ROW + "\n") * 3, P40_ROW, 1, None, True),
    ("docs/coverage.md", (FMTSTR_ROW + "\n") * 3, FMTSTR_ROW, 1, None, True),
    # ---------------- docs/status.md ----------------
    ("docs/status.md",
     "test result: ok. 583 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out   # **F14 轮 579 → 583**（其间",
     "test result: ok. 606 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out   # "
     "**F15 轮 583 → 606**（" + F15_DELTA + " = §10.11 的 `fmt!` 三段，裁定 = 偏差 **D-58**，实测见 "
     "`docs/coverage.md` 的 **P40**）；**上一档 F14 轮 579 → 583**（其间", 1, None, False),
    ("docs/status.md",
     "583 项按模块（`cargo test --offline --lib -j 1 -- --list` 逐条分组计数，合计 583；**F14 轮实测** = 上一版 579 + 4：",
     "606 项按模块（`cargo test --offline --lib -j 1 -- --list` 逐条分组计数，合计 606；**F15 轮实测** = "
     "上一档 583 + 23：" + F15_DELTA + "；**上一档 F14 轮实测** = 上一版 579 + 4：", 1, None, False),
    ("docs/status.md", "`macro_expand` 35 · `btrack`",
     "`macro_expand` **40**（**F15 轮 +5** = 内建 `fmt!` 的阶段 [2] lowering 五锁，见 P40） · `btrack`",
     1, "**F15 轮 +5** = 内建 `fmt!` 的阶段 [2] lowering 五锁", False),
    ("docs/status.md", "`codegen` **34**（D3 轮 22 → 26",
     "`codegen` **37**（**F15 轮 +3** = `fmt` 的 specs 表与实参数组发射、具名实参索引、放不下载体的点名拒绝；"
     "D3 轮 22 → 26", 1, None, False),
    ("docs/status.md", "`parser` **14**（D3 轮 +1 = §3.6 的 `@c` 语句形态",
     "`fmtstr` **10**（**F15 轮新建** = §10.11 格式串的编译期解析，阶段 [4]/[8] 共用同一份实现，"
     "`InfyFmtWhich` 的枚举名与 `runtime/infy_rt.h` 同源） · `parser` **15**（**F15 轮 +1** = "
     "`a_macro_call_arm_body_does_not_swallow_the_later_arms`：宏调用做臂体时不再吞掉后面的臂（**F63**）；"
     "D3 轮 +1 = §3.6 的 `@c` 语句形态", 1, "`fmtstr` **10**", False),
    ("docs/status.md", "`sema` **37**（", "`sema` **40**（", 1, None, False),
    ("docs/status.md", "`cli` **47**（", "`cli` **48**（", 1, None, False),
    # ---------------- docs/status.md : the "sum the modules" line the checker parses ----------------
    # `check-doc-numbers.py` reads the expression as `[0-9+→*]+`, so nothing but numbers, `+`, `→` and
    # the `**` bold markers may sit inside it -- the `fmtstr` note goes after the closing comma.
    ("docs/status.md",
     "= **583**（逐模块相加：79+58+41+47+35+**34**+27+**34**+24+**37**+22+22+18+15+23+12+19+9+**14**+7+4+2，"
     "与 `-- --list` 的 583 行一致）",
     "= **606**（逐模块相加：79+58+41+**48**+**40**+**34**+27+**37**+24+**40**+22+22+18+15+23+12+19+9+**15**+7+4+2"
     "+**10**，与 `-- --list` 的 606 行一致；末档 **10** = `fmtstr`，F15 轮新建）", 1, None, False),
    # ---------------- README.md ----------------
    ("README.md",
     "`cargo test --offline --lib` 目前 **583 passed / 0 failed**（**F14 轮 579 → 583**（其间",
     "`cargo test --offline --lib` 目前 **606 passed / 0 failed**（**F15 轮 583 → 606**（" + F15_DELTA + " " +
     F15_WHY + "；**上一档 F14 轮 579 → 583**（其间", 1, None, False),
    ("README.md",
     "codegen 只发射 §10 里有 `infy_rt.h` 入口的那部分（`Vec`/`String`/`fmt` 的构造入口缺 → 见下）",
     "codegen 只发射 §10 里有 `infy_rt.h` 入口的那部分（**F15 轮起 `fmt` 的构造入口已接** = `fmt!`/`fmt(...)` "
     "发射成 `InfyFmtSpec` 表 + `infy_fmt_run`，见 `docs/coverage.md` 的 P40；`Vec`/`String` 的构造入口仍缺 → 见下，"
     "而 §10.11 的 `write_fmt`/`fmt_dynamic` 一个落点都没有 = **F65**）", 1, None, False),
    # ---------------- 三处把 F15 当"未完成"的旧声称 ----------------
    # coverage.md's per-clause table (§5) is a LIVE claim surface -- it still said `fmt!` has no lowering path.
    ("docs/coverage.md",
     "`std.fmt` 的 `infy_fmt_run`（但 `fmt!` 宏本身没有 lowering 路径 → F15），其余 std 调用一律显式拒绝。",
     "`std.fmt` 的 `infy_fmt_run`（**F15 轮起 `fmt!` / `fmt(…)` 有完整的 lowering 链** = 阶段 [2] 把宏拼法改写成 "
     "§2.5 的调用形态 → 阶段 [4] 编译期解析格式串（`E-0180`/`E-0181` 由此第一次有发射点）→ 阶段 [8] 发 "
     "`InfyFmtSpec` 表 + `infy_fmt_run` 调用，见 **P40** 与偏差 **D-58**；但 §10.11 的 `write_fmt` 与 "
     "`fmt_dynamic` 仍**一个落点都没有** = **F65**），其余 std 调用一律显式拒绝。", 1, None, False),
    # status.md's C2-round paragraph is a historical record but written in the present tense; F13/F14 there
    # already carry "已关掉" markers, so F14's bare one and F15's get one instead of rewriting history.
    ("docs/status.md",
     "`Q::Y(n)` 的绑定不进作用域（F14）；\n`fmt!` 在阶段 [2]/[4] 没有 lowering 路径，`sema` 见残留 `MacroCall` 就发 `E-0402`（F15）——",
     "`Q::Y(n)` 的绑定不进作用域（F14，**cron-39 轮已关掉** = 变体模式的 owner 改成除最后一段以外的全部段，"
     "裁定 = 偏差 **D-56**）；\n`fmt!` 在阶段 [2]/[4] 没有 lowering 路径，`sema` 见残留 `MacroCall` 就发 `E-0402`"
     "（F15，**F15 轮已接** = 阶段 [2] 改写成 `fmt(…)` → 阶段 [4] 编译期解析 → 阶段 [8] 发 `InfyFmtSpec` 表，"
     "见 `docs/coverage.md` 的 **P40** 与偏差 **D-58**）——", 1, "**F15 轮已接**", False),
    # D-46's "same family as F15/F19/F29" example is stale: F15 got its emission shape this round.
    ("docs/spec-deviations.md",
     "未实现的行**不占任何诊断码**，在阶段 [8] 以条款号 `§7.6.4` 指名 `CgError`（与 F15/F19/F29 那一族同形）。",
     "未实现的行**不占任何诊断码**，在阶段 [8] 以条款号 `§7.6.4` 指名 `CgError`（与 F19/F29/F65 那一族同形 —— "
     "原来举的 F15 那一例已在 F15 轮拿到发射形态，换成正同样没有落点的 `fmt_dynamic` = **F65**）。", 1, None, False),
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
    for i, rule in enumerate(RULES):
        rel, old, new, want = rule[0], rule[1], rule[2], rule[3]
        skip_marker = rule[4] if len(rule) > 4 else None
        tolerant = rule[5] if len(rule) > 5 else False
        if rel not in texts:
            with io.open(os.path.join(REPO, rel), encoding="utf-8", newline="") as fh:
                texts[rel] = fh.read()
        t = texts[rel]
        eol = "\r\n" if "\r\n" in t else "\n"
        old = old.replace("\n", eol)
        new = new.replace("\n", eol)
        if skip_marker and skip_marker.replace("\n", eol) in t:
            report.append("rule %d %-18s SKIP-MARKER (%s already present)" % (i, rel, skip_marker[:28]))
            continue
        hits = t.count(old)
        done = t.count(new)
        if hits == 0 and tolerant:
            report.append("rule %d %-18s NOOP (nothing to repair)" % (i, rel))
            continue
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
