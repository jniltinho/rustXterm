//! Home tab (desktop-shell "Home page"): logo and title, two flat buttons, a gear for Settings,
//! the first-run theme chooser (thumbnails painted from tokens, no image assets) and recents.

use egui::{
    Align2, Color32, Rect, RichText, Sense, Stroke, StrokeKind, TextStyle, Ui, WidgetInfo, WidgetType, pos2, vec2,
};
use rxt_core::{CommandId, Metrics, Tokens};

use crate::app::{Action, App};
use crate::icons::{self, Icons};
use crate::theme::{DISPLAY, color};

/// Home logo (vector, scaled to 64 px).
const LOGO: &[u8] = include_bytes!("../../../assets/icons/rustxterm.svg");
/// Home button size.
const BUTTON: egui::Vec2 = vec2(200.0, 30.0);
/// First-run chooser panel size.
const CHOOSER: egui::Vec2 = vec2(540.0, 176.0);
/// Recent sessions listed at most.
const RECENT_MAX: usize = 8;

impl App {
    pub(crate) fn home(&self, ui: &mut Ui, out: &mut Vec<Action>) {
        let m = Metrics::DEFAULT;
        let area = ui.max_rect();
        let gear = Rect::from_min_size(pos2(area.max.x - 4.0 - 30.0, area.min.y + 4.0), vec2(30.0, 30.0));
        if flat_icon_button(ui, gear, &self.icons, "settings", m.icon_toolbar, "Settings").clicked() {
            out.push(Action::Run(CommandId::SETTINGS));
        }
        ui.add_space((area.height() * 0.22).round());
        ui.vertical_centered(|ui| {
            ui.horizontal(|ui| {
                let title =
                    egui::WidgetText::from(RichText::new("rustXterm").text_style(TextStyle::Name(DISPLAY.into())));
                let galley = title.into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, TextStyle::Body);
                let w = 64.0 + m.space[5] + galley.size().x;
                ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
                ui.add(egui::Image::from_bytes("bytes://rustxterm-logo.svg", LOGO).fit_to_exact_size(vec2(64.0, 64.0)));
                ui.add_space(m.space[5] - ui.spacing().item_spacing.x);
                ui.label(RichText::new("rustXterm").text_style(TextStyle::Name(DISPLAY.into())));
            });
            ui.add_space(m.space[6]);
            ui.horizontal(|ui| {
                let gap = m.space[6];
                ui.spacing_mut().item_spacing.x = gap;
                ui.add_space(((ui.available_width() - 2.0 * BUTTON.x - gap) / 2.0).max(0.0));
                if home_button(ui, &self.icons, "session-new", "New session", true).clicked() {
                    out.push(Action::Run(CommandId::NEW_SESSION));
                }
                let import = self.available(CommandId::IMPORT_SESSIONS);
                if home_button(ui, &self.icons, "open", "Import sessions", import).clicked() {
                    out.push(Action::Run(CommandId::IMPORT_SESSIONS));
                }
            });
            if !self.settings.first_run_theme_done {
                ui.add_space(m.space[4]);
                chooser(ui, out);
            }
            if !self.store.recent.is_empty() {
                ui.add_space(m.space[6]);
                ui.label(RichText::new("Recent sessions").text_style(TextStyle::Name(crate::theme::STRONG.into())));
                for s in self.store.recent.iter().take(RECENT_MAX) {
                    ui.horizontal(|ui| {
                        ui.add_space(((ui.available_width() - BUTTON.x) / 2.0).max(0.0));
                        if let Some(img) = self.icons.image(icons::session_icon(s), m.icon_small) {
                            ui.add(img);
                        }
                        if ui.link(&s.name).clicked() {
                            out.push(Action::Open(Box::new(s.clone())));
                        }
                    });
                }
            }
        });
    }
}

/// Flat rectangular 200×30 button: 16 px icon and label.
fn home_button(ui: &mut Ui, icons: &Icons, icon: &str, label: &str, enabled: bool) -> egui::Response {
    let img = icons.image(icon, Metrics::DEFAULT.icon_small);
    let b = egui::Button::opt_image_and_text(img, Some(label.into())).min_size(BUTTON);
    ui.add_enabled(enabled, b)
}

/// Icon-only button painted flat in `rect`, with tooltip and accessible name.
pub(crate) fn flat_icon_button(
    ui: &mut Ui,
    rect: Rect,
    icons: &Icons,
    icon: &str,
    px: f32,
    label: &str,
) -> egui::Response {
    let r = ui.interact(rect, ui.id().with(("flat-icon", label)), Sense::click());
    if r.hovered() {
        ui.painter().rect_filled(rect, 0.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    if let Some(img) = icons.image(icon, px) {
        img.paint_at(ui, Rect::from_center_size(rect.center(), vec2(px, px)));
    }
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
    r.on_hover_text(label)
}

/// First-run Light/Dark chooser; a pick runs the theme command (which marks the chooser answered).
fn chooser(ui: &mut Ui, out: &mut Vec<Action>) {
    let (rect, _) = ui.allocate_exact_size(CHOOSER, Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 0.0, ui.visuals().panel_fill);
    let text = ui.visuals().text_color();
    p.text(
        pos2(rect.center().x, rect.min.y + 16.0),
        Align2::CENTER_CENTER,
        "Select your favorite theme",
        TextStyle::Heading.resolve(ui.style()),
        text,
    );
    let thumb = vec2(200.0, 110.0);
    for (i, (label, cmd, tokens)) in
        [("Light", CommandId::THEME_LIGHT, Tokens::light()), ("Dark", CommandId::THEME_DARK, Tokens::dark())]
            .into_iter()
            .enumerate()
    {
        let cx = rect.center().x + if i == 0 { -135.0 } else { 135.0 };
        let cell = Rect::from_center_size(pos2(cx, rect.min.y + 34.0 + 68.0), vec2(thumb.x + 8.0, thumb.y + 28.0));
        let r = ui.interact(cell, ui.id().with(("theme-thumb", label)), Sense::click());
        let p = ui.painter();
        // The current theme's cell is marked like a selected tile (pressed fill, accent border).
        let current = ui.visuals().dark_mode == (i == 1);
        if current {
            let v = ui.visuals();
            p.rect(cell, 0.0, v.widgets.open.weak_bg_fill, Stroke::new(1.0, v.selection.bg_fill), StrokeKind::Inside);
        } else if r.hovered() {
            p.rect_filled(cell, 0.0, ui.visuals().widgets.hovered.weak_bg_fill);
        }
        let img = Rect::from_min_size(cell.min + vec2(4.0, 4.0), thumb);
        paint_thumbnail(p, img, &tokens);
        p.text(pos2(cx, img.max.y + 12.0), Align2::CENTER_CENTER, label, TextStyle::Body.resolve(ui.style()), text);
        r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, format!("{label} theme")));
        if r.clicked() {
            out.push(Action::Run(cmd));
        }
    }
}

/// Miniature main window drawn from `t`: menu/toolbar band, sidebar, tab row, content, status bar.
fn paint_thumbnail(p: &egui::Painter, r: Rect, t: &Tokens) {
    let c = |x| color(x);
    p.rect_filled(r, 0.0, c(t.surface));
    let band = |y0: f32, h: f32, fill: Color32| {
        p.rect_filled(Rect::from_min_size(pos2(r.min.x, r.min.y + y0), vec2(r.width(), h)), 0.0, fill);
    };
    band(0.0, 14.0, c(t.toolbar));
    for i in 0..6 {
        let x = r.min.x + 6.0 + i as f32 * 14.0;
        p.rect_filled(Rect::from_min_size(pos2(x, r.min.y + 3.0), vec2(8.0, 8.0)), 0.0, c(t.accent));
    }
    p.hline(r.x_range(), r.min.y + 14.0, Stroke::new(1.0, c(t.border_subtle)));
    let side = Rect::from_min_max(pos2(r.min.x, r.min.y + 15.0), pos2(r.min.x + 50.0, r.max.y - 6.0));
    p.rect_filled(side, 0.0, c(t.surface));
    p.rect_filled(Rect::from_min_size(side.min, vec2(8.0, side.height())), 0.0, c(t.panel));
    p.rect_filled(Rect::from_min_size(side.min + vec2(10.0, 4.0), vec2(36.0, 5.0)), 0.0, c(t.selection));
    for i in 1..5 {
        let y = side.min.y + 4.0 + i as f32 * 8.0;
        p.rect_filled(Rect::from_min_size(pos2(side.min.x + 14.0, y), vec2(28.0, 3.0)), 0.0, c(t.text_muted));
    }
    p.vline(side.max.x, side.y_range(), Stroke::new(1.0, c(t.border)));
    let content = Rect::from_min_max(pos2(side.max.x + 1.0, side.min.y), pos2(r.max.x, side.max.y));
    p.rect_filled(Rect::from_center_size(content.center() - vec2(0.0, 12.0), vec2(50.0, 6.0)), 0.0, c(t.text));
    p.rect_filled(Rect::from_center_size(content.center() + vec2(0.0, 4.0), vec2(40.0, 7.0)), 0.0, c(t.panel));
    band(r.height() - 6.0, 6.0, c(t.panel));
    p.rect_stroke(r, 0.0, Stroke::new(1.0, c(t.border)), StrokeKind::Inside);
}
