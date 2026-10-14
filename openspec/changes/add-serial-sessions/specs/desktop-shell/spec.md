## ADDED Requirements

### Requirement: Serial quick connect
The quick-connect field SHALL accept `serial://<device>[?baud=&data=&parity=&stop=&flow=]`, where `<device>` is an absolute path (`serial:///dev/ttyUSB0`) or a name under `/dev` (`serial://ttyUSB0`), and open a serial session without saving it. Omitted parameters SHALL default to 115200 baud, 8 data bits, no parity, 1 stop bit and no flow control.

#### Scenario: Quick serial with baud
- **WHEN** the user enters `serial:///dev/ttyUSB0?baud=9600`
- **THEN** a serial tab opens on `/dev/ttyUSB0` at 9600 8N1 without flow control

#### Scenario: Invalid serial parameters
- **WHEN** the user enters `serial:///dev/ttyUSB0?parity=q`
- **THEN** the field shows an inline error naming `parity` and no tab opens
