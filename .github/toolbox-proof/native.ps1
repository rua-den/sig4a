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
$scratch = Join-Path $env:RUNNER_TEMP ('sig4a-toolbox-' + [guid]::NewGuid().ToString('N'))
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
function Product-Window([string]$cls,[string]$label,[int]$seconds=75){
 $h=Wait-Proof $label {Visible $cls} $seconds
 Assert-Proof ((Get-Process -Id (Owner $h)).Path -ieq $realDashboard) "Unexpected executable for $label"
 return $h
}
function Enter-Text([IntPtr]$h,[string]$id,[string]$text){
 Click-Control $h '' $id 'Edit|Document'
 Send-Keys @(17,65)
 Set-Clipboard -Value $text
 Send-Keys @(17,86)
}
function Settings-Shortcuts([IntPtr]$dash){
 Click-Control $dash '^Cài đặt$'
 Click-Control $dash '' 't-shortcuts'
 [void](Find-Control $dash '' 'shortcut-toolbox' '' 'point')
}
function Request-Log {
 if(Test-Path $requests){Get-Content -LiteralPath $requests | Where-Object {$_} | ForEach-Object {try {ConvertFrom-Json $_}catch{}}}
}
function Require-Requests([string]$task,[string]$source,[int]$expected=1){
 $hits=@(Request-Log|Where-Object {$_.task -ceq $task -and $_.source -ceq $source})
 Assert-Proof ($hits.Count -eq $expected) "Expected $expected $task requests for '$source', observed $($hits.Count)"
}
function Prove-Copy([IntPtr]$h,[string]$button,[string]$expected,[string]$name=''){
 Set-Clipboard -Value ('not output '+[guid]::NewGuid().ToString())
 Click-Control $h $name $button
 [void](Wait-Proof "actual clipboard received '$expected'" {try{(Get-Clipboard -Raw -ErrorAction Stop) -ceq $expected}catch{$false}} 10)
}
function Exposed-Point([IntPtr]$h,[int]$left,[int]$top,[int]$right,[int]$bottom){
 if($right -le $left -or $bottom -le $top){return}
 # Try the center, then sample near all edges without allowing a hit outside the verified HWND.
 foreach($fx in @(0.50,0.04,0.96,0.20,0.80,0.35,0.65)){
  foreach($fy in @(0.50,0.04,0.96,0.20,0.80,0.35,0.65)){
   $xx=[int]($left+($right-$left-1)*$fx);$yy=[int]($top+($bottom-$top-1)*$fy)
   if((Root-At $xx $yy) -eq $h){return @{x=$xx;y=$yy}}
  }
 }
}
function External-Point([string]$label){
 return Wait-Proof $label {
  Exposed-Point $fixtureH $f.x $f.y ($f.x+$f.w) ($f.y+$f.h)
 } 8
}
function Focus-Fixture([bool]$selection=$false){
 # The dashboard may cover the editor. Activate the normal WPF window first
 # through an exposed pixel proven to belong to it, then physically click its editor.
 $available=External-Point 'exposed fixture pixel under current window stack'
 PhysicalClick $fixtureH $available.x $available.y
 [void](Wait-Proof 'synthetic WPF editor foreground' {[ProofWin]::GetForegroundWindow() -eq $fixtureH} 8)
 Click-Control $fixtureH '' 'sig4a-synthetic-edit' 'Edit'
 if($selection){Send-Keys @(17,65)}else{Send-Keys @(39)}
 $selected=Find-Control $fixtureH '' 'sig4a-synthetic-edit' 'Edit' 'state'
 if($selection){
   Assert-Proof (($selected.selectedText -join '') -ceq 'FixtureInputAlpha') "Physical keyboard selection not visible to external UIA: $($selected|ConvertTo-Json -Compress)"
 } else {
   Assert-Proof (!(($selected.selectedText -join '').Trim())) 'No-selection fixture still has selected text'
 }
}
function Close-Quick([IntPtr]$h,[string]$cls){
 Foreground $h;Send-Keys @(27)
 [void](Wait-Proof "Escape closes $cls" {!(Visible $cls)} 12)
}
function Open-Shortcut([int[]]$keys){
 Send-Keys $keys
 $h=Product-Window $dashClass 'global shortcut opens Tool Box'
 [void](Wait-Proof 'Tool Box active after global shortcut' {[ProofWin]::GetForegroundWindow() -eq $h} 8)
 return $h
}
function Capture-Crop {
 Focus-Fixture
 $x=[int]$f.x+10;$y=[int]$f.y+15
 Assert-Proof ((Root-At $x $y) -eq $fixtureH -and (Root-At ($x+360) ($y+85)) -eq $fixtureH) 'Physical screenshot endpoints do not belong to the foreground synthetic fixture'
 Send-Keys @(17,18,79)
 $pk=Product-Window $pickerClass 'native global OCR capture picker'
 [void](Wait-Proof 'selector owns both physical drag endpoints' {(Root-At $x $y) -eq $pk -and (Root-At ($x+360) ($y+85)) -eq $pk} 12)
 Mouse-At $x $y;[ProofWin]::mouse_event(2,0,0,0,[IntPtr]::Zero)
 try{Start-Sleep -Milliseconds 125;Mouse-At ($x+360) ($y+85);Start-Sleep -Milliseconds 125}
 finally{[ProofWin]::mouse_event(4,0,0,0,[IntPtr]::Zero)}
 $pop=Product-Window $popupClass 'real anchored OCR popup'
 Assert-Proof (!(Visible $pickerClass) -and !(Visible $dashClass)) 'OCR unexpectedly opened another surface'
 $rect=[ProofWin+Rect]::new()
 Assert-Proof ([ProofWin]::GetWindowRect($pop,[ref]$rect)) 'Cannot read OCR screen rectangle'
 $gap=[int][Math]::Round(8*[ProofWin]::GetDpiForWindow($pop)/96.0)
 Assert-Proof ([Math]::Abs($rect.Top-($y+85)-$gap) -le 4) 'OCR popup not physically anchored below selected crop'
 return $pop
}
function Assert-Float([IntPtr]$h){
 $r=[ProofWin+Rect]::new();Assert-Proof ([ProofWin]::GetWindowRect($h,[ref]$r)) 'No grammar popup bounds'
 $left=[Math]::Max($r.Left,$f.x);$top=[Math]::Max($r.Top,$f.y)
 $right=[Math]::Min($r.Right,($f.x+$f.w));$bottom=[Math]::Min($r.Bottom,($f.y+$f.h))
 $sharedPoint=Exposed-Point $h $left $top $right $bottom
 Assert-Proof (!!$sharedPoint) 'Normal app and Grammar popup do not share a currently exposed physical sample'
 $outside=External-Point 'visible normal app pixel not covered by grammar popup'
 Assert-Proof ($outside.x -lt $r.Left -or $outside.x -ge $r.Right -or $outside.y -lt $r.Top -or $outside.y -ge $r.Bottom) 'Selected external click was not outside Grammar rectangle'
 PhysicalClick $fixtureH $outside.x $outside.y
 [void](Wait-Proof 'external app gained focus after real outside click' {[ProofWin]::GetForegroundWindow() -eq $fixtureH} 8)
 Start-Sleep -Milliseconds 700
 Assert-Proof ([ProofWin]::GetForegroundWindow() -eq $fixtureH -and (Root-At $sharedPoint.x $sharedPoint.y) -eq $h) 'Grammar popup vanished or reclaimed external focus'
}
function Require-Focused-UIA([IntPtr]$h,[string]$id) {
 [void](Find-Control $h '' $id '' 'state')
 Assert-Proof ([ProofWin]::GetForegroundWindow() -eq $h) "$id exists but is not in foreground"
}
function Capture-Evidence([IntPtr]$hwnd,[string]$name){
 Add-Type -AssemblyName System.Drawing
 $r=[ProofWin+Rect]::new();Assert-Proof ([ProofWin]::GetWindowRect($hwnd,[ref]$r)) "Missing rectangle for $name"
 $img=[Drawing.Bitmap]::new($r.Right-$r.Left,$r.Bottom-$r.Top)
 $g=[Drawing.Graphics]::FromImage($img)
 try{$g.CopyFromScreen($r.Left,$r.Top,0,0,$img.Size);$img.Save((Join-Path $evidence $name),[Drawing.Imaging.ImageFormat]::Png)}
 finally{$g.Dispose();$img.Dispose()}
}
$script:tray=$fixture=$server=$null
$script:trayHwnd=[IntPtr]::Zero
try {
 $evidence=Join-Path $env:GITHUB_WORKSPACE 'evidence'
 New-Item -ItemType Directory -Path $evidence -Force|Out-Null
 $portFile=Join-Path $scratch 'port';$requests=Join-Path $scratch 'requests.ndjson'
 $python=(Get-Command python -ErrorAction Stop).Source
 $server=Start-Process $python -ArgumentList @('-I',"`"$PSScriptRoot/model.py`"","`"$portFile`"","`"$requests`"") -RedirectStandardError "$scratch/model.log" -PassThru
 [void](Wait-Proof 'loopback model fixture socket' {Test-Path $portFile})
 $port=[int](Get-Content -Raw $portFile)
 $data=Isolated-Environment 'toolbox'
 $cfg=Join-Path $data 'config.json'
 @{profiles=@();autostart=$false;widget_enabled=$false;notify_enabled=$false;token_refresh=$false;local_ai_endpoint="http://127.0.0.1:$port";local_ai_model='sig4a-toolbox-fixture';local_ai_backend='llama_cpp';local_ai_managed=$false;old_extra_fixture='preserve me'}|
   ConvertTo-Json|Set-Content -Encoding utf8 $cfg
 $ready=Join-Path $scratch 'fixture.json'
 $fixture=Start-Process powershell.exe -ArgumentList @('-NoProfile','-STA','-ExecutionPolicy','Bypass','-File',"`"$PSScriptRoot/fixture.ps1`"",'-Ready',"`"$ready`"") -RedirectStandardError "$scratch/fixture.log" -PassThru
 [void](Wait-Proof 'synthetic WPF input window and actual registered conflicting shortcut' {Test-Path $ready})
 $f=Get-Content -Raw $ready|ConvertFrom-Json;$fixtureH=[IntPtr]$f.hwnd
 Assert-Proof ((Owner $fixtureH) -eq $fixture.Id -and $f.reserved -ceq 'Ctrl+Shift+J') 'Synthetic fixture failed to prove its identity or chord'
 Start-Tray
 . "$PSScriptRoot/scenarios.ps1"
} catch {
 foreach($file in @("$scratch/model.log","$scratch/fixture.log","$requests","$data/events.log")){
  if($file -and (Test-Path -LiteralPath $file)){Write-Host "Tail $file";Get-Content -LiteralPath $file -Tail 30|Write-Host}
 }
 foreach($c in @($dashClass,$popupClass)){
  $h=Visible $c;if($h){Uia $h '' '' '' 'list'|ConvertTo-Json -Depth 5 -Compress|Write-Host}
 }
 throw
} finally {
 if($script:tray -and !$script:tray.HasExited){Stop-Process -Id $script:tray.Id -Force -ErrorAction SilentlyContinue}
 foreach($pr in @($fixture,$server)){if($pr -and !$pr.HasExited){Stop-Process -Id $pr.Id -Force -ErrorAction SilentlyContinue}}
}
