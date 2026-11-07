# Dragging a tab inside its own bar reorders it, and the new order is saved.
param([string]$Fix, [string]$Name)
$sb = New-AppEnv $Name
$app = Start-Session $sb @(New-WindowSpec -Fix $Fix -Tabs alpha, bravo, charlie -Bounds (New-Bounds 100 80))
$win = @(Get-AppWindows $app)[0]
Wait-TabNames $win 'alpha,bravo,charlie'
Focus-Window $win

# Release past the last tab: the dragged tab goes to the end.
$t = @(Get-Tabs $win)
Drag-Mouse $t[0].CX $t[0].CY ($t[2].Right + 8) $t[2].CY
Wait-TabNames $win 'bravo,charlie,alpha'
Assert-Equal (Get-ActiveFolder (Get-WindowByHwnd $app $win.Hwnd)) 'alpha' 'the dragged tab is the active one'

# Release between the first two tabs: the last one comes back to slot 1.
$t = @(Get-Tabs $win)
Drag-Mouse $t[2].CX $t[2].CY (($t[0].Right + $t[1].Left) / 2) $t[0].CY
Wait-TabNames $win 'bravo,alpha,charlie'

Assert-Equal @(Get-AppWindows $app).Count 1 'window count'
[void](Wait-SavedConfig $sb { param($c) (@($c.session.windows[0].tabs | ForEach-Object { Split-Path $_ -Leaf }) -join ',') -eq 'bravo,alpha,charlie' } 'new order saved')
[void](Wait-Log $app 'tab drag released')
