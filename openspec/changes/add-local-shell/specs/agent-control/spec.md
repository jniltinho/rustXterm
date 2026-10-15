## ADDED Requirements

### Requirement: Local terminal tabs
Agents SHALL see local terminal tabs in tab listings (id, name, protocol `local`, state). Every other agent operation that targets a local tab (read, wait, send, screenshot, file operations, close, serial controls) SHALL be denied with an error saying local terminals are not available to agents, and audited as denied. Agent `send` to a remote tab in a MultiExec group SHALL deliver nothing to local tabs; the result SHALL list them as `skipped` and the audit record of that request SHALL name them. Agent requests while the session database is locked keep the existing `locked` response. This holds until echo-off masking for local terminals is specified.

#### Scenario: Read refused
- **WHEN** an agent with content-read permission reads a local terminal tab
- **THEN** the request is denied with the local-terminal error, nothing is returned and the denial is audited

#### Scenario: Listed
- **WHEN** an agent lists tabs while a local terminal is open
- **THEN** the tab appears with protocol `local`

#### Scenario: MultiExec skips local tabs
- **WHEN** an agent sends input to a remote tab in a MultiExec group that includes a local terminal
- **THEN** zero bytes reach the local terminal, the result lists it as skipped and the audit record names it
