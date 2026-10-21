# Design

## W1. One protocol, gated by the platform

`Protocol::Wsl` exists on every platform so a database or export shared between machines keeps WSL
sessions intact. Whether it can run is a platform fact: `Platform::wsl: Option<Arc<dyn Wsl>>` is
`Some` only on Windows. The UI offers the type (editor, quick connect, new-session menu) only when it
is `Some`; opening one without it ends the tab at once with "WSL sessions run only on Windows".

```rust
pub struct WslOptions {
    pub distribution: Option<String>, // None = the default distribution
    pub user: Option<String>,
    pub start_folder: Option<String>, // Linux path, e.g. "~" or "/srv/app"
    pub startup_commands: Option<String>, // lines run in the shell, which then stays open
}

pub trait Wsl: Send + Sync {
    fn distributions(&self) -> Result<Vec<WslDistribution>>; // name, default, version, state
    fn launch_info(&self, opts: &WslOptions) -> Result<LaunchInfo>;
}
```

## W2. Listing distributions

WSL is installed when its service is registered (`HKLM\SYSTEM\CurrentControlSet\Services\WslService`
for the Store WSL, `LxssManager` for the inbox one); `wsl.exe` alone proves nothing (Windows ships the
launcher before WSL is installed). Distributions come from the per-user WSL registry entries (`HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss`,
one subkey per distribution with `DistributionName`, `Version` and `State`; `DefaultDistribution`
names the default). It needs no process, no VM start and no localized text. States shown:

| State | When |
|---|---|
| Not installed | no WSL service registered (with or without `wsl.exe`) |
| No distribution | a WSL service registered, no `Lxss` distribution subkey |
| Could not query | registry access failed |
| Listed | one row per distribution, the default marked |

`wsl.exe --list --verbose` is only a fallback (and for the running state): the child gets
`WSL_UTF8=1` explicitly, and the output is decoded as UTF-8, or as UTF-16LE when it has a `FF FE` BOM
or NUL bytes; the header line is skipped by position, never matched as text. The probe runs off the
UI thread with a 10 s timeout and is killed and reaped on timeout. The `wsl --install` hint is shown
only on builds whose `wsl.exe` supports it (Windows 10 2004+ / Windows 11, Server 2022); otherwise a
link to Microsoft's install guide.

## W3. Launch

`launch_info` builds a `LaunchInfo` for `%SystemRoot%\System32\wsl.exe` with `-d <distribution>`,
`-u <user>` and `--cd <folder>` (the Linux startup folder; `LaunchInfo.cwd` stays the Windows profile
folder); nothing follows `--`, so the distribution starts the user's own login shell, whatever it is
(bash, zsh, fish). The ConPTY command line quotes every argument with the `CommandLineToArgvW` rules
(today `platform-windows` `command_line` joins them unquoted; fixed here for the local shell too).

`--cd` needs a WSL that has it (the Store WSL, and inbox builds from 21286); the floor is detected
from the WslService registration or the Windows build. On an older inbox WSL the folder is sent as the
first startup line, which bash, zsh and fish all read the same way: `cd` alone for `~`, `cd ~/'<rest>'`
for `~/<rest>` (the tilde stays unquoted so the shell expands it), and `cd '<folder>'` otherwise. In
that mode a folder holding `'` is refused.

**Startup commands** are typed into the interactive shell once it is ready, as MobaXterm does.
Ready means a prompt: after the first output, the terminal has been quiet for 300 ms **and** the
cursor line ends with a usual prompt end (`$ `, `# `, `> `, `% `, `❯ `). When no prompt shows within
10 s (a profile still running, or asking a question), nothing is typed: the tab shows a bar "Run the
startup commands now?" with Run and Skip, so commands never feed a startup script's own question.
Each line is then sent followed by Enter. They run in the user's real interactive shell after its normal start-up,
so what they set (an activated virtualenv, functions, the folder) stays for the session, and no
quoting crosses Windows. They are visible in the terminal like typed input; they are not secrets
(the editor says so) and are not saved as credentials.

Validation (refused before any process starts):
- folder and startup commands: no control characters (C0 controls such as CR, LF inside a line,
  Ctrl-U, ESC, and DEL), which a line editor would act on; lines are split on the editor's newlines
  only;
- distribution: WSL's own registration rule, `[A-Za-z0-9._-]+`;
- user: POSIX-style names, `[A-Za-z_][A-Za-z0-9_.-]*` with an optional trailing `$`
  (`Dev_User`, `_service` and `machine$` are valid).

Process lifetime: closing the tab closes the ConPTY and ends `wsl.exe` (the local terminal's job
object); WSL then hangs up the session's terminal, so foreground programs end. Programs that detach
(`nohup`, `setsid`, services) keep running in the distribution, as with MobaXterm and Windows
Terminal; rustXterm never runs `wsl --terminate` or `--shutdown`. Two tabs on one distribution are
independent sessions.

## W4. Tab behavior and protections

Same as the local terminal (banner "WSL: <distribution>", resize, exit status, reconnect starts a
new process), but a saved session: session logging is available per session as for SSH. The
local-terminal protections are extended to WSL (they match `Protocol::Local` only today,
`engine/src/control.rs`): agents (CLI/MCP) are denied WSL tabs, MultiExec does not send to WSL tabs
from other tabs, and data-stream capture leaves WSL tabs out in every mode (typed input included),
across toggles and reconnects.

## W4a. Compatibility

An older reader fails on an unknown `protocol` value (`Store::sessions()` stops at the first such
row, and an export with one is refused). This change makes the reader skip a session with an unknown
protocol, keep its row untouched and report a warning, for the database and for imports, so future
types do not break older files either. WSL sessions themselves need rustXterm 0.8.0 or newer; the
release notes say so. Tests: a database with SSH and WSL rows read by a build without WSL support
(simulated with an unknown protocol string) keeps the SSH sessions and warns.

## W5. MobaXterm, observed

Observed on MobaXterm 26.5 Portable on the Windows Server 2022 lab VM (2026-10-19), as its license
(section 5.1) allows: observe, study and test its operation; nothing was decompiled. Its WSL session
dialog has:

- Basic: **Distribution** (`Default` or a listed one), **Specify username** (checkbox and name),
  **Run method**: Autodetect; Localhost connection (an SSH tunnel from Windows into the distribution,
  for its X server and SSH file browser); Native connector (starts `bash` in the distribution);
  Wsl command (the `wsl` command, no other check).
- Advanced: **Graphical environment (experimental)** (`Interactive shell` or a full desktop in a new
  window), **Startup directory**, **Execute the following commands at startup** (multi-line),
  **Execute macro at session start**.

rustXterm follows the "Wsl command" method (W3) with distribution, user, startup directory and
startup commands; the other methods, the graphical desktop and macros are out of scope.

Not observed yet: the `MobaXterm.ini` layout of a WSL bookmark (type 14; a public sample shows
`14%1%`, so the field after the type is not a host name) and the exact `wsl.exe` command lines.
The VM had no WSL distribution and the dialog stopped taking input before saving. Task 1.0
repeats the observation on a machine with a distribution: save default and named-distribution
bookmarks with each option, read the `[Bookmarks]` lines and the started command lines, and keep
sanitized fixtures for the importer tests; unknown fields become warnings.

## W6. Tests

- Registry listing (fake), `--list --verbose` fallback fixtures: UTF-8 (`WSL_UTF8=1`), UTF-16LE with
  and without BOM, NUL padding, localized headers, default marker, no distributions; probe timeout.
- Arguments: quoting round trip through `CommandLineToArgvW` rules; nothing after `--`; invalid
  distribution and user names refused; `Dev_User`, `_service`, `machine$` accepted; the `--cd`
  floor and the `cd` fallback (`~`, `~/src` and `/srv/app`; folders with `'` refused there);
  control characters (CR, Ctrl-U, ESC, DEL) refused in folders and startup commands.
- Startup commands: sent only at a prompt (quiet 300 ms and a prompt end on the cursor line), one
  line per Enter, never before the first output; a profile that prints, pauses and asks a question
  gets nothing typed and the "Run the startup commands now?" bar appears after 10 s; the session keeps what they set (a virtualenv's `python` on the next
  prompt in the native receipt; bash and fish as login shells).
- Probe states: launcher present with no WSL service ("not installed"), service present with an
  empty registry ("no distribution"), registry failure ("could not query").
- Engine with the fake `Wsl` and a `FakePty` that records the `LaunchInfo` it is given (today it
  discards it); refusal without `Platform::wsl`; agent denial and MultiExec skip for WSL tabs.
- Session logging writes a WSL tab's output; data-stream capture never does (with and without typed
  input, across toggles and a reconnect).
- Store: unknown-protocol rows skipped with a warning; mixed SSH/WSL export and import.
- UI snapshots of the WSL editor page (distributions, not installed, no distribution) light and dark;
  the Linux/macOS refusal.
- Native receipt in `docs/gates/wsl.md` on Windows 11 and on Server 2022 with WSL: default and named
  distribution, user, folder, startup commands (a virtualenv stays active; a fish login shell),
  close hangs up a foreground job, a
  `nohup` job survives, two tabs independent, not-installed and no-distribution states.
