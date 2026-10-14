//! Telnet and raw TCP client (design T1–T5): a sans-IO [`codec::Codec`] driven over a tokio
//! `TcpStream`. The engine pulls output with [`TelnetSession::read`] (the socket is read only when
//! called, so the engine's output buffer provides backpressure) and writes through a cloneable
//! [`TelnetWriter`]. Telnet is plaintext; passwords are only typed by opt-in auto-login and never
//! logged.

pub mod codec;
mod login;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use rxt_core::{Error, Prompter, Result, Session};
use secrecy::SecretString;
use secrecy::zeroize::Zeroizing;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::codec::Codec;
use crate::login::AutoLogin;

pub use crate::login::WINDOW as AUTO_LOGIN_WINDOW;

const DEFAULT_TERMINAL_TYPE: &str = "xterm-256color";
const READ_CHUNK: usize = 16 * 1024;

/// Connection context supplied by the engine.
pub struct Context {
    pub prompter: Arc<dyn Prompter>,
    /// Saved password from the encrypted store; used only when the session enables auto-login.
    pub password: Option<SecretString>,
    pub cancel: CancellationToken,
}

type Outbox = mpsc::UnboundedSender<Zeroizing<Vec<u8>>>;

struct Shared {
    /// `None` in raw TCP mode.
    codec: Option<Mutex<Codec>>,
    tx: Outbox,
}

impl Shared {
    fn send(&self, bytes: Vec<u8>) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.tx.send(Zeroizing::new(bytes)).map_err(|_| Error::Connection("telnet connection closed".into()))
    }

    fn encode(&self, input: &[u8]) -> Vec<u8> {
        match &self.codec {
            Some(c) => c.lock().unwrap_or_else(PoisonError::into_inner).encode_input(input),
            None => input.to_vec(),
        }
    }
}

/// Pause between seeing an auto-login prompt and typing the answer.
const LOGIN_TYPE_DELAY: std::time::Duration = std::time::Duration::from_millis(300);

/// An open Telnet or raw TCP connection.
pub struct TelnetSession {
    rd: OwnedReadHalf,
    buf: Box<[u8]>,
    shared: Arc<Shared>,
    login: Option<AutoLogin>,
    cancel: CancellationToken,
    _stop_on_drop: DropGuard,
}

/// Cloneable input side; every write is ordered through one socket writer task.
#[derive(Clone)]
pub struct TelnetWriter {
    shared: Arc<Shared>,
}

/// Open a Telnet session (or a raw TCP one when `session.telnet.raw`).
pub async fn connect(session: &Session, ctx: Context, cols: u16, rows: u16) -> Result<TelnetSession> {
    let opts = &session.telnet;
    let (host, port) = (session.host.as_str(), session.port);
    let stream = tokio::select! {
        () = ctx.cancel.cancelled() => return Err(Error::Cancelled),
        r = TcpStream::connect((host, port)) => r.map_err(|e| Error::Connection(format!("{host}:{port}: {e}")))?,
    };
    // Interactive traffic: keystrokes must not wait for Nagle.
    let _ = stream.set_nodelay(true);
    let (rd, wr) = stream.into_split();
    let cancel = ctx.cancel.child_token();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(pump(wr, rx, cancel.clone()));

    let codec = (!opts.raw).then(|| {
        let tt = opts.terminal_type.trim();
        let mut c = Codec::new(if tt.is_empty() { DEFAULT_TERMINAL_TYPE } else { tt }, opts.newline, cols, rows);
        let _ = tx.send(Zeroizing::new(c.start()));
        Mutex::new(c)
    });
    let login = match (opts.auto_login, session.user.as_deref(), ctx.password) {
        (true, Some(user), Some(password)) if !user.is_empty() => {
            Some(AutoLogin::new(user, password, &opts.login_prompt, &opts.password_prompt, Instant::now()))
        }
        _ => None,
    };
    tracing::debug!(host, port, raw = opts.raw, auto_login = login.is_some(), "telnet connected");
    Ok(TelnetSession {
        rd,
        buf: vec![0; READ_CHUNK].into_boxed_slice(),
        shared: Arc::new(Shared { codec, tx }),
        login,
        _stop_on_drop: cancel.clone().drop_guard(),
        cancel,
    })
}

/// Writes queued bytes until the channel closes, the socket fails or the session is cancelled.
async fn pump(mut wr: OwnedWriteHalf, mut rx: mpsc::UnboundedReceiver<Zeroizing<Vec<u8>>>, cancel: CancellationToken) {
    tokio::select! {
        () = cancel.cancelled() => {}
        () = async {
            while let Some(bytes) = rx.recv().await {
                if wr.write_all(&bytes).await.is_err() {
                    break;
                }
            }
        } => {}
    }
    let _ = wr.shutdown().await;
}

impl TelnetSession {
    /// Next chunk of terminal output; `None` once the connection is closed or cancelled.
    /// Negotiation replies and auto-login answers are sent as a side effect.
    pub async fn read(&mut self) -> Option<Vec<u8>> {
        loop {
            let n = tokio::select! {
                () = self.cancel.cancelled() => return None,
                r = self.rd.read(&mut self.buf) => match r {
                    Ok(n) if n > 0 => n,
                    _ => {
                        self.cancel.cancel();
                        return None;
                    }
                },
            };
            let input = &self.buf[..n];
            let data = match &self.shared.codec {
                Some(c) => {
                    let out = c.lock().unwrap_or_else(PoisonError::into_inner).feed(input);
                    let _ = self.shared.send(out.replies);
                    out.data
                }
                None => input.to_vec(),
            };
            if let Some(login) = &mut self.login
                && let Some(line) = login.on_output(&data, Instant::now())
            {
                // `login` prints the prompt, then turns echo off and flushes pending input
                // (TCSAFLUSH); typing at once can lose the answer and echo the password.
                let shared = Arc::clone(&self.shared);
                tokio::spawn(async move {
                    tokio::time::sleep(LOGIN_TYPE_DELAY).await;
                    let _ = shared.send(shared.encode(&line));
                });
            }
            if !data.is_empty() {
                return Some(data);
            }
        }
    }

    pub fn writer(&self) -> TelnetWriter {
        TelnetWriter { shared: Arc::clone(&self.shared) }
    }
}

impl TelnetWriter {
    /// Send user input (Enter as `\r`); Telnet escaping and the newline setting are applied here.
    pub async fn write(&self, bytes: &[u8]) -> Result<()> {
        self.shared.send(self.shared.encode(bytes))
    }

    /// New terminal size: a NAWS subnegotiation once the server accepted NAWS; no-op in raw mode.
    pub async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let naws = match &self.shared.codec {
            Some(c) => c.lock().unwrap_or_else(PoisonError::into_inner).set_size(cols, rows),
            None => None,
        };
        naws.map_or(Ok(()), |b| self.shared.send(b))
    }
}
