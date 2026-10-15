## Purpose
Local terminal tabs running the user's own shell on this machine, opened from the tab strip "+" like MobaXterm's local terminal, for quick local work next to remote sessions.

## ADDED Requirements

### Requirement: Open a local terminal
Where a local shell backend exists (Linux; Windows 10 1809 or later with ConPTY), the tab strip "+" button and `Ctrl+Shift+T` SHALL open a new active tab running a local shell in a pseudo-terminal, titled `user@host` with a local terminal icon. Opening SHALL NOT require an unlocked session database.

- Linux shell: `$SHELL` when it is an absolute path to an executable file, else the passwd login shell when executable, else `/bin/sh`; started as a login shell.
- Windows shell: `pwsh.exe` found on `PATH`, else `%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe`, else `%ComSpec%`; PowerShell started with `-NoLogo`. Paths with spaces SHALL work. No console window SHALL appear.
- Working folder: `$HOME` / `%USERPROFILE%`; when missing or not a folder, `/` / `%SystemDrive%\`.
- Environment: the application's environment, plus `TERM=xterm-256color` and `COLORTERM=truecolor`. On Linux, variables added by the AppImage runtime (`APPDIR`, `APPIMAGE`, `ARGV0`, `OWD`) SHALL be removed and entries under `$APPDIR` removed from path-list variables; the application's own environment SHALL NOT change. Environment contents SHALL never be logged.

#### Scenario: Plus opens a local shell on Linux
- **WHEN** the user clicks "+" with `SHELL=/bin/bash` and `HOME=/home/alice`
- **THEN** a new active tab opens, `pwd` prints `/home/alice` and `echo $TERM` prints `xterm-256color`

#### Scenario: Invalid SHELL falls back
- **WHEN** `SHELL` points to a missing or non-executable file
- **THEN** the passwd login shell (or `/bin/sh`) starts and no error is shown

#### Scenario: Windows opens PowerShell
- **WHEN** the user clicks "+" on Windows without PowerShell 7
- **THEN** Windows PowerShell starts in `%USERPROFILE%`, `$env:TERM` is `xterm-256color` and no console window opens

#### Scenario: Windows prefers PowerShell 7
- **WHEN** `pwsh.exe` is on `PATH`
- **THEN** "+" starts `pwsh.exe`

#### Scenario: Windows falls back to cmd
- **WHEN** neither PowerShell is available
- **THEN** `%ComSpec%` starts

#### Scenario: AppImage environment
- **WHEN** the AppImage build opens a local terminal
- **THEN** `echo $APPDIR` prints nothing and `LD_LIBRARY_PATH` has no entry under the AppImage mount

#### Scenario: Shell cannot start
- **WHEN** the backend exists but the shell fails to start (for example every candidate is missing)
- **THEN** the tab shows the error state with the reason and the application keeps running

### Requirement: Welcome banner
When "Show session banner" is on, a local terminal tab SHALL show a welcome box before the first prompt, in the session banner style: application name and version, `user@host`, the shell path, the working folder, and tips (New session with Ctrl+Shift+N, context menu with Ctrl+right click). The banner SHALL be local output only, never sent to the shell, and the first prompt SHALL appear below it (on Windows the pseudo-console starts at the cursor after the banner). It SHALL be shown again on restart (R).

#### Scenario: Banner shown
- **WHEN** a local terminal opens with the banner setting on
- **THEN** the box with the version, user@host, shell and folder appears and the first prompt is on the line below it

#### Scenario: Banner kept on resize
- **WHEN** the window is resized after the first prompt on Windows
- **THEN** the banner is still above the prompt

#### Scenario: Banner off
- **WHEN** the banner setting is off
- **THEN** the tab shows only the shell output

### Requirement: Local terminal behavior
A local terminal tab SHALL behave like other terminal tabs: resizing the view resizes the pseudo-terminal, typing (including Unicode and Ctrl+C) reaches the shell, and copy/paste, find, the context menu, Save terminal text, rename and close work. Duplicate SHALL open another local terminal. When the shell exits, the tab SHALL show the "Session stopped" banner after the shell's last output, and Reconnect (R) SHALL start a new shell in the same tab. A tab SHALL close promptly even when the shell produces no output or a flood of output.

#### Scenario: Resize
- **WHEN** the user resizes the window
- **THEN** `stty size` (Linux) or `$Host.UI.RawUI.WindowSize` (Windows) reports the new size

#### Scenario: Shell exits and restarts
- **WHEN** the user types `exit` and then presses R
- **THEN** the "Session stopped" banner appears and then a new prompt starts in the same tab

#### Scenario: Close during flood
- **WHEN** the user closes a local tab running `yes` (Linux) or an endless `Write-Output` loop (Windows)
- **THEN** the tab closes within 2 seconds and the UI stays responsive

### Requirement: Process containment
Closing a local tab (after the confirmation, when enabled) or quitting the application SHALL end the shell and the processes it started. On Windows every process started from the shell SHALL be in a job object assigned before the shell runs, killed when the tab closes or the application ends, including a forced end. On Linux the shell SHALL run in its own session; closing SHALL send SIGHUP and then SIGKILL after a grace period to every process group of that session. Processes that deliberately leave the session (`setsid`, daemons) are outside this guarantee on Linux, as in other terminal emulators.

#### Scenario: Background jobs end on close
- **WHEN** the user closes a Linux local tab running `sleep 1000 &`, `trap '' HUP; sleep 1000` and a foreground `sleep 1000 | cat`
- **THEN** none of those processes remain after 5 seconds

#### Scenario: Windows tree ends on close
- **WHEN** the user closes a Windows local tab whose shell started `cmd /c ping -t localhost`
- **THEN** no process started by that tab remains

#### Scenario: Windows tree ends when the app is killed
- **WHEN** the application is ended from Task Manager with a local tab running `ping -t localhost`
- **THEN** the `ping` process ends too

### Requirement: Local terminals are never persisted
Local terminals SHALL NOT appear in the session editor protocol list, the session tree, quick connect, recents, history, import or export, and the store SHALL refuse to save or import a session with the local protocol.

#### Scenario: Not in recents
- **WHEN** the user opens a local terminal and restarts the application
- **THEN** the recent sessions list has no local entry

#### Scenario: Crafted import refused
- **WHEN** the user imports a TOML file containing a session with `protocol = "local"`
- **THEN** that entry is rejected with a message and the other entries import

### Requirement: Platform availability
Where no local shell backend exists (macOS until its port; Windows without ConPTY, detected without crashing), "+" SHALL open the New session editor, `Ctrl+Shift+T` SHALL do nothing and no local terminal entry SHALL be shown.

#### Scenario: No backend
- **WHEN** the user clicks "+" on a system without a local shell backend
- **THEN** the New session editor opens
