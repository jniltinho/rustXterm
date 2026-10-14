# Proposal validation

Date: 2026-10-13. Scope: planning artifacts of `add-windows-port` only. Static review approval is not evidence that builds, tests or the application work; runtime evidence is in `docs/gates/windows.md`.

## Local checks

- OpenSpec CLI 1.10.0: `openspec validate add-windows-port --strict` passes.
- Planning artifacts: proposal, design (W1–W11), 8 capability deltas (`windows-platform` new; `agent-control`, `session-management`, `reproducible-distribution` with ADDED and MODIFIED requirements; `ssh-sessions`, `terminal-emulation`, `serial-sessions`, `desktop-shell` ADDED), tasks (W1–W3) with a requirement-coverage table.

## Independent Codex CLI review

Codex CLI 0.162.1, `codex exec -s read-only --ephemeral --skip-git-repo-check`, input limited to the planning files of this change plus the base change's design/specs and the current platform/build files for context (never `base/` or `docs/prints/`). Raw outputs stay local (`codex-windows-round-{1..6}.md`, `codex-windows-w1-round-{7,8}.md`; round 5 was run by a second, since-retired session with an equivalent prompt).

| Round | Verdict | Main points addressed |
|---|---|---|
| 1 | REVISE | pipe authentication by the connection's security context; pipe instance lifecycle; protection of existing files; network/FAT data dirs; Windows clippy as CI gate and lint-table checks; workspace/test selection vs Docker-only rule; Docker-only MSI fallback; signing/checksum order; reproducibility verification; Pageant consistency; receipt PASS gating release; coverage and MODIFIED inherited requirements |
| 2 | REVISE | MSI determinism (wixl PackageCode/dates → `xtask msi-normalize`); adapter `unsafe_code = "allow"` exemption; receipt bound to the published artifacts; coverage rows; CLI/MCP MODIFIED for `rustxterm-cli.exe` |
| 3 | REVISE | replaceable ancestors (profile containment); filtering-aware token policy (deny-only groups, UAC); CLI/MCP/database scenario verification |
| 4 | REVISE | ancestor ACL validation inside the profile; elevation matrix |
| 5 | REVISE | write-data/write-attributes (reparse creation) and null DACLs on ancestors |
| 6 | REVISE | pre-existing modifiable entries rejected instead of repaired; atomic owner-only creation; path normalization |
| 7 | REVISE | append/EA rights; untrusted owners of existing entries; whole-tree preflight |
| 8 | **APPROVE** | no remaining blockers (suggestions applied: EA-only fixture, W40/W41 in coverage) |

## User decisions recorded

`unsafe` allowed only in `crates/platform-windows` (own `[lints]` table, `// SAFETY:` per block, `undocumented_unsafe_blocks` and `multiple_unsafe_ops_per_block` denied, enforced by `cargo xtask layers`); Windows verification on the existing Windows Server 2022 VM (`lab/vagrant/windows`) instead of a new Windows 11 VM (Windows 11 optional later). Decisions taken during review: data directories must lie inside the user profile; CLI and GUI must run at the same elevation; SQLCipher memory security off on Windows (W1 finding).
