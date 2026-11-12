# Helpers for the end-to-end suite: launch the app in a sandbox, find its windows, drive the real
# mouse and keyboard, read the saved config and the log. Dot-source it; run.ps1 does.
# Needs PowerShell 7 and an interactive desktop.

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing, System.Windows.Forms

if (-not ('E2E.Native' -as [type])) {
  Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
namespace E2E {
  public static class Native {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, uint d, UIntPtr e);
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc p, IntPtr l);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);

    // "hwnd|left|top|right|bottom|title" for each visible top-level window of the process, topmost first.
    public static List<string> Windows(uint pid) {
      var list = new List<string>();
      EnumWindows((h, l) => {
        uint owner; GetWindowThreadProcessId(h, out owner);
        if (owner == pid && IsWindowVisible(h)) {
          RECT r; GetWindowRect(h, out r);
          var sb = new StringBuilder(512); GetWindowText(h, sb, 512);
          list.Add(h.ToInt64() + "|" + r.L + "|" + r.T + "|" + r.R + "|" + r.B + "|" + sb);
        }
        return true;
      }, IntPtr.Zero);
      return list;
    }
  }
}
'@
}
# Per-monitor v2 so that window rects, the cursor and UI Automation all speak physical pixels.
if (-not [E2E.Native]::SetProcessDpiAwarenessContext([IntPtr](-4))) { [void][E2E.Native]::SetProcessDPIAware() }

$global:E2E_Exe = $null
$global:E2E_OutDir = $null
$global:E2E_Work = $null
$global:E2E_Apps = New-Object System.Collections.ArrayList

function Initialize-E2E([string]$Exe, [string]$OutDir, [string]$Work) {
  $global:E2E_Exe = $Exe
  $global:E2E_OutDir = $OutDir
  $global:E2E_Work = $Work
  New-Item -ItemType Directory -Force $OutDir, $Work | Out-Null
}

# ---------------------------------------------------------------- assertions and waiting

function Assert-That($Condition, [string]$Message) {
  if (-not $Condition) { throw "ASSERT: $Message" }
}

function Assert-Equal($Actual, $Expected, [string]$What) {
  $a = ($Actual -join ','); $e = ($Expected -join ',')
  if ($a -ne $e) { throw "ASSERT: $What - expected [$e], got [$a]" }
}

function Wait-Until([scriptblock]$Condition, [int]$TimeoutMs = 8000, [string]$What = 'condition') {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
    $r = & $Condition
    if ($r) { return $r }
    Start-Sleep -Milliseconds 100
  }
  throw "TIMEOUT: $What (after $TimeoutMs ms)"
}

# ---------------------------------------------------------------- fixtures and sandbox

function New-TestImage([string]$Path, [int]$W, [int]$H, [string]$Hex) {
  $bmp = New-Object System.Drawing.Bitmap $W, $H
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.Clear([System.Drawing.ColorTranslator]::FromHtml($Hex))
  $g.Dispose()
  $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
  $bmp.Dispose()
}

# Five folders of six small PNGs each: alpha, bravo, charlie, delta, echo.
function New-Fixtures([string]$Root) {
  $colors = [ordered]@{ alpha = '#DC3C3C'; bravo = '#3CA0DC'; charlie = '#46BE5A'; delta = '#E6BE28'; echo = '#9650C8' }
  foreach ($name in $colors.Keys) {
    $dir = Join-Path $Root $name
    New-Item -ItemType Directory -Force $dir | Out-Null
    foreach ($i in 1..6) { New-TestImage (Join-Path $dir ('{0}_{1:D2}.png' -f $name, $i)) (96 + 16 * $i) 72 $colors[$name] }
  }
}

# A per-scenario config dir, cache dir, single-instance name and log, none shared with the real app.
function New-AppEnv([string]$Name) {
  $dir = Join-Path $global:E2E_Work $Name
  New-Item -ItemType Directory -Force (Join-Path $dir 'config'), (Join-Path $dir 'cache') | Out-Null
  [pscustomobject]@{
    Name      = $Name
    Dir       = $dir
    ConfigDir = Join-Path $dir 'config'
    CacheDir  = Join-Path $dir 'cache'
    Instance  = "e2e-$Name-$PID"
    Log       = Join-Path $global:E2E_OutDir "$Name.log"
  }
}

function Get-WorkArea { [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea }

# Bounds placed relative to the top-left of the primary monitor's work area.
function New-Bounds([int]$Dx, [int]$Dy, [int]$Width = 1000, [int]$Height = 650) {
  $wa = Get-WorkArea
  @{ x = $wa.X + $Dx; y = $wa.Y + $Dy; width = $Width; height = $Height }
}

# Far to the right of every monitor: a window stored here has been left behind by an unplugged display.
function New-OffscreenBounds([int]$Width = 800, [int]$Height = 500) {
  $vs = [System.Windows.Forms.SystemInformation]::VirtualScreen
  @{ x = $vs.Right + 4000; y = 100; width = $Width; height = $Height }
}

# One stored window. Tab names are fixture folders unless they are already absolute paths.
function New-WindowSpec([string[]]$Tabs, [string]$Fix, $Bounds = $null, [int]$Active = 0) {
  $paths = @($Tabs | ForEach-Object { if ([IO.Path]::IsPathRooted($_)) { $_ } else { Join-Path $Fix $_ } })
  $w = @{ tabs = $paths; active = $Active }
  if ($Bounds) { $w.bounds = $Bounds }
  $w
}

function Write-AppConfig($Sb, $Windows, [int]$ActiveWindow = 0) {
  $cfg = @{ session = @{ windows = @($Windows); active_window = $ActiveWindow }; target_size = 128.0 }
  $json = $cfg | ConvertTo-Json -Depth 8
  [IO.File]::WriteAllText((Join-Path $Sb.ConfigDir 'config.json'), $json, (New-Object Text.UTF8Encoding $false))
}

function Get-SavedConfig($Sb) {
  $p = Join-Path $Sb.ConfigDir 'config.json'
  if (Test-Path $p) { Get-Content $p -Raw | ConvertFrom-Json } else { $null }
}

# Waits for the app's debounced save to satisfy `$Check` (a scriptblock taking the parsed config).
function Wait-SavedConfig($Sb, [scriptblock]$Check, [string]$What = 'saved config') {
  Wait-Until { $c = Get-SavedConfig $Sb; if ($c -and (& $Check $c)) { $c } } 8000 $What
}

function Get-Leaf([string]$Path) { Split-Path $Path -Leaf }

# ---------------------------------------------------------------- launching

function Set-AppEnv($Sb) {
  $old = @{}
  $vars = @{
    IMAGE_MOZ_CONFIG_DIR = $Sb.ConfigDir
    IMAGE_MOZ_CACHE_DIR  = $Sb.CacheDir
    IMAGE_MOZ_INSTANCE   = $Sb.Instance
    RUST_LOG             = 'image_moz=debug'
  }
  foreach ($k in $vars.Keys) { $old[$k] = [Environment]::GetEnvironmentVariable($k); [Environment]::SetEnvironmentVariable($k, $vars[$k]) }
  $old
}

function Restore-AppEnv($Old) {
  foreach ($k in $Old.Keys) { [Environment]::SetEnvironmentVariable($k, $Old[$k]) }
}

function Start-App($Sb, [string]$Arg = $null) {
  $old = Set-AppEnv $Sb
  try {
    $a = @{ FilePath = $global:E2E_Exe; PassThru = $true; RedirectStandardError = $Sb.Log; RedirectStandardOutput = "$($Sb.Log).out" }
    if ($Arg) { $a.ArgumentList = "`"$Arg`"" }
    $p = Start-Process @a
  } finally { Restore-AppEnv $old }
  $app = [pscustomobject]@{ Process = $p; Sandbox = $Sb }
  [void]$global:E2E_Apps.Add($app)
  $app
}

# Writes the config, starts the app and waits until it shows one window per stored window.
function Start-Session($Sb, $Windows, [int]$ActiveWindow = 0, [int]$Expect = -1) {
  Write-AppConfig $Sb $Windows $ActiveWindow
  $app = Start-App $Sb
  if ($Expect -lt 0) { $Expect = @($Windows).Count }
  [void](Wait-AppWindows $app $Expect 25000)
  Start-Sleep -Milliseconds 800
  $app
}

# A second launch with an argument: it must hand the path to the running instance and exit 0.
function Send-Path($Sb, [string]$Path) {
  $old = Set-AppEnv $Sb
  try { $p = Start-Process -FilePath $global:E2E_Exe -ArgumentList "`"$Path`"" -PassThru } finally { Restore-AppEnv $old }
  if (-not $p.WaitForExit(15000)) { $p.Kill(); throw 'TIMEOUT: the forwarding launch did not exit' }
  Assert-Equal $p.ExitCode 0 'forwarding launch exit code'
}

function Stop-App($App) {
  Stop-Process -Id $App.Process.Id -Force -ErrorAction SilentlyContinue
}

function Stop-AllApps {
  foreach ($app in $global:E2E_Apps) { Stop-App $app }
  $global:E2E_Apps.Clear()
  # Anything left over from this exe (a launch that outlived its scenario).
  Get-CimInstance Win32_Process -Filter "Name='image-moz.exe'" | Where-Object { $_.ExecutablePath -eq $global:E2E_Exe } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
}

function Test-AppAlive($App) {
  $App.Process.Refresh()
  -not $App.Process.HasExited
}

# ---------------------------------------------------------------- windows

function Get-AppWindows($App) {
  $i = 0
  foreach ($line in [E2E.Native]::Windows([uint32]$App.Process.Id)) {
    $f = $line -split '\|', 6
    if ($f[5] -like '*Image Moz') {
      [pscustomobject]@{
        Hwnd = [int64]$f[0]; L = [int]$f[1]; T = [int]$f[2]; R = [int]$f[3]; B = [int]$f[4]
        Width = [int]$f[3] - [int]$f[1]; Height = [int]$f[4] - [int]$f[2]; Title = $f[5]; Z = $i; Log = $App.Sandbox.Log
      }
    }
    $i++
  }
}

function Wait-AppWindows($App, [int]$Count, [int]$TimeoutMs = 10000) {
  Wait-Until { $w = @(Get-AppWindows $App); if ($w.Count -eq $Count) { , $w } } $TimeoutMs "$Count window(s)"
}

# Folder name shown in a window's title (its active tab).
function Get-ActiveFolder($Window) {
  # Title is "<folder> — Image Moz"; the dash is built from its code point to stay encoding-proof.
  if ($Window.Title -match ('^(.+) ' + [char]0x2014 + ' Image Moz$')) { $Matches[1] } else { $null }
}

# The window currently showing `$Folder`; waits for it.
function Find-Window($App, [string]$Folder, [int]$TimeoutMs = 8000) {
  Wait-Until { @(Get-AppWindows $App) | Where-Object { (Get-ActiveFolder $_) -eq $Folder } | Select-Object -First 1 } $TimeoutMs "a window on '$Folder'"
}

function Get-WindowByHwnd($App, [int64]$Hwnd) {
  @(Get-AppWindows $App) | Where-Object { $_.Hwnd -eq $Hwnd } | Select-Object -First 1
}

function Get-Scale($Window) { [E2E.Native]::GetDpiForWindow([IntPtr]$Window.Hwnd) / 96.0 }

function Close-Window($Window) {
  [void][E2E.Native]::PostMessage([IntPtr]$Window.Hwnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
}

function Test-Foreground($Window) {
  [E2E.Native]::GetForegroundWindow().ToInt64() -eq $Window.Hwnd
}

# Clicks the title bar, which brings the window forward without touching its content.
function Focus-Window($Window) {
  $s = Get-Scale $Window
  Click-At ($Window.L + $Window.Width / 2) ($Window.T + 12 * $s)
  [void](Wait-Until { Test-Foreground $Window } 4000 'the window to take the focus')
}

# ---------------------------------------------------------------- tabs, through UI Automation

# Tab labels of a window, left to right, with their screen rectangles in physical pixels. The app logs
# the layout whenever it changes (secondary windows are not exposed to UI Automation).
function Get-Tabs($Window) {
  $line = $null
  if (Test-Path $Window.Log) {
    $pattern = 'tabbar outer=(-?\d+),(-?\d+) tabs=(.*)$'
    foreach ($l in (Get-Content $Window.Log -Tail 400)) {
      if ($l -match $pattern -and [math]::Abs([int]$Matches[1] - $Window.L) -le 2 -and [math]::Abs([int]$Matches[2] - $Window.T) -le 2) { $line = $Matches[3] }
    }
  }
  if (-not $line) { return }
  foreach ($entry in $line -split '\|') {
    $f = $entry -split ':'
    if ($f.Count -lt 5) { continue }
    [pscustomobject]@{
      Name = ($f[0..($f.Count - 5)] -join ':')
      Left = [int]$f[-4]; Right = [int]$f[-3]; Top = [int]$f[-2]; Bottom = [int]$f[-1]
      CX = [int](([int]$f[-4] + [int]$f[-3]) / 2); CY = [int](([int]$f[-2] + [int]$f[-1]) / 2)
    }
  }
}

# "alpha,bravo": tab names in order, with any "(n)" badge.
function Get-TabNames($Window) { (@(Get-Tabs $Window) | ForEach-Object { $_.Name }) -join ',' }

function Wait-TabNames($Window, [string]$Expected, [int]$TimeoutMs = 8000) {
  $seen = [ref]''
  try {
    [void](Wait-Until { $seen.Value = Get-TabNames $Window; $seen.Value -eq $Expected } $TimeoutMs "tabs [$Expected]")
  } catch { throw "$($_.Exception.Message); last seen [$($seen.Value)]" }
}

# ---------------------------------------------------------------- mouse and keyboard

function Move-Mouse($X, $Y) { [void][E2E.Native]::SetCursorPos([int]$X, [int]$Y) }
function Press-Mouse { [E2E.Native]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero) }
function Release-Mouse { [E2E.Native]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero) }

function Click-At($X, $Y) {
  Move-Mouse $X $Y; Start-Sleep -Milliseconds 80
  Press-Mouse; Start-Sleep -Milliseconds 60
  Release-Mouse; Start-Sleep -Milliseconds 200
}

function Middle-Click-At($X, $Y) {
  Move-Mouse $X $Y; Start-Sleep -Milliseconds 80
  [E2E.Native]::mouse_event(0x0020, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 60
  [E2E.Native]::mouse_event(0x0040, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 200
}

# Press at the start, glide to the end in small steps (egui needs motion), release unless -Hold.
function Drag-Mouse($X1, $Y1, $X2, $Y2, [int]$Steps = 24, [switch]$Hold) {
  Move-Mouse $X1 $Y1; Start-Sleep -Milliseconds 150
  Press-Mouse; Start-Sleep -Milliseconds 150
  foreach ($i in 1..$Steps) {
    Move-Mouse ($X1 + ($X2 - $X1) * $i / $Steps) ($Y1 + ($Y2 - $Y1) * $i / $Steps)
    Start-Sleep -Milliseconds 25
  }
  Start-Sleep -Milliseconds 250
  if (-not $Hold) { Release-Mouse; Start-Sleep -Milliseconds 700 }
}

function Click-Tab($Window, [string]$Name) {
  $t = @(Get-Tabs $Window) | Where-Object { $_.Name -eq $Name } | Select-Object -First 1
  Assert-That $t "no tab named '$Name' in [$(Get-TabNames $Window)]"
  Click-At $t.CX $t.CY
}

# Virtual-key codes: Ctrl 0x11, Shift 0x10, Tab 0x09, W 0x57, T 0x54.
function Send-Chord([int[]]$Keys) {
  foreach ($k in $Keys) { [E2E.Native]::keybd_event([byte]$k, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 50 }
  foreach ($k in $Keys[($Keys.Count - 1)..0]) { [E2E.Native]::keybd_event([byte]$k, 0, 2, [UIntPtr]::Zero); Start-Sleep -Milliseconds 50 }
  Start-Sleep -Milliseconds 400
}

# ---------------------------------------------------------------- log and diagnosis

function Get-AppLog($App) {
  if (Test-Path $App.Sandbox.Log) { Get-Content $App.Sandbox.Log -Raw } else { '' }
}

function Wait-Log($App, [string]$Pattern, [int]$TimeoutMs = 8000) {
  Wait-Until { (Get-AppLog $App) -match $Pattern } $TimeoutMs "log line /$Pattern/"
}

# Screenshot of the area the app's windows cover, for diagnosis only; never asserted on.
function Save-Shot([string]$Name) {
  try {
    $rects = foreach ($app in $global:E2E_Apps) { foreach ($w in @(Get-AppWindows $app)) { $w } }
    if (-not $rects) { return }
    $l = ($rects | Measure-Object L -Minimum).Minimum; $t = ($rects | Measure-Object T -Minimum).Minimum
    $r = ($rects | Measure-Object R -Maximum).Maximum; $b = ($rects | Measure-Object B -Maximum).Maximum
    $bmp = New-Object System.Drawing.Bitmap ([int]($r - $l)), ([int]($b - $t))
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen([int]$l, [int]$t, 0, 0, $bmp.Size)
    $bmp.Save((Join-Path $global:E2E_OutDir "$Name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
  } catch { Write-Host "  (no screenshot: $($_.Exception.Message))" }
}
