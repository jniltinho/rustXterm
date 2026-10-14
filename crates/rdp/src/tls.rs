//! TLS upgrade after X.224. Default: rustls (TLS 1.2/1.3). Opt-in per session
//! (`RdpOptions::allow_legacy_tls`): OpenSSL (vendored, statically linked) down to TLS 1.0 with
//! legacy ciphers, for old servers such as Windows Server 2008. Both return the leaf certificate
//! unverified; the trust decision in `trust` follows before any credential exists.

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use openssl::ssl::{SslConnector, SslMethod, SslOptions, SslSessionCacheMode, SslVerifyMode, SslVersion};
use rxt_core::{Error, Result};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::{self, AlertDescription, pki_types};

use crate::trust::Capture;

pub(crate) trait Io: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Io for T {}

/// The upgraded stream, whichever TLS stack produced it.
pub(crate) type Tls = Box<dyn Io>;

pub(crate) const LEGACY_HINT: &str = "The server only offers legacy TLS (TLS 1.0/1.1, e.g. Windows Server 2008). \
     Enable 'Allow legacy TLS' in the session's RDP settings to connect (less secure).";

/// TLS handshake on `tcp`; returns the stream and the server's leaf certificate (DER).
pub(crate) async fn upgrade(tcp: TcpStream, host: &str, legacy: bool, stall: Duration) -> Result<(Tls, Vec<u8>)> {
    let handshake = async { if legacy { openssl_upgrade(tcp, host).await } else { rustls_upgrade(tcp, host).await } };
    timeout(stall, handshake).await.map_err(|_| Error::Connection("TLS handshake timed out".into()))?
}

async fn rustls_upgrade(tcp: TcpStream, host: &str) -> Result<(Tls, Vec<u8>)> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let capture = Arc::new(Capture::new(provider.clone()));
    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::Protocol(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(capture.clone())
        .with_no_client_auth();
    // CredSSP does not support TLS session resumption.
    tls.resumption = rustls::client::Resumption::disabled();
    let name = pki_types::ServerName::try_from(host.to_owned())
        .map_err(|_| Error::Invalid(format!("invalid host name {host}")))?;
    let stream = TlsConnector::from(Arc::new(tls)).connect(name, tcp).await.map_err(|e| {
        if legacy_only(&e) {
            tracing::debug!(error = %e, "rustls handshake failed; server likely legacy-TLS only");
            Error::Connection(LEGACY_HINT.into())
        } else {
            Error::Connection(format!("TLS: {e}"))
        }
    })?;
    let leaf = capture.take().ok_or_else(|| Error::Protocol("server sent no certificate".into()))?;
    Ok((Box::new(stream), leaf))
}

async fn openssl_upgrade(tcp: TcpStream, host: &str) -> Result<(Tls, Vec<u8>)> {
    let ssl_err = |e: openssl::error::ErrorStack| Error::Connection(format!("TLS: {e}"));
    let mut builder = SslConnector::builder(SslMethod::tls_client()).map_err(ssl_err)?;
    builder.set_min_proto_version(Some(SslVersion::TLS1)).map_err(ssl_err)?;
    // OpenSSL 3 refuses TLS 1.0/1.1 (SHA-1 handshakes) and RSA key exchange above level 0.
    builder.set_cipher_list("DEFAULT:@SECLEVEL=0").map_err(ssl_err)?;
    // Certificate trust is decided by fingerprint (CertTrust), not by PKI.
    builder.set_verify(SslVerifyMode::NONE);
    // No resumption (CredSSP does not support it).
    builder.set_session_cache_mode(SslSessionCacheMode::OFF);
    // No empty-fragment CBC countermeasure: 2008 SChannel answers the first CredSSP record
    // after an empty one with an internal_error alert (FreeRDP disables it too).
    builder.set_options(SslOptions::NO_TICKET | SslOptions::DONT_INSERT_EMPTY_FRAGMENTS);
    let ssl = builder.build().configure().map_err(ssl_err)?.verify_hostname(false).into_ssl(host).map_err(ssl_err)?;
    let mut stream = tokio_openssl::SslStream::new(ssl, tcp).map_err(ssl_err)?;
    Pin::new(&mut stream).connect().await.map_err(|e| Error::Connection(format!("TLS: {e}")))?;
    tracing::debug!(version = stream.ssl().version_str(), "legacy TLS negotiated");
    let leaf = stream
        .ssl()
        .peer_certificate()
        .ok_or_else(|| Error::Protocol("server sent no certificate".into()))?
        .to_der()
        .map_err(ssl_err)?;
    Ok((Box::new(stream), leaf))
}

/// A rustls handshake failure that typically means the server speaks only TLS 1.0/1.1:
/// SChannel on 2008 resets the connection or alerts on the TLS 1.2 ClientHello.
fn legacy_only(e: &io::Error) -> bool {
    if matches!(e.kind(), io::ErrorKind::ConnectionReset | io::ErrorKind::UnexpectedEof) {
        return true;
    }
    let tls = e.get_ref().and_then(|inner| inner.downcast_ref::<rustls::Error>());
    matches!(
        tls,
        Some(
            rustls::Error::AlertReceived(AlertDescription::ProtocolVersion | AlertDescription::HandshakeFailure)
                | rustls::Error::PeerIncompatible(_)
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_only_classification() {
        let tls = |e: rustls::Error| io::Error::new(io::ErrorKind::InvalidData, e);
        assert!(legacy_only(&io::Error::from(io::ErrorKind::ConnectionReset)));
        assert!(legacy_only(&io::Error::from(io::ErrorKind::UnexpectedEof)));
        assert!(legacy_only(&tls(rustls::Error::AlertReceived(AlertDescription::ProtocolVersion))));
        assert!(legacy_only(&tls(rustls::Error::AlertReceived(AlertDescription::HandshakeFailure))));
        assert!(!legacy_only(&tls(rustls::Error::AlertReceived(AlertDescription::BadCertificate))));
        assert!(!legacy_only(&io::Error::from(io::ErrorKind::ConnectionRefused)));
        assert!(!legacy_only(&io::Error::from(io::ErrorKind::TimedOut)));
    }
}
