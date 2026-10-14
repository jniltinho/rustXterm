## MODIFIED Requirements

### Requirement: Main window layout
The main window SHALL be laid out top to bottom as: a menu bar 20 ± 1 px high, a toolbar 52 ± 2 px high (34 ± 2 px when captions are off), a row 26 ± 1 px high holding the quick-connect field (vertically centered) over the sidebar column and the tab strip over the work area with the tabs bottom-aligned, the body (sidebar column and work area) and a status bar 22 ± 1 px high, all in logical pixels at scale 1.0. The sidebar column SHALL default to 240 px wide and consist of a vertical icon strip 34 ± 1 px wide on its left edge and the active sidebar page on the right. The sidebar SHALL be collapsible and resizable (minimum 160 px, maximum half the window width), and its width, visibility and active page SHALL persist across restarts. When the sidebar is collapsed, the quick-connect field SHALL move to the left end of the tab-strip row.

#### Scenario: First launch
- **WHEN** the user starts the application with no configuration
- **THEN** the window shows the menu bar, the toolbar, the quick-connect field above an empty Sessions page with its "User sessions" root row, the Home tab and the status bar, and no error dialogs

#### Scenario: Layout persists
- **WHEN** the user resizes the sidebar, switches it to the SFTP page, closes the application and reopens it
- **THEN** the sidebar width, visibility and active page are restored

#### Scenario: Band heights
- **WHEN** the empty main window is rendered at 1280×800 and scale 1.0 in either theme
- **THEN** the menu bar, toolbar, quick-connect/tab row and status bar heights match the values above within their tolerances and the sidebar column is 240 ± 1 px wide

### Requirement: Tabs
Each opened session SHALL open in its own tab showing the session name, a 16 px protocol icon and a connection state (connecting, connected, disconnected, error). The tab strip SHALL begin with a pinned, icon-only Home tab (house icon, 36 ± 2 px wide) that cannot be closed or moved, and end with a "+" button that runs the new-session command. Session tabs SHALL be 24 ± 1 px high, between 80 and 220 px wide with an ellipsized title, square, with the close button (16 px) inside the tab's right edge. Settings SHALL offer "Show tab numbers" (default on, prefixing titles with `N. ` where N counts session tabs from 1 in strip order) and "Show close buttons" (default on). The active tab SHALL use tab_active and join the content area without a bottom border; inactive tabs use tab_inactive with a 1 px border. Tabs SHALL support reorder by drag, close, close others, rename and duplicate. Closing a tab with a live connection SHALL ask for confirmation unless disabled in Settings.

#### Scenario: Tab operations
- **WHEN** the user drags a tab, renames it, duplicates it and uses "close others"
- **THEN** order, title and set of tabs reflect each action, tab numbers follow the new order and a duplicate opens a new connection with the same session settings

#### Scenario: Close live tab
- **WHEN** the user closes a connected tab with confirmation enabled
- **THEN** a confirmation is shown and the connection stays open if the user cancels

#### Scenario: Disconnected tab
- **WHEN** the remote side closes the SSH connection
- **THEN** the tab shows a disconnected state and offers "Reconnect" without closing the tab

#### Scenario: Home tab and new-tab button
- **WHEN** two session tabs are open
- **THEN** the strip shows the house-only Home tab first, then `1. <name>` and `2. <name>` each with its close button inside, then the "+" button, and clicking "+" opens the session editor

#### Scenario: Close buttons hidden
- **WHEN** "Show close buttons" is off
- **THEN** no tab draws a close button and middle click or the close-tab shortcut still closes the tab

### Requirement: Review screenshots and icons
The project SHALL capture screenshots of every rustXterm screen and dialog in light and dark themes at a 1280×800 logical window and scale 1.0, and every screen again at scale 2.0, into `docs/prints/rustxterm/` (2× files suffixed `@2x`). `docs/prints/` SHALL be git-ignored and SHALL never enter build contexts or artifacts. Committed kittest snapshots SHALL cover every screen at 1280×800 and scale 1.0 in both themes; 2× images SHALL exist only as prints. The UI SHALL use the bundled original icon set in `assets/icons/app/` for session types, toolbar and tabs, and SHALL load replacement icons from a directory given by `RXT_ICON_OVERRIDE_DIR` when set.

#### Scenario: Capture
- **WHEN** the developer runs `cargo xtask prints`
- **THEN** PNGs for each screen in both themes at 1280×800 and at 2× appear under `docs/prints/rustxterm/` and `git status` shows none of them

#### Scenario: Icon override
- **WHEN** `RXT_ICON_OVERRIDE_DIR` points to a directory with an icon named like a built-in one
- **THEN** that icon is used instead of the bundled one, and without the variable the bundled icons are used

## ADDED Requirements

### Requirement: Menu bar
The main window SHALL show a menu bar with the menus Terminal, Sessions, View, Tools, Settings and Help, in that order. Every menu item SHALL run a command id, so the same action is reachable from shortcuts and agents, and SHALL show its configured shortcut right-aligned. Items whose command is unavailable SHALL be disabled, not hidden. The menus SHALL contain at least:
- Terminal: Find, Copy, Paste, Save terminal text, Close tab, Exit.
- Sessions: New session, Quick connect, Duplicate tab, Reconnect, Import sessions, Export sessions.
- View: Show sidebar, Fullscreen, Next tab, Previous tab, Theme (Light, Dark, System as radio items).
- Tools: Tunneling, MultiExec.
- Settings: Configuration, Change master password.
- Help: Help, About.

#### Scenario: Menu item runs command
- **WHEN** the user opens View › Theme and picks Dark
- **THEN** the theme switches to Dark exactly as from the Settings window and the Dark radio item is shown checked next time

#### Scenario: Disabled item
- **WHEN** no tab is open and the user opens the Terminal menu
- **THEN** Find, Copy, Paste, Save terminal text and Close tab are disabled and Exit is enabled

#### Scenario: Keyboard access without stealing terminal keys
- **WHEN** a terminal is focused and the user presses F10 or Alt+T
- **THEN** the terminal receives the key sequence and no menu opens, while the configurable "focus menu bar" shortcut (default Ctrl+Shift+F10) opens the Terminal menu

### Requirement: Toolbar
The toolbar SHALL show, left to right: Session, Sessions, Tunneling, MultiExec, Settings, Help, then flexible space, then Exit pinned to the right edge. Each button SHALL be a 24 px icon with an 11 px caption below it (or icon only when Settings › Display › "Toolbar captions" is off), at least 56 px wide with 4 px between buttons, without a band color different from the toolbar token. Toggle buttons (Sessions sidebar, MultiExec) SHALL show their on state with the pressed fill.

#### Scenario: Exit pinned right
- **WHEN** the window is resized from 1280 to 1600 px wide
- **THEN** the Exit button's right edge stays 4 ± 1 px from the window's right edge and the other buttons do not move

#### Scenario: Captions off
- **WHEN** the user turns "Toolbar captions" off
- **THEN** buttons show icons only, each has a tooltip with its name, and the toolbar shrinks to 34 ± 2 px

### Requirement: Quick connect placement
The quick-connect field SHALL span the sidebar column width (minus 2 px margins) in the row above the sidebar, with no separate label and a muted placeholder "Quick connect...". Focusing it with the quick-connect command SHALL select its text. Parse errors SHALL appear as a tooltip-style message under the field and a danger-colored 1 px border, without opening a dialog.

#### Scenario: Placement
- **WHEN** the main window is rendered with the sidebar visible
- **THEN** the quick-connect field's left edge is within 2 px of the window's left edge and its bottom edge aligns with the tab strip's bottom edge within 1 px

### Requirement: Sidebar icon strip
The left edge of the sidebar column SHALL be a vertical strip of icon-only buttons (24 px icons, 34×40 px buttons): Sessions (star) and SFTP, top-aligned. The active button SHALL share the sidebar page's background with no border on its right side; inactive buttons use panel with a 1 px border. Each button SHALL have a tooltip and an accessible name. Clicking the active button SHALL collapse the sidebar; clicking an inactive one SHALL switch the page.

#### Scenario: Switch to SFTP
- **WHEN** the user clicks the SFTP strip button
- **THEN** the sidebar page shows the SFTP panel and the SFTP button becomes active

#### Scenario: Collapse from strip
- **WHEN** the Sessions page is active and the user clicks the Sessions strip button
- **THEN** the sidebar collapses to the strip and the quick-connect field moves to the tab-strip row

### Requirement: Home page
The Home tab SHALL show, centered horizontally in the work area: the rustXterm logo (64 px) beside the title "rustXterm" in the display style; below it two flat rectangular buttons 200 ± 4 px wide and 30 ± 1 px high, each with a 16 px icon and a label, "New session" and either "Start local terminal" (when local shell sessions are available) or "Import sessions" (otherwise); and a gear button (24 px icon) at the top-right corner of the work area that opens Settings. On first run, a theme chooser panel (panel fill, about 540×176 px) SHALL appear under the buttons with Light and Dark thumbnails drawn from the theme tokens (no image assets) and captions; choosing one SHALL apply it, save it and hide the panel. When recent sessions exist, up to 8 SHALL be listed under the buttons; when none exist, no recent-sessions heading SHALL be shown.

#### Scenario: First run chooser
- **WHEN** the application starts with no settings file and the user clicks the Dark thumbnail
- **THEN** the UI switches to Dark, the selection is saved and the chooser does not appear on the next start

#### Scenario: No recent sessions
- **WHEN** the Home tab is shown and no session was ever opened
- **THEN** only the logo, title, two buttons and gear are visible (plus the chooser on first run)

### Requirement: Settings window
Settings SHALL open as a modal window of 800×560 ± 8 px with icon tabs (24 px icons with labels) General, Terminal, SSH, Display, Shortcuts, Security and Agent control, each page made of titled group boxes, and a single centered OK button with an icon at the bottom. Changes SHALL apply immediately; OK, Escape and the close button SHALL close the window. The Display page SHALL contain: Skin (dropdown Light, Dark, System), Interface size (Auto, 100 %, 125 %, 150 %, 200 %), Toolbar captions (checkbox), Show tab numbers and Show close buttons (checkboxes). The last opened tab SHALL be remembered for the session of the application.

#### Scenario: Skin dropdown
- **WHEN** the user opens Settings › Display and chooses Skin = System
- **THEN** the theme follows the desktop preference immediately and stays System after restart

#### Scenario: Escape closes
- **WHEN** the user changes the terminal font size and presses Escape
- **THEN** the window closes and the change stays applied

### Requirement: Tunneling window
The tunneling window SHALL be a resizable window (default 1060×410 px, minimum 760×300) with a centered one-line description, a bordered grid with header row (34 px) and columns Name, Type, Start/stop, Forward port, Destination server, SSH server, Settings, Order, and a bottom row with "New SSH tunnel", "Start all tunnels", "Stop all tunnels" on the left and "Close" on the right, each button with an icon. Start/stop SHALL be a single icon button per row showing play when stopped and stop when running; Settings SHALL be edit and delete icon buttons; Order SHALL be up/down icon buttons. The tunnel editor SHALL put Local, Remote and Dynamic radio buttons at the top, then the fields grouped by side (forwarded port, SSH server, destination), then Save and Cancel centered.

#### Scenario: Start all
- **WHEN** three stopped tunnels exist and the user clicks "Start all tunnels"
- **THEN** each row's button turns to stop as its tunnel starts, and errors appear in that row without stopping the others

#### Scenario: Empty grid
- **WHEN** no tunnel exists
- **THEN** the grid shows its header and a centered muted hint to create one, and "Start all" and "Stop all" are disabled

### Requirement: Status bar
The status bar SHALL be a single line 22 ± 1 px high in the caption style: the latest status message left-aligned (bold prefix allowed, at most one line, ellipsized) and the tab count right-aligned. Status messages SHALL never resize the bar.

#### Scenario: Long message
- **WHEN** a status message longer than the window width is posted
- **THEN** it is ellipsized on one line and the bar height stays 20 ± 1 px

### Requirement: Keyboard navigation
Every screen SHALL be fully usable without a pointer: Tab and Shift+Tab SHALL move focus in reading order (top to bottom, left to right) through all enabled controls; Enter SHALL activate the default button and Escape SHALL cancel or close dialogs; arrow keys SHALL move within lists, grids, trees, radio groups and tab rows; the Menu key and Shift+F10 SHALL open the focused element's context menu; F2 SHALL rename and Delete SHALL delete (with confirmation) in the session tree and file panes. Focus SHALL return to the element that opened a dialog when it closes. Inside a focused terminal or remote-desktop view, Tab, arrows and function keys SHALL go to the remote side; only configured application shortcuts and the release-key combination leave the view.

#### Scenario: Editor by keyboard
- **WHEN** the user opens the session editor with the new-session shortcut, presses Right to select SFTP, Tab to the Remote host field, types a host and presses Enter
- **THEN** the SFTP session is saved and focus returns to the session tree on the new session

#### Scenario: Terminal keeps Tab
- **WHEN** a terminal is focused and the user presses Tab
- **THEN** the terminal receives `0x09` and focus does not move

### Requirement: Accessible names and tooltips
Every interactive widget SHALL expose a non-empty accessible name and role through AccessKit; icon-only controls SHALL use the same text as their tooltip. Every icon-only control SHALL show a tooltip after 500 ± 100 ms of hover, containing its name and, when one is configured, its shortcut. Tooltips SHALL use the caption style on panel with a 1 px border and square corners.

#### Scenario: Name audit
- **WHEN** the accessibility test walks the AccessKit tree of every snapshot screen
- **THEN** no button, field, checkbox, combo, tab, tree item or menu item has an empty name

#### Scenario: Tooltip with shortcut
- **WHEN** the pointer rests on the Settings toolbar button for 600 ms
- **THEN** a tooltip shows its name and its configured shortcut

### Requirement: Context menus
Context menus SHALL be available by right click and by keyboard on: session tree rows (Connect, Edit, Duplicate, Rename, Delete, New session here, New folder here), folder rows and the empty tree area (New session, New folder, Import sessions), tabs (Rename, Duplicate, Reconnect, Close, Close others), terminals (Copy, Paste, Find, Save terminal text) and file pane rows. Items SHALL show icons (16 px) where a catalog icon exists, shortcuts right-aligned, separators between groups and disabled (not hidden) unavailable items.

#### Scenario: Tree context menu by keyboard
- **WHEN** a session row is focused and the user presses Shift+F10 then selects Duplicate
- **THEN** a copy of the session appears in the same folder and is selected

### Requirement: Drag affordances
While dragging, the UI SHALL show what will happen on drop: a 2 px accent insertion line between tabs or between tree rows, an accent 1 px outline on a target folder or file pane, the dragged item's name next to the pointer and a "not allowed" pointer over invalid targets. Escape SHALL cancel the drag with no change.

#### Scenario: Reorder tab
- **WHEN** the user drags the third tab over the first
- **THEN** a 2 px accent line appears at the drop position and on release the tab moves there

#### Scenario: Cancel drag
- **WHEN** the user presses Escape while dragging a session onto a folder
- **THEN** the session stays in its original folder

### Requirement: Empty, loading and error states
Every screen SHALL define an empty, a loading (when it waits for I/O) and an error presentation: a centered muted message, optionally with a 32 px icon and one action button. Loading SHALL show a spinner and a label after 200 ms of waiting (no flash for faster operations). Errors SHALL name the failed operation and the cause in one or two sentences and offer a retry or the next sensible action; they SHALL never be raw debug output.

#### Scenario: Connecting
- **WHEN** an SSH tab takes longer than 200 ms to connect
- **THEN** the tab content shows a spinner with "Connecting to <host>..." centered in the content area

#### Scenario: Connection error
- **WHEN** the server refuses the TCP connection
- **THEN** the tab content shows the error message and a Reconnect button, and the tab shows the error state

### Requirement: Dialog convention
All dialogs SHALL be modal, centered on the main window, with a 24 px title row on panel, square corners, a 1 px border, content in titled group boxes when there is more than one group, and a bottom button row with OK (green check icon) and Cancel (red cross icon) centered, 80×30 px each, 24 px apart; single-action dialogs SHALL show only a centered OK. The default button SHALL respond to Enter and Cancel to Escape. Destructive confirmations SHALL name the object and default to Cancel.

#### Scenario: Delete confirmation
- **WHEN** the user deletes the session `web1`
- **THEN** a dialog names `web1`, focus starts on Cancel, and pressing Enter cancels the delete

### Requirement: Window state persistence
The application SHALL persist the main window's size, maximized and fullscreen state (and position where the display server allows it) and restore them at start, clamped so at least 200×100 px of the window is on a connected monitor. The minimum window size SHALL be 800×500 logical pixels.

#### Scenario: Restore maximized
- **WHEN** the user maximizes the window, quits and restarts
- **THEN** the window opens maximized

#### Scenario: Monitor removed
- **WHEN** the saved position lies on a monitor that is no longer connected
- **THEN** the window opens fully on the primary monitor

### Requirement: Visual review gate
Before this UI is merged, every screen SHALL have an entry in a local review checklist (`docs/prints/review-checklist.md`, git-ignored) listing its rustXterm prints (light, dark, 2×), and each entry SHALL be marked reviewed by a human. The checklist SHALL never be required by CI.

#### Scenario: Checklist generated
- **WHEN** the developer runs `cargo xtask prints`
- **THEN** `docs/prints/review-checklist.md` lists every print with an unchecked review box, preserving boxes already checked

#### Scenario: CI without prints
- **WHEN** CI runs the test suite with no `docs/prints/` directory
- **THEN** all tests pass
