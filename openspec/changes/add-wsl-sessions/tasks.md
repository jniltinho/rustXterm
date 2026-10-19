## 1. WSL sessions (code)

- [ ] 1.1 `core`: `Protocol::Wsl` (saved, outside the Windows-only UI lists on other platforms), `WslOptions`, serde round trip; `wsl://` quick connect
- [ ] 1.0 Finish observing MobaXterm 26.5 (license 5.1: observe, study, test; no decompiling) on a machine with a WSL distribution: the `MobaXterm.ini` layout of WSL bookmarks (type 14) for each option and the `wsl.exe` command lines it starts; sanitized fixtures and notes in `docs/gates/wsl.md` (dialog options already observed, design W5)
- [ ] 1.2 `platform`: `Wsl` contract and fake; `platform-windows`: registry listing with the four states and the `--list --verbose` fallback (W2), `launch_info` with quoting, name validation and the `--cd` floor with the `cd` fallback (W3); `command_line` quotes every argument (local shell too)
- [ ] 1.3 `engine`: WSL tab path through the local terminal start with the WSL launch info; startup commands typed after the quiet period (W3); refusal without `Platform::wsl`; agent denial, MultiExec skip and no data-stream tap for WSL (W4); `FakePty` records the `LaunchInfo`
- [ ] 1.4 `ui`: WSL editor page (distribution picker with refresh, user, startup folder, startup commands, the three not-available states), new-session menu and icon, banner; kittest snapshots light and dark
- [ ] 1.5 `store`: unknown-protocol rows and import entries skipped with a warning (W4a); MobaXterm.ini type-14 WSL bookmarks from the 1.0 fixtures; export/import keeps WSL sessions

## 2. Verification and push

- [ ] 2.1 The W6 tests: listing and fallback fixtures, quoting round trip, name rules, engine with fake `Wsl`/`FakePty`, protections, session log vs capture, unknown-protocol store rows, UI snapshots
- [ ] 2.2 Native receipts on Windows 11 and Server 2022 with WSL (W6 list) in `docs/gates/wsl.md`
- [ ] 2.3 `make check`, Windows-target clippy, `cargo xtask guard`, `cargo xtask prints`; commit and push with the task ids
