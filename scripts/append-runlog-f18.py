#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""向 docs/RUNLOG.md 追加 cron-43 那一行（幂等：已有同轮次行就只核对不重复追加）。

为什么走脚本而不是 shell here-doc/`>>`：这一行里有大量反引号与中文，Git Bash 的双引号会把
`` `…` `` 当命令替换真的去执行（踩过），here-doc 又受控制台码页影响。字节级 UTF-8 写盘 + 行尾
沿用文件本身那一行的形状（CRLF/LF 混用时照抄表头那行）。
"""
import pathlib, sys

REPO = pathlib.Path(__file__).resolve().parent.parent
RUNLOG = REPO / "docs" / "RUNLOG.md"
ROUND = "| cron-43 |"

LINE = (
    "| cron-43 | 2026-10-19 09:49 抢锁（`prod-loop-43`，TTL 45min，09:58 同名 RENEW 一次，"
    "`-Action status` 复见 `job=prod-loop-43`；开轮那次的 STEAL 行记的是 prod-loop-42 已超 TTL） "
    "| **F18 勾 `[x]`，新登记 F71 未修**。开工现算欠账：`gh api …/git/refs/heads/toolchain` = "
    "`af8860df34c0…` == 本地 HEAD ⇒ **欠账 0**（F17 的交付树已在远端）。交付 = **两份工具第一次在"
    "同一夹具上逐行可比**：C 侧 `infy_art_dump_meta` 的 `meta` 行不再写死 `len=0`（改打 "
    "`INFY_META_FIXED_BYTES`=208）、块标题改成与 Rust 同串的 `segments (blob-relative)`、并补上此前"
    "只有 Rust 侧有的 `resource_keys`/`env_offsets`/`rodata` 三行 —— `rodata` 要求 `InfyMetaFixed` "
    "新增 `off_rodata/len_rodata` 并在 `parse_meta_record` 按 `INFY_META_OFF_RODATA=120` 解析（那枚"
    "常量 2026-10-18 就躺在 `infy_rt_internal.h:341`，只是没人读它）。顺带修上游报错装配："
    "`infy_art_read_file` 把非终止的 `InfyString` 当 `%s` 打，实测行尾拖出缓冲区外的字节"
    "（`open for read failedE…`）⇒ 改 `%.*s` 并补 `infy_string_drop`。新增 `runtime/art_dump_main.c`"
    "（给任意 `.inf` 打 C 侧渲染，只由脚本编译、不进 CI 编译行）与 `scripts/f18-dump-compare.sh`"
    "（默认干跑；`--apply` 在冻结夹具 + tour 真产物两份容器上各抽段表块逐字节 `diff`，外加两条"
    "牙齿检查）。**F71 是读代码时撞出来的**：Rust 侧 `src/artifact.rs:869-882` 对 8 个 meta 区段做"
    "`off+len > blob_len` 拒绝（注释还写着“infy_art_read_file performs”），C 侧 `infy_rt.c:3125-3130` "
    "只列了 6 个 —— 漏 `events` 与 `rodata` ⇒ 同一份坏字节 Rust 拒、C 收，本轮没修、已登记。 "
    "| `cargo check --offline --lib -j 1`；`cargo test --offline --lib -j 1`；"
    "`gcc -std=c11 -O1 -Wall -Wextra runtime/infy_rt.c runtime/test_rt.c -o target/scratch/f18/rt_tests.exe` "
    "后在 `target/scratch` 跑；`bash scripts/f18-dump-compare.sh` ⇒ `--apply`；"
    "`infy build examples/tour` 六连；`gh run view 39181242747 --log`；"
    "`python scripts/sync-doc-numbers-f18.py` ⇒ `--apply`；`python scripts/check-doc-numbers.py` | "
    "基线 **619 passed / 0 failed**（Rust 零改动）⇒ 收尾 **619 / 0**、`cargo check` **0 error / 告警数 0**、"
    "rt **32382 → 32387 assertions / 0 failures**（+5 = `test_rt.c` 那段**手写** 12 行期望块，数字抄自"
    "另一侧工具不是被测代码算的）、`-c` 与 rt 链各 stderr **0 字节**、tour `fmt --check/check/build/run/"
    "test/build --verify-reproducible` 六连 exit **0**、产物 **204548 → 205060**（宿主镜像 203397 → "
    "203909、blob 1087 未动 = `infy_rt.c` 机器码 +512）、`f18-dump-compare --apply` **exit 0 / 7 条全 "
    "PASS**（两份容器各 **12 行逐字节相同**；反向样本把 `len=208` 改回 `len=0` ⇒ `diff` 变红；不存在的"
    "容器 ⇒ exit 1 + stderr 整行 82 字节干净）、**远端回查 run #28（`39181242747`，push `af8860d`）= "
    "success**：步 7 **619/0**、步 8 告警 **0**、步 10 rt **32382**、步 11 两条 IFBC 各 **180/180**、"
    "步 12 tour 产物 **226689**（宿主镜像 225538 + blob 1087 + 段尾 64）⇒ README「最新一条远端实录」由 "
    "run #13 换成 #28、`check-doc-numbers.py` **exit 0**（`PASS total=619 modules=23 table_rows=23 "
    "status_terms=23`）。换数全走 `scripts/sync-doc-numbers-f18.py`（12 条规则逐条报命中、重跑报 ALREADY、"
    "字节级 + CRLF 计数守卫，README 192→192、status.md 0→0）。 "
    "| 下一轮 = ① 现算欠账补推 + 回查覆盖本轮树的那条 run（步 10 应打 **32387**、步 12 的 runner 产物数"
    "要换新，runner 侧别用本机 205060）；② §1 第一个 `[ ]` 仍是 **F17**（②格被 F70/F40 挡）⇒ 卡住顺延 "
    "**F19**；**F71** 按 §1 顺序排在阶段 F 末尾，别插队；③ 上一轮欠的变异探针（`Bad::EnvTableEscapesBlob` "
    "/ `infy_art_env_check` / `infy_lock_acquire` 声明校验）仍欠；④ 卫生欠账 F68/F53/F63 不变；"
    "⑤ Git Bash 按 CP936 交 argv，仓库根含中文 ⇒ C 侧工具别收绝对路径（已进经验库）。 |\n"
)


def main():
    data = RUNLOG.read_bytes()
    if ROUND.encode() in data:
        # 追加过一次：只把多写出来的那个空行收掉（HEAD 那份 90 行的表以 `|\n` 结尾，没有尾空行）。
        if data.endswith(b"\n\n"):
            RUNLOG.write_bytes(data[:-1])
            print("FIXED %s 已存在，收掉尾部多出的 1 个空行" % ROUND)
        else:
            print("SKIP 已有 %s 行，未重复追加" % ROUND)
        return 0
    # 行尾照抄表头之后那一行（文件里混用时保持与相邻数据行一致）
    lines = data.split(b"\n")
    tail = lines[-2] if len(lines) >= 2 else b""
    eol = b"\r\n" if tail.endswith(b"\r") else b"\n"
    body = LINE.encode("utf-8").rstrip(b"\n")
    if eol == b"\r\n":
        body = body.replace(b"\n", b"\r\n")
    if not data.endswith(b"\n"):
        data += eol
    RUNLOG.write_bytes(data + body + eol)
    print("WROTE 追加 1 行（%d 字节，eol=%s）" % (len(body), "CRLF" if eol == b"\r\n" else "LF"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
