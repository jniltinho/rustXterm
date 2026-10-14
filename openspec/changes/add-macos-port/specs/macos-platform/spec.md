## Purpose

Run rustXterm natively on macOS 13 Ventura or later on Apple Silicon through a macOS adapter for the platform contracts, with macOS storage locations, data protection and system integration, and without `unsafe` code.

## ADDED Requirements

### Requirement: Supported macOS versions
rustXterm SHALL run on macOS 13 Ventura or later on Apple Silicon (arm64) without installing a runtime (no Homebrew, Rust toolchain, Python or XQuartz). The executable SHALL link only to libraries and frameworks shipped with macOS.

#### Scenario: Clean Mac
- **WHEN** the `.dmg` is opened on a clean Mac with a supported version and `rustXterm.app` is dragged to Applications and started
- **THEN** the main window opens without a missing-library error

#### Scenario: Link allowlist
- **WHEN** the release check lists the libraries the executable links to
- **THEN** every entry is under `/usr/lib` or `/System/Library` and none is from Homebrew or a build directory

### Requirement: Per-user folders on macOS
On macOS, settings, the encrypted database, envelope and backups SHALL live in `~/Library/Application Support/rustxterm`, logs in `~/Library/Logs/rustxterm`, the cache in `~/Library/Caches/rustxterm`, and the SSH directory SHALL be `~/.ssh` (read-only for `known_hosts`, as on Linux). A `data_dir` setting SHALL override the data location as on Linux, and the data directory SHALL get the same owner-only modes (0700 directories, 0600 files) as on Linux.

#### Scenario: First run locations
- **WHEN** a new user starts rustXterm and creates the database
- **THEN** `config.toml` and the database are under `~/Library/Application Support/rustxterm` and the data directory has mode 0700

### Requirement: Theme following on macOS
With the theme set to System, rustXterm SHALL follow the macOS light/dark appearance, including the automatic mode, within 2 seconds of a change and without a restart.

#### Scenario: Appearance switch
- **WHEN** the user switches System Settings → Appearance from Light to Dark while rustXterm runs with the System theme
- **THEN** every rustXterm screen turns dark within 2 seconds

### Requirement: Native dialogs, notifications and opener on macOS
File dialogs SHALL be native macOS panels attached to the main window; notifications SHALL use the macOS notification center; opening a downloaded file, folder or URL SHALL use the macOS default application, and a failure SHALL be reported to the user instead of being ignored.

#### Scenario: Open a downloaded file
- **WHEN** the user opens a `.txt` file from the transfer list
- **THEN** it opens in the default text editor

#### Scenario: Save dialog
- **WHEN** the user chooses "Save terminal output"
- **THEN** a native macOS save panel appears in front of the main window

### Requirement: Agent control on macOS
The agent control channel SHALL use a Unix domain socket in a per-user directory that only the user can access, and SHALL serve only clients running as the same user, as on Linux; other clients SHALL be disconnected without a response.

#### Scenario: Other user refused
- **WHEN** a process of another macOS user connects to the control socket
- **THEN** the connection is closed without a response and nothing is executed

### Requirement: Serial ports on macOS
Serial port discovery on macOS SHALL list call-out devices (`/dev/cu.*`) with their USB vendor, product and serial number when available, and SHALL hide the matching dial-in devices (`/dev/tty.*`) so each adapter appears once.

#### Scenario: USB adapter listed once
- **WHEN** a USB serial adapter is plugged in
- **THEN** the port list shows one entry for it, its `/dev/cu.*` path, with the USB details
