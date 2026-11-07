# Builds the release exe, then the Inno Setup installer into installer/Output/.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

$cargoToml = Get-Content -Raw (Join-Path $root 'Cargo.toml')
if ($cargoToml -notmatch '(?m)^version\s*=\s*"([^"]+)"') { throw 'version not found in Cargo.toml' }
$version = $Matches[1]

$iscc = Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'
if (-not (Test-Path $iscc)) { throw "ISCC.exe not found: $iscc" }

Push-Location $root
try {
    & cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }

    & $iscc "/DAppVersion=$version" (Join-Path $PSScriptRoot 'image-moz.iss')
    if ($LASTEXITCODE -ne 0) { throw "ISCC failed ($LASTEXITCODE)" }
} finally {
    Pop-Location
}

$setup = Join-Path $PSScriptRoot "Output\image-moz-setup-$version.exe"
if (-not (Test-Path $setup)) { throw "installer not produced: $setup" }
Write-Host "OK: $setup ($((Get-Item $setup).Length) bytes)"
