//! Windows argument encoding. CMD escaping and CreateProcess quoting are distinct.
#![forbid(unsafe_code)]

/// Quote one argument according to the Microsoft C runtime argv convention.
pub fn quote_arg(value: &str) -> String {
    if !value.is_empty() && !value.chars().any(|c| c.is_whitespace() || c == '"') {
        return value.to_owned();
    }
    let mut result = String::from("\"");
    let mut slashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        result.extend(std::iter::repeat_n(
            '\\',
            if ch == '"' { slashes * 2 + 1 } else { slashes },
        ));
        slashes = 0;
        result.push(ch);
    }
    result.extend(std::iter::repeat_n('\\', slashes * 2));
    result.push('"');
    result
}

/// Encode an argument inside a `.cmd` batch file. The caller must disable
/// delayed expansion so exclamation marks remain literal. Percent signs must
/// be doubled here; caret escaping does not suppress batch variable expansion.
pub fn quote_cmd(value: &str) -> String {
    let escape = value
        .chars()
        .any(|c| matches!(c, '"' | '&' | '%' | '^' | '<' | '>' | '|' | '(' | ')'));
    let quote = value.is_empty() || escape || value.chars().any(|c| c.is_whitespace() || c == '*');
    if !quote {
        return value.into();
    }
    let mut encoded = String::from("\"");
    let mut slashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        encoded.extend(std::iter::repeat_n(
            '\\',
            if ch == '"' { slashes * 2 + 1 } else { slashes },
        ));
        encoded.push(ch);
        slashes = 0;
    }
    encoded.extend(std::iter::repeat_n('\\', slashes * 2));
    encoded.push('"');
    if !escape {
        return encoded;
    }
    let mut result = String::new();
    for ch in encoded.chars() {
        if ch == '%' {
            result.push_str("%%");
            continue;
        }
        if matches!(ch, '\\' | '"' | '&' | '^' | '<' | '>' | '|' | '(' | ')') {
            result.push('^');
        }
        result.push(ch);
    }
    result
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let n = (u32::from(group[0]) << 16)
            | (u32::from(*group.get(1).unwrap_or(&0)) << 8)
            | u32::from(*group.get(2).unwrap_or(&0));
        for shift in [18, 12] {
            output.push(ALPHABET[((n >> shift) & 63) as usize] as char);
        }
        output.push(if group.len() > 1 {
            ALPHABET[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        output.push(if group.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}
fn encoded_script(script: &str) -> String {
    base64(
        &script
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    )
}

/// A readable batch invocation when possible, with a data-encoded transport for
/// CR/LF or commands beyond cmd.exe's per-line limit. The decoded bytes become
/// ProcessStartInfo.Arguments, never executable PowerShell source.
pub fn batch_command(executable: &str, arguments: &[String]) -> String {
    let ordinary = format!(
        "{} {}",
        quote_cmd(executable),
        arguments
            .iter()
            .map(|s| quote_cmd(s))
            .collect::<Vec<_>>()
            .join(" ")
    );
    if !arguments.iter().any(|s| s.contains(['\r', '\n'])) && ordinary.encode_utf16().count() < 7900
    {
        return ordinary;
    }
    let commandline = arguments
        .iter()
        .map(|s| quote_arg(s))
        .collect::<Vec<_>>()
        .join(" ");
    let data = encoded_script(&commandline);
    let exe = executable.replace('\'', "''");
    let script = format!(
        "$s=[Diagnostics.ProcessStartInfo]::new();$s.FileName='{exe}';$s.Arguments=[Text.Encoding]::Unicode.GetString([Convert]::FromBase64String('{data}'));$s.UseShellExecute=$false;$s.CreateNoWindow=$true;$p=[Diagnostics.Process]::Start($s);$p.WaitForExit();exit $p.ExitCode"
    );
    let encoded = encoded_script(&script);
    let invocation =
        format!("powershell.exe -NoProfile -ExecutionPolicy Bypass -EncodedCommand {encoded}");
    if invocation.len() < 7900 {
        return invocation;
    }
    // Chunked environment transport keeps every generated batch line bounded.
    // SET values contain only base64, so no shell metacharacters or user source.
    let mut lines = Vec::new();
    let chunks = encoded.as_bytes().chunks(3000).collect::<Vec<_>>();
    for (index, chunk) in chunks.iter().enumerate() {
        lines.push(format!(
            "set \"XSM_DUMP_DATA_{index}={}\"",
            std::str::from_utf8(chunk).unwrap()
        ));
    }
    let bootstrap = format!(
        "$b=-join(0..{}|ForEach-Object{{[Environment]::GetEnvironmentVariable('XSM_DUMP_DATA_'+$_)}});&([ScriptBlock]::Create([Text.Encoding]::Unicode.GetString([Convert]::FromBase64String($b))))",
        chunks.len() - 1
    );
    lines.push(format!(
        "powershell.exe -NoProfile -ExecutionPolicy Bypass -EncodedCommand {} || exit /b 1",
        encoded_script(&bootstrap)
    ));
    for index in 0..chunks.len() {
        lines.push(format!("set \"XSM_DUMP_DATA_{index}=\""));
    }
    lines.join("\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn argv_escaping() {
        assert_eq!(quote_arg(""), "\"\"");
        assert_eq!(quote_arg("a"), "a");
        assert_eq!(quote_arg("a b\\"), "\"a b\\\\\"");
        assert_eq!(quote_arg("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote_arg("a\\\"b"), "\"a\\\\\\\"b\"");
    }
    #[test]
    fn cmd_metacharacters() {
        assert_eq!(quote_cmd("a&b"), "^\"a^&b^\"");
        assert_eq!(quote_cmd("%TEMP%"), "^\"%%TEMP%%^\"");
        assert_eq!(quote_cmd("a b"), "\"a b\"");
        assert_eq!(quote_cmd("路径"), "路径");
    }
    #[test]
    #[cfg(windows)]
    fn actual_batch_round_trip_preserves_metacharacters_and_unicode() {
        use std::os::windows::process::CommandExt;
        let root = std::env::temp_dir().join(format!("xsm-argv-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        let helper = root.join("capture.ps1");
        let output = root.join("arguments.txt");
        let batch = root.join("round trip.cmd");
        std::fs::write(&helper,"param([string]$Output)\r\n[IO.File]::WriteAllText($Output, ($args -join [char]0), [Text.UTF8Encoding]::new($false))\r\n").unwrap();
        let values = [
            "",
            "plain",
            "a b",
            "a\tb",
            "%SystemRoot%",
            "%1",
            "%%",
            "!SystemRoot!",
            "^",
            "a&b",
            "|",
            "<>",
            "(parentheses)",
            "a\"b",
            "a\\\"b",
            "trailing space \\",
            "C:\\tail\\",
            "路径 😀",
            "mixed %PATH% & !bang! ^ (paren) \\\"quote\\\"",
        ];
        let extended = "large mixed %PATH% ! ^ & | <> 中文😀 ".repeat(400);
        for corpus in [
            values.to_vec(),
            vec!["first\r\nsecond", "a\rb", "a\nb", "'quoted'"],
            vec![extended.as_str()],
        ] {
            let mut arguments = vec![
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                helper.to_string_lossy().into_owned(),
                output.to_string_lossy().into_owned(),
            ];
            arguments.extend(corpus.iter().map(|s| s.to_string()));
            let body = format!(
                "@echo off\r\nsetlocal DisableDelayedExpansion\r\nchcp 65001 >nul\r\n{}\r\n",
                batch_command("powershell.exe", &arguments)
            );
            assert!(body.lines().all(|line| line.encode_utf16().count() < 8191));
            std::fs::write(&batch, body).unwrap();
            let result = std::process::Command::new("cmd.exe")
                .args(["/d", "/v:on", "/c"])
                .raw_arg(quote_arg(&batch.to_string_lossy()))
                .creation_flags(0x0800_0000)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            let actual = std::fs::read_to_string(&output).unwrap();
            assert_eq!(actual.split('\0').collect::<Vec<_>>(), corpus);
        }
    }
    #[test]
    fn base64_standard_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected);
        }
    }
}
