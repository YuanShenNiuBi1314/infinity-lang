#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""F16 的变异探针：证明本轮新增的 7 条断言真的在盯行为，而不是把当前输出抄一遍。

七处变异各打掉 `sum_verify_text` / `artifact_cmd verify` / `Artifact::sums()` 的一个判据，
每处都要求**指定的那条测试变红**（不只是"总数变少"），跑完立刻按字节复原并复验。

四条硬规矩（BACKLOG §0.0）：
  ① 默认干跑 —— 只打印将要执行的七处变异，不写文件、不跑 cargo；`--apply` 才真做。
  ② 幂等 —— 每次运行都以"当前字节"为原样重新读取，结束前必须复原；复原失败 = 退码 1 并打出
     那份文件的字节数与 sha256，绝不带着变异体离开。
  ③ 动作与结果追加进 logs/f16-mutation-probe.log。
  ④ 改的只是工作树里的 src/{cli,artifact}.rs，且是**临时**的（finally 复原 + 事后核验），
     不动 git、不动 target/ 以外的任何东西。

用法：
  python scripts/f16-mutation-probe.py            # 干跑
  python scripts/f16-mutation-probe.py --apply    # 真跑
"""
import hashlib
import io
import os
import re
import subprocess
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LOG = os.path.join(REPO, "logs", "f16-mutation-probe.log")

# (名字, 相对路径, old 字节, new 字节, 期望变红的测试名)
MUTATIONS = [
    ("M1 去掉「lock 有而产物没有」那一半", "src/cli.rs",
     b"        if !rows.iter().any(|r| r.package == *m && r.version == *v) {",
     b"        if false && !rows.iter().any(|r| r.package == *m && r.version == *v) {",
     "cli::tests::verify_compares_both_directions_between_the_sum_rows_and_the_lock"),
    ("M2 把 dirhash 比对短路成永远一致", "src/cli.rs",
     b"            Some(h) if h != r.dirhash => fails.push(format!(",
     b"            Some(h) if false && h != r.dirhash => fails.push(format!(",
     "cli::tests::verify_reports_E_0560_when_the_lock_and_the_container_disagree_on_a_dirhash"),
    ("M3 把 `h1:` 前缀门禁摘掉", "src/cli.rs",
     b'h.strip_prefix("h1:")',
     b'h.strip_prefix("")',
     "cli::tests::an_undecidable_dirhash_is_refused_without_being_called_E_0560"),
    ("M4 去掉 sum 区的整除守卫", "src/cli.rs",
     b"    if region_len as usize % REC != 0 {",
     b"    if false && region_len as usize % REC != 0 {",
     "cli::tests::a_ragged_sum_region_is_refused_as_container_damage"),
    ("M5 把空集那一格写成假成功", "src/cli.rs",
     b"**0 \xe6\x9d\xa1\xe4\xbe\x9d\xe8\xb5\x96\xef\xbc\x8c\xe6\x97\xa0\xe5\x8f\xaf\xe6\xa0\xa1\xe9\xaa\x8c**",
     b"**\xe6\xa0\xa1\xe9\xaa\x8c\xe9\x80\x9a\xe8\xbf\x87**",
     "cli::tests::an_empty_sum_against_an_empty_lock_says_out_loud_that_nothing_was_verified"),
    ("M6 没有 lock 时返回 0（假通过）", "src/cli.rs",
     "                     第三个位置参数传进来即可。\\n\"\n                );\n                return exit::GENERIC;".encode("utf-8"),
     "                     第三个位置参数传进来即可。\\n\"\n                );\n                return exit::OK;".encode("utf-8"),
     "cli::tests::verify_refuses_when_there_is_no_lock_to_compare_against"),
    ("M7 `.inf sum` 的第三格读成第二格", "src/artifact.rs",
     b"                dirhash: self.pool_string(rd_u32(&c[8..12])),",
     b"                dirhash: self.pool_string(rd_u32(&c[4..8])),",
     "cli::tests::the_dirh_rows_a_lock_renders_are_the_rows_verify_reads_out_of_the_container"),
]

RESULT_RE = re.compile(r"(\d+) passed; (\d+) failed")


def log(lines):
    if not os.path.isdir(os.path.dirname(LOG)):
        os.makedirs(os.path.dirname(LOG))
    import datetime
    stamp = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    with io.open(LOG, "a", encoding="utf-8", newline="\n") as fh:
        for ln in lines:
            fh.write("%s  %s\n" % (stamp, ln))


def run_tests():
    """返回 (failed 数, 变红的测试名集合, 尾部输出)。"""
    p = subprocess.run(
        ["cargo", "test", "--offline", "--lib", "-j", "1"],
        cwd=REPO, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, shell=False)
    out = p.stdout.decode("utf-8", "replace")
    m = RESULT_RE.search(out)
    if not m:
        return None, set(), out
    failed = set()
    block = out.split("failures:\n")[-1] if "failures:" in out else ""
    for line in block.splitlines():
        s = line.strip()
        if s.startswith("cli::tests::") or s.startswith("artifact::tests::"):
            failed.add(s)
    return int(m.group(2)), failed, out[-1200:]


def main():
    apply = "--apply" in sys.argv
    report = []
    if not apply:
        for name, rel, old, new, want in MUTATIONS:
            with open(os.path.join(REPO, rel), "rb") as fh:
                hits = fh.read().count(old)
            report.append("DRY %-40s %-18s hits=%d want_red=%s" % (name, rel, hits, want.split("::")[-1][:40]))
        for ln in report:
            print(ln)
        log(report + ["DRY RUN -- nothing written"])
        print("DRY RUN: 加 --apply 才真跑")
        return 0

    originals = {}
    bad = 0
    try:
        for rel in sorted({m[1] for m in MUTATIONS}):
            with open(os.path.join(REPO, rel), "rb") as fh:
                originals[rel] = fh.read()
        for name, rel, old, new, want in MUTATIONS:
            src = originals[rel]
            hits = src.count(old)
            if hits != 1:
                report.append("SKIP %s -- 锚点命中 %d 次，不敢动" % (name, hits))
                bad += 1
                continue
            with open(os.path.join(REPO, rel), "wb") as fh:
                fh.write(src.replace(old, new, 1))
            failed, names, tail = run_tests()
            with open(os.path.join(REPO, rel), "wb") as fh:
                fh.write(src)
            with open(os.path.join(REPO, rel), "rb") as fh:
                back = fh.read()
            if back != src:
                report.append("RESTORE-FAIL %s -- 字节不一致，立刻停" % name)
                bad += 1
                break
            if failed is None:
                report.append("BUILD-FAIL %s -- 变异体没编出来，看不到红" % name)
                report.append("  tail: %s" % tail.replace("\n", " | ")[:400])
                bad += 1
                continue
            hit = want in names
            if not hit or failed == 0:
                report.append("DEAD %s -- 期望 %s 红，实得 failed=%d，红名=%s" % (name, want.split("::")[-1], failed, sorted(n.split("::")[-1] for n in names)))
                bad += 1
            else:
                report.append("KILLED %s -- failed=%d（含目标测试），其余红名=%d" % (name, failed, len(names) - 1))
    finally:
        for rel, src in originals.items():
            with open(os.path.join(REPO, rel), "rb") as fh:
                now = fh.read()
            if now != src:
                with open(os.path.join(REPO, rel), "wb") as fh:
                    fh.write(src)
                report.append("FINALLY-RESTORED %s" % rel)
        for rel, src in originals.items():
            with open(os.path.join(REPO, rel), "rb") as fh:
                now = fh.read()
            report.append("VERIFY %s sha256=%s bytes=%d eol_ok=%s" % (
                rel, hashlib.sha256(now).hexdigest()[:16], len(now),
                str(now.count(b"\r\n") == 0)))
    for ln in report:
        print(ln)
    log(report)
    if bad:
        print("FAIL: %d 处变异没被杀掉或复原异常" % bad)
        return 1
    print("PASS: 七处变异各自实测能红，且两份文件按字节复原")
    return 0


if __name__ == "__main__":
    sys.exit(main())
