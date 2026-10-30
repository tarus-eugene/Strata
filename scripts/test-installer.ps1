param([string]$SetupPath)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
if (-not $SetupPath) {
    $SetupPath = Get-ChildItem -Path (Join-Path $root 'dist/Strata-Setup-*-x64.exe') |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName
}
if (-not $SetupPath) { throw 'Build an installer first with scripts/package-installer.ps1.' }
$uninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{E859F42A-6241-4C1F-AFC8-D5B636CDCF83}_is1'
$existing = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($uninstallKey)
if ($existing) { $existing.Dispose(); throw 'An installed Strata already exists. Run this test in a clean Windows account.' }
try {
    $running = [Threading.Mutex]::OpenExisting('Local\Strata.DownloadManager')
    $running.Dispose()
    throw 'Quit Strata from the tray before running the installer test.'
} catch [Threading.WaitHandleCannotBeOpenedException] {}

$testRoot = Join-Path $root ('.tools/installer-test/' + [guid]::NewGuid().ToString('N'))
$destination = Join-Path $testRoot 'Strata with spaces'
$group = 'Strata'
$shortcut = Join-Path $env:APPDATA "Microsoft/Windows/Start Menu/Programs/$group/Strata.lnk"
if (Test-Path -LiteralPath $shortcut) { throw 'A Strata shortcut already exists. Run this test in a clean Windows account.' }
New-Item -ItemType Directory -Force -Path $testRoot | Out-Null
$retained = Join-Path $testRoot 'history-to-retain.json'
[IO.File]::WriteAllText($retained, '{"sentinel":"keep user data"}')
$saved = @{}
$roots = @(Get-Content (Join-Path $root 'resources/browser-registry.txt') | Where-Object { $_.Trim() })
$hostName = 'com.strata.download_manager'
foreach ($registryRoot in $roots) {
    $path = "$registryRoot\$hostName"
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($path)
    try { $saved[$path] = if ($key) { @{ Exists=$true; Value=$key.GetValue(''); Kind=$key.GetValueKind('') } } else { @{ Exists=$false } } }
    finally { if ($key) { $key.Dispose() } }
}
function Run-Setup([string]$Log, [bool]$ExpectSuccess = $true) {
    $arguments = '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /TASKS="browserbridge" /DIR="' + $destination + '" /GROUP="' + $group + '" /LOG="' + (Join-Path $testRoot $Log) + '"'
    $process = Start-Process -FilePath $SetupPath -ArgumentList $arguments -WindowStyle Hidden -PassThru -Wait
    if (($process.ExitCode -eq 0) -ne $ExpectSuccess) { throw "Unexpected installer exit $($process.ExitCode); see $Log" }
}
function Uninstall-Test {
    $uninstaller = Join-Path $destination 'unins000.exe'
    if (Test-Path -LiteralPath $uninstaller) {
        $process = Start-Process -FilePath $uninstaller -ArgumentList '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART' -WindowStyle Hidden -PassThru -Wait
        if ($process.ExitCode -ne 0) { throw "Uninstall failed: $($process.ExitCode)" }
    }
}
try {
    $mutex = [Threading.Mutex]::new($false, 'Local\Strata.DownloadManager')
    try { Run-Setup 'running-app.log' $false } finally { $mutex.Dispose() }
    Run-Setup 'install.log'
    foreach ($file in @('strata.exe','strata-bridge.exe','vcruntime140.dll','vcruntime140_1.dll','msvcp140.dll','extension/manifest.json','extension/background.js','extension/icons/strata-128.png','extension/setup.html','LICENSE','LICENSE-APACHE','NOTICE')) {
        if (-not (Test-Path -LiteralPath (Join-Path $destination $file))) { throw "Installer omitted $file" }
    }
    if (Test-Path -LiteralPath (Join-Path $destination 'extension/background.test.js')) { throw 'Developer tests leaked into the installer.' }
    if (-not (Test-Path -LiteralPath $shortcut)) { throw 'Start menu shortcut missing.' }
    $manifestPath = Join-Path $destination "browser/$hostName.json"
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    if ($manifest.path -ne (Join-Path $destination 'strata-bridge.exe')) { throw 'Installed bridge path is incorrect.' }
    $extensionId = (Get-Content (Join-Path $root 'extension/id.txt') -Raw).Trim()
    if ($manifest.allowed_origins.Count -ne 1 -or $manifest.allowed_origins[0] -ne "chrome-extension://$extensionId/") { throw 'Unexpected allowed extension origins.' }
    foreach ($registryRoot in $roots) {
        $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("$registryRoot\$hostName")
        try { if (-not $key -or $key.GetValue('') -ne $manifestPath) { throw "Missing native host registration: $registryRoot" } }
        finally { if ($key) { $key.Dispose() } }
    }
    Run-Setup 'upgrade.log'
    if ([IO.File]::ReadAllText($retained) -ne '{"sentinel":"keep user data"}') { throw 'Upgrade modified user data.' }
    $foreignPath = "$($roots[0])\$hostName"
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($foreignPath)
    try { $key.SetValue('', 'C:\another-strata-install\host.json') } finally { $key.Dispose() }
    Uninstall-Test
    if (Test-Path -LiteralPath (Join-Path $destination 'strata.exe')) { throw 'Uninstall left the app executable behind.' }
    if (Test-Path -LiteralPath $shortcut) { throw 'Uninstall left the shortcut behind.' }
    if ([IO.File]::ReadAllText($retained) -ne '{"sentinel":"keep user data"}') { throw 'Uninstall modified user data.' }
    foreach ($registryRoot in $roots) {
        $path = "$registryRoot\$hostName"
        $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($path)
        try {
            if ($path -eq $foreignPath) {
                if (-not $key -or $key.GetValue('') -ne 'C:\another-strata-install\host.json') { throw 'Uninstall removed another installation connection.' }
            } elseif ($key) { throw "Uninstall retained its own registry key: $path" }
        } finally { if ($key) { $key.Dispose() } }
    }
    Write-Output 'PASS: running-app protection, install, upgrade, runtime payload, browser registration, shortcuts, ownership-safe uninstall, and retained user data.'
} finally {
    try { Uninstall-Test } finally {
        foreach ($path in $saved.Keys) {
            if ($saved[$path].Exists) {
                $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($path)
                try { $key.SetValue('', $saved[$path].Value, $saved[$path].Kind) } finally { $key.Dispose() }
            } else { [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKey($path, $false) }
        }
    }
}
