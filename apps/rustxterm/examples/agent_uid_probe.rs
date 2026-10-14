//! Other-UID control-socket probe for `lab/agent/uid-test.sh` (task 9.1).
//!
//! - `agent_uid_probe serve <dir>`: headless engine with agent control on, socket in
//!   `<dir>/run/rustxterm/`, store in `<dir>/data/`; writes `<dir>/ready`, runs until `<dir>/done`
//!   exists, then prints one line per audit record (`audit <client> <command> <decision>`).
//! - `agent_uid_probe connect <dir> <client>`: sends one `tabs.list` request and prints
//!   `response`, `closed-without-response` or `connect-refused: <error>` (exit 0, 11, 10).

#[cfg(target_os = "linux")]
use std::io::{ErrorKind, Read, Write};
#[cfg(target_os = "linux")]
use std::os::unix::net::UnixStream;
#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::ExitCode;
#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use anyhow::{Context, Result, bail};
#[cfg(target_os = "linux")]
use rxt_engine::{Engine, EngineConfig};

#[cfg(target_os = "linux")]
fn main() -> Result<ExitCode> {
    let args: Vec<String> = std::env::args().collect();
    match (args.get(1).map(String::as_str), args.get(2)) {
        (Some("serve"), Some(dir)) => serve(Path::new(dir)).map(|()| ExitCode::SUCCESS),
        (Some("connect"), Some(dir)) => {
            connect(Path::new(dir), args.get(3).map_or("uid-probe", String::as_str)).map(ExitCode::from)
        }
        _ => bail!("usage: agent_uid_probe serve <dir> | connect <dir> [client]"),
    }
}

#[cfg(target_os = "linux")]
fn socket(dir: &Path) -> PathBuf {
    dir.join("run/rustxterm").join(rxt_platform_linux::SOCKET)
}

#[cfg(target_os = "linux")]
fn serve(dir: &Path) -> Result<()> {
    let vault = Arc::new(rxt_ui::Vault::fast(dir.join("data"), Arc::new(rxt_platform::fake::Fake::default())));
    vault.create("uid-probe-master", false)?;
    let config = EngineConfig {
        records: Some(vault.clone()),
        control: Some(Arc::new(rxt_platform_linux::UnixControl::new(Some(dir.join("run/rustxterm"))))),
        ..EngineConfig::default()
    };
    let engine = Engine::start(config, Arc::new(|| {}))?;
    engine.set_agent_control(true)?;
    std::fs::write(dir.join("ready"), b"")?;
    let deadline = Instant::now() + Duration::from_secs(300);
    while !dir.join("done").exists() {
        if Instant::now() > deadline {
            bail!("no `done` file after 300 s");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    for a in vault.with(|db| db.audit(0, 1000))?.iter().rev() {
        println!("audit {} {} {}", a.client, a.command, a.decision);
    }
    engine.shutdown(Duration::from_secs(2));
    Ok(())
}

#[cfg(target_os = "linux")]
fn connect(dir: &Path, client: &str) -> Result<u8> {
    let mut s = match UnixStream::connect(socket(dir)) {
        Ok(s) => s,
        Err(e) => {
            println!("connect-refused: {e}");
            return Ok(10);
        }
    };
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    let body = serde_json::json!({"id": 1, "client": client, "entry": "cli-live", "op": "list_tabs"}).to_string();
    let len = u32::try_from(body.len())?.to_be_bytes();
    // The server may already have closed the connection: a failed write is a closed channel.
    let sent = s.write_all(&len).and_then(|()| s.write_all(body.as_bytes()));
    let mut head = [0u8; 4];
    match sent.and_then(|()| s.read_exact(&mut head)) {
        Ok(()) => {
            let mut buf = vec![0; u32::from_be_bytes(head) as usize];
            s.read_exact(&mut buf).context("reading the response")?;
            println!("response {}", String::from_utf8_lossy(&buf));
            Ok(0)
        }
        Err(e) if matches!(e.kind(), ErrorKind::UnexpectedEof | ErrorKind::BrokenPipe | ErrorKind::ConnectionReset) => {
            println!("closed-without-response");
            Ok(11)
        }
        Err(e) => bail!("unexpected socket error: {e}"),
    }
}

/// The probe exercises the Unix control socket; other targets build an empty example.
#[cfg(not(target_os = "linux"))]
fn main() {}
