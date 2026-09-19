# 生成 DeepSeek Otter 图标源图（1024×1024 PNG）：
# 品牌蓝渐变圆角底 + 白色 "O" + 底部水波纹（呼应壳页水獭主题与 --accent 色）。
Add-Type -AssemblyName System.Drawing

$size = 1024
$bmp = New-Object System.Drawing.Bitmap($size, $size)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit

# 圆角矩形（半径 176，约 iOS 风格）作为整体裁剪边界。
$radius = 176
$path = New-Object System.Drawing.Drawing2D.GraphicsPath
$d = $radius * 2
$path.AddArc(0, 0, $d, $d, 180, 90)
$path.AddArc($size - $d, 0, $d, $d, 270, 90)
$path.AddArc($size - $d, $size - $d, $d, $d, 0, 90)
$path.AddArc(0, $size - $d, $d, $d, 90, 90)
$path.CloseFigure()
$g.SetClip($path)

# 背景渐变：品牌蓝 #4d6bfe → 深一档 #2f45c4。
$rect = New-Object System.Drawing.Rectangle(0, 0, $size, $size)
$brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
    $rect,
    ([System.Drawing.Color]::FromArgb(0xFF, 0x4D, 0x6B, 0xFE)),
    ([System.Drawing.Color]::FromArgb(0xFF, 0x2F, 0x45, 0xC4)),
    ([System.Drawing.Drawing2D.LinearGradientMode]::Vertical)
)
$g.FillRectangle($brush, $rect)

# 中央白色 "O"：水獭 Otter 之首字母，占视觉主体，偏上给水波留空间。
$font = New-Object System.Drawing.Font("Segoe UI", 520, ([System.Drawing.FontStyle]::Bold), ([System.Drawing.GraphicsUnit]::Pixel))
$white = [System.Drawing.Brushes]::White
$fmt = New-Object System.Drawing.StringFormat
$fmt.Alignment = [System.Drawing.StringAlignment]::Center
$fmt.LineAlignment = [System.Drawing.StringAlignment]::Center
$textRect = New-Object System.Drawing.RectangleF(0, -60, $size, $size)
$g.DrawString("O", $font, $white, $textRect, $fmt)

# 底部两道水波（sin 带，半透明白），第二道更浅更高，制造层次。
function Fill-Wave($graphics, $baseY, $amplitude, $alpha, $phase) {
    $wave = New-Object System.Drawing.Drawing2D.GraphicsPath
    $wave.AddLine(0, $size, 0, $baseY)
    for ($x = 0; $x -le $size; $x += 8) {
        $y = $baseY + [Math]::Sin(($x / $size) * [Math]::PI * 4 + $phase) * $amplitude
        $wave.AddLine($x, [float]$y, ($x + 8), ($baseY + [Math]::Sin((($x + 8) / $size) * [Math]::PI * 4 + $phase) * $amplitude))
    }
    $wave.AddLine($size, $baseY, $size, $size)
    $wave.CloseFigure()
    $waveBrush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb($alpha, 0xFF, 0xFF, 0xFF))
    $graphics.FillPath($waveBrush, $wave)
}
Fill-Wave $g 830 36 70 ([Math]::PI)
Fill-Wave $g 890 30 130 0

$g.Dispose()
$bmp.Save("D:\100work\103Tools\DeepseekOtter\app-icon.png", [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output "saved app-icon.png"
