param(
 [Parameter(Mandatory=$true)][long]$Hwnd,
 [string]$Id='',
 [string]$Name='',
 [string]$Kind='',
 [ValidateSet('point','focus','state','list')][string]$Mode='point'
)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName UIAutomationClient,UIAutomationTypes,WindowsBase
Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class DesktopUiaWin {
 [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
 public delegate bool EnumProc(IntPtr hwnd,IntPtr p);
 [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr root,EnumProc proc,IntPtr p);
 public static IntPtr[] Children(IntPtr root) {
  var x=new List<IntPtr>();
  EnumChildWindows(root,(h,p)=>{x.Add(h);return true;},IntPtr.Zero);
  return x.ToArray();
 }
}
'@
[void][DesktopUiaWin]::SetProcessDPIAware()
$main=[IntPtr]$Hwnd
$names=[Collections.Generic.List[object]]::new()
$rootBounds=$null
foreach($h in @($main)+@([DesktopUiaWin]::Children($main))){
 try {
  $root=[Windows.Automation.AutomationElement]::FromHandle($h)
  if(!$root){continue}
  if($Mode -eq 'list' -and $h -eq $main){
    # Same UIA screen coordinate space as the descendants, not Win32's DPI
    # virtualized GetWindowRect as observed by the invoking process.
    $rootInfo=$root.Current
    $rect=$rootInfo.BoundingRectangle
    if(!$rect.IsEmpty){
      $rootBounds=@{left=$rect.Left;top=$rect.Top;right=$rect.Right;bottom=$rect.Bottom;process_id=$rootInfo.ProcessId}
    }
  }
  $cond=[Windows.Automation.Condition]::TrueCondition
  foreach($e in $root.FindAll([Windows.Automation.TreeScope]::Descendants,$cond)){
    $c=$e.Current
    if($Kind -and $c.ControlType.ProgrammaticName -notmatch ('[.]('+$Kind+')$')){continue}
    if(($Id -or $Name) -and !(($Id -and $c.AutomationId -ceq $Id) -or ($Name -and $c.Name -match $Name))){continue}
    if($Mode -eq 'list'){
      if($c.Name -and $names.Count -lt 300){
        $rect=$c.BoundingRectangle
        $item=@{name=$c.Name;id=$c.AutomationId;kind=$c.ControlType.ProgrammaticName;enabled=$c.IsEnabled;offscreen=$c.IsOffscreen}
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
      $item|ConvertTo-Json -Compress
      exit 0
    }
  }
 }catch{}
}
if($Mode -eq 'list'){@{root=$rootBounds;items=@($names.ToArray())}|ConvertTo-Json -Depth 5 -Compress}
else{'{"found":false}'}
