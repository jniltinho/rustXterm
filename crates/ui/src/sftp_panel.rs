//! SFTP side panel following the active SSH tab, plus the transfers list.

use std::path::PathBuf;
use std::time::Instant;

use egui::{Align2, Rect, RichText, Sense, Stroke, StrokeKind, WidgetInfo, WidgetType};
use rxt_core::fileops::{join, parent};
use rxt_core::{Direction, FileEntry, FileKind, TabId, Tokens, TransferId};

use crate::app::{Action, Modal};
use crate::icons::Icons;
use crate::monitor_bar::MonitorView;
use crate::theme::color;
use crate::{sidebar, states, widgets};

/// Path sent with `Command::List` to ask for the remote home directory.
pub(crate) const HOME: &str = "~";

/// Panel state for the attached tab.
#[derive(Debug, Default)]
pub(crate) struct SftpState {
    pub tab: Option<TabId>,
    pub path: String,
    pub path_edit: String,
    pub entries: Vec<FileEntry>,
    /// "Show hidden (dot) files" turned off; dotfiles are shown by default.
    pub hide_hidden: bool,
    /// "ASCII mode" toggle. ponytail: shown state only, transfers stay binary until the engine
    /// takes a per-transfer ASCII flag.
    pub ascii: bool,
    /// Folders listed in this browser, most recent first (path combo dropdown).
    pub history: Vec<String>,
    pub selected: Option<String>,
    pub loading: bool,
    pub error: Option<String>,
    pub sort: Sort,
    /// `egui` time when the current listing request started (loading state delay).
    pub loading_since: Option<f64>,
    /// Side panel: list the terminal's folder when it changes ("Follow terminal folder").
    pub follow: bool,
}

/// Sortable listing columns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum SortColumn {
    #[default]
    Name,
    Size,
    Modified,
    Owner,
    Group,
    Permissions,
}

/// Listing order: column and direction (folders always come first).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Sort {
    pub column: SortColumn,
    pub descending: bool,
}

impl Sort {
    /// Header click: the active column flips direction, another one sorts ascending.
    pub(crate) fn toggle(&mut self, column: SortColumn) {
        *self = Self { column, descending: self.column == column && !self.descending };
    }
}

impl SftpState {
    pub(crate) fn selected_path(&self) -> Option<String> {
        self.selected.as_ref().map(|n| join(&self.path, n))
    }

    fn selected_entry(&self) -> Option<&FileEntry> {
        let name = self.selected.as_ref()?;
        self.entries.iter().find(|e| &e.name == name)
    }

    /// Show the listing of `path` and remember it in the recent folders.
    pub(crate) fn listed(&mut self, path: String, entries: Vec<FileEntry>) {
        self.history.retain(|p| *p != path);
        self.history.insert(0, path.clone());
        self.history.truncate(HISTORY);
        self.path_edit = with_slash(&path);
        self.path = path;
        self.entries = entries;
        self.loading = false;
        self.error = None;
        self.selected = None;
    }
}

/// Recent folders kept per browser.
const HISTORY: usize = 12;

/// Folder path as shown in the path combo: absolute with a trailing `/`.
pub(crate) fn with_slash(path: &str) -> String {
    if path.ends_with('/') { path.to_owned() } else { format!("{path}/") }
}

/// Typed path without its trailing `/` (`/` stays `/`).
fn typed_path(text: &str) -> String {
    let t = text.trim();
    match t.trim_end_matches('/') {
        "" if t.starts_with('/') => "/".into(),
        p => p.to_owned(),
    }
}

/// File browser requests handled by the app. The remote browser sends them as
/// [`Action::Sftp`], the local pane of a file tab as [`Action::Local`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SftpAction {
    Navigate(String),
    Refresh,
    Select(String),
    Download,
    Upload,
    UploadPaths(Vec<PathBuf>),
    /// Download `remote` into the local directory `dir` (drag to the local pane).
    DownloadTo {
        remote: String,
        dir: String,
    },
    Mkdir(String),
    /// Create an empty file at this path.
    NewFile(String),
    Rename {
        from: String,
        to: String,
    },
    Delete(String),
    Chmod {
        path: String,
        mode: u32,
    },
    CancelTransfer(TransferId),
    ClearFinished,
    /// "Follow terminal folder" toggled for the attached tab.
    SetFollow(bool),
    /// Turn remote monitoring of the attached tab on or off.
    SetMonitoring(bool),
}

/// Drag payload of a listing row.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FileDrag {
    pub local: bool,
    pub path: String,
    pub dir: bool,
}

fn wrap(local: bool, a: SftpAction) -> Action {
    if local { Action::Local(a) } else { Action::Sftp(a) }
}

/// One transfer row.
#[derive(Debug, Clone)]
pub(crate) struct TransferRow {
    pub id: TransferId,
    pub tab: TabId,
    pub direction: Direction,
    pub remote: String,
    /// Local file (download destination or upload source).
    pub local: String,
    pub total: Option<u64>,
    pub done: u64,
    /// Bytes per second, updated on progress events.
    pub speed: f64,
    pub last: Instant,
    /// `None` while running; `Some(Err(message))` on failure.
    pub result: Option<Result<u64, String>>,
}

impl TransferRow {
    pub(crate) fn progress(&mut self, done: u64, now: Instant) {
        let dt = now.duration_since(self.last).as_secs_f64();
        if dt > 0.0 {
            self.speed = done.saturating_sub(self.done) as f64 / dt;
        }
        self.done = done;
        self.last = now;
    }
}

/// Height of the listing header row.
const HEADER_H: f32 = 20.0;
/// Height of listing and transfer rows.
const ROW_H: f32 = 18.0;
/// Narrowest name column before the listing scrolls sideways.
const NAME_MIN: f32 = 140.0;
/// Fixed columns after the name: (column, title, width).
const COLUMNS: [(SortColumn, &str, f32); 5] = [
    (SortColumn::Size, "Size (KB)", 64.0),
    (SortColumn::Modified, "Last modified", 112.0),
    (SortColumn::Owner, "Owner", 72.0),
    (SortColumn::Group, "Group", 72.0),
    (SortColumn::Permissions, "Access", 84.0),
];

/// Panel body: title row, then the browser, transfers (while any) and the follow/monitoring area.
pub(crate) fn sftp_ui(
    ui: &mut egui::Ui,
    st: &mut SftpState,
    monitor: Option<&MonitorView>,
    transfers: &[TransferRow],
    icons: &Icons,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) {
    title_row(ui, icons, tokens);
    if st.tab.is_some() {
        egui::Panel::bottom("sftp-follow").show(ui, |ui| {
            let mut follow = st.follow;
            let r = ui.checkbox(&mut follow, "Follow terminal folder");
            if r.on_hover_text("Automatically browse to the terminal's current folder").changed() {
                out.push(Action::Sftp(SftpAction::SetFollow(follow)));
            }
            // Toggles the bar of the attached SSH tab; pressed while it is shown.
            if let Some(m) = monitor {
                let label = "Remote monitoring";
                let reason = m.disabled_reason();
                let on = m.shown() && reason.is_none();
                let b = match icons.image("monitor-pulse", 16.0) {
                    Some(img) => egui::Button::image_and_text(img, label),
                    None => egui::Button::new(label),
                };
                let b = b.selected(on).min_size(egui::vec2(ui.available_width(), 24.0));
                let r = ui.add_enabled(reason.is_none(), b);
                r.widget_info(|| WidgetInfo::selected(WidgetType::Button, reason.is_none(), on, label));
                let r = match reason {
                    Some(why) => r.on_disabled_hover_text(why),
                    None => r.on_hover_text("Show or hide the remote monitoring bar of this tab"),
                };
                if r.clicked() {
                    out.push(Action::Sftp(SftpAction::SetMonitoring(!on)));
                }
            }
        });
    }
    if !transfers.is_empty() {
        egui::Panel::bottom("transfers").resizable(true).default_size(110.0).show(ui, |ui| {
            transfers_ui(ui, transfers, icons, tokens, out);
        });
    }
    if st.tab.is_none() {
        states::empty(ui, icons, Some("sftp"), "Open a connected SSH tab to browse its files.", None);
        return;
    }
    // Files dragged in from the desktop upload into the shown directory.
    drop_target(ui, tokens, true, |_| false, |ui| browser(ui, st, false, icons, tokens, out));
}

/// 24 px panel title: SFTP icon and "SFTP browser".
fn title_row(ui: &mut egui::Ui, icons: &Icons, tokens: &Tokens) {
    let title = "SFTP browser";
    let (rect, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), Sense::hover());
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, title));
    let p = ui.painter();
    p.rect_filled(rect, 0.0, color(tokens.panel));
    p.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, color(tokens.border_subtle)));
    if let Some(img) = icons.image("sftp", 16.0) {
        img.paint_at(
            ui,
            Rect::from_min_size(egui::pos2(rect.left() + 8.0, rect.center().y - 8.0), egui::Vec2::splat(16.0)),
        );
    }
    let font = egui::TextStyle::Body.resolve(ui.style());
    ui.painter().text(
        egui::pos2(rect.left() + 32.0, rect.center().y),
        Align2::LEFT_CENTER,
        title,
        font,
        color(tokens.text),
    );
}

/// Drop area around a browser: a 1 px accent outline while an accepted drag (a row of the other
/// pane, or desktop files when `os_files`) hovers it. Returns a row dropped on it.
pub(crate) fn drop_target(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    os_files: bool,
    accepts: impl Fn(&FileDrag) -> bool,
    add: impl FnOnce(&mut egui::Ui),
) -> Option<std::sync::Arc<FileDrag>> {
    let mut prepared = egui::Frame::NONE.begin(ui);
    add(&mut prepared.content_ui);
    let response = prepared.end(ui);
    let row = response.dnd_hover_payload::<FileDrag>().is_some_and(|d| accepts(&d));
    let desktop = os_files && ui.input(|i| !i.raw.hovered_files.is_empty());
    if row || desktop {
        let stroke = Stroke::new(1.0, color(tokens.accent));
        ui.painter().rect_stroke(response.rect, 0.0, stroke, StrokeKind::Inside);
    }
    response.dnd_release_payload::<FileDrag>().filter(|d| accepts(d))
}

/// Flat 20 px icon button with a 16 px icon; `toggle` makes it a checkable toggle (framed when on).
fn tool(
    ui: &mut egui::Ui,
    icons: &Icons,
    icon: &str,
    label: &str,
    toggle: Option<bool>,
    enabled: bool,
) -> egui::Response {
    let on = toggle == Some(true);
    let b = match icons.image(icon, 16.0) {
        Some(img) => egui::Button::image(img).min_size(egui::vec2(20.0, 20.0)).frame_when_inactive(on),
        None => egui::Button::new(label).small(),
    };
    let r = ui.add_enabled(enabled, b.selected(on));
    match toggle {
        Some(on) => {
            r.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, enabled, on, label));
            r.on_hover_text(label)
        }
        None => widgets::describe(r, label, None),
    }
}

/// Toolbar, path combo and listing of one directory. `local` selects the local pane of a file
/// tab (no transfer or ASCII buttons, [`Action::Local`] actions). The listing has the [`COLUMNS`]
/// after the name and scrolls sideways when the pane is narrow.
pub(crate) fn browser(
    ui: &mut egui::Ui,
    st: &mut SftpState,
    local: bool,
    icons: &Icons,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) {
    let act = |a| wrap(local, a);
    // After a failed listing Up leads back from the path that failed.
    let shown = if st.error.is_some() { typed_path(&st.path_edit) } else { st.path.clone() };
    // Toolbar order; transfer and ASCII buttons are remote only.
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(1.0, 1.0);
        ui.spacing_mut().button_padding = egui::vec2(2.0, 2.0);
        let sel = st.selected_entry().map(|e| e.kind);
        if tool(ui, icons, "folder-up", "Parent directory", None, true).clicked() {
            out.push(act(SftpAction::Navigate(parent(&shown))));
        }
        if !local {
            let file = sel.is_some_and(|k| k != FileKind::Dir);
            if tool(ui, icons, "download", "Download selected files", None, file).clicked() {
                out.push(act(SftpAction::Download));
            }
            if tool(ui, icons, "upload", "Upload to current folder", None, true).clicked() {
                out.push(act(SftpAction::Upload));
            }
        }
        if tool(ui, icons, "restart", "Refresh folder", None, true).clicked() {
            out.push(act(SftpAction::Refresh));
        }
        if tool(ui, icons, "new-folder", "Create new directory", None, true).clicked() {
            out.push(Action::Ask(Modal::Mkdir { local, dir: st.path.clone(), name: String::new() }));
        }
        if tool(ui, icons, "file", "Create new file", None, true).clicked() {
            out.push(Action::Ask(Modal::NewFile { local, dir: st.path.clone(), name: String::new() }));
        }
        if tool(ui, icons, "cancel", "Delete selected files", None, sel.is_some()).clicked() {
            out.extend(delete(st, local));
        }
        if !local && tool(ui, icons, "font", "ASCII mode", Some(st.ascii), true).clicked() {
            st.ascii = !st.ascii;
        }
        if tool(ui, icons, "layout-sidebar", "Show hidden (dot) files", Some(!st.hide_hidden), true).clicked() {
            st.hide_hidden = !st.hide_hidden;
        }
    });
    path_combo(ui, st, local, tokens, out);
    let now = ui.input(|i| i.time);
    let loading_since = st.loading.then(|| *st.loading_since.get_or_insert(now));
    st.loading_since = loading_since;
    listing(ui, st, local, icons, tokens, loading_since, out);
}

/// Editable path with a dropdown of the recently listed folders.
fn path_combo(ui: &mut egui::Ui, st: &mut SftpState, local: bool, tokens: &Tokens, out: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let w = (ui.available_width() - 20.0).max(40.0);
        let r = ui.add_sized([w, 22.0], egui::TextEdit::singleline(&mut st.path_edit));
        ui.ctx().accesskit_node_builder(r.id, |n| n.set_label("Path"));
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            out.push(wrap(local, SftpAction::Navigate(typed_path(&st.path_edit))));
        }
        let (rect, btn) = ui.allocate_exact_size(egui::vec2(20.0, r.rect.height()), Sense::click());
        let p = ui.painter();
        p.rect_filled(rect, 0.0, color(if btn.hovered() { tokens.hover } else { tokens.input }));
        p.rect_stroke(rect, 0.0, Stroke::new(1.0, color(tokens.border)), StrokeKind::Inside);
        let c = rect.center();
        let chevron = vec![c + egui::vec2(-4.0, -2.0), c + egui::vec2(0.0, 2.0), c + egui::vec2(4.0, -2.0)];
        p.add(egui::Shape::line(chevron, Stroke::new(1.0, color(tokens.text))));
        let btn = widgets::describe(btn, "Recent folders", None);
        egui::Popup::menu(&btn).width(w + 20.0).show(|ui| {
            if st.history.is_empty() {
                ui.label(RichText::new("No recent folders").color(color(tokens.text_muted)));
            }
            for path in &st.history {
                if ui.button(with_slash(path)).clicked() {
                    out.push(wrap(local, SftpAction::Navigate(path.clone())));
                }
            }
        });
    });
}

/// Rename request for the selected entry.
fn rename(st: &SftpState, local: bool) -> Option<Action> {
    let (from, to) = (st.selected_path()?, st.selected.clone()?);
    Some(Action::Ask(Modal::RenameRemote { local, from, to }))
}

/// Delete request (confirmed by a dialog) for the selected entry.
fn delete(st: &SftpState, local: bool) -> Option<Action> {
    Some(Action::Ask(Modal::DeleteRemote { local, path: st.selected_path()? }))
}

/// Visible entries, folders first, ordered by `sort` (name breaks ties).
pub(crate) fn sorted(entries: &[FileEntry], hide_hidden: bool, sort: Sort) -> Vec<&FileEntry> {
    let mut v: Vec<&FileEntry> = entries.iter().filter(|e| !(hide_hidden && e.is_hidden())).collect();
    v.sort_by(|a, b| {
        let by = match sort.column {
            SortColumn::Name => std::cmp::Ordering::Equal,
            SortColumn::Size => a.size.cmp(&b.size),
            SortColumn::Modified => a.modified.cmp(&b.modified),
            SortColumn::Owner => a.owner.cmp(&b.owner),
            SortColumn::Group => a.group.cmp(&b.group),
            SortColumn::Permissions => a.mode.cmp(&b.mode),
        }
        .then_with(|| a.name.cmp(&b.name));
        let by = if sort.descending { by.reverse() } else { by };
        (a.kind != FileKind::Dir).cmp(&(b.kind != FileKind::Dir)).then(by)
    });
    v
}

/// Run `add` over the visible part of the (horizontally scrolled, wider) listing, so state
/// messages are centered in the pane instead of in the full column width.
fn in_view<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let visible = ui.available_rect_before_wrap().intersect(ui.clip_rect());
    ui.scope_builder(egui::UiBuilder::new().max_rect(visible), add).inner
}

#[allow(clippy::too_many_arguments)] // drawing entry point
fn listing(
    ui: &mut egui::Ui,
    st: &mut SftpState,
    local: bool,
    icons: &Icons,
    tokens: &Tokens,
    loading_since: Option<f64>,
    out: &mut Vec<Action>,
) {
    let act = |a| wrap(local, a);
    let cols: &[(SortColumn, &str, f32)] = &COLUMNS;
    let extra: f32 = cols.iter().map(|c| c.2).sum();
    let width = ui.available_width().max(NAME_MIN + extra);
    let name_w = width - extra;
    let list_id = ui.id().with(("file-list", local));
    egui::ScrollArea::horizontal().id_salt(("file-list-h", local)).auto_shrink(false).show(ui, |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 0.0;
        header(ui, &mut st.sort, name_w, cols, icons, tokens);
        if let Some(err) = &st.error {
            let path = st.path_edit.clone();
            let retry = in_view(ui, |ui| states::error(ui, icons, &format!("Cannot list {path}:"), err, Some("Retry")));
            if retry {
                out.push(act(SftpAction::Navigate(path)));
            }
            return;
        }
        if let Some(since) = loading_since {
            in_view(ui, |ui| states::loading(ui, &format!("Loading {}...", st.path_edit), since));
            return;
        }
        let entries = sorted(&st.entries, st.hide_hidden, st.sort);
        let mut rows: Vec<(egui::Id, Option<&FileEntry>)> = Vec::new();
        if st.path != "/" {
            rows.push((list_id.with(".."), None));
        }
        rows.extend(entries.iter().map(|e| (list_id.with(&e.name), Some(*e))));
        let focused = ui.memory(|m| m.focused()).and_then(|f| rows.iter().position(|(id, _)| *id == f));
        let mut next_focus = None;
        egui::ScrollArea::vertical().id_salt(("file-list-v", local)).auto_shrink(false).show(ui, |ui| {
            for (i, (id, entry)) in rows.iter().enumerate() {
                let selected = entry.is_some_and(|e| st.selected.as_deref() == Some(e.name.as_str()));
                let r = row(ui, *id, *entry, selected, name_w, cols, icons, tokens);
                let path = entry.map_or_else(|| parent(&st.path), |e| join(&st.path, &e.name));
                if let Some(e) = entry {
                    r.dnd_set_drag_payload(FileDrag { local, path: path.clone(), dir: e.kind == FileKind::Dir });
                    row_menu(&r, e, &path, local, icons, out);
                }
                let open = r.double_clicked() || (focused == Some(i) && r.has_focus() && enter_pressed(ui));
                if open {
                    out.push(match entry.map(|e| e.kind) {
                        None | Some(FileKind::Dir | FileKind::Symlink) => act(SftpAction::Navigate(path)),
                        Some(_) if !local => act(SftpAction::Download),
                        Some(_) => act(SftpAction::Select(entry.map(|e| e.name.clone()).unwrap_or_default())),
                    });
                } else if (r.clicked() || r.secondary_clicked())
                    && let Some(e) = entry
                {
                    out.push(act(SftpAction::Select(e.name.clone())));
                }
            }
            // An empty folder keeps its ".." row (the way back up) above the message.
            if entries.is_empty() {
                in_view(ui, |ui| states::empty(ui, icons, None, "This folder is empty", None));
            }
        });
        // Keyboard: arrows move the selection, F2 renames, Delete deletes (with confirmation).
        if let Some(i) = focused {
            let (up, down, f2, del) = ui.input_mut(|inp| {
                (
                    inp.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                    inp.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                    inp.consume_key(egui::Modifiers::NONE, egui::Key::F2),
                    inp.consume_key(egui::Modifiers::NONE, egui::Key::Delete),
                )
            });
            let target = if up {
                i.checked_sub(1)
            } else if down {
                Some((i + 1).min(rows.len() - 1))
            } else {
                None
            };
            if let Some(t) = target {
                next_focus = Some(rows[t].0);
                if let Some(e) = rows[t].1 {
                    out.push(act(SftpAction::Select(e.name.clone())));
                }
            }
            if f2 {
                out.extend(rename(st, local));
            }
            if del {
                out.extend(delete(st, local));
            }
        }
        if let Some(id) = next_focus {
            ui.memory_mut(|m| m.request_focus(id));
        }
    });
}

/// Context menu of a listing row (right click, Shift+F10).
fn row_menu(r: &egui::Response, e: &FileEntry, path: &str, local: bool, icons: &Icons, out: &mut Vec<Action>) {
    let dir = matches!(e.kind, FileKind::Dir | FileKind::Symlink);
    sidebar::context_menu(r, |ui| {
        if sidebar::menu_item(ui, icons, "open", "Open", Some("Enter"), dir || local) {
            out.push(match dir {
                true => wrap(local, SftpAction::Navigate(path.to_owned())),
                false => Action::OpenLocal { path: path.into(), folder: false },
            });
        }
        if !local && sidebar::menu_item(ui, icons, "download", "Download", None, !dir) {
            out.push(Action::Sftp(SftpAction::Select(e.name.clone())));
            out.push(Action::Sftp(SftpAction::Download));
        }
        ui.separator();
        if sidebar::menu_item(ui, icons, "rename", "Rename", Some("F2"), true) {
            out.push(Action::Ask(Modal::RenameRemote { local, from: path.to_owned(), to: e.name.clone() }));
        }
        if sidebar::menu_item(ui, icons, "delete", "Delete", Some("Del"), true) {
            out.push(Action::Ask(Modal::DeleteRemote { local, path: path.to_owned() }));
        }
        if !local && sidebar::menu_item(ui, icons, "key", "Permissions", None, true) {
            let mode = e.mode.unwrap_or(0o644) & 0o7777;
            out.push(Action::Ask(Modal::Chmod { path: path.to_owned(), mode: format!("{mode:o}"), error: None }));
        }
        ui.separator();
        if sidebar::menu_item(ui, icons, "copy", "Copy path", None, true) {
            ui.ctx().copy_text(path.to_owned());
        }
    });
}

fn enter_pressed(ui: &egui::Ui) -> bool {
    ui.input(|i| i.key_pressed(egui::Key::Enter))
}

/// 20 px header on the list background with sortable column titles and a bottom border; the
/// active column shows its sort arrow (left of "Name", right of the other titles).
fn header(
    ui: &mut egui::Ui,
    sort: &mut Sort,
    name_w: f32,
    cols: &[(SortColumn, &str, f32)],
    icons: &Icons,
    tokens: &Tokens,
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), HEADER_H), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, color(tokens.border_subtle)));
    let mut x = rect.left();
    for &(col, title, w) in std::iter::once(&(SortColumn::Name, "Name", name_w)).chain(cols) {
        let cell = Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(w, HEADER_H));
        x += w;
        let r = ui.interact(cell, ui.id().with(("sort", title)), Sense::click());
        let active = sort.column == col;
        let state = match (active, sort.descending) {
            (false, _) => String::new(),
            (true, false) => ", sorted ascending".into(),
            (true, true) => ", sorted descending".into(),
        };
        r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, format!("{title}{state}")));
        if r.clicked() {
            sort.toggle(col);
        }
        let p = ui.painter();
        if r.hovered() {
            p.rect_filled(cell, 0.0, color(tokens.hover));
        }
        p.vline(cell.right() - 0.5, cell.y_range(), Stroke::new(1.0, color(tokens.border_subtle)));
        let name = col == SortColumn::Name;
        let font = egui::TextStyle::Body.resolve(ui.style());
        let text_x = if name { cell.left() + 24.0 } else { cell.left() + 4.0 };
        p.text(egui::pos2(text_x, cell.center().y), Align2::LEFT_CENTER, title, font, color(tokens.text));
        let arrow = if sort.descending { "ft-sort-desc" } else { "ft-sort-asc" };
        if active && let Some(img) = icons.image(arrow, 16.0) {
            let ax = if name { cell.left() + 4.0 } else { cell.right() - 18.0 };
            img.paint_at(ui, Rect::from_min_size(egui::pos2(ax, cell.center().y - 8.0), egui::Vec2::splat(16.0)));
        }
    }
}

/// Catalog file-type icon of a file name (by extension; shell dotfiles get the script icon).
pub(crate) fn file_icon(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if [".bashrc", ".bash_profile", ".bash_logout", ".bash_history", ".profile", ".zshrc", ".zprofile"]
        .contains(&lower.as_str())
    {
        return "ft-executable";
    }
    let ext = lower.rsplit_once('.').filter(|(stem, _)| !stem.is_empty()).map_or("", |(_, e)| e);
    match ext {
        "sh" | "bash" | "zsh" | "ksh" | "csh" | "fish" => "ft-executable",
        "txt" | "log" | "md" | "conf" | "cfg" | "ini" | "csv" | "json" | "yaml" | "yml" | "toml" | "xml" => "ft-text",
        "c" => "ft-c",
        "h" | "hpp" | "hh" => "ft-c-header",
        "cpp" | "cc" | "cxx" => "ft-cpp",
        "cs" => "ft-csharp",
        "java" | "jar" | "class" => "ft-java",
        "js" | "mjs" | "cjs" | "ts" => "ft-javascript",
        "php" => "ft-php",
        "pl" | "pm" => "ft-perl",
        "py" | "pyw" => "ft-python",
        "tex" => "ft-tex",
        "r" => "ft-r",
        "f" | "f77" | "f90" | "f95" | "for" => "ft-fortran",
        "rpm" => "ft-rpm",
        "deb" => "ft-deb",
        "apk" => "ft-apk",
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" | "zst" => "ft-archive",
        "iso" | "img" | "qcow2" | "vmdk" | "vdi" => "ft-disk-image",
        "bin" | "o" | "so" | "a" | "exe" | "dll" => "ft-binary",
        "pem" | "key" | "crt" | "gpg" | "asc" | "kdbx" => "ft-locked",
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "svg" | "webp" => "image",
        _ => "ft-file",
    }
}

/// One listing row (`None` is the parent-folder row): 16 px icon and name, then the `cols`
/// values; no separator lines, selection fills the icon and name like the session tree.
#[allow(clippy::too_many_arguments)] // drawing helper
fn row(
    ui: &mut egui::Ui,
    id: egui::Id,
    entry: Option<&FileEntry>,
    selected: bool,
    name_w: f32,
    cols: &[(SortColumn, &str, f32)],
    icons: &Icons,
    tokens: &Tokens,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), Sense::hover());
    let r = ui.interact(rect, id, Sense::click_and_drag());
    let name = entry.map_or("..", |e| e.name.as_str());
    r.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, name));
    if !ui.is_rect_visible(rect) {
        return r;
    }
    let p = ui.painter();
    if r.hovered() && !selected {
        p.rect_filled(rect, 0.0, color(tokens.hover));
    }
    let icon = match entry {
        None => "ft-folder-up",
        Some(e) => match e.kind {
            FileKind::Dir => "folder",
            FileKind::Symlink => "symlink",
            FileKind::File | FileKind::Other => file_icon(&e.name),
        },
    };
    let icon_x = rect.left() + 4.0;
    let galley = egui::WidgetText::from(name).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (name_w - 28.0).max(1.0),
        egui::TextStyle::Body,
    );
    if selected {
        let w = 16.0 + 4.0 + galley.size().x + 4.0;
        let sel = Rect::from_min_size(egui::pos2(icon_x - 2.0, rect.top()), egui::vec2(w + 2.0, ROW_H));
        p.rect_filled(sel, 0.0, color(tokens.selection));
    }
    if let Some(img) = icons.image(icon, 16.0) {
        img.paint_at(ui, Rect::from_min_size(egui::pos2(icon_x, rect.center().y - 8.0), egui::Vec2::splat(16.0)));
    }
    let text = color(if selected { tokens.selection_text } else { tokens.text });
    let p = ui.painter();
    p.galley(egui::pos2(icon_x + 20.0, rect.center().y - galley.size().y / 2.0), galley, text);
    let Some(e) = entry else { return r };
    let size = if e.kind == FileKind::Dir { String::new() } else { size_kb(e.size) };
    let modified = e.modified.map(format_time).unwrap_or_default();
    let access = permissions(e.kind, e.mode);
    let (owner, group) = (e.owner.clone().unwrap_or_default(), e.group.clone().unwrap_or_default());
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mono = egui::TextStyle::Monospace.resolve(ui.style());
    let muted = color(tokens.text_muted);
    let mut x = rect.left() + name_w;
    let values = [
        (size, font.clone(), color(tokens.text)),
        (modified, font.clone(), muted),
        (owner, font.clone(), muted),
        (group, font, muted),
        (access, mono, muted),
    ];
    for ((_, _, w), (value, font, c)) in cols.iter().zip(values) {
        let cell = Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(*w, ROW_H));
        x += w;
        ui.painter_at(cell).text(egui::pos2(cell.left() + 4.0, cell.center().y), Align2::LEFT_CENTER, value, font, c);
    }
    r
}

/// Transfers list shared by the side panel and file tabs: one 20 px row per transfer.
pub(crate) fn transfers_ui(
    ui: &mut egui::Ui,
    transfers: &[TransferRow],
    icons: &Icons,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(RichText::new("Transfers").strong());
        if transfers.iter().any(|t| t.result.is_some()) && ui.small_button("Clear finished").clicked() {
            out.push(Action::Sftp(SftpAction::ClearFinished));
        }
    });
    if transfers.is_empty() {
        ui.label(RichText::new("No transfers").color(color(tokens.text_muted)));
        return;
    }
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for t in transfers {
            transfer_row(ui, t, icons, tokens, out);
        }
    });
}

fn transfer_row(ui: &mut egui::Ui, t: &TransferRow, icons: &Icons, tokens: &Tokens, out: &mut Vec<Action>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, color(tokens.border_subtle)));
    let layout = egui::Layout::left_to_right(egui::Align::Center);
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(4.0, 0.0))).layout(layout), |ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (icon, dir) =
            if t.direction == Direction::Download { ("download", "Download") } else { ("upload", "Upload") };
        if let Some(img) = icons.image(icon, 16.0) {
            ui.add(img.alt_text(dir));
        }
        let name = t.remote.rsplit('/').next().unwrap_or(&t.remote);
        let name_w = (ui.available_width() * 0.4).max(60.0);
        ui.add_sized([name_w, ROW_H], egui::Label::new(name).truncate());
        match &t.result {
            None => {
                let frac = t.total.filter(|n| *n > 0).map_or(0.0, |n| t.done as f32 / n as f32).clamp(0.0, 1.0);
                let total = t.total.map(human_size).unwrap_or_else(|| "?".into());
                let text = format!("{} / {total}  {}/s", human_size(t.done), human_size(t.speed as u64));
                let w = (ui.available_width() - 28.0).max(20.0);
                let (bar, r) = ui.allocate_exact_size(egui::vec2(w, 12.0), Sense::hover());
                let p = ui.painter();
                p.rect_filled(
                    Rect::from_min_size(bar.min, egui::vec2(bar.width() * frac, bar.height())),
                    0.0,
                    color(tokens.accent),
                );
                p.rect_stroke(bar, 0.0, Stroke::new(1.0, color(tokens.border)), StrokeKind::Inside);
                let pct = (frac * 100.0).round();
                r.widget_info(|| WidgetInfo::labeled(WidgetType::ProgressIndicator, true, format!("{name}: {pct}%")));
                r.on_hover_text(text);
                if tool(ui, icons, "close", &format!("Cancel transfer {name}"), None, true).clicked() {
                    out.push(Action::Sftp(SftpAction::CancelTransfer(t.id)));
                }
            }
            Some(Ok(_)) => {
                ui.colored_label(color(tokens.success), "Done");
                if t.direction == Direction::Download {
                    let path = PathBuf::from(&t.local);
                    if tool(ui, icons, "file", "Open", None, true).clicked() {
                        out.push(Action::OpenLocal { path: path.clone(), folder: false });
                    }
                    if tool(ui, icons, "folder-search", "Show in folder", None, true).clicked() {
                        out.push(Action::OpenLocal { path, folder: true });
                    }
                }
            }
            Some(Err(e)) => {
                ui.add(egui::Label::new(RichText::new(e).color(color(tokens.danger))).truncate()).on_hover_text(e);
            }
        }
    });
}

/// `1.5 KiB`-style size.
pub(crate) fn human_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{n} B") } else { format!("{v:.1} {}", UNITS[unit]) }
}

/// Bytes as kilobytes for the "Size (KB)" column: at most two decimals, trailing zeros dropped.
fn size_kb(n: u64) -> String {
    let s = format!("{:.2}", n as f64 / 1024.0);
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Unix seconds as `YYYY-MM-DD HH:MM` (UTC).
pub(crate) fn format_time(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

/// `drwxr-xr-x`-style permissions.
pub(crate) fn permissions(kind: FileKind, mode: Option<u32>) -> String {
    let Some(mode) = mode else { return String::new() };
    let mut s = String::with_capacity(10);
    s.push(match kind {
        FileKind::Dir => 'd',
        FileKind::Symlink => 'l',
        FileKind::File | FileKind::Other => '-',
    });
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 7;
        s.push(if bits & 4 != 0 { 'r' } else { '-' });
        s.push(if bits & 2 != 0 { 'w' } else { '-' });
        s.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    s
}

/// Parse an octal mode such as `755`; at most `0o7777`.
pub(crate) fn parse_mode(text: &str) -> Option<u32> {
    u32::from_str_radix(text.trim(), 8).ok().filter(|m| *m <= 0o7777)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(
            (size_kb(0), size_kb(220), size_kb(4096), size_kb(1536)),
            ("0".into(), "0.21".into(), "4".into(), "1.5".into())
        );
        assert_eq!(format_time(0), "1970-01-01 00:00");
        assert_eq!(format_time(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(permissions(FileKind::Dir, Some(0o755)), "drwxr-xr-x");
        assert_eq!(permissions(FileKind::File, Some(0o640)), "-rw-r-----");
        assert_eq!(parse_mode("755"), Some(0o755));
        assert_eq!(parse_mode("9"), None);
        assert_eq!(parse_mode("17777"), None);
    }

    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    struct Pane {
        st: SftpState,
        out: Vec<Action>,
        icons: Option<Icons>,
        tokens: Tokens,
        rect: Rect,
        /// Wrap the browser in a drop target accepting rows of the local pane.
        drop: bool,
    }

    fn file(name: &str, size: u64) -> FileEntry {
        FileEntry {
            name: name.into(),
            kind: FileKind::File,
            size,
            modified: Some(1_700_000_000),
            mode: Some(0o644),
            owner: Some("alice".into()),
            group: Some("staff".into()),
        }
    }

    fn pane(st: SftpState) -> Harness<'static, Pane> {
        let state =
            Pane { st, out: Vec::new(), icons: None, tokens: Tokens::light(), rect: Rect::NOTHING, drop: false };
        let h = Harness::builder().with_size(egui::vec2(640.0, 400.0)).build_ui_state(
            |ui, p: &mut Pane| {
                let icons = p.icons.get_or_insert_with(|| Icons::load(ui.ctx(), None));
                let (st, out, tokens) = (&mut p.st, &mut p.out, &p.tokens);
                if p.drop {
                    let before = ui.available_rect_before_wrap();
                    if let Some(d) =
                        drop_target(ui, tokens, false, |d| d.local, |ui| browser(ui, st, false, icons, tokens, out))
                    {
                        out.push(Action::Status(d.path.clone()));
                    }
                    p.rect = before;
                } else {
                    browser(ui, st, false, icons, tokens, out);
                }
            },
            state,
        );
        h.ctx.set_global_style(crate::theme::style(&Tokens::light()));
        h
    }

    fn listed(entries: Vec<FileEntry>) -> SftpState {
        SftpState {
            tab: Some(TabId(1)),
            path: "/home/alice".into(),
            path_edit: "/home/alice".into(),
            entries,
            ..SftpState::default()
        }
    }

    #[test]
    fn size_header_sorts_descending_on_second_click() {
        let dir = FileEntry { kind: FileKind::Dir, ..file("logs", 4096) };
        let mut h = pane(listed(vec![file("small.txt", 10), file("big.bin", 3000), file("mid.pdf", 200), dir]));
        h.run_steps(2);
        h.get_by_label("Size (KB)").click();
        h.run_steps(2);
        h.get_by_label("Size (KB), sorted ascending").click();
        h.run_steps(2);
        h.get_by_label("Size (KB), sorted descending");
        assert_eq!(h.state().st.sort, Sort { column: SortColumn::Size, descending: true });
        let y = |name: &str| h.get_by_label(name).rect().top();
        assert!(y("logs") < y("big.bin") && y("big.bin") < y("mid.pdf") && y("mid.pdf") < y("small.txt"));
        // Rows are 20 px apart.
        assert!((y("mid.pdf") - y("big.bin") - ROW_H).abs() <= 1.0);
    }

    #[test]
    fn empty_folder_shows_header_and_message() {
        let mut h = pane(listed(Vec::new()));
        h.run_steps(2);
        h.get_by_label("Name, sorted ascending");
        h.get_by_label("This folder is empty");
        h.get_by_label(".."); // the way back up stays
    }

    #[test]
    fn permission_denied_shows_error_with_retry_and_up() {
        let mut st = listed(vec![file("a.txt", 1)]);
        st.path_edit = "/root".into();
        st.error = Some("permission denied".into());
        let mut h = pane(st);
        h.run_steps(2);
        h.get_by_label("Cannot list /root:");
        h.get_by_label("permission denied");
        h.get_by_label("Retry").click();
        h.run_steps(1);
        h.get_by_label("Parent directory").click();
        h.run_steps(1);
        assert_eq!(
            h.state().out,
            [Action::Sftp(SftpAction::Navigate("/root".into())), Action::Sftp(SftpAction::Navigate("/".into()))]
        );
    }

    #[test]
    fn loading_shows_after_delay() {
        let mut h = pane(listed(Vec::new()));
        h.state_mut().st.loading = true;
        h.state_mut().st.path_edit = "/srv".into();
        h.input_mut().time = Some(5.0);
        h.step();
        assert!(h.query_by_label_contains("Loading").is_none(), "no flash before 200 ms");
        h.input_mut().time = Some(5.3);
        h.step();
        h.get_by_label("Loading /srv...");
    }

    #[test]
    fn dragging_a_local_file_outlines_the_remote_pane() {
        let mut h = pane(listed(vec![file("a.txt", 1)]));
        h.state_mut().drop = true;
        h.run_steps(2);
        let r = h.state().rect;
        let edge = egui::pos2(r.min.x, r.center().y);
        let at = |img: &image::RgbaImage| {
            let px = img.get_pixel(edge.x as u32, edge.y as u32).0;
            [px[0], px[1], px[2]]
        };
        let accent = color(Tokens::light().accent);
        let is_accent =
            |c: [u8; 3]| c.iter().zip([accent.r(), accent.g(), accent.b()]).all(|(a, b)| a.abs_diff(b) <= 24);
        assert!(!is_accent(at(&h.render().unwrap())), "no outline without a drag");
        egui::DragAndDrop::set_payload(&h.ctx, FileDrag { local: true, path: "/srv/data/x.txt".into(), dir: false });
        h.event(egui::Event::PointerMoved(r.center()));
        h.run_steps(2);
        assert!(is_accent(at(&h.render().unwrap())), "1 px accent outline while hovering");
        h.event(egui::Event::PointerButton {
            pos: r.center(),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        h.step();
        assert_eq!(h.state().out.last(), Some(&Action::Status("/srv/data/x.txt".into())));
    }

    #[test]
    fn toolbar_order() {
        let mut h = pane(listed(vec![file("a.txt", 1)]));
        h.run_steps(2);
        let order = [
            "Parent directory",
            "Download selected files",
            "Upload to current folder",
            "Refresh folder",
            "Create new directory",
            "Create new file",
            "Delete selected files",
            "ASCII mode",
            "Show hidden (dot) files",
        ];
        let xs: Vec<f32> = order.iter().map(|l| h.get_by_label(l).rect().left()).collect();
        assert!(xs.windows(2).all(|w| w[0] < w[1]), "toolbar order {xs:?}");
        let y = h.get_by_label(order[0]).rect().top();
        assert!(order.iter().all(|l| (h.get_by_label(l).rect().top() - y).abs() < 1.0), "one row");
    }

    #[test]
    fn parent_row_first_then_folders_and_dotfiles_shown() {
        let dir = FileEntry { kind: FileKind::Dir, ..file("zeta", 4096) };
        let mut h = pane(listed(vec![file("b.txt", 1), file(".profile", 1), dir]));
        h.run_steps(2);
        let y = |h: &Harness<'static, Pane>, name: &str| h.get_by_label(name).rect().top();
        assert!(y(&h, "..") < y(&h, "zeta") && y(&h, "zeta") < y(&h, ".profile") && y(&h, ".profile") < y(&h, "b.txt"));
        assert!(!h.state().st.hide_hidden, "dotfiles shown by default");
        h.get_by_label("Show hidden (dot) files").click();
        h.run_steps(2);
        assert!(h.state().st.hide_hidden);
        assert!(h.query_by_label(".profile").is_none());
        h.get_by_label("ASCII mode").click();
        h.run_steps(1);
        assert!(h.state().st.ascii);
    }

    #[test]
    fn path_combo_lists_recent_folders() {
        let mut st = listed(Vec::new());
        st.listed("/etc".into(), Vec::new());
        st.listed("/var/log".into(), Vec::new());
        st.listed("/etc".into(), Vec::new());
        assert_eq!(st.history, ["/etc", "/var/log"]);
        assert_eq!(st.path_edit, "/etc/");
        let mut h = pane(st);
        h.run_steps(2);
        h.get_by_label("Recent folders").click();
        h.run_steps(2);
        h.get_by_label("/var/log/").click();
        h.run_steps(2);
        assert_eq!(h.state().out, [Action::Sftp(SftpAction::Navigate("/var/log".into()))]);
        assert_eq!(typed_path(" /srv/data/ "), "/srv/data");
        assert_eq!(typed_path("///"), "/");
    }

    #[test]
    fn new_file_and_delete_buttons() {
        let mut h = pane(listed(vec![file("a.txt", 1)]));
        h.run_steps(2);
        h.get_by_label("Create new file").click();
        h.run_steps(1);
        let new = Action::Ask(Modal::NewFile { local: false, dir: "/home/alice".into(), name: String::new() });
        assert_eq!(h.state().out, [new]);
        h.get_by_label("Delete selected files").click();
        h.run_steps(1);
        assert_eq!(h.state().out.len(), 1, "Delete is disabled without a selection");
    }

    #[test]
    fn file_icons_by_type() {
        assert_eq!(file_icon(".bashrc"), "ft-executable");
        assert_eq!(file_icon("run.sh"), "ft-executable");
        assert_eq!(file_icon("main.PY"), "ft-python");
        assert_eq!(file_icon("backup.tar.gz"), "ft-archive");
        assert_eq!(file_icon(".Xauthority"), "ft-file");
        assert_eq!(file_icon("README"), "ft-file");
        for name in ["a.c", "a.h", "a.txt", "a.deb", "a.iso", "a.pem", "a.png", "x"] {
            assert!(crate::icons::BUNDLED.iter().any(|b| b.0 == file_icon(name)), "{name}");
        }
    }

    #[test]
    fn sort_toggles() {
        let mut s = Sort::default();
        s.toggle(SortColumn::Size);
        assert_eq!(s, Sort { column: SortColumn::Size, descending: false });
        s.toggle(SortColumn::Size);
        assert!(s.descending);
        s.toggle(SortColumn::Name);
        assert_eq!(s, Sort::default());
    }

    #[test]
    fn sorts_by_owner() {
        let by = |o: &str, n: &str| FileEntry { owner: Some(o.into()), ..file(n, 1) };
        let v = [by("root", "a"), by("alice", "b"), by("bob", "c")];
        let names: Vec<&str> = sorted(&v, false, Sort { column: SortColumn::Owner, descending: false })
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, ["b", "c", "a"]);
    }

    #[test]
    fn speed_from_progress() {
        let t0 = Instant::now();
        let mut row = TransferRow {
            id: TransferId(1),
            tab: TabId(1),
            direction: Direction::Upload,
            remote: "/tmp/a".into(),
            local: "/tmp/b".into(),
            total: Some(100),
            done: 0,
            speed: 0.0,
            last: t0,
            result: None,
        };
        row.progress(50, t0 + std::time::Duration::from_secs(1));
        assert_eq!((row.done, row.speed as u64), (50, 50));
    }
}
