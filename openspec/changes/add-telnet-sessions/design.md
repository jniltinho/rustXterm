## Context

`create-native-rust-linux-terminal` (design D1) reserves `crates/telnet` at L2 for phase 2. The engine already talks to protocol crates through `crates/engine/src/transport.rs` (`Transport::connect` → `Link::shell` → `ShellRead`/`ShellWrite`), so a terminal protocol only has to produce a byte stream with resize. The terminal widget, `term` model, MultiExec fan-out (D6), session logging and the "Unencrypted" marker (`crates/ui/src/file_tab.rs::unencrypted`, used today for plain FTP) already exist. A Telnet session has host, port, optional username, terminal settings and network settings.

`MobaXterm.ini` type codes: bookmark lines are `name=#<icon>#<type>%host%port%user...` where the first number is only the tree icon and the session type is the first `%` field: SSH=0, RDP=4, VNC=5, FTP=6, SFTP=7 known; **Telnet=1 assumed**, still to be confirmed with sample files. Escapes: `;`→`__PTVIRG__`, `%`→`__PERCENT__`, `#`→`__DIEZE__`, `|`→`__PIPE__`.

## Goals / Non-Goals

**Goals:**
- Interoperate with common telnetd implementations (inetutils, netkit, busybox, Cisco/Juniper-style CLIs) and raw TCP services.
- Never loop or allocate without bound on hostile option traffic.
- Make the plaintext risk visible; never type credentials unless the user opted in.

**Non-Goals:**
- See proposal non-goals (TN3270, TLS, Kerberos, NEW-ENVIRON, LINEMODE editing, server mode).

## Decisions

### T1. Own sans-IO codec in `crates/telnet`
A pure state machine `Codec::feed(&[u8]) -> (data, replies, events)` and `Codec::encode_input(&[u8])`, with a thin tokio `TcpStream` driver. Telnet is small (a few hundred lines) and a sans-IO core is easy to unit-test and fuzz. Alternatives: `libtelnet-rs` (MIT port of libtelnet; usable but pulls its own event model and is lightly maintained), `nectar` (tokio codec; fewer options). They remain code references only.

### T2. Option policy (RFC 1143 Q-method)
| Option | Local (WILL) | Remote (DO) |
|---|---|---|
| BINARY (0) | accept | accept |
| ECHO (1) | refuse | accept (server echoes) |
| SGA (3) | accept | accept |
| TTYPE (24) | accept; `SB TTYPE SEND` → `IS <terminal type>` (session setting, default `xterm-256color`) | — |
| NAWS (31) | accept; send cols/rows on accept and on every resize | — |
| LINEMODE (34) | refuse | refuse |
| any other | refuse (`WONT`/`DONT`) | refuse |

The client proactively offers `WILL TTYPE`, `WILL NAWS`, `DO SGA`, `DO ECHO`. The Q-method state per option prevents negotiation loops; a peer that toggles one option more than 32 times in a session gets refused silently afterwards.

### T3. Data path
- IAC (0xFF) in user input is doubled; IAC IAC in server data is one 0xFF.
- Without BINARY in the sending direction, Enter sends `CR LF` by default (session setting: `CR LF` / `CR NUL` / `CR`), and a lone CR from the server followed by NUL is reduced to CR.
- Subnegotiation buffers are capped at 1 KiB; longer ones are discarded to `SE`. Unknown commands (NOP, DM, GA, AYT, …) are consumed. The existing per-session 4 MiB output buffer (D6) provides backpressure: the driver stops reading the socket when full.
- Resize from the terminal widget becomes a NAWS subnegotiation (values containing 0xFF are escaped).

### T4. Raw TCP mode
A session flag `raw: bool` bypasses the codec entirely (no IAC parsing or escaping, Enter sends CR by default). It shares the same tab, logging and marker.

### T5. Auto-login (opt-in)
Off by default. Enabled only when the session has `auto_login = true`, a username, and a password saved in the encrypted store (session-management "Saved credentials"). The driver scans server output (ANSI-stripped, case-insensitive, last 256 bytes) for the login prompt (default `login:` or `username:`) and then the password prompt (default `password:`), each configurable, within 15 s of connecting. Each answer is sent at most once; if the prompts do not appear in order within the timeout, nothing more is typed and the user continues manually. The password is held in a zeroizing buffer and never logged: session logging masks the auto-typed password bytes, and agent-control output never contains it (D14 redaction). Quick-connect sessions never auto-login.

### T6. Integration
- `core`: `Protocol::Telnet` (port 23, scheme `telnet`, `is_terminal() = true`), `TelnetOptions { raw, terminal_type, newline, auto_login, login_prompt, password_prompt }` with `#[serde(default)]`.
- `engine`: `Real::connect` branches to `rxt_telnet` and returns a `Link` whose `shell()` yields the stream; `file_ops()` returns "not supported"; `start_tunnel` is not offered. MultiExec, reconnect and logging work unchanged because they act on `ShellRead`/`ShellWrite`.
- `ui`: type-picker entry, Telnet page (port, username, raw mode, newline, terminal type, auto-login with prompts and a plaintext warning), `unencrypted()` returns true for Telnet so tabs show the marker; on first connect of each session a dismissible "Telnet is not encrypted" notice appears in the tab.
- Icon: a Telnet session icon from the bundled set in `assets/icons/app/`; until one exists the catalog's `session` terminal icon is used.

### T7. Delta-spec strategy
New behavior in existing capabilities is expressed as **ADDED** requirements (e.g. "Telnet quick connect") rather than MODIFIED copies of the base requirements, because four phase-2 changes extend the same base requirements in parallel and MODIFIED blocks would overwrite each other when archived.

### T8. Lab
`telnetd` service: Debian with `inetutils-telnetd` under `inetutils-inetd` (or busybox `telnetd -F`), fictional user `alice` / `lab-alice-pass`, published on `127.0.0.1:2323`; a second port runs a raw TCP echo service (`socat TCP-LISTEN:7,fork EXEC:cat`) on `127.0.0.1:2007`. Hostile negotiation (option ping-pong, oversized SB, IAC storms) is covered by codec unit tests and a small hostile peer in `xtask`, like the RFB/RDP peers of D11.

## Risks / Trade-offs

- [Plaintext credentials] → marker, warning, opt-in auto-login, no auto-login on quick connect; recommendation to use SSH where possible.
- [Device CLIs with odd newline expectations] → per-session newline setting and raw mode.
- [Prompt matching false positives] → once-only answers, ordered prompts, timeout, configurable prompt strings.
- [Base import scenario says Telnet is skipped] → the ADDED import requirement supersedes it for Telnet; when this change is archived after the base, the base scenario is updated in the same archive step (see Open Questions).

## Migration Plan

Additive. Stored sessions without Telnet fields deserialize with defaults; no database migration beyond the existing session serialization. Rollback: older builds skip sessions with an unknown protocol (verify in the store tests) rather than failing to load.

## Open Questions

- Confirm the `MobaXterm.ini` Telnet type code (`1`, assumed) and its field positions (port, username, terminal settings) with sample files; adjust the import requirement if it differs.
- Does the base `MobaXterm.ini` import scenario ("1 skipped Telnet session") need a MODIFIED delta at archive time, or is it reworded in the base change before archive?
- Should `raw://host:port` be offered in quick connect later?
