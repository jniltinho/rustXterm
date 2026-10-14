use egui::{Event, FontId, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use rxt_core::{TerminalPrefs, TerminalScheme};
use rxt_term::Terminal;

use super::{ViewConfig, show};

struct State {
    term: Terminal,
    config: ViewConfig,
    input: Vec<u8>,
    paste_request: Option<String>,
    paste_from_clipboard: bool,
    /// Whether a keyboard event was still visible to a handler running after the widget.
    leaked_events: bool,
    origin: Pos2,
    /// Rect the widget was given (the work area in the app).
    rect: egui::Rect,
    has_focus: bool,
    font_fallback: Option<String>,
}

fn harness(term: Terminal, dark: bool) -> Harness<'static, State> {
    let config = ViewConfig {
        prefs: TerminalPrefs { cursor_blink: Some(false), ..TerminalPrefs::default() },
        scheme: TerminalScheme::for_dark(dark),
        copy_on_select: true,
        paste_guard: true,
    };
    let state = State {
        term,
        config,
        input: Vec::new(),
        paste_request: None,
        paste_from_clipboard: false,
        leaked_events: false,
        origin: Pos2::ZERO,
        rect: egui::Rect::NOTHING,
        has_focus: false,
        font_fallback: None,
    };
    Harness::builder().with_size(vec2(480.0, 220.0)).build_ui_state(
        |ui, s: &mut State| {
            s.rect = ui.available_rect_before_wrap();
            s.origin = s.rect.min;
            let r = show(ui, &mut s.term, &s.config);
            s.has_focus = r.has_focus;
            s.font_fallback = r.font_fallback;
            s.input.extend(r.input);
            s.paste_request = s.paste_request.take().or(r.paste_request);
            s.paste_from_clipboard |= r.paste_from_clipboard;
            s.leaked_events |= ui.input(|i| i.events.iter().any(|e| matches!(e, Event::Copy | Event::Key { .. })));
        },
        state,
    )
}

/// Center of viewport cell (`col`, `row`) with the default font.
fn cell_center(h: &Harness<'_, State>, col: usize, row: usize) -> Pos2 {
    let font = FontId::monospace(super::DEFAULT_FONT_SIZE);
    let (w, hgt) = h.ctx.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
    h.state().origin + vec2((col as f32 + 0.5) * w, (row as f32 + 0.5) * hgt)
}

fn click(h: &mut Harness<'_, State>, (col, row): (usize, usize), button: PointerButton) {
    let pos = cell_center(h, col, row);
    h.event(Event::PointerMoved(pos));
    h.event(Event::PointerButton { pos, button, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.event(Event::PointerButton { pos, button, pressed: false, modifiers: Modifiers::NONE });
    h.step();
}

fn sample() -> Terminal {
    let mut t = Terminal::new(40, 8, 100);
    t.feed(b"\x1b[1mbold\x1b[0m \x1b[3mitalic\x1b[0m \x1b[4munderline\x1b[0m \x1b[9mstrike\x1b[0m\r\n");
    for i in 0..16 {
        t.feed(format!("\x1b[48;5;{i}m  ").as_bytes());
    }
    t.feed(b"\x1b[0m\r\n");
    for i in 0..24 {
        t.feed(format!("\x1b[48;5;{}m ", 232 + i).as_bytes());
    }
    t.feed(b"\x1b[0m\r\n\x1b[38;2;255;100;0mtruecolor\x1b[0m \x1b[7minverse\x1b[0m \x1b[31mred\x1b[0m \x1b[1;32mbright\x1b[0m\r\n");
    t.feed("wide: \u{4e2d}\u{6587} ok\r\nuser@lab:~$ ".as_bytes());
    t
}

#[test]
fn snapshots_light_and_dark() {
    let mut results = egui_kittest::SnapshotResults::new();
    for (dark, name) in [(false, "terminal_light"), (true, "terminal_dark")] {
        let mut h = harness(sample(), dark);
        h.run_steps(2);
        // Linux (lavapipe) images are the reference; other targets only render (add-windows-port W10).
        if cfg!(target_os = "linux") {
            results.add(h.try_snapshot(name));
        }
    }
    results.unwrap();
}

#[test]
fn reports_resize_in_cells() {
    let mut h = harness(Terminal::new(10, 2, 10), false);
    h.step();
    let (cols, rows) = (h.state().term.cols(), h.state().term.rows());
    assert!(cols > 10 && rows > 2, "{cols}x{rows}");
}

#[test]
fn ctrl_c_sends_etx_and_is_not_leaked() {
    let mut h = harness(Terminal::new(20, 4, 10), false);
    h.step();
    click(&mut h, (1, 1), PointerButton::Primary);
    // egui-winit delivers Ctrl+C as Event::Copy; other integrations as a key event.
    h.event_modifiers(Event::Copy, Modifiers::CTRL);
    h.key_press_modifiers(Modifiers::CTRL, egui::Key::C);
    h.event(Event::Text("ls".into()));
    h.key_press(egui::Key::Enter);
    h.step();
    assert_eq!(h.state().input, b"\x03\x03ls\r");
    assert!(!h.state().leaked_events);
}

#[test]
fn paste_guard_and_bracketed_paste() {
    let mut t = Terminal::new(20, 4, 10);
    t.feed(b"\x1b[?2004h");
    let mut h = harness(t, false);
    h.step();
    click(&mut h, (0, 0), PointerButton::Primary);
    // Ctrl+Shift+V with three lines is held for confirmation.
    h.event_modifiers(Event::Paste("one\ntwo\nthree".into()), Modifiers::CTRL | Modifiers::SHIFT);
    h.step();
    assert_eq!(h.state().paste_request.as_deref(), Some("one\ntwo\nthree"));
    assert!(h.state().input.is_empty());
    // Single line goes straight through, bracketed.
    h.event_modifiers(Event::Paste("echo hi".into()), Modifiers::CTRL | Modifiers::SHIFT);
    h.step();
    assert_eq!(h.state().input, b"\x1b[200~echo hi\x1b[201~");
    // Plain Ctrl+V is a control character.
    h.state_mut().input.clear();
    h.event_modifiers(Event::Paste("x".into()), Modifiers::CTRL);
    h.step();
    assert_eq!(h.state().input, [0x16]);
    // Right click asks the caller for the clipboard.
    click(&mut h, (2, 0), PointerButton::Secondary);
    assert!(h.state().paste_from_clipboard);
}

#[test]
fn mouse_clicks_report_sgr_and_x10() {
    for (modes, expected) in [
        (&b"\x1b[?1000h\x1b[?1006h"[..], b"\x1b[<0;4;3M\x1b[<0;4;3m\x1b[<2;4;3M\x1b[<2;4;3m".to_vec()),
        (&b"\x1b[?1000h"[..], b"\x1b[M\x20\x24\x23\x1b[M\x23\x24\x23\x1b[M\x22\x24\x23\x1b[M\x23\x24\x23".to_vec()),
    ] {
        let mut t = Terminal::new(20, 4, 10);
        t.feed(modes);
        let mut h = harness(t, false);
        h.step();
        click(&mut h, (3, 2), PointerButton::Primary);
        click(&mut h, (3, 2), PointerButton::Secondary);
        assert_eq!(h.state().input, expected, "{:?}", String::from_utf8_lossy(&h.state().input));
        assert!(!h.state().paste_from_clipboard, "right click is reported, not pasted");
    }
}

#[test]
fn wheel_scrolls_back_or_reports() {
    let mut t = Terminal::new(20, 4, 100);
    for i in 0..50 {
        t.feed(format!("line {i}\r\n").as_bytes());
    }
    let mut h = harness(t, false);
    h.step();
    let pos = cell_center(&h, 1, 1);
    h.event(Event::PointerMoved(pos));
    h.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: vec2(0.0, 3.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    });
    h.step();
    assert_eq!(h.state().term.display_offset(), 3);
    assert!(h.state().input.is_empty());

    h.state_mut().term.feed(b"\x1b[?1000h\x1b[?1006h");
    h.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: vec2(0.0, -1.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    });
    h.step();
    assert_eq!(h.state().input, b"\x1b[<65;2;2M");
}

#[test]
fn ctrl_wheel_zooms_font_without_scrolling() {
    let mut t = Terminal::new(20, 4, 100);
    for i in 0..50 {
        t.feed(format!("line {i}\r\n").as_bytes());
    }
    let mut h = harness(t, false);
    h.step();
    let cols_before = h.state().term.cols();
    let pos = cell_center(&h, 1, 1);
    h.event(Event::PointerMoved(pos));
    let ctrl_wheel = |dy: f32| Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: vec2(0.0, dy),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::CTRL,
    };
    h.event(ctrl_wheel(1.0));
    h.event(ctrl_wheel(1.0));
    h.step();
    h.step();
    assert_eq!(h.state().term.font_zoom(), 2.0);
    assert_eq!(h.state().term.display_offset(), 0, "Ctrl+wheel must not scroll");
    assert!(h.state().term.cols() < cols_before, "a bigger font fits fewer columns");
    for _ in 0..200 {
        h.event(ctrl_wheel(-1.0));
    }
    h.step();
    h.step();
    assert_eq!(h.state().term.font_zoom(), 6.0 - super::DEFAULT_FONT_SIZE, "clamped at the minimum size");
}

#[test]
fn drag_selects_text() {
    let mut t = Terminal::new(20, 4, 10);
    t.feed(b"hello world");
    let mut h = harness(t, false);
    h.step();
    // Left half of the first cell to the right half of the fifth.
    let (a, b) = (cell_center(&h, 0, 0) - vec2(2.0, 0.0), cell_center(&h, 4, 0) + vec2(2.0, 0.0));
    h.event(Event::PointerMoved(a));
    h.event(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for x in 1..=8 {
        h.event(Event::PointerMoved(a.lerp(b, x as f32 / 8.0)));
        h.step();
    }
    h.event(Event::PointerButton {
        pos: b,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.step();
    assert_eq!(h.state().term.selection_text().as_deref(), Some("hello"));
    let copied = h
        .output()
        .platform_output
        .commands
        .iter()
        .any(|c| matches!(c, egui::OutputCommand::CopyText(t) if t == "hello"));
    assert!(copied, "copy on select");
}

#[test]
fn default_font_is_13px() {
    assert!((super::DEFAULT_FONT_SIZE - 13.0).abs() < f32::EPSILON);
}

#[test]
fn unknown_font_family_falls_back_with_message() {
    let mut h = harness(Terminal::new(10, 2, 10), false);
    h.step();
    assert_eq!(h.state().font_fallback, None, "the default family needs no message");
    h.state_mut().config.prefs.font_family = Some("No Such Font".into());
    h.step();
    let msg = h.state().font_fallback.clone().unwrap_or_default();
    assert!(msg.contains("No Such Font") && msg.contains("Cascadia Mono"), "{msg}");
    h.state_mut().config.prefs.font_family = Some("monospace".into());
    h.step();
    assert_eq!(h.state().font_fallback, None);
}

/// Pixel at logical position `p` of a rendered frame.
fn pixel(img: &image::RgbaImage, ppp: f32, p: Pos2) -> [u8; 3] {
    let px = img.get_pixel((p.x * ppp) as u32, (p.y * ppp) as u32).0;
    [px[0], px[1], px[2]]
}

fn near(a: [u8; 3], b: egui::Color32) -> bool {
    a.iter().zip([b.r(), b.g(), b.b()]).all(|(x, y)| x.abs_diff(y) <= 2)
}

#[test]
fn fills_its_rect_edge_to_edge_in_both_themes() {
    for dark in [false, true] {
        let mut t = Terminal::new(20, 4, 10);
        // A red cell in the top-left corner, cursor parked away from every edge.
        t.feed(b"\x1b[41m \x1b[0m\r\n\r\n  ");
        let mut h = harness(t, dark);
        h.run_steps(2);
        let img = h.render().expect("render");
        let ppp = h.ctx.pixels_per_point();
        let (rect, scheme) = (h.state().rect, h.state().config.scheme.clone());
        let bg = super::c32(scheme.background);
        let red = super::c32(scheme.ansi[1]);
        // The cell grid starts at the corner: the first cell is red 1 px inside it.
        assert!(near(pixel(&img, ppp, rect.min + vec2(1.0, 1.0)), red), "dark={dark}: grid inset");
        // 1 px inside every edge (the leftover strips right and bottom included) is background.
        let (l, r, t, b) = (rect.min.x + 1.0, rect.max.x - 2.0, rect.min.y + 1.0, rect.max.y - 2.0);
        let c = rect.center();
        for p in [
            egui::pos2(r, t),
            egui::pos2(c.x, t),
            egui::pos2(l, b),
            egui::pos2(r, b),
            egui::pos2(l, c.y),
            egui::pos2(r, c.y),
            egui::pos2(c.x, b),
        ] {
            assert!(near(pixel(&img, ppp, p), bg), "dark={dark}: {p:?} is {:?}, want {bg:?}", pixel(&img, ppp, p));
        }
    }
}

#[test]
fn tab_arrows_and_function_keys_stay_in_the_terminal() {
    let mut h = harness(Terminal::new(20, 4, 10), false);
    h.step();
    click(&mut h, (1, 1), PointerButton::Primary);
    // egui applies the focus lock (Tab stays) from the frame after focus was taken.
    h.step();
    for key in [egui::Key::Tab, egui::Key::ArrowUp, egui::Key::ArrowLeft, egui::Key::F5, egui::Key::Escape] {
        h.key_press(key);
        h.step();
    }
    h.key_press_modifiers(Modifiers::SHIFT, egui::Key::Tab);
    h.step();
    assert_eq!(h.state().input, b"\t\x1b[A\x1b[D\x1b[15~\x1b\x1b[Z");
    assert!(h.state().has_focus, "focus stays on the terminal");
    assert!(!h.state().leaked_events);
}
