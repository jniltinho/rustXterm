# Design

## D1. Levels and filters

| Level | File filter |
|---|---|
| Off | no file (console only, `log_level`) |
| Errors | `warn` |
| Normal | `info` |
| Full debug | the `--debug-log` filter: `debug`, graphics stack quiet |
| Protocol trace | Full debug plus `trace` for the chosen protocols' crates (`rxt_ssh`, `rxt_rdp` and `ironrdp_*`, `rxt_vnc` and `vnc`, `rxt_ftp`, `rxt_telnet`, `rxt_serial`, `rxt_local`) |

Every level appends the secret exclusions (`sspi`, `picky`, `picky_krb`,
`ironrdp_connector::credssp` at `info`); a unit test asserts that each level's filter string ends with
them.

Live switching: the file layer is always registered (also at Off), with `reload` used only on its
`EnvFilter` (Off = `off`). `reload` cannot replace a `Filtered` layer or insert one later, so the
writer is a `MakeWriter` over a shared slot that the settings code points at a new file, or at
nothing. Tested Off → Full debug → Off with a connection in progress.

## D2. Folder

Default: the current directory, then the config folder when the current directory is not writable
(same rule as `--debug-log`). Settings shows the resolved absolute path; **Choose folder…** stores an
explicit folder. Files: `rustxterm-diagnostic.log` and `streams/<session>-<yyyymmdd-hhmmss>.log`.
Files go into an app-owned `rustxterm-logs/` subfolder of the chosen folder, never into it directly.
Opening follows the `--debug-log` rules: Unix creates with `create_new` + `0600`, and an existing file
must be a regular file with one link and the same inode once opened (rotation files included). On
Windows only that subfolder is protected (owner-only ACL on it, not `Paths::make_private`, which
recurses and refuses folders outside the profile). A folder that cannot be created or protected
falls back to the config folder, and the page says why.

## D3. Data stream

The tap sits on the session transport after authentication finishes, so credentials the app sends
itself (saved passwords, Telnet auto-login, SSH user authentication) never pass through it. It records
`time direction length` and a hex+text dump of each chunk. Received bytes can still hold secrets the
server echoes (a password typed where echo is on, another MultiExec target's echo, a pasted secret),
so turning the switch on needs consent each time and the files are treated like session logs
(`session-management` "Saved credentials" still holds: the app never writes a saved credential).
Sent bytes are recorded only with **Include typed input**. Serial's synthetic local echo is generated
above the tap and is not recorded. RDP/VNC record the PDU/message type and length only. Local terminal
tabs have no tap (their spec forbids log files). The switch is never saved; while on, the status bar
shows a red "Recording stream" chip that opens the page.

## D3a. Writer

Protocol tasks never write files: taps and the log layer push records into a bounded channel
(4 MiB) drained by one background thread that owns the files and does rotation, clearing and folder
changes in order. When the channel is full, records are dropped and counted, and the file gets a
"N records dropped" line; a write error stops capture and shows a notice on the page.

## D4. Rotation

At 50 MB a file is renamed `.1` (`.1` to `.2`, `.2` to `.3`, `.3` deleted). The writer checks the
size on each write, so rotation needs no timer.

## D5. Precedence

`--debug-log` / `RXT_DEBUG_LOG` > Settings level for that run (the page then shows the level as set by
the command line and greys out the choice).
