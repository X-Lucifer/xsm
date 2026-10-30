//! Filtered process diagnostics. A single mutex serializes complete records and
//! rotation; application stdout/stderr remain an independent stream.
use super::registry;
use anyhow::{Context, Result, bail, ensure};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::{
    fmt,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Mutex, OnceLock},
};
use windows_sys::Win32::{
    Foundation::{ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, FILETIME, SYSTEMTIME},
    Storage::FileSystem::{FileTimeToLocalFileTime, MoveFileW},
    System::{EventLog::*, SystemInformation::GetLocalTime, Time::FileTimeToSystemTime},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}
impl Level {
    pub fn allows(self, level: Self) -> bool {
        level != Self::Off && self >= level
    }
}
impl FromStr for Level {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "off" => Self::Off,
            "error" => Self::Error,
            "warn" => Self::Warn,
            "info" => Self::Info,
            "debug" => Self::Debug,
            "trace" => Self::Trace,
            _ => bail!("Log level must be off, error, warn, info, debug or trace"),
        })
    }
}
impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Off => "off",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        })
    }
}
const DEFAULT_BYTES: u64 = 10 * 1024 * 1024;
const DEFAULT_FILES: u32 = 5;
const HEADER: &str = "Time | Level | Service | PID | Operation | Message\r\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Day(u32);
impl Day {
    fn from_system(time: &SYSTEMTIME) -> Self {
        Self(u32::from(time.wYear) * 10000 + u32::from(time.wMonth) * 100 + u32::from(time.wDay))
    }
    fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() < 10 || bytes[4] != b'-' || bytes[7] != b'-' {
            return None;
        }
        let year = std::str::from_utf8(&bytes[..4]).ok()?.parse::<u32>().ok()?;
        let month = std::str::from_utf8(&bytes[5..7])
            .ok()?
            .parse::<u32>()
            .ok()?;
        let day = std::str::from_utf8(&bytes[8..10])
            .ok()?
            .parse::<u32>()
            .ok()?;
        let max = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100))
                {
                    29
                } else {
                    28
                }
            }
            _ => return None,
        };
        (year > 0 && (1..=max).contains(&day)).then_some(Self(year * 10000 + month * 100 + day))
    }
}
fn local_time() -> SYSTEMTIME {
    let mut now = SYSTEMTIME::default();
    unsafe { GetLocalTime(&mut now) };
    now
}
fn metadata_day(file: &File) -> Result<Day> {
    let time = file
        .metadata()?
        .modified()?
        .duration_since(std::time::SystemTime::UNIX_EPOCH)?;
    let ticks = time.as_nanos() / 100 + 116444736000000000;
    let utc = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut local = FILETIME::default();
    let mut calendar = SYSTEMTIME::default();
    if unsafe { FileTimeToLocalFileTime(&utc, &mut local) } == 0
        || unsafe { FileTimeToSystemTime(&local, &mut calendar) } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(Day::from_system(&calendar))
}

struct LogPaths {
    directory: PathBuf,
    stem: String,
    extension: String,
}
impl LogPaths {
    fn new(path: &Path) -> Result<Self> {
        let mut stem = path
            .file_stem()
            .context("Diagnostics path must name a file")?
            .to_string_lossy()
            .into_owned();
        if stem.to_ascii_lowercase().ends_with("-latest") {
            stem.truncate(stem.len() - 7);
        }
        ensure!(
            !stem.is_empty(),
            "Diagnostics filename needs a base before -latest"
        );
        let extension = path
            .extension()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "log".into());
        Ok(Self {
            directory: path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."))
                .into(),
            stem,
            extension,
        })
    }
    fn active(&self) -> PathBuf {
        self.directory
            .join(format!("{}-latest.{}", self.stem, self.extension))
    }
    fn archive(&self, day: Day, sequence: u32) -> PathBuf {
        let suffix = if sequence == 0 {
            String::new()
        } else {
            format!(".{sequence:03}")
        };
        self.directory.join(format!(
            "{}-{:08}{suffix}.{}",
            self.stem, day.0, self.extension
        ))
    }
    fn archives(&self) -> Result<Vec<(Day, u32, PathBuf)>> {
        let prefix = format!("{}-", self.stem);
        let suffix = format!(".{}", self.extension);
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&self.directory)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let end = name.len().saturating_sub(suffix.len());
            if !name
                .get(..prefix.len())
                .is_some_and(|part| registry::equal_name(part, &prefix))
                || !name
                    .get(end..)
                    .is_some_and(|part| registry::equal_name(part, &suffix))
            {
                continue;
            }
            let Some(body) = name.get(prefix.len()..end) else {
                continue;
            };
            let (date, sequence) = body.split_once('.').map_or((body, "0"), |(d, s)| (d, s));
            if date.len() != 8 || !date.bytes().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let Some(day) = Day::parse(&format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..]))
            else {
                continue;
            };
            let Ok(sequence) = sequence.parse::<u32>() else {
                continue;
            };
            files.push((day, sequence, entry.path()));
        }
        files.sort_by_key(|(day, sequence, _)| (*day, *sequence));
        Ok(files)
    }
}

struct FileSink {
    paths: LogPaths,
    file: Option<File>,
    bytes: u64,
    max_bytes: u64,
    max_files: u32,
    day: Option<Day>,
    header: bool,
    has_records: bool,
}
impl FileSink {
    fn open(path: PathBuf, max_bytes: u64, max_files: u32) -> Result<Self> {
        ensure!(
            (128..=u64::from(u32::MAX)).contains(&max_bytes),
            "XsmLogMaxBytes must be 128..4294967295"
        );
        ensure!(
            (1..=100).contains(&max_files),
            "XsmLogMaxFiles must be 1..100"
        );
        let paths = LogPaths::new(&path)?;
        std::fs::create_dir_all(&paths.directory).context("Creating diagnostics directory")?;
        let mut file = Self::open_file(&paths.active())?;
        let bytes = file.metadata()?.len();
        let mut prefix = vec![0u8; HEADER.len() + 32];
        let read = file.read(&mut prefix)?;
        prefix.truncate(read);
        let header = prefix.starts_with(HEADER.as_bytes());
        let has_records = bytes > 0 && (!header || bytes > HEADER.len() as u64);
        let day = if has_records {
            let start = if header { HEADER.len() } else { 0 };
            let parsed = prefix
                .get(start..start + 10)
                .and_then(|v| std::str::from_utf8(v).ok())
                .and_then(Day::parse);
            Some(match parsed {
                Some(day) => day,
                None => metadata_day(&file)?,
            })
        } else {
            None
        };
        Ok(Self {
            paths,
            file: Some(file),
            bytes,
            max_bytes,
            max_files,
            day,
            header,
            has_records,
        })
    }
    fn open_file(path: &Path) -> Result<File> {
        // Other processes can tail the log, but cannot race this writer/rotator.
        OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .share_mode(1)
            .open(path)
            .context("Opening diagnostics file")
    }
    fn rotate(&mut self) -> Result<()> {
        if let Some(mut f) = self.file.take() {
            f.flush()?;
        }
        let day = self.day.context("Cannot archive a log without records")?;
        let mut sequence = self
            .paths
            .archives()?
            .iter()
            .filter(|(d, _, _)| *d == day)
            .map(|(_, s, _)| *s)
            .max()
            .map_or(Some(0), |n| n.checked_add(1))
            .context("Diagnostics archive sequence exhausted")?;
        let source: Vec<u16> = self
            .paths
            .active()
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect();
        loop {
            let destination: Vec<u16> = self
                .paths
                .archive(day, sequence)
                .as_os_str()
                .encode_wide()
                .chain([0])
                .collect();
            if unsafe { MoveFileW(source.as_ptr(), destination.as_ptr()) } != 0 {
                break;
            }
            let error = std::io::Error::last_os_error();
            if matches!(error.raw_os_error(),Some(code) if code==ERROR_ALREADY_EXISTS as i32||code==ERROR_FILE_EXISTS as i32)
            {
                sequence = sequence
                    .checked_add(1)
                    .context("Diagnostics archive sequence exhausted")?;
                continue;
            }
            return Err(error).context("Archiving diagnostics file");
        }
        self.file = Some(Self::open_file(&self.paths.active())?);
        self.bytes = 0;
        self.header = false;
        self.has_records = false;
        self.day = None;
        let files = self.paths.archives()?;
        let remove = files.len().saturating_sub(self.max_files as usize);
        for (_, _, path) in files.into_iter().take(remove) {
            std::fs::remove_file(path).context("Applying diagnostics archive retention")?;
        }
        Ok(())
    }
    fn append(&mut self, day: Day, record: &str) -> Result<()> {
        let record = bounded_record(record, self.max_bytes as usize - HEADER.len());
        if self.has_records
            && (self.day != Some(day)
                || !self.header
                || self.bytes.saturating_add(record.len() as u64) > self.max_bytes)
        {
            self.rotate()?;
        }
        if self.file.is_none() {
            self.file = Some(Self::open_file(&self.paths.active())?);
            self.bytes = self.file.as_ref().unwrap().metadata()?.len();
        }
        if !self.header {
            self.file.as_mut().unwrap().write_all(HEADER.as_bytes())?;
            self.bytes += HEADER.len() as u64;
            self.header = true;
        }
        self.file.as_mut().unwrap().write_all(record.as_bytes())?;
        self.bytes += record.len() as u64;
        self.day = Some(day);
        self.has_records = true;
        Ok(())
    }
}
fn bounded_record(record: &str, max: usize) -> String {
    if record.len() <= max {
        return record.into();
    }
    let mut fields = record
        .trim_end_matches(['\r', '\n'])
        .splitn(6, " | ")
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if fields.len() == 6 {
        for index in [5, 2, 4] {
            let length = fields.iter().map(String::len).sum::<usize>() + 17;
            if length <= max {
                break;
            }
            let keep = fields[index].len().saturating_sub(length - max).max(1);
            fields[index] = truncate(&fields[index], keep);
        }
        return format!("{}\r\n", fields.join(" | "));
    }
    format!(
        "{}\r\n",
        truncate(record.trim_end_matches(['\r', '\n']), max.saturating_sub(2))
    )
}
fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.into();
    }
    if max == 0 {
        return String::new();
    }
    let suffix = if max >= 14 { "...[truncated]" } else { "~" };
    let mut end = max.saturating_sub(suffix.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}", &value[..end])
}
struct State {
    level: Level,
    sink: Option<FileSink>,
    error: Option<String>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            level: Level::Info,
            sink: None,
            error: None,
        }
    }
}
fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}
pub fn enabled(level: Level) -> bool {
    state()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .level
        .allows(level)
}
pub fn last_error() -> Option<String> {
    state()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .error
        .clone()
}
fn release_sink(state: &mut State) -> Result<()> {
    if let Some(mut sink) = state.sink.take()
        && let Some(file) = sink.file.as_mut()
    {
        file.flush()
            .context("Flushing diagnostics before handing off execution")?;
    }
    Ok(())
}
/// Release the exclusive file writer before starting an elevated child process.
/// Filtering and Event Log diagnostics remain available in the waiting parent.
pub fn release_file() -> Result<()> {
    release_sink(&mut state().lock().unwrap_or_else(|e| e.into_inner()))
}
fn env_level() -> Result<Level> {
    std::env::var("XSM_LOG_LEVEL")
        .ok()
        .map(|v| v.parse())
        .transpose()
        .map(|v| v.unwrap_or(Level::Info))
}
fn env_path() -> String {
    std::env::var("XSM_LOG_FILE").unwrap_or_default()
}
fn apply(level: Level, path: &str, max_bytes: u64, max_files: u32) -> Result<()> {
    let path = registry::expand(path)?;
    let mut state = state().lock().unwrap_or_else(|e| e.into_inner());
    // Release a prior writer before reopening the same path with changed limits.
    state.sink = None;
    let sink = if path.is_empty() || level == Level::Off {
        None
    } else {
        Some(FileSink::open(PathBuf::from(path), max_bytes, max_files)?)
    };
    state.level = level;
    state.sink = sink;
    state.error = None;
    Ok(())
}
/// Configure CLI diagnostics from XSM_LOG_LEVEL and XSM_LOG_FILE.
pub fn initialize() -> Result<()> {
    apply(env_level()?, &env_path(), DEFAULT_BYTES, DEFAULT_FILES)
}
/// Called by the elevated installer; existing service and log settings are untouched.
pub fn register_source() -> Result<()> {
    let key = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
        .create_subkey("SYSTEM\\CurrentControlSet\\Services\\EventLog\\Application\\xsm")?
        .0;
    registry::set_string(
        &key,
        "EventMessageFile",
        &std::env::current_exe()?.to_string_lossy(),
        true,
    )?;
    key.set_value("TypesSupported", &7u32)?;
    Ok(())
}
/// Registry values override the environment only when explicitly present.
pub fn configure(service: &str) -> Result<()> {
    let key = registry::parameters(service, false)?;
    let level = match registry::string(&key, "XsmLogLevel")? {
        Some(value) => value.parse()?,
        None => env_level()?,
    };
    let path = registry::string(&key, "XsmLogPath")?.unwrap_or_else(env_path);
    let max = u64::from(registry::dword(&key, "XsmLogMaxBytes")?.unwrap_or(DEFAULT_BYTES as u32));
    let files = registry::dword(&key, "XsmLogMaxFiles")?.unwrap_or(DEFAULT_FILES);
    apply(level, &path, max, files)
}
fn sanitize(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('|', "\\u007C")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
        .replace('\0', "\\0")
}
fn record(now: &SYSTEMTIME, service: &str, level: Level, operation: &str, message: &str) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} | {:<5} | {} | {} | {} | {}\r\n",
        now.wYear,
        now.wMonth,
        now.wDay,
        now.wHour,
        now.wMinute,
        now.wSecond,
        now.wMilliseconds,
        level.to_string().to_ascii_uppercase(),
        sanitize(service),
        std::process::id(),
        sanitize(operation),
        sanitize(message)
    )
}
fn event(level: Level, message: &str) {
    let source: Vec<u16> = "xsm".encode_utf16().chain([0]).collect();
    // ReportEvent's insertion string limit is 31,839 UTF-16 code units.
    let mut message_units = Vec::with_capacity(message.len().min(30_001));
    for character in message.chars() {
        if message_units.len() + character.len_utf16() > 30_000 {
            break;
        }
        let mut units = [0u16; 2];
        message_units.extend_from_slice(character.encode_utf16(&mut units));
    }
    message_units.push(0);
    unsafe {
        let log = RegisterEventSourceW(std::ptr::null(), source.as_ptr());
        if !log.is_null() {
            let strings = [message_units.as_ptr()];
            ReportEventW(
                log,
                match level {
                    Level::Error => EVENTLOG_ERROR_TYPE,
                    Level::Warn => EVENTLOG_WARNING_TYPE,
                    _ => EVENTLOG_INFORMATION_TYPE,
                },
                0,
                0,
                std::ptr::null_mut(),
                1,
                0,
                strings.as_ptr(),
                std::ptr::null(),
            );
            DeregisterEventSource(log);
        }
    }
}
pub fn write(service: &str, level: Level, message: &str) {
    write_operation(service, level, "runtime", message)
}
/// Callers must supply diagnostic summaries, never passwords, complete command
/// lines, environment contents or account credentials.
pub fn write_operation(service: &str, level: Level, operation: &str, message: &str) {
    let mut state = state().lock().unwrap_or_else(|e| e.into_inner());
    if !state.level.allows(level) {
        return;
    }
    let now = local_time();
    let line = record(&now, service, level, operation, message);
    if let Some(sink) = state.sink.as_mut()
        && let Err(error) = sink.append(Day::from_system(&now), &line)
    {
        let summary = format!("Diagnostics file write failed: {error}");
        if state.error.as_ref() != Some(&summary) {
            event(Level::Error, &summary);
        }
        state.error = Some(summary);
    }
    event(level, &line);
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            static ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let root = std::env::temp_dir().join(format!(
                "xsm-diagnostics-test-{}-{}",
                std::process::id(),
                ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn time(year: u16, month: u16, day: u16, hour: u16) -> SYSTEMTIME {
        SYSTEMTIME {
            wYear: year,
            wMonth: month,
            wDay: day,
            wHour: hour,
            ..Default::default()
        }
    }
    fn append(sink: &mut FileSink, now: SYSTEMTIME, message: &str) {
        sink.append(
            Day::from_system(&now),
            &record(&now, "test", Level::Info, "operation", message),
        )
        .unwrap();
    }
    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }
    fn records(path: &Path) -> Vec<String> {
        let text = read(path);
        assert_eq!(text.lines().next().unwrap(), HEADER.trim_end());
        assert_eq!(text.matches(HEADER.trim_end()).count(), 1);
        text.lines().skip(1).map(str::to_owned).collect()
    }

    #[test]
    fn level_order_and_validation() {
        assert!(Level::Info.allows(Level::Error));
        assert!(!Level::Info.allows(Level::Debug));
        assert!(!Level::Off.allows(Level::Error));
        assert!(!Level::Trace.allows(Level::Off));
        assert_eq!("DEBUG".parse::<Level>().unwrap(), Level::Debug);
        assert!("verbose".parse::<Level>().is_err());
    }
    #[test]
    fn filename_normalizes_latest_and_supports_unicode() {
        let paths = LogPaths::new(Path::new("C:\\日志\\运行-latest.log")).unwrap();
        assert_eq!(paths.active(), PathBuf::from("C:\\日志\\运行-latest.log"));
        assert_eq!(
            paths.archive(Day(20261029), 0),
            PathBuf::from("C:\\日志\\运行-20261029.log")
        );
        assert_eq!(
            paths.archive(Day(20261029), 1),
            PathBuf::from("C:\\日志\\运行-20261029.001.log")
        );
    }
    #[test]
    fn local_clock_record_has_six_columns_and_escapes_injection() {
        let line = record(
            &time(2026, 10, 29, 16),
            "a|b",
            Level::Warn,
            "stop\r\nforged",
            "hello\0\r\nworld|pipe",
        );
        assert_eq!(line.lines().count(), 1);
        assert_eq!(line.matches('|').count(), 5);
        let fields = line.trim_end().split(" | ").collect::<Vec<_>>();
        assert_eq!(fields.len(), 6);
        assert_eq!(fields[0], "2026-10-29 16:00:00.000");
        assert_eq!(fields[1].trim(), "WARN");
        assert!(line.contains("stop\\r\\nforged"));
        assert!(!line.contains("level="));
        assert!(!line.contains("message="));
    }
    #[test]
    fn midnight_month_and_year_roll_into_previous_day_name() {
        let dir = TempDir::new();
        let base = dir.0.join("events.log");
        let mut sink = FileSink::open(base, 4096, 10).unwrap();
        append(&mut sink, time(2026, 10, 31, 23), "october");
        append(&mut sink, time(2026, 11, 1, 0), "november");
        append(&mut sink, time(2026, 12, 31, 23), "december");
        append(&mut sink, time(2027, 1, 1, 0), "january");
        drop(sink);
        for (day, message) in [
            ("20261031", "october"),
            ("20261101", "november"),
            ("20261231", "december"),
        ] {
            let rows = records(&dir.0.join(format!("events-{day}.log")));
            assert_eq!(rows.len(), 1);
            assert!(rows[0].ends_with(message));
        }
        assert!(records(&dir.0.join("events-latest.log"))[0].ends_with("january"));
    }
    #[test]
    fn restart_resumes_without_duplicate_header_and_rolls_old_day() {
        let dir = TempDir::new();
        let path = dir.0.join("events-latest.log");
        {
            let mut sink = FileSink::open(path.clone(), 4096, 5).unwrap();
            append(&mut sink, time(2026, 10, 29, 12), "before restart");
        }
        {
            let mut sink = FileSink::open(path.clone(), 4096, 5).unwrap();
            append(&mut sink, time(2026, 10, 29, 13), "after restart");
        }
        assert_eq!(records(&path).len(), 2);
        {
            let mut sink = FileSink::open(path.clone(), 4096, 5).unwrap();
            append(&mut sink, time(2026, 10, 30, 0), "next day");
        }
        assert_eq!(records(&dir.0.join("events-20261029.log")).len(), 2);
        assert_eq!(records(&path).len(), 1);
    }
    #[test]
    fn size_and_daily_collisions_never_overwrite_prior_records() {
        let dir = TempDir::new();
        let mut sink = FileSink::open(dir.0.join("events.log"), 160, 10).unwrap();
        for message in ["first", "second", "third"] {
            append(&mut sink, time(2026, 10, 29, 23), message);
        }
        append(&mut sink, time(2026, 10, 30, 0), "fourth");
        drop(sink);
        assert!(records(&dir.0.join("events-20261029.log"))[0].ends_with("first"));
        assert!(records(&dir.0.join("events-20261029.001.log"))[0].ends_with("second"));
        assert!(records(&dir.0.join("events-20261029.002.log"))[0].ends_with("third"));
        assert!(records(&dir.0.join("events-latest.log"))[0].ends_with("fourth"));
    }
    #[test]
    fn retention_keeps_latest_archives_even_when_canonical_is_removed() {
        let dir = TempDir::new();
        let path = dir.0.join("events.log");
        {
            let mut sink = FileSink::open(path, 160, 2).unwrap();
            for n in 0..30 {
                append(&mut sink, time(2026, 10, 29, 12), &format!("record {n:02}"));
            }
        }
        let paths = LogPaths::new(&dir.0.join("events.log")).unwrap();
        let archives = paths.archives().unwrap();
        assert_eq!(archives.len(), 2);
        assert!(records(&archives[0].2)[0].ends_with("record 27"));
        assert!(records(&archives[1].2)[0].ends_with("record 28"));
        assert!(records(&paths.active())[0].ends_with("record 29"));
    }
    #[test]
    fn bounded_unicode_records_preserve_columns_and_size() {
        let dir = TempDir::new();
        let path = dir.0.join("events.log");
        {
            let mut sink = FileSink::open(path.clone(), 128, 2).unwrap();
            let now = time(2026, 10, 29, 1);
            let line = record(
                &now,
                &"服务".repeat(100),
                Level::Error,
                &"操作".repeat(100),
                &"😀".repeat(100),
            );
            sink.append(Day::from_system(&now), &line).unwrap();
            assert!(sink.bytes <= 128);
        }
        let rows = records(&LogPaths::new(&path).unwrap().active());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].matches('|').count(), 5);
        assert!(rows[0].contains("ERROR"));
    }
    #[test]
    fn concurrent_records_are_serialized_with_one_header() {
        let dir = TempDir::new();
        let path = dir.0.join("concurrent.log");
        let sink = std::sync::Arc::new(Mutex::new(
            FileSink::open(path.clone(), 1024 * 1024, 2).unwrap(),
        ));
        let mut workers = Vec::new();
        for worker in 0..8 {
            let sink = sink.clone();
            workers.push(std::thread::spawn(move || {
                for n in 0..100 {
                    append(
                        &mut sink.lock().unwrap(),
                        time(2026, 10, 29, 12),
                        &format!("worker {worker} record {n}"),
                    );
                }
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
        drop(sink);
        let rows = records(&LogPaths::new(&path).unwrap().active());
        assert_eq!(rows.len(), 800);
        assert_eq!(
            rows.iter().collect::<std::collections::HashSet<_>>().len(),
            800
        );
    }
    #[test]
    fn date_validation_handles_leap_years() {
        assert_eq!(Day::parse("2024-02-29 00:00:00"), Some(Day(20240229)));
        assert_eq!(Day::parse("2026-02-29 00:00:00"), None);
        assert_eq!(Day::parse("2026-13-01 00:00:00"), None);
    }
    #[test]
    fn restart_uses_local_file_date_when_old_record_has_no_parseable_date() {
        let dir = TempDir::new();
        let active = dir.0.join("events-latest.log");
        std::fs::write(&active, "unformatted prior record\r\n").unwrap();
        let old_day = metadata_day(&File::open(&active).unwrap()).unwrap();
        {
            let mut sink = FileSink::open(active.clone(), 4096, 5).unwrap();
            append(&mut sink, time(2099, 1, 1, 0), "formatted new record");
        }
        let archive = LogPaths::new(&active).unwrap().archive(old_day, 0);
        assert_eq!(read(&archive), "unformatted prior record\r\n");
        assert_eq!(records(&active).len(), 1);
    }
    #[test]
    fn release_writer_allows_elevated_process_to_reopen_same_active_file() {
        let dir = TempDir::new();
        let base = dir.0.join("handoff.log");
        let mut sink = FileSink::open(base.clone(), 4096, 5).unwrap();
        append(&mut sink, time(2026, 10, 29, 12), "before handoff");
        let mut local = State {
            sink: Some(sink),
            ..Default::default()
        };
        assert!(FileSink::open(base.clone(), 4096, 5).is_err());
        release_sink(&mut local).unwrap();
        assert!(local.sink.is_none());
        {
            let mut next = FileSink::open(base.clone(), 4096, 5).unwrap();
            append(&mut next, time(2026, 10, 29, 13), "after handoff");
        }
        assert_eq!(records(&LogPaths::new(&base).unwrap().active()).len(), 2);
    }
}
