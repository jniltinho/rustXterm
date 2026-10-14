//! Read-only `~/.ssh/known_hosts` parser and trust evaluation (design D4).

use std::path::Path;

use data_encoding::BASE64;
use hmac::{Hmac, KeyInit, Mac};
use russh::keys::{HashAlg, PublicKey};
use rxt_core::{Error, Result};
use sha1::Sha1;

/// Parsed `known_hosts` file. Malformed and `@cert-authority` lines are dropped with a warning.
#[derive(Debug, Default)]
pub struct KnownHosts {
    entries: Vec<Entry>,
}

#[derive(Debug)]
struct Entry {
    revoked: bool,
    hosts: Hosts,
    key: PublicKey,
}

#[derive(Debug)]
enum Hosts {
    Hashed { salt: Vec<u8>, hash: Vec<u8> },
    Patterns(Vec<String>),
}

/// Outcome of matching a server key against every trust source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Trusted,
    Unknown,
    Revoked,
    /// A same-algorithm entry holds a different key; carries the stored fingerprint.
    Changed {
        old_fingerprint: String,
    },
}

/// Name used in `known_hosts`: `host` on port 22, `[host]:port` otherwise.
pub fn host_label(host: &str, port: u16) -> String {
    if port == 22 { host.to_owned() } else { format!("[{host}]:{port}") }
}

pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

impl KnownHosts {
    /// Load `path`. A missing file is empty; any other read error fails closed naming the file.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(&text, &path.display().to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::HostKey(format!("cannot read {}: {e}", path.display()))),
        }
    }

    pub fn parse(text: &str, origin: &str) -> Self {
        let entries = text
            .lines()
            .enumerate()
            .filter_map(|(n, line)| {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    return None;
                }
                match parse_line(line) {
                    Ok(entry) => entry,
                    Err(why) => {
                        tracing::warn!("{origin}:{}: ignoring malformed line: {why}", n + 1);
                        None
                    }
                }
            })
            .collect();
        Self { entries }
    }

    /// `(revoked, key)` for every entry whose host field matches.
    pub fn matching(&self, host: &str, port: u16) -> Vec<(bool, PublicKey)> {
        let label = host_label(host, port);
        self.entries.iter().filter(|e| e.hosts.matches(&label)).map(|e| (e.revoked, e.key.clone())).collect()
    }
}

/// `Ok(None)` for lines that are valid but ignored (`@cert-authority`).
fn parse_line(line: &str) -> std::result::Result<Option<Entry>, &'static str> {
    let mut fields = line.split_whitespace();
    let mut first = fields.next().ok_or("empty")?;
    let mut revoked = false;
    if let Some(marker) = first.strip_prefix('@') {
        match marker {
            "revoked" => revoked = true,
            "cert-authority" => {
                tracing::warn!("ignoring @cert-authority entry (host certificates are not supported)");
                return Ok(None);
            }
            _ => return Err("unknown marker"),
        }
        first = fields.next().ok_or("missing host field")?;
    }
    let (Some(alg), Some(b64)) = (fields.next(), fields.next()) else {
        return Err("missing key");
    };
    let key = PublicKey::from_openssh(&format!("{alg} {b64}")).map_err(|_| "invalid key")?;
    let hosts = match first.strip_prefix("|1|") {
        Some(rest) => {
            let (salt, hash) = rest.split_once('|').ok_or("invalid hashed host")?;
            let salt = BASE64.decode(salt.as_bytes()).map_err(|_| "invalid hashed host")?;
            let hash = BASE64.decode(hash.as_bytes()).map_err(|_| "invalid hashed host")?;
            Hosts::Hashed { salt, hash }
        }
        None if first.starts_with('|') => return Err("unsupported host hash"),
        None => Hosts::Patterns(first.split(',').map(str::to_ascii_lowercase).collect()),
    };
    Ok(Some(Entry { revoked, hosts, key }))
}

impl Hosts {
    fn matches(&self, label: &str) -> bool {
        match self {
            Self::Hashed { salt, hash } => {
                let Ok(mut mac) = <Hmac<Sha1> as KeyInit>::new_from_slice(salt) else { return false };
                mac.update(label.as_bytes());
                mac.verify_slice(hash).is_ok()
            }
            Self::Patterns(patterns) => {
                let label = label.to_ascii_lowercase();
                let mut hit = false;
                for p in patterns {
                    match p.strip_prefix('!') {
                        Some(neg) if glob(neg.as_bytes(), label.as_bytes()) => return false,
                        Some(_) => {}
                        None => hit |= glob(p.as_bytes(), label.as_bytes()),
                    }
                }
                hit
            }
        }
    }
}

/// `*` / `?` wildcard match.
fn glob(p: &[u8], s: &[u8]) -> bool {
    match (p.first(), s.first()) {
        (None, None) => true,
        (Some(b'*'), _) => glob(&p[1..], s) || (!s.is_empty() && glob(p, &s[1..])),
        (Some(b'?'), Some(_)) => glob(&p[1..], &s[1..]),
        (Some(a), Some(b)) if a == b => glob(&p[1..], &s[1..]),
        _ => false,
    }
}

/// Evaluate `key` against `(revoked, stored)` entries gathered from all sources for this host.
pub fn evaluate(key: &PublicKey, entries: &[(bool, PublicKey)]) -> Verdict {
    if entries.iter().any(|(revoked, k)| *revoked && k.key_data() == key.key_data()) {
        return Verdict::Revoked;
    }
    let same_alg = entries.iter().filter(|(revoked, k)| !revoked && k.algorithm() == key.algorithm());
    let mut matched = false;
    for (_, k) in same_alg {
        if k.key_data() != key.key_data() {
            return Verdict::Changed { old_fingerprint: fingerprint(k) };
        }
        matched = true;
    }
    if matched { Verdict::Trusted } else { Verdict::Unknown }
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::ssh_key::public::{Ed25519PublicKey, KeyData};

    const RSA: &str = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQCA/SwUH09cVkxkydIZZpydb2hTZMckGOeHDhspdOjBYyScXB+R4Y8kN9o0NsmFffov/4k7IUql4Yh2uGkjqpnpFysYDZPEPVj3DN4B4/hpmroGgeV0xoNoraI4GBmg7IByTNsr365s3WjkQwc+1GjdL2Q9ALo+sz1Z3QIupv2Y6IEgDdNFm3H6rJubip1XqHAv/+9EbTh5oLvY0/L0neJXwCpUMhf/6j5/T1mGhNF+sTc/uiMAjf4oWTQ3reo5W8M/iiI5odrWs2YHvZ7/uBJxaE+spiafdEZRm+U3Mxygu1JHU0oIK6XdgOcWqUfNqVAL482AsUl+ii4CVivUKP/J";

    fn ed(n: u8) -> PublicKey {
        PublicKey::new(KeyData::Ed25519(Ed25519PublicKey([n; 32])), "")
    }

    fn line(k: &PublicKey) -> String {
        k.to_openssh().unwrap()
    }

    fn verdict(text: &str, host: &str, port: u16, key: &PublicKey) -> Verdict {
        evaluate(key, &KnownHosts::parse(text, "test").matching(host, port))
    }

    fn hashed(name: &str) -> String {
        let salt = [7u8; 20];
        let mut mac = <Hmac<Sha1> as KeyInit>::new_from_slice(&salt).unwrap();
        mac.update(name.as_bytes());
        format!("|1|{}|{}", BASE64.encode(&salt), BASE64.encode(&mac.finalize().into_bytes()))
    }

    #[test]
    fn hashed_host_on_custom_port() {
        let k = ed(1);
        let text = format!("{} {}\n", hashed("[lab.example]:2222"), line(&k));
        assert_eq!(verdict(&text, "lab.example", 2222, &k), Verdict::Trusted);
        assert_eq!(verdict(&text, "lab.example", 22, &k), Verdict::Unknown);
    }

    #[test]
    fn bracketed_port_and_patterns() {
        let k = ed(1);
        let text = format!("[h.example]:2200,other {}\n*.lab,!bad.lab {}\n", line(&k), line(&k));
        assert_eq!(verdict(&text, "h.example", 2200, &k), Verdict::Trusted);
        assert_eq!(verdict(&text, "h.example", 22, &k), Verdict::Unknown);
        assert_eq!(verdict(&text, "OTHER", 22, &k), Verdict::Trusted);
        assert_eq!(verdict(&text, "a.lab", 22, &k), Verdict::Trusted);
        assert_eq!(verdict(&text, "bad.lab", 22, &k), Verdict::Unknown);
    }

    #[test]
    fn revoked_key_is_refused() {
        let k = ed(1);
        let text = format!("@revoked * {}\nh {}\n", line(&k), line(&k));
        assert_eq!(verdict(&text, "h", 22, &k), Verdict::Revoked);
    }

    #[test]
    fn cert_authority_and_malformed_lines_never_match() {
        let k = ed(1);
        let text = format!(
            "@cert-authority h {}\nh ssh-ed25519 !!notbase64!!\nh\n@weird h {}\n|2|x|y {}\n",
            line(&k),
            line(&k),
            line(&k)
        );
        assert!(KnownHosts::parse(&text, "test").entries.is_empty());
        assert_eq!(verdict(&text, "h", 22, &k), Verdict::Unknown);
    }

    #[test]
    fn unreadable_file_fails_closed_naming_it() {
        let dir = std::env::temp_dir().join(format!("rxt-kh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A directory exists but cannot be read as a file.
        let err = KnownHosts::load(&dir).unwrap_err();
        assert!(matches!(&err, Error::HostKey(m) if m.contains(&dir.display().to_string())), "{err:?}");
        assert!(KnownHosts::load(&dir.join("missing")).unwrap().entries.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn same_algorithm_conflict_is_changed() {
        let (old, new) = (ed(1), ed(2));
        let text = format!("h {}\n", line(&old));
        assert_eq!(verdict(&text, "h", 22, &new), Verdict::Changed { old_fingerprint: fingerprint(&old) });
        // Matches one source but conflicts with another (e.g. app trust records).
        let mut entries = KnownHosts::parse(&format!("h {}\n", line(&new)), "t").matching("h", 22);
        entries.push((false, old));
        assert!(matches!(evaluate(&new, &entries), Verdict::Changed { .. }));
    }

    #[test]
    fn different_algorithms_are_not_a_conflict() {
        let k = ed(1);
        let text = format!("h {RSA} fixture\nh {}\n", line(&k));
        assert_eq!(verdict(&text, "h", 22, &k), Verdict::Trusted);
        let rsa_only = format!("h {RSA}\n");
        assert_eq!(verdict(&rsa_only, "h", 22, &k), Verdict::Unknown);
    }
}
