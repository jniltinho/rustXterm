//! Linux adapter for the platform contracts: XDG paths, XDG portal theme, clipboard,
//! notifications, file dialogs, Secret Service keyring, the agent control socket ([`UnixControl`])
//! and serial ports ([`LinuxSerial`]).
//!
//! Contract used by the app: [`platform`] builds every service; it never fails
//! (missing portal/clipboard degrade to fallbacks).
//!
//! Compiled only for Linux; on other targets the crate is empty (add-windows-port design W8).
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ashpd::desktop::file_chooser::SelectedFiles;

use rxt_platform::{
    Clipboard, ControlTransport, Error, FileDialogs, Keyring, Notifier, Opener, Paths, Platform, Result,
};

mod control;
mod desktop;
mod keyring;
mod serial;
mod theme;

pub use control::{SOCKET, UnixControl, peer_allowed, private_dir};
pub use desktop::install_desktop_entry;
pub use keyring::SecretService;
pub use serial::{LinuxSerial, group_name, permission_message};
pub use theme::PortalTheme;

const APP_DIR: &str = "rustxterm";

/// Build the Linux platform services.
pub fn platform() -> Platform {
    Platform {
        paths: Arc::new(XdgPaths),
        theme: Arc::new(PortalTheme::spawn()),
        clipboard: Arc::new(SystemClipboard::default()),
        notifier: Arc::new(DesktopNotifier),
        dialogs: Arc::new(PortalDialogs::default()),
        keyring: Arc::new(SecretService),
        control: Arc::new(UnixControl::new(XdgPaths.runtime_dir())),
        opener: Arc::new(XdgOpen),
        serial: Arc::new(LinuxSerial::default()),
    }
}

/// XDG base directories with the `rustxterm` app subdirectory.
pub struct XdgPaths;

impl Paths for XdgPaths {
    fn config_dir(&self) -> PathBuf {
        dirs::config_dir().unwrap_or_else(|| self.home_dir().join(".config")).join(APP_DIR)
    }
    fn data_dir(&self) -> PathBuf {
        dirs::data_dir().unwrap_or_else(|| self.home_dir().join(".local/share")).join(APP_DIR)
    }
    fn cache_dir(&self) -> PathBuf {
        dirs::cache_dir().unwrap_or_else(|| self.home_dir().join(".cache")).join(APP_DIR)
    }
    fn runtime_dir(&self) -> Option<PathBuf> {
        dirs::runtime_dir().map(|d| d.join(APP_DIR))
    }
    fn home_dir(&self) -> PathBuf {
        dirs::home_dir().unwrap_or_else(std::env::temp_dir)
    }
}

/// arboard clipboard, connected on first use (no display → error, never a panic).
#[derive(Default)]
pub struct SystemClipboard {
    inner: Mutex<Option<arboard::Clipboard>>,
}

impl SystemClipboard {
    fn with<T>(&self, f: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>) -> Result<T> {
        let mut guard = self.inner.lock().map_err(|_| Error::Io("clipboard lock poisoned".into()))?;
        if guard.is_none() {
            *guard = Some(arboard::Clipboard::new().map_err(clip_err)?);
        }
        match guard.as_mut() {
            Some(c) => f(c).map_err(clip_err),
            None => Err(Error::Unsupported("clipboard".into())),
        }
    }
}

fn clip_err(e: arboard::Error) -> Error {
    match e {
        arboard::Error::ClipboardNotSupported => Error::Unsupported("clipboard".into()),
        arboard::Error::ContentNotAvailable => Error::NotFound("clipboard has no text".into()),
        other => Error::Io(format!("clipboard: {other}")),
    }
}

impl Clipboard for SystemClipboard {
    fn get_text(&self) -> Result<String> {
        self.with(|c| c.get_text())
    }
    fn set_text(&self, text: &str) -> Result<()> {
        self.with(|c| c.set_text(text))
    }
    fn set_primary(&self, text: &str) -> Result<()> {
        use arboard::{LinuxClipboardKind, SetExtLinux};
        self.with(|c| c.set().clipboard(LinuxClipboardKind::Primary).text(text))
    }
}

/// Freedesktop notifications over D-Bus.
pub struct DesktopNotifier;

impl Notifier for DesktopNotifier {
    fn notify(&self, title: &str, body: &str) -> Result<()> {
        notify_rust::Notification::new()
            .appname("rustXterm")
            .summary(title)
            .body(body)
            .show()
            .map(|_| ())
            .map_err(|e| Error::Unsupported(format!("notifications: {e}")))
    }
}

/// File dialogs through the XDG desktop portal (FileChooser), parented to the app window so
/// they open in front of it; rfd (portal or zenity) is the fallback when the portal call fails.
#[derive(Default)]
pub struct PortalDialogs {
    parent: Mutex<Option<String>>,
}

impl PortalDialogs {
    fn identifier(&self) -> Option<ashpd::WindowIdentifier> {
        let parent = self.parent.lock().ok()?.clone()?;
        parent_identifier(&parent)
    }

    /// Run one portal request on a private runtime (dialogs already run off the UI thread).
    fn portal<T>(
        &self,
        request: impl Future<Output = ashpd::Result<SelectedFiles>>,
        map: impl FnOnce(Vec<PathBuf>) -> Option<T>,
        fallback: impl FnOnce() -> Option<T>,
    ) -> Option<T> {
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::warn!("file dialog runtime: {e}");
                return fallback();
            }
        };
        match rt.block_on(request) {
            Ok(files) => map(files.uris().iter().filter_map(|u| uri_path(u.as_str())).collect()),
            Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => None,
            Err(e) => {
                tracing::warn!("file chooser portal: {e}; falling back");
                fallback()
            }
        }
    }
}

/// Parse an `x11:<hex xid>` parent id.
fn parent_identifier(parent: &str) -> Option<ashpd::WindowIdentifier> {
    parent.parse::<ashpd::WindowIdentifierType>().ok().map(ashpd::WindowIdentifier::from)
}

/// Local path of a `file://` URI (percent-decoded); other schemes are skipped.
fn uri_path(uri: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let rest = uri.strip_prefix("file://")?;
    let path = &rest[rest.find('/')?..];
    let (bytes, mut out) = (path.as_bytes(), Vec::with_capacity(path.len()));
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    Some(PathBuf::from(std::ffi::OsString::from_vec(out)))
}

impl FileDialogs for PortalDialogs {
    fn pick_files(&self, title: &str) -> Option<Vec<PathBuf>> {
        let req = SelectedFiles::open_file().identifier(self.identifier()).title(title).modal(true).multiple(true);
        self.portal(
            async move { req.send().await?.response() },
            |files| (!files.is_empty()).then_some(files),
            || rfd::FileDialog::new().set_title(title).pick_files(),
        )
    }
    fn pick_folder(&self, title: &str) -> Option<PathBuf> {
        let req = SelectedFiles::open_file().identifier(self.identifier()).title(title).modal(true).directory(true);
        self.portal(
            async move { req.send().await?.response() },
            |files| files.into_iter().next(),
            || rfd::FileDialog::new().set_title(title).pick_folder(),
        )
    }
    fn save_file(&self, title: &str, suggested_name: &str) -> Option<PathBuf> {
        let req = SelectedFiles::save_file()
            .identifier(self.identifier())
            .title(title)
            .modal(true)
            .current_name(suggested_name);
        self.portal(
            async move { req.send().await?.response() },
            |files| files.into_iter().next(),
            || rfd::FileDialog::new().set_title(title).set_file_name(suggested_name).save_file(),
        )
    }
    fn set_parent(&self, parent: Option<String>) {
        if let Ok(mut p) = self.parent.lock() {
            *p = parent;
        }
    }
}

/// `xdg-open`, detached; a reaper thread collects the exit status.
pub struct XdgOpen;

impl Opener for XdgOpen {
    fn open(&self, path: &Path) -> Result<()> {
        use std::process::{Command, Stdio};
        let mut child = Command::new("xdg-open")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        std::thread::spawn(move || child.wait());
        Ok(())
    }
}

/// Keyring stub (tests, headless) and control transport (M4) placeholder.
pub struct Unsupported;

impl Keyring for Unsupported {
    fn get(&self, _: &str) -> Result<Option<Vec<u8>>> {
        Err(Error::Unsupported("keyring".into()))
    }
    fn set(&self, _: &str, _: &[u8]) -> Result<()> {
        Err(Error::Unsupported("keyring".into()))
    }
    fn delete(&self, _: &str) -> Result<()> {
        Err(Error::Unsupported("keyring".into()))
    }
}

impl ControlTransport for Unsupported {
    fn endpoint(&self) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn x11_parent_id_parses_to_portal_identifier() {
        let id = parent_identifier("x11:4a00007").map(|i| i.to_string());
        assert_eq!(id.as_deref(), Some("x11:4a00007"));
        assert!(parent_identifier("bogus").is_none());
    }

    #[test]
    fn set_parent_feeds_the_identifier() {
        let d = PortalDialogs::default();
        assert!(d.identifier().is_none());
        d.set_parent(Some("x11:1f".into()));
        assert_eq!(d.identifier().map(|i| i.to_string()).as_deref(), Some("x11:1f"));
    }

    #[test]
    fn file_uris_decode_to_paths() {
        assert_eq!(uri_path("file:///home/a%20b/k%C3%A9y"), Some(PathBuf::from("/home/a b/k\u{e9}y")));
        assert_eq!(uri_path("file://host/tmp/x"), Some(PathBuf::from("/tmp/x")));
        assert_eq!(uri_path("https://x/y"), None);
    }

    #[test]
    fn stubs_are_unsupported() {
        assert!(matches!(Unsupported.get("x"), Err(Error::Unsupported(_))));
        assert!(matches!(Unsupported.set("x", b"y"), Err(Error::Unsupported(_))));
        assert_eq!(Unsupported.endpoint(), None);
    }

    #[test]
    fn fake_platform_round_trips() {
        let f = Arc::new(rxt_platform::fake::Fake::default());
        let p = f.platform();
        p.clipboard.set_text("hi").unwrap();
        assert_eq!(p.clipboard.get_text().unwrap(), "hi");
        f.set_dark(Some(true));
        assert_eq!(p.theme.prefers_dark(), Some(true));
    }

    /// Runs in a child process (env vars cannot be set safely in-process).
    #[test]
    fn xdg_paths_child() {
        if std::env::var_os("RXT_PATHS_CHILD").is_none() {
            return;
        }
        let p = XdgPaths;
        assert_eq!(p.config_dir(), Path::new("/x/cfg/rustxterm"));
        assert_eq!(p.data_dir(), Path::new("/x/data/rustxterm"));
        assert_eq!(p.cache_dir(), Path::new("/x/cache/rustxterm"));
        assert_eq!(p.runtime_dir(), Some(PathBuf::from("/x/run/rustxterm")));
        assert_eq!(p.home_dir(), Path::new("/x/home"));
        assert_eq!(p.ssh_dir(), Path::new("/x/home/.ssh"));
    }

    #[test]
    fn xdg_paths_honor_env() {
        let out = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::xdg_paths_child", "--test-threads=1"])
            .env("RXT_PATHS_CHILD", "1")
            .env("HOME", "/x/home")
            .env("XDG_CONFIG_HOME", "/x/cfg")
            .env("XDG_DATA_HOME", "/x/data")
            .env("XDG_CACHE_HOME", "/x/cache")
            .env("XDG_RUNTIME_DIR", "/x/run")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success() && text.contains("1 passed"), "{text}");
    }

    #[test]
    fn platform_never_fails_without_session() {
        // In the build container there is no display, D-Bus or portal.
        let start = std::time::Instant::now();
        let p = platform();
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        assert!(p.paths.config_dir().ends_with(APP_DIR));
        let _ = p.clipboard.get_text(); // error or text, never a panic
    }
}
