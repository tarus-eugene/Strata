$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot)
Add-Type -AssemblyName System.Drawing
New-Item -ItemType Directory -Force 'extension/icons' | Out-Null
$source = [Drawing.Image]::FromFile((Join-Path (Get-Location) 'assets/brand/mark.png'))
if ($source.Width -ne $source.Height) { $source.Dispose(); throw 'The source logo must be square.' }
$entries = @()
try {
    foreach ($dimension in @(16, 32, 48, 128, 256)) {
        $small = [Drawing.Bitmap]::new($dimension, $dimension)
        $resizer = [Drawing.Graphics]::FromImage($small)
        $resizer.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $resizer.DrawImage($source, 0, 0, $dimension, $dimension)
        $stream = [IO.MemoryStream]::new()
        $small.Save($stream, [Drawing.Imaging.ImageFormat]::Png)
        $bytes = $stream.ToArray()
        [IO.File]::WriteAllBytes((Join-Path (Get-Location) "extension/icons/strata-$dimension.png"), $bytes)
        $entries += @{ size = $dimension; bytes = $bytes }
        $stream.Dispose(); $resizer.Dispose(); $small.Dispose()
    }
} finally { $source.Dispose() }
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
