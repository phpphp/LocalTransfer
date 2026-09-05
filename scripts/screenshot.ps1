# DPI-aware window screenshot.
# Usage: powershell -File scripts\screenshot.ps1 -TitleLike LocalTransfer -Out shot.png
param(
  [string]$TitleLike = "LocalTransfer",
  [string]$Out = "shot.png",
  [int]$Index = 0
)

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class W {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

[void][W]::SetProcessDPIAware()

$procs = @(Get-Process -Name local-transfer -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 -and $_.MainWindowTitle -like "*$TitleLike*" } | Sort-Object Id)
if ($procs.Count -eq 0) { Write-Output "no window matching $TitleLike"; exit 1 }
if ($Index -ge $procs.Count) { Write-Output "index out of range (found $($procs.Count))"; exit 1 }
$p = $procs[$Index]
$h = $p.MainWindowHandle
[void][W]::SetForegroundWindow($h)
Start-Sleep -Milliseconds 400

$r = New-Object W+RECT
[void][W]::GetWindowRect($h, [ref]$r)
$w = $r.Right - $r.Left
$hh = $r.Bottom - $r.Top
Write-Output ("pid={0} rect={1},{2} size={3}x{4}" -f $p.Id, $r.Left, $r.Top, $w, $hh)

$bmp = New-Object System.Drawing.Bitmap $w, $hh
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($r.Left, $r.Top, 0, 0, (New-Object System.Drawing.Size $w, $hh))
$full = [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $Out))
$bmp.Save($full, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
Write-Output "saved $full"
