[CmdletBinding()]
param([string]$Executable = '')
$ErrorActionPreference = 'Stop'
if (-not $Executable) { $Executable = Join-Path $PSScriptRoot '..\target\release\xsm.exe' }
$exe = (Resolve-Path -LiteralPath $Executable).Path
$version = [Diagnostics.FileVersionInfo]::GetVersionInfo($exe)
$expected = @{
    ProductName = 'XSM'
    CompanyName = 'Lucifer'
    FileDescription = 'XSM Windows Service Manager'
    OriginalFilename = 'xsm.exe'
    LegalCopyright = 'Copyright © Lucifer. All rights reserved.'
}
foreach ($key in $expected.Keys) {
    if ($version.$key -cne $expected[$key]) { throw "Unexpected ${key}: $($version.$key)" }
}
$manifest = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\Cargo.toml') -Raw
$semver = [regex]::Match($manifest, '(?m)^version = "([0-9.]+)"').Groups[1].Value
if ($version.FileVersion -ne "$semver.0" -or $version.ProductVersion -ne "$semver.0") { throw 'Version resource differs from Cargo.toml' }
$peBytes = [IO.File]::ReadAllBytes($exe)
$peOffset = [BitConverter]::ToUInt32($peBytes, 0x3c)
if ([BitConverter]::ToUInt16($peBytes, $peOffset + 4 + 20 + 68) -ne 3) { throw "Incorrect console PE subsystem: $exe" }
Add-Type -AssemblyName System.Drawing
$icon = [Drawing.Icon]::ExtractAssociatedIcon($exe)
if (-not $icon) { throw 'Missing executable icon' }
try {
    $bitmap = $icon.ToBitmap()
    try {
        $results = Join-Path $PSScriptRoot '..\test-results'
        New-Item -ItemType Directory -Path $results -Force | Out-Null
        $bitmap.Save((Join-Path $results 'embedded-icon.png'), [Drawing.Imaging.ImageFormat]::Png)
    } finally { $bitmap.Dispose() }
} finally { $icon.Dispose() }
$bytes = [IO.File]::ReadAllBytes((Join-Path $PSScriptRoot '..\resources\xsm.ico'))
if ([BitConverter]::ToUInt16($bytes, 2) -ne 1 -or [BitConverter]::ToUInt16($bytes, 4) -ne 9) { throw 'ICO must contain all nine sizes' }
Write-Host "Console subsystem, version, copyright, product identity and embedded icon verified: $exe"
