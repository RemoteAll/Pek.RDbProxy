# 生成 Tauri 需要的图标（32/128/256 PNG + icon.ico），全部由代码画，不引入外部素材。
#
# 用法（项目根执行）：powershell -NoProfile -ExecutionPolicy Bypass -File desktop\tools\gen-icons.ps1
# 说明：ICO 采用「PNG 负载」格式（Vista+ 支持），避免额外的图像库依赖。

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$outDir = Join-Path $PSScriptRoot '..\src-tauri\icons'
$outDir = [System.IO.Path]::GetFullPath($outDir)
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

function New-IconBitmap([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit

    # 背景：深蓝渐变圆角方块
    $rect = New-Object System.Drawing.Rectangle(0, 0, $size, $size)
    $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
        $rect,
        [System.Drawing.Color]::FromArgb(255, 32, 41, 92),
        [System.Drawing.Color]::FromArgb(255, 91, 91, 255),
        45.0)
    $g.FillRectangle($brush, $rect)
    $brush.Dispose()

    # 数据库三层「盘片」
    $white = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(240, 255, 255, 255))
    $ellipseH = [int]($size * 0.16)
    $w = [int]($size * 0.56)
    $x = [int](($size - $w) / 2)
    foreach ($i in 0..2) {
        $y = [int]($size * 0.30 + $i * $size * 0.19)
        $g.FillEllipse($white, $x, $y, $w, $ellipseH)
        if ($i -lt 2) {
            $g.FillRectangle($white, $x, $y + [int]($ellipseH / 2), $w, [int]($ellipseH / 2) + [int]($size * 0.19))
        }
    }
    $white.Dispose()

    # 右下角「代理箭头」
    $pen = New-Object System.Drawing.Pen([System.Drawing.Color]::FromArgb(255, 34, 197, 94), [float]($size * 0.055))
    $pen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $pen.EndCap = [System.Drawing.Drawing2D.LineCap]::ArrowAnchor
    $g.DrawLine($pen,
        [float]($size * 0.58), [float]($size * 0.74),
        [float]($size * 0.84), [float]($size * 0.88))
    $pen.Dispose()

    $g.Dispose()
    return $bmp
}

$sizes = @{ '32x32.png' = 32; '128x128.png' = 128; '128x128@2x.png' = 256; 'icon.png' = 512 }
foreach ($name in $sizes.Keys) {
    $bmp = New-IconBitmap $sizes[$name]
    $path = Join-Path $outDir $name
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Write-Host "生成 $name（$($sizes[$name])x$($sizes[$name])）"
}

# ICO：多尺寸（16/32/48/64/128/256），每帧都是 PNG 负载
$icoSizes = @(16, 32, 48, 64, 128, 256)
$frames = @()
foreach ($s in $icoSizes) {
    $bmp = New-IconBitmap $s
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $frames += , @{ Size = $s; Bytes = $ms.ToArray() }
    $ms.Dispose()
    $bmp.Dispose()
}

$icoPath = Join-Path $outDir 'icon.ico'
$fs = [System.IO.File]::Create($icoPath)
$bw = New-Object System.IO.BinaryWriter($fs)
$bw.Write([UInt16]0)                 # 保留
$bw.Write([UInt16]1)                 # 类型 = ICO
$bw.Write([UInt16]$frames.Count)     # 帧数
$offset = 6 + 16 * $frames.Count
foreach ($frame in $frames) {
    $dim = if ($frame.Size -ge 256) { 0 } else { $frame.Size }
    $bw.Write([Byte]$dim)            # 宽（256 记 0）
    $bw.Write([Byte]$dim)            # 高
    $bw.Write([Byte]0)               # 调色板
    $bw.Write([Byte]0)               # 保留
    $bw.Write([UInt16]1)             # 平面数
    $bw.Write([UInt16]32)            # 位深
    $bw.Write([UInt32]$frame.Bytes.Length)
    $bw.Write([UInt32]$offset)
    $offset += $frame.Bytes.Length
}
foreach ($frame in $frames) { $bw.Write($frame.Bytes) }
$bw.Flush(); $bw.Dispose(); $fs.Dispose()
Write-Host "生成 icon.ico（$($frames.Count) 帧）"
