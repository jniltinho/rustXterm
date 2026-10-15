# Design

## Context

`TerminalScheme` (`crates/core/src/theme.rs`) already holds name, foreground, background, cursor, selection and 16 ANSI colors and is `Serialize`/`Deserialize`. `TerminalPrefs.scheme` stores the scheme by name. In the global prefs `None` means follow the UI theme; in a session `None` means inherit the global value (`TerminalPrefs::resolve`), and the session editor currently drops the picker's "Follow UI theme" choice. `ui::theme::scheme()` resolves the name but only knows the two rustXterm schemes, and `settings::scheme_combo` lists only those two.

## Goals / Non-Goals

**Goals:**
- One scheme catalog shared by the picker and the lookup.
- User schemes without a new file format: the existing `TerminalScheme` JSON.

**Non-Goals:**
- An in-app color editor.
- Importing .itermcolors, Windows Terminal JSON or base16 files (users can convert them to the JSON format; add an importer when asked).
- Hot reload of the `schemes` folder (load at start only).

## Decisions

- **Built-in schemes as `const`-like constructors in `rxt-core`**: a `TerminalScheme::builtin() -> Vec<TerminalScheme>` built from hex tables. Keeps core free of I/O and the palettes testable. Alternative: bundled JSON assets; rejected, it adds files that need `ATTRIBUTION.md` rows and runtime parsing for fixed data. Palettes are factual color values from each scheme's published definition; record the source and license of each in `ATTRIBUTION.md` (all are MIT or public domain).
- **User schemes loaded by `rxt-store`** from `<config dir>/schemes/*.json` at start (the folder next to `config.toml`), each file one `TerminalScheme`, read in file-name order. Files over 64 KiB, invalid JSON, and names already taken (built-in first, then earlier user files) are skipped with `tracing::warn!`.
- **Catalog is process-wide in `rxt-ui`** (`OnceLock<Vec<TerminalScheme>>`, built-ins plus user schemes, set once in `App::with_settings`; built-ins only when unset, e.g. in tests). This avoids threading the catalog through the Settings window and session editor signatures; it fits "load at start only". Lookup is a linear scan by name. Unknown name → `TerminalScheme::for_dark(dark)`; the stored name is not rewritten.
- **Per-session "Follow UI theme"** is stored as the reserved name `follow-ui`. It is never a catalog entry, so the lookup's unknown-name fallback already follows the UI theme; the pickers map it to the "Follow UI theme" entry. Global `None` keeps its meaning, so existing settings files need no migration.
- **Swatch preview**: each combo row paints a small rectangle with the background, then the foreground and the 8 normal ANSI colors as squares, using the existing theme tokens for the border. No new widget type.
- **Font zoom**: the terminal view returns the new size (current + step, no rounding, clamped to 6..=48) in `ViewResponse::font_size`; the app writes it to the global terminal prefs, or, when the session overrides the font size, to the stored session and every open tab of that session. Settings are written by the existing `save()`, which only writes when settings changed (every 30 s and on exit). Zoom keys follow GNOME Terminal (Ctrl+= / Ctrl++ / Ctrl+- / Ctrl+0) and are fixed, like Ctrl+wheel; Ctrl+- stops sending 0x1f; Ctrl+/ and Ctrl+_ (Ctrl+Shift+-, which `key_bytes` currently drops) send it.

## Risks / Trade-offs

- [Some built-in schemes have low contrast for some ANSI pairs] → ship the published palettes unchanged; users pick another scheme.
- [Ctrl+- no longer reaches the remote as 0x1f] → Ctrl+_ and Ctrl+/ send the same byte; documented in the hint.
- [A user scheme file is large or malformed] → skip files over 64 KiB and any parse error.
