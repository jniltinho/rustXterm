//! Offline store access (no running instance): unlock the store, then run the operation through
//! the engine's in-process authorization ([`rxt_engine::offline::apply`]: matrix + audit).

use rxt_core::control::ErrorCode;
use rxt_store::db::Store;
use rxt_store::envelope;
use secrecy::SecretString;

use crate::{ControlError, Ctx};

fn require_database(ctx: &Ctx) -> Result<(), ControlError> {
    if envelope::exists(&ctx.data_dir) {
        Ok(())
    } else {
        Err(ControlError::new(ErrorCode::Failed, "no database yet: create it in the rustXterm window first"))
    }
}

/// Unlock through the keyring wrap; `Ok(None)` when there is none. Reads only.
pub fn keyring_store(ctx: &Ctx) -> Result<Option<Store>, ControlError> {
    require_database(ctx)?;
    match envelope::unlock_with_keyring(&ctx.data_dir, ctx.keyring.as_ref())? {
        Some(key) => Ok(Some(Store::open(&ctx.data_dir, &key)?)),
        None => Ok(None),
    }
}

/// Unlock with the master password.
pub fn password_store(ctx: &Ctx, master: &SecretString) -> Result<Store, ControlError> {
    require_database(ctx)?;
    let key = envelope::unlock_with_password(&ctx.data_dir, master).map_err(|e| match e {
        rxt_core::Error::Auth(m) => ControlError::new(ErrorCode::Locked, m),
        other => other.into(),
    })?;
    Ok(Store::open(&ctx.data_dir, &key)?)
}

pub fn locked(hint: &str) -> ControlError {
    ControlError::new(ErrorCode::Locked, format!("the database is locked: {hint}"))
}
