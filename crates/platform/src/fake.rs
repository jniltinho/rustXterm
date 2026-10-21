//! In-memory platform for tests, snapshots and headless runs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub use crate::local_pty::{FakePty, FakeShell};
pub use crate::serial::{FakeDevice, FakeSerial};
pub use crate::wsl::FakeWsl;
use crate::*;

/// Fake platform rooted at `root`, with a settable theme.
#[derive(Default)]
pub struct Fake {
    pub root: PathBuf,
    pub dark: Mutex<Option<bool>>,
    pub listeners: Mutex<Vec<ThemeListener>>,
    pub clipboard: Mutex<String>,
    pub notes: Mutex<Vec<(String, String)>>,
    pub secrets: Mutex<HashMap<String, Vec<u8>>>,
    pub serial: crate::serial::FakeSerial,
    pub local_pty: crate::local_pty::FakePty,
    pub wsl: crate::wsl::FakeWsl,
    /// Build the platform with a WSL backend (as on Windows); off by default, like Linux/macOS.
    pub with_wsl: std::sync::atomic::AtomicBool,
    /// Last [`FileDialogs::set_parent`] value.
    pub dialog_parent: Mutex<Option<String>>,
    /// Directories passed to [`Paths::make_private`].
    pub private_dirs: Mutex<Vec<PathBuf>>,
    /// Start folders of the file dialogs, in order.
    pub dialog_starts: Mutex<Vec<PathBuf>>,
    /// What the next file dialog returns (`None`: cancelled).
    pub dialog_answer: Mutex<Option<PathBuf>>,
    /// Paths given to [`Opener::open`], in order.
    pub opened: Mutex<Vec<PathBuf>>,
    /// When set, [`Opener::open`] fails with this message.
    pub open_error: Mutex<Option<String>>,
    /// When set, [`Notifier::notify`] fails (notification service unavailable).
    pub notify_fails: std::sync::atomic::AtomicBool,
}

impl Fake {
    /// Change the fake desktop theme and notify subscribers.
    pub fn set_dark(&self, dark: Option<bool>) {
        if let Ok(mut d) = self.dark.lock() {
            *d = dark;
        }
        if let Ok(ls) = self.listeners.lock() {
            ls.iter().for_each(|l| l(dark));
        }
    }

    /// Build a [`Platform`] whose services all point at one shared fake.
    pub fn platform(self: &Arc<Self>) -> Platform {
        Platform {
            paths: self.clone(),
            theme: self.clone(),
            clipboard: self.clone(),
            notifier: self.clone(),
            dialogs: self.clone(),
            keyring: self.clone(),
            control: self.clone(),
            opener: self.clone(),
            serial: Arc::new(self.serial.clone()),
            local_pty: Some(Arc::new(self.local_pty.clone())),
            wsl: self
                .with_wsl
                .load(std::sync::atomic::Ordering::Relaxed)
                .then(|| Arc::new(self.wsl.clone()) as Arc<dyn Wsl>),
        }
    }
}

fn poisoned() -> Error {
    Error::Io("lock poisoned".into())
}

impl Paths for Fake {
    fn config_dir(&self) -> PathBuf {
        self.root.join("config")
    }
    fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }
    fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }
    fn runtime_dir(&self) -> Option<PathBuf> {
        Some(self.root.join("run"))
    }
    fn home_dir(&self) -> PathBuf {
        self.root.join("home")
    }
    fn download_dir(&self) -> Option<PathBuf> {
        Some(self.root.join("home").join("Downloads"))
    }
    fn make_private(&self, dir: &Path) -> Result<()> {
        self.private_dirs.lock().map_err(|_| poisoned())?.push(dir.to_owned());
        Ok(())
    }
}

impl ThemeSource for Fake {
    fn prefers_dark(&self) -> Option<bool> {
        self.dark.lock().ok().and_then(|d| *d)
    }
    fn subscribe(&self, on_change: ThemeListener) {
        if let Ok(mut ls) = self.listeners.lock() {
            ls.push(on_change);
        }
    }
}

impl Clipboard for Fake {
    fn get_text(&self) -> Result<String> {
        self.clipboard.lock().map(|c| c.clone()).map_err(|_| poisoned())
    }
    fn set_text(&self, text: &str) -> Result<()> {
        *self.clipboard.lock().map_err(|_| poisoned())? = text.into();
        Ok(())
    }
}

impl Notifier for Fake {
    fn notify(&self, title: &str, body: &str) -> Result<()> {
        if self.notify_fails.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(Error::Unsupported("notifications".into()));
        }
        self.notes.lock().map_err(|_| poisoned())?.push((title.into(), body.into()));
        Ok(())
    }
}

impl Fake {
    fn dialog(&self, start: &Path) -> Option<PathBuf> {
        self.dialog_starts.lock().ok()?.push(start.to_owned());
        self.dialog_answer.lock().ok()?.take()
    }
}

impl FileDialogs for Fake {
    fn pick_files(&self, _: &str, start: &Path) -> Option<Vec<PathBuf>> {
        self.dialog(start).map(|p| vec![p])
    }
    fn pick_folder(&self, _: &str, start: &Path) -> Option<PathBuf> {
        self.dialog(start)
    }
    fn save_file(&self, _: &str, _: &str, start: &Path) -> Option<PathBuf> {
        self.dialog(start)
    }
    fn set_parent(&self, parent: Option<String>) {
        if let Ok(mut p) = self.dialog_parent.lock() {
            *p = parent;
        }
    }
}

impl Keyring for Fake {
    fn get(&self, id: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.secrets.lock().map_err(|_| poisoned())?.get(id).cloned())
    }
    fn set(&self, id: &str, secret: &[u8]) -> Result<()> {
        self.secrets.lock().map_err(|_| poisoned())?.insert(id.into(), secret.to_vec());
        Ok(())
    }
    fn delete(&self, id: &str) -> Result<()> {
        self.secrets.lock().map_err(|_| poisoned())?.remove(id);
        Ok(())
    }
}

impl ControlTransport for Fake {
    fn endpoint(&self) -> Option<String> {
        None
    }
}

impl Opener for Fake {
    fn open(&self, path: &Path) -> Result<()> {
        if let Some(e) = self.open_error.lock().map_err(|_| poisoned())?.clone() {
            return Err(Error::Io(e));
        }
        self.opened.lock().map_err(|_| poisoned())?.push(path.to_owned());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_private_is_recorded() {
        let f = Fake::default();
        f.make_private(Path::new("/x/data")).unwrap();
        assert_eq!(*f.private_dirs.lock().unwrap(), vec![PathBuf::from("/x/data")]);
    }

    #[test]
    fn theme_subscription_fires() {
        let f = Arc::new(Fake::default());
        let p = f.platform();
        let seen = Arc::new(Mutex::new(None));
        let s = seen.clone();
        p.theme.subscribe(Box::new(move |d| *s.lock().unwrap() = Some(d)));
        f.set_dark(Some(true));
        assert_eq!(*seen.lock().unwrap(), Some(Some(true)));
        assert_eq!(p.theme.prefers_dark(), Some(true));
    }
}
