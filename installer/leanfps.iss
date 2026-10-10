; LeanFPS installer (Inno Setup 6). CI builds it with:
;   ISCC.exe /DAppVersion=<version> installer\leanfps.iss
; Output: target\installer\LeanFPS-Setup-<version>.exe

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{6C0B6E8C-2F1B-4D5E-9A1F-4C1E5F0A7B21}
AppName=LeanFPS
AppVersion={#AppVersion}
AppPublisher=LeanFPS
AppPublisherURL=https://leanfps.com/
DefaultDirName={autopf}\LeanFPS
DefaultGroupName=LeanFPS
DisableProgramGroupPage=yes
DisableDirPage=auto
; LeanFPS itself needs administrator rights, so install for all users.
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=..\target\installer
OutputBaseFilename=LeanFPS-Setup-{#AppVersion}
SetupIconFile=..\res\leanfps.ico
UninstallDisplayIcon={app}\leanfps.exe
UninstallDisplayName=LeanFPS
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Close a running LeanFPS (window or tray) before files are replaced or removed.
AppMutex=Local\LeanFPS.SingleInstance
CloseApplications=yes

[Tasks]
; Checked by default: most people expect a desktop icon.
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "..\target\release\leanfps.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\LeanFPS"; Filename: "{app}\leanfps.exe"; WorkingDir: "{app}"
Name: "{autodesktop}\LeanFPS"; Filename: "{app}\leanfps.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\leanfps.exe"; Description: "{cm:LaunchProgram,LeanFPS}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Put every changed Windows setting back before the program is removed. The saved previous
; values in %ProgramData%\LeanFPS are kept, so nothing the user had is lost.
Filename: "{app}\leanfps.exe"; Parameters: "--restore-all"; Flags: runhidden waituntilterminated; RunOnceId: "RestoreAll"
