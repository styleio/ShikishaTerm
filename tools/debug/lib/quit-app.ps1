<#
  Quit a running copy of the app the way a person does: its window's close
  button, then the answer to the question it asks on the way out.

    quit-app.ps1 -Root <folder the copy runs from> -Answer Yes|No|Cancel [-Seconds 20]

  The window is told to close (WM_CLOSE, what its ✕ sends). The question is the
  system's own message box, titled SHIKISHA-TERM, shown by a process of that
  copy; its button is pressed by its id (Yes 6, No 7, Cancel 2). Prints what
  the question said, then "answered <Answer>", or "no question" when the copy
  quit without asking. Nothing of any other copy is touched.
#>
param(
    [Parameter(Mandatory)][string]$Root,
    [Parameter(Mandatory)][ValidateSet('Yes', 'No', 'Cancel')][string]$Answer,
    [int]$Seconds = 20
)
$ErrorActionPreference = 'Stop'

Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class QuitApp {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetDlgItemText(IntPtr h, int id, StringBuilder s, int n);
    public static List<IntPtr> Windows(HashSet<uint> pids, string cls, bool hiddenToo) {
        var found = new List<IntPtr>();
        EnumWindows((h, l) => {
            uint pid; GetWindowThreadProcessId(h, out pid);
            if (!pids.Contains(pid) || (!hiddenToo && !IsWindowVisible(h))) return true;
            var c = new StringBuilder(256); GetClassName(h, c, 256);
            if (cls == null ? c.ToString() != "#32770" : c.ToString() == cls) found.Add(h);
            return true;
        }, IntPtr.Zero);
        return found;
    }
    public static string Said(IntPtr dlg) {
        var s = new StringBuilder(4096);
        GetDlgItemText(dlg, 0xFFFF, s, 4096);
        return s.ToString();
    }
}
'@

$pids = [System.Collections.Generic.HashSet[uint32]]::new()
Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and $_.Path -like "$Root\*" } |
    ForEach-Object { [void]$pids.Add([uint32]$_.Id) }
if ($pids.Count -eq 0) { Write-Output 'no copy running'; exit 2 }

# The window's close button
$WM_CLOSE = 0x0010
# The app's own window: the one titled with its name (the others are the
# window toolkit's, and closing them is not what a person does)
# Hidden too: a window put away still quits the app when it is told to close
$wins = [QuitApp]::Windows($pids, 'Window Class', $true)
if ($wins.Count -eq 0) {
    $all = [QuitApp]::Windows($pids, $null, $true) | ForEach-Object { $c = New-Object System.Text.StringBuilder 256; [void][QuitApp]::GetClassName($_, $c, 256); $c.ToString() }
    Write-Output ('no window (the copy has: ' + ($all -join ', ') + ')')
    exit 2
}
foreach ($w in $wins) { [void][QuitApp]::PostMessage($w, $WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero) }

# The question, from whichever process of the copy asks it
$ids = @{ Yes = 6; No = 7; Cancel = 2 }
$until = (Get-Date).AddSeconds($Seconds)
while ((Get-Date) -lt $until) {
    $alive = @(Get-Process -Id ([uint32[]]$pids) -ErrorAction SilentlyContinue)
    if ($alive.Count -eq 0) { Write-Output 'no question'; exit 0 }
    foreach ($d in [QuitApp]::Windows($pids, '#32770', $false)) {
        $text = New-Object System.Text.StringBuilder 256
        [void][QuitApp]::GetWindowText($d, $text, 256)
        if ($text.ToString() -ne 'SHIKISHA-TERM') { continue }
        Write-Output ('said: ' + ([QuitApp]::Said($d) -replace "`r?`n", ' / '))
        $WM_COMMAND = 0x0111
        [void][QuitApp]::PostMessage($d, $WM_COMMAND, [IntPtr]$ids[$Answer], [QuitApp]::GetDlgItem($d, $ids[$Answer]))
        Write-Output "answered $Answer"
        exit 0
    }
    Start-Sleep -Milliseconds 300
}
Write-Output 'the question did not come'
exit 1
