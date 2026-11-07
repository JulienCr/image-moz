# Closing a secondary window from its title bar closes its tabs and nothing else.
param([string]$Fix, [string]$Name)
$sb = New-AppEnv $Name
$app = Start-Session $sb @(
  (New-WindowSpec -Fix $Fix -Tabs alpha, bravo -Bounds (New-Bounds 40 10 900 360)),
  (New-WindowSpec -Fix $Fix -Tabs charlie -Bounds (New-Bounds 40 430 900 360))
)
$b = Find-Window $app 'charlie'
Close-Window $b

$left = @(Wait-AppWindows $app 1)
Assert-Equal (Get-TabNames $left[0]) 'alpha,bravo' 'tabs of the remaining window'
Assert-That (Test-AppAlive $app) 'the app is still running'
$c = Wait-SavedConfig $sb { param($c) @($c.session.windows).Count -eq 1 } 'closed window dropped from the session'
Assert-Equal (@($c.session.windows[0].tabs) | ForEach-Object { Split-Path $_ -Leaf }) 'alpha,bravo' 'saved tabs'
