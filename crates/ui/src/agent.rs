//! Agent control UI (design D14): approval prompts, the session editor's agent page, the
//! Settings section and the audit log window.

use egui::{Id, RichText};
use rxt_core::{ApprovalId, Tokens};
use rxt_store::db::{AgentPolicy, AuditRecord, Policy};

use crate::app::Action;
use crate::dialogs;
use crate::sftp_panel::format_time;
use crate::theme::color;

/// Audit records per page.
pub(crate) const AUDIT_PAGE: usize = 50;

/// Agent request waiting for the user's decision (`Event::ApprovalRequest`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Approval {
    pub id: ApprovalId,
    pub client: String,
    pub command: String,
    pub session: String,
    pub target: String,
    /// Unix seconds; denied automatically from then on.
    pub expires_at: i64,
}

/// Current unix time in seconds.
pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Show the oldest pending approval; `Some(approve)` once the user decided. Deny is the default
/// button (Enter); Escape denies too.
pub(crate) fn approval_ui(ctx: &egui::Context, a: &Approval, queued: usize, now: i64, tokens: &Tokens) -> Option<bool> {
    let mut answer = None;
    dialogs::dialog(ctx, Id::new(("approval", a.id.0)), "Agent request", 460.0, tokens, |ui, icons| {
        ui.label("An agent wants to run a command that changes something. Nothing runs until you approve.");
        egui::Grid::new("approval-grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label("Client");
            ui.vertical(|ui| {
                ui.strong(a.client.as_str());
                ui.label(RichText::new("Name declared by the client, not authenticated.").small());
            });
            ui.end_row();
            for (label, value) in [("Command", &a.command), ("Session", &a.session), ("Target", &a.target)] {
                ui.label(label);
                ui.label(RichText::new(value.as_str()).monospace());
                ui.end_row();
            }
        });
        let left = (a.expires_at - now).max(0);
        ui.colored_label(color(tokens.warning), format!("Denied automatically in {left} s."));
        if queued > 0 {
            ui.label(format!("{queued} more request(s) waiting."));
        }
        answer = dialogs::buttons(ui, icons, &["Approve", "Deny"], 1, true).map(|i| i == 0);
        answer.is_none()
    });
    answer
}

fn policy_label(p: AgentPolicy) -> &'static str {
    match p {
        AgentPolicy::Deny => "Deny",
        AgentPolicy::Ask => "Ask",
        AgentPolicy::Allow => "Allow",
    }
}

/// Session editor page. `policy` is `None` when it cannot be edited (`hint` says why); the
/// defaults are then shown read-only.
pub(crate) fn editor_page(ui: &mut egui::Ui, policy: Option<&mut Policy>, hint: &str, tokens: &Tokens) {
    let mut defaults = Policy::default();
    let editable = policy.is_some();
    let p = policy.unwrap_or(&mut defaults);
    ui.label("What AI agents and the rustxterm CLI may do with this session over the local control channel.");
    ui.add_space(4.0);
    ui.add_enabled_ui(editable, |ui| {
        egui::Grid::new("editor-agent").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            for (value, text) in [
                (AgentPolicy::Deny, "Agents cannot open or use this session (default)."),
                (AgentPolicy::Ask, "Every command that changes something asks you first; reads run directly."),
                (AgentPolicy::Allow, "Agents may open the session and run commands without asking."),
            ] {
                ui.radio_value(&mut p.policy, value, policy_label(value));
                ui.label(text);
                ui.end_row();
            }
        });
        ui.add_space(4.0);
        ui.checkbox(&mut p.content_read, "Agent may read content");
        ui.label(
            RichText::new("Terminal text, screenshots and file downloads. Off by default; it can contain anything shown on the remote side.")
                .small(),
        );
    });
    if !editable {
        ui.add_space(4.0);
        ui.colored_label(color(tokens.warning), hint);
    }
    ui.label(RichText::new("Agent policies can only be changed here, never by an agent or the CLI.").small());
}

/// Settings view of the agent control state.
pub(crate) struct AgentSettings<'a> {
    /// Workspace override; `None` uses `default_workspace`.
    pub workspace: &'a mut Option<String>,
    pub default_workspace: String,
}

/// "Agent control" section of the Settings window.
pub(crate) fn settings_section(ui: &mut egui::Ui, enabled: &mut bool, s: AgentSettings<'_>, out: &mut Vec<Action>) {
    ui.heading("Agent control");
    ui.checkbox(enabled, "Enable agent control (CLI live commands and MCP)");
    ui.label(
        RichText::new(
            "Uses a local socket reachable only by processes of your own user; nothing listens on the network. \
             Each session's agent policy still applies.",
        )
        .small(),
    );
    ui.horizontal(|ui| {
        let l = ui.label("Agent workspace");
        let mut text = s.workspace.clone().unwrap_or_default();
        let r = egui::TextEdit::singleline(&mut text).hint_text(s.default_workspace.as_str()).desired_width(300.0);
        if ui.add(r).labelled_by(l.id).changed() {
            *s.workspace = (!text.trim().is_empty()).then_some(text);
        }
        if ui.button("Browse…").clicked() {
            out.push(Action::BrowseAgentWorkspace);
        }
    });
    ui.label(RichText::new("Agent downloads and uploads are confined to this folder. Only changeable here.").small());
    if ui.button("Audit log…").clicked() {
        out.push(Action::ShowAudit);
    }
}

/// Audit log window state.
#[derive(Debug, Default)]
pub(crate) struct AuditView {
    pub page: usize,
    pub records: Vec<AuditRecord>,
    /// A record exists after this page.
    pub more: bool,
    pub confirm_clear: bool,
    pub error: Option<String>,
}

/// What the audit window can show.
pub(crate) enum AuditSource {
    NoDatabase,
    Locked,
    Unlocked,
}

/// Audit log window. Returns false when closed.
pub(crate) fn audit_window(
    ctx: &egui::Context,
    v: &mut AuditView,
    source: AuditSource,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) -> bool {
    let mut close = false;
    crate::widgets::plain_window(ctx, "Agent audit log").default_width(820.0).default_pos([160.0, 90.0]).show(
        ctx,
        |ui| {
            close = crate::widgets::window_title(ui, "Agent audit log");
            crate::widgets::window_body(ui, |ui| audit_source(ui, v, source, tokens, out));
        },
    );
    !close
}

fn audit_source(ui: &mut egui::Ui, v: &mut AuditView, source: AuditSource, tokens: &Tokens, out: &mut Vec<Action>) {
    match source {
        AuditSource::NoDatabase => {
            ui.label("The audit log is kept in the session database, which is not in use.");
        }
        AuditSource::Locked => {
            ui.horizontal(|ui| {
                ui.label("The session database is locked.");
                if ui.button("Unlock…").clicked() {
                    out.push(Action::ShowUnlock);
                }
            });
        }
        AuditSource::Unlocked => audit_body(ui, v, tokens, out),
    }
}

fn audit_body(ui: &mut egui::Ui, v: &mut AuditView, tokens: &Tokens, out: &mut Vec<Action>) {
    ui.label(
        RichText::new("Every agent request, newest first. Client names are as declared, not authenticated.").small(),
    );
    if let Some(e) = &v.error {
        ui.colored_label(color(tokens.danger), e.as_str());
    }
    if v.records.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new("No agent requests recorded.").color(color(tokens.text_muted)));
    } else {
        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
            egui::Grid::new("audit-grid").num_columns(7).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
                for h in ["Time (UTC)", "Client", "Entry point", "Command", "Target", "Decision", "Result"] {
                    ui.strong(h);
                }
                ui.end_row();
                for r in &v.records {
                    ui.label(format_time(r.ts));
                    ui.label(r.client.as_str());
                    ui.label(r.entry_point.as_str());
                    ui.label(RichText::new(r.command.as_str()).monospace());
                    ui.label(r.target.as_str());
                    let c = match r.decision.as_str() {
                        "denied" => tokens.danger,
                        "approved" => tokens.warning,
                        _ => tokens.text,
                    };
                    ui.colored_label(color(c), r.decision.as_str());
                    ui.label(r.result.as_str());
                    ui.end_row();
                }
            });
        });
    }
    ui.separator();
    ui.horizontal(|ui| {
        if ui.add_enabled(v.page > 0, egui::Button::new("Newer")).clicked() {
            out.push(Action::AuditPage(v.page - 1));
        }
        ui.label(format!("Page {}", v.page + 1));
        if ui.add_enabled(v.more, egui::Button::new("Older")).clicked() {
            out.push(Action::AuditPage(v.page + 1));
        }
        if ui.button("Refresh").clicked() {
            out.push(Action::AuditPage(v.page));
        }
        ui.separator();
        if v.confirm_clear {
            ui.colored_label(color(tokens.danger), "Delete every audit record?");
            if ui.button("Clear").clicked() {
                out.push(Action::ClearAudit);
            }
            if ui.button("Cancel").clicked() {
                v.confirm_clear = false;
            }
        } else if ui.add_enabled(!v.records.is_empty(), egui::Button::new("Clear log…")).clicked() {
            v.confirm_clear = true;
        }
    });
}
