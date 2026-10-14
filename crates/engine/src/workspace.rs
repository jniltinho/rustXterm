//! Agent file scope (design D14): local paths of agent `files get`/`files put` resolve inside
//! the agent workspace through a cap-std directory capability. Absolute paths, `..` and
//! symlinks leading out of the workspace are refused (lexically first, then by cap-std).

use std::io;
use std::path::{Component, Path, PathBuf};

use cap_std::ambient_authority;
use cap_std::fs::Dir;

/// Open agent workspace.
pub(crate) struct Workspace {
    pub root: PathBuf,
    pub dir: Dir,
}

/// Why a workspace path was refused or failed.
#[derive(Debug)]
pub(crate) enum PathError {
    /// Outside the workspace (absolute, `..`, symlink escape): an authorization denial.
    Escape(String),
    Io(io::Error),
}

impl Workspace {
    /// Open (creating it 0700 when missing) the workspace at `root`.
    pub fn open(root: &Path) -> io::Result<Self> {
        if root.as_os_str().is_empty() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "agent workspace is not configured"));
        }
        if !root.exists() {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true);
            // Windows: the app applies the owner-only ACL (`Paths::make_private`) to the workspace.
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            builder.create(root)?;
        }
        Ok(Self { root: root.to_owned(), dir: Dir::open_ambient_dir(root, ambient_authority())? })
    }

    /// Lexical check: a non-empty relative path made of normal components only.
    pub fn check(rel: &str) -> Result<&Path, PathError> {
        let path = Path::new(rel);
        let ok = !rel.is_empty() && path.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
        if ok { Ok(path) } else { Err(PathError::Escape(format!("{rel}: outside the agent workspace"))) }
    }

    /// Open `rel` for reading inside the workspace.
    pub fn open_read(&self, rel: &str) -> Result<std::fs::File, PathError> {
        let path = Self::check(rel)?;
        self.dir.open(path).map(|f| f.into_std()).map_err(|e| escape_or_io(rel, e))
    }

    /// Create (or truncate) `rel` inside the workspace.
    pub fn create(&self, rel: &str) -> Result<std::fs::File, PathError> {
        let path = Self::check(rel)?;
        self.dir.create(path).map(|f| f.into_std()).map_err(|e| escape_or_io(rel, e))
    }

    /// Rename `from` to `to`, both inside the workspace.
    pub fn rename(&self, from: &str, to: &str) -> Result<(), PathError> {
        let to_path = Self::check(to)?;
        self.dir.rename(Self::check(from)?, &self.dir, to_path).map_err(|e| escape_or_io(to, e))
    }

    pub fn remove(&self, rel: &str) {
        if let Ok(p) = Self::check(rel) {
            let _ = self.dir.remove_file(p);
        }
    }
}

/// cap-std reports escapes (absolute paths, `..`, symlinks out) as `PermissionDenied`.
fn escape_or_io(rel: &str, e: io::Error) -> PathError {
    if e.kind() == io::ErrorKind::PermissionDenied {
        PathError::Escape(format!("{rel}: outside the agent workspace"))
    } else {
        PathError::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(name: &str) -> (PathBuf, Workspace) {
        let base = std::env::temp_dir().join(format!("rxt-ws-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("outside")).unwrap();
        std::fs::write(base.join("outside/id_ed25519"), "secret").unwrap();
        let w = Workspace::open(&base.join("agent-files")).unwrap();
        (base, w)
    }

    #[cfg(unix)]
    fn escaped<T>(r: Result<T, PathError>) -> bool {
        matches!(r, Err(PathError::Escape(_)))
    }

    #[test]
    fn inside_paths_work() {
        let (base, w) = ws("in");
        use std::io::{Read, Write};
        w.create("a.txt").unwrap().write_all(b"hi").unwrap();
        let mut s = String::new();
        w.open_read("./a.txt").unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s, "hi");
        w.rename("a.txt", "b.txt").unwrap();
        assert!(base.join("agent-files/b.txt").exists());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    #[cfg(unix)]
    fn escapes_are_refused() {
        let (base, w) = ws("esc");
        assert!(escaped(w.open_read("../outside/id_ed25519")));
        assert!(escaped(w.open_read("../../.ssh/id_ed25519")));
        assert!(escaped(w.create("sub/../../x")));
        let abs = base.join("outside/id_ed25519");
        assert!(escaped(w.open_read(abs.to_str().unwrap())));
        assert!(escaped(w.open_read("")));
        // A symlink inside the workspace pointing outside: cap-std refuses to follow it.
        std::os::unix::fs::symlink(base.join("outside"), base.join("agent-files/link")).unwrap();
        std::os::unix::fs::symlink(&abs, base.join("agent-files/key")).unwrap();
        assert!(escaped(w.open_read("link/id_ed25519")));
        assert!(escaped(w.open_read("key")));
        assert!(escaped(w.create("link/new")));
        assert!(!base.join("outside/new").exists());
        let _ = std::fs::remove_dir_all(base);
    }
}
