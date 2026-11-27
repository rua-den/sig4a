# Hosted Windows smoke of the ordinary extracted package. Does not open a physical endpoint.
param([Parameter(Mandatory)][string]$Archive, [string]$RealGemmaManifestSha256 = "")
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
$refusalListener = $null
$proofDaemon = $null
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
    # Production refusal proof: a never-authorized ephemeral loopback endpoint. No e2e flags or fake code.
    $refusalListener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    $refusalListener.Start()
    $data = Join-Path $env:LOCALAPPDATA 'claude-usage'
    $hooks = Join-Path $env:USERPROFILE '.claude'
    New-Item -ItemType Directory -Force $data,$hooks | Out-Null
    $hookConfig = Join-Path $hooks 'settings.json'
    Set-Content -LiteralPath $hookConfig -Value '{}' -Encoding utf8NoBOM
    $fixtureConfig = Join-Path $data 'config.json'
    $port = ([Net.IPEndPoint]$refusalListener.LocalEndpoint).Port
    if ($port -eq 11434) { throw 'Ephemeral refusal fixture unexpectedly used reserved Ollama port.' }
    @{profiles=@(@{name='fixture';path=$hooks});autostart=$false;local_ai_backend='ollama';local_ai_endpoint="http://127.0.0.1:$port";local_ai_model='gemma4:e2b';local_ai_managed=$false} |
        ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $fixtureConfig -Encoding utf8NoBOM
    $fixtureHash = (Get-FileHash -LiteralPath $fixtureConfig).Hash
    $hookHash = (Get-FileHash -LiteralPath $hookConfig).Hash
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
    function Wait-PackageUia([string]$Pattern, [string]$Mode, [string]$Kind='', [int]$Seconds=12) {
        $clock = [Diagnostics.Stopwatch]::StartNew()
        do {
            $uiaArgs = @('-NoProfile','-STA','-File',$uia,'-Hwnd',"$($hwnd.ToInt64())",'-Name',$Pattern,'-Mode',$Mode)
            if ($Kind) { $uiaArgs += @('-Kind',$Kind) }
            $raw = & powershell.exe @uiaArgs
            if ($LASTEXITCODE -ne 0) { throw 'Native Quick Check UI automation failed.' }
            $item = ($raw -join "`n") | ConvertFrom-Json
            if ($item.found) { return $item }
            Start-Sleep -Milliseconds 200
        } until ($clock.Elapsed.TotalSeconds -gt $Seconds)
        throw "Native Quick Check UI did not reach expected control: $Pattern ($Mode)"
    }
    function Assert-PackageRefusalBoundary([string]$Phase) {
        if ($refusalListener.Pending()) { throw "$Phase connected to the unauthorized endpoint." }
        if ((Get-FileHash -LiteralPath $fixtureConfig).Hash -ne $fixtureHash -or
            (Get-FileHash -LiteralPath $hookConfig).Hash -ne $hookHash -or
            (Test-Path -LiteralPath (Join-Path $data 'notify_test.flag'))) {
            throw "$Phase modified configuration, a hook, or the notification flag."
        }
        if (Get-Process -Name 'Sig4a.LiveTranslate' -ErrorAction SilentlyContinue) { throw "$Phase started the Live Translate helper." }
    }
    Wait-PackageUia '^Cài đặt$' 'invoke' 'Button' | Out-Null
    Wait-PackageUia '^Kiểm tra nhanh$' 'invoke' 'Button' | Out-Null
    Wait-PackageUia '^Chưa kiểm tra[.]$' 'state' | Out-Null
    Start-Sleep -Milliseconds 500
    Assert-PackageRefusalBoundary 'Quick Check opening'
    Wait-PackageUia '^Làm mới trạng thái$' 'invoke' 'Button' | Out-Null
    Wait-PackageUia '^Đã làm mới; có trạng thái chưa xác minh[.]$' 'state' | Out-Null
    Wait-PackageUia '^Thử Translation$' 'invoke' 'Button' | Out-Null
    Wait-PackageUia 'Mẫu thử chỉ hỗ trợ Ollama chính thức trên cổng mặc định' 'state' | Out-Null
    Assert-PackageRefusalBoundary 'Refresh and fixed model sample refusal'
    'Ordinary Quick Check Settings route verified: passive entry and real refusal with no connection/config write/hook/flag/helper.'
    if ($RealGemmaManifestSha256) {
        $app.Kill($true); [void]$app.WaitForExit(5000); $app.Dispose(); $app = $null
        . (Join-Path $PSScriptRoot '../quick-check/signed-ollama-proof.ps1')
        $proofDaemon = Start-SignedOllamaProof -Scratch (Join-Path $profile 'ollama-proof') -ManifestSha256 $RealGemmaManifestSha256
        $config = Get-Content -LiteralPath $fixtureConfig -Raw | ConvertFrom-Json -AsHashtable
        $config.local_ai_endpoint = 'http://127.0.0.1:11434'
        $config | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $fixtureConfig -Encoding utf8NoBOM
        $fixtureHash = (Get-FileHash -LiteralPath $fixtureConfig).Hash
        $app = Start-Process -FilePath $dashboard -WorkingDirectory $root -PassThru
        $timer.Restart()
        $hwnd = [IntPtr]::Zero
        do {
            Start-Sleep -Milliseconds 200
            $app.Refresh()
            if ($app.HasExited) { throw 'Extracted proof dashboard exited before reopening.' }
            $hwnd = [Sig4aPackageSmokeWin]::FindDashboard([uint32]$app.Id, $app.MainWindowHandle)
        } until ($hwnd -ne [IntPtr]::Zero -or $timer.Elapsed.TotalSeconds -gt 20)
        if ($hwnd -eq [IntPtr]::Zero) { throw 'Extracted proof dashboard did not become visible.' }
        [void][Sig4aPackageSmokeWin]::SetWindowPos($hwnd, [IntPtr]::Zero, 24, 24, 900, 640, 0x0014)
        [void][Sig4aPackageSmokeWin]::SetForegroundWindow($hwnd)
        Wait-PackageUia '^Cài đặt$' 'invoke' 'Button' | Out-Null
        Wait-PackageUia '^Kiểm tra nhanh$' 'invoke' 'Button' | Out-Null
        foreach ($label in @('Translation','Grammar')) {
            Wait-PackageUia ("^Thử $label$") 'invoke' 'Button' | Out-Null
            $state = Wait-PackageUia ("^$label.+Mẫu khớp[.] Ollama báo Cloud đã tắt[.]") 'state' '' 36
            Write-Host "PRODUCTION_SAMPLE_MATCH: $($state.name)"
        }
        if ($proofDaemon.HasExited) { throw 'The same signed Ollama daemon was not retained across the package samples.' }
        Assert-PackageRefusalBoundary 'Signed package Quick Check actions'
    } else {
        'REAL_SIGNED_OLLAMA_GEMMA_NOT_RUN: optional job needs independently reviewed exact model digest.'
    }
    'SIG4A archive verified: actual released dashboard, explicit inventory, invalid source no fallback, no passive helper/capture. No physical endpoints accessed.'
} finally {
    if ($refusalListener) { $refusalListener.Stop() }
    foreach ($p in @($probe,$app,$proofDaemon)) {
        if ($p) {
            if (!$p.HasExited) { $p.Kill($true); [void]$p.WaitForExit(5000) }
            $p.Dispose()
        }
    }
}
