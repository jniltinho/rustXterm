## ADDED Requirements

### Requirement: Windows cross build in Docker
Windows executables SHALL be built in the same pinned Docker image family as the Linux release (pinned base image digest, snapshot packages, checksummed Rust components including the Windows target standard library, `cargo --locked`, network only in the fetch stage). Building and packaging (zip and MSI) SHALL NOT require a Windows host, Microsoft SDK downloads or a toolchain on the desktop.

#### Scenario: Windows build on a Linux host
- **WHEN** a developer with only Docker runs the documented Windows build command
- **THEN** `rustxterm.exe`, `rustxterm-cli.exe`, the portable zip and the MSI appear under `dist/`

### Requirement: Windows release artifacts
Each release SHALL include for Windows x86_64: a portable zip with `rustxterm.exe` (no console window), `rustxterm-cli.exe` (console: CLI, `mcp`, `--self-test`), license notices, third-party licenses and `ATTRIBUTION.md`; and a per-user MSI that installs without administrator rights and adds a Start Menu shortcut carrying the application's notification identity. SQLCipher and OpenSSL SHALL be linked statically. When a code-signing certificate is configured, both executables SHALL be Authenticode-signed before they are packaged, the MSI SHALL be signed after packaging, and every signature SHALL be verified (including the executables extracted from the zip and the MSI) before checksums are computed; a failed verification SHALL fail the release. Otherwise the release notes SHALL state that the Windows artifacts are unsigned and explain the SmartScreen prompt. `SHA256SUMS` SHALL be computed last and list every final artifact.

#### Scenario: Self-test on Windows
- **WHEN** `rustxterm-cli.exe --self-test` runs on a clean Windows runner
- **THEN** config load, database create/unlock in a temp dir, terminal echo and the MCP handshake pass and it exits 0

#### Scenario: Per-user MSI
- **WHEN** a standard user installs the MSI
- **THEN** no elevation prompt appears, rustXterm is in the Start Menu with the `rustXterm.rustXterm` application identity and uninstall removes the program files but not the user's data

#### Scenario: Unsigned release
- **WHEN** no signing certificate secret is configured
- **THEN** the release still publishes the Windows artifacts and its notes say they are unsigned

#### Scenario: Signed release
- **WHEN** a signing certificate secret is configured
- **THEN** the executables inside the zip and the MSI, and the MSI itself, carry valid signatures and their checksums match `SHA256SUMS`

### Requirement: Reproducible Windows artifacts
Two clean builds of the same commit SHALL produce byte-identical unsigned Windows executables, zip and MSI (no build timestamps, build paths or random identifiers in them).

#### Scenario: Rebuild comparison
- **WHEN** the release check builds the unsigned Windows artifacts twice from clean stages
- **THEN** both sets have the same SHA-256 digests

### Requirement: Windows quality gates
CI SHALL lint the workspace for the Windows target (clippy with `-D warnings`, which enforces the documented-`unsafe` rules), cross-compile the workspace tests for Windows in Docker and run them, plus `--self-test`, on a hosted Windows runner without installing or invoking a toolchain there, propagating every failure. Windows adapter integration tests (control pipe access and refusal, keyring, data protection, theme, opener, serial enumeration) SHALL run there. Linux snapshot images remain the UI reference; on Windows a render smoke test SHALL draw the main window with software rendering. The layer, license and guard checks SHALL cover the Windows target and Windows artifacts (zip and MSI members).

#### Scenario: Windows regression blocks release
- **WHEN** a Windows test or the Windows clippy gate fails
- **THEN** CI is red and the release workflow refuses to publish artifacts for that commit

#### Scenario: Guard scans Windows artifacts
- **WHEN** a zip or MSI member contains an absolute home path or matches the forbidden-digest list
- **THEN** the guard fails

### Requirement: Windows end-to-end verification
Before the first Windows release, the protocol lab SHALL be exercised from a Windows VM running the release-pipeline artifacts (SSH with password, key, Windows OpenSSH agent and Pageant, jump host, tunnels, SFTP, FTP/FTPS, RDP, VNC, Telnet, a serial port, agent CLI and MCP including refusal of another local user, keyring, data-directory protection against another local user, theme switch, notifications, file dialogs, MSI install and uninstall as a standard user), with fictional credentials and the lab still bound to the host's loopback. The result SHALL be recorded in a receipt in the repository naming the source commit, the Windows build number and the SHA-256 of the tested artifacts, with every mandatory check passing; the release workflow SHALL publish the Windows artifacts in every release and, while the receipt is missing or not passing for that commit, SHALL mark them as a preview in the release notes.

#### Scenario: Receipt present
- **WHEN** the Windows milestone is pushed
- **THEN** `docs/gates/windows.md` lists each mandatory check as PASS with the commit, Windows build number and artifact digests

#### Scenario: Failing receipt marks Windows as preview
- **WHEN** a mandatory receipt row is FAIL or missing
- **THEN** the release workflow still publishes the Windows zip and MSI, and the release notes call them a preview not covered by the receipt

## MODIFIED Requirements

### Requirement: Docker-only build
All compilation, linting and tests SHALL run inside a pinned Docker image (pinned Rust toolchain, pinned base image by digest, `cargo --locked`). The host SHALL need only Docker. Exception: Windows test executables and the Windows release executables built in that image SHALL run natively on a hosted Windows runner (and on the manual verification VM) without installing or invoking a toolchain there.

#### Scenario: Clean host build
- **WHEN** a developer with only Docker runs the documented build command
- **THEN** the release binary, AppImage and tarball appear under `dist/`

### Requirement: Quality gates
CI SHALL compare lab-test identities (names), not only counts, against the expected list, and SHALL run `cargo fmt --check`, `cargo clippy -D warnings` with workspace lints (`unsafe_code = "forbid"` in every crate except the Windows platform adapter, which allows only documented `unsafe` blocks by user decision; no `unwrap`/`expect` outside tests), unit tests, UI snapshot tests in light and dark theme, layer checks, and the lab integration tests.

#### Scenario: Panic path rejected
- **WHEN** non-test code adds `.unwrap()`
- **THEN** the clippy gate fails

#### Scenario: Unsafe lint relaxed
- **WHEN** a manifest removes `unsafe_code = "forbid"` from the workspace lints, or a crate other than the Windows adapter stops inheriting them
- **THEN** the layer check fails
