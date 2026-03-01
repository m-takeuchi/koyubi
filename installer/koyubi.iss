; Koyubi SKK - Inno Setup Installer Script
; Requires Inno Setup 6.0+

#define MyAppName "Koyubi SKK"
#define MyAppVersion "0.1.1"
#define MyAppPublisher "Koyubi Project"
#define MyAppURL "https://github.com/barewalker/koyubi"

; AppId matches CLSID_KOYUBI_TEXT_SERVICE in globals.rs
#define MyAppId "{{A7B3C4D5-E6F7-4890-AB12-CD34EF567890}"

[Setup]
AppId={#MyAppId}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
DefaultDirName={autopf}\Koyubi
DefaultGroupName={#MyAppName}
OutputDir=..\target\installer
OutputBaseFilename=koyubi-{#MyAppVersion}-setup
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
DisableProgramGroupPage=yes
UninstallDisplayName={#MyAppName}
MinVersion=10.0

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"

[Files]
; TSF COM DLL - regserver flag handles DllRegisterServer/DllUnregisterServer
Source: "..\target\x86_64-pc-windows-msvc\release\koyubi_tsf.dll"; \
    DestDir: "{app}"; Flags: regserver restartreplace uninsrestartdelete 64bit
; Dictionary download tool
Source: "..\target\x86_64-pc-windows-msvc\release\koyubi-dict.exe"; \
    DestDir: "{app}"; Flags: 64bit

[Run]
; Download SKK-JISYO.L after install (silent mode for winget)
Filename: "{app}\koyubi-dict.exe"; \
    Parameters: "download --dict SKK-JISYO.L --quiet"; \
    StatusMsg: "Downloading SKK dictionary..."; \
    Flags: runhidden; \
    Check: ShouldDownloadDict

[Code]
var
  DictPage: TInputOptionWizardPage;

procedure InitializeWizard;
begin
  { Dictionary selection page for interactive installs }
  DictPage := CreateInputOptionPage(
    wpSelectTasks,
    'Dictionary Selection',
    'Select SKK dictionaries to download.',
    'The following dictionaries are available. ' +
    'SKK-JISYO.L (large dictionary) is recommended for most users.',
    False, False);
  DictPage.Add('SKK-JISYO.L - Large dictionary (~4.3 MB) [recommended]');
  DictPage.Add('SKK-JISYO.jinmei - Personal names (~0.8 MB)');
  DictPage.Add('SKK-JISYO.geo - Place names (~0.4 MB)');
  DictPage.Add('SKK-JISYO.station - Station names (~0.2 MB)');
  DictPage.Add('SKK-JISYO.propernoun - Proper nouns (~0.3 MB)');
  DictPage.Values[0] := True; { SKK-JISYO.L checked by default }
end;

function ShouldDownloadDict: Boolean;
begin
  { In silent mode (/VERYSILENT from winget), always download SKK-JISYO.L }
  if WizardSilent then
    Result := True
  else
    Result := DictPage.Values[0];
end;

function GetDictArgs(Value: string): string;
var
  Args: string;
  Names: array[0..4] of string;
  I: Integer;
begin
  Names[0] := 'SKK-JISYO.L';
  Names[1] := 'SKK-JISYO.jinmei';
  Names[2] := 'SKK-JISYO.geo';
  Names[3] := 'SKK-JISYO.station';
  Names[4] := 'SKK-JISYO.propernoun';

  Args := '';
  for I := 0 to 4 do
  begin
    if DictPage.Values[I] then
      Args := Args + ' --dict ' + Names[I];
  end;

  if Args = '' then
    Result := ''
  else
    Result := 'download' + Args + ' --quiet';
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
  Args: string;
begin
  if CurStep = ssPostInstall then
  begin
    if not WizardSilent then
    begin
      Args := GetDictArgs('');
      if Args <> '' then
      begin
        Exec(ExpandConstant('{app}\koyubi-dict.exe'), Args,
          '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
      end;
    end;
  end;
end;
