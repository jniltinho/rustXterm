//! Empty, loading and error presentations shared by every screen (design R9): a centered muted
//! message, optionally with a 32 px icon and one action button.

use egui::{Align, Layout, RichText, Ui, UiBuilder, pos2, vec2};
use rxt_core::Metrics;

use crate::icons::Icons;

const M: Metrics = Metrics::DEFAULT;

/// Waiting shorter than this shows nothing (no flash for fast operations), in seconds.
pub(crate) const LOADING_DELAY: f64 = 0.2;

/// Muted message with an optional 32 px icon and one action button, centered in the available
/// area. True when the action was clicked.
pub(crate) fn empty(ui: &mut Ui, icons: &Icons, icon: Option<&str>, text: &str, action: Option<&str>) -> bool {
    let h = icon_height(icon) + line() + action_height(action);
    centered(ui, h, |ui| {
        if let Some(img) = icon.and_then(|i| icons.image(i, M.icon_tile)) {
            ui.add(img);
            ui.add_space(M.space[2]);
        }
        ui.label(RichText::new(text).color(ui.visuals().weak_text_color()));
        action_button(ui, action)
    })
}

/// Spinner and `text` once `since` (egui input time when the wait began) is 200 ms old; blank
/// before that. Keeps repainting until the label shows.
pub(crate) fn loading(ui: &mut Ui, text: &str, since: f64) {
    let waited = ui.input(|i| i.time) - since;
    if waited < LOADING_DELAY {
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(LOADING_DELAY - waited));
        return;
    }
    centered(ui, M.control_height, |ui| {
        let font = egui::TextStyle::Body.resolve(ui.style());
        let galley = ui.painter().layout_no_wrap(text.to_owned(), font, ui.visuals().weak_text_color());
        let width = M.icon_small + M.space[2] + galley.size().x;
        ui.allocate_ui_with_layout(vec2(width, M.control_height), Layout::left_to_right(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = M.space[2];
            ui.add(egui::Spinner::new().size(M.icon_small));
            ui.label(RichText::new(text).color(ui.visuals().weak_text_color()));
        });
    });
}

/// Failed operation (`op`, one sentence, danger color) and its `cause` (muted), with an
/// optional `error` icon and one action (retry or next step). True when the action was clicked.
pub(crate) fn error(ui: &mut Ui, icons: &Icons, op: &str, cause: &str, action: Option<&str>) -> bool {
    let icon = Some("error").filter(|i| icons.image(i, M.icon_tile).is_some());
    let h = icon_height(icon) + 2.0 * line() + action_height(action);
    centered(ui, h, |ui| {
        if let Some(img) = icon.and_then(|i| icons.image(i, M.icon_tile)) {
            ui.add(img);
            ui.add_space(M.space[2]);
        }
        ui.label(RichText::new(op).color(ui.visuals().error_fg_color));
        ui.label(RichText::new(cause).color(ui.visuals().weak_text_color()));
        action_button(ui, action)
    })
}

fn line() -> f32 {
    M.control_height - M.space[1]
}

fn icon_height(icon: Option<&str>) -> f32 {
    icon.map_or(0.0, |_| M.icon_tile + M.space[2])
}

fn action_height(action: Option<&str>) -> f32 {
    action.map_or(0.0, |_| M.dialog_button.1 + M.space[3])
}

fn action_button(ui: &mut Ui, action: Option<&str>) -> bool {
    let Some(a) = action else { return false };
    ui.add_space(M.space[1]);
    ui.add(egui::Button::new(a).min_size(vec2(M.dialog_button.0, M.dialog_button.1))).clicked()
}

/// Run `add` in a top-down, horizontally centered child whose content of about `height` is
/// vertically centered in the available rect; the whole rect is consumed.
fn centered<R>(ui: &mut Ui, height: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let avail = ui.available_rect_before_wrap();
    let top = avail.top() + ((avail.height() - height) / 2.0).max(0.0);
    let rect = egui::Rect::from_min_max(pos2(avail.left(), top), avail.max);
    let r = ui.scope_builder(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Center)), add).inner;
    ui.advance_cursor_after_rect(avail);
    r
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    #[test]
    fn empty_is_centered_and_action_clicks() {
        let mut h = Harness::builder().with_size(vec2(400.0, 300.0)).build_ui_state(
            |ui, clicked: &mut bool| {
                *clicked |= empty(ui, &Icons::default(), None, "No tunnels yet", Some("New tunnel"));
            },
            false,
        );
        h.run();
        let text = h.get_by_label("No tunnels yet").rect();
        assert!((text.center().x - 200.0).abs() <= 2.0, "horizontally centered");
        assert!(text.top() > 100.0 && text.bottom() < 200.0, "vertically centered: {text:?}");
        h.get_by_label("New tunnel").click();
        h.run();
        assert!(*h.state());
    }

    #[test]
    fn loading_waits_200_ms() {
        let mut h = Harness::builder().with_step_dt(0.05).build_ui(|ui| loading(ui, "Connecting to web1...", 0.0));
        h.step();
        assert!(h.query_by_label("Connecting to web1...").is_none(), "no flash before 200 ms");
        for _ in 0..5 {
            h.step();
        }
        assert!(h.query_by_label("Connecting to web1...").is_some());
    }

    #[test]
    fn error_names_operation_and_cause() {
        let mut h = Harness::builder().build_ui(|ui| {
            error(ui, &Icons::default(), "Could not connect to web1.", "Connection refused.", Some("Reconnect"));
        });
        h.run();
        for l in ["Could not connect to web1.", "Connection refused.", "Reconnect"] {
            assert!(h.query_by_label(l).is_some(), "{l}");
        }
    }
}
