$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
if (Test-Path -LiteralPath .tools\cargo\bin\cargo.exe) { . .\scripts\env.ps1 }
$clippySysroot = rustc --print sysroot
$clippyHost = ((rustc -vV | Select-String '^host:').ToString() -replace '^host:\s*', '')
$env:Path = "$clippySysroot\lib\rustlib\$clippyHost\bin;$env:Path"
New-Item -ItemType Directory -Force -Path dist\agents,dist\package,dist\package\docs | Out-Null
foreach ($clippyArch in @('x86_64', 'aarch64')) {
    $clippyTarget = "$clippyArch-unknown-linux-musl"
    rustup target add $clippyTarget
    if ($LASTEXITCODE -ne 0) { throw "Installing target failed: $clippyTarget" }
    cargo build --locked --release --bin clippy-agent --target $clippyTarget
    if ($LASTEXITCODE -ne 0) { throw "Agent build failed: $clippyTarget" }
    Copy-Item -LiteralPath "target\$clippyTarget\release\clippy-agent" -Destination "dist\agents\clippy-agent-linux-$clippyArch"
}
$clippyWindowsOutput = 'target\release'
if ($clippyHost -eq 'x86_64-pc-windows-msvc') {
    # An explicit target keeps host proc-macro builds separate from CRT flags.
    # End users must not need to install a Visual C++ redistributable.
    $clippyPreviousFlags = $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS
    try {
        $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = "$clippyPreviousFlags -C target-feature=+crt-static".Trim()
        cargo build --locked --release --bin clippy --bin clippy-background --target $clippyHost
        if ($LASTEXITCODE -ne 0) { throw 'Windows build failed' }
    } finally {
        $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = $clippyPreviousFlags
    }
    $clippyWindowsOutput = "target\$clippyHost\release"
} else {
    cargo build --locked --release --bin clippy --bin clippy-background
    if ($LASTEXITCODE -ne 0) { throw 'Windows build failed' }
}
Copy-Item -LiteralPath "$clippyWindowsOutput\clippy.exe" -Destination dist\package\clippy.exe
Copy-Item -LiteralPath "$clippyWindowsOutput\clippy-background.exe" -Destination dist\package\clippy-background.exe
& .\scripts\check-runtime.ps1 -Files @('dist\package\clippy.exe', 'dist\package\clippy-background.exe')
Copy-Item -LiteralPath README.md,LICENSE,CONTRIBUTING.md,SECURITY.md -Destination dist\package
Copy-Item -LiteralPath docs\validation.md,docs\roadmap.md,docs\architecture.md -Destination dist\package\docs
Compress-Archive -Path dist\package\* -DestinationPath dist\clippy-windows-x64.zip -Force
Get-FileHash -Algorithm SHA256 dist\clippy-windows-x64.zip
