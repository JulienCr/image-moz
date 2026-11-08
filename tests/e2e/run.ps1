# Builds the debug exe, then runs the end-to-end scenarios against it.
#   pwsh tests\e2e\run.ps1                    all scenarios
#   pwsh tests\e2e\run.ps1 reorder detach     only these (file names in scenarios\, without .ps1)
#   pwsh tests\e2e\run.ps1 -List              list them
#   pwsh tests\e2e\run.ps1 -NoBuild           reuse target\debug\image-moz.exe
# Moves the real mouse and types on the real keyboard: leave the machine alone while it runs.
param(
  [Parameter(ValueFromRemainingArguments = $true)][string[]]$Scenario,
  [switch]$NoBuild,
  [switch]$List
)

$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$repo = (Resolve-Path (Join-Path $here '..\..')).Path
$all = Get-ChildItem (Join-Path $here 'scenarios') -Filter '*.ps1' | Sort-Object Name
function Get-ShortName($File) { $File.BaseName -replace '^\d+-', '' }
if ($List) { $all | ForEach-Object { Get-ShortName $_ }; exit 0 }
if ($Scenario) {
  $unknown = $Scenario | Where-Object { $_ -notin ($all | ForEach-Object { Get-ShortName $_ }) }
  if ($unknown) { Write-Host "Unknown scenario(s): $($unknown -join ', '). Try -List."; exit 2 }
  $all = $all | Where-Object { (Get-ShortName $_) -in $Scenario }
}

. (Join-Path $here 'lib.ps1')

if (-not $NoBuild) {
  Push-Location $repo
  try {
    cargo build
    if ($LASTEXITCODE -ne 0) { Write-Host 'BUILD FAILED'; exit 2 }
  } finally { Pop-Location }
}
$exe = Join-Path $repo 'target\debug\image-moz.exe'
if (-not (Test-Path $exe)) { Write-Host "Missing $exe"; exit 2 }

$out = Join-Path $here 'out'
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
$work = Join-Path ([IO.Path]::GetTempPath()) ('image-moz-e2e-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
Initialize-E2E -Exe $exe -OutDir $out -Work $work

$realConfig = Join-Path $env:APPDATA 'image-moz\config.json'
function Get-RealConfigHash { if (Test-Path $realConfig) { (Get-FileHash $realConfig).Hash } else { 'absent' } }
$realBefore = Get-RealConfigHash

$fixtures = Join-Path $work 'fixtures'
New-Fixtures $fixtures

$results = @()
try {
  foreach ($file in $all) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $status = 'PASS'; $detail = ''
    try {
      & $file.FullName -Fix $fixtures -Name $file.BaseName
    } catch {
      $detail = $_.Exception.Message
      if ($detail -match '^SKIP: ') {
        $status = 'SKIP'
      } else {
        $status = 'FAIL'
        Save-Shot "$($file.BaseName)-failure"
      }
    } finally {
      Stop-AllApps
    }
    $results += [pscustomobject]@{ Name = $file.BaseName; Status = $status; Seconds = [math]::Round($sw.Elapsed.TotalSeconds, 1); Detail = $detail }
    $line = '{0,-4} {1,-22} {2,5}s' -f $status, $file.BaseName, $results[-1].Seconds
    if ($status -eq 'FAIL') { Write-Host $line -ForegroundColor Red; Write-Host "     $detail" -ForegroundColor Red }
    elseif ($status -eq 'SKIP') { Write-Host $line -ForegroundColor Yellow; Write-Host "     $detail" -ForegroundColor Yellow }
    else { Write-Host $line -ForegroundColor Green }
  }
} finally {
  Stop-AllApps
  Start-Sleep -Milliseconds 300
  Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

$realAfter = Get-RealConfigHash
$isolated = $realBefore -eq $realAfter
Write-Host ('{0,-4} {1}' -f ($(if ($isolated) { 'PASS' } else { 'FAIL' })), 'real config untouched') -ForegroundColor $(if ($isolated) { 'Green' } else { 'Red' })

$failed = @($results | Where-Object Status -eq 'FAIL').Count + $(if ($isolated) { 0 } else { 1 })
Write-Host ("`n{0} passed, {1} failed, {2} skipped. Logs and failure screenshots: {3}" -f (@($results | Where-Object Status -eq 'PASS').Count), $failed, @($results | Where-Object Status -eq 'SKIP').Count, $out)
exit $(if ($failed -gt 0) { 1 } else { 0 })
