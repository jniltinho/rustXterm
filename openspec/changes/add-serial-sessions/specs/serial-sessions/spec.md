## Purpose

Serial console sessions for network devices, single-board computers and microcontrollers, with port discovery, line configuration and robust handling of unplug and permission errors.

## ADDED Requirements

### Requirement: Serial port discovery
The application SHALL list available serial ports with their device path and, when known, a stable identity and a human description (USB vendor, product and serial number). It SHALL also accept a port path typed by hand.

#### Scenario: USB adapter listed
- **WHEN** a USB-serial adapter is connected and the user opens the port picker
- **THEN** it lists `/dev/ttyUSB0` with the adapter's description and the list refreshes on demand

#### Scenario: Manual path
- **WHEN** the user types a pseudo-terminal path that is not in the list
- **THEN** the session opens that path

### Requirement: Serial line configuration
A serial session SHALL configure baud rate (standard rates and a custom value), data bits 5–8, parity none/odd/even/mark/space, stop bits 1 or 2, flow control none/RTS-CTS/XON-XOFF, and the initial DTR and RTS state. Unsupported combinations SHALL be reported as an error naming the setting, without opening a half-configured port.

#### Scenario: Apply settings
- **WHEN** the user opens a session configured as 9600 7E1 with RTS-CTS
- **THEN** the port's line attributes read back as 9600 baud, 7 data bits, even parity, 1 stop bit and hardware flow control

#### Scenario: Unsupported setting
- **WHEN** the device rejects the configured custom baud rate
- **THEN** the tab shows an error naming the baud rate and the port is closed

### Requirement: Serial terminal I/O
Serial tabs SHALL display received bytes in the terminal and send typed input. Enter SHALL send CR, LF or CR LF per session setting, received LF SHALL be expanded to CR LF when that setting is enabled, and local echo SHALL be available per session. Serial tabs SHALL support scrollback, search, clipboard, session logging, MultiExec and close confirmation.

#### Scenario: Echo both ways
- **WHEN** the user types `help` and Enter on a session whose Enter sends CR
- **THEN** the device receives `help\r` and its response is displayed

#### Scenario: Local echo
- **WHEN** local echo is on and the device does not echo
- **THEN** typed characters appear once in the terminal

### Requirement: Serial line control commands
The application SHALL offer commands to send a break and to set DTR and RTS on an open serial session, available from the tab menu, shortcuts and agent control under the existing authorization rules.

#### Scenario: Send break
- **WHEN** the user chooses Send break on a connected serial tab
- **THEN** a break condition is sent on the line and the session stays open

### Requirement: Exclusive port access
The application SHALL open serial ports for exclusive use and SHALL report a busy port clearly instead of sharing it.

#### Scenario: Port in use
- **WHEN** a port is already open in another tab or program with exclusive access
- **THEN** the new session shows "port in use" and does not open

### Requirement: Device removal and reconnect
When the device disappears, the tab SHALL show a disconnected state that names device removal, keep its scrollback, and, when reconnect is enabled (default), reopen the same device automatically within 2 seconds of it reappearing, matching by stable identity when known.

#### Scenario: Unplug and replug
- **WHEN** the user unplugs a USB-serial adapter and plugs it back in, and it reappears as `/dev/ttyUSB1`
- **THEN** the tab shows "device removed", then reconnects to the same adapter on its new path and keeps the earlier scrollback

### Requirement: Permission errors
When opening a port is denied, the application SHALL show the device path, the group that owns it and how to join that group, and SHALL never escalate privileges or change device permissions itself.

#### Scenario: Missing dialout membership
- **WHEN** the user is not in the group owning `/dev/ttyUSB0` (e.g. `dialout`)
- **THEN** the error names `/dev/ttyUSB0` and `dialout` and explains to add the user to the group and log in again, and no privileged program is started
