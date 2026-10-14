//! Remote-desktop (RDP/VNC) tab widget (design D13): one texture per tab updated from the
//! framebuffer's dirty rectangles, scale-to-fit or 1:1 painting, pointer/keyboard mapping to
//! [`DesktopInput`] and a keyboard grab released by a configurable key combination.

use std::time::{Duration, Instant};

use egui::{
    Color32, ColorImage, CursorIcon, Event, EventFilter, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, Rect,
    Sense, TextureHandle, TextureOptions, Vec2,
};
use rxt_core::desktop::{MAX_EDGE, RemoteCursor};
use rxt_core::{DesktopInput, Framebuffer};

/// Wait this long after the tab size last changed before asking for a desktop resize.
const RESIZE_SETTLE: Duration = Duration::from_millis(400);
/// The grab hint strip stays this long (seconds) after focus enters the view.
pub(crate) const HINT_SECS: f64 = 3.0;
/// Height of the grab hint strip.
const HINT_HEIGHT: f32 = 20.0;
/// No "Connecting" label for faster connections (seconds).
const LOADING_DELAY: f64 = 0.2;
/// Letterbox and backdrop color in both themes.
pub(crate) const LETTERBOX: Color32 = Color32::BLACK;
/// Text on the black backdrop (both themes).
const BACKDROP_TEXT: Color32 = Color32::from_gray(0xf0);

/// Per-tab widget state.
#[derive(Default)]
pub(crate) struct DesktopView {
    texture: Option<TextureHandle>,
    /// Keyboard input goes to the remote side (app shortcuts included).
    pub grabbed: bool,
    mods: Modifiers,
    buttons: u8,
    /// Keys sent down through the key path, with the keysym used, so key-up matches.
    down: Vec<(Key, u32)>,
    settle: Option<([u32; 2], Instant)>,
    sent_size: Option<[u32; 2]>,
    /// `egui` time when the grab hint strip was shown (focus entered the view).
    hint_since: Option<f64>,
    /// `egui` time since the view has been waiting for the connection or the first frame.
    waiting_since: Option<f64>,
    cursor: Cursor,
}

/// Remote pointer shape as drawn by the view.
#[derive(Default)]
enum Cursor {
    /// Local OS arrow.
    #[default]
    Default,
    Hidden,
    /// Texture and hotspot, in desktop pixels.
    Bitmap(TextureHandle, Vec2),
}

/// Release combination: modifiers, optionally with a key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Release {
    pub mods: Modifiers,
    pub key: Option<Key>,
}

impl Default for Release {
    fn default() -> Self {
        Self { mods: Modifiers::CTRL | Modifiers::ALT, key: None }
    }
}

/// Parse `Ctrl+Alt` or `Ctrl+Alt+End`-style text; at least one modifier is required.
pub(crate) fn parse_release(text: &str) -> Option<Release> {
    let mut r = Release { mods: Modifiers::NONE, key: None };
    for part in text.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => r.mods |= Modifiers::CTRL,
            "shift" => r.mods |= Modifiers::SHIFT,
            "alt" => r.mods |= Modifiers::ALT,
            _ if r.key.is_none() => r.key = Some(Key::from_name(part)?),
            _ => return None,
        }
    }
    (r.mods.ctrl || r.mods.alt || r.mods.shift).then_some(r)
}

fn same_mods(a: Modifiers, b: Modifiers) -> bool {
    (a.ctrl, a.alt, a.shift) == (b.ctrl, b.alt, b.shift)
}

/// Per-frame configuration.
pub(crate) struct Config {
    pub view_only: bool,
    /// Paint scaled to the tab (aspect preserved) instead of 1:1 with scrolling.
    pub fit: bool,
    /// Ask the server to follow the tab size (RDP fit-to-tab).
    pub resize: bool,
    pub release: Release,
    pub release_text: String,
}

/// What happened this frame.
#[derive(Debug, Default)]
pub(crate) struct Response {
    pub input: Vec<DesktopInput>,
    /// The keyboard grab started this frame (sync the local clipboard).
    pub grab_started: bool,
}

impl DesktopView {
    /// Upload `fb` to the texture: the whole image when the size changed, else only dirty rects.
    pub(crate) fn sync(&mut self, ctx: &egui::Context, name: &str, fb: &mut Framebuffer) {
        if std::mem::take(&mut fb.cursor_dirty) {
            self.cursor = match &fb.cursor {
                RemoteCursor::Default => Cursor::Default,
                RemoteCursor::Hidden => Cursor::Hidden,
                RemoteCursor::Bitmap { width, height, hot_x, hot_y, rgba } => {
                    let img = ColorImage::from_rgba_unmultiplied([*width as usize, *height as usize], rgba);
                    let tex = ctx.load_texture(format!("{name}-cursor"), img, TextureOptions::LINEAR);
                    Cursor::Bitmap(tex, egui::vec2(*hot_x as f32, *hot_y as f32))
                }
            };
        }
        let size = [fb.width as usize, fb.height as usize];
        let dirty = fb.take_dirty();
        if size[0] == 0 || size[1] == 0 || fb.pixels.len() != size[0] * size[1] * 4 {
            return;
        }
        match &mut self.texture {
            Some(t) if t.size() == size => {
                for r in dirty {
                    if let Some(img) = rect_image(fb, r) {
                        t.set_partial([r.x as usize, r.y as usize], img, TextureOptions::LINEAR);
                    }
                }
            }
            _ => {
                let img = ColorImage::from_rgba_unmultiplied(size, &fb.pixels);
                self.texture = Some(ctx.load_texture(name, img, TextureOptions::LINEAR));
            }
        }
    }

    /// Drop the grab: every pressed key and button is released on the remote side.
    fn release(&mut self, out: &mut Vec<DesktopInput>) {
        self.grabbed = false;
        self.mods = Modifiers::NONE;
        self.buttons = 0;
        self.down.clear();
        out.push(DesktopInput::ReleaseAll);
    }

    /// Send modifier key changes so the remote modifier state matches `m`.
    fn sync_mods(&mut self, m: Modifiers, out: &mut Vec<DesktopInput>) {
        let old = self.mods;
        for (was, now, keysym) in [
            (old.shift, m.shift, keysym::SHIFT_L),
            (old.ctrl, m.ctrl, keysym::CONTROL_L),
            (old.alt, m.alt, keysym::ALT_L),
            (old.mac_cmd, m.mac_cmd, keysym::SUPER_L),
        ] {
            if was != now {
                out.push(DesktopInput::Key { keysym, down: now });
            }
        }
        self.mods = m;
    }

    fn key(&mut self, key: Key, pressed: bool, out: &mut Vec<DesktopInput>) {
        if pressed {
            // Printable keys without Ctrl also arrive as `Event::Text`, which carries the layout's character.
            if is_printable(key) && !self.mods.ctrl {
                return;
            }
            if let Some(keysym) = key_keysym(key, self.mods.shift) {
                self.down.push((key, keysym));
                out.push(DesktopInput::Key { keysym, down: true });
            }
        } else if let Some(i) = self.down.iter().position(|(k, _)| *k == key) {
            let (_, keysym) = self.down.remove(i);
            out.push(DesktopInput::Key { keysym, down: false });
        }
    }
}

/// Paint the desktop and handle its input.
pub(crate) fn show(ui: &mut egui::Ui, view: &mut DesktopView, cfg: &Config) -> Response {
    let mut resp = Response::default();
    let avail = ui.available_rect_before_wrap();
    // Edge to edge: whatever the framebuffer does not cover is letterbox.
    ui.painter().rect_filled(avail, 0.0, LETTERBOX);
    if cfg.resize {
        let ppp = ui.ctx().pixels_per_point();
        let want = desktop_size(avail.size() * ppp);
        let now = Instant::now();
        match view.settle {
            Some((size, since)) if size == want => {
                if now.duration_since(since) >= RESIZE_SETTLE && view.sent_size != Some(want) {
                    view.sent_size = Some(want);
                    let current = view.texture.as_ref().map(|t| t.size());
                    if current != Some([want[0] as usize, want[1] as usize]) {
                        resp.input.push(DesktopInput::Resize { width: want[0], height: want[1] });
                    }
                }
            }
            _ => {
                view.settle = Some((want, now));
                ui.ctx().request_repaint_after(RESIZE_SETTLE);
            }
        }
    }
    let now = ui.input(|i| i.time);
    let Some(tex) = view.texture.as_ref().map(|t| (t.id(), t.size_vec2())) else {
        connecting(ui, avail, view, now, "Waiting for the remote desktop...");
        return resp;
    };
    view.waiting_since = None;
    let (tex_id, size) = tex;
    let sense = if cfg.view_only { Sense::hover() } else { Sense::click_and_drag() };
    let (area, image) = if cfg.fit {
        let r = ui.allocate_rect(avail, sense);
        (r, fit_rect(avail, size))
    } else {
        let mut out = None;
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            let (rect, r) = ui.allocate_exact_size(size, sense);
            out = Some((r, rect));
        });
        match out {
            Some(o) => o,
            None => return resp,
        }
    };
    area.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Remote desktop"));
    let painter = ui.painter_at(area.rect);
    let uv = Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0));
    painter.image(tex_id, image, uv, Color32::WHITE);
    paint_cursor(ui, &painter, view, &area, image, size);
    if cfg.view_only {
        // The hint shows once when the desktop appears.
        view.hint_since.get_or_insert(now);
    } else {
        let fb = [size.x as u32, size.y as u32];
        input(ui, view, cfg, &area, image, fb, &mut resp);
        if resp.grab_started {
            view.hint_since = Some(now);
        }
    }
    hint_strip(ui, avail, view, cfg, now);
    resp
}

/// Remote pointer at the local mouse position (scaled like the desktop) instead of the OS arrow.
fn paint_cursor(
    ui: &egui::Ui,
    painter: &egui::Painter,
    view: &DesktopView,
    area: &egui::Response,
    image: Rect,
    size: Vec2,
) {
    if matches!(view.cursor, Cursor::Default) || !(area.hovered() || area.dragged()) {
        return;
    }
    ui.ctx().set_cursor_icon(CursorIcon::None);
    let (Cursor::Bitmap(tex, hot), Some(pos)) = (&view.cursor, ui.input(|i| i.pointer.latest_pos())) else {
        return;
    };
    let scale = if size.x > 0.0 { image.width() / size.x } else { 1.0 };
    let rect = Rect::from_min_size(pos - *hot * scale, tex.size_vec2() * scale);
    painter.image(tex.id(), rect, Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)), Color32::WHITE);
}

/// Text of the grab hint strip.
pub(crate) fn hint_text(view_only: bool, release: &str) -> String {
    if view_only {
        "View only: no input is sent.".to_owned()
    } else {
        format!("Keyboard grabbed: press {release} to release it.")
    }
}

/// 20 px strip at the top of the view naming the grab state, for [`HINT_SECS`] after focus enters.
fn hint_strip(ui: &egui::Ui, avail: Rect, view: &DesktopView, cfg: &Config, now: f64) {
    let Some(since) = view.hint_since else { return };
    let left = HINT_SECS - (now - since);
    if left <= 0.0 || !(cfg.view_only || view.grabbed) {
        return;
    }
    ui.ctx().request_repaint_after(Duration::from_secs_f64(left));
    let strip = Rect::from_min_size(avail.min, egui::vec2(avail.width(), HINT_HEIGHT));
    let painter = ui.painter_at(strip);
    painter.rect_filled(strip, 0.0, Color32::from_black_alpha(200));
    let font = egui::TextStyle::Small.resolve(ui.style());
    painter.text(
        strip.center(),
        egui::Align2::CENTER_CENTER,
        hint_text(cfg.view_only, &cfg.release_text),
        font,
        BACKDROP_TEXT,
    );
}

/// Centered spinner and `text` on the black backdrop once waiting lasted [`LOADING_DELAY`].
fn connecting(ui: &mut egui::Ui, avail: Rect, view: &mut DesktopView, now: f64, text: &str) {
    let since = *view.waiting_since.get_or_insert(now);
    if now - since < LOADING_DELAY {
        ui.ctx().request_repaint_after(Duration::from_secs_f64(LOADING_DELAY));
        return;
    }
    ui.scope_builder(egui::UiBuilder::new().max_rect(avail), |ui| {
        ui.centered_and_justified(|ui| {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().color(BACKDROP_TEXT));
                ui.label(egui::RichText::new(text).color(BACKDROP_TEXT));
            });
        });
    });
}

/// Connection state of a remote-desktop tab that is not connected: "Connecting to <host>..."
/// (after 200 ms) or the reason with a centered Reconnect button, edge to edge on the black
/// backdrop. Returns true when Reconnect was clicked.
pub(crate) fn state_ui(ui: &mut egui::Ui, view: &mut DesktopView, host: &str, state: &rxt_core::ConnState) -> bool {
    use rxt_core::ConnState;
    let avail = ui.available_rect_before_wrap();
    ui.painter().rect_filled(avail, 0.0, LETTERBOX);
    let now = ui.input(|i| i.time);
    let reason = match state {
        ConnState::Connected => return false,
        ConnState::Connecting => {
            connecting(ui, avail, view, now, &format!("Connecting to {host}..."));
            return false;
        }
        ConnState::Disconnected { reason, .. } => format!("Disconnected from {host}: {reason}"),
        ConnState::Error(e) => format!("Cannot connect to {host}: {e}"),
    };
    view.waiting_since = None;
    let mut reconnect = false;
    let height = 2.0 * ui.spacing().interact_size.y + 30.0;
    let inner = Rect::from_center_size(avail.center(), egui::vec2(avail.width(), height));
    ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new(reason).color(BACKDROP_TEXT));
            ui.add_space(8.0);
            reconnect = ui.add(egui::Button::new("Reconnect").min_size(egui::vec2(80.0, 30.0))).clicked();
        });
    });
    reconnect
}

fn input(
    ui: &mut egui::Ui,
    view: &mut DesktopView,
    cfg: &Config,
    area: &egui::Response,
    image: Rect,
    fb: [u32; 2],
    resp: &mut Response,
) {
    if area.clicked() || area.drag_started() || area.secondary_clicked() || area.middle_clicked() {
        area.request_focus();
        if !view.grabbed {
            view.grabbed = true;
            resp.grab_started = true;
        }
    }
    if view.grabbed && !area.has_focus() && !resp.grab_started {
        view.release(&mut resp.input);
    }
    let out = &mut resp.input;
    if view.grabbed {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                area.id,
                EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true },
            );
        });
        let is_key = |e: &Event| {
            matches!(e, Event::Text(_) | Event::Key { .. } | Event::Copy | Event::Cut | Event::Paste(_) | Event::Ime(_))
        };
        let (events, final_mods) = ui.input_mut(|i| {
            let all = i.events.clone();
            i.events.retain(|e| !is_key(e));
            (all, i.modifiers)
        });
        let mut released = false;
        for event in events {
            match event {
                Event::ModifiersChanged(m) => {
                    if cfg.release.key.is_none() && same_mods(m, cfg.release.mods) {
                        released = true;
                        break;
                    }
                    view.sync_mods(m, out);
                }
                Event::Key { key, pressed, modifiers, .. } => {
                    if pressed && cfg.release.key == Some(key) && same_mods(modifiers, cfg.release.mods) {
                        released = true;
                        break;
                    }
                    view.sync_mods(modifiers, out);
                    view.key(key, pressed, out);
                }
                Event::Text(text) | Event::Ime(egui::ImeEvent::Commit(text)) => {
                    for c in text.chars().filter(|c| !c.is_control()) {
                        let keysym = char_keysym(c);
                        out.extend([
                            DesktopInput::Key { keysym, down: true },
                            DesktopInput::Key { keysym, down: false },
                        ]);
                    }
                }
                // egui turns Ctrl+C/X/V into these; Ctrl is already down on the remote side.
                Event::Copy | Event::Cut | Event::Paste(_) => {
                    let c = match event {
                        Event::Copy => 'c',
                        Event::Cut => 'x',
                        _ => 'v',
                    };
                    let keysym = char_keysym(c);
                    out.extend([DesktopInput::Key { keysym, down: true }, DesktopInput::Key { keysym, down: false }]);
                }
                _ => {}
            }
        }
        if released {
            view.release(out);
            area.surrender_focus();
        } else {
            view.sync_mods(final_mods, out);
        }
    }
    pointer(ui, view, area, image, fb, out);
}

fn pointer(
    ui: &egui::Ui,
    view: &mut DesktopView,
    area: &egui::Response,
    image: Rect,
    fb: [u32; 2],
    out: &mut Vec<DesktopInput>,
) {
    let events = ui.input(|i| i.events.clone());
    let hovered = area.hovered();
    for event in events {
        match event {
            Event::PointerMoved(pos) if area.rect.contains(pos) => {
                let (x, y) = to_desktop(pos, image, fb);
                out.push(DesktopInput::Pointer { x, y, buttons: view.buttons });
            }
            Event::PointerButton { pos, button, pressed, .. } => {
                let bit = match button {
                    PointerButton::Primary => 1,
                    PointerButton::Middle => 2,
                    PointerButton::Secondary => 4,
                    _ => continue,
                };
                if pressed && !area.rect.contains(pos) || !pressed && view.buttons & bit == 0 {
                    continue;
                }
                view.buttons = if pressed { view.buttons | bit } else { view.buttons & !bit };
                let (x, y) = to_desktop(pos, image, fb);
                out.push(DesktopInput::Pointer { x, y, buttons: view.buttons });
            }
            Event::MouseWheel { unit, delta, .. } if hovered => {
                let pos = ui.input(|i| i.pointer.hover_pos()).unwrap_or(image.min);
                let (x, y) = to_desktop(pos, image, fb);
                let (dx, dy) = (wheel_steps(unit, delta.x), wheel_steps(unit, delta.y));
                if dx != 0 || dy != 0 {
                    out.push(DesktopInput::Wheel { x, y, dx, dy });
                }
            }
            _ => {}
        }
    }
}

fn wheel_steps(unit: MouseWheelUnit, delta: f32) -> i16 {
    let steps = match unit {
        MouseWheelUnit::Point => delta / 40.0,
        MouseWheelUnit::Line | MouseWheelUnit::Page => delta,
    };
    let n = steps.round().clamp(-100.0, 100.0) as i16;
    if n == 0 && delta != 0.0 { delta.signum() as i16 } else { n }
}

/// Largest rect inside `avail` with the aspect ratio of `size`, centered.
pub(crate) fn fit_rect(avail: Rect, size: Vec2) -> Rect {
    if size.x <= 0.0 || size.y <= 0.0 {
        return avail;
    }
    let scale = (avail.width() / size.x).min(avail.height() / size.y);
    Rect::from_center_size(avail.center(), size * scale)
}

/// Pointer position on screen → desktop pixel (inverse of the painted scaling), clamped.
pub(crate) fn to_desktop(pos: Pos2, image: Rect, fb: [u32; 2]) -> (u32, u32) {
    let axis = |p: f32, min: f32, len: f32, n: u32| {
        if len <= 0.0 || n == 0 {
            return 0;
        }
        (((p - min) / len * n as f32).floor().max(0.0) as u32).min(n - 1)
    };
    (axis(pos.x, image.min.x, image.width(), fb[0]), axis(pos.y, image.min.y, image.height(), fb[1]))
}

/// Desktop size for a tab of `px` physical pixels: even width, within RDP display-control limits.
pub(crate) fn desktop_size(px: Vec2) -> [u32; 2] {
    let w = (px.x.max(0.0) as u32).clamp(200, MAX_EDGE) & !1;
    let h = (px.y.max(0.0) as u32).clamp(200, MAX_EDGE);
    [w, h]
}

/// RGBA bytes of `r` copied out of the framebuffer, or `None` when empty or out of bounds.
pub(crate) fn rect_pixels(fb: &Framebuffer, r: rxt_core::Rect) -> Option<Vec<u8>> {
    let fits = u64::from(r.x) + u64::from(r.width) <= u64::from(fb.width)
        && u64::from(r.y) + u64::from(r.height) <= u64::from(fb.height);
    if r.area() == 0 || !fits {
        return None;
    }
    let row = r.width as usize * 4;
    let mut out = Vec::with_capacity(row * r.height as usize);
    for y in r.y..r.y + r.height {
        let at = (y as usize * fb.width as usize + r.x as usize) * 4;
        out.extend_from_slice(fb.pixels.get(at..at + row)?);
    }
    Some(out)
}

fn rect_image(fb: &Framebuffer, r: rxt_core::Rect) -> Option<ColorImage> {
    let px = rect_pixels(fb, r)?;
    Some(ColorImage::from_rgba_unmultiplied([r.width as usize, r.height as usize], &px))
}

/// X11 keysyms used for key mapping.
pub(crate) mod keysym {
    pub const SHIFT_L: u32 = 0xffe1;
    pub const CONTROL_L: u32 = 0xffe3;
    pub const ALT_L: u32 = 0xffe9;
    pub const SUPER_L: u32 = 0xffeb;
}

/// Keys that also produce `Event::Text` when typed without Ctrl.
fn is_printable(key: Key) -> bool {
    key_char(key).is_some()
}

/// Character of a printable key on the US layout (unshifted).
fn key_char(key: Key) -> Option<char> {
    let name = key.name();
    let c = match key {
        Key::Space => ' ',
        Key::Colon => ':',
        Key::Comma => ',',
        Key::Backslash => '\\',
        Key::Slash => '/',
        Key::Pipe => '|',
        Key::Questionmark => '?',
        Key::Exclamationmark => '!',
        Key::OpenBracket => '[',
        Key::CloseBracket => ']',
        Key::OpenCurlyBracket => '{',
        Key::CloseCurlyBracket => '}',
        Key::Backtick => '`',
        Key::Minus => '-',
        Key::Period => '.',
        Key::Plus => '+',
        Key::Equals => '=',
        Key::Semicolon => ';',
        Key::Quote => '\'',
        _ => {
            let mut chars = name.chars();
            let c = chars.next()?;
            // Letters are named "A".."Z", digits "0".."9".
            if chars.next().is_some() || !c.is_ascii_alphanumeric() {
                return None;
            }
            c.to_ascii_lowercase()
        }
    };
    Some(c)
}

/// X11 keysym for an egui key; letters follow `shift`.
pub(crate) fn key_keysym(key: Key, shift: bool) -> Option<u32> {
    if let Some(c) = key_char(key) {
        let c = if shift { c.to_ascii_uppercase() } else { c };
        return Some(c as u32);
    }
    let sym = match key {
        Key::Escape => 0xff1b,
        Key::Tab => 0xff09,
        Key::Backspace => 0xff08,
        Key::Enter => 0xff0d,
        Key::Insert => 0xff63,
        Key::Delete => 0xffff,
        Key::Home => 0xff50,
        Key::ArrowLeft => 0xff51,
        Key::ArrowUp => 0xff52,
        Key::ArrowRight => 0xff53,
        Key::ArrowDown => 0xff54,
        Key::PageUp => 0xff55,
        Key::PageDown => 0xff56,
        Key::End => 0xff57,
        Key::F1 => 0xffbe,
        Key::F2 => 0xffbf,
        Key::F3 => 0xffc0,
        Key::F4 => 0xffc1,
        Key::F5 => 0xffc2,
        Key::F6 => 0xffc3,
        Key::F7 => 0xffc4,
        Key::F8 => 0xffc5,
        Key::F9 => 0xffc6,
        Key::F10 => 0xffc7,
        Key::F11 => 0xffc8,
        Key::F12 => 0xffc9,
        _ => return None,
    };
    Some(sym)
}

/// Keysym of a typed character: Latin-1 maps directly, the rest to `0x0100_0000 + codepoint`.
pub(crate) fn char_keysym(c: char) -> u32 {
    let cp = c as u32;
    if (0x20..=0x7e).contains(&cp) || (0xa0..=0xff).contains(&cp) { cp } else { 0x0100_0000 + cp }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;
    use rxt_core::{DesktopEvent, Rect as DRect};

    #[test]
    fn keysym_samples() {
        assert_eq!(key_keysym(Key::A, false), Some(0x61));
        assert_eq!(key_keysym(Key::A, true), Some(0x41));
        assert_eq!(key_keysym(Key::Num7, false), Some(0x37));
        assert_eq!(key_keysym(Key::F1, false), Some(0xffbe));
        assert_eq!(key_keysym(Key::F12, false), Some(0xffc9));
        assert_eq!(key_keysym(Key::Enter, false), Some(0xff0d));
        assert_eq!(key_keysym(Key::Escape, false), Some(0xff1b));
        assert_eq!(key_keysym(Key::Delete, false), Some(0xffff));
        assert_eq!(key_keysym(Key::PageDown, false), Some(0xff56));
        assert_eq!(key_keysym(Key::Slash, false), Some(0x2f));
        assert_eq!(key_keysym(Key::Space, false), Some(0x20));
        assert_eq!(char_keysym('a'), 0x61);
        assert_eq!(char_keysym('é'), 0xe9);
        assert_eq!(char_keysym('€'), 0x0100_20ac);
        assert_eq!(char_keysym('ж'), 0x0100_0436);
    }

    #[test]
    fn release_combination_parses() {
        assert_eq!(parse_release("Ctrl+Alt"), Some(Release::default()));
        let r = parse_release("ctrl+shift+End").unwrap();
        assert_eq!((r.mods, r.key), (Modifiers::CTRL | Modifiers::SHIFT, Some(Key::End)));
        assert_eq!(parse_release("End"), None);
        assert_eq!(parse_release("Ctrl+A+B"), None);
    }

    #[test]
    fn pointer_coordinates_invert_scaling() {
        // 1000x500 desktop fitted into a 500x500 area: scaled by 0.5, letterboxed vertically.
        let avail = Rect::from_min_size(egui::pos2(100.0, 50.0), vec2(500.0, 500.0));
        let image = fit_rect(avail, vec2(1000.0, 500.0));
        assert_eq!(image, Rect::from_min_size(egui::pos2(100.0, 175.0), vec2(500.0, 250.0)));
        let fb = [1000, 500];
        assert_eq!(to_desktop(image.min, image, fb), (0, 0));
        assert_eq!(to_desktop(egui::pos2(350.0, 300.0), image, fb), (500, 250));
        assert_eq!(to_desktop(egui::pos2(10_000.0, -5.0), image, fb), (999, 0));
        // 1:1.
        let one = Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(1000.0, 500.0));
        assert_eq!(to_desktop(egui::pos2(12.5, 40.0), one, fb), (12, 40));
    }

    #[test]
    fn dirty_rect_extraction_copies_only_the_rect() {
        let mut fb = Framebuffer::default();
        fb.apply(&DesktopEvent::Resized { width: 4, height: 3 }).unwrap();
        for (i, b) in fb.pixels.iter_mut().enumerate() {
            *b = i as u8;
        }
        let r = DRect { x: 1, y: 1, width: 2, height: 2 };
        let px = rect_pixels(&fb, r).unwrap();
        let expect: Vec<u8> = [(1, 1), (2, 1), (1, 2), (2, 2)]
            .iter()
            .flat_map(|&(x, y)| ((y * 4 + x) * 4..(y * 4 + x) * 4 + 4).map(|v| v as u8))
            .collect();
        assert_eq!(px, expect);
        assert!(rect_pixels(&fb, DRect { x: 3, y: 0, width: 2, height: 1 }).is_none());
        assert!(rect_pixels(&fb, DRect { x: 0, y: 0, width: 0, height: 1 }).is_none());
    }

    #[test]
    fn sync_uploads_dirty_rects_and_clears_them() {
        let ctx = egui::Context::default();
        let mut fb = Framebuffer::default();
        fb.apply(&DesktopEvent::Resized { width: 8, height: 8 }).unwrap();
        let mut view = DesktopView::default();
        view.sync(&ctx, "t", &mut fb);
        assert_eq!(view.texture.as_ref().map(|t| t.size()), Some([8, 8]));
        let rect = DRect { x: 2, y: 2, width: 2, height: 2 };
        fb.apply(&DesktopEvent::Update { rect, rgba: vec![7; 16] }).unwrap();
        view.sync(&ctx, "t", &mut fb);
        assert!(fb.dirty.is_empty());
    }

    struct Ui {
        view: DesktopView,
        fb: Framebuffer,
        cfg: Config,
        rect: Rect,
        /// Connection state shown through [`state_ui`] instead of the desktop when set.
        state: Option<rxt_core::ConnState>,
        reconnect: bool,
    }

    fn desktop_harness(size: egui::Vec2, fb: [u32; 2], dark: bool) -> egui_kittest::Harness<'static, Ui> {
        let mut framebuffer = Framebuffer::default();
        framebuffer.apply(&DesktopEvent::Resized { width: fb[0], height: fb[1] }).unwrap();
        framebuffer.pixels.fill(255);
        let cfg = Config {
            view_only: false,
            fit: true,
            resize: false,
            release: Release::default(),
            release_text: "Ctrl+Alt".into(),
        };
        let state = Ui {
            view: DesktopView::default(),
            fb: framebuffer,
            cfg,
            rect: Rect::NOTHING,
            state: None,
            reconnect: false,
        };
        let h = egui_kittest::Harness::builder().with_size(size).build_ui_state(
            |ui, s: &mut Ui| {
                s.rect = ui.available_rect_before_wrap();
                if let Some(state) = &s.state {
                    s.reconnect |= state_ui(ui, &mut s.view, "rdp.lab.example", state);
                    return;
                }
                s.view.sync(ui.ctx(), "t", &mut s.fb);
                show(ui, &mut s.view, &s.cfg);
            },
            state,
        );
        let tokens = if dark { rxt_core::Tokens::dark() } else { rxt_core::Tokens::light() };
        h.ctx.set_global_style(crate::theme::style(&tokens));
        h
    }

    fn rgb(img: &image::RgbaImage, p: Pos2) -> [u8; 3] {
        let px = img.get_pixel(p.x as u32, p.y as u32).0;
        [px[0], px[1], px[2]]
    }

    #[test]
    fn letterbox_is_black_without_frame_in_both_themes() {
        for dark in [false, true] {
            // 1024x768 scaled to fit a wider area: bars left and right.
            let mut h = desktop_harness(vec2(1200.0, 600.0), [1024, 768], dark);
            h.run_steps(2);
            let img = h.render().unwrap();
            let r = h.state().rect;
            let bar = egui::pos2(r.min.x + 20.0, r.center().y);
            assert_eq!(rgb(&img, bar), [0, 0, 0], "dark={dark}: side bar");
            for corner in [r.min + vec2(1.0, 1.0), egui::pos2(r.max.x - 2.0, r.max.y - 2.0)] {
                assert_eq!(rgb(&img, corner), [0, 0, 0], "dark={dark}: no frame at {corner:?}");
            }
            assert_eq!(rgb(&img, r.center()), [255, 255, 255], "dark={dark}: framebuffer");
        }
    }

    #[test]
    fn grab_hint_shows_for_three_seconds() {
        let mut h = desktop_harness(vec2(400.0, 300.0), [400, 300], false);
        let at = |h: &mut egui_kittest::Harness<'static, Ui>, t: f64| {
            h.input_mut().time = Some(t);
            h.step();
            let img = h.render().unwrap();
            let r = h.state().rect;
            // Left end of the strip, clear of its centered text: dimmed when shown, white otherwise.
            rgb(&img, egui::pos2(r.min.x + 3.0, r.min.y + 10.0)) != [255, 255, 255]
        };
        assert!(!at(&mut h, 1.0), "no strip before focus enters");
        let center = h.state().rect.center();
        h.event(Event::PointerMoved(center));
        h.event(Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        h.input_mut().time = Some(1.1);
        h.step();
        h.event(Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        assert!(at(&mut h, 1.2), "strip after the grab starts");
        assert!(h.state().view.grabbed);
        assert!(at(&mut h, 3.9), "still shown before 3 s");
        assert!(!at(&mut h, 4.3), "hidden after 3 s");
        assert_eq!(hint_text(false, "Ctrl+Alt"), "Keyboard grabbed: press Ctrl+Alt to release it.");
    }

    #[test]
    fn connecting_and_error_states() {
        use egui_kittest::kittest::Queryable;
        use rxt_core::ConnState;
        let mut h = desktop_harness(vec2(600.0, 400.0), [8, 8], true);
        h.state_mut().state = Some(ConnState::Connecting);
        h.input_mut().time = Some(10.0);
        h.step();
        assert!(h.query_by_label_contains("Connecting to").is_none(), "no flash before 200 ms");
        h.input_mut().time = Some(10.3);
        h.step();
        h.get_by_label("Connecting to rdp.lab.example...");
        h.state_mut().state = Some(ConnState::Error("connection refused".into()));
        h.step();
        h.get_by_label("Cannot connect to rdp.lab.example: connection refused");
        h.get_by_label("Reconnect").click();
        h.step();
        assert!(h.state().reconnect);
        let img = h.render().unwrap();
        let r = h.state().rect;
        assert_eq!(rgb(&img, r.min + vec2(1.0, 1.0)), [0, 0, 0], "black backdrop in Dark");
    }

    #[test]
    fn remote_cursor_drawn_at_local_pointer() {
        let red = [255, 0, 0, 255].repeat(64);
        let shapes = [
            (RemoteCursor::Bitmap { width: 8, height: 8, hot_x: 6, hot_y: 6, rgba: red }, CursorIcon::None, true),
            (RemoteCursor::Hidden, CursorIcon::None, false),
            (RemoteCursor::Default, CursorIcon::Default, false),
        ];
        for (shape, icon, drawn) in shapes {
            let mut h = desktop_harness(vec2(400.0, 300.0), [400, 300], false);
            h.state_mut().fb.apply(&DesktopEvent::Cursor(shape.clone())).unwrap();
            h.step();
            let c = h.state().rect.center();
            h.hover_at(c);
            h.run_steps(2);
            assert_eq!(h.output().platform_output.cursor_icon, icon, "{shape:?}");
            let img = h.render().unwrap();
            // Hotspot (6, 6) at the pointer: the image spans pointer-6 .. pointer+2 on each axis.
            let inside = rgb(&img, c - vec2(4.0, 4.0));
            assert_eq!(inside == [255, 0, 0], drawn, "{shape:?}: {inside:?}");
            assert_eq!(rgb(&img, c + vec2(4.0, 4.0)), [255, 255, 255], "{shape:?}: past the cursor");
        }
    }

    #[test]
    fn rdp_size_is_even_and_bounded() {
        assert_eq!(desktop_size(vec2(1001.0, 700.0)), [1000, 700]);
        assert_eq!(desktop_size(vec2(10.0, 10.0)), [200, 200]);
        assert_eq!(desktop_size(vec2(1e6, 1e6)), [MAX_EDGE, MAX_EDGE]);
    }
}
