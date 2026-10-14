//! Windows "app mode" from `HKCU\...\Themes\Personalize\AppsUseLightTheme`, with live updates
//! from a thread blocked in `RegNotifyChangeKeyValue` (design W3).

use std::sync::{Arc, Mutex};

use rxt_platform::{ThemeListener, ThemeSource};

use crate::{APPS_USE_LIGHT_THEME, PERSONALIZE_KEY, prefers_dark, sys};

#[derive(Default)]
struct Shared {
    dark: Mutex<Option<bool>>,
    listeners: Mutex<Vec<Arc<ThemeListener>>>,
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

fn read() -> Option<bool> {
    prefers_dark(sys::reg_dword(PERSONALIZE_KEY, APPS_USE_LIGHT_THEME).ok())
}

/// Registry-backed theme source; a missing key leaves `None` (the UI falls back to Light).
pub struct RegistryTheme {
    shared: Arc<Shared>,
}

impl RegistryTheme {
    /// Read the current value and start the watcher thread; never fails.
    pub fn spawn() -> Self {
        let shared = Arc::new(Shared { dark: Mutex::new(read()), ..Shared::default() });
        let s = shared.clone();
        let started = std::thread::Builder::new().name("rxt-theme-registry".into()).spawn(move || {
            let key = match sys::RegWatch::open(PERSONALIZE_KEY) {
                Ok(k) => k,
                Err(e) => return tracing::warn!("theme: cannot watch the registry: {e}"),
            };
            while key.wait().is_ok() {
                s.update(read());
            }
        });
        if let Err(e) = started {
            tracing::warn!("theme watcher thread: {e}");
        }
        Self { shared }
    }
}

impl ThemeSource for RegistryTheme {
    fn prefers_dark(&self) -> Option<bool> {
        self.shared.dark.lock().ok().and_then(|d| *d)
    }
    fn subscribe(&self, on_change: ThemeListener) {
        if let Ok(mut l) = self.shared.listeners.lock() {
            l.push(Arc::new(on_change));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_value_matches_source() {
        assert_eq!(RegistryTheme::spawn().prefers_dark(), read());
    }
}
