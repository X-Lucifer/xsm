#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::Command;

fn xsm_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xsm"));
    #[cfg(windows)]
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    command
}

#[cfg(windows)]
#[test]
fn executable_uses_console_subsystem_for_synchronous_shell_commands() {
    let executable = std::fs::read(env!("CARGO_BIN_EXE_xsm")).unwrap();
    assert_eq!(&executable[..2], b"MZ");
    let pe = u32::from_le_bytes(executable[0x3c..0x40].try_into().unwrap()) as usize;
    assert_eq!(&executable[pe..pe + 4], b"PE\0\0");
    let subsystem = pe + 4 + 20 + 68;
    assert_eq!(
        u16::from_le_bytes(executable[subsystem..subsystem + 2].try_into().unwrap()),
        3
    );
}

#[cfg(windows)]
#[test]
fn command_scripts_preserve_output_redirection_and_exit_codes() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("cli-script-{nonce}"));
    std::fs::create_dir(&directory).unwrap();
    let script = directory.join("contract.cmd");
    std::fs::copy(env!("CARGO_BIN_EXE_xsm"), directory.join("xsm.exe")).unwrap();
    // Test the actual shipped names and extension-free shell resolution.
    let executable = directory.join("xsm").to_string_lossy().into_owned();
    std::fs::write(&script, format!(
        "@echo off\r\n\"{executable}\" version > \"{}\" 2> \"{}\"\r\nif errorlevel 1 exit /b 10\r\n\"{executable}\" get >nul 2>> \"{}\"\r\nif not errorlevel 1 exit /b 11\r\nexit /b 0\r\n",
        directory.join("out.txt").display(), directory.join("error.txt").display(), directory.join("error.txt").display())).unwrap();
    let output = Command::new("cmd.exe")
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .args(["/D", "/C"])
        .arg(&script)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(
        std::fs::read_to_string(directory.join("out.txt"))
            .unwrap()
            .contains(concat!("xsm ", env!("CARGO_PKG_VERSION")))
    );
    assert!(
        !std::fs::read(directory.join("error.txt"))
            .unwrap()
            .is_empty()
    );
    let escaped = executable.replace('\'', "''");
    let powershell = format!(
        "$ErrorActionPreference='Continue'; $v=& '{escaped}' version; if($LASTEXITCODE -ne 0 -or ($v -join '') -notmatch 'xsm') {{exit 20}}; $bad=& '{escaped}' get 2>$null; if($LASTEXITCODE -ne 1) {{exit 21}}; Write-Output $v; exit 0"
    );
    let output = Command::new("powershell.exe")
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .args(["-NoProfile", "-NonInteractive", "-Command", &powershell])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains(concat!("xsm ", env!("CARGO_PKG_VERSION")))
    );
    for name in ["contract.cmd", "out.txt", "error.txt", "xsm.exe"] {
        std::fs::remove_file(directory.join(name)).unwrap();
    }
    std::fs::remove_dir(directory).unwrap();
}

#[cfg(windows)]
#[test]
fn interactive_no_arguments_exits_quietly_without_opening_gui() {
    use std::{process::Stdio, time::Instant};
    let mut child = xsm_command()
        .env_remove("XSM_LOG_FILE")
        .env("XSM_LOG_LEVEL", "off")
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("A non-service invocation without arguments must exit promptly");
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(
        output.stdout.is_empty() && output.stderr.is_empty(),
        "{output:?}"
    );
}
#[test]
fn executable_version_and_help() {
    for flag in ["version", "--version", "-v", "/version"] {
        let output = xsm_command().arg(flag).output().unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8(output.stdout).unwrap().contains("xsm"));
    }
    let output = xsm_command().arg("help").output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    for command in [
        "install",
        "remove",
        "get",
        "set",
        "reset",
        "dump",
        "processes",
        "rotate",
        "statuscode",
    ] {
        assert!(text.contains(command));
    }
}
#[test]
fn malformed_commands_fail() {
    for args in [
        vec!["get"],
        vec!["set", "test", "Application"],
        vec!["unknown-command"],
        vec!["remove", "test", "yes"],
    ] {
        let output = xsm_command().args(args).output().unwrap();
        assert!(!output.status.success());
        assert!(!output.stderr.is_empty());
    }
}

#[test]
fn removed_entry_is_rejected_and_help_advertises_install_gui() {
    let output = xsm_command().arg("manage").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(!output.stderr.is_empty());
    for (locale, explanation) in [
        ("zh-CN", "install 未提供应用程序时打开可视化管理界面"),
        (
            "en-US",
            "Install without an application opens the service management window",
        ),
    ] {
        let output = xsm_command()
            .env("XSM_LANG", locale)
            .arg("help")
            .output()
            .unwrap();
        assert!(output.status.success());
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(help.contains(explanation), "{help}");
        assert!(
            !help
                .lines()
                .any(|line| line.split_whitespace().next() == Some("manage"))
        );
        assert!(!help.contains("--elevated-gui"));
    }
}

#[test]
fn help_supports_chinese_and_english_without_changing_commands() {
    for (locale, expected) in [
        ("zh-CN", "Windows 服务管理器"),
        ("en-US", "Windows service manager"),
    ] {
        let output = xsm_command()
            .env("XSM_LANG", locale)
            .arg("help")
            .output()
            .unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains(expected), "{locale}: {text}");
        assert!(text.contains("statuscode"));
    }
}

#[cfg(windows)]
#[test]
fn failed_account_command_keeps_password_out_of_diagnostics() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("cli-log-contract-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let base = directory.join("internal.log");
    let missing = format!("Xsm_Missing_{nonce}");
    let secret = "Private-test-password-不应记录-🔑";
    // Exercise the executable's normal error return, including parsed argument
    // ownership and logging, without creating or modifying any service.
    for _ in 0..2 {
        let output = xsm_command()
            .env("XSM_LOG_LEVEL", "debug")
            .env("XSM_LOG_FILE", &base)
            .args(["set", &missing, "ObjectName", ".\\AbsentUser", secret])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
    }
    let latest = directory.join("internal-latest.log");
    let text = std::fs::read_to_string(&latest).unwrap();
    let header = "Time | Level | Service | PID | Operation | Message";
    assert_eq!(text.lines().next(), Some(header));
    assert_eq!(text.lines().filter(|line| *line == header).count(), 1);
    assert!(text.contains(" | DEBUG | "));
    assert!(text.contains(" | ERROR | "));
    assert!(!text.contains(secret));
    assert!(!text.contains("level="));
    assert!(!text.contains("service="));
    assert!(!text.contains("operation="));
    assert!(!text.contains("message="));
    std::fs::remove_file(latest).unwrap();
    std::fs::remove_dir(directory).unwrap();
}
#[cfg(windows)]
#[test]
fn nonexistent_statuscode_is_zero_for_xsm_compatibility() {
    let output = xsm_command()
        .args([
            "statuscode",
            "Xsm_Missing_21A8A831_53B3_45F3_BF5B_8DE27CECA3DF",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(!output.stderr.is_empty());
}
#[cfg(windows)]
#[test]
fn quote_round_trips_through_windows_parser() {
    use windows_sys::Win32::{Foundation::LocalFree, UI::Shell::CommandLineToArgvW};
    // Deterministic combinatorial corpus exercises empty arguments, Unicode,
    // backslashes and quotes without introducing a fuzzing dependency.
    let alphabet = ["", "a", " ", "\t", "\\", "\"", "测试", "😀", "%", "&"];
    for a in alphabet {
        for b in alphabet {
            for c in alphabet {
                let original = format!("{a}{b}{c}");
                let line: Vec<u16> = format!("probe {}", xsm::quoting::quote_arg(&original))
                    .encode_utf16()
                    .chain(Some(0))
                    .collect();
                let mut count = 0;
                // SAFETY: line is NUL-terminated, count is writable; returned argv is freed
                // only after reading its documented count and NUL-terminated strings.
                unsafe {
                    let argv = CommandLineToArgvW(line.as_ptr(), &mut count);
                    assert!(!argv.is_null());
                    assert_eq!(count, 2);
                    let item = *argv.add(1);
                    let mut len = 0;
                    while *item.add(len) != 0 {
                        len += 1;
                    }
                    let actual = String::from_utf16(std::slice::from_raw_parts(item, len)).unwrap();
                    LocalFree(argv.cast());
                    assert_eq!(actual, original);
                }
            }
        }
    }
}
