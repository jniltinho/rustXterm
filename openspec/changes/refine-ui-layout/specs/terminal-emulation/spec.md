## MODIFIED Requirements

### Requirement: Fonts and appearance
The user SHALL choose the terminal font family, size, cursor shape and blink, and color scheme per session or globally. The default terminal font SHALL be the bundled JetBrains Mono at 13 logical pixels, available on every host without installed fonts; other families SHALL be selectable from fonts the user adds, and a family that fails to load SHALL fall back to the bundled default with a status message.

#### Scenario: Global defaults
- **WHEN** the user changes the global cursor shape to underline
- **THEN** every tab without a per-session override uses an underline cursor

#### Scenario: Per-session font
- **WHEN** a session overrides font size to 14
- **THEN** only tabs of that session use size 14

#### Scenario: Default font without system fonts
- **WHEN** a terminal tab opens on a host with no font packages
- **THEN** it renders with the bundled JetBrains Mono at 13 px and box-drawing characters join without gaps

## ADDED Requirements

### Requirement: Edge-to-edge terminal area
A terminal tab SHALL fill the work area from the tab strip's bottom edge to the status bar's top edge and from the sidebar's right edge to the window's right edge, with no margin, frame or rounded panel; the terminal background color SHALL cover that whole rectangle, and the cell grid SHALL start at most 2 logical pixels from its top-left corner. Leftover space smaller than one cell SHALL be painted with the terminal background.

#### Scenario: No inset
- **WHEN** a terminal tab is rendered at 1280×800 in either theme
- **THEN** pixels 1 px inside each edge of the work area have the terminal scheme's background color
