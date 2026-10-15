//! egui terminal widget rendering an [`rxt_term::Terminal`].
//!
//! Contract used by the rest of `ui`: [`show`] paints the terminal in the available rect,
//! handles keyboard/mouse/clipboard and reports bytes to send and size changes.
//!
//! Clipboard: egui cannot read the clipboard on demand, so right click and Shift+Insert set
//! [`ViewResponse::paste_from_clipboard`]; the caller reads the clipboard and calls [`paste`].
//! Ctrl+right click (or right click with [`ViewConfig::right_click_paste`] off) sets
//! [`ViewResponse::menu`]; the caller shows the context menu anchored on [`ViewResponse::area`].
//! A confirmed guarded paste is sent with [`paste_confirmed`].

pub mod input;

use egui::text::{LayoutJob, TextFormat};
use egui::{
    Color32, Event, EventFilter, FontFamily, FontId, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, Rect, Sense,
    Stroke, Ui, Vec2, pos2, vec2,
};
use rxt_core::{Rgba, TerminalScheme};
use rxt_term::{
    Cell, Color, Colors, Column, CursorShape, Flags, Match, Point, Rgb, SelectionType, Side, TermMode, Terminal,
    viewport_to_point,
};

use input::{BUTTON_LEFT, BUTTON_MIDDLE, BUTTON_NONE, BUTTON_RIGHT, MOTION, WHEEL_DOWN, WHEEL_UP};

/// Default font size in points when the prefs leave it unset (bundled Cascadia Mono).
pub const DEFAULT_FONT_SIZE: f32 = 13.0;
/// Ctrl+wheel zoom bounds, in points.
const MIN_FONT_SIZE: f32 = 6.0;
const MAX_FONT_SIZE: f32 = 48.0;

/// Family names that select the bundled default monospace font without a warning.
/// Names that mean the default font; `MobaFont` comes with sessions imported before the importer
/// dropped it (no such font on Linux).
const DEFAULT_FAMILIES: [&str; 2] = ["monospace", "MobaFont"];

/// Per-frame widget configuration (resolved global + per-session prefs).
#[derive(Debug, Clone)]
pub struct ViewConfig {
    pub prefs: rxt_core::TerminalPrefs,
    pub scheme: rxt_core::TerminalScheme,
    pub copy_on_select: bool,
    /// Right click pastes; Ctrl+right click (or any right click when off) opens the menu.
    pub right_click_paste: bool,
    pub paste_guard: bool,
}

/// What happened in the widget this frame.
#[derive(Debug, Default)]
pub struct ViewResponse {
    /// Bytes to send to the session (keys, mouse reports, confirmed paste).
    pub input: Vec<u8>,
    /// New grid size in cells, when it changed.
    pub resized: Option<(u16, u16)>,
    /// Multi-line paste waiting for confirmation (paste guard).
    pub paste_request: Option<String>,
    pub has_focus: bool,
    /// Right click or Shift+Insert: read the system clipboard and call [`paste`].
    pub paste_from_clipboard: bool,
    /// Status message when the configured font family is unavailable and the default is used.
    pub font_fallback: Option<String>,
    /// Terminal area, to anchor the context menu.
    pub area: Option<egui::Response>,
    /// Open (`Some(true)`) or close the context menu ([`crate::sidebar::popup_menu`]).
    pub menu: Option<bool>,
    /// New font size in points from Ctrl+wheel zoom; the caller stores it in the prefs.
    pub font_size: Option<f32>,
}

/// Paste `text`, holding it in [`ViewResponse::paste_request`] when the guard applies.
pub fn paste(term: &Terminal, text: &str, config: &ViewConfig, resp: &mut ViewResponse) {
    if text.is_empty() {
        return;
    }
    if config.paste_guard && text.contains(['\n', '\r']) {
        resp.paste_request = Some(text.to_owned());
    } else {
        resp.input.extend(paste_confirmed(term, text));
    }
}

/// Bytes for a paste the user confirmed (bracketed when the application asked for it).
pub fn paste_confirmed(term: &Terminal, text: &str) -> Vec<u8> {
    input::paste_bytes(text, term.mode().contains(TermMode::BRACKETED_PASTE))
}

/// Widget state kept in egui memory between frames.
#[derive(Clone, Copy, Default)]
struct State {
    /// Fractional wheel lines not yet applied.
    wheel: f32,
    /// Last cell reported for mouse motion.
    last_motion: Option<(usize, usize)>,
}

/// Paint `term` and handle input.
pub fn show(ui: &mut Ui, term: &mut Terminal, config: &ViewConfig) -> ViewResponse {
    let mut resp = ViewResponse::default();
    let (mut font, missing) = font_id(ui, config);
    font.size = font.size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
    resp.font_fallback =
        missing.map(|name| format!("Font \"{name}\" is not available; using {}.", crate::theme::MONO_FONTS[0]));
    let (advance, row_h) = ui.ctx().fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
    let ppp = ui.ctx().pixels_per_point();
    let cell = snap_cell(advance, row_h, ppp);
    let (cell_w, cell_h) = (cell.x, cell.y);
    let rect = ui.available_rect_before_wrap();
    // Stable id: banners shown above the terminal (disconnect, notices) must not steal its focus.
    ui.advance_cursor_after_rect(rect);
    let response = ui.interact(rect, ui.id().with("terminal"), Sense::click_and_drag());
    let id = response.id;
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Terminal"));

    // Resize in whole cells.
    let cols = ((rect.width() / cell_w).floor() as u16).max(2);
    let rows = ((rect.height() / cell_h).floor() as u16).max(1);
    if (usize::from(cols), usize::from(rows)) != (term.cols(), term.rows()) {
        term.resize(cols, rows);
        resp.resized = Some((cols, rows));
    }

    let shape = match config.prefs.cursor_shape.unwrap_or_default() {
        rxt_core::CursorShape::Block => CursorShape::Block,
        rxt_core::CursorShape::Underline => CursorShape::Underline,
        rxt_core::CursorShape::Beam => CursorShape::Beam,
    };
    term.set_cursor_style(shape, config.prefs.cursor_blink.unwrap_or(true));
    term.tick();

    if response.clicked() || response.drag_started() || response.secondary_clicked() {
        response.request_focus();
    }
    let focused = response.has_focus();
    resp.has_focus = focused;
    term.set_focused(focused);
    if focused {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true },
            );
        });
    }
    if term.mode().contains(TermMode::FOCUS_IN_OUT) {
        if response.gained_focus() {
            resp.input.extend(b"\x1b[I");
        } else if response.lost_focus() {
            resp.input.extend(b"\x1b[O");
        }
    }

    // The grid starts on a physical pixel, so every cell edge and glyph lands on one (sharp text).
    let origin = pos2(snap(rect.min.x, ppp), snap(rect.min.y, ppp));
    let grid = Grid { origin, cell, cols: term.cols(), rows: term.rows(), spacing: cell_w - advance, px: 1.0 / ppp };
    let mut state: State = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    if focused {
        handle_keyboard(ui, term, config, &mut resp);
    }
    handle_mouse(ui, &response, term, config, &grid, &mut state, &mut resp);
    ui.data_mut(|d| d.insert_temp(id, state));

    paint(ui, rect, term, config, &grid, &font, focused);
    if response.clicked() {
        resp.menu = Some(false);
    }
    resp.area = Some(response);
    resp
}

/// Terminal font and, when the configured family is not loaded, its name (the bundled
/// monospace default is used instead).
fn font_id(ui: &Ui, config: &ViewConfig) -> (FontId, Option<String>) {
    let size = config.prefs.font_size.unwrap_or(DEFAULT_FONT_SIZE);
    let name = config.prefs.font_family.as_deref().map(str::trim).filter(|n| !n.is_empty());
    let Some(name) = name.filter(|n| !DEFAULT_FAMILIES.iter().any(|d| d.eq_ignore_ascii_case(n))) else {
        return (FontId::new(size, FontFamily::Monospace), None);
    };
    let family = FontFamily::Name(name.into());
    if ui.ctx().fonts(|fonts| fonts.families().contains(&family)) {
        (FontId::new(size, family), None)
    } else {
        (FontId::new(size, FontFamily::Monospace), Some(name.to_owned()))
    }
}

/// Cell geometry of the widget.
struct Grid {
    origin: Pos2,
    /// Cell size in points, whole physical pixels ([`snap_cell`]).
    cell: Vec2,
    cols: usize,
    rows: usize,
    /// Extra advance per glyph so a run of text moves exactly one cell per character.
    spacing: f32,
    /// One physical pixel, in points.
    px: f32,
}

/// `v` points rounded to a physical pixel.
fn snap(v: f32, ppp: f32) -> f32 {
    (v * ppp).round() / ppp
}

/// Cell size from the font's advance and row height, in whole physical pixels: the width rounded
/// (glyphs then sit evenly instead of drifting by fractions of a pixel), the height rounded up
/// so glyphs are never clipped.
pub(crate) fn snap_cell(advance: f32, row_h: f32, ppp: f32) -> Vec2 {
    let w = (advance * ppp).round().max(1.0);
    // Tolerance: a height of exactly N px may come out as N.0001 in floating point.
    let h = (row_h * ppp - 0.01).ceil().max(1.0);
    vec2(w / ppp, h / ppp)
}

impl Grid {
    fn cell_rect(&self, col: usize, row: usize, width: usize) -> Rect {
        Rect::from_min_size(
            self.origin + vec2(col as f32 * self.cell.x, row as f32 * self.cell.y),
            vec2(width as f32 * self.cell.x, self.cell.y),
        )
    }

    /// Viewport cell under `pos` (clamped) and the side of the cell it is on.
    fn cell_at(&self, pos: Pos2) -> (usize, usize, Side) {
        let rel = (pos - self.origin) / self.cell;
        let col = (rel.x.max(0.0) as usize).min(self.cols - 1);
        let row = (rel.y.max(0.0) as usize).min(self.rows - 1);
        let side = if rel.x.fract() < 0.5 { Side::Left } else { Side::Right };
        (col, row, side)
    }
}

fn handle_keyboard(ui: &mut Ui, term: &mut Terminal, config: &ViewConfig, resp: &mut ViewResponse) {
    let app_cursor = term.mode().contains(TermMode::APP_CURSOR);
    let before = resp.input.len();
    // Take keyboard events so later handlers in this frame never see them (Ctrl+C stays ours).
    let is_key = |e: &Event| {
        matches!(e, Event::Text(_) | Event::Key { .. } | Event::Copy | Event::Cut | Event::Paste(_) | Event::Ime(_))
    };
    let (mut mods, events) = ui.input_mut(|i| {
        let all = i.events.clone();
        i.events.retain(|e| !is_key(e));
        (i.modifiers, all)
    });
    for event in events {
        match event {
            Event::ModifiersChanged(m) => mods = m,
            // egui-winit turns Ctrl+C / Ctrl+X / Ctrl+V into these; Shift selects the app action.
            Event::Copy if mods.shift => {
                if let Some(text) = term.selection_text() {
                    ui.ctx().copy_text(text);
                }
            }
            Event::Copy => resp.input.extend(alt_prefixed(mods, 0x03)),
            Event::Cut if mods.shift => {}
            Event::Cut => resp.input.extend(alt_prefixed(mods, 0x18)),
            Event::Paste(text) if mods.shift => paste(term, &text, config, resp),
            Event::Paste(_) => resp.input.extend(alt_prefixed(mods, 0x16)),
            Event::Text(text) | Event::Ime(egui::ImeEvent::Commit(text)) => {
                if mods.alt {
                    resp.input.push(0x1b);
                }
                resp.input.extend(text.as_bytes());
            }
            Event::Key { key, pressed: true, modifiers, .. } => match key {
                Key::Insert if modifiers.shift_only() => resp.paste_from_clipboard = true,
                // Font zoom as in GNOME Terminal; Ctrl+_ and Ctrl+/ still send 0x1f.
                Key::Plus | Key::Equals if modifiers.ctrl && !modifiers.alt => zoom(ui, config, resp, 1.0),
                Key::Minus if ctrl_only(modifiers) => zoom(ui, config, resp, -1.0),
                Key::Num0 if ctrl_only(modifiers) => {
                    resp.font_size = Some(DEFAULT_FONT_SIZE);
                    ui.ctx().request_repaint();
                }
                Key::PageUp | Key::PageDown if modifiers.shift_only() => {
                    let page = term.rows() as i32 - 1;
                    term.scroll_lines(if key == Key::PageUp { page } else { -page });
                }
                _ => resp.input.extend(input::key_bytes(key, modifiers, app_cursor).unwrap_or_default()),
            },
            _ => {}
        }
    }
    if resp.input.len() > before {
        term.scroll_to_bottom();
    }
}

/// Step the font size by `step` points into [`ViewResponse::font_size`].
fn zoom(ui: &Ui, config: &ViewConfig, resp: &mut ViewResponse, step: f32) {
    let current = resp.font_size.or(config.prefs.font_size).unwrap_or(DEFAULT_FONT_SIZE);
    resp.font_size = Some((current + step).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE));
    ui.ctx().request_repaint();
}

fn ctrl_only(m: Modifiers) -> bool {
    m.ctrl && !m.shift && !m.alt
}

fn alt_prefixed(mods: Modifiers, byte: u8) -> Vec<u8> {
    if mods.alt { vec![0x1b, byte] } else { vec![byte] }
}

fn handle_mouse(
    ui: &Ui,
    response: &egui::Response,
    term: &mut Terminal,
    config: &ViewConfig,
    grid: &Grid,
    state: &mut State,
    resp: &mut ViewResponse,
) {
    let mode = term.mode();
    let (mods, events, any_down) = ui.input(|i| (i.modifiers, i.events.clone(), i.pointer.any_down()));
    let reporting = mode.intersects(TermMode::MOUSE_MODE) && !mods.shift;
    let sgr = mode.contains(TermMode::SGR_MOUSE);
    let rect = Rect::from_min_size(grid.origin, grid.cell * vec2(grid.cols as f32, grid.rows as f32));
    let hovered = response.hovered();

    for event in &events {
        match *event {
            // Ctrl+wheel zooms the font instead of scrolling.
            Event::MouseWheel { delta, modifiers, .. } if hovered && modifiers.ctrl && delta.y != 0.0 => {
                zoom(ui, config, resp, delta.y.signum());
            }
            Event::MouseWheel { unit, delta, .. } if hovered => {
                let lines = match unit {
                    MouseWheelUnit::Point => delta.y / grid.cell.y,
                    MouseWheelUnit::Line => delta.y,
                    MouseWheelUnit::Page => delta.y * grid.rows as f32,
                };
                state.wheel += lines;
                let whole = state.wheel.trunc();
                state.wheel -= whole;
                let n = whole as i32;
                if n == 0 {
                    continue;
                }
                if reporting {
                    let pos = ui.input(|i| i.pointer.hover_pos()).unwrap_or(rect.min);
                    let (col, row, _) = grid.cell_at(pos);
                    let code = if n > 0 { WHEEL_UP } else { WHEEL_DOWN } + input::mouse_modifiers(mods);
                    for _ in 0..n.unsigned_abs() {
                        resp.input.extend(input::mouse_report(sgr, code, col, row, true).unwrap_or_default());
                    }
                } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
                    let key = if n > 0 { Key::ArrowUp } else { Key::ArrowDown };
                    let app_cursor = mode.contains(TermMode::APP_CURSOR);
                    for _ in 0..n.unsigned_abs() {
                        resp.input.extend(input::key_bytes(key, Modifiers::NONE, app_cursor).unwrap_or_default());
                    }
                } else {
                    term.scroll_lines(n);
                }
            }
            Event::PointerButton { pos, button, pressed, modifiers } if reporting => {
                if pressed && !rect.contains(pos) {
                    continue;
                }
                let code = match button {
                    PointerButton::Primary => BUTTON_LEFT,
                    PointerButton::Middle => BUTTON_MIDDLE,
                    PointerButton::Secondary => BUTTON_RIGHT,
                    _ => continue,
                };
                let (col, row, _) = grid.cell_at(pos);
                let code = code + input::mouse_modifiers(modifiers);
                resp.input.extend(input::mouse_report(sgr, code, col, row, pressed).unwrap_or_default());
                state.last_motion = Some((col, row));
            }
            Event::PointerMoved(pos) if reporting && rect.contains(pos) => {
                let cell = grid.cell_at(pos);
                if state.last_motion == Some((cell.0, cell.1)) {
                    continue;
                }
                let button = ui.input(|i| {
                    [
                        (PointerButton::Primary, BUTTON_LEFT),
                        (PointerButton::Middle, BUTTON_MIDDLE),
                        (PointerButton::Secondary, BUTTON_RIGHT),
                    ]
                    .into_iter()
                    .find(|(b, _)| i.pointer.button_down(*b))
                    .map(|(_, code)| code)
                });
                let report = match button {
                    Some(_) => mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION),
                    None => mode.contains(TermMode::MOUSE_MOTION),
                };
                if report {
                    let code = MOTION + button.unwrap_or(BUTTON_NONE) + input::mouse_modifiers(mods);
                    resp.input.extend(input::mouse_report(sgr, code, cell.0, cell.1, true).unwrap_or_default());
                    state.last_motion = Some((cell.0, cell.1));
                }
            }
            _ => {}
        }
    }
    if reporting {
        return;
    }

    // Local selection and paste.
    let offset = term.display_offset();
    let to_point = |pos: Pos2| {
        let (col, row, side) = grid.cell_at(pos);
        (viewport_to_point(offset, Point::new(row, Column(col))), side)
    };
    let pointer = response.interact_pointer_pos();
    let mut selection_done = false;
    if let Some(pos) = pointer {
        let (point, side) = to_point(pos);
        if response.triple_clicked() {
            term.start_selection(SelectionType::Lines, point, side);
            selection_done = true;
        } else if response.double_clicked() {
            term.start_selection(SelectionType::Semantic, point, side);
            selection_done = true;
        } else if response.drag_started_by(PointerButton::Primary) {
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
            let (start, start_side) = to_point(origin);
            term.start_selection(SelectionType::Simple, start, start_side);
            term.update_selection(point, side);
        } else if response.dragged_by(PointerButton::Primary) {
            term.update_selection(point, side);
        } else if response.clicked_by(PointerButton::Primary) {
            term.clear_selection();
        }
    }
    if response.drag_stopped_by(PointerButton::Primary) {
        selection_done = true;
    }
    if selection_done
        && config.copy_on_select
        && let Some(text) = term.selection_text()
    {
        ui.ctx().copy_text(text);
    }
    if response.secondary_clicked() {
        if config.right_click_paste && !mods.ctrl {
            resp.paste_from_clipboard = true;
        } else {
            resp.menu = Some(true);
        }
    }
    if any_down && response.dragged() {
        ui.ctx().request_repaint();
    }
}

// ---------------------------------------------------------------------------------------------
// Painting

fn c32(c: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(c.0, c.1, c.2, c.3)
}

fn rgb(c: Rgb) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

/// Resolves terminal colors against the scheme and colors changed by the application (OSC 4).
struct Palette<'a> {
    scheme: &'a TerminalScheme,
    colors: &'a Colors,
}

impl Palette<'_> {
    fn indexed(&self, i: usize) -> Color32 {
        if let Some(c) = self.colors[i] {
            return rgb(c);
        }
        let s = self.scheme;
        match i {
            0..16 => c32(s.ansi[i]),
            16..232 => {
                let v = |n: usize| if n == 0 { 0 } else { (55 + 40 * n) as u8 };
                let i = i - 16;
                Color32::from_rgb(v(i / 36), v(i / 6 % 6), v(i % 6))
            }
            232..256 => {
                let g = (8 + 10 * (i - 232)) as u8;
                Color32::from_rgb(g, g, g)
            }
            256 | 267 => c32(s.foreground),
            257 => c32(s.background),
            258 => c32(s.cursor),
            259..267 => self.dim(c32(s.ansi[i - 259])),
            _ => self.dim(c32(s.foreground)),
        }
    }

    fn color(&self, c: Color) -> Color32 {
        match c {
            Color::Spec(c) => rgb(c),
            Color::Indexed(i) => self.indexed(usize::from(i)),
            Color::Named(n) => self.indexed(n as usize),
        }
    }

    fn dim(&self, c: Color32) -> Color32 {
        c.lerp_to_gamma(c32(self.scheme.background), 0.33)
    }

    /// Foreground and background of a cell, after bold-bright, dim, inverse and hidden.
    fn cell(&self, cell: &Cell) -> (Color32, Color32) {
        let fg = match cell.fg {
            Color::Named(n) if cell.flags.contains(Flags::BOLD) && (n as usize) < 8 => Color::Named(n.to_bright()),
            Color::Indexed(i) if cell.flags.contains(Flags::BOLD) && i < 8 => Color::Indexed(i + 8),
            c => c,
        };
        let mut fg = self.color(fg);
        let mut bg = self.color(cell.bg);
        if cell.flags.contains(Flags::DIM) {
            fg = self.dim(fg);
        }
        if cell.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if cell.flags.contains(Flags::HIDDEN) {
            fg = bg;
        }
        (fg, bg)
    }
}

/// A horizontal run of cells sharing one style.
struct Run {
    col: usize,
    width: usize,
    text: String,
    fg: Color32,
    bg: Color32,
    flags: Flags,
}

const STYLE_FLAGS: Flags = Flags::BOLD.union(Flags::ITALIC).union(Flags::ALL_UNDERLINES).union(Flags::STRIKEOUT);

fn paint(ui: &Ui, rect: Rect, term: &mut Terminal, config: &ViewConfig, grid: &Grid, font: &FontId, focused: bool) {
    let painter = ui.painter_at(rect);
    let scheme = &config.scheme;
    let default_bg = c32(scheme.background);
    painter.rect_filled(rect, 0.0, default_bg);

    let matches = term.search_visible_matches();
    let focused_match = term.search_focused().cloned();
    let blinking = term.cursor_blinking() && focused;
    let content = term.renderable();
    let pal = Palette { scheme, colors: content.colors };
    let offset = content.display_offset;
    let selection = content.selection;
    let cursor = content.cursor;
    let cursor_vp = rxt_term::point_to_viewport(offset, cursor.point);

    let highlight = |point: Point, fg: Color32, bg: Color32| -> (Color32, Color32) {
        let in_match = |m: &Match| m.contains(&point);
        if focused_match.as_ref().is_some_and(in_match) {
            (c32(scheme.ansi[0]), c32(scheme.ansi[3]))
        } else if matches.iter().any(in_match) {
            (c32(scheme.ansi[0]), c32(scheme.ansi[11]))
        } else if selection.as_ref().is_some_and(|s| s.contains(point)) {
            (fg, c32(scheme.selection))
        } else {
            (fg, bg)
        }
    };

    let mut rows: Vec<Vec<Run>> = (0..grid.rows).map(|_| Vec::new()).collect();
    let mut cursor_char = ' ';
    for item in content.display_iter {
        let Some(vp) = rxt_term::point_to_viewport(offset, item.point) else { continue };
        let Some(runs) = rows.get_mut(vp.line) else { continue };
        let cell = item.cell;
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        if cursor_vp == Some(vp) {
            cursor_char = cell.c;
        }
        let (fg, bg) = pal.cell(cell);
        let (fg, bg) = highlight(item.point, fg, bg);
        let flags = cell.flags & STYLE_FLAGS;
        let wide = cell.flags.contains(Flags::WIDE_CHAR);
        let width = if wide { 2 } else { 1 };
        match runs.last_mut() {
            // Wide glyphs get their own run so the grid stays aligned whatever their advance.
            Some(r)
                if !wide
                    && (r.fg, r.bg, r.flags) == (fg, bg, flags)
                    && r.col + r.width == vp.column.0
                    && r.width == r.text.chars().count() =>
            {
                r.text.push(cell.c);
                r.width += 1;
            }
            _ => runs.push(Run { col: vp.column.0, width, text: cell.c.to_string(), fg, bg, flags }),
        }
        if let Some(zw) = cell.zerowidth()
            && let Some(r) = runs.last_mut()
        {
            r.text.extend(zw);
        }
    }

    // Lines one physical pixel thick (whole pixels at fractional scales).
    let thin = grid.px * (1.0 / grid.px).round().max(1.0);
    // Backgrounds first so underlines at the row bottom are not covered by the next row.
    for (row, runs) in rows.iter().enumerate() {
        for run in runs.iter().filter(|r| r.bg != default_bg) {
            painter.rect_filled(grid.cell_rect(run.col, row, run.width), 0.0, run.bg);
        }
    }
    for (row, runs) in rows.iter().enumerate() {
        for run in runs {
            if run.text.trim().is_empty() && !run.flags.intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT) {
                continue;
            }
            // egui caches galleys by job, so unchanged runs are not laid out again.
            let r = grid.cell_rect(run.col, row, run.width);
            let pos = r.min;
            let galley = painter.layout_job(text_job(&run.text, font, run.fg, run.flags, thin, grid.spacing));
            painter.galley(pos, galley.clone(), run.fg);
            if run.flags.intersects(Flags::ALL_UNDERLINES) {
                // Drawn inside the cell: egui's underline sits on the row boundary.
                painter.hline(r.x_range(), r.max.y - 1.5 * thin, Stroke::new(thin, run.fg));
            }
            if run.flags.contains(Flags::BOLD) {
                // Synthetic bold: overstrike shifted by one pixel.
                painter.galley(pos + vec2(thin, 0.0), galley, run.fg);
            }
        }
    }

    // Cursor.
    let Some(vp) = cursor_vp else { return };
    let visible = !blinking || (ui.input(|i| i.time) * 2.0) as i64 % 2 == 0;
    if blinking {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    }
    let r = grid.cell_rect(vp.column.0, vp.line, 1);
    let color = c32(scheme.cursor);
    let shape = if focused { cursor.shape } else { CursorShape::HollowBlock };
    match shape {
        CursorShape::Hidden => {}
        _ if !visible => {}
        CursorShape::Block => {
            painter.rect_filled(r, 0.0, color);
            if !cursor_char.is_whitespace() {
                let job = text_job(&cursor_char.to_string(), font, default_bg, Flags::empty(), thin, grid.spacing);
                painter.galley(r.min, painter.layout_job(job), default_bg);
            }
        }
        CursorShape::Underline => {
            painter.rect_filled(Rect::from_min_max(pos2(r.min.x, r.max.y - 2.0 * thin), r.max), 0.0, color);
        }
        CursorShape::Beam => {
            painter.rect_filled(Rect::from_min_max(r.min, pos2(r.min.x + thin, r.max.y)), 0.0, color);
        }
        CursorShape::HollowBlock => {
            painter.rect_stroke(r.shrink(thin / 2.0), 0.0, Stroke::new(thin, color), egui::StrokeKind::Inside);
        }
    }
}

fn text_job(text: &str, font: &FontId, color: Color32, flags: Flags, thin: f32, spacing: f32) -> LayoutJob {
    let line = |on: bool| if on { Stroke::new(thin, color) } else { Stroke::NONE };
    let format = TextFormat {
        font_id: font.clone(),
        color,
        italics: flags.contains(Flags::ITALIC),
        strikethrough: line(flags.contains(Flags::STRIKEOUT)),
        extra_letter_spacing: spacing,
        ..Default::default()
    };
    LayoutJob::single_section(text.to_owned(), format)
}

#[cfg(test)]
mod tests;
