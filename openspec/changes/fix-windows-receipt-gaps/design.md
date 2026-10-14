## Context

Findings F1–F5 come from the v0.2.2 Windows receipt (`docs/gates/windows.md`). Relevant code today:
- `crates/agent/src/cli.rs` `prompt_password` runs `stty -echo` / `stty echo` (absent on Windows).
- `crates/agent/src/mcp.rs` `Server::live()` checks `Path::new(endpoint).exists()`. On Windows the endpoint is a named pipe with a single listening instance; `exists()` opens it as a client, so it is false while the instance serves another call (the request then takes the offline path and returns `not_running`) and it costs the server an extra accept.
- The control servers (`platform-windows` `PipeControl`, `platform-linux` `UnixControl`) close refused clients without a reply; `rxt_platform_windows::server_allowed` already has the elevation message for the client-side check.
- `Notifier` and `Opener` are implemented by both adapters but never called by the UI or engine.
- Transfers live in one list shared by all file views; dialogs inherit the process working directory.

## Goals / Non-Goals

**Goals:** make rows W25, W27, W28, W30, W39 pass, remove the F1 flakiness behind W19, fix the minor items, and produce a release receipt that `cargo xtask windows-receipt --check` accepts.

**Non-Goals:** code signing (separate owner decision), new notification kinds beyond transfers and unexpected disconnects, an MSI change for the Server 2022 `DisableMsi` policy (documented instead), lab VM tweaks beyond notes.

## Decisions

1. **No-echo prompt (F2):** use a small, maintained crate already common in the Rust ecosystem for reading passwords from the console (`rpassword`), behind `rxt-agent`; it uses the console API on Windows and termios on Unix and restores the mode on drop. Alternative: hand-written `SetConsoleMode` would need `unsafe` outside `platform-windows`, which the workspace forbids; a platform trait method would work but adds surface for one call. `cargo deny` must accept the crate.
2. **Liveness (F1):** replace the `exists()` probe with a real call: try to connect with a short budget; on Windows a `ERROR_PIPE_BUSY` (all instances busy) means "running, retry", not "absent", and the connection is retried with `WaitNamedPipe`-equivalent waiting inside the adapter for up to 2 s. Only "endpoint absent" maps to `not_running`. The server side also keeps one spare listening instance ready while serving (create the next instance before handling the accepted one) so concurrent clients rarely see busy at all.
3. **Refusal reasons (F5):** the server writes one framed error response (`{"error":{"code":"denied","message":…}}`, the existing JSON shape) before closing a refused client; the reason text comes from the existing policy (`peer_allowed` cases), with the elevation case using the same sentence as `server_allowed`. The CLI maps `denied` to its denied exit code.
4. **Notifications (F3):** the UI owns the decision (it knows window focus via `ctx.input(|i| i.focused)`): on engine events `TransferDone/TransferFailed` and on a tab going to `Disconnected` without a user close/reconnect action, call `platform.notifier.notify(title, body)` off the UI thread when unfocused and the new `notifications` setting (default on) allows. Errors are logged at debug level only.
5. **Open (F4):** transfer rows get "Open" and "Show in folder" once done (download destination is already known); the local pane enables "Open" for files. Both call `platform.opener.open(path)` (folder for "Show in folder"); `Err` becomes a status/error toast in the app.
6. **Per-tab transfers:** key the transfer list by tab id (the engine events already carry the tab); the panel filters by its tab.
7. **Dialog start folder:** the platform `FileDialogs` calls take a start directory; the UI passes the last used folder per action kind (kept in memory, persisted in settings later if asked) or `dirs::download_dir()` / home.
8. **Serial on Windows:** quick connect parsing accepts `COM<n>` and `\\.\COM<n>` hosts on Windows and builds the port path the serial crate expects; the placeholder comes from the platform (`COM3` on Windows, `/dev/ttyUSB0` on Linux).

## Risks / Trade-offs

- New dependency `rpassword` → small and widely used; check licenses with `cargo deny`; fallback is a platform trait method if refused.
- Spare pipe instance → slightly more code in the Windows adapter's `unsafe` area; covered by the existing pipe tests plus a concurrency test (two clients in parallel, one held open).
- Notifications on unfocused state only → users who keep the window focused but look elsewhere get none; acceptable, matches common desktop behavior.
- The receipt needs the release commit itself; the release flow must commit the receipt right after the version commit (as the checker expects) or the checker must accept a list of docs-only follow-up commits.
