## Context

The Berkeley r-commands:
- **rlogin** (RFC 1282, TCP 513): client sends `\0 local-user \0 remote-user \0 term/speed \0`, server answers `\0`, then a raw terminal stream. The server requests window size with TCP urgent byte `0x80`; the client sends `FF FF 73 73` + rows, cols, xpixels, ypixels (big-endian u16) in band. Other urgent bytes (`0x02` flush, `0x10`/`0x20` flow-control mode) are hints.
- **rsh** (rshd, TCP 514): client sends `stderr-port \0 local-user \0 remote-user \0 command \0`; `0` as stderr port means no secondary channel (stderr is merged or lost, server-dependent); server answers `\0` or `\1` + message.
- **rexec** (rexecd, TCP 512): `stderr-port \0 user \0 password \0 command \0`, same answers; no privileged-port requirement, password in clear.

rlogind and rshd authenticate with `~/.rhosts`/`/etc/hosts.equiv` by trusting the client-supplied local user name, and justify that trust by requiring the client's source port to be in 512–1023 (only root can bind it on the client). Most implementations (netkit, BSD) **reject** connections from ports ≥ 1024 outright; rlogind falls back to a password prompt only after the port check passes.

On Linux an unprivileged process can bind ports < 1024 only with `CAP_NET_BIND_SERVICE`, as root, via a setuid helper, or when the administrator lowered `net.ipv4.ip_unprivileged_port_start`. Capabilities and setuid are not acceptable for rustXterm (security policy and portable/AppImage distribution, where file capabilities do not survive anyway).

`MobaXterm.ini` type codes: type is the first `%` field; SSH=0, RDP=4, VNC=5, FTP=6, SFTP=7 known; **Rsh=2 assumed**, to be confirmed with sample files. Whether the file format's Rsh type covers rlogin, and whether a separate rlogin code exists, is unknown.

## Goals / Non-Goals

**Goals:**
- Work wherever the platform legitimately allows it, and explain precisely why it does not work otherwise.
- Make the feature hard to use by accident (off by default, warnings, confirmation).

**Non-Goals:** see proposal.

## Decisions

### R1. One crate, three modes
`crates/rsh` holds sans-IO handshake encoders/decoders (bounded: error messages ≤ 1 KiB, user names and command ≤ 4 KiB, NUL rejected in fields) and a tokio driver yielding the `ShellRead`/`ShellWrite` pair used by the engine for terminal tabs. rlogin is interactive with resize; rsh and rexec run one command, forward typed input as stdin, and the tab shows "command finished" when the server closes.

### R2. Privileged source port strategy (evaluated)
| Option | Works for unprivileged user? | Verdict |
|---|---|---|
| setuid helper / file capability `cap_net_bind_service` | yes | rejected: privilege elevation, not portable (AppImage), out of policy |
| `sudo`/`pkexec` per connection | yes | rejected: privilege elevation |
| bind 512–1023 and fall through on `EACCES` | only if root or the admin lowered `ip_unprivileged_port_start` | **chosen** as the default path |
| connect from an ephemeral port | only if the server skips the port check | **offered** per session as "allow unprivileged source port" (off by default) |
| rexec | yes (no port check), password in clear | **offered** as a mode |
| system `rlogin`/`rsh` binary through a local PTY | yes where the distro ships it setuid | deferred: needs the phase-2 `crates/pty`; see Open Questions |

Algorithm: create a `tokio::net::TcpSocket`, try `bind` on ports 1023 down to 512 (skip `EADDRINUSE`); on the first `EACCES` stop. If the session allows unprivileged ports, connect from an ephemeral port and, if the server then closes or answers an error, show "server requires a privileged source port". Otherwise show the explanation: "rlogin/rsh servers only trust clients connecting from ports below 1024, which only privileged processes can open on Linux. rustXterm does not run with privileges. Options: use the rexec mode, use SSH, or ask the administrator to lower `net.ipv4.ip_unprivileged_port_start` (affects the whole system)." The current value of that sysctl is read (std, `/proc/sys`) and shown; rustXterm never writes it. This logic lives in the Linux adapter behind a small `platform` contract (`bind_reserved_port`) so other OSes can differ.

### R3. rlogin details
Terminal string `xterm-256color/38400` (term type configurable). Window size is sent proactively right after the server's `\0` and on every resize, which rlogind accepts after its initial window request; we do not read TCP urgent data (the urgent byte is not delivered in band without `SO_OOBINLINE`), so flush/flow-control hints are ignored. Local user name comes from the platform (`$USER` fallback to the passwd entry) and is editable per session ("local user to present"), because `.rhosts` matching depends on it.

### R4. rexec password
Taken from the encrypted store only when "save password" is ticked, otherwise prompted each time; zeroized after sending; never logged (the handshake is not written to session logs).

### R5. Legacy toggle and labelling
`settings.legacy_rcommands = false` in TOML. While false: `Protocol::Rsh` is hidden from the type picker, existing Rsh sessions show as disabled in the tree with "Enable in Settings → Security", quick-connect `rlogin://`/`rsh://` returns an inline error naming the toggle, the importer skips Rsh bookmarks with that reason, and agent-control `session.open` on Rsh sessions is refused with the same reason. While true: Unencrypted marker on tabs, a warning banner on the editor page, and a first-connect confirmation per session.

### R6. Integration
- `core`: `Protocol::Rsh` (scheme `rsh`, default port by mode: 513/514/512, `is_terminal() = true`), `RshOptions { mode, command, local_user, allow_unprivileged_port, terminal_type }`.
- `engine`: `Real::connect` branch to `rxt_rsh`; MultiExec applies to rlogin tabs (rsh/rexec tabs too while running, since they accept stdin).
- `ui`: Settings → Security toggle; Rsh page (mode, host, port, remote user, local user, command for rsh/rexec, allow unprivileged port, save password for rexec); icon from the bundled set in `assets/icons/app/`, fallback `terminal-user`.
- Quick connect: `rlogin://[user@]host[:port]` → rlogin mode; `rsh://[user@]host[:port]` → rlogin mode (classic `rsh host` behavior), `rsh://[user@]host[:port]?cmd=<urlencoded>` → rsh mode. rexec is not offered in quick connect (needs a password).

### R7. Lab
`rsh` service: Debian-based image with r-command servers under `inetd` (package chosen in task 1.1: `rsh-redone-server`, or netkit `rsh-server` where packaged; `rexecd` from the same family), fictional users `alice` (`.rhosts` trusting the lab client user, password `lab-alice-pass`) and `dave` (no `.rhosts`). It joins a dedicated bridge network with **no published ports**: Docker's userland proxy would rewrite the client source port, so tests connect to the container's bridge address directly from the toolchain container (host networking), which preserves the source port. The toolchain container runs tests as root, so the privileged-port path is exercised; the unprivileged path is exercised by forcing "allow unprivileged source port" (expect the server's refusal) and by unit tests with a binder that returns `EACCES`.

## Risks / Trade-offs

- [Feature mostly fails for normal desktop users] → accepted and stated up front; rexec mode and clear guidance; low priority.
- [Encouraging insecure protocols] → off by default, warnings, confirmation, no import while off.
- [Server variations in rsh stderr handling with stderr port 0] → documented; stderr may be missing.
- [Bridge-address lab access differs on some CI hosts] → fall back to running the test inside the lab network (open question).

## Migration Plan

Additive; toggle defaults off. Rollback: older builds skip sessions with an unknown protocol.

## Open Questions

- Confirm the `MobaXterm.ini` Rsh type code (`2`, assumed), its field layout, and whether rlogin has its own code or is a variant of Rsh, with sample files.
- Should a later change (after `crates/pty` lands) offer "run the system rlogin/rsh client" in a local PTY, for distros that still ship it setuid? This would be an explicit, visible delegation, not a silent fallback.
- Which maintained server package to use in the lab (rsh-redone vs. netkit) and whether it includes rexecd.
