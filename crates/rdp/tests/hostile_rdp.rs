//! Hostile RDP peer (design D13, task 5.2): an in-process server built on `ironrdp-acceptor`
//! (or raw X.224 where the attack happens before TLS) that announces oversized desktops,
//! sends oversized PDUs and fragment floods, stalls mid-PDU, and floods valid updates.
//! Runs without the lab.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::borrow::Cow;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{Script, ctx, next, session};
use ironrdp::core::encode_vec;
use ironrdp::pdu::bitmap::{BitmapData, BitmapUpdateData, Compression};
use ironrdp::pdu::geometry::InclusiveRectangle;
use ironrdp::pdu::mcs::SendDataIndication;
use ironrdp::pdu::nego::SecurityProtocol;
use ironrdp::pdu::rdp::capability_sets::{Bitmap, BitmapDrawingFlags, CapabilitySet};
use ironrdp::pdu::rdp::headers::{ServerDeactivateAll, ShareControlHeader, ShareControlPdu};
use ironrdp::pdu::x224::X224;
use ironrdp_acceptor::{Acceptor, AcceptorResult, BeginResult, DesktopSize, accept_begin, accept_finalize};
use ironrdp_tokio::{FramedWrite as _, TokioFramed};
use rxt_core::{DesktopEvent, Error, HostKeyDecision};
use rxt_rdp::{Desktop, Limits};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio_rustls::rustls::{self, ServerConfig};
use tokio_rustls::{TlsAcceptor, server::TlsStream};

const T: Duration = Duration::from_secs(20);

fn tls_acceptor() -> TlsAcceptor {
    use picky::key::{EcCurve, PrivateKey};
    use picky::x509::certificate::CertificateBuilder;
    use picky::x509::date::UtcDate;
    use picky::x509::name::DirectoryName;
    let key = PrivateKey::generate_ec(EcCurve::NistP256).unwrap();
    let cert = CertificateBuilder::new()
        .validity(UtcDate::ymd(2020, 1, 1).unwrap(), UtcDate::ymd(2099, 1, 1).unwrap())
        .self_signed(DirectoryName::new_common_name("hostile-rdp"), &key)
        .signature_hash_type(picky::signature::SignatureAlgorithm::Ecdsa(picky::hash::HashAlgorithm::SHA2_256))
        .build()
        .unwrap();
    let config = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(cert.to_der().unwrap())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.to_pkcs8().unwrap())),
        )
        .unwrap();
    TlsAcceptor::from(Arc::new(config))
}

fn acceptor(width: u16, height: u16) -> Acceptor {
    let bitmap = Bitmap {
        pref_bits_per_pix: 32,
        desktop_width: width,
        desktop_height: height,
        desktop_resize_flag: true,
        drawing_flags: BitmapDrawingFlags::empty(),
    };
    Acceptor::new(SecurityProtocol::SSL, DesktopSize { width, height }, vec![CapabilitySet::Bitmap(bitmap)], None)
}

type Server = TokioFramed<TlsStream<TcpStream>>;

/// Full TLS logon up to the active stage with the given advertised desktop size.
async fn accept(
    listener: &TcpListener,
    width: u16,
    height: u16,
) -> ironrdp::connector::ConnectorResult<(Server, Acceptor, AcceptorResult)> {
    let (tcp, _) = listener.accept().await.unwrap();
    let mut acc = acceptor(width, height);
    let BeginResult::ShouldUpgrade(tcp) = accept_begin(TokioFramed::new(tcp), &mut acc).await? else {
        panic!("client did not ask for TLS");
    };
    let tls = tls_acceptor().accept(tcp).await.unwrap();
    acc.mark_security_upgrade_as_done();
    let (framed, result) = accept_finalize(TokioFramed::new(tls), &mut acc).await?;
    Ok((framed, acc, result))
}

async fn listen() -> (TcpListener, u16) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    (l, port)
}

async fn client(port: u16, limits: Limits) -> rxt_core::Result<Desktop> {
    let s = Script::new(HostKeyDecision::AcceptOnce, Some("hostile-pass"));
    rxt_rdp::connect_with_limits(&session("alice", port), ctx(s, Arc::default()), (800, 600), limits).await
}

fn quick() -> Limits {
    Limits { stall: Duration::from_secs(1), ..Limits::default() }
}

/// Fast-path PDU with one update (`code`, fragmentation bits `frag`) carrying `data`.
fn fastpath(code: u8, frag: u8, data: &[u8]) -> Vec<u8> {
    let mut body = vec![code | (frag << 4)];
    body.extend_from_slice(&u16::try_from(data.len()).unwrap().to_le_bytes());
    body.extend_from_slice(data);
    let total = u16::try_from(body.len() + 3).unwrap() | 0x8000;
    let mut pdu = vec![0x00];
    pdu.extend_from_slice(&total.to_be_bytes());
    pdu.extend_from_slice(&body);
    pdu
}

/// Next event that is not `Resized`/`Update`, or the session error.
async fn first_error(d: &mut Desktop) -> Error {
    loop {
        match tokio::time::timeout(T, d.next()).await.expect("no error in time") {
            Some(Err(e)) => return e,
            Some(Ok(_)) => continue,
            None => panic!("session ended without an error"),
        }
    }
}

#[tokio::test]
async fn oversized_desktop_rejected_in_capability_exchange() {
    let (l, port) = listen().await;
    let server = tokio::spawn(async move { accept(&l, 9000, 600).await.map(drop) });
    let started = Instant::now();
    let e = client(port, quick()).await.err().expect("oversized desktop accepted");
    assert!(e.to_string().contains("9000x600"), "{e}");
    assert!(started.elapsed() < T);
    // The server never reaches the active stage.
    assert!(server.await.unwrap().is_err());
}

#[tokio::test]
async fn oversized_pdu_drops_connection() {
    // X.224 confirm selecting CredSSP, then a TSRequest with an over-long length and an
    // endless body: the client drops it once `max_pdu` bytes are buffered.
    let (l, port) = listen().await;
    let sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = sent.clone();
    tokio::spawn(async move {
        let (mut tcp, _) = l.accept().await.unwrap();
        let mut buf = [0; 512];
        let _ = tcp.read(&mut buf).await.unwrap();
        let confirm = [3, 0, 0, 19, 14, 0xd0, 0, 0, 0x12, 0x34, 0, 2, 0, 8, 0, 2, 0, 0, 0];
        tcp.write_all(&confirm).await.unwrap();
        let mut tls = tls_acceptor().accept(tcp).await.unwrap();
        let _ = tls.read(&mut buf).await.unwrap();
        tls.write_all(&[0x30, 0x84, 0x7f, 0xff, 0xff, 0xff]).await.unwrap();
        let chunk = [0xa0; 16 << 10];
        while tls.write_all(&chunk).await.is_ok() {
            counter.fetch_add(chunk.len(), std::sync::atomic::Ordering::Relaxed);
        }
    });
    let limits = Limits { max_pdu: 256 << 10, ..quick() };
    let started = Instant::now();
    let e = client(port, limits).await.err().expect("oversized PDU accepted");
    assert!(e.to_string().contains("exceeds"), "{e}");
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    // Bounded: at most the cap plus socket buffers were taken from the peer.
    assert!(sent.load(std::sync::atomic::Ordering::Relaxed) < 16 << 20);
}

#[tokio::test]
async fn stalled_peer_during_connection_times_out() {
    let (l, port) = listen().await;
    tokio::spawn(async move {
        let (mut tcp, _) = l.accept().await.unwrap();
        let mut buf = [0; 512];
        let _ = tcp.read(&mut buf).await.unwrap();
        tcp.write_all(&[3, 0, 0, 19, 14]).await.unwrap(); // partial X.224 confirm, then silence
        tokio::time::sleep(T).await;
    });
    let started = Instant::now();
    let e = client(port, quick()).await.err().expect("stalled peer accepted");
    assert!(e.to_string().contains("stalled"), "{e}");
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn fragment_flood_drops_connection() {
    let (l, port) = listen().await;
    tokio::spawn(async move {
        let (mut framed, _, _) = accept(&l, 640, 480).await.unwrap();
        let chunk = vec![0; 16_000];
        framed.write_all(&fastpath(1, 2, &chunk)).await.unwrap(); // First
        while framed.write_all(&fastpath(1, 3, &chunk)).await.is_ok() {} // Next, forever
    });
    let mut d = client(port, quick()).await.unwrap();
    assert_eq!(next(&mut d, T).await, DesktopEvent::Resized { width: 640, height: 480 });
    let e = first_error(&mut d).await;
    assert!(e.to_string().contains("fragmented update exceeds"), "{e}");
}

#[tokio::test]
async fn stalled_peer_mid_pdu_errors_and_drop_closes_promptly() {
    let (l, port) = listen().await;
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut framed, _, _) = accept(&l, 640, 480).await.unwrap();
        framed.write_all(&[0x00, 0x80]).await.unwrap(); // half a fast-path header
        let (mut tls, _) = framed.into_inner();
        let mut buf = [0; 64];
        while matches!(tls.read(&mut buf).await, Ok(n) if n > 0) {}
        let _ = closed_tx.send(Instant::now());
    });
    let mut d = client(port, quick()).await.unwrap();
    let started = Instant::now();
    let e = first_error(&mut d).await;
    assert!(e.to_string().contains("stalled"), "{e}");
    assert!(started.elapsed() < Duration::from_secs(3));
    drop(d);
    let closed = tokio::time::timeout(Duration::from_secs(2), closed_rx).await.expect("socket not closed").unwrap();
    assert!(closed.elapsed() < Duration::from_secs(2));
}

/// One 64x32 uncompressed 32 bpp bitmap update.
fn bitmap_update() -> Vec<u8> {
    let data = vec![0x80; 64 * 32 * 4];
    let update = BitmapUpdateData {
        rectangles: vec![BitmapData {
            rectangle: InclusiveRectangle { left: 0, top: 0, right: 63, bottom: 31 },
            width: 64,
            height: 32,
            bits_per_pixel: 32,
            compression_flags: Compression::empty(),
            compressed_data_header: None,
            bitmap_data: &data,
        }],
    };
    fastpath(1, 0, &encode_vec(&update).unwrap())
}

#[tokio::test]
async fn unread_events_stop_socket_reads_and_drop_is_prompt() {
    let (l, port) = listen().await;
    let (written_tx, written_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (framed, _, _) = accept(&l, 640, 480).await.unwrap();
        let (mut tls, _) = framed.into_inner();
        let pdu = bitmap_update();
        let mut written = 0usize;
        // Write until the client stops reading (a write blocks for 2 s) or 64 MiB went out.
        while written < 64 << 20 {
            match tokio::time::timeout(Duration::from_secs(2), tls.write_all(&pdu)).await {
                Ok(Ok(())) => written += pdu.len(),
                _ => break,
            }
        }
        let _ = written_tx.send(written);
    });
    let limits = Limits { queue_bytes: 1 << 20, max_update_bytes: 1 << 20, ..quick() };
    let d = client(port, limits).await.unwrap();
    // Never call `next`: the 1 MiB queue fills, then the socket is no longer read.
    let written = tokio::time::timeout(Duration::from_secs(60), written_rx).await.unwrap().unwrap();
    assert!(written < 32 << 20, "client kept reading: {written} bytes accepted");
    let started = Instant::now();
    drop(d);
    assert!(started.elapsed() < Duration::from_millis(100));
}

fn deactivate_all(result: &AcceptorResult) -> Vec<u8> {
    let pdu = ShareControlHeader {
        share_id: 0,
        pdu_source: result.io_channel_id,
        share_control_pdu: ShareControlPdu::ServerDeactivateAll(ServerDeactivateAll),
    };
    let indication = SendDataIndication {
        initiator_id: result.user_channel_id,
        channel_id: result.io_channel_id,
        user_data: Cow::Owned(encode_vec(&pdu).unwrap()),
    };
    encode_vec(&X224(indication)).unwrap()
}

#[tokio::test]
async fn reactivation_resizes_and_rejects_oversized() {
    let (l, port) = listen().await;
    tokio::spawn(async move {
        let (mut framed, mut acc, mut result) = accept(&l, 640, 480).await.unwrap();
        for size in [DesktopSize { width: 800, height: 600 }, DesktopSize { width: 8200, height: 600 }] {
            framed.write_all(&deactivate_all(&result)).await.unwrap();
            let mut acc2 = Acceptor::new_deactivation_reactivation(acc, result.static_channels, size).unwrap();
            let Ok((f, r)) = accept_finalize(framed, &mut acc2).await else { return };
            (framed, acc, result) = (f, acc2, r);
        }
    });
    let mut d = client(port, quick()).await.unwrap();
    assert_eq!(next(&mut d, T).await, DesktopEvent::Resized { width: 640, height: 480 });
    assert_eq!(next(&mut d, T).await, DesktopEvent::Resized { width: 800, height: 600 });
    let e = first_error(&mut d).await;
    assert!(e.to_string().contains("8200x600"), "{e}");
}
