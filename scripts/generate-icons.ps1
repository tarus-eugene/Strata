$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot)
Add-Type -AssemblyName System.Drawing
New-Item -ItemType Directory -Force 'extension/icons' | Out-Null
$svg = [xml](Get-Content 'assets/brand/mark.svg' -Raw)
$paths = $svg.GetElementsByTagName('path')
$entries = @()
foreach ($dimension in @(16, 32, 48, 128, 256)) {
    $large = [Drawing.Bitmap]::new($dimension * 4, $dimension * 4)
    $graphics = [Drawing.Graphics]::FromImage($large)
    $graphics.Clear([Drawing.Color]::FromArgb(12, 13, 15))
    $graphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $graphics.ScaleTransform($large.Width / 64.0, $large.Height / 64.0)
    $brush = [Drawing.SolidBrush]::new([Drawing.Color]::FromArgb(130, 185, 231))
    # The SVG consists of straight, filled polygons. Read it as the single source of truth.
    foreach ($path in $paths) {
        $drawing = [Drawing.Drawing2D.GraphicsPath]::new()
        $matches = [regex]::Matches($path.d, '([MLHVZ])([^MLHVZ]*)')
        $points = [Collections.Generic.List[Drawing.PointF]]::new()
        $x = 0.0; $y = 0.0
        foreach ($match in $matches) {
            $values = @([regex]::Matches($match.Groups[2].Value, '-?\d+(?:\.\d+)?') | ForEach-Object { [single]$_.Value })
            switch ($match.Groups[1].Value) {
                { $_ -in @('M', 'L') } { for ($i = 0; $i -lt $values.Count; $i += 2) { $x = $values[$i]; $y = $values[$i + 1]; $points.Add([Drawing.PointF]::new($x, $y)) } }
                'H' { $x = $values[0]; $points.Add([Drawing.PointF]::new($x, $y)) }
                'V' { $y = $values[0]; $points.Add([Drawing.PointF]::new($x, $y)) }
            }
        }
        $graphics.FillPolygon($brush, $points.ToArray())
        $drawing.Dispose()
    }
    $small = [Drawing.Bitmap]::new($dimension, $dimension)
    $resizer = [Drawing.Graphics]::FromImage($small)
    $resizer.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $resizer.DrawImage($large, 0, 0, $dimension, $dimension)
    $stream = [IO.MemoryStream]::new()
    $small.Save($stream, [Drawing.Imaging.ImageFormat]::Png)
    $bytes = $stream.ToArray()
    [IO.File]::WriteAllBytes((Join-Path (Get-Location) "extension/icons/strata-$dimension.png"), $bytes)
    $entries += @{ size = $dimension; bytes = $bytes }
    $stream.Dispose(); $resizer.Dispose(); $small.Dispose(); $brush.Dispose(); $graphics.Dispose(); $large.Dispose()
}
$icon = [IO.MemoryStream]::new()
$writer = [IO.BinaryWriter]::new($icon)
$writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]$entries.Count)
$offset = 6 + 16 * $entries.Count
foreach ($entry in $entries) {
    $sizeByte = if ($entry.size -eq 256) { 0 } else { $entry.size }
    $writer.Write([byte]$sizeByte); $writer.Write([byte]$sizeByte); $writer.Write([uint16]0)
    $writer.Write([uint16]1); $writer.Write([uint16]32)
    $writer.Write([uint32]$entry.bytes.Length); $writer.Write([uint32]$offset)
    $offset += $entry.bytes.Length
}
foreach ($entry in $entries) { $writer.Write([byte[]]$entry.bytes) }
[IO.File]::WriteAllBytes((Join-Path (Get-Location) 'assets/brand/strata.ico'), $icon.ToArray())
$writer.Dispose(); $icon.Dispose()
