[CmdletBinding()]
param([string]$Candidate = '', [string]$Probe = '', [string]$ServiceBinary = '')
$ErrorActionPreference = 'Stop'
if (-not $Candidate) { $Candidate = Join-Path $PSScriptRoot '..\target\release\xsm.exe' }
if (-not $Probe) { $Probe = Join-Path $PSScriptRoot '..\target\release\examples\service_probe.exe' }
$Candidate = (Resolve-Path -LiteralPath $Candidate).Path
$Probe = (Resolve-Path -LiteralPath $Probe).Path
if (-not $ServiceBinary) { $ServiceBinary = Join-Path (Split-Path -Parent $Candidate) 'xsm.exe' }
$ServiceBinary = (Resolve-Path -LiteralPath $ServiceBinary).Path
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
if (-not ([Security.Principal.WindowsPrincipal]::new($identity)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Elevated PowerShell is required for isolated Xsm_Test_* service tests.' }
$results = Join-Path (Split-Path -Parent $PSScriptRoot) ('test-results\multi-service-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $results | Out-Null
$cases = [Collections.Generic.List[object]]::new()
$services = [Collections.Generic.List[string]]::new()
$passed = [Collections.Generic.List[string]]::new()
$ownedProcesses = @{}
$cleanupErrors = [Collections.Generic.List[string]]::new()
$guiProcess = $null
$guiLifecycleVerified = $false
$noArgumentsExitVerified = $false
$guiModesVerified = [Collections.Generic.List[string]]::new()
$guiIdleSamples = [Collections.Generic.List[object]]::new()
$completed = $false
$failure = $null
$startedUtc = [DateTime]::UtcNow.ToString('o')
$candidateSha256 = (Get-FileHash -LiteralPath $Candidate -Algorithm SHA256).Hash
$serviceBinarySha256 = (Get-FileHash -LiteralPath $ServiceBinary -Algorithm SHA256).Hash
$probeSha256 = (Get-FileHash -LiteralPath $Probe -Algorithm SHA256).Hash
$hookPairs = @('Start/Pre','Start/Post','Stop/Pre','Exit/Post','Power/Change','Power/Resume','Rotate/Pre','Rotate/Post')
function Assert-True([bool]$Condition,[string]$Message) { if (-not $Condition) { throw $Message }; $passed.Add($Message) }
function Invoke-Xsm([string[]]$Arguments,[int]$Expected = 0) {
    $output = & $Candidate @Arguments 2>&1
    $code = $LASTEXITCODE
    $text = (@($output) -join "`n").Trim()
    if ($code -ne $Expected) { throw "xsm $($Arguments -join ' ') returned $code, expected $Expected`: $text" }
    return $text
}
function Set-Option([string]$Service,[string]$Name,[string[]]$Values) { Invoke-Xsm (@('set',$Service,$Name) + $Values) | Out-Null }
function Wait-Until([scriptblock]$Predicate,[string]$Message,[int]$Seconds = 20) {
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
function Read-Records([string]$Directory) {
    if (-not (Test-Path -LiteralPath $Directory)) { return }
    foreach ($file in Get-ChildItem -LiteralPath $Directory -Filter '*.record') {
        $data = @{}
        foreach ($line in (Read-Text $file.FullName) -split '\r?\n') {
            if ($line.Length -eq 0) { continue }
            $parts = $line.Split('=',2)
            if ($parts.Count -ne 2 -or $data.ContainsKey($parts[0])) { throw "Invalid snapshot: $($file.FullName)" }
            $data[$parts[0]] = $parts[1]
        }
        [pscustomobject]$data
    }
}
function Hooks($Case,[string]$Pair) {
    @(Read-Records $Case.Hooks | Where-Object { "$($_.XSM_EVENT)/$($_.XSM_ACTION)" -eq $Pair })
}
function Output-Text($Case,[string]$Stem = 'stdout') {
    (@(Get-ChildItem -LiteralPath $Case.Directory -Filter "$Stem*.log*" | ForEach-Object { Read-Text $_.FullName }) -join "`n")
}
function Get-Sequence($Case,[int]$ApplicationId) {
    $prefix = 'APP|' + $Case.Token + '|' + $ApplicationId + '|'
    $numbers = @((Output-Text $Case) -split '\r?\n' | Where-Object { $_.StartsWith($prefix) } | ForEach-Object { [long]$_.Substring($prefix.Length) })
    if ($numbers.Count) { return ($numbers | Measure-Object -Maximum).Maximum }
    return -1
}
function Track-Identity([string]$IdValue,[string]$BirthValue,[string]$ExpectedPath) {
    if (-not $IdValue -or -not $BirthValue) { return }
    $id = [int]$IdValue
    $process = Get-Process -Id $id -ErrorAction SilentlyContinue
    if ($process -and $process.StartTime.ToUniversalTime().ToFileTimeUtc() -eq [long]$BirthValue) {
        if ($process.Path -ne $ExpectedPath) { throw "Unexpected process identity for test PID $id" }
        $ownedProcesses[$id] = $process.StartTime
    }
}
function Track-Processes($Case) {
    foreach ($record in @(Read-Records $Case.Apps) + @(Read-Records $Case.Hooks)) {
        Track-Identity $record.PROBE_PID $record.PROBE_BIRTH_FILETIME $Probe
        Track-Identity $record.XSM_PID $record.XSM_WRAPPER_BIRTH_FILETIME $ServiceBinary
        Track-Identity $record.XSM_APPLICATION_PID $record.XSM_APPLICATION_BIRTH_FILETIME $Probe
    }
}
function Parallel-Control([object[]]$Targets,[string]$Command) {
    $clients = [Collections.Generic.List[object]]::new()
    try {
        foreach ($case in $Targets) {
            $info = [Diagnostics.ProcessStartInfo]::new($Candidate)
            $info.UseShellExecute = $false
            $info.CreateNoWindow = $true
            $info.RedirectStandardOutput = $true
            $info.RedirectStandardError = $true
            $info.ArgumentList.Add($Command)
            $info.ArgumentList.Add($case.Service)
            $process = [Diagnostics.Process]::Start($info)
            $clients.Add(@{ Process=$process; Output=$process.StandardOutput.ReadToEndAsync(); Error=$process.StandardError.ReadToEndAsync(); Service=$case.Service })
        }
        foreach ($client in $clients) {
            if (-not $client.Process.WaitForExit(30000)) { throw "Concurrent $Command timed out: $($client.Service)" }
            if ($client.Process.ExitCode -ne 0) { throw "Concurrent $Command failed for $($client.Service): $($client.Output.Result) $($client.Error.Result)" }
        }
    } finally {
        foreach ($client in $clients) {
            try { if (-not $client.Process.HasExited) { $client.Process.Kill(); $client.Process.WaitForExit() } }
            finally { $client.Process.Dispose() }
        }
    }
}
function New-Case([string]$Name,[bool]$Abort = $false) {
    $service = 'Xsm_Test_' + [Guid]::NewGuid().ToString('N')
    $directory = Join-Path $results $Name
    New-Item -ItemType Directory -Path $directory | Out-Null
    $case = @{ Name=$Name; Service=$service; Directory=$directory; Hooks=(Join-Path $directory 'hooks'); Apps=(Join-Path $directory 'applications'); Marker=(Join-Path $directory 'restart.marker'); Token=($Name + '_并行_' + [Guid]::NewGuid().ToString('N')); SharedValue=($Name + '_独立环境'); Abort=$Abort }
    $cases.Add($case)
    Invoke-Xsm @('install',$service,$Probe) | Out-Null
    $services.Add($service)
    $image = Invoke-Xsm @('get',$service,'ImagePath')
    Assert-True ([IO.Path]::GetFullPath($image.Trim('"')) -eq $ServiceBinary) "$Name installed through the CLI starts the main xsm.exe service binary"
    Set-Option $service 'Start' @('SERVICE_DEMAND_START')
    Set-Option $service 'AppParameters' @(('isolation "{0}" "{1}"' -f $case.Apps,$case.Marker))
    Set-Option $service 'AppDirectory' @($directory)
    Set-Option $service 'AppEnvironmentExtra' @(('XSM_TEST_TOKEN=' + $case.Token), ('XSM_SAME_NAME=' + $case.SharedValue))
    Set-Option $service 'AppThrottle' @('100')
    Set-Option $service 'AppStdout' @((Join-Path $directory 'stdout.log'))
    Set-Option $service 'AppStderr' @((Join-Path $directory 'stderr.log'))
    Set-Option $service 'XsmLogPath' @((Join-Path $directory 'internal.log'))
    Set-Option $service 'XsmLogLevel' @('trace')
    foreach ($setting in @('AppRotateFiles','AppRotateOnline','AppRedirectHook')) { Set-Option $service $setting @('1') }
    foreach ($pair in $hookPairs) {
        $code = if ($Abort -and $pair -eq 'Start/Pre') { 99 } else { 0 }
        $command = '"{0}" snapshot "{1}" {2} 200' -f $Probe,$case.Hooks,$code
        Set-Option $service 'AppEvents' @($pair,$command)
        Assert-True ((Invoke-Xsm @('get',$service,'AppEvents',$pair)) -ceq $command) "$Name has its own $pair hook configuration"
    }
    Assert-True ((Invoke-Xsm @('get',$service,'Type')) -eq 'SERVICE_WIN32_OWN_PROCESS') "$Name runs in an independent SCM process"
    return $case
}
function Assert-Peers($Peers,[string]$Operation) {
    foreach ($peer in $Peers) {
        Assert-True ((Invoke-Xsm @('status',$peer.Service)) -eq 'SERVICE_RUNNING') "$($peer.Name) stays RUNNING during $Operation"
        $live = Get-Process -Id $peer.ApplicationId -ErrorAction SilentlyContinue
        Assert-True ($live -and $live.StartTime -eq $peer.ApplicationBirth) "$($peer.Name) keeps the same live application during $Operation"
        $wrapper = Get-Process -Id $peer.WrapperId -ErrorAction SilentlyContinue
        Assert-True ($wrapper -and $wrapper.StartTime -eq $peer.WrapperBirth) "$($peer.Name) keeps its independent wrapper during $Operation"
        $before = Get-Sequence $peer $peer.ApplicationId
        Wait-Until { (Get-Sequence $peer $peer.ApplicationId) -gt $before } "$($peer.Name) producing output after $Operation" 5
        $passed.Add("$($peer.Name) continues producing output during $Operation")
        Assert-True (@(Hooks $peer 'Start/Post').Count -eq 1 -and @(Hooks $peer 'Stop/Pre').Count -eq 0) "$($peer.Name) has no extra start/stop hooks during $Operation"
    }
}
function Assert-GuiLifecycleIsolation($Peers) {
    if (-not ('Xsm.MultiServiceWindowProbe' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
namespace Xsm {
    public static class MultiServiceWindowProbe {
        private delegate bool Callback(IntPtr window, IntPtr data);
        [DllImport("user32.dll")] private static extern bool EnumWindows(Callback callback, IntPtr data);
        [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
        [DllImport("user32.dll", CharSet=CharSet.Unicode)] private static extern int GetClassNameW(IntPtr window, StringBuilder name, int capacity);
        [DllImport("user32.dll", CharSet=CharSet.Unicode)] private static extern int GetWindowTextW(IntPtr window, StringBuilder text, int capacity);
        [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr window);
        [DllImport("user32.dll", SetLastError=true)] private static extern bool PostMessageW(IntPtr window, uint message, UIntPtr wparam, IntPtr lparam);
        [DllImport("user32.dll", SetLastError=true)] private static extern IntPtr SendMessageTimeoutW(IntPtr window, uint message, UIntPtr wparam, IntPtr lparam, uint flags, uint timeout, out UIntPtr result);
        private static bool Owned(IntPtr window, uint process) {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner != process) return false;
            var name = new StringBuilder(128);
            if (GetClassNameW(window, name, name.Capacity) <= 0 || name.ToString() == "ConsoleWindowClass") return false;
            var title = new StringBuilder(512);
            return GetWindowTextW(window, title, title.Capacity) > 0 && title.ToString().StartsWith("XSM", StringComparison.Ordinal);
        }
        public static IntPtr Find(uint process) {
            IntPtr found = IntPtr.Zero;
            EnumWindows((window, data) => {
                if (!Owned(window, process)) return true;
                found = window;
                return false;
            }, IntPtr.Zero);
            return found;
        }
        public static bool Close(IntPtr window, uint process) {
            return Owned(window, process) && PostMessageW(window, 0x0010, UIntPtr.Zero, IntPtr.Zero);
        }
        public static bool Responsive(IntPtr window, uint process) {
            if (!Owned(window, process)) return false;
            UIntPtr result;
            return SendMessageTimeoutW(window, 0, UIntPtr.Zero, IntPtr.Zero, 3, 10000, out result) != IntPtr.Zero;
        }
    }
}
'@
    }
    # The shipped console executable is also the service host. Test interactive
    # no-argument exit without creating a visible terminal or touching user I/O.
    $image = [IO.File]::ReadAllBytes($ServiceBinary)
    if ($image.Length -lt 64 -or [BitConverter]::ToUInt16($image,0) -ne 0x5A4D) { throw 'Invalid main executable DOS header' }
    $pe = [BitConverter]::ToInt32($image,0x3c)
    if ($pe -lt 0 -or $pe -gt $image.Length - 94 -or [BitConverter]::ToUInt32($image,$pe) -ne 0x4550) { throw 'Invalid main executable PE header' }
    Assert-True ([BitConverter]::ToUInt16($image,($pe + 4 + 20 + 68)) -eq 3) 'The single executable uses the console PE subsystem'
    $noArguments = [Diagnostics.ProcessStartInfo]::new($ServiceBinary)
    $noArguments.UseShellExecute = $false
    $noArguments.CreateNoWindow = $true
    $noArguments.RedirectStandardOutput = $true
    $noArguments.RedirectStandardError = $true
    $noArguments.EnvironmentVariables.Remove('XSM_LOG_FILE')
    $noArguments.EnvironmentVariables['XSM_LOG_LEVEL'] = 'off'
    $script:guiProcess = [Diagnostics.Process]::Start($noArguments)
    $ownedProcesses[$guiProcess.Id] = $guiProcess.StartTime
    $noArgumentsOutput = $guiProcess.StandardOutput.ReadToEndAsync()
    $noArgumentsError = $guiProcess.StandardError.ReadToEndAsync()
    Assert-True ($guiProcess.WaitForExit(5000) -and $guiProcess.ExitCode -eq 0) 'Interactive no-argument invocation exits promptly without opening the management GUI'
    Assert-True ($noArgumentsOutput.GetAwaiter().GetResult().Length -eq 0 -and $noArgumentsError.GetAwaiter().GetResult().Length -eq 0) 'Interactive no-argument invocation produces no output'
    $guiProcess.Dispose()
    $script:guiProcess = $null
    $script:noArgumentsExitVerified = $true
    Assert-Peers $Peers 'the interactive no-argument invocation'
    foreach ($mode in @('install')) {
        # STARTUPINFO requests a hidden window from its first ShowWindow; never
        # show, focus, or send messages to existing user-owned windows.
        $launch = @{ FilePath=$ServiceBinary; WindowStyle='Hidden'; PassThru=$true }
        $launch.ArgumentList = 'install'
        $script:guiProcess = Start-Process @launch
        $ownedProcesses[$guiProcess.Id] = $guiProcess.StartTime
        Assert-True ($guiProcess.Id -notin @($Peers | ForEach-Object { $_.WrapperId })) "$mode GUI is distinct from all service wrapper processes"
        Wait-Until {
            if ($guiProcess.HasExited) { throw "$mode GUI exited before creating its window: $($guiProcess.ExitCode)" }
            [Xsm.MultiServiceWindowProbe]::Find([uint32]$guiProcess.Id) -ne [IntPtr]::Zero
        } "the newly created hidden $mode GUI window" 20
        $window = [Xsm.MultiServiceWindowProbe]::Find([uint32]$guiProcess.Id)
        Assert-True (-not [Xsm.MultiServiceWindowProbe]::IsWindowVisible($window)) "$mode GUI starts hidden without touching user windows"
        Assert-True ([Xsm.MultiServiceWindowProbe]::Responsive($window,[uint32]$guiProcess.Id)) "$mode GUI message loop responds while services are running"
        # WM_NULL confirms the window message loop responds for a console-subsystem
        # process too. Allow the initial
        # asynchronous service enumeration to settle before the short sample.
        Start-Sleep -Milliseconds 2000
        $guiProcess.Refresh()
        $cpuBefore = $guiProcess.TotalProcessorTime.TotalMilliseconds
        $sampleStarted = [DateTime]::UtcNow.ToString('o')
        $sampleTimer = [Diagnostics.Stopwatch]::StartNew()
        Start-Sleep -Milliseconds 1000
        $guiProcess.Refresh()
        if ($guiProcess.HasExited) { throw "$mode GUI exited during idle sampling" }
        $sampleTimer.Stop()
        $elapsedMs = $sampleTimer.Elapsed.TotalMilliseconds
        $cpuDeltaMs = $guiProcess.TotalProcessorTime.TotalMilliseconds - $cpuBefore
        $processors = [Environment]::ProcessorCount
        $hiddenAfterSample = -not [Xsm.MultiServiceWindowProbe]::IsWindowVisible($window)
        $guiIdleSamples.Add([ordered]@{
            mode=$mode; pid=$guiProcess.Id; creationUtc=$guiProcess.StartTime.ToUniversalTime().ToString('o')
            startedUtc=$sampleStarted; finishedUtc=[DateTime]::UtcNow.ToString('o'); hidden=$hiddenAfterSample
            elapsedMilliseconds=[Math]::Round($elapsedMs,3)
            stabilizationMilliseconds=2000
            cpuMilliseconds=[Math]::Round($cpuDeltaMs,3)
            oneCoreCpuPercent=[Math]::Round((100 * $cpuDeltaMs / $elapsedMs),3)
            normalizedCpuPercent=[Math]::Round((100 * $cpuDeltaMs / $elapsedMs / $processors),3)
            logicalProcessorCount=$processors; privateBytes=$guiProcess.PrivateMemorySize64
            workingSetBytes=$guiProcess.WorkingSet64; handleCount=$guiProcess.HandleCount; threadCount=$guiProcess.Threads.Count
            context='Approximately one second of hidden-window sampling after a bounded WM_NULL response and a two-second stabilization wait, with three running services; descriptive evidence, no absolute performance threshold'
        })
        Assert-True $hiddenAfterSample "$mode GUI remains hidden during the idle sample"
        Assert-True ([Xsm.MultiServiceWindowProbe]::Close($window,[uint32]$guiProcess.Id)) "$mode WM_CLOSE targets only the test-owned GUI window"
        Assert-True ($guiProcess.WaitForExit(10000) -and $guiProcess.ExitCode -eq 0) "$mode GUI closes normally without stopping any service"
        $guiProcess.Dispose()
        $script:guiProcess = $null
        Assert-Peers $Peers "opening and normally closing the $mode GUI"
        $guiModesVerified.Add($mode)
    }
    $script:guiLifecycleVerified = $guiModesVerified.Count -eq 1
}
try {
    $a = New-Case 'A'
    $b = New-Case 'B'
    $c = New-Case 'C'
    Parallel-Control @($a,$b,$c) 'start'
    foreach ($case in @($a,$b,$c)) {
        Wait-Until { @(Hooks $case 'Start/Post').Count -eq 1 } "$($case.Name) initial Start/Post"
        $post = @(Hooks $case 'Start/Post')[0]
        $case.ApplicationId = [int]$post.XSM_APPLICATION_PID
        $case.WrapperId = [int]$post.XSM_PID
        $case.ApplicationFileTime = $post.XSM_APPLICATION_BIRTH_FILETIME
        $case.WrapperFileTime = $post.XSM_WRAPPER_BIRTH_FILETIME
        $case.ApplicationBirth = (Get-Process -Id $case.ApplicationId).StartTime
        $case.WrapperBirth = (Get-Process -Id $case.WrapperId).StartTime
        Track-Processes $case
        $scm = Get-CimInstance Win32_Service -Filter "Name='$($case.Service)'"
        Assert-True ($scm.ProcessId -eq $case.WrapperId -and $case.WrapperId -ne $case.ApplicationId) "$($case.Name) hook wrapper PID matches its own SCM process"
        Assert-True ($post.XSM_START_COUNT -eq '1' -and $post.XSM_EXIT_COUNT -eq '0') "$($case.Name) starts with independent counters"
    }
    Assert-True (@(@($a.WrapperId,$b.WrapperId,$c.WrapperId) | Sort-Object -Unique).Count -eq 3) 'Three concurrent services have three distinct wrapper processes'
    Assert-True (@(@($a.ApplicationId,$b.ApplicationId,$c.ApplicationId) | Sort-Object -Unique).Count -eq 3) 'Three concurrent services have three distinct applications'
    Assert-GuiLifecycleIsolation @($a,$b,$c)
    Invoke-Xsm @('rotate',$a.Service) | Out-Null
    Wait-Until { @(Hooks $a 'Rotate/Pre').Count -eq 1 -and @(Hooks $a 'Rotate/Post').Count -eq 1 -and @(Get-ChildItem -LiteralPath $a.Directory -Filter 'stdout*.log').Count -gt 1 } 'A manual rotation and both hooks'
    Assert-True (@(Hooks $b 'Rotate/Pre').Count -eq 0 -and @(Hooks $c 'Rotate/Post').Count -eq 0) 'Rotating A does not invoke hooks in B or C'
    foreach ($peer in @($b,$c)) { Assert-True (@(Get-ChildItem -LiteralPath $peer.Directory -Filter 'stdout*.log').Count -eq 1) "$($peer.Name) stdout is not rotated by A" }
    Assert-Peers @($b,$c) 'A rotation'
    [IO.File]::WriteAllText($a.Marker,'restart')
    Wait-Until { @(Hooks $a 'Start/Post').Count -eq 2 } 'A application restart'
    $second = @(Hooks $a 'Start/Post' | Where-Object { $_.XSM_START_COUNT -eq '2' })
    Assert-True ($second.Count -eq 1 -and $second[0].XSM_PID -eq [string]$a.WrapperId -and $second[0].XSM_WRAPPER_BIRTH_FILETIME -eq $a.WrapperFileTime -and ($second[0].XSM_APPLICATION_PID -ne [string]$a.ApplicationId -or $second[0].XSM_APPLICATION_BIRTH_FILETIME -ne $a.ApplicationFileTime) -and $second[0].XSM_EXIT_COUNT -eq '1') 'A application restart advances only its own counters and application identity'
    Track-Processes $a
    Assert-Peers @($b,$c) 'A application restart'
    Invoke-Xsm @('restart',$a.Service) | Out-Null
    Wait-Until { @(Hooks $a 'Start/Post').Count -eq 3 } 'A wrapper restart'
    $fresh = @(Hooks $a 'Start/Post' | Where-Object { $_.XSM_PID -ne [string]$a.WrapperId -or $_.XSM_WRAPPER_BIRTH_FILETIME -ne $a.WrapperFileTime })
    Assert-True ($fresh.Count -eq 1 -and $fresh[0].XSM_START_COUNT -eq '1' -and $fresh[0].XSM_EXIT_COUNT -eq '0') 'Restarting A creates a fresh wrapper and resets only its own counters'
    Track-Processes $a
    Assert-Peers @($b,$c) 'A service restart'
    Invoke-Xsm @('stop',$a.Service) | Out-Null
    Assert-True ((Invoke-Xsm @('status',$a.Service)) -eq 'SERVICE_STOPPED') 'A can stop independently'
    Assert-Peers @($b,$c) 'A stop'
    $d = New-Case 'D-abort' $true
    Invoke-Xsm @('start',$d.Service) 1 | Out-Null
    Assert-True ((Invoke-Xsm @('status',$d.Service)) -eq 'SERVICE_STOPPED') 'D Start/Pre exit 99 stops only the rejected service'
    $pre = @(Hooks $d 'Start/Pre')
    Assert-True ($pre.Count -eq 1 -and $pre[0].XSM_APPLICATION_PID -eq '' -and $pre[0].XSM_START_COUNT -eq '0') 'D rejected hook has no application PID or successful-start counter'
    Assert-True (@(Read-Records $d.Apps).Count -eq 0 -and @(Hooks $d 'Start/Post').Count -eq 0) 'D rejection never launches its application or post-start hook'
    Assert-Peers @($b,$c) 'D hook rejection'
    Parallel-Control @($b,$c) 'stop'
    foreach ($case in @($a,$b,$c)) {
        foreach ($pair in @('Start/Pre','Start/Post','Stop/Pre','Exit/Post')) { Assert-True (@(Hooks $case $pair).Count -ge 1) "$($case.Name) executes its own $pair hook" }
    }
    foreach ($case in $cases) {
        $records = @(Read-Records $case.Hooks)
        Assert-True ($records.Count -gt 0 -and @($records | Where-Object { $_.XSM_SERVICE_NAME -cne $case.Service -or $_.XSM_TEST_TOKEN -cne $case.Token -or $_.XSM_SAME_NAME -cne $case.SharedValue }).Count -eq 0) "$($case.Name) hook service names and same-named environment variables never cross services"
        $applications = @(Read-Records $case.Apps)
        Assert-True (@($applications | Where-Object { $_.XSM_TEST_TOKEN -cne $case.Token -or $_.XSM_SAME_NAME -cne $case.SharedValue }).Count -eq 0) "$($case.Name) application environment remains isolated"
        $appIds = @($applications | ForEach-Object { "$($_.PROBE_PID)/$($_.PROBE_BIRTH_FILETIME)" })
        Assert-True (@($records | Where-Object { $_.XSM_APPLICATION_PID -ne '' -and "$($_.XSM_APPLICATION_PID)/$($_.XSM_APPLICATION_BIRTH_FILETIME)" -notin $appIds }).Count -eq 0) "$($case.Name) hooks reference only its own application PIDs and creation times"
        Assert-True (@($records | Where-Object { $_.XSM_EVENT -in @('Start','Exit') -and $_.XSM_ACTION -eq $(if ($_.XSM_EVENT -eq 'Start') { 'Pre' } else { 'Post' }) -and $_.XSM_APPLICATION_PID -ne '' }).Count -eq 0) "$($case.Name) pre-start and post-exit snapshots clear application PID"
        foreach ($post in @(Hooks $case 'Start/Post')) {
            Assert-True ([int]$post.XSM_START_REQUESTED_COUNT -eq [int]$post.XSM_START_COUNT -and [int]$post.XSM_EXIT_COUNT -eq ([int]$post.XSM_START_COUNT - 1)) "$($case.Name) wrapper $($post.XSM_PID) start $($post.XSM_START_COUNT) has consistent local counters"
        }
        $stdout = Output-Text $case
        $stderr = Output-Text $case 'stderr'
        Assert-True ($stdout.Contains('HOOK|' + $case.Token + '|') -and $stderr.Contains('HOOKERR|' + $case.Token + '|')) "$($case.Name) hook stdout/stderr reach its own configured paths"
        foreach ($other in $cases) {
            if ($other.Service -eq $case.Service) { continue }
            Assert-True (-not $stdout.Contains($other.Token) -and -not $stderr.Contains($other.Token)) "$($case.Name) output excludes $($other.Name) application and hook payloads"
        }
        $diagnostics = Read-Text (Join-Path $case.Directory 'internal-latest.log')
        $lines = @($diagnostics -split '\r?\n' | Where-Object { $_ -and -not $_.StartsWith('Time | ') })
        Assert-True ($lines.Count -gt 0 -and @($lines | Where-Object { ($_ -split ' \| ',6)[2] -cne $case.Service }).Count -eq 0) "$($case.Name) internal diagnostics contain only its own service column"
    }
    Assert-True ((Get-FileHash -LiteralPath $Candidate -Algorithm SHA256).Hash -eq $candidateSha256 -and (Get-FileHash -LiteralPath $ServiceBinary -Algorithm SHA256).Hash -eq $serviceBinarySha256 -and (Get-FileHash -LiteralPath $Probe -Algorithm SHA256).Hash -eq $probeSha256) 'CLI, main service and fixture binaries remain unchanged throughout the suite'
    $completed = $true
} catch {
    $failure = $_
    throw
} finally {
    if ($guiProcess) {
        try {
            if (-not $guiProcess.HasExited) {
                $window = [Xsm.MultiServiceWindowProbe]::Find([uint32]$guiProcess.Id)
                if ($window -ne [IntPtr]::Zero) { [Xsm.MultiServiceWindowProbe]::Close($window,[uint32]$guiProcess.Id) | Out-Null }
                if (-not $guiProcess.WaitForExit(5000)) { $guiProcess.Kill(); $guiProcess.WaitForExit() }
            }
        } catch { $cleanupErrors.Add("Test GUI: $_") }
        finally { $guiProcess.Dispose() }
    }
    foreach ($case in $cases) { try { Track-Processes $case } catch { $cleanupErrors.Add("Track $($case.Service): $_") } }
    foreach ($service in $services) {
        try { Invoke-Xsm @('stop',$service) | Out-Null } catch { $cleanupErrors.Add("Stop $service`: $_") }
        try { Invoke-Xsm @('remove',$service,'confirm') | Out-Null } catch { $cleanupErrors.Add("Remove $service`: $_") }
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
        try { Wait-Until { -not (Get-Service -Name $service -ErrorAction SilentlyContinue) } "Removal of test service $service" 5 }
        catch { $cleanupErrors.Add($_.ToString()) }
    }
    foreach ($entry in $ownedProcesses.GetEnumerator()) {
        try {
            $process = Get-Process -Id $entry.Key -ErrorAction SilentlyContinue
            if ($process -and $process.StartTime -eq $entry.Value) { throw "Test process survived cleanup: $($entry.Key)" }
        } catch { $cleanupErrors.Add($_.ToString()) }
    }
    $completed = $completed -and $cleanupErrors.Count -eq 0
    $evidenceFile = if ($completed) { 'passed.txt' } else { 'partial.txt' }
    [IO.File]::WriteAllLines((Join-Path $results $evidenceFile),$passed.ToArray(),[Text.UTF8Encoding]::new($true))
    [ordered]@{
        suite='multi-service'; completed=$completed; assertions=$passed.Count
        startedUtc=$startedUtc; finishedUtc=[DateTime]::UtcNow.ToString('o')
        candidate=$Candidate; candidateSha256=$candidateSha256; serviceBinary=$ServiceBinary; serviceBinarySha256=$serviceBinarySha256; probe=$Probe; probeSha256=$probeSha256
        services=@($services.ToArray()); ownedProcesses=@($ownedProcesses.GetEnumerator() | ForEach-Object { @{ pid=$_.Key; birthUtc=$_.Value.ToUniversalTime().ToString('o') } })
        guiLifecycleVerified=$guiLifecycleVerified
        noArgumentsExitVerified=$noArgumentsExitVerified
        guiModesVerified=@($guiModesVerified.ToArray())
        guiIdleSamples=@($guiIdleSamples.ToArray())
        guiConsoleEvidence='Single console PE subsystem=3; no-argument invocation runs with CREATE_NO_WINDOW and exits quietly; explicit install test starts hidden without attaching to user consoles'
        powerEvents='Power/Change and Power/Resume configured and read back; no power transition induced'
        failure=$(if ($failure) { $failure.ToString() } else { $null }); cleanupErrors=@($cleanupErrors.ToArray())
    } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $results 'result.json') -Encoding utf8
    Write-Host "$($passed.Count) multi-service assertions recorded; completed=$completed; evidence: $results"
    if ($cleanupErrors.Count -and -not $failure) { throw ('Cleanup verification failed: ' + ($cleanupErrors -join '; ')) }
}
