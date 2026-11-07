# Closing the first (root) window while another is open keeps the app alive: the other window's
# tabs take the root's place, at the other window's position.
param([string]$Fix, [string]$Name)
$sb = New-AppEnv $Name
$app = Start-Session $sb @(
  (New-WindowSpec -Fix $Fix -Tabs alpha, bravo -Bounds (New-Bounds 40 10 900 360)),
  (New-WindowSpec -Fix $Fix -Tabs charlie, delta -Bounds (New-Bounds 40 430 900 360))
)
$root = Find-Window $app 'alpha'
$other = Find-Window $app 'charlie'
Focus-Window $other
Close-Window $root

$left = @(Wait-AppWindows $app 1)
Wait-TabNames $left[0] 'charlie,delta'
Assert-That (Test-AppAlive $app) 'the app is still running after the root was closed'
Assert-That ([math]::Abs($left[0].L - $other.L) -le 3 -and [math]::Abs($left[0].T - $other.T) -le 3) "the survivor is at $($left[0].L),$($left[0].T), expected $($other.L),$($other.T)"
Assert-That ([math]::Abs($left[0].Width - $other.Width) -le 3) 'the survivor keeps the other window size'
$c = Wait-SavedConfig $sb { param($c) @($c.session.windows).Count -eq 1 } 'single window saved'
Assert-Equal (@($c.session.windows[0].tabs) | ForEach-Object { Split-Path $_ -Leaf }) 'charlie,delta' 'saved tabs'
