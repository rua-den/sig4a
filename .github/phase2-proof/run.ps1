$ErrorActionPreference='Stop'
$PSNativeCommandUseErrorActionPreference=$true
$root=(Resolve-Path "$PSScriptRoot/../..").Path
$dir=Join-Path $env:RUNNER_TEMP 'sig4a-phase2'
New-Item -ItemType Directory -Force -Path $dir | Out-Null
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class Win {
 [StructLayout(LayoutKind.Sequential)] public struct Pt {public int x,y;public Pt(int a,int b){x=a;y=b;}}
 [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern IntPtr FindWindow(string cls,string title);
 public static IntPtr Find(string cls){return FindWindow(cls,null);}
 [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h,out uint p);
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint m,UIntPtr w,IntPtr l);
 [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h,uint flags);
 [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(Pt p);
 [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
 [DllImport("user32.dll")] public static extern void mouse_event(uint flags,int dx,int dy,uint d,IntPtr x);
 [DllImport("user32.dll")] public static extern void keybd_event(byte key,byte scan,uint flags,IntPtr x);
}
'@
[void][Win]::SetProcessDPIAware()
function Check($ok,$label){if(!$ok){throw $label}}
function Wait-For($label,[scriptblock]$check,[int]$seconds=25){
 $t=[Diagnostics.Stopwatch]::StartNew()
 while($t.Elapsed.TotalSeconds -lt $seconds){$v=& $check;if($v){return $v};Start-Sleep -Milliseconds 120}
 throw "Timeout: $label"
}
function Stage($base,$stage) {
 $path="$base.$stage"
 [void](Wait-For "stage $stage" {(Test-Path $path) -or (Test-Path $base)} 23)
 if(!(Test-Path $path)){throw "Stopped before stage ${stage}: $(Get-Content -Raw $base)"}
 Get-Content -Raw $path | ConvertFrom-Json
}
function Release($base,$stage){New-Item -ItemType File "$base.$stage.go" | Out-Null}
function Result($base) {
 [void](Wait-For 'WebView2 result' {Test-Path $base} 24)
 $x=Get-Content -Raw $base|ConvertFrom-Json
 if($x.error){throw $x.error};$x|ConvertTo-Json -Compress -Depth 7|Write-Host
 return $x
}
function Keys([int[]]$keys){
 foreach($k in $keys){[Win]::keybd_event($k,0,0,[IntPtr]::Zero)}
 for($i=$keys.Length-1;$i -ge 0;$i--){[Win]::keybd_event($keys[$i],0,2,[IntPtr]::Zero)}
 Start-Sleep -Milliseconds 120
}
function Visible([string]$name){
 $h=[Win]::Find($name)
 if($h -ne [IntPtr]::Zero -and [Win]::IsWindowVisible($h)){$h}
}
function Front($h){
 [void][Win]::SetForegroundWindow($h)
 [void](Wait-For "foreground $h" {[Win]::GetForegroundWindow() -eq $h} 5)
}
function Click($point,$h){
 Check ([Win]::GetAncestor([Win]::WindowFromPoint([Win+Pt]::new($point.x,$point.y)),2) -eq $h) 'Synthetic click target not at expected location'
 Check ([Win]::SetCursorPos($point.x,$point.y)) 'Failed to position native pointer'
 [Win]::mouse_event(2,0,0,0,[IntPtr]::Zero);[Win]::mouse_event(4,0,0,0,[IntPtr]::Zero)
}
function Close-Escape($hwnd,$name){
 Front $hwnd;Keys @(27)
 [void](Wait-For "$name to close on Escape" {[Win]::Find($name) -eq [IntPtr]::Zero})
}
$env:LOCALAPPDATA=Join-Path $dir 'local'
$env:USERPROFILE=Join-Path $dir 'home'
$env:CU_NO_AUTOSTART='1';$env:CU_NO_REPAIR='1'
$env:CU_E2E_VISIBLE='1'
$env:SIG4A_PROOF_DIR=$dir
$env:SIG4A_PROOF_SOURCE=$PSScriptRoot
$bin=New-Item -ItemType Directory -Force (Join-Path $dir 'bin')
Copy-Item "$root/rust/target/debug/claude-usage.exe" $bin.FullName
Copy-Item "$root/rust/target/debug/claude-usage-dashboard.exe" "$($bin.FullName)/dashboard-product.exe"
rustc --edition=2021 "$PSScriptRoot/launcher.rs" -o "$($bin.FullName)/claude-usage-dashboard.exe"
$dashResult="$dir/dashboard.result"
$quickResult="$dir/quick.result" 
$data=Join-Path $env:LOCALAPPDATA 'claude-usage'
New-Item -ItemType Directory -Force -Path $data,$env:USERPROFILE | Out-Null
$profiles=@();$entries=[ordered]@{};$now=[DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
foreach($name in @('healthy','near-5h','stale-7d','ordinary-4','ordinary-5','ordinary-6','ordinary-7','ordinary-8')){
 $profile=Join-Path $dir $name;New-Item -ItemType Directory -Force $profile | Out-Null
 $profiles+=@{name=$name;path=$profile}
 $entries[$name]=@{path=$profile;five=21;week=25;five_reset='2999-01-01T00:00:00+00:00';week_reset='2999-01-01T00:00:00+00:00';fetched_at=$now;retry_at=$now+180;err=$null}
}
$entries['near-5h'].five=88;$entries['stale-7d'].week=51;$entries['stale-7d'].err='HTTP 429'
@{profiles=$profiles;autostart=$false;interval_sec=180;warn_pct=80;block_pct=95;widget_enabled=$false;notify_enabled=$false;local_ai_endpoint='http://127.0.0.1:18228';dashboard_accent='orange'} | ConvertTo-Json -Depth 6 | Set-Content -Encoding utf8 "$data/config.json"
@{ts=$now;block_enabled=$true;warn_pct=80;block_pct=95;busy=@();refresh=$null;profiles=$entries} | ConvertTo-Json -Depth 6 | Set-Content -Encoding utf8 "$data/usage_cache.json"
$ep='ClaudeUsageDashboard';$qp='ClaudeUsageQuickPopup';$trayProc=$fixture=$null
try{
 foreach($class in @('ClaudeUsageTray',$ep,$qp)){Check ([Win]::Find($class) -eq [IntPtr]::Zero) "Unexpected existing $class"}
 $trayProc=Start-Process "$($bin.FullName)/claude-usage.exe" -ArgumentList '--hidden' -PassThru
 $tray=Wait-For 'isolated tray' {$w=[Win]::Find('ClaudeUsageTray');if($w -ne [IntPtr]::Zero){$w}}
 [uint32]$owner=0;[void][Win]::GetWindowThreadProcessId($tray,[ref]$owner)
 Check ($owner -eq $trayProc.Id) 'Tray owner mismatch'
 [void][Win]::PostMessage($tray,0x111,[UIntPtr]1,[IntPtr]::Zero)
 $dash=Wait-For 'tray-launched dashboard' {Visible $ep}
 $overview=Stage $dashResult 'overview'
 $overview|ConvertTo-Json -Compress -Depth 7|Write-Host
 Release $dashResult 'overview'
 $local=Stage $dashResult 'localai'
 $local|ConvertTo-Json -Compress|Write-Host
 Front $dash
 Release $dashResult 'localai'
 foreach($route in @('overview','ai','usage','cleaner','settings')){
  [void](Stage $dashResult "tab_$route")
  Keys @(9)
  Release $dashResult "tab_$route"
  [void](Stage $dashResult "enter_$route")
  Keys @(13)
  Release $dashResult "enter_$route"
 }
 Check ((Result $dashResult).narrowWidth -eq 420) 'Dashboard lost the 420 px width'
 Close-Escape $dash $ep
 Start-Sleep -Milliseconds 800
 $ready="$dir/fixture.json"
 $fixture=Start-Process powershell.exe -ArgumentList @('-NoProfile','-STA','-ExecutionPolicy','Bypass','-File',"`"$PSScriptRoot/fixture.ps1`"",'-Ready',"`"$ready`"") -PassThru
 [void](Wait-For 'synthetic WPF fixture' {Test-Path $ready})
 $f=Get-Content -Raw $ready | ConvertFrom-Json
 $fh=[IntPtr]$f.hwnd
 Front $fh;Click $f.input $fh;Keys @(17,65);Keys @(17,18,71)
 $q=Wait-For 'Ctrl+Alt+G' {Visible $qp};Start-Sleep -Milliseconds 600
 $g=Result $quickResult
 Check ($g.tool -eq 'tool-grammar' -and $g.text -eq 'Synthetic phase two selection') 'Wrong Grammar hotkey result'
 Close-Escape $q $qp;Remove-Item $quickResult;Start-Sleep -Milliseconds 700
 Front $fh;Click $f.button $fh;Keys @(17,18,84)
 $q=Wait-For 'Ctrl+Alt+T' {Visible $qp}
 [void](Stage $quickResult 'paste')
 Set-Clipboard -Value 'Synthetic phase two paste'
 Front $q;Keys @(17,86)
 Check ((Result $quickResult).text -eq 'Synthetic phase two paste') 'Translation clipboard fallback failed'
 Close-Escape $q $qp;Remove-Item $quickResult;Start-Sleep -Milliseconds 700
 Front $fh;Keys @(17,18,79)
 $picker=Wait-For 'Ctrl+Alt+O and native picker' {Visible 'KaneRegion'}
 Front $picker;Keys @(27)
 $q=Wait-For 'OCR popup after picker Esc' {Visible $qp}
 Check ((Result $quickResult).noImage) 'OCR native picker did not cancel'
 Close-Escape $q $qp
 Write-Host 'P2-UI-001, P2-UI-002 and P2-UI-003 passed on native Windows.'
}catch{
 if(Test-Path "$data/events.log"){Get-Content "$data/events.log" -Tail 35 | Write-Host}
 throw
}finally{
 foreach($p in @($fixture,$trayProc)){if($p -and !$p.HasExited){Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue}}
}
