# A background tab keeps watching its folder and shows how many images arrived; showing it clears the badge.
param([string]$Fix, [string]$Name)
$sb = New-AppEnv $Name
$watched = Join-Path $sb.Dir 'bravo'
New-Item -ItemType Directory -Force $watched | Out-Null
Copy-Item (Join-Path $Fix 'bravo\*') $watched

$app = Start-Session $sb @(New-WindowSpec -Fix $Fix -Tabs alpha, $watched -Bounds (New-Bounds 100 80))
$win = @(Get-AppWindows $app)[0]
Wait-TabNames $win 'alpha,bravo'

New-TestImage (Join-Path $watched 'arrived_1.png') 120 90 '#101010'
New-TestImage (Join-Path $watched 'arrived_2.png') 120 90 '#202020'
Wait-TabNames $win 'alpha,bravo (2)' 12000

Click-Tab $win 'bravo (2)'
Wait-TabNames $win 'alpha,bravo'
Assert-Equal (Get-ActiveFolder (Get-WindowByHwnd $app $win.Hwnd)) 'bravo' 'the tab is shown'
