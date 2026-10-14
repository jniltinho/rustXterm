## Context

XDMCP (X Display Manager Control Protocol, X.Org specification) works backwards from what users expect: the local **X server** sends `Query` to the remote display manager on UDP 177; if `Willing`, it sends `Request`/`Manage`; the display manager then opens an ordinary X11 connection **back** to the local X server (TCP 6000+N) and runs the greeter and the user's session on it, authorized with an MIT-MAGIC-COOKIE-1 negotiated in the `Accept` packet. So XDMCP needs a local X server reachable over TCP from the remote host. Some terminal tools ship their own X server for this; `create-native-rust-linux-terminal` lists "built-in X server" and XDMCP as non-goals.

Existing pieces to reuse: `crates/vnc` (vnc-rs over any `AsyncRead + AsyncWrite`, with the D13 limits: 8192² max, decompressed-rect cap, 64 MiB queue, 30 s stall timeout, cancel on drop), the framebuffer widget with keyboard grab/release, `Protocol::Vnc` editor pages, and the `xdmcp`/`xserver` icons already present in `assets/icons/app/` (catalog entries `terminals/xdmcp`, `terminals/xserver`).

`MobaXterm.ini` type codes: type is the first `%` field; SSH=0, RDP=4, VNC=5, FTP=6, SFTP=7 known; **Xdmcp=3 assumed**, to be confirmed with sample files.

## Goals / Non-Goals

**Goals:**
- Smallest viable XDMCP session that runs embedded in a tab on both X11 and Wayland desktops.
- No X server code in rustXterm and no new `unsafe`.
- Honest failure messages: host unreachable / unwilling / Xvnc missing / display manager never connected.

**Non-Goals:** see proposal.

## Decisions

### X1. Approach: external `Xvnc` + embedded VNC viewer (chosen)
Options evaluated:

| Option | How | X11 desktop | Wayland desktop | Embedded in tab | Verdict |
|---|---|---|---|---|---|
| (a) Xephyr | `Xephyr :N -query host` opens its own nested window | yes | only via XWayland, separate window | no (egui/winit cannot reparent foreign windows; XEmbed is X11-only and unsupported) | rejected: separate window we can only "track", not embed; inconsistent on Wayland |
| (b) Xvfb + x11vnc | headless X server + separate VNC exporter, viewed with `rxt_vnc` | yes | yes | yes | works but two external processes and two packages |
| (b') **TigerVNC `Xvnc`** | one headless X server with built-in RFB, `-query host`, RFB on a private Unix socket, viewed with `rxt_vnc` | yes | yes | yes | **chosen**: one process, one package, reuses the whole VNC stack and its limits |
| (c) built-in X server | implement X11 | — | — | — | out of scope (v0.1 non-goal, enormous) |
| (d) defer | no XDMCP | — | — | — | fallback if X3's open questions fail |

Launch command (constructed by `crates/xdmcp`, executed by the platform adapter):
```text
Xvnc :N -query <host> -port <udp port, default 177> -once -listen tcp
     -geometry <WxH> -depth 24 -rfbport -1 -rfbunixpath <runtime dir>/xdmcp-<id>.sock -rfbunixmode 0600
     -SecurityTypes None -AlwaysShared=0 -NeverShared=1
```
`-rfbport -1` disables RFB over TCP; the RFB socket lives in `$XDG_RUNTIME_DIR/rustxterm/` (0700), so `SecurityTypes None` is safe for the same-user viewer. `-once` ends the server when the remote session ends. `-listen tcp` is required because the remote display manager connects back over TCP; X11 access is then limited to holders of the XDMCP-negotiated cookie. Minimum TigerVNC version (for `-rfbunixpath`) is checked at launch from `Xvnc -version`.

### X2. `crates/xdmcp` scope
- Sans-IO XDMCP v1 codec for `Query` (opcode 2), `Willing` (5) and `Unwilling` (6) only, with a 1 KiB datagram cap, used by a **probe** before launching: up to 3 `Query` datagrams 1 s apart; `Willing` → launch, `Unwilling` → show the server's status string, silence → "no XDMCP answer from host (UDP 177 blocked or XDMCP disabled)". The full Request/Accept/Manage exchange stays inside Xvnc.
- Display allocation plan: lowest free N from 20 upwards with no `/tmp/.X11-unix/X<N>`, no `/tmp/.X<N>-lock` and TCP port 6000+N free; retry with the next N if Xvnc exits with "server already active".
- Readiness: wait (≤ 10 s) for the RFB Unix socket to accept; then the engine connects `rxt_vnc` to it.
- "Display manager never connected": if no X client connects within the session timeout (default 30 s, detected by the Linux adapter as no ESTABLISHED TCP connection on local port 6000+N in `/proc/net/tcp`/`tcp6`, read with `std::fs`), the tab shows a hint about firewalls blocking TCP 6000+N from the remote host and about the display manager's access list (`Xaccess`/`[XDMCPServer]`).

### X3. Platform contract
```text
trait XServerLauncher: Send + Sync {
    fn find(&self) -> Result<XServerInfo>;                       // path + version, or NotInstalled
    fn free_display(&self, from: u16) -> Result<u16>;
    fn spawn(&self, args: &[String], log: &Path) -> Result<Box<dyn XServerProcess>>;
    fn has_tcp_client(&self, display: u16) -> Result<bool>;      // remote DM connected back
}
trait XServerProcess: Send { fn wait_exit(&mut self) -> BoxFut<'_, ExitInfo>; fn kill(&mut self); }
```
The Linux adapter spawns with `std::process::Command` in its own process group, captures stderr to a bounded per-session log in the cache dir, and kills the group on tab close, app exit and engine panic guard. A crash of Xvnc shows the last log lines in the tab error.

### X4. Integration
- `core`: `Protocol::Xdmcp` (default port 177, scheme `xdmcp`, `is_terminal() = false`), `XdmcpOptions { geometry, depth, timeout_s }`.
- `engine`: `Real::connect_desktop` for Xdmcp = probe → launch → wait socket → `rxt_vnc` over `UnixStream` → `DesktopPair`; dropping the pair kills Xvnc. Hidden tabs stop requesting VNC updates as for VNC tabs.
- `ui`: XDMCP editor page (host, UDP port, resolution, color depth, security warning), a "Xvnc not found" dialog with per-distro package names, remote-desktop tab with the existing `xdmcp` icon and the Unencrypted marker, resize = restart prompt (Xvnc geometry is fixed per session in v1).
- Agent control: opening an XDMCP session is classified like VNC (remote write); the framebuffer is never exposed to agents, as for RDP/VNC.

### X5. Security posture
XDMCP packets and the X11 stream (keystrokes, passwords typed into the greeter, screen contents) are unencrypted, and the local X server must accept TCP connections from the remote host. Mitigations: plaintext warning in the editor and a first-connect confirmation per session; Unencrypted marker; X access only with the XDMCP cookie (no `-ac`); RFB never on TCP; Xvnc killed with the tab. Recommended use: loopback, local VMs, isolated lab networks or a VPN. XDMCP over SSH is not offered: XDMCP is UDP and the X connection goes in the reverse direction, so it does not map onto an SSH local forward; users needing encryption should use SSH X11 forwarding, RDP or VNC-over-SSH instead.

### X6. Lab
`xdmcp` service: Debian with `xdm` (smallest XDMCP display manager), `DisplayManager.requestPort: 177`, `Xaccess` allowing only the lab bridge subnet, a fictional user `alice` / `lab-alice-pass` and `xterm` as session. The service publishes no host port: the lab test runs in the toolchain container on host networking, queries the container's bridge address, and xdm connects back to the bridge gateway address on TCP 6000+N. The toolchain image gains `tigervnc-standalone-server` for these tests. Tests: probe Willing, probe silence (stopped xdm), Unwilling (`Xaccess` deny host), greeter frame appears (non-black framebuffer within 20 s), login via VNC keystrokes reaches the `xterm` session, tab close kills Xvnc (no process left), Xvnc missing → dialog.

## Risks / Trade-offs

- [External runtime dependency] → explicit detection, install hint, version check; nothing silently falls back to another client.
- [Firewalls on the client host block 6000+N from the remote host] → specific hint after the timeout; documented.
- [Xvnc versions without `-rfbunixpath`] → minimum version check; fallback (open question) is RFB on 127.0.0.1 with a random VncAuth password.
- [Fixed geometry] → accepted for v1; restart to change size.
- [Lab X server listens on TCP during tests] → only inside the toolchain container on the lab host, only for the test duration, cookie-protected.

## Migration Plan

Additive. Rollback: older builds skip sessions with an unknown protocol.

## Open Questions

- Confirm the `MobaXterm.ini` XDMCP type code (`3`, assumed) and field layout with sample files.
- Is the `xdm` lab container's connect-back to the toolchain container over the bridge gateway reliable on all CI hosts? If not, run xdm and the test in one container.
- Should Xvnc be bundled in the AppImage later (size, licensing GPL-2.0 as a separate executable), or always remain a system dependency?
- If the project rejects any external runtime helper, this change is deferred (option d) rather than adding a built-in X server.
