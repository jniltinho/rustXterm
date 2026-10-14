//! Shared widgets (design R7, R9): titled group boxes, icon buttons whose tooltip and accessible
//! name come from one string, and the dialog convention's title row and centered button row.

use egui::{
    Color32, Context, Frame, InnerResponse, Key, Margin, Response, RichText, Sense, Stroke, StrokeKind, TextStyle, Ui,
    WidgetInfo, WidgetType, pos2, vec2,
};
use rxt_core::Metrics;

use crate::icons::Icons;
use crate::theme::STRONG;

const M: Metrics = Metrics::DEFAULT;

/// Tooltip of a control: its name, then the shortcut in parentheses when one is configured.
pub(crate) fn tooltip(label: &str, shortcut: Option<&str>) -> String {
    match shortcut.filter(|s| !s.is_empty()) {
        Some(s) => format!("{label} ({s})"),
        None => label.to_owned(),
    }
}

/// Give a (custom painted) button its accessible name `label` and the matching tooltip.
pub(crate) fn describe(r: Response, label: &str, shortcut: Option<&str>) -> Response {
    let enabled = r.enabled();
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    r.on_hover_text(tooltip(label, shortcut))
}

/// Flat icon-only button (16 px icon, 22×22 px); the label is both the tooltip and the
/// accessible name. Falls back to a small text button when the icon is missing.
pub(crate) fn icon_button(ui: &mut Ui, icons: &Icons, icon: &str, label: &str, shortcut: Option<&str>) -> Response {
    let r = match icons.image(icon, M.icon_small) {
        Some(img) => ui.add(
            egui::Button::image(img).frame_when_inactive(false).min_size(vec2(M.control_height, M.control_height)),
        ),
        None => ui.add(egui::Button::new(label).small()),
    };
    describe(r, label, shortcut)
}

/// Titled group box: a tab-like header (16 px icon and strong title) joined to a 1 px bordered
/// body with 8 px padding that spans the available width.
pub(crate) fn group_box<R>(
    ui: &mut Ui,
    icons: &Icons,
    icon: Option<&str>,
    title: &str,
    add: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    let border = Stroke::new(M.stroke, ui.visuals().widgets.inactive.bg_stroke.color);
    let fill = ui.visuals().panel_fill;
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let header = Frame::new()
            .stroke(border)
            .fill(fill)
            .inner_margin(Margin::symmetric(8, 2))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.set_height(M.control_height - 4.0);
                    ui.spacing_mut().item_spacing.x = M.space[1];
                    if let Some(img) = icon.and_then(|i| icons.image(i, M.icon_small)) {
                        ui.add(img);
                    }
                    ui.label(RichText::new(title).text_style(TextStyle::Name(STRONG.into())));
                });
            })
            .response
            .rect;
        let body = Frame::new().stroke(border).inner_margin(Margin::same(8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = M.space[1];
            add(ui)
        });
        // Join the header to the body: hide the body's top border under the header.
        let y = body.response.rect.top() + M.stroke / 2.0;
        ui.painter().hline(header.left() + M.stroke..=header.right() - M.stroke, y, Stroke::new(M.stroke, fill));
        body.inner
    })
}

/// Dialog title row height (the shell row is taller).
pub(crate) const TITLE_H: f32 = 24.0;

/// Dialog title row: 24 px on panel with the title in the body style and a bottom border.
pub(crate) fn title_row(ui: &mut Ui, title: &str) {
    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), TITLE_H), Sense::hover());
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, title));
    let v = ui.visuals();
    let p = ui.painter();
    p.rect_filled(rect, 0.0, v.panel_fill);
    p.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(M.stroke, v.widgets.noninteractive.bg_stroke.color));
    let font = TextStyle::Body.resolve(ui.style());
    p.text(pos2(rect.left() + M.space[3], rect.center().y), egui::Align2::LEFT_CENTER, title, font, v.text_color());
}

/// Window following the dialog convention instead of egui's title bar (centered heading on a
/// tinted band): no title bar and no inner margin; the content starts with [`window_title`].
pub(crate) fn plain_window(ctx: &Context, title: &str) -> egui::Window<'static> {
    let frame = Frame::window(&ctx.global_style()).inner_margin(0);
    egui::Window::new(title.to_owned()).title_bar(false).collapsible(false).frame(frame)
}

/// [`title_row`] of a [`plain_window`] with a close button at its right end; true when clicked.
pub(crate) fn window_title(ui: &mut Ui, title: &str) -> bool {
    let top = ui.cursor().min.y;
    title_row(ui, title);
    let size = vec2(TITLE_H, TITLE_H);
    let rect = egui::Rect::from_min_size(pos2(ui.max_rect().right() - TITLE_H, top), size);
    let r = ui.interact(rect, ui.id().with("window-close"), Sense::click());
    if r.hovered() {
        ui.painter().rect_filled(rect.shrink(M.stroke), 0.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    // A thin glyph like a native title bar, not the colored catalog icon.
    let x = egui::Rect::from_center_size(rect.center(), vec2(9.0, 9.0));
    let stroke = Stroke::new(M.stroke, ui.visuals().text_color());
    ui.painter().line_segment([x.left_top(), x.right_bottom()], stroke);
    ui.painter().line_segment([x.right_top(), x.left_bottom()], stroke);
    describe(r, "Close window", None).clicked()
}

/// Content area of a [`plain_window`] below its title row (8 px padding).
pub(crate) fn window_body<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new().inner_margin(Margin::same(M.space[3] as i8)).show(ui, add).inner
}

/// Bottom dialog button, 80×30 px with a 16 px icon and its label.
pub(crate) fn dialog_button(ui: &mut Ui, icons: &Icons, icon: &str, label: &str, enabled: bool) -> Response {
    let size = vec2(M.dialog_button.0, M.dialog_button.1);
    let button = egui::Button::opt_image_and_text(icons.image(icon, M.icon_small), Some(label.into())).min_size(size);
    ui.add_enabled_ui(enabled, |ui| ui.add_sized(size, button)).inner
}

/// Centered row of dialog buttons `(icon, label, enabled)`, 24 px apart; the index clicked.
pub(crate) fn button_row(ui: &mut Ui, icons: &Icons, buttons: &[(&str, &str, bool)]) -> Option<usize> {
    let gap = M.space[6];
    let n = buttons.len() as f32;
    let width = n * M.dialog_button.0 + (n - 1.0).max(0.0) * gap;
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
        for (i, (icon, label, enabled)) in buttons.iter().enumerate() {
            if dialog_button(ui, icons, icon, label, *enabled).clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// Centered OK (`ok` icon) and Cancel (`cancel` icon): `Some(true)` on OK, `Some(false)` on Cancel.
pub(crate) fn ok_cancel(ui: &mut Ui, icons: &Icons, ok_enabled: bool) -> Option<bool> {
    button_row(ui, icons, &[("ok", "OK", ok_enabled), ("cancel", "Cancel", true)]).map(|i| i == 0)
}

/// Dialog keys: `Some(true)` for Enter (default button), `Some(false)` for Escape (Cancel).
/// Call before showing the dialog, so an open combo or menu still owns Escape, and Enter on a
/// focused button stays that button's click; Enter counts when nothing or a text field is focused.
pub(crate) fn dialog_keys(ctx: &Context) -> Option<bool> {
    if egui::Popup::is_any_open(ctx) {
        return None;
    }
    let (enter, escape) = ctx.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
    let enter_ok = ctx.memory(|m| m.focused()).is_none() || ctx.text_edit_focused();
    if escape {
        Some(false)
    } else if enter && enter_ok {
        Some(true)
    } else {
        None
    }
}

/// Paint a 1 px danger border over an input's frame (invalid value).
pub(crate) fn danger_border(ui: &Ui, r: &Response, danger: Color32) {
    ui.painter().rect_stroke(r.rect, 0.0, Stroke::new(M.stroke, danger), StrokeKind::Inside);
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    #[test]
    fn tooltip_names_shortcut() {
        assert_eq!(tooltip("Settings", Some("Ctrl+,")), "Settings (Ctrl+,)");
        assert_eq!(tooltip("Settings", Some("")), "Settings");
        assert_eq!(tooltip("Settings", None), "Settings");
    }

    #[derive(Default)]
    struct State {
        icons: Icons,
        clicked: Option<usize>,
        ready: bool,
    }

    #[test]
    fn icon_button_name_and_centered_button_row() {
        let mut h = Harness::builder().with_size(vec2(400.0, 200.0)).build_ui_state(
            |ui, s: &mut State| {
                if !s.ready {
                    return;
                }
                ui.set_style(crate::theme::style(&rxt_core::Tokens::light()));
                icon_button(ui, &s.icons, "gear", "Settings", Some("Ctrl+,"));
                group_box(ui, &s.icons, Some("ssh"), "Basic SSH settings", |ui| ui.label("inside"));
                if let Some(i) = button_row(ui, &s.icons, &[("ok", "OK", true), ("cancel", "Cancel", true)]) {
                    s.clicked = Some(i);
                }
            },
            State::default(),
        );
        h.ctx.set_fonts(crate::theme::fonts());
        h.state_mut().icons = Icons::load(&h.ctx.clone(), None);
        h.state_mut().ready = true;
        h.run();
        assert!(h.query_by_label("Settings").is_some(), "icon-only button has its name");
        assert!(h.query_by_label("Basic SSH settings").is_some() && h.query_by_label("inside").is_some());
        let ok = h.get_by_label("OK").rect();
        let cancel = h.get_by_label("Cancel").rect();
        assert_eq!((ok.width(), ok.height()), (80.0, 30.0));
        assert!((cancel.left() - ok.right() - 24.0).abs() < 0.5, "24 px apart");
        assert!(((ok.left() + cancel.right()) / 2.0 - 200.0).abs() <= 2.0, "centered");
        h.get_by_label("Cancel").click();
        h.run();
        assert_eq!(h.state().clicked, Some(1));
    }

    #[test]
    fn dialog_keys_enter_and_escape() {
        let mut h = Harness::builder().build_ui_state(
            |ui, s: &mut Option<bool>| {
                if let Some(k) = dialog_keys(ui.ctx()) {
                    *s = Some(k);
                }
            },
            None,
        );
        h.key_press(Key::Enter);
        h.run();
        assert_eq!(*h.state(), Some(true));
        h.key_press(Key::Escape);
        h.run();
        assert_eq!(*h.state(), Some(false));
    }
}
