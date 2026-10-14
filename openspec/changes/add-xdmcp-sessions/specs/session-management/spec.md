## ADDED Requirements

### Requirement: XDMCP session editor
The session editor SHALL offer an XDMCP type with host, XDMCP port, resolution, color depth and session timeout, show the plaintext warning, and show whether a suitable external X server is installed.

#### Scenario: X server status shown
- **WHEN** the user selects the XDMCP type and Xvnc is not installed
- **THEN** the page shows that it is missing with the install hint, and saving the session is still allowed

### Requirement: MobaXterm.ini XDMCP import
The MobaXterm.ini importer SHALL map XDMCP bookmarks (session type field `3`, confirmed with sample files before implementation) to XDMCP sessions with host and port, and SHALL report X server options from the file that it cannot map.

#### Scenario: Import XDMCP bookmark
- **WHEN** the user imports an ini containing an XDMCP bookmark
- **THEN** the preview lists it as importable with its host and lists unmapped X server options as notes
