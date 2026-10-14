//! Settings loader: one test per spec scenario plus precedence edges.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use rxt_core::{ThemeMode, UiScale};
use rxt_store::settings::{CliOverrides, Settings, load, save};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rxt-settings-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, text).unwrap();
    p
}

fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

#[test]
fn missing_file_gives_defaults() {
    let dir = temp_dir("missing");
    let out = load(&dir.join("config.toml"), None, &[], &CliOverrides::default());
    assert_eq!(out.settings, Settings::default());
    assert!(out.notices.is_empty() && out.errors.is_empty());
    assert!(!out.settings.agent_control);
}

#[test]
fn env_file_beats_toml() {
    let dir = temp_dir("precedence");
    let cfg = write(&dir, "config.toml", "theme = \"light\"\nlog_level = \"warn\"\n");
    let dev = write(&dir, "dev.env", "RXT_THEME=dark\nOTHER=1\n");
    let out = load(&cfg, Some(&dev), &[], &CliOverrides::default());
    assert_eq!(out.settings.theme, ThemeMode::Dark);
    assert_eq!(out.settings.log_level, "warn");
    assert!(out.errors.is_empty(), "{:?}", out.errors);
}

#[test]
fn process_env_beats_env_file_and_cli_beats_both() {
    let dir = temp_dir("cli");
    let cfg = write(&dir, "config.toml", "theme = \"light\"\n");
    let dev = write(&dir, "dev.env", "RXT_THEME=dark\nRXT_AGENT_CONTROL=true\nRXT_DATA_DIR=/data/env-file\n");
    let process = env(&[("RXT_DATA_DIR", "/data/process"), ("HOME", "/home/alice")]);
    let out = load(&cfg, Some(&dev), &process, &CliOverrides::default());
    assert_eq!(out.settings.data_dir.as_deref(), Some(Path::new("/data/process")));
    assert!(out.settings.agent_control);

    let cli = CliOverrides { theme: Some(ThemeMode::Light), agent_control: Some(false), ..Default::default() };
    let out = load(&cfg, Some(&dev), &process, &cli);
    assert_eq!(out.settings.theme, ThemeMode::Light);
    assert!(!out.settings.agent_control);
}

#[test]
fn secret_key_in_toml_is_refused_naming_key_and_file() {
    let dir = temp_dir("secret-toml");
    let cfg = write(&dir, "config.toml", "theme = \"dark\"\n[terminal]\nSSH_Password = \"x\"\n");
    let out = load(&cfg, None, &[], &CliOverrides::default());
    assert_eq!(out.settings, Settings::default());
    let msg = out.errors[0].to_string();
    assert!(msg.contains("terminal.SSH_Password") && msg.contains("config.toml"), "{msg}");
    assert!(cfg.exists(), "a refused file is not moved");
}

#[test]
fn secret_key_in_env_file_is_refused() {
    let dir = temp_dir("secret-env");
    let dev = write(&dir, "dev.env", "RXT_THEME=dark\nDB_PASSWORD=x\n");
    let out = load(&dir.join("config.toml"), Some(&dev), &[], &CliOverrides::default());
    assert_eq!(out.settings.theme, ThemeMode::System);
    let msg = out.errors[0].to_string();
    assert!(msg.contains("DB_PASSWORD") && msg.contains("dev.env"), "{msg}");
}

#[test]
fn corrupt_toml_is_backed_up_with_notice() {
    let dir = temp_dir("corrupt");
    let cfg = write(&dir, "config.toml", "theme = [unclosed\n");
    let out = load(&cfg, None, &[], &CliOverrides::default());
    assert_eq!(out.settings, Settings::default());
    assert!(out.notices[0].contains("config.toml"), "{:?}", out.notices);
    assert!(!cfg.exists());
    let backups: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with("config.toml.corrupt-"))
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(std::fs::read_to_string(dir.join(&backups[0])).unwrap(), "theme = [unclosed\n");
}

#[test]
fn wrong_value_type_is_treated_as_corrupt() {
    let dir = temp_dir("badtype");
    let cfg = write(&dir, "config.toml", "theme = 5\n");
    let out = load(&cfg, None, &[], &CliOverrides::default());
    assert_eq!(out.settings, Settings::default());
    assert_eq!(out.notices.len(), 1);
}

#[test]
fn invalid_env_value_is_reported_and_ignored() {
    let out = load(Path::new("/nonexistent/config.toml"), None, &env(&[("RXT_THEME", "purple")]), &Default::default());
    assert_eq!(out.settings.theme, ThemeMode::System);
    assert!(out.errors[0].to_string().contains("RXT_THEME"));
}

#[test]
fn save_then_load_round_trips() {
    let dir = temp_dir("save");
    let path = dir.join("sub/config.toml");
    let mut s = Settings { theme: ThemeMode::Dark, agent_control: true, ..Settings::default() };
    s.shortcuts.insert("tab.close".into(), "Ctrl+W".into());
    s.data_dir = Some("/data".into());
    save(&path, &s).unwrap();
    let out = load(&path, None, &[], &CliOverrides::default());
    assert_eq!(out.settings, s);
    assert!(out.errors.is_empty() && out.notices.is_empty());
}

#[test]
fn old_settings_file_gets_new_ui_defaults_and_round_trips() {
    let dir = temp_dir("old-ui");
    let cfg = write(&dir, "config.toml", "theme = \"dark\"\nsidebar_open = false\nsidebar_width = 300.0\n");
    let out = load(&cfg, None, &[], &CliOverrides::default());
    assert!(out.errors.is_empty() && out.notices.is_empty(), "{:?} {:?}", out.errors, out.notices);
    let s = out.settings;
    assert_eq!((s.theme, s.sidebar_open, s.sidebar_width), (ThemeMode::Dark, false, 300.0));
    assert_eq!(s.sidebar_page, "sessions");
    assert!(s.show_tab_numbers && s.show_close_buttons && s.toolbar_captions);
    assert_eq!(s.ui_scale, UiScale::Auto);
    assert!(s.first_run_theme_done, "an existing file means an existing user");
    assert!(!Settings::default().first_run_theme_done, "a fresh install shows the chooser");
    assert!(s.remote_monitoring);
    assert_eq!(s.remote_monitoring_interval_secs, 2);

    let changed = Settings { ui_scale: UiScale::P150, show_tab_numbers: false, sidebar_page: "sftp".into(), ..s };
    save(&cfg, &changed).unwrap();
    assert!(std::fs::read_to_string(&cfg).unwrap().contains("ui_scale = \"150\""));
    assert_eq!(load(&cfg, None, &[], &CliOverrides::default()).settings, changed);
}

#[test]
fn monitoring_interval_is_clamped() {
    let dir = temp_dir("monitor");
    let cfg = write(&dir, "config.toml", "remote_monitoring = false\nremote_monitoring_interval_secs = 90\n");
    let s = load(&cfg, None, &[], &CliOverrides::default()).settings;
    assert!(!s.remote_monitoring);
    assert_eq!(s.remote_monitoring_interval_secs, 30);
    let cfg = write(&dir, "config.toml", "remote_monitoring_interval_secs = 0\n");
    assert_eq!(load(&cfg, None, &[], &CliOverrides::default()).settings.remote_monitoring_interval_secs, 1);
}
