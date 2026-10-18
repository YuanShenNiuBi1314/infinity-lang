#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Sync the numbers and the claims F16 moved, across the outward-facing docs.

F16 landed stage [10]'s comparison half: `artifact::Artifact::sums()` reads the 12-byte triples back
out of the container, `cli::dirhash_shape` gates §5.8 item 1's `h1:<base64-of-32-bytes>` form, and
`cli::sum_verify_text` + `infy artifact verify` compare `.inf sum` against `MODULE.infy.lock`'s
`dirh` records in **both** directions. The test count moved and nothing else did:

  * cargo test --offline --lib -j 1   606 -> 613   (`cli` 48 -> 55, all seven in cli::tests)

UNCHANGED and deliberately not touched: rt harness 32353, IFBC 180/180, tour artifact 200700 bytes,
C11 12512 bytes, node table 7-8-9-10-16-20, conformance 83 = 77/0/6. `runtime/*.c` was not edited and
`examples/tour` gained no lock file, so the emitted C and the container bytes are identical. Historical
rows keep the figure of the round that measured them — which is why this is a rule table instead of a
sweep over "606".

Claim lines that used to say "阶段 [10] 未做 / 没人拿它去比对" are rewritten, not deleted: what F16
closed is the lock comparison; the sumdb half (G21/G27) and the runtime self-check stay open, and
`E-0561`/`E-0562` still have zero emission sites.

Rule semantics (each entry: file, old, new, expected count, then two optional flags):
  * `skip_marker` — if this substring is present, the rule is a no-op. Insert-type rules need it:
    their anchor survives the insertion, so re-running without a marker would insert the row twice.
  * `tolerant` — `hits == 0` is accepted as a no-op instead of a mismatch.
Validation is all-or-nothing: any non-tolerant mismatch writes nothing at all.
`scripts/check-doc-numbers.py` re-verifies afterwards.

Line endings: docs/coverage.md and README.md are CRLF in this worktree, docs/status.md is LF with a
few stray CRs — so every anchor below is a **single-line** substring and the writer keeps the file's
own bytes untouched outside the replacement.

Usage:
  python scripts/sync-doc-numbers-f16.py            # dry run: per-rule counts, writes nothing
  python scripts/sync-doc-numbers-f16.py --apply    # write, then append the audit log
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
LOG = os.path.join(REPO, "logs", "sync-doc-numbers-f16.log")
ROLLBACK = "git checkout -- docs/status.md docs/coverage.md README.md"

F16_DELTA = "`cli` 48 → **55**"
F16_WHY = ("= 阶段 [10] 的 `.inf sum` ↔ `MODULE.infy.lock` 比对有了入口（`infy artifact verify`：双向逐条、"
           "不符 = `E-0560` + exit 7、空集明打「0 条依赖，无可校验」、形态不合法走不带码的硬阻断、"
           "无 lock 不放行；裁定 = 偏差 **D-59(a)–(g)**，实测见 **P41**）")

P40_ANCHOR = "| P40 | `cargo test --offline --lib -j 1`"

P41_ROW = (
    "| P41 | `cargo test --offline --lib -j 1` + `cargo check --offline --lib -j 1` + "
    "`cargo build --offline --release -j 1` + `bash scripts/f16-artifact-verify-demo.sh --apply` 的 "
    "17 条命令级探针 + `python scripts/f16-mutation-probe.py --apply` 七处变异 + 四条 C 链复跑 + "
    "tour 七连 + `conformance --report` | **F16 = 阶段 [10] 的 `.inf sum` ↔ `MODULE.infy.lock` 比对"
    "有入口了**（裁定 = 偏差 **D-59(a)–(g)**。BACKLOG 的 F16 验收行写着「用已有的 §5.8 族码 "
    "`E-0580`/`E-0583`」，这两个码在 `spec/infinity_lang_spec.md` 的 §9.7 与 `src/codes.rs` 里"
    "**都不存在**（05xx 到 `E-0570` 为止）⇒ 落的是规范自己定义的 `E-0560`，并登记 **F68** 追这笔假账）。"
    "实现面 = `artifact::Artifact::sums()`（读侧解码器：一行 = `package_pool, version_pool, "
    "dirhash_pool` 三个 u32 = 12 字节）+ `cli::dirhash_shape`（§5.8 第 1 项形态门禁）+ "
    "`cli::sum_verify_text`（双向逐条）+ `artifact_cmd` 的 `verify` 分支。**实测**："
    "`cargo test --offline --lib -j 1` **606 → 613 passed / 0 failed**（+7 全在 `cli` 48→**55**）、"
    "`cargo check --offline --lib -j 1` **0 error / 0 warning**、`cargo build --offline --release -j 1` "
    "exit **0**。命令级 17 条探针**全 PASS**：`examples/tour` 的 scratch 副本 + 一份两条 `dirh` 的 "
    "`MODULE.infy.lock` ⇒ `infy build` exit 0（产物 200863 字节）⇒ `infy artifact verify` **exit 0** 且打"
    "「逐条一致」；不给 lock 参数时从产物路径往上找到同一份 lock，仍 **exit 0**；把 lock 里的 hash 换成"
    "另一串 ⇒ **exit 7 + `error[E-0560]`**；删掉 lock 的 `dirh` 行 ⇒ **exit 7 +「比 lock 多」**；"
    "真 tour 产物（`examples/tour` 从来没有 lock）⇒ **exit 1** 并明说不放行；"
    "`runtime/fixtures/artifact_rust_writer.bin` 里那行 `h1:AAAA` ⇒ **exit 7 +「读不了」且不带 E-0560**"
    "（G28 要求的两条分离，脚本里是一条独立断言）。七处变异**各自实测能红**（摘掉反向比对 / 把 hash "
    "比对短路 / 摘掉 `h1:` 门禁 / 摘掉区长度整除守卫 / 把空集写成假成功 / 无 lock 时返回 0 / 把第三格"
    "读成第二格），跑完按字节 + sha256 复验复原。C 侧零改动仍复跑：两份 `-c` 各 stderr **0 字节**、"
    "rt **32353/0**、两条 IFBC **180/180** + `cmp` 无差异；tour 七连全 exit **0** 且产物 **200700** 字节"
    "一字未动；`infy conformance --report` 仍 **83 = 77/0/6 ⇒ exit 1**（92.8%）。**剩余面**（不许读成"
    "「§5.8 校验已通」）：**sumdb 那一面一次都没比对** —— 没有 sumdb 客户端、没有 §5.8 第 3 项的 "
    "inclusion-proof 验证器（G21），`INFY_NOSUMDB`/`[net] nosumdb` 没有消费者（G27），所以 `E-0561`/"
    "`E-0562` 至今**零发射点**；**运行时自校验**也没接（`infy_rt.c` 把 `off_sum`/`len_sum` 读进 "
    "`InfyMetaFixed` 之后没人碰）；`dirh-m` 行不在比对范围内（阶段 [9] 只抄 `dirh`） | 0 |"
)

SUM_ROW_10 = (
    "| §7.1 阶段 [10] 签名/校验段写入与 lock 比对 | `.inf sum` 由 `lock.dirh` 生成"
    "（`cli.rs::stage9`）+ `artifact::Artifact::sums()` 读回 + `cli.rs::sum_verify_text`"
    "（`infy artifact verify`） | 🟡 | F16 轮把「写进去」与「拿回来比」接上了：`infy artifact verify "
    "<产物> [<MODULE.infy.lock>]` 双向逐条比 `.inf sum` 与 lock 的 `dirh` —— 一致 exit 0、不符 "
    "`E-0560` + exit 7、空集明打「0 条依赖，无可校验」而不放行、形态不合法的 dirhash 走**不带码**的"
    "硬阻断（G28 的两条分离，登记 **F68**），裁定 = 偏差 **D-59**。记 🟡 的理由：§5.8 第 3 项的 "
    "**sumdb 记录/inclusion proof** 仍为零（无客户端、无 proof 验证器 = G21；`INFY_NOSUMDB`/"
    "`[net] nosumdb` 无消费者 = G27；`E-0561`/`E-0562` 至今零发射点），**运行时自校验**也没接"
    "（`infy_rt.c` 读进 `InfyMetaFixed` 之后没人碰），`dirh-m` 不在比对范围（阶段 [9] 只抄 `dirh`）；"
    "tour 无依赖 ⇒ 现有一切实例上这条路径是空集，证据来自 `target/scratch/f16/` 那份带 lock 的副本"
    "（见 **P41**） |"
)

# (file, old, new, want, skip_marker, tolerant)
RULES = [
    # ---------------- docs/coverage.md ----------------
    ("docs/coverage.md", "| `cli` | **48** | \u00a713.4 \u5b50\u547d\u4ee4\u4e0e\u9000\u51fa\u7801\uff08",
     "| `cli` | **55** | \u00a713.4 子命令与退出码（**F16 轮 +7** = `.inf sum` \u2194 `MODULE.infy.lock` "
     "\u7684\u9010\u6761\u6bd4\u5bf9\uff08`infy artifact verify`\uff0c\u88c1\u5b9a = \u504f\u5dee **D-59**\uff0c\u5b9e\u6d4b\u89c1 **P41**\uff09\u00b7 ",
     1, None, False),
    ("docs/coverage.md",
     "`606 passed; 0 failed; 0 ignored`\uff08**F15 \u8f6e\u57fa\u7ebf 606 = F14 \u8f6e 583 + 23**\uff1a",
     "`613 passed; 0 failed; 0 ignored`\uff08**F16 \u8f6e\u57fa\u7ebf 613 = F15 \u8f6e 606 + 7**\uff1a" + F16_DELTA + " " + F16_WHY
     + "\uff1b**\u4e0a\u4e00\u6863 F15 \u8f6e\u57fa\u7ebf 606 = F14 \u8f6e 583 + 23**\uff1a", 1, None, False),
    ("docs/coverage.md",
     "\u7684 **606** \u9879\u6309\u6a21\u5757\u5206\u5e03\uff08**F15 \u8f6e\u5b9e\u6d4b\u5206\u7ec4\u8ba1\u6570** = \u4e0a\u4e00\u6863 583 + 23\uff1a",
     "\u7684 **613** \u9879\u6309\u6a21\u5757\u5206\u5e03\uff08**F16 \u8f6e\u5b9e\u6d4b\u5206\u7ec4\u8ba1\u6570** = \u4e0a\u4e00\u6863 606 + 7\uff1a" + F16_DELTA
     + "\uff0c\u4e03\u6761\u5168\u662f\u9636\u6bb5 [10] \u7684\u6bd4\u5bf9\u9501\u70b9\uff08\u89c1 **P41** \u4e0e\u504f\u5dee **D-59**\uff09\uff1b**\u4e0a\u4e00\u6863 F15 \u8f6e\u5b9e\u6d4b\u5206\u7ec4\u8ba1\u6570** = \u4e0a\u4e00\u6863 583 + 23\uff1a",
     1, None, False),
    ("docs/coverage.md", P40_ANCHOR, P41_ROW + "\n" + P40_ANCHOR, 1, "| P41 |", False),
    ("docs/coverage.md", "LINE:| \u00a77.1 \u9636\u6bb5 [10] \u7b7e\u540d/\u6821\u9a8c\u6bb5\u5199\u5165\u4e0e lock \u6bd4\u5bf9 |", SUM_ROW_10, 1, None, False),
    # ---------------- docs/status.md ----------------
    ("docs/status.md",
     "test result: ok. 606 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out   # **F15 \u8f6e 583 \u2192 606**\uff08\u65b0\u6a21\u5757",
     "test result: ok. 613 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out   # **F16 \u8f6e 606 \u2192 613**\uff08"
     + F16_DELTA + " = \u9636\u6bb5 [10] \u7684 `.inf sum` \u2194 lock \u6bd4\u5bf9\uff0c\u88c1\u5b9a = \u504f\u5dee **D-59**\uff0c\u5b9e\u6d4b\u89c1 "
     "`docs/coverage.md` \u7684 **P41**\uff09\uff1b**\u4e0a\u4e00\u6863 F15 \u8f6e 583 \u2192 606**\uff08\u65b0\u6a21\u5757", 1, None, False),
    ("docs/status.md",
     "606 \u9879\u6309\u6a21\u5757\uff08`cargo test --offline --lib -j 1 -- --list` \u9010\u6761\u5206\u7ec4\u8ba1\u6570\uff0c\u5408\u8ba1 606\uff1b**F15 \u8f6e\u5b9e\u6d4b** = \u4e0a\u4e00\u6863 583 + 23\uff1a",
     "613 \u9879\u6309\u6a21\u5757\uff08`cargo test --offline --lib -j 1 -- --list` \u9010\u6761\u5206\u7ec4\u8ba1\u6570\uff0c\u5408\u8ba1 613\uff1b**F16 \u8f6e\u5b9e\u6d4b** = "
     "\u4e0a\u4e00\u6863 606 + 7\uff1a" + F16_DELTA + "\uff1b**\u4e0a\u4e00\u6863 F15 \u8f6e\u5b9e\u6d4b** = \u4e0a\u4e00\u6863 583 + 23\uff1a", 1, None, False),
    ("docs/status.md",
     "= **606**\uff08\u9010\u6a21\u5757\u76f8\u52a0\uff1a79+58+41+**48**+",
     "= **613**\uff08\u9010\u6a21\u5757\u76f8\u52a0\uff1a79+58+41+**55**+", 1, None, False),
    ("docs/status.md",
     "\u7684 606 \u884c\u4e00\u81f4",
     "\u7684 613 \u884c\u4e00\u81f4", 1, None, False),
    ("docs/status.md",
     "\u9636\u6bb5 [10]\uff08sumdb \u6821\u9a8c\u4e0e\u8fd0\u884c\u65f6\u81ea\u6821\u9a8c\u6bd4\u5bf9\uff09\u4ecd**\u672a\u505a**\uff0c`.inf sum` \u53ea\u662f\u628a `MODULE.infy.lock` \u7684 dirhash \u6e05\u5355\u6284\u8fdb\u5bb9\u5668\uff1b",
     "\u9636\u6bb5 [10] \u7684 **lock \u6bd4\u5bf9\u81ea F16 \u8f6e\u8d77\u53ef\u7528**\uff08`infy artifact verify`\uff1a`.inf sum` \u2194 `MODULE.infy.lock` \u7684 `dirh` "
     "\u53cc\u5411\u9010\u6761\uff0c\u4e0d\u7b26 = `E-0560` + exit 7\uff0c\u88c1\u5b9a = \u504f\u5dee **D-59**\uff0c\u5b9e\u6d4b\u89c1 `docs/coverage.md` \u7684 **P41**\uff09"
     "\uff1b\u4ecd\u672a\u505a\u7684\u662f **sumdb \u8bb0\u5f55/inclusion proof**\uff08G21\uff09\u4e0e**\u8fd0\u884c\u65f6\u81ea\u6821\u9a8c**\uff08`infy_rt.c` \u628a `off_sum`/`len_sum` "
     "\u8bfb\u8fdb `InfyMetaFixed` \u4e4b\u540e\u6ca1\u4eba\u78b0\uff09\uff1b", 1, None, False),
    ("docs/status.md",
     "\u9636\u6bb5 [10] \u7684 sumdb \u6821\u9a8c\u6bb5\u4e0e\u8fd0\u884c\u65f6\u81ea\u6821\u9a8c\u6bd4\u5bf9\uff08`.inf sum` \u53ea\u662f `lock.dirh` \u7684\u6284\u5199\uff0c",
     "\u9636\u6bb5 [10] \u7684 **sumdb \u6821\u9a8c\u6bb5\u4e0e\u8fd0\u884c\u65f6\u81ea\u6821\u9a8c\u6bd4\u5bf9**\uff08`.inf sum` \u2194 `lock.dirh` \u7684\u9010\u6761\u6bd4\u5bf9\u7531 F16 \u8f6e\u7684 "
     "`infy artifact verify` \u627f\u62c5\u4e86\uff0c\u5269\u4e0b\u7684\u624d\u662f\u8fd9\u91cc\u8bf4\u7684\u672a\u505a\uff1b\u6bd4\u5bf9\u8303\u56f4\u4e5f\u53ea\u5230 `dirh`\uff0c", 1, None, False),
    # ---------------- README.md ----------------
    ("README.md",
     "**606 passed / 0 failed**\uff08**F15 \u8f6e 583 \u2192 606**\uff08\u65b0\u6a21\u5757",
     "**613 passed / 0 failed**\uff08**F16 \u8f6e 606 \u2192 613**\uff08" + F16_DELTA + " " + F16_WHY
     + "\uff1b**\u4e0a\u4e00\u6863 F15 \u8f6e 583 \u2192 606**\uff08\u65b0\u6a21\u5757", 1, None, False),
    ("README.md",
     "**仍 🟡**：阶段 [10] 的 sumdb 校验/运行时自校验未做（`.inf sum` 只是 `lock.dirh` 的抄写，tour 无依赖 = 空集）、",
     "**仍 🟡**：阶段 [10] 到 F16 轮为止只落了 `.inf sum` ↔ `MODULE.infy.lock` 的逐条比对"
     "（`infy artifact verify`，不符 = `E-0560` + exit 7，裁定 = 偏差 **D-59**，实测见 "
     "`docs/coverage.md` 的 **P41**），sumdb 记录/inclusion proof（G21）与运行时自校验仍未做"
     "（现有一切实例上 tour 无依赖 = 空集）、", 1, None, False),
    ("README.md",
     "`infy build` \u7684\u9636\u6bb5 [10]\uff08\u4ea7\u7269\u7b7e\u540d/sumdb \u6821\u9a8c\u6bb5\u4e0e\u8fd0\u884c\u65f6\u81ea\u6821\u9a8c\u6bd4\u5bf9\uff0c\u00a75.8\uff09**\u672a\u505a** \u2014\u2014 \u5bb9\u5668\u91cc\u5199\u4e86\u4f9d\u8d56 dirhash \u6e05\u5355\uff0c\u4f46\u6ca1\u4eba\u62ff\u5b83\u53bb\u6bd4\u5bf9\uff1b",
     "`infy build` \u7684\u9636\u6bb5 [10]\uff1a\u5bb9\u5668\u91cc\u5199\u7684\u4f9d\u8d56 dirhash \u6e05\u5355\u4ece **F16 \u8f6e**\u8d77**\u6709\u4eba\u62ff\u5b83\u6bd4\u4e86**"
     "\uff08`infy artifact verify` \u2194 `MODULE.infy.lock` \u7684 `dirh`\uff0c\u4e0d\u7b26 = `E-0560` + exit 7\uff0c\u88c1\u5b9a = \u504f\u5dee "
     "**D-59**\uff09\uff1b**\u4ecd\u672a\u505a**\u7684\u662f sumdb \u8bb0\u5f55/inclusion proof\uff08G21\uff09\u4e0e\u8fd0\u884c\u65f6\u81ea\u6821\u9a8c\uff08`infy_rt.c` \u8bfb\u5230 "
     "`off_sum`/`len_sum` \u4e4b\u540e\u6ca1\u4eba\u78b0\uff09\uff1b", 1, None, False),
]


def log(lines):
    d = os.path.dirname(LOG)
    if not os.path.isdir(d):
        os.makedirs(d)
    stamp = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S")
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
        if old.startswith("LINE:"):
            # 整行改写：表行的其余部分带着已经过期的声称，逐段替换会留下两截尾巴，不如连行一起换。
            key = old[len("LINE:"):]
            lines = t.split(eol)
            idxs = [n for n, ln in enumerate(lines) if key in ln]
            if len(idxs) != want:
                report.append("rule %d %-18s MISMATCH line-hits=%d want=%d key=%s" % (i, rel, len(idxs), want, key[:48]))
                bad += 1
                continue
            report.append("rule %d %-18s ok (whole line %d)" % (i, rel, idxs[0] + 1))
            lines[idxs[0]] = new
            texts[rel] = eol.join(lines)
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
