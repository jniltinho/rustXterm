//! Persistent state (design D8, D9): basic settings in TOML/.env, the SQLCipher database
//! unlocked through the `keys.v1` envelope, saved credentials, trust records, policies, audit,
//! history, MobaXterm.ini import and TOML export.
//!
//! Module owners (parallel M3 work; keep public signatures stable, extend only):
//! - [`settings`]: `config.toml` + `--env-file` + `RXT_*` loader (task 6.1).
//! - [`envelope`]: key envelope, master password, keyring wrap (task 6.3).
//! - [`db`]: SQLCipher [`db::Store`] with migrations, backups and all tables (tasks 6.2, 6.4).
//! - [`moba_ini`]: MobaXterm.ini bookmark importer with preview report (task 6.5).
//! - [`export`]: TOML export/import without credentials (task 6.6).

pub mod db;
pub mod envelope;
pub mod export;
pub mod moba_ini;
pub mod settings;

pub use rxt_core::{Error, Result};
