//! Agent control (design D14): the single authorization boundary ([`authorize`]), parameter-bound
//! approvals, audit, the control-channel server and the live operations it runs.
//!
//! Rules applied to every agent request, live or offline:
//! - the request's declared entry point must match the path it arrived on (control channel:
//!   `cli-live`/`mcp-live`; [`crate::offline::apply`]: `cli-offline`/`mcp-offline`); `gui` and
//!   GUI-only operations are always rejected;
//! - the fixed matrix [`rxt_core::control::allowed`];
//! - live operations on a session (open, tab operations, tunnels) check the session's agent
//!   policy: `deny` rejects, `ask` asks the GUI for any operation with a write effect, `allow`
//!   runs; content reads (read, screenshot, files) also need the content-read flag. Store reads
//!   (lists, show, export) carry no session content and need no policy; offline owner operations
//!   are exempt from policy (matrix only);
//! - every request, including rejected and malformed ones, is audited; responses, errors and
//!   audit records carry no secrets (credentials appear as `"saved"`/`"absent"`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rxt_core::control::{AgentOp, EntryPoint, ErrorCode, Group, Request, Response, SessionRef, allowed};
use rxt_core::{ApprovalId, Event, Protocol, Session, SessionId, TabId, Tunnel};
use rxt_platform::ControlListener;
use rxt_platform::control::{read_frame, write_message};
use rxt_store::db::{AgentPolicy, AuditRecord, CredentialKind, Policy, Store};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::workspace::{PathError, Workspace};
use crate::{InputMsg, Shared, files};

/// Approval lifetime; unanswered approvals are denied.
pub const APPROVAL_TTL: Duration = Duration::from_secs(60);
/// Longest accepted `wait`.
const MAX_WAIT: Duration = Duration::from_secs(600);
/// Lines returned by `read` without an explicit count.
const DEFAULT_READ_LINES: usize = 100;
/// Serial break length when the agent gives none, and the cap.
pub(crate) const DEFAULT_BREAK_MS: u64 = 250;
const MAX_BREAK_MS: u64 = 5_000;

/// Authorization outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allowed,
    /// Allowed once the user approves it in the GUI (`ask` policy, write effect).
    NeedsApproval,
    Denied(String),
}

/// Error returned to agents; the message never contains secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlError {
    pub code: ErrorCode,
    pub message: String,
}

impl ControlError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }

    fn denied(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Denied, message)
    }
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ControlError {}

impl From<rxt_core::Error> for ControlError {
    fn from(e: rxt_core::Error) -> Self {
        use rxt_core::Error as E;
        let code = match &e {
            E::Invalid(_) | E::NotFound(_) | E::AlreadyExists(_) => ErrorCode::Usage,
            E::PermissionDenied(_) => ErrorCode::Denied,
            _ => ErrorCode::Failed,
        };
        Self::new(code, e.to_string())
    }
}

fn locked() -> ControlError {
    ControlError::new(ErrorCode::Locked, "the session database is locked")
}

/// Entry point label used in audit records (`cli-live`, `mcp-offline`, ...).
pub fn entry_name(entry: EntryPoint) -> &'static str {
    match entry {
        EntryPoint::Gui => "gui",
        EntryPoint::CliLive => "cli-live",
        EntryPoint::CliOffline => "cli-offline",
        EntryPoint::McpLive => "mcp-live",
        EntryPoint::McpOffline => "mcp-offline",
    }
}

/// Whether the operation acts on one session (and so is subject to its policy).
fn targets_session(op: &AgentOp) -> bool {
    matches!(op.group(), Group::Live | Group::ContentRead)
        && !matches!(op, AgentOp::ListTabs | AgentOp::SerialListPorts)
}

/// The authorization boundary. `policy` is the target session's policy for operations that
/// act on a session (`None` there means no such session); other operations ignore it.
pub fn authorize(entry: EntryPoint, op: &AgentOp, policy: Option<Policy>) -> Decision {
    if entry == EntryPoint::Gui || op.group() == Group::GuiOnly {
        return Decision::Denied(format!("{} is GUI-only", op.id()));
    }
    if !allowed(op.group(), entry) {
        return Decision::Denied(format!("{} is not allowed from {}", op.id(), entry_name(entry)));
    }
    if matches!(entry, EntryPoint::CliOffline | EntryPoint::McpOffline) || !targets_session(op) {
        return Decision::Allowed;
    }
    let Some(policy) = policy else { return Decision::Denied("no such session".into()) };
    if policy.policy == AgentPolicy::Deny {
        return Decision::Denied("denied by policy".into());
    }
    if op.group() == Group::ContentRead && !policy.content_read {
        return Decision::Denied("content read is not allowed for this session".into());
    }
    if policy.policy == AgentPolicy::Ask && op.effects().writes() {
        return Decision::NeedsApproval;
    }
    Decision::Allowed
}

/// Session as agent-facing JSON: every non-secret field plus `"password": "saved" | "absent"`.
pub fn session_json(session: &Session, has_saved_password: bool) -> Value {
    let mut v = serde_json::to_value(session).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut v {
        map.insert("password".into(), json!(if has_saved_password { "saved" } else { "absent" }));
    }
    v
}

/// Audit target of an operation: names, tab ids and paths, never input data or credentials.
pub fn describe(op: &AgentOp) -> String {
    let session = |s: &SessionRef| match s {
        SessionRef::Id(id) => format!("session {}", id.0),
        SessionRef::Name(n) => n.clone(),
    };
    match op {
        AgentOp::ListSessions
        | AgentOp::ListFolders
        | AgentOp::ListTunnels
        | AgentOp::Export
        | AgentOp::ListTabs
        | AgentOp::SerialListPorts => String::new(),
        AgentOp::ShowSession { session: s }
        | AgentOp::EditSession { session: s, .. }
        | AgentOp::RemoveSession { session: s }
        | AgentOp::Open { session: s } => session(s),
        AgentOp::AddSession { url, .. } => redact_url(url),
        AgentOp::ImportMobaxtermIni { path } => path.clone(),
        AgentOp::Send { tab, .. }
        | AgentOp::Wait { tab, .. }
        | AgentOp::Close { tab }
        | AgentOp::Read { tab, .. }
        | AgentOp::Screenshot { tab }
        | AgentOp::SerialSendBreak { tab, .. } => format!("tab {}", tab.0),
        AgentOp::SerialSetLines { tab, dtr, rts } => format!("tab {} dtr={dtr:?} rts={rts:?}", tab.0),
        AgentOp::FilesLs { tab, path } => format!("tab {}:{path}", tab.0),
        AgentOp::FilesGet { tab, remote, local } => format!("tab {}:{remote} -> {local}", tab.0),
        AgentOp::FilesPut { tab, local, remote } => format!("{local} -> tab {}:{remote}", tab.0),
        AgentOp::TunnelStart { name } | AgentOp::TunnelStop { name } => name.clone(),
        AgentOp::GuiOnly { name } => name.clone(),
    }
}

/// Drop a `:password` from a URL's user info.
fn redact_url(url: &str) -> String {
    let start = url.find("://").map_or(0, |i| i + 3);
    match url[start..].rfind('@').map(|at| start + at) {
        Some(at) => match url[start..at].find(':') {
            Some(colon) => format!("{}{}", &url[..start + colon], &url[at..]),
            None => url.to_owned(),
        },
        None => url.to_owned(),
    }
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Write one audit record; failures are logged (the request outcome stands).
#[allow(clippy::too_many_arguments)]
pub(crate) fn audit(
    store: &Store,
    client: &str,
    entry: &str,
    command: &str,
    session: Option<SessionId>,
    target: &str,
    decision: &str,
    result: &str,
) {
    let rec = AuditRecord {
        ts: now(),
        client: client.chars().take(128).collect(),
        entry_point: entry.into(),
        command: command.chars().take(128).collect(),
        session: session.filter(|s| s.0 != 0),
        target: target.into(),
        decision: decision.into(),
        result: result.into(),
    };
    if let Err(e) = store.append_audit(&rec) {
        tracing::warn!("cannot write agent audit record: {e}");
    }
}

/// Find a session by id or exact name (ambiguous names are refused).
pub(crate) fn find_session(store: &Store, r: &SessionRef) -> Result<Session, ControlError> {
    match r {
        SessionRef::Id(id) => Ok(store.session(*id)?),
        SessionRef::Name(name) => {
            let mut found = store.sessions()?.into_iter().filter(|s| s.name == *name);
            match (found.next(), found.next()) {
                (Some(s), None) => Ok(s),
                (Some(_), Some(_)) => {
                    Err(ControlError::new(ErrorCode::Usage, format!("several sessions are named {name:?}; use the id")))
                }
                (None, _) => Err(ControlError::new(ErrorCode::Usage, format!("no session named {name:?}"))),
            }
        }
    }
}

fn has_password(store: &Store, id: SessionId) -> bool {
    store.has_credential(id, CredentialKind::Password).unwrap_or(false)
}

fn policy_of(store: &Store, id: SessionId) -> Result<Policy, ControlError> {
    if id.0 == 0 { Ok(Policy::default()) } else { Ok(store.policy(id)?) }
}

/// Store reads shared by live and offline requests; `None` for other operations.
pub(crate) fn store_read(store: &Store, op: &AgentOp) -> Option<Result<Value, ControlError>> {
    let result = (|| match op {
        AgentOp::ListSessions => {
            let list = store.sessions()?;
            Ok(Value::Array(list.iter().map(|s| session_json(s, has_password(store, s.id))).collect()))
        }
        AgentOp::ShowSession { session } => {
            let s = find_session(store, session)?;
            let policy = policy_of(store, s.id)?;
            let mut v = session_json(&s, has_password(store, s.id));
            if let Value::Object(map) = &mut v {
                map.insert("agent_policy".into(), json!(policy.policy));
                map.insert("content_read".into(), json!(policy.content_read));
            }
            Ok(v)
        }
        AgentOp::ListFolders => Ok(json!(store.folders()?)),
        AgentOp::ListTunnels => Ok(json!(store.tunnels()?)),
        AgentOp::Export => {
            let export = rxt_store::export::Export::new(store.folders()?, store.sessions()?, store.tunnels()?);
            Ok(json!(export))
        }
        _ => Err(ControlError::new(ErrorCode::Failed, "")),
    })();
    matches!(
        op,
        AgentOp::ListSessions
            | AgentOp::ShowSession { .. }
            | AgentOp::ListFolders
            | AgentOp::ListTunnels
            | AgentOp::Export
    )
    .then_some(result)
}

/// sha256(command id ‖ canonical params JSON ‖ connection ‖ target). serde_json maps are
/// sorted, so the parameter JSON is canonical.
pub(crate) fn approval_hash(op: &AgentOp, connection: &str, target: &str) -> [u8; 32] {
    let params = serde_json::to_string(op).unwrap_or_default();
    let mut h = Sha256::new();
    for part in [op.id(), params.as_str(), connection, target] {
        h.update(part.as_bytes());
        h.update([0]);
    }
    h.finalize().into()
}

struct Pending {
    hash: [u8; 32],
    expires: Instant,
    approved: bool,
    notify: Option<oneshot::Sender<bool>>,
}

/// Pending approvals: bound to a parameter hash, single-use, expiring.
#[derive(Default)]
pub(crate) struct Approvals {
    next: u64,
    pending: HashMap<ApprovalId, Pending>,
}

impl Approvals {
    pub fn open(&mut self, hash: [u8; 32], now: Instant) -> (ApprovalId, oneshot::Receiver<bool>) {
        self.pending.retain(|_, p| p.expires > now);
        self.next += 1;
        let id = ApprovalId(self.next);
        let (tx, rx) = oneshot::channel();
        self.pending.insert(id, Pending { hash, expires: now + APPROVAL_TTL, approved: false, notify: Some(tx) });
        (id, rx)
    }

    /// GUI answer (`Command::AnswerApproval`).
    pub fn answer(&mut self, id: ApprovalId, approve: bool) {
        if let Some(p) = self.pending.get_mut(&id) {
            p.approved = approve;
            if let Some(tx) = p.notify.take() {
                let _ = tx.send(approve);
            }
        }
    }

    /// Consume an approval: true only once, when approved, unexpired and for the same parameters.
    pub fn take(&mut self, id: ApprovalId, hash: [u8; 32], now: Instant) -> bool {
        self.pending.remove(&id).is_some_and(|p| p.approved && p.hash == hash && now < p.expires)
    }
}

/// What a live request acts on, resolved before authorization.
#[derive(Default)]
struct Ctx {
    session: Option<Session>,
    policy: Option<Policy>,
    tab: Option<TabId>,
    tunnel: Option<Tunnel>,
}

/// Local and WSL terminals are off limits to agents until echo-off masking exists (agent-control
/// spec; WSL gets the local terminal's protections, design W4).
const LOCAL_DENIED: &str = "local terminals are not available to agents";

fn not_local(session: Session) -> Result<Session, ControlError> {
    if matches!(session.protocol, rxt_core::Protocol::Local | rxt_core::Protocol::Wsl) {
        Err(ControlError::denied(LOCAL_DENIED))
    } else {
        Ok(session)
    }
}

fn op_tab(op: &AgentOp) -> Option<TabId> {
    match op {
        AgentOp::Send { tab, .. }
        | AgentOp::Wait { tab, .. }
        | AgentOp::Close { tab }
        | AgentOp::Read { tab, .. }
        | AgentOp::Screenshot { tab }
        | AgentOp::FilesLs { tab, .. }
        | AgentOp::FilesGet { tab, .. }
        | AgentOp::FilesPut { tab, .. }
        | AgentOp::SerialSendBreak { tab, .. }
        | AgentOp::SerialSetLines { tab, .. } => Some(*tab),
        _ => None,
    }
}

impl Shared {
    /// Run `f` on the unlocked store; [`ErrorCode::Locked`] while locked or without records.
    pub(crate) fn with_store<R>(&self, f: impl FnOnce(&Store) -> Result<R, ControlError>) -> Result<R, ControlError> {
        let records = self.config.records.as_ref().ok_or_else(locked)?;
        let mut f = Some(f);
        let mut out = None;
        records
            .store(&mut |s| {
                if let Some(f) = f.take() {
                    out = Some(f(s));
                }
                Ok(())
            })
            .map_err(|_| locked())?;
        out.ok_or_else(locked)?
    }

    fn audit_live(&self, client: &str, entry: EntryPoint, op: &AgentOp, ctx: &Ctx, decision: &str, result: &str) {
        let session = ctx.session.as_ref().map(|s| s.id);
        let _ = self.with_store(|s| {
            audit(s, client, entry_name(entry), op.id(), session, &describe(op), decision, result);
            Ok(())
        });
    }

    fn resolve(&self, op: &AgentOp) -> Result<Ctx, ControlError> {
        if let Some(tab) = op_tab(op) {
            let session = self.tabs.lock().get(&tab).map(|t| t.session.clone());
            let session = session.ok_or_else(|| ControlError::new(ErrorCode::Usage, format!("no tab {}", tab.0)))?;
            let session = not_local(session)?;
            let policy = self.with_store(|s| policy_of(s, session.id))?;
            return Ok(Ctx { session: Some(session), policy: Some(policy), tab: Some(tab), tunnel: None });
        }
        match op {
            AgentOp::Open { session } => self.with_store(|s| {
                let session = not_local(find_session(s, session)?)?;
                let policy = policy_of(s, session.id)?;
                Ok(Ctx { session: Some(session), policy: Some(policy), ..Ctx::default() })
            }),
            AgentOp::TunnelStart { name } | AgentOp::TunnelStop { name } => self.with_store(|s| {
                let tunnel = s
                    .tunnels()?
                    .into_iter()
                    .find(|t| t.name == *name)
                    .ok_or_else(|| ControlError::new(ErrorCode::Usage, format!("no tunnel named {name:?}")))?;
                let session = not_local(s.session(tunnel.via)?)?;
                let policy = policy_of(s, session.id)?;
                Ok(Ctx { session: Some(session), policy: Some(policy), tab: None, tunnel: Some(tunnel) })
            }),
            _ => Ok(Ctx::default()),
        }
    }

    /// Ask the GUI; true once approved and consumed for exactly these parameters.
    async fn approve(&self, client: &str, op: &AgentOp, ctx: &Ctx) -> bool {
        let connection = match (ctx.tab, &ctx.session) {
            (Some(tab), _) => format!("tab:{}", tab.0),
            (None, Some(s)) => format!("session:{}", s.id.0),
            _ => String::new(),
        };
        let target = describe(op);
        let hash = approval_hash(op, &connection, &target);
        let (id, rx) = self.approvals.lock().open(hash, Instant::now());
        self.emit(Event::ApprovalRequest {
            id,
            client: client.chars().take(128).collect(),
            command: op.id().into(),
            session: ctx.session.as_ref().map(|s| s.name.clone()).unwrap_or_default(),
            target,
            expires_at: now() + APPROVAL_TTL.as_secs() as i64,
        })
        .await;
        let answered = matches!(tokio::time::timeout(APPROVAL_TTL, rx).await, Ok(Ok(true)));
        // `take` also removes unanswered (expired) entries.
        let approved = self.approvals.lock().take(id, hash, Instant::now());
        answered && approved
    }

    /// One live (control-channel) request through the boundary.
    pub(crate) async fn agent_request(
        self: &Arc<Self>,
        client: &str,
        entry: EntryPoint,
        op: AgentOp,
    ) -> Result<Value, ControlError> {
        if !matches!(entry, EntryPoint::CliLive | EntryPoint::McpLive) {
            let msg = format!("{} requests are not accepted on the control channel", entry_name(entry));
            self.audit_live(client, entry, &op, &Ctx::default(), "denied", &msg);
            return Err(ControlError::denied(msg));
        }
        if let Decision::Denied(msg) =
            authorize(entry, &op, Some(Policy { policy: AgentPolicy::Allow, content_read: true }))
        {
            // Matrix and GUI-only rejections need no store lookup.
            self.audit_live(client, entry, &op, &Ctx::default(), "denied", &msg);
            return Err(ControlError::denied(msg));
        }
        if let Some(result) = self.with_store(|s| Ok(store_read(s, &op)))? {
            let outcome = match &result {
                Ok(_) => "success".to_owned(),
                Err(e) => e.message.clone(),
            };
            self.audit_live(client, entry, &op, &Ctx::default(), "allowed", &outcome);
            return result;
        }
        let ctx = match self.resolve(&op) {
            Ok(ctx) => ctx,
            Err(e) => {
                self.audit_live(client, entry, &op, &Ctx::default(), "denied", &e.message);
                return Err(e);
            }
        };
        let decision = match authorize(entry, &op, ctx.policy) {
            Decision::Denied(msg) => {
                self.audit_live(client, entry, &op, &ctx, "denied", &msg);
                return Err(ControlError::denied(msg));
            }
            Decision::Allowed => "allowed",
            Decision::NeedsApproval => {
                if !self.approve(client, &op, &ctx).await {
                    let msg = "not approved (denied, expired or parameters changed)";
                    self.audit_live(client, entry, &op, &ctx, "denied", msg);
                    return Err(ControlError::denied(msg));
                }
                "approved"
            }
        };
        let result = self.execute(&op, &ctx).await;
        let (decision, outcome) = match &result {
            Ok(v) => match v.get("skipped").and_then(Value::as_array).filter(|s| !s.is_empty()) {
                Some(skipped) => (decision, format!("success; skipped local terminal tabs {skipped:?}")),
                None => (decision, "success".to_owned()),
            },
            Err(e) if e.code == ErrorCode::Denied => ("denied", e.message.clone()),
            Err(e) => (decision, e.message.clone()),
        };
        self.audit_live(client, entry, &op, &ctx, decision, &outcome);
        result
    }

    async fn execute(self: &Arc<Self>, op: &AgentOp, ctx: &Ctx) -> Result<Value, ControlError> {
        let failed = |m: &str| ControlError::new(ErrorCode::Failed, m);
        match op {
            AgentOp::Open { .. } => {
                let session = ctx.session.clone().ok_or_else(|| failed("no session"))?;
                Ok(json!({ "tab": self.open(session).await.0 }))
            }
            AgentOp::ListTabs => {
                let tabs = self.tabs.lock();
                let list: Vec<Value> = tabs
                    .iter()
                    .map(|(id, t)| {
                        json!({ "id": id.0, "name": t.session.name, "protocol": t.session.protocol, "state": t.state })
                    })
                    .collect();
                Ok(Value::Array(list))
            }
            AgentOp::Send { tab, data } => self.agent_send(*tab, data.as_bytes()),
            AgentOp::Read { tab, lines } => {
                let n = lines.map_or(DEFAULT_READ_LINES, |n| n as usize).min(crate::tail::MAX_LINES);
                let text = self.tail(*tab, |t| t.last(n))?;
                Ok(json!({ "text": text }))
            }
            AgentOp::Wait { tab, pattern, timeout_ms } => {
                let content = ctx.policy.is_some_and(|p| p.content_read);
                self.wait(*tab, pattern, Duration::from_millis(*timeout_ms), content).await
            }
            AgentOp::Screenshot { tab } => self.screenshot(*tab),
            AgentOp::Close { tab } => {
                self.close(*tab).await;
                Ok(json!({ "closed": tab.0 }))
            }
            AgentOp::TunnelStart { name } => {
                let tunnel = ctx.tunnel.clone().ok_or_else(|| failed("no tunnel"))?;
                self.start_tunnel(tunnel, ctx.session.clone().map(Box::new)).await;
                Ok(json!({ "tunnel": name, "state": "starting" }))
            }
            AgentOp::TunnelStop { name } => {
                let stopped = self.stop_tunnel(name.clone()).await;
                Ok(json!({ "tunnel": name, "stopped": stopped }))
            }
            AgentOp::FilesLs { tab, path } => {
                let fs = files::file_ops(self, *tab).await?;
                let dir = if path.is_empty() || path == "~" { fs.home().await? } else { path.clone() };
                let entries = fs.list(&dir).await?;
                Ok(json!({ "path": dir, "entries": entries }))
            }
            AgentOp::FilesGet { tab, remote, local } => self.files_get(*tab, remote, local).await,
            AgentOp::FilesPut { tab, local, remote } => self.files_put(*tab, local, remote).await,
            AgentOp::SerialSendBreak { tab, ms } => {
                let ms = ms.unwrap_or(DEFAULT_BREAK_MS).min(MAX_BREAK_MS);
                self.line_control(*tab)?.send_break(ms).await?;
                Ok(json!({ "tab": tab.0, "break_ms": ms }))
            }
            AgentOp::SerialSetLines { tab, dtr, rts } => {
                self.line_control(*tab)?.set_lines(*dtr, *rts).await?;
                Ok(json!({ "tab": tab.0, "dtr": dtr, "rts": rts }))
            }
            AgentOp::SerialListPorts => Ok(json!(self.list_serial_ports().await?)),
            _ => Err(ControlError::new(ErrorCode::Usage, format!("{} is not a live operation", op.id()))),
        }
    }

    fn tail<R>(&self, tab: TabId, f: impl FnOnce(&crate::tail::Tail) -> R) -> Result<R, ControlError> {
        let out = self.tabs.lock().get(&tab).map(|t| t.out.clone());
        let out = out.ok_or_else(|| ControlError::new(ErrorCode::Usage, format!("no tab {}", tab.0)))?;
        let tail = out.tail.lock();
        Ok(f(&tail))
    }

    /// Agent input: to `tab`, plus MultiExec peers only when their own session policy is `allow`.
    fn agent_send(&self, tab: TabId, bytes: &[u8]) -> Result<Value, ControlError> {
        let (peers, skipped): (Vec<(TabId, SessionId)>, Vec<u64>) = {
            let multi = self.multi.lock();
            let tabs = self.tabs.lock();
            let group: Vec<_> = if multi.enabled && multi.selected.contains(&tab) {
                multi.selected.iter().filter(|t| **t != tab).filter_map(|t| tabs.get(t).map(|x| (*t, x))).collect()
            } else {
                Vec::new()
            };
            let local =
                |x: &crate::Tab| matches!(x.session.protocol, rxt_core::Protocol::Local | rxt_core::Protocol::Wsl);
            (
                group.iter().filter(|(_, x)| !local(x)).map(|(t, x)| (*t, x.session.id)).collect(),
                group.iter().filter(|(_, x)| local(x)).map(|(t, _)| t.0).collect(),
            )
        };
        let mut targets = vec![tab];
        if !peers.is_empty() {
            let allowed = self.with_store(|s| {
                Ok(peers
                    .iter()
                    .filter(|(_, sid)| policy_of(s, *sid).is_ok_and(|p| p.policy == AgentPolicy::Allow))
                    .map(|(t, _)| *t)
                    .collect::<Vec<_>>())
            })?;
            targets.extend(allowed);
        }
        let tabs = self.tabs.lock();
        let mut sent = Vec::new();
        for t in targets {
            let Some(entry) = tabs.get(&t) else { continue };
            let Some(tx) = &entry.input else { continue };
            entry.out.tail.lock().mark();
            if tx.try_send(InputMsg::Bytes(bytes.to_vec())).is_ok() {
                sent.push(t.0);
            }
        }
        if !sent.contains(&tab.0) {
            return Err(ControlError::new(ErrorCode::Failed, "tab is not accepting input"));
        }
        Ok(json!({ "sent_to": sent, "skipped": skipped }))
    }

    async fn wait(&self, tab: TabId, pattern: &str, timeout: Duration, content: bool) -> Result<Value, ControlError> {
        let regex = match pattern.strip_prefix("re:") {
            Some(re) => {
                Some(regex::Regex::new(re).map_err(|e| ControlError::new(ErrorCode::Usage, format!("pattern: {e}")))?)
            }
            None => None,
        };
        let deadline = Instant::now() + timeout.min(MAX_WAIT);
        loop {
            let text = self.tail(tab, |t| t.since_mark())?;
            let hit = match &regex {
                Some(re) => re.is_match(&text),
                None => text.contains(pattern),
            };
            if hit {
                // Matched text is session content: only with the content-read flag.
                let text = if content { text } else { String::new() };
                return Ok(json!({ "matched": true, "text": text }));
            }
            if Instant::now() >= deadline {
                return Err(ControlError::new(ErrorCode::Failed, "timed out waiting for the pattern"));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// RDP/VNC: PNG of the framebuffer (base64). Terminal tabs: the visible text tail.
    fn screenshot(&self, tab: TabId) -> Result<Value, ControlError> {
        let (protocol, frame) = {
            let tabs = self.tabs.lock();
            let t = tabs.get(&tab).ok_or_else(|| ControlError::new(ErrorCode::Usage, format!("no tab {}", tab.0)))?;
            (t.session.protocol, t.frame.clone())
        };
        if !matches!(protocol, Protocol::Rdp | Protocol::Vnc) {
            let text = self.tail(tab, |t| t.last(50))?;
            return Ok(json!({ "format": "text", "text": text }));
        }
        let (width, height, pixels) = {
            let f = frame.lock();
            (f.width, f.height, f.pixels.clone())
        };
        let failed = || ControlError::new(ErrorCode::Failed, "no desktop image yet");
        let image = image::RgbaImage::from_raw(width, height, pixels).filter(|_| width > 0).ok_or_else(failed)?;
        let mut png = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut png, image::ImageFormat::Png)
            .map_err(|e| ControlError::new(ErrorCode::Failed, format!("png: {e}")))?;
        let data = data_encoding::BASE64.encode(png.get_ref());
        Ok(json!({ "format": "png", "width": width, "height": height, "png": data }))
    }

    fn workspace(&self) -> Result<Workspace, ControlError> {
        let root = self.agent_files.lock().clone();
        Workspace::open(&root).map_err(|e| ControlError::new(ErrorCode::Failed, format!("agent workspace: {e}")))
    }

    async fn files_get(&self, tab: TabId, remote: &str, local: &str) -> Result<Value, ControlError> {
        let ws = self.workspace()?;
        // Validate (and claim) the target before any remote read.
        ws.create(local).map_err(path_error)?;
        let tmp = format!(".rxt-partial-{}", self.next_id());
        ws.create(&tmp).map_err(path_error)?;
        let result = async {
            let fs = files::file_ops(self, tab).await?;
            let n = fs.get(remote, &ws.root.join(&tmp), &|_| {}).await?;
            ws.rename(&tmp, local).map_err(path_error)?;
            Ok::<_, ControlError>(n)
        }
        .await;
        match result {
            Ok(n) => Ok(json!({ "local": local, "bytes": n })),
            Err(e) => {
                ws.remove(&tmp);
                ws.remove(local);
                Err(e)
            }
        }
    }

    async fn files_put(&self, tab: TabId, local: &str, remote: &str) -> Result<Value, ControlError> {
        let ws = self.workspace()?;
        let src = ws.open_read(local).map_err(path_error)?;
        // ponytail: FileOps::put takes a path, so the capability-opened file is copied to a
        // private temp name first (double local IO); pass a reader when FileOps gains one.
        let tmp = format!(".rxt-partial-{}", self.next_id());
        let dst = ws.create(&tmp).map_err(path_error)?;
        let result = async {
            let mut dst = tokio::fs::File::from_std(dst);
            tokio::io::copy(&mut tokio::fs::File::from_std(src), &mut dst).await.map_err(rxt_core::Error::from)?;
            tokio::io::AsyncWriteExt::flush(&mut dst).await.map_err(rxt_core::Error::from)?;
            drop(dst);
            let fs = files::file_ops(self, tab).await?;
            Ok::<_, ControlError>(fs.put(&ws.root.join(&tmp), remote, &|_| {}).await?)
        }
        .await;
        ws.remove(&tmp);
        result.map(|n| json!({ "remote": remote, "bytes": n }))
    }
}

fn path_error(e: PathError) -> ControlError {
    match e {
        PathError::Escape(m) => ControlError::denied(m),
        PathError::Io(e) => ControlError::new(ErrorCode::Failed, format!("agent workspace: {e}")),
    }
}

/// Accept connections until `cancel`; each connection is served by its own task.
pub(crate) async fn serve(sh: Arc<Shared>, listener: Box<dyn ControlListener>, cancel: CancellationToken) {
    loop {
        let accepted = tokio::select! {
            () = cancel.cancelled() => break,
            r = listener.accept() => r,
        };
        match accepted {
            Ok(stream) => {
                let (sh, cancel) = (sh.clone(), cancel.clone());
                tokio::spawn(async move {
                    tokio::select! {
                        () = cancel.cancelled() => {}
                        () = connection(sh, stream) => {}
                    }
                });
            }
            Err(e) => {
                tracing::warn!("agent control accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

/// One connection: request, response, until EOF. Oversized or broken frames close it.
async fn connection(sh: Arc<Shared>, mut stream: Box<dyn rxt_platform::ControlStream>) {
    loop {
        let frame = match read_frame(&mut stream).await {
            Ok(Some(frame)) => frame,
            Ok(None) => return,
            Err(e) => return tracing::debug!("agent control connection closed: {e}"),
        };
        let response = match serde_json::from_slice::<Request>(&frame) {
            Ok(req) => {
                let id = req.id;
                match sh.agent_request(&req.client, req.entry, req.op).await {
                    Ok(result) => Response::Ok { id, result },
                    Err(e) => Response::Err { id, code: e.code, message: e.message },
                }
            }
            Err(e) => sh.malformed(&frame, &e.to_string()),
        };
        if write_message(&mut stream, &response).await.is_err() {
            return;
        }
    }
}

impl Shared {
    /// Unparseable request (unknown or GUI-only operation, bad parameters): rejected and audited.
    fn malformed(&self, frame: &[u8], error: &str) -> Response {
        let raw: Value = serde_json::from_slice(frame).unwrap_or(Value::Null);
        let field =
            |k: &str| raw.get(k).and_then(Value::as_str).unwrap_or("unknown").chars().take(64).collect::<String>();
        let id = raw.get("id").and_then(Value::as_u64).unwrap_or(0);
        let (client, entry, command) = (field("client"), field("entry"), field("op"));
        let message = format!("invalid request: {error}");
        let _ = self.with_store(|s| {
            audit(s, &client, &entry, &command, None, "", "denied", "invalid request");
            Ok(())
        });
        Response::Err { id, code: ErrorCode::Usage, message }
    }
}
