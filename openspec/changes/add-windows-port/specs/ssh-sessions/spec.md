## ADDED Requirements

### Requirement: SSH agent and X11 on Windows
On Windows, agent authentication and agent forwarding SHALL use the Windows OpenSSH agent pipe (or the pipe named by `SSH_AUTH_SOCK` when set) and SHALL fall back to Pageant when that pipe is absent; with neither, agent authentication is skipped and the next method is tried. X11 forwarding SHALL connect to a TCP display given by `DISPLAY`; when `DISPLAY` is unset the tab SHALL report that no local X server is configured and continue the session without X11. Host keys SHALL be read from `%USERPROFILE%\.ssh\known_hosts`.

#### Scenario: Windows OpenSSH agent
- **WHEN** the Windows `ssh-agent` service holds the user's key and a session uses agent authentication
- **THEN** the session authenticates without asking for a password

#### Scenario: No X server
- **WHEN** an SSH session with X11 forwarding enabled opens on Windows and `DISPLAY` is unset
- **THEN** the shell opens and the tab shows a notice that no local X server is configured
