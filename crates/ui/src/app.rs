//! Main window: toolbar, quick connect, sidebar, tabs, status bar and the action loop.
//!
//! Every user action becomes an [`Action`]; app-level actions are [`CommandId`]s run by
//! [`App::run`] (toolbar, menus and shortcuts share them), engine requests are [`Command`]s.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use egui::{Color32, RichText, Sense, Theme, WidgetInfo, WidgetType, vec2};
use rxt_core::fileops::join;
use rxt_core::{
    Command, CommandId, ConnState, DesktopInput, Direction, Event, FolderId, Framebuffer, Metrics, PromptAnswer,
    PromptId, Protocol, RdpResolution, Secret, Session, SessionId, TabId, ThemeMode, Tokens, TunnelState,
};
use rxt_engine::EngineHandle;
use rxt_platform::{FileDialogs, Paths, Platform};
use rxt_term::LogFormat;

use crate::agent::{self, Approval, AuditView};
use crate::banner;
use crate::desktop::{self, DesktopView};
pub(crate) use crate::dialogs::Modal;
use crate::dialogs::{self, Prompt};
use crate::file_tab;
use crate::icons::{self, Icons};
use crate::monitor_bar;
use crate::session_editor::{self, Editor};
use crate::settings::{self, Settings};
use crate::sftp_panel::{self, HOME, SftpAction, SftpState, TransferRow};
use crate::sidebar::{self, SessionStore};
use crate::store_ui::{self, StoreDialog};
use crate::tabs::{self, Tab, ended_session_key, move_tab, state_text};
use crate::terminal::{self, ViewConfig};
use crate::theme::{self, color};
use crate::tunnels::{self, TunnelEditor};
use crate::vault::Vault;

/// Help window.
pub const HELP: CommandId = CommandId::HELP;
/// Quit the application.
pub const EXIT: CommandId = CommandId::EXIT;

/// Output bytes fed to terminals per frame, across all tabs.
const OUTPUT_BUDGET: usize = 1 << 20;
/// Engine events handled per frame before yielding to paint.
const EVENT_BUDGET: usize = 1024;
const KEY_SETTINGS: &str = "rxt.settings";
const KEY_STORE: &str = "rxt.sessions";
/// Agent workspace override; kept out of `config.toml` because it is GUI-only.
const KEY_AGENT_WORKSPACE: &str = "rxt.agent_workspace";

/// Toolbar buttons left to right; Exit is pinned to the right edge separately.
const TOOLBAR: [(&str, &str, CommandId); 6] = [
    ("session-new", "Session", CommandId::NEW_SESSION),
    ("sessions", "Sessions", CommandId::TOGGLE_SIDEBAR),
    ("tunneling", "Tunneling", CommandId::TUNNELS),
    ("multiexec", "MultiExec", CommandId::MULTIEXEC),
    ("settings", "Settings", CommandId::SETTINGS),
    ("help", "Help", HELP),
];
/// Default `app.focus_menu` shortcut when the settings carry none.
const FOCUS_MENU_DEFAULT: &str = "Ctrl+Shift+F10";
/// Minimum main window size (logical px); also set on the native viewport.
pub const MIN_WINDOW: egui::Vec2 = egui::vec2(800.0, 500.0);

/// Frames painted before a graphics start counts as working.
const GPU_START_FRAMES: u32 = 3;

/// Crash-safe graphics start handed over by the binary ([`App::with_gpu_start`]).
pub struct GpuStart {
    /// `gpu_backend` value to save once painted (a working fallback).
    pub save: Option<String>,
    /// Status line explaining a fallback.
    pub notice: Option<String>,
    /// Called once painted: removes the start marker.
    pub on_started: Box<dyn FnOnce() + Send>,
}

/// What the UI needs from the engine; implemented by [`EngineHandle`] and by test doubles.
pub trait Backend {
    fn send(&self, command: Command) -> rxt_core::Result<()>;
    fn try_event(&self) -> Option<Event>;
    fn drain_output(&self, tab: TabId, budget: usize, out: &mut Vec<u8>) -> usize;
    fn pending_output(&self) -> Vec<TabId>;
    fn shutdown(&self, timeout: Duration) {
        let _ = timeout;
    }
    /// Run `f` on the tab's framebuffer; false when the tab has none.
    fn with_framebuffer(&self, tab: TabId, f: &mut dyn FnMut(&mut Framebuffer)) -> bool {
        let _ = (tab, f);
        false
    }
    /// Remote clipboard text received since the last call.
    fn take_clipboard(&self, tab: TabId) -> Option<String> {
        let _ = tab;
        None
    }
    /// Save the secret answering `prompt` once the login succeeds ("save password").
    fn save_secret(&self, prompt: PromptId) {
        let _ = prompt;
    }
    /// Open or close the local agent control channel (Settings → Agent control).
    fn set_agent_control(&self, enabled: bool) {
        let _ = enabled;
    }
    /// Directory agent `files get`/`put` are confined to.
    fn set_agent_workspace(&self, dir: &Path) {
        let _ = dir;
    }
    /// Latest remote monitoring sample of `tab`; taken only while its bar is painted.
    fn take_metrics(&self, tab: TabId) -> Option<rxt_core::MetricsSample> {
        let _ = tab;
        None
    }
    /// Global "Remote monitoring" setting for tabs connecting afterwards.
    fn set_remote_monitoring(&self, enabled: bool) {
        let _ = enabled;
    }
}

impl Backend for EngineHandle {
    fn send(&self, command: Command) -> rxt_core::Result<()> {
        EngineHandle::send(self, command)
    }
    fn try_event(&self) -> Option<Event> {
        EngineHandle::try_event(self)
    }
    fn drain_output(&self, tab: TabId, budget: usize, out: &mut Vec<u8>) -> usize {
        EngineHandle::drain_output(self, tab, budget, out)
    }
    fn pending_output(&self) -> Vec<TabId> {
        EngineHandle::pending_output(self)
    }
    fn shutdown(&self, timeout: Duration) {
        EngineHandle::shutdown(self, timeout);
    }
    fn with_framebuffer(&self, tab: TabId, f: &mut dyn FnMut(&mut Framebuffer)) -> bool {
        EngineHandle::with_framebuffer(self, tab, f).is_some()
    }
    fn take_clipboard(&self, tab: TabId) -> Option<String> {
        EngineHandle::take_clipboard(self, tab)
    }
    fn save_secret(&self, prompt: PromptId) {
        EngineHandle::save_secret(self, prompt);
    }
    fn set_agent_control(&self, enabled: bool) {
        if let Err(e) = EngineHandle::set_agent_control(self, enabled) {
            tracing::warn!("agent control: {e}");
        }
    }
    fn set_agent_workspace(&self, dir: &Path) {
        EngineHandle::set_agent_workspace(self, dir);
    }
    fn take_metrics(&self, tab: TabId) -> Option<rxt_core::MetricsSample> {
        EngineHandle::take_metrics(self, tab)
    }
    fn set_remote_monitoring(&self, enabled: bool) {
        EngineHandle::set_remote_monitoring(self, enabled);
    }
}

/// A user action produced while drawing, applied after the frame's layout.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Action {
    Run(CommandId),
    /// Make `tab` active, then run the command on it (tab context menu).
    OnTab(TabId, CommandId),
    SelectTab(Option<TabId>),
    MoveTab {
        from: usize,
        to: usize,
    },
    CloseTab(TabId),
    CloseOthers(TabId),
    /// Quit even though tabs are connected (the user confirmed).
    Exit,
    RenameTab(TabId, String),
    SelectMultiExec(TabId, bool),
    Open(Box<Session>),
    SelectSession(SessionId),
    NewSession(Option<FolderId>),
    EditSession(SessionId),
    DuplicateSession(SessionId),
    SaveSession {
        session: Box<Session>,
        connect: bool,
    },
    DeleteSession(SessionId),
    MoveSession(SessionId, Option<FolderId>),
    SaveFolder {
        id: Option<FolderId>,
        parent: Option<FolderId>,
        name: String,
        icon: Option<String>,
    },
    DeleteFolder(FolderId),
    Ask(Modal),
    Answer(PromptId, PromptAnswer),
    Engine(Command),
    PasteConfirmed(TabId, String),
    /// Remote browser request (side panel or the remote pane of the active file tab).
    Sftp(SftpAction),
    /// Local pane request of the active file tab.
    Local(SftpAction),
    Status(String),
    /// Ctrl+wheel zoom in a terminal tab: new font size in points, saved for next time.
    TerminalFontSize(TabId, f32),
    /// Open a local file with its default application (`folder`: show the folder containing it).
    OpenLocal {
        path: PathBuf,
        folder: bool,
    },
    BrowseKeyFile,
    StartTunnel(String),
    EditTunnel(String),
    SaveTunnel {
        original: Option<String>,
        tunnel: rxt_core::Tunnel,
    },
    DeleteTunnel(String),
    MoveTunnel {
        name: String,
        up: bool,
    },
    /// "Save password" ticked for this prompt's answer.
    SaveSecret(PromptId),
    /// Open the create or unlock dialog of the session database.
    ShowUnlock,
    CreateVault {
        master: Secret,
        keyring: bool,
    },
    Unlock(Secret),
    ShowChangePassword,
    ChangePassword(Secret),
    SetKeyring(bool),
    ImportIni,
    /// Settings → Diagnostics: choose the folder of `rustxterm-logs`.
    PickDiagFolder,
    /// Settings → Diagnostics: delete the diagnostic log and the stream files.
    ClearDiagnostics,
    /// Start (after consent) or stop data-stream capture.
    StreamCapture {
        on: bool,
        include_input: bool,
    },
    /// Import the MobaXterm.ini found on first run (the offer card).
    ImportMobaFound(std::path::PathBuf),
    /// Dismiss the first-run MobaXterm import offer without importing.
    DismissMobaOffer,
    ConfirmImport,
    ImportToml,
    ExportToml,
    ConfirmExport,
    /// Store the agent policy of a saved session (GUI only).
    SavePolicy(SessionId, rxt_store::db::Policy),
    BrowseAgentWorkspace,
    ShowAudit,
    /// Set DTR/RTS of a serial tab from its menu; `None` keeps a line.
    SerialLines {
        tab: TabId,
        dtr: Option<bool>,
        rts: Option<bool>,
    },
    /// Save the password the editor collected (Telnet auto-login) for the session just saved.
    SavePassword(Secret),
    /// Turn remote monitoring of a tab on or off (bar ×, SFTP panel button); not saved.
    SetMonitoring(TabId, bool),
    /// Load this page of the audit log.
    AuditPage(usize),
    ClearAudit,
}

/// Result of blocking session-database work run off the UI thread.
enum VaultMsg {
    /// Create or unlock finished; `Ok(false)`: the keyring could not unlock silently.
    Opened(rxt_core::Result<bool>),
    PasswordChanged(rxt_core::Result<()>),
    KeyringSet(bool, rxt_core::Result<()>),
}

/// Kind of file dialog; each remembers its last folder for this run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum DialogKind {
    Upload,
    Download,
    KeyFile,
    SaveText,
    Import,
    Export,
    Workspace,
    DiagFolder,
}

/// Result of a native file dialog run off the UI thread.
enum DialogResult {
    Upload { tab: TabId, dir: String, files: Vec<PathBuf> },
    Download { tab: TabId, remote: String, local: PathBuf },
    KeyFile(PathBuf),
    SaveText(PathBuf, String),
    ImportIni(PathBuf),
    ImportToml(PathBuf),
    ExportToml(PathBuf),
    AgentWorkspace(PathBuf),
    DiagFolder(PathBuf),
}

impl DialogResult {
    /// Dialog kind and the folder it ended in.
    fn folder(&self) -> Option<(DialogKind, PathBuf)> {
        let (kind, path) = match self {
            Self::Upload { files, .. } => (DialogKind::Upload, files.first()?),
            Self::Download { local, .. } => (DialogKind::Download, local),
            Self::KeyFile(p) => (DialogKind::KeyFile, p),
            Self::SaveText(p, _) => (DialogKind::SaveText, p),
            Self::ImportIni(p) | Self::ImportToml(p) => (DialogKind::Import, p),
            Self::ExportToml(p) => (DialogKind::Export, p),
            Self::AgentWorkspace(p) => return Some((DialogKind::Workspace, p.clone())),
            Self::DiagFolder(p) => return Some((DialogKind::DiagFolder, p.clone())),
        };
        Some((kind, path.parent()?.to_owned()))
    }
}

/// Transfers started by `tab` (each tab lists only its own).
fn tab_transfers(all: &[TransferRow], tab: Option<TabId>) -> Vec<TransferRow> {
    all.iter().filter(|t| Some(t.tab) == tab).cloned().collect()
}

/// Notification title and file name of a finished transfer.
fn transfer_note(t: &TransferRow) -> (&'static str, String) {
    let name = t.remote.rsplit('/').next().unwrap_or(&t.remote).to_owned();
    let title = match (t.direction, &t.result) {
        (Direction::Download, Some(Ok(_))) => "Download finished",
        (Direction::Download, _) => "Download failed",
        (Direction::Upload, Some(Ok(_))) => "Upload finished",
        (Direction::Upload, _) => "Upload failed",
    };
    (title, name)
}

/// Folder a dialog opens in: the last one used for its kind, else Downloads, else home.
pub(crate) fn start_dir(last: Option<&Path>, downloads: Option<PathBuf>, home: PathBuf) -> PathBuf {
    match last {
        Some(dir) => dir.to_owned(),
        None => downloads.filter(|d| d.is_dir()).unwrap_or(home),
    }
}

/// Sidebar page (persisted as `Settings::sidebar_page`).
pub(crate) type SidebarTab = sidebar::Page;

/// Terminal find bar state.
#[derive(Debug, Default)]
pub(crate) struct FindBar {
    pub text: String,
    pub case_sensitive: bool,
    pub status: Option<String>,
}

/// The rustXterm main window.
pub struct App {
    pub(crate) platform: Platform,
    backend: Box<dyn Backend>,
    pub(crate) settings: Settings,
    pub(crate) store: SessionStore,
    pub(crate) tabs: Vec<Tab>,
    pub(crate) active: Option<TabId>,
    pub(crate) quick: String,
    pub(crate) quick_error: Option<String>,
    /// Entry chosen with the arrow keys in the Quick connect list (see [`Self::quick_target`]).
    pub(crate) quick_pick: Option<usize>,
    /// Page shown. Starts as `settings.sidebar_page` (the page last chosen with the strip);
    /// automatic switches change only this field, so they are never saved.
    pub(crate) sidebar_tab: SidebarTab,
    /// Find session text; `None` while the field is hidden.
    pub(crate) filter: Option<String>,
    pub(crate) selected_session: Option<SessionId>,
    system_dark: Arc<Mutex<Option<bool>>>,
    pub(crate) tokens: Tokens,
    pub(crate) icons: Icons,
    pub(crate) prompts: VecDeque<Prompt>,
    pub(crate) modal: Option<Modal>,
    /// Set once the user confirmed quitting with connected tabs.
    exit_confirmed: bool,
    pub(crate) editor: Option<Editor>,
    pub(crate) show_settings: bool,
    pub(crate) show_tunnels: bool,
    pub(crate) tunnel_editor: Option<TunnelEditor>,
    pub(crate) tunnel_states: HashMap<String, TunnelState>,
    pub(crate) multiexec: bool,
    /// SFTP side panel, attached to the active connected SSH tab.
    pub(crate) sftp: SftpState,
    /// Browser state of the other connected SSH tabs, restored when they become active again.
    pub(crate) parked_sftp: HashMap<TabId, SftpState>,
    pub(crate) transfers: Vec<TransferRow>,
    /// Last folder of each file dialog kind (this run only).
    last_dirs: HashMap<DialogKind, PathBuf>,
    /// Main window has the input focus (read each frame); notifications only while it has not.
    pub(crate) window_focused: bool,
    pub(crate) find: Option<FindBar>,
    pub(crate) status: String,
    dialog_tx: mpsc::Sender<DialogResult>,
    dialog_rx: mpsc::Receiver<DialogResult>,
    ctx: Option<egui::Context>,
    pub(crate) fullscreen: bool,
    focus_quick: bool,
    /// Open the Terminal menu next frame (`app.focus_menu`).
    open_menu: bool,
    /// Window geometry checked against the monitor once (restored state may be off-screen).
    window_synced: bool,
    /// Crash-safe graphics start still waiting for painted frames, and frames painted so far.
    gpu_start: Option<(GpuStart, u32)>,
    /// The binary's diagnostic log, the settings last applied to it and what it reported.
    pub(crate) diagnostics: Option<Box<dyn crate::diagnostics::DiagnosticsControl>>,
    pub(crate) diag_applied: Option<rxt_core::DiagSettings>,
    pub(crate) diag_state: crate::diagnostics::DiagState,
    stream_chip_clicked: bool,
    /// Data-stream switch for this run (never saved).
    pub(crate) stream: crate::diagnostics::StreamUi,
    out_buf: Vec<u8>,
    /// Remote-desktop tab last reported visible to the engine.
    visible_desktop: Option<TabId>,
    /// Encrypted session database; `None` keeps sessions in eframe storage (tests).
    pub(crate) vault: Option<Arc<Vault>>,
    pub(crate) store_dialog: Option<StoreDialog>,
    /// A vault worker (Argon2, keyring) is running.
    pub(crate) vault_busy: bool,
    pub(crate) keyring_on: bool,
    vault_tx: mpsc::Sender<VaultMsg>,
    vault_rx: mpsc::Receiver<VaultMsg>,
    /// Sessions as last written to the database; `None` while locked.
    persisted: Option<SessionStore>,
    /// Sessions from eframe storage, moved into the database on the first unlock.
    legacy: Option<SessionStore>,
    /// `config.toml` path and the settings loaded from it (non-UI fields are written back as loaded).
    settings_file: Option<(PathBuf, rxt_store::settings::Settings)>,
    saved_settings: Settings,
    /// Pending agent approvals, oldest first.
    pub(crate) approvals: VecDeque<Approval>,
    pub(crate) audit: Option<AuditView>,
    /// Agent workspace chosen in Settings; `None` uses the default under the data directory.
    pub(crate) agent_workspace: Option<String>,
    /// Agent control state last handed to the backend.
    applied_agent: Option<(bool, PathBuf)>,
    /// Remote monitoring setting and interval last handed to the backend.
    applied_monitor: Option<(bool, u32)>,
    /// Unix seconds (injectable for tests).
    pub(crate) clock: fn() -> i64,
    /// A MobaXterm.ini found at startup to offer importing on first run; None once offered.
    pub(crate) moba_found: Option<std::path::PathBuf>,
}

impl App {
    /// Build the window state. `storage` restores settings and sessions saved by eframe.
    pub fn new(storage: Option<&dyn eframe::Storage>, platform: Platform, backend: impl Backend + 'static) -> Self {
        let settings: Settings = storage.and_then(|s| eframe::get_value(s, KEY_SETTINGS)).unwrap_or_default();
        let store: SessionStore = storage.and_then(|s| eframe::get_value(s, KEY_STORE)).unwrap_or_default();
        let agent_workspace = storage.and_then(|s| s.get_string(KEY_AGENT_WORKSPACE)).filter(|s| !s.is_empty());
        let system_dark = Arc::new(Mutex::new(platform.theme.prefers_dark()));
        let (dialog_tx, dialog_rx) = mpsc::channel();
        let (vault_tx, vault_rx) = mpsc::channel();
        let page = SidebarTab::of(&settings);
        let moba_found = (!settings.first_run_moba_offered)
            .then(|| rxt_store::moba_ini::find(&moba_dirs(&*platform.paths)))
            .flatten();
        Self {
            moba_found,
            saved_settings: settings.clone(),
            platform,
            backend: Box::new(backend),
            tokens: Tokens::for_dark(settings.theme.is_dark(None)),
            settings,
            store,
            tabs: Vec::new(),
            active: None,
            quick: String::new(),
            quick_error: None,
            quick_pick: None,
            sidebar_tab: page,
            filter: None,
            selected_session: None,
            system_dark,
            icons: Icons::default(),
            prompts: VecDeque::new(),
            modal: None,
            exit_confirmed: false,
            editor: None,
            show_settings: false,
            show_tunnels: false,
            tunnel_editor: None,
            tunnel_states: HashMap::new(),
            multiexec: false,
            sftp: SftpState::default(),
            parked_sftp: HashMap::new(),
            transfers: Vec::new(),
            last_dirs: HashMap::new(),
            window_focused: true,
            find: None,
            status: String::new(),
            dialog_tx,
            dialog_rx,
            ctx: None,
            fullscreen: false,
            focus_quick: false,
            open_menu: false,
            window_synced: false,
            gpu_start: None,
            diagnostics: None,
            diag_applied: None,
            diag_state: crate::diagnostics::DiagState::default(),
            stream_chip_clicked: false,
            stream: crate::diagnostics::StreamUi::default(),
            out_buf: Vec::new(),
            visible_desktop: None,
            vault: None,
            store_dialog: None,
            vault_busy: false,
            keyring_on: false,
            vault_tx,
            vault_rx,
            persisted: None,
            legacy: None,
            settings_file: None,
            approvals: VecDeque::new(),
            audit: None,
            agent_workspace,
            applied_agent: None,
            applied_monitor: None,
            clock: agent::unix_now,
        }
    }

    /// Use settings loaded from `path` (`config.toml` + `.env` + `RXT_*` + CLI) instead of eframe
    /// storage; load notices and errors are shown once the window opens.
    pub fn with_settings(mut self, loaded: rxt_store::settings::LoadOutcome, path: PathBuf) -> Self {
        self.settings = Settings::from_store(&loaded.settings);
        self.saved_settings = self.settings.clone();
        self.moba_found = (!self.settings.first_run_moba_offered)
            .then(|| rxt_store::moba_ini::find(&moba_dirs(&*self.platform.paths)))
            .flatten();
        self.sidebar_tab = SidebarTab::of(&self.settings);
        self.tokens = Tokens::for_dark(self.settings.theme.is_dark(None));
        let messages: Vec<String> =
            loaded.notices.into_iter().chain(loaded.errors.iter().map(ToString::to_string)).collect();
        if !messages.is_empty() {
            self.status = messages.join("; ");
            self.modal = Some(Modal::Notice(messages.join("\n\n")));
        }
        if let Some(dir) = path.parent() {
            theme::load_user_schemes(&dir.join("schemes"));
        }
        self.settings_file = Some((path, loaded.settings));
        self
    }

    /// Keep sessions in the encrypted database: locked until unlocked; sessions found in eframe
    /// storage are migrated on the first unlock.
    /// Hand over the crash-safe graphics start: its notice shows now; once frames painted, the
    /// marker is cleared and a working fallback backend is saved.
    pub fn with_gpu_start(mut self, start: GpuStart) -> Self {
        if let Some(notice) = &start.notice {
            self.status.clone_from(notice);
        }
        self.gpu_start = Some((start, 0));
        self
    }

    /// Hand over the binary's diagnostic log; Settings → Diagnostics drives it from now on.
    pub fn with_diagnostics(mut self, control: Box<dyn crate::diagnostics::DiagnosticsControl>) -> Self {
        self.diagnostics = Some(control);
        self.diag_applied = None;
        self
    }

    /// Applies changed diagnostics settings at once (log level, folder) and moves stream capture to
    /// the new folder.
    fn sync_diagnostics(&mut self) {
        self.poll_diagnostics();
        if self.diag_applied.as_ref() == Some(&self.settings.diagnostics) {
            return;
        }
        let Some(control) = &self.diagnostics else { return };
        self.diag_state = control.apply(&self.settings.diagnostics);
        self.diag_applied = Some(self.settings.diagnostics.clone());
        if self.stream.on {
            self.set_stream(true, self.stream.include_input);
        }
    }

    /// Writer failures stop what they affect and show on the page; the REC chip follows the real
    /// capture state.
    fn poll_diagnostics(&mut self) {
        if let Some(e) = self.diagnostics.as_ref().and_then(|c| c.poll_error()) {
            self.diag_state.error = Some(e);
            self.diag_state.file = None;
        }
        if self.stream.on {
            if let Some(e) = rxt_engine::stream::take_error() {
                let _ = rxt_engine::stream::configure(None);
                self.stream.error = Some(format!("stream capture stopped: {e}"));
            }
            self.stream.on = rxt_engine::stream::active();
        }
    }

    /// Deletes the diagnostic files: capture stops and the log closes first (written out, so no
    /// file is open while it is deleted), then both resume as they were.
    fn clear_diagnostics(&mut self) {
        let Some(dir) = self.diag_state.dir.clone() else { return };
        let was_on = self.stream.on;
        self.set_stream(false, self.stream.include_input);
        rxt_engine::stream::flush();
        if let Some(c) = &self.diagnostics {
            c.apply(&rxt_core::DiagSettings { level: rxt_core::DiagLevel::Off, ..self.settings.diagnostics.clone() });
            c.flush();
        }
        let keep = self.diag_state.file.clone().filter(|_| self.diag_state.forced);
        let (done, failed) = crate::diagnostics::clear(&dir, keep.as_deref());
        self.status = if failed == 0 {
            format!("Deleted {done} diagnostic file(s)")
        } else {
            format!("Deleted {done} diagnostic file(s); {failed} could not be deleted")
        };
        self.diag_applied = None;
        self.sync_diagnostics();
        if was_on {
            self.set_stream(true, self.stream.include_input);
        }
    }

    fn set_stream(&mut self, on: bool, include_input: bool) {
        self.stream.include_input = include_input;
        let dir = self.diag_state.dir.as_deref().map(crate::diagnostics::streams_dir);
        let cfg = match (on, dir) {
            (true, Some(dir)) => Some(rxt_engine::stream::StreamConfig { dir, include_input }),
            (true, None) => {
                let _ = rxt_engine::stream::configure(None);
                self.stream.on = false;
                self.stream.error = Some("no usable folder for the stream files".into());
                return;
            }
            (false, _) => None,
        };
        self.stream.on = cfg.is_some();
        self.stream.error = rxt_engine::stream::configure(cfg);
        if self.stream.error.is_some() {
            self.stream.on = false;
            let _ = rxt_engine::stream::configure(None);
        }
        tracing::info!(on = self.stream.on, include_input, "data-stream capture");
    }

    pub fn with_vault(mut self, vault: Arc<Vault>) -> Self {
        let old = std::mem::take(&mut self.store);
        if !(old.sessions.is_empty() && old.folders.is_empty() && old.tunnels.is_empty()) {
            self.legacy = Some(old);
        }
        self.vault = Some(vault);
        self
    }

    /// After [`GPU_START_FRAMES`] frames the graphics start counts as working.
    fn gpu_started(&mut self) {
        let Some((_, frames)) = self.gpu_start.as_mut() else { return };
        *frames += 1;
        if *frames < GPU_START_FRAMES {
            return;
        }
        let Some((start, _)) = self.gpu_start.take() else { return };
        if let Some(backend) = start.save {
            self.settings.gpu_backend = backend;
        }
        (start.on_started)();
    }

    /// Whether a session database exists but is not unlocked.
    pub(crate) fn locked(&self) -> bool {
        self.vault.as_ref().is_some_and(|v| !v.is_unlocked())
    }

    /// Whether the dark theme is applied.
    pub fn is_dark(&self) -> bool {
        self.tokens.dark
    }

    /// First frame: textures, visuals, live theme subscription, autostart tunnels.
    fn init(&mut self, ctx: &egui::Context) {
        self.ctx = Some(ctx.clone());
        self.icons = Icons::load(ctx, icons::override_dir_from_env().as_deref());
        ctx.set_fonts(theme::fonts());
        egui_extras::install_image_loaders(ctx);
        ctx.set_style_of(Theme::Light, theme::style(&Tokens::light()));
        ctx.set_style_of(Theme::Dark, theme::style(&Tokens::dark()));
        ctx.options_mut(|o| o.fallback_theme = Theme::Light);
        let shared = self.system_dark.clone();
        let repaint = ctx.clone();
        self.platform.theme.subscribe(Box::new(move |dark| {
            if let Ok(mut d) = shared.lock() {
                *d = dark;
            }
            repaint.request_repaint();
        }));
        self.apply_theme(ctx, true);
        match self.vault.clone() {
            None => self.autostart_tunnels(),
            Some(v) if v.is_new() => self.store_dialog = Some(StoreDialog::create()),
            Some(_) => self.spawn_vault(|v| VaultMsg::Opened(v.unlock_with_keyring())),
        }
    }

    fn autostart_tunnels(&mut self) {
        let autostart: Vec<String> =
            self.store.tunnels.iter().filter(|t| t.autostart).map(|t| t.name.clone()).collect();
        for name in autostart {
            self.apply(Action::StartTunnel(name));
        }
    }

    // ---- session database ----

    /// Run blocking vault work (Argon2, keyring, file IO) on a worker thread.
    fn spawn_vault(&mut self, f: impl FnOnce(&Vault) -> VaultMsg + Send + 'static) {
        let Some(vault) = self.vault.clone() else { return };
        let tx = self.vault_tx.clone();
        let ctx = self.ctx.clone();
        self.vault_busy = true;
        let spawned = std::thread::Builder::new().name("vault".into()).spawn(move || {
            let _ = tx.send(f(&vault));
            if let Some(c) = ctx {
                c.request_repaint();
            }
        });
        if let Err(e) = spawned {
            self.vault_busy = false;
            self.status = format!("Cannot start the unlock worker: {e}");
        }
    }

    fn on_vault(&mut self, msg: VaultMsg) {
        self.vault_busy = false;
        let fail = |e: rxt_core::Error| match e {
            rxt_core::Error::Auth(_) => "Wrong master password.".to_owned(),
            e => e.to_string(),
        };
        match msg {
            VaultMsg::Opened(Ok(true)) => {
                self.close_store_dialog();
                self.on_unlocked();
            }
            VaultMsg::Opened(Ok(false)) => {
                self.store_dialog.get_or_insert_with(StoreDialog::unlock);
            }
            VaultMsg::Opened(Err(e)) => self.store_dialog.get_or_insert_with(StoreDialog::unlock).set_error(fail(e)),
            VaultMsg::PasswordChanged(Ok(())) => {
                self.close_store_dialog();
                self.status = "Master password changed.".into();
            }
            VaultMsg::PasswordChanged(Err(e)) => {
                if let Some(d) = &mut self.store_dialog {
                    d.set_error(e.to_string());
                }
            }
            VaultMsg::KeyringSet(on, r) => match r {
                Ok(()) => self.keyring_on = on,
                Err(e) => self.status = format!("Keyring: {e}"),
            },
        }
    }

    fn close_store_dialog(&mut self) {
        if let Some(mut d) = self.store_dialog.take() {
            d.wipe();
        }
    }

    /// Load sessions from the freshly unlocked database, migrate eframe sessions, rebuild recents.
    fn on_unlocked(&mut self) {
        let Some(vault) = self.vault.clone() else { return };
        let loaded =
            vault.with(|s| Ok((s.sessions()?, s.folders()?, s.tunnels()?, s.history(0, 200)?, s.max_session_id()?)));
        let (sessions, folders, tunnels, history, max_session_id) = match loaded {
            Ok(l) => l,
            Err(e) => {
                self.status = format!("Cannot read the session database: {e}");
                return;
            }
        };
        // Include `max_session_id` (which counts rows skipped as unknown protocols) so a new
        // session never reuses and overwrites a skipped row's id.
        let next_id = sessions
            .iter()
            .map(|s| s.id.0)
            .chain(folders.iter().map(|f| f.id.0))
            .chain(std::iter::once(max_session_id))
            .max()
            .unwrap_or(0);
        let store = SessionStore { sessions, folders, tunnels, recent: Vec::new(), next_id };
        self.persisted = Some(store.clone());
        self.store = store;
        if let Some(old) = self.legacy.take() {
            let n = self.store.merge(&old.folders, &old.sessions, &old.tunnels);
            self.status = format!("Moved {n} saved session(s) into the encrypted database.");
        }
        for h in history.iter().rev() {
            let Some(target) = h.action.strip_prefix("open ") else { continue };
            let saved = h.session.and_then(|id| self.store.session(id)).cloned();
            if let Some(s) = saved.or_else(|| rxt_core::quick_connect::parse(target).ok()) {
                self.store.push_recent(&s);
            }
        }
        self.keyring_on = vault.keyring_enabled();
        self.sync_store();
        self.autostart_tunnels();
    }

    /// Write session, folder and tunnel changes made this frame to the database.
    fn sync_store(&mut self) {
        let (Some(vault), Some(old)) = (self.vault.clone(), self.persisted.as_ref()) else { return };
        let new = &self.store;
        if old.sessions == new.sessions && old.folders == new.folders && old.tunnels == new.tunnels {
            return;
        }
        let same_tunnel = |a: &rxt_core::Tunnel, b: &rxt_core::Tunnel| a.via == b.via && a.name == b.name;
        let r = vault.with(|db| {
            for f in new.folders.iter().filter(|f| !old.folders.contains(f)) {
                db.save_folder(f)?;
            }
            for s in new.sessions.iter().filter(|s| !old.sessions.contains(s)) {
                db.save_session(s)?;
            }
            for t in old.tunnels.iter().filter(|t| !new.tunnels.iter().any(|n| same_tunnel(n, t))) {
                db.delete_tunnel(t.via, &t.name)?;
            }
            for t in new.tunnels.iter().filter(|t| !old.tunnels.contains(t)) {
                db.save_tunnel(t)?;
            }
            for s in old.sessions.iter().filter(|s| !new.sessions.iter().any(|n| n.id == s.id)) {
                db.delete_session(s.id)?;
            }
            for f in old.folders.iter().filter(|f| !new.folders.iter().any(|n| n.id == f.id)) {
                db.delete_folder(f.id)?;
            }
            Ok(())
        });
        if let Err(e) = r {
            self.status = format!("Could not save sessions: {e}");
        }
        self.persisted = Some(self.store.clone());
    }

    /// Record an opened connection in the database history.
    fn history(&mut self, s: &Session) {
        let Some(vault) = self.vault.as_ref().filter(|v| v.is_unlocked()) else { return };
        // WSL targets live in `wsl` (host/port are empty); encode them so quick-connect can parse
        // the entry back when the session was not saved (its id is 0).
        let action = if s.protocol == Protocol::Wsl {
            format!("open wsl://{}", s.wsl.distribution.as_deref().unwrap_or_default())
        } else {
            let user = s.user.as_deref().map(|u| format!("{u}@")).unwrap_or_default();
            format!("open {}://{user}{}:{}", s.protocol.scheme(), s.host, s.port)
        };
        let id = (s.id.0 != 0).then_some(s.id);
        if let Err(e) = vault.with(|db| db.append_history(id, &action)) {
            tracing::warn!("history: {e}");
        }
    }

    /// Store actions need an unlocked database; otherwise open the create/unlock dialog.
    fn require_unlocked(&mut self) -> bool {
        if self.locked() {
            self.apply(Action::ShowUnlock);
            return false;
        }
        true
    }

    fn import_ini(&mut self, path: PathBuf) {
        match rxt_store::moba_ini::import_file(&path) {
            Ok(report) => self.store_dialog = Some(StoreDialog::Import { path, report }),
            Err(e) => self.modal = Some(Modal::Notice(format!("Cannot import {}: {e}", path.display()))),
        }
    }

    fn confirm_import(&mut self) {
        let Some(StoreDialog::Import { report, .. }) = self.store_dialog.take() else { return };
        for (folder, s) in &report.sessions {
            let folder = self.store.folder_by_path(folder);
            self.store.upsert(Session { id: SessionId(0), folder, ..s.clone() });
        }
        self.status = format!("Imported {} session(s).", report.sessions.len());
    }

    fn import_toml(&mut self, path: &std::path::Path) {
        let parsed = std::fs::read_to_string(path)
            .map_err(rxt_core::Error::from)
            .and_then(|text| rxt_store::export::Export::from_toml(&text));
        match parsed {
            Ok(x) => {
                let n = self.store.merge(&x.folders, &x.sessions, &x.tunnels);
                self.status = format!("Imported {n} session(s) from {}.", path.display());
                if x.skipped_local > 0 {
                    self.status +=
                        &format!(" Skipped {} local terminal entry(ies): they are never saved.", x.skipped_local);
                }
            }
            Err(e) => self.modal = Some(Modal::Notice(format!("Cannot import {}: {e}", path.display()))),
        }
    }

    fn confirm_export(&mut self) {
        let Some(StoreDialog::Export { path, .. }) = self.store_dialog.take() else { return };
        let st = &self.store;
        let export = rxt_store::export::Export::new(st.folders.clone(), st.sessions.clone(), st.tunnels.clone());
        let written = export.to_toml().and_then(|text| std::fs::write(&path, text).map_err(rxt_core::Error::from));
        self.status = match written {
            Ok(()) => format!("Exported {} session(s) to {}.", st.sessions.len(), path.display()),
            Err(e) => format!("Could not export to {}: {e}", path.display()),
        };
    }

    fn apply_theme(&mut self, ctx: &egui::Context, force: bool) {
        let system = self.system_dark.lock().map(|d| *d).unwrap_or(None);
        let dark = self.settings.theme.is_dark(system);
        if force || dark != self.tokens.dark {
            self.tokens = Tokens::for_dark(dark);
            ctx.set_theme(if dark { Theme::Dark } else { Theme::Light });
        }
    }

    /// Draw one frame and apply the resulting actions.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if self.ctx.is_none() {
            // Fonts and text styles registered here apply from the next pass; draw nothing now.
            self.init(&ctx);
            ctx.request_repaint();
            return;
        }
        self.sync_diagnostics();
        self.pump(&ctx);
        self.confirm_exit(&ctx);
        self.apply_theme(&ctx, false);
        ctx.set_zoom_factor(self.settings.ui_scale.zoom_factor(ctx.native_pixels_per_point()));
        self.sync_window_state(&ctx);
        let mut out = self.shortcuts(&ctx);
        self.layout(ui, &mut out);
        self.windows(&ctx, &mut out);
        theme::focus_ring(&ctx, &self.tokens);
        self.dropped_files(&ctx, &mut out);
        for a in out {
            self.apply(a);
        }
        self.sync_store();
        self.sync_sftp();
        self.sync_visible();
        self.gpu_started();
        self.sync_agent();
        self.sync_monitor();
    }

    /// Hand the remote monitoring settings to the backend when they change.
    fn sync_monitor(&mut self) {
        let want = (self.settings.remote_monitoring, self.settings.remote_monitoring_interval_secs.clamp(1, 30));
        if self.applied_monitor != Some(want) {
            self.backend.set_remote_monitoring(want.0);
            self.send(Command::SetMonitorInterval { secs: want.1 });
            self.applied_monitor = Some(want);
        }
    }

    /// Effective agent workspace directory.
    pub(crate) fn agent_workspace_dir(&self) -> PathBuf {
        match &self.agent_workspace {
            Some(dir) => PathBuf::from(dir.trim()),
            None => self.platform.paths.data_dir().join("agent-files"),
        }
    }

    /// Hand agent control changes to the backend; deny approvals past their deadline.
    fn sync_agent(&mut self) {
        let want = (self.settings.agent_control, self.agent_workspace_dir());
        if self.applied_agent.as_ref() != Some(&want) {
            // Windows: owner-only ACL on the workspace (no-op on Unix, which uses 0700 there).
            // ponytail: logged only; the engine still refuses paths outside the workspace.
            if want.0
                && let Err(e) = self.platform.paths.make_private(&want.1)
            {
                tracing::warn!("agent workspace: {e}");
            }
            self.backend.set_agent_workspace(&want.1);
            self.backend.set_agent_control(want.0);
            self.applied_agent = Some(want);
        }
        let now = (self.clock)();
        let expired: Vec<_> = self.approvals.iter().filter(|a| a.expires_at <= now).map(|a| a.id).collect();
        for id in expired {
            self.answer_approval(id, false);
        }
        if !self.approvals.is_empty()
            && let Some(ctx) = &self.ctx
        {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn answer_approval(&mut self, id: rxt_core::ApprovalId, approve: bool) {
        self.approvals.retain(|a| a.id != id);
        self.send(Command::AnswerApproval { id, approve });
    }

    /// Reload the visible audit page from the database.
    fn load_audit(&mut self, page: usize) {
        let Some(view) = self.audit.as_mut() else { return };
        let Some(vault) = self.vault.as_ref().filter(|v| v.is_unlocked()) else { return };
        view.confirm_clear = false;
        match vault.with(|db| db.audit(page * agent::AUDIT_PAGE, agent::AUDIT_PAGE + 1)) {
            Ok(mut records) => {
                view.more = records.len() > agent::AUDIT_PAGE;
                records.truncate(agent::AUDIT_PAGE);
                view.records = records;
                view.page = page;
                view.error = None;
            }
            Err(e) => view.error = Some(format!("Cannot read the audit log: {e}")),
        }
    }

    /// Hidden remote desktops stop requesting updates: report the active one as visible.
    fn sync_visible(&mut self) {
        let now = self.active_tab().filter(|t| t.desktop.is_some()).map(|t| t.id);
        if now == self.visible_desktop {
            return;
        }
        if let Some(prev) = self.visible_desktop.filter(|p| self.tabs.iter().any(|t| t.id == *p)) {
            self.send(Command::SetVisible { tab: prev, visible: false });
        }
        if let Some(tab) = now {
            self.send(Command::SetVisible { tab, visible: true });
        }
        self.visible_desktop = now;
    }

    // ---- engine ----

    fn send(&mut self, command: Command) {
        if let Err(e) = self.backend.send(command) {
            self.status = e.to_string();
        }
    }

    fn pump(&mut self, ctx: &egui::Context) {
        self.window_focused = ctx.input(|i| i.focused);
        for _ in 0..EVENT_BUDGET {
            let Some(ev) = self.backend.try_event() else { break };
            self.on_event(ev);
        }
        while let Ok(r) = self.dialog_rx.try_recv() {
            self.on_dialog(r);
        }
        while let Ok(m) = self.vault_rx.try_recv() {
            self.on_vault(m);
        }
        self.drain_output(ctx);
    }

    /// Feed buffered output to terminals within a byte budget; the visible tab gets the largest share.
    fn drain_output(&mut self, ctx: &egui::Context) {
        let mut order = self.backend.pending_output();
        if order.is_empty() {
            return;
        }
        // Background tabs share a reserved part of the budget (half when the visible tab has output)
        // so a flooding visible tab cannot starve them; the visible tab, drained last, gets the rest.
        let visible = self.active.and_then(|a| order.iter().position(|t| *t == a)).map(|pos| order.remove(pos));
        let reserved = if visible.is_some() { OUTPUT_BUDGET / 2 } else { OUTPUT_BUDGET };
        let slice = (reserved / order.len().max(1)).max(1);
        order.extend(visible);
        let mut budget = OUTPUT_BUDGET;
        let mut buf = std::mem::take(&mut self.out_buf);
        let mut replies = Vec::new();
        let mut cwds = Vec::new();
        for tab in order {
            if budget == 0 {
                break;
            }
            let cap = if Some(tab) == visible { budget } else { budget.min(slice) };
            buf.clear();
            budget = budget.saturating_sub(self.backend.drain_output(tab, cap, &mut buf));
            let Some(term) = self.tabs.iter_mut().find(|t| t.id == tab).and_then(|t| t.term.as_mut()) else { continue };
            for ev in term.feed(&buf) {
                match ev {
                    rxt_term::TermEvent::Reply(bytes) => replies.push(Command::Reply { tab, bytes }),
                    rxt_term::TermEvent::CwdChanged(path) => cwds.push((tab, path)),
                    rxt_term::TermEvent::Title(_) | rxt_term::TermEvent::Bell => {}
                }
            }
        }
        buf.clear();
        self.out_buf = buf;
        replies.into_iter().for_each(|c| self.send(c));
        cwds.into_iter().for_each(|(tab, path)| self.on_cwd(tab, path));
        // The engine waker is coalesced: keep polling while output is pending.
        ctx.request_repaint();
    }

    pub(crate) fn on_event(&mut self, ev: Event) {
        match ev {
            Event::ApprovalRequest { id, client, command, session, target, expires_at } => {
                if !self.approvals.iter().any(|a| a.id == id) {
                    self.approvals.push_back(Approval { id, client, command, session, target, expires_at });
                }
            }
            Event::TabOpened { tab, session } => {
                if !self.tabs.iter().any(|t| t.id == tab) {
                    let scrollback =
                        session.terminal.resolve(&self.settings.terminal).scrollback_lines.unwrap_or(10_000);
                    let mut t = Tab::new(tab, *session, scrollback);
                    if let Some(f) = t.files.as_mut() {
                        let home = self.platform.paths.home_dir();
                        file_tab::local_action(&mut f.local, SftpAction::Navigate(HOME.into()), &home);
                    }
                    // Local terminals never log (terminal-emulation spec, Session logging).
                    let log = t.session.terminal.log_file.clone().filter(|_| t.session.protocol != Protocol::Local);
                    if let (Some(term), Some(path)) = (t.term.as_mut(), log.as_deref()) {
                        let format = if t.session.terminal.log_raw { LogFormat::Raw } else { LogFormat::Plain };
                        if let Err(e) = term.start_log(std::path::Path::new(path), format) {
                            self.status = format!("Could not open log file {path}: {e}");
                        }
                    }
                    self.tabs.push(t);
                    self.active = Some(tab);
                }
            }
            Event::State { tab, state } => {
                let connected = state == ConnState::Connected;
                let was_connected = self.tabs.iter().any(|t| t.id == tab && t.state == ConnState::Connected);
                if was_connected && matches!(state, ConnState::Disconnected { .. } | ConnState::Error(_)) {
                    // User closes and reconnects cancel the session without a state event, so
                    // a drop from Connected was not asked for.
                    let body = format!("{} disconnected", self.tab_title(tab));
                    self.notify("Session disconnected", &body);
                }
                let stopped = state.clone();
                let mut resize = None;
                let mut list = None;
                if connected {
                    self.print_banner(tab, None);
                }
                if let Some(t) = self.tab_mut(tab) {
                    if state == ConnState::Connected {
                        resize = t.term.as_ref().map(|term| (term.cols() as u16, term.rows() as u16));
                        if t.files.as_ref().is_some_and(|f| f.remote.path.is_empty()) {
                            let start =
                                t.session.ftp.startup_folder.clone().filter(|_| t.session.protocol != Protocol::Sftp);
                            list = Some(start.unwrap_or_else(|| HOME.into()));
                        }
                    }
                    if state != ConnState::Connected {
                        // A new login starts a new shell and browser, and new monitoring.
                        t.cwd = None;
                        t.monitor = Default::default();
                        // A stale WSL startup offer must not survive into the next run (its engine
                        // receiver is gone, so its buttons would do nothing).
                        t.wsl_startup_pending = false;
                    }
                    t.state = state;
                }
                if !connected {
                    self.parked_sftp.remove(&tab);
                    // The session's last output goes above the notice, whatever the frame budget.
                    self.flush_output(tab);
                    self.print_stopped(tab, &stopped);
                }
                if let Some(path) = list {
                    self.list(tab, path);
                }
                if connected && self.status.starts_with("Connecting to ") {
                    self.status.clear();
                }
                if let Some((cols, rows)) = resize {
                    self.send(Command::Resize { tab, cols, rows });
                }
            }
            Event::LocalReady { tab, info } => {
                self.print_banner(tab, Some(&info));
                let start = self.tab_mut(tab).and_then(|t| t.term.as_ref()).map(|term| {
                    let cursor = term.renderable().cursor.point;
                    let cell = |v: usize| u16::try_from(v).unwrap_or(u16::MAX);
                    let cursor = (cell(usize::try_from(cursor.line.0).unwrap_or(0)), cell(cursor.column.0));
                    (cell(term.cols()), cell(term.rows()), cursor)
                });
                if let Some((cols, rows, cursor)) = start {
                    self.send(Command::LocalStart { tab, cols, rows, cursor });
                }
            }
            Event::SshInfo { tab, info } => {
                if let Some(t) = self.tab_mut(tab) {
                    t.no_sftp = info.sftp == Some(false);
                    t.ssh_info = Some(info);
                }
            }
            Event::Output { .. } => {}
            Event::Prompt { id, tab, kind } => self.prompts.push_back(Prompt::new(id, tab, kind)),
            Event::HostKeyChanged { host, old, new, .. } => self.modal = Some(Modal::HostKeyChanged { host, old, new }),
            Event::CwdChanged { tab, path } => self.on_cwd(tab, path),
            Event::Listing { tab, path, entries } => {
                if let Some(st) = self.remote_for(tab) {
                    st.listed(path, entries);
                }
            }
            Event::TransferStarted { id, tab, direction, remote, local, total } => {
                let row = TransferRow {
                    id,
                    tab,
                    direction,
                    remote,
                    local,
                    total,
                    done: 0,
                    speed: 0.0,
                    last: Instant::now(),
                    result: None,
                };
                match self.transfers.iter_mut().find(|t| t.id == id) {
                    Some(t) => {
                        t.remote = row.remote;
                        t.local = row.local;
                        t.total = row.total.or(t.total);
                    }
                    None => self.transfers.push(row),
                }
            }
            Event::TransferProgress { id, done } => {
                if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
                    t.progress(done, Instant::now());
                }
            }
            Event::TransferFinished { id, result } => {
                if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
                    if let Ok(n) = result {
                        t.done = n;
                    }
                    let (tab, upload) = (t.tab, t.direction == Direction::Upload);
                    t.result = Some(result.map_err(|e| e.to_string()));
                    let (title, name) = transfer_note(t);
                    self.notify(title, &format!("{name} in {}", self.tab_title(tab)));
                    if upload {
                        if let Some(path) = self.remote_for(tab).map(|st| st.path.clone()) {
                            self.list(tab, path);
                        }
                    } else if let Some(f) = self.tab_mut(tab).and_then(|t| t.files.as_mut()) {
                        file_tab::local_action(&mut f.local, SftpAction::Refresh, std::path::Path::new("/"));
                    }
                }
            }
            Event::TunnelState { name, state } => {
                self.tunnel_states.insert(name, state);
            }
            Event::MultiExec { enabled, selected } => {
                self.multiexec = enabled;
                for t in &mut self.tabs {
                    t.multiexec = selected.contains(&t.id);
                }
            }
            Event::Notice { tab, message } => {
                if let Some(st) = tab.and_then(|t| self.remote_for(t)).filter(|st| st.loading) {
                    st.loading = false;
                    st.error = Some(message.clone());
                }
                self.status = message;
            }
            Event::SerialPorts { result } => {
                if let Some(ed) = &mut self.editor {
                    match result {
                        Ok(ports) => {
                            ed.serial_ports = ports;
                            ed.ports_note = ed.serial_ports.is_empty().then(|| "No serial ports found.".into());
                        }
                        Err(e) => ed.ports_note = Some(e.to_string()),
                    }
                }
            }
            Event::WslDistributions { status } => {
                if let Some(ed) = &mut self.editor {
                    ed.wsl_status = Some(status);
                }
            }
            Event::WslStartupPending { tab } => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab) {
                    t.wsl_startup_pending = true;
                }
            }
            // The sample is taken while the bar is painted; the event only asks for a frame.
            Event::Metrics { .. } => {}
            Event::Monitor { tab, state } => {
                if let Some(t) = self.tab_mut(tab) {
                    t.monitor.set_state(state);
                }
            }
            Event::Closed { tab } => self.forget_tab(tab),
        }
    }

    /// Terminal folder from OSC 7 or the engine's shell-PID fallback; a browser that follows
    /// its terminal lists it.
    fn on_cwd(&mut self, tab: TabId, path: String) {
        let Some(t) = self.tab_mut(tab) else { return };
        t.cwd = Some(path.clone());
        if self.remote_for(tab).is_some_and(|st| st.follow && st.path != path) {
            self.list(tab, path);
        }
    }

    fn on_dialog(&mut self, r: DialogResult) {
        if let Some((kind, dir)) = r.folder() {
            self.last_dirs.insert(kind, dir);
        }
        match r {
            DialogResult::Upload { tab, dir, files } => self.upload(tab, &dir, files),
            DialogResult::Download { tab, remote, local } => {
                let local = local.to_string_lossy().into_owned();
                self.send(Command::Transfer { tab, direction: Direction::Download, remote, local });
            }
            DialogResult::KeyFile(path) => {
                if let Some(ed) = &mut self.editor {
                    ed.session.ssh.key_file = Some(path.to_string_lossy().into_owned());
                }
            }
            DialogResult::SaveText(path, text) => {
                self.status = match std::fs::write(&path, text) {
                    Ok(()) => format!("Saved {}", path.display()),
                    Err(e) => format!("Could not save {}: {e}", path.display()),
                };
            }
            DialogResult::ImportIni(path) => self.import_ini(path),
            DialogResult::ImportToml(path) => self.import_toml(&path),
            DialogResult::AgentWorkspace(path) => self.agent_workspace = Some(path.to_string_lossy().into_owned()),
            DialogResult::DiagFolder(path) => self.settings.diagnostics.folder = Some(path),
            DialogResult::ExportToml(path) => {
                let st = &self.store;
                let (sessions, folders, tunnels) = (st.sessions.len(), st.folders.len(), st.tunnels.len());
                self.store_dialog = Some(StoreDialog::Export { path, sessions, folders, tunnels });
            }
        }
    }

    /// Run a blocking file dialog on a worker thread; the result arrives in a later frame.
    /// `f` gets the folder to open in (see [`start_dir`]).
    fn spawn_dialog(
        &self,
        kind: DialogKind,
        f: impl FnOnce(&dyn FileDialogs, &Path) -> Option<DialogResult> + Send + 'static,
    ) {
        let paths = &self.platform.paths;
        let start = start_dir(self.last_dirs.get(&kind).map(PathBuf::as_path), paths.download_dir(), paths.home_dir());
        let dialogs = self.platform.dialogs.clone();
        let tx = self.dialog_tx.clone();
        let ctx = self.ctx.clone();
        let spawned = std::thread::Builder::new().name("file-dialog".into()).spawn(move || {
            if let Some(r) = f(dialogs.as_ref(), &start) {
                let _ = tx.send(r);
            }
            if let Some(c) = ctx {
                c.request_repaint();
            }
        });
        if let Err(e) = spawned {
            tracing::warn!("file dialog thread: {e}");
        }
    }

    fn upload(&mut self, tab: TabId, dir: &str, files: Vec<PathBuf>) {
        for local in files {
            let Some(name) = local.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
            let remote = join(dir, &name);
            self.send(Command::Transfer {
                tab,
                direction: Direction::Upload,
                remote,
                local: local.to_string_lossy().into_owned(),
            });
        }
    }

    fn list(&mut self, tab: TabId, path: String) {
        if let Some(st) = self.remote_for(tab) {
            st.loading = true;
            st.error = None;
            st.path_edit.clone_from(&path);
        }
        self.send(Command::List { tab, path });
    }

    /// Remote browser state attached to `tab`: the side panel or the tab's remote pane.
    fn remote_for(&mut self, tab: TabId) -> Option<&mut SftpState> {
        if self.sftp.tab == Some(tab) {
            return Some(&mut self.sftp);
        }
        if self.parked_sftp.contains_key(&tab) {
            return self.parked_sftp.get_mut(&tab);
        }
        self.tab_mut(tab).and_then(|t| t.files.as_mut()).map(|f| &mut f.remote)
    }

    /// Tab whose remote browser receives [`Action::Sftp`]: the active file tab, else the side panel's.
    fn remote_tab(&self) -> Option<TabId> {
        match self.active_tab() {
            Some(t) if t.files.is_some() => Some(t.id),
            _ => self.sftp.tab,
        }
    }

    /// Attach the SFTP panel to the active connected SSH tab, keeping one browser state per
    /// connected tab. With `sftp_auto_switch`, the SFTP page follows: shown (and the sidebar
    /// opened on the first attach after a login) for a connected SSH tab, Sessions otherwise.
    fn sync_sftp(&mut self) {
        let target = self
            .active_tab()
            .filter(|t| t.session.protocol == Protocol::Ssh && t.state == ConnState::Connected && !t.no_sftp)
            .map(|t| (t.id, t.session.ssh.follow_terminal_folder, t.cwd.clone()));
        if target.as_ref().map(|t| t.0) == self.sftp.tab {
            return;
        }
        let shown = std::mem::take(&mut self.sftp);
        if let Some(id) =
            shown.tab.filter(|id| self.tabs.iter().any(|t| t.id == *id && t.state == ConnState::Connected))
        {
            self.parked_sftp.insert(id, shown);
        }
        let auto = self.settings.sftp_auto_switch;
        let Some((tab, follow, cwd)) = target else {
            if auto && self.sidebar_tab == SidebarTab::Sftp {
                self.sidebar_tab = SidebarTab::Sessions;
            }
            return;
        };
        if auto {
            self.sidebar_tab = SidebarTab::Sftp;
        }
        if let Some(st) = self.parked_sftp.remove(&tab) {
            self.sftp = st;
            return;
        }
        // First attach since the login.
        self.sftp = SftpState { tab: Some(tab), follow, ..SftpState::default() };
        if auto {
            self.settings.sidebar_open = true;
        }
        self.list(tab, cwd.filter(|_| follow).unwrap_or_else(|| HOME.into()));
    }

    // ---- tabs ----

    fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    /// Tab name for notifications.
    fn tab_title(&self, id: TabId) -> String {
        self.tabs.iter().find(|t| t.id == id).map(|t| t.title.clone()).unwrap_or_default()
    }

    /// Desktop notification while the window is in the background and notifications are on.
    /// Sent off the UI thread; failures (service missing or turned off) are logged at debug only.
    fn notify(&self, title: &'static str, body: &str) {
        if self.window_focused || !self.settings.notifications {
            return;
        }
        let (notifier, body) = (self.platform.notifier.clone(), body.to_owned());
        let spawned = std::thread::Builder::new().name("notify".into()).spawn(move || {
            if let Err(e) = notifier.notify(title, &body) {
                tracing::debug!("notification not shown: {e}");
            }
        });
        if let Err(e) = spawned {
            tracing::debug!("notification thread: {e}");
        }
    }

    /// Open a local file with its default application, or (`folder`) the folder containing it.
    /// A missing file or an opener failure is reported in the status bar.
    fn open_local(&mut self, path: &Path, folder: bool) {
        let target = if folder { path.parent().unwrap_or(path) } else { path };
        if !path.exists() {
            self.status = format!("Cannot open {}: the file no longer exists", path.display());
            return;
        }
        if let Err(e) = self.platform.opener.open(target) {
            self.status = format!("Cannot open {}: {e}", target.display());
        }
    }

    pub(crate) fn active_tab(&self) -> Option<&Tab> {
        self.active.and_then(|id| self.tabs.iter().find(|t| t.id == id))
    }

    /// Closing the window (Exit, the title bar or the window manager) with connected tabs asks first.
    fn confirm_exit(&mut self, ctx: &egui::Context) {
        let ask = self.settings.confirm_close && !self.exit_confirmed && self.tabs.iter().any(Tab::is_live);
        if ask && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.modal = Some(Modal::ConfirmExit);
        }
    }

    fn request_close(&mut self, id: TabId) {
        let live = self.tabs.iter().any(|t| t.id == id && t.is_live());
        if live && self.settings.confirm_close {
            self.modal = Some(Modal::ConfirmClose(id));
        } else {
            self.close_tab(id);
        }
    }

    fn close_tab(&mut self, id: TabId) {
        if self.tabs.iter().any(|t| t.id == id) {
            self.send(Command::Close { tab: id });
        }
        self.forget_tab(id);
    }

    /// Drop local state for a tab, including its pending prompts.
    fn forget_tab(&mut self, id: TabId) {
        let Some(pos) = self.tabs.iter().position(|t| t.id == id) else { return };
        let closed = self.tabs.remove(pos);
        // Approvals name the session; drop them once no tab of that session is left.
        if !self.tabs.iter().any(|t| t.session.name == closed.session.name) {
            let stale: Vec<_> =
                self.approvals.iter().filter(|a| a.session == closed.session.name).map(|a| a.id).collect();
            stale.into_iter().for_each(|a| self.answer_approval(a, false));
        }
        self.prompts.retain(|p| p.tab != Some(id));
        if self.active == Some(id) {
            self.active = self.tabs.get(pos.min(self.tabs.len().saturating_sub(1))).map(|t| t.id);
        }
        self.parked_sftp.remove(&id);
        if self.visible_desktop == Some(id) {
            self.visible_desktop = None;
        }
    }

    /// Session info box of a newly connected SSH or Telnet tab, fed to its terminal before any
    /// remote output (Settings → SSH → Show session banner).
    /// Session box of SSH/Telnet tabs (on `Connected`) and the welcome box of local terminals
    /// (on `LocalReady`, with `local`).
    fn print_banner(&mut self, tab: TabId, local: Option<&rxt_core::LaunchInfo>) {
        if !self.settings.show_session_banner {
            return;
        }
        // Every bundled monospace family falls back to DejaVu Sans Mono, which has the box and
        // check glyphs (egui's `has_glyph` misreports glyphs of the primary face).
        let unicode = true;
        let sep = if unicode { " • " } else { " | " };
        let mut hint = "Ctrl+wheel, Ctrl+=/-/0: zoom".to_owned();
        if let Some(find) = self.settings.shortcut_text(CommandId::FIND) {
            hint = format!("{hint}{sep}{find}: find");
        }
        let mut tips: Vec<String> = Vec::new();
        if let Some(new) = self.settings.shortcut_text(CommandId::NEW_SESSION) {
            tips.push(format!("{new}: new session"));
        }
        tips.push("Ctrl+right click: terminal menu".into());
        let Some(pos) = self.tabs.iter().position(|t| t.id == tab) else { return };
        let config = self.view_config(&self.tabs[pos].session);
        let t = &mut self.tabs[pos];
        let wanted = match t.session.protocol {
            Protocol::Ssh | Protocol::Telnet => local.is_none(),
            // WSL, like a local terminal, starts via `LocalReady`: its banner prints then.
            Protocol::Local | Protocol::Wsl => local.is_some(),
            _ => false,
        };
        if !wanted {
            return;
        }
        let ssh = t.ssh_info.take();
        let Some(term) = t.term.as_mut() else { return };
        let info = banner::BannerInfo { session: &t.session, ssh: ssh.as_deref(), local, tips, hint, unicode };
        let mut bytes = banner::render(&info, term.cols(), &config.scheme);
        // A reconnect may leave the cursor after the old prompt.
        if term.renderable().cursor.point.column.0 != 0 {
            bytes.splice(0..0, *b"\r\n");
        }
        term.feed(&bytes);
    }

    /// Feed all of `tab`'s buffered output to its terminal now (before the stopped notice).
    fn flush_output(&mut self, tab: TabId) {
        let mut buf = Vec::new();
        let mut replies = Vec::new();
        // ponytail: bounded at 64 MiB; a stopped session produces no more output.
        for _ in 0..64 {
            buf.clear();
            if self.backend.drain_output(tab, 1 << 20, &mut buf) == 0 {
                break;
            }
            let Some(term) = self.tab_mut(tab).and_then(|t| t.term.as_mut()) else { return };
            for ev in term.feed(&buf) {
                if let rxt_term::TermEvent::Reply(bytes) = ev {
                    replies.push(Command::Reply { tab, bytes });
                }
            }
        }
        replies.into_iter().for_each(|c| self.send(c));
    }

    /// Write the "Session stopped" notice into a terminal tab whose session just ended.
    fn print_stopped(&mut self, tab: TabId, state: &ConnState) {
        let (why, next) = match state {
            ConnState::Disconnected { reason, reconnect_at } => {
                (reason.as_str(), reconnect_at.map(|at| format!("{} UTC", sftp_panel::format_time(at))))
            }
            ConnState::Error(e) => (e.as_str(), None),
            _ => return,
        };
        let Some(pos) = self.tabs.iter().position(|t| t.id == tab) else { return };
        let config = self.view_config(&self.tabs[pos].session);
        let Some(term) = self.tabs[pos].term.as_mut() else { return };
        let mut bytes = banner::stopped(term.cols(), &config.scheme, why, next.as_deref());
        if term.renderable().cursor.point.column.0 == 0 {
            bytes.drain(..2);
        }
        term.feed(&bytes);
    }

    fn view_config(&self, s: &Session) -> ViewConfig {
        let prefs = s.terminal.resolve(&self.settings.terminal);
        let scheme = theme::scheme(prefs.scheme.as_deref(), self.tokens.dark);
        ViewConfig {
            prefs,
            scheme,
            copy_on_select: self.settings.copy_on_select,
            right_click_paste: self.settings.right_click_paste,
            paste_guard: self.settings.paste_guard,
        }
    }

    /// Paste clipboard text into `tab` through the terminal (bracketed paste, guard).
    fn paste_into(&mut self, id: TabId, text: &str) {
        let Some(idx) = self.tabs.iter().position(|t| t.id == id) else { return };
        let cfg = self.view_config(&self.tabs[idx].session);
        let Some(term) = self.tabs[idx].term.as_ref() else { return };
        let mut resp = terminal::ViewResponse::default();
        terminal::paste(term, text, &cfg, &mut resp);
        self.after_view(id, resp);
    }

    fn after_view(&mut self, tab: TabId, resp: terminal::ViewResponse) {
        if !resp.input.is_empty() {
            self.send(Command::Input { tab, bytes: resp.input });
        }
        if let Some((cols, rows)) = resp.resized {
            self.send(Command::Resize { tab, cols, rows });
        }
        if let Some(text) = resp.paste_request {
            self.modal = Some(Modal::Paste { tab, text });
        }
    }

    // ---- actions ----

    /// Run an app-level command on the active tab.
    pub(crate) fn run(&mut self, cmd: CommandId) {
        let active = self.active;
        match cmd {
            CommandId::NEW_SESSION => {
                if self.require_unlocked() {
                    self.apply(Action::NewSession(None));
                }
            }
            CommandId::NEW_LOCAL_TERMINAL => {
                if let Some(pty) = self.platform.local_pty.clone() {
                    // No vault needed: nothing is saved. Named `user@host` like the tab title.
                    let mut s = Session::new(Protocol::Local, "localhost", None, None);
                    s.name = "Local terminal".into();
                    if let Ok(info) = pty.prepare() {
                        s.name = format!("{}@{}", info.user, info.host);
                        (s.host, s.user) = (info.host, Some(info.user));
                    }
                    self.apply(Action::Open(Box::new(s)));
                }
            }
            CommandId::QUICK_CONNECT => self.quick_connect(),
            CommandId::CLOSE_TAB => active.into_iter().for_each(|t| self.request_close(t)),
            CommandId::CLOSE_OTHERS => {
                let Some(keep) = active else { return };
                let others_live = self.tabs.iter().any(|t| t.id != keep && t.is_live());
                if others_live && self.settings.confirm_close {
                    self.modal = Some(Modal::ConfirmCloseOthers(keep));
                } else {
                    self.apply(Action::CloseOthers(keep));
                }
            }
            CommandId::NEXT_TAB | CommandId::PREV_TAB => {
                let ids: Vec<Option<TabId>> =
                    std::iter::once(None).chain(self.tabs.iter().map(|t| Some(t.id))).collect();
                let pos = ids.iter().position(|i| *i == active).unwrap_or(0);
                let step = if cmd == CommandId::NEXT_TAB { 1 } else { ids.len() - 1 };
                self.active = ids[(pos + step) % ids.len()];
            }
            CommandId::RENAME_TAB => {
                if let Some(t) = self.active_tab() {
                    self.modal = Some(Modal::RenameTab(t.id, t.title.clone()));
                }
            }
            CommandId::DUPLICATE_TAB => active.into_iter().for_each(|tab| self.send(Command::Duplicate { tab })),
            CommandId::RECONNECT => {
                if let Some(t) = active.and_then(|id| self.tab_mut(id)).filter(|t| !t.is_live()) {
                    t.state = ConnState::Connecting;
                    let tab = t.id;
                    self.send(Command::Reconnect { tab });
                }
            }
            CommandId::FIND => {
                let has_term = self.active_tab().is_some_and(|t| t.term.is_some());
                self.find = if self.find.is_none() && has_term { Some(FindBar::default()) } else { None };
            }
            CommandId::COPY => {
                let text = self.active_tab().and_then(|t| t.term.as_ref()).and_then(|t| t.selection_text());
                if let Some(text) = text
                    && let Err(e) = self.platform.clipboard.set_text(&text)
                {
                    self.status = e.to_string();
                }
            }
            CommandId::PASTE => {
                if let Some(tab) = active {
                    match self.platform.clipboard.get_text() {
                        Ok(text) => self.paste_into(tab, &text),
                        Err(e) => self.status = e.to_string(),
                    }
                }
            }
            CommandId::SAVE_TEXT => {
                if let Some(text) = self.active_tab().and_then(|t| t.term.as_ref()).map(|t| t.visible_text()) {
                    self.spawn_dialog(DialogKind::SaveText, move |d, start| {
                        d.save_file("Save terminal text", "terminal.txt", start)
                            .map(|p| DialogResult::SaveText(p, text))
                    });
                }
            }
            CommandId::SETTINGS => self.show_settings = !self.show_settings,
            CommandId::TOGGLE_SIDEBAR => self.settings.sidebar_open = !self.settings.sidebar_open,
            CommandId::TOGGLE_FULLSCREEN => {
                self.fullscreen = !self.fullscreen;
                if let Some(ctx) = &self.ctx {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
                }
            }
            CommandId::MULTIEXEC => {
                self.multiexec = !self.multiexec;
                let enabled = self.multiexec;
                self.send(Command::SetMultiExec { enabled });
            }
            CommandId::TUNNELS => self.show_tunnels = !self.show_tunnels,
            HELP | CommandId::ABOUT => self.modal = Some(Modal::About),
            CommandId::IMPORT_SESSIONS => self.apply(Action::ImportIni),
            CommandId::EXPORT_SESSIONS => self.apply(Action::ExportToml),
            CommandId::CHANGE_PASSWORD => self.apply(Action::ShowChangePassword),
            CommandId::THEME_LIGHT | CommandId::THEME_DARK | CommandId::THEME_SYSTEM => {
                self.settings.theme = match cmd {
                    CommandId::THEME_LIGHT => ThemeMode::Light,
                    CommandId::THEME_DARK => ThemeMode::Dark,
                    _ => ThemeMode::System,
                };
                // An explicit choice answers the first-run chooser too.
                self.settings.first_run_theme_done = true;
                if let Some(ctx) = self.ctx.clone() {
                    self.apply_theme(&ctx, false);
                }
            }
            CommandId::FOCUS_MENU => self.open_menu = true,
            CommandId::FIND_SESSION => {
                self.settings.sidebar_open = true;
                self.sidebar_tab = SidebarTab::Sessions;
                self.filter.get_or_insert_default();
                if let Some(ctx) = &self.ctx {
                    sidebar::focus_filter(ctx);
                }
            }
            CommandId::SERIAL_SEND_BREAK => {
                if let Some(tab) = self.active_serial() {
                    self.send(Command::SerialBreak { tab, ms: 250 });
                }
            }
            CommandId::SERIAL_SET_LINES => {
                // Shortcut form: toggle DTR (the usual device reset line).
                if let Some(tab) = self.active_serial() {
                    let dtr = self.tab_mut(tab).is_some_and(|t| !t.lines.0);
                    self.apply(Action::SerialLines { tab, dtr: Some(dtr), rts: None });
                }
            }
            CommandId::SERIAL_LIST_PORTS => self.send(Command::ListSerialPorts),
            CommandId::WSL_LIST_DISTRIBUTIONS => self.send(Command::ListWslDistributions),
            EXIT => {
                if let Some(ctx) = &self.ctx {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            other => self.status = format!("Unknown command {}", other.as_str()),
        }
    }

    /// Active tab when it is a serial session.
    fn active_serial(&self) -> Option<TabId> {
        self.active_tab().filter(|t| t.session.protocol == Protocol::Serial).map(|t| t.id)
    }

    /// Saved sessions matching the Quick connect text, listed in the sidebar while typing.
    fn quick_matches(&self) -> Vec<&Session> {
        if self.quick.trim().is_empty() {
            return Vec::new();
        }
        self.store.sessions.iter().filter(|s| sidebar::matches_filter(s, &self.quick)).collect()
    }

    /// Highlighted Quick connect entry: the arrow-key choice, else the first matching session.
    /// `None` connects to the typed target.
    fn quick_target(&self, matches: usize) -> Option<usize> {
        self.quick_pick.filter(|i| *i < matches + sidebar::QUICK_EXTRA.len()).or((matches > 0).then_some(0))
    }

    fn quick_connect(&mut self) {
        if self.quick.trim().is_empty() {
            self.focus_quick = true;
            return;
        }
        let matches: Vec<Session> = self.quick_matches().into_iter().cloned().collect();
        if let Some(i) = self.quick_target(matches.len()) {
            self.quick.clear();
            self.quick_pick = None;
            self.quick_error = None;
            match i.checked_sub(matches.len()) {
                None => self.apply(Action::Open(Box::new(matches[i].clone()))),
                Some(0) => self.run(CommandId::NEW_SESSION),
                Some(_) => self.run(HELP),
            }
            return;
        }
        match rxt_core::quick_connect::parse(&self.quick) {
            Ok(session) => {
                self.quick.clear();
                self.quick_error = None;
                self.apply(Action::Open(Box::new(session)));
            }
            Err(e) => self.quick_error = Some(e.to_string()),
        }
    }

    pub(crate) fn apply(&mut self, action: Action) {
        match action {
            Action::Run(cmd) => self.run(cmd),
            Action::OnTab(tab, cmd) => {
                self.active = Some(tab);
                self.run(cmd);
            }
            Action::SelectTab(tab) => self.active = tab,
            Action::MoveTab { from, to } => move_tab(&mut self.tabs, from, to),
            Action::CloseTab(tab) => self.close_tab(tab),
            Action::Exit => {
                self.exit_confirmed = true;
                if let Some(ctx) = &self.ctx {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            Action::CloseOthers(keep) => {
                let others: Vec<TabId> = self.tabs.iter().map(|t| t.id).filter(|id| *id != keep).collect();
                others.into_iter().for_each(|id| self.close_tab(id));
                self.active = Some(keep);
            }
            Action::RenameTab(tab, name) => {
                if let Some(t) = self.tab_mut(tab) {
                    t.title = name;
                }
            }
            Action::SelectMultiExec(tab, selected) => {
                if let Some(t) = self.tab_mut(tab) {
                    t.multiexec = selected;
                }
                self.send(Command::SelectMultiExec { tab, selected });
            }
            Action::Open(session) => {
                // Recents and other callers may hold an older copy: a saved session always opens
                // with its current settings (e.g. a key file added after it was last opened).
                let session = match self.store.session(session.id) {
                    Some(live) if session.id.0 != 0 => Box::new(live.clone()),
                    _ => session,
                };
                // Local terminals are never persisted (local-shell spec).
                if session.protocol != Protocol::Local {
                    self.store.push_recent(&session);
                    self.history(&session);
                }
                self.status = format!("Connecting to {}…", session.name);
                self.send(Command::Open { session });
            }
            Action::SelectSession(id) => self.selected_session = Some(id),
            Action::NewSession(_) if !self.require_unlocked() => {}
            Action::NewSession(folder) => {
                let mut ed = Editor::untyped(Session { folder, ..Session::default() });
                let mut users: Vec<String> = self.store.sessions.iter().filter_map(|s| s.user.clone()).collect();
                users.sort_unstable();
                users.dedup();
                ed.known_users = users;
                self.editor = Some(ed);
            }
            Action::EditSession(id) => {
                if let Some(s) = self.store.session(id) {
                    let mut ed = Editor::new(s.clone(), false);
                    let vault = self.vault.as_ref().filter(|v| v.is_unlocked());
                    ed.policy = vault.and_then(|v| v.with(|db| db.policy(id)).ok());
                    ed.has_password = vault.is_some_and(|v| {
                        v.with(|db| db.has_credential(id, rxt_store::db::CredentialKind::Password)).unwrap_or(false)
                    });
                    self.editor = Some(ed);
                }
            }
            Action::DuplicateSession(id) => {
                if let Some(n) = self.store.duplicate(id) {
                    self.selected_session = Some(n);
                }
            }
            Action::SaveSession { session, connect } => {
                let id = self.store.upsert(*session);
                self.selected_session = Some(id);
                if let Some(ctx) = &self.ctx {
                    sidebar::focus_row(ctx, id);
                }
                if connect && let Some(s) = self.store.session(id).cloned() {
                    self.apply(Action::Open(Box::new(s)));
                }
            }
            Action::SavePassword(secret) => {
                let saved = self.selected_session.zip(self.vault.as_ref()).map(|(id, vault)| {
                    let secret = secrecy::SecretString::from(secret.0.as_str());
                    vault.with(|db| db.save_credential(id, rxt_store::db::CredentialKind::Password, &secret))
                });
                if let Some(Err(e)) = saved {
                    self.status = format!("Could not save the password: {e}");
                }
            }
            Action::SerialLines { tab, dtr, rts } => {
                if let Some(t) = self.tab_mut(tab) {
                    t.lines = (dtr.unwrap_or(t.lines.0), rts.unwrap_or(t.lines.1));
                }
                self.send(Command::SerialLines { tab, dtr, rts });
            }
            Action::DeleteSession(id) => self.store.delete(id),
            Action::MoveSession(id, folder) => self.store.move_session(id, folder),
            Action::SaveFolder { id, parent, name, icon } => {
                let id = self.store.save_folder(id, parent, name);
                self.store.set_folder_icon(id, icon);
            }
            Action::DeleteFolder(id) => self.store.delete_folder(id),
            Action::Ask(m) => self.modal = Some(m),
            Action::Answer(prompt, answer) => self.send(Command::Answer { prompt, answer }),
            Action::Engine(c) => self.send(c),
            Action::SetMonitoring(tab, enabled) => {
                if let Some(t) = self.tab_mut(tab) {
                    t.monitor.set_state(if enabled { rxt_core::MonitorState::On } else { rxt_core::MonitorState::Off });
                }
                self.send(Command::SetMonitoring { tab, enabled });
            }
            Action::PasteConfirmed(tab, text) => {
                let bytes = self
                    .tabs
                    .iter()
                    .find(|t| t.id == tab)
                    .and_then(|t| t.term.as_ref())
                    .map(|term| terminal::paste_confirmed(term, &text));
                if let Some(bytes) = bytes {
                    self.send(Command::Input { tab, bytes });
                }
            }
            Action::Sftp(a) => self.sftp_action(a),
            Action::Local(a) => {
                let home = self.platform.paths.home_dir();
                let active = self.active;
                let pane = active.and_then(|id| self.tab_mut(id)).and_then(|t| t.files.as_mut());
                if let Some(err) = pane.and_then(|f| file_tab::local_action(&mut f.local, a, &home)) {
                    self.status = err;
                }
            }
            Action::Status(text) => self.status = text,
            Action::TerminalFontSize(tab, size) => {
                let Some(session) = self.tabs.iter().find(|t| t.id == tab).map(|t| t.session.id) else { return };
                let own = self.tabs.iter().any(|t| t.id == tab && t.session.terminal.font_size.is_some());
                // A session with its own font size keeps the zoom (all its tabs); others change the global default.
                if own {
                    for t in self
                        .tabs
                        .iter_mut()
                        .filter(|t| t.id == tab || (session != SessionId(0) && t.session.id == session))
                    {
                        t.session.terminal.font_size = Some(size);
                    }
                    if let Some(mut s) = self.store.session(session).cloned() {
                        s.terminal.font_size = Some(size);
                        self.store.upsert(s);
                    }
                } else {
                    self.settings.terminal.font_size = Some(size);
                }
            }
            Action::OpenLocal { path, folder } => self.open_local(&path, folder),
            Action::PickDiagFolder => {
                self.spawn_dialog(DialogKind::DiagFolder, |d, start| {
                    d.pick_folder("Diagnostics folder", start).map(DialogResult::DiagFolder)
                });
            }
            Action::ClearDiagnostics => self.clear_diagnostics(),
            Action::StreamCapture { on, include_input } => self.set_stream(on, include_input),
            Action::BrowseKeyFile => {
                self.spawn_dialog(DialogKind::KeyFile, |d, start| {
                    d.pick_files("Select private key", start)
                        .and_then(|f| f.into_iter().next())
                        .map(DialogResult::KeyFile)
                });
            }
            Action::StartTunnel(name) => {
                if let Some(tunnel) = self.store.tunnels.iter().find(|t| t.name == name).cloned() {
                    self.tunnel_states.insert(name, TunnelState::Starting);
                    let session = self.store.sessions.iter().find(|s| s.id == tunnel.via).cloned().map(Box::new);
                    self.send(Command::StartTunnel { tunnel, session });
                }
            }
            Action::EditTunnel(name) => {
                if let Some(t) = self.store.tunnels.iter().find(|t| t.name == name) {
                    self.tunnel_editor = Some(TunnelEditor { tunnel: t.clone(), original: Some(name), error: None });
                }
            }
            Action::SaveTunnel { original, tunnel } => {
                match original.and_then(|o| self.store.tunnels.iter_mut().find(|t| t.name == o)) {
                    Some(t) => *t = tunnel,
                    None => self.store.tunnels.push(tunnel),
                }
            }
            Action::DeleteTunnel(name) => self.store.tunnels.retain(|t| t.name != name),
            Action::MoveTunnel { name, up } => {
                if let Some(i) = self.store.tunnels.iter().position(|t| t.name == name) {
                    let j = if up { i.checked_sub(1) } else { Some(i + 1).filter(|j| *j < self.store.tunnels.len()) };
                    if let Some(j) = j {
                        self.store.tunnels.swap(i, j);
                    }
                }
            }
            Action::SaveSecret(prompt) => self.backend.save_secret(prompt),
            Action::ShowUnlock => {
                if let Some(v) = &self.vault
                    && !v.is_unlocked()
                {
                    self.store_dialog = Some(if v.is_new() { StoreDialog::create() } else { StoreDialog::unlock() });
                }
            }
            Action::CreateVault { master, keyring } => {
                self.spawn_vault(move |v| VaultMsg::Opened(v.create(&master.0, keyring).map(|()| true)));
            }
            Action::Unlock(master) => {
                self.spawn_vault(move |v| VaultMsg::Opened(v.unlock(&master.0).map(|()| true)));
            }
            Action::ShowChangePassword => {
                if self.require_unlocked() {
                    self.store_dialog = Some(StoreDialog::change_password());
                }
            }
            Action::ChangePassword(master) => {
                self.spawn_vault(move |v| VaultMsg::PasswordChanged(v.change_password(&master.0)));
            }
            Action::SetKeyring(on) => self.spawn_vault(move |v| VaultMsg::KeyringSet(on, v.set_keyring(on))),
            Action::ImportIni => {
                if self.require_unlocked() {
                    self.spawn_dialog(DialogKind::Import, |d, start| {
                        d.pick_files("Import MobaXterm.ini", start)
                            .and_then(|f| f.into_iter().next())
                            .map(DialogResult::ImportIni)
                    });
                }
            }
            Action::ImportMobaFound(path) => {
                self.settings.first_run_moba_offered = true;
                self.moba_found = None;
                if self.require_unlocked() {
                    self.import_ini(path);
                }
            }
            Action::DismissMobaOffer => {
                self.settings.first_run_moba_offered = true;
                self.moba_found = None;
            }
            Action::ConfirmImport => self.confirm_import(),
            Action::ImportToml => {
                if self.require_unlocked() {
                    self.spawn_dialog(DialogKind::Import, |d, start| {
                        d.pick_files("Import rustXterm sessions", start)
                            .and_then(|f| f.into_iter().next())
                            .map(DialogResult::ImportToml)
                    });
                }
            }
            Action::ExportToml => {
                if self.require_unlocked() {
                    self.spawn_dialog(DialogKind::Export, |d, start| {
                        d.save_file("Export rustXterm sessions", "rustxterm-sessions.toml", start)
                            .map(DialogResult::ExportToml)
                    });
                }
            }
            Action::ConfirmExport => self.confirm_export(),
            Action::SavePolicy(id, policy) => {
                let r = match &self.vault {
                    Some(v) => v.with(|db| db.set_policy(id, policy)),
                    None => Ok(()),
                };
                if let Err(e) = r {
                    self.status = format!("Could not save the agent policy: {e}");
                }
            }
            Action::BrowseAgentWorkspace => {
                self.spawn_dialog(DialogKind::Workspace, |d, start| {
                    d.pick_folder("Agent workspace", start).map(DialogResult::AgentWorkspace)
                });
            }
            Action::ShowAudit => {
                self.audit = Some(AuditView::default());
                self.load_audit(0);
            }
            Action::AuditPage(page) => self.load_audit(page),
            Action::ClearAudit => {
                if let Some(Err(e)) = self.vault.as_ref().map(|v| v.with(|db| db.clear_audit())) {
                    self.status = format!("Could not clear the audit log: {e}");
                }
                self.load_audit(0);
            }
        }
    }

    fn sftp_action(&mut self, a: SftpAction) {
        let Some(tab) = self.remote_tab() else { return };
        let local_dir =
            self.tabs.iter().find(|t| t.id == tab).and_then(|t| t.files.as_ref()).map(|f| f.local.path.clone());
        let Some(st) = self.remote_for(tab) else { return };
        let dir = st.path.clone();
        let (selected_path, selected) = (st.selected_path(), st.selected.clone());
        match a {
            SftpAction::Navigate(path) => self.list(tab, path),
            SftpAction::Refresh => self.list(tab, if dir.is_empty() { HOME.into() } else { dir }),
            SftpAction::Select(name) => st.selected = Some(name),
            SftpAction::Download => {
                let (Some(remote), Some(name)) = (selected_path, selected) else {
                    return;
                };
                if let Some(dir) = local_dir {
                    return self.sftp_action(SftpAction::DownloadTo { remote, dir });
                }
                self.spawn_dialog(DialogKind::Download, move |d, start| {
                    d.save_file("Download", &name, start).map(|local| DialogResult::Download { tab, remote, local })
                });
            }
            SftpAction::Upload => {
                self.spawn_dialog(DialogKind::Upload, move |d, start| {
                    d.pick_files("Upload", start).map(|files| DialogResult::Upload { tab, dir, files })
                });
            }
            SftpAction::UploadPaths(files) => self.upload(tab, &dir, files),
            SftpAction::DownloadTo { remote, dir } => {
                let name = remote.rsplit('/').next().unwrap_or(&remote).to_owned();
                let local = rxt_core::fileops::join(&dir, &name);
                self.send(Command::Transfer { tab, direction: Direction::Download, remote, local });
            }
            SftpAction::Mkdir(path) => self.mutate(Command::Mkdir { tab, path }),
            SftpAction::NewFile(remote) => {
                // An empty upload creates the file; the listing refreshes when it finishes.
                let dir = self.platform.paths.cache_dir();
                let local = dir.join("empty-file");
                match std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&local, b"")) {
                    Ok(()) => {
                        let local = local.to_string_lossy().into_owned();
                        self.send(Command::Transfer { tab, direction: Direction::Upload, remote, local });
                    }
                    Err(e) => self.status = format!("Cannot create the file: {e}"),
                }
            }
            SftpAction::Rename { from, to } => self.mutate(Command::Rename { tab, from, to }),
            SftpAction::Delete(path) => self.mutate(Command::Delete { tab, path }),
            SftpAction::Chmod { path, mode } => self.mutate(Command::Chmod { tab, path, mode }),
            SftpAction::CancelTransfer(id) => self.send(Command::CancelTransfer { id }),
            SftpAction::ClearFinished => self.transfers.retain(|t| t.result.is_none() || t.tab != tab),
            SftpAction::SetMonitoring(on) => {
                self.apply(Action::SetMonitoring(tab, on));
            }
            SftpAction::SetFollow(on) => {
                st.follow = on;
                self.send(Command::SetFollowCwd { tab, enabled: on });
                let cwd = self.tabs.iter().find(|t| t.id == tab).and_then(|t| t.cwd.clone());
                if let Some(cwd) = cwd.filter(|c| on && *c != dir) {
                    self.list(tab, cwd);
                }
            }
        }
    }

    /// Send a file operation, then refresh the listing.
    fn mutate(&mut self, c: Command) {
        self.send(c);
        self.apply(Action::Sftp(SftpAction::Refresh));
    }

    // ---- input ----

    /// Consume configured shortcuts (exact modifiers); unbound keys such as Ctrl+C stay for the terminal.
    fn shortcuts(&self, ctx: &egui::Context) -> Vec<Action> {
        // A grabbed remote desktop receives every key, application shortcuts included.
        if self.active_tab().and_then(|t| t.desktop.as_ref()).is_some_and(|d| d.grabbed) {
            return Vec::new();
        }
        let mut bindings = self.settings.bindings();
        if !self.settings.shortcuts.contains_key(CommandId::FOCUS_MENU.as_str())
            && let Some(sc) = settings::parse_shortcut(FOCUS_MENU_DEFAULT)
        {
            bindings.push((CommandId::FOCUS_MENU, sc));
        }
        let mut out = Vec::new();
        ctx.input_mut(|i| {
            for (cmd, sc) in &bindings {
                let before = i.events.len();
                i.events.retain(|e| {
                    !matches!(e, egui::Event::Key { key, pressed: true, modifiers, .. }
                        if *key == sc.logical_key && modifiers.matches_exact(sc.modifiers))
                });
                if i.events.len() != before {
                    out.push(Action::Run(*cmd));
                }
            }
        });
        out
    }

    /// Files dropped from the desktop upload to the attached SFTP directory.
    fn dropped_files(&mut self, ctx: &egui::Context, out: &mut Vec<Action>) {
        let files: Vec<PathBuf> = ctx.input(|i| {
            i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect()
        });
        if files.is_empty() {
            return;
        }
        if self.active_tab().is_some_and(|t| t.files.is_some()) {
            out.push(Action::Sftp(SftpAction::UploadPaths(files)));
        } else if self.sftp.tab.is_some() {
            self.sidebar_tab = SidebarTab::Sftp;
            self.settings.sidebar_page = SidebarTab::Sftp.key().to_owned();
            self.settings.sidebar_open = true;
            out.push(Action::Sftp(SftpAction::UploadPaths(files)));
        } else {
            self.status = "Open a connected SSH tab to upload dropped files.".into();
        }
    }

    // ---- layout ----

    /// Panels per design R5: menu 20, toolbar 52/34, quick-connect/tab row 24, status 20,
    /// sidebar column, zero-margin work area.
    fn layout(&mut self, ui: &mut egui::Ui, out: &mut Vec<Action>) {
        let m = Metrics::DEFAULT;
        let band = egui::Frame::NONE.fill(color(self.tokens.toolbar));
        let open_menu = std::mem::take(&mut self.open_menu);
        egui::Panel::top("menu")
            .exact_size(m.menu_height)
            .frame(band.inner_margin(egui::Margin::symmetric(2, 0)))
            .show_separator_line(false)
            .show(ui, |ui| self.menu_bar(ui, open_menu, out));
        let toolbar_h = if self.settings.toolbar_captions { m.toolbar_height } else { m.toolbar_compact_height };
        egui::Panel::top("toolbar")
            .exact_size(toolbar_h)
            .frame(band.inner_margin(egui::Margin::symmetric(4, 2)))
            .show(ui, |ui| self.toolbar(ui, out));
        egui::Panel::top("row").exact_size(m.row_height).frame(band).show(ui, |ui| self.row(ui, out));
        egui::Panel::bottom("status")
            .exact_size(m.status_height)
            .frame(egui::Frame::NONE.fill(color(self.tokens.panel)).inner_margin(egui::Margin::symmetric(6, 0)))
            .show(ui, |ui| self.status_bar(ui));
        if self.stream_chip_clicked {
            self.stream_chip_clicked = false;
            settings::open_on(ui.ctx(), settings::SettingsTab::Diagnostics);
            self.show_settings = true;
        }
        let max_w = (ui.max_rect().width() / 2.0).max(160.0);
        let side_frame = egui::Frame::NONE.fill(color(self.tokens.surface));
        if self.settings.sidebar_open {
            let r = egui::Panel::left("sidebar")
                .resizable(true)
                .frame(side_frame)
                .default_size(self.settings.sidebar_width)
                .size_range(160.0..=max_w)
                .show(ui, |ui| self.sidebar(ui, out));
            if self.settings.sidebar_open {
                self.settings.sidebar_width = r.response.rect.width().round();
            }
        } else {
            egui::Panel::left("sidebar-strip")
                .resizable(false)
                .frame(side_frame)
                .exact_size(m.strip_width)
                .show(ui, |ui| self.sidebar(ui, out));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(color(self.tokens.surface)))
            .show(ui, |ui| self.central(ui, out));
    }

    /// Quick connect over the sidebar column (or at the row start when collapsed), then the tab strip.
    fn row(&mut self, ui: &mut egui::Ui, out: &mut Vec<Action>) {
        let m = Metrics::DEFAULT;
        let rect = ui.max_rect();
        let column =
            if self.settings.sidebar_open { self.settings.sidebar_width } else { m.sidebar_width - m.strip_width };
        // 26 px field centered in the 28 px row.
        let field = egui::Rect::from_min_max(
            egui::pos2(rect.min.x + 2.0, rect.min.y + 1.0),
            egui::pos2(rect.min.x + column - 2.0, rect.max.y - 1.0),
        );
        self.quick_connect_field(ui, field, out);
        let strip = egui::Rect::from_min_max(egui::pos2(rect.min.x + column, rect.min.y), rect.max);
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(strip).layout(egui::Layout::left_to_right(egui::Align::Max)),
            |ui| {
                tabs::tab_strip(
                    ui,
                    &self.tabs,
                    self.active,
                    self.multiexec,
                    self.platform.local_pty.is_some(),
                    &self.settings,
                    &self.icons,
                    &self.tokens,
                    out,
                );
            },
        );
    }

    /// Placeholder instead of a label; parse errors as a tooltip-style note and a danger border.
    fn quick_connect_field(&mut self, ui: &mut egui::Ui, field: egui::Rect, out: &mut Vec<Action>) {
        let id = egui::Id::new("quick-connect");
        if ui.memory(|m| m.has_focus(id)) && !self.quick.trim().is_empty() {
            // Arrow keys walk the list shown in the sidebar instead of moving the text cursor.
            let (down, up) = ui.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                )
            });
            let matches = self.quick_matches().len();
            let last = matches + sidebar::QUICK_EXTRA.len() - 1;
            let cur = self.quick_target(matches);
            if down {
                self.quick_pick = Some(cur.map_or(0, |i| (i + 1).min(last)));
            } else if up {
                self.quick_pick = Some(cur.map_or(0, |i| i.saturating_sub(1)));
            }
        }
        let edit = egui::TextEdit::singleline(&mut self.quick)
            .id(id)
            .hint_text("Quick connect...")
            .desired_width(field.width())
            .min_size(field.size());
        let r = ui.put(field, edit);
        ui.ctx().accesskit_node_builder(r.id, |n| n.set_label("Quick connect"));
        if std::mem::take(&mut self.focus_quick) {
            r.request_focus();
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), r.id) {
                let all = egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(self.quick.chars().count()),
                );
                state.cursor.set_char_range(Some(all));
                state.store(ui.ctx(), r.id);
            }
        }
        if r.changed() {
            self.quick_error = None;
            self.quick_pick = None;
        }
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.quick.clear();
            self.quick_pick = None;
        }
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            out.push(Action::Run(CommandId::QUICK_CONNECT));
        }
        if let Some(e) = &self.quick_error {
            let danger = color(self.tokens.danger);
            ui.painter().rect_stroke(r.rect, 0.0, egui::Stroke::new(1.0, danger), egui::StrokeKind::Inside);
            egui::Area::new(ui.id().with("quick-connect-error"))
                .order(egui::Order::Tooltip)
                .fixed_pos(r.rect.left_bottom() + vec2(0.0, 2.0))
                .show(ui.ctx(), |ui| {
                    egui::Frame::NONE
                        .fill(color(self.tokens.panel))
                        .stroke(egui::Stroke::new(1.0, color(self.tokens.border)))
                        .inner_margin(egui::Margin::symmetric(6, 2))
                        .show(ui, |ui| {
                            ui.label(RichText::new(e).small().color(danger));
                        });
                });
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, out: &mut Vec<Action>) {
        let captions = self.settings.toolbar_captions;
        let pressed = color(self.tokens.pressed);
        ui.spacing_mut().item_spacing.x = Metrics::DEFAULT.space[1];
        let mut button = |ui: &mut egui::Ui, icon: &str, label: &str, cmd: CommandId, on: bool| {
            let hint = match self.settings.shortcut_text(cmd) {
                Some(s) => format!("{label} ({s})"),
                None => label.to_owned(),
            };
            let r = toolbar_button(ui, &self.icons, icon, label, on.then_some(pressed), captions);
            if r.on_hover_text(hint).clicked() {
                out.push(Action::Run(cmd));
            }
        };
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            for (icon, label, cmd) in TOOLBAR {
                let on = (cmd == CommandId::MULTIEXEC && self.multiexec)
                    || (cmd == CommandId::TOGGLE_SIDEBAR && self.settings.sidebar_open);
                button(ui, icon, label, cmd, on);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                button(ui, "exit", "Exit", EXIT, false);
            });
        });
    }

    /// One line: latest message (ellipsized) left, tab count right; never grows the bar.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(format!("{} tab(s)", self.tabs.len())).small());
            if self.stream.on {
                let text = RichText::new(" REC  Recording stream ")
                    .small()
                    .strong()
                    .color(egui::Color32::WHITE)
                    .background_color(color(self.tokens.danger));
                let chip = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
                if chip.on_hover_text("Data-stream capture is on: open Settings → Diagnostics").clicked() {
                    self.stream_chip_clicked = true;
                }
            }
            if self.multiexec {
                ui.label(RichText::new("MultiExec ON").small().color(color(self.tokens.warning)));
            }
            if let Some(t) = self.active_tab() {
                let s = &t.session;
                let text = format!("{}  —  {}", s.target(), state_text(&t.state));
                ui.label(RichText::new(text).small());
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let text =
                    if self.status.is_empty() { "Ready" } else { self.status.lines().next().unwrap_or_default() };
                ui.add(egui::Label::new(RichText::new(text).small()).truncate());
            });
        });
    }

    /// Restored geometry must leave at least 200×100 px on the monitor; otherwise refit it.
    /// Once the window exists, take over the fullscreen state eframe restored. eframe itself keeps
    /// a restored window on a connected monitor and within its size; the monitor's origin is not
    /// known here, so moving the window would pull it off a secondary monitor.
    fn sync_window_state(&mut self, ctx: &egui::Context) {
        if self.window_synced {
            return;
        }
        let Some(full) = ctx.input(|i| i.viewport().fullscreen) else { return };
        self.window_synced = true;
        self.fullscreen = full;
    }

    /// Icon strip on the left edge, then the active page (absent while collapsed).
    fn sidebar(&mut self, ui: &mut egui::Ui, out: &mut Vec<Action>) {
        // The strip works on the shown page; only a click that shows a page is saved as the choice.
        let (shown, was_open) = (self.sidebar_tab.key(), self.settings.sidebar_open);
        let chosen = std::mem::replace(&mut self.settings.sidebar_page, shown.to_owned());
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            sidebar::icon_strip(ui, &mut self.settings, &self.icons, &self.tokens);
            if self.settings.sidebar_page == shown && (was_open || !self.settings.sidebar_open) {
                self.settings.sidebar_page = chosen;
            } else {
                self.sidebar_tab = SidebarTab::of(&self.settings);
            }
            if self.settings.sidebar_open {
                ui.vertical(|ui| self.sidebar_page(ui, out));
            }
        });
    }

    fn sidebar_page(&mut self, ui: &mut egui::Ui, out: &mut Vec<Action>) {
        match self.sidebar_tab {
            SidebarTab::Sessions if !self.quick.trim().is_empty() => {
                let matches = self.quick_matches();
                let pick = self.quick_target(matches.len());
                if let Some(i) = sidebar::quick_list(ui, &matches, pick, &self.icons, &self.tokens) {
                    self.quick_pick = Some(i);
                    out.push(Action::Run(CommandId::QUICK_CONNECT));
                }
            }
            SidebarTab::Sessions => {
                let locked = self.locked().then(|| self.vault.as_ref().is_some_and(|v| v.is_new()));
                let connected: Vec<_> =
                    self.tabs.iter().filter(|t| t.state == ConnState::Connected).map(|t| t.session.id).collect();
                sidebar::sessions_ui(
                    ui,
                    &self.store,
                    locked,
                    &mut self.filter,
                    self.selected_session,
                    &connected,
                    &self.icons,
                    &self.tokens,
                    out,
                );
            }
            SidebarTab::Sftp => {
                let monitor = self.sftp.tab.and_then(|id| self.tabs.iter().find(|t| t.id == id)).map(|t| &t.monitor);
                let transfers = tab_transfers(&self.transfers, self.sftp.tab);
                sftp_panel::sftp_ui(ui, &mut self.sftp, monitor, &transfers, &self.icons, &self.tokens, out)
            }
        }
    }

    fn central(&mut self, ui: &mut egui::Ui, out: &mut Vec<Action>) {
        let Some(idx) = self.active.and_then(|id| self.tabs.iter().position(|t| t.id == id)) else {
            self.home(ui, out);
            return;
        };
        let cfg = self.view_config(&self.tabs[idx].session);
        let tokens = &self.tokens;
        let tab = &mut self.tabs[idx];
        let id = tab.id;
        if let Some(view) = tab.desktop.as_mut()
            && tab.state != ConnState::Connected
        {
            // Report the desktop area size while still connecting too: a server without dynamic
            // resize (e.g. Windows Server 2008) fixes its resolution at connect time, so the engine
            // waits for this to open the session at the real tab size instead of a small default.
            if tab.session.protocol == Protocol::Rdp && tab.session.rdp.resolution == RdpResolution::FitToTab {
                let ppp = ui.ctx().pixels_per_point();
                let want = desktop::desktop_size(ui.available_rect_before_wrap().size() * ppp);
                if view.sent_size != Some(want) {
                    view.sent_size = Some(want);
                    out.push(Action::Engine(Command::DesktopInput {
                        tab: id,
                        input: DesktopInput::Resize { width: want[0], height: want[1] },
                    }));
                }
            }
            let reconnect = desktop::state_ui(ui, view, &tab.session.host, &tab.state);
            // Keyboard shortcuts on a stopped desktop, like a stopped terminal: R reconnects,
            // Ctrl+D or Escape closes.
            let (key_reconnect, key_close) =
                if matches!(tab.state, ConnState::Disconnected { .. } | ConnState::Error(_)) {
                    ui.input(|i| {
                        (
                            i.key_pressed(egui::Key::R),
                            (i.modifiers.ctrl && i.key_pressed(egui::Key::D)) || i.key_pressed(egui::Key::Escape),
                        )
                    })
                } else {
                    (false, false)
                };
            if reconnect || key_reconnect {
                out.push(Action::OnTab(id, CommandId::RECONNECT));
            } else if key_close {
                out.push(Action::OnTab(id, CommandId::CLOSE_TAB));
            }
            return;
        }
        // Terminal tabs get the notice in the terminal itself (print_stopped).
        if let ConnState::Disconnected { .. } | ConnState::Error(_) = &tab.state
            && tab.term.is_none()
        {
            let next = match &tab.state {
                ConnState::Disconnected { reconnect_at: Some(at), .. } => {
                    format!("  Next attempt at {} UTC.", sftp_panel::format_time(*at))
                }
                _ => String::new(),
            };
            egui::Frame::new().fill(color(tokens.hover)).inner_margin(6).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(color(tokens.warning), format!("{}.{next}", state_text(&tab.state)));
                    if ui.button("Reconnect").clicked() {
                        out.push(Action::OnTab(id, CommandId::RECONNECT));
                    }
                });
            });
        }
        if let Some(notice) = tab.notice {
            egui::Frame::new().fill(color(tokens.hover)).inner_margin(6).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(color(tokens.warning), notice);
                    if ui.button("Dismiss").clicked() {
                        tab.notice = None;
                    }
                });
            });
        }
        if tab.wsl_startup_pending {
            egui::Frame::new().fill(color(tokens.hover)).inner_margin(6).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Run the startup commands now?");
                    let mut answer = None;
                    if ui.button("Run").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Skip").clicked() {
                        answer = Some(false);
                    }
                    if let Some(run) = answer {
                        tab.wsl_startup_pending = false;
                        out.push(Action::Engine(Command::WslStartup { tab: id, run }));
                    }
                });
            });
        }
        if let Some(files) = tab.files.as_mut() {
            let transfers = tab_transfers(&self.transfers, Some(tab.id));
            file_tab::show(ui, files, &tab.session, &transfers, &self.icons, tokens, out);
            return;
        }
        if let Some(view) = tab.desktop.as_mut() {
            desktop_tab(ui, id, view, &tab.session, &self.settings, self.backend.as_ref(), &self.platform, out);
            return;
        }
        let live = tab.is_live();
        let Some(term) = tab.term.as_mut() else { return };
        if tab.session.protocol == Protocol::Ssh && tab.state == ConnState::Connected && tab.monitor.shown() {
            if let Some(sample) = self.backend.take_metrics(id) {
                tab.monitor.push(sample, ui.input(|i| i.time));
            }
            let interval = self.settings.remote_monitoring_interval_secs;
            egui::Panel::bottom(egui::Id::new(("monitor-bar", id)))
                .exact_size(monitor_bar::BAR_H)
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    monitor_bar::bar(ui, &mut tab.monitor, id, &tab.session.host, interval, &self.icons, tokens, out)
                });
        }
        if let Some(f) = &mut self.find
            && !find_bar(ui, f, term)
        {
            self.find = None;
        }
        let mut resp = terminal::show(ui, term, &cfg);
        if let Some(size) = resp.font_size {
            out.push(Action::TerminalFontSize(id, size));
        }
        if let Some(note) = resp.font_fallback.take()
            && self.status.is_empty()
        {
            out.push(Action::Status(note));
        }
        if let Some(area) = resp.area.take()
            && let Some(cmd) = terminal_menu(&area, resp.menu, term, live, &self.icons, &self.settings)
        {
            out.push(Action::OnTab(id, cmd));
        }
        if std::mem::take(&mut resp.paste_from_clipboard) {
            match self.platform.clipboard.get_text() {
                Ok(text) => terminal::paste(term, &text, &cfg, &mut resp),
                Err(e) => self.status = e.to_string(),
            }
        }
        if matches!(tab.state, ConnState::Disconnected { .. } | ConnState::Error(_)) {
            // Stopped session: Enter or Ctrl+D closes the tab, R reconnects, S saves.
            if let Some(cmd) = ended_session_key(&resp.input) {
                out.push(Action::OnTab(id, cmd));
            }
        } else if !resp.input.is_empty() {
            out.push(Action::Engine(Command::Input { tab: id, bytes: std::mem::take(&mut resp.input) }));
        }
        if let Some((cols, rows)) = resp.resized {
            out.push(Action::Engine(Command::Resize { tab: id, cols, rows }));
        }
        if let Some(text) = resp.paste_request {
            out.push(Action::Ask(Modal::Paste { tab: id, text }));
        }
    }

    fn windows(&mut self, ctx: &egui::Context, out: &mut Vec<Action>) {
        if self.show_settings {
            let store = self
                .vault
                .as_ref()
                .map(|v| settings::StoreState { unlocked: v.is_unlocked(), keyring: self.keyring_on });
            let default_workspace = self.platform.paths.data_dir().join("agent-files").display().to_string();
            let agent = agent::AgentSettings { workspace: &mut self.agent_workspace, default_workspace };
            settings::window(
                ctx,
                &mut self.show_settings,
                &mut self.settings,
                store,
                agent,
                settings::DiagPage { state: &self.diag_state, stream: &mut self.stream },
                &self.icons,
                &self.tokens,
                out,
            );
        }
        if let Some(view) = &mut self.audit {
            let source = match &self.vault {
                None => agent::AuditSource::NoDatabase,
                Some(v) if !v.is_unlocked() => agent::AuditSource::Locked,
                Some(_) => agent::AuditSource::Unlocked,
            };
            if !agent::audit_window(ctx, view, source, &self.tokens, out) {
                self.audit = None;
            }
        }
        if self.show_tunnels {
            tunnels::window(
                ctx,
                &mut self.show_tunnels,
                &self.store,
                &self.tunnel_states,
                &mut self.tunnel_editor,
                &self.icons,
                &self.tokens,
                out,
            );
        }
        if let Some(ed) = &mut self.editor {
            ed.port_hint = self.platform.serial.example_port();
            ed.wsl_available = self.platform.wsl.is_some();
        }
        if let Some(ed) = &mut self.editor
            && !session_editor::window(ctx, ed, &self.store.folders, &self.icons, &self.tokens, out)
        {
            self.editor = None;
        }
        if let Some(a) = self.approvals.front() {
            let now = (self.clock)();
            if let Some(approve) = agent::approval_ui(ctx, a, self.approvals.len() - 1, now, &self.tokens) {
                let id = a.id;
                self.answer_approval(id, approve);
            }
        } else if let Some(p) = self.prompts.front_mut() {
            let unlocked = self.vault.as_ref().is_some_and(|v| v.is_unlocked());
            let saved = p.tab.and_then(|id| self.tabs.iter().find(|t| t.id == id)).is_some_and(|t| t.session.id.0 != 0);
            if dialogs::prompt_ui(ctx, p, unlocked && saved, &self.tokens, out) {
                self.prompts.pop_front();
            }
        } else if let Some(d) = &mut self.store_dialog {
            if !store_ui::dialog_ui(ctx, d, self.vault_busy, &self.tokens, out) && !self.vault_busy {
                self.close_store_dialog();
            }
        } else if let Some(m) = &mut self.modal {
            let tabs = &self.tabs;
            let title = |id: TabId| tabs.iter().find(|t| t.id == id).map_or_else(String::new, |t| t.title.clone());
            if !dialogs::modal_ui(ctx, m, &title, &self.tokens, out) {
                self.modal = None;
            }
        }
    }
}

/// MobaXterm's default config directories (installed under the config dir, portable under Documents),
/// where a `MobaXterm.ini` is looked for on first run.
fn moba_dirs(paths: &dyn Paths) -> Vec<PathBuf> {
    let home = paths.home_dir();
    vec![paths.config_dir().join("MobaXterm"), home.join("Documents").join("MobaXterm"), home.join("MobaXterm")]
}

/// Remote-desktop tab: texture upload, painting, input and clipboard sharing.
#[allow(clippy::too_many_arguments)] // plain drawing helper over disjoint App fields
fn desktop_tab(
    ui: &mut egui::Ui,
    tab: TabId,
    view: &mut DesktopView,
    s: &Session,
    settings: &Settings,
    backend: &dyn Backend,
    platform: &Platform,
    out: &mut Vec<Action>,
) {
    let ctx = ui.ctx().clone();
    backend.with_framebuffer(tab, &mut |fb| view.sync(&ctx, &format!("desktop-{}", tab.0), fb));
    let rdp = s.protocol == Protocol::Rdp;
    let clipboard = if rdp { s.rdp.clipboard } else { s.vnc.clipboard };
    let release_text = settings.desktop_release.trim().to_owned();
    let cfg = desktop::Config {
        view_only: !rdp && s.vnc.view_only,
        fit: rdp || s.vnc.scale_to_fit,
        resize: rdp && s.rdp.resolution == RdpResolution::FitToTab,
        release: desktop::parse_release(&release_text).unwrap_or_default(),
        release_text,
    };
    let resp = desktop::show(ui, view, &cfg);
    let mut inputs = resp.input;
    if clipboard
        && resp.grab_started
        && let Ok(text) = platform.clipboard.get_text()
        && !text.is_empty()
    {
        inputs.push(DesktopInput::Clipboard(text));
    }
    if clipboard
        && let Some(text) = backend.take_clipboard(tab)
        && let Err(e) = platform.clipboard.set_text(&text)
    {
        out.push(Action::Status(e.to_string()));
    }
    out.extend(inputs.into_iter().map(|input| Action::Engine(Command::DesktopInput { tab, input })));
}

/// Terminal context menu (MobaXterm-like): Ctrl+right click, or right click when it does not
/// paste. Local terminal actions run here; the others are returned to run on the tab.
fn terminal_menu(
    area: &egui::Response,
    open: Option<bool>,
    term: &mut rxt_term::Terminal,
    live: bool,
    icons: &Icons,
    settings: &Settings,
) -> Option<CommandId> {
    let mut run = None;
    sidebar::popup_menu(area, open, true, |ui| {
        let item = |ui: &mut egui::Ui, icon, label, cmd: Option<CommandId>, enabled| {
            let sc = cmd.and_then(|c| settings.shortcut_text(c));
            sidebar::menu_item(ui, icons, icon, label, sc.as_deref(), enabled)
        };
        let has_selection = term.selection_text().is_some();
        for (icon, label, cmd, enabled) in
            [("copy", "Copy", CommandId::COPY, has_selection), ("paste", "Paste", CommandId::PASTE, true)]
        {
            if item(ui, icon, label, Some(cmd), enabled) {
                run = Some(cmd);
            }
        }
        if item(ui, "selection", "Select all", None, true) {
            term.select_all();
        }
        for (icon, label, cmd) in
            [("search", "Find", CommandId::FIND), ("save", "Save terminal text", CommandId::SAVE_TEXT)]
        {
            if item(ui, icon, label, Some(cmd), true) {
                run = Some(cmd);
            }
        }
        ui.separator();
        if item(ui, "remove", "Clear scrollback", None, true) {
            term.clear_history();
        }
        if item(ui, "refresh", "Reset terminal", None, true) {
            term.reset();
        }
        ui.separator();
        for (icon, label, cmd, enabled) in [
            ("copy", "Duplicate tab", CommandId::DUPLICATE_TAB, true),
            ("restart", "Reconnect", CommandId::RECONNECT, !live),
            ("close", "Close tab", CommandId::CLOSE_TAB, true),
        ] {
            if item(ui, icon, label, Some(cmd), enabled) {
                run = Some(cmd);
            }
        }
    });
    run
}

fn find_bar(ui: &mut egui::Ui, f: &mut FindBar, term: &mut rxt_term::Terminal) -> bool {
    let mut keep = true;
    ui.horizontal(|ui| {
        let l = ui.label("Find");
        let r = ui.add(egui::TextEdit::singleline(&mut f.text).desired_width(220.0)).labelled_by(l.id);
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let case = ui.checkbox(&mut f.case_sensitive, "Match case").changed();
        let prev = ui.button("Previous").clicked();
        let next = ui.button("Next").clicked() || enter;
        if r.changed() || case || prev || next {
            f.status = match term.set_search(&f.text, f.case_sensitive) {
                Err(e) => Some(e.to_string()),
                Ok(()) if prev || next => term.search_next(next).is_none().then(|| "No matches".to_owned()),
                Ok(()) => None,
            };
        }
        if let Some(s) = &f.status {
            ui.label(s.as_str());
        }
        if ui.button("Close").clicked() {
            keep = false;
        }
    });
    keep
}

/// Toolbar button: 24 px icon with an 11 px caption below (or icon only); at least 56 px wide
/// with captions. `fill` is the pressed fill of a toggle that is on.
pub(crate) fn toolbar_button(
    ui: &mut egui::Ui,
    icons: &Icons,
    icon: &str,
    label: &str,
    fill: Option<Color32>,
    caption: bool,
) -> egui::Response {
    let m = Metrics::DEFAULT;
    let font = egui::TextStyle::Small.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, ui.visuals().text_color());
    let size = if caption {
        vec2((galley.size().x + 12.0).max(56.0), m.toolbar_height - 4.0)
    } else {
        vec2(m.toolbar_compact_height - 4.0, m.toolbar_compact_height - 4.0)
    };
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let v = ui.visuals();
        let bg = match fill {
            Some(c) => c,
            None if resp.hovered() => v.widgets.hovered.weak_bg_fill,
            None => Color32::TRANSPARENT,
        };
        ui.painter().rect_filled(rect, 0.0, bg);
        let px = m.icon_toolbar;
        let center = if caption { egui::pos2(rect.center().x, rect.min.y + 4.0 + px / 2.0) } else { rect.center() };
        if let Some(img) = icons.image(icon, px) {
            let img = if ui.is_enabled() { img } else { img.tint(Color32::from_white_alpha(110)) };
            img.paint_at(ui, egui::Rect::from_center_size(center, vec2(px, px)));
        }
        if caption {
            let text_color = if ui.is_enabled() { v.text_color() } else { v.weak_text_color() };
            let pos = egui::pos2(rect.center().x - galley.size().x / 2.0, rect.max.y - galley.size().y - 2.0);
            ui.painter().galley(pos, galley, text_color);
        }
    }
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Button, ui.is_enabled(), fill.is_some(), label));
    resp
}

/// Parent id for native file dialogs (XDG portal `x11:`, Windows `win32:`), so they open in
/// front of the window.
/// ponytail: X11 only; Wayland needs an xdg-foreign export (unsafe surface pointers), so
/// dialogs there stay unparented but modal.
pub(crate) fn dialog_parent(raw: eframe::wgpu::rwh::RawWindowHandle) -> Option<String> {
    use eframe::wgpu::rwh::RawWindowHandle;
    match raw {
        RawWindowHandle::Xlib(h) => Some(format!("x11:{:x}", h.window)),
        RawWindowHandle::Xcb(h) => Some(format!("x11:{:x}", h.window.get())),
        RawWindowHandle::Win32(h) => Some(format!("win32:{:x}", h.hwnd.get())),
        _ => None,
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        use eframe::wgpu::rwh::HasWindowHandle;
        if let Some(parent) = frame.window_handle().ok().and_then(|h| dialog_parent(h.as_raw())) {
            self.platform.dialogs.set_parent(Some(parent));
        }
        self.show(ui);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        match &self.settings_file {
            Some((path, base)) => {
                if self.settings != self.saved_settings {
                    // ponytail: values coming from RXT_*/.env/CLI are written too once the user changes a setting.
                    match rxt_store::settings::save(path, &self.settings.to_store(base)) {
                        Ok(()) => self.saved_settings = self.settings.clone(),
                        Err(e) => tracing::warn!("cannot save settings: {e}"),
                    }
                }
                storage.remove_string(KEY_SETTINGS);
            }
            None => eframe::set_value(storage, KEY_SETTINGS, &self.settings),
        }
        match (&self.vault, &self.legacy) {
            (None, _) => eframe::set_value(storage, KEY_STORE, &self.store),
            // Not migrated yet (never unlocked): keep the old copy.
            (Some(_), Some(old)) => eframe::set_value(storage, KEY_STORE, old),
            (Some(_), None) => storage.remove_string(KEY_STORE),
        }
        storage.set_string(KEY_AGENT_WORKSPACE, self.agent_workspace.clone().unwrap_or_default());
    }

    // On macOS the glow renderer is compiled in as the Metal fallback, which gives `on_exit` a
    // glow-context parameter; every other platform (wgpu only) keeps the no-argument form.
    #[cfg(not(target_os = "macos"))]
    fn on_exit(&mut self) {
        self.on_exit_shutdown();
    }

    #[cfg(target_os = "macos")]
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.on_exit_shutdown();
    }
}

impl App {
    /// Answer every pending approval as denied and stop the backend (shared by both `on_exit`
    /// signatures above).
    fn on_exit_shutdown(&mut self) {
        let pending: Vec<_> = self.approvals.iter().map(|a| a.id).collect();
        pending.into_iter().for_each(|id| self.answer_approval(id, false));
        self.backend.shutdown(Duration::from_secs(3));
        // Queued stream records and log lines reach the disk before the process ends.
        rxt_engine::stream::shutdown();
        if let Some(c) = &self.diagnostics {
            c.flush();
        }
    }
}
