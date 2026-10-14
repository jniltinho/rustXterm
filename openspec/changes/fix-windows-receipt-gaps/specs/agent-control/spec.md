## ADDED Requirements

### Requirement: Portable master-password prompt
Every CLI path that needs the master password on an interactive terminal SHALL read it without echo on Linux and Windows terminals alike (Windows console, Windows Terminal, an SSH session with a pseudo-terminal), SHALL restore the terminal's echo mode afterwards even when the read fails or is interrupted, and SHALL never print or log the typed value.

#### Scenario: Offline change on a Windows terminal
- **WHEN** the user runs `rustxterm-cli sessions remove web1` in a Windows console with the keyring unavailable
- **THEN** the CLI asks for the master password without echoing it, and the session is removed after the correct password

#### Scenario: Interrupted prompt restores echo
- **WHEN** the user presses Ctrl+C at the master-password prompt
- **THEN** the CLI exits with an error and the terminal echoes typed characters again

### Requirement: Live calls tolerate a busy control channel
The CLI and the MCP server SHALL decide whether to talk to the running application without opening the control channel as a probe, and a channel that exists but is momentarily busy SHALL be retried for a bounded time instead of being treated as "not running". Overlapping requests from one MCP session SHALL each be answered by the running application.

#### Scenario: Pipelined MCP requests
- **WHEN** an MCP client sends `send_input` and `wait_for_output` without waiting for the first answer, while the GUI runs with agent control on
- **THEN** both requests are answered by the GUI and neither reports `not_running`

#### Scenario: Application really stopped
- **WHEN** no rustXterm GUI is running
- **THEN** a live call reports `not_running` within two seconds

### Requirement: Refusal reasons reach the client
When the control server refuses a client, it SHALL send a structured refusal reason before closing the channel, and the CLI and MCP server SHALL show it. A refusal caused by a different elevation (integrity level) between CLI and GUI SHALL be reported in both directions as "agent control runs at a different elevation (run the CLI and rustXterm both elevated or both not)".

#### Scenario: Non-elevated CLI, elevated GUI
- **WHEN** a non-elevated `rustxterm-cli tabs list` connects to a GUI started elevated
- **THEN** the CLI prints the different-elevation message and exits with the "denied" exit code

#### Scenario: Elevated CLI, non-elevated GUI
- **WHEN** an elevated CLI connects to a non-elevated GUI
- **THEN** the CLI prints the same different-elevation message
