; Inno Setup script for the Windows installer. Built by .github/workflows/release.yml:
;   iscc /DAppVersion=0.3.2 /DSourceDir=dist\ByteForge /DOutputDir=out packaging\windows\byteforge.iss
; SourceDir holds byteforge.exe with themes\ and modules\ beside it (the layout AppPaths expects).

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\dist\ByteForge"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

[Setup]
AppId={{6E0B4C1E-2F3A-4D8B-9B7E-B7F0FE1D2C42}
AppName=ByteForge
AppVersion={#AppVersion}
AppPublisher=DevStarByte
AppPublisherURL=https://github.com/DevStarByte/ByteForge
AppSupportURL=https://github.com/DevStarByte/ByteForge/issues
DefaultDirName={autopf}\ByteForge
DefaultGroupName=ByteForge
DisableProgramGroupPage=yes
; installs for the current user without admin rights; the dialog offers all users
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=ByteForge-{#AppVersion}-windows-x64-setup
UninstallDisplayIcon={app}\byteforge.exe
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[InstallDelete]
; drop bundled themes and modules from older versions so removed ones don't linger
Type: filesandordirs; Name: "{app}\themes"
Type: filesandordirs; Name: "{app}\modules"

[Icons]
Name: "{autoprograms}\ByteForge"; Filename: "{app}\byteforge.exe"
Name: "{autodesktop}\ByteForge"; Filename: "{app}\byteforge.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\byteforge.exe"; Description: "{cm:LaunchProgram,ByteForge}"; Flags: nowait postinstall skipifsilent
