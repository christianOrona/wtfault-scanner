<#
.SYNOPSIS
    Draws the source app icon for the Tauri bundler.

.DESCRIPTION
    Writes a 1024x1024 PNG that `npm run tauri icon` expands into the full
    platform icon set. Kept as a script rather than a committed binary so the
    mark can be changed by editing code, and so a fresh clone can regenerate it
    without a design tool.

    The mark is a dial with its needle deflected, over the diagnostic connector
    trapezoid - a gauge you can actually read being the whole idea.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\make-icon.ps1
#>

[CmdletBinding()]
param(
    [string]$OutFile,
    [int]$Size = 1024
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Resolved here rather than as a parameter default: under Windows PowerShell
# 5.1 $PSScriptRoot is not yet populated while parameter defaults are bound.
if (-not $OutFile) {
    $root = Split-Path -Parent $MyInvocation.MyCommand.Path
    $OutFile = Join-Path $root '..\apps\desktop\src-tauri\icons\source.png'
}

Add-Type -AssemblyName System.Drawing

$dir = Split-Path -Parent $OutFile
if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }

$bmp = New-Object System.Drawing.Bitmap($Size, $Size)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias

try {
    $bg     = [System.Drawing.Color]::FromArgb(255, 14, 17, 22)
    $accent = [System.Drawing.Color]::FromArgb(255, 74, 158, 255)
    $dim    = [System.Drawing.Color]::FromArgb(255, 38, 45, 56)
    $hot    = [System.Drawing.Color]::FromArgb(255, 210, 153, 34)

    # Rounded background.
    $bgBrush = New-Object System.Drawing.SolidBrush($bg)
    $r = [int]($Size * 0.18)
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $path.AddArc(0, 0, $r * 2, $r * 2, 180, 90)
    $path.AddArc($Size - $r * 2, 0, $r * 2, $r * 2, 270, 90)
    $path.AddArc($Size - $r * 2, $Size - $r * 2, $r * 2, $r * 2, 0, 90)
    $path.AddArc(0, $Size - $r * 2, $r * 2, $r * 2, 90, 90)
    $path.CloseFigure()
    $g.FillPath($bgBrush, $path)

    $cx = $Size / 2.0
    $cy = $Size * 0.54
    $radius = $Size * 0.30

    # Dial track: a 240-degree sweep, open at the bottom like a real gauge.
    $trackPen = New-Object System.Drawing.Pen($dim, [float]($Size * 0.055))
    $trackPen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $trackPen.EndCap   = [System.Drawing.Drawing2D.LineCap]::Round
    $box = New-Object System.Drawing.RectangleF(
        [float]($cx - $radius), [float]($cy - $radius),
        [float]($radius * 2), [float]($radius * 2))
    $g.DrawArc($trackPen, $box, 150, 240)

    # The travelled part of the sweep, up to the needle.
    $sweep = 168
    $arcPen = New-Object System.Drawing.Pen($accent, [float]($Size * 0.055))
    $arcPen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $arcPen.EndCap   = [System.Drawing.Drawing2D.LineCap]::Round
    $g.DrawArc($arcPen, $box, 150, $sweep)

    # Needle.
    $angle = (150 + $sweep) * [Math]::PI / 180.0
    $needlePen = New-Object System.Drawing.Pen($hot, [float]($Size * 0.032))
    $needlePen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $needlePen.EndCap   = [System.Drawing.Drawing2D.LineCap]::Round
    $g.DrawLine($needlePen,
        [float]$cx, [float]$cy,
        [float]($cx + [Math]::Cos($angle) * $radius * 0.86),
        [float]($cy + [Math]::Sin($angle) * $radius * 0.86))

    # Hub.
    $hub = $Size * 0.045
    $hubBrush = New-Object System.Drawing.SolidBrush($hot)
    $g.FillEllipse($hubBrush, [float]($cx - $hub), [float]($cy - $hub), [float]($hub * 2), [float]($hub * 2))

    # The OBD-II connector trapezoid across the top.
    $w = $Size * 0.30
    $h = $Size * 0.105
    $ty = $Size * 0.135
    $conn = New-Object System.Drawing.Drawing2D.GraphicsPath
    $conn.AddPolygon([System.Drawing.PointF[]]@(
        (New-Object System.Drawing.PointF([float]($cx - $w), [float]$ty)),
        (New-Object System.Drawing.PointF([float]($cx + $w), [float]$ty)),
        (New-Object System.Drawing.PointF([float]($cx + $w * 0.80), [float]($ty + $h))),
        (New-Object System.Drawing.PointF([float]($cx - $w * 0.80), [float]($ty + $h)))
    ))
    $connPen = New-Object System.Drawing.Pen($accent, [float]($Size * 0.028))
    $connPen.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round
    $g.DrawPath($connPen, $conn)

    $bmp.Save($OutFile, [System.Drawing.Imaging.ImageFormat]::Png)
    Write-Host "wrote $OutFile ($Size x $Size)" -ForegroundColor Green
}
finally {
    $g.Dispose()
    $bmp.Dispose()
}
