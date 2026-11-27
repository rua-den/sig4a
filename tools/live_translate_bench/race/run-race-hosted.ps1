# Run only on an isolated Windows proof runner, elevated for application-scoped firewall blocks.
param([Parameter(Mandatory)][string]$Evidence)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$root = Join-Path $env:RUNNER_TEMP ('sig4a-asr-race-' + [guid]::NewGuid())
New-Item -ItemType Directory -Force $root,$Evidence | Out-Null
$Evidence = (Resolve-Path $Evidence).Path
$python = (Get-Command python.exe).Source
$rev = git rev-parse HEAD
& $python -c "import tkinter,sys; assert sys.version_info[:2] == (3,11)"

function Fetch([string]$uri,[string]$name,[string]$digest,[string]$algorithm='SHA256') {
  $target = Join-Path $root $name
  curl.exe -fsSL --retry 3 --connect-timeout 25 --max-time 240 --output $target $uri
  $hash = (Get-FileHash -LiteralPath $target -Algorithm $algorithm).Hash
  if ($hash -ne $digest) { throw "Checksum mismatch for $name" }
  [ordered]@{name=$name; sha256=(Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash; bytes=(Get-Item $target).Length} |
    ConvertTo-Json -Compress | Add-Content -LiteralPath (Join-Path $Evidence 'downloads.jsonl')
  return $target
}

$whisperZip = Fetch 'https://github.com/ggml-org/whisper.cpp/releases/download/v1.8.3/whisper-bin-x64.zip' 'whisper.zip' 'd824b1e37599f882b396e73f1ee0bfd5d0529f700314c48311dcbd00b803321d'
$tiny = Fetch 'https://huggingface.co/ggerganov/whisper.cpp/resolve/fb5ed9369c26eaf08392504d8446da431e0fffb9/ggml-tiny.en.bin' 'tiny.en.bin' '921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f'
$zipformerName = 'sherpa-onnx-streaming-zipformer-en-2023-06-26'
$zipformer = Fetch "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/$zipformerName.tar.bz2" 'sherpa.tar.bz2' '639e25b578e9e997131402199419c13a941f8e4e198e2da1ce57dbf5cf401282'
Expand-Archive $whisperZip (Join-Path $root 'whisper')
tar.exe -xf $zipformer -C $root
$whisper = (Get-ChildItem (Join-Path $root 'whisper') -Filter whisper-server.exe -Recurse | Select-Object -First 1).FullName
if (-not $whisper) { throw 'Pinned whisper-server.exe is missing' }
if (-not [Environment]::UserInteractive) { throw 'Interactive desktop required for the Tk caption probe.' }

# Read-only token accesses one pinned artifact from the same repository.
# curl's normal redirect logic drops Authorization before the blob-host redirect.
$fixtureZip = Join-Path $root 'frozen-source.zip'
curl.exe -fsSL --retry 2 --connect-timeout 20 --max-time 120 --header "Authorization: Bearer $env:BENCH_ACTIONS_TOKEN" --output $fixtureZip "$env:GITHUB_API_URL/repos/$env:GITHUB_REPOSITORY/actions/artifacts/15189618561/zip"
Remove-Item Env:BENCH_ACTIONS_TOKEN
if ((Get-FileHash -LiteralPath $fixtureZip -Algorithm SHA256).Hash -ne '47394d76cf554d4c18e2b9688187bd568cc3ac8631369b119eb4eee1634b8e89') {
  throw 'Exact #27 artifact checksum mismatch'
}
$fixture = Join-Path $root 'source'
Expand-Archive $fixtureZip $fixture
$frozen = Join-Path $fixture 'frozen.json'
$wav = Join-Path $fixture 'wav'
Copy-Item -LiteralPath $frozen -Destination (Join-Path $Evidence 'frozen.json')
function Start-ProofServer($exe,$argv,$name,$port) {
  if (Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue) { throw "$port already bound" }
  $timer = [Diagnostics.Stopwatch]::StartNew()
  $p = Start-Process -FilePath $exe -ArgumentList $argv -PassThru -RedirectStandardError (Join-Path $Evidence "$name.stderr.txt") -RedirectStandardOutput (Join-Path $Evidence "$name.stdout.txt")
  $servers.Add($p)
  while ($timer.Elapsed.TotalSeconds -lt 75) {
    $p.Refresh()
    if ($p.HasExited) { throw "$name exited before readiness ($($p.ExitCode))" }
    $listen = Get-NetTCPConnection -LocalAddress 127.0.0.1 -LocalPort $port -State Listen -ErrorAction SilentlyContinue | Where-Object OwningProcess -eq $p.Id
    if ($listen) {
      try {
        $response = Invoke-WebRequest -Uri "http://127.0.0.1:$port/health" -TimeoutSec 2 -NoProxy
        if ($response.StatusCode -eq 200) {
          "$name readiness_ms=$($timer.ElapsedMilliseconds)" | Add-Content -LiteralPath (Join-Path $Evidence 'readiness.txt')
          return $p
        }
      } catch {}
    }
    Start-Sleep -Milliseconds 500
  }
  throw "$name never became ready at its owned local port"
}
# TCP probe with no audio/text; stage comparison before firewall is changed.
$probeIp = [System.Net.Dns]::GetHostAddresses('api.github.com') | Where-Object AddressFamily -eq InterNetwork | Select-Object -First 1 -ExpandProperty IPAddressToString
$socketCheck = 'import socket,sys; s=socket.socket(); s.settimeout(2); connected=(s.connect_ex((sys.argv[1],443))==0); s.close(); assert connected==(sys.argv[2]=="allowed"), "Unexpected synthetic outbound result"; print("EGRESS_PROBE",sys.argv[2])'
& $python -c $socketCheck $probeIp allowed
$servers = [System.Collections.Generic.List[Diagnostics.Process]]::new()
$rules = [System.Collections.Generic.List[string]]::new()
try {
  foreach ($exe in @($python,$whisper)) {
    $name = 'sig4a-benchmark-' + [guid]::NewGuid()
    $profiles = @(Get-NetConnectionProfile | ForEach-Object { if ($_.NetworkCategory -eq 'DomainAuthenticated') { 'Domain' } else { [string]$_.NetworkCategory } } | Sort-Object -Unique)
    foreach ($profile in $profiles) {
      if ((Get-NetFirewallProfile -PolicyStore ActiveStore -Name $profile).Enabled -ne 'True') { throw "Windows Firewall disabled for active profile $profile." }
    }
    if (!$profiles.Count) { throw 'No active network profile found.' }
    New-NetFirewallRule -Name $name -DisplayName $name -Direction Outbound -Action Block -Profile Any -Program $exe -RemoteAddress Any | Out-Null
    $rules.Add($name)
    $active = Get-NetFirewallRule -PolicyStore ActiveStore -Name $name
    if ($active.Enabled -ne 'True' -or $active.Action -ne 'Block' -or $active.Direction -ne 'Outbound') {
      throw 'Expected outbound block not present in active policy.'
    }
  }
  & $python -c $socketCheck $probeIp denied

  $started = Start-ProofServer $whisper @('--host','127.0.0.1','--port','18080','--language','en','--no-gpu','-t','4','-ac','512','-m',"`"$tiny`"") 'whisper-tiny' 18080
  $common = @('--audio',$wav,'--frozen',$frozen,'--revision',$rev)
  $tinyRows = Join-Path $Evidence 'whisper-tiny-asr.jsonl'
  & $python "$PSScriptRoot\measure_race.py" @common --whisper-model $tiny --whisper-port 18080 --resource-pid $started.Id --output $tinyRows
  $started.Kill()
  $started.WaitForExit()
  $sherpaRows = Join-Path $Evidence 'sherpa-large-asr.jsonl'
  & $python "$PSScriptRoot\measure_race.py" @common --sherpa-model-dir (Join-Path $root $zipformerName) --output $sherpaRows
  foreach ($rows in @($tinyRows,$sherpaRows)) {
    & $python "$PSScriptRoot\report_asr.py" --results $rows --frozen $frozen | Set-Content -LiteralPath ($rows + '.diagnostic.json')
  }
} finally {
  foreach ($p in $servers) {
    $p.Refresh()
    if (-not $p.HasExited) { $p.Kill(); $p.WaitForExit() }
    $p.Dispose()
  }
  foreach ($name in $rules) { Remove-NetFirewallRule -Name $name -ErrorAction Continue }
}
