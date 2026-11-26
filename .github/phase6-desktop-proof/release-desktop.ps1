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
function Select-Nav([IntPtr]$h,[string]$name){
 $node=Find-Control $h $name '' 'Button' 'focus'
 Assert-Proof ($node.enabled -and $node.focused) "Native navigation could not focus $name"
 Send-Keys @(13)
}
function Click-Control([IntPtr]$h,[string]$name,[string]$id='',[string]$kind='Button'){
 $node=Find-Control $h $name $id $kind 'point' 4
 $node|Add-Member -NotePropertyName source -NotePropertyValue 'uia-screen'
 Button-Click $node $h
}
function Keyboard-Control([IntPtr]$h,[string]$name,[string]$id='',[string]$kind='Button',[int]$key=13){
 $node=Find-Control $h $name $id $kind 'focus'
 Assert-Proof ($node.enabled -and $node.focused) "Native focus not confirmed for $name $id"
 Send-Keys @($key)
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
function Capture-Dashboard([IntPtr]$h,[string]$label){
 $r=[ProofWin+Rect]::new()
 Assert-Proof ([ProofWin]::GetWindowRect($h,[ref]$r)) 'No real dashboard bounds for screenshot'
 Foreground $h
 $midX=[int](($r.Left+$r.Right)/2);$midY=[int](($r.Top+$r.Bottom)/2)
 Assert-Proof ((Root-At $midX $midY) -eq $h) 'Fixture screenshot centre covered by an unrelated window'
 $bmp=[Drawing.Bitmap]::new($r.Right-$r.Left,$r.Bottom-$r.Top)
 $g=[Drawing.Graphics]::FromImage($bmp)
 $m=[IO.MemoryStream]::new()
 try{
   $g.CopyFromScreen($r.Left,$r.Top,0,0,[Drawing.Size]::new($bmp.Width,$bmp.Height))
   $bmp.Save($m,[Drawing.Imaging.ImageFormat]::Png)
   $b=[Convert]::ToBase64String($m.ToArray())
   Write-Host "P6-DESKTOP-IMAGE:$label"
   for($i=0;$i -lt $b.Length;$i+=800){Write-Host ("P6-DESKTOP-BYTES:"+$b.Substring($i,[Math]::Min(800,$b.Length-$i)))}
   Write-Host "P6-DESKTOP-END:$label"
 }finally{$m.Dispose();$g.Dispose();$bmp.Dispose()}
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
function Snapshot-Files([string]$dir){
 return ((Get-ChildItem -LiteralPath $dir -Recurse -File -Force|Sort-Object FullName|ForEach-Object {"$($_.FullName):$((Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash)"}) -join "`n")
}
$script:tray=$null
$script:trayHwnd=[IntPtr]::Zero
try {
 Add-Type -AssemblyName System.Drawing
 $data=Isolated-Environment 'fresh'
 $cfg=Join-Path $data 'config.json'
 Assert-Proof (!(Test-Path -LiteralPath $cfg)) 'Fresh fixture already has config'
 Start-Tray
 $h=Open-Dashboard
 $fromFirst=$script:lastTray
 $owner=Owner $h
 $n=Open-Dashboard
 $fromRepeat=$script:lastTray
 Assert-Proof ($n -eq $h -and (Owner $n) -eq $owner -and [ProofWin]::VisibleCount($dashClass,$popupClass) -eq 1) 'Repeated opening produced another owned dashboard'
 Capture-Dashboard $h 'normal-fresh'
 $accent=Find-Control $h '^Màu giao diện$' 'accent' 'ComboBox' 'state'
 Assert-Proof ($accent.value -match 'Cam|orange' -or ($accent.selection -join ' ') -match 'Cam|orange') 'Fresh accent is not orange'
 Select-Nav $h '^Dọn ổ$'
 [void](Find-Control $h 'Chưa chọn thư mục' '' '' 'state')
 $scan=Find-Control $h 'Quét artefact' 'cscan' 'Button' 'state'
 Assert-Proof (!$scan.enabled) 'Fresh cleaner allowed scanning without an explicit root'
 $accent=Find-Control $h '^Màu giao diện$' 'accent' 'ComboBox' 'focus'
 Assert-Proof $accent.focused 'Could not focus accent menu'
 Send-Keys @(18,40);Send-Keys @(35);Send-Keys @(13)
 [void](Wait-Proof 'fresh purple selection persisted to the isolated file' { (Test-Path $cfg) -and (Get-Content -Raw $cfg|ConvertFrom-Json).dashboard_accent -eq 'purple' } 8)
 Close-Dashboard $h;Stop-OwnedTray
 Start-Tray
 $h=Open-Dashboard
 $fromRestart=$script:lastTray
 $acc=Find-Control $h '^Màu giao diện$' 'accent' 'ComboBox' 'state'
 Assert-Proof ($acc.value -match 'Tím|purple' -or ($acc.selection -join ' ') -match 'Tím|purple') 'Accent did not survive release restart'
 Close-Dashboard $h;Stop-OwnedTray
 Write-Host "P6-FRESH-RELEASE: first=$fromFirst repeat=$fromRepeat restart=$fromRestart; same HWND/PID on repeat open; no implicit Cleaner scope; fresh orange and persisted purple after tray restart."

 $data=Isolated-Environment 'legacy'
 $base=Split-Path (Split-Path $data -Parent) -Parent
 $fixtureRoot=Join-Path $base 'fixture-cleaner'
 $outside=Join-Path $base 'outside-cleaner'
 foreach($d in @("$fixtureRoot/work/node_modules/pkg","$fixtureRoot/work/bin/Release","$fixtureRoot/work/source","$fixtureRoot/dups",$outside)){New-Item -ItemType Directory -Force -Path $d|Out-Null}
 $fixtures=@{'work/package.json'='{"name":"legacy-release-test"}';'work/work.csproj'='<Project Sdk="Microsoft.NET.Sdk"/>'; 'work/node_modules/pkg/module.js'='module';'work/bin/Release/fake.dll'='temporary artifact';'work/source/keep.txt'='source';'dups/copy-one.txt'='exact text for the paired fixture files';'dups/copy-two.txt'='exact text for the paired fixture files'}
 foreach($k in $fixtures.Keys){Set-Content -NoNewline -LiteralPath (Join-Path $fixtureRoot $k) -Value $fixtures[$k]}
 Set-Content -NoNewline -LiteralPath (Join-Path $outside 'outside.txt') -Value 'outside scoped root'
 $before=Snapshot-Files $fixtureRoot;$beforeOutside=Snapshot-Files $outside
 $settingsHashes=@{};$profiles=@();$cache=[ordered]@{};$now=[DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
 foreach($name in @('routine-fixture','near-quota-fixture','stale-fixture')){
   $profile=Join-Path $base $name;New-Item -ItemType Directory -Force -Path $profile|Out-Null
   $profiles+=@{name=$name;path=$profile}
   $settings=Join-Path $profile 'settings.json'
   @{legacy_marker=$name;hooks=@{UserPromptSubmit=@(@{hooks=@(@{type='command';command='other-synthetic-tool.exe'})})}} | ConvertTo-Json -Depth 9 | Set-Content -Encoding utf8 $settings
   $settingsHashes[$settings]=(Get-FileHash $settings -Algorithm SHA256).Hash
   $cache[$name]=@{path=$profile;five=24;week=31;five_reset=[DateTimeOffset]::UtcNow.AddHours(1).ToString('o');week_reset=[DateTimeOffset]::UtcNow.AddDays(3).ToString('o');fetched_at=$now;retry_at=$now+180;err=$null}
 }
 $cache['near-quota-fixture'].five=88;$cache['stale-fixture'].err='HTTP 429';$cache['stale-fixture'].fetched_at=$now-700
 @{profiles=$profiles;autostart=$false;interval_sec=180;widget_enabled=$false;notify_enabled=$false;widget_label='short';widget_font=125;widget_opacity=84;widget_locked=$true;dash_rect=@(35,35,800,630);dashboard_accent='blue';unknown_legacy='retained';cleaner_skip_recent_days=0;cleaner_dups_min_kb=0;cleaner_roots=@($fixtureRoot)}|ConvertTo-Json -Depth 8|Set-Content -Encoding utf8 (Join-Path $data 'config.json')
 @{ts=$now;block_enabled=$true;warn_pct=80;block_pct=95;busy=@();profiles=$cache}|ConvertTo-Json -Depth 8|Set-Content -Encoding utf8 (Join-Path $data 'usage_cache.json')
 Start-Tray
 $h=Open-Dashboard
 $legacyTrayPath=$script:lastTray
 $legacyAccent=Find-Control $h '^Màu giao diện$' 'accent' 'ComboBox' 'state'
 Assert-Proof ($legacyAccent.value -match 'Xanh|blue' -or ($legacyAccent.selection -join ' ') -match 'Xanh|blue') 'Legacy blue accent did not render'
 Select-Nav $h '^Cài đặt$'
 Keyboard-Control $h '^Widget$' 't-look'
 $font=Find-Control $h '^Cỡ chữ' 'wfont' '' 'state'
 $opac=Find-Control $h '^Độ mờ' 'wopac' '' 'state'
 Assert-Proof (($font.value -eq '125' -or $font.range -eq 125) -and ($opac.value -eq '84' -or $opac.range -eq 84)) 'Legacy Widget preferences did not render'
 $actualDpi=[ProofWin]::GetDpiForWindow($h)
 $width=[int][Math]::Round(420*$actualDpi/96.0)
 $area=[ProofWin]::WorkAt(100,100)
 Assert-Proof ([ProofWin]::SetWindowPos($h,[IntPtr]::Zero,$area.Left+20,$area.Top+20,$width,[Math]::Min(670,$area.Bottom-$area.Top-45),0x0014)) 'Cannot set native narrow width'
 $actual=[ProofWin+Rect]::new()
 [void](Wait-Proof 'logical 420px release dashboard' {[void][ProofWin]::GetWindowRect($h,[ref]$actual);[Math]::Abs(($actual.Right-$actual.Left)-$width) -le 3} 8)
 Select-Nav $h '^Claude Usage$'
 [void](Find-Control $h 'near-quota-fixture' '' '' 'state')
 [void](Find-Control $h 'stale-fixture' '' '' 'state')
 [void](Find-Control $h '^(5h|5h )' '' '' 'state')
 [void](Find-Control $h '^(7d|7d )' '' '' 'state')
 Capture-Dashboard $h 'narrow-usage'
 Select-Nav $h '^Dọn ổ$'
 [void](Find-Control $h 'fixture-cleaner' '' '' 'state')
 Keyboard-Control $h '^Quét artefact$' 'cscan'
 $check=Find-Control $h 'Chọn .*node_modules' '' 'CheckBox' 'focus'
 Assert-Proof ($check.enabled -and $check.focused) 'Cannot focus isolated artifact at narrow width'
 Send-Keys @(32)
 Keyboard-Control $h '^Đưa vào Thùng rác' 'crecyc'
 [void](Find-Control $h '^Xác nhận đưa vào Thùng rác' '' '' 'state')
 Keyboard-Control $h '^Huỷ xác nhận' 'clean-cancel'
 Keyboard-Control $h '^Xoá vĩnh viễn' 'cperm'
 [void](Find-Control $h '^Xác nhận xoá vĩnh viễn' '' '' 'state')
 Keyboard-Control $h '^Huỷ xác nhận' 'clean-cancel'
 Keyboard-Control $h 'File trùng.*chỉ đọc' 'clean-view-dups'
 Keyboard-Control $h '^Tìm file trùng' 'dscan'
 [void](Find-Control $h 'copy-one\.txt' '' '' 'state')
 [void](Find-Control $h 'copy-two\.txt' '' '' 'state')
 Assert-Proof (!(Uia $h '^Xoá vĩnh viễn|^Đưa vào Thùng rác' '' 'Button' 'point').found) 'Duplicate view exposed visible removal'
 Capture-Dashboard $h 'narrow-duplicates'
 Assert-Proof ((Snapshot-Files $fixtureRoot) -ceq $before -and (Snapshot-Files $outside) -ceq $beforeOutside) 'Release Cleaner fixtures changed despite no confirm'
 $legacyConfig=Join-Path $data 'config.json'
 $accent=Find-Control $h '^Màu giao diện$' 'accent' 'ComboBox' 'focus'
 Send-Keys @(18,40);Send-Keys @(35);Send-Keys @(13)
 [void](Wait-Proof 'legacy unrelated accent save' {(Get-Content -Raw $legacyConfig|ConvertFrom-Json).dashboard_accent -eq 'purple'} 8)
 Close-Dashboard $h
 $h=Open-Dashboard
 $again=Find-Control $h '^Màu giao diện$' 'accent' 'ComboBox' 'state'
 Assert-Proof ($again.value -match 'Tím|purple' -or ($again.selection -join ' ') -match 'Tím|purple') 'Legacy accent did not reopen after unrelated save'
 Close-Dashboard $h;Stop-OwnedTray
 $legacy=Get-Content -Raw $legacyConfig|ConvertFrom-Json
 Assert-Proof ($legacy.dashboard_accent -eq 'purple' -and $legacy.widget_font -eq 125 -and $legacy.widget_opacity -eq 84 -and $legacy.widget_locked -and $legacy.unknown_legacy -eq 'retained') 'Legacy widget/settings did not survive'
 foreach($p in $settingsHashes.Keys){Assert-Proof ((Get-FileHash $p -Algorithm SHA256).Hash -ceq $settingsHashes[$p]) 'Legacy hook settings fixture changed'}
 Write-Host "P6-LEGACY-RELEASE: tray=$legacyTrayPath; ordinary release DPI $actualDpi at logical 420px; real keyboard Usage/reset windows and Cleaner cancelled, separate read-only duplicate report, all scoped files SHA-256 preserved; legacy settings unchanged."
} catch {
  if($data -and (Test-Path "$data/events.log")){Get-Content "$data/events.log" -Tail 40|Write-Host}
  if($script:tray){[ProofWin]::WindowsFor($script:tray.Id)|Write-Host}
  throw
} finally {
 if($script:tray -and !$script:tray.HasExited){Stop-Process -Id $script:tray.Id -Force -ErrorAction SilentlyContinue}
}
