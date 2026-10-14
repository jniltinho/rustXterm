//! Driver tests over an in-process TCP pair, including a hostile Telnet peer
//! (option ping-pong, oversized subnegotiation, IAC storm).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{ctx, read_until, session};
use rxt_core::Newline;
use rxt_telnet::codec::{DO, DONT, ECHO, IAC, NAWS, SB, SE, SGA, TTYPE, WILL, WONT};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const T: Duration = Duration::from_secs(10);
const OFFERS: [u8; 12] = [IAC, WILL, TTYPE, IAC, WILL, NAWS, IAC, DO, SGA, IAC, DO, ECHO];

async fn listener() -> (TcpListener, u16) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    (l, port)
}

async fn read_exact(s: &mut TcpStream, n: usize) -> Vec<u8> {
    let mut buf = vec![0; n];
    tokio::time::timeout(T, s.read_exact(&mut buf)).await.unwrap().unwrap();
    buf
}

/// Bytes the peer receives within `quiet` of silence.
async fn drain(s: &mut TcpStream, quiet: Duration) -> Vec<u8> {
    let mut all = Vec::new();
    let mut buf = [0; 4096];
    while let Ok(Ok(n)) = tokio::time::timeout(quiet, s.read(&mut buf)).await {
        if n == 0 {
            break;
        }
        all.extend_from_slice(&buf[..n]);
    }
    all
}

#[tokio::test]
async fn negotiation_naws_resize_and_input_encoding() {
    let (l, port) = listener().await;
    let mut s = session(port, None);
    s.telnet.newline = Newline::CrNul;
    let mut t = rxt_telnet::connect(&s, ctx(None), 80, 24).await.unwrap();
    let (mut peer, _) = l.accept().await.unwrap();
    assert_eq!(read_exact(&mut peer, OFFERS.len()).await, OFFERS);

    peer.write_all(&[IAC, DO, NAWS, IAC, DO, TTYPE, IAC, SB, TTYPE, 1, IAC, SE, IAC, WILL, ECHO, b'h', b'i'])
        .await
        .unwrap();
    assert_eq!(read_until(&mut t, b"hi", T).await, b"hi");
    let mut want = vec![IAC, SB, NAWS, 0, 80, 0, 24, IAC, SE, IAC, SB, TTYPE, 0];
    want.extend(b"xterm-256color");
    want.extend([IAC, SE]);
    assert_eq!(read_exact(&mut peer, want.len()).await, want);

    let w = t.writer();
    w.resize(120, 40).await.unwrap();
    assert_eq!(read_exact(&mut peer, 9).await, [IAC, SB, NAWS, 0, 120, 0, 40, IAC, SE]);
    w.write(&[b'l', b's', 0xFF, b'\r']).await.unwrap();
    assert_eq!(read_exact(&mut peer, 6).await, [b'l', b's', IAC, IAC, b'\r', 0]);
}

#[tokio::test]
async fn raw_mode_passes_bytes_unchanged() {
    let (l, port) = listener().await;
    let mut s = session(port, None);
    s.telnet.raw = true;
    let mut t = rxt_telnet::connect(&s, ctx(None), 80, 24).await.unwrap();
    let (mut peer, _) = l.accept().await.unwrap();
    let bytes = [IAC, DO, ECHO, 0, b'\r', b'\n', 0xFE];
    t.writer().write(&bytes).await.unwrap();
    t.writer().resize(100, 50).await.unwrap();
    assert_eq!(read_exact(&mut peer, bytes.len()).await, bytes, "no offers, no escaping, no NAWS");
    peer.write_all(&bytes).await.unwrap();
    assert_eq!(read_until(&mut t, &[0xFE], T).await, bytes);
    assert!(drain(&mut peer, Duration::from_millis(200)).await.is_empty(), "no negotiation replies");
}

#[tokio::test]
async fn auto_login_answers_each_prompt_once() {
    let (l, port) = listener().await;
    let mut s = session(port, Some("alice"));
    s.telnet.auto_login = true;
    let mut t = rxt_telnet::connect(&s, ctx(Some("lab-alice-pass")), 80, 24).await.unwrap();
    let (mut peer, _) = l.accept().await.unwrap();
    read_exact(&mut peer, OFFERS.len()).await;

    peer.write_all(b"\r\nhost login: ").await.unwrap();
    read_until(&mut t, b"login: ", T).await;
    assert_eq!(read_exact(&mut peer, 7).await, b"alice\r\n");
    peer.write_all(b"Password: ").await.unwrap();
    let out = read_until(&mut t, b"Password: ", T).await;
    assert_eq!(read_exact(&mut peer, 16).await, b"lab-alice-pass\r\n");
    assert!(!out.windows(5).any(|w| w == b"lab-a"), "password never appears in the output");
    peer.write_all(b"\r\nPassword: login: $ ").await.unwrap();
    read_until(&mut t, b"$ ", T).await;
    assert!(drain(&mut peer, Duration::from_millis(300)).await.is_empty(), "nothing typed twice");
}

#[tokio::test]
async fn auto_login_off_types_nothing() {
    let (l, port) = listener().await;
    let s = session(port, Some("alice"));
    let mut t = rxt_telnet::connect(&s, ctx(Some("lab-alice-pass")), 80, 24).await.unwrap();
    let (mut peer, _) = l.accept().await.unwrap();
    read_exact(&mut peer, OFFERS.len()).await;
    peer.write_all(b"login: ").await.unwrap();
    read_until(&mut t, b"login: ", T).await;
    peer.write_all(b"Password: ").await.unwrap();
    read_until(&mut t, b"Password: ", T).await;
    assert!(drain(&mut peer, Duration::from_millis(300)).await.is_empty());
}

#[tokio::test]
async fn cancel_and_close_end_the_session() {
    let (l, port) = listener().await;
    let c = ctx(None);
    let cancel = c.cancel.clone();
    let mut t = rxt_telnet::connect(&session(port, None), c, 80, 24).await.unwrap();
    let (_peer, _) = l.accept().await.unwrap();
    let w = t.writer();
    cancel.cancel();
    assert_eq!(tokio::time::timeout(T, t.read()).await.unwrap(), None);
    drop(t);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(w.write(b"x").await.is_err(), "writer fails after cancellation");

    // Remote close.
    let mut t = rxt_telnet::connect(&session(port, None), ctx(None), 80, 24).await.unwrap();
    let (peer, _) = l.accept().await.unwrap();
    drop(peer);
    assert_eq!(tokio::time::timeout(T, t.read()).await.unwrap(), None);
}

#[tokio::test]
async fn connect_refused_is_a_connection_error() {
    let (l, port) = listener().await;
    drop(l);
    let r = rxt_telnet::connect(&session(port, None), ctx(None), 80, 24).await;
    assert!(matches!(r, Err(rxt_core::Error::Connection(_))));
}

// ---- hostile peer -------------------------------------------------------------------------------

/// Peer that sends `attack` then `marker`, and counts the bytes the client sent back meanwhile.
async fn hostile(attack: Vec<u8>) -> usize {
    let (l, port) = listener().await;
    let mut t = rxt_telnet::connect(&session(port, None), ctx(None), 80, 24).await.unwrap();
    let (peer, _) = l.accept().await.unwrap();
    let (mut prd, mut pwr) = peer.into_split();
    let counter = tokio::spawn(async move {
        let mut n = 0;
        let mut buf = [0; 4096];
        while let Ok(Ok(k)) = tokio::time::timeout(Duration::from_millis(500), prd.read(&mut buf)).await {
            if k == 0 {
                break;
            }
            n += k;
        }
        n
    });
    let writer = tokio::spawn(async move {
        pwr.write_all(&attack).await.unwrap();
        pwr.write_all(b"MARKER").await.unwrap();
        pwr
    });
    let out = read_until(&mut t, b"MARKER", T).await;
    assert!(out.ends_with(b"MARKER"));
    let _pwr = writer.await.unwrap();
    counter.await.unwrap()
}

#[tokio::test]
async fn hostile_option_ping_pong_is_bounded() {
    let mut attack = Vec::new();
    for _ in 0..50_000 {
        attack.extend([IAC, WILL, SGA, IAC, WONT, SGA, IAC, DO, 99, IAC, DONT, 99]);
    }
    let replies = hostile(attack).await;
    // Offers + at most TOGGLE_LIMIT three-byte answers for each of the two options.
    assert!(replies <= OFFERS.len() + 2 * 32 * 3 + 9, "{replies} bytes answered");
}

#[tokio::test]
async fn hostile_oversized_subnegotiation_is_discarded() {
    let mut attack = vec![IAC, SB, TTYPE, 1];
    attack.resize(4 + (1 << 20), b'Z');
    attack.extend([IAC, SE]);
    let replies = hostile(attack).await;
    assert_eq!(replies, OFFERS.len());
}

#[tokio::test]
async fn hostile_iac_storm_is_bounded() {
    let mut attack = Vec::new();
    for i in 0..200_000u32 {
        // NOP, GA, AYT, stray SE, and requests for every option number.
        attack.extend([IAC, 241, IAC, 249, IAC, 246, IAC, SE, IAC, DO, (i % 256) as u8, IAC, WILL, (i % 251) as u8]);
    }
    let replies = hostile(attack).await;
    assert!(replies <= OFFERS.len() + 2 * 256 * 32 * 9, "{replies} bytes answered");
}
