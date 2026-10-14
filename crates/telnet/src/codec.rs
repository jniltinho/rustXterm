//! Sans-IO Telnet codec (design T1–T3): RFC 854/855 stream parsing and RFC 1143 Q-method option
//! negotiation. No IO; the driver feeds socket bytes in and writes the replies out.

use rxt_core::Newline;

pub const IAC: u8 = 255;
pub const DONT: u8 = 254;
pub const DO: u8 = 253;
pub const WONT: u8 = 252;
pub const WILL: u8 = 251;
pub const SB: u8 = 250;
pub const SE: u8 = 240;

pub const BINARY: u8 = 0;
pub const ECHO: u8 = 1;
pub const SGA: u8 = 3;
pub const TTYPE: u8 = 24;
pub const NAWS: u8 = 31;
pub const LINEMODE: u8 = 34;

const TTYPE_IS: u8 = 0;
const TTYPE_SEND: u8 = 1;
/// Longer subnegotiations are discarded up to `SE` without growing memory.
pub const SB_CAP: usize = 1024;
/// Negotiation commands accepted per option and session; later ones are ignored silently.
pub const TOGGLE_LIMIT: u8 = 32;

/// RFC 1143 option state. The client never disables an option on its own, so `WANTNO` and the
/// queue bits are never entered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Q {
    No,
    Yes,
    WantYes,
}

#[derive(Debug, Clone, Copy)]
enum State {
    Data,
    Iac,
    Verb(u8),
    Sb,
    SbIac,
}

/// Result of [`Codec::feed`]: terminal data and negotiation bytes to send back to the server.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Output {
    pub data: Vec<u8>,
    pub replies: Vec<u8>,
}

/// Telnet client state machine.
#[derive(Debug)]
pub struct Codec {
    us: [Q; 256],
    him: [Q; 256],
    toggles: [u8; 256],
    state: State,
    sb: Vec<u8>,
    sb_overflow: bool,
    cr: bool,
    terminal_type: Vec<u8>,
    newline: Newline,
    cols: u16,
    rows: u16,
}

fn accept_local(opt: u8) -> bool {
    matches!(opt, BINARY | SGA | TTYPE | NAWS)
}

fn accept_remote(opt: u8) -> bool {
    matches!(opt, BINARY | ECHO | SGA)
}

impl Codec {
    pub fn new(terminal_type: &str, newline: Newline, cols: u16, rows: u16) -> Self {
        Self {
            us: [Q::No; 256],
            him: [Q::No; 256],
            toggles: [0; 256],
            state: State::Data,
            sb: Vec::with_capacity(SB_CAP),
            sb_overflow: false,
            cr: false,
            terminal_type: terminal_type.as_bytes().to_vec(),
            newline,
            cols,
            rows,
        }
    }

    /// Proactive offers sent right after connecting: `WILL TTYPE`, `WILL NAWS`, `DO SGA`, `DO ECHO`.
    pub fn start(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        for (verb, opt) in [(WILL, TTYPE), (WILL, NAWS), (DO, SGA), (DO, ECHO)] {
            if verb == WILL {
                self.us[opt as usize] = Q::WantYes;
            } else {
                self.him[opt as usize] = Q::WantYes;
            }
            out.extend([IAC, verb, opt]);
        }
        out
    }

    /// Parse server bytes. State carries over between calls, so chunk boundaries do not matter.
    pub fn feed(&mut self, input: &[u8]) -> Output {
        let mut out = Output::default();
        for &b in input {
            self.state = match self.state {
                State::Data if b == IAC => State::Iac,
                State::Data => {
                    self.data(b, &mut out.data);
                    State::Data
                }
                State::Iac => self.after_iac(b, &mut out.data),
                State::Verb(verb) => {
                    self.negotiate(verb, b, &mut out.replies);
                    State::Data
                }
                State::Sb if b == IAC => State::SbIac,
                State::Sb => {
                    self.sb_push(b);
                    State::Sb
                }
                State::SbIac => match b {
                    IAC => {
                        self.sb_push(IAC);
                        State::Sb
                    }
                    SE => {
                        self.subnegotiation(&mut out.replies);
                        State::Data
                    }
                    // Malformed: abandon the subnegotiation and treat `b` as a command.
                    _ => self.after_iac(b, &mut out.data),
                },
            };
        }
        out
    }

    /// Encode user input: IAC doubled; outside local BINARY, Enter (`CR`, or `CR LF` in one write)
    /// becomes the configured newline.
    pub fn encode_input(&self, input: &[u8]) -> Vec<u8> {
        let binary = self.us[BINARY as usize] == Q::Yes;
        let mut out = Vec::with_capacity(input.len() + 4);
        let mut prev_cr = false;
        for &b in input {
            match b {
                IAC => out.extend([IAC, IAC]),
                b'\r' if !binary => out.extend_from_slice(self.newline.bytes()),
                b'\n' if !binary && prev_cr => {}
                _ => out.push(b),
            }
            prev_cr = b == b'\r';
        }
        out
    }

    /// Record a new window size; returns the NAWS subnegotiation when NAWS is active.
    pub fn set_size(&mut self, cols: u16, rows: u16) -> Option<Vec<u8>> {
        self.cols = cols;
        self.rows = rows;
        (self.us[NAWS as usize] == Q::Yes).then(|| self.naws())
    }

    fn naws(&self) -> Vec<u8> {
        let mut out = vec![IAC, SB, NAWS];
        for b in self.cols.to_be_bytes().into_iter().chain(self.rows.to_be_bytes()) {
            out.push(b);
            if b == IAC {
                out.push(IAC);
            }
        }
        out.extend([IAC, SE]);
        out
    }

    fn after_iac(&mut self, b: u8, data: &mut Vec<u8>) -> State {
        match b {
            IAC => {
                self.data(IAC, data);
                State::Data
            }
            WILL | WONT | DO | DONT => State::Verb(b),
            SB => {
                self.sb.clear();
                self.sb_overflow = false;
                State::Sb
            }
            // NOP, DM, BRK, IP, AO, AYT, EC, EL, GA and stray SE are consumed.
            _ => State::Data,
        }
    }

    fn data(&mut self, b: u8, data: &mut Vec<u8>) {
        if self.him[BINARY as usize] == Q::Yes {
            data.push(b);
            return;
        }
        // NVT: CR NUL is a bare CR.
        if !(self.cr && b == 0) {
            data.push(b);
        }
        self.cr = b == b'\r';
    }

    fn sb_push(&mut self, b: u8) {
        if self.sb.len() < SB_CAP {
            self.sb.push(b);
        } else {
            self.sb_overflow = true;
        }
    }

    /// Counts negotiation traffic per option; false once the peer exceeded the limit.
    fn allow(&mut self, opt: u8) -> bool {
        let n = &mut self.toggles[opt as usize];
        if *n >= TOGGLE_LIMIT {
            return false;
        }
        *n += 1;
        true
    }

    fn subnegotiation(&mut self, replies: &mut Vec<u8>) {
        let send = !self.sb_overflow
            && self.sb.as_slice() == [TTYPE, TTYPE_SEND]
            && self.us[TTYPE as usize] == Q::Yes
            && self.allow(TTYPE);
        self.sb.clear();
        if send {
            replies.extend([IAC, SB, TTYPE, TTYPE_IS]);
            for &b in &self.terminal_type {
                replies.push(b);
                if b == IAC {
                    replies.push(IAC);
                }
            }
            replies.extend([IAC, SE]);
        }
    }

    fn negotiate(&mut self, verb: u8, opt: u8, r: &mut Vec<u8>) {
        if !self.allow(opt) {
            return;
        }
        let i = opt as usize;
        match verb {
            WILL => match self.him[i] {
                Q::No if accept_remote(opt) => {
                    self.him[i] = Q::Yes;
                    r.extend([IAC, DO, opt]);
                }
                Q::No => r.extend([IAC, DONT, opt]),
                Q::WantYes => self.him[i] = Q::Yes,
                Q::Yes => {}
            },
            WONT => match self.him[i] {
                Q::Yes => {
                    self.him[i] = Q::No;
                    r.extend([IAC, DONT, opt]);
                }
                Q::WantYes => self.him[i] = Q::No,
                Q::No => {}
            },
            DO => match self.us[i] {
                Q::No if accept_local(opt) => {
                    self.us[i] = Q::Yes;
                    r.extend([IAC, WILL, opt]);
                    self.enabled(opt, r);
                }
                Q::No => r.extend([IAC, WONT, opt]),
                Q::WantYes => {
                    self.us[i] = Q::Yes;
                    self.enabled(opt, r);
                }
                Q::Yes => {}
            },
            _ => match self.us[i] {
                Q::Yes => {
                    self.us[i] = Q::No;
                    r.extend([IAC, WONT, opt]);
                }
                Q::WantYes => self.us[i] = Q::No,
                Q::No => {}
            },
        }
    }

    fn enabled(&self, opt: u8, r: &mut Vec<u8>) {
        if opt == NAWS {
            r.extend(self.naws());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codec() -> Codec {
        Codec::new("xterm-256color", Newline::CrLf, 80, 24)
    }

    /// Deterministic xorshift for fuzz-ish property tests (no extra dependencies).
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn bytes(&mut self, n: usize) -> Vec<u8> {
            // Bias towards Telnet command bytes so the parser states are all exercised.
            (0..n)
                .map(|_| match self.next() % 4 {
                    0 => 240 + (self.next() % 16) as u8,
                    1 => [0, 1, 3, 24, 31, 34, b'\r'][(self.next() % 7) as usize],
                    _ => self.next() as u8,
                })
                .collect()
        }
    }

    #[test]
    fn start_offers_ttype_naws_sga_echo() {
        assert_eq!(codec().start(), [IAC, WILL, TTYPE, IAC, WILL, NAWS, IAC, DO, SGA, IAC, DO, ECHO]);
    }

    #[test]
    fn answers_to_own_offers_are_silent_except_naws_size() {
        let mut c = codec();
        c.start();
        let out = c.feed(&[IAC, DO, TTYPE, IAC, WILL, ECHO, IAC, WILL, SGA, IAC, DO, NAWS]);
        assert_eq!(out.replies, [IAC, SB, NAWS, 0, 80, 0, 24, IAC, SE]);
        assert!(out.data.is_empty());
    }

    #[test]
    fn option_table() {
        let mut c = codec();
        let cases: &[(u8, u8, [u8; 3])] = &[
            (WILL, BINARY, [IAC, DO, BINARY]),
            (DO, BINARY, [IAC, WILL, BINARY]),
            (WILL, ECHO, [IAC, DO, ECHO]),
            (DO, ECHO, [IAC, WONT, ECHO]),
            (WILL, SGA, [IAC, DO, SGA]),
            (DO, SGA, [IAC, WILL, SGA]),
            (DO, TTYPE, [IAC, WILL, TTYPE]),
            (WILL, TTYPE, [IAC, DONT, TTYPE]),
            (DO, LINEMODE, [IAC, WONT, LINEMODE]),
            (WILL, LINEMODE, [IAC, DONT, LINEMODE]),
            (DO, 39, [IAC, WONT, 39]),
            (WILL, 39, [IAC, DONT, 39]),
        ];
        for &(verb, opt, want) in cases {
            assert_eq!(c.feed(&[IAC, verb, opt]).replies, want, "verb {verb} opt {opt}");
        }
        // Already enabled: no reply (no loop).
        assert!(c.feed(&[IAC, WILL, BINARY, IAC, DO, SGA]).replies.is_empty());
        // Disable acknowledged once.
        assert_eq!(c.feed(&[IAC, WONT, ECHO]).replies, [IAC, DONT, ECHO]);
        assert!(c.feed(&[IAC, WONT, ECHO]).replies.is_empty());
        assert_eq!(c.feed(&[IAC, DONT, SGA]).replies, [IAC, WONT, SGA]);
    }

    #[test]
    fn ttype_send_answers_is() {
        let mut c = Codec::new("vt100", Newline::CrLf, 80, 24);
        // Before TTYPE is enabled, SEND is ignored.
        assert!(c.feed(&[IAC, SB, TTYPE, TTYPE_SEND, IAC, SE]).replies.is_empty());
        c.feed(&[IAC, DO, TTYPE]);
        let out = c.feed(&[IAC, SB, TTYPE, TTYPE_SEND, IAC, SE]);
        assert_eq!(out.replies, [&[IAC, SB, TTYPE, TTYPE_IS][..], b"vt100", &[IAC, SE]].concat());
    }

    #[test]
    fn naws_on_accept_and_resize_with_ff_escaping() {
        let mut c = codec();
        assert_eq!(c.set_size(100, 30), None, "NAWS not negotiated yet");
        assert_eq!(c.feed(&[IAC, DO, NAWS]).replies, [IAC, WILL, NAWS, IAC, SB, NAWS, 0, 100, 0, 30, IAC, SE]);
        assert_eq!(c.set_size(255, 0xFF01), Some(vec![IAC, SB, NAWS, 0, 255, 255, 255, 255, 1, IAC, SE]));
    }

    #[test]
    fn iac_iac_and_cr_nul_in_data() {
        let mut c = codec();
        assert_eq!(c.feed(&[b'a', IAC, IAC, b'b']).data, [b'a', 0xFF, b'b']);
        assert_eq!(c.feed(b"x\r\0y\r\n").data, b"x\ry\r\n");
        // Split across feeds.
        assert_eq!(c.feed(b"z\r").data, b"z\r");
        assert_eq!(c.feed(b"\0w").data, b"w");
        // With server BINARY, NUL after CR is data.
        c.feed(&[IAC, WILL, BINARY]);
        assert_eq!(c.feed(b"\r\0").data, b"\r\0");
    }

    #[test]
    fn commands_are_consumed() {
        let mut c = codec();
        // NOP, GA, AYT, DM, stray SE.
        assert_eq!(c.feed(&[b'a', IAC, 241, IAC, 249, IAC, 246, IAC, 242, IAC, SE, b'b']).data, b"ab");
    }

    #[test]
    fn oversized_subnegotiation_is_discarded_bounded() {
        let mut c = codec();
        c.feed(&[IAC, DO, TTYPE]);
        let mut input = vec![IAC, SB, TTYPE, TTYPE_SEND];
        input.resize(4 + (1 << 20), b'A');
        input.extend([IAC, SE]);
        input.extend(b"after");
        let out = c.feed(&input);
        assert_eq!(out.data, b"after");
        assert!(out.replies.is_empty());
        assert!(c.sb.capacity() <= SB_CAP);
    }

    #[test]
    fn malformed_subnegotiation_resyncs() {
        let mut c = codec();
        assert_eq!(
            c.feed(&[IAC, SB, TTYPE, 1, IAC, WILL, SGA, b'x']),
            Output { data: b"x".to_vec(), replies: vec![IAC, DO, SGA] }
        );
    }

    #[test]
    fn toggle_loop_is_cut_off() {
        let mut c = codec();
        let mut replies = 0;
        for _ in 0..1000 {
            replies += c.feed(&[IAC, WILL, BINARY, IAC, WONT, BINARY]).replies.len() / 3;
        }
        assert!(replies <= TOGGLE_LIMIT as usize, "{replies} replies");
        // Other options and data are unaffected.
        assert_eq!(c.feed(&[IAC, WILL, SGA, b'k']), Output { data: b"k".to_vec(), replies: vec![IAC, DO, SGA] });
        let mut sends = 0;
        c.feed(&[IAC, DO, TTYPE]);
        for _ in 0..1000 {
            sends += usize::from(!c.feed(&[IAC, SB, TTYPE, TTYPE_SEND, IAC, SE]).replies.is_empty());
        }
        assert!(sends < TOGGLE_LIMIT as usize);
    }

    #[test]
    fn encode_input_newline_and_iac() {
        for (nl, want) in [(Newline::CrLf, &b"ls\r\n"[..]), (Newline::CrNul, b"ls\r\0"), (Newline::Cr, b"ls\r")] {
            let c = Codec::new("xterm", nl, 80, 24);
            assert_eq!(c.encode_input(b"ls\r"), want);
            assert_eq!(c.encode_input(b"ls\r\n"), want, "pasted CR LF is one Enter");
        }
        assert_eq!(codec().encode_input(&[1, IAC, 2]), [1, IAC, IAC, 2]);
        let mut c = codec();
        c.feed(&[IAC, DO, BINARY]);
        assert_eq!(c.encode_input(b"\r\n"), b"\r\n", "binary passes CR unchanged");
    }

    #[test]
    fn random_streams_never_panic_and_chunking_is_irrelevant() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..300 {
            let stream = rng.bytes(2000);
            let mut whole = codec();
            whole.start();
            let a = whole.feed(&stream);
            let mut parts = codec();
            parts.start();
            let mut b = Output::default();
            let mut rest = &stream[..];
            while !rest.is_empty() {
                let n = (rng.next() % 17) as usize + 1;
                let (head, tail) = rest.split_at(n.min(rest.len()));
                let o = parts.feed(head);
                b.data.extend(o.data);
                b.replies.extend(o.replies);
                rest = tail;
            }
            assert_eq!(a, b);
            assert!(a.data.len() <= stream.len());
            assert!(whole.sb.capacity() <= SB_CAP);
        }
    }

    #[test]
    fn iac_round_trip_over_binary() {
        let mut rng = Rng(42);
        let mut enc = codec();
        enc.feed(&[IAC, DO, BINARY]);
        let mut dec = codec();
        dec.feed(&[IAC, WILL, BINARY]);
        for _ in 0..200 {
            let n = (rng.next() % 512) as usize;
            let data = rng.bytes(n);
            let out = dec.feed(&enc.encode_input(&data));
            assert_eq!(out.data, data);
            assert!(out.replies.is_empty());
        }
    }
}
