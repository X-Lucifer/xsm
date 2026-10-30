//! Deterministic child process used by the Windows integration suite.
use std::{
    io::{self, Write},
    time::Duration,
};
#[cfg(windows)]
fn process_birth(pid: u32) -> Option<u64> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    // SAFETY: OpenProcess returns a new owned handle or null on failure.
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        return None;
    }
    // SAFETY: the successful factory transfers its sole handle to OwnedHandle.
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut birth = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: handle and all initialized writable buffers remain alive.
    if unsafe {
        GetProcessTimes(
            handle.as_raw_handle(),
            &mut birth,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return None;
    }
    Some((u64::from(birth.dwHighDateTime) << 32) | u64::from(birth.dwLowDateTime))
}
#[cfg(not(windows))]
fn process_birth(_pid: u32) -> Option<u64> {
    None
}
fn snapshot_record(kind: &str) -> String {
    let mut record = format!("RECORD_KIND={kind}\nPROBE_PID={}\n", std::process::id());
    for (name, pid) in [
        ("PROBE_BIRTH_FILETIME", Some(std::process::id())),
        (
            "XSM_WRAPPER_BIRTH_FILETIME",
            std::env::var("XSM_PID").ok().and_then(|v| v.parse().ok()),
        ),
        (
            "XSM_APPLICATION_BIRTH_FILETIME",
            std::env::var("XSM_APPLICATION_PID")
                .ok()
                .and_then(|v| v.parse().ok()),
        ),
    ] {
        record.push_str(&format!(
            "{name}={}\n",
            pid.and_then(process_birth)
                .map(|v| v.to_string())
                .unwrap_or_default()
        ));
    }
    for key in [
        "XSM_TEST_TOKEN",
        "XSM_SAME_NAME",
        "XSM_HOOK_VERSION",
        "XSM_SERVICE_NAME",
        "XSM_PID",
        "XSM_APPLICATION_PID",
        "XSM_EVENT",
        "XSM_ACTION",
        "XSM_TRIGGER",
        "XSM_LAST_CONTROL",
        "XSM_START_REQUESTED_COUNT",
        "XSM_START_COUNT",
        "XSM_EXIT_COUNT",
        "XSM_THROTTLE_COUNT",
        "XSM_EXITCODE",
        "XSM_APPLICATION_RUNTIME",
    ] {
        let value = std::env::var(key).unwrap_or_default();
        record.push_str(&format!(
            "{key}={}\n",
            value.replace('\r', "\\r").replace('\n', "\\n")
        ));
    }
    record
}
fn snapshot(directory: &str, kind: &str) -> io::Result<()> {
    let directory = std::path::Path::new(directory);
    std::fs::create_dir_all(directory)?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let name = format!("{}-{nonce}", std::process::id());
    let temporary = directory.join(format!("{name}.tmp"));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(snapshot_record(kind).as_bytes())?;
    file.flush()?;
    drop(file);
    std::fs::rename(temporary, directory.join(format!("{name}.record")))
}
#[cfg(windows)]
#[allow(
    clippy::zombie_processes,
    reason = "This fixture deliberately exits before its child so the service manager must reap it"
)]
fn spawn_on_stop(path: &str) -> io::Result<()> {
    use std::{
        os::windows::{io::AsRawHandle, process::CommandExt},
        sync::atomic::{AtomicBool, Ordering},
    };
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::{
            Console::{CTRL_C_EVENT, SetConsoleCtrlHandler},
            Threading::{CREATE_NO_WINDOW, GetProcessTimes},
        },
    };
    static STOP: AtomicBool = AtomicBool::new(false);
    unsafe extern "system" fn control(kind: u32) -> i32 {
        if kind == CTRL_C_EVENT {
            STOP.store(true, Ordering::Release);
            1
        } else {
            0
        }
    }
    if unsafe { SetConsoleCtrlHandler(Some(control), 1) } == 0 {
        return Err(io::Error::last_os_error());
    }
    println!("READY:{}:spawn-on-stop", std::process::id());
    io::stdout().flush()?;
    while !STOP.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let child = std::process::Command::new(std::env::current_exe()?)
        .arg("serve")
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()?;
    let mut created = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe {
        GetProcessTimes(
            child.as_raw_handle(),
            &mut created,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let birth = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    std::fs::write(path, format!("{}\n{birth}\n", child.id()))?;
    // The parent exits now; the manager must find the just-created descendant.
    Ok(())
}
fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str).unwrap_or("serve") {
        "snapshot" => {
            snapshot(
                args.get(2)
                    .ok_or_else(|| io::Error::other("snapshot directory missing"))?,
                "hook",
            )?;
            let tag = std::env::var("XSM_TEST_TOKEN").unwrap_or_default();
            let event = std::env::var("XSM_EVENT").unwrap_or_default();
            let action = std::env::var("XSM_ACTION").unwrap_or_default();
            println!("HOOK|{tag}|{event}|{action}");
            eprintln!("HOOKERR|{tag}|{event}|{action}");
            io::stdout().flush()?;
            io::stderr().flush()?;
            std::thread::sleep(Duration::from_millis(
                args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0),
            ));
            std::process::exit(args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0));
        }
        "isolation" => {
            snapshot(
                args.get(2)
                    .ok_or_else(|| io::Error::other("snapshot directory missing"))?,
                "application",
            )?;
            let restart = std::path::Path::new(
                args.get(3)
                    .ok_or_else(|| io::Error::other("restart marker missing"))?,
            );
            let tag = std::env::var("XSM_TEST_TOKEN").unwrap_or_default();
            let mut sequence = 0u64;
            loop {
                println!("APP|{tag}|{}|{sequence}", std::process::id());
                eprintln!("ERR|{tag}|{}|{sequence}", std::process::id());
                io::stdout().flush()?;
                io::stderr().flush()?;
                if restart.exists() {
                    std::fs::remove_file(restart)?;
                    std::process::exit(7);
                }
                sequence += 1;
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        "exit" => std::process::exit(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0)),
        "report" => {
            println!("pid={}", std::process::id());
            println!("cwd={}", std::env::current_dir()?.display());
            for (i, arg) in args.iter().enumerate() {
                println!("arg[{i}]={arg}");
            }
            for key in [
                "XSM_TEST",
                "XSM_EXTRA",
                "XSM_HOOK_VERSION",
                "XSM_SERVICE_NAME",
                "XSM_EVENT",
                "XSM_ACTION",
            ] {
                println!("{key}={}", std::env::var(key).unwrap_or_default());
            }
            Ok(())
        }
        "flood" => {
            for i in 0..args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10000) {
                println!("{i:08}: 0123456789abcdefghijklmnopqrstuvwxyz 测试日志");
                if i % 100 == 0 {
                    io::stdout().flush()?;
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        "hook" => {
            let path = args
                .get(2)
                .ok_or_else(|| io::Error::other("hook output path missing"))?;
            let mut out = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            writeln!(
                out,
                "{}:{}:{}",
                std::env::var("XSM_SERVICE_NAME").unwrap_or_default(),
                std::env::var("XSM_EVENT").unwrap_or_default(),
                std::env::var("XSM_ACTION").unwrap_or_default()
            )?;
            Ok(())
        }
        "gate" => {
            let path = args
                .get(2)
                .ok_or_else(|| io::Error::other("gate path missing"))?;
            if !std::path::Path::new(path).exists() {
                std::process::exit(7);
            }
            println!("GATE_OPEN:{}", std::process::id());
            io::stdout().flush()?;
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        "tree" => {
            let path = args
                .get(2)
                .ok_or_else(|| io::Error::other("PID file missing"))?;
            let child = std::process::Command::new(std::env::current_exe()?)
                .arg("serve")
                .spawn()?;
            std::fs::write(path, format!("{}\n{}\n", std::process::id(), child.id()))?;
            println!("TREE:{}:{}", std::process::id(), child.id());
            io::stdout().flush()?;
            // The service manager must clean up both generations on STOP.
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        #[cfg(windows)]
        "spawn-on-stop" => spawn_on_stop(
            args.get(2)
                .ok_or_else(|| io::Error::other("PID file missing"))?,
        ),
        "serve" => {
            println!(
                "READY:{}:{}",
                std::process::id(),
                std::env::var("XSM_TEST").unwrap_or_default()
            );
            eprintln!("STDERR:{}", std::process::id());
            io::stdout().flush()?;
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        _ => Err(io::Error::other("unknown probe mode")),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn snapshot_contains_all_isolation_fields_once() {
        let record = super::snapshot_record("hook");
        let fields: std::collections::BTreeMap<_, _> = record
            .lines()
            .map(|line| line.split_once('=').unwrap())
            .collect();
        assert_eq!(fields.len(), record.lines().count());
        assert_eq!(fields["RECORD_KIND"], "hook");
        assert_eq!(fields["PROBE_PID"], std::process::id().to_string());
        #[cfg(windows)]
        assert!(fields["PROBE_BIRTH_FILETIME"].parse::<u64>().unwrap() > 0);
        for name in [
            "XSM_SERVICE_NAME",
            "XSM_PID",
            "XSM_APPLICATION_PID",
            "XSM_START_COUNT",
            "XSM_EXIT_COUNT",
            "XSM_TEST_TOKEN",
            "XSM_SAME_NAME",
        ] {
            assert!(fields.contains_key(name));
        }
    }
}
