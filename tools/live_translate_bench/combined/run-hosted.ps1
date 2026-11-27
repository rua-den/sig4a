# Run only on an isolated Windows proof runner, elevated for application-scoped firewall blocks.
param([Parameter(Mandatory)][string]$Evidence)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$root = Join-Path $env:RUNNER_TEMP ('sig4a-combined-' + [guid]::NewGuid())
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

$zipformerName = 'sherpa-onnx-streaming-zipformer-en-2023-06-26'
$zipformer = Fetch "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/$zipformerName.tar.bz2" 'sherpa.tar.bz2' '639e25b578e9e997131402199419c13a941f8e4e198e2da1ce57dbf5cf401282'
tar.exe -xf $zipformer -C $root
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
$llamaZip = Fetch 'https://github.com/ggml-org/llama.cpp/releases/download/b12776/llama-b12776-bin-win-cpu-x64.zip' 'llama.zip' 'bba23437a597b7b346acb87dbac2e5452c7b188cec67ca6315bec513a067ace8'
$hymt = Fetch 'https://huggingface.co/tencent/HY-MT1.5-1.8B-GGUF/resolve/265b2e615a7dc9b06c435dc878829ad99a512ba2/HY-MT1.5-1.8B-Q4_K_M.gguf' 'hy-mt-q4-k-m.gguf' '4383ac0c3c8e476de98ff979c2a3f069f8c4fb385e7860cf2d28da896cc477c7'
Expand-Archive $llamaZip (Join-Path $root 'llama')
$llama = (Get-ChildItem (Join-Path $root 'llama') -Filter llama-server.exe -Recurse | Select-Object -First 1).FullName
if (-not $llama) { throw 'Pinned llama-server.exe is missing' }

& $python "$PSScriptRoot\continuous.py" prepare --audio $wav --frozen $frozen --output $Evidence
$boundaryHash = (Get-FileHash -LiteralPath (Join-Path $Evidence 'scoring-only-boundaries.json')).Hash
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
  foreach ($exe in @($python,$llama)) {
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

  $server = Start-ProofServer $llama @('--host','127.0.0.1','--port','18083','--alias','hymt-q4-k-m','-ngl','0','-t','4','-c','2048','-np','1','-m',"`"$hymt`"") 'hymt-q4-k-m' 18083
  & $python "$PSScriptRoot\continuous.py" play --output $Evidence --model-dir (Join-Path $root $zipformerName) --hymt-weights $hymt --server-pid $server.Id --revision $rev
  if ((Get-FileHash -LiteralPath (Join-Path $Evidence 'scoring-only-boundaries.json')).Hash -ne $boundaryHash) { throw 'Scoring labels changed during the replay.' }
  & $python "$PSScriptRoot\continuous.py" score --output $Evidence
} finally {
  foreach ($p in $servers) {
    $p.Refresh()
    if (-not $p.HasExited) { $p.Kill(); $p.WaitForExit() }
    $p.Dispose()
  }
  foreach ($name in $rules) { Remove-NetFirewallRule -Name $name -ErrorAction Continue }
}
