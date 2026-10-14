## ADDED Requirements

### Requirement: File browser presentation
The SFTP side panel and both panes of the dual-pane file tab SHALL use the same presentation: a path row with an editable path field, a row of 16 px icon-only buttons with tooltips (up, refresh, home, new folder, upload, download, hidden-files toggle), a list with a 22 px header row and 20 ± 1 px item rows separated by border_subtle lines, 16 px file-type icons, sortable columns (name, size, modified, permissions) with a sort arrow on the active column, and selection drawn like the session tree. The transfers list SHALL use one 20 px row per transfer with name, direction icon, a 1 px-bordered progress bar and a cancel icon button.

#### Scenario: Sort by size
- **WHEN** the user clicks the Size header twice
- **THEN** rows are sorted by size descending and the header shows a descending arrow

#### Scenario: Empty directory
- **WHEN** the current remote directory has no entries
- **THEN** the list shows the header and a centered muted "This folder is empty" message

#### Scenario: Listing error
- **WHEN** listing the directory fails with permission denied
- **THEN** the list area shows the path and the error in one sentence with a Retry button, and the previous path stays reachable with Up

#### Scenario: Drop target
- **WHEN** the user drags a local file over the remote pane
- **THEN** the remote pane shows a 1 px accent outline and releasing starts an upload into the shown directory
