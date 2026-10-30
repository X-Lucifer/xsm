//! SCM callback is nonblocking; one service thread owns all lifecycle state.
use super::{
    diagnostics::{self, Level},
    hooks::{self, Hooks},
    logging::{Streams, event},
    process::{self, Environment, Spawn, StopPolicy},
};
use anyhow::{Context, Result, bail};
use std::{
    ptr::null_mut,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{Services::*, SystemInformation::GetWindowsDirectoryW, Threading::*},
    UI::WindowsAndMessaging::*,
};
use winreg::{RegKey, enums::HKEY_LOCAL_MACHINE};

struct Shared {
    sender: Mutex<Option<Sender<(u32, u32)>>>,
    stop: AtomicBool,
    stop_control: AtomicU32,
}
static SHARED: OnceLock<Shared> = OnceLock::new();
fn shared() -> &'static Shared {
    SHARED.get_or_init(|| Shared {
        sender: Mutex::new(None),
        stop: AtomicBool::new(false),
        stop_control: AtomicU32::new(SERVICE_CONTROL_STOP),
    })
}
pub(crate) fn stop_requested() -> bool {
    shared().stop.load(Ordering::Acquire)
}
unsafe extern "system" fn handler(
    control: u32,
    event_type: u32,
    _event: *mut std::ffi::c_void,
    _context: *mut std::ffi::c_void,
) -> u32 {
    std::panic::catch_unwind(|| {
        if control == SERVICE_CONTROL_PAUSE {
            return ERROR_CALL_NOT_IMPLEMENTED;
        }
        if !matches!(
            control,
            SERVICE_CONTROL_STOP
                | SERVICE_CONTROL_SHUTDOWN
                | SERVICE_CONTROL_INTERROGATE
                | SERVICE_CONTROL_CONTINUE
                | SERVICE_CONTROL_POWEREVENT
                | 128
        ) {
            return ERROR_CALL_NOT_IMPLEMENTED;
        }
        if matches!(control, SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN) {
            shared().stop_control.store(control, Ordering::Relaxed);
            shared().stop.store(true, Ordering::Release);
        }
        if let Ok(guard) = shared().sender.lock()
            && let Some(tx) = guard.as_ref()
        {
            let _ = tx.send((control, event_type));
        }
        NO_ERROR
    })
    .unwrap_or(ERROR_EXCEPTION_IN_SERVICE)
}
struct Status {
    handle: SERVICE_STATUS_HANDLE,
    value: SERVICE_STATUS,
    updated: Instant,
}
impl Drop for Status {
    fn drop(&mut self) {
        if self.value.dwCurrentState != SERVICE_STOPPED {
            // Never leave SCM waiting after an unwind. This path performs no
            // allocation, locking, or diagnostic formatting that could panic.
            self.value.dwCurrentState = SERVICE_STOPPED;
            self.value.dwControlsAccepted = 0;
            self.value.dwWin32ExitCode = ERROR_EXCEPTION_IN_SERVICE;
            self.value.dwServiceSpecificExitCode = 0;
            self.value.dwWaitHint = 0;
            self.value.dwCheckPoint = 0;
            unsafe {
                SetServiceStatus(self.handle, &self.value);
            }
        }
    }
}
impl Status {
    fn new(name: &str) -> Result<Self> {
        let name = process::wide(name)?;
        let handle =
            unsafe { RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(handler), null_mut()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut status = Self {
            handle,
            value: Default::default(),
            updated: Instant::now(),
        };
        status.value.dwServiceType = SERVICE_WIN32_OWN_PROCESS;
        status.set(SERVICE_START_PENDING, 0);
        Ok(status)
    }
    fn set(&mut self, state: u32, code: u32) {
        self.value.dwCurrentState = state;
        self.value.dwWin32ExitCode = if code == 0 {
            0
        } else {
            ERROR_SERVICE_SPECIFIC_ERROR
        };
        self.value.dwServiceSpecificExitCode = code;
        let pending = matches!(
            state,
            SERVICE_START_PENDING | SERVICE_STOP_PENDING | SERVICE_CONTINUE_PENDING
        );
        self.value.dwControlsAccepted = if matches!(state, SERVICE_STOPPED | SERVICE_STOP_PENDING) {
            0
        } else {
            SERVICE_ACCEPT_STOP
                | SERVICE_ACCEPT_SHUTDOWN
                | SERVICE_ACCEPT_POWEREVENT
                | if state == SERVICE_PAUSED {
                    SERVICE_ACCEPT_PAUSE_CONTINUE
                } else {
                    0
                }
        };
        self.value.dwWaitHint = if pending { 22000 } else { 0 };
        self.value.dwCheckPoint = if pending {
            self.value.dwCheckPoint.wrapping_add(1).max(1)
        } else {
            0
        };
        self.publish();
    }
    fn publish(&mut self) {
        unsafe { SetServiceStatus(self.handle, &self.value) };
        self.updated = Instant::now();
    }
    fn heartbeat(&mut self) {
        if self.updated.elapsed() >= Duration::from_secs(1) {
            if matches!(
                self.value.dwCurrentState,
                SERVICE_START_PENDING | SERVICE_STOP_PENDING | SERVICE_CONTINUE_PENDING
            ) {
                self.value.dwCheckPoint = self.value.dwCheckPoint.wrapping_add(1).max(1);
            }
            self.publish();
        }
    }
}
struct Config {
    key: RegKey,
    application: String,
    parameters: String,
    directory: String,
    environment: Environment,
    flags: u32,
    affinity: usize,
    stop: StopPolicy,
    throttle: u32,
    restart: u32,
    display: String,
    redirect: bool,
}
fn number(key: &RegKey, name: &str, default: u32) -> Result<u32> {
    match key.get_value(name) {
        Ok(value) => Ok(value),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(default),
        Err(e) => Err(e).with_context(|| format!("Read {name}")),
    }
}
fn string(key: &RegKey, name: &str) -> Result<String> {
    match key.get_value(name) {
        Ok(value) => Ok(value),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("Read {name}")),
    }
}
fn affinity(value: &str) -> Result<usize> {
    if value.is_empty() || value.eq_ignore_ascii_case("All") {
        return Ok(0);
    }
    usize::try_from(crate::settings::parse_affinity(value)?)
        .context("AppAffinity exceeds this architecture's mask width")
}
fn default_directory(application: &str) -> Result<String> {
    if let Some(parent) = std::path::Path::new(application)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        return Ok(parent.to_string_lossy().into_owned());
    }
    let mut directory = vec![0u16; 32768];
    let len = unsafe { GetWindowsDirectoryW(directory.as_mut_ptr(), directory.len() as u32) };
    if len == 0 || len as usize >= directory.len() {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(String::from_utf16(&directory[..len as usize])?)
}
impl Config {
    fn load(service: &str) -> Result<Self> {
        let root = RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey(format!("SYSTEM\\CurrentControlSet\\Services\\{service}"))?;
        let key = root.open_subkey("Parameters")?;
        let mut environment = process::inherited_environment();
        match key.get_value::<Vec<String>, _>("AppEnvironment") {
            Ok(values) => {
                environment.clear();
                process::overlay_environment(&mut environment, &values)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        match key.get_value::<Vec<String>, _>("AppEnvironmentExtra") {
            Ok(values) => process::overlay_environment(&mut environment, &values)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let application = process::expand(&string(&key, "Application")?, &environment);
        if application.is_empty() {
            bail!("Application is not configured")
        }
        let parameters = process::expand(&string(&key, "AppParameters")?, &environment);
        let mut directory = process::expand(&string(&key, "AppDirectory")?, &environment);
        if directory.is_empty() {
            directory = default_directory(&application)?;
        }
        let priority = number(&key, "AppPriority", NORMAL_PRIORITY_CLASS)?;
        let priority = match priority {
            REALTIME_PRIORITY_CLASS
            | HIGH_PRIORITY_CLASS
            | ABOVE_NORMAL_PRIORITY_CLASS
            | NORMAL_PRIORITY_CLASS
            | BELOW_NORMAL_PRIORITY_CLASS
            | IDLE_PRIORITY_CLASS => priority,
            _ => NORMAL_PRIORITY_CLASS,
        };
        let flags = priority
            | if number(&key, "AppNoConsole", 0)? == 0 {
                CREATE_NEW_CONSOLE
            } else {
                CREATE_NO_WINDOW
            };
        let affinity = affinity(&string(&key, "AppAffinity")?).unwrap_or_else(|error| {
            diagnostics::write(
                service,
                Level::Warn,
                &format!("Invalid CPU affinity; retaining system default: {error}"),
            );
            0
        });
        Ok(Self {
            application,
            parameters,
            directory,
            environment,
            flags,
            affinity,
            stop: StopPolicy {
                skip: number(&key, "AppStopMethodSkip", 0)?,
                console: number(&key, "AppStopMethodConsole", 1500)?,
                window: number(&key, "AppStopMethodWindow", 1500)?,
                threads: number(&key, "AppStopMethodThreads", 1500)?,
                tree: number(&key, "AppKillProcessTree", 1)? != 0,
            },
            throttle: number(&key, "AppThrottle", 1500)?,
            restart: number(&key, "AppRestartDelay", 0)?,
            display: root
                .get_value("DisplayName")
                .unwrap_or_else(|_| service.into()),
            redirect: number(&key, "AppRedirectHook", 0)? != 0,
            key,
        })
    }
    fn command(&self) -> String {
        format!("\"{}\" {}", self.application, self.parameters)
    }
    fn exit_action(&self, code: u32) -> (String, bool) {
        let Ok(key) = self.key.open_subkey("AppExit") else {
            return ("Restart".into(), true);
        };
        if let Ok(value) = key.get_value::<String, _>(code.to_string()) {
            return (value, false);
        }
        (key.get_value("").unwrap_or_else(|_| "Restart".into()), true)
    }
}
pub fn restart_delay(consecutive: u32, configured: u32) -> u32 {
    let throttle = if consecutive == 0 {
        0
    } else {
        1000u32 << (consecutive.min(8) - 1)
    };
    configured.max(throttle)
}
struct Session {
    service: String,
    receiver: Receiver<(u32, u32)>,
    status: Status,
    hooks: Hooks,
    context: hooks::Context,
}
impl Session {
    fn stop_hook(&mut self, streams: Option<&Streams>) {
        diagnostics::write(
            &self.service,
            Level::Info,
            "Stopping application and running shutdown hook",
        );
        self.status.set(SERVICE_STOP_PENDING, 0);
        let control = shared().stop_control.load(Ordering::Relaxed);
        self.context.last_control = control;
        self.hooks.sync(
            &self.context,
            "Stop",
            "Pre",
            Some(control),
            20000,
            streams,
            || self.status.heartbeat(),
        );
    }
    fn command(&mut self, control: u32, event_type: u32, streams: Option<&Streams>) {
        diagnostics::write(
            &self.service,
            Level::Debug,
            &format!(
                "Handling service control {} (event {event_type})",
                hooks::control_name(control)
            ),
        );
        match control {
            SERVICE_CONTROL_INTERROGATE => self.status.publish(),
            SERVICE_CONTROL_POWEREVENT => {
                let action = match event_type {
                    PBT_APMRESUMEAUTOMATIC => Some("Resume"),
                    PBT_APMPOWERSTATUSCHANGE => Some("Change"),
                    _ => None,
                };
                if let Some(action) = action {
                    self.context.last_control = control;
                    self.hooks
                        .asynchronous(&self.context, "Power", action, Some(control), streams);
                }
            }
            128 => {
                self.context.last_control = control;
                self.hooks.sync(
                    &self.context,
                    "Rotate",
                    "Pre",
                    Some(control),
                    60000,
                    streams,
                    || self.status.heartbeat(),
                );
                if let Some(streams) = streams {
                    streams.rotate()
                }
                self.hooks
                    .asynchronous(&self.context, "Rotate", "Post", Some(control), streams);
            }
            _ => {}
        }
    }
    fn stopping(&self) -> bool {
        shared().stop.load(Ordering::Acquire)
    }
    fn wait_delay(&mut self, delay: u32) -> Result<()> {
        if delay == 0 {
            return Ok(());
        }
        self.status.set(SERVICE_PAUSED, 0);
        diagnostics::write(
            &self.service,
            Level::Info,
            &format!("Waiting {delay} ms before restart"),
        );
        let begin = Instant::now();
        loop {
            if self.stopping() {
                return Ok(());
            }
            let left = Duration::from_millis(u64::from(delay)).saturating_sub(begin.elapsed());
            if left.is_zero() {
                break;
            }
            match self.receiver.recv_timeout(left.min(Duration::from_secs(1))) {
                Ok((SERVICE_CONTROL_CONTINUE, _)) => {
                    self.context.last_control = SERVICE_CONTROL_CONTINUE;
                    self.context.throttle = 0;
                    break;
                }
                Ok((control, kind)) => self.command(control, kind, None),
                Err(RecvTimeoutError::Timeout) => {}
                Err(e) => return Err(e.into()),
            }
            self.status.heartbeat();
            self.hooks.reap();
        }
        Ok(())
    }
    fn finish(&mut self, code: u32) {
        self.status.set(SERVICE_STOP_PENDING, 0);
        self.hooks.finish(|| self.status.heartbeat());
        diagnostics::write(
            &self.service,
            Level::Info,
            &format!("Service stopped with code {code}"),
        );
        // Reporting STOPPED can unblock the dispatcher and let main exit.
        // All externally visible cleanup must finish before this final report.
        self.status.set(SERVICE_STOPPED, code);
    }
    fn run(&mut self) -> Result<u32> {
        diagnostics::configure(&self.service)?;
        diagnostics::write(&self.service, Level::Info, "Service dispatcher initialized");
        if let Err(error) = super::scm::enable_recovery(&self.service) {
            event(
                &self.service,
                true,
                &format!("Could not enable recovery for non-crash failures: {error:#}"),
            );
        }
        let mut delay = 0;
        loop {
            let cfg = Config::load(&self.service)?;
            diagnostics::write(
                &self.service,
                Level::Debug,
                "Application configuration loaded",
            );
            self.context.display = cfg.display.clone();
            self.context.command = cfg.command();
            self.context.directory = cfg.directory.clone();
            self.context.environment = cfg.environment.clone();
            self.context.redirect = cfg.redirect;
            self.context.start_requested = self.context.start_requested.saturating_add(1);
            self.context.throttle = self.context.throttle.saturating_add(1);
            self.wait_delay(delay)?;
            if self.stopping() {
                self.stop_hook(None);
                return Ok(0);
            }
            self.status.set(SERVICE_START_PENDING, 0);
            let streams = Streams::load(&cfg.key, &cfg.environment, &self.service, &cfg.directory)?;
            let pre = self.hooks.sync(
                &self.context,
                "Start",
                "Pre",
                Some(0),
                20000,
                Some(&streams),
                || self.status.heartbeat(),
            );
            if pre == 99 {
                event(
                    &self.service,
                    true,
                    "Start/Pre hook aborted application startup",
                );
                return Ok(5);
            }
            if self.stopping() {
                self.stop_hook(Some(&streams));
                return Ok(0);
            }
            let child = process::spawn(Spawn {
                application: Some(&cfg.application),
                command: &cfg.command(),
                directory: &cfg.directory,
                environment: &cfg.environment,
                stdio: Some(streams.handles()),
                flags: cfg.flags,
                affinity: cfg.affinity,
            })?;
            // Error/unwind cleanup must honor the configured skip/tree policy
            // even before the regular stop path has run.
            child.set_cleanup_policy(cfg.stop);
            if let Some(warning) = child.affinity_warning.as_ref() {
                diagnostics::write(&self.service, Level::Warn, warning);
            }
            diagnostics::write(
                &self.service,
                Level::Info,
                &format!("Application process started (PID {})", child.pid),
            );
            self.context.start_count = self.context.start_count.saturating_add(1);
            self.context.pid = child.pid;
            self.context.application_started = Some(child.started);
            let mut running = false;
            let exitcode = loop {
                if self.stopping() {
                    self.stop_hook(Some(&streams));
                    process::stop(&child, cfg.stop, || self.status.heartbeat())?;
                    let exit = child.exit_code()?;
                    if exit.is_none() {
                        diagnostics::write(
                            &self.service,
                            Level::Warn,
                            "Application remains running because configured stop methods did not end it",
                        );
                    }
                    break exit.unwrap_or(0);
                }
                if let Some(code) = child.exit_code()? {
                    break code;
                }
                if !running
                    && child.started.elapsed() >= Duration::from_millis(u64::from(cfg.throttle))
                {
                    running = true;
                    self.context.throttle = 0;
                    self.status.set(SERVICE_RUNNING, 0);
                    diagnostics::write(
                        &self.service,
                        Level::Debug,
                        "Application passed startup throttle threshold; service is running",
                    );
                    self.hooks.asynchronous(
                        &self.context,
                        "Start",
                        "Post",
                        Some(0),
                        Some(&streams),
                    );
                }
                match self.receiver.recv_timeout(Duration::from_millis(100)) {
                    Ok((control, kind)) => self.command(control, kind, Some(&streams)),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(e) => return Err(e.into()),
                }
                self.status.heartbeat();
                self.hooks.reap();
            };
            if !running && !self.stopping() {
                self.status.set(SERVICE_RUNNING, 0)
            }
            self.context.application_runtime = child.started.elapsed().as_millis();
            self.context.application_started = None;
            self.context.pid = 0;
            self.context.exitcode = exitcode;
            self.context.exit_count = self.context.exit_count.saturating_add(1);
            if cfg.stop.tree {
                process::stop(&child, cfg.stop, || self.status.heartbeat())?;
            }
            event(
                &self.service,
                false,
                &format!("Application exited with code {exitcode}"),
            );
            self.hooks
                .asynchronous(&self.context, "Exit", "Post", None, Some(&streams));
            // Outstanding hooks may own copies of log pipes; drain after they finish at stop.
            if self.stopping() {
                self.hooks.finish(|| self.status.heartbeat());
                drop(streams);
                return Ok(0);
            }
            let (action, default) = cfg.exit_action(exitcode);
            diagnostics::write(
                &self.service,
                Level::Debug,
                &format!("Application exit action: {action}; default: {default}"),
            );
            if cfg.restart != 0 && self.context.throttle == 0 {
                self.context.throttle = 1;
            }
            drop(streams);
            match action.to_ascii_lowercase().as_str() {
                "ignore" => {
                    self.status.set(SERVICE_RUNNING, 0);
                    while !self.stopping() {
                        match self.receiver.recv_timeout(Duration::from_secs(1)) {
                            Ok((control, kind)) => self.command(control, kind, None),
                            Err(RecvTimeoutError::Timeout) => {}
                            Err(e) => return Err(e.into()),
                        }
                        self.hooks.reap();
                    }
                    self.stop_hook(None);
                    return Ok(0);
                }
                "exit" => return Ok(exitcode),
                "suicide" => {
                    if default && exitcode == 0 {
                        return Ok(0);
                    }
                    self.hooks.finish(|| self.status.heartbeat());
                    std::process::exit(exitcode as i32)
                }
                _ => {
                    let consecutive = if self.context.throttle == 0 {
                        0
                    } else {
                        self.context.throttle.saturating_add(1)
                    };
                    delay = restart_delay(consecutive, cfg.restart);
                }
            }
        }
    }
}
unsafe extern "system" fn service_main(argc: u32, argv: *mut *mut u16) {
    let result = std::panic::catch_unwind(|| {
        if argc == 0 || argv.is_null() {
            return;
        }
        let ptr = unsafe { *argv };
        if ptr.is_null() {
            return;
        }
        let mut len = 0;
        while unsafe { *ptr.add(len) } != 0 {
            len += 1;
        }
        let name = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(ptr, len) });
        let (sender, receiver) = mpsc::channel();
        shared().stop.store(false, Ordering::Release);
        if let Ok(mut guard) = shared().sender.lock() {
            *guard = Some(sender)
        }
        let status = match Status::new(&name) {
            Ok(status) => status,
            Err(e) => {
                event(&name, true, &format!("Register SCM handler: {e:#}"));
                return;
            }
        };
        let mut session = Session {
            service: name.clone(),
            receiver,
            status,
            hooks: Hooks::default(),
            context: hooks::Context {
                service: name.clone(),
                display: name.clone(),
                command: String::new(),
                directory: String::new(),
                environment: Environment::new(),
                runtime: Instant::now(),
                application_started: None,
                application_runtime: 0,
                pid: 0,
                exitcode: 0,
                start_requested: 0,
                start_count: 0,
                exit_count: 0,
                throttle: 0,
                last_control: 0,
                redirect: false,
            },
        };
        let code = match session.run() {
            Ok(code) => code,
            Err(e) => {
                event(&name, true, &format!("Service failed: {e:#}"));
                1
            }
        };
        session.finish(code);
        if let Ok(mut guard) = shared().sender.lock() {
            *guard = None;
        }
    });
    if result.is_err() {
        event("xsm", true, "Service callback panic was contained");
    }
}
pub fn dispatch() -> Result<bool> {
    let mut name = process::wide("xsm")?;
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_mut_ptr(),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: null_mut(),
            lpServiceProc: None,
        },
    ];
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_FAILED_SERVICE_CONTROLLER_CONNECT as i32) {
            return Ok(false);
        }
        return Err(error).context("StartServiceCtrlDispatcherW");
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_backoff_is_bounded_and_delay_is_a_floor() {
        assert_eq!(restart_delay(0, 0), 0);
        assert_eq!(restart_delay(2, 0), 2000);
        assert_eq!(restart_delay(8, 0), 128000);
        assert_eq!(restart_delay(u32::MAX, 0), 128000);
        assert_eq!(restart_delay(2, 5000), 5000);
    }
    #[test]
    fn affinity_uses_cpu_indices_and_ranges() {
        assert_eq!(affinity("3").unwrap(), 8);
        assert_eq!(affinity("0-2,5").unwrap(), 39);
        assert_eq!(affinity("All").unwrap(), 0);
        assert!(affinity("64").is_err());
    }
}
