//! Paths, owner-only directories, clipboard, notifications, file dialogs and the opener.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rxt_platform::{Clipboard, Error, FileDialogs, Notifier, Opener, Paths, Platform, Result};

use crate::{
    APP_DIR, PipeControl, RegistryTheme, WindowsSerial, ancestor_safe, drive_root, inside, is_unc, owner_is,
    private_dir_sddl, same_path, sd_is_owner_only, strip_verbatim, sys,
};

/// Build the Windows platform services; never fails (missing pieces degrade to errors on use).
pub fn platform() -> Platform {
    Platform {
        paths: Arc::new(KnownFolders),
        theme: Arc::new(RegistryTheme::spawn()),
        clipboard: Arc::new(SystemClipboard::default()),
        notifier: Arc::new(Toasts),
        dialogs: Arc::new(Win32Dialogs::default()),
        keyring: Arc::new(crate::CredentialManager),
        control: Arc::new(PipeControl::new()),
        opener: Arc::new(ShellOpen),
        serial: Arc::new(WindowsSerial),
    }
}

/// Known Folders (design W3): config in `%APPDATA%`, data and cache in `%LOCALAPPDATA%` (the
/// encrypted database with WAL must not roam), SSH files in `%USERPROFILE%\.ssh`.
pub struct KnownFolders;

impl Paths for KnownFolders {
    fn config_dir(&self) -> PathBuf {
        dirs::config_dir().unwrap_or_else(|| self.home_dir().join("AppData").join("Roaming")).join(APP_DIR)
    }
    fn data_dir(&self) -> PathBuf {
        dirs::data_local_dir().unwrap_or_else(|| self.home_dir().join("AppData").join("Local")).join(APP_DIR)
    }
    fn cache_dir(&self) -> PathBuf {
        self.data_dir().join("cache")
    }
    fn runtime_dir(&self) -> Option<PathBuf> {
        None
    }
    fn home_dir(&self) -> PathBuf {
        dirs::home_dir().unwrap_or_else(std::env::temp_dir)
    }
    fn make_private(&self, dir: &Path) -> Result<()> {
        make_private(dir)
    }
}

/// `FILE_ATTRIBUTE_REPARSE_POINT`: symlinks, junctions, mount points.
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// Design W6: reject network and FAT/exFAT locations and reparse points, then give `dir` and
/// everything already inside it owner = current user and a protected owner-only DACL (inherited
/// by new files), verified by read-back. Errors name the failing path.
pub fn make_private(dir: &Path) -> Result<()> {
    let denied =
        |path: &Path, why: String| Error::PermissionDenied(format!("cannot protect {}: {why}", path.display()));
    let abs = std::path::absolute(dir).map_err(|e| denied(dir, e.to_string()))?;
    let text = abs.to_string_lossy();
    if is_unc(&text) {
        return Err(denied(dir, "network locations are not supported for rustXterm data".into()));
    }
    // Canonical path of the nearest existing ancestor plus the missing tail: a link or junction
    // anywhere in the ancestry makes it differ from the absolute path.
    let mut existing = abs.as_path();
    while !existing.exists() {
        existing = existing.parent().ok_or_else(|| denied(dir, "no existing ancestor".into()))?;
    }
    let tail = abs.strip_prefix(existing).map_err(|e| denied(dir, e.to_string()))?;
    let canonical = std::fs::canonicalize(existing).map_err(|e| denied(dir, e.to_string()))?.join(tail);
    let canonical = canonical.to_string_lossy();
    // Short (8.3) names such as C:\Users\RUNNER~1 are not links: compare the long form.
    let long = sys::long_path(existing).map_err(|e| denied(dir, e.to_string()))?.join(tail);
    if !same_path(&canonical, &long.to_string_lossy()) {
        return Err(denied(dir, format!("reached through a link or junction ({canonical})")));
    }
    // Inside the user profile, whose ACL keeps other users from replacing any ancestor (W6).
    let profile = dirs::home_dir().and_then(|h| std::fs::canonicalize(h).ok());
    if !profile.as_ref().is_some_and(|p| inside(&canonical, &p.to_string_lossy())) {
        return Err(denied(dir, "must be inside your user profile folder (other users could replace it)".into()));
    }
    // No ancestor up to the profile may let another user replace it or change its permissions.
    if let Some(profile) = &profile {
        let sid = sys::current_identity().map_err(|e| denied(dir, format!("current user: {e}")))?.sid;
        // Missing ancestors will be created with the inherited ACL of the first existing one.
        let mut ancestor = abs.parent().ok_or_else(|| denied(dir, "no parent folder".into()))?;
        while !ancestor.exists() {
            ancestor = ancestor.parent().ok_or_else(|| denied(dir, "no existing ancestor".into()))?;
        }
        loop {
            let sddl =
                sys::owner_dacl_sddl(ancestor).map_err(|e| denied(ancestor, format!("read access list: {e}")))?;
            if !ancestor_safe(&sddl, &sid) {
                return Err(denied(dir, format!("other users could replace {} ({sddl})", ancestor.display())));
            }
            let canonical_ancestor = std::fs::canonicalize(ancestor).map_err(|e| denied(ancestor, e.to_string()))?;
            if same_path(&canonical_ancestor.to_string_lossy(), &profile.to_string_lossy()) {
                break;
            }
            ancestor = ancestor.parent().ok_or_else(|| denied(dir, "left the user profile".into()))?;
        }
    }
    let root = drive_root(strip_verbatim(&canonical)).ok_or_else(|| denied(dir, "not on a local drive".into()))?;
    match sys::volume_is_local_with_acls(Path::new(&root)) {
        Ok(true) => {}
        Ok(false) => {
            return Err(denied(dir, "network drive or file system without access control lists (FAT/exFAT)".into()));
        }
        Err(e) => return Err(denied(dir, format!("volume {root}: {e}"))),
    }
    let sid = sys::current_identity().map_err(|e| denied(dir, format!("current user: {e}")))?.sid;
    // Missing directories are created owner-only from the start (no inherited-ACL window).
    let missing: Vec<&Path> = abs.ancestors().take_while(|p| !p.exists()).collect();
    for path in missing.into_iter().rev() {
        sys::create_dir_with(path, &private_dir_sddl(&sid)).map_err(|e| denied(path, e.to_string()))?;
    }
    // Check the whole existing tree (owners included) before changing anything, then repair.
    protect_tree(&abs, &sid, &denied, false)?;
    protect_tree(&abs, &sid, &denied, true)
}

fn protect_tree(path: &Path, sid: &str, denied: &dyn Fn(&Path, String) -> Error, repair: bool) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path).map_err(|e| denied(path, e.to_string()))?;
    if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(denied(path, "symbolic links and junctions are not allowed here".into()));
    }
    let current = sys::owner_dacl_sddl(path).map_err(|e| denied(path, format!("read access list: {e}")))?;
    // Another user who could write or re-permission an existing entry may already hold an open
    // handle that survives the repair (e.g. to turn a folder into a junction): reject, don't
    // repair. Extra read-only entries are repaired below.
    if !ancestor_safe(&current, sid) {
        return Err(denied(path, format!("other users could modify it ({current}); move or delete it first")));
    }
    if repair && !sd_is_owner_only(&current, sid) {
        sys::set_owner_and_protected_dacl(path, &private_dir_sddl(sid), owner_is(&current, sid))
            .map_err(|e| denied(path, format!("set owner and access list: {e}")))?;
        let back = sys::owner_dacl_sddl(path).map_err(|e| denied(path, format!("read access list: {e}")))?;
        if !sd_is_owner_only(&back, sid) {
            return Err(denied(path, format!("access list is not owner-only after setting it ({back})")));
        }
    }
    if meta.is_dir() {
        for entry in std::fs::read_dir(path).map_err(|e| denied(path, e.to_string()))? {
            let entry = entry.map_err(|e| denied(path, e.to_string()))?;
            protect_tree(&entry.path(), sid, denied, repair)?;
        }
    }
    Ok(())
}

/// arboard clipboard (Win32), opened on first use. No primary selection on Windows.
#[derive(Default)]
pub struct SystemClipboard {
    inner: Mutex<Option<arboard::Clipboard>>,
}

impl SystemClipboard {
    fn with<T>(&self, f: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>) -> Result<T> {
        let mut guard = self.inner.lock().map_err(|_| Error::Io("clipboard lock poisoned".into()))?;
        if guard.is_none() {
            *guard = Some(arboard::Clipboard::new().map_err(clip_err)?);
        }
        match guard.as_mut() {
            Some(c) => f(c).map_err(clip_err),
            None => Err(Error::Unsupported("clipboard".into())),
        }
    }
}

fn clip_err(e: arboard::Error) -> Error {
    match e {
        arboard::Error::ClipboardNotSupported => Error::Unsupported("clipboard".into()),
        arboard::Error::ContentNotAvailable => Error::NotFound("clipboard has no text".into()),
        other => Error::Io(format!("clipboard: {other}")),
    }
}

impl Clipboard for SystemClipboard {
    fn get_text(&self) -> Result<String> {
        self.with(|c| c.get_text())
    }
    fn set_text(&self, text: &str) -> Result<()> {
        self.with(|c| c.set_text(text))
    }
}

/// WinRT toast notifications. ponytail: uses notify-rust's default identity until the MSI
/// registers the `rustXterm.rustXterm` AUMID with its Start Menu shortcut (W2).
pub struct Toasts;

impl Notifier for Toasts {
    fn notify(&self, title: &str, body: &str) -> Result<()> {
        notify_rust::Notification::new()
            .appname("rustXterm")
            .summary(title)
            .body(body)
            .show()
            .map(|_| ())
            .map_err(|e| Error::Unsupported(format!("notifications: {e}")))
    }
}

/// rfd (`IFileDialog`) owned by the main window when its `win32:<hex HWND>` is known.
#[derive(Default)]
pub struct Win32Dialogs {
    parent: Mutex<Option<String>>,
}

impl Win32Dialogs {
    fn dialog(&self, title: &str) -> rfd::FileDialog {
        let dialog = rfd::FileDialog::new().set_title(title);
        let parent = self.parent.lock().ok().and_then(|p| p.as_deref().and_then(crate::parse_parent));
        match parent {
            Some(hwnd) => dialog.set_parent(&sys::ParentWindow(hwnd)),
            None => dialog,
        }
    }
}

impl FileDialogs for Win32Dialogs {
    fn pick_files(&self, title: &str) -> Option<Vec<PathBuf>> {
        self.dialog(title).pick_files()
    }
    fn pick_folder(&self, title: &str) -> Option<PathBuf> {
        self.dialog(title).pick_folder()
    }
    fn save_file(&self, title: &str, suggested_name: &str) -> Option<PathBuf> {
        self.dialog(title).set_file_name(suggested_name).save_file()
    }
    fn set_parent(&self, parent: Option<String>) {
        if let Ok(mut p) = self.parent.lock() {
            *p = parent;
        }
    }
}

/// `ShellExecuteW` with the `open` verb; failures (no association, missing file) are reported.
pub struct ShellOpen;

impl Opener for ShellOpen {
    fn open(&self, path: &Path) -> Result<()> {
        sys::shell_open(path.as_os_str()).map_err(|e| Error::Io(format!("cannot open {}: {e}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rxt-private-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// Pre-existing files with extra explicit entries are repaired; new files inherit.
    #[test]
    fn make_private_repairs_existing_entries() {
        let sid = sys::current_identity().unwrap().sid;
        let dir = tmp("repair");
        std::fs::create_dir_all(dir.join("backups")).unwrap();
        for f in ["rustxterm.db", "rustxterm.db-wal", "backups/old.db"] {
            let p = dir.join(f);
            std::fs::write(&p, b"x").unwrap();
            // Everyone may read, explicit and protected (an older tool's leftovers).
            sys::set_owner_and_protected_dacl(&p, &format!("O:{sid}D:P(A;;FR;;;WD)(A;;FA;;;{sid})"), true).unwrap();
            assert!(!sd_is_owner_only(&sys::owner_dacl_sddl(&p).unwrap(), &sid));
        }
        make_private(&dir).unwrap();
        std::fs::write(dir.join("envelope.json"), b"{}").unwrap();
        for f in ["", "rustxterm.db", "rustxterm.db-wal", "backups", "backups/old.db", "envelope.json"] {
            let back = sys::owner_dacl_sddl(&dir.join(f)).unwrap();
            assert!(sd_is_owner_only(&back, &sid), "{f}: {back}");
        }
        make_private(&dir).unwrap(); // idempotent
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn make_private_rejects_junctions_and_network_paths() {
        let dir = tmp("junction");
        let target = tmp("junction-target");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        let link = dir.join("link");
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "mklink /J failed");
        let e = make_private(&dir).unwrap_err();
        assert!(matches!(&e, Error::PermissionDenied(m) if m.contains("link")), "{e}");
        let _ = std::fs::remove_dir(&link);
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(target);
        let e = make_private(Path::new(r"\\rxt-no-such-server\share\rustxterm")).unwrap_err();
        assert!(matches!(&e, Error::PermissionDenied(m) if m.contains("network")), "{e}");
        let e = make_private(Path::new(r"C:\rxt-outside-profile\data")).unwrap_err();
        assert!(matches!(&e, Error::PermissionDenied(m) if m.contains("profile")), "{e}");
        assert!(!Path::new(r"C:\rxt-outside-profile").exists(), "nothing may be created outside the profile");
    }

    /// A folder inside the profile that grants Users full control makes everything below it
    /// replaceable by other users.
    #[test]
    fn make_private_rejects_a_permissive_ancestor() {
        let sid = sys::current_identity().unwrap().sid;
        // Full control, write-data only, write-attributes only (reparse-point creation).
        for rights in ["FA", "0x2", "0x100"] {
            let shared = tmp("shared");
            std::fs::create_dir_all(&shared).unwrap();
            let sddl = format!("O:{sid}D:P(A;OICI;FA;;;{sid})(A;;{rights};;;BU)");
            sys::set_owner_and_protected_dacl(&shared, &sddl, true).unwrap();
            let e = make_private(&shared.join("missing").join("rustxterm")).unwrap_err();
            assert!(
                matches!(&e, Error::PermissionDenied(m) if m.contains("other users could replace")),
                "{rights}: {e}"
            );
            assert!(!shared.join("missing").exists(), "{rights}: nothing may be created");
            let _ = std::fs::remove_dir_all(shared);
        }
    }

    /// An existing entry that another user may modify is rejected, not repaired (an open handle
    /// of that user would survive the repair), and nothing is changed.
    #[test]
    fn make_private_rejects_entries_others_can_modify() {
        let sid = sys::current_identity().unwrap().sid;
        for (rights, entry) in
            [("FA", "rustxterm.db"), ("0x4", "rustxterm.db-wal"), ("0x10", "envelope.json"), ("0x100", "backups")]
        {
            let dir = tmp("modifiable");
            std::fs::create_dir_all(&dir).unwrap();
            let p = dir.join(entry);
            if entry == "backups" {
                std::fs::create_dir(&p).unwrap();
            } else {
                std::fs::write(&p, b"x").unwrap();
            }
            let sddl = format!("O:{sid}D:P(A;;FA;;;{sid})(A;;{rights};;;BU)");
            sys::set_owner_and_protected_dacl(&p, &sddl, true).unwrap();
            let before = sys::owner_dacl_sddl(&p).unwrap();
            let e = make_private(&dir).unwrap_err();
            assert!(matches!(&e, Error::PermissionDenied(m) if m.contains("other users could modify")), "{e}");
            assert_eq!(sys::owner_dacl_sddl(&p).unwrap(), before, "a rejected entry must stay untouched");
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// Missing directories are created owner-only from the start.
    #[test]
    fn make_private_creates_missing_directories_owner_only() {
        let sid = sys::current_identity().unwrap().sid;
        let base = tmp("create");
        make_private(&base.join("a").join("b")).unwrap();
        for p in [base.clone(), base.join("a"), base.join("a").join("b")] {
            let sd = sys::owner_dacl_sddl(&p).unwrap();
            assert!(sd_is_owner_only(&sd, &sid) && sd.contains("D:P"), "{}: {sd}", p.display());
        }
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn make_private_rejects_a_junction_in_the_ancestry() {
        let real = tmp("anc-real");
        let link = tmp("anc-link");
        std::fs::create_dir_all(&real).unwrap();
        let ok = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(&real).output().unwrap();
        assert!(ok.status.success(), "mklink /J failed");
        let e = make_private(&link.join("data")).unwrap_err();
        assert!(matches!(&e, Error::PermissionDenied(m) if m.contains("junction")), "{e}");
        assert!(!real.join("data").exists());
        let _ = std::fs::remove_dir(&link);
        let _ = std::fs::remove_dir_all(real);
    }

    #[test]
    fn opener_reports_missing_file() {
        assert!(ShellOpen.open(Path::new(r"C:\rxt-no-such-dir\missing.txt")).is_err());
    }

    #[test]
    fn known_folders_layout() {
        let p = KnownFolders;
        assert!(p.config_dir().ends_with(APP_DIR) && p.config_dir().to_string_lossy().contains("Roaming"));
        assert!(p.data_dir().ends_with(APP_DIR) && p.data_dir().to_string_lossy().contains("Local"));
        assert_eq!(p.ssh_dir(), p.home_dir().join(".ssh"));
        assert_eq!(p.runtime_dir(), None);
    }
}
