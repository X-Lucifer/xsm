[CmdletBinding()]
param([switch]$DebugBuild)
$ErrorActionPreference = 'Stop'
$project = Split-Path -Parent $PSScriptRoot
Push-Location $project
try {
    $buildArguments = @('build', '--locked', '--bins', '--examples')
    if (-not $DebugBuild) { $buildArguments += '--release' }
    & cargo @buildArguments
    if ($LASTEXITCODE) { throw 'Build failed' }
    $profile = if ($DebugBuild) { 'debug' } else { 'release' }
    $output = Join-Path $project "target\$profile"
    Write-Host "Built the xsm.exe command-line entry and service host in $output"
} finally { Pop-Location }
