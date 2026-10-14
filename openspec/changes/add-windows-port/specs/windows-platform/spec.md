## Purpose

Run rustXterm natively on Windows 10 1809+, Windows 11 and Windows Server 2019+ with desktop experience (x86_64) through a Windows adapter for the platform contracts, with Windows-appropriate storage locations, data protection and system integration, while keeping unsafe code confined.

## ADDED Requirements

### Requirement: Supported Windows versions
rustXterm SHALL run on Windows 10 version 1809 or later, Windows 11 and Windows Server 2019 or later with desktop experience, x86_64, without installing a runtime (no Visual C++ redistributable, .NET, MSYS2, Wine or Rust toolchain). The executables SHALL import only DLLs shipped with Windows.

#### Scenario: Clean Windows machine
- **WHEN** the portable zip is extracted on a clean Windows installation of a supported version and `rustxterm.exe` is started
- **THEN** the main window opens without a missing-DLL error

#### Scenario: Import allowlist
- **WHEN** the release check lists the executables' imported DLLs
- **THEN** every entry is a Windows system DLL and none is a MinGW, OpenSSL or SQLite DLL

### Requirement: Per-user folders on Windows
On Windows, settings SHALL live in `%APPDATA%\rustxterm`, the encrypted database, envelope, backups, logs and cache in `%LOCALAPPDATA%\rustxterm`, and the SSH directory SHALL be `%USERPROFILE%\.ssh` (shared with Windows OpenSSH, read-only for `known_hosts`). A `data_dir` setting SHALL override the data location as on Linux.

#### Scenario: First run locations
- **WHEN** a new user starts rustXterm and creates the database
- **THEN** `config.toml` is under `%APPDATA%\rustxterm` and the database and envelope are under `%LOCALAPPDATA%\rustxterm`

### Requirement: Owner-only data protection on Windows
The data directory (default or custom `data_dir`) and, while agent control is enabled, the agent workspace SHALL be owned by the current user and carry a protected access-control list granting access only to the current user, inherited by new files; missing directories SHALL be created with that list from the start; files and subdirectories already inside that only grant others extra read access SHALL be given the same owner and owner-only list, while existing entries owned by another user or that another user could modify (write, append, change attributes or permissions, delete) SHALL be rejected rather than repaired, checking the whole tree before changing anything; the result SHALL be read back and verified. Directories outside the user's profile directory, directories with an ancestor (up to the profile directory) that has no access-control list or lets another user delete, rename, write or change attributes or permissions, directories reached through a symbolic link or junction in their path, directories on network locations (UNC paths, mapped network drives) or on volumes without persistent access-control lists (FAT, exFAT), and directories containing symbolic links, junctions or other reparse points, SHALL be rejected, so that no other standard user can replace the directory or an ancestor. When the data directory is rejected or its protection cannot be applied or verified, the application SHALL refuse to create or open the database and SHALL show which path failed; for the workspace, agent file transfer SHALL stay unavailable and the failure SHALL be reported. The configuration directory (settings only, no secrets) keeps the per-user profile protection.

#### Scenario: Other user denied
- **WHEN** another standard (non-administrator) user on the same machine tries to read the database, its WAL file, the key envelope or a backup
- **THEN** access is denied

#### Scenario: Existing files repaired
- **WHEN** the data directory already contains files that other users may only read (for example created by an older version or copied in)
- **THEN** after start every file in it is owned by the user and accessible only to the user

#### Scenario: Existing entry others can modify
- **WHEN** the data directory or an entry in it lets another user write, delete or change its permissions
- **THEN** the store is not opened, the entry is left unchanged and the error names it

#### Scenario: Network or FAT data directory
- **WHEN** `data_dir` points to a UNC path, a mapped network drive or a FAT/exFAT volume
- **THEN** no database file is created or opened there and the error names the directory

#### Scenario: Replaceable location
- **WHEN** `data_dir` points outside the user profile (for example `C:\shared\rustxterm`), below a profile folder that grants other users any write access or has no access-control list, or through a junction in its path
- **THEN** the store is not opened and the error names the directory

#### Scenario: Protection fails
- **WHEN** `data_dir` points to a location where the access-control list cannot be set or contains a junction
- **THEN** the store is not opened and an error names the path

### Requirement: Open with the default application on Windows
Opening a downloaded file, folder or URL SHALL use the Windows default handler for it, and a failure (no associated application, missing file) SHALL be reported to the user instead of being ignored.

#### Scenario: Open a downloaded file
- **WHEN** the user opens a `.txt` file from the transfer list
- **THEN** it opens in the default text editor

### Requirement: Unsafe code confinement
Platform code that needs direct Windows API calls SHALL be confined to the Windows adapter crate; every other crate SHALL keep forbidding `unsafe` code. Each unsafe block in the adapter SHALL carry a written safety justification, and the build checks SHALL fail when an unsafe block lacks one or another crate relaxes the rule (user decision).

#### Scenario: Unsafe outside the adapter
- **WHEN** a commit adds an `unsafe` block or relaxes the `unsafe_code` lint in any crate other than the Windows adapter
- **THEN** the lint or layer check fails

#### Scenario: Undocumented unsafe block
- **WHEN** an `unsafe` block in the Windows adapter has no `// SAFETY:` comment
- **THEN** the clippy gate fails
