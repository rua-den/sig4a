$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$scratch = Join-Path $env:RUNNER_TEMP 'sig4a-ocr-proof'
New-Item -ItemType Directory -Force -Path $scratch | Out-Null
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class ProofWin {
 [StructLayout(LayoutKind.Sequential)] public struct Point {
   public int X,Y;
   public Point(int x,int y) { X=x;Y=y; }
 }
 [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
 [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(Point p);
 [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr window, uint flags);
 [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr window, ref Point p);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr window);
 [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd,out uint process);
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
 [DllImport("user32.dll")] public static extern void mouse_event(uint flags,int dx,int dy,uint data,IntPtr extra);
}
'@
[void][ProofWin]::SetProcessDPIAware()
function Wait-Proof($label, [scriptblock]$check, [int]$seconds = 30) {
  $watch = [Diagnostics.Stopwatch]::StartNew()
  while ($watch.Elapsed.TotalSeconds -lt $seconds) {
    if (& $check) { return }
    Start-Sleep -Milliseconds 125
  }
  if (Test-Path $out) { Write-Host "Dashboard result: $(Get-Content -Raw $out)" }
  throw "Timed out waiting for $label"
}
function Owned-By([IntPtr]$hwnd,[int]$owner) {
  [uint32]$foundOwner = 0
  [void][ProofWin]::GetWindowThreadProcessId($hwnd,[ref]$foundOwner)
  return $hwnd -ne [IntPtr]::Zero -and $foundOwner -eq $owner
}
function Root-At([int]$x,[int]$y) {
  return [ProofWin]::GetAncestor([ProofWin]::WindowFromPoint([ProofWin+Point]::new($x,$y)),2)
}
function Mouse-At([int]$x,[int]$y) {
  if (-not [ProofWin]::SetCursorPos($x,$y)) { throw 'Could not position pointer' }
}
$port = 18080
$aliasName = 'qwen3-vl-2b'
$server = $fixture = $dashboard = $null
$out = Join-Path $scratch 'result.json'
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
  $env:CU_E2E_VISIBLE = '1'
  $cfg = Join-Path $env:LOCALAPPDATA 'claude-usage'
  New-Item -ItemType Directory -Force -Path $cfg,$env:USERPROFILE | Out-Null
  @{ autostart = $false; local_ai_endpoint = "http://127.0.0.1:$port"; local_ai_backend = 'llama_cpp'; local_ai_model = $aliasName; local_ai_managed = $false } |
    ConvertTo-Json | Set-Content -Encoding utf8 (Join-Path $cfg 'config.json')
  $fixtureReady = Join-Path $scratch 'fixture.json'
  $fixture = Start-Process powershell.exe -ArgumentList @('-NoProfile','-STA','-ExecutionPolicy','Bypass','-File',"`"$PSScriptRoot/fixture.ps1`"",'-Ready',"`"$fixtureReady`"") -RedirectStandardError (Join-Path $scratch 'fixture-error.log') -PassThru
  Wait-Proof 'synthetic text window' { Test-Path $fixtureReady }
  $f = Get-Content -Raw $fixtureReady | ConvertFrom-Json
  $dash = Join-Path $root 'rust/target/debug/claude-usage-dashboard.exe'
  $dashboard = Start-Process -FilePath $dash -ArgumentList @('--e2e',"`"$PSScriptRoot/proof.js`"","`"$out`"") -PassThru
  Wait-Proof 'dashboard OCR UI stage' { Test-Path "$out.capture" }
  New-Item -ItemType File "$out.capture.go" | Out-Null
  Wait-Proof 'native region picker owned by dashboard' {
    if ($dashboard.HasExited) { throw "Dashboard exited: $($dashboard.ExitCode)" }
    if (Test-Path $out) { throw "Dashboard finished before opening picker: $(Get-Content -Raw $out)" }
    $h = [ProofWin]::FindOwned($dashboard.Id, 'KaneRegion')
    (Owned-By $h $dashboard.Id) -and [ProofWin]::IsWindowVisible($h)
  } 50
  $picker = [ProofWin]::FindOwned($dashboard.Id, 'KaneRegion')
  $x1 = $f.x + 10; $y1 = $f.y + 10; $x2 = $x1 + 460; $y2 = $y1 + 110
  if ((Root-At $x1 $y1) -ne $picker -or (Root-At $x2 $y2) -ne $picker) { throw 'Picker does not own the intended drag area' }
  Mouse-At $x1 $y1
  [ProofWin]::mouse_event(2,0,0,0,[IntPtr]::Zero)
  try { Start-Sleep -Milliseconds 140; Mouse-At $x2 $y2; Start-Sleep -Milliseconds 140 }
  finally { [ProofWin]::mouse_event(4,0,0,0,[IntPtr]::Zero) }
  Write-Host 'Dragged actual Windows picker over synthetic text fixture.'
  Wait-Proof 'model OCR and Copy UI stage' { (Test-Path "$out.copy") -or (Test-Path $out) } 95
  if (!(Test-Path "$out.copy")) { throw "OCR stage failed: $(Get-Content -Raw $out)" }
  $c = Get-Content -Raw "$out.copy" | ConvertFrom-Json
  $hwnd = [ProofWin]::FindOwned($dashboard.Id, 'ClaudeUsageDashboard')
  if (!(Owned-By $hwnd $dashboard.Id)) { throw 'Dashboard Copy target not found' }
  $p = [ProofWin+Point]::new($c.x,$c.y)
  if (![ProofWin]::ClientToScreen($hwnd,[ref]$p) -or (Root-At $p.X $p.Y) -ne $hwnd) { throw 'Copy point is not over dashboard' }
  Set-Clipboard -Value 'SIG4A-clipboard-before-copy'
  Mouse-At $p.X $p.Y
  [ProofWin]::mouse_event(2,0,0,0,[IntPtr]::Zero)
  [ProofWin]::mouse_event(4,0,0,0,[IntPtr]::Zero)
  Wait-Proof 'exact OCR text on native Windows clipboard' { (Get-Clipboard -Raw) -ceq $c.text } 8
  Write-Host "Native Windows clipboard verified: $($c.text)"
  New-Item -ItemType File "$out.copy.go" | Out-Null
  Wait-Proof 'live translation on UI' { Test-Path $out } 70
  $result = Get-Content -Raw $out | ConvertFrom-Json
  Write-Host "OCR proof: $($result | ConvertTo-Json -Compress)"
  if ($result.error -or $result.translationInput -cne $c.text) { throw 'OCR was not handed to translation' }
  if (($result.translated -notmatch '(?i)đóng') -or ($result.translated -notmatch '(?i)cửa')) { throw 'Live model did not translate the fixture into Vietnamese' }
} catch {
  if ($dashboard) {
    Write-Host "Dashboard owner PID: $($dashboard.Id), session: $($dashboard.SessionId)"
    [ProofWin]::WindowsFor($dashboard.Id) | Write-Host
    if (Test-Path "$cfg/events.log") { Get-Content "$cfg/events.log" -Tail 35 | Write-Host }
    foreach ($stageFile in (Get-ChildItem $scratch -Filter 'result.json.*')) {
      Write-Host "$($stageFile.Name): $(Get-Content -Raw $stageFile.FullName)"
    }
  }
  throw
} finally {
  foreach ($process in @($dashboard,$fixture,$server)) {
    if ($process -and !$process.HasExited) { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue }
  }
}
