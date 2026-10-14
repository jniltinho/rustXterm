//! Session editor (design R7): fixed 900×600 "Session settings" dialog with the session-type
//! tiles, the "Basic <type> settings" group, icon tabs (Advanced, Terminal, Network, Bookmark,
//! Agent) over a two-column panel with the large protocol icon, and centered OK/Cancel.

use egui::{Rect, RichText, Sense, Stroke, StrokeKind, UiBuilder, WidgetInfo, WidgetType, pos2, vec2};
use rxt_core::{
    CommandId, CursorShape, DataBits, FlowControl, Folder, FolderId, FtpTls, JumpHost, Metrics, Newline, Parity,
    PortInfo, Protocol, RdpResolution, Secret, SerialEnter, Session, StopBits, Tokens, TypeScale,
};
use rxt_store::db::Policy;

use crate::app::Action;
use crate::icons::{BUNDLED, Icons, PICKER_CATEGORIES, session_icon};
use crate::settings::{cursor_combo, scheme_combo};
use crate::theme::color;
use crate::widgets::{danger_border, describe, dialog_keys, group_box, icon_button, ok_cancel, title_row};

const M: Metrics = Metrics::DEFAULT;
/// Dialog size including its 1 px border.
pub(crate) const SIZE: egui::Vec2 = vec2(900.0, 600.0);
/// Session-type tile size.
const TILE: egui::Vec2 = vec2(56.0, 56.0);
/// Large protocol icon tile at the right of the tab panel.
const BIG_TILE: f32 = 64.0;
/// Shown when the editor opens without a session type.
pub(crate) const CHOOSE_TYPE: &str = "Choose a session type...";

/// Editor tab; `Advanced` is the protocol page of every type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    Advanced,
    Terminal,
    Network,
    Bookmark,
    Agent,
}

/// Open editor state.
#[derive(Debug, Clone)]
pub(crate) struct Editor {
    pub session: Session,
    pub is_new: bool,
    /// False until a session type is chosen (editor opened from the toolbar or menu).
    pub typed: bool,
    pub page: Page,
    pub error: Option<String>,
    /// Icon picker expanded (Bookmark tab).
    pub icon_picker: bool,
    /// Port as typed; applied to `session.port` while valid.
    pub port: String,
    /// Agent policy from the session database; `None` while not editable (unsaved, locked).
    pub policy: Option<Policy>,
    /// User names offered by the saved-credentials picker next to Username.
    pub known_users: Vec<String>,
    /// Serial ports from the last `serial.list_ports`.
    pub serial_ports: Vec<PortInfo>,
    /// Port list status ("No serial ports found.", errors).
    pub ports_note: Option<String>,
    /// Example port for the empty field, from the platform (`/dev/ttyUSB0`, `COM3`).
    pub port_hint: &'static str,
    /// The port list was requested for this editor.
    pub ports_requested: bool,
    /// The session has a saved password (Telnet auto-login).
    pub has_password: bool,
    /// Password typed for Telnet auto-login; saved in the encrypted store on OK.
    pub password: String,
}

impl Editor {
    pub(crate) fn new(session: Session, is_new: bool) -> Self {
        let port = session.port.to_string();
        Self {
            session,
            is_new,
            typed: true,
            page: Page::Advanced,
            error: None,
            icon_picker: false,
            port,
            policy: None,
            known_users: Vec::new(),
            serial_ports: Vec::new(),
            ports_note: None,
            port_hint: "/dev/ttyUSB0",
            ports_requested: false,
            has_password: false,
            password: String::new(),
        }
    }

    /// New session without a type yet: no tile selected, only the "choose a type" prompt.
    /// For `Action::NewSession` from the toolbar and menus (wired in `app.rs`).
    pub(crate) fn untyped(session: Session) -> Self {
        Self { typed: false, ..Self::new(session, true) }
    }

    /// The typed port, when it is between 1 and 65535.
    pub(crate) fn parsed_port(&self) -> Option<u16> {
        self.port.trim().parse::<u16>().ok().filter(|p| *p != 0)
    }

    /// OK is enabled: a type is chosen and the form has no blocking problem.
    pub(crate) fn can_save(&self) -> bool {
        self.typed && self.problem().is_none()
    }

    /// Blocking problem shown under the form while typing.
    pub(crate) fn problem(&self) -> Option<&'static str> {
        let s = &self.session;
        match s.protocol {
            Protocol::Serial if s.serial.port.trim().is_empty() => Some(SERIAL_PORT_ERROR),
            Protocol::Serial => None,
            _ if self.parsed_port().is_none() => Some(PORT_ERROR),
            Protocol::Telnet if s.telnet.auto_login && s.user.as_deref().is_none_or(|u| u.trim().is_empty()) => {
                Some(AUTO_LOGIN_USER_ERROR)
            }
            Protocol::Telnet if s.telnet.auto_login && !self.has_password && self.password.is_empty() => {
                Some(AUTO_LOGIN_PASSWORD_ERROR)
            }
            _ => None,
        }
    }

    /// Choose the session type; the port follows the type's default unless the user changed it.
    pub(crate) fn set_type(&mut self, p: Protocol) {
        let s = &mut self.session;
        if !self.typed || s.port == s.protocol.default_port() {
            s.port = p.default_port();
        }
        s.protocol = p;
        self.typed = true;
        self.port = s.port.to_string();
        let network = p == Protocol::Ssh;
        if (!network && self.page == Page::Network) || (!p.is_terminal() && self.page == Page::Terminal) {
            self.page = Page::Advanced;
        }
    }

    /// Check the form; fills `error` and returns false when invalid.
    pub(crate) fn validate(&mut self) -> bool {
        if self.session.protocol == Protocol::Serial {
            self.session.serial.port = self.session.serial.port.trim().to_owned();
            self.error = self.problem().map(str::to_owned);
            let s = &mut self.session;
            if self.error.is_none() && s.name.trim().is_empty() {
                s.name = s.serial.title();
            }
            return self.error.is_none();
        }
        let port = self.parsed_port();
        let problem = self.problem();
        let s = &mut self.session;
        if let Some(p) = port {
            s.port = p;
        }
        s.host = s.host.trim().to_owned();
        self.error = if s.host.is_empty() {
            Some("Remote host is required.".into())
        } else if s.host.contains(char::is_whitespace) || s.host.contains('/') {
            Some(format!("Invalid host '{}'.", s.host))
        } else if port.is_none() {
            Some(PORT_ERROR.into())
        } else if s.ssh.jump_hosts.iter().any(|j| j.host.trim().is_empty()) {
            Some("Every jump host needs a host name.".into())
        } else if s.protocol == Protocol::Vnc && s.vnc.via_ssh.as_ref().is_some_and(|j| j.host.trim().is_empty()) {
            Some("The SSH gateway needs a host name.".into())
        } else {
            problem.map(str::to_owned)
        };
        if self.error.is_none() && s.name.trim().is_empty() {
            s.name = match &s.user {
                Some(u) => format!("{u}@{}", s.host),
                None => s.host.clone(),
            };
        }
        self.error.is_none()
    }

    /// Tabs offered for the current type: (page, label, icon).
    fn pages(&self) -> Vec<(Page, String, &'static str)> {
        let p = self.session.protocol;
        let mut v = vec![(Page::Advanced, format!("Advanced {} settings", label(p)), p.icon())];
        if p.is_terminal() {
            v.push((Page::Terminal, "Terminal settings".into(), "terminal"));
        }
        if p == Protocol::Ssh {
            v.push((Page::Network, "Network settings".into(), "network-nodes"));
        }
        v.push((Page::Bookmark, "Bookmark settings".into(), "star"));
        v.push((Page::Agent, "Agent settings".into(), "rocket"));
        v
    }
}

const PORT_ERROR: &str = "Port must be between 1 and 65535.";
const SERIAL_PORT_ERROR: &str = "Choose or type a serial port.";
const AUTO_LOGIN_USER_ERROR: &str = "Auto-login needs a username.";
const AUTO_LOGIN_PASSWORD_ERROR: &str = "Auto-login needs a saved password: type it below.";
/// Telnet page warning.
pub(crate) const TELNET_WARNING: &str =
    "Telnet sends all data, including passwords, in clear text. Prefer SSH where possible.";
/// Standard serial speeds offered by the speed combo (any other value can be typed).
const BAUDS: [u32; 16] = [
    300, 1200, 2400, 4800, 9600, 19_200, 38_400, 57_600, 115_200, 230_400, 460_800, 921_600, 1_000_000, 2_000_000,
    3_000_000, 4_000_000,
];

fn label(p: Protocol) -> &'static str {
    match p {
        Protocol::Ssh => "SSH",
        Protocol::Sftp => "SFTP",
        Protocol::Ftp => "FTP",
        Protocol::Ftps => "FTPS",
        Protocol::Rdp => "RDP",
        Protocol::Vnc => "VNC",
        Protocol::Telnet => "Telnet",
        Protocol::Serial => "Serial",
    }
}

fn description(p: Protocol) -> &'static str {
    match p {
        Protocol::Ssh => "Secure Shell (SSH) session",
        Protocol::Sftp => "Secure file transfer (SFTP) session",
        Protocol::Ftp => "File transfer (FTP) session",
        Protocol::Ftps => "File transfer over TLS (FTPS) session",
        Protocol::Rdp => "Remote desktop (RDP) session",
        Protocol::Vnc => "Remote desktop (VNC) session",
        Protocol::Telnet => "Telnet or raw TCP session (unencrypted)",
        Protocol::Serial => "Serial console (COM) session",
    }
}

/// Editor window. Returns false when closed.
pub(crate) fn window(
    ctx: &egui::Context,
    ed: &mut Editor,
    folders: &[Folder],
    icons: &Icons,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) -> bool {
    // Read before showing, so open combos keep their Escape (see `dialog_keys`).
    let keys = dialog_keys(ctx);
    let frame = egui::Frame::new().fill(color(tokens.panel)).stroke(Stroke::new(M.stroke, color(tokens.border)));
    let mut result = keys.filter(|ok| !ok || ed.can_save());
    let modal = egui::Modal::new(egui::Id::new("session-editor")).frame(frame);
    modal.backdrop_color(egui::Color32::TRANSPARENT).show(ctx, |ui| {
        let inner = SIZE - vec2(2.0 * M.stroke, 2.0 * M.stroke);
        ui.set_min_size(inner);
        ui.set_max_size(inner);
        ui.spacing_mut().item_spacing.y = 0.0;
        title_row(ui, "Session settings");
        let margin = egui::Margin::symmetric(M.space[5] as i8, M.space[3] as i8);
        egui::Frame::new().inner_margin(margin).show(ui, |ui| {
            ui.set_width(ui.available_width());
            type_tiles(ui, ed, icons, tokens);
            ui.add_space(M.space[4]);
            // Error line + button row + bottom margin.
            let bottom = M.control_height + M.dialog_button.1 + M.space[3];
            let body = ui.available_height() - bottom;
            ui.allocate_ui(vec2(ui.available_width(), body), |ui| {
                ui.set_min_height(body);
                if ed.typed {
                    typed_body(ui, ed, folders, icons, tokens, out);
                } else {
                    choose_type(ui, icons);
                }
            });
            let message = ed.error.clone().or_else(|| ed.problem().filter(|_| ed.typed).map(str::to_owned));
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), M.control_height),
                egui::Layout::top_down(egui::Align::Center),
                |ui| {
                    ui.set_min_height(M.control_height);
                    if let Some(m) = message {
                        ui.add_space(M.space[1]);
                        ui.colored_label(color(tokens.danger), m);
                    }
                },
            );
            if let Some(ok) = ok_cancel(ui, icons, ed.can_save()) {
                result = Some(ok);
            }
        });
    });
    if ed.typed && ed.session.protocol == Protocol::Serial && !ed.ports_requested {
        ed.ports_requested = true;
        out.push(Action::Run(CommandId::SERIAL_LIST_PORTS));
    }
    match result {
        Some(true) if ed.validate() => {
            out.push(Action::SaveSession { session: Box::new(ed.session.clone()), connect: ed.is_new });
            let telnet = ed.session.protocol == Protocol::Telnet && ed.session.telnet.auto_login;
            if telnet && !ed.password.is_empty() {
                out.push(Action::SavePassword(Secret(std::mem::take(&mut ed.password))));
            }
            if let Some(p) = ed.policy.filter(|_| ed.session.id.0 != 0) {
                out.push(Action::SavePolicy(ed.session.id, p));
            }
            false
        }
        Some(false) => false,
        _ => true,
    }
}

/// Row of session-type tiles: 32 px icon above the label; the chosen tile is filled.
/// Left/Right move the selection along the row while no text field has focus; an untyped
/// editor starts from the first tile, so the first Right chooses the second type.
fn type_tiles(ui: &mut egui::Ui, ed: &mut Editor, icons: &Icons, tokens: &Tokens) {
    let mut tiles = Vec::with_capacity(Protocol::ALL.len());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = M.space[1];
        for p in Protocol::ALL {
            let chosen = ed.typed && ed.session.protocol == p;
            let (rect, r) = ui.allocate_exact_size(TILE, Sense::click());
            if ui.is_rect_visible(rect) {
                let fill = match (chosen, r.hovered()) {
                    (true, _) => Some(color(if tokens.dark { tokens.selection } else { tokens.pressed })),
                    (false, true) => Some(color(tokens.hover)),
                    _ => None,
                };
                if let Some(fill) = fill {
                    let stroke = color(if tokens.dark { tokens.border } else { tokens.accent });
                    ui.painter().rect(rect, 0.0, fill, Stroke::new(M.stroke, stroke), StrokeKind::Inside);
                }
                let icon = Rect::from_min_size(pos2(rect.center().x - 16.0, rect.top() + 4.0), vec2(32.0, 32.0));
                if let Some(img) = icons.image(p.icon(), M.icon_tile) {
                    img.paint_at(ui, icon);
                }
                let font = egui::FontId::proportional(TypeScale::DEFAULT.body);
                let text = color(tokens.text);
                ui.painter().text(
                    pos2(rect.center().x, rect.bottom() - 3.0),
                    egui::Align2::CENTER_BOTTOM,
                    label(p),
                    font,
                    text,
                );
            }
            r.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, chosen, label(p)));
            let r = r.on_hover_text(description(p));
            if r.clicked() {
                ed.set_type(p);
            }
            tiles.push(r);
        }
    });
    let free = ui.memory(|m| m.focused()).is_none() || tiles.iter().any(|r| r.has_focus());
    let step = |k| free && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k));
    let delta = if step(egui::Key::ArrowRight) {
        1
    } else if step(egui::Key::ArrowLeft) {
        -1
    } else {
        return;
    };
    let current = Protocol::ALL.iter().position(|p| ed.typed && *p == ed.session.protocol).unwrap_or(0);
    let next = current.saturating_add_signed(delta).min(Protocol::ALL.len() - 1);
    ed.set_type(Protocol::ALL[next]);
    if let Some(r) = tiles.get(next) {
        r.request_focus();
    }
}

/// "Choose a session type..." prompt: large icon left of heading-size muted text, centered.
fn choose_type(ui: &mut egui::Ui, icons: &Icons) {
    let font = egui::FontId::proportional(TypeScale::DEFAULT.heading);
    let muted = ui.visuals().weak_text_color();
    let galley = ui.painter().layout_no_wrap(CHOOSE_TYPE.to_owned(), font, muted);
    let width = M.icon_large + M.space[5] + galley.size().x;
    let (rect, r) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    let left = rect.center().x - width / 2.0;
    if let Some(img) = icons.image("session", M.icon_large) {
        let icon =
            Rect::from_min_size(pos2(left, rect.center().y - M.icon_large / 2.0), vec2(M.icon_large, M.icon_large));
        img.paint_at(ui, icon);
    }
    let text = pos2(left + M.icon_large + M.space[5], rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(text, galley, muted);
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, CHOOSE_TYPE));
}

/// Basic group, icon tabs and the tab panel.
fn typed_body(
    ui: &mut egui::Ui,
    ed: &mut Editor,
    folders: &[Folder],
    icons: &Icons,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) {
    let p = ed.session.protocol;
    group_box(ui, icons, Some(p.icon()), &format!("Basic {} settings", label(p)), |ui| {
        if p == Protocol::Serial { serial_basic(ui, ed, icons, out) } else { basic(ui, ed, icons, tokens) }
    });
    ui.add_space(M.space[5]);
    let tab_rect = tabs(ui, ed, icons, tokens);
    let fill = color(tokens.panel);
    let border = Stroke::new(M.stroke, color(tokens.border));
    let height = ui.available_height();
    let panel = egui::Frame::new().stroke(border).fill(fill).inner_margin(egui::Margin::same(M.space[4] as i8)).show(
        ui,
        |ui| {
            let size = vec2(ui.available_width(), height - 2.0 * (M.space[4] + M.stroke));
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            // 48 px protocol icon in a 64×64 tile, right side, vertically centered.
            let tile =
                Rect::from_center_size(pos2(rect.right() - BIG_TILE / 2.0, rect.center().y), vec2(BIG_TILE, BIG_TILE));
            ui.painter().rect(tile, 0.0, color(tokens.input), border, StrokeKind::Inside);
            if let Some(img) = icons.image(p.icon(), M.icon_large) {
                img.paint_at(ui, Rect::from_center_size(tile.center(), vec2(M.icon_large, M.icon_large)));
            }
            let left = Rect::from_min_max(rect.min, pos2(tile.left() - M.space[5], rect.max.y));
            ui.scope_builder(UiBuilder::new().max_rect(left), |ui| {
                ui.spacing_mut().item_spacing.y = M.space[1];
                egui::ScrollArea::vertical().id_salt("editor-page").auto_shrink(false).show(ui, |ui| {
                    page(ui, ed, folders, icons, tokens, out);
                });
            });
        },
    );
    // The selected tab joins the panel.
    let y = panel.response.rect.top() + M.stroke / 2.0;
    ui.painter().hline(tab_rect.left() + M.stroke..=tab_rect.right() - M.stroke, y, Stroke::new(M.stroke, fill));
}

/// Host (required), username with the saved-credentials picker and port spin box on one row.
fn basic(ui: &mut egui::Ui, ed: &mut Editor, icons: &Icons, tokens: &Tokens) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = M.space[3];
        ui.add_space(M.space[5]);
        let l = ui.label("Remote host");
        ui.add_space(-M.space[2]);
        ui.label(RichText::new("*").color(color(tokens.danger)));
        let s = &mut ed.session;
        ui.add(egui::TextEdit::singleline(&mut s.host).desired_width(200.0)).labelled_by(l.id);
        ui.add_space(M.space[6]);
        let l = ui.label("Username");
        let mut user = s.user.clone().unwrap_or_default();
        if ui.add(egui::TextEdit::singleline(&mut user).desired_width(160.0)).labelled_by(l.id).changed() {
            s.user = (!user.trim().is_empty()).then_some(user);
        }
        ui.add_space(-M.space[2]);
        let picker = icon_button(ui, icons, "users", "Saved user names", None);
        egui::Popup::menu(&picker).show(|ui| {
            if ed.known_users.is_empty() {
                ui.add_enabled(false, egui::Button::new("No saved user names"));
            }
            for u in &ed.known_users {
                if ui.button(u).clicked() {
                    ed.session.user = Some(u.clone());
                }
            }
        });
        ui.add_space(M.space[6]);
        let l = ui.label("Port");
        let r = ui.add(egui::TextEdit::singleline(&mut ed.port).desired_width(48.0)).labelled_by(l.id);
        match ed.parsed_port() {
            Some(p) => ed.session.port = p,
            None => danger_border(ui, &r, color(tokens.danger)),
        }
        ui.add_space(-M.space[3]);
        spin(ui, &mut ed.port, r.rect.height());
    });
}

/// Up/down buttons of the port spin box.
fn spin(ui: &mut egui::Ui, port: &mut String, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(14.0, height), Sense::hover());
    let current = port.trim().parse::<u32>().unwrap_or(0);
    for (i, (name, delta)) in [("Increase port", 1i64), ("Decrease port", -1)].into_iter().enumerate() {
        let half =
            Rect::from_min_size(pos2(rect.left(), rect.top() + i as f32 * height / 2.0), vec2(14.0, height / 2.0));
        let r = ui.interact(half, ui.id().with(name), Sense::click());
        let v = ui.style().interact(&r);
        ui.painter().rect(half, 0.0, v.weak_bg_fill, v.bg_stroke, StrokeKind::Inside);
        let c = half.center();
        let dy = if delta > 0 { -2.0 } else { 2.0 };
        let tip = [pos2(c.x - 3.0, c.y - dy), pos2(c.x + 3.0, c.y - dy), pos2(c.x, c.y + dy)];
        ui.painter().add(egui::Shape::convex_polygon(tip.to_vec(), v.fg_stroke.color, Stroke::NONE));
        if describe(r, name, None).clicked() {
            *port = (i64::from(current) + delta).clamp(1, 65535).to_string();
        }
    }
}

/// Icon tab row; returns the selected tab's rectangle.
fn tabs(ui: &mut egui::Ui, ed: &mut Editor, icons: &Icons, tokens: &Tokens) -> Rect {
    let border = Stroke::new(M.stroke, color(tokens.border));
    let mut selected = Rect::NOTHING;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.spacing_mut().button_padding = vec2(M.space[3], M.space[1]);
        for (p, text, icon) in ed.pages() {
            let on = ed.page == p;
            let fill = color(if on { tokens.panel } else { tokens.toolbar });
            let b = egui::Button::opt_image_and_text(icons.image(icon, M.icon_small), Some(text.clone().into()))
                .fill(fill)
                .stroke(border)
                .min_size(vec2(0.0, crate::widgets::TITLE_H));
            let r = ui.add(b);
            r.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, on, &text));
            if on {
                selected = r.rect;
            }
            if r.clicked() {
                ed.page = p;
                ed.icon_picker = false;
            }
        }
    });
    selected
}

fn page(ui: &mut egui::Ui, ed: &mut Editor, folders: &[Folder], icons: &Icons, tokens: &Tokens, out: &mut Vec<Action>) {
    if ed.icon_picker {
        return icon_picker(ui, &mut ed.session, icons, &mut ed.icon_picker);
    }
    let hint = if ed.session.id.0 == 0 {
        "Save the session first to change its agent policy."
    } else {
        "Unlock the session database to change the agent policy."
    };
    let port = ed.session.port;
    let s = &mut ed.session;
    match (ed.page, s.protocol) {
        (Page::Agent, _) => crate::agent::editor_page(ui, ed.policy.as_mut(), hint, tokens),
        (Page::Bookmark, _) => bookmark(ui, s, folders, icons, &mut ed.icon_picker),
        (Page::Network, _) => network(ui, s, icons),
        (Page::Terminal, _) => terminal(ui, s),
        (_, Protocol::Ssh) => advanced(ui, s, out),
        (_, Protocol::Sftp) => sftp(ui, s, out),
        (_, Protocol::Ftp | Protocol::Ftps) => ftp(ui, s, tokens),
        (_, Protocol::Rdp) => rdp(ui, s),
        (_, Protocol::Vnc) => vnc(ui, s),
        (_, Protocol::Telnet) => telnet(ui, s, &mut ed.password, ed.has_password, tokens),
        (_, Protocol::Serial) => serial(ui, s),
    }
    // FTPS TLS modes move the default port.
    if ed.session.port != port {
        ed.port = ed.session.port.to_string();
    }
}

/// Session name, folder and icon: edited here only.
fn bookmark(ui: &mut egui::Ui, s: &mut Session, folders: &[Folder], icons: &Icons, picker: &mut bool) {
    grid(ui, "editor-bookmark", |ui| {
        text_row(ui, "Session name", &mut s.name);
        let l = ui.label("Folder");
        let name = |id: Option<FolderId>| {
            id.and_then(|id| folders.iter().find(|f| f.id == id)).map_or("User sessions".to_owned(), |f| f.name.clone())
        };
        let r = egui::ComboBox::from_id_salt("editor-folder").width(320.0).selected_text(name(s.folder)).show_ui(
            ui,
            |ui| {
                ui.selectable_value(&mut s.folder, None, "User sessions");
                for f in folders {
                    ui.selectable_value(&mut s.folder, Some(f.id), &f.name);
                }
            },
        );
        r.response.labelled_by(l.id);
        ui.end_row();
        ui.label("Session icon");
        ui.horizontal(|ui| {
            let img = icons.image(session_icon(s), M.icon_small);
            if ui.add(egui::Button::opt_image_and_text(img, Some("Choose icon…".into()))).clicked() {
                *picker = true;
            }
            if s.icon.is_some() && ui.button("Use protocol icon").clicked() {
                s.icon = None;
            }
        });
        ui.end_row();
    });
}

/// Session icon grid grouped by catalog category.
fn icon_picker(ui: &mut egui::Ui, s: &mut Session, icons: &Icons, open: &mut bool) {
    if ui.button("Hide icons").clicked() {
        *open = false;
    }
    for category in PICKER_CATEGORIES {
        let mut title = category.to_owned();
        title[..1].make_ascii_uppercase();
        ui.strong(title);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
            for (name, ..) in BUNDLED.iter().filter(|b| b.1 == category) {
                let Some(img) = icons.image(name, 32.0) else { continue };
                let selected = s.icon.as_deref() == Some(*name);
                let r = ui.add(egui::Button::image(img).selected(selected));
                r.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, selected, *name));
                if r.on_hover_text(*name).clicked() {
                    s.icon = Some((*name).to_owned());
                    *open = false;
                }
            }
        });
    }
}

/// Two-column (label, control) grid used by every page.
fn grid<R>(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Grid::new(id).num_columns(2).spacing([M.space[4], M.space[2]]).show(ui, add).inner
}

/// Checkboxes laid out two per row.
fn checks<const N: usize>(ui: &mut egui::Ui, id: &str, items: [(&mut bool, &str); N]) {
    ui.add_space(M.space[2]);
    egui::Grid::new(id).num_columns(2).spacing([M.space[6], M.space[2]]).show(ui, |ui| {
        for (i, (value, text)) in items.into_iter().enumerate() {
            ui.checkbox(value, text);
            if i % 2 == 1 {
                ui.end_row();
            }
        }
    });
}

fn advanced(ui: &mut egui::Ui, s: &mut Session, out: &mut Vec<Action>) {
    let o = &mut s.ssh;
    grid(ui, "editor-ssh", |ui| {
        key_file_row(ui, &mut o.key_file, out);
        opt_text_row(ui, "Execute command", &mut o.remote_command);
        opt_text_row(ui, "Startup directory", &mut o.startup_dir);
        let l = ui.label("Keepalive (seconds, 0 = off)");
        ui.add(egui::DragValue::new(&mut o.keepalive_secs).range(0..=3600)).labelled_by(l.id);
        ui.end_row();
        let monitoring = [(None, "Use global setting"), (Some(true), "On"), (Some(false), "Off")];
        combo(ui, "Remote monitoring", &mut o.remote_monitoring, &monitoring);
    });
    checks(
        ui,
        "editor-ssh-checks",
        [
            (&mut o.keep_open_after_command, "Do not exit after command ends"),
            (&mut o.use_agent, "Use SSH agent"),
            (&mut o.allow_keyboard_interactive, "Allow keyboard-interactive authentication"),
            (&mut o.allow_password, "Allow password authentication"),
            (&mut o.x11_forwarding, "X11 forwarding"),
            (&mut o.agent_forwarding, "Agent forwarding"),
            (&mut o.compression, "Compression"),
            (&mut o.follow_terminal_folder, "SFTP browser follows terminal folder"),
            (&mut o.auto_reconnect, "Reconnect automatically"),
        ],
    );
}

fn key_file_row(ui: &mut egui::Ui, key_file: &mut Option<String>, out: &mut Vec<Action>) {
    let l = ui.label("Private key file");
    ui.horizontal(|ui| {
        let mut key = key_file.clone().unwrap_or_default();
        if ui.add(egui::TextEdit::singleline(&mut key).desired_width(300.0)).labelled_by(l.id).changed() {
            *key_file = (!key.trim().is_empty()).then_some(key);
        }
        if ui.button("Browse…").clicked() {
            out.push(Action::BrowseKeyFile);
        }
    });
    ui.end_row();
}

fn sftp(ui: &mut egui::Ui, s: &mut Session, out: &mut Vec<Action>) {
    let o = &mut s.ssh;
    grid(ui, "editor-sftp", |ui| key_file_row(ui, &mut o.key_file, out));
    checks(
        ui,
        "editor-sftp-checks",
        [
            (&mut o.use_agent, "Use SSH agent"),
            (&mut o.allow_keyboard_interactive, "Allow keyboard-interactive authentication"),
            (&mut o.allow_password, "Allow password authentication"),
        ],
    );
}

fn ftp(ui: &mut egui::Ui, s: &mut Session, tokens: &Tokens) {
    let o = &mut s.ftp;
    grid(ui, "editor-ftp", |ui| {
        if s.protocol == Protocol::Ftps {
            ui.label("TLS mode");
            ui.horizontal(|ui| {
                let before = o.tls;
                ui.radio_value(&mut o.tls, FtpTls::Explicit, "Explicit (AUTH TLS)");
                ui.radio_value(&mut o.tls, FtpTls::Implicit, "Implicit");
                // Follow the mode's default port unless the user chose another one.
                let (old, new) = (tls_port(before), tls_port(o.tls));
                if before != o.tls && s.port == old {
                    s.port = new;
                }
            });
            ui.end_row();
        }
        ui.label("Data connection");
        ui.horizontal(|ui| {
            ui.radio_value(&mut o.passive, true, "Passive");
            ui.radio_value(&mut o.passive, false, "Active");
        });
        ui.end_row();
        ui.label("Transfer type");
        ui.horizontal(|ui| {
            ui.radio_value(&mut o.ascii, false, "Binary");
            ui.radio_value(&mut o.ascii, true, "ASCII");
        });
        ui.end_row();
        opt_text_row(ui, "Startup folder", &mut o.startup_folder);
    });
    if s.protocol == Protocol::Ftp {
        ui.add_space(M.space[2]);
        ui.colored_label(
            color(tokens.warning),
            "Plain FTP sends the password and files unencrypted. Prefer FTPS or SFTP.",
        );
    }
}

/// Default control port of an FTPS TLS mode.
pub(crate) fn tls_port(tls: FtpTls) -> u16 {
    match tls {
        FtpTls::Explicit => Protocol::Ftp.default_port(),
        FtpTls::Implicit => Protocol::Ftps.default_port(),
    }
}

/// Keyboard layouts offered for RDP (Windows layout ids).
const LAYOUTS: [(u32, &str); 9] = [
    (0, "Server default"),
    (0x0409, "English (US)"),
    (0x0809, "English (UK)"),
    (0x0407, "German"),
    (0x040c, "French"),
    (0x040a, "Spanish"),
    (0x0410, "Italian"),
    (0x0416, "Portuguese (Brazil)"),
    (0x0816, "Portuguese (Portugal)"),
];

fn rdp(ui: &mut egui::Ui, s: &mut Session) {
    let o = &mut s.rdp;
    grid(ui, "editor-rdp", |ui| {
        opt_text_row(ui, "Domain", &mut o.domain);
        ui.label("Resolution");
        ui.horizontal(|ui| {
            let fixed = matches!(o.resolution, RdpResolution::Fixed { .. });
            if ui.radio(!fixed, "Fit to tab").clicked() {
                o.resolution = RdpResolution::FitToTab;
            }
            if ui.radio(fixed, "Fixed").clicked() && !fixed {
                o.resolution = RdpResolution::Fixed { width: 1280, height: 800 };
            }
            if let RdpResolution::Fixed { width, height } = &mut o.resolution {
                ui.add(egui::DragValue::new(width).range(200..=8192)).on_hover_text("Width");
                ui.label("×");
                ui.add(egui::DragValue::new(height).range(200..=8192)).on_hover_text("Height");
            }
        });
        ui.end_row();
        let l = ui.label("Keyboard layout");
        let name = LAYOUTS.iter().find(|l| l.0 == o.keyboard_layout).map_or("Custom", |l| l.1);
        let r = egui::ComboBox::from_id_salt("editor-rdp-layout").selected_text(name).show_ui(ui, |ui| {
            for (id, text) in LAYOUTS {
                ui.selectable_value(&mut o.keyboard_layout, id, text);
            }
        });
        r.response.labelled_by(l.id);
        ui.end_row();
    });
    checks(
        ui,
        "editor-rdp-checks",
        [
            (&mut o.clipboard, "Share clipboard text"),
            (&mut o.allow_legacy_tls, "Allow legacy TLS 1.0/1.1 (old Windows servers, less secure)"),
        ],
    );
}

fn vnc(ui: &mut egui::Ui, s: &mut Session) {
    let o = &mut s.vnc;
    let mut via = o.via_ssh.is_some();
    checks(
        ui,
        "editor-vnc-checks",
        [
            (&mut o.view_only, "View only (send no keyboard or mouse input)"),
            (&mut o.scale_to_fit, "Scale to fit the tab"),
            (&mut o.clipboard, "Share clipboard text"),
            (&mut via, "Connect through an SSH gateway"),
        ],
    );
    if via != o.via_ssh.is_some() {
        o.via_ssh = via.then(|| JumpHost { port: 22, ..JumpHost::default() });
    }
    if let Some(j) = &mut o.via_ssh {
        ui.add_space(M.space[3]);
        grid(ui, "editor-vnc-ssh", |ui| {
            text_row(ui, "Gateway host", &mut j.host);
            let l = ui.label("Gateway port");
            ui.add(egui::DragValue::new(&mut j.port).range(1..=65535)).labelled_by(l.id);
            ui.end_row();
            text_row(ui, "Gateway user", &mut j.user);
            opt_text_row(ui, "Gateway key file", &mut j.key_file);
        });
    }
}

/// Serial port (free text plus the discovered list and a refresh button) and speed.
fn serial_basic(ui: &mut egui::Ui, ed: &mut Editor, icons: &Icons, out: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = M.space[3];
        ui.add_space(M.space[5]);
        let l = ui.label("Serial port");
        let o = &mut ed.session.serial;
        ui.add(egui::TextEdit::singleline(&mut o.port).hint_text(ed.port_hint).desired_width(200.0)).labelled_by(l.id);
        ui.add_space(-M.space[2]);
        let picker = icon_button(ui, icons, "plug", "Detected serial ports", None);
        egui::Popup::menu(&picker).show(|ui| {
            if let Some(note) = &ed.ports_note {
                ui.add_enabled(false, egui::Button::new(note.as_str()));
            }
            for p in &ed.serial_ports {
                let desc = p.description.as_deref().or(p.by_id.as_deref()).unwrap_or_default();
                let text = if desc.is_empty() { p.path.clone() } else { format!("{}  ({desc})", p.path) };
                if ui.button(text).clicked() {
                    o.port = p.path.clone();
                }
            }
        });
        ui.add_space(-M.space[2]);
        if icon_button(ui, icons, "refresh", "Refresh serial ports", None).clicked() {
            out.push(Action::Run(CommandId::SERIAL_LIST_PORTS));
        }
        ui.add_space(M.space[6]);
        let l = ui.label("Speed (bps)");
        let r = egui::ComboBox::from_id_salt("editor-serial-baud").width(100.0).selected_text(o.line.baud.to_string());
        r.show_ui(ui, |ui| {
            for b in BAUDS {
                ui.selectable_value(&mut o.line.baud, b, b.to_string());
            }
        })
        .response
        .labelled_by(l.id);
        let custom = ui.add(egui::DragValue::new(&mut o.line.baud).range(50..=12_000_000).speed(100));
        custom.widget_info(|| WidgetInfo::labeled(WidgetType::DragValue, true, "Custom speed"));
    });
}

fn combo<T: PartialEq + Copy>(ui: &mut egui::Ui, text: &str, value: &mut T, items: &[(T, &str)]) {
    let l = ui.label(text);
    let current = items.iter().find(|(v, _)| v == value).map_or("", |(_, t)| *t);
    let r = egui::ComboBox::from_id_salt(("editor-combo", text)).selected_text(current).show_ui(ui, |ui| {
        for (v, t) in items {
            ui.selectable_value(value, *v, *t);
        }
    });
    r.response.labelled_by(l.id);
    ui.end_row();
}

fn telnet(ui: &mut egui::Ui, s: &mut Session, password: &mut String, has_password: bool, tokens: &Tokens) {
    let o = &mut s.telnet;
    ui.colored_label(color(tokens.warning), TELNET_WARNING);
    ui.add_space(M.space[2]);
    grid(ui, "editor-telnet", |ui| {
        text_row(ui, "Terminal type", &mut o.terminal_type);
        let newlines = [(Newline::CrLf, "CR LF"), (Newline::CrNul, "CR NUL"), (Newline::Cr, "CR")];
        combo(ui, "Enter sends", &mut o.newline, &newlines);
    });
    checks(
        ui,
        "editor-telnet-checks",
        [(&mut o.raw, "Raw TCP (no Telnet negotiation)"), (&mut o.auto_login, "Log in automatically")],
    );
    if o.auto_login {
        ui.add_space(M.space[2]);
        grid(ui, "editor-telnet-login", |ui| {
            text_row(ui, "Login prompt", &mut o.login_prompt);
            text_row(ui, "Password prompt", &mut o.password_prompt);
            let l = ui.label("Password");
            let hint = if has_password { "saved (type to replace)" } else { "required" };
            let field = egui::TextEdit::singleline(password).password(true).hint_text(hint).desired_width(320.0);
            ui.add(field).labelled_by(l.id);
            ui.end_row();
        });
        ui.label(
            RichText::new("The password is kept in the encrypted session database and typed only at the prompts.")
                .color(ui.visuals().weak_text_color()),
        );
    }
}

fn serial(ui: &mut egui::Ui, s: &mut Session) {
    let o = &mut s.serial;
    grid(ui, "editor-serial", |ui| {
        let data: Vec<(DataBits, String)> = DataBits::ALL.iter().map(|d| (*d, d.bits().to_string())).collect();
        let data: Vec<(DataBits, &str)> = data.iter().map(|(d, t)| (*d, t.as_str())).collect();
        combo(ui, "Data bits", &mut o.line.data_bits, &data);
        let parity = [
            (Parity::None, "None"),
            (Parity::Odd, "Odd"),
            (Parity::Even, "Even"),
            (Parity::Mark, "Mark"),
            (Parity::Space, "Space"),
        ];
        combo(ui, "Parity", &mut o.line.parity, &parity);
        combo(ui, "Stop bits", &mut o.line.stop_bits, &[(StopBits::One, "1"), (StopBits::Two, "2")]);
        let flows: Vec<(FlowControl, &str)> = FlowControl::ALL.iter().map(|f| (*f, f.label())).collect();
        combo(ui, "Flow control", &mut o.line.flow, &flows);
        let enters = [(SerialEnter::Cr, "CR"), (SerialEnter::Lf, "LF"), (SerialEnter::CrLf, "CR LF")];
        combo(ui, "Enter sends", &mut o.enter, &enters);
        let lines = [(None, "Driver default"), (Some(true), "On"), (Some(false), "Off")];
        combo(ui, "DTR at open", &mut o.dtr, &lines);
        combo(ui, "RTS at open", &mut o.rts, &lines);
    });
    checks(
        ui,
        "editor-serial-checks",
        [
            (&mut o.lf_to_crlf, "Show received LF as CR LF"),
            (&mut o.local_echo, "Local echo"),
            (&mut o.reconnect, "Reconnect when the device returns"),
        ],
    );
}

fn network(ui: &mut egui::Ui, s: &mut Session, icons: &Icons) {
    let o = &mut s.ssh;
    group_box(ui, icons, Some("network-nodes"), "SSH gateway (jump hosts)", |ui| {
        let mut remove = None;
        // Fixed-width rows line up like a grid (a Grid would shrink the fields to last frame's column).
        for (i, j) in o.jump_hosts.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                hint(ui, &mut j.host, "Host", 180.0);
                ui.add_sized([56.0, M.control_height], egui::DragValue::new(&mut j.port).range(1..=65535));
                hint(ui, &mut j.user, "User", 120.0);
                let mut key = j.key_file.clone().unwrap_or_default();
                if hint(ui, &mut key, "Key file", 200.0).changed() {
                    j.key_file = (!key.trim().is_empty()).then_some(key);
                }
                if icon_button(ui, icons, "delete", "Remove jump host", None).clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            o.jump_hosts.remove(i);
        }
        if ui.button("Add jump host").clicked() {
            o.jump_hosts.push(JumpHost { port: 22, ..JumpHost::default() });
        }
    });
    ui.add_space(M.space[4]);
    group_box(ui, icons, None, "Environment variables", |ui| {
        let mut remove = None;
        for (i, (k, v)) in o.env.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                hint(ui, k, "Name", 180.0);
                hint(ui, v, "Value", 300.0);
                if icon_button(ui, icons, "delete", "Remove variable", None).clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            o.env.remove(i);
        }
        if ui.button("Add variable").clicked() {
            o.env.push((String::new(), String::new()));
        }
    });
}

fn terminal(ui: &mut egui::Ui, s: &mut Session) {
    let t = &mut s.terminal;
    ui.label("Checked values override the global terminal defaults for this session.");
    ui.add_space(M.space[2]);
    grid(ui, "editor-term", |ui| {
        override_row(
            ui,
            "Font family",
            &mut t.font_family,
            || "monospace".into(),
            |ui, v| {
                let mut family = Some(std::mem::take(v));
                crate::settings::font_combo(ui, "editor-font", &mut family);
                *v = family.unwrap_or_default();
            },
        );
        override_row(
            ui,
            "Font size",
            &mut t.font_size,
            || 13.0,
            |ui, v| {
                ui.add(egui::DragValue::new(v).range(6.0..=48.0).speed(0.5));
            },
        );
        override_row(ui, "Cursor shape", &mut t.cursor_shape, CursorShape::default, |ui, v| {
            cursor_combo(ui, "editor-cursor", v);
        });
        override_row(
            ui,
            "Blinking cursor",
            &mut t.cursor_blink,
            || true,
            |ui, v| {
                ui.checkbox(v, "");
            },
        );
        override_row(
            ui,
            "Color scheme",
            &mut t.scheme,
            || "rustxterm-dark".into(),
            |ui, v| {
                let mut opt = Some(v.clone());
                scheme_combo(ui, "editor-scheme", &mut opt);
                if let Some(n) = opt {
                    *v = n;
                }
            },
        );
        override_row(
            ui,
            "Scrollback lines",
            &mut t.scrollback_lines,
            || 10_000,
            |ui, v| {
                ui.add(egui::DragValue::new(v).range(0..=1_000_000).speed(100));
            },
        );
    });
}

fn override_row<T>(
    ui: &mut egui::Ui,
    text: &str,
    value: &mut Option<T>,
    default: impl FnOnce() -> T,
    edit: impl FnOnce(&mut egui::Ui, &mut T),
) {
    let mut on = value.is_some();
    if ui.checkbox(&mut on, text).changed() {
        *value = on.then(default);
    }
    match value {
        Some(v) => edit(ui, v),
        None => {
            ui.label(RichText::new("Global default").color(ui.visuals().weak_text_color()));
        }
    }
    ui.end_row();
}

fn text_row(ui: &mut egui::Ui, text: &str, value: &mut String) {
    let l = ui.label(text);
    ui.add(egui::TextEdit::singleline(value).desired_width(320.0)).labelled_by(l.id);
    ui.end_row();
}

fn opt_text_row(ui: &mut egui::Ui, text: &str, value: &mut Option<String>) {
    let l = ui.label(text);
    let mut v = value.clone().unwrap_or_default();
    if ui.add(egui::TextEdit::singleline(&mut v).desired_width(320.0)).labelled_by(l.id).changed() {
        *value = (!v.trim().is_empty()).then_some(v);
    }
    ui.end_row();
}

fn hint(ui: &mut egui::Ui, value: &mut String, text: &str, width: f32) -> egui::Response {
    let r = ui.add_sized([width, M.control_height], egui::TextEdit::singleline(value).hint_text(text));
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, text));
    r
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};

    use super::*;

    #[test]
    fn validate_requires_host_and_names_session() {
        let mut ed = Editor::new(Session::default(), true);
        assert!(!ed.validate());
        ed.session.host = " lab.example ".into();
        ed.session.user = Some("alice".into());
        ed.port = "70000".into();
        assert!(!ed.validate());
        ed.port = "22".into();
        assert!(ed.validate());
        assert_eq!(ed.session.name, "alice@lab.example");
        ed.session.ssh.jump_hosts.push(JumpHost::default());
        assert!(!ed.validate());
    }

    #[test]
    fn vnc_gateway_needs_host_and_ftps_ports_follow_tls() {
        let mut ed = Editor::new(Session::new(Protocol::Vnc, "kiosk", None, None), true);
        ed.session.vnc.via_ssh = Some(JumpHost::default());
        assert!(!ed.validate());
        assert_eq!((tls_port(FtpTls::Explicit), tls_port(FtpTls::Implicit)), (21, 990));
        let q = rxt_core::quick_connect::parse("ftps://files.example").unwrap();
        assert_eq!((q.ftp.tls, q.port), (FtpTls::Implicit, 990));
    }

    struct State {
        ed: Option<Editor>,
        icons: Icons,
        out: Vec<Action>,
        tokens: Tokens,
        ready: bool,
    }

    fn harness(ed: Editor, tokens: Tokens) -> Harness<'static, State> {
        let mut h = Harness::builder().with_size(vec2(1280.0, 800.0)).build_ui_state(
            |ui, s: &mut State| {
                // Wait for the bundled fonts (bound after the harness's first frame).
                if !s.ready {
                    return;
                }
                let ctx = ui.ctx().clone();
                let theme = if s.tokens.dark { egui::Theme::Dark } else { egui::Theme::Light };
                ctx.set_style_of(theme, crate::theme::style(&s.tokens));
                ctx.set_theme(theme);
                if let Some(ed) = &mut s.ed
                    && !window(&ctx, ed, &[], &s.icons, &s.tokens, &mut s.out)
                {
                    s.ed = None;
                }
            },
            State { ed: Some(ed), icons: Icons::default(), out: Vec::new(), tokens, ready: false },
        );
        h.ctx.set_fonts(crate::theme::fonts());
        h.state_mut().icons = Icons::load(&h.ctx.clone(), None);
        h.state_mut().ready = true;
        h.run_steps(4);
        h
    }

    /// First node with this label (a field labelled by a label shares its name).
    fn rect(h: &Harness<'_, State>, label: &str) -> Rect {
        h.query_all_by_label(label).next().map(|n| n.rect()).unwrap_or_else(|| panic!("no {label}"))
    }

    fn has(h: &Harness<'_, State>, label: &str) -> bool {
        h.query_all_by_label(label).next().is_some()
    }

    #[test]
    fn untyped_editor_prompts_then_ssh_gives_port_22() {
        let mut h = harness(Editor::untyped(Session { port: 0, ..Session::default() }), Tokens::light());
        assert!(has(&h, CHOOSE_TYPE));
        assert!(!has(&h, "Remote host"), "only the prompt below the tiles");
        let toggled = |h: &Harness<'_, State>, l: &str| h.get_by_label(l).accesskit_node().toggled();
        assert!(Protocol::ALL.iter().all(|p| toggled(&h, label(*p)) == Some(egui::accesskit::Toggled::False)));
        assert!(h.get_by_label("OK").accesskit_node().is_disabled(), "OK needs a type");
        h.get_by_label("SSH").click();
        h.run_steps(4);
        let ed = h.state().ed.as_ref().unwrap();
        assert!(ed.typed);
        assert_eq!((ed.session.port, ed.port.as_str()), (22, "22"));
        assert!(!has(&h, CHOOSE_TYPE));
        assert!(has(&h, "Basic SSH settings"));
        assert_eq!(toggled(&h, "SSH"), Some(egui::accesskit::Toggled::True));
    }

    #[test]
    fn dialog_size_one_row_basic_and_centered_buttons() {
        for tokens in [Tokens::light(), Tokens::dark()] {
            let h = harness(Editor::new(Session::new(Protocol::Ssh, "web1", None, None), true), tokens);
            let dialog = rect(&h, "Session settings");
            let (ok, cancel) = (rect(&h, "OK"), rect(&h, "Cancel"));
            assert_eq!((ok.size(), cancel.size()), (vec2(80.0, 30.0), vec2(80.0, 30.0)));
            // The dialog spans 900×600 centered on the 1280×800 window.
            let center = 640.0;
            assert!((dialog.left() - (center - 450.0)).abs() <= 8.0, "dialog left {dialog:?}");
            assert!(((ok.left() + cancel.right()) / 2.0 - center).abs() <= 2.0, "buttons centered");
            let ys: Vec<f32> = ["Remote host", "Username", "Port"].iter().map(|l| rect(&h, l).center().y).collect();
            let fields: Vec<f32> =
                h.query_all_by_role(egui::accesskit::Role::TextInput).take(3).map(|n| n.rect().center().y).collect();
            for y in ys.iter().chain(&fields) {
                assert!((y - ys[0]).abs() <= 1.0, "one row: {ys:?} {fields:?}");
            }
        }
    }

    #[test]
    fn invalid_port_disables_ok_and_paints_danger_border() {
        let mut ed = Editor::new(Session::new(Protocol::Ssh, "web1", None, None), true);
        ed.port = "70000".into();
        let mut h = harness(ed, Tokens::light());
        assert!(has(&h, PORT_ERROR));
        assert!(h.get_by_label("OK").accesskit_node().is_disabled());
        h.key_press(egui::Key::Enter);
        h.run_steps(2);
        assert!(h.state().out.is_empty() && h.state().ed.is_some(), "Enter does not save");
        h.get_by_label("Increase port").click();
        h.run_steps(2);
        assert_eq!(h.state().ed.as_ref().unwrap().port, "65535", "spin clamps");
        h.key_press(egui::Key::Enter);
        h.run_steps(2);
        assert!(h.state().ed.is_none(), "Enter saves once valid");
        assert!(matches!(h.state().out.first(), Some(Action::SaveSession { .. })));
    }

    #[test]
    fn danger_border_pixels() {
        let mut ed = Editor::new(Session::new(Protocol::Ssh, "web1", None, None), true);
        ed.port = "0".into();
        let tokens = Tokens::light();
        let mut h = harness(ed, tokens.clone());
        let field = h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Port").rect();
        let img = h.render().expect("render");
        let d = tokens.danger;
        let x = field.center().x as u32;
        let hit = (field.top() as u32 - 1..=field.top() as u32 + 1).any(|y| {
            let px = img.get_pixel(x, y);
            (px[0], px[1], px[2]) == (d.0, d.1, d.2)
        });
        assert!(hit, "danger border at {field:?}");
    }

    #[test]
    fn bookmark_tab_holds_name_folder_icon_only() {
        let mut h = harness(Editor::new(Session::new(Protocol::Rdp, "win", None, None), false), Tokens::dark());
        for l in ["Session name", "Folder", "Session icon"] {
            assert!(!has(&h, l), "{l} not on the Advanced tab");
        }
        assert!(!has(&h, "Network settings"), "RDP has no SSH network tab");
        h.get_by_label("Bookmark settings").click();
        h.run_steps(4);
        for l in ["Session name", "Folder", "Session icon"] {
            assert!(has(&h, l), "{l} on the Bookmark tab");
        }
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        assert!(h.state().ed.is_none() && h.state().out.is_empty(), "Escape cancels");
    }

    #[test]
    fn network_tab_groups_jump_hosts() {
        let mut ed = Editor::new(Session::new(Protocol::Ssh, "web1", None, None), true);
        ed.page = Page::Network;
        let mut h = harness(ed, Tokens::light());
        assert!(h.query_by_label("SSH gateway (jump hosts)").is_some());
        h.get_by_label("Add jump host").click();
        h.run_steps(2);
        assert_eq!(h.state().ed.as_ref().unwrap().session.ssh.jump_hosts.len(), 1);
        h.get_by_label("Remove jump host").click();
        h.run_steps(2);
        assert!(h.state().ed.as_ref().unwrap().session.ssh.jump_hosts.is_empty());
    }
}
