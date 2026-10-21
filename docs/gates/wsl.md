# WSL gate receipt (add-wsl-sessions)

This file holds the WSL validation receipts. The command-level run below validates the
Windows-only code that the Docker build cannot exercise (registry listing, service detection,
launch arguments, the `--cd` floor and the `cd` fallback). Rows that are not PASS keep the gate
open. The GUI smoke and the MobaXterm `MobaXterm.ini` type-14 observation (tasks 1.0 and the GUI
part of 2.2) need an interactive desktop session and are still open.

## Command-level validation — 2026-10-21

- Source: working tree on top of `e453905` (add-wsl-sessions changes uncommitted at run time).
- VM: `lab/vagrant/windows` (VirtualBox, headless), Windows Server 2022 Standard Evaluation,
  build 10.0.**20348**, nested VT-x enabled (`--nested-hw-virt on`), 8 GiB / 4 vCPU, fictional
  `vagrant` account. Reached over WinRM.
- WSL: the **inbox** WSL (feature `Microsoft-Windows-Subsystem-Linux` + `VirtualMachinePlatform`,
  service `LxssManager`); the Store WSL (`WslService`) and the WSL 2 kernel did not install on this
  build. A WSL 1 distribution (**Alpine** 3.20 minirootfs) was imported with
  `wsl --import Alpine C:\wsl\alpine alpine.tar.gz --version 1` — no kernel or nested VM needed.
- Build: `RXT_TOOLCHAIN_STAGE=windows-toolchain CARGO_TARGET_DIR=target/agent-windows
  scripts/cargo-docker.sh cargo build --release --target x86_64-pc-windows-gnu -p rustxterm`
  (Docker only), copied in with `lab/vagrant/windows/rxt.sh push`.
  - `rustxterm.exe` `cbefec46e5f2a0e9b3f242438875a84cfba16389355d8d7f9b538a6dc750d507`
  - `rustxterm-cli.exe` `a579f565a26a70dcfc574e282b525ef33d30e86cdec73a545a8d0d49682d5a13`

| Check | How (on the VM) | Result |
|---|---|---|
| `rustxterm-cli.exe --self-test` | config, terminal echo, theme tokens, engine, encrypted database, MCP handshake | PASS (6/6) |
| **Not installed** detection | `wsl.exe` present at `C:\Windows\System32`, but `HKLM\…\Services\WslService`, `LxssManager` and `HKCU\…\Lxss` all absent → `wsl_service_present()` false → `status()` = `NotInstalled { install_command: true }` (build 20348 ≥ 19041) | PASS |
| **No distribution** detection | after `wsl --install`: `LxssManager` service present and running, `WslService` absent, `Lxss` key present but with no GUID subkey → `status()` = `NoDistribution` | PASS |
| `--list --verbose` nonzero exit on no distribution | `wsl -l -v` exits non-zero and prints install guidance on stdout (UTF-16); the P2 fix (reject the output unless `status.success()`) stops it being parsed as a bogus distribution | PASS |
| **Listed** registry layout | after importing Alpine: one GUID subkey `{f07dc8fa-…}` with `DistributionName=Alpine` (REG_SZ), `State=1`, `Flags=7`; `Lxss\DefaultDistribution` = that same GUID. Matches `registry_distributions()` exactly (filter `{…}` subkeys, read `DistributionName`, mark default by GUID). | PASS |
| **WSL version source** | registry `Version` = **2** is the Lxss *schema* version, **not** the WSL version; `wsl -l -v` shows VERSION **1**. Fixed `registry_distributions()` to derive the WSL version from `Flags` bit 3 (`Flags=7` → WSL 1), matching the verbose listing. | PASS (fixed here) |
| `--list --verbose` parse | `WSL_UTF8=1 wsl -l -v` → `* Alpine  Stopped  1` (UTF-16LE, no BOM): `parse_wsl_list` reads name `Alpine`, default `true`, version `1` | PASS |
| UTF-16 output | every `wsl.exe` status/list output on this box is UTF-16LE; `decode_wsl_output` decodes it (BOM and NUL-detection paths both covered by tests) | PASS |
| Launch `-d` / `-u` | `wsl.exe -d Alpine -- echo …` → output, exit 0; `wsl.exe -d Alpine -u root -- whoami` → `root` | PASS |
| `--cd` floor is correct | on this inbox WSL (no `WslService`, build 20348 < 21286) `supports_cd()` = false, so the typed `cd` fallback is used — confirmed necessary: `wsl.exe -d Alpine --cd /tmp -- pwd` here resolves to **`/mnt/c/tmp`**, not `/tmp` (old `--cd` path-translation quirk) | PASS |
| `cd` fallback lines | typed into the real Alpine shell: `cd` → `/root`; `cd '/tmp'` → `/tmp`; `cd ~/'src'` → `/root/src` | PASS |
| Argument quoting / no shell | `command_line` quotes each argument by the `CommandLineToArgvW` rules; values reach `wsl.exe` as separate arguments and never through a shell (unit-tested; the launch above used the same path) | PASS |

## GUI smoke — 2026-10-21

The same VM booted with a display head (`VBoxManage startvm --type gui`) and auto-logon, driven
through `lab/vagrant/windows/rxt.sh` (clicks/keys/screenshots). rustXterm `rustxterm.exe` rendered
correctly (no black window). Screenshots in `docs/prints/windows/wsl_*.png` (git-ignored).

| Check | Result |
|---|---|
| The WSL type tile (Tux icon) appears in the session editor on Windows (`wsl_available`) | PASS |
| The distribution picker lists the live registry: selected text "Default (Alpine)", dropdown shows "Default (Alpine)" and "Alpine" | PASS |
| OK is enabled with no port (the WSL `validate()` path) | PASS |
| Opening the session starts a real Alpine shell: banner "WSL: Alpine", tab "WSL: Alpine", prompt `…:/mnt/c/Users/vagrant#` | PASS |
| The tab is interactive: `cat /etc/alpine-release && id` → `3.20.3`, `uid=0(root)…` | PASS |
| The session is saved under User sessions | PASS |

This exercises the whole chain on real Windows: editor → `Command::ListWslDistributions` →
`WindowsWsl::status()` → `registry_distributions()` → `Lxss` → Alpine → the picker, and
`launch_info()` → `wsl.exe -d Alpine` → ConPTY → the live shell and the WSL banner.

## MobaXterm type-14 observation (task 1.0) — 2026-10-21

MobaXterm 26.5 Personal/Portable was downloaded and run on the same VM (its license section 5.1
allows observing, studying and testing; nothing was decompiled). Its WSL session dialog matches
design W5: Basic (Distribution, Specify username, Run method = Autodetection), Advanced (Graphical
environment = Interactive shell, Startup directory, Execute commands at startup, Execute macro).

Two WSL bookmarks saved into `MobaXterm.ini` `[Bookmarks]` (sanitized, hand-typed into the test
fixture `crates/store/tests/fixtures/moba/wsl.ini`):

```
WSL-Alpine= #151#14%Alpine%
WSL-Default= #151#14%Default%%Interactive shell%%%0#MobaFont%10%0#0# #-1
```

Layout: `#<icon>#14%<distribution>%<username>%<graphical env>%<startup folder>%<startup commands>%<flag>#<terminal block>`.
The distribution (field 1, `Alpine` / `Default`) is **confirmed**; field 3 (`Interactive shell`) is
the confirmed graphical environment; the username/folder/commands positions are **inferred** from
the field order (MobaXterm's text fields would not take synthetic input, so those were not set with
distinctive values). The type-14 importer (`moba_ini.rs`) imports the distribution and the inferred
options, and reports an out-of-scope graphical environment as a warning.

### Open

- Task 2.2: the full native receipts on Windows 11 and Server 2022 with a Store WSL 2 distribution
  (this run used inbox WSL 1 / Alpine). The `--cd` floor's Store-WSL branch (`WslService` present)
  was not exercised here, only the inbox fallback.
- Confirm the inferred MobaXterm username/folder/commands field positions with a bookmark that sets
  distinctive values in each (blocked here by MobaXterm's synthetic-input handling).
