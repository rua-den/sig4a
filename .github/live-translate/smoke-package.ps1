# Hosted Windows smoke of the ordinary extracted package. Does not open a physical endpoint.
param([Parameter(Mandatory)][string]$Archive)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$root = Join-Path $env:RUNNER_TEMP ('sig4a-extracted-' + [guid]::NewGuid())
$profile = Join-Path $root 'isolated'
Expand-Archive -LiteralPath $Archive -DestinationPath $root
$runtime = Join-Path $root 'live-translate/Sig4a.LiveTranslate.exe'
$dashboard = Join-Path $root 'claude-usage-dashboard.exe'
$tray = Join-Path $root 'claude-usage.exe'
foreach ($path in @($runtime,$dashboard,$tray)) {
    if (!(Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing package member: $([IO.Path]::GetFileName($path))" }
}
& (Join-Path $PSScriptRoot '../../tools/live_translate_runtime/Smoke.ps1') -Published (Split-Path -Parent $runtime)
function Read-Event($proc, $task, [int]$ms=5500) {
    if (!$task.Wait($ms)) { throw 'Live Translate helper timed out.' }
    $line = $task.Result
    if (!$line -or $line.Length -gt 16384) { throw 'Live Translate helper returned invalid protocol frame.' }
    $j = $line | ConvertFrom-Json
    if ($j.v -ne 1) { throw 'Live Translate helper protocol version mismatch.' }
    return $j
}
function Read-IsolatedStartupSummary([string]$IsolatedProfile) {
    # Only the profile constructed by this smoke above; never use the runner's real profile.
    $path = Join-Path $IsolatedProfile 'local/claude-usage/events.log'
    try {
        if (!(Test-Path -LiteralPath $path -PathType Leaf)) { return '<no isolated startup log>' }
        if ((Get-Item -LiteralPath $path).Length -gt 65536) { return '<isolated startup log exceeds diagnostic limit>' }
        $lines = @(Get-Content -LiteralPath $path -Tail 10 | ForEach-Object { $_.Substring(0, [Math]::Min(240, $_.Length)) })
        if (!$lines.Count) { return '<empty isolated startup log>' }
        return ($lines -join ' | ')
    } catch {
        return '<isolated startup log unavailable>'
    }
}
$psi = [Diagnostics.ProcessStartInfo]::new($runtime)
$psi.WorkingDirectory = Split-Path -Parent $runtime
$psi.UseShellExecute = $false
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.CreateNoWindow = $true
$probe = $null
$app = $null
try {
    $probe = [Diagnostics.Process]::Start($psi)
    $first = $probe.StandardOutput.ReadLineAsync()
    if ($first.Wait(450)) { throw 'Unexpected unsolicited protocol output on launch; no capture should start.' }
    $probe.StandardInput.WriteLine('{"v":1,"type":"list"}')
    $probe.StandardInput.Flush()
    $result = Read-Event $probe $first
    if ($result.type -ne 'devices') { throw 'Explicit list did not return a device inventory.' }
    $probe.StandardInput.WriteLine('{"v":1,"type":"start","session":1,"source":"mic","device_id":"SIG4A-PROOF-INVALID-DEVICE","translate":false}')
    $probe.StandardInput.Flush()
    $result = Read-Event $probe ($probe.StandardOutput.ReadLineAsync())
    if ($result.type -ne 'error' -or $result.session -ne 1 -or $result.code -notin @('device_missing','model_missing','backend_missing')) {
        throw 'Invalid mic selection did not fail closed.'
    }
    $probe.StandardInput.WriteLine('{"v":1,"type":"quit"}')
    $probe.StandardInput.Close()
    if (!$probe.WaitForExit(6000) -or $probe.ExitCode -ne 0) { throw 'Helper did not exit cleanly after Quit.' }
    $env:LOCALAPPDATA = Join-Path $profile 'local'
    $env:USERPROFILE = Join-Path $profile 'home'
    New-Item -ItemType Directory -Force $env:LOCALAPPDATA,$env:USERPROFILE | Out-Null
    Remove-Item Env:HOMEDRIVE,Env:HOMEPATH,Env:CLAUDE_CONFIG_DIR -ErrorAction SilentlyContinue
    $env:CU_NO_AUTOSTART = '1'
    $env:CU_NO_REPAIR = '1'
    $env:CU_NO_DASHBOARD = '1'
    Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class Sig4aPackageSmokeWin {
    private delegate bool EnumWindowsCallback(IntPtr wnd, IntPtr param);
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumWindowsCallback callback, IntPtr param);
    [DllImport("user32.dll", CharSet=CharSet.Unicode, ExactSpelling=true)]
    private static extern int GetClassNameW(IntPtr wnd, StringBuilder name, int capacity);
    [DllImport("user32.dll")] private static extern IntPtr GetAncestor(IntPtr wnd, uint flags);
    [StructLayout(LayoutKind.Sequential)]
    private struct Rect { public int left, top, right, bottom; }
    [DllImport("user32.dll")] private static extern bool GetWindowRect(IntPtr wnd, out Rect rect);
    private static uint OwnerPid(IntPtr wnd) {
        uint pid;
        return wnd != IntPtr.Zero && GetWindowThreadProcessId(wnd, out pid) != 0 ? pid : 0;
    }
    private static string WindowClass(IntPtr wnd) {
        var name = new StringBuilder(256);
        return GetClassNameW(wnd, name, name.Capacity) > 0 ? name.ToString() : "<unavailable>";
    }
    public static bool IsVisibleOwnedRoot(IntPtr wnd, uint expectedPid) {
        return wnd != IntPtr.Zero && expectedPid != 0 && OwnerPid(wnd) == expectedPid &&
            IsWindowVisible(wnd) && GetAncestor(wnd, 2) == wnd; // GA_ROOT
    }
    public static IntPtr[] DashboardCandidates(uint expectedPid, IntPtr mainWindow) {
        var candidates = new List<IntPtr>();
        EnumWindows(delegate(IntPtr wnd, IntPtr unused) {
            if (IsVisibleOwnedRoot(wnd, expectedPid) &&
                String.Equals(WindowClass(wnd), "ClaudeUsageDashboard", StringComparison.Ordinal)) {
                candidates.Add(wnd);
            }
            return true;
        }, IntPtr.Zero);
        // A framework can expose the app through a different top-level class. The
        // launched process's actual main HWND is only a candidate; UIA must prove it.
        if (IsVisibleOwnedRoot(mainWindow, expectedPid) && !candidates.Contains(mainWindow)) {
            candidates.Add(mainWindow);
        }
        return candidates.ToArray();
    }
    public static string Describe(IntPtr wnd) {
        Rect rect;
        string bounds = GetWindowRect(wnd, out rect)
            ? String.Format("{0},{1},{2},{3}", rect.left, rect.top, rect.right, rect.bottom) : "<unavailable>";
        return String.Format("HWND={0}, class={1}, PID={2}, visible={3}, root={4}, bounds={5}",
            wnd, WindowClass(wnd), OwnerPid(wnd), IsWindowVisible(wnd), GetAncestor(wnd, 2), bounds);
    }
    public static string DescribeOwned(uint expectedPid) {
        var windows = new List<string>();
        EnumWindows(delegate(IntPtr wnd, IntPtr unused) {
            if (OwnerPid(wnd) == expectedPid) windows.Add(Describe(wnd));
            return windows.Count < 12;
        }, IntPtr.Zero);
        return windows.Count == 0 ? "<none>" : String.Join("; ", windows);
    }
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr wnd, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr wnd);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr wnd);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr wnd, IntPtr root, int x, int y, int w, int h, uint flags);
}
'@
    $app = Start-Process -FilePath $dashboard -WorkingDirectory $root -PassThru
    $timer = [Diagnostics.Stopwatch]::StartNew()
    [uint32]$launchPid = $app.Id
    do {
        Start-Sleep -Milliseconds 250
        $app.Refresh()
        if ($app.HasExited) {
            $startup = Read-IsolatedStartupSummary $profile
            throw "Extracted dashboard exited early ($($app.ExitCode)). Startup: $startup"
        }
        $candidates = @([Sig4aPackageSmokeWin]::DashboardCandidates($launchPid, $app.MainWindowHandle))
    } until ($candidates.Count -gt 0 -or $timer.Elapsed.TotalSeconds -gt 20)
    if ($candidates.Count -eq 0) {
        $main = [Sig4aPackageSmokeWin]::Describe($app.MainWindowHandle)
        $owned = [Sig4aPackageSmokeWin]::DescribeOwned($launchPid)
        $startup = Read-IsolatedStartupSummary $profile
        throw "Extracted dashboard has no visible owned root (launch PID=$launchPid; process main: $main; owned windows: $owned). Startup: $startup"
    }
    # A native HWND alone can be a dialog or a blank/framework window. Only pass
    # when the same visible owned root exposes canonical dashboard navigation.
    $uia = Join-Path $PSScriptRoot 'desktop-uia.ps1'
    $hwnd = [IntPtr]::Zero
    $sample = $null
    $examined = $null
    $placed = [Collections.Generic.HashSet[IntPtr]]::new()
    $timer.Restart()
    do {
        $app.Refresh()
        if ($app.HasExited) {
            $startup = Read-IsolatedStartupSummary $profile
            throw "Extracted dashboard exited before navigation was shown ($($app.ExitCode)). Startup: $startup"
        }
        $candidates = @([Sig4aPackageSmokeWin]::DashboardCandidates($launchPid, $app.MainWindowHandle))
        foreach ($h in $candidates) {
            if (![Sig4aPackageSmokeWin]::IsVisibleOwnedRoot($h, $launchPid)) { continue }
            $examined = [Sig4aPackageSmokeWin]::Describe($h)
            if ($placed.Add($h)) { [void][Sig4aPackageSmokeWin]::SetWindowPos($h, [IntPtr]::Zero, 24, 24, 900, 640, 0x0014) }
            [void][Sig4aPackageSmokeWin]::SetForegroundWindow($h)
            $raw = & powershell.exe -NoProfile -STA -File $uia -Hwnd "$($h.ToInt64())" -Mode list
            if ($LASTEXITCODE -ne 0) {
                $startup = Read-IsolatedStartupSummary $profile
                throw "Native dashboard UI automation failed ($examined). Startup: $startup"
            }
            $sample = ($raw -join "`n") | ConvertFrom-Json
            $buttons = @($sample.items | Where-Object {$_.kind -eq 'ControlType.Button' -and !$_.offscreen -and $_.right -gt $_.left -and $_.bottom -gt $_.top})
            $names = @($buttons | ForEach-Object name)
            if ('Live Translate' -cin $names -and 'Claude Usage' -cin $names -and 'Cài đặt' -cin $names -and
                [Sig4aPackageSmokeWin]::IsVisibleOwnedRoot($h, $launchPid)) {
                $hwnd = $h
                break
            }
            if ($timer.Elapsed.TotalSeconds -gt 28) { break }
        }
        if ($hwnd -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 350 }
    } until ($hwnd -ne [IntPtr]::Zero -or $timer.Elapsed.TotalSeconds -gt 28)
    if ($hwnd -eq [IntPtr]::Zero) {
        $main = [Sig4aPackageSmokeWin]::Describe($app.MainWindowHandle)
        $owned = [Sig4aPackageSmokeWin]::DescribeOwned($launchPid)
        $summary = @($sample.items | Select-Object -First 14 name,kind,offscreen) | ConvertTo-Json -Compress
        $startup = Read-IsolatedStartupSummary $profile
        $foreground = [Sig4aPackageSmokeWin]::GetForegroundWindow()
        throw "Extracted native dashboard navigation missing (launch PID=$launchPid; foreground=$foreground; process main: $main; owned: $owned; last examined: $examined): $summary. Startup: $startup"
    }
    $verifiedWindow = [Sig4aPackageSmokeWin]::Describe($hwnd)
    "Native UIA found visible Live Translate, Claude Usage and Settings buttons in the extracted app ($verifiedWindow)."
    if (Get-Process -Name 'Sig4a.LiveTranslate' -ErrorAction SilentlyContinue) { throw 'Helper should not start merely on dashboard launch.' }
    'SIG4A archive verified: actual released dashboard, explicit inventory, invalid source no fallback, no passive helper/capture. No physical endpoints accessed.'
} finally {
    foreach ($p in @($probe,$app)) {
        if ($p) {
            if (!$p.HasExited) { $p.Kill($true); [void]$p.WaitForExit(5000) }
            $p.Dispose()
        }
    }
}
