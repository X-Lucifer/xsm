[CmdletBinding()]
param([string]$Candidate = '', [string]$Probe = '')
$ErrorActionPreference = 'Stop'
if (-not $Candidate) { $Candidate = Join-Path $PSScriptRoot '..\target\release\xsm.exe' }
if (-not $Probe) { $Probe = Join-Path $PSScriptRoot '..\target\release\examples\service_probe.exe' }
$Candidate = (Resolve-Path -LiteralPath $Candidate).Path
$Probe = (Resolve-Path -LiteralPath $Probe).Path
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
if (-not ([Security.Principal.WindowsPrincipal]::new($identity)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Elevated PowerShell is required for isolated Xsm_Test_* service tests.' }
$results = Join-Path (Split-Path -Parent $PSScriptRoot) ('test-results\runtime-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $results | Out-Null
$services = [Collections.Generic.List[string]]::new()
$passed = [Collections.Generic.List[string]]::new()
$ownedProcesses = @{}
$completed = $false
$failure = $null
$cleanupErrors = [Collections.Generic.List[string]]::new()
$startedUtc = [DateTime]::UtcNow.ToString('o')
$candidateSha256 = (Get-FileHash -LiteralPath $Candidate -Algorithm SHA256).Hash
$serviceBinary = (Resolve-Path -LiteralPath (Join-Path (Split-Path -Parent $Candidate) 'xsm.exe')).Path
$serviceBinarySha256 = (Get-FileHash -LiteralPath $serviceBinary -Algorithm SHA256).Hash
$probeSha256 = (Get-FileHash -LiteralPath $Probe -Algorithm SHA256).Hash
function Invoke-Xsm([string[]]$Arguments, [int]$Expected = 0) {
    $output = & $Candidate @Arguments 2>&1
    $code = $LASTEXITCODE
    $text = (@($output) -join "`n").Trim()
    if ($code -ne $Expected) { throw "xsm $($Arguments -join ' ') returned $code, expected $Expected`: $text" }
    return $text
}
function Set-Option([string]$Service, [string]$Name, [string[]]$Values) { Invoke-Xsm (@('set', $Service, $Name) + $Values) | Out-Null }
function New-Case([string]$Name, [string]$Parameters) {
    $service = 'Xsm_Test_' + [Guid]::NewGuid().ToString('N')
    $directory = Join-Path $results $Name
    New-Item -ItemType Directory -Path $directory | Out-Null
    Invoke-Xsm @('install', $service, $Probe) | Out-Null
    $services.Add($service)
    Set-Option $service 'Start' @('SERVICE_DEMAND_START')
    Set-Option $service 'AppParameters' @($Parameters)
    Set-Option $service 'AppThrottle' @('100')
    Set-Option $service 'AppStdout' @((Join-Path $directory 'stdout.log'))
    Set-Option $service 'AppStderr' @((Join-Path $directory 'stderr.log'))
    Set-Option $service 'XsmLogPath' @((Join-Path $directory 'internal.log'))
    Set-Option $service 'XsmLogLevel' @('debug')
    return @{ Service = $service; Directory = $directory }
}
function Wait-Until([scriptblock]$Predicate, [string]$Message, [int]$Seconds = 15) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    do { if (& $Predicate) { return }; Start-Sleep -Milliseconds 75 } while ([DateTime]::UtcNow -lt $deadline)
    throw "Timeout: $Message"
}
function Read-Text([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return '' }
    try { $stream = [IO.FileStream]::new($Path,[IO.FileMode]::Open,[IO.FileAccess]::Read,([IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete)) }
    catch [IO.FileNotFoundException] { return '' }
    $reader = [IO.StreamReader]::new($stream,[Text.Encoding]::UTF8,$true)
    try { return $reader.ReadToEnd() } finally { $reader.Dispose() }
}
function Wait-State([string]$Service,[string]$State) { Wait-Until { (Invoke-Xsm @('status',$Service)) -eq $State } "service $Service reaching $State" }
function Assert-True([bool]$Condition,[string]$Message) { if (-not $Condition) { throw $Message }; $passed.Add($Message) }
try {
    $case = New-Case 'throttle' 'exit 7'
    $service = $case.Service
    $gate = Join-Path $case.Directory 'gate'
    Set-Option $service 'AppParameters' @(('gate "{0}"' -f $gate))
    Set-Option $service 'AppRestartDelay' @('10000')
    Invoke-Xsm @('start',$service) | Out-Null
    Wait-State $service 'SERVICE_PAUSED'
    $passed.Add('Rapid application failure enters PAUSED restart backoff')
    [IO.File]::WriteAllText($gate,'open')
    $timer = [Diagnostics.Stopwatch]::StartNew()
    Invoke-Xsm @('continue',$service) | Out-Null
    Wait-State $service 'SERVICE_RUNNING'
    Assert-True ($timer.Elapsed.TotalSeconds -lt 5) 'CONTINUE interrupts the configured 10 second backoff'
    Invoke-Xsm @('stop',$service) | Out-Null
    $log = Read-Text (Join-Path $case.Directory 'internal-latest.log')
    Assert-True ($log -match 'DEBUG' -and $log -notmatch 'TRACE') 'Debug diagnostics contain debug records and exclude trace'
    Assert-True ($log -match 'Service stopped with code 0') 'Console shutdown completes wrapper cleanup and reports a clean stop'
    Set-Option $service 'AppStopMethodConsole' @('0')
    $before = ([regex]::Matches($log,'Service stopped with code 0')).Count
    Invoke-Xsm @('start',$service) | Out-Null
    Invoke-Xsm @('stop',$service) | Out-Null
    $log = Read-Text (Join-Path $case.Directory 'internal-latest.log')
    Assert-True (([regex]::Matches($log,'Service stopped with code 0')).Count -eq ($before + 1)) 'Zero console grace period still completes wrapper cleanup'
    Assert-True (([regex]::Matches($log,'Time \| Level \| Service \| PID \| Operation \| Message')).Count -eq 1) 'Restart resumes the latest log without duplicating its header'
    Assert-True ($log -notmatch 'level=|service=|pid=|operation=|message=') 'Diagnostic records use readable columns without key prefixes'

    $case = New-Case 'environment-ignore' 'report'
    $service = $case.Service
    Set-Option $service 'AppEnvironment' @(('SystemRoot=' + $env:SystemRoot), 'XSM_TEST=base', ('XSM_TEST_ROOT=' + $case.Directory))
    Set-Option $service 'AppEnvironmentExtra' @('XSM_TEST=你好 override', 'XSM_EXTRA=extra')
    Set-Option $service 'AppDirectory' @('%XSM_TEST_ROOT%')
    Set-Option $service 'AppStdout' @('report.log')
    Set-Option $service 'AppExit' @('Default','Ignore')
    $hook = Join-Path $case.Directory 'hooks.log'
    Set-Option $service 'AppEvents' @('Stop/Pre',('"{0}" hook "{1}"' -f $Probe,$hook))
    Invoke-Xsm @('start',$service) | Out-Null
    $report = Join-Path $case.Directory 'report.log'
    Wait-Until { (Read-Text $report) -match 'XSM_EXTRA=extra' } 'environment report output'
    $text = Read-Text $report
    Assert-True ($text.Contains('XSM_TEST=你好 override')) 'Extra environment overrides the replacement block without losing Unicode'
    Assert-True ($text.Contains('cwd=' + $case.Directory)) 'AppDirectory expands against application environment'
    Assert-True (Test-Path -LiteralPath $report) 'Relative stdout path resolves against AppDirectory'
    Assert-True ((Invoke-Xsm @('status',$service)) -eq 'SERVICE_RUNNING') 'Ignore keeps wrapper RUNNING after application exits'
    Assert-True ((Invoke-Xsm @('processes',$service)) -notmatch 'service_probe') 'Ignore leaves no managed application process'
    Invoke-Xsm @('stop',$service) | Out-Null
    Assert-True ((Read-Text $hook) -match 'Stop:Pre') 'Stop/Pre runs when the application already exited under Ignore'

    $case = New-Case 'prestart-abort' 'serve'
    $service = $case.Service
    Set-Option $service 'AppEvents' @('Start/Pre',('"{0}" exit 99' -f $Probe))
    Invoke-Xsm @('start',$service) 1 | Out-Null
    Wait-State $service 'SERVICE_STOPPED'
    Assert-True ((Read-Text (Join-Path $case.Directory 'stdout.log')) -notmatch 'READY') 'Pre-start hook exit 99 prevents application launch'
    Assert-True ((Read-Text (Join-Path $case.Directory 'internal-latest.log')) -match 'aborted') 'Pre-start hook abort is recorded in diagnostics'

    $case = New-Case 'stop-during-prestart' 'serve'
    $service = $case.Service
    $powerShell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    Set-Option $service 'AppEvents' @('Start/Pre',('"{0}" -NoLogo -NoProfile -NonInteractive -Command "Start-Sleep -Seconds 60"' -f $powerShell))
    $starter = Start-Process -FilePath $Candidate -ArgumentList @('start',$service) -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $case.Directory 'start.out') -RedirectStandardError (Join-Path $case.Directory 'start.err')
    try {
        Wait-Until { (Read-Text (Join-Path $case.Directory 'internal-latest.log')) -match 'Hook Start/Pre started' } 'pre-start hook running'
        $timer = [Diagnostics.Stopwatch]::StartNew()
        Invoke-Xsm @('stop',$service) | Out-Null
        Wait-State $service 'SERVICE_STOPPED'
        Assert-True ($timer.Elapsed.TotalSeconds -lt 5) 'STOP interrupts a running synchronous pre-start hook promptly'
        Assert-True ((Read-Text (Join-Path $case.Directory 'stdout.log')) -notmatch 'READY') 'Application is never launched after STOP during pre-start hook'
        if (-not $starter.WaitForExit(5000)) { throw 'Asynchronous start command did not finish after STOP' }
    } finally { if (-not $starter.HasExited) { $starter.Kill() }; $starter.Dispose() }

    $case = New-Case 'process-tree' 'serve'
    $service = $case.Service
    $pidFile = Join-Path $case.Directory 'pids.txt'
    Set-Option $service 'AppParameters' @(('tree "{0}"' -f $pidFile))
    Invoke-Xsm @('start',$service) | Out-Null
    Wait-Until { (Read-Text $pidFile) -match '^\d+\s+\d+' } 'two-generation process tree'
    $ids = @((Read-Text $pidFile).Trim() -split '\s+' | ForEach-Object { [int]$_ })
    foreach ($id in $ids) { $process = Get-Process -Id $id; $ownedProcesses[$id] = $process.StartTime }
    Assert-True ($ids.Count -eq 2 -and $ids[0] -ne $ids[1]) 'Probe started two distinct process generations'
    Invoke-Xsm @('stop',$service) | Out-Null
    foreach ($id in $ids) {
        Wait-Until { $p = Get-Process -Id $id -ErrorAction SilentlyContinue; -not $p -or $p.StartTime -ne $ownedProcesses[$id] } "owned process $id exit"
    }
    $passed.Add('STOP removes application and descendant process without orphaning either')

    $case = New-Case 'exit-spawned-child' 'serve'
    $service = $case.Service
    $latePidFile = Join-Path $case.Directory 'late-child.txt'
    Set-Option $service 'AppParameters' @(('spawn-on-stop "{0}"' -f $latePidFile))
    Invoke-Xsm @('start',$service) | Out-Null
    Assert-True (-not (Test-Path -LiteralPath $latePidFile)) 'Exit-handler child does not exist before the stop signal'
    Invoke-Xsm @('stop',$service) | Out-Null
    $late = @((Read-Text $latePidFile).Trim() -split '\s+')
    if ($late.Count -ne 2) { throw 'Console exit handler did not report the new child identity' }
    $lateId = [int]$late[0]
    $birth = [DateTime]::FromFileTimeUtc([long]$late[1])
    $process = Get-Process -Id $lateId -ErrorAction SilentlyContinue
    if ($process -and $process.StartTime.ToUniversalTime() -eq $birth) { $ownedProcesses[$lateId] = $process.StartTime }
    Assert-True (-not $process -or $process.StartTime.ToUniversalTime() -ne $birth) 'STOP catches a child spawned only inside its parent exit handler'

    $case = New-Case 'skip-stop' 'serve'
    $service = $case.Service
    Set-Option $service 'AppStopMethodSkip' @('15')
    Set-Option $service 'AppKillProcessTree' @('0')
    Set-Option $service 'XsmLogLevel' @('warn')
    Invoke-Xsm @('start',$service) | Out-Null
    $output = Read-Text (Join-Path $case.Directory 'stdout.log')
    if ($output -notmatch 'READY:(\d+):') { throw 'Cannot identify intentional surviving application' }
    $id = [int]$Matches[1]
    $ownedProcesses[$id] = (Get-Process -Id $id).StartTime
    Invoke-Xsm @('stop',$service) | Out-Null
    Assert-True ($null -ne (Get-Process -Id $id -ErrorAction SilentlyContinue)) 'Skipping all stop methods deliberately leaves the application alive'
    $warn = Read-Text (Join-Path $case.Directory 'internal-latest.log')
    Assert-True ($warn -match 'WARN' -and $warn -match 'remains running' -and $warn -notmatch 'INFO|DEBUG|TRACE') 'Warn level records the surviving application without lower priority noise'
    $owned = Get-Process -Id $id
    if ($owned.StartTime -eq $ownedProcesses[$id]) { Stop-Process -Id $id -Force }

    $case = New-Case 'online-logs' 'flood 120'
    $service = $case.Service
    foreach ($name in @('AppRotateFiles','AppRotateOnline','AppTimestampLog')) { Set-Option $service $name @('1') }
    Set-Option $service 'AppRotateBytes' @('256')
    Set-Option $service 'XsmLogLevel' @('trace')
    Invoke-Xsm @('start',$service) | Out-Null
    $getLines = { @(Get-ChildItem -LiteralPath $case.Directory -Filter 'stdout*.log*' | ForEach-Object { (Read-Text $_.FullName) -split '\r?\n' | Where-Object { $_.Length -gt 0 } }) }
    Wait-Until { @(& $getLines).Count -eq 120 } 'all 120 log lines through online rotation'
    Invoke-Xsm @('stop',$service) | Out-Null
    $lines = @(& $getLines)
    $indices = @($lines | ForEach-Object { if ($_ -match '^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3}: (\d{8}):') { [int]$Matches[1] } })
    Assert-True ($indices.Count -eq 120 -and @($indices | Sort-Object -Unique).Count -eq 120 -and ($indices | Measure-Object -Minimum).Minimum -eq 0 -and ($indices | Measure-Object -Maximum).Maximum -eq 119) 'Automatic log rotation preserves every line exactly once with a timestamp'
    Assert-True (@(Get-ChildItem -LiteralPath $case.Directory -Filter 'stdout*.log').Count -gt 1) 'Automatic size threshold creates rotated log files'
    $trace = Read-Text (Join-Path $case.Directory 'internal-latest.log')
    Assert-True ($trace -match 'TRACE' -and $trace -match 'DEBUG' -and $trace -match 'INFO') 'Trace diagnostics include trace, debug and info'
    Assert-True ($trace -notmatch '测试日志') 'Internal trace logs record byte counts without application payload'

    foreach ($level in @('info','error','off')) {
        $case = New-Case ('level-' + $level) 'serve'
        $service = $case.Service
        Set-Option $service 'XsmLogLevel' @($level)
        if ($level -ne 'info') {
            Set-Option $service 'AppEvents' @('Start/Pre',('"{0}" exit 99' -f $Probe))
            Invoke-Xsm @('start',$service) 1 | Out-Null
            Wait-State $service 'SERVICE_STOPPED'
        } else { Invoke-Xsm @('start',$service) | Out-Null; Invoke-Xsm @('stop',$service) | Out-Null }
        $log = Read-Text (Join-Path $case.Directory 'internal-latest.log')
        if ($level -eq 'info') { Assert-True ($log -match 'INFO' -and $log -notmatch 'DEBUG|TRACE') 'Info level excludes debug and trace records' }
        elseif ($level -eq 'error') { Assert-True ($log -match 'ERROR' -and $log -notmatch 'DEBUG|TRACE|INFO|WARN') 'Error level keeps failure diagnostics only' }
        else { Assert-True ([string]::IsNullOrEmpty($log)) 'Off level suppresses diagnostics even on a real startup failure' }
    }
    $completed = $true
} catch {
    $failure = $_
    throw
} finally {
    foreach ($service in $services) {
        try { Invoke-Xsm @('stop', $service) | Out-Null }
        catch { $cleanupErrors.Add("Stop $service`: $_") }
        try { Invoke-Xsm @('remove', $service, 'confirm') | Out-Null }
        catch { $cleanupErrors.Add("Remove $service`: $_") }
    }
    foreach ($entry in $ownedProcesses.GetEnumerator()) {
        try {
            $process = Get-Process -Id $entry.Key -ErrorAction SilentlyContinue
            if ($process -and $process.StartTime -eq $entry.Value) {
                Stop-Process -InputObject $process -Force
                if (-not $process.WaitForExit(5000)) { throw "Owned process did not exit: $($entry.Key)" }
            }
        } catch { $cleanupErrors.Add("Process $($entry.Key): $_") }
    }
    foreach ($service in $services) {
        try {
            $deadline = [DateTime]::UtcNow.AddSeconds(5)
            while (Get-Service -Name $service -ErrorAction SilentlyContinue) {
                if ([DateTime]::UtcNow -ge $deadline) { throw "Test service survived cleanup: $service" }
                Start-Sleep -Milliseconds 100
            }
        } catch { $cleanupErrors.Add($_.ToString()) }
    }
    foreach ($entry in $ownedProcesses.GetEnumerator()) {
        try {
            $process = Get-Process -Id $entry.Key -ErrorAction SilentlyContinue
            if ($process -and $process.StartTime -eq $entry.Value) { throw "Test process survived cleanup: $($entry.Key)" }
        } catch { $cleanupErrors.Add($_.ToString()) }
    }
    $completed = $completed -and $cleanupErrors.Count -eq 0
    $evidenceFile = if ($completed) { 'passed.txt' } else { 'partial.txt' }
    [IO.File]::WriteAllLines((Join-Path $results $evidenceFile), $passed.ToArray(), [Text.UTF8Encoding]::new($true))
    [ordered]@{
        suite = 'runtime'; completed = $completed; assertions = $passed.Count
        startedUtc = $startedUtc; finishedUtc = [DateTime]::UtcNow.ToString('o')
        candidate = $Candidate; candidateSha256 = $candidateSha256; probe = $Probe; probeSha256 = $probeSha256
        serviceBinary = $serviceBinary; serviceBinarySha256 = $serviceBinarySha256
        failure = if ($failure) { $failure.ToString() } else { $null }; cleanupErrors = @($cleanupErrors.ToArray())
    } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $results 'result.json') -Encoding utf8
    Write-Host "$($passed.Count) runtime assertions recorded; completed=$completed; evidence: $results"
    if ($cleanupErrors.Count -and -not $failure) {
        throw ('Cleanup verification failed: ' + ($cleanupErrors -join '; '))
    }
}
