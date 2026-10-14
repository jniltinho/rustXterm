//! Agent control front ends (design D14): control-channel client, CLI subcommands and the
//! `rustxterm mcp` stdio server. Authorization lives in the engine/control server; this crate
//! only builds requests, opens the store for offline reads and owner changes, and formats output.
//!
//! Exit codes (stable, see [`exit_code`]):
//!
//! | Code | Meaning |
//! |---|---|
//! | 0 | ok |
//! | 1 | runtime failure |
//! | 2 | usage error |
//! | 3 | database locked (no keyring unlock and no terminal to type the master password) |
//! | 4 | denied (matrix, policy, approval, or offline change without an interactive terminal) |
//! | 5 | not running (no control socket) |
//! | 6 | agent control is off |

use std::path::PathBuf;
use std::sync::Arc;

use rxt_core::SessionId;
use rxt_core::control::{ErrorCode, SessionRef};
use rxt_platform::{ControlTransport, Keyring};

pub use rxt_engine::ControlError;

pub mod cli;
pub mod client;
pub mod mcp;
pub mod offline;

/// Client name the CLI declares on the control channel.
pub const CLI_CLIENT: &str = "rustxterm-cli";

/// Store location, keyring and control transport; built by the app from the platform.
#[derive(Clone)]
pub struct Ctx {
    pub data_dir: PathBuf,
    pub keyring: Arc<dyn Keyring>,
    pub control: Arc<dyn ControlTransport>,
}

/// Process exit code of an error class.
pub fn exit_code(code: ErrorCode) -> u8 {
    match code {
        ErrorCode::Failed => 1,
        ErrorCode::Usage => 2,
        ErrorCode::Locked => 3,
        ErrorCode::Denied => 4,
        ErrorCode::NotRunning => 5,
        ErrorCode::AgentControlOff => 6,
    }
}

/// `42` selects by id, anything else by exact name (an id that does not exist falls back to name).
pub fn parse_ref(s: &str) -> SessionRef {
    match s.parse::<u64>() {
        Ok(n) => SessionRef::Id(SessionId(n)),
        Err(_) => SessionRef::Name(s.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_are_distinct() {
        use ErrorCode::*;
        let codes: std::collections::BTreeSet<u8> =
            [Usage, Locked, Denied, NotRunning, AgentControlOff, Failed].into_iter().map(exit_code).collect();
        assert_eq!(codes.len(), 6);
        assert!(!codes.contains(&0));
    }

    #[test]
    fn refs() {
        assert_eq!(parse_ref("7"), SessionRef::Id(SessionId(7)));
        assert_eq!(parse_ref("prod"), SessionRef::Name("prod".into()));
    }
}
