# Proposal

## Why

When a connection fails on a user's machine (an RDP `STATUS_LOGON_FAILURE`, an SSH key exchange, a
VNC that stays black), the only evidence today is the one-line error in the tab. `--debug-log`
(0.6.2) writes a debug log, but it needs a command line, which a Windows shortcut or a macOS app
started from Finder does not have, and it is all or nothing. Users need to switch diagnostics on and
off from the app, pick how much is recorded, and, for protocol problems, capture the data stream of a
session.

## What Changes

- Settings gains a **Diagnostics** page: a log level for a diagnostic log file (Off, Errors, Normal,
  Full debug, Protocol trace with a choice of protocols), the folder (where rustXterm runs by default,
  the config folder when that is not writable, or a chosen folder) and buttons to open the folder and
  to clear the logs. Changes apply at once, without a restart.
- A separate **Record data stream** switch saves the traffic of remote sessions (SSH, Telnet,
  rlogin/rsh, Serial) to one file per session (hex and text, direction, timestamps). Turning it on asks
  for consent each time ("the transcript can contain passwords echoed by the server"), it is off at
  every start and shows a red indicator in the status bar while on. Typed input is recorded only with
  **Include typed input**; credentials the app sends itself (saved passwords, Telnet auto-login, SSH
  authentication) and Serial's synthetic local echo are never recorded. RDP and VNC record message
  types and lengths only. Local terminal tabs are excluded (they never write log files).
- Files rotate at 50 MB (3 kept). Passwords, keys and NTLM/CredSSP messages are never written to the
  diagnostic log (the `--debug-log` exclusions apply to every level).
- `--debug-log` and `RXT_DEBUG_LOG` keep working and win over the Settings level for that run.

## Capabilities

### New Capabilities

### Modified Capabilities
- `desktop-shell`: a Diagnostics settings page with log levels, protocol trace, data-stream capture,
  folder choice, rotation and live switching.

## Impact

- `apps/rustxterm/src/main.rs`: the file layer becomes reloadable (`tracing_subscriber::reload`).
- `crates/store/src/settings.rs`: `diagnostics` settings (level, protocols, folder); the stream switch
  is not persisted.
- `crates/engine` and the protocol crates: a stream tap after authentication on each remote transport
  (terminal byte streams; RDP/VNC message headers), feeding a bounded background writer.
- `crates/ui/src/settings.rs`: the Diagnostics page; status-bar indicator; `cargo xtask prints` in
  light and dark.
- No new dependencies (`tracing-subscriber` already has `reload`).
