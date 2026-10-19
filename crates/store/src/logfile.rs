//! Diagnostic files (add-diagnostic-logging D2–D4): the `rustxterm-logs` folder, safe owner-only
//! opening, 50 MB rotation and a background writer with a bounded queue, so protocol tasks never
//! wait on the disk.

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};

/// App-owned subfolder of the chosen folder; files never go into the chosen folder itself.
pub const LOGS_SUBDIR: &str = "rustxterm-logs";
/// Diagnostic log file name inside [`LOGS_SUBDIR`].
pub const DIAGNOSTIC_LOG: &str = "rustxterm-diagnostic.log";
/// A file is rotated past this size; `.1` to `.3` are kept.
pub const ROTATE_AT: u64 = 50 * 1024 * 1024;
const KEEP: u32 = 3;
/// Bytes waiting for the writer thread before new records are dropped.
const QUEUE_BUDGET: usize = 4 * 1024 * 1024;

/// Opens `path` for appending, owner-only. On Unix a new file is created with `create_new` and
/// mode `0600` (never through a symlink); an existing one must be a regular file with a single link
/// and the same inode once opened (no link planted in a shared folder), and is made owner-only.
/// Windows keeps the folder's permissions.
pub fn open_private_append(path: &Path) -> io::Result<std::fs::File> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
        match std::fs::OpenOptions::new().append(true).create_new(true).mode(0o600).open(path) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            r => return r,
        }
        let refuse = |why: &str| io::Error::new(io::ErrorKind::PermissionDenied, why.to_owned());
        let before = std::fs::symlink_metadata(path)?;
        if !before.file_type().is_file() || before.nlink() != 1 {
            return Err(refuse("not a regular file"));
        }
        let file = std::fs::OpenOptions::new().append(true).open(path)?;
        let after = file.metadata()?;
        if (after.dev(), after.ino()) != (before.dev(), before.ino()) {
            return Err(refuse("the file changed while opening it"));
        }
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        Ok(file)
    }
    #[cfg(not(unix))]
    std::fs::OpenOptions::new().create(true).append(true).open(path)
}

/// The diagnostics folder: `chosen` (or the current directory) joined with [`LOGS_SUBDIR`], else
/// the same subfolder of `fallback` (the config folder). Returns the absolute folder and, after a
/// fallback, why the first choice was not used.
pub fn logs_dir(chosen: Option<&Path>, fallback: &Path) -> io::Result<(PathBuf, Option<String>)> {
    let first = match chosen {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()?,
    };
    match usable_dir(&first.join(LOGS_SUBDIR)) {
        Ok(dir) => Ok((dir, None)),
        Err(e) => {
            let dir = usable_dir(&fallback.join(LOGS_SUBDIR))?;
            Ok((dir, Some(format!("{}: {e}", first.display()))))
        }
    }
}

/// Creates `dir` (owner-only on Unix), refuses a symlink in its place and checks that it is writable.
fn usable_dir(dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.file_type().is_dir() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "not a folder"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let probe = dir.join(format!(".probe-{}", std::process::id()));
    std::fs::OpenOptions::new().write(true).create_new(true).open(&probe)?;
    std::fs::remove_file(&probe)?;
    std::path::absolute(dir)
}

/// An append-only file rotated at `limit` bytes (`.1` newest … `.3` oldest).
pub struct Rotating {
    path: PathBuf,
    file: std::fs::File,
    len: u64,
    limit: u64,
}

impl Rotating {
    pub fn open(path: &Path, limit: u64) -> io::Result<Self> {
        let file = open_private_append(path)?;
        let len = file.metadata()?.len();
        Ok(Self { path: path.to_path_buf(), file, len, limit })
    }

    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.len > 0 && self.len + bytes.len() as u64 > self.limit {
            self.rotate()?;
        }
        self.file.write_all(bytes)?;
        self.len += bytes.len() as u64;
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        let numbered = |n: u32| {
            let mut p = self.path.clone().into_os_string();
            p.push(format!(".{n}"));
            PathBuf::from(p)
        };
        let gone = |r: io::Result<()>| match r {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
        gone(std::fs::remove_file(numbered(KEEP)))?;
        for n in (1..KEEP).rev() {
            gone(std::fs::rename(numbered(n), numbered(n + 1)))?;
        }
        std::fs::rename(&self.path, numbered(1))?;
        self.file = open_private_append(&self.path)?;
        self.len = 0;
        Ok(())
    }
}

enum Msg {
    Open(u64, PathBuf),
    Data(u64, Vec<u8>),
    Close(u64),
    Sync(SyncSender<()>),
}

/// Background writer: files keyed by a number, written in order by one thread. Nothing blocks the
/// caller: the queue is unbounded but holds at most 4 MiB of records (past that a record is
/// dropped and counted, and the next record of that file says how many were lost); open and close
/// requests are tiny and never dropped. Only [`Sink::flush`] waits for the disk.
#[derive(Clone)]
pub struct Sink {
    tx: Sender<Msg>,
    queued: Arc<AtomicUsize>,
    dropped: Arc<Mutex<HashMap<u64, u64>>>,
    error: Arc<Mutex<Option<String>>>,
    /// Records dropped since the start (for tests and the page).
    pub total_dropped: Arc<AtomicU64>,
}

impl Sink {
    pub fn start(name: &str) -> io::Result<Self> {
        let (tx, rx) = channel();
        let sink = Self {
            tx,
            queued: Arc::default(),
            dropped: Arc::default(),
            error: Arc::default(),
            total_dropped: Arc::default(),
        };
        let worker = sink.clone();
        std::thread::Builder::new().name(name.into()).spawn(move || worker.run(&rx))?;
        Ok(sink)
    }

    pub fn open(&self, key: u64, path: PathBuf) {
        let _ = self.tx.send(Msg::Open(key, path));
    }

    pub fn close(&self, key: u64) {
        let _ = self.tx.send(Msg::Close(key));
    }

    /// Queues `bytes` for `key`; false when dropped (queue full). The first record queued after a
    /// drop carries a note with the number of records lost.
    pub fn write(&self, key: u64, bytes: Vec<u8>) -> bool {
        let mut dropped = self.dropped.lock().unwrap_or_else(PoisonError::into_inner);
        let lost = dropped.get(&key).copied().unwrap_or(0);
        let bytes = if lost > 0 {
            let mut b = format!("[rustXterm: {lost} records dropped, the disk was too slow]\n").into_bytes();
            b.extend_from_slice(&bytes);
            b
        } else {
            bytes
        };
        let n = bytes.len();
        let fits = self.queued.fetch_add(n, Ordering::AcqRel) + n <= QUEUE_BUDGET;
        let sent = fits && self.tx.send(Msg::Data(key, bytes)).is_ok();
        if sent {
            dropped.remove(&key);
        } else {
            self.queued.fetch_sub(n, Ordering::AcqRel);
            *dropped.entry(key).or_default() += 1;
            self.total_dropped.fetch_add(1, Ordering::Relaxed);
        }
        sent
    }

    /// Waits until everything queued so far is written.
    pub fn flush(&self) {
        let (tx, rx) = sync_channel(1);
        if self.tx.send(Msg::Sync(tx)).is_ok() {
            let _ = rx.recv();
        }
    }

    /// The last open or write error, cleared by reading it.
    pub fn take_error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    fn fail(&self, what: &str, path: &Path, e: &io::Error) {
        *self.error.lock().unwrap_or_else(PoisonError::into_inner) = Some(format!("{what} {}: {e}", path.display()));
    }

    fn run(&self, rx: &Receiver<Msg>) {
        let mut files: HashMap<u64, Rotating> = HashMap::new();
        while let Ok(msg) = rx.recv() {
            match msg {
                Msg::Open(key, path) => match Rotating::open(&path, ROTATE_AT) {
                    Ok(f) => {
                        files.insert(key, f);
                    }
                    Err(e) => {
                        files.remove(&key);
                        self.fail("cannot open", &path, &e);
                    }
                },
                Msg::Close(key) => {
                    files.remove(&key);
                }
                Msg::Data(key, bytes) => {
                    self.queued.fetch_sub(bytes.len(), Ordering::AcqRel);
                    let Some(f) = files.get_mut(&key) else { continue };
                    if let Err(e) = f.write(&bytes) {
                        let path = f.path.clone();
                        files.remove(&key);
                        self.fail("cannot write", &path, &e);
                    }
                }
                Msg::Sync(done) => {
                    let _ = done.send(());
                }
            }
        }
    }
}

/// `io::Write` over one [`Sink`] file, for a tracing `fmt` layer.
#[derive(Clone)]
pub struct SinkWriter {
    pub sink: Sink,
    pub key: u64,
}

impl Write for SinkWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.sink.write(self.key, buf.to_vec());
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rxt-logfile-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn rotation_keeps_three_files() {
        let d = tmp("rotate");
        let path = d.join("x.log");
        let mut f = Rotating::open(&path, 10).unwrap();
        for i in 0..6u8 {
            f.write(&[b'0' + i; 8]).unwrap();
        }
        assert_eq!(std::fs::read(&path).unwrap(), b"55555555");
        assert_eq!(std::fs::read(d.join("x.log.1")).unwrap(), b"44444444");
        assert_eq!(std::fs::read(d.join("x.log.3")).unwrap(), b"22222222");
        assert!(!d.join("x.log.4").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn logs_dir_falls_back_when_the_choice_is_unusable() {
        let d = tmp("fallback");
        let blocked = d.join("file-not-folder");
        std::fs::write(&blocked, b"").unwrap();
        let (dir, why) = logs_dir(Some(&blocked), &d.join("config")).unwrap();
        assert_eq!(dir, std::path::absolute(d.join("config").join(LOGS_SUBDIR)).unwrap());
        assert!(why.is_some());
        let (dir, why) = logs_dir(Some(&d), &d.join("config")).unwrap();
        assert_eq!(dir, std::path::absolute(d.join(LOGS_SUBDIR)).unwrap());
        assert!(why.is_none());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn planted_links_are_refused_and_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("links");
        let log = d.join("a.log");
        drop(open_private_append(&log).unwrap());
        std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o644)).unwrap();
        drop(open_private_append(&log).unwrap());
        assert_eq!(std::fs::metadata(&log).unwrap().permissions().mode() & 0o777, 0o600);
        let target = d.join("target");
        std::fs::write(&target, b"").unwrap();
        std::os::unix::fs::symlink(&target, d.join("sym.log")).unwrap();
        assert!(open_private_append(&d.join("sym.log")).is_err());
        std::fs::hard_link(&target, d.join("hard.log")).unwrap();
        assert!(open_private_append(&d.join("hard.log")).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn sink_writes_in_order_and_counts_drops() {
        let d = tmp("sink");
        let sink = Sink::start("test-sink").unwrap();
        sink.open(1, d.join("s.log"));
        assert!(sink.write(1, b"a\n".to_vec()));
        assert!(!sink.write(1, vec![0; QUEUE_BUDGET + 1]));
        assert!(sink.write(1, b"b\n".to_vec()));
        sink.flush();
        let text = std::fs::read_to_string(d.join("s.log")).unwrap();
        assert_eq!(text, "a\n[rustXterm: 1 records dropped, the disk was too slow]\nb\n");
        sink.close(1);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
