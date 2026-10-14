//! SSH client on russh: authentication, host-key gate (design D4), jump hosts,
//! X11 forwarding, tunnels and shell channels. No UI dependency.
//!
//! Contract used by `engine` (keep signatures stable; extend, don't break):
//! - [`connect`] runs the host-key gate on every hop before any authentication.
//! - [`Client::shell`] opens a PTY shell; [`Shell::read`] is pulled by the engine only when
//!   its output buffer has room, so russh's channel window applies backpressure.
//! - [`Client::sftp_stream`] opens the `sftp` subsystem on the same connection.
//! - [`Client::exec`] runs a one-shot command on the same connection and returns its stdout.
//! - [`Client::exec_stream`] runs a long-lived command (remote monitoring) and returns its
//!   stdin/stdout as one byte stream; dropping the stream closes the channel.
//! - `keep_open_after_command` is honored by the engine (the tab stays open after
//!   [`ShellMsg::ExitStatus`]); this crate only runs the command.

mod handler;
pub mod known_hosts;
pub mod tunnel;
pub mod x11;

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use russh::client::{AuthResult, Handle, KeyboardInteractiveAuthResponse, Msg};
use russh::keys::agent::client::{AgentClient, AgentStream};
use russh::keys::{PrivateKey, PrivateKeyWithHashAlg, PublicKey};
use russh::{
    ChannelMsg, ChannelReadHalf, ChannelWriteHalf, MethodKind, MethodSet, Preferred, cipher, compression, kex, mac,
};
use rxt_core::{Error, PromptAnswer, PromptKind, Result, Session, SshInfo, Tunnel, TunnelState};
use tokio_util::sync::CancellationToken;

pub use handler::{AUTH_BANNER_CAP, sanitize_banner};
use handler::{ClientHandler, Gate, Shared};
pub use tunnel::{parse_socks5_request, reconnect_delay};

pub use rxt_core::Prompter;

/// Local SSH agent of any transport.
pub(crate) type Agent = AgentClient<Box<dyn AgentStream + Send + Unpin>>;

/// Default agent pipe of the Windows OpenSSH `ssh-agent` service.
pub const WINDOWS_AGENT_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";

/// Agent pipe on Windows: the pipe named by `SSH_AUTH_SOCK` when set, else the OpenSSH one.
pub fn windows_agent_pipe(ssh_auth_sock: Option<std::ffi::OsString>) -> std::ffi::OsString {
    ssh_auth_sock.filter(|s| !s.is_empty()).unwrap_or_else(|| WINDOWS_AGENT_PIPE.into())
}

/// Connect to the local agent: `$SSH_AUTH_SOCK` on Unix; on Windows the agent pipe
/// ([`windows_agent_pipe`]), then Pageant.
pub(crate) async fn connect_agent() -> std::result::Result<Agent, russh::keys::Error> {
    #[cfg(unix)]
    return AgentClient::connect_env().await.map(AgentClient::dynamic);
    #[cfg(windows)]
    {
        let pipe = windows_agent_pipe(std::env::var_os("SSH_AUTH_SOCK"));
        match AgentClient::connect_named_pipe(&pipe).await {
            Ok(agent) => return Ok(agent.dynamic()),
            Err(e) => tracing::debug!("agent pipe {}: {e}; trying Pageant", pipe.to_string_lossy()),
        }
        AgentClient::connect_pageant().await.map(AgentClient::dynamic)
    }
    #[cfg(not(any(unix, windows)))]
    Err(russh::keys::Error::EnvVar("SSH_AUTH_SOCK"))
}

/// Inputs for [`connect`].
#[derive(Clone)]
pub struct ConnectContext {
    pub prompter: Arc<dyn Prompter>,
    /// App trust records.
    pub trust: Arc<TrustStore>,
    /// `~/.ssh/known_hosts` (read only).
    pub known_hosts: PathBuf,
    pub cancel: CancellationToken,
}

/// Persistent backing of [`TrustStore`] (the app's encrypted store); keys are OpenSSH
/// public-key lines (`ssh-ed25519 AAAA…`).
pub trait HostKeyRecords: Send + Sync {
    fn load(&self, host: &str, port: u16) -> Vec<String>;
    fn save(&self, host: &str, port: u16, key: &str);
}

/// App-owned host-key trust records: in memory, plus an optional persistent backing.
#[derive(Default)]
pub struct TrustStore {
    records: Mutex<Vec<(String, u16, PublicKey)>>,
    backing: Option<Arc<dyn HostKeyRecords>>,
}

impl TrustStore {
    /// Trust store that also loads and saves through `backing`.
    pub fn persistent(backing: Arc<dyn HostKeyRecords>) -> Self {
        Self { records: Mutex::default(), backing: Some(backing) }
    }

    /// Record an accepted key ("accept and save").
    pub fn add(&self, host: &str, port: u16, key: PublicKey) {
        if let (Some(b), Ok(line)) = (&self.backing, key.to_openssh()) {
            b.save(host, port, &line);
        }
        let mut records = self.records.lock().unwrap_or_else(PoisonError::into_inner);
        if !records.iter().any(|(h, p, k)| h == host && *p == port && k.key_data() == key.key_data()) {
            records.push((host.to_owned(), port, key));
        }
    }

    /// Keys stored for `host:port`.
    pub fn keys(&self, host: &str, port: u16) -> Vec<PublicKey> {
        let mut keys: Vec<PublicKey> = {
            let records = self.records.lock().unwrap_or_else(PoisonError::into_inner);
            records.iter().filter(|(h, p, _)| h.eq_ignore_ascii_case(host) && *p == port).map(|r| r.2.clone()).collect()
        };
        for line in self.backing.iter().flat_map(|b| b.load(host, port)) {
            match PublicKey::from_openssh(&line) {
                Ok(k) if !keys.iter().any(|x| x.key_data() == k.key_data()) => keys.push(k),
                Ok(_) => {}
                Err(e) => tracing::warn!("ignoring invalid stored host key for {host}:{port}: {e}"),
            }
        }
        keys
    }
}

/// An authenticated SSH connection (possibly through jump hosts).
pub struct Client {
    handle: Arc<Handle<ClientHandler>>,
    shared: Arc<Shared>,
    /// Jump-host connections carrying `handle`; kept alive with it.
    _jumps: Vec<Handle<ClientHandler>>,
}

fn conn_err(e: impl std::fmt::Display) -> Error {
    Error::Connection(e.to_string())
}

/// Largest stdout kept by [`Client::exec`].
pub const EXEC_OUTPUT_CAP: usize = 8 << 20;

const COMPRESSION_ON: &[compression::Name] = &[compression::ZLIB_LEGACY, compression::ZLIB, compression::NONE];
const COMPRESSION_OFF: &[compression::Name] = &[compression::NONE];

/// russh's safe defaults followed by the legacy algorithms old network gear still needs
/// (Cisco IOS: `diffie-hellman-group14-sha1`, `aes*-cbc`, `hmac-sha1`). Negotiation picks the
/// first client entry the server offers, so modern servers never fall back to these.
fn with_legacy<T: Clone>(safe: &[T], legacy: &[T]) -> Cow<'static, [T]> {
    Cow::Owned([safe, legacy].concat())
}

fn preferred(compression: bool) -> Preferred {
    let d = Preferred::DEFAULT;
    Preferred {
        kex: with_legacy(&d.kex, &[kex::DH_G14_SHA1, kex::DH_GEX_SHA1, kex::DH_G1_SHA1]),
        cipher: with_legacy(&d.cipher, &[cipher::AES_256_CBC, cipher::AES_192_CBC, cipher::AES_128_CBC]),
        mac: with_legacy(&d.mac, &[mac::HMAC_SHA1_ETM, mac::HMAC_SHA1]),
        compression: Cow::Borrowed(if compression { COMPRESSION_ON } else { COMPRESSION_OFF }),
        ..d
    }
}

fn config(session: &Session) -> Arc<russh::client::Config> {
    let o = &session.ssh;
    Arc::new(russh::client::Config {
        keepalive_interval: (o.keepalive_secs > 0).then(|| Duration::from_secs(u64::from(o.keepalive_secs))),
        preferred: preferred(o.compression),
        nodelay: true,
        ..Default::default()
    })
}

/// One hop of the chain (jump hosts first, then the target).
struct Hop {
    host: String,
    port: u16,
    user: String,
    key_file: Option<String>,
}

fn hops(session: &Session) -> Vec<Hop> {
    let default_user = session.user.clone().or_else(|| std::env::var("USER").ok()).unwrap_or_default();
    let mut hops: Vec<Hop> = session
        .ssh
        .jump_hosts
        .iter()
        .map(|j| Hop {
            host: j.host.clone(),
            port: if j.port == 0 { 22 } else { j.port },
            user: if j.user.is_empty() { default_user.clone() } else { j.user.clone() },
            key_file: j.key_file.clone(),
        })
        .collect();
    hops.push(Hop {
        host: session.host.clone(),
        port: if session.port == 0 { 22 } else { session.port },
        user: default_user,
        key_file: session.ssh.key_file.clone(),
    });
    hops
}

/// Connect, verify host keys on every hop, then authenticate (agent, key, keyboard-interactive, password).
pub async fn connect(session: &Session, ctx: ConnectContext) -> Result<Client> {
    tokio::select! {
        r = connect_chain(session, &ctx) => r,
        _ = ctx.cancel.cancelled() => Err(Error::Cancelled),
    }
}

async fn connect_chain(session: &Session, ctx: &ConnectContext) -> Result<Client> {
    let config = config(session);
    let hops = hops(session);
    let last = hops.len() - 1;
    let mut jumps: Vec<Handle<ClientHandler>> = Vec::new();
    for (i, hop) in hops.into_iter().enumerate() {
        let shared = Arc::new(if i == last {
            Shared {
                x11: if session.ssh.x11_forwarding { x11::X11::from_env() } else { None },
                agent_forwarding: session.ssh.agent_forwarding,
                ..Shared::default()
            }
        } else {
            Shared::default()
        });
        let failure = Arc::new(Mutex::new(None));
        let handler = ClientHandler {
            gate: Gate {
                host: hop.host.clone(),
                port: hop.port,
                known_hosts: ctx.known_hosts.clone(),
                trust: ctx.trust.clone(),
                prompter: ctx.prompter.clone(),
                failure: failure.clone(),
            },
            shared: shared.clone(),
        };
        // `connect_stream` returns only after key exchange, i.e. after `check_server_key` accepted the key.
        let connected = match jumps.last() {
            None => {
                let tcp = tokio::net::TcpStream::connect((hop.host.as_str(), hop.port))
                    .await
                    .map_err(|e| Error::Connection(format!("{}:{}: {e}", hop.host, hop.port)))?;
                russh::client::connect_stream(config.clone(), tcp, handler).await
            }
            Some(prev) => {
                let channel = prev
                    .channel_open_direct_tcpip(hop.host.as_str(), u32::from(hop.port), "127.0.0.1", 0)
                    .await
                    .map_err(|e| Error::Connection(format!("jump to {}:{}: {e}", hop.host, hop.port)))?;
                russh::client::connect_stream(config.clone(), channel.into_stream(), handler).await
            }
        };
        let mut handle = match connected {
            Ok(h) => h,
            Err(e) => {
                let gate_error = failure.lock().unwrap_or_else(PoisonError::into_inner).take();
                return Err(gate_error.unwrap_or_else(|| Error::Connection(format!("{}:{}: {e}", hop.host, hop.port))));
            }
        };
        authenticate(&mut handle, &hop, session, ctx.prompter.as_ref()).await?;
        if i == last {
            return Ok(Client { handle: Arc::new(handle), shared, _jumps: jumps });
        }
        jumps.push(handle);
    }
    Err(Error::Invalid("empty hop list".into()))
}

fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

/// Load a private key, prompting for the passphrase when it is encrypted. `None` skips key auth.
async fn load_key(path: &Path, prompter: &dyn Prompter) -> Result<Option<PrivateKey>> {
    match russh::keys::load_secret_key(path, None) {
        Ok(k) => return Ok(Some(k)),
        Err(russh::keys::Error::KeyIsEncrypted) => {}
        Err(e) => {
            // A key the user configured explicitly must not silently fall back to a password prompt.
            return Err(Error::Invalid(format!("cannot load private key {}: {e}", path.display())));
        }
    }
    for _ in 0..3 {
        let kind = PromptKind::KeyPassphrase { path: path.display().to_string() };
        let PromptAnswer::Secret(passphrase) = prompter.prompt(kind).await else {
            return Err(Error::Cancelled);
        };
        match russh::keys::load_secret_key(path, Some(&passphrase.0)) {
            Ok(k) => return Ok(Some(k)),
            Err(e) => tracing::warn!("cannot decrypt key {}: {e}", path.display()),
        }
    }
    Err(Error::Auth(format!("wrong passphrase for private key {}", path.display())))
}

fn allowed(methods: &Option<MethodSet>, kind: MethodKind) -> bool {
    methods.as_ref().is_none_or(|m| m.contains(&kind))
}

/// Track the server's remaining methods; `true` on success.
fn record(result: AuthResult, methods: &mut Option<MethodSet>) -> bool {
    match result {
        AuthResult::Success => true,
        AuthResult::Failure { remaining_methods, .. } => {
            *methods = Some(remaining_methods);
            false
        }
    }
}

async fn rsa_hash(handle: &Handle<ClientHandler>, key: &PublicKey) -> Option<russh::keys::HashAlg> {
    if !key.algorithm().is_rsa() {
        return None;
    }
    // `None` from the server means "no ext-info": assume rsa-sha2-256 (ssh-rsa/SHA-1 is refused by
    // OpenSSH >= 8.8). An explicit `Some(None)` means the server only accepts ssh-rsa.
    match handle.best_supported_rsa_hash().await.ok().flatten() {
        Some(hash) => hash,
        None => Some(russh::keys::HashAlg::Sha256),
    }
}

/// Auth order: configured key file → agent → keyboard-interactive → password, honoring the session flags.
async fn authenticate(
    handle: &mut Handle<ClientHandler>,
    hop: &Hop,
    session: &Session,
    prompter: &dyn Prompter,
) -> Result<()> {
    let o = &session.ssh;
    let user = hop.user.as_str();
    let mut methods: Option<MethodSet> = None;

    // An explicitly configured key goes first: trying many agent identities first can exhaust the
    // server's MaxAuthTries before the configured key is offered.
    if let Some(path) = &hop.key_file
        && allowed(&methods, MethodKind::PublicKey)
        && let Some(key) = load_key(&expand_home(path), prompter).await?
    {
        let hash = rsa_hash(handle, key.public_key()).await;
        let r = handle
            .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
            .await
            .map_err(conn_err)?;
        if record(r, &mut methods) {
            return Ok(());
        }
    }

    if o.use_agent {
        match connect_agent().await {
            Ok(mut agent) => {
                for id in agent.request_identities().await.unwrap_or_default() {
                    if !allowed(&methods, MethodKind::PublicKey) {
                        break;
                    }
                    let key = id.public_key().into_owned();
                    let hash = rsa_hash(handle, &key).await;
                    match handle.authenticate_publickey_with(user, key, hash, &mut agent).await {
                        Ok(r) => {
                            if record(r, &mut methods) {
                                return Ok(());
                            }
                        }
                        Err(e) => tracing::debug!("agent auth: {e:?}"),
                    }
                }
            }
            Err(e) => tracing::debug!("no ssh agent: {e}"),
        }
    }

    if o.allow_keyboard_interactive && allowed(&methods, MethodKind::KeyboardInteractive) {
        let mut resp = handle.authenticate_keyboard_interactive_start(user, None).await.map_err(conn_err)?;
        loop {
            match resp {
                KeyboardInteractiveAuthResponse::Success => return Ok(()),
                KeyboardInteractiveAuthResponse::Failure { remaining_methods, .. } => {
                    methods = Some(remaining_methods);
                    break;
                }
                KeyboardInteractiveAuthResponse::InfoRequest { name, instructions, prompts } => {
                    let answers = if prompts.is_empty() {
                        Vec::new()
                    } else {
                        let kind = PromptKind::KeyboardInteractive {
                            name,
                            instructions,
                            prompts: prompts.into_iter().map(|p| (p.prompt, p.echo)).collect(),
                        };
                        match prompter.prompt(kind).await {
                            PromptAnswer::Responses(r) => r.into_iter().map(|s| s.0).collect(),
                            _ => return Err(Error::Cancelled),
                        }
                    };
                    resp = handle.authenticate_keyboard_interactive_respond(answers).await.map_err(conn_err)?;
                }
            }
        }
    }

    if o.allow_password && allowed(&methods, MethodKind::Password) {
        let mut retry = false;
        loop {
            let kind = PromptKind::Password { user: user.to_owned(), host: hop.host.clone(), retry };
            let PromptAnswer::Secret(password) = prompter.prompt(kind).await else {
                return Err(Error::Cancelled);
            };
            // The rejected password is dropped here; it is never stored.
            let r = handle.authenticate_password(user, password.0).await.map_err(conn_err)?;
            if record(r, &mut methods) {
                return Ok(());
            }
            if handle.is_closed() || !allowed(&methods, MethodKind::Password) {
                break;
            }
            retry = true;
        }
    }

    Err(Error::Auth(format!("{user}@{}: no authentication method succeeded", hop.host)))
}

/// Single-quote for POSIX shells.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Exec command for the session's startup directory and remote command; `None` runs the login shell.
fn startup_command(session: &Session) -> Option<String> {
    let o = &session.ssh;
    let cmd = o.remote_command.as_deref().filter(|c| !c.trim().is_empty());
    match (o.startup_dir.as_deref().filter(|d| !d.is_empty()), cmd) {
        (None, cmd) => cmd.map(str::to_owned),
        (Some(dir), Some(cmd)) => Some(format!("cd -- {} && {cmd}", sh_quote(dir))),
        (Some(dir), None) => Some(format!("cd -- {}; exec \"${{SHELL:-/bin/sh}}\" -l", sh_quote(dir))),
    }
}

/// Output of a shell channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellMsg {
    Data(Vec<u8>),
    ExitStatus(u32),
    Eof,
}

/// Interactive shell (or remote command) channel.
pub struct Shell {
    read: ChannelReadHalf,
    writer: ShellWriter,
}

/// Cloneable write half of a [`Shell`].
#[derive(Clone)]
pub struct ShellWriter {
    inner: Arc<ChannelWriteHalf<Msg>>,
}

impl Client {
    /// Open a PTY (`TERM=xterm-256color`) running the session's shell or remote command.
    pub async fn shell(&self, session: &Session, cols: u16, rows: u16) -> Result<Shell> {
        let o = &session.ssh;
        let ch = self.handle.channel_open_session().await.map_err(conn_err)?;
        if o.x11_forwarding {
            match &self.shared.x11 {
                Some(x) => {
                    ch.request_x11(false, false, "MIT-MAGIC-COOKIE-1", x.fake_hex(), x.display.screen)
                        .await
                        .map_err(conn_err)?;
                    self.shared.x11_requested.store(true, Ordering::Release);
                    self.lock_info().x11 = true;
                }
                None => {
                    tracing::warn!("X11 forwarding requested: {}", x11::UNAVAILABLE);
                    self.lock_info().x11_unavailable = Some(x11::UNAVAILABLE.into());
                }
            }
        }
        if o.agent_forwarding {
            ch.agent_forward(false).await.map_err(conn_err)?;
            self.lock_info().agent = true;
        }
        for (k, v) in &o.env {
            ch.set_env(false, k.as_str(), v.as_str()).await.map_err(conn_err)?;
        }
        ch.request_pty(false, "xterm-256color", u32::from(cols), u32::from(rows), 0, 0, &[]).await.map_err(conn_err)?;
        match startup_command(session) {
            Some(cmd) => ch.exec(false, cmd).await,
            None => ch.request_shell(false).await,
        }
        .map_err(conn_err)?;
        let (read, write) = ch.split();
        Ok(Shell { read, writer: ShellWriter { inner: Arc::new(write) } })
    }

    /// Open the `sftp` subsystem; returns a byte stream for `russh-sftp`.
    pub async fn sftp_stream(&self) -> Result<russh::ChannelStream<russh::client::Msg>> {
        let mut ch = self.handle.channel_open_session().await.map_err(conn_err)?;
        ch.request_subsystem(true, "sftp").await.map_err(conn_err)?;
        loop {
            match ch.wait().await {
                Some(ChannelMsg::Success) => return Ok(ch.into_stream()),
                Some(ChannelMsg::Failure) => {
                    return Err(Error::Unsupported("server refused the sftp subsystem".into()));
                }
                Some(_) => {}
                None => return Err(Error::Connection("channel closed while starting sftp".into())),
            }
        }
    }

    /// Run `command` on a one-shot exec channel (no PTY) of this connection and return its
    /// stdout (lossy UTF-8, capped at [`EXEC_OUTPUT_CAP`]); stderr and the exit status are ignored.
    pub async fn exec(&self, command: &str) -> Result<String> {
        let mut ch = self.handle.channel_open_session().await.map_err(conn_err)?;
        ch.exec(true, command).await.map_err(conn_err)?;
        let mut out = Vec::new();
        loop {
            match ch.wait().await {
                Some(ChannelMsg::Data { data }) if out.len() < EXEC_OUTPUT_CAP => out.extend_from_slice(&data),
                Some(ChannelMsg::Failure) => return Err(Error::Unsupported("server refused exec".into())),
                Some(ChannelMsg::Close) | None => break,
                Some(_) => {}
            }
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// Run `command` on an exec channel (no PTY) of this connection and return its stdin
    /// (write) and stdout (read) as one stream; stderr is dropped. Dropping the stream closes
    /// the channel, so the remote command reads EOF. A refused channel open (server session
    /// limit) is `Error::Busy`; a refused exec request is `Error::Unsupported`.
    pub async fn exec_stream(&self, command: &str) -> Result<russh::ChannelStream<russh::client::Msg>> {
        let mut ch = self.handle.channel_open_session().await.map_err(|e| match e {
            russh::Error::ChannelOpenFailure(reason) => {
                Error::Busy(format!("the server refused an extra channel ({reason:?})"))
            }
            e => conn_err(e),
        })?;
        ch.exec(true, command).await.map_err(conn_err)?;
        loop {
            match ch.wait().await {
                Some(ChannelMsg::Success) => return Ok(ch.into_stream()),
                Some(ChannelMsg::Failure) => return Err(Error::Unsupported("server refused exec".into())),
                Some(_) => {}
                None => return Err(Error::Connection("channel closed while starting a command".into())),
            }
        }
    }

    /// Open a direct-tcpip channel to `host:port` as seen from the server (VNC over SSH);
    /// no local listener is involved.
    pub async fn direct_tcpip(&self, host: &str, port: u16) -> Result<russh::ChannelStream<russh::client::Msg>> {
        let ch =
            self.handle.channel_open_direct_tcpip(host, u32::from(port), "127.0.0.1", 0).await.map_err(conn_err)?;
        Ok(ch.into_stream())
    }

    /// Start a tunnel; state changes are reported through `on_state`.
    pub async fn start_tunnel(
        &self,
        tunnel: &Tunnel,
        cancel: CancellationToken,
        on_state: Arc<dyn Fn(TunnelState) + Send + Sync>,
    ) -> Result<()> {
        tunnel::start(self.handle.clone(), self.shared.clone(), tunnel, cancel, on_state).await
    }

    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    /// Connection facts: negotiated algorithms, server id, host key, and the forwardings
    /// requested by [`Client::shell`] so far.
    pub fn info(&self) -> SshInfo {
        self.lock_info().clone()
    }

    /// The server's pre-auth banner text, sanitized for the terminal (CRLF line ends, no
    /// control sequences, capped at [`AUTH_BANNER_CAP`]); empty when none was sent. Taken once.
    pub fn take_auth_banner(&self) -> String {
        std::mem::take(&mut *self.shared.auth_banner.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn lock_info(&self) -> std::sync::MutexGuard<'_, SshInfo> {
        self.shared.info.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Shell {
    /// Next message; `None` when the channel is closed.
    pub async fn read(&mut self) -> Option<ShellMsg> {
        loop {
            match self.read.wait().await? {
                ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                    return Some(ShellMsg::Data(data.to_vec()));
                }
                ChannelMsg::ExitStatus { exit_status } => return Some(ShellMsg::ExitStatus(exit_status)),
                ChannelMsg::Eof => return Some(ShellMsg::Eof),
                ChannelMsg::Close => return None,
                _ => {}
            }
        }
    }

    pub fn writer(&self) -> ShellWriter {
        self.writer.clone()
    }
}

impl ShellWriter {
    pub async fn write(&self, bytes: &[u8]) -> Result<()> {
        self.inner.data(bytes).await.map_err(conn_err)
    }

    /// Send `window-change`.
    pub async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.inner.window_change(u32::from(cols), u32::from(rows), 0, 0).await.map_err(conn_err)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn windows_agent_pipe_prefers_ssh_auth_sock() {
        assert_eq!(super::windows_agent_pipe(None), super::WINDOWS_AGENT_PIPE);
        assert_eq!(super::windows_agent_pipe(Some("".into())), super::WINDOWS_AGENT_PIPE);
        assert_eq!(super::windows_agent_pipe(Some(r"\\.\pipe\custom".into())), r"\\.\pipe\custom");
    }

    struct NoPrompt;

    impl Prompter for NoPrompt {
        fn prompt(&self, _: PromptKind) -> rxt_core::BoxFut<'_, PromptAnswer> {
            Box::pin(async { PromptAnswer::Cancel })
        }
        fn host_key_changed(&self, _: &str, _: &str, _: &str) {}
    }

    #[tokio::test]
    async fn unloadable_configured_key_is_an_error_not_a_password_fallback() {
        let dir = std::env::temp_dir().join(format!("rxt-badkey-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("id_bad");
        std::fs::write(&path, b"not a private key").unwrap();
        let err = load_key(&path, &NoPrompt).await.unwrap_err();
        assert!(matches!(&err, Error::Invalid(m) if m.contains("cannot load private key")), "{err:?}");
        let missing = load_key(&dir.join("missing"), &NoPrompt).await.unwrap_err();
        assert!(matches!(missing, Error::Invalid(_)));
        std::fs::remove_dir_all(&dir).unwrap();
    }
    use super::*;

    #[test]
    fn legacy_algorithms_come_after_safe_defaults() {
        let p = preferred(false);
        let d = Preferred::DEFAULT;
        assert_eq!(&p.kex[..d.kex.len()], &d.kex[..]);
        assert!(p.kex[d.kex.len()..].contains(&kex::DH_G14_SHA1));
        assert!(p.kex.contains(&kex::DH_G1_SHA1));
        assert_eq!(&p.cipher[..d.cipher.len()], &d.cipher[..]);
        assert!(p.cipher.contains(&cipher::AES_128_CBC));
        assert_eq!(&p.mac[..d.mac.len()], &d.mac[..]);
        assert!(p.mac.contains(&mac::HMAC_SHA1));
    }

    #[test]
    fn startup_command_quotes_dir() {
        let mut s = Session::default();
        assert_eq!(startup_command(&s), None);
        s.ssh.startup_dir = Some("/tmp/it's here".into());
        assert_eq!(startup_command(&s).unwrap(), r#"cd -- '/tmp/it'\''s here'; exec "${SHELL:-/bin/sh}" -l"#);
        s.ssh.remote_command = Some("uname -a".into());
        assert_eq!(startup_command(&s).unwrap(), r"cd -- '/tmp/it'\''s here' && uname -a");
        s.ssh.startup_dir = None;
        assert_eq!(startup_command(&s).unwrap(), "uname -a");
    }

    #[test]
    fn hops_chain_jump_hosts_first() {
        let mut s = Session::new(rxt_core::Protocol::Ssh, "db.internal", Some("bob".into()), None);
        s.ssh.jump_hosts.push(rxt_core::JumpHost { host: "bastion".into(), ..Default::default() });
        let h = hops(&s);
        assert_eq!(
            h.iter().map(|h| (h.host.as_str(), h.port, h.user.as_str())).collect::<Vec<_>>(),
            [("bastion", 22, "bob"), ("db.internal", 22, "bob")]
        );
    }

    #[test]
    fn trust_store_dedups_and_filters() {
        let t = TrustStore::default();
        let k = PublicKey::new(
            russh::keys::ssh_key::public::KeyData::Ed25519(russh::keys::ssh_key::public::Ed25519PublicKey([1; 32])),
            "",
        );
        t.add("h", 22, k.clone());
        t.add("h", 22, k.clone());
        assert_eq!(t.keys("H", 22).len(), 1);
        assert!(t.keys("h", 2222).is_empty());
    }

    #[test]
    fn trust_store_saves_and_loads_through_backing() {
        #[derive(Default)]
        struct Mem(Mutex<Vec<(String, u16, String)>>);
        impl HostKeyRecords for Mem {
            fn load(&self, host: &str, port: u16) -> Vec<String> {
                let v = self.0.lock().unwrap();
                v.iter().filter(|r| r.0 == host && r.1 == port).map(|r| r.2.clone()).collect()
            }
            fn save(&self, host: &str, port: u16, key: &str) {
                self.0.lock().unwrap().push((host.into(), port, key.into()));
            }
        }
        let mem = Arc::new(Mem::default());
        let k = PublicKey::new(
            russh::keys::ssh_key::public::KeyData::Ed25519(russh::keys::ssh_key::public::Ed25519PublicKey([2; 32])),
            "",
        );
        TrustStore::persistent(mem.clone()).add("h", 22, k.clone());
        let fresh = TrustStore::persistent(mem);
        assert_eq!(
            fresh.keys("h", 22).iter().map(|k| k.key_data().clone()).collect::<Vec<_>>(),
            [k.key_data().clone()]
        );
    }

    #[test]
    fn compression_prefers_openssh_zlib() {
        let mut s = Session::default();
        assert_eq!(config(&s).preferred.compression.as_ref(), COMPRESSION_OFF);
        s.ssh.compression = true;
        s.ssh.keepalive_secs = 15;
        let c = config(&s);
        assert_eq!(c.preferred.compression.first(), Some(&compression::ZLIB_LEGACY));
        assert_eq!(c.keepalive_interval, Some(Duration::from_secs(15)));
    }
}
