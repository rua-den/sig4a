# Run only on an isolated Windows proof runner, elevated for application-scoped firewall blocks.
param([Parameter(Mandatory)][string]$Evidence)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$root = Join-Path $env:RUNNER_TEMP ('sig4a-mt-followup-' + [guid]::NewGuid())
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
$llamaZip = Fetch 'https://github.com/ggml-org/llama.cpp/releases/download/b12776/llama-b12776-bin-win-cpu-x64.zip' 'llama.zip' 'bba23437a597b7b346acb87dbac2e5452c7b188cec67ca6315bec513a067ace8'
$lfm = Fetch 'https://huggingface.co/LiquidAI/LFM2.5-VL-450M-GGUF/resolve/166cd80bbe157dc86d65f964eb8cc6a2cede62ca/LFM2.5-VL-450M-Q8_0.gguf' 'lfm-q8.gguf' '263aca93039e22140d55e046831c700c796affa8143d7638581c488a30c712bc'

$opus = Join-Path $root 'opus'
New-Item -ItemType Directory -Force $opus | Out-Null
$uri = 'https://huggingface.co/Helsinki-NLP/opus-mt-en-vi/resolve/3ab3b2d9aa096a3b0c20668d87566bb92613aba1'
$opusWeight = Fetch "$uri/model.safetensors" 'marian.safetensors' '652c300995d3d074c558ed799dbc9afba223441102cafee1b20e1c52a825d3a6'
Move-Item -LiteralPath $opusWeight -Destination (Join-Path $opus 'model.safetensors')
# Read immutable commit metadata for non-weight config/tokenizer content; record each sha.
foreach ($name in @('config.json','tokenizer_config.json','source.spm','target.spm','vocab.json')) {
  $p = Join-Path $opus $name
  curl.exe -fsSL --retry 3 --connect-timeout 25 --max-time 60 --output $p "$uri/$name"
  [ordered]@{name=$name; sha256=(Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash; bytes=(Get-Item $p).Length} |
    ConvertTo-Json -Compress | Add-Content -LiteralPath (Join-Path $Evidence 'downloads.jsonl')
}
Expand-Archive $llamaZip (Join-Path $root 'llama')
$llama = (Get-ChildItem (Join-Path $root 'llama') -Filter llama-server.exe -Recurse | Select-Object -First 1).FullName
if (-not $llama) { throw 'Pinned llama-server.exe is missing' }
& $python "$PSScriptRoot\compare_mt.py" --check-input
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
  $l = Start-ProofServer $llama @('--host','127.0.0.1','--port','18081','--alias','sig4a-lfm-q8','-ngl','0','-t','4','-c','2048','-m',"`"$lfm`"") 'llama' 18081

  & $python "$PSScriptRoot\compare_mt.py" --model-dir $opus --lfm-weights $lfm --output (Join-Path $Evidence 'mt-comparison.jsonl')
} finally {
  foreach ($p in $servers) {
    $p.Refresh()
    if (-not $p.HasExited) { $p.Kill(); $p.WaitForExit() }
    $p.Dispose()
  }
  foreach ($name in $rules) { Remove-NetFirewallRule -Name $name -ErrorAction Continue }
}
