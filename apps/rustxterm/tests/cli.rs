//! CLI subcommands and exit codes, run against the built binary with temporary XDG dirs.
//! Stdin is never a terminal here, so the TTY-only paths must deny or report locked.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used)]

use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use rxt_core::control::{AgentOp, EntryPoint, ErrorCode, Request, Response};
use rxt_store::db::Store;
use rxt_store::envelope::{self, KdfParams};
use secrecy::SecretString;
use serde_json::{Value, json};

struct Env {
    root: PathBuf,
}

impl Env {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("rxt-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["home", "config", "data", "run"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        Self { root }
    }

    fn data_dir(&self) -> PathBuf {
        self.root.join("data/rustxterm")
    }

    fn socket(&self) -> PathBuf {
        self.root.join("run/rustxterm/control.sock")
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_rustxterm"))
            .args(args)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    /// Database with one session named "prod".
    fn store(&self) -> Store {
        let key =
            envelope::create_with_params(&self.data_dir(), &SecretString::from("pw"), None, KdfParams::MIN).unwrap();
        let store = Store::open(&self.data_dir(), &key).unwrap();
        store.save_session(&rxt_core::Session::new(rxt_core::Protocol::Ssh, "prod.example", None, None)).unwrap();
        let mut s = store.sessions().unwrap().remove(0);
        s.name = "prod".into();
        store.save_session(&s).unwrap();
        store
    }

    /// Answer each connection with the next reply; returns the received requests.
    fn serve(&self, replies: Vec<Response>) -> std::sync::mpsc::Receiver<Request> {
        std::fs::create_dir_all(self.socket().parent().unwrap()).unwrap();
        let listener = UnixListener::bind(self.socket()).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for reply in replies {
                let (mut s, _) = listener.accept().unwrap();
                tx.send(serde_json::from_slice(&read_frame(&mut s)).unwrap()).unwrap();
                write_frame(&mut s, &serde_json::to_vec(&reply).unwrap());
            }
        });
        rx
    }
}

/// Control-channel framing (4-byte big-endian length + JSON), as the platform transport speaks it.
fn read_frame(r: &mut impl Read) -> Vec<u8> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).unwrap();
    let mut body = vec![0; u32::from_be_bytes(len) as usize];
    r.read_exact(&mut body).unwrap();
    body
}

fn write_frame(w: &mut impl Write, body: &[u8]) {
    w.write_all(&(body.len() as u32).to_be_bytes()).unwrap();
    w.write_all(body).unwrap();
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

fn stdout_json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&o.stdout)))
}

#[test]
fn usage_errors_exit_2() {
    let env = Env::new("usage");
    assert_eq!(code(&env.run(&["sessions", "frobnicate"])), 2);
    // There is no way to set agent policy or arbitrary keys from the CLI.
    assert_eq!(code(&env.run(&["sessions", "edit", "prod", "--agent-policy", "allow"])), 2);
    assert_eq!(code(&env.run(&["sessions", "edit", "prod", "--set", "agent_policy=allow"])), 2);
    assert_eq!(code(&env.run(&["wait", "1", "x", "--timeout", "soon"])), 2);
}

#[test]
fn offline_reads_without_terminal_or_keyring_are_locked() {
    let env = Env::new("locked");
    drop(env.store());
    for args in [
        &["sessions", "list"][..],
        &["sessions", "show", "prod"],
        &["folders", "list"],
        &["tunnels", "list"],
        &["export"],
    ] {
        let out = env.run(&[args, &["--json"]].concat());
        assert_eq!(code(&out), 3, "{args:?}");
        assert_eq!(stdout_json(&out)["error"]["code"], "locked");
    }
}

#[test]
fn offline_changes_without_terminal_are_denied_and_change_nothing() {
    let env = Env::new("denied");
    let store = env.store();
    let ini = env.root.join("MobaXterm.ini");
    std::fs::write(&ini, "[Bookmarks]\nSubRep=\n").unwrap();
    let cases: [&[&str]; 4] = [
        &["sessions", "remove", "prod"],
        &["sessions", "add", "ssh://root@new.example"],
        &["sessions", "edit", "prod", "--port", "2222"],
        &["import", "mobaxterm-ini", ini.to_str().unwrap()],
    ];
    for args in cases {
        let out = env.run(&[args, &["--json"]].concat());
        assert_eq!(code(&out), 4, "{args:?}");
        assert_eq!(stdout_json(&out)["error"]["code"], "denied");
    }
    let sessions = store.sessions().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!((sessions[0].name.as_str(), sessions[0].port), ("prod", 22));
    assert!(store.audit(0, 10).unwrap().is_empty());
}

#[test]
fn live_commands_without_instance_are_not_running() {
    let env = Env::new("notrunning");
    let cases: [&[&str]; 13] = [
        &["open", "prod"],
        &["tabs", "list"],
        &["send", "1", "uptime", "--enter"],
        &["read", "1", "--lines", "5"],
        &["wait", "1", "load", "--timeout", "1s"],
        &["screenshot", "1"],
        &["files", "ls", "1"],
        &["files", "get", "1", "/etc/hosts", "hosts"],
        &["files", "put", "1", "hosts", "/tmp/hosts"],
        &["tunnel", "start", "db"],
        &["tunnel", "stop", "db"],
        &["close", "1"],
        &["tabs", "list", "--json"],
    ];
    for args in cases {
        let out = env.run(args);
        assert_eq!(code(&out), 5, "{args:?}");
    }
    let out = env.run(&["close", "1", "--json"]);
    assert_eq!(stdout_json(&out)["error"]["code"], "not_running");
}

#[test]
fn live_requests_are_encoded_and_errors_mapped() {
    let env = Env::new("live");
    let rx = env.serve(vec![
        Response::Ok { id: 1, result: json!({"sent": 7}) },
        Response::Ok { id: 1, result: json!({"text": "load average: 0.00"}) },
        Response::Err { id: 1, code: ErrorCode::Denied, message: "denied by policy".into() },
        Response::Err { id: 1, code: ErrorCode::AgentControlOff, message: "agent control is off".into() },
        Response::Err { id: 1, code: ErrorCode::Failed, message: "boom".into() },
    ]);
    let out = env.run(&["send", "3", "uptime", "--enter", "--json"]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout_json(&out), json!({"sent": 7}));
    let req = rx.recv().unwrap();
    assert_eq!((req.client.as_str(), req.entry), ("rustxterm-cli", EntryPoint::CliLive));
    assert_eq!(req.op, AgentOp::Send { tab: rxt_core::TabId(3), data: "uptime\n".into() });

    let out = env.run(&["wait", "3", "load average", "--timeout", "2s"]);
    assert_eq!(code(&out), 0);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "load average: 0.00\n");
    let req = rx.recv().unwrap();
    assert_eq!(req.op, AgentOp::Wait { tab: rxt_core::TabId(3), pattern: "load average".into(), timeout_ms: 2000 });

    assert_eq!(code(&env.run(&["open", "prod"])), 4);
    assert!(matches!(rx.recv().unwrap().op, AgentOp::Open { .. }));
    assert_eq!(code(&env.run(&["tabs", "list"])), 6);
    assert_eq!(code(&env.run(&["close", "3"])), 1);
}

#[test]
fn self_test_includes_mcp_handshake() {
    let env = Env::new("selftest");
    let out = env.run(&["--self-test"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("ok    mcp handshake"), "{text}");
}
