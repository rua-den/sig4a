# Optional, explicit Windows x64 Vietnamese engine setup. It does not start
# inference or capture. Do not run while SIG4A is using this helper.
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
if (!(Test-Path -LiteralPath (Join-Path $PSScriptRoot 'Sig4a.LiveTranslate.exe') -PathType Leaf)) {
    throw 'Run this from the live-translate directory of the extracted SIG4A package.'
}
$manifest = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'translation-assets.json') | ConvertFrom-Json -AsHashtable
$base = Join-Path $PSScriptRoot 'models'
if (Test-Path -LiteralPath $base) {
    $item = Get-Item -Force -LiteralPath $base
    if (!$item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw 'Models folder must be a normal local directory.'
    }
} else { [void][IO.Directory]::CreateDirectory($base) }
$target = Join-Path $base 'hymt'
if (Test-Path -LiteralPath $target) { throw 'hymt already exists. Stop Live Translate and move the old folder aside before retrying.' }
$temp = Join-Path $base ('install-' + [Guid]::NewGuid().ToString('N'))
[void][IO.Directory]::CreateDirectory($temp)
$ready = Join-Path $temp 'hymt'
$runtime = Join-Path $ready 'runtime'
[void][IO.Directory]::CreateDirectory($runtime)
function Fetch([string]$uri,[string]$file,[string]$digest) {
    curl.exe --proto '=https' --proto-redir '=https' -fsSL --retry 2 --connect-timeout 20 --max-time 1000 --output $file $uri
    if ($LASTEXITCODE -ne 0 -or (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ine $digest) {
        throw 'Official asset download or checksum verification failed.'
    }
}
try {
    Write-Host 'Installing the optional local Vietnamese translator (~1.13 GB model plus runtime). No recording or inference starts.'
    $archive = Join-Path $temp 'llama.zip'
    Fetch 'https://github.com/ggml-org/llama.cpp/releases/download/b12776/llama-b12776-bin-win-cpu-x64.zip' $archive 'bba23437a597b7b346acb87dbac2e5452c7b188cec67ca6315bec513a067ace8'
    $zip = [IO.Compression.ZipFile]::OpenRead($archive)
    try {
        foreach ($name in $manifest.files.Keys) {
            if ($name -cnotmatch '^(?:[A-Za-z0-9_.-]+\.(?:exe|dll)|LICENSE-LLVM-OpenMP)$') { throw 'Invalid runtime path.' }
            $member = $zip.GetEntry($name)
            if (!$member) { throw "Required runtime file absent: $name" }
            $output = Join-Path $runtime $name
            [IO.Compression.ZipFileExtensions]::ExtractToFile($member, $output)
            if ((Get-FileHash -LiteralPath $output -Algorithm SHA256).Hash -ine $manifest.files[$name]) {
                throw "Runtime file checksum mismatch: $name"
            }
        }
    } finally { $zip.Dispose() }
    $modelName = 'HY-MT1.5-1.8B-Q4_K_M.gguf'
    Fetch "https://huggingface.co/tencent/HY-MT1.5-1.8B-GGUF/resolve/265b2e615a7dc9b06c435dc878829ad99a512ba2/$modelName" (Join-Path $ready $modelName) '4383ac0c3c8e476de98ff979c2a3f069f8c4fb385e7860cf2d28da896cc477c7'
    [IO.Directory]::Move($ready, $target)
    Write-Host 'Optional translation assets installed locally. No capture has started.'
} finally { if (Test-Path -LiteralPath $temp) { Remove-Item -LiteralPath $temp -Recurse -Force } }
