# Crop and scale a region out of a PNG for close inspection.
# Usage: powershell -File scripts\crop.ps1 -In shot_a.png -X 0 -Y 30 -W 240 -H 670 -Scale 2 -Out crop.png
param(
  [string]$In,
  [int]$X = 0, [int]$Y = 0, [int]$W = 200, [int]$H = 200,
  [double]$Scale = 2.0,
  [string]$Out = "crop.png"
)
Add-Type -AssemblyName System.Drawing
$src = [System.Drawing.Image]::FromFile([System.IO.Path]::GetFullPath((Join-Path (Get-Location) $In)))
$rect = New-Object System.Drawing.Rectangle $X, $Y, $W, $H
$dw = [int]($W * $Scale); $dh = [int]($H * $Scale)
$bmp = New-Object System.Drawing.Bitmap $dw, $dh
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
$g.DrawImage($src, (New-Object System.Drawing.Rectangle 0, 0, $dw, $dh), $rect, [System.Drawing.GraphicsUnit]::Pixel)
$full = [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $Out))
$bmp.Save($full, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose(); $src.Dispose()
Write-Output "saved $full ($dw x $dh)"
