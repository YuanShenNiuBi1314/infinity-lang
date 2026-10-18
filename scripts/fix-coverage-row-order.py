#!/usr/bin/env python3
"""Move the `| P38 |` evidence row of docs/coverage.md to sit right after `| P37 |`.

The table's convention is "new rows appended after the previous P3x", while P1 stays the first row;
this round inserted P38 at the top by mistake. Byte-level I/O + line-count guards, dry-run by default.
"""
import io
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TARGET = os.path.join(ROOT, "docs", "coverage.md")
LOG = os.path.join(ROOT, "logs", "fix-coverage-row-order.log")
APPLY = "--apply" in sys.argv


def log(msg):
    d = os.path.dirname(LOG)
    if not os.path.isdir(d):
        os.makedirs(d)
    with io.open(LOG, "a", encoding="utf-8", newline="\n") as f:
        f.write(msg + "\n")


def main():
    with open(TARGET, "rb") as f:
        raw = f.read()
    lines = raw.split(b"\n")
    cr = sum(1 for l in lines if l.endswith(b"\r"))
    idx38 = [i for i, l in enumerate(lines) if l.startswith(b"| P38 |")]
    idx37 = [i for i, l in enumerate(lines) if l.startswith(b"| P37 |")]
    if len(idx38) != 1 or len(idx37) != 1:
        log("REFUSED P38=%d P37=%d (expected exactly one of each)" % (len(idx38), len(idx37)))
        return 1
    i38, i37 = idx38[0], idx37[0]
    if i38 == i37 + 1:
        log("ALREADY ordered (P37 line %d -> P38 line %d); lines=%d CR=%d" % (i37 + 1, i38 + 1, len(lines), cr))
        return 0
    if i38 > i37:
        log("REFUSED P38 (line %d) is already after P37 (line %d) but not adjacent" % (i38 + 1, i37 + 1))
        return 1
    row = lines.pop(i38)
    i37 = [i for i, l in enumerate(lines) if l.startswith(b"| P37 |")][0]
    lines.insert(i37 + 1, row)
    out = b"\n".join(lines)
    after = out.split(b"\n")
    cr_after = sum(1 for l in after if l.endswith(b"\r"))
    if len(after) != len(lines) or cr_after != cr:
        log("REFUSED shape changed lines=%d->%d CR=%d->%d" % (len(lines), len(after), cr, cr_after))
        return 1
    log("%s moved P38 from line %d to line %d; lines=%d CR=%d bytes=%d->%d"
        % ("APPLY" if APPLY else "DRY-RUN", i38 + 1, i37 + 2, len(lines), cr, len(raw), len(out)))
    if APPLY:
        with open(TARGET, "wb") as f:
            f.write(out)
        log("APPLY wrote %s" % TARGET)
    return 0


sys.exit(main())
