## Why

Telnet is still the management interface of many switches, routers, embedded boards, lab equipment and legacy hosts, and users expect it as a first-class session type next to SSH. rustXterm v0.1 (`create-native-rust-linux-terminal`) lists Telnet for phase 2. This change adds it as a native, clearly-marked-insecure terminal session type that reuses the existing terminal widget, MultiExec, logging and session store. Behavior follows the public RFCs.

## What Changes

- New protocol crate `crates/telnet` (L2, per design D1 of `create-native-rust-linux-terminal`): a sans-IO RFC 854/855 Telnet codec with RFC 1143 option negotiation (BINARY, ECHO, SGA, TTYPE `xterm-256color`, NAWS window size, LINEMODE refused, everything else refused), IAC escaping, NVT newline handling and bounded subnegotiation, plus a tokio TCP driver.
- **Raw TCP mode** per session: no Telnet negotiation, bytes pass through unchanged (for serial-over-IP servers and line-oriented services).
- **Auto-login, opt-in only**: when the user explicitly enables it and has saved the password in the encrypted store, the client answers the first login and password prompts once; otherwise nothing is typed for the user.
- New `Protocol::Telnet` variant (default port 23, scheme `telnet`), a Telnet page in the session editor, a session-type icon from the `assets/icons/app/` catalog, quick connect `telnet://[user@]host[:port]`.
- Telnet tabs carry the existing "Unencrypted" marker used by plain FTP and show a one-time plaintext warning per session.
- Reuse: terminal widget and `term` model, MultiExec fan-out, session logging, reconnect, close confirmation, agent-control policies (Telnet is a terminal session like SSH).
- MobaXterm.ini import of Telnet bookmarks.
- Lab: a `telnetd` container (fictional credentials, 127.0.0.1 only) and lab tests.

This change depends on and comes after `create-native-rust-linux-terminal` (it needs the engine transport seam, terminal widget, encrypted store and lab).

### Non-goals

TN3270/TN5250, Telnet over TLS (RFC draft `STARTTLS`/telnets), Kerberos/SRA authentication options, NEW-ENVIRON/ENVIRON variable export, LINEMODE client editing, Telnet server mode, `raw://` quick-connect scheme.

## Capabilities

### New Capabilities

- `telnet-sessions`: Telnet and raw TCP terminal sessions: negotiation, newline handling, window size, auto-login opt-in, plaintext marking, bounded parsing.

### Modified Capabilities

- `desktop-shell`: quick connect accepts `telnet://` (added requirement; the base "Quick connect" requirement is unchanged).
- `session-management`: session editor offers the Telnet type; MobaXterm.ini import maps Telnet bookmarks (added requirements).

## Impact

- New crate `crates/telnet` (no new third-party dependency beyond tokio/bytes already in the workspace); `cargo xtask layers` gains the crate at L2.
- `crates/core` (`Protocol::Telnet`, `TelnetOptions` with serde defaults so existing stored sessions and exports still load), `crates/engine` (transport seam: `Transport::connect` returns a `Link` whose `shell()` is the Telnet stream), `crates/ui` (editor page, quick connect, icon, unencrypted marker), `crates/store` (importer mapping, credential lookup for auto-login).
- `lab/compose.yaml` (`telnetd` service), `lab/expected-tests.txt`, `ATTRIBUTION.md` if a new icon is added.
- Security: Telnet sends everything, including passwords, in clear text; the UI says so and auto-login is off by default.
