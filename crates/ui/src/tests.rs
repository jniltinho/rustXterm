//! kittest UI tests and snapshots of every screen and dialog in Light and Dark.
//! Regenerate snapshots with `UPDATE_SNAPSHOTS=1`.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use egui::accesskit::Role;
use egui::{Key, Modifiers, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use rxt_core::{
    Command, CommandId, ConnState, Direction, Event, FileEntry, FileKind, Folder, FolderId, Framebuffer,
    HostKeyDecision, JumpHost, PromptAnswer, PromptId, PromptKind, Protocol, Secret, Session, SessionId, TabId,
    ThemeMode, TransferId, Tunnel, TunnelKind, TunnelState,
};
use rxt_core::{DesktopEvent, DesktopInput, Rect};
use rxt_platform::fake::Fake;

use rxt_store::moba_ini::{ImportReport, Skipped};

use crate::app::{Action, App, Backend, Modal};
use crate::session_editor::{Editor, Page};
use crate::sftp_panel::{SftpAction, TransferRow};
use crate::store_ui::StoreDialog;
use crate::tunnels::TunnelEditor;
use crate::vault::Vault;

/// Engine double: records commands and answers `Open`/`Close` like the engine.
#[derive(Clone, Default)]
struct TestEngine {
    sent: Rc<RefCell<Vec<Command>>>,
    events: Rc<RefCell<VecDeque<Event>>>,
    next: Rc<RefCell<u64>>,
    framebuffers: Rc<RefCell<HashMap<TabId, Framebuffer>>>,
    clipboard: Rc<RefCell<HashMap<TabId, String>>>,
    /// `save_secret` calls, in order, interleaved with sent answers as `Command::Answer` markers.
    saves: Rc<RefCell<Vec<PromptId>>>,
    /// `set_agent_control` calls, in order.
    agent_control: Rc<RefCell<Vec<bool>>>,
    /// Remote output waiting to be drained, per tab.
    output: Rc<RefCell<HashMap<TabId, Vec<u8>>>>,
    /// Remote monitoring samples waiting in the latest-sample slot, per tab.
    metrics: Rc<RefCell<HashMap<TabId, rxt_core::MetricsSample>>>,
    /// `set_remote_monitoring` calls, in order.
    monitoring: Rc<RefCell<Vec<bool>>>,
    /// `Command::SetMonitorInterval` values (sent at startup, so kept out of `sent`).
    intervals: Rc<RefCell<Vec<u32>>>,
}

impl TestEngine {
    fn sent(&self) -> Vec<Command> {
        self.sent.borrow().clone()
    }
    fn push(&self, e: Event) {
        self.events.borrow_mut().push_back(e);
    }
}

impl Backend for TestEngine {
    fn send(&self, command: Command) -> rxt_core::Result<()> {
        match &command {
            Command::Open { session } => {
                *self.next.borrow_mut() += 1;
                let tab = TabId(*self.next.borrow());
                self.push(Event::TabOpened { tab, session: session.clone() });
            }
            Command::Close { tab } => self.push(Event::Closed { tab: *tab }),
            Command::SetMonitorInterval { secs } => {
                self.intervals.borrow_mut().push(*secs);
                return Ok(());
            }
            _ => {}
        }
        self.sent.borrow_mut().push(command);
        Ok(())
    }
    fn try_event(&self) -> Option<Event> {
        self.events.borrow_mut().pop_front()
    }
    fn drain_output(&self, tab: TabId, budget: usize, out: &mut Vec<u8>) -> usize {
        let mut all = self.output.borrow_mut();
        let Some(data) = all.get_mut(&tab) else { return 0 };
        let n = budget.min(data.len());
        out.extend(data.drain(..n));
        n
    }
    fn pending_output(&self) -> Vec<TabId> {
        self.output.borrow().iter().filter(|(_, d)| !d.is_empty()).map(|(t, _)| *t).collect()
    }
    fn with_framebuffer(&self, tab: TabId, f: &mut dyn FnMut(&mut Framebuffer)) -> bool {
        self.framebuffers.borrow_mut().get_mut(&tab).map(f).is_some()
    }
    fn take_clipboard(&self, tab: TabId) -> Option<String> {
        self.clipboard.borrow_mut().remove(&tab)
    }
    fn save_secret(&self, prompt: PromptId) {
        assert!(
            !self.sent.borrow().iter().any(|c| matches!(c, Command::Answer { prompt: p, .. } if *p == prompt)),
            "save requested after the answer"
        );
        self.saves.borrow_mut().push(prompt);
    }
    fn set_agent_control(&self, enabled: bool) {
        self.agent_control.borrow_mut().push(enabled);
    }
    fn take_metrics(&self, tab: TabId) -> Option<rxt_core::MetricsSample> {
        self.metrics.borrow_mut().remove(&tab)
    }
    fn set_remote_monitoring(&self, enabled: bool) {
        self.monitoring.borrow_mut().push(enabled);
    }
}

struct Env {
    h: Harness<'static, App>,
    engine: TestEngine,
    platform: Arc<Fake>,
}

fn env_with(dark: Option<bool>, storage: Option<&dyn eframe::Storage>) -> Env {
    let platform = Arc::new(Fake { root: std::env::temp_dir().join("rxt-ui-tests"), ..Fake::default() });
    platform.set_dark(dark);
    let engine = TestEngine::default();
    let app = App::new(storage, platform.platform(), engine.clone());
    let h = harness(app);
    Env { h, engine, platform }
}

/// 1280×800 window (scaled by `RXT_PRINTS_SCALE`) showing `app` like eframe does.
fn harness(mut app: App) -> Harness<'static, App> {
    // Screens other than the first-run Home show the regular Home page.
    app.settings.first_run_theme_done = true;
    // Screens keep the page they set; the automatic SFTP switch has its own tests.
    app.settings.sftp_auto_switch = false;
    // Terminal screens stay empty; the session banner has its own tests and snapshot.
    app.settings.show_session_banner = false;
    Harness::builder().with_size(vec2(1280.0, 800.0)).with_pixels_per_point(prints_scale()).build_ui_state(
        |ui, app: &mut App| {
            // Like eframe, the app gets the whole window (kittest adds an 8 px outer margin).
            let full = egui::UiBuilder::new().max_rect(ui.ctx().content_rect());
            ui.scope_builder(full, |ui| app.show(ui));
        },
        app,
    )
}

/// `RXT_PRINTS_SCALE` (set by `cargo xtask prints` for its `@2x` run); 1.0 otherwise.
fn prints_scale() -> f32 {
    std::env::var("RXT_PRINTS_SCALE").ok().and_then(|v| v.parse().ok()).filter(|s: &f32| *s > 0.0).unwrap_or(1.0)
}

fn env() -> Env {
    env_with(None, None)
}

impl Env {
    fn app(&mut self) -> &mut App {
        self.h.state_mut()
    }
    fn settle(&mut self) {
        self.h.run_steps(10);
    }
    fn event(&mut self, e: Event) {
        self.engine.push(e);
        self.settle();
    }
    fn open(&mut self, s: Session) -> TabId {
        self.app().apply(Action::Open(Box::new(s)));
        self.settle();
        self.h.state().active.unwrap_or_default()
    }
    fn connect(&mut self, s: Session) -> TabId {
        let tab = self.open(s);
        self.event(Event::State { tab, state: ConnState::Connected });
        tab
    }
    /// Snapshot in Light and Dark.
    fn snap(&mut self, name: &str) {
        // No pointer in prints: kittest draws it, and hover states would vary between screens.
        self.h.event(egui::Event::PointerGone);
        for (theme, suffix) in [(ThemeMode::Light, "light"), (ThemeMode::Dark, "dark")] {
            self.app().settings.theme = theme;
            self.settle();
            let name = format!("{name}_{suffix}");
            if !cfg!(target_os = "linux") {
                // Linux (lavapipe) images are the reference; other targets only render (W10).
            } else if prints_scale() == 1.0 {
                self.h.snapshot(&name);
            } else {
                // Scaled prints are never compared: written next to the 1× prints only.
                let dir = std::env::var("RXT_PRINTS_DIR").unwrap_or_else(|_| "../../target/prints".into());
                std::fs::create_dir_all(&dir).unwrap();
                let img = self.h.render().unwrap();
                img.save(format!("{dir}/{name}@{}x.png", prints_scale())).unwrap();
            }
        }
    }
}

/// The terminal view (the menu bar also has a "Terminal" button).
fn terminal_view<'a>(e: &'a Env) -> egui_kittest::Node<'a> {
    e.h.get_by(|n| n.label().as_deref() == Some("Terminal") && n.role() != Role::Button)
}

/// Switch the open Settings window to the tab with this label.
fn settings_tab(e: &mut Env, label: &str) {
    e.settle();
    // The window's tab buttons come after the toolbar and menu buttons of the same name.
    let tab =
        e.h.query_all_by_label(label)
            .rfind(|n| !matches!(n.accesskit_node().role(), Role::Label | Role::GenericContainer));
    tab.unwrap_or_else(|| panic!("no settings tab {label}")).click();
    e.settle();
}

fn ssh(host: &str) -> Session {
    Session::new(Protocol::Ssh, host, Some("alice".into()), None)
}

fn sample_store(app: &mut App) {
    let lab = app.store.save_folder(None, None, "Lab".into());
    let prod = app.store.save_folder(None, Some(lab), "Bastions".into());
    let web1 = Session { icon: Some("monitor-debian".into()), ..ssh("web1.lab.example") };
    for (s, folder) in [
        (web1, Some(lab)),
        (Session::new(Protocol::Sftp, "files.lab.example", None, None), Some(lab)),
        (ssh("bastion.lab.example"), Some(prod)),
        (Session::new(Protocol::Rdp, "win.lab.example", Some("bob".into()), None), None),
        (Session::new(Protocol::Vnc, "kiosk.lab.example", None, None), None),
    ] {
        let id = app.store.upsert(s);
        app.store.move_session(id, folder);
    }
}

// ---- snapshots ----

#[test]
fn snapshot_main_empty() {
    let mut e = env();
    e.snap("main_empty");
}

#[test]
fn snapshot_first_run_home() {
    let mut e = env();
    e.app().settings.first_run_theme_done = false;
    e.snap("home_first_run");
}

/// SSH tab connected with `info`, the remote sending `remote` right after login.
fn connect_with_banner(e: &mut Env, s: Session, info: rxt_core::SshInfo, remote: &[u8]) -> TabId {
    e.app().settings.show_session_banner = true;
    let tab = e.open(s);
    e.engine.output.borrow_mut().insert(tab, remote.to_vec());
    e.engine.push(Event::SshInfo { tab, info: Box::new(info) });
    e.event(Event::State { tab, state: ConnState::Connected });
    tab
}

fn banner_facts() -> rxt_core::SshInfo {
    rxt_core::SshInfo {
        server_id: "SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13".into(),
        kex: "curve25519-sha256".into(),
        cipher: "chacha20-poly1305@openssh.com".into(),
        compression: true,
        host_key_algorithm: "ssh-ed25519".into(),
        host_key_fingerprint: "SHA256:Zm9yIHRlc3RzIG9ubHkgLSBmaWN0aW9uYWwga2V5".into(),
        x11: false,
        x11_unavailable: Some("no usable $DISPLAY".into()),
        agent: false,
    }
}

fn tab_text(e: &Env, tab: TabId) -> String {
    e.h.state().tabs.iter().find(|t| t.id == tab).and_then(|t| t.term.as_ref()).map(|t| t.visible_text()).unwrap()
}

#[test]
fn connected_ssh_tab_shows_session_box_before_remote_output() {
    let mut e = env();
    let mut s = ssh("web1.lab.example");
    s.ssh.x11_forwarding = true;
    let tab = connect_with_banner(&mut e, s, banner_facts(), b"Welcome to the lab\r\nalice@web1:~$ ");
    let text = tab_text(&e, tab);
    let at = |what: &str| text.find(what).unwrap_or_else(|| panic!("no {what:?} in\n{text}"));
    assert!(at("\u{256d}\u{2500} rustXterm ") < at("SSH session"), "{text}");
    assert!(text.lines().nth(1).is_some_and(|l| l.starts_with("\u{2502} SSH session")), "{text}");
    assert!(at("SSH session to alice@web1.lab.example") < at("X11 forwarding (no usable $DISPLAY)"));
    assert!(at("Server: SSH-2.0-OpenSSH_9.6p1") < at("Welcome to the lab"), "{text}");
    // Nothing of the box was sent to the remote.
    assert!(!e.engine.sent().iter().any(|c| matches!(c, Command::Input { .. })));

    e.app().settings.show_session_banner = false;
    let quiet = e.open(ssh("web2.lab.example"));
    e.engine.output.borrow_mut().insert(quiet, b"plain$ ".to_vec());
    e.event(Event::State { tab: quiet, state: ConnState::Connected });
    let text = tab_text(&e, quiet);
    assert!(text.starts_with("plain$") && !text.contains("rustXterm"), "{text}");
}

#[test]
fn telnet_tab_shows_reduced_box() {
    let mut e = env();
    e.app().settings.show_session_banner = true;
    let tab = e.connect(Session::new(Protocol::Telnet, "router.lab.example", None, None));
    let text = tab_text(&e, tab);
    assert!(text.contains("Telnet session to router.lab.example") && text.contains("Encryption"), "{text}");
    assert!(!text.contains("Compression"), "{text}");
}

#[test]
fn snapshot_terminal_session_banner() {
    let mut e = env();
    let mut s = ssh("web1.lab.example");
    s.port = 2222;
    s.ssh.keepalive_secs = 30;
    s.ssh.jump_hosts =
        vec![JumpHost { host: "bastion.lab.example".into(), port: 22, user: "alice".into(), key_file: None }];
    let motd = b"Welcome to the fictional lab server.\r\n\r\nLast login: Tue Oct 13 10:00:00 2026\r\nalice@web1:~$ ";
    connect_with_banner(&mut e, s, banner_facts(), motd);
    e.snap("terminal_session_banner");
}

#[test]
fn snapshot_menus_open() {
    let mut e = env();
    e.connect(ssh("web1.lab.example"));
    for title in ["Terminal", "Sessions", "View", "Tools", "Settings", "Help"] {
        // The menu bar comes first in reading order (the toolbar has buttons of the same name).
        let menu = e.h.query_all_by_role_and_label(Role::Button, title).next();
        menu.unwrap_or_else(|| panic!("no menu {title}")).click();
        e.settle();
        e.snap(&format!("menu_{}", title.to_lowercase()));
        e.h.key_press(Key::Escape);
        e.settle();
    }
}

#[test]
fn snapshot_sessions_tree_and_recent() {
    let mut e = env();
    sample_store(e.app());
    let recent = e.app().store.sessions.clone();
    recent.iter().for_each(|s| e.app().store.push_recent(s));
    e.snap("sessions_tree");
    e.app().apply(Action::Run(CommandId::FIND_SESSION));
    e.settle();
    assert!(e.h.get_by_label("Filter sessions").is_focused(), "Find session focuses the field");
    e.app().filter = Some("lab".into());
    e.snap("sessions_filtered");
}

#[test]
fn snapshot_main_with_tabs() {
    let mut e = env();
    let rdp = e.open(Session::new(Protocol::Rdp, "win.lab.example", None, None));
    e.event(Event::State { tab: rdp, state: ConnState::Error("unsupported: RDP arrives in M2".into()) });
    e.open(Session::new(Protocol::Sftp, "files.lab.example", None, None));
    e.connect(ssh("web1.lab.example"));
    e.snap("main_tabs");
}

#[test]
fn snapshot_disconnected_tab_offers_reconnect() {
    let mut e = env();
    let tab = e.connect(ssh("web1.lab.example"));
    e.event(Event::State {
        tab,
        state: ConnState::Disconnected {
            reason: "connection closed by peer".into(),
            reconnect_at: Some(1_700_000_000),
        },
    });
    let text = e.h.state().tabs[0].term.as_ref().map(|t| t.visible_text()).unwrap_or_default();
    assert!(text.contains("Session stopped (connection closed by peer)"), "{text}");
    assert!(text.contains("Next automatic attempt at"), "{text}");
    e.snap("tab_disconnected");
    terminal_view(&e).click();
    e.h.event(egui::Event::Text("R".into()));
    e.settle();
    assert!(e.engine.sent().contains(&Command::Reconnect { tab }));
}

#[test]
fn snapshot_multiexec() {
    let mut e = env();
    let a = e.connect(ssh("web1"));
    e.connect(ssh("web2"));
    e.app().run(CommandId::MULTIEXEC);
    e.app().apply(Action::SelectMultiExec(a, true));
    e.snap("multiexec");
}

#[test]
fn snapshot_session_editor_pages() {
    let mut e = env();
    sample_store(e.app());
    let mut s = ssh("web1.lab.example");
    s.name = "web1".into();
    s.ssh.key_file = Some("~/.ssh/id_ed25519".into());
    s.ssh.remote_command = Some("htop".into());
    s.ssh.jump_hosts.push(JumpHost {
        host: "bastion.lab.example".into(),
        port: 22,
        user: "alice".into(),
        key_file: None,
    });
    s.ssh.env.push(("LANG".into(), "C.UTF-8".into()));
    s.terminal.font_size = Some(14.0);
    for (page, name) in [
        (Page::Advanced, "editor_ssh_advanced"),
        (Page::Network, "editor_ssh_network"),
        (Page::Terminal, "editor_ssh_terminal"),
    ] {
        let mut ed = Editor::new(s.clone(), true);
        ed.page = page;
        e.app().editor = Some(ed);
        e.snap(name);
    }
    for p in [Protocol::Sftp, Protocol::Ftp, Protocol::Ftps, Protocol::Rdp, Protocol::Vnc] {
        e.app().editor = Some(Editor::new(Session::new(p, "host.lab.example", None, None), true));
        e.snap(&format!("editor_{}", p.scheme()));
    }
    let mut ed = Editor::new(Session::default(), true);
    ed.validate();
    e.app().editor = Some(ed);
    e.snap("editor_error");
    e.app().editor = None;
    e.app().apply(Action::NewSession(None));
    e.snap("editor_empty");
}

fn prompt(e: &mut Env, id: u64, kind: PromptKind) {
    e.event(Event::Prompt { id: PromptId(id), tab: None, kind });
}

#[test]
fn snapshot_dialogs() {
    let mut e = env();
    let tab = e.connect(ssh("web1.lab.example"));
    let prompts = [
        (
            "dialog_hostkey_unknown",
            PromptKind::HostKeyUnknown {
                host: "web1.lab.example".into(),
                port: 22,
                algorithm: "ssh-ed25519".into(),
                fingerprint: "SHA256:Zq3Xb1mY0nC9u4PZ0r0yTqgk2k8fJw4v6Q1oR2sT3uI".into(),
            },
        ),
        (
            "dialog_password",
            PromptKind::Password { user: "alice".into(), host: "web1.lab.example".into(), retry: true },
        ),
        ("dialog_passphrase", PromptKind::KeyPassphrase { path: "/home/alice/.ssh/id_ed25519".into() }),
        (
            "dialog_keyboard_interactive",
            PromptKind::KeyboardInteractive {
                name: "Two-factor authentication".into(),
                instructions: "Enter your password and the code from your authenticator.".into(),
                prompts: vec![("Password:".into(), false), ("Verification code:".into(), true)],
            },
        ),
        ("dialog_overwrite", PromptKind::Overwrite { path: "/home/alice/report.pdf".into() }),
    ];
    for (i, (name, kind)) in prompts.into_iter().enumerate() {
        prompt(&mut e, i as u64, kind);
        e.snap(name);
        e.app().prompts.clear();
    }
    let modals = [
        (
            "dialog_hostkey_changed",
            Modal::HostKeyChanged {
                host: "web1.lab.example".into(),
                old: "SHA256:Zq3Xb1mY0nC9u4PZ0r0yTqgk2k8fJw4v6Q1oR2sT3uI".into(),
                new: "SHA256:8aKd02LmQvXyP3sN7tRb5cHf1gJ9wE4uZ6oI0pA2yTs".into(),
            },
        ),
        ("dialog_paste", Modal::Paste { tab, text: "sudo apt update\nsudo apt upgrade -y\nreboot".into() }),
        ("dialog_close_confirm", Modal::ConfirmClose(tab)),
        ("dialog_delete_remote", Modal::DeleteRemote { local: false, path: "/var/log/old".into() }),
        ("dialog_chmod", Modal::Chmod { path: "/home/alice/run.sh".into(), mode: "755".into(), error: None }),
        ("dialog_about", Modal::About),
        (
            "dialog_edit_folder",
            Modal::FolderName {
                id: Some(FolderId(1)),
                parent: None,
                name: "Production".into(),
                icon: Some("folder-blue".into()),
            },
        ),
    ];
    for (name, m) in modals {
        e.app().modal = Some(m);
        e.snap(name);
    }
}

#[test]
fn snapshot_settings() {
    let mut e = env();
    e.app().run(CommandId::SETTINGS);
    e.app().settings.shortcuts.insert(CommandId::FIND.as_str().into(), "Ctrl+F".into());
    // Shown at its default (the harness turns it off for other screens).
    e.app().settings.sftp_auto_switch = true;
    e.snap("settings");
    for tab in ["Display", "SSH", "Shortcuts"] {
        settings_tab(&mut e, tab);
        e.snap(&format!("settings_{}", tab.to_lowercase()));
    }
}

#[test]
fn snapshot_tunnels() {
    let mut e = env();
    let via = e.app().store.upsert(ssh("bastion.lab.example"));
    e.app().store.tunnels = vec![
        Tunnel {
            name: "postgres".into(),
            via,
            bind_port: 15432,
            dest_host: "db.internal".into(),
            dest_port: 5432,
            autostart: true,
            ..Tunnel::default()
        },
        Tunnel { name: "socks".into(), via, kind: TunnelKind::Dynamic, bind_port: 1080, ..Tunnel::default() },
        Tunnel {
            name: "web".into(),
            via,
            kind: TunnelKind::Remote,
            bind_port: 8080,
            dest_host: "localhost".into(),
            dest_port: 80,
            ..Tunnel::default()
        },
    ];
    e.app().tunnel_states.insert("postgres".into(), TunnelState::Running);
    e.app().tunnel_states.insert("web".into(), TunnelState::Error("port 8080 is in use".into()));
    e.app().show_tunnels = true;
    e.snap("tunnels");
    let t = Tunnel {
        name: "exposed".into(),
        via,
        bind_addr: "0.0.0.0".into(),
        bind_port: 9000,
        dest_host: "app".into(),
        dest_port: 80,
        ..Tunnel::default()
    };
    e.app().tunnel_editor = Some(TunnelEditor { tunnel: t, original: None, error: None });
    e.snap("tunnels_editor_exposure");
    let t = Tunnel {
        name: "postgres".into(),
        via,
        bind_port: 15432,
        dest_host: "db.internal".into(),
        dest_port: 5432,
        ..Tunnel::default()
    };
    e.app().tunnel_editor = Some(TunnelEditor { tunnel: t, original: Some("postgres".into()), error: None });
    e.snap("tunnels_editor");
    e.app().tunnel_editor = None;
    e.app().store.tunnels.clear();
    e.snap("tunnels_empty");
}

fn listing() -> Vec<FileEntry> {
    let f = |name: &str, kind, size, mode| FileEntry {
        name: name.into(),
        kind,
        size,
        modified: Some(1_700_000_000),
        mode: Some(mode),
        owner: Some("alice".into()),
        group: Some("staff".into()),
    };
    vec![
        f(".bashrc", FileKind::File, 3771, 0o644),
        f("logs", FileKind::Dir, 4096, 0o755),
        f("projects", FileKind::Dir, 4096, 0o750),
        f("current", FileKind::Symlink, 12, 0o777),
        f("report.pdf", FileKind::File, 1_572_864, 0o640),
        f("run.sh", FileKind::File, 512, 0o755),
    ]
}

#[test]
fn snapshot_sftp_panel_and_transfers() {
    let mut e = env();
    let tab = e.connect(ssh("web1.lab.example"));
    e.app().sidebar_tab = crate::app::SidebarTab::Sftp;
    e.event(Event::Listing { tab, path: "/home/alice".into(), entries: listing() });
    let now = Instant::now();
    let row = |id, direction, remote: &str, total, done, result| TransferRow {
        id: TransferId(id),
        tab,
        direction,
        remote: remote.into(),
        local: format!("/home/user/Downloads/{}", remote.rsplit('/').next().unwrap_or_default()),
        total,
        done,
        speed: 2_621_440.0,
        last: now,
        result,
    };
    e.app().transfers = vec![
        row(1, Direction::Download, "/home/alice/report.pdf", Some(1_572_864), 786_432, None),
        row(2, Direction::Upload, "/home/alice/backup.tar", Some(10_485_760), 10_485_760, Some(Ok(10_485_760))),
        row(
            3,
            Direction::Upload,
            "/home/alice/locked.txt",
            Some(10),
            0,
            Some(Err("permission denied: /home/alice/locked.txt".into())),
        ),
        row(4, Direction::Download, "/home/alice/notes.txt", Some(2048), 2048, Some(Ok(2048))),
    ];
    e.app().sftp.selected = Some("report.pdf".into());
    e.snap("sftp_panel");
    e.app().sftp.error = Some("permission denied".into());
    e.snap("sftp_panel_error");
    e.app().sftp.error = None;
    e.app().sftp.loading = true;
    e.app().sftp.loading_since = Some(0.0);
    e.snap("sftp_panel_loading");
    e.app().sftp.loading = false;
    e.app().sftp.loading_since = None;
    e.app().sftp.tab = None;
    e.app().active = None;
    e.snap("sftp_panel_detached");
}

// ---- behavior ----

#[test]
fn quick_connect_error_is_inline_and_opens_nothing() {
    let mut e = env();
    e.h.get_by_label("Quick connect").click();
    e.settle();
    e.h.get_by_label("Quick connect").type_text("ssh://");
    e.h.key_press(Key::Enter);
    e.settle();
    assert!(e.h.query_all_by_value("invalid input: missing host").next().is_some());
    assert!(e.engine.sent().is_empty());
    assert!(e.h.state().tabs.is_empty());
}

#[test]
fn snapshot_quick_connect_lists_matching_sessions() {
    let mut e = env();
    for name in ["psftp", "pssh", "prdp"] {
        let mut s = ssh(&format!("{name}.lab.example"));
        s.name = name.into();
        e.app().store.upsert(s);
    }
    let helps = e.h.query_all_by_label("Help").count();
    e.h.get_by_label("Quick connect").click();
    e.settle();
    e.h.get_by_label("Quick connect").type_text("ps");
    e.settle();
    for label in ["psftp", "pssh", "New session..."] {
        assert!(e.h.query_by_label(label).is_some(), "{label} listed");
    }
    assert_eq!(e.h.query_all_by_label("Help").count(), helps + 1, "Help entry listed");
    assert!(e.h.query_by_label("prdp").is_none(), "non-matching session hidden");
    e.snap("quick_connect_list");
    e.h.key_press(Key::ArrowDown);
    e.h.key_press(Key::Enter);
    e.settle();
    let Some(Command::Open { session }) = e.engine.sent().into_iter().next() else { panic!("no open") };
    assert_eq!(session.name, "pssh");
    assert!(e.h.state().quick.is_empty());
    assert!(e.h.query_by_label("prdp").is_some(), "tree is back");
}

#[test]
fn quick_connect_opens_unsaved_session() {
    let mut e = env();
    e.h.get_by_label("Quick connect").click();
    e.settle();
    e.h.get_by_label("Quick connect").type_text("alice@lab.example:2222");
    e.h.key_press(Key::Enter);
    e.settle();
    let Some(Command::Open { session }) = e.engine.sent().into_iter().next() else { panic!("no open") };
    assert_eq!((session.host.as_str(), session.port, session.user.as_deref()), ("lab.example", 2222, Some("alice")));
    assert!(e.h.state().store.sessions.is_empty());
    assert_eq!(e.h.state().tabs.len(), 1);
    assert!(e.h.state().quick_error.is_none());
}

#[test]
fn tab_operations() {
    let mut e = env();
    let a = e.connect(ssh("a"));
    let b = e.connect(ssh("b"));
    let c = e.connect(ssh("c"));
    e.app().apply(Action::MoveTab { from: 0, to: 2 });
    let order: Vec<TabId> = e.h.state().tabs.iter().map(|t| t.id).collect();
    assert_eq!(order, [b, c, a]);
    e.app().apply(Action::RenameTab(b, "build".into()));
    assert_eq!(e.h.state().tabs[0].title, "build");
    e.app().apply(Action::OnTab(c, CommandId::DUPLICATE_TAB));
    assert!(e.engine.sent().contains(&Command::Duplicate { tab: c }));
    // Close others with live tabs asks first.
    e.app().apply(Action::OnTab(c, CommandId::CLOSE_OTHERS));
    assert_eq!(e.h.state().modal, Some(Modal::ConfirmCloseOthers(c)));
    e.app().modal = None;
    e.app().settings.confirm_close = false;
    e.app().apply(Action::OnTab(c, CommandId::CLOSE_OTHERS));
    e.settle();
    let left: Vec<TabId> = e.h.state().tabs.iter().map(|t| t.id).collect();
    assert_eq!(left, [c]);
    assert!(e.engine.sent().contains(&Command::Close { tab: a }));
}

#[test]
fn next_and_previous_tab_cycle_through_home() {
    let mut e = env();
    let a = e.connect(ssh("a"));
    let b = e.connect(ssh("b"));
    e.app().run(CommandId::NEXT_TAB);
    assert_eq!(e.h.state().active, None);
    e.app().run(CommandId::NEXT_TAB);
    assert_eq!(e.h.state().active, Some(a));
    e.app().run(CommandId::PREV_TAB);
    e.app().run(CommandId::PREV_TAB);
    assert_eq!(e.h.state().active, Some(b));
}

#[test]
fn closing_live_tab_asks_and_cancel_keeps_connection() {
    let mut e = env();
    let tab = e.connect(ssh("web1"));
    e.h.get_by_label("Close 1. alice@web1").click();
    e.settle();
    assert_eq!(e.h.state().modal, Some(Modal::ConfirmClose(tab)));
    e.h.get_by_label("Cancel").click();
    e.settle();
    assert!(e.h.state().modal.is_none());
    assert_eq!(e.h.state().tabs.len(), 1);
    assert!(!e.engine.sent().iter().any(|c| matches!(c, Command::Close { .. })));
    // Shortcut path, then confirm.
    e.h.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::W);
    e.settle();
    e.h.get_by_label("Close").click();
    e.settle();
    assert!(e.h.state().tabs.is_empty());
    assert!(e.engine.sent().contains(&Command::Close { tab }));
}

#[test]
fn disconnected_tab_closes_without_confirmation() {
    let mut e = env();
    let tab = e.connect(ssh("web1"));
    e.event(Event::State { tab, state: ConnState::Disconnected { reason: String::new(), reconnect_at: None } });
    e.app().run(CommandId::CLOSE_TAB);
    assert!(e.h.state().modal.is_none());
    assert!(e.h.state().tabs.is_empty());
}

#[test]
fn ctrl_d_closes_stopped_session_and_r_reconnects() {
    let mut e = env();
    let tab = e.connect(ssh("web1"));
    terminal_view(&e).click();
    e.settle();
    // The remote shell exits: the banner appears above the still-focused terminal.
    e.event(Event::State { tab, state: ConnState::Disconnected { reason: String::new(), reconnect_at: None } });
    let text = e.h.state().tabs[0].term.as_ref().map(|t| t.visible_text()).unwrap_or_default();
    assert!(text.contains("Session stopped\n    - Press <Return> to exit tab"), "{text}");
    e.h.key_press(Key::R);
    e.h.event(egui::Event::Text("r".into()));
    e.settle();
    assert!(e.engine.sent().contains(&Command::Reconnect { tab }));
    e.event(Event::State { tab, state: ConnState::Disconnected { reason: String::new(), reconnect_at: None } });
    e.h.key_press_modifiers(Modifiers::CTRL, Key::D);
    e.settle();
    assert!(e.h.state().tabs.is_empty(), "the terminal keeps focus when the banner appears");
    assert!(e.engine.sent().contains(&Command::Close { tab }));
    assert!(!e.engine.sent().iter().any(|c| matches!(c, Command::Input { .. })), "no input reaches a stopped session");
}

#[test]
fn ctrl_c_reaches_terminal_and_fires_no_action() {
    let mut e = env();
    let tab = e.connect(ssh("web1"));
    terminal_view(&e).click();
    e.settle();
    let before = e.engine.sent().len();
    e.h.key_press_modifiers(Modifiers::CTRL, Key::C);
    e.settle();
    let sent = e.engine.sent();
    assert!(sent[before..].contains(&Command::Input { tab, bytes: vec![0x03] }), "{sent:?}");
    assert!(sent[before..].iter().all(|c| matches!(c, Command::Input { .. } | Command::Resize { .. })));
    assert_eq!(e.h.state().tabs.len(), 1);
    assert!(e.h.state().modal.is_none());
}

#[test]
fn theme_follows_system_live() {
    let mut e = env_with(Some(false), None);
    e.settle();
    assert!(!e.h.state().is_dark());
    e.platform.set_dark(Some(true));
    e.settle();
    assert!(e.h.state().is_dark());
    assert!((e.h.ctx.theme() == egui::Theme::Dark));
    e.platform.set_dark(Some(false));
    e.settle();
    assert!(!e.h.state().is_dark());
    e.app().settings.theme = ThemeMode::Dark;
    e.settle();
    assert!(e.h.state().is_dark());
}

#[test]
fn portal_missing_falls_back_to_light() {
    let mut e = env_with(None, None);
    e.settle();
    assert!(!e.h.state().is_dark());
    assert!(!(e.h.ctx.theme() == egui::Theme::Dark));
    assert!(e.h.state().status.is_empty());
}

#[test]
fn accessibility_names_present() {
    let mut e = env();
    e.settle();
    for name in [
        "Session",
        "Sessions",
        "Tunneling",
        "MultiExec",
        "Settings",
        "Help",
        "Exit",
        "Quick connect",
        "New session",
        "Home",
    ] {
        assert!(e.h.query_all_by_label(name).next().is_some(), "missing accessible name {name}");
    }
    e.connect(ssh("web1"));
    for name in ["1. alice@web1", "Close 1. alice@web1", "Terminal"] {
        assert!(e.h.query_all_by_label(name).next().is_some(), "missing accessible name {name}");
    }
}

#[test]
fn sftp_follows_active_tab_and_terminal_folder() {
    let mut e = env();
    let a = e.connect(ssh("a"));
    let b = e.connect(ssh("b"));
    assert!(e.engine.sent().contains(&Command::List { tab: b, path: "~".into() }));
    e.app().apply(Action::SelectTab(Some(a)));
    e.settle();
    assert_eq!(e.h.state().sftp.tab, Some(a));
    assert!(e.engine.sent().contains(&Command::List { tab: a, path: "~".into() }));
    e.event(Event::CwdChanged { tab: a, path: "/var/log".into() });
    assert!(e.engine.sent().contains(&Command::List { tab: a, path: "/var/log".into() }));
    e.event(Event::Listing { tab: a, path: "/var/log".into(), entries: listing() });
    assert_eq!(e.h.state().sftp.path, "/var/log");
    // Follow off: cwd changes are ignored.
    e.app().apply(Action::Sftp(SftpAction::SetFollow(false)));
    assert!(e.engine.sent().contains(&Command::SetFollowCwd { tab: a, enabled: false }));
    e.event(Event::CwdChanged { tab: a, path: "/tmp".into() });
    assert!(!e.engine.sent().contains(&Command::List { tab: a, path: "/tmp".into() }));
}

fn auto_env() -> Env {
    let mut e = env();
    e.app().settings.sftp_auto_switch = true;
    e
}

fn page(e: &Env) -> crate::app::SidebarTab {
    e.h.state().sidebar_tab
}

#[test]
fn sftp_page_shown_after_login_of_active_tab_only() {
    use crate::app::SidebarTab;
    let mut e = auto_env();
    e.app().settings.sidebar_open = false;
    let a = e.open(ssh("a"));
    // Prompts pending: still connecting, Sessions stays.
    e.event(Event::Prompt {
        id: PromptId(1),
        tab: Some(a),
        kind: PromptKind::Password { user: "alice".into(), host: "a".into(), retry: false },
    });
    assert_eq!((page(&e), e.h.state().settings.sidebar_open), (SidebarTab::Sessions, false));
    e.app().prompts.clear();
    e.event(Event::State { tab: a, state: ConnState::Connected });
    assert_eq!((page(&e), e.h.state().settings.sidebar_open), (SidebarTab::Sftp, true));
    // Automatic switches are not saved as the chosen page.
    assert_eq!(e.h.state().settings.sidebar_page, "sessions");
    // A tab that logs in while another one is active does not switch the page.
    let b = e.open(ssh("b"));
    assert_eq!(page(&e), SidebarTab::Sessions, "connecting tab is active");
    e.app().apply(Action::SelectTab(Some(a)));
    e.settle();
    assert_eq!(page(&e), SidebarTab::Sftp);
    e.app().apply(Action::SelectTab(None));
    e.settle();
    assert_eq!(page(&e), SidebarTab::Sessions, "Home shows Sessions");
    e.event(Event::State { tab: b, state: ConnState::Connected });
    assert_eq!(page(&e), SidebarTab::Sessions);
    e.app().settings.sidebar_open = false;
    e.app().apply(Action::SelectTab(Some(b)));
    e.settle();
    assert_eq!((page(&e), e.h.state().sftp.tab, e.h.state().settings.sidebar_open), (SidebarTab::Sftp, Some(b), true));
    // Setting off: no automatic switch.
    let mut off = env();
    let c = off.connect(ssh("c"));
    assert_eq!((page(&off), off.h.state().sftp.tab), (SidebarTab::Sessions, Some(c)));
}

#[test]
fn sftp_state_kept_per_tab_and_dropped_on_disconnect() {
    use crate::app::SidebarTab;
    let mut e = auto_env();
    let a = e.connect(ssh("a"));
    e.event(Event::Listing { tab: a, path: "/var/log".into(), entries: listing() });
    e.app().apply(Action::Sftp(SftpAction::Select("logs".into())));
    e.app().sftp.hide_hidden = true;
    let b = e.connect(ssh("b"));
    e.event(Event::Listing { tab: b, path: "/home/alice".into(), entries: Vec::new() });
    assert_eq!(e.h.state().sftp.tab, Some(b));
    // A listing for the hidden tab still lands in its own state.
    e.event(Event::Listing { tab: a, path: "/var/log".into(), entries: listing() });
    let lists = |e: &Env| e.engine.sent().iter().filter(|c| matches!(c, Command::List { .. })).count();
    let before = lists(&e);
    e.app().apply(Action::SelectTab(Some(a)));
    e.settle();
    let st = &e.h.state().sftp;
    assert_eq!((st.tab, st.path.as_str(), st.hide_hidden), (Some(a), "/var/log", true));
    assert_eq!(lists(&e), before, "switching back does not relist");
    // Disconnect: state dropped, Sessions shown; a reconnect starts over in the SFTP page.
    let gone = ConnState::Disconnected { reason: "eof".into(), reconnect_at: None };
    e.event(Event::State { tab: a, state: gone });
    assert_eq!((page(&e), e.h.state().sftp.tab), (SidebarTab::Sessions, None));
    e.event(Event::State { tab: a, state: ConnState::Connected });
    let st = &e.h.state().sftp;
    assert_eq!((page(&e), st.tab, st.path.as_str(), st.hide_hidden), (SidebarTab::Sftp, Some(a), "", false));
    assert_eq!(lists(&e), before + 1);
    // Closing a tab drops its parked state.
    assert!(e.h.state().parked_sftp.contains_key(&b));
    e.app().settings.confirm_close = false;
    e.app().apply(Action::SelectTab(Some(b)));
    e.settle();
    e.app().apply(Action::Run(CommandId::CLOSE_TAB));
    e.settle();
    assert!(!e.h.state().parked_sftp.contains_key(&b));
}

#[test]
fn follow_checkbox_is_per_tab_and_syncs_when_turned_on() {
    let mut e = env();
    let mut s = ssh("a");
    s.ssh.follow_terminal_folder = false;
    let a = e.connect(s);
    e.app().sidebar_tab = crate::app::SidebarTab::Sftp;
    e.event(Event::Listing { tab: a, path: "/home/alice".into(), entries: Vec::new() });
    assert!(!e.h.state().sftp.follow, "starts from the session option");
    e.event(Event::CwdChanged { tab: a, path: "/etc".into() });
    assert!(!e.engine.sent().contains(&Command::List { tab: a, path: "/etc".into() }));
    e.h.get_by_label("Follow terminal folder").click();
    e.settle();
    assert!(e.h.state().sftp.follow);
    assert!(e.engine.sent().contains(&Command::SetFollowCwd { tab: a, enabled: true }));
    // Remote monitoring is below the checkbox (off until the engine reports it).
    let monitor = e.h.get_by_label("Remote monitoring");
    assert!(!monitor.accesskit_node().is_disabled());
    assert!(monitor.rect().top() > e.h.get_by_label("Follow terminal folder").rect().top());
    assert!(e.engine.sent().contains(&Command::List { tab: a, path: "/etc".into() }));
    // The session itself is unchanged; a second tab starts from its own option.
    assert!(!e.h.state().tabs[0].session.ssh.follow_terminal_folder);
    let b = e.connect(ssh("b"));
    assert_eq!((e.h.state().sftp.tab, e.h.state().sftp.follow), (Some(b), true));
    assert!(e.h.state().parked_sftp[&a].follow);
}

#[test]
fn sftp_operations_send_commands() {
    let mut e = env();
    let tab = e.connect(ssh("a"));
    e.event(Event::Listing { tab, path: "/home/alice".into(), entries: listing() });
    e.app().apply(Action::Sftp(crate::sftp_panel::SftpAction::UploadPaths(vec!["/tmp/x.txt".into()])));
    let up = Command::Transfer {
        tab,
        direction: Direction::Upload,
        remote: "/home/alice/x.txt".into(),
        local: "/tmp/x.txt".into(),
    };
    assert!(e.engine.sent().contains(&up));
    e.app().modal = Some(Modal::Mkdir { local: false, dir: "/home/alice".into(), name: "new".into() });
    e.settle();
    e.h.get_by_label("Create").click();
    e.settle();
    assert!(e.engine.sent().contains(&Command::Mkdir { tab, path: "/home/alice/new".into() }));
    // New file: an empty local file is uploaded under the typed name.
    e.app().modal = Some(Modal::NewFile { local: false, dir: "/home/alice".into(), name: "notes.txt".into() });
    e.settle();
    e.h.get_by_label("Create").click();
    e.settle();
    let created = e.engine.sent().into_iter().find_map(|c| match c {
        Command::Transfer { direction: Direction::Upload, remote, local, .. } if remote == "/home/alice/notes.txt" => {
            Some(local)
        }
        _ => None,
    });
    let local = created.expect("empty upload sent");
    assert_eq!(std::fs::metadata(&local).map(|m| m.len()).ok(), Some(0));
    e.app().modal = Some(Modal::Chmod { path: "/home/alice/run.sh".into(), mode: "9z".into(), error: None });
    e.settle();
    e.h.get_by_label("Apply").click();
    e.settle();
    assert!(matches!(&e.h.state().modal, Some(Modal::Chmod { error: Some(_), .. })));
}

#[test]
fn prompts_answer_through_engine() {
    let mut e = env();
    let kind = PromptKind::HostKeyUnknown {
        host: "h".into(),
        port: 22,
        algorithm: "ssh-ed25519".into(),
        fingerprint: "SHA256:x".into(),
    };
    prompt(&mut e, 7, kind);
    e.h.get_by_label("Accept and save").click();
    e.settle();
    let ok = Command::Answer { prompt: PromptId(7), answer: PromptAnswer::HostKey(HostKeyDecision::AcceptAndSave) };
    assert!(e.engine.sent().contains(&ok));
    prompt(&mut e, 8, PromptKind::Password { user: "alice".into(), host: "h".into(), retry: false });
    e.h.get_by_label("Password for alice@h").click();
    e.settle();
    e.h.get_by_label("Password for alice@h").type_text("hunter2");
    e.settle();
    e.h.get_by_label("OK").click();
    e.settle();
    let ok = Command::Answer { prompt: PromptId(8), answer: PromptAnswer::Secret(Secret("hunter2".into())) };
    assert!(e.engine.sent().contains(&ok));
    assert!(e.h.state().prompts.is_empty());
}

#[test]
fn closed_tab_drops_its_prompts() {
    let mut e = env();
    let tab = e.connect(ssh("a"));
    e.event(Event::Prompt { id: PromptId(1), tab: Some(tab), kind: PromptKind::KeyPassphrase { path: "k".into() } });
    e.event(Event::Closed { tab });
    assert!(e.h.state().prompts.is_empty());
    assert!(e.h.state().tabs.is_empty());
}

#[test]
fn transfer_started_twice_is_one_row() {
    let mut e = env();
    let tab = e.connect(ssh("a"));
    let started = |total| Event::TransferStarted {
        id: TransferId(1),
        tab,
        direction: Direction::Download,
        remote: "/r".into(),
        local: "/l".into(),
        total,
    };
    e.event(started(None));
    e.event(started(Some(10)));
    e.event(Event::TransferProgress { id: TransferId(1), done: 5 });
    assert_eq!(e.h.state().transfers.len(), 1);
    assert_eq!((e.h.state().transfers[0].total, e.h.state().transfers[0].done), (Some(10), 5));
    e.event(Event::TransferFinished { id: TransferId(1), result: Ok(10) });
    assert!(matches!(e.h.state().transfers[0].result, Some(Ok(10))));
}

#[test]
fn multiexec_toggle_and_selection() {
    let mut e = env();
    let tab = e.connect(ssh("a"));
    e.h.get_by_label("MultiExec").click();
    e.settle();
    assert!(e.engine.sent().contains(&Command::SetMultiExec { enabled: true }));
    e.h.get_by_label("MultiExec alice@a").click();
    e.settle();
    assert!(e.engine.sent().contains(&Command::SelectMultiExec { tab, selected: true }));
    e.app().run(CommandId::MULTIEXEC);
    assert!(e.engine.sent().contains(&Command::SetMultiExec { enabled: false }));
}

#[test]
fn session_crud_and_drag_target_actions() {
    let mut e = env();
    e.app().apply(Action::NewSession(None));
    let ed = e.app().editor.as_mut().unwrap();
    ed.set_type(Protocol::Ssh);
    ed.session.host = "web1".into();
    e.settle();
    e.h.get_by_label("OK").click();
    e.settle();
    assert!(e.h.state().editor.is_none());
    assert_eq!(e.h.state().store.sessions.len(), 1);
    assert!(matches!(e.engine.sent().first(), Some(Command::Open { .. })));
    let id = e.h.state().store.sessions[0].id;
    e.app().apply(Action::SaveFolder { id: None, parent: None, name: "Lab".into(), icon: None });
    let folder: FolderId = e.h.state().store.folders[0].id;
    e.app().apply(Action::MoveSession(id, Some(folder)));
    assert_eq!(e.h.state().store.sessions[0].folder, Some(folder));
    e.app().apply(Action::DeleteSession(id));
    assert!(e.h.state().store.sessions.is_empty());
}

#[test]
fn tunnel_start_and_autostart() {
    let via = SessionId(1);
    let mut storage = MemStorage::default();
    {
        let mut e = env();
        e.app().store.upsert(ssh("bastion"));
        e.app().store.tunnels.push(Tunnel {
            name: "db".into(),
            via,
            bind_port: 1,
            dest_host: "d".into(),
            dest_port: 2,
            autostart: true,
            ..Tunnel::default()
        });
        eframe::App::save(e.app(), &mut storage);
    }
    let mut e = env_with(None, Some(&storage));
    e.settle();
    assert!(e.engine.sent().iter().any(|c| matches!(c, Command::StartTunnel { tunnel, .. } if tunnel.name == "db")));
    e.event(Event::TunnelState { name: "db".into(), state: TunnelState::Running });
    assert_eq!(e.h.state().tunnel_states.get("db"), Some(&TunnelState::Running));
}

#[derive(Default)]
struct MemStorage(HashMap<String, String>);

impl eframe::Storage for MemStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.into(), value);
    }
    fn remove_string(&mut self, key: &str) {
        self.0.remove(key);
    }
    fn flush(&mut self) {}
}

#[test]
fn layout_and_sessions_persist() {
    let mut storage = MemStorage::default();
    {
        let mut e = env();
        e.app().settings.sidebar_width = 333.0;
        e.app().settings.sidebar_open = false;
        e.app().store.folders.push(Folder { id: FolderId(9), name: "Lab".into(), parent: None, icon: None });
        eframe::App::save(e.app(), &mut storage);
    }
    let e = env_with(None, Some(&storage));
    assert_eq!(e.h.state().settings.sidebar_width, 333.0);
    assert!(!e.h.state().settings.sidebar_open);
    assert_eq!(e.h.state().store.folders.len(), 1);
}

/// Two tabs with endless output; records each drain's `(tab, budget)`.
#[derive(Clone, Default)]
struct FloodEngine(Rc<RefCell<Vec<(TabId, usize)>>>);

impl Backend for FloodEngine {
    fn send(&self, _: Command) -> rxt_core::Result<()> {
        Ok(())
    }
    fn try_event(&self) -> Option<Event> {
        None
    }
    fn drain_output(&self, tab: TabId, budget: usize, _: &mut Vec<u8>) -> usize {
        self.0.borrow_mut().push((tab, budget));
        budget
    }
    fn pending_output(&self) -> Vec<TabId> {
        vec![TabId(1), TabId(2)]
    }
}

#[test]
fn flooding_visible_tab_does_not_starve_background_tab() {
    let engine = FloodEngine::default();
    let platform = Arc::new(Fake::default());
    let mut app = App::new(None, platform.platform(), engine.clone());
    app.active = Some(TabId(1));
    let mut h = Harness::builder().build_ui_state(|ui, app: &mut App| app.show(ui), app);
    h.step();
    let drains = engine.0.borrow().clone();
    let got = |tab| drains.iter().filter(|(t, _)| *t == TabId(tab)).map(|(_, n)| n).sum::<usize>();
    assert!(got(2) > 0, "background tab starved: {drains:?}");
    assert!(got(1) >= got(2), "visible tab must keep the larger share: {drains:?}");
}

// ---- M2: file tabs and remote desktops ----

fn local_listing() -> Vec<FileEntry> {
    let f = |name: &str, kind, size, mode| FileEntry {
        name: name.into(),
        kind,
        size,
        modified: Some(1_690_000_000),
        mode: Some(mode),
        owner: Some("1000".into()),
        group: Some("1000".into()),
    };
    vec![
        f("Documents", FileKind::Dir, 4096, 0o755),
        f("Downloads", FileKind::Dir, 4096, 0o755),
        f("notes.txt", FileKind::File, 2048, 0o644),
        f("photo.jpg", FileKind::File, 3_145_728, 0o600),
    ]
}

/// Connected file tab with deterministic local and remote listings.
fn file_tab(e: &mut Env, protocol: Protocol) -> TabId {
    let tab = e.connect(Session::new(protocol, "files.lab.example", Some("alice".into()), None));
    e.event(Event::Listing { tab, path: "/home/alice".into(), entries: listing() });
    let t = e.app().tabs.iter_mut().find(|t| t.id == tab).unwrap();
    let local = &mut t.files.as_mut().unwrap().local;
    local.path = "/home/user".into();
    local.path_edit = "/home/user".into();
    local.entries = local_listing();
    local.error = None;
    e.settle();
    tab
}

#[test]
fn snapshot_file_tabs() {
    let mut e = env();
    let tab = file_tab(&mut e, Protocol::Sftp);
    assert!(e.engine.sent().contains(&Command::List { tab, path: "~".into() }));
    assert!(e.h.query_by_label("Unencrypted").is_none());
    e.snap("file_tab_sftp");
    e.app().apply(Action::CloseTab(tab));
    let mut s = Session::new(Protocol::Ftp, "ftp.lab.example", Some("alice".into()), None);
    s.ftp.startup_folder = Some("/pub".into());
    let ftp = e.connect(s);
    assert!(e.engine.sent().contains(&Command::List { tab: ftp, path: "/pub".into() }));
    e.app().apply(Action::CloseTab(ftp));
    file_tab(&mut e, Protocol::Ftp);
    assert!(e.h.query_by_label("Unencrypted").is_some());
    e.snap("file_tab_ftp");
}

#[test]
fn file_tab_drag_between_panes_transfers() {
    let mut e = env();
    let tab = file_tab(&mut e, Protocol::Sftp);
    let from = e.h.get_by_label("report.pdf").rect().center();
    let to = e.h.get_by_label("notes.txt").rect().center();
    e.h.drag_at(from);
    e.h.run_steps(2);
    e.h.hover_at(from + vec2(20.0, 4.0));
    e.h.run_steps(2);
    e.h.hover_at(to);
    e.h.run_steps(2);
    e.h.drop_at(to);
    e.settle();
    let down = Command::Transfer {
        tab,
        direction: Direction::Download,
        remote: "/home/alice/report.pdf".into(),
        local: "/home/user/report.pdf".into(),
    };
    assert!(e.engine.sent().contains(&down), "{:?}", e.engine.sent());
    // Local to remote uploads into the remote directory.
    let (from, to) = (e.h.get_by_label("photo.jpg").rect().center(), e.h.get_by_label("run.sh").rect().center());
    e.h.drag_at(from);
    e.h.run_steps(2);
    e.h.hover_at(from + vec2(20.0, 4.0));
    e.h.run_steps(2);
    e.h.hover_at(to);
    e.h.run_steps(2);
    e.h.drop_at(to);
    e.settle();
    let up = Command::Transfer {
        tab,
        direction: Direction::Upload,
        remote: "/home/alice/photo.jpg".into(),
        local: "/home/user/photo.jpg".into(),
    };
    assert!(e.engine.sent().contains(&up), "{:?}", e.engine.sent());
}

/// 320x200 test pattern: color bands plus a white square.
fn fake_framebuffer() -> Framebuffer {
    let mut fb = Framebuffer::default();
    fb.apply(&DesktopEvent::Resized { width: 320, height: 200 }).unwrap();
    for y in 0..200u32 {
        for x in 0..320u32 {
            let at = ((y * 320 + x) * 4) as usize;
            let band =
                [[0x1e, 0x3a, 0x8a], [0x0f, 0x76, 0x6e], [0xb4, 0x53, 0x09], [0x6d, 0x28, 0xd9]][(x / 80) as usize];
            let shade = (y * 255 / 200) as u8 / 3;
            fb.pixels[at..at + 3].copy_from_slice(&band.map(|c: u8| c.saturating_add(shade)));
        }
    }
    let rect = Rect { x: 120, y: 60, width: 80, height: 80 };
    fb.apply(&DesktopEvent::Update { rect, rgba: vec![0xff; 80 * 80 * 4] }).unwrap();
    fb
}

fn desktop_tab(e: &mut Env, s: Session) -> TabId {
    let tab = e.connect(s);
    e.engine.framebuffers.borrow_mut().insert(tab, fake_framebuffer());
    e.settle();
    tab
}

fn desktop_inputs(e: &Env) -> Vec<DesktopInput> {
    e.engine
        .sent()
        .into_iter()
        .filter_map(|c| if let Command::DesktopInput { input, .. } = c { Some(input) } else { None })
        .collect()
}

#[test]
fn snapshot_desktop_tabs() {
    let mut e = env();
    let rdp = desktop_tab(&mut e, Session::new(Protocol::Rdp, "win.lab.example", Some("bob".into()), None));
    e.snap("desktop_rdp");
    e.app().apply(Action::CloseTab(rdp));
    let mut vnc = Session::new(Protocol::Vnc, "kiosk.lab.example", None, None);
    vnc.vnc.scale_to_fit = false;
    vnc.vnc.view_only = true;
    desktop_tab(&mut e, vnc);
    e.snap("desktop_vnc_view_only");
    let failed = e.open(Session::new(Protocol::Rdp, "win2.lab.example", Some("bob".into()), None));
    e.event(Event::State { tab: failed, state: ConnState::Error("connection refused".into()) });
    e.snap("desktop_error");
}

#[test]
fn keyboard_grab_and_release_combination() {
    let mut e = env();
    let tab = desktop_tab(&mut e, Session::new(Protocol::Vnc, "kiosk", None, None));
    e.h.get_by_label("Remote desktop").click();
    e.settle();
    assert!(e.h.state().tabs[0].desktop.as_ref().unwrap().grabbed);
    // An app shortcut (New session) goes to the remote side instead.
    e.h.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::N);
    e.settle();
    let inputs = desktop_inputs(&e);
    for want in [
        DesktopInput::Key { keysym: 0xffe3, down: true },
        DesktopInput::Key { keysym: 0x4e, down: true },
        DesktopInput::Key { keysym: 0x4e, down: false },
        DesktopInput::Key { keysym: 0xffe3, down: false },
    ] {
        assert!(inputs.contains(&want), "{want:?} not in {inputs:?}");
    }
    assert!(e.h.state().editor.is_none());
    // Typed text maps through Unicode keysyms.
    e.h.event(egui::Event::Text("ж".into()));
    e.settle();
    assert!(desktop_inputs(&e).contains(&DesktopInput::Key { keysym: 0x0100_0436, down: true }));
    // Release combination (default Ctrl+Alt).
    e.h.event(egui::Event::ModifiersChanged(Modifiers::CTRL | Modifiers::ALT));
    e.h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    e.settle();
    assert!(desktop_inputs(&e).contains(&DesktopInput::ReleaseAll));
    assert!(!e.h.state().tabs[0].desktop.as_ref().unwrap().grabbed);
    let before = desktop_inputs(&e).len();
    e.h.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::N);
    e.settle();
    assert!(e.h.state().editor.is_some(), "app shortcut works again after release");
    let keys_after = desktop_inputs(&e)[before..].iter().filter(|i| matches!(i, DesktopInput::Key { .. })).count();
    assert_eq!(keys_after, 0, "keys are not forwarded after release");
    let _ = tab;
}

#[test]
fn view_only_sends_no_input() {
    let mut e = env();
    let mut s = Session::new(Protocol::Vnc, "kiosk", None, None);
    s.vnc.view_only = true;
    desktop_tab(&mut e, s);
    let r = e.h.get_by_label("Remote desktop").rect();
    e.h.hover_at(r.center());
    e.h.drag_at(r.center());
    e.h.drop_at(r.center());
    e.h.key_press(Key::A);
    e.h.event(egui::Event::Text("a".into()));
    e.settle();
    assert!(desktop_inputs(&e).is_empty(), "{:?}", desktop_inputs(&e));
}

#[test]
fn pointer_and_clipboard_reach_remote() {
    let mut e = env();
    let tab = desktop_tab(&mut e, Session::new(Protocol::Vnc, "kiosk", None, None));
    *e.platform.clipboard.lock().unwrap() = "local text".into();
    let r = e.h.get_by_label("Remote desktop").rect();
    e.h.get_by_label("Remote desktop").click();
    e.settle();
    let inputs = desktop_inputs(&e);
    assert!(inputs.contains(&DesktopInput::Clipboard("local text".into())), "{inputs:?}");
    assert!(inputs.iter().any(|i| matches!(i, DesktopInput::Pointer { buttons: 1, .. })), "{inputs:?}");
    assert!(inputs.iter().any(|i| matches!(i, DesktopInput::Pointer { buttons: 0, .. })));
    assert!(r.width() > 0.0);
    e.engine.clipboard.borrow_mut().insert(tab, "remote text".into());
    e.settle();
    assert_eq!(*e.platform.clipboard.lock().unwrap(), "remote text");
}

#[test]
fn hidden_desktop_tabs_stop_updates() {
    let mut e = env();
    let vnc = desktop_tab(&mut e, Session::new(Protocol::Vnc, "kiosk", None, None));
    assert!(e.engine.sent().contains(&Command::SetVisible { tab: vnc, visible: true }));
    e.connect(ssh("web1"));
    assert!(e.engine.sent().contains(&Command::SetVisible { tab: vnc, visible: false }));
    e.app().apply(Action::SelectTab(Some(vnc)));
    e.settle();
    let shown = e.engine.sent().iter().filter(|c| **c == Command::SetVisible { tab: vnc, visible: true }).count();
    assert_eq!(shown, 2);
}

#[test]
fn snapshot_m2_editors_and_certificate() {
    let mut e = env();
    let mut ftps = Session::new(Protocol::Ftps, "files.lab.example", Some("alice".into()), None);
    ftps.ftp.startup_folder = Some("/pub".into());
    let mut rdp = Session::new(Protocol::Rdp, "win.lab.example", Some("bob".into()), None);
    rdp.rdp.domain = Some("LAB".into());
    rdp.rdp.resolution = rxt_core::RdpResolution::Fixed { width: 1600, height: 900 };
    rdp.rdp.keyboard_layout = 0x0416;
    rdp.rdp.allow_legacy_tls = true;
    let mut vnc = Session::new(Protocol::Vnc, "kiosk.lab.example", None, None);
    vnc.vnc.via_ssh =
        Some(JumpHost { host: "bastion.lab.example".into(), port: 22, user: "alice".into(), key_file: None });
    for (s, name) in [(ftps, "editor_ftps_options"), (rdp, "editor_rdp_options"), (vnc, "editor_vnc_ssh")] {
        e.app().editor = Some(Editor::new(s, true));
        e.snap(name);
    }
    let mut ed = Editor::new(ssh("web1.lab.example"), true);
    ed.session.icon = Some("monitor-debian".into());
    ed.icon_picker = true;
    e.app().editor = Some(ed);
    e.snap("editor_icon_picker");
    e.app().editor = None;
    prompt(
        &mut e,
        1,
        PromptKind::CertificateUnknown {
            host: "files.lab.example".into(),
            port: 990,
            subject: "CN=files.lab.example".into(),
            fingerprint: "SHA256:4f:9a:0c:7e:21:b3:58:d6:aa:10:3f:c2:9e:77:05:b1".into(),
        },
    );
    e.snap("dialog_certificate_unknown");
}

// ---- encrypted store ----

/// Fresh, unique data directory for one test.
fn data_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("rxt-ui-vault-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

const MASTER: &str = "lab master password";

/// App with a session database in `dir`; `setup` prepares it before the app starts.
fn env_vault(dir: &std::path::Path, setup: impl FnOnce(&Vault)) -> Env {
    let platform = Arc::new(Fake { root: std::env::temp_dir().join("rxt-ui-tests"), ..Fake::default() });
    setup(&Vault::fast(dir.to_owned(), platform.platform().keyring));
    let vault = Arc::new(Vault::fast(dir.to_owned(), platform.platform().keyring));
    let engine = TestEngine::default();
    let app = App::new(None, platform.platform(), engine.clone()).with_vault(vault);
    let mut e = Env { h: harness(app), engine, platform };
    e.wait_vault();
    e
}

/// App whose database exists but is locked (no keyring wrap).
fn env_locked(name: &str) -> Env {
    env_vault(&data_dir(name), |v| v.create(MASTER, false).unwrap())
}

/// App with an unlocked database.
fn env_unlocked(name: &str) -> Env {
    let mut e = env_locked(name);
    e.app().store_dialog = None;
    e.app().apply(Action::Unlock(Secret(MASTER.into())));
    e.wait_vault();
    assert!(!e.app().locked());
    e
}

impl Env {
    /// Let a vault worker (Argon2, keyring) finish and its result arrive.
    fn wait_vault(&mut self) {
        for _ in 0..1000 {
            self.settle();
            if !self.h.state().vault_busy {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("vault worker did not finish");
    }
}

#[test]
fn snapshot_first_run_master_password() {
    let mut e = env_vault(&data_dir("first-run"), |_| {});
    assert!(matches!(e.h.state().store_dialog, Some(StoreDialog::Create { .. })));
    assert!(e.h.query_by_label_contains("cannot be recovered").is_some());
    e.app().store_dialog = Some(StoreDialog::Create {
        password: "correct horse".into(),
        confirm: "correct horse".into(),
        keyring: true,
        error: None,
    });
    e.snap("store_first_run");
}

#[test]
fn first_run_creates_database_and_sessions_persist() {
    let dir = data_dir("persist");
    {
        let mut e = env_vault(&dir, |_| {});
        e.app().apply(Action::CreateVault { master: Secret(MASTER.into()), keyring: false });
        e.wait_vault();
        assert!(e.h.state().store_dialog.is_none());
        let id = e.app().store.upsert(ssh("web1.lab.example"));
        let f = e.app().store.save_folder(None, None, "Lab".into());
        e.app().apply(Action::MoveSession(id, Some(f)));
        e.settle();
    }
    // Next start: locked, keyring not enabled, so the unlock dialog asks.
    let mut e = env_vault(&dir, |_| {});
    assert!(e.app().locked());
    assert!(matches!(e.h.state().store_dialog, Some(StoreDialog::Unlock { .. })));
    e.app().apply(Action::Unlock(Secret(MASTER.into())));
    e.wait_vault();
    let st = &e.h.state().store;
    assert_eq!(st.sessions.len(), 1);
    assert_eq!(st.folders[0].name, "Lab");
    assert_eq!(st.sessions[0].folder, Some(st.folders[0].id));
}

#[test]
fn keyring_unlocks_silently() {
    let dir = data_dir("keyring");
    let platform = Arc::new(Fake::default());
    let keyring = platform.platform().keyring;
    Vault::fast(dir.clone(), keyring.clone()).create(MASTER, true).unwrap();
    let vault = Arc::new(Vault::fast(dir, keyring));
    let app = App::new(None, platform.platform(), TestEngine::default()).with_vault(vault);
    let mut h = Harness::builder().with_size(vec2(1180.0, 720.0)).build_ui_state(|ui, app: &mut App| app.show(ui), app);
    for _ in 0..500 {
        h.run_steps(5);
        if !h.state().locked() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!h.state().locked());
    assert!(h.state().store_dialog.is_none());
    assert!(h.state().keyring_on);
}

#[test]
fn snapshot_unlock_dialog_and_wrong_password() {
    let mut e = env_locked("unlock");
    e.app().store_dialog = Some(StoreDialog::unlock());
    e.snap("store_unlock");
    e.app().store_dialog = Some(StoreDialog::Unlock { password: "wrong password".into(), error: None });
    e.app().apply(Action::Unlock(Secret("wrong password".into())));
    e.wait_vault();
    assert!(e.app().locked());
    assert!(e.h.query_by_label("Wrong master password.").is_some());
    e.snap("store_unlock_error");
}

#[test]
fn snapshot_locked_tree_and_quick_connect_still_works() {
    let mut e = env_locked("locked");
    e.app().store_dialog = None;
    e.snap("sessions_locked");
    // New session needs the database: the unlock dialog opens instead of the editor.
    e.app().run(CommandId::NEW_SESSION);
    assert!(e.h.state().editor.is_none());
    assert!(matches!(e.h.state().store_dialog, Some(StoreDialog::Unlock { .. })));
    e.app().store_dialog = None;
    e.app().quick = "alice@web1.lab.example".into();
    e.app().run(CommandId::QUICK_CONNECT);
    e.settle();
    assert_eq!(e.h.state().tabs.len(), 1);
    assert!(
        e.engine.sent().iter().any(|c| matches!(c, Command::Open { session } if session.host == "web1.lab.example"))
    );
    e.h.get_by_label("Unlock…").click();
    e.settle();
    assert!(matches!(e.h.state().store_dialog, Some(StoreDialog::Unlock { .. })));
}

#[test]
fn snapshot_change_password_and_keyring_toggle() {
    let dir = data_dir("change");
    let mut e = env_vault(&dir, |v| v.create(MASTER, false).unwrap());
    e.app().apply(Action::Unlock(Secret(MASTER.into())));
    e.wait_vault();
    e.app().run(CommandId::SETTINGS);
    settings_tab(&mut e, "Security");
    e.snap("settings_security");
    e.h.get_by_label("Unlock with system keyring").click();
    e.wait_vault();
    assert!(e.h.state().keyring_on);
    e.app().show_settings = false;
    e.app().apply(Action::ShowChangePassword);
    e.app().store_dialog = Some(StoreDialog::ChangePassword {
        password: "new master pass".into(),
        confirm: "new master typo".into(),
        error: None,
    });
    e.snap("store_change_password");
    e.app().apply(Action::ChangePassword(Secret("new master pass".into())));
    e.wait_vault();
    assert!(e.h.state().store_dialog.is_none());
    let v = Vault::fast(dir, e.platform.platform().keyring);
    assert!(v.unlock(MASTER).is_err());
    v.unlock("new master pass").unwrap();
}

fn import_report() -> ImportReport {
    let mut web = ssh("web1.lab.example");
    web.name = "web1".into();
    let db = Session { name: "db".into(), ..ssh("db.lab.example") };
    let files = Session { name: "files".into(), ..Session::new(Protocol::Sftp, "files.lab.example", None, None) };
    ImportReport {
        sessions: vec![("Lab".into(), web), ("Lab/Databases".into(), db), (String::new(), files)],
        folders: vec!["Lab".into(), "Lab/Databases".into()],
        skipped: vec![Skipped { line: 7, name: "router".into(), reason: "unsupported session type rsh (2)".into() }],
        warnings: Vec::new(),
    }
}

#[test]
fn snapshot_import_preview_and_confirm() {
    let mut e = env_unlocked("import");
    e.app().store_dialog =
        Some(StoreDialog::Import { path: "/home/alice/MobaXterm.ini".into(), report: import_report() });
    e.snap("store_import_preview");
    // The Home tab has an "Import" button too; the dialog's is drawn last.
    e.h.get_all_by_label("Import").last().unwrap().click();
    e.settle();
    let st = &e.h.state().store;
    assert_eq!(st.sessions.len(), 3);
    let db = st.sessions.iter().find(|s| s.name == "db").unwrap();
    let folder = st.folders.iter().find(|f| Some(f.id) == db.folder).unwrap();
    assert_eq!(folder.name, "Databases");
    // Written to the database.
    let saved = e.h.state().vault.as_ref().unwrap().with(|db| db.sessions()).unwrap();
    assert_eq!(saved.len(), 3);
}

#[test]
fn snapshot_export_confirmation_writes_toml() {
    let mut e = env_unlocked("export");
    sample_store(e.app());
    e.settle();
    let path = data_dir("export-file").with_extension("toml");
    let st = &e.h.state().store;
    let (sessions, folders, tunnels) = (st.sessions.len(), st.folders.len(), st.tunnels.len());
    e.app().store_dialog =
        Some(StoreDialog::Export { path: "/home/alice/rustxterm-sessions.toml".into(), sessions, folders, tunnels });
    e.snap("store_export");
    e.app().store_dialog = Some(StoreDialog::Export { path: path.clone(), sessions, folders, tunnels });
    e.settle();
    e.h.get_by_label("Export").click();
    e.settle();
    let back = rxt_store::export::Export::from_toml(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(back.sessions.len(), sessions);
    let _ = std::fs::remove_file(path);
}

#[test]
fn save_password_checkbox_requests_saving_before_answer() {
    let mut e = env_unlocked("save-password");
    let id = e.app().store.upsert(ssh("web1.lab.example"));
    let s = e.h.state().store.session(id).cloned().unwrap();
    let tab = e.open(s);
    e.event(Event::Prompt {
        id: PromptId(41),
        tab: Some(tab),
        kind: PromptKind::Password { user: "alice".into(), host: "web1.lab.example".into(), retry: false },
    });
    e.h.get_by_label("Save password").click();
    e.settle();
    e.h.get_by_label("Password for alice@web1.lab.example").click();
    e.settle();
    e.h.get_by_label("Password for alice@web1.lab.example").type_text("pw");
    e.settle();
    e.h.get_by_label("OK").click();
    e.settle();
    assert_eq!(*e.engine.saves.borrow(), [PromptId(41)]);
    assert!(
        e.engine
            .sent()
            .contains(&Command::Answer { prompt: PromptId(41), answer: PromptAnswer::Secret(Secret("pw".into())) })
    );
    // Unsaved quick-connect sessions offer no checkbox.
    let tab = e.open(ssh("adhoc.lab.example"));
    e.event(Event::Prompt {
        id: PromptId(42),
        tab: Some(tab),
        kind: PromptKind::Password { user: "alice".into(), host: "adhoc.lab.example".into(), retry: false },
    });
    assert!(e.h.query_by_label("Save password").is_none());
}

#[test]
fn eframe_sessions_migrate_once_on_first_unlock() {
    let mut storage = MemStorage::default();
    {
        let mut e = env();
        e.app().store.upsert(ssh("legacy.lab.example"));
        eframe::App::save(e.app(), &mut storage);
    }
    let dir = data_dir("migrate");
    let platform = Arc::new(Fake::default());
    let vault = Arc::new(Vault::fast(dir.clone(), platform.platform().keyring));
    vault.create(MASTER, false).unwrap();
    let mut app = App::new(Some(&storage), platform.platform(), TestEngine::default()).with_vault(vault);
    assert!(app.store.sessions.is_empty(), "locked: nothing shown");
    // Saving while still locked keeps the old copy for a later migration.
    eframe::App::save(&mut app, &mut storage);
    assert!(storage.0.contains_key("rxt.sessions"));
    let mut h = Harness::builder().with_size(vec2(1180.0, 720.0)).build_ui_state(|ui, app: &mut App| app.show(ui), app);
    h.state_mut().apply(Action::Unlock(Secret(MASTER.into())));
    // Until the sessions arrive (up to 30 s): the startup keyring attempt also reports back and
    // clears `vault_busy`, so on a loaded machine "not busy" can come before the unlock result.
    for _ in 0..3000 {
        h.run_steps(5);
        if !h.state().store.sessions.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(h.state().store.sessions.len(), 1);
    eframe::App::save(h.state_mut(), &mut storage);
    assert!(!storage.0.contains_key("rxt.sessions"), "plaintext copy removed after migration");
    let saved = Vault::fast(dir, platform.platform().keyring);
    saved.unlock(MASTER).unwrap();
    assert_eq!(saved.with(|db| db.sessions()).unwrap().len(), 1);
}

#[test]
fn settings_precedence_sets_theme_and_notice_names_file() {
    let dir = data_dir("settings");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, "theme = \"light\"\n").unwrap();
    let env = [("RXT_THEME".to_owned(), "dark".to_owned())];
    let loaded = rxt_store::settings::load(&path, None, &env, &Default::default());
    let platform = Arc::new(Fake::default());
    platform.set_dark(Some(false));
    let app = App::new(None, platform.platform(), TestEngine::default()).with_settings(loaded, path.clone());
    let mut h = Harness::builder().with_size(vec2(1180.0, 720.0)).build_ui_state(|ui, app: &mut App| app.show(ui), app);
    h.run_steps(3);
    assert!(h.state().is_dark(), "RXT_THEME overrides config.toml");

    std::fs::write(&path, "theme = [").unwrap();
    let loaded = rxt_store::settings::load(&path, None, &[], &Default::default());
    let app = App::new(None, platform.platform(), TestEngine::default()).with_settings(loaded, path.clone());
    let mut h = Harness::builder().with_size(vec2(1180.0, 720.0)).build_ui_state(|ui, app: &mut App| app.show(ui), app);
    h.run_steps(3);
    assert!(h.query_all_by_label_contains(&path.display().to_string()).next().is_some(), "notice names the file");
    // Changed settings are written back to config.toml.
    h.state_mut().settings.theme = ThemeMode::Dark;
    eframe::App::save(h.state_mut(), &mut MemStorage::default());
    let again = rxt_store::settings::load(&path, None, &[], &Default::default());
    assert_eq!(again.settings.theme, ThemeMode::Dark);
}

#[test]
fn invalid_port_shows_error_and_disables_save() {
    let mut e = env();
    let mut ed = Editor::new(ssh("web1.lab.example"), true);
    ed.port = "70000".into();
    e.app().editor = Some(ed);
    e.settle();
    assert!(e.h.query_by_label("Port must be between 1 and 65535.").is_some());
    e.h.get_by_label("OK").click();
    e.settle();
    assert!(e.h.state().editor.is_some(), "editor stays open");
    assert!(e.h.state().store.sessions.is_empty(), "nothing saved");
    e.app().editor.as_mut().unwrap().port = "2222".into();
    e.settle();
    e.h.get_by_label("OK").click();
    e.settle();
    assert_eq!(e.h.state().store.sessions[0].port, 2222);
}

// ---- agent control ----

const NOW: i64 = 1_700_000_000;

fn fixed_now() -> i64 {
    NOW
}

fn approval(id: u64, expires_at: i64) -> Event {
    Event::ApprovalRequest {
        id: rxt_core::ApprovalId(id),
        client: "claude-code".into(),
        command: "send".into(),
        session: "web1".into(),
        target: "tab 1: \"uptime\\n\"".into(),
        expires_at,
    }
}

fn answers(e: &Env) -> Vec<(u64, bool)> {
    e.engine
        .sent()
        .iter()
        .filter_map(|c| match c {
            Command::AnswerApproval { id, approve } => Some((id.0, *approve)),
            _ => None,
        })
        .collect()
}

#[test]
fn snapshot_approval_prompt_and_answers() {
    let mut e = env();
    e.app().clock = fixed_now;
    e.event(approval(1, NOW + 42));
    e.event(approval(2, NOW + 55));
    assert!(e.h.query_by_label("Denied automatically in 42 s.").is_some());
    assert!(e.h.query_by_label_contains("not authenticated").is_some());
    e.snap("agent_approval");
    e.h.get_by_label("Approve").click();
    e.settle();
    assert_eq!(answers(&e), [(1, true)]);
    e.h.get_by_label("Deny").click();
    e.settle();
    assert_eq!(answers(&e), [(1, true), (2, false)]);
    assert!(e.h.state().approvals.is_empty());
}

#[test]
fn approval_times_out_denied_and_closing_tab_clears() {
    let mut e = env();
    e.app().clock = fixed_now;
    e.event(approval(7, NOW - 1));
    assert_eq!(answers(&e), [(7, false)], "expired approvals are denied without a click");
    let mut s = ssh("web1.lab.example");
    s.name = "web1".into();
    let tab = e.open(s);
    e.event(approval(8, NOW + 60));
    e.app().apply(Action::CloseTab(tab));
    e.settle();
    assert_eq!(answers(&e), [(7, false), (8, false)]);
    assert!(e.h.state().approvals.is_empty());
}

#[test]
fn snapshot_editor_agent_page_and_policy_saved() {
    let mut e = env_unlocked("agent-policy");
    let id = e.app().store.upsert(Session { name: "web1".into(), ..ssh("web1.lab.example") });
    e.settle();
    e.app().apply(Action::EditSession(id));
    let ed = e.app().editor.as_mut().unwrap();
    assert_eq!(ed.policy, Some(rxt_store::db::Policy::default()), "deny and content-read off by default");
    ed.page = Page::Agent;
    e.settle();
    e.h.get_by_label("Ask").click();
    e.settle();
    e.h.get_by_label("Agent may read content").click();
    e.settle();
    e.snap("editor_agent");
    e.h.get_by_label("OK").click();
    e.settle();
    let vault = e.h.state().vault.clone().unwrap();
    let stored = vault.with(|db| db.policy(id)).unwrap();
    assert_eq!(stored, rxt_store::db::Policy { policy: rxt_store::db::AgentPolicy::Ask, content_read: true });
    // Unsaved sessions show the defaults read-only.
    let mut ed = Editor::new(Session::new(Protocol::Rdp, "win.lab.example", None, None), true);
    ed.page = Page::Agent;
    e.app().editor = Some(ed);
    e.settle();
    assert!(e.h.query_by_label("Save the session first to change its agent policy.").is_some());
    e.snap("editor_agent_unsaved");
}

#[test]
fn snapshot_settings_agent_control_applies_live() {
    let mut e = env();
    e.settle();
    assert_eq!(*e.engine.agent_control.borrow(), [false]);
    e.app().run(CommandId::SETTINGS);
    settings_tab(&mut e, "Agent control");
    e.h.get_by_label("Enable agent control (CLI live commands and MCP)").click();
    e.settle();
    assert!(e.h.state().settings.agent_control);
    assert_eq!(*e.engine.agent_control.borrow(), [false, true]);
    e.snap("settings_agent_control");
    let mut storage = MemStorage::default();
    e.app().agent_workspace = Some("/srv/agent".into());
    eframe::App::save(e.app(), &mut storage);
    let back = App::new(Some(&storage), e.platform.platform(), TestEngine::default());
    assert_eq!(back.agent_workspace.as_deref(), Some("/srv/agent"));
    assert!(back.settings.agent_control);
}

fn audit_record(i: i64, decision: &str, result: &str) -> rxt_store::db::AuditRecord {
    rxt_store::db::AuditRecord {
        ts: NOW + i * 60,
        client: "claude-code".into(),
        entry_point: "mcp".into(),
        command: ["send", "files.put", "read"][i as usize % 3].into(),
        session: Some(SessionId(1)),
        target: format!("web1: item {i}"),
        decision: decision.into(),
        result: result.into(),
    }
}

#[test]
fn snapshot_audit_view_pages_and_clear() {
    let mut e = env_unlocked("audit");
    e.app().apply(Action::ShowAudit);
    e.snap("audit_empty");
    let vault = e.h.state().vault.clone().unwrap();
    vault
        .with(|db| {
            for i in 0..3 {
                db.append_audit(&audit_record(i, ["allowed", "approved", "denied"][i as usize], "success"))?;
            }
            db.append_audit(&audit_record(3, "denied", "denied by policy"))
        })
        .unwrap();
    e.h.get_by_label("Refresh").click();
    e.settle();
    assert_eq!(e.h.state().audit.as_ref().unwrap().records[0].target, "web1: item 3", "newest first");
    e.snap("audit_view");
    vault.with(|db| (0..60).try_for_each(|i| db.append_audit(&audit_record(i, "allowed", "success")))).unwrap();
    e.app().apply(Action::AuditPage(0));
    e.settle();
    assert!(e.h.state().audit.as_ref().unwrap().more);
    e.h.get_by_label("Older").click();
    e.settle();
    assert_eq!(e.h.state().audit.as_ref().unwrap().records.len(), 14);
    e.h.get_by_label("Clear log…").click();
    e.settle();
    e.h.get_by_label("Clear").click();
    e.settle();
    assert!(vault.with(|db| db.audit(0, 10)).unwrap().is_empty());
    assert!(e.h.query_by_label("No agent requests recorded.").is_some());
}

#[test]
fn audit_view_locked_offers_unlock() {
    let mut e = env_locked("audit-locked");
    e.app().store_dialog = None;
    e.app().apply(Action::ShowAudit);
    e.settle();
    assert!(e.h.query_by_label("The session database is locked.").is_some());
}

#[test]
fn bundled_fonts_are_used_for_ui_and_terminal_text() {
    let mut e = env();
    e.settle();
    let (prop, mono) = e.h.ctx.fonts(|f| {
        let d = f.definitions();
        (d.families[&egui::FontFamily::Proportional][0].clone(), d.families[&egui::FontFamily::Monospace][0].clone())
    });
    assert_eq!((prop.as_str(), mono.as_str()), ("Noto Sans", "Cascadia Mono"));
    let width =
        |family: egui::FontFamily, c: char| e.h.ctx.fonts_mut(|f| f.glyph_width(&egui::FontId::new(12.0, family), c));
    assert_eq!(width(egui::FontFamily::Proportional, 'W'), width(egui::FontFamily::Name("Noto Sans".into()), 'W'));
    assert_eq!(width(egui::FontFamily::Monospace, 'W'), width(egui::FontFamily::Name("Cascadia Mono".into()), 'W'));
    assert_ne!(width(egui::FontFamily::Proportional, 'i'), width(egui::FontFamily::Proportional, 'W'));
}

#[test]
/// The frame that sees the new setting sets the zoom; egui applies it at the start of the next
/// pass, which it requests immediately (no restart, no extra input).
fn ui_scale_switch_applies_next_frame() {
    let mut e = env();
    e.settle();
    assert_eq!(e.h.ctx.pixels_per_point(), 1.0);
    e.app().settings.ui_scale = rxt_core::UiScale::P150;
    e.h.step();
    e.h.step();
    assert_eq!(e.h.ctx.zoom_factor(), 1.5);
    assert_eq!(e.h.ctx.pixels_per_point(), 1.5);
    e.app().settings.ui_scale = rxt_core::UiScale::Auto;
    e.settle();
    assert_eq!(e.h.ctx.pixels_per_point(), 1.0);
}

// ---- design system audits (task 7.2) ----

/// Font sizes (points) of every text shape in the last frame, monospace excluded (terminal).
fn ui_font_sizes(e: &Env) -> std::collections::BTreeSet<u32> {
    fn walk(shape: &egui::Shape, out: &mut std::collections::BTreeSet<u32>) {
        match shape {
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
            egui::Shape::Text(t) => {
                for section in &t.galley.job.sections {
                    let f = &section.format.font_id;
                    if f.family != egui::FontFamily::Monospace && !t.galley.job.text.trim().is_empty() {
                        out.insert((f.size * 10.0).round() as u32);
                    }
                }
            }
            _ => {}
        }
    }
    let mut out = std::collections::BTreeSet::new();
    e.h.output().shapes.iter().for_each(|c| walk(&c.shape, &mut out));
    out
}

/// AccessKit bounds of the first node with this label and role (any role for `None`).
fn bounds(e: &Env, role: impl Into<Option<Role>>, label: &str) -> egui::Rect {
    let role = role.into();
    let mut nodes = e.h.query_all_by_label(label).filter(|n| role.is_none_or(|r| n.accesskit_node().role() == r));
    let b = nodes.next().and_then(|n| n.accesskit_node().raw_bounds());
    let b = b.unwrap_or_else(|| panic!("no {role:?} {label}"));
    egui::Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32))
}

#[test]
fn main_window_font_sizes_and_band_heights() {
    let m = rxt_core::Metrics::DEFAULT;
    let t = rxt_core::TypeScale::DEFAULT;
    let allowed: Vec<u32> =
        [t.caption, t.body, t.strong, t.heading, t.display].map(|s| (s * 10.0).round() as u32).into();
    let near = |a: f32, b: f32, tol: f32| (a - b).abs() <= tol;
    let mut e = env();
    sample_store(e.app());
    e.app().store.push_recent(&ssh("web1.lab.example"));
    for theme in [ThemeMode::Light, ThemeMode::Dark] {
        e.app().settings.theme = theme;
        e.settle();
        let sizes = ui_font_sizes(&e);
        assert!(sizes.len() >= 2, "{theme:?}: the audit sees the text ({sizes:?})");
        assert!(
            sizes.iter().all(|s| allowed.contains(s)),
            "{theme:?}: UI text sizes ×10 {sizes:?}, allowed {allowed:?}"
        );
        // Menu bar 20, toolbar 52, quick connect / tab row 26, status 22 (window 800 high).
        let menu = bounds(&e, Role::Button, "Terminal");
        assert!(menu.max.y <= m.menu_height + 1.0, "{theme:?}: menu {menu:?}");
        let tool = bounds(&e, Role::Button, "Session");
        assert!(near(tool.min.y, m.menu_height + 2.0, 1.0), "{theme:?}: toolbar top {tool:?}");
        assert!(near(tool.max.y, m.menu_height + m.toolbar_height - 2.0, 1.0), "{theme:?}: toolbar bottom {tool:?}");
        let row_bottom = m.menu_height + m.toolbar_height + m.row_height;
        let quick = bounds(&e, Role::TextInput, "Quick connect");
        let centered =
            near(quick.center().y, row_bottom - m.row_height / 2.0, 1.0) && (24.0..=26.0).contains(&quick.height());
        assert!(centered, "{theme:?}: row {quick:?}");
        let ready = bounds(&e, Role::Label, "Ready");
        assert!(ready.min.y >= 800.0 - m.status_height - 1.0 && ready.max.y <= 800.0, "{theme:?}: status {ready:?}");
        // Sidebar: 34 px strip, then the tree up to the 240 px sidebar edge.
        let strip =
            e.h.query_all_by_label("Sessions")
                .filter_map(|n| n.accesskit_node().raw_bounds())
                .find(|b| b.x0 < 1.0 && b.y0 > 90.0)
                .map(|b| {
                    egui::Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32))
                });
        let strip = strip.unwrap_or_else(|| panic!("{theme:?}: no Sessions strip button"));
        assert!(
            near(strip.width(), m.strip_width, 1.0) && strip.min.y >= row_bottom - 1.0,
            "{theme:?}: strip {strip:?}"
        );
        let root = bounds(&e, None, "User sessions");
        assert!(near(root.min.x, m.strip_width, 2.0), "{theme:?}: tree left {root:?}");
        assert!(near(root.max.x, m.sidebar_width, 4.0), "{theme:?}: sidebar edge {root:?}");
    }
}

// ---- Telnet and Serial (add-telnet-sessions, add-serial-sessions) ----

fn telnet(host: &str) -> Session {
    Session::new(Protocol::Telnet, host, Some("admin".into()), None)
}

fn serial_ports() -> Vec<rxt_core::PortInfo> {
    vec![
        rxt_core::PortInfo {
            path: "/dev/ttyUSB0".into(),
            by_id: Some("/dev/serial/by-id/usb-FTDI_FT232R_LAB0-if00-port0".into()),
            description: Some("FTDI FT232R USB UART".into()),
            vid: Some(0x0403),
            pid: Some(0x6001),
            serial_number: Some("LAB0".into()),
        },
        rxt_core::PortInfo { path: "/dev/ttyS0".into(), ..Default::default() },
    ]
}

#[test]
fn snapshot_telnet_and_serial_editor_pages() {
    let mut e = env();
    let mut s = telnet("router.lab.example");
    e.app().editor = Some(Editor::new(s.clone(), true));
    e.snap("editor_telnet");
    s.telnet.auto_login = true;
    let mut ed = Editor::new(s, true);
    ed.has_password = true;
    e.app().editor = Some(ed);
    e.snap("editor_telnet_autologin");

    let mut serial = Session::serial("/dev/ttyUSB0", rxt_core::LineConfig::default());
    serial.serial.dtr = Some(false);
    e.app().editor = Some(Editor::new(serial.clone(), true));
    e.settle();
    assert!(e.engine.sent().contains(&Command::ListSerialPorts), "port list requested on open");
    e.event(Event::SerialPorts { result: Ok(serial_ports()) });
    assert!(!e.h.state().editor.as_ref().unwrap().serial_ports.is_empty());
    assert!(e.h.query_by_label("Remote host").is_none(), "no host field for Serial");
    e.snap("editor_serial");
    let mut ed = Editor::new(serial, true);
    ed.page = Page::Terminal;
    e.app().editor = Some(ed);
    e.snap("editor_serial_terminal");
}

#[test]
fn telnet_auto_login_needs_user_and_saved_password() {
    let mut ed = Editor::new(Session::new(Protocol::Telnet, "router", None, None), true);
    assert_eq!(ed.session.port, 23);
    assert!(ed.can_save());
    ed.session.telnet.auto_login = true;
    assert!(!ed.can_save() && !ed.validate());
    assert!(ed.error.as_deref().unwrap().contains("username"));
    ed.session.user = Some("admin".into());
    assert!(!ed.can_save(), "no saved password yet");
    ed.password = "fictional-pass".into();
    assert!(ed.can_save());
    ed.password.clear();
    ed.has_password = true;
    assert!(ed.can_save() && ed.validate());
}

#[test]
fn telnet_editor_warns_and_saves_typed_password() {
    let mut e = env();
    let mut s = telnet("router.lab.example");
    s.telnet.auto_login = true;
    e.app().editor = Some(Editor::new(s, true));
    e.settle();
    assert!(e.h.query_by_label(crate::session_editor::TELNET_WARNING).is_some());
    assert!(e.h.get_by_label("OK").accesskit_node().is_disabled(), "auto-login without a password");
    e.h.get_by_role_and_label(Role::PasswordInput, "Password").click();
    e.settle();
    e.h.get_by_role_and_label(Role::PasswordInput, "Password").type_text("fictional-pass");
    e.settle();
    e.h.get_by_label("OK").click();
    e.settle();
    assert!(e.h.state().editor.is_none());
    let saved = e.h.state().store.sessions.iter().find(|s| s.protocol == Protocol::Telnet).unwrap();
    assert!(saved.telnet.auto_login);
}

#[test]
fn serial_editor_requires_port_and_refreshes_ports() {
    let mut e = env();
    e.app().editor = Some(Editor::new(Session::serial("", rxt_core::LineConfig::default()), true));
    e.settle();
    assert!(e.h.get_by_label("OK").accesskit_node().is_disabled());
    assert!(e.h.query_by_label("Choose or type a serial port.").is_some());
    let before = e.engine.sent().iter().filter(|c| **c == Command::ListSerialPorts).count();
    e.h.get_by_label("Refresh serial ports").click();
    e.settle();
    assert_eq!(e.engine.sent().iter().filter(|c| **c == Command::ListSerialPorts).count(), before + 1);
    e.event(Event::SerialPorts { result: Err(rxt_core::Error::Unsupported("serial ports on this platform".into())) });
    assert!(e.h.state().editor.as_ref().unwrap().ports_note.as_deref().unwrap().contains("unsupported"));
    e.event(Event::SerialPorts { result: Ok(serial_ports()) });
    e.h.get_by_label("Detected serial ports").click();
    e.settle();
    e.h.get_by_label("/dev/ttyUSB0  (FTDI FT232R USB UART)").click();
    e.settle();
    assert_eq!(e.h.state().editor.as_ref().unwrap().session.serial.port, "/dev/ttyUSB0");
    e.h.get_by_label("OK").click();
    e.settle();
    let saved = e.h.state().store.sessions.iter().find(|s| s.protocol == Protocol::Serial).unwrap();
    assert_eq!(saved.name, "ttyUSB0 @ 115200");
}

#[test]
fn snapshot_telnet_tab_marker_and_notice() {
    let mut e = env();
    let tab = e.connect(telnet("router.lab.example"));
    // The tab marker is painted text (see the snapshot); the strip shows it for unencrypted protocols.
    assert!(crate::file_tab::unencrypted(Protocol::Telnet) && !crate::file_tab::unencrypted(Protocol::Serial));
    assert!(e.h.query_by_label(crate::tabs::TELNET_NOTICE).is_some());
    e.snap("tab_telnet");
    e.h.get_by_label("Dismiss").click();
    e.settle();
    assert!(e.h.query_by_label(crate::tabs::TELNET_NOTICE).is_none());
    assert!(e.h.state().tabs.iter().any(|t| t.id == tab && t.notice.is_none()));
}

#[test]
fn snapshot_serial_tab_and_line_menu() {
    let mut e = env();
    let tab = e.connect(Session::serial("/dev/ttyUSB0", rxt_core::LineConfig::default()));
    assert_eq!(e.h.state().tabs[0].title, "ttyUSB0 @ 115200");
    e.snap("tab_serial");
    e.app().run(CommandId::SERIAL_SEND_BREAK);
    e.app().apply(Action::SerialLines { tab, dtr: None, rts: Some(false) });
    e.app().run(CommandId::SERIAL_SET_LINES);
    let sent = e.engine.sent();
    assert!(sent.contains(&Command::SerialBreak { tab, ms: 250 }));
    assert!(sent.contains(&Command::SerialLines { tab, dtr: None, rts: Some(false) }));
    assert!(sent.contains(&Command::SerialLines { tab, dtr: Some(false), rts: None }), "shortcut toggles DTR");
    assert_eq!(e.h.state().tabs[0].lines, (false, false));
    e.event(Event::State {
        tab,
        state: ConnState::Disconnected { reason: "device removed".into(), reconnect_at: None },
    });
    let text = e.h.state().tabs[0].term.as_ref().map(|t| t.visible_text()).unwrap_or_default();
    assert!(text.contains("Session stopped (device removed)"), "{text}");
}

#[test]
fn quick_connect_telnet_and_serial() {
    let mut e = env();
    for (input, check) in [
        ("telnet://router.lab:2323", "router.lab"),
        ("serial:///dev/ttyUSB0?baud=9600", "ttyUSB0 @ 9600"),
        ("serial://ttyS1", "ttyS1 @ 115200"),
    ] {
        e.h.get_by_label("Quick connect").click();
        e.settle();
        e.h.get_by_label("Quick connect").type_text(input);
        e.h.key_press(Key::Enter);
        e.settle();
        assert!(e.h.state().tabs.iter().any(|t| t.title.contains(check)), "{input}");
    }
    let telnet = e.h.state().tabs.iter().find(|t| t.session.protocol == Protocol::Telnet).unwrap();
    assert_eq!((telnet.session.port, telnet.session.telnet.auto_login), (2323, false));
    let n = e.h.state().tabs.len();
    for (bad, msg) in [
        ("serial:///dev/ttyUSB0?parity=q", "invalid input: invalid parity 'q'"),
        ("telnet://", "invalid input: missing host"),
    ] {
        e.h.get_by_label("Quick connect").click();
        e.settle();
        e.app().quick.clear();
        e.h.get_by_label("Quick connect").type_text(bad);
        e.h.key_press(Key::Enter);
        e.settle();
        assert!(e.h.query_all_by_value(msg).next().is_some(), "{bad}");
        assert_eq!(e.h.state().tabs.len(), n, "{bad} opens nothing");
    }
}

#[test]
fn multiexec_and_session_log_on_telnet_tab() {
    let mut e = env();
    let log = std::env::temp_dir().join(format!("rxt-telnet-log-{}.log", std::process::id()));
    let mut s = telnet("router");
    s.terminal.log_file = Some(log.to_string_lossy().into_owned());
    let tab = e.connect(s);
    e.app().run(CommandId::MULTIEXEC);
    e.settle();
    e.h.get_by_label("MultiExec admin@router").click();
    e.settle();
    assert!(e.engine.sent().contains(&Command::SelectMultiExec { tab, selected: true }));
    let t = e.app().tabs.iter_mut().find(|t| t.id == tab).unwrap();
    let term = t.term.as_mut().unwrap();
    term.feed(b"router> show version\r\n");
    term.stop_log();
    assert!(std::fs::read_to_string(&log).unwrap().contains("router> show version"));
    std::fs::remove_file(&log).unwrap();
}

#[test]
fn edit_folder_dialog_saves_name_and_icon() {
    let mut e = env();
    e.app().apply(Action::SaveFolder { id: None, parent: None, name: "Lab".into(), icon: None });
    let f = e.h.state().store.folders[0].clone();
    e.app().apply(Action::Ask(Modal::edit_folder(&f)));
    e.settle();
    if let Some(Modal::FolderName { name, .. }) = &mut e.app().modal {
        name.push('2');
    }
    e.h.get_by_label("folder-red").click();
    e.settle();
    e.h.get_by_label("OK").click();
    e.settle();
    let saved = &e.h.state().store.folders[0];
    assert_eq!((saved.name.as_str(), saved.icon.as_deref()), ("Lab2", Some("folder-red")));
    // Picking the default yellow folder stores no icon.
    let f = e.h.state().store.folders[0].clone();
    e.app().apply(Action::Ask(Modal::edit_folder(&f)));
    e.settle();
    e.h.get_by_label("folder").click();
    e.h.get_by_label("OK").click();
    e.settle();
    assert_eq!(e.h.state().store.folders[0].icon, None);
}

#[test]
fn dialog_parent_is_the_x11_window() {
    use eframe::wgpu::rwh::{RawWindowHandle, Win32WindowHandle, XcbWindowHandle, XlibWindowHandle};
    let xlib = RawWindowHandle::Xlib(XlibWindowHandle::new(0x4a0_0007));
    assert_eq!(crate::app::dialog_parent(xlib).as_deref(), Some("x11:4a00007"));
    let xcb = RawWindowHandle::Xcb(XcbWindowHandle::new(std::num::NonZeroU32::new(0x1f).unwrap()));
    assert_eq!(crate::app::dialog_parent(xcb).as_deref(), Some("x11:1f"));
    let win32 = RawWindowHandle::Win32(Win32WindowHandle::new(std::num::NonZeroIsize::new(0x2b04e2).unwrap()));
    assert_eq!(crate::app::dialog_parent(win32).as_deref(), Some("win32:2b04e2"));
    assert_eq!(crate::app::dialog_parent(RawWindowHandle::Web(eframe::wgpu::rwh::WebWindowHandle::new(1))), None);
    // The fake platform records the parent handed to the dialogs service.
    let e = env();
    e.h.state().platform.dialogs.set_parent(Some("x11:1f".into()));
    assert_eq!(e.platform.dialog_parent.lock().unwrap().as_deref(), Some("x11:1f"));
}

// ---- remote monitoring ----

fn disk(source: &str, mount: &str, percent: u8) -> rxt_core::Disk {
    rxt_core::Disk { source: source.into(), mount: mount.into(), used_kib: 1 << 20, size_kib: 2 << 20, percent }
}

/// Linux sample: CPU at `cpu` %, 85 % memory (warning), a 97 % disk (danger).
fn sample(cpu: f32) -> rxt_core::MetricsSample {
    rxt_core::MetricsSample {
        host: "web1".into(),
        os: "Linux".into(),
        cpu: Some(cpu),
        cpu_user_system: Some(cpu - 2.0),
        mem: Some((13_600 << 20, 16_000 << 20)),
        net: Some((20_000, 2_500_000)),
        net_ifaces: vec!["eth0".into()],
        uptime_secs: Some(90_000),
        disks: Some(vec![disk("/dev/sda2", "/", 43), disk("/dev/sdb1", "/data", 97)]),
        disks_more: 0,
    }
}

/// Connected SSH tab with the SFTP panel shown and monitoring reported on.
fn monitored(e: &mut Env, host: &str) -> TabId {
    let tab = e.connect(ssh(host));
    e.app().sidebar_tab = crate::app::SidebarTab::Sftp;
    e.event(Event::Monitor { tab, state: rxt_core::MonitorState::On });
    tab
}

fn set_view(e: &mut Env, tab: TabId, s: rxt_core::MetricsSample) {
    let t = e.app().tabs.iter_mut().find(|t| t.id == tab).unwrap();
    t.monitor.sample = Some(s);
    t.monitor.cpu = (0..60).map(|i| 20.0 + (i as f32 * 0.7).sin().abs() * 70.0).collect();
    e.settle();
}

#[test]
fn monitoring_bar_takes_samples_closes_and_sftp_button_toggles() {
    let mut e = env();
    let tab = e.connect(ssh("web1.lab.example"));
    e.app().sidebar_tab = crate::app::SidebarTab::Sftp;
    e.settle();
    let full = terminal_view(&e).rect().height();
    e.event(Event::Monitor { tab, state: rxt_core::MonitorState::On });
    // Shown before the first sample: host from the session, CPU pending.
    assert!(e.h.query_by_label("Host: web1.lab.example").is_some());
    assert!(e.h.query_by_label("CPU: …").is_some());
    e.engine.metrics.borrow_mut().insert(tab, sample(42.0));
    e.event(Event::Metrics { tab });
    assert!(e.engine.metrics.borrow().is_empty(), "taken while painting the bar");
    for label in
        ["Host: web1", "CPU: 42 %", "Upload: 20.0 kb/s", "Download: 2.50 Mb/s", "Uptime: 1 d 1 h", "Disk: /: 43 %"]
    {
        assert!(e.h.query_by_label(label).is_some(), "{label}");
    }
    let with_bar = terminal_view(&e).rect().height();
    assert!((full - with_bar - monitor_bar_h()).abs() <= 1.0, "{full} {with_bar}");
    let pressed = |e: &Env| e.h.get_by_label("Remote monitoring").accesskit_node().toggled();
    assert_eq!(pressed(&e), Some(egui::accesskit::Toggled::True));

    // A second tab keeps its bar when the first one is closed.
    let other = monitored(&mut e, "db1.lab.example");
    e.app().apply(Action::SelectTab(Some(tab)));
    e.settle();
    e.h.get_by_label("Close remote monitoring").click();
    e.settle();
    assert!(e.engine.sent().contains(&Command::SetMonitoring { tab, enabled: false }));
    assert!(e.h.query_by_label("Close remote monitoring").is_none());
    assert!((terminal_view(&e).rect().height() - full).abs() <= 1.0, "the terminal grows back");
    assert!(e.h.state().tabs.iter().find(|t| t.id == other).unwrap().monitor.shown());

    // The SFTP panel button turns it back on (not saved anywhere).
    let before = e.h.state().settings.clone();
    e.h.get_by_label("Remote monitoring").click();
    e.settle();
    assert!(e.engine.sent().contains(&Command::SetMonitoring { tab, enabled: true }));
    assert!(e.h.query_by_label("Close remote monitoring").is_some());
    assert_eq!(pressed(&e), Some(egui::accesskit::Toggled::True));
    assert_eq!(e.h.state().settings, before);

    // Unavailable: reason in the bar, the button is disabled.
    e.event(Event::Monitor {
        tab,
        state: rxt_core::MonitorState::Unavailable("the server refused an extra channel".into()),
    });
    assert!(e.h.query_by_label("Remote monitoring: Monitoring unavailable").is_some());
    assert!(e.h.get_by_label("Remote monitoring").accesskit_node().is_disabled());
}

fn monitor_bar_h() -> f32 {
    crate::monitor_bar::BAR_H
}

#[test]
fn monitoring_never_shown_for_telnet_and_settings_reach_the_backend() {
    let mut e = env();
    e.settle();
    assert_eq!(*e.engine.monitoring.borrow(), [true]);
    assert_eq!(*e.engine.intervals.borrow(), [2]);
    e.app().settings.remote_monitoring = false;
    e.app().settings.remote_monitoring_interval_secs = 10;
    e.settle();
    assert_eq!(*e.engine.monitoring.borrow(), [true, false]);
    assert_eq!(*e.engine.intervals.borrow(), [2, 10]);

    let tab = e.connect(Session::new(Protocol::Telnet, "router.lab.example", None, None));
    e.event(Event::Monitor { tab, state: rxt_core::MonitorState::On });
    assert!(e.h.query_by_label("Close remote monitoring").is_none());
    e.app().sidebar_tab = crate::app::SidebarTab::Sftp;
    e.settle();
    assert!(e.h.query_by_label("Remote monitoring").is_none());
}

#[test]
fn snapshot_monitoring_bar() {
    let mut e = env();
    let tab = monitored(&mut e, "web1.lab.example");
    set_view(&mut e, tab, sample(97.0));
    e.snap("monitor_bar");

    // Overflow: more disks than fit; arrows scroll, × stays.
    let mut many = sample(35.0);
    many.disks = Some((0..8).map(|i| disk(&format!("/dev/sd{i}"), &format!("/srv/storage/volume-{i}"), 50)).collect());
    many.disks_more = 3;
    set_view(&mut e, tab, many);
    assert!(e.h.query_by_label("Scroll monitoring right").is_some());
    e.snap("monitor_bar_overflow");
    e.h.get_by_label("Scroll monitoring right").click();
    e.settle();
    assert_eq!(e.h.state().tabs[0].monitor.scroll, 1);

    // Non-Linux: host and disks only, the host tooltip explains why.
    let bsd = rxt_core::MetricsSample {
        host: "bsd1".into(),
        os: "FreeBSD".into(),
        disks: Some(vec![disk("/dev/ada0p2", "/", 25)]),
        ..rxt_core::MetricsSample::default()
    };
    set_view(&mut e, tab, bsd);
    assert!(e.h.query_by_label("CPU: …").is_none());
    e.snap("monitor_bar_freebsd");

    e.event(Event::Monitor {
        tab,
        state: rxt_core::MonitorState::Unavailable("the server refused an extra channel".into()),
    });
    e.snap("monitor_bar_unavailable");
}

/// Closing the window with a connected tab asks first; confirming quits.
#[test]
fn exit_with_connected_tabs_asks_first() {
    let mut e = env();
    e.connect(ssh("a"));
    let close = |e: &mut Env| {
        e.h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
        e.h.step();
        e.h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.clear();
    };
    close(&mut e);
    assert_eq!(e.h.state().modal, Some(Modal::ConfirmExit));
    e.app().modal = None;
    e.app().apply(Action::Exit);
    close(&mut e);
    assert_eq!(e.h.state().modal, None, "a confirmed exit is not asked again");
}

// ---- desktop integration: notifications, open, per-tab transfers, dialog folders ----

/// Notifications sent so far, after giving the notifier threads a moment.
fn notes(e: &Env, want: usize) -> Vec<(String, String)> {
    for _ in 0..200 {
        let n = e.platform.notes.lock().unwrap().clone();
        if n.len() >= want {
            return n;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    e.platform.notes.lock().unwrap().clone()
}

fn transfer(e: &mut Env, id: u64, tab: TabId, direction: Direction, remote: &str, local: &str, ok: bool) {
    let (remote, local) = (remote.to_owned(), local.to_owned());
    e.event(Event::TransferStarted { id: TransferId(id), tab, direction, remote, local, total: Some(4) });
    let result = if ok { Ok(4) } else { Err(rxt_core::Error::PermissionDenied("secret-ish detail".into())) };
    e.event(Event::TransferFinished { id: TransferId(id), result });
}

#[test]
fn notifications_only_in_background_and_when_enabled() {
    let mut e = env();
    let tab = e.connect(ssh("web1"));
    transfer(&mut e, 1, tab, Direction::Download, "/r/focused.txt", "/l/focused.txt", true);
    assert!(notes(&e, 1).is_empty(), "focused window: no notification");

    e.h.input_mut().focused = false;
    e.settle();
    transfer(&mut e, 2, tab, Direction::Download, "/r/report.pdf", "/l/report.pdf", true);
    transfer(&mut e, 3, tab, Direction::Upload, "/r/up.bin", "/l/up.bin", false);
    e.event(Event::State { tab, state: ConnState::Disconnected { reason: "eof".into(), reconnect_at: None } });
    let got = notes(&e, 3);
    assert_eq!(
        got,
        [
            ("Download finished".to_owned(), "report.pdf in alice@web1".to_owned()),
            ("Upload failed".to_owned(), "up.bin in alice@web1".to_owned()),
            ("Session disconnected".to_owned(), "alice@web1 disconnected".to_owned()),
        ]
    );
    assert!(got.iter().all(|(_, b)| !b.contains("secret-ish")), "no error details in notifications");

    // Turned off in Settings, or the service failing: nothing shown, no error, tab still updates.
    e.platform.notes.lock().unwrap().clear();
    e.app().settings.notifications = false;
    e.event(Event::State { tab, state: ConnState::Connected });
    e.event(Event::State { tab, state: ConnState::Error("reset".into()) });
    assert!(notes(&e, 1).is_empty());
    e.app().settings.notifications = true;
    e.platform.notify_fails.store(true, std::sync::atomic::Ordering::Relaxed);
    e.app().status.clear();
    e.event(Event::State { tab, state: ConnState::Connected });
    e.event(Event::State { tab, state: ConnState::Error("reset".into()) });
    assert!(notes(&e, 1).is_empty());
    assert!(!e.h.state().status.contains("notification"), "{}", e.h.state().status);
    assert!(matches!(e.h.state().tabs[0].state, ConnState::Error(_)));
    // A tab that never connected (failed login) is not an unexpected drop.
    e.platform.notify_fails.store(false, std::sync::atomic::Ordering::Relaxed);
    let other = e.open(ssh("web2"));
    e.event(Event::State { tab: other, state: ConnState::Error("auth failed".into()) });
    assert!(notes(&e, 1).is_empty());
}

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("rxt-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn downloaded_and_local_files_open_through_the_opener() {
    let dir = scratch("open");
    let file = dir.join("notes.txt");
    std::fs::write(&file, "hi").unwrap();
    let mut e = env();
    let tab = file_tab(&mut e, Protocol::Sftp);
    transfer(&mut e, 1, tab, Direction::Download, "/home/alice/notes.txt", file.to_str().unwrap(), true);
    e.h.get_by_label("Open").click();
    e.settle();
    e.h.get_by_label("Show in folder").click();
    e.settle();
    assert_eq!(*e.platform.opened.lock().unwrap(), [file.clone(), dir.clone()]);

    // Opener failure and a deleted file are reported; nothing else happens.
    *e.platform.open_error.lock().unwrap() = Some("no application is associated".into());
    e.h.get_by_label("Open").click();
    e.settle();
    assert!(e.h.state().status.contains("no application is associated"), "{}", e.h.state().status);
    *e.platform.open_error.lock().unwrap() = None;
    std::fs::remove_file(&file).unwrap();
    e.h.get_by_label("Open").click();
    e.settle();
    assert!(e.h.state().status.contains("notes.txt") && e.h.state().status.contains("no longer exists"));
    assert_eq!(e.platform.opened.lock().unwrap().len(), 2);

    // Local pane: "Open" on a file.
    e.app().transfers.clear();
    std::fs::write(dir.join("photo.jpg"), "x").unwrap();
    let t = e.app().tabs.iter_mut().find(|t| t.id == tab).unwrap();
    t.files.as_mut().unwrap().local.path = dir.to_string_lossy().into_owned();
    e.app().apply(Action::Local(SftpAction::Refresh));
    e.settle();
    e.h.get_by_label("photo.jpg").click_secondary();
    e.settle();
    // The menu item's accessible label carries its shortcut ("Open Enter").
    e.h.get_by(|n| n.role() == Role::Button && n.label().is_some_and(|l| l.starts_with("Open"))).click();
    e.settle();
    assert_eq!(e.platform.opened.lock().unwrap().last(), Some(&dir.join("photo.jpg")));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn transfer_lists_are_per_tab() {
    let mut e = env();
    let ssh_tab = e.connect(ssh("web1.lab.example"));
    let ftp = file_tab(&mut e, Protocol::Ftp);
    transfer(&mut e, 1, ftp, Direction::Upload, "/pub/upload.bin", "/home/user/upload.bin", true);
    assert!(e.h.query_by_label("upload.bin").is_some(), "shown in the FTP tab");
    e.app().apply(Action::SelectTab(Some(ssh_tab)));
    e.app().sidebar_tab = crate::app::SidebarTab::Sftp;
    e.settle();
    assert_eq!(e.h.state().sftp.tab, Some(ssh_tab));
    assert!(e.h.query_by_label("upload.bin").is_none(), "not in the SSH tab's SFTP panel");
    assert!(e.h.query_by_label("Transfers").is_none());
}

#[test]
fn dialogs_start_in_downloads_then_last_folder() {
    let mut e = env();
    let downloads = e.platform.root.join("home/Downloads");
    std::fs::create_dir_all(&downloads).unwrap();
    let out = scratch("dialog").join("out.txt");
    *e.platform.dialog_answer.lock().unwrap() = Some(out.clone());
    e.connect(ssh("web1"));
    let wait = |e: &mut Env, n: usize| {
        for _ in 0..500 {
            e.settle();
            if e.platform.dialog_starts.lock().unwrap().len() >= n && out.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    e.app().run(CommandId::SAVE_TEXT);
    wait(&mut e, 1);
    assert!(out.exists(), "terminal text saved");
    e.app().run(CommandId::SAVE_TEXT);
    wait(&mut e, 2);
    let starts = e.platform.dialog_starts.lock().unwrap().clone();
    assert_eq!(starts, [downloads, out.parent().unwrap().to_owned()]);
    let _ = std::fs::remove_dir_all(out.parent().unwrap());
    // Without a Downloads folder the home folder is used.
    let home = std::path::PathBuf::from("/home/u");
    assert_eq!(crate::app::start_dir(None, Some("/nonexistent/Downloads".into()), home.clone()), home);
}
