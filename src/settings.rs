//! Public service configuration vocabulary and native registry encoding.
#![forbid(unsafe_code)]
use crate::windows::{registry, scm};
use anyhow::{Context, Result, ensure};
use winreg::RegKey;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    String,
    Dword,
    Multi,
    Exit,
    Hook,
    Affinity,
    Priority,
    Native,
}
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub name: &'static str,
    pub kind: Kind,
    pub default: &'static str,
    pub native: bool,
}
macro_rules! specs {($(($name:literal,$kind:ident,$default:literal,$native:literal)),*$(,)?)=>{pub const SPECS:&[Spec]=&[$(Spec{name:$name,kind:Kind::$kind,default:$default,native:$native}),*];};}
specs![
    ("Application", String, "", false),
    ("AppParameters", String, "", false),
    ("AppDirectory", String, "", false),
    ("AppExit", Exit, "Restart", false),
    ("AppEvents", Hook, "", false),
    ("AppAffinity", Affinity, "All", false),
    ("AppEnvironment", Multi, "", false),
    ("AppEnvironmentExtra", Multi, "", false),
    ("AppNoConsole", Dword, "0", false),
    ("AppPriority", Priority, "NORMAL_PRIORITY_CLASS", false),
    ("AppRestartDelay", Dword, "0", false),
    ("AppStdin", String, "", false),
    ("AppStdinShareMode", Dword, "2", false),
    ("AppStdinCreationDisposition", Dword, "3", false),
    ("AppStdinFlagsAndAttributes", Dword, "128", false),
    ("AppStdout", String, "", false),
    ("AppStdoutShareMode", Dword, "3", false),
    ("AppStdoutCreationDisposition", Dword, "4", false),
    ("AppStdoutFlagsAndAttributes", Dword, "128", false),
    ("AppStdoutCopyAndTruncate", Dword, "0", false),
    ("AppStderr", String, "", false),
    ("AppStderrShareMode", Dword, "3", false),
    ("AppStderrCreationDisposition", Dword, "4", false),
    ("AppStderrFlagsAndAttributes", Dword, "128", false),
    ("AppStderrCopyAndTruncate", Dword, "0", false),
    ("AppStopMethodSkip", Dword, "0", false),
    ("AppStopMethodConsole", Dword, "1500", false),
    ("AppStopMethodWindow", Dword, "1500", false),
    ("AppStopMethodThreads", Dword, "1500", false),
    ("AppKillProcessTree", Dword, "1", false),
    ("AppThrottle", Dword, "1500", false),
    ("AppRedirectHook", Dword, "0", false),
    ("AppRotateFiles", Dword, "0", false),
    ("AppRotateOnline", Dword, "0", false),
    ("AppRotateSeconds", Dword, "0", false),
    ("AppRotateBytes", Dword, "0", false),
    ("AppRotateBytesHigh", Dword, "0", false),
    ("AppRotateDelay", Dword, "0", false),
    ("AppTimestampLog", Dword, "0", false),
    ("XsmLogLevel", String, "info", false),
    ("XsmLogPath", String, "", false),
    ("XsmLogMaxBytes", Dword, "10485760", false),
    ("XsmLogMaxFiles", Dword, "5", false),
    ("DependOnGroup", Native, "", true),
    ("DependOnService", Native, "", true),
    ("Description", Native, "", true),
    ("DisplayName", Native, "", true),
    ("Environment", Multi, "", true),
    ("ImagePath", Native, "", true),
    ("ObjectName", Native, "LocalSystem", true),
    ("Name", Native, "", true),
    ("Start", Native, "", true),
    ("Type", Native, "", true),
];
pub const HOOKS: &[&str] = &[
    "Start/Pre",
    "Start/Post",
    "Stop/Pre",
    "Exit/Post",
    "Power/Change",
    "Power/Resume",
    "Rotate/Pre",
    "Rotate/Post",
];
pub const PRIORITIES: &[(&str, u32)] = &[
    ("REALTIME_PRIORITY_CLASS", 0x100),
    ("HIGH_PRIORITY_CLASS", 0x80),
    ("ABOVE_NORMAL_PRIORITY_CLASS", 0x8000),
    ("NORMAL_PRIORITY_CLASS", 0x20),
    ("BELOW_NORMAL_PRIORITY_CLASS", 0x4000),
    ("IDLE_PRIORITY_CLASS", 0x40),
];
pub fn find(name: &str) -> Result<&'static Spec> {
    SPECS
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(name))
        .with_context(|| format!("Invalid parameter {name}"))
}
pub fn parse_number(value: &str) -> Result<u32> {
    let value = value.trim_start();
    let negative = value.starts_with('-');
    let digits = value.strip_prefix(['+', '-']).unwrap_or(value);
    let parsed = if let Some(hex) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        u32::from_str_radix(hex, 16)
    } else if digits.len() > 1 && digits.starts_with('0') {
        u32::from_str_radix(&digits[1..], 8)
    } else {
        digits.parse()
    };
    let number = parsed.with_context(|| format!("Invalid DWORD: {value}"))?;
    Ok(if negative {
        number.wrapping_neg()
    } else {
        number
    })
}
pub fn parse_affinity(value: &str) -> Result<u64> {
    if value.eq_ignore_ascii_case("All") || is_default(value) {
        return Ok(0);
    }
    let mut mask = 0u64;
    for part in value.split(',') {
        let (first, last) = if let Some((a, b)) = part.split_once('-') {
            (parse_number(a)?, parse_number(b)?)
        } else {
            let n = parse_number(part)?;
            (n, n)
        };
        ensure!(
            first <= last && last < 64,
            "Invalid affinity range {part}; indices must be 0..63"
        );
        for cpu in first..=last {
            mask |= 1u64 << cpu;
        }
    }
    Ok(mask)
}
pub fn format_affinity(mask: u64) -> String {
    if mask == 0 {
        return "All".into();
    }
    let mut parts = Vec::new();
    let mut i = 0;
    while i < 64 {
        if mask & (1u64 << i) == 0 {
            i += 1;
            continue;
        }
        let first = i;
        while i < 63 && mask & (1u64 << (i + 1)) != 0 {
            i += 1;
        }
        parts.push(if first == i {
            first.to_string()
        } else {
            format!("{first}-{i}")
        });
        i += 1;
    }
    parts.join(",")
}
fn is_default(value: &str) -> bool {
    value.is_empty() || value == "*" || value.eq_ignore_ascii_case("Default")
}
fn exit_value(value: &str) -> Result<String> {
    if is_default(value) {
        Ok(String::new())
    } else {
        Ok(parse_number(value)?.to_string())
    }
}
fn hook(value: &str) -> Result<(&'static str, &'static str)> {
    let canonical=HOOKS.iter().find(|s|s.eq_ignore_ascii_case(value)).context("Invalid hook; use Start/Pre, Start/Post, Stop/Pre, Exit/Post, Power/Change, Power/Resume, Rotate/Pre or Rotate/Post")?;
    Ok(canonical.split_once('/').unwrap())
}
fn optional_key(key: &RegKey, path: &str) -> Result<Option<RegKey>> {
    match key.open_subkey(path) {
        Ok(k) => Ok(Some(k)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Apply the documented :, +, - list grammar. Environment updates replace keys
/// case-insensitively; removals with KEY=VALUE require the specified value.
pub fn update_list(old: &[String], input: &str, environment: bool) -> Result<Vec<String>> {
    ensure!(!input.contains('\0'), "NUL is not valid in list values");
    let (operation, text) = match input.chars().next() {
        Some('+') => (1, &input[1..]),
        Some('-') => (-1, &input[1..]),
        Some(':') => (0, &input[1..]),
        _ => (0, input),
    };
    let items = text
        .split(['\r', '\n'])
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut result = if operation == 0 {
        Vec::new()
    } else {
        old.to_vec()
    };
    for item in items {
        let key = item.split_once('=').map_or(item.as_str(), |(k, _)| k);
        ensure!(!key.is_empty(), "Empty environment/dependency name");
        if environment && operation >= 0 {
            ensure!(
                item.contains('='),
                "Environment entry must be NAME=VALUE: {item}"
            );
        }
        let matches = |existing: &String| {
            if !environment {
                return registry::equal_name(
                    existing.trim_start_matches('+'),
                    item.trim_start_matches('+'),
                );
            }
            if operation < 0 && item.contains('=') {
                return registry::equal_name(existing, &item);
            }
            existing
                .split_once('=')
                .is_some_and(|(k, _)| registry::equal_name(k, key))
        };
        if operation < 0 {
            result.retain(|entry| !matches(entry));
        } else {
            if let Some(index) = result.iter().position(&matches) {
                // Collapse duplicate keys as well as replacing the first one;
                // otherwise later stale duplicates override the requested value.
                let mut seen = false;
                result.retain(|entry| {
                    if matches(entry) {
                        if seen {
                            false
                        } else {
                            seen = true;
                            true
                        }
                    } else {
                        true
                    }
                });
                result[index] = item;
            } else {
                result.push(item);
            }
        }
    }
    Ok(result)
}

pub fn get(service: &str, name: &str, additional: &[String]) -> Result<String> {
    let spec = find(name)?;
    let name = spec.name;
    let service = scm::resolve_name(service)?;
    if spec.kind == Kind::Native {
        return scm::get_native(&service, name);
    }
    if !spec.native {
        scm::ensure_managed(&service)?;
    }
    let key = if spec.native {
        registry::service_key(&service, false)?
    } else {
        registry::parameters(&service, false)?
    };
    match spec.kind {
        Kind::String => Ok(registry::string(&key, name)?.unwrap_or_else(|| spec.default.into())),
        Kind::Dword => Ok(registry::dword(&key, name)?
            .unwrap_or(spec.default.parse()?)
            .to_string()),
        Kind::Priority => {
            let v = registry::dword(&key, name)?.unwrap_or(0x20);
            Ok(PRIORITIES
                .iter()
                .find(|(_, n)| *n == v)
                .map_or("NORMAL_PRIORITY_CLASS", |(n, _)| n)
                .into())
        }
        Kind::Affinity => {
            let value = registry::string(&key, name)?.unwrap_or_else(|| "All".into());
            Ok(format_affinity(parse_affinity(&value)?))
        }
        Kind::Multi => {
            let values = registry::multi(&key, name)?.unwrap_or_default();
            if let Some(wanted) = additional.first() {
                Ok(values
                    .into_iter()
                    .find_map(|v| {
                        v.split_once('=')
                            .filter(|(k, _)| registry::equal_name(k, wanted))
                            .map(|(_, v)| v.to_owned())
                    })
                    .unwrap_or_default())
            } else {
                Ok(values.join("\r\n"))
            }
        }
        Kind::Exit => {
            let input = additional
                .first()
                .context("AppExit requires an exit code or Default")?;
            let code = if is_default(input) {
                String::new()
            } else {
                parse_number(input)?.to_string()
            };
            let Some(exit) = optional_key(&key, "AppExit")? else {
                return Ok("Restart".into());
            };
            Ok(registry::string(&exit, &code)?
                .or(registry::string(&exit, "")?)
                .unwrap_or_else(|| "Restart".into()))
        }
        Kind::Hook => {
            let (event, action) = hook(
                additional
                    .first()
                    .context("AppEvents requires Event/Action")?,
            )?;
            let Some(sub) = optional_key(&key, &format!("AppEvents\\{event}"))? else {
                return Ok(String::new());
            };
            registry::expand(&registry::string(&sub, action)?.unwrap_or_default())
        }
        Kind::Native => unreachable!(),
    }
}
pub fn set(service: &str, name: &str, values: &[String]) -> Result<()> {
    let spec = find(name)?;
    ensure!(!values.is_empty(), "{name} requires a value");
    for value in values {
        ensure!(!value.contains('\0'), "NUL is not valid in settings");
    }
    let service = scm::resolve_name(service)?;
    let name = spec.name;
    if spec.kind == Kind::Native {
        return scm::set_native(&service, name, values, false);
    }
    if spec.native {
        scm::ensure_editable(&service)?;
    } else {
        scm::ensure_managed(&service)?;
    }
    let key = if spec.native {
        registry::service_key(&service, true)?
    } else {
        registry::parameters(&service, true)?
    };
    let value = values.join(" ");
    match spec.kind {
        Kind::String if name == "XsmLogLevel" => {
            let level = value.parse::<crate::windows::diagnostics::Level>()?;
            registry::set_string(&key, name, &level.to_string(), false)
        }
        Kind::String => registry::set_string(&key, name, &value, true),
        Kind::Dword => {
            let number = parse_number(&value)?;
            if name == "XsmLogMaxBytes" {
                ensure!(number >= 128, "XsmLogMaxBytes must be at least 128");
            }
            if name == "XsmLogMaxFiles" {
                ensure!((1..=100).contains(&number), "XsmLogMaxFiles must be 1..100");
            }
            if spec.default != "0" && number == parse_number(spec.default)? {
                registry::delete(&key, name)
            } else {
                key.set_value(name, &number)?;
                Ok(())
            }
        }
        Kind::Priority => {
            let (priority, number) = PRIORITIES
                .iter()
                .find(|(s, _)| s.eq_ignore_ascii_case(&value))
                .context("Invalid priority class")?;
            if *priority == spec.default {
                registry::delete(&key, name)
            } else {
                key.set_value(name, number)?;
                Ok(())
            }
        }
        Kind::Affinity => {
            let mask = parse_affinity(&value)?;
            if mask == 0 {
                registry::delete(&key, name)
            } else {
                registry::set_string(&key, name, &format_affinity(mask), false)
            }
        }
        Kind::Multi => {
            let old = registry::multi(&key, name)?.unwrap_or_default();
            let updated = update_list(&old, &values.join("\r\n"), true)?;
            if updated.is_empty() {
                registry::delete(&key, name)
            } else {
                registry::set_multi(&key, name, &updated)
            }
        }
        Kind::Exit => {
            ensure!(
                values.len() >= 2,
                "AppExit requires an exit code and action"
            );
            let code = exit_value(&values[0])?;
            let action = values[1..].join(" ");
            let canonical = ["Restart", "Ignore", "Exit", "Suicide"]
                .into_iter()
                .find(|a| a.eq_ignore_ascii_case(&action))
                .context("Invalid exit action")?;
            registry::set_string(&key.create_subkey("AppExit")?.0, &code, canonical, false)
        }
        Kind::Hook => {
            ensure!(
                values.len() >= 2,
                "AppEvents requires Event/Action and command"
            );
            let (event, action) = hook(&values[0])?;
            let sub = key.create_subkey(format!("AppEvents\\{event}"))?.0;
            let command = values[1..].join(" ");
            if command.is_empty() {
                registry::delete(&sub, action)
            } else {
                registry::set_string(&sub, action, &command, true)
            }
        }
        Kind::Native => unreachable!(),
    }
}
pub fn reset(service: &str, name: &str, additional: &[String]) -> Result<()> {
    let spec = find(name)?;
    let service = scm::resolve_name(service)?;
    let name = spec.name;
    if spec.kind == Kind::Native {
        return scm::set_native(&service, name, &[], true);
    }
    if spec.native {
        scm::ensure_editable(&service)?;
    } else {
        scm::ensure_managed(&service)?;
    }
    let key = if spec.native {
        registry::service_key(&service, true)?
    } else {
        registry::parameters(&service, true)?
    };
    match spec.kind {
        Kind::Exit => {
            let code = exit_value(
                additional
                    .first()
                    .context("AppExit requires an exit code or Default")?,
            )?;
            let sub = key.create_subkey("AppExit")?.0;
            if code.is_empty() {
                registry::set_string(&sub, "", "Restart", false)
            } else {
                registry::delete(&sub, &code)
            }
        }
        Kind::Hook => {
            let (event, action) = hook(
                additional
                    .first()
                    .context("AppEvents requires Event/Action")?,
            )?;
            let sub = key.create_subkey(format!("AppEvents\\{event}"))?.0;
            registry::delete(&sub, action)
        }
        Kind::String if ["Application", "AppDirectory", "AppParameters"].contains(&name) => {
            registry::set_string(&key, name, "", true)
        }
        _ => registry::delete(&key, name),
    }
}
pub fn dump(service: &str, rename: Option<&str>) -> Result<String> {
    use crate::quoting::batch_command;
    let source = scm::resolve_name(service)?;
    let target = rename.unwrap_or(&source);
    registry::validate_service_name(target)?;
    let executable = std::env::current_exe()?.to_string_lossy().into_owned();
    let parameters = match registry::parameters(&source, false) {
        Ok(k) => Some(k),
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            None
        }
        Err(e) => return Err(e),
    };
    let parameters = if scm::query_config(&source)?.service_type
        & windows_sys::Win32::System::Services::SERVICE_WIN32_OWN_PROCESS
        != 0
    {
        match parameters {
            Some(key) if registry::string(&key, "Application")?.is_some() => Some(key),
            _ => None,
        }
    } else {
        None
    };
    ensure!(
        parameters.is_some() || rename.is_none(),
        "Renaming a dump requires a managed application service; native dumps configure an existing service"
    );
    let mut output =
        String::from("@echo off\r\nsetlocal DisableDelayedExpansion\r\nchcp 65001 >nul\r\n");
    if let Some(key) = &parameters {
        let app = registry::string(key, "Application")?.context("Application is missing")?;
        ensure!(
            !app.is_empty(),
            "Configure Application before exporting an installable service"
        );
        output.push_str(&batch_command(
            &executable,
            &["install".into(), target.into(), app],
        ));
        output.push_str("\r\n");
    }
    let mut emit = |setting: &str, args: &[String]| {
        let mut arguments = vec!["set".into(), target.into(), setting.into()];
        arguments.extend_from_slice(args);
        output.push_str(&batch_command(&executable, &arguments));
        output.push_str("\r\n");
    };
    for spec in SPECS {
        if ["Application", "ImagePath", "Name"].contains(&spec.name) {
            continue;
        }
        if !spec.native && parameters.is_none() {
            continue;
        }
        match spec.kind {
            Kind::Exit => {
                if let Some(sub) = optional_key(parameters.as_ref().unwrap(), "AppExit")? {
                    for entry in sub.enum_values() {
                        let (code, _) = entry?;
                        let action =
                            registry::string(&sub, &code)?.context("Missing exit action")?;
                        if code.is_empty() && action.eq_ignore_ascii_case("Restart") {
                            continue;
                        }
                        let key = if code.is_empty() {
                            "Default".into()
                        } else {
                            code
                        };
                        emit(spec.name, &[key, action]);
                    }
                }
            }
            Kind::Hook => {
                for name in HOOKS {
                    let value = get(&source, spec.name, &[(*name).into()])?;
                    if !value.is_empty() {
                        emit(spec.name, &[(*name).into(), value]);
                    }
                }
            }
            Kind::Multi => {
                let value = get(&source, spec.name, &[])?;
                for (index, line) in value.split("\r\n").filter(|v| !v.is_empty()).enumerate() {
                    emit(
                        spec.name,
                        &[format!("{}{line}", if index == 0 { ":" } else { "+" })],
                    );
                }
            }
            Kind::Native if spec.name.starts_with("DependOn") => {
                let value = get(&source, spec.name, &[])?;
                for (index, line) in value.split("\r\n").filter(|v| !v.is_empty()).enumerate() {
                    emit(
                        spec.name,
                        &[format!("{}{line}", if index == 0 { ":" } else { "+" })],
                    );
                }
            }
            Kind::Native if spec.name == "ObjectName" => {
                let value = get(&source, spec.name, &[])?;
                if value.to_ascii_lowercase().starts_with("nt service\\") {
                    emit(spec.name, &[format!("NT SERVICE\\{target}")]);
                } else if [
                    "LocalSystem",
                    "NT AUTHORITY\\LocalService",
                    "NT AUTHORITY\\NetworkService",
                ]
                .iter()
                .any(|s| s.eq_ignore_ascii_case(&value))
                {
                    emit(spec.name, &[value]);
                } else {
                    emit(spec.name, &[value, "****".into()]);
                }
            }
            Kind::Native if spec.name == "DisplayName" => {
                let value = get(&source, spec.name, &[])?;
                // The default display name follows a renamed installation. An
                // explicit duplicate display name would make SCM replay fail.
                if value != source {
                    emit(spec.name, &[value]);
                }
            }
            _ => {
                let value = get(&source, spec.name, &[])?;
                if value != spec.default {
                    emit(spec.name, &[value]);
                }
            }
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_vocabulary_is_complete() {
        assert_eq!(SPECS.len(), 53);
        let mut names = std::collections::HashSet::new();
        for spec in SPECS {
            assert!(names.insert(spec.name));
        }
    }
    #[test]
    fn numeric_grammar() {
        assert_eq!(parse_number("0xffffffff").unwrap(), u32::MAX);
        assert_eq!(parse_number("010").unwrap(), 8);
        assert_eq!(parse_number("-1").unwrap(), u32::MAX);
        for s in ["09", "4294967296", "12bad", "1 ", ""] {
            assert!(parse_number(s).is_err(), "{s}");
        }
    }
    #[test]
    fn affinity_canonicalization() {
        assert_eq!(
            format_affinity(parse_affinity("0x1-03, 0x4").unwrap()),
            "1-4"
        );
        assert_eq!(exit_value("0x2a").unwrap(), "42");
        assert_eq!(
            format_affinity(parse_affinity("5,0-2,1,63").unwrap()),
            "0-2,5,63"
        );
        for s in ["64", "3-1", "1,", "-1", "1-999999"] {
            assert!(parse_affinity(s).is_err());
        }
        assert_eq!(format_affinity(u64::MAX), "0-63");
    }
    #[test]
    fn environment_update_preserves_order() {
        assert_eq!(
            update_list(
                &["A=one".into(), "a=two".into(), "B=x".into()],
                "+A=three",
                true
            )
            .unwrap(),
            ["A=three", "B=x"]
        );
        assert_eq!(update_list(&[], "A=one\r\na=two", true).unwrap(), ["a=two"]);
        assert_eq!(
            update_list(&["ÄPATH=one".into()], "+äpath=two", true).unwrap(),
            ["äpath=two"]
        );
        let old = vec!["PATH=a".into(), "OTHER=b".into()];
        assert_eq!(
            update_list(&old, "+Path=c", true).unwrap(),
            ["Path=c", "OTHER=b"]
        );
        assert_eq!(update_list(&old, "-PATH=wrong", true).unwrap(), old);
        assert_eq!(update_list(&old, "-path", true).unwrap(), ["OTHER=b"]);
        assert_eq!(
            update_list(&old, ":+A=x\r\nB=y", true).unwrap(),
            ["+A=x", "B=y"]
        );
        assert!(update_list(&old, "+INVALID", true).is_err());
    }
    #[test]
    fn hook_pairs() {
        assert_eq!(hook("start/pre").unwrap(), ("Start", "Pre"));
        assert!(hook("Stop/Post").is_err());
        assert!(hook("Start/../../").is_err());
    }
    #[test]
    fn dependency_operations() {
        let old = vec!["RpcSs".into(), "Tcpip".into()];
        assert_eq!(update_list(&old, "-rpcss", false).unwrap(), ["Tcpip"]);
        assert_eq!(
            update_list(&old, "+RPCSS", false).unwrap(),
            ["RPCSS", "Tcpip"]
        );
        assert_eq!(update_list(&old, ":", false).unwrap(), Vec::<String>::new());
    }
}
