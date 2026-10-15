## ADDED Requirements

### Requirement: Terminal on Windows
Terminal sessions (SSH, Telnet, Serial) SHALL behave on Windows as on Linux, including clipboard copy and paste through the Windows clipboard (no primary selection). Local terminal tabs SHALL run on Windows through the pseudo-console (ConPTY, Windows 10 1809 or later) as the local-shell capability specifies (change `add-local-shell`); where ConPTY is missing, no command or menu SHALL claim to start one and "+" SHALL open New session. The New session editor SHALL NOT list a local shell type.

#### Scenario: Copy and paste on Windows
- **WHEN** the user selects text in an SSH tab and pastes it into Notepad
- **THEN** the text matches the selection

#### Scenario: Local shell not in the session editor
- **WHEN** the user opens the New session editor on Windows
- **THEN** no local shell type is listed

#### Scenario: Local terminal through ConPTY
- **WHEN** the user clicks "+" on Windows 10 1809 or later
- **THEN** a local PowerShell tab opens in `%USERPROFILE%` without a console window
