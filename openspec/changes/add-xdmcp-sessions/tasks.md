Depends on `create-native-rust-linux-terminal` being implemented (VNC client with limits, framebuffer widget, platform seam, lab runner).

## 1. XDMCP milestone (code)

- [ ] 1.1 Confirm in a disposable container that TigerVNC `Xvnc` with `-query`, `-listen tcp`, `-rfbunixpath` and `-once` completes an XDMCP session against `xdm`, and record the minimum TigerVNC version (design X1); verify the result is noted in the design before coding further, otherwise stop and defer (option d)
- [ ] 1.2 Create `crates/xdmcp` (L2, registered in `cargo xtask layers`, no dependency on `rxt_vnc`): sans-IO Query/Willing/Unwilling codec with 1 KiB cap, probe with retries, display allocation plan, Xvnc argument builder (X2); verify unit tests with packet fixtures, truncated/oversized datagrams and argument snapshots
- [ ] 1.3 `platform` `XServerLauncher`/`XServerProcess` contract with a fake; `platform-linux` adapter (find + version, free display, spawn in own process group, bounded log, kill on drop) (X3); verify unit tests with a fake `Xvnc` script and a process-left-behind check
- [ ] 1.4 `core` `Protocol::Xdmcp`, `XdmcpOptions`; `engine` composition probe → launch → socket wait → `rxt_vnc` over Unix socket → desktop pair, cleanup on drop/app exit/panic guard (X4); verify engine unit tests with the fake launcher and an in-memory RFB peer
- [ ] 1.5 `ui`: XDMCP editor page with security warning, first-connect confirmation, Xvnc-not-found dialog with distro package names, remote-desktop tab with the `xdmcp` icon and Unencrypted marker, error states (unwilling, no answer, DM never connected); quick connect `xdmcp://`; verify kittest snapshots in light and dark
- [ ] 1.6 `store`: MobaXterm.ini XDMCP import after confirming type code `3` and field layout with sample files (fixtures hand-written, fictional); verify importer unit tests
- [ ] 1.7 Lab: `xdmcp` (`xdm`) service per X6 without published ports; add `tigervnc-standalone-server` to the toolchain image; verify `lab/lab.sh up` reports it healthy

## 2. XDMCP verification batch and push

- [ ] 2.1 Lab tests: probe Willing, Unwilling, silence; greeter frame within 20 s; login reaches the xterm session; tab close leaves no Xvnc process; missing Xvnc reported; add names to `lab/expected-tests.txt`; verify `cargo xtask lab test` green
- [ ] 2.2 Manual check on a GNOME/Wayland and a KDE/X11 desktop against the lab: the session renders inside the tab on both; record in `docs/gates/xdmcp.md`
- [ ] 2.3 UI tests: `xdmcp://` parsing (host, port, invalid), warning and confirmation flow; `cargo xtask prints` for the XDMCP editor and tab in both themes; verify green and `git status` shows no `docs/prints/` files
- [ ] 2.4 Full gates in Docker, `cargo xtask guard`, then commit and push with the task ids; verify the commit is on the remote
