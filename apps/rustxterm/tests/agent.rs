//! M4 agent-control integration (task 9.1). A headless engine runs in this process with the
//! real Unix control socket in a temporary `$XDG_RUNTIME_DIR` and the encrypted store; the real
//! `rustxterm` binary drives it as the CLI and as `rustxterm mcp` over stdio. The test thread
//! plays the GUI: it answers prompts and approvals through the `EngineHandle`.
//!
//! Lab tests (`#[ignore = "lab"]`) need `lab/lab.sh up` (sshd on 127.0.0.1:2222, fictional
//! `alice` account). Other-UID rejection needs two containers: `lab/agent/uid-test.sh`.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{JoinHandle, sleep};
use std::time::{Duration, Instant};

use rxt_core::{ApprovalId, HostKeyDecision, PromptAnswer, PromptKind, Protocol, Session, SessionId, TabId};
use rxt_engine::{Engine, EngineConfig, EngineHandle};
use rxt_platform_linux::UnixControl;
use rxt_store::db::{AgentPolicy, AuditRecord, CredentialKind, Policy};
use rxt_ui::Vault;
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_rustxterm");
const MASTER: &str = "it-master-pass";
const ALICE_PASS: &str = "lab-alice-pass";

/// Temporary HOME/XDG tree.
struct Root(PathBuf);

impl Root {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("rxt-it-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["home", "config", "data", "run"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        Self(root)
    }

    fn data_dir(&self) -> PathBuf {
        self.0.join("data/rustxterm")
    }

    fn runtime(&self) -> PathBuf {
        self.0.join("run/rustxterm")
    }

    fn socket(&self) -> PathBuf {
        self.runtime().join(rxt_platform_linux::SOCKET)
    }

    fn cmd(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.0.join("home"))
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("XDG_DATA_HOME", self.0.join("data"))
            .env("XDG_RUNTIME_DIR", self.0.join("run"));
        c
    }

    /// CLI with stdin not a terminal.
    fn cli(&self, args: &[&str]) -> Output {
        self.cmd(BIN).args(args).stdin(Stdio::null()).output().unwrap()
    }

    /// CLI in the background (it may wait for an approval).
    fn cli_bg(&self, args: &[&str]) -> Child {
        self.cmd(BIN).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap()
    }

    /// CLI on a pseudo-terminal (`script`), typing the master password when prompted.
    /// Returns the exit code and everything the terminal showed.
    fn tty(&self, args: &[&str], password: &str) -> (i32, String) {
        let line = std::iter::once(BIN).chain(args.iter().copied()).map(|a| format!("'{a}'")).collect::<Vec<_>>();
        let mut child = self
            .cmd("script")
            .args(["-qec", &line.join(" "), "/dev/null"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n @ 1..) = stdout.read(&mut buf) {
                if tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        // Keep stdin open until the command ends: script forwards it to the terminal.
        let mut stdin = child.stdin.take().unwrap();
        let mut seen = Vec::new();
        let mut typed = false;
        while let Ok(chunk) = rx.recv_timeout(Duration::from_secs(30)) {
            seen.extend(chunk);
            if !typed && String::from_utf8_lossy(&seen).contains("Master password:") {
                // Let `stty -echo` run first.
                sleep(Duration::from_millis(300));
                stdin.write_all(format!("{password}\n").as_bytes()).unwrap();
                typed = true;
            }
        }
        let status = child.wait().unwrap();
        drop(stdin);
        (status.code().unwrap_or(-1), String::from_utf8_lossy(&seen).into_owned())
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

fn out_json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| {
        panic!("{e}: stdout {:?} stderr {:?}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
    })
}

/// Last JSON line printed on a terminal.
fn last_json(text: &str) -> Value {
    text.lines().rev().find_map(|l| serde_json::from_str(l.trim()).ok()).unwrap_or(Value::Null)
}

fn wait_until(what: &str, timeout: Duration, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        sleep(Duration::from_millis(50));
    }
}

/// An approval request as the GUI sees it.
#[derive(Debug, Clone)]
struct Approval {
    id: ApprovalId,
    command: String,
    session: String,
}

/// Headless engine + store + control socket; a pump thread plays the GUI.
struct Instance {
    root: Root,
    h: EngineHandle,
    vault: Arc<Vault>,
    approvals: mpsc::Receiver<Approval>,
    prompts: Arc<Mutex<Vec<PromptKind>>>,
    stop: Arc<AtomicBool>,
    pump: Option<JoinHandle<()>>,
}

impl Instance {
    fn start(tag: &str, agent_control: bool) -> Self {
        let root = Root::new(tag);
        let vault = Arc::new(Vault::fast(root.data_dir(), Arc::new(rxt_platform::fake::Fake::default())));
        vault.create(MASTER, false).unwrap();
        let config = EngineConfig {
            known_hosts: root.0.join("home/known_hosts"),
            records: Some(vault.clone()),
            control: Some(Arc::new(UnixControl::new(Some(root.runtime())))),
            agent_files: root.0.join("agent-files"),
            ..EngineConfig::default()
        };
        let h = Engine::start(config, Arc::new(|| {})).unwrap();
        if agent_control {
            h.set_agent_control(true).unwrap();
        }
        let (tx, approvals) = mpsc::channel();
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let pump = {
            let (h, prompts, stop) = (h.clone(), prompts.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    while let Some(ev) = h.try_event() {
                        match ev {
                            rxt_core::Event::Prompt { id, kind, .. } => {
                                // Unknown lab host keys are accepted once; any secret prompt is
                                // cancelled (saved credentials must do the login).
                                let answer = match kind {
                                    PromptKind::HostKeyUnknown { .. } => {
                                        PromptAnswer::HostKey(HostKeyDecision::AcceptOnce)
                                    }
                                    _ => PromptAnswer::Cancel,
                                };
                                prompts.lock().unwrap().push(kind);
                                h.send(rxt_core::Command::Answer { prompt: id, answer }).unwrap();
                            }
                            rxt_core::Event::ApprovalRequest { id, command, session, .. } => {
                                let _ = tx.send(Approval { id, command, session });
                            }
                            _ => {}
                        }
                    }
                    sleep(Duration::from_millis(10));
                }
            })
        };
        Self { root, h, vault, approvals, prompts, stop, pump: Some(pump) }
    }

    /// Saved session with a policy; `password` is stored as its saved credential.
    fn session(
        &self,
        name: &str,
        port: u16,
        policy: AgentPolicy,
        content_read: bool,
        password: Option<&str>,
    ) -> Session {
        let mut s = Session::new(Protocol::Ssh, "127.0.0.1", Some("alice".into()), Some(port));
        s.name = name.into();
        s.ssh.use_agent = false;
        self.vault
            .with(|db| {
                s.id = db.save_session(&s)?;
                db.set_policy(s.id, Policy { policy, content_read })?;
                if let Some(pw) = password {
                    db.save_credential(s.id, CredentialKind::Password, &secrecy::SecretString::from(pw))?;
                }
                Ok(())
            })
            .unwrap();
        s
    }

    fn policy(&self, id: SessionId) -> Policy {
        self.vault.with(|db| db.policy(id)).unwrap()
    }

    /// Audit records, oldest first.
    fn audit(&self) -> Vec<AuditRecord> {
        let mut a = self.vault.with(|db| db.audit(0, 1000)).unwrap();
        a.reverse();
        a
    }

    fn cli(&self, args: &[&str]) -> Output {
        self.root.cli(args)
    }

    fn approval(&self) -> Approval {
        self.approvals.recv_timeout(Duration::from_secs(10)).expect("approval request")
    }

    fn tabs(&self) -> Vec<Value> {
        let o = self.cli(&["tabs", "list", "--json"]);
        assert_eq!(code(&o), 0, "{o:?}");
        out_json(&o).as_array().unwrap().clone()
    }

    fn tab_named(&self, name: &str) -> TabId {
        let tabs = self.tabs();
        let t = tabs.iter().find(|t| t["name"] == name).unwrap_or_else(|| panic!("no tab {name}: {tabs:?}"));
        TabId(t["id"].as_u64().unwrap())
    }

    fn wait_connected(&self, tab: TabId) {
        wait_until("connected tab", Duration::from_secs(30), || {
            self.tabs().iter().any(|t| t["id"] == tab.0 && t["state"] == "Connected")
        });
    }

    /// Raw control-channel frame from this (same) UID; `None` when closed without a response.
    fn raw(&self, body: &str) -> Option<Value> {
        let mut s = UnixStream::connect(self.root.socket()).unwrap();
        s.write_all(&(body.len() as u32).to_be_bytes()).unwrap();
        s.write_all(body.as_bytes()).unwrap();
        let mut len = [0u8; 4];
        s.read_exact(&mut len).ok()?;
        let mut buf = vec![0; u32::from_be_bytes(len) as usize];
        s.read_exact(&mut buf).unwrap();
        Some(serde_json::from_slice(&buf).unwrap())
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(p) = self.pump.take() {
            let _ = p.join();
        }
        self.h.shutdown(Duration::from_secs(2));
    }
}

/// `rustxterm mcp` as a child process speaking JSON-RPC over stdio.
struct Mcp {
    child: Child,
    stdin: ChildStdin,
    out: BufReader<ChildStdout>,
    next: u64,
    /// Every line the server wrote.
    transcript: Vec<String>,
}

impl Mcp {
    fn start(root: &Root, client: &str) -> Self {
        let mut child = root
            .cmd(BIN)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let out = BufReader::new(child.stdout.take().unwrap());
        let mut m = Self { child, stdin, out, next: 0, transcript: Vec::new() };
        let init = m.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": client, "version": "1"}}),
        );
        assert_eq!(init["result"]["serverInfo"]["name"], "rustxterm", "{init}");
        m.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        m
    }

    fn send(&mut self, msg: Value) {
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let mut line = String::new();
            assert!(self.out.read_line(&mut line).unwrap() > 0, "MCP server closed stdout");
            self.transcript.push(line.clone());
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == id {
                return v;
            }
        }
    }

    fn tools(&mut self) -> Vec<String> {
        let r = self.request("tools/list", json!({}));
        r["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_owned()).collect()
    }

    /// Raw `tools/call` response.
    fn call(&mut self, name: &str, args: Value) -> Value {
        self.request("tools/call", json!({"name": name, "arguments": args}))
    }

    /// Successful tool result parsed as JSON.
    fn ok(&mut self, name: &str, args: Value) -> Value {
        let r = self.call(name, args);
        assert_ne!(r["result"]["isError"], true, "{name}: {r}");
        let text = r["result"]["content"][0]["text"].as_str().unwrap_or_else(|| panic!("{name}: {r}"));
        serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.into()))
    }

    /// Ends the server; returns its stderr.
    fn finish(mut self) -> String {
        drop(self.stdin);
        let mut err = String::new();
        self.child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
        let _ = self.child.wait();
        err
    }
}

/// 9.1: the real CLI and MCP processes reach the engine only through the socket, MCP lists
/// exactly the matrix columns (no mutation tool; a raw mutation call is rejected and audited),
/// invalid parameters name the parameter and the server keeps running.
#[test]
fn agent_cli_and_mcp_processes_over_real_socket() {
    let it = Instance::start("procs", true);
    let dir_mode = std::fs::metadata(it.root.runtime()).unwrap().permissions().mode() & 0o777;
    let sock_mode = std::fs::symlink_metadata(it.root.socket()).unwrap().permissions().mode() & 0o777;
    assert_eq!((dir_mode, sock_mode), (0o700, 0o600));
    it.session("closed", 22, AgentPolicy::Deny, false, Some("pw-should-stay-hidden"));

    assert_eq!(it.tabs(), Vec::<Value>::new());
    let o = it.cli(&["open", "closed", "--json"]);
    assert_eq!(code(&o), 4, "{o:?}");
    assert_eq!(out_json(&o)["error"]["message"], "denied by policy");
    assert!(it.tabs().is_empty(), "a denied open connects nothing");

    let mut mcp = Mcp::start(&it.root, "it-agent");
    let tools = mcp.tools();
    assert_eq!(tools.len(), 20, "{tools:?}");
    for t in &tools {
        for bad in ["add", "edit", "remove", "import", "approv", "policy", "credential", "password"] {
            assert!(!t.contains(bad), "{t}");
        }
    }
    let list = mcp.ok("list_sessions", json!({}));
    assert_eq!(list[0]["password"], "saved");
    // Mutation and GUI-only tools do not exist; the raw call is still rejected and audited.
    for name in ["add_session", "edit_session", "answer_approval"] {
        let r = mcp.call(name, json!({"session": "closed", "agent_policy": "allow"}));
        assert!(r["error"]["message"].as_str().unwrap().contains(name), "{r}");
    }
    // Invalid parameters: an MCP error naming the parameter; the server keeps running.
    for (tool, args, param) in [
        ("send_input", json!({"text": "uptime\n"}), "`tab`"),
        ("send_input", json!({"tab": "one", "text": "x"}), "`tab`"),
        ("send_input", json!({"tab": 1}), "`text`"),
        ("wait_for_output", json!({"tab": 1, "pattern": "x", "timeout_ms": "soon"}), "`timeout_ms`"),
        ("read_terminal", json!({"tab": -1}), "`tab`"),
        ("open_session", json!({"session": "closed", "agent_policy": "allow"}), "`agent_policy`"),
    ] {
        let r = mcp.call(tool, args);
        let msg = r["error"]["message"].as_str().unwrap_or_else(|| panic!("{tool}: {r}"));
        assert!(msg.contains(param), "{tool}: {msg}");
    }
    assert_eq!(mcp.ok("list_tabs", json!({})), json!([]));
    let r = mcp.call("open_session", json!({"session": "closed"}));
    assert_eq!(r["result"]["isError"], true);
    assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("denied by policy"));
    let stderr = mcp.finish();
    assert!(!stderr.contains("pw-should-stay-hidden"));
    assert!(it.h.agent_control_enabled());

    let audit = it.audit();
    let row = |cmd: &str| audit.iter().filter(|a| a.command == cmd).collect::<Vec<_>>();
    assert!(row("open").iter().any(|a| a.entry_point == "cli-live" && a.decision == "denied"));
    assert!(
        row("open").iter().any(|a| a.entry_point == "mcp-live" && a.client == "it-agent" && a.decision == "denied")
    );
    for name in ["add_session", "edit_session", "answer_approval"] {
        let r = row(name);
        assert_eq!(r.len(), 1, "{name}: {audit:?}");
        assert_eq!((r[0].decision.as_str(), r[0].client.as_str()), ("denied", "it-agent"));
    }
    assert!(row("sessions.list").iter().all(|a| a.entry_point == "mcp-live" && a.decision == "allowed"));
    assert!(!format!("{audit:?}").contains("pw-should-stay-hidden"));
}

/// 9.1: disabled agent control means no socket; live CLI commands exit 5 ("not running, or
/// agent control is off"); MCP falls back to the offline column. Enabling/disabling at runtime
/// creates and removes the socket.
#[test]
fn agent_control_disabled_has_no_socket() {
    let it = Instance::start("off", false);
    assert!(!it.root.socket().exists());
    for args in [&["tabs", "list", "--json"][..], &["open", "x", "--json"], &["send", "1", "id", "--json"]] {
        let o = it.cli(args);
        assert_eq!(code(&o), 5, "{args:?}");
        let err = &out_json(&o)["error"];
        assert_eq!(err["code"], "not_running");
        assert!(err["message"].as_str().unwrap().contains("agent control is off"), "{err}");
    }
    let mut mcp = Mcp::start(&it.root, "off-agent");
    assert_eq!(mcp.tools().len(), 5, "offline column only");
    mcp.finish();

    it.h.set_agent_control(true).unwrap();
    assert!(it.root.socket().exists());
    assert_eq!(code(&it.cli(&["tabs", "list"])), 0);
    it.h.set_agent_control(false).unwrap();
    wait_until("socket removal", Duration::from_secs(5), || !it.root.socket().exists());
    assert_eq!(code(&it.cli(&["tabs", "list"])), 5);
    // Nothing reached the engine while disabled; the one request in between was audited.
    assert_eq!(it.audit().len(), 1);
}

/// 9.1: raw socket requests for GUI-only commands (approval answers, policy and credential
/// changes, `gui` entry point), store changes over the live channel and a policy escalation
/// inside an edit are rejected, audited as denied, and change nothing.
#[test]
fn raw_socket_gui_only_and_policy_escalation_denied() {
    let it = Instance::start("raw", true);
    let s = it.session("prod", 22, AgentPolicy::Deny, false, None);
    let frames = [
        r#"{"id":1,"client":"evil","entry":"cli-live","op":"answer_approval","approval":1,"approve":true}"#,
        r#"{"id":2,"client":"evil","entry":"mcp-live","op":"gui_only","name":"approval.answer"}"#,
        r#"{"id":3,"client":"evil","entry":"mcp-live","op":"gui_only","name":"policy.set"}"#,
        r#"{"id":4,"client":"evil","entry":"cli-live","op":"gui_only","name":"credential.set"}"#,
        r#"{"id":5,"client":"evil","entry":"gui","op":"list_tabs"}"#,
        r#"{"id":6,"client":"evil","entry":"cli-offline","op":"remove_session","session":"prod"}"#,
        r#"{"id":7,"client":"evil","entry":"cli-live","op":"edit_session","session":"prod","patch":{"port":2200}}"#,
        r#"{"id":8,"client":"evil","entry":"mcp-live","op":"edit_session","session":"prod","patch":{"port":2200,"agent_policy":"allow"}}"#,
        r#"{"id":9,"client":"evil","entry":"mcp-live","op":"edit_session","session":"prod","patch":{"password":"x"}}"#,
        r#"{"id":10,"client":"evil","entry":"mcp-live","op":"add_session","url":"ssh://h","patch":{}}"#,
        r#"{"id":11,"client":"evil","entry":"cli-live","op":"set_policy","session":"prod","policy":"allow"}"#,
    ];
    for f in frames {
        let r = it.raw(f).expect("same-UID requests get a response");
        assert_eq!(r["status"], "err", "{f}: {r}");
        assert!(matches!(r["code"].as_str(), Some("denied" | "usage")), "{f}: {r}");
    }
    let after = it.vault.with(|db| db.session(s.id)).unwrap();
    assert_eq!(after.port, 22, "the whole edit was rejected");
    assert_eq!(it.policy(s.id), Policy { policy: AgentPolicy::Deny, content_read: false });
    assert!(it.tabs().is_empty());
    let audit = it.audit();
    let denied: Vec<&str> = audit.iter().filter(|a| a.decision == "denied").map(|a| a.command.as_str()).collect();
    assert_eq!(denied.len(), frames.len(), "{audit:?}");
    assert!(audit.iter().filter(|a| a.decision == "denied").all(|a| a.client == "evil"));
    for cmd in ["approval.answer", "policy.set", "credential.set", "sessions.remove", "sessions.edit", "sessions.add"] {
        assert!(denied.contains(&cmd), "{cmd}: {denied:?}");
    }
}

/// 9.1: `read`, `screenshot` and file operations on an `allow` session without the
/// content-read flag are denied and audited.
#[test]
fn content_read_denied_without_flag() {
    let it = Instance::start("content", true);
    // Port 1: nothing listens; the tab exists (connection error) and is enough for authorization.
    it.session("noread", 1, AgentPolicy::Allow, false, None);
    let o = it.cli(&["open", "noread", "--json"]);
    assert_eq!(code(&o), 0, "{o:?}");
    let tab = out_json(&o)["tab"].as_u64().unwrap().to_string();
    for args in [
        &["read", &tab, "--json"][..],
        &["screenshot", &tab, "--json"],
        &["files", "ls", &tab, "--json"],
        &["files", "get", &tab, "/etc/hostname", "h", "--json"],
    ] {
        let o = it.cli(args);
        assert_eq!(code(&o), 4, "{args:?}: {o:?}");
        assert!(out_json(&o)["error"]["message"].as_str().unwrap().contains("content read"), "{o:?}");
    }
    let mut mcp = Mcp::start(&it.root, "reader");
    let r = mcp.call("read_terminal", json!({"tab": tab.parse::<u64>().unwrap()}));
    assert_eq!(r["result"]["isError"], true, "{r}");
    mcp.finish();
    let denied = it.audit().into_iter().filter(|a| a.decision == "denied").count();
    assert_eq!(denied, 5);
}

/// 9.1: offline owner changes on an interactive terminal (pseudo-terminal via `script`) apply
/// to `deny` and `ask` sessions without any approval and are audited as `cli-offline`; new
/// sessions default to `deny` without content read; no policy flag exists (exit 2); without a
/// terminal the change is denied (exit 4) and nothing changes.
#[test]
fn offline_owner_changes_need_a_terminal() {
    let it = Instance::start("tty", false);
    let deny = it.session("deny-s", 22, AgentPolicy::Deny, false, None);
    let ask = it.session("ask-s", 22, AgentPolicy::Ask, false, None);

    for (name, port) in [("deny-s", "2200"), ("ask-s", "2201")] {
        let (rc, out) = it.root.tty(&["sessions", "edit", name, "--port", port, "--json"], MASTER);
        assert_eq!(rc, 0, "{out}");
        assert_eq!(last_json(&out)["name"], name, "{out}");
    }
    let port = |id| it.vault.with(|db| db.session(id)).unwrap().port;
    assert_eq!((port(deny.id), port(ask.id)), (2200, 2201));
    assert_eq!(it.policy(deny.id).policy, AgentPolicy::Deny, "owner edits never touch the policy");
    assert_eq!(it.policy(ask.id).policy, AgentPolicy::Ask);
    assert!(it.approvals.try_recv().is_err(), "no approval prompt");

    let (rc, out) = it.root.tty(&["sessions", "add", "ssh://bob@new.lab:2222", "--name", "new", "--json"], MASTER);
    assert_eq!(rc, 0, "{out}");
    let id = SessionId(last_json(&out)["id"].as_u64().unwrap());
    assert_eq!(it.policy(id), Policy { policy: AgentPolicy::Deny, content_read: false });

    // No way to set a policy (or other GUI-only field) from the CLI: usage error before unlock.
    for args in [
        &["sessions", "edit", "ask-s", "--agent-policy", "allow"][..],
        &["sessions", "edit", "ask-s", "--content-read"],
        &["sessions", "edit", "ask-s", "--password", "x"],
        &["sessions", "add", "ssh://h", "--agent-policy", "allow"],
    ] {
        let (rc, out) = it.root.tty(args, MASTER);
        assert_eq!(rc, 2, "{args:?}: {out}");
        assert!(!out.contains("Master password"), "rejected before the password prompt");
    }
    assert_eq!(it.policy(ask.id).policy, AgentPolicy::Ask);

    // A wrong master password changes nothing.
    let (rc, _) = it.root.tty(&["sessions", "remove", "deny-s", "--json"], "not-the-master");
    assert_eq!(rc, 3);
    // Without a terminal: denied, nothing changes, nothing is audited.
    let o = it.cli(&["sessions", "remove", "deny-s", "--json"]);
    assert_eq!(code(&o), 4);
    assert_eq!(out_json(&o)["error"]["code"], "denied");
    assert!(it.vault.with(|db| db.session(deny.id)).is_ok());

    let audit = it.audit();
    assert_eq!(audit.len(), 3, "{audit:?}");
    for a in &audit {
        assert_eq!(
            (a.entry_point.as_str(), a.decision.as_str(), a.result.as_str()),
            ("cli-offline", "allowed", "success")
        );
    }
    assert_eq!(
        audit.iter().map(|a| a.command.as_str()).collect::<Vec<_>>(),
        ["sessions.edit", "sessions.edit", "sessions.add"]
    );
    assert!(!format!("{audit:?}").contains(MASTER));
}

/// Open an `allow`/`content_read` lab SSH tab named `name` through MCP and wait until connected.
fn mcp_open(it: &Instance, mcp: &mut Mcp, name: &str) -> u64 {
    let tab = mcp.ok("open_session", json!({"session": name}))["tab"].as_u64().unwrap();
    it.wait_connected(TabId(tab));
    tab
}

/// 9.1: MCP `send_input` + `wait_for_output` on an `allow` lab SSH tab returns the matching text.
#[test]
#[ignore = "lab"]
fn agent_lab_mcp_send_input_and_wait_for_output() {
    let it = Instance::start("mcp-lab", true);
    it.session("lab-alice", 2222, AgentPolicy::Allow, true, Some(ALICE_PASS));
    let mut mcp = Mcp::start(&it.root, "lab-agent");
    let tab = mcp_open(&it, &mut mcp, "lab-alice");
    assert_eq!(mcp.ok("send_input", json!({"tab": tab, "text": "uptime\n"}))["sent_to"], json!([tab]));
    let r = mcp.ok("wait_for_output", json!({"tab": tab, "pattern": "load average", "timeout_ms": 10_000}));
    assert_eq!(r["matched"], true);
    assert!(r["text"].as_str().unwrap().contains("load average"), "{r}");
    let read = mcp.ok("read_terminal", json!({"tab": tab, "lines": 5}));
    assert!(read["text"].as_str().unwrap().contains("load average"), "{read}");
    assert_eq!(mcp.ok("close_tab", json!({"tab": tab}))["closed"], tab);
    let stderr = mcp.finish();
    assert!(!stderr.contains(ALICE_PASS) && !audit_contains(&it, ALICE_PASS));
    // Only the host-key prompt reached the GUI: the saved password logged in.
    assert!(it.prompts.lock().unwrap().iter().all(|p| matches!(p, PromptKind::HostKeyUnknown { .. })));
}

fn audit_contains(it: &Instance, needle: &str) -> bool {
    format!("{:?}", it.audit()).contains(needle)
}

/// 9.1: with MultiExec on a group holding an `allow` and a `deny` lab tab, agent input reaches
/// only the `allow` tab, while GUI input still reaches both.
#[test]
#[ignore = "lab"]
fn agent_lab_multiexec_reaches_only_allow_tabs() {
    let it = Instance::start("multi-lab", true);
    it.session("multi-allow", 2222, AgentPolicy::Allow, true, Some(ALICE_PASS));
    let deny = it.session("multi-deny", 2222, AgentPolicy::Deny, false, Some(ALICE_PASS));
    let o = it.cli(&["open", "multi-allow", "--json"]);
    assert_eq!(code(&o), 0, "{o:?}");
    let allow = TabId(out_json(&o)["tab"].as_u64().unwrap());
    it.h.send(rxt_core::Command::Open { session: Box::new(deny) }).unwrap(); // GUI path
    wait_until("deny tab", Duration::from_secs(10), || it.tabs().iter().any(|t| t["name"] == "multi-deny"));
    let denied = it.tab_named("multi-deny");
    it.wait_connected(allow);
    it.wait_connected(denied);
    it.h.send(rxt_core::Command::SetMultiExec { enabled: true }).unwrap();
    for t in [allow, denied] {
        it.h.send(rxt_core::Command::SelectMultiExec { tab: t, selected: true }).unwrap();
    }
    let mut seen = Vec::new();
    it.h.drain_output(denied, usize::MAX, &mut seen);

    let (a, d) = (allow.0.to_string(), denied.0.to_string());
    let o = it.cli(&["send", &a, "echo AGENT_$((40+2))", "--enter", "--json"]);
    assert_eq!(out_json(&o)["sent_to"], json!([allow.0]));
    assert_eq!(code(&it.cli(&["wait", &a, "AGENT_42", "--timeout", "10s"])), 0);
    // The GUI types into the same group: both tabs run it.
    it.h.send(rxt_core::Command::Input { tab: allow, bytes: b"echo GUI_$((40+3))\n".to_vec() }).unwrap();
    assert_eq!(code(&it.cli(&["wait", &a, "GUI_43", "--timeout", "10s"])), 0);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !String::from_utf8_lossy(&seen).contains("GUI_43") && Instant::now() < deadline {
        it.h.drain_output(denied, usize::MAX, &mut seen);
        sleep(Duration::from_millis(50));
    }
    let text = String::from_utf8_lossy(&seen);
    assert!(text.contains("GUI_43"), "GUI MultiExec input reaches the deny tab: {text}");
    assert!(!text.contains("AGENT_"), "agent input never reaches the deny tab: {text}");
    // The deny tab itself stays out of reach.
    assert_eq!(code(&it.cli(&["send", &d, "id", "--enter"])), 4);
}

/// 9.1: under `ask`, agent input waits for the GUI approval; reusing a consumed approval or
/// sending changed parameters needs a new approval, and a denied one sends nothing; all audited.
#[test]
#[ignore = "lab"]
fn agent_lab_ask_approval_replay_and_changed_params() {
    let it = Instance::start("ask-lab", true);
    let s = it.session("ask-alice", 2222, AgentPolicy::Ask, false, Some(ALICE_PASS));
    it.h.send(rxt_core::Command::Open { session: Box::new(s) }).unwrap();
    wait_until("ask tab", Duration::from_secs(10), || it.tabs().iter().any(|t| t["name"] == "ask-alice"));
    let tab = it.tab_named("ask-alice");
    it.wait_connected(tab);
    let t = tab.0.to_string();

    let child = it.root.cli_bg(&["send", &t, "echo ASK_$((1+1))", "--enter", "--json"]);
    let first = it.approval();
    assert_eq!((first.command.as_str(), first.session.as_str()), ("send", "ask-alice"));
    it.h.send(rxt_core::Command::AnswerApproval { id: first.id, approve: true }).unwrap();
    let o = child.wait_with_output().unwrap();
    assert_eq!(code(&o), 0, "{o:?}");
    // `wait` only reads (no approval under ask); without content read the text is withheld.
    let o = it.cli(&["wait", &t, "ASK_2", "--timeout", "10s", "--json"]);
    assert_eq!(out_json(&o), json!({"matched": true, "text": ""}));

    // Changed parameters: a new request, a new approval. Replaying the consumed approval id
    // approves nothing; denying the new one sends nothing.
    let child = it.root.cli_bg(&["send", &t, "echo CHANGED_$((2+2))", "--enter", "--json"]);
    let second = it.approval();
    assert_ne!(first.id, second.id);
    it.h.send(rxt_core::Command::AnswerApproval { id: first.id, approve: true }).unwrap();
    sleep(Duration::from_millis(300));
    it.h.send(rxt_core::Command::AnswerApproval { id: second.id, approve: false }).unwrap();
    let o = child.wait_with_output().unwrap();
    assert_eq!(code(&o), 4, "{o:?}");
    assert_eq!(code(&it.cli(&["wait", &t, "CHANGED_4", "--timeout", "2s"])), 1, "nothing was sent");

    // Replay of the identical, already approved request: again a new approval.
    let child = it.root.cli_bg(&["send", &t, "echo ASK_$((1+1))", "--enter"]);
    let third = it.approval();
    assert!(third.id != first.id && third.id != second.id);
    it.h.send(rxt_core::Command::AnswerApproval { id: third.id, approve: false }).unwrap();
    assert_eq!(child.wait_with_output().unwrap().status.code(), Some(4));

    let sends: Vec<(String, String)> =
        it.audit().into_iter().filter(|a| a.command == "send").map(|a| (a.decision, a.result)).collect();
    assert_eq!(sends.len(), 3, "{sends:?}");
    assert_eq!(sends[0], ("approved".into(), "success".into()));
    for (decision, result) in &sends[1..] {
        assert_eq!(decision, "denied");
        assert!(result.contains("not approved"), "{result}");
    }
}

/// 9.1: an approval left unanswered expires after 60 s; the request is denied and audited and
/// a late approval opens nothing. (In the lab set because it takes a minute.)
#[test]
#[ignore = "lab"]
fn agent_approval_expires_after_ttl() {
    let it = Instance::start("expire", true);
    it.session("ask-later", 1, AgentPolicy::Ask, false, None);
    let started = Instant::now();
    let child = it.root.cli_bg(&["open", "ask-later", "--json"]);
    let a = it.approval();
    assert_eq!(a.command, "open");
    let o = child.wait_with_output().unwrap();
    assert_eq!(code(&o), 4, "{o:?}");
    assert!(started.elapsed() >= Duration::from_secs(59), "{:?}", started.elapsed());
    it.h.send(rxt_core::Command::AnswerApproval { id: a.id, approve: true }).unwrap();
    sleep(Duration::from_millis(300));
    assert!(it.tabs().is_empty(), "late approval opens nothing");
    let audit = it.audit();
    let open = audit.iter().find(|a| a.command == "open").unwrap();
    assert_eq!(open.decision, "denied");
    assert!(open.result.contains("expired"), "{open:?}");
}

/// 9.1: after a failed login with a wrong saved password, no MCP/CLI output, error, stderr or
/// audit record contains the password.
#[test]
#[ignore = "lab"]
fn agent_lab_failed_login_discloses_no_secret() {
    const WRONG: &str = "wrong-Saved-pw-9917";
    let it = Instance::start("leak-lab", true);
    let s = it.session("lab-wrong", 2222, AgentPolicy::Allow, true, Some(WRONG));
    let mut mcp = Mcp::start(&it.root, "leak-agent");
    let shown = mcp.ok("show_session", json!({"session": "lab-wrong"}));
    assert_eq!(shown["password"], "saved");
    let tab = mcp.ok("open_session", json!({"session": "lab-wrong"}))["tab"].as_u64().unwrap();
    // The saved password is rejected; the retry prompt is cancelled by the "user".
    wait_until("login failure", Duration::from_secs(30), || {
        it.tabs().iter().any(|t| t["id"] == tab && t["state"] != "Connecting" && t["state"] != "Connected")
    });
    assert!(
        it.prompts.lock().unwrap().iter().any(|p| matches!(p, PromptKind::Password { retry: true, .. })),
        "{:?}",
        it.prompts.lock().unwrap()
    );
    let mut outputs = vec![mcp.call("list_tabs", json!({})).to_string()];
    outputs.push(mcp.call("read_terminal", json!({"tab": tab})).to_string());
    outputs.push(mcp.call("send_input", json!({"tab": tab, "text": "id\n"})).to_string());
    outputs.push(mcp.call("wait_for_output", json!({"tab": tab, "pattern": "x", "timeout_ms": 100})).to_string());
    outputs.push(mcp.call("list_sessions", json!({})).to_string());
    outputs.push(mcp.call("export", json!({})).to_string());
    outputs.extend(mcp.transcript.clone());
    outputs.push(mcp.finish());
    for args in
        [&["tabs", "list", "--json"][..], &["read", &tab.to_string(), "--json"], &["sessions", "show", "lab-wrong"]]
    {
        let o = it.cli(args);
        outputs.push(String::from_utf8_lossy(&o.stdout).into_owned());
        outputs.push(String::from_utf8_lossy(&o.stderr).into_owned());
    }
    let audit = it.audit();
    assert!(audit.iter().any(|a| a.command == "open" && a.decision == "allowed"), "{audit:?}");
    outputs.push(format!("{audit:?}"));
    for text in &outputs {
        assert!(!text.contains(WRONG), "secret leaked: {text}");
    }
    // The rejected saved password was forgotten.
    assert!(!it.vault.with(|db| db.has_credential(s.id, CredentialKind::Password)).unwrap());
}
