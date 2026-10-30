//! UI-independent form state. Editable values always use the backend vocabulary.
#![forbid(unsafe_code)]
use crate::settings::{self, Kind, SPECS, Spec};
use anyhow::{Context, Result, ensure};
use std::collections::{BTreeMap, BTreeSet};

pub(super) use settings::HOOKS;
pub(super) const TAB_LABELS: &[&str] = &[
    "应用配置",
    "服务设置",
    "登录账户",
    "依赖项",
    "进程",
    "恢复",
    "停止",
    "输入输出",
    "日志轮转",
    "环境变量",
    "事件钩子",
    "日志",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LogonAccount {
    LocalSystem,
    LocalService,
    NetworkService,
    Custom,
}
impl LogonAccount {
    pub const ALL: [Self; 4] = [
        Self::LocalSystem,
        Self::LocalService,
        Self::NetworkService,
        Self::Custom,
    ];

    /// Recognize unambiguous built-in names without changing stored account text.
    /// Unknown/localized names stay editable and are resolved by the SCM backend.
    pub fn from_raw(raw: &str) -> Self {
        for (choice, aliases) in [
            (Self::LocalSystem, ["LocalSystem", "NT AUTHORITY\\SYSTEM"]),
            (
                Self::LocalService,
                ["NT AUTHORITY\\LocalService", "NT AUTHORITY\\LOCAL SERVICE"],
            ),
            (
                Self::NetworkService,
                [
                    "NT AUTHORITY\\NetworkService",
                    "NT AUTHORITY\\NETWORK SERVICE",
                ],
            ),
        ] {
            if aliases.iter().any(|alias| raw.eq_ignore_ascii_case(alias)) {
                return choice;
            }
        }
        Self::Custom
    }

    pub fn canonical_name(self) -> Option<&'static str> {
        match self {
            Self::LocalSystem => Some("LocalSystem"),
            Self::LocalService => Some("NT AUTHORITY\\LocalService"),
            Self::NetworkService => Some("NT AUTHORITY\\NetworkService"),
            Self::Custom => None,
        }
    }
}

pub(super) fn account_needs_password(raw: &str, service: &str) -> bool {
    LogonAccount::from_raw(raw) == LogonAccount::Custom
        && (service.is_empty()
            || !raw.split_once('\\').is_some_and(|(authority, name)| {
                crate::windows::registry::equal_name(authority, "NT SERVICE")
                    && crate::windows::registry::equal_name(name, service)
            }))
}
pub(super) fn category(name: &str) -> usize {
    match name {
        "Application" | "AppDirectory" | "AppParameters" => 1,
        "ObjectName" => 3,
        "DependOnService" | "DependOnGroup" => 4,
        "AppAffinity" | "AppPriority" | "AppNoConsole" => 5,
        "AppExit" | "AppRestartDelay" | "AppThrottle" => 6,
        "AppKillProcessTree" => 7,
        "AppEnvironment" | "AppEnvironmentExtra" | "Environment" => 10,
        "AppEvents" | "AppRedirectHook" => 11,
        name if name.starts_with("AppStopMethod") => 7,
        name if name.starts_with("AppRotate") || name == "AppTimestampLog" => 9,
        name if name.starts_with("AppStd") => 8,
        name if name.starts_with("XsmLog") => 12,
        _ => 2,
    }
}
pub(super) fn choices(name: &str) -> &'static [&'static str] {
    match name {
        "XsmLogLevel" => &["off", "error", "warn", "info", "debug", "trace"],
        "AppExit" => &["Restart", "Ignore", "Exit", "Suicide"],
        "AppPriority" => &[
            "REALTIME_PRIORITY_CLASS",
            "HIGH_PRIORITY_CLASS",
            "ABOVE_NORMAL_PRIORITY_CLASS",
            "NORMAL_PRIORITY_CLASS",
            "BELOW_NORMAL_PRIORITY_CLASS",
            "IDLE_PRIORITY_CLASS",
        ],
        "Start" => &[
            "SERVICE_AUTO_START",
            "SERVICE_DELAYED_AUTO_START",
            "SERVICE_DEMAND_START",
            "SERVICE_DISABLED",
        ],
        "Type" => &["SERVICE_WIN32_OWN_PROCESS", "SERVICE_INTERACTIVE_PROCESS"],
        "AppNoConsole"
        | "AppKillProcessTree"
        | "AppRedirectHook"
        | "AppRotateFiles"
        | "AppRotateOnline"
        | "AppTimestampLog"
        | "AppStdoutCopyAndTruncate"
        | "AppStderrCopyAndTruncate" => &["0", "1"],
        "AppStdinCreationDisposition"
        | "AppStdoutCreationDisposition"
        | "AppStderrCreationDisposition" => &["1", "2", "3", "4", "5"],
        _ => &[],
    }
}
pub(super) fn state_name(state: u32) -> &'static str {
    match state {
        1 => "已停止",
        2 => "正在启动",
        3 => "正在停止",
        4 => "运行中",
        5 => "正在继续",
        6 => "正在暂停",
        7 => "已暂停",
        _ => "未知状态",
    }
}
#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct Basic {
    pub name: String,
    pub application: String,
    pub directory: String,
    pub parameters: String,
}
#[derive(Clone)]
pub(super) struct Field {
    pub spec: &'static Spec,
    pub key: Vec<String>,
    pub raw: String,
    pub initial: String,
    pub exits: bool,
}
impl Field {
    pub fn multiline(&self) -> bool {
        self.exits || is_list(self.spec) || self.spec.name == "Description"
    }
}
#[derive(Clone)]
pub(super) struct EditValues(pub Vec<String>);
impl Drop for EditValues {
    fn drop(&mut self) {
        for value in &mut self.0 {
            crate::secret::wipe_string(value);
        }
    }
}
#[derive(Clone)]
pub(super) struct Change {
    pub name: &'static str,
    pub key: Vec<String>,
    pub values: EditValues,
    pub reset: bool,
}
#[derive(Clone)]
pub(super) struct Model {
    pub service: Option<String>,
    pub managed: bool,
    pub editable: bool,
    pub elevated: bool,
    pub services: Vec<String>,
    pub active_page: usize,
    pub basic: Basic,
    pub basic_initial: Basic,
    pub fields: Vec<Field>,
    pub password: String,
    custom_account: Option<String>,
    selected_logon_account: Option<LogonAccount>,
    pub loaded: BTreeSet<usize>,
    pub state: u32,
    pub status: String,
    pub details: Option<String>,
}
impl Drop for Model {
    fn drop(&mut self) {
        crate::secret::wipe_string(&mut self.password);
    }
}
impl Model {
    pub fn new(name: Option<String>, elevated: bool) -> Self {
        let basic = Basic {
            name: name.unwrap_or_default(),
            ..Default::default()
        };
        let fields = SPECS
            .iter()
            .filter(|spec| category(spec.name) != 1)
            .flat_map(|spec| {
                let keys = match spec.kind {
                    Kind::Hook => HOOKS.iter().map(|key| vec![(*key).into()]).collect(),
                    Kind::Exit => vec![vec!["Default".into()], vec!["*".into()]],
                    _ => vec![vec![]],
                };
                keys.into_iter()
                    .map(|key: Vec<String>| {
                        let exits = key.first().is_some_and(|key| key == "*");
                        let raw = if exits {
                            ""
                        } else {
                            match spec.name {
                                "Start" => "SERVICE_AUTO_START",
                                "Type" => "SERVICE_WIN32_OWN_PROCESS",
                                _ => spec.default,
                            }
                        }
                        .to_owned();
                        Field {
                            spec,
                            key,
                            initial: raw.clone(),
                            raw,
                            exits,
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        Self {
            service: None,
            managed: true,
            editable: true,
            elevated,
            services: vec![],
            active_page: 1,
            basic_initial: basic.clone(),
            basic,
            fields,
            password: String::new(),
            custom_account: None,
            selected_logon_account: None,
            loaded: (1..=12).collect(),
            state: 0,
            status: "新建服务：填写应用程序；高级设置可先暂存，点击安装后一起保存。".into(),
            details: None,
        }
    }
    pub fn new_service(&mut self, name: Option<String>) {
        let mut next = Self::new(name, self.elevated);
        next.services = std::mem::take(&mut self.services);
        *self = next;
    }
    pub fn logon_account(&self) -> LogonAccount {
        self.selected_logon_account.unwrap_or_else(|| {
            self.fields
                .iter()
                .find(|field| field.spec.name == "ObjectName")
                .map_or(LogonAccount::LocalSystem, |field| {
                    LogonAccount::from_raw(&field.raw)
                })
        })
    }
    pub fn logon_password_enabled(&self) -> bool {
        if self.selected_logon_account == Some(LogonAccount::Custom) {
            // While editing, a built-in or virtual name can be only a prefix of
            // the intended account. Preserve the input; changes() still omits
            // credentials when the completed name requires no password.
            return true;
        }
        let service = self.service.as_deref().unwrap_or(&self.basic.name);
        self.fields
            .iter()
            .find(|field| field.spec.name == "ObjectName")
            .is_some_and(|field| account_needs_password(&field.raw, service))
    }
    pub fn select_logon_account(&mut self, choice: LogonAccount) {
        let previous = self.logon_account();
        let Some(field) = self
            .fields
            .iter_mut()
            .find(|field| field.spec.name == "ObjectName")
        else {
            return;
        };
        if previous != choice {
            if previous == LogonAccount::Custom {
                self.custom_account = Some(field.raw.clone());
            }
            field.raw = choice.canonical_name().map_or_else(
                || self.custom_account.clone().unwrap_or_default(),
                str::to_owned,
            );
        }
        self.selected_logon_account = Some(choice);
        if choice != LogonAccount::Custom {
            crate::secret::wipe_string(&mut self.password);
        }
    }
    /// Call after successful backend account loading, never before a failed read.
    pub fn reset_logon_account_selection(&mut self) {
        self.selected_logon_account = None;
        self.custom_account = None;
    }
    pub fn prepare(&mut self, action: &super::ops::Action) -> Result<()> {
        use super::ops::Action;
        match action {
            Action::LoadPage { page } => {
                ensure!((1..=12).contains(page), "无效设置页面");
                self.active_page = *page;
                self.loaded.remove(page);
            }
            Action::Apply { page, .. } => {
                ensure!(self.loaded.contains(page), "请先成功读取本页设置");
            }
            _ => {}
        }
        Ok(())
    }
    pub fn can_edit(&self, field: &Field) -> bool {
        self.elevated
            && self.editable
            && self.loaded.contains(&category(field.spec.name))
            && field.spec.name != "Name"
            && (self.managed || field.spec.native)
    }
    pub fn dirty(&self) -> bool {
        self.basic != self.basic_initial
            || !self.password.is_empty()
            || self.fields.iter().any(|field| field.raw != field.initial)
    }
    pub fn changes(&self, page: usize, reset: bool) -> Result<Vec<Change>> {
        ensure!((1..=12).contains(&page), "无效设置页面");
        ensure!(self.editable, "此服务类型仅支持查看配置");
        ensure!(self.loaded.contains(&page), "请先成功读取本页设置");
        let mut changes = vec![];
        for field in self.fields.iter().filter(|field| {
            category(field.spec.name) == page
                && field.spec.name != "Name"
                && (self.managed || field.spec.native)
                && (!reset || can_reset(field.spec.name))
        }) {
            if field.exits {
                let old = exit_rows(&field.initial)?;
                let new = if reset {
                    BTreeMap::new()
                } else {
                    exit_rows(&field.raw)?
                };
                for code in old.keys().chain(new.keys()).collect::<BTreeSet<_>>() {
                    if old.get(code) != new.get(code) {
                        changes.push(Change {
                            name: "AppExit",
                            key: vec![code.clone()],
                            values: EditValues(vec![new.get(code).cloned().unwrap_or_default()]),
                            reset: !new.contains_key(code),
                        });
                    }
                }
            } else {
                let has_password = field.spec.name == "ObjectName"
                    && account_needs_password(
                        &field.raw,
                        self.service.as_deref().unwrap_or(&self.basic.name),
                    )
                    && !self.password.is_empty();
                if !reset && field.raw == field.initial && !has_password {
                    continue;
                }
                let mut values = if is_list(field.spec) {
                    replacement_values(&field.raw)
                } else {
                    vec![field.raw.clone()]
                };
                if has_password && !reset {
                    values.push(self.password.clone());
                }
                changes.push(Change {
                    name: field.spec.name,
                    key: field.key.clone(),
                    values: EditValues(values),
                    reset,
                });
            }
        }
        Ok(changes)
    }
    /// New forms already retain all pages. Applying one page never clears another.
    pub fn stage_page(&mut self, page: usize, reset: bool) -> Result<()> {
        let _validated = self.changes(page, reset)?;
        if reset {
            for field in self
                .fields
                .iter_mut()
                .filter(|field| category(field.spec.name) == page && can_reset(field.spec.name))
            {
                field.raw = if field.exits {
                    String::new()
                } else {
                    field.spec.default.into()
                };
            }
            if page == 3 {
                crate::secret::wipe_string(&mut self.password);
                self.reset_logon_account_selection();
            }
        }
        // Keep initial at its install default so subsequent installation includes
        // staged values. Account credentials stay only in the guarded Model.
        self.status = "设置已暂存，点击“安装服务”后一起保存。".into();
        Ok(())
    }
}
pub(super) fn can_reset(name: &str) -> bool {
    !matches!(name, "Name" | "ImagePath" | "Start" | "Type")
}
pub(super) fn is_list(spec: &Spec) -> bool {
    spec.kind == Kind::Multi || matches!(spec.name, "DependOnService" | "DependOnGroup")
}
pub(super) fn replacement_values(value: &str) -> Vec<String> {
    let mut result: Vec<String> = value.lines().map(str::to_owned).collect();
    if result.is_empty() {
        result.push(String::new());
    }
    result[0].insert(0, ':');
    result
}
pub(super) fn exit_rows(value: &str) -> Result<BTreeMap<String, String>> {
    let mut rows = BTreeMap::new();
    for row in value.lines().map(str::trim).filter(|row| !row.is_empty()) {
        let (code, action) = row
            .split_once('=')
            .context("退出码覆盖格式应为 code=action")?;
        let code = settings::parse_number(code.trim())?.to_string();
        let action = choices("AppExit")
            .iter()
            .find(|value| value.eq_ignore_ascii_case(action.trim()))
            .context("退出动作应为 Restart、Ignore、Exit 或 Suicide")?;
        ensure!(
            rows.insert(code, (*action).into()).is_none(),
            "退出码不能重复"
        );
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn field(model: &mut Model, name: &str) -> usize {
        model
            .fields
            .iter()
            .position(|field| field.spec.name == name)
            .unwrap()
    }
    #[test]
    fn catalogue_has_every_setting_and_all_hook_entries() {
        let model = Model::new(None, true);
        let names = model
            .fields
            .iter()
            .map(|field| field.spec.name)
            .chain(["Application", "AppDirectory", "AppParameters"])
            .collect::<BTreeSet<_>>();
        assert_eq!(names, SPECS.iter().map(|spec| spec.name).collect());
        let hooks = model
            .fields
            .iter()
            .filter(|field| field.spec.name == "AppEvents")
            .map(|field| field.key[0].as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(hooks, HOOKS.iter().copied().collect());
        assert!(
            SPECS
                .iter()
                .all(|spec| (1..=TAB_LABELS.len()).contains(&category(spec.name)))
        );
    }
    #[test]
    fn logon_selector_defaults_and_reads_aliases_without_changing_raw_values() {
        let mut model = Model::new(None, true);
        let account = field(&mut model, "ObjectName");
        assert_eq!(model.logon_account(), LogonAccount::LocalSystem);
        assert!(!model.logon_password_enabled());
        assert!(model.changes(3, false).unwrap().is_empty());

        for (raw, expected) in [
            ("localsystem", LogonAccount::LocalSystem),
            ("NT AUTHORITY\\SYSTEM", LogonAccount::LocalSystem),
            ("nt authority\\localservice", LogonAccount::LocalService),
            ("NT AUTHORITY\\LOCAL SERVICE", LogonAccount::LocalService),
            ("NT AUTHORITY\\NetworkService", LogonAccount::NetworkService),
            (
                "NT AUTHORITY\\NETWORK SERVICE",
                LogonAccount::NetworkService,
            ),
            (".\\LocalService", LogonAccount::Custom),
            ("DOMAIN\\NetworkService", LogonAccount::Custom),
            ("未知域\\账户", LogonAccount::Custom),
        ] {
            model.reset_logon_account_selection();
            model.fields[account].raw = raw.into();
            model.fields[account].initial = raw.into();
            assert_eq!(model.logon_account(), expected, "{raw}");
            model.select_logon_account(expected);
            assert_eq!(model.fields[account].raw, raw);
            assert!(model.changes(3, false).unwrap().is_empty());
        }
    }
    #[test]
    fn choosing_an_account_only_changes_the_selected_account_and_its_password() {
        let mut model = Model::new(None, true);
        let account = field(&mut model, "ObjectName");
        let description = field(&mut model, "Description");
        model.fields[description].raw = "keep this unrelated draft".into();
        model.select_logon_account(LogonAccount::Custom);
        assert_eq!(model.fields[account].raw, "");
        model.fields[account].raw = ".\\worker".into();
        model.password = "test-only-secret".into();

        model.select_logon_account(LogonAccount::Custom);
        assert_eq!(model.fields[account].raw, ".\\worker");
        assert_eq!(model.password, "test-only-secret");
        assert_eq!(
            model.changes(3, false).unwrap()[0].values.0,
            [".\\worker", "test-only-secret"]
        );

        model.select_logon_account(LogonAccount::NetworkService);
        assert!(model.password.is_empty());
        assert_eq!(
            model.changes(3, false).unwrap()[0].values.0,
            ["NT AUTHORITY\\NetworkService"]
        );
        model.select_logon_account(LogonAccount::LocalService);
        assert_eq!(
            model.changes(3, false).unwrap()[0].values.0,
            ["NT AUTHORITY\\LocalService"]
        );
        model.select_logon_account(LogonAccount::Custom);
        assert_eq!(model.fields[account].raw, ".\\worker");
        assert!(model.password.is_empty());
        assert_eq!(model.fields[description].raw, "keep this unrelated draft");
        assert_eq!(model.fields[account].initial, "LocalSystem");

        model.select_logon_account(LogonAccount::LocalSystem);
        model.stage_page(3, true).unwrap();
        model.select_logon_account(LogonAccount::Custom);
        assert!(model.fields[account].raw.is_empty());

        model.new_service(None);
        model.select_logon_account(LogonAccount::Custom);
        assert!(model.fields[account].raw.is_empty());
    }
    #[test]
    fn account_changes_never_submit_passwords_for_builtin_accounts() {
        for raw in [
            "LocalSystem",
            "NT AUTHORITY\\SYSTEM",
            "NT AUTHORITY\\LocalService",
            "NT AUTHORITY\\NETWORK SERVICE",
        ] {
            let mut model = Model::new(None, true);
            let account = field(&mut model, "ObjectName");
            model.fields[account].initial = "DOMAIN\\previous".into();
            model.fields[account].raw = raw.into();
            model.password = "stale-test-secret".into();
            assert!(!model.logon_password_enabled(), "{raw}");
            assert_eq!(model.changes(3, false).unwrap()[0].values.0, [raw]);
            model.fields[account].initial = raw.into();
            assert!(model.changes(3, false).unwrap().is_empty());
            model.select_logon_account(model.logon_account());
            assert!(model.password.is_empty());
        }
        for raw in [".\\worker", "DOMAIN\\worker", "NT SERVICE \\worker"] {
            assert!(account_needs_password(raw, "worker"), "{raw}");
        }
    }
    #[test]
    fn virtual_account_password_policy_matches_only_the_current_service_name() {
        for (service, raw, needs_password) in [
            ("Example", "NT SERVICE\\Example", false),
            ("Example", "nt service\\example", false),
            ("服务Ä", "NT SERVICE\\服务ä", false),
            ("Example", "NT SERVICE\\Other", true),
            ("", "NT SERVICE\\Example", true),
            ("", "NT SERVICE\\", true),
            ("Example", "NT SERVICE \\Example", true),
        ] {
            assert_eq!(
                account_needs_password(raw, service),
                needs_password,
                "{service}: {raw}"
            );
            for existing in [false, true] {
                let mut model = Model::new(Some(service.into()), true);
                if existing {
                    model.service = Some(service.into());
                    // The selected service's canonical identity has precedence
                    // over editable install-form text.
                    model.basic.name = "DifferentDraftName".into();
                }
                let account = field(&mut model, "ObjectName");
                model.fields[account].raw = raw.into();
                model.password = "test-only-secret".into();
                assert_eq!(model.logon_account(), LogonAccount::Custom);
                assert_eq!(model.logon_password_enabled(), needs_password);
                let expected = if needs_password {
                    vec![raw, "test-only-secret"]
                } else {
                    vec![raw]
                };
                assert_eq!(model.changes(3, false).unwrap()[0].values.0, expected);
                model.select_logon_account(LogonAccount::Custom);
                assert_eq!(model.fields[account].raw, raw);
                assert!(model.logon_password_enabled());
                assert_eq!(model.password, "test-only-secret");
                assert_eq!(model.changes(3, false).unwrap()[0].values.0, expected);
            }
        }
    }
    #[test]
    fn custom_account_typing_keeps_the_selected_mode_and_password_at_builtin_prefixes() {
        for name in [
            "LocalSystemWorker",
            "NT AUTHORITY\\LocalServiceWorker",
            "NT AUTHORITY\\NetworkServiceWorker",
            "NT SERVICE\\ExampleWorker",
        ] {
            let mut model = Model::new(Some("Example".into()), true);
            let account = field(&mut model, "ObjectName");
            model.select_logon_account(LogonAccount::Custom);
            model.password = "test-only-secret".into();
            for character in name.chars() {
                model.fields[account].raw.push(character);
                assert_eq!(model.logon_account(), LogonAccount::Custom);
                assert!(model.logon_password_enabled());
                model.select_logon_account(LogonAccount::Custom);
                assert_eq!(model.password, "test-only-secret");
            }
            assert_eq!(model.fields[account].raw, name);
            assert_eq!(
                model.changes(3, false).unwrap()[0].values.0,
                [name, "test-only-secret"]
            );
        }
    }
    #[test]
    fn backend_load_and_form_reset_clear_explicit_logon_selection() {
        let mut model = Model::new(None, true);
        let account = field(&mut model, "ObjectName");
        model.select_logon_account(LogonAccount::Custom);
        model.fields[account].raw = "NT AUTHORITY\\NetworkService".into();
        model.fields[account].initial = model.fields[account].raw.clone();
        assert_eq!(model.logon_account(), LogonAccount::Custom);
        model.reset_logon_account_selection();
        assert_eq!(model.logon_account(), LogonAccount::NetworkService);
        assert!(!model.logon_password_enabled());

        model.select_logon_account(LogonAccount::Custom);
        model.stage_page(3, true).unwrap();
        assert!(model.selected_logon_account.is_none());
        assert_eq!(model.logon_account(), LogonAccount::LocalSystem);
        model.select_logon_account(LogonAccount::Custom);
        model.new_service(None);
        assert!(model.selected_logon_account.is_none());
        assert_eq!(model.logon_account(), LogonAccount::LocalSystem);
    }
    #[test]
    fn custom_password_change_is_kept_when_the_account_name_is_unchanged() {
        let mut model = Model::new(None, true);
        let account = field(&mut model, "ObjectName");
        model.fields[account].initial = "DOMAIN\\worker".into();
        model.fields[account].raw = "DOMAIN\\worker".into();
        model.password = "replacement-test-secret".into();
        assert_eq!(
            model.changes(3, false).unwrap()[0].values.0,
            ["DOMAIN\\worker", "replacement-test-secret"]
        );
        let reset = model.changes(3, true).unwrap();
        assert!(reset[0].reset);
        assert_eq!(reset[0].values.0, ["DOMAIN\\worker"]);
    }
    #[test]
    fn lists_replace_and_description_preserves_newlines() {
        let mut model = Model::new(None, true);
        let description = field(&mut model, "Description");
        model.fields[description].raw = "第一行\r\nsecond line".into();
        assert_eq!(
            model.changes(2, false).unwrap()[0].values.0,
            ["第一行\r\nsecond line"]
        );
        let group = field(&mut model, "DependOnGroup");
        model.fields[group].raw = "+GroupA\nGroupB".into();
        assert_eq!(
            model.changes(4, false).unwrap()[0].values.0,
            [":+GroupA", "GroupB"]
        );
        assert_eq!(replacement_values(""), [":"]);
    }
    #[test]
    fn staging_and_install_snapshot_preserve_other_pages_and_password() {
        let mut model = Model::new(Some("example".into()), true);
        let description = field(&mut model, "Description");
        let account = field(&mut model, "ObjectName");
        model.fields[description].raw = "description draft".into();
        model.fields[account].raw = ".\\testuser".into();
        model.password = "test-only-secret".into();
        model.stage_page(2, false).unwrap();
        assert_eq!(model.password, "test-only-secret");
        model.stage_page(3, false).unwrap();
        assert_eq!(model.fields[description].raw, "description draft");
        assert_eq!(
            model.changes(3, false).unwrap()[0].values.0,
            [".\\testuser", "test-only-secret"]
        );
        assert_eq!(
            model.changes(2, false).unwrap()[0].values.0,
            ["description draft"]
        );
        assert!(model.dirty());
    }
    #[test]
    fn failed_page_is_not_saveable_and_native_fields_are_gated() {
        let mut model = Model::new(None, true);
        model
            .prepare(&super::super::ops::Action::LoadPage { page: 5 })
            .unwrap();
        assert!(model.changes(5, false).is_err());
        assert!(
            model
                .prepare(&super::super::ops::Action::Apply {
                    page: 5,
                    reset: false
                })
                .is_err()
        );
        model.managed = false;
        assert!(
            model
                .fields
                .iter()
                .filter(|field| !field.spec.native)
                .all(|field| !model.can_edit(field))
        );
        model.editable = false;
        assert!(model.fields.iter().all(|field| !model.can_edit(field)));
        assert!(model.changes(2, false).is_err());
    }
    #[test]
    fn exit_rows_canonicalize_and_reject_duplicate_codes() {
        assert_eq!(
            exit_rows("0=restart\n0xA=EXIT").unwrap(),
            BTreeMap::from([("0".into(), "Restart".into()), ("10".into(), "Exit".into())])
        );
        assert!(exit_rows("10=Exit\n0xA=Ignore").is_err());
        assert!(exit_rows("2=unknown").is_err());
        assert!(exit_rows("2").is_err());
    }
    #[test]
    fn new_service_clears_password_and_all_previous_drafts() {
        let mut model = Model::new(None, true);
        model.password = "secret".into();
        model.fields[0].raw = "draft".into();
        model.new_service(Some("next".into()));
        assert!(model.password.is_empty());
        assert!(!model.dirty());
        assert_eq!(model.basic.name, "next");
    }
}
