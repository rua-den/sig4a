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
using System.Runtime.InteropServices;
using System.Text;
public static class Sig4aPackageSmokeWin {
    private delegate bool EnumWindowsCallback(IntPtr wnd, IntPtr param);
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumWindowsCallback callback, IntPtr param);
    [DllImport("user32.dll", CharSet=CharSet.Unicode, ExactSpelling=true)]
    private static extern int GetClassNameW(IntPtr wnd, StringBuilder name, int capacity);
    private static bool IsVisibleDashboard(IntPtr wnd, uint expectedPid) {
        if (wnd == IntPtr.Zero || !IsWindowVisible(wnd)) return false;
        uint ownerPid;
        if (GetWindowThreadProcessId(wnd, out ownerPid) == 0 || ownerPid != expectedPid) return false;
        var name = new StringBuilder(256);
        return GetClassNameW(wnd, name, name.Capacity) > 0 &&
            String.Equals(name.ToString(), "ClaudeUsageDashboard", StringComparison.Ordinal);
    }
    public static IntPtr FindDashboard(uint expectedPid, IntPtr mainWindow) {
        if (IsVisibleDashboard(mainWindow, expectedPid)) return mainWindow;
        IntPtr result = IntPtr.Zero;
        EnumWindows(delegate(IntPtr wnd, IntPtr unused) {
            if (!IsVisibleDashboard(wnd, expectedPid)) return true;
            result = wnd;
            return false;
        }, IntPtr.Zero);
        return result;
    }
    [DllImport("user32.dll",CharSet=CharSet.Unicode,ExactSpelling=true)] static extern int GetWindowTextW(IntPtr hwnd,StringBuilder text,int n);
    public static IntPtr ModelDialog(uint expectedPid) {
        IntPtr result=IntPtr.Zero;
        EnumWindows(delegate(IntPtr wnd,IntPtr p) {
            uint owner;
            if (GetWindowThreadProcessId(wnd,out owner)==0 || owner!=expectedPid) return true;
            var classname=new StringBuilder(128);
            var title=new StringBuilder(256);
            if (GetClassNameW(wnd,classname,classname.Capacity)>0 && classname.ToString()=="#32770" &&
                GetWindowTextW(wnd,title,title.Capacity)>0 && title.ToString()=="Live Translate · English model") {
                result=wnd;return false;
            }
            return true;
        },IntPtr.Zero);
        return result;
    }
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr wnd,int cmd);
    [DllImport("user32.dll",ExactSpelling=true)] public static extern bool PostMessageW(IntPtr wnd,uint msg,IntPtr wp,IntPtr lp);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags,uint dx,uint dy,uint data,UIntPtr ext);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr wnd, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr wnd);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr wnd);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr wnd, IntPtr root, int x, int y, int w, int h, uint flags);
}
'@
    $app = Start-Process -FilePath $dashboard -WorkingDirectory $root -PassThru
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $hwnd = [IntPtr]::Zero
    do {
        Start-Sleep -Milliseconds 250
        $app.Refresh()
        if ($app.HasExited) { throw "Extracted dashboard exited early ($($app.ExitCode))." }
        $hwnd = [Sig4aPackageSmokeWin]::FindDashboard([uint32]$app.Id, $app.MainWindowHandle)
    } until ($hwnd -ne [IntPtr]::Zero -or $timer.Elapsed.TotalSeconds -gt 20)
    if ($hwnd -eq [IntPtr]::Zero) { throw ("Extracted ordinary dashboard did not show a visible owned ClaudeUsageDashboard window (launch PID={0}, process main HWND={1})." -f $app.Id,$app.MainWindowHandle) }
    [void][Sig4aPackageSmokeWin]::SetWindowPos($hwnd, [IntPtr]::Zero, 24, 24, 900, 640, 0x0014)
    [void][Sig4aPackageSmokeWin]::SetForegroundWindow($hwnd)
    # A native HWND alone can be blank. Confirm the actual ordinary WebView2
    # navigation contains the experimental entry and a pre-existing tool.
    $uia = Join-Path $PSScriptRoot 'desktop-uia.ps1'
    $seen = $false
    $sample = $null
    $timer.Restart()
    do {
        $raw = & powershell.exe -NoProfile -STA -File $uia -Hwnd "$($hwnd.ToInt64())" -Mode list
        if ($LASTEXITCODE -ne 0) { throw 'Native dashboard UI automation failed.' }
        $sample = ($raw -join "`n") | ConvertFrom-Json
        $nodes = @($sample.items | Where-Object {!$_.offscreen -and $_.right -gt $_.left -and $_.bottom -gt $_.top})
        $seen = (@($nodes | Where-Object name -eq 'Live Translate').Count -gt 0) -and
                (@($nodes | Where-Object name -eq 'Claude Usage').Count -gt 0)
        if (!$seen) { Start-Sleep -Milliseconds 350 }
    } until ($seen -or $timer.Elapsed.TotalSeconds -gt 28)
    if (!$seen) {
        $foreground = [Sig4aPackageSmokeWin]::GetForegroundWindow() -eq $hwnd
        $summary = @($sample.items | Select-Object -First 14 name,kind,offscreen) | ConvertTo-Json -Compress
        throw "Extracted native dashboard navigation missing (foreground=$foreground): $summary"
    }
    'Native UIA found visible Live Translate and Claude Usage in the extracted app.'
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
