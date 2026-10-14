## ADDED Requirements

### Requirement: Telnet session editor
The session editor SHALL offer a Telnet type with host, port, username, raw mode, newline sequence, terminal type and auto-login settings (enable flag, login and password prompts), plus the shared Terminal and Network pages, and SHALL show a plaintext warning on the Telnet page.

#### Scenario: Auto-login needs a saved password
- **WHEN** the user enables auto-login without a username or without "save password"
- **THEN** the editor shows an error and Save stays disabled

### Requirement: MobaXterm.ini Telnet import
The MobaXterm.ini importer SHALL map Telnet bookmarks (session type field `1`, confirmed with sample files before implementation) to Telnet sessions with host, port and username, and SHALL never import or enable auto-login or passwords.

#### Scenario: Import Telnet bookmark
- **WHEN** the user imports an ini with one SSH and one Telnet bookmark
- **THEN** the preview lists both as importable and the Telnet session has auto-login disabled
