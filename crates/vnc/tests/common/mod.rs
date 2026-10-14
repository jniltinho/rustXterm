//! Shared helpers for the in-process RFB tests and the lab tests. Passwords are fictional.
#![allow(dead_code)]

use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

use rxt_core::{
    BoxFut, CertTrust, DesktopEvent, HostKeyDecision, PromptAnswer, PromptKind, Prompter, Protocol, Secret, Session,
};
use rxt_vnc::{Context, Desktop};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};
use tokio_util::sync::CancellationToken;

pub const LAB_VNC_PORT: u16 = 5901;
pub const LAB_VNC_PASS: &str = "lab-vnc1";

/// Answers password prompts with a fixed secret (or cancels) and records every prompt.
#[derive(Default)]
pub struct Script {
    pub password: Option<String>,
    pub prompts: Mutex<Vec<PromptKind>>,
}

impl Prompter for Script {
    fn prompt(&self, kind: PromptKind) -> BoxFut<'_, PromptAnswer> {
        self.prompts.lock().unwrap().push(kind.clone());
        let answer = match (&kind, &self.password) {
            (PromptKind::HostKeyUnknown { .. }, _) => PromptAnswer::HostKey(HostKeyDecision::AcceptOnce),
            (_, Some(p)) => PromptAnswer::Secret(Secret(p.clone())),
            (_, None) => PromptAnswer::Cancel,
        };
        Box::pin(async move { answer })
    }
    fn host_key_changed(&self, _: &str, _: &str, _: &str) {}
}

pub struct NoTrust;
impl CertTrust for NoTrust {
    fn get(&self, _: &str, _: u16) -> Option<String> {
        None
    }
    fn save(&self, _: &str, _: u16, _: &str) {}
}

pub fn ctx(script: Arc<Script>) -> Context {
    Context { prompter: script, trust: Arc::new(NoTrust), cancel: CancellationToken::new() }
}

pub fn script(password: Option<&str>) -> Arc<Script> {
    Arc::new(Script { password: password.map(str::to_owned), ..Script::default() })
}

pub fn session(port: u16) -> Session {
    Session::new(Protocol::Vnc, "127.0.0.1", None, Some(port))
}

/// Next event within `secs` seconds; panics on timeout.
pub async fn next(desktop: &mut Desktop, secs: u64) -> Option<rxt_core::Result<DesktopEvent>> {
    tokio::time::timeout(Duration::from_secs(secs), desktop.next()).await.expect("timed out waiting for an event")
}

/// Tee transport: records client→server bytes and counts server→client bytes.
pub struct Recorder<S> {
    pub inner: S,
    pub sent: Arc<Mutex<Vec<u8>>>,
    pub received: Arc<AtomicUsize>,
}

impl<S> Recorder<S> {
    pub fn new(inner: S) -> Self {
        Self { inner, sent: Arc::default(), received: Arc::default() }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Recorder<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let r = Pin::new(&mut self.inner).poll_read(cx, buf);
        self.received.fetch_add(buf.filled().len() - before, Ordering::Relaxed);
        r
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Recorder<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let r = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = r {
            self.sent.lock().unwrap().extend_from_slice(&buf[..n]);
        }
        r
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Client→server bytes of an RFB 3.8 VncAuth handshake before the first normal message.
pub const VNCAUTH_HANDSHAKE: usize = 12 + 1 + 16 + 1;

/// Parse client→server RFB messages: `(offset, type, incremental flag for type 3)`.
pub fn client_messages(bytes: &[u8], start: usize) -> Vec<(usize, u8, u8)> {
    let mut out = Vec::new();
    let mut at = start;
    while at < bytes.len() {
        let ty = bytes[at];
        let len = match ty {
            0 => 20,
            2 if at + 4 <= bytes.len() => 4 + 4 * usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]])),
            3 => 10,
            4 => 8,
            5 => 6,
            6 if at + 8 <= bytes.len() => {
                8 + u32::from_be_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]]) as usize
            }
            2 | 6 => break,
            other => panic!("unknown client message type {other} at {at}"),
        };
        out.push((at, ty, bytes.get(at + 1).copied().unwrap_or(0)));
        at += len;
    }
    out
}

/// In-process RFB server side.
pub struct Fake {
    pub io: DuplexStream,
}

pub enum Auth {
    None,
    /// Expected 16-byte response to the 0..16 challenge.
    Vnc([u8; 16]),
}

impl Fake {
    /// Run the server handshake up to and including ServerInit; returns the auth result.
    pub async fn handshake(&mut self, version: &[u8; 12], auth: Auth, width: u16, height: u16) -> bool {
        let io = &mut self.io;
        io.write_all(version).await.unwrap();
        let mut v = [0; 12];
        io.read_exact(&mut v).await.unwrap();
        let ty = match auth {
            Auth::None => 1,
            Auth::Vnc(_) => 2,
        };
        if version == b"RFB 003.003\n" {
            io.write_u32(ty).await.unwrap();
        } else {
            io.write_all(&[1, ty as u8]).await.unwrap();
            assert_eq!(io.read_u8().await.unwrap(), ty as u8);
        }
        let ok = match auth {
            Auth::None => {
                if version == b"RFB 003.008\n" {
                    io.write_u32(0).await.unwrap();
                }
                true
            }
            Auth::Vnc(expected) => {
                io.write_all(&core::array::from_fn::<u8, 16, _>(|i| i as u8)).await.unwrap();
                let mut resp = [0; 16];
                io.read_exact(&mut resp).await.unwrap();
                let ok = resp == expected;
                io.write_u32(u32::from(!ok)).await.unwrap();
                if !ok {
                    if version == b"RFB 003.008\n" {
                        io.write_u32(11).await.unwrap();
                        io.write_all(b"auth failed").await.unwrap();
                    }
                    return false;
                }
                true
            }
        };
        let _shared = io.read_u8().await.unwrap();
        io.write_u16(width).await.unwrap();
        io.write_u16(height).await.unwrap();
        io.write_all(&[32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]).await.unwrap();
        io.write_u32(4).await.unwrap();
        io.write_all(b"fake").await.unwrap();
        ok
    }

    /// Record everything the client sends from now on.
    pub fn record(self) -> (tokio::io::WriteHalf<DuplexStream>, Arc<Mutex<Vec<u8>>>, tokio::task::JoinHandle<()>) {
        let (mut rd, wr) = tokio::io::split(self.io);
        let log: Arc<Mutex<Vec<u8>>> = Arc::default();
        let sink = log.clone();
        let task = tokio::spawn(async move {
            let mut buf = [0; 4096];
            while let Ok(n @ 1..) = rd.read(&mut buf).await {
                sink.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        });
        (wr, log, task)
    }
}

/// A FramebufferUpdate rectangle header.
pub fn rect(x: u16, y: u16, w: u16, h: u16, encoding: i32) -> Vec<u8> {
    [x, y, w, h].iter().flat_map(|v| v.to_be_bytes()).chain(encoding.to_be_bytes()).collect()
}

/// FramebufferUpdate message header for `n` rectangles.
pub fn update(n: u16) -> Vec<u8> {
    let mut v = vec![0, 0];
    v.extend(n.to_be_bytes());
    v
}

/// Client connected to a fresh in-process server.
pub fn pair() -> (Fake, Box<dyn rxt_vnc::Stream>) {
    let (client, server) = tokio::io::duplex(1 << 20);
    (Fake { io: server }, Box::new(client))
}
