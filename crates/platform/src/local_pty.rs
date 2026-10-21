//! Local shell in a pseudo-terminal (local-shell spec). The adapter picks the shell, starts it
//! and contains its processes; the engine bridges the blocking handles to async.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rxt_core::{Error, LaunchInfo, Result};

/// Starts local shells. `None` in [`crate::Platform::local_pty`] means no backend.
pub trait LocalPty: Send + Sync {
    /// Resolve shell, arguments, folder, user and host without starting anything.
    fn prepare(&self) -> Result<LaunchInfo>;
    /// Start `info` in a `cols`×`rows` pseudo-terminal. `cursor` is the 0-based (row, column)
    /// the shell starts at (below the banner); backends that repaint the screen use it.
    fn spawn(&self, info: &LaunchInfo, cols: u16, rows: u16, cursor: (u16, u16)) -> Result<LocalShell>;
}

/// A running local shell. Dropping the last [`PtyControl`] closes it.
pub struct LocalShell {
    pub reader: Box<dyn PtyReader>,
    pub writer: Box<dyn PtyWriter>,
    pub control: Arc<dyn PtyControl>,
}

/// Shell output. Blocks until data, end of output or [`PtyControl::close`].
pub trait PtyReader: Send {
    /// `Ok(0)` at the end of the output.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize>;
}

/// Shell input. A write blocked on a shell that does not read is ended by [`PtyControl::close`]
/// (Windows) or once no process holds the terminal any more (Linux).
pub trait PtyWriter: Send {
    fn write_all(&mut self, bytes: &[u8]) -> Result<()>;
}

pub trait PtyControl: Send + Sync {
    fn resize(&self, cols: u16, rows: u16) -> Result<()>;
    /// End the shell and the processes it started; returns without waiting for them.
    fn close(&self);
    /// Exit code once the shell ended, waiting up to `timeout`; `None` while it runs.
    fn wait_exit(&self, timeout: Duration) -> Option<u32>;
}

/// State of one [`FakePty`] shell, as seen by tests.
#[derive(Debug, Default)]
pub struct FakeShell {
    /// The launch the shell was started with (WSL passes its `wsl.exe` args here).
    pub launch: LaunchInfo,
    pub cols: u16,
    pub rows: u16,
    pub cursor: (u16, u16),
    /// Output the shell will produce.
    pub output: VecDeque<u8>,
    /// No more output after `output` drains.
    pub eof: bool,
    /// Bytes written to the shell.
    pub input: Vec<u8>,
    pub resizes: Vec<(u16, u16)>,
    pub exit: Option<u32>,
    pub closed: bool,
    /// Writes block until the shell is closed, then fail.
    pub block_writes: bool,
}

type Shared = Arc<(Mutex<FakeShell>, Condvar)>;

fn lock(s: &Shared) -> MutexGuard<'_, FakeShell> {
    s.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// In-memory local shells for tests. Clones share state.
#[derive(Clone, Default)]
pub struct FakePty {
    shells: Arc<Mutex<Vec<Shared>>>,
    /// Error returned by the next `prepare`/`spawn`.
    pub fail: Arc<Mutex<Option<String>>>,
    /// New shells block writes (see [`FakeShell::block_writes`]).
    pub block_writes: Arc<std::sync::atomic::AtomicBool>,
}

impl FakePty {
    /// Fictional launch facts used by every fake shell.
    pub fn info() -> LaunchInfo {
        LaunchInfo {
            shell: "/bin/bash".into(),
            args: vec!["-l".into()],
            cwd: "/home/alice".into(),
            user: "alice".into(),
            host: "workstation".into(),
            ..LaunchInfo::default()
        }
    }

    /// Number of shells started so far.
    pub fn spawned(&self) -> usize {
        self.shells.lock().map_or(0, |s| s.len())
    }

    /// Run `f` on shell `i` (0 = first started) and wake its readers and waiters.
    pub fn with<R>(&self, i: usize, f: impl FnOnce(&mut FakeShell) -> R) -> Option<R> {
        let shell = self.shells.lock().ok()?.get(i).cloned()?;
        let r = f(&mut lock(&shell));
        shell.1.notify_all();
        Some(r)
    }

    /// Queue output on the latest shell.
    pub fn send(&self, bytes: &[u8]) {
        let last = self.spawned().saturating_sub(1);
        self.with(last, |s| s.output.extend(bytes));
    }

    /// The latest shell exits with `code` after its queued output.
    pub fn exit(&self, code: u32) {
        let last = self.spawned().saturating_sub(1);
        self.with(last, |s| {
            s.eof = true;
            s.exit = Some(code);
        });
    }
}

impl LocalPty for FakePty {
    fn prepare(&self) -> Result<LaunchInfo> {
        match self.fail.lock().ok().and_then(|f| f.clone()) {
            Some(e) => Err(Error::Io(e)),
            None => Ok(Self::info()),
        }
    }

    fn spawn(&self, info: &LaunchInfo, cols: u16, rows: u16, cursor: (u16, u16)) -> Result<LocalShell> {
        self.prepare()?;
        let block_writes = self.block_writes.load(std::sync::atomic::Ordering::Relaxed);
        let shell = FakeShell { launch: info.clone(), cols, rows, cursor, block_writes, ..FakeShell::default() };
        let shared: Shared = Arc::new((Mutex::new(shell), Condvar::new()));
        self.shells.lock().map_err(|_| Error::Io("fake pty lock poisoned".into()))?.push(shared.clone());
        Ok(LocalShell {
            reader: Box::new(FakeIo(shared.clone())),
            writer: Box::new(FakeIo(shared.clone())),
            control: Arc::new(FakeControl(shared)),
        })
    }
}

struct FakeIo(Shared);

impl PtyReader for FakeIo {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let mut s = lock(&self.0);
        loop {
            if !s.output.is_empty() {
                let n = buf.len().min(s.output.len());
                for (b, x) in buf.iter_mut().zip(s.output.drain(..n)) {
                    *b = x;
                }
                return Ok(n);
            }
            if s.eof || s.closed {
                return Ok(0);
            }
            s = self.0.1.wait(s).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

impl PtyWriter for FakeIo {
    fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        let mut s = lock(&self.0);
        while s.block_writes && !s.closed {
            s = self.0.1.wait(s).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        if s.closed || s.exit.is_some() {
            return Err(Error::Io("shell closed".into()));
        }
        s.input.extend_from_slice(bytes);
        Ok(())
    }
}

struct FakeControl(Shared);

impl PtyControl for FakeControl {
    fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let mut s = lock(&self.0);
        (s.cols, s.rows) = (cols, rows);
        s.resizes.push((cols, rows));
        Ok(())
    }

    fn close(&self) {
        let mut s = lock(&self.0);
        s.closed = true;
        s.exit.get_or_insert(129);
        drop(s);
        self.0.1.notify_all();
    }

    fn wait_exit(&self, timeout: Duration) -> Option<u32> {
        let deadline = Instant::now() + timeout;
        let mut s = lock(&self.0);
        loop {
            if let Some(code) = s.exit {
                return Some(code);
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            s = self.0.1.wait_timeout(s, left).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

impl Drop for FakeControl {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn fake_shell_reads_writes_resizes_and_exits() {
        let pty = FakePty::default();
        let info = pty.prepare().unwrap();
        let mut sh = pty.spawn(&info, 80, 24, (3, 0)).unwrap();
        pty.send(b"$ ");
        let mut buf = [0u8; 8];
        assert_eq!(sh.reader.read(&mut buf).unwrap(), 2);
        sh.writer.write_all(b"ls\r").unwrap();
        sh.control.resize(100, 30).unwrap();
        assert_eq!(sh.control.wait_exit(Duration::from_millis(10)), None);
        pty.exit(0);
        assert_eq!(sh.reader.read(&mut buf).unwrap(), 0);
        assert_eq!(sh.control.wait_exit(Duration::ZERO), Some(0));
        pty.with(0, |s| {
            assert_eq!((s.input.as_slice(), s.cursor, s.resizes.as_slice()), (&b"ls\r"[..], (3, 0), &[(100, 30)][..]));
        });
    }

    #[test]
    fn dropping_the_control_closes_and_ends_blocked_writes() {
        let pty = FakePty::default();
        pty.block_writes.store(true, std::sync::atomic::Ordering::Relaxed);
        let LocalShell { mut writer, control, .. } = pty.spawn(&FakePty::info(), 80, 24, (0, 0)).unwrap();
        let t = std::thread::spawn(move || writer.write_all(b"x"));
        drop(control);
        assert!(t.join().unwrap().is_err());
        assert_eq!(pty.with(0, |s| s.closed), Some(true));
    }
}
