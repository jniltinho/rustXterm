## ADDED Requirements

### Requirement: Rsh session editor
When legacy r-commands are enabled, the session editor SHALL offer an Rsh type with mode (rlogin, rsh, rexec), host, port defaulting per mode, remote user, local user to present, command (rsh and rexec), allow-unprivileged-source-port (off by default) and save-password (rexec only), with an insecurity warning banner.

#### Scenario: Command required
- **WHEN** the user saves an rsh-mode session without a command
- **THEN** the editor shows an error and Save stays disabled

### Requirement: MobaXterm.ini Rsh import
When legacy r-commands are enabled, the MobaXterm.ini importer SHALL map Rsh bookmarks (session type field `2`, confirmed with sample files before implementation) to Rsh sessions with host, user and command. When disabled, it SHALL list them as skipped with the reason "legacy r-commands disabled in Settings".

#### Scenario: Import while disabled
- **WHEN** the toggle is off and the ini contains one Rsh bookmark
- **THEN** the preview lists it as skipped with the Settings reason and imports the rest
