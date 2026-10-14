# Project guidelines

- rustXterm (`rustxterm`): native Rust terminal and remote session manager for Linux (SSH, SFTP, FTP/FTPS, RDP, VNC, Telnet, Serial, remote monitoring, agent control via CLI and MCP).
- `base/` and `docs/prints/` are local-only, git-ignored scratch/reference folders. Never add them to Git, Docker build contexts or artifacts. Run `cargo xtask guard` before every push.
- Specs live in `openspec/specs/`; active changes in `openspec/changes/` (archive completed ones with `/opsx:archive`).
- Architecture: one crate per protocol, `platform` contracts + `platform-linux` adapter (macOS/Windows later), layers enforced by `cargo xtask layers`, everything is a command, thin egui UI with theme tokens.
- Light, dark and system themes are required for every screen; `cargo xtask prints` writes screenshots of every screen to `docs/prints/`.
- Every committed icon, image, font and test fixture needs a row in `ATTRIBUTION.md` (checked by `cargo xtask guard`).
- Code first in large batches, then one verification batch per milestone; commit and push each verified milestone with task ids in the message.
- Parallel agents own disjoint crates and use `CARGO_TARGET_DIR=target/agent-<name>`.
- Builds, lints and tests run only in Docker; do not install toolchains on the desktop.
- For Rust, load `rust-best-practices`, `rust-async-patterns` and `rust-testing` skills as applicable.
- Tests use the Docker lab (`lab/`) or small Vagrant VMs with fictional credentials; never real servers.
- Write code, comments, CLI messages, docs and OpenSpec artifacts in English.
- Releases: tag `vX.Y.Z` only on a commit whose `ci` run on `main` is green (release.yml enforces it and only builds/publishes); follow the `create-release` skill for versioning and release notes.
