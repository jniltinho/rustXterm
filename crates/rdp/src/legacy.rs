//! Standard RDP Security (MS-RDPBCGR 5.3), opt-in per session (`RdpOptions::allow_standard_security`)
//! for servers whose Security Layer is "RDP". RC4 with 40/56/128-bit keys and MD5/SHA-1 MACs; FIPS
//! and encryption level None are refused. The transform runs inside `Wire`: outbound PDUs get the
//! security header, MAC and encryption; inbound ones are verified, decrypted and stripped, so
//! IronRDP keeps seeing the Enhanced-Security layout it parses.
//!
//! The layer cannot authenticate the server (the proprietary certificate is signed with a
//! published key), so trust is TOFU on the server's RSA key.

use std::borrow::Cow;

use ironrdp::core::{decode, encode_vec};
use ironrdp::pdu::gcc::{EncryptionLevel, EncryptionMethod, ServerSecurityData};
use ironrdp::pdu::mcs::{McsMessage, SendDataIndication, SendDataRequest};
use ironrdp::pdu::rdp::server_license::ServerCertificate;
use ironrdp::pdu::rdp::server_license::cert::CertificateType;
use ironrdp::pdu::x224::X224;
use md5::{Digest as _, Md5};
use num_bigint::BigUint;
use pkcs1::der::Decode as _;
use rxt_core::{Error, Result};
use sha1::Sha1;
use sha2::Sha256;

const SEC_EXCHANGE_PKT: u16 = 0x0001;
const SEC_ENCRYPT: u16 = 0x0008;
const SEC_LICENSE_PKT: u16 = 0x0080;
/// Same value for both directions in MS-RDPBCGR 2.2.8.1.1.2.1.
const SEC_LICENSE_ENCRYPT: u16 = 0x0200;
const SEC_REDIRECTION_PKT: u16 = 0x0400;
const SEC_SECURE_CHECKSUM: u16 = 0x0800;
const FASTPATH_SECURE_CHECKSUM: u8 = 0x1;
const FASTPATH_ENCRYPTED: u8 = 0x2;
const PAD1: [u8; 40] = [0x36; 40];
const PAD2: [u8; 48] = [0x5c; 48];
/// Key refresh interval (MS-RDPBCGR 5.3.7).
const UPDATE_EVERY: u32 = 4096;
const MAX_MODULUS_BITS: usize = 4096;

/// Prefix of a legacy RSA key fingerprint in `CertTrust`, so it never matches a TLS certificate.
pub(crate) const KEY_PREFIX: &str = "RSA ";
/// Shown as the trust prompt's subject.
pub(crate) const PROMPT_SUBJECT: &str =
    "Legacy RDP security (no TLS): this connection is not protected against interception";

fn proto(msg: impl Into<String>) -> Error {
    Error::Protocol(msg.into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bits {
    B40,
    B56,
    B128,
}

impl Bits {
    fn key_len(self) -> usize {
        if self == Self::B128 { 16 } else { 8 }
    }

    /// 40/56-bit keys keep the first 64 bits with a fixed salt (MS-RDPBCGR 5.3.5.1).
    fn salt(self, key: &mut [u8]) {
        match self {
            Self::B40 => key[..3].copy_from_slice(&[0xd1, 0x26, 0x9e]),
            Self::B56 => key[0] = 0xd1,
            Self::B128 => {}
        }
    }
}

/// Checks the server's choice; returns the key size and the server random.
fn negotiated(data: &ServerSecurityData) -> Result<(Bits, [u8; 32])> {
    if data.encryption_level == EncryptionLevel::Fips || data.encryption_method.contains(EncryptionMethod::FIPS) {
        return Err(proto("the server requires FIPS encryption, which legacy RDP security does not support"));
    }
    if data.encryption_level == EncryptionLevel::None || data.encryption_method.is_empty() {
        return Err(proto("the server offers no encryption (level None); credentials would travel in clear"));
    }
    let bits = match data.encryption_method {
        m if m == EncryptionMethod::BIT_128 => Bits::B128,
        m if m == EncryptionMethod::BIT_56 => Bits::B56,
        m if m == EncryptionMethod::BIT_40 => Bits::B40,
        m => return Err(proto(format!("unexpected server encryption method {:#x}", m.bits()))),
    };
    let random = data.server_random.ok_or_else(|| proto("server random missing"))?;
    Ok((bits, random))
}

/// The server's RSA key from Server Security Data (proprietary certificate or X.509 chain).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServerKey {
    /// Big-endian, without leading zeros.
    modulus: Vec<u8>,
    exponent: Vec<u8>,
}

impl ServerKey {
    pub(crate) fn parse(cert: &[u8]) -> Result<Self> {
        let cert = decode::<ServerCertificate>(cert).map_err(|e| proto(format!("server certificate: {e}")))?;
        let key = match cert.certificate {
            CertificateType::Proprietary(c) => {
                // Little-endian modulus followed by 8 zero bytes of padding.
                let len = c.public_key.modulus.len().saturating_sub(8);
                let mut modulus = c.public_key.modulus[..len].to_vec();
                modulus.reverse();
                Self { modulus, exponent: c.public_key.public_exponent.to_be_bytes().to_vec() }
            }
            CertificateType::X509(chain) => {
                let leaf = chain.certificate_array.last().ok_or_else(|| proto("empty server certificate chain"))?;
                let bits = crate::trust::public_key(leaf)?;
                let rsa =
                    pkcs1::RsaPublicKey::from_der(&bits).map_err(|e| proto(format!("server key is not RSA: {e}")))?;
                Self { modulus: rsa.modulus.as_bytes().to_vec(), exponent: rsa.public_exponent.as_bytes().to_vec() }
            }
        };
        let modulus = trim(&key.modulus);
        if modulus.len() < 64 || modulus.len() * 8 > MAX_MODULUS_BITS {
            return Err(proto(format!("unsupported server RSA key size ({} bits)", modulus.len() * 8)));
        }
        Ok(Self { modulus: modulus.to_vec(), exponent: trim(&key.exponent).to_vec() })
    }

    /// `RSA AA:BB:...`: SHA-256 over the big-endian modulus and exponent.
    pub(crate) fn fingerprint(&self) -> String {
        let digest = Sha256::new().chain_update(&self.modulus).chain_update(&self.exponent).finalize();
        let hex: Vec<String> = digest.iter().map(|b| format!("{b:02X}")).collect();
        format!("{KEY_PREFIX}{}", hex.join(":"))
    }

    /// Raw RSA of `random` (little-endian in and out), modulus width plus 8 zero bytes
    /// (MS-RDPBCGR 5.3.4.1).
    fn encrypt(&self, random: &[u8]) -> Vec<u8> {
        let n = BigUint::from_bytes_be(&self.modulus);
        let e = BigUint::from_bytes_be(&self.exponent);
        let mut out = BigUint::from_bytes_le(random).modpow(&e, &n).to_bytes_le();
        out.resize(self.modulus.len() + 8, 0);
        out
    }
}

fn trim(b: &[u8]) -> &[u8] {
    let start = b.iter().position(|&x| x != 0).unwrap_or(b.len());
    &b[start..]
}

#[derive(Clone)]
struct Rc4 {
    s: [u8; 256],
    i: u8,
    j: u8,
}

impl Rc4 {
    fn new(key: &[u8]) -> Self {
        let mut s = [0u8; 256];
        for (v, i) in s.iter_mut().zip(0u8..=255) {
            *v = i;
        }
        let mut j = 0u8;
        for i in 0..256 {
            j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
            s.swap(i, usize::from(j));
        }
        Self { s, i: 0, j: 0 }
    }

    fn apply(&mut self, data: &mut [u8]) {
        for b in data {
            self.i = self.i.wrapping_add(1);
            self.j = self.j.wrapping_add(self.s[usize::from(self.i)]);
            self.s.swap(usize::from(self.i), usize::from(self.j));
            let t = self.s[usize::from(self.i)].wrapping_add(self.s[usize::from(self.j)]);
            *b ^= self.s[usize::from(t)];
        }
    }
}

/// `SaltedHash(S, I) = MD5(S + SHA1(I + S + A + B))`.
fn salted_hash(s: &[u8], i: &[u8], a: &[u8], b: &[u8]) -> [u8; 16] {
    let sha = Sha1::new().chain_update(i).chain_update(s).chain_update(a).chain_update(b).finalize();
    Md5::new().chain_update(s).chain_update(sha).finalize().into()
}

/// Three salted hashes (`A`, `BB`, `CCC` or `X`, `YY`, `ZZZ`) concatenated.
fn hash48(s: &[u8], labels: [&[u8]; 3], client: &[u8; 32], server: &[u8; 32]) -> [u8; 48] {
    let mut out = [0u8; 48];
    for (chunk, label) in out.chunks_mut(16).zip(labels) {
        chunk.copy_from_slice(&salted_hash(s, label, client, server));
    }
    out
}

/// Session keys (MS-RDPBCGR 5.3.5.1): `(mac, client encrypt, client decrypt)`, salted for
/// 40/56 bits; only the first `key_len` bytes are used.
fn session_keys(client: &[u8; 32], server: &[u8; 32], bits: Bits) -> ([u8; 16], [u8; 16], [u8; 16]) {
    let mut pre_master = [0u8; 48];
    pre_master[..24].copy_from_slice(&client[..24]);
    pre_master[24..].copy_from_slice(&server[..24]);
    let master = hash48(&pre_master, [b"A", b"BB", b"CCC"], client, server);
    let blob = hash48(&master, [b"X", b"YY", b"ZZZ"], client, server);
    let final_hash = |k: &[u8]| -> [u8; 16] {
        Md5::new().chain_update(k).chain_update(client).chain_update(server).finalize().into()
    };
    let mut mac = [0u8; 16];
    mac.copy_from_slice(&blob[..16]);
    let mut decrypt = final_hash(&blob[16..32]);
    let mut encrypt = final_hash(&blob[32..48]);
    for k in [&mut mac, &mut encrypt, &mut decrypt] {
        bits.salt(k);
    }
    (mac, encrypt, decrypt)
}

/// MAC (MS-RDPBCGR 5.3.6.1); `count` selects the salted variant (5.3.6.1.1).
fn mac(key: &[u8], data: &[u8], count: Option<u32>) -> [u8; 8] {
    let len = u32::try_from(data.len()).unwrap_or(u32::MAX).to_le_bytes();
    let mut sha = Sha1::new().chain_update(key).chain_update(PAD1).chain_update(len).chain_update(data);
    if let Some(count) = count {
        sha.update(count.to_le_bytes());
    }
    let md = Md5::new().chain_update(key).chain_update(PAD2).chain_update(sha.finalize()).finalize();
    let mut out = [0u8; 8];
    out.copy_from_slice(&md[..8]);
    out
}

/// One direction: RC4 stream, refreshed every 4096 packets (MS-RDPBCGR 5.3.7).
struct Cipher {
    initial: [u8; 16],
    key: [u8; 16],
    rc4: Rc4,
    since_update: u32,
    /// Packets so far, for the salted MAC.
    total: u32,
}

impl Cipher {
    fn new(key: [u8; 16], bits: Bits) -> Self {
        Self { initial: key, key, rc4: Rc4::new(&key[..bits.key_len()]), since_update: 0, total: 0 }
    }

    /// Encrypts or decrypts one packet; returns the packet's count for the salted MAC.
    fn apply(&mut self, data: &mut [u8], bits: Bits) -> u32 {
        let len = bits.key_len();
        if self.since_update >= UPDATE_EVERY {
            let sha = Sha1::new()
                .chain_update(&self.initial[..len])
                .chain_update(PAD1)
                .chain_update(&self.key[..len])
                .finalize();
            self.key =
                Md5::new().chain_update(&self.initial[..len]).chain_update(PAD2).chain_update(sha).finalize().into();
            Rc4::new(&self.key[..len]).apply(&mut self.key[..len]);
            bits.salt(&mut self.key);
            self.rc4 = Rc4::new(&self.key[..len]);
            self.since_update = 0;
        }
        self.rc4.apply(data);
        self.since_update += 1;
        let count = self.total;
        self.total = self.total.wrapping_add(1);
        count
    }
}

/// Fast-path length (PER, 1 or 2 bytes); returns `(length, bytes used)`.
fn fp_length(frame: &[u8]) -> Result<(usize, usize)> {
    match frame.get(1..) {
        Some([b, ..]) if b & 0x80 == 0 => Ok((usize::from(*b), 1)),
        Some([hi, lo, ..]) => Ok(((usize::from(hi & 0x7f) << 8) | usize::from(*lo), 2)),
        _ => Err(proto("short fast-path header")),
    }
}

fn fp_frame(header: u8, body: &[u8]) -> Result<Vec<u8>> {
    let len = body.len() + 2;
    let mut out = Vec::with_capacity(len + 1);
    out.push(header);
    if len <= 0x7f {
        out.push(u8::try_from(len).map_err(|_| proto("fast-path length"))?);
    } else {
        let len =
            u16::try_from(len + 1).ok().filter(|l| *l <= 0x7fff).ok_or_else(|| proto("fast-path PDU too large"))?;
        out.extend_from_slice(&(len | 0x8000).to_be_bytes());
    }
    out.extend_from_slice(body);
    Ok(out)
}

/// Length of the next frame in `buf` (TPKT or fast-path).
fn frame_len(buf: &[u8]) -> Result<usize> {
    let len = match buf {
        [3, _, hi, lo, ..] => usize::from(u16::from_be_bytes([*hi, *lo])),
        [h, ..] if h & 0x3 == 0 => fp_length(buf)?.0,
        _ => return Err(proto("unknown outbound frame")),
    };
    if len == 0 || len > buf.len() { Err(proto("truncated outbound frame")) } else { Ok(len) }
}

/// Active Standard RDP Security state of one connection.
pub(crate) struct Security {
    bits: Bits,
    level: EncryptionLevel,
    mac_key: [u8; 16],
    encrypt: Cipher,
    decrypt: Cipher,
    salted_seen: bool,
}

/// The Security Exchange PDU and the state to install once it is sent.
pub(crate) struct Exchange {
    pub(crate) pdu: Vec<u8>,
    pub(crate) security: Security,
}

/// Validates Server Security Data and builds the Security Exchange PDU (client random encrypted
/// with the server key) for `initiator` on `channel`.
pub(crate) fn exchange(data: &ServerSecurityData, key: &ServerKey, initiator: u16, channel: u16) -> Result<Exchange> {
    let (bits, server_random) = negotiated(data)?;
    tracing::info!(?bits, level = ?data.encryption_level, "Standard RDP Security");
    let client_random: [u8; 32] = rand::random();
    let encrypted = key.encrypt(&client_random);
    let mut user_data = Vec::with_capacity(encrypted.len() + 8);
    user_data.extend_from_slice(&(SEC_EXCHANGE_PKT | SEC_LICENSE_ENCRYPT).to_le_bytes());
    user_data.extend_from_slice(&0u16.to_le_bytes());
    user_data.extend_from_slice(&u32::try_from(encrypted.len()).map_err(|_| proto("key size"))?.to_le_bytes());
    user_data.extend_from_slice(&encrypted);
    let pdu = SendDataRequest { initiator_id: initiator, channel_id: channel, user_data: Cow::Owned(user_data) };
    let pdu = encode_vec(&X224(pdu)).map_err(|e| proto(e.to_string()))?;
    Ok(Exchange { pdu, security: Security::new(bits, data.encryption_level, &client_random, &server_random) })
}

impl Security {
    fn new(bits: Bits, level: EncryptionLevel, client: &[u8; 32], server: &[u8; 32]) -> Self {
        let (mac_key, encrypt, decrypt) = session_keys(client, server, bits);
        let (encrypt, decrypt) = (Cipher::new(encrypt, bits), Cipher::new(decrypt, bits));
        Self { bits, level, mac_key, encrypt, decrypt, salted_seen: false }
    }

    fn mac_key(&self) -> &[u8] {
        &self.mac_key[..self.bits.key_len()]
    }

    /// MAC then encrypt `body`; returns the signature.
    fn seal_body(&mut self, body: &mut [u8]) -> [u8; 8] {
        let sig = mac(self.mac_key(), body, None);
        self.encrypt.apply(body, self.bits);
        sig
    }

    /// Decrypt `body` and check its signature.
    fn open_body(&mut self, sig: &[u8], body: &mut [u8], salted: bool) -> Result<()> {
        let count = self.decrypt.apply(body, self.bits);
        if salted && !self.salted_seen {
            self.salted_seen = true;
            tracing::info!("Standard RDP Security: server uses salted MACs");
        }
        if mac(self.mac_key(), body, salted.then_some(count)) == sig {
            Ok(())
        } else {
            Err(proto("legacy RDP security: invalid packet signature"))
        }
    }

    /// Encrypts every frame in `buf`. With `has_header`, slow-path PDUs already start with a
    /// basic security header (Client Info, licensing) whose flags are kept.
    pub(crate) fn seal(&mut self, mut buf: &[u8], has_header: bool) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(buf.len() + 16);
        while !buf.is_empty() {
            let len = frame_len(buf)?;
            let (frame, rest) = buf.split_at(len);
            buf = rest;
            if frame[0] == 3 {
                out.extend(self.seal_slow(frame, has_header)?);
            } else {
                let (_, n) = fp_length(frame)?;
                let mut body = frame[1 + n..].to_vec();
                let sig = self.seal_body(&mut body);
                let header = (frame[0] & 0x3f) | (FASTPATH_ENCRYPTED << 6);
                out.extend(fp_frame(header, &[&sig[..], &body].concat())?);
            }
        }
        Ok(out)
    }

    fn seal_slow(&mut self, frame: &[u8], has_header: bool) -> Result<Vec<u8>> {
        let Ok(X224(McsMessage::SendDataRequest(req))) = decode::<X224<McsMessage<'_>>>(frame) else {
            return Ok(frame.to_vec());
        };
        let data = req.user_data.as_ref();
        let (mut flags, body) = if has_header {
            let flags = data.get(..2).ok_or_else(|| proto("short security header"))?;
            (u16::from_le_bytes([flags[0], flags[1]]), data.get(4..).ok_or_else(|| proto("short security header"))?)
        } else {
            (0, data)
        };
        flags |= SEC_ENCRYPT;
        if flags & SEC_LICENSE_PKT != 0 {
            flags |= SEC_LICENSE_ENCRYPT;
        }
        let mut body = body.to_vec();
        let sig = self.seal_body(&mut body);
        let mut user_data = Vec::with_capacity(body.len() + 12);
        user_data.extend_from_slice(&flags.to_le_bytes());
        user_data.extend_from_slice(&0u16.to_le_bytes());
        user_data.extend_from_slice(&sig);
        user_data.extend_from_slice(&body);
        let req = SendDataRequest { user_data: Cow::Owned(user_data), ..req };
        encode_vec(&X224(req)).map_err(|e| proto(e.to_string()))
    }

    /// Verifies, decrypts and strips one inbound frame.
    pub(crate) fn open(&mut self, frame: &[u8]) -> Result<Vec<u8>> {
        if frame.first() == Some(&3) { self.open_slow(frame) } else { self.open_fast(frame) }
    }

    /// Clear traffic is legal only at level Low (server-to-client unencrypted).
    fn require_encrypted(&self) -> Result<()> {
        if self.level == EncryptionLevel::Low {
            Ok(())
        } else {
            Err(proto(format!("legacy RDP security: unencrypted PDU at level {:?}", self.level)))
        }
    }

    fn open_fast(&mut self, frame: &[u8]) -> Result<Vec<u8>> {
        let flags = frame[0] >> 6;
        if flags & FASTPATH_ENCRYPTED == 0 {
            self.require_encrypted()?;
            return Ok(frame.to_vec());
        }
        let (_, n) = fp_length(frame)?;
        let rest = &frame[1 + n..];
        let (sig, body) = rest.split_at_checked(8).ok_or_else(|| proto("short fast-path signature"))?;
        let mut body = body.to_vec();
        self.open_body(sig, &mut body, flags & FASTPATH_SECURE_CHECKSUM != 0)?;
        fp_frame(frame[0] & 0x3f, &body)
    }

    fn open_slow(&mut self, frame: &[u8]) -> Result<Vec<u8>> {
        let Ok(X224(McsMessage::SendDataIndication(ind))) = decode::<X224<McsMessage<'_>>>(frame) else {
            return Ok(frame.to_vec());
        };
        let data = ind.user_data.as_ref();
        let flags =
            data.get(..2).map(|f| u16::from_le_bytes([f[0], f[1]])).ok_or_else(|| proto("short security header"))?;
        if flags & SEC_REDIRECTION_PKT != 0 {
            return Err(proto("server redirection is not supported with legacy RDP security"));
        }
        let rest = data.get(4..).ok_or_else(|| proto("short security header"))?;
        let body = if flags & SEC_ENCRYPT != 0 {
            let (sig, body) = rest.split_at_checked(8).ok_or_else(|| proto("short security signature"))?;
            let mut body = body.to_vec();
            self.open_body(sig, &mut body, flags & SEC_SECURE_CHECKSUM != 0)?;
            body
        } else {
            if flags & SEC_LICENSE_PKT == 0 {
                self.require_encrypted()?;
            }
            rest.to_vec()
        };
        // Licensing PDUs keep a plain basic security header (IronRDP expects one).
        let user_data = if flags & SEC_LICENSE_PKT != 0 {
            [&SEC_LICENSE_PKT.to_le_bytes()[..], &[0, 0], &body].concat()
        } else {
            body
        };
        let ind = SendDataIndication { user_data: Cow::Owned(user_data), ..ind };
        encode_vec(&X224(ind)).map_err(|e| proto(e.to_string()))
    }
}

#[cfg(test)]
mod tests;
