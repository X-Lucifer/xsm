//! Hook invocations snapshot their environment; no global environment mutation.
use super::{
    diagnostics::{self, Level},
    logging::{Streams, event},
    process::{self, Environment, Spawn, StopPolicy},
};
use anyhow::Result;
use std::{
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
use winreg::{RegKey, enums::HKEY_LOCAL_MACHINE};

#[derive(Clone)]
pub struct Context {
    pub service: String,
    pub display: String,
    pub command: String,
    pub directory: String,
    pub environment: Environment,
    pub runtime: Instant,
    pub application_started: Option<Instant>,
    pub application_runtime: u128,
    pub pid: u32,
    pub exitcode: u32,
    pub start_requested: u32,
    pub start_count: u32,
    pub exit_count: u32,
    pub throttle: u32,
    pub last_control: u32,
    pub redirect: bool,
}
pub fn control_name(control: u32) -> &'static str {
    match control {
        0 => "START",
        1 => "STOP",
        2 => "PAUSE",
        3 => "CONTINUE",
        4 => "INTERROGATE",
        5 => "SHUTDOWN",
        6 => "PARAMCHANGE",
        13 => "POWEREVENT",
        128 => "ROTATE",
        _ => "UNKNOWN",
    }
}
fn hook_environment(
    ctx: &Context,
    event: &str,
    action: &str,
    trigger: Option<u32>,
    deadline: u32,
) -> Environment {
    let mut env = ctx.environment.clone();
    let mut set = |key: &str, value: String| {
        env.insert(key.into(), (key.into(), value));
    };
    for (key, value) in [
        ("XSM_HOOK_VERSION", "1"),
        (
            "XSM_CONFIGURATION",
            if cfg!(target_pointer_width = "64") {
                "64-bit"
            } else {
                "32-bit"
            },
        ),
        ("XSM_VERSION", env!("CARGO_PKG_VERSION")),
        ("XSM_BUILD_DATE", env!("XSM_BUILD_DATE")),
        ("XSM_EVENT", event),
        ("XSM_ACTION", action),
        ("XSM_SERVICE_NAME", &ctx.service),
        ("XSM_SERVICE_DISPLAYNAME", &ctx.display),
        ("XSM_COMMAND_LINE", &ctx.command),
        ("XSM_LAST_CONTROL", control_name(ctx.last_control)),
        ("XSM_TRIGGER", trigger.map(control_name).unwrap_or("")),
    ] {
        set(key, value.into());
    }
    set(
        "XSM_EXE",
        std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    set("XSM_PID", std::process::id().to_string());
    set("XSM_RUNTIME", ctx.runtime.elapsed().as_millis().to_string());
    set("XSM_DEADLINE", deadline.to_string());
    set("XSM_START_REQUESTED_COUNT", ctx.start_requested.to_string());
    set("XSM_START_COUNT", ctx.start_count.to_string());
    set("XSM_EXIT_COUNT", ctx.exit_count.to_string());
    set("XSM_THROTTLE_COUNT", ctx.throttle.to_string());
    let pre = event == "Start" && action == "Pre";
    set(
        "XSM_APPLICATION_PID",
        if ctx.pid == 0 {
            String::new()
        } else {
            ctx.pid.to_string()
        },
    );
    set(
        "XSM_APPLICATION_RUNTIME",
        if ctx.pid == 0 && (pre || ctx.exit_count == 0) {
            String::new()
        } else {
            ctx.application_started
                .map(|s| s.elapsed().as_millis())
                .unwrap_or(ctx.application_runtime)
                .to_string()
        },
    );
    set(
        "XSM_EXITCODE",
        if ctx.pid != 0 || pre {
            String::new()
        } else {
            ctx.exitcode.to_string()
        },
    );
    env
}
fn command(service: &str, event: &str, action: &str) -> Result<Option<String>> {
    let root = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = match root.open_subkey(format!(
        "SYSTEM\\CurrentControlSet\\Services\\{service}\\Parameters\\AppEvents\\{event}"
    )) {
        Ok(key) => key,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    match key.get_value::<String, _>(action) {
        Ok(command) => Ok((!command.is_empty()).then_some(command)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
struct Running {
    child: process::Child,
    streams: Option<Streams>,
    deadline: u32,
    service: String,
    name: String,
}
impl Running {
    fn wait(self, mut progress: impl FnMut() -> bool) -> u32 {
        let started = self.child.started;
        let mut status = 102;
        loop {
            if progress() {
                status = 101;
                break;
            }
            match self.child.exit_code() {
                Ok(Some(0)) => {
                    status = 0;
                    break;
                }
                Ok(Some(99)) => {
                    status = 99;
                    break;
                }
                Ok(Some(_)) => {
                    status = 111;
                    break;
                }
                Err(_) => {
                    status = 100;
                    break;
                }
                Ok(None) => {}
            }
            if started.elapsed() >= Duration::from_millis(u64::from(self.deadline)) {
                break;
            }
            self.child.wait(100);
        }
        if let Err(e) = process::stop(&self.child, StopPolicy::default(), || {
            progress();
        }) {
            event(
                &self.service,
                true,
                &format!("Hook {} cleanup: {e:#}", self.name),
            );
        }
        drop(self.streams);
        diagnostics::write(
            &self.service,
            Level::Debug,
            &format!("Hook {} completed with status {status}", self.name),
        );
        if status != 0 {
            event(
                &self.service,
                true,
                &format!("Hook {} returned status {status}", self.name),
            );
        }
        status
    }
}
#[derive(Default)]
pub struct Hooks {
    threads: Vec<JoinHandle<u32>>,
}
impl Hooks {
    fn launch(
        &mut self,
        ctx: &Context,
        event: &str,
        action: &str,
        trigger: Option<u32>,
        deadline: u32,
        streams: Option<&Streams>,
    ) -> Result<Option<Running>> {
        let Some(command) = command(&ctx.service, event, action)? else {
            return Ok(None);
        };
        let environment = hook_environment(ctx, event, action, trigger, deadline);
        let streams = if ctx.redirect {
            streams.map(Streams::duplicate).transpose()?
        } else {
            None
        };
        let command = process::expand(&command, &environment);
        let child = process::spawn(Spawn {
            application: None,
            command: &command,
            directory: &ctx.directory,
            environment: &environment,
            stdio: streams.as_ref().map(Streams::handles),
            flags: CREATE_NO_WINDOW,
            affinity: 0,
        })?;
        diagnostics::write(
            &ctx.service,
            Level::Debug,
            &format!(
                "Hook {event}/{action} started (PID {}, deadline {deadline} ms)",
                child.pid
            ),
        );
        Ok(Some(Running {
            child,
            streams,
            deadline,
            service: ctx.service.clone(),
            name: format!("{event}/{action}"),
        }))
    }
    #[allow(clippy::too_many_arguments)]
    pub fn sync(
        &mut self,
        ctx: &Context,
        event_name: &str,
        action: &str,
        trigger: Option<u32>,
        deadline: u32,
        streams: Option<&Streams>,
        mut progress: impl FnMut(),
    ) -> u32 {
        match self.launch(ctx, event_name, action, trigger, deadline, streams) {
            Ok(Some(running)) => running.wait(|| {
                progress();
                event_name != "Stop" && super::runtime::stop_requested()
            }),
            Ok(None) => 1,
            Err(e) => {
                event(
                    &ctx.service,
                    true,
                    &format!("Hook {event_name}/{action}: {e:#}"),
                );
                100
            }
        }
    }
    pub fn asynchronous(
        &mut self,
        ctx: &Context,
        event_name: &str,
        action: &str,
        trigger: Option<u32>,
        streams: Option<&Streams>,
    ) {
        self.reap();
        match self.launch(ctx, event_name, action, trigger, 60000, streams) {
            Ok(Some(running)) => {
                match thread::Builder::new()
                    .name("xsm-hook".into())
                    .spawn(move || running.wait(|| false))
                {
                    Ok(thread) => self.threads.push(thread),
                    Err(e) => event(&ctx.service, true, &format!("Hook monitor: {e}")),
                }
            }
            Ok(None) => {}
            Err(e) => event(
                &ctx.service,
                true,
                &format!("Hook {event_name}/{action}: {e:#}"),
            ),
        }
    }
    pub fn reap(&mut self) {
        let mut index = 0;
        while index < self.threads.len() {
            if self.threads[index].is_finished() {
                let handle = self.threads.swap_remove(index);
                let _ = handle.join();
            } else {
                index += 1
            }
        }
    }
    pub fn finish(&mut self, mut progress: impl FnMut()) {
        while !self.threads.is_empty() {
            self.reap();
            if self.threads.is_empty() {
                break;
            }
            progress();
            thread::sleep(Duration::from_millis(100));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn context() -> Context {
        Context {
            service: "service".into(),
            display: "Display".into(),
            command: "app".into(),
            directory: String::new(),
            environment: Environment::new(),
            runtime: Instant::now(),
            application_started: None,
            application_runtime: 123,
            pid: 0,
            exitcode: 7,
            start_requested: 2,
            start_count: 1,
            exit_count: 1,
            throttle: 1,
            last_control: 1,
            redirect: false,
        }
    }
    #[test]
    fn hook_exit_snapshot_has_exit_information() {
        let env = hook_environment(&context(), "Exit", "Post", None, 60000);
        assert_eq!(env["XSM_EXITCODE"].1, "7");
        assert_eq!(env["XSM_APPLICATION_RUNTIME"].1, "123");
        assert_eq!(env["XSM_TRIGGER"].1, "");
    }
    #[test]
    fn prestart_clears_stale_exit_information() {
        let env = hook_environment(&context(), "Start", "Pre", Some(0), 20000);
        assert_eq!(env["XSM_EXITCODE"].1, "");
        assert_eq!(env["XSM_APPLICATION_RUNTIME"].1, "");
        assert_eq!(env["XSM_TRIGGER"].1, "START");
    }
    #[test]
    fn no_application_runtime_is_reported_before_first_launch() {
        let mut ctx = context();
        ctx.exit_count = 0;
        let env = hook_environment(&ctx, "Stop", "Pre", Some(5), 20000);
        assert_eq!(env["XSM_APPLICATION_RUNTIME"].1, "");
        assert_eq!(env["XSM_TRIGGER"].1, "SHUTDOWN");
    }
}
