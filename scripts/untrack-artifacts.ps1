<#
.SYNOPSIS
  把仓库里被跟踪的"一次性产物"从 git 索引里摘掉（文件留在磁盘上），让 docs/RUNLOG.md 成为唯一的审计线（BACKLOG F3）。
.DESCRIPTION
  为什么是 --cached 而不是删除：这些 log/碎片是脚本与历史轮次的现场记录，删掉等于销毁证据。
  本脚本只改索引（git rm --cached），工作树一个字节都不动 ⇒ 完全可逆（日志里逐条写了回滚命令）。
  四条硬规矩同 cleanup.ps1：① 默认干跑，必须显式 -Apply；② 幂等（已脱离跟踪的直接跳过）；
  ③ 动作与结果追加到 logs/untrack-artifacts.log，失败退出码非 0；④ 白名单封闭 —— 只有匹配产物形状
  的路径能进入，docs/、spec/、src/、.gitattributes、.github/ 这些真值源命中即拒。
  本机只有 Windows PowerShell 5.1：用 powershell.exe 调用，本文件必须带 UTF-8 BOM；
  原生命令一律走 Run-Native（$ErrorActionPreference='Stop' 下 git 的 stderr 会被当成异常）。
.PARAMETER Apply
  真正脱离跟踪。不带它只打印「将对哪条路径跑 git rm --cached」。
.EXAMPLE
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/untrack-artifacts.ps1
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/untrack-artifacts.ps1 -Apply
#>
[CmdletBinding()]
param(
  [switch]$Apply
)

$repo   = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$logDir = Join-Path $repo 'logs'
if (-not (Test-Path $logDir)) { New-Item -ItemType Directory -Path $logDir | Out-Null }
$log = Join-Path $logDir 'untrack-artifacts.log'
function Log($m) { "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')  $m" | Add-Content -Path $log }

function Run-Native {
  param([string]$Exe, [string[]]$Args_)
  $prev = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  $out = & $Exe @Args_ 2>&1 | Out-String
  $rc  = $LASTEXITCODE
  $ErrorActionPreference = $prev
  return @{ rc = $rc; text = $out }
}

# 封闭白名单：仓库根下的 *.log / *.tmp / 已知碎片，加上 logs/ 里的 *.log。别的形状一律拒。
function Test-Artifact([string]$rel) {
  $slash = $rel -replace '\\','/'
  if ($slash -like 'docs/*' -or $slash -like 'spec/*' -or $slash -like 'src/*' -or
      $slash -like 'runtime/*' -or $slash -like '.github/*' -or $slash -like 'conformance/*' -or
      $slash -like 'examples/*' -or $slash -like 'capabilities/*' -or $slash -like 'scripts/*' -or
      $slash -eq '.gitignore' -or $slash -eq '.gitattributes') { return $false }
  if ($slash -like 'logs/*.log') { return $true }
  if ($slash -like '*.log' -or $slash -like '*.tmp' -or $slash -like '*.o') { return $true }
  if ($slash -eq 'chunk.txt') { return $true }
  return $false
}

Push-Location $repo
try {
  $tracked = (Run-Native 'git' @('ls-files')).text -split "\r?\n" | Where-Object { $_.Trim() }
  $hit = @($tracked | Where-Object { Test-Artifact $_ })
  $foreign = @($tracked | Where-Object { -not (Test-Artifact $_) -and (($_ -like '*.log') -or ($_ -like '*.tmp') -or ($_ -like '*.o')) })

  # 真值源里若混进产物形状（docs/RUNLOG.md 之类），必须人工看清再动 —— 拒，不改判据
  foreach ($f in $foreign) { Log "REFUSED $f（不在封闭白名单里）"; Write-Warning "refused: $f" }

  if ($hit.Count -eq 0) { Log 'NOOP   没有仍在跟踪的一次性产物'; Write-Host 'nothing tracked'; exit 0 }

  $mode = if ($Apply) { 'APPLY ' } else { 'DRYRUN' }
  Log "$mode  $($hit.Count) 项"
  $failed = 0
  foreach ($rel in $hit) {
    $full = [IO.Path]::GetFullPath((Join-Path $repo $rel))
    if (-not $full.StartsWith($repo + [IO.Path]::DirectorySeparatorChar)) { Log "REFUSED $rel（解析到仓库外）"; $failed++; continue }
    if (-not $Apply) { Log "would untrack  $rel"; Write-Host "[dry-run] 将脱离跟踪 $rel"; continue }
    $r = Run-Native 'git' @('rm','--cached','--',$rel)
    if ($r.rc -ne 0) {
      Log "FAILED   $rel : rc=$($r.rc) $($r.text.Trim())"
      Write-Warning "git rm --cached 失败：$rel"
      $failed++
      continue
    }
    Log "untracked $rel  (文件留在磁盘；回滚 = git add -- $rel)"
    Write-Host "untracked: $rel"
  }

  $left = @(((Run-Native 'git' @('ls-files')).text -split "\r?\n" | Where-Object { $_.Trim() }) | Where-Object { Test-Artifact $_ })
  Log "VERIFY mode=$mode 命中=$($hit.Count) 失败=$failed 剩余跟踪=$($left.Count)"
  if ($failed -gt 0) { Write-Warning "有 $failed 项失败 —— 记进 docs/RUNLOG.md"; exit 1 }
  if ($Apply -and $left.Count -gt 0) { Write-Warning "仍有 $($left.Count) 项在跟踪：$($left -join ', ')"; exit 1 }
  if ($Apply) { Write-Host "done; 仍跟踪的一次性产物 = $($left.Count)" }
  else { Write-Host "干跑结束：确认无误后加 -Apply（幂等，可重复运行）" }
  exit 0
} finally { Pop-Location }
