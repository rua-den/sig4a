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
    $app = Start-Process -FilePath $dashboard -WorkingDirectory $root -PassThru
    $timer = [Diagnostics.Stopwatch]::StartNew()
    do {
        Start-Sleep -Milliseconds 250
        $app.Refresh()
        if ($app.HasExited) { throw "Extracted dashboard exited early ($($app.ExitCode))." }
    } until ($app.MainWindowHandle -ne [IntPtr]::Zero -or $timer.Elapsed.TotalSeconds -gt 20)
    if (!$app.MainWindowHandle) { throw 'Extracted ordinary dashboard did not show its native window.' }
    # A native HWND alone can be blank. Confirm the actual ordinary WebView2
    # navigation contains the experimental entry and existing tools.
    $uia = Join-Path $PSScriptRoot 'desktop-uia.ps1'
    $seen = $false
    $timer.Restart()
    do {
        $raw = powershell.exe -NoProfile -File $uia -Hwnd ($app.MainWindowHandle.ToInt64()) -Name 'Live Translate|Claude Usage|Tool Box' -Mode list
        if ($LASTEXITCODE -ne 0) { throw 'Native dashboard UI automation failed.' }
        $names = @((($raw -join "`n") | ConvertFrom-Json).items | Where-Object {!$_.offscreen} | ForEach-Object name)
        $seen = ($names -match 'Live Translate') -and ($names -match 'Claude Usage')
        if (!$seen) { Start-Sleep -Milliseconds 250 }
    } until ($seen -or $timer.Elapsed.TotalSeconds -gt 22)
    if (!$seen) { throw 'Extracted dashboard has no visible Live Translate and Claude Usage navigation.' }
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
