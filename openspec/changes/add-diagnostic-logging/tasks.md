## 1. Diagnostics (code)

- [x] 1.1 `store`: `diagnostics` settings (level, protocols, folder) with defaults and round trip; the stream switch is not persisted (D3)
- [x] 1.2 `apps/rustxterm`: always-registered file layer with a reloadable filter and a swappable writer slot (D1); unit test that every level keeps the secret exclusions; Off → Full debug → Off during a connection
- [x] 1.3 Folder resolution, `rustxterm-logs/` subfolder, safe open (D2) and rotation (D4) shared with `--debug-log`; (Windows: inherited permissions, ACL left as a follow-up); unit tests for the fallback, planted links and the `.1`-`.3` rotation
- [x] 1.3a Bounded background writer (D3a): drop counting, write-error notice, ordered rotation/clear/folder change
- [x] 1.4 Stream tap after authentication on SSH, Telnet, rlogin/rsh and Serial (above Serial's local echo), RDP/VNC message headers; no tap on local terminals (D3)
- [x] 1.5 `ui`: Diagnostics page (level, protocols, folder, open/clear buttons, consent dialog for the stream, include-input warning), status-bar chip; kittest snapshots light and dark

## 2. Verification and push

- [ ] 2.1 Lab: SSH (saved password), Telnet (auto-login), Serial (local echo) with stream on/off and input on/off, MultiExec and paste; the secret scan over the stream files and a Protocol trace log (SSH password, RDP NLA on the Windows VM); full queue with a slow writer. Done so far: engine test `stream_capture_records_server_output_but_not_typed_passwords`, serial `local_echo_is_tagged_apart_from_device_output`, sink drop counting, RDP NLA debug-log scan on the Windows VM (0.6.2)
- [ ] 2.1a Native receipts on Windows (VM) and macOS (CI runner or a Mac): folder fallback, ACL, rotation
- [x] 2.2 Diagnostics page snapshots `settings_diagnostics(_recording)_{light,dark}` (kittest); docs (README, docs/rdp.md)
- [x] 2.3 `make check`, `cargo xtask guard`, Windows-target clippy; commit and push with the task ids
