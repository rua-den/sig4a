# Optional manual/CI wrapper. Invoking it is the explicit request to install the
# same pinned English model (~310 MB). Users can instead use Live Translate UI.
$ErrorActionPreference = 'Stop'
$exe = Join-Path $PSScriptRoot 'Sig4a.LiveTranslate.exe'
if (!(Test-Path -LiteralPath $exe -PathType Leaf)) {
    throw 'Run this from the live-translate directory of the extracted SIG4A package.'
}
Write-Host 'Installing the pinned English model (~310 MB download, ~72.5 MB retained). No room audio or inference is started.'
$s = [Diagnostics.ProcessStartInfo]::new($exe)
$s.UseShellExecute = $false
$s.RedirectStandardInput = $true
$s.RedirectStandardOutput = $true
$s.RedirectStandardError = $true
$s.ArgumentList.Add('--english-model-setup')
$p = [Diagnostics.Process]::Start($s)
$discard = $p.StandardError.BaseStream.CopyToAsync([IO.Stream]::Null)
$time = [Diagnostics.Stopwatch]::StartNew()
$terminal = $false
try {
    $p.StandardInput.WriteLine('{"v":1,"type":"model_setup_start","session":1,"action":"install"}')
    $p.StandardInput.Flush()
    while ($time.Elapsed.TotalMinutes -lt 13) {
        $pending = $p.StandardOutput.ReadLineAsync()
        if (!$pending.Wait([TimeSpan]::FromMinutes(13) - $time.Elapsed)) { throw 'English model setup timed out.' }
        if (!$pending.Result) { break }
        $event = $pending.Result | ConvertFrom-Json
        if ($event.v -ne 1 -or $event.type -ne 'model_setup' -or $event.session -ne 1) {
            throw 'Malformed English setup response.'
        }
        switch ($event.state) {
            'downloading' {
                if ($event.total -ne 310414022 -or $event.received -lt 0 -or $event.received -gt $event.total) {
                    throw 'Invalid setup byte count.'
                }
                Write-Progress -Activity 'Installing offline English model' -PercentComplete (100 * $event.received / $event.total)
            }
            { $_ -in 'checking_archive','extracting','checking_files','committing' } {
                Write-Progress -Activity 'Installing offline English model' -Status $_
            }
            'ready' { $terminal = $true; break }
            'cancelled' { throw 'English model setup was cancelled.' }
            'failed' {
                $allowed = 'busy','not_installed','needs_repair','access_denied','insufficient_space','network_unavailable',
                    'timeout','digest_mismatch','archive_invalid','invalid_path','cleanup_incomplete','installer_internal'
                $reason = if ($event.code -cin $allowed) { $event.code } else { 'installer_internal' }
                throw "English model setup failed: $reason."
            }
            default { throw 'Unknown English setup stage.' }
        }
        if ($terminal) { break }
    }
    if (!$terminal -or !$p.WaitForExit(3000) -or $p.ExitCode -ne 0) { throw 'English model setup did not complete.' }
    Write-Host 'English model verified and ready. Capture remains stopped until you press Start.'
} finally {
    Write-Progress -Activity 'Installing offline English model' -Completed
    if (!$p.HasExited) {
        try { $p.StandardInput.WriteLine('{"v":1,"type":"model_setup_cancel","session":1}'); $p.StandardInput.Close() } catch {}
        if (!$p.WaitForExit(2000)) { $p.Kill($true); $p.WaitForExit() }
    }
    $p.Dispose()
}
