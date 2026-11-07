# A second launch hands its path to the running instance: it opens in the window used last, which
# comes forward. A folder already open anywhere is shown there instead of opened twice.
param([string]$Fix, [string]$Name)
$sb = New-AppEnv $Name
$app = Start-Session $sb @(
  (New-WindowSpec -Fix $Fix -Tabs alpha -Bounds (New-Bounds 40 10 900 360)),
  (New-WindowSpec -Fix $Fix -Tabs bravo -Bounds (New-Bounds 40 430 900 360))
)
$a = Find-Window $app 'alpha'
$b = Find-Window $app 'bravo'

# Folder, with the second window last used.
Focus-Window $b
Send-Path $sb (Join-Path $Fix 'echo')
Wait-TabNames (Get-WindowByHwnd $app $b.Hwnd) 'bravo,echo'
Assert-Equal (Get-TabNames (Get-WindowByHwnd $app $a.Hwnd)) 'alpha' 'the other window is untouched'
Assert-That (Wait-Until { Test-Foreground (Get-WindowByHwnd $app $b.Hwnd) } 4000 'window B in front') 'window B is in front'

# Image file: its folder opens, in the window used last (now the first one).
Focus-Window (Get-WindowByHwnd $app $a.Hwnd)
Send-Path $sb (Join-Path $Fix 'charlie\charlie_02.png')
Wait-TabNames (Get-WindowByHwnd $app $a.Hwnd) 'alpha,charlie'
Assert-Equal (Get-ActiveFolder (Get-WindowByHwnd $app $a.Hwnd)) 'charlie' 'the new tab is active'

# A folder open in the other window: no new tab, that window comes forward.
Send-Path $sb (Join-Path $Fix 'echo')
$b2 = Get-WindowByHwnd $app $b.Hwnd
Wait-TabNames $b2 'bravo,echo'
[void](Wait-Until { Test-Foreground (Get-WindowByHwnd $app $b.Hwnd) } 4000 'the window holding echo to come forward')
Assert-Equal (Get-ActiveFolder (Get-WindowByHwnd $app $b.Hwnd)) 'echo' 'echo became the active tab there'
Assert-Equal (Get-TabNames (Get-WindowByHwnd $app $a.Hwnd)) 'alpha,charlie' 'no duplicate tab in the first window'
Assert-Equal @(Get-AppWindows $app).Count 2 'window count'
