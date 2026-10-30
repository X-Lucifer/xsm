//! Typed access to the service registry schema. Missing values and corrupt values
//! are intentionally distinct; corruption must not silently change a service.
use anyhow::{Context, Result, bail, ensure};
use winreg::{RegKey, RegValue, enums::*};

pub fn service_key(service: &str, write: bool) -> Result<RegKey> {
    validate_service_name(service)?;
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(
            format!("SYSTEM\\CurrentControlSet\\Services\\{service}"),
            if write {
                KEY_READ | KEY_WRITE
            } else {
                KEY_READ
            },
        )
        .with_context(|| format!("Opening registry for service {service}"))
}
pub fn parameters(service: &str, write: bool) -> Result<RegKey> {
    let root = service_key(service, write)?;
    if write {
        Ok(root.create_subkey("Parameters")?.0)
    } else {
        Ok(root.open_subkey("Parameters")?)
    }
}
pub fn validate_service_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty() && name.encode_utf16().count() <= 256,
        "Service name must contain 1..256 UTF-16 code units"
    );
    ensure!(!name.contains(['\\', '/', '\0']), "Invalid service name");
    Ok(())
}
/// Match Windows' ordinal case-insensitive name comparison, including Unicode.
pub fn equal_name(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    if left.is_ascii() && right.is_ascii() {
        return left.eq_ignore_ascii_case(right);
    }
    use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
    let left: Vec<u16> = left.encode_utf16().collect();
    let right: Vec<u16> = right.encode_utf16().collect();
    let (Ok(left_len), Ok(right_len)) = (i32::try_from(left.len()), i32::try_from(right.len()))
    else {
        return false;
    };
    unsafe {
        CompareStringOrdinal(left.as_ptr(), left_len, right.as_ptr(), right_len, 1) == CSTR_EQUAL
    }
}
fn raw(key: &RegKey, name: &str) -> Result<Option<RegValue>> {
    match key.get_raw_value(name) {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("Reading registry value {name}")),
    }
}
fn units(bytes: &[u8]) -> Result<Vec<u16>> {
    ensure!(
        bytes.len().is_multiple_of(2),
        "Invalid UTF-16 registry data length"
    );
    Ok(bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect())
}
pub fn string(key: &RegKey, name: &str) -> Result<Option<String>> {
    let Some(v) = raw(key, name)? else {
        return Ok(None);
    };
    ensure!(
        matches!(v.vtype, REG_SZ | REG_EXPAND_SZ),
        "{name} must be REG_SZ or REG_EXPAND_SZ"
    );
    let data = units(&v.bytes)?;
    let end = data.iter().position(|&v| v == 0).unwrap_or(data.len());
    Ok(Some(
        String::from_utf16(&data[..end]).context("Invalid UTF-16 string")?,
    ))
}
pub fn dword(key: &RegKey, name: &str) -> Result<Option<u32>> {
    let Some(v) = raw(key, name)? else {
        return Ok(None);
    };
    ensure!(
        v.vtype == REG_DWORD && v.bytes.len() == 4,
        "{name} must be REG_DWORD"
    );
    Ok(Some(u32::from_le_bytes(v.bytes.try_into().unwrap())))
}
pub fn multi(key: &RegKey, name: &str) -> Result<Option<Vec<String>>> {
    let Some(v) = raw(key, name)? else {
        return Ok(None);
    };
    ensure!(v.vtype == REG_MULTI_SZ, "{name} must be REG_MULTI_SZ");
    let data = units(&v.bytes)?;
    let mut result = Vec::new();
    for part in data.split(|&v| v == 0) {
        if part.is_empty() {
            break;
        }
        result.push(String::from_utf16(part).context("Invalid UTF-16 multi-string")?);
    }
    Ok(Some(result))
}
pub fn set_string(key: &RegKey, name: &str, value: &str, expand: bool) -> Result<()> {
    ensure!(!value.contains('\0'), "Registry strings cannot contain NUL");
    let bytes = value
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    key.set_raw_value(
        name,
        &RegValue {
            bytes,
            vtype: if expand { REG_EXPAND_SZ } else { REG_SZ },
        },
    )?;
    Ok(())
}
pub fn set_multi(key: &RegKey, name: &str, values: &[String]) -> Result<()> {
    let mut data = Vec::new();
    for value in values {
        if value.is_empty() || value.contains('\0') {
            bail!("Invalid empty or NUL-containing MULTI_SZ entry");
        }
        data.extend(value.encode_utf16());
        data.push(0);
    }
    data.push(0);
    if values.is_empty() {
        data.push(0);
    }
    key.set_raw_value(
        name,
        &RegValue {
            bytes: data.into_iter().flat_map(u16::to_le_bytes).collect(),
            vtype: REG_MULTI_SZ,
        },
    )?;
    Ok(())
}
pub fn delete(key: &RegKey, name: &str) -> Result<()> {
    match key.delete_value(name) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
pub fn expand(value: &str) -> Result<String> {
    use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
    ensure!(!value.contains('\0'), "String contains NUL");
    let input: Vec<u16> = value.encode_utf16().chain([0]).collect();
    let mut size = unsafe { ExpandEnvironmentStringsW(input.as_ptr(), std::ptr::null_mut(), 0) };
    for _ in 0..4 {
        ensure!(
            size > 0 && size <= 16 * 1024 * 1024,
            "Invalid expanded environment length"
        );
        let mut result = vec![0u16; size as usize];
        let needed =
            unsafe { ExpandEnvironmentStringsW(input.as_ptr(), result.as_mut_ptr(), size) };
        if needed == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if needed <= size {
            return Ok(String::from_utf16(&result[..needed as usize - 1])?);
        }
        size = needed;
    }
    bail!("Environment changed repeatedly while expanding string")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_registry_path_injection() {
        for name in ["", "foo\\bar", "foo/bar", "a\0b"] {
            assert!(validate_service_name(name).is_err());
        }
        assert!(validate_service_name("中文 service").is_ok());
    }
    #[test]
    fn utf16_requires_even_bytes() {
        assert!(units(&[1]).is_err());
        assert_eq!(units(&[0x34, 0x12]).unwrap(), vec![0x1234]);
    }
    #[test]
    fn windows_name_comparison_handles_unicode() {
        assert!(equal_name("ÄPath", "äPATH"));
        assert!(!equal_name("ÄPath", "aPATH"));
        assert!(equal_name("", ""));
    }
    struct TestKey {
        path: String,
        key: RegKey,
    }
    impl TestKey {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let path = format!(
                "Software\\Xsm\\Tests\\{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let key = RegKey::predef(HKEY_CURRENT_USER)
                .create_subkey(&path)
                .unwrap()
                .0;
            Self { path, key }
        }
    }
    impl Drop for TestKey {
        fn drop(&mut self) {
            let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&self.path);
        }
    }
    #[test]
    fn registry_unicode_and_types_roundtrip() {
        let t = TestKey::new();
        let k = &t.key;
        assert_eq!(string(k, "missing").unwrap(), None);
        set_string(k, "text", "路径 😀 %SystemRoot%", true).unwrap();
        assert_eq!(
            string(k, "text").unwrap().as_deref(),
            Some("路径 😀 %SystemRoot%")
        );
        assert_eq!(k.get_raw_value("text").unwrap().vtype, REG_EXPAND_SZ);
        let values = vec!["A=hello world".into(), "变量=值".into()];
        set_multi(k, "env", &values).unwrap();
        assert_eq!(multi(k, "env").unwrap(), Some(values));
        k.set_value("number", &u32::MAX).unwrap();
        assert_eq!(dword(k, "number").unwrap(), Some(u32::MAX));
        assert!(string(k, "number").is_err());
        delete(k, "text").unwrap();
        delete(k, "text").unwrap();
        assert_eq!(string(k, "text").unwrap(), None);
    }
    #[test]
    fn registry_corruption_is_reported() {
        let t = TestKey::new();
        for (vtype, bytes) in [
            (REG_DWORD, vec![1]),
            (REG_SZ, vec![0xff]),
            (REG_SZ, vec![0, 0xd8, 0, 0]),
        ] {
            t.key
                .set_raw_value(
                    "bad",
                    &RegValue {
                        bytes,
                        vtype: vtype.clone(),
                    },
                )
                .unwrap();
            if vtype == REG_DWORD {
                assert!(dword(&t.key, "bad").is_err());
            } else {
                assert!(string(&t.key, "bad").is_err());
            }
        }
        assert!(set_string(&t.key, "bad", "nul\0bad", false).is_err());
        assert!(set_multi(&t.key, "bad", &[String::new()]).is_err());
    }
    #[test]
    fn missing_environment_tokens_survive_expansion() {
        assert_eq!(
            expand("a%XSM_UNDEFINED_32F8756B%b").unwrap(),
            "a%XSM_UNDEFINED_32F8756B%b"
        );
        assert!(!expand("%SystemRoot%").unwrap().contains('%'));
    }
}
