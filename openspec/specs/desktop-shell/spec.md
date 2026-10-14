# desktop-shell Specification

## Purpose
Native Linux main window with a session sidebar, tabbed work area, quick connect and themes.

## Requirements

### Requirement: Main window layout
The application SHALL show a left sidebar with Sessions and SFTP tabs, a tabbed work area, a quick-connect field and a status bar. The sidebar SHALL be collapsible and resizable, and its state SHALL persist across restarts.

#### Scenario: First launch
- **WHEN** the user starts the application with no configuration
- **THEN** the window shows the sidebar with an empty Sessions tree, a Home tab with "New session", "Import" and recent-sessions actions, and no error dialogs

#### Scenario: Layout persists
- **WHEN** the user resizes the sidebar, closes the application and reopens it
- **THEN** the sidebar width and visibility are restored

### Requirement: Light, dark and system themes
The application SHALL offer Light, Dark and System themes. System SHALL follow the desktop color-scheme preference (XDG portal `org.freedesktop.appearance color-scheme`) and update live when it changes. Terminal color schemes SHALL be selectable independently of the UI theme, with a default terminal scheme matching each UI theme.

#### Scenario: Switch theme
- **WHEN** the user selects Dark in Settings
- **THEN** all panels, dialogs and the default terminal scheme switch to dark immediately without restart

#### Scenario: Follow system
- **WHEN** the theme is System and the desktop switches from light to dark
- **THEN** the application switches to dark within 2 seconds

#### Scenario: Portal unavailable
- **WHEN** the theme is System and no appearance portal is reachable
- **THEN** the application uses Light and shows no error

### Requirement: Tabs
Each opened session SHALL open in its own tab showing the session name, a protocol icon and a connection state (connecting, connected, disconnected, error). Tabs SHALL support reorder by drag, close, close others, rename and duplicate. Closing a tab with a live connection SHALL ask for confirmation unless disabled in Settings.

#### Scenario: Tab operations
- **WHEN** the user drags a tab, renames it, duplicates it and uses "close others"
- **THEN** order, title and set of tabs reflect each action and a duplicate opens a new connection with the same session settings

#### Scenario: Close live tab
- **WHEN** the user closes a connected tab with confirmation enabled
- **THEN** a confirmation is shown and the connection stays open if the user cancels

#### Scenario: Disconnected tab
- **WHEN** the remote side closes the SSH connection
- **THEN** the tab shows a disconnected state and offers "Reconnect" without closing the tab

### Requirement: Quick connect
The quick-connect field SHALL accept `[user@]host[:port]` (SSH by default) and `protocol://[user@]host[:port]` for ssh, sftp, ftp, ftps, rdp and vnc, and open a session without saving it.

#### Scenario: Quick SSH
- **WHEN** the user types `alice@lab.example:2222` and presses Enter
- **THEN** an SSH tab opens to lab.example port 2222 as alice

#### Scenario: Protocol prefixes
- **WHEN** the user enters each of `sftp://u@h`, `ftp://u@h`, `ftps://h:990`, `rdp://h:3390`, `vnc://h`
- **THEN** each opens the matching session type with the given user and port and default ports otherwise

#### Scenario: Invalid input
- **WHEN** the user types `ssh://` with no host
- **THEN** the field shows an inline error and no tab opens

### Requirement: MultiExec
The application SHALL offer a MultiExec mode where keystrokes typed in the focused terminal are sent to every terminal tab the user selected. Tabs in MultiExec SHALL be visually marked and the mode SHALL be toggleable with one action.

#### Scenario: Marking and toggle
- **WHEN** the user toggles MultiExec on and then off with its action
- **THEN** selected tabs show a marker while on, and no input is broadcast after it is off

#### Scenario: Broadcast command
- **WHEN** MultiExec is on with three SSH tabs selected and the user types `uptime` and Enter
- **THEN** all three tabs receive `uptime\r`

### Requirement: Keyboard shortcuts and accessibility
Main actions (new session, close tab, next/previous tab, find, settings, toggle sidebar, toggle fullscreen) SHALL have configurable shortcuts that do not collide with terminal control sequences by default. Widgets SHALL expose accessibility names through AccessKit.

#### Scenario: Shortcut does not steal terminal input
- **WHEN** a terminal is focused and the user presses Ctrl+C
- **THEN** the terminal receives `0x03` and no application action fires

### Requirement: Review screenshots and icons
The project SHALL capture screenshots of every rustXterm screen and dialog in light and dark themes into `docs/prints/` for visual review. `docs/prints/` SHALL be git-ignored. The UI SHALL use the bundled original icon set in `assets/icons/app/` for session types, toolbar and tabs, and SHALL load replacement icons from a directory given by `RXT_ICON_OVERRIDE_DIR` when set.

#### Scenario: Capture
- **WHEN** the developer runs the screenshot task
- **THEN** PNGs for each screen in both themes appear under `docs/prints/` and `git status` shows none of them

#### Scenario: Icon override
- **WHEN** `RXT_ICON_OVERRIDE_DIR` points to a directory with an icon named like a built-in one
- **THEN** that icon is used instead of the bundled one, and without the variable the bundled icons are used

### Requirement: Never crash
Invalid configuration, malformed import files, network errors and protocol errors SHALL produce actionable messages and SHALL NOT terminate the application or close other tabs.

#### Scenario: Corrupt settings file
- **WHEN** the settings file contains invalid TOML
- **THEN** the application starts with defaults, keeps a backup of the corrupt file and shows a notice naming the file
