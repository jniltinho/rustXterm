//! Engine-side plain-text tail of a terminal tab, so agent `read`/`wait` work headless.
//! Escape sequences are stripped by a small state machine (CSI, OSC/DCS strings, two-byte
//! escapes); `\r` followed by anything but `\n` restarts the line, backspace erases one byte.
//! Screen-addressed output (full-screen apps) reads as its printable bytes, not as a screen.

use std::collections::VecDeque;

/// Lines kept per tab.
pub(crate) const MAX_LINES: usize = 2000;
/// Bytes kept per line; the rest of a longer line is dropped.
const MAX_LINE: usize = 4096;

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Esc,
    EscInter,
    Csi,
    Str,
    StrEsc,
}

#[derive(Default)]
pub(crate) struct Tail {
    lines: VecDeque<String>,
    cur: Vec<u8>,
    state: State,
    cr: bool,
    /// Completed lines ever (index of the current line).
    total: u64,
    /// Line index where the last agent `send` started (`wait` searches from here).
    mark: u64,
}

impl Tail {
    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.byte(b);
        }
    }

    fn byte(&mut self, b: u8) {
        self.state = match self.state {
            State::Ground => {
                if self.cr && b != b'\n' {
                    self.cur.clear();
                }
                self.cr = false;
                match b {
                    0x1b => return self.state = State::Esc,
                    b'\n' => self.newline(),
                    b'\r' => self.cr = true,
                    0x08 => {
                        let _ = self.cur.pop();
                    }
                    b'\t' => self.push(b),
                    b if b < 0x20 || b == 0x7f => {}
                    b => self.push(b),
                }
                State::Ground
            }
            State::Esc => match b {
                b'[' => State::Csi,
                b']' | b'P' | b'X' | b'^' | b'_' => State::Str,
                0x20..=0x2f => State::EscInter,
                _ => State::Ground,
            },
            State::EscInter if (0x30..=0x7e).contains(&b) => State::Ground,
            State::Csi if (0x40..=0x7e).contains(&b) => State::Ground,
            State::Str if b == 0x07 => State::Ground,
            State::Str if b == 0x1b => State::StrEsc,
            State::StrEsc if b == b'\\' => State::Ground,
            State::StrEsc => State::Str,
            s => s,
        };
    }

    fn push(&mut self, b: u8) {
        if self.cur.len() < MAX_LINE {
            self.cur.push(b);
        }
    }

    fn newline(&mut self) {
        let line = String::from_utf8_lossy(&self.cur).into_owned();
        self.cur.clear();
        self.lines.push_back(line);
        self.total += 1;
        if self.lines.len() > MAX_LINES {
            self.lines.pop_front();
        }
    }

    /// Remember where new output starts (called before an agent sends input).
    pub fn mark(&mut self) {
        self.mark = self.total;
    }

    /// Last `n` lines, including the unfinished current line.
    pub fn last(&self, n: usize) -> String {
        let partial = usize::from(!self.cur.is_empty());
        let skip = (self.lines.len() + partial).saturating_sub(n);
        self.text(skip)
    }

    /// Text from the line where the last agent `send` started.
    pub fn since_mark(&self) -> String {
        let first = self.total - self.lines.len() as u64;
        self.text(usize::try_from(self.mark.saturating_sub(first)).unwrap_or(usize::MAX))
    }

    fn text(&self, skip: usize) -> String {
        let mut parts: Vec<&str> = self.lines.iter().skip(skip).map(String::as_str).collect();
        let cur = String::from_utf8_lossy(&self.cur);
        if !cur.is_empty() && skip <= self.lines.len() {
            parts.push(&cur);
        }
        parts.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_escapes_and_handles_cr_and_backspace() {
        let mut t = Tail::default();
        t.feed(b"\x1b[1;32mgreen\x1b[0m ok\r\n\x1b]0;title\x07pro\x1b(Bgress 10%\rprogress 100%\r\nab\x08c\n$ ");
        assert_eq!(t.last(10), "green ok\nprogress 100%\nac\n$ ");
        assert_eq!(t.last(1), "$ ");
        assert_eq!(t.last(2), "ac\n$ ");
    }

    #[test]
    fn bounded_and_marked() {
        let mut t = Tail::default();
        for i in 0..MAX_LINES + 10 {
            t.feed(format!("line {i}\n").as_bytes());
        }
        assert_eq!(t.lines.len(), MAX_LINES);
        assert!(t.last(1).ends_with(&format!("line {}", MAX_LINES + 9)));
        t.mark();
        t.feed(b"uptime\r\n load average: 0.1\r\n$ ");
        assert_eq!(t.since_mark(), "uptime\n load average: 0.1\n$ ");
        t.feed(&[b'x'; MAX_LINE * 2]);
        assert!(t.last(1).len() <= MAX_LINE + 2);
    }
}
