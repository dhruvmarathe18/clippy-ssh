#ifndef AppVersion
  #error AppVersion must be supplied by scripts/build-installer.ps1
#endif
#define Root AddBackslash(SourcePath) + ".."

[Setup]
AppId={{9E84EB4C-6997-4450-A76A-5568654BD550}
AppName=Clippy SSH
AppVersion={#AppVersion}
AppVerName=Clippy SSH {#AppVersion}
AppPublisher=Clippy SSH contributors
AppPublisherURL=https://github.com/dhruvmarathe18/clippy-ssh
AppSupportURL=https://github.com/dhruvmarathe18/clippy-ssh/issues
AppUpdatesURL=https://github.com/dhruvmarathe18/clippy-ssh/releases
DefaultDirName={localappdata}\Programs\Clippy SSH
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableWelcomePage=no
PrivilegesRequired=lowest
SetupArchitecture=x64
ArchitecturesAllowed=x64os
ArchitecturesInstallIn64BitMode=x64os
MinVersion=10.0.19041
OutputDir={#Root}\dist
OutputBaseFilename=Clippy-Setup-{#AppVersion}-x64
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=no
RestartApplications=no
UninstallDisplayIcon={app}\clippy.exe
UninstallDisplayName=Clippy SSH
VersionInfoVersion={#AppVersion}
VersionInfoDescription=Clippy SSH Installer
VersionInfoProductName=Clippy SSH
SetupLogging=yes

[Messages]
WelcomeLabel1=Copy locally. Paste over SSH.
WelcomeLabel2=Setup installs Clippy SSH for your Windows account, enables clipboard file and image paste in Windows Terminal with PowerShell, and starts Clippy at sign-in.%n%nBoth Linux helpers are included. Your server is prepared automatically when you connect.%n%nRequires Windows Terminal and Windows OpenSSH Client. Existing SSH connections stay open.
FinishedHeadingLabel=Clippy SSH is ready
FinishedLabel=Open a NEW PowerShell tab in Windows Terminal and connect with your normal ssh command.%n%nCopy a screenshot or Explorer file and press Ctrl+V. Clippy uploads it and inserts its remote path without pressing Enter.%n%nNo build tools or manual setup commands are needed. Use the tray icon to pause Clippy. Uninstall from Windows Settings > Apps.

[Files]
Source: "{#Root}\dist\package\clippy.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\dist\package\clippy-background.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\installer\getting-started.html"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Clippy SSH\Getting started"; Filename: "{app}\getting-started.html"
Name: "{userprograms}\Clippy SSH\Uninstall Clippy SSH"; Filename: "{uninstallexe}"

[Code]
function RunClippy(const Executable, Arguments: String): String;
var
  ExitCode, Index: Integer;
  Output: TExecOutput;
begin
  Result := '';
  if not ExecAndCaptureOutput(Executable, Arguments, '', SW_HIDE, ewWaitUntilTerminated, ExitCode, Output) then
    Result := 'Could not start Clippy: ' + SysErrorMessage(ExitCode)
  else if (ExitCode <> 0) or Output.Error then begin
    for Index := 0 to GetArrayLength(Output.StdErr) - 1 do
      Result := Result + Output.StdErr[Index] + #13#10;
    if Result = '' then Result := 'Clippy setup failed (code ' + IntToStr(ExitCode) + ').';
  end;
  if Result <> '' then Log(Result);
end;

procedure InitializeWizard;
begin
  WizardForm.ReadyMemo.Lines.Add('Clippy SSH configures only the current Windows account.');
end;

function UpdateReadyMemo(Space, NewLine, MemoUserInfoInfo, MemoDirInfo, MemoTypeInfo, MemoComponentsInfo, MemoGroupInfo, MemoTasksInfo: String): String;
begin
  Result := 'Clippy SSH will:' + NewLine + NewLine +
    '  Install the client and background app for your account' + NewLine +
    '  Enable normal ssh in new PowerShell tabs' + NewLine +
    '  Start the background app now and at sign-in' + NewLine +
    '  Add uninstall and getting-started shortcuts' + NewLine + NewLine +
    'If PowerShell has no configured script policy, Setup enables locally created profiles using RemoteSigned for your account. Explicit and organization policies are preserved. Uninstall restores a default policy changed by Setup when it is still unchanged.';
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  ExtractTemporaryFile('clippy.exe');
  Result := RunClippy(ExpandConstant('{tmp}\clippy.exe'), 'install --check --enable-profiles');
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Error: String;
begin
  if CurStep = ssPostInstall then begin
    WizardForm.StatusLabel.Caption := 'Enabling clipboard paste and starting Clippy...';
    Error := RunClippy(ExpandConstant('{app}\clippy.exe'), 'install --enable-profiles');
    if Error <> '' then RaiseException(Error);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Error: String;
begin
  { usUninstall runs after confirmation and before any files are deleted. }
  if CurUninstallStep = usUninstall then begin
    Error := RunClippy(ExpandConstant('{app}\clippy.exe'), 'uninstall');
    if Error <> '' then RaiseException(Error);
  end;
end;
