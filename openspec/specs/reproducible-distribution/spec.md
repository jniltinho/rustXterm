# reproducible-distribution Specification

## Purpose
Build, test and ship rustXterm without installing toolchains on the desktop, and keep local-only material out of the repository.

## Requirements

### Requirement: Docker-only build
All compilation, linting and tests SHALL run inside a pinned Docker image (pinned Rust toolchain, pinned base image by digest, `cargo --locked`). The host SHALL need only Docker.

#### Scenario: Clean host build
- **WHEN** a developer with only Docker runs the documented build command
- **THEN** the release binary, AppImage and tarball appear under `dist/`

### Requirement: Portable release artifacts
Each release SHALL produce a stripped x86_64 binary linked against glibc 2.31 or older symbols, with SQLite/SQLCipher and OpenSSL linked statically and graphics/windowing libraries (Vulkan/GL, Wayland, X11, xkbcommon) loaded at run time; an AppImage; and a tarball with a `.desktop` file, an original icon, license notices and SHA-256 checksums. The binary SHALL not depend on Qt, GTK webviews, Wine or a Rust toolchain at runtime. `rustxterm --self-test` SHALL run headless checks (config load, database create/unlock with a test password in a temp dir, terminal model echo, MCP handshake) and exit non-zero on failure.

#### Scenario: glibc baseline
- **WHEN** `objdump -T` lists the binary's versioned glibc symbols
- **THEN** none is newer than GLIBC_2.31

#### Scenario: Distribution matrix
- **WHEN** the binary and AppImage run `--self-test` in clean containers of Ubuntu 20.04 (glibc 2.31), 22.04, 24.04 and 26.04, Debian 12 and 13, Fedora latest and Arch Linux
- **THEN** all pass, and a GUI smoke run under Xvfb with software rendering opens the main window and an SSH tab to a loopback lab sshd on Ubuntu 24.04 and 26.04

#### Scenario: Desktop session
- **WHEN** the AppImage runs on a clean Ubuntu 24.04 GNOME/Wayland and a KDE/X11 session
- **THEN** the window opens, an SSH session to the lab works and the theme follows the desktop

### Requirement: Protocol test lab
The repository SHALL provide a disposable Docker Compose lab with OpenSSH servers (password, key, keyboard-interactive, a bastion and an internal host), an SFTP-only account, vsftpd with plain FTP and explicit/implicit FTPS, xrdp and a VNC server, bound to 127.0.0.1 only and using fictional credentials. Optional small Vagrant boxes (Linux; Windows for RDP) SHALL be documented for cases containers cannot cover.

#### Scenario: Lab integration test
- **WHEN** the integration suite runs against the lab
- **THEN** SSH auth methods, jump host, tunnels, SFTP and FTP/FTPS transfers, RDP and VNC pass

### Requirement: Quality gates
CI SHALL compare lab-test identities (names), not only counts, against the expected list, and SHALL run `cargo fmt --check`, `cargo clippy -D warnings` with workspace lints (`unsafe_code = "forbid"`, no `unwrap`/`expect` outside tests), unit tests, UI snapshot tests in light and dark theme, layer checks, and the lab integration tests.

#### Scenario: Panic path rejected
- **WHEN** non-test code adds `.unwrap()`
- **THEN** the clippy gate fails

### Requirement: Local-only material stays local
`base/` (local working files) and `docs/prints/` (local screenshots) SHALL be git-ignored, excluded from Docker build contexts and never published. Every committed third-party file — code, resources, icons, fonts or fixtures — SHALL be listed in `ATTRIBUTION.md` with its origin and license.

#### Scenario: Guard covers history and releases
- **WHEN** any commit being pushed or in history contains a path under `base/` or `docs/prints/`, or any historical blob, tracked file or release artifact member (including files inside AppImage/tarball) has a SHA-256 in the forbidden-digest list (all files under `base/`, computed locally) or contains an absolute home path
- **THEN** the local pre-push guard fails, verified with disposable fixtures including a renamed copy of a forbidden file; the CI guard, which has no access to `base/`, enforces the path, home-path and attribution checks only

#### Scenario: Asset provenance
- **WHEN** a commit adds an icon, image, font or test fixture
- **THEN** it must have a row in `ATTRIBUTION.md` naming its origin and license, otherwise the guard fails

### Requirement: Modular, portable architecture
Each protocol (SSH, SFTP, FTP, RDP, VNC) SHALL live in its own crate with no UI dependency, and operating-system services (paths, keyring, theme detection, clipboard, notifications, local control transport, file dialogs) SHALL be reached only through a platform contract crate with a Linux adapter, so that macOS and Windows adapters can be added without changing protocol, engine or UI crates. Layering SHALL be enforced by a build check.

#### Scenario: Layer violation
- **WHEN** a protocol crate adds a dependency on the UI crate or calls a Linux-only API directly
- **THEN** the layer check fails

### Requirement: Milestone delivery
Work SHALL be delivered in milestones; each milestone whose tasks are verified SHALL be committed and pushed to the private GitHub repository only after the guard passes, with the milestone and verified task ids in the commit message.

#### Scenario: Milestone push
- **WHEN** a milestone's tasks are verified
- **THEN** a commit naming the milestone and task ids is pushed and the guard ran green before the push
