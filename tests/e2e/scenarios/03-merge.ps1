# Releasing a tab on another window's bar moves it there at the cursor's slot, activates it and
# focuses that window; the emptied window closes.
param([string]$Fix, [string]$Name)
$sb = New-AppEnv $Name
$app = Start-Session $sb @(
  (New-WindowSpec -Fix $Fix -Tabs alpha, bravo -Bounds (New-Bounds 40 10 900 360)),
  (New-WindowSpec -Fix $Fix -Tabs charlie -Bounds (New-Bounds 40 430 900 360))
)
$a = Find-Window $app 'alpha'
$b = Find-Window $app 'charlie'
Wait-TabNames $a 'alpha,bravo'
Wait-TabNames $b 'charlie'
Focus-Window $b

$from = @(Get-Tabs $b)[0]
$into = @(Get-Tabs $a)
Drag-Mouse $from.CX $from.CY (($into[0].Right + $into[1].Left) / 2) $into[0].CY

[void](Wait-AppWindows $app 1)
$a = @(Get-AppWindows $app)[0]
Wait-TabNames $a 'alpha,charlie,bravo'
Assert-Equal (Get-ActiveFolder $a) 'charlie' 'the moved tab is active'
[void](Wait-Until { Test-Foreground $a } 4000 'the target window to be in front')
Assert-That (Test-AppAlive $app) 'the app is still running'
[void](Wait-SavedConfig $sb { param($c) @($c.session.windows).Count -eq 1 } 'single window saved')
