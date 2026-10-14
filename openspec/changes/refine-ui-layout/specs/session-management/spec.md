## MODIFIED Requirements

### Requirement: Session tree
The Sessions sidebar page SHALL show sessions in nested folders under a single root row "User sessions" (users icon), with 16 px icons per type, 20 ± 1 px rows, 16 px indentation per level and expand arrows on folders. The selected row SHALL be filled with the selection token and drawn in selection_text across the row's label width; hover SHALL use the hover token. A compact bottom row (24 px) SHALL hold icon-only New session and New folder buttons and a filter field; there SHALL be no action or filter row above the tree. The tree SHALL support create, edit, duplicate, delete (with confirmation), move by drag-and-drop, rename, filter by text, and open by double click or Enter.

#### Scenario: Filter
- **WHEN** the user types `prod` in the filter
- **THEN** only sessions or folders whose name or host contains `prod` remain visible, with parent folders expanded

#### Scenario: Root row and selection
- **WHEN** the Sessions page is rendered with one session selected in Light
- **THEN** the first row is "User sessions", and the selected row's label background is `#0078d4` with white text (in Dark, `#4a4a4a` with white text)

#### Scenario: Empty tree
- **WHEN** no session exists
- **THEN** the tree shows only the root row and a muted hint under it to create a session or import sessions

#### Scenario: Locked store
- **WHEN** the encrypted session store is locked
- **THEN** the tree shows the root row with a lock icon and an Unlock button instead of sessions

### Requirement: Session editor
The session editor SHALL be a fixed 900×600 ± 8 px dialog titled "Session settings" with, top to bottom: a row of session-type tiles (32 px icon above a 12 px label, tiles 56 ± 4 px wide, the chosen tile filled with pressed in Light or selection in Dark and a 1 px border); a "Basic <type> settings" group with the remote host (marked required), username (with a saved-credentials picker button) and port (spin box) on one row; a row of icon tabs (16 px icon and label each) Advanced <type> settings, Terminal settings, Network settings, Bookmark settings and Agent settings, as applicable to the type; the tab panel laid out as a two-column grid with a 48 px protocol icon in a 64×64 tile at its right; and OK and Cancel centered at the bottom. Session name, folder and icon SHALL be edited in the Bookmark tab. The editor SHALL validate fields before saving. When opened from the toolbar or menu without a type, the tiles SHALL show none selected and the area below SHALL show only a centered muted prompt to choose a session type.

#### Scenario: Invalid port
- **WHEN** the user enters port 70000
- **THEN** the field shows an error with a danger-colored border and OK stays disabled

#### Scenario: One-row basic settings
- **WHEN** the SSH editor is rendered at 1280×800 and scale 1.0
- **THEN** the Remote host, Username and Port fields share one row whose vertical centers differ by at most 1 px, and the OK and Cancel buttons are centered within ±2 px of the dialog's center line

#### Scenario: Choose a type first
- **WHEN** the user opens the editor with the toolbar Session button
- **THEN** no tile is selected, only the prompt is shown, and selecting SSH reveals the SSH groups with port 22

#### Scenario: Bookmark tab
- **WHEN** the user opens the Bookmark settings tab of an existing session
- **THEN** the session name, folder and icon fields are there and nowhere else in the editor
