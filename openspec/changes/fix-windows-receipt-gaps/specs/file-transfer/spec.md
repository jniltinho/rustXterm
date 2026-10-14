## ADDED Requirements

### Requirement: Open transferred and local files
A finished download in the transfer list SHALL offer "Open" (default application) and "Show in folder"; the local pane SHALL offer "Open" for files. Opening SHALL go through the platform opener, and a failure (no associated application, file moved or deleted) SHALL be reported to the user.

#### Scenario: Open a downloaded text file
- **WHEN** the user downloads `notes.txt` and chooses Open on its transfer row
- **THEN** the file opens in the system's default text editor

#### Scenario: File removed before opening
- **WHEN** the downloaded file was deleted and the user chooses Open
- **THEN** an error names the missing file and nothing else happens

### Requirement: Per-tab transfer lists
Each tab SHALL show only the transfers it started; a transfer started in one tab SHALL NOT appear in another tab's transfer list.

#### Scenario: FTP upload and SSH panel
- **WHEN** an upload runs in an FTP tab while an SSH tab's SFTP panel is visible
- **THEN** the upload appears only in the FTP tab's transfer list
