#ifndef AppVersion
  #error AppVersion must be supplied by package-installer.ps1
#endif

[Setup]
AppId={{E859F42A-6241-4C1F-AFC8-D5B636CDCF83}
AppName=Strata
AppVersion={#AppVersion}
AppPublisher=Strata
DefaultDirName={localappdata}\Programs\Strata
DefaultGroupName=Strata
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
UninstallDisplayIcon={app}\strata.exe
LicenseFile={#PayloadDir}\LICENSE
InfoAfterFile=finish.txt
OutputDir={#OutputPath}
OutputBaseFilename=Strata-Setup-{#AppVersion}-x64
Compression=lzma2
SolidCompression=yes
SetupLogging=yes

[Tasks]
Name: browserbridge; Description: "Connect the browser companion for this Windows account"
Name: desktopicon; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "{#PayloadDir}\strata.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\strata-bridge.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\strata-cli.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\runtime\*.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\runtime\*.dll"; DestDir: "{app}\tools"; Flags: ignoreversion
Source: "{#PayloadDir}\tools\*"; DestDir: "{app}\tools"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#PayloadDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\NOTICE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\extension\*"; DestDir: "{app}\extension"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\Strata"; Filename: "{app}\strata.exe"; WorkingDir: "{app}"
Name: "{group}\Browser companion setup"; Filename: "{app}\extension\setup.html"
Name: "{autodesktop}\Strata"; Filename: "{app}\strata.exe"; Tasks: desktopicon; WorkingDir: "{app}"

[Run]
Filename: "{app}\strata.exe"; Description: "Launch Strata"; Flags: nowait postinstall skipifsilent
Filename: "{app}\extension\setup.html"; Description: "Set up the browser companion"; Flags: shellexec postinstall skipifsilent unchecked

[UninstallRun]
Filename: "{app}\strata.exe"; Parameters: "--unregister-browser-host"; Flags: runhidden; RunOnceId: "DisconnectBrowserHost"

[UninstallDelete]
Type: files; Name: "{app}\browser\com.strata.download_manager.json"
Type: dirifempty; Name: "{app}\browser"

[Messages]
SetupAppRunningError=Strata is running. Right-click its system tray icon and choose Quit Strata, then try again.
UninstallAppRunningError=Strata is running. Right-click its system tray icon and choose Quit Strata, then try again.

[Code]
procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('browserbridge') then
    if not Exec(ExpandConstant('{app}\strata.exe'), '--register-browser-host', '', SW_HIDE,
      ewWaitUntilTerminated, ResultCode) or (ResultCode <> 0) then
      RaiseException('The browser bridge could not be connected. Open Strata Settings and click Connect browsers to retry.');
end;
