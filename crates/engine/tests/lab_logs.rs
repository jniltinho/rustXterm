//! Task 5.4: debug/trace logs written during SSH, FTP/FTPS, RDP, VNC and Telnet (auto-login)
//! logins through the engine contain no password (plain, hex, base64 or byte-list form). Also the
//! engine-level Telnet reconnect (add-telnet-sessions 2.1). Run with `lab/lab.sh up && lab/lab.sh test`.
//! Credentials are the fictional lab accounts from `lab/README.md`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rxt_core::{
    Command, ConnState, Event, FtpTls, HostKeyDecision, JumpHost, PromptAnswer, PromptKind, Protocol, Secret, Session,
    TabId,
};
use rxt_engine::{Engine, EngineConfig, EngineHandle, Records};

const ALICE_PASS: &str = "lab-alice-pass";
const VNC_PASS: &str = "lab-vnc1";
const TELNETD: u16 = 2323;

/// Saved credentials for Telnet auto-login only (other protocols are answered at the prompt).
struct TelnetRecords;

impl Records for TelnetRecords {
    fn credential(&self, session: &Session, kind: &PromptKind) -> Option<Secret> {
        let telnet = session.protocol == Protocol::Telnet && matches!(kind, PromptKind::Password { .. });
        telnet.then(|| Secret(ALICE_PASS.into()))
    }
    fn save_credential(&self, _: &Session, _: &PromptKind, _: &Secret) {}
    fn forget_credential(&self, _: &Session, _: &PromptKind) {}
    fn host_keys(&self, _: &str, _: u16) -> Vec<String> {
        Vec::new()
    }
    fn save_host_key(&self, _: &str, _: u16, _: &str) {}
    fn certificate(&self, _: &str, _: u16) -> Option<String> {
        None
    }
    fn save_certificate(&self, _: &str, _: u16, _: &str) {}
}

fn engine() -> EngineHandle {
    let config = EngineConfig { records: Some(Arc::new(TelnetRecords)), ..EngineConfig::default() };
    Engine::start(config, Arc::new(|| {})).unwrap()
}

fn telnet_auto_login() -> Session {
    let mut s = session(Protocol::Telnet, Some("alice"), TELNETD);
    s.telnet.auto_login = true;
    s
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn session(protocol: Protocol, user: Option<&str>, port: u16) -> Session {
    let mut s = Session::new(protocol, "127.0.0.1", user.map(Into::into), Some(port));
    s.ssh.use_agent = false;
    s.name = format!("{protocol:?}-{port}");
    s
}

/// Forms of a secret that must never reach a log.
fn forms(secret: &str) -> Vec<String> {
    let b = secret.as_bytes();
    let list = format!("{b:?}");
    vec![
        secret.to_owned(),
        data_encoding::HEXLOWER.encode(b),
        data_encoding::HEXUPPER.encode(b),
        data_encoding::BASE64.encode(b),
        data_encoding::BASE64.encode(b).trim_end_matches('=').to_owned(),
        list.trim_matches(['[', ']']).to_owned(),
    ]
}

#[test]
#[ignore = "lab"]
fn engine_debug_log_has_no_secrets() {
    let capture = Capture::default();
    let writer = capture.clone();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .try_init()
        .unwrap();

    let mut ftps_explicit = session(Protocol::Ftps, Some("alice"), 2121);
    ftps_explicit.ftp.tls = FtpTls::Explicit;
    let mut vnc_via_ssh = session(Protocol::Vnc, None, 5900);
    vnc_via_ssh.host = "vnc".into();
    vnc_via_ssh.vnc.via_ssh =
        Some(JumpHost { host: "127.0.0.1".into(), port: 2222, user: "alice".into(), key_file: None });
    let logins = [
        (session(Protocol::Ssh, Some("alice"), 2222), ALICE_PASS),
        (session(Protocol::Ftp, Some("alice"), 2121), ALICE_PASS),
        (ftps_explicit, ALICE_PASS),
        (session(Protocol::Ftps, Some("alice"), 2990), ALICE_PASS),
        (session(Protocol::Rdp, Some("alice"), 13389), ALICE_PASS),
        (session(Protocol::Vnc, None, 5901), VNC_PASS),
        (vnc_via_ssh, VNC_PASS),
        (telnet_auto_login(), ALICE_PASS),
    ];

    let h = engine();
    let mut pending: HashMap<String, &str> = HashMap::new();
    for (s, pass) in &logins {
        pending.insert(s.name.clone(), pass);
        h.send(Command::Open { session: Box::new(s.clone()) }).unwrap();
    }
    let mut tabs: HashMap<TabId, (String, &str)> = HashMap::new();
    let mut connected = Vec::new();
    let mut failed = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(90);
    while connected.len() + failed.len() < logins.len() {
        assert!(Instant::now() < deadline, "timed out; connected {connected:?}, failed {failed:?}");
        let Some(ev) = h.try_event() else {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        };
        match ev {
            Event::TabOpened { tab, session } => {
                let pass = pending[&session.name];
                tabs.insert(tab, (session.name.clone(), pass));
            }
            Event::Prompt { id, tab, kind } => {
                let (_, pass) = tabs[&tab.expect("login prompts belong to a tab")];
                let answer = match kind {
                    PromptKind::HostKeyUnknown { .. } | PromptKind::CertificateUnknown { .. } => {
                        PromptAnswer::HostKey(HostKeyDecision::AcceptOnce)
                    }
                    // The VNC gateway login of `vnc_via_ssh` uses alice's SSH password.
                    PromptKind::Password { user, .. } if user == "alice" => {
                        PromptAnswer::Secret(Secret(ALICE_PASS.into()))
                    }
                    PromptKind::Password { .. } => PromptAnswer::Secret(Secret(pass.into())),
                    PromptKind::KeyboardInteractive { prompts, .. } => {
                        PromptAnswer::Responses(prompts.iter().map(|_| Secret(pass.into())).collect())
                    }
                    _ => PromptAnswer::Cancel,
                };
                h.send(Command::Answer { prompt: id, answer }).unwrap();
            }
            Event::State { tab, state: ConnState::Connected } => connected.push(tabs[&tab].0.clone()),
            Event::State { tab, state: ConnState::Error(e) } => failed.push(format!("{}: {e}", tabs[&tab].0)),
            _ => {}
        }
    }
    // Let desktops receive their first frames, then end every session (logs from teardown count too).
    std::thread::sleep(Duration::from_secs(2));
    h.shutdown(Duration::from_secs(5));

    eprintln!("connected: {connected:?}\nfailed: {failed:?}");
    let log = String::from_utf8_lossy(&capture.0.lock().unwrap()).into_owned();
    assert!(log.len() > 1000, "the subscriber captured almost nothing: {log}");
    let forms: Vec<String> = [ALICE_PASS, VNC_PASS].into_iter().flat_map(forms).collect();
    let leaks: Vec<String> = log
        .lines()
        .filter(|l| forms.iter().any(|f| l.contains(f.as_str())))
        .map(|l| l.chars().take(300).collect())
        .collect();
    assert!(leaks.is_empty(), "debug log contains secrets ({} lines):\n{}", leaks.len(), leaks.join("\n"));
    assert!(failed.is_empty(), "logins failed: {failed:?}");
}

/// Drain `tab` output until `needle` shows up.
fn wait_output(h: &EngineHandle, tab: TabId, needle: &str, seen: &mut Vec<u8>) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !String::from_utf8_lossy(seen).contains(needle) {
        assert!(Instant::now() < deadline, "no {needle:?} in {:?}", String::from_utf8_lossy(seen));
        while h.try_event().is_some() {}
        if h.drain_output(tab, 1 << 20, seen) == 0 {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Substitute for "reconnect after server restart" (the test container cannot restart lab
/// services): the server ends the connection (`exit`), the engine reconnects automatically and
/// auto-login opens a new shell.
#[test]
#[ignore = "lab"]
fn telnet_engine_auto_login_reconnects_after_server_close() {
    let h = engine();
    let mut s = telnet_auto_login();
    s.ssh.auto_reconnect = true;
    h.send(Command::Open { session: Box::new(s) }).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let tab = loop {
        assert!(Instant::now() < deadline, "no tab");
        match h.try_event() {
            Some(Event::TabOpened { tab, .. }) => break tab,
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    };
    let mut seen = Vec::new();
    wait_output(&h, tab, "$ ", &mut seen);
    h.send(Command::Input { tab, bytes: b"echo rxt-$((40+2))\r".to_vec() }).unwrap();
    wait_output(&h, tab, "rxt-42", &mut seen);
    h.send(Command::Input { tab, bytes: b"exit\r".to_vec() }).unwrap();
    // A new login after the drop: the auto-login prompt answer and a fresh shell.
    let mut after = Vec::new();
    wait_output(&h, tab, "login:", &mut after);
    wait_output(&h, tab, "$ ", &mut after);
    h.send(Command::Input { tab, bytes: b"echo rxt-$((40+3))\r".to_vec() }).unwrap();
    wait_output(&h, tab, "rxt-43", &mut after);
    assert!(!String::from_utf8_lossy(&seen).contains(ALICE_PASS), "password echoed");
    h.shutdown(Duration::from_secs(5));
}
