# Generate LocalTransfer app icon: assets/icon.ico (256x256 rounded square + dual arrows)
Add-Type -AssemblyName System.Drawing

$size = 256
$bmp = New-Object System.Drawing.Bitmap($size, $size)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.Clear([System.Drawing.Color]::Transparent)

# rounded square background (indigo #6366F1)
$r = 52
$rect = New-Object System.Drawing.Rectangle(8, 8, 240, 240)
$path = New-Object System.Drawing.Drawing2D.GraphicsPath
$d = $r * 2
$path.AddArc($rect.X, $rect.Y, $d, $d, 180, 90)
$path.AddArc($rect.Right - $d, $rect.Y, $d, $d, 270, 90)
$path.AddArc($rect.Right - $d, $rect.Bottom - $d, $d, $d, 0, 90)
$path.AddArc($rect.X, $rect.Bottom - $d, $d, $d, 90, 90)
$path.CloseFigure()
$bg = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 99, 102, 241))
$g.FillPath($bg, $path)

# white up/down arrows
$white = [System.Drawing.Brushes]::White
$penW = New-Object System.Drawing.Pen([System.Drawing.Color]::White, 16)
$penW.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
$penW.EndCap = [System.Drawing.Drawing2D.LineCap]::Round

# left arrow pointing up
$g.DrawLine($penW, 92, 168, 92, 96)
$up = @(
    (New-Object System.Drawing.PointF(64, 106)),
    (New-Object System.Drawing.PointF(92, 72)),
    (New-Object System.Drawing.PointF(120, 106))
)
$g.FillPolygon($white, $up)

# right arrow pointing down
$g.DrawLine($penW, 164, 88, 164, 160)
$down = @(
    (New-Object System.Drawing.PointF(136, 150)),
    (New-Object System.Drawing.PointF(164, 184)),
    (New-Object System.Drawing.PointF(192, 150))
)
$g.FillPolygon($white, $down)

$g.Dispose()

$outDir = Join-Path $PSScriptRoot "..\assets"
New-Item -ItemType Directory -Force $outDir | Out-Null
$pngPath = Join-Path $outDir "icon.png"
$bmp.Save($pngPath, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()

# wrap PNG into ICO container (single 256x256 image)
$png = [System.IO.File]::ReadAllBytes($pngPath)
$icoPath = Join-Path $outDir "icon.ico"
$ms = New-Object System.IO.MemoryStream
$ms.Write([BitConverter]::GetBytes([UInt16]0), 0, 2)
$ms.Write([BitConverter]::GetBytes([UInt16]1), 0, 2)
$ms.Write([BitConverter]::GetBytes([UInt16]1), 0, 2)
$ms.WriteByte(0); $ms.WriteByte(0); $ms.WriteByte(0); $ms.WriteByte(0)
$ms.Write([BitConverter]::GetBytes([UInt16]1), 0, 2)
$ms.Write([BitConverter]::GetBytes([UInt16]32), 0, 2)
$ms.Write([BitConverter]::GetBytes([UInt32]$png.Length), 0, 4)
$ms.Write([BitConverter]::GetBytes([UInt32]22), 0, 4)
$ms.Write($png, 0, $png.Length)
[System.IO.File]::WriteAllBytes($icoPath, $ms.ToArray())
Write-Host "Icon generated: $icoPath ($($png.Length + 22) bytes)"
