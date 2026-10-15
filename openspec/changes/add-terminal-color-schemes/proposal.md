# Proposal

## Why

The terminal ships only two color schemes (`rustxterm-light` and `rustxterm-dark`), so users cannot pick the palettes they use in other terminals (Solarized, Dracula, Gruvbox...). Font zoom was also per-tab and lost on close; it now persists, and its shortcuts need to be part of the spec.

## What Changes

- Add a set of built-in terminal color schemes: Solarized Dark, Solarized Light, Dracula, Gruvbox Dark, Gruvbox Light, Nord, One Dark, Monokai, Tango Dark, Tango Light and xterm, next to the two existing rustXterm schemes.
- The scheme picker (Settings and session editor) lists every scheme with a color swatch preview; "Follow UI theme" stays the default.
- Users can add their own schemes by dropping JSON files into `<config dir>/schemes/`; they appear in the picker after a restart.
- A session or global setting naming a scheme that no longer exists falls back to "Follow UI theme".
- Font zoom: Ctrl+wheel, Ctrl+= / Ctrl++, Ctrl+- and Ctrl+0 (reset to the default size) change the terminal font size; the size is saved (to the session when it overrides the font size, otherwise globally) and restored on the next start. Already implemented; this change records it in the spec.

## Capabilities

### New Capabilities

### Modified Capabilities
- `terminal-emulation`: "Fonts and appearance" gains built-in and user color schemes, the scheme preview, the unknown-scheme fallback and the persisted font zoom shortcuts.

## Impact

- `crates/core/src/theme.rs`: scheme catalog.
- `crates/ui/src/theme.rs` (`scheme` lookup), `crates/ui/src/settings.rs` (`scheme_combo`), `crates/ui/src/session_editor.rs`.
- `crates/store`: reading `schemes/*.json` from the config dir.
- `cargo xtask prints`: Settings screen with the scheme picker open, light and dark.
- No new dependencies; settings files stay compatible (scheme is already stored by name).
