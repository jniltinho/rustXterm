# session-management Specification

## Purpose
Persistent, searchable session definitions in an encrypted local database, basic settings in TOML/.env, and import of sessions from `MobaXterm.ini` files.

## Requirements

### Requirement: Basic settings in TOML and .env
Non-secret application settings (theme, UI layout defaults, data directory, log level, agent-control enablement) SHALL be read from `$XDG_CONFIG_HOME/rustxterm/config.toml` (platform equivalent elsewhere), optionally overridden by a `.env` file passed explicitly with `--env-file` and by `RXT_*` environment variables, with precedence CLI flag > environment/.env > TOML > built-in default. Secret-like keys SHALL be rejected in TOML and .env with an error naming the key. An invalid TOML file SHALL be backed up and replaced by defaults with a notice naming the file.

#### Scenario: Precedence
- **WHEN** `config.toml` sets `theme = "light"` and `--env-file dev.env` contains `RXT_THEME=dark`
- **THEN** the application starts in dark theme

#### Scenario: Secret in TOML
- **WHEN** `config.toml` contains a `password` key
- **THEN** that configuration is refused and the error names the key and file

#### Scenario: Corrupt TOML
- **WHEN** `config.toml` contains invalid TOML
- **THEN** the application starts with defaults, keeps a backup of the file and shows a notice naming it

### Requirement: Encrypted session database
Sessions, folders, tunnels, saved credentials, SSH/RDP trust records owned by the application, agent policies, agent audit log and history SHALL be stored in one embedded SQLite database encrypted at rest (SQLCipher, AES-256) under `$XDG_DATA_HOME/rustxterm/` (platform equivalent elsewhere), with file mode 0600, WAL mode, schema migrations by version and 5 rolling encrypted backups. The OpenSSH `~/.ssh/known_hosts` file is only read, never written.

#### Scenario: Encrypted at rest
- **WHEN** the database file is opened by a plain SQLite client or scanned for a known session host name
- **THEN** it is not readable and the host name is not found

#### Scenario: Crash during write
- **WHEN** the application is killed while saving a session
- **THEN** on next start the database opens consistently with either the old or the new session

#### Scenario: Migration
- **WHEN** a database from schema version N is opened by a build with version N+1
- **THEN** an encrypted backup is taken, the migration runs in one transaction, and on failure the original database is restored

### Requirement: Key envelope and master password
The database key SHALL be a random 256-bit key stored only inside a versioned key envelope next to the database. The envelope SHALL always contain a wrap under a key derived from the user's master password (Argon2id with stored parameters bounded to memory 64–1024 MiB, iterations 2–10, parallelism 1–4, a random 16-byte salt and XChaCha20-Poly1305 with a fresh random 24-byte nonce per wrap) and MAY additionally contain a wrap under a random key stored in the desktop keyring when the user enables "unlock with system keyring". The envelope SHALL be replaced atomically (write, fsync, rename) and every database backup SHALL be stored with the envelope that unlocks it. The raw key and master password SHALL never be written to disk, logs, CLI or MCP output.

#### Scenario: First start
- **WHEN** the application starts with no database
- **THEN** the user creates a master password before any session can be saved, and is told that a forgotten master password makes the data unrecoverable

#### Scenario: Keyring convenience
- **WHEN** "unlock with system keyring" is enabled and the keyring is available
- **THEN** the database unlocks without prompting, and the master password still unlocks it when the keyring is unavailable

#### Scenario: Wrong master password
- **WHEN** the user enters a wrong master password
- **THEN** the database stays locked, an error is shown, and repeated failures add increasing delay

#### Scenario: Interrupted password change
- **WHEN** the application is killed while changing the master password
- **THEN** on next start either the old or the new master password unlocks the database

#### Scenario: Restore backup without keyring
- **WHEN** the user restores a backup on a machine with no keyring
- **THEN** the master password valid at backup time unlocks it

#### Scenario: Locked database
- **WHEN** the database is locked
- **THEN** the session tree shows a locked state with an unlock action and nothing else is lost

### Requirement: Saved credentials
Passwords and key passphrases SHALL be saved only when the user ticks "save password", only inside the encrypted database, held in memory in zeroizing buffers, and never included in exports, logs, errors or agent output.

#### Scenario: Save password
- **WHEN** the user ticks "save password" and connects successfully
- **THEN** the next connection of that session does not prompt and the password is not present in plaintext in any file

### Requirement: Session tree
The Sessions sidebar SHALL show sessions in nested folders with icons per type, and support create, edit, duplicate, delete (with confirmation), move by drag-and-drop, rename, filter by text, and open by double click.

#### Scenario: Filter
- **WHEN** the user types `prod` in the filter
- **THEN** only sessions or folders whose name or host contains `prod` remain visible, with parent folders expanded

### Requirement: Session editor
The session editor SHALL present a session-type picker (SSH, SFTP, FTP, RDP, VNC in v0.1) and type-specific Basic, Advanced, Terminal, Network and Agent settings, validating fields before saving.

#### Scenario: Invalid port
- **WHEN** the user enters port 70000
- **THEN** the field shows an error and Save stays disabled

### Requirement: MobaXterm.ini import
The application SHALL import session bookmarks from a user-selected `MobaXterm.ini` (Home or Portable), mapping folders and the SSH, SFTP, FTP/FTPS, RDP and VNC session types (identified by the bookmark's type field, not its icon) and supported fields (host, port, user, private key path, X11, compression, jump host, remote command, terminal font/colors when parseable). Other types SHALL be listed and skipped. Password and credential sections SHALL never be read. Import SHALL show a preview and never modify the source file.

#### Scenario: Import bookmarks
- **WHEN** the user imports an ini with 10 SSH sessions in 2 folders and 1 Telnet session
- **THEN** the preview shows 10 importable sessions in 2 folders and 1 skipped Telnet session, and confirming adds them

#### Scenario: Malformed line
- **WHEN** a bookmark line has fewer fields than expected
- **THEN** that entry is reported as skipped with its line number and the rest import normally

#### Scenario: Source untouched
- **WHEN** an import completes
- **THEN** the source file is byte-identical to before

### Requirement: Export and import
The user SHALL export and import sessions, folders and tunnels as rustXterm TOML without credentials.

#### Scenario: Round trip
- **WHEN** sessions are exported and imported into a clean profile
- **THEN** all sessions and folders are identical except credentials
