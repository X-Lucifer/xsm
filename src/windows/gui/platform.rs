//! Small Windows adapters. No GUI model pointers cross these FFI boundaries.
use crate::i18n::Language;
use anyhow::{Context, Result, anyhow, ensure};
use wgpu::rwh::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::{
    Foundation::RECT,
    System::Threading::{GetStartupInfoW, STARTF_USESHOWWINDOW, STARTUPINFOW},
    UI::{Controls::Dialogs::*, HiDpi::GetDpiForSystem, WindowsAndMessaging::*},
};
pub(super) fn startup_window() -> ([f32; 2], bool) {
    let mut startup = STARTUPINFOW::default();
    let mut work = RECT::default();
    // SAFETY: both output structs are initialized and live for each synchronous call.
    unsafe {
        GetStartupInfoW(&mut startup);
        let scale = GetDpiForSystem().max(96) as f32 / 96.0;
        let size =
            if SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut work as *mut RECT).cast(), 0) != 0 {
                [
                    ((work.right - work.left) as f32 / scale - 24.0).min(1120.0),
                    ((work.bottom - work.top) as f32 / scale - 24.0).min(820.0),
                ]
            } else {
                [1120.0, 820.0]
            };
        (
            size,
            startup.dwFlags & STARTF_USESHOWWINDOW == 0 || startup.wShowWindow != SW_HIDE as u16,
        )
    }
}
pub(super) fn icon() -> eframe::egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../../../resources/xsm.png"))
        .unwrap_or_default()
}
pub(super) fn configure_native(context: &eframe::CreationContext<'_>, hidden: bool) -> Result<()> {
    let RawWindowHandle::Win32(handle) = context
        .window_handle()
        .context("Window handle unavailable")?
        .as_raw()
    else {
        return Ok(());
    };
    let hwnd = handle.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    // SAFETY: the borrowed creation context owns this live window on this thread.
    // The subclass stores no pointers or Rust state and is removed on destruction.
    unsafe {
        use windows_sys::Win32::Graphics::Dwm::*;
        let corner = DWMWCP_ROUND;
        let border = DWMWA_COLOR_NONE;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            (&corner as *const i32).cast(),
            std::mem::size_of_val(&corner) as u32,
        );
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR as u32,
            (&border as *const u32).cast(),
            std::mem::size_of_val(&border) as u32,
        );
        if hidden {
            ensure!(
                windows_sys::Win32::UI::Shell::SetWindowSubclass(
                    hwnd,
                    Some(hidden_proc),
                    0x58534d,
                    0
                ) != 0,
                "Cannot preserve hidden startup"
            );
        }
    }
    Ok(())
}
unsafe extern "system" fn hidden_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    _data: usize,
) -> isize {
    // No allocations, model access or panicking code can cross this callback.
    unsafe {
        if message == WM_WINDOWPOSCHANGING && lparam != 0 {
            let position = &mut *(lparam as *mut WINDOWPOS);
            if position.flags & SWP_SHOWWINDOW != 0 {
                position.flags = (position.flags & !SWP_SHOWWINDOW) | SWP_HIDEWINDOW;
            }
        }
        if message == WM_NCDESTROY {
            windows_sys::Win32::UI::Shell::RemoveWindowSubclass(hwnd, Some(hidden_proc), 0x58534d);
        }
        let result = windows_sys::Win32::UI::Shell::DefSubclassProc(hwnd, message, wparam, lparam);
        if (message == WM_WINDOWPOSCHANGED || message == WM_SHOWWINDOW && wparam != 0)
            && IsWindowVisible(hwnd) != 0
        {
            ShowWindow(hwnd, SW_HIDE);
        }
        result
    }
}
pub(super) fn file_dialog(save: bool, language: Language) -> Result<Option<String>> {
    let mut buffer = vec![0u16; 32768];
    let chinese = language == Language::ZhCn;
    let filter = if chinese && save {
        "命令脚本 (*.cmd)\0*.cmd\0所有文件 (*.*)\0*.*\0\0"
    } else if chinese {
        "可执行文件 (*.exe;*.com;*.bat;*.cmd)\0*.exe;*.com;*.bat;*.cmd\0所有文件 (*.*)\0*.*\0\0"
    } else if save {
        "Command script (*.cmd)\0*.cmd\0All files (*.*)\0*.*\0\0"
    } else {
        "Executable (*.exe;*.com;*.bat;*.cmd)\0*.exe;*.com;*.bat;*.cmd\0All files (*.*)\0*.*\0\0"
    }
    .encode_utf16()
    .collect::<Vec<_>>();
    let extension = "cmd\0".encode_utf16().collect::<Vec<_>>();
    let title = match (chinese, save) {
        (true, true) => "导出服务配置",
        (true, false) => "选择应用程序",
        (false, true) => "Export service configuration",
        (false, false) => "Select application",
    }
    .encode_utf16()
    .chain(Some(0))
    .collect::<Vec<_>>();
    let mut data = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: filter.as_ptr(),
        lpstrTitle: title.as_ptr(),
        lpstrFile: buffer.as_mut_ptr(),
        nMaxFile: buffer.len() as u32,
        lpstrDefExt: if save {
            extension.as_ptr()
        } else {
            std::ptr::null()
        },
        Flags: OFN_EXPLORER
            | OFN_NOCHANGEDIR
            | OFN_PATHMUSTEXIST
            | if save {
                OFN_OVERWRITEPROMPT
            } else {
                OFN_FILEMUSTEXIST
            },
        ..Default::default()
    };
    // SAFETY: dialog owns no Rust data; all buffers remain alive until it returns.
    unsafe {
        let ok = if save {
            GetSaveFileNameW(&mut data)
        } else {
            GetOpenFileNameW(&mut data)
        };
        if ok != 0 {
            let len = buffer
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(buffer.len());
            Ok(Some(String::from_utf16_lossy(&buffer[..len])))
        } else {
            let error = CommDlgExtendedError();
            if error == 0 {
                Ok(None)
            } else {
                Err(anyhow!(
                    "{}: {error:#x}",
                    if chinese {
                        "文件对话框失败"
                    } else {
                        "File dialog failed"
                    }
                ))
            }
        }
    }
}

pub(super) fn directory_dialog(initial: &str, language: Language) -> Result<Option<String>> {
    directory_picker::show(initial, language)
}

mod directory_picker {
    use super::*;
    use ::windows::{
        Win32::{
            Foundation::ERROR_CANCELLED,
            System::Com::{
                CLSCTX_INPROC_SERVER, COINIT, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
                CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
            },
            UI::Shell::{
                FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS,
                FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName,
                SIGDN_FILESYSPATH,
            },
        },
        core::{HRESULT, PCWSTR, PWSTR},
    };
    use std::{marker::PhantomData, path::Path, rc::Rc};

    const CANCELLED: HRESULT = HRESULT::from_win32(ERROR_CANCELLED.0);

    fn accepted(result: ::windows::core::Result<()>) -> Result<bool> {
        match result {
            Ok(()) => Ok(true),
            Err(error) if error.code() == CANCELLED => Ok(false),
            Err(error) => Err(error).context("Show directory dialog"),
        }
    }

    struct Apartment(PhantomData<Rc<()>>);

    impl Apartment {
        fn initialize(flags: COINIT) -> Result<Self> {
            // SAFETY: no reserved argument; initialization is thread-local. Both
            // S_OK and S_FALSE need one matching CoUninitialize. Failure (including
            // RPC_E_CHANGED_MODE) creates no guard and must not be uninitialized.
            unsafe { CoInitializeEx(None, flags) }
                .ok()
                .context("Initialize directory dialog COM apartment")?;
            Ok(Self(PhantomData))
        }
    }

    impl Drop for Apartment {
        fn drop(&mut self) {
            // SAFETY: this !Send/!Sync guard balances one successful call on the
            // same thread, after all dialog/ShellItem interfaces have dropped.
            unsafe { CoUninitialize() };
        }
    }

    struct TaskString(PWSTR);

    impl Drop for TaskString {
        fn drop(&mut self) {
            // SAFETY: GetDisplayName transfers a CoTaskMem allocation. A null
            // pointer is also valid for CoTaskMemFree.
            unsafe { CoTaskMemFree(Some(self.0.as_ptr().cast())) };
        }
    }

    fn create_dialog() -> Result<IFileOpenDialog> {
        // SAFETY: caller owns a live STA guard. The generated interface owns its
        // COM reference and performs Release automatically on every exit path.
        unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }
            .context("Create directory dialog")
    }

    fn shell_item(path: &str) -> Result<IShellItem> {
        let path = crate::windows::process::wide(path)?;
        // SAFETY: path is NUL-terminated and remains alive throughout the call;
        // the generated binding requests and owns an IShellItem reference.
        unsafe { SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None) }
            .context("Read initial directory")
    }

    fn filesystem_path(item: &IShellItem) -> Result<String> {
        // SAFETY: item is a live interface in the current apartment. On success
        // the shell returns a NUL-terminated, CoTaskMem-allocated UTF-16 string.
        let path = TaskString(
            unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }
                .context("Read selected directory path")?,
        );
        ensure!(!path.0.is_null(), "COM returned a null directory path");
        // SAFETY: path owns that allocation until after decoding, including the
        // error path. PWSTR::to_string reads only through its terminating NUL.
        unsafe { path.0.to_string() }.context("Directory path is not valid Unicode")
    }

    fn initial_directory(initial: &str) -> Option<String> {
        let expanded = crate::windows::registry::expand(initial).ok()?;
        Path::new(&expanded).is_dir().then_some(expanded)
    }

    pub(super) fn show(initial: &str, language: Language) -> Result<Option<String>> {
        // Declared first so all generated COM interfaces drop before COM does.
        let _apartment = Apartment::initialize(COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE)?;
        let dialog = create_dialog()?;
        let title = crate::windows::process::wide(if language == Language::ZhCn {
            "选择工作目录"
        } else {
            "Select working directory"
        })?;
        // SAFETY: the dialog is live in this STA, all input strings/interfaces
        // outlive the synchronous calls, and generated bindings own each output.
        unsafe {
            let options = dialog
                .GetOptions()
                .context("Read directory dialog options")?;
            dialog
                .SetOptions(
                    options
                        | FOS_PICKFOLDERS
                        | FOS_FORCEFILESYSTEM
                        | FOS_PATHMUSTEXIST
                        | FOS_NOCHANGEDIR,
                )
                .context("Set directory dialog options")?;
            dialog
                .SetTitle(PCWSTR(title.as_ptr()))
                .context("Set directory dialog title")?;
            if let Some(initial) = initial_directory(initial)
                && let Ok(folder) = shell_item(&initial)
            {
                // Initial directory is only a hint. Deletion/access changes
                // between validation and SetFolder fall back to the default.
                let _ = dialog.SetFolder(&folder);
            }
            if !accepted(dialog.Show(None))? {
                return Ok(None);
            }
            let item = dialog
                .GetResult()
                .context("Read directory dialog selection")?;
            filesystem_path(&item).map(Some)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use ::windows::Win32::{
            Foundation::{E_ACCESSDENIED, RPC_E_CHANGED_MODE},
            System::Com::COINIT_MULTITHREADED,
        };

        #[test]
        fn cancellation_is_distinct_from_success_and_failure() {
            assert!(accepted(HRESULT(0).ok()).unwrap());
            assert!(accepted(HRESULT(1).ok()).unwrap());
            assert!(!accepted(CANCELLED.ok()).unwrap());
            let error = accepted(E_ACCESSDENIED.ok()).unwrap_err();
            assert_eq!(
                error
                    .downcast_ref::<::windows::core::Error>()
                    .unwrap()
                    .code(),
                E_ACCESSDENIED
            );
        }

        #[test]
        fn nested_sta_initialization_balances_every_success() {
            std::thread::spawn(|| {
                let outer = Apartment::initialize(COINIT_APARTMENTTHREADED).unwrap();
                let nested = Apartment::initialize(COINIT_APARTMENTTHREADED).unwrap();
                drop(nested);
                assert!(Apartment::initialize(COINIT_MULTITHREADED).is_err());
                drop(outer);
                // A remaining nested initialization would prevent changing to MTA.
                let _mta = Apartment::initialize(COINIT_MULTITHREADED).unwrap();
            })
            .join()
            .unwrap();
        }

        #[test]
        fn incompatible_apartment_is_not_uninitialized_on_failure() {
            std::thread::spawn(|| {
                let mta = Apartment::initialize(COINIT_MULTITHREADED).unwrap();
                for _ in 0..2 {
                    let error = Apartment::initialize(COINIT_APARTMENTTHREADED)
                        .err()
                        .expect("MTA must reject STA initialization");
                    assert_eq!(
                        error
                            .downcast_ref::<::windows::core::Error>()
                            .unwrap()
                            .code(),
                        RPC_E_CHANGED_MODE
                    );
                }
                drop(mta);
                let _sta = Apartment::initialize(COINIT_APARTMENTTHREADED).unwrap();
            })
            .join()
            .unwrap();
        }

        #[test]
        fn invalid_initial_directory_falls_back_to_default() {
            assert!(initial_directory("").is_none());
            assert!(initial_directory("invalid\0directory").is_none());
            assert!(initial_directory("%XSM_UNDEFINED_DIRECTORY_71B6922B%\\missing").is_none());
            assert!(initial_directory("%SystemRoot%").is_some());
        }

        #[test]
        fn shell_item_preserves_unicode_directory_without_opening_a_dialog() {
            std::thread::spawn(|| {
                let path = std::env::temp_dir()
                    .join(format!("xsm-directory-picker-中文-{}", std::process::id()));
                std::fs::create_dir(&path).unwrap();
                let result = (|| -> Result<()> {
                    let _sta = Apartment::initialize(COINIT_APARTMENTTHREADED)?;
                    let original = path.to_str().context("Test path is not Unicode")?;
                    assert_eq!(initial_directory(original).as_deref(), Some(original));
                    let item = shell_item(original)?;
                    assert_eq!(filesystem_path(&item)?, original);
                    let dialog = create_dialog()?;
                    // SAFETY: both interfaces are live in this STA. Setting the
                    // folder exercises the generated API without calling Show.
                    unsafe { dialog.SetFolder(&item) }.context("Set test directory")?;
                    Ok(())
                })();
                std::fs::remove_dir(&path).unwrap();
                result.unwrap();
            })
            .join()
            .unwrap();
        }
    }
}
