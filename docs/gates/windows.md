# Windows gate receipt (add-windows-port)

This file holds the W1 smoke run (task 1.12), the W2 packaging checks and, at the end, the release
receipt of task 3.2 (rows W01–W41 checked by `xtask windows-receipt --check`). The release receipt
is open: rows that are not PASS keep the gate closed.

## W1 smoke — 2026-10-13

- Source: working tree on top of `136efbb` (W1 changes uncommitted at the time of the run).
- VM: the Windows Server 2022 test VM (lab/vagrant/windows), Windows Server 2022 Standard 21H2,
  build 10.0.20348.4648, VirtualBox `vboxsvga` without 3D acceleration, fictional `vagrant`
  account (local administrator; UAC does not filter the interactive token on this box).
- Lab: host Docker lab bound to 127.0.0.1, reached from the VM through VirtualBox NAT at `10.0.2.2`.
  For the protocol test executables (which target 127.0.0.1) a temporary
  `netsh interface portproxy` mapped 127.0.0.1:{2222,2223,13389,2323,2007,5901} to `10.0.2.2`;
  it was removed after the run.
- Build: `RXT_TOOLCHAIN_STAGE=windows-toolchain CARGO_TARGET_DIR=target/agent-windows
  scripts/cargo-docker.sh cargo build --release --target x86_64-pc-windows-gnu -p rustxterm`
  (Docker only), copied in with `lab/vagrant/windows/rxt.sh push`.
- Artifacts (unsigned, not the release pipeline):
  - `rustxterm.exe` `70564e4a7a408c46aa8420dd4e8d14224dc7b848075f9af2f2b2ec0e595977e9`
  - `rustxterm-cli.exe` `9faec50bcec803da2bc499d264af7c7f31d82f449dccb1da361235b7bb6f97a0`

| Check | How | Result |
|---|---|---|
| PE headers | `objdump -p`: `rustxterm.exe` subsystem 2 (GUI), `rustxterm-cli.exe` subsystem 3 (console), timestamp 0, stack reserve 8 MiB | PASS |
| DLL imports | only Windows system DLLs (kernel32, advapi32, user32, gdi32, ws2_32, crypt32, bcrypt, ncrypt, dxgi, opengl32, ole32, shell32, setupapi, uxtheme, dwmapi, imm32, msvcrt, api-ms-win-core-*), no MinGW/OpenSSL/SQLite DLL | PASS |
| `rustxterm-cli.exe --self-test` (WinRM and desktop session) | config, terminal echo, theme tokens, engine, encrypted database, MCP handshake | PASS (6/6) |
| `rustxterm.exe --self-test` | exits with the usage code 2, no window | PASS |
| Adapter tests `rxt_platform_windows` (desktop session) | 32 tests: pipe round trip, concurrent clients, pipe gone after drop, second listen refused, squatted name refused, restricted-SID / deny-only / low-integrity clients refused, privilege-only client served, silent client closed after 5 s; Credential Manager round trip; make_private repair, rejection of modifiable entries, owner-only creation, junction inside / in the ancestry, permissive ancestor, outside profile, network path; theme read; opener error; COM enumeration | PASS (32/32) |
| Keyring without a credential store | same test from a WinRM network logon: `ERROR_NO_SUCH_LOGON_SESSION` → `Unsupported` (master-password fallback) | PASS |
| Store, engine, core, ssh, serial, term, platform unit/integration test executables | cross-compiled debug tests run on the VM | PASS (all) |
| RDP to the lab (`lab_rdp --ignored`) | xrdp 13389: certificate prompt/accept/save, changed cert refused, login alice, input, resize, fixed resolution, hidden tab | PASS (7/7) |
| SSH + SFTP to the lab (`lab_sftp --ignored`) | sshd 2222, alice password: shell + SFTP on one connection, 100 MB put/get checksum, cancel, overwrite, OSC 7 follow, open-with upload-back | PASS (7/7) |
| Telnet (`lab_telnet --ignored`) | login, echo, terminal type, NAWS, auto-login opt-in/out, raw echo | PASS (5/5) |
| VNC (`lab_vnc --ignored`) | password auth, Raw/CopyRect/Tight/ZRLE, view-only, hidden tab, VNC through SSH direct-tcpip | PASS (8/8) |
| Agent control over the named pipe | GUI started with `RXT_AGENT_CONTROL=1`: pipe `rustxterm-<SID>-control` exists; `rustxterm-cli.exe tabs list --json` from the desktop session and from WinRM gets the engine's answer `{"error":{"code":"locked",…}}`, exit 3 (database not unlocked yet; same contract as Linux) | PASS (transport); unlocked `tabs list` pending |
| UI rendering offscreen (kittest, wgpu DX12) | `rxt_ui` snapshot test rendered the first-run home screen correctly (`docs/prints/windows/home_first_run_{light,dark}.png`, git-ignored) | PASS |
| GUI window (before the fix) | window, title and taskbar icon appeared, process responsive, **content black** with DX12; OpenGL: no adapter; Vulkan: no surface | FAIL → fixed below |

### GUI smoke after the WARP adapter fix (windows-w2, 2026-10-14)

Root cause of the black window: on this VM (VirtualBox SVGA without 3D) DXGI lists two
"Microsoft Basic Render Driver" (WARP) adapters: a non-software one (vendor 0x1414, flags 0) that
owns the display output, and the software one (`DXGI_ADAPTER_FLAG_SOFTWARE`) without outputs.
wgpu-hal 30 skips the first on purpose and rendered on the second; DX12 flip presentation from an
adapter without the output stays black (probe `EnumAdapters1`/`EnumOutputs`/`D3D12CreateDevice` run
in the desktop session). Fix: vendored `wgpu-hal` keeps the non-software WARP adapter when it owns
an output (`vendor/README.md`); no app or config change, no VirtualBox 3D needed. VirtualBox
framebuffer screenshots work again as well. Build: same command as above with the patch,
`rustxterm.exe` and `rustxterm-cli.exe` pushed with `rxt.sh push`; GUI driven with `rxt.sh
click/keys/key/shot` (VirtualBox keyboard, so the hidden helper task does not take the focus).

| Check | How | Result |
|---|---|---|
| GUI renders | first run: home screen and the master-password dialog drawn (`gui_first_run_master_password_light.png`) | PASS |
| Master password first run + keyring | password typed twice, "Unlock with system keyring" ticked, Create; `cmdkey /list` shows `rustxterm:rustxterm-db-…` | PASS |
| SSH tab to the host lab | quick connect `alice@10.0.2.2:2222`, unknown host key → Accept and save, password `lab-alice-pass`; Ubuntu banner and prompt | PASS |
| SFTP panel + follow cd | panel lists `/home/alice`; `mkdir … && cd /tmp/rxtwin` in the terminal → panel follows to `/tmp/rxtwin` with `from-windows.txt` (`ssh_sftp_follow_light.png`) | PASS |
| Theme follows Windows app mode | `theme = "system"`; `AppsUseLightTheme` 1→0 → dark (`ssh_sftp_follow_dark.png`), 0→1 while an RDP tab runs → light (`rdp_light.png`), live, no restart | PASS |
| Keyring auto-unlock after restart | app exited, restarted: sessions unlocked without a prompt (`gui_keyring_unlocked_dark.png`) | PASS |
| RDP tab to the lab xrdp | quick connect `rdp://alice@10.0.2.2:13389`, certificate prompt → Accept once, password; remote xterm session shown (`rdp_dark.png`) | PASS |
| Agent control with an unlocked vault | `agent_control = true`; `rustxterm-cli.exe tabs list --json` from the desktop session → `[{"id":1,"name":"alice@10.0.2.2","protocol":"rdp","state":"Connected"}]`, exit 0 | PASS |
| Browse dialog in front (HWND parent) | session editor → SSH → Private key Browse…: `IFileDialog` (`#32770` "Select private key") owned by the main window, main window disabled while it is open (`browse_dialog_light.png`) | PASS |
| Data-directory DACL read-back | `%LOCALAPPDATA%\rustxterm`: `O:BA … D:PAI(A;OICI;FA;;;<vagrant SID>)`; db and envelope inherit the single ACE (owner Administrators: unfiltered admin token on this box, accepted by W6) | PASS |

Notes: the file dialog opens in the process working directory (`C:\Windows\System32` when started
from the scheduled task); the main window closes without the confirm-close prompt from the toolbar
Exit with an RDP/SSH tab open (to check against Linux behavior in W3).

Finding fixed during the run: SQLCipher `cipher_memory_security` overflowed the stack on Windows
(VirtualLock fails with `ERROR_WORKING_SET_QUOTA`, then WAL setup recurses); it is disabled on
Windows (`crates/store/src/db.rs`). Also the main-thread stack is set to 8 MiB as on Linux.

The VM was halted (not destroyed) after the run.

## W2 packaging checks — 2026-10-14 (windows-w2; not the release receipt)

- Build: `docker build --target dist --build-arg SOURCE_DATE_EPOCH=1791936000` (working tree, uncommitted).
  Output: Linux binary, tarball, AppImage plus `rustxterm-0.2.0-windows-x86_64.zip` and
  `rustxterm-0.2.0-x86_64.msi`, SHA256SUMS over all five (Dockerfile `checksums` stage).
- Reproducibility: two builds of one source snapshot with
  `--no-cache-filter fetch,build,metadata,appimage,dist-windows,checksums`: zip
  `53701a82a6301f78350c2ffbba9232d1d0fb6369d00ca61d28bfdcae2c2af063` and MSI `b8e9584a82691a06d356483a7de9483b161f4a525ee29e437d0dd82af3ac5699`
  identical in both (`cargo xtask dist-check a --reproduce b` in the windows-toolchain image: ok). The
  Linux ELF/tarball/AppImage differ between the two clean builds (pre-existing zigbuild issue; not in
  the Windows comparison).

| Check | How | Result |
|---|---|---|
| dist-check | PE subsystem 2/3, timestamp 0, DLL import allowlist on the exes from the zip, zip and MSI exes identical, SHA256SUMS, guard over tarball/AppImage/zip/MSI | PASS |
| MSI tables | `msiinfo export … MsiShortcutProperty` → `AppUserModelId StartMenuShortcut {9F4C2855-…}, 5 rustXterm.rustXterm`; `msiinfo suminfo` → PackageCode UUIDv5 of the version, dates = SOURCE_DATE_EPOCH | PASS |
| Guard fixtures | zip and MSI (wixl) fixtures with a renamed forbidden file and a home path fail `guard` (`windows_zip_and_msi_members_are_scanned`, windows-toolchain image) | PASS |
| Signing (throwaway self-signed cert) | `packaging/windows/sign.sh`: exes signed, zip/MSI repackaged, MSI signed, `osslsigncode verify` of the MSI and of the exes from zip and MSI ok; with an unrelated CA the run exits 1 and the unsigned files stay unchanged; a byte-flipped signed MSI fails verify | PASS |
| MSI install on the VM (per-user, no prompt) | `msiexec /i … /qn` exit 0 as `vagrant` in the desktop session: files in `%LOCALAPPDATA%\Programs\rustXterm`, Start Menu `rustXterm.lnk` with AUMID `rustXterm.rustXterm`, `HKCU\Software\rustXterm`; launched from the shortcut → GUI renders, vault auto-unlocked (`msi_start_menu_launch.png`) | PASS |
| MSI uninstall | `msiexec /x … /qn` exit 0: install dir, shortcut, HKCU key and ARP entry gone; user data in `%LOCALAPPDATA%\rustxterm` kept | PASS |
| Test executables on Windows (CI `windows-test` equivalent on the VM) | 43 cross-compiled test exes (`CARGO_PROFILE_DEV_DEBUG=0`, stripped; xtask excluded) with `PATH=%SystemRoot%\System32;%SystemRoot%`, sources at `\work`: all pass after the fixes below, incl. the WARP render smoke test; `rustxterm-cli.exe --self-test` 6/6 | PASS |

Fixes from that run: `rxt-platform-windows` pipe tests share the per-user pipe name and now run
one at a time (they collided under the default parallel runner); kittest image comparisons run on
Linux only (Windows renders without comparing, design W10); `eframe_sessions_migrate_once_on_first_unlock`
waited on `vault_busy`, which the startup keyring attempt also clears, and now waits for the sessions.
VirtualBox framebuffer screenshots failed again after the VM was used by another project; the
`msi_start_menu_launch.png` capture was taken in the guest. The VM was halted afterwards.

## Release receipt — v0.2.2 (task 3.2, 2026-10-14)

- Source commit: `5e39b41077e852500752b04a146c9f418f93a785`
- Release: tag `v0.2.2`; artifacts from `dist/v0.2.2/` (published, unsigned), checked against its
  `SHA256SUMS` before the run and on the VM after the copy.
- SHA-256 `rustxterm-0.2.2-windows-x86_64.zip`: `0ad0ac15579c2b0171b896e199e7e7bec0244fa0d1f74bb3d5c17769f2482ab0`
- SHA-256 `rustxterm-0.2.2-x86_64.msi`: `95cde1098b6440a1a3fa26aa4e9d4e585cfd04de916a9f1904a74fe8b5f73388`
- SHA-256 `rustxterm.exe`: `56eeb4b3dd9dcb87ae9159671b04b2790261c9aa9996c9482a8af10da5b019aa`
- SHA-256 `rustxterm-cli.exe`: `0b1928d4c69a554073a7c757bfc94ee02e177b19793beb88ee5bfa9a8d368b90`
- VM: the Windows Server 2022 test VM (lab/vagrant/windows), recreated with `vagrant up`, Windows Server 2022 Standard Evaluation
  build 10.0.20348.4648, one 1280×800 monitor, `vagrant` account (local administrator,
  `EnableLUA=0`, so the interactive token is not UAC-filtered). Portable zip extracted to
  `C:\rxt-portable`; MSI installed per-user by a second fictional standard user. GUI driven with
  `lab/vagrant/windows/rxt.sh it/click/keys/key/shot`; screenshots in the git-ignored
  `docs/prints/windows/` (names below).
- Lab: `lab/lab.sh up` on the host, reached at `10.0.2.2`. A temporary
  `netsh interface portproxy` mapped 127.0.0.1:{2121,2990,2222,2223,30000–30019} to `10.0.2.2`
  for FTP/FTPS (the lab announces PASV address 127.0.0.1); it was reset afterwards.
- Extra test setup (temporary, removed afterwards unless noted): COM1 as a VirtualBox UART in
  `tcpclient` mode to a host `socat` on 127.0.0.1:27001 (sends `tick-from-host` every 3 s, logs
  what it receives); Windows OpenSSH `ssh-agent` service enabled for W04/W06 and disabled again;
  PuTTY 0.85 `pageant.exe` (Authenticode signed by Simon Tatham) with the lab key converted to
  `.ppk`; fictional standard user `rxtother` (deleted afterwards, its profile folder remains);
  a 64 MB FAT32 VHD (detached and deleted afterwards).

| Row | Check | Evidence | Result |
|---|---|---|---|
| W01 | First-run locations | Before the first start neither `%APPDATA%\rustxterm` nor `%LOCALAPPDATA%\rustxterm` existed. After the first run: `config.toml` and eframe `data\app.ron` in `%APPDATA%\rustxterm`; `rustxterm.db`, `-wal`, `-shm`, `keys.v1` in `%LOCALAPPDATA%\rustxterm` with `D:P(A;OICI;FA;;;<vagrant SID>)` (`w01_first_run.png`, `w01_after_create.png`) | PASS |
| W02 | SSH password | Saved session `alice@10.0.2.2:2222`, unknown host key → Accept and save, password `lab-alice-pass`; shell prompt, `whoami` → `alice` (`w02_w07_ssh_x11.png`) | PASS |
| W03 | SSH key | Session `bob@10.0.2.2:2222` (bob is key-only), private key file `C:\rxtin\bob_ed25519`, SSH agent off; logged in (`w03_key_1.png`) | PASS |
| W04 | SSH via Windows OpenSSH agent | `ssh-agent` service started, `ssh-add` of the bob key; bob session without key file, "Use SSH agent" on; logged in (`w04_w06_openssh_agent.png`) | PASS |
| W05 | SSH via Pageant | OpenSSH agent stopped and its pipe gone, Pageant with `bob.ppk`; new bob login succeeded (`w05_w06_pageant.png`) | PASS |
| W06 | Agent forwarding, each provider | `ssh-add -l` on the remote lists `SHA256:vYvfrWdG… rxt-lab-bob (ED25519)` with the OpenSSH agent and again with Pageant (same screenshots as W04/W05) | PASS |
| W07 | X11 without `DISPLAY` | X11 forwarding on, no `DISPLAY`: banner "✗ X11 forwarding (no local X server is configured …)" and the tab notice; shell works, remote `DISPLAY=` empty (`w02_w07_ssh_x11.png`) | PASS |
| W08 | Jump host | `alice@internal:22` through gateway `10.0.2.2:2223`; banner "✓ Gateway connection (1 jump host)", `hostname` → `internal` (`w08_jump.png`) | PASS |
| W09 | Tunnels | Local forward `127.0.0.1:18022 → 127.0.0.1:22` via `alice@10.0.2.2`, started from the Tunneling window; TCP connect to 127.0.0.1:18022 reads `SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.19`, listener owned by the rustxterm process (`w09_tunnel_started.png`) | PASS |
| W10 | SFTP | SFTP panel lists `/home/alice`, new remote file appears after refresh, `notes.txt` downloaded to `Downloads` (transfer "Done") (`sftp_refresh.png`, `sftp_downloaded.png`) | PASS |
| W11 | FTP/FTPS | `ftp://alice@127.0.0.1:2121` login, upload of `notes.txt` (Done, listed remotely); `ftps://alice@127.0.0.1:2990` (implicit TLS) certificate prompt → Accept once, login and listing (`w11_ftp.png`, `w11_ftps.png`) | PASS |
| W12 | RDP | `rdp://alice@10.0.2.2:13389`, certificate prompt → Accept once, password; remote xterm session shown (`w12_rdp.png`) | PASS |
| W13 | VNC | `vnc://10.0.2.2:5901`, password `lab-vnc1`; remote desktop shown (`w13_vnc.png`) | PASS |
| W14 | Telnet | `telnet://alice@10.0.2.2:2323`, unencrypted warning, login, `echo telnet-ok $(whoami)` → `telnet-ok alice` (`w14_telnet.png`) | PASS |
| W15 | Serial COM1 open and I/O | Serial session `COM1 @ 115200` (session editor): host ticks shown in the tab (`w15_serial_rx.png`); typed `typed-from-rustxterm-42` + Enter received by the host (`com1_rx.log`: `typed-from-rustxterm-42\r`) | PASS |
| W16 | Serial busy | A PowerShell process (`System.IO.Ports.SerialPort`) held COM1: the tab shows "Session stopped (port in use: COM1)"; after the holder released it, R reconnected and data flowed (`w16_serial_busy.png`, `w16_serial_after_release.png`) | PASS |
| W17 | USB-serial unplug and reconnect | Not run: needs a physical USB-serial adapter passed through to the VM (no hardware here) | NOT RUN |
| W18 | Agent CLI over the pipe | `agent_control = true`, session policy Allow: `rustxterm-cli.exe open 1 --json` → `{"tab":1}`; `tabs list --json` → `[{"id":1,…,"state":"Connected"}]`, exit 0 (default policy Deny first returned `denied by policy`, exit 4) | PASS |
| W19 | MCP over the pipe | `rustxterm-cli.exe mcp`: `send_input` `uptime\n` → `{"sent_to":[1]}`, then `wait_for_output` `load average` → `matched:true`, text `… load average: 28.61, 21.45, 13.65 …`. See finding F1: pipelined calls in one MCP session fail intermittently | PASS |
| W20 | Agent control off | GUI running with `agent_control = false`: no `rustxterm-*` pipe; `tabs list --json` → `{"error":{"code":"not_running","message":"rustXterm is not running, or agent control is off"}}`, exit 5 | PASS |
| W21 | Second local user refused by the pipe | As `rxtother`: the pipe `rustxterm-<vagrant SID>-control` is listed but `NamedPipeClientStream.Connect` → "Access to the path is denied"; `rustxterm-cli tabs list` (own SID) → not_running | PASS |
| W22 | Second local user denied on DB/WAL/envelope/backup | As `rxtother` (traverse checking bypassed, full paths): opening `rustxterm.db`, `-wal`, `-shm`, `keys.v1` → `UnauthorizedAccessException`; every entry has only the vagrant SID ACE. No backup file exists in a fresh 0.2.2 store; backups are created in the same directory and inherit its protected owner-only ACE | PASS |
| W23 | Keyring unlock after restart | App closed and restarted, and again after a VM reboot: sessions unlocked without a prompt (`window_position_restored.png`, `w23_after_reboot.png`) | PASS |
| W24 | Credential deleted → master password | `cmdkey /delete:rustxterm:rustxterm-db-…`, restart: "Unlock sessions / Enter the master password" dialog, `lab-master-pass` unlocks (`w24_master_prompt.png`, `w24_unlocked.png`) | PASS |
| W25 | Credential Manager unavailable → master password | WinRM network logon: no credential visible, `sessions list --json` without a terminal → `locked … type the master password on a terminal`, exit 3. On a terminal (OpenSSH network logon with a pty) the prompt fails: `Master password: rustxterm: cannot turn off terminal echo (stty)` — finding F2, the master password cannot be entered on Windows | FAIL |
| W26 | Theme switch live | Skin = System; `AppsUseLightTheme` 1→0 → dark, 0→1 → light, same process, no restart (`w26_theme_dark.png`, `w26_theme_light.png`) | PASS |
| W27 | Toast shown | No code path in 0.2.2 calls `Notifier::notify` (only the platform adapters implement it), so no toast can be raised; the session itself shows toasts (VirtualBox Guest Additions toast seen) — finding F3 | FAIL |
| W28 | Toasts disabled → no error | Not run: blocked by W27 (nothing raises a toast) | NOT RUN |
| W29 | File dialogs modal | Session editor → Private key Browse…: `#32770` "Select private key" owned by the main window (owner HWND = main), main window disabled while open (`w29_browse_dialog.png`, EnumWindows output) | PASS |
| W30 | Open a downloaded `.txt` from the transfer list | Not possible in 0.2.2: transfer rows only show name and "Done" (no action), "Open" is disabled for files in the SFTP and local panes, double-click downloads; no UI code calls `Opener` — finding F4 | FAIL |
| W31 | Clipboard SSH tab → Notepad | Selected `clip-token-7319` in the SSH tab (copy on select), `Get-Clipboard` = `clip-token-7319`, Ctrl+V in Notepad shows it (`w31_notepad_paste.png`) | PASS |
| W32 | No console window for `rustxterm.exe` | GUI started with `Start-Process` in the desktop session: no `conhost.exe` child, only the GUI window (`w01_first_run.png`) | PASS |
| W33 | No local shell entry | New session types: SSH, Telnet, SFTP, FTP, FTPS, RDP, VNC, Serial — no local shell (`w33_new_session.png`) | PASS |
| W34 | MSI install/uninstall as a standard user, AUMID | As `rxtother` (not an administrator): `msiexec /i … /qn` exit 0, files in `%LOCALAPPDATA%\Programs\rustXterm` (exes identical to the zip), Start Menu `rustXterm.lnk` with AUMID `rustXterm.rustXterm`, `HKCU\Software\rustXterm`, nothing under Program Files, installed `rustxterm-cli.exe --self-test` 6/6; `msiexec /x … /qn` exit 0 removes dir, shortcut and key. Server 2022 defaults to `DisableMsi=1` (exit 1625 for non-admins); the policy was set to 0 for this row and the value removed afterwards | PASS |
| W35 | Offline `sessions list --json` after unlocking | GUI running, keyring unlocked: JSON list of the saved session (password shown only as `"saved"`), exit 0 | PASS |
| W36 | `sessions remove` without a terminal | stdin piped: `offline changes require the master password typed on an interactive terminal`, exit 4 (denied); list unchanged | PASS |
| W37 | MCP missing parameter | `send_input` without `text` → JSON-RPC error `-32602 missing required parameter \`text\``; following `list_tabs` answered, GUI still running | PASS |
| W38 | GUI exe given CLI work | `rustxterm.exe sessions list`, `rustxterm.exe mcp`, `rustxterm.exe --self-test` → exit 2 each, no window (`w38_no_window.png`) | PASS |
| W39 | Elevation mismatch refused with the elevation message | UAC is off on this box, so a filtered token from `runas /trustlevel:0x20000` (Administrators deny-only) stands in for "non-elevated". Filtered CLI → unfiltered GUI: refused, but `{"code":"failed","message":"control channel: io error: The pipe is being closed. (os error 232)"}`, exit 1. Unfiltered CLI → filtered GUI: refused with `control pipe is not served by rustXterm of this user`, exit 4. Same filtering on both sides works (`[]`, exit 0). Neither direction prints the elevation message — finding F5 | FAIL |
| W40 | `data_dir` on FAT → rejected, nothing created | 64 MB FAT32 VHD: `RXT_DATA_DIR=R:\rxtdata` → `must be inside your user profile folder`, exit 1; mounted at `C:\Users\vagrant\fatmnt` → `reached through a link or junction (\\?\R:\rxtdata)`, exit 1; the volume stayed empty both times | PASS |
| W41 | Other user's write/append handle, foreign read-only owner | Folders in the vagrant profile pre-created with `rxtother` Modify; `rxtother` held a write handle on `a\rustxterm.db` and an append-only handle on `b\rustxterm.db`; `c\keys.v1` owned by `rxtother`, vagrant read-only. `RXT_DATA_DIR` = a, b, c → `permission denied: cannot protect … other users could modify it … move or delete it first`, exit 1 each; SDDL and sizes of all six entries unchanged | PASS |

Additional check (fix `020cfe2` "stop pulling the window back to the primary monitor"): that fix is
**not** in v0.2.2 (it landed after the tag). On the single 1280×800 monitor, a window resized to
1000×650 and dragged to (200,150) was saved (`outer_position_pixels: (200,150)` in `app.ron`) and
reopened at (200,150) after a restart (`window_position_restored.png`). A window larger than the
monitor (the default 1280×800 inner size on this screen) saved at (160,120) reopened at (0,0): the
0.2.2 clamp refits windows that do not fit. Moving to and reopening on a secondary monitor needs
real multi-monitor hardware and was not tested.

Findings (task 3.3):

- **F1 MCP live calls fail while another call is in flight.** In one `rustxterm-cli.exe mcp`
  session, requests sent without waiting (send_input, wait_for_output, …) made `wait_for_output`
  return `not_running` in 3 of 4 runs; sequential calls and six parallel `tabs list` processes all
  succeed. Likely cause: `Server::live()` (`crates/agent/src/mcp.rs`) probes the pipe with
  `Path::exists()`, which on Windows opens the pipe as a client: it returns false while the single
  listening instance is busy (then the call falls back to the offline path → `not_running`) and the
  probe itself is an extra connection the server must accept and drop.
- **F2 master password cannot be typed on Windows.** `prompt_password` (`crates/agent/src/cli.rs`)
  runs `stty -echo`, which does not exist on Windows, so every CLI path that needs the master
  password on a terminal (offline read without keyring, `sessions add/edit/remove`, `import`) fails
  with `cannot turn off terminal echo (stty)`.
- **F3 no toast is ever raised.** `Notifier` has Windows/Linux implementations but no caller in the
  app, UI or engine.
- **F4 no "open" for downloaded or local files.** `Opener` has no caller; the transfer list has no
  action and the panes' "Open" entry is disabled for files.
- **F5 elevation mismatch not reported as such.** The server closes a refused client without a
  response (client sees os error 232, code `failed`); the client-side refusal says "not served by
  rustXterm of this user". Design W4 requires "agent control runs at a different elevation".
- Minor: quick connect `serial://COM1` opens `/dev/COM1` (not found); the serial port field
  placeholder is `/dev/ttyUSB0` on Windows; the transfer list is shared between file tabs (an FTP
  upload shows in the SSH tab's SFTP panel); save/open dialogs start in the process working
  directory (`C:\Windows\System32` when launched from the helper task).
- Lab/VM: the box adds a NAT rule `tcp53389` for RDP on all host interfaces next to the Vagrantfile's
  loopback rule (conflict warning on start); with a serial UART connected at boot, host data on COM1
  reaches the EFI console and stops the boot in the firmware menu (start the host side after logon).

Gate: **closed** — W17 and W28 not run, W25, W27, W30 and W39 FAIL. The VM was powered off (not
destroyed) after the run.
