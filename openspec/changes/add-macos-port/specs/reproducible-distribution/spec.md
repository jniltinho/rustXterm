## ADDED Requirements

### Requirement: macOS release artifacts
Each release SHALL add, for Apple Silicon, `rustxterm-X.Y.Z-macos-arm64.dmg` containing `rustXterm.app` and a link to Applications, and `rustxterm-X.Y.Z-macos-arm64.tar.gz` containing the same app bundle and the licenses, both listed in `SHA256SUMS`. The app bundle SHALL carry the app icon, version and bundle identifier. Without signing secrets the bundle SHALL be ad-hoc signed and the release notes SHALL explain how to open an app from an unidentified developer; when Developer ID and notarization secrets exist, the bundle and the `.dmg` SHALL be signed and notarized.

#### Scenario: Release contains macOS artifacts
- **WHEN** a version tag is pushed and the macOS receipt covers the tagged commit
- **THEN** the GitHub release lists the `.dmg` and the macOS `.tar.gz`, and `SHA256SUMS` matches them

#### Scenario: Unsigned build opens after approval
- **WHEN** a user downloads the ad-hoc signed `.dmg`, copies the app and opens it through Finder's Open or System Settings → Privacy & Security → Open Anyway
- **THEN** the app starts and later launches without a prompt

### Requirement: macOS quality gates
The macOS target SHALL be built and tested natively on macOS — a disposable local macOS KVM VM for development, or a hosted macOS arm64 runner for CI — with the pinned toolchain and `--locked`, running `cargo clippy -D warnings`, the unit tests, the UI snapshot tests and the app's `--self-test`. Tests that need the Docker lab or Linux-only facilities SHALL be skipped by target, not deleted. The macOS KVM VM SHALL be disposable, kept out of Git, Docker build contexts and artifacts, and never used for CI or distributed builds.

#### Scenario: macOS regression caught
- **WHEN** a change breaks compilation or a unit test on macOS
- **THEN** the `macos` CI job fails and the commit cannot be released

### Requirement: macOS end-to-end verification
The first release with macOS artifacts, and every later release with changes to the macOS adapter or packaging, SHALL be preceded by a written receipt from a real Mac (window opens, master password, SSH session to the lab reachable from the Mac, theme follows the system, Keychain unlock), bound to the commit and to the artifact checksums; without a covering receipt the release SHALL ship without macOS artifacts.

#### Scenario: No receipt
- **WHEN** a version is tagged and no macOS receipt covers that commit
- **THEN** the release is published with the Linux (and covered Windows) artifacts only and says why macOS is missing

## MODIFIED Requirements

### Requirement: Docker-only build
All compilation, linting and tests SHALL run inside a pinned Docker image (pinned Rust toolchain, pinned base image by digest, `cargo --locked`). The host SHALL need only Docker. Exceptions: Windows test executables and the Windows release executables built in that image SHALL run natively on a hosted Windows runner (and on the manual verification VM) without installing or invoking a toolchain there; macOS builds, lints and tests SHALL run natively on macOS — a disposable local macOS KVM VM, or a hosted macOS runner — with the pinned toolchain from `rust-toolchain.toml`, because the Apple SDK may only be used on Apple hardware. No toolchain SHALL be installed on developer desktops for any target.

#### Scenario: Clean host build
- **WHEN** a developer with only Docker runs the documented build command
- **THEN** the release binary, AppImage and tarball appear under `dist/`

#### Scenario: macOS build without a local Mac toolchain
- **WHEN** a commit is pushed to `main`
- **THEN** the macOS artifacts are built and tested on the hosted macOS runner and no developer machine needed a Rust toolchain

#### Scenario: macOS build in a disposable VM
- **WHEN** a developer without a Mac builds and tests the macOS target in the local macOS KVM VM over SSH
- **THEN** the toolchain is installed only inside the VM, the VM is not committed or distributed, and the result does not gate a release until a real-Mac receipt covers it
