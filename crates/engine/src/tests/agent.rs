//! Agent control: authorization boundary, approvals, MultiExec filtering, audit, workspace
//! scope and the control-channel server (fake transport over in-memory pipes).

use std::path::PathBuf;
use std::sync::Mutex as StdMutex;

use rxt_core::control::{AgentOp, EntryPoint, ErrorCode, Group, Request, Response, SessionPatch, SessionRef, allowed};
use rxt_core::{ApprovalId, BoxFut, SessionId};
use rxt_platform::control::{read_message, write_frame, write_message};
use rxt_platform::{ControlListener, ControlStream, ControlTransport};
use rxt_store::db::{AgentPolicy, AuditRecord, CredentialKind, Policy, Store};
use rxt_store::envelope::DbKey;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use zeroize::Zeroizing;

use super::*;
use crate::control::{APPROVAL_TTL, Approvals, ControlError, Decision, approval_hash, authorize};

struct StoreRecords(StdMutex<Store>);

impl Records for StoreRecords {
    fn credential(&self, _: &Session, _: &PromptKind) -> Option<Secret> {
        None
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
    fn store(&self, f: &mut dyn FnMut(&Store) -> Result<()>) -> Result<()> {
        f(&self.0.lock().unwrap())
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("rxt-agent-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn open_store(dir: &std::path::Path) -> Store {
    Store::open(dir, &DbKey(Zeroizing::new([7u8; 32]))).unwrap()
}

struct Lab {
    h: EngineHandle,
    state: Arc<State>,
    records: Arc<StoreRecords>,
    dir: PathBuf,
}

impl Lab {
    fn new(name: &str) -> Self {
        let dir = temp_dir(name);
        let records = Arc::new(StoreRecords(StdMutex::new(open_store(&dir.join("db")))));
        let config = EngineConfig {
            records: Some(records.clone()),
            agent_files: dir.join("agent-files"),
            ..EngineConfig::default()
        };
        let (h, state, _) = start_config(config);
        Self { h, state, records, dir }
    }

    fn store<R>(&self, f: impl FnOnce(&Store) -> R) -> R {
        f(&self.records.0.lock().unwrap())
    }

    /// Saved session `host` (also its name) with the given policy.
    fn session(&self, host: &str, protocol: Protocol, policy: AgentPolicy, content_read: bool) -> Session {
        self.store(|s| {
            let mut session = Session::new(protocol, host, None, None);
            session.name = host.into();
            session.id = s.save_session(&session).unwrap();
            s.set_policy(session.id, Policy { policy, content_read }).unwrap();
            session
        })
    }

    fn block<F: std::future::Future>(&self, f: F) -> F::Output {
        let handle = self.h.runtime.runtime.lock().as_ref().unwrap().handle().clone();
        handle.block_on(f)
    }

    fn req(&self, entry: EntryPoint, op: AgentOp) -> std::result::Result<Value, ControlError> {
        let sh = self.h.shared.clone();
        self.block(async move { sh.agent_request("test-agent", entry, op).await })
    }

    /// Run a request in the background (it may wait for an approval).
    fn spawn_req(&self, op: AgentOp) -> std::thread::JoinHandle<std::result::Result<Value, ControlError>> {
        let sh = self.h.shared.clone();
        let handle = self.h.runtime.runtime.lock().as_ref().unwrap().handle().clone();
        std::thread::spawn(move || {
            handle.block_on(async move { sh.agent_request("test-agent", EntryPoint::McpLive, op).await })
        })
    }

    fn audit(&self) -> Vec<AuditRecord> {
        self.store(|s| s.audit(0, 100).unwrap())
    }

    fn connect_tab(&self, session: &Session) -> TabId {
        let tab = open_session(&self.h, session.clone());
        wait_state(&self.h, tab, |s| *s == ConnState::Connected);
        tab
    }

    fn approval(&self) -> (ApprovalId, String, String) {
        wait_for(&self.h, |ev| match ev {
            Event::ApprovalRequest { id, session, target, .. } => Some((*id, session.clone(), target.clone())),
            _ => None,
        })
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        self.h.shutdown(WAIT);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn denied(r: &std::result::Result<Value, ControlError>) -> bool {
    matches!(r, Err(e) if e.code == ErrorCode::Denied)
}

fn all_ops() -> Vec<AgentOp> {
    let s = || SessionRef::Name("x".into());
    let t = TabId(1);
    vec![
        AgentOp::ListSessions,
        AgentOp::ShowSession { session: s() },
        AgentOp::ListFolders,
        AgentOp::ListTunnels,
        AgentOp::Export,
        AgentOp::AddSession { url: "ssh://h".into(), patch: SessionPatch::default() },
        AgentOp::EditSession { session: s(), patch: SessionPatch::default() },
        AgentOp::RemoveSession { session: s() },
        AgentOp::ImportMobaxtermIni { path: "m.ini".into() },
        AgentOp::Open { session: s() },
        AgentOp::ListTabs,
        AgentOp::Send { tab: t, data: "x".into() },
        AgentOp::Wait { tab: t, pattern: "x".into(), timeout_ms: 1 },
        AgentOp::Close { tab: t },
        AgentOp::TunnelStart { name: "n".into() },
        AgentOp::TunnelStop { name: "n".into() },
        AgentOp::Read { tab: t, lines: None },
        AgentOp::Screenshot { tab: t },
        AgentOp::FilesLs { tab: t, path: "/".into() },
        AgentOp::FilesGet { tab: t, remote: "r".into(), local: "l".into() },
        AgentOp::FilesPut { tab: t, local: "l".into(), remote: "r".into() },
        AgentOp::GuiOnly { name: "approval.answer".into() },
    ]
}

#[test]
fn matrix_per_entry_point() {
    use EntryPoint::*;
    let open = Some(Policy { policy: AgentPolicy::Allow, content_read: true });
    for op in all_ops() {
        for entry in [CliLive, CliOffline, McpLive, McpOffline] {
            let d = authorize(entry, &op, open);
            assert_eq!(d == Decision::Allowed, allowed(op.group(), entry), "{} from {entry:?}: {d:?}", op.id());
        }
        // `gui` is never an agent entry point, and GUI-only ops are rejected everywhere.
        assert!(matches!(authorize(Gui, &op, open), Decision::Denied(_)));
    }
    let gui_only = AgentOp::GuiOnly { name: "policy.set".into() };
    assert_eq!(gui_only.group(), Group::GuiOnly);
    for entry in [CliLive, CliOffline, McpLive, McpOffline] {
        assert!(matches!(authorize(entry, &gui_only, open), Decision::Denied(_)));
    }
}

#[test]
fn policy_content_read_and_effects() {
    let send = AgentOp::Send { tab: TabId(1), data: "x".into() };
    let read = AgentOp::Read { tab: TabId(1), lines: None };
    let p = |policy, content_read| Some(Policy { policy, content_read });
    let live = EntryPoint::McpLive;
    assert_eq!(authorize(live, &send, p(AgentPolicy::Deny, true)), Decision::Denied("denied by policy".into()));
    assert_eq!(authorize(live, &send, p(AgentPolicy::Ask, false)), Decision::NeedsApproval);
    assert_eq!(authorize(live, &send, p(AgentPolicy::Allow, false)), Decision::Allowed);
    assert!(matches!(authorize(live, &read, p(AgentPolicy::Allow, false)), Decision::Denied(_)));
    assert_eq!(authorize(live, &read, p(AgentPolicy::Ask, true)), Decision::Allowed, "reads need no approval");
    assert!(matches!(authorize(live, &send, None), Decision::Denied(_)));
    // Offline owner operations ignore policy.
    let edit = AgentOp::EditSession { session: SessionRef::Id(SessionId(1)), patch: SessionPatch::default() };
    assert_eq!(authorize(EntryPoint::CliOffline, &edit, p(AgentPolicy::Deny, false)), Decision::Allowed);
}

#[test]
fn serial_line_control_follows_session_policy_and_listing_is_local() {
    let p = |policy| Some(Policy { policy, content_read: false });
    let live = EntryPoint::CliLive;
    for op in [
        AgentOp::SerialSendBreak { tab: TabId(1), ms: None },
        AgentOp::SerialSetLines { tab: TabId(1), dtr: Some(true), rts: None },
    ] {
        assert_eq!(authorize(live, &op, p(AgentPolicy::Ask)), Decision::NeedsApproval, "{}", op.id());
        assert_eq!(authorize(live, &op, p(AgentPolicy::Allow)), Decision::Allowed);
        assert!(matches!(authorize(live, &op, p(AgentPolicy::Deny)), Decision::Denied(_)));
        assert!(matches!(authorize(EntryPoint::CliOffline, &op, None), Decision::Denied(_)), "live only");
    }
    // Listing ports targets no session: allowed live without a policy, never offline.
    assert_eq!(authorize(live, &AgentOp::SerialListPorts, None), Decision::Allowed);
    assert!(matches!(authorize(EntryPoint::McpOffline, &AgentOp::SerialListPorts, None), Decision::Denied(_)));
}

#[test]
fn approvals_are_single_use_bound_and_expiring() {
    let op = AgentOp::Send { tab: TabId(3), data: "uptime\n".into() };
    let hash = approval_hash(&op, "tab:3", "tab 3");
    let changed = approval_hash(&AgentOp::Send { tab: TabId(3), data: "rm -rf /\n".into() }, "tab:3", "tab 3");
    assert_ne!(hash, changed);
    assert_ne!(hash, approval_hash(&op, "tab:4", "tab 3"));
    let now = Instant::now();
    let mut a = Approvals::default();

    let (id, _rx) = a.open(hash, now);
    a.answer(id, true);
    assert!(a.take(id, hash, now));
    assert!(!a.take(id, hash, now), "replay");

    let (id, _rx) = a.open(hash, now);
    a.answer(id, true);
    assert!(!a.take(id, changed, now), "changed parameters");

    let (id, _rx) = a.open(hash, now);
    a.answer(id, true);
    assert!(!a.take(id, hash, now + APPROVAL_TTL), "expired");

    let (id, _rx) = a.open(hash, now);
    a.answer(id, false);
    assert!(!a.take(id, hash, now), "denied");
    let (id, _rx) = a.open(hash, now);
    assert!(!a.take(id, hash, now), "unanswered");
}

#[test]
fn deny_policy_blocks_open_and_allow_opens() {
    let lab = Lab::new("open");
    lab.session("denied-host", Protocol::Ssh, AgentPolicy::Deny, false);
    lab.session("open-host", Protocol::Ssh, AgentPolicy::Allow, false);
    let r = lab.req(EntryPoint::McpLive, AgentOp::Open { session: SessionRef::Name("denied-host".into()) });
    assert!(denied(&r), "{r:?}");
    assert_eq!(r.unwrap_err().message, "denied by policy");
    let r = lab.req(EntryPoint::CliLive, AgentOp::Open { session: SessionRef::Name("open-host".into()) }).unwrap();
    let tab = TabId(r["tab"].as_u64().unwrap());
    wait_state(&lab.h, tab, |s| *s == ConnState::Connected);
    let tabs = lab.req(EntryPoint::McpLive, AgentOp::ListTabs).unwrap();
    assert_eq!(tabs.as_array().unwrap().len(), 1, "the denied session never opened: {tabs}");
    assert_eq!(tabs[0]["name"], "open-host");
    assert_eq!(tabs[0]["state"], "Connected");
    let audit = lab.audit();
    assert_eq!(audit.len(), 3);
    assert_eq!((audit[2].decision.as_str(), audit[2].entry_point.as_str()), ("denied", "mcp-live"));
    assert_eq!((audit[1].decision.as_str(), audit[1].result.as_str()), ("allowed", "success"));
}

#[test]
fn ask_waits_for_gui_approval_and_replay_needs_a_new_one() {
    let lab = Lab::new("ask");
    let session = lab.session("ask-host", Protocol::Ssh, AgentPolicy::Ask, false);
    let tab = lab.connect_tab(&session);
    let send = AgentOp::Send { tab, data: "uptime\n".into() };

    let pending = lab.spawn_req(send.clone());
    let (id, name, target) = lab.approval();
    assert_eq!((name.as_str(), target), ("ask-host", format!("tab {}", tab.0)));
    sleep(Duration::from_millis(50));
    assert!(lab.state.writes_to("ask-host").is_empty(), "no byte before approval");
    lab.h.send(Command::AnswerApproval { id, approve: true }).unwrap();
    assert!(pending.join().unwrap().is_ok());
    wait_until(|| lab.state.writes_to("ask-host").len() == 1);

    // The same request again is a new approval; denying it sends nothing.
    let pending = lab.spawn_req(send);
    let (id2, _, _) = lab.approval();
    assert_ne!(id, id2);
    // Re-answering the consumed approval changes nothing.
    lab.h.send(Command::AnswerApproval { id, approve: true }).unwrap();
    lab.h.send(Command::AnswerApproval { id: id2, approve: false }).unwrap();
    assert!(denied(&pending.join().unwrap()));
    sleep(Duration::from_millis(50));
    assert_eq!(lab.state.writes_to("ask-host").len(), 1);
    let decisions: Vec<String> = lab.audit().into_iter().map(|a| a.decision).collect();
    assert_eq!(decisions, ["denied", "approved"]);
}

#[test]
fn content_read_flag_gates_read_and_tail_strips_escapes() {
    let lab = Lab::new("read");
    let plain = lab.session("exit", Protocol::Ssh, AgentPolicy::Allow, false);
    let tab = open_session(&lab.h, plain.clone());
    wait_until(|| lab.h.shared.tabs.lock().get(&tab).is_some_and(|t| t.out.tail.lock().last(1) == "bye"));
    let r = lab.req(EntryPoint::CliLive, AgentOp::Read { tab, lines: None });
    assert!(denied(&r), "{r:?}");
    lab.store(|s| s.set_policy(plain.id, Policy { policy: AgentPolicy::Allow, content_read: true })).unwrap();
    let r = lab.req(EntryPoint::CliLive, AgentOp::Read { tab, lines: Some(5) }).unwrap();
    assert_eq!(r["text"], "bye");
    let r = lab.req(EntryPoint::McpLive, AgentOp::Wait { tab, pattern: "re:^b.e$".into(), timeout_ms: 1000 }).unwrap();
    assert_eq!((r["matched"].as_bool(), r["text"].as_str()), (Some(true), Some("bye")));
    let r = lab.req(EntryPoint::McpLive, AgentOp::Wait { tab, pattern: "nope".into(), timeout_ms: 100 });
    assert!(matches!(r, Err(ControlError { code: ErrorCode::Failed, .. })));
    let shot = lab.req(EntryPoint::McpLive, AgentOp::Screenshot { tab }).unwrap();
    assert_eq!(shot["format"], "text");
}

#[test]
fn desktop_screenshot_is_png() {
    let lab = Lab::new("shot");
    let desk = lab.session("desk", Protocol::Vnc, AgentPolicy::Allow, true);
    let tab = lab.connect_tab(&desk);
    wait_until(|| lab.h.with_framebuffer(tab, |f| f.width == 4).unwrap_or(false));
    let shot = lab.req(EntryPoint::McpLive, AgentOp::Screenshot { tab }).unwrap();
    assert_eq!((shot["format"].as_str(), shot["width"].as_u64()), (Some("png"), Some(4)));
    let png = data_encoding::BASE64.decode(shot["png"].as_str().unwrap().as_bytes()).unwrap();
    assert_eq!(&png[..4], b"\x89PNG");
}

#[test]
fn multiexec_broadcast_reaches_only_allow_tabs() {
    let lab = Lab::new("multi");
    let a = lab.session("alpha", Protocol::Ssh, AgentPolicy::Allow, false);
    let b = lab.session("bravo", Protocol::Ssh, AgentPolicy::Deny, false);
    let c = lab.session("charlie", Protocol::Ssh, AgentPolicy::Allow, false);
    let d = lab.session("delta", Protocol::Ssh, AgentPolicy::Ask, false);
    let tabs: Vec<TabId> = [&a, &b, &c, &d].iter().map(|s| lab.connect_tab(s)).collect();
    lab.h.send(Command::SetMultiExec { enabled: true }).unwrap();
    for t in &tabs {
        lab.h.send(Command::SelectMultiExec { tab: *t, selected: true }).unwrap();
    }
    let r = lab.req(EntryPoint::McpLive, AgentOp::Send { tab: tabs[0], data: "id\n".into() }).unwrap();
    assert_eq!(r["sent_to"].as_array().unwrap().len(), 2, "{r}");
    wait_until(|| lab.state.writes_to("charlie").len() == 1);
    sleep(Duration::from_millis(50));
    assert_eq!(lab.state.writes_to("alpha").len(), 1);
    assert!(lab.state.writes_to("bravo").is_empty());
    assert!(lab.state.writes_to("delta").is_empty());
    // An agent cannot target the deny tab directly either.
    assert!(denied(&lab.req(EntryPoint::McpLive, AgentOp::Send { tab: tabs[1], data: "id\n".into() })));
}

#[test]
fn session_output_and_audit_hold_no_secrets() {
    let lab = Lab::new("secrets");
    let s = lab.session("prod", Protocol::Ssh, AgentPolicy::Allow, false);
    lab.store(|db| db.save_credential(s.id, CredentialKind::Password, &"hunter2-secret".to_owned().into())).unwrap();
    let list = lab.req(EntryPoint::McpLive, AgentOp::ListSessions).unwrap();
    assert_eq!(list[0]["password"], "saved");
    let show = lab.req(EntryPoint::CliLive, AgentOp::ShowSession { session: SessionRef::Name("prod".into()) }).unwrap();
    assert_eq!((show["password"].as_str(), show["agent_policy"].as_str()), (Some("saved"), Some("allow")));
    let export = lab.req(EntryPoint::McpLive, AgentOp::Export).unwrap();
    let audit = lab.audit();
    assert_eq!(audit.len(), 3);
    for text in [list.to_string(), show.to_string(), export.to_string(), format!("{audit:?}")] {
        assert!(!text.contains("hunter2"), "{text}");
    }
}

#[test]
#[cfg(unix)]
fn workspace_escape_is_denied_and_audited() {
    let lab = Lab::new("scope");
    let ftp = lab.session("files", Protocol::Ftp, AgentPolicy::Allow, true);
    let tab = lab.connect_tab(&ftp);
    let ls = lab.req(EntryPoint::McpLive, AgentOp::FilesLs { tab, path: "~".into() }).unwrap();
    assert_eq!(ls["path"], "/home/alice");
    for local in ["../../.ssh/id_ed25519", "/etc/passwd", "a/../../x"] {
        let r = lab.req(EntryPoint::McpLive, AgentOp::FilesPut { tab, local: local.into(), remote: "/tmp/k".into() });
        assert!(denied(&r), "{local}: {r:?}");
        let r = lab.req(EntryPoint::McpLive, AgentOp::FilesGet { tab, remote: "/etc/x".into(), local: local.into() });
        assert!(denied(&r), "{local}: {r:?}");
    }
    let outside = lab.dir.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("id_ed25519"), "key").unwrap();
    std::os::unix::fs::symlink(&outside, lab.dir.join("agent-files/link")).unwrap();
    let r = lab
        .req(EntryPoint::McpLive, AgentOp::FilesPut { tab, local: "link/id_ed25519".into(), remote: "/tmp/k".into() });
    assert!(denied(&r), "{r:?}");
    assert!(!lab.state.fs_calls.lock().unwrap().iter().any(|c| c.contains("/tmp/k")));
    let audit = lab.audit();
    assert_eq!(audit.iter().filter(|a| a.decision == "denied").count(), 7);
}

#[test]
fn offline_owner_changes_and_defaults() {
    let dir = temp_dir("offline");
    let store = open_store(&dir);
    let add = AgentOp::AddSession {
        url: "ssh://bob@db.lab:2222".into(),
        patch: SessionPatch { name: Some("db".into()), folder: Some("lab/dbs".into()), ..SessionPatch::default() },
    };
    let r = crate::offline::apply(&store, "cli", EntryPoint::CliOffline, &add).unwrap();
    let id = SessionId(r["id"].as_u64().unwrap());
    assert_eq!(store.policy(id).unwrap(), Policy { policy: AgentPolicy::Deny, content_read: false });
    assert_eq!(store.folders().unwrap().len(), 2);
    // Owner edit of a deny session applies without prompts.
    let edit = AgentOp::EditSession {
        session: SessionRef::Name("db".into()),
        patch: SessionPatch { port: Some(2200), ..SessionPatch::default() },
    };
    crate::offline::apply(&store, "cli", EntryPoint::CliOffline, &edit).unwrap();
    assert_eq!(store.session(id).unwrap().port, 2200);
    // MCP offline is read-only; live entry points do not run offline.
    assert!(denied(&crate::offline::apply(&store, "mcp", EntryPoint::McpOffline, &edit)));
    assert!(denied(&crate::offline::apply(&store, "cli", EntryPoint::CliLive, &edit)));
    let list = crate::offline::apply(&store, "mcp", EntryPoint::McpOffline, &AgentOp::ListSessions).unwrap();
    assert_eq!(list[0]["password"], "absent");
    let bad = AgentOp::AddSession { url: "ssh://bob:pw123@h".into(), patch: SessionPatch::default() };
    assert!(crate::offline::apply(&store, "cli", EntryPoint::CliOffline, &bad).is_err());
    let audit = store.audit(0, 100).unwrap();
    assert_eq!(audit.len(), 6);
    assert!(audit.iter().all(|a| !a.target.contains("pw123")));
    assert_eq!(audit[5].entry_point, "cli-offline");
    let _ = std::fs::remove_dir_all(dir);
}

/// In-memory transport: `connect` hands one end of a pipe to the listener.
struct PipeControl {
    tx: mpsc::UnboundedSender<tokio::io::DuplexStream>,
    rx: StdMutex<Option<mpsc::UnboundedReceiver<tokio::io::DuplexStream>>>,
    live: Arc<std::sync::atomic::AtomicBool>,
}

struct PipeListener(
    tokio::sync::Mutex<mpsc::UnboundedReceiver<tokio::io::DuplexStream>>,
    Arc<std::sync::atomic::AtomicBool>,
);

impl ControlListener for PipeListener {
    fn accept(&self) -> BoxFut<'_, Result<Box<dyn ControlStream>>> {
        Box::pin(async move {
            match self.0.lock().await.recv().await {
                Some(s) => Ok(Box::new(s) as Box<dyn ControlStream>),
                None => std::future::pending().await,
            }
        })
    }
}

impl Drop for PipeListener {
    fn drop(&mut self) {
        self.1.store(false, Ordering::SeqCst);
    }
}

impl ControlTransport for PipeControl {
    fn endpoint(&self) -> Option<String> {
        None
    }
    fn listen(&self) -> Result<Box<dyn ControlListener>> {
        let rx = self.rx.lock().unwrap().take().ok_or_else(|| Error::AlreadyExists("listening".into()))?;
        self.live.store(true, Ordering::SeqCst);
        Ok(Box::new(PipeListener(tokio::sync::Mutex::new(rx), self.live.clone())))
    }
    fn connect(&self) -> BoxFut<'_, Result<Box<dyn ControlStream>>> {
        let (a, b) = tokio::io::duplex(1 << 16);
        let sent = self.tx.send(b);
        Box::pin(async move {
            sent.map_err(|_| Error::NotFound("not running".into()))?;
            Ok(Box::new(a) as Box<dyn ControlStream>)
        })
    }
}

#[test]
fn control_channel_round_trip_raw_bypass_and_disable() {
    let dir = temp_dir("channel");
    let records = Arc::new(StoreRecords(StdMutex::new(open_store(&dir))));
    let (tx, rx) = mpsc::unbounded_channel();
    let live = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let transport = Arc::new(PipeControl { tx, rx: StdMutex::new(Some(rx)), live: live.clone() });
    let config =
        EngineConfig { records: Some(records.clone()), control: Some(transport.clone()), ..EngineConfig::default() };
    let (h, _, _) = start_config(config);
    assert!(!live.load(Ordering::SeqCst), "no endpoint until enabled");
    h.set_agent_control(true).unwrap();
    h.set_agent_control(true).unwrap();
    assert!(h.agent_control_enabled() && live.load(Ordering::SeqCst));

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let mut c = transport.connect().await.unwrap();
        let req = Request { id: 9, client: "cli".into(), entry: EntryPoint::CliLive, op: AgentOp::ListTabs };
        write_message(&mut c, &req).await.unwrap();
        let resp: Response = read_message(&mut c).await.unwrap().unwrap();
        assert_eq!(resp, Response::Ok { id: 9, result: Value::Array(Vec::new()) });
        // Raw bypass attempts: an approval answer and a policy change are rejected.
        for raw in [
            r#"{"id":10,"client":"evil","entry":"cli-live","op":"answer_approval","id_":1,"approve":true}"#,
            r#"{"id":11,"client":"evil","entry":"mcp-live","op":"gui_only","name":"policy.set"}"#,
            r#"{"id":12,"client":"evil","entry":"gui","op":"list_tabs"}"#,
        ] {
            write_frame(&mut c, raw.as_bytes()).await.unwrap();
            let resp: Response = read_message(&mut c).await.unwrap().unwrap();
            assert!(matches!(resp, Response::Err { code: ErrorCode::Usage | ErrorCode::Denied, .. }), "{resp:?}");
        }
        // An oversized frame closes the connection without a response.
        c.write_all(&((rxt_core::control::MAX_FRAME as u32) + 1).to_be_bytes()).await.unwrap();
        assert!(read_message::<Response, _>(&mut c).await.unwrap().is_none());
    });
    let audit = records.0.lock().unwrap().audit(0, 100).unwrap();
    let denied: Vec<&str> = audit.iter().filter(|a| a.decision == "denied").map(|a| a.command.as_str()).collect();
    assert_eq!(denied, ["tabs.list", "policy.set", "answer_approval"]);

    h.set_agent_control(false).unwrap();
    assert!(!h.agent_control_enabled());
    wait_until(|| !live.load(Ordering::SeqCst));
    h.shutdown(WAIT);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn locked_store_reports_locked() {
    let (h, _, _) = start_config(EngineConfig::default());
    let sh = h.shared.clone();
    let rt = h.runtime.runtime.lock().as_ref().unwrap().handle().clone();
    let r = rt.block_on(async move { sh.agent_request("a", EntryPoint::CliLive, AgentOp::ListSessions).await });
    assert!(matches!(r, Err(ControlError { code: ErrorCode::Locked, .. })));
    h.shutdown(WAIT);
}
