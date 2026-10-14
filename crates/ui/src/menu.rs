//! Menu bar (desktop-shell "Menu bar", design R4): every item runs a [`CommandId`].
//!
//! No Alt mnemonics and no F10 handling: those keys belong to a focused terminal or remote view.
//! The keyboard path is the `app.focus_menu` shortcut (default Ctrl+Shift+F10).

use egui::{Button, Popup, WidgetInfo, WidgetType};
use rxt_core::{CommandId, ThemeMode};

use crate::app::{Action, App};

/// (menu title, items); `None` is a separator.
type Menu = (&'static str, &'static [Option<(CommandId, &'static str)>]);

const MENUS: [Menu; 6] = [
    (
        "Terminal",
        &[
            Some((CommandId::FIND, "Find")),
            Some((CommandId::COPY, "Copy")),
            Some((CommandId::PASTE, "Paste")),
            Some((CommandId::SAVE_TEXT, "Save terminal text")),
            None,
            Some((CommandId::CLOSE_TAB, "Close tab")),
            None,
            Some((CommandId::EXIT, "Exit")),
        ],
    ),
    (
        "Sessions",
        &[
            Some((CommandId::NEW_SESSION, "New session")),
            Some((CommandId::QUICK_CONNECT, "Quick connect")),
            Some((CommandId::DUPLICATE_TAB, "Duplicate tab")),
            Some((CommandId::RECONNECT, "Reconnect")),
            Some((CommandId::FIND_SESSION, "Find session…")),
            None,
            Some((CommandId::IMPORT_SESSIONS, "Import sessions")),
            Some((CommandId::EXPORT_SESSIONS, "Export sessions")),
        ],
    ),
    (
        "View",
        &[
            Some((CommandId::TOGGLE_SIDEBAR, "Show sidebar")),
            Some((CommandId::TOGGLE_FULLSCREEN, "Fullscreen")),
            None,
            Some((CommandId::NEXT_TAB, "Next tab")),
            Some((CommandId::PREV_TAB, "Previous tab")),
            None,
            Some((CommandId::THEME_LIGHT, "Light")),
            Some((CommandId::THEME_DARK, "Dark")),
            Some((CommandId::THEME_SYSTEM, "System")),
        ],
    ),
    ("Tools", &[Some((CommandId::TUNNELS, "Tunneling")), Some((CommandId::MULTIEXEC, "MultiExec"))]),
    (
        "Settings",
        &[
            Some((CommandId::SETTINGS, "Configuration")),
            Some((CommandId::CHANGE_PASSWORD, "Change master password")),
            None,
            Some((CommandId::IMPORT_SESSIONS, "Import sessions")),
            Some((CommandId::EXPORT_SESSIONS, "Export sessions")),
        ],
    ),
    ("Help", &[Some((CommandId::HELP, "Help")), Some((CommandId::ABOUT, "About"))]),
];

impl App {
    /// Whether `cmd` can run now; unavailable menu items are disabled, not hidden.
    pub(crate) fn available(&self, cmd: CommandId) -> bool {
        let tab = self.active_tab();
        match cmd {
            CommandId::FIND | CommandId::COPY | CommandId::PASTE | CommandId::SAVE_TEXT => {
                tab.is_some_and(|t| t.term.is_some())
            }
            CommandId::CLOSE_TAB | CommandId::DUPLICATE_TAB | CommandId::RENAME_TAB | CommandId::CLOSE_OTHERS => {
                tab.is_some()
            }
            CommandId::RECONNECT => tab.is_some_and(|t| !t.is_live()),
            CommandId::NEXT_TAB | CommandId::PREV_TAB => !self.tabs.is_empty(),
            CommandId::IMPORT_SESSIONS | CommandId::EXPORT_SESSIONS | CommandId::CHANGE_PASSWORD => {
                self.vault.is_some()
            }
            _ => true,
        }
    }

    /// Checked state of toggle and radio items.
    fn checked(&self, cmd: CommandId) -> Option<bool> {
        let theme = self.settings.theme;
        match cmd {
            CommandId::TOGGLE_SIDEBAR => Some(self.settings.sidebar_open),
            CommandId::TOGGLE_FULLSCREEN => Some(self.fullscreen),
            CommandId::MULTIEXEC => Some(self.multiexec),
            CommandId::THEME_LIGHT => Some(theme == ThemeMode::Light),
            CommandId::THEME_DARK => Some(theme == ThemeMode::Dark),
            CommandId::THEME_SYSTEM => Some(theme == ThemeMode::System),
            _ => None,
        }
    }

    /// Draw the menu bar; `open_terminal` opens the Terminal menu (focus-menu shortcut).
    pub(crate) fn menu_bar(&self, ui: &mut egui::Ui, open_terminal: bool, out: &mut Vec<Action>) {
        ui.spacing_mut().interact_size.y = rxt_core::Metrics::DEFAULT.menu_height;
        egui::MenuBar::new().ui(ui, |ui| {
            for (title, items) in MENUS {
                let (resp, _) = egui::containers::menu::MenuButton::new(title).ui(ui, |ui| {
                    ui.set_min_width(220.0);
                    for item in items {
                        match item {
                            None => {
                                ui.separator();
                            }
                            Some((cmd, label)) => self.item(ui, *cmd, label, out),
                        }
                    }
                });
                if open_terminal && title == "Terminal" {
                    Popup::open_id(ui.ctx(), Popup::default_response_id(&resp));
                }
            }
        });
    }

    fn item(&self, ui: &mut egui::Ui, cmd: CommandId, label: &str, out: &mut Vec<Action>) {
        let mut b = match self.checked(cmd) {
            Some(on) if cmd.as_str().starts_with("view.theme_") => Button::new(format!("{} {label}", radio(on))),
            Some(on) => Button::new(format!("{} {label}", check(on))),
            None => Button::new(format!("   {label}")),
        };
        if let Some(sc) = self.settings.shortcut_text(cmd) {
            b = b.shortcut_text(sc);
        }
        let enabled = self.available(cmd);
        let r = ui.add_enabled(enabled, b);
        // Accessible name without the check/radio glyph.
        r.widget_info(|| match self.checked(cmd) {
            Some(on) if cmd.as_str().starts_with("view.theme_") => {
                WidgetInfo::selected(WidgetType::RadioButton, enabled, on, label)
            }
            Some(on) => WidgetInfo::selected(WidgetType::Checkbox, enabled, on, label),
            None => WidgetInfo::labeled(WidgetType::Button, enabled, label),
        });
        if r.clicked() {
            out.push(Action::Run(cmd));
        }
    }
}

/// Radio mark: a bullet on the chosen item only, like native menus ("●" is not in Noto Sans).
fn radio(on: bool) -> &'static str {
    if on { "•" } else { "   " }
}

fn check(on: bool) -> &'static str {
    if on { "✔" } else { "   " }
}
