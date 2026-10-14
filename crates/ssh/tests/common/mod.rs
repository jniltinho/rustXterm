//! Shared lab-test helpers (also included by `crates/sftp/tests` via `#[path]`).
//! Lab accounts and passwords are fictional (see `lab/README.md`).
#![allow(dead_code)]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rxt_core::{BoxFut, HostKeyDecision, PromptAnswer, PromptKind, Protocol, Secret, Session};
use rxt_ssh::{Client, ConnectContext, Prompter, Shell, ShellMsg, TrustStore};
use tokio_util::sync::CancellationToken;

pub const SSHD: u16 = 2222;
pub const BASTION: u16 = 2223;
pub const ALICE_PASS: &str = "lab-alice-pass";
pub const CAROL_PASS: &str = "lab-carol-pass";
pub const SFTP_PASS: &str = "lab-sftp-pass";

/// Scripted prompter: fixed host-key decision, queued secrets, and a record of every prompt.
pub struct Script {
    pub host_key: HostKeyDecision,
    pub passwords: Mutex<VecDeque<String>>,
    pub kbd: Mutex<VecDeque<Vec<String>>>,
    pub prompts: Mutex<Vec<PromptKind>>,
    /// `(host, old, new)` per `host_key_changed` call.
    pub changed: Mutex<Vec<(String, String, String)>>,
}

impl Script {
    pub fn new(host_key: HostKeyDecision, passwords: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            host_key,
            passwords: Mutex::new(passwords.iter().map(|p| (*p).to_owned()).collect()),
            kbd: Mutex::new(VecDeque::new()),
            prompts: Mutex::new(Vec::new()),
            changed: Mutex::new(Vec::new()),
        })
    }

    pub fn prompts(&self) -> Vec<PromptKind> {
        self.prompts.lock().unwrap().clone()
    }

    pub fn host_key_prompts(&self) -> usize {
        self.prompts().iter().filter(|p| matches!(p, PromptKind::HostKeyUnknown { .. })).count()
    }

    pub fn credential_prompts(&self) -> usize {
        self.prompts().iter().filter(|p| !matches!(p, PromptKind::HostKeyUnknown { .. })).count()
    }
}

impl Prompter for Script {
    fn prompt(&self, kind: PromptKind) -> BoxFut<'_, PromptAnswer> {
        self.prompts.lock().unwrap().push(kind.clone());
        let answer = match kind {
            PromptKind::HostKeyUnknown { .. } => PromptAnswer::HostKey(self.host_key),
            PromptKind::Password { .. } => match self.passwords.lock().unwrap().pop_front() {
                Some(p) => PromptAnswer::Secret(Secret(p)),
                None => PromptAnswer::Cancel,
            },
            PromptKind::KeyboardInteractive { .. } => match self.kbd.lock().unwrap().pop_front() {
                Some(r) => PromptAnswer::Responses(r.into_iter().map(Secret).collect()),
                None => PromptAnswer::Cancel,
            },
            _ => PromptAnswer::Cancel,
        };
        Box::pin(async move { answer })
    }

    fn host_key_changed(&self, host: &str, old: &str, new: &str) {
        self.changed.lock().unwrap().push((host.to_owned(), old.to_owned(), new.to_owned()));
    }
}

/// Fresh, unique temp directory.
pub fn tmp_dir(tag: &str) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let d = std::env::temp_dir().join(format!(
        "rxt-lab-{tag}-{}-{nanos}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Unique token for names that must not collide across runs.
pub fn unique(tag: &str) -> String {
    tmp_dir(tag).file_name().unwrap().to_string_lossy().replace("rxt-lab-", "")
}

/// Session to the lab on 127.0.0.1, agent disabled.
pub fn session(user: &str, port: u16) -> Session {
    let mut s = Session::new(Protocol::Ssh, "127.0.0.1", Some(user.into()), Some(port));
    s.ssh.use_agent = false;
    s
}

pub fn ctx(prompter: Arc<Script>, trust: Arc<TrustStore>, known_hosts: &Path) -> ConnectContext {
    ConnectContext { prompter, trust, known_hosts: known_hosts.to_owned(), cancel: CancellationToken::new() }
}

/// Connect with a fresh known_hosts path and trust store, accepting the host key once.
pub async fn connect_with(s: &Session, script: Arc<Script>) -> rxt_core::Result<Client> {
    let kh = tmp_dir("kh").join("known_hosts");
    timeout(rxt_ssh::connect(s, ctx(script, Arc::default(), &kh))).await
}

pub async fn alice(port: u16) -> Client {
    connect_with(&session("alice", port), Script::new(HostKeyDecision::AcceptOnce, &[ALICE_PASS])).await.unwrap()
}

pub async fn timeout<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(60), f).await.expect("lab operation timed out")
}

/// Shell output collected so far.
#[derive(Default, Debug)]
pub struct Output {
    pub text: String,
    pub exit: Option<u32>,
    pub closed: bool,
}

/// Read until `done` holds for the output, the channel closes, or 30 s pass.
pub async fn read_until(shell: &mut Shell, mut done: impl FnMut(&Output) -> bool) -> Output {
    let mut out = Output::default();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !done(&out) {
        match tokio::time::timeout_at(deadline, shell.read()).await {
            Err(_) => panic!("timed out waiting for shell output; got: {out:?}"),
            Ok(None) => {
                out.closed = true;
                break;
            }
            Ok(Some(ShellMsg::Data(d))) => out.text.push_str(&String::from_utf8_lossy(&d)),
            Ok(Some(ShellMsg::ExitStatus(c))) => out.exit = Some(c),
            Ok(Some(ShellMsg::Eof)) => {}
        }
    }
    out
}

/// Run a remote command (exec channel) and collect output until the channel closes.
pub async fn run(client: &Client, s: &Session, cmd: &str) -> Output {
    let mut s = s.clone();
    s.ssh.remote_command = Some(cmd.into());
    let mut shell = timeout(client.shell(&s, 120, 40)).await.unwrap();
    read_until(&mut shell, |_| false).await
}

/// sshd log of a lab host (`sshd`, `bastion`, `internal`).
pub fn log_path(host: &str) -> PathBuf {
    PathBuf::from(std::env::var("RXT_LAB_LOG_DIR").expect("RXT_LAB_LOG_DIR is set by `cargo xtask lab test`"))
        .join(format!("{host}.log"))
}

pub fn log_len(host: &str) -> u64 {
    std::fs::metadata(log_path(host)).map(|m| m.len()).unwrap_or(0)
}

/// Log text appended after `offset`.
pub fn log_since(host: &str, offset: u64) -> String {
    let data = std::fs::read(log_path(host)).unwrap();
    String::from_utf8_lossy(data.get(usize::try_from(offset).unwrap()..).unwrap_or_default()).into_owned()
}
