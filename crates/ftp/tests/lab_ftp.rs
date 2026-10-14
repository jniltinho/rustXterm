//! FTP/FTPS lab tests (task 5.1). Run with `lab/lab.sh up` then `lab/lab.sh test`.
//! Lab accounts and passwords are fictional (see `lab/README.md`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use rxt_core::{
    BoxFut, CertTrust, Error, FileKind, FileOps, FtpTls, HostKeyDecision, PromptAnswer, PromptKind, Prompter, Protocol,
    Secret, Session,
};
use rxt_ftp::{Context, Ftp};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

const FTP: u16 = 2121;
const FTPS_IMPLICIT: u16 = 2990;
const ALICE_PASS: &str = "lab-alice-pass";
const TEN_MB: usize = 10 * 1024 * 1024;

/// Scripted prompter and in-memory trust records.
struct Script {
    cert: HostKeyDecision,
    prompts: Mutex<Vec<PromptKind>>,
    changed: Mutex<Vec<(String, String, String)>>,
    trust: Mutex<HashMap<(String, u16), String>>,
}

impl Script {
    fn new(cert: HostKeyDecision) -> Arc<Self> {
        Arc::new(Self { cert, prompts: Mutex::default(), changed: Mutex::default(), trust: Mutex::default() })
    }

    fn prompts(&self) -> Vec<PromptKind> {
        self.prompts.lock().unwrap().clone()
    }

    fn cert_prompts(&self) -> Vec<(String, String)> {
        self.prompts()
            .into_iter()
            .filter_map(|p| match p {
                PromptKind::CertificateUnknown { subject, fingerprint, .. } => Some((subject, fingerprint)),
                _ => None,
            })
            .collect()
    }

    fn ctx(self: &Arc<Self>) -> Context {
        Context { prompter: self.clone(), trust: self.clone(), cancel: CancellationToken::new() }
    }
}

impl Prompter for Script {
    fn prompt(&self, kind: PromptKind) -> BoxFut<'_, PromptAnswer> {
        self.prompts.lock().unwrap().push(kind.clone());
        let answer = match kind {
            PromptKind::CertificateUnknown { .. } => PromptAnswer::HostKey(self.cert),
            PromptKind::Password { .. } => PromptAnswer::Secret(Secret(ALICE_PASS.into())),
            _ => PromptAnswer::Cancel,
        };
        Box::pin(async move { answer })
    }

    fn host_key_changed(&self, host: &str, old: &str, new: &str) {
        self.changed.lock().unwrap().push((host.into(), old.into(), new.into()));
    }
}

impl CertTrust for Script {
    fn get(&self, host: &str, port: u16) -> Option<String> {
        self.trust.lock().unwrap().get(&(host.into(), port)).cloned()
    }

    fn save(&self, host: &str, port: u16, fingerprint: &str) {
        self.trust.lock().unwrap().insert((host.into(), port), fingerprint.into());
    }
}

fn session(protocol: Protocol, host: &str, port: u16, tls: FtpTls) -> Session {
    let mut s = Session::new(protocol, host, Some("alice".into()), Some(port));
    s.ftp.tls = tls;
    s
}

async fn timeout<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(120), f).await.expect("lab operation timed out")
}

async fn connect(s: &Session, script: &Arc<Script>) -> rxt_core::Result<Ftp> {
    timeout(rxt_ftp::connect(s, script.ctx())).await
}

/// Unique token for remote and local names.
fn unique(tag: &str) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    format!("rxt-{tag}-{}-{nanos}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed))
}

fn tmp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(unique(tag));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 10 MB of deterministic pseudo-random bytes (xorshift), written once per test binary.
static BIG: LazyLock<(PathBuf, String)> = LazyLock::new(|| {
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    let data: Vec<u8> = (0..TEN_MB / 8)
        .flat_map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x.to_le_bytes()
        })
        .collect();
    let path = tmp_dir("big").join("big.bin");
    std::fs::write(&path, &data).unwrap();
    (path, sha256(&data))
});

fn sha256(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

async fn put_get_checksum(ftp: &Ftp, tag: &str) {
    let (src, sum) = &*BIG;
    let remote = format!("/{}", unique(tag));
    let last = AtomicU64::new(0);
    let sent = timeout(ftp.put(src, &remote, &|n| last.store(n, Ordering::Relaxed)))
        .await
        .unwrap_or_else(|e| panic!("{tag}: {e}"));
    assert_eq!((sent, last.load(Ordering::Relaxed)), (TEN_MB as u64, TEN_MB as u64));
    assert_eq!(ftp.stat(&remote).await.unwrap().map(|e| e.size), Some(TEN_MB as u64));
    let local = tmp_dir(tag).join("back.bin");
    assert_eq!(timeout(ftp.get(&remote, &local, &|_| {})).await.unwrap(), TEN_MB as u64);
    assert_eq!(&sha256(&std::fs::read(&local).unwrap()), sum);
    ftp.delete(&remote).await.unwrap();
}

#[tokio::test]
#[ignore = "lab"]
async fn ftp_plain_login_list_and_file_ops() {
    let script = Script::new(HostKeyDecision::Cancel);
    let ftp = connect(&session(Protocol::Ftp, "127.0.0.1", FTP, FtpTls::Explicit), &script).await.unwrap();
    assert!(script.cert_prompts().is_empty());
    assert!(matches!(&script.prompts()[..], [PromptKind::Password { retry: false, .. }]));
    let home = ftp.home().await.unwrap();
    assert_eq!(home, "/");

    let dir = format!("/{}", unique("ops"));
    ftp.mkdir(&dir).await.unwrap();
    ftp.mkdir(&format!("{dir}/sub dir")).await.unwrap();
    let local = tmp_dir("ops").join("ñandú.txt");
    std::fs::write(&local, b"hello").unwrap();
    let file = format!("{dir}/sub dir/ñandú.txt");
    assert_eq!(ftp.put(&local, &file, &|_| {}).await.unwrap(), 5);
    ftp.chmod(&file, 0o600).await.unwrap();
    let entries = ftp.list(&format!("{dir}/sub dir")).await.unwrap();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!((entries[0].name.as_str(), entries[0].kind, entries[0].size), ("ñandú.txt", FileKind::File, 5));
    assert_eq!(entries[0].mode, Some(0o600));
    let renamed = format!("{dir}/renamed.txt");
    ftp.rename(&file, &renamed).await.unwrap();
    assert_eq!(ftp.stat(&file).await.unwrap(), None);
    assert!(ftp.list(&home).await.unwrap().iter().any(|e| format!("/{}", e.name) == dir && e.kind == FileKind::Dir));

    ftp.delete(&dir).await.unwrap();
    assert_eq!(ftp.stat(&dir).await.unwrap(), None);
    assert!(matches!(ftp.delete(&dir).await, Err(Error::NotFound(_))));
}

#[tokio::test]
#[ignore = "lab"]
async fn ftp_explicit_tls_prompt_shows_fingerprint_then_lists() {
    let script = Script::new(HostKeyDecision::AcceptAndSave);
    let s = session(Protocol::Ftps, "127.0.0.1", FTP, FtpTls::Explicit);
    let ftp = connect(&s, &script).await.unwrap();
    let prompts = script.prompts();
    // Certificate first, password only after the certificate is accepted.
    assert!(
        matches!(&prompts[..], [PromptKind::CertificateUnknown { .. }, PromptKind::Password { .. }]),
        "{prompts:?}"
    );
    let [(subject, fingerprint)] = &script.cert_prompts()[..] else { panic!("one certificate prompt") };
    assert_eq!(subject, "rxt-lab-ftp");
    assert_eq!(fingerprint.len(), 95, "{fingerprint}");
    assert_eq!(script.get("127.0.0.1", FTP).as_ref(), Some(fingerprint));
    // Listing goes through a TLS data connection (PROT P).
    ftp.list("/").await.unwrap();

    // Saved: no second certificate prompt.
    let again = Script::new(HostKeyDecision::Cancel);
    again.save("127.0.0.1", FTP, fingerprint);
    connect(&s, &again).await.unwrap().list("/").await.unwrap();
    assert!(again.cert_prompts().is_empty());
}

#[tokio::test]
#[ignore = "lab"]
async fn ftp_explicit_tls_changed_certificate_refused_before_login() {
    let script = Script::new(HostKeyDecision::AcceptOnce);
    let old = "00:".repeat(31) + "00";
    script.save("127.0.0.1", FTP, &old);
    let r = connect(&session(Protocol::Ftps, "127.0.0.1", FTP, FtpTls::Explicit), &script).await;
    assert!(matches!(r, Err(Error::HostKey(_))));
    assert!(script.prompts().is_empty(), "no prompt and no password before the refusal");
    let changed = script.changed.lock().unwrap().clone();
    assert!(matches!(&changed[..], [(host, o, _)] if host == "127.0.0.1:2121" && *o == old), "{changed:?}");
}

#[tokio::test]
#[ignore = "lab"]
async fn ftp_implicit_tls_prompt_then_transfer() {
    let s = session(Protocol::Ftps, "127.0.0.1", FTPS_IMPLICIT, FtpTls::Implicit);
    let refused = Script::new(HostKeyDecision::Cancel);
    assert!(matches!(connect(&s, &refused).await, Err(Error::HostKey(_))));
    assert!(matches!(&refused.prompts()[..], [PromptKind::CertificateUnknown { .. }]));

    let script = Script::new(HostKeyDecision::AcceptOnce);
    let ftp = connect(&s, &script).await.unwrap();
    assert_eq!(script.cert_prompts().len(), 1);
    assert_eq!(script.get("127.0.0.1", FTPS_IMPLICIT), None, "accept once is not saved");
    let local = tmp_dir("implicit").join("a.txt");
    std::fs::write(&local, b"over tls").unwrap();
    let remote = format!("/{}", unique("implicit"));
    ftp.put(&local, &remote, &|_| {}).await.unwrap();
    assert!(ftp.list("/").await.unwrap().iter().any(|e| format!("/{}", e.name) == remote));
    let back = local.with_file_name("b.txt");
    ftp.get(&remote, &back, &|_| {}).await.unwrap();
    assert_eq!(std::fs::read(&back).unwrap(), b"over tls");
    ftp.delete(&remote).await.unwrap();
}

#[tokio::test]
#[ignore = "lab"]
async fn ftp_passive_10mb_put_get_checksum() {
    let script = Script::new(HostKeyDecision::AcceptOnce);
    let plain = connect(&session(Protocol::Ftp, "127.0.0.1", FTP, FtpTls::Explicit), &script).await.unwrap();
    put_get_checksum(&plain, "pasv").await;
    let tls = connect(&session(Protocol::Ftps, "127.0.0.1", FTP, FtpTls::Explicit), &script).await.unwrap();
    put_get_checksum(&tls, "pasv-tls").await;
}

/// Active mode needs the server to connect back to the client. Through Docker port publishing
/// that is impossible (the PORT address 127.0.0.1 is the server container's own loopback), so the
/// test talks to the `ftp` container directly on the bridge network: `STAT` reveals the bridge
/// gateway (the address the server sees), and the container is found on port 21 in that /24.
async fn ftp_container_addr() -> IpAddr {
    let stream = TcpStream::connect(("127.0.0.1", FTP)).await.unwrap();
    let (r, mut w) = stream.into_split();
    let mut lines = BufReader::new(r).lines();
    for cmd in ["USER alice", &format!("PASS {ALICE_PASS}"), "STAT", "QUIT"] {
        w.write_all(format!("{cmd}\r\n").as_bytes()).await.unwrap();
    }
    let mut gateway = None;
    while let Some(line) = lines.next_line().await.unwrap() {
        if let Some(ip) = line.trim().strip_prefix("Connected to ") {
            gateway = ip.parse::<Ipv4Addr>().ok();
        }
    }
    let gw = gateway.expect("vsftpd STAT shows the client address").octets();
    for last in 2..=254u8 {
        let ip = IpAddr::V4(Ipv4Addr::new(gw[0], gw[1], gw[2], last));
        let probe = tokio::time::timeout(Duration::from_millis(300), TcpStream::connect(SocketAddr::new(ip, 21)));
        if let Ok(Ok(s)) = probe.await {
            let mut banner = String::new();
            let _ = tokio::time::timeout(Duration::from_secs(2), BufReader::new(s).read_line(&mut banner)).await;
            if banner.starts_with("220") {
                return ip;
            }
        }
    }
    panic!("ftp container not found next to gateway {gw:?}");
}

#[tokio::test]
#[ignore = "lab"]
async fn ftp_active_10mb_put_get_checksum() {
    let ip = ftp_container_addr().await.to_string();
    let script = Script::new(HostKeyDecision::AcceptOnce);
    let mut s = session(Protocol::Ftp, &ip, 21, FtpTls::Explicit);
    s.ftp.passive = false;
    let plain = connect(&s, &script).await.unwrap();
    put_get_checksum(&plain, "active").await;
    s.protocol = Protocol::Ftps;
    let tls = connect(&s, &script).await.unwrap();
    put_get_checksum(&tls, "active-tls").await;
}

#[tokio::test]
#[ignore = "lab"]
async fn ftp_cancel_removes_partials() {
    let script = Script::new(HostKeyDecision::AcceptOnce);
    let ftp = connect(&session(Protocol::Ftp, "127.0.0.1", FTP, FtpTls::Explicit), &script).await.unwrap();
    let (src, _) = &*BIG;
    let notify = tokio::sync::Notify::new();
    let first_chunk = |_| notify.notify_one();

    // Upload cancelled after the first chunk: the remote partial is removed.
    let remote = format!("/{}", unique("cancel"));
    tokio::select! {
        biased;
        () = notify.notified() => {}
        r = ftp.put(src, &remote, &first_chunk) => panic!("upload finished before cancel: {r:?}"),
    }
    let mut gone = false;
    for _ in 0..50 {
        if ftp.stat(&remote).await.unwrap().is_none() {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(gone, "partial upload {remote} still present");

    // Download cancelled after the first chunk: neither target nor partial remains.
    put_get_checksum(&ftp, "cancel-src").await; // connection still usable after the cancel
    ftp.put(src, &remote, &|_| {}).await.unwrap();
    let dir = tmp_dir("cancel");
    let local = dir.join("big.bin");
    tokio::select! {
        biased;
        () = notify.notified() => {}
        r = ftp.get(&remote, &local, &first_chunk) => panic!("download finished before cancel: {r:?}"),
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "local partial left behind");
    ftp.list("/").await.unwrap();
    ftp.delete(&remote).await.unwrap();
}
