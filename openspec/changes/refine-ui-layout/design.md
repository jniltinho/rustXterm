## Context

See proposal.md (Why). All screens of `create-native-rust-linux-terminal` exist in `crates/ui` and render offscreen through kittest; `cargo xtask prints` copies those snapshots to `docs/prints/rustxterm/` (today 1180×720, scale 1.0). Theme tokens live in `rxt_core::Tokens` (colors only) and are mapped to `egui::Visuals` in `crates/ui/src/theme.rs`, which already forces square corners. Icons are embedded at 16 and 32 px only (`crates/ui/build.rs`) although the catalog ships 48/64/96 px. Menu-like actions are `CommandId`s (`crates/core/src/command.rs`, 20 ids) dispatched through `ui::app::Action::Run`.

Review notes are local-only (`docs/prints/review-notes.md`, 17 numbered layout issues). The target sizes below are our own layout decisions for a 1280×800 window; the Home theme thumbnails are painted from our tokens.

## Goals / Non-Goals

**Goals:**
- One token source drives every pixel (colors, spacing, sizes, fonts), testable by unit tests.
- Each review-notes item maps to a spec scenario with a numeric tolerance.
- Snapshots stay deterministic; 2× renders stay outside Git and CI.

**Non-Goals:**
- Pixel-exact layouts. Targets are band heights, ordering, density and palette; tolerances absorb font differences.
- New behavior in protocols, storage or the engine.

## Decisions

### R1. Tokens become a design system in `core`
Extend `rxt_core::Tokens` with the new color fields (input, text_disabled, border_subtle, pressed, selection_text, focus) and add `Metrics` (spacing scale, control/dialog-button heights, band heights, icon sizes) and `TypeScale` (caption 11, body 12, strong 12 bold, heading 15, display 32). Values per the `ui-design-system` spec. `theme.rs` maps them to `egui::Style` (text styles, spacing, interaction sizes, `animation_time = 0.1`, zero shadows in tests, `CornerRadius::ZERO`). Alternative: keep sizes as literals in each screen; rejected, it is what produced today's drift.
Palette choice: Light follows a classic desktop look (white chrome, `#f0f0f0` dialogs, `#0078d4` selection); Dark is pure neutral grey (`#1e1e1e` chrome/content, `#2b2b2b` panels, grey selection) with a brighter accent `#3794ff` only for focus/links so focus stays visible (≥ 3:1).

### R2. Bundled open fonts
UI: **Noto Sans** Regular + Bold (OFL 1.1) — metrics close to Segoe UI at 12 px, wide Latin/Greek/Cyrillic coverage. Terminal default: **JetBrains Mono** Regular + Bold (OFL 1.1) — clear at 13 px, complete box-drawing set, ligatures disabled. egui's built-in fonts stay as fallbacks (emoji, symbols). Files go to `assets/fonts/` with `OFL.txt` per family and `ATTRIBUTION.md` rows; `cargo xtask guard` treats `assets/fonts/**` like icons (row required). Alternatives: Inter (no Cyrillic in some cuts, taller x-height than Segoe), DejaVu Sans Mono (wider cells, looks dated vs Consolas). CJK is not bundled (size); missing glyphs use egui fallback.

### R3. HiDPI icons
`build.rs` embeds every size present per icon (16/32/48/64/96) instead of only 16/32; `Icons::image(name, logical_px)` picks the bitmap by `logical_px × ctx.pixels_per_point()` with the spec rule and caches per (name, size). Override directory lookup tries `<dir>/<size>/<name>.png` with the chosen size first. Cost: about +2–4 MiB binary. Alternative: load larger sizes from disk at runtime; rejected (AppImage must stay self-contained).

### R4. Menu bar and command ids
`egui::MenuBar` in a top panel above the toolbar. Every item is `Action::Run(CommandId)`. New ids: `session.import`, `session.export`, `app.about`, `vault.change_password`, `view.theme_light`, `view.theme_dark`, `view.theme_system`, `app.focus_menu` (default Ctrl+Shift+F10). The menu never takes Alt/F10 while a terminal or remote view has focus (D6 of the base change: shortcuts must not steal terminal input). `CommandId::ALL` and its uniqueness test grow accordingly; agent-control permission tables are unchanged because these are UI-only commands (marked not agent-callable).

### R5. Shell chrome layout in egui
Panels in order: `TopBottomPanel::top(menu)` 20 px, `top(toolbar)` 52/34 px, `top(row)` 26 px split at the sidebar width (quick connect | tab strip), `bottom(status)` 22 px, `SidePanel::left(sidebar)` with an inner fixed 34 px strip, `CentralPanel` with `Frame::NONE` (inner margin 0) for the work area. Toolbar uses a right-to-left layout for Exit. Sidebar collapse moves quick connect to the start of the tab row. Layout state (width, visible, page) already persists via settings; add `sidebar_page`.

### R6. Tabs
Custom painter in `tabs.rs`: Home tab is a fixed 36 px icon button outside the reorderable list; session tabs draw icon, optional `N. ` prefix, ellipsized title and an inner close button hit-tested before the tab body; "+" follows the last tab. Drag shows a 2 px accent insertion line. Settings gain `show_tab_numbers` and `show_close_buttons` (serde defaults `true`). Rounded tabs are a non-goal (square-corner decision). "+" opens the session editor; making it open a local terminal is left to the local-shell change, which will modify this requirement.

### R7. Session editor
Fixed-size `egui::Modal`/window 900×600 with an inner grid layout: tile row, Basic group (custom `group_box(title, icon)` helper also used by settings and tunnels), icon tab row, panel `Grid` with two columns plus a right-hand 64×64 tile holding the 48 px protocol icon, centered OK/Cancel with `ok`/`cancel` catalog icons. Session name/folder/icon move into the Bookmark tab; Agent settings stays as the last tab (rustXterm-specific). Empty state when opened without a type. Existing validation logic is reused unchanged. Network tab keeps the inline jump-host editor inside a "SSH gateway (jump hosts)" group; a "Proxy settings" group is not added because sessions have no proxy model (no new behavior in this change).

### R8. Settings and tunnels windows
Settings becomes a fixed 800×560 window with icon tabs (General: `gear`, Terminal: `terminal`, SSH: `ssh`, Display: `monitor`, Shortcuts: `keyboard`, Security: `lock`, Agent control: `rocket`), groups via the R7 helper, a single centered OK. Live apply already exists; OK only closes. Security keeps the existing vault page (master password, keyring) and Agent control the existing agent-control page; both are restyled, not changed. Interface size maps to `ctx.set_zoom_factor` (Auto = 1.0 × native `pixels_per_point`). Tunnels window uses `egui_extras::TableBuilder` with fixed header 34 px, `play`/`stop` icons, `edit`/`delete`, `arrow-up-gray`/`arrow-down` for order, and the bottom row; Start all/Stop all are loops over existing per-tunnel commands.

### R9. States, accessibility, tooltips, context menus
A small `ui::states` module with `empty(ui, icon, text, action)`, `loading(ui, text, since)` (200 ms delay) and `error(ui, op, cause, action)` used by every screen. Every icon-only control goes through one `icon_button(name, label, shortcut)` helper that sets the tooltip (`on_hover_text`, 500 ms delay via `style.interaction.tooltip_delay`) and the AccessKit label from the same string — one helper makes the name audit trivially true. Focus ring: custom 1 px rect painted on `response.has_focus()` when the last input was keyboard (egui tracks this). Context menus reuse `response.context_menu` and the Menu/Shift+F10 key handler opens the same closure for the focused element.

### R10. Window state
eframe `persist_window = true` (size, maximized, fullscreen; position on X11 — Wayland ignores positioning, which the spec allows) plus `ViewportBuilder::with_min_inner_size([800, 500])`. Clamp on start against `ctx.input(|i| i.viewport().monitor_size)`.

### R11. Verification pipeline
- Kittest harness size becomes 1280×800 (`tests.rs`), animations off (`animation_time = 0`), shadows off; all snapshots regenerated in one batch (`UPDATE_SNAPSHOTS=1`) after review.
- `cargo xtask prints` runs the snapshot tests, then runs them again with `RXT_PRINTS_SCALE=2` (harness `pixels_per_point = 2.0`, output to a temp dir, never compared or committed) and copies both sets into `docs/prints/rustxterm/` (`<screen>_<theme>@2x.png`).
- `cargo xtask prints` generates `docs/prints/review-checklist.md` with one row per print, merging existing check marks.
- Manual gate: the reviewer ticks every checklist row before the milestone commit.

### R12. Parallel ownership
Batch 1 (tokens/fonts/icons: `core::theme`, `ui/theme.rs`, `ui/icons.rs`, `ui/build.rs`, `assets/fonts`) lands first because everything else consumes it. Then disjoint agents: shell chrome (`app.rs` panels, menu, toolbar, status), sidebar+tabs (`sidebar.rs`, `tabs.rs`), editor (`session_editor.rs`), settings+tunnels (`settings.rs`, `tunnels.rs`), content views (`desktop.rs`, `file_tab.rs`, `sftp_panel.rs`, `dialogs.rs`). `tests.rs` snapshot names are touched by one integrator at the verification batch to avoid conflicts. Each agent uses `CARGO_TARGET_DIR=target/agent-<name>`.

## Risks / Trade-offs

- [Font metrics differ from Segoe UI, so band heights drift] → tolerances in specs (±1–2 px) and band heights fixed by tokens, not by text height.
- [Snapshot churn makes review hard] → regenerate all snapshots once in the verification batch; review via prints in both themes.
- [Larger binary from fonts and HiDPI icons (~+5 MiB)] → acceptable for an AppImage; fonts limited to two weights each.
- [Icon licensing] → the bundled icon set in `assets/icons/app/` is original and listed in `ATTRIBUTION.md`; `RXT_ICON_OVERRIDE_DIR` swaps icons.
- [Wayland ignores window position] → spec only requires position "where the display server allows it".
- [Alt-key menu access conflicts with terminal Meta] → no Alt mnemonics while a terminal has focus; explicit `app.focus_menu` shortcut.

## Migration Plan

Settings gain fields with serde defaults (`sidebar_page`, `show_tab_numbers`, `show_close_buttons`, `toolbar_captions`, `ui_scale`, `first_run_theme_done`); old files load unchanged. Existing users are not shown the first-run theme chooser (marked done when a settings file already exists). Rollback is reverting the milestone commit; no data migration.
