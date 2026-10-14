//! Offline agent operations (no running GUI): the CLI/MCP open the store themselves and pass
//! each operation through the same authorization boundary in-process.
//!
//! `cli-offline` store changes are owner operations: the caller must only use
//! [`EntryPoint::CliOffline`] after the master password was typed on an interactive terminal
//! (never keyring, pipes or environment). They are matrix-checked and audited but exempt from
//! session policy and approvals; new sessions get policy `deny` with content-read off.

use std::path::Path;

use rxt_core::control::{AgentOp, EntryPoint, ErrorCode, SessionPatch};
use rxt_core::{Folder, FolderId, Session, SessionId};
use rxt_store::db::{Policy, Store};
use serde_json::{Value, json};

use crate::control::{ControlError, Decision, audit, authorize, describe, entry_name, find_session, store_read};

/// Run one offline operation for `client` (declared name) arriving through `entry`
/// (`CliOffline` or `McpOffline`; anything else is denied). Every call is audited.
pub fn apply(store: &Store, client: &str, entry: EntryPoint, op: &AgentOp) -> Result<Value, ControlError> {
    let target = describe(op);
    let record = |session: Option<SessionId>, decision: &str, result: &str| {
        audit(store, client, entry_name(entry), op.id(), session, &target, decision, result);
    };
    if !matches!(entry, EntryPoint::CliOffline | EntryPoint::McpOffline) {
        let msg = format!("{} requests need the running instance", entry_name(entry));
        record(None, "denied", &msg);
        return Err(ControlError::new(ErrorCode::Denied, msg));
    }
    if let Decision::Denied(msg) = authorize(entry, op, None) {
        record(None, "denied", &msg);
        return Err(ControlError::new(ErrorCode::Denied, msg));
    }
    let result = match store_read(store, op) {
        Some(r) => r.map(|v| (None, v)),
        None => write(store, op),
    };
    match result {
        Ok((session, value)) => {
            record(session, "allowed", "success");
            Ok(value)
        }
        Err(e) => {
            record(None, "allowed", &e.message);
            Err(e)
        }
    }
}

fn usage(msg: impl Into<String>) -> ControlError {
    ControlError::new(ErrorCode::Usage, msg)
}

/// Store changes; returns the affected session (for the audit record) and the JSON result.
fn write(store: &Store, op: &AgentOp) -> Result<(Option<SessionId>, Value), ControlError> {
    match op {
        AgentOp::AddSession { url, patch } => {
            let mut s = rxt_core::quick_connect::parse(url)?;
            if s.user.as_deref().is_some_and(|u| u.contains(':')) {
                return Err(usage("credentials are not accepted in session URLs"));
            }
            patch_session(store, &mut s, patch)?;
            s.id = SessionId(0);
            let id = store.save_session(&s)?;
            store.set_policy(id, Policy::default())?;
            Ok((Some(id), json!({ "id": id.0, "name": s.name })))
        }
        AgentOp::EditSession { session, patch } => {
            let mut s = find_session(store, session)?;
            patch_session(store, &mut s, patch)?;
            let id = store.save_session(&s)?;
            Ok((Some(id), json!({ "id": id.0, "name": s.name })))
        }
        AgentOp::RemoveSession { session } => {
            let s = find_session(store, session)?;
            store.delete_session(s.id)?;
            Ok((Some(s.id), json!({ "removed": s.id.0 })))
        }
        AgentOp::ImportMobaxtermIni { path } => {
            let report = rxt_store::moba_ini::import_file(Path::new(path))?;
            let mut imported = 0;
            for (folder, mut s) in report.sessions {
                s.folder = ensure_folder(store, &folder)?;
                s.id = SessionId(0);
                let id = store.save_session(&s)?;
                store.set_policy(id, Policy::default())?;
                imported += 1;
            }
            let skipped: Vec<Value> =
                report.skipped.iter().map(|k| json!({ "line": k.line, "name": k.name, "reason": k.reason })).collect();
            Ok((None, json!({ "imported": imported, "skipped": skipped })))
        }
        other => Err(usage(format!("{} is not an offline operation", other.id()))),
    }
}

fn patch_session(store: &Store, s: &mut Session, patch: &SessionPatch) -> Result<(), ControlError> {
    if let Some(name) = &patch.name {
        s.name = name.clone();
    }
    if let Some(host) = &patch.host {
        if host.trim().is_empty() || host.chars().any(|c| c.is_whitespace() || c == '/') {
            return Err(usage(format!("invalid host {host:?}")));
        }
        s.host = host.clone();
    }
    if let Some(port) = patch.port {
        s.port = port;
    }
    if let Some(user) = &patch.user {
        s.user = (!user.is_empty()).then(|| user.clone());
    }
    if let Some(folder) = &patch.folder {
        s.folder = ensure_folder(store, folder)?;
    }
    if s.name.trim().is_empty() {
        return Err(usage("session name is empty"));
    }
    Ok(())
}

/// Folder for a `/`-separated path, creating missing levels; `None` for the root.
fn ensure_folder(store: &Store, path: &str) -> Result<Option<FolderId>, ControlError> {
    let mut parent = None;
    for name in path.split('/').filter(|n| !n.is_empty()) {
        let existing = store.folders()?.into_iter().find(|f| f.name == name && f.parent == parent);
        parent = Some(match existing {
            Some(f) => f.id,
            None => store.save_folder(&Folder { id: FolderId(0), name: name.into(), parent, icon: None })?,
        });
    }
    Ok(parent)
}
