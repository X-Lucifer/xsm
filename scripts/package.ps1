[CmdletBinding()]
param(
    [ValidateSet('', 'x86_64-pc-windows-msvc', 'i686-pc-windows-msvc', 'aarch64-pc-windows-msvc')]
    [string]$Target = ''
)
$ErrorActionPreference = 'Stop'
$project = Split-Path -Parent $PSScriptRoot
$targetDirectory = Join-Path $project 'target'
if ($Target) { $targetDirectory = Join-Path $targetDirectory $Target }
$release = Join-Path $targetDirectory 'release'
& (Join-Path $PSScriptRoot 'verify-resources.ps1') -Executable (Join-Path $release 'xsm.exe')
$manifest = Get-Content -LiteralPath (Join-Path $project 'Cargo.toml') -Raw
$version = [regex]::Match($manifest, '(?m)^version = "([0-9.]+)"').Groups[1].Value
$image = [IO.File]::ReadAllBytes((Join-Path $release 'xsm.exe'))
$pe = [BitConverter]::ToUInt32($image, 0x3c)
$architecture = switch ([BitConverter]::ToUInt16($image, $pe + 4)) {
    0x8664 { 'x64' }
    0x014c { 'x86' }
    0xaa64 { 'arm64' }
    default { throw 'Unsupported PE architecture' }
}
$destination = Join-Path $project 'dist'
$screenshots = Join-Path $destination 'assets\screenshots'
New-Item -ItemType Directory -Path $screenshots -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $release 'xsm.exe') -Destination (Join-Path $destination 'xsm.exe') -Force
Copy-Item -LiteralPath (Join-Path $project 'resources\xsm.ico') -Destination (Join-Path $destination 'xsm.ico') -Force
Copy-Item -LiteralPath (Join-Path $project 'README.md') -Destination (Join-Path $destination 'README.md') -Force
$files = @('xsm.exe', 'xsm.ico', 'README.md')
foreach ($name in @('gui-light.png', 'gui-dark.png')) {
    Copy-Item -LiteralPath (Join-Path $project "assets\screenshots\$name") -Destination (Join-Path $screenshots $name) -Force
    $files += "assets/screenshots/$name"
}
$checksums = foreach ($name in $files) {
    '{0}  {1}' -f (Get-FileHash -LiteralPath (Join-Path $destination $name) -Algorithm SHA256).Hash, $name
}
[IO.File]::WriteAllLines((Join-Path $destination 'SHA256SUMS.txt'), $checksums, [Text.UTF8Encoding]::new($false))
$archive = Join-Path $destination "xsm-$version-windows-$architecture.zip"
Compress-Archive -LiteralPath @(
    (Join-Path $destination 'xsm.exe'),
    (Join-Path $destination 'xsm.ico'), (Join-Path $destination 'README.md'),
    (Join-Path $destination 'SHA256SUMS.txt'), (Join-Path $destination 'assets')
) -DestinationPath $archive -Force
Write-Host "Packaged $archive"
