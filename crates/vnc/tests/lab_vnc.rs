//! VNC lab tests (task 5.3) against TigerVNC on 127.0.0.1:5901 (`lab/lab.sh up`).
//! Run: `cargo test -p rxt-vnc -- --ignored`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{LAB_VNC_PASS, LAB_VNC_PORT, Recorder, VNCAUTH_HANDSHAKE, client_messages, ctx, next, script, session};
use rxt_core::{DesktopEvent, DesktopInput, Framebuffer, Protocol, Session};
use rxt_vnc::{Desktop, VncEncoding};
use tokio::net::TcpStream;

const SCREEN: u64 = 1024 * 768;

struct Lab {
    desktop: Desktop,
    sent: Arc<Mutex<Vec<u8>>>,
    received: Arc<std::sync::atomic::AtomicUsize>,
    fb: Framebuffer,
}

async fn open(s: &Session, encodings: &[VncEncoding]) -> Lab {
    let tcp = TcpStream::connect(("127.0.0.1", LAB_VNC_PORT)).await.unwrap();
    let rec = Recorder::new(tcp);
    let (sent, received) = (rec.sent.clone(), rec.received.clone());
    let desktop =
        rxt_vnc::connect_with(s, ctx(script(Some(LAB_VNC_PASS))), Some(Box::new(rec)), encodings).await.unwrap();
    Lab { desktop, sent, received, fb: Framebuffer::default() }
}

impl Lab {
    /// Apply events until `done` holds for the event just applied (30 s budget).
    async fn until(&mut self, mut done: impl FnMut(&DesktopEvent, &Framebuffer) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            assert!(Instant::now() < deadline, "timed out waiting for desktop events");
            let event = next(&mut self.desktop, 40).await.expect("session ended").expect("session error");
            self.fb.apply(&event).unwrap();
            if done(&event, &self.fb) {
                return;
            }
        }
    }

    /// Wait until updates covered at least one full screen.
    async fn full_screen(&mut self) -> u64 {
        let mut area = 0;
        self.until(|e, _| {
            if let DesktopEvent::Update { rect, .. } = e {
                area += rect.area();
            }
            area >= SCREEN
        })
        .await;
        area
    }

    /// Apply whatever arrives for `dur`.
    async fn drain(&mut self, dur: Duration) {
        let end = tokio::time::Instant::now() + dur;
        while let Ok(Some(Ok(e))) = tokio::time::timeout_at(end, self.desktop.next()).await {
            self.fb.apply(&e).unwrap();
        }
    }

    fn messages(&self) -> Vec<(usize, u8, u8)> {
        client_messages(&self.sent.lock().unwrap(), VNCAUTH_HANDSHAKE)
    }
}

fn vnc() -> Session {
    session(LAB_VNC_PORT)
}

#[tokio::test]
#[ignore = "lab"]
async fn vnc_auth_password() {
    let mut lab = open(&vnc(), &rxt_vnc::DEFAULT_ENCODINGS).await;
    lab.until(|e, _| matches!(e, DesktopEvent::Resized { width: 1024, height: 768 })).await;
    lab.full_screen().await;
    // The desktop is not uniformly black: the xterm is drawn.
    assert!(lab.fb.pixels.as_chunks::<4>().0.iter().any(|p| p[..3] != [0, 0, 0]));
    // Wrong passwords are covered in tests/rfb.rs: TigerVNC blacklists the client address after
    // failures, which would break the other lab tests running in parallel.
}

/// Forced single encoding (Raw is always allowed): compressed encodings must move far less than
/// raw pixels for the first full screen, which proves the server used them.
async fn encoding(enc: VncEncoding, compressed: bool) {
    let mut lab = open(&vnc(), &[enc]).await;
    lab.full_screen().await;
    let bytes = lab.received.load(Ordering::Relaxed) as u64;
    if compressed {
        assert!(bytes < SCREEN, "{enc:?} moved {bytes} bytes");
    } else {
        assert!(bytes >= SCREEN * 4, "Raw moved only {bytes} bytes");
    }
}

#[tokio::test]
#[ignore = "lab"]
async fn vnc_encoding_raw() {
    encoding(VncEncoding::Raw, false).await;
}

#[tokio::test]
#[ignore = "lab"]
async fn vnc_encoding_zrle() {
    encoding(VncEncoding::Zrle, true).await;
}

#[tokio::test]
#[ignore = "lab"]
async fn vnc_encoding_tight() {
    encoding(VncEncoding::Tight, true).await;
}

fn type_text(input: &rxt_vnc::InputSender, text: &str) {
    for c in text.chars() {
        let keysym = if c == '\n' { 0xff0d } else { u32::from(c) };
        input.send(DesktopInput::Key { keysym, down: true }).unwrap();
        input.send(DesktopInput::Key { keysym, down: false }).unwrap();
    }
}

#[tokio::test]
#[ignore = "lab"]
async fn vnc_encoding_copyrect_on_xterm_scroll() {
    let mut lab = open(&vnc(), &[VncEncoding::Zrle, VncEncoding::CopyRect]).await;
    lab.full_screen().await;
    let input = lab.desktop.input();
    // No window manager: keyboard focus follows the pointer into the xterm at the top left.
    input.send(DesktopInput::Pointer { x: 20, y: 20, buttons: 0 }).unwrap();
    // Slow, changing output: each line scrolls with XCopyArea, which the server sends as CopyRect.
    type_text(&input, "for i in $(seq 40); do date +%N; sleep 0.05; done\n");
    lab.until(|e, _| matches!(e, DesktopEvent::Copy { .. })).await;
}

#[tokio::test]
#[ignore = "lab"]
async fn vnc_view_only_sends_no_input() {
    let mut s = vnc();
    s.vnc.view_only = true;
    let mut lab = open(&s, &rxt_vnc::DEFAULT_ENCODINGS).await;
    lab.full_screen().await;
    let input = lab.desktop.input();
    input.send(DesktopInput::Pointer { x: 20, y: 20, buttons: 1 }).unwrap();
    input.send(DesktopInput::Wheel { x: 20, y: 20, dx: 0, dy: 1 }).unwrap();
    input.send(DesktopInput::Clipboard("view-only".into())).unwrap();
    type_text(&input, "echo view-only\n");
    lab.drain(Duration::from_millis(500)).await;
    let msgs = lab.messages();
    assert!(msgs.iter().any(|m| m.1 == 3), "parser saw the update requests");
    assert!(msgs.iter().all(|m| !matches!(m.1, 4..=6)), "view-only sent input: {msgs:?}");
}

#[tokio::test]
#[ignore = "lab"]
async fn vnc_through_ssh_direct_tcpip_without_local_listener() {
    let mut ssh = Session::new(Protocol::Ssh, "127.0.0.1", Some("alice".into()), Some(2223));
    ssh.ssh.use_agent = false;
    let dir = std::env::temp_dir().join(format!("rxt-lab-vnc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let client = rxt_ssh::connect(
        &ssh,
        rxt_ssh::ConnectContext {
            prompter: script(Some("lab-alice-pass")),
            trust: Arc::default(),
            known_hosts: dir.join("known_hosts"),
            cancel: tokio_util::sync::CancellationToken::new(),
        },
    )
    .await
    .unwrap();
    // `vnc:5900` is only reachable from inside the lab network; no local port is opened.
    let channel = client.direct_tcpip("vnc", 5900).await.unwrap();
    let mut desktop = rxt_vnc::connect(&vnc(), ctx(script(Some(LAB_VNC_PASS))), Some(Box::new(channel))).await.unwrap();
    assert!(matches!(next(&mut desktop, 30).await, Some(Ok(DesktopEvent::Resized { width: 1024, height: 768 }))));
    let mut event = next(&mut desktop, 30).await;
    while matches!(event, Some(Ok(DesktopEvent::Cursor(_)))) {
        event = next(&mut desktop, 30).await;
    }
    assert!(matches!(event, Some(Ok(DesktopEvent::Update { .. }))), "{event:?}");
}

#[tokio::test]
#[ignore = "lab"]
async fn vnc_hidden_tab_sends_no_requests_and_redraws_within_500ms() {
    let mut lab = open(&vnc(), &rxt_vnc::DEFAULT_ENCODINGS).await;
    lab.full_screen().await;
    let input = lab.desktop.input();
    input.send(DesktopInput::Visible(false)).unwrap();
    lab.drain(Duration::from_millis(300)).await;
    let mark = lab.sent.lock().unwrap().len();
    lab.drain(Duration::from_secs(5)).await;
    let shown = Instant::now();
    input.send(DesktopInput::Visible(true)).unwrap();
    lab.until(|e, _| matches!(e, DesktopEvent::Update { .. })).await;
    let redraw = shown.elapsed();
    assert!(redraw <= Duration::from_millis(500), "redraw took {redraw:?}");
    let first = lab.messages().into_iter().find(|m| m.1 == 3 && m.0 >= mark);
    assert_eq!(first.map(|m| m.2), Some(0), "no requests while hidden; full redraw when shown");
}
