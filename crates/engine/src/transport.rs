//! Seam between the engine and the protocol crates, so unit tests can drive the engine
//! with an in-memory transport. The real implementation is a thin forward to
//! `rxt_ssh`/`rxt_sftp`/`rxt_ftp`/`rxt_rdp`/`rxt_vnc`/`rxt_telnet`/`rxt_serial`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use rxt_core::{
    BoxFut, CertTrust, DesktopEvent, DesktopInput, Error, FileOps, JumpHost, PromptKind, Protocol, Result, Session,
    Tunnel, TunnelState,
};
use rxt_platform::SerialPorts;
use rxt_ssh::{ConnectContext, HostKeyRecords, Prompter, ShellMsg, TrustStore};
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

use crate::Records;

pub(crate) type OnTunnelState = Arc<dyn Fn(TunnelState) + Send + Sync>;
pub(crate) type ShellPair = (Box<dyn ShellRead>, Box<dyn ShellWrite>);
/// Non-blocking desktop input sink (`rxt_rdp`/`rxt_vnc` `InputSender::send`).
pub(crate) type DesktopSend = Arc<dyn Fn(DesktopInput) -> Result<()> + Send + Sync>;
pub(crate) type DesktopPair = (Box<dyn DesktopRead>, DesktopSend);
/// Stdout and stdin of a long-lived exec channel; dropping both closes the channel.
pub(crate) type ExecStream =
    (Box<dyn tokio::io::AsyncRead + Send + Unpin>, Box<dyn tokio::io::AsyncWrite + Send + Unpin>);

/// Opens authenticated connections.
pub(crate) trait Transport: Send + Sync {
    fn connect(
        &self,
        session: Session,
        prompter: Arc<dyn Prompter>,
        cancel: CancellationToken,
    ) -> BoxFut<'static, Result<Arc<dyn Link>>>;

    /// FTP/FTPS login; the result backs a file-only tab.
    fn connect_files(
        &self,
        session: Session,
        prompter: Arc<dyn Prompter>,
        cancel: CancellationToken,
    ) -> BoxFut<'static, Result<Arc<dyn FileOps>>>;

    /// RDP/VNC login; `size` is the initial desktop size (RDP).
    fn connect_desktop(
        &self,
        session: Session,
        prompter: Arc<dyn Prompter>,
        cancel: CancellationToken,
        size: (u32, u32),
    ) -> BoxFut<'static, Result<DesktopPair>>;
}

/// Event stream of a connected remote desktop; dropping it ends the session.
pub(crate) trait DesktopRead: Send {
    fn next(&mut self) -> BoxFut<'_, Option<Result<DesktopEvent>>>;
}

/// One authenticated connection.
pub(crate) trait Link: Send + Sync {
    fn shell<'a>(&'a self, session: &'a Session, cols: u16, rows: u16) -> BoxFut<'a, Result<ShellPair>>;
    fn file_ops(&self) -> BoxFut<'_, Result<Arc<dyn FileOps>>>;
    fn start_tunnel<'a>(
        &'a self,
        tunnel: &'a Tunnel,
        cancel: CancellationToken,
        on_state: OnTunnelState,
    ) -> BoxFut<'a, Result<()>>;
    /// Serial line control of this connection; `None` for non-serial links.
    fn line_control(&self) -> Option<Arc<dyn LineControl>> {
        None
    }
    /// Connection facts for the session banner (SSH only).
    fn ssh_info(&self) -> Option<rxt_core::SshInfo> {
        None
    }
    /// Sanitized server pre-auth banner, taken once; empty when none (SSH only).
    fn take_auth_banner(&self) -> String {
        String::new()
    }
    /// Run a one-shot command on this connection and return its stdout (SSH only).
    fn exec<'a>(&'a self, command: &'a str) -> BoxFut<'a, Result<String>> {
        let _ = command;
        Box::pin(async { Err(Error::Unsupported("remote commands on this connection".into())) })
    }
    /// Run a long-lived command on this connection with its stdin and stdout (SSH only).
    fn exec_stream<'a>(&'a self, command: &'a str) -> BoxFut<'a, Result<ExecStream>> {
        let _ = command;
        Box::pin(async { Err(Error::Unsupported("remote commands on this connection".into())) })
    }
}

pub(crate) trait ShellRead: Send {
    fn read(&mut self) -> BoxFut<'_, Option<ShellMsg>>;
    /// The last data read was local echo of the user's input (Serial), not the device's output.
    fn last_was_local_echo(&self) -> bool {
        false
    }
    /// Output may carry a credential the app typed itself (Telnet auto-login): not captured.
    fn capture_paused(&self) -> bool {
        false
    }
    /// Why the stream ended, once `read` returned `None` (serial: `Error::Gone` on unplug).
    fn end_reason(&mut self) -> Option<Error> {
        None
    }
}

/// Break and modem lines of a serial connection.
pub(crate) trait LineControl: Send + Sync {
    fn send_break(&self, ms: u64) -> BoxFut<'_, Result<()>>;
    fn set_lines(&self, dtr: Option<bool>, rts: Option<bool>) -> BoxFut<'_, Result<()>>;
}

pub(crate) trait ShellWrite: Send + Sync {
    fn write<'a>(&'a self, bytes: &'a [u8]) -> BoxFut<'a, Result<()>>;
    fn resize(&self, cols: u16, rows: u16) -> BoxFut<'_, Result<()>>;
}

/// A live connection of a tab plus its file operations: the lazily opened SFTP channel of an
/// SSH link, or the FTP connection itself (no link).
pub(crate) struct Conn {
    pub link: Option<Arc<dyn Link>>,
    pub sftp: OnceCell<Arc<dyn FileOps>>,
}

impl Conn {
    pub fn new(link: Arc<dyn Link>) -> Self {
        Self { link: Some(link), sftp: OnceCell::new() }
    }

    pub fn files(fs: Arc<dyn FileOps>) -> Self {
        Self { link: None, sftp: OnceCell::new_with(Some(fs)) }
    }
}

/// TLS certificate trust records (FTPS, RDP): `host:port` -> SHA-256 fingerprint, in memory
/// and in the injected [`Records`] when present.
#[derive(Default)]
pub(crate) struct CertStore {
    records: Mutex<HashMap<(String, u16), String>>,
    backing: Option<Arc<dyn Records>>,
}

impl CertTrust for CertStore {
    fn get(&self, host: &str, port: u16) -> Option<String> {
        let host = host.to_ascii_lowercase();
        let mem = self.records.lock().get(&(host.clone(), port)).cloned();
        mem.or_else(|| self.backing.as_ref()?.certificate(&host, port))
    }

    fn save(&self, host: &str, port: u16, fingerprint: &str) {
        let host = host.to_ascii_lowercase();
        if let Some(b) = &self.backing {
            b.save_certificate(&host, port, fingerprint);
        }
        self.records.lock().insert((host, port), fingerprint.to_owned());
    }
}

/// SSH host keys through [`Records`].
struct HostKeys(Arc<dyn Records>);

impl HostKeyRecords for HostKeys {
    fn load(&self, host: &str, port: u16) -> Vec<String> {
        self.0.host_keys(host, port)
    }
    fn save(&self, host: &str, port: u16, key: &str) {
        self.0.save_host_key(host, port, key);
    }
}

/// Real transport over the protocol crates.
pub(crate) struct Real {
    trust: Arc<TrustStore>,
    certs: Arc<CertStore>,
    known_hosts: PathBuf,
    records: Option<Arc<dyn Records>>,
    serial_ports: Option<Arc<dyn SerialPorts>>,
}

impl Real {
    pub fn new(
        known_hosts: PathBuf,
        records: Option<Arc<dyn Records>>,
        serial_ports: Option<Arc<dyn SerialPorts>>,
    ) -> Self {
        let trust = match &records {
            Some(r) => TrustStore::persistent(Arc::new(HostKeys(r.clone()))),
            None => TrustStore::default(),
        };
        let certs = CertStore { records: Mutex::default(), backing: records.clone() };
        Self { trust: Arc::new(trust), certs: Arc::new(certs), known_hosts, records, serial_ports }
    }

    /// Saved password for Telnet auto-login: only when the session opted in and has a user.
    fn telnet_password(&self, session: &Session) -> Option<rxt_core::Secret> {
        let user = session.user.clone().filter(|u| session.telnet.auto_login && !u.is_empty())?;
        let kind = PromptKind::Password { user, host: session.host.clone(), retry: false };
        self.records.as_ref()?.credential(session, &kind)
    }

    fn ssh_ctx(&self, prompter: Arc<dyn Prompter>, cancel: CancellationToken) -> ConnectContext {
        ConnectContext { prompter, trust: self.trust.clone(), known_hosts: self.known_hosts.clone(), cancel }
    }
}

/// SSH session to a VNC gateway.
fn gateway_session(j: &JumpHost) -> Session {
    let user = (!j.user.is_empty()).then(|| j.user.clone());
    let mut s = Session::new(Protocol::Ssh, j.host.clone(), user, (j.port != 0).then_some(j.port));
    s.ssh.key_file = j.key_file.clone();
    s
}

impl Transport for Real {
    fn connect(
        &self,
        session: Session,
        prompter: Arc<dyn Prompter>,
        cancel: CancellationToken,
    ) -> BoxFut<'static, Result<Arc<dyn Link>>> {
        match session.protocol {
            Protocol::Telnet => {
                let password = self.telnet_password(&session);
                let link = TelnetLink { session, prompter, password, cancel };
                return Box::pin(async move { Ok(Arc::new(link) as Arc<dyn Link>) });
            }
            Protocol::Serial => {
                let ports = self.serial_ports.clone();
                return Box::pin(async move {
                    let ports = ports.ok_or_else(|| Error::Unsupported("serial ports on this platform".into()))?;
                    let serial = rxt_serial::open(ports, &session.serial, cancel)?;
                    Ok(Arc::new(SerialLink::new(serial)) as Arc<dyn Link>)
                });
            }
            _ => {}
        }
        let ctx = self.ssh_ctx(prompter, cancel);
        Box::pin(async move { Ok(Arc::new(rxt_ssh::connect(&session, ctx).await?) as Arc<dyn Link>) })
    }

    fn connect_files(
        &self,
        session: Session,
        prompter: Arc<dyn Prompter>,
        cancel: CancellationToken,
    ) -> BoxFut<'static, Result<Arc<dyn FileOps>>> {
        let ctx = rxt_ftp::Context { prompter, trust: self.certs.clone(), cancel };
        Box::pin(async move { Ok(Arc::new(rxt_ftp::connect(&session, ctx).await?) as Arc<dyn FileOps>) })
    }

    fn connect_desktop(
        &self,
        session: Session,
        prompter: Arc<dyn Prompter>,
        cancel: CancellationToken,
        size: (u32, u32),
    ) -> BoxFut<'static, Result<DesktopPair>> {
        let trust: Arc<dyn CertTrust> = self.certs.clone();
        let ssh = self.ssh_ctx(prompter.clone(), cancel.clone());
        Box::pin(async move {
            if session.protocol == Protocol::Rdp {
                let ctx = rxt_rdp::Context { prompter, trust, cancel };
                let desktop = rxt_rdp::connect(&session, ctx, size).await?;
                let input = desktop.input();
                let send: DesktopSend = Arc::new(move |i| input.send(i));
                return Ok((Box::new(desktop) as Box<dyn DesktopRead>, send));
            }
            // VNC, directly or through an SSH gateway (direct-tcpip, no local listener).
            let (gateway, stream) = match &session.vnc.via_ssh {
                Some(j) => {
                    let client = rxt_ssh::connect(&gateway_session(j), ssh).await?;
                    let port = if session.port == 0 { Protocol::Vnc.default_port() } else { session.port };
                    let stream = client.direct_tcpip(&session.host, port).await?;
                    (Some(client), Some(Box::new(stream) as Box<dyn rxt_vnc::Stream>))
                }
                None => (None, None),
            };
            let ctx = rxt_vnc::Context { prompter, trust, cancel };
            let desktop = rxt_vnc::connect(&session, ctx, stream).await?;
            let input = desktop.input();
            let send: DesktopSend = Arc::new(move |i| input.send(i));
            Ok((Box::new(Vnc { desktop, _gateway: gateway }) as Box<dyn DesktopRead>, send))
        })
    }
}

impl DesktopRead for rxt_rdp::Desktop {
    fn next(&mut self) -> BoxFut<'_, Option<Result<DesktopEvent>>> {
        Box::pin(rxt_rdp::Desktop::next(self))
    }
}

/// VNC desktop plus the SSH gateway connection carrying it, kept alive together.
struct Vnc {
    desktop: rxt_vnc::Desktop,
    _gateway: Option<rxt_ssh::Client>,
}

impl DesktopRead for Vnc {
    fn next(&mut self) -> BoxFut<'_, Option<Result<DesktopEvent>>> {
        Box::pin(self.desktop.next())
    }
}

impl Link for rxt_ssh::Client {
    fn shell<'a>(&'a self, session: &'a Session, cols: u16, rows: u16) -> BoxFut<'a, Result<ShellPair>> {
        Box::pin(async move {
            let shell = rxt_ssh::Client::shell(self, session, cols, rows).await?;
            let writer = shell.writer();
            Ok((Box::new(shell) as Box<dyn ShellRead>, Box::new(writer) as Box<dyn ShellWrite>))
        })
    }

    fn file_ops(&self) -> BoxFut<'_, Result<Arc<dyn FileOps>>> {
        Box::pin(async move { Ok(Arc::new(rxt_sftp::Sftp::open(self).await?) as Arc<dyn FileOps>) })
    }

    fn start_tunnel<'a>(
        &'a self,
        tunnel: &'a Tunnel,
        cancel: CancellationToken,
        on_state: OnTunnelState,
    ) -> BoxFut<'a, Result<()>> {
        Box::pin(rxt_ssh::Client::start_tunnel(self, tunnel, cancel, on_state))
    }

    fn exec<'a>(&'a self, command: &'a str) -> BoxFut<'a, Result<String>> {
        Box::pin(rxt_ssh::Client::exec(self, command))
    }

    fn exec_stream<'a>(&'a self, command: &'a str) -> BoxFut<'a, Result<ExecStream>> {
        Box::pin(async move {
            let (read, write) = tokio::io::split(rxt_ssh::Client::exec_stream(self, command).await?);
            Ok((Box::new(read) as Box<dyn tokio::io::AsyncRead + Send + Unpin>, Box::new(write) as _))
        })
    }

    fn ssh_info(&self) -> Option<rxt_core::SshInfo> {
        Some(self.info())
    }

    fn take_auth_banner(&self) -> String {
        rxt_ssh::Client::take_auth_banner(self)
    }
}

impl ShellRead for rxt_ssh::Shell {
    fn read(&mut self) -> BoxFut<'_, Option<ShellMsg>> {
        Box::pin(rxt_ssh::Shell::read(self))
    }
}

impl ShellWrite for rxt_ssh::ShellWriter {
    fn write<'a>(&'a self, bytes: &'a [u8]) -> BoxFut<'a, Result<()>> {
        Box::pin(rxt_ssh::ShellWriter::write(self, bytes))
    }

    fn resize(&self, cols: u16, rows: u16) -> BoxFut<'_, Result<()>> {
        Box::pin(rxt_ssh::ShellWriter::resize(self, cols, rows))
    }
}

/// Telnet TCP connect limit (the protocol crate has none).
const TELNET_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Telnet connects in `shell`, where the initial window size (NAWS) is known.
struct TelnetLink {
    session: Session,
    prompter: Arc<dyn Prompter>,
    /// Saved password for opted-in auto-login.
    password: Option<rxt_core::Secret>,
    cancel: CancellationToken,
}

impl Link for TelnetLink {
    fn shell<'a>(&'a self, session: &'a Session, cols: u16, rows: u16) -> BoxFut<'a, Result<ShellPair>> {
        let _ = session;
        Box::pin(async move {
            let ctx = rxt_telnet::Context {
                prompter: self.prompter.clone(),
                password: self.password.as_ref().map(|p| secrecy::SecretString::from(p.0.clone())),
                cancel: self.cancel.clone(),
            };
            let connect = rxt_telnet::connect(&self.session, ctx, cols, rows);
            let telnet = tokio::time::timeout(TELNET_CONNECT_TIMEOUT, connect).await.map_err(|_| {
                Error::Connection(format!("{}:{}: connection timed out", self.session.host, self.session.port))
            })??;
            let writer = telnet.writer();
            Ok((Box::new(telnet) as Box<dyn ShellRead>, Box::new(writer) as Box<dyn ShellWrite>))
        })
    }

    fn file_ops(&self) -> BoxFut<'_, Result<Arc<dyn FileOps>>> {
        Box::pin(async { Err(Error::Unsupported("file browsing on Telnet sessions".into())) })
    }

    fn start_tunnel<'a>(&'a self, _: &'a Tunnel, _: CancellationToken, _: OnTunnelState) -> BoxFut<'a, Result<()>> {
        Box::pin(async { Err(Error::Unsupported("tunnels over Telnet".into())) })
    }
}

impl ShellRead for rxt_telnet::TelnetSession {
    fn read(&mut self) -> BoxFut<'_, Option<ShellMsg>> {
        Box::pin(async move { rxt_telnet::TelnetSession::read(self).await.map(ShellMsg::Data) })
    }

    fn capture_paused(&self) -> bool {
        self.has_auto_login()
    }
}

impl ShellWrite for rxt_telnet::TelnetWriter {
    fn write<'a>(&'a self, bytes: &'a [u8]) -> BoxFut<'a, Result<()>> {
        Box::pin(rxt_telnet::TelnetWriter::write(self, bytes))
    }

    fn resize(&self, cols: u16, rows: u16) -> BoxFut<'_, Result<()>> {
        Box::pin(rxt_telnet::TelnetWriter::resize(self, cols, rows))
    }
}

/// An open serial port; `shell` hands out the session once.
struct SerialLink {
    session: Mutex<Option<rxt_serial::SerialSession>>,
    writer: rxt_serial::SerialWriter,
}

impl SerialLink {
    fn new(session: rxt_serial::SerialSession) -> Self {
        Self { writer: session.writer(), session: Mutex::new(Some(session)) }
    }
}

impl Link for SerialLink {
    fn shell<'a>(&'a self, _: &'a Session, _: u16, _: u16) -> BoxFut<'a, Result<ShellPair>> {
        let session = self.session.lock().take();
        Box::pin(async move {
            let session = session.ok_or_else(|| Error::Invalid("serial port already in use by this tab".into()))?;
            let writer = session.writer();
            Ok((
                Box::new(SerialReader { session, end: None, echo: false }) as Box<dyn ShellRead>,
                Box::new(writer) as _,
            ))
        })
    }

    fn file_ops(&self) -> BoxFut<'_, Result<Arc<dyn FileOps>>> {
        Box::pin(async { Err(Error::Unsupported("file browsing on serial sessions".into())) })
    }

    fn start_tunnel<'a>(&'a self, _: &'a Tunnel, _: CancellationToken, _: OnTunnelState) -> BoxFut<'a, Result<()>> {
        Box::pin(async { Err(Error::Unsupported("tunnels over serial".into())) })
    }

    fn line_control(&self) -> Option<Arc<dyn LineControl>> {
        Some(Arc::new(self.writer.clone()))
    }
}

struct SerialReader {
    session: rxt_serial::SerialSession,
    end: Option<Error>,
    echo: bool,
}

impl ShellRead for SerialReader {
    fn read(&mut self) -> BoxFut<'_, Option<ShellMsg>> {
        Box::pin(async move {
            match self.session.read_tagged().await? {
                Ok((bytes, echo)) => {
                    self.echo = echo;
                    Some(ShellMsg::Data(bytes))
                }
                Err(e) => {
                    self.end = Some(e);
                    None
                }
            }
        })
    }

    fn last_was_local_echo(&self) -> bool {
        self.echo
    }

    fn end_reason(&mut self) -> Option<Error> {
        self.end.take()
    }
}

impl ShellWrite for rxt_serial::SerialWriter {
    fn write<'a>(&'a self, bytes: &'a [u8]) -> BoxFut<'a, Result<()>> {
        Box::pin(rxt_serial::SerialWriter::write(self, bytes))
    }

    /// Serial lines have no window size.
    fn resize(&self, _: u16, _: u16) -> BoxFut<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

impl LineControl for rxt_serial::SerialWriter {
    fn send_break(&self, ms: u64) -> BoxFut<'_, Result<()>> {
        Box::pin(rxt_serial::SerialWriter::send_break(self, ms))
    }

    fn set_lines(&self, dtr: Option<bool>, rts: Option<bool>) -> BoxFut<'_, Result<()>> {
        Box::pin(rxt_serial::SerialWriter::set_lines(self, dtr, rts))
    }
}
