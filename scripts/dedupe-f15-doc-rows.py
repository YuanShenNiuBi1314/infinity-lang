#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Collapse the duplicate markdown rows `sync-doc-numbers-f15.py` wrote on 2026-10-19.

What happened: its two *insert* rules used the following row as the anchor
(`| P6 | …` and `` | `pkg` | 24 | ``). That anchor is still present after the insertion, so every
extra `--apply` run inserted the row once more -- `docs/coverage.md` ended up with three `P40` rows
and three `fmtstr` rows. `check-doc-numbers.py` could not see it: it keys the per-module table by
module name, so duplicated rows collapse into one entry and the table still "agreed".

Semantics: for each pattern, keep the FIRST matching line and delete the later ones. Idempotent --
a second run reports "0 removed". Default is a dry run; only `--apply` writes.
Actions and results are appended to logs/dedupe-f15-doc-rows.log; failures exit non-zero.

Usage:
  python scripts/dedupe-f15-doc-rows.py            # dry run
  python scripts/dedupe-f15-doc-rows.py --apply    # write
Rollback (if ever needed): git checkout -- docs/coverage.md
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
LOG = os.path.join(REPO, "logs", "dedupe-f15-doc-rows.log")
TARGETS = [
    ("docs/coverage.md", "| P40 | "),
    ("docs/coverage.md", "| `fmtstr` | "),
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
    report = []
    texts = {}
    removed_total = 0
    for rel, prefix in TARGETS:
        if rel not in texts:
            with io.open(os.path.join(REPO, rel), encoding="utf-8", newline="") as fh:
                texts[rel] = fh.read()
        t = texts[rel]
        eol = "\r\n" if "\r\n" in t else "\n"
        lines = t.split(eol)
        first = None
        keep = []
        removed = 0
        for ln in lines:
            if ln.startswith(prefix):
                if first is None:
                    first = ln
                    keep.append(ln)
                    continue
                if ln != first:
                    report.append("WARN %s: duplicate %r rows are not byte-identical" % (rel, prefix))
                removed += 1
                continue
            keep.append(ln)
        removed_total += removed
        report.append("%-20s prefix=%-16s copies=%d removed=%d" % (rel, prefix, removed + 1, removed))
        if removed and apply:
            texts[rel] = eol.join(keep)
    for ln in report:
        print(ln)
    if apply and removed_total:
        for rel in sorted(texts):
            with io.open(os.path.join(REPO, rel), "w", encoding="utf-8", newline="\n") as fh:
                fh.write(texts[rel])
        print("APPLIED: removed %d duplicate row(s)" % removed_total)
        log(report + ["APPLIED removed=%d" % removed_total])
    elif removed_total:
        print("DRY RUN: %d duplicate row(s) would be removed (add --apply)" % removed_total)
        log(report + ["DRY RUN removed=%d" % removed_total])
    else:
        print("CLEAN: no duplicate rows; nothing written")
        log(report + ["CLEAN nothing written"])
    return 0


if __name__ == "__main__":
    sys.exit(main())
