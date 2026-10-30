[CmdletBinding()]
param(
    [string]$Candidate = '',
    [string]$Reference = '',
    [string]$Probe = ''
)
$ErrorActionPreference = 'Stop'
if (-not $Candidate) { $Candidate = Join-Path $PSScriptRoot '..\target\release\xsm.exe' }
if (-not $Probe) { $Probe = Join-Path $PSScriptRoot '..\target\release\examples\service_probe.exe' }
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
if (-not ([Security.Principal.WindowsPrincipal]::new($identity)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Run in an elevated PowerShell. This suite creates and removes only uniquely named Xsm_Test_* services.' }
$Candidate = (Resolve-Path -LiteralPath $Candidate).Path
$Probe = (Resolve-Path -LiteralPath $Probe).Path
$Reference = if ($Reference -and (Test-Path -LiteralPath $Reference)) { (Resolve-Path -LiteralPath $Reference).Path } else { $null }
$service = 'Xsm_Test_' + [Guid]::NewGuid().ToString('N')
$results = Join-Path (Split-Path -Parent $PSScriptRoot) ('test-results\' + $service)
New-Item -ItemType Directory -Path $results -Force | Out-Null
$passed = [Collections.Generic.List[string]]::new()
$created = $false
$completed = $false
$failure = $null
$cleanupErrors = [Collections.Generic.List[string]]::new()
$startedUtc = [DateTime]::UtcNow.ToString('o')
$candidateSha256 = (Get-FileHash -LiteralPath $Candidate -Algorithm SHA256).Hash
$serviceBinary = (Resolve-Path -LiteralPath (Join-Path (Split-Path -Parent $Candidate) 'xsm.exe')).Path
$serviceBinarySha256 = (Get-FileHash -LiteralPath $serviceBinary -Algorithm SHA256).Hash
$probeSha256 = (Get-FileHash -LiteralPath $Probe -Algorithm SHA256).Hash
$referenceSha256 = if ($Reference) { (Get-FileHash -LiteralPath $Reference -Algorithm SHA256).Hash } else { $null }
function Invoke-Xsm([string]$Exe, [string[]]$Arguments, [int]$Expected = 0) {
    $result = & $Exe @Arguments 2>&1
    $code = $LASTEXITCODE
    $text = (@($result | Where-Object { "$_".Trim().Length -gt 0 }) -join "`n").Trim()
    if ($code -ne $Expected) { throw "$Exe $($Arguments -join ' ') returned $code (expected $Expected): $text" }
    return $text
}
function Set-Option([string]$Name, [string[]]$Values) { Invoke-Xsm $Candidate (@('set', $service, $Name) + $Values) | Out-Null }
function Assert-Equal($Actual, $Expected, [string]$Message) {
    if ($Actual -cne $Expected) { throw "$Message`nExpected: [$Expected]`nActual: [$Actual]" }
    $passed.Add($Message)
}
function Wait-File([string]$Path, [string]$Pattern, [int]$Timeout = 15) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Timeout)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ((Test-Path -LiteralPath $Path) -and ((Get-Content -LiteralPath $Path -Raw) -match $Pattern)) { return }
        Start-Sleep -Milliseconds 100
    }
    throw "Timed out waiting for $Pattern in $Path"
}
try {
    Invoke-Xsm $Candidate @('install', $service, $Probe, 'serve') | Out-Null
    $created = $true
    Set-Option 'Start' @('SERVICE_DEMAND_START')
    Assert-Equal (Invoke-Xsm $Candidate @('statuscode', $service) 1) 'SERVICE_STOPPED' 'Stopped statuscode uses exit code 1'
    $settings = @('Application', 'AppDirectory', 'AppParameters', 'AppAffinity', 'AppPriority', 'AppNoConsole', 'AppRestartDelay', 'AppStopMethodSkip', 'AppStopMethodConsole', 'AppStopMethodWindow', 'AppStopMethodThreads', 'AppKillProcessTree', 'AppThrottle', 'AppRedirectHook', 'AppRotateFiles', 'AppRotateOnline', 'AppRotateSeconds', 'AppRotateBytes', 'AppRotateBytesHigh', 'AppRotateDelay', 'AppTimestampLog', 'AppStdinShareMode', 'AppStdinCreationDisposition', 'AppStdinFlagsAndAttributes', 'AppStdoutShareMode', 'AppStdoutCreationDisposition', 'AppStdoutFlagsAndAttributes', 'AppStdoutCopyAndTruncate', 'AppStderrShareMode', 'AppStderrCreationDisposition', 'AppStderrFlagsAndAttributes', 'AppStderrCopyAndTruncate', 'Start', 'Type', 'Name', 'ObjectName', 'DisplayName', 'Description')
    if ($Reference) {
        foreach ($setting in $settings) {
            Assert-Equal (Invoke-Xsm $Candidate @('get', $service, $setting)) (Invoke-Xsm $Reference @('get', $service, $setting)) "Reference default: $setting"
        }
    }
    $cases = @(
        @{ Name = 'AppParameters'; Values = @('serve') },
        @{ Name = 'AppEnvironmentExtra'; Values = @('XSM_TEST=test value', 'XSM_EXTRA=second') },
        @{ Name = 'AppExit'; Values = @('Default', 'Restart') },
        @{ Name = 'AppExit'; Values = @('42', 'Exit') },
        @{ Name = 'AppPriority'; Values = @('BELOW_NORMAL_PRIORITY_CLASS') },
        @{ Name = 'Description'; Values = @('Rust XSM integration') },
        @{ Name = 'DisplayName'; Values = @($service + ' Display') },
        @{ Name = 'AppThrottle'; Values = @('250') }
    )
    foreach ($case in $cases) {
        Set-Option $case.Name $case.Values
        $additional = if ($case.Name -eq 'AppExit') { @($case.Values[0]) } else { @() }
        if ($Reference) { Assert-Equal (Invoke-Xsm $Candidate (@('get', $service, $case.Name) + $additional)) (Invoke-Xsm $Reference (@('get', $service, $case.Name) + $additional)) "Reference configured: $($case.Name) $additional" }
    }
    Set-Option 'AppEnvironmentExtra' @('XSM_TEST=测试 value', 'XSM_EXTRA=second')
    Assert-Equal (Invoke-Xsm $Candidate @('get', $service, 'AppEnvironmentExtra', 'XSM_TEST')) '测试 value' 'Unicode environment value round-trip'
    $stdout = Join-Path $results 'stdout.log'
    $stderr = Join-Path $results 'stderr.log'
    $hooks = Join-Path $results 'hooks.log'
    $internal = Join-Path $results 'internal-latest.log'
    Set-Option 'XsmLogLevel' @('debug')
    Set-Option 'XsmLogPath' @((Join-Path $results 'internal.log'))
    Set-Option 'AppStdout' @($stdout)
    Set-Option 'AppStderr' @($stderr)
    Set-Option 'AppEvents' @('Start/Pre', ('"{0}" hook "{1}"' -f $Probe, $hooks))
    Set-Option 'AppEvents' @('Stop/Pre', ('"{0}" hook "{1}"' -f $Probe, $hooks))
    Invoke-Xsm $Candidate @('start', $service) | Out-Null
    Wait-File $stdout 'READY:.*:测试 value'
    Wait-File $stderr 'STDERR:'
    Wait-File $hooks 'Start:Pre'
    Wait-File $internal 'DEBUG|INFO'
    $passed.Add('Internal runtime log honors configured file sink')
    Assert-Equal (Invoke-Xsm $Candidate @('statuscode', $service) 4) 'SERVICE_RUNNING' 'Running statuscode uses exit code 4'
    Assert-Equal (Invoke-Xsm $Candidate @('status', ($service + ' Display'))) 'SERVICE_RUNNING' 'Display-name lookup'
    $tree = Invoke-Xsm $Candidate @('processes', $service)
    if ($tree -notmatch 'service_probe') { throw "Managed process absent from process tree: $tree" }
    $passed.Add('Managed process enumeration')
    Invoke-Xsm $Candidate @('stop', $service) | Out-Null
    Wait-File $hooks 'Stop:Pre'
    Assert-Equal (Invoke-Xsm $Candidate @('status', $service)) 'SERVICE_STOPPED' 'Stop reaches STOPPED'
    Set-Option 'AppParameters' @('exit 42')
    # The process exits before START reaches RUNNING; service exit code is still 42.
    Invoke-Xsm $Candidate @('start', $service) 1 | Out-Null
    Start-Sleep -Milliseconds 750
    Assert-Equal (Invoke-Xsm $Candidate @('status', $service)) 'SERVICE_STOPPED' 'Per-exit-code Exit action'
    Set-Option 'AppParameters' @('serve')
    Set-Option 'AppRotateFiles' @('1')
    Set-Option 'AppRotateOnline' @('1')
    Invoke-Xsm $Candidate @('start', $service) | Out-Null
    Invoke-Xsm $Candidate @('rotate', $service) | Out-Null
    Invoke-Xsm $Candidate @('restart', $service) | Out-Null
    Assert-Equal (Invoke-Xsm $Candidate @('status', $service)) 'SERVICE_RUNNING' 'Restart reaches RUNNING'
    Invoke-Xsm $Candidate @('stop', $service) | Out-Null
    $dump = Invoke-Xsm $Candidate @('dump', $service)
    if ($dump -notmatch 'install' -or $dump -notmatch 'AppEvents' -or $dump -notmatch 'AppEnvironmentExtra') { throw 'Configuration dump is incomplete' }
    $dump | Set-Content -LiteralPath (Join-Path $results 'dump.cmd') -Encoding utf8
    $passed.Add('Configuration dump contains application, hooks and environment')
    if ($Reference) {
        Invoke-Xsm $Reference @('set', $service, 'AppRestartDelay', '321') | Out-Null
        Assert-Equal (Invoke-Xsm $Candidate @('get', $service, 'AppRestartDelay')) '321' 'Read reference-written registry value'
    }
    Invoke-Xsm $Candidate @('reset', $service, 'AppRestartDelay') | Out-Null
    Assert-Equal (Invoke-Xsm $Candidate @('get', $service, 'AppRestartDelay')) '0' 'Reset restores default'
    $completed = $true
} catch {
    $failure = $_
    throw
} finally {
    if ($created) {
        try { Invoke-Xsm $Candidate @('stop', $service) | Out-Null }
        catch { $cleanupErrors.Add("Stop $service`: $_") }
        try { Invoke-Xsm $Candidate @('remove', $service, 'confirm') | Out-Null }
        catch { $cleanupErrors.Add("Remove $service`: $_") }
    }
    try {
        $deadline = [DateTime]::UtcNow.AddSeconds(5)
        while (Get-Service -Name $service -ErrorAction SilentlyContinue) {
            if ([DateTime]::UtcNow -ge $deadline) { throw "Test service survived cleanup: $service" }
            Start-Sleep -Milliseconds 100
        }
    } catch { $cleanupErrors.Add($_.ToString()) }
    $completed = $completed -and $cleanupErrors.Count -eq 0
    $evidenceFile = if ($completed) { 'passed.txt' } else { 'partial.txt' }
    [IO.File]::WriteAllLines((Join-Path $results $evidenceFile), $passed.ToArray(), [Text.UTF8Encoding]::new($true))
    [ordered]@{
        suite = 'integration'; completed = $completed; assertions = $passed.Count
        startedUtc = $startedUtc; finishedUtc = [DateTime]::UtcNow.ToString('o')
        candidate = $Candidate; candidateSha256 = $candidateSha256; probe = $Probe; probeSha256 = $probeSha256
        serviceBinary = $serviceBinary; serviceBinarySha256 = $serviceBinarySha256
        reference = $Reference; referenceSha256 = $referenceSha256
        failure = if ($failure) { $failure.ToString() } else { $null }; cleanupErrors = @($cleanupErrors.ToArray())
    } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $results 'result.json') -Encoding utf8
    Write-Host "$($passed.Count) assertions recorded; completed=$completed; evidence: $results"
    if ($cleanupErrors.Count -and -not $failure) {
        throw ('Cleanup verification failed: ' + ($cleanupErrors -join '; '))
    }
}
