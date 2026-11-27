# Requires PowerShell 7, .NET 8 and AudioProbe.dll built as described in README.
[CmdletBinding()]
param(
    [ValidateSet('list','mic','playback')][string]$Source='playback',
    [string]$Device='default',
    [ValidateSet('','mic','playback')][string]$Then='',
    [string]$ThenDevice='default',
    [ValidateRange(1,15)][int]$Seconds=3,
    [string]$Assembly=(Join-Path $PSScriptRoot 'bin/Release/net8.0-windows/AudioProbe.dll')
)
$ErrorActionPreference='Stop'
if (-not (Test-Path -LiteralPath $Assembly)) { throw 'Build AudioProbe.csproj first.' }

$info=[Diagnostics.ProcessStartInfo]::new('dotnet')
$info.ArgumentList.Add((Resolve-Path -LiteralPath $Assembly).ProviderPath)
if ($Source -eq 'list') { $info.ArgumentList.Add('--list') }
else {
    foreach ($part in @([string]$Seconds,$Source,$Device)) { $info.ArgumentList.Add($part) }
    if ($Then) { $info.ArgumentList.Add($Then); $info.ArgumentList.Add($ThenDevice) }
}
$info.UseShellExecute=$false
$process=[Diagnostics.Process]::Start($info)
try {
    if (-not $process.WaitForExit((($Seconds+10)*2+8)*1000)) {
        $process.Kill($true)
        [void]$process.WaitForExit(5000)
        [Console]::Out.WriteLine('{"ok":false,"error":"hard_timeout","persisted_audio":false}')
        exit 124
    }
    exit $process.ExitCode
} finally {
    if (!$process.HasExited) { $process.Kill($true); [void]$process.WaitForExit(5000) }
    $process.Dispose()
}
