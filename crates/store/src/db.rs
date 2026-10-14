//! SQLCipher database (design D8): sessions, folders, tunnels, trust records, saved credentials,
//! agent policies, audit and history. File mode 0600, WAL, `user_version` migrations with a
//! pre-migration backup, 5 rolling backups paired with the envelope that unlocks them.

use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OptionalExtension, Row, params};
use rxt_core::{CertTrust, Error, Folder, FolderId, Result, Session, SessionId, Tunnel};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::envelope::{DbKey, ENVELOPE_FILE};

/// Database file name under the data directory.
pub const DB_FILE: &str = "rustxterm.db";
/// Backup directory under the data directory.
pub const BACKUP_DIR: &str = "backups";
/// Rolling backups kept (each paired with its envelope copy).
pub const MAX_BACKUPS: usize = 5;

/// Schema migrations; `user_version` = number applied. Append only, never edit.
pub const MIGRATIONS: &[&str] = &[
    r#"
CREATE TABLE folders (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    parent INTEGER REFERENCES folders(id) ON DELETE SET NULL
);
CREATE TABLE sessions (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    folder INTEGER REFERENCES folders(id) ON DELETE SET NULL,
    host TEXT NOT NULL,
    json TEXT NOT NULL
);
CREATE TABLE tunnels (
    via INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    json TEXT NOT NULL,
    PRIMARY KEY (via, name)
);
CREATE TABLE ssh_trust (
    host TEXT NOT NULL,
    port INTEGER NOT NULL,
    algorithm TEXT NOT NULL,
    key TEXT NOT NULL,
    added_at INTEGER NOT NULL,
    PRIMARY KEY (host, port, algorithm, key)
);
CREATE TABLE cert_trust (
    host TEXT NOT NULL,
    port INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    PRIMARY KEY (host, port)
);
CREATE TABLE credentials (
    session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    secret BLOB NOT NULL,
    PRIMARY KEY (session_id, kind)
);
CREATE TABLE policies (
    session_id INTEGER PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    policy TEXT NOT NULL,
    content_read INTEGER NOT NULL
);
CREATE TABLE audit (
    id INTEGER PRIMARY KEY,
    ts INTEGER NOT NULL,
    client TEXT NOT NULL,
    entry_point TEXT NOT NULL,
    command TEXT NOT NULL,
    session_id INTEGER,
    target TEXT NOT NULL,
    decision TEXT NOT NULL,
    result TEXT NOT NULL
);
CREATE TABLE history (
    id INTEGER PRIMARY KEY,
    ts INTEGER NOT NULL,
    session_id INTEGER,
    action TEXT NOT NULL
);
"#,
    "ALTER TABLE folders ADD COLUMN icon TEXT;",
];

/// Saved credential kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CredentialKind {
    Password,
    Passphrase,
}

impl CredentialKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::Passphrase => "passphrase",
        }
    }
}

/// Per-session agent policy (agent-control spec); one type shared with the engine.
pub use rxt_core::control::AgentPolicy;

fn policy_str(p: AgentPolicy) -> &'static str {
    match p {
        AgentPolicy::Deny => "deny",
        AgentPolicy::Ask => "ask",
        AgentPolicy::Allow => "allow",
    }
}

fn parse_policy(s: &str) -> AgentPolicy {
    match s {
        "ask" => AgentPolicy::Ask,
        "allow" => AgentPolicy::Allow,
        _ => AgentPolicy::Deny, // unknown values fail closed
    }
}

/// Agent policy plus the separate content-read flag. Default: deny, content-read off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Policy {
    pub policy: AgentPolicy,
    pub content_read: bool,
}

/// One agent audit entry. Holds no secret fields by construction.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AuditRecord {
    /// Unix seconds.
    pub ts: i64,
    /// Client name as declared (not authenticated).
    pub client: String,
    /// Entry point as serialized by `rxt_core::control::EntryPoint` ("cli-live", "cli-offline", "mcp-live", ...).
    pub entry_point: String,
    pub command: String,
    pub session: Option<SessionId>,
    pub target: String,
    /// "allowed", "approved", "denied".
    pub decision: String,
    /// "success" or an error message (never secrets).
    pub result: String,
}

/// One history entry (connections opened, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Unix seconds.
    pub ts: i64,
    pub session: Option<SessionId>,
    pub action: String,
}

/// Open, unlocked database.
pub struct Store {
    conn: Connection,
    dir: PathBuf,
    /// `x'<hex>'` key literal, needed to attach encrypted backups.
    key_literal: Zeroizing<String>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").field("dir", &self.dir).finish_non_exhaustive()
    }
}

/// Maps a database error. SQL never embeds secrets (keys and credentials are bound parameters
/// or handled by [`key_error`]), so the message is safe to show.
fn db(e: rusqlite::Error) -> Error {
    Error::Io(format!("database: {e}"))
}

fn json(e: serde_json::Error) -> Error {
    Error::Invalid(format!("database record: {e}"))
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Create `path` with mode 0600 if missing, then force 0600 (umask independent). On Windows the
/// file inherits the owner-only ACL of the data directory (`Paths::make_private`).
fn create_private(path: &Path) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?;
    set_mode(path, 0o600)
}

/// Unix permission bits; a no-op elsewhere (the directory ACL protects the files).
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(mode))?;
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

fn chmod_sidecars(db_path: &Path) -> Result<()> {
    for ext in ["-wal", "-shm"] {
        let mut p = db_path.as_os_str().to_owned();
        p.push(ext);
        let p = PathBuf::from(p);
        if p.exists() {
            set_mode(&p, 0o600)?;
        }
    }
    Ok(())
}

fn id_i64(id: u64) -> Result<i64> {
    i64::try_from(id).map_err(|_| Error::Invalid(format!("id {id} out of range")))
}

fn opt_id(id: Option<i64>) -> Option<u64> {
    id.map(|v| v as u64)
}

impl Store {
    /// Open (or create) `dir/rustxterm.db` with `key`, running pending migrations.
    /// A wrong key fails with [`Error::Auth`].
    pub fn open(dir: &Path, key: &DbKey) -> Result<Self> {
        Self::open_with_migrations(dir, key, MIGRATIONS)
    }

    /// [`Store::open`] with an explicit migration list (tests use it to simulate upgrades).
    #[doc(hidden)]
    pub fn open_with_migrations(dir: &Path, key: &DbKey, migrations: &[&str]) -> Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join(DB_FILE);
        create_private(&path)?;
        let conn = Connection::open(&path).map_err(db)?;

        let mut key_literal = Zeroizing::new(String::with_capacity(67));
        key_literal.push_str("x'");
        for b in key.0.iter() {
            let _ = write!(key_literal, "{b:02x}");
        }
        key_literal.push('\'');
        // Windows: SQLCipher's memory security VirtualLocks every allocation, which the default
        // working-set quota refuses (ERROR_WORKING_SET_QUOTA) and, in WAL mode, ends in a stack
        // overflow (add-windows-port W1 finding). Keys in Rust memory stay zeroized either way.
        #[cfg(not(windows))]
        pragma(&conn, "PRAGMA cipher_memory_security = ON").map_err(db)?;
        // The `x'..'` literal makes SQLCipher use the raw key (no PBKDF2). Built in a zeroizing
        // buffer; errors are replaced so the statement text never surfaces.
        let mut sql = Zeroizing::new(String::with_capacity(80));
        let _ = write!(sql, "PRAGMA key = \"{}\"", key_literal.as_str());
        pragma(&conn, &sql).map_err(|_| key_error())?;
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(())).map_err(|_| key_error())?;
        pragma(&conn, "PRAGMA journal_mode = WAL").map_err(db)?;
        pragma(&conn, "PRAGMA foreign_keys = ON").map_err(db)?;
        conn.busy_timeout(Duration::from_secs(5)).map_err(db)?;

        let mut store = Self { conn, dir: dir.to_owned(), key_literal };
        store.migrate(migrations)?;
        chmod_sidecars(&path)?;
        Ok(store)
    }

    fn migrate(&mut self, migrations: &[&str]) -> Result<()> {
        let version: i64 = self.conn.pragma_query_value(None, "user_version", |r| r.get(0)).map_err(db)?;
        let version = usize::try_from(version).unwrap_or(0);
        if version >= migrations.len() {
            return Ok(());
        }
        if version > 0 {
            self.backup()?;
        }
        // ponytail: all pending migrations share one transaction, so a failure rolls back to the
        // exact original; the pre-migration backup stays for manual restore.
        let tx = self.conn.transaction().map_err(db)?;
        for (i, sql) in migrations.iter().enumerate().skip(version) {
            tx.execute_batch(sql).map_err(|e| Error::Io(format!("migration {} failed: {e}", i + 1)))?;
        }
        tx.execute_batch(&format!("PRAGMA user_version = {}", migrations.len())).map_err(db)?;
        tx.commit().map_err(db)
    }

    /// Encrypted backup (`backups/rustxterm-<timestamp>.db`) with a byte copy of the envelope
    /// that unlocks it; keeps the newest [`MAX_BACKUPS`]. Returns the backup path.
    pub fn backup(&self) -> Result<PathBuf> {
        let dir = self.dir.join(BACKUP_DIR);
        fs::create_dir_all(&dir)?;
        set_mode(&dir, 0o700)?;
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let stem = format!("rustxterm-{stamp:020}");
        let path = dir.join(format!("{stem}.db"));
        create_private(&path)?;
        let target = path.to_str().ok_or_else(|| Error::Invalid("non-UTF-8 backup path".into()))?;
        // `sqlcipher_export` writes into an attached database encrypted with the same raw key.
        self.conn
            .execute("ATTACH DATABASE ?1 AS backup KEY ?2", params![target, self.key_literal.as_str()])
            .map_err(|_| Error::Io("cannot attach backup database".into()))?;
        let exported = self.conn.query_row("SELECT sqlcipher_export('backup')", [], |_| Ok(())).map_err(db);
        let detached = self.conn.execute_batch("DETACH DATABASE backup").map_err(db);
        exported.and(detached)?;
        set_mode(&path, 0o600)?;

        let envelope = self.dir.join(ENVELOPE_FILE);
        if envelope.exists() {
            let copy = dir.join(format!("{stem}.{ENVELOPE_FILE}"));
            create_private(&copy)?;
            fs::write(&copy, fs::read(&envelope)?)?;
        }
        prune_backups(&dir)?;
        Ok(path)
    }

    // ---- sessions ----

    pub fn sessions(&self) -> Result<Vec<Session>> {
        let mut stmt = self.conn.prepare("SELECT id, folder, json FROM sessions ORDER BY name, id").map_err(db)?;
        let rows = stmt.query_map([], session_row).map_err(db)?;
        rows.map(|r| r.map_err(db)?).collect()
    }

    pub fn session(&self, id: SessionId) -> Result<Session> {
        self.conn
            .query_row("SELECT id, folder, json FROM sessions WHERE id = ?1", [id_i64(id.0)?], session_row)
            .optional()
            .map_err(db)?
            .ok_or_else(|| Error::NotFound(format!("session {}", id.0)))?
    }

    /// Insert or update; assigns an id when `session.id` is 0 and returns it.
    /// New sessions have no policy row, i.e. [`Policy::default`] (deny, content-read off).
    pub fn save_session(&self, session: &Session) -> Result<SessionId> {
        let folder = session.folder.map(|f| id_i64(f.0)).transpose()?;
        let tx = self.conn.unchecked_transaction().map_err(db)?;
        let id = if session.id.0 == 0 {
            tx.execute(
                "INSERT INTO sessions (name, folder, host, json) VALUES (?1, ?2, ?3, '{}')",
                params![session.name, folder, session.host],
            )
            .map_err(db)?;
            tx.last_insert_rowid()
        } else {
            let id = id_i64(session.id.0)?;
            tx.execute(
                "INSERT INTO sessions (id, name, folder, host, json) VALUES (?1, ?2, ?3, ?4, '{}')
                 ON CONFLICT(id) DO UPDATE SET name = ?2, folder = ?3, host = ?4",
                params![id, session.name, folder, session.host],
            )
            .map_err(db)?;
            id
        };
        let stored = Session { id: SessionId(id as u64), ..session.clone() };
        let text = serde_json::to_string(&stored).map_err(json)?;
        tx.execute("UPDATE sessions SET json = ?1 WHERE id = ?2", params![text, id]).map_err(db)?;
        tx.commit().map_err(db)?;
        Ok(stored.id)
    }

    /// Delete a session with its tunnels, credentials and policy.
    pub fn delete_session(&self, id: SessionId) -> Result<()> {
        self.conn.execute("DELETE FROM sessions WHERE id = ?1", [id_i64(id.0)?]).map_err(db)?;
        Ok(())
    }

    /// Copy a session (not its credentials or policy) as "<name> (copy)"; returns the new id.
    pub fn duplicate_session(&self, id: SessionId) -> Result<SessionId> {
        let s = self.session(id)?;
        self.save_session(&Session { id: SessionId(0), name: format!("{} (copy)", s.name), ..s })
    }

    // ---- folders ----

    pub fn folders(&self) -> Result<Vec<Folder>> {
        let mut stmt = self.conn.prepare("SELECT id, name, parent, icon FROM folders ORDER BY name, id").map_err(db)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Folder {
                    id: FolderId(r.get::<_, i64>(0)? as u64),
                    name: r.get(1)?,
                    parent: opt_id(r.get(2)?).map(FolderId),
                    icon: r.get(3)?,
                })
            })
            .map_err(db)?;
        rows.collect::<rusqlite::Result<_>>().map_err(db)
    }

    /// Insert or update; assigns an id when `folder.id` is 0 and returns it.
    pub fn save_folder(&self, folder: &Folder) -> Result<FolderId> {
        let parent = folder.parent.map(|f| id_i64(f.0)).transpose()?;
        if folder.id.0 == 0 {
            self.conn
                .execute(
                    "INSERT INTO folders (name, parent, icon) VALUES (?1, ?2, ?3)",
                    params![folder.name, parent, folder.icon],
                )
                .map_err(db)?;
            return Ok(FolderId(self.conn.last_insert_rowid() as u64));
        }
        self.conn
            .execute(
                "INSERT INTO folders (id, name, parent, icon) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET name = ?2, parent = ?3, icon = ?4",
                params![id_i64(folder.id.0)?, folder.name, parent, folder.icon],
            )
            .map_err(db)?;
        Ok(folder.id)
    }

    /// Delete a folder; its sessions and subfolders move to its parent.
    pub fn delete_folder(&self, id: FolderId) -> Result<()> {
        let id = id_i64(id.0)?;
        let tx = self.conn.unchecked_transaction().map_err(db)?;
        let parent: Option<i64> = tx
            .query_row("SELECT parent FROM folders WHERE id = ?1", [id], |r| r.get(0))
            .optional()
            .map_err(db)?
            .flatten();
        tx.execute("UPDATE folders SET parent = ?1 WHERE parent = ?2", params![parent, id]).map_err(db)?;
        tx.execute("UPDATE sessions SET folder = ?1 WHERE folder = ?2", params![parent, id]).map_err(db)?;
        tx.execute("DELETE FROM folders WHERE id = ?1", [id]).map_err(db)?;
        tx.commit().map_err(db)
    }

    // ---- tunnels (keyed by session + name) ----

    pub fn tunnels(&self) -> Result<Vec<Tunnel>> {
        let mut stmt = self.conn.prepare("SELECT json FROM tunnels ORDER BY via, name").map_err(db)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(db)?;
        rows.map(|r| serde_json::from_str(&r.map_err(db)?).map_err(json)).collect()
    }

    /// Insert or replace the tunnel named `tunnel.name` on session `tunnel.via`.
    pub fn save_tunnel(&self, tunnel: &Tunnel) -> Result<()> {
        let text = serde_json::to_string(tunnel).map_err(json)?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO tunnels (via, name, json) VALUES (?1, ?2, ?3)",
                params![id_i64(tunnel.via.0)?, tunnel.name, text],
            )
            .map_err(db)?;
        Ok(())
    }

    pub fn delete_tunnel(&self, via: SessionId, name: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM tunnels WHERE via = ?1 AND name = ?2", params![id_i64(via.0)?, name])
            .map_err(db)?;
        Ok(())
    }

    // ---- trust records ----

    /// App-owned SSH host keys for `host:port` as `(algorithm, OpenSSH base64)`.
    pub fn ssh_trust_keys(&self, host: &str, port: u16) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT algorithm, key FROM ssh_trust WHERE host = ?1 AND port = ?2 ORDER BY added_at")
            .map_err(db)?;
        let rows =
            stmt.query_map(params![host.to_ascii_lowercase(), port], |r| Ok((r.get(0)?, r.get(1)?))).map_err(db)?;
        rows.collect::<rusqlite::Result<_>>().map_err(db)
    }

    /// Record an accepted host key ("accept and save"); duplicates are ignored.
    pub fn add_ssh_trust(&self, host: &str, port: u16, algorithm: &str, key_base64: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO ssh_trust (host, port, algorithm, key, added_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![host.to_ascii_lowercase(), port, algorithm, key_base64, now()],
            )
            .map_err(db)?;
        Ok(())
    }

    /// Trusted TLS certificate SHA-256 fingerprint for `host:port` (FTPS, RDP).
    pub fn cert_trust(&self, host: &str, port: u16) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT sha256 FROM cert_trust WHERE host = ?1 AND port = ?2",
                params![host.to_ascii_lowercase(), port],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)
    }

    pub fn save_cert_trust(&self, host: &str, port: u16, sha256: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO cert_trust (host, port, sha256) VALUES (?1, ?2, ?3)",
                params![host.to_ascii_lowercase(), port, sha256],
            )
            .map_err(db)?;
        Ok(())
    }

    // ---- saved credentials (only when the user ticked "save password") ----

    pub fn save_credential(&self, session: SessionId, kind: CredentialKind, secret: &SecretString) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO credentials (session_id, kind, secret) VALUES (?1, ?2, ?3)",
                params![id_i64(session.0)?, kind.as_str(), secret.expose_secret().as_bytes()],
            )
            .map_err(|_| Error::Io(format!("cannot save {} for session {}", kind.as_str(), session.0)))?;
        Ok(())
    }

    /// The saved secret, read straight from SQLite's buffer into a zeroizing box.
    pub fn credential(&self, session: SessionId, kind: CredentialKind) -> Result<Option<SecretString>> {
        self.conn
            .query_row(
                "SELECT secret FROM credentials WHERE session_id = ?1 AND kind = ?2",
                params![id_i64(session.0)?, kind.as_str()],
                |r| match r.get_ref(0)? {
                    ValueRef::Blob(b) | ValueRef::Text(b) => Ok(std::str::from_utf8(b).ok().map(SecretString::from)),
                    _ => Ok(None),
                },
            )
            .optional()
            .map(Option::flatten)
            .map_err(|_| Error::Io(format!("cannot read {} for session {}", kind.as_str(), session.0)))
    }

    /// Whether a secret of `kind` is saved for `session` (the secret itself is not read).
    pub fn has_credential(&self, session: SessionId, kind: CredentialKind) -> Result<bool> {
        self.conn
            .query_row(
                "SELECT 1 FROM credentials WHERE session_id = ?1 AND kind = ?2",
                params![id_i64(session.0)?, kind.as_str()],
                |_| Ok(()),
            )
            .optional()
            .map(|r| r.is_some())
            .map_err(db)
    }

    pub fn delete_credential(&self, session: SessionId, kind: CredentialKind) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM credentials WHERE session_id = ?1 AND kind = ?2",
                params![id_i64(session.0)?, kind.as_str()],
            )
            .map_err(db)?;
        Ok(())
    }

    // ---- agent policies ----

    /// Policy of `session`; [`Policy::default`] (deny, content-read off) when never set.
    pub fn policy(&self, session: SessionId) -> Result<Policy> {
        Ok(self
            .conn
            .query_row("SELECT policy, content_read FROM policies WHERE session_id = ?1", [id_i64(session.0)?], |r| {
                Ok(Policy { policy: parse_policy(&r.get::<_, String>(0)?), content_read: r.get(1)? })
            })
            .optional()
            .map_err(db)?
            .unwrap_or_default())
    }

    pub fn set_policy(&self, session: SessionId, policy: Policy) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO policies (session_id, policy, content_read) VALUES (?1, ?2, ?3)",
                params![id_i64(session.0)?, policy_str(policy.policy), policy.content_read],
            )
            .map_err(db)?;
        Ok(())
    }

    // ---- audit and history ----

    pub fn append_audit(&self, rec: &AuditRecord) -> Result<()> {
        let session = rec.session.map(|s| id_i64(s.0)).transpose()?;
        self.conn
            .execute(
                "INSERT INTO audit (ts, client, entry_point, command, session_id, target, decision, result)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    rec.ts,
                    rec.client,
                    rec.entry_point,
                    rec.command,
                    session,
                    rec.target,
                    rec.decision,
                    rec.result
                ],
            )
            .map_err(db)?;
        Ok(())
    }

    /// Audit entries, newest first.
    pub fn audit(&self, offset: usize, limit: usize) -> Result<Vec<AuditRecord>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT ts, client, entry_point, command, session_id, target, decision, result
                 FROM audit ORDER BY id DESC LIMIT ?1 OFFSET ?2",
            )
            .map_err(db)?;
        let rows = stmt
            .query_map(params![limit as i64, offset as i64], |r| {
                Ok(AuditRecord {
                    ts: r.get(0)?,
                    client: r.get(1)?,
                    entry_point: r.get(2)?,
                    command: r.get(3)?,
                    session: opt_id(r.get(4)?).map(SessionId),
                    target: r.get(5)?,
                    decision: r.get(6)?,
                    result: r.get(7)?,
                })
            })
            .map_err(db)?;
        rows.collect::<rusqlite::Result<_>>().map_err(db)
    }

    pub fn clear_audit(&self) -> Result<()> {
        self.conn.execute("DELETE FROM audit", []).map_err(db)?;
        Ok(())
    }

    pub fn append_history(&self, session: Option<SessionId>, action: &str) -> Result<()> {
        let session = session.map(|s| id_i64(s.0)).transpose()?;
        self.conn
            .execute(
                "INSERT INTO history (ts, session_id, action) VALUES (?1, ?2, ?3)",
                params![now(), session, action],
            )
            .map_err(db)?;
        Ok(())
    }

    /// History entries, newest first.
    pub fn history(&self, offset: usize, limit: usize) -> Result<Vec<HistoryEntry>> {
        let mut stmt = self
            .conn
            .prepare("SELECT ts, session_id, action FROM history ORDER BY id DESC LIMIT ?1 OFFSET ?2")
            .map_err(db)?;
        let rows = stmt
            .query_map(params![limit as i64, offset as i64], |r| {
                Ok(HistoryEntry { ts: r.get(0)?, session: opt_id(r.get(1)?).map(SessionId), action: r.get(2)? })
            })
            .map_err(db)?;
        rows.collect::<rusqlite::Result<_>>().map_err(db)
    }
}

/// Run a pragma, draining any result rows (SQLCipher pragmas may answer "ok").
fn pragma(conn: &Connection, sql: &str) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query([])?;
    while rows.next()?.is_some() {}
    Ok(())
}

fn key_error() -> Error {
    Error::Auth("database key rejected".into())
}

/// Decode a session row; columns (id, folder) win over the JSON copy.
fn session_row(r: &Row<'_>) -> rusqlite::Result<Result<Session>> {
    let id: i64 = r.get(0)?;
    let folder: Option<i64> = r.get(1)?;
    let text: String = r.get(2)?;
    Ok(serde_json::from_str::<Session>(&text).map_err(json).map(|s| Session {
        id: SessionId(id as u64),
        folder: opt_id(folder).map(FolderId),
        ..s
    }))
}

/// Keep the newest [`MAX_BACKUPS`] database backups and their envelope copies.
fn prune_backups(dir: &Path) -> Result<()> {
    let mut stems: Vec<String> = fs::read_dir(dir)?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".db").filter(|s| s.starts_with("rustxterm-")).map(str::to_owned))
        .collect();
    stems.sort(); // zero-padded timestamps sort chronologically
    let excess = stems.len().saturating_sub(MAX_BACKUPS);
    for stem in &stems[..excess] {
        fs::remove_file(dir.join(format!("{stem}.db")))?;
        let env = dir.join(format!("{stem}.{ENVELOPE_FILE}"));
        if env.exists() {
            fs::remove_file(env)?;
        }
    }
    Ok(())
}

/// Cloneable, thread-safe store handle for the engine and protocol layers.
#[derive(Clone, Debug)]
pub struct StoreHandle(pub Arc<Mutex<Store>>);

impl StoreHandle {
    pub fn new(store: Store) -> Self {
        Self(Arc::new(Mutex::new(store)))
    }

    pub fn lock(&self) -> MutexGuard<'_, Store> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// See [`Store::ssh_trust_keys`]; errors read as "no keys" (fails closed to a prompt).
    pub fn ssh_trust_keys(&self, host: &str, port: u16) -> Vec<(String, String)> {
        self.lock().ssh_trust_keys(host, port).unwrap_or_else(|e| {
            tracing::warn!("ssh trust lookup failed: {e}");
            Vec::new()
        })
    }

    pub fn add_ssh_trust(&self, host: &str, port: u16, algorithm: &str, key_base64: &str) {
        if let Err(e) = self.lock().add_ssh_trust(host, port, algorithm, key_base64) {
            tracing::warn!("cannot save ssh trust for {host}:{port}: {e}");
        }
    }
}

impl CertTrust for StoreHandle {
    fn get(&self, host: &str, port: u16) -> Option<String> {
        self.lock().cert_trust(host, port).unwrap_or_else(|e| {
            tracing::warn!("cert trust lookup failed: {e}");
            None
        })
    }

    fn save(&self, host: &str, port: u16, fingerprint: &str) {
        if let Err(e) = self.lock().save_cert_trust(host, port, fingerprint) {
            tracing::warn!("cannot save cert trust for {host}:{port}: {e}");
        }
    }
}
