//! Domain types shared by every rustXterm crate: sessions, theme tokens,
//! commands/events and the `FileOps` contract. No IO lives here.

pub mod command;
pub mod control;
pub mod desktop;
pub mod error;
pub mod fileops;
pub mod monitor;
pub mod prompt;
pub mod quick_connect;
pub mod session;
pub mod theme;

pub use command::{
    ApprovalId, Command, CommandId, ConnState, Direction, Event, HostKeyDecision, LaunchInfo, PromptAnswer, PromptId,
    PromptKind, Secret, SshInfo, TabId, TransferId, TunnelState,
};
pub use desktop::{DesktopEvent, DesktopInput, Framebuffer, Rect};
pub use error::{Error, Result};
pub use fileops::{BoxFut, FileEntry, FileKind, FileOps, Overwrite};
pub use monitor::{Disk, MetricsSample, MonitorState};
pub use prompt::{CertTrust, Prompter};
pub use session::{
    CursorShape, DataBits, FlowControl, Folder, FolderId, FtpOptions, FtpTls, JumpHost, LineConfig, Newline, Parity,
    PortInfo, Protocol, RdpOptions, RdpResolution, SerialEnter, SerialOptions, Session, SessionId, SshOptions,
    StopBits, TelnetOptions, TerminalPrefs, Tunnel, TunnelKind, VncOptions,
};
pub use theme::{Metrics, Rgba, TerminalScheme, ThemeMode, Tokens, TypeScale, UiScale};
