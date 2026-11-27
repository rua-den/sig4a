# Run only in an isolated scratch directory, not in a user installation.
# Neither captures a microphone nor changes the extracted input package.
param([Parameter(Mandatory)][string]$Published, [Parameter(Mandatory)][string]$Scratch,[switch]$Vietnamese)
$ErrorActionPreference='Stop'
$PSNativeCommandUseErrorActionPreference=$true
$source=[IO.Path]::GetFullPath($Published)
$dir=[IO.Path]::GetFullPath($Scratch)
if ($dir -eq $source -or (Test-Path -LiteralPath $dir)) { throw 'Use a new isolated scratch directory.' }
if (!(Test-Path -LiteralPath "$source/models/zipformer")) { throw 'Provision verified EN assets in the test package first.' }
if ($Vietnamese -and !(Test-Path -LiteralPath "$source/models/hymt")) { throw 'Provision verified VI assets first.' }
dotnet publish "$PSScriptRoot/tests/inference/InferenceSmoke.csproj" -c Release -r win-x64 --self-contained true -o $dir
if ($LASTEXITCODE -ne 0) { throw 'Unable to publish isolated smoke harness.' }
New-Item -ItemType Directory -Path "$dir/models" | Out-Null
Copy-Item -LiteralPath "$source/models/zipformer" -Destination "$dir/models" -Recurse
if ($Vietnamese) { Copy-Item -LiteralPath "$source/models/hymt" -Destination "$dir/models" -Recurse }
powershell.exe -NoProfile -File "$PSScriptRoot/tests/New-SyntheticFixture.ps1" -Output "$dir/synthetic.wav"
if ($LASTEXITCODE -ne 0) { throw 'Could not synthesize controlled fixture.' }
$optional=@()
if ($Vietnamese) { $optional += '--vi' }
& "$dir/Sig4a.LiveTranslate.InferenceSmoke.exe" "$dir/synthetic.wav" @optional
if ($LASTEXITCODE -ne 0) { throw 'Isolated native Windows inference smoke failed.' }
