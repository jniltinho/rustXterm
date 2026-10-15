## MODIFIED Requirements

### Requirement: Fonts and appearance
The user SHALL choose the terminal font family, size, cursor shape and blink, and color scheme per session or globally. The global color scheme SHALL be "Follow UI theme" (default), a built-in scheme or a user scheme; a session SHALL either inherit the global scheme or override it with "Follow UI theme", a built-in scheme or a user scheme. The built-in schemes SHALL be rustxterm-light, rustxterm-dark, Solarized Dark, Solarized Light, Dracula, Gruvbox Dark, Gruvbox Light, Nord, One Dark, Monokai, Tango Dark, Tango Light and xterm. User schemes SHALL be loaded at start from JSON files of at most 64 KiB in the `schemes` folder of the configuration directory; scheme names SHALL be unique, built-in names win over user files and, among user files, the first file in file-name order wins. Font zoom SHALL change the font size by exactly one point per step and the new size SHALL apply to every open tab using it and persist across restarts.

#### Scenario: Global defaults
- **WHEN** the user changes the global cursor shape to underline
- **THEN** every tab without a per-session override uses an underline cursor

#### Scenario: Per-session font
- **WHEN** a session overrides font size to 14
- **THEN** only tabs of that session use size 14

#### Scenario: Built-in scheme
- **WHEN** the user picks "Dracula" as the global color scheme
- **THEN** every terminal tab without a per-session scheme uses the Dracula foreground, background, cursor, selection and 16 ANSI colors, in both light and dark UI themes

#### Scenario: Scheme preview
- **WHEN** the user opens the color scheme picker in Settings or the session editor
- **THEN** each entry shows its name and a swatch of its background, foreground and ANSI colors

#### Scenario: User scheme
- **WHEN** a valid scheme JSON file is placed in the `schemes` folder of the configuration directory and the application starts
- **THEN** the scheme appears in the picker under its name and can be selected

#### Scenario: Invalid user scheme
- **WHEN** a file in the `schemes` folder is not a valid scheme or is larger than 64 KiB
- **THEN** the file is skipped, a warning is logged, and the other schemes still load

#### Scenario: Duplicate scheme names
- **WHEN** a user file uses the name "Dracula", and two user files `a.json` and `b.json` both use the name "Mine"
- **THEN** the built-in Dracula is kept, "Mine" comes from `a.json`, and each skipped file is logged

#### Scenario: Session follows the UI theme
- **WHEN** the global scheme is Dracula and a session overrides the scheme with "Follow UI theme"
- **THEN** tabs of that session use the rustXterm light or dark scheme matching the UI theme, and the override is kept after saving and reopening the session

#### Scenario: Missing scheme
- **WHEN** a session or the global setting names a scheme that is not available
- **THEN** the terminal follows the UI theme and the stored setting is kept unchanged

#### Scenario: Follow UI theme
- **WHEN** the scheme is "Follow UI theme" and the user switches the UI between light and dark
- **THEN** the terminal switches between the rustXterm light and dark schemes

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
