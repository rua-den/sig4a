param([string]$Ready)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName PresentationCore,PresentationFramework,WindowsBase
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class FixtureWin {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr window, ref Point p);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool RegisterHotKey(IntPtr hwnd,int id,uint mods,uint key);
  [DllImport("user32.dll")] public static extern bool UnregisterHotKey(IntPtr hwnd,int id);
  [StructLayout(LayoutKind.Sequential)] public struct Point { public int X,Y; }
}
'@
[void][FixtureWin]::SetProcessDPIAware()
$win = [Windows.Window]::new()
$win.Title='SIG4A synthetic user input and reserved chord'
$win.WindowStyle='None';$win.ResizeMode='NoResize';$win.WindowStartupLocation='Manual'
$win.Left=30;$win.Top=45;$win.Width=880;$win.Height=635;$win.Topmost=$false
$canvas=[Windows.Controls.Canvas]::new();$canvas.Background=[Windows.Media.Brushes]::White
$txt=[Windows.Controls.TextBox]::new()
$txt.Width=360;$txt.Height=85;$txt.Padding=[Windows.Thickness]::new(12,10,12,10)
$txt.FontSize=24;$txt.Text='FixtureInputAlpha'
[Windows.Automation.AutomationProperties]::SetAutomationId($txt,'sig4a-synthetic-edit')
[Windows.Controls.Canvas]::SetLeft($txt,10);[Windows.Controls.Canvas]::SetTop($txt,15)
[void]$canvas.Children.Add($txt)
$win.Content=$canvas
$win.Add_ContentRendered({
  $h=([Windows.Interop.WindowInteropHelper]::new($win)).Handle
  $origin=[FixtureWin+Point]::new()
  if(![FixtureWin]::ClientToScreen($h,[ref]$origin)){throw 'Failed to obtain WPF client origin'}
  # Reserve Ctrl+Shift+J in another real GUI process for an actual RegisterHotKey conflict.
  if(![FixtureWin]::RegisterHotKey($h,0x453,0x4006,0x4A)){throw 'Could not reserve fixture chord Ctrl+Shift+J'}
  @{ x=$origin.X; y=$origin.Y; w=[int]$canvas.ActualWidth; h=[int]$canvas.ActualHeight; hwnd=$h.ToInt64(); reserved='Ctrl+Shift+J' }|
    ConvertTo-Json -Compress | Set-Content -Encoding ASCII $Ready
})
$win.Add_Closed({[void][FixtureWin]::UnregisterHotKey(([Windows.Interop.WindowInteropHelper]::new($win)).Handle,0x453)})
[void]$win.ShowDialog()
