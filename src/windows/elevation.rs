//! Optional UAC relaunch, separate from read-only service access.
use super::process::Handle;
use anyhow::{Context, Result, bail, ensure};
use std::{
    mem::size_of,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::*,
    System::{Console::FreeConsole, Threading::*},
    UI::{
        Shell::*,
        WindowsAndMessaging::{
            MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW, SW_HIDE, SW_SHOWNORMAL,
        },
    },
};

pub fn is_elevated() -> bool {
    let mut token = null_mut();
    // SAFETY: output token is initialized by OpenProcessToken and owned below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return false;
    }
    // SAFETY: successful OpenProcessToken returned a unique owned handle.
    let Ok(token) = (unsafe { Handle::from_raw_owned(token) }) else {
        return false;
    };
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0;
    unsafe {
        GetTokenInformation(
            token.raw(),
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        ) != 0
            && elevation.TokenIsElevated != 0
    }
}
fn start(args: &[String], show: i32) -> Result<Handle> {
    super::diagnostics::release_file()?;
    let path = std::env::current_exe().context("Resolve executable for UAC")?;
    let path = super::process::wide(&path.to_string_lossy())?;
    let verb = super::process::wide("runas")?;
    let parameters = super::process::wide(
        &args
            .iter()
            .map(|a| crate::quoting::quote_arg(a))
            .collect::<Vec<_>>()
            .join(" "),
    )?;
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: verb.as_ptr(),
        lpFile: path.as_ptr(),
        lpParameters: parameters.as_ptr(),
        lpDirectory: null(),
        nShow: show,
        ..Default::default()
    };
    // SAFETY: UTF-16 parameters remain alive through the synchronous shell call.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(std::io::Error::last_os_error())
            .context("UAC relaunch failed or was cancelled");
    }
    // SAFETY: SEE_MASK_NOCLOSEPROCESS transfers the returned process handle.
    unsafe { Handle::from_raw_owned(info.hProcess) }
}
/// Return after Windows has launched the new UI; it owns its own message loop.
pub fn launch(args: &[String]) -> Result<()> {
    let arguments = gui_arguments(args)?;
    drop(start(&arguments, SW_HIDE)?);
    Ok(())
}

fn gui_arguments(args: &[String]) -> Result<[String; 1]> {
    ensure!(
        args.len() == 1 && args[0].eq_ignore_ascii_case("install"),
        "The graphical elevation launcher only accepts install without arguments"
    );
    Ok([crate::cli::ELEVATED_GUI_FLAG.into()])
}

/// Used only by the explicit elevated-GUI entry, before its window is created.
/// Detaching this process leaves any other process attached to a shared console.
pub fn detach_gui_console() -> Result<()> {
    // SAFETY: FreeConsole has no pointer arguments and only detaches the calling
    // process. No other window is hidden, closed, or otherwise manipulated.
    if unsafe { FreeConsole() } == 0 {
        return Err(std::io::Error::last_os_error()).context("Detach the GUI startup console");
    }
    Ok(())
}

/// The explicit GUI launch has no console after detaching. Report its startup
/// error without reading or including command-line arguments or credentials.
pub fn show_gui_startup_error(error: &anyhow::Error) {
    let title = dialog_wide(crate::i18n::text("XSM 启动失败", "XSM startup failed"));
    let message = dialog_wide(&format!(
        "{}\n\n{error:#}",
        crate::i18n::text(
            "无法打开服务管理界面。",
            "The service management window could not be opened."
        )
    ));
    // SAFETY: both buffers contain valid UTF-16 with one terminating NUL and
    // remain alive throughout this synchronous call. No parent window is used.
    unsafe {
        MessageBoxW(
            null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

fn dialog_wide(value: &str) -> Vec<u16> {
    value
        .encode_utf16()
        .map(|unit| if unit == 0 { 0xfffd } else { unit })
        .chain([0])
        .collect()
}
/// CLI installs/removals propagate the elevated command's actual exit status.
pub fn relaunch(args: &[String]) -> Result<i32> {
    let process = start(args, SW_SHOWNORMAL)?;
    if unsafe { WaitForSingleObject(process.raw(), INFINITE) } != WAIT_OBJECT_0 {
        bail!(
            "Could not wait for elevated command: {}",
            std::io::Error::last_os_error()
        );
    }
    let mut code = 0;
    if unsafe { GetExitCodeProcess(process.raw(), &mut code) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(code as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphical_elevation_never_rewrites_other_commands() {
        let args = gui_arguments(&["install".into()]).unwrap();
        assert_eq!(
            crate::cli::parse(&args).unwrap(),
            crate::cli::Command::ElevatedGui
        );
        for args in [
            vec![],
            vec!["install".into(), "service".into(), "app.exe".into()],
            vec!["install".into(), "extra".into()],
            vec!["manage".into()],
        ] {
            assert!(gui_arguments(&args).is_err());
        }
    }

    #[test]
    fn graphical_error_text_preserves_unicode_and_cannot_be_truncated_by_nul() {
        let text = dialog_wide("启动失败🔧\0GPU error");
        assert_eq!(text.last(), Some(&0));
        assert!(!text[..text.len() - 1].contains(&0));
        assert_eq!(
            String::from_utf16(&text[..text.len() - 1]).unwrap(),
            "启动失败🔧\u{fffd}GPU error"
        );
    }
}
