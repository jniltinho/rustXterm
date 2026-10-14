Depends on `create-native-rust-linux-terminal` being implemented (engine transport seam, terminal widget, encrypted store, lab).

## 1. Telnet milestone (code)

- [x] 1.1 Create `crates/telnet` (L2) and register it in `cargo xtask layers`; verify the layer check passes and fails on a deliberate `ui` dependency
- [x] 1.2 Sans-IO codec per design T1–T3 (Q-method option table, TTYPE/NAWS subnegotiation, IAC escaping, NVT newline modes, 1 KiB SB cap, toggle limit); verify unit tests with byte fixtures including option ping-pong, oversized SB, IAC IAC, NAWS values containing 0xFF, CR NUL
- [x] 1.3 tokio driver with raw TCP mode (T4), backpressure through the session output buffer and cancellation on drop; verify unit tests over an in-process TCP pair
- [x] 1.4 Auto-login state machine (T5): ordered prompts, once-only, 15 s timeout, password from the store only, masking in session logs; verify unit tests for match, no-match timeout, out-of-order prompts and log masking
- [x] 1.5 `core`: `Protocol::Telnet`, `TelnetOptions` with serde defaults; `engine`: Telnet branch in `Real::connect` returning a shell-only `Link`; verify serde round-trip, loading of pre-change session data and engine unit tests with the in-memory transport (MultiExec includes Telnet tabs)
- [x] 1.6 `ui`: type picker, Telnet editor page with plaintext warning, quick connect `telnet://`, unencrypted tab marker and first-connect notice, session icon (from `assets/icons/app/`, or fall back to `session`); verify kittest snapshots in light and dark
- [ ] 1.7 `store`: MobaXterm.ini Telnet import after confirming type code `1` and field positions with sample files (fixtures hand-written with fictional data); verify importer unit tests
- [x] 1.8 Lab: `telnetd` service and raw TCP echo service on 127.0.0.1 (design T8), hostile Telnet peer in `xtask`; verify `lab/lab.sh up` reports them healthy (hostile peer lives in `crates/telnet/tests/driver.rs` as in-process tests)

## 2. Telnet verification batch and push

- [x] 2.1 Lab tests: login with manual typing, auto-login opt-in success and opt-out (nothing typed), NAWS after resize (`stty size` on the server), TTYPE (`echo $TERM`), raw mode echo, hostile peer stays bounded, reconnect after server restart; add names to `lab/expected-tests.txt`; verify `cargo xtask lab test` green
- [x] 2.2 UI tests: quick-connect parsing of `telnet://` with and without user/port and invalid input, unencrypted marker, MultiExec broadcast to a Telnet tab, session log of a Telnet tab; verify green
- [x] 2.3 `cargo xtask prints` for the Telnet editor page and a Telnet tab in both themes; verify files exist under `docs/prints/` and `git status` shows none of them
- [x] 2.4 Full gates (`fmt`, `clippy -D warnings`, unit, snapshots, layers) in Docker, `cargo xtask guard`, then commit and push with the task ids; verify the commit is on the remote
