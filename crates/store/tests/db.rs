//! Encrypted store integration tests (tasks 6.2, 6.4, 7.1 storage parts).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use rxt_core::{CertTrust, Error, Folder, Protocol, Session, SessionId, Tunnel};
use rxt_store::db::{
    AgentPolicy, AuditRecord, BACKUP_DIR, CredentialKind, DB_FILE, MAX_BACKUPS, MIGRATIONS, Policy, Store, StoreHandle,
};
use rxt_store::envelope::{DbKey, ENVELOPE_FILE};
use secrecy::{ExposeSecret, SecretString};
use zeroize::Zeroizing;

const HOST: &str = "prod-db.example.internal";
const PASSWORD: &str = "Tr0ub4dor-fictional-pw";

fn key() -> DbKey {
    DbKey(Zeroizing::new([7u8; 32]))
}

/// Fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("rxt-store-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn backups(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir.join(BACKUP_DIR))
        .map(|rd| rd.map(|e| e.unwrap().file_name().into_string().unwrap()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Unix permission bits; on Windows files carry no mode (the data directory ACL protects them),
/// so only existence is checked there.
fn mode(p: &Path) -> u32 {
    let meta = std::fs::metadata(p).unwrap();
    #[cfg(unix)]
    return std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o777;
    #[cfg(not(unix))]
    {
        assert!(meta.is_file());
        0o600
    }
}

#[test]
fn create_open_wrong_key_and_mode() {
    let t = TempDir::new("open");
    let store = Store::open(&t.0, &key()).unwrap();
    store.save_session(&Session::new(Protocol::Ssh, "h", None, None)).unwrap();
    let db = t.0.join(DB_FILE);
    assert_eq!(mode(&db), 0o600);
    for side in ["rustxterm.db-wal", "rustxterm.db-shm"] {
        let p = t.0.join(side);
        if p.exists() {
            assert_eq!(mode(&p), 0o600, "{side}");
        }
    }
    drop(store);

    let err = Store::open(&t.0, &DbKey(Zeroizing::new([8u8; 32]))).unwrap_err();
    assert!(matches!(err, Error::Auth(_)), "{err:?}");
    assert_eq!(Store::open(&t.0, &key()).unwrap().sessions().unwrap().len(), 1);
}

#[test]
fn local_terminal_is_never_saved() {
    let t = TempDir::new("local");
    let s = Store::open(&t.0, &key()).unwrap();
    let err = s.save_session(&Session::new(Protocol::Local, "workstation", None, None)).unwrap_err();
    assert!(matches!(err, Error::Invalid(_)), "{err:?}");
    assert!(s.sessions().unwrap().is_empty());
}

#[test]
fn crud_round_trips() {
    let t = TempDir::new("crud");
    let s = Store::open(&t.0, &key()).unwrap();

    let parent = s.save_folder(&Folder { name: "prod".into(), ..Folder::default() }).unwrap();
    let child = s.save_folder(&Folder { name: "db".into(), parent: Some(parent), ..Folder::default() }).unwrap();
    let mut sess = Session::new(Protocol::Ssh, HOST, Some("alice".into()), Some(2222));
    sess.folder = Some(child);
    let id = s.save_session(&sess).unwrap();
    assert_ne!(id, SessionId(0));
    let loaded = s.session(id).unwrap();
    assert_eq!(loaded, Session { id, ..sess.clone() });

    // update
    s.save_session(&Session { name: "renamed".into(), ..loaded }).unwrap();
    assert_eq!(s.session(id).unwrap().name, "renamed");

    let dup = s.duplicate_session(id).unwrap();
    assert_ne!(dup, id);
    assert_eq!(s.session(dup).unwrap().name, "renamed (copy)");
    assert_eq!(s.sessions().unwrap().len(), 2);

    // deleting a folder moves its children to the parent
    s.delete_folder(child).unwrap();
    assert_eq!(s.session(id).unwrap().folder, Some(parent));
    assert_eq!(s.folders().unwrap(), vec![Folder { id: parent, name: "prod".into(), parent: None, icon: None }]);

    let tun = Tunnel {
        name: "pg".into(),
        via: id,
        bind_port: 15432,
        dest_host: "db".into(),
        dest_port: 5432,
        ..Tunnel::default()
    };
    s.save_tunnel(&tun).unwrap();
    s.save_tunnel(&Tunnel { bind_port: 15433, ..tun.clone() }).unwrap();
    assert_eq!(s.tunnels().unwrap(), vec![Tunnel { bind_port: 15433, ..tun.clone() }]);
    s.delete_tunnel(id, "pg").unwrap();
    assert!(s.tunnels().unwrap().is_empty());

    s.add_ssh_trust("Host", 22, "ssh-ed25519", "AAAAC3Nza").unwrap();
    s.add_ssh_trust("host", 22, "ssh-ed25519", "AAAAC3Nza").unwrap();
    assert_eq!(s.ssh_trust_keys("HOST", 22).unwrap(), vec![("ssh-ed25519".into(), "AAAAC3Nza".into())]);
    assert!(s.ssh_trust_keys("host", 2222).unwrap().is_empty());

    assert_eq!(s.policy(id).unwrap(), Policy { policy: AgentPolicy::Deny, content_read: false });
    let p = Policy { policy: AgentPolicy::Ask, content_read: true };
    s.set_policy(id, p).unwrap();
    assert_eq!(s.policy(id).unwrap(), p);

    for i in 0..3 {
        s.append_audit(&AuditRecord { ts: i, client: "c".into(), command: format!("cmd{i}"), ..Default::default() })
            .unwrap();
    }
    let page = s.audit(1, 1).unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].command, "cmd1");
    s.clear_audit().unwrap();
    assert!(s.audit(0, 10).unwrap().is_empty());

    s.append_history(Some(id), "open").unwrap();
    assert_eq!(s.history(0, 10).unwrap()[0].session, Some(id));

    s.save_credential(id, CredentialKind::Password, &SecretString::from(PASSWORD)).unwrap();
    let got = s.credential(id, CredentialKind::Password).unwrap().unwrap();
    assert_eq!(got.expose_secret(), PASSWORD);
    assert!(s.credential(id, CredentialKind::Passphrase).unwrap().is_none());

    // deleting the session cascades to tunnels, credentials and policy
    s.save_tunnel(&tun).unwrap();
    s.delete_session(id).unwrap();
    assert!(s.credential(id, CredentialKind::Password).unwrap().is_none());
    assert_eq!(s.policy(id).unwrap(), Policy::default());
    assert!(s.tunnels().unwrap().is_empty());
    assert!(matches!(s.session(id), Err(Error::NotFound(_))));
}

#[test]
fn cert_trust_through_handle() {
    let t = TempDir::new("cert");
    let h = StoreHandle::new(Store::open(&t.0, &key()).unwrap());
    assert_eq!(h.get("rdp.example", 3389), None);
    h.save("RDP.example", 3389, "SHA256:abc");
    assert_eq!(h.get("rdp.example", 3389).as_deref(), Some("SHA256:abc"));
    h.add_ssh_trust("h", 22, "ssh-rsa", "AAAAB3");
    assert_eq!(h.ssh_trust_keys("h", 22).len(), 1);
}

#[test]
fn secrets_never_in_debug_or_errors() {
    let t = TempDir::new("redact");
    let s = Store::open(&t.0, &key()).unwrap();
    let id = s.save_session(&Session::new(Protocol::Ssh, HOST, None, None)).unwrap();
    let pw = SecretString::from(PASSWORD);
    s.save_credential(id, CredentialKind::Password, &pw).unwrap();
    let got = s.credential(id, CredentialKind::Password).unwrap();
    // unknown session: foreign key error
    let err = s.save_credential(SessionId(999), CredentialKind::Password, &pw).unwrap_err();
    let dumps = [
        format!("{s:?}"),
        format!("{got:?}"),
        format!("{pw:?}"),
        format!("{err:?} {err}"),
        format!("{:?}", s.policy(id).unwrap()),
        format!("{:?}", s.audit(0, 10).unwrap()),
        format!("{:?}", StoreHandle::new(s)),
    ];
    for d in dumps {
        assert!(!d.contains(PASSWORD), "{d}");
    }
}

fn scan(dir: &Path, needles: &[&str]) {
    let mut files = vec![];
    for d in [dir.to_path_buf(), dir.join(BACKUP_DIR)] {
        for e in std::fs::read_dir(d).into_iter().flatten() {
            let p = e.unwrap().path();
            if p.is_file() {
                files.push(p);
            }
        }
    }
    assert!(files.iter().any(|p| p.ends_with(DB_FILE)));
    for p in files {
        let bytes = std::fs::read(&p).unwrap();
        for n in needles {
            assert!(!bytes.windows(n.len()).any(|w| w == n.as_bytes()), "{n} found in {}", p.display());
        }
    }
}

#[test]
fn plaintext_scan() {
    let t = TempDir::new("scan");
    let s = Store::open(&t.0, &key()).unwrap();
    let id = s.save_session(&Session::new(Protocol::Ssh, HOST, None, None)).unwrap();
    s.save_credential(id, CredentialKind::Password, &SecretString::from(PASSWORD)).unwrap();
    s.backup().unwrap();
    scan(&t.0, &[HOST, PASSWORD]); // while open: -wal holds the pages
    drop(s);
    scan(&t.0, &[HOST, PASSWORD]);
}

#[test]
fn migration_upgrade_backup_and_rollback() {
    let t = TempDir::new("migrate");
    std::fs::write(t.0.join(ENVELOPE_FILE), b"envelope-v1").unwrap();
    let s = Store::open(&t.0, &key()).unwrap();
    let id = s.save_session(&Session::new(Protocol::Ssh, "keep", None, None)).unwrap();
    drop(s);
    assert!(backups(&t.0).is_empty(), "fresh database needs no backup");

    // failing N -> N+1: original kept, backup taken
    let mut bad = MIGRATIONS.to_vec();
    bad.push("CREATE TABLE extra (a); INSERT INTO nope VALUES (1);");
    assert!(Store::open_with_migrations(&t.0, &key(), &bad).is_err());
    assert_eq!(backups(&t.0).len(), 2); // .db + keys.v1 copy
    let conn = raw(&t.0);
    assert_eq!(user_version(&conn), MIGRATIONS.len() as i64);
    assert!(conn.prepare("SELECT * FROM extra").is_err());
    drop(conn);

    // the backup is encrypted with the same key and holds the data
    let b = backups(&t.0);
    let bak = b.iter().find(|n| n.ends_with(".db")).unwrap();
    assert_eq!(
        std::fs::read(t.0.join(BACKUP_DIR).join(b.iter().find(|n| n.ends_with(ENVELOPE_FILE)).unwrap())).unwrap(),
        b"envelope-v1"
    );
    assert_eq!(mode(&t.0.join(BACKUP_DIR).join(bak)), 0o600);
    let bconn = rusqlite::Connection::open(t.0.join(BACKUP_DIR).join(bak)).unwrap();
    assert!(bconn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0)).is_err());
    bconn.execute_batch(&format!("PRAGMA key = \"x'{}'\"", "07".repeat(32))).unwrap();
    assert_eq!(bconn.query_row("SELECT count(*) FROM sessions", [], |r| r.get::<_, i64>(0)).unwrap(), 1);

    // successful N -> N+1
    let mut good = MIGRATIONS.to_vec();
    good.push("CREATE TABLE extra (a);");
    let s = Store::open_with_migrations(&t.0, &key(), &good).unwrap();
    assert_eq!(s.session(id).unwrap().name, "keep");
    drop(s);
    assert_eq!(user_version(&raw(&t.0)), good.len() as i64);
    assert_eq!(backups(&t.0).len(), 4);
}

#[test]
fn rolling_backups_capped() {
    let t = TempDir::new("rolling");
    std::fs::write(t.0.join(ENVELOPE_FILE), b"env").unwrap();
    let s = Store::open(&t.0, &key()).unwrap();
    for _ in 0..MAX_BACKUPS + 3 {
        s.backup().unwrap();
    }
    let b = backups(&t.0);
    assert_eq!(b.iter().filter(|n| n.ends_with(".db")).count(), MAX_BACKUPS);
    for db in b.iter().filter(|n| n.ends_with(".db")) {
        let env = format!("{}.{ENVELOPE_FILE}", db.trim_end_matches(".db"));
        assert!(b.contains(&env), "missing {env}");
    }
    assert_eq!(b.len(), 2 * MAX_BACKUPS);
}

#[test]
fn folder_icon_migration_keeps_old_folders() {
    let t = TempDir::new("folder-icon");
    std::fs::write(t.0.join(ENVELOPE_FILE), b"env").unwrap();
    drop(Store::open_with_migrations(&t.0, &key(), &MIGRATIONS[..1]).unwrap());
    raw(&t.0).execute("INSERT INTO folders (name) VALUES ('old')", []).unwrap();
    let s = Store::open(&t.0, &key()).unwrap();
    let mut old = s.folders().unwrap().remove(0);
    assert_eq!((old.name.as_str(), old.icon.as_deref()), ("old", None));
    old.icon = Some("folder-blue".into());
    s.save_folder(&old).unwrap();
    assert_eq!(s.folders().unwrap(), vec![old]);
    assert_eq!(backups(&t.0).iter().filter(|n| n.ends_with(".db")).count(), 1, "pre-migration backup");
}

fn raw(dir: &Path) -> rusqlite::Connection {
    let c = rusqlite::Connection::open(dir.join(DB_FILE)).unwrap();
    c.execute_batch(&format!("PRAGMA key = \"x'{}'\"", "07".repeat(32))).unwrap();
    c
}

fn user_version(c: &rusqlite::Connection) -> i64 {
    c.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap()
}

const CHILD_ENV: &str = "RXT_STORE_KILL_CHILD_DIR";

/// Child half of `kill_during_write`: writes forever until killed.
#[test]
fn kill_child_writer() {
    let Some(dir) = std::env::var_os(CHILD_ENV) else { return };
    let s = Store::open(Path::new(&dir), &key()).unwrap();
    std::fs::write(Path::new(&dir).join("ready"), b"").unwrap();
    for i in 0..u64::MAX {
        let id = s.save_session(&Session::new(Protocol::Ssh, format!("h{i}"), None, None)).unwrap();
        s.save_credential(id, CredentialKind::Password, &SecretString::from("x")).unwrap();
    }
}

#[test]
fn kill_during_write() {
    let t = TempDir::new("kill");
    drop(Store::open(&t.0, &key()).unwrap());
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "kill_child_writer", "--nocapture"])
        .env(CHILD_ENV, &t.0)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..200 {
        if t.0.join("ready").exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    std::thread::sleep(Duration::from_millis(300));
    child.kill().unwrap();
    child.wait().unwrap();

    let s = Store::open(&t.0, &key()).unwrap();
    let sessions = s.sessions().unwrap();
    assert!(!sessions.is_empty(), "child never wrote");
    drop(s);
    let c = raw(&t.0);
    let ok: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
    assert_eq!(ok, "ok");
}
