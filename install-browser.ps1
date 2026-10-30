[CmdletBinding(SupportsShouldProcess)]
param(
    [string]$AppDirectory = (Join-Path $PSScriptRoot 'target/debug'),
    [switch]$Uninstall
)
$ErrorActionPreference = 'Stop'
$hostName = 'com.strata.download_manager'
$registryRoots = @(Get-Content (Join-Path $PSScriptRoot 'resources/browser-registry.txt') | Where-Object { $_.Trim() })
if ($Uninstall) {
    if ($PSCmdlet.ShouldProcess('101 Downloader browser connections for this Windows account', 'Unregister')) {
        foreach ($root in $registryRoots) { [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKey("$root\$hostName", $false) }
        Write-Output '101 Downloader disconnected from browsers. App data and browser downloads were retained.'
    }
    return
}
$appRoot = (Resolve-Path -LiteralPath $AppDirectory).Path
$bridge = Join-Path $appRoot 'strata-bridge.exe'
if (Test-Path -LiteralPath (Join-Path $appRoot '101Downloader-Bridge.exe')) { $bridge = Join-Path $appRoot '101Downloader-Bridge.exe' }
if (-not (Test-Path -LiteralPath $bridge)) { throw 'Build the app and bridge first: .\run.ps1 build' }
if (-not ((Test-Path -LiteralPath (Join-Path $appRoot '101Downloader.exe')) -or (Test-Path -LiteralPath (Join-Path $appRoot 'strata.exe')))) { throw 'The desktop app must be beside the browser bridge.' }
$extensionId = (Get-Content (Join-Path $PSScriptRoot 'extension/id.txt') -Raw).Trim()
if ($extensionId -notmatch '^[a-p]{32}$') { throw 'Invalid extension ID.' }
$manifestDirectory = Join-Path $appRoot 'browser'
$manifestPath = Join-Path $manifestDirectory "$hostName.json"
$manifest = [ordered]@{
    name = $hostName
    description = '101 Downloader local download handoff'
    path = $bridge
    type = 'stdio'
    allowed_origins = @("chrome-extension://$extensionId/")
}
if ($PSCmdlet.ShouldProcess($manifestPath, 'Register the 101 Downloader native host for Chromium browsers')) {
    New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null
    $json = $manifest | ConvertTo-Json -Depth 4
    [IO.File]::WriteAllText($manifestPath, $json, [Text.UTF8Encoding]::new($false))
    foreach ($root in $registryRoots) {
        $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("$root\$hostName")
        try { $key.SetValue('', $manifestPath, [Microsoft.Win32.RegistryValueKind]::String) }
        finally { $key.Dispose() }
    }
    Write-Output "Browser bridge registered for extension $extensionId."
    Write-Output "Load unpacked extension folder: $(Join-Path $PSScriptRoot 'extension')"
    Write-Output 'Open your browser extensions page, enable Developer mode, choose Load unpacked, and select that folder.'
}
