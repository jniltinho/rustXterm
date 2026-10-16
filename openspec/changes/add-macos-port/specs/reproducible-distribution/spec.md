## ADDED Requirements

### Requirement: macOS release artifacts
When a release is eligible for macOS (target verification on an available compliant Apple-Silicon host AND a receipt covering the tagged commit), it SHALL add, for Apple Silicon, `rustxterm-X.Y.Z-macos-arm64.dmg` containing `rustXterm.app` and a link to Applications, and `rustxterm-X.Y.Z-macos-arm64.tar.gz` containing the same app bundle and the licenses, both listed in `SHA256SUMS`. The app bundle SHALL carry the app icon, version and bundle identifier. Without signing secrets the bundle SHALL be ad-hoc signed and the release notes SHALL explain how to open an app from an unidentified developer; when Developer ID and notarization secrets exist, the bundle and the `.dmg` SHALL be signed and notarized. When a release is not eligible for macOS, it SHALL ship without macOS artifacts and SHALL remain valid (Linux and covered Windows artifacts only).

#### Scenario: Release contains macOS artifacts
- **WHEN** a version tag is pushed, the hosted macOS job verified the arm64 target, and the macOS receipt covers the tagged commit
- **THEN** the GitHub release lists the `.dmg` and the macOS `.tar.gz`, and `SHA256SUMS` matches them

#### Scenario: Release without macOS
- **WHEN** a version tag is pushed but the macOS job is disabled or no receipt covers the commit
- **THEN** the release is published with the Linux (and covered Windows) artifacts only and says why macOS is missing

#### Scenario: Unsigned build opens after approval
- **WHEN** a user downloads the ad-hoc signed `.dmg`, copies the app and opens it through Finder's Open or System Settings → Privacy & Security → Open Anyway
- **THEN** the app starts and later launches without a prompt

### Requirement: macOS quality gates
The `aarch64-apple-darwin` target SHALL be built and tested on a hosted macOS arm64 runner (the macOS job in the release workflow, also runnable on demand before tagging) with the pinned toolchain, `--locked` and `MACOSX_DEPLOYMENT_TARGET=13.0`, running `cargo clippy -D warnings`, the unit tests, a render smoke test on a real Metal device, and the app's `--self-test`. This runner is the authoritative gate for the macOS artifacts; its failure SHALL block shipping them. Tests that need the Docker lab, the UI pixel snapshots, or Linux-only facilities SHALL be skipped by target, not deleted. A fast build-smoke workflow MAY compile the target on demand so contributors without a Mac see compile results, but it does not run the graphics gate or gate a release.

#### Scenario: macOS regression caught
- **WHEN** a change breaks compilation, a unit test, or the render smoke test on the macOS arm64 runner
- **THEN** the `macos` job fails and the commit cannot ship macOS artifacts

### Requirement: macOS deployment-target floor
The macOS binaries SHALL encode a minimum OS of macOS 13.0, set by `MACOSX_DEPLOYMENT_TARGET=13.0` for the Rust and native compilation (not by `Info.plist` alone). `cargo xtask dist-check` SHALL verify the Mach-O architecture is `arm64` and its minimum-version load command is 13.0, and the receipt SHALL record the SDK/toolchain used and include macOS 13 execution evidence before that floor is claimed.

#### Scenario: Wrong minimum version rejected
- **WHEN** the shipped binary's Mach-O minimum-version metadata is not 13.0 or its architecture is not arm64
- **THEN** `dist-check` fails and the artifacts are not released

### Requirement: macOS end-to-end verification
The first release with macOS artifacts, and every later release that changes the macOS adapter or packaging, SHALL be preceded by a build-once→test→promote receipt produced on a compliant Apple-Silicon host: the host builds the artifacts once, runs the gates and end-to-end checks (window opens with a frame drawn on a real Metal device, master password, SSH session to the lab reachable from the Mac, theme follows a system appearance change, Keychain unlock, dialog sheet attachment) against those exact artifacts, and `docs/gates/macos.md` binds the commit to the checksums of the promoted artifacts plus the SDK/toolchain and Mach-O arch+min-version. `release.yml` SHALL promote those immutable artifacts without rebuilding them and SHALL verify their checksums equal the receipt; without a covering receipt the release SHALL ship without macOS artifacts.

#### Scenario: No receipt
- **WHEN** a version is tagged and no macOS receipt covers that commit
- **THEN** the release is published with the Linux (and covered Windows) artifacts only and says why macOS is missing

#### Scenario: Promoted artifacts match the receipt
- **WHEN** a release ships macOS artifacts
- **THEN** their `SHA256SUMS` entries equal the checksums recorded in the covering receipt, and no macOS artifact was rebuilt after the receipt

## MODIFIED Requirements

### Requirement: Docker-only build
All compilation, linting and tests SHALL run inside a pinned Docker image (pinned Rust toolchain, pinned base image by digest, `cargo --locked`). The host SHALL need only Docker. Exceptions: Windows test executables and the Windows release executables built in that image SHALL run natively on a hosted Windows runner (and on the manual verification VM) without installing or invoking a toolchain there; macOS builds, lints and tests SHALL run natively on macOS on a hosted macOS arm64 runner with the pinned toolchain from `rust-toolchain.toml`, because the Apple SDK may only be used on Apple hardware. No toolchain SHALL be installed on developer desktops for any target.

#### Scenario: Clean host build
- **WHEN** a developer with only Docker runs the documented build command
- **THEN** the release binary, AppImage and tarball appear under `dist/`

#### Scenario: macOS build on the hosted runner
- **WHEN** a release is prepared (a tag is pushed, or the macOS job is dispatched beforehand)
- **THEN** the macOS arm64 artifacts are built and tested on the hosted macOS runner and no developer machine needed a Rust toolchain

#### Scenario: macOS compile smoke without a Mac
- **WHEN** a contributor without a Mac triggers the build-smoke workflow on the hosted runner
- **THEN** the target compiles (or the failure is reported) and nothing is published or gated until the full macOS job and a real-Mac receipt cover the commit
