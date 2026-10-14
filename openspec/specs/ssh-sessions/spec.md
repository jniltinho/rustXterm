# ssh-sessions Specification

## Purpose
SSH sessions with configurable authentication, trust and forwarding options.

## Requirements

### Requirement: SSH connection and authentication
SSH sessions SHALL support host, port (default 22), username, password, public-key files (OpenSSH and PEM, with passphrase), the user's SSH agent (`SSH_AUTH_SOCK`) and keyboard-interactive (including 2FA prompts shown in a dialog). Authentication methods SHALL be tried in the order agent, key, keyboard-interactive, password unless the session restricts them.

#### Scenario: Key authentication
- **WHEN** a session uses an ed25519 private key accepted by the server
- **THEN** the tab opens a shell without asking for a password

#### Scenario: Wrong password
- **WHEN** the server rejects the password
- **THEN** the tab offers to retry with a new password and does not store the rejected one

### Requirement: Host key trust
The application SHALL verify server host keys against `~/.ssh/known_hosts` (read only) and its own trust records in the encrypted database (read/write). On an unknown key it SHALL show the fingerprint (SHA-256) and ask to accept once, accept and save, or cancel. On a changed key it SHALL refuse to connect and show both fingerprints.

Trust evaluation SHALL fail closed: an existing but unreadable `~/.ssh/known_hosts` blocks the connection with an error naming the file, and a locked database blocks connections that need its trust records; malformed lines are ignored with a warning and never count as a match; `@revoked` keys are refused; `@cert-authority` lines are ignored with a warning (certificates unsupported in v0.1); hashed hostnames and `[host]:port` entries are honored; a key that matches in one source but conflicts with another entry of the same key algorithm for the same host in either source is treated as changed; entries of different algorithms for the same host are not a conflict. No authentication data SHALL be sent to any hop, including jump hosts, before its host key is trusted.

#### Scenario: Revoked key
- **WHEN** the server key is listed under `@revoked`
- **THEN** the connection is refused and no authentication is attempted

#### Scenario: Unreadable known_hosts
- **WHEN** `~/.ssh/known_hosts` exists but cannot be read
- **THEN** the connection is blocked with an error naming the file

#### Scenario: Multiple algorithms
- **WHEN** `known_hosts` has an ed25519 and an rsa entry for the host and the server presents the matching ed25519 key
- **THEN** the connection proceeds without a warning

#### Scenario: Hashed host on custom port
- **WHEN** `~/.ssh/known_hosts` contains a hashed entry for `[lab.example]:2222` matching the server key
- **THEN** the connection proceeds without a trust prompt

#### Scenario: Changed host key
- **WHEN** the server presents a key different from the stored one
- **THEN** the connection is aborted, a warning shows old and new fingerprints, and no credential is sent

### Requirement: Jump hosts
A session SHALL allow one or more SSH jump hosts (gateway chain), each with its own credentials.

#### Scenario: Through a bastion
- **WHEN** a session targets `db.internal` via jump host `bastion.example`
- **THEN** the tab connects to db.internal through bastion and both host keys are verified

### Requirement: Session options
SSH sessions SHALL support remote command execution (with "do not exit after command ends"), compression, keepalive interval, X11 forwarding to the local display (off by default), agent forwarding (off by default) and remote environment variables and startup directory. X11 forwarding SHALL send a random fake MIT-MAGIC-COOKIE-1 to the server, accept only X11 channels opened while forwarding is requested and carrying that cookie, and replace it with the local display's real cookie before connecting to the local X server.

#### Scenario: X11 forwarding
- **WHEN** X11 forwarding is enabled and the user runs `xeyes` remotely
- **THEN** the window appears on the local desktop (X11 or XWayland)

#### Scenario: Compression and keepalive
- **WHEN** a session enables compression and a 15 s keepalive against the lab sshd
- **THEN** the negotiated compression is `zlib@openssh.com` and the server log shows keepalive requests every 15 s while idle

#### Scenario: Bad X11 cookie
- **WHEN** the server opens an X11 channel with a wrong cookie, or X11 forwarding was not requested
- **THEN** the channel is rejected and nothing connects to the local display

#### Scenario: Command completes
- **WHEN** a session runs remote command `uname -a` with "do not exit after command ends" enabled
- **THEN** the output stays visible and the tab offers a new shell or reconnect instead of closing

#### Scenario: Startup directory and environment
- **WHEN** a session sets startup directory `/tmp` and environment `FOO=bar`
- **THEN** in the lab, `pwd` prints `/tmp` and `echo $FOO` prints `bar` (when the server accepts `FOO` via AcceptEnv)

#### Scenario: Agent forwarding off by default
- **WHEN** a new SSH session is created
- **THEN** agent forwarding is disabled until the user enables it

### Requirement: Port-forwarding tunnels
The application SHALL manage named tunnels of type local, remote and dynamic (SOCKS5), each bound to an SSH server, with start/stop, autostart and status. Local and dynamic listeners SHALL bind to 127.0.0.1 and remote forwards SHALL request the server-side loopback address, unless the user explicitly chooses another address and confirms the exposure warning for that tunnel. Tunnels marked autostart SHALL start when the application starts and show their state.

#### Scenario: Local forward
- **WHEN** a local tunnel 127.0.0.1:15432 → db.internal:5432 via `bastion` is started
- **THEN** connections to 127.0.0.1:15432 reach db.internal:5432 and the tunnel shows "running"

#### Scenario: Not exposed by default
- **WHEN** a local, dynamic or remote tunnel is created with default settings
- **THEN** connecting from a non-loopback address of the respective host is refused

#### Scenario: Autostart
- **WHEN** a tunnel has autostart enabled and the application starts
- **THEN** the tunnel reaches "running" without user action, or shows its error

#### Scenario: Port in use
- **WHEN** the local port is already taken
- **THEN** the tunnel shows an error naming the port and stays stopped

### Requirement: Reconnect
A disconnected SSH tab SHALL offer manual reconnect, and optionally automatic reconnect with backoff, reusing stored settings and prompting again for secrets that were not saved.

#### Scenario: Network drop
- **WHEN** the network drops and auto-reconnect is enabled
- **THEN** the tab retries with increasing delay and shows the next attempt time
