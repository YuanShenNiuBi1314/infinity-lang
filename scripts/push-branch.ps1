<#
.SYNOPSIS
  把当前 HEAD 推到一个受控分支，带重试与远端回查。cron 协议 §0 / §7 的交付入口。
.DESCRIPTION
  为什么这个脚本默认就动手、而 cleanup.ps1 默认干跑：推送是**加法**（前进一个分支 ref，
  可用 `git push origin :branch` 撤销），删除是**减法**（不可逆）。免确认规则针对的是不可逆损失，
  所以本脚本改用硬性拒绝对冲风险：绝不碰 main/master、绝不 --force、绝不删远端分支。
  本机没有 pwsh（PowerShell 7），只有 Windows PowerShell 5.1 —— 用 powershell.exe 调用，
  且本文件必须带 UTF-8 BOM，否则 5.1 按 ANSI 解析中文注释会直接语法报错。
  另两个 5.1 坑：$ErrorActionPreference='Stop' 下原生命令的 stderr 会被当成异常（git 的进度条
  正写在 stderr），以及中文输出受代码页影响 —— 所以本脚本一律用 Run-Native 包一层。
.PARAMETER Branch
  目标分支，缺省 toolchain。main/master 一律拒绝（合并主干由人决定）。
.PARAMETER DryRun
  只打印将要执行的命令，不推送。
.EXAMPLE
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/push-branch.ps1 -Branch toolchain
#>
[CmdletBinding()]
param(
  [string]$Branch = 'toolchain',
  [switch]$DryRun
)

$Remote = 'origin'
$repo   = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$logDir = Join-Path $repo 'logs'
if (-not (Test-Path $logDir)) { New-Item -ItemType Directory -Path $logDir | Out-Null }
$log = Join-Path $logDir 'push-branch.log'
function Log($m) { "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')  $m" | Add-Content -Path $log }

# 原生命令统一走这里：不因为 stderr 抛异常，并如实带回退出码
function Run-Native {
  param([string]$Exe, [string[]]$Args_)
  $prev = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  $out = & $Exe @Args_ 2>&1 | Out-String
  $rc  = $LASTEXITCODE
  $ErrorActionPreference = $prev
  return @{ rc = $rc; text = $out }
}

# --- 硬门禁 -------------------------------------------------------------------------
if ($Branch -in @('main','master')) { Log "REFUSED: $Branch 受保护，合并主干由项目所有者决定"; Write-Error "refusing to push to protected branch '$Branch'"; exit 2 }
if ($Branch -notmatch '^[A-Za-z0-9._/-]{1,100}$') { Log "REFUSED: 非法分支名 $Branch"; Write-Error 'invalid branch name'; exit 2 }

Push-Location $repo
try {
  $head = (Run-Native 'git' @('rev-parse','--short','HEAD')).text.Trim()
  $sha  = (Run-Native 'git' @('rev-parse','HEAD')).text.Trim()
  if ($DryRun) { Log "DRYRUN HEAD=$head -> $Remote/$Branch"; Write-Host "[dry-run] push HEAD:$Branch ($head)"; exit 0 }

  $urlRes = Run-Native 'git' @('config','--get',"remote.$Remote.url")
  if ($urlRes.rc -ne 0 -or -not $urlRes.text.Trim()) { Log 'REFUSED: 没有可用远端'; Write-Error "no remote $Remote"; exit 2 }
  $url = $urlRes.text.Trim()

  # 幂等：远端已经等于我们就直接成功
  $api = "repos/YuanShenNiuBi1314/infinity-lang/git/refs/heads/$Branch"
  $before = (Run-Native 'gh' @('api', $api, '--jq', '.object.sha')).text.Trim()
  if ($before -eq $sha) { Log "NOOP  远端 $Branch 已 = $head"; Write-Host "already up to date ($head)"; exit 0 }

  $attempts = 0; $rc = 1
  foreach ($extra in @(@(), @('-c','http.version=HTTP/1.1'))) {
    $attempts++
    Log "TRY   #$attempts HEAD=$head -> $Remote/$Branch"
    $a = @($extra) + @('-c','credential.helper=','push','--quiet',$url,"HEAD:$Branch")
    $r = Run-Native 'git' $a
    $rc = $r.rc
    foreach ($line in ($r.text -split "\r?\n")) { if ($line.Trim()) { Log "  out: $line" } }
    if ($rc -eq 0) { break }
    Start-Sleep -Seconds 8
    if ($attempts -ge 2) { break }   # 两次即停，不循环轰炸
  }

  # 成功也要回查远端：欠账只认现算
  $after  = (Run-Native 'gh' @('api', $api, '--jq', '.object.sha')).text.Trim()
  $behind = -1
  if ($after) { $behind = [int](Run-Native 'git' @('rev-list','--count',"$after..HEAD")).text.Trim() }
  Log "VERIFY push_rc=$rc 远端=$after 本地=$sha 欠账=$behind"
  if ($rc -eq 0 -and $after -eq $sha) { Write-Host "pushed; remote verified at $head (debt 0)"; exit 0 }
  Write-Warning "推送未确认：rc=$rc 远端=$after 本地=$sha 欠账=$behind —— 记进 docs/RUNLOG.md，下一轮再推"
  exit 1
} finally { Pop-Location }
