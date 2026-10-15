## 1. Core, store and contract

- [x] 1.1 Add `Protocol::Local` (outside `Protocol::ALL`, `is_terminal()` true, port 0, icon `terminal-shell`) and make every exhaustive match compile; verify with core unit tests for `is_terminal`/`ALL`
- [x] 1.2 Reject Local in `Store::save_session` and TOML import (other entries still import, with a message); verify with store tests for a direct save and a crafted TOML file
- [x] 1.3 Add the `LocalPty` contract (`prepare` → `LaunchInfo`, `spawn(info, size, start_cursor)`, split reader/writer/control, writer on its own thread with a bounded queue), `Platform.local_pty: Option<…>` and `FakePty` (scripted output, recorded input/resizes, exit, blocked writes); verify with `FakePty` unit tests and `cargo xtask layers`

## 2. Linux adapter

- [x] 2.1 Add pinned `portable-pty` to `rxt-platform-linux` (target-gated), add it to `OWNED` in `xtask/src/layers.rs` with a negative test, update `Cargo.lock`; verify `cargo xtask layers`, `cargo deny check` and `cargo xtask dist-check` on a `make dist` build
- [x] 2.2 Implement spawn (shell selection, cwd fallback, login shell, environment policy with AppImage cleanup, no env logging) and close (SIGHUP to the session's groups, SIGKILL after 1 s, reap); verify with Docker tests: `pwd`, `$TERM`, invalid `SHELL`, missing `HOME`, `stty size` after resize, AppImage-variable cleanup with a fake `APPDIR`, close leaving no `sleep 1000 &` / `trap '' HUP` / `sleep | cat` process, and close with a blocked write (shell stopped with `kill -STOP $$` while 1 MiB is queued) returning within 2 s with the writer thread ended after the kill

## 3. Windows adapter

- [x] 3.1 Implement ConPTY detection via `GetProcAddress` and spawn with `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` + `PROC_THREAD_ATTRIBUTE_JOB_LIST` (kill-on-close job, non-inheritable handle, fail closed), shell selection `pwsh` → Windows PowerShell → `%ComSpec%` (paths with spaces), cwd fallback, inherited environment + `TERM`/`COLORTERM`, `PSEUDOCONSOLE_INHERIT_CURSOR`; verify with Windows-target clippy in `windows-build` and native tests in `windows-test` (spawn each shell branch with an injected resolver, echo, Unicode round trip, resize, Ctrl+C)
- [x] 3.2 Answer ConPTY's inherited-cursor query in the adapter (start cursor from `spawn`, query stripped from output) and implement the adapter-owned teardown thread (drain, answer a pending query, `ClosePseudoConsole`, `TerminateJobObject`, close handles); verify with native tests: close idle shell, close right after spawn before the cursor reply, close during an output flood, close with a blocked write (`CancelSynchronousIo`), teardown and writer threads and handles gone, child `ping -t` gone after close, spawn inside a nested job still contained

## 4. Engine

- [x] 4.1 Inject `local_pty` through `EngineConfig`/`Real::new` and app startup; add the start sequence (`prepare` → `Event::LocalReady` → `Command::LocalStart` → `spawn`, nothing spawned when closed before start, shell closed when closed during spawn) and `LocalLink` (bounded channel, writer task, resize, exit after EOF drain → `Disconnected`, reconnect respawns) and the `tab.rs` dispatch arm; exit watched concurrently (shell exit with a surviving child still stops the tab), close during spawn; verify with engine tests on `FakePty`: output, input, resize, exit while a child holds the PTY, reconnect, close during flood, close with a blocked write, close during spawn
- [x] 4.2 Add `Command::Reply` and route `TermEvent::Reply` through it to the originating tab only; verify with an engine test (MultiExec A+B, reply on A, B receives nothing) and the existing UI tests
- [x] 4.3 Agent boundary: deny every tab- or session-targeted op on Local (read, wait, send, screenshot, files, close, serial, open, tunnels) with an audited denial; `agent_send` skips Local MultiExec peers, returns `skipped` and audits them; locked store keeps `Locked`; verify with control tests per op through both CLI and MCP entry points, a mixed MultiExec group (zero bytes to Local) and a locked start

## 5. UI

- [x] 5.1 Add `CommandId::NEW_LOCAL_TERMINAL` (UI-only, Ctrl+Shift+T, Settings shortcut row, no vault requirement) opening an unsaved Local session titled from `LaunchInfo`; skip Local in `push_recent`/`history`; verify with UI tests: active local tab, `Command::Open` with Local, nothing in recents after reopening the app state
- [x] 5.2 "+" runs `NEW_LOCAL_TERMINAL` when `local_pty` exists (tooltip "New local terminal"), else `NEW_SESSION`; verify both branches with UI tests
- [x] 5.3 Add `banner::local` from `LaunchInfo` (version, user@host, shell, folder, tips); on `LocalReady` write it (when on) and send `LocalStart` with the terminal cursor and size, on open and on R; verify with a UI test that `LocalStart` carries the cursor row right below the banner (and row 0 with the banner off); verify with banner unit tests (narrow and wide) and Light/Dark/System snapshots using fictional `FakePty` metadata (`make snapshots`, review, then frozen run)
- [x] 5.4 On `Disconnected`, drain and feed the tab's pending output before writing the stopped banner (all protocols); verify with a UI regression where the final output exceeds one frame's byte budget and the banner is last
- [x] 5.5 Local tab behavior: Duplicate → new local terminal, close confirmation while running, no SFTP switch, no monitoring bar; verify with UI tests

## 6. Windows planning and receipt

- [x] 6.1 Edit `openspec/changes/add-windows-port`: replace the "Local shell not offered" requirement and scenario, update proposal/design non-goals and the coverage table, add W42 to task 3.2; set `xtask/src/receipt.rs` rows to 1..=42 with its tests; verify `openspec validate add-windows-port --strict` and `cargo test -p xtask`
- [ ] 6.2 Run W42 (and re-run the full receipt for the new artifacts) in the existing Windows Server 2022 VM with the release-pipeline zip and MSI: PowerShell in `%USERPROFILE%`, banner above the prompt, resize, `exit` + R, close and Task Manager kill end `ping -t`, no console window for pwsh, powershell and cmd; verify `cargo xtask windows-receipt --check`

## 7. Docs and verification

- [x] 7.1 User guide "Local terminal" (shells, folder, containment limits, no log file, agents cannot use it) and README feature line; verify `cargo xtask guard`
- [ ] 7.2 At sync/archive (`/opsx:archive`), sync the deltas into `openspec/specs/` and add their rows to `docs/gates/release.md` in the same commit; verify `cargo xtask scenarios` (it reads only main specs, so it cannot pass earlier)
- [x] 7.3 Full Docker verification batch: fmt, clippy `-D warnings` (Linux and Windows targets), workspace tests, frozen snapshots, `cargo xtask prints`, layers, guard, deny, lab; manual check of "+" in the `make dist` binary and AppImage (`pwd`, banner, `exit` + R, close kills background jobs)
