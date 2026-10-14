# file-transfer Specification

## Purpose
SFTP file browser side panel attached to SSH tabs, plus standalone SFTP and FTP/FTPS dual-pane sessions.

## Requirements

### Requirement: SFTP side panel
When an SSH tab is active the sidebar SFTP tab SHALL browse the same server over an SFTP subsystem on the same connection, without asking for credentials again. The panel SHALL list name, size, modification date and permissions, and support navigation, refresh, hidden-files toggle and path entry.

#### Scenario: Browser follows SSH tab
- **WHEN** the user switches between two SSH tabs
- **THEN** the SFTP panel shows the directory of the active tab's server

### Requirement: Follow terminal folder
The SFTP panel SHALL optionally follow the current directory of the terminal when the remote shell reports it (OSC 7), and fall back to the home directory otherwise.

#### Scenario: cd follows
- **WHEN** "follow terminal folder" is on and the user runs `cd /var/log` in a bash that emits OSC 7
- **THEN** the panel shows /var/log

### Requirement: File operations
The panel SHALL support download, upload (button and drag-and-drop from the desktop file manager), create folder, rename, delete (with confirmation), change permissions and open-with local application (download to temp, upload back on save when the user confirms).

#### Scenario: Drag-and-drop upload
- **WHEN** the user drops a 100 MB file from the file manager onto the panel
- **THEN** a transfer row shows progress and the file appears in the listing on completion

#### Scenario: Open with local application
- **WHEN** the user opens a remote file with a local editor, saves it and confirms upload
- **THEN** the remote file contains the edited content and the temporary local copy is deleted when the tab closes

#### Scenario: Overwrite
- **WHEN** an upload targets an existing remote file
- **THEN** the user chooses overwrite, skip or rename before any byte is written

### Requirement: Transfers
Transfers SHALL run in the background with progress, speed, cancel and an error per file, and SHALL NOT block the terminal or UI.

#### Scenario: Cancel
- **WHEN** the user cancels an in-progress download
- **THEN** the transfer stops and the partial local file is removed

### Requirement: Standalone SFTP session
An SFTP session type SHALL open a dual-pane tab (local and remote) with the same operations.

#### Scenario: Open SFTP session
- **WHEN** the user opens a saved SFTP session
- **THEN** a tab shows local home on one side and remote home on the other

#### Scenario: Dual-pane copy
- **WHEN** the user drags a file from the local pane to the remote pane
- **THEN** it uploads with the same progress, overwrite and cancel behavior as the side panel

### Requirement: FTP and FTPS sessions
FTP sessions SHALL open a dual-pane tab with the same operations and transfer behavior as SFTP, supporting plain FTP, explicit FTPS (AUTH TLS) and implicit FTPS (default port 990), passive (default) and active mode, binary (default) and ASCII transfer, UTF-8 file names and a remote startup folder. FTPS SHALL verify the server certificate against the system trust store with a trust prompt for unknown certificates, and plain FTP SHALL show a visible "unencrypted" marker on the tab.

#### Scenario: Explicit FTPS
- **WHEN** the user opens an explicit FTPS session to the lab vsftpd with a self-signed certificate
- **THEN** a trust prompt shows the certificate fingerprint and, after acceptance, the remote listing appears

#### Scenario: Plain FTP warning
- **WHEN** the user opens a plain FTP session
- **THEN** the tab shows the unencrypted marker and transfers work

#### Scenario: Passive and active
- **WHEN** the session switches between passive and active mode against the lab server
- **THEN** listing and a 10 MB upload succeed in both modes with matching checksums
