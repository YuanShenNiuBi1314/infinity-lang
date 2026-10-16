#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Sync the two local-scope numbers F8 moved, across the outward-facing docs.

F8 changed runtime/infy_rt.c, which is statically linked into every artifact, so the local
end-to-end constants moved:
  * rt harness          32335 assertions  -> 32340   (+4 CK_EQ, +1 expect_panic)
  * tour artifact bytes 199975            -> 200644  (host image 199173 -> 199842)
  * build-id / C11 bytes / blob / trailer UNCHANGED (build-id inputs exclude the host image)

Why a script instead of 20 hand edits: the protocol (BACKLOG §0 rule 0) requires batch file
edits to be dry-run by default, idempotent, logged, and abortable. The validation is
all-or-nothing: every rule must match its expected occurrence count before anything is written,
so a half-updated document set cannot happen.

Idempotency: a rule whose `old` is absent is accepted if `new` is already present the expected
number of times (that is how it reports "already applied" on a second run).

Usage:
  python scripts/sync-doc-numbers.py            # dry run: report per-rule counts
  python scripts/sync-doc-numbers.py --apply    # write, then log
Rollback (logged too): git checkout -- docs/status.md docs/coverage.md README.md

Python rather than PowerShell because the anchors are Chinese literals: Windows PowerShell 5.1
mis-parses BOM-less UTF-8 sources, which would make every anchor miss.
"""
import io
import re
import sys
import datetime

REPO = r"F:\infinity语言\INFY_DELIVERY_v0.1.0\infinity-lang"
LOG = REPO + r"\logs\sync-doc-numbers.log"
ROLLBACK = "git checkout -- docs/status.md docs/coverage.md README.md"

# (file, kind, old, new, expected)
RULES = [
    # ---------------- docs/status.md : the live head block ----------------
    ("docs/status.md", "regex",
     r"(?m)^artifact ok — 199975 byte\(s\) 写到 examples/tour/.*$",
     "artifact ok — 200644 byte(s) 写到 examples/tour/.infy-build/x86_64-pc-windows-gnu/ci.exe（§7.4：宿主镜像 199842 + 段 blob 738 + 段尾 64，6 段）",
     1),
    ("docs/status.md", "regex",
     r"(?m)^  build-id d7c4408e84614ac24fe0475c31e1af24c9ab27c2790c605d9e371b4fac370015.*$",
     "  build-id d7c4408e84614ac24fe0475c31e1af24c9ab27c2790c605d9e371b4fac370015   # exit 0（C3 轮起阶段 [9] 已落地；**F6 轮换值** = D-17 把 tour 规范化 ⇒ dirhash 变；**F8 轮未变** —— build-id 的九类输入里没有宿主镜像，而产物字节恰恰因为 F8 改了 `infy_rt.c` 才 199975 → 200644）",
     1),
    ("docs/status.md", "regex",
     r"(?m)^artifact ok — 199975 byte\(s\) 写到 %TEMP%.*$",
     "artifact ok — 200644 byte(s) 写到 %TEMP%\\infy-run-<pid>-<n>\\ci.exe（…）       # 程序的 stdout/stderr 逐字节转发",
     1),
    ("docs/status.md", "regex",
     r"(?m)^verify-reproducible ok — 两个独立构建目录产物字节相同（199975 字节）.*$",
     "verify-reproducible ok — 两个独立构建目录产物字节相同（200644 字节）            # exit 0，§7.3 第 3 项",
     1),
    ("docs/status.md", "regex",
     r"(?m)^32335 assertions, 0 failures.*$",
     "32340 assertions, 0 failures                     # exit 0，0 error / 0 warning；链的是生产 infy_rt.c；C3 轮 +32（Rust 写 ↔ C 读的夹具锁 + D-33）；F8 轮 +5 = §11.1 host 调用跨 panic 的回滚（4 条 CK_EQ + 1 条 expect_panic）",
     1),
    ("docs/status.md", "substr",
     "（产物 199975 与段 blob 738 没动）。这三组数只随 tour 的字节动",
     "（产物 199975 与段 blob 738 没动）。这三组数只随 tour 的字节动。**F8 轮再加一句口径**：产物字节是**宿主镜像**的函数，`infy_rt.c` 一动它就动 ⇒ 本机现值 **200644**（宿主镜像 199842），而 build-id / C11 / 节点表行号三组没动（它们的输入里没有宿主镜像）",
     1),
    ("docs/status.md", "substr",
     "（C3 轮复跑 **32335 断言 / 0 失败**，直接链生产 `infy_rt.c`）",
     "（**当前口径 32340 断言 / 0 失败**，直接链生产 `infy_rt.c`；C3 轮那次是 32335，F8 轮 +5）",
     1),
    # ---------------- docs/coverage.md ----------------
    # P4 / P13 / P26 / P33 / §7.1 / §7.3 / the two matrix rows are claims about TODAY's build;
    # P30's step list and the "步 10 rt ..." sentence are records of a replay and of remote runs,
    # so those get an annotation instead of a rewrite (remote numbers are never pre-filled).
    ("docs/coverage.md", "substr", "199975 byte(s)", "200644 byte(s)", 1),
    ("docs/coverage.md", "substr", "宿主镜像 199173 +", "宿主镜像 199842 +", 1),
    ("docs/coverage.md", "substr",
     "verify-reproducible ok — 两个独立构建目录产物字节相同（199975 字节）",
     "verify-reproducible ok — 两个独立构建目录产物字节相同（200644 字节）", 1),
    ("docs/coverage.md", "substr", "`--verify-reproducible`（199975 字节）",
     "`--verify-reproducible`（**200644** 字节 = F8 轮起本机口径，重放它的那一轮记的是当时的 199975）", 1),
    ("docs/coverage.md", "substr", "12512/199975/", "12512/200644/", 1),
    ("docs/coverage.md", "substr", "产物仍 199975 字节 / blob 738",
     "产物仍 199975 字节 / blob 738（F6 轮口径；F8 轮改了 `infy_rt.c` ⇒ 本机现在是 200644 / blob 738 未变）", 1),
    ("docs/coverage.md", "substr", "两个独立构建目录 199975 字节逐字节相同",
     "两个独立构建目录 200644 字节逐字节相同", 1),
    ("docs/coverage.md", "substr", "真产出 199975 字节的 §7.4 容器",
     "真产出 200644 字节的 §7.4 容器", 1),
    ("docs/coverage.md", "substr", "rt **32335** 项", "rt **32340** 项", 1),
    ("docs/coverage.md", "substr", "P9：`32335 assertions, 0 failures`",
     "P9：`32340 assertions, 0 failures`", 1),
    ("docs/coverage.md", "substr", "→ rt **32335 assertions / 0 failures** →",
     "→ rt **32335 assertions / 0 failures**（F1 轮重放的实测；F8 轮起本机该项为 **32340**）→", 1),
    ("docs/coverage.md", "substr", "`32335 assertions, 0 failures`，0 error / 0 warning",
     "`32340 assertions, 0 failures`，0 error / 0 warning", 1),
    ("docs/coverage.md", "substr", "C3 轮 +32（基线 32303）",
     "C3 轮 +32（基线 32303），F8 轮 +5（§11.1 host 调用跨 panic 的回滚 + 成对断言）", 1),
    # ---------------- README.md ----------------
    ("README.md", "substr", "本机 MSYS2 `gcc 16.2.0` 产 199975 字节",
     "本机 MSYS2 `gcc 16.2.0` 产 200644 字节（F8 轮改 `infy_rt.c` 后；改前 199975）", 1),
    ("README.md", "substr", "阶段 [9] 产出 199975 字节容器",
     "阶段 [9] 产出 200644 字节容器", 1),
    ("README.md", "substr", "**32335 assertions / 0 failures**",
     "**32340 assertions / 0 failures**", 2),
]


def read(path):
    """Byte-faithful text: no newline translation in either direction.

    This matters. docs/status.md carries three *literal* CR bytes as content (a code sample that
    describes CRLF), and coverage.md/README.md are CRLF in the working tree. A text-mode read
    turns lone CR into LF and a newline="" write then re-splits those lines -- which is exactly
    what happened on the first run of this script. Byte I/O plus the count guard below is the fix.
    """
    with io.open(path, "rb") as f:
        return f.read().decode("utf-8")


def write_back(path, text):
    with io.open(path, "wb") as f:
        f.write(text.encode("utf-8"))


def eol_counts(text):
    return (text.count("\r"), text.count("\n"))


def count(text, kind, needle):
    if kind == "regex":
        return len(re.findall(needle, text))
    return text.count(needle)


def main():
    apply = "--apply" in sys.argv[1:]
    lines = []
    texts = {}
    failures = []
    for path in sorted({r[0] for r in RULES}):
        texts[path] = read(REPO + "\\" + path.replace("/", "\\"))

    for (path, kind, old, new, expected) in RULES:
        text = texts[path]
        hits = count(text, kind, old)
        already = count(text, kind, new) if kind == "substr" else 0
        if hits == expected:
            state = "MATCH"
        elif hits == 0 and already >= expected:
            state = "ALREADY-APPLIED"
        else:
            state = "MISMATCH"
            failures.append((path, old[:70], hits, expected, already))
        lines.append("%-16s %-14s hits=%d want=%d already=%d  %s"
                     % (path, state, hits, expected, already, (old[:60] + ("..." if len(old) > 60 else ""))))

    stamp = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    print(("APPLY" if apply else "DRY-RUN") + " — %d rules, rollback: %s" % (len(RULES), ROLLBACK))
    for line in lines:
        print("  " + line)

    if failures:
        print("REFUSED: %d rule(s) did not match the expected count; nothing was written." % len(failures))
        for (path, anchor, hits, want, already) in failures:
            print("  MISMATCH in %s: found %d want %d already %d for anchor %r" % (path, hits, want, already, anchor))
        with io.open(LOG, "a", encoding="utf-8") as f:
            f.write("%s  REFUSED rules=%d mismatches=%s\n" % (stamp, len(RULES), [x[1] for x in failures]))
        return 1

    if not apply:
        print("dry-run only: pass --apply to write")
        return 0

    # Stage every file in memory first: a guard that fires must leave all three untouched.
    staged = {}
    for path in sorted({r[0] for r in RULES}):
        text = texts[path]
        before = eol_counts(text)
        for (p, kind, old, new, expected) in RULES:
            if p != path:
                continue
            if kind == "regex":
                # lambda: a replacement string goes through re's escape parser, and the anchors
                # here contain backslashes (Windows paths like %TEMP%\infy-run-...), which raises
                # "bad escape" mid-write. Carrying it as a function skips that parsing entirely.
                text = re.sub(old, lambda _m, _new=new: _new, text)
            elif old in text:
                text = text.replace(old, new)
        after = eol_counts(text)
        if before != after:
            print("REFUSED %s: the rewrite moved CR/LF bytes (%s -> %s); nothing was written."
                  % (path, before, after))
            with io.open(LOG, "a", encoding="utf-8") as f:
                f.write("%s  REFUSED-EOL %s %s -> %s\n" % (stamp, path, before, after))
            return 1
        # post-condition, per rule kind:
        #   substr -> the old anchor must be gone, unless the rule deliberately keeps it inside its
        #            own annotation ("F6 轮口径；F8 起本机是 ...")
        #   regex  -> these rewrite whole live-status lines, so the new line must be present the
        #            expected number of times (the old pattern still matches it by construction)
        stale = []
        for r in RULES:
            if r[0] != path:
                continue
            p, kind, old, new, expected = r
            if kind == "regex":
                if text.count(new) < expected:
                    stale.append(new[:60])
            elif old not in new and count(text, kind, old) != 0:
                stale.append(old[:60])
        if stale:
            print("REFUSED %s: anchors still present after rewriting: %s" % (path, stale))
            with io.open(LOG, "a", encoding="utf-8") as f:
                f.write("%s  REFUSED-STALE %s %s\n" % (stamp, path, stale))
            return 1
        staged[path] = text

    for path, text in staged.items():
        write_back(REPO + "\\" + path.replace("/", "\\"), text)
        n = sum(1 for r in RULES if r[0] == path)
        print("wrote %s (%d rules, CR/LF=%s)" % (path, n, eol_counts(text)))

    with io.open(LOG, "a", encoding="utf-8") as f:
        f.write("%s  APPLY rules=%d files=3 rollback=%s\n" % (stamp, len(RULES), ROLLBACK))
        for line in lines:
            f.write("%s    %s\n" % (stamp, line))
        for path in sorted(staged):
            f.write("%s  EOL %s %s\n" % (stamp, path, eol_counts(staged[path])))
    print("POST-CHECK ok: all %d anchors gone, CR/LF counts unchanged in all three files" % len(RULES))
    return 0


if __name__ == "__main__":
    sys.exit(main())
