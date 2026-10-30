//! Blocking backend operations. The view runs these on a worker and commits only success.
#![forbid(unsafe_code)]
use super::model::{self, Basic, Change, EditValues, Model};
use crate::{
    settings,
    windows::{registry, scm},
};
use anyhow::{Context, Result, bail, ensure};
use std::{collections::BTreeMap, path::PathBuf};

pub(super) enum Action {
    Refresh { all: bool, clear_selection: bool },
    Load { name: String, page: usize },
    LoadPage { page: usize },
    Apply { page: usize, reset: bool },
    SaveBasics,
    Control { verb: String },
    Remove,
    Processes,
    Export { path: PathBuf },
    Elevate,
}
pub(super) fn run(mut model: Model, action: Action) -> Result<Model> {
    match action {
        Action::Refresh {
            all,
            clear_selection,
        } => {
            refresh_using(&mut model, clear_selection, || scm::list(all))?;
        }
        Action::Load { name, page } => {
            let resolved = scm::resolve_name(&name)?;
            let (editable, managed) = service_capabilities(&resolved)?;
            load_service_using(
                &mut model,
                &resolved,
                managed,
                editable,
                page,
                read_value,
                || scm::status(&resolved),
            )?;
        }
        Action::LoadPage { page } => {
            load_page(&mut model, page)?;
        }
        Action::Apply { page, reset } => {
            ensure!(model.elevated, "需要管理员权限");
            ensure!(model.editable, "此服务类型仅支持查看配置");
            if page == 1 {
                save_basics(&mut model)?;
            } else if model.service.is_none() {
                model.stage_page(page, reset)?;
            } else {
                let service = model.service.as_ref().context("请先选择服务")?;
                let changes = model.changes(page, reset)?;
                if changes.is_empty() {
                    model.status = "本页没有未保存的更改。".into();
                } else {
                    apply_changes(service, &changes)?;
                    model.status = match load_page(&mut model, page) {
                        Ok(()) => "本页设置已保存；重启服务后生效。".into(),
                        Err(error) => {
                            format!("设置已保存，但重新读取失败：{error:#}。请重新读取本页。")
                        }
                    };
                }
            }
        }
        Action::SaveBasics => {
            save_basics(&mut model)?;
        }
        Action::Control { verb } => {
            ensure!(model.elevated, "需要管理员权限");
            let name = model.service.as_ref().context("请先选择服务")?;
            model.state = scm::control(name, &verb, &[])?;
            model.status = format!("{} · {}", name, model::state_name(model.state));
        }
        Action::Remove => {
            ensure!(model.elevated, "需要管理员权限");
            let name = model.service.as_ref().context("请先选择服务")?.clone();
            scm::remove(&name)?;
            model.new_service(None);
            model.services.retain(|service| service != &name);
            model.status = format!("服务 {name} 已移除。");
        }
        Action::Processes => {
            let name = model.service.as_ref().context("请先选择服务")?;
            model.details = Some(scm::processes(name)?);
            model.status = format!("已读取 {name} 的进程。");
        }
        Action::Export { path } => {
            let name = model.service.as_ref().context("请先选择服务")?;
            std::fs::write(&path, settings::dump(name, None)?)
                .with_context(|| format!("保存 {}", path.display()))?;
            model.status = format!("配置已保存：{}", path.display());
        }
        Action::Elevate => {
            crate::windows::elevation::launch(&["install".into()])?;
            model.status = "管理员窗口已打开。当前窗口仍处于只读模式。".into();
        }
    }
    Ok(model)
}

fn refresh_using(
    model: &mut Model,
    clear_selection: bool,
    list: impl FnOnce() -> Result<Vec<String>>,
) -> Result<()> {
    // Do not discard the current selection or drafts unless enumeration succeeds.
    let services = list()?;
    if clear_selection {
        model.new_service(None);
    }
    model.services = services;
    model.status = format!("已加载 {} 个服务", model.services.len());
    Ok(())
}

fn service_capabilities(service: &str) -> Result<(bool, bool)> {
    // A denied/malformed registry read is a real load failure, not evidence that
    // the selected service is native. Preserve the previous form on that path.
    let config = scm::query_config(service)?;
    if config.service_type & 0x10 == 0 {
        return Ok((false, false));
    }
    let service_key = registry::service_key(service, false)?;
    match service_key.open_subkey("Parameters") {
        Ok(key) => Ok((true, registry::string(&key, "Application")?.is_some())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok((true, false)),
        Err(error) => Err(error.into()),
    }
}

fn read_exit_rows(service: &str) -> Result<String> {
    let parameters = registry::parameters(service, false)?;
    let key = match parameters.open_subkey("AppExit") {
        Ok(key) => key,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => return Err(error.into()),
    };
    let mut rows = BTreeMap::new();
    for value in key.enum_values() {
        let (code, _) = value?;
        if !code.is_empty() {
            rows.insert(code.clone(), settings::get(service, "AppExit", &[code])?);
        }
    }
    Ok(rows
        .into_iter()
        .map(|(code, action)| format!("{code}={action}"))
        .collect::<Vec<_>>()
        .join("\r\n"))
}
fn read_value(service: &str, name: &str, key: &[String], exits: bool) -> Result<String> {
    if exits {
        read_exit_rows(service)
    } else {
        settings::get(service, name, key)
    }
}
fn load_service_using(
    model: &mut Model,
    name: &str,
    managed: bool,
    editable: bool,
    page: usize,
    mut get: impl FnMut(&str, &str, &[String], bool) -> Result<String>,
    status: impl FnOnce() -> Result<u32>,
) -> Result<()> {
    let mut loaded = Model::new(Some(name.into()), model.elevated);
    loaded.loaded.clear();
    loaded.service = Some(name.into());
    loaded.managed = managed;
    loaded.editable = editable;
    loaded.active_page = page;
    loaded.services = model.services.clone();
    if managed {
        loaded.basic.application = get(name, "Application", &[], false)?;
        loaded.basic.directory = get(name, "AppDirectory", &[], false)?;
        loaded.basic.parameters = get(name, "AppParameters", &[], false)?;
    }
    loaded.basic_initial = loaded.basic.clone();
    loaded.state = status()?;
    load_page_using(&mut loaded, page, get)?;
    loaded.status = format!("{} · {}", name, model::state_name(loaded.state));
    *model = loaded;
    Ok(())
}
pub(super) fn load_page(model: &mut Model, page: usize) -> Result<()> {
    load_page_using(model, page, read_value)
}
fn load_page_using(
    model: &mut Model,
    page: usize,
    mut get: impl FnMut(&str, &str, &[String], bool) -> Result<String>,
) -> Result<()> {
    ensure!((1..=12).contains(&page), "无效设置页面");
    model.loaded.remove(&page);
    let Some(service) = model.service.as_deref() else {
        model.loaded.insert(page);
        return Ok(());
    };
    let mut loaded = vec![];
    if page == 1 && model.managed {
        let basic = Basic {
            name: service.into(),
            application: get(service, "Application", &[], false)?,
            directory: get(service, "AppDirectory", &[], false)?,
            parameters: get(service, "AppParameters", &[], false)?,
        };
        model.basic = basic.clone();
        model.basic_initial = basic;
    }
    for (index, field) in model
        .fields
        .iter()
        .enumerate()
        .filter(|(_, field)| model::category(field.spec.name) == page)
    {
        let value = if model.managed || field.spec.native {
            get(service, field.spec.name, &field.key, field.exits)?
        } else {
            field.spec.default.into()
        };
        loaded.push((index, value));
    }
    for (index, value) in loaded {
        model.fields[index].raw = value.clone();
        model.fields[index].initial = value;
    }
    if page == 3 {
        crate::secret::wipe_string(&mut model.password);
        model.reset_logon_account_selection();
    }
    model.loaded.insert(page);
    Ok(())
}
fn original_value(service: &str, change: &Change) -> Result<Option<String>> {
    let (path, value) = match change.name {
        "AppExit" => (
            "AppExit".into(),
            if change.key[0] == "Default" {
                String::new()
            } else {
                change.key[0].clone()
            },
        ),
        "AppEvents" => {
            let (event, action) = change.key[0].split_once('/').context("Invalid hook key")?;
            (format!("AppEvents\\{event}"), action.into())
        }
        _ => return Ok(Some(settings::get(service, change.name, &change.key)?)),
    };
    let root = registry::parameters(service, false)?;
    match root.open_subkey(path) {
        Ok(sub) => registry::string(&sub, &value),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn write(service: &str, change: &Change) -> Result<()> {
    if change.reset {
        settings::reset(service, change.name, &change.key)
    } else {
        let mut args = change.key.clone();
        args.extend(change.values.0.iter().cloned());
        settings::set(service, change.name, &EditValues(args).0)
    }
}
fn apply_changes(service: &str, changes: &[Change]) -> Result<()> {
    let previous = changes
        .iter()
        .map(|change| original_value(service, change))
        .collect::<Result<Vec<_>>>()?;
    write_changes(changes, &previous, |change| write(service, change))
}
fn write_changes(
    changes: &[Change],
    previous: &[Option<String>],
    mut write: impl FnMut(&Change) -> Result<()>,
) -> Result<()> {
    ensure!(changes.len() == previous.len(), "配置快照不完整");
    for (index, change) in changes.iter().enumerate() {
        if let Err(error) = write(change) {
            let mut failures = vec![];
            // Start uses two native configuration calls. The second can fail
            // after the first changed state, so restore that failing field too.
            let end = index + usize::from(change.name == "Start");
            for prior in (0..end).rev() {
                let change = &changes[prior];
                let restored = Change {
                    name: change.name,
                    key: change.key.clone(),
                    reset: previous[prior].is_none(),
                    values: EditValues(match &previous[prior] {
                        None => vec![],
                        Some(value) if model::is_list(settings::find(change.name)?) => {
                            model::replacement_values(value)
                        }
                        Some(value) => vec![value.clone()],
                    }),
                };
                if let Err(error) = write(&restored) {
                    failures.push(error.to_string());
                }
            }
            bail!(
                "保存失败：{error:#}{}",
                if failures.is_empty() {
                    String::new()
                } else {
                    format!("；回滚失败：{}", failures.join("；"))
                }
            );
        }
    }
    Ok(())
}
fn save_basics(model: &mut Model) -> Result<()> {
    ensure!(model.elevated, "需要管理员权限");
    ensure!(model.managed, "原生服务不支持应用配置");
    ensure!(!model.basic.name.trim().is_empty(), "请填写服务名称");
    ensure!(
        !model.basic.application.trim().is_empty(),
        "请填写应用程序路径"
    );
    if let Some(service) = model.service.as_deref() {
        ensure!(model.loaded.contains(&1), "请先成功读取本页设置");
        let changes = [
            ("Application", &model.basic.application),
            ("AppDirectory", &model.basic.directory),
            ("AppParameters", &model.basic.parameters),
        ]
        .into_iter()
        .map(|(name, value)| Change {
            name,
            key: vec![],
            values: EditValues(vec![value.clone()]),
            reset: false,
        })
        .collect::<Vec<_>>();
        apply_changes(service, &changes)?;
        model.basic_initial = model.basic.clone();
        model.status = "应用配置已保存；重启服务后生效。".into();
    } else {
        let mut changes = vec![];
        for page in model.loaded.iter().copied().filter(|page| *page != 1) {
            changes.extend(model.changes(page, false)?);
        }
        let name = &model.basic.name;
        scm::install(name, &model.basic.application, &model.basic.parameters)?;
        let applied = (|| -> Result<()> {
            if !model.basic.directory.is_empty() {
                settings::set(
                    name,
                    "AppDirectory",
                    std::slice::from_ref(&model.basic.directory),
                )?;
            }
            for change in &changes {
                write(name, change)?;
            }
            Ok(())
        })();
        if let Err(error) = applied {
            match scm::remove(name) {
                Ok(()) => {
                    bail!("安装设置失败，已撤销新服务：{error:#}。请重新填写高级设置后重试。")
                }
                Err(rollback) => bail!(
                    "安装设置失败：{error:#}；新服务 {name} 未能移除：{rollback:#}。请刷新并修正该服务。"
                ),
            }
        }
        model.service = Some(name.clone());
        if !model.services.contains(name) {
            model.services.push(name.clone());
            model.services.sort();
        }
        model.basic_initial = model.basic.clone();
        for field in &mut model.fields {
            field.initial = field.raw.clone();
        }
        crate::secret::wipe_string(&mut model.password);
        model.state = 1;
        model.status = "服务已安装。".into();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refresh_draft(name: &str, elevated: bool) -> Model {
        let mut model = Model::new(Some(name.into()), elevated);
        model.service = Some(name.into());
        model.managed = false;
        model.editable = false;
        model.services = vec![name.into(), "old-other-service".into()];
        model.active_page = 11;
        model.basic.application = "C:\\apps\\worker.exe".into();
        model.basic.directory = "C:\\apps".into();
        model.basic.parameters = "--unsaved arguments".into();
        for (index, field) in model.fields.iter_mut().enumerate() {
            field.raw = format!("draft-{index}");
            field.initial = format!("saved-{index}");
        }
        model.password = "test-only-secret".into();
        model.loaded = [1, 3, 11].into_iter().collect();
        model.state = 4;
        model.status = "previous status".into();
        model.details = Some("previous process details".into());
        model
    }

    fn assert_same_model(actual: &Model, expected: &Model) {
        assert_eq!(actual.service, expected.service);
        assert_eq!(actual.managed, expected.managed);
        assert_eq!(actual.editable, expected.editable);
        assert_eq!(actual.elevated, expected.elevated);
        assert_eq!(actual.services, expected.services);
        assert_eq!(actual.active_page, expected.active_page);
        assert!(actual.basic == expected.basic, "basic form changed");
        assert!(
            actual.basic_initial == expected.basic_initial,
            "basic form baseline changed"
        );
        assert_eq!(actual.fields.len(), expected.fields.len());
        for (actual, expected) in actual.fields.iter().zip(&expected.fields) {
            assert_eq!(actual.spec.name, expected.spec.name);
            assert_eq!(actual.key, expected.key);
            assert_eq!(actual.raw, expected.raw);
            assert_eq!(actual.initial, expected.initial);
            assert_eq!(actual.exits, expected.exits);
        }
        assert_eq!(actual.password, expected.password);
        assert_eq!(actual.loaded, expected.loaded);
        assert_eq!(actual.state, expected.state);
        assert_eq!(actual.status, expected.status);
        assert_eq!(actual.details, expected.details);
    }

    #[test]
    fn explicit_refresh_clears_selection_and_drafts_after_listing_succeeds() {
        for elevated in [false, true] {
            let mut model = refresh_draft("selected", elevated);
            refresh_using(&mut model, true, || {
                Ok(vec!["new-first".into(), "new-second".into()])
            })
            .unwrap();

            let mut expected = Model::new(None, elevated);
            expected.services = vec!["new-first".into(), "new-second".into()];
            expected.status = "已加载 2 个服务".into();
            assert_same_model(&model, &expected);
            assert!(!model.dirty());
        }
    }

    #[test]
    fn failed_refresh_preserves_all_model_state_and_drafts() {
        for clear_selection in [false, true] {
            let mut model = refresh_draft("selected", true);
            let previous = model.clone();
            let error = refresh_using(&mut model, clear_selection, || {
                bail!("injected service enumeration failure")
            })
            .unwrap_err();

            assert_eq!(error.to_string(), "injected service enumeration failure");
            assert_same_model(&model, &previous);
        }
    }

    #[test]
    fn startup_and_filter_refresh_preserve_prefilled_name_selection_and_drafts() {
        for existing_service in [false, true] {
            let mut model = refresh_draft("install-prefill", true);
            if !existing_service {
                model.service = None;
            }
            let mut expected = model.clone();
            expected.services = vec!["new-list-entry".into()];
            expected.status = "已加载 1 个服务".into();

            refresh_using(&mut model, false, || Ok(vec!["new-list-entry".into()])).unwrap();

            assert_same_model(&model, &expected);
            assert!(model.dirty());
        }
    }

    #[test]
    fn failed_switch_preserves_identity_every_draft_and_password() {
        let mut model = Model::new(Some("old".into()), true);
        model.service = Some("old".into());
        model.basic.parameters = "unsaved arguments".into();
        model.fields[0].raw = "unsaved field".into();
        model.password = "test-only-secret".into();
        let failed = load_service_using(
            &mut model,
            "new",
            true,
            true,
            2,
            |_, name, _, _| {
                if name == "AppParameters" {
                    bail!("injected read error");
                }
                Ok("new data".into())
            },
            || Ok(4),
        );
        assert!(failed.is_err());
        assert_eq!(model.service.as_deref(), Some("old"));
        assert_eq!(model.basic.parameters, "unsaved arguments");
        assert_eq!(model.fields[0].raw, "unsaved field");
        assert_eq!(model.password, "test-only-secret");
        // Also cover failure after all basics/status succeeded, in the target page.
        assert!(
            load_service_using(
                &mut model,
                "new",
                true,
                true,
                2,
                |_, name, _, _| {
                    if name == "Description" {
                        bail!("injected page error");
                    }
                    Ok("new data".into())
                },
                || Ok(4)
            )
            .is_err()
        );
        assert_eq!(model.service.as_deref(), Some("old"));
        assert_eq!(model.password, "test-only-secret");
    }
    #[test]
    fn partial_page_read_never_replaces_fields_and_disables_save() {
        let mut model = Model::new(None, true);
        model.service = Some("example".into());
        for field in &mut model.fields {
            field.raw = "draft".into();
        }
        let mut calls = 0;
        assert!(
            load_page_using(&mut model, 2, |_, _, _, _| {
                calls += 1;
                if calls == 2 {
                    bail!("injected");
                }
                Ok("read".into())
            })
            .is_err()
        );
        assert!(model.fields.iter().all(|field| field.raw == "draft"));
        assert!(!model.loaded.contains(&2));
        assert!(model.changes(2, false).is_err());
    }
    #[test]
    fn refreshing_a_page_preserves_password_and_other_page_drafts() {
        let mut model = Model::new(None, true);
        model.service = Some("example".into());
        model.password = "test-only-secret".into();
        let process = model
            .fields
            .iter()
            .position(|field| field.spec.name == "AppNoConsole")
            .unwrap();
        model.fields[process].raw = "1".into();
        load_page_using(&mut model, 2, |_, _, _, _| Ok("read".into())).unwrap();
        assert_eq!(model.password, "test-only-secret");
        assert_eq!(model.fields[process].raw, "1");
        assert!(load_page_using(&mut model, 3, |_, _, _, _| bail!("injected")).is_err());
        assert_eq!(model.password, "test-only-secret");
        load_page_using(&mut model, 3, |_, _, _, _| Ok("LocalSystem".into())).unwrap();
        assert!(model.password.is_empty());
    }
    fn change(name: &'static str, value: &str) -> Change {
        Change {
            name,
            key: vec![],
            values: EditValues(vec![value.into()]),
            reset: false,
        }
    }
    #[test]
    fn rollback_preserves_group_replacement_and_missing_hook_key() {
        let mut hook = change("AppEvents", "new command");
        hook.key = vec!["Start/Pre".into()];
        let changes = vec![
            change("DependOnGroup", ":new"),
            hook,
            change("Description", "reject"),
        ];
        let mut writes = vec![];
        assert!(
            write_changes(
                &changes,
                &[
                    Some("+Original\r\nSecond".into()),
                    None,
                    Some("old description".into())
                ],
                |change| {
                    writes.push((change.name, change.values.0.clone(), change.reset));
                    if change.name == "Description" {
                        bail!("injected");
                    }
                    Ok(())
                }
            )
            .is_err()
        );
        assert_eq!(writes[3], ("AppEvents", vec![], true));
        assert_eq!(
            writes[4],
            (
                "DependOnGroup",
                vec![":+Original".into(), "Second".into()],
                false
            )
        );
    }
    #[test]
    fn startup_partial_failure_restores_the_failing_field() {
        let mut writes = vec![];
        assert!(
            write_changes(
                &[change("Start", "SERVICE_DELAYED_AUTO_START")],
                &[Some("SERVICE_DEMAND_START".into())],
                |change| {
                    writes.push(change.values.0[0].clone());
                    if writes.len() == 1 {
                        bail!("second native call failed");
                    }
                    Ok(())
                }
            )
            .is_err()
        );
        assert_eq!(
            writes,
            ["SERVICE_DELAYED_AUTO_START", "SERVICE_DEMAND_START"]
        );
    }
}
