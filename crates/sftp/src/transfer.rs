//! Protocol-independent transfer helpers: chunked copy, partial-file guard and open-with edit copies.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use rxt_core::{FileOps, Result, fileops::Progress};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const CHUNK: usize = 64 * 1024;

/// Copy in fixed chunks, reporting cumulative bytes after each chunk. Cancellation-safe at chunk boundaries.
pub async fn copy_chunked<R, W>(src: &mut R, dst: &mut W, progress: Progress<'_>) -> Result<u64>
where
    R: AsyncRead + Unpin + ?Sized,
    W: AsyncWrite + Unpin + ?Sized,
{
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        let n = src.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n]).await?;
        total += n as u64;
        progress(total);
    }
    dst.flush().await?;
    Ok(total)
}

/// Download target written to `<name>.rxt-partial` and renamed over `target` only by [`finish`](Self::finish).
/// Dropping it earlier (error or cancelled future) removes the partial file and leaves `target` untouched.
pub struct LocalPartial {
    file: tokio::fs::File,
    partial: PathBuf,
    target: PathBuf,
    done: bool,
}

impl LocalPartial {
    pub async fn create(target: &Path) -> Result<Self> {
        let mut name = target.file_name().map(OsString::from).unwrap_or_else(|| OsString::from("download"));
        name.push(".rxt-partial");
        let partial = target.with_file_name(name);
        let file = tokio::fs::File::create(&partial).await?;
        Ok(Self { file, partial, target: target.to_owned(), done: false })
    }

    pub fn file(&mut self) -> &mut tokio::fs::File {
        &mut self.file
    }

    pub async fn finish(mut self) -> Result<()> {
        self.file.flush().await?;
        self.file.sync_all().await?;
        tokio::fs::rename(&self.partial, &self.target).await?;
        self.done = true;
        Ok(())
    }
}

impl Drop for LocalPartial {
    fn drop(&mut self) {
        if !self.done {
            // Sync remove: Drop cannot await, and the file is local.
            let _ = std::fs::remove_file(&self.partial);
        }
    }
}

/// A remote file downloaded for "open with"; upload it back after the local editor saves.
#[derive(Debug, Clone)]
pub struct EditCopy {
    pub remote: String,
    pub local: PathBuf,
    synced: Option<SystemTime>,
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl EditCopy {
    /// Download `remote` into a fresh subdirectory of `dir`, keeping the remote file name.
    pub async fn download(ops: &dyn FileOps, remote: &str, dir: &Path) -> Result<Self> {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let sub = dir.join(format!("rxt-edit-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
        tokio::fs::create_dir_all(&sub).await?;
        let name = remote.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("file");
        let local = sub.join(name);
        ops.get(remote, &local, &|_| {}).await?;
        Ok(Self { remote: remote.to_owned(), synced: mtime(&local), local })
    }

    /// Whether the local copy changed since the last download or upload.
    pub fn changed(&self) -> bool {
        mtime(&self.local) != self.synced
    }

    /// Upload the local copy back to its remote path.
    pub async fn upload_back(&mut self, ops: &dyn FileOps, progress: Progress<'_>) -> Result<u64> {
        let n = ops.put(&self.local, &self.remote, progress).await?;
        self.synced = mtime(&self.local);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use rxt_core::{BoxFut, Error, FileEntry, FileKind};

    use super::*;

    /// In-memory FileOps: path → contents.
    #[derive(Default)]
    struct Mock(Mutex<HashMap<String, Vec<u8>>>);

    impl FileOps for Mock {
        fn home(&self) -> BoxFut<'_, Result<String>> {
            Box::pin(async { Ok("/home/test".into()) })
        }
        fn list<'a>(&'a self, _: &'a str) -> BoxFut<'a, Result<Vec<FileEntry>>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn stat<'a>(&'a self, path: &'a str) -> BoxFut<'a, Result<Option<FileEntry>>> {
            Box::pin(async move {
                Ok(self.0.lock().unwrap().get(path).map(|d| FileEntry {
                    name: path.into(),
                    kind: FileKind::File,
                    size: d.len() as u64,
                    modified: None,
                    mode: None,
                    owner: None,
                    group: None,
                }))
            })
        }
        fn mkdir<'a>(&'a self, _: &'a str) -> BoxFut<'a, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn rename<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFut<'a, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn delete<'a>(&'a self, path: &'a str) -> BoxFut<'a, Result<()>> {
            Box::pin(async move {
                self.0.lock().unwrap().remove(path);
                Ok(())
            })
        }
        fn chmod<'a>(&'a self, _: &'a str, _: u32) -> BoxFut<'a, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn get<'a>(&'a self, remote: &'a str, local: &'a Path, progress: Progress<'a>) -> BoxFut<'a, Result<u64>> {
            Box::pin(async move {
                let data = self.0.lock().unwrap().get(remote).cloned().ok_or_else(|| Error::NotFound(remote.into()))?;
                let mut partial = LocalPartial::create(local).await?;
                let n = copy_chunked(&mut data.as_slice(), partial.file(), progress).await?;
                partial.finish().await?;
                Ok(n)
            })
        }
        fn put<'a>(&'a self, local: &'a Path, remote: &'a str, progress: Progress<'a>) -> BoxFut<'a, Result<u64>> {
            Box::pin(async move {
                let mut out = Vec::new();
                let n = copy_chunked(&mut tokio::fs::File::open(local).await?, &mut out, progress).await?;
                self.0.lock().unwrap().insert(remote.into(), out);
                Ok(n)
            })
        }
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rxt-sftp-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[tokio::test]
    async fn chunked_copy_reports_progress() {
        let data = vec![7u8; CHUNK * 2 + 10];
        let seen = Mutex::new(Vec::new());
        let mut out = Vec::new();
        let n = copy_chunked(&mut data.as_slice(), &mut out, &|b| seen.lock().unwrap().push(b)).await.unwrap();
        assert_eq!(n, data.len() as u64);
        assert_eq!(out, data);
        assert_eq!(*seen.lock().unwrap().last().unwrap(), n);
    }

    #[tokio::test]
    async fn partial_download_removed_on_drop_and_target_kept() {
        let dir = tmp("partial");
        let target = dir.join("f.txt");
        std::fs::write(&target, b"old").unwrap();
        {
            let mut p = LocalPartial::create(&target).await.unwrap();
            p.file().write_all(b"half").await.unwrap();
            assert!(dir.join("f.txt.rxt-partial").exists());
        } // dropped: cancelled transfer
        assert!(!dir.join("f.txt.rxt-partial").exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"old");

        let mut p = LocalPartial::create(&target).await.unwrap();
        p.file().write_all(b"new").await.unwrap();
        p.finish().await.unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert!(!dir.join("f.txt.rxt-partial").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn cancelled_get_future_leaves_no_partial_file() {
        /// Reader that yields one chunk, then never completes.
        struct Stall(bool);
        impl AsyncRead for Stall {
            fn poll_read(
                mut self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                buf: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                if self.0 {
                    return std::task::Poll::Pending;
                }
                self.0 = true;
                buf.put_slice(b"chunk");
                std::task::Poll::Ready(Ok(()))
            }
        }
        let dir = tmp("cancel");
        let target = dir.join("big.bin");
        let fut = async {
            let mut p = LocalPartial::create(&target).await?;
            copy_chunked(&mut Stall(false), p.file(), &|_| {}).await?;
            p.finish().await
        };
        assert!(tokio::time::timeout(std::time::Duration::from_millis(50), fut).await.is_err());
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none(), "partial file left behind");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn open_with_round_trip() {
        let ops = Mock::default();
        ops.0.lock().unwrap().insert("/etc/app.conf".into(), b"a=1".to_vec());
        let dir = tmp("edit");
        let mut copy = EditCopy::download(&ops, "/etc/app.conf", &dir).await.unwrap();
        assert_eq!(copy.local.file_name().unwrap(), "app.conf");
        assert_eq!(std::fs::read(&copy.local).unwrap(), b"a=1");
        assert!(!copy.changed());
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&copy.local, b"a=2").unwrap();
        assert!(copy.changed());
        copy.upload_back(&ops, &|_| {}).await.unwrap();
        assert!(!copy.changed());
        assert_eq!(ops.0.lock().unwrap()["/etc/app.conf"], b"a=2");
        assert!(ops.stat("/etc/app.conf").await.unwrap().is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
