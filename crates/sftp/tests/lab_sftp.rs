//! SFTP lab tests (task 3.3). Run with `lab/lab.sh up && lab/lab.sh test`; every test is `#[ignore = "lab"]`.
//! Credentials are the fictional lab accounts from `lab/README.md`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../ssh/tests/common/mod.rs"]
mod common;

use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use common::*;
use rxt_core::{FileKind, FileOps, HostKeyDecision, PromptKind};
use rxt_sftp::{EditCopy, Sftp};
use rxt_term::{TermEvent, Terminal};
use sha2::{Digest, Sha256};

async fn sftponly() -> Sftp {
    let client =
        connect_with(&session("sftponly", SSHD), Script::new(HostKeyDecision::AcceptOnce, &[SFTP_PASS])).await.unwrap();
    timeout(Sftp::open(&client)).await.unwrap()
}

async fn alice_sftp() -> Sftp {
    timeout(Sftp::open(&alice(SSHD).await)).await.unwrap()
}

/// Write `mib` MiB of pseudo-random data; returns its SHA-256.
fn random_file(path: &Path, mib: usize) -> Vec<u8> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut hash = Sha256::new();
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut chunk = vec![0u8; 1 << 20];
    for _ in 0..mib {
        for b in chunk.as_chunks_mut::<8>().0 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = x.to_le_bytes();
        }
        hash.update(&chunk);
        f.write_all(&chunk).unwrap();
    }
    f.flush().unwrap();
    hash.finalize().to_vec()
}

fn sha256_file(path: &Path) -> Vec<u8> {
    let mut f = std::fs::File::open(path).unwrap();
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    hash.finalize().to_vec()
}

#[tokio::test]
#[ignore = "lab"]
async fn sftp_100mb_put_get_checksum() {
    let sftp = sftponly().await;
    let dir = tmp_dir("big");
    let src = dir.join("big.bin");
    let sum = random_file(&src, 100);
    let remote = format!("/upload/big-{}.bin", unique("b"));
    let last = std::sync::atomic::AtomicU64::new(0);
    let progress = |b| last.store(b, std::sync::atomic::Ordering::Relaxed);
    let up = tokio::time::timeout(Duration::from_secs(300), sftp.put(&src, &remote, &progress)).await.unwrap().unwrap();
    assert_eq!(up, 100 << 20);
    assert_eq!(last.load(std::sync::atomic::Ordering::Relaxed), up);
    assert_eq!(sftp.stat(&remote).await.unwrap().unwrap().size, up);

    let dst = dir.join("back.bin");
    let down = tokio::time::timeout(Duration::from_secs(300), sftp.get(&remote, &dst, &|_| {})).await.unwrap().unwrap();
    assert_eq!(down, up);
    assert_eq!(sha256_file(&dst), sum, "checksum mismatch after round trip");
    sftp.delete(&remote).await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
#[ignore = "lab"]
async fn sftp_overwrite_decided_by_stat_before_put() {
    let sftp = sftponly().await;
    let dir = tmp_dir("ow");
    let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
    std::fs::write(&a, "first").unwrap();
    std::fs::write(&b, "second version").unwrap();
    let remote = format!("/upload/ow-{}.txt", unique("o"));
    assert!(sftp.stat(&remote).await.unwrap().is_none(), "new target: no question needed");
    sftp.put(&a, &remote, &|_| {}).await.unwrap();

    // Existing target: the caller sees it before any byte is written and decides.
    let existing = sftp.stat(&remote).await.unwrap().expect("target exists");
    assert_eq!((existing.kind, existing.size), (FileKind::File, 5));
    // "Skip": nothing written, content unchanged.
    let back = dir.join("check.txt");
    sftp.get(&remote, &back, &|_| {}).await.unwrap();
    assert_eq!(std::fs::read_to_string(&back).unwrap(), "first");
    // "Overwrite": replaced (and truncated) content.
    sftp.put(&b, &remote, &|_| {}).await.unwrap();
    sftp.get(&remote, &back, &|_| {}).await.unwrap();
    assert_eq!(std::fs::read_to_string(&back).unwrap(), "second version");
    sftp.delete(&remote).await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
#[ignore = "lab"]
async fn sftp_cancel_removes_partials() {
    let sftp = sftponly().await;
    let dir = tmp_dir("cancel");

    // Upload cancelled mid-transfer: the remote partial is removed.
    let src = dir.join("up.bin");
    random_file(&src, 64);
    let remote = format!("/upload/cancel-{}.bin", unique("c"));
    let started = tokio::sync::Notify::new();
    let progress = |b| {
        if b >= 8 << 20 {
            started.notify_one();
        }
    };
    tokio::select! {
        biased;
        () = started.notified() => {} // the user cancels: the put future is dropped
        r = sftp.put(&src, &remote, &progress) => panic!("upload finished before the cancel: {r:?}"),
    }
    let mut gone = false;
    for _ in 0..100 {
        if sftp.stat(&remote).await.unwrap().is_none() {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(gone, "remote partial {remote} left behind");

    // Download cancelled mid-transfer (endless source): no local partial or target is left.
    let alice = alice_sftp().await;
    let target = dir.join("down.bin");
    let r = tokio::time::timeout(Duration::from_millis(500), alice.get("/dev/zero", &target, &|_| {})).await;
    assert!(r.is_err());
    let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(left, [std::ffi::OsString::from("up.bin")], "partial download left behind");
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
#[ignore = "lab"]
async fn sftp_file_operations_round_trip() {
    let sftp = alice_sftp().await;
    let home = sftp.home().await.unwrap();
    assert_eq!(home, "/home/alice");
    let base = format!("{home}/ops-{}", unique("d"));
    sftp.mkdir(&base).await.unwrap();
    let local = tmp_dir("ops").join("f.txt");
    std::fs::write(&local, "hello").unwrap();
    let file = format!("{base}/f.txt");
    sftp.put(&local, &file, &|_| {}).await.unwrap();
    sftp.mkdir(&format!("{base}/sub")).await.unwrap();

    let names = |v: Vec<rxt_core::FileEntry>| {
        let mut n: Vec<_> = v.into_iter().map(|e| (e.name, e.kind)).collect();
        n.sort_by(|a, b| a.0.cmp(&b.0));
        n
    };
    let listed = sftp.list(&base).await.unwrap();
    // Owner and group come from READDIR longnames as names, not numeric ids.
    assert!(listed.iter().all(|e| e.owner.as_deref() == Some("alice")), "{listed:?}");
    assert!(listed.iter().all(|e| e.group.as_deref().is_some_and(|g| g.parse::<u32>().is_err())), "{listed:?}");
    assert_eq!(names(listed), [("f.txt".to_owned(), FileKind::File), ("sub".to_owned(), FileKind::Dir)]);

    let renamed = format!("{base}/sub/g.txt");
    sftp.rename(&file, &renamed).await.unwrap();
    assert!(sftp.stat(&file).await.unwrap().is_none());
    sftp.chmod(&renamed, 0o600).await.unwrap();
    let st = sftp.stat(&renamed).await.unwrap().unwrap();
    assert_eq!((st.name.as_str(), st.size, st.mode), ("g.txt", 5, Some(0o600)));

    sftp.delete(&base).await.unwrap(); // recursive
    assert!(sftp.stat(&base).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "lab"]
async fn sftp_open_with_upload_back() {
    let sftp = alice_sftp().await;
    let remote = format!("/home/alice/edit-{}.conf", unique("e"));
    let seed = tmp_dir("seed").join("app.conf");
    std::fs::write(&seed, "a=1\n").unwrap();
    sftp.put(&seed, &remote, &|_| {}).await.unwrap();

    let dir = tmp_dir("edit");
    let mut copy = EditCopy::download(&sftp, &remote, &dir).await.unwrap();
    assert_eq!(std::fs::read_to_string(&copy.local).unwrap(), "a=1\n");
    assert!(!copy.changed());
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&copy.local, "a=2\n").unwrap(); // the local editor saves
    assert!(copy.changed());
    copy.upload_back(&sftp, &|_| {}).await.unwrap();
    assert!(!copy.changed());

    let check = dir.join("check");
    sftp.get(&remote, &check, &|_| {}).await.unwrap();
    assert_eq!(std::fs::read_to_string(&check).unwrap(), "a=2\n");
    sftp.delete(&remote).await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
#[ignore = "lab"]
async fn sftp_same_connection_as_shell_no_reauth() {
    let script = Script::new(HostKeyDecision::AcceptOnce, &[ALICE_PASS, ALICE_PASS]);
    let s = session("alice", SSHD);
    let client = connect_with(&s, script.clone()).await.unwrap();
    let mut shell = timeout(client.shell(&s, 80, 24)).await.unwrap();
    let sftp = timeout(Sftp::open(&client)).await.unwrap();
    shell.writer().write(b"echo SHELL-$((40+2))\n").await.unwrap();
    read_until(&mut shell, |o| o.text.contains("SHELL-42")).await;
    assert!(!sftp.list("/home/alice").await.unwrap().is_empty());
    let pw = script.prompts().iter().filter(|p| matches!(p, PromptKind::Password { .. })).count();
    assert_eq!(pw, 1, "credentials asked again: {:?}", script.prompts());
}

#[tokio::test]
#[ignore = "lab"]
async fn sftp_follows_osc7_from_shell() {
    let client = alice(SSHD).await;
    let s = session("alice", SSHD);
    let mut shell = timeout(client.shell(&s, 80, 24)).await.unwrap();
    let sftp = timeout(Sftp::open(&client)).await.unwrap();
    shell.writer().write(b"printf '\\033]7;file://sshd/var/log\\007'\n").await.unwrap();

    let mut term = Terminal::new(80, 24, 1000);
    let mut cwd = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while cwd.is_none() {
        let msg = tokio::time::timeout_at(deadline, shell.read()).await.expect("no OSC 7 seen").expect("shell closed");
        if let rxt_ssh::ShellMsg::Data(d) = msg {
            cwd = term.feed(&d).into_iter().find_map(|e| match e {
                TermEvent::CwdChanged(p) => Some(p),
                _ => None,
            });
        }
    }
    assert_eq!(cwd.as_deref(), Some("/var/log"));
    let entries = sftp.list(cwd.as_deref().unwrap()).await.unwrap();
    assert!(!entries.is_empty(), "/var/log listing is empty");
}
