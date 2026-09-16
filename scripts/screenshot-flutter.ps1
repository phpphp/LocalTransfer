# DPI-aware screenshot of the Flutter test app window (process name: localtransfer, no dash).
param([string]$Out = "flutter_win.png")
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class WF {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@
[void][WF]::SetProcessDPIAware()
$p = Get-Process -Name localtransfer -ErrorAction SilentlyContinue
if (-not $p) { $p = Get-Process -Name localtransfer -ErrorAction Stop }
[void][WF]::SetForegroundWindow($p[0].MainWindowHandle)
Start-Sleep -Milliseconds 500
$r = New-Object WF+RECT
[void][WF]::GetWindowRect($p[0].MainWindowHandle, [ref]$r)
$w = $r.Right - $r.Left; $h = $r.Bottom - $r.Top
$bmp = New-Object System.Drawing.Bitmap $w, $h
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($r.Left, $r.Top, 0, 0, (New-Object System.Drawing.Size $w, $h))
$bmp.Save([System.IO.Path]::GetFullPath((Join-Path (Get-Location) $Out)), [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
Write-Output "saved $Out ($w x $h)"
