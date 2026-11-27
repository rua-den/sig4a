# Ordinary extracted win-x64 preview, isolated hosted interactive Windows only.
# Invoked by smoke-package.ps1, which owns the dashboard HWND and workspace.
param([Diagnostics.Process]$Dashboard,[IntPtr]$Window,[string]$Package,[string]$Uia)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$helper = Join-Path $Package 'live-translate'
$models = Join-Path $helper 'models'
$pointer = Join-Path $models 'zipformer.current'
$trace = 'sig4a-english-model-start-' + [guid]::NewGuid().ToString('N')

function Wait-Until([scriptblock]$Check,[int]$Seconds,[string]$Failure) {
    $time = [Diagnostics.Stopwatch]::StartNew()
    do {
        if ($Dashboard.HasExited) { throw 'Ordinary extracted dashboard exited during installer smoke.' }
        $value = & $Check
        if ($value) { return $value }
        Start-Sleep -Milliseconds 180
    } while ($time.Elapsed.TotalSeconds -lt $Seconds)
    throw $Failure
}
function Node([long]$Parent,[string]$Id,[string]$Name='', [string]$Kind='',[string]$Mode='point') {
    $raw = & powershell.exe -NoProfile -STA -File $Uia -Hwnd "$Parent" -Id $Id -Name $Name -Kind $Kind -Mode $Mode
    if ($LASTEXITCODE -ne 0) { throw 'Real WebView/UIA query failed.' }
    return ($raw -join "`n") | ConvertFrom-Json
}
function Click([IntPtr]$Parent,[string]$Id,[string]$Name='') {
    $control = Wait-Until {
        $typed = Node ($Parent.ToInt64()) $Id $Name 'Button'
        if ($typed.found -and $typed.enabled -and !$typed.offscreen) { $typed }
    } 18 "Control is unavailable: $Id"
    [void][Sig4aPackageSmokeWin]::SetForegroundWindow($Parent)
    [void][Sig4aPackageSmokeWin]::SetCursorPos($control.x,$control.y)
    [Sig4aPackageSmokeWin]::mouse_event(0x0002,0,0,0,[UIntPtr]::Zero)
    [Sig4aPackageSmokeWin]::mouse_event(0x0004,0,0,0,[UIntPtr]::Zero)
}
function Dialog() {
    Wait-Until {
        $test = [Sig4aPackageSmokeWin]::ModelDialog([uint32]$Dashboard.Id)
        if ($test -ne [IntPtr]::Zero) { $test }
    } 18 'Native pinned English model consent dialog missing.'
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
    Click $Window '' '^Live Translate$'
    # Merely opening the page performs no launch, model access, or install.
    Ensure-NoSetup
    Click $Window 'live-model-install' '^Cài model tiếng Anh$'
    $declined = Dialog
    Click $declined '7' '^No$'
    [void](Wait-Until { [Sig4aPackageSmokeWin]::ModelDialog([uint32]$Dashboard.Id) -eq [IntPtr]::Zero } 8 'Decline did not close modal.')
    Ensure-NoSetup
    'PASS: opening Live Translate and declining native consent did not launch an installer or stage assets.'

    # Stale ownership regression: invalidate the actual owned renderer while
    # the dialog is open. Reply YES only to the verified same-PID native dialog.
    Click $Window 'live-model-install' '^Cài model tiếng Anh$'
    $stale = Dialog
    [void][Sig4aPackageSmokeWin]::ShowWindow($Window,0)
    if ([Sig4aPackageSmokeWin]::IsWindowVisible($Window)) { throw 'Fixture could not hide the ordinary owned renderer.' }
    if (![Sig4aPackageSmokeWin]::PostMessageW($stale,0x0111,[IntPtr]6,[IntPtr]::Zero)) { throw 'Fixture could not accept invalidated native modal.' }
    [void](Wait-Until { [Sig4aPackageSmokeWin]::ModelDialog([uint32]$Dashboard.Id) -eq [IntPtr]::Zero } 8 'Stale dialog did not close.')
    Ensure-NoSetup
    [void][Sig4aPackageSmokeWin]::ShowWindow($Window,9)
    [void][Sig4aPackageSmokeWin]::SetForegroundWindow($Window)
    'PASS: accepting after ordinary renderer disappearance did not start the setup process.'

    Click $Window 'live-model-install' '^Cài model tiếng Anh$'
    $accepted = Dialog
    Click $accepted '6' '^Yes$'
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
