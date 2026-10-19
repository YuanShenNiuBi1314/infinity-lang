#!/usr/bin/env python3
"""F17 轮的文档数字同步（BACKLOG §0.0：批量改文件必须走脚本）。

默认干跑（只打印将改什么），`--apply` 才真写。四条规矩：
  ①  默认干跑；② 幂等（已改过的规则命中 0 次即视为已生效，不报错）；
  ③  动作与结果追加 logs/sync-doc-numbers-f17.log；任何规则命中数 != 期望 ⇒ 退出码非 0；
  ④  只动 docs/status.md、docs/coverage.md、README.md 三个文件，字节级读写（不碰行尾）。

规则 = (文件, 原文, 新文, 期望命中数)。期望命中数写成 0 表示"已应用过"是允许的（幂等）。
"""
import io
import os
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LOG = os.path.join(ROOT, "logs", "sync-doc-numbers-f17.log")

STATUS = "docs/status.md"
COVERAGE = "docs/coverage.md"
README = "README.md"

RULES = [
    # ---------------- docs/status.md : 测试合计 ----------------
    (STATUS, "= **613**\uFF08\u9010\u6a21\u5757\u76f8\u52a0\uff1a", "= **619**\uFF08\u9010\u6a21\u5757\u76f8\u52a0\uff1a", 1),
    (STATUS, "22+22+18+15+23+12+19+9", "22+22+**24**+15+23+12+19+9", 1),
    (STATUS, " 613 \u884c\u4e00\u81f4", " 619 \u884c\u4e00\u81f4", 1),
    (STATUS, "test result: ok. 613 passed; 0 failed", "test result: ok. 619 passed; 0 failed", 1),
    (STATUS, "# **F16 \u8f6e 606 \u2192 613**\uff08",
     "# **F17 \u8f6e 613 \u2192 619**\uff08`artifact` 18 \u2192 **24** = EnvOffsets \u7684\u5199\u56de\u8bfb\u3001\u533a\u6bb5\u63a8\u5bfc\u3001\u62d2\u7edd\u3001\u6e32\u67d3\u516d\u9501\uff0c\u89c1 P42\uff09\uff1b**\u4e0a\u4e00\u6863 F16 \u8f6e 606 \u2192 613**\uff08", 1),
    (STATUS, "613 \u9879\u6309\u6a21\u5757", "619 \u9879\u6309\u6a21\u5757", 1),
    (STATUS, "\u5408\u8ba1 613\uff1b**F16 \u8f6e\u5b9e\u6d4b**",
     "\u5408\u8ba1 619\uff1b**F17 \u8f6e\u5b9e\u6d4b** = \u4e0a\u4e00\u6863 613 + 6\uff1a`artifact` 18 \u2192 **24**\uff08EnvOffsets \u5199\u56de\u8bfb\u3001\u533a\u6bb5\u63a8\u5bfc\u3001\u62d2\u7edd\u3001\u6e32\u67d3\u516d\u9501 = P42\uff09\uff1b**\u4e0a\u4e00\u6863 F16 \u8f6e\u5b9e\u6d4b**", 1),
    # ---------------- docs/status.md : rt 断言 ----------------
    (STATUS, "32353 assertions, 0 failures", "32382 assertions, 0 failures", 1),
    (STATUS, "\u5f53\u524d\u53e3\u5f84 32353 \u65ad\u8a00 / 0 \u5931\u8d25",
     "\u5f53\u524d\u53e3\u5f84 32382 \u65ad\u8a00 / 0 \u5931\u8d25\uff08F17 \u8f6e +29\uff1aC \u4fa7\u8bfb EnvOffsets/ResourceKeys + \u884c\u6570\u9003\u9038\u62d2\u7edd\uff09", 1),
    # ---------------- docs/status.md : tour 产物字节 ----------------
    (STATUS, "artifact ok \u2014 200700 byte(s)", "artifact ok \u2014 204548 byte(s)", 2),
    (STATUS, "\u5bbf\u4e3b\u955c\u50cf 199898 + \u6bb5 blob 738", "\u5bbf\u4e3b\u955c\u50cf 203397 + \u6bb5 blob 1087", 1),
    (STATUS, "\u4ea7\u7269\u5b57\u8282\u76f8\u540c\uff08200700 \u5b57\u8282\uff09", "\u4ea7\u7269\u5b57\u8282\u76f8\u540c\uff08204548 \u5b57\u8282\uff09", 1),
    # ---------------- docs/status.md : 夹具 ----------------
    (STATUS, "artifact_rust_writer.bin`\uff08730 \u5b57\u8282\uff09",
     "artifact_rust_writer.bin`\uff081079 \u5b57\u8282\uff0cF17 \u8f6e\u91cd\u751f\u6210\uff1a+11 \u4e2a\u6c60\u540d\u3001+180 \u5b57\u8282 EnvOffsets \u533a\uff09", 1),
    # ---------------- docs/coverage.md ----------------
    (COVERAGE, "| `artifact` | 18 |", "| `artifact` | 24 |", 1),
    (COVERAGE, "`613 passed; 0 failed; 0 ignored`", "`619 passed; 0 failed; 0 ignored`", 1),
    (COVERAGE, "**F16 \u8f6e\u57fa\u7ebf 613 = F15 \u8f6e 606 + 7**",
     "**F17 \u8f6e\u57fa\u7ebf 619 = F16 \u8f6e 613 + 6**\uff1a`artifact` 18 \u2192 **24**\uff08\u89c1 P42\uff09\uff1b**\u4e0a\u4e00\u6863 F16 \u8f6e\u57fa\u7ebf 613 = F15 \u8f6e 606 + 7**", 1),
    (COVERAGE, "\u7684 **613** \u9879\u6309\u6a21\u5757\u5206\u5e03", "\u7684 **619** \u9879\u6309\u6a21\u5757\u5206\u5e03", 1),
    (COVERAGE, "**F16 \u8f6e\u5b9e\u6d4b\u5206\u7ec4\u8ba1\u6570** = \u4e0a\u4e00\u6863 606 + 7",
     "**F17 \u8f6e\u5b9e\u6d4b\u5206\u7ec4\u8ba1\u6570** = \u4e0a\u4e00\u6863 613 + 6\uff1a`artifact` 18 \u2192 **24**\uff1b**\u4e0a\u4e00\u6863 F16 \u8f6e\u5b9e\u6d4b\u5206\u7ec4\u8ba1\u6570** = \u4e0a\u4e00\u6863 606 + 7", 1),
    (COVERAGE, "artifact ok \u2014 200700 byte(s)", "artifact ok \u2014 204548 byte(s)", 1),
    (COVERAGE, "\u4ea7\u7269\u5b57\u8282\u76f8\u540c\uff08200700 \u5b57\u8282\uff09", "\u4ea7\u7269\u5b57\u8282\u76f8\u540c\uff08204548 \u5b57\u8282\uff09", 1),
    (COVERAGE, "**200700** \u5b57\u8282 = F12 \u8f6e\u8d77\u672c\u673a\u53e3\u5f84",
     "**204548** \u5b57\u8282 = F17 \u8f6e\u8d77\u672c\u673a\u53e3\u5f84\uff08F12 \u8f6e 200700\uff09", 1),
    (COVERAGE, "\u6bb5 blob 738", "\u6bb5 blob 1087", 2),
    # ---------------- README.md ----------------
    (README, "**613 passed / 0 failed**", "**619 passed / 0 failed**", 1),
    (README, "**F16 \u8f6e 606 \u2192 613**",
     "**F17 \u8f6e 613 \u2192 619**\uff1a`artifact` 18 \u2192 **24**\uff1b**\u4e0a\u4e00\u6863 F16 \u8f6e 606 \u2192 613**", 1),
    (README, "**32353 assertions / 0 failures**", "**32382 assertions / 0 failures**", 2),
    (README, "**F12 \u8f6e 32340 \u2192 32353**", "**F12 \u8f6e 32340 \u2192 32353\u3001F17 \u8f6e 32353 \u2192 32382**", 2),
    (README, "\u4ea7 200700 \u5b57\u8282\uff08F8 \u8f6e\u6539 `infy_rt.c` \u540e\uff1b\u6539\u524d 199975\uff09",
     "\u4ea7 204548 \u5b57\u8282\uff08F17 \u8f6e\u53c8\u6539 `infy_rt.c`\uff1a\u955c\u50cf +3499\u3001blob +349\uff1bF8 \u8f6e\u6539\u540e 200700\uff0c\u6539\u524d 199975\uff09", 1),
    (README, "\u6bb5 blob 738", "\u6bb5 blob 1087", 2),
    (README, "\u4ea7\u7269 **221823**\uff08\u5bbf\u4e3b\u955c\u50cf 221021 + blob 738 + \u6bb5\u5c3e 64\uff09",
     "\u4ea7\u7269 **221823**\uff08\u5bbf\u4e3b\u955c\u50cf 221021 + blob 738 + \u6bb5\u5c3e 64\uff1b\u8be5\u884c = F16 \u4ea4\u4ed8\u6811\u7684\u90a3\u6761 run\uff0cF17 \u8f6e\u672c\u673a blob \u5df2\u53d8 1087\uff0c\u4e0b\u4e00\u73ed\u56de\u67e5\u6362\u65b0\u6570\uff09", 1),
]

P42_ROW = (
    "| P42 | F17 \u7684 `EnvOffsets` \u9762\uff1a`cargo test --offline --lib -j 1`\uff08`artifact` \u516d\u6761\u65b0\u9501\uff09 + "
    "`gcc -std=c11 -O1 -Wall -Wextra runtime/infy_rt.c runtime/test_rt.c -o target/scratch/rt_tests.exe`\uff0c"
    "\u518d\u5728 `target/scratch` \u91cc `./rt_tests.exe`\uff0c\u6700\u540e `infy build examples/tour` + "
    "`infy artifact info <\u4ea7\u7269>` | `artifact` 18 \u2192 **24**\u3001\u5408\u8ba1 **619 passed / 0 failed**\uff1b"
    "rt **32382 assertions / 0 failures**\uff08+29\uff09\uff1b\u4e24\u6761 IFBC \u5404 **180/180** + `cmp` \u65e0\u5dee\u5f02\uff1b"
    "\u4ea7\u7269 **204548** \u5b57\u8282\uff08\u5bbf\u4e3b\u955c\u50cf 203397 + blob 1087 + \u6bb5\u5c3e 64\uff09\uff0c"
    "`artifact info` \u6253\u51fa **11 \u884c** `env.*` \u504f\u79fb | 0 |\n"
)


def log(msg):
    d = os.path.dirname(LOG)
    if not os.path.isdir(d):
        os.makedirs(d)
    with io.open(LOG, "a", encoding="utf-8", newline="\n") as f:
        f.write("%s %s\n" % (time.strftime("%Y-%m-%d %H:%M:%S"), msg))


def main():
    apply = "--apply" in sys.argv
    files = {}
    problems = []
    for path, old, new, want in RULES:
        if path not in files:
            with io.open(os.path.join(ROOT, path), "rb") as f:
                files[path] = f.read().decode("utf-8")
        text = files[path]
        n = text.count(old)
        if n == want or (want == 0 and n == 0) or (n == 0 and new in text):
            status = "hit=%d" % n
            if n > 0:
                files[path] = text.replace(old, new)
                status += " -> rewrite"
            else:
                status += " -> already applied / no-op"
            log("%-18s %s [%s]" % ("APPLY" if apply else "DRY", status, path))
            if not apply and n > 0:
                log("   old: %s" % old[:70])
                log("   new: %s" % new[:70])
        else:
            problems.append("%s expects %d hits, found %d for %r" % (path, want, n, old[:60]))
            log("FAIL %s" % problems[-1])

    # P42 行：coverage 的 P 表按倒序排，P41 之前插 P42
    if COVERAGE not in files:
        with io.open(os.path.join(ROOT, COVERAGE), "rb") as f:
            files[COVERAGE] = f.read().decode("utf-8")
    if "| P42 |" in files[COVERAGE]:
        log("%-18s P42 row already present" % ("DRY" if not apply else "APPLY"))
    else:
        anchor = "| P41 |"
        if anchor not in files[COVERAGE]:
            problems.append("coverage.md has no `| P41 |` anchor for the P42 row")
        else:
            if apply:
                files[COVERAGE] = files[COVERAGE].replace(anchor, P42_ROW + anchor, 1)
            log("%-18s insert P42 before P41" % ("APPLY" if apply else "DRY"))

    if problems:
        log("ABORT %d rule(s) did not match; nothing written" % len(problems))
        for p in problems:
            print("PROBLEM: " + p)
        return 1
    if not apply:
        log("DRYRUN ok: %d rules + P42 row would apply; no file touched" % len(RULES))
        print("dry run: %d rules matched, pass --apply to write" % len(RULES))
        return 0
    for path, text in files.items():
        full = os.path.join(ROOT, path)
        with io.open(full, "rb") as f:
            before = f.read()
        after = text.encode("utf-8")
        if before == after:
            log("SKIP %s unchanged" % path)
            continue
        tmp = full + ".tmp-f17"
        with io.open(tmp, "wb") as f:
            f.write(after)
        os.replace(tmp, full)
        crlf_before = before.count(b"\r\n")
        crlf_after = after.count(b"\r\n")
        log("WROTE %s %d -> %d bytes; CRLF %d -> %d" % (path, len(before), len(after), crlf_before, crlf_after))
        if crlf_before != crlf_after:
            log("FAIL line endings changed in %s" % path)
            return 1
    log("APPLY ok: %d file(s)" % len(files))
    print("applied %d rules to %d files" % (len(RULES), len(files)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
