## Context

Target behavior: a one-line bar sits under the terminal of an SSH tab. It holds chips with icons for the host name, CPU % with a small live graph, RAM used/total, upload and download throughput (Mb/s), uptime ("25 hours") and one chip per mount point ("/: 42%", "/home: 84%", …). Left/right arrows scroll the chips when they overflow, and a red × closes the bar. The SFTP side panel ends with a "Remote monitoring" button. The global setting "Remote monitoring" on the Settings › SSH page is on by default. The data comes from an exec channel on the terminal's SSH connection that runs a `/proc` polling loop.

Seams in rustXterm today:
- `crates/ssh/src/lib.rs` `Client::exec`: a one-shot session channel with a 64 KiB output cap, used by `crates/engine/src/follow.rs` through `Link::exec` (`transport.rs`).
- `crates/engine/src/tab.rs` starts the `follow` future after the shell channel opens and joins it in the same `select!` as the output and input pumps, so it ends with the connection run.
- D6: commands in, events out, byte-bounded per-tab buffers drained by the UI per frame (the UI's drain is the backpressure signal).
- D7: `core::theme::Tokens` already has `warning`, `danger` and `success` in light and dark. The icon catalog has `chip`, `ram`, `upload`, `download`, `monitor-clock`, `hard-disk`, `server-tower`, `monitor-pulse`, `close`, `back` and `forward`.
- `refine-ui-layout`: the Settings window has an SSH page, and the SFTP side panel ends with "Follow terminal folder". The "Remote monitoring" button below it is being added by parallel UI work. This change wires it.

## Goals / Non-Goals

**Goals:**
- At-a-glance server metrics with nothing installed or written on the server.
- Negligible cost: no server work while nobody looks, bounded bytes, one extra channel.
- Graceful degradation on servers without Linux `/proc`.

**Non-Goals:**
- See proposal non-goals (process lists, alerts, history, non-SSH protocols, agent access).

## Decisions

### M1. Collector: one long-lived, request-driven exec channel per monitored tab
The engine opens one session channel with exec on the tab's own SSH connection (the final target, never a jump host). It runs this fixed command (no interpolation, no secrets; `rxt-mon` marks it in `ps`):

```sh
exec sh -c 'echo "rxt-mon 1 $(uname -s) $(uname -n)"
while read -r k; do
  echo @s
  if [ -r /proc/stat ]; then
    head -n 1 /proc/stat
    grep -E "^(MemTotal|MemAvailable|MemFree|Buffers|Cached):" /proc/meminfo
    echo "@u $(cat /proc/uptime)"
    echo @r; grep -E "^[^[:space:]]+[[:space:]]+00000000[[:space:]]" /proc/net/route
    echo @n; head -n 130 /proc/net/dev
  fi
  if [ "$k" = d ]; then echo @d; df -Pkl 2>/dev/null || df -Pk; fi
  echo @e
done'
```

- **Pull, not push**: the remote loop blocks in `read` until the engine writes a request line (`\n`, or `d\n` to include disks). The engine owns the cadence: interval changes apply without a restart, pausing costs the server nothing (one sleeping `sh`), and closing the channel gives `read` an EOF, so the loop exits on its own.
- **Raw excerpts, parsed in Rust**: the server only runs `head`, `grep`, `cat` and `df`. These exist in coreutils and BusyBox. All logic (CPU jiffy deltas, MemAvailable fallback, default-route interface, rates, disk filtering) is pure Rust over captured text, so it can be unit-tested with real `/proc` samples.
- **Disks less often**: `d` is sent on the first request and then at most every 30 s (`df` is the only heavy command and disk usage changes slowly). `-l` skips network mounts on GNU/BSD/macOS, so a hung NFS server cannot stall the loop. BusyBox lacks `-l` and falls back to plain `df -Pk`.
- The header line `rxt-mon 1 <os> <host>` must arrive within 10 s. Lines before it (login-script noise) are ignored. A missing or wrong header (Windows OpenSSH, `ForceCommand`, sftp-only accounts, non-POSIX shells) marks monitoring unsupported for this connection and closes the channel.

Alternatives:
- Push loop (`while :; do …; sleep 2; done`). The server works even when nobody looks, pausing needs a channel restart, and `sleep` with fractions is not portable.
- Repeated one-shot `exec` per sample. This means a channel open, a login-shell start and an auth-log line every 2 s. It is about 10× the overhead and noisy in server logs.
- SFTP reads of `/proc` files over the tab's SFTP channel. This has no `df`, many `/proc` files report size 0 to SFTP stat (servers read them anyway, but behavior varies), and it would contend with user transfers.
- Server tools (`top -b`, `free`, `vmstat`, `ip`). Output formats differ across procps, BusyBox and BSD, and parsing is harder than raw `/proc`.

### M2. Parsing and deltas (`crates/engine/src/monitor.rs`)
- Sample = `{ cpu: Option<f32>, mem: Option<(used, total)>, net: Option<(tx_bps, rx_bps)>, uptime: Option<Duration>, disks: Option<Vec<Disk>>, host, os }`. `None` hides the chip.
- CPU: the `cpu` line of `/proc/stat`. busy = total − (idle + iowait). The percentage comes from the delta to the previous sample. The first sample shows no CPU value. Counters that go backwards (reset) skip that sample.
- RAM: used = MemTotal − MemAvailable. Kernels before 3.14 lack MemAvailable, so the fallback is MemFree + Buffers + Cached. Without MemTotal, `None`.
- Network: the interfaces of the default route(s) from `/proc/net/route`. Without a default route, the sum of all interfaces except `lo`. The rate is the byte delta divided by the delta of `/proc/uptime` (the server's monotonic clock, so client scheduling jitter does not skew rates), shown in bits/s (kb/s, Mb/s, Gb/s). Wrapping counters (old 32-bit) skip the sample. Interface lines are capped at 128.
- Uptime: first field of `/proc/uptime`, shown as `12 min`, `5 h 12 min` or `3 d 4 h`.
- Disks: `df -P` rows kept when the mount point is `/` or the source starts with `/dev/` (excluding `/dev/loop*`). This drops tmpfs, devtmpfs, overlay (except as `/`), proc and squashfs snaps. Rows are de-duplicated by source (bind mounts keep the shortest mount point), sorted `/` first then by path, and capped at 8 chips. Percent = used / (used + available), like `df`. Unparsable rows are skipped.
- Bounds: lines longer than 4 KiB are dropped, a frame larger than 64 KiB is discarded, and a reply must arrive within 10 s. Three consecutive failures (timeout or bad frame) stop the collector with a reason shown in the bar. Bytes after the cap are drained and discarded so the channel window keeps moving.
- Non-Linux (`uname -s` ≠ `Linux`, or no `/proc/stat`): only host and disks (POSIX `df -P`) are shown. The host chip's tooltip says "CPU, memory, network and uptime need a Linux server (/proc); this server reports <os>". BusyBox Linux gets everything its `/proc` provides.

### M3. Pacing, pause and stop (engine-side, no UI timer)
- Each monitored tab has a **latest-sample slot** in `Shared`, the same pattern as the per-tab output buffer (D6). The collector writes the slot and emits a coalesced `Event::Metrics { tab }` (no payload). The UI takes the sample only while it paints that tab's bar.
- The collector sends the next request when both the interval since the last request has elapsed and the slot has been taken. A hidden tab, a minimized window or an occluded Wayland window paints no bar, so the slot stays full and collection pauses after at most one sample, with no explicit visibility plumbing. When painting resumes, the take wakes the collector, which requests immediately. The first CPU/network delta after a pause spans the gap. It is shown as an average, and the sparkline gets a single point.
- Interval: `Settings.remote_monitoring_interval_secs`, default 2, clamped to 1–30, applied live (`Command::SetMonitorInterval`).
- Stop: the collector future joins the tab's run `select!` next to `follow`, so disconnect, reconnect, tab close and cancellation drop it. Toggle off (`Command::SetMonitoring { tab, enabled: false }`, from × or the SFTP button) drops the channel. In every case stdin closes, the remote `read` sees EOF and the loop exits. Server-side processes left behind are checked in the lab.
- Never started for: protocols other than SSH (Telnet, Serial, Rsh, XDMCP and Mosh have no exec channel), standalone SFTP/FTP sessions, RDP/VNC (including VNC over SSH), sessions whose `remote_command` closes the tab, or tabs where the effective setting is off.

### M4. Channel budget
OpenSSH's `MaxSessions` (default 10) counts session channels per connection. A tab uses the shell, possibly the SFTP subsystem, short-lived `follow` execs, and now one monitoring exec. If the server refuses the channel open, monitoring is marked unavailable ("the server refused an extra channel") and is not retried until the user toggles it or the tab reconnects. The shell and SFTP are never affected because the monitor opens after them.

### M5. Effective setting and commands
- The effective state is decided at connect: the session's `ssh.remote_monitoring` (`Some(bool)`) overrides the global `Settings.remote_monitoring` (default `true`). Later changes to the settings apply to tabs connected afterwards. Live tabs are toggled with the bar's × or the SFTP panel button. Runtime toggles are not persisted (live-toggle semantics are an open question).
- New command ids: `tab.monitoring.toggle` (`Command::SetMonitoring`) and `settings.monitoring.interval` (`Command::SetMonitorInterval`). Both are **GUI-only** in the agent permission matrix (rejected from control channel, MCP and offline CLI). `Event::Metrics` is not forwarded to agents. Rationale: there is no v1 use case, and metrics disclose server state that would otherwise need the content-read flag. A later change can add a read-only `metrics` op gated by content-read.

### M6. Bar UI (`crates/ui/src/monitor_bar.rs`)
- Placement: a bottom strip 26 ± 1 px high inside SSH terminal tabs, between the terminal area and the window status bar. It is full width of the tab area and does not overlap the terminal; the terminal grid resizes (one resize when the bar appears or disappears). Shown only while the effective state is on and the tab is connected.
- Chips, left to right: host (`server-tower`, from `uname -n`, falling back to the session host), CPU (`chip`, `NN %` + a sparkline 48 × 14 px of the last 60 samples), RAM (`ram`, `used / total` in GiB/MiB), upload (`upload`), download (`download`), uptime (`monitor-clock`), then one chip per disk (`hard-disk`, `<mount>: NN %`). A chip is hidden when its value is `None`, except that CPU shows `…` until the second sample.
- Colors: CPU, RAM and disk values use `Tokens::warning` at ≥ 80 % and `Tokens::danger` at ≥ 95 %, otherwise the normal text token. The sparkline uses `accent`. Chip separators use `border`. Everything comes from tokens, so light and dark are complete. A stale sample (no reply for 2 intervals) renders muted.
- Overflow: when the chips exceed the width, `back`/`forward` arrow buttons at the right end scroll by one chip. The close button (`close`, ×) is always visible at the far right and sends `SetMonitoring { enabled: false }`.
- Tooltips: every chip has an accessible name and a tooltip with details (CPU: `user+system NN %`; RAM: exact bytes; network: the interface(s); disk: source, used/size; host: OS and any degradation reason). Keyboard: the bar's buttons are in the tab's focus order after the terminal.
- History (60 CPU values per tab) lives in the UI and is reset on reconnect.

### M7. Settings and SFTP panel button
- Settings › SSH gains a "Remote monitoring" group: checkbox "Show remote monitoring bar on SSH tabs" (default on) and "Refresh interval" (1–30 s, default 2).
- The session editor's SSH advanced options gain a tri-state "Remote monitoring" control (Use global setting / On / Off; default "Use global setting").
- The SFTP panel's "Remote monitoring" button (`monitor-pulse` icon) sends `SetMonitoring` for the attached tab with the inverted state. It renders as pressed/selected while monitoring is active. It is disabled with a tooltip when monitoring is unsupported or unavailable on that connection.

### M8. Testing
- Unit (`engine`): parser fixtures hand-captured from lab containers and written with fictional host names. They cover Ubuntu 24.04, Debian 12, an old kernel without MemAvailable, BusyBox/Alpine, a FreeBSD-like header with only `df`, missing sections, oversized lines, garbage before the header, counter reset and wrap, no default route, bind mounts, and loop/tmpfs/overlay filtering.
- Engine (`FakeLink` gains `exec_stream` with a scripted peer): the request cadence follows the interval, with paused virtual time; no request while the slot is untaken (pause); an immediate request after a take; channel closed on disconnect, toggle off and tab close; three timeouts stop the collector; refused channel open → unavailable without retry; unsupported header → closed; non-SSH protocols never start a collector.
- UI kittest snapshots, light and dark: full bar, warning/danger colors, overflow with arrows, degraded non-Linux bar, SFTP button active and inactive, Settings › SSH group.
- Lab (`#[ignore]`, `cargo xtask lab test`, lab sshd): within 5 s of connecting, the bar's sample has host, CPU (second sample), RAM, net, uptime and disk `/`. After toggle off and after disconnect, a second lab session running `ps -eo args` finds no `rxt-mon` process within 5 s. A refused extra channel (lab sshd profile with `MaxSessions 1`) shows unavailable and leaves the shell working.

## Risks / Trade-offs

- [Visible footprint on the server: `sh` process, exec entries in auth logs] → a fixed, documented, marked command; on by default but easy to switch off globally or per session.
- [`df` stalls on a hung local mount or BusyBox NFS] → the 10 s reply timeout, then stop after 3 failures with a reason; disks are requested at most every 30 s.
- [Login-shell oddities (csh/fish quoting, noisy rc files, restricted shells)] → single-quoted fixed script without `!`, header detection, and "unsupported" instead of guessing.
- [Default-route interface misses multi-homed traffic] → the tooltip names the interface(s). `ponytail:` comment in the parser: per-interface selection if users ask.
- [Over 128 interfaces or 8 disks on big container hosts] → capped by design. The tooltip says "+N more".
- [Settings apply only at connect] → the simpler rule. The open question below may change it.

## Migration Plan

Additive. New settings and the session field have serde defaults (`true`, `2`, `None`), so existing `config.toml` files and stored sessions load unchanged. No database migration. Rollback: older builds ignore the unknown fields.

## Open Questions

- Should the global "Remote monitoring" switch affect already-open tabs live? If so, apply the global setting live to tabs without a runtime override.
- Should disk chips include network mounts (NFS/CIFS) as an option? Excluded in v1 to avoid stalls.
- Read-only agent access (`metrics` op gated by content-read) in a later change?
