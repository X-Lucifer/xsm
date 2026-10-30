[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Run in an elevated PowerShell. This check manages only one new Xsm_Test_* service.' }
$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\release\xsm.exe'
$serviceBinary = Join-Path $root 'target\release\xsm.exe'
$probe = Join-Path $root 'target\release\examples\service_probe.exe'
$name = 'Xsm_Test_' + [Guid]::NewGuid().ToString('N')
$created = $false
function Invoke-Xsm([string[]]$Arguments) {
    $output = & $exe @Arguments 2>&1
    if ($LASTEXITCODE) { throw ($output | Out-String) }
}
function Measure-GuiIdle {
    if (-not ('Xsm.PerformanceWindowProbe' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
namespace Xsm {
    public static class PerformanceWindowProbe {
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
        public static bool Responsive(IntPtr window, uint process) {
            if (!Owned(window, process)) return false;
            UIntPtr result;
            return SendMessageTimeoutW(window, 0, UIntPtr.Zero, IntPtr.Zero, 3, 10000, out result) != IntPtr.Zero;
        }
        public static bool Close(IntPtr window, uint process) {
            return Owned(window, process) && PostMessageW(window, 0x0010, UIntPtr.Zero, IntPtr.Zero);
        }
    }
}
'@
    }
    $gui = $null
    $guiBirth = $null
    $sample = $null
    try {
        $gui = Start-Process -FilePath $serviceBinary -ArgumentList 'install' -WindowStyle Hidden -PassThru
        $guiBirth = $gui.StartTime.ToUniversalTime()
        $deadline = [DateTime]::UtcNow.AddSeconds(20)
        do {
            if ($gui.HasExited) { throw "The test GUI exited during startup: $($gui.ExitCode)" }
            $window = [Xsm.PerformanceWindowProbe]::Find([uint32]$gui.Id)
            if ($window -ne [IntPtr]::Zero) { break }
            Start-Sleep -Milliseconds 75
        } while ([DateTime]::UtcNow -lt $deadline)
        if ($window -eq [IntPtr]::Zero) { throw 'The test GUI did not create its management window' }
        if ([Xsm.PerformanceWindowProbe]::IsWindowVisible($window)) { throw 'The test GUI must remain hidden' }
        if (-not [Xsm.PerformanceWindowProbe]::Responsive($window,[uint32]$gui.Id)) { throw 'The test GUI message loop did not respond to WM_NULL' }
        # CUI executables can own GUI windows; WM_NULL checks that exact window
        # without WaitForInputIdle's graphical-subsystem assumption.
        Start-Sleep -Seconds 2
        $gui.Refresh()
        if ($gui.HasExited) { throw 'The test GUI exited before idle sampling' }
        $cpuBefore = $gui.TotalProcessorTime.TotalSeconds
        $sampleStarted = [DateTime]::UtcNow.ToString('o')
        $sampleWatch = [Diagnostics.Stopwatch]::StartNew()
        Start-Sleep -Seconds 5
        $gui.Refresh()
        $sampleWatch.Stop()
        if ($gui.HasExited) { throw 'The test GUI exited during idle sampling' }
        if ([Xsm.PerformanceWindowProbe]::IsWindowVisible($window)) { throw 'The test GUI became visible during idle sampling' }
        $cpuSeconds = $gui.TotalProcessorTime.TotalSeconds - $cpuBefore
        $sample = [ordered]@{
            mode = 'install'; pid = $gui.Id; creation_utc = $guiBirth.ToString('o')
            started_utc = $sampleStarted; finished_utc = [DateTime]::UtcNow.ToString('o')
            hidden = $true; stabilization_seconds = 2
            elapsed_seconds = $sampleWatch.Elapsed.TotalSeconds; cpu_seconds = $cpuSeconds
            one_core_cpu_percent = 100 * $cpuSeconds / $sampleWatch.Elapsed.TotalSeconds
            normalized_cpu_percent = 100 * $cpuSeconds / $sampleWatch.Elapsed.TotalSeconds / [Environment]::ProcessorCount
            logical_processors = [Environment]::ProcessorCount
            private_bytes = $gui.PrivateMemorySize64; working_set_bytes = $gui.WorkingSet64
            handles = $gui.HandleCount; threads = $gui.Threads.Count
            closed_normally = $false
            context = 'Five-second hidden install-window sample after bounded WM_NULL response and two-second stabilization, with the test service stopped; descriptive evidence without an absolute performance threshold'
        }
    } finally {
        if ($gui) {
            try {
                if (-not $gui.HasExited) {
                    $current = Get-Process -Id $gui.Id -ErrorAction SilentlyContinue
                    try {
                        if (-not $current -or -not $guiBirth -or $current.StartTime.ToUniversalTime() -ne $guiBirth) { throw 'Cannot verify the test GUI identity for cleanup' }
                        $window = [Xsm.PerformanceWindowProbe]::Find([uint32]$gui.Id)
                        if ($window -ne [IntPtr]::Zero) { [void][Xsm.PerformanceWindowProbe]::Close($window,[uint32]$gui.Id) }
                    } finally { if ($current) { $current.Dispose() } }
                    if (-not $gui.WaitForExit(10000)) {
                        $current = Get-Process -Id $gui.Id -ErrorAction SilentlyContinue
                        try {
                            if (-not $current -or $current.StartTime.ToUniversalTime() -ne $guiBirth) { throw 'The test GUI identity changed before timeout cleanup' }
                            # Kill only the original retained Process handle after
                            # verifying its PID and creation time again.
                            $gui.Kill()
                            if (-not $gui.WaitForExit(5000)) { throw 'The test GUI survived timeout cleanup' }
                        } finally { if ($current) { $current.Dispose() } }
                        throw 'The test GUI required forced cleanup after WM_CLOSE timed out'
                    }
                }
                if ($sample) {
                    if ($gui.ExitCode -ne 0) { throw "The test GUI closed with exit code $($gui.ExitCode)" }
                    $sample.closed_normally = $true
                }
            } finally { $gui.Dispose() }
        }
    }
    return $sample
}
try {
    Invoke-Xsm @('install', $name, $probe, 'serve')
    $created = $true
    Invoke-Xsm @('set', $name, 'Start', 'SERVICE_DEMAND_START')
    Invoke-Xsm @('set', $name, 'XsmLogLevel', 'off')
    Invoke-Xsm @('set', $name, 'AppThrottle', '0')
    Invoke-Xsm @('start', $name)
    $service = Get-CimInstance Win32_Service -Filter "Name='$name'"
    $proc = Get-Process -Id $service.ProcessId
    $beforeCpu = $proc.TotalProcessorTime.TotalSeconds
    $watch = [Diagnostics.Stopwatch]::StartNew()
    Start-Sleep -Seconds 5
    $proc.Refresh()
    $idle = [ordered]@{
        elapsed_seconds = $watch.Elapsed.TotalSeconds
        cpu_seconds = $proc.TotalProcessorTime.TotalSeconds - $beforeCpu
        private_bytes = $proc.PrivateMemorySize64
        working_set_bytes = $proc.WorkingSet64
        handles = $proc.HandleCount
        threads = $proc.Threads.Count
    }
    Invoke-Xsm @('stop', $name)
    # Exercise many application lifecycles inside one persistent supervisor.
    Invoke-Xsm @('set', $name, 'AppParameters', 'exit 0')
    Invoke-Xsm @('set', $name, 'AppRestartDelay', '20')
    & $exe start $name 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    $service = Get-CimInstance Win32_Service -Filter "Name='$name'"
    if (-not $service.ProcessId) { throw 'Restart loop supervisor is absent' }
    $proc = Get-Process -Id $service.ProcessId
    $initialHandles = $proc.HandleCount
    $initialPrivate = $proc.PrivateMemorySize64
    Start-Sleep -Seconds 5
    $proc.Refresh()
    $stress = [ordered]@{
        initial_handles = $initialHandles
        final_handles = $proc.HandleCount
        initial_private_bytes = $initialPrivate
        final_private_bytes = $proc.PrivateMemorySize64
    }
    if ($stress.final_handles -gt $stress.initial_handles + 16) { throw 'Unexpected handle growth during restart loop' }
    Invoke-Xsm @('stop', $name)
    $guiIdle = Measure-GuiIdle
    $results = Join-Path $root 'test-results'
    New-Item -ItemType Directory -Path $results -Force | Out-Null
    [ordered]@{ measured_utc = [DateTime]::UtcNow.ToString('o'); executable_sha256 = (Get-FileHash -LiteralPath $serviceBinary -Algorithm SHA256).Hash; idle = $idle; restart_loop = $stress; gui_idle = $guiIdle } |
        ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $results 'performance.json') -Encoding utf8
    Write-Host "Performance measurements saved to $results\performance.json"
} finally {
    if ($created) {
        & $exe stop $name 2>&1 | Out-Null
        & $exe remove $name confirm 2>&1 | Out-Null
        if ($LASTEXITCODE) { Write-Warning "Cleanup required for $name" }
    }
}
