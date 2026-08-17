; Aura Browser – Installer (Inno Setup 6)
;
; Installiert pro Benutzer nach %LOCALAPPDATA%\Programs\Aura Browser. Das ist
; Absicht: der eingebaute Auto-Updater tauscht Dateien im Installationsordner,
; und dafuer darf kein Administrator noetig sein.
;
; Bauen:  iscc installer\aura.iss  (Ausgabe landet in dist\)

#define AppName      "Aura Browser"
#define AppExe       "aura-browser.exe"
#define Publisher    "Edgar Kessler"
#define AppUrl       "https://github.com/edgar-kessler/aura-browser"
#ifndef AppVersion
  #define AppVersion "0.1.9"
#endif
#ifndef SourceDir
  #define SourceDir  "..\dist\payload"
#endif

[Setup]
AppId={{8E4B1C6A-2F71-4B2E-9E1C-3A9D6F0B7C51}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#Publisher}
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}/issues
AppUpdatesURL={#AppUrl}/releases
VersionInfoVersion={#AppVersion}

; Pro Benutzer, ohne Adminrechte – Voraussetzung fuers Selbst-Update.
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\Aura Browser
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
AllowNoIcons=yes
UninstallDisplayName={#AppName}
UninstallDisplayIcon={app}\{#AppExe}

OutputDir=..\dist
OutputBaseFilename=AuraBrowserSetup-{#AppVersion}
SetupIconFile=..\assets\aura.ico
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=force
RestartApplications=no

[Languages]
Name: "de"; MessagesFile: "compiler:Languages\German.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "taskbar"; Description: "An die Taskleiste anheften"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "defaultbrowser"; Description: "Aura bei Windows als Browser anmelden"; GroupDescription: "Verknuepfungen mit dem System"

[Files]
Source: "{#SourceDir}\{#AppExe}";          DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\WebView2Loader.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\assets\*";           DestDir: "{app}\assets"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#SourceDir}\LICENSE";            DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\README.md";          DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}";           Filename: "{app}\{#AppExe}"; IconFilename: "{app}\{#AppExe}"
Name: "{group}\{#AppName} entfernen"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}";     Filename: "{app}\{#AppExe}"; IconFilename: "{app}\{#AppExe}"; Tasks: desktopicon
Name: "{userappdata}\Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar\{#AppName}"; \
      Filename: "{app}\{#AppExe}"; Tasks: taskbar

[Registry]
; Damit Windows Aura unter "Standard-Apps" anbietet.
Root: HKCU; Subkey: "Software\Classes\AuraHTML"; ValueType: string; ValueName: ""; ValueData: "Aura HTML-Dokument"; Flags: uninsdeletekey; Tasks: defaultbrowser
Root: HKCU; Subkey: "Software\Classes\AuraHTML\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe},0"; Tasks: defaultbrowser
Root: HKCU; Subkey: "Software\Classes\AuraHTML\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: defaultbrowser

Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\AuraBrowser"; ValueType: string; ValueName: ""; ValueData: "{#AppName}"; Flags: uninsdeletekey; Tasks: defaultbrowser
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\AuraBrowser\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe},0"; Tasks: defaultbrowser
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\AuraBrowser\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"""; Tasks: defaultbrowser
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\AuraBrowser\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "{#AppName}"; Tasks: defaultbrowser
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\AuraBrowser\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "Nativer Windows-Browser mit eingebautem Werbeblocker"; Tasks: defaultbrowser
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\AuraBrowser\Capabilities"; ValueType: string; ValueName: "ApplicationIcon"; ValueData: "{app}\{#AppExe},0"; Tasks: defaultbrowser
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\AuraBrowser\Capabilities\URLAssociations"; ValueType: string; ValueName: "http"; ValueData: "AuraHTML"; Tasks: defaultbrowser
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\AuraBrowser\Capabilities\URLAssociations"; ValueType: string; ValueName: "https"; ValueData: "AuraHTML"; Tasks: defaultbrowser
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\AuraBrowser\Capabilities\FileAssociations"; ValueType: string; ValueName: ".html"; ValueData: "AuraHTML"; Tasks: defaultbrowser
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\AuraBrowser\Capabilities\FileAssociations"; ValueType: string; ValueName: ".htm"; ValueData: "AuraHTML"; Tasks: defaultbrowser
Root: HKCU; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "{#AppName}"; ValueData: "Software\Clients\StartMenuInternet\AuraBrowser\Capabilities"; Flags: uninsdeletevalue; Tasks: defaultbrowser

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Der Reste-Ordner des Auto-Updaters.
Type: filesandordirs; Name: "{app}\aura-browser.old.exe"

[Code]
{ Ohne WebView2-Laufzeit rendert nichts. Auf Windows 11 ist sie da, auf
  aelteren Systemen nicht immer – dann wird der offizielle Installer geholt. }
function WebView2Installed: Boolean;
var
  Value: string;
begin
  Result :=
    RegQueryStringValue(HKLM, 'SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', Value) or
    RegQueryStringValue(HKLM, 'SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', Value) or
    RegQueryStringValue(HKCU, 'SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', Value);
  if Result then
    Result := (Value <> '') and (Value <> '0.0.0.0');
end;

procedure InstallWebView2;
var
  TempFile: string;
  ResultCode: Integer;
begin
  TempFile := ExpandConstant('{tmp}\MicrosoftEdgeWebview2Setup.exe');
  WizardForm.StatusLabel.Caption := 'Lade die WebView2-Laufzeit …';
  if not DownloadTemporaryFile('https://go.microsoft.com/fwlink/p/?LinkId=2124703',
        'MicrosoftEdgeWebview2Setup.exe', '', nil) = 0 then
    ;
  if FileExists(TempFile) then
    Exec(TempFile, '/silent /install', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  if not WebView2Installed then
  begin
    if MsgBox('Aura braucht die WebView2-Laufzeit von Microsoft. Jetzt herunterladen und installieren?',
              mbConfirmation, MB_YESNO) = IDYES then
      InstallWebView2
    else
      Result := 'Ohne die WebView2-Laufzeit kann Aura keine Seiten anzeigen.';
  end;
end;
