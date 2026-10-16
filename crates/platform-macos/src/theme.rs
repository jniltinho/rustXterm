//! Desktop color scheme on macOS, read from the global `AppleInterfaceStyle` default
//! (`defaults read -g AppleInterfaceStyle` → "Dark" when dark, absent/error when light).
//!
//! There is no eframe→adapter bridge: the UI reads appearance only through [`ThemeSource`], so
//! this adapter owns the value. A background thread polls the default and notifies listeners on
//! change. Reading via `defaults` is a safe subprocess (no `unsafe`, no AppKit linkage).
//!
//! ponytail: 2 s poll instead of a `NSDistributedNotificationCenter` observer — no extra crate,
//! no main-thread requirement; switch to an observer (objc2, still safe) if the latency matters.

use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rxt_platform::{ThemeListener, ThemeSource};

/// How often the appearance is re-read.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

type Listener = Arc<dyn Fn(Option<bool>) + Send + Sync>;

#[derive(Default)]
struct Shared {
    dark: Mutex<Option<bool>>,
    listeners: Mutex<Vec<Listener>>,
}

impl Shared {
    fn update(&self, dark: Option<bool>) {
        match self.dark.lock() {
            Ok(mut d) if *d != dark => *d = dark,
            _ => return,
        }
        // Clone the list so callbacks may call back into the theme source.
        let listeners = self.listeners.lock().map(|l| l.clone()).unwrap_or_default();
        listeners.iter().for_each(|l| l(dark));
    }
}

/// Theme source backed by `AppleInterfaceStyle`.
pub struct AppleTheme {
    shared: Arc<Shared>,
}

/// Read the current appearance: `Some(true)` dark, `Some(false)` light, `None` when `defaults`
/// itself could not be run.
fn read_appearance() -> Option<bool> {
    match Command::new("defaults").args(["read", "-g", "AppleInterfaceStyle"]).output() {
        // "Dark\n" when dark; a non-zero exit (key absent) means Light.
        Ok(out) if out.status.success() => Some(String::from_utf8_lossy(&out.stdout).trim() == "Dark"),
        Ok(_) => Some(false),
        Err(_) => None,
    }
}

impl AppleTheme {
    /// Read the appearance once and start polling for changes; never fails or blocks for long.
    pub fn spawn() -> Self {
        let shared = Arc::new(Shared::default());
        if let Ok(mut d) = shared.dark.lock() {
            *d = read_appearance();
        }
        let s = shared.clone();
        let started = std::thread::Builder::new().name("rxt-theme-macos".into()).spawn(move || {
            loop {
                std::thread::sleep(POLL_INTERVAL);
                s.update(read_appearance());
            }
        });
        if let Err(e) = started {
            tracing::warn!("theme poll thread: {e}");
        }
        Self { shared }
    }
}

impl ThemeSource for AppleTheme {
    fn prefers_dark(&self) -> Option<bool> {
        self.shared.dark.lock().ok().and_then(|d| *d)
    }
    fn subscribe(&self, on_change: ThemeListener) {
        if let Ok(mut l) = self.shared.listeners.lock() {
            l.push(Arc::from(on_change));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_appearance_never_panics() {
        // On a CI runner or VM `defaults` may or may not exist; either way no panic.
        let _ = read_appearance();
    }

    #[test]
    fn listeners_fire_on_change_only() {
        let t = AppleTheme { shared: Arc::default() };
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        t.subscribe(Box::new(move |d| s.lock().unwrap().push(d)));
        t.shared.update(Some(true));
        t.shared.update(Some(true));
        t.shared.update(Some(false));
        assert_eq!(*seen.lock().unwrap(), vec![Some(true), Some(false)]);
        assert_eq!(t.prefers_dark(), Some(false));
    }
}
