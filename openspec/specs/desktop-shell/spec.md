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

### Requirement: Remote monitoring bar
SSH terminal tabs with monitoring active SHALL show a bar 26 ± 1 px high along the bottom of the tab area, below the terminal and above the window status bar, without overlapping the terminal grid. Left to right it SHALL hold chips with catalog icons: host, CPU percent with a sparkline, memory used/total, upload, download, uptime, then one chip per disk. CPU, memory and disk values SHALL use the theme's warning color at 80 % or more and its danger color at 95 % or more. Values with no reply for two intervals SHALL render muted. When the chips do not fit, left/right arrow buttons SHALL scroll them. A close (×) button SHALL always be visible at the right end and SHALL turn monitoring off for that tab. Every chip and button SHALL have an accessible name and a tooltip with details. The bar SHALL be complete in light, dark and system themes.

#### Scenario: Bar appears
- **WHEN** an SSH tab connects to a Linux server with remote monitoring on
- **THEN** within 5 s the bar shows the host, CPU, memory, throughput, uptime and root disk chips

#### Scenario: Close button
- **WHEN** the user clicks the bar's × button
- **THEN** the bar disappears from that tab only, the terminal grows by the bar's height, and the monitoring channel closes

#### Scenario: Overflow
- **WHEN** the window is too narrow for all chips
- **THEN** arrow buttons appear, scroll the chips one at a time, and the × stays visible

#### Scenario: Dark theme thresholds
- **WHEN** CPU usage is 97 % in the dark theme
- **THEN** the CPU value uses the dark theme's danger token

### Requirement: Remote monitoring settings
Settings › SSH SHALL contain a "Remote monitoring" group with "Show remote monitoring bar on SSH tabs" (default on) and "Refresh interval" (1–30 s, default 2). The SSH session editor's advanced options SHALL offer "Remote monitoring": Use global setting (default), On, Off. The effective state SHALL be decided when a tab connects: the session choice overrides the global one. The interval SHALL apply to running tabs immediately.

#### Scenario: Session opt-out
- **WHEN** the global setting is on and a session sets Remote monitoring to Off
- **THEN** that session's tabs connect without the bar or a monitoring channel

#### Scenario: Interval change
- **WHEN** the user changes the refresh interval from 2 s to 10 s
- **THEN** visible monitored tabs refresh every 10 s from then on without reconnecting

### Requirement: SFTP panel monitoring button
The "Remote monitoring" button at the bottom of the SFTP side panel SHALL toggle monitoring for the attached SSH tab. It SHALL appear pressed while monitoring is active, and it SHALL be disabled with a tooltip giving the reason when monitoring is unsupported or unavailable on that connection. The toggle SHALL not change any saved setting.

#### Scenario: Turn on from the panel
- **WHEN** the bar of the attached tab was closed and the user clicks "Remote monitoring"
- **THEN** the bar reappears with fresh values and the button shows as pressed

#### Scenario: Toggle is per tab
- **WHEN** two SSH tabs are open and the user turns monitoring off in one
- **THEN** the other tab keeps its bar
