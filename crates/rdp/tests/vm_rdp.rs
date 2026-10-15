//! Manual receipt against the Windows Server 2022 Vagrant VM under `lab/vagrant/` (RDP forwarded to
//! 127.0.0.1:53389, Vagrant box account `vagrant`, fictional box credentials): NLA/CredSSP, input and
//! display-control resize. Not in CI or `lab/expected-tests.txt`; receipt in `docs/gates/m2.md`.
//! Run: `cargo test -p rxt-rdp --features windows-vm --test vm_rdp -- --ignored --nocapture`.
#![cfg(feature = "windows-vm")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{Script, Trust, ctx, next, next_update, session};
use rxt_core::{DesktopEvent, DesktopInput, HostKeyDecision, PromptKind};

const VM: u16 = 53389;
const T: Duration = Duration::from_secs(60);

#[tokio::test]
#[ignore = "windows-vm"]
async fn rdp_windows_nla_login_input_and_resize() {
    let script = Script::new(HostKeyDecision::AcceptOnce, Some("vagrant"));
    let started = Instant::now();
    let mut d = rxt_rdp::connect(&session("vagrant", VM), ctx(script.clone(), Arc::new(Trust::default())), (1024, 768))
        .await
        .unwrap();
    println!("VM connected in {:?}; prompts: {:?}", started.elapsed(), script.prompts());
    let p = script.prompts();
    assert!(matches!(p[0], PromptKind::CertificateUnknown { port: VM, .. }), "{p:?}");
    assert!(matches!(p[1], PromptKind::Password { .. }), "{p:?}");
    assert_eq!(next(&mut d, T).await, DesktopEvent::Resized { width: 1024, height: 768 });
    let DesktopEvent::Update { rect, .. } = next_update(&mut d, T).await else { unreachable!() };
    println!("VM first update {rect:?} after {:?}", started.elapsed());

    let input = d.input();
    input.send(DesktopInput::Pointer { x: 500, y: 400, buttons: 0 }).unwrap();
    // Super_L opens the Start menu (a visible screen change); Escape closes it.
    for keysym in [0xffeb_u32, 0xff1b] {
        input.send(DesktopInput::Key { keysym, down: true }).unwrap();
        input.send(DesktopInput::Key { keysym, down: false }).unwrap();
        next_update(&mut d, T).await;
    }

    let asked = Instant::now();
    input.send(DesktopInput::Resize { width: 1280, height: 720 }).unwrap();
    let deadline = Instant::now() + T;
    loop {
        assert!(Instant::now() < deadline, "no Resized after display-control request");
        if let DesktopEvent::Resized { width, height } = next(&mut d, T).await {
            println!("VM resized to {width}x{height} after {:?}", asked.elapsed());
            assert_eq!((width, height), (1280, 720));
            break;
        }
    }
    let DesktopEvent::Update { rect, .. } = next_update(&mut d, T).await else { unreachable!() };
    println!("VM update after resize {rect:?}");
}

/// NLA proof: a wrong password fails inside CredSSP, before any desktop exists (TLS-only security
/// would instead reach the Windows logon screen).
#[tokio::test]
#[ignore = "windows-vm"]
async fn rdp_windows_nla_wrong_password_fails_in_credssp() {
    let script = Script::new(HostKeyDecision::AcceptOnce, Some("not-the-password"));
    let r = rxt_rdp::connect(&session("vagrant", VM), ctx(script, Arc::new(Trust::default())), (1024, 768)).await;
    let Err(e) = r else { panic!("connected with a wrong password") };
    println!("VM wrong password: {e}");
    assert!(matches!(&e, rxt_core::Error::Auth(m) if m.contains("CredSSP")), "{e:?}");
}

/// Standard RDP Security: run with the VM's Security Layer set to RDP
/// (`SecurityLayer=0`, `UserAuthentication=0`; recipe in `lab/vagrant/README.md`).
/// Opted in: the legacy key prompt comes first, then login, input and resize work.
#[tokio::test]
#[ignore = "windows-vm-standard-security"]
async fn rdp_windows_standard_security_login_input_and_resize() {
    let script = Script::new(HostKeyDecision::AcceptOnce, Some("vagrant"));
    let mut s = session("vagrant", VM);
    s.rdp.allow_standard_security = true;
    let started = Instant::now();
    let mut d = rxt_rdp::connect(&s, ctx(script.clone(), Arc::new(Trust::default())), (1024, 768)).await.unwrap();
    println!("VM (Standard RDP Security) connected in {:?}; prompts: {:?}", started.elapsed(), script.prompts());
    let p = script.prompts();
    assert!(
        matches!(&p[0], PromptKind::CertificateUnknown { subject, fingerprint, .. }
            if subject.starts_with("Legacy RDP security") && fingerprint.starts_with("RSA ")),
        "{p:?}"
    );
    assert!(matches!(p[1], PromptKind::Password { .. }), "{p:?}");
    assert_eq!(next(&mut d, T).await, DesktopEvent::Resized { width: 1024, height: 768 });
    next_update(&mut d, T).await;
    let input = d.input();
    input.send(DesktopInput::Pointer { x: 500, y: 400, buttons: 0 }).unwrap();
    // Enough key traffic to cross the 4096-packet RC4 key update in the client-to-server stream.
    for _ in 0..2100 {
        // Shift down/up: a harmless key that changes nothing on screen.
        for down in [true, false] {
            input.send(DesktopInput::Key { keysym: 0xffe1, down }).unwrap();
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    for keysym in [0xffeb_u32, 0xff1b] {
        input.send(DesktopInput::Key { keysym, down: true }).unwrap();
        input.send(DesktopInput::Key { keysym, down: false }).unwrap();
        next_update(&mut d, T).await;
    }
    input.send(DesktopInput::Resize { width: 1280, height: 720 }).unwrap();
    let deadline = Instant::now() + T;
    loop {
        assert!(Instant::now() < deadline, "no Resized after display-control request");
        if let DesktopEvent::Resized { width, height } = next(&mut d, T).await {
            assert_eq!((width, height), (1280, 720));
            break;
        }
    }
    next_update(&mut d, T).await;
    println!("VM (Standard RDP Security) input and resize ok after {:?}", started.elapsed());
}

/// Option off: the actionable error, before any prompt.
#[tokio::test]
#[ignore = "windows-vm-standard-security"]
async fn rdp_windows_standard_security_off_is_actionable() {
    let script = Script::new(HostKeyDecision::AcceptOnce, Some("vagrant"));
    let r =
        rxt_rdp::connect(&session("vagrant", VM), ctx(script.clone(), Arc::new(Trust::default())), (1024, 768)).await;
    let Err(e) = r else { panic!("connected without the option") };
    println!("VM option off: {e}");
    assert!(e.to_string().contains("Allow legacy RDP security"), "{e}");
    assert!(script.prompts().is_empty());
}
