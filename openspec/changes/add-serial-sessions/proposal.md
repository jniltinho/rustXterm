## Why

Network engineers and embedded developers use serial sessions to reach consoles of switches, routers, single-board computers and microcontrollers over USB-serial adapters. On Linux this is usually done with `screen`, `minicom` or `picocom`, which lack a session tree, tabs, logging UI and MultiExec. `create-native-rust-linux-terminal` lists Serial for phase 2; this change adds it as a native session type behind the platform seam so macOS/Windows adapters can follow.

## What Changes

- New protocol crate `crates/serial` (L2): serial session logic (settings model, line-ending translation, local echo, break, reconnect after replug) over a byte stream provided by the platform contract. No OS calls in the crate.
- New contract in `crates/platform`: serial port enumeration (path, description, USB vendor/product/serial when known) and opening a configured port as a byte stream; Linux implementation in `crates/platform-linux` on a maintained pure-Rust-API crate (see design S1), with no `unsafe` in our crates.
- Settings per session: port, baud rate (standard list plus custom), data bits (5–8), parity (none/odd/even/mark/space), stop bits (1/2), flow control (none/RTS-CTS/XON-XOFF), Enter sends (CR, LF, CR LF), received-LF handling, local echo, initial DTR/RTS state.
- Commands: send break, toggle DTR/RTS, list ports (agent-visible read).
- Hot-unplug: the tab shows "disconnected (device removed)", keeps scrollback, and can reconnect automatically when the same device reappears.
- Permission errors explain the owning group (`dialout`, `uucp`, …) and how to join it; the app never escalates privileges.
- New `Protocol::Serial` (no network port), session editor page with a port picker, icon from the `assets/icons/app/` catalog, quick connect `serial:///dev/ttyUSB0?baud=115200`.
- Reuse: terminal widget, MultiExec, session logging, close confirmation.
- MobaXterm.ini import of Serial bookmarks.
- Tests: virtual serial pairs (`socat` pty pairs) in the toolchain container; no real hardware required in CI.

This change depends on and comes after `create-native-rust-linux-terminal`.

### Non-goals

File transfer protocols over serial (XMODEM/YMODEM/ZMODEM), RFC 2217 / serial-over-TCP server mode, modem dialing (AT command scripts), hex view/hex send, Bluetooth RFCOMM pairing UI, macOS/Windows adapters (only the contract).

## Capabilities

### New Capabilities

- `serial-sessions`: serial port discovery, configuration, terminal I/O, line control, hot-unplug and permission handling.

### Modified Capabilities

- `desktop-shell`: quick connect accepts `serial://` URLs (added requirement).
- `session-management`: session editor offers the Serial type; MobaXterm.ini import maps Serial bookmarks (added requirements).

## Impact

- New crate `crates/serial`; `crates/platform` (new `SerialPorts` trait, fake implementation for tests); `crates/platform-linux` (sysfs enumeration, port opening); `cargo xtask layers` updated.
- New dependency (design S1): `serialport` 4.x with default features off (no libudev C library), or `rustix` termios as the fallback; both permissive licenses, checked by `cargo deny`.
- `crates/core` (`Protocol::Serial`, `SerialOptions` with serde defaults), `crates/engine` (serial branch in the transport seam), `crates/ui` (editor page, port picker, quick connect, icon), `crates/store` (importer).
- Docker toolchain image gains `socat` for virtual serial tests; `lab/expected-tests.txt` gains serial tests.
- AppImage/portable binary: no new runtime library (no libudev link).
