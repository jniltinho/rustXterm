## ADDED Requirements

### Requirement: Windows control channel
On Windows the local control channel SHALL be a named pipe whose name includes the current user's SID, whose access-control list grants access only to the current user, and which rejects remote clients. Startup SHALL create the first pipe instance exclusively and SHALL fail (agent control reported as not started) when the name already exists; while agent control is enabled the application SHALL keep at least one listening instance, each created with the same access-control list before an accepted connection is handed on. The server SHALL authenticate each connection by its own security context (impersonating the client for identification only, after reading at most the first 4 bytes within 5 seconds and before processing any request) and SHALL close without a response any connection whose token belongs to another user, has restricting SIDs, is an AppContainer token, has a lower integrity level than the server, has a group as deny-only that is enabled for the server (more filtered than the server; ordinary UAC filtering shared by both is accepted), or cannot be inspected. The client SHALL refuse to talk to a pipe whose server process does not run as the same user. Frame limit, enable/disable rules and "not running" reporting SHALL match Linux; the channel SHALL never listen on a network interface.

#### Scenario: Other user on Windows
- **WHEN** a process of another Windows user connects to the control pipe
- **THEN** the connection is refused or closed without a response

#### Scenario: Restricted or lower-integrity client
- **WHEN** a process of the same user connects with a token that has restricting SIDs, extra deny-only groups, or low integrity
- **THEN** the connection is closed without a response and nothing is executed

#### Scenario: Ordinary non-elevated client
- **WHEN** the CLI runs from a normal (UAC-filtered) console of the same user while the GUI runs non-elevated
- **THEN** the command is served

#### Scenario: Different elevation
- **WHEN** the CLI runs elevated while the GUI runs non-elevated, or the reverse
- **THEN** the command is refused and the CLI reports that agent control runs at a different elevation

#### Scenario: Impostor pipe
- **WHEN** another process created a pipe with the expected name before rustXterm started
- **THEN** rustXterm does not serve on it, reports that agent control could not start, and the CLI refuses to connect to a pipe served by another user

#### Scenario: Many clients
- **WHEN** several CLI commands run one after another and concurrently while agent control is enabled
- **THEN** each is served, and the pipe name stays owned by rustXterm between connections

#### Scenario: Disabled on Windows
- **WHEN** agent control is disabled
- **THEN** no control pipe exists and live CLI commands report that agent control is off

### Requirement: Windows CLI executable
On Windows, CLI subcommands, `mcp` and `--self-test` SHALL be provided by the console executable `rustxterm-cli.exe` with the same arguments, output and exit codes as `rustxterm` on Linux; `rustxterm.exe` SHALL open only the GUI and SHALL NOT open a console window; given a subcommand, `mcp` or `--self-test` it SHALL exit with the usage error code without opening a window.

#### Scenario: MCP client on Windows
- **WHEN** an MCP client starts `rustxterm-cli.exe mcp` over stdio while the GUI runs with agent control enabled
- **THEN** the handshake succeeds and live tools reach the running instance

#### Scenario: GUI start without console
- **WHEN** the user starts `rustxterm.exe` from the Start Menu or Explorer
- **THEN** only the main window opens and no console window appears

## MODIFIED Requirements

### Requirement: CLI
The `rustxterm` binary (on Windows the console executable `rustxterm-cli.exe`) SHALL provide subcommands with stable JSON output (`--json`): offline store reads (`sessions list|show`, `folders list`, `tunnels list`, `export`), offline store changes (`sessions add|edit|remove`, `import mobaxterm-ini`) and live operations against the running instance (`open`, `tabs list`, `send`, `read`, `wait`, `screenshot`, `files ls|get|put`, `tunnel start|stop`, `close`). Offline changes SHALL require the master password typed on an interactive terminal (never the keyring, stdin pipes or environment) and SHALL be audited. Exit codes SHALL distinguish usage error, locked database, denied, not running and runtime failure.

#### Scenario: Offline list
- **WHEN** the GUI is not running and the user runs `rustxterm sessions list --json` (`rustxterm-cli.exe sessions list --json` on Windows) and unlocks the database
- **THEN** sessions are printed as JSON without credentials

#### Scenario: Offline change without a terminal
- **WHEN** `rustxterm sessions remove prod` runs with stdin not attached to a terminal
- **THEN** it exits with the denied code and nothing changes

#### Scenario: Not running
- **WHEN** a live command runs and no instance is running
- **THEN** it exits with the "not running" code without starting the GUI

### Requirement: MCP server
`rustxterm mcp` (on Windows `rustxterm-cli.exe mcp`) SHALL run a stdio MCP server exposing only the MCP columns of the permission matrix (list sessions, open session, list tabs, read terminal text, send input, wait for output pattern with timeout, file list/get/put, tunnel start/stop, close tab, screenshot tab) with JSON-schema parameters and structured errors, reaching the running instance through the control channel. Offline, it SHALL expose only read-only store tools.

#### Scenario: Agent runs a command
- **WHEN** an MCP client calls `send_input` with `uptime\n` on an approved SSH tab and then `wait_for_output` for `load average`
- **THEN** the tool returns the matching terminal text

#### Scenario: Invalid parameters
- **WHEN** a tool is called with a missing or wrong-typed parameter
- **THEN** it returns an MCP error naming the parameter and the application keeps running
