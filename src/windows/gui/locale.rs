//! Presentation-only translations; service names, paths and setting values remain opaque.
#![forbid(unsafe_code)]
use crate::i18n::Language;

// These pure presentation adapters are shared by UI frontends. They deliberately
// do not translate free-form values such as service names, paths or commands.
mod vocabulary {
    use super::Language;

    type Entry = (&'static str, &'static str, &'static str);
    pub(super) const SETTINGS: &[Entry] = &[
        ("Application", "应用程序", "Application"),
        ("AppParameters", "启动参数", "Arguments"),
        ("AppDirectory", "启动目录", "Working directory"),
        ("AppExit", "退出动作", "Exit action"),
        ("AppEvents", "事件钩子", "Event hooks"),
        ("AppAffinity", "处理器亲和性", "Processor affinity"),
        (
            "AppEnvironment",
            "替换应用环境变量",
            "Replace application environment",
        ),
        (
            "AppEnvironmentExtra",
            "附加应用环境变量",
            "Additional application environment",
        ),
        (
            "AppNoConsole",
            "禁用应用控制台",
            "Disable application console",
        ),
        ("AppPriority", "进程优先级", "Process priority"),
        ("AppRestartDelay", "重启延迟（毫秒）", "Restart delay (ms)"),
        ("AppStdin", "标准输入文件", "Standard input file"),
        (
            "AppStdinShareMode",
            "输入文件共享权限",
            "Input file sharing flags",
        ),
        (
            "AppStdinCreationDisposition",
            "输入文件打开方式",
            "Input file creation mode",
        ),
        (
            "AppStdinFlagsAndAttributes",
            "输入文件属性与标志",
            "Input file attributes and flags",
        ),
        ("AppStdout", "标准输出文件", "Standard output file"),
        (
            "AppStdoutShareMode",
            "输出文件共享权限",
            "Output file sharing flags",
        ),
        (
            "AppStdoutCreationDisposition",
            "输出文件打开方式",
            "Output file creation mode",
        ),
        (
            "AppStdoutFlagsAndAttributes",
            "输出文件属性与标志",
            "Output file attributes and flags",
        ),
        (
            "AppStdoutCopyAndTruncate",
            "输出轮转时复制并清空",
            "Copy and truncate output on rotation",
        ),
        ("AppStderr", "标准错误文件", "Standard error file"),
        (
            "AppStderrShareMode",
            "错误文件共享权限",
            "Error file sharing flags",
        ),
        (
            "AppStderrCreationDisposition",
            "错误文件打开方式",
            "Error file creation mode",
        ),
        (
            "AppStderrFlagsAndAttributes",
            "错误文件属性与标志",
            "Error file attributes and flags",
        ),
        (
            "AppStderrCopyAndTruncate",
            "错误轮转时复制并清空",
            "Copy and truncate errors on rotation",
        ),
        (
            "AppStopMethodSkip",
            "跳过的停止方法（位掩码）",
            "Shutdown methods to skip (bitmask)",
        ),
        (
            "AppStopMethodConsole",
            "控制台停止等待（毫秒）",
            "Console shutdown timeout (ms)",
        ),
        (
            "AppStopMethodWindow",
            "窗口关闭等待（毫秒）",
            "Window shutdown timeout (ms)",
        ),
        (
            "AppStopMethodThreads",
            "线程退出等待（毫秒）",
            "Thread shutdown timeout (ms)",
        ),
        (
            "AppKillProcessTree",
            "停止整个进程树",
            "Stop the entire process tree",
        ),
        (
            "AppThrottle",
            "快速退出判定时间（毫秒）",
            "Rapid exit threshold (ms)",
        ),
        ("AppRedirectHook", "重定向钩子输出", "Redirect hook output"),
        ("AppRotateFiles", "启用日志轮转", "Enable log rotation"),
        (
            "AppRotateOnline",
            "运行期间轮转日志",
            "Rotate logs while running",
        ),
        (
            "AppRotateSeconds",
            "轮转文件最短存续时间（秒）",
            "Minimum file age for rotation (seconds)",
        ),
        (
            "AppRotateBytes",
            "轮转大小低位（字节）",
            "Rotation size low bits (bytes)",
        ),
        (
            "AppRotateBytesHigh",
            "轮转大小高 32 位",
            "Rotation size high 32 bits",
        ),
        ("AppRotateDelay", "轮转等待（毫秒）", "Rotation delay (ms)"),
        (
            "AppTimestampLog",
            "输出附加时间戳",
            "Timestamp application output",
        ),
        ("XsmLogLevel", "日志级别", "Log level"),
        ("XsmLogPath", "日志路径", "Log path"),
        (
            "XsmLogMaxBytes",
            "日志大小上限（字节）",
            "Log size limit (bytes)",
        ),
        (
            "XsmLogMaxFiles",
            "日志归档保留数量",
            "Log archives to retain",
        ),
        (
            "DependOnGroup",
            "依赖的服务组",
            "Service group dependencies",
        ),
        ("DependOnService", "依赖的服务", "Service dependencies"),
        ("Description", "服务描述", "Service description"),
        ("DisplayName", "服务显示名称", "Service display name"),
        (
            "Environment",
            "服务宿主环境变量",
            "Service host environment",
        ),
        ("ImagePath", "服务宿主命令", "Service host command"),
        ("ObjectName", "登录账户", "Logon account"),
        ("Name", "服务名称", "Service name"),
        ("Start", "启动方式", "Startup type"),
        ("Type", "服务类型", "Service type"),
    ];
    pub(super) const HOOKS: &[Entry] = &[
        ("Start/Pre", "启动前", "Before start"),
        ("Start/Post", "启动后", "After start"),
        ("Stop/Pre", "停止前", "Before stop"),
        ("Exit/Post", "应用退出后", "After application exit"),
        ("Power/Change", "电源状态变化", "Power status changed"),
        ("Power/Resume", "系统恢复后", "After system resume"),
        ("Rotate/Pre", "日志轮转前", "Before log rotation"),
        ("Rotate/Post", "日志轮转后", "After log rotation"),
    ];
    const BOOLEAN: &[Entry] = &[("0", "关闭", "Off"), ("1", "开启", "On")];
    const CREATION: &[Entry] = &[
        ("1", "仅新建文件", "Create new only"),
        ("2", "新建或覆盖文件", "Create or overwrite"),
        ("3", "仅打开已有文件", "Open existing only"),
        ("4", "打开或新建文件", "Open or create"),
        ("5", "清空已有文件", "Truncate existing"),
    ];

    fn options(setting: &str) -> &'static [Entry] {
        match setting {
            "AppExit" => &[
                ("Restart", "重新启动应用", "Restart application"),
                ("Ignore", "保持服务运行", "Keep service running"),
                ("Exit", "正常停止服务", "Stop service normally"),
                ("Suicide", "报告服务失败", "Report service failure"),
            ],
            "AppPriority" => &[
                ("REALTIME_PRIORITY_CLASS", "实时", "Realtime"),
                ("HIGH_PRIORITY_CLASS", "高", "High"),
                ("ABOVE_NORMAL_PRIORITY_CLASS", "高于正常", "Above normal"),
                ("NORMAL_PRIORITY_CLASS", "正常", "Normal"),
                ("BELOW_NORMAL_PRIORITY_CLASS", "低于正常", "Below normal"),
                ("IDLE_PRIORITY_CLASS", "低", "Low"),
            ],
            "Start" => &[
                ("SERVICE_AUTO_START", "自动", "Automatic"),
                (
                    "SERVICE_DELAYED_AUTO_START",
                    "自动（延迟启动）",
                    "Automatic (delayed start)",
                ),
                ("SERVICE_DEMAND_START", "手动", "Manual"),
                ("SERVICE_DISABLED", "禁用", "Disabled"),
                ("SERVICE_BOOT_START", "系统引导时启动", "Boot start"),
                ("SERVICE_SYSTEM_START", "系统初始化时启动", "System start"),
                ("SERVICE_UNKNOWN", "未知启动方式", "Unknown startup type"),
            ],
            "Type" => &[
                (
                    "SERVICE_WIN32_OWN_PROCESS",
                    "独立进程服务",
                    "Own process service",
                ),
                (
                    "SERVICE_INTERACTIVE_PROCESS",
                    "交互式服务",
                    "Interactive service",
                ),
                (
                    "SERVICE_WIN32_SHARE_PROCESS",
                    "共享进程服务",
                    "Shared process service",
                ),
                (
                    "SERVICE_WIN32_SHARE_PROCESS|SERVICE_INTERACTIVE_PROCESS",
                    "共享进程交互式服务",
                    "Shared process interactive service",
                ),
                ("SERVICE_KERNEL_DRIVER", "内核驱动程序", "Kernel driver"),
                (
                    "SERVICE_FILE_SYSTEM_DRIVER",
                    "文件系统驱动程序",
                    "File system driver",
                ),
                ("SERVICE_UNKNOWN", "未知服务类型", "Unknown service type"),
            ],
            "XsmLogLevel" => &[
                ("off", "关闭", "Off"),
                ("error", "错误", "Error"),
                ("warn", "警告", "Warning"),
                ("info", "信息", "Information"),
                ("debug", "调试", "Debug"),
                ("trace", "跟踪", "Trace"),
            ],
            "AppAffinity" => &[("All", "所有可用处理器", "All available processors")],
            "AppEvents" => HOOKS,
            "AppNoConsole"
            | "AppKillProcessTree"
            | "AppRedirectHook"
            | "AppRotateFiles"
            | "AppRotateOnline"
            | "AppTimestampLog"
            | "AppStdoutCopyAndTruncate"
            | "AppStderrCopyAndTruncate" => BOOLEAN,
            "AppStdinCreationDisposition"
            | "AppStdoutCreationDisposition"
            | "AppStderrCreationDisposition" => CREATION,
            _ => &[],
        }
    }
    fn display(entry: &Entry, language: Language) -> &'static str {
        match language {
            Language::ZhCn => entry.1,
            Language::EnUs => entry.2,
        }
    }
    fn to_label<'a>(entries: &[Entry], value: &'a str, language: Language) -> &'a str {
        entries
            .iter()
            .find(|entry| entry.0.eq_ignore_ascii_case(value))
            .map_or(value, |entry| display(entry, language))
    }
    #[cfg(test)]
    fn to_value<'a>(entries: &[Entry], label: &'a str, language: Language) -> &'a str {
        entries
            .iter()
            .find(|entry| display(entry, language) == label)
            .map_or(label, |entry| entry.0)
    }
    /// Unknown extension settings retain their exact spelling.
    pub fn setting_label(name: &str, language: Language) -> &str {
        to_label(SETTINGS, name, language)
    }
    #[cfg(test)]
    pub fn setting_name(label: &str, language: Language) -> &str {
        to_value(SETTINGS, label, language)
    }
    /// Call only for enum/boolean choices, never free-form account names or paths.
    /// Prefer retaining the canonical value alongside its display label in UI state.
    pub fn option_label<'a>(setting: &str, value: &'a str, language: Language) -> &'a str {
        to_label(options(setting), value, language)
    }
    #[cfg(test)]
    pub fn option_value<'a>(setting: &str, label: &'a str, language: Language) -> &'a str {
        to_value(options(setting), label, language)
    }
    pub fn hook_label(value: &str, language: Language) -> &str {
        to_label(HOOKS, value, language)
    }
    #[cfg(test)]
    pub fn hook_value(label: &str, language: Language) -> &str {
        to_value(HOOKS, label, language)
    }
    /// A label for a canonical setting and optional subkey, without modifying either.
    pub fn field_label(setting: &str, key: Option<&str>, language: Language) -> String {
        let caption = setting_label(setting, language);
        let key = match (setting, key) {
            ("AppEvents", Some(key)) => return hook_label(key, language).into(),
            ("AppExit", Some("*")) => {
                return match language {
                    Language::ZhCn => "按退出码覆盖动作".into(),
                    Language::EnUs => "Actions by exit code".into(),
                };
            }
            ("AppExit", Some(key)) if key.eq_ignore_ascii_case("Default") => match language {
                Language::ZhCn => "默认",
                Language::EnUs => "Default",
            },
            (_, Some(key)) if !key.is_empty() => key,
            _ => return caption.into(),
        };
        format!("{caption} · {key}")
    }

    /// Human-readable help; callers may show the canonical key separately for experts.
    pub fn setting_help(setting: &str, language: Language) -> &'static str {
        let (zh, en) = match setting {
            "Application" => (
                "填写要作为服务运行的 EXE 路径，或使用浏览按钮选择应用程序。",
                "Enter the path to the EXE to run as a service, or select it with the browse button.",
            ),
            "AppDirectory" => (
                "选择应用后自动使用 EXE 所在目录；更换应用时仍自动更新，手动指定的目录会保留。也可使用目录按钮选择工作目录。",
                "Selecting an application uses its EXE directory automatically. Changing the application updates an automatic directory while preserving a manually chosen one. You can also choose the working directory with the folder button.",
            ),
            "AppParameters" => (
                "填写传给应用程序的 Windows 命令行参数。含空格的参数使用双引号，例如 --name \"My Service\"；引号与反斜杠按 Windows 命令行规则处理。",
                "Enter the application's Windows command-line arguments. Use double quotes around arguments containing spaces, for example --name \"My Service\". Quotes and backslashes follow Windows command-line rules.",
            ),
            "AppEvents" => (
                "填写在标签所示事件触发时执行的命令。需要 shell 内建命令时请显式调用 cmd.exe /c。启动前钩子返回 99 可取消启动。",
                "Enter the command to run when the event named by this label occurs. For shell built-in commands, explicitly use cmd.exe /c. A before-start hook can return 99 to cancel startup.",
            ),
            "AppExit" => (
                "选择应用退出后的动作；可为特定退出码设置不同动作。报告服务失败可触发 Windows 的服务恢复策略。",
                "Choose an action after application exit, with optional overrides by exit code. Reporting service failure can trigger Windows service recovery.",
            ),
            "AppAffinity" => (
                "填写处理器编号或范围，例如 0,2-4；也可选择所有可用处理器。",
                "Enter processor numbers or ranges, such as 0,2-4, or select all available processors.",
            ),
            "AppPriority" => (
                "选择应用进程优先级。实时优先级可能影响系统响应，请谨慎使用。",
                "Select the application process priority. Use realtime priority with care because it can affect system responsiveness.",
            ),
            "AppNoConsole" => (
                "开启后不为应用创建控制台，控制台停止信号也将不可用。",
                "When enabled, the application has no console and console shutdown signals are unavailable.",
            ),
            "ObjectName" => (
                "选择本地系统、本地服务、网络服务或指定账户。指定账户可填写本地账户 .\\用户名、域账户 DOMAIN\\用户名，或当前服务的虚拟账户 NT SERVICE\\当前服务名。内置账户及当前服务虚拟账户的凭据由 Windows 管理，无需输入密码。",
                "Choose Local system, Local service, Network service, or Custom account. Custom names can be local (.\\user), domain (DOMAIN\\user), or the current service's virtual account (NT SERVICE\\current-service-name). Windows manages credentials for built-in accounts and the current service's virtual account; no password is needed.",
            ),
            "AppEnvironment" => (
                "每行一个“名称=值”。此列表替换应用继承的环境；通常应使用附加应用环境变量。",
                "One NAME=VALUE per line. Replaces the application's inherited environment; additional application variables are usually sufficient.",
            ),
            "AppEnvironmentExtra" => (
                "每行一个“名称=值”，覆盖同名变量。保存时替换完整列表；删除一行即可移除该条配置。",
                "One NAME=VALUE per line, overriding names already present. Saving replaces the full list; delete a line to remove that configured entry.",
            ),
            "Environment" => (
                "每行一个“名称=值”。配置服务宿主的环境，修改后需重启服务。保存时替换完整列表。",
                "One NAME=VALUE per line for the service host. Restart the service after changes. Saving replaces the full list.",
            ),
            "DependOnService" => (
                "每行填写一个真实服务名，也可搜索本机服务后添加；搜索列表排除当前服务和已添加项。Windows 会先启动依赖服务，再启动当前服务。保存时替换完整依赖列表。",
                "Enter one actual service name per line, or search local services to add one. Search excludes the current service and existing entries. Windows starts dependencies before this service. Saving replaces the complete dependency list.",
            ),
            "DependOnGroup" => (
                "每行填写一个服务组名称，无需添加 + 前缀。保存时替换完整的服务组依赖列表。",
                "Enter one service group name per line; no + prefix is needed. Saving replaces the complete list of service group dependencies.",
            ),
            "Start" => (
                "选择自动、延迟自动、手动或禁用。修改启动方式不会立即启动或停止服务。",
                "Choose automatic, delayed automatic, manual or disabled. Changing the startup type does not immediately start or stop the service.",
            ),
            "Type" => (
                "选择独立进程或交互式服务。交互式服务受 Windows 会话隔离限制。",
                "Choose an own-process or interactive service. Interactive services are subject to Windows session isolation.",
            ),
            "Name" => (
                "新服务的唯一系统标识。填写表单不会创建服务，点击“安装服务”后才创建；已有服务的名称只读。",
                "The new service's unique system identifier. Filling in the form does not create a service; select Install service to create it. Existing service names are read-only.",
            ),
            "AppStopMethodSkip" => (
                "0 表示不跳过，依次尝试停止方法，应用退出后即停止尝试。位值：1 跳过 Ctrl+C，2 跳过窗口关闭消息，4 跳过线程退出消息，8 跳过强制终止；相加组合，例如 3=1+2。包含 8 时，若其他方法未使应用退出，进程可能继续运行。",
                "0 skips nothing: try methods in order until the application exits. Flags: 1 skip Ctrl+C, 2 skip window-close messages, 4 skip thread-quit messages, 8 skip forced termination. Add flags to combine, e.g. 3=1+2. When 8 is included, the process may keep running if the other methods do not stop it.",
            ),
            "AppKillProcessTree" => (
                "开启后停止应用时一并停止其子进程。",
                "When enabled, stopping the application also stops its child processes.",
            ),
            "AppRedirectHook" => (
                "将钩子输出写入应用配置的标准输出和标准错误文件。",
                "Write hook output to the application's configured standard output and standard error files.",
            ),
            "AppRotateSeconds" => (
                "启动轮转时，仅处理存续时间达到此秒数的文件；这不是定时轮转间隔。",
                "For startup rotation, only rotate files at least this many seconds old. This is not a periodic rotation interval.",
            ),
            "AppRotateBytes" => (
                "轮转阈值的低 32 位，单位为字节；与高 32 位组合为完整大小。",
                "Low 32 bits of the rotation threshold in bytes, combined with the high 32 bits for the full size.",
            ),
            "AppRotateBytesHigh" => (
                "轮转阈值的高 32 位；与低位组合为 64 位字节数。",
                "High 32 bits of the rotation threshold, combined with the low bits to form a 64-bit byte count.",
            ),
            "AppStdoutCopyAndTruncate" | "AppStderrCopyAndTruncate" => (
                "轮转时复制内容后清空原文件，保留原文件句柄；关闭时使用文件重命名方式。",
                "Copy contents then truncate the original file during rotation, retaining its handle. Otherwise rotation renames the file.",
            ),
            "XsmLogLevel" => (
                "设置 XSM 自身日志的详细程度。应用程序的标准输出和标准错误由各自设置控制。",
                "Set the verbosity of XSM logs. Application standard output and standard error are configured separately.",
            ),
            "XsmLogPath" => (
                "日志路径，留空时仅使用 Windows 事件日志。服务账户必须有目标目录的写入权限。",
                "Log path. Leave empty for Windows Event Log only. The service account needs write access to the directory.",
            ),
            "XsmLogMaxBytes" => (
                "日志单文件大小上限，单位为字节。默认 10485760（10 MiB）。",
                "Maximum log file size in bytes. Default: 10485760 (10 MiB).",
            ),
            "XsmLogMaxFiles" => (
                "日志轮转时保留的归档文件数量，默认 5。",
                "Number of log archives to retain. Default: 5.",
            ),
            name if name.ends_with("ShareMode") => (
                "文件共享权限：1 读取、2 写入、4 删除；可相加组合。",
                "File sharing flags: 1 read, 2 write, 4 delete. Add flags to combine them.",
            ),
            name if name.ends_with("CreationDisposition") => (
                "1 仅新建，2 新建或覆盖，3 仅打开已有文件，4 打开或新建，5 清空已有文件。",
                "1 create new only, 2 create or overwrite, 3 open existing only, 4 open or create, 5 truncate existing.",
            ),
            name if name.ends_with("FlagsAndAttributes") => (
                "Windows 文件属性与标志，支持十进制或以 0x 开头的十六进制。",
                "Windows file attributes and flags, in decimal or hexadecimal with a 0x prefix.",
            ),
            "AppThrottle" => (
                "应用运行时间短于此值时视为快速退出，并对反复重启进行节流。单位为毫秒。",
                "An application lifetime shorter than this threshold is a rapid exit; repeated restarts are throttled. Value in milliseconds.",
            ),
            "AppRestartDelay"
            | "AppRotateDelay"
            | "AppStopMethodConsole"
            | "AppStopMethodWindow"
            | "AppStopMethodThreads" => (
                "以毫秒为单位，在对应的服务生命周期操作中生效。",
                "Time in milliseconds, applied during the corresponding service lifecycle operation.",
            ),
            _ => (
                "设置保存在 Windows 注册表或服务配置中。应用的运行策略和环境配置更改在后续应用启动时生效。",
                "Saved in the Windows registry or service configuration. Changes to application runtime policy and environment settings take effect on subsequent application starts.",
            ),
        };
        match language {
            Language::ZhCn => zh,
            Language::EnUs => en,
        }
    }
}
pub use vocabulary::{field_label, option_label, setting_help, setting_label};
#[cfg(test)]
use vocabulary::{hook_label, hook_value, option_value, setting_name};

pub(super) fn logon_account_label(
    account: super::model::LogonAccount,
    language: Language,
) -> &'static str {
    use super::model::LogonAccount;
    let (zh, en) = match account {
        LogonAccount::LocalSystem => ("本地系统", "Local system"),
        LogonAccount::LocalService => ("本地服务", "Local service"),
        LogonAccount::NetworkService => ("网络服务", "Network service"),
        LogonAccount::Custom => ("指定账户", "Custom account"),
    };
    match language {
        Language::ZhCn => zh,
        Language::EnUs => en,
    }
}

pub(super) fn account_name_help(language: Language) -> &'static str {
    match language {
        Language::ZhCn => {
            "本地账户使用 .\\用户名，域账户使用 DOMAIN\\用户名；当前服务的虚拟账户使用 NT SERVICE\\当前服务名。虚拟账户中的服务名必须与当前服务的真实名称一致。"
        }
        Language::EnUs => {
            "Use .\\user for a local account, DOMAIN\\user for a domain account, or NT SERVICE\\current-service-name for this service's virtual account. The virtual account name must match the current service's actual name."
        }
    }
}

pub(super) fn password_help(language: Language) -> &'static str {
    match language {
        Language::ZhCn => {
            "已有账户未变时，留空保留原密码。使用指定普通账户安装新服务，或更换到其他普通账户时，需填写该账户的密码。内置账户及当前服务的虚拟账户无需密码，凭据由 Windows 管理。密码不会回显或导出。"
        }
        Language::EnUs => {
            "Leave blank to retain the password when the existing account is unchanged. Supply the account password when installing a service with a custom user account or switching to another user account. Built-in accounts and the current service's virtual account need no password; Windows manages their credentials. Passwords are never displayed or exported."
        }
    }
}

pub const MESSAGES: &[(&str, &str)] = &[
    (
        "此服务类型仅支持查看配置",
        "Configuration is read-only for this service type",
    ),
    ("需要管理员权限", "Administrator permission is required"),
    ("配置快照不完整", "Configuration snapshot is incomplete"),
    (
        "原生服务不支持应用配置",
        "Application configuration is unavailable for native services",
    ),
    (
        "设置已保存，但重新读取失败：{error:#}。请重新读取本页。",
        "Settings were saved, but reloading failed: {error:#}. Reload this page.",
    ),
    ("无法初始化设置页", "Could not initialize native tabs"),
    ("事件钩子条目无效", "Invalid hook key"),
    ("无法打开 GitHub 页面", "Could not open the GitHub page"),
    ("管理员重启", "Run as admin"),
    ("服务设置", "Service"),
    ("重新读取", "Reload"),
    ("应用本页", "Apply page"),
    ("重置本页", "Reset page"),
    ("恢复", "Recovery"),
    ("输入输出", "I/O"),
    ("日志", "Logs"),
    ("本页没有未保存的更改。", "No unsaved changes on this page."),
    (
        "请先成功读取本页设置",
        "Reload this page successfully before saving.",
    ),
    (
        "本页设置已保存；重启服务后生效。",
        "Page saved. Restart the service to apply changes.",
    ),
    (
        "退出码覆盖（每行 code=action）",
        "Exit overrides (one code=action per line)",
    ),
    (
        "退出码覆盖格式应为 code=action",
        "Use code=action for exit overrides",
    ),
    (
        "退出动作应为 Restart、Ignore、Exit 或 Suicide",
        "Exit actions must be Restart, Ignore, Exit or Suicide",
    ),
    ("退出码不能重复", "Exit codes must be unique"),
    ("无效设置页面", "Invalid settings page"),
    ("最小化", "Minimize"),
    ("最大化或还原", "Maximize or restore"),
    ("关闭", "Close"),
    ("创建界面提示失败", "Could not create tooltips"),
    ("深色", "Dark"),
    ("浅色", "Light"),
    (
        "可执行文件 (*.exe;*.com;*.bat;*.cmd)\0*.exe;*.com;*.bat;*.cmd\0所有文件 (*.*)\0*.*\0",
        "Executable files (*.exe;*.com;*.bat;*.cmd)\0*.exe;*.com;*.bat;*.cmd\0All files (*.*)\0*.*\0",
    ),
    (
        "命令脚本 (*.cmd)\0*.cmd\0所有文件 (*.*)\0*.*\0",
        "Command scripts (*.cmd)\0*.cmd\0All files (*.*)\0*.*\0",
    ),
    ("全部设置", "All settings"),
    ("应用程序", "Application"),
    ("服务属性", "Service details"),
    ("登录账户", "Log on"),
    ("依赖项", "Dependencies"),
    ("进程", "Process"),
    ("退出与重启", "Exit and restart"),
    ("停止", "Shutdown"),
    ("标准输入输出", "Input / output"),
    ("日志轮转", "Log rotation"),
    ("环境变量", "Environment"),
    ("事件钩子", "Event hooks"),
    ("XSM 自身日志", "XSM logs"),
    ("刷新(&F)", "Refresh (&F)"),
    ("新建(&N)", "New (&N)"),
    ("启动(&S)", "Start (&S)"),
    ("停止(&T)", "Stop (&T)"),
    ("重启(&R)", "Restart (&R)"),
    ("移除(&D)", "Remove (&D)"),
    ("浏览…", "Browse…"),
    ("安装服务(&I)", "Install service (&I)"),
    ("保存应用配置(&I)", "Save application (&I)"),
    ("读取(&L)", "Reload (&L)"),
    ("应用设置(&A)", "Apply (&A)"),
    ("重置(&E)", "Reset (&E)"),
    ("执行", "Run"),
    ("以管理员身份重启", "Run as administrator"),
    ("管理员模式", "Administrator"),
    ("服务", "Services"),
    ("服务名称", "Service name"),
    ("启动目录", "Directory"),
    ("启动参数", "Arguments"),
    ("设置分类", "Category"),
    ("参数名称", "Setting"),
    ("条目", "Entry"),
    ("参数值", "Value"),
    ("账户密码", "Password"),
    ("就绪", "Ready"),
    ("Windows 服务管理", "Windows service manager"),
    ("应用配置", "Application"),
    ("高级设置", "Advanced settings"),
    ("包括系统服务", "Include system services"),
    ("暂停服务", "Pause service"),
    ("继续服务", "Continue service"),
    ("轮转日志", "Rotate logs"),
    ("查看进程", "View processes"),
    ("导出配置", "Export configuration"),
    ("已停止", "Stopped"),
    ("正在启动", "Starting"),
    ("正在停止", "Stopping"),
    ("运行中", "Running"),
    ("正在继续", "Continuing"),
    ("正在暂停", "Pausing"),
    ("已暂停", "Paused"),
    ("未知状态", "Unknown"),
    (" · Windows 原生服务", " · Native Windows service"),
    (
        "条目填写事件/动作。Start/Pre 返回 99 可中止启动；命令按 Windows 命令行语法填写。",
        "Select an event/action pair. Start/Pre can return 99 to abort startup. Use Windows command-line syntax.",
    ),
    (
        "条目填写 Default 或退出码。值：Restart、Ignore、Exit、Suicide。",
        "Enter Default or an exit code. Actions: Restart, Ignore, Exit, Suicide.",
    ),
    (
        "填写 CPU 编号或范围，例如 0,2-4；All 表示所有可用 CPU。",
        "Enter CPU numbers or ranges, such as 0,2-4. All uses every available CPU.",
    ),
    (
        "填写 LocalSystem、NT Authority\\LocalService、NT Authority\\NetworkService 或账户名。密码不会显示或导出；修改账户时可在下方输入密码。",
        "Enter LocalSystem, NT Authority\\LocalService, NT Authority\\NetworkService or an account name. Passwords are never displayed or exported.",
    ),
    (
        "每行 NAME=VALUE；此设置替换继承的环境。通常使用 AppEnvironmentExtra 添加变量。",
        "One NAME=VALUE per line. Replaces the inherited environment. Use AppEnvironmentExtra to add variables.",
    ),
    (
        "每行 NAME=VALUE；保存时用本页完整列表替换该设置。删除一行即可移除变量。",
        "One NAME=VALUE per line. Saving replaces this setting with the complete list. Delete a line to remove a variable.",
    ),
    (
        "每行一个依赖；保存时用完整列表替换原值。依赖组名称可以保留 + 前缀。",
        "One dependency per line. Saving replaces the complete list. Group names may keep their + prefix.",
    ),
    (
        "SERVICE_WIN32_OWN_PROCESS 或 SERVICE_INTERACTIVE_PROCESS。",
        "SERVICE_WIN32_OWN_PROCESS or SERVICE_INTERACTIVE_PROCESS.",
    ),
    ("系统服务名称是只读属性。", "The service name is read-only."),
    (
        "位掩码：1 跳过控制台，2 跳过窗口消息，4 跳过线程消息，8 跳过强制终止。",
        "Skip-method bitmask: 1 console, 2 window messages, 4 thread messages, 8 forced termination.",
    ),
    (
        "轮转大小的高 32 位，与 AppRotateBytes 组合为 64 位字节数。",
        "High 32 bits of the rotation size. Combined with AppRotateBytes to form a 64-bit byte count.",
    ),
    (
        "Windows 文件共享标志：1 读取、2 写入、4 删除；可组合。",
        "Windows sharing flags: 1 read, 2 write, 4 delete. Flags may be combined.",
    ),
    (
        "Windows 文件创建策略：1 CREATE_NEW，2 CREATE_ALWAYS，3 OPEN_EXISTING，4 OPEN_ALWAYS，5 TRUNCATE_EXISTING。",
        "Creation mode: 1 CREATE_NEW, 2 CREATE_ALWAYS, 3 OPEN_EXISTING, 4 OPEN_ALWAYS, 5 TRUNCATE_EXISTING.",
    ),
    (
        "Windows 文件属性/标志。可输入十进制或 0x 十六进制。",
        "Windows file attributes/flags, in decimal or 0x hexadecimal.",
    ),
    (
        "以毫秒为单位。设置将在对应服务生命周期操作中生效。",
        "Time in milliseconds. Takes effect at the corresponding service lifecycle operation.",
    ),
    (
        "设置保存在 Windows 注册表与服务配置中。运行中的应用通常需要重启服务后才使用新配置。",
        "Saved in the Windows registry and service configuration. Restart a running service to apply changes.",
    ),
    (
        "XSM 自身日志级别：off 关闭、error 错误、warn 警告、info 信息、debug 调试、trace 跟踪。应用程序输出仍由 AppStdout / AppStderr 独立配置。",
        "XSM log level: off, error, warn, info, debug or trace. Configure application output separately with AppStdout / AppStderr.",
    ),
    (
        "XSM 自身日志文件路径。留空时仅使用 Windows 事件日志。服务账户必须有目标目录的写入权限。",
        "Log path. Leave empty for Windows Event Log only. The service account needs write access.",
    ),
    (
        "XSM 自身日志单文件大小上限，单位为字节。默认 10485760（10 MiB）。",
        "Maximum log size in bytes. Default: 10485760 (10 MiB).",
    ),
    (
        "XSM 自身日志轮转时保留的归档文件数量，默认 5。",
        "Number of log archives to retain. Default: 5.",
    ),
    (
        "新建服务：填写应用程序；高级设置可先暂存，点击安装后一起保存。",
        "New service: enter an application, stage advanced settings, then select Install service.",
    ),
    (
        "设置已暂存，点击“安装服务”后一起保存。",
        "Setting staged. Select Install service to save it with the new service.",
    ),
    (
        "已取消该条目的暂存设置。",
        "The staged setting has been cleared.",
    ),
    ("操作正在执行", "An operation is already running"),
    (
        "操作尚未完成，请稍候再关闭窗口。",
        "Please wait for the current operation before closing.",
    ),
    (
        "操作意外中止，请检查当前服务状态",
        "The operation stopped unexpectedly. Check the service state.",
    ),
    (
        "无法创建后台操作线程",
        "Could not start the operation thread",
    ),
    (
        "后台操作连接已关闭",
        "The operation channel closed unexpectedly",
    ),
    ("正在读取服务列表…", "Loading services…"),
    ("正在安装服务…", "Installing service…"),
    ("正在保存应用配置…", "Saving application configuration…"),
    ("正在保存设置…", "Saving setting…"),
    ("正在移除服务…", "Removing service…"),
    ("正在导出配置…", "Exporting configuration…"),
    ("正在执行服务操作…", "Running service operation…"),
    ("正在控制服务…", "Controlling service…"),
    (
        "正在请求管理员权限…",
        "Requesting administrator permission…",
    ),
    ("请填写服务名称", "Enter a service name"),
    ("请填写应用程序路径", "Enter an application path"),
    (
        "请填写退出码或事件/动作条目",
        "Enter an exit code or event/action pair",
    ),
    ("请先选择服务", "Select a service first"),
    ("请选择参数", "Select a setting"),
    ("选择应用程序", "Select application"),
    ("路径不是有效 UTF-16", "The path is not valid UTF-16"),
    (
        "文件路径不是有效 UTF-16",
        "The file path is not valid UTF-16",
    ),
    ("读取窗口消息失败", "Could not read window messages"),
    ("服务已安装。", "Service installed."),
    (
        "应用配置已保存；重启服务后生效。",
        "Application configuration saved. Restart the service to apply it.",
    ),
    ("服务管理器 — 操作失败", "XSM — Operation failed"),
    ("移除服务", "Remove service"),
    ("未保存的配置", "Unsaved configuration"),
    ("XSM 服务进程", "XSM service processes"),
    ("XSM 服务管理器", "XSM service manager"),
    ("XSM — Windows 服务管理器", "XSM — Windows service manager"),
    (
        "暂存的高级设置尚未安装，确定关闭吗？",
        "There are staged settings for an uninstalled service. Close anyway?",
    ),
    (
        "将打开新的管理员窗口。当前窗口中尚未保存的输入不会自动转移，是否继续？",
        "Open a new administrator window? Unsaved inputs in this window will not be transferred.",
    ),
    ("以管理员身份重启 XSM", "Run XSM as administrator"),
    (
        "管理员窗口已打开。当前窗口仍处于只读模式。",
        "Administrator window opened. This window remains read-only.",
    ),
    (
        "界面操作意外中止。请检查服务状态后重试。",
        "The UI operation stopped unexpectedly. Check the service state and try again.",
    ),
    (
        "选择左侧服务，然后点击移除。",
        "Select a service on the left, then select Remove.",
    ),
    (
        "XSM native UI test fixture · 尚未安装服务",
        "XSM native UI test fixture · Service not installed",
    ),
    ("重置", "reset"),
    ("保存", "saved"),
    (
        "管理员窗口已关闭。当前窗口仍处于只读模式。",
        "Administrator window closed. This window remains read-only.",
    ),
    ("已加载 {} 个服务", "Loaded {} services"),
    ("服务 {name} 已移除。", "Service {name} removed."),
    ("配置已保存：{path}", "Configuration saved: {path}"),
    ("已读取 {name} 的进程。", "Loaded processes for {name}."),
    ("{name}：{verb} 已完成。", "{name}: {verb} completed."),
    ("{setting} 已{}。", "{setting}: updated."),
    ("操作失败：{error:#}", "Operation failed: {error:#}"),
    (
        "无法读取设置：{error:#}",
        "Could not read setting: {error:#}",
    ),
    ("创建窗口失败：{}", "Could not create window: {}"),
    ("注册窗口失败：{}", "Could not register window: {}"),
    ("创建界面控件失败: {}", "Could not create control: {}"),
    (
        "打开保存窗口失败：{error}",
        "Could not open save dialog: {error}",
    ),
    (
        "打开文件选择器失败: {error}",
        "Could not open file picker: {error}",
    ),
    ("保存 {path}", "Saving {path}"),
    ("设置 {setting}", "Setting {setting}"),
    ("保存失败：{error:#}{}", "Save failed: {error:#}{}"),
    ("；回滚失败：{}", "; rollback failed: {}"),
    (
        "安装设置失败，已撤销新服务：{error:#}。请重新填写高级设置后重试。",
        "Configuration failed; the new service was removed: {error:#}. Re-enter advanced settings and retry.",
    ),
    (
        "安装设置失败：{error:#}；新服务 {name} 未能移除：{rollback:#}。请刷新并修正该服务。",
        "Configuration failed: {error:#}; could not remove new service {name}: {rollback:#}. Refresh and correct the service.",
    ),
    (
        "确定移除服务“{name}”吗？\n应用程序文件和日志将保留。正在运行的服务可能需要先停止。",
        "Remove service \"{name}\"?\nApplication files and logs will be retained. You may need to stop the service first.",
    ),
];

fn render_template(source: &str, pattern: &str, replacement: &str) -> Option<String> {
    let mut input = source;
    let mut template = pattern;
    let mut values = Vec::new();
    while let Some(open) = template.find('{') {
        let prefix = &template[..open];
        input = input.strip_prefix(prefix)?;
        let close = template[open..].find('}')? + open;
        let key = &template[open + 1..close];
        template = &template[close + 1..];
        let next = template.find('{').unwrap_or(template.len());
        let delimiter = &template[..next];
        let length = if delimiter.is_empty() {
            input.len()
        } else {
            input.find(delimiter)?
        };
        values.push((key, &input[..length]));
        input = &input[length..];
    }
    if input != template || values.is_empty() {
        return None;
    }
    // Parse only the template, never a substituted value. A service name or path
    // containing braces must not turn into another interpolation instruction.
    let mut output = String::new();
    let mut rest = replacement;
    while let Some(open) = rest.find('{') {
        output.push_str(&rest[..open]);
        let close = rest[open..].find('}')? + open;
        let key = &rest[open + 1..close];
        let index = values.iter().position(|(candidate, _)| *candidate == key)?;
        output.push_str(values.remove(index).1);
        rest = &rest[close + 1..];
    }
    output.push_str(rest);
    Some(output)
}
pub fn translate(source: &str, language: Language) -> String {
    if language == Language::ZhCn {
        if let Some((chinese, _)) = MESSAGES.iter().find(|(_, english)| *english == source) {
            return (*chinese).into();
        }
        return source.to_owned();
    }
    if let Some((_, english)) = MESSAGES.iter().find(|(chinese, _)| *chinese == source) {
        return (*english).into();
    }
    for (chinese, english) in MESSAGES {
        if let Some(output) = render_template(source, chinese, english) {
            return output;
        }
    }
    // A service-status line retains the service name verbatim.
    if let Some((name, status)) = source.rsplit_once(" · ")
        && let Some((_, english)) = MESSAGES.iter().find(|(chinese, _)| *chinese == status)
    {
        return format!("{name} · {english}");
    }
    source.into()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_messages_have_english_without_chinese_prose() {
        for (_, english) in MESSAGES {
            assert!(
                !english
                    .chars()
                    .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
                "{english}"
            );
        }
    }
    #[test]
    fn formatted_labels_preserve_opaque_values() {
        assert_eq!(
            translate("服务 服务甲 已移除。", Language::EnUs),
            "Service 服务甲 removed."
        );
        assert_eq!(
            translate("已加载 53 个服务", Language::EnUs),
            "Loaded 53 services"
        );
        assert_eq!(
            translate("服务甲 · 运行中", Language::EnUs),
            "服务甲 · Running"
        );
        assert_eq!(
            render_template(
                "name={path}; path=C:\\数据",
                "name={name}; path={path}",
                "{name}: {path}"
            ),
            Some("{path}: C:\\数据".into())
        );
    }
    #[test]
    fn every_setting_has_unique_reversible_bilingual_labels() {
        let canonical = crate::settings::SPECS
            .iter()
            .map(|spec| spec.name)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            canonical,
            vocabulary::SETTINGS.iter().map(|entry| entry.0).collect()
        );
        assert_eq!(canonical.len(), vocabulary::SETTINGS.len());
        for language in [Language::ZhCn, Language::EnUs] {
            let mut displayed = std::collections::BTreeSet::new();
            for spec in crate::settings::SPECS {
                let label = setting_label(spec.name, language);
                assert!(displayed.insert(label), "duplicate label: {label}");
                assert_eq!(setting_name(label, language), spec.name);
                assert!(!setting_help(spec.name, language).is_empty());
                if language == Language::ZhCn {
                    assert!(
                        label
                            .chars()
                            .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
                        "{label}"
                    );
                }
            }
        }
    }
    #[test]
    fn all_frontend_choices_and_hooks_roundtrip_without_protocol_labels() {
        for language in [Language::ZhCn, Language::EnUs] {
            for spec in crate::settings::SPECS {
                let mut labels = std::collections::BTreeSet::new();
                for raw in super::super::model::choices(spec.name) {
                    let label = option_label(spec.name, raw, language);
                    assert_ne!(label, *raw, "{}: {raw}", spec.name);
                    assert!(labels.insert(label));
                    assert_eq!(option_value(spec.name, label, language), *raw);
                }
            }
            for hook in crate::settings::HOOKS {
                let label = hook_label(hook, language);
                assert_ne!(label, *hook);
                assert_eq!(hook_value(label, language), *hook);
                assert_eq!(field_label("AppEvents", Some(hook), language), label);
            }
        }
    }
    #[test]
    fn account_choices_have_distinct_bilingual_labels() {
        for language in [Language::ZhCn, Language::EnUs] {
            let labels = super::super::model::LogonAccount::ALL
                .into_iter()
                .map(|choice| logon_account_label(choice, language))
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(labels.len(), 4);
            assert!(labels.iter().all(|label| !label.is_empty()));
        }
        assert_eq!(translate("停止", Language::EnUs), "Shutdown");
    }
    #[test]
    fn arbitrary_values_and_future_settings_remain_opaque() {
        for raw in [
            "Restart",
            "服务甲",
            "C:\\应用\\info.exe",
            "NAME=关闭",
            "unknown",
            "2",
        ] {
            for language in [Language::ZhCn, Language::EnUs] {
                assert_eq!(option_label("Application", raw, language), raw);
                assert_eq!(option_value("ObjectName", raw, language), raw);
                assert_eq!(option_value("NewSetting", raw, language), raw);
            }
        }
        assert_eq!(
            setting_label("FutureSetting", Language::ZhCn),
            "FutureSetting"
        );
        assert_eq!(option_label("AppNoConsole", "2", Language::ZhCn), "2");
        assert_eq!(
            field_label("AppExit", Some("Default"), Language::ZhCn),
            "退出动作 · 默认"
        );
        assert_eq!(
            field_label("AppExit", Some("99"), Language::EnUs),
            "Exit action · 99"
        );
    }
    #[test]
    fn english_ui_errors_have_chinese_presentations() {
        assert_eq!(
            translate("Invalid hook key", Language::ZhCn),
            "事件钩子条目无效"
        );
        assert_eq!(
            translate("Could not open the GitHub page", Language::ZhCn),
            "无法打开 GitHub 页面"
        );
    }
}
