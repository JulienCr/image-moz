# Releasing a tab outside every tab bar opens it in a new window under the cursor, as large as the source.
param([string]$Fix, [string]$Name)
$sb = New-AppEnv $Name
$app = Start-Session $sb @(New-WindowSpec -Fix $Fix -Tabs alpha, bravo, charlie -Bounds (New-Bounds 100 80))
$src = @(Get-AppWindows $app)[0]
Wait-TabNames $src 'alpha,bravo,charlie'
Focus-Window $src

$t = @(Get-Tabs $src)
$cx = [int]($src.L + $src.Width / 2); $cy = [int]($src.T + $src.Height * 0.6)
Drag-Mouse $t[2].CX $t[2].CY $cx $cy

$wins = Wait-AppWindows $app 2
$new = $wins | Where-Object { $_.Hwnd -ne $src.Hwnd } | Select-Object -First 1
Assert-Equal (Get-TabNames $new) 'charlie' 'tabs of the new window'
Assert-Equal (Get-TabNames (Get-WindowByHwnd $app $src.Hwnd)) 'alpha,bravo' 'tabs left in the source'

$s = Get-Scale $new
Assert-That ([math]::Abs(($cx - $new.L) - 80 * $s) -le 30 * $s) "cursor x is not over the first tab of the new window (offset $($cx - $new.L))"
Assert-That ([math]::Abs(($cy - $new.T) - 46 * $s) -le 30 * $s) "cursor y is not over the tab bar of the new window (offset $($cy - $new.T))"
Assert-That ([math]::Abs($new.Width - $src.Width) -le 3 -and [math]::Abs($new.Height - $src.Height) -le 3) "size $($new.Width)x$($new.Height) differs from the source $($src.Width)x$($src.Height)"
Assert-That (Test-AppAlive $app) 'the app is still running'

$c = Wait-SavedConfig $sb { param($c) @($c.session.windows).Count -eq 2 } 'both windows saved'
Assert-Equal (@($c.session.windows[1].tabs) | ForEach-Object { Split-Path $_ -Leaf }) 'charlie' 'saved tabs of the second window'
