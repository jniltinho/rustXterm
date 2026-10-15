## MODIFIED Requirements

### Requirement: Tabs
Each opened session SHALL open in its own tab showing the session name, a protocol icon and a connection state (connecting, connected, disconnected, error). Tabs SHALL support reorder by drag, close, close others, rename and duplicate. Closing a tab with a live connection SHALL ask for confirmation unless disabled in Settings. The "+" button after the last tab SHALL open a local terminal where a local shell backend exists (tooltip "New local terminal") and the New session editor otherwise.

#### Scenario: Tab operations
- **WHEN** the user drags a tab, renames it, duplicates it and uses "close others"
- **THEN** order, title and set of tabs reflect each action and a duplicate opens a new connection with the same session settings

#### Scenario: Close live tab
- **WHEN** the user closes a connected tab with confirmation enabled
- **THEN** a confirmation is shown and the connection stays open if the user cancels

#### Scenario: Disconnected tab
- **WHEN** the remote side closes the SSH connection
- **THEN** the tab shows a disconnected state and offers "Reconnect" without closing the tab

#### Scenario: Plus button
- **WHEN** the user clicks "+" on Linux or Windows with ConPTY
- **THEN** a local terminal tab opens and becomes active
