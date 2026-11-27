# Opt-in hosted proof setup, never called by the app. Uses an isolated user/model directory.
function Start-SignedOllamaProof {
    param(
        [Parameter(Mandatory)][string]$Scratch,
        [Parameter(Mandatory)][ValidatePattern('^b37049369adf[0-9a-f]{52}$')][string]$ManifestSha256
    )
    $ErrorActionPreference = 'Stop'
    $version = 'v0.31.1'
    $zipHash = '9ecf5a631561c7dff3a143925f11e2008327be738a7279fcf0c5462b9c422700'
    $zip = Join-Path $Scratch 'ollama-windows-amd64.zip'
    $manifest = Join-Path $Scratch 'ollama-gemma4-e2b-q4-manifest.json'
    $target = Join-Path $env:LOCALAPPDATA 'Programs/Ollama'
    if (Test-Path -LiteralPath $target) { throw 'Expected unused isolated Ollama directory.' }
    if (Get-NetTCPConnection -State Listen -LocalPort 11434 -ErrorAction SilentlyContinue) {
        throw 'Proof will not change any existing Ollama listener.'
    }
    New-Item -ItemType Directory -Force $Scratch,$target | Out-Null
    Invoke-WebRequest -Uri 'https://registry.ollama.ai/v2/library/gemma4/manifests/e2b-it-q4_K_M' -OutFile $manifest -MaximumRedirection 0 -TimeoutSec 20
    if ((Get-FileHash -LiteralPath $manifest -Algorithm SHA256).Hash -ine $ManifestSha256) {
        throw 'Official model manifest SHA256 no longer matches reviewed exact digest; do not download the model.'
    }

    Invoke-WebRequest -Uri "https://github.com/ollama/ollama/releases/download/$version/ollama-windows-amd64.zip" -OutFile $zip -MaximumRedirection 2 -TimeoutSec 720
    if ((Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash -ine $zipHash) { throw 'Official archive SHA256 mismatch.' }
    Expand-Archive -LiteralPath $zip -DestinationPath $target
    $exe = Join-Path $target 'ollama.exe'
    $signature = Get-AuthenticodeSignature -LiteralPath $exe
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch '(?:^|,\s*)O=Ollama Inc[.](?:,|$)') {
        throw 'The portable archive did not provide a verifier-approved signed executable on this runner. Real proof blocked.'
    }
    $env:OLLAMA_HOST = '127.0.0.1:11434'
    $env:OLLAMA_NO_CLOUD = '1'
    $env:OLLAMA_MODELS = Join-Path $Scratch 'models'
    $env:OLLAMA_CONTEXT_LENGTH = '2048'
    $env:OLLAMA_NUM_PARALLEL = '1'
    $env:OLLAMA_KEEP_ALIVE = '5m'
    $daemon = $pull = $copy = $null
    $handedOff = $false
    try {
        $opts = @{FilePath=$exe;WorkingDirectory=$target;PassThru=$true;NoNewWindow=$true}
        $daemon = Start-Process @opts -ArgumentList 'serve' -RedirectStandardOutput (Join-Path $Scratch 'server.out') -RedirectStandardError (Join-Path $Scratch 'server.err')
        $clock = [Diagnostics.Stopwatch]::StartNew()
        $state = $null
        do {
            if ($daemon.HasExited) { throw 'Official signed Ollama exited during setup.' }
            try { $state = Invoke-RestMethod -Uri 'http://127.0.0.1:11434/api/status' -NoProxy -MaximumRedirection 0 -TimeoutSec 1 } catch {}
            if (!$state) { Start-Sleep -Milliseconds 250 }
        } until ($state -or $clock.Elapsed.TotalSeconds -gt 25)
        $owners = @(Get-NetTCPConnection -State Listen -LocalPort 11434 -ErrorAction Stop)
        if (!$state -or $state.cloud.disabled -ne $true -or $state.cloud.source -cnotin @('env','both') -or
            $owners.Count -ne 1 -or $owners[0].OwningProcess -ne $daemon.Id -or $owners[0].LocalAddress -ne '127.0.0.1') {
            throw 'Proof daemon failed exact owner and env-forced Cloud-off checks.'
        }
        # This is operator-approved CI provisioning before opening Sig4a; no user/model text is sent.
        $name = 'gemma4:e2b-it-q4_K_M'
        $pull = Start-Process @opts -ArgumentList @('pull',$name) -RedirectStandardOutput (Join-Path $Scratch 'pull.out') -RedirectStandardError (Join-Path $Scratch 'pull.err')
        if (!$pull.WaitForExit(1200000) -or $pull.ExitCode -ne 0) { throw 'Did not provision the model in the proof model store.' }
        $tags = Invoke-RestMethod -Uri 'http://127.0.0.1:11434/api/tags' -NoProxy -MaximumRedirection 0 -TimeoutSec 5
        $model = @($tags.models | Where-Object { $_.name -ceq $name })
        if ($model.Count -ne 1 -or $model[0].digest -ine $ManifestSha256) {
            throw ('Review installed manifest against the intended Q4 fixture digest. Observed: ' + ($model.digest -join ','))
        }
        $copy = Start-Process @opts -ArgumentList @('cp',$name,'gemma4:e2b') -RedirectStandardOutput (Join-Path $Scratch 'copy.out') -RedirectStandardError (Join-Path $Scratch 'copy.err')
        if (!$copy.WaitForExit(6000) -or $copy.ExitCode -ne 0) { throw 'Could not create the exact local fixture tag.' }
        $tags = Invoke-RestMethod -Uri 'http://127.0.0.1:11434/api/tags' -NoProxy -MaximumRedirection 0 -TimeoutSec 5
        $alias = @($tags.models | Where-Object { $_.name -ceq 'gemma4:e2b' })
        if ($alias.Count -ne 1 -or $alias[0].digest -ine $ManifestSha256) { throw 'Local alias did not retain reviewed model digest.' }
        Write-Host "PROOF_SETUP_ONLY: Ollama $version ZIP SHA256=$zipHash, signed image SHA256=$((Get-FileHash -LiteralPath $exe).Hash), model=$ManifestSha256; cloud.disabled=true source=$($state.cloud.source)."
        $handedOff = $true
        return $daemon
    } finally {
        foreach ($p in @($pull,$copy)) {
            if ($p) { if (!$p.HasExited) { $p.Kill($true); [void]$p.WaitForExit(5000) }; $p.Dispose() }
        }
        if (!$handedOff -and $daemon) {
            if (!$daemon.HasExited) { $daemon.Kill($true); [void]$daemon.WaitForExit(5000) }
            $daemon.Dispose()
        }
    }
}
