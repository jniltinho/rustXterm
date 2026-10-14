## ADDED Requirements

### Requirement: r-commands quick connect
When legacy r-commands are enabled, the quick-connect field SHALL accept `rlogin://[user@]host[:port]` and `rsh://[user@]host[:port]` (both open an rlogin session) and `rsh://[user@]host[:port]?cmd=<url-encoded command>` (opens an rsh command session). When disabled, these prefixes SHALL show an inline error naming the Settings toggle.

#### Scenario: Quick rsh command
- **WHEN** legacy r-commands are enabled and the user enters `rsh://alice@old-unix.lab?cmd=uptime`
- **THEN** an rsh tab runs `uptime` on old-unix.lab as alice on port 514

#### Scenario: Prefix while disabled
- **WHEN** legacy r-commands are disabled and the user enters `rlogin://old-unix.lab`
- **THEN** the field shows an inline error naming the Settings toggle and no tab opens

### Requirement: Legacy protocol settings
Settings SHALL contain a Security section with the "Enable legacy r-commands (rlogin/rsh/rexec, insecure)" toggle, off by default, persisted in the basic settings file.

#### Scenario: Enable toggle
- **WHEN** the user enables the toggle
- **THEN** the Rsh type appears in the session editor immediately without restart
