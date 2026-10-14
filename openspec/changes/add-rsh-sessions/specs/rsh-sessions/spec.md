## Purpose

Legacy rlogin, rsh and rexec sessions for old Unix hosts and isolated labs, disabled by default, clearly marked as insecure, and honest about the privileged source port they require.

## ADDED Requirements

### Requirement: Legacy r-commands toggle
rlogin, rsh and rexec sessions SHALL be disabled by default and enabled only through an explicit Settings toggle labelled as insecure. While disabled, the application SHALL NOT open, create, quick-connect, import or let agents open such sessions, and SHALL name the toggle in every refusal.

#### Scenario: Disabled by default
- **WHEN** a fresh profile opens the session editor
- **THEN** the Rsh type is not offered

#### Scenario: Existing session while disabled
- **WHEN** the toggle is off and the user double-clicks a saved Rsh session
- **THEN** nothing connects and the message points to the Settings toggle

### Requirement: rlogin sessions
With the toggle on, the application SHALL open rlogin sessions (default TCP 513) in a terminal tab, presenting a configurable local user name, the remote user name and the terminal type, and SHALL send the window size after login and on every resize.

#### Scenario: Trusted host login
- **WHEN** the server trusts the presented local user for the remote user and the client connects from a privileged source port
- **THEN** a remote shell opens without a password prompt

#### Scenario: Resize
- **WHEN** the user resizes a connected rlogin tab to 100×30
- **THEN** the server's `stty size` reports `30 100`

### Requirement: rsh and rexec command sessions
With the toggle on, the application SHALL run one remote command per rsh session (default TCP 514) or rexec session (default TCP 512) in a terminal tab, forward typed input to the command, show server error messages, and mark the tab as finished when the command ends. rexec passwords SHALL come only from the encrypted store when saved, or from a prompt, and SHALL never appear in logs, errors or agent output.

#### Scenario: rsh command
- **WHEN** the user runs an rsh session with command `uname -a` on a host that trusts them
- **THEN** the output appears and the tab shows the command finished

#### Scenario: rexec wrong password
- **WHEN** the rexec server rejects the password
- **THEN** the tab shows the server's error text and no password is written to the session log

### Requirement: Privileged source port handling
For rlogin and rsh the application SHALL try to connect from a source port between 512 and 1023 and SHALL NOT use setuid helpers, capabilities, privilege-elevation tools or change system settings to obtain one. When the operating system refuses such a port and the session does not allow unprivileged source ports, the application SHALL explain why the server requires a privileged port and list the alternatives (rexec mode, SSH, or an administrator-set unprivileged port range), showing the current system setting. When the session allows unprivileged source ports, it SHALL connect from an ordinary port and report a server refusal as "server requires a privileged source port".

#### Scenario: Unprivileged desktop user
- **WHEN** a normal user opens an rlogin session and binding ports below 1024 is not permitted
- **THEN** no connection is attempted from an unprivileged port and the explanation with alternatives is shown

#### Scenario: Server refuses unprivileged port
- **WHEN** the session allows unprivileged source ports and the server rejects the connection
- **THEN** the tab shows "server requires a privileged source port"

### Requirement: r-commands insecurity labelling
rlogin, rsh and rexec tabs SHALL show the "Unencrypted" marker, the editor page SHALL show a warning that these protocols send data in clear text and trust client-supplied identities, and the first connection of each session SHALL require confirmation.

#### Scenario: First connect
- **WHEN** the user opens a saved rlogin session for the first time
- **THEN** a confirmation describing the risks is shown and nothing is sent if the user cancels
