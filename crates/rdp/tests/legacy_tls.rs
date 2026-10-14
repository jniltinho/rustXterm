//! Legacy TLS (opt-in `allow_legacy_tls`): an in-process RDP peer that speaks only TLS 1.0,
//! like an unpatched Windows Server 2008. Legacy mode connects with the same trust flow;
//! the default rustls mode fails with an actionable error.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use common::{Script, Trust, ctx, session};
use ironrdp::pdu::nego::SecurityProtocol;
use ironrdp::pdu::rdp::capability_sets::{Bitmap, BitmapDrawingFlags, CapabilitySet};
use ironrdp_acceptor::{Acceptor, BeginResult, DesktopSize, accept_begin, accept_finalize};
use ironrdp_tokio::TokioFramed;
use openssl::asn1::Asn1Time;
use openssl::hash::MessageDigest;
use openssl::pkey::PKey;
use openssl::rsa::Rsa;
use openssl::ssl::{SslAcceptor, SslMethod, SslVersion};
use openssl::x509::{X509, X509NameBuilder};
use rxt_core::{CertTrust as _, Error, HostKeyDecision, PromptAnswer, PromptKind};
use rxt_rdp::{Desktop, fingerprint};
use tokio::net::TcpListener;
use tokio_openssl::SslStream;

const T: Duration = Duration::from_secs(20);

/// TLS 1.0-only acceptor with a fresh self-signed RSA certificate; returns it and the cert DER.
fn tls10_acceptor() -> (SslAcceptor, Vec<u8>) {
    let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
    let mut name = X509NameBuilder::new().unwrap();
    name.append_entry_by_text("CN", "legacy-rdp").unwrap();
    let name = name.build();
    let mut cert = X509::builder().unwrap();
    cert.set_version(2).unwrap();
    cert.set_subject_name(&name).unwrap();
    cert.set_issuer_name(&name).unwrap();
    cert.set_pubkey(&key).unwrap();
    cert.set_not_before(&Asn1Time::days_from_now(0).unwrap()).unwrap();
    cert.set_not_after(&Asn1Time::days_from_now(365).unwrap()).unwrap();
    cert.sign(&key, MessageDigest::sha256()).unwrap();
    let cert = cert.build();
    let mut acc = SslAcceptor::mozilla_intermediate(SslMethod::tls_server()).unwrap();
    acc.set_min_proto_version(Some(SslVersion::TLS1)).unwrap();
    acc.set_max_proto_version(Some(SslVersion::TLS1)).unwrap();
    acc.set_cipher_list("DEFAULT:@SECLEVEL=0").unwrap();
    acc.set_private_key(&key).unwrap();
    acc.set_certificate(&cert).unwrap();
    (acc.build(), cert.to_der().unwrap())
}

/// Serves one client: X.224 (TLS selected), TLS 1.0, then the TLS logon to the active stage.
/// Resolves to whether the logon completed.
async fn serve_one(listener: TcpListener, tls: SslAcceptor) -> bool {
    let (tcp, _) = listener.accept().await.unwrap();
    let bitmap = Bitmap {
        pref_bits_per_pix: 32,
        desktop_width: 640,
        desktop_height: 480,
        desktop_resize_flag: true,
        drawing_flags: BitmapDrawingFlags::empty(),
    };
    let size = DesktopSize { width: 640, height: 480 };
    let mut acc = Acceptor::new(SecurityProtocol::SSL, size, vec![CapabilitySet::Bitmap(bitmap)], None);
    let Ok(BeginResult::ShouldUpgrade(tcp)) = accept_begin(TokioFramed::new(tcp), &mut acc).await else {
        return false;
    };
    let mut stream = SslStream::new(openssl::ssl::Ssl::new(tls.context()).unwrap(), tcp).unwrap();
    if Pin::new(&mut stream).accept().await.is_err() {
        return false;
    }
    acc.mark_security_upgrade_as_done();
    accept_finalize(TokioFramed::new(stream), &mut acc).await.is_ok()
}

async fn run(legacy: bool, script: Arc<Script>, trust: Arc<Trust>) -> (rxt_core::Result<Desktop>, bool, Vec<u8>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let (tls, der) = tls10_acceptor();
    let server = tokio::spawn(serve_one(l, tls));
    let mut s = session("alice", port);
    s.rdp.allow_legacy_tls = legacy;
    let client = tokio::time::timeout(T, rxt_rdp::connect(&s, ctx(script, trust), (800, 600))).await.unwrap();
    let logged_on = tokio::time::timeout(T, server).await.unwrap().unwrap();
    (client, logged_on, der)
}

#[tokio::test]
async fn legacy_mode_connects_to_a_tls10_only_server_with_the_trust_flow() {
    let script = Script::new(HostKeyDecision::AcceptAndSave, Some("legacy-pass"));
    let trust = Arc::new(Trust::default());
    let (client, logged_on, der) = run(true, script.clone(), trust.clone()).await;
    assert!(client.is_ok(), "{:?}", client.err());
    assert!(logged_on);
    let prompts = script.prompts();
    assert!(
        matches!(&prompts[0], PromptKind::CertificateUnknown { subject, fingerprint: fp, .. }
            if subject.contains("legacy-rdp") && *fp == fingerprint(&der)),
        "{prompts:?}"
    );
    assert!(matches!(prompts[1], PromptKind::Password { .. }));
    assert_eq!(trust.get("127.0.0.1", trust_port(&prompts)), Some(fingerprint(&der)));
}

fn trust_port(prompts: &[PromptKind]) -> u16 {
    match &prompts[0] {
        PromptKind::CertificateUnknown { port, .. } => *port,
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn default_mode_reports_the_legacy_tls_hint() {
    let script = Script::new(HostKeyDecision::AcceptOnce, Some("legacy-pass"));
    let (client, logged_on, _) = run(false, script.clone(), Arc::default()).await;
    let e = client.err().expect("rustls accepted TLS 1.0");
    assert!(e.to_string().contains("Allow legacy TLS"), "{e}");
    assert!(!logged_on);
    assert!(script.prompts().is_empty(), "no prompt before a TLS session exists");
}

#[tokio::test]
async fn legacy_mode_refuses_before_any_credential() {
    // Rejected certificate: no password prompt, so nothing reaches CredSSP or the logon.
    let script = Script::answer(PromptAnswer::Cancel, Some("legacy-pass"));
    let (client, logged_on, _) = run(true, script.clone(), Arc::default()).await;
    assert_eq!(client.err(), Some(Error::Cancelled));
    assert!(!logged_on);
    assert_eq!((script.cert_prompts(), script.password_prompts()), (1, 0));

    // Changed certificate: refused, user informed, no prompt at all.
    let trust = Arc::new(Trust::default());
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    trust.save("127.0.0.1", port, "00:11");
    let script = Script::new(HostKeyDecision::AcceptOnce, Some("legacy-pass"));
    let l = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    let (tls, _) = tls10_acceptor();
    let server = tokio::spawn(serve_one(l, tls));
    let mut s = session("alice", port);
    s.rdp.allow_legacy_tls = true;
    let e = rxt_rdp::connect(&s, ctx(script.clone(), trust), (800, 600)).await.err().expect("changed cert accepted");
    assert!(matches!(e, Error::HostKey(_)), "{e}");
    assert!(!server.await.unwrap());
    assert!(script.prompts().is_empty());
    assert_eq!(script.changed.lock().unwrap().len(), 1);
}

/// Manual check against a real legacy server, never in CI: set `RXT_LEGACY_RDP_HOST` (and
/// optionally `RXT_LEGACY_RDP_USER`). Accepts the certificate once and sends one obviously
/// wrong password so the NLA error path is exercised; prints the certificate prompt.
#[cfg(feature = "windows-vm")]
#[tokio::test]
#[ignore = "needs RXT_LEGACY_RDP_HOST"]
async fn real_legacy_server_reaches_certificate_prompt_and_credssp() {
    let host = std::env::var("RXT_LEGACY_RDP_HOST").expect("RXT_LEGACY_RDP_HOST");
    let user = std::env::var("RXT_LEGACY_RDP_USER").unwrap_or_else(|_| "administrator".into());
    let script = Script::new(HostKeyDecision::AcceptOnce, Some("obviously-wrong-password"));
    let mut s = rxt_core::Session::new(rxt_core::Protocol::Rdp, host, Some(user), None);
    s.rdp.allow_legacy_tls = true;
    let r = tokio::time::timeout(T, rxt_rdp::connect(&s, ctx(script.clone(), Arc::default()), (1024, 768))).await;
    for p in script.prompts() {
        println!("prompt: {p:?}");
    }
    println!("result: {:?}", r.map(|r| r.map(drop)));
    assert_eq!(script.cert_prompts(), 1);
}
