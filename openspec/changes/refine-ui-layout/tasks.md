Depends on `create-native-rust-linux-terminal` (all screens exist). Lands before the Telnet/Serial/Rsh/XDMCP changes add editor pages. Code batches 1–6 first (batch 1 before the others; batches 2–6 in parallel by disjoint files, design R12, `CARGO_TARGET_DIR=target/agent-<name>`), then one verification batch. Builds, lints and tests run only in Docker. Prints stay in `docs/prints/` (local-only).

## 1. Tokens, fonts and icons (agent `ui-tokens`)

- [x] 1.1 `core::theme`: add color tokens (input, text_disabled, border_subtle, pressed, selection_text, focus), set Light/Dark values from the `ui-design-system` palette table, add `Metrics` (spacing scale, control 22, dialog button 30×80, band heights 20/52/34/24/20, sidebar 240/strip 34, icon sizes 16/24/32/48) and `TypeScale` (11/12/12b/15/32); verify unit tests for neutral greys (max channel diff ≤ 4), WCAG contrast pairs and that no token equals `#232833`
- [x] 1.2 `ui/theme.rs`: map tokens to `egui::Style` (text styles, spacing, interaction sizes, 1 px strokes, zero radius, no shadows, `animation_time` 0.1 s, 0 in tests, tooltip delay 0.5 s, hover/pressed/disabled visuals) and paint the 1 px focus ring for keyboard focus; verify unit tests on visuals for both themes (radius 0, stroke 1, colors)
- [x] 1.3 Add Noto Sans Regular/Bold and JetBrains Mono Regular/Bold with `OFL.txt` under `assets/fonts/`, `ATTRIBUTION.md` rows, register them as UI and default terminal fonts with egui fallbacks; extend `cargo xtask guard` to require rows for `assets/fonts/**`; verify guard fails on a deliberately unlisted font file and a kittest render uses the bundled fonts
- [x] 1.4 `ui/build.rs` + `icons.rs`: embed every available size (16/32/48/64/96), pick the bitmap by `logical × pixels_per_point` with largest-available fallback, override lookup by chosen size first; verify unit tests for 24 px at 1.0/1.5/2.0 and 32 px at 1.25 and for an icon missing the 48 px file
- [x] 1.5 Settings fields with serde defaults (`sidebar_page`, `show_tab_numbers`, `show_close_buttons`, `toolbar_captions`, `ui_scale`, `first_run_theme_done`) and `ui_scale` applied via zoom factor (Auto = native); verify round-trip of an old settings file and a test switching 1.0 → 1.5 in one frame

## 2. Shell chrome (agent `ui-shell`)

- [x] 2.1 New `CommandId`s (`session.import`, `session.export`, `app.about`, `vault.change_password`, `view.theme_light|dark|system`, `app.focus_menu` default Ctrl+Shift+F10), marked not agent-callable; verify the id uniqueness/parse tests and the agent policy table test
- [x] 2.2 Menu bar (Terminal, Sessions, View, Tools, Settings, Help) with shortcuts right-aligned, disabled unavailable items, theme radio items, no Alt/F10 capture while a terminal or remote view has focus; verify UI tests for theme switch via menu, disabled items with no tab, and F10/Alt+T reaching the terminal
- [x] 2.3 Toolbar: order Session, Sessions, Tunneling, MultiExec, Settings, Help | spacer | Exit; 24 px icons with 11 px captions or icon-only (34 px band); toggles use pressed fill; verify UI test that Exit's right edge stays 4 ± 1 px from the window edge at 1280 and 1600 px
- [x] 2.4 Panel layout per design R5 (menu 20, toolbar 52/34, row 24, status 20, zero-margin work area), quick connect in the sidebar column row with placeholder and inline error, moved to the tab row when the sidebar is collapsed; verify UI tests of band rectangles within tolerance and quick-connect placement
- [x] 2.5 Status bar single line with ellipsis; Home page (logo + title, two 200×30 buttons, gear, recent list only when non-empty, first-run Light/Dark chooser with token-painted thumbnails); verify UI tests for first-run choose Dark → saved and hidden, and empty Home showing no recent heading
- [x] 2.6 Window state persistence (size, maximized, fullscreen, position where supported), min 800×500, clamp to a connected monitor; verify unit test of the clamp function and manual restart check recorded in the checklist

## 3. Sidebar and tabs (agent `ui-sidebar-tabs`)

- [x] 3.1 Vertical icon strip (Sessions, SFTP; 34×40 buttons, active joins page, click-active collapses); verify UI tests for page switch, collapse and persisted page
- [x] 3.2 Session tree: "User sessions" root row (`users` icon), 20 px rows, 16 px indent, solid selection token, bottom row with New session/New folder icons and filter, empty and locked states; verify UI tests for filter, selection pixel colors in both themes, empty and locked trees
- [x] 3.3 Tab strip: pinned 36 px house-only Home tab, 24 px square tabs 80–220 px with ellipsis, close button inside, `N. ` numbering and close buttons per settings, "+" running `session.new`, 2 px accent insertion line on drag, Escape cancels drag; verify UI tests for numbering after reorder, close buttons hidden + middle-click close, "+" opening the editor
- [x] 3.4 Context menus for tree rows, folders, empty tree area and tabs with icons, shortcuts and disabled items, reachable by Menu key and Shift+F10; verify UI test duplicating a session from the keyboard

## 4. Session editor (agent `ui-editor`)

- [x] 4.1 Shared `group_box(title, icon)` and `icon_button(name, label, shortcut)` helpers (tooltip + AccessKit name from one string) and `states::{empty, loading, error}` (200 ms loading delay); verify unit/UI tests for the helpers
- [x] 4.2 Editor shell: fixed 900×600 "Session settings" dialog, type tile row (32 px icons, chosen-tile fill per theme), "choose a type" empty state when opened without a type; verify UI tests for empty state and SSH selection giving port 22
- [x] 4.3 "Basic <type> settings" group with host (required), username + credentials picker, port spin box on one row; icon sub-tabs Advanced/Terminal/Network/Bookmark/Agent; name, folder and icon moved to Bookmark; verify UI tests for one-row alignment (centers within 1 px) and Bookmark-only name/folder fields
- [x] 4.4 Two-column grids for every type page (SSH, SFTP, FTP, FTPS, RDP, VNC) with the 64×64 tile holding the 48 px protocol icon; Network tab wraps the inline jump-host editor in a titled group; centered OK/Cancel with `ok`/`cancel` icons, Enter/Escape; verify the invalid-port scenario (danger border, OK disabled) and centered buttons within ±2 px

## 5. Settings and tunnels windows (agent `ui-settings-tunnels`)

- [x] 5.1 Settings window 800×560 with icon tabs General, Terminal, SSH, Display, Shortcuts, Security, Agent control (existing pages restyled), group boxes, single centered OK, live apply, Escape closes, last tab remembered; Display has Skin, Interface size, Toolbar captions, Show tab numbers, Show close buttons; verify UI tests for Skin = System persisting and Escape keeping changes
- [x] 5.2 Tunneling window: resizable 1060×410 (min 760×300), grid with header 34 px and columns Name/Type/Start-stop/Forward port/Destination server/SSH server/Settings/Order, play/stop, edit/delete and up/down icon buttons, bottom row New/Start all/Stop all … Close, empty-grid hint; editor with Local/Remote/Dynamic radios on top and centered Save/Cancel; verify UI tests for Start all with one failing tunnel and the empty grid

## 6. Content views and polish (agent `ui-content`)

- [x] 6.1 Edge-to-edge terminal (no inset, cell grid ≤ 2 px from the corner, leftover painted with terminal background) and default terminal font JetBrains Mono 13 px with fallback on load failure; verify UI test sampling edge pixels in both themes
- [x] 6.2 Remote desktop view edge to edge, `#000000` letterbox, connecting/error states with Reconnect, 3 s grab-hint strip; verify UI tests for letterbox color and hint timing (simulated clock)
- [x] 6.3 SFTP panel and dual-pane file tab presentation: path row, 16 px icon toolbar with tooltips, 22 px header, 20 px rows, sortable columns with arrow, empty/error/loading states, drop-target outline, compact transfers rows; verify UI tests for sort, empty folder, permission-denied error and drop outline
- [x] 6.4 Dialog convention for every dialog in `dialogs.rs` and `vault.rs`/`store_ui.rs` (24 px title row, groups, centered OK/Cancel 80×30 with icons, destructive defaults to Cancel, focus returns to opener); verify UI test for the delete-session confirmation defaulting to Cancel
- [x] 6.5 Keyboard navigation and accessibility pass across all screens (reading order, arrows in lists/trees/tabs/radios, F2/Delete, terminal keeps Tab/arrows/F-keys); verify the "editor by keyboard" and "terminal keeps Tab" UI tests and an AccessKit walk asserting no empty names on every snapshot screen

## 7. Verification batch, prints and push

- [x] 7.1 Kittest harness at 1280×800 with animations and shadows off; regenerate every snapshot in both themes (`UPDATE_SNAPSHOTS=1`) and add snapshots for new screens (menus open, settings Display tab, first-run Home, editor empty state, tunnel editor, loading/error states); verify `cargo test -p rxt-ui` green in Docker with no leftover `.new.png`
- [x] 7.2 Font-size audit test (only 11/12/15/32 px UI text) and band-height test (menu/toolbar/row/status/sidebar within tolerance) over the main window in both themes; verify green
- [x] 7.3 `cargo xtask prints`: 1× prints for every screen in both themes plus `@2x` prints from a second run with `RXT_PRINTS_SCALE=2` (never committed), and `docs/prints/review-checklist.md` with one row per print, preserving existing check marks; verify files exist and `git status` shows nothing under `docs/prints/`
- [x] 7.4 CI runs without `docs/prints/`; verify the test suite passes when the directory is absent
- [ ] 7.5 Manual review gate: walk the checklist (light, dark, 2×) against the 17 items of `review-notes.md`, fix deviations, tick every row; verify every row is checked
- [x] 7.6 Full gates in Docker (`fmt`, `clippy -D warnings`, unit, snapshots, `cargo xtask layers`), `cargo xtask guard`, then commit and push the milestone with the task ids in the message; verify the commit is on the remote and contains no file from `base/` or `docs/prints/`
