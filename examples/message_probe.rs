//! Session-zero shutdown probe. All windows are created without WS_VISIBLE.
#[cfg(windows)]
mod windows {
    use std::{
        cell::Cell,
        fs::OpenOptions,
        io::{self, Write},
        panic::{AssertUnwindSafe, catch_unwind},
        path::{Path, PathBuf},
        ptr::null_mut,
        time::Duration,
    };
    use windows_sys::Win32::{
        Foundation::{FILETIME, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        System::{
            Console::SetConsoleCtrlHandler,
            LibraryLoader::GetModuleHandleW,
            RemoteDesktop::ProcessIdToSessionId,
            Threading::{GetCurrentProcess, GetCurrentThreadId, GetProcessTimes},
        },
        UI::WindowsAndMessaging::*,
    };

    fn record(path: &Path, message: &str) -> io::Result<()> {
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(file, "{message}")?;
        file.flush()
    }

    fn ready(path: &Path) -> io::Result<()> {
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let mut session = 0;
        if unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        } == 0
            || unsafe { ProcessIdToSessionId(std::process::id(), &mut session) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let birth = (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
        record(
            path,
            &format!(
                "READY pid={} created={birth} session={session} thread={}",
                std::process::id(),
                unsafe { GetCurrentThreadId() }
            ),
        )
    }

    unsafe extern "system" fn ignore_console(_control: u32) -> i32 {
        // Never unwind through a Windows callback, even if this probe is extended.
        catch_unwind(|| 1).unwrap_or(1)
    }

    struct State {
        marker: PathBuf,
        ignore_close: bool,
        callback_failed: Cell<bool>,
    }
    impl State {
        fn record(&self, value: &str) {
            if record(&self.marker, value).is_err() {
                self.callback_failed.set(true);
            }
        }
    }

    unsafe extern "system" fn window_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        catch_unwind(AssertUnwindSafe(|| unsafe {
            if message == WM_NCCREATE {
                let create = &*(lparam as *const CREATESTRUCTW);
                SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize);
            }
            let state = GetWindowLongPtrW(window, GWLP_USERDATA) as *const State;
            if let Some(state) = state.as_ref() {
                match message {
                    WM_CLOSE => {
                        state.record("WINDOW_CLOSE");
                        if !state.ignore_close {
                            DestroyWindow(window);
                        }
                        return 0;
                    }
                    WM_ENDSESSION => {
                        state.record("WINDOW_ENDSESSION");
                        return 0;
                    }
                    WM_DESTROY => {
                        PostQuitMessage(0);
                        return 0;
                    }
                    WM_NCDESTROY => {
                        SetWindowLongPtrW(window, GWLP_USERDATA, 0);
                    }
                    _ => {}
                }
            }
            DefWindowProcW(window, message, wparam, lparam)
        }))
        .unwrap_or(0)
    }

    struct Window(HWND);
    impl Drop for Window {
        fn drop(&mut self) {
            unsafe {
                if IsWindow(self.0) != 0 {
                    DestroyWindow(self.0);
                }
            }
        }
    }
    struct Class {
        name: Vec<u16>,
        instance: HINSTANCE,
    }
    impl Drop for Class {
        fn drop(&mut self) {
            unsafe { UnregisterClassW(self.name.as_ptr(), self.instance) };
        }
    }

    pub fn run() -> io::Result<()> {
        let args: Vec<_> = std::env::args_os().collect();
        let mode = args.get(1).and_then(|value| value.to_str()).unwrap_or("");
        if !matches!(mode, "window" | "window-ignore" | "thread" | "resistant") {
            return Err(io::Error::other(
                "mode must be window, window-ignore, thread or resistant",
            ));
        }
        let marker = PathBuf::from(
            args.get(2)
                .ok_or_else(|| io::Error::other("marker path missing"))?,
        );
        if unsafe { SetConsoleCtrlHandler(Some(ignore_console), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if mode == "resistant" {
            ready(&marker)?;
            loop {
                std::thread::sleep(Duration::from_secs(60));
            }
        }
        let mut message = MSG::default();
        // Explicitly create the main thread's message queue before publishing READY.
        unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, PM_NOREMOVE) };
        if mode == "thread" {
            ready(&marker)?;
            loop {
                match unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } {
                    -1 => return Err(io::Error::last_os_error()),
                    0 => {
                        record(&marker, "THREAD_QUIT")?;
                        return record(&marker, "EXIT");
                    }
                    _ => {}
                }
            }
        }
        let state = Box::new(State {
            marker,
            ignore_close: mode == "window-ignore",
            callback_failed: Cell::new(false),
        });
        let class = Class {
            name: format!("Xsm_Message_Probe_{}\0", std::process::id())
                .encode_utf16()
                .collect(),
            instance: unsafe { GetModuleHandleW(null_mut()) },
        };
        let definition = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: class.instance,
            lpszClassName: class.name.as_ptr(),
            ..Default::default()
        };
        if unsafe { RegisterClassW(&definition) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let window = Window(unsafe {
            CreateWindowExW(
                0,
                class.name.as_ptr(),
                class.name.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                1,
                1,
                null_mut(),
                null_mut(),
                class.instance,
                (&*state as *const State).cast_mut().cast(),
            )
        });
        if window.0.is_null() {
            return Err(io::Error::last_os_error());
        }
        if unsafe { IsWindowVisible(window.0) } != 0 {
            return Err(io::Error::other("probe window must remain hidden"));
        }
        record(&state.marker, "HIDDEN_WINDOW")?;
        ready(&state.marker)?;
        loop {
            match unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } {
                -1 => return Err(io::Error::last_os_error()),
                0 => {
                    // A still-live window proves WM_QUIT came from the thread stage.
                    if unsafe { IsWindow(window.0) } != 0 {
                        record(&state.marker, "THREAD_QUIT")?;
                    }
                    break;
                }
                _ => unsafe {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                },
            }
        }
        if state.callback_failed.get() {
            return Err(io::Error::other(
                "window callback could not record its marker",
            ));
        }
        record(&state.marker, "EXIT")
    }
}

#[cfg(windows)]
fn main() -> std::io::Result<()> {
    windows::run()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("message_probe requires Windows");
}
