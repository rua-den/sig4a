# Isolated hosted Windows only. This sends synthetic sound to default playback;
# never run against a user's computer or an active meeting. Never opens a mic.
param(
    [Parameter(Mandatory)][string]$Published,
    [Parameter(Mandatory)][string]$Scratch,
    [switch]$ExpectSample,
    [switch]$RequirePlayback
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$base = [IO.Path]::GetFullPath($Published)
$dir = [IO.Path]::GetFullPath($Scratch)
$exe = Join-Path $base 'Sig4a.LiveTranslate.exe'
if ((Test-Path -LiteralPath $dir) -or $dir -eq $base) { throw 'Smoke scratch must be new and isolated.' }
if ((Test-Path -LiteralPath "$base/models/zipformer") -ne $ExpectSample.IsPresent) {
    throw 'Use an unprovisioned preview for meter-only test or -ExpectSample after EN provisioning.'
}

$start = [Diagnostics.ProcessStartInfo]::new($exe)
$start.UseShellExecute = $false
$start.RedirectStandardInput = $true
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
$child = [Diagnostics.Process]::Start($start)
$play = $null
$discardErrors = $child.StandardError.BaseStream.CopyToAsync([IO.Stream]::Null)
function Send($value) {
    $child.StandardInput.WriteLine(($value | ConvertTo-Json -Compress))
    $child.StandardInput.Flush()
}
function Next([int]$wait = 3000) {
    $line = $child.StandardOutput.ReadLineAsync()
    if (!$line.Wait($wait) -or !$line.Result) { throw 'Timed out waiting for bounded helper protocol.' }
    return $line.Result | ConvertFrom-Json -AsHashtable
}
function Begin([long]$id) {
    Send ([ordered]@{ v=1; type='check_start'; session=$id; source='playback'; device_id=$null })
    $started = Next
    if ($started.type -ne 'check_status' -or $started.state -ne 'started' -or $started.session -ne $id) {
        throw 'A known isolated default playback endpoint could not start.'
    }
    return [Diagnostics.Stopwatch]::StartNew()
}
try {
    Send @{v=1;type='list'}
    $inventory = Next
    if ($inventory.type -ne 'devices' -or
        !@($inventory.devices | Where-Object { $_.source -eq 'playback' -and $_.is_default }).Count) {
        if ($RequirePlayback) { throw 'Isolated hosted Windows has no usable default playback endpoint.' }
        Write-Host 'SKIP: no active default playback endpoint. No native capture result.'
        return
    }

    [void][IO.Directory]::CreateDirectory($dir)
    $fixture = Join-Path $dir 'audio-check-fixture.wav'
    powershell.exe -NoProfile -File "$PSScriptRoot/tests/New-SyntheticFixture.ps1" -Output $fixture
    if ($LASTEXITCODE -ne 0) { throw 'Controlled speech fixture unavailable.' }

    # A stale prior generation cannot stop it, and normal capture cannot preempt it.
    $clock = Begin 2
    Send @{v=1;type='check_stop';session=1}
    Send @{v=1;type='start';session=3;source='playback';device_id=$null;translate=$false}
    $speech = [Diagnostics.ProcessStartInfo]::new('powershell.exe')
    $speech.UseShellExecute = $false
    $speech.CreateNoWindow = $true
    $speech.Environment['SIG4A_SMOKE_FIXTURE'] = $fixture
    foreach ($arg in @('-NoProfile','-NonInteractive','-Command',
            '$p=New-Object System.Media.SoundPlayer -ArgumentList $env:SIG4A_SMOKE_FIXTURE; $p.PlaySync()')) {
        $speech.ArgumentList.Add($arg)
    }
    $play = [Diagnostics.Process]::Start($speech)
    $heard = $false
    $meter = $false
    $busy = $false
    $sample = $false
    $model = $null
    $complete = $false
    while ($clock.ElapsedMilliseconds -lt 6500) {
        $event = Next 3500
        if ($event.type -eq 'error' -and $event.session -eq 3 -and $event.code -eq 'busy') { $busy = $true; continue }
        if ($event.session -ne 2) { throw 'A result escaped its active audio-check session.' }
        switch ($event.type) {
            'check_meter' {
                if ($event.level -lt 0 -or $event.level -gt 100 -or $event.signal -isnot [bool]) { throw 'Invalid meter value.' }
                if ($event.level -gt 0) { $meter = $true }
                $heard = $heard -or $event.signal
            }
            'check_model' {
                if ($null -ne $model) { throw 'Unexpected duplicate model state.' }
                $model = $event.state
            }
            'check_sample' {
                if (!$ExpectSample -or $sample -or [string]::IsNullOrWhiteSpace($event.en)) { throw 'Invalid short English sample.' }
                $sample = $true
            }
            'check_status' {
                if ($event.state -ne 'stopped' -or $event.completed -ne $true) { throw 'Expected natural window release.' }
                $complete = $true
            }
            default { throw 'Unexpected IPC result during synthetic check.' }
        }
        if ($complete) { break }
    }
    if (!$complete -or !$busy -or !$meter -or !$heard -or $clock.ElapsedMilliseconds -lt 4400 -or $clock.ElapsedMilliseconds -gt 6500) {
        throw 'Synthetic audio, started/stopped ACK window, exclusivity or aggregate signal proof did not pass.'
    }
    if ($ExpectSample -and ($model -ne 'ready' -or !$sample)) { throw 'Native optional English did not complete inside capture window.' }
    if (!$ExpectSample -and ($model -ne 'unavailable' -or $sample)) { throw 'Meter-only check did not report the missing English model.' }
    # In a quiet second pass stop immediately. Require release ACK and no starts.
    $clock = Begin 4
    Send @{v=1;type='check_stop';session=4}
    do { $event = Next } until ($event.type -eq 'check_status' -or $clock.ElapsedMilliseconds -gt 2400)
    if ($event.type -ne 'check_status' -or $event.state -ne 'stopped' -or $event.completed -ne $false -or
        $event.session -ne 4 -or $clock.ElapsedMilliseconds -gt 2400) { throw 'Explicit Stop lacked a prompt release ACK.' }

    $child.StandardInput.Close()
    if (!$child.WaitForExit(3500) -or $child.ExitCode -ne 0) { throw 'Helper did not exit on EOF.' }
    Write-Host 'PASS: real hosted WASAPI render-loopback with synthetic output, stale stop and busy guards, meter, auto- and explicit Stop. No physical-device evidence.'
} finally {
    if ($play) {
        if (!$play.HasExited) { $play.Kill($true) }
        $play.Dispose()
    }
    if (!$child.HasExited) { $child.Kill($true) }
    $child.Dispose()
}
