# A folder on a WSL volume (\\wsl.localhost\...) refreshes when files change from the Linux side: the OS
# delivers no change notifications there, so the app polls. Skipped when WSL or the Ubuntu distro is absent.
param([string]$Fix, [string]$Name)

$distro = 'Ubuntu'
$skip = $null
if (-not (Get-Command wsl.exe -ErrorAction SilentlyContinue)) { $skip = 'wsl.exe is not installed' }
else {
  & wsl.exe -d $distro -- true 2>&1 | Out-Null
  if ($LASTEXITCODE -ne 0) { $skip = "WSL distro '$distro' is not available" }
}
if ($skip) { throw "SKIP: $skip" }

function Invoke-Linux([string]$Command) {
  & wsl.exe -d $distro -- sh -c $Command 2>&1 | Out-Null
  Assert-Equal $LASTEXITCODE 0 "wsl command: $Command"
}

# Writes a PNG into the Linux folder from the Linux side, through base64 on stdin.
function Write-LinuxImage([string]$LinuxPath, [int]$Width, [string]$Hex) {
  $tmp = Join-Path $global:E2E_Work "$Name-$Width.png"
  New-TestImage $tmp $Width 60 $Hex
  $b64 = [Convert]::ToBase64String([IO.File]::ReadAllBytes($tmp))
  $b64 | & wsl.exe -d $distro -- sh -c "tr -d '\r' | base64 -d > '$LinuxPath'"
  Assert-Equal $LASTEXITCODE 0 "writing $LinuxPath"
}

function Get-LogCount($App, [string]$Pattern) { [regex]::Matches((Get-AppLog $App), $Pattern).Count }

$leaf = "image-moz-e2e-$([guid]::NewGuid().ToString('N').Substring(0, 8))"
$linuxDir = "/tmp/$leaf"
$unc = "\\wsl.localhost\$distro\tmp\$leaf"
Invoke-Linux "mkdir -p '$linuxDir'"
try {
  $sb = New-AppEnv $Name
  $app = Start-Session $sb @(New-WindowSpec -Fix $Fix -Tabs alpha, $unc -Bounds (New-Bounds 100 80))
  $win = @(Get-AppWindows $app)[0]
  Wait-TabNames $win "alpha,$leaf"

  Write-LinuxImage "$linuxDir/one.png" 100 '#3366CC'
  Wait-TabNames $win "alpha,$leaf (1)" 15000

  $before = Get-LogCount $app 'fs change: Upsert\(.*one\.png'
  Write-LinuxImage "$linuxDir/one.png" 180 '#CC6633'
  [void](Wait-Until { (Get-LogCount $app 'fs change: Upsert\(.*one\.png') -gt $before } 15000 'the rewritten image to be reported')

  Invoke-Linux "rm '$linuxDir/one.png'"
  [void](Wait-Until { (Get-LogCount $app 'fs change: Remove\(.*one\.png') -ge 1 } 15000 'the deleted image to be reported')
} finally {
  & wsl.exe -d $distro -- rm -rf $linuxDir 2>&1 | Out-Null
}
