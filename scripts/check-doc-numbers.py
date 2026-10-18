#!/usr/bin/env python3
"""Cross-check the per-module test counts that the docs claim, against the test binary itself.

Why this exists: the same number is written twice in docs/coverage.md (a prose paragraph AND a
22-row table) and twice in docs/status.md, and two separate rounds (F11, F13) shipped a prose update
that left a table row behind — docs/coverage.md's table summed to 561 while its own prose claimed 570.
Counting by hand is the failure mode; this script makes it mechanical.

Read-only: it runs `cargo test --offline --lib -j 1 -- --list`, sums the per-module groups, sums both
docs' tables, and appends PASS/FAIL to logs/check-doc-numbers.log. Exit 0 = everything agrees, 1 = drift.
"""
import io
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LOG = os.path.join(ROOT, "logs", "check-doc-numbers.log")
COVERAGE = os.path.join(ROOT, "docs", "coverage.md")
STATUS = os.path.join(ROOT, "docs", "status.md")


def log(msg):
    d = os.path.dirname(LOG)
    if not os.path.isdir(d):
        os.makedirs(d)
    with io.open(LOG, "a", encoding="utf-8", newline="\n") as f:
        f.write(msg + "\n")


def measured():
    """Per-module counts from the test binary (`-- --list` is the only trustworthy grouping)."""
    p = subprocess.run(
        ["cargo", "test", "--offline", "--lib", "-j", "1", "--", "--list"],
        cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, shell=False)
    if p.returncode != 0:
        return None, None
    groups = {}
    for line in p.stdout.decode("utf-8", "replace").splitlines():
        m = re.match(r"^([a-z0-9_]+)::tests::.*: test$", line.strip())
        if m:
            groups[m.group(1)] = groups.get(m.group(1), 0) + 1
    return groups, sum(groups.values())


def coverage_table():
    rows = {}
    for line in io.open(COVERAGE, encoding="utf-8"):
        m = re.match(r"^\| `([a-z0-9_]+)` \| \**(\d+)\** \|", line)
        if m:
            rows[m.group(1)] = int(m.group(2))
    return rows


def status_line():
    """The 'sum the modules' line in docs/status.md: `a+b+c+...` plus the claimed total."""
    for line in io.open(STATUS, encoding="utf-8"):
        m = re.search("= \\*\\*(\\d+)\\*\\*（逐模块相加：([0-9+→*]+)，", line)
        if m:
            claimed = int(m.group(1))
            expr = m.group(2).replace("**", "").replace("→", "+")
            parts = [int(x) for x in expr.split("+") if x.strip().isdigit()]
            return claimed, sum(parts), len(parts)
    return None, None, 0


def main():
    groups, total = measured()
    if groups is None:
        log("FAIL cargo test -- --list did not run")
        return 1
    tab = coverage_table()
    claimed, summed, parts = status_line()
    problems = []
    if not tab:
        problems.append("coverage.md §2 table not parsed")
    else:
        for mod, n in sorted(tab.items()):
            if groups.get(mod) != n:
                problems.append("coverage table `%s`=%d but binary has %d" % (mod, n, groups.get(mod, 0)))
        if sum(tab.values()) != total:
            problems.append("coverage table sums to %d but the binary lists %d" % (sum(tab.values()), total))
    if claimed is None:
        problems.append("status.md per-module sum line not found")
    else:
        if claimed != total:
            problems.append("status.md claims %d but the binary lists %d" % (claimed, total))
        if summed != claimed:
            problems.append("status.md's own addition (%s terms) sums to %d, not %d" % (parts, summed, claimed))
    log("%s total=%d modules=%d table_rows=%d status_terms=%d%s"
        % ("PASS" if not problems else "FAIL", total, len(groups), len(tab), parts or 0,
           "" if not problems else " | " + " ; ".join(problems)))
    return 0 if not problems else 1


sys.exit(main())
