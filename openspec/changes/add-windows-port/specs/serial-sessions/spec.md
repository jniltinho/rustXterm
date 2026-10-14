## ADDED Requirements

### Requirement: Serial ports on Windows
On Windows the port list SHALL show COM ports (`COM3`) with USB vendor, product and serial number when known, and the user SHALL be able to type a port name such as `COM12`. Quick connect SHALL accept `serial://COM3[?…]`. A port already open by another program SHALL be reported as busy; an unplugged adapter SHALL be reported as device removed with the same reconnect behavior as Linux. Permission errors SHALL NOT mention Unix groups.

#### Scenario: COM port listed
- **WHEN** a USB-serial adapter is connected on Windows and the user opens the port picker
- **THEN** it lists the adapter's `COMn` port with its description

#### Scenario: COM port busy
- **WHEN** another program holds `COM3` and the user opens a session on `COM3`
- **THEN** the tab reports that the port is in use
