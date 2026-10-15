//! Local terminal tabs on [`FakePty`]: start sequence, I/O, exit, reconnect and close.

use rxt_platform::fake::FakePty;

use super::*;

fn start_local() -> (EngineHandle, FakePty) {
    let pty = FakePty::default();
    let (h, _, _) = start_config(EngineConfig { local_pty: Some(Arc::new(pty.clone())), ..EngineConfig::default() });
    (h, pty)
}

/// Open a local tab and answer `LocalReady` with a start cursor; returns the tab.
pub(super) fn open_local(h: &EngineHandle, cursor: (u16, u16)) -> TabId {
    h.send(Command::Open { session: Box::new(Session::new(Protocol::Local, "workstation", None, None)) }).unwrap();
    let tab = wait_for(h, |ev| match ev {
        Event::LocalReady { tab, info } => {
            assert_eq!(**info, FakePty::info());
            Some(*tab)
        }
        _ => None,
    });
    h.send(Command::LocalStart { tab, cols: 100, rows: 30, cursor }).unwrap();
    wait_state(h, tab, |s| *s == ConnState::Connected);
    tab
}

fn output(h: &EngineHandle, tab: TabId, needle: &str) -> String {
    let mut text = String::new();
    wait_until(|| {
        let mut buf = Vec::new();
        h.drain_output(tab, usize::MAX, &mut buf);
        text.push_str(&String::from_utf8_lossy(&buf));
        text.contains(needle)
    });
    text
}

#[test]
fn start_sequence_io_resize_and_exit() {
    let (h, pty) = start_local();
    let tab = open_local(&h, (7, 0));
    assert_eq!(pty.with(0, |s| (s.cols, s.rows, s.cursor)), Some((100, 30, (7, 0))));
    pty.send(b"$ ");
    output(&h, tab, "$ ");
    h.send(Command::Input { tab, bytes: b"ls\r".to_vec() }).unwrap();
    h.send(Command::Resize { tab, cols: 90, rows: 20 }).unwrap();
    wait_until(|| pty.with(0, |s| s.input == b"ls\r" && s.resizes == [(90, 20)]).unwrap_or(false));
    // The last output comes before the stopped state.
    pty.send(b"bye\r\n");
    pty.exit(0);
    let stopped = wait_state(&h, tab, |s| matches!(s, ConnState::Disconnected { .. }));
    assert_eq!(stopped, ConnState::Disconnected { reason: "exited with status 0".into(), reconnect_at: None });
    output(&h, tab, "bye");
    assert_eq!(pty.with(0, |s| s.closed), Some(true));
    h.shutdown(WAIT);
}

#[test]
fn exit_while_a_child_keeps_the_terminal_open() {
    let (h, pty) = start_local();
    let tab = open_local(&h, (0, 0));
    // Exit code without end of output: a background child still holds the terminal.
    pty.with(0, |s| s.exit = Some(2));
    wait_state(&h, tab, |s| matches!(s, ConnState::Disconnected { .. }));
    assert_eq!(pty.with(0, |s| s.closed), Some(true), "closing ends the leftover processes");
    h.shutdown(WAIT);
}

#[test]
fn reconnect_starts_a_new_shell_after_a_new_banner() {
    let (h, pty) = start_local();
    let tab = open_local(&h, (0, 0));
    pty.exit(0);
    wait_state(&h, tab, |s| matches!(s, ConnState::Disconnected { .. }));
    h.send(Command::Reconnect { tab }).unwrap();
    wait_for(&h, |ev| matches!(ev, Event::LocalReady { tab: t, .. } if *t == tab).then_some(()));
    assert_eq!(pty.spawned(), 1, "nothing starts before LocalStart");
    h.send(Command::LocalStart { tab, cols: 80, rows: 24, cursor: (9, 0) }).unwrap();
    wait_state(&h, tab, |s| *s == ConnState::Connected);
    assert_eq!(pty.with(1, |s| s.cursor), Some((9, 0)));
    h.shutdown(WAIT);
}

#[test]
fn close_before_start_spawns_nothing_and_close_ends_the_shell() {
    let (h, pty) = start_local();
    h.send(Command::Open { session: Box::new(Session::new(Protocol::Local, "workstation", None, None)) }).unwrap();
    let tab = wait_for(&h, |ev| if let Event::LocalReady { tab, .. } = ev { Some(*tab) } else { None });
    h.send(Command::Close { tab }).unwrap();
    wait_for(&h, |ev| matches!(ev, Event::Closed { tab: t } if *t == tab).then_some(()));
    h.send(Command::LocalStart { tab, cols: 80, rows: 24, cursor: (0, 0) }).unwrap();
    sleep(Duration::from_millis(100));
    assert_eq!(pty.spawned(), 0);

    // Closing a running tab whose shell blocks writes and floods output.
    pty.block_writes.store(true, Ordering::Relaxed);
    let tab = open_local(&h, (0, 0));
    pty.send(&vec![b'y'; 1 << 20]);
    h.send(Command::Input { tab, bytes: b"x".to_vec() }).unwrap();
    let t = Instant::now();
    h.send(Command::Close { tab }).unwrap();
    wait_for(&h, |ev| matches!(ev, Event::Closed { tab: c } if *c == tab).then_some(()));
    wait_until(|| pty.with(0, |s| s.closed).unwrap_or(false));
    assert!(t.elapsed() < Duration::from_secs(2));
    h.shutdown(WAIT);
}

#[test]
fn no_backend_is_an_error_state() {
    let (h, _, _) = start(1024);
    let tab = open_session(&h, Session::new(Protocol::Local, "workstation", None, None));
    let s = wait_state(&h, tab, |s| matches!(s, ConnState::Error(_)));
    assert!(matches!(s, ConnState::Error(ref e) if e.contains("local terminals")), "{s:?}");
    h.shutdown(WAIT);
}

#[test]
fn replies_go_to_their_tab_only() {
    let (h, state, _) = start(1024);
    let a = connect(&h, "a");
    let b = connect(&h, "b");
    for tab in [a, b] {
        h.send(Command::SelectMultiExec { tab, selected: true }).unwrap();
    }
    h.send(Command::SetMultiExec { enabled: true }).unwrap();
    wait_for(&h, |ev| matches!(ev, Event::MultiExec { enabled: true, selected } if selected.len() == 2).then_some(()));
    h.send(Command::Reply { tab: a, bytes: b"\x1b[3;1R".to_vec() }).unwrap();
    wait_until(|| state.writes_to("a").len() == 1);
    sleep(Duration::from_millis(50));
    assert!(state.writes_to("b").is_empty(), "replies are never broadcast");
    h.shutdown(WAIT);
}
