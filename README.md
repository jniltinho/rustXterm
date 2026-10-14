<div align="center">

<img src="assets/icons/rustxterm.svg" width="96" alt="rustXterm logo">

# rustXterm

**A native Rust terminal and remote session manager for Linux (Windows preview).**

[![Release](https://img.shields.io/github/v/release/jniltinho/rustXterm?include_prereleases&sort=semver&color=blue)](https://github.com/jniltinho/rustXterm/releases)
[![CI](https://github.com/jniltinho/rustXterm/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/jniltinho/rustXterm/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

[Install](#install) · [Quick start](#quick-start) · [Build](#build-from-source) · [CLI and MCP](#cli-and-mcp) · [Docs](#documentation)

</div>

## Why rustXterm

rustXterm brings the familiar "session tree + tabbed terminals + file panels + remote desktops" workflow
to the desktop as one fast, themed binary: no Electron, no webview. Sessions and
credentials live in an encrypted store, and AI agents can drive it through a CLI and an MCP server under
explicit, audited permissions.

It is written from scratch in Rust: native SSH, SFTP, FTP/FTPS, RDP, VNC, Telnet and Serial clients,
an embedded terminal emulator, live remote monitoring and an audited agent-control API, all in one
binary with light and dark themes.

## Features

| Area | What you get |
|---|---|
| **SSH** | Password, public key, agent and keyboard-interactive auth; host keys checked before auth on every hop; jump hosts; compression; remote command; X11 forwarding (off by default) |
| **Terminal** | xterm-256color with truecolor, mouse reporting, scrollback, search, paste confirmation; MultiExec typing into selected tabs; bundled Cascadia Mono (default), JetBrains Mono, Hack and DejaVu Sans Mono; bar, block or underline cursor |
| **Files** | SFTP side panel that follows the shell's directory, with Name, Size (KB), Last modified, Owner, Group and Access columns; dual-pane SFTP and FTP/FTPS tabs (explicit/implicit TLS, passive/active); per-tab transfer list with Open and Show in folder for downloads |
| **Telnet and Serial** | Telnet sessions (raw TCP mode, optional auto-login, plaintext warning); serial consoles with port discovery and line settings |
| **Monitoring** | Agentless bar under SSH tabs with live CPU, memory, disk usage, network throughput and uptime of the remote host |
| **Remote desktop** | Native RDP (IronRDP) and VNC (vnc-rs) embedded in tabs, VNC over SSH, view-only mode, limits against hostile servers |
| **Tunnels** | Local, remote and dynamic (SOCKS5) forwarding; listeners bind `127.0.0.1` unless you confirm otherwise |
| **Encrypted store** | Sessions, credentials, trust records, audit and history in SQLCipher, unlocked by a master password and optionally the desktop keyring |
| **Import** | Import sessions from a MobaXterm.ini file (never passwords) with preview and skip report; TOML export without credentials |
| **Agent control** | `rustxterm` CLI and local MCP server: off by default, same user only, per-session policies, approvals, audit log |
| **Desktop** | Light, dark and follow-system themes on every screen; original flat icon set; notifications for background transfers and dropped sessions; session folders remember their state; window reopens on the monitor you left it on |

Status: **v0.3.0** is the first public release (SSH, files, remote desktops, agent control, Telnet,
Serial and remote monitoring) for Linux x86_64. Windows builds are in preview while the Windows
end-to-end check is completed; a native Apple Silicon port, XDMCP and rsh are planned; see the
[roadmap](docs/README.md#roadmap-and-openspec-changes).

## Screenshots

<table>
  <tr>
    <th>Main window (light)</th>
    <th>Dual-pane FTP tab (dark)</th>
  </tr>
  <tr>
    <td><a href="docs/screenshots/main_tabs_light@2x.png"><img src="docs/screenshots/main_tabs_light@2x.png" width="420" alt="Main window with SSH, SFTP and RDP tabs (light theme)"></a></td>
    <td><a href="docs/screenshots/file_tab_ftp_dark@2x.png"><img src="docs/screenshots/file_tab_ftp_dark@2x.png" width="420" alt="Dual-pane FTP tab (dark theme)"></a></td>
  </tr>
</table>

Rendered offscreen by the test suite with fictional data and the project's own icon set. Every screen
in both themes is in the [screenshot gallery](docs/screenshots/README.md).

## Install

Download from [GitHub Releases](https://github.com/jniltinho/rustXterm/releases). Linux x86_64, any
glibc-based distribution with glibc 2.31 or newer (Ubuntu 20.04+, Debian 12+, Fedora, Arch).

| Asset | Use |
|---|---|
| `rustxterm-X.Y.Z-x86_64.AppImage` | Download, `chmod +x`, run |
| `rustxterm-X.Y.Z-x86_64.tar.gz` | Binary, `.desktop` file, icon, licenses |
| `rustxterm-X.Y.Z-x86_64` | Portable single binary |
| `SHA256SUMS` | Checksums for all of the above |

```sh
sha256sum --check --ignore-missing SHA256SUMS
chmod +x rustxterm-X.Y.Z-x86_64.AppImage
./rustxterm-X.Y.Z-x86_64.AppImage
```

It needs a Vulkan or OpenGL driver (Mesa is fine) and an X11 or Wayland session. On X11 (or
Xwayland) it also needs `libxkbcommon-x11`, which some Wayland-first installs omit (Ubuntu 26.04:
`sudo apt install libxkbcommon-x11-0`). FUSE 2 is not needed for the AppImage.
`rustxterm --self-test` runs headless checks. On first start it writes
`~/.local/share/applications/rustxterm.desktop` and its icon (unless a system entry exists), so
GNOME and KDE show the rustXterm icon on Wayland.

**Windows (preview, x86_64):** `rustxterm-X.Y.Z-windows-x86_64.zip` (portable `rustxterm.exe` and
`rustxterm-cli.exe`) and a per-user `rustxterm-X.Y.Z-x86_64.msi` are published once the Windows
end-to-end check passes; they are unsigned, so SmartScreen may ask for confirmation. The MSI installs
without administrator rights, but Windows Server 2022 disables installs by standard users by default
(`DisableMsi` policy, error 1625): install as an administrator there, or use the zip.

The renderer picks a GPU backend automatically (OpenGL on Windows when a hardware adapter exists,
since DX12 flickers on some Intel drivers). Set `WGPU_BACKEND=gl`, `vulkan` or `dx12` to force one if
the window flickers or stays black.

## Quick start

1. Start `rustxterm` and choose a master password. It encrypts all sessions and credentials and
   **cannot be recovered**. Optionally let the desktop keyring unlock it.
2. Create a session with **New session**, or type `user@host` in quick connect.
3. Check the host key fingerprint and accept it. The SFTP panel opens next to the terminal.
4. To import sessions from a MobaXterm.ini file, use **Import sessions** or
   `rustxterm import mobaxterm-ini MobaXterm.ini`.

<details>
<summary>Configuration files and environment variables</summary>

Settings live in `~/.config/rustxterm/config.toml`; data lives under `~/.local/share/rustxterm/`.
Settings can be overridden with `--env-file` or `RXT_*` variables:

| Variable | Effect |
|---|---|
| `RXT_THEME` | `light`, `dark` or `system` |
| `RXT_LOG_LEVEL` | Tracing filter, e.g. `info`, `debug` |
| `RXT_DATA_DIR` | Data directory |
| `RXT_AGENT_CONTROL` | Agent control on or off (`true`/`false`) |

Precedence: CLI > environment / `.env` > TOML > defaults. Secret-like keys are refused.

</details>

## CLI and MCP

The same binary is a CLI. Reads of saved data work offline; commands on open tabs talk to the running
instance once you enable **Settings → Agent control**.

```sh
rustxterm sessions list --json
rustxterm open prod-web                 # open a saved session in the running GUI
rustxterm send 3 'uptime' --enter
rustxterm wait 3 'load average' --timeout 10s
rustxterm read 3 --lines 20
rustxterm files get 3 /var/log/syslog syslog.txt
rustxterm tunnel start db-forward
```

MCP clients start `rustxterm mcp` over stdio:

```json
{ "mcpServers": { "rustxterm": { "command": "rustxterm", "args": ["mcp"] } } }
```

New sessions default to the `deny` agent policy; `ask` shows an approval prompt in the GUI. Adding,
editing or removing sessions, trust decisions, credentials and policies are never available to agents.

<details>
<summary>MCP tools</summary>

17 tools while the GUI runs, including `list_sessions`, `open_session`, `read_terminal`, `send_input`,
`wait_for_output`, `screenshot_tab`, `file_get`, `file_put` and `tunnel_start`; 5 read-only tools work
offline. See [agent control](docs/README.md#agent-control-cli-and-mcp) in the documentation index.

</details>

<details>
<summary>Exit codes</summary>

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Failed |
| 2 | Usage error |
| 3 | Store locked |
| 4 | Denied by policy or approval |
| 5 | rustXterm is not running |
| 6 | Agent control is off |

</details>

## Architecture

<p align="center">
  <img src="docs/diagrams/01-architecture-layers.png" width="760" alt="Architecture: crate layers">
</p>

One crate per protocol, a `platform` contract crate with a Linux adapter, an engine where every UI action
is a command, and a thin egui UI with theme tokens; `cargo xtask layers` enforces the layers. The other
seven diagrams (data flow, SSH sequence, encrypted store, agent control, release pipeline, test lab,
roadmap) are in the [documentation index](docs/README.md).

## Build from source

Builds, lints and tests run only in Docker; the host needs only Docker, git and optionally `make`.
The root `Makefile` wraps the Docker workflow (`make help` lists every target):

```sh
make build                      # debug build of the workspace
make check                      # fmt-check, clippy, tests, layers, scenarios
make guard                      # publication guard, run before every push
make lab-up lab-test lab-down   # integration tests against the disposable protocol lab
make dist VERSION=0.0.0-test    # binary, AppImage, tarball and SHA256SUMS in dist/
```

Without `make`, call the wrapper directly: `scripts/cargo-docker.sh cargo test --workspace`. The
[developer guide](docs/DEVELOPMENT.md) covers the full workflow.

The toolchain stages (pinned Rust, mingw-w64 for Windows, AppImage tools) are also published on Docker
Hub as `jniltinho/rustxterm-{toolchain,windows-toolchain,appimage-tools}:snapshot-20261001`; CI uses
them as build cache, and the `Dockerfile` remains the source of truth.

## Documentation

| Document | Contents |
|---|---|
| [Developer guide](docs/DEVELOPMENT.md) | Prerequisites, Docker workflow, tests, lab, coding rules |
| [Documentation index](docs/README.md) | Architecture, data flow, store, agent control, diagrams, roadmap |
| [Screenshot gallery](docs/screenshots/README.md) | Every screen in light and dark themes |
| [Releases](docs/release.md) | Cutting and verifying a release |
| [Protocol lab](lab/README.md) | Docker lab services and fictional credentials |

## Security

- All stored data is encrypted at rest (SQLCipher, AES-256); the database key is wrapped with Argon2id and
  XChaCha20-Poly1305 by your master password and, optionally, the desktop keyring (Secret Service).
- Host keys are checked before authentication on every hop; changed or revoked keys are refused, and an
  unreadable trust source fails closed. FTPS and RDP certificates are pinned on first use.
- Agent control is off by default, reachable only by your user (a Unix socket on Linux, a named pipe
  with an owner-only ACL and an elevation check on Windows), and never discloses
  passwords or keys: credentials appear only as `saved` or `absent`.
- Remote desktop clients cap framebuffer size and memory and time out stalled servers.
- Releases are built in a pinned, offline Docker build and published with `SHA256SUMS`.

## License

rustXterm is released under the [MIT License](LICENSE).

Every icon, image, font and fixture is listed with its origin and license in
[ATTRIBUTION.md](ATTRIBUTION.md). The application icon set in `assets/icons/app/` is original and MIT
licensed; the distribution logos in `assets/icons/app/brands/` are trademarks of their owners.

<details>
<summary>Acknowledgements</summary>

rustXterm stands on great open-source work, including
[egui/eframe](https://github.com/emilk/egui), [alacritty_terminal](https://github.com/alacritty/alacritty),
[russh](https://github.com/Eugeny/russh), [suppaftp](https://github.com/veeso/suppaftp),
[IronRDP](https://github.com/Devolutions/IronRDP), [vnc-rs](https://github.com/HsuJv/vnc-rs),
[rusqlite](https://github.com/rusqlite/rusqlite) with [SQLCipher](https://www.zetetic.net/sqlcipher/),
[oo7](https://github.com/bilelmoussaoui/oo7), the [RustCrypto](https://github.com/RustCrypto) crates,
[rmcp](https://github.com/modelcontextprotocol/rust-sdk), [clap](https://github.com/clap-rs/clap),
[rpassword](https://github.com/conradkleinespel/rpassword),
[cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild),
[simple-icons](https://simpleicons.org/) for distribution logos, Tux by Larry Ewing, and the
[Noto Sans](https://notofonts.github.io/), [Cascadia Mono](https://github.com/microsoft/cascadia-code),
[JetBrains Mono](https://www.jetbrains.com/lp/mono/), [Hack](https://sourcefoundry.org/hack/) and
[DejaVu Sans Mono](https://dejavu-fonts.github.io/) fonts.

</details>
