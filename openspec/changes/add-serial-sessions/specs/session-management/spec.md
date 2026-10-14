## ADDED Requirements

### Requirement: Serial session editor
The session editor SHALL offer a Serial type with a port picker (refreshable list and free text), baud rate, data bits, parity, stop bits, flow control, Enter sequence, received-LF handling, local echo, initial DTR/RTS and reconnect-on-replug settings, plus the shared Terminal page; host and network port fields SHALL not be shown for Serial.

#### Scenario: Missing port
- **WHEN** the user saves a Serial session without a port
- **THEN** the editor shows an error and Save stays disabled

### Requirement: MobaXterm.ini Serial import
The MobaXterm.ini importer SHALL map Serial bookmarks (session type field `8`, confirmed with sample files before implementation) to Serial sessions with port and line settings when parseable, and SHALL report unparseable line settings while importing the port with defaults.

#### Scenario: Import Serial bookmark
- **WHEN** the user imports an ini containing a Serial bookmark for `COM3` at 9600 baud
- **THEN** the preview lists it as importable with 9600 baud and flags that the Windows port name must be replaced with a Linux device path
