param([Parameter(Mandatory=$true)][string]$ProductRoot,[Parameter(Mandatory=$true)][string]$ExpectedSha)
$ErrorActionPreference='Stop'
$PSNativeCommandUseErrorActionPreference=$true
$root=(Resolve-Path $ProductRoot).Path
$revision=(git -C $root rev-parse HEAD).Trim()
if($revision -cne $ExpectedSha){throw "Unexpected product revision $revision"}
Write-Host "Native text and usage exact product: $revision"
$dir=Join-Path $env:RUNNER_TEMP ('sig4a-phase5-'+[guid]::NewGuid().ToString('N'))
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
 [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr hwnd,ref Pt pt);
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
rustc +stable --edition=2021 "$PSScriptRoot/launcher.rs" -o "$dir/launcher.exe"
$env:LOCALAPPDATA=Join-Path $dir 'local'
$env:USERPROFILE=Join-Path $dir 'home'
$env:CU_NO_AUTOSTART='1'
$env:CU_NO_REPAIR='1'
$env:CU_E2E_VISIBLE='1'
Remove-Item Env:HOMEDRIVE,Env:HOMEPATH,Env:CLAUDE_CONFIG_DIR -ErrorAction SilentlyContinue
$env:SIG4A_PROOF_DIR=$dir
$env:SIG4A_PROOF_SOURCE=$dir
$common=Get-Content -Raw (Join-Path $PSScriptRoot 'text-observer.js')
foreach($name in @('quick','dashboard')){
  $body=Get-Content -Raw (Join-Path $PSScriptRoot "$name-body.js")
  Set-Content -Encoding utf8 (Join-Path $dir "$name.js") ($common+"`n"+$body)
}
Copy-Item (Join-Path $PSScriptRoot 'dashboard-reopen.js') (Join-Path $dir 'dashboard-reopen.js')
$bin=New-Item -ItemType Directory -Force (Join-Path $dir 'bin')
Copy-Item "$root/rust/target/debug/claude-usage.exe" $bin.FullName
Copy-Item "$root/rust/target/debug/claude-usage-dashboard.exe" "$($bin.FullName)/dashboard-product.exe"
Copy-Item "$dir/launcher.exe" "$($bin.FullName)/claude-usage-dashboard.exe"
$exe=Join-Path $bin.FullName 'claude-usage.exe'
$dashResult="$dir/dashboard.result"
$quickResult="$dir/quick.result"
$reqLog="$dir/provider-requests.txt"
$data=Join-Path $env:LOCALAPPDATA 'claude-usage'
New-Item -ItemType Directory -Force -Path $data,$env:USERPROFILE | Out-Null
$profiles=@();$entries=[ordered]@{};$now=[DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
$names=@('ordinary-b','ordinary-a','limit-without-hook','stale-quota','login-needed','near-5h','refreshing','dead-hook','disabled-hook','active-hook')
$realSettings=@{}
foreach($name in $names){
 $profile=Join-Path $dir $name;New-Item -ItemType Directory -Force $profile | Out-Null
 $profiles+=@{name=$name;path=$profile}
 $entries[$name]=@{path=$profile;five=22;week=26;five_reset='2999-01-01T00:00:00+00:00';week_reset='2999-01-05T00:00:00+00:00';fetched_at=$now;retry_at=$now+180;err=$null}
 $settings=@{sentinel="legacy-$name"}
 if($name -in @('active-hook','dead-hook','disabled-hook')){
   $target=if($name -eq 'dead-hook'){Join-Path $dir 'missing-claude-usage.exe'}else{$exe}
   $settings.hooks=@{UserPromptSubmit=@(@{hooks=@(@{type='command';command="`"$target`" hook"})})}
   if($name -eq 'disabled-hook'){$settings.disableAllHooks=$true}
 }
 $path=Join-Path $profile 'settings.json'
 $settings|ConvertTo-Json -Depth 8|Set-Content -Encoding utf8 $path
 $realSettings[$path]=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
}
$entries['active-hook'].five=98;$entries['limit-without-hook'].five=99;$entries['dead-hook'].week=98;$entries['disabled-hook'].five=99
$entries['near-5h'].five=84;$entries['stale-quota'].fetched_at=$now-1300;$entries['stale-quota'].err='HTTP 429'
$entries['login-needed'].err='HTTP 401';$entries['login-needed'].Remove('five');$entries['login-needed'].Remove('week')
$configPath=Join-Path $data 'config.json'
@{profiles=$profiles;autostart=$false;interval_sec=180;warn_pct=80;block_pct=95;block_enabled=$true;widget_enabled=$false;notify_enabled=$false;widget_label='Legacy fixture badge';widget_font=124;widget_opacity=81;widget_locked=$true;dashboard_accent='orange';compat_sentinel='preserve me';local_ai_backend='ollama';local_ai_endpoint='http://127.0.0.1:18229';local_ai_model='sig4a-fixture';local_ai_managed=$false} | ConvertTo-Json -Depth 8 | Set-Content -Encoding utf8 $configPath
@{ts=$now;block_enabled=$true;warn_pct=80;block_pct=95;busy=@('refreshing');refresh=$null;profiles=$entries}|ConvertTo-Json -Depth 8|Set-Content -Encoding utf8 "$data/usage_cache.json"
$ep='ClaudeUsageDashboard';$qp='ClaudeUsageQuickPopup';$trayProc=$fixture=$provider=$null
function NativePoint($point,[IntPtr]$h){
 $client=[Win+Pt]::new($point.x,$point.y)
 Check ([Win]::ClientToScreen($h,[ref]$client)) 'Failed converting stage point'
 return @{x=$client.x;y=$client.y}
}
function PointStage($base,$name,[IntPtr]$h){
 $v=Stage $base $name
 Click (NativePoint $v $h) $h
}
function ClickStage($base,$name,[IntPtr]$h){
 PointStage $base $name $h
 Release $base $name
}
function ClipboardText {
 try { return Get-Clipboard -Raw -ErrorAction Stop } catch { return '' }
}
function RequestCount {
 if(Test-Path $reqLog){return @(Get-Content $reqLog).Count}
 return 0
}
function NativeFlow($base,$prefix,$kind,$manual,[IntPtr]$h,$isQuick){
 if($manual){
  PointStage $base ($prefix+'paste') $h
  if($isQuick){Check ((RequestCount) -eq $script:beforeQuick) 'No-selection hotkey made an unsolicited inference request'}
  Set-Clipboard -Value 'Synthetic fixture paste.'
  Keys @(17,86)
  Release $base ($prefix+'paste')
 }
 if($kind -eq 'translation'){
  PointStage $base ($prefix+'english') $h
  Keys @(35);Keys @(13)
  Release $base ($prefix+'english')
 }
 if(!$isQuick -or $manual -or $kind -eq 'translation'){ClickStage $base ($prefix+'run') $h}
 $edited=if($kind -eq 'grammar'){'Edited Grammar Result.'}else{'Edited Translation Result.'}
 PointStage $base ($prefix+'edit') $h
 Set-Clipboard -Value $edited
 Keys @(17,65);Keys @(17,86)
 Release $base ($prefix+'edit')
 Set-Clipboard -Value 'Copy sentinel has not been overwritten'
 PointStage $base ($prefix+'copy') $h
 [void](Wait-For 'Windows clipboard contains edited output' {(ClipboardText) -ceq $edited})
 Release $base ($prefix+'copy')
}
try{
 $port=18229
 $provider=Start-Process node.exe -ArgumentList @("`"$PSScriptRoot/provider.mjs`"","$port","`"$reqLog`"") -RedirectStandardOutput "$dir/provider.log" -RedirectStandardError "$dir/provider.error" -PassThru
 [void](Wait-For 'loopback fixture text provider' {try{(Invoke-RestMethod "http://127.0.0.1:$port/api/tags" -NoProxy -TimeoutSec 2).models[0].name -eq 'sig4a-fixture'}catch{$false}})
 foreach($class in @('ClaudeUsageTray',$ep,$qp)){Check ([Win]::Find($class) -eq [IntPtr]::Zero) "Unexpected existing $class"}
 $trayProc=Start-Process $exe -ArgumentList '--hidden' -PassThru
 $tray=Wait-For 'real isolated tray' {$w=[Win]::Find('ClaudeUsageTray');if($w -ne [IntPtr]::Zero){$w}}
 [uint32]$owner=0;[void][Win]::GetWindowThreadProcessId($tray,[ref]$owner);Check ($owner -eq $trayProc.Id) 'Wrong tray owner'
 $launch=[Diagnostics.Stopwatch]::StartNew()
 [void][Win]::PostMessage($tray,0x111,[UIntPtr]1,[IntPtr]::Zero)
 $dash=Wait-For 'native visible dashboard' {Visible $ep}
 $firstVisible=$launch.ElapsedMilliseconds
 Front $dash
 ClickStage $dashResult 'open-usage' $dash
 $priority=Stage $dashResult 'usage-priority'
 $readout=$launch.ElapsedMilliseconds
 Check ($priority.visible -and $priority.rowInfo.Count -eq 10 -and ($readout-$firstVisible) -le 5000) "Ten-profile priority not visible within 5s after open"
 Write-Host ("P5-UI-002: launch-to-visible=$firstVisible ms; visible-to-priority="+($readout-$firstVisible)+" ms; "+($priority.rowInfo|ConvertTo-Json -Compress -Depth 7))
 Release $dashResult 'usage-priority'
 ClickStage $dashResult 'open-ai' $dash
 NativeFlow $dashResult 'dg-' 'grammar' $true $dash $false
 ClickStage $dashResult 'open-translation' $dash
 NativeFlow $dashResult 'dt-' 'translation' $true $dash $false
 foreach($name in @('open-settings','alerts-tab','hooks-tab','appearance-tab')){ClickStage $dashResult $name $dash}
 PointStage $dashResult 'change-accent' $dash
 Keys @(35);Keys @(13)
 Release $dashResult 'change-accent'
 $dashEvidence=Result $dashResult
 Check ($dashEvidence.legacyLabel -eq 'Legacy fixture badge') 'Legacy appearance not rendered'
 [void](Wait-For 'unrelated accent save persisted' {(Get-Content -Raw $configPath|ConvertFrom-Json).dashboard_accent -eq 'purple'})
 Close-Escape $dash $ep
 Remove-Item "$dashResult*" -Force
 Copy-Item "$dir/dashboard-reopen.js" "$dir/dashboard.js" -Force
 [void][Win]::PostMessage($tray,0x111,[UIntPtr]1,[IntPtr]::Zero)
 $dash=Wait-For 'reopened real dashboard' {Visible $ep}
 $again=Result $dashResult
 $after=Get-Content -Raw $configPath|ConvertFrom-Json
 Check ($again.savedAccent -eq 'purple' -and $after.widget_label -eq 'Legacy fixture badge' -and $after.widget_font -eq 124 -and $after.widget_opacity -eq 81 -and $after.widget_locked -and $after.compat_sentinel -eq 'preserve me') 'Unrelated save did not preserve legacy config'
 foreach($item in $realSettings.GetEnumerator()){Check ((Get-FileHash -LiteralPath $item.Key -Algorithm SHA256).Hash -ceq $item.Value) 'A hook settings fixture was changed'}
 Close-Escape $dash $ep
 Write-Host 'P5-UI-003: Synthetic legacy widget and hook preferences survived unrelated native setting change and reopen; profile settings SHA-256 unchanged.'
 $ready="$dir/fixture.json";$fixture=Start-Process powershell.exe -ArgumentList @('-NoProfile','-STA','-ExecutionPolicy','Bypass','-File',"`"$PSScriptRoot/fixture.ps1`"",'-Ready',"`"$ready`"") -PassThru
 [void](Wait-For 'synthetic WPF fixture' {Test-Path $ready})
 $f=Get-Content -Raw $ready|ConvertFrom-Json;$fh=[IntPtr]$f.hwnd
 foreach($kind in @('grammar','translation')){
  foreach($manual in @($false,$true)){
   Front $fh
   if($manual){Click $f.button $fh}else{Click $f.input $fh;Keys @(17,65)}
   Set-Clipboard -Value 'Selection did not touch clipboard'
   $script:beforeQuick=RequestCount
   $vk=if($kind -eq 'grammar'){71}else{84}
   Keys @(17,18,$vk)
   $q=Wait-For "native quick $kind" {Visible $qp}
   Check ((ClipboardText) -ceq 'Selection did not touch clipboard') 'Hotkey changed the native clipboard'
   Front $q
   NativeFlow $quickResult '' $kind $manual $q $true
   $v=Result $quickResult
   Check ($v.tool -eq $kind -and $(if($manual){!$v.initial}else{$v.initial -ceq 'Synthetic phase five selection'})) 'Wrong selected/manual source for quick tool'
   Close-Escape $q $qp
   Remove-Item "$quickResult*" -Force
  }
 }
 Write-Host 'P5-UI-001: Actual tray Ctrl+Alt+G/T with native selected text and no-selection Ctrl+V; real Dashboard and quick Grammar/Translation, English target, output edits and real clipboard.'
}catch{
 if(Test-Path "$data/events.log"){Get-Content "$data/events.log" -Tail 35|Write-Host}
 if(Test-Path "$dir/provider.error"){Get-Content "$dir/provider.error" -Tail 15|Write-Host}
 throw
}finally{
 foreach($proc in @($fixture,$provider,$trayProc)){if($proc -and !$proc.HasExited){Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue}}
}
