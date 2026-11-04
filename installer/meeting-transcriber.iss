; The Windows installer, built by scripts/package-windows.ps1 -Installer with
; Inno Setup 6 (ISCC). It installs for the current user, without
; administrator rights, into the same folder as the app's own Install button
; (src/platform/windows/install.rs), so either way the program, its Start
; menu shortcut and a downloaded CUDA add-on (cuda\) are in one place.
;
; Defines passed by the script:
;   AppVersion  the version in Cargo.toml
;   SourceDir   the staged package: meeting-transcriber.exe, vulkan\, README.txt
;   OutputDir   where Meeting-Transcriber-Setup-<Arch>.exe goes
;   Arch        x86_64 or arm64

#ifndef AppVersion
  #error AppVersion is not defined
#endif

[Setup]
; Keeps upgrades and the uninstall entry together across versions: never change.
AppId={{B0BB3C99-C30D-4633-9F06-0B52508925F7}
AppName=Meeting Transcriber
AppVersion={#AppVersion}
AppVerName=Meeting Transcriber {#AppVersion}
AppPublisher=Patrick Sun
AppPublisherURL=https://github.com/PatrickSun93/meeting-transcriber
AppSupportURL=https://github.com/PatrickSun93/meeting-transcriber/issues
DefaultDirName={localappdata}\Programs\Meeting Transcriber
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir={#OutputDir}
OutputBaseFilename=Meeting-Transcriber-Setup-{#Arch}
UninstallDisplayIcon={app}\meeting-transcriber.exe
UninstallDisplayName=Meeting Transcriber
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
#if Arch == "x86_64"
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#else
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#endif
; A running copy (also one closed to the notification area) is asked to
; close before its files are replaced.
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: desktopicon; Description: "Create a shortcut on the &desktop"; GroupDescription: "Shortcuts:"

[Files]
Source: "{#SourceDir}\meeting-transcriber.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\vulkan\*"; DestDir: "{app}\vulkan"; Flags: ignoreversion recursesubdirs createallsubdirs skipifsourcedoesntexist
Source: "{#SourceDir}\README.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; The same shortcut the app's Install makes, which it looks for to know it is
; installed.
Name: "{autoprograms}\Meeting Transcriber"; Filename: "{app}\meeting-transcriber.exe"; Comment: "Live meeting transcription and dictation"
Name: "{autodesktop}\Meeting Transcriber"; Filename: "{app}\meeting-transcriber.exe"; Comment: "Live meeting transcription and dictation"; Tasks: desktopicon

[Run]
Filename: "{app}\meeting-transcriber.exe"; Description: "Open Meeting Transcriber"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Takes away what the app added itself (its startup entry, the shortcuts its
; own Install made). Settings, models and transcripts stay.
Filename: "{app}\meeting-transcriber.exe"; Parameters: "uninstall"; Flags: runhidden waituntilterminated; RunOnceId: "AppUninstall"

[UninstallDelete]
; Downloaded by the app, so not in the list of installed files.
Type: filesandordirs; Name: "{app}\cuda"
Type: filesandordirs; Name: "{app}\cuda.new"
Type: files; Name: "{app}\cuda-addon.zip.part"
Type: dirifempty; Name: "{app}"
