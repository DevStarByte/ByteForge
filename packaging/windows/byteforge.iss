; Inno Setup script for the Windows installer. Built by .github/workflows/release.yml:
;   iscc /DAppVersion=0.3.2 /DSourceDir=dist\ByteForge /DOutputDir=out packaging\windows\byteforge.iss
; SourceDir holds byteforge.exe with themes\ and modulehost\ beside it (the layout AppPaths expects).

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef NumVersion
  ; numeric x.y.z for the file version (AppVersion may carry a "-beta.N" suffix)
  #define NumVersion "0.0.0"
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
AppCopyright=DevStarByte, GPL-2.0-only
VersionInfoVersion={#NumVersion}
VersionInfoDescription=ByteForge Setup
VersionInfoProductName=ByteForge
VersionInfoCompany=DevStarByte
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
; the updater closes ByteForge itself; this covers a second window or a slow exit
CloseApplications=yes

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[InstallDelete]
; drop bundled themes and modules from older versions so removed ones don't linger (modules\ isn't bundled any more)
Type: filesandordirs; Name: "{app}\themes"
Type: filesandordirs; Name: "{app}\modules"
Type: filesandordirs; Name: "{app}\modulehost"

[Icons]
Name: "{autoprograms}\ByteForge"; Filename: "{app}\byteforge.exe"
Name: "{autodesktop}\ByteForge"; Filename: "{app}\byteforge.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\byteforge.exe"; Description: "{cm:LaunchProgram,ByteForge}"; Flags: nowait postinstall skipifsilent
; the built-in updater runs the setup with /SILENT /RELAUNCH=1 and expects ByteForge back afterwards
Filename: "{app}\byteforge.exe"; Flags: nowait runasoriginaluser; Check: ShouldRelaunch

[Code]
const
  UninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{6E0B4C1E-2F3A-4D8B-9B7E-B7F0FE1D2C42}_is1';

function ShouldRelaunch: Boolean;
begin
  Result := WizardSilent and (ExpandConstant('{param:RELAUNCH|0}') = '1');
end;

function HasParam(const Name: String): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), Name) = 0 then
      Result := True;
end;

{ The built-in updater starts this setup silently and without admin rights. Setup then reuses the install
  mode of the installed version; for an install for all users (Program Files) it would run without the rights
  to replace its files and fail with "access denied". So in that case it starts itself again with admin
  rights (one UAC prompt) and waits, and ByteForge comes back without admin rights afterwards. }
function InitializeSetup: Boolean;
var
  Dir: String;
  Code: Integer;
begin
  Result := True;
  if IsAdmin or not (HasParam('/SILENT') or HasParam('/VERYSILENT')) or HasParam('/CURRENTUSER') then
    Exit;
  if RegKeyExists(HKCU, UninstallKey) then
    Exit;
  if not (RegQueryStringValue(HKLM64, UninstallKey, 'InstallLocation', Dir)
          or RegQueryStringValue(HKLM32, UninstallKey, 'InstallLocation', Dir)) then
    Exit;
  if not ShellExec('runas', ExpandConstant('{srcexe}'),
                   '/SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS /ALLUSERS /RELAUNCH=0',
                   '', SW_SHOWNORMAL, ewWaitUntilTerminated, Code) then
    Log('Starting Setup with admin rights failed: ' + SysErrorMessage(Code));
  { Updated or not (the UAC prompt can be declined), ByteForge comes back. }
  if ExpandConstant('{param:RELAUNCH|0}') = '1' then
    Exec(AddBackslash(Dir) + 'byteforge.exe', '', '', SW_SHOWNORMAL, ewNoWait, Code);
  Result := False;
end;
