//! Session, folder and tunnel definitions.

use serde::{Deserialize, Serialize};

/// Stable session identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct SessionId(pub u64);

/// Stable folder identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct FolderId(pub u64);

/// Session protocol. Phase 2 adds Mosh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    #[default]
    Ssh,
    Telnet,
    Sftp,
    Ftp,
    Ftps,
    Rdp,
    Vnc,
    Serial,
    /// WSL distribution shell (Windows only). Saved and serialized on every platform so shared
    /// databases and exports keep it, but offered and started only where [`crate::Protocol::Wsl`]
    /// has a backend (see the Windows-only availability gate).
    Wsl,
    /// Local shell in a pseudo-terminal ("+" in the tab strip). Never saved: outside [`Self::ALL`],
    /// refused by the store and import.
    Local,
}

impl Protocol {
    /// Protocols of saved sessions, in UI order ([`Self::Local`] is not one). [`Self::Wsl`] is
    /// saved but offered only where it has a backend (Windows).
    pub const ALL: [Protocol; 9] =
        [Self::Ssh, Self::Telnet, Self::Sftp, Self::Ftp, Self::Ftps, Self::Rdp, Self::Vnc, Self::Serial, Self::Wsl];

    /// Default TCP port. FTPS uses the implicit-TLS port; Serial has none (0).
    pub fn default_port(self) -> u16 {
        match self {
            Self::Ssh | Self::Sftp => 22,
            Self::Telnet => 23,
            Self::Serial | Self::Local | Self::Wsl => 0,
            Self::Ftp => 21,
            Self::Ftps => 990,
            Self::Rdp => 3389,
            Self::Vnc => 5900,
        }
    }

    /// URL scheme used by quick connect.
    pub fn scheme(self) -> &'static str {
        match self {
            Self::Ssh => "ssh",
            Self::Telnet => "telnet",
            Self::Serial => "serial",
            Self::Sftp => "sftp",
            Self::Ftp => "ftp",
            Self::Ftps => "ftps",
            Self::Rdp => "rdp",
            Self::Vnc => "vnc",
            Self::Wsl => "wsl",
            Self::Local => "local",
        }
    }

    /// Parse a quick-connect scheme (case-insensitive).
    pub fn from_scheme(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.scheme().eq_ignore_ascii_case(s))
    }

    /// Icon name under `assets/icons/`.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Telnet => "terminal",
            Self::Serial => "plug",
            Self::Local => "terminal-shell",
            Self::Wsl => "tux",
            _ => self.scheme(),
        }
    }

    /// Whether tabs of this protocol host a terminal.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Ssh | Self::Telnet | Self::Serial | Self::Local | Self::Wsl)
    }

    /// Whether the protocol sends everything, credentials included, in clear text.
    pub fn is_unencrypted(self) -> bool {
        matches!(self, Self::Ftp | Self::Telnet)
    }
}

/// Terminal cursor shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CursorShape {
    Block,
    Underline,
    /// Thin vertical bar (the default).
    #[default]
    Beam,
}

/// Terminal appearance; `None` fields inherit the global defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TerminalPrefs {
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub cursor_shape: Option<CursorShape>,
    pub cursor_blink: Option<bool>,
    /// Terminal scheme name; `None` follows the UI theme.
    pub scheme: Option<String>,
    pub scrollback_lines: Option<usize>,
    pub log_file: Option<String>,
    /// Log raw bytes (true) or plain text without escape sequences (false).
    pub log_raw: bool,
}

impl TerminalPrefs {
    /// Fill unset fields from `global`.
    pub fn resolve(&self, global: &TerminalPrefs) -> TerminalPrefs {
        TerminalPrefs {
            font_family: self.font_family.clone().or_else(|| global.font_family.clone()),
            font_size: self.font_size.or(global.font_size),
            cursor_shape: self.cursor_shape.or(global.cursor_shape),
            cursor_blink: self.cursor_blink.or(global.cursor_blink),
            scheme: self.scheme.clone().or_else(|| global.scheme.clone()),
            scrollback_lines: self.scrollback_lines.or(global.scrollback_lines),
            log_file: self.log_file.clone().or_else(|| global.log_file.clone()),
            log_raw: self.log_raw,
        }
    }
}

/// One hop of a jump-host chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct JumpHost {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub key_file: Option<String>,
}

/// SSH-specific options. Defaults are the safe ones from the spec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SshOptions {
    pub key_file: Option<String>,
    pub use_agent: bool,
    pub allow_keyboard_interactive: bool,
    pub allow_password: bool,
    pub jump_hosts: Vec<JumpHost>,
    pub x11_forwarding: bool,
    pub agent_forwarding: bool,
    pub compression: bool,
    /// Keepalive interval in seconds; 0 disables it.
    pub keepalive_secs: u32,
    pub remote_command: Option<String>,
    pub keep_open_after_command: bool,
    pub startup_dir: Option<String>,
    pub env: Vec<(String, String)>,
    pub auto_reconnect: bool,
    pub follow_terminal_folder: bool,
    /// Remote monitoring bar: `None` follows the global setting.
    pub remote_monitoring: Option<bool>,
    /// `TERM` requested with the PTY; empty means `xterm-256color`.
    pub terminal_type: String,
}

impl Default for SshOptions {
    fn default() -> Self {
        Self {
            key_file: None,
            use_agent: true,
            allow_keyboard_interactive: true,
            allow_password: true,
            jump_hosts: Vec::new(),
            x11_forwarding: false,
            agent_forwarding: false,
            compression: false,
            keepalive_secs: 0,
            remote_command: None,
            keep_open_after_command: false,
            startup_dir: None,
            env: Vec::new(),
            auto_reconnect: false,
            follow_terminal_folder: true,
            remote_monitoring: None,
            terminal_type: "xterm-256color".into(),
        }
    }
}

/// FTP/FTPS options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FtpOptions {
    /// FTPS mode; ignored for plain FTP. `Protocol::Ftps` defaults to implicit TLS on port 990.
    pub tls: FtpTls,
    /// Passive (default) or active data connections.
    pub passive: bool,
    /// ASCII instead of binary (default) transfers.
    pub ascii: bool,
    pub startup_folder: Option<String>,
}

impl Default for FtpOptions {
    fn default() -> Self {
        Self { tls: FtpTls::Implicit, passive: true, ascii: false, startup_folder: None }
    }
}

/// FTPS TLS mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FtpTls {
    /// `AUTH TLS` on the control port.
    Explicit,
    #[default]
    Implicit,
}

/// RDP resolution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RdpResolution {
    /// Follow the tab size (display-control resize).
    #[default]
    FitToTab,
    Fixed {
        width: u32,
        height: u32,
    },
}

/// RDP options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RdpOptions {
    pub domain: Option<String>,
    pub resolution: RdpResolution,
    /// Keyboard layout id (e.g. 0x0409 US); 0 = server default.
    pub keyboard_layout: u32,
    pub clipboard: bool,
    /// Accept TLS 1.0/1.1 with legacy ciphers (old Windows servers such as 2008); less secure.
    pub allow_legacy_tls: bool,
    /// Fall back to Standard RDP Security (RC4, no TLS/NLA) when the server's Security Layer is
    /// "RDP"; insecure (no server authentication).
    pub allow_standard_security: bool,
}

impl Default for RdpOptions {
    fn default() -> Self {
        Self {
            domain: None,
            resolution: RdpResolution::FitToTab,
            keyboard_layout: 0,
            clipboard: true,
            allow_legacy_tls: false,
            allow_standard_security: false,
        }
    }
}

/// VNC options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VncOptions {
    pub view_only: bool,
    pub scale_to_fit: bool,
    pub clipboard: bool,
    /// Tunnel through this SSH gateway (direct-tcpip), with no local listener.
    pub via_ssh: Option<JumpHost>,
}

impl Default for VncOptions {
    fn default() -> Self {
        Self { view_only: false, scale_to_fit: true, clipboard: true, via_ssh: None }
    }
}

/// Byte sequence sent for Enter on a Telnet session (outside BINARY mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Newline {
    #[default]
    CrLf,
    CrNul,
    Cr,
}

impl Newline {
    pub fn bytes(self) -> &'static [u8] {
        match self {
            Self::CrLf => b"\r\n",
            Self::CrNul => b"\r\0",
            Self::Cr => b"\r",
        }
    }
}

/// Telnet / raw TCP options (design T6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TelnetOptions {
    /// Raw TCP: no negotiation and no IAC escaping.
    pub raw: bool,
    pub terminal_type: String,
    pub newline: Newline,
    /// Type the saved username/password at the prompts (opt-in).
    pub auto_login: bool,
    pub login_prompt: String,
    pub password_prompt: String,
}

impl Default for TelnetOptions {
    fn default() -> Self {
        Self {
            raw: false,
            terminal_type: "xterm-256color".into(),
            newline: Newline::CrLf,
            auto_login: false,
            login_prompt: "login:".into(),
            password_prompt: "password:".into(),
        }
    }
}

/// Serial data bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum DataBits {
    #[serde(rename = "5")]
    Five,
    #[serde(rename = "6")]
    Six,
    #[serde(rename = "7")]
    Seven,
    #[default]
    #[serde(rename = "8")]
    Eight,
}

impl DataBits {
    pub const ALL: [DataBits; 4] = [Self::Five, Self::Six, Self::Seven, Self::Eight];

    pub fn bits(self) -> u8 {
        match self {
            Self::Five => 5,
            Self::Six => 6,
            Self::Seven => 7,
            Self::Eight => 8,
        }
    }

    pub fn from_bits(n: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.bits() == n)
    }
}

/// Serial parity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Parity {
    #[default]
    None,
    Odd,
    Even,
    Mark,
    Space,
}

impl Parity {
    pub const ALL: [Parity; 5] = [Self::None, Self::Odd, Self::Even, Self::Mark, Self::Space];

    /// One-letter code as in `8N1`.
    pub fn letter(self) -> char {
        match self {
            Self::None => 'N',
            Self::Odd => 'O',
            Self::Even => 'E',
            Self::Mark => 'M',
            Self::Space => 'S',
        }
    }

    /// Parse a letter (`n`) or word (`none`), case-insensitive.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "n" | "none" => Some(Self::None),
            "o" | "odd" => Some(Self::Odd),
            "e" | "even" => Some(Self::Even),
            "m" | "mark" => Some(Self::Mark),
            "s" | "space" => Some(Self::Space),
            _ => None,
        }
    }
}

/// Serial stop bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum StopBits {
    #[default]
    #[serde(rename = "1")]
    One,
    #[serde(rename = "2")]
    Two,
}

impl StopBits {
    pub fn bits(self) -> u8 {
        match self {
            Self::One => 1,
            Self::Two => 2,
        }
    }
}

/// Serial flow control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FlowControl {
    #[default]
    None,
    /// RTS/CTS.
    Hardware,
    /// XON/XOFF.
    Software,
}

impl FlowControl {
    pub const ALL: [FlowControl; 3] = [Self::None, Self::Hardware, Self::Software];

    /// Parse `none`, `rtscts`/`hardware`/`hw`, `xonxoff`/`software`/`sw`.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().replace(['-', '_', '/'], "").as_str() {
            "none" | "n" => Some(Self::None),
            "rtscts" | "hardware" | "hw" => Some(Self::Hardware),
            "xonxoff" | "software" | "sw" => Some(Self::Software),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Hardware => "RTS/CTS",
            Self::Software => "XON/XOFF",
        }
    }
}

/// Serial line configuration (default 115200 8N1, no flow control).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct LineConfig {
    pub baud: u32,
    pub data_bits: DataBits,
    pub parity: Parity,
    pub stop_bits: StopBits,
    pub flow: FlowControl,
}

impl Default for LineConfig {
    fn default() -> Self {
        Self {
            baud: 115_200,
            data_bits: DataBits::Eight,
            parity: Parity::None,
            stop_bits: StopBits::One,
            flow: FlowControl::None,
        }
    }
}

impl std::fmt::Display for LineConfig {
    /// `115200 8N1`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}{}{}", self.baud, self.data_bits.bits(), self.parity.letter(), self.stop_bits.bits())
    }
}

/// Bytes sent for Enter on a serial session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SerialEnter {
    /// Most device consoles expect CR.
    #[default]
    Cr,
    Lf,
    CrLf,
}

impl SerialEnter {
    pub fn bytes(self) -> &'static [u8] {
        match self {
            Self::Cr => b"\r",
            Self::Lf => b"\n",
            Self::CrLf => b"\r\n",
        }
    }
}

/// One discovered serial port.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PortInfo {
    /// Device path, e.g. `/dev/ttyUSB0`.
    pub path: String,
    /// Stable `/dev/serial/by-id/...` link, when present.
    pub by_id: Option<String>,
    /// Human description (manufacturer / product).
    pub description: Option<String>,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    pub serial_number: Option<String>,
}

/// Serial session options (design S6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SerialOptions {
    /// Device path, e.g. `/dev/ttyUSB0`.
    pub port: String,
    pub line: LineConfig,
    pub enter: SerialEnter,
    /// Expand received LF to CR LF.
    pub lf_to_crlf: bool,
    pub local_echo: bool,
    /// Initial DTR/RTS state; `None` leaves the driver default.
    pub dtr: Option<bool>,
    pub rts: Option<bool>,
    /// Reopen the device when it comes back after an unplug.
    pub reconnect: bool,
}

impl Default for SerialOptions {
    fn default() -> Self {
        Self {
            port: String::new(),
            line: LineConfig::default(),
            enter: SerialEnter::Cr,
            lf_to_crlf: false,
            local_echo: false,
            dtr: None,
            rts: None,
            reconnect: true,
        }
    }
}

impl SerialOptions {
    /// Tab title: `ttyUSB0 @ 115200`.
    pub fn title(&self) -> String {
        let name = self.port.rsplit('/').next().unwrap_or(&self.port);
        format!("{name} @ {}", self.line.baud)
    }
}

/// WSL session options (design W1). Windows only; saved on every platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WslOptions {
    /// `None` runs the default distribution.
    pub distribution: Option<String>,
    /// `None` uses the distribution's default user.
    pub user: Option<String>,
    /// Linux startup folder, e.g. `~` or `/srv/app`; `None` keeps the distribution default.
    pub start_folder: Option<String>,
    /// Lines typed into the shell once it is ready; it then stays open. Visible, not secret.
    pub startup_commands: Option<String>,
}

impl WslOptions {
    /// Distribution name for display, or `Default` for the default distribution.
    pub fn label(&self) -> &str {
        self.distribution.as_deref().unwrap_or("Default")
    }
}

/// One WSL distribution, as listed from the per-user registry (`Lxss`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WslDistribution {
    pub name: String,
    pub default: bool,
    /// WSL version (1 or 2); `None` when the registry does not say.
    pub version: Option<u8>,
    /// Running (from `--list --verbose`); `None` when not probed.
    pub running: Option<bool>,
}

/// Result of listing WSL distributions. The editor tells these four apart (design W2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WslStatus {
    /// No WSL service registered (with or without `wsl.exe`). `install_command` is true on builds
    /// whose `wsl.exe` supports `wsl --install` (otherwise the editor links the install guide).
    NotInstalled { install_command: bool },
    /// A WSL service is registered but there is no distribution.
    NoDistribution,
    /// Registry (or probe) access failed; carries a short reason.
    CouldNotQuery(String),
    /// One row per distribution, the default marked.
    Listed(Vec<WslDistribution>),
}

/// A saved or quick-connect session definition. Never holds secrets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Session {
    pub id: SessionId,
    pub name: String,
    pub folder: Option<FolderId>,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
    /// Session icon name under `assets/icons/`; `None` uses the protocol icon.
    pub icon: Option<String>,
    pub ssh: SshOptions,
    pub ftp: FtpOptions,
    pub rdp: RdpOptions,
    pub vnc: VncOptions,
    pub telnet: TelnetOptions,
    pub serial: SerialOptions,
    pub wsl: WslOptions,
    pub terminal: TerminalPrefs,
}

impl Session {
    /// New session with the protocol's default port and a `user@host` name.
    pub fn new(protocol: Protocol, host: impl Into<String>, user: Option<String>, port: Option<u16>) -> Self {
        let host = host.into();
        let name = match &user {
            Some(u) => format!("{u}@{host}"),
            None => host.clone(),
        };
        Self { name, protocol, port: port.unwrap_or(protocol.default_port()), host, user, ..Self::default() }
    }

    /// Display target: `ssh://alice@host:22`, or `serial:///dev/ttyUSB0 115200 8N1`.
    pub fn target(&self) -> String {
        if self.protocol == Protocol::Serial {
            return format!("serial://{} {}", self.serial.port, self.serial.line);
        }
        if self.protocol == Protocol::Local {
            let user = self.user.as_deref().map(|u| format!("{u}@")).unwrap_or_default();
            return format!("Local terminal {user}{}", self.host);
        }
        if self.protocol == Protocol::Wsl {
            let user = self.wsl.user.as_deref().map(|u| format!("{u}@")).unwrap_or_default();
            return format!("wsl://{user}{}", self.wsl.label());
        }
        let user = self.user.as_deref().map(|u| format!("{u}@")).unwrap_or_default();
        format!("{}://{user}{}:{}", self.protocol.scheme(), self.host, self.port)
    }

    /// New serial session on `port` with the given line settings.
    pub fn serial(port: impl Into<String>, line: LineConfig) -> Self {
        let serial = SerialOptions { port: port.into(), line, ..SerialOptions::default() };
        let name = if serial.port.is_empty() { String::new() } else { serial.title() };
        Self { name, protocol: Protocol::Serial, serial, ..Self::default() }
    }
}

/// Session tree folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Folder {
    pub id: FolderId,
    pub name: String,
    pub parent: Option<FolderId>,
    /// Bundled icon name (catalog "folders"); `None` is the default yellow folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// Port-forwarding direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TunnelKind {
    #[default]
    Local,
    Remote,
    /// SOCKS5.
    Dynamic,
}

/// A named tunnel bound to an SSH session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tunnel {
    pub name: String,
    pub kind: TunnelKind,
    pub via: SessionId,
    /// Listen address; loopback unless `exposure_confirmed`.
    pub bind_addr: String,
    pub bind_port: u16,
    /// Destination; unused for dynamic tunnels.
    pub dest_host: String,
    pub dest_port: u16,
    pub exposure_confirmed: bool,
    pub autostart: bool,
}

impl Default for Tunnel {
    fn default() -> Self {
        Self {
            name: String::new(),
            kind: TunnelKind::Local,
            via: SessionId::default(),
            bind_addr: "127.0.0.1".into(),
            bind_port: 0,
            dest_host: String::new(),
            dest_port: 0,
            exposure_confirmed: false,
            autostart: false,
        }
    }
}

impl Tunnel {
    /// Effective bind address: non-loopback addresses require confirmation.
    /// Remote forwards fall back to `localhost`, local/dynamic to `127.0.0.1`.
    pub fn effective_bind_addr(&self) -> &str {
        let loopback = matches!(self.bind_addr.as_str(), "127.0.0.1" | "::1" | "localhost");
        if loopback || self.exposure_confirmed {
            &self.bind_addr
        } else if self.kind == TunnelKind::Remote {
            "localhost"
        } else {
            "127.0.0.1"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_and_x11_forwarding_off_by_default() {
        let s = Session::new(Protocol::Ssh, "h", None, None);
        assert!(!s.ssh.agent_forwarding);
        assert!(!s.ssh.x11_forwarding);
        assert_eq!(s.port, 22);
    }

    #[test]
    fn tunnel_not_exposed_without_confirmation() {
        let mut t = Tunnel { bind_addr: "0.0.0.0".into(), ..Tunnel::default() };
        assert_eq!(t.effective_bind_addr(), "127.0.0.1");
        t.kind = TunnelKind::Remote;
        assert_eq!(t.effective_bind_addr(), "localhost");
        t.exposure_confirmed = true;
        assert_eq!(t.effective_bind_addr(), "0.0.0.0");
    }

    #[test]
    fn terminal_prefs_inherit_global() {
        let global =
            TerminalPrefs { cursor_shape: Some(CursorShape::Underline), font_size: Some(11.0), ..Default::default() };
        let per = TerminalPrefs { font_size: Some(14.0), ..Default::default() };
        let r = per.resolve(&global);
        assert_eq!(r.cursor_shape, Some(CursorShape::Underline));
        assert_eq!(r.font_size, Some(14.0));
    }

    #[test]
    fn local_is_a_terminal_outside_saved_protocols() {
        assert!(Protocol::Local.is_terminal());
        assert!(!Protocol::ALL.contains(&Protocol::Local));
        assert_eq!(Protocol::from_scheme("local"), None);
        assert_eq!(Protocol::Local.icon(), "terminal-shell");
        let local = Session::new(Protocol::Local, "workstation", Some("alice".into()), None);
        assert_eq!(local.target(), "Local terminal alice@workstation");
    }

    #[test]
    fn serde_round_trip() {
        let mut s = Session::new(Protocol::Rdp, "win", Some("bob".into()), Some(3390));
        s.ssh.env.push(("FOO".into(), "bar".into()));
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Session>(&json).unwrap(), s);
        let t = Tunnel {
            name: "db".into(),
            bind_port: 15432,
            dest_host: "db.internal".into(),
            dest_port: 5432,
            ..Tunnel::default()
        };
        assert_eq!(serde_json::from_str::<Tunnel>(&serde_json::to_string(&t).unwrap()).unwrap(), t);
    }

    #[test]
    fn telnet_and_serial_round_trip_and_defaults() {
        let mut t = Session::new(Protocol::Telnet, "router", Some("alice".into()), None);
        assert_eq!(t.port, 23);
        assert!(!t.telnet.auto_login && !t.telnet.raw);
        assert_eq!(t.telnet.terminal_type, "xterm-256color");
        t.telnet.newline = Newline::CrNul;
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(serde_json::from_str::<Session>(&json).unwrap(), t);

        let line = LineConfig { baud: 9600, data_bits: DataBits::Seven, parity: Parity::Even, ..LineConfig::default() };
        let mut s = Session::serial("/dev/ttyUSB0", line);
        s.serial.dtr = Some(false);
        assert_eq!(s.name, "ttyUSB0 @ 9600");
        assert_eq!(s.serial.line.to_string(), "9600 7E1");
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Session>(&json).unwrap(), s);
    }

    #[test]
    fn pre_change_session_json_loads_with_defaults() {
        let old = r#"{"id":3,"name":"web","protocol":"ssh","host":"h","port":22}"#;
        let s: Session = serde_json::from_str(old).unwrap();
        assert_eq!(s.telnet, TelnetOptions::default());
        assert_eq!(s.serial, SerialOptions::default());
        assert!(s.serial.reconnect);
        assert_eq!(s.serial.line, LineConfig::default());
        assert_eq!(s.ssh.remote_monitoring, None);
        let off: SshOptions = serde_json::from_str(r#"{"follow_terminal_folder":false}"#).unwrap();
        assert_eq!(off.remote_monitoring, None);
        let set = SshOptions { remote_monitoring: Some(false), ..SshOptions::default() };
        assert_eq!(serde_json::from_str::<SshOptions>(&serde_json::to_string(&set).unwrap()).unwrap(), set);
    }

    #[test]
    fn parity_and_flow_parse() {
        assert_eq!(Parity::parse("e"), Some(Parity::Even));
        assert_eq!(Parity::parse("NONE"), Some(Parity::None));
        assert_eq!(Parity::parse("q"), None);
        assert_eq!(FlowControl::parse("rts/cts"), Some(FlowControl::Hardware));
        assert_eq!(FlowControl::parse("xonxoff"), Some(FlowControl::Software));
        assert_eq!(FlowControl::parse("x"), None);
    }

    #[test]
    fn rdp_legacy_tls_is_opt_in_and_round_trips() {
        let old: RdpOptions = serde_json::from_str(r#"{"clipboard":false}"#).unwrap();
        assert!(!old.allow_legacy_tls && !old.allow_standard_security && !old.clipboard);
        let on = RdpOptions { allow_legacy_tls: true, allow_standard_security: true, ..RdpOptions::default() };
        assert_eq!(serde_json::from_str::<RdpOptions>(&serde_json::to_string(&on).unwrap()).unwrap(), on);
    }
}
