//! Agent control transport: a Unix domain socket `control.sock` (0600) in
//! `$TMPDIR/rustxterm-<uid>/` (0700, owned by this user). Only same-UID peers are served
//! (getpeereid / LOCAL_PEERCRED); others get one `denied` response with the reason and are closed. The socket exists only while a
//! [`ControlListener`] is alive; dropping it removes the file.

use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rxt_core::BoxFut;
use rxt_core::control::{ErrorCode, Response};
use rxt_platform::control::write_message;
use rxt_platform::{ControlListener, ControlStream, ControlTransport, Error, Result};
use tokio::net::{UnixListener, UnixStream};

/// Socket file name inside the runtime directory.
pub const SOCKET: &str = "control.sock";
/// Reason sent to a peer of another user (or with unknown credentials).
pub const OTHER_USER: &str = "agent control serves only the user running rustXterm";
/// How long a refused client gets to read its `denied` response.
const REFUSAL_LINGER: Duration = Duration::from_secs(1);

/// Unix-socket control transport rooted at `dir` (normally `Paths::runtime_dir()`).
pub struct UnixControl {
    dir: Option<PathBuf>,
}

impl UnixControl {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self { dir }
    }

    fn socket(&self) -> Result<PathBuf> {
        self.dir
            .as_ref()
            .map(|d| d.join(SOCKET))
            .ok_or_else(|| Error::Unsupported("no runtime directory ($TMPDIR) for agent control".into()))
    }
}

/// This process's effective UID, read back through the peer credentials of a socket pair
/// (`getpeereid` on macOS; no libc). Call only inside a Tokio runtime.
fn own_uid() -> Result<u32> {
    let (a, _b) = UnixStream::pair()?;
    Ok(a.peer_cred()?.uid())
}

/// Whether a peer may use the channel: same UID only, unknown credentials refused.
pub fn peer_allowed(peer_uid: Option<u32>, own_uid: u32) -> bool {
    peer_uid == Some(own_uid)
}

/// Create `dir` as 0700 if missing, then require: a real directory (not a symlink), owned by
/// `uid`, without group/other permissions. Never loosens or takes over an existing directory.
pub fn private_dir(dir: &Path, uid: u32) -> Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    let meta = fs::symlink_metadata(dir)?;
    if !meta.is_dir() {
        return Err(Error::PermissionDenied(format!("{} is not a directory", dir.display())));
    }
    if meta.uid() != uid {
        return Err(Error::PermissionDenied(format!("{} is owned by another user", dir.display())));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(Error::PermissionDenied(format!("{} is accessible by other users", dir.display())));
    }
    Ok(())
}

impl ControlTransport for UnixControl {
    fn endpoint(&self) -> Option<String> {
        self.socket().ok().map(|p| p.display().to_string())
    }

    fn listen(&self) -> Result<Box<dyn ControlListener>> {
        let path = self.socket()?;
        let uid = own_uid()?;
        if let Some(dir) = path.parent() {
            if let Some(base) = dir.parent() {
                fs::create_dir_all(base)?;
            }
            private_dir(dir, uid)?;
        }
        if fs::symlink_metadata(&path).is_ok() {
            // A live socket means another instance serves agents; a stale one is replaced.
            if std::os::unix::net::UnixStream::connect(&path).is_ok() {
                return Err(Error::AlreadyExists(format!("agent control socket {} is in use", path.display())));
            }
            fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)?;
        let socket = Socket { listener, path, uid };
        // The directory is private already; 0600 also guards against a later chmod of it.
        fs::set_permissions(&socket.path, fs::Permissions::from_mode(0o600))?;
        Ok(Box::new(socket))
    }

    fn connect(&self) -> BoxFut<'_, Result<Box<dyn ControlStream>>> {
        Box::pin(async move {
            let path = self.socket()?;
            if fs::symlink_metadata(&path).is_err() {
                return Err(Error::NotFound("no running instance with agent control enabled".into()));
            }
            let stream = UnixStream::connect(&path).await.map_err(|e| match e.kind() {
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound => {
                    Error::NotFound("no running instance with agent control enabled".into())
                }
                _ => e.into(),
            })?;
            // The server must be this user too.
            if !peer_allowed(stream.peer_cred().ok().map(|c| c.uid()), own_uid()?) {
                return Err(Error::PermissionDenied("control socket is served by another user".into()));
            }
            Ok(Box::new(stream) as Box<dyn ControlStream>)
        })
    }
}

struct Socket {
    listener: UnixListener,
    path: PathBuf,
    uid: u32,
}

impl ControlListener for Socket {
    fn accept(&self) -> BoxFut<'_, Result<Box<dyn ControlStream>>> {
        Box::pin(async move {
            loop {
                let (stream, _) = self.listener.accept().await?;
                if peer_allowed(stream.peer_cred().ok().map(|c| c.uid()), self.uid) {
                    return Ok(Box::new(stream) as Box<dyn ControlStream>);
                }
                tracing::warn!("agent control: refused a connection from another user");
                tokio::spawn(refuse(stream, OTHER_USER));
            }
        })
    }
}

/// Tell a refused client why, then close: one `denied` response, and the rest of its request is
/// drained (bounded) so the client can still read the answer after its write.
async fn refuse(mut stream: UnixStream, reason: &'static str) {
    let response = Response::Err { id: 0, code: ErrorCode::Denied, message: reason.into() };
    let _ = tokio::time::timeout(REFUSAL_LINGER, async {
        if write_message(&mut stream, &response).await.is_ok() {
            let _ = tokio::io::copy(&mut stream, &mut tokio::io::sink()).await;
        }
    })
    .await;
}

impl Drop for Socket {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rxt_core::control::{AgentOp, EntryPoint, Request};
    use rxt_platform::control::{read_message, write_message};

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rxt-ctl-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn other_uid_is_refused() {
        assert!(peer_allowed(Some(1000), 1000));
        assert!(!peer_allowed(Some(0), 1000));
        assert!(!peer_allowed(None, 1000));
    }

    /// A refused peer reads one `denied` response with the reason after sending its request.
    #[test]
    fn refused_peer_gets_the_reason() {
        rt().block_on(async {
            let (server, mut client) = UnixStream::pair().unwrap();
            tokio::spawn(refuse(server, OTHER_USER));
            let req = Request { id: 1, client: "t".into(), entry: EntryPoint::CliLive, op: AgentOp::ListTabs };
            write_message(&mut client, &req).await.unwrap();
            let got: Option<Response> = read_message(&mut client).await.unwrap();
            assert_eq!(got, Some(Response::Err { id: 0, code: ErrorCode::Denied, message: OTHER_USER.into() }));
            assert!(read_message::<Response, _>(&mut client).await.unwrap().is_none(), "closed after the reason");
        });
    }

    #[test]
    fn socket_lifecycle_modes_and_round_trip() {
        let base = tmp("life");
        let dir = base.join("rustxterm");
        let t = UnixControl::new(Some(dir.clone()));
        let sock = dir.join(SOCKET);
        rt().block_on(async {
            assert!(matches!(t.connect().await, Err(Error::NotFound(_))));
            let listener = t.listen().unwrap();
            assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
            assert_eq!(fs::symlink_metadata(&sock).unwrap().permissions().mode() & 0o777, 0o600);
            assert!(matches!(t.listen(), Err(Error::AlreadyExists(_))), "second instance must not steal the socket");
            let server = async {
                // Two connections arrive: the empty probe from the refused second `listen` and the
                // real client. Their arrival order is not guaranteed across platforms (Linux queues
                // the probe first, macOS may not), so accept until a request is read and reply to it.
                loop {
                    let mut s = listener.accept().await.unwrap();
                    if let Some(req) = read_message::<Request, _>(&mut s).await.unwrap() {
                        write_message(&mut s, &req).await.unwrap();
                        break;
                    }
                }
            };
            let client = async {
                let mut c = t.connect().await.unwrap();
                let req = Request { id: 1, client: "t".into(), entry: EntryPoint::CliLive, op: AgentOp::ListTabs };
                write_message(&mut c, &req).await.unwrap();
                assert_eq!(read_message::<Request, _>(&mut c).await.unwrap(), Some(req));
            };
            tokio::join!(server, client);
            drop(listener);
        });
        assert!(!sock.exists(), "socket must be removed when the listener is dropped");
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn stale_socket_is_replaced() {
        let base = tmp("stale");
        let dir = base.join("rustxterm");
        let t = UnixControl::new(Some(dir.clone()));
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        // A crashed instance leaves the file behind with nobody listening.
        drop(std::os::unix::net::UnixListener::bind(dir.join(SOCKET)).unwrap());
        assert!(dir.join(SOCKET).exists());
        rt().block_on(async { assert!(t.listen().is_ok()) });
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn insecure_runtime_dir_is_refused() {
        let base = tmp("perm");
        let dir = base.join("rustxterm");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        let uid = fs::metadata(&base).unwrap().uid();
        assert!(matches!(private_dir(&dir, uid), Err(Error::PermissionDenied(_))));
        assert!(matches!(private_dir(&dir, uid + 1), Err(Error::PermissionDenied(_))));
        let link = base.join("link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(private_dir(&dir, uid).is_ok());
        assert!(matches!(private_dir(&link, uid), Err(Error::PermissionDenied(_))));
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn no_runtime_dir_is_unsupported() {
        let t = UnixControl::new(None);
        assert_eq!(t.endpoint(), None);
        rt().block_on(async { assert!(matches!(t.listen(), Err(Error::Unsupported(_)))) });
    }
}
