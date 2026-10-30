//! Windows Service Control Manager boundary. All owned SCM handles close on drop.
use super::registry;
use anyhow::{Context, Result, bail, ensure};
use std::{
    io,
    mem::size_of,
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authentication::Identity::*, *},
    Storage::FileSystem::DELETE,
    System::{
        Diagnostics::ToolHelp::*, Services::*, SystemServices::SERVICE_INTERACTIVE_PROCESS,
        Threading::*,
    },
};

struct ServiceHandle(SC_HANDLE);
struct SecretString(String);
impl Drop for SecretString {
    fn drop(&mut self) {
        crate::secret::wipe_string(&mut self.0);
    }
}
impl Drop for ServiceHandle {
    fn drop(&mut self) {
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}
fn checked_handle(handle: SC_HANDLE) -> Result<ServiceHandle> {
    if handle.is_null() {
        Err(io::Error::last_os_error().into())
    } else {
        Ok(ServiceHandle(handle))
    }
}
fn manager(access: u32) -> Result<ServiceHandle> {
    unsafe { checked_handle(OpenSCManagerW(null(), null(), access)) }
        .context("Opening Service Control Manager")
}
fn wide(value: &str) -> Result<Vec<u16>> {
    ensure!(!value.contains('\0'), "String contains NUL");
    Ok(value.encode_utf16().chain([0]).collect())
}
fn check(ok: i32) -> Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error().into())
    } else {
        Ok(())
    }
}

pub fn resolve_name(name: &str) -> Result<String> {
    let scm = manager(SC_MANAGER_CONNECT)?;
    let w = wide(name)?;
    let mut display = vec![0u16; 257];
    let mut len = display.len() as u32;
    // Resolving a name should not require extra service rights. The actual
    // operation opens the service with its own minimal access mask afterward.
    if unsafe { GetServiceDisplayNameW(scm.0, w.as_ptr(), display.as_mut_ptr(), &mut len) } != 0 {
        let mut canonical = vec![0u16; 257];
        len = canonical.len() as u32;
        check(unsafe {
            GetServiceKeyNameW(scm.0, display.as_ptr(), canonical.as_mut_ptr(), &mut len)
        })?;
        return Ok(String::from_utf16(&canonical[..len as usize])?);
    }
    let first_error = io::Error::last_os_error();
    if first_error.raw_os_error() != Some(ERROR_SERVICE_DOES_NOT_EXIST as i32) {
        return Err(first_error.into());
    }
    let mut len = 257;
    let mut result = vec![0u16; len as usize];
    check(unsafe { GetServiceKeyNameW(scm.0, w.as_ptr(), result.as_mut_ptr(), &mut len) })?;
    Ok(String::from_utf16(&result[..len as usize])?)
}
fn open(name: &str, access: u32) -> Result<ServiceHandle> {
    let scm = manager(SC_MANAGER_CONNECT)?;
    let name = wide(&resolve_name(name)?)?;
    unsafe { checked_handle(OpenServiceW(scm.0, name.as_ptr(), access)) }.context("Opening service")
}

#[derive(Debug)]
pub struct Config {
    pub service_type: u32,
    pub start: u32,
    pub image: String,
    pub display: String,
    pub account: String,
    pub dependencies: Vec<String>,
    pub description: String,
    pub delayed: bool,
}
// Pointer fields returned by SCM point into the caller's buffer. Bound every read
// to that allocation before building a Rust slice.
fn buffered_string(ptr: *const u16, buffer: &[usize]) -> Result<String> {
    if ptr.is_null() {
        return Ok(String::new());
    }
    let start = buffer.as_ptr() as usize;
    let end = start + std::mem::size_of_val(buffer);
    let addr = ptr as usize;
    ensure!(
        addr >= start && addr < end && addr.is_multiple_of(2),
        "SCM returned an invalid string pointer"
    );
    let units = unsafe { std::slice::from_raw_parts(ptr, (end - addr) / 2) };
    let len = units
        .iter()
        .position(|&v| v == 0)
        .context("SCM string lacks terminator")?;
    Ok(String::from_utf16(&units[..len])?)
}
fn config_buffer(handle: &ServiceHandle, level: Option<u32>) -> Result<Vec<usize>> {
    let mut needed = 0;
    unsafe {
        if let Some(level) = level {
            QueryServiceConfig2W(handle.0, level, null_mut(), 0, &mut needed);
        } else {
            QueryServiceConfigW(handle.0, null_mut(), 0, &mut needed);
        }
    }
    let minimum = match level {
        None => size_of::<QUERY_SERVICE_CONFIGW>(),
        Some(SERVICE_CONFIG_DESCRIPTION) => size_of::<SERVICE_DESCRIPTIONW>(),
        Some(SERVICE_CONFIG_DELAYED_AUTO_START_INFO) => {
            size_of::<SERVICE_DELAYED_AUTO_START_INFO>()
        }
        _ => 1,
    };
    ensure!(
        needed as usize >= minimum && needed <= 1024 * 1024,
        "Invalid SCM configuration buffer size: {needed}"
    );
    // usize alignment satisfies every SCM structure used here.
    let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    check(unsafe {
        if let Some(level) = level {
            QueryServiceConfig2W(
                handle.0,
                level,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        } else {
            QueryServiceConfigW(handle.0, buffer.as_mut_ptr().cast(), needed, &mut needed)
        }
    })?;
    Ok(buffer)
}
pub fn query_config(name: &str) -> Result<Config> {
    let service = open(name, SERVICE_QUERY_CONFIG)?;
    let b = config_buffer(&service, None)?;
    let c = unsafe { &*b.as_ptr().cast::<QUERY_SERVICE_CONFIGW>() };
    let mut dependencies = Vec::new();
    let mut ptr = c.lpDependencies;
    if !ptr.is_null() {
        loop {
            let item = buffered_string(ptr, &b)?;
            if item.is_empty() {
                break;
            }
            ptr = unsafe { ptr.add(item.encode_utf16().count() + 1) };
            dependencies.push(item);
        }
    }
    let description_buffer = config_buffer(&service, Some(SERVICE_CONFIG_DESCRIPTION))?;
    let description = buffered_string(
        unsafe { (*description_buffer.as_ptr().cast::<SERVICE_DESCRIPTIONW>()).lpDescription },
        &description_buffer,
    )?;
    let delayed_buffer = config_buffer(&service, Some(SERVICE_CONFIG_DELAYED_AUTO_START_INFO))?;
    let delayed = unsafe {
        (*delayed_buffer
            .as_ptr()
            .cast::<SERVICE_DELAYED_AUTO_START_INFO>())
        .fDelayedAutostart
            != 0
    };
    Ok(Config {
        service_type: c.dwServiceType,
        start: c.dwStartType,
        image: buffered_string(c.lpBinaryPathName, &b)?,
        display: buffered_string(c.lpDisplayName, &b)?,
        account: buffered_string(c.lpServiceStartName, &b)?,
        dependencies,
        description,
        delayed,
    })
}
pub fn ensure_editable(name: &str) -> Result<()> {
    ensure!(
        query_config(name)?.service_type & SERVICE_WIN32_OWN_PROCESS != 0,
        "Only services running in their own process can be edited"
    );
    Ok(())
}
pub fn ensure_managed(name: &str) -> Result<()> {
    ensure_editable(name)?;
    let key = registry::parameters(name, false)
        .context("This service has no application configuration")?;
    ensure!(
        registry::string(&key, "Application")?.is_some(),
        "Application settings are only available for a managed service"
    );
    Ok(())
}
pub fn install(name: &str, application: &str, parameters: &str) -> Result<()> {
    registry::validate_service_name(name)?;
    ensure!(!application.is_empty(), "Application is required");
    let scm = manager(SC_MANAGER_CREATE_SERVICE)?;
    let namew = wide(name)?;
    let binary = wide(&crate::quoting::quote_arg(
        &std::env::current_exe()?.to_string_lossy(),
    ))?;
    let service = unsafe {
        checked_handle(CreateServiceW(
            scm.0,
            namew.as_ptr(),
            namew.as_ptr(),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            binary.as_ptr(),
            null(),
            null_mut(),
            null(),
            null(),
            null(),
        ))
    }
    .context("Creating service")?;
    let result = (|| -> Result<()> {
        let key = registry::parameters(name, true)?;
        registry::set_string(&key, "Application", application, true)?;
        registry::set_string(&key, "AppParameters", parameters, true)?;
        let directory = std::path::Path::new(application)
            .parent()
            .map(|v| v.to_string_lossy().into_owned())
            .filter(|v| !v.is_empty())
            .unwrap_or_default();
        registry::set_string(&key, "AppDirectory", &directory, true)?;
        let (exit, _) = key.create_subkey("AppExit")?;
        registry::set_string(&exit, "", "Restart", false)?;
        let flag = SERVICE_FAILURE_ACTIONS_FLAG {
            fFailureActionsOnNonCrashFailures: 1,
        };
        check(unsafe {
            ChangeServiceConfig2W(
                service.0,
                SERVICE_CONFIG_FAILURE_ACTIONS_FLAG,
                (&flag as *const SERVICE_FAILURE_ACTIONS_FLAG).cast(),
            )
        })?;
        super::diagnostics::register_source()?;
        Ok(())
    })();
    if let Err(error) = result {
        check(unsafe { DeleteService(service.0) })
            .context("Failed to roll back partially installed service")?;
        return Err(error);
    }
    Ok(())
}
pub fn remove(name: &str) -> Result<()> {
    let service = open(name, DELETE)?;
    check(unsafe { DeleteService(service.0) }).context("Deleting service")
}
/// Enable configured SCM failure actions for a nonzero, orderly service exit.
/// This does not change the administrator's action list or reset period.
pub fn enable_recovery(name: &str) -> Result<()> {
    let service = open(name, SERVICE_CHANGE_CONFIG)?;
    let flag = SERVICE_FAILURE_ACTIONS_FLAG {
        fFailureActionsOnNonCrashFailures: 1,
    };
    check(unsafe {
        ChangeServiceConfig2W(
            service.0,
            SERVICE_CONFIG_FAILURE_ACTIONS_FLAG,
            (&flag as *const SERVICE_FAILURE_ACTIONS_FLAG).cast(),
        )
    })
}
fn query_status(service: &ServiceHandle) -> Result<SERVICE_STATUS_PROCESS> {
    let mut status = SERVICE_STATUS_PROCESS::default();
    let mut needed = 0;
    check(unsafe {
        QueryServiceStatusEx(
            service.0,
            SC_STATUS_PROCESS_INFO,
            (&mut status as *mut SERVICE_STATUS_PROCESS).cast(),
            size_of::<SERVICE_STATUS_PROCESS>() as u32,
            &mut needed,
        )
    })?;
    Ok(status)
}
pub fn status(name: &str) -> Result<u32> {
    Ok(query_status(&open(name, SERVICE_QUERY_STATUS)?)?.dwCurrentState)
}
pub fn status_name(state: u32) -> &'static str {
    match state {
        SERVICE_STOPPED => "SERVICE_STOPPED",
        SERVICE_START_PENDING => "SERVICE_START_PENDING",
        SERVICE_STOP_PENDING => "SERVICE_STOP_PENDING",
        SERVICE_RUNNING => "SERVICE_RUNNING",
        SERVICE_CONTINUE_PENDING => "SERVICE_CONTINUE_PENDING",
        SERVICE_PAUSE_PENDING => "SERVICE_PAUSE_PENDING",
        SERVICE_PAUSED => "SERVICE_PAUSED",
        _ => "SERVICE_UNKNOWN",
    }
}
pub fn control(name: &str, verb: &str, start_args: &[String]) -> Result<u32> {
    if verb.eq_ignore_ascii_case("restart") {
        control(name, "stop", &[])?;
        return control(name, "start", start_args);
    }
    let verb = verb.to_ascii_lowercase();
    let (access, code, target) = match verb.as_str() {
        "start" => (SERVICE_START, 0, SERVICE_RUNNING),
        "stop" => (SERVICE_STOP, SERVICE_CONTROL_STOP, SERVICE_STOPPED),
        "pause" => (
            SERVICE_PAUSE_CONTINUE,
            SERVICE_CONTROL_PAUSE,
            SERVICE_PAUSED,
        ),
        "continue" => (
            SERVICE_PAUSE_CONTINUE,
            SERVICE_CONTROL_CONTINUE,
            SERVICE_RUNNING,
        ),
        "rotate" => (SERVICE_USER_DEFINED_CONTROL, 128, 0),
        _ => bail!("Unknown control {verb}"),
    };
    let service = open(name, access | SERVICE_QUERY_STATUS)?;
    let result = if code == 0 {
        let args: Vec<Vec<u16>> = start_args.iter().map(|v| wide(v)).collect::<Result<_>>()?;
        let pointers: Vec<_> = args.iter().map(|v| v.as_ptr()).collect();
        unsafe { StartServiceW(service.0, pointers.len() as u32, pointers.as_ptr()) }
    } else {
        let mut status = SERVICE_STATUS::default();
        unsafe { ControlService(service.0, code, &mut status) }
    };
    if result == 0 {
        let err = io::Error::last_os_error();
        if code == SERVICE_CONTROL_STOP
            && err.raw_os_error() == Some(ERROR_SERVICE_NOT_ACTIVE as i32)
        {
            return Ok(SERVICE_STOPPED);
        }
        if err.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
            return Err(err.into());
        }
    }
    if target == 0 {
        return status(name);
    }
    let started = Instant::now();
    let mut last_checkpoint = 0;
    let mut progress = Instant::now();
    loop {
        let state = query_status(&service)?;
        if state.dwCurrentState == target {
            return Ok(target);
        }
        if code == 0 && state.dwCurrentState == SERVICE_PAUSED {
            return Ok(SERVICE_PAUSED);
        }
        if state.dwCurrentState == SERVICE_STOPPED && target != SERVICE_STOPPED {
            bail!(
                "Service stopped while starting (Win32 {}, service {})",
                state.dwWin32ExitCode,
                state.dwServiceSpecificExitCode
            );
        }
        if state.dwCheckPoint != last_checkpoint {
            last_checkpoint = state.dwCheckPoint;
            progress = Instant::now();
        }
        ensure!(
            started.elapsed() < Duration::from_secs(300)
                && progress.elapsed()
                    < Duration::from_millis(u64::from(state.dwWaitHint.max(20_000))),
            "Timed out waiting for {}: {}",
            name,
            status_name(state.dwCurrentState)
        );
        std::thread::sleep(Duration::from_millis(u64::from(
            (state.dwWaitHint / 10).clamp(100, 1000),
        )));
    }
}
pub fn list(all: bool) -> Result<Vec<String>> {
    let scm = manager(SC_MANAGER_ENUMERATE_SERVICE)?;
    let mut resume = 0;
    let mut names = Vec::new();
    loop {
        let mut b = vec![0usize; 256 * 1024 / size_of::<usize>()];
        let mut needed = 0;
        let mut count = 0;
        let ok = unsafe {
            EnumServicesStatusExW(
                scm.0,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_STATE_ALL,
                b.as_mut_ptr().cast(),
                (b.len() * size_of::<usize>()) as u32,
                &mut needed,
                &mut count,
                &mut resume,
                null(),
            )
        };
        if ok == 0 && unsafe { GetLastError() } != ERROR_MORE_DATA {
            return Err(io::Error::last_os_error().into());
        }
        ensure!(
            (count as usize) * size_of::<ENUM_SERVICE_STATUS_PROCESSW>()
                <= std::mem::size_of_val(b.as_slice()),
            "Invalid service count"
        );
        let entries = unsafe {
            std::slice::from_raw_parts(
                b.as_ptr().cast::<ENUM_SERVICE_STATUS_PROCESSW>(),
                count as usize,
            )
        };
        for e in entries {
            let name = buffered_string(e.lpServiceName, &b)?;
            if all
                || (e.ServiceStatusProcess.dwServiceType & SERVICE_WIN32_OWN_PROCESS != 0
                    && registry::parameters(&name, false)
                        .and_then(|k| registry::string(&k, "Application"))
                        .ok()
                        .flatten()
                        .is_some())
            {
                names.push(name);
            }
        }
        if ok != 0 {
            break;
        }
        ensure!(count > 0, "Service enumeration made no progress");
    }
    Ok(names)
}
pub fn get_native(name: &str, setting: &str) -> Result<String> {
    if setting == "Name" {
        return resolve_name(name);
    }
    let c = query_config(name)?;
    Ok(match setting {
        "DisplayName" => c.display,
        "Description" => c.description,
        "ImagePath" => c.image,
        "ObjectName" => c.account,
        "DependOnGroup" => c
            .dependencies
            .into_iter()
            .filter(|s| s.starts_with('+'))
            .collect::<Vec<_>>()
            .join("\r\n"),
        "DependOnService" => c
            .dependencies
            .into_iter()
            .filter(|s| !s.starts_with('+'))
            .collect::<Vec<_>>()
            .join("\r\n"),
        "Start" => match c.start {
            SERVICE_AUTO_START if c.delayed => "SERVICE_DELAYED_AUTO_START",
            SERVICE_AUTO_START => "SERVICE_AUTO_START",
            SERVICE_DEMAND_START => "SERVICE_DEMAND_START",
            SERVICE_DISABLED => "SERVICE_DISABLED",
            SERVICE_BOOT_START => "SERVICE_BOOT_START",
            SERVICE_SYSTEM_START => "SERVICE_SYSTEM_START",
            _ => "SERVICE_UNKNOWN",
        }
        .into(),
        "Type" => match c.service_type {
            SERVICE_WIN32_OWN_PROCESS => "SERVICE_WIN32_OWN_PROCESS",
            SERVICE_WIN32_SHARE_PROCESS => "SERVICE_WIN32_SHARE_PROCESS",
            SERVICE_KERNEL_DRIVER => "SERVICE_KERNEL_DRIVER",
            SERVICE_FILE_SYSTEM_DRIVER => "SERVICE_FILE_SYSTEM_DRIVER",
            0x110 => "SERVICE_INTERACTIVE_PROCESS",
            0x120 => "SERVICE_WIN32_SHARE_PROCESS|SERVICE_INTERACTIVE_PROCESS",
            _ => "SERVICE_UNKNOWN",
        }
        .into(),
        _ => bail!("Unknown native setting {setting}"),
    })
}
fn native_text_value(setting: &str, values: &[String]) -> String {
    // Account values contain a password. Only the dedicated scrubbed buffer in
    // that branch may join them; the ordinary setting text must not copy it.
    if setting == "ObjectName" {
        String::new()
    } else {
        values.join(" ")
    }
}
pub fn set_native(name: &str, setting: &str, values: &[String], reset: bool) -> Result<()> {
    ensure_editable(name)?;
    let canonical = resolve_name(name)?;
    let service = open(&canonical, SERVICE_CHANGE_CONFIG | SERVICE_QUERY_CONFIG)?;
    let value = native_text_value(setting, values);
    let mut kind = SERVICE_NO_CHANGE;
    let mut start = SERVICE_NO_CHANGE;
    let mut binary = None;
    let mut display = None;
    let mut account = None;
    let mut password = None;
    let mut deps = None;
    match setting {
        "Name" => bail!("Name is read-only"),
        "Description" => {
            let mut text = wide(if reset { "" } else { &value })?;
            let description = SERVICE_DESCRIPTIONW {
                lpDescription: text.as_mut_ptr(),
            };
            return check(unsafe {
                ChangeServiceConfig2W(
                    service.0,
                    SERVICE_CONFIG_DESCRIPTION,
                    (&description as *const SERVICE_DESCRIPTIONW).cast(),
                )
            });
        }
        "DisplayName" => display = Some(wide(if reset { &canonical } else { &value })?),
        "ImagePath" => {
            ensure!(!reset, "ImagePath has no default");
            binary = Some(wide(&value)?);
        }
        "Start" => {
            ensure!(!reset, "Start has no default");
            start = match value.to_ascii_uppercase().as_str() {
                "SERVICE_AUTO_START" | "SERVICE_DELAYED_AUTO_START" => SERVICE_AUTO_START,
                "SERVICE_DEMAND_START" => SERVICE_DEMAND_START,
                "SERVICE_DISABLED" => SERVICE_DISABLED,
                _ => bail!("Invalid startup type {value}"),
            };
        }
        "Type" => {
            ensure!(!reset, "Type has no default");
            kind = match value.to_ascii_uppercase().as_str() {
                "SERVICE_WIN32_OWN_PROCESS" => SERVICE_WIN32_OWN_PROCESS,
                "SERVICE_INTERACTIVE_PROCESS" => {
                    SERVICE_WIN32_OWN_PROCESS | SERVICE_INTERACTIVE_PROCESS
                }
                _ => bail!("Invalid service type {value}"),
            };
            if kind & SERVICE_INTERACTIVE_PROCESS != 0 {
                ensure!(
                    get_native(name, "ObjectName")?.eq_ignore_ascii_case("LocalSystem"),
                    "Interactive service requires LocalSystem"
                );
            }
        }
        "DependOnService" | "DependOnGroup" => {
            let c = query_config(name)?;
            let group = setting == "DependOnGroup";
            let old = c
                .dependencies
                .iter()
                .filter(|s| s.starts_with('+') == group)
                .map(|s| s.trim_start_matches('+').to_owned())
                .collect::<Vec<_>>();
            let joined = values.join("\r\n");
            let updated =
                crate::settings::update_list(&old, if reset { "" } else { &joined }, false)?;
            let mut merged = c
                .dependencies
                .into_iter()
                .filter(|s| s.starts_with('+') != group)
                .collect::<Vec<_>>();
            for item in updated {
                let item = if group {
                    let groups = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
                        .open_subkey("SYSTEM\\CurrentControlSet\\Control\\ServiceGroupOrder")?;
                    let known = registry::multi(&groups, "List")?.unwrap_or_default();
                    let requested = item.trim_start_matches('+');
                    let canonical = known
                        .iter()
                        .find(|g| g.eq_ignore_ascii_case(requested))
                        .with_context(|| format!("Unknown service group {requested}"))?;
                    format!("+{canonical}")
                } else {
                    resolve_name(&item)?
                };
                ensure!(
                    !item.eq_ignore_ascii_case(&canonical),
                    "A service cannot depend on itself"
                );
                merged.push(item);
            }
            let mut w = Vec::new();
            for d in merged {
                w.extend(wide(&d)?);
            }
            w.push(0);
            if w.len() == 1 {
                w.push(0);
            }
            deps = Some(w);
        }
        "ObjectName" => {
            let requested = if reset {
                "LocalSystem"
            } else {
                values.first().context("ObjectName requires an account")?
            };
            let known = canonical_account(requested)?;
            let username = known.as_deref().unwrap_or(requested);
            let virtual_account =
                registry::equal_name(username, &format!("NT SERVICE\\{canonical}"));
            let joined_password = SecretString(values.get(1..).unwrap_or_default().join(" "));
            let pass = if known.is_some() || virtual_account {
                ""
            } else {
                ensure!(
                    values.len() >= 2,
                    "ObjectName requires a password for this account"
                );
                &joined_password.0
            };
            if known.is_none() && !virtual_account {
                grant_logon_as_service(username)?;
            }
            if !username.eq_ignore_ascii_case("LocalSystem") {
                kind = query_config(name)?.service_type & !SERVICE_INTERACTIVE_PROCESS;
            }
            account = Some(wide(username)?);
            // Virtual service accounts require a null password pointer, not an
            // empty password. Windows otherwise rejects the account with 1057.
            password = if virtual_account {
                None
            } else {
                Some(wide(pass)?)
            };
        }
        _ => bail!("Unknown native setting {setting}"),
    }
    let ptr = |v: &Option<Vec<u16>>| v.as_ref().map_or(null(), |s| s.as_ptr());
    let result = check(unsafe {
        ChangeServiceConfigW(
            service.0,
            kind,
            start,
            SERVICE_NO_CHANGE,
            ptr(&binary),
            null(),
            null_mut(),
            ptr(&deps),
            ptr(&account),
            ptr(&password),
            ptr(&display),
        )
    });
    if let Some(secret) = &mut password {
        crate::secret::wipe(secret);
    }
    result?;
    if setting == "Start" {
        let delayed = SERVICE_DELAYED_AUTO_START_INFO {
            fDelayedAutostart: i32::from(value.eq_ignore_ascii_case("SERVICE_DELAYED_AUTO_START")),
        };
        check(unsafe {
            ChangeServiceConfig2W(
                service.0,
                SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
                (&delayed as *const SERVICE_DELAYED_AUTO_START_INFO).cast(),
            )
        })?;
    }
    Ok(())
}

fn lookup_sid(name: &str) -> Result<(Vec<usize>, SID_NAME_USE)> {
    let name = wide(name)?;
    let mut size = 0;
    let mut domain_size = 0;
    let mut kind = 0;
    unsafe {
        LookupAccountNameW(
            null(),
            name.as_ptr(),
            null_mut(),
            &mut size,
            null_mut(),
            &mut domain_size,
            &mut kind,
        );
    }
    ensure!(
        size > 0,
        "Account lookup failed: {}",
        io::Error::last_os_error()
    );
    let mut sid = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    let mut domain = vec![0u16; domain_size as usize];
    check(unsafe {
        LookupAccountNameW(
            null(),
            name.as_ptr(),
            sid.as_mut_ptr().cast(),
            &mut size,
            domain.as_mut_ptr(),
            &mut domain_size,
            &mut kind,
        )
    })?;
    Ok((sid, kind))
}
fn canonical_account(name: &str) -> Result<Option<String>> {
    if name.eq_ignore_ascii_case("LocalSystem") {
        return Ok(Some("LocalSystem".into()));
    }
    let Ok((sid, _)) = lookup_sid(name) else {
        return Ok(None);
    };
    for (kind, canonical) in [
        (WinLocalSystemSid, "LocalSystem"),
        (WinLocalServiceSid, "NT Authority\\LocalService"),
        (WinNetworkServiceSid, "NT Authority\\NetworkService"),
    ] {
        if unsafe { IsWellKnownSid(sid.as_ptr() as _, kind) } != 0 {
            return Ok(Some(canonical.into()));
        }
    }
    Ok(None)
}
fn grant_logon_as_service(account: &str) -> Result<()> {
    let (sid, kind) = lookup_sid(account)?;
    ensure!(
        kind == SidTypeUser,
        "Service logon account must resolve to a user; refusing to grant rights to a group"
    );
    let attributes = LSA_OBJECT_ATTRIBUTES {
        Length: size_of::<LSA_OBJECT_ATTRIBUTES>() as u32,
        ..Default::default()
    };
    let mut handle: LSA_HANDLE = 0;
    let status = unsafe {
        LsaOpenPolicy(
            null(),
            &attributes,
            (POLICY_LOOKUP_NAMES | POLICY_CREATE_ACCOUNT) as u32,
            &mut handle,
        )
    };
    if status != 0 {
        return Err(
            io::Error::from_raw_os_error(unsafe { LsaNtStatusToWinError(status) } as i32).into(),
        );
    }
    struct Policy(LSA_HANDLE);
    impl Drop for Policy {
        fn drop(&mut self) {
            unsafe {
                LsaClose(self.0);
            }
        }
    }
    let policy = Policy(handle);
    let mut text = wide("SeServiceLogonRight")?;
    let right = LSA_UNICODE_STRING {
        Length: ((text.len() - 1) * 2) as u16,
        MaximumLength: (text.len() * 2) as u16,
        Buffer: text.as_mut_ptr(),
    };
    let status = unsafe { LsaAddAccountRights(policy.0, sid.as_ptr() as _, &right, 1) };
    if status != 0 {
        return Err(
            io::Error::from_raw_os_error(unsafe { LsaNtStatusToWinError(status) } as i32).into(),
        );
    }
    Ok(())
}

struct DebugPrivilege {
    token: Option<super::process::Handle>,
    old: TOKEN_PRIVILEGES,
    changed: bool,
    revert: bool,
}
impl DebugPrivilege {
    fn acquire() -> Self {
        let mut guard = Self {
            token: None,
            old: Default::default(),
            changed: false,
            revert: false,
        };
        let mut token = null_mut();
        if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 0, &mut token) } != 0 {
            // An existing impersonation token can be shared. Optional process
            // inspection must never change its privileges or replace its identity.
            // SAFETY: OpenThreadToken transferred this unique handle, not the token itself.
            drop(unsafe { super::process::Handle::from_raw_owned(token) });
            return guard;
        }
        if unsafe { GetLastError() } != ERROR_NO_TOKEN
            || unsafe { ImpersonateSelf(SecurityImpersonation) } == 0
        {
            return guard;
        }
        guard.revert = true;
        if unsafe {
            OpenThreadToken(
                GetCurrentThread(),
                TOKEN_QUERY | TOKEN_ADJUST_PRIVILEGES,
                0,
                &mut token,
            )
        } == 0
        {
            return guard;
        }
        // SAFETY: successful OpenThreadToken transfers a unique kernel handle.
        guard.token = unsafe { super::process::Handle::from_raw_owned(token) }.ok();
        if guard.token.is_none() {
            return guard;
        }
        let mut luid = Default::default();
        let name = wide("SeDebugPrivilege").expect("static name");
        if unsafe { LookupPrivilegeValueW(null(), name.as_ptr(), &mut luid) } == 0 {
            return guard;
        }
        let requested = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        let mut size = 0;
        if unsafe {
            AdjustTokenPrivileges(
                token,
                0,
                &requested,
                size_of::<TOKEN_PRIVILEGES>() as u32,
                &mut guard.old,
                &mut size,
            )
        } != 0
        {
            guard.changed = guard.old.PrivilegeCount != 0;
        }
        guard
    }
}
impl Drop for DebugPrivilege {
    fn drop(&mut self) {
        if self.changed
            && let Some(token) = &self.token
        {
            let restored = unsafe {
                AdjustTokenPrivileges(token.raw(), 0, &self.old, 0, null_mut(), null_mut())
            };
            let error = unsafe { GetLastError() };
            if restored == 0 || error == ERROR_NOT_ALL_ASSIGNED {
                super::diagnostics::write_operation(
                    "process",
                    super::diagnostics::Level::Error,
                    "permissions",
                    &format!(
                        "Could not restore temporary thread privileges (Windows error {error})"
                    ),
                );
            }
        }
        if self.revert && unsafe { RevertToSelf() } == 0 {
            let error = unsafe { GetLastError() };
            super::diagnostics::write_operation(
                "process",
                super::diagnostics::Level::Error,
                "permissions",
                &format!(
                    "Could not restore the original thread security context (Windows error {error})"
                ),
            );
        }
    }
}

pub fn processes(name: &str) -> Result<String> {
    let _privilege = DebugPrivilege::acquire();
    let state = query_status(&open(name, SERVICE_QUERY_STATUS)?)?;
    if state.dwProcessId == 0 {
        return Ok(String::new());
    }
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    ensure!(
        snapshot != INVALID_HANDLE_VALUE,
        "Process snapshot failed: {}",
        io::Error::last_os_error()
    );
    let mut snapshot_time = FILETIME::default();
    unsafe {
        windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime(&mut snapshot_time)
    };
    let snapshot_time =
        (u64::from(snapshot_time.dwHighDateTime) << 32) | u64::from(snapshot_time.dwLowDateTime);
    // SAFETY: the successful snapshot factory returned a unique kernel handle.
    let snapshot = unsafe { super::process::Handle::from_raw_owned(snapshot) }?;
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut entries = Vec::new();
    let mut more = unsafe { Process32FirstW(snapshot.raw(), &mut entry) } != 0;
    while more {
        let len = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        entries.push((
            entry.th32ProcessID,
            entry.th32ParentProcessID,
            String::from_utf16_lossy(&entry.szExeFile[..len]),
        ));
        more = unsafe { Process32NextW(snapshot.raw(), &mut entry) } != 0;
    }
    let mut output = String::new();
    let mut seen = std::collections::HashSet::new();
    let mut pending = vec![(state.dwProcessId, 0, 0u64)];
    while let Some((pid, depth, parent_birth)) = pending.pop() {
        if !seen.insert(pid) {
            continue;
        }
        // SAFETY: OpenProcess returns an owned kernel handle or a null failure.
        let process = unsafe {
            super::process::Handle::from_raw_owned(OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            ))
        };
        let Ok(process) = process else {
            if depth == 0 {
                bail!("Cannot inspect service process {pid}");
            }
            super::diagnostics::write(
                name,
                super::diagnostics::Level::Warn,
                &format!("Cannot inspect descendant PID {pid}; process tree is incomplete"),
            );
            output.push_str(&format!(
                "{pid:8} {}<unavailable: access denied or process exited>\n",
                " ".repeat(depth)
            ));
            continue;
        };
        let (birth, death) = super::process::process_times(&process)?;
        if death != 0 || birth < parent_birth || birth > snapshot_time {
            continue;
        }
        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
        let name = if unsafe {
            QueryFullProcessImageNameW(process.raw(), 0, buffer.as_mut_ptr(), &mut length)
        } != 0
        {
            String::from_utf16_lossy(&buffer[..length as usize])
        } else {
            "???".into()
        };
        output.push_str(&format!("{pid:8} {}{name}\n", " ".repeat(depth)));
        for e in entries.iter().rev().filter(|e| e.1 == pid) {
            pending.push((e.0, depth + 2, birth));
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn token_privileges(token: &super::super::process::Handle) -> Vec<(u32, i32, u32)> {
        let mut size = 0;
        unsafe { GetTokenInformation(token.raw(), TokenPrivileges, null_mut(), 0, &mut size) };
        assert!(size as usize >= size_of::<u32>());
        let mut buffer = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        assert_ne!(
            unsafe {
                GetTokenInformation(
                    token.raw(),
                    TokenPrivileges,
                    buffer.as_mut_ptr().cast(),
                    size,
                    &mut size,
                )
            },
            0
        );
        let header = buffer.as_ptr().cast::<TOKEN_PRIVILEGES>();
        let count = unsafe { (*header).PrivilegeCount } as usize;
        let offset = std::mem::offset_of!(TOKEN_PRIVILEGES, Privileges);
        assert!(
            count
                <= (std::mem::size_of_val(buffer.as_slice()) - offset)
                    / size_of::<LUID_AND_ATTRIBUTES>()
        );
        // SAFETY: the returned privilege array is aligned and bounded by the allocation above.
        let entries = unsafe {
            std::slice::from_raw_parts(
                std::ptr::addr_of!((*header).Privileges).cast::<LUID_AND_ATTRIBUTES>(),
                count,
            )
        };
        let mut result = entries
            .iter()
            .map(|entry| (entry.Luid.LowPart, entry.Luid.HighPart, entry.Attributes))
            .collect::<Vec<_>>();
        result.sort_unstable();
        result
    }
    #[test]
    fn ordinary_native_text_never_copies_account_password() {
        let values = vec!["domain\\account".into(), "sensitive password".into()];
        assert!(native_text_value("ObjectName", &values).is_empty());
        assert_eq!(
            native_text_value("Description", &["service".into(), "description".into()]),
            "service description"
        );
    }
    #[test]
    fn string_pointer_must_be_inside_buffer() {
        let b = vec![0usize; 4];
        assert!(buffered_string(std::ptr::dangling::<u16>(), &b).is_err());
        assert_eq!(buffered_string(b.as_ptr().cast(), &b).unwrap(), "");
    }
    #[test]
    fn state_names() {
        assert_eq!(status_name(4), "SERVICE_RUNNING");
        assert_eq!(status_name(0), "SERVICE_UNKNOWN");
    }
    #[test]
    fn debug_privilege_scope_restores_thread_token() {
        fn thread_token_present() -> bool {
            let mut handle = null_mut();
            let ok =
                unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 0, &mut handle) } != 0;
            if ok {
                // SAFETY: OpenThreadToken succeeded and transferred this handle.
                drop(unsafe { super::super::process::Handle::from_raw_owned(handle) }.unwrap());
            }
            ok
        }
        let before = thread_token_present();
        {
            let _scope = DebugPrivilege::acquire();
        }
        assert_eq!(thread_token_present(), before);
    }
    #[test]
    fn existing_impersonation_token_and_privileges_are_unchanged() {
        std::thread::spawn(|| {
            assert_ne!(unsafe { ImpersonateSelf(SecurityImpersonation) }, 0);
            struct Revert;
            impl Drop for Revert {
                fn drop(&mut self) {
                    unsafe { RevertToSelf() };
                }
            }
            let _revert = Revert;
            let mut raw = null_mut();
            assert_ne!(
                unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 0, &mut raw) },
                0
            );
            // SAFETY: OpenThreadToken transferred this owned handle.
            let token = unsafe { super::super::process::Handle::from_raw_owned(raw) }.unwrap();
            let before = token_privileges(&token);
            let statistics = |token: &super::super::process::Handle| {
                let mut statistics = TOKEN_STATISTICS::default();
                let mut size = 0;
                assert_ne!(
                    unsafe {
                        GetTokenInformation(
                            token.raw(),
                            TokenStatistics,
                            (&mut statistics as *mut TOKEN_STATISTICS).cast(),
                            size_of::<TOKEN_STATISTICS>() as u32,
                            &mut size,
                        )
                    },
                    0
                );
                (
                    statistics.TokenId.LowPart,
                    statistics.TokenId.HighPart,
                    statistics.ModifiedId.LowPart,
                    statistics.ModifiedId.HighPart,
                )
            };
            let identity = statistics(&token);
            {
                let scope = DebugPrivilege::acquire();
                assert!(scope.token.is_none() && !scope.changed && !scope.revert);
                assert_eq!(token_privileges(&token), before);
            }
            assert_ne!(
                unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 0, &mut raw) },
                0
            );
            // SAFETY: this second successful OpenThreadToken returned another owned handle.
            let current = unsafe { super::super::process::Handle::from_raw_owned(raw) }.unwrap();
            assert_eq!(statistics(&current), identity);
            assert_eq!(token_privileges(&current), before);
        })
        .join()
        .unwrap();
    }
    #[test]
    fn temporary_debug_token_restores_actual_privilege_attributes() {
        std::thread::spawn(|| {
            let mut raw = null_mut();
            assert_ne!(
                unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) },
                0
            );
            // SAFETY: the successful factory transferred an owned token handle.
            let process = unsafe { super::super::process::Handle::from_raw_owned(raw) }.unwrap();
            let before = token_privileges(&process);
            let scope = DebugPrivilege::acquire();
            assert!(scope.revert);
            let temporary = scope
                .token
                .as_ref()
                .expect("temporary thread token")
                .duplicate(false)
                .unwrap();
            let mut luid = LUID::default();
            assert_ne!(
                unsafe {
                    LookupPrivilegeValueW(
                        null(),
                        wide("SeDebugPrivilege").unwrap().as_ptr(),
                        &mut luid,
                    )
                },
                0
            );
            if before
                .iter()
                .any(|entry| entry.0 == luid.LowPart && entry.1 == luid.HighPart)
            {
                assert!(
                    token_privileges(&temporary)
                        .iter()
                        .any(|entry| entry.0 == luid.LowPart
                            && entry.1 == luid.HighPart
                            && entry.2 & SE_PRIVILEGE_ENABLED != 0)
                );
                eprintln!("Real SeDebugPrivilege enable and restore path exercised");
            }
            drop(scope);
            assert_eq!(token_privileges(&temporary), before);
            assert_eq!(token_privileges(&process), before);
            assert_eq!(
                unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 0, &mut raw) },
                0
            );
            assert_eq!(unsafe { GetLastError() }, ERROR_NO_TOKEN);
        })
        .join()
        .unwrap();
    }
}
