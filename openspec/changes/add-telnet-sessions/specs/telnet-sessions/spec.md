## Purpose

Telnet and raw TCP terminal sessions for network devices and legacy hosts, embedded in the existing terminal tab and clearly marked as unencrypted.

## ADDED Requirements

### Requirement: Telnet connection and negotiation
The application SHALL open Telnet sessions (default port 23) in a terminal tab and negotiate options per RFC 854/855 without loops: it SHALL accept BINARY, SGA and server ECHO, SHALL offer terminal type (default `xterm-256color`, configurable per session) and window size (NAWS), SHALL refuse LINEMODE and every other option, and SHALL send a new window size whenever the terminal is resized.

#### Scenario: Interactive login
- **WHEN** the user opens a Telnet session to a telnetd host and types the username and password at the prompts
- **THEN** the remote shell starts, `echo $TERM` prints the configured terminal type, and characters are echoed once (by the server, not locally)

#### Scenario: Resize
- **WHEN** the user resizes a connected Telnet tab to 120×40
- **THEN** the server's `stty size` reports `40 120`

#### Scenario: Negotiation loop
- **WHEN** a server repeatedly toggles the same option on and off
- **THEN** the client answers per the option state machine, stops answering after a fixed limit, and the tab stays responsive

### Requirement: Telnet data handling
The client SHALL escape byte 0xFF in user input, unescape doubled 0xFF in server data, and send Enter as `CR LF` by default, configurable per session as `CR LF`, `CR NUL` or `CR`. Subnegotiations longer than 1 KiB SHALL be discarded without growing memory.

#### Scenario: Oversized subnegotiation
- **WHEN** a server sends a 1 MiB subnegotiation
- **THEN** it is discarded, memory stays bounded and following data is displayed normally

#### Scenario: Newline setting
- **WHEN** a session is set to `CR NUL` and the user presses Enter
- **THEN** the bytes `0x0D 0x00` are sent

### Requirement: Raw TCP mode
A Telnet session SHALL offer a raw mode in which no Telnet negotiation or byte escaping happens and bytes pass unchanged in both directions.

#### Scenario: Raw echo service
- **WHEN** a raw-mode session connects to a TCP echo service and the user sends bytes including 0xFF
- **THEN** the same bytes come back and are displayed without any negotiation traffic

### Requirement: Telnet auto-login opt-in
The client SHALL type the username and password for the user only when the session explicitly enables auto-login and its password is saved in the encrypted store. It SHALL answer the login prompt and then the password prompt at most once each, within 15 seconds of connecting, and otherwise type nothing. Auto-typed passwords SHALL never appear in session logs, errors or agent output. Quick-connect sessions SHALL never auto-login.

#### Scenario: Opt-in login
- **WHEN** auto-login is enabled with a saved password and the server shows `login:` then `Password:`
- **THEN** each is answered once and the shell starts without user typing

#### Scenario: Not opted in
- **WHEN** auto-login is disabled
- **THEN** nothing is typed for the user even if a password is saved

#### Scenario: Prompt never appears
- **WHEN** auto-login is enabled and no login prompt appears within 15 seconds
- **THEN** nothing is typed and the user can log in manually

#### Scenario: Log masking
- **WHEN** session logging is on and auto-login sends the password
- **THEN** the log file does not contain the password

### Requirement: Plaintext marking
Telnet and raw TCP tabs SHALL show the "Unencrypted" marker, and the session editor SHALL warn that Telnet sends all data, including passwords, in clear text.

#### Scenario: Marker
- **WHEN** a Telnet tab is open
- **THEN** its tab shows the Unencrypted marker in both light and dark themes

### Requirement: Terminal features on Telnet tabs
Telnet tabs SHALL support the same terminal features as SSH tabs: scrollback, search, clipboard, session logging, MultiExec, reconnect and close confirmation.

#### Scenario: MultiExec with mixed tabs
- **WHEN** MultiExec is on with one SSH tab and one Telnet tab selected and the user types `uptime` and Enter
- **THEN** both tabs receive the command, the Telnet tab with its configured newline
