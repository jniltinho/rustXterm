//! File operations contract shared by SFTP and FTP (design D5).

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

use crate::Result;

/// Boxed `Send` future so `FileOps` stays object safe.
pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Remote entry kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileKind {
    File,
    Dir,
    Symlink,
    Other,
}

/// One remote directory entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub kind: FileKind,
    pub size: u64,
    /// Seconds since the Unix epoch.
    pub modified: Option<i64>,
    /// Unix permission bits.
    pub mode: Option<u32>,
    /// Owner name (or numeric uid when the server gives no name).
    #[serde(default)]
    pub owner: Option<String>,
    /// Group name (or numeric gid when the server gives no name).
    #[serde(default)]
    pub group: Option<String>,
}

impl FileEntry {
    pub fn is_hidden(&self) -> bool {
        self.name.starts_with('.')
    }
}

/// Decision when a transfer target already exists. Asked before any byte is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Overwrite {
    Overwrite,
    Skip,
    Rename(String),
}

/// Transfer progress callback: bytes done so far.
pub type Progress<'a> = &'a (dyn Fn(u64) + Send + Sync);

/// Remote file system operations. Paths are remote POSIX paths.
/// Implementations must remove partial targets when cancelled.
pub trait FileOps: Send + Sync {
    fn home(&self) -> BoxFut<'_, Result<String>>;
    fn list<'a>(&'a self, path: &'a str) -> BoxFut<'a, Result<Vec<FileEntry>>>;
    fn stat<'a>(&'a self, path: &'a str) -> BoxFut<'a, Result<Option<FileEntry>>>;
    fn mkdir<'a>(&'a self, path: &'a str) -> BoxFut<'a, Result<()>>;
    fn rename<'a>(&'a self, from: &'a str, to: &'a str) -> BoxFut<'a, Result<()>>;
    /// Delete a file, or a directory recursively.
    fn delete<'a>(&'a self, path: &'a str) -> BoxFut<'a, Result<()>>;
    fn chmod<'a>(&'a self, path: &'a str, mode: u32) -> BoxFut<'a, Result<()>>;
    /// Download `remote` to `local`. The caller resolves `Overwrite` beforehand.
    fn get<'a>(&'a self, remote: &'a str, local: &'a Path, progress: Progress<'a>) -> BoxFut<'a, Result<u64>>;
    /// Upload `local` to `remote`. The caller resolves `Overwrite` beforehand.
    fn put<'a>(&'a self, local: &'a Path, remote: &'a str, progress: Progress<'a>) -> BoxFut<'a, Result<u64>>;
    /// Absolute path of `path` with symlinks resolved (SFTP `realpath`); FTP cannot do this.
    fn real_path<'a>(&'a self, path: &'a str) -> BoxFut<'a, Result<String>> {
        let _ = path;
        Box::pin(async { Err(crate::Error::Unsupported("resolving remote paths".into())) })
    }
}

/// Join a remote POSIX directory and name.
pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") }
}

/// Owner and group of an `ls -l` style line (SFTP `longname`, FTP `LIST`): fields 3 and 4 of
/// `perms links owner group size month day time name`; `None` for shorter lines or lines that do
/// not start with a mode string (DOS listings).
pub fn ls_owner_group(line: &str) -> Option<(String, String)> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    let mode_like = fields.first().is_some_and(|m| m.len() >= 10 && m.starts_with(['-', 'd', 'l', 'c', 'b', 'p', 's']));
    if fields.len() < 8 || !mode_like {
        return None;
    }
    Some((fields[2].to_owned(), fields[3].to_owned()))
}

/// Parent of a remote POSIX path (`/` stays `/`).
pub fn parent(path: &str) -> String {
    match path.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) | None => "/".into(),
        Some((p, _)) => p.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(join("/var", "log"), "/var/log");
        assert_eq!(join("/", "etc"), "/etc");
        assert_eq!(parent("/var/log/"), "/var");
        assert_eq!(parent("/var"), "/");
        assert_eq!(parent("/"), "/");
    }

    #[test]
    fn serde_round_trip() {
        let e = FileEntry {
            name: ".bashrc".into(),
            kind: FileKind::File,
            size: 3,
            modified: Some(1),
            mode: Some(0o644),
            owner: Some("alice".into()),
            group: None,
        };
        assert!(e.is_hidden());
        assert_eq!(serde_json::from_str::<FileEntry>(&serde_json::to_string(&e).unwrap()).unwrap(), e);
        // Older JSON without owner/group still parses.
        let old = r#"{"name":"a","kind":"File","size":1,"modified":null,"mode":null}"#;
        assert_eq!(serde_json::from_str::<FileEntry>(old).unwrap().owner, None);
    }

    #[test]
    fn owner_group_from_ls_line() {
        let l = "-rw-r--r--    1 alice    staff         220 Jan  6  2022 .bash logout";
        assert_eq!(ls_owner_group(l), Some(("alice".into(), "staff".into())));
        let n = "drwxr-xr-x 2 1000 1000 4096 Jan 6 12:00 dir";
        assert_eq!(ls_owner_group(n), Some(("1000".into(), "1000".into())));
        assert_eq!(ls_owner_group("-rw-r--r-- 1 alice"), None);
        assert_eq!(ls_owner_group(""), None);
        assert_eq!(ls_owner_group("01-02-24  10:00AM  <DIR>  a b c d e f"), None);
    }
}
