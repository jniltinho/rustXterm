//! macOS adapter for the platform contracts: `~/Library` paths, the `AppleInterfaceStyle` theme,
//! clipboard, notifications, file dialogs, the macOS Keychain keyring ([`Keychain`]), the agent
//! control socket ([`UnixControl`]) and serial ports ([`MacSerial`]). There is no local shell in
//! phase 1 (`local_pty` is `None`).
//!
//! Contract used by the app: [`platform`] builds every service; it never fails (a missing
//! clipboard/notification backend degrades to an error at call time, never a panic).
//!
//! No `unsafe`: Keychain uses `security-framework`'s safe API and the theme reads a default via a
//! subprocess, so the workspace `unsafe_code = "forbid"` holds (the Windows exception is not
//! extended). Compiled only for macOS; on other targets the crate is empty.
#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rxt_platform::{Clipboard, Error, FileDialogs, Notifier, Opener, Paths, Platform, Result};

mod control;
mod keychain;
mod serial;
mod theme;

pub use control::{SOCKET, UnixControl, peer_allowed, private_dir};
pub use keychain::Keychain;
pub use serial::MacSerial;
pub use theme::AppleTheme;

const APP_DIR: &str = "rustxterm";

/// Build the macOS platform services.
pub fn platform() -> Platform {
    use std::sync::Arc;
    Platform {
        paths: Arc::new(MacPaths),
        theme: Arc::new(AppleTheme::spawn()),
        clipboard: Arc::new(SystemClipboard::default()),
        notifier: Arc::new(DesktopNotifier),
        dialogs: Arc::new(MacDialogs),
        keyring: Arc::new(Keychain),
        control: Arc::new(UnixControl::new(MacPaths.runtime_dir())),
        opener: Arc::new(MacOpen),
        serial: Arc::new(MacSerial),
        local_pty: None,
        wsl: None,
    }
}

/// macOS per-user directories under `~/Library`, with the `rustxterm` subdirectory.
pub struct MacPaths;

impl Paths for MacPaths {
    fn config_dir(&self) -> PathBuf {
        // On macOS `dirs::config_dir()` is `~/Library/Application Support` (same as data).
        dirs::config_dir().unwrap_or_else(|| self.home_dir().join("Library/Application Support")).join(APP_DIR)
    }
    fn data_dir(&self) -> PathBuf {
        dirs::data_dir().unwrap_or_else(|| self.home_dir().join("Library/Application Support")).join(APP_DIR)
    }
    fn cache_dir(&self) -> PathBuf {
        dirs::cache_dir().unwrap_or_else(|| self.home_dir().join("Library/Caches")).join(APP_DIR)
    }
    fn runtime_dir(&self) -> Option<PathBuf> {
        use std::os::unix::fs::MetadataExt;
        // Per-user `$TMPDIR` (private, mode 0700) with a per-uid subdir; `control` checks it.
        // The uid comes from `$TMPDIR`'s owner (sync, no Tokio runtime); the socketpair-based
        // `own_uid` is only safe to call from inside a runtime (listen/connect/accept).
        let dir = std::env::temp_dir();
        let name =
            std::fs::metadata(&dir).map(|m| format!("rustxterm-{}", m.uid())).unwrap_or_else(|_| APP_DIR.to_owned());
        Some(dir.join(name))
    }
    fn home_dir(&self) -> PathBuf {
        dirs::home_dir().unwrap_or_else(std::env::temp_dir)
    }
    fn download_dir(&self) -> Option<PathBuf> {
        dirs::download_dir()
    }
}

/// arboard clipboard, connected on first use (no window server → error, never a panic).
#[derive(Default)]
pub struct SystemClipboard {
    inner: Mutex<Option<arboard::Clipboard>>,
}

impl SystemClipboard {
    fn with<T>(&self, f: impl FnOnce(&mut arboard::Clipboard) -> std::result::Result<T, arboard::Error>) -> Result<T> {
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
    // macOS has no primary selection; `set_primary` keeps the no-op default.
}

/// User Notifications through `notify-rust` (NSUserNotification / UNUserNotification).
pub struct DesktopNotifier;

impl Notifier for DesktopNotifier {
    fn notify(&self, title: &str, body: &str) -> Result<()> {
        notify_rust::Notification::new()
            .summary(title)
            .body(body)
            .show()
            .map(|_| ())
            .map_err(|e| Error::Unsupported(format!("notifications: {e}")))
    }
}

/// Native file dialogs through `rfd` (AppKit `NSOpenPanel`/`NSSavePanel`), opening in `start`.
///
/// ponytail / Mac-verification pending (design 3b): AppKit panels must be driven from the main
/// thread, while this contract calls dialogs off the UI thread. The sync `rfd` form below is the
/// starting point; on a real Mac this likely moves to `rfd::AsyncFileDialog` with main-window
/// parenting (`set_parent`, sheet attachment) — still no raw AppKit handle and no `unsafe`. The
/// threading and sheet attachment are part of the macOS receipt, so task 1.4 stays unverified
/// until then.
pub struct MacDialogs;

impl FileDialogs for MacDialogs {
    fn pick_files(&self, title: &str, start: &Path) -> Option<Vec<PathBuf>> {
        rfd::FileDialog::new().set_title(title).set_directory(start).pick_files()
    }
    fn pick_folder(&self, title: &str, start: &Path) -> Option<PathBuf> {
        rfd::FileDialog::new().set_title(title).set_directory(start).pick_folder()
    }
    fn save_file(&self, title: &str, suggested_name: &str, start: &Path) -> Option<PathBuf> {
        rfd::FileDialog::new().set_title(title).set_directory(start).set_file_name(suggested_name).save_file()
    }
}

/// `/usr/bin/open`, detached; a reaper thread collects the exit status.
pub struct MacOpen;

impl Opener for MacOpen {
    fn open(&self, path: &Path) -> Result<()> {
        use std::process::{Command, Stdio};
        let mut child = Command::new("/usr/bin/open")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        std::thread::spawn(move || child.wait());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_live_under_library_and_app_dir() {
        let p = MacPaths;
        assert!(p.config_dir().ends_with(APP_DIR));
        assert!(p.data_dir().ends_with(APP_DIR));
        assert!(p.cache_dir().ends_with(APP_DIR));
        assert!(p.ssh_dir().ends_with(".ssh"));
        // The runtime dir is a per-user `$TMPDIR` subdir checked by `control`.
        assert!(p.runtime_dir().is_some_and(|d| d.to_string_lossy().contains("rustxterm")));
    }

    #[test]
    fn platform_never_fails_without_a_session() {
        // On a headless runner there is no window server; building must still be quick and safe.
        let start = std::time::Instant::now();
        let p = platform();
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        assert!(p.paths.config_dir().ends_with(APP_DIR));
        assert!(p.local_pty.is_none());
        let _ = p.clipboard.get_text(); // error or text, never a panic
    }
}
