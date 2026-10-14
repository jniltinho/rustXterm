## ADDED Requirements

### Requirement: Desktop notifications for background events
While the main window is not focused, the application SHALL raise a desktop notification when a file transfer finishes or fails and when a session disconnects without the user asking, naming the tab and the event. A setting SHALL turn these notifications off. Notifications SHALL never include passwords, keys or terminal output, and an unavailable or disabled notification service SHALL never show an error or block the application.

#### Scenario: Download finished in the background
- **WHEN** a 100 MB download finishes while another application has focus
- **THEN** a notification names the file and the tab, and the transfer list shows it as done

#### Scenario: Notifications disabled
- **WHEN** notifications are turned off in rustXterm or in the operating system and a session drops in the background
- **THEN** no notification and no error appear, and the tab shows the disconnected state

### Requirement: Dialog start folder
Save and open dialogs SHALL start in the folder last used for the same kind of action, or in the user's Downloads folder (home folder if it does not exist), never in the process working directory.

#### Scenario: First save
- **WHEN** the user saves terminal output for the first time after starting rustXterm from a shortcut
- **THEN** the save dialog opens in the Downloads folder

### Requirement: Windows serial ports in quick connect
On Windows, quick connect SHALL accept `serial://COM<n>` (and `serial://\\.\COM<n>`) and open that COM port, and the serial port field SHALL show a Windows example (`COM3`) as its placeholder.

#### Scenario: Quick connect to COM1
- **WHEN** the user types `serial://COM1?baud=9600` in quick connect on Windows
- **THEN** a Serial tab opens on COM1 at 9600 baud
