//! FTP/FTPS client on suppaftp implementing [`rxt_core::FileOps`] (design D5).
//!
//! Contract used by `engine` (keep stable; extend, don't break): [`connect`] returns an
//! [`Ftp`] that implements `FileOps`; FTPS certificates go through `CertTrust` + `Prompter`.
//! The control connection is not concurrent: every operation holds one mutex for its duration.

mod tls;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use rxt_core::fileops::{Progress, join, parent};
use rxt_core::{
    BoxFut, CertTrust, Error, FileEntry, FileKind, FileOps, FtpTls, PromptAnswer, PromptKind, Prompter, Protocol,
    Result, Session,
};
use suppaftp::list::{ListParser, PosixPexQuery};
use suppaftp::tokio::{AsyncDataStream, AsyncRustlsFtpStream, AsyncRustlsStream, TokioTlsStream, TransferStream};
use suppaftp::types::{FileType, FormatControl};
use suppaftp::{FtpError, Mode, Status};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub use tls::fingerprint;

/// Plain connections use the same stream type without a TLS context.
type Stream = AsyncRustlsFtpStream;

/// Inputs for [`connect`].
#[derive(Clone)]
pub struct Context {
    pub prompter: Arc<dyn Prompter>,
    pub trust: Arc<dyn CertTrust>,
    pub cancel: CancellationToken,
}

/// Logged-in FTP/FTPS connection.
pub struct Ftp {
    inner: Arc<Mutex<Stream>>,
    home: String,
    /// Server advertises MLST/MLSD (RFC 3659); otherwise `LIST` is parsed.
    mlsd: bool,
    ascii: bool,
}

const LOGIN_ATTEMPTS: usize = 3;
const CHUNK: usize = 64 * 1024;

fn map_err(e: FtpError) -> Error {
    match e {
        FtpError::ConnectionError(e) => Error::Connection(e.to_string()),
        FtpError::SecureError(m) => Error::Connection(format!("TLS: {m}")),
        FtpError::UnexpectedResponse(r) => {
            let msg = r.to_string();
            match r.status {
                Status::NotLoggedIn | Status::InvalidCredentials => Error::Auth(msg),
                // ponytail: 550 also means "permission denied" on many servers; FTP has no finer code.
                Status::FileUnavailable => Error::NotFound(msg),
                Status::BadFilename => Error::PermissionDenied(msg),
                _ => Error::Protocol(msg),
            }
        }
        other => Error::Protocol(other.to_string()),
    }
}

/// Connect, verify the FTPS certificate (system roots, then trust records, then prompt), log in.
pub async fn connect(session: &Session, ctx: Context) -> Result<Ftp> {
    tokio::select! {
        biased;
        () = ctx.cancel.cancelled() => Err(Error::Cancelled),
        r = connect_inner(session, &ctx) => r,
    }
}

async fn connect_inner(session: &Session, ctx: &Context) -> Result<Ftp> {
    let (host, port) = (session.host.as_str(), session.port);
    let mut ftp = match session.protocol {
        Protocol::Ftp => Stream::connect((host, port)).await.map_err(map_err)?,
        Protocol::Ftps => secure(host, port, session.ftp.tls, ctx).await?,
        p => return Err(Error::Invalid(format!("{p:?} is not an FTP protocol"))),
    };
    login(&mut ftp, session, ctx.prompter.as_ref()).await?;

    let features = ftp.feat().await.unwrap_or_default();
    if features.contains_key("UTF8")
        && let Err(e) = ftp.opts("UTF8", Some("ON")).await
    {
        tracing::debug!("OPTS UTF8 ON refused: {e}");
    }
    let opts = &session.ftp;
    let kind = if opts.ascii { FileType::Ascii(FormatControl::Default) } else { FileType::Binary };
    ftp.transfer_type(kind).await.map_err(map_err)?;
    if opts.passive {
        // Connect data channels to the control peer, never to an address the server names.
        ftp.set_passive_nat_workaround(true);
    } else {
        ftp.set_mode(Mode::Active);
    }
    if let Some(dir) = opts.startup_folder.as_deref().filter(|d| !d.is_empty()) {
        ftp.cwd(dir).await.map_err(map_err)?;
    }
    let home = ftp.pwd().await.map_err(map_err)?;
    Ok(Ftp { inner: Arc::new(Mutex::new(ftp)), home, mlsd: features.contains_key("MLST"), ascii: opts.ascii })
}

async fn open_tls(host: &str, port: u16, mode: FtpTls, verifier: &Arc<tls::Verifier>) -> Result<Stream> {
    let connector = verifier.connector()?;
    let r = match mode {
        FtpTls::Explicit => Stream::connect((host, port)).await.map_err(map_err)?.into_secure(connector, host).await,
        FtpTls::Implicit => match Stream::connect_secure_implicit((host, port), connector, host).await {
            // suppaftp protects data connections only after AUTH TLS; implicit needs it too.
            Ok(mut s) => match s.custom_command("PBSZ 0", &[Status::CommandOk]).await {
                Ok(_) => s.custom_command("PROT P", &[Status::CommandOk]).await.map(|_| s),
                Err(e) => Err(e),
            },
            Err(e) => Err(e),
        },
    };
    r.map_err(map_err)
}

/// Close an upload's TLS data channel cleanly. TLS 1.3 servers send session tickets on it that
/// an uploading client never reads; closing with them unread makes the kernel reset the
/// connection and the server reports the upload as failed (426). So send close_notify and read
/// until the server closes, then let `finish` read the reply.
async fn close_upload(dst: &mut TransferStream<AsyncRustlsStream>) -> Result<()> {
    let AsyncDataStream::Ssl(tls) = dst.get_mut() else { return Ok(()) };
    tls.mut_ref().get_mut().1.send_close_notify();
    dst.flush().await?;
    let mut sink = [0u8; 4096];
    let drain = async { while let Ok(1..) = dst.read(&mut sink).await {} };
    let _ = tokio::time::timeout(Duration::from_secs(10), drain).await;
    Ok(())
}

/// TLS before login: an unknown certificate is prompted for, then the connection is reopened
/// trusting exactly the accepted fingerprint. A changed pinned certificate is refused.
async fn secure(host: &str, port: u16, mode: FtpTls, ctx: &Context) -> Result<Stream> {
    let roots = tls::system_roots();
    let pinned = ctx.trust.get(host, port);
    let first = tls::Verifier::new(roots.clone(), pinned.clone(), None);
    let err = match open_tls(host, port, mode, &first).await {
        Ok(s) => return Ok(s),
        Err(e) => e,
    };
    let Some(refusal) = first.refusal() else { return Err(err) };
    let accepted = tls::resolve(refusal, ctx.prompter.as_ref(), ctx.trust.as_ref(), host, port).await?;
    let retry = tls::Verifier::new(roots, pinned, Some(accepted));
    match open_tls(host, port, mode, &retry).await {
        Err(_) if retry.refusal().is_some() => {
            Err(Error::HostKey(format!("{host}:{port}: certificate changed while connecting")))
        }
        r => r,
    }
}

async fn login(ftp: &mut Stream, session: &Session, prompter: &dyn Prompter) -> Result<()> {
    let Some(user) = session.user.as_deref().filter(|u| !u.is_empty()) else {
        return ftp.login("anonymous", "anonymous@").await.map_err(map_err);
    };
    let mut err = Error::Auth(format!("{user}@{}", session.host));
    for attempt in 0..LOGIN_ATTEMPTS {
        let kind = PromptKind::Password { user: user.to_owned(), host: session.host.clone(), retry: attempt > 0 };
        let PromptAnswer::Secret(pass) = prompter.prompt(kind).await else { return Err(Error::Cancelled) };
        match ftp.login(user, pass.0.as_str()).await.map_err(map_err) {
            Ok(()) => return Ok(()),
            Err(e @ Error::Auth(_)) => err = e,
            Err(e) => return Err(e),
        }
    }
    Err(err)
}

fn base_name(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

/// One `MLSD` or `LIST` (unix `ls -l`, then DOS) line; `.`, `..` and `total` lines give `None`.
fn parse_line(line: &str, mlsd: bool) -> Option<FileEntry> {
    let f = if mlsd {
        ListParser::parse_mlsd(line)
    } else {
        ListParser::parse_posix(line).or_else(|_| ListParser::parse_dos(line))
    }
    .ok()?;
    if matches!(f.name(), "." | "..") {
        return None;
    }
    // MLSD only carries numeric ids; LIST lines carry the names.
    let (owner, group) = if mlsd {
        (f.uid().map(|u| u.to_string()), f.gid().map(|g| g.to_string()))
    } else {
        rxt_core::fileops::ls_owner_group(line).unzip()
    };
    let kind = if f.is_directory() {
        FileKind::Dir
    } else if f.is_symlink() {
        FileKind::Symlink
    } else if f.is_file() {
        FileKind::File
    } else {
        FileKind::Other
    };
    let mode = [PosixPexQuery::Owner, PosixPexQuery::Group, PosixPexQuery::Others].into_iter().fold(0, |m, who| {
        m << 3 | u32::from(f.can_read(who)) << 2 | u32::from(f.can_write(who)) << 1 | u32::from(f.can_execute(who))
    });
    Some(FileEntry {
        name: f.name().to_owned(),
        kind,
        size: f.size() as u64,
        modified: f.modified().duration_since(UNIX_EPOCH).ok().and_then(|d| i64::try_from(d.as_secs()).ok()),
        mode: Some(mode),
        owner,
        group,
    })
}

/// Line-ending conversion for ASCII transfers (the server's own conversion is not relied on).
enum Eol {
    Keep,
    /// Upload: LF → CRLF; holds the previous byte.
    ToCrlf(u8),
    /// Download: CRLF → LF; holds a pending CR.
    FromCrlf(bool),
}

impl Eol {
    fn convert(&mut self, chunk: &[u8], out: &mut Vec<u8>) {
        out.clear();
        match self {
            Self::Keep => out.extend_from_slice(chunk),
            Self::ToCrlf(prev) => {
                for &b in chunk {
                    if b == b'\n' && *prev != b'\r' {
                        out.push(b'\r');
                    }
                    out.push(b);
                    *prev = b;
                }
            }
            Self::FromCrlf(cr) => {
                for &b in chunk {
                    if std::mem::take(cr) && b != b'\n' {
                        out.push(b'\r');
                    }
                    if b == b'\r' { *cr = true } else { out.push(b) }
                }
            }
        }
    }

    fn tail(&self) -> &'static [u8] {
        if matches!(self, Self::FromCrlf(true)) { b"\r" } else { b"" }
    }
}

/// Chunked copy reporting cumulative source bytes; cancellation-safe at chunk boundaries.
async fn copy<R, W>(src: &mut R, dst: &mut W, progress: Progress<'_>, mut eol: Eol) -> Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (mut buf, mut out) = (vec![0u8; CHUNK], Vec::with_capacity(CHUNK * 2));
    let mut total = 0u64;
    loop {
        let n = src.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        eol.convert(&buf[..n], &mut out);
        dst.write_all(&out).await?;
        total += n as u64;
        progress(total);
    }
    dst.write_all(eol.tail()).await?;
    dst.flush().await?;
    Ok(total)
}

/// Download target written to `<name>.rxt-partial`, renamed over the target only on success;
/// dropped earlier (error or cancelled future), the partial file is removed.
struct LocalPartial {
    file: tokio::fs::File,
    partial: PathBuf,
    target: PathBuf,
    done: bool,
}

impl LocalPartial {
    async fn create(target: &Path) -> Result<Self> {
        let mut name = target.file_name().map(OsString::from).unwrap_or_else(|| OsString::from("download"));
        name.push(".rxt-partial");
        let partial = target.with_file_name(name);
        let file = tokio::fs::File::create(&partial).await?;
        Ok(Self { file, partial, target: target.to_owned(), done: false })
    }

    async fn finish(mut self) -> Result<()> {
        self.file.sync_all().await?;
        tokio::fs::rename(&self.partial, &self.target).await?;
        self.done = true;
        Ok(())
    }
}

impl Drop for LocalPartial {
    fn drop(&mut self) {
        if !self.done {
            let _ = std::fs::remove_file(&self.partial);
        }
    }
}

/// Removes a partially uploaded remote file unless `done`.
struct RemotePartial {
    inner: Arc<Mutex<Stream>>,
    path: String,
    done: bool,
}

impl Drop for RemotePartial {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let (inner, path) = (self.inner.clone(), std::mem::take(&mut self.path));
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                if let Err(e) = inner.lock().await.rm(&path).await {
                    tracing::warn!("cannot remove partial upload {path}: {e}");
                }
            });
        }
    }
}

impl Ftp {
    async fn list_with(&self, ftp: &mut Stream, path: &str) -> Result<Vec<FileEntry>> {
        let lines = if self.mlsd { ftp.mlsd(Some(path)).await } else { ftp.list(Some(path)).await };
        Ok(lines.map_err(map_err)?.iter().filter_map(|l| parse_line(l, self.mlsd)).collect())
    }

    async fn stat_with(&self, ftp: &mut Stream, path: &str) -> Result<Option<FileEntry>> {
        let name = base_name(path);
        if name.is_empty() {
            return Ok(Some(FileEntry {
                name: "/".into(),
                kind: FileKind::Dir,
                size: 0,
                modified: None,
                mode: None,
                owner: None,
                group: None,
            }));
        }
        // ponytail: FTP has no portable stat; the parent listing hides dot files on some servers.
        match self.list_with(ftp, &parent(path)).await {
            Ok(entries) => Ok(entries.into_iter().find(|e| e.name == name)),
            Err(Error::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn delete_with(&self, ftp: &mut Stream, path: &str) -> Result<()> {
        let entry = self.stat_with(ftp, path).await?.ok_or_else(|| Error::NotFound(path.into()))?;
        if entry.kind != FileKind::Dir {
            return ftp.rm(path).await.map_err(map_err);
        }
        for e in self.list_with(ftp, path).await? {
            Box::pin(self.delete_with(ftp, &join(path, &e.name))).await?;
        }
        ftp.rmdir(path).await.map_err(map_err)
    }
}

impl FileOps for Ftp {
    fn home(&self) -> BoxFut<'_, Result<String>> {
        Box::pin(async { Ok(self.home.clone()) })
    }

    fn list<'a>(&'a self, path: &'a str) -> BoxFut<'a, Result<Vec<FileEntry>>> {
        Box::pin(async move { self.list_with(&mut *self.inner.lock().await, path).await })
    }

    fn stat<'a>(&'a self, path: &'a str) -> BoxFut<'a, Result<Option<FileEntry>>> {
        Box::pin(async move { self.stat_with(&mut *self.inner.lock().await, path).await })
    }

    fn mkdir<'a>(&'a self, path: &'a str) -> BoxFut<'a, Result<()>> {
        Box::pin(async move { self.inner.lock().await.mkdir(path).await.map_err(map_err) })
    }

    fn rename<'a>(&'a self, from: &'a str, to: &'a str) -> BoxFut<'a, Result<()>> {
        Box::pin(async move { self.inner.lock().await.rename(from, to).await.map_err(map_err) })
    }

    fn delete<'a>(&'a self, path: &'a str) -> BoxFut<'a, Result<()>> {
        Box::pin(async move { self.delete_with(&mut *self.inner.lock().await, path).await })
    }

    fn chmod<'a>(&'a self, path: &'a str, mode: u32) -> BoxFut<'a, Result<()>> {
        Box::pin(async move {
            let cmd = format!("CHMOD {:o} {path}", mode & 0o7777);
            self.inner.lock().await.site(cmd).await.map(drop).map_err(map_err)
        })
    }

    fn get<'a>(&'a self, remote: &'a str, local: &'a Path, progress: Progress<'a>) -> BoxFut<'a, Result<u64>> {
        Box::pin(async move {
            let mut ftp = self.inner.lock().await;
            let mut src = ftp.retr_as_stream(remote).await.map_err(map_err)?;
            let mut partial = LocalPartial::create(local).await?;
            let eol = if self.ascii { Eol::FromCrlf(false) } else { Eol::Keep };
            let n = copy(&mut src, &mut partial.file, progress, eol).await?;
            src.finish().await.map_err(map_err)?;
            partial.finish().await?;
            Ok(n)
        })
    }

    fn put<'a>(&'a self, local: &'a Path, remote: &'a str, progress: Progress<'a>) -> BoxFut<'a, Result<u64>> {
        Box::pin(async move {
            let mut src = tokio::fs::File::open(local).await?;
            let mut ftp = self.inner.lock().await;
            let mut dst = ftp.put_with_stream(remote).await.map_err(map_err)?;
            let mut guard = RemotePartial { inner: self.inner.clone(), path: remote.to_owned(), done: false };
            let eol = if self.ascii { Eol::ToCrlf(0) } else { Eol::Keep };
            let n = copy(&mut src, &mut dst, progress, eol).await?;
            close_upload(&mut dst).await?;
            dst.finish().await.map_err(map_err)?;
            guard.done = true;
            Ok(n)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_lines() {
        let e = parse_line("drwxr-x--- 2 1000 1000 4096 Oct 13 12:34 my dir", false).unwrap();
        assert_eq!((e.name.as_str(), e.kind, e.mode), ("my dir", FileKind::Dir, Some(0o750)));
        let e = parse_line("-rw-r--r-- 1 ftp ftp 10485760 Jan 02 2024 data.bin", false).unwrap();
        assert_eq!((e.owner.as_deref(), e.group.as_deref()), (Some("ftp"), Some("ftp")));
        assert_eq!((e.kind, e.size, e.mode), (FileKind::File, 10_485_760, Some(0o644)));
        assert!(e.modified.is_some());
        let e = parse_line("lrwxrwxrwx 1 0 0 7 Oct 13 12:34 link -> target", false).unwrap();
        assert_eq!((e.name.as_str(), e.kind), ("link", FileKind::Symlink));
        let e = parse_line("-rw-r--r-- 1 0 0 3 Oct 13 12:34 ñandú.txt", false).unwrap();
        assert_eq!(e.name, "ñandú.txt");
        assert_eq!(parse_line("total 12", false), None);
        let e = parse_line("type=file;size=3;modify=20240102030405;unix.mode=0640; a.txt", true).unwrap();
        assert_eq!((e.name.as_str(), e.size, e.mode), ("a.txt", 3, Some(0o640)));
        assert_eq!(parse_line("type=cdir;modify=20240102030405; .", true), None);
    }

    #[test]
    fn path_helpers() {
        assert_eq!(base_name("/a/b/"), "b");
        assert_eq!(base_name("c"), "c");
        assert_eq!(base_name("/"), "");
    }

    fn eol(mut e: Eol, chunks: &[&[u8]]) -> Vec<u8> {
        let (mut all, mut out) = (Vec::new(), Vec::new());
        for c in chunks {
            e.convert(c, &mut out);
            all.extend_from_slice(&out);
        }
        all.extend_from_slice(e.tail());
        all
    }

    #[test]
    fn ascii_line_endings() {
        assert_eq!(eol(Eol::ToCrlf(0), &[b"a\nb\r", b"\nc\n"]), b"a\r\nb\r\nc\r\n");
        assert_eq!(eol(Eol::FromCrlf(false), &[b"a\r", b"\nb\r\rc\r"]), b"a\nb\r\rc\r");
        assert_eq!(eol(Eol::Keep, &[b"a\r\n"]), b"a\r\n");
    }

    #[tokio::test]
    async fn local_partial_removed_unless_finished() {
        let dir = std::env::temp_dir().join(format!("rxt-ftp-unit-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let target = dir.join("t.bin");
        drop(LocalPartial::create(&target).await.unwrap());
        assert!(!dir.join("t.bin.rxt-partial").exists() && !target.exists());
        LocalPartial::create(&target).await.unwrap().finish().await.unwrap();
        assert!(target.exists());
        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }
}
