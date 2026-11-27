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
Compress-Archive -LiteralPath $files -DestinationPath $archive -CompressionLevel Optimal -Force

Write-Host "Built $archive"
