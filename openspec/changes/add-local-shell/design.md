## Context

Terminal tabs reach the engine through `Transport::connect`, which returns an `Arc<dyn Link>`. Serial is the precedent for a non-network link: the `platform::SerialPorts`/`SerialStream` contract (blocking read with timeout, cloned writer), the `serialport` adapter, `SerialLink` in `engine/src/transport.rs`. Other facts that shape this change:

- The workspace forbids `unsafe`; only `rxt-platform-windows` may use it (`windows-sys` there). `cargo xtask layers` has an OS-crate ownership table (`OWNED`).
- `engine/src/tab.rs` has an exhaustive protocol dispatch; `Protocol::is_terminal()` gates terminal routing; `EngineConfig`/`Real::new` inject platform services; the launch size reaches the link through `Link::shell`.
- `ui/src/app.rs` turns `TermEvent::Reply` into `Command::Input`, and `Shared::input` (`engine/src/lib.rs`) broadcasts input through MultiExec — so replies are broadcast today.
- `Action::Open` pushes every session to recents and history; `Store::save_session` and TOML import accept any `Session`.
- Logging starts only from `session.terminal.log_file`; the banner code handles SSH/Telnet only.
- CI: `windows-build` (cross build + Windows-target clippy in Docker) and `windows-test` (runs the cross-compiled test executables on `windows-latest`). The Windows receipt (`xtask windows-receipt`) checks rows W01–W41, the commit and the artifact digests.

## Goals / Non-Goals

**Goals:**
- "+" and Ctrl+Shift+T open a local shell tab on Linux and Windows (ConPTY), with a banner, contained processes and a strict agent boundary.
- Fix reply routing so no tab's terminal replies reach another tab.

**Non-Goals:**
- Saved local-shell sessions, a shell picker, Git Bash/WSL detection, continuous logging for local tabs (Save terminal text works) — later changes.
- macOS (contract ready; no adapter).
- A MobaXterm-style bundled Unix environment (`/home/mobaxterm`): the real home folder is used.
- Echo-off masking for agent reads (agents get no access instead).

## Decisions

1. **`Protocol::Local`** (not in `Protocol::ALL`, `is_terminal()` true, port 0, icon `terminal-shell`). Tabs, icons, state and the agent listing key on `Session.protocol`. Persistence is blocked at the boundaries, not by `id == 0`: `Store::save_session` and import reject Local; `push_recent`/`history` skip it; agent `Open` and tunnel ops deny a resolved Local session. *Alternative:* a separate tab kind outside `Session` — more churn everywhere `tab.session` is read.

2. **Contract in `crates/platform`** — split handles so reads never block control:
   - Two steps, so the banner exists before the shell: `LocalPty::prepare() -> Result<LaunchInfo>` resolves shell, arguments, cwd, user and host without starting anything; `LocalPty::spawn(&LaunchInfo, size, start_cursor) -> Result<LocalShell>` starts it. Availability is `Platform.local_pty: Option<Arc<dyn LocalPty>>`.
   - `LocalShell { reader: Box<dyn PtyReader>, writer: Box<dyn PtyWriter>, control: Box<dyn PtyControl> }`.
   - `PtyReader::read` blocks until data, EOF or `PtyControl::close`; `PtyWriter::write_all`; `PtyControl::{resize, close, wait_exit(timeout)}`; `LaunchInfo { shell, args, cwd, user, host }` feeds the tab title and banner (fictional values in `FakePty`).
   - **Blocked writes.** `PtyWriter` runs on a dedicated adapter thread fed by a bounded queue; `close` never joins it. Linux: close kills the session's process groups (decision 3); once no process holds the slave, the kernel fails the pending `write` on the master with `EIO` and the thread ends. A process that escaped the session and holds the slave without reading keeps that one thread blocked until it exits or reads — the documented Linux containment limit; the tab, UI and engine never wait on it. Windows: teardown calls `CancelSynchronousIo` on the writer thread, then closes the input pipe; the thread ends.
   - **Start sequence (open and R).** Engine: `prepare` on a blocking task → `Event::LocalReady { tab, info }`. UI: writes the banner (or nothing when off) into the tab's terminal, reads the terminal's cursor and size → `Command::LocalStart { tab, cursor, size }`. Engine: `spawn` with them. A tab closed at any step before `LocalStart` spawns nothing; a tab closed while `spawn` runs closes the returned shell at once. `prepare` failure → error state, as a spawn failure.
   - `FakePty` for engine/UI tests (scripted output, recorded input/resizes, exit, slow/blocked writes).

3. **Linux adapter on `portable-pty`** (safe API; the crate cannot use `unsafe`). Exact version pinned; listed in `OWNED` as Linux-only (`rxt-platform-linux`, target-gated). The child gets `setsid` + controlling TTY (portable-pty does this). Close: SIGHUP to the session's process groups (`/proc/*/stat` session id = shell pid), wait 1 s, SIGKILL, reap. Environment: built explicitly from `std::env::vars_os()` with the AppImage cleanup (decision 6), never logged.

4. **Windows adapter on `windows-sys`** (in the one `unsafe` crate, with the crate's documented-unsafe lints):
   - Availability: `GetProcAddress(kernel32, "CreatePseudoConsole")` at startup; missing → `local_pty: None` (no panic, no probe console).
   - Spawn: `CreatePipe` ×2, `CreatePseudoConsole(size, in, out, PSEUDOCONSOLE_INHERIT_CURSOR)`, a job object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` (handle not inheritable), `CreateProcessW` with `EXTENDED_STARTUPINFO_PRESENT` and both `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` and `PROC_THREAD_ATTRIBUTE_JOB_LIST`, so the shell is in the job before its first instruction; failure to assign → spawn fails (fail closed). Inherited environment block (`lpEnvironment = null`) plus `TERM`/`COLORTERM` via a built block; no registry rebuild.
   - Teardown: keep the reader thread draining, `ClosePseudoConsole` on a worker thread, `TerminateJobObject`, close the job handle; bounded by a 2 s timeout. If the app is killed, the job handle closes with the process and the OS kills the tree.
   - `PSEUDOCONSOLE_INHERIT_CURSOR` makes ConPTY ask for the cursor position (`ESC[6n`) once at start. The **adapter** answers it itself, never the UI: `spawn` takes the start cursor (the terminal cursor after the banner, from `LocalStart`), the reader thread recognizes the first `ESC[6n`, writes `ESC[row;colR` to the input pipe and removes the query from the output. Before `ClosePseudoConsole`, an unanswered query is answered the same way, so teardown never waits on a UI tab that close already removed (Microsoft's inherited-cursor deadlock).
   - Ownership: one adapter-owned teardown thread holds the pseudo-console, pipes, process and job handles; `PtyControl::close` signals it and returns. The thread drains output, answers a pending cursor query, calls `ClosePseudoConsole`, `TerminateJobObject`, closes every handle and exits; a native test asserts the thread and handles are gone, including a close right after spawn.
   *Alternative:* `portable-pty` on Windows — rejected: it starts the process running (no job assignment at creation), rebuilds the environment from the registry and panics if ConPTY symbols are missing.

5. **`LocalLink` in `engine/src/transport.rs`**, created by `Real::connect` for `Protocol::Local` from the injected `local_pty` (`EngineConfig` → `Real::new`, wired in app startup; `tab.rs` dispatch arm). Reader thread → bounded channel (same capacity as serial) with backpressure; writer task; control used for resize/close. Exit is watched **concurrently** by a waiter on `wait_exit` (a child that survives the shell can keep the PTY open, so EOF is not the signal). On shell exit the link reads what is already buffered for up to 200 ms, then closes the PTY (same teardown as close) and reports `Disconnected`. Blocked writes and the start sequence follow decision 2; the link never awaits the writer thread on close. Reconnect spawns a new `LocalShell`.

6. **Environment policy.** Child env = parent env − (`APPDIR`, `APPIMAGE`, `ARGV0`, `OWD`) − path-list entries under `$APPDIR` (`PATH`, `LD_LIBRARY_PATH`, `XDG_DATA_DIRS`, `GSETTINGS_SCHEMA_DIR`...; a variable left empty is removed) + `TERM`, `COLORTERM`. Applied only when `APPIMAGE` is set. Windows keys compared case-insensitively. Nothing from the vault is ever in the parent env.

7. **Reply routing.** New `Command::Reply { tab, bytes }` sent for `TermEvent::Reply`; the engine writes it to that tab only (no MultiExec, no follow kick). Fixes the existing cross-tab path too.

8. **Stopped-banner ordering in the UI.** Today the UI applies state events before draining the output buffer, so final bytes could land after "Session stopped". When a tab's state becomes `Disconnected`, the UI first drains and feeds that tab's pending output (no frame budget for this tab), then writes the stopped banner. Applies to every protocol.

9. **Banner.** `banner::local(&LaunchInfo, version, cols)` in the box style, written by the UI on `LocalReady`, before `LocalStart` (decision 2), on open and on R. On Windows the adapter's inherited-cursor answer keeps ConPTY below it.

10. **UI.** `CommandId::NEW_LOCAL_TERMINAL` (UI-only, default Ctrl+Shift+T, Settings shortcut row), no vault requirement. "+" runs it when `platform.local_pty.is_some()`, else `NEW_SESSION`. Local tabs: no SFTP auto-switch, no monitoring bar, Duplicate → new local terminal.

11. **Agent boundary** in `engine/src/control.rs`: one check after resolving the target tab or session — Local → `ErrorCode::Denied` "local terminals are not available to agents", audited. `agent_send` filters MultiExec targets, returns `skipped`, audits them. Locked store → existing `Locked`.

12. **Receipt.** W33 keeps its meaning (no local shell in the session editor). New row W42 "local terminal": "+" opens PowerShell in `%USERPROFILE%`, banner above the prompt, resize, `exit` + R, close and Task Manager kill end `ping -t`, no console window for each shell branch (pwsh, powershell, cmd). `xtask/src/receipt.rs` rows become 1..=42; a fresh receipt for the new artifacts is required.

## Risks / Trade-offs

- [Linux containment is bounded] → processes that call `setsid` escape, as in any terminal; documented in the spec and user guide. A cgroup-based scope is a later option.
- [ConPTY quirks (repaint on resize, inherited cursor)] → native tests in `windows-test` and W42 in the existing Windows Server 2022 VM (Windows 11 optional, as recorded in add-windows-port).
- [New unsafe Win32 code] → confined to `rxt-platform-windows` with its lints; each unsafe block documented; native tests for spawn, job assignment failure (nested job), teardown under flood.
- [Snapshots depending on the host] → snapshots use `FakePty` with fictional `LaunchInfo`; goldens regenerated with `make snapshots`, reviewed, then compared frozen in CI.
- [Dependency surface] → `portable-pty` pinned, Linux-only, owned by `rxt-platform-linux` in `xtask layers` with a negative test; `cargo deny check` for both targets; `dist-check` glibc limit verified.
