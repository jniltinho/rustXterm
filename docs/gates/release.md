# Release gate receipt: v0.1.0 (tasks 10.2–10.5)

Date: 2026-10-13. Spec: `reproducible-distribution` plus the scenario map of every spec in
`openspec/specs/`.

## 10.2 Distribution matrix and GUI smoke

Commands (host needs only Docker):

```sh
docker build --target dist --build-arg RXT_VERSION=0.1.0 --output type=local,dest=dist/rc .
scripts/cargo-docker.sh cargo xtask dist-check dist/rc
scripts/distro-matrix.sh dist/rc
```

`scripts/distro-matrix.sh` runs, in one clean container per image (pinned by digest in the script,
`--network none`), the portable binary's `--self-test` and the AppImage's (`--appimage-extract`, then
`squashfs-root/AppRun --self-test`; containers have no FUSE). The GUI smoke installs Xvfb, `x11-utils`
and Mesa (`mesa-vulkan-drivers`, lavapipe software Vulkan) in a clean Ubuntu container, starts the
portable binary on `:99` with an empty `$HOME`, requires a mapped top-level window named `rustXterm`
(`xwininfo -root -tree`) and the process still alive 10 s later. `release.yml` runs the same script on
the tag's artifacts before publishing.

`dist-check dist/rc`: newest glibc symbol GLIBC_2.31 or older, `NEEDED` only libc-family libraries,
stripped, both self-tests 6/6, `SHA256SUMS` ok, guard over tarball (8 members) and AppImage
(17 members) ok. Artifacts: `rustxterm-0.1.0-x86_64` (45.2 MB), `.AppImage` (18.2 MB), `.tar.gz`
(18.7 MB). The workspace version was still `0.1.0-dev.0` for this run, so the binary reports
`0.1.0-dev.0`; the coordinator bumps it with the tag.

| Image (digest pinned in the script) | glibc | Binary `--self-test` | AppImage `--self-test` | GUI smoke |
|---|---|---|---|---|
| ubuntu:20.04 | 2.31 | pass (6/6) | pass (6/6) | — |
| ubuntu:22.04 | 2.35 | pass (6/6) | pass (6/6) | — |
| ubuntu:24.04 | 2.39 | pass (6/6) | pass (6/6) | pass: window `"rustXterm"` 1200x760, alive after 10 s |
| ubuntu:26.04 | 2.43 | pass (6/6) | pass (6/6) | pass: window `"rustXterm"` 1200x760, alive after 10 s |
| debian:12 | 2.36 | pass (6/6) | pass (6/6) | — |
| debian:13 | 2.41 | pass (6/6) | pass (6/6) | — |
| fedora:latest (43) | 2.43 | pass (6/6) | pass (6/6) | — |
| archlinux:latest | 2.44 | pass (6/6) | pass (6/6) | — |

Self-test checks: config defaults, terminal echo, theme tokens, engine start/shutdown, encrypted
database (create/unlock in a temp dir), MCP handshake.

**SSH tab in the GUI smoke.** The release binary has no test hook to open a tab from outside, and
the agent-control path (`rustxterm open`) needs a saved session in an unlocked store, which the
first-run master-password dialog blocks. Rather than adding a product hook, the "opens an SSH tab to
a loopback lab sshd" half is covered by `crates/ui/src/lab_e2e.rs::e2e_ssh_quick_connect_runs_command`:
the real `App` in a kittest harness rendered through wgpu on Mesa lavapipe (same software stack as
the smoke), quick-connecting to the lab sshd on 127.0.0.1:2222 and running a command. Last green run:
the full lab set in the M4 receipt (68 tests, `docs/gates/m4.md`), and every CI/release run executes
it through `lab/lab.sh test`. A re-run during this batch was not possible because `crates/ui` was
being edited by another agent (uncommitted `ui_scale` work did not compile); it must be green in the
release CI run.

## 10.3 CI

- `ci.yml` `checks`: fmt, clippy `-D warnings`, `cargo test --workspace` (includes UI snapshot tests in
  light and dark on lavapipe), `xtask layers`, `xtask guard --ci`, `xtask scenarios`, and `cargo deny check` (cargo-deny
  0.20.2 musl release binary, SHA-256 pinned in the workflow, run inside the toolchain container;
  config `deny.toml`).
- `ci.yml` `lab`: `lab/lab.sh up`, `lab/lab.sh test` (names must equal `lab/expected-tests.txt`),
  compose logs on failure, `lab/lab.sh down`.
- `release.yml` (tags `v*.*.*` only): reuses `ci.yml` (checks + lab), then builds `--target dist` with
  the tag version, `xtask dist-check`, `scripts/distro-matrix.sh dist`, and only then publishes.
- Both workflows pass `actionlint` 1.7.12 (pinned image); `scripts/distro-matrix.sh` passes
  shellcheck 0.11.0.

`cargo deny check` locally (toolchain container, graph limited to `x86_64-unknown-linux-gnu`):
`advisories ok, bans ok, licenses ok, sources ok`. 47 duplicate-version warnings (allowed as
warnings). One advisory is ignored with a reason in `deny.toml`: RUSTSEC-2023-0071 (`rsa`, Marvin
timing side channel; no patched release exists; reached through russh/ssh-key and picky/sspi;
rustXterm is a client and never acts as an RSA decryption oracle).

Status: **not verified** until a green run on GitHub (needs a push).

## 10.4 Desktop acceptance

Steps (each on a clean desktop VM, the Docker lab reachable from it):

1. Copy `rustxterm-X.Y.Z-x86_64.AppImage` and `SHA256SUMS`; `sha256sum -c SHA256SUMS --ignore-missing`.
2. `chmod +x` and start it from the file manager (double click) and from a terminal.
3. Window opens, first-run master-password dialog shows; create a password.
4. Quick connect `alice@<lab host>:2222`, accept the host key, password `lab-alice-pass`; run a command.
5. Switch the desktop between light and dark (GNOME: Settings → Appearance; KDE: Global Theme
   Breeze/Breeze Dark) with rustXterm theme set to System; it follows within 2 s.
6. Screenshots into `docs/prints/` (local only, never committed).

| Session | Window opens | SSH session to the lab | Theme follows desktop | Receipt |
|---|---|---|---|---|
| Ubuntu 26.04 GNOME / Wayland | pass | pass | pass | Desktop acceptance v0.2.1 (VM, agent-driven) |
| KDE Plasma / X11 | pass | pass | pass | Desktop acceptance v0.2.1 (Arch, Plasma 6 X11 session) |

Not covered: a run on real GPU hardware (the VMs render with Mesa software drivers).

### Desktop acceptance v0.2.1

Date: 2026-10-14. Artifact: the published `rustxterm-0.2.1-x86_64.AppImage` of the v0.2.1 GitHub
release (tag at `e5e3a72`), sha256 `544840622e8c4ff618d72e95020253a6df7c8c937c5e32e03bac9f2d71d5e8ba`,
checked with `sha256sum -c SHA256SUMS --ignore-missing` in each VM. Fresh VMs from
[`create-vm.sh`](../../lab/vagrant/desktop/create-vm.sh), destroyed afterwards; screenshots in
`docs/prints/vm-desktop-0.2.1/` (local).

| Step | Ubuntu 26.04 (GNOME 50.1, Wayland) | Arch (Plasma 6.7.5, Xorg `plasmax11`) |
|---|---|---|
| 1 Checksum | pass | pass |
| 2 File manager (double click) | pass: Nautilus, no prompt, window in ~6 s, dock icon | pass: Dolphin asks "Would you like to launch this file?" → Launch |
| 2 Terminal | pass (`vm-run.sh start … default`) | pass (`vm-run.sh start … x11`, `WM_CLASS rustxterm`, 256 px icon) |
| 3 Master-password dialog | pass | pass |
| 4 SSH to the lab (`alice@10.0.2.2:2222`, host key, `uname -a`) | pass | pass |
| 5 Theme System follows desktop light↔dark within 2 s | pass (`color-scheme`) | pass (Breeze ↔ Breeze Dark) |
| Renderer | Vulkan lavapipe | GL llvmpipe (no Vulkan driver) |

Found and fixed: Exit (and closing the window) quit at once with a connected SSH tab although
"Confirm before closing a connected tab" was on; it now asks first
(`crates/ui/src/tests.rs::exit_with_connected_tabs_asks_first`). Known cosmetic: on GNOME the frame
drawn by winit has no title text.

### Desktop matrix v0.2.0

Date: 2026-10-14. Artifacts: `docker build --target dist --build-arg RXT_VERSION=0.2.0` at `0e99966`
(the v0.2.0 tag commit), checked with `sha256sum -c SHA256SUMS` in every VM:

- binary `326667c1947607d96f0fa1271cc6ae157263f20b83a5120095551e70fdb08288`
- AppImage `41ae199e755eebc6eccf29431727452ae79ba58209c319ba0247163133c052f5`

VMs: VirtualBox 7.2.10, VMSVGA without 3D (Mesa software rendering), official cloud images +
cloud-init, fictional user `tester` with autologin, made with
[`lab/vagrant/desktop/create-vm.sh`](../../lab/vagrant/desktop/create-vm.sh) (same approach as
rust-microsip's `scripts/vm/create-desktop-vm.sh`; Arch publishes no VirtualBox box). The app was
started inside the logged-in session with [`vm-run.sh`](../../lab/vagrant/desktop/vm-run.sh)
(`wayland` = `WAYLAND_DISPLAY` only, `DISPLAY` unset; `x11` = `DISPLAY` only; `default` = both, as a
user launch) and driven with ydotool/xdotool ([`ydo.sh`](../../lab/vagrant/desktop/ydo.sh)); screens
were captured with `VBoxManage controlvm … screenshotpng` into `docs/prints/vm-desktop/`. SSH went to
the host's Docker lab at `10.0.2.2:2222` (needs `--nat-localhostreachable1 on`; VirtualBox 7 NAT
refuses the host loopback otherwise). The VMs were destroyed afterwards.

| Distro | Session | Artifact | Self-test | Window | Theme | Dialogs | Keyring | SSH | Clipboard | Notes |
|---|---|---|---|---|---|---|---|---|---|---|
| Ubuntu 26.04 (glibc 2.43, GNOME 50) | Wayland native | binary | 6/6 | opens; winit CSD (no title text); dock icon generic before fix B | live light→dark | portal FileChooser (GNOME), SFTP upload done | gnome-keyring: prompt to create "Default"; Cancel → inline "cancelled", vault still usable; accept → stored | quick connect + host key + password, SFTP pane | copy-on-select → `wl-paste` ok; Ctrl+Shift+V ok; Shift+Insert failed (`X11 server connection timed out`) until fix A, still fails with `DISPLAY` unset (GNOME has no data-control protocol; never the case in a real GNOME session) | Vulkan lavapipe; minimized: 0 CPU ticks/5 s, output kept flowing and showed on restore; SIGTERM while minimized exits < 1 s |
| Ubuntu 26.04 | Xwayland (X11) | AppImage | 6/6 (FUSE 3 mount, no libfuse2) | **crashed: libxkbcommon-x11 not installed by default** (fix C: clear error, exit 1); opens after `apt install libxkbcommon-x11-0`; SSD title, `_NET_WM_ICON` 256×256, `WM_CLASS` class `rustxterm` | live light↔dark | — | — | pass | Shift+Insert ok (X11 path) | |
| Ubuntu 26.04 | default (both) | binary + fixes | — | opens on Wayland; dock icon from the written entry (fix B) | — | — | — | pass | Shift+Insert and right-click paste ok | |
| Fedora 44 (glibc 2.43, GNOME) | Wayland native | AppImage | 6/6 (no libfuse2) | opens; CSD no title text; dash icon generic before fix B, rustXterm icon after | live dark | portal FileChooser; **portal masked → rfd fallback (zenity GTK dialog) opens** | gnome-keyring "Default" created; restart auto-unlocks the vault | pass | Ctrl+Shift+V ok | Vulkan lavapipe |
| Fedora 44 | Xwayland (X11) | binary | 6/6 | opens; SSD title, icon 256×256 | live dark | — | — | — | — | libxkbcommon-x11 preinstalled |
| Arch (glibc 2.44, Plasma 6) | Wayland native | binary, AppImage | 6/6 / 6/6 (no libfuse2) | opens; KWin SSD with title; window/taskbar icon generic "W" for the binary before fix B, rustXterm after | live BreezeDark | — | KWallet (ksecretd): wallet creation prompt, then restart auto-unlocks | pass (host key saved in vault) | Ctrl+Shift+V ok; **Shift+Insert and right-click paste did nothing in the default launch** (KWin does not sync the Wayland clipboard to Xwayland for a Wayland window) — fixed by fix A (ext-data-control) | **no Vulkan driver installed: wgpu fell back to GL (llvmpipe) by itself** |
| Arch (Plasma 6) | Xorg (plasmax11) | AppImage | — | opens; SSD title, taskbar + title icon, `_NET_WM_ICON` 256×256 | live BreezeDark → BreezeLight | portal FileChooser (KDE) opens | — | pass | Shift+Insert, right-click paste and copy-on-select (`xclip`) ok | |

Fixes found by this matrix (uncommitted at the time of the run):

- **A, clipboard on Wayland**: the platform clipboard (`arboard`, used by right click, Shift+Insert
  and remote-desktop clipboard sync) only spoke X11. `arboard` now has `wayland-data-control`: it uses
  the ext/wlr data-control protocol when the compositor offers it (KDE, wlroots) and falls back to
  X11/Xwayland otherwise (GNOME). Verified on Plasma Wayland and GNOME (default launch).
- **B, icon on Wayland**: GNOME and KDE take the dock/taskbar icon from the `.desktop` entry named
  after the app id, not from the window. On start the app writes
  `$XDG_DATA_HOME/applications/rustxterm.desktop` (absolute `Exec`, the `.AppImage` path inside an
  AppImage; paths needing escaping are skipped) and the 256 px icon, refreshes an entry it wrote when
  the executable moves, and never touches an entry it did not write or adds one when a system entry
  exists (`crates/platform-linux/src/desktop.rs::writes_refreshes_and_respects_foreign_entries`).
  GLib accepts the entry (`Gio.DesktopAppInfo` icon `rustxterm`); verified on Ubuntu, Fedora, Plasma.
- **C, missing libxkbcommon-x11**: winit panics when it cannot dlopen it; the binary now reports
  `cannot open the window: libxkbcommon-x11.so is not installed` and exits 1 (other panics still
  propagate). README lists the dependency.

Not changed: the Wayland frame drawn by winit on GNOME has no title text (cosmetic; winit's
`wayland-csd-adwaita` would add it); no `SIGTERM` handler exists, so a stalled frame loop on a
covered Wayland window cannot block exit (the default action ends the process).

## 10.5 Scenario map

Every `#### Scenario:` of the change's specs, with its automated test (`file::test`) or receipt.
`lab` = `#[ignore]`d test run by `lab/lab.sh test`; `receipt` = command run recorded in a gate
document; `partial` = the test covers the rule but not every literal detail of the scenario (gap
stated); `manual` = human receipt; `gap` = no test yet, closest evidence named. `cargo xtask scenarios`
(run in CI) fails unless every scenario has exactly one row and every cited `file::test` exists.

Totals: 141 scenarios, 141 mapped, 0 unmapped.

| Kind | Count |
|---|---|
| Automated (unit/integration and/or lab tests; some also cite a gate receipt) | 124 |
| Partial: automated, with the uncovered detail stated (Find text, Drag-and-drop upload, Open with local application, Offline list, Audit entry, Filter, Overflow, FreeBSD server, SFTP-only account, Agent tries to toggle, Channel limit) | 11 |
| Receipt or configuration only (Clean host build, Milestone push, Panic path rejected, Desktop session) | 4 |
| Manual receipt (Windows target, M2) | 1 |
| Gap: no test, closest evidence named (Jump host untouched) | 1 |

Narrower gaps noted inside automated rows: Distribution matrix (the Xvfb smoke opens no SSH tab; the
lab E2E does), Hidden tab (hidden for seconds, not 60 s), Global defaults / Per-session font
(resolution, not rendering), X11 forwarding (Xvfb display, real desktop in 10.4), Nothing written on the server (seconds, not one
minute), Hidden tab pauses (fake peer under paused time).

| Spec | Scenario | Evidence | Status |
|---|---|---|---|
| terminal-emulation | Full-screen application | `crates/term/tests/reference_grid.rs::vim`, `crates/term/tests/reference_grid.rs::htop`, `crates/term/tests/reference_grid.rs::tmux`, `crates/ui/src/terminal/tests.rs::mouse_clicks_report_sgr_and_x10` | auto |
| terminal-emulation | Resize propagation | `crates/ssh/tests/lab_gates.rs::ssh_gate_flood_input_echo_resize_one_connection` (window-change ≤ 50 ms, `stty size` ≤ 500 ms, docs/gates/m1.md), `crates/ui/src/terminal/tests.rs::reports_resize_in_cells` | lab + receipt |
| terminal-emulation | Find text | `crates/term/src/lib.rs::search_next_prev_and_case` | partial: next/previous, case and visible-match highlighting tested on a short buffer, not 5000 lines |
| terminal-emulation | Multi-line paste guard | `crates/ui/src/terminal/tests.rs::paste_guard_and_bracketed_paste` | auto |
| terminal-emulation | Select all from the menu | `crates/ui/src/tests.rs::terminal_context_menu`, `crates/ui/src/terminal/tests.rs::ctrl_right_click_or_paste_setting_off_opens_menu`, `crates/term/src/lib.rs::select_all_clear_history_and_reset` | auto |
| terminal-emulation | Global defaults | `crates/core/src/session.rs::terminal_prefs_inherit_global` | auto (preference resolution; rendering of the cursor shape is not asserted) |
| terminal-emulation | Per-session font | `crates/core/src/session.rs::terminal_prefs_inherit_global` | auto (resolution level) |
| terminal-emulation | Log to file | `crates/term/src/lib.rs::log_raw_and_plain`, `crates/term/src/lib.rs::log_open_error` | auto (fed bytes, not a live `ls`) |
| terminal-emulation | Output flood | `crates/ssh/tests/lab_gates.rs::ssh_gate_flood_input_echo_resize_one_connection`, `crates/ui/src/lab_gates.rs::ui_gate_flood_tab_switch_echo_memory_resize`, `crates/ui/src/tests.rs::flooding_visible_tab_does_not_starve_background_tab` | lab + receipt (docs/gates/m1.md) |
| remote-desktop | Connect to lab xrdp | `crates/rdp/tests/lab_rdp.rs::rdp_login_alice_updates_and_input`, `crates/ui/src/lab_e2e.rs::e2e_rdp_desktop_renders_and_accepts_input` | lab |
| remote-desktop | Unknown certificate | `crates/rdp/tests/lab_rdp.rs::rdp_cert_unknown_prompt_then_accept_and_save`, `crates/rdp/tests/lab_rdp.rs::rdp_cert_cancelled_asks_no_password`, `crates/rdp/src/trust.rs::trust_decisions_gate_the_password_prompt` | lab |
| remote-desktop | Changed certificate | `crates/rdp/tests/lab_rdp.rs::rdp_cert_changed_refused_without_password` | lab |
| remote-desktop | Resize | `crates/rdp/tests/lab_rdp.rs::rdp_resize_via_display_control`, `crates/rdp/tests/lab_rdp.rs::rdp_fixed_resolution_ignores_tab_size`, `crates/rdp/src/session.rs::display_control_adjustments`, `crates/ui/src/desktop.rs::pointer_coordinates_invert_scaling` | lab |
| remote-desktop | Windows target | `crates/rdp/tests/vm_rdp.rs::rdp_windows_nla_login_input_and_resize` (Windows VM, docs/gates/m2.md) | manual receipt |
| remote-desktop | Connect to lab VNC | `crates/vnc/tests/lab_vnc.rs::vnc_auth_password`, `crates/vnc/tests/lab_vnc.rs::vnc_encoding_copyrect_on_xterm_scroll`, `crates/ui/src/lab_e2e.rs::e2e_vnc_desktop_renders` | lab |
| remote-desktop | View only | `crates/vnc/tests/lab_vnc.rs::vnc_view_only_sends_no_input`, `crates/vnc/src/lib.rs::view_only_suppresses_all_input`, `crates/ui/src/tests.rs::view_only_sends_no_input` | lab + auto |
| remote-desktop | Through SSH | `crates/vnc/tests/lab_vnc.rs::vnc_through_ssh_direct_tcpip_without_local_listener`, `crates/ui/src/lab_e2e.rs::e2e_vnc_via_ssh_gateway_renders` | lab |
| remote-desktop | Release key | `crates/ui/src/tests.rs::keyboard_grab_and_release_combination`, `crates/ui/src/desktop.rs::release_combination_parses` | auto |
| remote-desktop | Hostile server sizes | `crates/vnc/tests/rfb.rs::hostile_framebuffer_size_is_rejected`, `crates/vnc/tests/rfb.rs::hostile_huge_compressed_rect_ends_session`, `crates/vnc/tests/rfb.rs::hostile_oversized_rect_ends_session`, `crates/rdp/tests/hostile_rdp.rs::oversized_desktop_rejected_in_capability_exchange`, `crates/rdp/tests/hostile_rdp.rs::oversized_pdu_drops_connection`, `crates/engine/src/tests.rs::oversized_desktop_ends_tab_with_error` | auto |
| remote-desktop | Stalled peer | `crates/vnc/tests/rfb.rs::hostile_stall_mid_message_times_out_after_30s`, `crates/vnc/tests/rfb.rs::drop_during_stall_returns_immediately_and_releases_the_connection`, `crates/rdp/tests/hostile_rdp.rs::stalled_peer_mid_pdu_errors_and_drop_closes_promptly`, `crates/rdp/tests/hostile_rdp.rs::stalled_peer_during_connection_times_out` | auto |
| remote-desktop | Hidden tab | `crates/vnc/tests/lab_vnc.rs::vnc_hidden_tab_sends_no_requests_and_redraws_within_500ms`, `crates/vnc/tests/rfb.rs::view_only_sends_no_input_and_hidden_sends_no_requests`, `crates/ui/src/tests.rs::hidden_desktop_tabs_stop_updates` | lab + auto (hidden for seconds, not 60 s) |
| remote-desktop | Debug logging | `crates/engine/tests/lab_logs.rs::engine_debug_log_has_no_secrets` | lab |
| reproducible-distribution | Clean host build | `docker build --target dist` (10.2 above), `release.yml` | receipt |
| reproducible-distribution | glibc baseline | `xtask/src/dist.rs::glibc_versions_compare_numerically`, `cargo xtask dist-check` (10.2 above) | auto + receipt |
| reproducible-distribution | Distribution matrix | `scripts/distro-matrix.sh` (10.2 above); SSH-tab half: `crates/ui/src/lab_e2e.rs::e2e_ssh_quick_connect_runs_command` | receipt + lab (gap: the smoke itself opens no SSH tab) |
| reproducible-distribution | Desktop session | 10.4 above (Desktop acceptance v0.2.1, Desktop matrix v0.2.0) | receipt (VM; real GPU not covered) |
| reproducible-distribution | Lab integration test | `lab/lab.sh test`, `xtask/src/lab.rs::renamed_test_fails_comparison`, `xtask/src/lab.rs::compare_reports_missing_and_unexpected` | lab + auto |
| reproducible-distribution | Panic path rejected | workspace lints `unwrap_used = "deny"`, `expect_used = "deny"` in `Cargo.toml`, clippy `-D warnings` in `ci.yml` | config (gap: no fixture test that adds an `.unwrap()` and expects clippy to fail) |
| reproducible-distribution | Guard covers history and releases | `xtask/src/guard.rs::committed_base_path_fails_even_after_removal`, `xtask/src/guard.rs::renamed_forbidden_copy_fails_locally_not_in_ci`, `xtask/src/guard.rs::home_path_only_in_history_fails`, `xtask/src/guard.rs::artifact_members_are_scanned` | auto |
| reproducible-distribution | Asset provenance | `xtask/src/guard.rs::missing_attribution_row_fails`, `xtask/src/guard.rs::unlisted_font_file_fails` | auto |
| reproducible-distribution | Layer violation | `xtask/src/layers.rs::violations_are_reported`, `xtask/src/layers.rs::clean_graph_passes` | auto |
| reproducible-distribution | Milestone push | `git log` M1–M4 commit subjects with task ids; guard run before each push (docs/gates/m1.md, m2.md, m4.md) | receipt (process) |
| desktop-shell | Driver crash on OpenGL | `apps/rustxterm/src/gpu.rs::plan_and_what_is_saved`, `apps/rustxterm/src/gpu.rs::marker_held_is_not_a_crash_and_left_marker_is`, `crates/ui/src/tests.rs::gpu_start_saves_a_working_fallback_after_painting` | auto (a real driver crash is not reproduced) |
| desktop-shell | Instance still starting | `apps/rustxterm/src/gpu.rs::marker_held_is_not_a_crash_and_left_marker_is` | auto |
| desktop-shell | First launch | `crates/ui/src/tests.rs::snapshot_main_empty`, `crates/ui/src/tests.rs::first_run_creates_database_and_sessions_persist` | auto |
| desktop-shell | Layout persists | `crates/ui/src/tests.rs::layout_and_sessions_persist` | auto |
| desktop-shell | Switch theme | `crates/ui/src/tests.rs::theme_follows_system_live`, `crates/ui/src/theme.rs::scheme_defaults_follow_theme` | auto |
| desktop-shell | Follow system | `crates/ui/src/tests.rs::theme_follows_system_live`, `crates/platform-linux/src/theme.rs::listeners_fire_on_change_only` | auto (live desktop switch: 10.4) |
| desktop-shell | Portal unavailable | `crates/ui/src/tests.rs::portal_missing_falls_back_to_light`, `crates/platform-linux/src/theme.rs::missing_bus_falls_back_quickly`, `crates/core/src/theme.rs::system_falls_back_to_light` | auto |
| desktop-shell | Tab operations | `crates/ui/src/tests.rs::tab_operations`, `crates/ui/src/tabs.rs::move_tab_reorders_and_ignores_out_of_range` | auto |
| desktop-shell | Close live tab | `crates/ui/src/tests.rs::closing_live_tab_asks_and_cancel_keeps_connection` | auto |
| desktop-shell | Disconnected tab | `crates/ui/src/tests.rs::snapshot_disconnected_tab_offers_reconnect`, `crates/engine/src/tests.rs::exit_keeps_tab_drop_schedules_reconnect_command_closes`, `crates/ssh/tests/lab_ssh.rs::ssh_disconnect_detected_then_reconnect` | auto + lab |
| desktop-shell | Quick SSH | `crates/core/src/quick_connect.rs::plain_ssh`, `crates/ui/src/tests.rs::quick_connect_opens_unsaved_session`, `crates/ui/src/lab_e2e.rs::e2e_ssh_quick_connect_runs_command` | auto + lab |
| desktop-shell | Protocol prefixes | `crates/core/src/quick_connect.rs::prefixes` | auto |
| desktop-shell | Invalid input | `crates/core/src/quick_connect.rs::invalid`, `crates/ui/src/tests.rs::quick_connect_error_is_inline_and_opens_nothing` | auto |
| desktop-shell | Marking and toggle | `crates/ui/src/tests.rs::multiexec_toggle_and_selection`, `crates/ui/src/tests.rs::snapshot_multiexec` | auto |
| desktop-shell | Broadcast command | `crates/engine/src/tests.rs::multiexec_fans_out_only_when_enabled` | auto |
| desktop-shell | Shortcut does not steal terminal input | `crates/ui/src/tests.rs::ctrl_c_reaches_terminal_and_fires_no_action`, `crates/ui/src/terminal/tests.rs::ctrl_c_sends_etx_and_is_not_leaked` | auto |
| desktop-shell | Capture | `xtask/src/prints.rs::copies_only_final_snapshots`, `cargo xtask prints` (`docs/prints/` is git-ignored and guard-checked) | auto + receipt |
| desktop-shell | Icon override | `crates/ui/src/icons.rs::override_dir_wins_and_falls_back` | auto |
| desktop-shell | Corrupt settings file | `crates/store/tests/settings.rs::corrupt_toml_is_backed_up_with_notice`, `crates/ui/src/tests.rs::settings_precedence_sets_theme_and_notice_names_file` | auto |
| file-transfer | Browser follows SSH tab | `crates/ui/src/tests.rs::sftp_follows_active_tab_and_terminal_folder`, `crates/ui/src/lab_e2e.rs::e2e_sftp_side_panel_follows_ssh_tab` | auto + lab |
| file-transfer | cd follows | `crates/sftp/tests/lab_sftp.rs::sftp_follows_osc7_from_shell`, `crates/term/src/lib.rs::osc7_events_and_replies` | lab |
| file-transfer | Drag-and-drop upload | `crates/ui/src/file_tab.rs::drops_map_to_transfers`, `crates/sftp/tests/lab_sftp.rs::sftp_100mb_put_get_checksum`, `crates/ui/src/tests.rs::snapshot_sftp_panel_and_transfers` | partial: drop → transfer mapping and a 100 MB transfer are tested separately; a real file-manager drop is not automated |
| file-transfer | Open with local application | `crates/sftp/tests/lab_sftp.rs::sftp_open_with_upload_back`, `crates/sftp/src/transfer.rs::open_with_round_trip` | partial: edit and upload-back tested; deletion of the temporary copy on tab close is not asserted |
| file-transfer | Overwrite | `crates/sftp/tests/lab_sftp.rs::sftp_overwrite_decided_by_stat_before_put` | lab |
| file-transfer | Cancel | `crates/sftp/tests/lab_sftp.rs::sftp_cancel_removes_partials`, `crates/sftp/src/transfer.rs::cancelled_get_future_leaves_no_partial_file`, `crates/ftp/tests/lab_ftp.rs::ftp_cancel_removes_partials` | lab + auto |
| file-transfer | Open SFTP session | `crates/ui/src/lab_e2e.rs::e2e_sftp_standalone_dual_pane_transfer`, `crates/ui/src/tests.rs::snapshot_file_tabs` | lab + auto |
| file-transfer | Dual-pane copy | `crates/ui/src/tests.rs::file_tab_drag_between_panes_transfers`, `crates/ui/src/lab_e2e.rs::e2e_sftp_standalone_dual_pane_transfer` | auto + lab |
| file-transfer | Explicit FTPS | `crates/ftp/tests/lab_ftp.rs::ftp_explicit_tls_prompt_shows_fingerprint_then_lists`, `crates/ui/src/lab_e2e.rs::e2e_ftps_explicit_cert_prompt_then_listing` | lab |
| file-transfer | Plain FTP warning | `crates/ui/src/lab_e2e.rs::e2e_ftp_plain_marker_and_listing`, `crates/ftp/tests/lab_ftp.rs::ftp_plain_login_list_and_file_ops` | lab |
| file-transfer | Passive and active | `crates/ftp/tests/lab_ftp.rs::ftp_passive_10mb_put_get_checksum`, `crates/ftp/tests/lab_ftp.rs::ftp_active_10mb_put_get_checksum` | lab |
| ssh-sessions | Key authentication | `crates/ssh/tests/lab_ssh.rs::ssh_auth_publickey_bob` | lab |
| ssh-sessions | Wrong password | `crates/ssh/tests/lab_ssh.rs::ssh_auth_wrong_password_then_retry`, `crates/engine/src/tests.rs::saved_password_skips_prompt_and_rejected_one_is_forgotten` | lab + auto |
| ssh-sessions | Revoked key | `crates/ssh/src/known_hosts.rs::revoked_key_is_refused`, `crates/ssh/tests/lab_ssh.rs::ssh_hostkey_revoked_refused` | auto + lab |
| ssh-sessions | Unreadable known_hosts | `crates/ssh/src/known_hosts.rs::unreadable_file_fails_closed_naming_it`, `crates/ssh/tests/lab_ssh.rs::ssh_hostkey_unreadable_known_hosts_blocks` | auto + lab |
| ssh-sessions | Multiple algorithms | `crates/ssh/src/known_hosts.rs::different_algorithms_are_not_a_conflict` | auto |
| ssh-sessions | Hashed host on custom port | `crates/ssh/src/known_hosts.rs::hashed_host_on_custom_port` | auto |
| ssh-sessions | Changed host key | `crates/ssh/src/known_hosts.rs::same_algorithm_conflict_is_changed`, `crates/ssh/tests/lab_ssh.rs::ssh_hostkey_changed_refused_without_auth` | auto + lab |
| ssh-sessions | Through a bastion | `crates/ssh/tests/lab_ssh.rs::ssh_jump_chain_verifies_each_hop_before_auth`, `crates/ssh/src/lib.rs::hops_chain_jump_hosts_first` | lab + auto |
| ssh-sessions | X11 forwarding | `crates/ssh/tests/lab_ssh.rs::ssh_x11_forwarding_with_cookie_substitution` | lab (Xvfb display; window on a real desktop: 10.4) |
| ssh-sessions | Compression and keepalive | `crates/ssh/tests/lab_ssh.rs::ssh_compression_and_keepalive_in_server_log`, `crates/ssh/src/lib.rs::compression_prefers_openssh_zlib` | lab |
| ssh-sessions | Bad X11 cookie | `crates/ssh/tests/lab_ssh.rs::ssh_x11_forwarding_with_cookie_substitution`, `crates/ssh/tests/lab_ssh.rs::ssh_x11_not_requested_has_no_display` | lab |
| ssh-sessions | Command completes | `crates/ssh/tests/lab_ssh.rs::ssh_remote_command_output_then_exit_status`, `crates/engine/src/tests.rs::exit_keeps_tab_drop_schedules_reconnect_command_closes` | lab + auto |
| ssh-sessions | Startup directory and environment | `crates/ssh/tests/lab_ssh.rs::ssh_environment_and_startup_directory`, `crates/ssh/src/lib.rs::startup_command_quotes_dir` | lab |
| ssh-sessions | Agent forwarding off by default | `crates/core/src/session.rs::agent_and_x11_forwarding_off_by_default` | auto |
| ssh-sessions | Local forward | `crates/ssh/tests/lab_ssh.rs::ssh_tunnel_local_forward_via_bastion`, `crates/ui/src/lab_e2e.rs::e2e_tunnel_local_forward_through_bastion` | lab |
| ssh-sessions | Not exposed by default | `crates/ssh/src/tunnel.rs::tunnel_bind_defaults_are_loopback`, `crates/ssh/tests/lab_ssh.rs::ssh_tunnel_remote_forward_loopback_only`, `crates/core/src/session.rs::tunnel_not_exposed_without_confirmation`, `crates/ui/src/tunnels.rs::exposure_requires_confirmation` | auto + lab |
| ssh-sessions | Autostart | `crates/ui/src/tests.rs::tunnel_start_and_autostart` | auto |
| ssh-sessions | Port in use | `crates/ssh/src/tunnel.rs::port_in_use_error_names_port` | auto |
| ssh-sessions | Network drop | `crates/ssh/tests/lab_ssh.rs::ssh_disconnect_detected_then_reconnect`, `crates/ssh/src/tunnel.rs::backoff_grows_and_caps`, `crates/ui/src/tests.rs::snapshot_disconnected_tab_offers_reconnect` | lab + auto |
| agent-control | Offline list | `crates/agent/src/mcp.rs::offline_reads_with_keyring_redact_and_audit`, `crates/agent/src/cli.rs::session_line_has_no_secrets`, `apps/rustxterm/tests/cli.rs::offline_reads_without_terminal_or_keyring_are_locked` | partial: offline reads tested through MCP with the keyring and the locked CLI path; `sessions list --json` after a typed master password is not a separate test |
| agent-control | Offline change without a terminal | `apps/rustxterm/tests/cli.rs::offline_changes_without_terminal_are_denied_and_change_nothing` | auto |
| agent-control | Not running | `apps/rustxterm/tests/cli.rs::live_commands_without_instance_are_not_running`, `crates/agent/src/client.rs::no_instance_is_not_running` | auto |
| agent-control | Other user | `crates/platform-linux/src/control.rs::other_uid_is_refused`, `lab/agent/uid-test.sh` (two UIDs, docs/gates/m4.md) | auto + receipt |
| agent-control | Disabled | `apps/rustxterm/tests/agent.rs::agent_control_disabled_has_no_socket` | auto |
| agent-control | Agent runs a command | `apps/rustxterm/tests/agent.rs::agent_lab_mcp_send_input_and_wait_for_output` | lab |
| agent-control | Invalid parameters | `crates/agent/src/mcp.rs::invalid_params_and_unknown_tools_keep_running`, `apps/rustxterm/tests/agent.rs::agent_cli_and_mcp_processes_over_real_socket` | auto |
| agent-control | MCP mutation | `crates/agent/src/mcp.rs::exactly_the_matrix_columns`, `apps/rustxterm/tests/agent.rs::agent_cli_and_mcp_processes_over_real_socket` | auto |
| agent-control | Offline CLI edit | `apps/rustxterm/tests/agent.rs::offline_owner_changes_need_a_terminal` | auto |
| agent-control | Owner edit of a deny session | `apps/rustxterm/tests/agent.rs::offline_owner_changes_need_a_terminal`, `crates/engine/src/tests/agent.rs::offline_owner_changes_and_defaults` | auto |
| agent-control | Offline creation defaults | `apps/rustxterm/tests/agent.rs::offline_owner_changes_need_a_terminal`, `crates/engine/src/tests/agent.rs::offline_owner_changes_and_defaults` | auto |
| agent-control | Direct socket bypass | `apps/rustxterm/tests/agent.rs::raw_socket_gui_only_and_policy_escalation_denied`, `crates/engine/src/tests/agent.rs::control_channel_round_trip_raw_bypass_and_disable` | auto |
| agent-control | Policy escalation through edit | `apps/rustxterm/tests/agent.rs::raw_socket_gui_only_and_policy_escalation_denied`, `crates/core/src/control.rs::patch_rejects_gui_only_fields` | auto |
| agent-control | Denied by default | `crates/engine/src/tests/agent.rs::deny_policy_blocks_open_and_allow_opens` | auto |
| agent-control | Ask | `crates/engine/src/tests/agent.rs::ask_waits_for_gui_approval_and_replay_needs_a_new_one`, `apps/rustxterm/tests/agent.rs::agent_lab_ask_approval_replay_and_changed_params`, `crates/ui/src/tests.rs::snapshot_approval_prompt_and_answers` | auto + lab |
| agent-control | Replay or changed parameters | `apps/rustxterm/tests/agent.rs::agent_lab_ask_approval_replay_and_changed_params`, `apps/rustxterm/tests/agent.rs::agent_approval_expires_after_ttl`, `crates/engine/src/tests/agent.rs::approvals_are_single_use_bound_and_expiring` | lab + auto |
| agent-control | Content read off | `apps/rustxterm/tests/agent.rs::content_read_denied_without_flag`, `crates/engine/src/tests/agent.rs::content_read_flag_gates_read_and_tail_strips_escapes` | auto |
| agent-control | MultiExec | `apps/rustxterm/tests/agent.rs::agent_lab_multiexec_reaches_only_allow_tabs`, `crates/engine/src/tests/agent.rs::multiexec_broadcast_reaches_only_allow_tabs` | lab + auto |
| agent-control | Escape attempt | `crates/engine/src/tests/agent.rs::workspace_escape_is_denied_and_audited`, `crates/engine/src/workspace.rs::escapes_are_refused` | auto |
| agent-control | Show session | `crates/engine/src/tests/agent.rs::session_output_and_audit_hold_no_secrets`, `crates/agent/src/mcp.rs::offline_reads_with_keyring_redact_and_audit` | auto |
| agent-control | Failed login error | `apps/rustxterm/tests/agent.rs::agent_lab_failed_login_discloses_no_secret` | lab |
| agent-control | Audit entry | `apps/rustxterm/tests/agent.rs::agent_lab_ask_approval_replay_and_changed_params` (approved `send` audited `approved`/`success` with the client name), `crates/ui/src/tests.rs::snapshot_audit_view_pages_and_clear` | partial: the approved-and-completed audit row is tested for `send`, not for `put` |
| session-management | Precedence | `crates/store/tests/settings.rs::env_file_beats_toml`, `crates/store/tests/settings.rs::process_env_beats_env_file_and_cli_beats_both`, `crates/ui/src/tests.rs::settings_precedence_sets_theme_and_notice_names_file` | auto |
| session-management | Secret in TOML | `crates/store/tests/settings.rs::secret_key_in_toml_is_refused_naming_key_and_file` | auto |
| session-management | Corrupt TOML | `crates/store/tests/settings.rs::corrupt_toml_is_backed_up_with_notice`, `crates/store/tests/settings.rs::wrong_value_type_is_treated_as_corrupt` | auto |
| session-management | Encrypted at rest | `crates/store/tests/db.rs::plaintext_scan`, `crates/store/tests/db.rs::create_open_wrong_key_and_mode` | auto |
| session-management | Crash during write | `crates/store/tests/db.rs::kill_during_write`, `crates/store/tests/db.rs::kill_child_writer` | auto |
| session-management | Migration | `crates/store/tests/db.rs::migration_upgrade_backup_and_rollback` | auto |
| session-management | First start | `crates/ui/src/tests.rs::snapshot_first_run_master_password`, `crates/ui/src/tests.rs::first_run_creates_database_and_sessions_persist` | auto |
| session-management | Keyring convenience | `crates/store/tests/envelope.rs::keyring_wrap_and_master_fallback`, `crates/ui/src/tests.rs::keyring_unlocks_silently` | auto |
| session-management | Wrong master password | `crates/store/tests/envelope.rs::wrong_password_delays_increase_and_reset`, `crates/ui/src/tests.rs::snapshot_unlock_dialog_and_wrong_password` | auto |
| session-management | Interrupted password change | `crates/store/tests/envelope.rs::interrupted_rewrap_keeps_old_or_new` | auto |
| session-management | Restore backup without keyring | `crates/store/tests/envelope.rs::keyring_wrap_and_master_fallback` | auto |
| session-management | Locked database | `crates/ui/src/tests.rs::snapshot_locked_tree_and_quick_connect_still_works`, `crates/ui/src/lab_e2e.rs::e2e_locked_store_quick_connect_still_works` | auto + lab |
| session-management | Save password | `crates/ui/src/tests.rs::save_password_checkbox_requests_saving_before_answer`, `crates/engine/src/tests.rs::saved_password_skips_prompt_and_rejected_one_is_forgotten`, `crates/store/tests/db.rs::plaintext_scan` | auto |
| session-management | Filter | `crates/ui/src/sidebar.rs::filter_matches_name_host_user`, `crates/ui/src/tests.rs::snapshot_sessions_tree_and_recent` | partial: name/host/user matching tested and the filtered tree is snapshotted; parent-folder expansion is only visible in the snapshot, not asserted |
| session-management | Invalid port | `crates/ui/src/tests.rs::invalid_port_shows_error_and_disables_save` | auto |
| session-management | Import bookmarks | `crates/store/tests/moba_ini.rs::ten_ssh_in_two_folders_and_one_skipped_rsh`, `crates/ui/src/tests.rs::snapshot_import_preview_and_confirm` | auto |
| session-management | Malformed line | `crates/store/tests/moba_ini.rs::malformed_lines_are_skipped_with_line_numbers` | auto |
| session-management | Source untouched | `crates/store/tests/moba_ini.rs::source_file_is_byte_identical_after_import` | auto |
| session-management | Round trip | `crates/store/tests/export.rs::round_trip_is_identical`, `crates/store/tests/export.rs::credential_keys_in_imported_file_are_dropped` | auto |
| desktop-shell | Bar appears | `crates/engine/tests/lab_monitor.rs::ssh_monitoring_bar_data_then_no_leftover_process` (host, CPU, memory, net, uptime and `/` within 5 s), `crates/ui/src/tests.rs::monitoring_bar_takes_samples_closes_and_sftp_button_toggles` | lab + auto |
| desktop-shell | Close button | `crates/ui/src/tests.rs::monitoring_bar_takes_samples_closes_and_sftp_button_toggles`, `crates/engine/src/tests.rs::monitoring_samples_toggle_reconnect_and_close_the_channel` | auto |
| desktop-shell | Overflow | `crates/ui/src/tests.rs::snapshot_monitoring_bar` | partial: arrows appear and scroll one chip at a time; the × staying visible is only in the snapshot, not asserted |
| desktop-shell | Dark theme thresholds | `crates/ui/src/monitor_bar.rs::thresholds_and_history`, `crates/ui/src/tests.rs::snapshot_monitoring_bar` | auto (97 % CPU rendered in the dark snapshot; the danger level is asserted) |
| desktop-shell | Session opt-out | `crates/engine/tests/lab_monitor.rs::ssh_monitoring_bar_data_then_no_leftover_process` (session Off, global on: no `rxt-mon` process), `crates/engine/src/tests.rs::monitoring_follows_settings_and_only_ssh_terminals` | lab + auto |
| desktop-shell | Interval change | `crates/engine/src/monitor/tests.rs::cadence_pause_until_taken_and_live_interval`, `crates/ui/src/tests.rs::monitoring_never_shown_for_telnet_and_settings_reach_the_backend` | auto |
| desktop-shell | Toggle is per tab | `crates/ui/src/tests.rs::monitoring_bar_takes_samples_closes_and_sftp_button_toggles` | auto |
| desktop-shell | Turn on from the panel | `crates/ui/src/tests.rs::monitoring_bar_takes_samples_closes_and_sftp_button_toggles`, `crates/ui/src/monitor_bar.rs::thresholds_and_history` (history cleared on re-enable), `crates/engine/src/tests.rs::monitoring_samples_toggle_reconnect_and_close_the_channel` | auto |
| remote-monitoring | Nothing written on the server | `crates/engine/tests/lab_monitor.rs::ssh_monitoring_bar_data_then_no_leftover_process` (files under `$HOME`/`/tmp` unchanged, exactly one `rxt-mon` process), `crates/engine/src/monitor/tests.rs::script_is_fixed_and_marked` | lab + auto (monitoring runs for seconds, not one minute) |
| remote-monitoring | Jump host untouched | `Client::exec_stream` in `crates/ssh/src/lib.rs` opens the channel on the final-target handle (code reading only) | gap: no test monitors a session through a jump host |
| remote-monitoring | First samples | `crates/engine/src/monitor/tests.rs::ubuntu_first_then_second_sample`, `crates/engine/tests/lab_monitor.rs::ssh_monitoring_bar_data_then_no_leftover_process` | auto + lab |
| remote-monitoring | Container mounts filtered | `crates/engine/src/monitor/tests.rs::loop_tmpfs_overlay_filtered_and_capped` | auto (fixture has loop, devtmpfs, overlay and proc; plain tmpfs falls under the same non-`/dev/` rule) |
| remote-monitoring | Counter reset | `crates/engine/src/monitor/tests.rs::counter_reset_and_wrap_skip_values` | auto |
| remote-monitoring | FreeBSD server | `crates/engine/src/monitor/tests.rs::freebsd_like_server_has_host_and_disks_only`, `crates/ui/src/tests.rs::snapshot_monitoring_bar` | partial: host and disk chips only are tested; the host tooltip text is not asserted |
| remote-monitoring | SFTP-only account | `crates/engine/src/monitor/tests.rs::unsupported_header_closes_within_10_s` | partial: fake peer with an SFTP-only banner is unsupported at 10 s; no lab account with a forced SFTP command, terminal and SFTP panel not exercised |
| remote-monitoring | Hidden tab pauses | `crates/engine/src/monitor/tests.rs::cadence_pause_until_taken_and_live_interval` | auto (fake peer, paused time) |
| remote-monitoring | No leftover processes | `crates/engine/tests/lab_monitor.rs::ssh_monitoring_bar_data_then_no_leftover_process`, `crates/engine/src/monitor/tests.rs::toggle_off_closes_channel_and_refused_reopen_is_not_retried`, `crates/engine/src/monitor/tests.rs::dropped_with_the_run_closes_channel` | lab + auto |
| remote-monitoring | Server refuses the extra channel | `crates/engine/tests/lab_monitor.rs::ssh_monitoring_refused_channel_is_unavailable_and_shell_works`, `crates/engine/src/monitor/tests.rs::toggle_off_closes_channel_and_refused_reopen_is_not_retried` | lab + auto |
| remote-monitoring | Telnet tab | `crates/ui/src/tests.rs::monitoring_never_shown_for_telnet_and_settings_reach_the_backend`, `crates/engine/src/tests.rs::monitoring_follows_settings_and_only_ssh_terminals` | auto |
| remote-monitoring | Agent tries to toggle | `crates/core/src/command.rs::monitoring_ids_are_gui_only`, `crates/agent/src/mcp.rs::live_calls_carry_client_name_and_audit_unknown_tools`, `crates/engine/src/tests/agent.rs::matrix_per_entry_point` | partial: the ids are GUI-only and GUI-only MCP calls are rejected and audited, but no test sends `tab.monitoring.toggle` itself |
| ssh-sessions | Opt-out sends no exec | `crates/engine/src/tests.rs::monitoring_follows_settings_and_only_ssh_terminals` (exec-stream opens counted), `crates/engine/tests/lab_monitor.rs::ssh_monitoring_bar_data_then_no_leftover_process` (session Off: no `rxt-mon` process) | auto + lab |
| ssh-sessions | Channel limit | `crates/engine/tests/lab_monitor.rs::ssh_monitoring_refused_channel_is_unavailable_and_shell_works`, `crates/ui/src/tests.rs::monitoring_bar_takes_samples_closes_and_sftp_button_toggles` (unavailable reason, button disabled) | partial: the shell keeps working in the lab (`MaxSessions 2`); the SFTP panel is not exercised there |
