#!/usr/bin/env bash
# F18 的命令级验收：同一份 `.inf` 容器交给两份工具，段表必须**逐行相同**。
#
# 为什么要脚本而不是只加 C 断言：`test_rt.c` 里那条手写期望块证明的是「C 侧渲染 == 我抄下来的
# Rust 侧数字」，它证明不了 Rust 侧此刻真的还打这些数字（改 `dump_sections` 不会让 rt 变红）。
# 这里两份工具都真跑：`infy artifact sections`（release 二进制）与 `art_dump`（链接生产读取器
# `infy_art_read_file` + 生产渲染器 `infy_art_dump_meta`），夹具两份——冻结的 Rust 写入器夹具
# 与 tour 真产物——各比一遍，并把 segments 块（含 12 行的形状守卫）逐字节 diff。
#
# 四条硬规矩（BACKLOG §0.0）：① 默认干跑，只有 `--apply` 才真跑；② 幂等（自己的 scratch 目录
# 每次重建，可重复运行）；③ 动作与结果追加进 logs/f18-dump-compare.log，任一探针失败退码非 0；
# ④ 不改本机状态 —— 写只在 target/scratch/f18/ 与 logs/ 里，runtime/ 与 examples/ 一个字节都不动。
#
# 用法：bash scripts/f18-dump-compare.sh            # 干跑，只打印将做什么
#       bash scripts/f18-dump-compare.sh --apply    # 真跑
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1
INFY="$REPO/target/release/infy.exe"
S="$REPO/target/scratch/f18"
LOG="$REPO/logs/f18-dump-compare.log"
# 传给 C 侧工具的容器路径一律**仓库相对**：这台机器的仓库根含中文，Git Bash 把 argv 按 ANSI 码页
# （CP936）交给原生 C 程序，`infy_fs_read_all` 再按 UTF-8 转 UTF-16 ⇒ 绝对路径里的「语言」两个字节
# 必然解不开（实测 CreateFileW 失败）。Rust 侧走 Unicode argv，不受影响。
FIX="runtime/fixtures/artifact_rust_writer.bin"
TOUR_EXE="$(ls examples/tour/.infy-build/*/ci.exe 2>/dev/null | head -1)"

mkdir -p "$(dirname "$LOG")"
log() { printf '%s  %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$1" >> "$LOG"; }

# 抽段表块：从 `segments (blob-relative)` 起，到下一个块标题（trailer regions / string pool）前止。
extract_block() {
  awk '/^segments \(blob-relative\)/{p=1} p&&/^(trailer regions|string pool)/{exit} p' "$1"
}

if [ "${1:-}" != "--apply" ]; then
  echo "DRY RUN — 将执行的步骤（加 --apply 才真跑）："
  echo "  1 rm -rf $S && mkdir -p $S"
  echo "  2 gcc -std=c11 -O1 -Wall -Wextra runtime/infy_rt.c runtime/art_dump_main.c -> $S/art_dump.exe（stderr 必须 0 字节）"
  echo "  3 gcc -std=c11 -O1 -Wall -Wextra runtime/infy_rt.c runtime/test_rt.c -> $S/rt_tests.exe，**跑在 target/scratch**（CWD 前提 3：夹具路径按 ../../runtime 解）"
  echo "  4   期望 rt.out 末行 '0 failures'，并把 rt_fixture_dump.txt（C 侧对夹具的渲染）收回 $S"
  echo "  5 $INFY artifact sections $FIX 与 $S/art_dump.exe $FIX 各抽 segments 块 => 期望 diff 0 差异、12 行"
  echo "  6 tour 真产物（$TOUR_EXE）同一条路再比一遍 => 期望 diff 0 差异、12 行"
  echo "  7 反例守卫：把 C 块里 meta 那行的 208 改成 0 再 diff => 期望 diff **非** 0（证明比对有牙齿）"
  echo "  8 读一个不存在的容器 => 期望退码 1 且 stderr 整行干净（%.*s 修好后不许再拖出缓冲区外的字节）"
  log "DRYRUN no writes"
  exit 0
fi

FAILS=0
note() { # note <PASS|FAIL> <名字> <细节>
  printf '  %s  %s — %s\n' "$1" "$2" "$3"
  log "$1 $2 $3"
  [ "$1" = PASS ] || FAILS=$((FAILS + 1))
}

rm -rf "$S"
mkdir -p "$S"

gcc -std=c11 -O1 -Wall -Wextra "$REPO/runtime/infy_rt.c" "$REPO/runtime/art_dump_main.c" \
    -o "$S/art_dump.exe" 2>"$S/gcc-art_dump.log"
if [ $? -ne 0 ] || [ -s "$S/gcc-art_dump.log" ]; then
  note FAIL compile-art_dump "退码 $?，stderr $(wc -c < "$S/gcc-art_dump.log") 字节（基线是 0 告警）"
  cat "$S/gcc-art_dump.log"
else
  note PASS compile-art_dump "0 告警"
fi

gcc -std=c11 -O1 -Wall -Wextra "$REPO/runtime/infy_rt.c" "$REPO/runtime/test_rt.c" \
    -o "$S/rt_tests.exe" 2>"$S/gcc-rt.log"
if [ $? -ne 0 ] || [ -s "$S/gcc-rt.log" ]; then
  note FAIL compile-rt "退码 $?，stderr $(wc -c < "$S/gcc-rt.log") 字节"
  cat "$S/gcc-rt.log"
else
  note PASS compile-rt "0 告警"
fi

# rt harness 的 CWD 前提（BACKLOG 前提 3）：它按 `../../runtime/fixtures/…` 找 Rust 写入器夹具，
# 只有从 target/scratch 跑才解得开，所以 exe 编到 $S、跑在 target/scratch，再把 dump 收回 $S。
( cd "$REPO/target/scratch" && "$S/rt_tests.exe" > "$S/rt.out" 2>&1 )
RT_RC=$?
cp -f "$REPO/target/scratch/rt_fixture_dump.txt" "$S/rt_fixture_dump.txt" 2>/dev/null
RT_LINE="$(grep -o '[0-9]* assertions, [0-9]* failures' "$S/rt.out" | tail -1)"
if [ "$RT_RC" -eq 0 ] && [ -n "$RT_LINE" ] && printf '%s' "$RT_LINE" | grep -q '0 failures' \
   && [ -s "$S/rt_fixture_dump.txt" ]; then
  note PASS rt-harness "$RT_LINE，rt_fixture_dump.txt $(wc -c < "$S/rt_fixture_dump.txt") 字节"
else
  note FAIL rt-harness "退码或断言不符（'$RT_LINE'），rt_fixture_dump.txt $(wc -c < "$S/rt_fixture_dump.txt" 2>/dev/null || echo 0) 字节"
  tail -3 "$S/rt.out" 2>/dev/null
fi

# 两份工具在同一夹具上的段表逐行比对
compare() { # compare <名字> <容器路径>
  local name="$1" file="$2" rc cdiff lines
  "$INFY" artifact sections "$file" > "$S/r-$name.txt" 2>&1
  if [ $? -ne 0 ]; then note FAIL "$name-rust" "infy artifact sections 退码非 0"; return; fi
  "$S/art_dump.exe" "$file" > "$S/c-$name.txt" 2>&1
  if [ $? -ne 0 ]; then note FAIL "$name-c" "art_dump 退码非 0"; return; fi
  extract_block "$S/r-$name.txt" > "$S/r-$name.block"
  extract_block "$S/c-$name.txt" > "$S/c-$name.block"
  lines=$(wc -l < "$S/c-$name.block")
  if [ "$lines" -ne 12 ]; then
    note FAIL "$name-shape" "C 块只有 $lines 行（期望 12：1 行标题 + 11 个区段）"
    return
  fi
  if ! grep -q '^meta               off=0          len=208$' "$S/c-$name.block"; then
    note FAIL "$name-meta" "C 块的 meta 行不是 off=0 len=208"
    return
  fi
  if diff -u "$S/r-$name.block" "$S/c-$name.block" > "$S/$name.diff"; then
    cdiff=0
  else
    cdiff=1
  fi
  if [ "$cdiff" -eq 0 ]; then
    note PASS "$name-block" "两份工具 12 行逐字节相同（$file）"
  else
    note FAIL "$name-block" "段表有差异，见 $S/$name.diff"
    head -20 "$S/$name.diff"
  fi
}

compare fixture "$FIX"

if [ -n "$TOUR_EXE" ]; then
  compare tour "$TOUR_EXE"
else
  note FAIL tour-missing "examples/tour 的真产物不在（先跑 infy build examples/tour），这一格没有比过"
fi

# 牙齿检查：手写地把 C 块的 meta 长度改回 bug 形态（len=0），比对必须变红。
# 只动 $S 里的副本，两份生产输出一个字节都不改。
if [ -f "$S/c-fixture.block" ]; then
  sed 's/^meta               off=0          len=208$/meta               off=0          len=0/' \
      "$S/c-fixture.block" > "$S/c-fixture.broken"
  if cmp -s "$S/c-fixture.broken" "$S/c-fixture.block"; then
    note FAIL teeth "反向样本与原块相同（说明 meta 行锚点没落上）"
  elif diff -q "$S/r-fixture.block" "$S/c-fixture.broken" >/dev/null; then
    note FAIL teeth "摘掉 len=208 之后比对仍然通过 ⇒ 这条判据是假的"
  else
    note PASS teeth "把 meta 改回 len=0 后 diff 变红（判据有牙齿）"
  fi
else
  note FAIL teeth "没有 C 块可改，反向检查没跑"
fi

# 报错装配那一行也要有牙齿：`infy_art_read_file` 早先把非终止的 InfyString 当 `%s` 打，
# 行尾会拖出缓冲区外的字节（实测 `open for read failedE…`）。改 `%.*s` 之后整行必须干净收尾。
"$S/art_dump.exe" no-such-artifact-file.bin >/dev/null 2>"$S/io-err.txt"
if [ $? -eq 1 ] && grep -qE "^artifact rejected: cannot read 'no-such-artifact-file\.bin': open for read failed$" "$S/io-err.txt"; then
  note PASS io-error-line "退码 1，stderr 整行 $(wc -c < "$S/io-err.txt") 字节、无缓冲区外字节"
else
  note FAIL io-error-line "退码或 stderr 形态不符：$(cat "$S/io-err.txt" 2>/dev/null | head -1)"
fi

if [ "$FAILS" -eq 0 ]; then
  log "RESULT 全部通过 FAILS=0"
  echo "RESULT: PASS（FAILS=0）"
  exit 0
fi
log "RESULT 有失败 FAILS=$FAILS"
echo "RESULT: FAIL（FAILS=$FAILS）"
exit 1
