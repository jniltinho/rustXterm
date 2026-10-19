//! Settings → Diagnostics (add-diagnostic-logging): the diagnostic log level and folder, applied
//! live through the binary's [`DiagnosticsControl`], and the data-stream switch, which needs
//! consent each time and is never saved.

use std::path::{Path, PathBuf};

use rxt_core::diag::{DiagLevel, DiagSettings, TRACE_PROTOCOLS};
use rxt_core::{Metrics, Tokens};

use crate::app::Action;
use crate::icons::Icons;
use crate::settings::group;
use crate::theme::color;

/// The binary's diagnostic log (it owns the tracing subscriber).
pub trait DiagnosticsControl {
    /// Applies the settings at once and reports where the log goes. Never waits for the disk: an
    /// open or write failure shows up later through [`Self::poll_error`].
    fn apply(&self, settings: &DiagSettings) -> DiagState;
    /// The writer's last error, cleared by reading it (polled every frame).
    fn poll_error(&self) -> Option<String> {
        None
    }
    /// Waits until the queued log lines are written (before clearing files, and at exit).
    fn flush(&self) {}
}

/// Result of [`DiagnosticsControl::apply`], shown on the page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiagState {
    /// Absolute `rustxterm-logs` folder in use.
    pub dir: Option<PathBuf>,
    /// The diagnostic log file being written; `None` when Off.
    pub file: Option<PathBuf>,
    /// Why the folder fell back to the config folder.
    pub note: Option<String>,
    pub error: Option<String>,
    /// `--debug-log` decides the file and level for this run.
    pub forced: bool,
}

/// Data-stream switch state for this run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StreamUi {
    pub on: bool,
    pub include_input: bool,
    /// The consent box is showing.
    pub asking: bool,
    /// "Clear logs" waits for its confirmation.
    pub clearing: bool,
    pub error: Option<String>,
}

/// Folder of the stream files inside the diagnostics folder.
pub(crate) fn streams_dir(dir: &Path) -> PathBuf {
    dir.join("streams")
}

fn muted(ui: &mut egui::Ui, tokens: &Tokens, text: &str) {
    ui.label(egui::RichText::new(text).small().color(color(tokens.text_muted)));
}

fn danger(ui: &mut egui::Ui, tokens: &Tokens, text: &str) {
    ui.label(egui::RichText::new(text).small().color(color(tokens.danger)));
}

pub(crate) fn page(
    ui: &mut egui::Ui,
    s: &mut DiagSettings,
    state: &DiagState,
    stream: &mut StreamUi,
    icons: &Icons,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) {
    let m = Metrics::DEFAULT;
    group(ui, icons, "Diagnostic log", |ui| {
        ui.add_enabled_ui(!state.forced, |ui| {
            ui.horizontal(|ui| {
                let l = ui.label("Level");
                egui::ComboBox::from_id_salt("diag-level")
                    .selected_text(s.level.label())
                    .width(180.0)
                    .show_ui(ui, |ui| {
                        for level in DiagLevel::ALL {
                            ui.selectable_value(&mut s.level, level, level.label());
                        }
                    })
                    .response
                    .labelled_by(l.id);
            });
            if s.level == DiagLevel::ProtocolTrace {
                ui.horizontal_wrapped(|ui| {
                    for (name, label, _) in TRACE_PROTOCOLS {
                        let mut on = s.protocols.iter().any(|p| p == name);
                        if ui.checkbox(&mut on, label).changed() {
                            s.protocols.retain(|p| p != name);
                            if on {
                                s.protocols.push(name.to_owned());
                            }
                        }
                    }
                });
            }
        });
        muted(
            ui,
            tokens,
            match s.level {
                DiagLevel::Off => "No log file is written.",
                DiagLevel::Errors => "Warnings and errors only.",
                DiagLevel::Normal => "Connections, disconnections and errors.",
                DiagLevel::FullDebug => "Every connection step of every protocol; changes apply at once.",
                DiagLevel::ProtocolTrace => "Full debug plus the protocol details of the chosen protocols.",
            },
        );
        ui.add_space(m.space[1]);
        match (&state.file, state.forced) {
            (Some(f), true) => muted(ui, tokens, &format!("Set by --debug-log for this run: {}", f.display())),
            (Some(f), false) => muted(ui, tokens, &format!("Writing to {}", f.display())),
            (None, _) => {}
        }
        muted(ui, tokens, "Passwords, keys and NTLM/CredSSP messages are never written to the log.");
        if let Some(e) = &state.error {
            danger(ui, tokens, e);
        }
    });

    group(ui, icons, "Folder", |ui| {
        let shown = state.dir.as_ref().map_or_else(|| "—".to_owned(), |d| d.display().to_string());
        ui.label(egui::RichText::new(shown).monospace());
        muted(
            ui,
            tokens,
            if s.folder.is_some() {
                "A rustxterm-logs folder inside the folder you chose."
            } else {
                "A rustxterm-logs folder where rustXterm runs, or in the config folder when that one is not writable."
            },
        );
        if let Some(note) = &state.note {
            muted(ui, tokens, &format!("Using the config folder: {note}"));
        }
        ui.horizontal(|ui| {
            if ui.button("Choose folder…").clicked() {
                out.push(Action::PickDiagFolder);
            }
            if s.folder.is_some() && ui.button("Use the folder rustXterm runs from").clicked() {
                s.folder = None;
            }
            if let Some(dir) = &state.dir {
                if ui.button("Open folder").clicked() {
                    out.push(Action::OpenLocal { path: dir.clone(), folder: false });
                }
                if ui.button("Clear logs…").clicked() {
                    stream.clearing = true;
                }
            }
        });
        if stream.clearing {
            ui.horizontal(|ui| {
                danger(ui, tokens, "Delete the diagnostic log and every stream file?");
                if ui.button("Delete").clicked() {
                    stream.clearing = false;
                    out.push(Action::ClearDiagnostics);
                }
                if ui.button("Cancel").clicked() {
                    stream.clearing = false;
                }
            });
        }
    });

    group(ui, icons, "Data stream", |ui| {
        let mut on = stream.on || stream.asking;
        if ui.checkbox(&mut on, "Record data stream").changed() {
            if on {
                stream.asking = true;
            } else {
                stream.asking = false;
                out.push(Action::StreamCapture { on: false, include_input: stream.include_input });
            }
        }
        if stream.asking {
            egui::Frame::new()
                .stroke(egui::Stroke::new(m.stroke, color(tokens.warning)))
                .inner_margin(m.space[2])
                .show(ui, |ui| {
                    ui.label(
                        "The transcript of every remote session is saved, including passwords the server echoes \
                         back and anything remote programs print. Turn it on only to diagnose a problem, and \
                         delete the files afterwards.",
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Start recording").clicked() {
                            stream.asking = false;
                            out.push(Action::StreamCapture { on: true, include_input: stream.include_input });
                        }
                        if ui.button("Cancel").clicked() {
                            stream.asking = false;
                        }
                    });
                });
        }
        let mut input = stream.include_input;
        if ui.checkbox(&mut input, "Include typed input").changed() {
            stream.include_input = input;
            if stream.on {
                out.push(Action::StreamCapture { on: true, include_input: input });
            }
        }
        if stream.include_input {
            danger(ui, tokens, "Typed input can contain passwords typed with echo off (sudo, su, database clients).");
        }
        let dir = state.dir.as_deref().map(streams_dir);
        muted(
            ui,
            tokens,
            &format!(
                "One file per session in {}. Off at every start. RDP and VNC record event types and sizes only; \
                 saved passwords are never recorded; Telnet sessions with automatic login and local terminals \
                 are not recorded.",
                dir.map_or_else(|| "the streams folder".to_owned(), |d| d.display().to_string())
            ),
        );
        if let Some(e) = &stream.error {
            danger(ui, tokens, e);
        }
    });
}

/// Deletes the diagnostic log (with its rotations) and the stream files in `dir`, except `keep`
/// (the `--debug-log` file in use); returns (deleted, failed).
pub(crate) fn clear(dir: &Path, keep: Option<&Path>) -> (usize, usize) {
    // Compared as canonical paths, so `logs/../logs/x.log` is still recognized.
    let keep = keep.and_then(|k| std::fs::canonicalize(k).ok());
    let kept = |p: &Path| keep.is_some() && std::fs::canonicalize(p).ok() == keep;
    let mut done = (0, 0);
    let mut remove = |p: &Path| match std::fs::remove_file(p) {
        Ok(()) => done.0 += 1,
        Err(_) => done.1 += 1,
    };
    let is_log = |p: &Path, prefix: &str| {
        p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(prefix) && n.contains(".log"))
    };
    for (folder, prefix) in [(dir.to_path_buf(), rxt_store::logfile::DIAGNOSTIC_LOG), (streams_dir(dir), "")] {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_file()) && is_log(&p, prefix) && !kept(&p) {
                remove(&p);
            }
        }
    }
    done
}

#[cfg(test)]
mod tests {
    #[test]
    fn clear_removes_only_diagnostic_files() {
        let d = std::env::temp_dir().join(format!("rxt-diag-clear-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(super::streams_dir(&d)).unwrap();
        for f in ["rustxterm-diagnostic.log", "rustxterm-diagnostic.log.1", "other.txt"] {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        std::fs::write(super::streams_dir(&d).join("web-20261019-120000-1.log"), b"x").unwrap();
        let forced = d.join("rustxterm-diagnostic.log.1");
        let aliased = d.join("streams").join("..").join("rustxterm-diagnostic.log.1");
        assert_eq!(super::clear(&d, Some(&aliased)), (2, 0));
        assert!(d.join("other.txt").exists() && forced.exists());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
