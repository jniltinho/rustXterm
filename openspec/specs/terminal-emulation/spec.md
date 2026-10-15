# terminal-emulation Specification

## Purpose
Embedded terminal used by every text session (SSH in v0.1; local shell, Telnet, Serial and Mosh in phase 2) with xterm-compatible behavior.

## Requirements

### Requirement: xterm compatibility
The terminal SHALL advertise `TERM=xterm-256color` and support 256 colors, 24-bit truecolor, bold/italic/underline, alternate screen, bracketed paste, application cursor keys, mouse reporting (X10, SGR) and window resize propagation to the session.

#### Scenario: Full-screen application
- **WHEN** recorded output of `vim`, `htop` and `tmux` sessions (fixtures) is replayed into a terminal of fixed size
- **THEN** the resulting cell grid (characters, colors, attributes, cursor) equals the committed reference grid, and synthetic mouse clicks produce the expected SGR/X10 byte sequences

#### Scenario: Resize propagation
- **WHEN** the user resizes the window and the size settles
- **THEN** within 50 ms an SSH `window-change` request with the new rows/columns is sent
- **AND** in the Docker lab the remote `stty size` reports the new size within 500 ms

### Requirement: Scrollback and search
The terminal SHALL keep a configurable scrollback (default 10000 lines) and offer text search with next/previous and case sensitivity.

#### Scenario: Find text
- **WHEN** the user searches for `error` in a tab with 5000 lines of output
- **THEN** matches are highlighted and next/previous move through them

### Requirement: Clipboard
The terminal SHALL copy on selection (configurable), paste with right click (configurable) or Shift+Insert/Ctrl+Shift+V, and ask for confirmation before pasting text containing a newline into a remote session (configurable).

#### Scenario: Multi-line paste guard
- **WHEN** the user pastes three lines into an SSH tab with the guard enabled
- **THEN** a dialog shows the text and nothing is sent until the user confirms

### Requirement: Terminal context menu
Ctrl+right click on a terminal (or right click when right-click paste is off) SHALL open a menu with Copy, Paste, Select all, Find, Save terminal text, Clear scrollback, Reset terminal, Duplicate tab, Reconnect and Close tab. Unavailable items are disabled, not hidden.

#### Scenario: Select all from the menu
- **WHEN** the user Ctrl+right clicks a terminal and chooses Select all
- **THEN** the scrollback and the screen are selected and Copy copies them

### Requirement: Fonts and appearance
The user SHALL choose the terminal font family, size, cursor shape and blink, and color scheme per session or globally. The color scheme SHALL be "Follow UI theme", a built-in scheme or a user scheme loaded at start from the configuration directory; a session SHALL inherit the global scheme unless it overrides it. Font zoom SHALL change the size by one point per step and the size SHALL persist across tabs and restarts.

#### Scenario: Global defaults
- **WHEN** the user changes the global cursor shape to underline
- **THEN** every tab without a per-session override uses an underline cursor

#### Scenario: Per-session font
- **WHEN** a session overrides font size to 14
- **THEN** only tabs of that session use size 14

#### Scenario: Built-in scheme
- **WHEN** the user opens the color scheme picker
- **THEN** it lists rustxterm-light, rustxterm-dark, Solarized Dark, Solarized Light, Dracula, Gruvbox Dark, Gruvbox Light, Nord, One Dark, Monokai, Tango Dark, Tango Light and xterm

#### Scenario: Global scheme
- **WHEN** the user picks "Dracula" as the global color scheme
- **THEN** every terminal tab without a per-session scheme uses the Dracula foreground, background, cursor, selection and 16 ANSI colors, in both light and dark UI themes

#### Scenario: Scheme preview
- **WHEN** the user opens the color scheme picker in Settings or the session editor
- **THEN** each entry shows its name and a swatch of its background, foreground and ANSI colors

#### Scenario: User scheme
- **WHEN** a valid scheme JSON file of at most 64 KiB is placed in the `schemes` folder of the configuration directory and the application starts
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

### Requirement: Session logging
The user SHALL be able to log terminal output of a session to a file chosen in session settings, and save the visible terminal text to a file on demand.

#### Scenario: Log to file
- **WHEN** logging is enabled for a session and the user runs `ls`
- **THEN** the output appears in the log file without terminal escape sequences when "plain text" is selected, and with them when "raw" is selected

### Requirement: Bounded resource use
A single terminal SHALL NOT exceed its configured scrollback, buffered unread output per session SHALL be bounded (default 4 MiB, then the transport is no longer read until the UI drains it), and an output flood SHALL keep the UI responsive.

#### Scenario: Output flood
- **WHEN** an SSH session to the lab runs a sustained producer (`yes "$(head -c 200 /dev/urandom | base64 -w0)"`, which never finishes on its own) for 30 seconds on the reference machine recorded in the gate receipt (x86_64, 4 cores, integrated GPU)
- **THEN** the p95 time from a key event to its bytes being written to the transport is at most 100 ms
- **AND** the p95 time from a tab-switch event to the first frame showing the new tab is at most 100 ms
- **AND** the process resident memory measured after 30 s exceeds the resident memory measured with the same tab idle and an empty scrollback by at most the scrollback's maximum size in cells × 32 bytes plus 64 MiB
