//! User-interaction contracts shared by every protocol crate (no protocol crate depends on another).

use crate::{BoxFut, PromptAnswer, PromptKind};

/// Asks the user (through engine events) and waits for the answer.
pub trait Prompter: Send + Sync {
    fn prompt(&self, kind: PromptKind) -> BoxFut<'_, PromptAnswer>;
    /// Changed host key or certificate: the connection is refused and the user is informed
    /// with both fingerprints.
    fn host_key_changed(&self, host: &str, old_fingerprint: &str, new_fingerprint: &str);
}

/// App-owned certificate trust records for TLS-based protocols (FTPS, RDP): SHA-256
/// fingerprints per `host:port`. In memory until the M3 database lands.
pub trait CertTrust: Send + Sync {
    fn get(&self, host: &str, port: u16) -> Option<String>;
    fn save(&self, host: &str, port: u16, fingerprint: &str);
}
