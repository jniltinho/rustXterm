//! Operating-system contracts. Protocol, engine and UI crates reach the OS only
//! through these traits; `platform-linux` implements them (macOS/Windows later).

use std::path::PathBuf;
use std::sync::Arc;

pub use rxt_core::{Error, Result};

/// Per-user directories.
pub trait Paths: Send + Sync {
    fn config_dir(&self) -> PathBuf;
    fn data_dir(&self) -> PathBuf;
    fn cache_dir(&self) -> PathBuf;
    /// Per-session runtime dir (control socket); `None` when unavailable.
    fn runtime_dir(&self) -> Option<PathBuf>;
    fn home_dir(&self) -> PathBuf;
    /// User SSH directory (`~/.ssh`).
    fn ssh_dir(&self) -> PathBuf {
        self.home_dir().join(".ssh")
    }
    /// Create `dir` if missing and restrict it to the current user, files inside included.
    /// Unix keeps its per-file 0600/0700 modes (default: nothing to do); Windows applies a
    /// protected owner-only ACL and fails when it cannot be applied or verified.
    fn make_private(&self, dir: &std::path::Path) -> Result<()> {
        let _ = dir;
        Ok(())
    }
}

/// Theme change callback; receives the new `prefers_dark` value.
pub type ThemeListener = Box<dyn Fn(Option<bool>) + Send + Sync>;

/// Desktop color-scheme source.
pub trait ThemeSource: Send + Sync {
    /// `Some(true)` dark, `Some(false)` light, `None` unknown/no preference.
    fn prefers_dark(&self) -> Option<bool>;
    /// Subscribe to live changes; the callback runs on a background thread.
    /// Implementations without live updates may never call it.
    fn subscribe(&self, on_change: ThemeListener);
}

/// Text clipboard. `primary` selects the X11/Wayland primary selection where supported.
pub trait Clipboard: Send + Sync {
    fn get_text(&self) -> Result<String>;
    fn set_text(&self, text: &str) -> Result<()>;
    fn set_primary(&self, text: &str) -> Result<()> {
        let _ = text;
        Ok(())
    }
}

/// Desktop notifications.
pub trait Notifier: Send + Sync {
    fn notify(&self, title: &str, body: &str) -> Result<()>;
}

/// Native file dialogs; blocking calls must run off the UI thread.
pub trait FileDialogs: Send + Sync {
    fn pick_files(&self, title: &str) -> Option<Vec<PathBuf>>;
    fn pick_folder(&self, title: &str) -> Option<PathBuf>;
    fn save_file(&self, title: &str, suggested_name: &str) -> Option<PathBuf>;
    /// Window the dialogs are transient for, as a platform-specific parent id (`x11:<hex xid>`
    /// for the XDG portal, `win32:<hex HWND>` on Windows); `None` opens them unparented.
    fn set_parent(&self, parent: Option<String>) {
        let _ = parent;
    }
}

/// Secret storage (keyring). Implemented in M3.
pub trait Keyring: Send + Sync {
    fn get(&self, id: &str) -> Result<Option<Vec<u8>>>;
    fn set(&self, id: &str, secret: &[u8]) -> Result<()>;
    fn delete(&self, id: &str) -> Result<()>;
}

/// Byte stream of one control connection (same-user peer, already checked).
pub trait ControlStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin> ControlStream for T {}

/// Listening control endpoint; dropping it removes the endpoint (e.g. the socket file).
pub trait ControlListener: Send + Sync {
    /// Next connection from a same-user peer. Other peers are closed silently and skipped.
    fn accept(&self) -> rxt_core::BoxFut<'_, Result<Box<dyn ControlStream>>>;
}

/// Local, same-user control transport for agents (never a network socket). Framing is
/// [`control::read_frame`]/[`control::write_frame`] on the returned streams.
pub trait ControlTransport: Send + Sync {
    /// Endpoint description (e.g. socket path); `None` when the platform has none.
    fn endpoint(&self) -> Option<String>;
    /// Create the endpoint and listen. Call inside a tokio runtime.
    fn listen(&self) -> Result<Box<dyn ControlListener>> {
        Err(Error::Unsupported("agent control transport".into()))
    }
    /// Client side: connect to the running instance; `Error::NotFound` when nothing listens.
    fn connect(&self) -> rxt_core::BoxFut<'_, Result<Box<dyn ControlStream>>> {
        Box::pin(async { Err(Error::Unsupported("agent control transport".into())) })
    }
}

/// Open a file or URL with the desktop's default application.
pub trait Opener: Send + Sync {
    fn open(&self, path: &std::path::Path) -> Result<()>;
}

/// All platform services, wired in by the app binary.
#[derive(Clone)]
pub struct Platform {
    pub paths: Arc<dyn Paths>,
    pub theme: Arc<dyn ThemeSource>,
    pub clipboard: Arc<dyn Clipboard>,
    pub notifier: Arc<dyn Notifier>,
    pub dialogs: Arc<dyn FileDialogs>,
    pub keyring: Arc<dyn Keyring>,
    pub control: Arc<dyn ControlTransport>,
    pub opener: Arc<dyn Opener>,
    pub serial: Arc<dyn SerialPorts>,
}

pub mod control;
pub mod fake;
pub mod serial;

pub use serial::{PortInfo, SerialPorts, SerialStream};
