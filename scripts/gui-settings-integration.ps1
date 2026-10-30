[CmdletBinding()]
param([string]$Candidate = '', [string]$Probe = '')

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$OutputEncoding = [Console]::OutputEncoding
if (-not $Candidate) { $Candidate = Join-Path $PSScriptRoot '..\target\release\xsm.exe' }
if (-not $Probe) { $Probe = Join-Path $PSScriptRoot '..\target\release\examples\service_probe.exe' }
$Candidate = (Resolve-Path -LiteralPath $Candidate).Path
$Probe = (Resolve-Path -LiteralPath $Probe).Path
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
try {
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    $elevated = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}
finally { $identity.Dispose() }
if (-not $elevated) {
    throw 'Requires elevation; configures and removes only two new Xsm_Test_* services. No service is started.'
}

$service = 'Xsm_Test_' + [Guid]::NewGuid().ToString('N')
$dependency = 'Xsm_Test_' + [Guid]::NewGuid().ToString('N')
$targets = @($service, $dependency)
$attempted = [Collections.Generic.HashSet[string]]::new()
$results = Join-Path (Split-Path -Parent $PSScriptRoot) ('test-results\gui-settings-' + $service)
New-Item -ItemType Directory -Path $results -Force | Out-Null
$passed = [Collections.Generic.List[string]]::new()
$commands = [Collections.Generic.List[object]]::new()
$cleanupErrors = [Collections.Generic.List[string]]::new()
$completed = $false
$failure = $null
$startedUtc = [DateTime]::UtcNow.ToString('o')
$candidateSha256 = (Get-FileHash -LiteralPath $Candidate -Algorithm SHA256).Hash
$serviceBinary = (Resolve-Path -LiteralPath (Join-Path (Split-Path -Parent $Candidate) 'xsm.exe')).Path
$serviceBinarySha256 = (Get-FileHash -LiteralPath $serviceBinary -Algorithm SHA256).Hash
$probeSha256 = (Get-FileHash -LiteralPath $Probe -Algorithm SHA256).Hash

function Quote-WindowsArgument([string]$Value) {
    $builder = [Text.StringBuilder]::new()
    [void]$builder.Append('"')
    $slashes = 0
    foreach ($character in $Value.ToCharArray()) {
        if ($character -eq '\') { $slashes++; continue }
        $count = if ($character -eq '"') { $slashes * 2 + 1 } else { $slashes }
        if ($count) { [void]$builder.Append(('\' * $count)) }
        [void]$builder.Append($character)
        $slashes = 0
    }
    if ($slashes) { [void]$builder.Append(('\' * ($slashes * 2))) }
    [void]$builder.Append('"')
    return $builder.ToString()
}

function Invoke-Xsm([string[]]$Argv, [switch]$Reject) {
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $Candidate
    $info.Arguments = (@($Argv | ForEach-Object { Quote-WindowsArgument $_ }) -join ' ')
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.StandardOutputEncoding = [Text.UTF8Encoding]::new($false)
    $info.StandardErrorEncoding = [Text.UTF8Encoding]::new($false)
    $process = [Diagnostics.Process]::Start($info)
    try {
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(30000)) {
            $process.Kill()
            [void]$process.WaitForExit(5000)
            throw 'Configuration CLI timed out'
        }
        $output = $stdout.GetAwaiter().GetResult().Replace("`r`n", "`n").TrimEnd()
        $errorOutput = $stderr.GetAwaiter().GetResult().Replace("`r`n", "`n").TrimEnd()
        $code = $process.ExitCode
    } finally { $process.Dispose() }
    # This suite never accepts or passes a password, environment value, or user secret.
    $commands.Add([ordered]@{
        utc = [DateTime]::UtcNow.ToString('o'); arguments = @($Argv)
        exitCode = $code; stdout = $output; stderr = $errorOutput
    })
    if ($Reject) {
        if ($code -eq 0) { throw "Expected rejection: $($Argv -join ' ')" }
        return ($output + "`n" + $errorOutput).Trim()
    }
    if ($code -ne 0) { throw "Failed ($code): $($Argv -join ' ')`n$output`n$errorOutput" }
    return $output
}

function Get-Option([string]$Target, [string]$Name) {
    return Invoke-Xsm @('get', $Target, $Name)
}

function Set-Option([string]$Target, [string]$Name, [string[]]$Values) {
    Invoke-Xsm (@('set', $Target, $Name) + $Values) | Out-Null
}

function Assert-Equal($Actual, $Expected, [string]$Message) {
    if ([string]$Actual -cne [string]$Expected) {
        throw "$Message`nExpected: [$Expected]`nActual: [$Actual]"
    }
    $passed.Add($Message)
}

function Test-ServiceExists([string]$Target) {
    $controller = Get-Service -Name $Target -ErrorAction SilentlyContinue
    try { return $null -ne $controller }
    finally { if ($null -ne $controller) { $controller.Dispose() } }
}

function Read-RegistryValue([string]$Target, [string]$Name) {
    $key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey("SYSTEM\CurrentControlSet\Services\$Target")
    if ($null -eq $key) { throw "Test service registry key is missing: $Target" }
    try { return $key.GetValue($Name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) }
    finally { $key.Dispose() }
}

function Read-ConfigSnapshot([string]$Target) {
    $key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey("SYSTEM\CurrentControlSet\Services\$Target")
    if ($null -eq $key) { throw "Test service registry key is missing: $Target" }
    try {
        $snapshot = [ordered]@{}
        $names = @($key.GetValueNames())
        foreach ($name in @('ImagePath', 'Type', 'Start', 'ErrorControl', 'ObjectName', 'DisplayName', 'DependOnService', 'DependOnGroup', 'Description')) {
            $present = $names -contains $name
            $snapshot[$name] = [ordered]@{
                present = $present
                kind = if ($present) { $key.GetValueKind($name).ToString() } else { $null }
                value = $key.GetValue($name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            }
        }
        return $snapshot | ConvertTo-Json -Depth 5 -Compress
    } finally { $key.Dispose() }
}

try {
    foreach ($target in $targets) {
        if (Test-ServiceExists $target) {
            throw "Refusing to use an existing service: $target"
        }
        [void]$attempted.Add($target)
        Invoke-Xsm @('install', $target, $Probe, 'serve') | Out-Null
        Set-Option $target 'Start' @('SERVICE_DEMAND_START')
        Assert-Equal (Get-Option $target 'Application') $Probe "Probe application configured for $target"
        Assert-Equal (Get-Option $target 'Start') 'SERVICE_DEMAND_START' "Demand startup configured for $target"
        Assert-Equal (Invoke-Xsm @('status', $target)) 'SERVICE_STOPPED' "New service remains stopped: $target"
    }

    $display = 'XSM dependency source ' + $service.Substring(9)
    $dependencyDisplay = 'XSM dependency target ' + $dependency.Substring(9)
    Set-Option $service 'DisplayName' @($display)
    Set-Option $dependency 'DisplayName' @($dependencyDisplay)
    Set-Option $service 'DependOnService' @(':' + $dependencyDisplay)
    Assert-Equal (Get-Option $service 'DependOnService') $dependency 'Dependency display name resolves to the canonical service name'
    Assert-Equal ((Read-RegistryValue $service 'DependOnService') -join "`n") $dependency 'SCM registry stores the canonical dependency name'
    Set-Option $service 'DependOnService' @(':' + $dependency.ToLowerInvariant())
    Assert-Equal (Get-Option $service 'DependOnService') $dependency 'Dependency service name is canonicalized case-insensitively'

    foreach ($selfName in @($service, $display)) {
        $before = Read-ConfigSnapshot $service
        Invoke-Xsm @('set', $service, 'DependOnService', (':' + $selfName)) -Reject | Out-Null
        $passed.Add("Self dependency is rejected: $selfName")
        Assert-Equal (Read-ConfigSnapshot $service) $before "Self dependency rejection preserves the complete native configuration: $selfName"
    }

    $beforeSource = Read-ConfigSnapshot $service
    $beforeTarget = Read-ConfigSnapshot $dependency
    # ChangeServiceConfigW documents ERROR_CIRCULAR_DEPENDENCY (1059):
    # https://learn.microsoft.com/en-us/windows/win32/api/winsvc/nf-winsvc-changeserviceconfigw
    # The first edge is A -> B; requesting B -> A must fail without either service changing.
    $cycleError = Invoke-Xsm @('set', $dependency, 'DependOnService', (':' + $service)) -Reject
    $passed.Add('A two-service dependency cycle is rejected')
    if ($cycleError -notmatch '(?<!\d)1059(?!\d)|ERROR_CIRCULAR_DEPENDENCY') {
        throw "Cycle rejection did not identify SCM error 1059: $cycleError"
    }
    $passed.Add('Cycle rejection identifies SCM circular-dependency error 1059')
    Assert-Equal (Read-ConfigSnapshot $service) $beforeSource 'Cycle rejection preserves source service configuration'
    Assert-Equal (Read-ConfigSnapshot $dependency) $beforeTarget 'Cycle rejection preserves target service configuration'

    foreach ($account in @('LocalSystem', 'NT AUTHORITY\LocalService', 'NT AUTHORITY\NetworkService')) {
        Set-Option $service 'ObjectName' @($account)
        # SCM canonicalizes the authority's casing (NT Authority); account
        # identity is case-insensitive and is not a literal text round-trip.
        Assert-Equal ((Get-Option $service 'ObjectName').ToUpperInvariant()) $account.ToUpperInvariant() "Built-in account round-trip without a password: $account"
        Assert-Equal ((Read-RegistryValue $service 'ObjectName').ToUpperInvariant()) $account.ToUpperInvariant() "SCM stores built-in account: $account"
        Assert-Equal (Invoke-Xsm @('status', $service)) 'SERVICE_STOPPED' "Built-in account is configured without starting a service: $account"
    }

    $virtualAccount = 'NT SERVICE\' + $service
    Set-Option $service 'ObjectName' @($virtualAccount)
    Assert-Equal (Get-Option $service 'ObjectName') $virtualAccount 'Current-service virtual account accepts an omitted password'
    Assert-Equal (Read-RegistryValue $service 'ObjectName') $virtualAccount 'SCM stores the current-service virtual account'
    Assert-Equal (Invoke-Xsm @('status', $service)) 'SERVICE_STOPPED' 'Virtual account is configured without starting a service'

    $before = Read-ConfigSnapshot $service
    Invoke-Xsm @('set', $service, 'ObjectName', ('NT SERVICE\' + $dependency)) -Reject | Out-Null
    $passed.Add('Another service virtual account rejects an omitted password')
    Assert-Equal (Read-ConfigSnapshot $service) $before 'Invalid virtual account preserves complete native configuration'
    Assert-Equal (Get-Option $service 'ObjectName') $virtualAccount 'Invalid virtual account preserves the previous account'

    Set-Option $service 'ObjectName' @('LocalSystem')
    Assert-Equal (Get-Option $service 'ObjectName') 'LocalSystem' 'LocalSystem restored after account tests'
    Invoke-Xsm @('reset', $service, 'DependOnService') | Out-Null
    Assert-Equal (Get-Option $service 'DependOnService') '' 'Temporary dependency removed after dependency tests'
    foreach ($target in $targets) {
        Assert-Equal (Invoke-Xsm @('status', $target)) 'SERVICE_STOPPED' "Service stayed stopped throughout configuration tests: $target"
    }
    $completed = $true
} catch {
    $failure = $_
    throw
} finally {
    foreach ($target in $targets) {
        if ($attempted.Contains($target)) {
            try {
                if (Test-ServiceExists $target) {
                    Invoke-Xsm @('remove', $target, 'confirm') | Out-Null
                }
            } catch { $cleanupErrors.Add("Remove ${target}: $_") }
        }
    }
    foreach ($target in $attempted) {
        try {
            $deadline = [DateTime]::UtcNow.AddSeconds(5)
            while (Test-ServiceExists $target) {
                if ([DateTime]::UtcNow -ge $deadline) { throw "Test service survived cleanup: $target" }
                Start-Sleep -Milliseconds 100
            }
            if (Test-Path -LiteralPath ("HKLM:\SYSTEM\CurrentControlSet\Services\" + $target)) {
                throw "Test service registry key survived cleanup: $target"
            }
        } catch { $cleanupErrors.Add($_.ToString()) }
    }
    $completed = $completed -and $cleanupErrors.Count -eq 0
    $evidenceFile = if ($completed) { 'passed.txt' } else { 'partial.txt' }
    [IO.File]::WriteAllLines((Join-Path $results $evidenceFile), $passed.ToArray(), [Text.UTF8Encoding]::new($true))
    ConvertTo-Json -InputObject @($commands.ToArray()) -Depth 5 | Set-Content -LiteralPath (Join-Path $results 'commands.json') -Encoding utf8
    [ordered]@{
        suite = 'gui-settings'; completed = $completed; assertions = $passed.Count
        startedUtc = $startedUtc; finishedUtc = [DateTime]::UtcNow.ToString('o')
        candidate = $Candidate; candidateSha256 = $candidateSha256
        serviceBinary = $serviceBinary; serviceBinarySha256 = $serviceBinarySha256
        probe = $Probe; probeSha256 = $probeSha256
        services = @($targets); cleanupVerified = $cleanupErrors.Count -eq 0
        failure = if ($failure) { $failure.ToString() } else { $null }
        cleanupErrors = @($cleanupErrors.ToArray())
    } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $results 'result.json') -Encoding utf8
    Write-Host "$($passed.Count) GUI settings assertions recorded; completed=$completed; evidence: $results"
    if ($cleanupErrors.Count -and -not $failure) {
        throw ('Cleanup verification failed: ' + ($cleanupErrors -join '; '))
    }
}
