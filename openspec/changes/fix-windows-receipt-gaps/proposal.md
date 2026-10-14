## Why

The Windows end-to-end receipt for v0.2.2 (`docs/gates/windows.md`, task 3.2 of `add-windows-port`) passed 35 of 41 rows but failed four (W25, W27, W30, W39) and could not run two (W17 needs hardware, W28 is blocked by W27). The failures are product gaps, not test problems: the CLI cannot read the master password on Windows, nothing ever raises a desktop notification, downloaded files cannot be opened, MCP calls fail while another call is in flight, and an elevation mismatch is not reported as such. Until they are fixed, the release pipeline keeps the Windows zip and MSI out of releases.

## What Changes

- **F2, master password on every platform:** the CLI reads the master password without echo through a portable terminal routine (no `stty`), so offline reads, `sessions add/edit/remove` and `import` work on Windows terminals as on Linux.
- **F1, MCP live calls:** deciding whether the GUI is running no longer probes the control endpoint by opening it (on Windows that consumes the single listening pipe instance); a busy channel is retried instead of falling back to the offline path, so overlapping MCP calls in one session succeed.
- **F5, refusal reasons:** a refused control client receives a reason before the channel closes, and an elevation mismatch is reported in both directions as "agent control runs at a different elevation".
- **F3, desktop notifications:** the app raises a notification when a transfer finishes or fails, or a session disconnects unexpectedly, while the main window is not focused; a setting turns them off; an unavailable notification service never shows an error.
- **F4, open files:** finished downloads can be opened or shown in their folder from the transfer list, and local files can be opened from the local pane, through the platform opener; failures are reported.
- **Minor:** quick connect `serial://COM1` opens `COM1` on Windows; the serial port field shows a Windows example; each tab has its own transfer list; save/open dialogs start in the user's Downloads (or last used) folder instead of the process working directory.
- **Receipt:** a new Windows receipt run on the release commit, with W17 on real hardware or explicitly waived by the owner.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `agent-control`: portable master-password prompt, live-call routing that tolerates a busy channel, and refusal reasons delivered to clients (added requirements).
- `desktop-shell`: desktop notifications for background events, dialog start folder, Windows serial names in quick connect (added requirements).
- `file-transfer`: open/show downloaded and local files, per-tab transfer lists (added requirements).

## Impact

- Code: `crates/agent` (password prompt, MCP live routing), `crates/platform-windows` and `crates/platform-linux` (control server refusal message), `crates/core` (endpoint liveness contract if needed), `crates/ui` (notifications, transfer list actions, local pane Open, dialog start folder, quick connect serial parsing, serial placeholder), `crates/engine` (events that drive notifications).
- Docs: `docs/gates/windows.md` (new receipt), `openspec/changes/add-windows-port/tasks.md` (task 3.2).
- No new dependencies expected beyond a small cross-platform no-echo reader if the standard library route is not enough (to be decided in design).
