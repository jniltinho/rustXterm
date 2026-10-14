## ADDED Requirements

### Requirement: Remote monitoring channel
When remote monitoring is effective for an SSH tab, the client SHALL open one additional session channel with an exec request on the tab's connection to the final target, after the shell (and SFTP, when used) channels are open, and never on a jump host. The session option "Remote monitoring" (Use global setting, On, Off) SHALL let users opt a session out. A server refusing the channel SHALL never affect the shell, SFTP or tunnels of the same connection.

#### Scenario: Opt-out sends no exec
- **WHEN** a session with Remote monitoring set to Off connects
- **THEN** the server receives no monitoring exec request on that connection

#### Scenario: Channel limit
- **WHEN** the server refuses the monitoring channel because its per-connection session limit is reached
- **THEN** the terminal and SFTP panel keep working and monitoring is reported as unavailable
