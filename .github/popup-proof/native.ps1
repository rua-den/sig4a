param(
  [Parameter(Mandatory=$true)][string]$ProductRoot,
  [Parameter(Mandatory=$true)][string]$ExpectedSha
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$root = (Resolve-Path $ProductRoot).Path
$revision = (git -C $root rev-parse HEAD).Trim()
if ($revision -cne $ExpectedSha) { throw "Expected $ExpectedSha but checked out $revision" }
Write-Host "Native product revision: $revision"
$scratch = Join-Path $env:RUNNER_TEMP ('sig4a-popup-' + [guid]::NewGuid().ToString('N'))
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
function Probe([string]$mode,[IntPtr]$hwnd) {
  for($attempt=0;$attempt -lt 5;$attempt++){
    $seconds = if($attempt -eq 0){45}else{5}
    $target = powershell.exe -NoProfile -STA -ExecutionPolicy Bypass -File "$PSScriptRoot/uia.ps1" -PopupHwnd ($hwnd.ToInt64()) -Mode $mode -TimeoutSeconds $seconds | ConvertFrom-Json
    if($target.found -and (Root-At $target.x $target.y) -eq $hwnd){return $target}
    $bounds=[ProofWin+Rect]::new()
    Assert-Proof ([ProofWin]::GetWindowRect($hwnd,[ref]$bounds)) "Missing popup while looking for $mode"
    # Scroll only inside our proven HWND, with a physical wheel event.
    $x=[int](($bounds.Left+$bounds.Right)/2);$y=[int](($bounds.Top+$bounds.Bottom)/2)
    Assert-Proof ((Root-At $x $y) -eq $hwnd) 'Cannot scroll popup at its physical center'
    Mouse-At $x $y
    $wheel=if($mode -eq 'translated-copy'){[uint32]4294967056}else{[uint32]240}
    [ProofWin]::mouse_event(0x0800,0,0,$wheel,[IntPtr]::Zero)
    Start-Sleep -Milliseconds 150
  }
  throw "Could not locate a physically clickable popup button: $mode"
}
function Compare-Clipboard($expected) {
  try { (Get-Clipboard -Raw -ErrorAction Stop) -ceq $expected } catch { $false }
}
$port=18080
$aliasName='qwen3-vl-2b'
$server=$fixture=$tray=$watcher=$null
try {
  $archive = Join-Path $scratch 'llama.zip'
  curl.exe -fLS --retry 3 -o $archive 'https://github.com/ggml-org/llama.cpp/releases/download/b12776/llama-b12776-bin-win-cpu-x64.zip'
  if ((Get-FileHash -Algorithm SHA256 $archive).Hash.ToLowerInvariant() -ne 'bba23437a597b7b346acb87dbac2e5452c7b188cec67ca6315bec513a067ace8') { throw 'Unexpected llama.cpp archive hash' }
  Expand-Archive $archive (Join-Path $scratch 'llama') -Force
  $exe = Get-ChildItem (Join-Path $scratch 'llama') -Filter llama-server.exe -Recurse | Select-Object -First 1
  $model = Join-Path $scratch 'model.gguf'
  $vision = Join-Path $scratch 'vision.gguf'
  $hf = 'https://huggingface.co/Qwen/Qwen3-VL-2B-Instruct-GGUF/resolve/52d6c8f'
  curl.exe -fLS --retry 3 -o $model "$hf/Qwen3VL-2B-Instruct-Q4_K_M.gguf"
  curl.exe -fLS --retry 3 -o $vision "$hf/mmproj-Qwen3VL-2B-Instruct-Q8_0.gguf"
  $modelLog = Join-Path $scratch 'model.log'
  $server = Start-Process -FilePath $exe.FullName -ArgumentList @('--host','127.0.0.1','--port',"$port",'-m',"`"$model`"",'--mmproj',"`"$vision`"",'--alias',$aliasName,'-c','4096','-np','1','-ngl','0','-t','4') -RedirectStandardError $modelLog -RedirectStandardOutput (Join-Path $scratch 'server.out') -PassThru
  Wait-Proof 'local vision model' {
    if ($server.HasExited) { Get-Content $modelLog -Tail 40 | Write-Host; throw "llama-server exited: $($server.ExitCode)" }
    try { (Invoke-RestMethod "http://127.0.0.1:$port/health" -NoProxy -TimeoutSec 2).status -eq 'ok' } catch { $false }
  } 120
  Write-Host 'Actual Qwen3-VL 2B vision model is healthy on loopback.'
  $env:LOCALAPPDATA = Join-Path $scratch 'local'
  $env:USERPROFILE = Join-Path $scratch 'home'
  $env:CU_NO_AUTOSTART = '1'
  $env:CU_NO_REPAIR = '1'
  Remove-Item Env:CU_E2E_VISIBLE,Env:HOMEDRIVE,Env:HOMEPATH,Env:CLAUDE_CONFIG_DIR -ErrorAction SilentlyContinue
  # Narrow runner-only switch: explicitly expose Chromium controls to native UI Automation.
  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--force-renderer-accessibility'
  $data=Join-Path $env:LOCALAPPDATA 'claude-usage'
  New-Item -ItemType Directory -Force -Path $data,$env:USERPROFILE | Out-Null
  @{ profiles=@(); autostart=$false; widget_enabled=$false; notify_enabled=$false; token_refresh=$false; local_ai_endpoint="http://127.0.0.1:$port"; local_ai_backend='llama_cpp'; local_ai_model=$aliasName; local_ai_managed=$false } |
    ConvertTo-Json | Set-Content -Encoding utf8 (Join-Path $data 'config.json')
  foreach($cls in @($trayClass,$popupClass,$dashClass,$pickerClass)){
    Assert-Proof ([ProofWin]::Find($cls) -eq [IntPtr]::Zero) "Unexpected existing window: $cls"
  }
  $fixtureReady = Join-Path $scratch 'fixture.json'
  $fixture = Start-Process powershell.exe -ArgumentList @('-NoProfile','-STA','-ExecutionPolicy','Bypass','-File',"`"$PSScriptRoot/fixture.ps1`"",'-Ready',"`"$fixtureReady`"") -RedirectStandardError (Join-Path $scratch 'fixture-error.log') -PassThru
  [void](Wait-Proof 'non-central text fixture' { Test-Path $fixtureReady })
  $f=Get-Content -Raw $fixtureReady | ConvertFrom-Json
  $fh=[IntPtr]$f.hwnd
  Assert-Proof ((Owner $fh) -eq $fixture.Id) 'Fixture owner mismatch'
  $tray=Start-Process (Join-Path $root 'rust/target/debug/claude-usage.exe') -ArgumentList '--hidden' -PassThru
  $trayWindow=Wait-Proof 'real tray window' { if($tray.HasExited){throw "Tray exited before creating its window: $($tray.ExitCode)"};$w=[ProofWin]::Find($trayClass); if($w -ne [IntPtr]::Zero){$w} }
  Assert-Proof ((Owner $trayWindow) -eq $tray.Id) 'Tray owner mismatch'
  Start-Sleep -Milliseconds 500
  Foreground $fh

  # Start watching before the real global shortcut. Sample top-level visible windows every 8 ms.
  $watcher=[ProofWin+VisibilityWatch]::new($focusClasses[0],$focusClasses[1])
  $coldClock=[Diagnostics.Stopwatch]::StartNew()
  Send-Keys @(17,18,79)
  $picker=Wait-Proof 'first native picker from Ctrl+Alt+O' { Visible $pickerClass }
  $coldMs=$coldClock.ElapsedMilliseconds
  $childOwner=Owner $picker
  Assert-Proof ($childOwner -ne 0 -and (Get-Process -Id $childOwner).ProcessName -ceq 'claude-usage-dashboard') 'Picker not owned by product dashboard binary'
  Start-Sleep -Milliseconds 300
  Assert-Proof (!$watcher.Seen -and !(Visible $popupClass) -and !(Visible $dashClass)) 'Popup/dashboard visible during first picker'
  Foreground $picker
  Send-Keys @(27)
  [void](Wait-Proof 'cancelled fresh picker and popup closed' {
    [ProofWin]::Find($pickerClass) -eq [IntPtr]::Zero -and
    [ProofWin]::Find($popupClass) -eq [IntPtr]::Zero
  })
  [void](Wait-Proof 'first picker child actually exits' { !(Get-Process -Id $childOwner -ErrorAction SilentlyContinue) } 8)
  Start-Sleep -Milliseconds 500
  Assert-Proof (!$watcher.Seen -and $watcher.Samples -gt 15 -and !(Visible $popupClass) -and !(Visible $dashClass)) 'Empty popup visible after first Escape'
  Write-Host "OCRPOP-WIN-1: Ctrl+Alt+O presented native region picker, Esc closed fresh picker and child PID $childOwner with no visible popup/dashboard across $($watcher.Samples) samples; first picker observed in ${coldMs} ms."
  $watcher.Dispose();$watcher=$null

  Foreground $fh
  $x1=[int]$f.x+10;$y1=[int]$f.y+10;$x2=$x1+460;$y2=$y1+110
  Assert-Proof ((Root-At $x1 $y1) -eq $fh -and (Root-At $x2 $y2) -eq $fh) 'Fixture does not own the intended physical pixels'
  $watcher=[ProofWin+VisibilityWatch]::new($focusClasses[0],$focusClasses[1])
  Send-Keys @(17,18,79)
  $picker=Wait-Proof 'repeated Ctrl+Alt+O native picker' { Visible $pickerClass }
  $childOwner=Owner $picker
  Start-Sleep -Milliseconds 250
  Assert-Proof (!$watcher.Seen) 'Popup shown before repeated shortcut selection'
  $watcher.Dispose();$watcher=$null

  # Assert the fixture really covered the sampled pixels before showing the picker and that both
  # endpoints are now physically owned by the native picker. The region is deliberately left/top.
  Assert-Proof ((Root-At $x1 $y1) -eq $picker -and (Root-At $x2 $y2) -eq $picker) 'Native picker does not own the two drag endpoints'
  Foreground $picker
  Mouse-At $x1 $y1
  [ProofWin]::mouse_event(2,0,0,0,[IntPtr]::Zero)
  try { Start-Sleep -Milliseconds 125;Mouse-At $x2 $y2;Start-Sleep -Milliseconds 125 }
  finally { [ProofWin]::mouse_event(4,0,0,0,[IntPtr]::Zero) }
  $popupClock=[Diagnostics.Stopwatch]::StartNew()
  $popup=Wait-Proof 'anchored popup after selected drag' { Visible $popupClass } 45
  $panelMs=$popupClock.ElapsedMilliseconds
  Assert-Proof ((Owner $popup) -eq $childOwner -and !(Visible $pickerClass) -and !(Visible $dashClass)) 'Wrong process/window after capture'
  $r=[ProofWin+Rect]::new()
  Assert-Proof ([ProofWin]::GetWindowRect($popup,[ref]$r)) 'Could not get physical popup bounds'
  $wa=[ProofWin]::WorkAt([int](($x1+$x2)/2),[int](($y1+$y2)/2))
  $dpi=[ProofWin]::GetDpiForWindow($popup)
  Assert-Proof ($dpi -gt 0) 'Could not get popup DPI'
  $gap=[int][Math]::Round(8*$dpi/96.0)
  $tol=[Math]::Max(3,[int][Math]::Ceiling($dpi/96.0*3))
  Assert-Proof ($r.Left -ge $wa.Left-$tol -and $r.Top -ge $wa.Top-$tol -and $r.Right -le $wa.Right+$tol -and $r.Bottom -le $wa.Bottom+$tol) 'Physical popup lies outside selected monitor work area'
  $expectedLeft=[Math]::Min([Math]::Max($x1,$wa.Left),$wa.Right-($r.Right-$r.Left))
  Assert-Proof ([Math]::Abs($r.Left-$expectedLeft) -le $tol) 'Popup horizontal origin not aligned to captured region'
  $adjacent=([Math]::Abs($r.Top-$y2-$gap) -le $tol) -or ([Math]::Abs($y1-$r.Bottom-$gap) -le $tol)
  Assert-Proof $adjacent 'Physical popup is not adjacent above/below selection at the expected DPI-scaled gap'
  Write-Host "Bounds (physical px): capture=[$x1,$y1,$x2,$y2] popup=[$($r.Left),$($r.Top),$($r.Right),$($r.Bottom)] work=[$($wa.Left),$($wa.Top),$($wa.Right),$($wa.Bottom)] DPI=$dpi"

  Foreground $popup
  # No Recognize action: an enabled Copy button and the unique fixture text on
  # the real clipboard prove that automatic OCR actually ran on the selected image.
  $copy=Probe 'copy' $popup
  $ocrMs=$popupClock.ElapsedMilliseconds
  Set-Clipboard -Value 'SIG4A-popup-clipboard-before-copy'
  Button-Click $copy $popup
  $ocr=Wait-Proof 'recognized fixture text in actual clipboard' {
    try {
      $value=Get-Clipboard -Raw -ErrorAction Stop
      if($value -match '(?i)^close the door[.!]?$'){return $value}
    } catch {}
  } 8

  $translate=Probe 'translate' $popup
  Button-Click $translate $popup
  $viCopy=Probe 'translated-copy' $popup
  Assert-Proof ((Visible $popupClass) -eq $popup -and !(Visible $dashClass)) 'Translation opened a different native window'
  Set-Clipboard -Value 'SIG4A-popup-clipboard-before-translation-copy'
  Button-Click $viCopy $popup
  $vi=Wait-Proof 'semantically correct inline Vietnamese on actual clipboard' {
    try {
      $value=Get-Clipboard -Raw -ErrorAction Stop
      if($value -match '(?i)đóng' -and $value -match '(?i)cửa'){return $value}
    } catch {}
  } 8
  Write-Host "Native UIA buttons + physical clicks copied OCR '$ocr' and inline Vietnamese '$vi' from HWND $popup."
  # A second hotkey while the popup is open must hide it during selection,
  # then Esc must restore the same HWND, text and inline translation.
  Foreground $popup
  Send-Keys @(17,18,79)
  $picker=Wait-Proof 'recapture from an open popup' { Visible $pickerClass }
  Assert-Proof ((Owner $picker) -eq $childOwner -and !(Visible $popupClass)) 'Recapture did not hide/reuse the previous window'
  Foreground $picker
  Send-Keys @(27)
  [void](Wait-Proof 'recapture cancellation restores the previous popup' { (Visible $popupClass) -eq $popup -and !(Visible $pickerClass) })
  $copyAfter=Probe 'copy' $popup
  Set-Clipboard -Value 'SIG4A-popup-after-escape-before-ocr-copy'
  Button-Click $copyAfter $popup
  [void](Wait-Proof 'same OCR value copied after warm Escape' { Compare-Clipboard $ocr } 8)
  $viCopyAfter=Probe 'translated-copy' $popup
  Set-Clipboard -Value 'SIG4A-popup-after-escape-before-translation-copy'
  Button-Click $viCopyAfter $popup
  [void](Wait-Proof 'same inline translation copied after warm Escape' { Compare-Clipboard $vi } 8)
  Write-Host 'Recapture Esc restored the same popup, OCR text and inline translation.'
  Write-Host ("OCRPOP-WIN-2: " + (@{ native=$true; product=$revision; backend='llama_cpp'; model=$aliasName; instrumentation='uia-physical-pointer'; ocr=$ocr; inlineVietnamese=$vi; copiedOcr=$true; copiedVietnamese=$true; samePopup=$true; recaptureEscapePreserved=$true; hotkeyToPickerMs=$coldMs; selectionToVisibleMs=$panelMs; selectionToOcrObservedMs=$ocrMs } | ConvertTo-Json -Compress))

} catch {
  if (Test-Path "$data/events.log") { Get-Content "$data/events.log" -Tail 40 | Write-Host }
  if ($tray) { [ProofWin]::WindowsFor($tray.Id) | Write-Host }
  if ($childOwner) { [ProofWin]::WindowsFor($childOwner) | Write-Host }
  if ($modelLog -and (Test-Path $modelLog)) { Get-Content $modelLog -Tail 20 | Write-Host }
  throw
} finally {
  if ($watcher) { $watcher.Dispose() }
  foreach($process in @($tray,$fixture,$server)) {
    if($process -and !$process.HasExited){Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue}
  }
}
