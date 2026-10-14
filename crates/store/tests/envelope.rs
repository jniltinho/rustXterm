//! Key envelope (task 6.3). Uses the minimum KDF cost to stay fast.

#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use rxt_platform::fake::Fake;
use rxt_platform::{Error, Keyring, Result};
use rxt_store::envelope::{self, DbKey, ENVELOPE_FILE, ENVELOPE_TMP, FAILURES_FILE, KdfParams};
use secrecy::SecretString;

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("rxt-envelope-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn pw(s: &str) -> SecretString {
    SecretString::from(s.to_owned())
}

fn create(dir: &std::path::Path, master: &str, kr: Option<&dyn Keyring>) -> DbKey {
    envelope::create_with_params(dir, &pw(master), kr, KdfParams::MIN).unwrap()
}

/// Unlock without sleeping; returns the result and the recorded delays.
fn unlock(dir: &std::path::Path, master: &str) -> (Result<DbKey>, Vec<Duration>) {
    let slept = RefCell::new(Vec::new());
    let r = envelope::unlock_with_password_using(dir, &pw(master), &|d| slept.borrow_mut().push(d));
    (r, slept.into_inner())
}

fn json(dir: &std::path::Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(dir.join(ENVELOPE_FILE)).unwrap()).unwrap()
}

fn put(dir: &std::path::Path, v: &serde_json::Value) {
    fs::write(dir.join(ENVELOPE_FILE), serde_json::to_vec(v).unwrap()).unwrap();
}

struct NoKeyring;

impl Keyring for NoKeyring {
    fn get(&self, _: &str) -> Result<Option<Vec<u8>>> {
        Err(Error::Unsupported("no keyring".into()))
    }
    fn set(&self, _: &str, _: &[u8]) -> Result<()> {
        Err(Error::Unsupported("no keyring".into()))
    }
    fn delete(&self, _: &str) -> Result<()> {
        Err(Error::Unsupported("no keyring".into()))
    }
}

#[test]
fn create_then_unlock_and_format() {
    let dir = tmpdir("create");
    assert!(!envelope::exists(&dir));
    let key = create(&dir, "correct horse", None);
    assert!(envelope::exists(&dir));
    let (got, slept) = unlock(&dir, "correct horse");
    assert_eq!(*got.unwrap().0, *key.0);
    assert!(slept.is_empty());

    let v = json(&dir);
    assert_eq!(v["version"], 1);
    assert_eq!(v["kdf"]["alg"], "argon2id");
    assert_eq!(v["kdf"]["m_kib"], 64 * 1024);
    assert_eq!(v["wraps"].as_array().unwrap().len(), 1);
    assert_eq!(v["wraps"][0]["kind"], "master");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(dir.join(ENVELOPE_FILE)).unwrap().permissions().mode() & 0o777, 0o600);
    }

    // Never silently replaces an existing key.
    let again = envelope::create_with_params(&dir, &pw("x"), None, KdfParams::MIN);
    assert!(matches!(again, Err(Error::AlreadyExists(_))));
}

#[test]
fn wrong_password_delays_increase_and_reset() {
    let dir = tmpdir("wrong");
    create(&dir, "right", None);
    let mut delays = Vec::new();
    for _ in 0..3 {
        let (r, slept) = unlock(&dir, "wrong");
        let e = r.unwrap_err();
        assert!(matches!(e, Error::Auth(_)));
        delays.extend(slept);
    }
    let ms: Vec<u128> = delays.iter().map(Duration::as_millis).collect();
    assert_eq!(ms, [250, 500, 1000]);
    assert_eq!(fs::read_to_string(dir.join(FAILURES_FILE)).unwrap(), "3");
    assert_eq!(envelope::failure_delay(20), Duration::from_secs(30));

    assert!(unlock(&dir, "right").0.is_ok());
    assert!(!dir.join(FAILURES_FILE).exists());
    assert_eq!(unlock(&dir, "wrong").1, [Duration::from_millis(250)]);
}

#[test]
fn kdf_bounds_rejected_on_write_and_read() {
    let dir = tmpdir("bounds");
    for p in [
        KdfParams { m_kib: 32 * 1024, ..KdfParams::MIN },
        KdfParams { m_kib: 1025 * 1024, ..KdfParams::MIN },
        KdfParams { t: 1, ..KdfParams::MIN },
        KdfParams { t: 11, ..KdfParams::MIN },
        KdfParams { p: 0, ..KdfParams::MIN },
        KdfParams { p: 5, ..KdfParams::MIN },
    ] {
        let r = envelope::create_with_params(&dir, &pw("x"), None, p);
        assert!(matches!(r, Err(Error::Invalid(_))), "{p:?}");
        assert!(!envelope::exists(&dir));
    }

    create(&dir, "x", None);
    let good = json(&dir);
    for (field, value) in [("m_kib", 4u64 << 20), ("m_kib", 1024), ("t", 100), ("t", 1), ("p", 64)] {
        let mut v = good.clone();
        v["kdf"][field] = value.into();
        put(&dir, &v);
        // Rejected before any expensive derivation and without counting a failure.
        let (r, slept) = unlock(&dir, "x");
        assert!(matches!(r, Err(Error::Invalid(_))), "{field}={value}");
        assert!(slept.is_empty());
    }
    let mut v = good.clone();
    v["version"] = 2.into();
    put(&dir, &v);
    assert!(matches!(unlock(&dir, "x").0, Err(Error::Invalid(_))));
    let mut v = good;
    v["wraps"] = serde_json::json!([]);
    put(&dir, &v);
    assert!(matches!(unlock(&dir, "x").0, Err(Error::Invalid(_))));
}

#[test]
fn nonces_and_salts_are_fresh() {
    let dir = tmpdir("nonce");
    let fake = Fake::default();
    let key = create(&dir, "a", Some(&fake));
    let mut nonces = Vec::new();
    let mut salts = Vec::new();
    for i in 0..4 {
        let v = json(&dir);
        salts.push(v["kdf"]["salt"].as_str().unwrap().to_owned());
        for w in v["wraps"].as_array().unwrap() {
            nonces.push(w["nonce"].as_str().unwrap().to_owned());
        }
        envelope::change_password(&dir, &key, &pw(&format!("p{i}"))).unwrap();
        envelope::set_keyring(&dir, &key, Some(&fake)).unwrap();
    }
    let n = nonces.len();
    nonces.sort();
    nonces.dedup();
    assert_eq!(nonces.len(), n);
    salts.sort();
    salts.dedup();
    assert_eq!(salts.len(), 4);
}

#[test]
fn keyring_wrap_and_master_fallback() {
    let dir = tmpdir("keyring");
    let fake = Fake::default();
    let key = create(&dir, "master", Some(&fake));
    let v = json(&dir);
    let id = v["wraps"][1]["id"].as_str().unwrap().to_owned();
    assert!(id.starts_with("rustxterm-db-"));
    assert_eq!(fake.get(&id).unwrap().unwrap().len(), 32);
    assert_eq!(*envelope::unlock_with_keyring(&dir, &fake).unwrap().unwrap().0, *key.0);

    // Restore on a machine without a keyring: silent unlock is unavailable, the master works.
    assert!(envelope::unlock_with_keyring(&dir, &NoKeyring).unwrap().is_none());
    assert!(envelope::unlock_with_keyring(&dir, &Fake::default()).unwrap().is_none());
    assert_eq!(*unlock(&dir, "master").0.unwrap().0, *key.0);

    // Replacing the keyring wrap deletes the old secret; removing it drops the wrap.
    envelope::set_keyring(&dir, &key, Some(&fake)).unwrap();
    assert!(fake.get(&id).unwrap().is_none());
    envelope::remove_keyring(&dir, &key, &fake).unwrap();
    assert!(fake.secrets.lock().unwrap().is_empty());
    assert_eq!(json(&dir)["wraps"].as_array().unwrap().len(), 1);
    assert!(envelope::unlock_with_keyring(&dir, &fake).unwrap().is_none());
    assert_eq!(*unlock(&dir, "master").0.unwrap().0, *key.0);

    // Keyring unavailable when enabling: error, envelope untouched.
    let before = fs::read(dir.join(ENVELOPE_FILE)).unwrap();
    assert!(matches!(envelope::set_keyring(&dir, &key, Some(&NoKeyring)), Err(Error::Unsupported(_))));
    assert_eq!(fs::read(dir.join(ENVELOPE_FILE)).unwrap(), before);
    envelope::set_keyring(&dir, &key, None).unwrap();
}

#[test]
fn change_password_rewraps_same_key() {
    let dir = tmpdir("change");
    let fake = Fake::default();
    let key = create(&dir, "old", Some(&fake));
    envelope::change_password(&dir, &key, &pw("new")).unwrap();
    assert!(matches!(unlock(&dir, "old").0, Err(Error::Auth(_))));
    assert_eq!(*unlock(&dir, "new").0.unwrap().0, *key.0);
    // Keyring wrap kept.
    assert_eq!(*envelope::unlock_with_keyring(&dir, &fake).unwrap().unwrap().0, *key.0);
}

/// Atomic replacement: a kill before rename leaves the old envelope plus a stray temp file
/// (old password works); after rename the new envelope is in place (new password works).
#[test]
fn interrupted_rewrap_keeps_old_or_new() {
    let dir = tmpdir("interrupt");
    let key = create(&dir, "old", None);
    let old = fs::read(dir.join(ENVELOPE_FILE)).unwrap();
    envelope::change_password(&dir, &key, &pw("new")).unwrap();
    let new = fs::read(dir.join(ENVELOPE_FILE)).unwrap();
    assert!(!dir.join(ENVELOPE_TMP).exists());

    // Killed before rename (full or partially written temp file).
    for tmp in [&new[..], &new[..new.len() / 2]] {
        fs::write(dir.join(ENVELOPE_FILE), &old).unwrap();
        fs::write(dir.join(ENVELOPE_TMP), tmp).unwrap();
        assert_eq!(*unlock(&dir, "old").0.unwrap().0, *key.0);
        assert!(matches!(unlock(&dir, "new").0, Err(Error::Auth(_))));
    }
    // A later change overwrites the stray temp file.
    envelope::change_password(&dir, &key, &pw("newer")).unwrap();
    assert!(!dir.join(ENVELOPE_TMP).exists());
    assert_eq!(*unlock(&dir, "newer").0.unwrap().0, *key.0);

    // Killed after rename.
    fs::write(dir.join(ENVELOPE_FILE), &new).unwrap();
    assert_eq!(*unlock(&dir, "new").0.unwrap().0, *key.0);
}

#[test]
fn tampering_is_detected() {
    let dir = tmpdir("tamper");
    let fake = Fake::default();
    create(&dir, "pw", Some(&fake));
    let good = json(&dir);
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let flip = |s: &str| {
        let mut b = b64.decode(s).unwrap();
        b[0] ^= 1;
        b64.encode(b)
    };
    for (i, field) in [(0, "ct"), (0, "nonce")] {
        let mut v = good.clone();
        v["wraps"][i][field] = flip(v["wraps"][i][field].as_str().unwrap()).into();
        put(&dir, &v);
        assert!(matches!(unlock(&dir, "pw").0, Err(Error::Auth(_))), "{field}");
    }
    let mut v = good.clone();
    v["kdf"]["salt"] = flip(v["kdf"]["salt"].as_str().unwrap()).into();
    put(&dir, &v);
    assert!(matches!(unlock(&dir, "pw").0, Err(Error::Auth(_))));
    // Swapping wrap kinds fails authentication (kind is bound as associated data).
    let mut v = good.clone();
    v["wraps"][1]["ct"] = v["wraps"][0]["ct"].clone();
    v["wraps"][1]["nonce"] = v["wraps"][0]["nonce"].clone();
    put(&dir, &v);
    assert!(envelope::unlock_with_keyring(&dir, &fake).unwrap().is_none());
}

#[test]
fn secrets_never_in_debug_errors_or_file() {
    let dir = tmpdir("secrets");
    let master = "fictional-master-Zq9";
    let key = create(&dir, master, None);
    let hex: String = key.0.iter().map(|b| format!("{b:02x}")).collect();
    let dbg = format!("{key:?}");
    assert!(!dbg.contains(&hex) && !dbg.contains(&format!("{:?}", &key.0[..4])), "{dbg}");
    assert!(!format!("{:?}", pw(master)).contains(master));

    let e = unlock(&dir, &format!("{master}-wrong")).0.unwrap_err();
    for text in [e.to_string(), format!("{e:?}")] {
        assert!(!text.contains(master) && !text.contains(&hex), "{text}");
    }
    let raw = fs::read(dir.join(ENVELOPE_FILE)).unwrap();
    let text = String::from_utf8_lossy(&raw);
    assert!(!text.contains(master) && !text.contains(&hex));
    assert!(!raw.windows(32).any(|w| w == &key.0[..]));
}
