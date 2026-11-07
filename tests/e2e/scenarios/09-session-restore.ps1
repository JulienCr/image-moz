# Every stored window comes back with its tabs and bounds. A vanished folder is skipped, a window left
# empty is dropped, one stored off-screen lands on a monitor, and the last active window is restored.
param([string]$Fix, [string]$Name)
$sb = New-AppEnv $Name
$gone = Join-Path $Fix 'does-not-exist'
$b1 = New-Bounds 40 10 900 360
$b2 = New-Bounds 40 430 800 360
$specs = @(
  (New-WindowSpec -Fix $Fix -Tabs alpha, $gone, bravo -Bounds $b1 -Active 2),
  (New-WindowSpec -Fix $Fix -Tabs charlie -Bounds $b2),
  (New-WindowSpec -Fix $Fix -Tabs delta -Bounds (New-OffscreenBounds)),
  (New-WindowSpec -Fix $Fix -Tabs $gone -Bounds (New-Bounds 10 10))
)
$app = Start-Session $sb $specs 1 3

$wins = @(Get-AppWindows $app)
Assert-Equal $wins.Count 3 'the window left empty was dropped'
$w1 = Find-Window $app 'bravo'
Assert-Equal (Get-TabNames $w1) 'alpha,bravo' 'first window: vanished folder skipped, active tab kept'
$w2 = Find-Window $app 'charlie'
$w3 = Find-Window $app 'delta'

foreach ($pair in @(@($w1, $b1), @($w2, $b2))) {
  $w = $pair[0]; $b = $pair[1]
  Assert-That ([math]::Abs($w.L - $b.x) -le 3 -and [math]::Abs($w.T - $b.y) -le 3) "window at $($w.L),$($w.T), stored at $($b.x),$($b.y)"
}

$onScreen = [System.Windows.Forms.Screen]::AllScreens | Where-Object {
  $r = $_.WorkingArea
  $w3.L -lt $r.Right -and $w3.R -gt $r.Left -and $w3.T -lt $r.Bottom -and $w3.B -gt $r.Top
}
Assert-That ($null -ne $onScreen) "the window stored off-screen is at $($w3.L),$($w3.T), on no monitor"

# The stored active window is the one a forwarded path opens in.
Send-Path $sb (Join-Path $Fix 'echo')
Wait-TabNames (Get-WindowByHwnd $app $w2.Hwnd) 'charlie,echo'
