# A window holding one tab does not detach it (it would only recreate itself).
param([string]$Fix, [string]$Name)
$sb = New-AppEnv $Name
$app = Start-Session $sb @(
  (New-WindowSpec -Fix $Fix -Tabs alpha, bravo -Bounds (New-Bounds 40 10 900 360)),
  (New-WindowSpec -Fix $Fix -Tabs charlie -Bounds (New-Bounds 40 430 900 360))
)
$a = Find-Window $app 'alpha'
$b = Find-Window $app 'charlie'
Wait-TabNames $b 'charlie'
Focus-Window $b

$tab = @(Get-Tabs $b)[0]
Drag-Mouse $tab.CX $tab.CY ($b.L + $b.Width / 2) ($b.T + $b.Height * 0.7)

$now = @(Get-AppWindows $app)
Assert-Equal $now.Count 2 'window count'
$after = Get-WindowByHwnd $app $b.Hwnd
Assert-Equal @($after.L, $after.T, $after.Width, $after.Height) @($b.L, $b.T, $b.Width, $b.Height) 'the window did not move or resize'
Assert-Equal (Get-TabNames $after) 'charlie' 'tabs'
Assert-That (Test-AppAlive $app) 'the app is still running'
