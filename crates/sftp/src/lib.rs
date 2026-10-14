//! SFTP over an existing SSH connection, implementing [`rxt_core::FileOps`].
//!
//! Contract used by `engine`: `Sftp::open(&client).await` returns an `Arc<dyn FileOps>`-able value.
//! Overwrite decisions are resolved by the caller (via `stat`) before `get`/`put`.

mod transfer;

use std::path::Path;
use std::sync::Arc;

use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::{RawSftpSession, SftpSession};
use russh_sftp::protocol::{FileAttributes, FileType, StatusCode};
use rxt_core::{BoxFut, Error, FileEntry, FileKind, FileOps, Result, fileops::Progress};
use tokio::io::AsyncWriteExt;

pub use transfer::{EditCopy, LocalPartial, copy_chunked};

/// SFTP session on an SSH connection (no re-authentication).
pub struct Sftp {
    session: Arc<SftpSession>,
    /// Second channel for READDIR: the high-level `read_dir` drops each entry's `longname`, the
    /// only place SFTPv3 servers send owner and group names. `None` if the server refused it.
    raw: Option<RawSftpSession>,
}

fn map_err(e: SftpError) -> Error {
    match e {
        SftpError::Status(s) => match s.status_code {
            StatusCode::NoSuchFile => Error::NotFound(s.error_message),
            StatusCode::PermissionDenied => Error::PermissionDenied(s.error_message),
            code => Error::Protocol(format!("{code}: {}", s.error_message)),
        },
        SftpError::IO(m) => Error::Io(m),
        other => Error::Protocol(other.to_string()),
    }
}

/// Owner and group: the `longname` names, else the attribute names, else the numeric ids.
fn owner_group(longname: &str, attrs: &FileAttributes) -> (Option<String>, Option<String>) {
    if let Some((o, g)) = rxt_core::fileops::ls_owner_group(longname) {
        return (Some(o), Some(g));
    }
    (
        attrs.user.clone().or_else(|| attrs.uid.map(|u| u.to_string())),
        attrs.group.clone().or_else(|| attrs.gid.map(|g| g.to_string())),
    )
}

fn entry(name: String, longname: &str, attrs: &FileAttributes) -> FileEntry {
    let kind = match attrs.file_type() {
        FileType::Dir => FileKind::Dir,
        FileType::File => FileKind::File,
        FileType::Symlink => FileKind::Symlink,
        FileType::Other => FileKind::Other,
    };
    let (owner, group) = owner_group(longname, attrs);
    FileEntry {
        name,
        kind,
        size: attrs.size.unwrap_or(0),
        modified: attrs.mtime.map(i64::from),
        mode: attrs.permissions.map(|p| p & 0o7777),
        owner,
        group,
    }
}

fn base_name(path: &str) -> String {
    path.trim_end_matches('/').rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("/").to_owned()
}

/// Removes a partially uploaded remote file unless [`RemotePartial::done`] was called.
struct RemotePartial {
    session: Arc<SftpSession>,
    path: String,
    done: bool,
}

impl Drop for RemotePartial {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let (session, path) = (self.session.clone(), std::mem::take(&mut self.path));
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                if let Err(e) = session.remove_file(path.as_str()).await {
                    tracing::warn!("cannot remove partial upload {path}: {e}");
                }
            });
        }
    }
}

impl Sftp {
    pub async fn open(client: &rxt_ssh::Client) -> Result<Self> {
        let stream = client.sftp_stream().await?;
        let session = SftpSession::new(stream).await.map_err(map_err)?;
        let raw = match Self::open_raw(client).await {
            Ok(raw) => Some(raw),
            Err(e) => {
                tracing::warn!("sftp listing channel unavailable, owner names fall back to ids: {e}");
                None
            }
        };
        Ok(Self { session: Arc::new(session), raw })
    }

    async fn open_raw(client: &rxt_ssh::Client) -> Result<RawSftpSession> {
        let raw = RawSftpSession::new(client.sftp_stream().await?);
        raw.init().await.map_err(map_err)?;
        Ok(raw)
    }

    async fn delete_recursive(&self, path: &str) -> Result<()> {
        let meta = self.session.symlink_metadata(path).await.map_err(map_err)?;
        if meta.file_type() != FileType::Dir {
            return self.session.remove_file(path).await.map_err(map_err);
        }
        for e in self.list_dir(path).await? {
            let child = rxt_core::fileops::join(path, &e.name);
            Box::pin(self.delete_recursive(&child)).await?;
        }
        self.session.remove_dir(path).await.map_err(map_err)
    }

    async fn list_dir(&self, path: &str) -> Result<Vec<FileEntry>> {
        let Some(raw) = &self.raw else {
            let dir = self.session.read_dir(path).await.map_err(map_err)?;
            return Ok(dir
                .filter(|e| !matches!(e.file_name().as_str(), "." | ".."))
                .map(|e| entry(e.file_name(), "", &e.metadata()))
                .collect());
        };
        let handle = raw.opendir(path).await.map_err(map_err)?.handle;
        let mut out = Vec::new();
        let read = loop {
            match raw.readdir(handle.as_str()).await {
                Ok(name) => out.extend(
                    name.files
                        .into_iter()
                        .filter(|f| !matches!(f.filename.as_str(), "." | ".."))
                        .map(|f| entry(f.filename, &f.longname, &f.attrs)),
                ),
                Err(SftpError::Status(s)) if s.status_code == StatusCode::Eof => break Ok(()),
                Err(e) => break Err(map_err(e)),
            }
        };
        // Close the handle even when a READDIR failed; a close error only matters on success.
        let closed = raw.close(handle).await.map_err(map_err);
        read?;
        closed?;
        Ok(out)
    }
}

impl FileOps for Sftp {
    fn home(&self) -> BoxFut<'_, Result<String>> {
        Box::pin(async { self.session.canonicalize(".").await.map_err(map_err) })
    }

    fn list<'a>(&'a self, path: &'a str) -> BoxFut<'a, Result<Vec<FileEntry>>> {
        Box::pin(self.list_dir(path))
    }

    fn stat<'a>(&'a self, path: &'a str) -> BoxFut<'a, Result<Option<FileEntry>>> {
        Box::pin(async move {
            match self.session.metadata(path).await.map_err(map_err) {
                Ok(m) => Ok(Some(entry(base_name(path), "", &m))),
                Err(Error::NotFound(_)) => Ok(None),
                Err(e) => Err(e),
            }
        })
    }

    fn mkdir<'a>(&'a self, path: &'a str) -> BoxFut<'a, Result<()>> {
        Box::pin(async move { self.session.create_dir(path).await.map_err(map_err) })
    }

    fn rename<'a>(&'a self, from: &'a str, to: &'a str) -> BoxFut<'a, Result<()>> {
        Box::pin(async move { self.session.rename(from, to).await.map_err(map_err) })
    }

    fn delete<'a>(&'a self, path: &'a str) -> BoxFut<'a, Result<()>> {
        Box::pin(self.delete_recursive(path))
    }

    fn chmod<'a>(&'a self, path: &'a str, mode: u32) -> BoxFut<'a, Result<()>> {
        Box::pin(async move {
            let attrs = FileAttributes { permissions: Some(mode & 0o7777), ..FileAttributes::empty() };
            self.session.set_metadata(path, attrs).await.map_err(map_err)
        })
    }

    fn get<'a>(&'a self, remote: &'a str, local: &'a Path, progress: Progress<'a>) -> BoxFut<'a, Result<u64>> {
        Box::pin(async move {
            let mut src = self.session.open(remote).await.map_err(map_err)?;
            let mut partial = LocalPartial::create(local).await?;
            let n = copy_chunked(&mut src, partial.file(), progress).await?;
            partial.finish().await?;
            Ok(n)
        })
    }

    fn real_path<'a>(&'a self, path: &'a str) -> BoxFut<'a, Result<String>> {
        Box::pin(async move { self.session.canonicalize(path).await.map_err(map_err) })
    }

    fn put<'a>(&'a self, local: &'a Path, remote: &'a str, progress: Progress<'a>) -> BoxFut<'a, Result<u64>> {
        Box::pin(async move {
            let mut src = tokio::fs::File::open(local).await?;
            let mut dst = self.session.create(remote).await.map_err(map_err)?;
            let mut guard = RemotePartial { session: self.session.clone(), path: remote.to_owned(), done: false };
            let n = copy_chunked(&mut src, &mut dst, progress).await?;
            dst.shutdown().await?;
            guard.done = true;
            Ok(n)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_entries() {
        assert_eq!(base_name("/var/log/"), "log");
        assert_eq!(base_name("a.txt"), "a.txt");
        assert_eq!(base_name("/"), "/");
        let mut a = FileAttributes::empty();
        a.permissions = Some(0o040755);
        a.size = Some(4096);
        a.mtime = Some(7);
        let e = entry("d".into(), "drwxr-xr-x 2 alice staff 4096 Jan 6 12:00 d", &a);
        assert_eq!((e.kind, e.mode, e.modified), (FileKind::Dir, Some(0o755), Some(7)));
        assert_eq!((e.owner.as_deref(), e.group.as_deref()), (Some("alice"), Some("staff")));
    }

    #[test]
    fn owner_group_fallbacks() {
        let mut a = FileAttributes::empty();
        assert_eq!(owner_group("garbage", &a), (None, None));
        a.uid = Some(1000);
        a.gid = Some(100);
        assert_eq!(owner_group("", &a), (Some("1000".into()), Some("100".into())));
        a.user = Some("bob".into());
        assert_eq!(owner_group("short line", &a), (Some("bob".into()), Some("100".into())));
    }
}
