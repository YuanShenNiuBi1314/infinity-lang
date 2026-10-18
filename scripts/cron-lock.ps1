<#
.SYNOPSIS
  跨定时任务的全局锁：谁持锁谁干活，别人静默让路。
.DESCRIPTION
  多个 cron（2 小时生产循环、每天推送任务）会在同一棵工作树上互相踩：并行的 `git add`、
  并行的构建、并行的推送。协议是**项目根放一个锁文件**，任何任务开工第一件事是查锁，
  锁被人持有就**静默放弃本轮**（不报错、不写 RUNLOG 流水、不改任何文件）。

  锁必须自带过期，否则一次崩溃就把后面所有轮次永久锁死：默认 TTL 45 分钟，超时视为死锁可抢占，
  抢占会在 logs/cron-lock.log 留下痕迹（谁抢的、老锁 took 多久）。

  用 powershell.exe 调用（本机没有 pwsh），且本文件带 UTF-8 BOM。
.PARAMETER Action
  check   —— 只查：无人持有则退出码 0；被人持有则退出码 10 并打印持有者。
  acquire —— 抢锁：空闲则写入自己（job/pid/时间）并退 0；被持有则退 10（不偷）。
  release —— 放锁：只有 job 名匹配才删（避免把别人的锁删掉）。
  status  —— 打印当前锁状态，永不失败。
.PARAMETER Job
  **一班一个唯一名字**，形如 prod-loop-38 / daily-push-03；release 只认同名。
  固定名（prod-loop）会让同名调用互相认领与释放，手测时复用生产名就会吃掉真班次的锁（2026-10-18 实测踩过）。
.PARAMETER TtlMinutes
  锁的存活时长，超过即视为死锁可抢占。缺省 45。
.EXAMPLE
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/cron-lock.ps1 -Action acquire -Job daily-push-03
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/cron-lock.ps1 -Action release -Job daily-push-03
  # 手测请用独立名字（如 locktest-1），不要用生产班次名。
#>
[CmdletBinding()]
param(
  [ValidateSet('check','acquire','release','status')] [string]$Action = 'check',
  [string]$Job = 'unnamed',
  [int]$TtlMinutes = 45
)

$ErrorActionPreference = 'Stop'
$repo  = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$lock  = Join-Path $repo 'cron.lock'
$logDir = Join-Path $repo 'logs'
if (-not (Test-Path $logDir)) { New-Item -ItemType Directory -Path $logDir | Out-Null }
$logFile = Join-Path $logDir 'cron-lock.log'
function Log($m) { "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')  [$Job] $m" | Add-Content -Path $logFile -Encoding utf8 }

# 锁文件是**别的进程**写的，被 kill 在半路可能留下缺字段或解析不了的 taken。
# 读不懂就当作已过期（并留痕），否则一次崩溃会把后面所有班次永久锁死。
function Lock-AgeMinutes($h) {
  try { return (New-TimeSpan -Start ([datetime]::Parse($h['taken'])) -End (Get-Date)).TotalMinutes }
  catch { return [double]::PositiveInfinity }
}

function Read-Lock {
  if (-not (Test-Path -LiteralPath $lock)) { return $null }
  $h = @{}
  foreach ($line in (Get-Content -LiteralPath $lock -ErrorAction SilentlyContinue)) {
    $kv = $line -split '=', 2
    if ($kv.Count -eq 2) { $h[$kv[0].Trim()] = $kv[1].Trim() }
  }
  $h['path'] = $lock
  return $h
}

function Held-BySomeoneElse {
  param($h)
  if (-not $h) { return $false }
  # **不按 pid 判活**：acquire 是短命令，写完就退，记下的 pid 在班次干活时早就没了 ⇒ "进程还活着"
  # 这条判据永远不会真（比没有更坏，它会骗人）。独占性靠"一班一个唯一 job 名"（见 §0.0.1 与 .PARAMETER Job）。
  $age = Lock-AgeMinutes $h
  $ttl = $TtlMinutes
  if ($h['ttl'] -and $h['ttl'] -match '^\d+$') { $ttl = [int]$h['ttl'] }
  if ($age -gt $ttl) { return $false }          # 死锁或读不懂，可抢
  return ($h['job'] -ne $Job)
}

switch ($Action) {
  'check' {
    $h = Read-Lock
    if (Held-BySomeoneElse $h) { Write-Host "held by $($h['job']) since $($h['taken'])"; Log 'YIELD  锁被他人持有，本轮静默放弃'; exit 10 }
    Write-Host 'free'; exit 0
  }
  'status' {
    $h = Read-Lock
    if (-not $h) { Write-Host 'lock: none'; exit 0 }
    $raw = Lock-AgeMinutes $h
    $age = if ([double]::IsInfinity($raw)) { 'unreadable' } else { [math]::Round($raw, 1) }
    Write-Host "lock: job=$($h['job']) pid=$($h['pid']) taken=$($h['taken']) age=${age}min ttl=$($h['ttl'])min"
    exit 0
  }
  'acquire' {
    $h = Read-Lock
    if (Held-BySomeoneElse $h) {
      Write-Host "busy: held by $($h['job']) since $($h['taken'])"
      $bage = Lock-AgeMinutes $h
      if ([double]::IsInfinity($bage)) { $bage = 'unreadable' } else { $bage = [math]::Round($bage, 1) }
      Log "BUSY   让路给 $($h['job'])（其锁 taken=$($h['taken']) ageMin=$bage）"
      exit 10
    }
    if ($h -and $h['job'] -ne $Job) { Log "STEAL  旧锁已过期（$($h['job']) @ $($h['taken'])），抢占" }
    if ($h -and $h['job'] -eq $Job) { Log "RENEW  同一班次重入刷新（taken=$($h['taken'])）" }
    @( "job=$Job", "pid=$PID", "taken=$((Get-Date).ToString('yyyy-MM-dd HH:mm:ss'))", "ttl=$TtlMinutes" ) |
      Set-Content -LiteralPath $lock -Encoding UTF8
    Log "TAKE   锁已获取 ttl=${TtlMinutes}min"
    exit 0
  }
  'release' {
    $h = Read-Lock
    if (-not $h) { Log 'RELEASE 无锁可放'; exit 0 }
    if ($h['job'] -ne $Job) { Write-Warning "锁属于 $($h['job'])，$Job 不放别人的锁"; Log 'REFUSED release 越界'; exit 0 }
    Remove-Item -LiteralPath $lock -Force -ErrorAction SilentlyContinue
    Log 'GIVE   锁已释放'
    exit 0
  }
}
