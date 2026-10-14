//! CLI subcommands. Offline reads open the store (keyring, else the master password typed on a
//! terminal); offline changes require the master password typed on an interactive terminal;
//! live operations go over the control channel and never start the GUI.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine;
use clap::Subcommand;
use rxt_core::TabId;
use rxt_core::control::{AgentOp, EntryPoint, ErrorCode, SessionPatch};
use rxt_store::db::Store;
use secrecy::SecretString;
use serde_json::{Value, json};

use crate::{CLI_CLIENT, ControlError, Ctx, client, exit_code, offline, parse_ref};

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Saved sessions (offline store).
    Sessions {
        #[command(subcommand)]
        cmd: SessionsCmd,
    },
    /// Session folders (offline store).
    Folders {
        #[command(subcommand)]
        cmd: ListCmd,
    },
    /// Saved tunnels (offline store).
    Tunnels {
        #[command(subcommand)]
        cmd: ListCmd,
    },
    /// Export sessions, folders and tunnels as TOML (no credentials).
    Export {
        /// Write to this file instead of stdout.
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
    },
    /// Import sessions (preview, then confirm on the terminal).
    Import {
        #[command(subcommand)]
        cmd: ImportCmd,
    },
    /// Open a saved session in the running instance.
    Open { session: String },
    /// Open tabs of the running instance.
    Tabs {
        #[command(subcommand)]
        cmd: ListCmd,
    },
    /// Send text to a tab.
    Send {
        tab: u64,
        text: String,
        /// Append a newline.
        #[arg(long)]
        enter: bool,
    },
    /// Read a tab's terminal text.
    Read {
        tab: u64,
        #[arg(long, value_name = "N")]
        lines: Option<u32>,
    },
    /// Wait until a tab's output matches a pattern.
    Wait {
        tab: u64,
        pattern: String,
        /// e.g. 10s, 500ms, 2m (plain numbers are seconds).
        #[arg(long, default_value = "10s", value_parser = parse_duration_ms)]
        timeout: u64,
    },
    /// Capture a tab as PNG.
    Screenshot {
        tab: u64,
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
    },
    /// File operations on a tab (local paths are inside the agent workspace).
    Files {
        #[command(subcommand)]
        cmd: FilesCmd,
    },
    /// Start or stop a saved tunnel in the running instance.
    Tunnel {
        #[command(subcommand)]
        cmd: TunnelCmd,
    },
    /// Close a tab.
    Close { tab: u64 },
    /// Serial ports and line control in the running instance.
    Serial {
        #[command(subcommand)]
        cmd: SerialCmd,
    },
    /// Run the MCP server on stdio.
    Mcp,
}

#[derive(Debug, Subcommand)]
pub enum SerialCmd {
    /// List local serial ports.
    Ports,
    /// Send a break on a serial tab.
    Break {
        tab: u64,
        /// Break length in milliseconds (default 250).
        #[arg(long)]
        ms: Option<u64>,
    },
    /// Set DTR and/or RTS on a serial tab.
    Lines {
        tab: u64,
        #[arg(long, value_parser = parse_on_off)]
        dtr: Option<bool>,
        #[arg(long, value_parser = parse_on_off)]
        rts: Option<bool>,
    },
}

fn parse_on_off(s: &str) -> Result<bool, String> {
    match s {
        "on" | "1" | "true" => Ok(true),
        "off" | "0" | "false" => Ok(false),
        _ => Err(format!("expected on or off, got `{s}`")),
    }
}

#[derive(Debug, Subcommand)]
pub enum ListCmd {
    List,
}

#[derive(Debug, Subcommand)]
pub enum SessionsCmd {
    List,
    Show {
        session: String,
    },
    /// Add a session from a URL such as ssh://user@host:22 (master password on a terminal).
    Add {
        url: String,
        #[arg(long)]
        name: Option<String>,
        /// Folder path, `/`-separated.
        #[arg(long)]
        folder: Option<String>,
    },
    /// Edit non-secret fields (master password on a terminal).
    Edit {
        session: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        user: Option<String>,
        #[arg(long)]
        folder: Option<String>,
    },
    /// Remove a session (master password on a terminal).
    Remove {
        session: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ImportCmd {
    /// Import bookmarks from a MobaXterm.ini file.
    MobaxtermIni { file: PathBuf },
}

#[derive(Debug, Subcommand)]
pub enum FilesCmd {
    Ls {
        tab: u64,
        #[arg(default_value = ".")]
        path: String,
    },
    Get {
        tab: u64,
        remote: String,
        local: String,
    },
    Put {
        tab: u64,
        local: String,
        remote: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum TunnelCmd {
    Start { name: String },
    Stop { name: String },
}

/// `10s`, `500ms`, `2m` or a plain number of seconds, in milliseconds.
pub fn parse_duration_ms(s: &str) -> Result<u64, String> {
    let (num, scale) = if let Some(n) = s.strip_suffix("ms") {
        (n, 1)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1000)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60_000)
    } else {
        (s, 1000)
    };
    num.trim().parse::<u64>().map(|n| n.saturating_mul(scale)).map_err(|_| format!("invalid duration '{s}'"))
}

/// Run one subcommand (not `mcp`); prints the result and returns the exit code.
pub fn run(cmd: Command, json: bool, ctx: &Ctx) -> u8 {
    match execute(cmd, ctx) {
        Ok(v) => {
            print_result(&v, json);
            0
        }
        Err(f) => {
            if json {
                println!("{}", json!({ "error": { "code": f.code, "message": f.message } }));
            } else {
                eprintln!("rustxterm: {}", f.message);
            }
            exit_code(f.code)
        }
    }
}

fn execute(cmd: Command, ctx: &Ctx) -> Result<Value, ControlError> {
    let live = |op| client::call_blocking(ctx.control.as_ref(), CLI_CLIENT, EntryPoint::CliLive, op);
    let tab = TabId;
    match cmd {
        Command::Sessions { cmd } => match cmd {
            SessionsCmd::List => read_offline(ctx, &AgentOp::ListSessions),
            SessionsCmd::Show { session } => read_offline(ctx, &AgentOp::ShowSession { session: parse_ref(&session) }),
            SessionsCmd::Add { url, name, folder } => change_offline(
                ctx,
                AgentOp::AddSession { url, patch: SessionPatch { name, folder, ..Default::default() } },
            ),
            SessionsCmd::Edit { session, name, host, port, user, folder } => change_offline(
                ctx,
                AgentOp::EditSession {
                    session: parse_ref(&session),
                    patch: SessionPatch { name, host, port, user, folder },
                },
            ),
            SessionsCmd::Remove { session } => {
                change_offline(ctx, AgentOp::RemoveSession { session: parse_ref(&session) })
            }
        },
        Command::Folders { cmd: ListCmd::List } => read_offline(ctx, &AgentOp::ListFolders),
        Command::Tunnels { cmd: ListCmd::List } => read_offline(ctx, &AgentOp::ListTunnels),
        Command::Export { out } => {
            let toml = read_offline(ctx, &AgentOp::Export)?;
            match out {
                None => Ok(toml),
                Some(path) => {
                    std::fs::write(&path, toml.as_str().unwrap_or_default()).map_err(rxt_core::Error::from)?;
                    Ok(json!({ "path": path }))
                }
            }
        }
        Command::Import { cmd: ImportCmd::MobaxtermIni { file } } => import(ctx, file),
        Command::Open { session } => live(AgentOp::Open { session: parse_ref(&session) }),
        Command::Tabs { cmd: ListCmd::List } => live(AgentOp::ListTabs),
        Command::Send { tab: t, mut text, enter } => {
            if enter {
                text.push('\n');
            }
            live(AgentOp::Send { tab: tab(t), data: text })
        }
        Command::Read { tab: t, lines } => live(AgentOp::Read { tab: tab(t), lines }),
        Command::Wait { tab: t, pattern, timeout } => live(AgentOp::Wait { tab: tab(t), pattern, timeout_ms: timeout }),
        Command::Screenshot { tab: t, out } => {
            let v = live(AgentOp::Screenshot { tab: tab(t) })?;
            let Some(path) = out else { return Ok(v) };
            let png = v.get("png").and_then(Value::as_str).unwrap_or_default();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(png)
                .map_err(|e| ControlError::new(ErrorCode::Failed, format!("invalid screenshot data: {e}")))?;
            std::fs::write(&path, bytes).map_err(rxt_core::Error::from)?;
            Ok(json!({ "path": path }))
        }
        Command::Files { cmd } => live(match cmd {
            FilesCmd::Ls { tab: t, path } => AgentOp::FilesLs { tab: tab(t), path },
            FilesCmd::Get { tab: t, remote, local } => AgentOp::FilesGet { tab: tab(t), remote, local },
            FilesCmd::Put { tab: t, local, remote } => AgentOp::FilesPut { tab: tab(t), local, remote },
        }),
        Command::Tunnel { cmd } => live(match cmd {
            TunnelCmd::Start { name } => AgentOp::TunnelStart { name },
            TunnelCmd::Stop { name } => AgentOp::TunnelStop { name },
        }),
        Command::Close { tab: t } => live(AgentOp::Close { tab: tab(t) }),
        Command::Serial { cmd } => live(match cmd {
            SerialCmd::Ports => AgentOp::SerialListPorts,
            SerialCmd::Break { tab: t, ms } => AgentOp::SerialSendBreak { tab: tab(t), ms },
            SerialCmd::Lines { tab: t, dtr, rts } => AgentOp::SerialSetLines { tab: tab(t), dtr, rts },
        }),
        Command::Mcp => Err(ControlError::new(ErrorCode::Usage, "mcp is not a CLI command")),
    }
}

/// Offline read: keyring unlock, else the master password typed on a terminal.
fn read_offline(ctx: &Ctx, op: &AgentOp) -> Result<Value, ControlError> {
    let store = match offline::keyring_store(ctx)? {
        Some(s) => s,
        None if std::io::stdin().is_terminal() => offline::password_store(ctx, &prompt_password()?)?,
        None => return Err(offline::locked("type the master password on a terminal, or enable the keyring")),
    };
    rxt_engine::offline::apply(&store, CLI_CLIENT, EntryPoint::CliOffline, op)
}

/// Offline change: never the keyring, pipes or environment — only a terminal.
fn change_offline(ctx: &Ctx, op: AgentOp) -> Result<Value, ControlError> {
    let store = terminal_store(ctx)?;
    rxt_engine::offline::apply(&store, CLI_CLIENT, EntryPoint::CliOffline, &op)
}

fn terminal_store(ctx: &Ctx) -> Result<Store, ControlError> {
    if !std::io::stdin().is_terminal() {
        return Err(ControlError::new(
            ErrorCode::Denied,
            "offline changes require the master password typed on an interactive terminal",
        ));
    }
    offline::password_store(ctx, &prompt_password()?)
}

fn import(ctx: &Ctx, file: PathBuf) -> Result<Value, ControlError> {
    if !std::io::stdin().is_terminal() {
        return Err(ControlError::new(ErrorCode::Denied, "import requires an interactive terminal"));
    }
    let report = rxt_store::moba_ini::import_file(&file)?;
    eprintln!("{}: {} session(s) in {} folder(s)", file.display(), report.sessions.len(), report.folders.len());
    for (folder, s) in &report.sessions {
        eprintln!("  {folder}/{}  {}://{}:{}", s.name, s.protocol.scheme(), s.host, s.port);
    }
    for s in &report.skipped {
        eprintln!("  skipped line {} {:?}: {}", s.line, s.name, s.reason);
    }
    if !confirm(&format!("Import {} session(s)? [y/N] ", report.sessions.len()))? {
        return Err(ControlError::new(ErrorCode::Failed, "import cancelled"));
    }
    let store = terminal_store(ctx)?;
    let op = AgentOp::ImportMobaxtermIni { path: file.to_string_lossy().into_owned() };
    rxt_engine::offline::apply(&store, CLI_CLIENT, EntryPoint::CliOffline, &op)
}

fn stdin_line() -> Result<String, ControlError> {
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).map_err(rxt_core::Error::from)?;
    while line.ends_with(['\n', '\r']) {
        line.pop();
    }
    Ok(line)
}

fn confirm(question: &str) -> Result<bool, ControlError> {
    eprint!("{question}");
    let _ = std::io::stderr().flush();
    Ok(matches!(stdin_line()?.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

/// True while the master-password prompt owns Ctrl+C (see [`ctrl_c_watcher`]).
static PROMPTING: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
fn ctrl_c_signal() -> std::io::Result<tokio::signal::unix::Signal> {
    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
}

#[cfg(windows)]
fn ctrl_c_signal() -> std::io::Result<tokio::signal::windows::CtrlC> {
    tokio::signal::windows::ctrl_c()
}

/// Once per process: take over Ctrl+C. The no-echo reader sees Ctrl+C as a key and raises the
/// interrupt itself while echo is still off; with the default action the process would die
/// before echo is restored. During a prompt the interrupt is ignored here (the reader returns an
/// error and restores echo on drop); otherwise the process exits with 130 as before.
fn ctrl_c_watcher() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
        let Ok(mut signal) = rt.block_on(async { ctrl_c_signal() }) else { return };
        std::thread::spawn(move || {
            rt.block_on(async {
                while signal.recv().await.is_some() {
                    if !PROMPTING.load(Ordering::SeqCst) {
                        std::process::exit(130);
                    }
                }
            });
        });
    });
}

/// Read the master password from the terminal without echo (console API on Windows, termios on
/// Unix); echo is restored on success, error and Ctrl+C.
fn prompt_password() -> Result<SecretString, ControlError> {
    ctrl_c_watcher();
    PROMPTING.store(true, Ordering::SeqCst);
    let read = rpassword::prompt_password("Master password: ");
    PROMPTING.store(false, Ordering::SeqCst);
    match read {
        Ok(line) => Ok(SecretString::from(line)),
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
            Err(ControlError::new(ErrorCode::Failed, "master password prompt interrupted"))
        }
        Err(e) => Err(ControlError::new(ErrorCode::Failed, format!("cannot read the master password: {e}"))),
    }
}

fn print_result(v: &Value, json: bool) {
    if json {
        println!("{v}");
        return;
    }
    match v {
        Value::String(s) => print!("{s}{}", if s.ends_with('\n') { "" } else { "\n" }),
        Value::Array(items) => items.iter().for_each(|i| println!("{}", line(i))),
        Value::Object(o) if o.get("text").is_some_and(Value::is_string) => {
            println!("{}", o.get("text").and_then(Value::as_str).unwrap_or_default());
        }
        other => println!("{}", serde_json::to_string_pretty(other).unwrap_or_default()),
    }
}

/// One-line human form: sessions as `id  name  scheme://user@host:port`, others compact JSON.
fn line(v: &Value) -> String {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    match v.get("host") {
        Some(_) => {
            let user = v.get("user").and_then(Value::as_str).map(|u| format!("{u}@")).unwrap_or_default();
            format!("{}\t{}\t{}://{user}{}:{}", v["id"], s("name"), s("protocol"), s("host"), v["port"])
        }
        None => v.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration_ms("10s"), Ok(10_000));
        assert_eq!(parse_duration_ms("500ms"), Ok(500));
        assert_eq!(parse_duration_ms("2m"), Ok(120_000));
        assert_eq!(parse_duration_ms("3"), Ok(3000));
        assert!(parse_duration_ms("soon").is_err());
    }

    #[test]
    fn session_line_has_no_secrets() {
        let v = json!({"id": 3, "name": "prod", "protocol": "ssh", "host": "h", "port": 22, "user": "u", "password": "saved"});
        assert_eq!(line(&v), "3\tprod\tssh://u@h:22");
    }
}
