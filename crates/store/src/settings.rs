//! Basic settings: `config.toml` + explicit `--env-file` + `RXT_*`, precedence
//! CLI > env/.env > TOML > default; secret-like keys refused; corrupt TOML backed up.
//!
//! Environment keys (process env overrides the `.env` file):
//!
//! | Key | Field | Values |
//! |---|---|---|
//! | `RXT_THEME` | `theme` | `light`, `dark`, `system` |
//! | `RXT_LOG_LEVEL` | `log_level` | tracing filter, e.g. `info`, `debug` |
//! | `RXT_DATA_DIR` | `data_dir` | path |
//! | `RXT_AGENT_CONTROL` | `agent_control` | `true`/`false`/`1`/`0`/`yes`/`no`/`on`/`off` |

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use rxt_core::{CursorShape, Error, Result, TerminalPrefs, ThemeMode, UiScale};
use serde::{Deserialize, Serialize};

/// Substrings (lower-case) that mark a key as secret-like; such keys never belong in plain settings.
pub const SECRET_KEY_DENYLIST: [&str; 8] =
    ["password", "passwd", "secret", "token", "passphrase", "private_key", "key_material", "api_key"];

/// Non-secret application settings. Every field has a default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeMode,
    /// Tracing filter directive.
    pub log_level: String,
    /// Data directory override; `None` uses the platform default.
    pub data_dir: Option<PathBuf>,
    /// Agent control (CLI live commands and MCP); off by default.
    pub agent_control: bool,
    /// Global terminal defaults; sessions override per field.
    pub terminal: TerminalPrefs,
    pub copy_on_select: bool,
    pub paste_guard: bool,
    pub confirm_close: bool,
    pub sidebar_open: bool,
    pub sidebar_width: f32,
    /// Command id → shortcut text such as `Ctrl+Shift+N`; overrides the built-in bindings.
    pub shortcuts: BTreeMap<String, String>,
    /// Key combination that releases a remote desktop's keyboard grab.
    pub desktop_release: String,
    /// Active sidebar page: `sessions` or `sftp`.
    pub sidebar_page: String,
    /// Show the SFTP browser when the active SSH tab logs in or is selected.
    pub sftp_auto_switch: bool,
    /// Print the session info box into SSH and Telnet terminals when they connect.
    pub show_session_banner: bool,
    /// Prefix session tabs with `N. `.
    pub show_tab_numbers: bool,
    /// Close button inside each session tab.
    pub show_close_buttons: bool,
    /// Captions under toolbar icons.
    pub toolbar_captions: bool,
    /// Interface size; Auto follows the windowing system.
    pub ui_scale: UiScale,
    /// Remote monitoring bar on SSH tabs (sessions may override); on by default.
    pub remote_monitoring: bool,
    /// Remote monitoring refresh interval, seconds (clamped to 1–30 on load).
    pub remote_monitoring_interval_secs: u32,
    /// Desktop notifications for background transfers and dropped sessions; on by default.
    pub notifications: bool,
    /// First-run Light/Dark chooser answered. Missing in an existing file means an existing
    /// user (`true`); a fresh install starts from [`Default`] (`false`).
    #[serde(default = "first_run_done_in_old_files")]
    pub first_run_theme_done: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            log_level: "info".into(),
            data_dir: None,
            agent_control: false,
            terminal: TerminalPrefs {
                font_family: Some("monospace".into()),
                font_size: Some(13.0),
                cursor_shape: Some(CursorShape::Beam),
                cursor_blink: Some(true),
                scheme: None,
                scrollback_lines: Some(10_000),
                ..TerminalPrefs::default()
            },
            copy_on_select: true,
            paste_guard: true,
            confirm_close: true,
            sidebar_open: true,
            sidebar_width: 240.0,
            shortcuts: BTreeMap::new(),
            desktop_release: "Ctrl+Alt".into(),
            sidebar_page: "sessions".into(),
            sftp_auto_switch: true,
            show_session_banner: true,
            show_tab_numbers: true,
            show_close_buttons: true,
            toolbar_captions: true,
            ui_scale: UiScale::Auto,
            remote_monitoring: true,
            remote_monitoring_interval_secs: 2,
            notifications: true,
            first_run_theme_done: false,
        }
    }
}

fn first_run_done_in_old_files() -> bool {
    true
}

/// Command-line flags; `Some` wins over every other source.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CliOverrides {
    pub theme: Option<ThemeMode>,
    pub log_level: Option<String>,
    pub data_dir: Option<PathBuf>,
    pub agent_control: Option<bool>,
}

/// Result of [`load`]: usable settings plus messages for the UI. Loading never fails.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadOutcome {
    pub settings: Settings,
    /// Informational messages (e.g. corrupt file backed up).
    pub notices: Vec<String>,
    /// Refused configuration (secret-like keys, unreadable files, invalid values).
    pub errors: Vec<Error>,
}

/// Load settings. `env` is the process environment (only `RXT_*` keys are used); the `.env`
/// file is read as a list and never mutates the process environment.
pub fn load(config_path: &Path, env_file: Option<&Path>, env: &[(String, String)], cli: &CliOverrides) -> LoadOutcome {
    let mut out = LoadOutcome { settings: Settings::default(), notices: Vec::new(), errors: Vec::new() };
    load_toml(config_path, &mut out);

    if let Some(path) = env_file {
        match read_env_file(path) {
            Ok(pairs) => apply_env(&pairs, path, &mut out),
            Err(e) => out.errors.push(e),
        }
    }
    apply_env(env, Path::new("environment"), &mut out);

    let s = &mut out.settings;
    if let Some(v) = cli.theme {
        s.theme = v;
    }
    if let Some(v) = &cli.log_level {
        s.log_level.clone_from(v);
    }
    if let Some(v) = &cli.data_dir {
        s.data_dir = Some(v.clone());
    }
    if let Some(v) = cli.agent_control {
        s.agent_control = v;
    }
    s.remote_monitoring_interval_secs = s.remote_monitoring_interval_secs.clamp(1, 30);
    out
}

fn load_toml(path: &Path, out: &mut LoadOutcome) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            out.errors.push(Error::Io(format!("cannot read settings file {}: {e}", path.display())));
            return;
        }
    };
    let parsed = toml::from_str::<toml::Table>(&text).map_err(|e| e.to_string()).and_then(|table| {
        if let Some(key) = find_secret_key(&table, "") {
            return Ok(Err(key));
        }
        Settings::deserialize(table).map(Ok).map_err(|e| e.to_string())
    });
    match parsed {
        Ok(Ok(settings)) => out.settings = settings,
        Ok(Err(key)) => out.errors.push(secret_error(&key, path)),
        Err(reason) => backup_corrupt(path, &reason, out),
    }
}

fn backup_corrupt(path: &Path, reason: &str, out: &mut LoadOutcome) {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut backup = path.as_os_str().to_owned();
    backup.push(format!(".corrupt-{stamp}"));
    let backup = PathBuf::from(backup);
    match std::fs::rename(path, &backup) {
        Ok(()) => out.notices.push(format!(
            "settings file {} is invalid ({}); using defaults, backup kept at {}",
            path.display(),
            reason.trim(),
            backup.display()
        )),
        Err(e) => out.errors.push(Error::Io(format!(
            "settings file {} is invalid and could not be backed up: {e}; using defaults",
            path.display()
        ))),
    }
}

/// First secret-like key in `table` (dotted path), searching nested tables and arrays.
pub fn find_secret_key(table: &toml::Table, prefix: &str) -> Option<String> {
    fn walk(value: &toml::Value, path: &str) -> Option<String> {
        match value {
            toml::Value::Table(t) => find_secret_key(t, path),
            toml::Value::Array(items) => items.iter().find_map(|v| walk(v, path)),
            _ => None,
        }
    }
    table.iter().find_map(|(k, v)| {
        let path = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
        if is_secret_key(k) { Some(path) } else { walk(v, &path) }
    })
}

/// Whether a key name looks like it holds a secret (case-insensitive substring match).
pub fn is_secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    SECRET_KEY_DENYLIST.iter().any(|d| key.contains(d))
}

fn secret_error(key: &str, file: &Path) -> Error {
    Error::Invalid(format!(
        "secret-like key `{key}` in {}: configuration refused (store secrets in the encrypted vault)",
        file.display()
    ))
}

fn read_env_file(path: &Path) -> Result<Vec<(String, String)>> {
    let iter = dotenvy::from_path_iter(path)
        .map_err(|e| Error::Io(format!("cannot read env file {}: {e}", path.display())))?;
    let pairs = iter
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| Error::Invalid(format!("env file {} is invalid: {e}", path.display())))?;
    match pairs.iter().find(|(k, _)| is_secret_key(k)) {
        Some((k, _)) => Err(secret_error(k, path)),
        None => Ok(pairs),
    }
}

fn apply_env(pairs: &[(String, String)], source: &Path, out: &mut LoadOutcome) {
    let s = &mut out.settings;
    for (key, value) in pairs {
        let bad = |what: &str| Error::Invalid(format!("{key}={value} in {}: expected {what}", source.display()));
        match key.as_str() {
            "RXT_THEME" => match value.to_ascii_lowercase().as_str() {
                "light" => s.theme = ThemeMode::Light,
                "dark" => s.theme = ThemeMode::Dark,
                "system" => s.theme = ThemeMode::System,
                _ => out.errors.push(bad("light, dark or system")),
            },
            "RXT_LOG_LEVEL" => s.log_level.clone_from(value),
            "RXT_DATA_DIR" => s.data_dir = Some(PathBuf::from(value)),
            "RXT_AGENT_CONTROL" => match parse_bool(value) {
                Some(b) => s.agent_control = b,
                None => out.errors.push(bad("true or false")),
            },
            _ => {}
        }
    }
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Write settings atomically (temp file, fsync, rename).
pub fn save(path: &Path, settings: &Settings) -> Result<()> {
    let text = toml::to_string_pretty(settings).map_err(|e| Error::Invalid(e.to_string()))?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
