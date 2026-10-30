#![cfg(windows)]

use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsString,
    fs,
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    process::Command,
    ptr::{null, null_mut},
    time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::*,
    Storage::FileSystem::*,
    System::{Console::*, Threading::*},
    UI::WindowsAndMessaging::{IsWindowVisible, SW_HIDE},
};

const REPORT_VARIABLE: &str = "XSM_TEST_CONSOLE_REPORT";
const DETACH_VARIABLE: &str = "XSM_TEST_CONSOLE_DETACH_CHILD";
const HOST_TEST: &str = "hidden_console_host";

fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    value.encode_wide().chain([0]).collect()
}

struct Report(PathBuf);
impl Drop for Report {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

struct Host(OwnedHandle);
impl Drop for Host {
    fn drop(&mut self) {
        // SAFETY: this handle identifies only the helper created by this test.
        unsafe {
            if WaitForSingleObject(self.0.as_raw_handle(), 0) == WAIT_TIMEOUT {
                TerminateProcess(self.0.as_raw_handle(), 1);
                WaitForSingleObject(self.0.as_raw_handle(), 5000);
            }
        }
    }
}

fn host_environment(report: &Path) -> Vec<u16> {
    let mut variables: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(key, _)| {
            ![REPORT_VARIABLE, DETACH_VARIABLE]
                .iter()
                .any(|name| key.to_string_lossy().eq_ignore_ascii_case(name))
        })
        .collect();
    variables.push((REPORT_VARIABLE.into(), report.as_os_str().into()));
    variables.sort_by_cached_key(|(key, _)| key.to_string_lossy().to_uppercase());
    let mut block = Vec::new();
    for (key, value) in variables {
        block.extend(key.encode_wide());
        block.push(b'=' as u16);
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

#[test]
fn cli_inherits_hidden_parent_console_without_losing_pipes() -> Result<()> {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let report = Report(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!(
                "console-inherit-{}-{nonce}.txt",
                std::process::id()
            )),
    );
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&report.0)?;
    let executable = std::env::current_exe()?;
    let application = wide(executable.as_os_str());
    let arguments = format!(
        "{} --ignored --exact {HOST_TEST} --nocapture --test-threads=1",
        xsm::quoting::quote_arg(&executable.to_string_lossy())
    );
    let mut command = wide(std::ffi::OsStr::new(&arguments));
    let environment = host_environment(&report.0);
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        dwFlags: STARTF_USESHOWWINDOW,
        wShowWindow: SW_HIDE as u16,
        ..Default::default()
    };
    let mut information = PROCESS_INFORMATION::default();
    // SAFETY: all UTF-16 buffers and initialized output structures outlive the
    // call. No user handles are inherited. The new console starts hidden.
    if unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            null(),
            null(),
            0,
            CREATE_NEW_CONSOLE | CREATE_UNICODE_ENVIRONMENT,
            environment.as_ptr().cast(),
            null(),
            &startup,
            &mut information,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error()).context("Create hidden console host");
    }
    // SAFETY: successful CreateProcessW transfers both uniquely owned handles.
    let host = Host(unsafe { OwnedHandle::from_raw_handle(information.hProcess) });
    // SAFETY: the thread handle has separate ownership from the process handle.
    let thread = unsafe { OwnedHandle::from_raw_handle(information.hThread) };
    drop(thread);
    // SAFETY: the host process handle remains live throughout the bounded wait.
    let wait = unsafe { WaitForSingleObject(host.0.as_raw_handle(), 30000) };
    ensure!(
        wait == WAIT_OBJECT_0,
        "Hidden console host did not finish: {wait:#x}"
    );
    let mut code = 0;
    // SAFETY: code is writable and host is still owned after process completion.
    ensure!(
        unsafe { GetExitCodeProcess(host.0.as_raw_handle(), &mut code) } != 0,
        "Cannot read console host exit status: {}",
        std::io::Error::last_os_error()
    );
    let evidence = fs::read_to_string(&report.0)?;
    ensure!(
        code == 0 && evidence.starts_with("PASS\n"),
        "Hidden console host failed (exit {code}): {evidence}"
    );
    Ok(())
}

fn console_buffer() -> Result<OwnedHandle> {
    let name = wide(std::ffi::OsStr::new("CONOUT$"));
    // SAFETY: name is NUL terminated, no security/pointer outputs are borrowed.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    };
    ensure!(
        handle != INVALID_HANDLE_VALUE && !handle.is_null(),
        "Open helper console buffer: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: CreateFileW returned a unique owned kernel handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn buffer_length(console: &OwnedHandle) -> Result<u32> {
    let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
    // SAFETY: the console remains owned and info is an initialized output buffer.
    ensure!(
        unsafe { GetConsoleScreenBufferInfo(console.as_raw_handle(), &mut info) } != 0,
        "Read console dimensions: {}",
        std::io::Error::last_os_error()
    );
    ensure!(
        info.dwSize.X > 0 && info.dwSize.Y > 0,
        "Invalid console dimensions"
    );
    let length = u32::from(info.dwSize.X as u16) * u32::from(info.dwSize.Y as u16);
    ensure!(
        length <= 16 * 1024 * 1024,
        "Unexpectedly large test console buffer"
    );
    Ok(length)
}

fn clear(console: &OwnedHandle) -> Result<()> {
    let length = buffer_length(console)?;
    let origin = COORD { X: 0, Y: 0 };
    let mut written = 0;
    // SAFETY: these operations affect only this helper's newly-created console.
    ensure!(
        unsafe {
            FillConsoleOutputCharacterW(
                console.as_raw_handle(),
                b' ' as u16,
                length,
                origin,
                &mut written,
            )
        } != 0,
        "Clear helper console: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: origin is within the verified console dimensions.
    ensure!(
        unsafe { SetConsoleCursorPosition(console.as_raw_handle(), origin) } != 0,
        "Position helper console cursor: {}",
        std::io::Error::last_os_error()
    );
    Ok(())
}

fn read_console(console: &OwnedHandle) -> Result<String> {
    let length = buffer_length(console)?;
    let mut text = vec![0u16; length as usize];
    let mut read = 0;
    // SAFETY: text has length writable UTF-16 units; console remains owned.
    ensure!(
        unsafe {
            ReadConsoleOutputCharacterW(
                console.as_raw_handle(),
                text.as_mut_ptr(),
                length,
                COORD { X: 0, Y: 0 },
                &mut read,
            )
        } != 0,
        "Read real console output: {}",
        std::io::Error::last_os_error()
    );
    ensure!(
        read <= length,
        "Console returned an invalid character count"
    );
    Ok(String::from_utf16_lossy(&text[..read as usize]))
}

fn cli() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xsm"));
    command.env("XSM_LANG", "en-US");
    // Keep unrelated host diagnostics configuration out of this I/O contract test.
    command.env_remove("XSM_LOG_FILE");
    command.env("XSM_LOG_LEVEL", "off");
    command
}

fn exercise_console() -> Result<()> {
    // SAFETY: the helper is the only process whose console is queried here.
    let window = unsafe { GetConsoleWindow() };
    ensure!(!window.is_null(), "Helper must own a real console");
    // SAFETY: GetConsoleWindow returned this helper's window handle.
    ensure!(
        unsafe { IsWindowVisible(window) } == 0,
        "Test console must remain hidden"
    );
    let console = console_buffer()?;
    clear(&console)?;
    let version = cli().arg("version").status()?;
    ensure!(
        version.success(),
        "Inherited-console version failed: {version}"
    );
    ensure!(
        read_console(&console)?.contains(&format!("xsm {}", env!("CARGO_PKG_VERSION"))),
        "Version text did not reach the real parent console"
    );

    clear(&console)?;
    let invalid = cli().arg("get").status()?;
    ensure!(
        invalid.code() == Some(1),
        "Malformed get must return exit code 1"
    );
    ensure!(
        read_console(&console)?.contains("Missing command arguments"),
        "Malformed get stderr did not reach the real parent console"
    );

    clear(&console)?;
    let version = cli().arg("version").output()?;
    ensure!(
        version.status.success() && version.stderr.is_empty(),
        "Piped version status/stderr changed: {version:?}"
    );
    ensure!(
        String::from_utf8(version.stdout)?.contains(&format!("xsm {}", env!("CARGO_PKG_VERSION"))),
        "Console inheritance discarded piped stdout"
    );
    let invalid = cli().arg("get").output()?;
    ensure!(
        invalid.status.code() == Some(1) && invalid.stdout.is_empty(),
        "Piped malformed get status/stdout changed: {invalid:?}"
    );
    ensure!(
        String::from_utf8(invalid.stderr)?.contains("Missing command arguments"),
        "Console inheritance discarded piped stderr"
    );
    ensure!(
        read_console(&console)?.trim().is_empty(),
        "Redirected output leaked into parent console"
    );
    // SAFETY: the borrowed console window is still owned by this helper process.
    ensure!(
        unsafe { IsWindowVisible(window) } == 0,
        "CLI made the hidden console visible"
    );

    // A GUI-only UAC child detaches itself. The same operation in this isolated
    // child must leave the host's shared console and standard streams intact.
    let detached = Command::new(std::env::current_exe()?)
        .args([
            "--ignored",
            "--exact",
            HOST_TEST,
            "--nocapture",
            "--test-threads=1",
        ])
        .env(DETACH_VARIABLE, "1")
        .output()?;
    ensure!(
        detached.status.success(),
        "Console-detach child failed: {detached:?}"
    );
    // SAFETY: this only queries the current hidden test host's console.
    ensure!(
        unsafe { GetConsoleWindow() } == window,
        "Detaching a child changed its parent's console"
    );
    clear(&console)?;
    ensure!(
        cli().arg("version").status()?.success(),
        "Parent console stopped working after child detach"
    );
    ensure!(
        read_console(&console)?.contains(&format!("xsm {}", env!("CARGO_PKG_VERSION"))),
        "Parent console no longer receives output after child detach"
    );
    Ok(())
}

#[test]
#[ignore = "Only the parent test launches this helper in a new hidden console"]
fn hidden_console_host() {
    let Some(report) = std::env::var_os(REPORT_VARIABLE) else {
        return;
    };
    if std::env::var_os(DETACH_VARIABLE).as_deref() == Some(std::ffi::OsStr::new("1")) {
        // SAFETY: the test parent explicitly supplied a hidden shared console.
        let window = unsafe { GetConsoleWindow() };
        assert!(!window.is_null());
        // SAFETY: this handle was just returned for the current test child.
        assert_eq!(unsafe { IsWindowVisible(window) }, 0);
        xsm::windows::elevation::detach_gui_console().expect("Detach only the test child");
        // SAFETY: query only the calling child's association after detaching.
        assert!(unsafe { GetConsoleWindow() }.is_null());
        return;
    }
    let result = exercise_console();
    let evidence = match &result {
        Ok(()) => "PASS\nReal hidden parent console receives stdout/stderr; piped stdout/stderr and exit codes remain intact; GUI-only child detach preserves the parent's console.\n".to_owned(),
        Err(error) => format!("FAIL\n{error:#}\n"),
    };
    fs::write(report, &evidence).expect("write console test report");
    assert!(result.is_ok(), "{evidence}");
}
