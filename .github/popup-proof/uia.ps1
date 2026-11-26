param(
  [Parameter(Mandatory=$true)][long]$PopupHwnd,
  [Parameter(Mandatory=$true)][ValidateSet('copy','translate','translated-copy')][string]$Mode,
  [int]$TimeoutSeconds = 45
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient,UIAutomationTypes,WindowsBase
Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class UiaNative {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
  public delegate bool EnumProc(IntPtr hwnd,IntPtr data);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr root,EnumProc proc,IntPtr data);
  public static IntPtr[] Children(IntPtr root) {
    var result = new List<IntPtr>();
    EnumChildWindows(root,(hwnd,data)=>{result.Add(hwnd);return true;},IntPtr.Zero);
    return result.ToArray();
  }
}
'@
[void][UiaNative]::SetProcessDPIAware()
$hwnd = [IntPtr]$PopupHwnd
# Spell Vietnamese labels with code points for Windows PowerShell 5's BOM-less
# script decoding. Names or HTML automation IDs are accepted, scoped to our HWND.
$saoChep = 'Sao ch' + [char]0x00e9 + 'p'
$dich = 'D' + [char]0x1ecb + 'ch'
switch($Mode) {
  'copy' { $automationId = 'ocr-copy'; $buttonName = $saoChep }
  'translate' { $automationId = ''; $buttonName = $dich }
  'translated-copy' { $automationId = 'ocr-translation-copy'; $buttonName = $saoChep + ' b' + [char]0x1ea3 + 'n d' + [char]0x1ecb + 'ch' }
}
$conditions = @(
  [Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::NameProperty,$buttonName)
)
if($automationId) {
  $conditions += [Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::AutomationIdProperty,$automationId)
}
$buttonType = [Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ControlTypeProperty,[Windows.Automation.ControlType]::Button)
$identity = $conditions[0]
if($conditions.Count -gt 1) {
  $identity = [Windows.Automation.OrCondition]::new([Windows.Automation.Condition[]]$conditions)
}
$filter = [Windows.Automation.AndCondition]::new($buttonType,$identity)
$watch = [Diagnostics.Stopwatch]::StartNew()
$lastError = ''
$observedOffscreen = $false
while($watch.Elapsed.TotalSeconds -lt $TimeoutSeconds) {
  if(![UiaNative]::IsWindowVisible($hwnd)) { throw 'The requested popup is not visible.' }
  foreach($candidate in @($hwnd)+@([UiaNative]::Children($hwnd))) {
    try {
      $root = [System.Windows.Automation.AutomationElement]::FromHandle($candidate)
      if(!$root) { continue }
      foreach($button in $root.FindAll([Windows.Automation.TreeScope]::Descendants,$filter)) {
        if(!$button.Current.IsEnabled) { continue }
        if($button.Current.IsOffscreen) { $observedOffscreen = $true; continue }
        $rect = $button.Current.BoundingRectangle
        if($rect.IsEmpty -or $rect.Width -lt 1 -or $rect.Height -lt 1) { $observedOffscreen = $true; continue }
        $point = [Windows.Point]::new(0,0)
        if(!$button.TryGetClickablePoint([ref]$point)) {
          $point = [Windows.Point]::new($rect.X+$rect.Width/2,$rect.Y+$rect.Height/2)
        }
        @{ x=[int][Math]::Round($point.X); y=[int][Math]::Round($point.Y); source='uia-screen'; found=$true } |
          ConvertTo-Json -Compress
        exit 0
      }
    } catch { $lastError = $_.Exception.Message }
  }
  if($observedOffscreen) { '{"offscreen":true}'; exit 0 }
  Start-Sleep -Milliseconds 150
}
$labels = @()
try {
  $root = [System.Windows.Automation.AutomationElement]::FromHandle($hwnd)
  foreach($button in $root.FindAll([Windows.Automation.TreeScope]::Descendants,$buttonType)) {
    $labels += ('{0} id={1} enabled={2} offscreen={3}' -f $button.Current.Name,$button.Current.AutomationId,$button.Current.IsEnabled,$button.Current.IsOffscreen)
  }
} catch { $lastError = $_.Exception.Message }
throw "No enabled $Mode button via popup UI Automation. Buttons: $($labels -join ' | '); provider: $lastError"
