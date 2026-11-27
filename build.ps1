[CmdletBinding()]
param([switch]$SkipLiveTranslate)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '.')).Path
$rustRoot = Join-Path $repoRoot 'rust'

Push-Location $rustRoot
try {
    $metadata = (& cargo metadata --locked --no-deps --format-version 1 | ConvertFrom-Json)
    if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed with exit code $LASTEXITCODE" }
    $dashboardPackage = $metadata.packages | Where-Object name -eq 'claude-usage-dashboard'
    if (-not $dashboardPackage) { throw 'Could not find claude-usage-dashboard in cargo metadata' }
    $version = $dashboardPackage.version
    $releaseDir = Join-Path $metadata.target_directory 'release'
    & cargo build --locked --release --workspace
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }
} finally {
    Pop-Location
}

$outputDir = Join-Path $repoRoot 'dist'
$archive = Join-Path $outputDir "sig4a-$version-windows.zip"
New-Item -ItemType Directory -Force -Path $outputDir | Out-Null
$files = @(
    (Join-Path $releaseDir 'claude-usage.exe'),
    (Join-Path $releaseDir 'claude-usage-dashboard.exe'),
    (Join-Path $repoRoot 'README.md')
)
foreach ($source in $files[0..1]) {
    if (-not (Test-Path -LiteralPath $source)) { throw "Build did not produce $source" }
}
# Package the Windows companion beside the existing executables. Models are
# provisioned only on explicit request after extraction; build never downloads
# weights or opens an audio device.
$companionPublish = Join-Path $repoRoot 'tools/live_translate_runtime/Publish.ps1'
if (!$SkipLiveTranslate -and (!(Test-Path -LiteralPath $companionPublish -PathType Leaf) -or
    !(Get-Command dotnet -ErrorAction SilentlyContinue))) {
    throw 'Live Translate packaging requires the .NET 8 SDK. Use -SkipLiveTranslate to build only the existing tools.'
}

$stage = Join-Path ([System.IO.Path]::GetTempPath()) ("sig4a-package-" + [guid]::NewGuid())
try {
    New-Item -Path $stage -ItemType Directory | Out-Null
    Copy-Item -LiteralPath $files -Destination $stage
    if (!$SkipLiveTranslate) {
        $companion = Join-Path $stage 'live-translate'
        & $companionPublish -Output $companion
        if (!(Test-Path -LiteralPath (Join-Path $companion 'Sig4a.LiveTranslate.exe') -PathType Leaf)) {
            throw 'Live Translate helper executable is missing.'
        }
    }
    Compress-Archive -LiteralPath @(Get-ChildItem -LiteralPath $stage | ForEach-Object FullName) -DestinationPath $archive -CompressionLevel Optimal -Force
} finally {
    if (Test-Path -LiteralPath $stage) {
        Remove-Item -LiteralPath $stage -Recurse -Force
    }
}

Write-Host "Built $archive"
