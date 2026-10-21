# Proposal

## Why

On Windows, MobaXterm users open their WSL distributions (Ubuntu, Debian…) as saved sessions next to
SSH and RDP: a tab with the distribution's shell, one click away. rustXterm's local terminal opens
only PowerShell or `cmd.exe`, and the local-shell change left "Git Bash/WSL detection" for later.
Users moving from MobaXterm expect the same WSL sessions, and their MobaXterm.ini bookmarks to come
along.

## What Changes

- A **WSL** session type, **Windows only**: a saved session that opens a WSL distribution's shell in a
  terminal tab. Options, as in MobaXterm's WSL session: distribution (the default one or a named one),
  user, startup folder and startup commands (run in the shell, which then stays open).
- The session editor lists the installed distributions (from the WSL registry entries, default
  marked) with a refresh button, and tells apart "WSL not installed", "no distribution" and "could not
  query" (detected from the WSL service, not from `wsl.exe`, which Windows ships before WSL is
  installed); rustXterm never installs anything.
- The tab runs `wsl.exe -d <distribution> [-u <user>] [--cd <folder>]` in the local terminal's
  ConPTY, each value quoted as its own argument (MobaXterm's "Wsl command" run method), so the user's
  own login shell starts; startup commands are then typed into it once it is ready, as MobaXterm
  does. Resize, exit status and reconnect as in the local terminal. Closing
  the tab hangs up the session (its foreground programs end); programs that detach (`nohup`,
  `setsid`, services) keep running in the distribution, as with MobaXterm and Windows Terminal.
- WSL tabs get the local terminal's protections: no agent access (CLI/MCP), skipped by MultiExec
  from other tabs, never in data-stream capture; per-session logging works as for SSH.
- Quick connect `wsl://` (default distribution) and `wsl://<distribution>`; MobaXterm.ini WSL
  bookmarks (type 14) are imported once their field layout is confirmed by observing MobaXterm (its
  license, section 5.1, allows observing, studying and testing its operation; no decompiling).
- Databases and exports stay readable: a reader skips sessions of an unknown protocol with a warning
  instead of failing, and WSL sessions need rustXterm 0.8.0 or newer (release notes say so).
- On Linux and macOS the type is not offered; a WSL session from a shared database or export is kept
  and shows "WSL sessions run only on Windows" instead of starting.
- Out of scope (MobaXterm has them; later changes): its "Localhost connection" run method (an SSH
  tunnel into the distribution, used for its X server and file browser), the experimental graphical
  desktop, a file browser of the distribution (`\\wsl.localhost\…`), macros; also installing or
  managing distributions, Git Bash/Cygwin, WSLg specifics.

## Capabilities

### New Capabilities
- `wsl-sessions`: WSL session type, distribution detection, launch, quick connect, import and the
  Windows-only availability.

### Modified Capabilities

## Impact

- `crates/core`: `Protocol::Wsl` (saved, serialized on every platform) and `WslOptions`;
  `wsl://` in `quick_connect`.
- `crates/platform`: `Wsl` contract (`distributions`, `launch_info`) with a fake; `Platform::wsl`
  is `Some` only in `crates/platform-windows` (reuses the ConPTY `LocalPty::spawn`).
- `crates/engine`: the WSL tab path next to the local terminal (`local::start` with the WSL
  launch info); WSL tabs are left out of data-stream capture like local terminals.
- `crates/ui`: WSL page in the session editor (distribution picker, user, folder, command), icon,
  banner; `cargo xtask prints` light and dark.
- `crates/store`: MobaXterm.ini WSL bookmarks; export/import keeps `wsl` sessions.
- No new dependencies.
