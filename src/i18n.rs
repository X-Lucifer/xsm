//! Presentation language never changes machine-readable settings or state tokens.
use anyhow::Result;
use std::sync::{
    Once,
    atomic::{AtomicU8, Ordering},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    ZhCn,
    EnUs,
}
static INIT: Once = Once::new();
static LANGUAGE: AtomicU8 = AtomicU8::new(1);
fn parse(value: &str) -> Option<Language> {
    match value.to_ascii_lowercase().replace('_', "-").as_str() {
        "zh" | "zh-cn" | "zh-hans" => Some(Language::ZhCn),
        "en" | "en-us" => Some(Language::EnUs),
        _ => None,
    }
}
fn system_language() -> Language {
    #[cfg(windows)]
    if unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() } & 0x3ff == 4 {
        return Language::ZhCn;
    }
    Language::EnUs
}
fn preference() -> Option<Language> {
    #[cfg(windows)]
    {
        let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
            .open_subkey("Software\\xsm")
            .ok()?;
        let value: String = key.get_value("Language").ok()?;
        parse(&value)
    }
    #[cfg(not(windows))]
    {
        None
    }
}
pub fn initialize() {
    INIT.call_once(|| {
        let value = std::env::var("XSM_LANG")
            .ok()
            .as_deref()
            .and_then(parse)
            .or_else(preference)
            .unwrap_or_else(system_language);
        LANGUAGE.store(u8::from(value == Language::EnUs), Ordering::Relaxed);
    });
}
pub fn language() -> Language {
    initialize();
    if LANGUAGE.load(Ordering::Relaxed) == 0 {
        Language::ZhCn
    } else {
        Language::EnUs
    }
}
pub fn text(zh: &'static str, en: &'static str) -> &'static str {
    if language() == Language::ZhCn { zh } else { en }
}
pub fn set_language(language: Language) -> Result<()> {
    initialize();
    #[cfg(windows)]
    {
        let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
            .create_subkey("Software\\xsm")?
            .0;
        key.set_value(
            "Language",
            &if language == Language::ZhCn {
                "zh-CN"
            } else {
                "en-US"
            },
        )?;
    }
    LANGUAGE.store(u8::from(language == Language::EnUs), Ordering::Relaxed);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn locale_names_are_bounded() {
        assert_eq!(parse("zh_CN"), Some(Language::ZhCn));
        assert_eq!(parse("en-US"), Some(Language::EnUs));
        assert_eq!(parse("fr"), None);
    }
}
