<#
.SYNOPSIS
  把当前 HEAD 推到一个受控分支，带重试与远端回查。cron 协议 §0 / §7 的交付入口。
.DESCRIPTION
  为什么这个脚本是「默认就动手」而 cleanup.ps1 是「默认干跑」：
  推送是**加法**（新建/前进一个分支 ref，可用 `git push origin :branch` 撤销），
  而删除文件是**减法**（不可逆）。免确认规则只针对会造成不可逆损失的动作。
  因此本脚本用硬性拒绝对冲风险：绝不推 main/master、绝不 --force、绝不删远端分支。
.PARAMETER Branch
  目标分支。缺省 toolchain。值为 main/master 一律拒绝（合并主干由人决定）。
.PARAMETER Remote
  远端名，缺省 origin。
.PARAMETER DryRun
  只打印将要执行的命令，不推送。
.EXAMPLE
  pwsh -File scripts/push-branch.ps1 -Branch toolchain
  pwsh -File scripts/push-branch.ps1 -Branch toolchain -DryRun
#>
[CmdletBinding()]
param(
  [string]$Branch = 'toolchain',
  [string]$Remote = 'origin',
  [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$repo   = Split-Path -Parent $PSScriptRoot
$logDir = Join-Path $repo 'logs'
if (-not (Test-Path $logDir)) { New-Item -ItemType Directory -Path $logDir | Out-Null }
$log    = Join-Path $logDir 'push-branch.log'
function Log($m) { "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')  $m" | Add-Content -Path $log }

# --- 硬门禁：越界就红着退出，不做「差不多也行」 -------------------------------------------
if ($Branch -in @('main', 'master')) {
  Log "REFUSED: 分支 $Branch 受保护，合并主干由项目所有者决定"
  Write-Error "refusing to push to protected branch '$Branch' (merge by a human instead)"
  exit 2
}
if ($Branch -notmatch '^[A-Za-z0-9._/-]{1,100}$') { Log "REFUSED: 非法分支名 $Branch"; Write-Error "invalid branch name"; exit 2 }

Push-Location $repo
try {
  $head = (git rev-parse --short HEAD).Trim()
  $sha  = (git rev-parse HEAD).Trim()
  if ($DryRun) { Log "DRYRUN  HEAD=$head  -> $Remote/$Branch"; Write-Host "[dry-run] git push $Remote HEAD:$Branch  (HEAD=$head)"; exit 0 }

  # 幂等：远端已经是我们就直接成功返回，不重复推
  $url = (git config --get "remote.$Remote.url")
  if (-not $url) { Log "REFUSED: 没有远端 $Remote"; Write-Error "no such remote"; exit 2 }
  $creds = [uri]::new($url)
  $bare  = "$($creds.Scheme)://$($creds.Host)$($creds.AbsolutePath)"
  $remoteSha = ''
  try { $remoteSha = (gh api "repos/YuanShenNiuBi1314/infinity-lang/git/refs/heads/$Branch" --jq '.object.sha' 2>$null).Trim() } catch { $remoteSha = '' }
  if ($remoteSha -eq $sha) { Log "NOOP   远端 $Branch 已 = $head"; Write-Host "already up to date ($head)"; exit 0 }

  $attempts = 0; $rc = 1
  foreach ($extra in @(@(), @('-c','http.version=HTTP/1.1'))) {
    $attempts++
    Log "TRY    #$attempts HEAD=$head -> $Remote/$Branch"
    & git -c credential.helper= push $extra @('--quiet', $url, "HEAD:$Branch") 2>&1 | ForEach-Object { Log "  out: $_" }
    $rc = $LASTEXITCODE
    if ($rc -eq 0) { break }
    Start-Sleep -Seconds 8
    if ($attempts -ge 2) { break }   # 不循环轰炸：两次即停，下一轮再试
  }

  # 成功也要回查远端（协议 §0.6：欠账只认现算）
  $after = ''
  try { $after = (gh api "repos/YuanShenNiuBi1314/infinity-lang/git/refs/heads/$Branch" --jq '.object.sha' 2>$null).Trim() } catch { $after = '' }
  $behind = if ($after) { [int](git rev-list --count "$after..HEAD") } else { -1 }
  Log "VERIFY push_rc=$rc 远端=$after 本地=$sha 欠账=$behind"
  if ($rc -eq 0 -and $after -eq $sha) { Write-Host "pushed; remote verified at $head (debt 0)"; exit 0 }
  Write-Warning "推送未确认：rc=$rc 远端=$after 本地=$sha 欠账=$behind —— 记进 docs/RUNLOG.md，下一轮再推"
  exit 1
} finally { Pop-Location }
