Depends on `create-native-rust-linux-terminal` being implemented (engine transport seam, terminal widget, settings, encrypted store, lab runner). Lowest priority of the phase-2 modules.

## 1. r-commands milestone (code)

- [ ] 1.1 Pick the lab server package (rsh-redone or netkit; rlogind, rshd, rexecd) and build the `rsh` lab image on a bridge network without published ports (design R7); verify a manual `rlogin` from the toolchain container as root reaches a shell
- [ ] 1.2 Create `crates/rsh` (L2, registered in `cargo xtask layers`): bounded sans-IO handshakes for rlogin, rsh (stderr port 0) and rexec, rlogin window-size encoding (R1, R3); verify unit tests with byte fixtures, server error answers, oversized error messages and NUL in fields
- [ ] 1.3 Reserved-port binding contract in `platform` and Linux adapter (try 1023→512, stop on `EACCES`, report `ip_unprivileged_port_start`, never write it) and the per-session unprivileged fallback (R2); verify unit tests with a fake binder returning `EACCES`/`EADDRINUSE`
- [ ] 1.4 tokio driver producing the shell pair, rexec password from the store or prompt with zeroization and no logging (R4); verify unit tests over an in-process TCP peer
- [ ] 1.5 `core` `Protocol::Rsh`, `RshOptions`, `settings.legacy_rcommands` (default false); `engine` transport branch and refusal of agent `session.open` while disabled (R5, R6); verify serde/settings tests and engine unit tests
- [ ] 1.6 `ui`: Settings → Security toggle, hidden/disabled states, Rsh editor page with warning banner, first-connect confirmation, Unencrypted marker, privileged-port explanation dialog, quick connect `rlogin://`/`rsh://`, icon (from `assets/icons/app/`, or `terminal-user` fallback); verify kittest snapshots in light and dark with the toggle on and off
- [ ] 1.7 `store`: MobaXterm.ini Rsh import after confirming type code `2`, field layout and rlogin representation with sample files; skipped-with-reason while the toggle is off; verify importer unit tests with hand-written fictional fixtures

## 2. r-commands verification batch and push

- [ ] 2.1 Lab tests: rlogin with `.rhosts` (privileged port), rlogin password fallback for `dave`, window size after resize (`stty size`), rsh command output and exit, rexec with password and wrong password, unprivileged source port refused with the explanatory error, toggle off blocks open/quick connect/import/agent open; add names to `lab/expected-tests.txt`; verify `cargo xtask lab test` green
- [ ] 2.2 UI tests: `rlogin://`/`rsh://` parsing with and without `cmd`, invalid input, disabled toggle message; `cargo xtask prints` for the Rsh page and tab in both themes; verify green and `git status` shows no `docs/prints/` files
- [ ] 2.3 Full gates in Docker, `cargo xtask guard`, then commit and push with the task ids; verify the commit is on the remote
