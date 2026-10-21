//! Tab strip: pinned Home tab, one tab per connection with protocol and state icons, "+" for a
//! new session, drag to reorder, context menu and MultiExec selection.

use egui::{
    Align, DragAndDrop, LayerId, Layout, Order, Rect, Response, Sense, Stroke, StrokeKind, TextStyle, Vec2, WidgetInfo,
    WidgetType, vec2,
};
use rxt_core::{CommandId, ConnState, Protocol, Session, TabId, Tokens};

use crate::app::Action;
use crate::desktop::DesktopView;
use crate::file_tab::{FileView, unencrypted};
use crate::icons::Icons;
use crate::settings::Settings;
use crate::sidebar::{context_menu, menu_item};
use crate::theme::color;
use crate::widgets::describe;

/// One open connection.
pub(crate) struct Tab {
    pub id: TabId,
    pub session: Session,
    pub title: String,
    pub state: ConnState,
    pub term: Option<rxt_term::Terminal>,
    /// Last directory reported by the shell (OSC 7).
    pub cwd: Option<String>,
    pub multiexec: bool,
    /// Dual-pane browser of SFTP/FTP/FTPS tabs.
    pub files: Option<Box<FileView>>,
    /// Framebuffer view of RDP/VNC tabs.
    pub desktop: Option<Box<DesktopView>>,
    /// Dismissible notice above the terminal (Telnet: not encrypted).
    pub notice: Option<&'static str>,
    /// Serial DTR/RTS as last set from this tab (driver default: asserted).
    pub lines: (bool, bool),
    /// Connection facts of the current SSH login, for the session banner.
    pub ssh_info: Option<Box<rxt_core::SshInfo>>,
    /// The server refused the sftp subsystem at login: no SFTP browser for this tab.
    pub no_sftp: bool,
    /// Remote monitoring bar state (SSH terminals).
    pub monitor: crate::monitor_bar::MonitorView,
    /// A WSL tab's startup commands await the user's go-ahead (no prompt within the timeout).
    pub wsl_startup_pending: bool,
}

impl Tab {
    pub(crate) fn new(id: TabId, session: Session, scrollback: usize) -> Self {
        let term = session.protocol.is_terminal().then(|| rxt_term::Terminal::new(80, 24, scrollback));
        let files = matches!(session.protocol, Protocol::Sftp | Protocol::Ftp | Protocol::Ftps).then(|| {
            let mut v = Box::<FileView>::default();
            v.remote.tab = Some(id);
            v
        });
        let desktop = matches!(session.protocol, Protocol::Rdp | Protocol::Vnc).then(Box::<DesktopView>::default);
        let session_is_telnet = session.protocol == Protocol::Telnet;
        let lines = (session.serial.dtr.unwrap_or(true), session.serial.rts.unwrap_or(true));
        Self {
            id,
            title: session.name.clone(),
            session,
            state: ConnState::Connecting,
            term,
            cwd: None,
            multiexec: false,
            files,
            desktop,
            lines,
            notice: (session_is_telnet).then_some(TELNET_NOTICE),
            ssh_info: None,
            no_sftp: false,
            monitor: crate::monitor_bar::MonitorView::default(),
            wsl_startup_pending: false,
        }
    }

    /// Connecting or connected: closing it drops a live connection.
    pub(crate) fn is_live(&self) -> bool {
        matches!(self.state, ConnState::Connecting | ConnState::Connected)
    }
}

/// First-connect notice of Telnet tabs.
pub(crate) const TELNET_NOTICE: &str =
    "Telnet is not encrypted: everything you type, including passwords, is sent in clear text.";

/// Icon name for a connection state.
pub(crate) fn state_icon(s: &ConnState) -> &'static str {
    match s {
        ConnState::Connecting => "state-connecting",
        ConnState::Connected => "state-connected",
        ConnState::Disconnected { .. } => "state-disconnected",
        ConnState::Error(_) => "error",
    }
}

pub(crate) fn state_text(s: &ConnState) -> String {
    match s {
        ConnState::Connecting => "Connecting".into(),
        ConnState::Connected => "Connected".into(),
        ConnState::Disconnected { reason, .. } if reason.is_empty() => "Disconnected".into(),
        ConnState::Disconnected { reason, .. } => format!("Disconnected: {reason}"),
        ConnState::Error(e) => format!("Error: {e}"),
    }
}

/// Command for a key typed into a stopped session: Enter or Ctrl+D closes the tab, R reconnects,
/// S saves the terminal text.
pub(crate) fn ended_session_key(input: &[u8]) -> Option<CommandId> {
    match input.last()? {
        b'\r' | b'\n' | 0x04 => Some(CommandId::CLOSE_TAB),
        b'r' | b'R' => Some(CommandId::RECONNECT),
        b's' | b'S' => Some(CommandId::SAVE_TEXT),
        _ => None,
    }
}

/// Move the tab at `from` so it ends up at index `to`.
pub(crate) fn move_tab<T>(tabs: &mut Vec<T>, from: usize, to: usize) {
    if from < tabs.len() && to < tabs.len() && from != to {
        let t = tabs.remove(from);
        tabs.insert(to, t);
    }
}

/// Index the dragged tab at `from` gets when dropped into `gap` (0 = before the first tab).
pub(crate) fn drop_index(from: usize, gap: usize) -> usize {
    if gap > from { gap - 1 } else { gap }
}

/// Tab caption: `N. title` (N counts session tabs from 1 in strip order) or the bare title.
pub(crate) fn tab_caption(index: usize, title: &str, numbers: bool) -> String {
    if numbers { format!("{}. {title}", index + 1) } else { title.to_owned() }
}

/// Strip geometry (design R6).
const TAB_HEIGHT: f32 = 26.0;
const HOME_WIDTH: f32 = 36.0;
const TAB_WIDTH: std::ops::RangeInclusive<f32> = 80.0..=220.0;
const CLOSE: f32 = 16.0;

/// Drag payload: index of the dragged tab.
#[derive(Clone, Copy)]
struct TabDrag(usize);

/// Draw the strip: pinned Home tab, one tab per connection, then "+". Numbering and close
/// buttons follow `settings.show_tab_numbers` / `show_close_buttons`.
#[allow(clippy::too_many_arguments)] // drawing entry point; app state is split across fields
pub(crate) fn tab_strip(
    ui: &mut egui::Ui,
    tabs: &[Tab],
    active: Option<TabId>,
    multiexec: bool,
    local: bool,
    settings: &Settings,
    icons: &Icons,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) {
    let ctx = ui.ctx().clone();
    let row = ui.with_layout(Layout::left_to_right(Align::Max), |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if home_tab(ui, icons, tokens, active.is_none()).clicked() {
            out.push(Action::SelectTab(None));
        }
        let mut rects = Vec::with_capacity(tabs.len());
        for (i, tab) in tabs.iter().enumerate() {
            if multiexec && tab.session.protocol.is_terminal() {
                let mut sel = tab.multiexec;
                let r = ui.checkbox(&mut sel, "");
                r.widget_info(|| {
                    WidgetInfo::selected(WidgetType::Checkbox, true, sel, format!("MultiExec {}", tab.title))
                });
                if r.changed() {
                    out.push(Action::SelectMultiExec(tab.id, sel));
                }
            }
            let caption = tab_caption(i, &tab.title, settings.show_tab_numbers);
            let (r, close) = tab_widget(
                ui,
                icons,
                tokens,
                &TabLook {
                    selected: active == Some(tab.id),
                    icon: crate::icons::session_icon(&tab.session),
                    state: state_icon(&tab.state),
                    marker: multiexec && tab.multiexec,
                    title: &caption,
                    unencrypted: unencrypted(tab.session.protocol),
                    close: settings.show_close_buttons,
                },
            );
            rects.push(r.rect);
            if close.is_some_and(|c| c.clicked()) || r.middle_clicked() {
                out.push(Action::OnTab(tab.id, CommandId::CLOSE_TAB));
            } else if r.clicked() {
                out.push(Action::SelectTab(Some(tab.id)));
            }
            r.dnd_set_drag_payload(TabDrag(i));
            let r = r.on_hover_text(format!("{}\n{}", tab.title, state_text(&tab.state)));
            context_menu(&r, |ui| {
                let sc = |cmd| settings.shortcut_text(cmd);
                for (icon, label, cmd, enabled) in [
                    ("rename", "Rename", CommandId::RENAME_TAB, true),
                    ("copy", "Duplicate", CommandId::DUPLICATE_TAB, true),
                    ("restart", "Reconnect", CommandId::RECONNECT, !tab.is_live()),
                    ("close", "Close", CommandId::CLOSE_TAB, true),
                    ("close", "Close others", CommandId::CLOSE_OTHERS, tabs.len() > 1),
                ] {
                    if cmd == CommandId::CLOSE_TAB {
                        ui.separator();
                    }
                    if menu_item(ui, icons, icon, label, sc(cmd).as_deref(), enabled) {
                        out.push(Action::OnTab(tab.id, cmd));
                    }
                    if cmd == CommandId::RECONNECT && tab.session.protocol == Protocol::Serial {
                        serial_menu(ui, tab, icons, &sc, out);
                    }
                }
            });
        }
        // "+" opens a local terminal (MobaXterm-like) where the platform has one.
        let (cmd, label) = if local {
            (CommandId::NEW_LOCAL_TERMINAL, "New local terminal")
        } else {
            (CommandId::NEW_SESSION, "New session")
        };
        let sc = settings.shortcut_text(cmd);
        if describe(plus_button(ui, tokens), label, sc.as_deref()).clicked() {
            out.push(Action::Run(cmd));
        }
        rects
    });
    // 2 px accent insertion line at the drop gap; the move happens on release. The gap is
    // remembered because the pointer may already be gone in the release frame.
    let rects = row.inner;
    let gap_id = ui.id().with("tab-drop");
    let Some(drag) = DragAndDrop::payload::<TabDrag>(&ctx) else {
        ctx.data_mut(|d| d.remove::<usize>(gap_id));
        return;
    };
    if let Some(pos) = ctx.pointer_hover_pos() {
        let inside = !rects.is_empty() && row.response.rect.y_range().contains(pos.y);
        let gap = rects.iter().position(|r| pos.x < r.center().x).unwrap_or(rects.len());
        ctx.data_mut(|d| {
            if inside {
                d.insert_temp(gap_id, gap);
            } else {
                d.remove::<usize>(gap_id);
            }
        });
    }
    let gap = ctx.data(|d| d.get_temp::<usize>(gap_id));
    if let Some((gap, last)) = gap.zip(rects.last()) {
        let x = rects.get(gap).map_or(last.right() + 1.0, |r| r.left() - 1.0);
        let line = Stroke::new(2.0, color(tokens.accent));
        ctx.layer_painter(LayerId::new(Order::Foreground, gap_id)).vline(x, last.y_range(), line);
    }
    if ctx.input(|i| i.pointer.any_released()) {
        DragAndDrop::clear_payload(&ctx);
        ctx.data_mut(|d| d.remove::<usize>(gap_id));
        if let Some(gap) = gap {
            let to = drop_index(drag.0, gap);
            if to != drag.0 {
                out.push(Action::MoveTab { from: drag.0, to });
            }
        }
    }
}

/// Serial line entries of the tab context menu: Send break, DTR and RTS toggles.
fn serial_menu(
    ui: &mut egui::Ui,
    tab: &Tab,
    icons: &Icons,
    sc: &dyn Fn(CommandId) -> Option<String>,
    out: &mut Vec<Action>,
) {
    let live = tab.state == ConnState::Connected;
    let brk = CommandId::SERIAL_SEND_BREAK;
    if menu_item(ui, icons, "bolt", "Send break", sc(brk).as_deref(), live) {
        out.push(Action::OnTab(tab.id, brk));
    }
    let (dtr, rts) = tab.lines;
    let on_off = |on: bool| if on { "on" } else { "off" };
    if menu_item(ui, icons, "plug", &format!("DTR ({}): turn {}", on_off(dtr), on_off(!dtr)), None, live) {
        out.push(Action::SerialLines { tab: tab.id, dtr: Some(!dtr), rts: None });
    }
    if menu_item(ui, icons, "plug", &format!("RTS ({}): turn {}", on_off(rts), on_off(!rts)), None, live) {
        out.push(Action::SerialLines { tab: tab.id, dtr: None, rts: Some(!rts) });
    }
}

/// Fill and border of a tab; the active tab joins the content area (no bottom border).
fn paint_tab(ui: &egui::Ui, rect: Rect, selected: bool, hovered: bool, tokens: &Tokens) {
    let fill = match (selected, hovered) {
        (true, _) => tokens.tab_active,
        (false, true) => tokens.hover,
        (false, false) => tokens.tab_inactive,
    };
    let p = ui.painter();
    p.rect_filled(rect, 0.0, color(fill));
    let line = Stroke::new(1.0, color(tokens.border));
    if selected {
        let r = rect.shrink(0.5);
        p.line_segment([r.left_bottom() + vec2(0.0, 0.5), r.left_top()], line);
        p.line_segment([r.left_top(), r.right_top()], line);
        p.line_segment([r.right_top(), r.right_bottom() + vec2(0.0, 0.5)], line);
        p.rect_filled(Rect::from_min_size(rect.min, vec2(rect.width(), 2.0)), 0.0, color(tokens.accent));
    } else {
        p.rect_stroke(rect, 0.0, line, StrokeKind::Inside);
    }
}

/// Pinned, icon-only Home tab.
fn home_tab(ui: &mut egui::Ui, icons: &Icons, tokens: &Tokens, selected: bool) -> Response {
    let (rect, r) = ui.allocate_exact_size(vec2(HOME_WIDTH, TAB_HEIGHT), Sense::click());
    if ui.is_rect_visible(rect) {
        paint_tab(ui, rect, selected, r.hovered(), tokens);
        if let Some(img) = icons.image("house", 16.0) {
            img.paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(16.0)));
        }
    }
    r.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, "Home"));
    r.on_hover_text("Home")
}

/// Square "+" button after the last tab.
fn plus_button(ui: &mut egui::Ui, tokens: &Tokens) -> Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(TAB_HEIGHT), Sense::click());
    if ui.is_rect_visible(rect) {
        paint_tab(ui, rect, false, r.hovered(), tokens);
        let (c, s) = (rect.center(), Stroke::new(1.5, color(tokens.text)));
        ui.painter().hline(c.x - 5.0..=c.x + 5.0, c.y, s);
        ui.painter().vline(c.x, c.y - 5.0..=c.y + 5.0, s);
    }
    r
}

/// What a session tab shows.
struct TabLook<'a> {
    selected: bool,
    icon: &'a str,
    state: &'a str,
    marker: bool,
    title: &'a str,
    unencrypted: bool,
    close: bool,
}

/// Session tab (80–220 px, ellipsized title) and its inner close button, hit-tested first.
fn tab_widget(ui: &mut egui::Ui, icons: &Icons, tokens: &Tokens, t: &TabLook<'_>) -> (Response, Option<Response>) {
    let font = TextStyle::Button.resolve(ui.style());
    // Plain FTP: a visible "Unencrypted" badge after the title.
    let badge =
        t.unencrypted.then(|| ui.painter().layout_no_wrap("Unencrypted".into(), font.clone(), color(tokens.warning)));
    let icon_count = 2 + usize::from(t.marker);
    let badge_w = badge.as_ref().map_or(0.0, |b| b.size().x + 6.0);
    let fixed = 6.0 + icon_count as f32 * 18.0 + badge_w + if t.close { CLOSE + 4.0 } else { 0.0 } + 6.0;
    let natural = ui.painter().layout_no_wrap(t.title.to_owned(), font, color(tokens.text)).size().x;
    let width = (fixed + natural + 2.0).clamp(*TAB_WIDTH.start(), *TAB_WIDTH.end());
    let galley = egui::WidgetText::from(t.title).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (width - fixed - 2.0).max(1.0),
        TextStyle::Button,
    );
    let (rect, resp) = ui.allocate_exact_size(vec2(width, TAB_HEIGHT), Sense::click_and_drag());
    let close_rect =
        Rect::from_min_size(egui::pos2(rect.right() - 4.0 - CLOSE, rect.center().y - CLOSE / 2.0), Vec2::splat(CLOSE));
    let close = t.close.then(|| ui.interact(close_rect, resp.id.with("close"), Sense::click()));
    if ui.is_rect_visible(rect) {
        paint_tab(ui, rect, t.selected, resp.hovered(), tokens);
        let mut x = rect.min.x + 6.0;
        for name in [t.icon, t.state].into_iter().chain(t.marker.then_some("multiexec")) {
            if let Some(img) = icons.image(name, 16.0) {
                img.paint_at(ui, Rect::from_min_size(egui::pos2(x, rect.center().y - 8.0), Vec2::splat(16.0)));
            }
            x += 18.0;
        }
        let title_w = galley.size().x;
        let p = ui.painter();
        p.galley(egui::pos2(x + 2.0, rect.center().y - galley.size().y / 2.0), galley, color(tokens.text));
        if let Some(b) = badge {
            p.galley(egui::pos2(x + 2.0 + title_w + 6.0, rect.center().y - b.size().y / 2.0), b, color(tokens.warning));
        }
        if let Some(c) = &close {
            if c.hovered() {
                p.rect_filled(close_rect, 0.0, color(tokens.pressed));
            }
            // Thin glyph, not the colored catalog icon.
            let (r, s) = (close_rect.shrink(5.0), Stroke::new(1.0, color(tokens.text)));
            p.line_segment([r.left_top(), r.right_bottom()], s);
            p.line_segment([r.right_top(), r.left_bottom()], s);
        }
    }
    let title = t.title.to_owned();
    if let Some(c) = &close {
        let name = format!("Close {title}");
        c.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &name));
    }
    let selected = t.selected;
    resp.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, &title));
    let resp = if t.unencrypted {
        resp.on_hover_text("Unencrypted: the password and all data are sent in clear text.")
    } else {
        resp
    };
    (resp, close.map(|c| c.on_hover_text("Close tab")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ended_session_keys() {
        assert_eq!(ended_session_key(b"\x04"), Some(CommandId::CLOSE_TAB));
        assert_eq!(ended_session_key(b"\r"), Some(CommandId::CLOSE_TAB));
        assert_eq!(ended_session_key(b"R"), Some(CommandId::RECONNECT));
        assert_eq!(ended_session_key(b"s"), Some(CommandId::SAVE_TEXT));
        assert_eq!(ended_session_key(b"x"), None);
        assert_eq!(ended_session_key(b""), None);
    }

    #[test]
    fn move_tab_reorders_and_ignores_out_of_range() {
        let mut v = vec![1, 2, 3, 4];
        move_tab(&mut v, 0, 2);
        assert_eq!(v, [2, 3, 1, 4]);
        move_tab(&mut v, 3, 0);
        assert_eq!(v, [4, 2, 3, 1]);
        move_tab(&mut v, 9, 0);
        assert_eq!(v, [4, 2, 3, 1]);
    }

    #[test]
    fn drop_index_and_caption() {
        assert_eq!(drop_index(2, 0), 0);
        assert_eq!(drop_index(0, 3), 2);
        assert_eq!(drop_index(1, 1), 1);
        assert_eq!(tab_caption(1, "web", true), "2. web");
        assert_eq!(tab_caption(1, "web", false), "web");
    }

    use egui::{PointerButton, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};

    struct Strip {
        tabs: Vec<Tab>,
        settings: Settings,
        out: Vec<Action>,
    }

    fn harness(names: &[&str], settings: Settings) -> Harness<'static, Strip> {
        let tabs = names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let mut s = Session::new(Protocol::Ssh, *n, None, None);
                s.name = (*n).to_owned();
                Tab::new(TabId(i as u64 + 1), s, 10)
            })
            .collect();
        let st = Strip { tabs, settings, out: Vec::new() };
        let mut h = Harness::builder().with_size(vec2(1000.0, 200.0)).build_ui_state(
            |ui, st: &mut Strip| {
                let (icons, tokens) = (Icons::default(), Tokens::light());
                let mut new = Vec::new();
                tab_strip(ui, &st.tabs, Some(TabId(1)), false, false, &st.settings, &icons, &tokens, &mut new);
                for a in new {
                    if let Action::MoveTab { from, to } = a {
                        move_tab(&mut st.tabs, from, to);
                    }
                    st.out.push(a);
                }
            },
            st,
        );
        h.ctx.set_fonts(crate::theme::fonts());
        h.ctx.set_style_of(egui::Theme::Light, crate::theme::style(&Tokens::light()));
        h.run();
        h
    }

    fn drag(h: &mut Harness<'static, Strip>, from: &str, to: egui::Pos2, cancel: bool) {
        let from = h.get_by_label(from).rect().center();
        h.drag_at(from);
        h.run_steps(2);
        h.hover_at(from + vec2(10.0, 0.0));
        h.run_steps(2);
        h.hover_at(to);
        h.run_steps(2);
        if cancel {
            h.key_press(egui::Key::Escape);
            h.run_steps(2);
        }
        h.drop_at(to);
        h.run_steps(3);
    }

    #[test]
    fn home_plus_sizes_and_numbering_after_reorder() {
        let long = "a-very-long-session-name-that-does-not-fit-into-one-tab.example.org";
        let mut h = harness(&["alpha", "beta", long], Settings::default());
        let home = h.get_by_label("Home").rect();
        assert!((home.width() - 36.0).abs() <= 2.0, "{home:?}");
        for name in ["1. alpha", "2. beta", &format!("3. {long}")] {
            let r = h.get_by_label(name).rect();
            assert!((r.height() - 26.0).abs() <= 1.0 && (80.0..=220.0).contains(&r.width()), "{name}: {r:?}");
            let close = h.get_by_label(&format!("Close {name}")).rect();
            assert!(r.contains_rect(close), "close inside {name}");
        }
        assert_eq!(h.get_by_label(&format!("3. {long}")).rect().width(), 220.0);
        let plus = h.get_by_label("New session").rect();
        assert!(plus.left() > h.get_by_label(&format!("3. {long}")).rect().right());
        // Drag the third tab over the first: it moves to the front and numbers follow.
        let first = h.get_by_label("1. alpha").rect();
        drag(&mut h, &format!("3. {long}"), first.left_center() + vec2(5.0, 0.0), false);
        assert!(h.state().out.contains(&Action::MoveTab { from: 2, to: 0 }), "{:?}", h.state().out);
        assert!(h.query_by_label(&format!("1. {long}")).is_some());
        assert!(h.query_by_label("2. alpha").is_some() && h.query_by_label("3. beta").is_some());
    }

    #[test]
    fn escape_cancels_tab_drag() {
        let mut h = harness(&["alpha", "beta", "gamma"], Settings::default());
        let first = h.get_by_label("1. alpha").rect();
        drag(&mut h, "3. gamma", first.left_center() + vec2(5.0, 0.0), true);
        assert!(!h.state().out.iter().any(|a| matches!(a, Action::MoveTab { .. })), "{:?}", h.state().out);
        assert!(h.query_by_label("3. gamma").is_some());
    }

    #[test]
    fn close_buttons_hidden_middle_click_closes_and_plus_runs_new_session() {
        let settings = Settings { show_close_buttons: false, show_tab_numbers: false, ..Settings::default() };
        let mut h = harness(&["alpha", "beta"], settings);
        assert!(h.query_by_label_contains("Close").is_none());
        h.get_by_label("beta").click_button(PointerButton::Middle);
        h.run();
        assert!(h.state().out.contains(&Action::OnTab(TabId(2), CommandId::CLOSE_TAB)));
        h.get_by_label("New session").click();
        h.run();
        assert!(h.state().out.contains(&Action::Run(CommandId::NEW_SESSION)));
    }

    #[test]
    fn inner_close_button_closes_without_selecting() {
        let mut h = harness(&["alpha", "beta"], Settings::default());
        h.get_by_label("Close 2. beta").click();
        h.run();
        assert_eq!(h.state().out, [Action::OnTab(TabId(2), CommandId::CLOSE_TAB)]);
    }

    #[test]
    fn tab_context_menu_by_keyboard_has_disabled_reconnect() {
        let mut h = harness(&["alpha"], Settings::default());
        h.get_by_label("1. alpha").focus();
        h.run();
        h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::F10);
        h.run();
        // Live tab: Reconnect is shown but disabled; Close others needs another tab.
        assert!(h.get_by_label_contains("Reconnect").accesskit_node().is_disabled());
        assert!(h.get_by_label_contains("Close others").accesskit_node().is_disabled());
        h.get_by_label_contains("Duplicate").click();
        h.run();
        assert!(h.state().out.contains(&Action::OnTab(TabId(1), CommandId::DUPLICATE_TAB)));
    }
}
