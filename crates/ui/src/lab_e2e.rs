//! End-to-end connection tests against the Docker lab: the egui [`App`] in a kittest harness,
//! driven like a user (quick connect, session editor, dialogs, panes, tunnels window), on top of
//! the real engine and protocol crates. `#[ignore = "lab"]`; credentials are the fictional lab
//! accounts from `lab/README.md`.

use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use egui::accesskit::Role;
use egui::{Key, Modifiers, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use rxt_core::{CommandId, ConnState, FtpTls, JumpHost, Protocol, Session, TabId, TunnelState};
use rxt_engine::{Engine, EngineConfig, EngineHandle};
use rxt_platform::fake::Fake;

use crate::app::{Action, App};
use crate::lab_gates::ONE_GATE;
use crate::sftp_panel::SftpAction;
use crate::vault::Vault;

const ALICE: &str = "lab-alice-pass";
const WAIT: Duration = Duration::from_secs(30);

struct E2e {
    h: Harness<'static, App>,
    handle: EngineHandle,
    /// Scratch directory of this test (known_hosts, local files, vault).
    dir: PathBuf,
    _one: MutexGuard<'static, ()>,
}

/// Fresh app on the real engine; `vault` locks the session database first.
fn e2e(name: &str, vault: bool) -> E2e {
    let one = ONE_GATE.lock().unwrap_or_else(PoisonError::into_inner);
    let dir = std::env::temp_dir().join(format!("rxt-lab-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let config = EngineConfig { known_hosts: dir.join("known_hosts"), ..EngineConfig::default() };
    let handle = Engine::start(config, Arc::new(|| {})).unwrap();
    let platform = Arc::new(Fake { root: dir.join("platform"), ..Fake::default() });
    let mut app = App::new(None, platform.platform(), handle.clone());
    if vault {
        Vault::fast(dir.join("vault"), platform.platform().keyring).create("lab master", false).unwrap();
        app = app.with_vault(Arc::new(Vault::fast(dir.join("vault"), platform.platform().keyring)));
    }
    let h = Harness::builder().with_size(vec2(1180.0, 720.0)).build_ui_state(|ui, app: &mut App| app.show(ui), app);
    E2e { h, handle, dir, _one: one }
}

impl E2e {
    /// Step frames until `done`; panics with the app state after [`WAIT`].
    fn until(&mut self, what: &str, done: impl Fn(&Harness<'static, App>) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done(&self.h) {
            if Instant::now() > deadline {
                let app = self.h.state();
                let tabs: Vec<_> = app.tabs.iter().map(|t| (&t.title, &t.state)).collect();
                let prompts: Vec<_> = app.prompts.iter().map(|p| &p.kind).collect();
                let errors: Vec<_> = app
                    .tabs
                    .iter()
                    .filter_map(|t| t.files.as_ref())
                    .map(|f| (&f.remote.path, &f.remote.error))
                    .collect();
                panic!(
                    "timed out waiting for {what}; status {:?}; tabs {tabs:?}; prompts {prompts:?}; tunnels {:?}; panel {:?} {:?}; panes {errors:?}",
                    app.status, app.tunnel_states, app.sftp.path, app.sftp.error
                );
            }
            self.h.step();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Wait for a widget, then let new windows finish their sizing pass so it takes input.
    fn wait_label(&mut self, label: &str) {
        self.until(label, |h| h.query_all_by_label(label).next().is_some());
        self.h.run_steps(3);
    }

    fn click(&mut self, label: &str) {
        self.wait_label(label);
        self.h.get_by_label(label).click();
        self.h.run_steps(2);
    }

    /// Replace the text of the text field labelled `label` (the `nth` match; labels and grid
    /// headers of the same name are skipped) and press Enter.
    fn type_into(&mut self, label: &str, nth: usize, text: &str, enter: bool) {
        self.wait_label(label);
        self.h.query_all_by_role_and_label(Role::TextInput, label).nth(nth).unwrap().click();
        self.h.run_steps(2);
        self.h.key_press_modifiers(Modifiers::COMMAND, Key::A);
        self.h.run_steps(1);
        self.h.query_all_by_role_and_label(Role::TextInput, label).nth(nth).unwrap().type_text(text);
        self.h.run_steps(2);
        if enter {
            self.h.key_press(Key::Enter);
            self.h.run_steps(2);
        }
    }

    fn quick_connect(&mut self, target: &str) {
        self.type_into("Quick connect", 0, target, true);
    }

    /// Host key or certificate dialog: "Accept once".
    fn accept(&mut self, heading: &str) {
        self.until(heading, |h| h.query_all_by_value(heading).next().is_some());
        self.click("Accept once");
    }

    /// Password dialog whose field label starts with `prompt` ("Password for alice@").
    fn password(&mut self, prompt: &str, secret: &str) {
        self.until(prompt, |h| h.query_all_by_label_contains(prompt).next().is_some());
        self.h.run_steps(3);
        self.h.query_all_by_label_contains(prompt).next().unwrap().click();
        self.h.run_steps(2);
        self.h.query_all_by_label_contains(prompt).next().unwrap().type_text(secret);
        self.h.run_steps(2);
        self.click("OK");
    }

    /// Active tab once it is connected.
    fn connected(&mut self) -> TabId {
        self.until("connected tab", |h| h.state().active_tab().is_some_and(|t| t.state == ConnState::Connected));
        self.h.state().active.unwrap()
    }

    fn remote_entries(&self, tab: TabId) -> Vec<String> {
        remote_entries(self.h.state(), tab)
    }

    /// Wait until the remote browser of `tab` shows `name`.
    fn wait_entry(&mut self, tab: TabId, name: &str) {
        self.until(name, |h| {
            remote_entries(h.state(), tab).iter().any(|n| n == name) && h.query_all_by_label(name).next().is_some()
        });
    }

    /// Remote pane of a file tab: create a folder through the dialog, see it listed, delete it.
    fn file_tab_mkdir_roundtrip(&mut self, tab: TabId) {
        let name = unique("e2e-dir");
        self.until("remote listing", |h| {
            let t = h.state().tabs.iter().find(|t| t.id == tab).unwrap();
            let r = &t.files.as_ref().unwrap().remote;
            !r.path.is_empty() && !r.loading && r.error.is_none()
        });
        let remote_new = self.h.query_all_by_label("Create new directory").count() - 1;
        self.h.query_all_by_label("Create new directory").nth(remote_new).unwrap().click();
        self.h.run_steps(2);
        self.type_into("Folder name", 0, &name, false);
        self.click("Create");
        self.wait_entry(tab, &name);
        self.click(&name);
        let remote_delete = self.h.query_all_by_label("Delete selected files").count() - 1;
        self.h.query_all_by_label("Delete selected files").nth(remote_delete).unwrap().click();
        // The confirmation dialog's button is the last "Delete".
        self.until("delete confirmation", |h| h.query_all_by_label("Delete").next().is_some());
        self.h.run_steps(3);
        self.h.query_all_by_label("Delete").last().unwrap().click();
        self.h.run_steps(2);
        let gone = |e: &Self| !e.remote_entries(tab).contains(&name);
        let deadline = Instant::now() + WAIT;
        while !gone(self) {
            assert!(Instant::now() < deadline, "{name} not deleted");
            self.h.step();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Desktop tab: the engine framebuffer gets real pixels and the rendered tab shows them.
    fn desktop_renders(&mut self, tab: TabId) {
        let handle = self.handle.clone();
        let lit = move || {
            handle
                .with_framebuffer(tab, |fb| fb.pixels.as_chunks::<4>().0.iter().any(|p| p[..3].iter().any(|c| *c > 40)))
                == Some(true)
        };
        self.until("non-black remote framebuffer", |_| lit());
        self.until("desktop texture", |h| h.query_all_by_value("Waiting for the remote desktop…").next().is_none());
        self.h.run_steps(3);
        let rect = self.h.get_by_label("Remote desktop").rect();
        let ppp = self.h.ctx.pixels_per_point();
        let img = self.h.render().unwrap();
        let lit_px = img
            .enumerate_pixels()
            .filter(|(x, y, _)| rect.contains(egui::pos2(*x as f32 / ppp, *y as f32 / ppp)))
            .filter(|(_, _, p)| p.0[..3].iter().any(|c| *c > 40))
            .count();
        assert!(lit_px > 0, "rendered desktop area {rect:?} is black");
    }

    fn finish(self) {
        self.handle.shutdown(Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A file dropped from the desktop onto the window.
#[derive(Debug)]
struct Dropped(PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

fn remote_entries(app: &App, tab: TabId) -> Vec<String> {
    let st = if app.sftp.tab == Some(tab) {
        &app.sftp
    } else {
        &app.tabs.iter().find(|t| t.id == tab).unwrap().files.as_ref().unwrap().remote
    };
    st.entries.iter().map(|e| e.name.clone()).collect()
}

fn unique(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
    format!("{prefix}-{}-{nanos}", std::process::id())
}

fn terminal_text(h: &Harness<'static, App>, tab: TabId) -> String {
    h.state().tabs.iter().find(|t| t.id == tab).and_then(|t| t.term.as_ref()).map(|t| t.visible_text()).unwrap()
}

/// Quick connect to the lab sshd as alice through the dialogs; returns the connected tab.
fn ssh_tab(e: &mut E2e) -> TabId {
    e.quick_connect("alice@127.0.0.1:2222");
    e.accept("Unknown host key");
    e.password("Password for alice@", ALICE);
    e.connected()
}

/// Type a command into the focused terminal widget and wait for `expect` on the grid.
fn run_in_terminal(e: &mut E2e, tab: TabId, command: &str, expect: &str) {
    // The terminal view, not the "Terminal" menu.
    e.h.get_by(|n| n.label().as_deref() == Some("Terminal") && n.role() != Role::Button).click();
    e.h.run_steps(2);
    e.h.event(egui::Event::Text(command.into()));
    e.h.key_press(Key::Enter);
    e.until(expect, |h| terminal_text(h, tab).contains(expect));
}

#[test]
#[ignore = "lab"]
fn e2e_ssh_quick_connect_runs_command() {
    let mut e = e2e("ssh", false);
    let tab = ssh_tab(&mut e);
    // The typed line holds `$((40+2))`; only the shell's output holds `E2E_42`.
    run_in_terminal(&mut e, tab, "echo E2E_$((40+2))", "E2E_42");
    e.finish();
}

#[test]
#[ignore = "lab"]
fn e2e_ssh_session_box_then_server_banner_then_prompt() {
    let mut e = e2e("banner", false);
    let tab = ssh_tab(&mut e);
    e.until("shell prompt", |h| terminal_text(h, tab).contains(":~$"));
    let text = terminal_text(&e.h, tab);
    let at = |what: &str| text.find(what).unwrap_or_else(|| panic!("no {what:?} in\n{text}"));
    let (title, target, server) = (at("rustXterm "), at("SSH session to alice@127.0.0.1:2222"), at("Server: SSH-2.0-"));
    let (compression, banner, prompt) = (at("Compression"), at("RXT LAB BANNER: fictional"), at(":~$"));
    assert!(
        title < target && target < compression && compression < server && server < banner && banner < prompt,
        "{text}"
    );
    e.finish();
}

#[test]
#[ignore = "lab"]
fn e2e_sftp_side_panel_follows_ssh_tab() {
    let mut e = e2e("sftp-panel", false);
    let tab = ssh_tab(&mut e);
    // The sidebar switches to SFTP by itself after login; clicking the active icon would collapse it.
    e.h.step();
    if e.h.state().sidebar_tab != crate::sidebar::Page::Sftp || !e.h.state().settings.sidebar_open {
        e.click("SFTP");
    }
    e.until("home listing", |h| h.state().sftp.tab == Some(tab) && h.state().sftp.path == "/home/alice");
    e.until("home entries", |h| h.query_all_by_label("..").next().is_some());
    // New folder through the dialog, then into it with a double click.
    let dir = unique("e2e-panel");
    e.click("Create new directory");
    e.type_into("Folder name", 0, &dir, false);
    e.click("Create");
    e.wait_entry(tab, &dir);
    let path = format!("/home/alice/{dir}");
    e.type_into("Path", 0, &path, true);
    e.until("navigated", |h| h.state().sftp.path == path);
    // Upload a file dropped on the window: it lands in the panel's directory.
    let local = e.dir.join("dropped.txt");
    std::fs::write(&local, b"e2e upload\n").unwrap();
    e.h.input_mut().dropped_files.push(Arc::new(Dropped(local)));
    e.h.run_steps(2);
    e.wait_entry(tab, "dropped.txt");
    e.until("transfer done", |h| h.state().transfers.iter().any(|t| matches!(t.result, Some(Ok(_)))));
    e.h.state_mut().apply(Action::Sftp(SftpAction::Delete(path)));
    e.h.run_steps(5);
    e.finish();
}

#[test]
#[ignore = "lab"]
fn e2e_sftp_standalone_dual_pane_transfer() {
    let mut e = e2e("sftp-tab", false);
    e.quick_connect("sftp://alice@127.0.0.1:2222");
    e.accept("Unknown host key");
    e.password("Password for alice@", ALICE);
    let tab = e.connected();
    let local_dir = e.dir.join("downloads");
    std::fs::create_dir_all(&local_dir).unwrap();
    // The first remote listing resets the path field: type only after it arrived.
    e.until("home listing", |h| {
        let r = &h.state().tabs.iter().find(|t| t.id == tab).unwrap().files.as_ref().unwrap().remote;
        r.path == "/home/alice" && !r.loading
    });
    // Left pane: the local folder; right pane: /etc on the server.
    e.type_into("Path", 0, &local_dir.to_string_lossy(), true);
    e.until("local pane", |h| {
        h.state().tabs.iter().find(|t| t.id == tab).unwrap().files.as_ref().unwrap().local.path
            == local_dir.to_string_lossy()
    });
    e.type_into("Path", 1, "/etc", true);
    e.wait_entry(tab, "hostname");
    e.h.get_by_label("hostname").scroll_to_me();
    e.h.run_steps(3);
    e.click("hostname");
    e.click("Download selected files");
    let file = local_dir.join("hostname");
    e.until("downloaded file", |_| std::fs::read_to_string(&file).is_ok_and(|s| s == "sshd\n"));
    e.until("listed in the local pane", |h| {
        let local = &h.state().tabs.iter().find(|t| t.id == tab).unwrap().files.as_ref().unwrap().local;
        local.entries.iter().any(|f| f.name == "hostname")
    });
    e.finish();
}

#[test]
#[ignore = "lab"]
fn e2e_ftp_plain_marker_and_listing() {
    let mut e = e2e("ftp", false);
    e.quick_connect("ftp://alice@127.0.0.1:2121");
    e.password("Password for alice@", ALICE);
    let tab = e.connected();
    assert!(e.h.query_all_by_value("Unencrypted").next().is_some(), "plain FTP shows the Unencrypted marker");
    e.file_tab_mkdir_roundtrip(tab);
    e.finish();
}

/// FTPS session created in the session editor: certificate dialog, password, listing.
fn ftps(name: &str, tls: FtpTls, port: u16) {
    let mut e = e2e(name, false);
    e.click("Session");
    e.click("FTPS");
    if tls == FtpTls::Explicit {
        e.click("Explicit (AUTH TLS)");
    }
    e.type_into("Remote host", 0, "127.0.0.1", false);
    e.type_into("Port", 0, &port.to_string(), false);
    e.type_into("Username", 0, "alice", false);
    e.click("OK");
    let saved = e.h.state().store.sessions.last().cloned().unwrap();
    assert_eq!((saved.protocol, saved.ftp.tls, saved.port), (Protocol::Ftps, tls, port));
    e.accept("Unknown certificate");
    e.password("Password for alice@", ALICE);
    let tab = e.connected();
    assert!(e.h.query_all_by_value("Unencrypted").next().is_none());
    e.file_tab_mkdir_roundtrip(tab);
    e.finish();
}

#[test]
#[ignore = "lab"]
fn e2e_ftps_explicit_cert_prompt_then_listing() {
    ftps("ftps-explicit", FtpTls::Explicit, 2121);
}

#[test]
#[ignore = "lab"]
fn e2e_ftps_implicit_cert_prompt_then_listing() {
    ftps("ftps-implicit", FtpTls::Implicit, 2990);
}

#[test]
#[ignore = "lab"]
fn e2e_rdp_desktop_renders_and_accepts_input() {
    let mut e = e2e("rdp", false);
    e.quick_connect("rdp://alice@127.0.0.1:13389");
    e.accept("Unknown certificate");
    e.password("Password for alice@", ALICE);
    let tab = e.connected();
    e.desktop_renders(tab);
    // Click (grabs the keyboard) and Enter: harmless in the lab xterm shared with the gates.
    e.click("Remote desktop");
    e.h.key_press(Key::Enter);
    let settle = Instant::now();
    while settle.elapsed() < Duration::from_secs(1) {
        e.h.step();
    }
    let t = e.h.state().active_tab().unwrap();
    assert!(t.desktop.as_ref().unwrap().grabbed);
    assert_eq!(t.state, ConnState::Connected, "status {:?}", e.h.state().status);
    e.finish();
}

#[test]
#[ignore = "lab"]
fn e2e_vnc_desktop_renders() {
    let mut e = e2e("vnc", false);
    e.quick_connect("vnc://127.0.0.1:5901");
    e.password("Password for @", "lab-vnc1");
    let tab = e.connected();
    e.desktop_renders(tab);
    e.finish();
}

#[test]
#[ignore = "lab"]
fn e2e_vnc_via_ssh_gateway_renders() {
    let mut e = e2e("vnc-ssh", false);
    let mut s = Session::new(Protocol::Vnc, "vnc", None, Some(5900));
    s.vnc.via_ssh = Some(JumpHost { host: "127.0.0.1".into(), port: 2223, user: "alice".into(), key_file: None });
    e.h.state_mut().apply(Action::SaveSession { session: Box::new(s), connect: true });
    e.accept("Unknown host key");
    e.password("Password for alice@", ALICE);
    e.password("Password for @", "lab-vnc1");
    let tab = e.connected();
    e.desktop_renders(tab);
    e.finish();
}

#[test]
#[ignore = "lab"]
fn e2e_tunnel_local_forward_through_bastion() {
    let mut e = e2e("tunnel", false);
    let mut bastion = Session::new(Protocol::Ssh, "127.0.0.1", Some("alice".into()), Some(2223));
    bastion.ssh.use_agent = false;
    e.h.state_mut().store.upsert(bastion);
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    e.click("Tunneling");
    e.click("New SSH tunnel");
    e.type_into("Name", 0, "e2e-internal", false);
    e.type_into("Destination host", 0, "internal", false);
    // Ports are drag values; set them like a typed value.
    let ed = e.h.state_mut().tunnel_editor.as_mut().unwrap();
    (ed.tunnel.bind_port, ed.tunnel.dest_port) = (port, 22);
    e.click("Save");
    e.click("Start e2e-internal");
    e.accept("Unknown host key");
    e.password("Password for alice@", ALICE);
    let state = |h: &Harness<'static, App>| h.state().tunnel_states.get("e2e-internal").cloned();
    e.until("tunnel running", |h| state(h) == Some(TunnelState::Running));
    assert_eq!(e.h.state().tunnel_states.get("e2e-internal"), Some(&TunnelState::Running));
    let mut conn = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut banner = [0u8; 8];
    conn.read_exact(&mut banner).unwrap();
    assert_eq!(&banner, b"SSH-2.0-");
    drop(conn);
    e.click("Stop e2e-internal");
    e.until("tunnel stopped", |h| state(h) != Some(TunnelState::Running));
    e.finish();
}

#[test]
#[ignore = "lab"]
fn e2e_locked_store_quick_connect_still_works() {
    let mut e = e2e("locked", true);
    e.until("unlock dialog", |h| h.state().store_dialog.is_some());
    assert!(e.h.state().locked());
    e.click("Cancel");
    let tab = ssh_tab(&mut e);
    assert!(e.h.state().locked());
    run_in_terminal(&mut e, tab, "echo LOCKED_$((6*7))", "LOCKED_42");
    // Saved-session features still need the database.
    e.h.state_mut().run(CommandId::NEW_SESSION);
    e.h.run_steps(2);
    assert!(e.h.state().editor.is_none());
    e.finish();
}
