## MODIFIED Requirements

### Requirement: Fonts and appearance
The user SHALL choose the terminal font family, size, cursor shape and blink per session or globally. The default terminal font SHALL be the bundled Cascadia Mono at 13 logical pixels, available on every host without installed fonts; a family that fails to load SHALL fall back to the bundled default with a status message. Font zoom SHALL change the size by one point per step and persist across tabs and restarts.

#### Scenario: Global defaults
- **WHEN** the user changes the global cursor shape to underline
- **THEN** every tab without a per-session override uses an underline cursor

#### Scenario: Per-session font
- **WHEN** a session overrides font size to 14
- **THEN** only tabs of that session use size 14

#### Scenario: Zoom with mouse and keyboard
- **WHEN** the pointer is over the terminal and the user presses Ctrl+wheel up, or the terminal has focus and the user presses Ctrl+= or Ctrl++
- **THEN** the font grows by exactly one point (13.5 becomes 14.5), up to 48 points; Ctrl+wheel down or Ctrl+- shrinks it by one point, down to 6 points; Ctrl+0 resets it to the default size

#### Scenario: Zoom persists
- **WHEN** the user zooms a tab whose session has no font size override and restarts the application
- **THEN** every terminal tab opens with the zoomed size, and a session with its own font size keeps its own zoomed size instead

#### Scenario: Zoom applies to sibling tabs
- **WHEN** two tabs of a session with its own font size are open and the user zooms one of them
- **THEN** both tabs use the new size, and zooming the other tab steps from the new size

#### Scenario: Control bytes kept
- **WHEN** the user presses Ctrl+Shift+- (Ctrl+_) or Ctrl+/ in the terminal
- **THEN** byte 0x1f is sent to the session and the font size does not change

#### Scenario: Default font without system fonts
- **WHEN** a terminal tab opens on a host with no font packages
- **THEN** it renders with the bundled Cascadia Mono at 13 px and box-drawing characters join without gaps

## ADDED Requirements

### Requirement: Edge-to-edge terminal area
A terminal tab SHALL fill the work area from the tab strip's bottom edge to the status bar's top edge and from the sidebar's right edge to the window's right edge, with no margin, frame or rounded panel; the terminal background color SHALL cover that whole rectangle, and the cell grid SHALL start at most 2 logical pixels from its top-left corner. Leftover space smaller than one cell SHALL be painted with the terminal background.

#### Scenario: No inset
- **WHEN** a terminal tab is rendered at 1280×800 in either theme
- **THEN** pixels 1 px inside each edge of the work area have the terminal scheme's background color
