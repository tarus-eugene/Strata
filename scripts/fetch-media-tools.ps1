param([string]$Destination = (Join-Path (Split-Path $PSScriptRoot) '.tools/media-tools'))
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$root = Split-Path $PSScriptRoot
$cache = Join-Path $root '.tools/media-downloads'
New-Item -ItemType Directory -Force -Path $cache, $Destination | Out-Null
$tools = Get-Content (Join-Path $root 'resources/media-tools.json') -Raw | ConvertFrom-Json
Add-Type -AssemblyName System.IO.Compression.FileSystem
$provenance = @('# Strata media helpers', '', 'Bundled helpers run as separate processes. They retain their upstream licenses.', '')
function Download-Asset([string]$Url, [string]$Path, [long]$Length) {
    $Url += '?strata=range-download'
    if ($length -le 0 -or $length -gt 512MB) { throw 'Unexpected media archive size.' }
    $file = [IO.File]::Create($Path)
    try {
        for ($offset = 0L; $offset -lt $length; $offset += $received) {
            $end = [long][Math]::Min($offset + 512KB - 1, $length - 1)
            $request = [Net.HttpWebRequest]::Create($Url); $request.AddRange([long]$offset, [long]$end); $request.Timeout = 30000
            $request.KeepAlive = $false; $request.ReadWriteTimeout = 30000
            if ($request.Headers['Range'] -ne "bytes=$offset-$end") { throw 'Invalid archive range request.' }
            $response = $request.GetResponse()
            try {
                if ([int]$response.StatusCode -ne 206 -or $response.Headers['Content-Range'] -ne "bytes $offset-$end/$length") { throw 'The download server returned an unexpected byte range.' }
                $stream = $response.GetResponseStream(); $stream.ReadTimeout = 30000
                $buffer = New-Object byte[] (128KB); $received = 0L
                while ($received -lt ($end - $offset + 1)) {
                    $count = $stream.Read($buffer,0,[int][Math]::Min($buffer.Length,$end-$offset+1-$received))
                    if ($count -eq 0) { break }
                    $received += $count
                    if ($received -gt ($end - $offset + 1)) { throw 'The archive response exceeded its assigned range.' }
                    $file.Write($buffer,0,$count)
                }
                if ($received -ne ($end - $offset + 1)) { throw 'The archive response ended early.' }
                if (($end + 1) % 8MB -eq 0 -or $end + 1 -eq $length) { Write-Output ('  {0:N1}/{1:N1} MiB' -f (($end + 1)/1MB),($length/1MB)) }
            } finally { $response.Dispose() }
        }
    } finally { $file.Dispose() }
}
foreach ($tool in $tools) {
    $extension = [IO.Path]::GetExtension($tool.url)
    $archive = Join-Path $cache ($tool.name + '-' + $tool.sha256 + $extension)
    if (-not (Test-Path -LiteralPath $archive) -or (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $tool.sha256) {
        Write-Output "Downloading $($tool.name) $($tool.version)..."
        Download-Asset $tool.url "$archive.part" $tool.size
        if ((Get-FileHash -LiteralPath "$archive.part" -Algorithm SHA256).Hash.ToLowerInvariant() -ne $tool.sha256) { throw "Checksum mismatch: $($tool.name). Update resources/media-tools.json from an official release before rebuilding." }
        Move-Item -LiteralPath "$archive.part" -Destination $archive -Force
    }
    if ($extension -eq '.exe') { Copy-Item -LiteralPath $archive -Destination (Join-Path $Destination "$($tool.name).exe") -Force }
    else {
        $unpacked = Join-Path $cache ([guid]::NewGuid().ToString('N'))
        [IO.Compression.ZipFile]::ExtractToDirectory($archive, $unpacked)
        Get-ChildItem -LiteralPath $unpacked -File -Recurse | Where-Object { ($_.Extension -in @('.exe','.dll')) -and $_.Name -ne 'ffplay.exe' } | Copy-Item -Destination $Destination -Force
        $licenses = Join-Path $Destination "licenses/$($tool.name)"
        New-Item -ItemType Directory -Force -Path $licenses | Out-Null
        Get-ChildItem -LiteralPath $unpacked -File -Recurse | Where-Object { $_.Name -match '^(LICENSE|COPYING|README)' } | Copy-Item -Destination $licenses -Force
    }
    if ($tool.license) {
        $license = Join-Path $Destination "licenses/$($tool.name).txt"
        New-Item -ItemType Directory -Force -Path (Split-Path $license) | Out-Null
        Invoke-WebRequest -UseBasicParsing -Uri $tool.license -OutFile $license -TimeoutSec 60
    }
    $provenance += @("## $($tool.name) $($tool.version)", "Binary: $($tool.url)", "SHA-256: $($tool.sha256)", "Corresponding source and build instructions: $($tool.source)", '')
}
Copy-Item -LiteralPath (Join-Path $root 'resources/media-tools.json') -Destination $Destination -Force
[IO.File]::WriteAllLines((Join-Path $Destination 'README.md'), $provenance, [Text.UTF8Encoding]::new($false))
foreach ($name in @('yt-dlp.exe','deno.exe','ffmpeg.exe','ffprobe.exe')) {
    if (-not (Test-Path -LiteralPath (Join-Path $Destination $name))) { throw "Missing media helper: $name" }
}
Write-Output "Verified media helpers: $Destination"
