## 0. Prerequisites (user decisions)

- [ ] 0.1 Unblock GitHub-hosted macOS arm64 runners (user decision: macOS builds run on GitHub, not in Docker): Actions budget for macOS minutes, or a public repository
- [ ] 0.2 Decide on Developer ID signing and notarization (Apple Developer Program) or ad-hoc signing only
- [ ] 0.3 Feasibility probe on the `macos-probe` branch: `cargo check`/`cargo test` of the portable crates on `macos-15`; record what fails, then delete the branch

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

- [ ] 4.1 `ci.yml` job `macos` on `macos-15`: pinned toolchain, clippy, tests, `--self-test` (on `main` only)
- [ ] 4.2 `release.yml`: macOS build job, bundle, dist-check, artifacts joined into the release and `SHA256SUMS`; receipt gate as for Windows
- [ ] 4.3 Release notes text for unsigned builds (how to open an app from an unidentified developer)

## 5. Verification

- [ ] 5.1 `docs/gates/macos.md`: receipt on a real Mac (window, master password, SSH to the lab, theme following, Keychain unlock, serial adapter if available), bound to commit and checksums
- [ ] 5.2 Update README (supported platforms, install on macOS) and the docs index
