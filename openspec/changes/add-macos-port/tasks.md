## 0. Prerequisites (user decisions)

- [x] 0.1 Confirm the authoritative host: a hosted GitHub macOS arm64 runner (`macos-14`/`macos-15`) — free for this public repo. A real Mac is needed only for the end-to-end receipt.
- [x] 0.2 Decide on Developer ID signing and notarization (Apple Developer Program) or ad-hoc signing only — ad-hoc by default; `bundle.sh` auto-enables Developer ID + `--options runtime --timestamp` (and notarization) when signing secrets are present.
- [x] 0.3 Feasibility probe on the runner: `cargo check`/`cargo test` of the portable crates for `aarch64-apple-darwin`; record what fails — the adapter builds, clippies and tests green and the app binary links on `macos-14`; no compile failures to record.

## 1. Adapter (`crates/platform-macos`)

- [x] 1.1 Crate skeleton (L1, `cfg(target_os = "macos")`, workspace lints with `unsafe_code = "forbid"`), registered in `cargo xtask layers`
- [x] 1.2 Paths: Application Support, Caches, Logs, `~/.ssh`, per-user runtime dir under `$TMPDIR`; owner-only modes as on Linux
- [x] 1.3 Theme: a watcher behind `ThemeSource` reading `AppleInterfaceStyle` and reporting light/dark/auto changes (safe `core-foundation` distributed-notification observer, or a short poll if no safe path); no eframe coupling, no `unsafe`
- [ ] 1.4 Clipboard, notifications, opener with reported failures — done; native dialogs still open (sync `rfd` lands as a flagged first cut that panics off the main thread; the verified form is `rfd`'s async main-window API, sheet-attached, no raw AppKit handle, no `unsafe` — needs a Mac, design 3b)
- [x] 1.5 Keychain keyring wrap (`rustxterm:<id>` generic password) with master-password fallback
- [x] 1.6 Agent control on a Unix socket, same-user peer check (`peer_cred`), tests for allow/deny
- [x] 1.7 Serial enumeration of `/dev/cu.*` with USB details, `/dev/tty.*` twins hidden
- [x] 1.8 Unit tests for each module that run on the macOS runner

## 2. App wiring

- [x] 2.1 `apps/rustxterm`: target dependency and `os.rs` branch for macOS; skip the `.desktop` entry
- [ ] 2.2 Gate Linux-only tests and code paths found by the probe (`cfg(target_os = "linux")` where `cfg(unix)` is too wide) — depends on the feasibility probe (0.3) on a Mac/runner
- [x] 2.3 `rust-toolchain.toml` and `deny.toml` learn `aarch64-apple-darwin`

## 3. Packaging

- [ ] 3.1 `packaging/macos/Info.plist` template (`LSMinimumSystemVersion` 13.0) and `.icns` from the existing icons — `Info.plist` done; the `.icns` app icon is still pending (bundle currently ships without a custom icon).
- [x] 3.2 `packaging/macos/bundle.sh`: export `MACOSX_DEPLOYMENT_TARGET=13.0` for the Rust and native compile; build the `.app`, ad-hoc or Developer ID signature, `.dmg` and `.tar.gz`, optional notarization in order (`codesign` app → `.dmg` → `codesign` dmg → `notarytool submit --wait` → `stapler`)
- [ ] 3.3 `cargo xtask dist-check`: macOS bundle layout, `otool -L` allowlist, version, universal Mach-O (`arm64` + `x86_64` slices) each with minimum-version (13.0) metadata, checksums equal to the receipt, guard — interim: `release.yml` runs `cargo xtask guard` on the tarball and prints `otool -L`/`vtool -show-build`; the dedicated macOS `dist-check` subcommand is not written yet.

## 4. CI and release

- [ ] 4.1 macOS (arm64) job in `release.yml` (+ `workflow_dispatch` to run it before tagging), following photocraft's "macOS tested only in release.yml" model to spare per-push runner minutes: pinned toolchain, `MACOSX_DEPLOYMENT_TARGET=13.0`, clippy, tests, the render smoke test, `--self-test`; a failure blocks the macOS artifacts. `ci.yml` stays Docker/Linux-only per push. A fast build-smoke workflow lets contributors without a Mac compile the target on demand — shipped: the `macos` job (`macos-14`) in `release.yml`, the `macos.yml` build-smoke workflow with `workflow_dispatch`, pinned toolchain, `MACOSX_DEPLOYMENT_TARGET=13.0`, adapter clippy + tests, and `if-no-files-found: error` so a failure blocks the macOS artifacts; still pending: the render smoke test and `--self-test` (need a real Metal device).
- [ ] 4.2 In that job, build the ARM64 artifacts **once**, run the gates, write/verify the receipt, and promote those exact artifacts (not rebuilt) into the release and `SHA256SUMS`; no covering receipt ⇒ no macOS artifacts (do not copy the Windows path that ships without a receipt). When the hosted runner is unavailable, the release ships Linux/Windows only — build-once→promote is implemented (the `macos` job builds the artifacts once and uploads them; the `release` job folds the exact `.dmg`/`.tar.gz` into the release set and the single `SHA256SUMS`, never rebuilding them); still pending: the "no covering receipt ⇒ no macOS artifacts" gate — v0.6.1 shipped macOS as an unsigned preview without a covering receipt.
- [x] 4.3 Release notes text for unsigned builds (how to open an app from an unidentified developer)

## 5. Verification

- [ ] 5.1 `docs/gates/macos.md`: build-once→test→promote receipt on Apple Silicon (window + render smoke on a real Metal device, master password, SSH to the lab, theme following system change, Keychain unlock, dialog sheet attachment, serial adapter if available), recording SDK/toolchain and Mach-O arch+min-version, bound to the commit and to the checksums of the exact artifacts promoted
- [x] 5.2 Update README (supported platforms, install on macOS) and the docs index
