//! Left sidebar: in-memory session model (written through to the encrypted store by the app),
//! vertical icon strip, session tree with folders, Find session filter, drag-and-drop and context menus.

use egui::{
    Align2, DragAndDrop, Key, KeyboardShortcut, LayerId, Modifiers, Order, Popup, Pos2, Rect, Response, RichText,
    Sense, SetOpenCommand, Stroke, StrokeKind, TextStyle, Vec2, WidgetInfo, WidgetType, vec2,
};
use rxt_core::{Folder, FolderId, Metrics, Session, SessionId, Tokens, Tunnel};
use serde::{Deserialize, Serialize};

use crate::app::{Action, Modal};
use crate::icons::Icons;
use crate::settings::Settings;
use crate::theme::color;

const RECENT_MAX: usize = 10;

/// Saved sessions, folders, tunnels and recent quick connects.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct SessionStore {
    pub sessions: Vec<Session>,
    pub folders: Vec<Folder>,
    pub tunnels: Vec<Tunnel>,
    pub recent: Vec<Session>,
    pub next_id: u64,
}

impl SessionStore {
    fn alloc(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub(crate) fn session(&self, id: SessionId) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    /// Insert (id 0) or replace a session; returns its id.
    pub(crate) fn upsert(&mut self, mut session: Session) -> SessionId {
        if session.id == SessionId(0) {
            session.id = SessionId(self.alloc());
        }
        let id = session.id;
        match self.sessions.iter_mut().find(|s| s.id == id) {
            Some(s) => *s = session,
            None => self.sessions.push(session),
        }
        id
    }

    pub(crate) fn duplicate(&mut self, id: SessionId) -> Option<SessionId> {
        let mut copy = self.session(id)?.clone();
        copy.id = SessionId(0);
        copy.name = format!("{} (copy)", copy.name);
        Some(self.upsert(copy))
    }

    pub(crate) fn delete(&mut self, id: SessionId) {
        self.sessions.retain(|s| s.id != id);
    }

    pub(crate) fn move_session(&mut self, id: SessionId, folder: Option<FolderId>) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.folder = folder;
        }
    }

    /// Create (id `None`) or rename a folder.
    pub(crate) fn save_folder(&mut self, id: Option<FolderId>, parent: Option<FolderId>, name: String) -> FolderId {
        match id.and_then(|id| self.folders.iter_mut().find(|f| f.id == id)) {
            Some(f) => {
                f.name = name;
                f.id
            }
            None => {
                let id = FolderId(self.alloc());
                self.folders.push(Folder { id, name, parent, icon: None });
                id
            }
        }
    }

    pub(crate) fn set_folder_icon(&mut self, id: FolderId, icon: Option<String>) {
        if let Some(f) = self.folders.iter_mut().find(|f| f.id == id) {
            f.icon = icon;
        }
    }

    /// Delete a folder; its sessions and subfolders move to its parent.
    pub(crate) fn delete_folder(&mut self, id: FolderId) {
        let parent = self.folders.iter().find(|f| f.id == id).and_then(|f| f.parent);
        self.folders.retain(|f| f.id != id);
        self.folders.iter_mut().filter(|f| f.parent == Some(id)).for_each(|f| f.parent = parent);
        self.sessions.iter_mut().filter(|s| s.folder == Some(id)).for_each(|s| s.folder = parent);
    }

    /// Folder for a `/`-separated path, creating missing levels; `None` for the root.
    pub(crate) fn folder_by_path(&mut self, path: &str) -> Option<FolderId> {
        let mut parent = None;
        for name in path.split('/').map(str::trim).filter(|n| !n.is_empty()) {
            let found = self.folders.iter().find(|f| f.parent == parent && f.name == name).map(|f| f.id);
            parent = Some(found.unwrap_or_else(|| self.save_folder(None, parent, name.to_owned())));
        }
        parent
    }

    /// Add folders, sessions and tunnels from elsewhere (import, migration) under fresh ids.
    /// Returns the number of sessions added.
    pub(crate) fn merge(&mut self, folders: &[Folder], sessions: &[Session], tunnels: &[Tunnel]) -> usize {
        let mut ids = std::collections::HashMap::new();
        let mut pending: Vec<&Folder> = folders.iter().collect();
        // Parents first; folders whose parent never resolves (cycles) are dropped.
        while !pending.is_empty() {
            let before = pending.len();
            pending.retain(|f| {
                let parent = match f.parent {
                    Some(p) if folders.iter().any(|x| x.id == p) => match ids.get(&p) {
                        Some(id) => Some(*id),
                        None => return true,
                    },
                    _ => None,
                };
                let id = self.save_folder(None, parent, f.name.clone());
                self.set_folder_icon(id, f.icon.clone());
                ids.insert(f.id, id);
                false
            });
            if pending.len() == before {
                break;
            }
        }
        let mut sids = std::collections::HashMap::new();
        for s in sessions {
            let folder = s.folder.and_then(|f| ids.get(&f).copied());
            sids.insert(s.id, self.upsert(Session { id: SessionId(0), folder, ..s.clone() }));
        }
        for t in tunnels {
            if let Some(via) = sids.get(&t.via) {
                self.tunnels.push(Tunnel { via: *via, ..t.clone() });
            }
        }
        sids.len()
    }

    /// Remember an opened session (most recent first, deduplicated by target).
    pub(crate) fn push_recent(&mut self, s: &Session) {
        // WSL targets live in `wsl` (host/port/user are empty), so include them or different
        // distributions and users would collapse into one recent entry.
        let key = |x: &Session| {
            (x.protocol, x.host.clone(), x.port, x.user.clone(), x.wsl.distribution.clone(), x.wsl.user.clone())
        };
        self.recent.retain(|r| key(r) != key(s));
        self.recent.insert(0, s.clone());
        self.recent.truncate(RECENT_MAX);
    }
}

/// Whether `s` matches the tree filter (case-insensitive name/host/user).
pub(crate) fn matches_filter(s: &Session, filter: &str) -> bool {
    let f = filter.trim().to_lowercase();
    f.is_empty()
        || s.name.to_lowercase().contains(&f)
        || s.host.to_lowercase().contains(&f)
        || s.user.as_deref().is_some_and(|u| u.to_lowercase().contains(&f))
}

/// Sidebar page shown right of the icon strip, persisted in `Settings::sidebar_page`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    Sessions,
    Sftp,
}

impl Page {
    /// Strip order: (page, settings value, icon, label).
    const STRIP: [(Self, &'static str, &'static str, &'static str); 2] =
        [(Self::Sessions, "sessions", "star", "Sessions"), (Self::Sftp, "sftp", "sftp", "SFTP")];

    /// Page stored in the settings; unknown values fall back to Sessions.
    pub(crate) fn of(settings: &Settings) -> Self {
        Self::STRIP.iter().find(|p| p.1 == settings.sidebar_page).map_or(Self::Sessions, |p| p.0)
    }

    /// Settings value of the page.
    pub(crate) fn key(self) -> &'static str {
        Self::STRIP.iter().find(|p| p.0 == self).map_or("sessions", |p| p.1)
    }
}

/// Strip button height.
const STRIP_BUTTON: f32 = 40.0;

/// Vertical icon strip at the sidebar's left edge (`strip_width` wide, full height). A click on
/// an inactive page shows it (opening a collapsed sidebar); a click on the active page collapses
/// the sidebar to the strip.
pub(crate) fn icon_strip(ui: &mut egui::Ui, settings: &mut Settings, icons: &Icons, tokens: &Tokens) {
    let m = Metrics::DEFAULT;
    let (rect, _) = ui.allocate_exact_size(vec2(m.strip_width, ui.available_height()), Sense::hover());
    let p = ui.painter().clone();
    p.rect_filled(rect, 0.0, color(tokens.panel));
    let line = Stroke::new(m.stroke, color(tokens.border));
    let current = Page::of(settings);
    let mut gap = None;
    for (i, (page, key, icon, label)) in Page::STRIP.into_iter().enumerate() {
        let b = Rect::from_min_size(rect.min + vec2(0.0, i as f32 * STRIP_BUTTON), vec2(m.strip_width, STRIP_BUTTON));
        let r = ui.interact(b, ui.id().with(("sidebar-strip", key)), Sense::click());
        let active = settings.sidebar_open && page == current;
        let fill = match (active, r.hovered()) {
            (true, _) => tokens.surface,
            (false, true) => tokens.hover,
            (false, false) => tokens.panel,
        };
        p.rect_filled(b, 0.0, color(fill));
        if active {
            // Joins the page: border on top, left and bottom only.
            let b = b.shrink(0.5);
            p.line_segment([b.right_top(), b.left_top()], line);
            p.line_segment([b.left_top(), b.left_bottom()], line);
            p.line_segment([b.left_bottom(), b.right_bottom()], line);
            gap = Some(b.y_range());
        } else {
            p.rect_stroke(b, 0.0, line, StrokeKind::Inside);
        }
        if let Some(img) = icons.image(icon, m.icon_toolbar) {
            img.paint_at(ui, Rect::from_center_size(b.center(), Vec2::splat(m.icon_toolbar)));
        }
        r.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, active, label));
        if r.on_hover_text(label).clicked() {
            if active {
                settings.sidebar_open = false;
            } else {
                settings.sidebar_page = key.to_owned();
                settings.sidebar_open = true;
            }
        }
    }
    // Right edge, open beside the active button.
    let x = rect.right() - 0.5;
    match gap {
        Some(g) => {
            p.vline(x, rect.top()..=g.min, line);
            p.vline(x, g.max..=rect.bottom(), line);
        }
        None => {
            p.vline(x, rect.y_range(), line);
        }
    }
}

/// Context menu on `r`, opened by a secondary click or, while `r` has keyboard focus, by
/// Shift+F10 (design R9). egui 0.36 has no key code for the Menu key, so it is not handled.
pub(crate) fn context_menu(r: &Response, add: impl FnOnce(&mut egui::Ui)) {
    let by_key =
        r.has_focus() && r.ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::SHIFT, Key::F10)));
    let open = if r.secondary_clicked() || by_key {
        Some(true)
    } else if r.clicked() {
        Some(false)
    } else {
        None
    };
    popup_menu(r, open, !by_key, add);
}

/// Menu anchored on `r`: `open` opens (at the pointer when `at_pointer`) or closes it.
pub(crate) fn popup_menu(r: &Response, open: Option<bool>, at_pointer: bool, add: impl FnOnce(&mut egui::Ui)) {
    let id = Popup::default_response_id(r);
    if open == Some(true) {
        let pos = if at_pointer { r.interact_pointer_pos() } else { None };
        r.ctx.data_mut(|d| d.insert_temp(id, pos.unwrap_or(r.rect.left_bottom())));
    }
    let pos = r.ctx.data(|d| d.get_temp::<Pos2>(id)).unwrap_or(r.rect.left_bottom());
    Popup::menu(r).open_memory(open.map(SetOpenCommand::Bool)).at_position(pos).show(add);
}

/// Context-menu item with a 16 px icon, a right-aligned shortcut and a disabled (not hidden)
/// state; closes the menu when clicked.
pub(crate) fn menu_item(
    ui: &mut egui::Ui,
    icons: &Icons,
    icon: &str,
    label: &str,
    shortcut: Option<&str>,
    enabled: bool,
) -> bool {
    let mut b = match icons.image(icon, Metrics::DEFAULT.icon_small) {
        Some(img) => egui::Button::image_and_text(img, label),
        None => egui::Button::new(label),
    };
    if let Some(s) = shortcut {
        b = b.shortcut_text(s);
    }
    let clicked = ui.add_enabled(enabled, b).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

/// Tree row height, session icon size and indentation per level (22 px icons on 24 px rows).
const ROW: f32 = 24.0;
const ICON: f32 = 22.0;
const INDENT: f32 = 16.0;

/// Sessions page: the "User sessions" tree fills the page. `filter` is the
/// hidden-by-default Find session field shown above the tree (`None` while hidden); `connected`
/// lists saved sessions with a Connected tab. `locked` is `Some(first_run)` while the session
/// database is locked.
#[allow(clippy::too_many_arguments)] // drawing entry point; app state is split across fields
pub(crate) fn sessions_ui(
    ui: &mut egui::Ui,
    store: &SessionStore,
    locked: Option<bool>,
    filter: &mut Option<String>,
    selected: Option<SessionId>,
    connected: &[SessionId],
    icons: &Icons,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) {
    let ctx = ui.ctx().clone();
    let tree = Tree { store, selected, connected, icons, tokens };
    let frame = egui::Frame::new().fill(color(tokens.surface)).inner_margin(egui::Margin::same(2));
    frame.show(ui, |ui| {
        if let Some(text) = filter {
            let edit = egui::TextEdit::singleline(text).hint_text("Find session").desired_width(f32::INFINITY);
            let r = ui.add(edit);
            ui.ctx().accesskit_node_builder(r.id, |b| b.set_label("Filter sessions"));
            let focus = egui::Id::new(FOCUS_FILTER);
            if ui.data(|d| d.get_temp::<bool>(focus).is_some()) {
                ui.data_mut(|d| d.remove::<bool>(focus));
                r.request_focus();
            }
            // Esc hides the field and restores the full tree.
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                *filter = None;
            }
            ui.add_space(2.0);
        }
        let filter = filter.as_deref().unwrap_or("");
        egui::ScrollArea::vertical().id_salt("session-tree").auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            tree.show(ui, locked, filter, out);
        });
    });
    // Dragged session name next to the pointer.
    if let (Some(id), Some(pos)) = (DragAndDrop::payload::<SessionId>(&ctx), ctx.pointer_hover_pos())
        && let Some(s) = store.session(*id)
    {
        let p = ctx.layer_painter(LayerId::new(Order::Tooltip, egui::Id::new("tree-drag")));
        let font = TextStyle::Body.resolve(ui.style());
        p.text(pos + vec2(14.0, 0.0), Align2::LEFT_CENTER, &s.name, font, color(tokens.text));
    }
}

/// Entries of the Quick connect list after the matching sessions.
pub(crate) const QUICK_EXTRA: [(&str, &str); 2] = [("session-new", "New session..."), ("help", "Help")];

/// Quick connect list that replaces the tree while the Quick connect field holds text: `matches`,
/// then [`QUICK_EXTRA`]. `pick` is the highlighted entry; returns the clicked entry.
pub(crate) fn quick_list(
    ui: &mut egui::Ui,
    matches: &[&Session],
    pick: Option<usize>,
    icons: &Icons,
    tokens: &Tokens,
) -> Option<usize> {
    let store = SessionStore::default();
    let tree = Tree { store: &store, selected: None, connected: &[], icons, tokens };
    let entries =
        matches.iter().map(|s| (crate::icons::session_icon(s), s.name.as_str())).chain(QUICK_EXTRA.iter().copied());
    let mut clicked = None;
    egui::Frame::new().fill(color(tokens.surface)).inner_margin(egui::Margin::same(2)).show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("quick-list").auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (i, (icon, label)) in entries.enumerate() {
                let spec = RowSpec {
                    depth: 0,
                    icon,
                    label,
                    open: None,
                    selected: pick == Some(i),
                    connected: false,
                    sense: Sense::click(),
                    kind: WidgetType::SelectableLabel,
                };
                if tree.row(ui, spec).clicked() {
                    clicked = Some(i);
                }
            }
        });
    });
    clicked
}

struct Tree<'a> {
    store: &'a SessionStore,
    selected: Option<SessionId>,
    connected: &'a [SessionId],
    icons: &'a Icons,
    tokens: &'a Tokens,
}

/// What a tree row shows.
struct RowSpec<'a> {
    depth: usize,
    icon: &'a str,
    label: &'a str,
    /// Folders: expanded or not.
    open: Option<bool>,
    selected: bool,
    /// Green status dot after the label: the session has a Connected tab.
    connected: bool,
    sense: Sense,
    kind: WidgetType,
}

impl Tree<'_> {
    fn show(&self, ui: &mut egui::Ui, locked: Option<bool>, filter: &str, out: &mut Vec<Action>) {
        let muted = color(self.tokens.text_muted);
        let root_id = ui.id().with("tree-root");
        let open =
            locked.is_some() || !filter.trim().is_empty() || ui.data_mut(|d| d.get_persisted(root_id).unwrap_or(true));
        let root = self.row(
            ui,
            RowSpec {
                depth: 0,
                icon: if locked.is_some() { "lock" } else { "folder-user" },
                label: "User sessions",
                open: Some(open),
                selected: false,
                connected: false,
                sense: Sense::click(),
                kind: WidgetType::CollapsingHeader,
            },
        );
        if let Some(first_run) = locked {
            let indent = INDENT + 4.0;
            ui.horizontal(|ui| {
                ui.add_space(indent);
                ui.label(RichText::new("Saved sessions are locked.").color(muted));
            });
            ui.horizontal(|ui| {
                ui.add_space(indent);
                let label = if first_run { "Create master password…" } else { "Unlock…" };
                if ui.button(label).clicked() {
                    out.push(Action::ShowUnlock);
                }
            });
            return;
        }
        if root.clicked() {
            ui.data_mut(|d| d.insert_persisted(root_id, !open));
        }
        self.drop_target(&root, None, out);
        context_menu(&root, |ui| self.folder_menu(ui, None, out));
        if open {
            let shown = self.folder_contents(ui, None, 1, filter, out);
            let hint = if !filter.trim().is_empty() && shown == 0 {
                Some("No matching sessions")
            } else if self.store.sessions.is_empty() && self.store.folders.is_empty() {
                Some("No sessions yet. Create one with New session or import sessions.")
            } else {
                None
            };
            if let Some(hint) = hint {
                ui.horizontal(|ui| {
                    ui.add_space(INDENT + 4.0);
                    ui.add(egui::Label::new(RichText::new(hint).color(muted)).wrap());
                });
            }
        }
        // Empty area below the rows: drop to root, context menu for root actions.
        let rest = ui.allocate_response(vec2(ui.available_width(), ui.available_height().max(ROW)), Sense::click());
        rest.widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, "Session tree"));
        self.drop_target(&rest, None, out);
        context_menu(&rest, |ui| self.folder_menu(ui, None, out));
    }

    /// Rows of `parent` at `depth`; returns the number of sessions shown.
    fn folder_contents(
        &self,
        ui: &mut egui::Ui,
        parent: Option<FolderId>,
        depth: usize,
        filter: &str,
        out: &mut Vec<Action>,
    ) -> usize {
        let filtering = !filter.trim().is_empty();
        let mut shown = 0;
        for f in self.store.folders.iter().filter(|f| f.parent == parent) {
            let name_hit = filtering && name_matches(&f.name, filter);
            if filtering && !name_hit && !self.has_match(f.id, filter) {
                continue;
            }
            // Persisted (eframe saves egui memory), so folders reopen as the user left them.
            let id = ui.id().with(("tree-folder", f.id.0));
            let open = filtering || ui.data_mut(|d| d.get_persisted(id).unwrap_or(true));
            let r = self.row(
                ui,
                RowSpec {
                    depth,
                    icon: crate::icons::folder_icon(f),
                    label: &f.name,
                    open: Some(open),
                    selected: false,
                    connected: false,
                    sense: Sense::click(),
                    kind: WidgetType::CollapsingHeader,
                },
            );
            if r.clicked() && !filtering {
                ui.data_mut(|d| d.insert_persisted(id, !open));
            }
            if r.has_focus() && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F2)) {
                out.push(Action::Ask(Modal::edit_folder(f)));
            }
            self.drop_target(&r, Some(f.id), out);
            context_menu(&r, |ui| self.folder_menu(ui, Some(f), out));
            if open {
                // A folder whose name matches shows all of its contents.
                shown += self.folder_contents(ui, Some(f.id), depth + 1, if name_hit { "" } else { filter }, out);
            }
        }
        for s in self.store.sessions.iter().filter(|s| s.folder == parent && matches_filter(s, filter)) {
            self.session_row(ui, s, depth, out);
            shown += 1;
        }
        shown
    }

    /// Whether folder `id` holds a matching session or folder at any depth.
    fn has_match(&self, id: FolderId, filter: &str) -> bool {
        self.store.sessions.iter().any(|s| s.folder == Some(id) && matches_filter(s, filter))
            || self
                .store
                .folders
                .iter()
                .filter(|f| f.parent == Some(id))
                .any(|f| name_matches(&f.name, filter) || self.has_match(f.id, filter))
    }

    /// One 20 px row: expand arrow (folders), 16 px icon, label; selection fills the icon and
    /// label width, hover the whole row.
    fn row(&self, ui: &mut egui::Ui, spec: RowSpec<'_>) -> Response {
        let t = self.tokens;
        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), spec.sense);
        let x0 = rect.left() + 2.0 + spec.depth as f32 * INDENT;
        let text_color = color(if spec.selected { t.selection_text } else { t.text });
        let galley = egui::WidgetText::from(spec.label).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            (rect.right() - x0 - 40.0).max(1.0),
            TextStyle::Body,
        );
        if ui.is_rect_visible(rect) {
            let p = ui.painter();
            if r.hovered() && !spec.selected {
                p.rect_filled(rect, 0.0, color(t.hover));
            }
            if let Some(open) = spec.open {
                let c = egui::pos2(x0 + 6.0, rect.center().y);
                let pts = if open {
                    vec![c + vec2(-4.0, -2.0), c + vec2(4.0, -2.0), c + vec2(0.0, 3.0)]
                } else {
                    vec![c + vec2(-2.0, -4.0), c + vec2(3.0, 0.0), c + vec2(-2.0, 4.0)]
                };
                p.add(egui::Shape::convex_polygon(pts, color(t.text_muted), Stroke::NONE));
            }
            // Flat rows (the Quick connect list) have no expander column.
            let icon_x = if spec.depth == 0 && spec.open.is_none() { x0 } else { x0 + 14.0 };
            if spec.selected {
                let w = ICON + 4.0 + galley.size().x + 4.0;
                let sel = Rect::from_min_size(egui::pos2(icon_x - 2.0, rect.top()), vec2(w + 2.0, ROW));
                p.rect_filled(sel, 0.0, color(t.selection));
            }
            if let Some(img) = self.icons.image(spec.icon, ICON) {
                let at = egui::pos2(icon_x, rect.center().y - ICON / 2.0);
                img.paint_at(ui, Rect::from_min_size(at, Vec2::splat(ICON)));
            }
            let pos = egui::pos2(icon_x + ICON + 4.0, rect.center().y - galley.size().y / 2.0);
            let dot = egui::pos2(pos.x + galley.size().x + 10.0, rect.center().y);
            ui.painter().galley(pos, galley, text_color);
            if spec.connected {
                ui.painter().circle_filled(dot, 3.5, color(t.success));
            }
        }
        let (kind, label, selected, open) = (spec.kind, spec.label.to_owned(), spec.selected, spec.open);
        r.widget_info(move || match open {
            Some(open) => WidgetInfo::selected(kind, true, open, &label),
            None => WidgetInfo::selected(kind, true, selected, &label),
        });
        r
    }

    fn drop_target(&self, r: &Response, folder: Option<FolderId>, out: &mut Vec<Action>) {
        if r.dnd_hover_payload::<SessionId>().is_some() {
            r.ctx.layer_painter(r.layer_id).rect_stroke(
                r.rect,
                0.0,
                Stroke::new(1.0, color(self.tokens.accent)),
                StrokeKind::Inside,
            );
        }
        if let Some(id) = r.dnd_release_payload::<SessionId>() {
            out.push(Action::MoveSession(*id, folder));
        }
    }

    fn folder_menu(&self, ui: &mut egui::Ui, folder: Option<&Folder>, out: &mut Vec<Action>) {
        let icons = self.icons;
        let id = folder.map(|f| f.id);
        if menu_item(ui, icons, "session-new", "New session", None, true) {
            out.push(Action::NewSession(id));
        }
        if menu_item(ui, icons, "new-folder", "New folder", None, true) {
            out.push(Action::Ask(Modal::folder(None, id, String::new())));
        }
        if menu_item(ui, icons, "download", "Import sessions…", None, true) {
            out.push(Action::ImportIni);
        }
        if let Some(f) = folder {
            ui.separator();
            if menu_item(ui, icons, "edit", "Edit folder…", Some("F2"), true) {
                out.push(Action::Ask(Modal::edit_folder(f)));
            }
            ui.menu_button("Change color", |ui| {
                for name in crate::icons::FOLDER_COLORS {
                    let label = match name.strip_prefix("folder-") {
                        Some(c) => c[..1].to_uppercase() + &c[1..],
                        None => "Yellow".to_owned(),
                    };
                    if menu_item(ui, icons, name, &label, None, true) {
                        let icon = crate::icons::folder_choice(name);
                        out.push(Action::SaveFolder { id: Some(f.id), parent: f.parent, name: f.name.clone(), icon });
                    }
                }
            });
            if menu_item(ui, icons, "delete", "Delete folder", None, true) {
                out.push(Action::Ask(Modal::DeleteFolder(f.id)));
            }
        }
    }

    fn session_row(&self, ui: &mut egui::Ui, s: &Session, depth: usize, out: &mut Vec<Action>) {
        let r = self.row(
            ui,
            RowSpec {
                depth,
                icon: crate::icons::session_icon(s),
                label: &s.name,
                open: None,
                selected: self.selected == Some(s.id),
                connected: self.connected.contains(&s.id),
                sense: Sense::click_and_drag(),
                kind: WidgetType::SelectableLabel,
            },
        );
        r.dnd_set_drag_payload(s.id);
        let focus = egui::Id::new(FOCUS_ROW);
        if ui.data(|d| d.get_temp::<SessionId>(focus)) == Some(s.id) {
            ui.data_mut(|d| d.remove::<SessionId>(focus));
            r.request_focus();
        }
        let key = |k| r.has_focus() && ui.input_mut(|i| i.consume_key(Modifiers::NONE, k));
        if r.double_clicked() || key(Key::Enter) {
            out.push(Action::Open(Box::new(s.clone())));
        } else if r.clicked() {
            out.push(Action::SelectSession(s.id));
        }
        if key(Key::F2) {
            out.push(Action::EditSession(s.id));
        }
        if key(Key::Delete) {
            out.push(Action::Ask(Modal::DeleteSavedSession { id: s.id, name: s.name.clone() }));
        }
        let r = r.on_hover_text(s.target());
        context_menu(&r, |ui| {
            let icons = self.icons;
            for (icon, label, shortcut, action) in [
                ("play", "Connect", Some("Enter"), Action::Open(Box::new(s.clone()))),
                ("edit", "Edit session", None, Action::EditSession(s.id)),
                ("copy", "Duplicate session", None, Action::DuplicateSession(s.id)),
                ("rename", "Rename session", Some("F2"), Action::EditSession(s.id)),
                (
                    "delete",
                    "Delete session",
                    Some("Del"),
                    Action::Ask(Modal::DeleteSavedSession { id: s.id, name: s.name.clone() }),
                ),
            ] {
                if menu_item(ui, icons, icon, label, shortcut, true) {
                    out.push(action);
                }
            }
            ui.separator();
            if menu_item(ui, icons, "session-new", "New session here", None, true) {
                out.push(Action::NewSession(s.folder));
            }
            if menu_item(ui, icons, "new-folder", "New folder here", None, true) {
                out.push(Action::Ask(Modal::folder(None, s.folder, String::new())));
            }
            if s.folder.is_some() && menu_item(ui, icons, "folder-up", "Move to root", None, true) {
                out.push(Action::MoveSession(s.id, None));
            }
        });
    }
}

/// Memory key of the session row that takes keyboard focus on its next frame.
const FOCUS_ROW: &str = "sidebar-focus-row";
/// Memory key set when the Find session field takes keyboard focus on its next frame.
const FOCUS_FILTER: &str = "sidebar-focus-filter";

/// Give the Find session field keyboard focus when it is next shown.
pub(crate) fn focus_filter(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(FOCUS_FILTER), true));
}

/// Give the session's tree row keyboard focus when it is next shown (after the editor saves it).
pub(crate) fn focus_row(ctx: &egui::Context, id: SessionId) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(FOCUS_ROW), id));
}

fn name_matches(name: &str, filter: &str) -> bool {
    name.to_lowercase().contains(&filter.trim().to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rxt_core::Protocol;

    #[test]
    fn crud_and_folders() {
        let mut st = SessionStore::default();
        let a = st.upsert(Session::new(Protocol::Ssh, "a", None, None));
        let f = st.save_folder(None, None, "lab".into());
        let sub = st.save_folder(None, Some(f), "inner".into());
        st.move_session(a, Some(sub));
        assert_eq!(st.session(a).unwrap().folder, Some(sub));
        st.delete_folder(sub);
        assert_eq!(st.session(a).unwrap().folder, Some(f));
        let b = st.duplicate(a).unwrap();
        assert_ne!(a, b);
        assert_eq!(st.session(b).unwrap().name, "a (copy)");
        st.save_folder(Some(f), None, "renamed".into());
        assert_eq!(st.folders[0].name, "renamed");
        st.delete(a);
        assert!(st.session(a).is_none());
    }

    #[test]
    fn merge_remaps_ids_and_paths_reuse_folders() {
        let mut src = SessionStore::default();
        let lab = src.save_folder(None, None, "Lab".into());
        let inner = src.save_folder(None, Some(lab), "Inner".into());
        let a = src.upsert(Session { folder: Some(inner), ..Session::new(Protocol::Ssh, "a", None, None) });
        src.tunnels.push(Tunnel { name: "t".into(), via: a, ..Tunnel::default() });
        let mut st = SessionStore::default();
        st.upsert(Session::new(Protocol::Ssh, "existing", None, None));
        // Children listed before parents still resolve.
        let folders: Vec<Folder> = src.folders.iter().rev().cloned().collect();
        assert_eq!(st.merge(&folders, &src.sessions, &src.tunnels), 1);
        let s = st.sessions.iter().find(|s| s.host == "a").unwrap().clone();
        let f = st.folders.iter().find(|f| Some(f.id) == s.folder).unwrap();
        assert_eq!(f.name, "Inner");
        assert_eq!(st.tunnels[0].via, s.id);
        assert_eq!(st.folder_by_path("Lab/Inner"), s.folder);
        let n = st.folders.len();
        st.folder_by_path("Lab/New");
        assert_eq!(st.folders.len(), n + 1);
    }

    #[test]
    fn recent_dedups_and_caps() {
        let mut st = SessionStore::default();
        for i in 0..15 {
            st.push_recent(&Session::new(Protocol::Ssh, format!("h{i}"), None, None));
        }
        st.push_recent(&Session::new(Protocol::Ssh, "h14", None, None));
        assert_eq!(st.recent.len(), RECENT_MAX);
        assert_eq!(st.recent[0].host, "h14");
        assert_eq!(st.recent.iter().filter(|s| s.host == "h14").count(), 1);
    }

    #[test]
    fn filter_matches_name_host_user() {
        let s = Session::new(Protocol::Ssh, "db.lab", Some("alice".into()), None);
        assert!(matches_filter(&s, "DB"));
        assert!(matches_filter(&s, "ali"));
        assert!(!matches_filter(&s, "bob"));
    }

    use egui::Theme;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    fn styled<S>(mut h: Harness<'static, S>, dark: bool) -> Harness<'static, S> {
        h.ctx.set_fonts(crate::theme::fonts());
        h.ctx.set_style_of(Theme::Light, crate::theme::style(&Tokens::light()));
        h.ctx.set_style_of(Theme::Dark, crate::theme::style(&Tokens::dark()));
        h.ctx.set_theme(if dark { Theme::Dark } else { Theme::Light });
        h.run();
        h
    }

    #[test]
    fn strip_switches_collapses_and_persists_page() {
        let h = Harness::builder().with_size(vec2(34.0, 300.0)).build_ui_state(
            |ui, s: &mut Settings| icon_strip(ui, s, &Icons::default(), &Tokens::light()),
            Settings::default(),
        );
        let mut h = styled(h, false);
        let b = h.get_by_label("Sessions").rect();
        assert!((b.width() - 34.0).abs() <= 1.0 && (b.height() - 40.0).abs() <= 1.0, "{b:?}");
        h.get_by_label("SFTP").click();
        h.run();
        assert_eq!((Page::of(h.state()), h.state().sidebar_open), (Page::Sftp, true));
        // Clicking the active page collapses; clicking a page while collapsed opens it.
        h.get_by_label("SFTP").click();
        h.run();
        assert_eq!((Page::of(h.state()), h.state().sidebar_open), (Page::Sftp, false));
        h.get_by_label("SFTP").click();
        h.run();
        assert!(h.state().sidebar_open);
        let base = rxt_store::settings::Settings::default();
        let restored = Settings::from_store(&h.state().to_store(&base));
        assert_eq!((Page::of(&restored), Page::Sftp.key()), (Page::Sftp, "sftp"));
        assert_eq!(Page::of(&Settings { sidebar_page: "bogus".into(), ..Settings::default() }), Page::Sessions);
    }

    struct TreeState {
        store: SessionStore,
        locked: Option<bool>,
        filter: Option<String>,
        selected: Option<SessionId>,
        connected: Vec<SessionId>,
        out: Vec<Action>,
    }

    fn tree(
        store: SessionStore,
        locked: Option<bool>,
        selected: Option<SessionId>,
        dark: bool,
    ) -> Harness<'static, TreeState> {
        let st = TreeState { store, locked, filter: None, selected, connected: Vec::new(), out: Vec::new() };
        let h = Harness::builder().with_size(vec2(240.0, 300.0)).build_ui_state(
            |ui, st: &mut TreeState| {
                let tokens = Tokens::for_dark(ui.ctx().theme() == Theme::Dark);
                let icons = Icons::default();
                let (store, connected) = (&st.store, &st.connected);
                sessions_ui(ui, store, st.locked, &mut st.filter, st.selected, connected, &icons, &tokens, &mut st.out);
            },
            st,
        );
        styled(h, dark)
    }

    fn sample() -> (SessionStore, SessionId) {
        let mut st = SessionStore::default();
        let lab = st.save_folder(None, None, "Lab".into());
        let inner = st.save_folder(None, Some(lab), "Inner".into());
        let mut web = Session::new(Protocol::Ssh, "web1.prod.example", None, None);
        web.name = "web1".into();
        web.folder = Some(inner);
        let id = st.upsert(web);
        st.upsert(Session { name: "beta".into(), ..Session::new(Protocol::Ssh, "beta.dev", None, None) });
        (st, id)
    }

    #[test]
    fn root_row_rows_and_find_session_filter() {
        let (store, _) = sample();
        let mut h = tree(store, None, None, false);
        let root = h.get_by_label("User sessions").rect();
        let lab = h.get_by_label("Lab").rect();
        assert!(root.top() < lab.top() && (root.height() - ROW).abs() <= 1.0);
        assert_eq!(h.get_by_label("Inner").rect().top() - lab.top(), ROW);
        assert!(h.query_by_label("Filter sessions").is_none(), "hidden by default");
        // Collapse Lab, then Find session: the matching session shows with its parents expanded.
        h.get_by_label("Lab").click();
        h.run();
        assert!(h.query_by_label("web1").is_none());
        h.state_mut().filter = Some(String::new());
        focus_filter(&h.ctx);
        h.run();
        h.run();
        let field = h.get_by_label("Filter sessions");
        assert!(field.is_focused(), "field focused when opened");
        assert!(field.rect().bottom() <= h.get_by_label("User sessions").rect().top(), "field above the tree");
        field.type_text("prod");
        h.run();
        assert!(h.query_by_label("web1").is_some() && h.query_by_label("Inner").is_some());
        assert!(h.query_by_label("beta").is_none());
        let web1 = h.get_by_label("web1").rect().left();
        let inner = h.get_by_label("Inner").rect();
        assert!(web1 >= inner.left(), "rows span the width");
        // Esc hides the field and restores the full tree.
        h.key_press(Key::Escape);
        h.run();
        h.run();
        assert_eq!(h.state().filter, None);
        assert!(h.query_by_label("Filter sessions").is_none());
        assert!(h.query_by_label("beta").is_some());
    }

    #[test]
    fn tree_fills_the_page_without_a_bottom_row() {
        let (store, _) = sample();
        let h = tree(store, None, None, false);
        assert!(h.query_by_label("New folder").is_none() && h.query_by_label("New session").is_none());
        // kittest adds an 8 px outer margin.
        let gap = 300.0 - 8.0 - h.get_by_label("Session tree").rect().bottom();
        assert!((0.0..=4.0).contains(&gap), "gap {gap}");
    }

    #[test]
    fn connected_session_shows_a_green_dot() {
        for dark in [false, true] {
            let (store, id) = sample();
            let mut h = tree(store, None, None, dark);
            let r = h.get_by_label("beta").rect();
            let t = Tokens::for_dark(dark);
            let success = [t.success.0, t.success.1, t.success.2, 255];
            let has_dot = |h: &mut Harness<'static, TreeState>| {
                let img = h.render().expect("render");
                (r.left() as u32..r.right() as u32).any(|x| img.get_pixel(x, r.center().y as u32).0 == success)
            };
            assert!(!has_dot(&mut h), "dark={dark}: no dot while offline");
            let beta = h.state().store.sessions.iter().find(|s| s.name == "beta").map(|s| s.id);
            assert_ne!(beta, Some(id));
            h.state_mut().connected = beta.into_iter().collect();
            h.run();
            assert!(has_dot(&mut h), "dark={dark}: dot while connected");
        }
    }

    #[test]
    fn folder_f2_and_change_color_menu() {
        let (store, _) = sample();
        let lab = store.folders[0].clone();
        let mut h = tree(store, None, None, false);
        h.get_by_label("Lab").focus();
        h.run();
        h.key_press(Key::F2);
        h.run();
        assert_eq!(h.state().out, [Action::Ask(Modal::edit_folder(&lab))]);
        h.state_mut().out.clear();
        h.key_press_modifiers(Modifiers::SHIFT, Key::F10);
        h.run();
        assert!(h.query_by_label_contains("Edit folder").is_some(), "menu open");
        h.get_by_label_contains("Change color").click();
        h.run();
        h.get_by_label("Blue").click();
        h.run();
        let want =
            Action::SaveFolder { id: Some(lab.id), parent: None, name: "Lab".into(), icon: Some("folder-blue".into()) };
        assert_eq!(h.state().out, [want]);
    }

    #[test]
    fn folder_icon_defaults_and_merge_keeps_it() {
        let mut f = Folder { name: "x".into(), ..Folder::default() };
        assert_eq!(crate::icons::folder_icon(&f), "folder");
        f.icon = Some("not-bundled".into());
        assert_eq!(crate::icons::folder_icon(&f), "folder");
        f.icon = Some("folder-docker".into());
        assert_eq!(crate::icons::folder_icon(&f), "folder-docker");
        assert_eq!(crate::icons::folder_choice("folder"), None);
        let mut st = SessionStore::default();
        st.merge(&[f], &[], &[]);
        assert_eq!(st.folders[0].icon.as_deref(), Some("folder-docker"));
    }

    #[test]
    fn selection_uses_solid_token_in_both_themes() {
        for dark in [false, true] {
            let (store, id) = sample();
            let mut h = tree(store, None, Some(id), dark);
            let r = h.get_by_label("web1").rect();
            let img = h.render().expect("render");
            let t = Tokens::for_dark(dark);
            // Depth 3: selection starts 2 px left of the icon at 2 + 3 × 16 + 14.
            let x = (r.left() + 2.0 + 3.0 * INDENT + 14.0 - 1.0) as u32;
            let px = img.get_pixel(x, r.center().y as u32).0;
            assert_eq!(px, [t.selection.0, t.selection.1, t.selection.2, 255], "dark={dark}");
            // Label text is drawn in selection_text (white) on the selection.
            let label_x = (x + 22)..(x + 50);
            let brightest = label_x
                .flat_map(|x| (r.top() as u32..r.bottom() as u32).map(move |y| (x, y)))
                .map(|(x, y)| img.get_pixel(x, y).0[..3].iter().map(|c| u32::from(*c)).sum::<u32>())
                .max()
                .unwrap_or(0);
            assert!(brightest >= 3 * 230, "dark={dark}: {brightest}");
        }
    }

    #[test]
    fn empty_and_locked_trees() {
        let mut h = tree(SessionStore::default(), None, None, false);
        assert!(h.query_by_label("User sessions").is_some());
        assert!(h.query_by_label_contains("No sessions yet").is_some());
        let (store, _) = sample();
        h = tree(store, Some(false), None, false);
        assert!(h.query_by_label("User sessions").is_some());
        assert!(h.query_by_label("Lab").is_none() && h.query_by_label("beta").is_none());
        h.get_by_label("Unlock…").click();
        h.run();
        assert_eq!(h.state().out, [Action::ShowUnlock]);
    }

    #[test]
    fn duplicate_session_from_keyboard_menu() {
        let (store, id) = sample();
        let mut h = tree(store, None, Some(id), false);
        h.get_by_label("web1").focus();
        h.run();
        assert!(h.get_by_label("web1").is_focused(), "row focused");
        h.key_press_modifiers(Modifiers::SHIFT, Key::F10);
        h.run();
        assert!(h.query_by_label_contains("Delete session").is_some(), "menu open");
        h.get_by_label("Duplicate session").click();
        h.run();
        assert_eq!(h.state().out, [Action::DuplicateSession(id)]);
        assert!(h.query_by_label_contains("Delete session").is_none(), "menu closed");
    }

    #[test]
    fn enter_opens_and_empty_area_menu_offers_root_actions() {
        let (store, id) = sample();
        let mut h = tree(store, None, Some(id), false);
        h.get_by_label("web1").focus();
        h.run();
        h.key_press(Key::Enter);
        h.run();
        assert!(matches!(h.state().out.as_slice(), [Action::Open(s)] if s.id == id), "{:?}", h.state().out);
        h.get_by_label("Session tree").click_secondary();
        h.run();
        h.get_by_label("Import sessions…").click();
        h.run();
        assert_eq!(h.state().out.last(), Some(&Action::ImportIni));
    }
}
