param(
 [Parameter(Mandatory=$true)][long]$Hwnd,
 [string]$Id='',
 [string]$Name='',
 [string]$Kind='',
 [ValidateSet('point','focus','state','list','invoke')][string]$Mode='point',
 [switch]$Patterns,
 [uint32]$ExpectedRootPid=0
)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName UIAutomationClient,UIAutomationTypes,WindowsBase
Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class DesktopUiaWin {
 [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
 public delegate bool EnumProc(IntPtr hwnd,IntPtr p);
 [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr root,EnumProc proc,IntPtr p);
 [DllImport("user32.dll",CharSet=CharSet.Unicode,ExactSpelling=true)]
 private static extern int GetClassNameW(IntPtr h,StringBuilder name,int count);
 private static bool IsAuxiliary(IntPtr h) {
  var name=new StringBuilder(256);
  if(GetClassNameW(h,name,name.Capacity)==0)return false;
  var cls=name.ToString();
  return cls=="Tao Thread Event Target" || cls=="MSCTFIME UI" || cls=="IME";
 }
 public static IntPtr[] Children(IntPtr root) {
  var x=new List<IntPtr>();
  EnumChildWindows(root,(h,p)=>{if(!IsAuxiliary(h))x.Add(h);return true;},IntPtr.Zero);
  return x.ToArray();
 }
}
'@
[void][DesktopUiaWin]::SetProcessDPIAware()
$main=[IntPtr]$Hwnd
$names=[Collections.Generic.List[object]]::new()
$rootBounds=$null
$rootState='not_observed'
$rootsRead=0
$scanErrors=0
$rootOwnerVerified=$false
function Write-UiaJson($Value) {
 # Windows PowerShell's console code page may not represent Vietnamese text.
 # Escape localized names in every response, not just navigation inventory.
 $json=$Value|ConvertTo-Json -Depth 5 -Compress
 [regex]::Replace($json,'[^\x00-\x7f]',{param($m) '\u{0:x4}' -f [int][char]$m.Value[0]})
}
foreach($h in @($main)+@([DesktopUiaWin]::Children($main))){
 try {
  $root=[Windows.Automation.AutomationElement]::FromHandle($h)
  if(!$root){continue}
  $rootsRead++
  if(($Mode -eq 'list' -or $ExpectedRootPid) -and $h -eq $main){
    # Same UIA screen coordinate space as the descendants, not Win32's DPI
    # virtualized GetWindowRect as observed by the invoking process.
    $rootState='bounds_unavailable'
    $rootInfo=$root.Current
    if($ExpectedRootPid -and $rootInfo.ProcessId -ne $ExpectedRootPid){
      $rootState='unexpected_process'
      break
    }
    $rootOwnerVerified=$true
    $rect=$rootInfo.BoundingRectangle
    if(!$rect.IsEmpty){
      $rootState='observed'
      $rootBounds=@{left=$rect.Left;top=$rect.Top;right=$rect.Right;bottom=$rect.Bottom;process_id=$rootInfo.ProcessId}
    }
  }
  if($ExpectedRootPid -and !$rootOwnerVerified){break}
  $cond=[Windows.Automation.Condition]::TrueCondition
  foreach($e in $root.FindAll([Windows.Automation.TreeScope]::Descendants,$cond)){
    try {$c=$e.Current} catch {
      if($Mode -ne 'list'){throw}
      $scanErrors++
      continue
    }
    if($Kind -and $c.ControlType.ProgrammaticName -notmatch ('[.]('+$Kind+')$')){continue}
    if(($Id -or $Name) -and !(($Id -and $c.AutomationId -ceq $Id) -or ($Name -and $c.Name -match $Name))){continue}
    if($Mode -eq 'list'){
      if($c.Name -and $names.Count -lt 300){
        $rect=$c.BoundingRectangle
        $item=@{name=$c.Name;id=$c.AutomationId;kind=$c.ControlType.ProgrammaticName;enabled=$c.IsEnabled;offscreen=$c.IsOffscreen}
        if($Patterns){
          try {$item.patterns=@($e.GetSupportedPatterns() | ForEach-Object ProgrammaticName)} catch {}
        }
        if(!$rect.IsEmpty){
          $item.left=[int][Math]::Round($rect.Left);$item.top=[int][Math]::Round($rect.Top)
          $item.right=[int][Math]::Round($rect.Right);$item.bottom=[int][Math]::Round($rect.Bottom)
        }
        $names.Add($item)
      }
      continue
    }
    if($Mode -eq 'point' -and $c.IsOffscreen -and $c.IsEnabled){
      try {$e.GetCurrentPattern([Windows.Automation.ScrollItemPattern]::Pattern).ScrollIntoView();$c=$e.Current} catch {}
    }
    if($Mode -eq 'focus' -and $c.IsEnabled){ try{$e.SetFocus()}catch{continue} }
    if($Mode -eq 'invoke'){
      if(!$c.IsEnabled){continue}
      if($c.IsOffscreen){
        try {$e.GetCurrentPattern([Windows.Automation.ScrollItemPattern]::Pattern).ScrollIntoView()} catch {}
        $c=$e.Current
        if($c.IsOffscreen){
          try {$e.SetFocus()} catch {}
          $c=$e.Current
        }
        if($c.IsOffscreen){continue}
      }
      if($ExpectedRootPid){
        $r=$c.BoundingRectangle
        if(!$rootBounds -or $r.IsEmpty -or $r.Width -le 0 -or $r.Height -le 0){continue}
        $cx=$r.X+$r.Width/2
        $cy=$r.Y+$r.Height/2
        if($cx -lt $rootBounds.left -or $cx -ge $rootBounds.right -or
           $cy -lt $rootBounds.top -or $cy -ge $rootBounds.bottom){continue}
      }
      # aria-pressed Chromium buttons expose TogglePattern, not always InvokePattern.
      # A real UIA activate is required; the caller still verifies the resulting page/status.
      $activated=''
      try {
        $p=$null
        if($e.TryGetCurrentPattern([Windows.Automation.InvokePattern]::Pattern,[ref]$p)){
          ([Windows.Automation.InvokePattern]$p).Invoke()
          $activated='Invoke'
        } elseif($e.TryGetCurrentPattern([Windows.Automation.TogglePattern]::Pattern,[ref]$p)){
          ([Windows.Automation.TogglePattern]$p).Toggle()
          $activated='Toggle'
        } elseif($c.ControlType -eq [Windows.Automation.ControlType]::TabItem -and
                 $e.TryGetCurrentPattern([Windows.Automation.SelectionItemPattern]::Pattern,[ref]$p)){
          ([Windows.Automation.SelectionItemPattern]$p).Select()
          $activated='Select'
        }
      } catch { continue }
      if(!$activated){continue}
      Write-UiaJson @{found=$true;name=$c.Name;id=$c.AutomationId;pattern=$activated}
      exit 0
    }
    $item=@{found=$true;name=$c.Name;id=$c.AutomationId;kind=$c.ControlType.ProgrammaticName;enabled=$c.IsEnabled;offscreen=$c.IsOffscreen;focused=$e.Current.HasKeyboardFocus}
    try {$item.selectedText=@($e.GetCurrentPattern([Windows.Automation.TextPattern]::Pattern).GetSelection() | ForEach-Object {$_.GetText(-1)})} catch {}
    try {$item.toggle=[string]$e.GetCurrentPattern([Windows.Automation.TogglePattern]::Pattern).Current.ToggleState} catch {}
    try {$item.range=$e.GetCurrentPattern([Windows.Automation.RangeValuePattern]::Pattern).Current.Value} catch {}
    try {$item.value=$e.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value} catch {}
    try {$item.selection=@($e.GetCurrentPattern([Windows.Automation.SelectionPattern]::Pattern).Current.GetSelection()|ForEach-Object {$_.Current.Name})} catch {}
    $rect=$c.BoundingRectangle
    if(!$rect.IsEmpty -and $rect.Width -gt 0 -and $rect.Height -gt 0){
      $p=[Windows.Point]::new(0,0)
      if(!$e.TryGetClickablePoint([ref]$p)){$p=[Windows.Point]::new($rect.X+$rect.Width/2,$rect.Y+$rect.Height/2)}
      $item.x=[int][Math]::Round($p.X);$item.y=[int][Math]::Round($p.Y)
    }
    if($Mode -ne 'point' -or ($item.enabled -and !$item.offscreen -and $item.ContainsKey('x'))){
      Write-UiaJson $item
      exit 0
    }
  }
 }catch{if($Mode -eq 'list'){$scanErrors++}}
}
if($Mode -eq 'list'){
 Write-UiaJson @{root=$rootBounds;root_state=$rootState;roots_read=$rootsRead;scan_errors=$scanErrors;items=@($names.ToArray())}
}
else{'{"found":false}'}
