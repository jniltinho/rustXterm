## Why

Many people who manage remote servers work from Windows; rustXterm ships only for Linux. `create-native-rust-linux-terminal` built the platform seam (`crates/platform` contracts, one Linux adapter, layer check) so that macOS/Windows adapters could be added without touching protocol, engine or UI crates. The Linux release (0.2.0) is stable enough to test that promise with a Windows adapter and a Windows release.

## What Changes

- New crate `crates/platform-windows` (L1) implementing every platform contract for Windows 10 1809+, Windows 11 and Windows Server 2019+ (desktop experience) on x86_64: Known Folder paths, theme from the registry with live updates, clipboard, toast notifications, Win32 file dialogs parented to the main window, Windows Credential Manager keyring, a named-pipe agent control transport restricted to the current user, a ShellExecute-style opener, and COM-port serial enumeration and I/O.
- **User decision (unsafe policy):** `unsafe` is allowed only in `crates/platform-windows` (direct Win32 calls through the `windows` crate). That crate alone overrides the workspace `unsafe_code = "forbid"`; every `unsafe` block carries a `// SAFETY:` comment; clippy `undocumented_unsafe_blocks` is denied there; `cargo xtask layers` fails if any other crate relaxes the lint.
- Small `#[cfg]` changes outside the adapter where code is Unix-specific today: file modes 0600/0700 in `store` and the agent workspace (Windows uses a verified owner-only DACL applied by the platform, and rejects network and FAT/exFAT data directories), SSH agent auth/forwarding (Windows OpenSSH agent pipe, then Pageant), X11 forwarding (TCP display only; reported unavailable without a local X server), Unix-only tests gated.
- The app picks the adapter per target. On Windows two executables are built from the same code: `rustxterm.exe` (GUI subsystem, no console window) and `rustxterm-cli.exe` (console subsystem: CLI subcommands, `mcp`, `--self-test`).
- Docker-only cross build for `x86_64-pc-windows-gnu` (pinned mingw-w64 from the existing Ubuntu snapshot, pinned Rust std for the target); no Windows build host and no MSVC SDK download.
- Release artifacts for Windows: portable `.zip` and a per-user `.msi`, reproducible when unsigned, with SHA-256 checksums; optional Authenticode signing of the exes and the MSI when a certificate secret exists; winget manifest later. The first Windows release is gated on a passing VM receipt.
- CI: Windows test binaries are cross-compiled in Docker and executed on a `windows-latest` runner (no toolchain installed there); the Docker lab stays Linux-only, so lab end-to-end runs by hand with a receipt from the existing local Windows Server 2022 VM (`lab/vagrant/windows`, user decision; a Windows 11 VM is optional later).
- `cargo xtask layers`, `guard`, `dist-check` and `deny.toml` learn about the Windows target and artifacts.

### Non-goals

aarch64 Windows (later, same design), Windows 7/8.1, local shell/ConPTY (delivered by `add-local-shell`), built-in X server, MSIX/Microsoft Store, portable "data next to the exe" mode, macOS.

## Capabilities

### New Capabilities

- `windows-platform`: Windows adapter behavior — per-user folders, theme following, notifications, file dialogs, opener, owner-only data protection, and the confinement of `unsafe` code.

### Modified Capabilities

- `reproducible-distribution`: Windows cross build in Docker, Windows artifacts, Windows CI and verification (added requirements); the Docker-only build and quality-gate requirements are modified for native execution of Windows test binaries and the adapter-only `unsafe` exception.
- `agent-control`: named-pipe control channel restricted to the current user; console CLI executable on Windows (added requirements).
- `session-management`: keyring wrap stored in Windows Credential Manager (added requirement); the encrypted-database requirement is modified (owner-only ACL instead of mode 0600 on Windows).
- `ssh-sessions`: agent authentication/forwarding and X11 forwarding on Windows (added requirement).
- `terminal-emulation`: terminals on Windows; the local terminal runs through ConPTY as `add-local-shell` specifies (added requirement).
- `serial-sessions`: COM port discovery, naming and errors on Windows (added requirement; from `add-serial-sessions`).
- `desktop-shell`: Windows theme following, native dialogs and notifications (added requirement).

## Impact

- New: `crates/platform-windows`; Windows-only dependencies `windows` (Win32 bindings), plus the already-used `arboard`, `rfd`, `notify-rust`, `serialport`, `dirs` as target-specific dependencies of the adapter.
- Changed: `apps/rustxterm` (per-target adapter selection, second binary, Windows resources: icon, version info, manifest), `crates/platform` (doc of the dialog parent id; one `Paths` method to make a directory private), `crates/store` and `crates/engine` (cfg around Unix modes), `crates/ssh` (agent and X11 cfg), Unix-only tests gated, `xtask` (layers, guard, dist-check), `Dockerfile` (mingw-w64, Windows Rust std, msitools, osslsigncode, Windows dist stage), `.github/workflows/{ci,release}.yml`, `deny.toml` (Windows triple), new `docker/rust-components-windows.sha256`, `scripts/cargo-docker.sh` (`RXT_TOOLCHAIN_STAGE=windows-toolchain`).
- Unchanged: `core`, protocol crates (`ssh` aside from the cfg above, `sftp`, `ftp`, `rdp`, `vnc`, `telnet`, `serial`), `engine` logic, `agent`, `ui` logic, store format (databases move between OSes with export/import as today).
- Depends on `create-native-rust-linux-terminal` (implemented) and, for the serial delta, `add-serial-sessions`.
