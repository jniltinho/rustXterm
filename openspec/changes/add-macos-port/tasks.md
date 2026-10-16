## 0. Prerequisites (user decisions)

- [ ] 0.1 Choose the macOS build/test host (user decision): a disposable local macOS KVM VM (osx-kvm / ultimate-macOS-kvm; note the Apple-licence caveat on non-Apple hardware), a hosted GitHub macOS runner (needs Actions budget or a public repo), or a borrowed Mac
- [ ] 0.2 Decide on Developer ID signing and notarization (Apple Developer Program) or ad-hoc signing only
- [ ] 0.3 Feasibility probe: `cargo check`/`cargo test` of the portable crates on the chosen macOS host (the KVM VM or a Mac); record what fails

## 0b. macOS test VM (local KVM, lab/macos/)

- [ ] 0b.1 `lab/macos/README.md`: create/destroy recipe for the disposable macOS VM on osx-kvm (`boot-macOS-headless.sh`) or ultimate-macOS-kvm, pinned guest macOS version and tool commit, fictional guest account, forwarded SSH port; never committed VM image, never in a Docker context or artifact (verified by `cargo xtask guard`)
- [ ] 0b.2 Driver `lab/macos/rxt.sh` mirroring `lab/vagrant/windows/rxt.sh`: `push` the macOS build into the VM, run a command over SSH, capture a screenshot; the macOS `cargo build`/`clippy`/`test`/bundle run through it
- [ ] 0b.3 Prove the toolchain lives only inside the VM: `rustup` + `cargo test --locked` of the portable crates succeed over SSH, and the host desktop has no Rust toolchain for the Apple target

## 1. Adapter (`crates/platform-macos`)

- [ ] 1.1 Crate skeleton (L1, `cfg(target_os = "macos")`, workspace lints with `unsafe_code = "forbid"`), registered in `cargo xtask layers`
- [ ] 1.2 Paths: Application Support, Caches, Logs, `~/.ssh`, per-user runtime dir under `$TMPDIR`; owner-only modes as on Linux
- [ ] 1.3 Theme: report the system appearance and its changes (light/dark/auto)
- [ ] 1.4 Clipboard, notifications, native dialogs parented to the main window, opener with reported failures
- [ ] 1.5 Keychain keyring wrap (`rustxterm:<id>` generic password) with master-password fallback
- [ ] 1.6 Agent control on a Unix socket, same-user peer check (`peer_cred`), tests for allow/deny
- [ ] 1.7 Serial enumeration of `/dev/cu.*` with USB details, `/dev/tty.*` twins hidden
- [ ] 1.8 Unit tests for each module that run on the macOS runner

## 2. App wiring

- [ ] 2.1 `apps/rustxterm`: target dependency and `os.rs` branch for macOS; skip the `.desktop` entry
- [ ] 2.2 Gate Linux-only tests and code paths found by the probe (`cfg(target_os = "linux")` where `cfg(unix)` is too wide)
- [ ] 2.3 `rust-toolchain.toml` and `deny.toml` learn `aarch64-apple-darwin`

## 3. Packaging

- [ ] 3.1 `packaging/macos/Info.plist` template and `.icns` from the existing icons
- [ ] 3.2 `packaging/macos/bundle.sh`: `.app`, ad-hoc or Developer ID signature, `.dmg` and `.tar.gz`, optional notarization
- [ ] 3.3 `cargo xtask dist-check`: macOS bundle layout, `otool -L` allowlist, version, checksums, guard

## 4. CI and release

- [ ] 4.1 Optional `ci.yml` job `macos` on `macos-15` (enabled once a hosted runner is available): pinned toolchain, clippy, tests, `--self-test` (on `main` only); development uses the local KVM VM instead
- [ ] 4.2 `release.yml`: macOS build job, bundle, dist-check, artifacts joined into the release and `SHA256SUMS`; receipt gate as for Windows
- [ ] 4.3 Release notes text for unsigned builds (how to open an app from an unidentified developer)

## 5. Verification

- [ ] 5.1 `docs/gates/macos.md`: receipt on a real Mac (window, master password, SSH to the lab, theme following, Keychain unlock, serial adapter if available), bound to commit and checksums
- [ ] 5.2 Update README (supported platforms, install on macOS) and the docs index
