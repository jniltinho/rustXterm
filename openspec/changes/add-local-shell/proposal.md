## Why

In MobaXterm the "+" button after the last tab opens a local terminal on the user's own machine (a shell in `/home/mobaxterm` with a welcome banner). rustXterm's "+" only opens the New session editor, so users coming from MobaXterm lose the quickest way to get a local prompt. The specs already plan the local shell for "phase 2" (terminal-emulation purpose, agent-control echo-off note, the Windows port's ConPTY deferral); this change delivers it on Linux and Windows.

## What Changes

- The tab strip "+" (and `Ctrl+Shift+T`) opens a **local terminal tab** in a pseudo-terminal:
  - Linux: the user's login shell (`$SHELL` → passwd shell → `/bin/sh`) in `$HOME`.
  - Windows (ConPTY, Windows 10 1809 or later): `pwsh.exe` → Windows PowerShell → `%ComSpec%`, in `%USERPROFILE%`, with no console window.
- A MobaXterm-like **welcome banner** (version, user@host, shell, home folder, tips) is printed before the first prompt when "Show session banner" is on; on Windows the pseudo-console starts below it.
- Tab lifecycle like other terminal tabs: resize, copy/paste, find, context menu, Save terminal text, Duplicate (another local shell), "Session stopped" on exit, R restarts, close ends the shell and the processes it started (process group/session on Linux, job object on Windows).
- Local terminals are never persisted: not saved, imported, exported, in recents or in history.
- **Agent boundary**: local tabs are listed (protocol `local`) but every other agent operation on them, and agent input reaching them through MultiExec, is refused. Terminal protocol replies (DA, DSR) go only to the tab that asked, never through MultiExec (also closes an existing cross-tab path between remote tabs).
- New session stays on the toolbar, the Sessions menu and `Ctrl+Shift+N`; where no backend exists (macOS, Windows without ConPTY) "+" keeps opening New session.
- Dependencies: `portable-pty` (MIT) in `platform-linux` only; the Windows backend is written on the existing `windows-sys` in `platform-windows` (the only crate allowed `unsafe`), because job assignment must happen before the shell runs.

## Capabilities

### New Capabilities
- `local-shell`: local terminal tab (shell selection, working folder, environment, banner, lifecycle and containment, persistence exclusion, platform availability).

### Modified Capabilities
- `desktop-shell`: the Tabs requirement — "+" opens a local terminal where supported.
- `agent-control`: new requirement for local terminal tabs; MultiExec input from agents skips local tabs.
- `terminal-emulation`: new requirement — terminal replies are delivered only to their originating tab; Session logging — local tabs save visible text only, no log file.
- Active change `add-windows-port` (not archived, edited in place): its "Local shell not offered" terminal delta, non-goals and coverage table; new receipt row W42 for the local terminal (W33, "no local shell entry in the session editor", stays true).

## Impact

- `crates/core` (`Protocol::Local`, `is_terminal`), `crates/store` (reject Local on save/import), `crates/platform` (`LocalPty` contract, launch metadata, `FakePty`), `crates/platform-linux` (`portable-pty`), `crates/platform-windows` (ConPTY + job object on `windows-sys`), `crates/engine` (`LocalLink`, tab dispatch, `EngineConfig`/`Real::new` injection, reply routing, agent checks), `crates/ui` (command, "+", banner, recents/history skip), app startup wiring.
- `xtask/src/layers.rs` (`portable-pty` owned by `rxt-platform-linux`), `xtask/src/receipt.rs` (rows 1..=42), `deny.toml` if needed, `Cargo.lock`.
- Docs: user guide, `docs/gates/release.md` scenario map, `docs/gates/windows.md` (fresh receipt with W42).
