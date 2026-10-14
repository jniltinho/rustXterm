//! FTPS certificate verification (design D5): system roots first, then the app trust record
//! (SHA-256 fingerprint of the leaf certificate per `host:port`), then a user prompt.
//!
//! rustls verifiers are synchronous, so the verifier never prompts: it records why it refused
//! and [`crate::connect`] asks the user, then reconnects with the accepted fingerprint.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rxt_core::{CertTrust, Error, HostKeyDecision, PromptAnswer, PromptKind, Prompter, Result};
use sha2::{Digest, Sha256};
use suppaftp::tokio::AsyncRustlsConnector;
use suppaftp::tokio_rustls::TlsConnector;
use suppaftp::tokio_rustls::rustls::client::WebPkiServerVerifier;
use suppaftp::tokio_rustls::rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use suppaftp::tokio_rustls::rustls::crypto::{CryptoProvider, ring, verify_tls12_signature, verify_tls13_signature};
use suppaftp::tokio_rustls::rustls::pki_types::pem::PemObject;
use suppaftp::tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use suppaftp::tokio_rustls::rustls::{
    self, CertificateError, ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme,
};

/// Usual CA bundle locations on Linux distributions; `SSL_CERT_FILE` takes precedence.
// ponytail: Linux paths only; move behind the platform contract when macOS/Windows land.
const CA_BUNDLES: [&str; 4] = [
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/ca-bundle.pem",
    "/etc/ssl/cert.pem",
];

/// SHA-256 of a DER certificate as colon-separated upper-case hex.
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// Last commonName in the certificate (the subject's; the issuer's comes first).
// ponytail: byte scan for OID 2.5.4.3 with short-form lengths, not a full X.509 parser.
fn common_name(der: &[u8]) -> Option<String> {
    const CN: [u8; 5] = [0x06, 0x03, 0x55, 0x04, 0x03];
    let at = der.windows(CN.len()).rposition(|w| w == CN)? + CN.len();
    let len = usize::from(*der.get(at + 1)?);
    der.get(at + 2..at + 2 + len).map(|v| String::from_utf8_lossy(v).into_owned())
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(ring::default_provider())
}

/// WebPKI verifier over the given roots; `None` when there are none.
pub fn roots_verifier(roots: RootCertStore) -> Option<Arc<WebPkiServerVerifier>> {
    WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider()).build().ok()
}

/// Verifier over the system CA bundle, if one is readable.
pub fn system_roots() -> Option<Arc<WebPkiServerVerifier>> {
    let path = std::env::var_os("SSL_CERT_FILE")
        .map(PathBuf::from)
        .into_iter()
        .chain(CA_BUNDLES.iter().map(PathBuf::from))
        .find(|p| p.is_file())?;
    let mut roots = RootCertStore::empty();
    for cert in CertificateDer::pem_file_iter(&path).ok()?.flatten() {
        let _ = roots.add(cert);
    }
    roots_verifier(roots)
}

/// Why a certificate that is not system-trusted was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Unknown { fingerprint: String, subject: String },
    Changed { old: String, new: String },
}

#[derive(Debug)]
pub struct Verifier {
    roots: Option<Arc<WebPkiServerVerifier>>,
    /// Fingerprint saved in the trust records for this `host:port`.
    pinned: Option<String>,
    /// Fingerprint the user accepted for this connection.
    accepted: Option<String>,
    provider: Arc<CryptoProvider>,
    refusal: Mutex<Option<Refusal>>,
}

impl Verifier {
    pub fn new(
        roots: Option<Arc<WebPkiServerVerifier>>,
        pinned: Option<String>,
        accepted: Option<String>,
    ) -> Arc<Self> {
        Arc::new(Self { roots, pinned, accepted, provider: provider(), refusal: Mutex::new(None) })
    }

    pub fn refusal(&self) -> Option<Refusal> {
        self.refusal.lock().ok().and_then(|r| r.clone())
    }

    /// suppaftp connector using this verifier (control and data connections).
    pub fn connector(self: &Arc<Self>) -> Result<AsyncRustlsConnector> {
        let config = ClientConfig::builder_with_provider(self.provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| Error::Connection(format!("TLS setup: {e}")))?
            .dangerous()
            .with_custom_certificate_verifier(self.clone())
            .with_no_client_auth();
        Ok(AsyncRustlsConnector::from(TlsConnector::from(Arc::new(config))))
    }
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        if let Some(roots) = &self.roots
            && roots.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now).is_ok()
        {
            return Ok(ServerCertVerified::assertion());
        }
        let new = fingerprint(end_entity);
        if [&self.pinned, &self.accepted].iter().any(|f| f.as_deref() == Some(new.as_str())) {
            return Ok(ServerCertVerified::assertion());
        }
        let refusal = match &self.pinned {
            Some(old) => Refusal::Changed { old: old.clone(), new },
            None => Refusal::Unknown {
                fingerprint: new,
                subject: common_name(end_entity).unwrap_or_else(|| "(no common name)".into()),
            },
        };
        if let Ok(mut r) = self.refusal.lock() {
            r.get_or_insert(refusal);
        }
        Err(rustls::Error::InvalidCertificate(CertificateError::UnknownIssuer))
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// Turn a refusal into a user decision: returns the fingerprint accepted for a retry.
pub async fn resolve(
    refusal: Refusal,
    prompter: &dyn Prompter,
    trust: &dyn CertTrust,
    host: &str,
    port: u16,
) -> Result<String> {
    let label = format!("{host}:{port}");
    match refusal {
        Refusal::Changed { old, new } => {
            prompter.host_key_changed(&label, &old, &new);
            Err(Error::HostKey(format!("{label}: certificate changed (was {old}, now {new})")))
        }
        Refusal::Unknown { fingerprint, subject } => {
            let kind =
                PromptKind::CertificateUnknown { host: host.into(), port, subject, fingerprint: fingerprint.clone() };
            match prompter.prompt(kind).await {
                PromptAnswer::HostKey(HostKeyDecision::AcceptOnce) => Ok(fingerprint),
                PromptAnswer::HostKey(HostKeyDecision::AcceptAndSave) => {
                    trust.save(host, port, &fingerprint);
                    Ok(fingerprint)
                }
                _ => Err(Error::HostKey(format!("{label}: certificate not accepted"))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use rxt_core::BoxFut;

    use super::*;

    const CA: &[u8] = include_bytes!("../tests/fixtures/ca.der");
    const LEAF: &[u8] = include_bytes!("../tests/fixtures/leaf.der");
    const SELF: &[u8] = include_bytes!("../tests/fixtures/self.der");

    fn ca_roots() -> Option<Arc<WebPkiServerVerifier>> {
        let mut roots = RootCertStore::empty();
        roots.add(CertificateDer::from(CA)).unwrap();
        roots_verifier(roots)
    }

    fn verify(v: &Verifier, cert: &[u8], name: &str) -> bool {
        let name = ServerName::try_from(name.to_owned()).unwrap();
        v.verify_server_cert(&CertificateDer::from(cert), &[], &name, &[], UnixTime::now()).is_ok()
    }

    #[derive(Default)]
    struct Fake {
        answer: Option<HostKeyDecision>,
        prompts: Mutex<Vec<PromptKind>>,
        changed: Mutex<Vec<(String, String, String)>>,
        saved: Mutex<HashMap<String, String>>,
    }

    impl Prompter for Fake {
        fn prompt(&self, kind: PromptKind) -> BoxFut<'_, PromptAnswer> {
            self.prompts.lock().unwrap().push(kind);
            let a = self.answer.map_or(PromptAnswer::Cancel, PromptAnswer::HostKey);
            Box::pin(async move { a })
        }
        fn host_key_changed(&self, host: &str, old: &str, new: &str) {
            self.changed.lock().unwrap().push((host.into(), old.into(), new.into()));
        }
    }

    impl CertTrust for Fake {
        fn get(&self, host: &str, port: u16) -> Option<String> {
            self.saved.lock().unwrap().get(&format!("{host}:{port}")).cloned()
        }
        fn save(&self, host: &str, port: u16, fp: &str) {
            self.saved.lock().unwrap().insert(format!("{host}:{port}"), fp.into());
        }
    }

    #[test]
    fn system_trusted_needs_no_prompt() {
        let v = Verifier::new(ca_roots(), None, None);
        assert!(verify(&v, LEAF, "ftp.example.test"));
        assert_eq!(v.refusal(), None);
        // Wrong name: not system-trusted, falls through to the prompt.
        assert!(!verify(&v, LEAF, "other.example.test"));
        assert!(matches!(v.refusal(), Some(Refusal::Unknown { .. })));
    }

    #[test]
    fn unknown_is_refused_with_fingerprint_and_subject() {
        let v = Verifier::new(ca_roots(), None, None);
        assert!(!verify(&v, SELF, "self.example.test"));
        assert_eq!(
            v.refusal(),
            Some(Refusal::Unknown { fingerprint: fingerprint(SELF), subject: "self.example.test".into() })
        );
        // The retry with the accepted fingerprint passes.
        assert!(verify(&Verifier::new(None, None, Some(fingerprint(SELF))), SELF, "127.0.0.1"));
    }

    #[test]
    fn saved_needs_no_prompt_and_changed_is_refused() {
        let saved = Verifier::new(None, Some(fingerprint(SELF)), None);
        assert!(verify(&saved, SELF, "127.0.0.1"));
        assert_eq!(saved.refusal(), None);

        let changed = Verifier::new(None, Some(fingerprint(LEAF)), None);
        assert!(!verify(&changed, SELF, "127.0.0.1"));
        assert_eq!(changed.refusal(), Some(Refusal::Changed { old: fingerprint(LEAF), new: fingerprint(SELF) }));
    }

    #[tokio::test]
    async fn resolve_prompts_saves_or_refuses() {
        let unknown = Refusal::Unknown { fingerprint: "AA".into(), subject: "s".into() };

        let save = Fake { answer: Some(HostKeyDecision::AcceptAndSave), ..Fake::default() };
        assert_eq!(resolve(unknown.clone(), &save, &save, "h", 21).await, Ok("AA".into()));
        assert_eq!(save.get("h", 21).as_deref(), Some("AA"));
        assert!(
            matches!(&save.prompts.lock().unwrap()[..], [PromptKind::CertificateUnknown { fingerprint, .. }] if fingerprint == "AA")
        );

        let once = Fake { answer: Some(HostKeyDecision::AcceptOnce), ..Fake::default() };
        assert_eq!(resolve(unknown.clone(), &once, &once, "h", 21).await, Ok("AA".into()));
        assert_eq!(once.get("h", 21), None);

        let cancel = Fake { answer: Some(HostKeyDecision::Cancel), ..Fake::default() };
        assert!(matches!(resolve(unknown, &cancel, &cancel, "h", 21).await, Err(Error::HostKey(_))));

        let changed = Fake::default();
        let r = resolve(Refusal::Changed { old: "A".into(), new: "B".into() }, &changed, &changed, "h", 21).await;
        assert!(matches!(r, Err(Error::HostKey(_))));
        assert!(changed.prompts.lock().unwrap().is_empty());
        assert_eq!(changed.changed.lock().unwrap()[..], [("h:21".into(), "A".into(), "B".into())]);
    }

    #[test]
    fn fingerprint_and_common_name() {
        assert_eq!(fingerprint(b"").len(), 32 * 3 - 1);
        assert!(fingerprint(b"").starts_with("E3:B0:C4:42"));
        assert_eq!(common_name(LEAF).as_deref(), Some("ftp.example.test"));
        assert_eq!(common_name(b"junk"), None);
    }
}
