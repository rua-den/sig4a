$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing,System.Windows.Forms
$bmp=[Drawing.Bitmap]::new(460,110)
$g=[Drawing.Graphics]::FromImage($bmp)
$font=[Drawing.Font]::new('Arial',32,[Drawing.FontStyle]::Bold,[Drawing.GraphicsUnit]::Pixel)
$align=[Drawing.StringFormat]::new()
$align.Alignment='Center'; $align.LineAlignment='Center'
try{
 $g.Clear([Drawing.Color]::White)
 $g.DrawString('Open the window.',$font,[Drawing.Brushes]::Black,[Drawing.RectangleF]::new(0,0,460,110),$align)
 [Windows.Forms.Clipboard]::SetImage($bmp)
} finally { $align.Dispose();$font.Dispose();$g.Dispose();$bmp.Dispose() }
