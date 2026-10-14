## ADDED Requirements

### Requirement: Windows desktop integration
On Windows, System theme mode SHALL follow the Windows "app mode" (light/dark) setting and update live when it changes; file pickers SHALL be the native Windows dialogs, modal to the main window; notifications SHALL appear as Windows toast notifications when available and SHALL never block or crash the application when unavailable; the window and taskbar SHALL show the rustXterm icon. Light, dark and system themes SHALL be complete on Windows as on Linux.

#### Scenario: Theme follows Windows
- **WHEN** System theme mode is selected and the user switches Windows from light to dark app mode
- **THEN** rustXterm switches to the dark theme without a restart

#### Scenario: Modal file picker
- **WHEN** the user chooses Upload in an SFTP pane
- **THEN** the Windows file dialog opens in front of and owned by the rustXterm window

#### Scenario: Notifications unavailable
- **WHEN** toast notifications are disabled in Windows settings and a transfer finishes
- **THEN** no error dialog appears and the transfer list still shows completion
