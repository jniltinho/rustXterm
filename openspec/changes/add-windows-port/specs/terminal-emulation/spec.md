## ADDED Requirements

### Requirement: Terminal on Windows
Remote terminal sessions (SSH, Telnet, Serial) SHALL behave on Windows as on Linux, including clipboard copy and paste through the Windows clipboard (no primary selection). Local shell tabs are phase 2: on Windows they SHALL NOT be offered until a local pseudo-console (ConPTY) backend exists, and no command or menu SHALL claim to start one.

#### Scenario: Copy and paste on Windows
- **WHEN** the user selects text in an SSH tab and pastes it into Notepad
- **THEN** the text matches the selection

#### Scenario: Local shell not offered
- **WHEN** the user opens the new-session menu on Windows before the local shell exists
- **THEN** no local shell entry is shown
