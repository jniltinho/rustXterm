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
The terminal SHALL copy on selection (configurable), paste with right click or Shift+Insert/Ctrl+Shift+V, and ask for confirmation before pasting text containing a newline into a remote session (configurable).

#### Scenario: Multi-line paste guard
- **WHEN** the user pastes three lines into an SSH tab with the guard enabled
- **THEN** a dialog shows the text and nothing is sent until the user confirms

### Requirement: Fonts and appearance
The user SHALL choose the terminal font family, size, cursor shape and blink, and color scheme per session or globally.

#### Scenario: Global defaults
- **WHEN** the user changes the global cursor shape to underline
- **THEN** every tab without a per-session override uses an underline cursor

#### Scenario: Per-session font
- **WHEN** a session overrides font size to 14
- **THEN** only tabs of that session use size 14

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
