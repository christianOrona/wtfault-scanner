<#
.SYNOPSIS
    Screenshots a top-level window by title, for documenting the desktop app.

.DESCRIPTION
    A development convenience: the Tauri window is a native window, so the
    browser tooling cannot see it. This grabs the window rectangle from the
    desktop, which is enough to check a layout or attach a picture to a report.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\capture-window.ps1 -Title "AI Mechanic" -OutFile shot.png
#>

[CmdletBinding()]
param(
    [string]$Title = 'AI Mechanic',
    [string]$OutFile
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not $OutFile) {
    $root = Split-Path -Parent $MyInvocation.MyCommand.Path
    $OutFile = Join-Path $root 'window.png'
}

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms

Add-Type @'
using System;
using System.Runtime.InteropServices;
public class Win32Cap {
    [DllImport("user32.dll")] public static extern IntPtr FindWindow(string cls, string name);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }
}
'@

$hwnd = [Win32Cap]::FindWindow($null, $Title)
if ($hwnd -eq [IntPtr]::Zero) {
    # Fall back to a process lookup: the title may not match exactly.
    $proc = Get-Process | Where-Object { $_.MainWindowTitle -like "*$Title*" } | Select-Object -First 1
    if ($null -eq $proc) { throw "no window titled '$Title' is open" }
    $hwnd = $proc.MainWindowHandle
}

[void][Win32Cap]::ShowWindow($hwnd, 9)   # SW_RESTORE
[void][Win32Cap]::SetForegroundWindow($hwnd)
Start-Sleep -Milliseconds 600

$rect = New-Object Win32Cap+RECT
if (-not [Win32Cap]::GetWindowRect($hwnd, [ref]$rect)) { throw 'GetWindowRect failed' }

$w = $rect.Right - $rect.Left
$h = $rect.Bottom - $rect.Top
if ($w -le 0 -or $h -le 0) { throw "window has no usable size ($w x $h)" }

$bmp = New-Object System.Drawing.Bitmap($w, $h)
$g = [System.Drawing.Graphics]::FromImage($bmp)
try {
    $g.CopyFromScreen($rect.Left, $rect.Top, 0, 0, (New-Object System.Drawing.Size($w, $h)))
    $bmp.Save($OutFile, [System.Drawing.Imaging.ImageFormat]::Png)
    Write-Host "wrote $OutFile ($w x $h)" -ForegroundColor Green
}
finally {
    $g.Dispose()
    $bmp.Dispose()
}
