Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
  public struct RECT { public int Left, Top, Right, Bottom; }
}
"@
$p = Get-Process msedge | Where-Object { $_.MainWindowTitle -like '*Tonearm*' -or $_.MainWindowTitle -like '*127.0.0.1*' } | Select-Object -First 1
if (-not $p) { Get-Process msedge | Where-Object MainWindowTitle | Select-Object Id, MainWindowTitle; exit 1 }
$r = New-Object Win+RECT
[Win]::GetWindowRect($p.MainWindowHandle, [ref]$r) | Out-Null
$w = $r.Right - $r.Left; $h = $r.Bottom - $r.Top
$bmp = New-Object System.Drawing.Bitmap $w, $h
$g = [System.Drawing.Graphics]::FromImage($bmp)
$dc = $g.GetHdc()
[Win]::PrintWindow($p.MainWindowHandle, $dc, 2) | Out-Null
$g.ReleaseHdc($dc)
$bmp.Save((Join-Path $PSScriptRoot 'tv.png'))
"saved $w x $h from '$($p.MainWindowTitle)'"
