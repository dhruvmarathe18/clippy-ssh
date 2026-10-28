$env:RUSTUP_HOME = Join-Path $PSScriptRoot '..\.tools\rustup'
$env:CARGO_HOME = Join-Path $PSScriptRoot '..\.tools\cargo'
$clippyToolchain = Join-Path $env:RUSTUP_HOME 'toolchains\stable-x86_64-pc-windows-gnu\lib\rustlib\x86_64-pc-windows-gnu\bin'
$env:Path = "$env:CARGO_HOME\bin;$clippyToolchain;$env:Path"
$clippyMingw = Get-ChildItem -LiteralPath (Join-Path $PSScriptRoot '..\.tools\mingw') -Directory -ErrorAction SilentlyContinue | Select-Object -First 1
if ($clippyMingw) {
    $env:Path = "$($clippyMingw.FullName)\bin;$env:Path"
    $clippyWrapper = Join-Path $PSScriptRoot '..\.tools\rustc-host-wrapper.exe'
    $clippyWrapperSource = Join-Path $PSScriptRoot 'rustc-host-wrapper.rs'
    if (!(Test-Path -LiteralPath $clippyWrapper) -or (Get-Item $clippyWrapperSource).LastWriteTime -gt (Get-Item $clippyWrapper).LastWriteTime) {
        rustc $clippyWrapperSource -C linker=ld.lld.exe -C linker-flavor=ld -C link-self-contained=yes -o $clippyWrapper
        if ($LASTEXITCODE -ne 0) { throw 'Building host compiler wrapper failed' }
    }
    $env:RUSTC_WRAPPER = $clippyWrapper
}
