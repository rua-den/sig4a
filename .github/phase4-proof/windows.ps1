param([Parameter(Mandatory=$true)][string]$ProductRoot,[Parameter(Mandatory=$true)][string]$ExpectedSha)
$ErrorActionPreference='Stop'
$PSNativeCommandUseErrorActionPreference=$true
$product=(Resolve-Path $ProductRoot).Path
$actual=(git -C $product rev-parse HEAD).Trim()
if($actual -cne $ExpectedSha){throw "Wrong product revision $actual, expected $ExpectedSha"}
Write-Host "Native Cleaner product revision: $actual"
$root=Join-Path $env:RUNNER_TEMP ('sig4a-cleaner-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$env:LOCALAPPDATA=Join-Path $root 'local'
$env:USERPROFILE=Join-Path $root 'home'
$env:CU_NO_AUTOSTART='1'
$env:CU_NO_REPAIR='1'
$env:CU_E2E_VISIBLE='1'
Remove-Item Env:HOMEDRIVE,Env:HOMEPATH,Env:CLAUDE_CONFIG_DIR -ErrorAction SilentlyContinue
$data=Join-Path $env:LOCALAPPDATA 'claude-usage'
$fixture=Join-Path $root 'isolated-fixture'
$outside=Join-Path $root 'outside-scope'
$profile=Join-Path $root 'synthetic-profile'
New-Item -ItemType Directory -Force -Path $data,$fixture,$outside,$profile,$env:USERPROFILE | Out-Null
foreach($name in @('.vs','bin/Release','obj/Release','node_modules/fake','TestResults','source','dups')){
  New-Item -ItemType Directory -Force -Path (Join-Path $fixture "work/$name") | Out-Null
}
@{
 'work/demo.sln'='Synthetic solution fixture'
 'work/demo.csproj'='<Project Sdk="Microsoft.NET.Sdk"/>'
 'work/package.json'='{"name":"isolated-fixture"}'
 'work/.vs/safety.bin'='vs-artifact'
 'work/bin/Release/safety.dll'='bin-artifact'
 'work/obj/Release/safety.obj'='obj-artifact'
 'work/node_modules/fake/index.js'='module-artifact'
 'work/TestResults/test.result'='test-artifact'
 'work/source/main.cs'='source-must-not-match'
 'work/dups/duplicate-one.txt'='the same bytes in both disposable duplicate fixtures'
 'work/dups/duplicate-two.txt'='the same bytes in both disposable duplicate fixtures'
}.GetEnumerator() | ForEach-Object { Set-Content -NoNewline -Encoding utf8 -LiteralPath (Join-Path $fixture $_.Key) -Value $_.Value }
Set-Content -NoNewline -Encoding utf8 -LiteralPath (Join-Path $outside 'keep.txt') -Value 'outside fixture must stay untouched'
New-Item -ItemType Directory -Force -Path (Join-Path $outside 'canary/node_modules') | Out-Null
Set-Content -NoNewline -Encoding utf8 -LiteralPath (Join-Path $outside 'canary/package.json') -Value '{"name":"outside-canary"}'
Set-Content -NoNewline -Encoding utf8 -LiteralPath (Join-Path $outside 'canary/node_modules/keep.txt') -Value 'outside artifact must not be offered'
Set-Content -NoNewline -Encoding utf8 -LiteralPath (Join-Path $outside 'duplicate-outside.txt') -Value 'the same bytes in both disposable duplicate fixtures'
Set-Content -NoNewline -Encoding utf8 -LiteralPath (Join-Path $profile 'settings.json') -Value '{}'
@{autostart=$false;widget_enabled=$false;notify_enabled=$false;profiles=@(@{name='fixture';path=$profile});dash_rect=@(45,36,900,650);cleaner_roots=@($fixture);cleaner_skip_recent_days=0;cleaner_dups_min_kb=0} |
  ConvertTo-Json -Depth 7 | Set-Content -Encoding utf8 (Join-Path $data 'config.json')
function Snapshot {
  $all=Get-ChildItem -File -Force -Recurse -LiteralPath $fixture,$outside | Sort-Object FullName
  return (($all | ForEach-Object { "$($_.FullName)|$((Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash)" }) -join "`n")
}
$before=Snapshot
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class Native {
 [StructLayout(LayoutKind.Sequential)] public struct Point {public int X,Y;public Point(int x,int y){X=x;Y=y;}}
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern IntPtr FindWindow(string cls,string title);
 public static IntPtr Find(string cls){return FindWindow(cls,null);}
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h,out uint proc);
 [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h,ref Point pt);
 [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(Point pt);
 [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h,uint flags);
 [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
 [DllImport("user32.dll")] public static extern void mouse_event(uint flags,int x,int y,uint data,IntPtr extra);
 [DllImport("user32.dll")] public static extern void keybd_event(byte key,byte scan,uint flags,IntPtr extra);
}
'@
[void][Native]::SetProcessDPIAware()
function Wait-Proof([string]$label,[scriptblock]$predicate,[int]$seconds=30){
  $timer=[Diagnostics.Stopwatch]::StartNew()
  while($timer.Elapsed.TotalSeconds -lt $seconds){
    $v=& $predicate
    if($v){return $v}
    Start-Sleep -Milliseconds 80
  }
  throw "Timed out waiting for $label"
}
function Assert-Proof($ok,[string]$reason){if(!$ok){throw $reason}}
$out=Join-Path $root 'native-result'
function Read-Stage([string]$name) {
  $path="$out.$name"
  [void](Wait-Proof "stage $name" {(Test-Path $path) -or (Test-Path $out)})
  if(!(Test-Path $path)){throw "Stopped before $($name): $(Get-Content -Raw $out)"}
  return Get-Content -Raw $path | ConvertFrom-Json
}
function Release-Stage([string]$name){New-Item -ItemType File "$out.$name.go" | Out-Null}
function Click-Stage([string]$name,[IntPtr]$hwnd){
  $target=Read-Stage $name
  $point=[Native+Point]::new([int]$target.x,[int]$target.y)
  Assert-Proof ([Native]::ClientToScreen($hwnd,[ref]$point)) "No screen location for $name"
  Assert-Proof ([Native]::GetAncestor([Native]::WindowFromPoint($point),2) -eq $hwnd) "Native click hit an unrelated window at $name"
  Assert-Proof ([Native]::SetCursorPos($point.X,$point.Y)) "Move cursor failed for $name"
  [Native]::mouse_event(2,0,0,0,[IntPtr]::Zero)
  [Native]::mouse_event(4,0,0,0,[IntPtr]::Zero)
  Release-Stage $name
}
$exe=Join-Path $product 'rust/target/debug/claude-usage-dashboard.exe'
$script=Join-Path $PSScriptRoot 'observed-cleaner.js'
$dashboard=$null
try {
  Assert-Proof ([Native]::Find('ClaudeUsageDashboard') -eq [IntPtr]::Zero) 'Unexpected existing dashboard'
  $dashboard=Start-Process $exe -ArgumentList @('--e2e',"`"$script`"","`"$out`"") -PassThru
  $hwnd=Wait-Proof 'visible real Cleaner dashboard' {
    $h=[Native]::Find('ClaudeUsageDashboard')
    if($h -ne [IntPtr]::Zero -and [Native]::IsWindowVisible($h)){$h}
  }
  [uint32]$pidOwner=0
  [void][Native]::GetWindowThreadProcessId($hwnd,[ref]$pidOwner)
  Assert-Proof ($pidOwner -eq $dashboard.Id) 'Unexpected dashboard owner'
  [void][Native]::SetForegroundWindow($hwnd)
  [void](Wait-Proof 'dashboard focus' {[Native]::GetForegroundWindow() -eq $hwnd} 7)
  foreach($stage in @('open-cleaner','run-artifact-scan','select-artifact','open-recycle','cancel-recycle','open-permanent','cancel-permanent','open-duplicates','run-duplicate-scan')){
    Click-Stage $stage $hwnd
  }
  [void](Wait-Proof 'native completion report' {Test-Path $out})
  $report=Get-Content -Raw $out | ConvertFrom-Json
  if($report.error){throw $report.error}
  Assert-Proof ($report.cancelledRecycle -and $report.cancelledPermanent -and $report.duplicatesReadOnly -and $report.noDeletion) 'Unexpected native Cleaner report'
  Assert-Proof ((Snapshot) -ceq $before) 'Synthetic artifacts, source, duplicates, or out-of-scope sentinel changed'
  Write-Host ('P4-NATIVE: '+($report | ConvertTo-Json -Depth 8 -Compress))
  Write-Host 'All fixture files unchanged, verified by sha256 including both duplicates and outside-scope sentinel.'
} catch {
  if(Test-Path "$data/events.log"){Get-Content "$data/events.log" -Tail 30 | Write-Host}
  throw
} finally {
  if($dashboard -and !$dashboard.HasExited){Stop-Process -Id $dashboard.Id -Force -ErrorAction SilentlyContinue}
}
