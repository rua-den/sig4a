# Run only on an isolated Windows proof runner, elevated for application-scoped firewall blocks.
param([Parameter(Mandatory)][string]$Evidence)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$root = Join-Path $env:RUNNER_TEMP ('sig4a-bench-' + [guid]::NewGuid())
New-Item -ItemType Directory -Force $root,$Evidence | Out-Null
$Evidence = (Resolve-Path $Evidence).Path
$python = (Get-Command python.exe).Source
$rev = git rev-parse HEAD
& $python -c "import tkinter,sys; assert sys.version_info[:2] == (3,11)"
if (-not [Environment]::UserInteractive) { throw 'Interactive desktop required for the Tk caption harness.' }

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
$wolf = Fetch 'https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin' 'ggml-base.en.bin' '137c40403d78fd54d454da0f9bd998f78703390c' 'SHA1'
$llamaZip = Fetch 'https://github.com/ggml-org/llama.cpp/releases/download/b12776/llama-b12776-bin-win-cpu-x64.zip' 'llama.zip' 'bba23437a597b7b346acb87dbac2e5452c7b188cec67ca6315bec513a067ace8'
$lfm = Fetch 'https://huggingface.co/LiquidAI/LFM2.5-VL-450M-GGUF/resolve/166cd80bbe157dc86d65f964eb8cc6a2cede62ca/LFM2.5-VL-450M-Q8_0.gguf' 'lfm-q8.gguf' '263aca93039e22140d55e046831c700c796affa8143d7638581c488a30c712bc'
$sherpaArchive = Fetch 'https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17.tar.bz2' 'sherpa.tar.bz2' '9c559283e8498d3fe95913c79ca1cb454bb26281ac2b102b41306c7d752765d9'
Expand-Archive $whisperZip (Join-Path $root 'whisper')
Expand-Archive $llamaZip (Join-Path $root 'llama')
tar.exe -xf $sherpaArchive -C $root
$sherpaDir = Join-Path $root 'sherpa-onnx-streaming-zipformer-en-20M-2023-02-17'
$whisper = (Get-ChildItem (Join-Path $root 'whisper') -Filter whisper-server.exe -Recurse | Select-Object -First 1).FullName
$llama = (Get-ChildItem (Join-Path $root 'llama') -Filter llama-server.exe -Recurse | Select-Object -First 1).FullName
if (-not $whisper -or -not $llama) { throw 'Pinned server executable missing in expected archives' }
$wav = Join-Path $root 'wav'
$frozen = Join-Path $Evidence 'frozen.json'
powershell.exe -NoProfile -File "$PSScriptRoot\synthesize.ps1" -Directory $wav
& $python "$PSScriptRoot\measure.py" freeze --audio $wav --frozen $frozen

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
  foreach ($exe in @($python,$whisper,$llama)) {
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
  $w = Start-ProofServer $whisper @('--host','127.0.0.1','--port','18080','--language','en','--no-gpu','-t','4','-m',"`"$wolf`"") 'whisper' 18080
  $l = Start-ProofServer $llama @('--host','127.0.0.1','--port','18081','--alias','sig4a-lfm-q8','-ngl','0','-t','4','-c','2048','-m',"`"$lfm`"") 'llama' 18081
  $common = @('--audio',$wav,'--frozen',$frozen,'--translator-blob',$lfm,'--translator-backend','llama.cpp','--translator-version','b12776','--translator-port','18081','--revision',$rev)
  $whisperRows = Join-Path $Evidence 'whisper.jsonl'
  & $python "$PSScriptRoot\measure.py" run @common --whisper-model $wolf --whisper-version v1.8.3 --whisper-port 18080 --resource-pid $w.Id --resource-pid $l.Id --output $whisperRows
  $w.Kill()
  $w.WaitForExit()
  $sherpaRows = Join-Path $Evidence 'sherpa.jsonl'
  & $python "$PSScriptRoot\measure.py" run @common --sherpa-model-dir $sherpaDir --resource-pid $l.Id --output $sherpaRows
  foreach ($rows in @($whisperRows,$sherpaRows)) {
    & $python "$PSScriptRoot\run_benchmark.py" --results $rows --frozen $frozen | Set-Content -LiteralPath ($rows + '.ungraded.json')
  }
} finally {
  foreach ($p in $servers) {
    $p.Refresh()
    if (-not $p.HasExited) { $p.Kill(); $p.WaitForExit() }
    $p.Dispose()
  }
  foreach ($name in $rules) { Remove-NetFirewallRule -Name $name -ErrorAction Continue }
}
