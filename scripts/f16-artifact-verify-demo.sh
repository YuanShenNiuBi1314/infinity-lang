#!/usr/bin/env bash
# F16（阶段 [10]）的命令级验收：`infy artifact verify` 拿真产物里的 `.inf sum` 去比真 lock。
#
# 为什么要有这份脚本而不是只在 Rust 测试里造容器：测试用的 `artifact::write` 夹具证明的是
# 「12 字节三元组写得对、读得回」，它证明不了阶段 [9] 真会去读 `MODULE.infy.lock`（那一格此前
# 是写-only 死路）。这里走完整链路：真 module 副本 + 手写 lock → `infy build` → 真产物 →
# `infy artifact verify` → 退出码。
#
# 四条硬规矩（BACKLOG §0.0）：① 默认干跑，只有 `--apply` 才真动手；② 幂等（每次都重建自己的
# scratch 目录，可重复运行）；③ 动作与结果追加进 logs/f16-artifact-verify-demo.log，任一探针失败
# 退码非 0；④ 不改本机状态 —— 写只在 target/scratch/f16/ 里，examples/ 一个字节都不动。
#
# 用法：bash scripts/f16-artifact-verify-demo.sh            # 干跑，只打印
#       bash scripts/f16-artifact-verify-demo.sh --apply    # 真跑
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INFY="$REPO/target/release/infy.exe"
S="$REPO/target/scratch/f16"
LOG="$REPO/logs/f16-artifact-verify-demo.log"
FIX="$REPO/runtime/fixtures/artifact_rust_writer.bin"
TOUR_EXE_GLOB="examples/tour/.infy-build/*/ci.exe"
# §5.8 第 1 项的合法形态：`h1:` + 32 字节（SHA-256 Merkle 根）的标准 base64。这里是 32 个 0x11。
H_OK="h1:ERERERERERERERERERERERERERERERERERERERERERE="
# 同一长度、不同内容 —— 用来制造「lock 与产物不一致」那一格。
H_BAD="h1:RERERERERERERERERERERERERERERERERERERERERE="

mkdir -p "$(dirname "$LOG")"
log() { printf '%s  %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$1" >> "$LOG"; }

if [ "${1:-}" != "--apply" ]; then
  echo "DRY RUN — 将执行的步骤（加 --apply 才真跑）："
  echo "  1 rm -rf $S && cp -r examples/tour -> $S/mod（删掉副本里的 .infy-build）"
  echo "  2 写 $S/mod/MODULE.infy.lock：两条 dirh（$H_OK 与另一条包）"
  echo "  3 $INFY build $S/mod"
  echo "  4 artifact verify <产物> <同一份 lock>            => 期望 0 + 「逐条一致」"
  echo "  5 artifact verify <产物>（不给 lock 参数）        => 期望 0（从产物路径向上找到 lock）"
  echo "  6 artifact verify <产物> <把 hash 换成 $H_BAD>     => 期望 7 + error[E-0560]"
  echo "  7 artifact verify <产物> <删掉 dirh 行的 lock>     => 期望 7 + 「比 lock 多」"
  echo "  8 artifact verify <真 tour 产物>                   => 期望 1（examples/tour 没有 lock，明说不放行）"
  echo "  9 artifact verify $FIX <含 h1:AAAA 的 lock>        => 期望 7 + 「读不了」且**不带** E-0560（G28 两条分离）"
  echo "  10 tour 七连（fmt --check|check|build|run|test|build --verify-reproducible|artifact info --nodes）=> 全 0"
  echo "  11 conformance --report => 期望 1、83 = 77/0/6"
  log "DRYRUN no writes"
  exit 0
fi

FAILS=0
probe() { # probe <名字> <期望退码> <期望子串（可空）> <命令…>
  local name="$1" want="$2" sub="$3" out rc verdict
  shift 3
  out="$("$@" 2>&1)"; rc=$?
  verdict=PASS
  [ "$rc" = "$want" ] || verdict=FAIL
  if [ -n "$sub" ] && ! printf '%s' "$out" | grep -qF -- "$sub"; then verdict=FAIL; fi
  [ "$verdict" = PASS ] || FAILS=$((FAILS + 1))
  printf '%-30s rc=%-2s want=%-2s %-6s %s\n' "$name" "$rc" "$want" "$verdict" "${sub:-(只看退码)}"
  log "PROBE $name rc=$rc want=$want sub=[$sub] verdict=$verdict"
  LAST_OUT="$out"
}

log "START infy=$INFY"
[ -x "$INFY" ] || { echo "缺 release 二进制：先 cargo build --offline --release -j 1"; log "ABORT no release binary"; exit 1; }

# --- 夹具：tour 的副本 + 一份真 lock -------------------------------------------------------------
rm -rf "$S"
mkdir -p "$S"
cp -r "$REPO/examples/tour" "$S/mod"
rm -rf "$S/mod/.infy-build"
printf 'req github.com/user/mathx 1.5.0\ndirh github.com/user/mathx 1.5.0 %s\ndirh github.com/user/units 0.2.1 %s\n' "$H_OK" "$H_OK" > "$S/mod/MODULE.infy.lock"
probe build_fixture 0 "" "$INFY" build "$S/mod"
[ "$FAILS" = 0 ] || { echo "夹具都建不出来，后面免谈"; tail -5 <<< "$LAST_OUT"; log "ABORT fixture build failed"; exit 1; }
EXE="$(ls "$S"/mod/.infy-build/*/ci.exe | head -1)"
EXE_BYTES=$(wc -c < "$EXE")
log "FIXTURE exe=$EXE bytes=$EXE_BYTES"
echo "夹具产物 = $EXE（$EXE_BYTES 字节）"

# --- 比对本体 ------------------------------------------------------------------------------------
probe verify_matches 0 "逐条一致" "$INFY" artifact verify "$EXE" "$S/mod/MODULE.infy.lock"
probe verify_discover 0 "逐条一致" "$INFY" artifact verify "$EXE"
sed -i "s|$H_OK|$H_BAD|" "$S/mod/MODULE.infy.lock"
probe verify_dirhash_mismatch 7 "error[E-0560]" "$INFY" artifact verify "$EXE" "$S/mod/MODULE.infy.lock"
printf 'req github.com/user/mathx 1.5.0\n' > "$S/mod/MODULE.infy.lock"
probe verify_sum_ahead_of_lock 7 "比 lock 多" "$INFY" artifact verify "$EXE" "$S/mod/MODULE.infy.lock"
printf 'dirh dep/core 1.2.3 h1:AAAA\n' > "$S/fixture.lock"
probe undecidable_dirhash_7 7 "读不了" "$INFY" artifact verify "$FIX" "$S/fixture.lock"
if printf '%s' "$LAST_OUT" | grep -qF 'error[E-0560]'; then
  echo "G28 破了：形态不合法被报成校验和不匹配"; FAILS=$((FAILS + 1)); log "PROBE g28_separation verdict=FAIL"
else
  log "PROBE g28_separation verdict=PASS（读不了 ≠ E-0560）"
fi

# --- 不许弄坏既有的东西 ---------------------------------------------------------------------------
probe tour_fmt_check 0 "" "$INFY" fmt --check "$REPO/examples/tour"
probe tour_check 0 "" "$INFY" check "$REPO/examples/tour"
probe tour_build 0 "" "$INFY" build "$REPO/examples/tour"
probe tour_run 0 "" "$INFY" run "$REPO/examples/tour"
probe tour_test 0 "" "$INFY" test "$REPO/examples/tour"
probe tour_verify_repro 0 "" "$INFY" build "$REPO/examples/tour" --verify-reproducible
TOUR_EXE="$(ls "$REPO"/$TOUR_EXE_GLOB | head -1)"
TOUR_BYTES=$(wc -c < "$TOUR_EXE")
# tour 从来没有 MODULE.infy.lock ⇒ 这一格必须**拒绝**而不是放行（BACKLOG 的「不假装通过」）。
probe no_lock_refuses 1 "不返回 0" "$INFY" artifact verify "$TOUR_EXE"
probe tour_artifact_info 0 "" "$INFY" artifact info "$TOUR_EXE" --nodes
probe usage_lists_verify 0 "artifact info|sections|verify" "$INFY" help
probe conformance_report 1 "77" "$INFY" conformance --report
echo "tour 产物 = $TOUR_BYTES 字节（上一档记录 200700）"
log "TOUR bytes=$TOUR_BYTES fixture_bytes=$EXE_BYTES"

if [ "$FAILS" != 0 ]; then
  echo "FAIL: $FAILS 个探针不符期望"; log "END FAIL=$FAILS"; exit 1
fi
echo "PASS: 全部探针符合期望"
log "END FAIL=0"
exit 0
