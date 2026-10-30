param(
    [switch]$SkipBuild,
    [string]$CompilerPath,
    [string]$RuntimeDirectory
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
if (-not $SkipBuild) {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root 'run.ps1') build -Release
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
}
if (-not $CompilerPath) {
    $onPath = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    $candidates = @(
        $(if ($onPath) { $onPath.Source }),
        (Join-Path $root '.tools/inno/ISCC.exe'),
        'C:/Program Files/Inno Setup 7/ISCC.exe',
        'C:/Program Files (x86)/Inno Setup 7/ISCC.exe',
        'C:/Program Files (x86)/Inno Setup 6/ISCC.exe'
    )
    $CompilerPath = $candidates | Where-Object { $_ -and (Test-Path -LiteralPath $_) } | Select-Object -First 1
}
if (-not $CompilerPath -or -not (Test-Path -LiteralPath $CompilerPath)) {
    throw 'Install Inno Setup 6.3 or newer from https://jrsoftware.org/isdl.php, or pass -CompilerPath to ISCC.exe.'
}
if (-not $RuntimeDirectory) {
    $vswhere = 'C:/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe'
    if (Test-Path -LiteralPath $vswhere) {
        $vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        if ($vs) {
            $redist = Join-Path $vs 'VC/Redist/MSVC'
            $RuntimeDirectory = Get-ChildItem -LiteralPath $redist -Directory | Where-Object { $_.Name -match '^\d+\.' } | Sort-Object { [version]$_.Name } -Descending |
                ForEach-Object { Get-ChildItem -Path (Join-Path $_.FullName 'x64/Microsoft.VC*.CRT') -Directory } |
                Select-Object -First 1 -ExpandProperty FullName
        }
    }
}
if (-not $RuntimeDirectory -or -not (Test-Path -LiteralPath (Join-Path $RuntimeDirectory 'vcruntime140.dll'))) {
    throw 'The x64 Visual C++ redistributable files are missing. Install the C++ build tools or pass -RuntimeDirectory to Microsoft.VC*.CRT.'
}
$version = ([regex]::Match([IO.File]::ReadAllText((Join-Path $root 'Cargo.toml')), '(?m)^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"')).Groups[1].Value
if (-not $version) { throw 'Cannot read the package version from Cargo.toml.' }
$extensionVersion = (Get-Content -LiteralPath (Join-Path $root 'extension/manifest.json') -Raw | ConvertFrom-Json).version
if ($extensionVersion -ne $version) { throw 'The extension and app versions must match before packaging.' }

$stage = Join-Path $root ('.tools/installer-payload/' + [guid]::NewGuid().ToString('N'))
$runtime = Join-Path $stage 'runtime'
New-Item -ItemType Directory -Force -Path $runtime | Out-Null
foreach ($name in @('strata.exe','strata-bridge.exe','strata-cli.exe')) {
    Copy-Item -LiteralPath (Join-Path $root "target/release/$name") -Destination $stage
}
foreach ($name in @('LICENSE','LICENSE-APACHE','NOTICE')) {
    Copy-Item -LiteralPath (Join-Path $root $name) -Destination $stage
}
Copy-Item -LiteralPath (Join-Path $root 'assets/brand/strata.ico') -Destination $stage
Get-ChildItem -LiteralPath $RuntimeDirectory -Filter '*.dll' -File | Copy-Item -Destination $runtime
& (Join-Path $PSScriptRoot 'fetch-media-tools.ps1') -Destination (Join-Path $stage 'tools') | Out-Host
& (Join-Path $PSScriptRoot 'package-extension.ps1') | Out-Host
Add-Type -AssemblyName System.IO.Compression.FileSystem
[IO.Compression.ZipFile]::ExtractToDirectory((Join-Path $root 'dist/101-downloader-browser-extension.zip'), (Join-Path $stage 'extension'))
$output = Join-Path $root 'dist'
& $CompilerPath "/DAppVersion=$version" "/DPayloadDir=$stage" "/DOutputPath=$output" (Join-Path $root 'installer/strata.iss')
if ($LASTEXITCODE -ne 0) { throw 'Installer compilation failed.' }
$installer = Join-Path $output "101-Downloader-Setup-$version-x64.exe"
$hash = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()
[IO.File]::WriteAllText("$installer.sha256", "$hash  $([IO.Path]::GetFileName($installer))`n", [Text.Encoding]::ASCII)
Write-Output "Installer: $installer"
Write-Output "SHA256: $hash"
