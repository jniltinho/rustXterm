//! Server certificate pinning (design D13). The TLS verifier only captures the leaf and checks
//! handshake signatures; [`trust_then_password`] then decides trust against [`CertTrust`] and is
//! the only place a password is obtained, so no credential exists before the certificate is trusted.

use std::sync::{Arc, Mutex, PoisonError};

use rxt_core::{CertTrust, Error, HostKeyDecision, PromptAnswer, PromptKind, Prompter, Result};
use sha2::{Digest as _, Sha256};
use tokio_rustls::rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use tokio_rustls::rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{self, DigitallySignedStruct, SignatureScheme};
use x509_cert::der::Decode as _;

/// Upper-case hex SHA-256 of the DER certificate, `AA:BB:...`.
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// Captures the leaf certificate; signatures are verified with the provider's algorithms.
#[derive(Debug)]
pub(crate) struct Capture {
    leaf: Mutex<Option<Vec<u8>>>,
    provider: Arc<CryptoProvider>,
}

impl Capture {
    pub(crate) fn new(provider: Arc<CryptoProvider>) -> Self {
        Self { leaf: Mutex::new(None), provider }
    }

    pub(crate) fn take(&self) -> Option<Vec<u8>> {
        self.leaf.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

impl ServerCertVerifier for Capture {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        *self.leaf.lock().unwrap_or_else(PoisonError::into_inner) = Some(end_entity.to_vec());
        Ok(ServerCertVerified::assertion())
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

/// Subject distinguished name, or `?` when the certificate does not parse.
fn subject(der: &[u8]) -> String {
    x509_cert::Certificate::from_der(der)
        .map(|c| c.tbs_certificate().subject().to_string())
        .unwrap_or_else(|_| "?".into())
}

/// Subject public key bits (what CredSSP binds to).
pub(crate) fn public_key(der: &[u8]) -> Result<Vec<u8>> {
    let cert =
        x509_cert::Certificate::from_der(der).map_err(|e| Error::Protocol(format!("server certificate: {e}")))?;
    cert.tbs_certificate()
        .subject_public_key_info()
        .subject_public_key
        .as_bytes()
        .map(<[u8]>::to_vec)
        .ok_or_else(|| Error::Protocol("server public key is not byte aligned".into()))
}

/// Trust decision for `leaf`, then (only if trusted) the password prompt.
/// Unknown → `CertificateUnknown` prompt; changed → `host_key_changed` and refusal.
pub(crate) async fn trust_then_password(
    prompter: &dyn Prompter,
    trust: &dyn CertTrust,
    host: &str,
    port: u16,
    user: &str,
    leaf: &[u8],
) -> Result<String> {
    let fp = fingerprint(leaf);
    match trust.get(host, port) {
        Some(saved) if saved == fp => {}
        Some(saved) => {
            prompter.host_key_changed(&format!("{host}:{port}"), &saved, &fp);
            return Err(Error::HostKey(format!("certificate of {host}:{port} changed")));
        }
        None => {
            let kind = PromptKind::CertificateUnknown {
                host: host.into(),
                port,
                subject: subject(leaf),
                fingerprint: fp.clone(),
            };
            match prompter.prompt(kind).await {
                PromptAnswer::HostKey(HostKeyDecision::AcceptOnce) => {}
                PromptAnswer::HostKey(HostKeyDecision::AcceptAndSave) => trust.save(host, port, &fp),
                _ => return Err(Error::Cancelled),
            }
        }
    }
    match prompter.prompt(PromptKind::Password { user: user.into(), host: host.into(), retry: false }).await {
        PromptAnswer::Secret(s) => Ok(s.0),
        _ => Err(Error::Cancelled),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rxt_core::{BoxFut, Secret};

    use super::*;

    struct Script {
        cert: PromptAnswer,
        log: Mutex<Vec<String>>,
    }

    impl Prompter for Script {
        fn prompt(&self, kind: PromptKind) -> BoxFut<'_, PromptAnswer> {
            let answer = match &kind {
                PromptKind::CertificateUnknown { .. } => self.cert.clone(),
                _ => PromptAnswer::Secret(Secret("pw".into())),
            };
            let tag = match kind {
                PromptKind::CertificateUnknown { .. } => "cert",
                PromptKind::Password { .. } => "password",
                _ => "other",
            };
            self.log.lock().unwrap().push(tag.into());
            Box::pin(async move { answer })
        }

        fn host_key_changed(&self, _: &str, _: &str, _: &str) {
            self.log.lock().unwrap().push("changed".into());
        }
    }

    #[derive(Default)]
    struct Trust(Mutex<HashMap<(String, u16), String>>);

    impl CertTrust for Trust {
        fn get(&self, host: &str, port: u16) -> Option<String> {
            self.0.lock().unwrap().get(&(host.to_owned(), port)).cloned()
        }
        fn save(&self, host: &str, port: u16, fp: &str) {
            self.0.lock().unwrap().insert((host.to_owned(), port), fp.to_owned());
        }
    }

    async fn run(answer: PromptAnswer, trust: &Trust) -> (Result<String>, Vec<String>) {
        let p = Script { cert: answer, log: Mutex::new(Vec::new()) };
        let r = trust_then_password(&p, trust, "h", 3389, "alice", b"leaf").await;
        (r, p.log.into_inner().unwrap())
    }

    #[tokio::test]
    async fn trust_decisions_gate_the_password_prompt() {
        let t = Trust::default();
        let (r, log) = run(PromptAnswer::HostKey(HostKeyDecision::AcceptOnce), &t).await;
        assert_eq!((r.unwrap(), log), ("pw".into(), vec!["cert".into(), "password".into()]));
        assert_eq!(t.get("h", 3389), None);

        let (r, log) = run(PromptAnswer::Cancel, &t).await;
        assert_eq!((r, log), (Err(Error::Cancelled), vec!["cert".to_owned()]));

        let (r, _) = run(PromptAnswer::HostKey(HostKeyDecision::AcceptAndSave), &t).await;
        assert!(r.is_ok());
        assert_eq!(t.get("h", 3389), Some(fingerprint(b"leaf")));

        // Saved and matching: no certificate prompt.
        let (r, log) = run(PromptAnswer::Cancel, &t).await;
        assert_eq!((r.unwrap(), log), ("pw".into(), vec!["password".into()]));

        // Changed: refused, user informed, no password prompt.
        t.save("h", 3389, "00:11");
        let (r, log) = run(PromptAnswer::HostKey(HostKeyDecision::AcceptOnce), &t).await;
        assert!(matches!(r, Err(Error::HostKey(_))));
        assert_eq!(log, vec!["changed".to_owned()]);
    }

    #[test]
    fn fingerprint_format() {
        let fp = fingerprint(b"");
        assert!(fp.starts_with("E3:B0:C4:42"));
        assert_eq!(fp.len(), 32 * 3 - 1);
    }
}
