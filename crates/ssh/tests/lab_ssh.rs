//! SSH lab tests (task 3.2). Run with `lab/lab.sh up && lab/lab.sh test`; every test is `#[ignore = "lab"]`.
//! Credentials are the fictional lab accounts from `lab/README.md`.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use common::*;
use russh::keys::PublicKey;
use russh::keys::ssh_key::public::{Ed25519PublicKey, KeyData};
use rxt_core::{Error, HostKeyDecision, JumpHost, PromptKind, Tunnel, TunnelKind, TunnelState};
use rxt_ssh::TrustStore;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

// ---- authentication -------------------------------------------------------------------------

#[tokio::test]
#[ignore = "lab"]
async fn ssh_auth_password_alice() {
    let script = Script::new(HostKeyDecision::AcceptOnce, &[ALICE_PASS]);
    let s = session("alice", SSHD);
    let client = connect_with(&s, script.clone()).await.unwrap();
    let out = run(&client, &s, "id -un").await;
    assert!(out.text.contains("alice"), "{out:?}");
    assert_eq!(script.credential_prompts(), 1);
}

#[tokio::test]
#[ignore = "lab"]
async fn ssh_auth_publickey_bob() {
    let script = Script::new(HostKeyDecision::AcceptOnce, &[]);
    let mut s = session("bob", SSHD);
    s.ssh.key_file = Some(std::env::var("RXT_LAB_SSH_KEY").expect("RXT_LAB_SSH_KEY is set by the lab runner"));
    let client = connect_with(&s, script.clone()).await.unwrap();
    let out = run(&client, &s, "id -un").await;
    assert!(out.text.contains("bob"), "{out:?}");
    assert_eq!(script.credential_prompts(), 0, "key auth must not ask for a password: {:?}", script.prompts());
}

#[tokio::test]
#[ignore = "lab"]
async fn ssh_auth_keyboard_interactive_carol() {
    let script = Script::new(HostKeyDecision::AcceptOnce, &[]);
    script.kbd.lock().unwrap().push_back(vec![CAROL_PASS.into()]);
    let s = session("carol", SSHD);
    let client = connect_with(&s, script.clone()).await.unwrap();
    let out = run(&client, &s, "id -un").await;
    assert!(out.text.contains("carol"), "{out:?}");
    assert!(script.prompts().iter().any(|p| matches!(p, PromptKind::KeyboardInteractive { .. })));
    assert!(!script.prompts().iter().any(|p| matches!(p, PromptKind::Password { .. })));
}

#[tokio::test]
#[ignore = "lab"]
async fn ssh_auth_wrong_password_then_retry() {
    let script = Script::new(HostKeyDecision::AcceptOnce, &["wrong-pass", ALICE_PASS]);
    let s = session("alice", SSHD);
    connect_with(&s, script.clone()).await.unwrap();
    let pw: Vec<bool> = script
        .prompts()
        .iter()
        .filter_map(|p| match p {
            PromptKind::Password { retry, .. } => Some(*retry),
            _ => None,
        })
        .collect();
    // The rejected password is never reused: the user is asked again, flagged as a retry.
    assert_eq!(pw, [false, true]);
}

// ---- host-key trust --------------------------------------------------------------------------

/// The lab server's current host key (fetched with accept-and-save; auth is cancelled).
async fn server_key(port: u16) -> PublicKey {
    let trust = Arc::new(TrustStore::default());
    let kh = tmp_dir("kh").join("known_hosts");
    let script = Script::new(HostKeyDecision::AcceptAndSave, &[]);
    let r = timeout(rxt_ssh::connect(&session("alice", port), ctx(script, trust.clone(), &kh))).await;
    assert!(r.is_err());
    trust.keys("127.0.0.1", port).pop().expect("host key saved")
}

#[tokio::test]
#[ignore = "lab"]
async fn ssh_hostkey_unknown_prompt_then_accept_and_save() {
    let trust = Arc::new(TrustStore::default());
    let kh = tmp_dir("kh").join("known_hosts");
    let s = session("alice", SSHD);
    let first = Script::new(HostKeyDecision::AcceptAndSave, &[ALICE_PASS]);
    timeout(rxt_ssh::connect(&s, ctx(first.clone(), trust.clone(), &kh))).await.unwrap();
    let prompts = first.prompts();
    match &prompts[0] {
        PromptKind::HostKeyUnknown { host, port, fingerprint, .. } => {
            assert_eq!((host.as_str(), *port), ("127.0.0.1", SSHD));
            assert!(fingerprint.starts_with("SHA256:"), "{fingerprint}");
        }
        other => panic!("first prompt must be the host key, got {other:?}"),
    }
    assert_eq!(trust.keys("127.0.0.1", SSHD).len(), 1);

    // Saved: the second connect with the same trust store asks only for the password.
    let second = Script::new(HostKeyDecision::Cancel, &[ALICE_PASS]);
    timeout(rxt_ssh::connect(&s, ctx(second.clone(), trust, &kh))).await.unwrap();
    assert_eq!(second.host_key_prompts(), 0, "{:?}", second.prompts());

    // Cancelling an unknown key refuses the connection before any credential is asked.
    let cancel = Script::new(HostKeyDecision::Cancel, &[ALICE_PASS]);
    let r = connect_with(&s, cancel.clone()).await;
    assert!(matches!(r, Err(Error::HostKey(_))), "{:?}", r.err());
    assert_eq!(cancel.credential_prompts(), 0);
}

fn other_key_line(label: &str, real: &PublicKey) -> String {
    assert!(matches!(real.key_data(), KeyData::Ed25519(_)), "lab negotiates ed25519 host keys");
    let fake = PublicKey::new(KeyData::Ed25519(Ed25519PublicKey([0x5a; 32])), "");
    format!("{label} {}\n", fake.to_openssh().unwrap())
}

#[tokio::test]
#[ignore = "lab"]
async fn ssh_hostkey_changed_refused_without_auth() {
    let real = server_key(SSHD).await;
    let dir = tmp_dir("changed");
    let kh = dir.join("known_hosts");
    std::fs::write(&kh, other_key_line("[127.0.0.1]:2222", &real)).unwrap();
    let user = format!("ghost-{}", unique("u").replace(['-', '_'], ""));
    let s = session(&user, SSHD);

    let script = Script::new(HostKeyDecision::AcceptOnce, &["x"]);
    let r = timeout(rxt_ssh::connect(&s, ctx(script.clone(), Arc::default(), &kh))).await;
    assert!(matches!(&r, Err(Error::HostKey(m)) if m.contains("changed")), "{:?}", r.err());
    let changed = script.changed.lock().unwrap().clone();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].0, "[127.0.0.1]:2222");
    assert_ne!(changed[0].1, changed[0].2);
    assert!(script.prompts().is_empty(), "nothing may be asked on a changed key: {:?}", script.prompts());

    // Positive control: the same user name reaches the server log only once auth is attempted.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!log_since("sshd", 0).contains(&user), "user name sent before trust");
    let control = Script::new(HostKeyDecision::AcceptOnce, &[]);
    assert!(connect_with(&s, control).await.is_err());
    let mut seen = false;
    for _ in 0..50 {
        if log_since("sshd", 0).contains(&format!("Invalid user {user}")) {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(seen, "control auth attempt not found in the sshd log");
}

#[tokio::test]
#[ignore = "lab"]
async fn ssh_hostkey_revoked_refused() {
    let real = server_key(SSHD).await;
    let kh = tmp_dir("revoked").join("known_hosts");
    std::fs::write(&kh, format!("@revoked [127.0.0.1]:2222 {}\n", real.to_openssh().unwrap())).unwrap();
    let script = Script::new(HostKeyDecision::AcceptOnce, &[ALICE_PASS]);
    let r = timeout(rxt_ssh::connect(&session("alice", SSHD), ctx(script.clone(), Arc::default(), &kh))).await;
    assert!(matches!(&r, Err(Error::HostKey(m)) if m.contains("revoked")), "{:?}", r.err());
    assert!(script.prompts().is_empty(), "{:?}", script.prompts());
}

#[tokio::test]
#[ignore = "lab"]
async fn ssh_hostkey_unreadable_known_hosts_blocks() {
    use std::os::unix::fs::PermissionsExt;
    let kh = tmp_dir("unreadable").join("known_hosts");
    std::fs::write(&kh, "").unwrap();
    std::fs::set_permissions(&kh, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&kh).is_ok() {
        panic!("tests must run as a non-root user so chmod 000 makes the file unreadable");
    }
    let script = Script::new(HostKeyDecision::AcceptOnce, &[ALICE_PASS]);
    let r = timeout(rxt_ssh::connect(&session("alice", SSHD), ctx(script.clone(), Arc::default(), &kh))).await;
    let msg = match r {
        Err(Error::HostKey(m)) => m,
        other => panic!("expected a host-key error, got {:?}", other.err()),
    };
    assert!(msg.contains(&kh.display().to_string()), "{msg}");
    assert!(script.prompts().is_empty());
}

// ---- jump hosts ------------------------------------------------------------------------------

fn via_bastion(target_user: &str) -> rxt_core::Session {
    let mut s = rxt_core::Session::new(rxt_core::Protocol::Ssh, "internal", Some(target_user.into()), Some(22));
    s.ssh.use_agent = false;
    s.ssh.jump_hosts.push(JumpHost { host: "127.0.0.1".into(), port: BASTION, user: "alice".into(), key_file: None });
    s
}

/// Index of the first prompt matching `f`.
fn pos(prompts: &[PromptKind], f: impl Fn(&PromptKind) -> bool) -> usize {
    prompts.iter().position(f).unwrap_or_else(|| panic!("prompt missing in {prompts:?}"))
}

#[tokio::test]
#[ignore = "lab"]
async fn ssh_jump_chain_verifies_each_hop_before_auth() {
    let trust = Arc::new(TrustStore::default());
    let kh = tmp_dir("jump").join("known_hosts");
    let s = via_bastion("alice");
    let script = Script::new(HostKeyDecision::AcceptAndSave, &[ALICE_PASS, ALICE_PASS]);
    let client = timeout(rxt_ssh::connect(&s, ctx(script.clone(), trust.clone(), &kh))).await.unwrap();
    let out = run(&client, &s, "hostname").await;
    assert!(out.text.contains("internal"), "{out:?}");

    let p = script.prompts();
    let key_bastion = pos(&p, |k| matches!(k, PromptKind::HostKeyUnknown { port, .. } if *port == BASTION));
    let pw_bastion = pos(&p, |k| matches!(k, PromptKind::Password { host, .. } if host == "127.0.0.1"));
    let key_internal = pos(&p, |k| matches!(k, PromptKind::HostKeyUnknown { host, .. } if host == "internal"));
    let pw_internal = pos(&p, |k| matches!(k, PromptKind::Password { host, .. } if host == "internal"));
    assert!(key_bastion < pw_bastion && pw_bastion < key_internal && key_internal < pw_internal, "{p:?}");

    // A changed key on the inner hop is refused before any credential for that hop.
    let real = trust.keys("internal", 22).pop().unwrap();
    std::fs::write(&kh, other_key_line("internal", &real)).unwrap();
    let only_bastion = Arc::new(TrustStore::default());
    only_bastion.add("127.0.0.1", BASTION, trust.keys("127.0.0.1", BASTION).pop().unwrap());
    let script = Script::new(HostKeyDecision::AcceptOnce, &[ALICE_PASS, ALICE_PASS]);
    let r = timeout(rxt_ssh::connect(&s, ctx(script.clone(), only_bastion, &kh))).await;
    assert!(matches!(r, Err(Error::HostKey(_))), "{:?}", r.err());
    assert_eq!(script.changed.lock().unwrap()[0].0, "internal");
    assert!(
        !script.prompts().iter().any(|k| matches!(k, PromptKind::Password { host, .. } if host == "internal")),
        "{:?}",
        script.prompts()
    );
}

// ---- session options -------------------------------------------------------------------------

#[tokio::test]
#[ignore = "lab"]
async fn ssh_compression_and_keepalive_in_server_log() {
    let offset = log_len("sshd");
    let mut s = session("alice", SSHD);
    s.ssh.compression = true;
    s.ssh.keepalive_secs = 2;
    let client = connect_with(&s, Script::new(HostKeyDecision::AcceptOnce, &[ALICE_PASS])).await.unwrap();
    let out = run(&client, &s, "echo compressed-ok").await;
    assert!(out.text.contains("compressed-ok"));
    tokio::time::sleep(Duration::from_secs(7)).await; // idle: ~3 keepalives at 2 s
    assert!(!client.is_closed());
    let log = log_since("sshd", offset);
    assert!(log.contains("compression: zlib@openssh.com"), "zlib@openssh.com not negotiated");
    let keepalives = log.matches("rtype keepalive@openssh.com").count();
    assert!(keepalives >= 2, "expected keepalive requests within 7 s at 2 s interval, saw {keepalives}");
}

#[tokio::test]
#[ignore = "lab"]
async fn ssh_environment_and_startup_directory() {
    let mut s = session("alice", SSHD);
    s.ssh.startup_dir = Some("/tmp".into());
    s.ssh.env.push(("FOO".into(), "bar".into()));
    let client = alice(SSHD).await;
    let mut shell = timeout(client.shell(&s, 120, 40)).await.unwrap();
    shell.writer().write(b"echo \"D=$(pwd) F=$FOO\"\n").await.unwrap();
    let out = read_until(&mut shell, |o| o.text.contains("D=/tmp F=bar")).await;
    assert!(!out.closed, "{out:?}");
}

#[tokio::test]
#[ignore = "lab"]
async fn ssh_remote_command_output_then_exit_status() {
    let client = alice(SSHD).await;
    let out = run(&client, &session("alice", SSHD), "uname -a").await;
    assert!(out.text.contains("Linux"), "{out:?}");
    assert_eq!(out.exit, Some(0));
}

/// Kills the child process on drop.
struct Kill(std::process::Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

const X11_TEST: &str = "ssh_x11_forwarding_with_cookie_substitution";

/// The X11 parent starts Xvfb (with a cookie it requires) and re-runs this test in a child process
/// with `DISPLAY`/`XAUTHORITY` set, since the test process cannot change its own environment.
#[tokio::test]
#[ignore = "lab"]
async fn ssh_x11_forwarding_with_cookie_substitution() {
    if std::env::var_os("RXT_X11_CHILD").is_some() {
        return x11_child().await;
    }
    let dir = tmp_dir("x11");
    let cookie = "0123456789abcdef0123456789abcdef";
    let server_auth = dir.join("server.xauth");
    let xauth = |file: &std::path::Path, display: &str| {
        let st = std::process::Command::new("xauth")
            .arg("-q")
            .arg("-f")
            .arg(file)
            .args(["add", display, ".", cookie])
            .status()
            .expect("xauth is installed in the toolchain image");
        assert!(st.success());
    };
    xauth(&server_auth, ":0");
    let mut xvfb = Kill(
        std::process::Command::new("Xvfb")
            .args(["-displayfd", "1", "-nolisten", "tcp", "-auth"])
            .arg(&server_auth)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("Xvfb is installed in the toolchain image"),
    );
    let mut number = String::new();
    let mut stdout = xvfb.0.stdout.take().unwrap();
    let mut byte = [0u8; 1];
    while stdout.read(&mut byte).unwrap() == 1 && byte[0] != b'\n' {
        number.push(char::from(byte[0]));
    }
    let display = format!(":{}", number.trim());
    // The display really requires the cookie: a setup without authentication is refused (status 0).
    let mut x = std::os::unix::net::UnixStream::connect(format!("/tmp/.X11-unix/X{}", number.trim())).unwrap();
    std::io::Write::write_all(&mut x, &[b'l', 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    let mut status = [0u8; 1];
    x.read_exact(&mut status).unwrap();
    assert_eq!(status[0], 0, "Xvfb accepted an unauthenticated client");
    let client_auth = dir.join("client.xauth");
    xauth(&client_auth, &display);

    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", X11_TEST, "--nocapture", "--test-threads=1"])
        .env("RXT_X11_CHILD", "1")
        .env("DISPLAY", &display)
        .env("XAUTHORITY", &client_auth)
        .output()
        .unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && text.contains("1 passed"), "X11 child failed:\n{text}");
    drop(xvfb);
}

async fn x11_child() {
    let mut s = session("alice", SSHD);
    s.ssh.x11_forwarding = true;
    let client = connect_with(&s, Script::new(HostKeyDecision::AcceptOnce, &[ALICE_PASS])).await.unwrap();
    let cmd = "xdpyinfo >/dev/null && echo X11-GOOD; \
               XA=$(mktemp); xauth -q -f \"$XA\" add \"$DISPLAY\" MIT-MAGIC-COOKIE-1 00112233445566778899aabbccddeeff; \
               XAUTHORITY=\"$XA\" xdpyinfo >/dev/null 2>&1 || echo X11-BAD-REJECTED; rm -f \"$XA\"";
    let out = run(&client, &s, cmd).await;
    assert!(out.text.contains("X11-GOOD"), "forwarded xdpyinfo failed: {out:?}");
    assert!(out.text.contains("X11-BAD-REJECTED"), "wrong cookie was not rejected: {out:?}");
}

#[tokio::test]
#[ignore = "lab"]
async fn ssh_x11_not_requested_has_no_display() {
    let client = alice(SSHD).await;
    let s = session("alice", SSHD);
    assert!(!s.ssh.x11_forwarding, "X11 forwarding is off by default");
    let out = run(&client, &s, "xdpyinfo >/dev/null 2>&1; echo \"rc=$? display=[$DISPLAY]\"").await;
    assert!(out.text.contains("display=[]"), "{out:?}");
    assert!(!out.text.contains("rc=0"), "{out:?}");
}

// ---- tunnels ---------------------------------------------------------------------------------

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// A non-loopback address of this host (no packet is sent).
fn non_loopback_ip() -> std::net::IpAddr {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    s.connect("192.0.2.1:9").expect("a default route to find a non-loopback address");
    let ip = s.local_addr().unwrap().ip();
    assert!(!ip.is_loopback() && !ip.is_unspecified());
    ip
}

async fn start(
    client: &rxt_ssh::Client,
    tunnel: Tunnel,
) -> (CancellationToken, Arc<std::sync::Mutex<Vec<TunnelState>>>) {
    let states = Arc::new(std::sync::Mutex::new(Vec::new()));
    let st = states.clone();
    let cancel = CancellationToken::new();
    timeout(client.start_tunnel(&tunnel, cancel.clone(), Arc::new(move |s| st.lock().unwrap().push(s)))).await.unwrap();
    assert_eq!(states.lock().unwrap().last(), Some(&TunnelState::Running));
    (cancel, states)
}

async fn read_banner(stream: &mut TcpStream) -> String {
    let mut buf = vec![0u8; 64];
    let n = timeout(stream.read(&mut buf)).await.unwrap();
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

async fn assert_not_exposed(port: u16) {
    let r = tokio::time::timeout(Duration::from_secs(3), TcpStream::connect((non_loopback_ip(), port))).await;
    assert!(!matches!(r, Ok(Ok(_))), "tunnel port {port} reachable from a non-loopback address");
}

#[tokio::test]
#[ignore = "lab"]
async fn ssh_tunnel_local_forward_via_bastion() {
    let client = alice(BASTION).await;
    let port = free_port();
    let tunnel = Tunnel {
        kind: TunnelKind::Local,
        bind_port: port,
        dest_host: "internal".into(),
        dest_port: 22,
        ..Tunnel::default()
    };
    let (cancel, states) = start(&client, tunnel).await;
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    assert!(read_banner(&mut s).await.starts_with("SSH-2.0-OpenSSH"));
    assert_not_exposed(port).await;
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(states.lock().unwrap().last(), Some(&TunnelState::Stopped));
}

#[tokio::test]
#[ignore = "lab"]
async fn ssh_tunnel_dynamic_socks5_via_bastion() {
    let client = alice(BASTION).await;
    let port = free_port();
    let (cancel, _) = start(&client, Tunnel { kind: TunnelKind::Dynamic, bind_port: port, ..Tunnel::default() }).await;
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut hello = [0u8; 2];
    s.read_exact(&mut hello).await.unwrap();
    assert_eq!(hello, [5, 0]);
    let mut req = vec![5, 1, 0, 3, 8];
    req.extend_from_slice(b"internal");
    req.extend_from_slice(&22u16.to_be_bytes());
    s.write_all(&req).await.unwrap();
    let mut reply = [0u8; 10];
    timeout(s.read_exact(&mut reply)).await.unwrap();
    assert_eq!(reply[..2], [5, 0]);
    assert!(read_banner(&mut s).await.starts_with("SSH-2.0-OpenSSH"));
    assert_not_exposed(port).await;
    cancel.cancel();
}

#[tokio::test]
#[ignore = "lab"]
async fn ssh_tunnel_remote_forward_loopback_only() {
    let local = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_port = local.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut c, _)) = local.accept().await {
            let _ = c.write_all(b"HELLO-REMOTE\n").await;
        }
    });
    let client = alice(SSHD).await;
    let remote_port = 20000 + u16::try_from(std::process::id() % 10000).unwrap();
    let tunnel = Tunnel {
        kind: TunnelKind::Remote,
        bind_port: remote_port,
        dest_host: "127.0.0.1".into(),
        dest_port: local_port,
        ..Tunnel::default()
    };
    let (cancel, _) = start(&client, tunnel).await;
    let cmd = format!(
        "exec 3<>/dev/tcp/127.0.0.1/{remote_port} && head -n1 <&3; \
         ip=$(hostname -I | cut -d' ' -f1); \
         (exec 4<>/dev/tcp/$ip/{remote_port}) 2>/dev/null && echo EXPOSED || echo NOT-EXPOSED-$ip"
    );
    let out = run(&client, &session("alice", SSHD), &cmd).await;
    assert!(out.text.contains("HELLO-REMOTE"), "{out:?}");
    assert!(out.text.contains("NOT-EXPOSED-"), "{out:?}");
    assert!(!out.text.lines().any(|l| l.trim() == "EXPOSED"), "{out:?}");
    cancel.cancel();
}

// ---- disconnect / reconnect ------------------------------------------------------------------

/// Substitute for "reconnect after sshd restart": the test container cannot restart lab
/// containers, so the server side of the session is killed instead.
#[tokio::test]
#[ignore = "lab"]
async fn ssh_disconnect_detected_then_reconnect() {
    let s = session("alice", SSHD);
    let client = alice(SSHD).await;
    let mut shell = timeout(client.shell(&s, 80, 24)).await.unwrap();
    shell.writer().write(b"echo READY; kill -KILL $PPID\n").await.unwrap();
    let out = read_until(&mut shell, |o| o.closed).await;
    assert!(out.text.contains("READY"), "{out:?}");
    for _ in 0..50 {
        if client.is_closed() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(client.is_closed(), "dropped connection not detected");

    let again = alice(SSHD).await;
    let out = run(&again, &s, "echo RECONNECTED").await;
    assert!(out.text.contains("RECONNECTED"), "{out:?}");
}
