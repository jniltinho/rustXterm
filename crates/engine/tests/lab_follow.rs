//! "Follow terminal folder" without shell cooperation against the lab sshd (Ubuntu bash, no
//! OSC 7; the engine never parses OSC 7 anyway, so every `CwdChanged` here comes from the
//! shell-PID fallback). Run with `lab/lab.sh up && lab/lab.sh test`. Fictional lab credentials.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use rxt_core::{
    Command, ConnState, Event, HostKeyDecision, PromptAnswer, PromptKind, Protocol, Secret, Session, TabId,
};
use rxt_engine::{Engine, EngineConfig, EngineHandle};

const ALICE_PASS: &str = "lab-alice-pass";

/// Log in as alice on the lab sshd and wait for the shell prompt.
fn connect(h: &EngineHandle) -> TabId {
    let mut s = Session::new(Protocol::Ssh, "127.0.0.1", Some("alice".into()), Some(2222));
    s.ssh.use_agent = false;
    h.send(Command::Open { session: Box::new(s) }).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut tab = None;
    loop {
        assert!(Instant::now() < deadline, "login timed out");
        let Some(ev) = h.try_event() else {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        };
        match ev {
            Event::TabOpened { tab: t, .. } => tab = Some(t),
            Event::Prompt { id, kind, .. } => {
                let answer = match kind {
                    PromptKind::HostKeyUnknown { .. } => PromptAnswer::HostKey(HostKeyDecision::AcceptOnce),
                    PromptKind::Password { .. } => PromptAnswer::Secret(Secret(ALICE_PASS.into())),
                    PromptKind::KeyboardInteractive { prompts, .. } => {
                        PromptAnswer::Responses(prompts.iter().map(|_| Secret(ALICE_PASS.into())).collect())
                    }
                    _ => PromptAnswer::Cancel,
                };
                h.send(Command::Answer { prompt: id, answer }).unwrap();
            }
            Event::State { state: ConnState::Connected, .. } => break,
            Event::State { state: ConnState::Error(e), .. } => panic!("login failed: {e}"),
            _ => {}
        }
    }
    let tab = tab.unwrap();
    drain_until(h, tab, "$ ", Duration::from_secs(20));
    tab
}

/// Drain output and events until the output contains `needle`.
fn drain_until(h: &EngineHandle, tab: TabId, needle: &str, within: Duration) {
    let deadline = Instant::now() + within;
    let mut seen = Vec::new();
    while !String::from_utf8_lossy(&seen).contains(needle) {
        assert!(Instant::now() < deadline, "no {needle:?} in {:?}", String::from_utf8_lossy(&seen));
        while h.try_event().is_some() {}
        if h.drain_output(tab, 1 << 20, &mut seen) == 0 {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Folders reported for `tab` within `within`, draining output meanwhile.
fn folders(h: &EngineHandle, tab: TabId, within: Duration) -> Vec<String> {
    let deadline = Instant::now() + within;
    let (mut out, mut sink) = (Vec::new(), Vec::new());
    while Instant::now() < deadline {
        h.drain_output(tab, 1 << 20, &mut sink);
        match h.try_event() {
            Some(Event::CwdChanged { tab: t, path }) if t == tab => out.push(path),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    out
}

fn type_line(h: &EngineHandle, tab: TabId, line: &str) {
    h.send(Command::Input { tab, bytes: format!("{line}\r").into_bytes() }).unwrap();
}

#[test]
#[ignore = "lab"]
fn ssh_follow_terminal_folder_without_osc7() {
    let h = Engine::start(EngineConfig::default(), Arc::new(|| {})).unwrap();
    let tab = connect(&h);
    // Give the one-shot `ps` probe time to find the login shell.
    std::thread::sleep(Duration::from_secs(1));
    type_line(&h, tab, "cd /etc");
    assert_eq!(folders(&h, tab, Duration::from_secs(2)), ["/etc"], "cd in the login shell is followed within 2 s");
    h.send(Command::List { tab, path: "/etc".into() }).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "no listing of /etc");
        match h.try_event() {
            Some(Event::Listing { path, entries, .. }) if path == "/etc" => {
                assert!(entries.iter().any(|e| e.name == "passwd"));
                break;
            }
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    // A child shell is not followed; after `exit` the login shell is followed again.
    // Each line waits for the shell: typeahead across a shell switch can be mangled by the tty.
    type_line(&h, tab, "sh");
    assert!(folders(&h, tab, Duration::from_secs(1)).is_empty());
    type_line(&h, tab, "cd /tmp");
    assert!(folders(&h, tab, Duration::from_secs(2)).is_empty(), "child shell cd must not be followed");
    type_line(&h, tab, "exit");
    assert!(folders(&h, tab, Duration::from_secs(1)).is_empty());
    type_line(&h, tab, "cd /var");
    assert_eq!(folders(&h, tab, Duration::from_secs(2)), ["/var"]);
    h.shutdown(Duration::from_secs(5));
}
