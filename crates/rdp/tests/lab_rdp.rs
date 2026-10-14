//! RDP lab tests against xrdp on 127.0.0.1:13389 (task 5.2). Run with `lab/lab.sh up` first.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{ALICE_PASS, Script, Trust, ctx, next, next_update, session};
use rxt_core::{DesktopEvent, DesktopInput, Error, HostKeyDecision, PromptAnswer, PromptKind, RdpResolution};
use rxt_rdp::Desktop;

const RDP: u16 = 13389;
const T: Duration = Duration::from_secs(30);

/// xrdp hands a new login of the same user the existing X session and drops the previous
/// connection, so the lab tests run one at a time.
static ONE_SESSION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn alice(size: (u32, u32)) -> Desktop {
    let s = Script::new(HostKeyDecision::AcceptOnce, Some(ALICE_PASS));
    rxt_rdp::connect(&session("alice", RDP), ctx(s, Arc::default()), size).await.unwrap()
}

#[tokio::test]
#[ignore = "lab"]
async fn rdp_login_alice_updates_and_input() {
    let _one = ONE_SESSION.lock().await;
    let mut d = alice((1024, 768)).await;
    assert_eq!(next(&mut d, T).await, DesktopEvent::Resized { width: 1024, height: 768 });
    let DesktopEvent::Update { rect, rgba } = next_update(&mut d, T).await else { unreachable!() };
    assert_eq!(rgba.len() as u64, rect.area() * 4);

    let input = d.input();
    input.send(DesktopInput::Pointer { x: 100, y: 100, buttons: 1 }).unwrap();
    input.send(DesktopInput::Pointer { x: 100, y: 100, buttons: 0 }).unwrap();
    for keysym in [u32::from(b'e'), u32::from(b'c'), u32::from(b'h'), u32::from(b'o'), 0xff0d] {
        input.send(DesktopInput::Key { keysym, down: true }).unwrap();
        input.send(DesktopInput::Key { keysym, down: false }).unwrap();
    }
    input.send(DesktopInput::Key { keysym: 0x00e9, down: true }).unwrap(); // unicode fallback
    input.send(DesktopInput::Key { keysym: 0x00e9, down: false }).unwrap();
    input.send(DesktopInput::Wheel { x: 100, y: 100, dx: 0, dy: -1 }).unwrap();
    input.send(DesktopInput::ReleaseAll).unwrap();
    // Typing into the xterm redraws it; the session stays healthy.
    next_update(&mut d, T).await;
}

#[tokio::test]
#[ignore = "lab"]
async fn rdp_cert_unknown_prompt_then_accept_and_save() {
    let _one = ONE_SESSION.lock().await;
    let trust = Arc::new(Trust::default());
    let s = Script::new(HostKeyDecision::AcceptAndSave, Some(ALICE_PASS));
    let mut d = rxt_rdp::connect(&session("alice", RDP), ctx(s.clone(), trust.clone()), (800, 600)).await.unwrap();
    let p = s.prompts();
    assert!(matches!(p[0], PromptKind::CertificateUnknown { port: RDP, .. }), "{p:?}");
    assert!(matches!(p[1], PromptKind::Password { .. }), "{p:?}");
    let PromptKind::CertificateUnknown { fingerprint, .. } = &p[0] else { unreachable!() };
    assert_eq!(rxt_core::CertTrust::get(&*trust, "127.0.0.1", RDP).as_ref(), Some(fingerprint));
    next_update(&mut d, T).await;
    drop(d);

    // Saved: the second connection asks only for the password.
    let s = Script::new(HostKeyDecision::Cancel, Some(ALICE_PASS));
    let _d = rxt_rdp::connect(&session("alice", RDP), ctx(s.clone(), trust), (800, 600)).await.unwrap();
    assert_eq!((s.cert_prompts(), s.password_prompts()), (0, 1));
}

#[tokio::test]
#[ignore = "lab"]
async fn rdp_cert_cancelled_asks_no_password() {
    let _one = ONE_SESSION.lock().await;
    let s = Script::answer(PromptAnswer::Cancel, Some(ALICE_PASS));
    let r = rxt_rdp::connect(&session("alice", RDP), ctx(s.clone(), Arc::default()), (800, 600)).await;
    assert!(matches!(r, Err(Error::Cancelled)));
    assert_eq!((s.cert_prompts(), s.password_prompts()), (1, 0));
}

#[tokio::test]
#[ignore = "lab"]
async fn rdp_cert_changed_refused_without_password() {
    let _one = ONE_SESSION.lock().await;
    let trust = Arc::new(Trust::default());
    rxt_core::CertTrust::save(&*trust, "127.0.0.1", RDP, "00:11:22:33");
    let s = Script::new(HostKeyDecision::AcceptOnce, Some(ALICE_PASS));
    let r = rxt_rdp::connect(&session("alice", RDP), ctx(s.clone(), trust), (800, 600)).await;
    assert!(matches!(r, Err(Error::HostKey(_))), "{:?}", r.err());
    assert!(s.prompts().is_empty(), "{:?}", s.prompts());
    let changed = s.changed.lock().unwrap().clone();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].1, "00:11:22:33");
}

/// xrdp 0.9.24 accepts the DisplayControl channel and receives the monitor layout
/// (`dynamic_monitor_data: msg_type 2` in xrdp.log) but does not resize; the client's
/// deactivation-reactivation path is covered by `hostile_rdp::reactivation_resizes`.
#[tokio::test]
#[ignore = "lab"]
async fn rdp_resize_via_display_control() {
    let _one = ONE_SESSION.lock().await;
    let mut d = alice((1024, 768)).await;
    assert_eq!(next(&mut d, T).await, DesktopEvent::Resized { width: 1024, height: 768 });
    next_update(&mut d, T).await;
    while tokio::time::timeout(Duration::from_secs(2), d.next()).await.is_ok() {}
    let input = d.input();
    input.send(DesktopInput::Resize { width: 801, height: 600 }).unwrap();
    for keysym in [u32::from(b'x'), 0xff0d] {
        input.send(DesktopInput::Key { keysym, down: true }).unwrap();
        input.send(DesktopInput::Key { keysym, down: false }).unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut updates = 0;
    while let Ok(e) = tokio::time::timeout(deadline - Instant::now(), d.next()).await {
        match e.expect("session ended").unwrap_or_else(|e| panic!("session error: {e}")) {
            DesktopEvent::Resized { width, height } => assert_eq!((width, height), (800, 600)),
            DesktopEvent::Update { .. } => updates += 1,
            _ => {}
        }
    }
    assert!(updates > 0, "session stopped updating after the resize request");
}

#[tokio::test]
#[ignore = "lab"]
async fn rdp_fixed_resolution_ignores_tab_size() {
    let _one = ONE_SESSION.lock().await;
    let mut s = session("alice", RDP);
    s.rdp.resolution = RdpResolution::Fixed { width: 640, height: 480 };
    let script = Script::new(HostKeyDecision::AcceptOnce, Some(ALICE_PASS));
    let mut d = rxt_rdp::connect(&s, ctx(script, Arc::default()), (1500, 900)).await.unwrap();
    assert_eq!(next(&mut d, T).await, DesktopEvent::Resized { width: 640, height: 480 });
}

#[tokio::test]
#[ignore = "lab"]
async fn rdp_hidden_tab_suppresses_output_then_resumes() {
    let _one = ONE_SESSION.lock().await;
    let mut d = alice((1024, 768)).await;
    next_update(&mut d, T).await;
    // Let the initial paint settle.
    while tokio::time::timeout(Duration::from_secs(2), d.next()).await.is_ok() {}

    let input = d.input();
    input.send(DesktopInput::Visible(false)).unwrap();
    // Type into the xterm while hidden: the screen changes but no update reaches the tab.
    for keysym in [u32::from(b'h'), u32::from(b'i'), 0xff0d] {
        input.send(DesktopInput::Key { keysym, down: true }).unwrap();
        input.send(DesktopInput::Key { keysym, down: false }).unwrap();
    }
    let hidden = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let DesktopEvent::Update { .. } = d.next().await.unwrap().unwrap() {
                return;
            }
        }
    })
    .await;
    assert!(hidden.is_err(), "update received while hidden");

    let shown = Instant::now();
    input.send(DesktopInput::Visible(true)).unwrap();
    next_update(&mut d, Duration::from_millis(500)).await;
    assert!(shown.elapsed() <= Duration::from_millis(500));
}
