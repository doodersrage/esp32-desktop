; Inno Setup script for ESP32 Bench (packaging/windows/build.sh passes Version, Source
; and the output directory). Installs for the current user, without administrator
; rights, and registers esp32bench:// links from esp32bench.dev.

[Setup]
AppId={{6E2B1C2A-6B4E-4C59-9C4F-2F1E3B7A9E10}
AppName=ESP32 Bench
AppVersion={#Version}
AppPublisher=esp32bench
AppPublisherURL=https://esp32bench.dev/
DefaultDirName={autopf}\ESP32 Bench
DefaultGroupName=ESP32 Bench
PrivilegesRequired=lowest
OutputBaseFilename=esp32bench-{#Version}-windows-x86_64-setup
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile={#Source}\LICENSE.txt
UninstallDisplayIcon={app}\esp32bench-gui.exe

[Files]
Source: "{#Source}\*"; DestDir: "{app}"; Flags: recursesubdirs ignoreversion

[Icons]
Name: "{group}\ESP32 Bench"; Filename: "{app}\esp32bench-gui.exe"
Name: "{group}\Uninstall ESP32 Bench"; Filename: "{uninstallexe}"

[Registry]
Root: HKCU; Subkey: "Software\Classes\esp32bench"; ValueType: string; ValueName: ""; ValueData: "URL:ESP32 Bench project link"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\esp32bench"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\esp32bench\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\esp32bench-gui.exe"" ""%1"""

[Run]
Filename: "{app}\esp32bench-gui.exe"; Description: "Start ESP32 Bench"; Flags: nowait postinstall skipifsilent
