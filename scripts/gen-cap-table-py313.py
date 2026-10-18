#!/usr/bin/env python3
"""Rebuild the CPython 3.13 capability table from CPython 3.13's own data (BACKLOG F58).

Provenance rule this script exists to enforce: an opcode *number* may only come from that minor
itself. This host runs CPython 3.14.6, so 3.13's numbers cannot be observed here -- they are read
out of CPython's own released `Include/opcode_ids.h` instead. That file also contains the
specialization opcodes, which `dis.opmap` (the map the scanner actually matches against) does not,
so the selection rule matters:

    opmap = names defined in opcode_ids.h  -  members of any `family(...) = { ... }` block

That rule is not assumed: it is **calibrated** on 3.14, where a real interpreter is available. The
script derives 3.14's opmap from the `v3.14.6` tag with the same code path and compares it against
`dis.opmap` of the interpreter on this host; if that comparison is not exact, or no interpreter is
found, generation is refused (exit 4) rather than guessed.

Usage:
    python scripts/gen-cap-table-py313.py              # dry run: report only, writes nothing
    python scripts/gen-cap-table-py313.py --apply      # write the table + the Rust contract module
    python scripts/gen-cap-table-py313.py --offline    # use the cached upstream inputs only

Every run appends to logs/gen-cap-table-py313.log and exits non-zero on failure. Re-running with
--apply is idempotent: the same inputs produce byte-identical output.
"""
import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import urllib.request

BRANCH = "3.13"
CROSS_TAG = "v3.13.0"
CALIB_TAG = "v3.14.6"
UPSTREAM = [
    ("opids-%s.h" % BRANCH, "https://raw.githubusercontent.com/python/cpython/%s/Include/opcode_ids.h" % BRANCH),
    ("bytecodes-%s.c" % BRANCH, "https://raw.githubusercontent.com/python/cpython/%s/Python/bytecodes.c" % BRANCH),
    ("opids-%s.h" % CROSS_TAG, "https://raw.githubusercontent.com/python/cpython/%s/Include/opcode_ids.h" % CROSS_TAG),
    ("bytecodes-%s.c" % CROSS_TAG, "https://raw.githubusercontent.com/python/cpython/%s/Python/bytecodes.c" % CROSS_TAG),
    ("opids-%s.h" % CALIB_TAG, "https://raw.githubusercontent.com/python/cpython/%s/Include/opcode_ids.h" % CALIB_TAG),
    ("bytecodes-%s.c" % CALIB_TAG, "https://raw.githubusercontent.com/python/cpython/%s/Python/bytecodes.c" % CALIB_TAG),
]

HDR = re.compile(r"^#define\s+([A-Z][A-Z0-9_]*)\s+(\d+)\s*$")
NON_OP_MACROS = {"HAVE_ARGUMENT", "MIN_INSTRUMENTED_OPCODE", "MIN_SPECIALIZED_OPCODE",
                 "PSEUDO_OPS_BEGIN", "PSEUDO_OPS_END", "MAX_OPS_PER_CLASS"}
FAMILY = re.compile(r"^\s*family\(\s*[A-Z][A-Z0-9_]*\s*,[^)]*\)\s*=\s*\{(.*?)\}\s*;", re.S | re.M)
MEMBER = re.compile(r"[A-Z][A-Z0-9_]*")
ROW = re.compile(r'^([A-Z][A-Z0-9_]*) = "(.*)"(?:\s+#.*)?$')

# Rows the 3.14 table cannot supply because the instruction does not exist there. Each value is a
# statement about INFY's own lowering, so each carries the reason that goes into the generated
# header. "-"/"ignore" per appendix E.3's value grammar; nothing here claims a lowering that the
# IFBC ISA does not have.
PER_313 = {
    "BINARY_SUBSCR": ("idx.get",
                      "3.13 keeps x[i] as its own opcode; 3.14 folded [] into BINARY_OP, whose "
                      "'[]->idx.get' arm is the same target"),
    "RETURN_CONST": ("-",
                     "no fused push+ret mnemonic in appendix D; candidates in 'a|b' are alternatives, "
                     "not sequences, so emitting 'push' alone would drop the return (fake success)"),
    "BUILD_CONST_KEY_MAP": ("-",
                            "needs a 'key tuple + N values' fused dict build; IFBC only has new.dict "
                            "plus per-entry map.insert, so the row cannot name one instruction"),
    "LOAD_ASSERTION_ERROR": ("-",
                            "assert's raise path is RAISE_VARARGS, which is '-' in both tables; "
                            "mapping the push alone would claim a half-built construct"),
    "BEFORE_WITH": ("-",
                    "no IFBC counterpart for the with-entry protocol; SETUP_WITH is '-' in the "
                    "generated 3.14 table too"),
    "BEFORE_ASYNC_WITH": ("-",
                          "async is on appendix F.1's closed do-not-do list"),
    "LOAD_METHOD": ("-",
                    "bound-method dispatch needs the Python object model; the 3.14 table rejects "
                    "LOAD_ATTR's 'method' arm for the same reason"),
    "LOAD_SUPER_METHOD": ("-", "same object-model gap as LOAD_METHOD, plus super() lookup"),
    "LOAD_ZERO_SUPER_ATTR": ("-", "pseudo-op used only by super() lowering; not mapped"),
    "LOAD_ZERO_SUPER_METHOD": ("-", "pseudo-op used only by super() lowering; not mapped"),
    "INSTRUMENTED_RETURN_CONST": ("-",
                                  "instrumented forms appear only under tracing/profiling; every "
                                  "other INSTRUMENTED_* row is '-'"),
}
# BINARY_OP's argument domain differs: in 3.13 a subscript is BINARY_SUBSCR, so 3.13's BINARY_OP can
# never carry the '[]' case. Dropping the arm keeps the table from claiming an unreachable case.
BINARY_OP_313 = ("arg:+->add._|host:std.str.concat,-->sub._,*->mul._,/->div._|host:std.math.truediv,"
                 "&->and._,|->or._,^->xor._,<<->shl._,>>->shr._,//->-,%->-,**->-,@->-,else->-")


def log(lines):
    os.makedirs("logs", exist_ok=True)
    with open(os.path.join("logs", "gen-cap-table-py313.log"), "a", encoding="utf-8") as f:
        for l in lines:
            f.write(l + "\n")


def fetch(name, url, cache, offline):
    path = os.path.join(cache, name)
    if os.path.exists(path) and os.path.getsize(path) > 0:
        return open(path, encoding="utf-8", errors="strict").read()
    if offline:
        raise SystemExit("refused: %s is not cached and --offline was given" % name)
    with urllib.request.urlopen(url, timeout=60) as r:
        text = r.read().decode("utf-8")
    os.makedirs(cache, exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    return text


def parse_header(text):
    out = {}
    for line in text.splitlines():
        m = HDR.match(line)
        if m and m.group(1) not in NON_OP_MACROS:
            out[m.group(1)] = int(m.group(2))
    return out


def family_members(text):
    members = set()
    for m in FAMILY.finditer(text):
        members.update(MEMBER.findall(m.group(1)))
    return members


def derive_opmap(header_text, bytecodes_text):
    hdr = parse_header(header_text)
    spec = family_members(bytecodes_text)
    return {n: v for n, v in hdr.items() if n not in spec}, hdr, spec


def host_opmap():
    """dis.opmap of the interpreter installed on this build host, or None."""
    try:
        out = subprocess.run([sys.executable, "-c",
                              "import dis,json,sys; sys.stdout.write(json.dumps(dis.opmap))"],
                             capture_output=True, timeout=120)
    except (OSError, subprocess.SubprocessError):
        return None
    if out.returncode != 0:
        return None
    return json.loads(out.stdout.decode("utf-8"))


def inherit_rows(path):
    """The `[opcode]` section only -- `[builtin]` also carries uppercase keys (exception classes)."""
    text = open(path, encoding="utf-8").read()
    m = re.search(r"^\[opcode\][^\n]*\n(.*?)^\[", text, re.S | re.M)
    rows = {}
    for line in m.group(1).splitlines():
        r = ROW.match(line)
        if r:
            rows[r.group(1)] = r.group(2)
    return rows


def carried_section(path, header):
    """Copy a non-opcode section of the previous table verbatim (it is not opcode-derived).

    The section is annotated as *not* re-derived, so a reader at the section itself is not misled;
    the annotation is filtered back out on the next run, which keeps re-generation idempotent.
    """
    text = open(path, encoding="utf-8").read()
    m = re.search(r"^\[?%s\]?" % re.escape(header), text, re.M)
    start = m.start()
    rest = text[start:]
    nxt = re.search(r"^\[[a-z]+\]", rest[len(header) + 1:], re.M)
    section = (rest[:len(header) + 1 + nxt.start()] if nxt else rest).rstrip("\n")
    lines = [l for l in section.splitlines() if l.strip() not in NOTE_SECTION]
    return "\n".join(lines[:1] + NOTE_SECTION + lines[1:])


NOTE_SECTION = [
    "# NOT re-derived from 3.13: this list is the one the 3.14 interpreter on the build host gave, and",
    "# regenerating it is BACKLOG F59 -- opcode_contract() and the E-0713 version check cover [opcode] only.",
]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--apply", action="store_true", help="write the outputs (default: dry run)")
    ap.add_argument("--offline", action="store_true", help="never touch the network")
    ap.add_argument("--cache", default=os.path.join("target", "scratch", "upstream"))
    ap.add_argument("--table", default=os.path.join("capabilities", "IFBC-CAP-py-3.13.toml"))
    ap.add_argument("--from314", default=os.path.join("capabilities", "IFBC-CAP-py-3.14.toml"))
    ap.add_argument("--contract", default=os.path.join("src", "cap_opmap.rs"))
    args = ap.parse_args()

    report = []
    def say(line):
        report.append(line)
        print(line)

    inputs = {name: fetch(name, url, args.cache, args.offline) for name, url in UPSTREAM}
    digests = {name: hashlib.sha256(text.encode("utf-8")).hexdigest() for name, text in inputs.items()}

    # 1. calibrate the selection rule against a real interpreter of a version we can observe.
    calib, _, _ = derive_opmap(inputs["opids-%s.h" % CALIB_TAG], inputs["bytecodes-%s.c" % CALIB_TAG])
    real = host_opmap()
    if real is None:
        say("REFUSED: no interpreter on this host to calibrate the selection rule against")
        log(report)
        return 4
    if calib != real:
        miss = sorted(set(real) - set(calib))
        extra = sorted(set(calib) - set(real))
        say("REFUSED: rule does not reproduce dis.opmap of %s (missing %s, extra %s)"
            % (sys.version.split()[0], miss, extra))
        log(report)
        return 4
    say("calibration OK: family rule == dis.opmap of %s (%d opcodes, %s)"
        % (sys.version.split()[0], len(real), CALIB_TAG))

    # 2. derive 3.13 twice (branch head and the first release tag) and require them to agree.
    op313, hdr313, spec313 = derive_opmap(inputs["opids-%s.h" % BRANCH], inputs["bytecodes-%s.c" % BRANCH])
    alt, _, _ = derive_opmap(inputs["opids-%s.h" % CROSS_TAG], inputs["bytecodes-%s.c" % CROSS_TAG])
    if op313 != alt:
        say("REFUSED: %s and %s disagree, so 3.13 numbers are not stable: %s"
            % (BRANCH, CROSS_TAG, sorted(set(op313.items()) ^ set(alt.items()))))
        log(report)
        return 3
    say("derivation OK: %s == %s, %d opcodes in 3.13's opmap (header defines %d, family members %d)"
        % (BRANCH, CROSS_TAG, len(op313), len(hdr313), len(spec313)))

    # 3. build rows: inherited from the generated 3.14 table, overridden where 3.13 differs.
    base = inherit_rows(args.from314)
    rows, why = {}, {}
    for name in op313:
        if name in PER_313:
            rows[name], why[name] = PER_313[name][0], PER_313[name][1]
        elif name == "BINARY_OP":
            rows[name], why[name] = BINARY_OP_313, "3.13 has no '[]' case: subscript is BINARY_SUBSCR"
        elif name in base:
            rows[name] = base[name]
        else:
            say("REFUSED: %s has no mapping source" % name)
            log(report)
            return 5
    only313 = sorted(set(op313) - set(base))
    say("rows: %d total, %d 3.13-only names carry an authored mapping (%s)"
        % (len(rows), len(only313), ", ".join(only313)))

    body = []
    body.append("# appendix E.3 - INFY v2.0 B-track capability table (IFBC-CAP-<lang>-<minor>.toml).")
    body.append("# Normative data for the capability scan of 7.7.2; it REPLACED the old hand-written subset list.")
    body.append("#")
    body.append("# GENERATED by scripts/gen-cap-table-py313.py --apply. Do not edit by hand: the name set and")
    body.append("# every 0xNN value are CPython 3.13's own, read from")
    body.append("#   %s (sha256 %s)" % (UPSTREAM[0][1], digests["opids-%s.h" % BRANCH][:16]))
    body.append("#   %s (sha256 %s)" % (UPSTREAM[1][1], digests["bytecodes-%s.c" % BRANCH][:16]))
    body.append("# and cross-checked against tag %s, whose derivation is byte-identical to the %s branch."
                % (CROSS_TAG, BRANCH))
    body.append("# opcode_ids.h also lists the specialization opcodes, which dis.opmap does not; the rows here")
    body.append("# are header names MINUS every 'family(...) = { ... }' member -- and a name commented out")
    body.append("# inside such a block still counts as a member, which is how BINARY_OP_INPLACE_ADD_UNICODE")
    body.append("# (0x03) stays out; that reading reproduces dis.opmap exactly on the calibration version.")
    body.append("# The rule was not assumed: the")
    body.append("# generator re-derives %s from tag %s and refuses to write anything unless it equals the"
                % ("dis.opmap", CALIB_TAG))
    body.append("# dis.opmap of the interpreter actually installed on this host (%d opcodes). A 3.13 build" % len(real))
    body.append("# host can re-run the same script; a 3.13 interpreter is NOT required to fill the numbers in,")
    body.append("# because the numbers come from 3.13's own released header -- inventing them from a neighbour")
    body.append("# version is exactly the defect this table used to be (BACKLOG F58/F28).")
    body.append("#")
    body.append("# [opcode] rows appear under BOTH forms of the same key: the numeric form E.3 mandates")
    body.append("# (0xNN, the value CPython's dis.opmap reports for that minor) and the OPNAME alias.")
    body.append("# The scanner looks up the number first and the name second, and refuses a table whose two")
    body.append("# rows disagree (CapTable::check_alias_agreement) - 7.7.2 wants a table an auditor can trust.")
    body.append("# Every name here is additionally checked against CapTable::opcode_contract(\"py\", \"3.13\")")
    body.append("# at load time, and a table that does not match its declared minor is E-0713, not E-0712.")
    body.append("#")
    body.append("# Rows the 3.14 table could not supply (each is a declared gap, never a silent mapping):")
    for name in only313:
        body.append("#   %-28s = %-9s %s" % (name, '"%s"' % rows[name], why[name]))
    body.append("#   BINARY_OP arm '[]' is dropped: in 3.13 a subscript is BINARY_SUBSCR, so the case is")
    body.append("#     unreachable and keeping it would claim an arm the argument domain can never produce.")
    body.append("#")
    body.append("# Value grammar (mirrored in docs/internals/ifbc-encoding-rust.md):")
    body.append('#   "-"          unmapped on purpose  -> E-0712 naming this exact item')
    body.append('#   "ignore"     no IFBC counterpart; the instruction is dropped')
    body.append('#   "<mnemonic>" one-to-one mapping onto an appendix-D instruction')
    body.append('#   "<op>._"     the `_` is the type suffix the lowering inferred (add._ -> add.i64/add.f64)')
    body.append('#   "arg:k->v,..."  dispatch on the instruction argument; "else->v" is the default branch')
    body.append('#   "a|b"        candidate list, first applicable one wins (e.g. add._|host:std.str.concat)')
    body.append('#   "host:<path>" emits call.host and creates the import_table row (7.8.3 check item 5)')
    body.append("")
    body.append("[cap]")
    body.append('lang = "cpython"')
    body.append('format = "CPY-313"')
    body.append("cap_version = 1")
    hash_line_index = len(body)
    body.append("")  # placeholder for the table_hash row, filled in below

    body.append("")
    body.append("[opcode]   # scan item 1 (7.7.2)")
    for name in sorted(op313, key=lambda n: (op313[n], n)):
        body.append('0x%02x = "%s"   # %s' % (op313[name], rows[name], name))
    for name in sorted(op313):
        body.append('%s = "%s"' % (name, rows[name]))
    body.append("")
    body.append(carried_section(args.table, "[builtin]").rstrip("\n"))
    body.append("")
    body.append(carried_section(args.table, "[import]").rstrip("\n"))
    body.append("")
    body.append(carried_section(args.table, "[exception]").rstrip("\n"))
    body.append("")

    text = "\n".join(body)
    body[hash_line_index] = "table_hash = PLACEHOLDER"
    canon = "\n".join(l for l in "\n".join(body).splitlines() if not l.strip().startswith("table_hash"))
    digest = hashlib.sha256(canon.encode("utf-8")).hexdigest()
    body[hash_line_index] = ('table_hash = "sha256:%s"   # sha256 over every line that does not '
                             "start with table_hash" % digest)
    text = "\n".join(body)
    say("table_hash computed = sha256:%s" % digest[:16])

    contract = []
    contract.append("//! Generated by `scripts/gen-cap-table-py313.py --apply` -- do not edit by hand.")
    contract.append("//!")
    contract.append("//! Each shipped B-track minor's opcode name set and numbers, straight from CPython's own")
    contract.append("//! released `Include/opcode_ids.h` (`dis.opmap` = that header minus the `family(...)`")
    contract.append("//! members). This is the other half of §7.7.3's version lock: the capability table says")
    contract.append("//! what each instruction maps to, and this contract says which instructions a table is")
    contract.append("//! allowed to mention at all. A table renamed from a neighbouring minor fails against it")
    contract.append("//! with `E-0713` instead of quietly mis-scanning (BACKLOG F58).")
    contract.append("")
    contract.append("/// `opcode_contract(lang, minor)`: every opcode name that minor's `dis.opmap` contains,")
    contract.append("/// with its number. Keyed by the `btrack` language key (`py`), not by the table's")
    contract.append("/// `[cap] lang`, which spells the same thing as `cpython`.")
    contract.append('pub fn opcode_contract(lang: &str, minor: &str) -> Option<&\'static [(&\'static str, u32)]> {')
    contract.append("    Some(match (lang, minor) {")
    for ver, opmap in (("3.13", op313), ("3.14", calib)):
        contract.append("        // py %s: %d names, straight from CPython %s's opcode_ids.h"
                        % (ver, len(opmap), ver))
        contract.append('        ("py", "%s") => &[' % ver)
        for name in sorted(opmap, key=lambda n: (opmap[n], n)):
            contract.append('            ("%s", 0x%02x),' % (name, opmap[name]))
        contract.append("        ],")
    contract.append('        _ => return None,')
    contract.append("    })")
    contract.append("}")
    contract_text = "\n".join(contract) + "\n"

    say("Rust contract module: %d names (3.13) + %d names (3.14)" % (len(op313), len(calib)))
    if not args.apply:
        say("DRY RUN: nothing written (use --apply). Table would be %d lines, %d bytes."
            % (len(text.splitlines()), len(text.encode("utf-8"))))
        log(report)
        return 0
    with open(args.table, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    with open(args.contract, "w", encoding="utf-8", newline="\n") as f:
        f.write(contract_text)
    say("WROTE %s (%d lines) and %s" % (args.table, len(text.splitlines()), args.contract))
    log(report)
    return 0


if __name__ == "__main__":
    sys.exit(main())
