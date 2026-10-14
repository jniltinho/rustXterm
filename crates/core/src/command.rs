//! Commands (UI/CLI/MCP → engine) and events (engine → UI), design D6.
//!
//! Every user action has a [`CommandId`]; shortcuts, MultiExec and agents reuse these types.
//! Terminal output is not an event: it flows through per-session byte buffers owned by the engine.

use serde::{Deserialize, Serialize};

use crate::{FileEntry, PortInfo, Session, Tunnel};

/// Tab (open connection) identifier, allocated by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct TabId(pub u64);

/// Prompt identifier, allocated by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PromptId(pub u64);

/// Transfer identifier, allocated by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TransferId(pub u64);

/// Agent approval identifier, allocated by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ApprovalId(pub u64);

/// Registered command id (stable string, used for shortcuts and agents).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandId(pub &'static str);

impl CommandId {
    pub const NEW_SESSION: Self = Self("session.new");
    pub const QUICK_CONNECT: Self = Self("session.quick_connect");
    pub const CLOSE_TAB: Self = Self("tab.close");
    pub const CLOSE_OTHERS: Self = Self("tab.close_others");
    pub const NEXT_TAB: Self = Self("tab.next");
    pub const PREV_TAB: Self = Self("tab.prev");
    pub const RENAME_TAB: Self = Self("tab.rename");
    pub const DUPLICATE_TAB: Self = Self("tab.duplicate");
    pub const RECONNECT: Self = Self("tab.reconnect");
    pub const FIND: Self = Self("terminal.find");
    pub const COPY: Self = Self("terminal.copy");
    pub const PASTE: Self = Self("terminal.paste");
    pub const SAVE_TEXT: Self = Self("terminal.save_text");
    pub const SETTINGS: Self = Self("app.settings");
    pub const TOGGLE_SIDEBAR: Self = Self("app.toggle_sidebar");
    pub const TOGGLE_FULLSCREEN: Self = Self("app.toggle_fullscreen");
    pub const MULTIEXEC: Self = Self("multiexec.toggle");
    pub const TUNNELS: Self = Self("tunnels.open");
    pub const HELP: Self = Self("app.help");
    pub const EXIT: Self = Self("app.exit");
    // UI-only ids (menu bar, design R4). Agents cannot reach any `CommandId`: the control channel
    // only accepts `AgentOp`s, and a GUI-only name is refused outside the GUI (`control::allowed`).
    pub const IMPORT_SESSIONS: Self = Self("session.import");
    pub const EXPORT_SESSIONS: Self = Self("session.export");
    pub const ABOUT: Self = Self("app.about");
    pub const CHANGE_PASSWORD: Self = Self("vault.change_password");
    pub const THEME_LIGHT: Self = Self("view.theme_light");
    pub const THEME_DARK: Self = Self("view.theme_dark");
    pub const THEME_SYSTEM: Self = Self("view.theme_system");
    /// Open the Terminal menu from the keyboard (default Ctrl+Shift+F10); F10/Alt stay with terminals.
    pub const FOCUS_MENU: Self = Self("app.focus_menu");
    pub const SERIAL_SEND_BREAK: Self = Self("serial.send_break");
    pub const SERIAL_SET_LINES: Self = Self("serial.set_lines");
    pub const SERIAL_LIST_PORTS: Self = Self("serial.list_ports");
    /// Turn remote monitoring of a tab on or off (`Command::SetMonitoring`); GUI-only.
    pub const MONITORING_TOGGLE: Self = Self("tab.monitoring.toggle");
    /// Remote monitoring refresh interval (`Command::SetMonitorInterval`); GUI-only.
    pub const MONITORING_INTERVAL: Self = Self("settings.monitoring.interval");
    /// Show and focus the Find session filter above the session tree; GUI-only.
    pub const FIND_SESSION: Self = Self("session.find");

    /// UI-only ids, never callable by agents.
    pub const UI_ONLY: [Self; 11] = [
        Self::IMPORT_SESSIONS,
        Self::EXPORT_SESSIONS,
        Self::ABOUT,
        Self::CHANGE_PASSWORD,
        Self::THEME_LIGHT,
        Self::THEME_DARK,
        Self::THEME_SYSTEM,
        Self::FOCUS_MENU,
        Self::MONITORING_TOGGLE,
        Self::MONITORING_INTERVAL,
        Self::FIND_SESSION,
    ];

    /// Every registered command id.
    pub const ALL: [Self; 34] = [
        Self::NEW_SESSION,
        Self::QUICK_CONNECT,
        Self::CLOSE_TAB,
        Self::CLOSE_OTHERS,
        Self::NEXT_TAB,
        Self::PREV_TAB,
        Self::RENAME_TAB,
        Self::DUPLICATE_TAB,
        Self::RECONNECT,
        Self::FIND,
        Self::COPY,
        Self::PASTE,
        Self::SAVE_TEXT,
        Self::SETTINGS,
        Self::TOGGLE_SIDEBAR,
        Self::TOGGLE_FULLSCREEN,
        Self::MULTIEXEC,
        Self::TUNNELS,
        Self::HELP,
        Self::EXIT,
        Self::IMPORT_SESSIONS,
        Self::EXPORT_SESSIONS,
        Self::ABOUT,
        Self::CHANGE_PASSWORD,
        Self::THEME_LIGHT,
        Self::THEME_DARK,
        Self::THEME_SYSTEM,
        Self::FOCUS_MENU,
        Self::SERIAL_SEND_BREAK,
        Self::SERIAL_SET_LINES,
        Self::SERIAL_LIST_PORTS,
        Self::MONITORING_TOGGLE,
        Self::MONITORING_INTERVAL,
        Self::FIND_SESSION,
    ];

    pub fn as_str(self) -> &'static str {
        self.0
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.0 == s)
    }
}

/// A secret typed by the user. Never printed or serialized in clear.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl Serialize for Secret {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(if self.0.is_empty() { "absent" } else { "saved" })
    }
}

/// Connection state shown on tabs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnState {
    Connecting,
    Connected,
    /// Remote closed or command finished; `reconnect_at` is the next automatic attempt (unix secs).
    Disconnected {
        reason: String,
        reconnect_at: Option<i64>,
    },
    Error(String),
}

/// Host-key decision for an unknown key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostKeyDecision {
    AcceptOnce,
    AcceptAndSave,
    Cancel,
}

/// What the engine asks the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromptKind {
    /// Unknown host key (SHA-256 fingerprint).
    HostKeyUnknown {
        host: String,
        port: u16,
        algorithm: String,
        fingerprint: String,
    },
    Password {
        user: String,
        host: String,
        retry: bool,
    },
    KeyPassphrase {
        path: String,
    },
    /// Keyboard-interactive round; each prompt is (text, echo).
    KeyboardInteractive {
        name: String,
        instructions: String,
        prompts: Vec<(String, bool)>,
    },
    /// Transfer target exists.
    Overwrite {
        path: String,
    },
    /// Unknown TLS certificate (FTPS, RDP): answered with `PromptAnswer::HostKey`.
    CertificateUnknown {
        host: String,
        port: u16,
        subject: String,
        fingerprint: String,
    },
}

/// User answer to a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromptAnswer {
    HostKey(HostKeyDecision),
    Secret(Secret),
    Responses(Vec<Secret>),
    Overwrite(crate::Overwrite),
    Cancel,
}

/// Transfer direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Download,
    Upload,
}

/// Tunnel runtime state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TunnelState {
    Stopped,
    Starting,
    Running,
    Error(String),
}

/// Requests to the engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Command {
    Open {
        session: Box<Session>,
    },
    Close {
        tab: TabId,
    },
    Reconnect {
        tab: TabId,
    },
    Duplicate {
        tab: TabId,
    },
    /// Keyboard/paste bytes; routed through the high-priority input queue (and MultiExec fan-out).
    Input {
        tab: TabId,
        bytes: Vec<u8>,
    },
    Resize {
        tab: TabId,
        cols: u16,
        rows: u16,
    },
    /// Keyboard/pointer/clipboard input for an RDP/VNC tab (high-priority path).
    DesktopInput {
        tab: TabId,
        input: crate::DesktopInput,
    },
    /// Tab shown or hidden; hidden remote desktops stop requesting updates.
    SetVisible {
        tab: TabId,
        visible: bool,
    },
    SetMultiExec {
        enabled: bool,
    },
    SelectMultiExec {
        tab: TabId,
        selected: bool,
    },
    Answer {
        prompt: PromptId,
        answer: PromptAnswer,
    },
    /// List a remote directory over the tab's SFTP channel; `~` or empty means the remote home.
    List {
        tab: TabId,
        path: String,
    },
    Mkdir {
        tab: TabId,
        path: String,
    },
    Rename {
        tab: TabId,
        from: String,
        to: String,
    },
    Delete {
        tab: TabId,
        path: String,
    },
    Chmod {
        tab: TabId,
        path: String,
        mode: u32,
    },
    Transfer {
        tab: TabId,
        direction: Direction,
        remote: String,
        local: String,
    },
    CancelTransfer {
        id: TransferId,
    },
    /// Turn following the terminal folder of an SSH tab on or off for the current connection
    /// (starts from `SshOptions::follow_terminal_folder`). On: the engine reports the login
    /// shell's folder as `Event::CwdChanged` after each Enter, and checks once right away.
    SetFollowCwd {
        tab: TabId,
        enabled: bool,
    },
    /// `session` is the SSH session named by `tunnel.via`; used to open a dedicated
    /// connection when no connected tab of that session exists (autostart).
    StartTunnel {
        tunnel: Tunnel,
        session: Option<Box<Session>>,
    },
    StopTunnel {
        name: String,
    },
    /// Send a break condition on a serial tab (`serial.send_break`).
    SerialBreak {
        tab: TabId,
        ms: u64,
    },
    /// Set DTR/RTS on a serial tab; `None` leaves a line unchanged (`serial.set_lines`).
    SerialLines {
        tab: TabId,
        dtr: Option<bool>,
        rts: Option<bool>,
    },
    /// Enumerate serial ports; answered with `Event::SerialPorts` (`serial.list_ports`).
    ListSerialPorts,
    /// Start or stop remote monitoring of an SSH tab for its current connection
    /// (`tab.monitoring.toggle`, GUI-only); not saved. Ignored for other tabs.
    SetMonitoring {
        tab: TabId,
        enabled: bool,
    },
    /// Remote monitoring refresh interval in seconds, clamped to 1–30; applies to running tabs
    /// (`settings.monitoring.interval`, GUI-only).
    SetMonitorInterval {
        secs: u32,
    },
    /// GUI-only answer to `Event::ApprovalRequest`.
    AnswerApproval {
        id: ApprovalId,
        approve: bool,
    },
    Shutdown,
}

/// Facts of an established SSH connection, shown in the session banner.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshInfo {
    /// Server identification string (`SSH-2.0-…`), as sent by the server.
    pub server_id: String,
    pub kex: String,
    pub cipher: String,
    /// Compression negotiated (zlib) in either direction.
    pub compression: bool,
    pub host_key_algorithm: String,
    /// `SHA256:…`.
    pub host_key_fingerprint: String,
    /// X11 forwarding requested on the shell channel.
    pub x11: bool,
    /// Why X11 forwarding is off although the session asked for it.
    pub x11_unavailable: Option<String>,
    /// Agent forwarding requested on the shell channel.
    pub agent: bool,
    /// The server offers the `sftp` subsystem (probed at login); `None` when not probed.
    #[serde(default)]
    pub sftp: Option<bool>,
}

/// Notifications from the engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    TabOpened {
        tab: TabId,
        session: Box<Session>,
    },
    State {
        tab: TabId,
        state: ConnState,
    },
    /// Connection facts of an SSH tab; sent just before its `Connected` state.
    SshInfo {
        tab: TabId,
        info: Box<SshInfo>,
    },
    /// New output is buffered for `tab`; the UI drains it on its next frame.
    Output {
        tab: TabId,
    },
    Prompt {
        id: PromptId,
        tab: Option<TabId>,
        kind: PromptKind,
    },
    /// Changed host key: connection refused, both fingerprints shown.
    HostKeyChanged {
        tab: TabId,
        host: String,
        old: String,
        new: String,
    },
    CwdChanged {
        tab: TabId,
        path: String,
    },
    Listing {
        tab: TabId,
        path: String,
        entries: Vec<FileEntry>,
    },
    TransferStarted {
        id: TransferId,
        tab: TabId,
        direction: Direction,
        remote: String,
        local: String,
        total: Option<u64>,
    },
    TransferProgress {
        id: TransferId,
        done: u64,
    },
    TransferFinished {
        id: TransferId,
        result: Result<u64, crate::Error>,
    },
    TunnelState {
        name: String,
        state: TunnelState,
    },
    MultiExec {
        enabled: bool,
        selected: Vec<TabId>,
    },
    /// Actionable error not tied to a state change.
    /// An agent asks to run a command under an `ask` policy; answered with `Command::AnswerApproval`
    /// (GUI only). Denied automatically at `expires_at` (unix secs).
    ApprovalRequest {
        id: ApprovalId,
        client: String,
        command: String,
        session: String,
        target: String,
        expires_at: i64,
    },
    Notice {
        tab: Option<TabId>,
        message: String,
    },
    /// Answer to `Command::ListSerialPorts`.
    SerialPorts {
        result: Result<Vec<PortInfo>, crate::Error>,
    },
    /// A new remote monitoring sample of `tab` waits in the engine's latest-sample slot
    /// (coalesced: sent only when the slot was empty). Never forwarded to agents.
    Metrics {
        tab: TabId,
    },
    /// Remote monitoring of `tab` started, stopped or failed.
    Monitor {
        tab: TabId,
        state: crate::MonitorState,
    },
    Closed {
        tab: TabId,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Protocol;

    #[test]
    fn command_ids_unique_and_resolvable() {
        let mut ids: Vec<_> = CommandId::ALL.iter().map(|c| c.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), CommandId::ALL.len());
        assert_eq!(CommandId::parse("tab.close"), Some(CommandId::CLOSE_TAB));
        assert_eq!(CommandId::parse("app.focus_menu"), Some(CommandId::FOCUS_MENU));
        assert!(CommandId::UI_ONLY.iter().all(|c| CommandId::parse(c.as_str()) == Some(*c)));
    }

    #[test]
    fn ui_only_ids_are_not_agent_callable() {
        use crate::control::{AgentOp, EntryPoint, allowed};
        for c in CommandId::UI_ONLY {
            let op = AgentOp::GuiOnly { name: c.as_str().into() };
            for entry in [EntryPoint::CliLive, EntryPoint::CliOffline, EntryPoint::McpLive, EntryPoint::McpOffline] {
                assert!(!allowed(op.group(), entry), "{} reachable from {entry:?}", c.as_str());
            }
        }
    }

    #[test]
    fn secrets_never_serialize_in_clear() {
        let c = Command::Answer { prompt: PromptId(1), answer: PromptAnswer::Secret(Secret("hunter2".into())) };
        let json = serde_json::to_string(&c).unwrap();
        assert!(!json.contains("hunter2"));
        assert!(!format!("{c:?}").contains("hunter2"));
    }

    #[test]
    fn serde_round_trip() {
        let cmds = [
            Command::Open { session: Box::new(Session::new(Protocol::Ssh, "h", None, None)) },
            Command::Input { tab: TabId(3), bytes: b"uptime\r".to_vec() },
            Command::Resize { tab: TabId(3), cols: 80, rows: 24 },
            Command::Answer { prompt: PromptId(2), answer: PromptAnswer::HostKey(HostKeyDecision::AcceptOnce) },
            Command::SetMonitoring { tab: TabId(3), enabled: false },
            Command::SetMonitorInterval { secs: 10 },
        ];
        for c in cmds {
            assert_eq!(serde_json::from_str::<Command>(&serde_json::to_string(&c).unwrap()).unwrap(), c);
        }
        let ev =
            Event::State { tab: TabId(1), state: ConnState::Disconnected { reason: "eof".into(), reconnect_at: None } };
        assert_eq!(serde_json::from_str::<Event>(&serde_json::to_string(&ev).unwrap()).unwrap(), ev);
        let ev = Event::Monitor { tab: TabId(1), state: crate::MonitorState::Unavailable("refused".into()) };
        assert_eq!(serde_json::from_str::<Event>(&serde_json::to_string(&ev).unwrap()).unwrap(), ev);
    }

    #[test]
    fn monitoring_ids_are_gui_only() {
        assert!(CommandId::UI_ONLY.contains(&CommandId::MONITORING_TOGGLE));
        assert!(CommandId::UI_ONLY.contains(&CommandId::MONITORING_INTERVAL));
        assert!(CommandId::UI_ONLY.contains(&CommandId::FIND_SESSION));
    }
}
