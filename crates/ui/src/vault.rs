//! The encrypted session database behind the `keys.v1` envelope (design D8), shared by the UI
//! and the engine. Locked until the master password (or the keyring) unlocks it; while locked,
//! engine lookups find nothing and saves are dropped (trust stays in the engine's memory).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rxt_core::{PromptKind, Result, Secret, Session};
use rxt_platform::Keyring;
use rxt_store::db::{CredentialKind, Store};
use rxt_store::envelope::{self, DbKey, KdfParams};
use secrecy::{ExposeSecret, SecretString};
use zeroize::Zeroizing;

/// Data directory, keyring and the open database once unlocked.
pub struct Vault {
    dir: PathBuf,
    keyring: Arc<dyn Keyring>,
    kdf: KdfParams,
    open: Mutex<Option<(Store, DbKey)>>,
}

impl Vault {
    /// Vault in `dir` (the data directory); nothing is read until unlock.
    pub fn new(dir: PathBuf, keyring: Arc<dyn Keyring>) -> Self {
        Self { dir, keyring, kdf: KdfParams::DEFAULT, open: Mutex::new(None) }
    }

    /// Vault whose new envelope uses the cheapest accepted KDF cost (tests, self-test).
    pub fn fast(dir: PathBuf, keyring: Arc<dyn Keyring>) -> Self {
        Self { kdf: KdfParams::MIN, ..Self::new(dir, keyring) }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// First start: no envelope yet.
    pub fn is_new(&self) -> bool {
        !envelope::exists(&self.dir)
    }

    pub fn is_unlocked(&self) -> bool {
        self.guard().is_some()
    }

    fn guard(&self) -> MutexGuard<'_, Option<(Store, DbKey)>> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Create the envelope and the database (first start). Blocking (Argon2).
    pub fn create(&self, master: &str, keyring: bool) -> Result<()> {
        let master = SecretString::from(master);
        let kr = keyring.then_some(self.keyring.as_ref());
        let key = envelope::create_with_params(&self.dir, &master, kr, self.kdf)?;
        self.open_with(key)
    }

    /// Unlock with the master password; wrong passwords fail after the envelope's delay. Blocking.
    pub fn unlock(&self, master: &str) -> Result<()> {
        let key = envelope::unlock_with_password(&self.dir, &SecretString::from(master))?;
        self.open_with(key)
    }

    /// Unlock through the keyring wrap; `Ok(false)` when absent or unavailable. Blocking.
    pub fn unlock_with_keyring(&self) -> Result<bool> {
        match envelope::unlock_with_keyring(&self.dir, self.keyring.as_ref())? {
            Some(key) => self.open_with(key).map(|()| true),
            None => Ok(false),
        }
    }

    fn open_with(&self, key: DbKey) -> Result<()> {
        let store = Store::open(&self.dir, &key)?;
        *self.guard() = Some((store, key));
        Ok(())
    }

    /// Copy of the key, so slow envelope work does not hold the lock.
    fn key(&self) -> Result<DbKey> {
        let guard = self.guard();
        let (_, key) = guard.as_ref().ok_or_else(locked)?;
        Ok(DbKey(Zeroizing::new(*key.0)))
    }

    /// Re-wrap the key under a new master password. Blocking (Argon2).
    pub fn change_password(&self, master: &str) -> Result<()> {
        envelope::change_password(&self.dir, &self.key()?, &SecretString::from(master))
    }

    /// Whether the envelope holds a keyring wrap.
    pub fn keyring_enabled(&self) -> bool {
        envelope::has_keyring(&self.dir)
    }

    /// Add or remove the keyring wrap.
    pub fn set_keyring(&self, on: bool) -> Result<()> {
        let key = self.key()?;
        if on {
            envelope::set_keyring(&self.dir, &key, Some(self.keyring.as_ref()))
        } else {
            envelope::remove_keyring(&self.dir, &key, self.keyring.as_ref())
        }
    }

    /// Run `f` on the open database; `Err` when locked.
    pub fn with<R>(&self, f: impl FnOnce(&Store) -> Result<R>) -> Result<R> {
        let guard = self.guard();
        let (store, _) = guard.as_ref().ok_or_else(locked)?;
        f(store)
    }

    /// Like [`Vault::with`] for best-effort engine calls: errors are logged, `None` when locked.
    fn quiet<R>(&self, what: &str, f: impl FnOnce(&Store) -> Result<R>) -> Option<R> {
        let guard = self.guard();
        let (store, _) = guard.as_ref()?;
        f(store).map_err(|e| tracing::warn!("{what}: {e}")).ok()
    }
}

fn locked() -> rxt_core::Error {
    rxt_core::Error::Auth("the session database is locked".into())
}

/// Saved-credential slot answering `kind` for `session`. Passwords are saved for the session's
/// own host only (jump hosts and gateways keep prompting); unsaved sessions (id 0) have none.
fn slot(session: &Session, kind: &PromptKind) -> Option<CredentialKind> {
    if session.id.0 == 0 {
        return None;
    }
    match kind {
        PromptKind::Password { host, .. } if host.eq_ignore_ascii_case(&session.host) => Some(CredentialKind::Password),
        // ponytail: one passphrase per session; key per path if sessions ever hold several keys.
        PromptKind::KeyPassphrase { .. } => Some(CredentialKind::Passphrase),
        _ => None,
    }
}

impl rxt_engine::Records for Vault {
    fn store(&self, f: &mut dyn FnMut(&rxt_store::db::Store) -> Result<()>) -> Result<()> {
        self.with(|s| f(s))
    }

    fn credential(&self, session: &Session, kind: &PromptKind) -> Option<Secret> {
        let slot = slot(session, kind)?;
        let secret = self.quiet("saved credential", |s| s.credential(session.id, slot))??;
        Some(Secret(secret.expose_secret().to_owned()))
    }

    fn save_credential(&self, session: &Session, kind: &PromptKind, secret: &Secret) {
        if let Some(slot) = slot(session, kind) {
            let secret = SecretString::from(secret.0.as_str());
            self.quiet("save credential", |s| s.save_credential(session.id, slot, &secret));
        }
    }

    fn forget_credential(&self, session: &Session, kind: &PromptKind) {
        if let Some(slot) = slot(session, kind) {
            self.quiet("forget credential", |s| s.delete_credential(session.id, slot));
        }
    }

    fn host_keys(&self, host: &str, port: u16) -> Vec<String> {
        let keys = self.quiet("host keys", |s| s.ssh_trust_keys(host, port)).unwrap_or_default();
        keys.into_iter().map(|(alg, b64)| format!("{alg} {b64}")).collect()
    }

    fn save_host_key(&self, host: &str, port: u16, key: &str) {
        let mut parts = key.split_whitespace();
        if let (Some(alg), Some(b64)) = (parts.next(), parts.next()) {
            self.quiet("save host key", |s| s.add_ssh_trust(host, port, alg, b64));
        }
    }

    fn certificate(&self, host: &str, port: u16) -> Option<String> {
        self.quiet("certificate", |s| s.cert_trust(host, port))?
    }

    fn save_certificate(&self, host: &str, port: u16, fingerprint: &str) {
        self.quiet("save certificate", |s| s.save_cert_trust(host, port, fingerprint));
    }
}
