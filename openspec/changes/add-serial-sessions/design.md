## Context

`create-native-rust-linux-terminal` D1 reserves `crates/serial` at L2 and requires OS services to go through `crates/platform` contracts (`Paths`, `Clipboard`, `Keyring`, … in `crates/platform/src/lib.rs`) with the Linux adapter in `crates/platform-linux`, enforced by `cargo xtask layers`. Workspace lints forbid `unsafe` in our crates; dependencies may contain audited `unsafe` (tokio already does). The engine consumes terminal protocols through `crates/engine/src/transport.rs` (`Link::shell` → `ShellRead`/`ShellWrite`).

`MobaXterm.ini` type codes: the session type is the first `%` field; SSH=0, RDP=4, VNC=5, FTP=6, SFTP=7 known; **Serial=8 assumed**, to be confirmed with sample files. The serial page has port, speed and an advanced section with data bits, stop bits, parity and flow control.

## Goals / Non-Goals

**Goals:**
- Work with USB-serial (FTDI, CP210x, CH34x, CDC-ACM) and on-board UARTs on any glibc distro without extra system libraries.
- Clear, actionable errors for permissions, busy ports and unplugged devices.
- Fully testable without hardware.

**Non-Goals:** see proposal.

## Decisions

### S1. Linux serial backend: `serialport` (no default features), own sysfs enumeration
Options evaluated:
- **`serialport` 4.x** (MPL-2.0, maintained by the serialport-rs org): termios setup including arbitrary baud via `termios2`, DTR/RTS, break, exclusive access, timeouts. `unsafe` lives inside the crate (nix/libc), not in ours. Its default `libudev` feature links the C libudev; we disable default features so nothing new is linked.
- **`tokio-serial`/`mio-serial`**: async wrappers over `serialport`; add little over a blocking reader on a dedicated thread with a short read timeout, so not needed.
- **`rustix` termios + `tokio::io::unix::AsyncFd`**: pure Rust (linux_raw), safe API for `tcgetattr`/`tcsetattr`/`tcsendbreak`/speed; modem-line ioctls (DTR/RTS) are not exposed as safe calls, so it would leave DTR/RTS unsupported. Kept as fallback if `serialport` licensing (MPL-2.0, file-level copyleft, compatible with distributing our MIT binary unmodified) is rejected.

Decision: `serialport` with `default-features = false` for open/configure/I/O; enumeration is our own sysfs scan in `platform-linux` (`/sys/class/tty/*/device`, plus `idVendor`, `idProduct`, `serial`, `product`, `manufacturer` from the USB parent, and `/dev/serial/by-id` links), written with `std::fs` only. Virtual consoles (`tty0..63`) and ports without a device link are skipped; pty and `/dev/tty*` paths typed by hand are still accepted.

### S2. Platform contract
```text
trait SerialPorts: Send + Sync {
    fn list(&self) -> Result<Vec<PortInfo>>;               // path, by_id, description, usb ids
    fn open(&self, path: &str, cfg: &LineConfig) -> Result<Box<dyn SerialStream>>;
}
trait SerialStream: Send {                                   // blocking, read timeout 100 ms
    fn read(&mut self, buf: &mut [u8]) -> Result<usize>;     // Ok(0) on timeout
    fn write_all(&mut self, bytes: &[u8]) -> Result<()>;
    fn set_lines(&mut self, dtr: Option<bool>, rts: Option<bool>) -> Result<()>;
    fn send_break(&mut self, dur: Duration) -> Result<()>;
    fn reconfigure(&mut self, cfg: &LineConfig) -> Result<()>;
    fn try_clone_writer(&self) -> Result<Box<dyn SerialStream>>;
}
```
Errors are typed (`PermissionDenied { group }`, `Busy`, `NotFound`, `Gone`, `Unsupported`) so the UI never parses OS strings. Ports are opened with exclusive access (`TIOCEXCL` via the backend); a second open reports `Busy`. `core` gains the plain data types (`LineConfig`, `Parity`, `FlowControl`, `StopBits`, `DataBits`); the traits live in `platform`.

### S3. `crates/serial` session logic
Owns a reader thread (`spawn_blocking`) and a writer; maps to `ShellRead`/`ShellWrite` for the engine. Translates Enter per setting (CR / LF / CR LF), optional received-LF → CR LF, optional local echo (written into the output buffer, marked as local). Resize is a no-op. Backpressure: when the session output buffer (D6, 4 MiB) is full the reader stops reading; the kernel/driver buffer then fills and hardware/software flow control (if enabled) throttles the device. Cancellation sets a flag checked after each 100 ms timeout.

### S4. Hot-unplug and reconnect
A read/write returning `EIO`/`ENXIO`/`ENODEV`, or the device path disappearing, ends the stream with `Gone`. The tab shows "Disconnected — device removed", keeps scrollback, and, if "reconnect when the device returns" is on (default on), polls `SerialPorts::list` every 1 s and reopens when the same port reappears, matching by `/dev/serial/by-id` identity when known (USB serial number), otherwise by path. No udev/netlink monitor in v1 (polling is enough at 1 s and needs no new dependency).

### S5. Permissions
On `EACCES` the Linux adapter stats the device and resolves the owning group name from `/etc/group` (std only). The UI message: "Permission denied for /dev/ttyUSB0 (group `dialout`). Add your user to the group (`sudo usermod -aG dialout $USER`) and log out and back in." On Arch-like systems the group is `uucp`; the message always uses the actual group. The app never runs `sudo`, `pkexec` or changes permissions itself. Snap/Flatpak sandboxes are not targets (AppImage/portable only).

### S6. Integration
- `core`: `Protocol::Serial` (no default port; `scheme()` = `serial`, `is_terminal()` = true), `SerialOptions { port, baud, data_bits, parity, stop_bits, flow, enter, lf_to_crlf, local_echo, dtr, rts, reconnect }` with `#[serde(default)]`; host/port fields are hidden in the editor.
- `engine`: `Real::connect` for Serial builds the session from the injected `Arc<dyn SerialPorts>` (wired by `apps/rustxterm` from `platform-linux`); new command ids `serial.send_break`, `serial.set_lines`, `serial.list_ports` registered like every other command (D6), so shortcuts, MultiExec rules and agent control reuse them. Agent effect classification: `serial.list_ports` local read; opening and writing to a serial session remote write (it drives a device).
- `ui`: Serial page with a port combo (refresh button, by-id description, free text allowed), baud combo (300…4 000 000 plus custom), advanced settings, tab context menu entries Send break / DTR / RTS; tab title defaults to `ttyUSB0 @ 115200`.
- Quick connect: `serial:///dev/ttyUSB0?baud=115200&data=8&parity=n&stop=1&flow=none`; omitted parameters use 115200 8N1 no flow control. `serial://ttyUSB0` (no leading slash) is accepted as `/dev/ttyUSB0`.
- Icon: Serial session icon from the bundled set in `assets/icons/app/`; fallback `plug` from the existing catalog.

### S7. Tests without hardware
Unit tests use the fake `SerialPorts` from `platform`. Lab tests (the `#[ignore]` set run by `cargo xtask lab test` in the toolchain container) create a pair with `socat -d -d pty,raw,echo=0,link=/tmp/rxt-ttyA pty,raw,echo=0,link=/tmp/rxt-ttyB`, open A through the real Linux adapter and drive B from the test. Unplug is simulated by killing `socat` (A returns `EIO`), replug by restarting it with the same link. Termios settings are verified by reading back B/A attributes with `stty -F`. Permission errors are tested on a `chmod 000`-style copy under a non-root user in the container. Baud accuracy and flow control on real UARTs are covered by an optional manual checklist (USB-serial loopback adapter), never in CI.

## Risks / Trade-offs

- [ptys ignore baud/parity] → lab verifies configuration via termios read-back; real-wire behavior goes to the manual checklist.
- [`serialport` MPL-2.0] → file-level copyleft only, used unmodified; fallback `rustix` without DTR/RTS documented in S1.
- [Device renumbering on replug (`ttyUSB0` → `ttyUSB1`)] → match by `/dev/serial/by-id` when available.
- [Blocking reader thread per tab] → one thread per open serial tab is acceptable (serial tabs are few); documented.

## Migration Plan

Additive; new session fields default. Rollback: older builds skip sessions with an unknown protocol.

## Open Questions

- Confirm the `MobaXterm.ini` Serial type code (`8`, assumed) and its field layout (port, speed, data bits, parity, stop bits, flow) with sample files.
- Accept `serialport` (MPL-2.0) under the project's license policy, or use the `rustix` fallback without DTR/RTS control?
- Should the default line ending for Enter be CR (most device consoles) or LF?
