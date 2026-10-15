## ADDED Requirements

### Requirement: Terminal replies stay in their tab
Bytes the terminal generates in answer to the session (device attributes, cursor position reports and similar replies) SHALL be sent only to the tab whose output asked for them, never broadcast through MultiExec.

#### Scenario: Reply not broadcast
- **WHEN** MultiExec is on with tabs A and B selected and tab A's remote side sends a cursor position request
- **THEN** the reply is sent to tab A only and tab B receives no bytes

## MODIFIED Requirements

### Requirement: Session logging
The user SHALL be able to log terminal output of a session to a file chosen in session settings, and save the visible terminal text to a file on demand. Local terminal tabs have no session settings: they offer only saving the visible text on demand, and SHALL NOT write a log file.

#### Scenario: Log to file
- **WHEN** logging is enabled for a session and the user runs `ls`
- **THEN** the output appears in the log file without terminal escape sequences when "plain text" is selected, and with them when "raw" is selected

#### Scenario: Local terminal text
- **WHEN** the user chooses "Save terminal text" in a local terminal tab
- **THEN** the visible text is written to the chosen file and no log file is created for the tab
