//! Modal dialogs: engine prompts (host keys, secrets, keyboard-interactive, overwrite)
//! and local confirmations (close, delete, rename, paste, SFTP operations).

use std::sync::{Arc, Mutex, PoisonError};

use egui::{Frame, Id, Margin, RichText, Stroke, Ui, vec2};
use rxt_core::fileops::{join, parent};
use rxt_core::{
    FolderId, HostKeyDecision, Metrics, Overwrite, PromptAnswer, PromptId, PromptKind, Secret, SessionId, TabId, Tokens,
};

use crate::app::Action;
use crate::icons::{self, Icons};
use crate::sftp_panel::{SftpAction, parse_mode};
use crate::theme::color;
use crate::widgets;

const M: Metrics = Metrics::DEFAULT;

/// Temp-data key: the dialog shown this frame still has to place its initial focus.
const FOCUS_PENDING: &str = "rxt-dialog-focus-pending";

/// Engine prompt waiting for the user.
#[derive(Debug)]
pub(crate) struct Prompt {
    pub id: PromptId,
    pub tab: Option<TabId>,
    pub kind: PromptKind,
    /// Text typed per field (secrets, keyboard-interactive responses, rename target).
    pub inputs: Vec<String>,
    /// "Save password" ticked (passwords and key passphrases).
    pub save: bool,
}

impl Prompt {
    pub(crate) fn new(id: PromptId, tab: Option<TabId>, kind: PromptKind) -> Self {
        let fields = match &kind {
            PromptKind::KeyboardInteractive { prompts, .. } => prompts.len(),
            PromptKind::Overwrite { path } => {
                return Self { id, tab, inputs: vec![format!("{path}.1")], kind, save: false };
            }
            _ => 1,
        };
        Self { id, tab, kind, inputs: vec![String::new(); fields], save: false }
    }

    /// Overwrite secrets in memory before dropping them.
    fn wipe(&mut self) {
        for s in &mut self.inputs {
            // Overwrite the same allocation with NULs (keeps UTF-8 valid), then clear.
            let len = s.len();
            s.clear();
            s.extend(std::iter::repeat_n('\0', len));
            s.clear();
        }
    }
}

/// Local modal dialog.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Modal {
    ConfirmClose(TabId),
    ConfirmCloseOthers(TabId),
    ConfirmExit,
    RenameTab(TabId, String),
    /// Delete confirmation naming the saved session.
    DeleteSavedSession {
        id: SessionId,
        name: String,
    },
    /// Create (`id` None) or edit a folder: name and icon.
    FolderName {
        id: Option<FolderId>,
        parent: Option<FolderId>,
        name: String,
        icon: Option<String>,
    },
    DeleteFolder(FolderId),
    Paste {
        tab: TabId,
        text: String,
    },
    /// New folder in `dir`; `local` targets the local pane of a file tab.
    Mkdir {
        local: bool,
        dir: String,
        name: String,
    },
    /// New empty file in `dir`.
    NewFile {
        local: bool,
        dir: String,
        name: String,
    },
    RenameRemote {
        local: bool,
        from: String,
        to: String,
    },
    DeleteRemote {
        local: bool,
        path: String,
    },
    Chmod {
        path: String,
        mode: String,
        error: Option<String>,
    },
    HostKeyChanged {
        host: String,
        old: String,
        new: String,
    },
    /// Informational message (e.g. settings file problems at startup).
    Notice(String),
    About,
}

impl Modal {
    pub(crate) fn folder(id: Option<FolderId>, parent: Option<FolderId>, name: String) -> Self {
        Self::FolderName { id, parent, name, icon: None }
    }

    /// Edit dialog for an existing folder.
    pub(crate) fn edit_folder(f: &rxt_core::Folder) -> Self {
        Self::FolderName { id: Some(f.id), parent: f.parent, name: f.name.clone(), icon: f.icon.clone() }
    }
}

/// Labeled single-line field; `secret` masks input. Returns true when Enter was pressed.
/// The first field of a newly opened dialog takes the keyboard focus.
pub(crate) fn field(ui: &mut egui::Ui, label: &str, text: &mut String, secret: bool) -> bool {
    let l = ui.label(label);
    let r = ui.add(egui::TextEdit::singleline(text).password(secret).desired_width(f32::INFINITY)).labelled_by(l.id);
    if take_focus_pending(ui.ctx()) {
        r.request_focus();
    }
    r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

fn take_focus_pending(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(Id::new(FOCUS_PENDING))).unwrap_or(false)
}

/// Icons for dialogs, which are drawn from a context without the app's [`Icons`] (one set per
/// context, decoded once).
fn dialog_icons(ctx: &egui::Context) -> Arc<Mutex<Icons>> {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_insert_with(Id::new("rxt-dialog-icons"), || {
            Arc::new(Mutex::new(Icons::load(ctx, icons::override_dir_from_env().as_deref())))
        })
        .clone()
    })
}

/// Dialog convention (design R9): modal and centered, square corners, 1 px border, a 24 px title
/// row on panel, then `add`'s body; `add` returns whether the dialog stays open. Focus starts on
/// the first text field (or the default button) and returns to the opener when it closes.
pub(crate) fn dialog(
    ctx: &egui::Context,
    id: Id,
    title: &str,
    width: f32,
    tokens: &Tokens,
    add: impl FnOnce(&mut Ui, &Icons) -> bool,
) -> bool {
    let opener_key = id.with("opener");
    if ctx.data(|d| d.get_temp::<Option<Id>>(opener_key)).is_none() {
        let opener = ctx.memory(|m| m.focused());
        ctx.data_mut(|d| {
            d.insert_temp(opener_key, opener);
            d.insert_temp(Id::new(FOCUS_PENDING), true);
        });
    }
    let icons = dialog_icons(ctx);
    let icons = icons.lock().unwrap_or_else(PoisonError::into_inner);
    let frame = Frame::new()
        .fill(color(tokens.panel))
        .stroke(Stroke::new(M.stroke, color(tokens.border)))
        .inner_margin(Margin::ZERO);
    let mut keep = true;
    // No dimmed backdrop: the window behind dialogs stays as is.
    egui::Modal::new(id).frame(frame).backdrop_color(egui::Color32::TRANSPARENT).show(ctx, |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 0.0;
        widgets::title_row(ui, title);
        Frame::new().inner_margin(Margin::same(M.space[4] as i8)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = M.space[1];
            keep = add(ui, &icons);
        });
    });
    ctx.data_mut(|d| d.remove_temp::<bool>(Id::new(FOCUS_PENDING)));
    if !keep && let Some(opener) = ctx.data_mut(|d| d.remove_temp::<Option<Id>>(opener_key)).flatten() {
        restore_focus(ctx, opener);
    }
    keep
}

/// Give the focus back to `opener` once the closed dialog no longer blocks the layers below
/// (egui keeps the last modal layer for one more pass and drops focus requested behind it).
fn restore_focus(ctx: &egui::Context, opener: Id) {
    let key = Id::new("rxt-dialog-restore-focus");
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(key, (opener, pass)));
    ctx.request_repaint();
    if ctx.data(|d| d.get_temp::<bool>(key.with("hook"))).is_some() {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(key.with("hook"), true));
    ctx.on_end_pass(
        "rxt-dialog-restore-focus",
        Arc::new(move |ui: &mut Ui| {
            let ctx = ui.ctx();
            let Some((id, closed)) = ctx.data(|d| d.get_temp::<(Id, u64)>(key)) else { return };
            let pass = ctx.cumulative_pass_nr();
            if pass > closed {
                ctx.memory_mut(|m| m.request_focus(id));
                ctx.data_mut(|d| d.remove::<(Id, u64)>(key));
            } else {
                ctx.request_repaint();
            }
        }),
    );
}

/// Centered bottom row of dialog buttons, at least 80×30 px and 24 px apart: the first with the
/// `ok` icon, the last (Cancel) with the `cancel` icon, a single button is OK. `default` gets the
/// initial focus and answers Enter (destructive confirmations pass the Cancel index); Escape
/// answers the last button. `first_enabled` disables the first button (invalid input).
pub(crate) fn buttons(
    ui: &mut egui::Ui,
    icons: &Icons,
    labels: &[&str],
    default: usize,
    first_enabled: bool,
) -> Option<usize> {
    let keys = widgets::dialog_keys(ui.ctx());
    let last = labels.len().saturating_sub(1);
    let pad = ui.spacing().button_padding.x;
    let font = egui::TextStyle::Button.resolve(ui.style());
    let widths: Vec<f32> = labels
        .iter()
        .map(|l| {
            let text = ui.painter().layout_no_wrap((*l).to_owned(), font.clone(), egui::Color32::WHITE).size().x;
            (text + M.icon_small + M.space[1] + 2.0 * pad).max(M.dialog_button.0).ceil()
        })
        .collect();
    let gap = M.space[6];
    let total = widths.iter().sum::<f32>() + gap * last as f32;
    let mut hit = None;
    ui.add_space(M.space[3]);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
        for (i, (label, w)) in labels.iter().zip(&widths).enumerate() {
            let icon = match i {
                0 => Some("ok"),
                i if i == last => Some("cancel"),
                _ => None,
            };
            let image = icon.and_then(|n| icons.image(n, M.icon_small));
            let button =
                egui::Button::opt_image_and_text(image, Some((*label).into())).min_size(vec2(*w, M.dialog_button.1));
            let enabled = i != 0 || first_enabled;
            let r = ui.add_enabled(enabled, button);
            if i == default && take_focus_pending(ui.ctx()) {
                r.request_focus();
            }
            if r.clicked() {
                hit = Some(i);
            }
        }
    });
    let usable = |i: usize| (i != 0 || first_enabled).then_some(i);
    hit.or(match keys {
        Some(true) => usable(default),
        Some(false) => Some(last),
        None => None,
    })
}

/// Title of an engine prompt's dialog.
fn prompt_title(kind: &PromptKind) -> &str {
    match kind {
        PromptKind::HostKeyUnknown { .. } => "Unknown host key",
        PromptKind::CertificateUnknown { .. } => "Unknown certificate",
        PromptKind::Password { .. } => "Password",
        PromptKind::KeyPassphrase { .. } => "Key passphrase",
        PromptKind::KeyboardInteractive { name, .. } if !name.is_empty() => name,
        PromptKind::KeyboardInteractive { .. } => "Authentication",
        PromptKind::Overwrite { .. } => "File exists",
    }
}

/// Show the first pending engine prompt. `can_save` offers "Save password" (unlocked store,
/// saved session). Returns true when it was answered.
pub(crate) fn prompt_ui(
    ctx: &egui::Context,
    p: &mut Prompt,
    can_save: bool,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) -> bool {
    let mut answer = None;
    let title = prompt_title(&p.kind).to_owned();
    dialog(ctx, Id::new(("prompt", p.id.0)), &title, 440.0, tokens, |ui, icons| {
        answer = prompt_body(ui, icons, p, can_save, tokens);
        answer.is_none()
    });
    match answer {
        Some(a) => {
            if p.save && matches!(a, PromptAnswer::Secret(_)) {
                out.push(Action::SaveSecret(p.id));
            }
            out.push(Action::Answer(p.id, a));
            p.wipe();
            true
        }
        None => false,
    }
}

/// Trust decision: Accept once, Accept and save, Cancel (the default).
fn trust_buttons(ui: &mut Ui, icons: &Icons) -> Option<PromptAnswer> {
    buttons(ui, icons, &["Accept once", "Accept and save", "Cancel"], 2, true).map(|i| {
        PromptAnswer::HostKey(match i {
            0 => HostKeyDecision::AcceptOnce,
            1 => HostKeyDecision::AcceptAndSave,
            _ => HostKeyDecision::Cancel,
        })
    })
}

/// Fingerprint in a titled group, monospace and selectable.
fn fingerprint(ui: &mut Ui, icons: &Icons, title: &str, value: &str) {
    widgets::group_box(ui, icons, Some("key"), title, |ui| {
        ui.add(egui::Label::new(RichText::new(value).monospace()).selectable(true));
    });
}

fn prompt_body(ui: &mut Ui, icons: &Icons, p: &mut Prompt, can_save: bool, tokens: &Tokens) -> Option<PromptAnswer> {
    match &p.kind {
        PromptKind::HostKeyUnknown { host, port, algorithm, fingerprint: fp } => {
            ui.label(format!("The authenticity of host {host}:{port} cannot be established."));
            fingerprint(ui, icons, &format!("{algorithm} key fingerprint"), fp);
            ui.label("Accept the key only if it matches the fingerprint published by the server administrator.");
            trust_buttons(ui, icons)
        }
        PromptKind::CertificateUnknown { host, port, subject, fingerprint: fp } => {
            ui.label(format!("The TLS certificate of {host}:{port} is not signed by a trusted authority."));
            ui.label(format!("Subject: {subject}"));
            fingerprint(ui, icons, "SHA-256 certificate fingerprint", fp);
            ui.label("No credentials are sent until you accept.");
            ui.label(
                "Accept the certificate only if it matches the fingerprint published by the server administrator.",
            );
            trust_buttons(ui, icons)
        }
        PromptKind::Password { user, host, retry } => {
            if *retry {
                ui.colored_label(color(tokens.danger), "Authentication failed. Try again.");
            }
            field(ui, &format!("Password for {user}@{host}"), &mut p.inputs[0], true);
            if can_save {
                ui.checkbox(&mut p.save, "Save password");
            }
            secret_buttons(ui, icons, &p.inputs[0])
        }
        PromptKind::KeyPassphrase { path } => {
            field(ui, &format!("Passphrase for {path}"), &mut p.inputs[0], true);
            if can_save {
                ui.checkbox(&mut p.save, "Save passphrase");
            }
            secret_buttons(ui, icons, &p.inputs[0])
        }
        PromptKind::KeyboardInteractive { instructions, prompts, .. } => {
            if !instructions.is_empty() {
                ui.label(instructions);
            }
            for ((text, echo), value) in prompts.iter().zip(p.inputs.iter_mut()) {
                field(ui, text, value, !echo);
            }
            match buttons(ui, icons, &["OK", "Cancel"], 0, true)? {
                0 => Some(responses(&p.inputs)),
                _ => Some(PromptAnswer::Cancel),
            }
        }
        PromptKind::Overwrite { path } => {
            ui.label(format!("{path} already exists. Nothing has been written yet."));
            field(ui, "Rename to", &mut p.inputs[0], false);
            buttons(ui, icons, &["Overwrite", "Skip", "Rename", "Cancel"], 3, true).map(|i| match i {
                0 => PromptAnswer::Overwrite(Overwrite::Overwrite),
                1 => PromptAnswer::Overwrite(Overwrite::Skip),
                2 => PromptAnswer::Overwrite(Overwrite::Rename(p.inputs[0].clone())),
                _ => PromptAnswer::Cancel,
            })
        }
    }
}

fn secret_buttons(ui: &mut Ui, icons: &Icons, value: &str) -> Option<PromptAnswer> {
    match buttons(ui, icons, &["OK", "Cancel"], 0, true)? {
        0 => Some(PromptAnswer::Secret(Secret(value.to_owned()))),
        _ => Some(PromptAnswer::Cancel),
    }
}

fn responses(inputs: &[String]) -> PromptAnswer {
    PromptAnswer::Responses(inputs.iter().map(|s| Secret(s.clone())).collect())
}

/// Title of a local modal.
fn modal_title(m: &Modal) -> &'static str {
    match m {
        Modal::ConfirmClose(_) => "Close tab",
        Modal::ConfirmCloseOthers(_) => "Close other tabs",
        Modal::ConfirmExit => "Exit",
        Modal::RenameTab(..) => "Rename tab",
        Modal::DeleteSavedSession { .. } => "Delete session",
        Modal::FolderName { id: Some(_), .. } => "Edit folder",
        Modal::FolderName { id: None, .. } | Modal::Mkdir { .. } => "New folder",
        Modal::DeleteFolder(_) => "Delete folder",
        Modal::Paste { .. } => "Paste multiple lines",
        Modal::NewFile { .. } => "New file",
        Modal::RenameRemote { .. } => "Rename file",
        Modal::DeleteRemote { .. } => "Confirm delete",
        Modal::Chmod { .. } => "Permissions",
        Modal::HostKeyChanged { .. } => "Host key changed",
        Modal::Notice(_) => "Notice",
        Modal::About => "About rustXterm",
    }
}

/// Show a local modal. Returns false when it closed.
pub(crate) fn modal_ui(
    ctx: &egui::Context,
    m: &mut Modal,
    tab_title: &dyn Fn(TabId) -> String,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) -> bool {
    let title = modal_title(m);
    dialog(ctx, Id::new("local-modal"), title, 420.0, tokens, |ui, icons| body(ui, icons, m, tab_title, tokens, out))
}

/// Folder icon choices; the default yellow folder is stored as `None`.
fn folder_icon_grid(ui: &mut Ui, icons: &Icons, icon: &mut Option<String>) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
        for name in crate::icons::folder_icons() {
            let Some(img) = icons.image(name, 24.0) else { continue };
            let selected = icon.as_deref().unwrap_or(crate::icons::FOLDER) == name;
            let r = ui.add(egui::Button::image(img).selected(selected));
            r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, name));
            if r.on_hover_text(name).clicked() {
                *icon = crate::icons::folder_choice(name);
            }
        }
    });
}

/// Index of Cancel in two-button confirmations: destructive ones default to it.
const CANCEL: usize = 1;

fn body(
    ui: &mut Ui,
    icons: &Icons,
    m: &mut Modal,
    tab_title: &dyn Fn(TabId) -> String,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) -> bool {
    let two =
        |ui: &mut Ui, ok: &str, default: usize, enabled: bool| buttons(ui, icons, &[ok, "Cancel"], default, enabled);
    match m {
        Modal::ConfirmClose(tab) => {
            ui.label(format!("\"{}\" is still connected. Close it and disconnect?", tab_title(*tab)));
            confirm(two(ui, "Close", CANCEL, true), Action::CloseTab(*tab), out)
        }
        Modal::ConfirmCloseOthers(tab) => {
            ui.label("Other tabs are still connected. Close them and disconnect?");
            confirm(two(ui, "Close others", CANCEL, true), Action::CloseOthers(*tab), out)
        }
        Modal::ConfirmExit => {
            ui.label("Some tabs are still connected. Exit and disconnect them?");
            confirm(two(ui, "Exit", CANCEL, true), Action::Exit, out)
        }
        Modal::RenameTab(tab, name) => {
            field(ui, "Tab name", name, false);
            let ok = !name.trim().is_empty();
            confirm(two(ui, "Rename", 0, ok), Action::RenameTab(*tab, name.trim().to_owned()), out)
        }
        Modal::DeleteSavedSession { id, name } => {
            ui.label(format!("Delete the saved session \"{name}\"? Open tabs stay open."));
            confirm(two(ui, "Delete", CANCEL, true), Action::DeleteSession(*id), out)
        }
        Modal::FolderName { id, parent, name, icon } => {
            field(ui, "Folder name", name, false);
            ui.label("Folder icon");
            folder_icon_grid(ui, icons, icon);
            let ok = !name.trim().is_empty();
            let action =
                Action::SaveFolder { id: *id, parent: *parent, name: name.trim().to_owned(), icon: icon.clone() };
            confirm(two(ui, "OK", 0, ok), action, out)
        }
        Modal::DeleteFolder(id) => {
            ui.label("Delete this folder? Its sessions move to the parent folder.");
            confirm(two(ui, "Delete", CANCEL, true), Action::DeleteFolder(*id), out)
        }
        Modal::Paste { tab, text } => {
            ui.label(format!("Paste {} lines into \"{}\"?", text.lines().count(), tab_title(*tab)));
            egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(text.as_str()).monospace()).wrap());
            });
            confirm(two(ui, "Paste", 0, true), Action::PasteConfirmed(*tab, text.clone()), out)
        }
        Modal::Mkdir { local, dir, name } => {
            ui.label(format!("In {dir}"));
            field(ui, "Folder name", name, false);
            let ok = valid_name(name);
            confirm(two(ui, "Create", 0, ok), files(*local, SftpAction::Mkdir(join(dir, name.trim()))), out)
        }
        Modal::NewFile { local, dir, name } => {
            ui.label(format!("In {dir}"));
            field(ui, "File name", name, false);
            let ok = valid_name(name);
            confirm(two(ui, "Create", 0, ok), files(*local, SftpAction::NewFile(join(dir, name.trim()))), out)
        }
        Modal::RenameRemote { local, from, to } => {
            ui.label(from.as_str());
            field(ui, "New name", to, false);
            let ok = valid_name(to);
            let action = files(*local, SftpAction::Rename { from: from.clone(), to: join(&parent(from), to.trim()) });
            confirm(two(ui, "Rename", 0, ok), action, out)
        }
        Modal::DeleteRemote { local, path } => {
            ui.label(format!("Permanently delete {path}? Folders are deleted with their contents."));
            confirm(two(ui, "Delete", CANCEL, true), files(*local, SftpAction::Delete(path.clone())), out)
        }
        Modal::Chmod { path, mode, error } => {
            ui.label(path.as_str());
            field(ui, "Octal mode", mode, false);
            if let Some(e) = error {
                ui.colored_label(color(tokens.danger), e.as_str());
            }
            match two(ui, "Apply", 0, true) {
                Some(0) => match parse_mode(mode) {
                    Some(m) => {
                        out.push(Action::Sftp(SftpAction::Chmod { path: path.clone(), mode: m }));
                        false
                    }
                    None => {
                        *error = Some("Enter an octal mode such as 644 or 755.".into());
                        true
                    }
                },
                Some(_) => false,
                None => true,
            }
        }
        Modal::HostKeyChanged { host, old, new } => {
            ui.colored_label(
                color(tokens.danger),
                format!("The host key of {host} has changed. The connection was refused and no credentials were sent."),
            );
            fingerprint(ui, icons, "Stored fingerprint", old);
            fingerprint(ui, icons, "Presented fingerprint", new);
            ui.label("This can mean the server was reinstalled, or that someone is intercepting the connection.");
            buttons(ui, icons, &["OK"], 0, true).is_none()
        }
        Modal::Notice(text) => {
            ui.label(text.as_str());
            buttons(ui, icons, &["OK"], 0, true).is_none()
        }
        Modal::About => {
            ui.label(RichText::new("rustXterm").heading());
            ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
            ui.label("Native terminal and remote session manager.");
            ui.label("Double-click a saved session or type user@host in Quick connect to open a tab.");
            ui.label("Icons: see ATTRIBUTION.md.");
            buttons(ui, icons, &["OK"], 0, true).is_none()
        }
    }
}

/// Button 0 confirms with `action`; any button closes. Returns whether the modal stays open.
fn confirm(hit: Option<usize>, action: Action, out: &mut Vec<Action>) -> bool {
    match hit {
        Some(0) => {
            out.push(action);
            false
        }
        Some(_) => false,
        None => true,
    }
}

fn files(local: bool, a: SftpAction) -> Action {
    if local { Action::Local(a) } else { Action::Sftp(a) }
}

fn valid_name(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && !n.contains('/') && n != "." && n != ".."
}

#[cfg(test)]
mod keyboard_tests;

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};

    use super::*;

    #[derive(Default)]
    struct State {
        modal: Option<Modal>,
        out: Vec<Action>,
        opener_focused: bool,
    }

    fn harness() -> Harness<'static, State> {
        let h = Harness::builder().with_size(vec2(800.0, 500.0)).build_ui_state(
            |ui, s: &mut State| {
                let r = ui.button("Delete web1...");
                if r.clicked() {
                    s.modal = Some(Modal::DeleteSavedSession { id: SessionId(7), name: "web1".into() });
                }
                s.opener_focused = r.has_focus();
                if let Some(m) = &mut s.modal
                    && !modal_ui(ui.ctx(), m, &|_| String::new(), &Tokens::light(), &mut s.out)
                {
                    s.modal = None;
                }
            },
            State::default(),
        );
        h.ctx.set_fonts(crate::theme::fonts());
        h.ctx.set_global_style(crate::theme::style(&Tokens::light()));
        h
    }

    #[test]
    fn delete_session_defaults_to_cancel_and_returns_focus() {
        let mut h = harness();
        h.run();
        h.get_by_label("Delete web1...").focus();
        h.run();
        h.key_press(egui::Key::Enter);
        h.run();
        assert!(h.state().modal.is_some(), "the opener opened the dialog");
        h.get_by_label_contains("\"web1\"");
        assert!(h.get_by_label("Cancel").accesskit_node().is_focused(), "focus starts on Cancel");
        let (ok, cancel) = (h.get_by_label("Delete").rect(), h.get_by_label("Cancel").rect());
        assert!(ok.width() >= 80.0 && (ok.height() - 30.0).abs() < 0.5, "{ok:?}");
        assert!((cancel.left() - ok.right() - 24.0).abs() < 0.5, "24 px apart");
        h.key_press(egui::Key::Enter);
        h.run();
        h.step();
        assert!(h.state().modal.is_none(), "Enter cancels");
        assert!(h.state().out.is_empty(), "nothing deleted");
        assert!(h.state().opener_focused, "focus returns to the opener");
    }

    #[test]
    fn escape_cancels_and_delete_confirms() {
        let mut h = harness();
        h.run();
        h.get_by_label("Delete web1...").click();
        h.run();
        h.key_press(egui::Key::Escape);
        h.run();
        assert!(h.state().modal.is_none() && h.state().out.is_empty());
        h.get_by_label("Delete web1...").click();
        h.run();
        h.get_by_label("Delete").click();
        h.run();
        assert_eq!(h.state().out, [Action::DeleteSession(SessionId(7))]);
    }
}
