# Tasks

## 1. Scheme catalog

- [x] 1.1 Add `TerminalScheme::builtin()` in `crates/core/src/theme.rs` with the 13 schemes named in the spec; unit test: names unique, list matches the spec, every scheme round-trips through JSON
- [x] 1.2 Add a palette row to `ATTRIBUTION.md` (published palette values, their authors and licenses); `cargo xtask guard` passes

## 2. User schemes

- [x] 2.1 `rxt_store::schemes::load(dir, taken)` reads `*.json` in file-name order, skipping files over 64 KiB, invalid JSON and taken names with a warning; test with a temp dir holding a valid file, an invalid file, an oversized file, a file named like a built-in and two files sharing a name
- [x] 2.2 Process-wide catalog in `crates/ui/src/theme.rs` set from `App::with_settings` (folder next to `config.toml`); `theme::scheme()` looks names up in it; test: unknown name and `follow-ui` fall back to the UI-theme scheme

## 3. Picker

- [x] 3.1 `scheme_combo` lists "Follow UI theme" then the catalog, each row with a color swatch; Settings uses it for the global value
- [x] 3.2 Session editor override maps "Follow UI theme" to `follow-ui` instead of dropping it; test: global Dracula + session `follow-ui` resolves to the UI-theme scheme and survives save/reopen
- [x] 3.3 `cargo xtask prints` still writes every screen in light and dark themes

## 4. Font zoom

- [x] 4.1 Ctrl+wheel and Ctrl+= / Ctrl++ / Ctrl+- / Ctrl+0 change the font size, saved to the session override or the global prefs; tests `ctrl_wheel_zooms_font_without_scrolling` and `ctrl_plus_minus_zero_zoom_font`
- [x] 4.2 Step by exactly one point (no rounding); widget test from 13.5 to 14.5
- [x] 4.3 Zoom of a session override updates every open tab of that session; app test with two tabs
- [x] 4.4 Ctrl+_ (Ctrl+Shift+-) and Ctrl+/ send 0x1f; widget test asserts the bytes
- [x] 4.5 Banner hint mentions the keyboard zoom keys; banner tests updated

## 5. Verification

- [x] 5.1 fmt, clippy and the workspace tests green in Docker; `cargo xtask guard` and `cargo xtask layers` pass
