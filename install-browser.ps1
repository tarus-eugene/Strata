[CmdletBinding(SupportsShouldProcess)]
param(
    [string]$AppDirectory = (Join-Path $PSScriptRoot 'target/debug'),
    [switch]$Uninstall
)
$ErrorActionPreference = 'Stop'
$hostName = 'com.strata.download_manager'
$keyPath = "Software\Google\Chrome\NativeMessagingHosts\$hostName"
if ($Uninstall) {
    if ($PSCmdlet.ShouldProcess('Strata Chrome connection for this Windows account', 'Unregister')) {
        [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKey($keyPath, $false)
        Write-Output 'Strata disconnected from Chrome. App data and browser downloads were retained.'
    }
    return
}
$appRoot = (Resolve-Path -LiteralPath $AppDirectory).Path
$bridge = Join-Path $appRoot 'strata-bridge.exe'
if (-not (Test-Path -LiteralPath $bridge)) { throw 'Build the app and bridge first: .\run.ps1 build' }
if (-not (Test-Path -LiteralPath (Join-Path $appRoot 'strata.exe'))) { throw 'strata.exe must be beside strata-bridge.exe.' }
$extensionId = (Get-Content (Join-Path $PSScriptRoot 'extension/id.txt') -Raw).Trim()
if ($extensionId -notmatch '^[a-p]{32}$') { throw 'Invalid extension ID.' }
$manifestDirectory = Join-Path $env:LOCALAPPDATA 'Strata/browser'
$manifestPath = Join-Path $manifestDirectory "$hostName.json"
$manifest = [ordered]@{
    name = $hostName
    description = 'Strata local download handoff'
    path = $bridge
    type = 'stdio'
    allowed_origins = @("chrome-extension://$extensionId/")
}
if ($PSCmdlet.ShouldProcess($manifestPath, 'Register the Strata native host for Chrome')) {
    New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null
    $json = $manifest | ConvertTo-Json -Depth 4
    [IO.File]::WriteAllText($manifestPath, $json, [Text.UTF8Encoding]::new($false))
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($keyPath)
    try { $key.SetValue('', $manifestPath, [Microsoft.Win32.RegistryValueKind]::String) }
    finally { $key.Dispose() }
    Write-Output "Chrome bridge registered for extension $extensionId."
    Write-Output "Load unpacked extension folder: $(Join-Path $PSScriptRoot 'extension')"
    Write-Output 'Open chrome://extensions, enable Developer mode, choose Load unpacked, and select that folder.'
}
