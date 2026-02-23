[Setup]
AppName=Haven
AppVersion=2.3.0
AppPublisher=Haven
DefaultDirName={autopf}\Haven
DefaultGroupName=Haven
OutputDir=installer_output
OutputBaseFilename=Haven_2.3.0_x64-setup
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=windows\runner\resources\app_icon.ico
UninstallDisplayIcon={app}\haven_app.exe
WizardStyle=modern
DisableProgramGroupPage=yes

[Files]
Source: "build\windows\x64\runner\Release\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{group}\Haven"; Filename: "{app}\haven_app.exe"
Name: "{autodesktop}\Haven"; Filename: "{app}\haven_app.exe"

[Run]
Filename: "{app}\haven_app.exe"; Description: "Launch Haven"; Flags: nowait postinstall skipifsilent
