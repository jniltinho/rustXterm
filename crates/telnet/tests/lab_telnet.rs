//! Lab tests against the `telnetd` (127.0.0.1:2323) and raw `echo` (127.0.0.1:2007) services.
//! Run with `lab/lab.sh up` and `cargo test -p rxt-telnet -- --ignored` on the host network.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{ctx, read_until, session};
use rxt_telnet::TelnetSession;

const TELNETD: u16 = 2323;
const ECHO: u16 = 2007;
const ALICE_PASS: &str = "lab-alice-pass";
const T: Duration = Duration::from_secs(20);
/// Shell prompt set right after login so command output is easy to find.
const PROMPT: &[u8] = b"RXT> ";

/// Waits for the login shell prompt (typeahead is flushed by `login`), then sets [`PROMPT`].
async fn shell_ready(t: &mut TelnetSession) -> Vec<u8> {
    let mut out = read_until(t, b"$ ", T).await;
    t.writer().write(b"PS1='RXT''> '\r").await.unwrap();
    out.extend(read_until(t, PROMPT, T).await);
    out
}

async fn manual_login(cols: u16, rows: u16) -> TelnetSession {
    let mut t = rxt_telnet::connect(&session(TELNETD, None), ctx(None), cols, rows).await.unwrap();
    read_until(&mut t, b"login: ", T).await;
    t.writer().write(b"alice\r").await.unwrap();
    read_until(&mut t, b"assword: ", T).await;
    t.writer().write(format!("{ALICE_PASS}\r").as_bytes()).await.unwrap();
    shell_ready(&mut t).await;
    t
}

async fn run(t: &mut TelnetSession, cmd: &str, expect: &[u8]) -> Vec<u8> {
    t.writer().write(format!("{cmd}\r").as_bytes()).await.unwrap();
    read_until(t, expect, T).await
}

#[tokio::test]
#[ignore = "lab"]
async fn telnet_login_echo_and_terminal_type() {
    let mut t = manual_login(80, 24).await;
    run(&mut t, "echo rxt-$((40+2))", b"rxt-42").await;
    run(&mut t, "echo T=$TERM", b"T=xterm-256color").await;
}

#[tokio::test]
#[ignore = "lab"]
async fn telnet_naws_resize_seen_by_stty_size() {
    let mut t = manual_login(80, 24).await;
    run(&mut t, "stty size", b"24 80").await;
    t.writer().resize(120, 40).await.unwrap();
    run(&mut t, "stty size", b"40 120").await;
}

#[tokio::test]
#[ignore = "lab"]
async fn telnet_auto_login_opt_in() {
    let mut s = session(TELNETD, Some("alice"));
    s.telnet.auto_login = true;
    let mut t = rxt_telnet::connect(&s, ctx(Some(ALICE_PASS)), 80, 24).await.unwrap();
    let mut out = read_until(&mut t, b"assword: ", T).await;
    out.extend(shell_ready(&mut t).await);
    out.extend(run(&mut t, "echo rxt-$((1+1))", b"rxt-2").await);
    assert!(!out.windows(ALICE_PASS.len()).any(|w| w == ALICE_PASS.as_bytes()), "password echoed");
}

#[tokio::test]
#[ignore = "lab"]
async fn telnet_auto_login_opt_out_types_nothing() {
    let s = session(TELNETD, Some("alice"));
    let mut t = rxt_telnet::connect(&s, ctx(Some(ALICE_PASS)), 80, 24).await.unwrap();
    let mut out = read_until(&mut t, b"login: ", T).await;
    while let Ok(Some(chunk)) = tokio::time::timeout(Duration::from_secs(2), t.read()).await {
        out.extend(chunk);
    }
    assert!(!out.windows(8).any(|w| w.eq_ignore_ascii_case(b"password")), "{}", String::from_utf8_lossy(&out));
}

#[tokio::test]
#[ignore = "lab"]
async fn telnet_raw_echo() {
    let mut s = session(ECHO, None);
    s.telnet.raw = true;
    let mut t = rxt_telnet::connect(&s, ctx(None), 80, 24).await.unwrap();
    let bytes = [0xFF, 0xFD, 0x01, b'r', b'x', b't', b'\r', b'\n'];
    t.writer().write(&bytes).await.unwrap();
    assert_eq!(read_until(&mut t, b"rxt\r\n", T).await, bytes);
}
