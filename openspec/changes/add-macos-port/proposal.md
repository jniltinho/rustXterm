## Why

Many of the people who manage servers from a laptop use a Mac with Apple Silicon, so a native rustXterm for macOS fills a real gap. The platform seam built for Linux and proven by `add-windows-port` (one adapter crate per OS, chosen in `apps/rustxterm/src/os.rs`) makes this a bounded change: protocol, engine and UI crates stay untouched.

## What Changes

- New crate `crates/platform-macos` (L1) implementing every platform contract for macOS 13 Ventura or later on Apple Silicon (`aarch64-apple-darwin`): per-user folders (`~/Library/Application Support`, `~/Library/Caches`, `~/Library/Logs`), theme following (light/dark appearance), clipboard, notifications, native file dialogs, Keychain for the optional keyring wrap, the Unix-socket agent control channel restricted to the same user (peer credentials through the standard library, no `unsafe`), `open`-style opener, and serial ports (`/dev/cu.*`).
- No `unsafe` code: the workspace `unsafe_code = "forbid"` stays in force for the new crate (the Windows exception is not extended).
- The app picks the macOS adapter on `target_os = "macos"`; Unix code paths already gated with `cfg(unix)` (file modes 0600/0700, SSH agent socket, X11 over Unix sockets) work unchanged; Linux-only tests stay gated.
- **Build exception:** macOS binaries cannot be built in Docker (the Apple SDK may only be used on Apple hardware), so macOS compilation, linting and tests run natively on macOS; the Linux and Windows builds stay Docker-only and nothing is installed on developer desktops.
  - **Validation is GitHub CI on a hosted macOS arm64 runner** (`macos-15`): clippy, unit tests, a render smoke test on a real Metal device and `--self-test`, with `MACOSX_DEPLOYMENT_TARGET=13.0`. Following [photocraft](https://github.com/storytold/photocraft)'s model (which keeps its scarce macOS slots for releases — "macOS is tested only by release.yml"), this job lives in `release.yml` (plus a `workflow_dispatch` to run it before tagging), so per-push `ci.yml` stays Docker/Linux-only. It is the authoritative gate and the only place the ARM64 build must pass. (photocraft, like rustXterm, is an egui/wgpu Rust app and builds macOS only on real Apple runners — never cross-compiled in Docker, which cannot run, render, sign or notarize the result.)
  - **A disposable local macOS KVM VM is an optional developer convenience** for contributors without a Mac, mirroring the Windows Vagrant VM under `lab/`, built on [osx-kvm](https://github.com/kholia/osx-kvm) (`boot-macOS-headless.sh`) or [ultimate-macOS-kvm](https://github.com/Coopydood/ultimate-macOS-kvm). It is an **x86_64 (Intel) guest with no Metal GPU**, so it is limited to non-graphical iteration — `cargo check`/`clippy`/non-graphical `cargo test` for `x86_64-apple-darwin` and bundle-script shakeout over SSH; it **cannot build or verify the `aarch64-apple-darwin` target or the UI**, never gates CI and never ships artifacts (see the compliance note).
- Release artifacts for macOS: `rustxterm.app` in a `.dmg` and a `.tar.gz`, ad-hoc signed by default (safe AppKit touches, if any, via `objc2`/`objc2-app-kit` as photocraft does, keeping `unsafe_code = "forbid"`); Developer ID signing and notarization when the corresponding secrets exist. Because macOS packaging/signing is not byte-reproducible, the release follows a **build-once → test → promote** receipt: the Apple-Silicon host builds the artifacts once, tests them, and those exact artifacts are promoted; the first macOS release (and any later adapter/packaging change) is gated on that receipt bound to their checksums.
- The binary's minimum-OS floor is set by `MACOSX_DEPLOYMENT_TARGET=13.0` (not `Info.plist` alone) and checked in `dist-check` (Mach-O arch `arm64` + min-version 13.0). The Docker lab is not available on macOS, so protocol lab tests stay Linux-only.

### Non-goals

Intel Macs (x86_64; can be added later as a universal binary), macOS 12 or older, Mac App Store and sandboxing, local shell (phase 2), built-in X server (XQuartz is used when present), Homebrew cask (later).

### Compliance note

Apple's macOS software licence permits running macOS only on Apple-branded hardware. The local KVM VM (osx-kvm / ultimate-macOS-kvm) is a widely-used developer convenience but falls outside that licence on non-Apple hardware; using it is the operator's decision and responsibility, and it is never part of CI or distributed artifacts. The licence-clean paths are a hosted macOS runner or a real Mac; the end-to-end release receipt (below) still requires a real Mac.

### CI budget

The macOS CI job needs GitHub macOS runner minutes (billed at ~10x Linux on a private repo, free on a public one). To keep it cheap the job runs only on `main` and tags with a cached `target/`. The optional local KVM VM lets a contributor without a Mac iterate before spending CI minutes.

## Capabilities

### New Capabilities

- `macos-platform`: macOS adapter behavior — supported versions, per-user folders, theme following, notifications, file dialogs, opener, serial port naming, and the agent control channel on macOS.

### Modified Capabilities

- `reproducible-distribution`: macOS native build and validation on a hosted GitHub macOS runner as an exception to the Docker-only build, with an optional local KVM VM for developer iteration (modified); macOS release artifacts, signing and verification (added).
- `session-management`: keyring wrap stored in the macOS Keychain (added).

## Impact

- New: `crates/platform-macos`; macOS-only dependency `security-framework` (Keychain, safe API) plus the already-used `arboard`, `rfd`, `notify-rust`, `serialport`, `dirs`, `tokio` as target-specific dependencies of the adapter.
- Changed: `apps/rustxterm` (adapter selection in `os.rs`, `.app` bundle metadata and icon), `xtask` (layers know the new crate; `dist-check` learns the macOS artifacts), `.github/workflows/release.yml` (macOS arm64 job + `workflow_dispatch`; `ci.yml` stays Docker/Linux-only per push), `packaging/macos/` (Info.plist, dmg script), `lab/macos/` (the KVM VM recipe and a driver like `lab/vagrant/windows/rxt.sh`), `deny.toml` (macOS triple), `rust-toolchain.toml` (target list).
- Unchanged: `core`, protocol crates, `engine`, `agent`, `ui` logic, store format.
- Depends on `create-native-rust-linux-terminal` (archived). Independent of `add-windows-port`, except that both modify the Docker-only build requirement (the text below includes the Windows exception).
