## ADDED Requirements

### Requirement: WSL session type on Windows
On Windows, the application SHALL offer a WSL session type that opens a WSL distribution's shell in a terminal tab, with the distribution (default or named), an optional user, an optional startup folder and optional startup commands run in the shell, which then stays open. On Linux and macOS the type SHALL NOT be offered, and opening an existing WSL session SHALL end the tab with "WSL sessions run only on Windows" without starting anything.

#### Scenario: Open the default distribution
- **WHEN** a Windows user saves a WSL session with the default distribution and opens it
- **THEN** a terminal tab shows the default distribution's login shell

#### Scenario: Closing the tab hangs up the session
- **WHEN** a WSL tab runs `sleep 1000` in the foreground and `nohup sleep 2000 &`, and the user closes the tab
- **THEN** `sleep 1000` ends, `sleep 2000` keeps running in the distribution, and rustXterm never terminates the distribution

#### Scenario: Named distribution, user, folder and startup commands
- **WHEN** a WSL session names distribution `Debian`, user `dev`, startup folder `/srv/app` and the startup command `source .venv/bin/activate`
- **THEN** the tab runs `wsl.exe -d Debian -u dev --cd /srv/app`, types the command into the shell once it is ready, and the next prompt is in `/srv/app` with the virtualenv active

#### Scenario: Any login shell
- **WHEN** the WSL user's login shell is fish and the session has startup commands
- **THEN** fish starts and runs the startup commands as typed lines

#### Scenario: Older WSL without --cd
- **WHEN** the installed WSL has no `--cd` option and the session's startup folder is `/srv/app`, `~` or `~/src`
- **THEN** the shell starts and the first startup line is `cd '/srv/app'`, `cd` or `cd ~/'src'`

#### Scenario: Startup commands wait for a prompt
- **WHEN** the user's profile prints a message, pauses and asks a question before showing a prompt
- **THEN** no startup command is typed into that question, and after 10 s the tab offers to run them now or skip them

#### Scenario: Startup commands reach the Linux shell unchanged
- **WHEN** a WSL session's startup command is `ls "$HOME" | grep -c "my file"`
- **THEN** the interactive shell receives that exact line and stays open

#### Scenario: Not offered outside Windows
- **WHEN** a WSL session imported from a Windows machine is opened on Linux
- **THEN** the tab ends with "WSL sessions run only on Windows" and the session stays in the tree unchanged

### Requirement: Distribution detection
The session editor SHALL list the installed WSL distributions with the default one marked and a refresh action, read without blocking the interface and without matching localized text, and SHALL tell apart "WSL not installed", "no distribution" and "could not query". The application SHALL NOT install WSL or distributions.

#### Scenario: Distributions listed
- **WHEN** WSL has `Ubuntu` (default) and `Debian` installed and the user edits a WSL session
- **THEN** the distribution picker shows "Default (Ubuntu)", `Ubuntu` and `Debian`

#### Scenario: WSL missing
- **WHEN** no WSL service is registered, even though `wsl.exe` is present
- **THEN** the editor shows that WSL is not installed, with the install hint that fits the Windows build, and nothing is installed

#### Scenario: No distribution
- **WHEN** WSL is installed without any distribution
- **THEN** the editor shows that no distribution is installed, not that WSL is missing

#### Scenario: Localized and UTF-8 output
- **WHEN** the fallback `wsl.exe --list --verbose` prints a non-English header, in UTF-16LE with a BOM or in UTF-8 (`WSL_UTF8=1`)
- **THEN** the distributions and the default are still read correctly

### Requirement: Safe launch arguments
The WSL launch SHALL pass only the distribution, user and folder to `wsl.exe`, as separately quoted arguments and never through a Windows shell; startup commands SHALL be typed into the shell as terminal input. It SHALL refuse distribution names outside `[A-Za-z0-9._-]`, user names outside POSIX-style names (letters, digits, `_`, `.`, `-`, not starting with a digit, `-` or `.`, optional trailing `$`), and control characters in the folder or the startup commands.

#### Scenario: Injection attempt refused
- **WHEN** a session's distribution is `Ubuntu & calc.exe`
- **THEN** the tab ends with an invalid-name error and no process is started

#### Scenario: Control characters refused
- **WHEN** a session's startup folder or a startup command contains a carriage return, Ctrl-U or ESC
- **THEN** the tab ends with an invalid-value error and nothing is typed or started

#### Scenario: Valid unusual user names
- **WHEN** the user is `Dev_User`, `_service` or `machine$`
- **THEN** the session starts as that user

### Requirement: WSL quick connect and import
Quick connect SHALL accept `wsl://` (default distribution) and `wsl://<distribution>` on Windows, and the MobaXterm.ini importer SHALL turn WSL bookmarks into WSL sessions with their distribution, reporting unknown fields as warnings.

#### Scenario: Quick connect
- **WHEN** a Windows user types `wsl://Debian` in quick connect
- **THEN** a WSL tab opens on the `Debian` distribution

#### Scenario: Import a WSL bookmark
- **WHEN** a MobaXterm.ini contains a WSL bookmark for `Ubuntu-22.04`
- **THEN** the import preview lists a WSL session for `Ubuntu-22.04`

### Requirement: WSL tabs are protected like local terminals
WSL tabs SHALL support per-session logging like other saved sessions, and SHALL get the local terminal's protections: no agent (CLI/MCP) access, no MultiExec input from other tabs, and no data-stream capture in any mode.

#### Scenario: Session log written
- **WHEN** a WSL session has logging enabled and the user runs `ls`
- **THEN** the session's log file contains the output of `ls`

#### Scenario: Stream capture never records WSL
- **WHEN** data-stream capture is on, with and without typed input, toggled and after a reconnect, and a WSL tab is open
- **THEN** no stream file is created for the WSL tab

#### Scenario: Agents and MultiExec
- **WHEN** an agent asks to read or send to a WSL tab, or MultiExec sends from an SSH tab with a WSL tab selected
- **THEN** the agent is denied as for local terminals and the WSL tab receives nothing

### Requirement: Readers tolerate unknown session types
Reading the session database or an import SHALL skip a session whose protocol is unknown to this version, keep its stored row unchanged and report a warning, instead of failing.

#### Scenario: Older reader meets a WSL session
- **WHEN** a database holds an SSH session and a session of a protocol this build does not know
- **THEN** the SSH session loads, the unknown one is reported in a warning and its row is left untouched
