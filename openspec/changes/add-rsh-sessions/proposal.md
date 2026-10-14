## Why

Some legacy Unix systems, appliances and isolated lab networks are only reachable with the Berkeley r-commands (rlogin, rsh, rexec). rustXterm v0.1 excludes rlogin/rsh (`create-native-rust-linux-terminal` non-goals). These protocols are insecure by design: rlogin and rsh authenticate by trusting the client-supplied user name because the connection comes from a privileged source port (< 1024), and rexec sends the password in clear text. This change adds them as **legacy, off-by-default** session types with an honest treatment of the privileged-port problem, so users with legacy hosts can reach them from rustXterm. Behavior follows RFC 1282 and the BSD documentation.

## What Changes

- New protocol crate `crates/rsh` (L2): sans-IO handshakes and a tokio driver for **rlogin** (RFC 1282, TCP 513, interactive terminal with window-size updates), **rsh** (TCP 514, one remote command, no separate stderr channel) and **rexec** (TCP 512, one remote command, user + password).
- Privileged source port handling: try to bind a source port in 512–1023; when the OS refuses (normal for an unprivileged desktop app), fail with a clear explanation, or — only if the user enabled it for that session — connect from an unprivileged port for servers that accept it. rustXterm never uses setuid helpers, file capabilities, `sudo`/`pkexec`, or changes system settings.
- **Off by default**: a Settings toggle "Enable legacy r-commands (rlogin/rsh/rexec, insecure)". While off, the type is hidden from the editor and quick connect explains how to enable it.
- Strong insecurity labelling: Unencrypted marker, a warning on every editor page and a first-connect confirmation.
- New `Protocol::Rsh` with a mode (rlogin/rsh/rexec), session editor page, icon from the `assets/icons/app/` catalog, quick connect `rlogin://[user@]host[:port]` and `rsh://[user@]host[:port][?cmd=…]`.
- Reuse: terminal widget, session logging, MultiExec (rlogin tabs), reconnect, saved credentials (rexec password only, encrypted store).
- MobaXterm.ini import of Rsh bookmarks (only when the toggle is on; otherwise reported as skipped with the reason).
- Lab: an rsh-server container (rlogind, rshd, rexecd under inetd) with fictional users.

This change depends on and comes after `create-native-rust-linux-terminal`. It is the lowest-priority phase-2 module.

### Non-goals

Kerberized r-commands (`klogin`/`kshell`/`ekshell`), the separate stderr back-channel of rsh/rexec, rcp, running r-command servers, any privilege elevation (setuid, `CAP_NET_BIND_SERVICE`, polkit), delegating to system `rlogin`/`rsh` binaries (see design open questions), changing `net.ipv4.ip_unprivileged_port_start`.

## Capabilities

### New Capabilities

- `rsh-sessions`: rlogin/rsh/rexec sessions, privileged-port behavior, legacy toggle, insecurity labelling.

### Modified Capabilities

- `desktop-shell`: quick connect accepts `rlogin://` and `rsh://` when legacy r-commands are enabled; Settings gains the legacy toggle (added requirements).
- `session-management`: session editor offers the Rsh type when enabled; MobaXterm.ini import maps Rsh bookmarks (added requirements).

## Impact

- New crate `crates/rsh` (tokio `TcpSocket` for bind-then-connect; no new dependency); `cargo xtask layers` updated.
- `crates/core` (`Protocol::Rsh`, `RshOptions`), `crates/engine` (transport branch; rsh/rexec run as a terminal tab that ends when the command exits), `crates/ui` (settings toggle, editor page, warnings, quick connect, icon), `crates/store` (importer, rexec saved password), `crates/platform` (current local user name for the rlogin/rsh handshake, if not already exposed).
- `lab/compose.yaml` (`rsh` service on a bridge network without published ports), `lab/expected-tests.txt`.
