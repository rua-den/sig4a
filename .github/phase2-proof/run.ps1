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
function Web([string]$js){
 $js | Set-Content -Encoding utf8 "$dir/eval.js"
 $r=node "$PSScriptRoot/cdp.mjs" "$dir/eval.js"
 ConvertFrom-Json -InputObject ($r -join "`n")
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
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=18227 --remote-debugging-address=127.0.0.1'
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
 $trayProc=Start-Process "$root/rust/target/debug/claude-usage.exe" -ArgumentList '--hidden' -PassThru
 $tray=Wait-For 'isolated tray' {$w=[Win]::Find('ClaudeUsageTray');if($w -ne [IntPtr]::Zero){$w}}
 [uint32]$owner=0;[void][Win]::GetWindowThreadProcessId($tray,[ref]$owner)
 Check ($owner -eq $trayProc.Id) 'Tray owner mismatch'
 [void][Win]::PostMessage($tray,0x111,[UIntPtr]1,[IntPtr]::Zero)
 $dash=Wait-For 'tray-launched dashboard' {Visible $ep}
 $overview=Web @'
(()=>({route:document.querySelector('.toolnav [aria-current]').dataset.tool,rows:[...document.querySelectorAll('.quota-row')].map(x=>x.innerText),keys:[...document.querySelectorAll('.tile-hotkey')].map(x=>x.innerText),above:document.querySelector('#overview-quota').getBoundingClientRect().top<document.querySelector('.overview').getBoundingClientRect().top,tokens:['--bg','--panel','--accent'].map(k=>getComputedStyle(document.documentElement).getPropertyValue(k).trim()),font:getComputedStyle(document.body).fontFamily}))()
'@
 $overview | ConvertTo-Json -Depth 6 -Compress | Write-Host
 Check ($overview.route -eq 'overview' -and $overview.above -and $overview.rows[0] -match 'near-5h.*5h 88%.*gần giới hạn' -and $overview.rows[1] -match 'stale-7d.*7d 51%.*dữ liệu cũ') 'Quota-first priority and text statuses failed'
 Check (($overview.keys -join ',') -eq 'Ctrl+Alt+G,Ctrl+Alt+T,Ctrl+Alt+O') 'Missing hotkey labels'
 Check (($overview.tokens -join ',') -eq '#0a0a0a,#141414,#fab283' -and $overview.font -match 'Cascadia Mono') 'Phase 1 theme mismatch'
 $small=Web @'
(async()=>{const d=devicePixelRatio;await api.set_rect(500,110,Math.round(420*d),Math.round(400*d));await syncWin();await new Promise(r=>setTimeout(r,200));const m=document.querySelector('main'),link=document.querySelector('.tile[onclick*="localai"]');m.scrollTop=m.scrollHeight;const before=m.scrollTop;link.focus({preventScroll:true});link.click();return {before,after:m.scrollTop,route:document.querySelector('.toolnav [aria-current]').dataset.tool,visible:document.querySelector('#tool-localai').getBoundingClientRect().height>0,helper:link.innerText,focus:document.activeElement.dataset.tool,w:innerWidth};})()
'@
 $small | ConvertTo-Json -Compress | Write-Host
 Check ($small.before -gt 0 -and $small.after -eq 0 -and $small.route -eq 'settings' -and $small.visible -and $small.helper -match 'Cài đặt' -and $small.focus -eq 'settings' -and $small.w -eq 420) 'Scrolled Home Local AI did not reset to Settings at 420 px'
 [void](Web "document.querySelector('#accent').focus();document.activeElement.id")
 Front $dash
 foreach($route in @('overview','ai','usage','cleaner','settings')){
  $focus=$null
  for($i=0;$i -lt 12;$i++){
   Keys @(9)
   $focus=Web "({route:document.activeElement.dataset.tool||'',visible:document.activeElement.matches(':focus-visible'),edge:getComputedStyle(document.activeElement).borderTopColor})"
   if($focus.route -eq $route){break}
  }
  Check ($focus.route -eq $route -and $focus.visible -and $focus.edge -eq 'rgb(250, 178, 131)') "Focus not visible on route $route"
  Keys @(13)
  $view=Web "(()=>{const m=document.querySelector('main');return {route:document.querySelector('.toolnav [aria-current]').dataset.tool,panels:[...document.querySelectorAll('.toolpanel.on')].map(x=>x.id),w:innerWidth,overflow:Math.max(m.scrollWidth-m.clientWidth,document.documentElement.scrollWidth-innerWidth)};})()"
  $view | ConvertTo-Json -Compress | Write-Host
  Check ($view.route -eq $route -and $view.w -eq 420 -and $view.overflow -le 1 -and $view.panels.Count -eq 1) "Tab/Enter route or overflow failed: $route"
 }
 $t=Web "(async()=>{const a=document.querySelector('#accent');a.value='purple';a.dispatchEvent(new Event('change',{bubbles:true}));await new Promise(r=>setTimeout(r,380));return {color:getComputedStyle(document.documentElement).getPropertyValue('--accent').trim(),saved:(await api.state()).cfg.dashboard_accent}})()"
 Check ($t.color -eq '#c7a0ff' -and $t.saved -eq 'purple') 'Purple accent failed to persist'
 Close-Escape $dash $ep
 Start-Sleep -Milliseconds 800
 $ready="$dir/fixture.json"
 $fixture=Start-Process powershell.exe -ArgumentList @('-NoProfile','-STA','-ExecutionPolicy','Bypass','-File',"`"$PSScriptRoot/fixture.ps1`"",'-Ready',"`"$ready`"") -PassThru
 [void](Wait-For 'synthetic WPF fixture' {Test-Path $ready})
 $f=Get-Content -Raw $ready | ConvertFrom-Json
 $fh=[IntPtr]$f.hwnd
 Front $fh;Click $f.input $fh;Keys @(17,65);Keys @(17,18,71)
 $q=Wait-For 'Ctrl+Alt+G' {Visible $qp};Start-Sleep -Milliseconds 600
 $g=Web "({quick:window.__QUICK_MODE__,tool:document.querySelector('.toolpanel.on')?.id,value:document.querySelector('#grammar-in').value})"
 $g | ConvertTo-Json -Compress | Write-Host
 Check ($g.quick -and $g.tool -eq 'tool-grammar' -and $g.value -eq 'Synthetic phase two selection') 'Grammar did not receive actual selected text'
 Close-Escape $q $qp;Start-Sleep -Milliseconds 700
 Front $fh;Click $f.button $fh;Keys @(17,18,84)
 $q=Wait-For 'Ctrl+Alt+T' {Visible $qp};Start-Sleep -Milliseconds 600
 $tr=Web "({quick:window.__QUICK_MODE__,tool:document.querySelector('.toolpanel.on')?.id,value:document.querySelector('#translation-in').value,focus:document.activeElement.id})"
 Check ($tr.quick -and $tr.tool -eq 'tool-translation' -and $tr.value -eq '' -and $tr.focus -eq 'translation-in') 'Translation focus/no-selection fallback failed'
 Set-Clipboard -Value 'Synthetic phase two paste'
 Front $q;Keys @(17,86)
 Check ((Web "document.querySelector('#translation-in').value") -eq 'Synthetic phase two paste') 'Real clipboard paste failed'
 Write-Host 'Ctrl+Alt+T fallback focus and actual clipboard paste verified.'
 Close-Escape $q $qp;Start-Sleep -Milliseconds 700
 Front $fh;Keys @(17,18,79)
 $picker=Wait-For 'Ctrl+Alt+O and native picker' {Visible 'KaneRegion'}
 Front $picker;Keys @(27)
 $q=Wait-For 'OCR popup after picker Esc' {Visible $qp};Start-Sleep -Milliseconds 450
 $ocr=Web "({quick:window.__QUICK_MODE__,tool:document.querySelector('.toolpanel.on')?.id,status:document.querySelector('#ocr-status').innerText,noImage:!document.querySelector('#ocr-preview').getAttribute('src')})"
 $ocr | ConvertTo-Json -Compress | Write-Host
 Check ($ocr.quick -and $ocr.tool -eq 'tool-ocr' -and $ocr.status -match 'huỷ' -and $ocr.noImage) 'OCR picker Escape did not return a cancelled OCR popup'
 Close-Escape $q $qp
 Write-Host 'P2-UI-001, P2-UI-002 and P2-UI-003 passed on native Windows.'
}catch{
 if(Test-Path "$data/events.log"){Get-Content "$data/events.log" -Tail 35 | Write-Host}
 throw
}finally{
 foreach($p in @($fixture,$trayProc)){if($p -and !$p.HasExited){Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue}}
}
