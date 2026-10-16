<#
.SYNOPSIS
  清理仓库内的一次性构建产物与临时文件。默认**干跑**，必须显式 -Apply 才真删。
.DESCRIPTION
  协议 §0 的样板：破坏性操作只能走脚本，且①默认干跑 ②幂等 ③写 logs/ ④不留系统改动。
  白名单是**封闭**的：只允许仓库内下列前缀，任何解析到仓库外的路径一律拒绝并红着退出。
.PARAMETER Apply
  真正删除。不带它只打印「将会删除什么」。
.PARAMETER Deep
  额外清 target/（cargo 构建目录，重编代价大，默认不动）。
.EXAMPLE
  pwsh -File scripts/cleanup.ps1
  pwsh -File scripts/cleanup.ps1 -Apply
#>
[CmdletBinding()]
param(
  [switch]$Apply,
  [switch]$Deep
)

$ErrorActionPreference = 'Stop'
$repo   = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$logDir = Join-Path $repo 'logs'
if (-not (Test-Path $logDir)) { New-Item -ItemType Directory -Path $logDir | Out-Null }
$log    = Join-Path $logDir 'cleanup.log'
function Log($m) { "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')  $m" | Add-Content -Path $log }

# 封闭白名单：仓库内的相对路径前缀
$allow = @('target/scratch', 'target/debug', 'target/release', 'logs/tmp')
# 仓库根下允许按名字删的一次性产物（编译出来的可执行文件等）
$loose = @('rt_tests.exe','ifbc_tests.exe','rt_tests','ifbc_tests','a.out','*.tmp')

function Resolve-Inside([string]$p) {
  $full = [IO.Path]::GetFullPath((Join-Path $repo $p.TrimStart('.','/','\')))
  if (-not $full.StartsWith($repo + [IO.Path]::DirectorySeparatorChar)) { return $null }
  return $full
}

$targets = @()
foreach ($pat in $loose) {
  Get-ChildItem -LiteralPath $repo -Filter $pat -File -ErrorAction SilentlyContinue | ForEach-Object { $targets += $_.FullName }
}
foreach ($p in $allow) {
  if ($p -in @('target/debug','target/release') -and -not $Deep) { continue }
  $full = Resolve-Inside $p
  if ($full -and (Test-Path -LiteralPath $full)) { $targets += $full }
}

# F4 轮补的 stray 族：C harness 的夹具路径按 `cd target/scratch` 的配方写死（`test_rt.c:1501`），
# 从错误的 CWD 跑就往**运行目录**里拉 `rt_artifact*.bin/.txt`、`rt_diag.log` 等一次性文件。
# 只扫两个**具体目录的这一层**（仓库根与 runtime/），名字族是封闭清单；不进 runtime/fixtures/ ——
# 那里是提交进仓库的真值源，任何通配都不许覆盖它。
$strayDirs = @($repo, (Join-Path $repo 'runtime'))
$strayPats = @('rt_artifact*.bin','rt_artifact*.txt','rt_diag.log','rt_render*.log','rt_sample.infy','rt_fs_*')
$repoRoot  = $repo.TrimEnd('\','/')
$sep       = [IO.Path]::DirectorySeparatorChar
foreach ($d in $strayDirs) {
  $full = $d.TrimEnd('\','/')
  if (-not (Test-Path -LiteralPath $full)) { continue }
  if ($full -ne $repoRoot -and -not $full.StartsWith($repoRoot + $sep)) { Log "REFUSED stray 目录在仓库外：$full"; continue }
  foreach ($pat in $strayPats) {
    Get-ChildItem -LiteralPath $full -Filter $pat -File -ErrorAction SilentlyContinue | ForEach-Object { $targets += $_.FullName }
  }
}

if ($targets.Count -eq 0) { Log "NOOP   没有可清理项"; Write-Host 'nothing to clean'; exit 0 }

$mode = if ($Apply) { 'APPLY' } else { 'DRYRUN' }
Log "$mode  $($targets.Count) 项"
foreach ($t in $targets) {
  $rel = $t.Substring($repo.Length).TrimStart('\','/')
  $inside = $false
  foreach ($p in $allow) { if ($rel -replace '\\','/' -like "$p*") { $inside = $true } }
  if (-not $inside) {
    # 白名单外的具体文件（*.tmp / 测试二进制）也要确认落在仓库内
    if ($null -eq (Resolve-Inside $rel)) { Log "REFUSED $rel（解析到仓库外）"; Write-Warning "refused: $rel"; continue }
  }
  $size = try { [int]((Get-Item -LiteralPath $t).Length / 1KB) } catch { -1 }
  if (-not $Apply) { Log "would delete  $rel  (${size}KB)"; Write-Host "[dry-run] 将删除 $rel"; continue }
  try { Remove-Item -LiteralPath $t -Recurse -Force -ErrorAction Stop; Log "deleted  $rel" } 
  catch { Log "FAILED   $rel : $($_.Exception.Message)"; Write-Warning "删不掉 $rel : $($_.Exception.Message)" }
}
if (-not $Apply) { Write-Host "干跑结束：确认无误后加 -Apply（幂等，可重复运行）"; exit 0 }
exit 0
