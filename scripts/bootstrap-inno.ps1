param([string]$Destination = (Join-Path $PSScriptRoot '..\.tools\innosetup-7.1.1'))
$ErrorActionPreference = 'Stop'
$Destination = [IO.Path]::GetFullPath($Destination)
$compiler = Join-Path $Destination 'ISCC.exe'
if (Test-Path -LiteralPath $compiler) { return $compiler }
$downloads = Join-Path $PSScriptRoot '..\.tools\downloads'
New-Item -ItemType Directory -Force -Path $downloads | Out-Null
$bootstrap = Join-Path $downloads 'innosetup-7.1.1-x64.exe'
$expected = '8a5a6711c3766a6e5fb74f280ba0e3551e719ce94538d901fdbd1f8f9920b3f0'
if (!(Test-Path -LiteralPath $bootstrap)) {
    Invoke-WebRequest -UseBasicParsing 'https://github.com/jrsoftware/issrc/releases/download/is-7_1_1/innosetup-7.1.1-x64.exe' -OutFile $bootstrap
}
if ((Get-FileHash -LiteralPath $bootstrap -Algorithm SHA256).Hash -ne $expected) { throw 'Inno Setup download SHA-256 mismatch' }
$signature = Get-AuthenticodeSignature -LiteralPath $bootstrap
if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'Pyrsys B\.V\.') { throw 'Inno Setup publisher signature verification failed' }
$setup = Start-Process -FilePath $bootstrap -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', '/CURRENTUSER', '/NOICONS', '/TASKS=', ('/DIR="' + $Destination + '"')) -WindowStyle Hidden -Wait -PassThru
if ($setup.ExitCode -ne 0 -or !(Test-Path -LiteralPath $compiler)) { throw "Inno Setup installation failed: $($setup.ExitCode)" }
$compiler
