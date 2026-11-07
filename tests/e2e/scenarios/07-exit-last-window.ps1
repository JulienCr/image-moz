# The app exits when its last window closes, and the session is saved.
param([string]$Fix, [string]$Name)
$sb = New-AppEnv $Name
$app = Start-Session $sb @(New-WindowSpec -Fix $Fix -Tabs alpha, bravo -Bounds (New-Bounds 100 80))
$win = @(Get-AppWindows $app)[0]
Close-Window $win

Assert-That ($app.Process.WaitForExit(8000)) 'the process did not exit after its only window was closed'
[void](Wait-Log $app 'exiting with 1 window')
$c = Get-SavedConfig $sb
Assert-Equal (@($c.session.windows[0].tabs) | ForEach-Object { Split-Path $_ -Leaf }) 'alpha,bravo' 'saved tabs'
Assert-That ($null -ne $c.session.windows[0].bounds) 'window bounds were saved'
