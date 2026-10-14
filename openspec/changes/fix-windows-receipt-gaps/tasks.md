## 1. Agent control

- [x] 1.1 F2: portable no-echo master-password prompt in `crates/agent` (rpassword or platform method), echo restored on error/Ctrl+C; test on Linux pty; `cargo deny` clean
- [x] 1.2 F1: live detection without probing by opening the endpoint; busy channel retried up to 2 s; only "absent" → `not_running`; MCP test with pipelined `send_input` + `wait_for_output`
- [x] 1.3 F1: Windows pipe server keeps a spare listening instance while serving; adapter test with two concurrent clients
- [x] 1.4 F5: control servers send a structured `denied` reason before closing a refused client (Linux and Windows); CLI prints it with the denied exit code; elevation case uses the different-elevation sentence in both directions; tests for each refusal reason

## 2. Desktop integration

- [x] 2.1 F3: notifications on transfer done/failed and unexpected disconnect while unfocused; `notifications` setting (Settings → General, default on); never block or show errors; UI tests with a fake notifier
- [x] 2.2 F4: transfer row "Open" / "Show in folder" after a download; local pane "Open" for files; opener errors shown; UI tests with a fake opener
- [x] 2.3 Per-tab transfer lists; test that an FTP tab's upload is absent from an SSH tab's SFTP panel
- [x] 2.4 Dialog start folder: last used per action, else Downloads/home; `FileDialogs` takes a start dir; Linux and Windows adapters
- [x] 2.5 Windows serial: quick connect `serial://COM<n>` and `\\.\COM<n>`; platform placeholder (`COM3` / `/dev/ttyUSB0`)
- [x] 2.6 Light/dark snapshots for the new controls (`make snapshots`), `make check` green

## 3. Verification

- [ ] 3.1 Windows clippy (`--target x86_64-pc-windows-gnu`) and the CI `windows-test` job green
- [ ] 3.2 New Windows receipt on the Windows Server 2022 test VM for the release commit: W19 (pipelined), W25, W27, W28, W30, W39 PASS; W17 on a USB-serial adapter or waived by the owner in the receipt; `cargo xtask windows-receipt --check dist/<version>` passes
- [x] 3.3 Document the Server 2022 `DisableMsi` policy note (standard-user MSI install) in the release notes / docs
