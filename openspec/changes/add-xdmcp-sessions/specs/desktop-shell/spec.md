## ADDED Requirements

### Requirement: XDMCP quick connect
The quick-connect field SHALL accept `xdmcp://host[:port]` (UDP port 177 by default) and open an XDMCP session without saving it, after the plaintext confirmation.

#### Scenario: Quick XDMCP
- **WHEN** the user enters `xdmcp://unix-ws.lab` and confirms the plaintext warning
- **THEN** an XDMCP tab opens to unix-ws.lab on UDP port 177

#### Scenario: Invalid XDMCP input
- **WHEN** the user enters `xdmcp://` with no host
- **THEN** the field shows an inline error and no tab opens
