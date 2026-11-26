param([string]$Ready,[string]$Text="Close the door.")
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms,System.Drawing
Add-Type @'
using System.Runtime.InteropServices;
public class FixtureDpi {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
'@
[void][FixtureDpi]::SetProcessDPIAware()
$form = [Windows.Forms.Form]::new()
$form.Text = 'SIG4A synthetic OCR fixture'
$form.FormBorderStyle = 'None'
$form.StartPosition = 'Manual'
$form.Location = [Drawing.Point]::new(30, 55)
$form.ClientSize = [Drawing.Size]::new(480, 130)
$form.BackColor = [Drawing.Color]::White
$form.TopMost = $true
$label = [Windows.Forms.Label]::new()
$label.Dock = 'Fill'
$label.Text = $Text
$label.TextAlign = 'MiddleCenter'
$label.ForeColor = [Drawing.Color]::Black
$label.Font = [Drawing.Font]::new('Arial', 30, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
$form.Controls.Add($label)
$form.Add_Shown({
  $form.Activate()
  $form.Refresh()
  $p = $form.PointToScreen([Drawing.Point]::new(0, 0))
  @{ x = $p.X; y = $p.Y; w = $form.ClientSize.Width; h = $form.ClientSize.Height; hwnd = $form.Handle.ToInt64() } |
    ConvertTo-Json -Compress | Set-Content -Encoding ASCII $Ready
})
[Windows.Forms.Application]::Run($form)

