//! `rustxterm mcp`: stdio MCP server exposing exactly the MCP columns of the permission matrix.
//! Live tools need a running instance with agent control (control channel, entry `mcp-live`);
//! without one only the read-only store tools exist (entry `mcp-offline`, keyring unlock only).
//! There are no session add/edit/remove/import tools; unknown tool calls are MCP errors and,
//! when an instance is reachable, are sent as `GuiOnly` so the engine rejects and audits them.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation, JsonObject, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use rxt_core::TabId;
use rxt_core::control::{self, AgentOp, EntryPoint, ErrorCode};
use serde_json::{Value, json};

use crate::{ControlError, Ctx, client, offline, parse_ref};

#[derive(Clone, Copy)]
enum Kind {
    Str,
    Int,
}

struct Param {
    name: &'static str,
    kind: Kind,
    required: bool,
    desc: &'static str,
}

struct ToolDef {
    name: &'static str,
    desc: &'static str,
    params: &'static [Param],
    /// Needs a running instance (not in the MCP-offline column).
    live_only: bool,
}

const fn p(name: &'static str, kind: Kind, required: bool, desc: &'static str) -> Param {
    Param { name, kind, required, desc }
}

const SESSION: Param = p("session", Kind::Str, true, "Session id or exact name");
const TAB: Param = p("tab", Kind::Int, true, "Tab id from list_tabs");

const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "list_sessions",
        desc: "List saved sessions (credentials shown only as saved/absent)",
        params: &[],
        live_only: false,
    },
    ToolDef { name: "show_session", desc: "Show one saved session", params: &[SESSION], live_only: false },
    ToolDef { name: "list_folders", desc: "List session folders", params: &[], live_only: false },
    ToolDef { name: "list_tunnels", desc: "List saved tunnels", params: &[], live_only: false },
    ToolDef {
        name: "export",
        desc: "Export sessions, folders and tunnels as TOML (no credentials)",
        params: &[],
        live_only: false,
    },
    ToolDef { name: "open_session", desc: "Open a saved session in a new tab", params: &[SESSION], live_only: true },
    ToolDef { name: "list_tabs", desc: "List open tabs", params: &[], live_only: true },
    ToolDef {
        name: "read_terminal",
        desc: "Read a terminal tab's text (needs the session's content-read flag)",
        params: &[TAB, p("lines", Kind::Int, false, "Only the last N lines")],
        live_only: true,
    },
    ToolDef {
        name: "send_input",
        desc: "Send text to a tab; include \\n to press Enter",
        params: &[TAB, p("text", Kind::Str, true, "Text to send")],
        live_only: true,
    },
    ToolDef {
        name: "wait_for_output",
        desc: "Wait until a tab's output matches a pattern and return the matching text",
        params: &[
            TAB,
            p("pattern", Kind::Str, true, "Pattern to wait for"),
            p("timeout_ms", Kind::Int, false, "Timeout in milliseconds (default 10000)"),
        ],
        live_only: true,
    },
    ToolDef { name: "screenshot_tab", desc: "Capture a tab as a PNG image", params: &[TAB], live_only: true },
    ToolDef {
        name: "file_list",
        desc: "List a remote directory of a tab",
        params: &[TAB, p("path", Kind::Str, false, "Remote directory (default .)")],
        live_only: true,
    },
    ToolDef {
        name: "file_get",
        desc: "Download a remote file into the agent workspace",
        params: &[
            TAB,
            p("remote", Kind::Str, true, "Remote file path"),
            p("local", Kind::Str, true, "Path inside the agent workspace"),
        ],
        live_only: true,
    },
    ToolDef {
        name: "file_put",
        desc: "Upload a file from the agent workspace",
        params: &[
            TAB,
            p("local", Kind::Str, true, "Path inside the agent workspace"),
            p("remote", Kind::Str, true, "Remote file path"),
        ],
        live_only: true,
    },
    ToolDef {
        name: "tunnel_start",
        desc: "Start a saved tunnel",
        params: &[p("name", Kind::Str, true, "Tunnel name")],
        live_only: true,
    },
    ToolDef {
        name: "tunnel_stop",
        desc: "Stop a running tunnel",
        params: &[p("name", Kind::Str, true, "Tunnel name")],
        live_only: true,
    },
    ToolDef { name: "close_tab", desc: "Close a tab", params: &[TAB], live_only: true },
    ToolDef { name: "serial_list_ports", desc: "List local serial ports", params: &[], live_only: true },
    ToolDef {
        name: "serial_send_break",
        desc: "Send a break on a serial tab",
        params: &[TAB, p("ms", Kind::Int, false, "Break length in milliseconds (default 250)")],
        live_only: true,
    },
    ToolDef {
        name: "serial_set_lines",
        desc: "Set DTR and/or RTS on a serial tab",
        params: &[TAB, p("dtr", Kind::Int, false, "1 = on, 0 = off"), p("rts", Kind::Int, false, "1 = on, 0 = off")],
        live_only: true,
    },
];

/// `1`/`0` line parameter.
fn arg_on_off(args: &JsonObject, name: &str) -> Result<Option<bool>, ErrorData> {
    match arg_u64(args, name)? {
        None => Ok(None),
        Some(v @ (0 | 1)) => Ok(Some(v == 1)),
        Some(v) => Err(bad(format!("parameter `{name}` must be 0 or 1, got {v}"))),
    }
}

fn schema(params: &[Param]) -> JsonObject {
    let mut props = serde_json::Map::new();
    for prm in params {
        let mut s = match prm.kind {
            Kind::Str => json!({ "type": "string" }),
            Kind::Int => json!({ "type": "integer", "minimum": 0 }),
        };
        s["description"] = prm.desc.into();
        props.insert(prm.name.into(), s);
    }
    let required: Vec<_> = params.iter().filter(|p| p.required).map(|p| p.name).collect();
    let mut o = JsonObject::new();
    o.insert("type".into(), "object".into());
    o.insert("properties".into(), props.into());
    o.insert("required".into(), json!(required));
    o.insert("additionalProperties".into(), false.into());
    o
}

/// Tools for the live (instance reachable) or offline column.
pub fn tools(live: bool) -> Vec<Tool> {
    TOOLS
        .iter()
        .filter(|t| live || !t.live_only)
        .map(|t| Tool::new(t.name, t.desc, Arc::new(schema(t.params))))
        .collect()
}

fn bad(msg: String) -> ErrorData {
    ErrorData::invalid_params(msg, None)
}

fn arg_str(args: &JsonObject, name: &str) -> Result<Option<String>, ErrorData> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(bad(format!("parameter `{name}` must be a string"))),
    }
}

fn arg_u64(args: &JsonObject, name: &str) -> Result<Option<u64>, ErrorData> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            v.as_u64().map(Some).ok_or_else(|| bad(format!("parameter `{name}` must be a non-negative integer")))
        }
    }
}

fn need<T>(v: Option<T>, name: &str) -> Result<T, ErrorData> {
    v.ok_or_else(|| bad(format!("missing required parameter `{name}`")))
}

/// Validate `args` against the tool and build its operation.
fn build_op(def: &ToolDef, args: &JsonObject) -> Result<AgentOp, ErrorData> {
    if let Some(k) = args.keys().find(|k| !def.params.iter().any(|p| p.name == k.as_str())) {
        return Err(bad(format!("unknown parameter `{k}` for tool {}", def.name)));
    }
    let s = |n: &str| arg_str(args, n).and_then(|v| need(v, n));
    let tab = || arg_u64(args, "tab").and_then(|v| need(v, "tab")).map(TabId);
    Ok(match def.name {
        "list_sessions" => AgentOp::ListSessions,
        "show_session" => AgentOp::ShowSession { session: parse_ref(&s("session")?) },
        "list_folders" => AgentOp::ListFolders,
        "list_tunnels" => AgentOp::ListTunnels,
        "export" => AgentOp::Export,
        "open_session" => AgentOp::Open { session: parse_ref(&s("session")?) },
        "list_tabs" => AgentOp::ListTabs,
        "read_terminal" => {
            let lines = arg_u64(args, "lines")?
                .map(u32::try_from)
                .transpose()
                .map_err(|_| bad("parameter `lines` is too large".into()))?;
            AgentOp::Read { tab: tab()?, lines }
        }
        "send_input" => AgentOp::Send { tab: tab()?, data: s("text")? },
        "wait_for_output" => AgentOp::Wait {
            tab: tab()?,
            pattern: s("pattern")?,
            timeout_ms: arg_u64(args, "timeout_ms")?.unwrap_or(10_000),
        },
        "screenshot_tab" => AgentOp::Screenshot { tab: tab()? },
        "file_list" => AgentOp::FilesLs { tab: tab()?, path: arg_str(args, "path")?.unwrap_or_else(|| ".".into()) },
        "file_get" => AgentOp::FilesGet { tab: tab()?, remote: s("remote")?, local: s("local")? },
        "file_put" => AgentOp::FilesPut { tab: tab()?, local: s("local")?, remote: s("remote")? },
        "tunnel_start" => AgentOp::TunnelStart { name: s("name")? },
        "tunnel_stop" => AgentOp::TunnelStop { name: s("name")? },
        "close_tab" => AgentOp::Close { tab: tab()? },
        "serial_list_ports" => AgentOp::SerialListPorts,
        "serial_send_break" => AgentOp::SerialSendBreak { tab: tab()?, ms: arg_u64(args, "ms")? },
        "serial_set_lines" => {
            AgentOp::SerialSetLines { tab: tab()?, dtr: arg_on_off(args, "dtr")?, rts: arg_on_off(args, "rts")? }
        }
        other => return Err(bad(format!("unknown tool `{other}`"))),
    })
}

/// The MCP server.
#[derive(Clone)]
pub struct Server {
    ctx: Arc<Ctx>,
}

impl Server {
    pub fn new(ctx: Ctx) -> Self {
        Self { ctx: Arc::new(ctx) }
    }

    /// An instance with agent control enabled is listening.
    fn live(&self) -> bool {
        self.ctx.control.endpoint().is_some_and(|p| std::path::Path::new(&p).exists())
    }

    async fn live_call(&self, client: String, op: AgentOp) -> Result<Value, ControlError> {
        client::call(self.ctx.control.as_ref(), &client, EntryPoint::McpLive, op).await
    }

    async fn offline_read(&self, client: String, op: AgentOp) -> Result<Value, ControlError> {
        if !control::allowed(op.group(), EntryPoint::McpOffline) {
            return Err(ControlError::new(ErrorCode::NotRunning, "rustXterm is not running, or agent control is off"));
        }
        let ctx = self.ctx.clone();
        let task = tokio::task::spawn_blocking(move || {
            let store = offline::keyring_store(&ctx)?
                .ok_or_else(|| offline::locked("without a running instance only the keyring can unlock it"))?;
            rxt_engine::offline::apply(&store, &client, EntryPoint::McpOffline, &op)
        });
        task.await.map_err(|e| ControlError::new(ErrorCode::Failed, e.to_string()))?
    }

    async fn run(&self, client: String, op: AgentOp) -> Result<Value, ControlError> {
        if !self.live() {
            return self.offline_read(client, op).await;
        }
        match self.live_call(client.clone(), op.clone()).await {
            Err(f) if f.code == ErrorCode::NotRunning => self.offline_read(client, op).await,
            other => other,
        }
    }
}

fn content(tool: &str, v: Value) -> Vec<ContentBlock> {
    match (tool, &v) {
        ("screenshot_tab", Value::Object(o)) if o.get("png").is_some_and(Value::is_string) => {
            vec![ContentBlock::image(o.get("png").and_then(Value::as_str).unwrap_or_default(), "image/png")]
        }
        (_, Value::String(s)) => vec![ContentBlock::text(s.clone())],
        _ => vec![ContentBlock::text(v.to_string())],
    }
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::new(ServerCapabilities::builder().enable_tools().build());
        info.server_info = Implementation::new("rustxterm", env!("CARGO_PKG_VERSION"));
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tools(self.live())))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let client =
            context.peer.peer_info().map(|i| i.client_info.name.clone()).unwrap_or_else(|| "mcp-client".into());
        let name = request.name.to_string();
        let Some(def) = TOOLS.iter().find(|t| t.name == name) else {
            if self.live() {
                // Let the engine reject and audit the raw request; the answer is irrelevant.
                let _ = self.live_call(client, AgentOp::GuiOnly { name: name.clone() }).await;
            }
            return Err(bad(format!("unknown tool `{name}`")));
        };
        let op = build_op(def, &request.arguments.unwrap_or_default())?;
        let result = match self.run(client, op).await {
            Ok(v) => CallToolResult::success(content(def.name, v)),
            Err(f) => CallToolResult::error(vec![ContentBlock::text(
                json!({ "code": f.code, "message": f.message }).to_string(),
            )]),
        };
        Ok(result.into())
    }
}

/// Serve MCP on stdin/stdout until the client disconnects.
pub fn run(ctx: Ctx) -> Result<(), ControlError> {
    let fail = |e: String| ControlError::new(ErrorCode::Failed, e);
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| fail(e.to_string()))?;
    rt.block_on(async {
        let running = Server::new(ctx).serve(rmcp::transport::io::stdio()).await.map_err(|e| fail(e.to_string()))?;
        running.waiting().await.map_err(|e| fail(e.to_string()))?;
        Ok(())
    })
}

/// Run `messages` (JSON-RPC objects) against an in-process server over an in-memory duplex and
/// return the responses to the messages that have an `id`, in order.
pub async fn exchange(ctx: Ctx, messages: Vec<Value>) -> Result<Vec<Value>, String> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (ours, theirs) = tokio::io::duplex(1 << 20);
    let server = tokio::spawn(async move {
        if let Ok(running) = Server::new(ctx).serve(theirs).await {
            let _ = running.waiting().await;
        }
    });
    let (r, mut w) = tokio::io::split(ours);
    let mut lines = BufReader::new(r).lines();
    let mut out = Vec::new();
    for msg in messages {
        let id = msg.get("id").cloned();
        w.write_all(format!("{msg}\n").as_bytes()).await.map_err(|e| e.to_string())?;
        let Some(id) = id else { continue };
        loop {
            let line = tokio::time::timeout(std::time::Duration::from_secs(10), lines.next_line())
                .await
                .map_err(|_| "MCP server did not answer in 10 s".to_string())?
                .map_err(|e| e.to_string())?
                .ok_or("MCP server closed the stream")?;
            let v: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
            if v.get("id") == Some(&id) {
                out.push(v);
                break;
            }
        }
    }
    server.abort();
    Ok(out)
}

/// JSON-RPC `initialize` + `initialized` for [`exchange`].
pub fn handshake(client: &str) -> Vec<Value> {
    vec![
        json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": client, "version": "1.0"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    ]
}

/// `--self-test`: initialize + tools/list in-process; returns the tool names.
pub fn self_test(ctx: Ctx) -> Result<Vec<String>, String> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    let mut msgs = handshake("rustxterm-self-test");
    msgs.push(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}));
    let replies = rt.block_on(exchange(ctx, msgs))?;
    let server = replies.first().and_then(|r| r.pointer("/result/serverInfo/name")).and_then(Value::as_str);
    if server != Some("rustxterm") {
        return Err(format!("unexpected initialize reply: {:?}", replies.first()));
    }
    let tools =
        replies.get(1).and_then(|r| r.pointer("/result/tools")).and_then(Value::as_array).ok_or("no tool list")?;
    Ok(tools.iter().filter_map(|t| t["name"].as_str().map(str::to_owned)).collect())
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use super::*;
    use crate::client::fake;
    use rxt_core::control::Response;
    use std::path::PathBuf;

    fn ctx(tag: &str) -> (PathBuf, Ctx) {
        let dir = std::env::temp_dir().join(format!("rxt-agent-mcp-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = Ctx {
            data_dir: dir.join("data"),
            keyring: Arc::new(rxt_platform::fake::Fake::default()),
            control: Arc::new(rxt_platform_linux::UnixControl::new(Some(dir.join("run")))),
        };
        (dir, ctx)
    }

    fn call(id: u64, name: &str, args: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": args}})
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
    }

    #[test]
    fn schema_snapshot() {
        let got: Vec<Value> =
            tools(true).iter().map(|t| json!({"name": t.name, "inputSchema": *t.input_schema})).collect();
        let got = serde_json::to_string_pretty(&got).unwrap() + "\n";
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/snapshots/mcp_tools.json");
        if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
            std::fs::write(path, &got).unwrap();
        }
        let want = std::fs::read_to_string(path).unwrap_or_default();
        assert_eq!(got, want, "MCP tool schemas changed; rerun with UPDATE_SNAPSHOTS=1 and review");
    }

    #[test]
    fn exactly_the_matrix_columns() {
        let names = |live| tools(live).iter().map(|t| t.name.to_string()).collect::<Vec<_>>();
        assert_eq!(names(false), ["list_sessions", "show_session", "list_folders", "list_tunnels", "export"]);
        assert_eq!(names(true).len(), 20);
        for t in names(true) {
            for bad in ["add", "edit", "remove", "import", "approv", "policy", "credential", "password"] {
                assert!(!t.contains(bad), "{t} looks like a mutation/GUI-only tool");
            }
        }
        // Every tool maps to an op the matrix allows for MCP live; offline ones also for MCP offline.
        for def in TOOLS {
            let args: JsonObject = def
                .params
                .iter()
                .map(|p| (p.name.to_string(), if matches!(p.kind, Kind::Int) { json!(1) } else { json!("x") }))
                .collect();
            let op = build_op(def, &args).unwrap();
            assert!(control::allowed(op.group(), EntryPoint::McpLive), "{}", def.name);
            assert_eq!(control::allowed(op.group(), EntryPoint::McpOffline), !def.live_only, "{}", def.name);
        }
    }

    #[test]
    fn invalid_params_and_unknown_tools_keep_running() {
        let (dir, ctx) = ctx("invalid");
        let mut msgs = handshake("t");
        msgs.extend([
            call(1, "list_tabs", json!({})), // offline: no such tool → not running error
            call(2, "send_input", json!({"text": "x"})),
            call(3, "send_input", json!({"tab": "one", "text": "x"})),
            call(4, "send_input", json!({"tab": 1, "text": "x", "agent_policy": "allow"})),
            call(5, "add_session", json!({"url": "ssh://h"})),
            json!({"jsonrpc": "2.0", "id": 6, "method": "tools/list"}),
        ]);
        let r = rt().block_on(exchange(ctx, msgs)).unwrap();
        assert_eq!(r[1]["result"]["isError"], true);
        assert!(r[1]["result"]["content"][0]["text"].as_str().unwrap().contains("not_running"));
        assert!(r[2]["error"]["message"].as_str().unwrap().contains("`tab`"));
        assert!(r[3]["error"]["message"].as_str().unwrap().contains("`tab`"));
        assert!(r[4]["error"]["message"].as_str().unwrap().contains("`agent_policy`"));
        assert!(r[5]["error"]["message"].as_str().unwrap().contains("add_session"));
        assert_eq!(r[6]["result"]["tools"].as_array().unwrap().len(), 5);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn offline_reads_without_keyring_are_locked() {
        let (dir, ctx) = ctx("locked");
        rxt_store::envelope::create_with_params(
            &ctx.data_dir,
            &secrecy::SecretString::from("pw"),
            None,
            rxt_store::envelope::KdfParams::MIN,
        )
        .unwrap();
        let mut msgs = handshake("t");
        msgs.push(call(1, "list_sessions", json!({})));
        let r = rt().block_on(exchange(ctx, msgs)).unwrap();
        assert_eq!(r[1]["result"]["isError"], true);
        assert!(r[1]["result"]["content"][0]["text"].as_str().unwrap().contains("\"locked\""));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn offline_reads_with_keyring_redact_and_audit() {
        use rxt_store::db::{CredentialKind, Store};
        let (dir, ctx) = ctx("keyring");
        let key = rxt_store::envelope::create_with_params(
            &ctx.data_dir,
            &secrecy::SecretString::from("pw"),
            Some(ctx.keyring.as_ref()),
            rxt_store::envelope::KdfParams::MIN,
        )
        .unwrap();
        let store = Store::open(&ctx.data_dir, &key).unwrap();
        let mut s = rxt_core::Session::new(rxt_core::Protocol::Ssh, "h.example", None, None);
        s.name = "prod".into();
        let id = store.save_session(&s).unwrap();
        store.save_credential(id, CredentialKind::Password, &secrecy::SecretString::from("hunter2-secret")).unwrap();
        let mut msgs = handshake("offline-agent");
        msgs.extend([call(1, "show_session", json!({"session": "prod"})), call(2, "export", json!({}))]);
        let r = rt().block_on(exchange(ctx, msgs)).unwrap();
        let shown: Value = serde_json::from_str(r[1]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(shown["password"], "saved");
        assert!(r[2]["result"]["content"][0]["text"].as_str().unwrap().contains("h.example"));
        assert!(!serde_json::to_string(&r).unwrap().contains("hunter2"));
        let audit = store.audit(0, 10).unwrap();
        assert!(audit.iter().all(|a| a.client == "offline-agent" && a.entry_point == "mcp-offline"), "{audit:?}");
        assert_eq!(audit.len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn live_calls_carry_client_name_and_audit_unknown_tools() {
        let (dir, mut ctx) = ctx("live");
        let replies = vec![
            Response::Ok { id: 1, result: json!({"text": "load average: 0.1"}) },
            Response::Err { id: 1, code: ErrorCode::Denied, message: "gui-only".into() },
        ];
        let mut msgs = handshake("claude-test");
        msgs.extend([
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            call(2, "wait_for_output", json!({"tab": 4, "pattern": "load average"})),
            call(3, "approve", json!({})),
        ]);
        let (r, mut rx) = rt().block_on(async {
            let (control, rx) = fake::serve(&dir.join("run"), replies);
            ctx.control = control;
            (exchange(ctx, msgs).await.unwrap(), rx)
        });
        assert_eq!(r[1]["result"]["tools"].as_array().unwrap().len(), 20);
        assert!(r[2]["result"]["content"][0]["text"].as_str().unwrap().contains("load average"));
        assert!(r[3]["error"].is_object());
        let req = rx.try_recv().unwrap();
        assert_eq!((req.client.as_str(), req.entry), ("claude-test", EntryPoint::McpLive));
        assert_eq!(req.op, AgentOp::Wait { tab: TabId(4), pattern: "load average".into(), timeout_ms: 10_000 });
        let audited = rx.try_recv().unwrap();
        assert_eq!(audited.op, AgentOp::GuiOnly { name: "approve".into() });
        let _ = std::fs::remove_dir_all(dir);
    }
}
