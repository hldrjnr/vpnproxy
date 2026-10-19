# Gera os ícones da app (sem dependências externas, só .NET do Windows).
# Desenha um símbolo "power" sobre fundo escuro arredondado.
Add-Type -AssemblyName System.Drawing

$outDir = Join-Path (Join-Path (Join-Path $PSScriptRoot "..") "src-tauri") "icons"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

function New-ProxyIcon($size, $path) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.Clear([System.Drawing.Color]::Transparent)

    # fundo arredondado #202020
    $rad = [int]($size * 0.24)
    $d = $rad * 2
    $bg = New-Object System.Drawing.Drawing2D.GraphicsPath
    $bg.AddArc(0, 0, $d, $d, 180, 90)
    $bg.AddArc($size - $d, 0, $d, $d, 270, 90)
    $bg.AddArc($size - $d, $size - $d, $d, $d, 0, 90)
    $bg.AddArc(0, $size - $d, $d, $d, 90, 90)
    $bg.CloseFigure()
    $brush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(32, 32, 32))
    $g.FillPath($brush, $bg)

    # símbolo power #4CC2FF
    $accent = [System.Drawing.Color]::FromArgb(76, 194, 255)
    $pen = New-Object System.Drawing.Pen($accent, [float]($size * 0.075))
    $pen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $pen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
    $cx = $size / 2.0
    $cy = $size / 2.0
    $r = $size * 0.23
    $g.DrawArc($pen, ($cx - $r), ($cy - $r), ($r * 2), ($r * 2), 300, 300)
    $g.DrawLine($pen, $cx, ($cy - $r * 1.3), $cx, ($cy + $r * 0.15))

    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose(); $brush.Dispose(); $pen.Dispose(); $bg.Dispose()
}

New-ProxyIcon 32  (Join-Path $outDir "32x32.png")
New-ProxyIcon 128 (Join-Path $outDir "128x128.png")
New-ProxyIcon 256 (Join-Path $outDir "128x128@2x.png")

# .ico a partir do png de 256
$src = New-Object System.Drawing.Bitmap((Join-Path $outDir "128x128@2x.png"))
$src.Save((Join-Path $outDir "icon.ico"), [System.Drawing.Imaging.ImageFormat]::Icon)
$src.Dispose()

Get-ChildItem $outDir | Format-Table Name, Length
