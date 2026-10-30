[CmdletBinding()]
param([string]$Candidate = '', [string]$Probe = '')
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
if (-not $Candidate) { $Candidate = Join-Path $PSScriptRoot '..\target\release\xsm.exe' }
if (-not $Probe) { $Probe = Join-Path $PSScriptRoot '..\target\release\examples\message_probe.exe' }
$Candidate = (Resolve-Path -LiteralPath $Candidate).Path
$Probe = (Resolve-Path -LiteralPath $Probe).Path
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Requires elevation; modifies only new Xsm_Test_* services and their verified probe processes.' }
$results = Join-Path (Split-Path -Parent $PSScriptRoot) ('test-results\shutdown-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $results | Out-Null
$services = [Collections.Generic.List[string]]::new()
$ownedProcesses = [Collections.Generic.List[object]]::new()
$passed = [Collections.Generic.List[string]]::new()
$completed = $false
$failure = $null
$cleanupErrors = [Collections.Generic.List[string]]::new()
$startedUtc = [DateTime]::UtcNow.ToString('o')
$candidateSha256 = (Get-FileHash -LiteralPath $Candidate -Algorithm SHA256).Hash
$serviceBinary = (Resolve-Path -LiteralPath (Join-Path (Split-Path -Parent $Candidate) 'xsm.exe')).Path
$serviceBinarySha256 = (Get-FileHash -LiteralPath $serviceBinary -Algorithm SHA256).Hash
$probeSha256 = (Get-FileHash -LiteralPath $Probe -Algorithm SHA256).Hash
function Quote-WindowsArgument([string]$Value) {
    $builder = [Text.StringBuilder]::new(); [void]$builder.Append('"'); $slashes = 0
    foreach ($character in $Value.ToCharArray()) {
        if ($character -eq '\') { $slashes++; continue }
        $count = if ($character -eq '"') { $slashes * 2 + 1 } else { $slashes }
        if ($count) { [void]$builder.Append(('\' * $count)) }
        [void]$builder.Append($character); $slashes = 0
    }
    if ($slashes) { [void]$builder.Append(('\' * ($slashes * 2))) }
    [void]$builder.Append('"'); return $builder.ToString()
}
function Invoke-Xsm([string[]]$Argv) {
    $info = [Diagnostics.ProcessStartInfo]::new(); $info.FileName = $Candidate
    $info.Arguments = (@($Argv | ForEach-Object { Quote-WindowsArgument $_ }) -join ' ')
    $info.UseShellExecute = $false; $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true; $info.RedirectStandardError = $true
    $info.StandardOutputEncoding = [Text.UTF8Encoding]::new($false); $info.StandardErrorEncoding = [Text.UTF8Encoding]::new($false)
    $process = [Diagnostics.Process]::Start($info)
    try {
        $stdout = $process.StandardOutput.ReadToEndAsync(); $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(30000)) { $process.Kill(); throw 'Shutdown test CLI timed out' }
        $text = ($stdout.GetAwaiter().GetResult() + $stderr.GetAwaiter().GetResult()).Trim()
        $code = $process.ExitCode
    } finally { $process.Dispose() }
    if ($code -ne 0) { throw "Failed ($code): $($Argv -join ' ')`n$text" }
    return $text
}
function Set-Option([string]$Service, [string]$Name, [string[]]$Values) { Invoke-Xsm (@('set', $Service, $Name) + $Values) | Out-Null }
function Read-Marker([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return '' }
    $stream = [IO.FileStream]::new($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, ([IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete))
    $reader = [IO.StreamReader]::new($stream, [Text.Encoding]::UTF8, $true)
    try { return $reader.ReadToEnd() } finally { $reader.Dispose() }
}
function Assert-True([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
    $passed.Add($Message)
}
function Wait-Until([scriptblock]$Predicate, [string]$Message, [int]$Seconds = 15) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    do { if (& $Predicate) { return }; Start-Sleep -Milliseconds 50 } while ([DateTime]::UtcNow -lt $deadline)
    throw "Timeout: $Message"
}
function Get-OwnedProcess($Identity) {
    $process = Get-Process -Id $Identity.Id -ErrorAction SilentlyContinue
    if ($process -and $process.StartTime.ToFileTimeUtc() -eq $Identity.Birth) { return $process }
    return $null
}
function New-Case([string]$Name, [string]$Mode, [int]$Skip) {
    $service = 'Xsm_Test_' + [Guid]::NewGuid().ToString('N')
    $directory = Join-Path $results $Name
    New-Item -ItemType Directory -Path $directory | Out-Null
    $marker = Join-Path $directory 'messages.txt'
    Invoke-Xsm @('install', $service, $Probe) | Out-Null
    $services.Add($service)
    Set-Option $service 'Start' @('SERVICE_DEMAND_START')
    Set-Option $service 'AppParameters' @($Mode + ' ' + (Quote-WindowsArgument $marker))
    Set-Option $service 'AppThrottle' @('100')
    # A console host has its own close behavior; isolate application window/thread delivery.
    Set-Option $service 'AppNoConsole' @('1')
    Set-Option $service 'AppStopMethodSkip' @([string]$Skip)
    Set-Option $service 'AppStopMethodWindow' @('600')
    Set-Option $service 'AppStopMethodThreads' @('600')
    Set-Option $service 'AppStopMethodConsole' @('600')
    Set-Option $service 'AppExit' @('Default', 'Exit')
    Set-Option $service 'XsmLogLevel' @('debug')
    Set-Option $service 'XsmLogPath' @((Join-Path $directory 'internal.log'))
    Set-Option $service 'AppStderr' @((Join-Path $directory 'stderr.log'))
    Invoke-Xsm @('start', $service) | Out-Null
    Wait-Until { (Read-Marker $marker) -match 'READY pid=\d+ created=\d+ session=\d+' } "$Name probe readiness"
    $text = Read-Marker $marker
    if ($text -notmatch 'READY pid=(\d+) created=(\d+) session=(\d+)') { throw 'Invalid probe identity marker' }
    $identity = @{ Id = [int]$Matches[1]; Birth = [long]$Matches[2] }
    $session = [int]$Matches[3]
    $ownedProcesses.Add($identity)
    Assert-True ($session -eq 0) "$Name runs in service session zero"
    Assert-True ($null -ne (Get-OwnedProcess $identity)) "$Name READY matches the live PID and creation time"
    if ($Mode -like 'window*') { Assert-True ($text -match '(?m)^HIDDEN_WINDOW\r?$') "$Name creates only a hidden window" }
    return @{ Name = $Name; Service = $service; Marker = $marker; Identity = $identity }
}
function Stop-Case($Case, [switch]$Survives) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    Invoke-Xsm @('stop', $Case.Service) | Out-Null
    $timer.Stop()
    Assert-True ((Invoke-Xsm @('status', $Case.Service)) -eq 'SERVICE_STOPPED') "$($Case.Name) reaches SERVICE_STOPPED"
    if ($Survives) {
        Assert-True ($null -ne (Get-OwnedProcess $Case.Identity)) "$($Case.Name) preserves the explicitly exempted process"
    } else {
        Wait-Until { $null -eq (Get-OwnedProcess $Case.Identity) } "$($Case.Name) probe exit"
        $passed.Add("$($Case.Name) exits without leaving the original process alive")
    }
    return $timer.Elapsed.TotalMilliseconds
}
try {
    $case = New-Case 'window-only' 'window' 13
    $null = Stop-Case $case
    $text = Read-Marker $case.Marker
    Assert-True ($text -match '(?m)^WINDOW_CLOSE\r?$' -and $text -match '(?m)^EXIT\r?$') 'WM_CLOSE alone cooperatively exits with console, thread and force methods disabled'
    Assert-True ($text -notmatch '(?m)^THREAD_QUIT\r?$') 'Window-only shutdown never requires a thread quit signal'

    $case = New-Case 'thread-only' 'thread' 11
    $null = Stop-Case $case
    $text = Read-Marker $case.Marker
    Assert-True ($text -match '(?m)^THREAD_QUIT\r?$' -and $text -match '(?m)^EXIT\r?$') 'WM_QUIT alone exits a real thread queue with console, window and force methods disabled'

    $case = New-Case 'window-skipped' 'window' 11
    $null = Stop-Case $case
    $text = Read-Marker $case.Marker
    Assert-True ($text -notmatch 'WINDOW_CLOSE|WINDOW_ENDSESSION') 'Window skip bit suppresses both window shutdown messages for an existing top-level window'
    Assert-True ($text -match '(?m)^THREAD_QUIT\r?$' -and $text -match '(?m)^EXIT\r?$') 'Skipping window shutdown still permits the later thread shutdown stage'

    $case = New-Case 'window-timeout-force' 'window-ignore' 5
    $elapsed = Stop-Case $case
    $text = Read-Marker $case.Marker
    Assert-True ($text -match '(?m)^WINDOW_CLOSE\r?$' -and $text -match '(?m)^WINDOW_ENDSESSION\r?$') 'Window timeout probe receives both window notifications and deliberately ignores them'
    Assert-True ($elapsed -ge 450 -and $elapsed -lt 10000) 'Unresponsive window waits for its configured grace period before force fallback'
    Assert-True ($text -notmatch '(?m)^(THREAD_QUIT|EXIT)\r?$') 'Window timeout force fallback skips thread delivery and bypasses cooperative exit'

    $case = New-Case 'window-timeout-thread' 'window-ignore' 9
    $elapsed = Stop-Case $case
    $text = Read-Marker $case.Marker
    Assert-True ($text -match '(?m)^WINDOW_CLOSE\r?$' -and $text -match '(?m)^THREAD_QUIT\r?$' -and $text -match '(?m)^EXIT\r?$') 'Unresponsive window falls through to WM_QUIT and exits while force termination is disabled'
    Assert-True ($elapsed -ge 450 -and $elapsed -lt 10000) 'Window-to-thread fallback observes the window grace period'

    $case = New-Case 'force-only' 'resistant' 7
    $null = Stop-Case $case
    $text = Read-Marker $case.Marker
    Assert-True ($text -notmatch '(?m)^(WINDOW_CLOSE|THREAD_QUIT|EXIT)\r?$') 'Force-only shutdown terminates an infinite sleeping probe without cooperative notifications'

    $case = New-Case 'all-skipped' 'window' 15
    $null = Stop-Case $case -Survives
    Assert-True ((Read-Marker $case.Marker) -notmatch '(?m)^(WINDOW_CLOSE|WINDOW_ENDSESSION|THREAD_QUIT|EXIT)\r?$') 'Skipping every method emits no shutdown notification and deliberately leaves the child alive'
    $survivor = Get-OwnedProcess $case.Identity
    if ($survivor) { $survivor.Kill(); $survivor.WaitForExit(); $survivor.Dispose() }
    $completed = $true
} catch {
    $failure = $_
    throw
} finally {
    foreach ($service in $services) {
        try {
            # Failed assertions must not leave the currently running service exempted from cleanup.
            Set-Option $service 'AppStopMethodSkip' @('7')
            if ((Invoke-Xsm @('status', $service)) -ne 'SERVICE_STOPPED') { Invoke-Xsm @('stop', $service) | Out-Null }
            Invoke-Xsm @('remove', $service, 'confirm') | Out-Null
        } catch { $cleanupErrors.Add("Cleanup of isolated service $service`: $_") }
    }
    foreach ($identity in $ownedProcesses) {
        try {
            $process = Get-OwnedProcess $identity
            if ($process) {
                $process.Kill()
                if (-not $process.WaitForExit(5000)) { throw "Owned process did not exit: $($identity.Id)" }
                $process.Dispose()
            }
        } catch { $cleanupErrors.Add("Process $($identity.Id): $_") }
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
    foreach ($identity in $ownedProcesses) {
        try {
            if (Get-OwnedProcess $identity) { throw "Test process survived cleanup: $($identity.Id)" }
        } catch { $cleanupErrors.Add($_.ToString()) }
    }
    $services | Set-Content -LiteralPath (Join-Path $results 'services.txt') -Encoding utf8
    $completed = $completed -and $cleanupErrors.Count -eq 0
    $evidenceFile = if ($completed) { 'passed.txt' } else { 'partial.txt' }
    [IO.File]::WriteAllLines((Join-Path $results $evidenceFile), $passed.ToArray(), [Text.UTF8Encoding]::new($true))
    [ordered]@{
        suite = 'shutdown'; completed = $completed; assertions = $passed.Count
        startedUtc = $startedUtc; finishedUtc = [DateTime]::UtcNow.ToString('o')
        candidate = $Candidate; candidateSha256 = $candidateSha256; probe = $Probe; probeSha256 = $probeSha256
        serviceBinary = $serviceBinary; serviceBinarySha256 = $serviceBinarySha256
        failure = if ($failure) { $failure.ToString() } else { $null }; cleanupErrors = @($cleanupErrors.ToArray())
    } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $results 'result.json') -Encoding utf8
    Write-Host "$($passed.Count) shutdown assertions recorded; completed=$completed; evidence: $results"
    if ($cleanupErrors.Count -and -not $failure) {
        throw ('Cleanup verification failed: ' + ($cleanupErrors -join '; '))
    }
}
Write-Host 'All isolated shutdown services and verified probe processes were removed.'
