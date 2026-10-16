<#
.SYNOPSIS
  F8 mutation probe: proves the new host-nesting rollback assertions actually go red.
.DESCRIPTION
  Copies the C runtime sources into target/scratch/f8mut, applies one named mutation, builds the
  rt harness there and runs it. Nothing in the repository is modified: the mutated trees are
  throwaway copies under the gitignored target/ directory.

  Why the probe exists: the F8 fix adds assertions that a caught panic must leave both
  g_host_nesting and the host-lock depth at the catch point's snapshot. An assertion that nobody
  has seen fail is not evidence. M2 also demonstrates the leaked-lock shape, which hangs instead
  of failing, so every run is bounded by a timeout and the process is killed when it expires.

  The harness resolves its artifact fixture as ../../runtime/fixtures/..., so the working
  directory of every run is target/scratch (depth 2 from the repo root), like the main chains.

  ASCII-only on purpose: Windows PowerShell 5.1 parses a BOM-less UTF-8 file as ANSI, and the
  log lines are read back by other tools, so this script writes neither.
.PARAMETER Apply
  Really build and run. Without it the script only prints the plan (dry-run, the default).
.PARAMETER TimeoutSec
  Per-run wall-clock budget, default 120 seconds. A run that exceeds it is reported as DEADLOCK
  and killed.
.EXAMPLE
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/f8-mutation-probe.ps1 -Apply
#>
[CmdletBinding()]
param(
  [switch]$Apply,
  [int]$TimeoutSec = 60
)

$ErrorActionPreference = 'Stop'
$repo   = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$srcDir = Join-Path $repo 'runtime'
$work   = Join-Path $repo 'target\scratch\f8mut'
$runDir = Join-Path $repo 'target\scratch'
$logDir = Join-Path $repo 'logs'
if (-not (Test-Path $logDir)) { New-Item -ItemType Directory -Path $logDir | Out-Null }
$log    = Join-Path $logDir 'f8-mutation-probe.log'
$enc    = New-Object System.Text.UTF8Encoding($false)
function Log($m) { [IO.File]::AppendAllText($log, "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')  $m`r`n", $enc) }

function Invoke-Native {
  param([string]$Exe, [string[]]$Args_)
  $prev = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  $out = & $Exe @Args_ 2>&1 | Out-String
  $rc  = $LASTEXITCODE
  $ErrorActionPreference = $prev
  return @{ rc = $rc; text = $out }
}

# Each mutation is (name, description, old text, new text). Anchors are exact substrings of
# runtime/infy_rt.c; a mutation whose anchor is missing is a hard error, never a silent skip.
$Mutations = @(
  @{ name = 'control'
     desc = 'no mutation: the copy must reproduce the green run'
     old  = ''
     new  = '' }
  @{ name = 'M1-no-rollback'
     desc = 'catch points forget to roll host-call state back (the pre-F8 implementation)'
     old  = '    host_call_rollback(t.host_depth, t.host_lock_depth);'
     new  = '    /* MUTANT: rollback removed */' }
  @{ name = 'M2-no-unlock'
     desc = 'depth is clamped but the leaked host lock is not released'
     old  = '    while (g_host_lock_depth > to_lock_depth) { g_host_lock_depth--; infy_mtx_unlock(&g_host_mu); }'
     new  = '    g_host_lock_depth = to_lock_depth; /* MUTANT: lock never released */' }
)

Log "=== probe start apply=$Apply timeout=${TimeoutSec}s repo=$repo ==="

if (-not $Apply) {
  Log 'DRY-RUN: plan only, nothing built or run'
  Write-Host '[dry-run] plan:'
  Write-Host "  copy  $srcDir\*.c / *.h  ->  $work\<mutation>"
  Write-Host "  build gcc -std=c11 -O1 -Wall -Wextra infy_rt.c test_rt.c -o rt.exe"
  Write-Host "  run   from $runDir with a ${TimeoutSec}s timeout, kill on expiry"
  foreach ($m in $Mutations) { Write-Host ("  mutation {0}: {1}" -f $m.name, $m.desc) }
  Write-Host '[dry-run] pass -Apply to execute'
  exit 0
}

if (-not (Get-Command gcc -ErrorAction SilentlyContinue)) {
  Log 'ABORT: gcc not on PATH'
  Write-Error 'gcc not found on PATH; the probe needs it to build the harness'
  exit 2
}

$rc_all = 0
foreach ($m in $Mutations) {
  $dir   = Join-Path $work $m.name
  $exe   = Join-Path $dir 'rt.exe'
  $outF  = Join-Path $runDir ("f8_{0}.out" -f $m.name)
  New-Item -ItemType Directory -Force -Path $dir | Out-Null
  Copy-Item (Join-Path $srcDir '*.c') $dir -Force
  Copy-Item (Join-Path $srcDir '*.h') $dir -Force
  $rtPath = Join-Path $dir 'infy_rt.c'
  if ($m.old -ne '') {
    $text = [IO.File]::ReadAllText($rtPath)
    $hits = ([regex]::Matches($text, [regex]::Escape($m.old))).Count
    if ($hits -lt 1) {
      Log ("ABORT {0}: anchor not found in infy_rt.c -- the mutation no longer matches the source" -f $m.name)
      Write-Error ("{0}: anchor missing" -f $m.name)
      $rc_all = 1
      continue
    }
    $text = $text.Replace($m.old, $m.new)
    [IO.File]::WriteAllText($rtPath, $text, $enc)
    Log ("{0}: anchor replaced at {1} site(s)" -f $m.name, $hits)
  }

  # exact CI step 10 recipe: infy_rt.c + test_rt.c, nothing else
  $b = Invoke-Native gcc @('-std=c11', '-O1', '-Wall', '-Wextra',
                          (Join-Path $dir 'infy_rt.c'),
                          (Join-Path $dir 'test_rt.c'), '-o', $exe)
  if ($b.rc -ne 0) {
    Log ("{0}: BUILD FAILED rc={1}`n{2}" -f $m.name, $b.rc, $b.text)
    Write-Host ("{0}: build failed (rc {1})`n{2}" -f $m.name, $b.rc, $b.text)
    $rc_all = 1
    continue
  }
  # A mutant that leaks the host lock hangs inside the harness (AcquireSRWLockExclusive by the
  # owning thread never returns), so stdout goes to a file through cmd and the whole process tree
  # gets killed on timeout -- the partial output still carries the FAIL lines.
  if (Test-Path $outF) { Remove-Item $outF -Force }
  # One single argument string: cmd needs the inner quotes preserved, and ArgumentList arrays get
  # re-quoted by PowerShell in a way cmd rejects on this path (Chinese directory name).
  $cmdLine = '/c ""{0}" > "{1}" 2>&1"' -f $exe, $outF
  $p = Start-Process -FilePath $env:ComSpec -ArgumentList $cmdLine `
        -WorkingDirectory $runDir -NoNewWindow -PassThru
  $timedOut = $false
  if (-not $p.WaitForExit($TimeoutSec * 1000)) {
    $timedOut = $true
    & taskkill.exe '/F' '/T' '/PID' $p.Id 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
  } else {
    $p.WaitForExit()
  }
  $exit = if ($timedOut) { 'killed' } else { try { $p.ExitCode } catch { 'unknown' } }
  $text = if (Test-Path $outF) { [IO.File]::ReadAllText($outF) } else { '' }
  if (-not $text) { Log ("{0}: no output captured; cmd line was {1}" -f $m.name, $cmdLine) }
  $fails = ([regex]::Matches($text, '  FAIL')).Count
  $lines = @($text -split "`r?`n")
  $tail = ($lines | Where-Object { $_ -ne '' } | Select-Object -Last 1)
  Log ("{0}: RESULT=exit={1} fail_lines={2} timed_out={3} last=`"{4}`"" -f $m.name, $exit, $fails, $timedOut, $tail)
  Write-Host ("{0}: exit={1}  FAIL lines={2}  timed_out={3}" -f $m.name, $exit, $fails, $timedOut)
  foreach ($fl in (@($lines | Where-Object { $_ -like '*  FAIL*' }))) { Write-Host "    $fl" }
  if ($m.name -eq 'control') {
    # the control copy must reproduce the shipped green run: bounded output plus a zero-failure tail
    if ($timedOut -or $fails -ne 0 -or $text -notmatch '0 failures') { $rc_all = 1 }
  }
}

Log "=== probe done rc_all=$rc_all ==="
if ($rc_all -ne 0) { Write-Host 'probe: control build/run did not stay green (see logs/f8-mutation-probe.log)'; exit 1 }
exit 0
