//! Engine: session manager, command registry, bounded command/event channels,
//! per-session byte-bounded output buffers, transfer queue, MultiExec (design D6). Owns tokio.
//!
//! Contract used by `ui` and the app (keep signatures stable; extend, don't break):
//! - [`Engine::start`] spawns the runtime and returns an [`EngineHandle`].
//! - [`EngineHandle::send`] never blocks the UI; `Command::Input` goes to the high-priority queue.
//! - [`EngineHandle::drain_output`] is called each frame, visible tab first, with a byte budget.
//! - Remote desktops (RDP/VNC): `Command::DesktopInput`/`SetVisible` take the fast path like
//!   `Input`; the UI reads pixels with [`EngineHandle::with_framebuffer`] (copy dirty rects, then
//!   `take_dirty`) and remote clipboard text with [`EngineHandle::take_clipboard`].
//!
//! - Saved credentials and trust records go through the app-injected [`Records`]
//!   ([`EngineConfig::records`]); the UI asks to save a typed secret with
//!   [`EngineHandle::save_secret`] before answering the prompt.
//!
//! Agent control (design D14, module [`control`]):
//! - [`EngineConfig::control`] is the platform control transport; [`EngineHandle::set_agent_control`]
//!   creates/removes the endpoint (Linux: `$XDG_RUNTIME_DIR/rustxterm/control.sock`) as the
//!   `agent_control` setting or `RXT_AGENT_CONTROL=1` says. Frames: 4-byte big-endian length +
//!   JSON `rxt_core::control::Request` -> `Response` (`rxt_platform::control`).
//! - Every request passes [`control::authorize`] and is audited through [`Records::store`].
//!   Under `ask`, the GUI gets `Event::ApprovalRequest` (`session` = session name) and answers
//!   with `Command::AnswerApproval`; unanswered approvals are denied after 60 s.
//! - [`EngineHandle::set_agent_workspace`] sets the only local directory agent file
//!   transfers may touch (default for the app: `<data dir>/agent-files`).
//! - Offline CLI/MCP: [`offline::apply`]`(store, client, entry, op) -> Result<Value, ControlError>`.
//! - Agent-facing session JSON: [`session_json`] (`"password": "saved" | "absent"`).
//!
//! The waker is coalesced: it fires at most once between two UI polls (`try_event` or
//! `pending_output`). The UI must keep repainting while `pending_output` is non-empty.

pub mod control;
mod files;
mod follow;
mod monitor;
pub mod offline;
mod tab;
mod tail;
#[cfg(test)]
mod tests;
mod transport;
mod workspace;

pub use control::{ControlError, Decision, authorize, session_json};

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use bytes::{Buf, BytesMut};
use parking_lot::Mutex;
use rxt_core::{
    Command, ConnState, DesktopInput, Error, Event, Framebuffer, PromptAnswer, PromptId, PromptKind, Result, Secret,
    Session, TabId, TransferId, Tunnel, TunnelState,
};
use tokio::sync::{Notify, Semaphore, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::tab::TabPrompter;
use crate::transport::{Conn, DesktopSend, OnTunnelState, Transport};

/// Concurrent transfers; the rest wait in the queue.
const TRANSFER_SLOTS: usize = 2;
/// Initial desktop size until the UI sends `DesktopInput::Resize`.
const DEFAULT_DESKTOP: (u32, u32) = (1280, 800);
/// Grace period used when the last handle is dropped without `shutdown`.
const DROP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// Persistent records injected by the app (normally the encrypted store). While the store is
/// locked an implementation returns nothing and saves nothing; trust stays in memory.
pub trait Records: Send + Sync {
    /// Saved password or key passphrase answering `kind` (`retry` is always false) for `session`.
    fn credential(&self, session: &Session, kind: &PromptKind) -> Option<Secret>;
    fn save_credential(&self, session: &Session, kind: &PromptKind, secret: &Secret);
    fn forget_credential(&self, session: &Session, kind: &PromptKind);
    /// SSH host keys (OpenSSH public-key lines) accepted with "accept and save".
    fn host_keys(&self, host: &str, port: u16) -> Vec<String>;
    fn save_host_key(&self, host: &str, port: u16, key: &str);
    /// TLS certificate SHA-256 fingerprint accepted with "accept and save".
    fn certificate(&self, host: &str, port: u16) -> Option<String>;
    fn save_certificate(&self, host: &str, port: u16, fingerprint: &str);
    /// Run `f` on the open database (agent control: lists, policies, audit). The default, and
    /// a locked store, return an error: agent requests then fail as "locked".
    fn store(&self, f: &mut dyn FnMut(&rxt_store::db::Store) -> Result<()>) -> Result<()> {
        let _ = f;
        Err(Error::Auth("the session database is locked".into()))
    }
}

/// Engine configuration.
#[derive(Clone)]
pub struct EngineConfig {
    /// Max unread output per session before the transport stops being read.
    pub output_buffer_bytes: usize,
    pub command_queue: usize,
    pub event_queue: usize,
    pub known_hosts: PathBuf,
    /// Saved credentials and trust records; `None` keeps everything in memory.
    pub records: Option<Arc<dyn Records>>,
    /// Agent control transport; `None` keeps agent control unavailable.
    pub control: Option<Arc<dyn rxt_platform::ControlTransport>>,
    /// Agent workspace for `files get`/`put`; empty disables agent file transfers.
    pub agent_files: PathBuf,
    /// Serial port access; `None` makes serial sessions fail as unsupported.
    pub serial_ports: Option<Arc<dyn rxt_platform::SerialPorts>>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            output_buffer_bytes: 4 << 20,
            command_queue: 1024,
            event_queue: 4096,
            known_hosts: PathBuf::new(),
            records: None,
            control: None,
            agent_files: PathBuf::new(),
            serial_ports: None,
        }
    }
}

/// Called after events or output are queued so the UI can request a repaint.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Entry point.
pub struct Engine;

impl Engine {
    pub fn start(config: EngineConfig, waker: Waker) -> Result<EngineHandle> {
        let transport = Arc::new(transport::Real::new(
            config.known_hosts.clone(),
            config.records.clone(),
            config.serial_ports.clone(),
        ));
        Self::start_with(config, waker, transport)
    }

    fn start_with(config: EngineConfig, waker: Waker, transport: Arc<dyn Transport>) -> Result<EngineHandle> {
        let workers = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 4));
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(workers)
            .thread_name("rxt-engine")
            .enable_all()
            .build()?;
        let (cmd_tx, cmd_rx) = mpsc::channel(config.command_queue.max(1));
        let (ev_tx, ev_rx) = mpsc::channel(config.event_queue.max(1));
        let root = CancellationToken::new();
        let shared = Arc::new(Shared {
            agent_files: Mutex::new(config.agent_files.clone()),
            config,
            transport,
            events: ev_tx,
            waker,
            wake_armed: AtomicBool::new(false),
            root: root.clone(),
            next_id: AtomicU64::new(1),
            rr: AtomicUsize::new(0),
            tabs: Mutex::new(BTreeMap::new()),
            multi: Mutex::new(MultiExec::default()),
            prompts: Mutex::new(HashMap::new()),
            save_requests: Mutex::new(HashSet::new()),
            transfers: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(HashMap::new()),
            transfer_slots: Semaphore::new(TRANSFER_SLOTS),
            approvals: Mutex::new(control::Approvals::default()),
            control_server: Mutex::new(None),
            monitor_default: AtomicBool::new(true),
            monitor_interval: Arc::new(AtomicU32::new(monitor::DEFAULT_INTERVAL_SECS)),
        });
        runtime.spawn(dispatch(shared.clone(), cmd_rx));
        Ok(EngineHandle {
            shared,
            commands: cmd_tx,
            events: Arc::new(Mutex::new(ev_rx)),
            runtime: Arc::new(RuntimeGuard { runtime: Mutex::new(Some(runtime)), root }),
        })
    }
}

/// Cheap, cloneable handle used by the UI thread.
#[derive(Clone)]
pub struct EngineHandle {
    shared: Arc<Shared>,
    commands: mpsc::Sender<Command>,
    events: Arc<Mutex<mpsc::Receiver<Event>>>,
    runtime: Arc<RuntimeGuard>,
}

impl EngineHandle {
    /// Queue a command without blocking. Fails when the engine is gone or the queue is full.
    /// `Input`/`Resize` go straight to the per-tab input queues; MultiExec changes apply immediately.
    pub fn send(&self, command: Command) -> Result<()> {
        if self.shared.root.is_cancelled() {
            return Err(stopped());
        }
        match command {
            Command::Input { tab, bytes } => self.shared.input(tab, bytes),
            Command::Resize { tab, cols, rows } => self.shared.resize(tab, cols, rows),
            Command::DesktopInput { tab, input } => self.shared.desktop_input(tab, input),
            Command::SetVisible { tab, visible } => self.shared.desktop_input(tab, DesktopInput::Visible(visible)),
            Command::SetMultiExec { enabled } => {
                self.shared.multi.lock().enabled = enabled;
                self.shared.emit_now(self.shared.multi_event());
            }
            Command::SelectMultiExec { tab, selected } => {
                let mut multi = self.shared.multi.lock();
                if selected {
                    multi.selected.insert(tab)
                } else {
                    multi.selected.remove(&tab)
                };
                drop(multi);
                self.shared.emit_now(self.shared.multi_event());
            }
            other => {
                return self.commands.try_send(other).map_err(|e| match e {
                    mpsc::error::TrySendError::Full(_) => Error::Io("engine busy: command queue full".into()),
                    mpsc::error::TrySendError::Closed(_) => stopped(),
                });
            }
        }
        Ok(())
    }

    /// Next pending event, if any.
    pub fn try_event(&self) -> Option<Event> {
        self.shared.wake_armed.store(false, Ordering::Release);
        self.events.lock().try_recv().ok()
    }

    /// Move up to `budget` buffered output bytes of `tab` into `out`; returns bytes moved.
    /// Draining frees buffer space and resumes reading the transport.
    pub fn drain_output(&self, tab: TabId, budget: usize, out: &mut Vec<u8>) -> usize {
        let buf = self.shared.tabs.lock().get(&tab).map(|t| t.out.clone());
        buf.map_or(0, |b| b.drain(budget, out))
    }

    /// Tabs with pending output, in round-robin order.
    pub fn pending_output(&self) -> Vec<TabId> {
        self.shared.wake_armed.store(false, Ordering::Release);
        let mut tabs: Vec<TabId> =
            self.shared.tabs.lock().iter().filter(|(_, t)| t.out.len() > 0).map(|(id, _)| *id).collect();
        if !tabs.is_empty() {
            let start = self.shared.rr.fetch_add(1, Ordering::Relaxed) % tabs.len();
            tabs.rotate_left(start);
        }
        tabs
    }

    /// Run `f` on the tab's remote-desktop framebuffer (`None` for unknown tabs). Keep `f` short:
    /// the desktop reader waits for it.
    pub fn with_framebuffer<R>(&self, tab: TabId, f: impl FnOnce(&mut Framebuffer) -> R) -> Option<R> {
        let frame = self.shared.tabs.lock().get(&tab).map(|t| t.frame.clone())?;
        let mut frame = frame.lock();
        Some(f(&mut frame))
    }

    /// Latest remote clipboard text of a desktop tab not yet taken by the UI.
    pub fn take_clipboard(&self, tab: TabId) -> Option<String> {
        self.shared.tabs.lock().get_mut(&tab)?.clipboard.take()
    }

    /// Ask to save the secret answering `prompt` ("save password"); it is stored only when
    /// the login succeeds. Call before sending the answer.
    pub fn save_secret(&self, prompt: PromptId) {
        self.shared.save_requests.lock().insert(prompt);
    }

    /// Create (true) or remove (false) the agent control endpoint. Idempotent. Fails when the
    /// platform has no transport or the endpoint cannot be created safely.
    pub fn set_agent_control(&self, enabled: bool) -> Result<()> {
        let mut server = self.shared.control_server.lock();
        if !enabled {
            if let Some(cancel) = server.take() {
                cancel.cancel();
            }
            return Ok(());
        }
        if server.as_ref().is_some_and(|c| !c.is_cancelled()) {
            return Ok(());
        }
        let transport = self.shared.config.control.clone().ok_or_else(|| Error::Unsupported("agent control".into()))?;
        let handle = self.runtime.runtime.lock().as_ref().map(|r| r.handle().clone()).ok_or_else(stopped)?;
        let _enter = handle.enter();
        let listener = transport.listen()?;
        let cancel = self.shared.root.child_token();
        handle.spawn(control::serve(self.shared.clone(), listener, cancel.clone()));
        *server = Some(cancel);
        Ok(())
    }

    /// Whether the agent control endpoint is up.
    pub fn agent_control_enabled(&self) -> bool {
        self.shared.control_server.lock().as_ref().is_some_and(|c| !c.is_cancelled())
    }

    /// Latest remote monitoring sample of `tab`, taken by the UI only while it paints the
    /// tab's bar; taking it lets the collector request the next one (design M3).
    pub fn take_metrics(&self, tab: TabId) -> Option<rxt_core::MetricsSample> {
        let mon = self.shared.tabs.lock().get(&tab).map(|t| t.monitor.clone())?;
        mon.take()
    }

    /// Global "Remote monitoring" setting, used by tabs connecting afterwards whose session
    /// does not override it (default on).
    pub fn set_remote_monitoring(&self, enabled: bool) {
        self.shared.monitor_default.store(enabled, Ordering::Relaxed);
    }

    /// Set the agent workspace directory (GUI only).
    pub fn set_agent_workspace(&self, dir: &Path) {
        *self.shared.agent_files.lock() = dir.to_owned();
    }

    /// Stop all sessions and the runtime; returns once shut down or after `timeout`.
    /// Call from a non-async thread (the UI thread).
    pub fn shutdown(&self, timeout: std::time::Duration) {
        self.runtime.shutdown(timeout);
    }
}

fn stopped() -> Error {
    Error::Io("engine stopped".into())
}

struct RuntimeGuard {
    runtime: Mutex<Option<tokio::runtime::Runtime>>,
    root: CancellationToken,
}

impl RuntimeGuard {
    fn shutdown(&self, timeout: std::time::Duration) {
        // Cancelling first wakes every reader, prompt and transfer; dropped tasks end the rest.
        self.root.cancel();
        let runtime = self.runtime.lock().take();
        if let Some(runtime) = runtime {
            runtime.shutdown_timeout(timeout);
        }
    }
}

impl Drop for RuntimeGuard {
    fn drop(&mut self) {
        self.shutdown(DROP_TIMEOUT);
    }
}

/// Engine state shared by the handle and the runtime tasks.
struct Shared {
    config: EngineConfig,
    transport: Arc<dyn Transport>,
    events: mpsc::Sender<Event>,
    waker: Waker,
    wake_armed: AtomicBool,
    root: CancellationToken,
    next_id: AtomicU64,
    rr: AtomicUsize,
    tabs: Mutex<BTreeMap<TabId, Tab>>,
    multi: Mutex<MultiExec>,
    prompts: Mutex<HashMap<PromptId, oneshot::Sender<PromptAnswer>>>,
    /// Prompts whose secret answer the user asked to save.
    save_requests: Mutex<HashSet<PromptId>>,
    transfers: Mutex<HashMap<TransferId, CancellationToken>>,
    tunnels: Mutex<HashMap<String, CancellationToken>>,
    transfer_slots: Semaphore,
    approvals: Mutex<control::Approvals>,
    /// Cancels the running control server; `None` when agent control is off.
    control_server: Mutex<Option<CancellationToken>>,
    agent_files: Mutex<PathBuf>,
    /// Global remote monitoring setting for tabs whose session does not override it.
    monitor_default: AtomicBool,
    /// Remote monitoring interval, seconds (1–30), read live by every collector.
    monitor_interval: Arc<AtomicU32>,
}

#[derive(Default)]
struct MultiExec {
    enabled: bool,
    selected: BTreeSet<TabId>,
}

struct Tab {
    session: Session,
    out: Arc<OutBuf>,
    /// Tab lifetime.
    cancel: CancellationToken,
    /// Current run (connection + auto-reconnects); replaced on manual reconnect.
    run: CancellationToken,
    /// Run generation; stale runs never touch the tab.
    generation: u64,
    conn: Option<Arc<Conn>>,
    input: Option<mpsc::Sender<InputMsg>>,
    size: (u16, u16),
    /// Remote desktop (RDP/VNC) state; unused by terminal and file tabs.
    frame: Arc<Mutex<Framebuffer>>,
    desktop: Option<DesktopSend>,
    /// Last requested desktop size; the initial RDP size on (re)connect.
    desktop_size: (u32, u32),
    visible: bool,
    clipboard: Option<String>,
    /// Last emitted connection state (agent `tabs list`).
    state: ConnState,
    /// Follow the terminal folder ([`follow`]); reset from the session on each run.
    follow: bool,
    /// Wakes the follow task of the current SSH run (`true`: report even when unchanged).
    follow_kick: Option<mpsc::Sender<bool>>,
    /// Remote monitoring control and latest-sample slot ([`monitor`]); enabled state reset
    /// from the settings on each run.
    monitor: Arc<monitor::Monitor>,
}

enum InputMsg {
    Bytes(Vec<u8>),
    Resize(u16, u16),
}

/// Unread output of one tab, bounded by `cap` (may exceed it by one transport chunk).
struct OutBuf {
    data: Mutex<BytesMut>,
    /// Plain-text tail for agent `read`/`wait`, fed alongside the UI buffer.
    tail: Mutex<tail::Tail>,
    space: Notify,
    cap: usize,
}

impl OutBuf {
    fn new(cap: usize) -> Self {
        Self { data: Mutex::new(BytesMut::new()), tail: Mutex::default(), space: Notify::new(), cap: cap.max(1) }
    }

    fn len(&self) -> usize {
        self.data.lock().len()
    }

    /// Append; true when the buffer was empty (first output since the last full drain).
    fn push(&self, bytes: &[u8]) -> bool {
        let mut data = self.data.lock();
        let was_empty = data.is_empty();
        data.extend_from_slice(bytes);
        self.tail.lock().feed(bytes);
        was_empty
    }

    fn drain(&self, budget: usize, out: &mut Vec<u8>) -> usize {
        let mut data = self.data.lock();
        let n = budget.min(data.len());
        out.extend_from_slice(&data[..n]);
        data.advance(n);
        drop(data);
        if n > 0 {
            self.space.notify_one();
        }
        n
    }
}

impl Shared {
    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn wake(&self) {
        if !self.wake_armed.swap(true, Ordering::AcqRel) {
            (self.waker)();
        }
    }

    /// Emit an event, waiting for queue space (backpressure on engine tasks, never on the UI).
    async fn emit(&self, event: Event) {
        if self.events.send(event).await.is_ok() {
            self.wake();
        }
    }

    /// Non-blocking emit for sync contexts; drops the event when the queue is full.
    fn emit_now(&self, event: Event) {
        match self.events.try_send(event) {
            Ok(()) => self.wake(),
            Err(e) => tracing::warn!("engine event dropped: {e}"),
        }
    }

    fn multi_event(&self) -> Event {
        let multi = self.multi.lock();
        Event::MultiExec { enabled: multi.enabled, selected: multi.selected.iter().copied().collect() }
    }

    async fn set_state(&self, tab: TabId, generation: u64, state: ConnState) {
        let current = match self.tabs.lock().get_mut(&tab).filter(|t| t.generation == generation) {
            Some(t) => {
                t.state = state.clone();
                true
            }
            None => false,
        };
        if current {
            self.emit(Event::State { tab, state }).await;
        }
    }

    /// Store the live connection of the current run; returns the tab's output buffer.
    fn set_conn(
        &self,
        tab: TabId,
        generation: u64,
        conn: Arc<Conn>,
        input: Option<mpsc::Sender<InputMsg>>,
    ) -> Option<Arc<OutBuf>> {
        let mut tabs = self.tabs.lock();
        let t = tabs.get_mut(&tab).filter(|t| t.generation == generation)?;
        t.conn = Some(conn);
        t.input = input;
        Some(t.out.clone())
    }

    fn clear_conn(&self, tab: TabId, generation: u64) {
        if let Some(t) = self.tabs.lock().get_mut(&tab).filter(|t| t.generation == generation) {
            t.conn = None;
            t.input = None;
            t.desktop = None;
            t.follow_kick = None;
        }
    }

    /// Whether the tab follows its terminal folder.
    fn follows(&self, tab: TabId) -> bool {
        self.tabs.lock().get(&tab).is_some_and(|t| t.follow)
    }

    /// Turn following on or off; turning it on checks the folder right away.
    fn set_follow(&self, tab: TabId, enabled: bool) {
        if let Some(t) = self.tabs.lock().get_mut(&tab) {
            t.follow = enabled;
            if let (true, Some(kick)) = (enabled, &t.follow_kick) {
                let _ = kick.try_send(true);
            }
        }
    }

    fn size(&self, tab: TabId) -> (u16, u16) {
        self.tabs.lock().get(&tab).map_or((80, 24), |t| t.size)
    }

    /// Route input to the tab, or to every selected tab when MultiExec is on and `tab` is selected.
    fn input(&self, tab: TabId, bytes: Vec<u8>) {
        let targets: Vec<TabId> = {
            let multi = self.multi.lock();
            if multi.enabled && multi.selected.contains(&tab) {
                multi.selected.iter().copied().collect()
            } else {
                vec![tab]
            }
        };
        let enter = bytes.iter().any(|b| matches!(b, b'\r' | b'\n'));
        let mut dropped = Vec::new();
        {
            let tabs = self.tabs.lock();
            for target in targets {
                let Some(t) = tabs.get(&target) else { continue };
                let Some(tx) = t.input.as_ref() else { continue };
                if tx.try_send(InputMsg::Bytes(bytes.clone())).is_err() {
                    dropped.push(target);
                } else if let Some(kick) = t.follow_kick.as_ref().filter(|_| t.follow && enter) {
                    // Full queue: a check is already pending.
                    let _ = kick.try_send(false);
                }
            }
        }
        for tab in dropped {
            self.emit_now(Event::Notice {
                tab: Some(tab),
                message: "input dropped: session is not accepting input".into(),
            });
        }
    }

    fn resize(&self, tab: TabId, cols: u16, rows: u16) {
        if let Some(t) = self.tabs.lock().get_mut(&tab) {
            t.size = (cols, rows);
            if let Some(tx) = &t.input {
                // Latest size is kept in `t.size`; a dropped resize is re-sent on reconnect.
                let _ = tx.try_send(InputMsg::Resize(cols, rows));
            }
        }
    }

    /// Forward desktop input without blocking; remembers the size and visibility for reconnects.
    fn desktop_input(&self, tab: TabId, input: DesktopInput) {
        let send = {
            let mut tabs = self.tabs.lock();
            let Some(t) = tabs.get_mut(&tab) else { return };
            match input {
                DesktopInput::Resize { width, height } => t.desktop_size = (width, height),
                DesktopInput::Visible(v) => t.visible = v,
                _ => {}
            }
            t.desktop.clone()
        };
        if let Some(Err(e)) = send.map(|send| send(input)) {
            tracing::debug!(?tab, "desktop input dropped: {e}");
        }
    }

    /// Ask the user; resolves to `Cancel` when `cancel` fires first. The flag tells whether the
    /// user asked to save the answer.
    async fn ask(&self, tab: Option<TabId>, kind: PromptKind, cancel: &CancellationToken) -> (PromptAnswer, bool) {
        let id = PromptId(self.next_id());
        let (tx, rx) = oneshot::channel();
        self.prompts.lock().insert(id, tx);
        // Removes the pending entry however this future ends, including when it is dropped.
        let _pending = PendingPrompt(self, id);
        self.emit(Event::Prompt { id, tab, kind }).await;
        let answer = tokio::select! {
            a = rx => a.unwrap_or(PromptAnswer::Cancel),
            () = cancel.cancelled() => PromptAnswer::Cancel,
        };
        (answer, self.save_requests.lock().remove(&id))
    }

    /// Spawn a task whose panic is reported instead of propagating. With `run`, a panic
    /// also turns that tab run into an `Error` state.
    fn spawn_guarded(
        self: &Arc<Self>,
        tab: Option<TabId>,
        run: Option<u64>,
        task: impl Future<Output = ()> + Send + 'static,
    ) {
        let handle = tokio::spawn(task);
        let sh = self.clone();
        tokio::spawn(async move {
            let Err(e) = handle.await else { return };
            if !e.is_panic() {
                return;
            }
            tracing::error!(?tab, "engine task panicked");
            if let (Some(tab), Some(generation)) = (tab, run) {
                sh.clear_conn(tab, generation);
                sh.set_state(tab, generation, ConnState::Error("internal error: session task panicked".into())).await;
            }
            sh.emit(Event::Notice { tab, message: "internal error: a background task panicked".into() }).await;
        });
    }

    async fn open(self: &Arc<Self>, session: Session) -> TabId {
        let tab = TabId(self.next_id());
        let cancel = self.root.child_token();
        let entry = Tab {
            session: session.clone(),
            out: Arc::new(OutBuf::new(self.config.output_buffer_bytes)),
            run: cancel.child_token(),
            cancel,
            generation: 0,
            conn: None,
            input: None,
            size: (80, 24),
            frame: Arc::default(),
            desktop: None,
            desktop_size: DEFAULT_DESKTOP,
            visible: true,
            clipboard: None,
            state: ConnState::Connecting,
            follow: session.ssh.follow_terminal_folder,
            follow_kick: None,
            monitor: Arc::new(monitor::Monitor::new(self.monitor_interval.clone())),
        };
        self.tabs.lock().insert(tab, entry);
        self.emit(Event::TabOpened { tab, session: Box::new(session) }).await;
        self.start_run(tab);
        tab
    }

    /// Stop a running tunnel; false when none had that name.
    async fn stop_tunnel(&self, name: String) -> bool {
        let token = self.tunnels.lock().remove(&name);
        let Some(token) = token else { return false };
        token.cancel();
        self.emit(Event::TunnelState { name, state: TunnelState::Stopped }).await;
        true
    }

    /// Start (or restart) the tab's connection; any previous run is cancelled and ignored.
    fn start_run(self: &Arc<Self>, tab: TabId) {
        let mut tabs = self.tabs.lock();
        let Some(t) = tabs.get_mut(&tab) else { return };
        t.run.cancel();
        t.run = t.cancel.child_token();
        t.generation += 1;
        t.conn = None;
        t.input = None;
        t.desktop = None;
        t.follow = t.session.ssh.follow_terminal_folder;
        t.follow_kick = None;
        // Decided at connect: the session's choice overrides the global setting (design M5).
        let monitored = t.session.ssh.remote_monitoring.unwrap_or(self.monitor_default.load(Ordering::Relaxed));
        t.monitor = Arc::new(monitor::Monitor::new(self.monitor_interval.clone()));
        t.monitor.set_enabled(monitored);
        let (generation, run, session) = (t.generation, t.run.clone(), t.session.clone());
        drop(tabs);
        self.spawn_guarded(Some(tab), Some(generation), tab::run(self.clone(), tab, generation, session, run));
    }

    async fn close(&self, tab: TabId) {
        let removed = self.tabs.lock().remove(&tab);
        let Some(t) = removed else { return };
        t.cancel.cancel();
        let deselected = self.multi.lock().selected.remove(&tab);
        if deselected {
            self.emit(self.multi_event()).await;
        }
        self.emit(Event::Closed { tab }).await;
    }

    async fn start_tunnel(self: &Arc<Self>, tunnel: Tunnel, session: Option<Box<Session>>) {
        let name = tunnel.name.clone();
        // A tunnel rides on a connected tab of its session (stopping with it); without one,
        // it opens a dedicated connection that lives until the tunnel is stopped.
        let found = self.tabs.lock().values().find_map(|t| {
            let link = t.conn.as_ref().and_then(|c| c.link.clone()).filter(|_| t.session.id == tunnel.via)?;
            Some((link, t.run.child_token()))
        });
        let (conn, cancel) = match (found, session) {
            (Some(found), _) => found,
            (None, Some(session)) => {
                let cancel = self.root.child_token();
                // Register before connecting so a Stop sent while prompts are pending cancels it.
                if let Some(old) = self.tunnels.lock().insert(name.clone(), cancel.clone()) {
                    old.cancel();
                }
                let prompter = Arc::new(TabPrompter::new(self.clone(), None, &session, cancel.clone()));
                self.emit(Event::TunnelState { name: name.clone(), state: TunnelState::Starting }).await;
                match self.transport.connect(*session, prompter.clone(), cancel.clone()).await {
                    Ok(link) => {
                        prompter.commit();
                        (link, cancel)
                    }
                    Err(e) => {
                        let state = TunnelState::Error(e.to_string());
                        return self.emit(Event::TunnelState { name, state }).await;
                    }
                }
            }
            (None, None) => {
                let state = TunnelState::Error("connect a tab of this session first".into());
                return self.emit(Event::TunnelState { name, state }).await;
            }
        };
        if cancel.is_cancelled() {
            return self.emit(Event::TunnelState { name, state: TunnelState::Stopped }).await;
        }
        if let Some(old) = self.tunnels.lock().insert(name.clone(), cancel.clone())
            && old != cancel
        {
            old.cancel();
        }
        self.emit(Event::TunnelState { name: name.clone(), state: TunnelState::Starting }).await;
        let sh = self.clone();
        self.spawn_guarded(None, None, async move {
            let reporter = sh.clone();
            let label = name.clone();
            let on_state: OnTunnelState = Arc::new(move |state: TunnelState| {
                reporter.emit_now(Event::TunnelState { name: label.clone(), state })
            });
            if let Err(e) = conn.start_tunnel(&tunnel, cancel, on_state).await {
                sh.emit(Event::TunnelState { name, state: TunnelState::Error(e.to_string()) }).await;
            }
        });
    }

    /// Serial line control of a connected serial tab.
    fn line_control(&self, tab: TabId) -> Result<Arc<dyn transport::LineControl>> {
        let tabs = self.tabs.lock();
        tabs.get(&tab)
            .and_then(|t| t.conn.as_ref()?.link.as_ref()?.line_control())
            .ok_or_else(|| Error::Invalid(format!("tab {} is not a connected serial session", tab.0)))
    }

    /// Run a line-control action in the background; failures become a tab notice.
    fn serial_control(
        self: &Arc<Self>,
        tab: TabId,
        action: impl FnOnce(Arc<dyn transport::LineControl>) -> rxt_core::BoxFut<'static, Result<()>> + Send + 'static,
    ) {
        let sh = self.clone();
        self.spawn_guarded(Some(tab), None, async move {
            let result = match sh.line_control(tab) {
                Ok(lc) => action(lc).await,
                Err(e) => Err(e),
            };
            if let Err(e) = result {
                sh.emit(Event::Notice { tab: Some(tab), message: e.to_string() }).await;
            }
        });
    }

    async fn list_serial_ports(&self) -> Result<Vec<rxt_core::PortInfo>> {
        let ports = self
            .config
            .serial_ports
            .clone()
            .ok_or_else(|| Error::Unsupported("serial ports on this platform".into()))?;
        tokio::task::spawn_blocking(move || ports.list()).await.map_err(|e| Error::Io(e.to_string()))?
    }

    async fn handle(self: &Arc<Self>, cmd: Command) {
        match cmd {
            Command::Open { session } => {
                self.open(*session).await;
            }
            Command::Close { tab } => self.close(tab).await,
            Command::Reconnect { tab } => self.start_run(tab),
            Command::Duplicate { tab } => {
                let session = self.tabs.lock().get(&tab).map(|t| t.session.clone());
                if let Some(session) = session {
                    self.open(session).await;
                }
            }
            Command::Answer { prompt, answer } => {
                if let Some(tx) = self.prompts.lock().remove(&prompt) {
                    let _ = tx.send(answer);
                }
            }
            cmd @ (Command::List { tab, .. }
            | Command::Mkdir { tab, .. }
            | Command::Rename { tab, .. }
            | Command::Delete { tab, .. }
            | Command::Chmod { tab, .. }) => {
                self.spawn_guarded(Some(tab), None, files::file_command(self.clone(), tab, cmd));
            }
            Command::Transfer { tab, direction, remote, local } => {
                let cancel = self.tabs.lock().get(&tab).map(|t| t.cancel.child_token());
                let Some(cancel) = cancel else {
                    return self.emit(Event::Notice { tab: Some(tab), message: "tab is closed".into() }).await;
                };
                let id = TransferId(self.next_id());
                self.transfers.lock().insert(id, cancel.clone());
                let job = files::Job { id, tab, direction, remote, local: PathBuf::from(local) };
                self.spawn_guarded(Some(tab), None, files::transfer(self.clone(), job, cancel));
            }
            Command::SetFollowCwd { tab, enabled } => self.set_follow(tab, enabled),
            Command::SetMonitoring { tab, enabled } => {
                if let Some(t) = self.tabs.lock().get(&tab) {
                    t.monitor.set_enabled(enabled);
                }
            }
            Command::SetMonitorInterval { secs } => {
                self.monitor_interval.store(monitor::clamp_interval(secs), Ordering::Relaxed);
                for t in self.tabs.lock().values() {
                    t.monitor.kick();
                }
            }
            Command::CancelTransfer { id } => {
                if let Some(cancel) = self.transfers.lock().remove(&id) {
                    cancel.cancel();
                }
            }
            // A dedicated tunnel connection waits for prompt answers, which arrive on this loop.
            Command::StartTunnel { tunnel, session } => {
                let sh = self.clone();
                self.spawn_guarded(None, None, async move { sh.start_tunnel(tunnel, session).await });
            }
            Command::StopTunnel { name } => {
                self.stop_tunnel(name).await;
            }
            Command::SerialBreak { tab, ms } => {
                self.serial_control(tab, move |lc| Box::pin(async move { lc.send_break(ms).await }));
            }
            Command::SerialLines { tab, dtr, rts } => {
                self.serial_control(tab, move |lc| Box::pin(async move { lc.set_lines(dtr, rts).await }));
            }
            Command::ListSerialPorts => {
                let sh = self.clone();
                self.spawn_guarded(None, None, async move {
                    let result = sh.list_serial_ports().await;
                    sh.emit(Event::SerialPorts { result }).await;
                });
            }
            // Only the GUI produces commands; agents never reach this channel.
            Command::AnswerApproval { id, approve } => self.approvals.lock().answer(id, approve),
            Command::Shutdown => self.root.cancel(),
            // Applied directly by `EngineHandle::send`.
            Command::Input { .. }
            | Command::Resize { .. }
            | Command::SetMultiExec { .. }
            | Command::SelectMultiExec { .. }
            | Command::DesktopInput { .. }
            | Command::SetVisible { .. } => {}
        }
    }
}

struct PendingPrompt<'a>(&'a Shared, PromptId);

impl Drop for PendingPrompt<'_> {
    fn drop(&mut self) {
        self.0.prompts.lock().remove(&self.1);
        self.0.save_requests.lock().remove(&self.1);
    }
}

async fn dispatch(sh: Arc<Shared>, mut commands: mpsc::Receiver<Command>) {
    loop {
        let cmd = tokio::select! {
            () = sh.root.cancelled() => break,
            cmd = commands.recv() => match cmd {
                Some(cmd) => cmd,
                None => break,
            },
        };
        sh.handle(cmd).await;
    }
    sh.root.cancel();
}
