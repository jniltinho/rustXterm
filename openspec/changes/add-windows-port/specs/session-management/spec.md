## ADDED Requirements

### Requirement: Keyring wrap on Windows
On Windows the optional keyring wrap of the key envelope SHALL store its random key in the current user's Windows Credential Manager as a generic credential that does not roam, named so the user can find and delete it (`rustxterm:<id>`). When Credential Manager is unavailable (for example in a logon session without a credential store) the application SHALL fall back to the master password without failing, as on Linux.

#### Scenario: Unlock without master password on Windows
- **WHEN** the user enabled keyring unlock and starts rustXterm again in the same Windows account
- **THEN** the database opens without asking for the master password and a `rustxterm:` entry exists in Credential Manager

#### Scenario: Credential removed
- **WHEN** the user deletes the `rustxterm:` entry in Credential Manager
- **THEN** the next start asks for the master password and nothing is lost

#### Scenario: Credential Manager unavailable
- **WHEN** Credential Manager cannot be used in the current session
- **THEN** the master password prompt appears and unlocking works

## MODIFIED Requirements

### Requirement: Encrypted session database
Sessions, folders, tunnels, saved credentials, SSH/RDP trust records owned by the application, agent policies, agent audit log and history SHALL be stored in one embedded SQLite database encrypted at rest (SQLCipher, AES-256) under `$XDG_DATA_HOME/rustxterm/` (platform equivalent elsewhere: `%LOCALAPPDATA%\rustxterm` on Windows), with file mode 0600 on Unix and, on Windows, a protected owner-only access-control list on the data directory that the database, its WAL/SHM files, the key envelope and backups inherit, WAL mode, schema migrations by version and 5 rolling encrypted backups. The OpenSSH `~/.ssh/known_hosts` file is only read, never written.

#### Scenario: Encrypted at rest
- **WHEN** the database file is opened by a plain SQLite client or scanned for a known session host name
- **THEN** it is not readable and the host name is not found

#### Scenario: Crash during write
- **WHEN** the application is killed while saving a session
- **THEN** on next start the database opens consistently with either the old or the new session

#### Scenario: Migration
- **WHEN** a database from schema version N is opened by a build with version N+1
- **THEN** an encrypted backup is taken, the migration runs in one transaction, and on failure the original database is restored
