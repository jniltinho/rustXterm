//! rustXterm TOML export/import of sessions, folders and tunnels, without credentials.
//! [`Session`], [`Folder`] and [`Tunnel`] hold no secrets by construction, so nothing
//! credential-like can be emitted; unknown keys in an imported file are ignored.

use rxt_core::{Error, Folder, Result, Session, Tunnel};
use serde::{Deserialize, Serialize};

/// Value of the `format` key.
pub const FORMAT: &str = "rustxterm-export";
/// Current export version.
pub const VERSION: u32 = 1;

/// Exported profile content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Export {
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub folders: Vec<Folder>,
    #[serde(default)]
    pub sessions: Vec<Session>,
    #[serde(default)]
    pub tunnels: Vec<Tunnel>,
}

impl Export {
    pub fn new(folders: Vec<Folder>, sessions: Vec<Session>, tunnels: Vec<Tunnel>) -> Self {
        Self { format: FORMAT.into(), version: VERSION, folders, sessions, tunnels }
    }

    /// Serialize to TOML.
    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(|e| Error::Invalid(format!("export failed: {e}")))
    }

    /// Parse a TOML export, checking format and version.
    pub fn from_toml(text: &str) -> Result<Self> {
        let export: Self = toml::from_str(text).map_err(|e| Error::Invalid(format!("invalid export file: {e}")))?;
        if export.format != FORMAT {
            return Err(Error::Invalid(format!("not a rustXterm export (format = {:?})", export.format)));
        }
        if export.version != VERSION {
            return Err(Error::Unsupported(format!("export version {} (supported: {VERSION})", export.version)));
        }
        Ok(export)
    }
}
