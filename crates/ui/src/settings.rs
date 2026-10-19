//! Persisted UI settings, configurable shortcuts and the Settings window.

use std::collections::BTreeMap;

use egui::{Key, KeyboardShortcut, Modifiers};
use rxt_core::{
    CommandId, CursorShape, DiagSettings, Metrics, TerminalPrefs, TerminalScheme, ThemeMode, Tokens, UiScale,
};
use rxt_store::settings::Settings as StoreSettings;
use serde::{Deserialize, Serialize};

use crate::agent::AgentSettings;
use crate::app::Action;
use crate::icons::Icons;
use crate::theme::color;
use crate::widgets;

/// Global settings, persisted in `config.toml` ([`rxt_store::settings`]); eframe storage only
/// when no settings file is configured (tests).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    pub theme: ThemeMode,
    /// Global terminal defaults; sessions override per field.
    pub terminal: TerminalPrefs,
    pub copy_on_select: bool,
    pub right_click_paste: bool,
    pub paste_guard: bool,
    pub confirm_close: bool,
    /// Command id → shortcut text such as `Ctrl+Shift+N`; missing or empty means unbound.
    pub shortcuts: BTreeMap<String, String>,
    pub sidebar_open: bool,
    pub sidebar_width: f32,
    /// Key combination that releases a remote desktop's keyboard grab, e.g. `Ctrl+Alt`.
    pub desktop_release: String,
    /// Local control channel for the CLI and MCP (agent-control spec); off by default.
    pub agent_control: bool,
    /// Page chosen with the sidebar strip: `sessions` or `sftp` (automatic switches are not saved).
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
    /// Remote monitoring bar on SSH tabs (sessions may override).
    pub remote_monitoring: bool,
    /// Remote monitoring refresh interval, seconds (1–30).
    pub remote_monitoring_interval_secs: u32,
    /// Desktop notifications while the window is not focused (transfers, dropped sessions).
    pub notifications: bool,
    /// Rendering backend for the next start (`auto`, `gl`, `dx12`, `vulkan`, `cpu`).
    pub gpu_backend: String,
    /// First-run Light/Dark chooser answered. Missing in an existing file means an existing
    /// user (`true`); a fresh install starts from [`Default`] (`false`).
    #[serde(default = "first_run_done_in_old_files")]
    pub first_run_theme_done: bool,
    /// The first-run MobaXterm.ini import offer was shown (so it is not offered again).
    #[serde(default = "first_run_done_in_old_files")]
    pub first_run_moba_offered: bool,
    /// Settings → Diagnostics (the data-stream switch is not saved).
    #[serde(default)]
    pub diagnostics: DiagSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
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
            right_click_paste: true,
            paste_guard: true,
            confirm_close: true,
            shortcuts: default_shortcuts(),
            sidebar_open: true,
            sidebar_width: 240.0,
            desktop_release: "Ctrl+Alt".into(),
            agent_control: false,
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
            gpu_backend: "auto".into(),
            first_run_theme_done: false,
            first_run_moba_offered: false,
            diagnostics: DiagSettings::default(),
        }
    }
}

fn first_run_done_in_old_files() -> bool {
    true
}

/// Default shortcuts. None uses plain Ctrl+letter or Alt, which terminals need
/// (Ctrl+C must reach the remote as 0x03). Copy/paste stay with the terminal widget.
pub(crate) fn default_shortcuts() -> BTreeMap<String, String> {
    [
        (CommandId::NEW_SESSION, "Ctrl+Shift+N"),
        (CommandId::NEW_LOCAL_TERMINAL, "Ctrl+Shift+T"),
        (CommandId::QUICK_CONNECT, "Ctrl+Shift+K"),
        (CommandId::CLOSE_TAB, "Ctrl+Shift+W"),
        (CommandId::NEXT_TAB, "Ctrl+Tab"),
        (CommandId::PREV_TAB, "Ctrl+Shift+Tab"),
        (CommandId::DUPLICATE_TAB, "Ctrl+Shift+D"),
        (CommandId::RECONNECT, "Ctrl+Shift+R"),
        (CommandId::FIND, "Ctrl+Shift+F"),
        (CommandId::SETTINGS, "Ctrl+Shift+Comma"),
        (CommandId::TOGGLE_SIDEBAR, "Ctrl+Shift+B"),
        (CommandId::TOGGLE_FULLSCREEN, "F11"),
        (CommandId::MULTIEXEC, "Ctrl+Shift+M"),
        (CommandId::TUNNELS, "Ctrl+Shift+U"),
        (CommandId::FOCUS_MENU, "Ctrl+Shift+F10"),
        (CommandId::FIND_SESSION, "Ctrl+Shift+S"),
    ]
    .into_iter()
    .map(|(c, s)| (c.as_str().to_owned(), s.to_owned()))
    .collect()
}

/// Parse `Ctrl+Shift+N`-style text.
pub(crate) fn parse_shortcut(text: &str) -> Option<KeyboardShortcut> {
    let mut modifiers = Modifiers::NONE;
    let mut key = None;
    for part in text.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers |= Modifiers::CTRL,
            "shift" => modifiers |= Modifiers::SHIFT,
            "alt" => modifiers |= Modifiers::ALT,
            _ if key.is_none() => {
                key = Some(Key::from_name(part).or_else(|| Key::from_name(&part.to_ascii_uppercase()))?)
            }
            _ => return None,
        }
    }
    Some(KeyboardShortcut::new(modifiers, key?))
}

/// Shortcut text in canonical form.
pub(crate) fn format_shortcut(s: &KeyboardShortcut) -> String {
    let mut out = String::new();
    if s.modifiers.ctrl {
        out.push_str("Ctrl+");
    }
    if s.modifiers.alt {
        out.push_str("Alt+");
    }
    if s.modifiers.shift {
        out.push_str("Shift+");
    }
    out.push_str(s.logical_key.name());
    out
}

/// Whether an application shortcut leaves terminal input intact: Ctrl+Shift+key,
/// Ctrl+Tab and bare F11/F12. Plain keys, Shift+key, Ctrl+key and Alt+key are refused
/// because terminals turn them into control bytes or escape sequences.
pub(crate) fn shortcut_allowed(s: &KeyboardShortcut) -> bool {
    let m = s.modifiers;
    if m.alt {
        return false;
    }
    match (m.ctrl, m.shift) {
        (true, true) => true,
        (true, false) => s.logical_key == Key::Tab,
        (false, false) => matches!(s.logical_key, Key::F11 | Key::F12),
        (false, true) => false,
    }
}

impl Settings {
    /// UI view of the loaded settings; shortcuts in the file override the defaults.
    pub(crate) fn from_store(s: &StoreSettings) -> Self {
        let mut shortcuts = default_shortcuts();
        shortcuts.extend(s.shortcuts.clone());
        Self {
            theme: s.theme,
            terminal: s.terminal.clone(),
            copy_on_select: s.copy_on_select,
            right_click_paste: s.right_click_paste,
            paste_guard: s.paste_guard,
            confirm_close: s.confirm_close,
            shortcuts,
            sidebar_open: s.sidebar_open,
            sidebar_width: s.sidebar_width,
            desktop_release: s.desktop_release.clone(),
            agent_control: s.agent_control,
            sidebar_page: s.sidebar_page.clone(),
            sftp_auto_switch: s.sftp_auto_switch,
            show_session_banner: s.show_session_banner,
            show_tab_numbers: s.show_tab_numbers,
            show_close_buttons: s.show_close_buttons,
            toolbar_captions: s.toolbar_captions,
            ui_scale: s.ui_scale,
            remote_monitoring: s.remote_monitoring,
            remote_monitoring_interval_secs: s.remote_monitoring_interval_secs,
            notifications: s.notifications,
            gpu_backend: s.gpu_backend.clone(),
            first_run_theme_done: s.first_run_theme_done,
            first_run_moba_offered: true,
            diagnostics: s.diagnostics.clone(),
        }
    }

    /// Settings to write: `base` (non-UI fields as loaded) with the UI fields; only shortcuts
    /// that differ from the defaults are written.
    pub(crate) fn to_store(&self, base: &StoreSettings) -> StoreSettings {
        let defaults = default_shortcuts();
        StoreSettings {
            theme: self.theme,
            terminal: self.terminal.clone(),
            copy_on_select: self.copy_on_select,
            right_click_paste: self.right_click_paste,
            paste_guard: self.paste_guard,
            confirm_close: self.confirm_close,
            shortcuts: self
                .shortcuts
                .iter()
                .filter(|(k, v)| defaults.get(*k) != Some(*v))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            sidebar_open: self.sidebar_open,
            sidebar_width: self.sidebar_width,
            desktop_release: self.desktop_release.clone(),
            agent_control: self.agent_control,
            sidebar_page: self.sidebar_page.clone(),
            sftp_auto_switch: self.sftp_auto_switch,
            show_session_banner: self.show_session_banner,
            show_tab_numbers: self.show_tab_numbers,
            show_close_buttons: self.show_close_buttons,
            toolbar_captions: self.toolbar_captions,
            ui_scale: self.ui_scale,
            remote_monitoring: self.remote_monitoring,
            remote_monitoring_interval_secs: self.remote_monitoring_interval_secs,
            notifications: self.notifications,
            gpu_backend: self.gpu_backend.clone(),
            first_run_theme_done: self.first_run_theme_done,
            diagnostics: self.diagnostics.clone(),
            ..base.clone()
        }
    }

    /// Valid bindings in use: (command, shortcut).
    pub(crate) fn bindings(&self) -> Vec<(CommandId, KeyboardShortcut)> {
        CommandId::ALL
            .into_iter()
            .filter_map(|c| {
                let sc = parse_shortcut(self.shortcuts.get(c.as_str())?)?;
                shortcut_allowed(&sc).then_some((c, sc))
            })
            .collect()
    }

    /// Display text of the shortcut bound to `cmd`, if valid.
    pub(crate) fn shortcut_text(&self, cmd: CommandId) -> Option<String> {
        self.bindings().into_iter().find(|(c, _)| *c == cmd).map(|(_, s)| format_shortcut(&s))
    }
}

/// Human label for a command id.
pub(crate) fn command_label(c: CommandId) -> &'static str {
    match c {
        CommandId::NEW_SESSION => "New session",
        CommandId::NEW_LOCAL_TERMINAL => "New local terminal",
        CommandId::QUICK_CONNECT => "Quick connect",
        CommandId::CLOSE_TAB => "Close tab",
        CommandId::CLOSE_OTHERS => "Close other tabs",
        CommandId::NEXT_TAB => "Next tab",
        CommandId::PREV_TAB => "Previous tab",
        CommandId::RENAME_TAB => "Rename tab",
        CommandId::DUPLICATE_TAB => "Duplicate tab",
        CommandId::RECONNECT => "Reconnect",
        CommandId::FIND => "Find in terminal",
        CommandId::COPY => "Copy",
        CommandId::PASTE => "Paste",
        CommandId::SAVE_TEXT => "Save terminal text",
        CommandId::SETTINGS => "Settings",
        CommandId::TOGGLE_SIDEBAR => "Toggle sidebar",
        CommandId::TOGGLE_FULLSCREEN => "Toggle fullscreen",
        CommandId::MULTIEXEC => "MultiExec",
        CommandId::TUNNELS => "Tunnels",
        CommandId::HELP => "Help",
        CommandId::EXIT => "Exit",
        CommandId::IMPORT_SESSIONS => "Import sessions",
        CommandId::EXPORT_SESSIONS => "Export sessions",
        CommandId::ABOUT => "About rustXterm",
        CommandId::CHANGE_PASSWORD => "Change master password",
        CommandId::THEME_LIGHT => "Light theme",
        CommandId::THEME_DARK => "Dark theme",
        CommandId::THEME_SYSTEM => "System theme",
        CommandId::FOCUS_MENU => "Focus menu bar",
        CommandId::FIND_SESSION => "Find session",
        CommandId::SERIAL_SEND_BREAK => "Serial: send break",
        CommandId::SERIAL_SET_LINES => "Serial: toggle DTR",
        CommandId::SERIAL_LIST_PORTS => "Serial: list ports",
        _ => "Command",
    }
}

/// Encrypted-store state shown in the Settings window.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StoreState {
    pub unlocked: bool,
    pub keyring: bool,
}

/// Settings window pages, in tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SettingsTab {
    #[default]
    General,
    Terminal,
    Ssh,
    Display,
    Shortcuts,
    Security,
    AgentControl,
    Diagnostics,
}

impl SettingsTab {
    pub(crate) const ALL: [Self; 8] = [
        Self::General,
        Self::Terminal,
        Self::Ssh,
        Self::Display,
        Self::Shortcuts,
        Self::Security,
        Self::AgentControl,
        Self::Diagnostics,
    ];

    /// (label, catalog icon).
    fn label_icon(self) -> (&'static str, &'static str) {
        match self {
            Self::General => ("General", "gear"),
            Self::Terminal => ("Terminal", "terminal"),
            Self::Ssh => ("SSH", "ssh"),
            Self::Display => ("Display", "monitor"),
            Self::Shortcuts => ("Shortcuts", "keyboard"),
            Self::Security => ("Security", "lock"),
            Self::AgentControl => ("Agent control", "rocket"),
            Self::Diagnostics => ("Diagnostics", "terminal-list"),
        }
    }
}

/// Outer size of the Settings window (design R8).
pub(crate) const SETTINGS_SIZE: egui::Vec2 = egui::vec2(800.0, 560.0);

/// egui temp-memory key of the last opened tab; kept for the application's lifetime only.
fn tab_id() -> egui::Id {
    egui::Id::new("settings-last-tab")
}

/// Makes the Settings window open on `tab` next time it is shown.
pub(crate) fn open_on(ctx: &egui::Context, tab: SettingsTab) {
    ctx.data_mut(|d| d.insert_temp(tab_id(), tab));
}

/// The Diagnostics page's live state, owned by the app.
pub(crate) struct DiagPage<'a> {
    pub state: &'a crate::diagnostics::DiagState,
    pub stream: &'a mut crate::diagnostics::StreamUi,
}

/// Settings window; edits apply immediately, OK, Escape and the close button only close it.
/// `store` enables the Security page actions.
#[allow(clippy::too_many_arguments)]
pub(crate) fn window(
    ctx: &egui::Context,
    open: &mut bool,
    s: &mut Settings,
    store: Option<StoreState>,
    agent: AgentSettings<'_>,
    diag: DiagPage<'_>,
    icons: &Icons,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) {
    let mut tab: SettingsTab = ctx.data(|d| d.get_temp(tab_id())).unwrap_or_default();
    let mut ok = false;
    let mut close = false;
    widgets::plain_window(ctx, "Settings")
        .resizable(false)
        .fixed_size(SETTINGS_SIZE)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            close = widgets::window_title(ui, "Settings");
            widgets::window_body(ui, |ui| body(ui, &mut tab, &mut ok, s, store, agent, diag, icons, tokens, out));
        });
    let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !egui::Popup::is_any_open(ctx);
    if ok || escape || close {
        *open = false;
    }
    ctx.data_mut(|d| d.insert_temp(tab_id(), tab));
}

/// Tab row, the bordered page and the centered OK button.
#[allow(clippy::too_many_arguments)]
fn body(
    ui: &mut egui::Ui,
    tab: &mut SettingsTab,
    ok: &mut bool,
    s: &mut Settings,
    store: Option<StoreState>,
    agent: AgentSettings<'_>,
    diag: DiagPage<'_>,
    icons: &Icons,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) {
    let m = Metrics::DEFAULT;
    ui.spacing_mut().item_spacing.y = 0.0;
    let tab_rect = {
        let mut selected = egui::Rect::NOTHING;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            // Tabs like the session editor's: the selected one is on panel and joins the page.
            for t in SettingsTab::ALL {
                let (label, icon) = t.label_icon();
                let on = *tab == t;
                let b = egui::Button::opt_image_and_text(icons.image(icon, m.icon_toolbar), Some(label.into()))
                    .fill(color(if on { tokens.panel } else { tokens.toolbar }))
                    .stroke(egui::Stroke::new(m.stroke, color(tokens.border)))
                    .min_size(egui::vec2(0.0, m.icon_toolbar + 2.0 * m.space[1]));
                let r = ui.add(b);
                r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, label));
                if on {
                    selected = r.rect;
                }
                if r.clicked() {
                    *tab = t;
                }
            }
        });
        selected
    };
    ui.spacing_mut().item_spacing.y = 0.0;
    let footer = m.dialog_button.1 + 2.0 * m.space[3];
    let fill = color(tokens.panel);
    let page_frame = egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(m.stroke, color(tokens.border)))
        .inner_margin(m.space[3])
        .show(ui, |ui| {
            ui.set_min_size(egui::vec2(ui.available_width(), ui.available_height() - footer));
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(ui.available_height() - footer)
                .show(ui, |ui| page(ui, *tab, s, store, agent, diag, icons, tokens, out));
        });
    let y = page_frame.response.rect.top() + m.stroke / 2.0;
    ui.painter().hline(tab_rect.left() + m.stroke..=tab_rect.right() - m.stroke, y, egui::Stroke::new(m.stroke, fill));
    ui.add_space(m.space[3]);
    *ok = widgets::button_row(ui, icons, &[("ok", "OK", true)]).is_some();
}

/// Titled group box followed by the standard gap.
pub(crate) fn group(ui: &mut egui::Ui, icons: &Icons, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    widgets::group_box(ui, icons, None, title, add);
    ui.add_space(Metrics::DEFAULT.space[3]);
}

/// Untitled bordered box, used where the page content brings its own heading.
pub(crate) fn plain_box(ui: &mut egui::Ui, tokens: &Tokens, add: impl FnOnce(&mut egui::Ui)) {
    let m = Metrics::DEFAULT;
    egui::Frame::new().stroke(egui::Stroke::new(m.stroke, color(tokens.border))).inner_margin(m.space[3]).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        },
    );
    ui.add_space(m.space[3]);
}

fn muted(ui: &mut egui::Ui, tokens: &Tokens, text: &str) {
    ui.label(egui::RichText::new(text).small().color(color(tokens.text_muted)));
}

#[allow(clippy::too_many_arguments)]
fn page(
    ui: &mut egui::Ui,
    tab: SettingsTab,
    s: &mut Settings,
    store: Option<StoreState>,
    agent: AgentSettings<'_>,
    diag: DiagPage<'_>,
    icons: &Icons,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) {
    match tab {
        SettingsTab::General => general(ui, s, icons, tokens),
        SettingsTab::Terminal => terminal(ui, s, icons),
        SettingsTab::Ssh => {
            group(ui, icons, "SSH sessions", |ui| {
                ui.checkbox(&mut s.sftp_auto_switch, "Automatically switch to SFTP browser after login");
                ui.checkbox(&mut s.show_session_banner, "Show session banner");
                muted(
                    ui,
                    tokens,
                    "Host keys, authentication, jump hosts and port forwarding are set per session in the \
                     session editor and in the Tunneling window.",
                );
            });
            group(ui, icons, "Remote monitoring", |ui| {
                ui.checkbox(&mut s.remote_monitoring, "Show remote monitoring bar on SSH tabs");
                ui.horizontal(|ui| {
                    let l = ui.label("Refresh interval");
                    ui.add(egui::DragValue::new(&mut s.remote_monitoring_interval_secs).range(1..=30).suffix(" s"))
                        .labelled_by(l.id);
                });
                muted(
                    ui,
                    tokens,
                    "Sessions can override the bar in their advanced SSH settings. A running tab is toggled \
                     with the bar's close button or the SFTP panel's Remote monitoring button.",
                );
            });
        }
        SettingsTab::Display => display(ui, s, icons),
        SettingsTab::Shortcuts => shortcuts(ui, s, icons, tokens),
        SettingsTab::Security => match store {
            Some(st) => security(ui, st, icons, tokens, out),
            None => group(ui, icons, "Security and data", |ui| {
                muted(ui, tokens, "No session database is configured.");
            }),
        },
        SettingsTab::AgentControl => plain_box(ui, tokens, |ui| {
            crate::agent::settings_section(ui, &mut s.agent_control, agent, out);
        }),
        SettingsTab::Diagnostics => {
            crate::diagnostics::page(ui, &mut s.diagnostics, diag.state, diag.stream, icons, tokens, out);
        }
    }
}

fn general(ui: &mut egui::Ui, s: &mut Settings, icons: &Icons, tokens: &Tokens) {
    group(ui, icons, "Tabs", |ui| {
        ui.checkbox(&mut s.confirm_close, "Confirm before closing a connected tab");
    });
    group(ui, icons, "Notifications", |ui| {
        ui.checkbox(&mut s.notifications, "Show desktop notifications while rustXterm is in the background");
    });
    group(ui, icons, "Remote desktop", |ui| {
        ui.horizontal(|ui| {
            let l = ui.label("Keyboard release keys");
            ui.add(egui::TextEdit::singleline(&mut s.desktop_release).desired_width(160.0)).labelled_by(l.id);
            if crate::desktop::parse_release(&s.desktop_release).is_none() {
                ui.colored_label(color(tokens.danger), "Use modifiers such as Ctrl+Alt, optionally with a key");
            }
        });
    });
}

/// Terminal font picker: the default, then the bundled monospace faces; a name set elsewhere (an
/// imported session, `config.toml`) stays selectable.
pub(crate) fn font_combo(ui: &mut egui::Ui, id: &str, family: &mut Option<String>) -> egui::Response {
    let default = format!("Default ({})", crate::theme::MONO_FONTS[0]);
    let current = family.as_deref().filter(|f| !f.trim().is_empty() && *f != "monospace");
    let mut names: Vec<&str> = crate::theme::MONO_FONTS.to_vec();
    if let Some(c) = current.filter(|c| !names.contains(c)) {
        names.push(c);
    }
    let mut pick = current.map(str::to_owned);
    let r = egui::ComboBox::from_id_salt(id).selected_text(current.unwrap_or(&default)).show_ui(ui, |ui| {
        ui.selectable_value(&mut pick, None, &default);
        for n in names {
            ui.selectable_value(&mut pick, Some(n.to_owned()), n);
        }
    });
    if pick.as_deref() != current {
        *family = Some(pick.unwrap_or_else(|| "monospace".to_owned()));
    }
    r.response
}

fn terminal(ui: &mut egui::Ui, s: &mut Settings, icons: &Icons) {
    let t = &mut s.terminal;
    group(ui, icons, "Font and cursor", |ui| {
        egui::Grid::new("settings-terminal").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            let l = ui.label("Font family");
            font_combo(ui, "settings-font", &mut t.font_family).labelled_by(l.id);
            ui.end_row();
            let l = ui.label("Font size");
            let mut size = t.font_size.unwrap_or(13.0);
            if ui.add(egui::DragValue::new(&mut size).range(6.0..=48.0).speed(0.5)).labelled_by(l.id).changed() {
                t.font_size = Some(size);
            }
            ui.end_row();
            ui.label("Cursor");
            cursor_combo(ui, "settings-cursor", t.cursor_shape.get_or_insert_default());
            ui.end_row();
            ui.label("");
            let mut blink = t.cursor_blink.unwrap_or(true);
            if ui.checkbox(&mut blink, "Blinking cursor").changed() {
                t.cursor_blink = Some(blink);
            }
            ui.end_row();
            ui.label("Color scheme");
            scheme_combo(ui, "settings-scheme", &mut t.scheme);
            ui.end_row();
        });
    });
    group(ui, icons, "Terminal features", |ui| {
        ui.horizontal(|ui| {
            let l = ui.label("Scrollback lines");
            let mut lines = t.scrollback_lines.unwrap_or(10_000);
            if ui.add(egui::DragValue::new(&mut lines).range(0..=1_000_000).speed(100)).labelled_by(l.id).changed() {
                t.scrollback_lines = Some(lines);
            }
        });
        ui.checkbox(&mut s.copy_on_select, "Copy on select");
        ui.checkbox(&mut s.right_click_paste, "Paste with right click (Ctrl+right click opens the menu)");
        ui.checkbox(&mut s.paste_guard, "Confirm multi-line paste");
    });
}

fn skin_label(t: ThemeMode) -> &'static str {
    match t {
        ThemeMode::Light => "Light",
        ThemeMode::Dark => "Dark",
        ThemeMode::System => "System",
    }
}

/// `gpu_backend` values and labels (Settings › Display › Graphics).
const GPU_BACKENDS: [(&str, &str); 5] =
    [("auto", "Automatic"), ("gl", "OpenGL"), ("dx12", "DirectX 12"), ("vulkan", "Vulkan"), ("cpu", "Software (CPU)")];

fn display(ui: &mut egui::Ui, s: &mut Settings, icons: &Icons) {
    group(ui, icons, "Appearance", |ui| {
        ui.horizontal(|ui| {
            let l = ui.label("Skin");
            egui::ComboBox::from_id_salt("settings-skin")
                .selected_text(skin_label(s.theme))
                .show_ui(ui, |ui| {
                    for t in [ThemeMode::Light, ThemeMode::Dark, ThemeMode::System] {
                        ui.selectable_value(&mut s.theme, t, skin_label(t));
                    }
                })
                .response
                .labelled_by(l.id);
        });
    });
    group(ui, icons, "Toolbar and tabs", |ui| {
        ui.checkbox(&mut s.toolbar_captions, "Toolbar captions");
        ui.checkbox(&mut s.show_tab_numbers, "Show tab numbers");
        ui.checkbox(&mut s.show_close_buttons, "Show close buttons");
    });
    group(ui, icons, "Graphics", |ui| {
        ui.horizontal(|ui| {
            let l = ui.label("Rendering backend");
            let label = |v: &str| GPU_BACKENDS.iter().find(|(k, _)| *k == v).map_or("Automatic", |(_, l)| *l);
            egui::ComboBox::from_id_salt("settings-gpu-backend")
                .selected_text(label(&s.gpu_backend))
                .show_ui(ui, |ui| {
                    for (value, text) in GPU_BACKENDS {
                        if cfg!(windows) || value != "dx12" {
                            ui.selectable_value(&mut s.gpu_backend, value.to_owned(), text);
                        }
                    }
                })
                .response
                .labelled_by(l.id);
        });
        ui.label(
            egui::RichText::new("Applies at the next start. After a driver crash rustXterm picks a safer one.").small(),
        );
    });
    group(ui, icons, "Interface size", |ui| {
        ui.horizontal(|ui| {
            let l = ui.label("Interface size");
            egui::ComboBox::from_id_salt("settings-ui-scale")
                .selected_text(s.ui_scale.label())
                .show_ui(ui, |ui| {
                    for u in UiScale::ALL {
                        ui.selectable_value(&mut s.ui_scale, u, u.label());
                    }
                })
                .response
                .labelled_by(l.id);
        });
    });
}

fn shortcuts(ui: &mut egui::Ui, s: &mut Settings, icons: &Icons, tokens: &Tokens) {
    group(ui, icons, "Keyboard shortcuts", |ui| {
        ui.horizontal(|ui| {
            muted(ui, tokens, "Use Ctrl+Shift+key, Ctrl+Tab or F11/F12; terminal keys such as Ctrl+C are reserved.");
            if ui.button("Reset defaults").clicked() {
                s.shortcuts = default_shortcuts();
            }
        });
        egui::Grid::new("settings-shortcuts").num_columns(3).min_col_width(150.0).spacing([12.0, 4.0]).show(ui, |ui| {
            for c in CommandId::ALL {
                let l = ui.label(command_label(c));
                let text = s.shortcuts.entry(c.as_str().to_owned()).or_default();
                ui.add(egui::TextEdit::singleline(text).desired_width(160.0)).labelled_by(l.id);
                let valid = text.trim().is_empty() || parse_shortcut(text).is_some_and(|sc| shortcut_allowed(&sc));
                if valid {
                    ui.label("");
                } else {
                    ui.colored_label(color(tokens.danger), "Not allowed");
                }
                ui.end_row();
            }
        });
    });
}

fn security(ui: &mut egui::Ui, st: StoreState, icons: &Icons, tokens: &Tokens, out: &mut Vec<Action>) {
    group(ui, icons, "Master password", |ui| {
        if !st.unlocked {
            ui.horizontal(|ui| {
                ui.label("The session database is locked.");
                if ui.button("Unlock…").clicked() {
                    out.push(Action::ShowUnlock);
                }
            });
        }
        ui.add_enabled_ui(st.unlocked, |ui| {
            let mut keyring = st.keyring;
            if ui.checkbox(&mut keyring, "Unlock with system keyring").changed() {
                out.push(Action::SetKeyring(keyring));
            }
            muted(ui, tokens, "The master password always unlocks the database, also without the keyring.");
            if ui.button("Change master password…").clicked() {
                out.push(Action::ShowChangePassword);
            }
        });
    });
    group(ui, icons, "Import and export", |ui| {
        ui.add_enabled_ui(st.unlocked, |ui| {
            ui.horizontal(|ui| {
                for (label, action) in [
                    ("Import MobaXterm.ini…", Action::ImportIni),
                    ("Import rustXterm TOML…", Action::ImportToml),
                    ("Export rustXterm TOML…", Action::ExportToml),
                ] {
                    if ui.button(label).clicked() {
                        out.push(action);
                    }
                }
            });
        });
    });
}

/// Cursor shape picker.
pub(crate) fn cursor_combo(ui: &mut egui::Ui, id: &str, shape: &mut CursorShape) {
    egui::ComboBox::from_id_salt(id).selected_text(cursor_label(*shape)).show_ui(ui, |ui| {
        for c in [CursorShape::Block, CursorShape::Underline, CursorShape::Beam] {
            ui.selectable_value(shape, c, cursor_label(c));
        }
    });
}

fn cursor_label(c: CursorShape) -> &'static str {
    match c {
        CursorShape::Block => "Block",
        CursorShape::Underline => "Underline",
        CursorShape::Beam => "Beam",
    }
}

/// Terminal scheme picker; `None` follows the UI theme.
pub(crate) fn scheme_combo(ui: &mut egui::Ui, id: &str, scheme: &mut Option<String>) {
    if scheme.as_deref() == Some(crate::theme::FOLLOW_UI) {
        *scheme = None;
    }
    let label = scheme.clone().unwrap_or_else(|| "Follow UI theme".into());
    egui::ComboBox::from_id_salt(id).selected_text(label).height(420.0).show_ui(ui, |ui| {
        ui.selectable_value(scheme, None, "Follow UI theme");
        for s in crate::theme::catalog() {
            ui.horizontal(|ui| {
                scheme_swatch(ui, s);
                ui.selectable_value(scheme, Some(s.name.clone()), &s.name);
            });
        }
    });
}

/// Small preview: background with the foreground and the 8 normal ANSI colors.
fn scheme_swatch(ui: &mut egui::Ui, s: &TerminalScheme) {
    let c = color;
    let cell = 7.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(cell * 9.0 + 4.0, cell + 4.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, c(s.background));
    painter.rect_stroke(rect, 0.0, ui.visuals().widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
    for (i, color) in std::iter::once(s.foreground).chain(s.ansi[..8].iter().copied()).enumerate() {
        let min = rect.min + egui::vec2(2.0 + cell * i as f32, 2.0);
        painter.rect_filled(egui::Rect::from_min_size(min, egui::vec2(cell - 1.0, cell)), 0.0, c(color));
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use egui_kittest::kittest::Queryable;

    use super::*;

    #[test]
    fn scheme_combo_lists_catalog_and_maps_follow_ui() {
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(400.0, 600.0)).build_ui_state(
            |ui, s: &mut Option<String>| scheme_combo(ui, "t", s),
            Some(crate::theme::FOLLOW_UI.to_owned()),
        );
        h.run();
        assert_eq!(h.state(), &None, "follow-ui shows as Follow UI theme");
        h.get_by_role(egui::accesskit::Role::ComboBox).click();
        h.run();
        h.get_by_label("Dracula").click();
        h.run();
        assert_eq!(h.state().as_deref(), Some("Dracula"));
    }

    #[test]
    fn defaults_are_valid_and_never_plain_ctrl_letter() {
        let s = Settings::default();
        assert_eq!(s.bindings().len(), s.shortcuts.len());
        for (_, sc) in s.bindings() {
            assert!(!(sc.modifiers.ctrl && !sc.modifiers.shift && sc.logical_key != Key::Tab), "{sc:?}");
        }
    }

    #[test]
    fn terminal_keys_are_refused() {
        for bad in ["Ctrl+C", "Ctrl+D", "Alt+X", "Shift+A", "A", "Ctrl+Alt+Shift+T", "F1"] {
            let sc = parse_shortcut(bad).unwrap();
            assert!(!shortcut_allowed(&sc), "{bad}");
        }
        assert!(shortcut_allowed(&parse_shortcut("ctrl+shift+t").unwrap()));
    }

    #[test]
    fn parse_and_format_round_trip() {
        for text in ["Ctrl+Shift+N", "Ctrl+Tab", "F11", "Ctrl+Shift+Comma"] {
            assert_eq!(format_shortcut(&parse_shortcut(text).unwrap()), text);
        }
        assert!(parse_shortcut("Ctrl+Shift+").is_none());
        assert!(parse_shortcut("Ctrl+A+B").is_none());
    }

    #[test]
    fn store_settings_round_trip_keeps_only_changed_shortcuts() {
        let base = StoreSettings { theme: ThemeMode::Dark, log_level: "debug".into(), ..StoreSettings::default() };
        let mut ui = Settings::from_store(&base);
        assert_eq!(ui.theme, ThemeMode::Dark);
        assert_eq!(ui.shortcuts, default_shortcuts());
        ui.shortcuts.insert(CommandId::FIND.as_str().into(), String::new());
        let out = ui.to_store(&base);
        assert_eq!(out.log_level, "debug");
        assert_eq!(out.shortcuts.len(), 1);
        assert_eq!(Settings::from_store(&out), ui);
    }

    #[test]
    fn settings_serde_round_trip() {
        let mut s = Settings { sidebar_width: 312.0, theme: ThemeMode::Dark, ..Settings::default() };
        s.shortcuts.insert("tab.close".into(), String::new());
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    /// Settings window in a harness: (open, settings, emitted actions).
    type WinState = (bool, Settings, Vec<Action>);

    pub(crate) fn harness(tab: SettingsTab) -> egui_kittest::Harness<'static, WinState> {
        let mut icons = None;
        let mut h = egui_kittest::Harness::builder()
            .with_theme(egui::Theme::Light)
            .with_size(egui::vec2(1000.0, 700.0))
            .build_ui_state(
                move |ui, st: &mut WinState| {
                    let icons = icons.get_or_insert_with(|| Icons::load(ui.ctx(), None));
                    let (open, s, out) = st;
                    if *open {
                        let mut ws = None;
                        let agent = AgentSettings { workspace: &mut ws, default_workspace: "/tmp/agent".into() };
                        let state = crate::diagnostics::DiagState {
                            dir: Some("/home/user/rustxterm-logs".into()),
                            ..Default::default()
                        };
                        let mut stream = crate::diagnostics::StreamUi::default();
                        let diag = DiagPage { state: &state, stream: &mut stream };
                        window(ui.ctx(), open, s, None, agent, diag, icons, &Tokens::light(), out);
                    }
                },
                // Opened after the theme is installed (the builder runs the first frame).
                (false, Settings::default(), Vec::new()),
            );
        install_theme(&h.ctx);
        h.ctx.data_mut(|d| d.insert_temp(tab_id(), tab));
        h.state_mut().0 = true;
        h
    }

    /// Fonts and token styles as the app installs them.
    pub(crate) fn install_theme(ctx: &egui::Context) {
        ctx.set_fonts(crate::theme::fonts());
        ctx.set_style_of(egui::Theme::Light, crate::theme::style(&Tokens::light()));
        ctx.set_style_of(egui::Theme::Dark, crate::theme::style(&Tokens::dark()));
    }

    #[test]
    fn skin_system_applies_and_persists() {
        let mut h = harness(SettingsTab::Display);
        h.state_mut().1.theme = ThemeMode::Dark;
        h.run();
        h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Skin").click();
        h.run();
        h.get_by_label("System").click();
        h.run();
        assert_eq!(h.state().1.theme, ThemeMode::System);
        assert!(h.state().0, "live apply keeps the window open");
        // Persisted through config.toml and read back after a restart.
        let dir = std::env::temp_dir().join(format!("rxt-settings-skin-{}", std::process::id()));
        let path = dir.join("config.toml");
        rxt_store::settings::save(&path, &h.state().1.to_store(&StoreSettings::default())).unwrap();
        let loaded = rxt_store::settings::load(&path, None, &[], &Default::default());
        assert_eq!(Settings::from_store(&loaded.settings).theme, ThemeMode::System);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn escape_closes_and_keeps_changes() {
        let mut h = harness(SettingsTab::Terminal);
        h.run();
        let before = h.state().1.copy_on_select;
        h.get_by_label("Copy on select").click();
        h.run();
        h.key_press(egui::Key::Escape);
        h.run();
        assert!(!h.state().0);
        assert_eq!(h.state().1.copy_on_select, !before);
    }

    #[test]
    fn ok_closes_and_last_tab_is_remembered() {
        let mut h = harness(SettingsTab::General);
        h.run();
        h.get_by_label("Display").click();
        h.run();
        assert!(h.query_by_label("Show tab numbers").is_some());
        let size = h.get_by_role_and_label(egui::accesskit::Role::Window, "Settings").rect().size();
        assert!((size.x - SETTINGS_SIZE.x).abs() <= 8.0 && (size.y - SETTINGS_SIZE.y).abs() <= 8.0, "{size:?}");
        h.get_by_label("OK").click();
        h.run();
        assert!(!h.state().0);
        h.state_mut().0 = true;
        h.run();
        assert!(h.query_by_label("Show tab numbers").is_some());
    }
}
