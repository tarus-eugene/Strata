#ifndef AppVersion
  #error AppVersion must be supplied by package-installer.ps1
#endif

[Setup]
AppId={{E859F42A-6241-4C1F-AFC8-D5B636CDCF83}
AppName=101 Downloader
AppVersion={#AppVersion}
AppPublisher=101 Downloader
DefaultDirName={localappdata}\Programs\101 Downloader
DefaultGroupName=101 Downloader
UsePreviousGroup=no
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19041
AppMutex=Local\Strata.DownloadManager
CloseApplications=no
RestartApplications=no
WizardStyle=modern
SetupIconFile={#PayloadDir}\strata.ico
UninstallDisplayIcon={app}\101Downloader.exe
LicenseFile={#PayloadDir}\LICENSE
InfoAfterFile=finish.txt
OutputDir={#OutputPath}
OutputBaseFilename=101-Downloader-Setup-{#AppVersion}-x64
Compression=lzma2
SolidCompression=yes
SetupLogging=yes

[Tasks]
Name: browserbridge; Description: "Connect the browser companion for this Windows account"
Name: desktopicon; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "{#PayloadDir}\strata.exe"; DestDir: "{app}"; DestName: "101Downloader.exe"; Flags: ignoreversion
Source: "{#PayloadDir}\strata-bridge.exe"; DestDir: "{app}"; DestName: "101Downloader-Bridge.exe"; Flags: ignoreversion
Source: "{#PayloadDir}\strata-cli.exe"; DestDir: "{app}"; DestName: "101Downloader-cli.exe"; Flags: ignoreversion
Source: "{#PayloadDir}\runtime\*.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\runtime\*.dll"; DestDir: "{app}\tools"; Flags: ignoreversion
Source: "{#PayloadDir}\tools\*"; DestDir: "{app}\tools"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#PayloadDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\NOTICE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\extension\*"; DestDir: "{app}\extension"; Flags: ignoreversion recursesubdirs createallsubdirs

[InstallDelete]
Type: files; Name: "{app}\strata.exe"
Type: files; Name: "{app}\strata-bridge.exe"
Type: files; Name: "{app}\strata-cli.exe"
Type: files; Name: "{userprograms}\Strata\Strata.lnk"
Type: files; Name: "{userprograms}\Strata\Browser companion setup.lnk"
Type: dirifempty; Name: "{userprograms}\Strata"
Type: files; Name: "{userdesktop}\Strata.lnk"

[Icons]
Name: "{group}\101 Downloader"; Filename: "{app}\101Downloader.exe"; WorkingDir: "{app}"
Name: "{group}\Browser companion setup"; Filename: "{app}\extension\setup.html"
Name: "{autodesktop}\101 Downloader"; Filename: "{app}\101Downloader.exe"; Tasks: desktopicon; WorkingDir: "{app}"

[Run]
Filename: "{app}\101Downloader.exe"; Description: "Launch 101 Downloader"; Flags: nowait postinstall skipifsilent
Filename: "{app}\extension\setup.html"; Description: "Set up the browser companion"; Flags: shellexec postinstall skipifsilent unchecked

[UninstallRun]
Filename: "{app}\101Downloader.exe"; Parameters: "--unregister-browser-host"; Flags: runhidden; RunOnceId: "DisconnectBrowserHost"

[UninstallDelete]
Type: files; Name: "{app}\browser\com.strata.download_manager.json"
Type: dirifempty; Name: "{app}\browser"

[Messages]
SetupAppRunningError=101 Downloader is running. Right-click its system tray icon and choose Quit 101 Downloader, then try again.
UninstallAppRunningError=101 Downloader is running. Right-click its system tray icon and choose Quit 101 Downloader, then try again.

[Code]
procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('browserbridge') then
    if not Exec(ExpandConstant('{app}\101Downloader.exe'), '--register-browser-host', '', SW_HIDE,
      ewWaitUntilTerminated, ResultCode) or (ResultCode <> 0) then
      RaiseException('The browser bridge could not be connected. Open 101 Downloader Settings and click Connect browsers to retry.');
end;
