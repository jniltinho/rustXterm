//! Dual-pane file tab for standalone SFTP and FTP/FTPS sessions (design D5): the local
//! file system on the left, the remote browser on the right, drag between panes to transfer.

use std::path::Path;

use rxt_core::{FileEntry, FileKind, Protocol, Session, Tokens};

use crate::app::Action;
use crate::icons::Icons;
use crate::sftp_panel::{self, FileDrag, HOME, SftpAction, SftpState, TransferRow};
use crate::theme::color;

const TRANSFERS_HEIGHT: f32 = 130.0;

/// Local and remote panes of one file tab.
#[derive(Debug, Default)]
pub(crate) struct FileView {
    pub local: SftpState,
    pub remote: SftpState,
}

/// Plain FTP and Telnet send credentials and data in clear text.
pub(crate) fn unencrypted(p: Protocol) -> bool {
    p.is_unencrypted()
}

/// Draw the dual-pane tab.
pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &mut FileView,
    session: &Session,
    transfers: &[TransferRow],
    icons: &Icons,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) {
    // ponytail: fixed-height transfers strip; make it resizable if users ask.
    let panes = (ui.available_height() - TRANSFERS_HEIGHT).max(120.0);
    ui.allocate_ui(egui::vec2(ui.available_width(), panes), |ui| {
        ui.columns(2, |cols| {
            pane(&mut cols[0], view, true, session, icons, tokens, out);
            pane(&mut cols[1], view, false, session, icons, tokens, out);
        });
    });
    ui.separator();
    sftp_panel::transfers_ui(ui, transfers, icons, tokens, out);
}

fn pane(
    ui: &mut egui::Ui,
    view: &mut FileView,
    local: bool,
    session: &Session,
    icons: &Icons,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        let icon = if local { "home" } else { session.protocol.icon() };
        if let Some(img) = icons.image(icon, 16.0) {
            ui.add(img);
        }
        if local {
            ui.strong("Local");
        } else {
            ui.strong(format!("{}://{}", session.protocol.scheme(), session.host));
            if unencrypted(session.protocol) {
                if let Some(img) = icons.image("unlock", 16.0) {
                    ui.add(img);
                }
                ui.colored_label(color(tokens.warning), "Unencrypted")
                    .on_hover_text("Plain FTP sends the password and files without encryption.");
            }
        }
    });
    let local_dir = view.local.path.clone();
    let st = if local { &mut view.local } else { &mut view.remote };
    // Rows dragged from the other pane transfer; desktop files upload to the remote pane.
    let dropped = ui
        .push_id(local, |ui| {
            sftp_panel::drop_target(
                ui,
                tokens,
                !local,
                |d| d.local != local,
                |ui| {
                    sftp_panel::browser(ui, st, local, icons, tokens, out);
                },
            )
        })
        .inner;
    if let Some(drag) = dropped {
        out.push(drop_action(&drag, local, &local_dir));
    }
}

/// Transfer requested by dropping `drag` on the local (`to_local`) or remote pane.
pub(crate) fn drop_action(drag: &FileDrag, to_local: bool, local_dir: &str) -> Action {
    if drag.dir {
        return Action::Status("Folders cannot be transferred yet; drag files instead.".into());
    }
    if to_local {
        Action::Sftp(SftpAction::DownloadTo { remote: drag.path.clone(), dir: local_dir.to_owned() })
    } else {
        Action::Sftp(SftpAction::UploadPaths(vec![drag.path.clone().into()]))
    }
}

/// Apply a local-pane request. Returns an error message for the status bar.
pub(crate) fn local_action(st: &mut SftpState, a: SftpAction, home: &Path) -> Option<String> {
    let result = match a {
        SftpAction::Navigate(path) if path == HOME => return navigate(st, &home.to_string_lossy()),
        SftpAction::Navigate(path) => return navigate(st, &path),
        SftpAction::Refresh => return navigate(st, &st.path.clone()),
        SftpAction::Select(name) => {
            st.selected = Some(name);
            return None;
        }
        SftpAction::Mkdir(path) => std::fs::create_dir(&path),
        SftpAction::NewFile(path) => std::fs::File::create_new(&path).map(drop),
        SftpAction::Rename { from, to } => std::fs::rename(&from, &to),
        SftpAction::Delete(path) => match std::fs::symlink_metadata(&path) {
            Ok(m) if m.is_dir() => std::fs::remove_dir_all(&path),
            Ok(_) => std::fs::remove_file(&path),
            Err(e) => Err(e),
        },
        _ => return None,
    };
    let err = result.err().map(|e| e.to_string());
    let path = st.path.clone();
    navigate(st, &path).or(err)
}

/// List `path` into the pane; on error the pane keeps its listing and shows the error.
fn navigate(st: &mut SftpState, path: &str) -> Option<String> {
    match read_local(Path::new(path)) {
        Ok(entries) => {
            st.listed(path.to_owned(), entries);
            None
        }
        Err(e) => {
            // The listing stays; the pane shows the failed path and the cause with Retry.
            st.path_edit = path.to_owned();
            st.error = Some(e.to_string());
            Some(format!("{path}: {e}"))
        }
    }
}

/// Local directory listing in the shared [`FileEntry`] shape.
pub(crate) fn read_local(dir: &Path) -> std::io::Result<Vec<FileEntry>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let Ok(link) = entry.file_type() else { continue };
        let meta = std::fs::metadata(entry.path()).ok();
        let kind = if link.is_symlink() {
            FileKind::Symlink
        } else if link.is_dir() {
            FileKind::Dir
        } else if link.is_file() {
            FileKind::File
        } else {
            FileKind::Other
        };
        let modified = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|d| i64::try_from(d.as_secs()).ok());
        let (owner, group) = meta.as_ref().map_or((None, None), ids);
        out.push(FileEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            kind,
            size: meta.as_ref().map_or(0, |m| m.len()),
            modified,
            mode: meta.as_ref().and_then(mode),
            owner,
            group,
        });
    }
    Ok(out)
}

#[cfg(unix)]
fn mode(m: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(m.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn mode(_: &std::fs::Metadata) -> Option<u32> {
    None
}

/// Numeric owner and group ids (no name lookup without a libc dependency).
#[cfg(unix)]
fn ids(m: &std::fs::Metadata) -> (Option<String>, Option<String>) {
    use std::os::unix::fs::MetadataExt;
    (Some(m.uid().to_string()), Some(m.gid().to_string()))
}

#[cfg(not(unix))]
fn ids(_: &std::fs::Metadata) -> (Option<String>, Option<String>) {
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_pane_operations() {
        let dir = std::env::temp_dir().join(format!("rxt-local-pane-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"abc").unwrap();
        let d = dir.to_string_lossy().into_owned();
        let mut st = SftpState::default();
        assert_eq!(local_action(&mut st, SftpAction::Navigate(d.clone()), &dir), None);
        assert_eq!(st.entries.len(), 1);
        assert_eq!((st.entries[0].kind, st.entries[0].size), (FileKind::File, 3));
        local_action(&mut st, SftpAction::Mkdir(format!("{d}/sub")), &dir);
        local_action(&mut st, SftpAction::Rename { from: format!("{d}/a.txt"), to: format!("{d}/b.txt") }, &dir);
        let mut names: Vec<_> = st.entries.iter().map(|e| e.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["b.txt", "sub"]);
        local_action(&mut st, SftpAction::Delete(format!("{d}/sub")), &dir);
        assert_eq!(st.entries.len(), 1);
        assert!(local_action(&mut st, SftpAction::Navigate(format!("{d}/missing")), &dir).is_some());
        assert_eq!(st.path, d, "a failed navigation keeps the listing");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn drops_map_to_transfers() {
        let file = FileDrag { local: false, path: "/srv/report.pdf".into(), dir: false };
        assert_eq!(
            drop_action(&file, true, "/home/alice"),
            Action::Sftp(SftpAction::DownloadTo { remote: "/srv/report.pdf".into(), dir: "/home/alice".into() })
        );
        let up = FileDrag { local: true, path: "/home/alice/x".into(), dir: false };
        assert_eq!(drop_action(&up, false, "/"), Action::Sftp(SftpAction::UploadPaths(vec!["/home/alice/x".into()])));
        assert!(matches!(drop_action(&FileDrag { dir: true, ..up }, false, "/"), Action::Status(_)));
    }
}
