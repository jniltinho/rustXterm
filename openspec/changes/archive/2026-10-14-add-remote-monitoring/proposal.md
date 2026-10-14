## Why

Users of an SSH tab want an at-a-glance view of the server without installing anything on it: host name, CPU load with a small live graph, memory used/total, upload and download throughput, uptime and disk usage per mount point. rustXterm v0.1 (`create-native-rust-linux-terminal`) has no equivalent, and `refine-ui-layout` listed the status chips as out of scope. This change adds a thin "remote monitoring" bar at the bottom of every SSH tab, on by default and toggled from a "Remote monitoring" button at the bottom of the SFTP side panel.

## What Changes

- **Collector (engine)**: one long-lived exec channel per SSH connection runs a fixed POSIX `sh` loop. It installs nothing, writes no files, and answers one request line with one framed excerpt of `/proc/stat`, `/proc/meminfo`, `/proc/uptime`, `/proc/net/route`, `/proc/net/dev` and, less often, `df -P`. The engine paces the requests, so pausing costs the server nothing. Parsing and rate/percentage deltas happen in Rust. Non-Linux and BusyBox servers degrade by hiding the chips they cannot fill, and a tooltip says why.
- **Cost and safety bounds**: interval 1–30 s (default 2 s); the collector pauses while the bar is not painted (hidden tab, minimized or occluded window) and stops on disconnect, on toggle off or when the tab closes. It opens at most one extra session channel, never runs on jump hosts or non-SSH sessions, and caps the bytes it reads per line and per sample. The command string is fixed and contains no secrets.
- **Bar (UI)**: a 26 px bar at the bottom of SSH terminal tabs with chips for host, CPU % plus a 60-sample sparkline, RAM, upload, download, uptime and disks (one chip per mount, capped). Icons come from the existing catalog. Warning/danger colors come from theme tokens in light and dark. Scroll arrows appear when the chips overflow, and a close (×) button stops monitoring for that tab.
- **Settings**: a global "Remote monitoring" setting (default on) and the sampling interval on the Settings › SSH page, plus a per-session override (default: follow global) in the SSH session editor's advanced options.
- **SFTP panel button**: the "Remote monitoring" button at the bottom of the SFTP side panel (added by the `refine-ui-layout` follow-up work) toggles monitoring for the attached tab and shows whether it is active.
- Agents (CLI/MCP) can neither read metrics nor toggle monitoring in this change. The new command id is GUI-only.

This change depends on and comes after `create-native-rust-linux-terminal` (engine/UI seam D6, SSH exec on the tab connection as used by `follow.rs`, theme tokens D7, lab D11). It reuses the SFTP panel layout from `refine-ui-layout`.

### Non-goals

Per-process lists, history beyond the 60-sample sparkline, alerts or notifications, monitoring of Telnet/Serial/Rsh/XDMCP/RDP/VNC/FTP or standalone SFTP sessions, Windows OpenSSH servers (no POSIX shell), GPU/temperature/load-average chips, agent access to metrics, exporting metrics.

## Capabilities

### New Capabilities

- `remote-monitoring`: remote metric collection over SSH (collector protocol, supported servers, bounds, pause/stop rules, parsing and deltas) and the per-tab monitoring bar.

### Modified Capabilities

- `desktop-shell`: the bar's placement in SSH tabs, the Remote monitoring settings and the SFTP panel toggle (ADDED requirements; base requirements unchanged).
- `ssh-sessions`: the extra exec channel on the tab's connection and the per-session opt-out (ADDED requirement).

## Impact

- `crates/ssh`: `Client::exec_stream` (a session channel with exec, returning stdin writer + stdout reader; the existing one-shot `exec` is unchanged).
- `crates/engine`: new `monitor.rs` (collector task, parser, deltas), a `Link::exec_stream` default method, collector started next to `follow::run` in `tab.rs`, a per-tab latest-sample slot, `Command::SetMonitoring`, `Event::Metrics`.
- `crates/core`: `MetricsSample`, `Command::SetMonitoring`, `SshOptions.remote_monitoring: Option<bool>` (serde default `None`).
- `crates/store`: `Settings.remote_monitoring` (default `true`) and `remote_monitoring_interval_secs` (default 2, clamped 1–30), both with serde defaults.
- `crates/ui`: new `monitor_bar.rs`, Settings › SSH group, session editor checkbox, SFTP panel button wiring.
- No new third-party dependency. No database migration. `lab/expected-tests.txt` gains the monitoring lab tests.
- Server side: one extra `sh` process per monitored tab. Its command line (marked `rxt-mon`) is visible to the server's administrators in `ps` and in the auth log entries for exec requests.
