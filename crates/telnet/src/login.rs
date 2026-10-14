//! Opt-in auto-login (design T5): answer the login prompt, then the password prompt, at most once
//! each and only within [`WINDOW`] of connecting. Prompts are matched case-insensitively at the end
//! of the ANSI-stripped last [`TAIL`] bytes of server output.

use std::time::{Duration, Instant};

use secrecy::zeroize::Zeroizing;
use secrecy::{ExposeSecret, SecretString};

pub const WINDOW: Duration = Duration::from_secs(15);
const TAIL: usize = 256;
/// Alternative accepted while the login prompt is the default `login:`.
const DEFAULT_LOGIN: &str = "login:";
const ALT_LOGIN: &str = "username:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Login,
    Password,
    Done,
}

#[derive(Debug, Clone, Copy)]
enum Ansi {
    Text,
    Esc,
    Csi,
    Osc,
    OscEsc,
}

pub struct AutoLogin {
    user: String,
    password: SecretString,
    login_prompts: Vec<Vec<u8>>,
    password_prompt: Vec<u8>,
    stage: Stage,
    tail: Vec<u8>,
    ansi: Ansi,
    deadline: Instant,
}

fn prompt(s: &str, default: &str) -> Vec<u8> {
    let s = s.trim();
    if s.is_empty() { default } else { s }.to_ascii_lowercase().into_bytes()
}

impl AutoLogin {
    pub fn new(user: &str, password: SecretString, login_prompt: &str, password_prompt: &str, start: Instant) -> Self {
        let login = prompt(login_prompt, DEFAULT_LOGIN);
        let mut login_prompts = vec![login];
        if login_prompts[0] == DEFAULT_LOGIN.as_bytes() {
            login_prompts.push(ALT_LOGIN.as_bytes().to_vec());
        }
        Self {
            user: user.to_owned(),
            password,
            login_prompts,
            password_prompt: prompt(password_prompt, "password:"),
            stage: Stage::Login,
            tail: Vec::with_capacity(TAIL * 2),
            ansi: Ansi::Text,
            deadline: start + WINDOW,
        }
    }

    /// Feed server output; returns the text to type (with a trailing Enter `\r`), if any.
    pub fn on_output(&mut self, data: &[u8], now: Instant) -> Option<Zeroizing<Vec<u8>>> {
        if self.stage == Stage::Done {
            return None;
        }
        if now > self.deadline {
            self.stage = Stage::Done;
            return None;
        }
        self.strip(data);
        let end = self.tail.trim_ascii_end();
        let answer = match self.stage {
            Stage::Login if self.login_prompts.iter().any(|p| end.ends_with(p)) => {
                self.stage = Stage::Password;
                Some(self.user.as_bytes())
            }
            // Out of order: never type the password without having typed the user.
            Stage::Login if end.ends_with(&self.password_prompt) => {
                self.stage = Stage::Done;
                None
            }
            Stage::Password if end.ends_with(&self.password_prompt) => {
                self.stage = Stage::Done;
                Some(self.password.expose_secret().as_bytes())
            }
            _ => None,
        }?;
        let mut line = Zeroizing::new(Vec::with_capacity(answer.len() + 1));
        line.extend_from_slice(answer);
        line.push(b'\r');
        self.tail.clear();
        Some(line)
    }

    fn strip(&mut self, data: &[u8]) {
        for &b in data {
            self.ansi = match (self.ansi, b) {
                (Ansi::Text, 0x1b) => Ansi::Esc,
                (Ansi::Text, b'\r' | b'\n') => {
                    self.tail.push(b'\n');
                    Ansi::Text
                }
                (Ansi::Text, b) if b < 0x20 || b == 0x7f => Ansi::Text,
                (Ansi::Text, b) => {
                    self.tail.push(b.to_ascii_lowercase());
                    Ansi::Text
                }
                (Ansi::Esc, b'[') => Ansi::Csi,
                (Ansi::Esc, b']') => Ansi::Osc,
                (Ansi::Esc, _) => Ansi::Text,
                (Ansi::Csi, 0x40..=0x7e) => Ansi::Text,
                (Ansi::Csi, _) => Ansi::Csi,
                (Ansi::Osc, 0x07) => Ansi::Text,
                (Ansi::Osc, 0x1b) => Ansi::OscEsc,
                (Ansi::Osc, _) => Ansi::Osc,
                (Ansi::OscEsc, b'\\') => Ansi::Text,
                (Ansi::OscEsc, _) => Ansi::Osc,
            };
        }
        if self.tail.len() > TAIL {
            self.tail.drain(..self.tail.len() - TAIL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login(t0: Instant) -> AutoLogin {
        AutoLogin::new("alice", SecretString::from("s3cret"), "login:", "password:", t0)
    }

    fn typed(a: Option<Zeroizing<Vec<u8>>>) -> Option<Vec<u8>> {
        a.map(|z| z.to_vec())
    }

    #[test]
    fn ordered_prompts_answered_once() {
        let t0 = Instant::now();
        let mut l = login(t0);
        assert_eq!(typed(l.on_output(b"Debian GNU/Linux\r\nLast login: Mon\r\n", t0)), None);
        assert_eq!(typed(l.on_output(b"\x1b[1mhost\x1b[0m LOGIN: ", t0)), Some(b"alice\r".to_vec()));
        assert_eq!(typed(l.on_output(b"login: ", t0)), None, "user typed once");
        assert_eq!(typed(l.on_output(b"alice\r\nPass", t0)), None);
        assert_eq!(typed(l.on_output(b"word: ", t0)), Some(b"s3cret\r".to_vec()), "split prompt");
        assert_eq!(typed(l.on_output(b"Password: ", t0)), None, "password typed once");
        assert_eq!(typed(l.on_output(b"login: ", t0)), None);
    }

    #[test]
    fn username_prompt_and_osc_title() {
        let t0 = Instant::now();
        let mut l = login(t0);
        assert_eq!(typed(l.on_output(b"\x1b]0;router\x07Username:", t0)), Some(b"alice\r".to_vec()));
    }

    #[test]
    fn custom_prompts() {
        let t0 = Instant::now();
        let mut l = AutoLogin::new("bob", SecretString::from("pw"), "User Name>", "Secret>", t0);
        assert_eq!(typed(l.on_output(b"login: ", t0)), None);
        assert_eq!(typed(l.on_output(b"user name> ", t0)), Some(b"bob\r".to_vec()));
        assert_eq!(typed(l.on_output(b"SECRET>", t0)), Some(b"pw\r".to_vec()));
    }

    #[test]
    fn out_of_order_password_prompt_types_nothing() {
        let t0 = Instant::now();
        let mut l = login(t0);
        assert_eq!(typed(l.on_output(b"Password: ", t0)), None);
        assert_eq!(typed(l.on_output(b"login: ", t0)), None);
    }

    #[test]
    fn nothing_after_window() {
        let t0 = Instant::now();
        let mut l = login(t0);
        assert_eq!(typed(l.on_output(b"booting...\r\n", t0 + Duration::from_secs(5))), None);
        assert_eq!(typed(l.on_output(b"login: ", t0 + WINDOW + Duration::from_millis(1))), None);
        assert_eq!(typed(l.on_output(b"login: ", t0)), None, "stays done");
    }

    #[test]
    fn tail_is_bounded() {
        let t0 = Instant::now();
        let mut l = login(t0);
        for _ in 0..100 {
            l.on_output(&[b'x'; 1000], t0);
        }
        assert!(l.tail.len() <= TAIL);
    }
}
