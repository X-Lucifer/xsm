fn main() {
    let seconds = match std::env::var("SOURCE_DATE_EPOCH") {
        Ok(value) => value
            .parse::<u64>()
            .expect("SOURCE_DATE_EPOCH must be an unsigned Unix timestamp"),
        Err(_) => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("build clock")
            .as_secs(),
    };
    assert!(
        seconds <= 253_402_300_799,
        "Build date must be within years 1970..9999"
    );
    let mut days = seconds / 86_400;
    let mut year: u64 = 1970;
    let leap = |year: u64| {
        year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100))
    };
    while days >= if leap(year) { 366 } else { 365 } {
        days -= if leap(year) { 366 } else { 365 };
        year += 1;
    }
    let months = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 0;
    while days >= months[month] {
        days -= months[month];
        month += 1;
    }
    println!(
        "cargo:rustc-env=XSM_BUILD_DATE={year:04}/{:02}/{:02}",
        month + 1,
        days + 1
    );
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let version = std::env::var("CARGO_PKG_VERSION").expect("package version");
        let parts: Vec<&str> = version.split('.').collect();
        let numbers = format!("{},{},{},0", parts[0], parts[1], parts[2]);
        let macros = [
            format!("XSM_VERSION_NUM={numbers}"),
            format!("XSM_VERSION_STR=\"{version}.0\""),
        ];
        embed_resource::compile_for("resources/xsm.rc", ["xsm"], &macros)
            .manifest_required()
            .expect("Compile Windows application resources");
        let manifest = std::fs::read_to_string("resources/xsm.manifest")
            .expect("Read Windows manifest template")
            .replace("@XSM_VERSION@", &format!("{version}.0"));
        let manifest_path = std::path::PathBuf::from(
            std::env::var_os("OUT_DIR").expect("Cargo build output directory"),
        )
        .join("xsm.manifest");
        std::fs::write(&manifest_path, manifest).expect("Write versioned Windows manifest");
        // Use the caller's token. Read-only commands must not force elevation.
        println!("cargo:rustc-link-arg-bin=xsm=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin=xsm=/MANIFESTINPUT:{}",
            manifest_path.display()
        );
    }
    println!("cargo:rerun-if-changed=resources/xsm.manifest");
    println!("cargo:rerun-if-changed=resources/xsm.rc");
    println!("cargo:rerun-if-changed=resources/xsm.ico");
    println!("cargo:rerun-if-changed=resources/event-message.bin");
}
