//! Keyring over the macOS Keychain via `security-framework`'s safe generic-password API
//! (service `rustxterm`, account `rustxterm:<id>`). No `unsafe`: the crate wraps the Security
//! framework for us, so the workspace `unsafe_code = "forbid"` stays in force.

use rxt_platform::{Error, Keyring, Result};
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};

const SERVICE: &str = "rustxterm";

/// `errSecItemNotFound`: no such Keychain item.
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
/// `errSecUserCanceled`: the user dismissed an unlock/allow prompt.
const ERR_SEC_USER_CANCELED: i32 = -128;

/// macOS Keychain keyring.
pub struct Keychain;

fn account(id: &str) -> String {
    format!("{SERVICE}:{id}")
}

fn map_err(e: security_framework::base::Error) -> Error {
    match e.code() {
        ERR_SEC_USER_CANCELED => Error::Cancelled,
        code => Error::Io(format!("keychain: {e} (OSStatus {code})")),
    }
}

impl Keyring for Keychain {
    fn get(&self, id: &str) -> Result<Option<Vec<u8>>> {
        match get_generic_password(SERVICE, &account(id)) {
            Ok(secret) => Ok(Some(secret)),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(map_err(e)),
        }
    }

    fn set(&self, id: &str, secret: &[u8]) -> Result<()> {
        // `set_generic_password` creates or replaces the item.
        set_generic_password(SERVICE, &account(id), secret).map_err(map_err)
    }

    fn delete(&self, id: &str) -> Result<()> {
        // Deleting a secret that is already gone is success.
        match delete_generic_password(SERVICE, &account(id)) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(map_err(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_is_namespaced() {
        assert_eq!(account("db-main"), "rustxterm:db-main");
    }

    #[test]
    fn not_found_code_is_mapped_to_none() {
        // The mapping is what turns a missing item into `Ok(None)` in `get`.
        assert_eq!(ERR_SEC_ITEM_NOT_FOUND, -25300);
    }
}
