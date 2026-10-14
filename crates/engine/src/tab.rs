//! Per-tab session lifecycle: connect, shell pumps with backpressure, keep-open and auto-reconnect.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use rxt_core::{
    BoxFut, ConnState, DesktopEvent, DesktopInput, Error, Event, PromptAnswer, PromptKind, Protocol, Secret, Session,
    TabId,
};
use rxt_ssh::{Prompter, ShellMsg};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::transport::{Conn, DesktopRead, ShellRead, ShellWrite};
use crate::{InputMsg, OutBuf, Shared};

/// Per-tab input queue length (messages, not bytes).
const INPUT_QUEUE: usize = 1024;
/// Longest wait for the SFTP channel opened at login for the banner.
const SFTP_PROBE: Duration = Duration::from_secs(5);
/// Auto-reconnect delay cap.
const MAX_BACKOFF_SECS: u64 = 60;

enum End {
    /// Remote command finished without "keep open": the tab closes.
    Close,
    /// Shell exited normally: tab stays, no auto-reconnect.
    Exited(String),
    /// Transport lost: auto-reconnect applies.
    Dropped(String),
    /// Serial device removed: reopened when it returns (design S4).
    Gone,
    Failed(Error),
}

/// Supervises one tab run (one connection plus its automatic reconnects).
pub(crate) async fn run(sh: Arc<Shared>, tab: TabId, generation: u64, session: Session, cancel: CancellationToken) {
    let mut session = session;
    // Serial: the device as listed at open, to find it again after an unplug (S4).
    let configured = session.serial.port.clone();
    let last = serial_port_info(&sh, &session).await;
    let mut replugging = false;
    let mut retries = 0u32;
    loop {
        if !replugging {
            sh.set_state(tab, generation, ConnState::Connecting).await;
        }
        let (end, connected) = connect_and_run(&sh, tab, generation, &session, &cancel).await;
        sh.clear_conn(tab, generation);
        if cancel.is_cancelled() {
            return;
        }
        if connected {
            retries = 0;
            replugging = false;
        }
        // While waiting for a replugged device, a failed reopen just means "not back yet".
        let away = |e: &Error| matches!(e, Error::Busy(_) | Error::NotFound(_) | Error::Gone(_) | Error::Io(_));
        let end = match end {
            End::Failed(e) if replugging && away(&e) => End::Gone,
            end => end,
        };
        let reason = match end {
            End::Close => return sh.close(tab).await,
            End::Exited(reason) => {
                return sh.set_state(tab, generation, ConnState::Disconnected { reason, reconnect_at: None }).await;
            }
            // Only retry connection-level failures, and only once a reconnect cycle started.
            End::Failed(e) if retries > 0 && matches!(e, Error::Connection(_) | Error::Io(_)) => e.to_string(),
            End::Failed(e) => return sh.set_state(tab, generation, ConnState::Error(e.to_string())).await,
            End::Dropped(reason) => reason,
            End::Gone => {
                if !replugging {
                    let reason = "device removed".to_string();
                    sh.set_state(tab, generation, ConnState::Disconnected { reason, reconnect_at: None }).await;
                }
                if !session.serial.reconnect {
                    return;
                }
                replugging = true;
                match next_port(&sh, &configured, last.as_ref(), &cancel).await {
                    Some(path) => session.serial.port = path,
                    None => return,
                }
                continue;
            }
        };
        if !auto_reconnect(&session) {
            return sh.set_state(tab, generation, ConnState::Disconnected { reason, reconnect_at: None }).await;
        }
        let delay = Duration::from_secs((2u64 << retries.min(5)).min(MAX_BACKOFF_SECS));
        retries += 1;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let at = i64::try_from(now + delay.as_secs()).ok();
        sh.set_state(tab, generation, ConnState::Disconnected { reason, reconnect_at: at }).await;
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(delay) => {}
        }
    }
}

/// Returns how the run ended and whether it reached `Connected`.
async fn connect_and_run(
    sh: &Arc<Shared>,
    tab: TabId,
    generation: u64,
    session: &Session,
    cancel: &CancellationToken,
) -> (End, bool) {
    let tab_prompter = Arc::new(TabPrompter::new(sh.clone(), Some(tab), session, cancel.clone()));
    let prompter: Arc<dyn Prompter> = tab_prompter.clone();
    match session.protocol {
        Protocol::Ssh | Protocol::Sftp | Protocol::Telnet | Protocol::Serial => {}
        Protocol::Ftp | Protocol::Ftps => {
            let fs = tokio::select! {
                () = cancel.cancelled() => return (End::Failed(Error::Cancelled), false),
                r = sh.transport.connect_files(session.clone(), prompter, cancel.clone()) => match r {
                    Ok(fs) => fs,
                    Err(e) => return (End::Failed(e), false),
                },
            };
            tab_prompter.commit();
            return park_files(sh, tab, generation, Conn::files(fs), cancel).await;
        }
        Protocol::Rdp | Protocol::Vnc => {
            return run_desktop(sh, tab, generation, session, &tab_prompter, cancel).await;
        }
    }
    let link = tokio::select! {
        () = cancel.cancelled() => return (End::Failed(Error::Cancelled), false),
        r = sh.transport.connect(session.clone(), prompter, cancel.clone()) => match r {
            Ok(link) => link,
            Err(e) => return (End::Failed(e), false),
        },
    };
    tab_prompter.commit();
    if session.protocol == Protocol::Sftp {
        return park_files(sh, tab, generation, Conn::new(link), cancel).await;
    }
    let (cols, rows) = sh.size(tab);
    let (reader, writer) = tokio::select! {
        () = cancel.cancelled() => return (End::Failed(Error::Cancelled), false),
        r = link.shell(session, cols, rows) => match r {
            Ok(pair) => pair,
            Err(e) => return (End::Failed(e), false),
        },
    };
    let mut link_info = link.ssh_info();
    let auth_banner = link.take_auth_banner();
    let conn = Arc::new(Conn::new(link.clone()));
    let (tx, rx) = mpsc::channel(INPUT_QUEUE);
    let Some(out) = sh.set_conn(tab, generation, conn.clone(), Some(tx)) else {
        return (End::Failed(Error::Cancelled), false);
    };
    // Open the SFTP channel now (kept for the browser) so the banner can say whether the server
    // offers it: network gear (Cisco) has no sftp subsystem.
    if let Some(info) = link_info.as_mut().filter(|_| session.protocol == Protocol::Ssh) {
        info.sftp = tokio::time::timeout(SFTP_PROBE, crate::files::file_ops(sh, tab)).await.ok().map(|r| r.is_ok());
    }
    // Windows has no X server by default: say so in the tab instead of dropping X11 silently.
    if cfg!(windows)
        && session.protocol == Protocol::Ssh
        && session.ssh.x11_forwarding
        && rxt_ssh::x11::X11::from_env().is_none()
    {
        out.push(format!("[rustXterm] X11 forwarding is off: {}\r\n", rxt_ssh::x11::UNAVAILABLE).as_bytes());
    }
    // Terminal folder fallback for SSH shells; started after the shell channel (see `follow`).
    let kicks = (session.protocol == Protocol::Ssh).then(|| {
        let (kick, kicks) = mpsc::channel(1);
        if let Some(t) = sh.tabs.lock().get_mut(&tab).filter(|t| t.generation == generation) {
            t.follow_kick = Some(kick);
        }
        kicks
    });
    // Remote monitoring (design M3): SSH terminals only, never when the command closes the tab.
    let closes = session.ssh.remote_command.is_some() && !session.ssh.keep_open_after_command;
    let mon = sh.tabs.lock().get(&tab).filter(|t| t.generation == generation).map(|t| t.monitor.clone());
    let monitor = async {
        if let Some(mon) = mon.filter(|_| session.protocol == Protocol::Ssh && !closes) {
            let reporter = sh.clone();
            let emit: crate::monitor::Emit = Arc::new(move |e| reporter.emit_now(e));
            crate::monitor::run(mon, link.clone(), tab, emit).await;
        }
        std::future::pending::<End>().await
    };
    let follow = async {
        if let Some(kicks) = kicks {
            crate::follow::run(sh.clone(), tab, link.clone(), kicks).await;
        }
        std::future::pending::<End>().await
    };
    // Banner order in the terminal: the UI's session box (on `Connected`), then the server's
    // pre-auth banner, then the shell output.
    if let Some(info) = link_info {
        sh.emit(Event::SshInfo { tab, info: Box::new(info) }).await;
    }
    sh.set_state(tab, generation, ConnState::Connected).await;
    if !auth_banner.is_empty() && out.push(auth_banner.as_bytes()) {
        sh.emit(Event::Output { tab }).await;
    }
    let end = tokio::select! {
        () = cancel.cancelled() => End::Failed(Error::Cancelled),
        end = pump_output(sh, tab, &out, reader, session) => end,
        end = pump_input(writer, rx) => end,
        end = follow => end,
        end = monitor => end,
    };
    (end, true)
}

/// Automatic reconnect after a dropped connection (serial unplug is handled separately).
fn auto_reconnect(session: &Session) -> bool {
    match session.protocol {
        Protocol::Serial => false,
        _ => session.ssh.auto_reconnect,
    }
}

/// Serial device poll interval while waiting for a replug.
const REPLUG_POLL: Duration = Duration::from_secs(1);

/// The session's port as listed by the platform (by-id link, USB ids), when known.
async fn serial_port_info(sh: &Shared, session: &Session) -> Option<rxt_core::PortInfo> {
    let ports = sh.config.serial_ports.clone().filter(|_| session.protocol == Protocol::Serial)?;
    let port = session.serial.port.clone();
    let list = tokio::task::spawn_blocking(move || ports.list()).await.ok()?.ok()?;
    list.into_iter().find(|p| p.path == port || p.by_id.as_deref() == Some(&port))
}

/// After [`REPLUG_POLL`], the path to try next: the same device as `last` when listed (it may
/// have a new name), else the configured path (ptys are not listed). `None` when cancelled.
async fn next_port(
    sh: &Shared,
    configured: &str,
    last: Option<&rxt_core::PortInfo>,
    cancel: &CancellationToken,
) -> Option<String> {
    tokio::select! {
        () = cancel.cancelled() => return None,
        () = tokio::time::sleep(REPLUG_POLL) => {}
    }
    let list = match sh.config.serial_ports.clone() {
        Some(ports) => tokio::task::spawn_blocking(move || ports.list()).await.ok().and_then(|r| r.ok()),
        None => None,
    };
    let found = list.and_then(|l| rxt_serial::find_port(&l, configured, last));
    Some(found.unwrap_or_else(|| configured.to_owned()))
}

/// File-only tab (standalone SFTP, FTP/FTPS): file commands use `conn` until the tab run ends.
// ponytail: parks until closed; add connection liveness polling if needed.
async fn park_files(
    sh: &Arc<Shared>,
    tab: TabId,
    generation: u64,
    conn: Conn,
    cancel: &CancellationToken,
) -> (End, bool) {
    sh.set_conn(tab, generation, Arc::new(conn), None);
    sh.set_state(tab, generation, ConnState::Connected).await;
    cancel.cancelled().await;
    (End::Failed(Error::Cancelled), true)
}

/// RDP/VNC tab: connect, then apply desktop events to the tab framebuffer until the session ends.
async fn run_desktop(
    sh: &Arc<Shared>,
    tab: TabId,
    generation: u64,
    session: &Session,
    prompter: &Arc<TabPrompter>,
    cancel: &CancellationToken,
) -> (End, bool) {
    let size = sh.tabs.lock().get(&tab).map_or(crate::DEFAULT_DESKTOP, |t| t.desktop_size);
    let (reader, send) = tokio::select! {
        () = cancel.cancelled() => return (End::Failed(Error::Cancelled), false),
        r = sh.transport.connect_desktop(session.clone(), prompter.clone(), cancel.clone(), size) => match r {
            Ok(pair) => pair,
            Err(e) => return (End::Failed(e), false),
        },
    };
    prompter.commit();
    let frame = {
        let mut tabs = sh.tabs.lock();
        let Some(t) = tabs.get_mut(&tab).filter(|t| t.generation == generation) else {
            return (End::Failed(Error::Cancelled), false);
        };
        t.desktop = Some(send.clone());
        if !t.visible {
            let _ = send(DesktopInput::Visible(false));
        }
        t.frame.clone()
    };
    sh.set_state(tab, generation, ConnState::Connected).await;
    let end = tokio::select! {
        () = cancel.cancelled() => End::Failed(Error::Cancelled),
        end = pump_desktop(sh, tab, &frame, reader) => end,
    };
    (end, true)
}

/// Applies desktop events; protocol or framebuffer errors end the session with `Error`.
async fn pump_desktop(
    sh: &Shared,
    tab: TabId,
    frame: &parking_lot::Mutex<rxt_core::Framebuffer>,
    mut reader: Box<dyn DesktopRead>,
) -> End {
    loop {
        match reader.next().await {
            Some(Ok(DesktopEvent::Clipboard(text))) => {
                if let Some(t) = sh.tabs.lock().get_mut(&tab) {
                    t.clipboard = Some(text);
                }
            }
            // ponytail: bell is ignored; add an Event when the UI wants to flash the tab.
            Some(Ok(DesktopEvent::Bell)) => continue,
            Some(Ok(event)) => {
                if let Err(e) = frame.lock().apply(&event) {
                    return End::Failed(e);
                }
            }
            Some(Err(e)) => return End::Failed(e),
            None => return End::Dropped("connection closed".into()),
        }
        sh.wake();
    }
}

/// Reads the transport only while the tab's output buffer has room (channel-window backpressure).
async fn pump_output(sh: &Shared, tab: TabId, out: &OutBuf, mut reader: Box<dyn ShellRead>, session: &Session) -> End {
    let mut exit = None;
    loop {
        while out.len() >= out.cap {
            out.space.notified().await;
        }
        match reader.read().await {
            Some(ShellMsg::Data(bytes)) => {
                if out.push(&bytes) {
                    sh.emit(Event::Output { tab }).await;
                } else {
                    sh.wake();
                }
            }
            Some(ShellMsg::ExitStatus(code)) => exit = Some(code),
            // Exit status may still follow EOF; wait for the channel to close.
            Some(ShellMsg::Eof) => {}
            None => break,
        }
    }
    match reader.end_reason() {
        Some(Error::Gone(_)) => return End::Gone,
        Some(e) => return End::Dropped(e.to_string()),
        None => {}
    }
    match exit {
        Some(_) if session.ssh.remote_command.is_some() && !session.ssh.keep_open_after_command => End::Close,
        Some(code) => End::Exited(format!("exited with status {code}")),
        None => End::Dropped("connection closed".into()),
    }
}

async fn pump_input(writer: Box<dyn ShellWrite>, mut rx: mpsc::Receiver<InputMsg>) -> End {
    while let Some(msg) = rx.recv().await {
        let result = match msg {
            InputMsg::Bytes(bytes) => writer.write(&bytes).await,
            InputMsg::Resize(cols, rows) => writer.resize(cols, rows).await,
        };
        if let Err(e) = result {
            return End::Dropped(e.to_string());
        }
    }
    std::future::pending().await
}

/// Forwards prompts to the UI as `Event::Prompt`; cancelled when the tab run ends. Passwords and
/// key passphrases are answered from saved credentials first; a saved one that is asked for
/// again was rejected, so it is forgotten and the user is asked. Secrets the user asked to save
/// are stored by [`TabPrompter::commit`] once the login succeeded.
pub(crate) struct TabPrompter {
    sh: Arc<Shared>,
    /// `None` for connections not tied to a tab (dedicated tunnel connections).
    tab: Option<TabId>,
    cancel: CancellationToken,
    session: Session,
    /// Credential prompts answered from saved credentials during this login.
    used_saved: Mutex<Vec<PromptKind>>,
    /// Typed secrets to save on success.
    to_save: Mutex<Vec<(PromptKind, Secret)>>,
}

impl TabPrompter {
    pub fn new(sh: Arc<Shared>, tab: Option<TabId>, session: &Session, cancel: CancellationToken) -> Self {
        Self { sh, tab, cancel, session: session.clone(), used_saved: Mutex::default(), to_save: Mutex::default() }
    }

    /// Login succeeded: store the secrets the user asked to save.
    pub fn commit(&self) {
        let pending = std::mem::take(&mut *self.to_save.lock());
        if let Some(records) = &self.sh.config.records {
            for (kind, secret) in pending {
                records.save_credential(&self.session, &kind, &secret);
            }
        }
    }

    async fn credential(&self, kind: PromptKind) -> PromptAnswer {
        let key = match &kind {
            PromptKind::Password { user, host, .. } => {
                PromptKind::Password { user: user.clone(), host: host.clone(), retry: false }
            }
            _ => kind.clone(),
        };
        if let Some(records) = &self.sh.config.records {
            let retried = self.used_saved.lock().contains(&key);
            if retried {
                records.forget_credential(&self.session, &key);
            } else if let Some(secret) = records.credential(&self.session, &key) {
                self.used_saved.lock().push(key);
                return PromptAnswer::Secret(secret);
            }
        }
        let (answer, save) = self.sh.ask(self.tab, kind, &self.cancel).await;
        if let (true, PromptAnswer::Secret(secret)) = (save, &answer) {
            let mut to_save = self.to_save.lock();
            to_save.retain(|(k, _)| *k != key);
            to_save.push((key, secret.clone()));
        }
        answer
    }
}

impl Prompter for TabPrompter {
    fn prompt(&self, kind: PromptKind) -> BoxFut<'_, PromptAnswer> {
        match kind {
            PromptKind::Password { .. } | PromptKind::KeyPassphrase { .. } => Box::pin(self.credential(kind)),
            kind => Box::pin(async move { self.sh.ask(self.tab, kind, &self.cancel).await.0 }),
        }
    }

    fn host_key_changed(&self, host: &str, old_fingerprint: &str, new_fingerprint: &str) {
        let event = match self.tab {
            Some(tab) => Event::HostKeyChanged {
                tab,
                host: host.into(),
                old: old_fingerprint.into(),
                new: new_fingerprint.into(),
            },
            None => Event::Notice {
                tab: None,
                message: format!("host key of {host} changed (was {old_fingerprint}, now {new_fingerprint}); refused"),
            },
        };
        self.sh.emit_now(event);
    }
}
