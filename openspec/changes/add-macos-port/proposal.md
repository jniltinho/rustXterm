## Why

Many of the people who manage servers from a laptop use a Mac with Apple Silicon, so a native rustXterm for macOS fills a real gap. The platform seam built for Linux and proven by `add-windows-port` (one adapter crate per OS, chosen in `apps/rustxterm/src/os.rs`) makes this a bounded change: protocol, engine and UI crates stay untouched.

## What Changes

- New crate `crates/platform-macos` (L1) implementing every platform contract for macOS 13 Ventura or later on Apple Silicon (`aarch64-apple-darwin`): per-user folders (`~/Library/Application Support`, `~/Library/Caches`, `~/Library/Logs`), theme following (light/dark appearance), clipboard, notifications, native file dialogs, Keychain for the optional keyring wrap, the Unix-socket agent control channel restricted to the same user (peer credentials through the standard library, no `unsafe`), `open`-style opener, and serial ports (`/dev/cu.*`).
- No `unsafe` code: the workspace `unsafe_code = "forbid"` stays in force for the new crate (the Windows exception is not extended).
- The app picks the macOS adapter on `target_os = "macos"`; Unix code paths already gated with `cfg(unix)` (file modes 0600/0700, SSH agent socket, X11 over Unix sockets) work unchanged; Linux-only tests stay gated.
- **Build exception (user decision required):** macOS binaries cannot be built in Docker (the Apple SDK may only be used on Apple hardware), so macOS compilation, linting and tests run natively on macOS. Two hosts are supported, and nothing is installed on developer desktops; the Linux and Windows builds stay Docker-only:
  - **A disposable local macOS KVM VM** (the default for development, mirroring the Windows Vagrant VM under `lab/`), built on [osx-kvm](https://github.com/kholia/osx-kvm) (`boot-macOS-headless.sh`) or [ultimate-macOS-kvm](https://github.com/Coopydood/ultimate-macOS-kvm); the macOS target's `cargo build`/`clippy`/`test`/bundle run inside the VM over SSH, as the Windows receipt runs in its VM. This removes the dependency on paid GitHub macOS minutes.
  - **A hosted macOS arm64 runner** for CI on `main`/tags when available (an alternative, not required to develop).
- Release artifacts for macOS: `rustxterm.app` in a `.dmg` and a `.tar.gz`, ad-hoc signed by default; Developer ID signing and notarization when the corresponding secrets exist. Like Windows, the first macOS release is gated on a receipt from a real Mac.
- CI: a `macos` job builds and tests on the runner (unit tests, UI snapshot tests, `--self-test`); the Docker lab is not available there, so protocol lab tests stay Linux-only.

### Non-goals

Intel Macs (x86_64; can be added later as a universal binary), macOS 12 or older, Mac App Store and sandboxing, local shell (phase 2), built-in X server (XQuartz is used when present), Homebrew cask (later).

### Compliance note

Apple's macOS software licence permits running macOS only on Apple-branded hardware. The local KVM VM (osx-kvm / ultimate-macOS-kvm) is a widely-used developer convenience but falls outside that licence on non-Apple hardware; using it is the operator's decision and responsibility, and it is never part of CI or distributed artifacts. The licence-clean paths are a hosted macOS runner or a real Mac; the end-to-end release receipt (below) still requires a real Mac.

### Prior blocker (now optional)

GitHub-hosted macOS runners for this private repository were refused by the account's Actions billing. With the local KVM VM as the default build/test host this no longer blocks development; the hosted runner stays an optional CI path once billing is resolved or the repository is public.

## Capabilities

### New Capabilities

- `macos-platform`: macOS adapter behavior — supported versions, per-user folders, theme following, notifications, file dialogs, opener, serial port naming, and the agent control channel on macOS.

### Modified Capabilities

- `reproducible-distribution`: macOS native build on a hosted runner or a local macOS KVM VM as an exception to the Docker-only build (modified), macOS release artifacts, signing and verification (added).
- `session-management`: keyring wrap stored in the macOS Keychain (added).

## Impact

- New: `crates/platform-macos`; macOS-only dependency `security-framework` (Keychain, safe API) plus the already-used `arboard`, `rfd`, `notify-rust`, `serialport`, `dirs`, `tokio` as target-specific dependencies of the adapter.
- Changed: `apps/rustxterm` (adapter selection in `os.rs`, `.app` bundle metadata and icon), `xtask` (layers know the new crate; `dist-check` learns the macOS artifacts), `.github/workflows/{ci,release}.yml` (optional macOS job), `packaging/macos/` (Info.plist, dmg script), `lab/macos/` (the KVM VM recipe and a driver like `lab/vagrant/windows/rxt.sh`), `deny.toml` (macOS triple), `rust-toolchain.toml` (target list).
- Unchanged: `core`, protocol crates, `engine`, `agent`, `ui` logic, store format.
- Depends on `create-native-rust-linux-terminal` (archived). Independent of `add-windows-port`, except that both modify the Docker-only build requirement (the text below includes the Windows exception).
