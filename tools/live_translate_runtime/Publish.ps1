# Run on Windows with the .NET 8 SDK. Build-stage network use may restore pinned NuGet dependencies.
param([Parameter(Mandatory)][string]$Output)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$out = [IO.Path]::GetFullPath($Output)
if (Test-Path -LiteralPath $out) {
    if (!(Test-Path -LiteralPath $out -PathType Container) -or @(Get-ChildItem -Force -LiteralPath $out).Count) {
        throw 'Live Translate publish output must be a new or empty folder.'
    }
}
[void][IO.Directory]::CreateDirectory($out)
$project = Join-Path $PSScriptRoot 'Sig4a.LiveTranslate.csproj'
# Lock file contains the full project RuntimeIdentifiers graph. Supplying
# -r to restore narrows it on some SDKs; select win-x64 only at publish.
dotnet restore $project -p:SelfContained=true --locked-mode
if ($LASTEXITCODE -ne 0) { throw 'Pinned helper dependency restore failed.' }
$assets = Join-Path $PSScriptRoot 'obj/project.assets.json'
$restore = Get-Content -Raw -LiteralPath $assets | ConvertFrom-Json -AsHashtable
# NuGet versions differ in whether the Windows default platform version
# appears in the asset key. Read the selected framework key from the graph.
$frameworks = @($restore.project.frameworks.Keys)
if ($frameworks.Count -ne 1) { throw 'Expected a single declared helper target framework.' }
foreach ($rid in @('win-x64','linux-x64')) {
    $target = $frameworks[0] + '/' + $rid
    if (!$restore.targets.ContainsKey($target)) { throw "Declared helper restore target missing: $target" }
}
$packageFile = @($restore.packageFolders.Keys | ForEach-Object {
    Join-Path $_ 'sharpziplib/1.4.2/sharpziplib.1.4.2.nupkg'
} | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf })
if (!$packageFile.Count -or
    (Get-FileHash -LiteralPath $packageFile[0] -Algorithm SHA256).Hash -ine 'fe0895aa2930a2b1b65caa9f37db8bca35125ebdf9cc1b99d855f6afc5ff5946') {
    throw 'Restored SharpZipLib package does not match exact upstream bytes.'
}
dotnet publish $project --no-restore -c Release -r win-x64 --self-contained true -p:PublishSingleFile=false -o $out
if ($LASTEXITCODE -ne 0) { throw 'Live Translate helper publish failed.' }
foreach ($item in @('Sig4a.LiveTranslate.exe', 'onnxruntime.dll', 'sherpa-onnx-c-api.dll', 'ICSharpCode.SharpZipLib.dll')) {
    if (!(Test-Path -LiteralPath (Join-Path $out $item) -PathType Leaf)) { throw "Required published helper file missing: $item" }
}
$sharp = Join-Path $out 'ICSharpCode.SharpZipLib.dll'
if ((Get-FileHash -Algorithm SHA256 -LiteralPath $sharp).Hash -ine '32ea2d0ce3512e74f1c7ad82591fe67e6b8939d76a8a4ff9c93ead030131e71c') {
    throw 'Unexpected SharpZipLib publish selection.'
}
foreach ($unshipped in @('ICSharpCode.SharpZipLib.pdb','ICSharpCode.SharpZipLib.xml')) {
    $generated = Join-Path $out $unshipped
    if (Test-Path -LiteralPath $generated -PathType Leaf) { Remove-Item -LiteralPath $generated }
}
$noticeDir = Join-Path $out 'notices'
& (Join-Path $PSScriptRoot 'package-notices.ps1') -Assets (Join-Path $PSScriptRoot 'obj/project.assets.json') -Destination $noticeDir
$upstreamFiles = Join-Path $PSScriptRoot 'third-party'
$checked = @{}
foreach ($line in Get-Content -LiteralPath (Join-Path $upstreamFiles 'CHECKSUMS.sha256')) {
    if ($line -cnotmatch '^([0-9a-f]{64})  ([A-Za-z0-9_.-]+\.txt)$') { throw 'Invalid notice manifest.' }
    $digest=$Matches[1]; $name=$Matches[2]
    $file = Join-Path $upstreamFiles $name
    if ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ine $digest) { throw "Changed upstream notice: $name" }
    Copy-Item -LiteralPath $file -Destination $noticeDir
    $checked[$name] = $true
}
foreach ($name in @('sherpa-onnx-v1.12.30-LICENSE.txt','onnxruntime-v1.23.2-LICENSE.txt','onnxruntime-v1.23.2-THIRD-PARTY-NOTICES.txt','llama.cpp-b12776-LICENSE.txt','HY-MT1.5-1.8B-LICENSE.txt','HY-MT1.5-1.8B-NOTICE.txt','SharpZipLib-v1.4.2-LICENSE.txt')) {
    if (!$checked.ContainsKey($name)) { throw "Required helper notice not staged: $name" }
}
foreach ($name in @('Provision-Model.ps1','Provision-Translation.ps1','README.md','model-assets.json','translation-assets.json')) {
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination $out
}
