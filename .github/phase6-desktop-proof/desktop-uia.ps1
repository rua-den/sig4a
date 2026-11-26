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
foreach($h in @($main)+@([DesktopUiaWin]::Children($main))){
 try {
  $root=[Windows.Automation.AutomationElement]::FromHandle($h)
  if(!$root){continue}
  $cond=[Windows.Automation.Condition]::TrueCondition
  foreach($e in $root.FindAll([Windows.Automation.TreeScope]::Descendants,$cond)){
    $c=$e.Current
    if($Kind -and $c.ControlType.ProgrammaticName -notlike ("*."+$Kind)){continue}
    if(($Id -or $Name) -and !(($Id -and $c.AutomationId -ceq $Id) -or ($Name -and $c.Name -match $Name))){continue}
    if($Mode -eq 'list'){
      if($c.Name -and $names.Count -lt 180){$names.Add(@{name=$c.Name;id=$c.AutomationId;kind=$c.ControlType.ProgrammaticName;enabled=$c.IsEnabled;offscreen=$c.IsOffscreen})}
      continue
    }
    if($Mode -eq 'focus' -and $c.IsEnabled){ try{$e.SetFocus()}catch{continue} }
    $item=@{found=$true;name=$c.Name;id=$c.AutomationId;kind=$c.ControlType.ProgrammaticName;enabled=$c.IsEnabled;offscreen=$c.IsOffscreen;focused=$e.Current.HasKeyboardFocus}
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
if($Mode -eq 'list'){@{items=@($names.ToArray())}|ConvertTo-Json -Depth 5 -Compress}
else{'{"found":false}'}
