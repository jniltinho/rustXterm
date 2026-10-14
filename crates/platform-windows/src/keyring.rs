//! Keyring over Windows Credential Manager generic credentials `rustxterm:<id>` with
//! local-machine persistence (does not roam), design W3.

use rxt_platform::{Error, Keyring, Result};

use crate::{MAX_CREDENTIAL_BLOB, credential_target, sys};

/// Credential Manager keyring.
pub struct CredentialManager;

/// Win32 codes meaning "no usable credential store" (e.g. a network logon without a profile):
/// callers fall back to the master password.
const ERROR_NO_SUCH_LOGON_SESSION: i32 = 1312;
const ERROR_INVALID_FUNCTION: i32 = 1;

fn err(e: std::io::Error) -> Error {
    match e.raw_os_error() {
        Some(ERROR_NO_SUCH_LOGON_SESSION | ERROR_INVALID_FUNCTION) => {
            Error::Unsupported(format!("credential manager: {e}"))
        }
        _ => Error::Io(format!("credential manager: {e}")),
    }
}

impl Keyring for CredentialManager {
    fn get(&self, id: &str) -> Result<Option<Vec<u8>>> {
        sys::cred_read(&credential_target(id)).map_err(err)
    }

    fn set(&self, id: &str, secret: &[u8]) -> Result<()> {
        if secret.len() > MAX_CREDENTIAL_BLOB {
            return Err(Error::Invalid(format!(
                "keyring secret of {} bytes exceeds {MAX_CREDENTIAL_BLOB}",
                secret.len()
            )));
        }
        sys::cred_write(&credential_target(id), "rustxterm", secret).map_err(err)
    }

    fn delete(&self, id: &str) -> Result<()> {
        sys::cred_delete(&credential_target(id)).map_err(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_credential_store_is_unsupported() {
        let e = err(std::io::Error::from_raw_os_error(ERROR_NO_SUCH_LOGON_SESSION));
        assert!(matches!(e, Error::Unsupported(_)), "{e}");
        assert!(matches!(err(std::io::Error::from_raw_os_error(5)), Error::Io(_)));
    }

    /// Needs an interactive logon (Credential Manager is per logon session); on the Windows
    /// runner and the VM. A network logon reports `Unsupported`, which is accepted.
    #[test]
    fn round_trip() {
        let id = format!("test-{}", std::process::id());
        let k = CredentialManager;
        match k.get(&id) {
            Err(Error::Unsupported(e)) => return eprintln!("round trip skipped: {e}"),
            r => assert_eq!(r.unwrap(), None),
        }
        k.set(&id, b"fictional-wrapping-key").unwrap();
        assert_eq!(k.get(&id).unwrap().as_deref(), Some(&b"fictional-wrapping-key"[..]));
        k.set(&id, b"replaced").unwrap();
        assert_eq!(k.get(&id).unwrap().as_deref(), Some(&b"replaced"[..]));
        k.delete(&id).unwrap();
        assert_eq!(k.get(&id).unwrap(), None);
        k.delete(&id).unwrap();
        assert!(matches!(k.set(&id, &[0; MAX_CREDENTIAL_BLOB + 1]), Err(Error::Invalid(_))));
    }
}
