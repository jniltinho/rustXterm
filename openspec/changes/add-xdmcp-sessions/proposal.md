## Why

An XDMCP session shows the graphical login of a remote Unix host (lightdm, gdm, xdm) and the full remote X desktop through a local X server. Some labs, thin-client setups and legacy Unix workstations are still reached this way. rustXterm v0.1 excludes XDMCP and a built-in X server (`create-native-rust-linux-terminal` non-goals). XDMCP cannot work without a local X server that the remote display manager connects back to, so the question is whether a small, honest version is possible without writing an X server. This change proposes the smallest viable approach: launch an **external** X server that also speaks VNC (TigerVNC `Xvnc`), point it at the remote host with `-query`, and show it inside a tab through the existing embedded VNC client. Behavior follows the public XDMCP specification.

## What Changes

- New protocol crate `crates/xdmcp` (L2): XDMCP `Query`/`Willing`/`Unwilling` probe over UDP (sans-IO packet codec, bounded), display-number allocation and an `Xvnc` launch plan (arguments, private Unix socket for RFB, lifetime tied to the tab). It does not implement an X server.
- New platform contract for locating and supervising an external X server process (`XServerLauncher`), Linux adapter in `crates/platform-linux` (finds `Xvnc` on `PATH`, allocates a free display, starts and kills the process group, reports exit).
- The engine composes the session: probe → launch `Xvnc -query <host> -once -listen tcp -rfbunixpath <private socket> -SecurityTypes None …` → connect the existing `rxt_vnc` client to the private Unix socket → render in a remote-desktop tab (same framebuffer widget, input, scaling, limits as VNC).
- Works the same on X11 and Wayland desktops (Xvnc is headless; nothing is drawn by the host display server outside our window).
- Strong security labelling: XDMCP (UDP 177) and the X11 connection back to us are unencrypted; the tab shows the "Unencrypted" marker and the editor recommends use only on trusted networks, VMs or a VPN, or using SSH X11 forwarding/RDP/VNC instead.
- New `Protocol::Xdmcp`, session editor page, the existing `xdmcp` icon from `assets/icons/app/`, quick connect `xdmcp://host[:port]`.
- MobaXterm.ini import of XDMCP bookmarks.
- Lab: an `xdm` container with XDMCP enabled for a fictional user.

This change depends on and comes after `create-native-rust-linux-terminal` (it reuses the VNC client, framebuffer widget and remote-desktop limits).

### Non-goals

A built-in X server; bundling Xvnc in the AppImage/tarball; Xephyr or other nested-window modes; XDMCP broadcast and indirect (chooser) queries; tunnelling XDMCP through SSH; multi-monitor, audio, rootless/multiwindow X; using the launched X server for SSH X11 forwarding.

## Capabilities

### New Capabilities

- `xdmcp-sessions`: XDMCP query probe, external X server lifecycle, embedded display, plaintext warnings and failure reporting.

### Modified Capabilities

- `desktop-shell`: quick connect accepts `xdmcp://` (added requirement).
- `session-management`: session editor offers the XDMCP type; MobaXterm.ini import maps XDMCP bookmarks (added requirements).

## Impact

- New crate `crates/xdmcp`; `crates/platform` (`XServerLauncher` contract + fake); `crates/platform-linux` (process supervision, display allocation); `cargo xtask layers` updated (the xdmcp crate does not depend on `rxt_vnc`; the engine composes them).
- `crates/core` (`Protocol::Xdmcp`, `XdmcpOptions`), `crates/engine` (composition, cleanup on tab close/crash), `crates/ui` (editor page, missing-Xvnc dialog, marker), `crates/store` (importer).
- Runtime requirement, not bundled: TigerVNC `Xvnc` (packages `tigervnc-standalone-server` on Debian/Ubuntu, `tigervnc-server` on Fedora, `tigervnc` on Arch). Without it the session type shows an install hint and does not start.
- Lab: `xdm` service and `tigervnc-standalone-server` in the toolchain image for lab tests.
