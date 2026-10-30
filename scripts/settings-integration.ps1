[CmdletBinding()]
param([string]$Candidate = '', [string]$Probe = '')
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$OutputEncoding = [Console]::OutputEncoding
if (-not $Candidate) { $Candidate = Join-Path $PSScriptRoot '..\target\release\xsm.exe' }
if (-not $Probe) { $Probe = Join-Path $PSScriptRoot '..\target\release\examples\service_probe.exe' }
$Candidate = (Resolve-Path -LiteralPath $Candidate).Path
$Probe = (Resolve-Path -LiteralPath $Probe).Path
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Requires elevation; modifies only two new Xsm_Test_* services.' }
$service = 'Xsm_Test_' + [Guid]::NewGuid().ToString('N')
$replay = 'Xsm_Test_' + [Guid]::NewGuid().ToString('N')
$results = Join-Path (Split-Path -Parent $PSScriptRoot) ('test-results\settings-' + $service)
New-Item -ItemType Directory -Path $results -Force | Out-Null
$passed = [Collections.Generic.List[string]]::new()
$covered = [Collections.Generic.HashSet[string]]::new()
$created = $false
$replayed = $false
$completed = $false
$failure = $null
$cleanupErrors = [Collections.Generic.List[string]]::new()
$startedUtc = [DateTime]::UtcNow.ToString('o')
$candidateSha256 = (Get-FileHash -LiteralPath $Candidate -Algorithm SHA256).Hash
$serviceBinary = (Resolve-Path -LiteralPath (Join-Path (Split-Path -Parent $Candidate) 'xsm.exe')).Path
$serviceBinarySha256 = (Get-FileHash -LiteralPath $serviceBinary -Algorithm SHA256).Hash
$probeSha256 = (Get-FileHash -LiteralPath $Probe -Algorithm SHA256).Hash
function Quote-WindowsArgument([string]$Value) {
    $builder=[Text.StringBuilder]::new(); [void]$builder.Append('"'); $slashes=0
    foreach($character in $Value.ToCharArray()) {
        if($character -eq '\') { $slashes++; continue }
        $count=if($character -eq '"') { $slashes*2+1 } else { $slashes }
        if($count) { [void]$builder.Append(('\' * $count)) }
        [void]$builder.Append($character); $slashes=0
    }
    if($slashes) { [void]$builder.Append(('\' * ($slashes*2))) }
    [void]$builder.Append('"'); return $builder.ToString()
}
function Invoke-Xsm([string[]]$Argv, [switch]$Reject) {
    $info=[Diagnostics.ProcessStartInfo]::new(); $info.FileName=$Candidate
    $info.Arguments=(@($Argv | ForEach-Object { Quote-WindowsArgument $_ }) -join ' ')
    $info.UseShellExecute=$false; $info.CreateNoWindow=$true
    $info.RedirectStandardOutput=$true; $info.RedirectStandardError=$true
    $info.StandardOutputEncoding=[Text.UTF8Encoding]::new($false); $info.StandardErrorEncoding=[Text.UTF8Encoding]::new($false)
    $process=[Diagnostics.Process]::Start($info)
    try {
        $stdout=$process.StandardOutput.ReadToEndAsync(); $stderr=$process.StandardError.ReadToEndAsync()
        if(-not $process.WaitForExit(30000)) { $process.Kill(); throw 'Configuration CLI timed out' }
        $text=($stdout.GetAwaiter().GetResult()+$stderr.GetAwaiter().GetResult()).Replace("`r`n", "`n").TrimEnd()
        $code=$process.ExitCode
    } finally { $process.Dispose() }
    if ($Reject) { if ($code -eq 0) { throw "Expected rejection: $($Argv -join ' ')" }; return $text }
    if ($code -ne 0) { throw "Failed ($code): $($Argv -join ' ')`n$text" }
    return $text
}
function Get-Option([string]$Name, [string[]]$Extra = @(), [string]$Target = $service) { return Invoke-Xsm (@('get', $Target, $Name) + $Extra) }
function Set-Option([string]$Name, [string[]]$Values) { Invoke-Xsm (@('set', $service, $Name) + $Values) | Out-Null; [void]$covered.Add($Name) }
function Reset-Option([string]$Name, [string[]]$Extra = @()) { Invoke-Xsm (@('reset', $service, $Name) + $Extra) | Out-Null }
function Assert-Equal($Actual, $Expected, [string]$Message) {
    if ([string]$Actual -cne [string]$Expected) { throw "$Message`nExpected: [$Expected]`nActual: [$Actual]" }
    $passed.Add($Message)
}
function Reject-Unchanged([string]$Name, [string[]]$Values, [string[]]$Extra = @()) {
    $before = Get-Option $Name $Extra
    Invoke-Xsm (@('set', $service, $Name) + $Values) -Reject | Out-Null
    Assert-Equal (Get-Option $Name $Extra) $before "Invalid $Name is rejected without mutation"
}
try {
    Invoke-Xsm @('install', $service, $Probe, 'serve') | Out-Null
    $created = $true
    Set-Option 'Start' @('SERVICE_DEMAND_START')
    $dwords = [ordered]@{
        AppNoConsole=0; AppRestartDelay=0; AppStdinShareMode=2; AppStdinCreationDisposition=3; AppStdinFlagsAndAttributes=128;
        AppStdoutShareMode=3; AppStdoutCreationDisposition=4; AppStdoutFlagsAndAttributes=128; AppStdoutCopyAndTruncate=0;
        AppStderrShareMode=3; AppStderrCreationDisposition=4; AppStderrFlagsAndAttributes=128; AppStderrCopyAndTruncate=0;
        AppStopMethodSkip=0; AppStopMethodConsole=1500; AppStopMethodWindow=1500; AppStopMethodThreads=1500;
        AppKillProcessTree=1; AppThrottle=1500; AppRedirectHook=0; AppRotateFiles=0; AppRotateOnline=0;
        AppRotateSeconds=0; AppRotateBytes=0; AppRotateBytesHigh=0; AppRotateDelay=0; AppTimestampLog=0;
        XsmLogMaxBytes=10485760; XsmLogMaxFiles=5
    }
    foreach ($name in $dwords.Keys) {
        Assert-Equal (Get-Option $name) $dwords[$name] "$name default"
        $value = if ($name -eq 'XsmLogMaxFiles') { 7 } elseif ($name -eq 'XsmLogMaxBytes') { 4096 } else { 17 }
        Set-Option $name @([string]$value)
        Assert-Equal (Get-Option $name) $value "$name numeric round-trip"
        Reject-Unchanged $name @('not-a-number')
        Reset-Option $name
        Assert-Equal (Get-Option $name) $dwords[$name] "$name reset"
    }
    Set-Option 'AppRotateBytesHigh' @('0xffffffff')
    Assert-Equal (Get-Option 'AppRotateBytesHigh') '4294967295' 'DWORD upper boundary'
    Reset-Option 'AppRotateBytesHigh'
    $unicode = [string][char]0x6D4B + [char]0x8BD5
    $strings = [ordered]@{
        Application=$Probe; AppParameters=('serve "quoted value" & literal ' + $unicode);
        AppDirectory=(Split-Path -Parent $Probe); AppStdin='%SystemRoot%\Temp\xsm-input.txt';
        AppStdout=(Join-Path $results 'stdout.log'); AppStderr=(Join-Path $results 'stderr.log');
        XsmLogPath=(Join-Path $results 'diagnostics.log')
    }
    foreach ($name in $strings.Keys) {
        Set-Option $name @($strings[$name])
        Assert-Equal (Get-Option $name) $strings[$name] "$name string round-trip"
        Reset-Option $name
        Assert-Equal (Get-Option $name) '' "$name reset"
        if($name -eq 'Application') {
            $managed=Invoke-Xsm @('list')
            if(($managed -split "`n") -notcontains $service){throw 'Empty Application marker disappeared from managed list'}
            $passed.Add('Empty Application remains discoverable for repair')
            Invoke-Xsm @('dump',$service) -Reject | Out-Null
        }
        Set-Option $name @($strings[$name])
    }
    # Missing marker is native; an explicitly present empty marker remains recoverable.
    $key=[Microsoft.Win32.Registry]::LocalMachine.OpenSubKey("SYSTEM\CurrentControlSet\Services\$service\Parameters",$true)
    try{$key.DeleteValue('Application')}finally{$key.Dispose()}
    try {
        Invoke-Xsm @('get',$service,'AppParameters') -Reject | Out-Null
        Invoke-Xsm @('set',$service,'AppPriority','HIGH_PRIORITY_CLASS') -Reject | Out-Null
        Assert-Equal (Get-Option 'Name') $service 'Native service properties remain readable without Application marker'
        Set-Option 'Description' @('Native own-process test')
        Assert-Equal (Get-Option 'Description') 'Native own-process test' 'Native own-process properties remain editable'
        Reset-Option 'Description'
    } finally {
        $key=[Microsoft.Win32.Registry]::LocalMachine.OpenSubKey("SYSTEM\CurrentControlSet\Services\$service\Parameters",$true)
        try{$key.SetValue('Application',$Probe,[Microsoft.Win32.RegistryValueKind]::ExpandString)}finally{$key.Dispose()}
    }
    Set-Option 'AppAffinity' @('5,0-2,1,63')
    Assert-Equal (Get-Option 'AppAffinity') '0-2,5,63' 'Affinity canonical ranges'
    Reject-Unchanged 'AppAffinity' @('64')
    Set-Option 'AppAffinity' @('0x1-03, 0x4')
    Assert-Equal (Get-Option 'AppAffinity') '1-4' 'Affinity hexadecimal and octal indices'
    Reset-Option 'AppAffinity'
    Assert-Equal (Get-Option 'AppAffinity') 'All' 'Affinity reset'
    foreach ($priority in @('IDLE_PRIORITY_CLASS','BELOW_NORMAL_PRIORITY_CLASS','NORMAL_PRIORITY_CLASS','ABOVE_NORMAL_PRIORITY_CLASS','HIGH_PRIORITY_CLASS','REALTIME_PRIORITY_CLASS')) {
        Set-Option 'AppPriority' @($priority)
        Assert-Equal (Get-Option 'AppPriority') $priority "Priority $priority"
    }
    Reject-Unchanged 'AppPriority' @('invalid')
    Reset-Option 'AppPriority'
    Assert-Equal (Get-Option 'AppPriority') 'NORMAL_PRIORITY_CLASS' 'Priority reset'
    foreach ($name in @('AppEnvironment','AppEnvironmentExtra','Environment')) {
        Set-Option $name @(':A=one','B=two')
        Assert-Equal (Get-Option $name) "A=one`nB=two" "$name replace"
        Set-Option $name @('+a=changed')
        Assert-Equal (Get-Option $name @('A')) 'changed' "$name append replaces key case-insensitively"
        Set-Option $name @('-A=wrong')
        Assert-Equal (Get-Option $name @('A')) 'changed' "$name mismatched conditional remove retains value"
        Set-Option $name @('-A=changed')
        Assert-Equal (Get-Option $name) 'B=two' "$name conditional remove"
        Set-Option $name @('-b')
        Assert-Equal (Get-Option $name) '' "$name remove by key"
        Set-Option $name @('VALUE=has=equals')
        Assert-Equal (Get-Option $name @('value')) 'has=equals' "$name values may contain equals"
        Reject-Unchanged $name @('not-assignment')
        $upperKey=([string][char]0xC4)+'PATH'; $lowerKey=([string][char]0xE4)+'path'
        Set-Option $name @(':'+$upperKey+'=old')
        Set-Option $name @('+'+$lowerKey+'=new')
        Assert-Equal (Get-Option $name @($upperKey)) 'new' "$name Unicode case-insensitive names"
        Set-Option $name @('A=one','a=two')
        Assert-Equal (Get-Option $name) 'a=two' "$name duplicate names canonicalize to latest value"
        Set-Option $name @('+A=three')
        Assert-Equal (Get-Option $name) 'A=three' "$name duplicate update has one effective value"
        Reset-Option $name
        Assert-Equal (Get-Option $name) '' "$name reset"
    }
    Set-Option 'AppEnvironmentExtra' @(':A=one','B=two')
    foreach ($action in @('Restart','Ignore','Exit','Suicide')) {
        Set-Option 'AppExit' @('Default',$action)
        Assert-Equal (Get-Option 'AppExit' @('Default')) $action "Exit default $action"
        Assert-Equal (Get-Option 'AppExit' @('9191')) $action "Exit fallback $action"
    }
    Set-Option 'AppExit' @('42','Exit')
    Set-Option 'AppExit' @('0x2a','Ignore')
    Assert-Equal (Get-Option 'AppExit' @('42')) 'Ignore' 'Exit hexadecimal key uses canonical numeric code'
    Reset-Option 'AppExit' @('052')
    Assert-Equal (Get-Option 'AppExit' @('42')) 'Suicide' 'Exit octal reset addresses same code'
    Set-Option 'AppExit' @('42','Exit')
    Assert-Equal (Get-Option 'AppExit' @('42')) 'Exit' 'Explicit exit-code action'
    Reject-Unchanged 'AppExit' @('42','invalid') @('42')
    Reset-Option 'AppExit' @('42')
    Assert-Equal (Get-Option 'AppExit' @('42')) 'Suicide' 'Exit reset restores default fallback'
    Reset-Option 'AppExit' @('Default')
    Assert-Equal (Get-Option 'AppExit' @('Default')) 'Restart' 'Default exit reset'
    Set-Option 'AppExit' @('42','Exit')
    $hooks = @('Start/Pre','Start/Post','Stop/Pre','Exit/Post','Power/Change','Power/Resume','Rotate/Pre','Rotate/Post')
    foreach ($hook in $hooks) {
        $command = 'cmd.exe /c echo hook-' + $hook
        Set-Option 'AppEvents' @($hook,$command)
        Assert-Equal (Get-Option 'AppEvents' @($hook)) $command "Hook $hook round-trip"
        Reset-Option 'AppEvents' @($hook)
        Assert-Equal (Get-Option 'AppEvents' @($hook)) '' "Hook $hook reset"
        Set-Option 'AppEvents' @($hook,$command)
    }
    Invoke-Xsm @('set',$service,'AppEvents','Stop/Post','cmd /c echo invalid') -Reject | Out-Null
    $passed.Add('Invalid hook pair rejected')
    foreach ($level in @('off','error','warn','info','debug','trace')) {
        Set-Option 'XsmLogLevel' @($level.ToUpperInvariant())
        Assert-Equal (Get-Option 'XsmLogLevel') $level "Logging level $level"
    }
    Reject-Unchanged 'XsmLogLevel' @('verbose')
    Reject-Unchanged 'XsmLogMaxBytes' @('0')
    Reject-Unchanged 'XsmLogMaxFiles' @('101')
    Reset-Option 'XsmLogLevel'
    Assert-Equal (Get-Option 'XsmLogLevel') 'info' 'Logging level reset'
    Set-Option 'Description' @('XSM settings test ' + $unicode)
    Assert-Equal (Get-Option 'Description') ('XSM settings test ' + $unicode) 'Unicode description'
    Reset-Option 'Description'
    Assert-Equal (Get-Option 'Description') '' 'Description reset'
    $multilineDescription="XSM replay line one`r`nSecond 'quoted' %SystemRoot% !value!"
    Set-Option 'Description' @($multilineDescription)
    Assert-Equal (Get-ItemProperty -LiteralPath ("HKLM:\SYSTEM\CurrentControlSet\Services\"+$service) -Name Description).Description $multilineDescription 'Multiline Description stored with exact CRLF'
    Set-Option 'DisplayName' @($service + ' Display ' + $unicode)
    Assert-Equal (Get-Option 'DisplayName') ($service + ' Display ' + $unicode) 'Unicode display name'
    Assert-Equal (Invoke-Xsm @('get',($service + ' Display ' + $unicode),'Name')) $service 'Display name resolves canonical name'
    Assert-Equal (Invoke-Xsm @('get',$service.ToLowerInvariant(),'Name')) $service 'Case-insensitive canonical service name'
    [void]$covered.Add('Name')
    Reject-Unchanged 'Name' @('renamed')
    Reset-Option 'DisplayName'
    Assert-Equal (Get-Option 'DisplayName') $service 'Display name reset'
    $image = Get-Option 'ImagePath'
    Set-Option 'ImagePath' @($image)
    Assert-Equal (Get-Option 'ImagePath') $image 'SCM image path round-trip'
    Invoke-Xsm @('reset',$service,'ImagePath') -Reject | Out-Null
    Assert-Equal (Get-Option 'ImagePath') $image 'ImagePath has no reset default'
    foreach ($startup in @('SERVICE_DELAYED_AUTO_START','SERVICE_AUTO_START','SERVICE_DISABLED','SERVICE_DEMAND_START')) {
        Set-Option 'Start' @($startup)
        Assert-Equal (Get-Option 'Start') $startup "Startup $startup"
    }
    Reject-Unchanged 'Start' @('invalid')
    foreach ($account in @('NT AUTHORITY\LocalService','NT AUTHORITY\NetworkService',('NT SERVICE\' + $service),'LocalSystem')) {
        Set-Option 'ObjectName' @($account)
        Assert-Equal ((Get-Option 'ObjectName').ToLowerInvariant()) $account.ToLowerInvariant() "Service account $account"
    }
    Reset-Option 'ObjectName'
    Assert-Equal (Get-Option 'ObjectName') 'LocalSystem' 'Account reset'
    $administrators=[Security.Principal.SecurityIdentifier]::new([Security.Principal.WellKnownSidType]::BuiltinAdministratorsSid,$null).Translate([Security.Principal.NTAccount]).Value
    Reject-Unchanged 'ObjectName' @($administrators,'')
    foreach ($type in @('SERVICE_INTERACTIVE_PROCESS','SERVICE_WIN32_OWN_PROCESS')) {
        Set-Option 'Type' @($type)
        Assert-Equal (Get-Option 'Type') $type "Service type $type"
    }
    Reject-Unchanged 'Type' @('SERVICE_KERNEL_DRIVER')
    & sc.exe config $service type= share | Out-Null
    if($LASTEXITCODE) { throw 'Cannot prepare temporary shared-process service' }
    try {
        Assert-Equal (Get-Option 'Type') 'SERVICE_WIN32_SHARE_PROCESS' 'Shared process properties remain readable'
        Reject-Unchanged 'Description' @('must not mutate shared-process service')
        Invoke-Xsm @('set',$service,'AppPriority','HIGH_PRIORITY_CLASS') -Reject | Out-Null
        $passed.Add('Shared-process application edits rejected')
    } finally { & sc.exe config $service type= own | Out-Null; if($LASTEXITCODE){throw 'Cannot restore temporary own-process service'} }
    Set-Option 'DependOnService' @('RpcSs')
    Assert-Equal (Get-Option 'DependOnService') 'RpcSs' 'Service dependency canonical name'
    Set-Option 'DependOnService' @('+EventLog')
    Assert-Equal (Get-Option 'DependOnService') "RpcSs`nEventLog" 'Service dependency append'
    Set-Option 'DependOnService' @('-RpcSs')
    Assert-Equal (Get-Option 'DependOnService') 'EventLog' 'Service dependency remove'
    Reject-Unchanged 'DependOnService' @('Xsm_Missing_' + [Guid]::NewGuid().ToString('N'))
    Reset-Option 'DependOnService'
    Assert-Equal (Get-Option 'DependOnService') '' 'Service dependency reset'
    Set-Option 'DependOnService' @('RpcSs')
    $group = @((Get-ItemProperty -LiteralPath 'HKLM:\SYSTEM\CurrentControlSet\Control\ServiceGroupOrder').List)[0]
    Set-Option 'DependOnGroup' @(':' + $group)
    Assert-Equal (Get-Option 'DependOnGroup') ('+' + $group) 'Group dependency canonical prefix'
    Assert-Equal (Get-Option 'DependOnService') 'RpcSs' 'Group update retains service dependencies'
    Reject-Unchanged 'DependOnGroup' @(':Xsm_Invalid_Group')
    Reset-Option 'DependOnGroup'
    Assert-Equal (Get-Option 'DependOnGroup') '' 'Group dependency reset'

    # Replay only the trusted dump of our disposable service. No managed program is started.
    $dump = Invoke-Xsm @('dump',$service,$replay)
    $dumpFile = Join-Path $results 'replay.cmd'
    $checkedDump=(@($dump -split "`n" | Where-Object { $_.Trim().Length -gt 0 } | ForEach-Object { $_.TrimEnd() + ' || exit /b 1' }) -join "`r`n")
    [IO.File]::WriteAllText($dumpFile, "@echo off`r`nchcp 65001 >nul`r`n" + $checkedDump + "`r`n", [Text.UTF8Encoding]::new($false))
    $replayed = $true # Finally attempts cleanup even if replay partially succeeds.
    $cmdOutput = & $env:ComSpec /d /c $dumpFile 2>&1
    if ($LASTEXITCODE -ne 0) { throw "Dump replay failed: $cmdOutput" }
    foreach ($name in @($dwords.Keys) + @($strings.Keys) + @('AppAffinity','AppPriority','AppEnvironment','AppEnvironmentExtra','Environment','XsmLogLevel','Description','Start','Type','ObjectName','DependOnService','DependOnGroup')) {
        Assert-Equal (Get-Option $name @() $replay) (Get-Option $name) "Dump replay $name"
    }
    foreach ($hook in $hooks) { Assert-Equal (Get-Option 'AppEvents' @($hook) $replay) (Get-Option 'AppEvents' @($hook)) "Dump replay $hook" }
    Assert-Equal (Get-ItemProperty -LiteralPath ("HKLM:\SYSTEM\CurrentControlSet\Services\"+$replay) -Name Description).Description $multilineDescription 'Dump preserves multiline Description exact CRLF'
    Assert-Equal (Get-Option 'AppExit' @('42') $replay) 'Exit' 'Dump replay explicit exit action'
    Assert-Equal (Get-Option 'Name' @() $replay) $replay 'Dump rename creates requested service'
    Assert-Equal $covered.Count 53 'Every configuration setting exercised'
    $completed = $true
} catch {
    $failure = $_
    throw
} finally {
    foreach ($entry in @(@($replay,$replayed),@($service,$created))) {
        if ($entry[1]) {
            try { Invoke-Xsm @('remove', $entry[0], 'confirm') | Out-Null }
            catch { $cleanupErrors.Add("Remove $($entry[0]): $_") }
        }
    }
    foreach ($target in @($service, $replay)) {
        try {
            $deadline = [DateTime]::UtcNow.AddSeconds(5)
            while (Get-Service -Name $target -ErrorAction SilentlyContinue) {
                if ([DateTime]::UtcNow -ge $deadline) { throw "Test service survived cleanup: $target" }
                Start-Sleep -Milliseconds 100
            }
        } catch { $cleanupErrors.Add($_.ToString()) }
    }
    $completed = $completed -and $cleanupErrors.Count -eq 0
    $evidenceFile = if ($completed) { 'passed.txt' } else { 'partial.txt' }
    [IO.File]::WriteAllLines((Join-Path $results $evidenceFile), $passed.ToArray(), [Text.UTF8Encoding]::new($true))
    [ordered]@{
        suite = 'settings'; completed = $completed; assertions = $passed.Count
        startedUtc = $startedUtc; finishedUtc = [DateTime]::UtcNow.ToString('o')
        candidate = $Candidate; candidateSha256 = $candidateSha256; probe = $Probe; probeSha256 = $probeSha256
        serviceBinary = $serviceBinary; serviceBinarySha256 = $serviceBinarySha256
        failure = if ($failure) { $failure.ToString() } else { $null }; cleanupErrors = @($cleanupErrors.ToArray())
    } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $results 'result.json') -Encoding utf8
    Write-Host "$($passed.Count) settings assertions recorded; completed=$completed; evidence: $results"
    if ($cleanupErrors.Count -and -not $failure) {
        throw ('Cleanup verification failed: ' + ($cleanupErrors -join '; '))
    }
}
