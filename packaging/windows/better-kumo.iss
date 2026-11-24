; Needs Inno Setup 6.5+ (Ukrainian.isl).

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef BinDir
  #define BinDir "..\..\core\target\release"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

#define AppGuid "8F2B6C1E-4D7A-4B3E-9C51-2E6A0D9B7F43"
#define AppName "Better Kumo"
#define AppExe "Better Kumo.exe"
#define RunKey "Software\Microsoft\Windows\CurrentVersion\Run"
#define StartupApprovedKey "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"
; The name Better Kumo's own "Start at login" uses (auto-launch app name), so both switch the same entry.
#define RunValue "better-kumo"

[Setup]
AppId={{{#AppGuid}}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=Igor Kravtsov
AppPublisherURL=https://github.com/IgorKravtsov/better-kumo-app
AppSupportURL=https://github.com/IgorKravtsov/better-kumo-app/blob/main/docs/install.md
AppUpdatesURL=https://github.com/IgorKravtsov/better-kumo-app/releases/latest
DefaultDirName={autopf}\{#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
WizardStyle=modern
SetupIconFile=better-kumo.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
OutputDir={#OutputDir}
OutputBaseFilename=Better-Kumo-Windows-x64-{#AppVersion}-Setup
Compression=lzma2/max
SolidCompression=yes
CloseApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"
Name: "ukrainian"; MessagesFile: "compiler:Languages\Ukrainian.isl"

[CustomMessages]
english.StartAtLogin=Start {#AppName} when I sign in to Windows
russian.StartAtLogin=Запускать {#AppName} при входе в Windows
ukrainian.StartAtLogin=Запускати {#AppName} під час входу в Windows
english.StartupGroup=Startup:
russian.StartupGroup=Автозапуск:
ukrainian.StartupGroup=Автозапуск:

[Tasks]
; Only offered on the first install: on an update the app's own "Start at login" choice stands.
Name: "startup"; Description: "{cm:StartAtLogin}"; GroupDescription: "{cm:StartupGroup}"; Check: IsFirstInstall
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#BinDir}\better-kumo.exe"; DestDir: "{app}"; DestName: "{#AppExe}"; Flags: ignoreversion
Source: "{#BinDir}\kumo-mcp.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "{#RunKey}"; ValueType: string; ValueName: "{#RunValue}"; ValueData: """{app}\{#AppExe}"" "; Tasks: startup
; Re-enables the entry if it was turned off in Task Manager's Startup apps.
Root: HKCU; Subkey: "{#StartupApprovedKey}"; ValueType: binary; ValueName: "{#RunValue}"; ValueData: "02 00 00 00 00 00 00 00 00 00 00 00"; Tasks: startup
Root: HKCU; Subkey: "{#RunKey}"; ValueType: none; ValueName: "{#RunValue}"; Flags: uninsdeletevalue
Root: HKCU; Subkey: "{#StartupApprovedKey}"; ValueType: none; ValueName: "{#RunValue}"; Flags: uninsdeletevalue dontcreatekey

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
Filename: "{app}\{#AppExe}"; Flags: nowait; Check: RelaunchAfterSilentUpdate

[Code]
function IsFirstInstall: Boolean;
begin
  Result := not RegKeyExists(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{{#AppGuid}}_is1');
end;

var
  WasRunning: Boolean;

// The tray app has no window for Restart Manager to close, and the uninstaller doesn't use it.
// kumo-mcp.exe may be held by an AI agent, which starts it again when needed.
function StopBetterKumo: Boolean;
var
  Code, McpCode: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM kumo-mcp.exe', '', SW_HIDE, ewWaitUntilTerminated, McpCode);
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM "{#AppExe}"', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Result := Code = 0;
  if Result or (McpCode = 0) then
    Sleep(1000);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  WasRunning := StopBetterKumo;
  Result := '';
end;

function RelaunchAfterSilentUpdate: Boolean;
begin
  Result := WizardSilent and WasRunning;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    StopBetterKumo;
end;
