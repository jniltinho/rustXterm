# agent-control Specification

## Purpose
Let the user and AI agents drive rustXterm from a CLI and a local MCP server with the same commands the UI uses, behind one authorization, approval and audit boundary, without disclosing application-held secrets.

## Requirements

### Requirement: CLI
The `rustxterm` binary SHALL provide subcommands with stable JSON output (`--json`): offline store reads (`sessions list|show`, `folders list`, `tunnels list`, `export`), offline store changes (`sessions add|edit|remove`, `import mobaxterm-ini`) and live operations against the running instance (`open`, `tabs list`, `send`, `read`, `wait`, `screenshot`, `files ls|get|put`, `tunnel start|stop`, `close`). Offline changes SHALL require the master password typed on an interactive terminal (never the keyring, stdin pipes or environment) and SHALL be audited. Exit codes SHALL distinguish usage error, locked database, denied, not running and runtime failure.

#### Scenario: Offline list
- **WHEN** the GUI is not running and the user runs `rustxterm sessions list --json` and unlocks the database
- **THEN** sessions are printed as JSON without credentials

#### Scenario: Offline change without a terminal
- **WHEN** `rustxterm sessions remove prod` runs with stdin not attached to a terminal
- **THEN** it exits with the denied code and nothing changes

#### Scenario: Not running
- **WHEN** a live command runs and no instance is running
- **THEN** it exits with the "not running" code without starting the GUI

### Requirement: Local control channel
Live control SHALL use a local same-user transport provided by the platform layer — on Linux a Unix domain socket in `$XDG_RUNTIME_DIR/rustxterm/` (directory 0700, socket 0600) accepting only peers with the same UID (SO_PEERCRED). The channel SHALL exist only while agent control is enabled in Settings or by `RXT_AGENT_CONTROL=1`, and SHALL never listen on a network interface. Messages larger than 1 MiB SHALL be rejected.

#### Scenario: Other user
- **WHEN** a process of another UID connects to the socket
- **THEN** the connection is closed without a response

#### Scenario: Disabled
- **WHEN** agent control is disabled
- **THEN** no socket exists and live CLI commands report that agent control is off

### Requirement: MCP server
`rustxterm mcp` SHALL run a stdio MCP server exposing only the MCP columns of the permission matrix (list sessions, open session, list tabs, read terminal text, send input, wait for output pattern with timeout, file list/get/put, tunnel start/stop, close tab, screenshot tab) with JSON-schema parameters and structured errors, reaching the running instance through the control channel. Offline, it SHALL expose only read-only store tools.

#### Scenario: Agent runs a command
- **WHEN** an MCP client calls `send_input` with `uptime\n` on an approved SSH tab and then `wait_for_output` for `load average`
- **THEN** the tool returns the matching terminal text

#### Scenario: Invalid parameters
- **WHEN** a tool is called with a missing or wrong-typed parameter
- **THEN** it returns an MCP error naming the parameter and the application keeps running

### Requirement: Command permission matrix
Agent-reachable commands and their entry points SHALL be fixed by this matrix; any other command or entry point combination SHALL be rejected and audited.

| Command group | GUI | CLI live | CLI offline | MCP live | MCP offline |
|---|---|---|---|---|---|
| sessions/folders/tunnels list and show, export | yes | yes | yes | yes | yes |
| sessions add/edit/remove, import mobaxterm-ini | yes | no | yes (interactive TTY master password) | no | no |
| open, tabs list, send, wait, close, tunnel start/stop | yes | yes | no | yes | no |
| read, screenshot, files ls/get/put (content-read flag) | yes | yes | no | yes | no |
| approvals, trust decisions, credentials, agent policy, master password, agent settings | yes | no | no | no | no |

#### Scenario: MCP mutation
- **WHEN** an MCP client calls a session add or import tool
- **THEN** no such tool exists in the server's tool list and a raw request for it is rejected and audited

#### Scenario: Offline CLI edit
- **WHEN** the user runs `rustxterm sessions edit prod --port 2222` on an interactive terminal and types the master password
- **THEN** the change is applied through the engine authorization step and audited, and an attempt to set `agent_policy` in the same command is rejected

### Requirement: Single authorization boundary
Every request from the control channel, the MCP server and offline CLI changes SHALL pass one engine authorization step that checks the permission matrix and writes the audit record. For control-channel and MCP requests it SHALL also check the target session's agent policy and approval. Offline CLI changes authenticated by the master password typed on an interactive terminal are owner operations: they are exempt from the session's `deny`/`ask` policy and need no GUI approval, but still pass the matrix check, are audited with entry point "cli-offline", and can never set GUI-only fields. Sessions created this way get the default `deny` policy and content-read off. Approval responses, host-key and certificate trust decisions, credential changes, agent policy changes, master-password and agent-control settings SHALL be GUI-only and rejected from every agent entry point.

#### Scenario: Owner edit of a deny session
- **WHEN** the user edits a session whose policy is `deny` (or `ask`) with `rustxterm sessions edit` on an interactive terminal and the master password
- **THEN** the edit is applied without a GUI prompt and audited as "cli-offline"

#### Scenario: Offline creation defaults
- **WHEN** the user adds a session with `rustxterm sessions add`
- **THEN** it is stored with policy `deny` and content-read off

#### Scenario: Direct socket bypass
- **WHEN** a client sends a raw socket message invoking an approval response or a policy change
- **THEN** it is rejected and audited as denied

#### Scenario: Policy escalation through edit
- **WHEN** an agent edits a session and includes an `agent_policy` or credential field
- **THEN** the whole edit is rejected

### Requirement: Policies, effects and approvals
Every session SHALL have an agent policy: `deny` (default), `ask` or `allow`, and a separate "agent may read content" flag (default off) covering terminal text, screenshots and downloads. Every agent command SHALL be classified by its local and remote effects; any command with a write effect on either side (connect, send input, remote write/delete, local file write by `get`, local file read by `put`, tunnel start, close) SHALL require approval under `ask`. An approval SHALL name the agent client, command, connection and target, be bound to a hash of the immutable parameters, be single-use, and expire after 60 s (deny by default). Agent input SHALL never be broadcast by MultiExec to tabs whose session does not allow it.

#### Scenario: Denied by default
- **WHEN** an agent tries to open a session whose policy is `deny`
- **THEN** the action fails with "denied by policy" and nothing connects

#### Scenario: Ask
- **WHEN** an agent sends input to a tab of a session with policy `ask`
- **THEN** a prompt appears and no byte is sent until the user approves

#### Scenario: Replay or changed parameters
- **WHEN** an agent reuses an approval, or changes a parameter after approval, or the approval expired
- **THEN** the command is denied and audited

#### Scenario: Content read off
- **WHEN** an agent calls `read` on an `allow` session without the content-read flag
- **THEN** it is denied

#### Scenario: MultiExec
- **WHEN** MultiExec includes a tab of a `deny` session and an agent sends input to an `allow` tab in the group
- **THEN** only tabs of sessions that allow the agent receive the bytes

### Requirement: Agent file scope
Local paths used by agent `get` and `put` SHALL be confined to an agent workspace directory (default `$XDG_DATA_HOME/rustxterm/agent-files/`, configurable only in the GUI), resolved without following symlinks out of it; paths outside it, including `~/.ssh` and application directories, SHALL be denied.

#### Scenario: Escape attempt
- **WHEN** an agent calls `put` with `../../.ssh/id_ed25519` or a symlink inside the workspace pointing outside
- **THEN** the command is denied and audited

### Requirement: No disclosure of application-held secrets
CLI and MCP output, errors and audit records SHALL never contain application-held secrets: saved passwords, passphrases, private key contents, the database key, keyring keys or the master password; credential fields SHALL appear only as `saved` or `absent`. Content read by agents (terminal text, screenshots, downloaded files) may contain anything the remote side or user displayed, which is why it requires the separate content-read flag. (Echo-off masking for local terminals is deferred to phase 2 with the local shell.)

#### Scenario: Show session
- **WHEN** an agent requests a session with a saved password
- **THEN** the output contains `"password": "saved"` and not the password

#### Scenario: Failed login error
- **WHEN** an agent opens a session whose saved password is rejected by the server
- **THEN** the error and the audit record do not contain the password

### Requirement: Agent audit log
Every agent request SHALL be recorded in the encrypted database with time, client name (as declared, not authenticated), entry point, command, target, decision (allowed, approved, denied) and result, viewable and clearable in the GUI.

#### Scenario: Audit entry
- **WHEN** an agent's `put` is approved and completes
- **THEN** the audit view shows one entry with the client name, target path, "approved" and "success"
