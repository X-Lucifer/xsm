//! Redirected streams, timestamping and rotation; each pipe has one owned reader.
use super::process::{Environment, Handle, expand, wide};
use anyhow::{Context, Result};
use std::{
    fs::{self, File},
    io::Write,
    mem::size_of,
    os::windows::io::BorrowedHandle,
    path::{Path, PathBuf},
    ptr::null_mut,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::SECURITY_ATTRIBUTES,
    Storage::FileSystem::*,
    System::{Pipes::*, SystemInformation::*, Time::FileTimeToSystemTime},
};
use winreg::RegKey;

pub fn event(service: &str, error: bool, message: &str) {
    super::diagnostics::write(
        service,
        if error {
            super::diagnostics::Level::Error
        } else {
            super::diagnostics::Level::Info
        },
        message,
    );
}
#[derive(Clone)]
pub struct OutputConfig {
    pub path: PathBuf,
    pub sharing: u32,
    pub disposition: u32,
    pub flags: u32,
    pub copy: bool,
    pub rotate: bool,
    pub online: bool,
    pub bytes: u64,
    pub seconds: u32,
    pub delay: u32,
    pub timestamp: bool,
}
fn number(key: &RegKey, name: &str, default: u32) -> u32 {
    key.get_value(name).unwrap_or(default)
}
impl OutputConfig {
    fn load(key: &RegKey, name: &str, env: &Environment, directory: &str) -> Self {
        let raw: String = key.get_value(name).unwrap_or_default();
        Self {
            path: stream_path(&raw, env, directory),
            sharing: number(key, &format!("{name}ShareMode"), 3),
            disposition: number(key, &format!("{name}CreationDisposition"), OPEN_ALWAYS),
            flags: number(
                key,
                &format!("{name}FlagsAndAttributes"),
                FILE_ATTRIBUTE_NORMAL,
            ),
            copy: number(key, &format!("{name}CopyAndTruncate"), 0) != 0,
            rotate: number(key, "AppRotateFiles", 0) != 0,
            online: number(key, "AppRotateOnline", 0) != 0,
            bytes: (u64::from(number(key, "AppRotateBytesHigh", 0)) << 32)
                | u64::from(number(key, "AppRotateBytes", 0)),
            seconds: number(key, "AppRotateSeconds", 0),
            delay: number(key, "AppRotateDelay", 0),
            timestamp: number(key, "AppTimestampLog", 0) != 0,
        }
    }
}
fn stream_path(raw: &str, env: &Environment, directory: &str) -> PathBuf {
    let path = PathBuf::from(expand(raw, env));
    if path.as_os_str().is_empty() {
        return path;
    }
    let path = if path.is_absolute() || directory.is_empty() {
        path
    } else {
        Path::new(directory).join(path)
    };
    // Windows absolute() normalizes dot segments before stream identity checks.
    std::path::absolute(&path).unwrap_or(path)
}
fn open(path: &Path, read: bool, sharing: u32, disposition: u32, flags: u32) -> Result<Handle> {
    let path = wide(&path.to_string_lossy())?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    // SAFETY: CreateFileW returns a new owned kernel handle; all input buffers
    // and the security attributes remain alive through the call.
    let handle = unsafe {
        Handle::from_raw_owned(CreateFileW(
            path.as_ptr(),
            if read { GENERIC_READ } else { GENERIC_WRITE },
            sharing,
            &attributes,
            disposition,
            flags,
            null_mut(),
        ))
    }?;
    if !read
        && disposition == OPEN_ALWAYS
        && unsafe { SetFilePointerEx(handle.raw(), 0, null_mut(), FILE_END) } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(handle)
}
fn output_file(config: &OutputConfig) -> Result<File> {
    let handle = open(
        &config.path,
        false,
        config.sharing,
        config.disposition,
        config.flags,
    )?;
    Ok(handle.into_file())
}
fn system_time(value: SystemTime) -> SYSTEMTIME {
    let ticks = value
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        / 100
        + 116444736000000000;
    let filetime = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut st = Default::default();
    unsafe { FileTimeToSystemTime(&filetime, &mut st) };
    st
}
pub fn timestamp() -> String {
    let mut st: SYSTEMTIME = Default::default();
    unsafe { GetSystemTime(&mut st) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}: ",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
    )
}
fn rotated_name(path: &Path, time: SystemTime) -> PathBuf {
    let st = system_time(time);
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path
        .extension()
        .map(|v| format!(".{}", v.to_string_lossy()))
        .unwrap_or_default();
    path.with_file_name(format!(
        "{stem}-{:04}{:02}{:02}T{:02}{:02}{:02}.{:03}{ext}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
    ))
}
fn rotate_path(config: &OutputConfig, check_thresholds: bool) -> Result<()> {
    rotate_path_interruptible(config, check_thresholds, None)
}
fn rotate_path_interruptible(
    config: &OutputConfig,
    check_thresholds: bool,
    stop: Option<&AtomicBool>,
) -> Result<()> {
    let metadata = match fs::metadata(&config.path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let modified = metadata.modified()?;
    if check_thresholds {
        if config.bytes != 0 && metadata.len() < config.bytes {
            return Ok(());
        }
        if config.seconds != 0
            && modified.elapsed().unwrap_or_default()
                < Duration::from_secs(u64::from(config.seconds))
        {
            return Ok(());
        }
    }
    let base = rotated_name(&config.path, modified);
    let mut target = base.clone();
    let mut sequence = 0;
    while target.exists() {
        sequence += 1;
        target = PathBuf::from(format!("{}.{}", base.display(), sequence));
    }
    if config.copy {
        let mut source = File::open(&config.path)?;
        let mut destination = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?;
        std::io::copy(&mut source, &mut destination)?;
        destination.flush()?;
        let started = Instant::now();
        let delay = Duration::from_millis(u64::from(config.delay));
        while started.elapsed() < delay {
            if stop.is_some_and(|s| s.load(Ordering::Acquire)) || super::runtime::stop_requested() {
                break;
            }
            thread::sleep(
                delay
                    .saturating_sub(started.elapsed())
                    .min(Duration::from_millis(50)),
            );
        }
        let file = fs::OpenOptions::new().write(true).open(&config.path)?;
        file.set_len(0)?;
    } else {
        let source = wide(&config.path.to_string_lossy())?;
        let target = wide(&target.to_string_lossy())?;
        if unsafe { MoveFileW(source.as_ptr(), target.as_ptr()) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}
struct Worker {
    rotate: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<Result<()>>>,
}
pub struct Streams {
    pub input: Handle,
    pub output: Handle,
    pub error: Handle,
    workers: Arc<Vec<Worker>>,
}
impl Streams {
    pub fn load(key: &RegKey, env: &Environment, service: &str, directory: &str) -> Result<Self> {
        let stdin: String = key.get_value("AppStdin").unwrap_or_default();
        let input = open(
            &if stdin.is_empty() {
                PathBuf::from("NUL")
            } else {
                stream_path(&stdin, env, directory)
            },
            true,
            number(key, "AppStdinShareMode", FILE_SHARE_WRITE),
            number(key, "AppStdinCreationDisposition", OPEN_EXISTING),
            number(key, "AppStdinFlagsAndAttributes", FILE_ATTRIBUTE_NORMAL),
        )?;
        let stdout = OutputConfig::load(key, "AppStdout", env, directory);
        let stderr = OutputConfig::load(key, "AppStderr", env, directory);
        let mut workers = Vec::new();
        let output = Self::output(stdout.clone(), service, &mut workers)?;
        let error = if !stdout.path.as_os_str().is_empty()
            && super::registry::equal_name(
                &stdout.path.to_string_lossy(),
                &stderr.path.to_string_lossy(),
            ) {
            output.duplicate(true)?
        } else {
            Self::output(stderr, service, &mut workers)?
        };
        Ok(Self {
            input,
            output,
            error,
            workers: Arc::new(workers),
        })
    }
    fn output(config: OutputConfig, service: &str, workers: &mut Vec<Worker>) -> Result<Handle> {
        if config.path.as_os_str().is_empty() {
            return open(
                Path::new("NUL"),
                false,
                3,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
            );
        }
        if config.rotate
            && let Err(error) = rotate_path(&config, true)
        {
            event(
                service,
                true,
                &format!("Startup output rotation failed; retaining current file: {error:#}"),
            );
        }
        let file = output_file(&config)?;
        if !config.online && !config.timestamp {
            return Ok(Handle::from_file(file));
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null_mut(),
            bInheritHandle: 1,
        };
        let mut read = null_mut();
        let mut write = null_mut();
        if unsafe { CreatePipe(&mut read, &mut write, &attributes, 65536) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: successful CreatePipe transfers each end exactly once.
        let read = unsafe { Handle::from_raw_owned(read) }?;
        let write = unsafe { Handle::from_raw_owned(write) }?;
        read.set_inherit(false)?;
        let rotate = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let r = rotate.clone();
        let s = stop.clone();
        let service = service.to_owned();
        let join = thread::Builder::new()
            .name("xsm-log".into())
            .spawn(move || {
                let result = log_loop(read, file, config, r, s, &service);
                if let Err(ref e) = result {
                    event(&service, true, &format!("Logging failed: {e:#}"));
                }
                result
            })?;
        workers.push(Worker {
            rotate,
            stop,
            join: Some(join),
        });
        Ok(write)
    }
    pub fn handles(&self) -> [BorrowedHandle<'_>; 3] {
        [
            self.input.borrow(),
            self.output.borrow(),
            self.error.borrow(),
        ]
    }
    pub fn duplicate(&self) -> Result<Self> {
        Ok(Self {
            input: self.input.duplicate(true)?,
            output: self.output.duplicate(true)?,
            error: self.error.duplicate(true)?,
            workers: self.workers.clone(),
        })
    }
    pub fn rotate(&self) {
        for worker in self.workers.iter() {
            worker.rotate.fetch_add(1, Ordering::Release);
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
#[derive(Default)]
struct TimestampWriter {
    started: bool,
    utf16: Option<bool>,
    pending: Vec<u8>,
}
impl TimestampWriter {
    fn transform(&mut self, data: &[u8], stamp: &str) -> Vec<u8> {
        self.pending.extend_from_slice(data);
        if self.utf16.is_none() {
            if self.pending.len() < 2 {
                return Vec::new();
            }
            self.utf16 = Some(self.pending.starts_with(&[0xff, 0xfe]) || self.pending[1] == 0);
        }
        let wide = self.utf16 == Some(true);
        let complete = if wide {
            self.pending.len() / 2 * 2
        } else {
            self.pending.len()
        };
        let data: Vec<_> = self.pending.drain(..complete).collect();
        let mut out = Vec::new();
        for unit in data.chunks(if wide { 2 } else { 1 }) {
            if unit == [0xff, 0xfe] {
                out.extend(unit);
                continue;
            }
            if !self.started {
                if wide {
                    for c in stamp.encode_utf16() {
                        out.extend(c.to_le_bytes())
                    }
                } else {
                    out.extend(stamp.as_bytes())
                }
                self.started = true;
            }
            out.extend(unit);
            if (wide && unit == [b'\n', 0]) || (!wide && unit == b"\n") {
                self.started = false;
            }
        }
        out
    }
}
fn rotate_output(
    mut file: File,
    config: &OutputConfig,
    stop: &AtomicBool,
    service: &str,
    warned: &mut bool,
) -> Result<File> {
    file.flush()?;
    drop(file);
    match rotate_path_interruptible(config, false, Some(stop)) {
        Ok(()) => {
            *warned = false;
            super::diagnostics::write(
                service,
                super::diagnostics::Level::Debug,
                "Output log rotated",
            );
            output_file(config)
        }
        Err(error) => {
            if !*warned {
                event(
                    service,
                    true,
                    &format!("Output rotation failed; continuing current log: {error:#}"),
                );
            }
            *warned = true;
            let mut fallback = config.clone();
            fallback.disposition = OPEN_ALWAYS;
            output_file(&fallback)
        }
    }
}
fn log_loop(
    read: Handle,
    mut file: File,
    config: OutputConfig,
    rotate: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    service: &str,
) -> Result<()> {
    super::diagnostics::write(
        service,
        super::diagnostics::Level::Debug,
        &format!(
            "Output logger initialized: online rotation={}, timestamps={}",
            config.online, config.timestamp
        ),
    );
    let mut buffer = vec![0u8; 65536];
    let mut seen = 0;
    let mut writer = TimestampWriter::default();
    let mut stopping = None;
    let mut line_start = true;
    let mut size = file.metadata()?.len();
    let mut rotation_failed = false;
    let mut retry_after_size = None;
    loop {
        if stop.load(Ordering::Acquire) && stopping.is_none() {
            stopping = Some(Instant::now());
        }
        let mut available = 0;
        if unsafe {
            PeekNamedPipe(
                read.raw(),
                null_mut(),
                0,
                null_mut(),
                &mut available,
                null_mut(),
            )
        } == 0
        {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
                break;
            } else {
                return Err(e.into());
            }
        }
        let requested = rotate.load(Ordering::Acquire);
        if config.online
            && line_start
            && (requested != seen
                || (config.bytes != 0
                    && size >= config.bytes
                    && retry_after_size.is_none_or(|previous| size > previous)))
        {
            file = rotate_output(file, &config, &stop, service, &mut rotation_failed)?;
            size = file.metadata()?.len();
            retry_after_size = rotation_failed.then_some(size);
            seen = requested;
        }
        if available == 0 {
            if stopping.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
            continue;
        }
        let mut count = 0;
        let take = available.min(buffer.len() as u32);
        if unsafe {
            ReadFile(
                read.raw(),
                buffer.as_mut_ptr(),
                take,
                &mut count,
                null_mut(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let data = &buffer[..count as usize];
        if data.is_empty() {
            break;
        }
        super::diagnostics::write(
            service,
            super::diagnostics::Level::Trace,
            &format!("Output logger received {count} bytes"),
        );
        // Decode complete UTF-16 code units even when timestamp insertion is off;
        // rotation must never separate the two bytes of a newline.
        let transformed = writer.transform(
            data,
            &if config.timestamp {
                timestamp()
            } else {
                String::new()
            },
        );
        // Rotate at each line boundary, including boundaries inside one pipe read.
        let unit = if writer.utf16 == Some(true) { 2 } else { 1 };
        let mut offset = 0;
        while offset < transformed.len() {
            if config.online
                && line_start
                && (requested != seen
                    || (config.bytes != 0
                        && size >= config.bytes
                        && retry_after_size.is_none_or(|previous| size > previous)))
            {
                file = rotate_output(file, &config, &stop, service, &mut rotation_failed)?;
                size = file.metadata()?.len();
                retry_after_size = rotation_failed.then_some(size);
                seen = requested;
            }
            let next = transformed[offset..].chunks(unit).position(|part| {
                if unit == 2 {
                    part == [b'\n', 0]
                } else {
                    part == b"\n"
                }
            });
            let end = next.map_or(transformed.len(), |index| offset + (index + 1) * unit);
            let segment = &transformed[offset..end];
            file.write_all(segment)?;
            size = size.saturating_add(segment.len() as u64);
            line_start = next.is_some();
            offset = end;
        }
        if stopping.is_some_and(|s| s.elapsed() > Duration::from_millis(1500)) {
            break;
        }
    }
    if !writer.pending.is_empty() {
        if config.timestamp && !writer.started {
            if writer.utf16 == Some(true) {
                file.write_all(
                    &timestamp()
                        .encode_utf16()
                        .flat_map(u16::to_le_bytes)
                        .collect::<Vec<_>>(),
                )?;
            } else {
                file.write_all(timestamp().as_bytes())?;
            }
        }
        file.write_all(&writer.pending)?;
    }
    file.flush().context("Flush application log")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamps_survive_chunk_boundaries() {
        let mut writer = TimestampWriter::default();
        assert_eq!(writer.transform(b"he", "T "), b"T he");
        assert_eq!(writer.transform(b"llo\nnext\n", "U "), b"llo\nU next\n");
        assert_eq!(writer.transform(b"a", "V "), b"V a");
    }
    #[test]
    fn utf16_bom_and_split_units_are_preserved() {
        let mut w = TimestampWriter::default();
        assert_eq!(w.transform(&[255], "T"), Vec::<u8>::new());
        assert_eq!(
            w.transform(&[254, 65, 0, 10], "T"),
            [255, 254, 84, 0, 65, 0]
        );
        assert_eq!(w.transform(&[0, 66, 0], "U"), [10, 0, 85, 0, 66, 0]);
    }
    #[test]
    fn rotation_filename_preserves_extension() {
        let path = rotated_name(Path::new("C:\\logs\\app.log"), SystemTime::UNIX_EPOCH);
        assert_eq!(path, PathBuf::from("C:\\logs\\app-19700101T000000.000.log"));
    }
    #[test]
    fn relative_streams_resolve_against_application_directory() {
        assert_eq!(
            stream_path("logs\\out.txt", &Environment::new(), "C:\\app"),
            PathBuf::from("C:\\app\\logs\\out.txt")
        );
        assert!(
            stream_path("", &Environment::new(), "C:\\app")
                .as_os_str()
                .is_empty()
        );
    }
    struct TempDirectory(PathBuf);
    impl TempDirectory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "xsm-output-log-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn config(path: PathBuf) -> OutputConfig {
        OutputConfig {
            path,
            sharing: 3,
            disposition: OPEN_ALWAYS,
            flags: FILE_ATTRIBUTE_NORMAL,
            copy: false,
            rotate: true,
            online: true,
            bytes: 4,
            seconds: 0,
            delay: 0,
            timestamp: false,
        }
    }
    #[test]
    fn offline_rotation_obeys_threshold_and_preserves_bytes() {
        let dir = TempDirectory::new();
        let cfg = config(dir.0.join("测试.log"));
        fs::write(&cfg.path, b"abc").unwrap();
        rotate_path(&cfg, true).unwrap();
        assert_eq!(fs::read(&cfg.path).unwrap(), b"abc");
        fs::write(&cfg.path, b"abcdef").unwrap();
        rotate_path(&cfg, true).unwrap();
        assert!(!cfg.path.exists());
        let rotated: Vec<_> = fs::read_dir(&dir.0).unwrap().collect();
        assert_eq!(rotated.len(), 1);
        assert_eq!(
            fs::read(rotated[0].as_ref().unwrap().path()).unwrap(),
            b"abcdef"
        );
    }
    #[test]
    fn copy_truncate_retains_original_and_snapshot() {
        let dir = TempDirectory::new();
        let mut cfg = config(dir.0.join("copy.log"));
        cfg.copy = true;
        fs::write(&cfg.path, b"payload").unwrap();
        rotate_path(&cfg, false).unwrap();
        assert_eq!(fs::read(&cfg.path).unwrap(), b"");
        let files: Vec<_> = fs::read_dir(&dir.0)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(files.len(), 2);
        let copy = files.iter().find(|p| **p != cfg.path).unwrap();
        assert_eq!(fs::read(copy).unwrap(), b"payload");
    }
    #[test]
    fn real_child_preserves_unicode_environment_and_separates_streams() {
        let dir = TempDirectory::new();
        let input = open(
            Path::new("NUL"),
            true,
            3,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
        )
        .unwrap();
        let out_path = dir.0.join("out.log");
        let err_path = dir.0.join("err.log");
        let output = open(&out_path, false, 3, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL).unwrap();
        let error = open(&err_path, false, 3, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL).unwrap();
        let mut env = super::super::process::inherited_environment();
        super::super::process::overlay_environment(&mut env, &["XSM_TEST_VALUE=你好 world".into()])
            .unwrap();
        let executable = std::env::var("ComSpec").unwrap();
        let command = format!("\"{executable}\" /d /u /c echo %XSM_TEST_VALUE% & echo error 1>&2");
        let child = super::super::process::spawn(super::super::process::Spawn {
            application: Some(&executable),
            command: &command,
            directory: "",
            environment: &env,
            stdio: Some([input.borrow(), output.borrow(), error.borrow()]),
            flags: windows_sys::Win32::System::Threading::CREATE_NO_WINDOW,
            affinity: 0,
        })
        .unwrap();
        assert!(child.wait(10000));
        assert_eq!(child.exit_code().unwrap(), Some(0));
        let decode = |path: &Path| {
            String::from_utf16(
                &fs::read(path)
                    .unwrap()
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| u16::from_le_bytes([b[0], b[1]]))
                    .collect::<Vec<_>>(),
            )
            .unwrap()
        };
        assert_eq!(decode(&out_path).trim(), "你好 world");
        assert_eq!(decode(&err_path).trim(), "error");
    }
    #[test]
    fn pipe_rotation_splits_multiple_lines_in_a_single_read() {
        let dir = TempDirectory::new();
        let cfg = config(dir.0.join("online.log"));
        let mut workers = Vec::new();
        let handle = Streams::output(cfg.clone(), "test", &mut workers).unwrap();
        let mut writer = handle.into_file();
        writer.write_all(b"one\ntwo\nthree\n").unwrap();
        drop(writer);
        drop(workers);
        let mut contents: Vec<Vec<u8>> = fs::read_dir(&dir.0)
            .unwrap()
            .map(|e| fs::read(e.unwrap().path()).unwrap())
            .collect();
        contents.sort();
        assert_eq!(
            contents,
            vec![b"one\n".to_vec(), b"three\n".to_vec(), b"two\n".to_vec()]
        );
    }
    #[test]
    fn utf16_rotation_without_timestamps_preserves_split_code_units() {
        let mut decoder = TimestampWriter::default();
        assert_eq!(decoder.transform(&[0xff], ""), Vec::<u8>::new());
        assert_eq!(decoder.transform(&[0xfe, b'A'], ""), [0xff, 0xfe]);
        assert_eq!(decoder.transform(&[0, b'\n'], ""), [b'A', 0]);
        assert_eq!(
            decoder.transform(&[0, b'B', 0, b'\n', 0], ""),
            [b'\n', 0, b'B', 0, b'\n', 0]
        );
        let dir = TempDirectory::new();
        let cfg = config(dir.0.join("utf16.log"));
        let mut workers = Vec::new();
        let handle = Streams::output(cfg, "test", &mut workers).unwrap();
        let mut pipe = handle.into_file();
        pipe.write_all(&[0xff, 0xfe, b'A', 0, b'\n', 0, b'B', 0, b'\n', 0])
            .unwrap();
        drop(pipe);
        drop(workers);
        let mut files: Vec<_> = fs::read_dir(&dir.0)
            .unwrap()
            .map(|f| fs::read(f.unwrap().path()).unwrap())
            .filter(|v| !v.is_empty())
            .collect();
        files.sort();
        assert_eq!(
            files,
            vec![vec![b'B', 0, b'\n', 0], vec![0xff, 0xfe, b'A', 0, b'\n', 0]]
        );
    }
    #[test]
    fn copy_rotation_delay_is_interruptible_during_shutdown() {
        let dir = TempDirectory::new();
        let mut cfg = config(dir.0.join("interrupt.log"));
        cfg.copy = true;
        cfg.delay = 60_000;
        fs::write(&cfg.path, b"before shutdown").unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let signal = stop.clone();
        let started = Instant::now();
        let task = thread::spawn(move || rotate_path_interruptible(&cfg, false, Some(&signal)));
        thread::sleep(Duration::from_millis(50));
        stop.store(true, Ordering::Release);
        task.join().unwrap().unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(fs::read(dir.0.join("interrupt.log")).unwrap(), b"");
    }
    #[test]
    fn failed_rotation_keeps_logging_without_losing_output() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = TempDirectory::new();
        let cfg = config(dir.0.join("locked.log"));
        let mut file = output_file(&cfg).unwrap();
        file.write_all(b"first\n").unwrap();
        let locked = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&cfg.path)
            .unwrap();
        let mut warned = false;
        let mut reopened =
            rotate_output(file, &cfg, &AtomicBool::new(false), "test", &mut warned).unwrap();
        assert!(warned);
        reopened.write_all(b"second\n").unwrap();
        drop(reopened);
        drop(locked);
        assert_eq!(fs::read(&cfg.path).unwrap(), b"first\nsecond\n");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }
    #[test]
    fn stream_identity_normalizes_dot_segments() {
        assert_eq!(
            stream_path("logs\\..\\output.log", &Environment::new(), "C:\\app"),
            stream_path("output.log", &Environment::new(), "C:\\app")
        );
    }
}
