//! Desktop entry for the portable binary and the AppImage.
//!
//! Wayland compositors (GNOME, KDE) take a window's icon from the `.desktop` file whose name
//! matches the app id (`rustxterm`), not from the window, so without one the dock shows a generic
//! icon. GLib also ignores an entry whose `Exec` program is not on `PATH`, so the entry names the
//! running executable (the `.AppImage` file inside an AppImage) by absolute path.
//!
//! Written to `$XDG_DATA_HOME/applications/rustxterm.desktop` and refreshed when the executable
//! moves. An entry rustXterm did not write (a distribution package, the user's own) is never
//! touched, and none is written when a system entry exists.

use std::path::{Path, PathBuf};

const MARKER: &str = "X-rustXterm-Generated=true";
const ENTRY: &str = "applications/rustxterm.desktop";
const ICON: &str = "icons/hicolor/256x256/apps/rustxterm.png";

/// Install or refresh the entry and its icon for the running executable.
/// Returns whether anything was written.
pub fn install_desktop_entry(icon_png: &[u8]) -> std::io::Result<bool> {
    let Some(data_home) = dirs::data_dir() else { return Ok(false) };
    let Some(exe) = launcher() else { return Ok(false) };
    let system: Vec<PathBuf> = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into())
        .split(':')
        .map(PathBuf::from)
        .collect();
    install(&data_home, &system, &exe, icon_png)
}

/// The program the entry should start: `$APPIMAGE` when running from that AppImage's mount,
/// otherwise this executable.
fn launcher() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let appdir = std::env::var_os("APPDIR").map(PathBuf::from);
    match (std::env::var_os("APPIMAGE"), appdir) {
        (Some(image), Some(dir)) if exe.starts_with(&dir) => Some(PathBuf::from(image)),
        _ => Some(exe),
    }
}

fn install(data_home: &Path, system: &[PathBuf], exe: &Path, icon_png: &[u8]) -> std::io::Result<bool> {
    if system.iter().any(|d| d.join(ENTRY).exists()) {
        return Ok(false);
    }
    let Some(content) = entry(exe) else { return Ok(false) };
    let path = data_home.join(ENTRY);
    match std::fs::read_to_string(&path) {
        Ok(old) if old == content || !old.lines().any(|l| l == MARKER) => return Ok(false),
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let icon = data_home.join(ICON);
    if std::fs::read(&icon).ok().as_deref() != Some(icon_png) {
        write(&icon, icon_png)?;
    }
    write(&path, content.as_bytes())?;
    Ok(true)
}

/// Write through a temporary file so a crash never leaves a truncated entry.
fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// The entry text, or `None` for a path that would need escaping in `Exec`.
fn entry(exe: &Path) -> Option<String> {
    let exe = exe.to_str()?;
    if !exe.starts_with('/') || exe.chars().any(|c| c.is_control() || "\"`$\\%=".contains(c)) {
        return None;
    }
    Some(format!(
        "[Desktop Entry]\nType=Application\nName=rustXterm\nGenericName=Terminal and Remote Session Manager\n\
         Comment=SSH, SFTP, FTP, RDP and VNC sessions in one window\nExec=\"{exe}\"\nIcon=rustxterm\n\
         Terminal=false\nCategories=Network;RemoteAccess;System;TerminalEmulator;\nStartupWMClass=rustxterm\n{MARKER}\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_refreshes_and_respects_foreign_entries() {
        let root = std::env::temp_dir().join(format!("rxt-desktop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (home, sys) = (root.join("home"), root.join("sys"));
        let icon = b"png";
        let path = home.join(ENTRY);

        assert!(install(&home, std::slice::from_ref(&sys), Path::new("/opt/a/rustxterm"), icon).unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("Exec=\"/opt/a/rustxterm\"") && text.contains(MARKER));
        assert_eq!(std::fs::read(home.join(ICON)).unwrap(), icon);
        // Same executable: nothing to do; moved executable: refreshed.
        assert!(!install(&home, std::slice::from_ref(&sys), Path::new("/opt/a/rustxterm"), icon).unwrap());
        assert!(install(&home, std::slice::from_ref(&sys), Path::new("/opt/b/rustxterm"), icon).unwrap());
        assert!(std::fs::read_to_string(&path).unwrap().contains("/opt/b/"));

        // An entry without the marker belongs to someone else.
        std::fs::write(&path, "[Desktop Entry]\nExec=rustxterm\n").unwrap();
        assert!(!install(&home, std::slice::from_ref(&sys), Path::new("/opt/c/rustxterm"), icon).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[Desktop Entry]\nExec=rustxterm\n");

        // A system entry (package install) wins: nothing is written.
        std::fs::remove_file(&path).unwrap();
        write(&sys.join(ENTRY), b"x").unwrap();
        assert!(!install(&home, std::slice::from_ref(&sys), Path::new("/opt/c/rustxterm"), icon).unwrap());
        assert!(!path.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn exec_paths_that_need_escaping_are_refused() {
        assert!(entry(Path::new("/home/user/Apps/rustxterm.AppImage")).is_some());
        for bad in ["relative/rustxterm", "/a\"b", "/a$b", "/a`b", "/a\\b", "/a%b", "/a=b", "/a\nb"] {
            assert!(entry(Path::new(bad)).is_none(), "{bad}");
        }
    }
}
