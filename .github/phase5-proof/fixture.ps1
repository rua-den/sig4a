param([string]$Ready)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName PresentationFramework
$win = [Windows.Window]::new()
$win.Title = 'SIG4A phase five synthetic selection'
$win.Left=20; $win.Top=50; $win.Width=405; $win.Height=160; $win.Topmost=$false
$panel=[Windows.Controls.StackPanel]::new()
$box=[Windows.Controls.TextBox]::new(); $box.Text='Synthetic phase five selection'; $box.FontSize=18; $box.Height=38
$button=[Windows.Controls.Button]::new(); $button.Content='Focus without selection'; $button.Height=34
[void]$panel.Children.Add($box); [void]$panel.Children.Add($button); $win.Content=$panel
$win.Add_ContentRendered({
 [void]$win.Activate()
 $i=$box.PointToScreen([Windows.Point]::new(70,18)); $b=$button.PointToScreen([Windows.Point]::new(70,18))
 @{hwnd=([Windows.Interop.WindowInteropHelper]::new($win)).Handle.ToInt64();input=@{x=[int]$i.X;y=[int]$i.Y};button=@{x=[int]$b.X;y=[int]$b.Y}} | ConvertTo-Json -Compress | Set-Content -Encoding ASCII $Ready
})
$win.ShowDialog() | Out-Null
