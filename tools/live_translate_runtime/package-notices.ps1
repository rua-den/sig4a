# Build-time only. Do not include this script in the operator package.
param(
  [Parameter(Mandatory)][string]$Assets,
  [Parameter(Mandatory)][string]$Destination
)
$ErrorActionPreference = 'Stop'
$files = [ordered]@{}
New-Item -ItemType Directory -Force -Path $Destination | Out-Null
$naudio = 'NAudio-v2.2.1-LICENSE.txt'
$naudioSource = Join-Path $PSScriptRoot "third-party/$naudio"
$naudioDigest = ((Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'third-party/NAudio-v2.2.1-SHA256.txt')) -split '\s+')[0]
if ($naudioDigest -cnotmatch '^[0-9a-f]{64}$' -or
    (Get-FileHash -Algorithm SHA256 -LiteralPath $naudioSource).Hash -ine $naudioDigest) {
  throw 'Cached NAudio license does not match the checked-in upstream checksum.'
}
Copy-Item -LiteralPath $naudioSource -Destination $Destination
$files[$naudio] = @{
  source = 'https://raw.githubusercontent.com/naudio/NAudio/b5d5ff83fd378f046398891fe5cd99426ce44732/license.txt'
  version = '2.2.1'
  sha256 = $naudioDigest
}

# Resolve the exact runtime pack used by the preceding self-contained publish
# from that project's assets file, rather than assuming an SDK/runtime patch.
$project = Get-Content -Raw -LiteralPath $Assets | ConvertFrom-Json -AsHashtable
$pack = 'Microsoft.NETCore.App.Runtime.win-x64'
$versions = @(
  foreach ($framework in $project.project.frameworks.Values) {
    foreach ($entry in $framework.downloadDependencies) {
      if ($entry.name -ieq $pack) { [string]$entry.version }
    }
  }
  foreach ($key in $project.libraries.Keys) {
    if ($key -ilike "$pack/*") { $key.Substring($pack.Length + 1) }
  }
)
$versions = @(
  $versions | ForEach-Object {
    # NuGet serializers may use either [v] or [v, v] for an exact range.
    if ($_ -cmatch '^8\.0\.\d+$') { $_ }
    elseif ($_ -cmatch '^\[(8\.0\.\d+)(?:,\s*\1)?\]$') { $Matches[1] }
    else { throw 'Runtime pack version is not an exact .NET 8 patch.' }
  } | Sort-Object -Unique
)
if ($versions.Count -ne 1 -or $versions[0] -cnotmatch '^8\.0\.\d+$') {
  throw 'Cannot identify an exact .NET 8 Windows x64 runtime package in publish assets.'
}
$version = $versions[0]
$lowerPack = $pack.ToLowerInvariant()
$nupkg = "$lowerPack.$version.nupkg"
$cache = @($project.packageFolders.Keys | ForEach-Object { Join-Path $_ "$lowerPack/$version/$nupkg" } |
  Where-Object { (Test-Path -LiteralPath $_ -PathType Leaf) -and (Test-Path -LiteralPath "$_.sha512" -PathType Leaf) })
if (!$cache.Count) { throw 'Published runtime NuGet archive and cached checksum are required.' }
$archive = $cache[0]
$cachedHash = (Get-Content -Raw -LiteralPath "$archive.sha512").Trim()
$actualHash = [Convert]::ToBase64String([Convert]::FromHexString((Get-FileHash -Algorithm SHA512 -LiteralPath $archive).Hash))
if ($cachedHash -cne $actualHash) { throw 'Runtime package bytes differ from the NuGet cache checksum.' }

$runtimeFiles = [ordered]@{
  'LICENSE.TXT' = 'dotnet-runtime-LICENSE.txt'
  'THIRD-PARTY-NOTICES.TXT' = 'dotnet-runtime-THIRD-PARTY-NOTICES.txt'
}
$zip = [IO.Compression.ZipFile]::OpenRead($archive)
try {
  foreach ($member in $runtimeFiles.Keys) {
    $entry = $zip.GetEntry($member)
    if (!$entry -or $entry.Length -lt 100) { throw "Runtime pack lacks required legal file: $member" }
    $target = Join-Path $Destination $runtimeFiles[$member]
    $inputStream = $entry.Open()
    try {
      $outputStream = [IO.File]::Open($target, [IO.FileMode]::CreateNew)
      try { $inputStream.CopyTo($outputStream) } finally { $outputStream.Dispose() }
    } finally { $inputStream.Dispose() }
    $files[$runtimeFiles[$member]] = @{
      source = "https://api.nuget.org/v3-flatcontainer/$lowerPack/$version/$nupkg"
      package_sha512_base64 = $actualHash
      package_member = $member
      version = $version
      sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $target).Hash.ToLowerInvariant()
    }
  }
} finally { $zip.Dispose() }
$files | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $Destination 'notice-provenance.json') -Encoding utf8
