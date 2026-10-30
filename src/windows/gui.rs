//! Event-driven editor: rendering only reads form state, never the service backend.
mod locale;
mod model;
mod ops;
mod platform;
mod style;
#[cfg(test)]
mod tests;
use crate::i18n::Language;
use anyhow::{Result, anyhow};
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Id, Layout, Rect, Response, Sense, Stroke, StrokeKind,
    Ui, UiBuilder, pos2, vec2,
};
use model::{Model, category, choices};
use ops::Action;
use std::{
    sync::mpsc::{self, Receiver},
    thread,
};

#[derive(Debug)]
pub enum GuiMode {
    ElevatedInstall,
    Install(Option<String>),
    Edit(String),
    Remove(Option<String>),
}
const BUTTON: f32 = 108.0;
enum Work {
    Backend(Action),
    Browse,
    BrowseDirectory,
    DependencyServices,
    Export,
}
enum Completion {
    Model {
        model: Box<Model>,
        reset_editors: bool,
    },
    Application(Option<String>),
    Directory(Option<String>),
    DependencyServices(std::result::Result<Vec<String>, String>),
}
enum Intent {
    Switch(String),
    Refresh,
    New,
    Remove,
    Close,
}
struct App {
    model: Model,
    language: Language,
    dark: bool,
    all: bool,
    more: usize,
    busy: Option<Receiver<Result<Completion>>>,
    after_refresh: Option<String>,
    remove_after_load: bool,
    pending: Option<Intent>,
    error: Option<String>,
    allow_close: bool,
    dependency_services: Option<Vec<String>>,
    dependency_error: Option<String>,
    dependency_filter: String,
    dependency_choice: Option<String>,
    #[cfg(test)]
    geometry: Vec<(String, Rect)>,
}
impl App {
    fn new(mode: GuiMode, elevated: bool) -> Self {
        let (name, selection, remove) = match mode {
            GuiMode::ElevatedInstall => (None, None, false),
            GuiMode::Install(name) => (name, None, false),
            GuiMode::Edit(name) => (None, Some(name), false),
            GuiMode::Remove(name) => (None, name, true),
        };
        Self {
            model: Model::new(name, elevated),
            language: crate::i18n::language(),
            dark: false,
            all: false,
            more: 0,
            busy: None,
            after_refresh: selection,
            remove_after_load: remove,
            pending: None,
            error: None,
            allow_close: false,
            dependency_services: None,
            dependency_error: None,
            dependency_filter: String::new(),
            dependency_choice: None,
            #[cfg(test)]
            geometry: vec![],
        }
    }
    fn tr(&self, text: &str) -> String {
        locale::translate(text, self.language)
    }
    fn text<'a>(&self, zh: &'a str, en: &'a str) -> &'a str {
        if self.language == Language::ZhCn {
            zh
        } else {
            en
        }
    }
    fn record(&mut self, name: &str, response: &Response) {
        #[cfg(test)]
        self.geometry.push((name.into(), response.rect));
        #[cfg(not(test))]
        let _ = (name, response);
    }
    fn submit(&mut self, ctx: &egui::Context, work: Work) {
        if self.busy.is_some() {
            return;
        }
        if let Work::Backend(action) = &work
            && let Err(error) = self.model.prepare(action)
        {
            self.error = Some(format!("{error:#}"));
            return;
        }
        let snapshot = self.model.clone();
        let language = self.language;
        let repaint = ctx.clone();
        let (sender, receiver) = mpsc::channel();
        let started = thread::Builder::new()
            .name("xsm-gui-operation".into())
            .spawn(move || {
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match work {
                        Work::Backend(action) => {
                            let reset_editors = matches!(
                                action,
                                Action::Refresh {
                                    clear_selection: true,
                                    ..
                                }
                            );
                            ops::run(snapshot, action).map(|model| Completion::Model {
                                model: Box::new(model),
                                reset_editors,
                            })
                        }
                        Work::Browse => {
                            platform::file_dialog(false, language).map(Completion::Application)
                        }
                        Work::BrowseDirectory => {
                            platform::directory_dialog(&snapshot.basic.directory, language)
                                .map(Completion::Directory)
                        }
                        Work::DependencyServices => Ok(Completion::DependencyServices(
                            super::scm::list(true)
                                .map(|mut names| {
                                    names.sort_unstable_by_key(|name| name.to_lowercase());
                                    names
                                })
                                .map_err(|error| format!("{error:#}")),
                        )),
                        Work::Export => match platform::file_dialog(true, language)? {
                            Some(path) => ops::run(snapshot, Action::Export { path: path.into() })
                                .map(|model| Completion::Model {
                                    model: Box::new(model),
                                    reset_editors: false,
                                }),
                            None => Ok(Completion::Model {
                                model: Box::new(snapshot),
                                reset_editors: false,
                            }),
                        },
                    }))
                    .unwrap_or_else(|_| Err(anyhow!("操作意外中止，请检查当前服务状态")));
                let _ = sender.send(result);
                repaint.request_repaint();
            });
        match started {
            Ok(_) => self.busy = Some(receiver),
            Err(error) => self.error = Some(error.to_string()),
        }
    }
    fn poll(&mut self, ctx: &egui::Context) {
        let result = match self.busy.as_ref().map(|receiver| receiver.try_recv()) {
            Some(Ok(result)) => result,
            Some(Err(mpsc::TryRecvError::Disconnected)) => Err(anyhow!("后台操作连接已关闭")),
            _ => return,
        };
        self.busy = None;
        match result {
            Ok(Completion::Model {
                model,
                reset_editors,
            }) => {
                let clear = reset_editors
                    || self.model.service != model.service
                    || (!self.model.password.is_empty() && model.password.is_empty());
                self.model = *model;
                if clear {
                    self.clear_editor_state(ctx);
                    self.dependency_choice = None;
                }
                if reset_editors {
                    self.dependency_services = None;
                }
                if let Some(name) = self.after_refresh.take() {
                    self.submit(ctx, Work::Backend(Action::Load { name, page: 1 }));
                } else if self.remove_after_load && self.model.service.is_some() {
                    self.remove_after_load = false;
                    self.pending = Some(Intent::Remove);
                } else if self.model.active_page == 4
                    && self.dependency_services.is_none()
                    && self.dependency_error.is_none()
                {
                    self.submit(ctx, Work::DependencyServices);
                }
            }
            Ok(Completion::Application(Some(path))) => {
                let previous = self.model.basic.application.clone();
                self.model.basic.application = path;
                sync_working_directory(&mut self.model.basic, &previous);
            }
            Ok(Completion::Directory(Some(path))) => self.model.basic.directory = path,
            Ok(Completion::Application(None) | Completion::Directory(None)) => {}
            Ok(Completion::DependencyServices(result)) => match result {
                Ok(services) => {
                    if self
                        .dependency_choice
                        .as_ref()
                        .is_some_and(|name| !services.contains(name))
                    {
                        self.dependency_choice = None;
                    }
                    self.dependency_services = Some(services);
                    self.dependency_error = None;
                }
                Err(error) => self.dependency_error = Some(error),
            },
            Err(error) => {
                self.after_refresh = None;
                self.error = Some(format!("{error:#}"));
            }
        }
    }
    fn intent(&mut self, ctx: &egui::Context, intent: Intent) {
        if self.busy.is_some() {
            return;
        }
        if self.model.dirty() || matches!(intent, Intent::Remove) {
            self.pending = Some(intent);
        } else {
            self.perform(ctx, intent);
        }
    }
    fn perform(&mut self, ctx: &egui::Context, intent: Intent) {
        match intent {
            Intent::Refresh => self.submit(
                ctx,
                Work::Backend(Action::Refresh {
                    all: self.all,
                    clear_selection: true,
                }),
            ),
            Intent::Switch(name) => self.submit(
                ctx,
                Work::Backend(Action::Load {
                    name,
                    page: self.model.active_page,
                }),
            ),
            Intent::New => {
                self.clear_editor_state(ctx);
                self.model.new_service(None);
                self.dependency_choice = None;
                ctx.memory_mut(|memory| memory.request_focus(Id::unique(("basic", "Name"))));
            }
            Intent::Remove => self.submit(ctx, Work::Backend(Action::Remove)),
            Intent::Close => {
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
    fn clear_editor_state(&self, ctx: &egui::Context) {
        egui::Popup::close_all(ctx);
        let mut ids = vec![Id::unique("account-password")];
        ids.extend(
            ["Name", "Application", "AppDirectory", "AppParameters"]
                .iter()
                .map(|name| Id::unique(("basic", name))),
        );
        ids.extend(
            self.model
                .fields
                .iter()
                .map(|field| Id::unique(("setting", field.spec.name, &field.key))),
        );
        ctx.data_mut(|data| {
            for id in ids {
                data.remove::<egui::text_edit::TextEditState>(id);
            }
        });
    }
    fn select_page(&mut self, ctx: &egui::Context, page: usize) {
        if self.busy.is_some() {
            return;
        }
        egui::Popup::close_all(ctx);
        if self.model.loaded.contains(&page) {
            self.model.active_page = page;
            if page == 4 && self.dependency_services.is_none() && self.dependency_error.is_none() {
                self.submit(ctx, Work::DependencyServices);
            }
        } else {
            self.submit(ctx, Work::Backend(Action::LoadPage { page }));
        }
    }
    fn button(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        label: &str,
        enabled: bool,
        accent: bool,
    ) -> Response {
        let mut child = child(ui, rect, label);
        if !enabled {
            child.disable();
        }
        let caption = egui::RichText::new(label).color(if accent {
            if self.dark {
                Color32::from_rgb(17, 46, 34)
            } else {
                Color32::WHITE
            }
        } else {
            style::Palette::new(self.dark).text
        });
        let mut button = egui::Button::new(caption).min_size(rect.size());
        if accent {
            button = button
                .fill(style::Palette::new(self.dark).accent)
                .stroke(Stroke::NONE);
        }
        let response = child.add_sized(rect.size(), button);
        self.record(label, &response);
        response
    }
    fn render(&mut self, ui: &mut Ui) {
        #[cfg(test)]
        self.geometry.clear();
        let rect = ui.max_rect();
        let colors = style::Palette::new(self.dark);
        ui.painter().rect_filled(rect, 0, colors.canvas);
        self.header(ui, rect);
        self.toolbar(ui, rect);
        let sidebar = Rect::from_min_max(
            rect.min + vec2(16.0, 112.0),
            pos2(rect.left() + 244.0, rect.bottom() - 48.0),
        );
        let content = Rect::from_min_max(
            pos2(sidebar.right() + 16.0, sidebar.top()),
            rect.max - vec2(16.0, 48.0),
        );
        self.sidebar(ui, sidebar);
        self.content(ui, content);
        self.footer(
            ui,
            Rect::from_min_max(pos2(rect.left(), rect.bottom() - 40.0), rect.max),
        );
        self.dialogs(ui.ctx());
        style::resize_edges(ui, rect);
        style::finish_interaction_cursor(ui.ctx());
    }
    fn header(&mut self, ui: &mut Ui, rect: Rect) {
        let colors = style::Palette::new(self.dark);
        for (index, zh, en, color) in [
            (0, "关闭", "Close", Color32::from_rgb(237, 120, 109)),
            (1, "最小化", "Minimize", Color32::from_rgb(229, 190, 100)),
            (
                2,
                "最大化或还原",
                "Maximize or restore",
                Color32::from_rgb(130, 178, 139),
            ),
        ] {
            let area = Rect::from_min_size(
                rect.min + vec2(16.0 + index as f32 * 34.0, 12.0),
                vec2(32.0, 32.0),
            );
            let response = style::icon_button(
                ui,
                area,
                self.text(zh, en),
                style::Icon::Traffic(index),
                color,
            );
            self.record(&format!("traffic-{index}"), &response);
            if response.clicked() {
                match index {
                    0 => self.intent(ui.ctx(), Intent::Close),
                    1 => ui
                        .ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                    _ => style::maximize(ui.ctx()),
                }
            }
        }
        ui.painter().text(
            rect.min + vec2(132.0, 28.0),
            Align2::LEFT_CENTER,
            "XSM",
            FontId::proportional(24.0),
            colors.text,
        );
        if rect.width() > 1000.0 {
            ui.painter().text(
                rect.min + vec2(198.0, 28.0),
                Align2::LEFT_CENTER,
                self.text("Windows 服务管理", "Windows service manager"),
                FontId::proportional(14.0),
                colors.muted,
            );
        }
        let area = Rect::from_min_max(
            rect.min + vec2(126.0, 5.0),
            pos2(rect.right() - 332.0, rect.top() + 52.0),
        );
        let drag = ui.interact(area, Id::unique("title-drag"), Sense::click_and_drag());
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            style::maximize(ui.ctx());
        }
        let switch = Rect::from_min_size(
            pos2(rect.right() - 318.0, rect.top() + 12.0),
            vec2(106.0, 32.0),
        );
        let language_switch = style::language_switch(ui, switch, self.language, self.dark);
        for (index, response) in language_switch.segments.iter().enumerate() {
            self.record(&format!("language-{index}"), response);
        }
        if let Some(language) = language_switch.changed {
            match crate::i18n::set_language(language) {
                Ok(()) => {
                    self.language = language;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Title(
                        self.tr("XSM — Windows 服务管理器"),
                    ));
                }
                Err(error) => self.error = Some(error.to_string()),
            }
        }
        let theme = Rect::from_min_size(
            pos2(rect.right() - 200.0, rect.top() + 12.0),
            vec2(32.0, 32.0),
        );
        let theme_response = style::icon_button(
            ui,
            theme,
            self.text("切换主题", "Switch theme"),
            if self.dark {
                style::Icon::Sun
            } else {
                style::Icon::Moon
            },
            colors.text,
        );
        self.record("theme", &theme_response);
        if theme_response.clicked() {
            self.dark = !self.dark;
            style::configure(ui.ctx(), self.dark);
        }
        let admin = Rect::from_min_size(
            pos2(rect.right() - 152.0, rect.top() + 12.0),
            vec2(136.0, 32.0),
        );
        if self.model.elevated {
            ui.painter().circle_filled(
                pos2(admin.left() + 9.0, admin.center().y),
                3.0,
                colors.accent,
            );
            let response = ui.put(admin.translate(vec2(8.0, 0.0)).shrink2(vec2(8.0, 0.0)),
                egui::Label::new(egui::RichText::new(self.text("管理员权限", "Administrator"))
                    .color(colors.muted).size(13.0)).selectable(false))
                .on_hover_text(self.text("当前窗口已具有管理员权限，可安装和修改服务。",
                    "This window already has administrator privileges to install and modify services."));
            self.record("admin-status", &response);
        } else if self
            .button(
                ui,
                admin,
                self.text("以管理员打开", "Run as admin"),
                self.busy.is_none(),
                false,
            )
            .on_hover_text(self.text(
                "当前为只读模式；通过 UAC 打开新的管理员窗口。",
                "Read-only mode. Open a new administrator window through UAC.",
            ))
            .clicked()
        {
            self.submit(ui.ctx(), Work::Backend(Action::Elevate));
        }
    }
    fn toolbar(&mut self, ui: &mut Ui, rect: Rect) {
        for (index, (zh, en)) in [
            ("刷新", "Refresh"),
            ("新建服务", "New service"),
            ("启动", "Start"),
            ("停止", "Stop"),
            ("重启", "Restart"),
            ("移除", "Remove"),
        ]
        .iter()
        .enumerate()
        {
            let area = Rect::from_min_size(
                rect.min + vec2(16.0 + index as f32 * (BUTTON + 8.0), 64.0),
                vec2(BUTTON, 32.0),
            );
            let enabled = self.busy.is_none()
                && (index < 2 || self.model.service.is_some() && self.model.elevated);
            let label = self.text(zh, en).to_owned();
            let response = self.button(ui, area, &label, enabled, false);
            let response = if index == 1 {
                response.on_hover_text(self.text(
                "清空表单并准备新服务；点击下方“安装服务”后才会创建服务。",
                "Clear the form for a new service. Windows creates it only when you select Install service."))
            } else {
                response
            };
            if response.clicked() {
                match index {
                    0 => self.intent(ui.ctx(), Intent::Refresh),
                    1 => self.intent(ui.ctx(), Intent::New),
                    2..=4 => self.submit(
                        ui.ctx(),
                        Work::Backend(Action::Control {
                            verb: ["start", "stop", "restart"][index - 2].into(),
                        }),
                    ),
                    _ => self.intent(ui.ctx(), Intent::Remove),
                }
            }
        }
    }
    fn sidebar(&mut self, ui: &mut Ui, rect: Rect) {
        let colors = style::Palette::new(self.dark);
        ui.painter().rect_filled(rect, 12, colors.sidebar);
        let mut inner = child(ui, rect.shrink(12.0), "services");
        if self.busy.is_some() {
            inner.disable();
        }
        inner.label(egui::RichText::new(self.text("服务", "Services")).strong());
        inner.add_space(6.0);
        let label = self.text("包括系统服务", "Include system services");
        let filter = inner
            .checkbox(&mut self.all, label)
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        self.record("service-filter", &filter);
        if filter.changed() {
            self.submit(
                inner.ctx(),
                Work::Backend(Action::Refresh {
                    all: self.all,
                    clear_selection: false,
                }),
            );
        }
        inner.add_space(10.0);
        let list_height = (rect.height() - 150.0).max(40.0);
        let mut selected = None;
        egui::ScrollArea::vertical()
            .id_salt("service-list")
            .auto_shrink([false, false])
            .max_height(list_height)
            .show(&mut inner, |ui| {
                ui.set_min_height(list_height);
                ui.set_min_width(ui.available_width());
                for name in &self.model.services {
                    let response = ui
                        .add_sized(
                            [ui.available_width(), 32.0],
                            egui::Button::new(name)
                                .truncate()
                                .selected(self.model.service.as_ref() == Some(name))
                                .frame(self.model.service.as_ref() == Some(name)),
                        )
                        .on_hover_text(name);
                    #[cfg(test)]
                    self.geometry.push(("service-row".into(), response.rect));
                    if response.clicked() {
                        selected = Some(name.clone());
                    }
                }
                if self.model.services.is_empty() {
                    ui.label(
                        egui::RichText::new(self.text("暂无服务", "No services"))
                            .color(colors.muted),
                    );
                }
            });
        if let Some(name) = selected {
            self.intent(ui.ctx(), Intent::Switch(name));
        }
        let area = Rect::from_min_size(
            pos2(rect.left() + 12.0, rect.bottom() - 84.0),
            vec2(rect.width() - 24.0, 32.0),
        );
        let labels = [
            self.text("暂停服务", "Pause service"),
            self.text("继续服务", "Continue service"),
            self.text("轮转日志", "Rotate logs"),
            self.text("查看进程", "View processes"),
            self.text("导出配置", "Export configuration"),
        ];
        let mut combo = child(ui, area, "action");
        if self.busy.is_some() || self.model.service.is_none() {
            combo.disable();
        }
        let response = egui::ComboBox::from_id_salt("service-action")
            .width(area.width())
            .selected_text(labels[self.more])
            .show_ui(&mut combo, |ui| {
                for (index, label) in labels.iter().enumerate() {
                    ui.selectable_value(&mut self.more, index, *label);
                }
            })
            .response
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        self.record("service-action", &response);
        let run = area.translate(vec2(0.0, 40.0));
        let label = self.text("执行", "Run").to_owned();
        let enabled = self.busy.is_none()
            && self.model.service.is_some()
            && (self.model.elevated || self.more >= 3);
        if self.button(ui, run, &label, enabled, false).clicked() {
            match self.more {
                0..=2 => self.submit(
                    ui.ctx(),
                    Work::Backend(Action::Control {
                        verb: ["pause", "continue", "rotate"][self.more].into(),
                    }),
                ),
                3 => self.submit(ui.ctx(), Work::Backend(Action::Processes)),
                _ => self.submit(ui.ctx(), Work::Export),
            }
        }
    }
    fn content(&mut self, ui: &mut Ui, rect: Rect) {
        let colors = style::Palette::new(self.dark);
        let status = Rect::from_min_size(rect.min, vec2(rect.width(), 32.0));
        ui.painter().rect(
            status,
            8,
            colors.raised,
            Stroke::new(1.0, colors.line),
            StrokeKind::Inside,
        );
        let text = if self.busy.is_some() {
            self.text("正在处理…", "Working…").into()
        } else {
            self.tr(&self.model.status)
        };
        child(ui, status.shrink2(vec2(12.0, 6.0)), "status")
            .add(egui::Label::new(egui::RichText::new(text).color(colors.muted)).truncate());
        let panel = Rect::from_min_max(rect.min + vec2(0.0, 44.0), rect.max - vec2(0.0, 48.0));
        ui.painter().rect(
            panel,
            12,
            colors.surface,
            Stroke::new(1.0, colors.line),
            StrokeKind::Inside,
        );
        let tabs =
            Rect::from_min_size(panel.min + vec2(8.0, 8.0), vec2(panel.width() - 16.0, 72.0));
        ui.painter().rect_filled(tabs, 8, colors.sidebar);
        let width = (tabs.width() - 20.0) / 6.0;
        for (index, label) in model::TAB_LABELS.iter().enumerate() {
            let page = index + 1;
            let area = Rect::from_min_size(
                tabs.min
                    + vec2(
                        (index % 6) as f32 * (width + 4.0),
                        (index / 6) as f32 * 36.0,
                    ),
                vec2(width, 32.0),
            );
            let selected = self.model.active_page == page;
            let caption = self.tr(label);
            let mut tab = child(ui, area, format!("tab-{page}"));
            tab.spacing_mut().button_padding.x = 6.0;
            if self.busy.is_some() {
                tab.disable();
            }
            let response = tab.add_sized(
                area.size(),
                egui::Button::new(egui::RichText::new(&caption).size(13.0))
                    .wrap_mode(egui::TextWrapMode::Extend)
                    .selected(selected)
                    .frame(selected)
                    .fill(if selected {
                        colors.surface
                    } else {
                        colors.sidebar
                    })
                    .stroke(if selected {
                        Stroke::new(1.0, colors.line)
                    } else {
                        Stroke::NONE
                    })
                    .min_size(area.size()),
            );
            if selected {
                ui.painter().hline(
                    area.left() + 12.0..=area.right() - 12.0,
                    area.bottom() - 2.0,
                    Stroke::new(2.0, colors.accent),
                );
            }
            self.record(&format!("tab-{page}"), &response);
            if response.clicked() {
                self.select_page(ui.ctx(), page);
            }
        }
        ui.painter().hline(
            panel.left() + 1.0..=panel.right() - 1.0,
            panel.top() + 88.0,
            Stroke::new(1.0, colors.line),
        );
        let body = Rect::from_min_max(panel.min + vec2(16.0, 102.0), panel.max - vec2(16.0, 12.0));
        let mut form = child(ui, body, "form");
        if self.busy.is_some() {
            form.disable();
        }
        let page = self.model.active_page;
        egui::ScrollArea::vertical()
            .id_salt(("page", page))
            .auto_shrink([false, false])
            .max_height(body.height())
            .show(&mut form, |ui| {
                if page == 1 {
                    self.basic_form(ui);
                } else {
                    self.settings_form(ui);
                }
            });
        let y = rect.bottom() - 32.0;
        if page == 1 {
            let label = if self.model.service.is_some() {
                self.text("保存应用配置", "Save application")
            } else {
                self.text("安装服务", "Install service")
            }
            .to_owned();
            if self
                .button(
                    ui,
                    Rect::from_min_size(pos2(rect.left(), y), vec2(144.0, 32.0)),
                    &label,
                    self.busy.is_none()
                        && self.model.elevated
                        && self.model.managed
                        && self.model.editable
                        && self.model.loaded.contains(&1),
                    true,
                )
                .clicked()
            {
                self.submit(ui.ctx(), Work::Backend(Action::SaveBasics));
            }
        } else {
            for (index, (zh, en)) in [
                ("重新读取", "Reload"),
                ("应用本页", "Apply page"),
                ("重置本页", "Reset page"),
            ]
            .iter()
            .enumerate()
            {
                let enabled = self.busy.is_none()
                    && (index == 0
                        || self.model.loaded.contains(&page)
                            && self.model.elevated
                            && self.model.editable);
                let label = self.text(zh, en).to_owned();
                if self
                    .button(
                        ui,
                        Rect::from_min_size(
                            pos2(rect.left() + index as f32 * (BUTTON + 8.0), y),
                            vec2(BUTTON, 32.0),
                        ),
                        &label,
                        enabled,
                        index == 1,
                    )
                    .clicked()
                {
                    self.submit(
                        ui.ctx(),
                        Work::Backend(if index == 0 {
                            Action::LoadPage { page }
                        } else {
                            Action::Apply {
                                page,
                                reset: index == 2,
                            }
                        }),
                    );
                }
            }
        }
    }
    fn basic_form(&mut self, ui: &mut Ui) {
        let language = self.language;
        let enabled = self.model.managed
            && self.model.elevated
            && self.model.editable
            && self.model.loaded.contains(&1);
        for (index, name) in ["Name", "Application", "AppDirectory", "AppParameters"]
            .iter()
            .enumerate()
        {
            let previous_application = self.model.basic.application.clone();
            let help = style::help_label(
                ui,
                ("basic-help", name),
                locale::setting_label(name, language),
                locale::setting_help(name, language),
                self.dark,
            );
            self.record(&format!("help-basic-{name}"), &help);
            ui.add_space(3.0);
            let width = ui.available_width();
            let editable = enabled && (index != 0 || self.model.service.is_none());
            let text = match index {
                0 => &mut self.model.basic.name,
                1 => &mut self.model.basic.application,
                2 => &mut self.model.basic.directory,
                _ => &mut self.model.basic.parameters,
            };
            let (response, browse) = ui
                .add_enabled_ui(editable, |ui| {
                    ui.horizontal(|ui| {
                        let response = field_edit(
                            ui,
                            Id::unique(("basic", name)),
                            text,
                            if index == 1 || index == 2 {
                                width - BUTTON - 8.0
                            } else {
                                width
                            },
                            false,
                            false,
                        );
                        let browse = if index == 1 || index == 2 {
                            Some(ui.add_sized(
                                [BUTTON, 32.0],
                                egui::Button::new(if language == Language::ZhCn {
                                    "浏览…"
                                } else {
                                    "Browse…"
                                }),
                            ))
                        } else {
                            None
                        };
                        (response, browse)
                    })
                    .inner
                })
                .inner;
            self.record(name, &response);
            if index == 1 && response.changed() {
                sync_working_directory(&mut self.model.basic, &previous_application);
            }
            if let Some(browse) = browse {
                self.record(
                    if index == 1 {
                        "Browse"
                    } else {
                        "BrowseDirectory"
                    },
                    &browse,
                );
                if browse.clicked() {
                    self.submit(
                        ui.ctx(),
                        if index == 1 {
                            Work::Browse
                        } else {
                            Work::BrowseDirectory
                        },
                    );
                }
            }
            ui.add_space(14.0);
        }
    }
    fn settings_form(&mut self, ui: &mut Ui) {
        let page = self.model.active_page;
        let language = self.language;
        if page == 3 {
            self.logon_form(ui);
            return;
        }
        // Service dependencies are the common case; put their picker above
        // service groups without allocating a reordered catalogue each frame.
        let group = self
            .model
            .fields
            .iter()
            .position(|field| page == 4 && field.spec.name == "DependOnGroup");
        for index in (0..self.model.fields.len())
            .filter(|index| Some(*index) != group)
            .chain(group)
        {
            let field = &self.model.fields[index];
            if category(field.spec.name) != page {
                continue;
            }
            let label = locale::field_label(
                field.spec.name,
                field.key.first().map(String::as_str),
                language,
            );
            let enabled = self.model.can_edit(field);
            let multiline = field.multiline();
            let name = field.spec.name;
            let key = field.key.clone();
            let exits = field.exits;
            let help = style::help_label(
                ui,
                ("setting-help", name, &key),
                &label,
                locale::setting_help(name, language),
                self.dark,
            );
            self.record(&format!("help-setting-{index}"), &help);
            ui.add_space(3.0);
            if name == "DependOnService" {
                self.dependency_picker(ui, index, enabled);
                ui.add_space(6.0);
            }
            let width = ui.available_width();
            let options = if exits { &[][..] } else { choices(name) };
            let field = &mut self.model.fields[index];
            let response = ui
                .add_enabled_ui(enabled, |ui| {
                    if name == "AppAffinity" {
                        let mut all = field.raw.eq_ignore_ascii_case("All");
                        let previous = all;
                        let all_label = locale::option_label(name, "All", language);
                        let custom_label = if language == Language::ZhCn {
                            "指定处理器"
                        } else {
                            "Custom processors"
                        };
                        let response = egui::ComboBox::from_id_salt(("affinity", &key))
                            .width(width)
                            .selected_text(if all { all_label } else { custom_label })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut all, true, all_label);
                                ui.selectable_value(&mut all, false, custom_label);
                            })
                            .response
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if all != previous {
                            field.raw = if all { "All" } else { "0" }.into();
                        }
                        if !all {
                            ui.add_space(6.0);
                            field_edit(
                                ui,
                                Id::unique(("setting", name, &key)),
                                &mut field.raw,
                                width,
                                false,
                                false,
                            );
                        }
                        response
                    } else if options.is_empty() {
                        field_edit(
                            ui,
                            Id::unique(("setting", name, &key)),
                            &mut field.raw,
                            width,
                            multiline,
                            false,
                        )
                    } else {
                        egui::ComboBox::from_id_salt(("setting", name, &key))
                            .width(width)
                            .selected_text(locale::option_label(name, &field.raw, language))
                            .show_ui(ui, |ui| {
                                for raw in options {
                                    ui.selectable_value(
                                        &mut field.raw,
                                        (*raw).into(),
                                        locale::option_label(name, raw, language),
                                    );
                                }
                            })
                            .response
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                    }
                })
                .inner;
            self.record(&format!("field-{index}"), &response);
            if response.gained_focus() {
                ui.scroll_to_rect(response.rect, Some(Align::Center));
            }
            ui.add_space(14.0);
        }
    }
    fn logon_form(&mut self, ui: &mut Ui) {
        let Some(index) = self
            .model
            .fields
            .iter()
            .position(|field| field.spec.name == "ObjectName")
        else {
            return;
        };
        let enabled = self.model.can_edit(&self.model.fields[index]);
        let width = ui.available_width();
        let language = self.language;
        let help = style::help_label(
            ui,
            "logon-account-help",
            locale::setting_label("ObjectName", language),
            locale::setting_help("ObjectName", language),
            self.dark,
        );
        self.record("help-logon-account", &help);
        ui.add_space(3.0);
        let previous = self.model.logon_account();
        let mut choice = previous;
        let response = ui
            .add_enabled_ui(enabled, |ui| {
                egui::ComboBox::from_id_salt("logon-account")
                    .width(width)
                    .selected_text(locale::logon_account_label(choice, language))
                    .show_ui(ui, |ui| {
                        for value in model::LogonAccount::ALL {
                            ui.selectable_value(
                                &mut choice,
                                value,
                                locale::logon_account_label(value, language),
                            );
                        }
                    })
                    .response
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
            })
            .inner;
        self.record(&format!("field-{index}"), &response);
        self.record("logon-account", &response);
        if choice != previous {
            self.model.select_logon_account(choice);
            ui.ctx().data_mut(|data| {
                data.remove::<egui::text_edit::TextEditState>(Id::unique("account-password"));
                data.remove::<egui::text_edit::TextEditState>(Id::unique((
                    "setting",
                    "ObjectName",
                    Vec::<String>::new(),
                )));
            });
        }
        if choice == model::LogonAccount::Custom {
            // Keep the editing mode stable while a username temporarily matches
            // a built-in account prefix during typing.
            self.model.select_logon_account(model::LogonAccount::Custom);
            ui.add_space(14.0);
            let help = style::help_label(
                ui,
                "logon-name-help",
                self.text("账户名称", "Account name"),
                locale::account_name_help(language),
                self.dark,
            );
            self.record("help-logon-name", &help);
            ui.add_space(3.0);
            let key = self.model.fields[index].key.clone();
            let response = ui
                .add_enabled_ui(enabled, |ui| {
                    field_edit(
                        ui,
                        Id::unique(("setting", "ObjectName", &key)),
                        &mut self.model.fields[index].raw,
                        width,
                        false,
                        false,
                    )
                })
                .inner;
            self.record("logon-name", &response);
        }
        let password_enabled = self.model.logon_password_enabled();
        if !password_enabled && !self.model.password.is_empty() {
            crate::secret::wipe_string(&mut self.model.password);
            ui.ctx().data_mut(|data| {
                data.remove::<egui::text_edit::TextEditState>(Id::unique("account-password"))
            });
        }
        ui.add_space(14.0);
        let help = style::help_label(
            ui,
            "logon-password-help",
            self.text("密码", "Password"),
            locale::password_help(language),
            self.dark,
        );
        self.record("help-password", &help);
        ui.add_space(3.0);
        let response = ui
            .add_enabled_ui(enabled && password_enabled, |ui| {
                field_edit(
                    ui,
                    Id::unique("account-password"),
                    &mut self.model.password,
                    width,
                    false,
                    true,
                )
            })
            .inner;
        self.record("password", &response);
    }
    fn dependency_picker(&mut self, ui: &mut Ui, index: usize, enabled: bool) {
        let language = self.language;
        let label = |zh, en| if language == Language::ZhCn { zh } else { en };
        let width = (ui.available_width() - 2.0 * (BUTTON + 8.0)).max(120.0);
        let current = self
            .model
            .service
            .as_ref()
            .unwrap_or(&self.model.basic.name)
            .clone();
        let raw = self.model.fields[index].raw.clone();
        let mut add = false;
        let mut reload = false;
        ui.horizontal(|ui| {
            let response = ui
                .allocate_ui_with_layout(vec2(width, 32.0), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(width);
                    ui.add_enabled_ui(enabled && self.dependency_services.is_some(), |ui| {
                        egui::ComboBox::from_id_salt("dependency-services")
                            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                            .width(width)
                            .truncate()
                            .selected_text(
                                self.dependency_choice
                                    .as_deref()
                                    .unwrap_or(label("选择服务…", "Choose service…")),
                            )
                            .show_ui(ui, |ui| {
                                let search = ui.add(
                                    egui::TextEdit::singleline(&mut self.dependency_filter)
                                        .hint_text(label("搜索服务名称", "Search service names"))
                                        .desired_width(width - 20.0),
                                );
                                #[cfg(test)]
                                self.geometry
                                    .push(("dependency-search".into(), search.rect));
                                #[cfg(not(test))]
                                let _ = search;
                                let query = self.dependency_filter.to_lowercase();
                                let mut count = 0;
                                for name in
                                    self.dependency_services.iter().flatten().filter(|name| {
                                        dependency_available(&raw, &current, name)
                                            && name.to_lowercase().contains(&query)
                                    })
                                {
                                    let response = ui.selectable_value(
                                        &mut self.dependency_choice,
                                        Some(name.clone()),
                                        name,
                                    );
                                    #[cfg(test)]
                                    self.geometry
                                        .push((format!("dependency-option-{name}"), response.rect));
                                    if response.clicked() {
                                        ui.close();
                                    }
                                    count += 1;
                                }
                                if count == 0 {
                                    ui.label(label("没有可添加的服务", "No matching services"));
                                }
                            })
                            .response
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                    })
                    .inner
                })
                .inner;
            self.record("dependency-services", &response);
            let response = ui.add_enabled(
                enabled
                    && self
                        .dependency_choice
                        .as_deref()
                        .is_some_and(|name| dependency_available(&raw, &current, name)),
                egui::Button::new(label("添加", "Add")).min_size(vec2(BUTTON, 32.0)),
            );
            add = response.clicked();
            self.record("dependency-add", &response);
            let response = ui.add_sized(
                [BUTTON, 32.0],
                egui::Button::new(label("刷新列表", "Refresh list")),
            );
            reload = response.clicked();
            self.record("dependency-refresh", &response);
        });
        if add && let Some(name) = self.dependency_choice.take() {
            append_dependency(&mut self.model.fields[index].raw, &name);
        }
        if reload {
            self.dependency_error = None;
            self.submit(ui.ctx(), Work::DependencyServices);
        }
        if let Some(error) = &self.dependency_error {
            ui.label(format!(
                "{}: {error}",
                label(
                    "无法读取服务列表，可直接填写或重试",
                    "Could not load services; enter names or retry"
                )
            ));
        }
    }
    fn footer(&mut self, ui: &mut Ui, rect: Rect) {
        let colors = style::Palette::new(self.dark);
        let galley = ui.painter().layout_no_wrap(
            "Copyright © Lucifer. All rights reserved.".into(),
            FontId::proportional(13.0),
            colors.muted,
        );
        let x = rect.center().x - (36.0 + galley.size().x) / 2.0;
        let icon = Rect::from_center_size(pos2(x + 14.0, rect.center().y), vec2(28.0, 28.0));
        let response = style::icon_button(ui, icon, "GitHub", style::Icon::Github, colors.text);
        self.record("github", &response);
        if response.clicked() {
            ui.ctx()
                .open_url(egui::OpenUrl::new_tab("https://github.com/X-Lucifer"));
        }
        ui.painter().galley(
            pos2(x + 36.0, rect.center().y - galley.size().y / 2.0),
            galley,
            colors.muted,
        );
    }
    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.pending.is_some() {
            let remove = matches!(self.pending, Some(Intent::Remove));
            let title = self
                .text(
                    if remove {
                        "移除服务"
                    } else {
                        "未保存的更改"
                    },
                    if remove {
                        "Remove service"
                    } else {
                        "Unsaved changes"
                    },
                )
                .to_owned();
            let message = if remove {
                format!(
                    "{}\n{}",
                    self.model.service.as_deref().unwrap_or_default(),
                    self.text(
                        "确定移除此服务？应用文件和日志会保留。",
                        "Remove this service? Application files and logs will be retained."
                    )
                )
            } else {
                self.text(
                    "继续将丢弃当前尚未保存的更改。",
                    "Continuing will discard the unsaved changes.",
                )
                .into()
            };
            let mut accepted = false;
            let mut cancelled = false;
            egui::Modal::new(Id::unique("confirm")).show(ctx, |ui| {
                ui.heading(title);
                ui.add_space(8.0);
                ui.label(message);
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    accepted = ui
                        .add_sized(
                            [108.0, 32.0],
                            egui::Button::new(self.text("继续", "Continue")),
                        )
                        .clicked();
                    cancelled = ui
                        .add_sized(
                            [108.0, 32.0],
                            egui::Button::new(self.text("取消", "Cancel")),
                        )
                        .clicked();
                });
            });
            if accepted {
                if let Some(intent) = self.pending.take() {
                    self.perform(ctx, intent);
                }
            } else if cancelled {
                self.pending = None;
            }
        }
        if let Some(error) = self.error.clone() {
            let mut close = false;
            egui::Modal::new(Id::unique("error")).show(ctx, |ui| {
                ui.set_max_width(620.0);
                ui.heading(self.text("操作失败", "Operation failed"));
                ui.add_space(8.0);
                ui.label(self.tr(&error));
                ui.add_space(14.0);
                close = ui
                    .add_sized([108.0, 32.0], egui::Button::new(self.text("关闭", "Close")))
                    .clicked();
            });
            if close {
                self.error = None;
            }
        }
        if self.model.details.is_some() {
            let mut open = true;
            let title = self.text("进程信息", "Process details").to_owned();
            egui::Window::new(title)
                .id(Id::unique("details"))
                .open(&mut open)
                .default_size([560.0, 400.0])
                .show(ctx, |ui| {
                    egui::ScrollArea::both().show(ui, |ui| {
                        ui.monospace(self.model.details.as_deref().unwrap_or_default());
                    });
                });
            if !open {
                self.model.details = None;
            }
        }
    }
}
fn sync_working_directory(basic: &mut model::Basic, previous_application: &str) {
    let parent = |path: &str| {
        std::path::Path::new(path)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(|parent| parent.to_string_lossy().into_owned())
    };
    let previous_directory = parent(previous_application);
    if (basic.directory.is_empty() || previous_directory.as_deref() == Some(&basic.directory))
        && let Some(directory) = parent(&basic.application)
    {
        basic.directory = directory;
    }
}
fn dependency_available(raw: &str, current: &str, name: &str) -> bool {
    !super::registry::equal_name(current, name)
        && !raw
            .lines()
            .any(|line| super::registry::equal_name(line.trim(), name))
}
fn append_dependency(raw: &mut String, name: &str) {
    if !raw.is_empty() && !raw.ends_with('\n') {
        raw.push_str("\r\n");
    }
    raw.push_str(name);
}
fn child(ui: &mut Ui, rect: Rect, id: impl std::hash::Hash + std::fmt::Debug) -> Ui {
    ui.new_child(
        UiBuilder::new()
            .id_salt(id)
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
    )
}
fn field_edit(
    ui: &mut Ui,
    id: Id,
    text: &mut String,
    width: f32,
    multiline: bool,
    password: bool,
) -> Response {
    let edit = if multiline {
        egui::TextEdit::multiline(text)
    } else {
        egui::TextEdit::singleline(text)
    };
    let mut output = edit
        .id(id)
        .password(password)
        .margin(vec2(10.0, 7.0))
        .min_size(vec2(width, if multiline { 88.0 } else { 32.0 }))
        .desired_width(width)
        .desired_rows(if multiline { 4 } else { 1 })
        .show(ui);
    if password {
        output.state.clear_undoer();
        output.state.store(ui.ctx(), output.response.id);
    }
    output.response.response
}
impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        if ctx.input(|input| input.viewport().close_requested()) && !self.allow_close {
            if self.busy.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            } else if self.model.dirty() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.pending = Some(Intent::Close);
            }
        }
    }
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.render(ui);
    }
}
pub fn run(mode: GuiMode) -> Result<()> {
    let (size, visible) = platform::startup_window();
    let visible = visible || matches!(mode, GuiMode::ElevatedInstall);
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(locale::translate(
                "XSM — Windows 服务管理器",
                crate::i18n::language(),
            ))
            .with_decorations(false)
            .with_resizable(true)
            .with_inner_size(size)
            .with_min_inner_size([880.0, 480.0])
            .with_visible(visible)
            .with_icon(platform::icon()),
        centered: true,
        persist_window: false,
        ..Default::default()
    };
    configure_gpu(&mut options);
    eframe::run_native(
        "XSM",
        options,
        Box::new(move |context| {
            platform::configure_native(context, !visible)?;
            style::configure(&context.egui_ctx, false);
            style::fonts(&context.egui_ctx);
            let mut app = App::new(mode, super::elevation::is_elevated());
            app.submit(
                &context.egui_ctx,
                Work::Backend(Action::Refresh {
                    all: false,
                    clear_selection: false,
                }),
            );
            Ok(Box::new(app))
        }),
    )
    .map_err(|error| anyhow!("{error}"))
}
fn configure_gpu(options: &mut eframe::NativeOptions) {
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        setup.power_preference = wgpu::PowerPreference::LowPower;
        // A form editor does not need the large host/device allocation blocks
        // intended for throughput-heavy rendering. Preserve eframe's limits.
        let descriptor = setup.device_descriptor.clone();
        setup.device_descriptor = std::sync::Arc::new(move |adapter| {
            let mut device = descriptor(adapter);
            device.memory_hints = wgpu::MemoryHints::MemoryUsage;
            device
        });
        setup.instance_descriptor.backends = wgpu::Backends::DX12;
        setup
            .instance_descriptor
            .backend_options
            .dx12
            .shader_compiler = wgpu::Dx12Compiler::Fxc;
    }
}
