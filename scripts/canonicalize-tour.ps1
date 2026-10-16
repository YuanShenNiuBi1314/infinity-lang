<#
.SYNOPSIS
  把 examples/tour 的源码规范化到 §9.5 的格式化不动点（BACKLOG F6 / 偏差 D-17 的执行入口）。
.DESCRIPTION
  为什么这件事必须走脚本：`infy fmt` 是**写盘**动作，而 F44 未修 —— 裸 `infy fmt` 与 `-w` 都会原地
  改写文件。一次写错的格式化的代价 = 语料被静默重写 + 依赖 tour 字节的 §8.4 节点表金样一起位移。
  所以本脚本默认只报告，只有 -Apply 才写，并且写完立刻回核。

  四条硬规矩：① 默认干跑（只打 `fmt --check` 的结论与将要执行的命令）；② 幂等（已是规范写法时
  直接 exit 0，第二遍不会改写任何字节）；③ 动作与结果（含回滚命令）追加 `logs/canonicalize-tour.log`，
  失败退出码非 0；④ 不改本机状态 —— 它只动 `examples/tour/**/*.infy`，越出这个集合就拒绝并还原。

  范围守卫是**封闭白名单**：只允许 `examples/tour/` 下的 `.infy` 被改写；`MODULE.infy.toml`、
  `.infy-build/`（编译输出目录，纪律禁止触碰）与 tour 外的任何路径一律不动。
.PARAMETER Apply
  真写盘。缺省 = 干跑。
.PARAMETER Infy
  infy 可执行文件路径。缺省依次找 target/release/infy.exe、target/debug/infy.exe；都没有则失败
  （本脚本不代替构建，避免在无人值守里藏一次长编译）。
.EXAMPLE
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/canonicalize-tour.ps1
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/canonicalize-tour.ps1 -Apply
#>
[CmdletBinding()]
param(
  [switch]$Apply,
  [string]$Infy = ''
)

$ErrorActionPreference = 'Stop'
$repo   = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$logDir = Join-Path $repo 'logs'
if (-not (Test-Path $logDir)) { New-Item -ItemType Directory -Path $logDir | Out-Null }
$log    = Join-Path $logDir 'canonicalize-tour.log'
# `-Encoding utf8` 是刻意的：5.1 的 Add-Content 缺省按 ANSI(GBK) 落盘，logs/ 里既有脚本的中文行
# 因此在 bash / GitHub / 审阅工具里全是乱码（同一族的缺陷登记为 BACKLOG F46）。
function Log($m) { "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')  $m" | Add-Content -Path $log -Encoding utf8 }

# 原生命令统一走这里：5.1 在 Stop 下会把 stderr 当异常，而 infy 的状态行正写 stderr。
function Run-Native {
  param([string]$Exe, [string[]]$Args_)
  $prev = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  $out = & $Exe @Args_ 2>&1 | Out-String
  $rc  = $LASTEXITCODE
  $ErrorActionPreference = $prev
  return @{ rc = $rc; text = $out }
}

function Tour-Infy {
  Get-ChildItem -Path (Join-Path $repo 'examples\tour') -Recurse -Filter '*.infy' -File |
    Where-Object { $_.FullName -notlike '*\.infy-build\*' } |
    Sort-Object FullName
}

function Snapshot($files) {
  foreach ($f in $files) {
    $h = (Get-FileHash -Path $f.FullName -Algorithm SHA256).Hash
    $rel = $f.FullName.Substring($repo.Length + 1)
    Log("  sha256 $h  $($f.Length)B  $rel")
  }
}

Set-Location $repo
$mode = if ($Apply) { 'APPLY' } else { 'DRY-RUN' }
$head = Run-Native 'git' @('rev-parse', '--short', 'HEAD')
Log "=== canonicalize-tour start  mode=$mode  HEAD=$($head.text.Trim()) ==="

if (-not $Infy) {
  foreach ($c in @('target\release\infy.exe', 'target\debug\infy.exe')) {
    if (Test-Path $c) { $Infy = (Resolve-Path $c).Path; break }
  }
}
if (-not (Test-Path $Infy)) {
  Log "FAIL 找不到 infy 可执行文件（先跑 cargo build --offline）；本次未改动任何文件"
  Log "=== canonicalize-tour end  rc=1 ==="
  exit 1
}
Log "infy = $Infy"

$files = @(Tour-Infy)
if ($files.Count -eq 0) {
  Log 'FAIL examples/tour 下找不到 .infy 源文件；本次未改动任何文件'
  Log "=== canonicalize-tour end  rc=1 ==="
  exit 1
}
Log "范围内的 tour 源文件：$($files.Count) 个"
Log '改动前：'
Snapshot $files

$pre = Run-Native $Infy @('fmt', '--check', 'examples/tour')
Log "``fmt --check（改前）rc=$($pre.rc)"
foreach ($l in ($pre.text -split "`r?`n")) { if ($l.Trim()) { Log "  $l" } }

if ($pre.rc -eq 0) {
  Log '已是 §9.5 不动点 —— 幂等退出，未改写任何字节'
  Log "=== canonicalize-tour end  rc=0 ==="
  exit 0
}
if ($pre.rc -ne 2) {
  Log "FAIL `fmt --check` 返回码 $pre.rc 既不是 0 也不是 2（预期 2 = 有文件非规范）；未写盘"
  Log "=== canonicalize-tour end  rc=1 ==="
  exit 1
}

if (-not $Apply) {
  Log 'DRY-RUN：将执行 `infy fmt -w examples/tour`，然后回核 `fmt --check`。未写盘。'
  Log "回滚命令（若写盘后需要还原）：git checkout -- examples/tour/cmd/runner/main.infy examples/tour/core/build/mod.infy"
  Log "=== canonicalize-tour end  rc=0 (dry-run) ==="
  exit 0
}

$w = Run-Native $Infy @('fmt', '-w', 'examples/tour')
Log "fmt -w rc=$($w.rc)"
foreach ($l in ($w.text -split "`r?`n")) { if ($l.Trim()) { Log "  $l" } }

$post = Run-Native $Infy @('fmt', '--check', 'examples/tour')
Log "fmt --check（改后）rc=$($post.rc)"
foreach ($l in ($post.text -split "`r?`n")) { if ($l.Trim()) { Log "  $l" } }
Log '改动后：'
Snapshot @(Tour-Infy)

# 范围守卫：git 认为被改动的路径必须全部是 examples/tour 下的 .infy。
$changed = Run-Native 'git' @('status', '--porcelain', '--', 'examples')
$bad = @()
foreach ($l in ($changed.text -split "`r?`n")) {
  if (-not $l.Trim()) { continue }
  $p = $l.Substring(3).Trim().Trim('"')
  if ($p -notlike 'examples/tour/*') { $bad += $p }
  elseif ($p -notlike '*.infy') { $bad += $p }
  elseif ($p -like '*.infy-build*') { $bad += $p }
}
if ($bad.Count -gt 0) {
  Log "FAIL 越出白名单的改动：$($bad -join ', ') —— 立即用 git checkout 还原 examples/tour"
  $rb = Run-Native 'git' @('checkout', '--', 'examples/tour')
  Log "git checkout rc=$($rb.rc)  $($rb.text.Trim())"
  Log "=== canonicalize-tour end  rc=1 ==="
  exit 1
}
Log "范围守卫通过：被改动 $((($changed.text -split "`r?`n") | Where-Object { $_.Trim() }).Count) 个文件，全部在 examples/tour/**/*.infy 内"

if ($post.rc -ne 0) {
  Log "FAIL 写盘后 `fmt --check` 仍非 0（rc=$($post.rc)）—— §9.5 规则 12 的不动点不成立，请回滚并记 RUNLOG"
  Log "回滚命令：git checkout -- examples/tour/cmd/runner/main.infy examples/tour/core/build/mod.infy"
  Log "=== canonicalize-tour end  rc=1 ==="
  exit 1
}

Log 'OK examples/tour 现在是 §9.5 的格式化不动点（fmt --check 回核 exit 0）'
Log "=== canonicalize-tour end  rc=0 ==="
exit 0
