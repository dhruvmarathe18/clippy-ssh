param([switch]$SkipPackage, [string]$Iscc)
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
if (!$SkipPackage) { & .\scripts\package.ps1 }
if (Test-Path -LiteralPath .tools\cargo\bin\cargo.exe) { . .\scripts\env.ps1 }
$metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'Cannot read package version' }
$version = ($metadata.packages | Where-Object name -eq 'clippy-ssh').version
if ((& .\dist\package\clippy.exe --version) -ne "clippy $version") { throw 'Packaged executable version does not match Cargo.toml' }
if (!$Iscc) { $Iscc = & .\scripts\bootstrap-inno.ps1 }
& $Iscc ("/DAppVersion=$version") .\installer\clippy.iss
if ($LASTEXITCODE -ne 0) { throw 'Installer compilation failed' }
$files = @("Clippy-Setup-$version-x64.exe", 'clippy-windows-x64.zip')
$lines = foreach ($file in $files) {
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path dist $file)).Hash.ToLowerInvariant()
    "$hash  $file"
}
[IO.File]::WriteAllLines((Join-Path (Get-Location) 'dist\SHA256SUMS.txt'), $lines, [Text.UTF8Encoding]::new($false))
Get-Item -LiteralPath (Join-Path dist $files[0]) | Select-Object Name,Length
