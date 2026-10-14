//! Agent control over a named pipe restricted to the current user (design W4):
//! `\\.\pipe\rustxterm-<SID>-control`, exclusive first instance, remote clients rejected, a
//! protected DACL granting access to the user's SID only. Each connection is authenticated by its
//! own security context (impersonation for identification, after reading the 4-byte frame
//! length): same user, unrestricted, not AppContainer, integrity not below ours; otherwise it is
//! closed without a response. The client checks the server process's user. The pipe exists only
//! while a [`ControlListener`] is alive.

use std::io;
use std::os::windows::io::AsRawHandle;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use rxt_core::BoxFut;
use rxt_platform::{ControlListener, ControlStream, ControlTransport, Error, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;

use crate::{peer_allowed, pipe_name, pipe_sddl, sys};

const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_ACCESS_DENIED: i32 = 5;
const ERROR_PIPE_BUSY: i32 = 231;
/// How long the client waits for a free pipe instance.
const BUSY_WAIT: Duration = Duration::from_secs(5);
/// Bounded pre-authentication: the first 4 bytes must arrive within this time.
const PRE_AUTH: Duration = Duration::from_secs(5);
/// `SECURITY_IDENTIFICATION`: the server may identify the client but not act as it.
const SECURITY_IDENTIFICATION: u32 = 0x0001_0000;

/// Named-pipe control transport of the current user.
pub struct PipeControl {
    /// This process's identity; `None` when it could not be read (transport unsupported).
    me: Option<crate::Identity>,
}

impl Default for PipeControl {
    fn default() -> Self {
        Self::new()
    }
}

impl PipeControl {
    pub fn new() -> Self {
        let me = match sys::current_identity() {
            Ok(id) => Some(id),
            Err(e) => {
                tracing::warn!("agent control: cannot read the current user identity: {e}");
                None
            }
        };
        Self { me }
    }

    fn me(&self) -> Result<&crate::Identity> {
        self.me.as_ref().ok_or_else(|| Error::Unsupported("agent control: unknown user identity".into()))
    }
}

/// Create one pipe instance with the owner-only descriptor.
fn create(name: &str, sid: &str, first: bool) -> std::io::Result<NamedPipeServer> {
    let sd = sys::SecurityDescriptor::from_sddl(&pipe_sddl(sid))?;
    let mut attrs = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.as_ptr(),
        bInheritHandle: 0,
    };
    let mut options = ServerOptions::new();
    options.first_pipe_instance(first).reject_remote_clients(true);
    let attrs: *mut SECURITY_ATTRIBUTES = &mut attrs;
    // SAFETY: `attrs` points to a valid SECURITY_ATTRIBUTES whose descriptor (`sd`) is alive for
    // the call; CreateNamedPipeW copies the descriptor into the kernel object.
    unsafe { options.create_with_security_attributes_raw(name, attrs.cast()) }
}

impl ControlTransport for PipeControl {
    fn endpoint(&self) -> Option<String> {
        self.me.as_ref().map(|me| pipe_name(&me.sid))
    }

    fn listen(&self) -> Result<Box<dyn ControlListener>> {
        let me = self.me()?.clone();
        let sid = me.sid.clone();
        let name = pipe_name(&sid);
        let first = create(&name, &sid, true).map_err(|e| match e.raw_os_error() {
            // The name exists: another instance of ours, or a squatter (another user's pipe).
            Some(ERROR_ACCESS_DENIED) => Error::AlreadyExists(format!("agent control pipe {name} is in use")),
            _ => Error::Io(format!("agent control pipe {name}: {e}")),
        })?;
        Ok(Box::new(Pipe { name, me, next: tokio::sync::Mutex::new(first) }))
    }

    fn connect(&self) -> BoxFut<'_, Result<Box<dyn ControlStream>>> {
        Box::pin(async move {
            let me = self.me()?;
            let name = pipe_name(&me.sid);
            let deadline = tokio::time::Instant::now() + BUSY_WAIT;
            let client = loop {
                match ClientOptions::new().security_qos_flags(SECURITY_IDENTIFICATION).open(&name) {
                    Ok(c) => break c,
                    Err(e) if e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND) => {
                        return Err(Error::NotFound("no running instance with agent control enabled".into()));
                    }
                    Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && tokio::time::Instant::now() < deadline => {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    Err(e) => return Err(Error::Io(format!("agent control pipe: {e}"))),
                }
            };
            // The server must be this user too (never talk to an impostor pipe).
            // The server must pass the same policy as a client: this user, not sandboxed or lower.
            let server = sys::pipe_server_pid(client.as_raw_handle()).and_then(sys::process_identity).ok();
            crate::server_allowed(server.as_ref(), me).map_err(|m| Error::PermissionDenied(m.into()))?;
            Ok(Box::new(client) as Box<dyn ControlStream>)
        })
    }
}

/// Listening pipe: the instance waiting for the next client.
struct Pipe {
    name: String,
    me: crate::Identity,
    next: tokio::sync::Mutex<NamedPipeServer>,
}

impl Pipe {
    /// Pre-authentication read, then the impersonation check (design W4).
    async fn authenticate(&self, mut conn: NamedPipeServer) -> Option<Replay> {
        let mut head = [0u8; 4];
        match tokio::time::timeout(PRE_AUTH, conn.read_exact(&mut head)).await {
            Ok(Ok(_)) => {}
            _ => return None,
        }
        // Synchronous: impersonate, read the token, revert, with no await point in between.
        let client = sys::pipe_client_identity(conn.as_raw_handle()).ok();
        peer_allowed(client.as_ref(), &self.me).then_some(Replay { head, used: 0, inner: conn })
    }
}

/// Accepted connection that first replays the 4 bytes read during authentication.
struct Replay {
    head: [u8; 4],
    used: usize,
    inner: NamedPipeServer,
}

impl AsyncRead for Replay {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        if self.used < self.head.len() {
            let n = buf.remaining().min(self.head.len() - self.used);
            let start = self.used;
            buf.put_slice(&self.head[start..start + n]);
            self.used += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Replay {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl ControlListener for Pipe {
    fn accept(&self) -> BoxFut<'_, Result<Box<dyn ControlStream>>> {
        Box::pin(async move {
            loop {
                let mut next = self.next.lock().await;
                let connected = next.connect().await;
                // A fresh instance takes over listening before the connected one is handed out.
                let fresh = create(&self.name, &self.me.sid, false)?;
                let conn = std::mem::replace(&mut *next, fresh);
                drop(next);
                connected?;
                // ponytail: authentication runs inline, so a silent same-user client delays the
                // next accept by up to PRE_AUTH; other users cannot open the pipe at all.
                if let Some(stream) = self.authenticate(conn).await {
                    return Ok(Box::new(stream) as Box<dyn ControlStream>);
                }
                tracing::warn!("agent control: closed a connection that failed authentication");
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rxt_core::control::{AgentOp, EntryPoint, Request};
    use rxt_platform::control::MAX_FRAME;
    use rxt_platform::control::{read_message, write_frame, write_message};

    /// The pipe name is per user, so tests that listen on it run one at a time.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static PIPE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        PIPE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
    }

    /// Same-user round trip, second listener refused, pipe gone after drop, frame limit.
    #[test]
    fn pipe_lifecycle_and_round_trip() {
        let _pipe = serial();
        let t = PipeControl::new();
        rt().block_on(async {
            assert!(matches!(t.connect().await, Err(Error::NotFound(_))));
            let listener = t.listen().unwrap();
            assert!(matches!(t.listen(), Err(Error::AlreadyExists(_))), "second instance must not take the pipe");
            let server = async {
                let mut s = listener.accept().await.unwrap();
                let req: Request = read_message(&mut s).await.unwrap().unwrap();
                write_message(&mut s, &req).await.unwrap();
                let mut big = listener.accept().await.unwrap();
                assert!(read_message::<Request, _>(&mut big).await.is_err(), "oversized frame accepted");
            };
            let client = async {
                let mut c = t.connect().await.unwrap();
                let req = Request { id: 1, client: "t".into(), entry: EntryPoint::CliLive, op: AgentOp::ListTabs };
                write_message(&mut c, &req).await.unwrap();
                assert_eq!(read_message::<Request, _>(&mut c).await.unwrap(), Some(req));
                let mut c = t.connect().await.unwrap();
                tokio::io::AsyncWriteExt::write_all(&mut c, &((MAX_FRAME as u32) + 1).to_be_bytes()).await.unwrap();
                assert!(write_frame(&mut tokio::io::sink(), &vec![0; MAX_FRAME + 1]).await.is_err());
            };
            tokio::join!(server, client);
            drop(listener);
            assert!(matches!(t.connect().await, Err(Error::NotFound(_))), "pipe must vanish with the listener");
        });
    }

    /// A client whose connection token is restricted or low integrity is never handed on.
    fn refused_with(token: sys::test_tokens::Token) {
        let _pipe = serial();
        let t = PipeControl::new();
        rt().block_on(async {
            let listener = t.listen().unwrap();
            let client = std::thread::spawn(move || {
                let _imp = sys::test_tokens::impersonate(&token).unwrap();
                rt().block_on(async {
                    let Ok(mut c) = PipeControl::new().connect().await else { return true };
                    let req = Request { id: 9, client: "t".into(), entry: EntryPoint::CliLive, op: AgentOp::ListTabs };
                    let _ = write_message(&mut c, &req).await;
                    matches!(read_message::<Request, _>(&mut c).await, Err(_) | Ok(None))
                })
            });
            let accepted = tokio::time::timeout(Duration::from_secs(8), listener.accept()).await;
            assert!(accepted.is_err(), "a refused client must not be handed to the engine");
            assert!(client.join().unwrap(), "the refused client got a response");
        });
    }

    /// A client that sends nothing is closed after the pre-authentication timeout, unserved.
    #[test]
    fn silent_client_is_closed_after_pre_auth_timeout() {
        let _pipe = serial();
        let t = PipeControl::new();
        rt().block_on(async {
            let listener = t.listen().unwrap();
            let client = std::thread::spawn(|| {
                rt().block_on(async {
                    let mut c = PipeControl::new().connect().await.unwrap();
                    let start = std::time::Instant::now();
                    let closed = matches!(read_message::<Request, _>(&mut c).await, Err(_) | Ok(None));
                    (closed, start.elapsed())
                })
            });
            let accepted = tokio::time::timeout(PRE_AUTH + Duration::from_secs(2), listener.accept()).await;
            assert!(accepted.is_err(), "a silent client must not be handed to the engine");
            let (closed, waited) = client.join().unwrap();
            assert!(closed && waited >= PRE_AUTH - Duration::from_millis(500), "{waited:?}");
        });
    }

    #[test]
    fn restricted_token_client_is_refused() {
        refused_with(sys::test_tokens::restricted().unwrap());
    }

    #[test]
    fn deny_only_group_client_is_refused() {
        refused_with(sys::test_tokens::filtered(true).unwrap());
    }

    /// Removing privileges alone does not widen what a client can do: served.
    #[test]
    fn privilege_only_filtered_client_is_served() {
        let _pipe = serial();
        let token = sys::test_tokens::filtered(false).unwrap();
        let t = PipeControl::new();
        rt().block_on(async {
            let listener = t.listen().unwrap();
            let client = std::thread::spawn(move || {
                let _imp = sys::test_tokens::impersonate(&token).unwrap();
                rt().block_on(async {
                    let mut c = PipeControl::new().connect().await.unwrap();
                    let req = Request { id: 3, client: "t".into(), entry: EntryPoint::CliLive, op: AgentOp::ListTabs };
                    write_message(&mut c, &req).await.unwrap();
                    read_message::<Request, _>(&mut c).await.unwrap() == Some(req)
                })
            });
            let mut s = tokio::time::timeout(Duration::from_secs(8), listener.accept()).await.unwrap().unwrap();
            let req: Request = read_message(&mut s).await.unwrap().unwrap();
            write_message(&mut s, &req).await.unwrap();
            assert!(client.join().unwrap());
        });
    }

    #[test]
    fn low_integrity_client_is_refused() {
        refused_with(sys::test_tokens::low_integrity().unwrap());
    }

    #[test]
    fn concurrent_clients_are_served() {
        let _pipe = serial();
        let t = PipeControl::new();
        rt().block_on(async {
            let listener = t.listen().unwrap();
            let server = async {
                for _ in 0..4 {
                    let mut s = listener.accept().await.unwrap();
                    tokio::spawn(async move {
                        let req: Request = read_message(&mut s).await.unwrap().unwrap();
                        write_message(&mut s, &req).await.unwrap();
                    });
                }
            };
            let clients = futures_join(&t);
            let ((), ok) = tokio::join!(server, clients);
            assert_eq!(ok, 4);
        });
    }

    async fn futures_join(t: &PipeControl) -> usize {
        let one = |id| async move {
            let mut c = t.connect().await.unwrap();
            let req = Request { id, client: "t".into(), entry: EntryPoint::CliLive, op: AgentOp::ListTabs };
            write_message(&mut c, &req).await.unwrap();
            read_message::<Request, _>(&mut c).await.unwrap() == Some(req)
        };
        let (a, b, c, d) = tokio::join!(one(1), one(2), one(3), one(4));
        [a, b, c, d].iter().filter(|ok| **ok).count()
    }

    /// A pipe created first by someone else (here: a squatter with the default DACL) is not
    /// taken over, and `listen` reports it.
    #[test]
    fn squatted_name_is_refused() {
        let _pipe = serial();
        let t = PipeControl::new();
        let name = t.endpoint().unwrap();
        rt().block_on(async {
            let _squatter = ServerOptions::new().first_pipe_instance(true).create(&name).unwrap();
            assert!(t.listen().is_err(), "listen must not serve on an existing pipe");
        });
    }
}
