## ADDED Requirements

### Requirement: Telnet quick connect
The quick-connect field SHALL accept `telnet://[user@]host[:port]` and open a Telnet session on port 23 by default without saving it. A given user SHALL only prefill the session's username and SHALL never trigger auto-login.

#### Scenario: Quick Telnet
- **WHEN** the user enters `telnet://router.lab:2323`
- **THEN** a Telnet tab opens to router.lab port 2323 with the Unencrypted marker

#### Scenario: Invalid Telnet input
- **WHEN** the user enters `telnet://` with no host
- **THEN** the field shows an inline error and no tab opens
