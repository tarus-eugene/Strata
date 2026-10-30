$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
$extension = Join-Path $root 'extension'
$output = Join-Path $root 'dist'
New-Item -ItemType Directory -Force -Path $output | Out-Null
$files = @('manifest.json','background.js','popup.html','popup.css','popup.js','setup.html','setup.css','id.txt','README.md','icons')
$paths = @($files | ForEach-Object {
    $path = Join-Path $extension $_
    if (-not (Test-Path -LiteralPath $path)) { throw "Missing extension file: $_" }
    $path
})
$archive = Join-Path $output '101-downloader-browser-extension.zip'
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$stream = [IO.File]::Open($archive, [IO.FileMode]::Create)
$zip = [IO.Compression.ZipArchive]::new($stream, [IO.Compression.ZipArchiveMode]::Create)
try {
    foreach ($path in $paths) {
        $entryFiles = if (Test-Path -LiteralPath $path -PathType Container) { Get-ChildItem -LiteralPath $path -File -Recurse } else { Get-Item -LiteralPath $path }
        foreach ($file in $entryFiles) {
            $relative = $file.FullName.Substring($extension.Length + 1).Replace('\', '/')
            [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $file.FullName, $relative) | Out-Null
        }
    }
} finally { $zip.Dispose(); $stream.Dispose() }
Write-Output $archive
