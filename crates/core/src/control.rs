//! Agent control contract (design D14): requests from the CLI and MCP server, the fixed
//! permission matrix and effect classification. Authorization itself lives in the engine.

use serde::{Deserialize, Serialize};

use crate::{SessionId, TabId};

/// Largest accepted control-channel frame.
pub const MAX_FRAME: usize = 1 << 20;

/// Where a request comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntryPoint {
    Gui,
    CliLive,
    CliOffline,
    McpLive,
    McpOffline,
}

/// Agent policy of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentPolicy {
    #[default]
    Deny,
    Ask,
    Allow,
}

/// Session selector: id or exact name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SessionRef {
    Id(SessionId),
    Name(String),
}

/// Editable non-secret session fields for offline CLI changes. GUI-only fields (agent policy,
/// credentials) are not representable; unknown fields in JSON are rejected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SessionPatch {
    pub name: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub folder: Option<String>,
}

/// Every operation reachable by an agent entry point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum AgentOp {
    ListSessions,
    ShowSession {
        session: SessionRef,
    },
    ListFolders,
    ListTunnels,
    Export,
    AddSession {
        url: String,
        patch: SessionPatch,
    },
    EditSession {
        session: SessionRef,
        patch: SessionPatch,
    },
    RemoveSession {
        session: SessionRef,
    },
    ImportMobaxtermIni {
        path: String,
    },
    Open {
        session: SessionRef,
    },
    ListTabs,
    Send {
        tab: TabId,
        data: String,
    },
    Wait {
        tab: TabId,
        pattern: String,
        timeout_ms: u64,
    },
    Close {
        tab: TabId,
    },
    TunnelStart {
        name: String,
    },
    TunnelStop {
        name: String,
    },
    Read {
        tab: TabId,
        lines: Option<u32>,
    },
    Screenshot {
        tab: TabId,
    },
    FilesLs {
        tab: TabId,
        path: String,
    },
    /// Download `remote` into the agent workspace path `local`.
    FilesGet {
        tab: TabId,
        remote: String,
        local: String,
    },
    /// Upload the agent workspace path `local` to `remote`.
    FilesPut {
        tab: TabId,
        local: String,
        remote: String,
    },
    /// Serial line control (`serial.send_break`, default 250 ms).
    SerialSendBreak {
        tab: TabId,
        ms: Option<u64>,
    },
    /// `serial.set_lines`: `None` leaves a line unchanged.
    SerialSetLines {
        tab: TabId,
        dtr: Option<bool>,
        rts: Option<bool>,
    },
    /// `serial.list_ports`: enumerate local serial ports.
    SerialListPorts,
    /// GUI-only operations, representable only so raw requests can be rejected and audited.
    GuiOnly {
        name: String,
    },
}

/// Permission-matrix row of an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Group {
    Read,
    StoreWrite,
    Live,
    ContentRead,
    GuiOnly,
}

/// Local/remote read/write effects; any write needs approval under `ask`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Effects {
    pub local_read: bool,
    pub local_write: bool,
    pub remote_read: bool,
    pub remote_write: bool,
}

impl Effects {
    pub fn writes(&self) -> bool {
        self.local_write || self.remote_write
    }
}

impl AgentOp {
    /// Stable command id used in audit records and MCP tool names.
    pub fn id(&self) -> &str {
        match self {
            Self::ListSessions => "sessions.list",
            Self::ShowSession { .. } => "sessions.show",
            Self::ListFolders => "folders.list",
            Self::ListTunnels => "tunnels.list",
            Self::Export => "export",
            Self::AddSession { .. } => "sessions.add",
            Self::EditSession { .. } => "sessions.edit",
            Self::RemoveSession { .. } => "sessions.remove",
            Self::ImportMobaxtermIni { .. } => "import.mobaxterm_ini",
            Self::Open { .. } => "open",
            Self::ListTabs => "tabs.list",
            Self::Send { .. } => "send",
            Self::Wait { .. } => "wait",
            Self::Close { .. } => "close",
            Self::TunnelStart { .. } => "tunnel.start",
            Self::TunnelStop { .. } => "tunnel.stop",
            Self::Read { .. } => "read",
            Self::Screenshot { .. } => "screenshot",
            Self::FilesLs { .. } => "files.ls",
            Self::FilesGet { .. } => "files.get",
            Self::FilesPut { .. } => "files.put",
            Self::SerialSendBreak { .. } => "serial.send_break",
            Self::SerialSetLines { .. } => "serial.set_lines",
            Self::SerialListPorts => "serial.list_ports",
            Self::GuiOnly { name } => name,
        }
    }

    pub fn group(&self) -> Group {
        match self {
            Self::ListSessions | Self::ShowSession { .. } | Self::ListFolders | Self::ListTunnels | Self::Export => {
                Group::Read
            }
            Self::AddSession { .. }
            | Self::EditSession { .. }
            | Self::RemoveSession { .. }
            | Self::ImportMobaxtermIni { .. } => Group::StoreWrite,
            Self::Open { .. }
            | Self::ListTabs
            | Self::Send { .. }
            | Self::Wait { .. }
            | Self::Close { .. }
            | Self::TunnelStart { .. }
            | Self::TunnelStop { .. }
            | Self::SerialSendBreak { .. }
            | Self::SerialSetLines { .. }
            | Self::SerialListPorts => Group::Live,
            Self::Read { .. }
            | Self::Screenshot { .. }
            | Self::FilesLs { .. }
            | Self::FilesGet { .. }
            | Self::FilesPut { .. } => Group::ContentRead,
            Self::GuiOnly { .. } => Group::GuiOnly,
        }
    }

    pub fn effects(&self) -> Effects {
        let none = Effects::default();
        match self {
            // Serial line control drives a device: a remote write.
            Self::Open { .. }
            | Self::Send { .. }
            | Self::Close { .. }
            | Self::TunnelStart { .. }
            | Self::SerialSendBreak { .. }
            | Self::SerialSetLines { .. } => Effects { remote_write: true, ..none },
            Self::SerialListPorts => Effects { local_read: true, ..none },
            Self::TunnelStop { .. } => Effects { local_write: true, ..none },
            Self::FilesGet { .. } => Effects { remote_read: true, local_write: true, ..none },
            Self::FilesPut { .. } => Effects { local_read: true, remote_write: true, ..none },
            Self::Read { .. } | Self::Screenshot { .. } | Self::FilesLs { .. } | Self::Wait { .. } => {
                Effects { remote_read: true, ..none }
            }
            _ => none,
        }
    }
}

/// The fixed permission matrix (agent-control spec).
pub fn allowed(group: Group, entry: EntryPoint) -> bool {
    use EntryPoint::*;
    match group {
        Group::Read => true,
        Group::StoreWrite => matches!(entry, Gui | CliOffline),
        Group::Live | Group::ContentRead => matches!(entry, Gui | CliLive | McpLive),
        Group::GuiOnly => entry == Gui,
    }
}

/// One control-channel request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    /// Declared client name (not authenticated).
    pub client: String,
    pub entry: EntryPoint,
    #[serde(flatten)]
    pub op: AgentOp,
}

/// Error classes; the CLI maps them to distinct exit codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Usage,
    Locked,
    Denied,
    NotRunning,
    AgentControlOff,
    Failed,
}

/// Control-channel response; `result` is operation-specific JSON (never secrets).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok { id: u64, result: serde_json::Value },
    Err { id: u64, code: ErrorCode, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_matches_spec() {
        use EntryPoint::*;
        let rows = [
            (Group::Read, [true, true, true, true, true]),
            (Group::StoreWrite, [true, false, true, false, false]),
            (Group::Live, [true, true, false, true, false]),
            (Group::ContentRead, [true, true, false, true, false]),
            (Group::GuiOnly, [true, false, false, false, false]),
        ];
        for (g, cols) in rows {
            for (e, want) in [Gui, CliLive, CliOffline, McpLive, McpOffline].into_iter().zip(cols) {
                assert_eq!(allowed(g, e), want, "{g:?} {e:?}");
            }
        }
    }

    #[test]
    fn writes_need_approval_classification() {
        assert!(AgentOp::Send { tab: TabId(1), data: "x".into() }.effects().writes());
        assert!(AgentOp::FilesGet { tab: TabId(1), remote: "a".into(), local: "b".into() }.effects().writes());
        assert!(!AgentOp::Read { tab: TabId(1), lines: None }.effects().writes());
        assert!(!AgentOp::ListSessions.effects().writes());
        assert!(AgentOp::SerialSendBreak { tab: TabId(1), ms: None }.effects().remote_write);
        assert!(AgentOp::SerialSetLines { tab: TabId(1), dtr: Some(true), rts: None }.effects().writes());
        let list = AgentOp::SerialListPorts.effects();
        assert!(list.local_read && !list.writes());
        assert_eq!(AgentOp::SerialListPorts.id(), "serial.list_ports");
    }

    #[test]
    fn patch_rejects_gui_only_fields() {
        assert!(serde_json::from_str::<SessionPatch>(r#"{"port": 2222, "agent_policy": "allow"}"#).is_err());
        let r: Request =
            serde_json::from_str(r#"{"id":1,"client":"t","entry":"mcp-live","op":"send","tab":3,"data":"uptime\n"}"#)
                .unwrap();
        assert_eq!(r.op.id(), "send");
    }
}
