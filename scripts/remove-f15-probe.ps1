<#
.SYNOPSIS
  删掉 F15 轮留下的一次性解析探针 `tests/tmp_probe_arms.rs`。默认**干跑**，必须显式 -Apply 才真删。
.DESCRIPTION
  协议 §0.0 的形状：破坏性动作只能走脚本，且①默认干跑 ②幂等（文件不在就等于已完成，退 0）
  ③动作与结果追加进 logs/remove-f15-probe.log ④不碰本机状态。
  白名单是**封闭**的：只有这一个相对路径。任何解析到仓库外的路径、任何别的文件一律 REFUSED + 退非 0。
  为什么需要它：F15 的根因（宏调用做 `match` 臂体时把后面的臂吞掉，= BACKLOG F63）是靠"把解析后的 AST
  打印出来"定死的，那个探针是 `#[test] fn tmp_probe_arm_counts()` + 一个必然失败的 `panic!`，
  留在 `tests/` 里会让 `cargo test --offline` 多一个目标并且**永远红**，所以必须随交付一起删掉；
  它的结论已经以常驻锁点 `parser::tests::a_macro_call_arm_body_does_not_swallow_the_later_arms` 落地。
.PARAMETER Apply
  真正删除。不带它只打印将要删什么。
.EXAMPLE
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/remove-f15-probe.ps1
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/remove-f15-probe.ps1 -Apply
#>
[CmdletBinding()]
param(
  [switch]$Apply
)

$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$logDir = Join-Path $repo 'logs'
if (-not (Test-Path $logDir)) { New-Item -ItemType Directory -Path $logDir | Out-Null }
$log = Join-Path $logDir 'remove-f15-probe.log'
function Log($m) { "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')  $m" | Add-Content -Path $log -Encoding utf8 }

# 封闭白名单：仓库内相对路径，逐条整名相等
$allow = @('tests/tmp_probe_arms.rs')

function Resolve-Inside([string]$rel) {
  if ($rel -match '[\\/]') { }  # 允许子目录，但仍必须在仓库内
  $full = [IO.Path]::GetFullPath((Join-Path $repo $rel))
  if (-not $full.StartsWith($repo + [IO.Path]::DirectorySeparatorChar)) { return $null }
  return $full
}

$rc = 0
try {
  foreach ($rel in $allow) {
    $full = Resolve-Inside $rel
    if (-not $full) { Log "REFUSED 越界: $rel"; Write-Host "REFUSED outside repo: $rel" -ForegroundColor Red; $rc = 2; continue }
    if (-not (Test-Path -LiteralPath $full)) { Log "SKIP    absent: $rel"; Write-Host "SKIP (already gone): $rel"; continue }
    $len = (Get-Item -LiteralPath $full).Length
    if (-not $Apply) {
      Log "DRYRUN  would delete: $rel ($len bytes)"
      Write-Host "[dry-run] would delete $rel ($len bytes) — add -Apply"
      continue
    }
    Remove-Item -LiteralPath $full -Force
    if (Test-Path -LiteralPath $full) { Log "FAIL    still present: $rel"; Write-Host "FAIL: still present $rel" -ForegroundColor Red; $rc = 3; continue }
    Log "DELETED $rel ($len bytes)"
    Write-Host "deleted $rel ($len bytes)"
  }
  if ($rc -eq 0) { Log "OK      Apply=$Apply rc=0" } else { Log "FAIL    Apply=$Apply rc=$rc" }
  exit $rc
} catch {
  Log "ERROR   $($_.Exception.Message)"
  Write-Host "ERROR: $($_.Exception.Message)" -ForegroundColor Red
  exit 1
}
