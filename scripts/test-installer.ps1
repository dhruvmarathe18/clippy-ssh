param([Parameter(Mandatory = $true)][string]$Installer)
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or $env:RUNNER_OS -ne 'Windows') {
    throw 'This lifecycle test changes user profiles and execution policy. Run only on a disposable GitHub-hosted Windows runner.'
}
Remove-Item Env:\PSExecutionPolicyPreference -ErrorAction SilentlyContinue
$Installer = (Resolve-Path -LiteralPath $Installer).Path
$clippyRoot = Join-Path $env:LOCALAPPDATA 'Clippy'
$manifestPath = Join-Path $clippyRoot 'installation.json'
if (Test-Path -LiteralPath $manifestPath) { throw 'Refusing to test over an existing Clippy installation' }
$testRoot = Join-Path $env:RUNNER_TEMP ('clippy-installer-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $testRoot | Out-Null
$appDir = Join-Path $testRoot 'Installed App'
$documents = [Environment]::GetFolderPath('MyDocuments')
$profiles = @((Join-Path $documents 'WindowsPowerShell\profile.ps1'), (Join-Path $documents 'PowerShell\profile.ps1'))
foreach ($profilePath in $profiles) {
    New-Item -ItemType Directory -Force -Path (Split-Path $profilePath) | Out-Null
    [IO.File]::WriteAllText($profilePath, "# Installer fixture`r`nfunction ClippyInstallerSentinel { 'preserved' }`r`n", [Text.UnicodeEncoding]::new($false, $true))
}
$originalProfiles = @($profiles | ForEach-Object { (Get-FileHash -LiteralPath $_).Hash })
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
New-Item -Path $runKey -Force | Out-Null
Set-ItemProperty -LiteralPath $runKey -Name ClippyInstallerTestNeighbor -Value 'preserve me'
function Run-Setup([string]$LogName, [bool]$ShouldSucceed = $true) {
    $p = Start-Process -FilePath $Installer -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', ('/DIR="' + $appDir + '"'), ('/LOG="' + (Join-Path $testRoot $LogName) + '"')) -WindowStyle Hidden -PassThru
    if (!$p.WaitForExit(120000)) { throw 'Installer timed out' }
    if (($p.ExitCode -eq 0) -ne $ShouldSucceed) { Get-Content -LiteralPath (Join-Path $testRoot $LogName); throw "Unexpected installer exit: $($p.ExitCode)" }
}

# Explicit restrictions must fail preflight, without touching integration.
Set-ExecutionPolicy -Scope CurrentUser Restricted -Force
Run-Setup 'restricted.log' $false
if (Test-Path -LiteralPath $manifestPath) { throw 'Restricted-policy install changed integration' }
for ($i=0; $i -lt $profiles.Count; $i++) {
    if ((Get-FileHash -LiteralPath $profiles[$i]).Hash -ne $originalProfiles[$i]) { throw 'Failed preflight changed a profile' }
}

# Model a fresh Windows account: no configured policy at either ordinary scope.
Set-ExecutionPolicy -Scope CurrentUser Undefined -Force
Set-ExecutionPolicy -Scope LocalMachine Undefined -Force
Run-Setup 'install.log'
$manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
if ((Get-ExecutionPolicy -Scope CurrentUser) -ne 'RemoteSigned') { throw 'Default PowerShell policy was not configured' }
& $manifest.client background check
if ($LASTEXITCODE -ne 0) { throw 'Installed background registration failed' }
$startup = Get-ItemPropertyValue -LiteralPath $runKey -Name ClippySSH
if ($startup -ne $manifest.startup) { throw 'Startup entry mismatch' }
$backgroundPid = (Get-CimInstance Win32_Process -Filter "Name='clippy-background.exe'" | Where-Object ExecutablePath -eq $manifest.background).ProcessId
if (!$backgroundPid) { throw 'Installed background process missing' }
foreach ($profilePath in $profiles) {
    if ((Select-String -LiteralPath $profilePath -SimpleMatch '# >>> Clippy SSH managed integration >>>').Count -ne 1) { throw 'Profile integration missing or duplicated' }
}
$profileCheck = & powershell.exe -NoLogo -NonInteractive -Command "if ((Get-Command ssh).CommandType -ne 'Function') {exit 1}; ClippyInstallerSentinel; ssh -V" 2>&1
if ($LASTEXITCODE -ne 0 -or ($profileCheck -join ' ') -notmatch 'preserved') { throw 'New PowerShell session did not load integration' }

Run-Setup 'repeat.log'
if (!(Get-Process -Id $backgroundPid -ErrorAction SilentlyContinue)) { throw 'Repeat install stopped the background process' }
foreach ($profilePath in $profiles) {
    if ((Select-String -LiteralPath $profilePath -SimpleMatch '# >>> Clippy SSH managed integration >>>').Count -ne 1) { throw 'Repeat install duplicated profile integration' }
}

# Force a post-configuration startup failure using our own copied test payload.
# The wrong background program exits immediately; no unrelated application runs.
$broken = Join-Path $testRoot 'Broken Payload'
New-Item -ItemType Directory -Path $broken | Out-Null
Copy-Item -LiteralPath (Join-Path $appDir 'clippy.exe') -Destination (Join-Path $broken 'clippy.exe')
Copy-Item -LiteralPath (Join-Path $appDir 'clippy.exe') -Destination (Join-Path $broken 'clippy-background.exe')
$configuration = @($manifestPath, $manifest.script) + $profiles
$before = @($configuration | ForEach-Object { (Get-FileHash -LiteralPath $_).Hash })
& (Join-Path $broken 'clippy.exe') install --enable-profiles
if ($LASTEXITCODE -eq 0) { throw 'Broken background payload unexpectedly installed' }
for ($i=0; $i -lt $configuration.Count; $i++) {
    if ((Get-FileHash -LiteralPath $configuration[$i]).Hash -ne $before[$i]) { throw 'Failed install did not restore configuration' }
}
if ((Get-ItemPropertyValue -LiteralPath $runKey -Name ClippySSH) -ne $startup) { throw 'Failed install changed startup' }
if (!(Get-Process -Id $backgroundPid -ErrorAction SilentlyContinue)) { throw 'Failed install stopped the previous background app' }

$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{9E84EB4C-6997-4450-A76A-5568654BD550}_is1'
if (!(Test-Path -LiteralPath $uninstallKey)) { throw 'Windows Apps uninstall registration missing' }
$uninstaller = Join-Path $appDir 'unins000.exe'
$p = Start-Process -FilePath $uninstaller -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', ('/LOG="' + (Join-Path $testRoot 'uninstall.log') + '"')) -WindowStyle Hidden -PassThru
if (!$p.WaitForExit(120000) -or $p.ExitCode -ne 0) { throw 'Uninstall failed' }
if (Test-Path -LiteralPath $manifestPath) { throw 'Uninstall left its installation record' }
if (Get-ItemProperty -LiteralPath $runKey -Name ClippySSH -ErrorAction SilentlyContinue) { throw 'Uninstall left startup enabled' }
if (Test-Path -LiteralPath $uninstallKey) { throw 'Uninstall left Windows Apps registration' }
if ((Get-ExecutionPolicy -Scope CurrentUser) -ne 'Undefined') { throw 'Uninstall did not restore the original profile policy' }
for ($i=0; $i -lt $profiles.Count; $i++) {
    if ((Get-FileHash -LiteralPath $profiles[$i]).Hash -ne $originalProfiles[$i]) { throw 'Uninstall changed unrelated profile content or encoding' }
}
if ((Get-ItemPropertyValue -LiteralPath $runKey -Name ClippyInstallerTestNeighbor) -ne 'preserve me') { throw 'Uninstall changed another startup entry' }
Remove-ItemProperty -LiteralPath $runKey -Name ClippyInstallerTestNeighbor
# The intentional failed-payload test leaves LASTEXITCODE=1. GitHub's PowerShell
# wrapper propagates it unless we explicitly mark the completed test successful.
$global:LASTEXITCODE = 0
Write-Output 'Installer lifecycle passed: restriction handling, fresh install, profile loading, repeat install, rollback, startup, background IPC, uninstall, and policy restoration.'
