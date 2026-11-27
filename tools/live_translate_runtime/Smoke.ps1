# Smoke only the extracted Windows helper. Never starts capture: model files must be absent.
param([Parameter(Mandatory)][string]$Published)
$ErrorActionPreference = 'Stop'
$path = [IO.Path]::GetFullPath($Published)
if (Test-Path -LiteralPath (Join-Path $path 'models')) { throw 'Smoke needs an isolated package without models.' }
$exe = Join-Path $path 'Sig4a.LiveTranslate.exe'
function Probe([string]$wire,[int]$expected) {
    $start = [Diagnostics.ProcessStartInfo]::new($exe)
    $start.UseShellExecute = $false
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $p = [Diagnostics.Process]::Start($start)
    try {
        $p.StandardInput.Write($wire)
        $p.StandardInput.Flush()
        $out = $p.StandardOutput.ReadToEndAsync()
        $err = $p.StandardError.ReadToEndAsync()
        $p.StandardInput.Close()
        if (!$p.WaitForExit(6000) -or $p.ExitCode -ne $expected) { throw 'Helper smoke failed to exit as expected.' }
        if ($err.Result) { throw 'Unexpected helper stderr.' }
        return @($out.Result -split '?
' | Where-Object { $_ } | ForEach-Object { $_ | ConvertFrom-Json })
    } finally { if (!$p.HasExited) { $p.Kill($true) }; $p.Dispose() }
}
$listOutput = @(Probe "{`"v`":1,`"type`":`"list`"}`n" 0)
if ($listOutput.Count -ne 1 -or $listOutput[0].type -notin @('devices','error')) { throw 'Expected exactly one capped inventory response.' }
"Spoken capture never opened; inventory result type: $($listOutput[0].type)"
if (@(Probe ('x' * 16385) 2)[0].code -ne 'protocol_error') { throw 'Oversized frame refusal missing.' }
if (@(Probe "{`"v`":1,`"v`":1,`"type`":`"list`"}`n" 2)[0].code -ne 'protocol_error') { throw 'Duplicate field refusal missing.' }
# Keep stdin open until model_missing and stopped are obtained so EOF cannot mask the result.
$start = [Diagnostics.ProcessStartInfo]::new($exe)
$start.UseShellExecute = $false
$start.RedirectStandardInput = $true
$start.RedirectStandardOutput = $true
$p = [Diagnostics.Process]::Start($start)
try {
    $p.StandardInput.WriteLine('{"v":1,"type":"start","session":1,"source":"playback","device_id":null,"translate":false}')
    $p.StandardInput.Flush()
    foreach ($kind in @('error','status')) {
        $line = $p.StandardOutput.ReadLineAsync()
        if (!$line.Wait(6000) -or !$line.Result) { throw 'Expected missing model refusal and idle completion.' }
        $result = $line.Result | ConvertFrom-Json
        if ($result.session -ne 1 -or $result.type -ne $kind -or
            ($kind -eq 'error' -and $result.code -ne 'model_missing') -or
            ($kind -eq 'status' -and $result.state -ne 'stopped')) { throw 'Unexpected helper result.' }
    }
    $p.StandardInput.Close()
    if (!$p.WaitForExit(6000) -or $p.ExitCode -ne 0) { throw 'Helper did not exit gracefully on EOF.' }
} finally { if (!$p.HasExited) { $p.Kill($true) }; $p.Dispose() }
'Windows companion smoke passed without capture or models.'
