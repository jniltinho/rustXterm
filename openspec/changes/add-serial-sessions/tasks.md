Depends on `create-native-rust-linux-terminal` being implemented (platform seam, engine transport seam, terminal widget, store, lab runner).

## 1. Serial milestone (code)

- [x] 1.1 Decide the backend per design S1 (license check of `serialport` with `cargo deny`; fallback `rustix`); pin it with default features off; verify `cargo tree` shows no libudev/C dependency and `cargo deny check` passes
- [x] 1.2 `core` line-configuration types and `platform` `SerialPorts`/`SerialStream` contract with typed errors and a fake implementation (S2); verify serde round-trip and contract unit tests
- [x] 1.3 `platform-linux`: sysfs enumeration with by-id and USB metadata, exclusive open, configuration, DTR/RTS, break, error mapping including owning-group lookup (S1, S5); verify unit tests on a fixture sysfs tree and `/etc/group` parsing
- [x] 1.4 Create `crates/serial` (L2, registered in `cargo xtask layers`): reader thread with timeout-based cancellation, Enter translation, LF handling, local echo, reconnect-on-replug matcher (S3, S4); verify unit tests against the fake platform, including unplug and replug by-id
- [x] 1.5 `core` `Protocol::Serial` and `SerialOptions`; `engine` serial branch and command ids `serial.send_break`, `serial.set_lines`, `serial.list_ports` with agent effect classification; verify engine unit tests with the in-memory transport and MultiExec on a serial tab
- [x] 1.6 `ui`: Serial editor page with port picker and refresh, advanced settings, tab menu (break, DTR, RTS), disconnected/device-removed state, permission error dialog, quick connect `serial://`, icon (from `assets/icons/app/`, or `plug` fallback); verify kittest snapshots in light and dark
- [ ] 1.7 `store`: MobaXterm.ini Serial import after confirming type code `8` and field layout with sample files (fixtures hand-written, fictional); verify importer unit tests
- [x] 1.8 Add `socat` to the Docker toolchain image for virtual serial pairs (S7); verify `docker build --target toolchain` and `socat -V` in the image

## 2. Serial verification batch and push

- [x] 2.1 Lab tests over socat pty pairs: open/echo both directions, 8N1 and 7E1 read back with `stty -F`, Enter variants, local echo, break and DTR/RTS calls succeed, busy port reported, permission denied names the group (non-root user), unplug shows device removed and replug reconnects; add names to `lab/expected-tests.txt`; verify `cargo xtask lab test` green
- [x] 2.2 UI tests: `serial://` parsing (defaults, all parameters, `serial://ttyUSB0`, invalid baud/parity), port picker refresh, error dialogs; verify green
- [ ] 2.3 Manual hardware checklist (USB-serial adapter with TX/RX loopback: 9600 and 115200, RTS/CTS) recorded in `docs/gates/serial.md`; verify the receipt exists
- [x] 2.4 `cargo xtask prints` for the Serial editor page and a Serial tab in both themes; verify `git status` shows no `docs/prints/` files
- [x] 2.5 Full gates in Docker, `cargo xtask guard`, then commit and push with the task ids; verify the commit is on the remote
