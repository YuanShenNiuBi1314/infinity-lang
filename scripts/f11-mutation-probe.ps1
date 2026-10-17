<#
.SYNOPSIS
  F11 mutation probe: proves the array-length assertions go red when the silent zero comes back.
.DESCRIPTION
  Spec 4.5 / 2.12 (BACKLOG F11) replaced `const_eval_usize(len).unwrap_or(0)` with an explicit
  `E-0131` plus an `Invalid` type, dropped the `u128 -> u64` fold in the same reader, and stopped
  the same root cause from printing twice (a struct field type is resolved in two passes). Each of
  those four decisions has an assertion; an assertion nobody has seen fail is not evidence, so this
  script rebuilds the crate from throwaway copies with one decision reverted at a time.

  Nothing in the repository is modified. Every copy lives under target/scratch/f11mut/work, which
  is gitignored, and the copy keeps its own target/ so only the edited crate recompiles per mutant.
  The copy is rebuilt from the repository at the start of every probe (idempotent: two runs see the
  same input), and every file a mutant can touch is restored from the repository before that mutant
  runs, so no mutation leaks into the next one.

  The copied tree includes conformance/, docs/ and examples/ because the test suite reads its own
  corpus, the pass-rate table and the tour sources from CWD.

  ASCII-only on purpose: Windows PowerShell 5.1 parses a BOM-less UTF-8 file as ANSI.
.PARAMETER Apply
  Really copy, mutate, build and test. Without it the script only prints the plan (the default).
.PARAMETER Only
  Run a single mutation by name (repeatable). Default: control plus every mutation.
.PARAMETER TimeoutSec
  Wall-clock budget per cargo run, default 900 seconds.
.EXAMPLE
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/f11-mutation-probe.ps1 -Apply
#>
[CmdletBinding()]
param(
  [switch]$Apply,
  [string[]]$Only = @(),
  [int]$TimeoutSec = 900
)

$ErrorActionPreference = 'Stop'
$repo   = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$work   = Join-Path $repo 'target\scratch\f11mut\work'
$logDir = Join-Path $repo 'logs'
if (-not (Test-Path $logDir)) { New-Item -ItemType Directory -Path $logDir | Out-Null }
$log    = Join-Path $logDir 'f11-mutation-probe.log'
$enc    = New-Object System.Text.UTF8Encoding($false)
function Log($m) { [IO.File]::AppendAllText($log, "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')  $m`r`n", $enc) }

function Invoke-Native {
  param([string]$Exe, [string[]]$Args_, [string]$Cwd)
  $prev = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  Push-Location $Cwd
  $out = & $Exe @Args_ 2>&1 | Out-String
  $rc  = $LASTEXITCODE
  Pop-Location
  $ErrorActionPreference = $prev
  return @{ rc = $rc; text = $out }
}

# Parts of the repository that are NOT copied: build output, history, and this script's own logs.
$Exclude = @('target', '.git', 'logs')

# (name, file, exact old text, new text, the test name(s) that MUST go red).
$Mutations = @(
  @{ name = 'control'; file = ''; old = ''; new = ''; expect = @()
     desc = 'no mutation: the copy must reproduce the green suite' }
  @{ name = 'M1-unwrap-zero'; file = 'src\sema.rs'
     desc = 'the F11 shape comes back: an unevaluable length becomes a silent 0 again'
     old  = 'match self.const_eval_usize(len) {'
     new  = 'match Some(self.const_eval_usize(len).unwrap_or(0)) {'
     expect = @('an_unevaluable_array_length_is_e0131', 'an_over_wide_array_length_does_not_wrap') }
  @{ name = 'M2-uint-as-u64'; file = 'src\sema.rs'
     desc = 'the width fold comes back in the length reader (F10 family)'
     old  = 'ConstValue::UInt(u) => u64::try_from(u).ok(),'
     new  = 'ConstValue::UInt(u) => Some(u as u64),'
     expect = @('an_over_wide_array_length_does_not_wrap') }
  @{ name = 'M3-no-dedupe'; file = 'src\sema.rs'
     desc = 'the length reports on every pass, so a field prints it twice'
     old  = 'if self.h.diags.len() == before && self.unresolved_ty_spans.insert(key) {'
     new  = 'if self.h.diags.len() == before { let _ = self.unresolved_ty_spans.insert(key);'
     expect = @('an_unevaluable_array_length_is_e0131', 'an_over_wide_array_length_does_not_wrap') }
  @{ name = 'M4-field-fallback'; file = 'src\sema.rs'
     desc = 'the generic field message ignores what resolution already reported'
     old  = 'if t == Ty::Invalid && !self.unresolved_ty_spans.contains(&(f.ty.span.file, f.ty.span.start, f.ty.span.end)) {'
     new  = 'if t == Ty::Invalid {'
     expect = @('an_unevaluable_array_length_is_e0131') }
  @{ name = 'M5-binding-fallback'; file = 'src\sema.rs'
     desc = 'the binding fallback claims "neither annotation nor initializer" under a reported annotation'
     old  = 'if ty.is_none() && init.is_none() {'
     new  = 'if matches!(actual, Ty::Invalid) && init.is_none() {'
     expect = @('an_unevaluable_array_length_is_e0131') }
)

Log "=== probe start apply=$Apply timeout=${TimeoutSec}s repo=$repo ==="

if (-not $Apply) {
  Log 'DRY-RUN: plan only, nothing copied, built or tested'
  Write-Host '[dry-run] plan:'
  Write-Host "  copy  repo tree (minus $($Exclude -join ', ')) -> $work"
  Write-Host  '  test  cargo test --offline --lib -j 1   (bounded by a timeout, killed on expiry)'
  foreach ($m in $Mutations) { Write-Host ("  mutation {0}: {1}" -f $m.name, $m.desc) }
  Write-Host '[dry-run] pass -Apply to execute'
  exit 0
}

$selected = if ($Only.Count -gt 0) { @($Mutations | Where-Object { $Only -contains $_.name }) } else { $Mutations }
if ($selected.Count -eq 0) { Log 'ABORT: -Only named no known mutation'; Write-Error 'unknown mutation name'; exit 2 }

# One copy for the whole probe: the crate rebuilds incrementally, and each mutant restores only the
# file it touches. A missing anchor or a missing source file is fatal here, not a silent skip --
# a probe whose copy silently failed would report "no teeth" for every mutation.
if (Test-Path $work) {
  # A leftover file where a directory belongs makes the copy silently incomplete (an earlier version
  # of this script copied `src\*` into a destination that did not exist yet and ended up with `src`
  # as a single file), which every later verdict would misread. Clean only this script's own work dir.
  $full = [IO.Path]::GetFullPath($work)
  $root = [IO.Path]::GetFullPath((Join-Path $repo 'target\scratch'))
  if (-not $full.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) {
    Log ("ABORT: refusing to clean {0} -- outside {1}" -f $full, $root)
    Write-Error 'work dir is outside target/scratch'
    exit 2
  }
  Remove-Item -Recurse -Force $full
  Log ("cleaned stale work dir {0}" -f $full)
}
New-Item -ItemType Directory -Force -Path $work | Out-Null
foreach ($entry in Get-ChildItem -Path $repo -Force) {
  if ($Exclude -contains $entry.Name) { continue }
  $dst = Join-Path $work $entry.Name
  if ($entry.PSIsContainer) {
    New-Item -ItemType Directory -Force -Path $dst | Out-Null
    Copy-Item -Path (Join-Path $entry.FullName '*') -Destination $dst -Recurse -Force
  } else {
    Copy-Item -Path $entry.FullName -Destination $dst -Force
  }
}
foreach ($need in @('Cargo.toml', 'src\lib.rs', 'src\sema.rs', 'conformance\cases', 'docs\coverage.md')) {
  if (-not (Test-Path (Join-Path $work $need))) {
    Log ("ABORT: the copy is missing {0} -- the suite cannot be run there" -f $need)
    Write-Error ("incomplete copy: {0}" -f $need)
    exit 2
  }
}
Log ("copy ready at {0} ({1} top-level entries)" -f $work, (Get-ChildItem -Path $work -Force).Count)

$rc_all = 0
foreach ($m in $selected) {
  # 1. the tree is already in place; restore every file a mutant can touch, so no mutant leaks into
  #    the next one even when -Only reorders them
  foreach ($f in @('src\sema.rs', 'src\hir.rs')) {
    Copy-Item -Path (Join-Path $repo $f) -Destination (Join-Path $work $f) -Force
  }

  # 2. the mutation itself: an anchor that is missing is a hard error, never a silent skip
  if ($m.file -ne '') {
    $path = Join-Path $work $m.file
    $bytes = [IO.File]::ReadAllBytes($path)
    $text = $enc.GetString($bytes)
    $hits = ([regex]::Matches($text, [regex]::Escape($m.old))).Count
    if ($hits -lt 1) {
      Log ("ABORT {0}: anchor not found in {1} -- the mutation no longer matches the source" -f $m.name, $m.file)
      Log ("{0}: anchor was: {1}" -f $m.name, $m.old)
      Write-Host ("{0}: ANCHOR MISSING in {1}" -f $m.name, $m.file)
      $rc_all = 1
      continue
    }
    [IO.File]::WriteAllBytes($path, $enc.GetBytes($text.Replace($m.old, $m.new)))
    Log ("{0}: anchor replaced at {1} site(s) in {2}" -f $m.name, $hits, $m.file)
  }

  # 3. build + run the suite in the copy
  $r = Invoke-Native 'cargo' @('test', '--offline', '--lib', '-j', '1') $work
  $lines = @($r.text -split "`r?`n")
  $resultLine = ($lines | Where-Object { $_ -like 'test result:*' } | Select-Object -First 1)
  $red = @($lines | Where-Object { $_ -like '    sema::tests::*' -or $_ -like '    hir::tests::*' })
  $pass = 0; $fail = 0
  if ($resultLine -match '(\d+) passed') { $pass = [int]$Matches[1] }
  if ($resultLine -match '(\d+) failed') { $fail = [int]$Matches[1] }
  Log ("{0}: rc={1} result=`"{2}`"" -f $m.name, $r.rc, $resultLine)
  foreach ($rl in $red) { Log ("{0}: red {1}" -f $m.name, $rl.Trim()) }
  Write-Host ("{0}: cargo rc={1} passed={2} failed={3}" -f $m.name, $r.rc, $pass, $fail)
  foreach ($rl in $red) { Write-Host ("    {0}" -f $rl.Trim()) }

  # 4. the verdict: control must stay green, a mutant must turn its own assertion red
  if ($m.name -eq 'control') {
    if ($r.rc -ne 0 -or $fail -ne 0) {
      Log 'control: the copy did not reproduce a green suite'
      Write-Host '  FAIL: control copy is not green'
      $rc_all = 1
    }
  } else {
    if ($fail -eq 0) {
      Log ("{0}: NO TEETH -- reverting this decision left the suite green" -f $m.name)
      Write-Host '  FAIL: mutant did not go red'
      $rc_all = 1
    }
    foreach ($want in $m.expect) {
      $hit = @($red | Where-Object { $_ -like "*$want*" }).Count
      if ($hit -lt 1) {
        Log ("{0}: expected {1} to go red, it did not" -f $m.name, $want)
        Write-Host "  FAIL: expected red test $want is missing"
        $rc_all = 1
      }
    }
  }
}

Log "=== probe done rc_all=$rc_all ==="
if ($rc_all -ne 0) { Write-Host 'probe: at least one assertion has no teeth (see logs/f11-mutation-probe.log)'; exit 1 }
Write-Host 'probe: control green, every mutant red on its own assertion'
exit 0
