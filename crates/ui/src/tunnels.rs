//! Tunneling window: grid with start/stop, edit/delete and order buttons, Start all/Stop all, and
//! the tunnel editor with exposure warning.

use std::collections::HashMap;

use rxt_core::{Command, Metrics, Protocol, Session, Tokens, Tunnel, TunnelKind, TunnelState};

use crate::app::Action;
use crate::icons::Icons;
use crate::settings::{group, plain_box};
use crate::sidebar::SessionStore;
use crate::theme::color;
use crate::widgets::{self, icon_button};

/// Tunnel being created or edited.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TunnelEditor {
    pub tunnel: Tunnel,
    /// Name before editing; `None` for a new tunnel.
    pub original: Option<String>,
    pub error: Option<String>,
}

pub(crate) fn is_loopback(addr: &str) -> bool {
    matches!(addr.trim(), "127.0.0.1" | "::1" | "localhost")
}

impl TunnelEditor {
    /// Validate against existing tunnels and SSH sessions.
    pub(crate) fn validate(&mut self, tunnels: &[Tunnel], sessions: &[Session]) -> bool {
        let t = &mut self.tunnel;
        t.name = t.name.trim().to_owned();
        t.bind_addr = t.bind_addr.trim().to_owned();
        let duplicate = tunnels.iter().any(|o| o.name == t.name && Some(&o.name) != self.original.as_ref());
        self.error = if t.name.is_empty() {
            Some("Name is required.".into())
        } else if duplicate {
            Some(format!("A tunnel named '{}' already exists.", t.name))
        } else if !sessions.iter().any(|s| s.id == t.via && s.protocol == Protocol::Ssh) {
            Some("Choose the SSH session the tunnel goes through.".into())
        } else if t.bind_port == 0 {
            Some("Listen port is required.".into())
        } else if t.kind != TunnelKind::Dynamic && (t.dest_host.trim().is_empty() || t.dest_port == 0) {
            Some("Destination host and port are required.".into())
        } else if !is_loopback(&t.bind_addr) && !t.exposure_confirmed {
            Some("Confirm the exposure warning or use a loopback address.".into())
        } else {
            None
        };
        if is_loopback(&t.bind_addr) {
            t.exposure_confirmed = false;
        }
        self.error.is_none()
    }
}

fn kind_label(k: TunnelKind) -> &'static str {
    match k {
        TunnelKind::Local => "Local",
        TunnelKind::Remote => "Remote",
        TunnelKind::Dynamic => "Dynamic",
    }
}

/// Default and minimum size of the tunneling window (design R8).
pub(crate) const TUNNELS_SIZE: egui::Vec2 = egui::vec2(1060.0, 410.0);
pub(crate) const TUNNELS_MIN_SIZE: egui::Vec2 = egui::vec2(760.0, 300.0);
const HEADER_HEIGHT: f32 = 34.0;
const ROW_HEIGHT: f32 = 28.0;

fn running(state: Option<&TunnelState>) -> bool {
    matches!(state, Some(TunnelState::Running | TunnelState::Starting))
}

/// Start every stopped or failed tunnel; each start is independent, so one failure leaves the others.
pub(crate) fn start_all(tunnels: &[Tunnel], states: &HashMap<String, TunnelState>, out: &mut Vec<Action>) {
    for t in tunnels.iter().filter(|t| !running(states.get(&t.name))) {
        out.push(Action::StartTunnel(t.name.clone()));
    }
}

/// Stop every running or starting tunnel.
pub(crate) fn stop_all(tunnels: &[Tunnel], states: &HashMap<String, TunnelState>, out: &mut Vec<Action>) {
    for t in tunnels.iter().filter(|t| running(states.get(&t.name))) {
        out.push(Action::Engine(Command::StopTunnel { name: t.name.clone() }));
    }
}

/// Tunneling window; the editor opens as a separate window on top.
#[allow(clippy::too_many_arguments)]
pub(crate) fn window(
    ctx: &egui::Context,
    open: &mut bool,
    store: &SessionStore,
    states: &HashMap<String, TunnelState>,
    editor: &mut Option<TunnelEditor>,
    icons: &Icons,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) {
    let (tunnels, sessions) = (&store.tunnels, &store.sessions);
    let mut close = false;
    let mut x = false;
    crate::widgets::plain_window(ctx, "Tunneling")
        .resizable(true)
        .default_size(TUNNELS_SIZE)
        .min_size(TUNNELS_MIN_SIZE)
        .default_pos([110.0, 90.0])
        .show(ctx, |ui| {
            x = crate::widgets::window_title(ui, "Tunneling");
            crate::widgets::window_body(ui, |ui| {
                let m = Metrics::DEFAULT;
                egui::Panel::bottom("tunnels-buttons").frame(egui::Frame::NONE).show(ui, |ui| {
                    ui.add_space(m.space[3]);
                    ui.horizontal(|ui| {
                        if bottom_button(ui, icons, "magic-wand", "New SSH tunnel", true).clicked() {
                            let via =
                                sessions.iter().find(|s| s.protocol == Protocol::Ssh).map(|s| s.id).unwrap_or_default();
                            *editor = Some(TunnelEditor {
                                tunnel: Tunnel { via, ..Tunnel::default() },
                                original: None,
                                error: None,
                            });
                        }
                        let any = !tunnels.is_empty();
                        if bottom_button(ui, icons, "play", "Start all tunnels", any).clicked() {
                            start_all(tunnels, states, out);
                        }
                        if bottom_button(ui, icons, "stop", "Stop all tunnels", any).clicked() {
                            stop_all(tunnels, states, out);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            close = bottom_button(ui, icons, "close", "Close", true).clicked();
                        });
                    });
                });
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label("Forward ports through SSH sessions: local, remote and dynamic (SOCKS5) tunnels.");
                    });
                    ui.add_space(m.space[2]);
                    // Grid body on the input color under a panel-colored header, like a list view.
                    let grid_frame = egui::Frame::new().fill(color(tokens.input));
                    grid_frame.stroke(egui::Stroke::new(m.stroke, color(tokens.border))).show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        grid(ui, tunnels, states, sessions, icons, tokens, out);
                    });
                });
            });
        });
    if close || x {
        *open = false;
    }
    if let Some(ed) = editor {
        if !edit_window(ctx, ed, tunnels, sessions, icons, tokens, out) {
            *editor = None;
        }
    } else if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !egui::Popup::is_any_open(ctx) {
        *open = false;
    }
}

/// Bottom-row button: icon and label, wider than the 80 px dialog minimum as its label needs.
fn bottom_button(ui: &mut egui::Ui, icons: &Icons, icon: &str, label: &str, enabled: bool) -> egui::Response {
    let m = Metrics::DEFAULT;
    let b = egui::Button::opt_image_and_text(icons.image(icon, m.icon_small), Some(label.into()))
        .min_size(egui::vec2(m.dialog_button.0, m.dialog_button.1));
    ui.add_enabled(enabled, b)
}

fn grid(
    ui: &mut egui::Ui,
    tunnels: &[Tunnel],
    states: &HashMap<String, TunnelState>,
    sessions: &[Session],
    icons: &Icons,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) {
    use egui_extras::{Column, TableBuilder};
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
    let center = egui::Layout::left_to_right(egui::Align::Center).with_main_align(egui::Align::Center);
    let last = tunnels.len().saturating_sub(1);
    TableBuilder::new(ui)
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(170.0).at_least(80.0).clip(true))
        .column(Column::exact(80.0))
        .column(Column::exact(72.0))
        .column(Column::exact(96.0))
        .column(Column::remainder().at_least(110.0).clip(true))
        .column(Column::remainder().at_least(110.0).clip(true))
        .column(Column::exact(80.0))
        .column(Column::exact(72.0))
        .header(HEADER_HEIGHT, |mut row| {
            for h in
                ["Name", "Type", "Start/stop", "Forward port", "Destination server", "SSH server", "Settings", "Order"]
            {
                row.col(|ui| {
                    // Cover the column gap too: header cells form one bordered band.
                    let r = ui.max_rect().expand2(egui::vec2(ui.spacing().item_spacing.x / 2.0, 0.0));
                    let p = egui::Painter::new(ui.ctx().clone(), ui.layer_id(), r);
                    let line = egui::Stroke::new(Metrics::DEFAULT.stroke, color(tokens.border));
                    p.rect_filled(r, 0.0, color(tokens.panel));
                    p.hline(r.x_range(), r.bottom() - 0.5, line);
                    p.vline(r.right() - 0.5, r.y_range(), line);
                    ui.centered_and_justified(|ui| ui.label(h));
                });
            }
        })
        .body(|mut body| {
            for (i, t) in tunnels.iter().enumerate() {
                let state = states.get(&t.name);
                let on = running(state);
                body.row(ROW_HEIGHT, |mut row| {
                    row.col(|ui| {
                        ui.label(&t.name);
                        if let Some(TunnelState::Error(e)) = state {
                            ui.colored_label(color(tokens.danger), format!("Error: {e}")).on_hover_text(e);
                        }
                    });
                    row.col(|ui| {
                        ui.label(kind_label(t.kind));
                    });
                    row.col(|ui| {
                        ui.with_layout(center, |ui| {
                            if on {
                                if icon_button(ui, icons, "stop", &format!("Stop {}", t.name), None).clicked() {
                                    out.push(Action::Engine(Command::StopTunnel { name: t.name.clone() }));
                                }
                            } else if icon_button(ui, icons, "play", &format!("Start {}", t.name), None).clicked() {
                                out.push(Action::StartTunnel(t.name.clone()));
                            }
                        });
                    });
                    row.col(|ui| {
                        ui.label(format!("{}:{}", t.effective_bind_addr(), t.bind_port));
                    });
                    row.col(|ui| {
                        ui.label(if t.kind == TunnelKind::Dynamic {
                            "SOCKS5".into()
                        } else {
                            format!("{}:{}", t.dest_host, t.dest_port)
                        });
                    });
                    row.col(|ui| {
                        let via = sessions.iter().find(|s| s.id == t.via);
                        match via {
                            Some(s) => ui.label(&s.name),
                            None => ui.colored_label(color(tokens.text_muted), "(missing session)"),
                        };
                    });
                    row.col(|ui| {
                        ui.add_enabled_ui(!on, |ui| {
                            if icon_button(ui, icons, "edit", &format!("Edit {}", t.name), None).clicked() {
                                out.push(Action::EditTunnel(t.name.clone()));
                            }
                            if icon_button(ui, icons, "delete", &format!("Delete {}", t.name), None).clicked() {
                                out.push(Action::DeleteTunnel(t.name.clone()));
                            }
                        });
                    });
                    row.col(|ui| {
                        let name = t.name.clone();
                        let up = ui.add_enabled_ui(i > 0, |ui| {
                            icon_button(ui, icons, "arrow-up-gray", &format!("Move {} up", t.name), None)
                        });
                        if up.inner.clicked() {
                            out.push(Action::MoveTunnel { name: name.clone(), up: true });
                        }
                        let down = ui.add_enabled_ui(i < last, |ui| {
                            icon_button(ui, icons, "arrow-down", &format!("Move {} down", t.name), None)
                        });
                        if down.inner.clicked() {
                            out.push(Action::MoveTunnel { name, up: false });
                        }
                    });
                });
            }
        });
    if tunnels.is_empty() {
        crate::states::empty(
            ui,
            icons,
            Some("tunnel"),
            "No tunnels yet. Click \"New SSH tunnel\" to create one.",
            None,
        );
    }
}

/// Editor window; returns false when closed.
fn edit_window(
    ctx: &egui::Context,
    ed: &mut TunnelEditor,
    tunnels: &[Tunnel],
    sessions: &[Session],
    icons: &Icons,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) -> bool {
    let mut close = false;
    let mut keep = true;
    let title = if ed.original.is_some() { "Edit SSH tunnel" } else { "New SSH tunnel" };
    crate::widgets::plain_window(ctx, title)
        .id(egui::Id::new("tunnel-editor"))
        .resizable(false)
        .default_width(560.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            close = crate::widgets::window_title(ui, title);
            keep = crate::widgets::window_body(ui, |ui| edit(ui, ed, tunnels, sessions, icons, tokens, out));
        });
    let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !egui::Popup::is_any_open(ctx);
    !close && keep && !escape
}

/// Editor form: kind radios on top, fields grouped by side, centered Save/Cancel; false when done.
fn edit(
    ui: &mut egui::Ui,
    ed: &mut TunnelEditor,
    tunnels: &[Tunnel],
    sessions: &[Session],
    icons: &Icons,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) -> bool {
    let t = &mut ed.tunnel;
    plain_box(ui, tokens, |ui| {
        ui.horizontal(|ui| {
            ui.radio_value(&mut t.kind, TunnelKind::Local, "Local port forwarding");
            ui.radio_value(&mut t.kind, TunnelKind::Remote, "Remote port forwarding");
            ui.radio_value(&mut t.kind, TunnelKind::Dynamic, "Dynamic port forwarding (SOCKS5)");
        });
    });
    group(ui, icons, "Tunnel", |ui| {
        ui.horizontal(|ui| {
            let l = ui.label("Name");
            ui.text_edit_singleline(&mut t.name).labelled_by(l.id);
        });
        ui.checkbox(&mut t.autostart, "Start automatically with the application");
    });
    let side = match t.kind {
        TunnelKind::Remote => "Forwarded port (on the SSH server)",
        TunnelKind::Local => "Forwarded port (on this computer)",
        TunnelKind::Dynamic => "SOCKS5 port (on this computer)",
    };
    group(ui, icons, side, |ui| {
        ui.horizontal(|ui| {
            let l = ui.label("Listen address");
            ui.add(egui::TextEdit::singleline(&mut t.bind_addr).desired_width(160.0)).labelled_by(l.id);
            let l = ui.label("Listen port");
            ui.add(egui::DragValue::new(&mut t.bind_port).range(0..=65535)).labelled_by(l.id);
        });
        if !is_loopback(&t.bind_addr) {
            let net = if t.kind == TunnelKind::Remote { "the remote server's network" } else { "your local network" };
            ui.colored_label(
                color(tokens.warning),
                format!("Listening on {} exposes this tunnel to other hosts on {net}.", t.bind_addr.trim()),
            );
            ui.checkbox(&mut t.exposure_confirmed, "I understand and want to expose this tunnel");
        }
    });
    group(ui, icons, "SSH server", |ui| {
        ui.horizontal(|ui| {
            let l = ui.label("Session");
            let via = sessions.iter().find(|s| s.id == t.via).map_or("Choose…", |s| s.name.as_str());
            egui::ComboBox::from_id_salt("tunnel-via")
                .selected_text(via)
                .show_ui(ui, |ui| {
                    for s in sessions.iter().filter(|s| s.protocol == Protocol::Ssh) {
                        ui.selectable_value(&mut t.via, s.id, &s.name);
                    }
                })
                .response
                .labelled_by(l.id);
        });
    });
    if t.kind != TunnelKind::Dynamic {
        let title =
            if t.kind == TunnelKind::Remote { "Destination (from this computer)" } else { "Destination server" };
        group(ui, icons, title, |ui| {
            ui.horizontal(|ui| {
                let l = ui.label("Destination host");
                ui.add(egui::TextEdit::singleline(&mut t.dest_host).desired_width(200.0)).labelled_by(l.id);
                let l = ui.label("Destination port");
                ui.add(egui::DragValue::new(&mut t.dest_port).range(0..=65535)).labelled_by(l.id);
            });
        });
    }
    if let Some(e) = &ed.error {
        ui.colored_label(color(tokens.danger), e);
    }
    let mut keep = true;
    let m = Metrics::DEFAULT;
    ui.add_space(m.space[3]);
    match widgets::button_row(ui, icons, &[("ok", "Save", true), ("cancel", "Cancel", true)]) {
        Some(0) if ed.validate(tunnels, sessions) => {
            out.push(Action::SaveTunnel { original: ed.original.clone(), tunnel: ed.tunnel.clone() });
            keep = false;
        }
        Some(1) => keep = false,
        _ => {}
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::{NodeT, Queryable};
    use rxt_core::SessionId;

    fn ssh() -> Vec<Session> {
        let mut s = Session::new(Protocol::Ssh, "bastion", None, None);
        s.id = SessionId(7);
        vec![s]
    }

    #[test]
    fn exposure_requires_confirmation() {
        let t = Tunnel {
            name: "db".into(),
            via: SessionId(7),
            bind_addr: "0.0.0.0".into(),
            bind_port: 15432,
            dest_host: "db.internal".into(),
            dest_port: 5432,
            ..Tunnel::default()
        };
        let mut ed = TunnelEditor { tunnel: t, original: None, error: None };
        assert!(!ed.validate(&[], &ssh()));
        ed.tunnel.exposure_confirmed = true;
        assert!(ed.validate(&[], &ssh()));
    }

    #[test]
    fn duplicate_names_and_missing_session_rejected() {
        let t = Tunnel {
            name: "db".into(),
            via: SessionId(7),
            bind_port: 1,
            kind: TunnelKind::Dynamic,
            ..Tunnel::default()
        };
        let mut ed = TunnelEditor { tunnel: t.clone(), original: None, error: None };
        assert!(!ed.validate(std::slice::from_ref(&t), &ssh()));
        ed.original = Some("db".into());
        assert!(ed.validate(std::slice::from_ref(&t), &ssh()));
        assert!(!ed.validate(&[], &[]));
    }

    /// (window open, store, states, editor, emitted actions).
    type WinState = (bool, SessionStore, HashMap<String, TunnelState>, Option<TunnelEditor>, Vec<Action>);

    fn harness(tunnels: Vec<Tunnel>) -> egui_kittest::Harness<'static, WinState> {
        let store = SessionStore { sessions: ssh(), tunnels, ..SessionStore::default() };
        let mut icons = None;
        let mut h = egui_kittest::Harness::builder()
            .with_theme(egui::Theme::Light)
            .with_size(egui::vec2(1200.0, 600.0))
            .build_ui_state(
                move |ui, st: &mut WinState| {
                    let icons = icons.get_or_insert_with(|| Icons::load(ui.ctx(), None));
                    let (open, store, states, editor, out) = st;
                    if *open {
                        window(ui.ctx(), open, store, states, editor, icons, &Tokens::light(), out);
                    }
                },
                (false, store, HashMap::new(), None, Vec::new()),
            );
        crate::settings::tests::install_theme(&h.ctx);
        h.state_mut().0 = true;
        h.run();
        h
    }

    fn local(name: &str, port: u16) -> Tunnel {
        Tunnel {
            name: name.into(),
            via: SessionId(7),
            bind_port: port,
            dest_host: "db.internal".into(),
            dest_port: 5432,
            ..Tunnel::default()
        }
    }

    #[test]
    fn start_all_with_one_failing_tunnel() {
        let mut h = harness(vec![local("a", 15001), local("b", 15002), local("c", 15003)]);
        h.run();
        h.get_by_label("Start all tunnels").click();
        h.run();
        let started: Vec<_> = h
            .state()
            .4
            .iter()
            .filter_map(|a| if let Action::StartTunnel(n) = a { Some(n.clone()) } else { None })
            .collect();
        assert_eq!(started, ["a", "b", "c"]);
        // Engine answers: b fails, the others run.
        let states = &mut h.state_mut().2;
        states.insert("a".into(), TunnelState::Running);
        states.insert("b".into(), TunnelState::Error("port 15002 is in use".into()));
        states.insert("c".into(), TunnelState::Running);
        h.run();
        assert!(h.query_by_label("Stop a").is_some());
        assert!(h.query_by_label("Start b").is_some());
        assert!(h.query_by_label("Stop c").is_some());
        assert!(h.query_by_label("Error: port 15002 is in use").is_some());
        // Start all again only retries the failed one.
        h.state_mut().4.clear();
        h.get_by_label("Start all tunnels").click();
        h.run();
        assert!(matches!(h.state().4.as_slice(), [Action::StartTunnel(n)] if n == "b"));
        let size = h.get_by_role_and_label(egui::accesskit::Role::Window, "Tunneling").rect().size();
        assert!((size.x - TUNNELS_SIZE.x).abs() <= 8.0 && (size.y - TUNNELS_SIZE.y).abs() <= 8.0, "{size:?}");
    }

    #[test]
    fn empty_grid_shows_hint_and_disables_bulk_buttons() {
        let mut h = harness(Vec::new());
        h.run();
        assert!(h.query_by_label("No tunnels yet. Click \"New SSH tunnel\" to create one.").is_some());
        assert!(h.query_by_label("Destination server").is_some());
        assert!(h.get_by_label("Start all tunnels").accesskit_node().is_disabled());
        assert!(h.get_by_label("Stop all tunnels").accesskit_node().is_disabled());
        h.get_by_label("New SSH tunnel").click();
        h.run();
        assert!(h.state().3.is_some());
        assert!(h.query_by_label("Local port forwarding").is_some());
    }

    #[test]
    fn order_buttons_move_tunnels() {
        let mut h = harness(vec![local("a", 15001), local("b", 15002)]);
        h.run();
        assert!(h.get_by_label("Move a up").accesskit_node().is_disabled());
        h.get_by_label("Move a down").click();
        h.run();
        assert!(matches!(h.state().4.as_slice(), [Action::MoveTunnel { name, up: false }] if name == "a"));
    }
}
