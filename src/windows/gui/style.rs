//! Shared metrics and vector drawing; no operating-system control painting.
#![forbid(unsafe_code)]
use crate::i18n::Language;
use eframe::egui::{self, Color32, Id, Rect, Response, Sense, Stroke, Ui, pos2, vec2};
pub(super) struct Palette {
    pub canvas: Color32,
    pub sidebar: Color32,
    pub surface: Color32,
    pub raised: Color32,
    pub hover: Color32,
    pub line: Color32,
    pub accent: Color32,
    pub text: Color32,
    pub muted: Color32,
}
fn rgb(value: u32) -> Color32 {
    Color32::from_rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}
impl Palette {
    pub fn new(dark: bool) -> Self {
        let (c, s, u, r, h, l, a, t, m) = if dark {
            (
                0x202020, 0x1B1B1B, 0x292929, 0x323232, 0x3C3C3C, 0x3D3D3D, 0x65B18D, 0xF2F3F2,
                0xB7BCB9,
            )
        } else {
            (
                0xF6F7F6, 0xEEF1EF, 0xFFFFFF, 0xF1F4F2, 0xE7EDE9, 0xE0E5E2, 0x367C60, 0x29312C,
                0x637167,
            )
        };
        Self {
            canvas: rgb(c),
            sidebar: rgb(s),
            surface: rgb(u),
            raised: rgb(r),
            hover: rgb(h),
            line: rgb(l),
            accent: rgb(a),
            text: rgb(t),
            muted: rgb(m),
        }
    }
}
pub(super) fn configure(ctx: &egui::Context, dark: bool) {
    let p = Palette::new(dark);
    let mut style = egui::Style::default();
    style.spacing.interact_size = vec2(32.0, 32.0);
    style.spacing.button_padding = vec2(10.0, 6.0);
    style.spacing.item_spacing = vec2(8.0, 4.0);
    style.animation_time = 0.0;
    style.visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    style.visuals.override_text_color = Some(p.text);
    style.visuals.weak_text_color = Some(p.muted);
    style.visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);
    style.visuals.panel_fill = p.canvas;
    style.visuals.window_fill = p.surface;
    style.visuals.extreme_bg_color = p.surface;
    style.visuals.selection.bg_fill = p.hover;
    style.visuals.selection.stroke = Stroke::new(1.0, p.accent);
    for visual in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.noninteractive,
    ] {
        visual.bg_fill = p.raised;
        visual.weak_bg_fill = p.raised;
        visual.bg_stroke = Stroke::new(1.0, p.line);
        visual.fg_stroke = Stroke::new(1.0, p.text);
        visual.corner_radius = 8.into();
        visual.expansion = 0.0;
    }
    for visual in [
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        visual.bg_fill = p.hover;
        visual.weak_bg_fill = p.hover;
        visual.bg_stroke = Stroke::new(1.0, p.accent);
        visual.fg_stroke = Stroke::new(1.0, p.text);
        visual.corner_radius = 8.into();
        visual.expansion = 0.0;
    }
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
    let theme = if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    };
    ctx.set_style_of(theme, style);
    ctx.set_theme(theme);
}
pub(super) struct LanguageSwitch {
    pub changed: Option<Language>,
    pub segments: [Response; 2],
}

/// A compact caption with a keyboard-accessible help button. The caption and
/// explanation are already localized; the stable key is never displayed.
pub(super) fn help_label(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    caption: &str,
    help: &str,
    dark: bool,
) -> Response {
    let colors = Palette::new(dark);
    let id = Id::unique(("field-help", id));
    let response = ui
        .scope(|ui| {
            // This is a label row, not a standard 32-point form control.
            ui.spacing_mut().interact_size.y = 20.0;
            ui.spacing_mut().item_spacing.x = 5.0;
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::Label::new(caption).wrap().selectable(false));
                let (_, rect) = ui.allocate_space(vec2(20.0, 20.0));
                let response = ui
                    .interact(rect, id, Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), caption)
                });
                let color = if response.hovered() || response.has_focus() {
                    colors.accent
                } else {
                    colors.muted
                };
                let painter = ui.painter();
                if response.hovered() {
                    painter.circle_filled(rect.center(), 9.5, colors.hover);
                }
                painter.circle_stroke(rect.center(), 7.5, Stroke::new(1.0, color));
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "?",
                    egui::FontId::proportional(12.0),
                    color,
                );
                if response.has_focus() {
                    painter.circle_stroke(rect.center(), 9.5, Stroke::new(1.0, colors.accent));
                }
                response
            })
            .inner
        })
        .inner;

    let popup_id = egui::Popup::default_response_id(&response);
    if !response.enabled() {
        egui::Popup::close_id(ui.ctx(), popup_id);
        return response;
    }
    let screen = ui.ctx().content_rect();
    let frame = egui::Frame::popup(ui.style())
        .inner_margin(12)
        .fill(colors.surface)
        .stroke(Stroke::new(1.0, colors.line));
    let margin = frame.total_margin().sum();
    let width = (screen.width() - 24.0).clamp(40.0, 420.0);
    let height = (screen.height() - 24.0).clamp(40.0, 300.0);
    egui::Popup::from_toggle_button_response(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .accessible_name(caption)
        .frame(frame)
        .width(width)
        .gap(6.0)
        .show(|ui| {
            ui.set_width((width - margin.x).max(1.0));
            egui::ScrollArea::vertical()
                .id_salt(("help-content", id))
                .auto_shrink([false, true])
                .max_height((height - margin.y).max(1.0))
                .show(ui, |ui| {
                    ui.add(egui::Label::new(help).wrap().selectable(false));
                });
        });
    response
}

pub(super) fn finish_interaction_cursor(ctx: &egui::Context) {
    // Window close/collapse controls use raw interactions instead of Button.
    // Respect text, drag and resize cursors already chosen by their widgets.
    if ctx.output(|output| output.cursor_icon) != egui::CursorIcon::Default {
        return;
    }
    let hovered =
        ctx.interaction_snapshot(|state| state.hovered.iter().copied().collect::<Vec<_>>());
    if hovered.into_iter().any(|id| {
        ctx.read_response(id).is_some_and(|response| {
            response.enabled() && response.hovered() && response.sense.senses_click()
        })
    }) {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

pub(super) fn language_switch(
    ui: &mut Ui,
    rect: Rect,
    current: Language,
    dark: bool,
) -> LanguageSwitch {
    let colors = Palette::new(dark);
    ui.painter().rect(
        rect,
        8,
        colors.raised,
        Stroke::new(1.0, colors.line),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink(2.0);
    let segment_size = vec2(inner.width() / 2.0, inner.height());
    let options = [(Language::ZhCn, "中文"), (Language::EnUs, "EN")];
    let segments = std::array::from_fn(|index| {
        let (language, label) = options[index];
        let area = Rect::from_min_size(
            inner.min + vec2(index as f32 * segment_size.x, 0.0),
            segment_size,
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(("language-switch", index))
                .max_rect(area)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        // The global 32-point control minimum must not grow a 28-point segment
        // outside its padded container. Restrict this override to the segment.
        child.spacing_mut().interact_size = segment_size;
        child.spacing_mut().button_padding = vec2(4.0, 2.0);
        child
            .add_sized(
                segment_size,
                egui::Button::new(label)
                    .selected(language == current)
                    .frame(language == current)
                    .min_size(segment_size),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
    });
    let changed = options
        .iter()
        .zip(&segments)
        .find_map(|((language, _), response)| {
            (response.clicked() && *language != current).then_some(*language)
        });
    LanguageSwitch { changed, segments }
}
pub(super) fn fonts(ctx: &egui::Context) {
    // Install CJK coverage before the first visible frame; system fallback remains
    // available for other scripts and avoids changing geometry as tabs first open.
    let root = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:\\Windows".into());
    if let Ok(data) = std::fs::read(root.join("Fonts\\msyh.ttc")) {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "xsm-cjk".into(),
            std::sync::Arc::new(egui::FontData::from_owned(data)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .push("xsm-cjk".into());
        ctx.set_fonts(fonts);
    }
}
pub(super) enum Icon {
    Traffic(usize),
    Sun,
    Moon,
    Github,
}
pub(super) fn icon_button(
    ui: &mut Ui,
    rect: Rect,
    label: &str,
    icon: Icon,
    color: Color32,
) -> Response {
    let response = ui
        .interact(rect, Id::unique(("icon", label)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
    let p = ui.painter();
    let c = rect.center();
    let stroke = Stroke::new(1.5, color);
    if response.has_focus() {
        p.rect_stroke(
            rect.shrink(1.0),
            6,
            Stroke::new(1.0, color),
            egui::StrokeKind::Inside,
        );
    }
    match icon {
        Icon::Traffic(index) => {
            p.circle_filled(c, 9.0, color);
            let stroke = Stroke::new(
                1.2,
                Color32::from_black_alpha(if response.hovered() { 170 } else { 100 }),
            );
            match index {
                0 => {
                    p.line_segment([c - vec2(3.0, 3.0), c + vec2(3.0, 3.0)], stroke);
                    p.line_segment([c + vec2(-3.0, 3.0), c + vec2(3.0, -3.0)], stroke);
                }
                1 => {
                    p.line_segment([c - vec2(3.5, 0.0), c + vec2(3.5, 0.0)], stroke);
                }
                _ => {
                    p.rect_stroke(
                        Rect::from_center_size(c, vec2(6.0, 6.0)),
                        0,
                        stroke,
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }
        Icon::Sun => {
            p.circle_stroke(c, 4.0, stroke);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                let direction = vec2(a.cos(), a.sin());
                p.line_segment([c + direction * 7.0, c + direction * 10.0], stroke);
            }
        }
        Icon::Moon => {
            p.circle_filled(c, 8.0, color);
            p.circle_filled(c + vec2(3.8, -3.8), 7.2, ui.visuals().panel_fill);
        }
        Icon::Github => {
            p.text(
                c,
                egui::Align2::CENTER_CENTER,
                egui::special_emojis::GITHUB,
                egui::FontId::proportional(20.0),
                color,
            );
        }
    }
    response.on_hover_text(label)
}

pub(super) fn maximize(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(
        !ctx.input(|input| input.viewport().maximized.unwrap_or(false)),
    ));
}
pub(super) fn resize_edges(ui: &mut Ui, rect: Rect) {
    if ui.input(|input| input.viewport().maximized.unwrap_or(false)) {
        return;
    }
    for (index, area, direction, cursor) in [
        (
            0,
            Rect::from_min_max(rect.min, pos2(rect.left() + 5.0, rect.bottom())),
            egui::ResizeDirection::West,
            egui::CursorIcon::ResizeHorizontal,
        ),
        (
            1,
            Rect::from_min_max(pos2(rect.right() - 5.0, rect.top()), rect.max),
            egui::ResizeDirection::East,
            egui::CursorIcon::ResizeHorizontal,
        ),
        (
            2,
            Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + 5.0)),
            egui::ResizeDirection::North,
            egui::CursorIcon::ResizeVertical,
        ),
        (
            3,
            Rect::from_min_max(pos2(rect.left(), rect.bottom() - 5.0), rect.max),
            egui::ResizeDirection::South,
            egui::CursorIcon::ResizeVertical,
        ),
        (
            4,
            Rect::from_min_size(rect.min, vec2(10.0, 10.0)),
            egui::ResizeDirection::NorthWest,
            egui::CursorIcon::ResizeNwSe,
        ),
        (
            5,
            Rect::from_min_size(pos2(rect.right() - 10.0, rect.top()), vec2(10.0, 10.0)),
            egui::ResizeDirection::NorthEast,
            egui::CursorIcon::ResizeNeSw,
        ),
        (
            6,
            Rect::from_min_size(pos2(rect.left(), rect.bottom() - 10.0), vec2(10.0, 10.0)),
            egui::ResizeDirection::SouthWest,
            egui::CursorIcon::ResizeNeSw,
        ),
        (
            7,
            Rect::from_min_size(rect.max - vec2(10.0, 10.0), vec2(10.0, 10.0)),
            egui::ResizeDirection::SouthEast,
            egui::CursorIcon::ResizeNwSe,
        ),
    ] {
        let response = ui
            .interact(area, Id::unique(("resize", index)), Sense::drag())
            .on_hover_cursor(cursor);
        if response.drag_started() {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn help_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        caption: &str,
        help: &str,
        size: egui::Vec2,
        scale: f32,
    ) -> (Response, egui::CursorIcon) {
        let mut raw = input(scale, events);
        raw.screen_rect = Some(Rect::from_min_size(pos2(0.0, 0.0), size));
        let mut response = None;
        let output = ctx.run_ui(raw, |ui| {
            let mut child = ui.new_child(egui::UiBuilder::new().id_salt("help-test").max_rect(
                Rect::from_min_size(pos2(16.0, 16.0), size - vec2(32.0, 32.0)),
            ));
            response = Some(help_label(&mut child, "setting", caption, help, false));
        });
        let cursor = output.platform_output.cursor_icon;
        output.drop_without_applying_deltas();
        (response.unwrap(), cursor)
    }

    fn help_key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn help_click(ctx: &egui::Context, point: egui::Pos2) {
        for pressed in [None, Some(true), Some(false)] {
            let mut events = vec![egui::Event::PointerMoved(point)];
            if let Some(pressed) = pressed {
                events.push(egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            help_frame(
                ctx,
                events,
                "Setting",
                "Explanation",
                vec2(320.0, 240.0),
                1.0,
            );
        }
    }

    #[test]
    fn help_icon_opens_only_on_click_and_closes_on_toggle_outside_or_escape() {
        let ctx = egui::Context::default();
        configure(&ctx, false);
        let (response, _) = help_frame(
            &ctx,
            vec![],
            "Setting",
            "Explanation",
            vec2(320.0, 240.0),
            1.0,
        );
        let point = response.rect.center();
        let (_, cursor) = help_frame(
            &ctx,
            vec![egui::Event::PointerMoved(point)],
            "Setting",
            "Explanation",
            vec2(320.0, 240.0),
            1.0,
        );
        assert_eq!(cursor, egui::CursorIcon::PointingHand);
        assert!(!egui::Popup::is_any_open(&ctx));
        assert_eq!(response.rect.size(), vec2(20.0, 20.0));
        assert!(response.sense.is_focusable());
        help_click(&ctx, point - vec2(24.0, 0.0));
        assert!(!egui::Popup::is_any_open(&ctx), "caption is not clickable");
        help_click(&ctx, point);
        assert!(egui::Popup::is_any_open(&ctx));
        help_click(&ctx, point);
        assert!(!egui::Popup::is_any_open(&ctx));
        help_click(&ctx, point);
        help_click(&ctx, pos2(300.0, 225.0));
        assert!(!egui::Popup::is_any_open(&ctx));
        help_click(&ctx, point);
        help_frame(
            &ctx,
            vec![help_key(egui::Key::Escape)],
            "Setting",
            "Explanation",
            vec2(320.0, 240.0),
            1.0,
        );
        assert!(!egui::Popup::is_any_open(&ctx));
    }

    #[test]
    fn help_icon_supports_tab_enter_and_space_without_popup_focus_transfer() {
        let ctx = egui::Context::default();
        configure(&ctx, false);
        help_frame(
            &ctx,
            vec![],
            "Setting",
            "Explanation",
            vec2(320.0, 240.0),
            1.0,
        );
        help_frame(
            &ctx,
            vec![help_key(egui::Key::Tab)],
            "Setting",
            "Explanation",
            vec2(320.0, 240.0),
            1.0,
        );
        let (response, _) = help_frame(
            &ctx,
            vec![],
            "Setting",
            "Explanation",
            vec2(320.0, 240.0),
            1.0,
        );
        assert!(response.has_focus());
        help_frame(
            &ctx,
            vec![help_key(egui::Key::Enter)],
            "Setting",
            "Explanation",
            vec2(320.0, 240.0),
            1.0,
        );
        assert!(egui::Popup::is_any_open(&ctx));
        assert!(ctx.memory(|memory| memory.has_focus(response.id)));
        help_frame(
            &ctx,
            vec![help_key(egui::Key::Space)],
            "Setting",
            "Explanation",
            vec2(320.0, 240.0),
            1.0,
        );
        assert!(!egui::Popup::is_any_open(&ctx));
    }

    #[test]
    fn wrapped_help_label_and_scrollable_popup_fit_small_and_regular_windows() {
        let caption = "A longer setting caption that must wrap within the available form width";
        let help = "A long explanation with enough content to require scrolling.\n".repeat(80);
        for size in [vec2(320.0, 240.0), vec2(900.0, 600.0)] {
            for scale in [1.0, 2.0] {
                let ctx = egui::Context::default();
                configure(&ctx, false);
                let (response, _) = help_frame(&ctx, vec![], caption, &help, size, scale);
                assert_eq!(response.rect.size(), vec2(20.0, 20.0));
                assert!(
                    Rect::from_min_size(pos2(16.0, 16.0), size - vec2(32.0, 32.0))
                        .contains_rect(response.rect)
                );
                ctx.memory_mut(|memory| memory.request_focus(response.id));
                help_frame(
                    &ctx,
                    vec![help_key(egui::Key::Enter)],
                    caption,
                    &help,
                    size,
                    scale,
                );
                for _ in 0..2 {
                    help_frame(&ctx, vec![], caption, &help, size, scale);
                }
                let popup = ctx
                    .read_response(egui::Popup::default_response_id(&response))
                    .unwrap();
                assert!(
                    Rect::from_min_size(pos2(0.0, 0.0), size)
                        .expand(0.5)
                        .contains_rect(popup.rect)
                );
                assert!(popup.rect.height() <= 300.5);
                assert!(popup.rect.width() <= 420.5);
            }
        }
    }

    #[test]
    fn raw_click_controls_use_hand_and_preserve_explicit_cursors() {
        let ctx = egui::Context::default();
        let point = pos2(40.0, 40.0);
        for enabled in [true, false] {
            for explicit in [
                egui::CursorIcon::Default,
                egui::CursorIcon::Text,
                egui::CursorIcon::ResizeHorizontal,
                egui::CursorIcon::Grabbing,
            ] {
                for frame in 0..2 {
                    let output =
                        ctx.run_ui(input(1.0, vec![egui::Event::PointerMoved(point)]), |ui| {
                            if !enabled {
                                ui.disable();
                            }
                            ui.interact(
                                Rect::from_min_size(pos2(20.0, 20.0), vec2(40.0, 40.0)),
                                egui::Id::unique("raw-click"),
                                egui::Sense::click(),
                            );
                            ctx.set_cursor_icon(explicit);
                            finish_interaction_cursor(&ctx);
                        });
                    let expected = if enabled && explicit == egui::CursorIcon::Default {
                        egui::CursorIcon::PointingHand
                    } else {
                        explicit
                    };
                    // Interaction hit testing takes the preceding frame's geometry.
                    if frame == 1 {
                        assert_eq!(output.platform_output.cursor_icon, expected);
                    }
                    output.drop_without_applying_deltas();
                }
            }
        }
    }

    fn input(scale: f32, events: Vec<egui::Event>) -> egui::RawInput {
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 100.0))),
            events,
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
    fn language_segments_remain_inside_container_at_all_scales() {
        let ctx = egui::Context::default();
        fonts(&ctx);
        let container = Rect::from_min_size(pos2(20.0, 20.0), vec2(106.0, 32.0));
        for current in [Language::ZhCn, Language::EnUs] {
            for dark in [false, true] {
                configure(&ctx, dark);
                for scale in [1.0, 1.5, 2.0] {
                    let output = ctx.run_ui(input(scale, vec![]), |ui| {
                        let switch = language_switch(ui, container, current, dark);
                        for segment in &switch.segments {
                            assert!(container.shrink(2.0).contains_rect(segment.rect));
                            assert!((segment.rect.width() - 51.0).abs() < 0.01);
                            assert!((segment.rect.height() - 28.0).abs() < 0.01);
                        }
                        assert!(switch.segments[0].rect.right() <= switch.segments[1].rect.left());
                        assert_eq!(ui.spacing().interact_size, vec2(32.0, 32.0));
                        assert_eq!(switch.changed, None);
                    });
                    output.drop_without_applying_deltas();
                }
            }
        }
    }

    #[test]
    fn language_switch_only_activates_enabled_segment_and_has_hand_cursor() {
        let ctx = egui::Context::default();
        configure(&ctx, false);
        let container = Rect::from_min_size(pos2(20.0, 20.0), vec2(106.0, 32.0));
        let point = pos2(98.5, 36.0);
        for enabled in [true, false] {
            for pressed in [None, Some(true), Some(false)] {
                let mut changed = None;
                let mut events = vec![egui::Event::PointerMoved(point)];
                if let Some(pressed) = pressed {
                    events.push(egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    });
                }
                let output = ctx.run_ui(input(1.0, events), |ui| {
                    if !enabled {
                        ui.disable();
                    }
                    changed = language_switch(ui, container, Language::ZhCn, false).changed;
                });
                if pressed.is_some() {
                    assert_eq!(
                        output.platform_output.cursor_icon,
                        if enabled {
                            egui::CursorIcon::PointingHand
                        } else {
                            egui::CursorIcon::Default
                        }
                    );
                }
                assert_eq!(
                    changed,
                    (enabled && pressed == Some(false)).then_some(Language::EnUs)
                );
                output.drop_without_applying_deltas();
            }
        }
    }
}
