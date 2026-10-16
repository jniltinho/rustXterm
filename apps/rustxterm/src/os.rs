//! Platform adapter per target (add-windows-port design W5): the Linux adapter on Linux, the
//! Windows adapter on Windows. Everything else in the binary is target-independent.

use std::sync::Arc;

use rxt_platform::{ControlTransport, Keyring};

#[cfg(target_os = "linux")]
pub use rxt_platform_linux::{XdgPaths as SystemPaths, platform};
#[cfg(target_os = "macos")]
pub use rxt_platform_macos::{MacPaths as SystemPaths, platform};
#[cfg(windows)]
pub use rxt_platform_windows::{KnownFolders as SystemPaths, platform};

/// `rustxterm.exe` on Windows: GUI subsystem, no console (CLI runs from `rustxterm-cli.exe`).
pub fn gui_only() -> bool {
    cfg!(windows) && env!("CARGO_BIN_NAME") == "rustxterm"
}

/// Keyring for CLI/MCP runs (no window).
pub fn keyring() -> Arc<dyn Keyring> {
    #[cfg(target_os = "linux")]
    return Arc::new(rxt_platform_linux::SecretService);
    #[cfg(windows)]
    return Arc::new(rxt_platform_windows::CredentialManager);
    #[cfg(target_os = "macos")]
    return Arc::new(rxt_platform_macos::Keychain);
}

/// Control transport for CLI/MCP runs.
pub fn control() -> Arc<dyn ControlTransport> {
    #[cfg(target_os = "linux")]
    return Arc::new(rxt_platform_linux::UnixControl::new(rxt_platform::Paths::runtime_dir(&SystemPaths)));
    #[cfg(windows)]
    return Arc::new(rxt_platform_windows::PipeControl::new());
    #[cfg(target_os = "macos")]
    return Arc::new(rxt_platform_macos::UnixControl::new(rxt_platform::Paths::runtime_dir(&SystemPaths)));
}
