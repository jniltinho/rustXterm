//! Keyboard navigation and accessibility over the whole window (task 6.5): the editor driven
//! by keyboard only, Tab kept by a focused terminal, and an AccessKit walk asserting that no
//! interactive widget of the main screens and dialogs has an empty name.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use egui::accesskit::Role;
use egui::{Event as Input, Key, Modifiers, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use rxt_core::{Command, ConnState, Event, Framebuffer, PromptId, PromptKind, Protocol, Session, TabId};
use rxt_platform::fake::Fake;

use crate::app::{Action, App, Backend, Modal};
use crate::dialogs::Prompt;

/// Engine double: records commands and answers `Open` with a new tab.
#[derive(Clone, Default)]
struct Engine {
    sent: Rc<RefCell<Vec<Command>>>,
    events: Rc<RefCell<VecDeque<Event>>>,
    next: Rc<RefCell<u64>>,
}

impl Backend for Engine {
    fn send(&self, command: Command) -> rxt_core::Result<()> {
        if let Command::Open { session } = &command {
            *self.next.borrow_mut() += 1;
            let tab = TabId(*self.next.borrow());
            self.events.borrow_mut().push_back(Event::TabOpened { tab, session: session.clone() });
        }
        self.sent.borrow_mut().push(command);
        Ok(())
    }
    fn try_event(&self) -> Option<Event> {
        self.events.borrow_mut().pop_front()
    }
    fn drain_output(&self, _: TabId, _: usize, _: &mut Vec<u8>) -> usize {
        0
    }
    fn pending_output(&self) -> Vec<TabId> {
        Vec::new()
    }
    fn with_framebuffer(&self, _: TabId, f: &mut dyn FnMut(&mut Framebuffer)) -> bool {
        let mut fb = Framebuffer::default();
        let _ = fb.apply(&rxt_core::DesktopEvent::Resized { width: 64, height: 48 });
        f(&mut fb);
        true
    }
}

fn app() -> (Harness<'static, App>, Engine) {
    let platform = Arc::new(Fake { root: std::env::temp_dir().join("rxt-ui-keyboard-tests"), ..Fake::default() });
    let engine = Engine::default();
    let app = App::new(None, platform.platform(), engine.clone());
    // The harness Ui of the first frame predates the style the app installs: give it the
    // app's style up front.
    let mut h = Harness::builder().with_size(vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut App| {
            ui.set_style(crate::theme::style(&app.tokens));
            app.show(ui);
        },
        app,
    );
    h.run_steps(5);
    (h, engine)
}

fn open(h: &mut Harness<'static, App>, engine: &Engine, s: Session) -> TabId {
    h.state_mut().apply(Action::Open(Box::new(s)));
    h.run_steps(5);
    let tab = h.state().active.unwrap_or_default();
    engine.events.borrow_mut().push_back(Event::State { tab, state: ConnState::Connected });
    h.run_steps(5);
    tab
}

/// Label of the focused AccessKit node, if any.
fn focused(h: &Harness<'static, App>) -> Option<String> {
    h.query_by(|n| n.is_focused()).map(|n| n.accesskit_node().label().unwrap_or_default())
}

#[test]
fn terminal_keeps_tab() {
    let (mut h, engine) = app();
    let tab = open(&mut h, &engine, Session::new(Protocol::Ssh, "web1.lab.example", Some("alice".into()), None));
    h.get_by(|n| n.label().as_deref() == Some("Terminal") && n.role() == Role::Unknown).click();
    h.run_steps(3);
    engine.sent.borrow_mut().clear();
    h.key_press(Key::Tab);
    h.run_steps(3);
    let input: Vec<u8> = engine
        .sent
        .borrow()
        .iter()
        .filter_map(|c| match c {
            Command::Input { tab: t, bytes } if *t == tab => Some(bytes.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(input, [0x09], "the terminal receives Tab");
    assert_eq!(focused(&h).as_deref(), Some("Terminal"), "focus does not move");
}

#[test]
fn editor_by_keyboard() {
    let (mut h, _engine) = app();
    h.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::N);
    h.run_steps(3);
    assert!(h.state().editor.is_some(), "the new-session shortcut opens the editor");
    h.key_press(Key::ArrowRight);
    h.run_steps(2);
    assert_eq!(h.state().editor.as_ref().map(|e| e.session.protocol), Some(Protocol::Telnet), "Right selects Telnet");
    for _ in 0..12 {
        if focused(&h).as_deref() == Some("Remote host") {
            break;
        }
        h.key_press(Key::Tab);
        h.run_steps(1);
    }
    assert_eq!(focused(&h).as_deref(), Some("Remote host"), "Tab reaches the Remote host field");
    h.event(Input::Text("files.lab.example".into()));
    h.run_steps(1);
    h.key_press(Key::Enter);
    h.run_steps(5);
    assert!(h.state().editor.is_none(), "Enter saves and closes");
    let saved = h.state().store.sessions.first().cloned();
    let saved = saved.unwrap_or_else(|| panic!("no session saved"));
    assert_eq!((saved.protocol, saved.host.as_str()), (Protocol::Telnet, "files.lab.example"));
    assert_eq!(focused(&h), Some(saved.name.clone()), "focus returns to the tree on the new session");
}

/// Interactive roles whose nodes need a non-empty name.
const NAMED: [Role; 13] = [
    Role::Button,
    Role::CheckBox,
    Role::ComboBox,
    Role::TextInput,
    Role::MultilineTextInput,
    Role::PasswordInput,
    Role::Tab,
    Role::TreeItem,
    Role::MenuItem,
    Role::MenuItemCheckBox,
    Role::MenuItemRadio,
    Role::RadioButton,
    Role::SpinButton,
];

/// Interactive nodes without a name, as "Role at rect" strings.
fn unnamed(h: &Harness<'static, App>) -> Vec<String> {
    h.query_all_by(|n| NAMED.contains(&n.role()) && n.label().is_none_or(|l| l.trim().is_empty()))
        .map(|n| format!("{:?} at {:?}", n.accesskit_node().role(), n.rect()))
        .collect()
}

#[test]
fn accesskit_names_are_never_empty() {
    let (mut h, engine) = app();
    let mut missing = Vec::new();
    let mut check = |h: &mut Harness<'static, App>, screen: &str| {
        h.run_steps(3);
        assert!(h.query_all_by_role(Role::Button).count() > 0, "{screen}: the walk sees the tree");
        missing.extend(unnamed(h).into_iter().map(|n| format!("{screen}: {n}")));
    };
    check(&mut h, "main_empty");
    open(&mut h, &engine, Session::new(Protocol::Ssh, "web1.lab.example", Some("alice".into()), None));
    check(&mut h, "terminal");
    open(&mut h, &engine, Session::new(Protocol::Sftp, "files.lab.example", Some("alice".into()), None));
    check(&mut h, "file_tab");
    open(&mut h, &engine, Session::new(Protocol::Rdp, "win.lab.example", Some("bob".into()), None));
    check(&mut h, "desktop_rdp");
    h.state_mut().show_settings = true;
    check(&mut h, "settings");
    h.state_mut().show_settings = false;
    h.state_mut().show_tunnels = true;
    check(&mut h, "tunnels");
    h.state_mut().show_tunnels = false;
    h.state_mut().apply(Action::NewSession(None));
    check(&mut h, "editor");
    h.state_mut().editor = None;
    let tab = h.state().active.unwrap_or_default();
    let modals = [
        Modal::ConfirmClose(tab),
        Modal::RenameTab(tab, "web1".into()),
        Modal::DeleteRemote { local: false, path: "/var/log/old".into() },
        Modal::Chmod { path: "/home/alice/run.sh".into(), mode: "755".into(), error: None },
        Modal::Mkdir { local: false, dir: "/home/alice".into(), name: String::new() },
        Modal::About,
    ];
    for m in modals {
        let name = format!("{m:?}");
        h.state_mut().modal = Some(m);
        check(&mut h, &name);
    }
    h.state_mut().modal = None;
    let prompts = [
        PromptKind::Password { user: "alice".into(), host: "web1".into(), retry: false },
        PromptKind::HostKeyUnknown {
            host: "web1".into(),
            port: 22,
            algorithm: "ssh-ed25519".into(),
            fingerprint: "SHA256:abc".into(),
        },
        PromptKind::Overwrite { path: "/tmp/a".into() },
    ];
    for (i, kind) in prompts.into_iter().enumerate() {
        let name = format!("{kind:?}");
        h.state_mut().prompts.push_back(Prompt::new(PromptId(i as u64 + 1), None, kind));
        check(&mut h, &name);
        h.state_mut().prompts.clear();
    }
    assert!(missing.is_empty(), "interactive widgets without a name:\n{}", missing.join("\n"));
}
