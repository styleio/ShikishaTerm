<#
  Photograph one copy of the app's window: the one named, and no other.

  By the process, not by a region of the screen. Another copy of the app --
  the one somebody is working in -- can sit on top of the copy being checked,
  and a grab of the screen would take that one instead, along with whatever
  else is open beside it.

  Which copy is said one of two ways, because there are two ways of knowing.
  A script that started the copy holds its process id; a session that only
  knows where the build lives has the folder.

  PW_RENDERFULLCONTENT, because the window's page is a WebView2 that draws
  through DirectComposition, and a plain PrintWindow of it comes back black.

    tools/debug/shot-window.win.ps1 -Under <the folder the exe runs from> -Out shot.png
    tools/debug/shot-window.win.ps1 -ProcessId <its process id> -Out shot.png
#>
param(
    [string]$Under,
    [int]$ProcessId,
    [Parameter(Mandatory)] [string]$Out
)
$ErrorActionPreference = 'Stop'
if (-not $Under -and -not $ProcessId) { throw 'say which copy: -Under <folder> or -ProcessId <id>' }
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class ShotWindow {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
}
"@
if ($ProcessId) {
    # The process named, or one of its children: the window may belong to a copy
    # that was started by the one whose id is held (a test runner starts the app)
    $p = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
    if ($p -and $p.MainWindowHandle -eq 0) {
        $kids = Get-CimInstance Win32_Process -Filter "ParentProcessId = $ProcessId" |
                ForEach-Object { Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue }
        $p = $kids | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
    }
    if (-not $p) { throw "no window for process $ProcessId, or for anything it started" }
} else {
    $p = Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue |
         Where-Object { $_.Path -and $_.Path -like "$Under\*" -and $_.MainWindowHandle -ne 0 } |
         Select-Object -First 1
    if (-not $p) { throw "no window for a copy of the app under $Under" }
}
$r = New-Object ShotWindow+RECT
[void][ShotWindow]::GetWindowRect($p.MainWindowHandle, [ref]$r)
$bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
[void][ShotWindow]::PrintWindow($p.MainWindowHandle, $hdc, 2)
$g.ReleaseHdc($hdc); $g.Dispose()
New-Item -ItemType Directory -Force (Split-Path $Out) | Out-Null
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
