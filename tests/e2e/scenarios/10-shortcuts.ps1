# Ctrl+Tab and Ctrl+W act on the window that has the focus, not on every window.
param([string]$Fix, [string]$Name)
$sb = New-AppEnv $Name
$app = Start-Session $sb @(
  (New-WindowSpec -Fix $Fix -Tabs alpha, bravo, charlie -Bounds (New-Bounds 40 10 900 360)),
  (New-WindowSpec -Fix $Fix -Tabs delta, echo -Bounds (New-Bounds 40 430 900 360))
)
$a = Find-Window $app 'alpha'
$b = Find-Window $app 'delta'
$CTRL = 0x11; $SHIFT = 0x10; $TAB = 0x09; $W = 0x57

Focus-Window $a
Send-Chord @($CTRL, $TAB)
[void](Wait-Until { (Get-ActiveFolder (Get-WindowByHwnd $app $a.Hwnd)) -eq 'bravo' } 4000 'Ctrl+Tab to show bravo')
Assert-Equal (Get-ActiveFolder (Get-WindowByHwnd $app $b.Hwnd)) 'delta' 'the other window did not cycle'
Send-Chord @($CTRL, $SHIFT, $TAB)
[void](Wait-Until { (Get-ActiveFolder (Get-WindowByHwnd $app $a.Hwnd)) -eq 'alpha' } 4000 'Ctrl+Shift+Tab to go back to alpha')

Send-Chord @($CTRL, $W)
Wait-TabNames (Get-WindowByHwnd $app $a.Hwnd) 'bravo,charlie'
Assert-Equal (Get-TabNames (Get-WindowByHwnd $app $b.Hwnd)) 'delta,echo' 'Ctrl+W closed a tab of the other window too'

Focus-Window (Get-WindowByHwnd $app $b.Hwnd)
Send-Chord @($CTRL, $W)
Wait-TabNames (Get-WindowByHwnd $app $b.Hwnd) 'echo'
Assert-Equal (Get-TabNames (Get-WindowByHwnd $app $a.Hwnd)) 'bravo,charlie' 'the first window was left alone'
