//! Desktop color scheme from the XDG Settings portal
//! (`org.freedesktop.appearance color-scheme`), with live updates.
//!
//! A background thread owns a small current-thread tokio runtime. A missing
//! D-Bus session or portal only leaves the value at `None` (the UI falls back to Light).

use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use ashpd::desktop::settings::{ColorScheme, Settings};
use ashpd::zbus::export::futures_core::Stream;
use rxt_platform::{ThemeListener, ThemeSource};

/// How long [`PortalTheme::spawn`] waits for the first portal answer.
const STARTUP_WAIT: Duration = Duration::from_millis(300);
/// Bound on connecting to D-Bus and reading the initial value.
const PORTAL_TIMEOUT: Duration = Duration::from_secs(3);

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

/// Theme source backed by the Settings portal.
pub struct PortalTheme {
    shared: Arc<Shared>,
}

impl PortalTheme {
    /// Start watching the session bus portal; never fails or blocks for long.
    pub fn spawn() -> Self {
        Self::spawn_at(None)
    }

    /// Like [`spawn`](Self::spawn) but on an explicit D-Bus address (tests).
    pub fn spawn_at(address: Option<String>) -> Self {
        let shared = Arc::new(Shared::default());
        let (ready_tx, ready_rx) = mpsc::channel();
        let s = shared.clone();
        let started = std::thread::Builder::new().name("rxt-theme-portal".into()).spawn(move || {
            match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt.block_on(watch(address, s, ready_tx)),
                Err(e) => tracing::warn!("theme portal runtime: {e}"),
            }
        });
        match started {
            Ok(_) => {
                let _ = ready_rx.recv_timeout(STARTUP_WAIT);
            }
            Err(e) => tracing::warn!("theme portal thread: {e}"),
        }
        Self { shared }
    }
}

impl ThemeSource for PortalTheme {
    fn prefers_dark(&self) -> Option<bool> {
        self.shared.dark.lock().ok().and_then(|d| *d)
    }
    fn subscribe(&self, on_change: ThemeListener) {
        if let Ok(mut l) = self.shared.listeners.lock() {
            l.push(Arc::from(on_change));
        }
    }
}

/// Portal value → `prefers_dark` (1 = dark, 2 = light, 0 = no preference).
pub(crate) fn map_scheme(scheme: ColorScheme) -> Option<bool> {
    match scheme {
        ColorScheme::PreferDark => Some(true),
        ColorScheme::PreferLight => Some(false),
        ColorScheme::NoPreference => None,
    }
}

async fn connect(address: Option<String>) -> Result<Settings, String> {
    match address {
        None => Settings::new().await.map_err(|e| e.to_string()),
        Some(a) => {
            let conn = ashpd::zbus::connection::Builder::address(a.as_str())
                .map_err(|e| e.to_string())?
                .build()
                .await
                .map_err(|e| e.to_string())?;
            Settings::with_connection(conn).await.map_err(|e| e.to_string())
        }
    }
}

async fn watch(address: Option<String>, shared: Arc<Shared>, ready: mpsc::Sender<()>) {
    let initial = tokio::time::timeout(PORTAL_TIMEOUT, async {
        let settings = connect(address).await?;
        let scheme = settings.color_scheme().await.map_err(|e| e.to_string())?;
        Ok::<_, String>((settings, scheme))
    })
    .await
    .unwrap_or_else(|_| Err("timed out".into()));
    let settings = match initial {
        Ok((settings, scheme)) => {
            shared.update(map_scheme(scheme));
            settings
        }
        Err(e) => {
            tracing::info!("color-scheme portal unavailable ({e}); using Light fallback");
            let _ = ready.send(());
            return;
        }
    };
    let _ = ready.send(());
    let stream = match settings.receive_color_scheme_changed().await {
        Ok(s) => s,
        Err(e) => {
            tracing::info!("color-scheme live updates unavailable: {e}");
            return;
        }
    };
    let mut stream = std::pin::pin!(stream);
    while let Some(scheme) = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
        shared.update(map_scheme(scheme));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn maps_color_scheme() {
        assert_eq!(map_scheme(ColorScheme::PreferDark), Some(true));
        assert_eq!(map_scheme(ColorScheme::PreferLight), Some(false));
        assert_eq!(map_scheme(ColorScheme::NoPreference), None);
    }

    #[test]
    fn missing_bus_falls_back_quickly() {
        let start = Instant::now();
        let t = PortalTheme::spawn_at(Some("unix:path=/nonexistent/rxt-bus".into()));
        assert!(start.elapsed() <= STARTUP_WAIT + Duration::from_millis(200));
        assert_eq!(t.prefers_dark(), None);
        t.subscribe(Box::new(|_| {}));
    }

    #[test]
    fn listeners_fire_on_change_only() {
        let t = PortalTheme { shared: Arc::default() };
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
