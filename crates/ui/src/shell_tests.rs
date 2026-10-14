//! Shell chrome tests (tasks 2.1–2.6): menu bar, toolbar, band heights, quick connect, status
//! bar, Home page and window clamp. Pixel rectangles come from the AccessKit tree.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;

use egui::accesskit::{Role, Toggled};
use egui::{Key, Modifiers, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use rxt_core::{Command, CommandId, ConnState, Event, Metrics, Protocol, Session, TabId, ThemeMode};
use rxt_platform::fake::Fake;

use crate::app::{Action, App, Backend};

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
}

#[derive(Default)]
struct Mem(HashMap<String, String>);

impl eframe::Storage for Mem {
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

/// `build_ui_state` wraps the app in an 8 px outer margin: the app window starts here.
const O: f32 = 8.0;

struct Env {
    h: Harness<'static, App>,
    engine: Engine,
}

fn env_sized(width: f32, storage: Option<&dyn eframe::Storage>) -> Env {
    let platform = Arc::new(Fake { root: std::env::temp_dir().join("rxt-shell-tests"), ..Fake::default() });
    let engine = Engine::default();
    let app = App::new(storage, platform.platform(), engine.clone());
    let h = Harness::builder().with_size(vec2(width, 800.0)).build_ui_state(|ui, app: &mut App| app.show(ui), app);
    let mut e = Env { h, engine };
    e.settle();
    e
}

fn env() -> Env {
    env_sized(1280.0, None)
}

impl Env {
    fn settle(&mut self) {
        self.h.run_steps(6);
    }
    fn rect(&self, role: Role, label: &str) -> Rect {
        let b = self.h.get_by_role_and_label(role, label).accesskit_node().raw_bounds().unwrap_or_default();
        Rect::from_min_max(pos2(b.x0 as f32 - O, b.y0 as f32 - O), pos2(b.x1 as f32 - O, b.y1 as f32 - O))
    }
    fn connect(&mut self) -> TabId {
        let s = Session::new(Protocol::Ssh, "web1", Some("alice".into()), None);
        self.h.state_mut().apply(Action::Open(Box::new(s)));
        self.settle();
        let tab = self.h.state().active.unwrap_or_default();
        self.engine.events.borrow_mut().push_back(Event::State { tab, state: ConnState::Connected });
        self.settle();
        tab
    }
    fn open_menu(&mut self, title: &str) {
        self.h.get_by_role_and_label(Role::Button, title).click();
        self.settle();
    }
}

fn near(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn menu_theme_switch_runs_command() {
    let mut e = env();
    e.open_menu("View");
    e.h.get_by_label("Dark").click();
    e.settle();
    assert_eq!(e.h.state().settings.theme, ThemeMode::Dark);
    assert!(e.h.state().is_dark());
    e.open_menu("View");
    let on = |e: &Env, name: &str| e.h.get_by_label(name).accesskit_node().toggled() == Some(Toggled::True);
    assert!(on(&e, "Dark") && !on(&e, "Light") && !on(&e, "System"));
}

#[test]
fn terminal_menu_disables_tab_items_without_tab() {
    let mut e = env();
    e.open_menu("Terminal");
    for name in ["Find", "Copy", "Paste", "Save terminal text", "Close tab"] {
        assert!(e.h.get_by_label(name).accesskit_node().is_disabled(), "{name} enabled");
    }
    // Menu item and toolbar button.
    assert!(e.h.query_all_by_label("Exit").all(|n| !n.accesskit_node().is_disabled()));
}

#[test]
fn f10_and_alt_t_reach_terminal_and_focus_menu_shortcut_opens_menu() {
    let mut e = env();
    let tab = e.connect();
    if let Some(view) = e.h.query_all_by_label("Terminal").find(|n| n.accesskit_node().role() != Role::Button) {
        view.click();
    }
    e.settle();
    let before = e.engine.sent.borrow().len();
    e.h.key_press(Key::F10);
    e.settle();
    let key = |pressed| egui::Event::Key {
        key: Key::T,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::ALT,
    };
    e.h.event(key(true));
    e.h.event(egui::Event::Text("t".into()));
    e.h.event(key(false));
    e.settle();
    let sent = e.engine.sent.borrow()[before..].to_vec();
    let input: Vec<u8> = sent
        .iter()
        .filter_map(|c| match c {
            Command::Input { tab: t, bytes } if *t == tab => Some(bytes.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(
        input.starts_with(b"\x1b[21~") && input.ends_with(b"t"),
        "terminal did not receive F10 and Alt+T: {sent:?}"
    );
    assert!(!egui::Popup::is_any_open(&e.h.ctx), "a menu opened");
    e.h.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::F10);
    e.settle();
    assert!(egui::Popup::is_any_open(&e.h.ctx), "focus-menu shortcut opened nothing");
    assert!(e.h.query_by_label("Save terminal text").is_some());
}

#[test]
fn exit_stays_pinned_right() {
    for width in [1280.0, 1600.0] {
        let e = env_sized(width, None);
        let exit = e.rect(Role::Button, "Exit");
        let window = width - 2.0 * O;
        assert!(near(window - exit.max.x, 4.0, 1.0), "{width}: exit right edge {}", exit.max.x);
        let session = e.rect(Role::Button, "Session");
        assert!(near(session.min.x, 4.0, 1.0), "{width}: first button at {}", session.min.x);
    }
}

#[test]
fn band_heights_and_quick_connect_placement() {
    let m = Metrics::DEFAULT;
    let mut e = env();
    let menu = e.rect(Role::Button, "Terminal");
    assert!(menu.max.y <= m.menu_height + 1.0, "menu bar item bottom {}", menu.max.y);
    let tool = e.rect(Role::Button, "Session");
    let toolbar_bottom = m.menu_height + m.toolbar_height;
    assert!(near(tool.min.y, m.menu_height + 2.0, 1.0) && near(tool.max.y, toolbar_bottom - 2.0, 1.0), "{tool:?}");
    let quick = e.rect(Role::TextInput, "Quick connect");
    let row_bottom = toolbar_bottom + m.row_height;
    assert!(near(quick.min.x, 2.0, 2.0), "quick connect left {}", quick.min.x);
    let centered = |q: Rect, bottom: f32| {
        near(q.center().y, bottom - m.row_height / 2.0, 1.0) && (24.0..=26.0).contains(&q.height())
    };
    assert!(centered(quick, row_bottom), "quick connect {quick:?} vs row bottom {row_bottom}");
    assert!(quick.max.x <= e.h.state().settings.sidebar_width, "quick connect over the work area");
    let ready = e.rect(Role::Label, "Ready");
    let bottom = 800.0 - 2.0 * O;
    assert!(ready.min.y >= bottom - m.status_height - 1.0 && ready.max.y <= bottom, "status {ready:?}");

    // Captions off: compact toolbar shifts the row up.
    e.h.state_mut().settings.toolbar_captions = false;
    e.settle();
    let quick = e.rect(Role::TextInput, "Quick connect");
    assert!(centered(quick, m.menu_height + m.toolbar_compact_height + m.row_height), "{quick:?}");

    // Collapsed sidebar: quick connect stays at the left end of the tab-strip row.
    e.h.state_mut().run(CommandId::TOGGLE_SIDEBAR);
    e.settle();
    let quick = e.rect(Role::TextInput, "Quick connect");
    assert!(near(quick.min.x, 2.0, 2.0) && quick.width() > 100.0, "{quick:?}");
}

#[test]
fn long_status_is_one_line() {
    let mut e = env();
    let line = "connection refused by peer ".repeat(80);
    e.h.state_mut().status = format!("{line}\nsecond line");
    e.settle();
    let label = e.h.query_all(egui_kittest::kittest::by().role(Role::Label)).find(|n| {
        n.accesskit_node().value().or(n.accesskit_node().label()).is_some_and(|v| v.starts_with("connection refused"))
    });
    let Some(label) = label else { panic!("status label missing") };
    let b = label.accesskit_node().raw_bounds().unwrap_or_default();
    let bottom = f64::from(800.0 - O);
    assert!(b.y1 - b.y0 <= 22.0 && b.y1 <= bottom && b.y0 >= bottom - 23.0, "{b:?}");
    assert!(b.x1 <= f64::from(1280.0 - O));
}

#[test]
fn first_run_chooser_saves_dark_and_hides() {
    let mut e = env();
    assert!(e.h.query_by_label("Dark theme").is_some(), "chooser missing on first run");
    e.h.get_by_label("Dark theme").click();
    e.settle();
    assert!(e.h.state().is_dark());
    assert!(e.h.state().settings.first_run_theme_done);
    assert!(e.h.query_by_label("Dark theme").is_none());
    let mut storage = Mem::default();
    eframe::App::save(e.h.state_mut(), &mut storage);
    let e = env_sized(1280.0, Some(&storage));
    assert_eq!(e.h.state().settings.theme, ThemeMode::Dark);
    assert!(e.h.query_by_label("Dark theme").is_none(), "chooser shown again");
}

#[test]
fn empty_home_has_no_recent_heading() {
    let mut e = env();
    e.h.state_mut().settings.first_run_theme_done = true;
    e.settle();
    assert!(e.h.query_by_label("Recent sessions").is_none());
    for name in ["New session", "Import sessions", "Settings", "rustXterm"] {
        assert!(e.h.query_all_by_label(name).next().is_some(), "{name} missing");
    }
    let new =
        e.h.query_all_by_label("New session")
            .find(|n| n.accesskit_node().raw_bounds().is_some_and(|b| near((b.x1 - b.x0) as f32, 200.0, 4.0)));
    let Some(new) = new else { panic!("200 px New session button missing") };
    let b = new.accesskit_node().raw_bounds().unwrap_or_default();
    assert!(near((b.y1 - b.y0) as f32, 30.0, 1.0), "{b:?}");
    let s = Session::new(Protocol::Ssh, "web1", None, None);
    e.h.state_mut().store.push_recent(&s);
    e.settle();
    assert!(e.h.query_by_label("Recent sessions").is_some());
}
