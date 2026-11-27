# Explicit, opt-in model setup for a colocated Live Translate helper. No logs/audio or auto-start.
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
if (!(Test-Path -LiteralPath (Join-Path $PSScriptRoot 'Sig4a.LiveTranslate.exe') -PathType Leaf)) {
    throw 'Run this from the live-translate directory of the extracted SIG4A package.'
}
$manifest = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'model-assets.json') | ConvertFrom-Json -AsHashtable
$name = 'sherpa-onnx-streaming-zipformer-en-2023-06-26'
$archiveDigest = '639e25b578e9e997131402199419c13a941f8e4e198e2da1ce57dbf5cf401282'
$base = Join-Path $PSScriptRoot 'models'
if (Test-Path -LiteralPath $base) {
    $item = Get-Item -Force -LiteralPath $base
    if (!$item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw 'Models folder must be a normal local directory.'
    }
} else { [void][IO.Directory]::CreateDirectory($base) }
$target = Join-Path $base 'zipformer'
if (Test-Path -LiteralPath $target) { throw 'zipformer already exists. Stop Live Translate and move the old folder aside before retrying.' }
$temp = Join-Path $base ('install-' + [Guid]::NewGuid().ToString('N'))
[void][IO.Directory]::CreateDirectory($temp)
$archive = Join-Path $temp 'zipformer.tar.bz2'
$members = @()
try {
    curl.exe --proto '=https' --proto-redir '=https' -fsSL --retry 2 --connect-timeout 15 --max-time 360 --output $archive "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/$name.tar.bz2"
    if ($LASTEXITCODE -ne 0 -or (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ine $archiveDigest) {
        throw 'Model download failed or did not match its publisher archive checksum.'
    }
    foreach ($member in $manifest.files.Keys) {
        if ($member -cnotmatch '^([A-Za-z0-9_-]+\.)+(txt|onnx|md)$') { throw 'Unexpected model path.' }
        $members += "$name/$member"
    }
    tar.exe -xf $archive -C $temp @members
    if ($LASTEXITCODE -ne 0) { throw 'Unable to extract the pinned English model.' }
    $ready = Join-Path $temp $name
    foreach ($member in $manifest.files.Keys) {
        $file = Join-Path $ready $member
        if ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ine $manifest.files[$member]) {
            throw "Checksum mismatch for selected model member $member."
        }
    }
    [IO.Directory]::Move($ready, $target)
    Write-Host 'English model has been installed locally. Live Translate remains stopped until you press Start.'
} finally { if (Test-Path -LiteralPath $temp) { Remove-Item -LiteralPath $temp -Recurse -Force } }
