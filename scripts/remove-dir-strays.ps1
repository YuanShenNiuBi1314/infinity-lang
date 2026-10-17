<#
.SYNOPSIS
  Remove untracked one-off directories that sit directly in the repository root. Dry-run by
  default; -Apply is what actually deletes.
.DESCRIPTION
  Why this exists as a script instead of a bare rm: BACKLOG 0.0 says every destructive step has to
  run through a script that (1) dry-runs by default, (2) is idempotent, (3) appends its actions to
  logs/, (4) never leaves machine state changed. This round hit it for real: `infy init <path>`
  ignores the directory part of the argument and creates the module next to the current directory
  (registered as BACKLOG F50), which left disc2/ and disc3/ in the repo root.
  Comments and messages are ASCII on purpose: this file runs under Windows PowerShell 5.1, which
  reads a BOM-less file as ANSI(GBK), so non-ASCII text here would be mojibake (see the
  "PowerShell 5.1" entry in the lessons index).
.PARAMETER Name
  Directory names to remove, relative to the repository root. Required. `powershell.exe -File`
  binds "-Name a,b" as one string, so commas/semicolons are split here rather than by the caller.
.PARAMETER Apply
  Actually delete. Without it the script only prints what it would do and exits 0.
.EXAMPLE
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/remove-dir-strays.ps1 -Name disc2,disc3
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/remove-dir-strays.ps1 -Name disc2,disc3 -Apply
#>
[CmdletBinding()]
param(
  [string]$Name = '',
  [switch]$Apply
)

$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$logDir = Join-Path $repo 'logs'
if (-not (Test-Path $logDir)) { New-Item -ItemType Directory -Path $logDir | Out-Null }
$log = Join-Path $logDir 'remove-dir-strays.log'
function Log($m) { "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')  $m" | Add-Content -Path $log -Encoding utf8 }

# Anything that is a real part of the project, whether tracked or not, is off limits.
$protected = @('src','runtime','docs','spec','examples','conformance','scripts','logs','target','.git','.github','tests')

function Run-Native {
  param([string]$Exe, [string[]]$Args_)
  $prev = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  $out = & $Exe @Args_ 2>&1 | Out-String
  $rc  = $LASTEXITCODE
  $ErrorActionPreference = $prev
  return @{ rc = $rc; text = $out }
}

$names = @($Name -split '[,;]' | ForEach-Object { $_.Trim() } | Where-Object { $_ })
if ($names.Count -eq 0) { Log 'REFUSED: no -Name given'; Write-Error 'nothing to do: -Name is required'; exit 2 }

$failed = $false
$removed = 0
foreach ($n in $names) {
  $leaf = $n.TrimEnd('\','/')
  if ($leaf -eq '' -or $leaf -eq '.' -or $leaf -eq '..' -or $leaf -match '[\\/]') {
    Log "REFUSED: '$n' is not a bare directory name under the repo root"; $failed = $true; continue
  }
  if ($leaf -in $protected) { Log "REFUSED: '$leaf' is a protected project directory"; $failed = $true; continue }
  $full = [IO.Path]::GetFullPath((Join-Path $repo $leaf))
  $rootWithSep = $repo.TrimEnd('\','/') + [IO.Path]::DirectorySeparatorChar
  if (-not $full.StartsWith($rootWithSep)) { Log "REFUSED: '$full' resolves outside the repo"; $failed = $true; continue }
  if (-not (Test-Path -LiteralPath $full -PathType Container)) {
    Log "NOOP: '$leaf' is not a directory here (already gone?)"; continue
  }
  # Tracked content must never be deleted by this script: that is a git operation, not a cleanup.
  $tracked = Run-Native 'git' @('ls-files','--',"$leaf/")
  if ($tracked.rc -ne 0) { Log "REFUSED: git ls-files failed for '$leaf': $($tracked.text.Trim())"; $failed = $true; continue }
  if ($tracked.text.Trim()) { Log "REFUSED: '$leaf' holds tracked files, refusing to delete"; $failed = $true; continue }

  $files = @(Get-ChildItem -LiteralPath $full -Recurse -File -ErrorAction SilentlyContinue)
  $bytes = ($files | Measure-Object -Property Length -Sum).Sum
  if (-not $Apply) {
    Log "DRYRUN would remove '$leaf' ($($files.Count) file(s), $bytes byte(s))"
    Write-Host "[dry-run] would remove $leaf ($($files.Count) file(s), $bytes byte(s))"
    continue
  }
  $list = ($files | ForEach-Object { $_.FullName.Substring($repo.Length + 1) }) -join ', '
  try {
    Remove-Item -LiteralPath $full -Recurse -Force
  } catch {
    Log "FAIL: Remove-Item '$leaf' threw: $_"; $failed = $true; continue
  }
  if (Test-Path -LiteralPath $full) { Log "FAIL: '$leaf' still exists after Remove-Item"; $failed = $true; continue }
  Log "APPLIED removed '$leaf' ($($files.Count) file(s), $bytes byte(s)): $list"
  Write-Host "removed $leaf ($($files.Count) file(s), $bytes byte(s))"
  $removed++
}

if ($failed) { Log "RESULT exit 1 (removed=$removed)"; exit 1 }
Log "RESULT exit 0 apply=$([bool]$Apply) removed=$removed names=$($names -join ',')"
exit 0
