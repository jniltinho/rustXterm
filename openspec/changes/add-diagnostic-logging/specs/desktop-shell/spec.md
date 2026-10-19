## ADDED Requirements

### Requirement: Diagnostic logging settings
The application SHALL offer a Diagnostics settings page that switches a diagnostic log file on and off at runtime with the levels Off, Errors, Normal, Full debug and Protocol trace (for chosen protocols), writes it to the folder where rustXterm runs by default (the config folder when that folder is not writable, or a folder the user chooses), rotates files at 50 MB keeping three, and never writes passwords, private keys or NTLM/CredSSP messages to it at any level.

#### Scenario: Turn on full debug without a restart
- **WHEN** the user selects "Full debug" on the Diagnostics page
- **THEN** the next connection's steps are appended to `rustxterm-diagnostic.log` in the shown folder, without restarting the application

#### Scenario: Default folder not writable
- **WHEN** rustXterm runs from a directory it cannot write (an app started from Finder, a Windows shortcut in Program Files) and a level other than Off is selected
- **THEN** the log is written to the config folder and the page shows that absolute path

#### Scenario: Protocol trace for one protocol
- **WHEN** the user selects "Protocol trace" with only RDP chosen
- **THEN** RDP code logs at trace level while the other protocols stay at debug

#### Scenario: Secrets never logged
- **WHEN** an RDP login with NLA and an SSH password login run with "Protocol trace" for every protocol
- **THEN** the diagnostic log contains neither password in plain text, UTF-16, hex or Base64, nor NTLM messages

#### Scenario: Switch off and on during a connection
- **WHEN** a connection is in progress and the user goes from Off to Full debug and back to Off
- **THEN** the log records the connection only while Full debug was selected and the connection is not interrupted

#### Scenario: Planted link refused
- **WHEN** `rustxterm-logs/rustxterm-diagnostic.log` is a symbolic link or a hard link on Linux or macOS
- **THEN** the file is not opened and the page reports why

#### Scenario: Command line wins
- **WHEN** rustXterm starts with `--debug-log PATH`
- **THEN** that file receives the full debug log and the Diagnostics page shows the level as set by the command line

### Requirement: Session data-stream capture
The application SHALL offer a "Record data stream" switch, turned on only after explicit consent each time, that saves each remote session's traffic after authentication to its own file with timestamps, direction and a hex and text dump; records sent bytes only when "Include typed input" is also chosen; never records credentials the application sends itself or Serial's synthetic local echo; records only message types and lengths for RDP and VNC; excludes local terminal tabs; is off at every start; and shows a status-bar indicator while it is on. Protocol tasks SHALL NOT block on file writes: records go through a bounded queue, and dropped records are counted in the file.

#### Scenario: Received bytes only by default
- **WHEN** stream recording is on without "Include typed input" and the user types a password at an SSH prompt
- **THEN** the session's stream file contains the server's output but not the typed characters

#### Scenario: Off after restart
- **WHEN** the user turns stream recording on and restarts rustXterm
- **THEN** stream recording is off and no stream file grows

#### Scenario: Indicator while recording
- **WHEN** stream recording is on
- **THEN** the status bar shows a red "Recording stream" chip that opens the Diagnostics page

#### Scenario: Saved and automatic credentials never recorded
- **WHEN** stream recording is on with "Include typed input" and a session logs in with a saved SSH password, and a Telnet session uses auto-login
- **THEN** neither stream file contains either password in plain text, hex or Base64

#### Scenario: Serial local echo not recorded
- **WHEN** stream recording is on without "Include typed input" on a Serial session with local echo
- **THEN** the typed characters do not appear in the stream file

#### Scenario: Local terminal excluded
- **WHEN** stream recording is on and a local terminal tab is open
- **THEN** no stream file is created for the local tab

#### Scenario: Slow disk does not stall a session
- **WHEN** the stream queue is full
- **THEN** the session keeps running, the extra records are dropped and the file records how many
