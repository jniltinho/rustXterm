## Why

Many of the people who manage servers from a laptop use a Mac with Apple Silicon, so a native rustXterm for macOS fills a real gap. The platform seam built for Linux and proven by `add-windows-port` (one adapter crate per OS, chosen in `apps/rustxterm/src/os.rs`) makes this a bounded change: protocol, engine and UI crates stay untouched.

## What Changes

- New crate `crates/platform-macos` (L1) implementing every platform contract for macOS 13 Ventura or later on Apple Silicon (`aarch64-apple-darwin`): per-user folders (`~/Library/Application Support`, `~/Library/Caches`, `~/Library/Logs`), theme following (light/dark appearance), clipboard, notifications, native file dialogs, Keychain for the optional keyring wrap, the Unix-socket agent control channel restricted to the same user (peer credentials through the standard library, no `unsafe`), `open`-style opener, and serial ports (`/dev/cu.*`).
- No `unsafe` code: the workspace `unsafe_code = "forbid"` stays in force for the new crate (the Windows exception is not extended).
- The app picks the macOS adapter on `target_os = "macos"`; Unix code paths already gated with `cfg(unix)` (file modes 0600/0700, SSH agent socket, X11 over Unix sockets) work unchanged; Linux-only tests stay gated.
- **Build exception (user decision required):** macOS binaries cannot be built in Docker (the Apple SDK may only be used on Apple hardware), so macOS compilation, linting and tests run natively on a GitHub-hosted macOS arm64 runner. The Linux and Windows builds stay Docker-only, and nothing is installed on developer desktops.
- Release artifacts for macOS: `rustxterm.app` in a `.dmg` and a `.tar.gz`, ad-hoc signed by default; Developer ID signing and notarization when the corresponding secrets exist. Like Windows, the first macOS release is gated on a receipt from a real Mac.
- CI: a `macos` job builds and tests on the runner (unit tests, UI snapshot tests, `--self-test`); the Docker lab is not available there, so protocol lab tests stay Linux-only.

### Non-goals

Intel Macs (x86_64; can be added later as a universal binary), macOS 12 or older, Mac App Store and sandboxing, local shell (phase 2), built-in X server (XQuartz is used when present), Homebrew cask (later).

### Blocker

GitHub-hosted macOS runners for this private repository are currently refused by the account's Actions billing ("recent account payments have failed or your spending limit needs to be increased"); a feasibility probe on the `macos-probe` branch could not start. The CI tasks need that resolved (budget for macOS minutes, or a public repository) or a self-hosted Mac runner.

## Capabilities

### New Capabilities

- `macos-platform`: macOS adapter behavior — supported versions, per-user folders, theme following, notifications, file dialogs, opener, serial port naming, and the agent control channel on macOS.

### Modified Capabilities

- `reproducible-distribution`: macOS native build on a hosted runner as an exception to the Docker-only build (modified), macOS release artifacts, signing and verification (added).
- `session-management`: keyring wrap stored in the macOS Keychain (added).

## Impact

- New: `crates/platform-macos`; macOS-only dependency `security-framework` (Keychain, safe API) plus the already-used `arboard`, `rfd`, `notify-rust`, `serialport`, `dirs`, `tokio` as target-specific dependencies of the adapter.
- Changed: `apps/rustxterm` (adapter selection in `os.rs`, `.app` bundle metadata and icon), `xtask` (layers know the new crate; `dist-check` learns the macOS artifacts), `.github/workflows/{ci,release}.yml` (macOS jobs), `packaging/macos/` (Info.plist, dmg script), `deny.toml` (macOS triple), `rust-toolchain.toml` (target list).
- Unchanged: `core`, protocol crates, `engine`, `agent`, `ui` logic, store format.
- Depends on `create-native-rust-linux-terminal` (archived). Independent of `add-windows-port`, except that both modify the Docker-only build requirement (the text below includes the Windows exception).
