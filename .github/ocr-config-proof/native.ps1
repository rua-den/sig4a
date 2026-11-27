param(
  [Parameter(Mandatory=$true)][string]$ProductRoot,
  [Parameter(Mandatory=$true)][string]$ExpectedSha,
  [string]$BinDirectory
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$root = (Resolve-Path $ProductRoot).Path
$revision = (git -C $root rev-parse HEAD).Trim()
if ($revision -cne $ExpectedSha) { throw "Expected $ExpectedSha but checked out $revision" }
Write-Host "Native ordinary release product revision: $revision"
$bin=if($BinDirectory){(Resolve-Path $BinDirectory).Path}else{(Resolve-Path (Join-Path $root 'rust/target/release')).Path}
$realTray=Join-Path $bin 'claude-usage.exe'
$realDashboard=Join-Path $bin 'claude-usage-dashboard.exe'
foreach($f in @($realTray,$realDashboard)){
  if(!(Test-Path -LiteralPath $f)){throw "Missing expected colocated release executable $f"}
  Write-Host "Release SHA-256 $([IO.Path]::GetFileName($f)): $((Get-FileHash -LiteralPath $f -Algorithm SHA256).Hash)"
}
$scratch = Join-Path $env:RUNNER_TEMP ('sig4a-desktop-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $scratch | Out-Null
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class ProofWin {
 [StructLayout(LayoutKind.Sequential)] public struct Point {
   public int X,Y;
   public Point(int x,int y) { X=x;Y=y; }
 }
 [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern IntPtr FindWindow(string cls, string title);
 public static IntPtr Find(string cls) { return FindWindow(cls, null); }
 [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(Point p);
 [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr window, uint flags);
 [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr window, ref Point p);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr window);
 [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint msg,UIntPtr w,IntPtr l);
 [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h,IntPtr after,int x,int y,int w,int height,uint flags);

 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd,out uint process);
 [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left,Top,Right,Bottom; }
 [StructLayout(LayoutKind.Sequential)] public struct MonitorInfo {
   public int Size; public Rect Monitor,Work; public int Flags;
 }
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd,out Rect result);
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hwnd);
 [DllImport("user32.dll")] public static extern IntPtr MonitorFromPoint(Point p,uint flags);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern bool GetMonitorInfo(IntPtr h,ref MonitorInfo info);
 public static Rect WorkAt(int x,int y) {
   var info=new MonitorInfo(); info.Size=Marshal.SizeOf(typeof(MonitorInfo));
   if(!GetMonitorInfo(MonitorFromPoint(new Point(x,y),2),ref info)) throw new Exception("GetMonitorInfo failed");
   return info.Work;
 }
 public static int VisibleCount(string a,string b) {
   int count=0;
   EnumWindows((hwnd,state) => {
     if(!IsWindowVisible(hwnd)) return true;
     var cls=new System.Text.StringBuilder(256); GetClassName(hwnd,cls,cls.Capacity);
     if(cls.ToString()==a || cls.ToString()==b) count++;
     return true;
   },IntPtr.Zero);
   return count;
 }
 public sealed class VisibilityWatch : IDisposable {
   System.Threading.Thread thread; volatile bool stop, seen; int samples;
   public bool Seen { get { return seen; } } public int Samples { get { return samples; } }
   public VisibilityWatch(string a,string b) {
     thread=new System.Threading.Thread(() => {
       while(!stop) {
         if(VisibleCount(a,b)>0) seen=true;
         System.Threading.Interlocked.Increment(ref samples);
         System.Threading.Thread.Sleep(8);
       }
     });
     thread.IsBackground=true; thread.Start();
   }
   public void Dispose(){stop=true;thread.Join();}
 }
 public delegate bool EnumFn(IntPtr hwnd,IntPtr state);
 [DllImport("user32.dll")] public static extern bool EnumWindows(EnumFn callback,IntPtr state);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr hwnd,System.Text.StringBuilder text,int max);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr hwnd,System.Text.StringBuilder text,int max);
 public static IntPtr FindOwned(uint owner,string expectedClass) {
   IntPtr result=IntPtr.Zero;
   EnumWindows((hwnd,state) => {
     uint found;
     GetWindowThreadProcessId(hwnd,out found);
     if (found!=owner || !IsWindowVisible(hwnd)) return true;
     var cls=new System.Text.StringBuilder(256);
     GetClassName(hwnd,cls,cls.Capacity);
     if (cls.ToString()!=expectedClass) return true;
     result=hwnd;
     return false;
   },IntPtr.Zero);
   return result;
 }
 public static string[] WindowsFor(uint owner) {
   var result=new System.Collections.Generic.List<string>();
   EnumWindows((hwnd,state) => {
     uint found;
     GetWindowThreadProcessId(hwnd,out found);
     if (found==owner) {
       var cls=new System.Text.StringBuilder(256);
       var title=new System.Text.StringBuilder(256);
       GetClassName(hwnd,cls,cls.Capacity);
       GetWindowText(hwnd,title,title.Capacity);
       result.Add(hwnd.ToString()+" | "+cls+" | "+title+" | visible="+IsWindowVisible(hwnd));
     }
     return true;
   },IntPtr.Zero);
   return result.ToArray();
 }
 [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
 [DllImport("user32.dll")] public static extern void keybd_event(byte key,byte scan,uint flags,IntPtr extra);
 [DllImport("user32.dll")] public static extern void mouse_event(uint flags,int dx,int dy,uint data,IntPtr extra);
}
'@
[void][ProofWin]::SetProcessDPIAware()
$pickerClass = 'KaneRegion'
$popupClass = 'ClaudeUsageQuickPopup'
$dashClass = 'ClaudeUsageDashboard'
$trayClass = 'ClaudeUsageTray'
$focusClasses = @($popupClass,$dashClass)
function Assert-Proof($ok,[string]$label) { if (!$ok) { throw $label } }
function Wait-Proof([string]$label,[scriptblock]$check,[int]$seconds=30) {
  $watch = [Diagnostics.Stopwatch]::StartNew()
  while ($watch.Elapsed.TotalSeconds -lt $seconds) {
    $result = & $check
    if ($result) { return $result }
    Start-Sleep -Milliseconds 80
  }
  throw "Timed out waiting for $label"
}
function Visible([string]$cls) {
  $hwnd = [ProofWin]::Find($cls)
  if ($hwnd -ne [IntPtr]::Zero -and [ProofWin]::IsWindowVisible($hwnd)) { return $hwnd }
}
function Owner([IntPtr]$hwnd) {
  [uint32]$ownerId = 0
  [void][ProofWin]::GetWindowThreadProcessId($hwnd,[ref]$ownerId)
  return $ownerId
}
function Root-At([int]$x,[int]$y) {
  return [ProofWin]::GetAncestor([ProofWin]::WindowFromPoint([ProofWin+Point]::new($x,$y)),2)
}
function Mouse-At([int]$x,[int]$y) {
  if (-not [ProofWin]::SetCursorPos($x,$y)) { throw "Unable to move cursor to $x,$y" }
}
function Send-Keys([int[]]$keys) {
  foreach($key in $keys){[ProofWin]::keybd_event($key,0,0,[IntPtr]::Zero)}
  for($k=$keys.Length-1;$k -ge 0;$k--){[ProofWin]::keybd_event($keys[$k],0,2,[IntPtr]::Zero)}
}
function Foreground([IntPtr]$hwnd) {
  [void][ProofWin]::SetForegroundWindow($hwnd)
  [void](Wait-Proof "foreground $hwnd" { [ProofWin]::GetForegroundWindow() -eq $hwnd } 5)
}
function Button-Click($target,[IntPtr]$hwnd) {
  Assert-Proof ($target.source -ceq 'uia-screen') 'Unexpected native screen coordinate source'
  Assert-Proof ((Root-At $target.x $target.y) -eq $hwnd) "Physical click ($($target.x),$($target.y)) does not hit expected popup"
  Mouse-At $target.x $target.y
  [ProofWin]::mouse_event(2,0,0,0,[IntPtr]::Zero)
  [ProofWin]::mouse_event(4,0,0,0,[IntPtr]::Zero)
}
function Uia([IntPtr]$h,[string]$name,[string]$id='',[string]$kind='',[string]$mode='point'){
 if($h -eq [IntPtr]::Zero){return @{found=$false}}
 try {
  $args=@('-NoProfile','-STA','-ExecutionPolicy','Bypass','-File',"$PSScriptRoot/desktop-uia.ps1",'-Hwnd',"$($h.ToInt64())",'-Mode',$mode)
  if($name){$args+=@('-Name',$name)}
  if($id){$args+=@('-Id',$id)}
  if($kind){$args+=@('-Kind',$kind)}
  $result=& powershell.exe @args
  return $result|ConvertFrom-Json
 }catch{return @{found=$false}}
}
function Find-Control([IntPtr]$h,[string]$name,[string]$id='',[string]$kind='',[string]$mode='point',[int]$seconds=16){
 return Wait-Proof "UIA $name $id $mode" {$n=Uia $h $name $id $kind $mode;if($n.found -and ($mode -ne 'focus' -or ($n.enabled -and $n.focused))){$n}} $seconds
}
function Show-Point([IntPtr]$h,[string]$name,[string]$id='',[string]$kind='Button'){
  $n=Uia $h $name $id $kind 'point'
  if($n.found){return $n}
  # Ask Windows accessibility to bring this exact control into view, then
  # re-resolve an actual screen location instead of relying on UIA focus reporting.
  [void](Uia $h $name $id $kind 'focus')
  return Find-Control $h $name $id $kind 'point' 12
}
function Click-Control([IntPtr]$h,[string]$name,[string]$id='',[string]$kind='Button'){
 $node=Show-Point $h $name $id $kind
 $node|Add-Member -NotePropertyName source -NotePropertyValue 'uia-screen'
 Button-Click $node $h
}
function Shell-Node([string]$pattern){
 foreach($cls in @('Shell_TrayWnd','Shell_SecondaryTrayWnd','NotifyIconOverflowWindow','TopLevelWindowForOverflowXamlIsland')){
  $shell=[ProofWin]::Find($cls)
  if($shell -eq [IntPtr]::Zero){continue}
  $n=Uia $shell $pattern
  if($n.found){
   $owner=Owner (Root-At $n.x $n.y)
   if($owner -and (Get-Process -Id $owner).ProcessName -ceq 'explorer'){return $n}
  }
 }
}
function Shell-Click($n,[bool]$right=$false){
 Assert-Proof (!!$n) 'Windows task notification icon was not available'
 Mouse-At $n.x $n.y
 $down=if($right){8}else{2};$up=if($right){16}else{4}
 [ProofWin]::mouse_event($down,0,0,0,[IntPtr]::Zero)
 [ProofWin]::mouse_event($up,0,0,0,[IntPtr]::Zero)
}
function Show-From-Tray {
 $n=Shell-Node "^(Claude usage|Kane.s Tool Box)"
 if(!$n){
  $overflow=Shell-Node '(?i)(show hidden icons|hidden icons|notification overflow|chevron)'
  if($overflow){Shell-Click $overflow;Start-Sleep -Milliseconds 350;$n=Shell-Node "^(Claude usage|Kane.s Tool Box)"}
 }
 if($n){
  Shell-Click $n
  return 'physical-explorer-icon'
 }
 # Some hosted images hide all notification icons. Open the app's real native menu
 # via its own notification callback, then choose the default through a physical key.
 # This remains a native tray handler/menu test, not a physical Explorer icon click.
 Mouse-At 360 330
 [void][ProofWin]::PostMessage($script:trayHwnd,0x800A,[UIntPtr]::Zero,[IntPtr]0x205)
 $menu=Wait-Proof 'owned Win32 tray menu' { $m=[ProofWin]::Find('#32768'); if($m -ne [IntPtr]::Zero -and (Owner $m) -eq $script:tray.Id){$m} } 8
 Send-Keys @(40)
 Send-Keys @(13)
 return 'native-tray-menu-keyboard'
}
function Isolated-Environment([string]$case){
 $base=Join-Path $scratch $case
 New-Item -ItemType Directory -Force -Path $base|Out-Null
 $env:LOCALAPPDATA=Join-Path $base 'local';$env:USERPROFILE=Join-Path $base 'home';$env:APPDATA=Join-Path $base 'roaming'
 $env:TEMP=Join-Path $base 'temp';$env:TMP=$env:TEMP
 $env:CU_NO_AUTOSTART='1';$env:CU_NO_REPAIR='1'
 Remove-Item Env:HOMEDRIVE,Env:HOMEPATH,Env:CLAUDE_CONFIG_DIR,Env:CU_E2E_VISIBLE,Env:CU_NO_DASHBOARD,Env:CU_NO_HOTKEYS -ErrorAction SilentlyContinue
 $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--force-renderer-accessibility'
 $d=Join-Path $env:LOCALAPPDATA 'claude-usage'
 New-Item -ItemType Directory -Force -Path $d,$env:USERPROFILE,$env:APPDATA,$env:TEMP|Out-Null
 return $d
}
function Start-Tray {
 foreach($c in @($trayClass,$dashClass,$popupClass,$pickerClass)){
  Assert-Proof ([ProofWin]::Find($c) -eq [IntPtr]::Zero) "Unrelated or unclosed $c; abort, do not close it"
 }
 $script:tray=Start-Process $realTray -ArgumentList '--hidden' -PassThru
 $script:trayHwnd=Wait-Proof 'owned ordinary release tray' {
  $h=[ProofWin]::Find($trayClass)
  if($h -ne [IntPtr]::Zero -and (Owner $h) -eq $script:tray.Id){$h}
 } 30
}
function Stop-OwnedTray {
 if($script:tray -and !$script:tray.HasExited){
  Assert-Proof ((Owner $script:trayHwnd) -eq $script:tray.Id) 'Refusing to stop a non-fixture process'
  [void][ProofWin]::PostMessage($script:trayHwnd,0x111,[UIntPtr]6,[IntPtr]::Zero)
  [void](Wait-Proof 'close owned release tray' {$script:tray.HasExited} 12)
 }
 $script:tray=$null
}
function Open-Dashboard {
 $script:lastTray=Show-From-Tray
 $h=Wait-Proof 'visible ordinary release dashboard' { Visible $dashClass } 75
 $owner=Get-Process -Id (Owner $h)
 Assert-Proof ($owner.Path -ieq $realDashboard) 'Dashboard path was not the unmodified sibling release executable'
 Foreground $h
 return $h
}
function Close-Dashboard([IntPtr]$h){
 Foreground $h;Send-Keys @(27)
 [void](Wait-Proof 'release dashboard Escape closes owned HWND' { [ProofWin]::Find($dashClass) -eq [IntPtr]::Zero } 15)
}
function PhysicalClick([IntPtr]$window,[int]$x,[int]$y) {
 Assert-Proof ((Root-At $x $y) -eq $window) "Unexpected physical input target at ($x,$y)"
 Mouse-At $x $y
 [ProofWin]::mouse_event(2,0,0,0,[IntPtr]::Zero)
 [ProofWin]::mouse_event(4,0,0,0,[IntPtr]::Zero)
}
function Open-Ocr {
 # Physical click (not SetForegroundWindow) to guarantee a normal external foreground app.
 PhysicalClick $fh ($f.x+650) ($f.y+70)
 [void](Wait-Proof 'fixture has keyboard focus before capture' {[ProofWin]::GetForegroundWindow() -eq $fh})
 $oldText=$null
 if(Test-Path $observed){Remove-Item -LiteralPath $observed}
 $x=[int]$f.x+10;$y=[int]$f.y+15
 Send-Keys @(17,18,79)
 $picker=Wait-Proof 'actual global shortcut -> region selector' {Visible $pickerClass}
 Assert-Proof ((Get-Process -Id (Owner $picker)).Path -ieq $realDashboard) 'Picker not owned by product'
 [void](Wait-Proof 'native picker owns physical drag endpoints' {(Root-At $x $y) -eq $picker -and (Root-At ($x+360) ($y+85)) -eq $picker})
 Mouse-At $x $y;[ProofWin]::mouse_event(2,0,0,0,[IntPtr]::Zero)
 try { Start-Sleep -Milliseconds 130;Mouse-At ($x+360) ($y+85);Start-Sleep -Milliseconds 130 }
 finally { [ProofWin]::mouse_event(4,0,0,0,[IntPtr]::Zero) }
 $h=Wait-Proof 'visible ordinary release OCR popup' {Visible $popupClass} 75
 Assert-Proof ((Get-Process -Id (Owner $h)).Path -ieq $realDashboard) 'Popup not owned by product'
 [void](Wait-Proof 'loopback double received a correctly sized PNG region' {if(Test-Path $observed){(Get-Content -Raw $observed) -match '^Fixture OCR [0-9a-f]{16}$'}})
 Confirm-Copy $h
 return $h
}
function Confirm-Copy([IntPtr]$h){
 Set-Clipboard -Value ('not OCR '+[guid]::NewGuid().ToString())
 Click-Control $h '' 'ocr-copy'
 [void](Wait-Proof 'OS clipboard matches synthetic result for physical crop' {try { (Get-Clipboard -Raw -ErrorAction Stop) -ceq (Get-Content -Raw $observed)} catch {$false}} 8)
}
function Outside([IntPtr]$popup){
 $r=[ProofWin+Rect]::new(); Assert-Proof ([ProofWin]::GetWindowRect($popup,[ref]$r)) 'Popup bounds unavailable'
 $rx=[int](($r.Left+$r.Right)/2);$ry=[int](($r.Top+$r.Bottom)/2)
 Assert-Proof ($rx -gt $f.x -and $rx -lt $f.x+$f.w -and $ry -gt $f.y -and $ry -lt $f.y+$f.h) 'Test fixture does not really overlap popup'
 Assert-Proof ((Root-At $rx $ry) -eq $popup) 'Sample point not exposed on popup before activating external window'
 PhysicalClick $fh ($f.x+650) ($f.y+70)
 [void](Wait-Proof 'external app actually became foreground via physical click' {[ProofWin]::GetForegroundWindow() -eq $fh} 8)
 return @{x=$rx;y=$ry}
}
function Warm-Cancel([IntPtr]$popup){
 $ownerId=Owner $popup
 Send-Keys @(17,18,79)
 $pk=Wait-Proof 'warm native picker' {Visible $pickerClass}
 Assert-Proof ((Owner $pk) -eq $ownerId -and !(Visible $popupClass)) 'Warm picker must not close/occlude itself or leave popup on screen'
 Foreground $pk;Send-Keys @(27)
 [void](Wait-Proof 'same visible popup after cancelling recapture' {(Visible $popupClass) -eq $popup -and (Owner $popup) -eq $ownerId -and !(Visible $pickerClass)})
 Confirm-Copy $popup
}
function Verify-Config([IntPtr]$h,[string]$expected){
 Click-Control $h '^Cài đặt$'
 Click-Control $h '^OCR Popup$' 't-ocrpopup'
 $setting=Find-Control $h '' 'ocr-popup-topmost' 'CheckBox' 'state'
 Assert-Proof ($setting.toggle -ceq $expected) "Expected '$expected' for OCR popup, got $($setting|ConvertTo-Json -Compress)"
}
function Capture-Evidence([string]$file,[int]$x,[int]$y,[int]$w,[int]$height){
 Add-Type -AssemblyName System.Drawing
 $img=[Drawing.Bitmap]::new($w,$height)
 $g=[Drawing.Graphics]::FromImage($img)
 try{$g.CopyFromScreen($x,$y,0,0,$img.Size);$img.Save((Join-Path $outdir $file),[Drawing.Imaging.ImageFormat]::Png)}
 finally{$g.Dispose();$img.Dispose()}
}
$script:tray=$fixture=$server=$null
$script:trayHwnd=[IntPtr]::Zero
try {
 $outdir=Join-Path $env:GITHUB_WORKSPACE 'evidence'
 New-Item -ItemType Directory -Path $outdir -Force|Out-Null
 $portFile=Join-Path $scratch 'port';$observed=Join-Path $scratch 'model-result'
 $python=(Get-Command python -ErrorAction Stop).Source
 $server=Start-Process $python -ArgumentList @('-I',"`"$PSScriptRoot/model.py`"","`"$portFile`"","`"$observed`"") -RedirectStandardError "$scratch/model.log" -PassThru
 [void](Wait-Proof 'local fixture listening' {Test-Path $portFile})
 $port=[int](Get-Content -Raw $portFile)
 $data=Isolated-Environment 'sticky'
 $cfg=Join-Path $data 'config.json'
 # Intentionally omit the new key: this tests the migration/default on an existing installation.
 @{profiles=@();autostart=$false;widget_enabled=$false;notify_enabled=$false;token_refresh=$false;local_ai_endpoint="http://127.0.0.1:$port";local_ai_model='sig4a-focus-fixture';local_ai_backend='llama_cpp';local_ai_managed=$false;old_extra_fixture='preserve me'}|
   ConvertTo-Json|Set-Content -Encoding utf8 $cfg
 $ready=Join-Path $scratch 'fixture.json'
 $fixture=Start-Process powershell.exe -ArgumentList @('-NoProfile','-STA','-ExecutionPolicy','Bypass','-File',"`"$PSScriptRoot/fixture.ps1`"",'-Ready',"`"$ready`"") -RedirectStandardError "$scratch/fixture.log" -PassThru
 [void](Wait-Proof 'visible synthetic regular (not topmost) window' {Test-Path $ready})
 $f=Get-Content -Raw $ready | ConvertFrom-Json; $fh=[IntPtr]$f.hwnd
 Assert-Proof ((Owner $fh) -eq $fixture.Id) 'Fixture PID/HWND mismatch'
 Start-Tray
 $pop=Open-Ocr
 $pidBefore=Owner $pop
 $sample=Outside $pop
 # IsWindowVisible by itself is insufficient: a covered ordinary window is still "visible".
 Start-Sleep -Milliseconds 700
 Assert-Proof ((Visible $popupClass) -eq $pop -and (Owner $pop) -eq $pidBefore -and [ProofWin]::GetForegroundWindow() -eq $fh -and (Root-At $sample.x $sample.y) -eq $pop) 'OCR popup was closed, covered, or stole keyboard focus after the outside click'
 Capture-Evidence 'sticky-outside.png' $f.x $f.y $f.w $f.h
 Confirm-Copy $pop
 Warm-Cancel $pop
 Foreground $pop;Send-Keys @(27)
 [void](Wait-Proof 'explicit Escape closes sticky popup' {!(Visible $popupClass) -and !(Get-Process -Id $pidBefore -ErrorAction SilentlyContinue)})
 Write-Host 'PASS default/absent key: physical external activation, overlapping HWND hit-test, text copy retained, warm recapture Escape restored same HWND and explicit Escape closed.'
 $dash=Open-Dashboard;Verify-Config $dash 'On'
 $dr=[ProofWin+Rect]::new(); Assert-Proof ([ProofWin]::GetWindowRect($dash,[ref]$dr)) 'Missing dashboard rectangle'
 Capture-Evidence 'config.png' $dr.Left $dr.Top ($dr.Right-$dr.Left) ($dr.Bottom-$dr.Top)
 Click-Control $dash '' 'ocr-popup-topmost' 'CheckBox'
 Click-Control $dash '^Lưu cài đặt$' 'settings-save'
 [void](Wait-Proof 'persisted disabled pinning from UI' {try {$j=Get-Content -Raw $cfg|ConvertFrom-Json;($j.ocr_popup_always_on_top -is [bool]) -and !$j.ocr_popup_always_on_top -and $j.old_extra_fixture -ceq 'preserve me' -and $j.local_ai_model -ceq 'sig4a-focus-fixture'}catch{$false}} 10)
 Close-Dashboard $dash;Stop-OwnedTray
 Start-Tray;$dash=Open-Dashboard;Verify-Config $dash 'Off'
 Close-Dashboard $dash
 $pop=Open-Ocr
 Warm-Cancel $pop
 $sample=Outside $pop
 [void](Wait-Proof 'disabled pinning restores ordinary z-order' {(Visible $popupClass) -eq $pop -and (Root-At $sample.x $sample.y) -eq $fh -and [ProofWin]::GetForegroundWindow() -eq $fh} 8)
 # Covered but not destroyed: user can trigger the shortcut and cancel to recover the old result.
 Warm-Cancel $pop
 Foreground $pop;Send-Keys @(27)
 [void](Wait-Proof 'explicit Escape closes unpinned popup' {!(Visible $popupClass)})
 Write-Host 'PASS opt-out: physical Settings > OCR Popup selection and save, after tray restart still Off. Normal external window covers the same live HWND; Ctrl+Alt+O and cancel restores its original copyable result.'
} catch {
 if(Test-Path "$scratch/model.log"){Get-Content "$scratch/model.log" -Tail 24|Write-Host}
 if($data -and (Test-Path "$data/events.log")){Get-Content "$data/events.log" -Tail 30|Write-Host}
 $d=Visible $dashClass;if($d){Uia $d '' '' '' 'list'|ConvertTo-Json -Depth 5 -Compress|Write-Host}
 throw
} finally {
 if($script:tray -and !$script:tray.HasExited){Stop-Process -Id $script:tray.Id -Force -ErrorAction SilentlyContinue}
 foreach($pr in @($fixture,$server)){if($pr -and !$pr.HasExited){Stop-Process -Id $pr.Id -Force -ErrorAction SilentlyContinue}}
}
