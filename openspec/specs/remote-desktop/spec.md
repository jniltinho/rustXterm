# remote-desktop Specification

## Purpose
Native RDP and VNC clients embedded in tabs, so remote desktops share the same session tree, tabs and themes as terminals without external programs.

## Requirements

### Requirement: Embedded RDP client
RDP sessions SHALL connect natively inside a tab with TLS and NLA (CredSSP) authentication, username/domain/password, server certificate verification with a trust prompt (fingerprint shown; a changed certificate is refused unless the user explicitly re-trusts it), selectable resolution (fixed, or fit to tab with dynamic resize when the server supports it), keyboard layout and clipboard text sharing (toggle).

#### Scenario: Connect to lab xrdp
- **WHEN** the user opens an RDP session to the lab xrdp with valid credentials
- **THEN** the desktop renders inside the tab and mouse and keyboard input work

#### Scenario: Unknown certificate
- **WHEN** the server presents a certificate not yet trusted
- **THEN** a prompt shows its fingerprint and nothing is sent to CredSSP until the user accepts; cancelling closes the connection

#### Scenario: Changed certificate
- **WHEN** the server certificate differs from the trusted one
- **THEN** the connection stops before credentials are sent and both fingerprints are shown

#### Scenario: Resize
- **WHEN** "fit to tab" is on and the user resizes the window
- **THEN** the server receives a display-control resize, or the image is scaled when the server does not support it

#### Scenario: Windows target
- **WHEN** the user connects to the optional Windows Vagrant VM with NLA enabled
- **THEN** login succeeds and the desktop renders (manual receipt)

### Requirement: Embedded VNC client
VNC sessions SHALL connect natively inside a tab supporting RFB 3.3/3.7/3.8, VNC password authentication, at least Raw, CopyRect, ZRLE and Tight encodings when the server offers them, view-only mode, scale-to-fit, and clipboard text sharing (toggle). Sessions SHALL offer tunneling through an existing SSH session.

#### Scenario: Connect to lab VNC
- **WHEN** the user opens a VNC session to the lab VNC server with the right password
- **THEN** the framebuffer renders in the tab and updates within 500 ms of a remote change

#### Scenario: View only
- **WHEN** view-only is on
- **THEN** no pointer or key events are sent to the server

#### Scenario: Through SSH
- **WHEN** a VNC session is set to tunnel through an SSH session
- **THEN** the VNC stream uses a direct-tcpip channel on that SSH connection and no local port is opened

### Requirement: Remote desktop input and focus
While a remote desktop tab is focused, system-like key combinations (Alt+Tab, Super) SHALL be forwarded only when "grab keyboard" is enabled, and a visible, configurable release combination SHALL always return focus to the application.

#### Scenario: Release key
- **WHEN** keyboard grab is on and the user presses the release combination
- **THEN** focus returns to the application and subsequent keys are not forwarded

### Requirement: Bounded remote desktop resources
A remote desktop tab SHALL keep at most one framebuffer copy plus one GPU texture, SHALL reject server-declared sizes above 8192×8192, SHALL cap decompressed rectangle sizes and queued update bytes (default 64 MiB per tab), SHALL close a session whose peer stalls mid-message beyond a 30 s timeout, SHALL stop requesting updates while hidden when the protocol allows it, and SHALL keep tab switching at or below 100 ms p95 while the remote side plays full-screen video.

#### Scenario: Hostile server sizes
- **WHEN** a test server declares a 100000×100000 framebuffer or a rectangle that decompresses beyond the cap
- **THEN** the session closes with an error and process memory stays within the cap

#### Scenario: Stalled peer
- **WHEN** a test server stops sending in the middle of an update
- **THEN** the tab can be closed immediately and the session ends within 30 s without leaking tasks

#### Scenario: Hidden tab
- **WHEN** a VNC tab is hidden behind another tab for 60 s
- **THEN** no framebuffer update requests are sent while hidden and the tab redraws within 500 ms when shown

### Requirement: Remote desktop credentials
RDP and VNC passwords SHALL be saved only in the encrypted database under the same rules as other credentials and never logged.

#### Scenario: Debug logging
- **WHEN** debug logging is enabled during an RDP login
- **THEN** the log contains no password, NTLM hash or session key material
