//! Terminal model on `alacritty_terminal`: OSC 7 scanner, selection, search,
//! scrollback limit and logging (design D3). No UI dependency.
//!
//! Contract used by `ui` (keep stable; extend, don't break):
//! - [`Terminal::feed`] parses output bytes and returns [`TermEvent`]s
//!   (cwd changes from OSC 7, title, bell, bytes to write back such as DA replies).
//! - The egui widget in `crates/ui/src/terminal/` renders from `Terminal`.

mod log;
mod osc7;

use std::io;
use std::ops::RangeInclusive;
use std::path::Path;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Direction};
use alacritty_terminal::selection::Selection;
use alacritty_terminal::term::search::{RegexIter, RegexSearch};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{CursorStyle, Handler, Processor, StdSyncHandler};

pub use alacritty_terminal::index::{Column, Line, Point, Side};
pub use alacritty_terminal::selection::SelectionType;
pub use alacritty_terminal::term::cell::{Cell, Flags};
pub use alacritty_terminal::term::color::Colors;
pub use alacritty_terminal::term::{RenderableContent, TermMode, point_to_viewport, viewport_to_point};
pub use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};
pub use log::LogFormat;
pub use osc7::MAX_PAYLOAD as OSC7_MAX_PAYLOAD;

/// Value of `TERM` advertised to remote sessions; the emulation follows xterm-256color.
pub const TERM_NAME: &str = "xterm-256color";

/// A search match, inclusive, in grid coordinates (negative lines are scrollback).
pub type Match = RangeInclusive<Point>;

/// Event produced while feeding output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermEvent {
    CwdChanged(String),
    Title(String),
    Bell,
    /// Bytes the terminal must send back to the remote (device attribute replies, etc.).
    Reply(Vec<u8>),
}

/// Invalid search pattern.
#[derive(Debug, thiserror::Error)]
#[error("invalid search pattern: {0}")]
pub struct SearchError(String);

#[derive(Clone, Default)]
struct Listener(Arc<Mutex<Vec<TermEvent>>>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let ev = match event {
            Event::Title(t) => TermEvent::Title(t),
            Event::ResetTitle => TermEvent::Title(String::new()),
            Event::Bell => TermEvent::Bell,
            Event::PtyWrite(s) => TermEvent::Reply(s.into_bytes()),
            // ponytail: color queries (OSC 10/11) and pixel-size requests go unanswered;
            // answer them once the scheme is known to the model.
            _ => return,
        };
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(ev);
    }
}

struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

impl Size {
    fn new(cols: u16, rows: u16) -> Self {
        Self { cols: usize::from(cols.max(2)), rows: usize::from(rows.max(1)) }
    }
}

struct Search {
    regex: RegexSearch,
    focused: Option<Match>,
}

/// Terminal state for one tab.
pub struct Terminal {
    term: Term<Listener>,
    parser: Processor<StdSyncHandler>,
    events: Listener,
    config: Config,
    osc7: osc7::Osc7Scanner,
    log: Option<log::Log>,
    log_error: Option<io::Error>,
    search: Option<Search>,
}

impl Terminal {
    pub fn new(cols: u16, rows: u16, scrollback_lines: usize) -> Self {
        let config = Config { scrolling_history: scrollback_lines, ..Config::default() };
        let events = Listener::default();
        let term = Term::new(config.clone(), &Size::new(cols, rows), events.clone());
        Self {
            term,
            parser: Processor::new(),
            events,
            config,
            osc7: osc7::Osc7Scanner::default(),
            log: None,
            log_error: None,
            search: None,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<TermEvent> {
        let mut out = Vec::new();
        self.osc7.scan(bytes, |path| out.push(TermEvent::CwdChanged(path)));
        if let Some(log) = &mut self.log
            && let Err(e) = log.write(bytes)
        {
            self.log = None;
            self.log_error = Some(e);
        }
        self.parser.advance(&mut self.term, bytes);
        self.tick();
        out.append(&mut self.events.0.lock().unwrap_or_else(|e| e.into_inner()));
        out
    }

    /// Apply a synchronized update (DECSET 2026) whose timeout expired. Call once per frame.
    pub fn tick(&mut self) {
        if self.parser.sync_bytes_count() > 0
            && self.parser.sync_timeout().sync_timeout().is_some_and(|t| t <= std::time::Instant::now())
        {
            self.parser.stop_sync(&mut self.term);
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let size = Size::new(cols, rows);
        if size.cols != self.term.columns() || size.rows != self.term.screen_lines() {
            self.term.resize(size);
            self.search_clear_focus();
        }
    }

    pub fn cols(&self) -> usize {
        self.term.columns()
    }

    pub fn rows(&self) -> usize {
        self.term.screen_lines()
    }

    /// Mode flags: `APP_CURSOR`, `BRACKETED_PASTE`, `MOUSE_MODE`, `SGR_MOUSE`, `ALT_SCREEN`...
    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    /// Everything needed to paint the viewport.
    pub fn renderable(&self) -> RenderableContent<'_> {
        self.term.renderable_content()
    }

    /// Whether the active cursor style blinks (default style or set by DECSCUSR).
    pub fn cursor_blinking(&self) -> bool {
        self.term.cursor_style().blinking
    }

    /// Default cursor style used until the application sets one.
    pub fn set_cursor_style(&mut self, shape: CursorShape, blinking: bool) {
        let style = CursorStyle { shape, blinking };
        if self.config.default_cursor_style != style {
            self.config.default_cursor_style = style;
            self.term.set_options(self.config.clone());
        }
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.term.is_focused = focused;
    }

    // ---- scrollback ----

    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    pub fn history_size(&self) -> usize {
        self.term.history_size()
    }

    /// Scroll the viewport; positive moves up into the scrollback.
    pub fn scroll_lines(&mut self, delta: i32) {
        self.term.scroll_display(Scroll::Delta(delta));
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    /// Drop the scrollback; the screen stays.
    pub fn clear_history(&mut self) {
        self.term.selection = None;
        self.search_clear_focus();
        self.term.grid_mut().clear_history();
    }

    /// Full reset (RIS): screen, scrollback, modes, colors and cursor style.
    pub fn reset(&mut self) {
        self.search_clear_focus();
        self.term.reset_state();
    }

    // ---- selection ----

    pub fn start_selection(&mut self, ty: SelectionType, point: Point, side: Side) {
        self.term.selection = Some(Selection::new(ty, point, side));
    }

    pub fn update_selection(&mut self, point: Point, side: Side) {
        if let Some(sel) = &mut self.term.selection {
            sel.update(point, side);
        }
    }

    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    /// Select the scrollback and the screen.
    pub fn select_all(&mut self) {
        let start = Point::new(self.term.topmost_line(), Column(0));
        let mut sel = Selection::new(SelectionType::Lines, start, Side::Left);
        sel.update(Point::new(self.term.bottommost_line(), self.term.last_column()), Side::Right);
        self.term.selection = Some(sel);
    }

    /// Selected text, `None` without a (non-empty) selection.
    pub fn selection_text(&self) -> Option<String> {
        self.term.selection.as_ref().filter(|s| !s.is_empty())?;
        self.term.selection_to_string().filter(|s| !s.is_empty())
    }

    // ---- search ----

    /// Set the literal search text; an empty string clears the search.
    pub fn set_search(&mut self, text: &str, case_sensitive: bool) -> Result<(), SearchError> {
        if text.is_empty() {
            self.search = None;
            return Ok(());
        }
        let flags = if case_sensitive { "(?-i)" } else { "(?i)" };
        let regex =
            RegexSearch::new(&format!("{flags}{}", escape_regex(text))).map_err(|e| SearchError(e.to_string()))?;
        self.search = Some(Search { regex, focused: None });
        Ok(())
    }

    /// Move to the next (`forward`) or previous match, wrapping around, and scroll to it.
    pub fn search_next(&mut self, forward: bool) -> Option<Match> {
        let search = self.search.as_mut()?;
        let (origin, direction, side) = match (&search.focused, forward) {
            (Some(m), true) => (m.end().add(&self.term, Boundary::None, 1), Direction::Right, Side::Left),
            (Some(m), false) => (m.start().sub(&self.term, Boundary::None, 1), Direction::Left, Side::Right),
            (None, true) => (self.term.grid().cursor.point, Direction::Right, Side::Left),
            (None, false) => (self.term.grid().cursor.point, Direction::Left, Side::Right),
        };
        let found = self.term.search_next(&mut search.regex, origin, direction, side, None);
        if let Some(m) = &found {
            self.term.scroll_to_point(*m.start());
        }
        search.focused.clone_from(&found);
        found
    }

    /// Current match selected by [`Self::search_next`].
    pub fn search_focused(&self) -> Option<&Match> {
        self.search.as_ref()?.focused.as_ref()
    }

    /// Matches intersecting the viewport, to highlight.
    pub fn search_visible_matches(&mut self) -> Vec<Match> {
        let Some(search) = self.search.as_mut() else { return Vec::new() };
        let top = -(self.term.grid().display_offset() as i32);
        let start = Point::new(Line(top), Column(0));
        let end = Point::new(Line(top + self.term.screen_lines() as i32 - 1), self.term.last_column());
        RegexIter::new(start, end, Direction::Right, &self.term, &mut search.regex).collect()
    }

    fn search_clear_focus(&mut self) {
        if let Some(s) = &mut self.search {
            s.focused = None;
        }
    }

    // ---- text and logging ----

    /// Text of the visible viewport, one line per row, trailing spaces trimmed.
    pub fn visible_text(&self) -> String {
        let mut lines = vec![String::new(); self.rows()];
        let content = self.term.renderable_content();
        let offset = content.display_offset;
        for cell in content.display_iter {
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            if let Some(p) = point_to_viewport(offset, cell.point)
                && let Some(line) = lines.get_mut(p.line)
            {
                line.push(cell.c);
                line.extend(cell.zerowidth().into_iter().flatten());
            }
        }
        let mut text: String =
            lines.iter().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n").trim_end_matches('\n').to_owned();
        text.push('\n');
        text
    }

    /// Save [`Self::visible_text`] to `path`.
    pub fn save_visible_text(&self, path: &Path) -> io::Result<()> {
        std::fs::write(path, self.visible_text())
    }

    /// Append future output to `path` (raw bytes or plain text).
    pub fn start_log(&mut self, path: &Path, format: LogFormat) -> io::Result<()> {
        self.log = Some(log::Log::open(path, format)?);
        self.log_error = None;
        Ok(())
    }

    pub fn stop_log(&mut self) {
        self.log = None;
    }

    pub fn is_logging(&self) -> bool {
        self.log.is_some()
    }

    /// Write error that stopped logging, if any (taken once).
    pub fn take_log_error(&mut self) -> Option<io::Error> {
        self.log_error.take()
    }
}

fn escape_regex(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term_with(bytes: &[u8]) -> Terminal {
        let mut t = Terminal::new(20, 5, 100);
        t.feed(bytes);
        t
    }

    #[test]
    fn osc7_events_and_replies() {
        let mut t = Terminal::new(20, 5, 100);
        let ev = t.feed(b"\x1b]7;file://lab/home/lab%20user\x07\x1b]0;hi\x07\x07\x1b[c");
        assert_eq!(ev[0], TermEvent::CwdChanged("/home/lab user".into()));
        assert!(ev.contains(&TermEvent::Title("hi".into())));
        assert!(ev.contains(&TermEvent::Bell));
        assert!(ev.iter().any(|e| matches!(e, TermEvent::Reply(r) if r.starts_with(b"\x1b[?"))));
        // Split across calls.
        assert!(t.feed(b"\x1b]7;file://lab/sr").is_empty());
        assert_eq!(t.feed(b"v\x1b\\"), [TermEvent::CwdChanged("/srv".into())]);
    }

    #[test]
    fn colors_and_attributes() {
        let t = term_with(b"\x1b[1;3;4;38;2;10;200;30;48;5;196mA\x1b[0m\x1b[7mB");
        let cells: Vec<Cell> = t.renderable().display_iter.take(2).map(|c| c.cell.clone()).collect();
        assert_eq!(cells[0].fg, Color::Spec(Rgb { r: 10, g: 200, b: 30 }));
        assert_eq!(cells[0].bg, Color::Indexed(196));
        assert!(cells[0].flags.contains(Flags::BOLD | Flags::ITALIC | Flags::UNDERLINE));
        assert!(cells[1].flags.contains(Flags::INVERSE));
        assert_eq!(TERM_NAME, "xterm-256color");
    }

    #[test]
    fn scrollback_is_bounded() {
        let mut t = Terminal::new(10, 3, 50);
        for i in 0..500 {
            t.feed(format!("{i}\r\n").as_bytes());
        }
        assert_eq!(t.history_size(), 50);
        t.scroll_lines(1000);
        assert_eq!(t.display_offset(), 50);
        t.scroll_to_bottom();
        assert_eq!(t.display_offset(), 0);
    }

    #[test]
    fn select_all_clear_history_and_reset() {
        let mut t = Terminal::new(10, 3, 50);
        t.feed(b"a\r\nb\r\nc\r\nd\r\ne\x1b[?1h");
        t.select_all();
        assert_eq!(t.selection_text().as_deref(), Some("a\nb\nc\nd\ne\n"));
        t.clear_history();
        assert_eq!((t.history_size(), t.selection_text()), (0, None));
        assert!(t.visible_text().starts_with("c\n"));
        t.reset();
        assert_eq!(t.visible_text(), "\n");
        assert!(!t.mode().contains(TermMode::APP_CURSOR));
    }

    #[test]
    fn modes_are_exposed() {
        let t = term_with(b"\x1b[?1h\x1b[?2004h\x1b[?1000h\x1b[?1006h");
        let m = t.mode();
        assert!(m.contains(TermMode::APP_CURSOR | TermMode::BRACKETED_PASTE | TermMode::MOUSE_REPORT_CLICK));
        assert!(m.contains(TermMode::SGR_MOUSE));
        assert!(!term_with(b"").mode().intersects(TermMode::MOUSE_MODE));
    }

    #[test]
    fn selection_text() {
        let mut t = term_with(b"hello world\r\nsecond line");
        assert_eq!(t.selection_text(), None);
        t.start_selection(SelectionType::Simple, Point::new(Line(0), Column(6)), Side::Left);
        t.update_selection(Point::new(Line(1), Column(5)), Side::Right);
        assert_eq!(t.selection_text().as_deref(), Some("world\nsecond"));
        t.start_selection(SelectionType::Semantic, Point::new(Line(0), Column(1)), Side::Left);
        assert_eq!(t.selection_text().as_deref(), Some("hello"));
        t.clear_selection();
        assert_eq!(t.selection_text(), None);
    }

    #[test]
    fn search_next_prev_and_case() {
        let mut t = Terminal::new(20, 3, 100);
        for line in ["Error one", "ok", "error two", "ok", "ok", "ok", "x.y"] {
            t.feed(format!("{line}\r\n").as_bytes());
        }
        t.set_search("error", true).unwrap();
        let m = t.search_next(false).unwrap();
        assert_eq!(m.start().column, Column(0));
        let line_two = m.start().line;
        // Scrolled into view.
        assert!(point_to_viewport(t.display_offset(), *m.start()).is_some());
        assert_eq!(t.search_visible_matches(), vec![m.clone()]);
        // Only one case-sensitive match: wraps to itself.
        assert_eq!(t.search_next(false).unwrap().start().line, line_two);

        t.set_search("error", false).unwrap();
        let a = t.search_next(false).unwrap();
        let b = t.search_next(false).unwrap();
        assert_eq!(a.start().line, line_two);
        assert!(b.start().line < a.start().line);
        assert_eq!(t.search_next(true).unwrap(), a);
        assert_eq!(t.search_focused(), Some(&a));

        // Literal, not a regex.
        t.set_search("x.y", true).unwrap();
        assert!(t.search_next(false).is_some());
        t.set_search("x+y", true).unwrap();
        assert!(t.search_next(false).is_none());
    }

    #[test]
    fn visible_text_and_save() {
        let t = term_with(b"\x1b[31mred\x1b[0m  \r\n\xe4\xb8\xad\xe6\x96\x87 ok");
        assert_eq!(t.visible_text(), "red\n\u{4e2d}\u{6587} ok\n");
        let dir = std::env::temp_dir().join(format!("rxt-term-visible-{}", std::process::id()));
        t.save_visible_text(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(&dir).unwrap(), t.visible_text());
        std::fs::remove_file(dir).unwrap();
    }

    #[test]
    fn log_raw_and_plain() {
        let out = b"\x1b[01;34mdir\x1b[0m  file.txt\r\n\x1b]0;title\x07\tend\r\n";
        let base = std::env::temp_dir().join(format!("rxt-term-log-{}", std::process::id()));
        for (format, expected) in
            [(LogFormat::Raw, out.to_vec()), (LogFormat::Plain, b"dir  file.txt\n\tend\n".to_vec())]
        {
            let path = base.with_extension(format!("{format:?}"));
            let _ = std::fs::remove_file(&path);
            let mut t = Terminal::new(40, 5, 10);
            t.start_log(&path, format).unwrap();
            assert!(t.is_logging());
            // Split mid escape sequence: the plain parser keeps state across feeds.
            t.feed(&out[..3]);
            t.feed(&out[3..]);
            t.stop_log();
            t.feed(b"not logged");
            assert_eq!(std::fs::read(&path).unwrap(), expected, "{format:?}");
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn log_open_error() {
        let mut t = Terminal::new(10, 2, 10);
        assert!(t.start_log(Path::new("/nonexistent-dir/x.log"), LogFormat::Raw).is_err());
        assert!(!t.is_logging());
    }
}
