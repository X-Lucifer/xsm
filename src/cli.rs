//! XSM command grammar, independent of privileges and the Windows SCM.
#![forbid(unsafe_code)]
use anyhow::{Result, bail};

pub(crate) const ELEVATED_GUI_FLAG: &str = "--elevated-gui";

/// Own process arguments so their Rust buffers are cleared on every return path.
/// Windows retains its original process command line independently of this copy.
pub struct Arguments(Vec<String>);
impl Arguments {
    pub fn from_env() -> Result<Self> {
        let mut args = Self(Vec::new());
        for argument in std::env::args_os().skip(1) {
            let argument = argument
                .into_string()
                .map_err(|_| anyhow::anyhow!("Command line contains invalid Unicode"))?;
            args.0.push(argument);
        }
        Ok(args)
    }

    pub fn as_slice(&self) -> &[String] {
        &self.0
    }
}
impl Drop for Arguments {
    fn drop(&mut self) {
        for value in &mut self.0 {
            crate::secret::wipe_string(value);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Dispatch,
    Help,
    Version,
    ElevatedGui,
    Install {
        service: Option<String>,
        application: Option<String>,
        parameters: String,
    },
    Edit(String),
    Remove {
        service: Option<String>,
        confirmed: bool,
    },
    Get {
        service: String,
        setting: String,
        additional: Vec<String>,
    },
    Set {
        service: String,
        setting: String,
        values: Vec<String>,
    },
    Reset {
        service: String,
        setting: String,
        additional: Vec<String>,
    },
    Dump {
        service: String,
        rename: Option<String>,
    },
    Control {
        verb: String,
        service: String,
        arguments: Vec<String>,
    },
    List {
        all: bool,
    },
    Processes(Vec<String>),
}

impl Drop for Command {
    fn drop(&mut self) {
        // Parsed values are independent allocations from Arguments and can also
        // contain account passwords, environment secrets or application tokens.
        match self {
            Self::Install { parameters, .. } => crate::secret::wipe_string(parameters),
            Self::Set { values, .. }
            | Self::Get {
                additional: values, ..
            }
            | Self::Reset {
                additional: values, ..
            }
            | Self::Control {
                arguments: values, ..
            } => {
                for value in values {
                    crate::secret::wipe_string(value);
                }
            }
            _ => {}
        }
    }
}

impl Command {
    /// The unnamed installer also provides read-only service management. Named
    /// installation forms retain automatic elevation and their prefilled name.
    pub fn requires_automatic_elevation(&self) -> bool {
        matches!(
            self,
            Self::Install {
                service: Some(_),
                ..
            } | Self::Remove { .. }
                | Self::Edit(_)
        )
    }

    /// Deliberately excludes argument values, passwords and environment data.
    pub fn log_context(&self) -> (&str, &str) {
        match self {
            Self::Dispatch => ("dispatch", "process"),
            Self::Help => ("help", "cli"),
            Self::Version => ("version", "cli"),
            Self::ElevatedGui => ("install", "gui"),
            Self::Install { service, .. } => ("install", service.as_deref().unwrap_or("gui")),
            Self::Edit(service) => ("edit", service),
            Self::Remove { service, .. } => ("remove", service.as_deref().unwrap_or("gui")),
            Self::Get { service, .. } => ("get", service),
            Self::Set { service, .. } => ("set", service),
            Self::Reset { service, .. } => ("reset", service),
            Self::Dump { service, .. } => ("dump", service),
            Self::Control { service, verb, .. } => (verb, service),
            Self::List { .. } => ("list", "cli"),
            Self::Processes(_) => ("processes", "cli"),
        }
    }
}

pub fn parse(args: &[String]) -> Result<Command> {
    let Some(verb) = args.first() else {
        return Ok(Command::Dispatch);
    };
    if args.iter().any(|arg| arg.contains('\0')) {
        bail!("Arguments cannot contain NUL");
    }
    let tail = &args[1..];
    let required = |n: usize| -> Result<()> {
        if tail.len() < n {
            bail!(
                "{}: {verb}. {}",
                crate::i18n::text("缺少命令参数", "Missing command arguments"),
                crate::i18n::text("使用 xsm help 查看帮助", "Use xsm help for usage")
            );
        }
        Ok(())
    };
    Ok(match verb.to_ascii_lowercase().as_str() {
        "version" | "/version" | "-version" | "--version" | "-v" => Command::Version,
        "help" | "--help" | "-h" | "/?" | "-?" => Command::Help,
        ELEVATED_GUI_FLAG if tail.is_empty() && verb == ELEVATED_GUI_FLAG => Command::ElevatedGui,
        "install" => Command::Install {
            service: tail.first().cloned(),
            application: tail.get(1).cloned(),
            // Deliberately preserve XSM's concatenation. Requoting would break scripts
            // which already pass embedded quotes for AppParameters.
            parameters: tail.get(2..).unwrap_or_default().join(" "),
        },
        "edit" => {
            required(1)?;
            Command::Edit(tail[0].clone())
        }
        "remove" => {
            if tail.len() > 1 && !tail[1].eq_ignore_ascii_case("confirm") {
                bail!("Use xsm remove <service> confirm to remove without a dialog");
            }
            Command::Remove {
                service: tail.first().cloned(),
                confirmed: tail.len() > 1,
            }
        }
        "get" | "set" | "reset" | "unset" => {
            required(if verb.eq_ignore_ascii_case("set") {
                3
            } else {
                2
            })?;
            let service = tail[0].clone();
            let setting = tail[1].clone();
            let values = tail[2..].to_vec();
            match verb.to_ascii_lowercase().as_str() {
                "get" => Command::Get {
                    service,
                    setting,
                    additional: values,
                },
                "set" => Command::Set {
                    service,
                    setting,
                    values,
                },
                _ => Command::Reset {
                    service,
                    setting,
                    additional: values,
                },
            }
        }
        "dump" => {
            required(1)?;
            Command::Dump {
                service: tail[0].clone(),
                rename: tail.get(1).cloned(),
            }
        }
        "start" | "stop" | "restart" | "pause" | "continue" | "status" | "statuscode"
        | "rotate" => {
            required(1)?;
            Command::Control {
                verb: verb.to_ascii_lowercase(),
                service: tail[0].clone(),
                arguments: tail[1..].to_vec(),
            }
        }
        "list" => Command::List {
            all: tail.first().is_some_and(|v| v.eq_ignore_ascii_case("all")),
        },
        "processes" => {
            required(1)?;
            Command::Processes(tail.to_vec())
        }
        _ => bail!("Unknown command: {verb}. Use xsm help."),
    })
}

pub fn state_name(state: u32) -> &'static str {
    match state {
        1 => "SERVICE_STOPPED",
        2 => "SERVICE_START_PENDING",
        3 => "SERVICE_STOP_PENDING",
        4 => "SERVICE_RUNNING",
        5 => "SERVICE_CONTINUE_PENDING",
        6 => "SERVICE_PAUSE_PENDING",
        7 => "SERVICE_PAUSED",
        _ => "SERVICE_UNKNOWN",
    }
}

pub const HELP: &str = "XSM Windows service manager\n\
Usage: xsm <command> [arguments]\n\
  install [service] [application] [arguments ...]\n\
  edit <service>\n\
  remove [service] [confirm]\n\
  get <service> <setting> [subparameter]\n\
  set <service> <setting> [subparameter] <value ...>\n\
  reset|unset <service> <setting> [subparameter]\n\
  start|stop|restart|pause|continue|status|statuscode|rotate <service>\n\
  list [all]\n\
  processes <service ...>\n\
  dump <service> [new-service-name]\n\
  version\n\
Install without an application opens the service management window.\n\
Remove without confirmation opens a dialog.\n\
Service display names are accepted. statuscode returns the Windows service state as its exit code.";

pub fn help() -> &'static str {
    crate::i18n::text(
        "XSM — Windows 服务管理器\n\
用法：xsm <命令> [参数]\n\
  install [服务名称] [应用程序] [应用参数 ...]\n\
  edit <服务名称>\n\
  remove [服务名称] [confirm]\n\
  get <服务名称> <设置> [子项]\n\
  set <服务名称> <设置> [子项] <值 ...>\n\
  reset|unset <服务名称> <设置> [子项]\n\
  start|stop|restart|pause|continue|status|statuscode|rotate <服务名称>\n\
  list [all]\n\
  processes <服务名称 ...>\n\
  dump <服务名称> [新服务名称]\n\
  version\n\
install 未提供应用程序时打开可视化管理界面。\n\
remove 未确认时打开对话框；支持使用服务显示名称。\n\
statuscode 通过退出码返回 Windows 服务状态。\n\
设置 XSM_LANG=zh-CN 或 XSM_LANG=en-US 可选择显示语言。",
        HELP,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p(args: &[&str]) -> Result<Command> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }
    #[test]
    fn version_aliases() {
        for a in [
            "version",
            "VERSION",
            "-v",
            "-V",
            "-version",
            "--version",
            "/version",
        ] {
            assert_eq!(p(&[a]).unwrap(), Command::Version);
        }
    }
    #[test]
    fn install_preserves_existing_quoting() {
        assert_eq!(
            p(&[
                "install",
                "测试 service",
                "C:\\bin\\app.exe",
                "\"hello world\"",
                "-x"
            ])
            .unwrap(),
            Command::Install {
                service: Some("测试 service".into()),
                application: Some("C:\\bin\\app.exe".into()),
                parameters: "\"hello world\" -x".into()
            }
        );
    }
    #[test]
    fn incomplete_install_is_gui() {
        assert!(matches!(
            p(&["install"]).unwrap(),
            Command::Install {
                service: None,
                application: None,
                ..
            }
        ));
        assert!(matches!(
            p(&["install", "demo"]).unwrap(),
            Command::Install {
                service: Some(_),
                application: None,
                ..
            }
        ));
    }
    #[test]
    fn reject_accidental_removal() {
        assert!(p(&["remove", "x", "yes"]).is_err());
    }
    #[test]
    fn missing_arguments_fail_without_side_effects() {
        for verb in [
            "edit",
            "get",
            "set",
            "reset",
            "unset",
            "dump",
            "start",
            "stop",
            "processes",
        ] {
            assert!(p(&[verb]).is_err(), "{verb}");
        }
    }
    #[test]
    fn reset_alias_and_case() {
        assert_eq!(
            p(&["RESET", "s", "AppExit", "Default"]).unwrap(),
            p(&["unset", "s", "AppExit", "Default"]).unwrap()
        );
    }
    #[test]
    fn subparameters_are_opaque() {
        assert_eq!(
            p(&["set", "s", "AppEvents", "Start/Pre", "cmd /c hello"]).unwrap(),
            Command::Set {
                service: "s".into(),
                setting: "AppEvents".into(),
                values: vec!["Start/Pre".into(), "cmd /c hello".into()]
            }
        );
    }
    #[test]
    fn processes_accept_multiple_services() {
        assert_eq!(
            p(&["processes", "one", "two"]).unwrap(),
            Command::Processes(vec!["one".into(), "two".into()])
        );
    }
    #[test]
    fn no_nul() {
        assert!(p(&["install", "bad\0name"]).is_err());
    }
    #[test]
    fn no_args_dispatch() {
        assert_eq!(p(&[]).unwrap(), Command::Dispatch);
    }

    #[test]
    fn elevated_gui_marker_is_an_exact_private_invocation_only() {
        assert_eq!(p(&["--elevated-gui"]).unwrap(), Command::ElevatedGui);
        for args in [
            vec!["manage"],
            vec!["manage", "ordinary-extra-argument"],
            vec!["manage", "--elevated-gui", "extra"],
            vec!["--elevated-gui", "extra"],
            vec!["--ELEVATED-GUI"],
        ] {
            assert!(p(&args).is_err());
        }
        assert_eq!(
            p(&["install", "--elevated-gui"]).unwrap(),
            Command::Install {
                service: Some("--elevated-gui".into()),
                application: None,
                parameters: String::new(),
            }
        );
        assert_eq!(p(&["version", "--elevated-gui"]).unwrap(), Command::Version);
        assert_eq!(
            p(&["install", "example", "app.exe", "--elevated-gui"]).unwrap(),
            Command::Install {
                service: Some("example".into()),
                application: Some("app.exe".into()),
                parameters: "--elevated-gui".into(),
            }
        );
    }

    #[test]
    fn only_the_unnamed_installer_bypasses_automatic_elevation() {
        for args in [vec!["install"], vec!["--elevated-gui"]] {
            assert!(!p(&args).unwrap().requires_automatic_elevation());
        }
        for args in [
            vec!["install", "example"],
            vec!["install", "example", "app.exe"],
            vec!["remove", "example", "confirm"],
            vec!["remove"],
            vec!["edit", "example"],
        ] {
            assert!(p(&args).unwrap().requires_automatic_elevation());
        }
    }
}
