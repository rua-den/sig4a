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
dotnet publish (Join-Path $PSScriptRoot 'Sig4a.LiveTranslate.csproj') -c Release -r win-x64 --self-contained true -p:PublishSingleFile=false -o $out
if ($LASTEXITCODE -ne 0) { throw 'Live Translate helper publish failed.' }
foreach ($item in @('Sig4a.LiveTranslate.exe', 'onnxruntime.dll', 'sherpa-onnx-c-api.dll')) {
    if (!(Test-Path -LiteralPath (Join-Path $out $item) -PathType Leaf)) { throw "Required published helper file missing: $item" }
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
foreach ($name in @('sherpa-onnx-v1.12.30-LICENSE.txt','onnxruntime-v1.23.2-LICENSE.txt','onnxruntime-v1.23.2-THIRD-PARTY-NOTICES.txt','llama.cpp-b12776-LICENSE.txt','HY-MT1.5-1.8B-LICENSE.txt')) {
    if (!$checked.ContainsKey($name)) { throw "Required helper notice not staged: $name" }
}
foreach ($name in @('Provision-Model.ps1','Provision-Translation.ps1','README.md','model-assets.json','translation-assets.json')) {
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination $out
}
