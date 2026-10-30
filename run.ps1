param(
    [ValidateSet('run', 'build', 'test', 'check', 'fmt', 'clippy')]
    [string]$Task = 'run',
    [switch]$Release
)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
if (Test-Path '.tools/cargo/bin/cargo.exe') {
    $env:CARGO_HOME = Join-Path $PSScriptRoot '.tools/cargo'
    $env:RUSTUP_HOME = Join-Path $PSScriptRoot '.tools/rustup'
    $env:PATH = "$env:CARGO_HOME/bin;$env:PATH"
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'Install Rust from https://rustup.rs and the Visual Studio C++ build tools, then run this script again.'
}
$vswhere = 'C:/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe'
if (Test-Path $vswhere) {
    $vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($vs) {
        $dev = Join-Path $vs 'Common7/Tools/VsDevCmd.bat'
        New-Item -ItemType Directory -Force '.tools' | Out-Null
        Set-Content '.tools/dev-env.cmd' -Value "@echo off`r`ncall `"$dev`" -arch=x64 -host_arch=x64 >nul`r`nset" -Encoding ascii
        & cmd.exe /d /c .tools\dev-env.cmd | ForEach-Object {
            $pair = $_ -split '=', 2
            if ($pair.Length -eq 2 -and $pair[0] -notmatch '^=') { [Environment]::SetEnvironmentVariable($pair[0], $pair[1], 'Process') }
        }
    }
}
$cargoArgs = @($Task)
if ($Release -and $Task -in @('run', 'build', 'test', 'check', 'clippy')) { $cargoArgs += '--release' }
& cargo @cargoArgs
exit $LASTEXITCODE
