## Purpose

XDMCP sessions that show a remote host's graphical login and X desktop inside a tab, using an external headless X server and the embedded VNC viewer, with explicit plaintext warnings.

## ADDED Requirements

### Requirement: XDMCP availability probe
Before starting an XDMCP session, the application SHALL send an XDMCP query to the host's XDMCP port (default UDP 177) and SHALL report a willing host, an unwilling host (with its status text) or no answer within 3 seconds as distinct outcomes. Datagrams larger than 1 KiB or malformed SHALL be ignored.

#### Scenario: Host unwilling
- **WHEN** the display manager answers Unwilling
- **THEN** the tab shows an error with the host's status text and no X server is started

#### Scenario: No answer
- **WHEN** the host does not answer within 3 seconds
- **THEN** the tab explains that XDMCP is disabled on the host or UDP 177 is blocked, and no X server is started

### Requirement: External X server
XDMCP sessions SHALL use an external, separately installed headless X server with built-in VNC (TigerVNC Xvnc) started by the application for that tab only. If it is not installed or too old, the application SHALL say so with installation hints per distribution and SHALL NOT start the session or fall back to another client. The X server SHALL never expose its VNC endpoint on the network, SHALL require XDMCP-negotiated authorization for X11 clients, and SHALL be stopped when the tab closes, the application exits or the remote session ends.

#### Scenario: Xvnc missing
- **WHEN** the user opens an XDMCP session and Xvnc is not installed
- **THEN** a dialog names the package for Debian/Ubuntu, Fedora and Arch and no process is started

#### Scenario: Cleanup on close
- **WHEN** the user closes a connected XDMCP tab
- **THEN** the X server process started for it is gone within 2 seconds

### Requirement: Embedded XDMCP display
A willing host's login screen and desktop SHALL be displayed inside a remote-desktop tab with the same input, keyboard grab, scaling, hidden-tab and resource limits as VNC tabs, on both X11 and Wayland desktop sessions.

#### Scenario: Greeter shown
- **WHEN** the user opens an XDMCP session to a willing host
- **THEN** the remote greeter appears in the tab within 20 seconds and the user can log in with the keyboard

#### Scenario: Wayland desktop
- **WHEN** rustXterm runs on a Wayland session
- **THEN** the XDMCP display is shown inside the tab and no extra top-level window is opened

### Requirement: Display manager connection failure
If the host is willing but no display manager connection reaches the local X server within the session timeout (default 30 seconds), the tab SHALL explain that the remote host must reach the local X display port over TCP and that the display manager's access list must allow this client.

#### Scenario: Firewall blocks connect-back
- **WHEN** the host answers Willing but its connection back to the local X port is blocked
- **THEN** after the timeout the tab shows the connect-back hint with the display port number and stops the X server

### Requirement: XDMCP plaintext warning
XDMCP tabs SHALL show the "Unencrypted" marker, and the first connection of each XDMCP session SHALL require a confirmation stating that the login, keystrokes and screen contents travel unencrypted and that the session is meant for trusted networks.

#### Scenario: First connect confirmation
- **WHEN** the user opens a saved XDMCP session for the first time
- **THEN** a confirmation describing the plaintext risk is shown and nothing is sent to the host if the user cancels
