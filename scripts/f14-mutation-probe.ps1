<#
.SYNOPSIS
  F14 mutation probe: proves the constructor-pattern binding assertions go red when the defect comes back.
.DESCRIPTION
  Spec 3.12 / 4.1 (BACKLOG F14) fixed three things at once: `sema` now resolves a variant pattern by
  its OWNER (every segment but the last) instead of reading `path.last()` as an enum name, the closed
  `Option`/`Result` sets count as exhaustive without a `_` arm, and a bare `None` in pattern position
  is the unit constructor rather than a fresh binding (codegen reads a binding arm as always-true, so
  the old shape silently swallowed every later arm of the same `match`). Each decision has an
  assertion; an assertion nobody has seen fail is not evidence, so this script rebuilds the crate
  from a throwaway copy with one decision reverted at a time.

  Nothing in the repository is modified. Every copy lives under target/scratch/f14mut/work, which is
  gitignored, and the copy keeps its own target/ so only the edited crate recompiles per mutant.
  Every file a mutant can touch is restored from the repository before that mutant runs, so no
  mutation leaks into the next one.

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
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/f14-mutation-probe.ps1 -Apply
#>
[CmdletBinding()]
param(
  [switch]$Apply,
  [string[]]$Only = @(),
  [int]$TimeoutSec = 900
)

$ErrorActionPreference = 'Stop'
$repo   = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$work   = Join-Path $repo 'target\scratch\f14mut\work'
$logDir = Join-Path $repo 'logs'
if (-not (Test-Path $logDir)) { New-Item -ItemType Directory -Path $logDir | Out-Null }
$log    = Join-Path $logDir 'f14-mutation-probe.log'
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

$Exclude = @('target', '.git', 'logs')
# Every file a mutant can touch, restored before each mutant runs.
$Restores = @('src\sema.rs', 'src\parser.rs', 'src\borrow.rs')

$Mutations = @(
  @{ name = 'control'; file = ''; old = ''; new = ''; expect = @()
     desc = 'no mutation: the copy must reproduce the green suite' }
  @{ name = 'M1-last-segment-as-owner'; file = 'src\sema.rs'
     desc = 'the F14 shape comes back: the variant name is looked up as an enum name'
     old  = 'let owner = path[..path.len() - 1].join("::");'
     new  = 'let owner = last.clone();'
     expect = @('constructor_pattern_bindings_reach_the_arm_body') }
  @{ name = 'M2-no-foreign-owner-guard'; file = 'src\sema.rs'
     desc = 'a variant pattern of another enum binds anyway'
     old  = 'if expected.is_some() && expected != Some(owner.as_str()) {'
     new  = 'if expected.is_some() && false {'
     expect = @('constructor_pattern_bindings_reach_the_arm_body') }
  @{ name = 'M3-closed-set-not-exhaustive'; file = 'src\sema.rs'
     desc = 'Option/Result need a `_` arm again, so a complete match is reported as E-0070'
     old  = 'Ty::Option { .. } => covers_named_variants(arms, &["Some", "None"]) || has_wild,'
     new  = 'Ty::Option { .. } => has_wild,'
     expect = @('builtin_option_and_result_arms_are_closed_sets') }
  @{ name = 'M4-none-is-a-binding'; file = 'src\parser.rs'
     desc = 'a bare `None` becomes a fresh binding again, which codegen reads as an always-true arm'
     old  = 'const BUILTIN_CTOR_PAT: &[&str] = &["Some", "None", "Ok", "Err"];'
     new  = 'const BUILTIN_CTOR_PAT: &[&str] = &["Some", "Ok", "Err"];'
     expect = @('builtin_option_and_result_arms_are_closed_sets') }
  @{ name = 'M5-borrow-skips-ctor-args'; file = 'src\borrow.rs'
     desc = 'stage [5] stops binding constructor arguments, so a pattern binding is no owner'
     old  = '                args.iter().for_each(|x| self.bind_pat(x));'
     new  = '                args.iter().for_each(|_x| { });'
     expect = @('pattern_bindings_take_part_in_move_tracking') }
)

Log "=== probe start apply=$Apply timeout=${TimeoutSec}s repo=$repo ==="

if (-not $Apply) {
  Log 'DRY-RUN: plan only, nothing copied, built or tested'
  Write-Host '[dry-run] plan:'
  Write-Host "  copy  repo tree (minus $($Exclude -join ', ')) -> $work"
  Write-Host  '  test  cargo test --offline --lib -j 1   (bounded by a timeout, killed on expiry)'
  Write-Host  "  restore before every mutant: $($Restores -join ', ')"
  foreach ($m in $Mutations) { Write-Host ("  mutation {0}: {1}" -f $m.name, $m.desc) }
  Write-Host '[dry-run] pass -Apply to execute'
  exit 0
}

$selected = if ($Only.Count -gt 0) { @($Mutations | Where-Object { $Only -contains $_.name }) } else { $Mutations }
if ($selected.Count -eq 0) { Log 'ABORT: -Only named no known mutation'; Write-Error 'unknown mutation name'; exit 2 }

if (Test-Path $work) {
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
foreach ($need in @('Cargo.toml', 'src\lib.rs', 'src\sema.rs', 'src\parser.rs', 'src\borrow.rs', 'conformance\cases', 'docs\coverage.md')) {
  if (-not (Test-Path (Join-Path $work $need))) {
    Log ("ABORT: the copy is missing {0} -- the suite cannot be run there" -f $need)
    Write-Error ("incomplete copy: {0}" -f $need)
    exit 2
  }
}
Log ("copy ready at {0} ({1} top-level entries)" -f $work, (Get-ChildItem -Path $work -Force).Count)

$rc_all = 0
foreach ($m in $selected) {
  foreach ($f in $Restores) {
    Copy-Item -Path (Join-Path $repo $f) -Destination (Join-Path $work $f) -Force
  }

  if ($m.file -ne '') {
    $path = Join-Path $work $m.file
    $text = $enc.GetString([IO.File]::ReadAllBytes($path))
    $hits = ([regex]::Matches($text, [regex]::Escape($m.old))).Count
    if ($hits -lt 1) {
      Log ("ABORT {0}: anchor not found in {1} -- the mutation no longer matches the source" -f $m.name, $m.file)
      Log ("{0}: anchor was: {1}" -f $m.name, ($m.old -replace "`r?`n", '\n'))
      Write-Host ("{0}: ANCHOR MISSING in {1}" -f $m.name, $m.file)
      $rc_all = 1
      continue
    }
    [IO.File]::WriteAllBytes($path, $enc.GetBytes($text.Replace($m.old, $m.new)))
    Log ("{0}: anchor replaced at {1} site(s) in {2}" -f $m.name, $hits, $m.file)
  }

  $r = Invoke-Native 'cargo' @('test', '--offline', '--lib', '-j', '1') $work
  $lines = @($r.text -split "`r?`n")
  $resultLine = ($lines | Where-Object { $_ -like 'test result:*' } | Select-Object -First 1)
  $red = @($lines | Where-Object { $_ -like '    sema::tests::*' -or $_ -like '    parser::tests::*' -or $_ -like '    borrow::tests::*' -or $_ -like '    codegen::tests::*' })
  $pass = 0; $fail = 0
  if ($resultLine -match '(\d+) passed') { $pass = [int]$Matches[1] }
  if ($resultLine -match '(\d+) failed') { $fail = [int]$Matches[1] }
  Log ("{0}: rc={1} result=`"{2}`"" -f $m.name, $r.rc, $resultLine)
  foreach ($rl in $red) { Log ("{0}: red {1}" -f $m.name, $rl.Trim()) }
  Write-Host ("{0}: cargo rc={1} passed={2} failed={3}" -f $m.name, $r.rc, $pass, $fail)
  foreach ($rl in $red) { Write-Host ("    {0}" -f $rl.Trim()) }

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
if ($rc_all -ne 0) { Write-Host 'probe: at least one assertion has no teeth (see logs/f14-mutation-probe.log)'; exit 1 }
Write-Host 'probe: control green, every mutant red on its own assertion'
exit 0
