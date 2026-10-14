//! `keys.v1` key envelope (design D8). The 256-bit database key exists only wrapped:
//! always under the master password (Argon2id + XChaCha20-Poly1305), optionally under a
//! random key kept in the platform keyring.
//!
//! Format (JSON): `{version: 1, kdf: {alg: "argon2id", m_kib, t, p, salt}, wraps: [{kind:
//! "master", nonce, ct}, {kind: "keyring", id, nonce, ct}?]}`; salt, nonces and ciphertexts are
//! standard base64. KDF parameters are bounded on read and write (memory 64–1024 MiB,
//! iterations 2–10, parallelism 1–4); every wrap uses a fresh random 24-byte nonce.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::Aead;
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use rxt_core::{Error, Result};
use rxt_platform::Keyring;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// Envelope file name next to the database.
pub const ENVELOPE_FILE: &str = "keys.v1";
/// Temporary file used for atomic replacement; a leftover copy is ignored and overwritten.
pub const ENVELOPE_TMP: &str = "keys.v1.tmp";
/// Consecutive wrong-password counter (drives the unlock delay).
pub const FAILURES_FILE: &str = "keys.v1.failures";

const KEYRING_ID_PREFIX: &str = "rustxterm-db-";
const MAX_DELAY: Duration = Duration::from_secs(30);

/// The raw database key; zeroized on drop, never logged or serialized.
pub struct DbKey(pub Zeroizing<[u8; 32]>);

impl fmt::Debug for DbKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DbKey(<redacted>)")
    }
}

/// Argon2id cost parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl KdfParams {
    /// Default for new envelopes: roughly 0.5–1 s per unlock on a typical machine.
    pub const DEFAULT: Self = Self { m_kib: 256 * 1024, t: 3, p: 1 };
    /// Lowest accepted cost (tests use it to stay fast).
    pub const MIN: Self = Self { m_kib: 64 * 1024, t: 2, p: 1 };

    fn check(self) -> Result<Self> {
        let ok =
            (64 * 1024..=1024 * 1024).contains(&self.m_kib) && (2..=10).contains(&self.t) && (1..=4).contains(&self.p);
        if ok { Ok(self) } else { Err(Error::Invalid(format!("key envelope: kdf parameters out of bounds: {self:?}"))) }
    }
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u32,
    kdf: Kdf,
    wraps: Vec<Wrap>,
}

#[derive(Serialize, Deserialize)]
struct Kdf {
    alg: String,
    m_kib: u32,
    t: u32,
    p: u32,
    salt: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Master,
    Keyring,
}

#[derive(Serialize, Deserialize, Clone)]
struct Wrap {
    kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    nonce: String,
    ct: String,
}

impl Kind {
    fn aad(self) -> &'static [u8] {
        match self {
            Kind::Master => b"rustxterm keys.v1 master",
            Kind::Keyring => b"rustxterm keys.v1 keyring",
        }
    }
}

impl Envelope {
    fn params(&self) -> KdfParams {
        KdfParams { m_kib: self.kdf.m_kib, t: self.kdf.t, p: self.kdf.p }
    }
    fn wrap(&self, kind: Kind) -> Option<&Wrap> {
        self.wraps.iter().find(|w| w.kind == kind)
    }
    fn set_wrap(&mut self, wrap: Option<Wrap>, kind: Kind) {
        self.wraps.retain(|w| w.kind != kind);
        self.wraps.extend(wrap);
    }
    fn validate(self) -> Result<Self> {
        let bad = |what: &str| Error::Invalid(format!("key envelope: {what}"));
        if self.version != 1 {
            return Err(bad("unsupported version"));
        }
        if self.kdf.alg != "argon2id" {
            return Err(bad("unsupported kdf"));
        }
        self.params().check()?;
        if decode(&self.kdf.salt)?.len() != 16 {
            return Err(bad("salt must be 16 bytes"));
        }
        let count = |k| self.wraps.iter().filter(|w| w.kind == k).count();
        if count(Kind::Master) != 1 || count(Kind::Keyring) > 1 {
            return Err(bad("expected one master wrap and at most one keyring wrap"));
        }
        for w in &self.wraps {
            let id_ok = match (w.kind, &w.id) {
                (Kind::Keyring, Some(id)) => id.starts_with(KEYRING_ID_PREFIX),
                (Kind::Master, None) => true,
                _ => false,
            };
            if !id_ok || decode(&w.nonce)?.len() != 24 || decode(&w.ct)?.len() != 48 {
                return Err(bad("malformed wrap"));
            }
        }
        Ok(self)
    }
}

fn decode(s: &str) -> Result<Vec<u8>> {
    B64.decode(s).map_err(|_| Error::Invalid("key envelope: bad base64".into()))
}

fn random<const N: usize>() -> Zeroizing<[u8; N]> {
    let mut b = Zeroizing::new([0u8; N]);
    rand::fill(&mut b[..]);
    b
}

fn derive(master: &SecretString, params: KdfParams, salt: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    let p = argon2::Params::new(params.m_kib, params.t, params.p, Some(32))
        .map_err(|e| Error::Invalid(format!("key envelope: kdf parameters: {e}")))?;
    let mut out = Zeroizing::new([0u8; 32]);
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, p)
        .hash_password_into(master.expose_secret().as_bytes(), salt, &mut out[..])
        .map_err(|e| Error::Io(format!("key envelope: kdf failed: {e}")))?;
    Ok(out)
}

fn seal(kind: Kind, id: Option<String>, wrapping: &[u8; 32], key: &DbKey) -> Result<Wrap> {
    let nonce = random::<24>();
    let ct = cipher(wrapping)?
        .encrypt(&XNonce::from(*nonce), chacha20poly1305::aead::Payload { msg: &key.0[..], aad: kind.aad() })
        .map_err(|_| Error::Io("key envelope: encryption failed".into()))?;
    Ok(Wrap { kind, id, nonce: B64.encode(&nonce[..]), ct: B64.encode(ct) })
}

/// `None` when the wrapping key does not authenticate the wrap.
fn open(wrap: &Wrap, wrapping: &[u8; 32]) -> Result<Option<DbKey>> {
    let nonce: [u8; 24] =
        decode(&wrap.nonce)?.try_into().map_err(|_| Error::Invalid("key envelope: malformed wrap".into()))?;
    let ct = decode(&wrap.ct)?;
    let payload = chacha20poly1305::aead::Payload { msg: &ct, aad: wrap.kind.aad() };
    let Ok(plain) = cipher(wrapping)?.decrypt(&XNonce::from(nonce), payload) else {
        return Ok(None);
    };
    let plain = Zeroizing::new(plain);
    let mut key = DbKey(Zeroizing::new([0u8; 32]));
    if plain.len() != 32 {
        return Err(Error::Invalid("key envelope: malformed wrap".into()));
    }
    key.0.copy_from_slice(&plain);
    Ok(Some(key))
}

fn cipher(key: &[u8; 32]) -> Result<XChaCha20Poly1305> {
    XChaCha20Poly1305::new_from_slice(key).map_err(|_| Error::Io("key envelope: bad key length".into()))
}

fn master_wrap(master: &SecretString, params: KdfParams, key: &DbKey) -> Result<(Kdf, Wrap)> {
    let params = params.check()?;
    let salt = random::<16>();
    let wrapping = derive(master, params, &salt[..])?;
    let kdf =
        Kdf { alg: "argon2id".into(), m_kib: params.m_kib, t: params.t, p: params.p, salt: B64.encode(&salt[..]) };
    Ok((kdf, seal(Kind::Master, None, &wrapping, key)?))
}

/// New random wrapping key stored in the keyring under a fresh id.
fn keyring_wrap(keyring: &dyn Keyring, key: &DbKey) -> Result<Wrap> {
    let id = format!("{KEYRING_ID_PREFIX}{}", random::<16>().iter().map(|b| format!("{b:02x}")).collect::<String>());
    let wrapping = random::<32>();
    keyring.set(&id, &wrapping[..])?;
    seal(Kind::Keyring, Some(id), &wrapping, key)
}

fn read(dir: &Path) -> Result<Envelope> {
    let text = fs::read(dir.join(ENVELOPE_FILE))?;
    serde_json::from_slice::<Envelope>(&text)
        .map_err(|_| Error::Invalid("key envelope: not valid JSON".into()))?
        .validate()
}

/// Atomic replacement: write the temp file, fsync, rename, fsync the directory (Unix).
fn write(dir: &Path, env: &Envelope) -> Result<()> {
    let json = serde_json::to_vec_pretty(env).map_err(|e| Error::Io(format!("key envelope: {e}")))?;
    let tmp = dir.join(ENVELOPE_TMP);
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut f = options.open(&tmp)?;
    f.write_all(&json)?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, dir.join(ENVELOPE_FILE))?;
    // Directory fsync makes the rename durable on Unix; Windows cannot open a directory as a
    // file and `MoveFileExW` replacement is already durable enough there.
    #[cfg(unix)]
    fs::File::open(dir)?.sync_all()?;
    Ok(())
}

fn failures(dir: &Path) -> u32 {
    fs::read_to_string(dir.join(FAILURES_FILE)).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

/// Delay applied after the `(n+1)`-th consecutive wrong password: `min(2^n * 250 ms, 30 s)`.
pub fn failure_delay(n: u32) -> Duration {
    Duration::from_millis(250u64.saturating_mul(1u64 << n.min(16))).min(MAX_DELAY)
}

/// Whether the envelope in `dir` has a keyring wrap (false when absent or unreadable).
pub fn has_keyring(dir: &Path) -> bool {
    read(dir).is_ok_and(|env| env.wraps.iter().any(|w| w.kind == Kind::Keyring))
}

/// Whether an envelope exists in `dir` (first start = no envelope).
pub fn exists(dir: &Path) -> bool {
    dir.join(ENVELOPE_FILE).exists()
}

/// First start: new random key wrapped under `master` (and the keyring when given).
pub fn create(dir: &Path, master: &SecretString, keyring: Option<&dyn Keyring>) -> Result<DbKey> {
    create_with_params(dir, master, keyring, KdfParams::DEFAULT)
}

/// [`create`] with explicit (bounded) KDF parameters. Refuses to replace an existing envelope;
/// a keyring error aborts before anything is written.
pub fn create_with_params(
    dir: &Path,
    master: &SecretString,
    keyring: Option<&dyn Keyring>,
    params: KdfParams,
) -> Result<DbKey> {
    if exists(dir) {
        return Err(Error::AlreadyExists("key envelope".into()));
    }
    let key = DbKey(random::<32>());
    let (kdf, master_wrap) = master_wrap(master, params, &key)?;
    let mut wraps = vec![master_wrap];
    if let Some(kr) = keyring {
        wraps.push(keyring_wrap(kr, &key)?);
    }
    fs::create_dir_all(dir)?;
    write(dir, &Envelope { version: 1, kdf, wraps })?;
    let _ = fs::remove_file(dir.join(FAILURES_FILE));
    Ok(key)
}

/// Unlock with the master password. Wrong password → `Error::Auth` after an increasing delay.
pub fn unlock_with_password(dir: &Path, master: &SecretString) -> Result<DbKey> {
    unlock_with_password_using(dir, master, &std::thread::sleep)
}

/// [`unlock_with_password`] with an injectable sleep (tests record the delay instead).
pub fn unlock_with_password_using(dir: &Path, master: &SecretString, sleep: &dyn Fn(Duration)) -> Result<DbKey> {
    let env = read(dir)?;
    let wrap = env.wrap(Kind::Master).ok_or_else(|| Error::Invalid("key envelope: no master wrap".into()))?;
    let wrapping = derive(master, env.params(), &decode(&env.kdf.salt)?)?;
    if let Some(key) = open(wrap, &wrapping)? {
        let _ = fs::remove_file(dir.join(FAILURES_FILE));
        return Ok(key);
    }
    let n = failures(dir);
    // Best effort: a read-only directory must not turn a wrong password into an I/O error.
    let _ = fs::write(dir.join(FAILURES_FILE), (n.saturating_add(1)).to_string());
    sleep(failure_delay(n));
    Err(Error::Auth("wrong master password".into()))
}

/// Unlock silently through the keyring wrap; `Ok(None)` when absent or unavailable.
pub fn unlock_with_keyring(dir: &Path, keyring: &dyn Keyring) -> Result<Option<DbKey>> {
    let env = read(dir)?;
    let Some(wrap) = env.wrap(Kind::Keyring) else { return Ok(None) };
    let id = wrap.id.as_deref().unwrap_or_default();
    let secret = match keyring.get(id) {
        Ok(Some(s)) => Zeroizing::new(s),
        Ok(None) => return Ok(None),
        Err(e) => {
            tracing::warn!("keyring unavailable, falling back to master password: {e}");
            return Ok(None);
        }
    };
    let Ok(wrapping) = <[u8; 32]>::try_from(&secret[..]) else { return Ok(None) };
    let wrapping = Zeroizing::new(wrapping);
    open(wrap, &wrapping)
}

/// Re-wrap under a new master password (atomic replace: write, fsync, rename, fsync dir).
/// The database key and the KDF cost stay the same; salt and nonce are fresh.
pub fn change_password(dir: &Path, key: &DbKey, new_master: &SecretString) -> Result<()> {
    let mut env = read(dir)?;
    let (kdf, wrap) = master_wrap(new_master, env.params(), key)?;
    env.kdf = kdf;
    env.set_wrap(Some(wrap), Kind::Master);
    write(dir, &env)
}

/// Add (`Some`) or remove (`None`) the keyring wrap. A previous keyring secret is deleted
/// (best effort) after the new envelope is in place.
pub fn set_keyring(dir: &Path, key: &DbKey, keyring: Option<&dyn Keyring>) -> Result<()> {
    set_keyring_in(dir, key, keyring, keyring)
}

/// [`set_keyring`] removal with the keyring that holds the old secret.
pub fn remove_keyring(dir: &Path, key: &DbKey, keyring: &dyn Keyring) -> Result<()> {
    set_keyring_in(dir, key, None, Some(keyring))
}

fn set_keyring_in(dir: &Path, key: &DbKey, new: Option<&dyn Keyring>, old: Option<&dyn Keyring>) -> Result<()> {
    let mut env = read(dir)?;
    let old_id = env.wrap(Kind::Keyring).and_then(|w| w.id.clone());
    let wrap = new.map(|kr| keyring_wrap(kr, key)).transpose()?;
    env.set_wrap(wrap, Kind::Keyring);
    write(dir, &env)?;
    if let (Some(kr), Some(id)) = (old, old_id)
        && let Err(e) = kr.delete(&id)
    {
        tracing::warn!("could not delete the old keyring secret: {e}");
    }
    Ok(())
}
