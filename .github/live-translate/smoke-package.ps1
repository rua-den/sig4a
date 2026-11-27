# Hosted Windows smoke of the ordinary extracted package. Does not open a physical endpoint.
param([Parameter(Mandatory)][string]$Archive,[switch]$InstallEnglishFromUI)
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
function Test-PositiveUiaRect($Rect) {
    if ($null -eq $Rect) { return $false }
    foreach ($edge in @('left','top','right','bottom')) {
        $coordinate = $Rect.$edge
        if ($null -eq $coordinate -or !($coordinate -is [ValueType])) { return $false }
        $number = [double]$coordinate
        if ([double]::IsNaN($number) -or [Math]::Abs($number) -gt 1000000) { return $false }
    }
    return ($Rect.right -gt $Rect.left -and $Rect.bottom -gt $Rect.top)
}
function Test-UiaButtonInRoot($Button,$Root) {
    if ($Button.kind -ne 'ControlType.Button' -or $Button.offscreen -ne $false -or
        !(Test-PositiveUiaRect $Button) -or !(Test-PositiveUiaRect $Root)) { return $false }
    $centerX = ([double]$Button.left + [double]$Button.right) / 2
    $centerY = ([double]$Button.top + [double]$Button.bottom) / 2
    return ($centerX -ge $Root.left -and $centerX -lt $Root.right -and
        $centerY -ge $Root.top -and $centerY -lt $Root.bottom)
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
    [DllImport("user32.dll")] private static extern IntPtr GetWindow(IntPtr wnd, uint what);
    [DllImport("user32.dll")] private static extern IntPtr GetDlgItem(IntPtr dialog, int id);
    [DllImport("user32.dll")] private static extern int GetDlgCtrlID(IntPtr wnd);
    [DllImport("user32.dll")] private static extern bool IsChild(IntPtr parent, IntPtr child);
    [DllImport("user32.dll")] private static extern bool IsWindowEnabled(IntPtr wnd);
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
    private static bool IsOwnedDashboardRoot(IntPtr wnd, uint expectedPid) {
        return wnd != IntPtr.Zero && expectedPid != 0 && OwnerPid(wnd) == expectedPid &&
            GetAncestor(wnd, 2) == wnd && // GA_ROOT
            String.Equals(WindowClass(wnd), "ClaudeUsageDashboard", StringComparison.Ordinal);
    }
    public static bool IsVisibleOwnedRoot(IntPtr wnd, uint expectedPid) {
        return IsOwnedDashboardRoot(wnd, expectedPid) && IsWindowVisible(wnd) && !IsIconic(wnd);
    }
    public static IntPtr[] DashboardCandidates(uint expectedPid) {
        return DashboardRoots(expectedPid, true);
    }
    public static IntPtr[] DashboardRoots(uint expectedPid, bool visibleOnly) {
        var candidates = new List<IntPtr>();
        EnumWindows(delegate(IntPtr wnd, IntPtr unused) {
            if (IsOwnedDashboardRoot(wnd, expectedPid) &&
                (!visibleOnly || (IsWindowVisible(wnd) && !IsIconic(wnd)))) {
                candidates.Add(wnd);
            }
            return true;
        }, IntPtr.Zero);
        // MainWindowHandle can be Tao's event target (or another auxiliary
        // window). Keep it diagnostic only; never show or probe those roots.
        return candidates.ToArray();
    }
    public static bool CanRelaunchDashboard(IntPtr wnd, uint expectedPid) {
        Rect rect;
        if (!IsOwnedDashboardRoot(wnd, expectedPid) ||
            !GetWindowRect(wnd, out rect) || rect.right <= rect.left || rect.bottom <= rect.top ||
            (IsWindowVisible(wnd) && !IsIconic(wnd))) return false;
        var roots = 0;
        EnumWindows(delegate(IntPtr h, IntPtr unused) {
            if (GetAncestor(h, 2) == h &&
                String.Equals(WindowClass(h), "ClaudeUsageDashboard", StringComparison.Ordinal)) roots++;
            return roots < 2;
        }, IntPtr.Zero);
        // The production second-open path does a plain FindWindowW by class.
        // Prove that it can only select the original owned root before invoking it.
        return roots == 1 && FindWindowW("ClaudeUsageDashboard", null) == wnd &&
            IsOwnedDashboardRoot(wnd, expectedPid);
    }
    public static string Describe(IntPtr wnd) {
        Rect rect;
        string bounds = GetWindowRect(wnd, out rect)
            ? String.Format("{0},{1},{2},{3}", rect.left, rect.top, rect.right, rect.bottom) : "<unavailable>";
        return String.Format("HWND={0}, class={1}, PID={2}, visible={3}, iconic={4}, root={5}, bounds={6}",
            wnd, WindowClass(wnd), OwnerPid(wnd), IsWindowVisible(wnd), IsIconic(wnd), GetAncestor(wnd, 2), bounds);
    }
    public static string DescribeOwned(uint expectedPid) {
        var windows = new List<string>();
        EnumWindows(delegate(IntPtr wnd, IntPtr unused) {
            if (OwnerPid(wnd) == expectedPid) windows.Add(Describe(wnd));
            return windows.Count < 12;
        }, IntPtr.Zero);
        return windows.Count == 0 ? "<none>" : String.Join("; ", windows);
    }
    [DllImport("user32.dll",CharSet=CharSet.Unicode,ExactSpelling=true)] static extern int GetWindowTextW(IntPtr hwnd,StringBuilder text,int n);
    private static bool IsModelDialog(IntPtr dialog, uint expectedPid) {
        var title=new StringBuilder(256);
        return OwnerPid(dialog)==expectedPid && WindowClass(dialog)=="#32770" &&
            GetWindowTextW(dialog,title,title.Capacity)>0 && title.ToString()=="Live Translate · English model";
    }
    public static IntPtr ModelDialog(uint expectedPid, IntPtr expectedOwner) {
        IntPtr result=IntPtr.Zero;
        EnumWindows(delegate(IntPtr wnd,IntPtr p) {
            if (IsModelDialog(wnd,expectedPid) && GetWindow(wnd,4)==expectedOwner) { // GW_OWNER
                result=wnd;return false;
            }
            return true;
        },IntPtr.Zero);
        return result;
    }
    public static IntPtr ConsentButton(IntPtr dialog, IntPtr expectedOwner, uint expectedPid, int id, bool visible) {
        if ((id!=6 && id!=7) || !IsModelDialog(dialog,expectedPid) ||
            GetWindow(dialog,4)!=expectedOwner || OwnerPid(expectedOwner)!=expectedPid) return IntPtr.Zero;
        IntPtr button=GetDlgItem(dialog,id);
        if (button==IntPtr.Zero || !IsChild(dialog,button) || OwnerPid(button)!=expectedPid ||
            GetDlgCtrlID(button)!=id || WindowClass(button)!="Button" || !IsWindowEnabled(button) ||
            (visible && (!IsWindowVisible(dialog) || !IsWindowVisible(button)))) return IntPtr.Zero;
        return button;
    }
    public static string DescribeConsent(IntPtr dialog) {
        var yes=GetDlgItem(dialog,6);
        var no=GetDlgItem(dialog,7);
        return String.Format("dialog=[{0}], owner={1}, IDYES=[{2}, enabled={3}], IDNO=[{4}, enabled={5}], foreground={6}",
            Describe(dialog),GetWindow(dialog,4),Describe(yes),IsWindowEnabled(yes),Describe(no),IsWindowEnabled(no),GetForegroundWindow());
    }
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr wnd,int cmd);
    [DllImport("user32.dll",ExactSpelling=true)] public static extern bool PostMessageW(IntPtr wnd,uint msg,IntPtr wp,IntPtr lp);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags,uint dx,uint dy,uint data,UIntPtr ext);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr wnd, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr wnd);
    [DllImport("user32.dll")] private static extern bool IsIconic(IntPtr wnd);
    [DllImport("user32.dll", CharSet=CharSet.Unicode, ExactSpelling=true)]
    private static extern IntPtr FindWindowW(string className, string windowName);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr wnd);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr wnd, IntPtr root, int x, int y, int w, int h, uint flags);
}
'@
    $app = Start-Process -FilePath $dashboard -WorkingDirectory $root -PassThru
    $timer = [Diagnostics.Stopwatch]::StartNew()
    [uint32]$launchPid = $app.Id
    $activation = 'not requested'
    $activatedRoot = [IntPtr]::Zero
    do {
        Start-Sleep -Milliseconds 250
        $app.Refresh()
        if ($app.HasExited) {
            $startup = Read-IsolatedStartupSummary $profile
            throw "Extracted dashboard exited early ($($app.ExitCode)). Startup: $startup"
        }
        $candidates = @([Sig4aPackageSmokeWin]::DashboardCandidates($launchPid))
        # If the original exact product root stays hidden, use the ordinary
        # second-open path once. Still require the original root and real UI.
        if ($candidates.Count -eq 0 -and $activation -eq 'not requested' -and $timer.Elapsed.TotalSeconds -ge 8) {
            $roots = @([Sig4aPackageSmokeWin]::DashboardRoots($launchPid, $false))
            if ($roots.Count -eq 1 -and [Sig4aPackageSmokeWin]::CanRelaunchDashboard($roots[0], $launchPid)) {
                $candidate = $roots[0]
                $details = [Sig4aPackageSmokeWin]::Describe($candidate)
                Write-Host "Existing dashboard confirmed before ordinary second open: $details"
                $reopen = Start-Process -FilePath $dashboard -WorkingDirectory $root -PassThru
                try {
                    if (!$reopen.WaitForExit(5000)) { throw 'Ordinary dashboard second-open did not return within five seconds.' }
                    if ($reopen.ExitCode -ne 0) { throw "Ordinary dashboard second-open returned $($reopen.ExitCode)." }
                    $app.Refresh()
                    if ($app.HasExited) { throw 'Original packaged dashboard exited during ordinary second-open.' }
                    $activation = "ordinary second-open completed for $details"
                    $activatedRoot = $candidate
                } finally {
                    if (!$reopen.HasExited) { $reopen.Kill($true); [void]$reopen.WaitForExit(3000) }
                    $reopen.Dispose()
                }
            }
        }
    } until ($candidates.Count -gt 0 -or $timer.Elapsed.TotalSeconds -gt 20)
    if ($candidates.Count -eq 0) {
        $main = [Sig4aPackageSmokeWin]::Describe($app.MainWindowHandle)
        $owned = [Sig4aPackageSmokeWin]::DescribeOwned($launchPid)
        $startup = Read-IsolatedStartupSummary $profile
        throw "Extracted dashboard has no visible owned ClaudeUsageDashboard root (launch PID=$launchPid; process main: $main; owned windows: $owned; native activation: $activation). Startup: $startup"
    }
    # A native HWND alone can be a dialog or a blank/framework window. Only pass
    # when the same visible owned root exposes canonical dashboard navigation.
    $uia = Join-Path $PSScriptRoot 'desktop-uia.ps1'
    $hwnd = [IntPtr]::Zero
    $observations = [ordered]@{}
    $expectedButtons = @('Live Translate','Claude Usage','Cài đặt')
    $examined = $null
    $placed = [Collections.Generic.HashSet[IntPtr]]::new()
    $timer.Restart()
    do {
        $app.Refresh()
        if ($app.HasExited) {
            $startup = Read-IsolatedStartupSummary $profile
            throw "Extracted dashboard exited before navigation was shown ($($app.ExitCode)). Startup: $startup"
        }
        $candidates = @([Sig4aPackageSmokeWin]::DashboardCandidates($launchPid))
        foreach ($h in $candidates) {
            if ($activatedRoot -ne [IntPtr]::Zero -and $h -ne $activatedRoot) { continue }
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
            $buttons = @($sample.items | Where-Object {Test-UiaButtonInRoot $_ $sample.root})
            $names = @($buttons | ForEach-Object name)
            $matched = @($expectedButtons | Where-Object {$_ -cin $names})
            $allNamed = @($sample.items | Where-Object {$_.name -cin $expectedButtons})
            $key = [string]($h.ToInt64())
            if ($observations.Contains($key) -or $observations.Count -lt 6) {
                $entry = $observations[$key]
                if ($null -eq $entry) {
                    $entry = @{window=$examined;attempts=0;bestScore=-1;best=$null;latest=$null}
                    $observations[$key] = $entry
                }
                $entry.attempts++
                $snapshot = @{
                    root=$sample.root; rootState=$sample.root_state; rootsRead=$sample.roots_read
                    scanErrors=$sample.scan_errors; itemCount=@($sample.items).Count
                    metRoleAndBounds=$matched
                    namedCandidates=@($allNamed | Select-Object -First 9 name,id,kind,enabled,offscreen,left,top,right,bottom)
                    sample=@($sample.items | Select-Object -First 8 @{Name='name';Expression={$_.name.Substring(0,[Math]::Min(96,$_.name.Length))}},kind,offscreen,left,top,right,bottom)
                }
                $score = 100000 * $matched.Count + 1000 * $allNamed.Count + @($sample.items).Count
                $entry.latest = $snapshot
                if ($score -gt $entry.bestScore) {
                    $entry.bestScore = $score
                    $entry.best = $snapshot
                }
            }
            if ($matched.Count -eq $expectedButtons.Count -and [Sig4aPackageSmokeWin]::IsVisibleOwnedRoot($h, $launchPid)) {
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
        $summary = @($observations.Values) | ConvertTo-Json -Depth 7 -Compress
        $startup = Read-IsolatedStartupSummary $profile
        $foreground = [Sig4aPackageSmokeWin]::GetForegroundWindow()
        throw "Extracted native dashboard navigation missing (launch PID=$launchPid; foreground=$foreground; process main: $main; owned: $owned; last examined: $examined). Per-dashboard UIA observations: $summary. Startup: $startup"
    }
    $verifiedWindow = [Sig4aPackageSmokeWin]::Describe($hwnd)
    "Native UIA found visible Live Translate, Claude Usage and Settings buttons within the extracted app root ($verifiedWindow). Startup activation: $activation."
    if (Get-Process -Name 'Sig4a.LiveTranslate' -ErrorAction SilentlyContinue) { throw 'Helper should not start merely on dashboard launch.' }
    'SIG4A archive verified: actual released dashboard, explicit inventory, invalid source no fallback, no passive helper/capture. No physical endpoints accessed.'
    if ($InstallEnglishFromUI) {
        & (Join-Path $PSScriptRoot 'smoke-english-install-ui.ps1') -Dashboard $app -Window $hwnd -Package $root -Uia $uia
    }
} finally {
    foreach ($p in @($probe,$app)) {
        if ($p) {
            if (!$p.HasExited) { $p.Kill($true); [void]$p.WaitForExit(5000) }
            $p.Dispose()
        }
    }
}
