//! Local terminal tabs (local-shell spec, design decisions 2 and 5): start sequence and the link
//! bridging the platform's blocking pseudo-terminal handles to the shell pumps.
//!
//! Start: `prepare` → `Event::LocalReady` (the UI writes the banner) → `Command::LocalStart`
//! (cursor and size) → `spawn`. Nothing starts when the tab closes before; a shell started while
//! the tab closed is dropped, which closes it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use rxt_core::{BoxFut, Error, Event, FileOps, Result, Session, TabId, Tunnel};
use rxt_platform::{LocalShell, PtyControl};
use rxt_ssh::ShellMsg;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::Shared;
use crate::transport::{Link, OnTunnelState, ShellPair, ShellRead, ShellWrite};

/// Output chunks buffered between the reader thread and the pump.
const READ_CHUNKS: usize = 2;
const READ_BUF: usize = 8192;
/// Input messages queued for the writer thread.
const WRITE_QUEUE: usize = 64;
/// Exit polling step of the waiter thread.
const EXIT_POLL: Duration = Duration::from_millis(250);
/// Output still read after the shell exited (a child may keep the terminal open).
const FINAL_OUTPUT: Duration = Duration::from_millis(200);

/// Start sequence of one run; `Err(Cancelled)` when the tab closed before the shell started.
pub(crate) async fn start(
    sh: &Arc<Shared>,
    tab: TabId,
    generation: u64,
    cancel: &CancellationToken,
) -> Result<Arc<dyn Link>> {
    let pty =
        sh.config.local_pty.clone().ok_or_else(|| Error::Unsupported("local terminals on this platform".into()))?;
    let p = pty.clone();
    let info = tokio::select! {
        () = cancel.cancelled() => return Err(Error::Cancelled),
        r = tokio::task::spawn_blocking(move || p.prepare()) => r.map_err(|e| Error::Io(e.to_string()))??,
    };
    let (tx, rx) = oneshot::channel();
    {
        let mut tabs = sh.tabs.lock();
        let t = tabs.get_mut(&tab).filter(|t| t.generation == generation).ok_or(Error::Cancelled)?;
        t.local_start = Some(tx);
    }
    sh.emit(Event::LocalReady { tab, info: Box::new(info.clone()) }).await;
    let (cols, rows, cursor) = tokio::select! {
        () = cancel.cancelled() => return Err(Error::Cancelled),
        r = rx => r.map_err(|_| Error::Cancelled)?,
    };
    // A shell finishing after a cancel is dropped with the task result, which closes it.
    let shell = tokio::select! {
        () = cancel.cancelled() => return Err(Error::Cancelled),
        r = tokio::task::spawn_blocking(move || pty.spawn(&info, cols, rows, cursor)) => {
            r.map_err(|e| Error::Io(e.to_string()))??
        }
    };
    Ok(Arc::new(LocalLink(Mutex::new(Some(shell)))))
}

struct LocalLink(Mutex<Option<LocalShell>>);

impl Link for LocalLink {
    fn shell<'a>(&'a self, _: &'a Session, _: u16, _: u16) -> BoxFut<'a, Result<ShellPair>> {
        let shell = self.0.lock().take();
        Box::pin(async move {
            let shell = shell.ok_or_else(|| Error::Invalid("local shell already started".into()))?;
            pair(shell)
        })
    }

    fn file_ops(&self) -> BoxFut<'_, Result<Arc<dyn FileOps>>> {
        Box::pin(async { Err(Error::Unsupported("file browsing in a local terminal".into())) })
    }

    fn start_tunnel<'a>(&'a self, _: &'a Tunnel, _: CancellationToken, _: OnTunnelState) -> BoxFut<'a, Result<()>> {
        Box::pin(async { Err(Error::Unsupported("tunnels over a local terminal".into())) })
    }
}

/// Reader, writer and exit-waiter threads around the blocking handles.
fn pair(shell: LocalShell) -> Result<ShellPair> {
    let LocalShell { mut reader, mut writer, control } = shell;
    let spawn = |name: &str, f: Box<dyn FnOnce() + Send>| {
        std::thread::Builder::new().name(name.into()).spawn(f).map(drop).map_err(|e| Error::Io(e.to_string()))
    };
    let (data_tx, data_rx) = mpsc::channel(READ_CHUNKS);
    spawn(
        "rxt-local-read",
        Box::new(move || {
            let mut buf = vec![0u8; READ_BUF];
            // Blocks while the pump is not pulling (output backpressure).
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || data_tx.blocking_send(buf[..n].to_vec()).is_err() {
                    return;
                }
            }
        }),
    )?;
    // Never joined: a write blocked on a shell that does not read ends when the platform closes it.
    let (in_tx, mut in_rx) = mpsc::channel::<Vec<u8>>(WRITE_QUEUE);
    spawn(
        "rxt-local-write",
        Box::new(move || {
            while let Some(bytes) = in_rx.blocking_recv() {
                if writer.write_all(&bytes).is_err() {
                    return;
                }
            }
        }),
    )?;
    let stop = Arc::new(AtomicBool::new(false));
    let (exit_tx, exit_rx) = oneshot::channel();
    let (waiter, stopped) = (control.clone(), stop.clone());
    spawn(
        "rxt-local-wait",
        Box::new(move || {
            while !stopped.load(Ordering::Acquire) {
                if let Some(code) = waiter.wait_exit(EXIT_POLL) {
                    let _ = exit_tx.send(code);
                    return;
                }
            }
        }),
    )?;
    let read = LocalRead { data: data_rx, exit: exit_rx, state: ReadState::Running, control: control.clone(), stop };
    Ok((Box::new(read), Box::new(LocalWrite { input: in_tx, control })))
}

enum ReadState {
    Running,
    /// Shell exited with this code: pass on what is already buffered, then report it.
    Draining(u32, tokio::time::Instant),
    Done,
}

struct LocalRead {
    data: mpsc::Receiver<Vec<u8>>,
    exit: oneshot::Receiver<u32>,
    state: ReadState,
    control: Arc<dyn PtyControl>,
    stop: Arc<AtomicBool>,
}

impl ShellRead for LocalRead {
    fn read(&mut self) -> BoxFut<'_, Option<ShellMsg>> {
        Box::pin(async move {
            loop {
                match self.state {
                    ReadState::Running => {
                        tokio::select! {
                            biased;
                            d = self.data.recv() => match d {
                                Some(bytes) => return Some(ShellMsg::Data(bytes)),
                                // End of output: the exit code follows.
                                None => {
                                    let code = tokio::time::timeout(Duration::from_secs(2), &mut self.exit).await;
                                    self.state = ReadState::Done;
                                    self.control.close();
                                    return code.ok().and_then(Result::ok).map(ShellMsg::ExitStatus);
                                }
                            },
                            code = &mut self.exit => {
                                let code = code.unwrap_or(1);
                                self.state = ReadState::Draining(code, tokio::time::Instant::now() + FINAL_OUTPUT);
                            }
                        }
                    }
                    ReadState::Draining(code, until) => match tokio::time::timeout_at(until, self.data.recv()).await {
                        Ok(Some(bytes)) => return Some(ShellMsg::Data(bytes)),
                        _ => {
                            self.state = ReadState::Done;
                            self.control.close();
                            return Some(ShellMsg::ExitStatus(code));
                        }
                    },
                    ReadState::Done => return None,
                }
            }
        })
    }
}

impl Drop for LocalRead {
    /// The run ended (tab closed, reconnect, exit): end the shell and the waiter.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.control.close();
    }
}

struct LocalWrite {
    input: mpsc::Sender<Vec<u8>>,
    control: Arc<dyn PtyControl>,
}

impl ShellWrite for LocalWrite {
    fn write<'a>(&'a self, bytes: &'a [u8]) -> BoxFut<'a, Result<()>> {
        Box::pin(async move {
            self.input.send(bytes.to_vec()).await.map_err(|_| Error::Io("local shell input closed".into()))
        })
    }

    fn resize(&self, cols: u16, rows: u16) -> BoxFut<'_, Result<()>> {
        let r = self.control.resize(cols, rows);
        Box::pin(async move { r })
    }
}
