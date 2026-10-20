; Voi's Windows setup: one Voi-<version>.exe that installs Voi for the person
; running it (no administrator), puts it in the Start menu with its icon,
; optionally starts it at sign-in, and can be removed from Settings > Apps.
;
; Built by scripts\package-windows.ps1 from the folder it has already
; assembled and scanned for keys — never from a working tree:
;
;   ISCC.exe /DAppVersion=0.1.0.65 /DSource=<staged Voi folder> /DOutputDir=<dir> scripts\voi.iss
;
; Settings, the Soniox key and transcripts live in %APPDATA%\voi, outside
; the install folder: an update keeps them, and so does removing Voi.

#ifndef AppVersion
  #error Pass /DAppVersion=x.y.z
#endif
#ifndef Source
  #error Pass /DSource=<folder holding Voi.exe, cli\voi.exe and README.txt>
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

[Setup]
; Never change: Windows knows an installed Voi, for updates and removal, by it.
AppId={{6F1B7C1E-4E0A-4C8E-9D7A-2B9F3C5D7A11}
AppName=Voi
AppVersion={#AppVersion}
AppVerName=Voi {#AppVersion}
AppPublisher=Voi
DefaultDirName={autopf}\Voi
DefaultGroupName=Voi
DisableProgramGroupPage=yes
DisableDirPage=yes
DisableReadyPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
SetupIconFile=..\assets\voi.ico
UninstallDisplayIcon={app}\Voi.exe
UninstallDisplayName=Voi
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
OutputDir={#OutputDir}
OutputBaseFilename=Voi-{#AppVersion}
; A running Voi holds Voi.exe open: [Code] below stops it first.
CloseApplications=no
ShowLanguageDialog=auto
#ifdef Sign
; package-windows.ps1 defines the voisign tool when a certificate is given;
; the uninstaller is signed as well.
SignTool=voisign
SignedUninstaller=yes
#endif

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "uk"; MessagesFile: "compiler:Languages\Ukrainian.isl"

[CustomMessages]
en.AtSignIn=Start Voi when I sign in
uk.AtSignIn=Запускати Voi під час входу в систему
en.StartNow=Start Voi now
uk.StartNow=Запустити Voi зараз

[Tasks]
Name: "atsignin"; Description: "{cm:AtSignIn}"

[Files]
Source: "{#Source}\Voi.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Source}\cli\voi.exe"; DestDir: "{app}\cli"; Flags: ignoreversion
Source: "{#Source}\README.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Voi"; Filename: "{app}\Voi.exe"; IconFilename: "{app}\Voi.exe"

[Run]
; The same switch Settings uses, so the two never disagree.
Filename: "{app}\Voi.exe"; Parameters: "--launch-at-login on"; Tasks: atsignin; Flags: runhidden waituntilterminated
Filename: "{app}\Voi.exe"; Description: "{cm:StartNow}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{app}\Voi.exe"; Parameters: "--launch-at-login off"; RunOnceId: "AtSignInOff"; Flags: runhidden waituntilterminated

[UninstallDelete]
; What Voi.exe notes beside its own Start menu entry.
Type: files; Name: "{autoprograms}\Voi.lnk.target"

[Code]
// Voi runs in the notification area with no window to close, so Windows'
// Restart Manager does not stop it: stop it here, before files are replaced
// or removed. What it had to keep is already saved.
procedure StopVoi();
var
  Code: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM Voi.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Sleep(500);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopVoi();
  Result := '';
end;

function InitializeUninstall(): Boolean;
begin
  StopVoi();
  Result := True;
end;
