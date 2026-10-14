//! In-process RFB server tests (no lab): handshake variants, VncAuth vector, decoders, and the
//! hostile-server limits from design D13 (oversized sizes, stalls, cancellation).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{Auth, client_messages, ctx, next, pair, rect, script, session, update};
use rxt_core::{DesktopEvent, DesktopInput, Error, PromptKind, Rect};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// DES-ECB of challenge 00..0f with the bit-reversed key "password" (computed with OpenSSL).
const PASSWORD_VECTOR: [u8; 16] =
    [0xb8, 0x66, 0x92, 0x41, 0x25, 0xc8, 0xee, 0xbb, 0x9d, 0xeb, 0xc1, 0xdb, 0x61, 0xc5, 0x38, 0xe2];

async fn resized(desktop: &mut rxt_vnc::Desktop) -> (u32, u32) {
    match next(desktop, 5).await {
        Some(Ok(DesktopEvent::Resized { width, height })) => (width, height),
        other => panic!("expected Resized, got {other:?}"),
    }
}

#[tokio::test]
async fn vncauth_matches_known_vector_and_prompts_with_empty_user() {
    let (mut server, stream) = pair();
    let s = script(Some("password"));
    let srv = tokio::spawn(async move {
        assert!(server.handshake(b"RFB 003.008\n", Auth::Vnc(PASSWORD_VECTOR), 8, 8).await);
        server
    });
    let mut desktop = rxt_vnc::connect(&session(5900), ctx(s.clone()), Some(stream)).await.unwrap();
    assert_eq!(resized(&mut desktop).await, (8, 8));
    let _server = srv.await.unwrap();
    assert!(matches!(&s.prompts.lock().unwrap()[..], [PromptKind::Password { user, .. }] if user.is_empty()));
}

#[tokio::test]
async fn wrong_password_is_an_auth_error_and_cancel_is_cancelled() {
    for (password, version) in [(Some("nope"), b"RFB 003.008\n"), (Some("nope"), b"RFB 003.007\n")] {
        let (mut server, stream) = pair();
        let srv = tokio::spawn(async move { server.handshake(version, Auth::Vnc(PASSWORD_VECTOR), 8, 8).await });
        let err = rxt_vnc::connect(&session(5900), ctx(script(password)), Some(stream)).await.err();
        assert!(matches!(err, Some(Error::Auth(_))), "{err:?}");
        assert!(!srv.await.unwrap());
    }
    let (mut server, stream) = pair();
    let _srv = tokio::spawn(async move { server.handshake(b"RFB 003.008\n", Auth::Vnc(PASSWORD_VECTOR), 8, 8).await });
    let err = rxt_vnc::connect(&session(5900), ctx(script(None)), Some(stream)).await.err();
    assert!(matches!(err, Some(Error::Cancelled)), "{err:?}");
}

#[tokio::test]
async fn rfb_33_and_37_handshakes_without_auth() {
    for version in [b"RFB 003.003\n", b"RFB 003.007\n", b"RFB 003.008\n"] {
        let (mut server, stream) = pair();
        let srv = tokio::spawn(async move {
            server.handshake(version, Auth::None, 16, 4).await;
            server
        });
        let s = script(None);
        let mut desktop = rxt_vnc::connect(&session(5900), ctx(s.clone()), Some(stream)).await.unwrap();
        assert_eq!(resized(&mut desktop).await, (16, 4));
        assert!(s.prompts.lock().unwrap().is_empty(), "no prompt without auth");
        drop(srv.await.unwrap());
    }
}

#[tokio::test]
async fn decodes_raw_copyrect_and_zrle() {
    let (mut server, stream) = pair();
    let srv = tokio::spawn(async move {
        server.handshake(b"RFB 003.008\n", Auth::None, 4, 4).await;
        let mut msg = update(3);
        msg.extend(rect(0, 0, 2, 2, 0));
        msg.extend([1, 2, 3, 0, 4, 5, 6, 0, 7, 8, 9, 0, 10, 11, 12, 0]);
        msg.extend(rect(2, 0, 2, 2, 1));
        msg.extend([0, 0, 0, 0]);
        msg.extend(rect(0, 2, 4, 2, 16));
        // zlib header + one non-final stored block: tile control 1 (solid), CPIXEL 10,20,30.
        let zlib = [0x78, 0x01, 0x00, 4, 0, !4, 0xff, 1, 10, 20, 30];
        msg.extend((zlib.len() as u32).to_be_bytes());
        msg.extend(zlib);
        server.io.write_all(&msg).await.unwrap();
        server
    });
    let mut desktop = rxt_vnc::connect(&session(5900), ctx(script(None)), Some(stream)).await.unwrap();
    assert_eq!(resized(&mut desktop).await, (4, 4));
    let raw = Rect { x: 0, y: 0, width: 2, height: 2 };
    assert_eq!(
        next(&mut desktop, 5).await.unwrap().unwrap(),
        DesktopEvent::Update { rect: raw, rgba: vec![1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255] }
    );
    assert_eq!(
        next(&mut desktop, 5).await.unwrap().unwrap(),
        DesktopEvent::Copy { src_x: 0, src_y: 0, dst: Rect { x: 2, y: 0, width: 2, height: 2 } }
    );
    assert_eq!(
        next(&mut desktop, 5).await.unwrap().unwrap(),
        DesktopEvent::Update { rect: Rect { x: 0, y: 2, width: 4, height: 2 }, rgba: [10, 20, 30, 255].repeat(8) }
    );
    let _server = srv.await.unwrap();
}

#[tokio::test]
async fn hostile_framebuffer_size_is_rejected() {
    let (mut server, stream) = pair();
    let _srv = tokio::spawn(async move {
        server.handshake(b"RFB 003.008\n", Auth::None, u16::MAX, u16::MAX).await;
        server
    });
    let result =
        tokio::time::timeout(Duration::from_secs(5), rxt_vnc::connect(&session(5900), ctx(script(None)), Some(stream)))
            .await
            .unwrap();
    assert!(result.is_err());
}

/// Server sends `body` after a 64x64 ServerInit; the session must end with an error.
async fn ends_with_error(body: Vec<u8>) {
    let (mut server, stream) = pair();
    let _srv = tokio::spawn(async move {
        server.handshake(b"RFB 003.008\n", Auth::None, 64, 64).await;
        server.io.write_all(&body).await.unwrap();
        tokio::time::sleep(Duration::from_secs(3600)).await;
    });
    let mut desktop = rxt_vnc::connect(&session(5900), ctx(script(None)), Some(stream)).await.unwrap();
    assert_eq!(resized(&mut desktop).await, (64, 64));
    assert!(matches!(next(&mut desktop, 5).await, Some(Err(_))));
    assert!(next(&mut desktop, 5).await.is_none(), "session ends after the error");
}

#[tokio::test]
async fn hostile_oversized_rect_ends_session() {
    let mut body = update(1);
    body.extend(rect(0, 0, 8192, 8192, 0));
    ends_with_error(body).await;
}

#[tokio::test]
async fn hostile_huge_compressed_rect_ends_session() {
    let mut body = update(1);
    body.extend(rect(0, 0, 64, 64, 16));
    body.extend(u32::MAX.to_be_bytes());
    ends_with_error(body).await;
}

#[tokio::test(start_paused = true)]
async fn hostile_stall_mid_message_times_out_after_30s() {
    let (mut server, stream) = pair();
    let _srv = tokio::spawn(async move {
        server.handshake(b"RFB 003.008\n", Auth::None, 64, 64).await;
        let mut body = update(1);
        body.extend(rect(0, 0, 16, 16, 0));
        body.extend([0; 10]);
        server.io.write_all(&body).await.unwrap();
        // Keep reading so client requests never block; never send the rest.
        let mut sink = [0; 1024];
        while let Ok(1..) = server.io.read(&mut sink).await {}
    });
    let mut desktop = rxt_vnc::connect(&session(5900), ctx(script(None)), Some(stream)).await.unwrap();
    assert_eq!(resized(&mut desktop).await, (64, 64));
    let start = tokio::time::Instant::now();
    let event = next(&mut desktop, 60).await;
    let waited = start.elapsed();
    assert!(matches!(&event, Some(Err(Error::Protocol(m))) if m.contains("stalled")), "{event:?}");
    assert!(waited >= rxt_vnc::STALL_TIMEOUT && waited < rxt_vnc::STALL_TIMEOUT + Duration::from_secs(1), "{waited:?}");
}

#[tokio::test(start_paused = true)]
async fn idle_between_messages_is_not_a_stall() {
    let (mut server, stream) = pair();
    let _srv = tokio::spawn(async move {
        server.handshake(b"RFB 003.008\n", Auth::None, 8, 8).await;
        let (mut wr, _log, _task) = server.record();
        tokio::time::sleep(Duration::from_secs(120)).await;
        wr.write_all(&[2]).await.unwrap(); // Bell
        tokio::time::sleep(Duration::from_secs(3600)).await;
    });
    let mut desktop = rxt_vnc::connect(&session(5900), ctx(script(None)), Some(stream)).await.unwrap();
    resized(&mut desktop).await;
    assert_eq!(next(&mut desktop, 300).await.unwrap().unwrap(), DesktopEvent::Bell);
}

#[tokio::test]
async fn drop_during_stall_returns_immediately_and_releases_the_connection() {
    let (mut server, stream) = pair();
    let (eof_tx, eof_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        server.handshake(b"RFB 003.008\n", Auth::None, 64, 64).await;
        let mut body = update(1);
        body.extend(rect(0, 0, 16, 16, 0));
        server.io.write_all(&body).await.unwrap();
        let mut sink = [0; 1024];
        while let Ok(1..) = server.io.read(&mut sink).await {}
        let _ = eof_tx.send(());
    });
    let mut desktop = rxt_vnc::connect(&session(5900), ctx(script(None)), Some(stream)).await.unwrap();
    resized(&mut desktop).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let start = std::time::Instant::now();
    drop(desktop);
    assert!(start.elapsed() < Duration::from_millis(50));
    tokio::time::timeout(Duration::from_secs(2), eof_rx).await.unwrap().unwrap();
}

#[tokio::test]
async fn context_cancel_ends_the_session() {
    let (mut server, stream) = pair();
    let _srv = tokio::spawn(async move {
        server.handshake(b"RFB 003.008\n", Auth::None, 8, 8).await;
        tokio::time::sleep(Duration::from_secs(3600)).await;
    });
    let c = ctx(script(None));
    let cancel = c.cancel.clone();
    let mut desktop = rxt_vnc::connect(&session(5900), c, Some(stream)).await.unwrap();
    resized(&mut desktop).await;
    cancel.cancel();
    assert!(next(&mut desktop, 2).await.is_none());
}

#[tokio::test]
async fn view_only_sends_no_input_and_hidden_sends_no_requests() {
    let (mut server, stream) = pair();
    let srv = tokio::spawn(async move {
        server.handshake(b"RFB 003.008\n", Auth::None, 8, 8).await;
        server.record()
    });
    let mut s = session(5900);
    s.vnc.view_only = true;
    let desktop = rxt_vnc::connect(&s, ctx(script(None)), Some(stream)).await.unwrap();
    let (_wr, log, _task) = srv.await.unwrap();
    let input = desktop.input();
    for i in [
        DesktopInput::Key { keysym: 0x61, down: true },
        DesktopInput::Pointer { x: 1, y: 1, buttons: 1 },
        DesktopInput::Wheel { x: 1, y: 1, dx: 0, dy: 1 },
        DesktopInput::Clipboard("x".into()),
        DesktopInput::ReleaseAll,
    ] {
        input.send(i).unwrap();
    }
    input.send(DesktopInput::Visible(false)).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mark = log.lock().unwrap().len();
    tokio::time::sleep(Duration::from_secs(1)).await;
    input.send(DesktopInput::Visible(true)).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // ServerInit was read already; the recorder starts at the client's first message.
    let msgs = client_messages(&log.lock().unwrap(), 0);
    assert!(msgs.iter().all(|m| !matches!(m.1, 4..=6)), "view-only sent input: {msgs:?}");
    // Any request sent while hidden would precede the non-incremental one sent on show.
    let first = msgs.iter().find(|m| m.1 == 3 && m.0 >= mark);
    assert_eq!(first.map(|m| m.2), Some(0), "no requests while hidden, full redraw when shown: {msgs:?}");
    drop(desktop);
}
