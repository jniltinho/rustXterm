//! OSC 7 (`ESC ] 7 ; file://host/path ST`) scanner run over raw output before the parser.
//!
//! Alacritty's event API has no current-directory event, so the bytes are observed here
//! (they still reach the parser, which ignores OSC 7). Sequences may be split across
//! `scan` calls; payloads longer than [`MAX_PAYLOAD`] bytes are dropped.

/// Maximum OSC 7 payload kept per sequence.
pub const MAX_PAYLOAD: usize = 4096;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Esc,
    /// Saw `ESC ]`.
    Osc,
    /// Saw `ESC ] 7`.
    Osc7,
    /// Collecting the payload after `ESC ] 7 ;`.
    Payload,
    /// Payload too long: discard until the terminator.
    Overflow,
    /// Saw `ESC` inside a payload (possible `ESC \` terminator).
    PayloadEsc {
        overflow: bool,
    },
}

#[derive(Debug, Default)]
pub struct Osc7Scanner {
    state: State,
    buf: Vec<u8>,
}

impl Osc7Scanner {
    /// Scan `bytes`, calling `on_cwd` with each decoded path.
    pub fn scan(&mut self, bytes: &[u8], mut on_cwd: impl FnMut(String)) {
        for &b in bytes {
            self.state = match (self.state, b) {
                (State::Ground, ESC) => State::Esc,
                (State::Ground, _) => State::Ground,
                (State::Esc, b']') => State::Osc,
                (State::Esc | State::Osc | State::Osc7, ESC) => State::Esc,
                (State::Osc, b'7') => State::Osc7,
                (State::Osc7, b';') => {
                    self.buf.clear();
                    State::Payload
                }
                (State::Esc | State::Osc | State::Osc7, _) => State::Ground,
                (State::Payload | State::Overflow, BEL) => {
                    if self.state == State::Payload {
                        self.finish(&mut on_cwd);
                    }
                    State::Ground
                }
                (State::Payload | State::Overflow, ESC) => {
                    State::PayloadEsc { overflow: self.state == State::Overflow }
                }
                (State::Payload | State::Overflow, CAN | SUB) => State::Ground,
                (State::Payload, _) if self.buf.len() >= MAX_PAYLOAD => {
                    self.buf.clear();
                    State::Overflow
                }
                (State::Payload, _) => {
                    self.buf.push(b);
                    State::Payload
                }
                (State::Overflow, _) => State::Overflow,
                (State::PayloadEsc { overflow }, b'\\') => {
                    if !overflow {
                        self.finish(&mut on_cwd);
                    }
                    State::Ground
                }
                // Any other ESC sequence aborts the OSC string; it may start a new one.
                (State::PayloadEsc { .. }, b']') => State::Osc,
                (State::PayloadEsc { .. }, ESC) => State::Esc,
                (State::PayloadEsc { .. }, _) => State::Ground,
            };
        }
    }

    fn finish(&mut self, on_cwd: &mut impl FnMut(String)) {
        if let Some(path) = parse_file_url(&self.buf) {
            on_cwd(path);
        }
        self.buf.clear();
    }
}

/// `file://host/path` -> percent-decoded `/path`.
fn parse_file_url(payload: &[u8]) -> Option<String> {
    let rest = payload.strip_prefix(b"file://")?;
    let path = &rest[rest.iter().position(|&b| b == b'/')?..];
    let mut out = Vec::with_capacity(path.len());
    let mut i = 0;
    while i < path.len() {
        if path[i] == b'%'
            && let Some(v) = path.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok())
            && let Ok(v) = u8::from_str_radix(v, 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(path[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_chunks(chunks: &[&[u8]]) -> Vec<String> {
        let mut s = Osc7Scanner::default();
        let mut out = Vec::new();
        for c in chunks {
            s.scan(c, |p| out.push(p));
        }
        out
    }

    #[test]
    fn bel_and_st_terminators() {
        assert_eq!(scan_chunks(&[b"a\x1b]7;file://lab/home/user\x07b"]), ["/home/user"]);
        assert_eq!(scan_chunks(&[b"\x1b]7;file://lab/tmp\x1b\\"]), ["/tmp"]);
    }

    #[test]
    fn percent_decoding_and_empty_host() {
        assert_eq!(scan_chunks(&[b"\x1b]7;file:///srv/my%20dir/%C3%A9t%C3%A9\x07"]), ["/srv/my dir/\u{e9}t\u{e9}"]);
    }

    #[test]
    fn split_across_every_byte() {
        let seq = b"xx\x1b]7;file://lab/var/log\x1b\\yy";
        let chunks: Vec<&[u8]> = seq.chunks(1).collect();
        assert_eq!(scan_chunks(&chunks), ["/var/log"]);
    }

    #[test]
    fn oversized_is_dropped_and_scanner_recovers() {
        let mut big = b"\x1b]7;file://lab/".to_vec();
        big.extend(std::iter::repeat_n(b'a', MAX_PAYLOAD + 10));
        big.push(BEL);
        let (first, second) = big.split_at(2000);
        assert!(scan_chunks(&[first, second]).is_empty());
        assert_eq!(scan_chunks(&[&big, b"\x1b]7;file://lab/ok\x07"]), ["/ok"]);
        // Exactly at the limit is kept.
        let mut edge = b"\x1b]7;".to_vec();
        let url = format!("file://h/{}", "b".repeat(MAX_PAYLOAD - 9));
        edge.extend(url.as_bytes());
        edge.push(BEL);
        assert_eq!(scan_chunks(&[&edge])[0].len(), MAX_PAYLOAD - 8);
    }

    #[test]
    fn ignores_other_osc_and_bad_urls() {
        assert!(scan_chunks(&[b"\x1b]0;title\x07\x1b]77;file://h/x\x07\x1b]7;http://h/x\x07"]).is_empty());
        // Aborted by CAN, then a new sequence after an interrupting ESC ].
        assert_eq!(scan_chunks(&[b"\x1b]7;file://h/a\x18\x1b]7;file://h/b\x1b]7;file://h/c\x07"]), ["/c"]);
    }
}
