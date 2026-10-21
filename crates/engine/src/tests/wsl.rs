//! WSL tabs on the fake [`Wsl`] backend and [`FakePty`] (wsl-sessions spec, design W3/W4):
//! launch arguments, the Windows-only refusal, startup-command typing and capture exclusion.

use rxt_core::WslOptions;
use rxt_platform::fake::{FakePty, FakeWsl};

use super::*;

fn wsl_session(opts: WslOptions) -> Session {
    named_wsl_session("wsl", opts)
}

fn named_wsl_session(name: &str, opts: WslOptions) -> Session {
    Session { protocol: Protocol::Wsl, name: name.into(), wsl: opts, ..Session::default() }
}

/// Engine with both a fake WSL backend and a fake local PTY (as on Windows).
fn start_wsl() -> (EngineHandle, FakePty) {
    let pty = FakePty::default();
    let (h, _, _) = start_config(EngineConfig {
        local_pty: Some(Arc::new(pty.clone())),
        wsl: Some(Arc::new(FakeWsl::default())),
        ..EngineConfig::default()
    });
    (h, pty)
}

/// Open a WSL tab, answer its `LocalReady`, and return the tab.
fn open_wsl(h: &EngineHandle, opts: WslOptions) -> TabId {
    open_wsl_session(h, wsl_session(opts))
}

fn open_wsl_session(h: &EngineHandle, session: Session) -> TabId {
    h.send(Command::Open { session: Box::new(session) }).unwrap();
    let tab = wait_for(h, |ev| match ev {
        Event::LocalReady { tab, .. } => Some(*tab),
        _ => None,
    });
    h.send(Command::LocalStart { tab, cols: 100, rows: 30, cursor: (0, 0) }).unwrap();
    wait_state(h, tab, |s| *s == ConnState::Connected);
    tab
}

#[test]
fn launch_passes_distribution_user_and_folder() {
    let (h, pty) = start_wsl();
    let opts = WslOptions {
        distribution: Some("Debian".into()),
        user: Some("dev".into()),
        start_folder: Some("/srv/app".into()),
        startup_commands: None,
    };
    open_wsl(&h, opts);
    let args = pty.with(0, |s| s.launch.args.clone()).unwrap();
    assert_eq!(args, ["-d", "Debian", "-u", "dev", "--cd", "/srv/app"]);
    assert!(pty.with(0, |s| s.launch.shell.clone()).unwrap().ends_with("wsl.exe"));
    h.shutdown(WAIT);
}

#[test]
fn refused_without_a_wsl_backend() {
    // Linux: a local PTY exists but no WSL backend. macOS: neither backend exists — the WSL
    // diagnostic must still win over "local terminals on this platform".
    for local_pty in [Some(Arc::new(FakePty::default()) as Arc<dyn rxt_platform::LocalPty>), None] {
        let (h, _, _) = start_config(EngineConfig { local_pty, wsl: None, ..EngineConfig::default() });
        h.send(Command::Open { session: Box::new(wsl_session(WslOptions::default())) }).unwrap();
        let tab = wait_for(&h, |ev| match ev {
            Event::TabOpened { tab, .. } => Some(*tab),
            _ => None,
        });
        let state = wait_state(&h, tab, |s| matches!(s, ConnState::Error(_)));
        let ConnState::Error(reason) = state else { unreachable!() };
        assert!(reason.contains("WSL sessions run only on Windows"), "{reason}");
        h.shutdown(WAIT);
    }
}

#[test]
fn startup_commands_typed_at_a_prompt() {
    let (h, pty) = start_wsl();
    let opts = WslOptions { startup_commands: Some("source .venv/bin/activate".into()), ..WslOptions::default() };
    let _tab = open_wsl(&h, opts);
    // No prompt yet: nothing is typed.
    pty.send(b"Last login: Mon\r\n");
    std::thread::sleep(Duration::from_millis(450));
    assert_eq!(pty.with(0, |s| s.input.clone()), Some(Vec::new()));
    // A prompt appears; after the quiet period the startup line is typed with Enter.
    pty.send(b"dev@box:~$ ");
    wait_until(|| pty.with(0, |s| s.input == b"source .venv/bin/activate\r").unwrap_or(false));
    h.shutdown(WAIT);
}

#[test]
fn multiexec_from_another_tab_does_not_reach_wsl() {
    let (h, pty) = start_wsl();
    // A local terminal as the typing source (shell 0), a WSL tab selected alongside it (shell 1).
    h.send(Command::Open { session: Box::new(Session::new(Protocol::Local, "box", None, None)) }).unwrap();
    let local = wait_for(&h, |ev| match ev {
        Event::LocalReady { tab, .. } => Some(*tab),
        _ => None,
    });
    h.send(Command::LocalStart { tab: local, cols: 80, rows: 24, cursor: (0, 0) }).unwrap();
    wait_state(&h, local, |s| *s == ConnState::Connected);
    let wsl = open_wsl(&h, WslOptions::default());
    for tab in [local, wsl] {
        h.send(Command::SelectMultiExec { tab, selected: true }).unwrap();
    }
    h.send(Command::SetMultiExec { enabled: true }).unwrap();
    wait_for(&h, |ev| matches!(ev, Event::MultiExec { enabled: true, selected } if selected.len() == 2).then_some(()));
    // Typing in the local tab is broadcast to the group, but never into the WSL tab (design W4).
    h.send(Command::Input { tab: local, bytes: b"secret\r".to_vec() }).unwrap();
    wait_until(|| pty.with(0, |s| s.input == b"secret\r").unwrap_or(false));
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(pty.with(1, |s| s.input.clone()), Some(Vec::new()), "WSL tab receives no MultiExec input");
    h.shutdown(WAIT);
}

#[test]
fn tab_is_left_out_of_stream_capture() {
    let _one = crate::stream::TEST_LOCK.lock();
    let dir = std::env::temp_dir().join(format!("rxt-engine-wsl-stream-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        crate::stream::configure(Some(crate::stream::StreamConfig { dir: dir.clone(), include_input: true })).is_none()
    );
    let (h, pty) = start_wsl();
    // A distinctive name so the check is specific to this WSL tab (other tabs in parallel tests
    // share the global capture directory).
    let name = format!("wslcap-{}", std::process::id());
    let tab = open_wsl_session(&h, named_wsl_session(&name, WslOptions::default()));
    pty.send(b"hello\r\n");
    let mut out = Vec::new();
    wait_until(|| {
        h.drain_output(tab, usize::MAX, &mut out);
        out.ends_with(b"hello\r\n")
    });
    h.send(Command::Input { tab, bytes: b"ls\r".to_vec() }).unwrap();
    wait_until(|| pty.with(0, |s| s.input == b"ls\r").unwrap_or(false));
    h.shutdown(WAIT);
    crate::stream::shutdown();
    // No stream file was written for the WSL tab (other tabs' files may exist in the shared dir).
    let wsl_files: Vec<_> = std::fs::read_dir(&dir)
        .map(|rd| rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(&name)).collect())
        .unwrap_or_default();
    assert!(wsl_files.is_empty(), "WSL tab must not be captured: {wsl_files:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
