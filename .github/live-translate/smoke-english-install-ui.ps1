# Ordinary extracted win-x64 preview, isolated hosted interactive Windows only.
# Invoked by smoke-package.ps1, which owns the dashboard HWND and workspace.
param([Diagnostics.Process]$Dashboard,[IntPtr]$Window,[string]$Package,[string]$Uia)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$helper = Join-Path $Package 'live-translate'
$models = Join-Path $helper 'models'
$pointer = Join-Path $models 'zipformer.current'
$trace = 'sig4a-english-model-start-' + [guid]::NewGuid().ToString('N')

function Wait-Until([scriptblock]$Check,[int]$Seconds,[string]$Failure,[scriptblock]$Describe) {
    $time = [Diagnostics.Stopwatch]::StartNew()
    do {
        if ($Dashboard.HasExited) { throw 'Ordinary extracted dashboard exited during installer smoke.' }
        $value = & $Check
        if ($value) { return $value }
        Start-Sleep -Milliseconds 180
    } while ($time.Elapsed.TotalSeconds -lt $Seconds)
    if ($Describe) {
        try { $detail = [string](& $Describe) } catch { $detail = '<diagnostic unavailable>' }
        $Failure += ' Context: ' + $detail.Substring(0, [Math]::Min($detail.Length, 1200))
    }
    throw $Failure
}
function Node([long]$Parent,[string]$Id,[string]$Name='', [string]$Kind='',[string]$Mode='point') {
    # Bound each child query as well as the aggregate wait. UIA can block if a
    # window closes while the provider is walking the accessibility tree.
    $start = [Diagnostics.ProcessStartInfo]::new('powershell.exe')
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($arg in @('-NoProfile','-STA','-File',$Uia,'-Hwnd',"$Parent",'-Mode',$Mode)) {
        [void]$start.ArgumentList.Add($arg)
    }
    foreach ($entry in @(@('-Id',$Id),@('-Name',$Name),@('-Kind',$Kind))) {
        if ($entry[1]) { [void]$start.ArgumentList.Add($entry[0]); [void]$start.ArgumentList.Add($entry[1]) }
    }
    $proc = [Diagnostics.Process]::Start($start)
    try {
        $output = $proc.StandardOutput.ReadToEndAsync()
        $stderr = $proc.StandardError.ReadToEndAsync()
        if (!$proc.WaitForExit(4500)) {
            try { $proc.Kill() } catch { if (!$proc.HasExited) { throw } }
            return @{found=$false;timed_out=$true}
        }
        if ($proc.ExitCode -ne 0) { throw "Real WebView/UIA query failed (exit=$($proc.ExitCode))." }
        return $output.GetAwaiter().GetResult() | ConvertFrom-Json
    } finally { $proc.Dispose() }
}
function Click-WebViewButton([string]$Id,[string]$Name='') {
    [void][Sig4aPackageSmokeWin]::SetForegroundWindow($Window)
    $control = Wait-Until {
        if (![Sig4aPackageSmokeWin]::IsVisibleOwnedRoot($Window,[uint32]$Dashboard.Id)) { return }
        $typed = Node ($Window.ToInt64()) $Id $Name 'Button'
        if ($typed.found -and $typed.enabled -and !$typed.offscreen) { $typed }
    } 18 "WebView button is unavailable: $Id $Name" {
        $state = Node ($Window.ToInt64()) $Id $Name 'Button' 'state'
        $windowSummary = [Sig4aPackageSmokeWin]::Describe($Window)
        "root=[$windowSummary]; foreground=$([Sig4aPackageSmokeWin]::GetForegroundWindow()); found=$($state.found); enabled=$($state.enabled); offscreen=$($state.offscreen); timedOut=$($state.timed_out); point=$($state.x),$($state.y)"
    }
    [void][Sig4aPackageSmokeWin]::SetForegroundWindow($Window)
    if (![Sig4aPackageSmokeWin]::SetCursorPos($control.x,$control.y)) { throw 'Could not place the cursor on the real WebView button.' }
    [Sig4aPackageSmokeWin]::mouse_event(0x0002,0,0,0,[UIntPtr]::Zero)
    [Sig4aPackageSmokeWin]::mouse_event(0x0004,0,0,0,[UIntPtr]::Zero)
}
function Dialog {
    Wait-Until {
        $test = [Sig4aPackageSmokeWin]::ModelDialog([uint32]$Dashboard.Id,$Window)
        if ($test -ne [IntPtr]::Zero) { $test }
    } 18 'Native pinned English model consent dialog missing.' {
        "foreground=$([Sig4aPackageSmokeWin]::GetForegroundWindow()); owned=$([Sig4aPackageSmokeWin]::DescribeOwned([uint32]$Dashboard.Id))"
    }
}
function Answer-NativeConsent([IntPtr]$Dialog,[int]$ControlId,[switch]$StaleOwner) {
    # HWND and child IDs are Win32 MessageBox semantics, not UIA AutomationId
    # or localized accessible labels. The latter may be absent or localized.
    if (!$StaleOwner) { [void][Sig4aPackageSmokeWin]::SetForegroundWindow($Dialog) }
    $button = Wait-Until {
        $child = [Sig4aPackageSmokeWin]::ConsentButton($Dialog,$Window,[uint32]$Dashboard.Id,$ControlId,(!$StaleOwner))
        if ($child -ne [IntPtr]::Zero -and ($StaleOwner -or [Sig4aPackageSmokeWin]::GetForegroundWindow() -eq $Dialog)) { $child }
    } 8 "Verified native consent button $ControlId is unavailable." {
        [Sig4aPackageSmokeWin]::DescribeConsent($Dialog)
    }
    if ($StaleOwner) {
        # Deliberately inject the real BN_CLICKED notification after hiding the
        # renderer; do not re-activate it merely to exercise the stale case.
        $sent = [Sig4aPackageSmokeWin]::PostMessageW($Dialog,0x0111,[IntPtr]$ControlId,$button)
    } else {
        # BM_CLICK activates the actual native button and notifies MessageBox.
        # The app still receives IDNO or IDYES through its production modal.
        $sent = [Sig4aPackageSmokeWin]::PostMessageW($button,0x00f5,[IntPtr]::Zero,[IntPtr]::Zero)
    }
    if (!$sent) { throw "Could not activate verified native consent button $ControlId. $([Sig4aPackageSmokeWin]::DescribeConsent($Dialog))" }
    [void](Wait-Until {
        [Sig4aPackageSmokeWin]::ModelDialog([uint32]$Dashboard.Id,$Window) -eq [IntPtr]::Zero
    } 8 "Native consent button $ControlId did not close the modal." {
        [Sig4aPackageSmokeWin]::DescribeConsent($Dialog)
    })
}
function Ensure-NoSetup() {
    # A mode process is prerequisite for any English setup egress. WMI collects
    # start events rather than just sampling today's process list.
    Start-Sleep -Milliseconds 3200
    if (@(Get-Event -SourceIdentifier $trace -ErrorAction SilentlyContinue).Count -ne 0 -or
        (Test-Path -LiteralPath $models)) { throw 'No-consent/stale-view install side effect found.' }
}

# Use actual process observation with no logs containing raw URLs or audio.
$subscription = Register-CimIndicationEvent -Namespace 'root/cimv2' -SourceIdentifier $trace -Query "SELECT * FROM Win32_ProcessStartTrace WHERE ProcessName = 'Sig4a.LiveTranslate.exe'"
try {
    Click-WebViewButton '' '^Live Translate$'
    # Merely opening the page performs no launch, model access, or install.
    Ensure-NoSetup
    Click-WebViewButton 'live-model-install' '^Cài model tiếng Anh$'
    $declined = Dialog
    Answer-NativeConsent $declined 7
    Ensure-NoSetup
    'PASS: opening Live Translate and declining native consent did not launch an installer or stage assets.'

    # Stale ownership regression: invalidate the actual owned renderer while
    # the dialog is open. Reply YES only to the verified same-PID native dialog.
    Click-WebViewButton 'live-model-install' '^Cài model tiếng Anh$'
    $stale = Dialog
    [void][Sig4aPackageSmokeWin]::ShowWindow($Window,0)
    if ([Sig4aPackageSmokeWin]::IsWindowVisible($Window)) { throw 'Fixture could not hide the ordinary owned renderer.' }
    Answer-NativeConsent $stale 6 -StaleOwner
    Ensure-NoSetup
    [void][Sig4aPackageSmokeWin]::ShowWindow($Window,9)
    [void][Sig4aPackageSmokeWin]::SetForegroundWindow($Window)
    'PASS: accepting after ordinary renderer disappearance did not start the setup process.'

    Click-WebViewButton 'live-model-install' '^Cài model tiếng Anh$'
    $accepted = Dialog
    Answer-NativeConsent $accepted 6
    [void](Wait-Until {
        $report = Node ($Window.ToInt64()) '' 'Model tiếng Anh đã được xác minh và sẵn sàng' '' 'state'
        if ($report.found -and $report.name -match '(?i)sẵn sàng|đã xác minh' -and (Test-Path -LiteralPath $pointer -PathType Leaf)) { $true }
    } 780 'Ordinary user-initiated pinned model install did not produce verified Ready.')
    if (@(Get-Event -SourceIdentifier $trace -ErrorAction SilentlyContinue).Count -ne 1) {
        throw 'Expected exactly one true setup helper launch after valid consent.'
    }
    if (Test-Path -LiteralPath (Join-Path $models 'zipformer')) { throw 'Model setup wrote or coexisted with legacy slot.' }
    $raw = [IO.File]::ReadAllText($pointer)
    if ($raw -cnotmatch '\ASIG4A-EN-1 ([0-9a-f]{32})\n\z') { throw 'Unexpected committed marker.' }
    $slot = ".sig4a-en-" + $Matches[1]
    $payload = Join-Path $models "$slot/payload"
    $pins = Get-Content -Raw -LiteralPath (Join-Path $helper 'model-assets.json') | ConvertFrom-Json -AsHashtable
    foreach ($name in $pins.files.Keys) {
        $file = Join-Path $payload $name
        if ((Get-Item -LiteralPath $file).Length -ne $pins.member_bytes[$name] -or
            (Get-FileHash -Algorithm SHA256 -LiteralPath $file).Hash -ine $pins.files[$name]) {
            throw 'Extracted dashboard install did not commit pinned bytes.'
        }
    }
    # Consume this exact install with the product recognizer in a separate
    # unshipped fixture. No Start/Mic button click and no endpoint capture.
    $proof = Join-Path $PSScriptRoot '../../tools/live_translate_runtime/Inference-Smoke.ps1'
    & $proof -Published $helper -Scratch (Join-Path $env:RUNNER_TEMP ('sig4a-gui-installed-inference-' + [guid]::NewGuid().ToString('N')))
    'PASS: actual extracted dashboard consent -> exact publisher download -> atomic verified Ready -> true offline synthetic English inference. No physical microphone or meeting used.'
} finally {
    [void][Sig4aPackageSmokeWin]::ShowWindow($Window,9)
    Unregister-Event -SourceIdentifier $trace -ErrorAction SilentlyContinue
    Remove-Event -SourceIdentifier $trace -ErrorAction SilentlyContinue
    if ($subscription -is [IDisposable]) { $subscription.Dispose() }
}
