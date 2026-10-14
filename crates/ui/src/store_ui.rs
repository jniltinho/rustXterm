//! Encrypted-store dialogs: first-run master password, unlock, change password, MobaXterm.ini
//! import preview and export confirmation; plus the locked session-tree placeholder.

use std::path::PathBuf;

use egui::{Id, RichText, Ui};
use rxt_core::{Secret, Tokens};
use rxt_store::moba_ini::ImportReport;

use crate::app::Action;
use crate::dialogs::{self, field};
use crate::icons::Icons;
use crate::theme::color;

/// Shortest accepted master password.
pub(crate) const MIN_MASTER: usize = 8;

/// Open store dialog.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StoreDialog {
    /// First start: choose the master password.
    Create {
        password: String,
        confirm: String,
        keyring: bool,
        error: Option<String>,
    },
    Unlock {
        password: String,
        error: Option<String>,
    },
    ChangePassword {
        password: String,
        confirm: String,
        error: Option<String>,
    },
    Import {
        path: PathBuf,
        report: ImportReport,
    },
    Export {
        path: PathBuf,
        sessions: usize,
        folders: usize,
        tunnels: usize,
    },
}

impl StoreDialog {
    pub(crate) fn create() -> Self {
        Self::Create { password: String::new(), confirm: String::new(), keyring: false, error: None }
    }
    pub(crate) fn unlock() -> Self {
        Self::Unlock { password: String::new(), error: None }
    }
    pub(crate) fn change_password() -> Self {
        Self::ChangePassword { password: String::new(), confirm: String::new(), error: None }
    }

    /// Show a worker error in the dialog.
    pub(crate) fn set_error(&mut self, e: String) {
        match self {
            Self::Create { error, .. } | Self::Unlock { error, .. } | Self::ChangePassword { error, .. } => {
                *error = Some(e);
            }
            Self::Import { .. } | Self::Export { .. } => {}
        }
    }

    /// Overwrite typed passwords before the dialog is dropped.
    pub(crate) fn wipe(&mut self) {
        let fields: Vec<&mut String> = match self {
            Self::Create { password, confirm, .. } | Self::ChangePassword { password, confirm, .. } => {
                vec![password, confirm]
            }
            Self::Unlock { password, .. } => vec![password],
            Self::Import { .. } | Self::Export { .. } => Vec::new(),
        };
        for s in fields {
            let len = s.len();
            s.clear();
            s.extend(std::iter::repeat_n('\0', len));
            s.clear();
        }
    }
}

/// Rough strength hint: length plus character classes.
pub(crate) fn strength(p: &str) -> &'static str {
    let classes =
        [char::is_lowercase, char::is_uppercase, |c: char| c.is_ascii_digit(), |c: char| !c.is_alphanumeric()]
            .iter()
            .filter(|f| p.chars().any(f))
            .count();
    match p.chars().count() {
        n if n < MIN_MASTER => "Too short",
        n if n >= 16 || (n >= 12 && classes >= 3) => "Strong",
        _ if classes >= 2 => "Fair",
        _ => "Weak",
    }
}

/// New password + confirmation fields; the problem that keeps OK disabled, if any.
fn new_password(ui: &mut egui::Ui, password: &mut String, confirm: &mut String, tokens: &Tokens) -> Option<String> {
    field(ui, "Master password", password, true);
    if !password.is_empty() {
        ui.label(RichText::new(format!("Strength: {}", strength(password))).small().color(color(tokens.text_muted)));
    }
    field(ui, "Confirm master password", confirm, true);
    if password.chars().count() < MIN_MASTER {
        Some(format!("Use at least {MIN_MASTER} characters."))
    } else if password != confirm {
        Some("The passwords do not match.".to_owned())
    } else {
        None
    }
}

fn error_line(ui: &mut egui::Ui, error: &Option<String>, tokens: &Tokens) {
    if let Some(e) = error {
        ui.colored_label(color(tokens.danger), e.as_str());
    }
}

/// Draw the dialog. `busy` shows a worker in progress (inputs stay, OK is disabled).
/// Returns false when the dialog closed.
pub(crate) fn dialog_ui(
    ctx: &egui::Context,
    d: &mut StoreDialog,
    busy: bool,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) -> bool {
    let title = match d {
        StoreDialog::Create { .. } => "Create master password",
        StoreDialog::Unlock { .. } => "Unlock sessions",
        StoreDialog::ChangePassword { .. } => "Change master password",
        StoreDialog::Import { .. } => "Import MobaXterm.ini",
        StoreDialog::Export { .. } => "Export sessions",
    };
    dialogs::dialog(ctx, Id::new("store-dialog"), title, 460.0, tokens, |ui, icons| {
        let keep = body(ui, icons, d, busy, tokens, out);
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Working...");
            });
        }
        keep
    })
}

/// OK-style button `ok` (enabled when `enabled`) and `cancel`; the clicked index.
fn buttons(ui: &mut Ui, icons: &Icons, ok: &str, cancel: &str, enabled: bool) -> Option<usize> {
    dialogs::buttons(ui, icons, &[ok, cancel], 0, enabled)
}

fn body(ui: &mut Ui, icons: &Icons, d: &mut StoreDialog, busy: bool, tokens: &Tokens, out: &mut Vec<Action>) -> bool {
    match d {
        StoreDialog::Create { password, confirm, keyring, error } => {
            ui.label("Saved sessions, credentials and trust records are stored in an encrypted database.");
            ui.colored_label(
                color(tokens.warning),
                "If you forget the master password, the saved data cannot be recovered.",
            );
            let problem = new_password(ui, password, confirm, tokens);
            ui.checkbox(keyring, "Unlock with system keyring");
            error_line(ui, error, tokens);
            let ok = problem.is_none() && !busy;
            if !password.is_empty()
                && !confirm.is_empty()
                && let Some(p) = &problem
            {
                ui.colored_label(color(tokens.danger), p.as_str());
            }
            match buttons(ui, icons, "Create", "Later", ok) {
                Some(0) => {
                    out.push(Action::CreateVault { master: Secret(password.clone()), keyring: *keyring });
                    true
                }
                Some(_) => false,
                None => true,
            }
        }
        StoreDialog::Unlock { password, error } => {
            ui.label("Enter the master password to open saved sessions.");
            field(ui, "Master password", password, true);
            error_line(ui, error, tokens);
            let ok = !password.is_empty() && !busy;
            match buttons(ui, icons, "Unlock", "Cancel", ok) {
                Some(0) => {
                    out.push(Action::Unlock(Secret(password.clone())));
                    true
                }
                Some(_) => false,
                None => true,
            }
        }
        StoreDialog::ChangePassword { password, confirm, error } => {
            ui.colored_label(
                color(tokens.warning),
                "If you forget the new master password, the saved data cannot be recovered.",
            );
            let problem = new_password(ui, password, confirm, tokens);
            error_line(ui, error, tokens);
            if !password.is_empty()
                && !confirm.is_empty()
                && let Some(p) = &problem
            {
                ui.colored_label(color(tokens.danger), p.as_str());
            }
            let ok = problem.is_none() && !busy;
            match buttons(ui, icons, "Change", "Cancel", ok) {
                Some(0) => {
                    out.push(Action::ChangePassword(Secret(password.clone())));
                    true
                }
                Some(_) => false,
                None => true,
            }
        }
        StoreDialog::Import { path, report } => {
            ui.label(RichText::new(path.display().to_string()).small().color(color(tokens.text_muted)));
            ui.label(format!(
                "{} session(s) in {} folder(s) can be imported; {} entr{} skipped. Passwords are never read.",
                report.sessions.len(),
                report.folders.len(),
                report.skipped.len(),
                if report.skipped.len() == 1 { "y is" } else { "ies are" },
            ));
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| preview(ui, report, tokens));
            match buttons(ui, icons, "Import", "Cancel", !report.sessions.is_empty()) {
                Some(0) => {
                    out.push(Action::ConfirmImport);
                    true
                }
                Some(_) => false,
                None => true,
            }
        }
        StoreDialog::Export { path, sessions, folders, tunnels } => {
            ui.label(format!(
                "Write {sessions} session(s), {folders} folder(s) and {tunnels} tunnel(s) to {}?",
                path.display()
            ));
            ui.label("Passwords, passphrases and trust records are not exported.");
            match buttons(ui, icons, "Export", "Cancel", true) {
                Some(0) => {
                    out.push(Action::ConfirmExport);
                    true
                }
                Some(_) => false,
                None => true,
            }
        }
    }
}

/// Import preview: sessions grouped by folder, then skipped entries with line and reason.
fn preview(ui: &mut egui::Ui, report: &ImportReport, tokens: &Tokens) {
    let roots = std::iter::once(String::new()).chain(report.folders.iter().cloned());
    egui::Grid::new("import-preview").num_columns(3).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
        for folder in roots {
            let items: Vec<_> = report.sessions.iter().filter(|(f, _)| *f == folder).map(|(_, s)| s).collect();
            if items.is_empty() {
                continue;
            }
            let title = if folder.is_empty() { "User sessions".to_owned() } else { folder.clone() };
            ui.label(RichText::new(title).strong());
            ui.label("");
            ui.label("");
            ui.end_row();
            for s in items {
                ui.label(format!("    {}", s.name));
                ui.label(s.protocol.scheme().to_uppercase());
                if s.protocol == rxt_core::Protocol::Serial {
                    ui.label(format!("{} {}", s.serial.port, s.serial.line));
                } else {
                    let user = s.user.as_deref().map(|u| format!("{u}@")).unwrap_or_default();
                    ui.label(format!("{user}{}:{}", s.host, s.port));
                }
                ui.end_row();
            }
        }
    });
    for (title, rows) in [("Warnings", &report.warnings), ("Skipped", &report.skipped)] {
        if rows.is_empty() {
            continue;
        }
        ui.add_space(6.0);
        ui.label(RichText::new(title).strong());
        egui::Grid::new(("import-rows", title)).num_columns(3).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
            for s in rows {
                ui.label(format!("line {}", s.line));
                ui.label(s.name.as_str());
                ui.colored_label(color(tokens.warning), s.reason.as_str());
                ui.end_row();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strength_hint() {
        assert_eq!(strength("abc"), "Too short");
        assert_eq!(strength("abcdefgh"), "Weak");
        assert_eq!(strength("abcdefg1"), "Fair");
        assert_eq!(strength("Abcdefgh1234"), "Strong");
        assert_eq!(strength("correct horse battery"), "Strong");
    }
}
