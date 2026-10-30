[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$project = Split-Path -Parent $PSScriptRoot
Push-Location $project
try {
    cargo fmt --all -- --check
    if ($LASTEXITCODE) { throw 'Formatting failed' }
    cargo clippy --locked --all-targets -- -D warnings
    if ($LASTEXITCODE) { throw 'Clippy failed' }
    cargo test --locked --all-targets
    if ($LASTEXITCODE) { throw 'Tests failed' }
    & (Join-Path $PSScriptRoot 'build.ps1')
} finally { Pop-Location }
