//! Process ownership and bounded, cooperative shutdown. All raw handles are owned.
use anyhow::{Context, Result, bail};
use std::{
    collections::{BTreeMap, HashSet},
    ffi::c_void,
    mem::size_of,
    os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle},
    ptr::{null, null_mut},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};
use windows_sys::Win32::{
    Foundation::*,
    System::{Console::*, Diagnostics::ToolHelp::*, Threading::*},
    UI::WindowsAndMessaging::*,
};

pub struct Handle(OwnedHandle);
impl Handle {
    /// Take the unique ownership returned by a successful Windows handle factory.
    ///
    /// # Safety
    /// A non-null, non-invalid value must be a real handle that can be closed
    /// with CloseHandle, exclusively owned by this call. Borrowed/pseudo handles
    /// and handles requiring a different close API must never be passed here.
    pub unsafe fn from_raw_owned(raw: HANDLE) -> Result<Self> {
        if raw.is_null() || raw == INVALID_HANDLE_VALUE {
            Err(std::io::Error::last_os_error().into())
        } else {
            // SAFETY: the caller transfers a unique real handle, validated above.
            Ok(Self(unsafe { OwnedHandle::from_raw_handle(raw) }))
        }
    }
    pub fn raw(&self) -> HANDLE {
        self.0.as_raw_handle()
    }
    pub fn borrow(&self) -> BorrowedHandle<'_> {
        self.0.as_handle()
    }
    pub fn into_file(self) -> std::fs::File {
        self.0.into()
    }
    pub fn from_file(file: std::fs::File) -> Self {
        Self(file.into())
    }
    pub fn set_inherit(&self, inherit: bool) -> Result<()> {
        // SAFETY: the owned handle remains alive for the complete call.
        if unsafe {
            SetHandleInformation(
                self.raw(),
                HANDLE_FLAG_INHERIT,
                if inherit { HANDLE_FLAG_INHERIT } else { 0 },
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
    pub fn duplicate(&self, inherit: bool) -> Result<Self> {
        let cloned = Self(self.0.try_clone()?);
        cloned.set_inherit(inherit)?;
        Ok(cloned)
    }
}
pub fn wide(value: &str) -> Result<Vec<u16>> {
    if value.contains('\0') {
        bail!("NUL in Windows string");
    }
    Ok(value.encode_utf16().chain(Some(0)).collect())
}
pub type Environment = BTreeMap<String, (String, String)>;
pub fn inherited_environment() -> Environment {
    std::env::vars()
        .map(|(k, v)| (k.to_uppercase(), (k, v)))
        .collect()
}
pub fn expand(text: &str, env: &Environment) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        if let Some(end) = rest[1..].find('%') {
            let end = end + 1;
            let key = &rest[1..end];
            if let Some((_, value)) = env.get(&key.to_uppercase()) {
                out.push_str(value);
            } else {
                out.push_str(&rest[..=end]);
            }
            rest = &rest[end + 1..];
        } else {
            out.push_str(rest);
            return out;
        }
    }
    out.push_str(rest);
    out
}
pub fn overlay_environment(env: &mut Environment, entries: &[String]) -> Result<()> {
    for entry in entries {
        let (name, value) = entry
            .split_once('=')
            .context("Environment entry must contain =")?;
        if name.is_empty() || entry.contains('\0') {
            bail!("Invalid environment entry");
        }
        let value = expand(value, env);
        env.insert(name.to_uppercase(), (name.into(), value));
    }
    Ok(())
}
pub fn environment_block(env: &Environment) -> Result<Vec<u16>> {
    let mut block = Vec::new();
    for (k, v) in env.values() {
        if k.contains(['\0', '=']) || v.contains('\0') {
            bail!("Invalid environment variable");
        }
        block.extend(format!("{k}={v}").encode_utf16());
        block.push(0);
    }
    block.push(0);
    if block.len() == 1 {
        block.push(0);
    }
    Ok(block)
}

pub struct Child {
    pub process: Handle,
    pub pid: u32,
    pub created: u64,
    pub started: Instant,
    cleanup: AtomicBool,
    cleanup_tree: AtomicBool,
    pub affinity_warning: Option<String>,
}
impl Drop for Child {
    fn drop(&mut self) {
        if !self.cleanup.load(Ordering::Relaxed) {
            return;
        }
        // A failed hook-monitor launch can drop us after its parent already
        // exited. Retain the parent's lifetime bounds when finding its children.
        if self.cleanup_tree.load(Ordering::Relaxed)
            && let Ok((created, exited)) = process_times(&self.process)
            && created == self.created
            && let Ok(children) = descendants(self.pid, created, exited)
        {
            for (_, handle) in children {
                unsafe {
                    TerminateProcess(handle.raw(), 1);
                }
            }
        }
        if unsafe { WaitForSingleObject(self.process.raw(), 0) } == WAIT_TIMEOUT {
            unsafe {
                TerminateProcess(self.process.raw(), 1);
            }
        }
    }
}
pub struct Spawn<'a> {
    pub application: Option<&'a str>,
    pub command: &'a str,
    pub directory: &'a str,
    pub environment: &'a Environment,
    pub stdio: Option<[BorrowedHandle<'a>; 3]>,
    pub flags: u32,
    pub affinity: usize,
}
pub fn spawn(spec: Spawn<'_>) -> Result<Child> {
    let app = spec.application.map(wide).transpose()?;
    let mut command = wide(spec.command)?;
    if command.len() > 32767 {
        bail!("Command line exceeds Windows limit");
    }
    let directory = wide(spec.directory)?;
    let environment = environment_block(spec.environment)?;
    let mut info: STARTUPINFOEXW = Default::default();
    info.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    // Restrict inheritance to explicitly selected streams even when hooks launch concurrently.
    let mut bytes = 0usize;
    let mut attributes: Vec<usize> = Vec::new();
    let mut handles = spec.stdio.map(|streams| streams.map(|h| h.as_raw_handle()));
    if let Some(ref mut streams) = handles {
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes);
        }
        attributes.resize(bytes.div_ceil(size_of::<usize>()), 0);
        info.lpAttributeList = attributes.as_mut_ptr().cast();
        if unsafe { InitializeProcThreadAttributeList(info.lpAttributeList, 1, 0, &mut bytes) } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        if unsafe {
            UpdateProcThreadAttribute(
                info.lpAttributeList,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                streams.as_mut_ptr().cast(),
                size_of::<[HANDLE; 3]>(),
                null_mut(),
                null(),
            )
        } == 0
        {
            unsafe { DeleteProcThreadAttributeList(info.lpAttributeList) };
            return Err(std::io::Error::last_os_error().into());
        }
        info.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        info.StartupInfo.hStdInput = streams[0];
        info.StartupInfo.hStdOutput = streams[1];
        info.StartupInfo.hStdError = streams[2];
    }
    let mut pi: PROCESS_INFORMATION = Default::default();
    let ok = unsafe {
        CreateProcessW(
            app.as_ref().map_or(null(), |v| v.as_ptr()),
            command.as_mut_ptr(),
            null(),
            null(),
            handles.is_some() as i32,
            spec.flags
                | CREATE_UNICODE_ENVIRONMENT
                | CREATE_SUSPENDED
                | EXTENDED_STARTUPINFO_PRESENT,
            environment.as_ptr().cast::<c_void>(),
            if spec.directory.is_empty() {
                null()
            } else {
                directory.as_ptr()
            },
            &info.StartupInfo,
            &mut pi,
        )
    };
    let error = std::io::Error::last_os_error();
    if !info.lpAttributeList.is_null() {
        unsafe { DeleteProcThreadAttributeList(info.lpAttributeList) };
    }
    if ok == 0 {
        return Err(error).context("CreateProcessW");
    }
    // SAFETY: successful CreateProcessW transfers both unique handles to us.
    let process = unsafe { Handle::from_raw_owned(pi.hProcess) }?;
    let thread = unsafe { Handle::from_raw_owned(pi.hThread) }?;
    let mut affinity_warning = None;
    if spec.affinity != 0 {
        let mut current = 0;
        let mut system = 0;
        unsafe {
            GetProcessAffinityMask(process.raw(), &mut current, &mut system);
        }
        let mask = if system == 0 {
            spec.affinity
        } else {
            spec.affinity & system
        };
        if mask == 0 || unsafe { SetProcessAffinityMask(process.raw(), mask) } == 0 {
            affinity_warning =
                Some("Configured CPU affinity is unavailable; retaining the system default".into());
        }
    }
    let created = match process_times(&process) {
        Ok(times) => times.0,
        Err(error) => {
            unsafe { TerminateProcess(process.raw(), 1) };
            return Err(error);
        }
    };
    if unsafe { ResumeThread(thread.raw()) } == u32::MAX {
        unsafe { TerminateProcess(process.raw(), 1) };
        bail!("ResumeThread: {}", std::io::Error::last_os_error());
    }
    Ok(Child {
        process,
        pid: pi.dwProcessId,
        created,
        started: Instant::now(),
        cleanup: AtomicBool::new(true),
        cleanup_tree: AtomicBool::new(true),
        affinity_warning,
    })
}
impl Child {
    pub fn set_cleanup_policy(&self, policy: StopPolicy) {
        self.cleanup_tree.store(policy.tree, Ordering::Relaxed);
        self.cleanup.store(policy.skip & 8 == 0, Ordering::Relaxed);
    }
    pub fn exit_code(&self) -> Result<Option<u32>> {
        let wait = unsafe { WaitForSingleObject(self.process.raw(), 0) };
        if wait == WAIT_TIMEOUT {
            return Ok(None);
        }
        if wait != WAIT_OBJECT_0 {
            bail!(
                "WaitForSingleObject failed: {}",
                std::io::Error::last_os_error()
            );
        }
        let mut code = 0;
        if unsafe { GetExitCodeProcess(self.process.raw(), &mut code) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Some(code))
    }
    pub fn wait(&self, millis: u32) -> bool {
        unsafe { WaitForSingleObject(self.process.raw(), millis) == WAIT_OBJECT_0 }
    }
}
fn time(ft: FILETIME) -> u64 {
    (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
}
pub fn process_times(handle: &Handle) -> Result<(u64, u64)> {
    let mut creation = Default::default();
    let mut exit = Default::default();
    let mut kernel = Default::default();
    let mut user = Default::default();
    if unsafe {
        GetProcessTimes(
            handle.raw(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok((time(creation), time(exit)))
}
#[derive(Clone, Copy)]
pub struct StopPolicy {
    pub skip: u32,
    pub console: u32,
    pub window: u32,
    pub threads: u32,
    pub tree: bool,
}
impl Default for StopPolicy {
    fn default() -> Self {
        Self {
            skip: 0,
            console: 1500,
            window: 1500,
            threads: 1500,
            tree: true,
        }
    }
}
struct WindowSignal {
    pid: u32,
    signalled: bool,
}
static CONSOLE_CONTROL_DELIVERED: AtomicBool = AtomicBool::new(false);
unsafe extern "system" fn acknowledge_console_control(control: u32) -> i32 {
    if control == CTRL_C_EVENT {
        CONSOLE_CONTROL_DELIVERED.store(true, Ordering::Release);
        1
    } else {
        0
    }
}
unsafe extern "system" fn close_window(hwnd: HWND, data: LPARAM) -> i32 {
    let signal = unsafe { &mut *(data as *mut WindowSignal) };
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == signal.pid {
        unsafe {
            signal.signalled |= PostMessageW(hwnd, WM_CLOSE, 0, 0) != 0;
            signal.signalled |= PostMessageW(
                hwnd,
                WM_ENDSESSION,
                1,
                (ENDSESSION_CLOSEAPP | ENDSESSION_CRITICAL | ENDSESSION_LOGOFF) as isize,
            ) != 0;
        }
    }
    1
}
fn stop_one(
    handle: HANDLE,
    pid: u32,
    policy: StopPolicy,
    progress: &mut impl FnMut(),
) -> Result<()> {
    let wait = |ms: u32, progress: &mut dyn FnMut()| {
        let start = Instant::now();
        loop {
            progress();
            let left = u128::from(ms).saturating_sub(start.elapsed().as_millis());
            if unsafe { WaitForSingleObject(handle, left.min(500) as u32) } == WAIT_OBJECT_0 {
                return true;
            }
            if left <= 500 {
                return false;
            }
        }
    };
    if wait(0, progress) {
        return Ok(());
    }
    if policy.skip & 1 == 0 {
        // Console attachment and handlers are process-global, including for hook
        // cleanup threads. A zero grace period still must wait for our own
        // asynchronous Ctrl-C delivery before detaching from the child's console.
        static CONSOLE: Mutex<()> = Mutex::new(());
        let _guard = CONSOLE.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            if AttachConsole(pid) != 0 {
                CONSOLE_CONTROL_DELIVERED.store(false, Ordering::Release);
                let unignored = SetConsoleCtrlHandler(None, 0) != 0;
                let protected =
                    unignored && SetConsoleCtrlHandler(Some(acknowledge_console_control), 1) != 0;
                let signalled = protected && GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0) != 0;
                if signalled {
                    let delivered = Instant::now();
                    while !CONSOLE_CONTROL_DELIVERED.load(Ordering::Acquire)
                        && delivered.elapsed() < std::time::Duration::from_secs(2)
                    {
                        progress();
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                }
                let exited = signalled && wait(policy.console, progress);
                FreeConsole();
                if exited {
                    return Ok(());
                }
            }
        }
    }
    if policy.skip & 2 == 0 {
        let mut signal = WindowSignal {
            pid,
            signalled: false,
        };
        unsafe { EnumWindows(Some(close_window), &mut signal as *mut _ as isize) };
        if signal.signalled && wait(policy.window, progress) {
            return Ok(());
        }
    }
    if policy.skip & 4 == 0 {
        let mut signalled = false;
        // SAFETY: the snapshot factory returns a new uniquely owned handle.
        let snapshot =
            unsafe { Handle::from_raw_owned(CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)) }?;
        let mut entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut ok = unsafe { Thread32First(snapshot.raw(), &mut entry) };
        while ok != 0 {
            if entry.th32OwnerProcessID == pid {
                signalled |= unsafe { PostThreadMessageW(entry.th32ThreadID, WM_QUIT, 0, 0) } != 0
            }
            ok = unsafe { Thread32Next(snapshot.raw(), &mut entry) }
        }
        if signalled && wait(policy.threads, progress) {
            return Ok(());
        }
    }
    if policy.skip & 8 == 0 {
        if unsafe { TerminateProcess(handle, 0) } == 0 && !wait(0, progress) {
            return Err(std::io::Error::last_os_error().into());
        }
        wait(5000, progress);
    }
    Ok(())
}
pub fn descendants(pid: u32, created: u64, exited: u64) -> Result<Vec<(u32, Handle)>> {
    // SAFETY: the snapshot factory returns a new uniquely owned handle.
    let snapshot =
        unsafe { Handle::from_raw_owned(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)) }?;
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut entries = Vec::new();
    let mut ok = unsafe { Process32FirstW(snapshot.raw(), &mut entry) };
    while ok != 0 {
        entries.push((entry.th32ParentProcessID, entry.th32ProcessID));
        ok = unsafe { Process32NextW(snapshot.raw(), &mut entry) }
    }
    let mut parents = vec![(pid, created, exited)];
    let mut seen = HashSet::from([pid]);
    let mut result = Vec::new();
    while let Some((parent, start, end)) = parents.pop() {
        for &(ppid, id) in &entries {
            if ppid != parent || seen.contains(&id) {
                continue;
            }
            // SAFETY: OpenProcess returns a fresh unique reference to the process.
            let Ok(handle) = (unsafe {
                Handle::from_raw_owned(OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE,
                    0,
                    id,
                ))
            }) else {
                continue;
            };
            let Ok((birth, death)) = process_times(&handle) else {
                continue;
            };
            if birth < start || (end != 0 && birth > end) {
                continue;
            }
            seen.insert(id);
            parents.push((id, birth, death));
            result.push((id, handle));
        }
    }
    Ok(result)
}
pub fn stop(child: &Child, policy: StopPolicy, mut progress: impl FnMut()) -> Result<()> {
    child.set_cleanup_policy(policy);
    let mut queue = std::collections::VecDeque::from([(
        child.pid,
        child.process.duplicate(false)?,
        child.created,
    )]);
    let mut seen = HashSet::from([child.pid]);
    let mut error = None;
    while let Some((pid, handle, created)) = queue.pop_front() {
        // Snapshot both before and after each parent's graceful shutdown: an
        // application can spawn children from its own exit handler.
        for after in [false, true] {
            if after && let Err(e) = stop_one(handle.raw(), pid, policy, &mut progress) {
                error.get_or_insert(e);
            }
            if policy.tree {
                let discovered =
                    process_times(&handle).and_then(|(_, exit)| descendants(pid, created, exit));
                match discovered {
                    Ok(children) => {
                        for (id, owned) in children {
                            if seen.insert(id) {
                                match process_times(&owned) {
                                    Ok((birth, _)) => queue.push_back((id, owned, birth)),
                                    Err(e) => {
                                        error.get_or_insert(e);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error.get_or_insert(e);
                    }
                }
            }
        }
    }
    if let Some(e) = error {
        Err(e)
    } else {
        // The configured skip mask can deliberately leave an application alive.
        child.cleanup.store(false, Ordering::Relaxed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owned_handles_release_only_after_last_duplicate_and_round_trip_to_file() {
        use std::{
            fs::OpenOptions,
            io::Write,
            os::windows::fs::OpenOptionsExt,
            path::PathBuf,
            time::{SystemTime, UNIX_EPOCH},
        };
        struct TemporaryFile(PathBuf);
        impl Drop for TemporaryFile {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let path = TemporaryFile(std::env::temp_dir().join(format!(
            "xsm-owned-handle-{}-{}.txt",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        )));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .share_mode(0)
            .open(&path.0)
            .unwrap();
        let original = Handle::from_file(file);
        original.set_inherit(false).unwrap();
        let duplicate = original.duplicate(true).unwrap();
        for (handle, expected) in [(&original, 0), (&duplicate, HANDLE_FLAG_INHERIT)] {
            let mut flags = 0;
            // SAFETY: both owned handles remain live for this output-only query.
            assert_ne!(unsafe { GetHandleInformation(handle.raw(), &mut flags) }, 0);
            assert_eq!(flags & HANDLE_FLAG_INHERIT, expected);
        }
        let reopen = || OpenOptions::new().write(true).share_mode(0).open(&path.0);
        assert_eq!(
            reopen().unwrap_err().raw_os_error(),
            Some(ERROR_SHARING_VIOLATION as i32)
        );
        drop(original);
        assert_eq!(
            reopen().unwrap_err().raw_os_error(),
            Some(ERROR_SHARING_VIOLATION as i32)
        );
        let mut file = duplicate.into_file();
        file.write_all("句柄所有权".as_bytes()).unwrap();
        drop(file);
        // The final owner releases the OS sharing lock; no raw-handle reuse race.
        drop(reopen().unwrap());
        assert_eq!(std::fs::read_to_string(&path.0).unwrap(), "句柄所有权");
    }
    #[test]
    fn invalid_factory_results_are_not_owned() {
        // SAFETY: sentinel failure results are explicitly accepted and rejected.
        assert!(unsafe { Handle::from_raw_owned(null_mut()) }.is_err());
        // SAFETY: sentinel failure results are explicitly accepted and rejected.
        assert!(unsafe { Handle::from_raw_owned(INVALID_HANDLE_VALUE) }.is_err());
    }
    #[test]
    fn failure_cleanup_reaps_children_after_the_parent_has_already_exited() {
        struct Cleanup(Vec<(u32, Handle)>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                for (_, handle) in &self.0 {
                    // SAFETY: retained handles identify exactly these children.
                    unsafe {
                        TerminateProcess(handle.raw(), 1);
                        WaitForSingleObject(handle.raw(), 5000);
                    }
                }
            }
        }
        let executable = std::env::var("ComSpec").unwrap();
        let ping = format!(
            "{}\\System32\\ping.exe",
            std::env::var("SystemRoot").unwrap()
        );
        let command =
            format!("\"{executable}\" /d /c start \"\" /b \"{ping}\" -n 60 127.0.0.1 >nul");
        let environment = inherited_environment();
        let child = spawn(Spawn {
            application: Some(&executable),
            command: &command,
            directory: "",
            environment: &environment,
            stdio: None,
            flags: CREATE_NO_WINDOW,
            affinity: 0,
        })
        .unwrap();
        assert!(
            child.wait(10000),
            "launcher must exit before failure cleanup"
        );
        let (created, exited) = process_times(&child.process).unwrap();
        assert_ne!(exited, 0);
        let descendants = Cleanup(descendants(child.pid, created, exited).unwrap());
        assert!(!descendants.0.is_empty(), "launcher must create a child");
        assert!(
            descendants.0.iter().any(|(_, handle)| {
                // SAFETY: the test retains ownership while querying the exact child.
                unsafe { WaitForSingleObject(handle.raw(), 0) == WAIT_TIMEOUT }
            }),
            "at least one child must still be alive"
        );
        drop(child);
        for (_, handle) in &descendants.0 {
            // SAFETY: retained handles remain valid after the parent's guard drops.
            assert_eq!(
                unsafe { WaitForSingleObject(handle.raw(), 5000) },
                WAIT_OBJECT_0
            );
        }
    }
    #[test]
    fn expansion_is_case_insensitive_and_preserves_unknowns() {
        let mut env = Environment::new();
        overlay_environment(
            &mut env,
            &["Path=C:\\bin".into(), "PATH=%path%;D:\\bin".into()],
        )
        .unwrap();
        assert_eq!(
            expand("%Path% %missing% %", &env),
            "C:\\bin;D:\\bin %missing% %"
        );
        assert_eq!(env.len(), 1);
    }
    #[test]
    fn empty_environment_is_double_terminated() {
        assert_eq!(environment_block(&Environment::new()).unwrap(), vec![0, 0]);
    }
    #[test]
    fn embedded_nul_is_rejected() {
        assert!(wide("a\0b").is_err());
        assert!(overlay_environment(&mut Environment::new(), &["=bad".into()]).is_err());
    }
    #[test]
    fn real_child_exit_code_259_is_not_confused_with_still_active() {
        let env = inherited_environment();
        let executable = std::env::var("ComSpec").unwrap();
        let command = format!("\"{executable}\" /d /c exit 259");
        let child = spawn(Spawn {
            application: Some(&executable),
            command: &command,
            directory: "",
            environment: &env,
            stdio: None,
            flags: CREATE_NO_WINDOW,
            affinity: 0,
        })
        .unwrap();
        assert!(child.wait(10000));
        assert_eq!(child.exit_code().unwrap(), Some(259));
    }
    #[test]
    fn failed_spawn_and_invalid_environment_return_errors() {
        let env = Environment::new();
        assert!(
            spawn(Spawn {
                application: Some("Z:\\xsm-nonexistent-app.exe"),
                command: "nonexistent",
                directory: "",
                environment: &env,
                stdio: None,
                flags: CREATE_NO_WINDOW,
                affinity: 0
            })
            .is_err()
        );
        let mut invalid = Environment::new();
        invalid.insert("BAD".into(), ("BAD=NAME".into(), "value".into()));
        assert!(environment_block(&invalid).is_err());
    }
}
