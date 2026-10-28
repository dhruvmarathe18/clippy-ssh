param([Parameter(Mandatory = $true)][string[]]$Files)
$ErrorActionPreference = 'Stop'
# Inspect the PE import table without requiring Visual Studio tools on PATH.
# https://learn.microsoft.com/windows/win32/debug/pe-format
foreach ($file in $Files) {
    $bytes = [IO.File]::ReadAllBytes((Resolve-Path -LiteralPath $file).Path)
    $pe = [BitConverter]::ToInt32($bytes, 0x3c)
    if ([BitConverter]::ToUInt32($bytes, $pe) -ne 0x4550) { throw "Invalid PE file: $file" }
    $sections = [BitConverter]::ToUInt16($bytes, $pe + 6)
    $optionalSize = [BitConverter]::ToUInt16($bytes, $pe + 20)
    $optional = $pe + 24
    if ([BitConverter]::ToUInt16($bytes, $optional) -ne 0x20b) { throw "Expected a 64-bit PE executable: $file" }
    $sectionTable = $optional + $optionalSize
    function Convert-Rva([uint32]$Address) {
        for ($section=0; $section -lt $sections; $section++) {
            $offset = $sectionTable + 40 * $section
            $virtualSize = [BitConverter]::ToUInt32($bytes, $offset + 8)
            $virtualAddress = [BitConverter]::ToUInt32($bytes, $offset + 12)
            $rawSize = [BitConverter]::ToUInt32($bytes, $offset + 16)
            $rawAddress = [BitConverter]::ToUInt32($bytes, $offset + 20)
            if ($Address -ge $virtualAddress -and $Address -lt ($virtualAddress + [Math]::Max($virtualSize, $rawSize))) {
                return [int]($Address - $virtualAddress + $rawAddress)
            }
        }
        throw "Unmapped PE address in $file"
    }
    $imports = [BitConverter]::ToUInt32($bytes, $optional + 112 + 8)
    if (!$imports) { throw "Import table missing: $file" }
    $descriptor = Convert-Rva $imports
    $dlls = @()
    while ($true) {
        $name = [BitConverter]::ToUInt32($bytes, $descriptor + 12)
        if (!$name) { break }
        $start = Convert-Rva $name
        $end = $start
        while ($bytes[$end] -ne 0) { $end++ }
        $dll = [Text.Encoding]::ASCII.GetString($bytes, $start, $end-$start)
        if ($dll -match '^(vcruntime|msvcp|concrt|libgcc|libstdc|libwinpthread)') { throw "$file requires an unbundled runtime: $dll" }
        $dlls += $dll
        $descriptor += 20
    }
    Write-Output ("Runtime imports checked: {0} ({1})" -f $file, ($dlls -join ', '))
}
