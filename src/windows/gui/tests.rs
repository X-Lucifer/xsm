use super::*;
fn input(width: f32, height: f32, scale: f32) -> egui::RawInput {
    let mut input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, height))),
        ..Default::default()
    };
    input
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .native_pixels_per_point = Some(scale);
    input
}
#[test]
fn every_page_has_stable_aligned_geometry_in_both_languages_and_all_scales() {
    let ctx = egui::Context::default();
    style::fonts(&ctx);
    let mut app = App::new(GuiMode::Install(Some("ExampleService".into())), true);
    app.model.services=vec!["A very long service name that should fit one row without wrapping or moving the following controls".into()];
    let long_name = "VeryLongDependencyServiceName".repeat(8);
    app.dependency_services = Some(vec![long_name.clone()]);
    app.dependency_choice = Some(long_name);
    app.model.select_logon_account(model::LogonAccount::Custom);
    app.model
        .fields
        .iter_mut()
        .find(|field| field.spec.name == "ObjectName")
        .unwrap()
        .raw = ".\\custom-user".into();
    for language in [Language::ZhCn, Language::EnUs] {
        for dark in [false, true] {
            app.language = language;
            app.dark = dark;
            style::configure(&ctx, dark);
            for scale in [1.0, 1.5, 2.0] {
                for size in [[1120.0, 820.0], [900.0, 520.0]] {
                    for page in 1..=12 {
                        app.model.active_page = page;
                        let output =
                            ctx.run_ui(input(size[0], size[1], scale), |ui| app.render(ui));
                        output.drop_without_applying_deltas();
                        let tabs: Vec<_> = app
                            .geometry
                            .iter()
                            .filter(|(id, _)| id.starts_with("tab-"))
                            .collect();
                        assert_eq!(tabs.len(), 12);
                        for (_, rect) in &tabs {
                            assert!(
                                (rect.height() - 32.0).abs() < 0.1,
                                "tab height {}",
                                rect.height()
                            );
                            assert!(rect.right() <= size[0] - 15.0);
                        }
                        for row in tabs.chunks(6) {
                            for pair in row.windows(2) {
                                assert!(
                                    pair[0].1.right() <= pair[1].1.left() + 0.1,
                                    "tabs overlap"
                                );
                            }
                        }
                        let fields: Vec<_> = app
                            .geometry
                            .iter()
                            .filter(|(id, _)| {
                                id.starts_with("field-")
                                    || [
                                        "Name",
                                        "Application",
                                        "AppDirectory",
                                        "AppParameters",
                                        "password",
                                    ]
                                    .contains(&id.as_str())
                            })
                            .collect();
                        for pair in fields.windows(2) {
                            assert!(
                                pair[0].1.bottom() <= pair[1].1.top() + 0.1,
                                "fields overlap page {page}"
                            );
                        }
                        for (id, rect) in &app.geometry {
                            assert!(
                                rect.is_finite() && rect.width() > 0.0 && rect.height() > 0.0,
                                "{id}"
                            );
                            if id == "service-action"
                                || id == "Browse"
                                || id == "BrowseDirectory"
                                || id == "Application"
                                || id == "AppDirectory"
                                || id == "dependency-services"
                                || id == "dependency-add"
                                || id == "dependency-refresh"
                                || id == "service-row"
                            {
                                assert!(
                                    (rect.height() - 32.0).abs() < 0.1,
                                    "{id} actual height {}",
                                    rect.height()
                                );
                            }
                            if id == "BrowseDirectory"
                                || id.starts_with("dependency-")
                                || id == "logon-name"
                            {
                                assert!(rect.right() <= size[0] - 16.0, "{id} exceeds form width");
                            }
                        }
                    }
                }
            }
        }
    }
}
#[test]
fn idle_frames_do_not_schedule_animation_or_polling_and_language_keeps_raw_values() {
    let ctx = egui::Context::default();
    style::fonts(&ctx);
    style::configure(&ctx, false);
    let mut app = App::new(GuiMode::Install(None), true);
    app.model.password = "pending-secret".into();
    app.model
        .fields
        .iter_mut()
        .find(|f| f.spec.name == "AppPriority")
        .unwrap()
        .raw = "HIGH_PRIORITY_CLASS".into();
    for language in [Language::ZhCn, Language::EnUs] {
        app.language = language;
        for _ in 0..4 {
            ctx.run_ui(input(1120.0, 820.0, 1.0), |ui| app.render(ui))
                .drop_without_applying_deltas();
        }
    }
    let output = ctx.run_ui(input(1120.0, 820.0, 1.0), |ui| app.render(ui));
    assert!(
        output.viewport_output[&egui::ViewportId::ROOT].repaint_delay
            > std::time::Duration::from_secs(1)
    );
    assert_eq!(app.model.password, "pending-secret");
    assert_eq!(
        app.model
            .fields
            .iter()
            .find(|f| f.spec.name == "AppPriority")
            .unwrap()
            .raw,
        "HIGH_PRIORITY_CLASS"
    );
    output.drop_without_applying_deltas();
}
#[test]
fn editor_state_is_cleared_on_new_service_without_changing_other_ui_state() {
    let ctx = egui::Context::default();
    let mut app = App::new(GuiMode::Install(None), true);
    let id = Id::unique(("basic", "Application"));
    egui::text_edit::TextEditState::default().store(&ctx, id);
    assert!(egui::text_edit::TextEditState::load(&ctx, id).is_some());
    app.perform(&ctx, Intent::New);
    assert!(egui::text_edit::TextEditState::load(&ctx, id).is_none());
}
#[test]
fn refresh_waits_for_unsaved_changes_and_resets_editor_history_on_success() {
    let ctx = egui::Context::default();
    let mut app = App::new(GuiMode::Install(None), true);
    app.model.basic.application = "draft.exe".into();
    let id = Id::unique(("basic", "Application"));
    egui::text_edit::TextEditState::default().store(&ctx, id);
    app.intent(&ctx, Intent::Refresh);
    assert!(matches!(app.pending, Some(Intent::Refresh)));
    assert!(app.busy.is_none());
    assert_eq!(app.model.basic.application, "draft.exe");
    assert!(egui::text_edit::TextEditState::load(&ctx, id).is_some());

    // Simulate the successful worker result, including the no-selection case
    // where comparing service identities alone would leave the draft undo state.
    app.pending = None;
    let (sender, receiver) = mpsc::channel();
    app.busy = Some(receiver);
    let mut refreshed = Model::new(None, true);
    refreshed.services = vec!["ExistingService".into()];
    sender
        .send(Ok(Completion::Model {
            model: Box::new(refreshed),
            reset_editors: true,
        }))
        .unwrap();
    app.poll(&ctx);
    assert!(app.busy.is_none());
    assert!(app.model.service.is_none());
    assert!(app.model.basic.application.is_empty());
    assert_eq!(app.model.services, ["ExistingService"]);
    assert!(egui::text_edit::TextEditState::load(&ctx, id).is_none());
}

#[test]
fn failed_refresh_keeps_selection_draft_and_editor_history() {
    let ctx = egui::Context::default();
    let mut app = App::new(GuiMode::Install(None), true);
    app.model.service = Some("SelectedService".into());
    app.model.basic.application = "draft.exe".into();
    let id = Id::unique(("basic", "Application"));
    egui::text_edit::TextEditState::default().store(&ctx, id);
    let (sender, receiver) = mpsc::channel();
    app.busy = Some(receiver);
    sender.send(Err(anyhow!("refresh failed"))).unwrap();
    app.poll(&ctx);
    assert!(app.busy.is_none());
    assert_eq!(app.model.service.as_deref(), Some("SelectedService"));
    assert_eq!(app.model.basic.application, "draft.exe");
    assert!(egui::text_edit::TextEditState::load(&ctx, id).is_some());
    assert_eq!(app.error.as_deref(), Some("refresh failed"));
}

fn deliver(app: &mut App, ctx: &egui::Context, completion: Completion) {
    let (sender, receiver) = mpsc::channel();
    app.busy = Some(receiver);
    sender.send(Ok(completion)).unwrap();
    app.poll(ctx);
}

#[test]
fn application_picker_tracks_default_directory_but_preserves_an_explicit_directory() {
    let ctx = egui::Context::default();
    let mut app = App::new(GuiMode::Install(None), true);
    deliver(
        &mut app,
        &ctx,
        Completion::Application(Some("C:\\Apps\\first.exe".into())),
    );
    assert_eq!(app.model.basic.directory, "C:\\Apps");
    deliver(
        &mut app,
        &ctx,
        Completion::Application(Some("D:\\Tools\\second.exe".into())),
    );
    assert_eq!(app.model.basic.directory, "D:\\Tools");
    deliver(
        &mut app,
        &ctx,
        Completion::Directory(Some("E:\\运行目录".into())),
    );
    deliver(
        &mut app,
        &ctx,
        Completion::Application(Some("C:\\Other\\third.exe".into())),
    );
    assert_eq!(app.model.basic.application, "C:\\Other\\third.exe");
    assert_eq!(app.model.basic.directory, "E:\\运行目录");
    deliver(&mut app, &ctx, Completion::Application(None));
    deliver(&mut app, &ctx, Completion::Directory(None));
    assert_eq!(app.model.basic.application, "C:\\Other\\third.exe");
    assert_eq!(app.model.basic.directory, "E:\\运行目录");
}

#[test]
fn dependencies_filter_self_and_duplicates_and_keep_manual_groups_separate() {
    assert!(!dependency_available(
        "RpcSs\r\nDnscache",
        "CurrentService",
        "RPCSS"
    ));
    assert!(!dependency_available(
        "",
        "CurrentService",
        "currentservice"
    ));
    assert!(!dependency_available("服务甲", "other", "服务甲"));
    assert!(dependency_available("RpcSs", "CurrentService", "Dnscache"));
    for (raw, expected) in [
        ("", "Dnscache"),
        ("RpcSs", "RpcSs\r\nDnscache"),
        ("RpcSs\r\n", "RpcSs\r\nDnscache"),
    ] {
        let mut raw = raw.to_owned();
        append_dependency(&mut raw, "Dnscache");
        assert_eq!(raw, expected);
    }
    let ctx = egui::Context::default();
    let mut app = App::new(GuiMode::Install(None), true);
    app.model
        .fields
        .iter_mut()
        .find(|field| field.spec.name == "DependOnGroup")
        .unwrap()
        .raw = "NetworkProvider".into();
    deliver(
        &mut app,
        &ctx,
        Completion::DependencyServices(Ok(vec!["RpcSs".into()])),
    );
    assert_eq!(app.dependency_services.as_deref().unwrap(), ["RpcSs"]);
    assert_eq!(
        app.model
            .fields
            .iter()
            .find(|field| field.spec.name == "DependOnGroup")
            .unwrap()
            .raw,
        "NetworkProvider"
    );
    deliver(
        &mut app,
        &ctx,
        Completion::DependencyServices(Err("denied".into())),
    );
    assert_eq!(app.dependency_error.as_deref(), Some("denied"));
    assert_eq!(app.dependency_services.as_deref().unwrap(), ["RpcSs"]);
    app.dependency_choice = Some("RemovedService".into());
    deliver(
        &mut app,
        &ctx,
        Completion::DependencyServices(Ok(vec!["Dnscache".into()])),
    );
    assert!(app.dependency_choice.is_none());
}

#[test]
fn new_service_prepares_a_blank_form_without_installing_or_stopping_anything() {
    let ctx = egui::Context::default();
    let mut app = App::new(GuiMode::Install(None), true);
    app.model.service = Some("RunningService".into());
    app.model.state = 4;
    app.model.basic.application = "worker.exe".into();
    app.model.password = "test-password".into();
    app.intent(&ctx, Intent::New);
    assert!(matches!(app.pending, Some(Intent::New)));
    assert!(app.busy.is_none());
    assert_eq!(app.model.service.as_deref(), Some("RunningService"));
    app.pending = None;
    app.perform(&ctx, Intent::New);
    assert!(app.busy.is_none());
    assert!(app.model.service.is_none());
    assert!(app.model.basic.application.is_empty());
    assert!(app.model.password.is_empty());
    assert_eq!(app.model.active_page, 1);
    assert!(ctx.memory(|memory| memory.has_focus(Id::unique(("basic", "Name")))));
}

#[test]
fn field_help_remains_available_read_only_and_page_changes_close_it_without_modifying_drafts() {
    let ctx = egui::Context::default();
    style::fonts(&ctx);
    style::configure(&ctx, false);
    let mut app = App::new(GuiMode::Install(None), false);
    app.model.active_page = 7;
    app.model.basic.application = "C:\\Apps\\worker.exe".into();
    let index = app
        .model
        .fields
        .iter()
        .position(|field| field.spec.name == "AppStopMethodSkip")
        .unwrap();
    app.model.fields[index].raw = "3".into();
    let before = app.model.clone();
    hover(&mut app, &ctx, pos2(0.0, 0.0));
    let help = app
        .geometry
        .iter()
        .find(|(name, _)| name == &format!("help-setting-{index}"))
        .unwrap()
        .1;
    assert_eq!(
        hover(&mut app, &ctx, help.center()),
        egui::CursorIcon::PointingHand
    );
    assert!(
        !egui::Popup::is_any_open(&ctx),
        "hover alone must not open help"
    );
    for pressed in [true, false] {
        let mut raw = input(1120.0, 820.0, 1.0);
        raw.events.push(egui::Event::PointerButton {
            pos: help.center(),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        ctx.run_ui(raw, |ui| app.render(ui))
            .drop_without_applying_deltas();
    }
    assert!(egui::Popup::is_any_open(&ctx));
    assert!(app.busy.is_none());
    assert_eq!(app.model.basic.application, before.basic.application);
    assert_eq!(app.model.fields[index].raw, before.fields[index].raw);
    app.select_page(&ctx, 1);
    assert!(!egui::Popup::is_any_open(&ctx));
    assert!(app.busy.is_none());
    assert_eq!(app.model.fields[index].raw, "3");
}

fn hover(app: &mut App, ctx: &egui::Context, point: egui::Pos2) -> egui::CursorIcon {
    let mut cursor = egui::CursorIcon::Default;
    for _ in 0..2 {
        let mut raw = input(1120.0, 820.0, 1.0);
        raw.events.push(egui::Event::PointerMoved(point));
        let output = ctx.run_ui(raw, |ui| app.render(ui));
        cursor = output.platform_output.cursor_icon;
        output.drop_without_applying_deltas();
    }
    cursor
}

#[test]
fn actionable_controls_use_hand_without_overriding_text_resize_or_disabled_controls() {
    let ctx = egui::Context::default();
    style::fonts(&ctx);
    style::configure(&ctx, false);
    let mut app = App::new(GuiMode::Install(None), true);
    app.language = Language::EnUs;
    app.model.service = Some("SelectedService".into());
    app.model.services = vec!["SelectedService".into()];
    hover(&mut app, &ctx, pos2(0.0, 0.0));
    for name in [
        "Refresh",
        "New service",
        "Start",
        "service-filter",
        "service-row",
        "service-action",
        "tab-2",
        "language-0",
        "language-1",
        "traffic-0",
        "traffic-1",
        "traffic-2",
        "theme",
        "github",
        "BrowseDirectory",
    ] {
        let rect = app.geometry.iter().find(|(id, _)| id == name).unwrap().1;
        assert_eq!(
            hover(&mut app, &ctx, rect.center()),
            egui::CursorIcon::PointingHand,
            "{name}"
        );
    }
    let text = app
        .geometry
        .iter()
        .find(|(id, _)| id == "Application")
        .unwrap()
        .1;
    assert_eq!(hover(&mut app, &ctx, text.center()), egui::CursorIcon::Text);
    assert_eq!(
        hover(&mut app, &ctx, pos2(2.0, 400.0)),
        egui::CursorIcon::ResizeHorizontal
    );
    let disabled = app
        .geometry
        .iter()
        .find(|(id, _)| id == "admin-status")
        .unwrap()
        .1;
    assert_eq!(
        hover(&mut app, &ctx, disabled.center()),
        egui::CursorIcon::Default
    );

    app.model.active_page = 5;
    hover(&mut app, &ctx, pos2(0.0, 0.0));
    for setting in ["AppAffinity", "AppPriority"] {
        let index = app
            .model
            .fields
            .iter()
            .position(|field| field.spec.name == setting)
            .unwrap();
        let rect = app
            .geometry
            .iter()
            .find(|(id, _)| id == &format!("field-{index}"))
            .unwrap()
            .1;
        assert_eq!(
            hover(&mut app, &ctx, rect.center()),
            egui::CursorIcon::PointingHand,
            "{setting}"
        );
    }
}
#[test]
fn ime_commit_and_password_undo_respect_current_form_identity() {
    let ctx = egui::Context::default();
    style::fonts(&ctx);
    style::configure(&ctx, false);
    let mut app = App::new(GuiMode::Install(None), true);
    ctx.run_ui(input(1120.0, 820.0, 1.0), |ui| app.render(ui))
        .drop_without_applying_deltas();
    ctx.memory_mut(|memory| memory.request_focus(Id::unique(("basic", "Name"))));
    let mut preedit = input(1120.0, 820.0, 1.0);
    preedit
        .events
        .push(egui::Event::Ime(egui::ImeEvent::Preedit {
            text: "服务测试".into(),
            active_range_chars: None,
        }));
    ctx.run_ui(preedit, |ui| app.render(ui))
        .drop_without_applying_deltas();
    let mut commit = input(1120.0, 820.0, 1.0);
    commit
        .events
        .push(egui::Event::Ime(egui::ImeEvent::Commit("服务测试".into())));
    ctx.run_ui(commit, |ui| app.render(ui))
        .drop_without_applying_deltas();
    assert_eq!(app.model.basic.name, "服务测试");
    app.model.active_page = 3;
    app.model.select_logon_account(model::LogonAccount::Custom);
    app.model
        .fields
        .iter_mut()
        .find(|field| field.spec.name == "ObjectName")
        .unwrap()
        .raw = ".\\test-user".into();
    ctx.run_ui(input(1120.0, 820.0, 1.0), |ui| app.render(ui))
        .drop_without_applying_deltas();
    ctx.memory_mut(|memory| memory.request_focus(Id::unique("account-password")));
    let mut typed = input(1120.0, 820.0, 1.0);
    typed.events.push(egui::Event::Text("test-secret".into()));
    ctx.run_ui(typed, |ui| app.render(ui))
        .drop_without_applying_deltas();
    assert_eq!(app.model.password, "test-secret");
    let mut undo = input(1120.0, 820.0, 1.0);
    undo.events.push(egui::Event::Key {
        key: egui::Key::Z,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        },
    });
    ctx.run_ui(undo.clone(), |ui| app.render(ui))
        .drop_without_applying_deltas();
    assert_eq!(app.model.password, "test-secret");
    app.perform(&ctx, Intent::New);
    app.model.active_page = 3;
    ctx.run_ui(undo, |ui| app.render(ui))
        .drop_without_applying_deltas();
    assert!(app.model.password.is_empty());
}

#[derive(Clone)]
struct CaptureCase {
    language: Language,
    dark: bool,
    scale: f32,
    size: [f32; 2],
    page: usize,
}
impl CaptureCase {
    fn name(&self) -> String {
        format!(
            "egui-{}-{}-{}dpi-{}-{}.bmp",
            if self.language == Language::ZhCn {
                "zh"
            } else {
                "en"
            },
            if self.dark { "dark" } else { "light" },
            (self.scale * 96.0) as u32,
            if self.size[0] < 1000.0 {
                "compact"
            } else {
                "normal"
            },
            match self.page {
                1 => "application",
                3 => "logon",
                4 => "dependencies",
                _ => "settings",
            }
        )
    }
}
struct CaptureApp {
    app: App,
    cases: Vec<CaptureCase>,
    index: usize,
    frame: u8,
    receiver: Option<Receiver<Result<()>>>,
    failure: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    started: std::time::Instant,
}
impl eframe::App for CaptureApp {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if frame
            .winit_window()
            .is_some_and(|window| window.is_visible() == Some(true))
        {
            *self.failure.lock().unwrap() = Some("Capture window became visible".into());
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if self.started.elapsed() > std::time::Duration::from_secs(90) {
            *self.failure.lock().unwrap() = Some("GPU capture timed out".into());
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if let Some(receiver) = &self.receiver
            && let Ok(result) = receiver.try_recv()
        {
            self.receiver = None;
            if let Err(error) = result {
                *self.failure.lock().unwrap() = Some(error.to_string());
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            self.index += 1;
            self.frame = 0;
        }
        if self.index == self.cases.len() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if self.frame < 6 {
            let case = &self.cases[self.index];
            self.app.language = case.language;
            self.app.dark = case.dark;
            self.app.model.active_page = case.page;
            style::configure(ctx, case.dark);
            let native = ctx.input(|input| input.viewport().native_pixels_per_point.unwrap_or(1.0));
            let zoom = case.scale / native;
            ctx.set_zoom_factor(zoom);
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(
                case.size[0],
                case.size[1],
            )));
            if self.frame == 0 {
                self.frame = 1;
            }
        }
        if self.frame < 7 {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
            ctx.request_repaint();
        }
    }
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.app.render(ui);
        if self.frame < 6 {
            self.frame += 1;
            ui.ctx().request_repaint();
        } else if self.frame == 6 {
            let case = self.cases[self.index].clone();
            let (sender, receiver) = mpsc::channel();
            self.receiver = Some(receiver);
            self.frame = 7;
            let ctx = ui.ctx().clone();
            ui.ctx().request_screenshot(move |image| {
                let expected = [
                    (case.size[0] * case.scale) as usize,
                    (case.size[1] * case.scale) as usize,
                ];
                let result = if image.size != expected {
                    Err(anyhow!(
                        "{}: expected {:?}, got {:?}",
                        case.name(),
                        expected,
                        image.size
                    ))
                } else {
                    assert_antialiasing(&image, &case).and_then(|()| {
                        write_bitmap(&std::path::Path::new("target").join(case.name()), &image)
                    })
                };
                let _ = sender.send(result);
                ctx.request_repaint();
            });
        }
    }
}
fn assert_antialiasing(image: &egui::ColorImage, case: &CaptureCase) -> Result<()> {
    let background = style::Palette::new(case.dark).canvas;
    let mut covered = 0;
    for y in (17.0 * case.scale) as usize..(39.0 * case.scale) as usize {
        for x in (21.0 * case.scale) as usize..(43.0 * case.scale) as usize {
            let pixel = image.pixels[y * image.size[0] + x];
            let alpha =
                (pixel.g() as f32 - background.g() as f32) / (120.0 - background.g() as f32);
            if (0.05..0.95).contains(&alpha) {
                let expected = background.r() as f32 + alpha * (237.0 - background.r() as f32);
                if (pixel.r() as f32 - expected).abs() < 6.0 {
                    covered += 1;
                }
            }
        }
    }
    anyhow::ensure!(
        covered > 10,
        "{} has insufficient antialiased traffic-light edge samples ({covered})",
        case.name()
    );
    Ok(())
}
fn write_bitmap(path: &std::path::Path, image: &egui::ColorImage) -> Result<()> {
    use std::io::Write;
    let width = image.size[0] as i32;
    let height = image.size[1] as i32;
    let size = (width * height * 4) as u32;
    let mut file = std::fs::File::create(path)?;
    file.write_all(b"BM")?;
    file.write_all(&(54 + size).to_le_bytes())?;
    file.write_all(&[0; 4])?;
    file.write_all(&54u32.to_le_bytes())?;
    file.write_all(&40u32.to_le_bytes())?;
    file.write_all(&width.to_le_bytes())?;
    file.write_all(&(-height).to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&32u16.to_le_bytes())?;
    file.write_all(&[0; 24])?;
    let mut pixels = Vec::with_capacity(size as usize);
    for color in &image.pixels {
        pixels.extend_from_slice(&[color.b(), color.g(), color.r(), 255]);
    }
    file.write_all(&pixels)?;
    Ok(())
}
#[test]
#[ignore = "Creates an owned hidden DX12 window and captures rendered pixels"]
fn capture_native_gpu_matrix() {
    use winit::platform::windows::EventLoopBuilderExtWindows;
    let mut cases = vec![];
    for language in [Language::ZhCn, Language::EnUs] {
        for dark in [false, true] {
            for scale in [1.0, 1.5, 2.0] {
                for size in [[1120.0, 820.0], [900.0, 520.0]] {
                    cases.push(CaptureCase {
                        language,
                        dark,
                        scale,
                        size,
                        page: if size[0] < 1000.0 { 5 } else { 11 },
                    });
                    if scale == 1.0 {
                        cases.push(CaptureCase {
                            language,
                            dark,
                            scale,
                            size,
                            page: 1,
                        });
                    }
                    if scale == 1.0 || scale == 2.0 {
                        for page in [3, 4] {
                            cases.push(CaptureCase {
                                language,
                                dark,
                                scale,
                                size,
                                page,
                            });
                        }
                    }
                }
            }
        }
    }
    let failure = std::sync::Arc::new(std::sync::Mutex::new(None));
    let shared = failure.clone();
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("XSM GPU capture fixture")
            .with_decorations(false)
            .with_visible(false)
            .with_inner_size([1120.0, 820.0]),
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_any_thread(true);
        })),
        ..Default::default()
    };
    configure_gpu(&mut options);
    eframe::run_native(
        "XSM Capture",
        options,
        Box::new(move |context| {
            platform::configure_native(context, true)?;
            style::fonts(&context.egui_ctx);
            style::configure(&context.egui_ctx, false);
            let mut app = App::new(GuiMode::Install(Some("ExampleWorker".into())), true);
            app.model.basic.application = "C:\\Apps\\Example\\worker.exe".into();
            app.model.basic.directory = "C:\\Apps\\Example".into();
            app.model.basic.parameters = "--config worker.json".into();
            app.model.status = "ExampleWorker · 已停止".into();
            app.model.services = vec!["ExampleWorker".into(), "BackgroundTasks".into()];
            app.dependency_services = Some(vec![
                "BackgroundTasks".into(),
                "Dnscache".into(),
                "RpcSs".into(),
            ]);
            context
                .egui_ctx
                .send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
            context.egui_ctx.request_repaint();
            Ok(Box::new(CaptureApp {
                app,
                cases,
                index: 0,
                frame: 0,
                receiver: None,
                failure: shared,
                started: std::time::Instant::now(),
            }))
        }),
    )
    .unwrap();
    assert!(
        failure.lock().unwrap().is_none(),
        "{:?}",
        failure.lock().unwrap()
    );
}

#[test]
fn dependency_picker_search_and_add_work_through_real_input() {
    fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) {
        let mut raw = input(1120.0, 820.0, 1.0);
        raw.events = events;
        ctx.run_ui(raw, |ui| app.render(ui))
            .drop_without_applying_deltas();
        assert!(app.busy.is_none(), "cached picker must not start a worker");
    }

    fn click(app: &mut App, ctx: &egui::Context, target: &str) {
        let point = app
            .geometry
            .iter()
            .find(|(name, _)| name == target)
            .unwrap_or_else(|| panic!("missing target: {target}"))
            .1
            .center();
        frame(app, ctx, vec![egui::Event::PointerMoved(point)]);
        for pressed in [true, false] {
            frame(
                app,
                ctx,
                vec![egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
    }

    for language in [Language::ZhCn, Language::EnUs] {
        let ctx = egui::Context::default();
        style::fonts(&ctx);
        style::configure(&ctx, false);
        let mut app = App::new(GuiMode::Install(None), true);
        app.language = language;
        app.model.basic.name = "CurrentWorker".into();
        app.model.active_page = 4;
        app.dependency_services = Some(
            ["RpcSs", "Dnscache", "CurrentWorker", "EventLog"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        );
        let services = app
            .model
            .fields
            .iter()
            .position(|field| field.spec.name == "DependOnService")
            .unwrap();
        let groups = app
            .model
            .fields
            .iter()
            .position(|field| field.spec.name == "DependOnGroup")
            .unwrap();
        app.model.fields[services].raw = "EventLog".into();
        app.model.fields[groups].raw = "NetworkProvider".into();

        frame(&mut app, &ctx, vec![]);
        click(&mut app, &ctx, "dependency-services");
        frame(&mut app, &ctx, vec![]);
        assert!(egui::Popup::is_any_open(&ctx));
        for (name, visible) in [
            ("RpcSs", true),
            ("Dnscache", true),
            ("CurrentWorker", false),
            ("EventLog", false),
        ] {
            assert_eq!(
                app.geometry
                    .iter()
                    .any(|(id, _)| id == &format!("dependency-option-{name}")),
                visible,
                "unexpected visibility for {name}"
            );
        }

        click(&mut app, &ctx, "dependency-search");
        assert!(egui::Popup::is_any_open(&ctx));
        frame(&mut app, &ctx, vec![egui::Event::Text("Rpc".into())]);
        assert_eq!(app.dependency_filter, "Rpc");
        assert!(egui::Popup::is_any_open(&ctx));
        let visible: Vec<_> = app
            .geometry
            .iter()
            .filter(|(id, _)| id.starts_with("dependency-option-"))
            .map(|(id, _)| id.as_str())
            .collect();
        assert_eq!(visible, ["dependency-option-RpcSs"]);

        click(&mut app, &ctx, "dependency-option-RpcSs");
        assert!(!egui::Popup::is_any_open(&ctx));
        assert_eq!(app.dependency_choice.as_deref(), Some("RpcSs"));
        assert_eq!(app.model.fields[services].raw, "EventLog");
        click(&mut app, &ctx, "dependency-add");
        assert_eq!(app.model.fields[services].raw, "EventLog\r\nRpcSs");
        assert_eq!(app.model.fields[groups].raw, "NetworkProvider");
        assert!(app.dependency_choice.is_none());
        assert!(app.pending.is_none());
    }
}
