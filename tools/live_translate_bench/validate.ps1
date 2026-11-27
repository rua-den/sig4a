$ErrorActionPreference = 'Stop'
python "$PSScriptRoot\run_benchmark.py" --validate-manifest
if ($LASTEXITCODE -ne 0) { throw "manifest validation failed" }
Write-Host "Phase 1 fixture manifest is frozen and valid. Physical mic/headphone acceptance remains open."
