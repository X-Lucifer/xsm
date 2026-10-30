use anyhow::Result;
use xsm::cli::{self, Command};

fn main() {
    // Finish the command scope before exiting so sensitive argument guards run.
    std::process::exit(entry());
}

fn entry() -> i32 {
    xsm::i18n::initialize();
    let args = match cli::Arguments::from_env() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error:#}");
            return 1;
        }
    };
    let parsed = cli::parse(args.as_slice());
    #[cfg(windows)]
    if let Err(error) = xsm::windows::diagnostics::initialize() {
        eprintln!("Cannot initialize xsm logging: {error:#}");
        return 1;
    }
    let command = match parsed {
        Ok(command) => command,
        Err(error) => {
            #[cfg(windows)]
            xsm::windows::diagnostics::write(
                "cli",
                xsm::windows::diagnostics::Level::Error,
                "Command failed; diagnostic details are written to stderr",
            );
            eprintln!("{error:#}");
            return if args
                .as_slice()
                .first()
                .is_some_and(|s| s.eq_ignore_ascii_case("remove"))
            {
                100
            } else {
                1
            };
        }
    };
    let statuscode = matches!(&command, Command::Control { verb, .. } if verb == "statuscode");
    #[cfg(windows)]
    if command.requires_automatic_elevation() && !xsm::windows::elevation::is_elevated() {
        return match xsm::windows::elevation::relaunch(args.as_slice()) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("{error:#}");
                1
            }
        };
    }
    #[cfg(windows)]
    let (operation, service) = {
        let (operation, service) = command.log_context();
        (operation.to_owned(), service.to_owned())
    };
    #[cfg(windows)]
    if !matches!(&command, Command::Dispatch) {
        // The service runtime owns its lifecycle records and service identity.
        // The generic dispatcher has no service identity that can safely be
        // logged after a service-specific runtime returns.
        xsm::windows::diagnostics::write_operation(
            &service,
            xsm::windows::diagnostics::Level::Debug,
            &operation,
            "Command started",
        );
    }
    match run(&command) {
        Ok(code) => {
            #[cfg(windows)]
            if !matches!(&command, Command::Dispatch) {
                xsm::windows::diagnostics::write_operation(
                    &service,
                    xsm::windows::diagnostics::Level::Debug,
                    &operation,
                    &format!("Command completed with status {code}"),
                );
            }
            code
        }
        Err(error) => {
            #[cfg(windows)]
            {
                let detached_gui = matches!(&command, Command::ElevatedGui);
                xsm::windows::diagnostics::write_operation(
                    &service,
                    xsm::windows::diagnostics::Level::Error,
                    &operation,
                    if detached_gui {
                        "Graphical manager startup failed; diagnostic details are shown in a dialog"
                    } else {
                        "Command failed; diagnostic details are written to stderr"
                    },
                );
                if detached_gui {
                    xsm::windows::elevation::show_gui_startup_error(&error);
                    return 1;
                }
            }
            eprintln!("{error:#}");
            if statuscode { 0 } else { 1 }
        }
    }
}

fn run(command: &Command) -> Result<i32> {
    match command {
        Command::Help => {
            println!("{}", cli::help());
            Ok(0)
        }
        Command::Version => {
            println!("xsm {} {}-bit", env!("CARGO_PKG_VERSION"), usize::BITS);
            Ok(0)
        }
        #[cfg(windows)]
        other => run_windows(other),
        #[cfg(not(windows))]
        _ => anyhow::bail!("Service management is only available on Windows"),
    }
}

#[cfg(windows)]
fn run_windows(command: &Command) -> Result<i32> {
    use xsm::{
        settings,
        windows::{
            gui::{self, GuiMode},
            runtime, scm,
        },
    };
    match command {
        Command::Dispatch => {
            // An interactive invocation without arguments exits quietly. Only
            // an SCM launch enters the service runtime; GUI use is explicit.
            runtime::dispatch()?;
        }
        Command::ElevatedGui => {
            xsm::windows::elevation::detach_gui_console()?;
            gui::run(GuiMode::ElevatedInstall)?;
        }
        Command::Install {
            service: Some(service),
            application: Some(application),
            parameters,
        } => {
            scm::install(service, application, parameters)?;
            if xsm::i18n::language() == xsm::i18n::Language::ZhCn {
                println!("服务“{service}”安装成功。");
            } else {
                println!("Service \"{service}\" installed successfully!");
            }
        }
        Command::Install { service, .. } => gui::run(GuiMode::Install(service.clone()))?,
        Command::Edit(service) => gui::run(GuiMode::Edit(service.clone()))?,
        Command::Remove {
            service: Some(service),
            confirmed: true,
        } => {
            scm::remove(service)?;
            if xsm::i18n::language() == xsm::i18n::Language::ZhCn {
                println!("服务“{service}”已移除。");
            } else {
                println!("Service \"{service}\" removed successfully!");
            }
        }
        Command::Remove { service, .. } => gui::run(GuiMode::Remove(service.clone()))?,
        Command::Get {
            service,
            setting,
            additional,
        } => println!("{}", settings::get(service, setting, additional)?),
        Command::Set {
            service,
            setting,
            values,
        } => {
            settings::set(service, setting, values)?;
            if xsm::i18n::language() == xsm::i18n::Language::ZhCn {
                println!("已设置服务“{service}”的“{setting}”。");
            } else {
                println!("Set parameter \"{setting}\" for service \"{service}\".");
            }
        }
        Command::Reset {
            service,
            setting,
            additional,
        } => {
            settings::reset(service, setting, additional)?;
            if xsm::i18n::language() == xsm::i18n::Language::ZhCn {
                println!("已将服务“{service}”的“{setting}”重置为默认值。");
            } else {
                println!("Reset parameter \"{setting}\" for service \"{service}\" to its default.");
            }
        }
        Command::Dump { service, rename } => {
            print!("{}", settings::dump(service, rename.as_deref())?)
        }
        Command::List { all } => {
            for name in scm::list(*all)? {
                println!("{name}");
            }
        }
        Command::Processes(services) => {
            let mut errors = 0;
            for service in services {
                match scm::processes(service) {
                    Ok(output) => print!("{output}"),
                    Err(error) => {
                        eprintln!("{error:#}");
                        errors += 1;
                    }
                }
            }
            return Ok(errors);
        }
        Command::Control {
            verb,
            service,
            arguments,
        } => {
            if verb == "status" || verb == "statuscode" {
                let state = scm::status(service)?;
                println!("{}", cli::state_name(state));
                return Ok(if verb == "statuscode" {
                    state as i32
                } else {
                    0
                });
            }
            if verb == "restart" {
                scm::control(service, "stop", &[])?;
            }
            let control = if verb == "restart" { "start" } else { verb };
            scm::control(service, control, arguments)?;
            println!(
                "{}: {}: The operation completed successfully.",
                scm::resolve_name(service)?,
                control.to_ascii_uppercase()
            );
        }
        Command::Help | Command::Version => unreachable!("handled before platform dispatch"),
    }
    Ok(0)
}
