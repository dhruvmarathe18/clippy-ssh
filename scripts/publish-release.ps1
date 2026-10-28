param([Parameter(Mandatory = $true)][string]$Tag)
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
$version = ((cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages | Where-Object name -eq clippy-ssh).version
if ($Tag -cne "v$version") { throw 'Release tag must match Cargo.toml exactly' }
$commit = (git rev-parse HEAD).Trim()
$notes = Join-Path 'docs\releases' "$Tag.md"
if (!(Test-Path -LiteralPath $notes)) { throw "Release notes missing: $notes" }
$installer = "dist\Clippy-Setup-$version-x64.exe"
$portable = 'dist\clippy-windows-x64.zip'
foreach ($line in Get-Content -LiteralPath dist\SHA256SUMS.txt) {
    if ($line -notmatch '^([0-9a-f]{64})  ([A-Za-z0-9._-]+)$') { throw 'Invalid checksum manifest' }
    if ((Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path dist $Matches[2])).Hash -ine $Matches[1]) { throw 'Release asset changed after verification' }
}
& gh release create $Tag $installer $portable 'dist\SHA256SUMS.txt' --repo dhruvmarathe18/clippy-ssh --target $commit --prerelease --title "Clippy SSH $Tag - Windows installer preview" --notes-file $notes
if ($LASTEXITCODE -ne 0) { throw 'GitHub release publication failed' }
