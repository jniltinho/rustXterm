## Why

rustXterm v0.1 (`create-native-rust-linux-terminal`) works, but a review of its prints (local-only `docs/prints/review-notes.md`, 17 numbered issues) shows it does not yet look or feel like a polished desktop tool: blue-slate dark theme, oversized toolbar, no menu bar, quick connect in the wrong place, horizontal sidebar tabs, text Home tab, stacked session editor fields, single-page settings and inset terminal. This change gives the UI a measurable visual and interaction finish ("AAA+") before more protocols (Telnet, Serial, ...) multiply the screens that would need rework.

## What Changes

- **Visual system**: theme tokens grow from colors only to a full design system: neutral Light (`#f0f0f0`/white family, Windows-blue accent) and neutral Dark (`#1e1e1e`–`#2b2b2b`, no blue tint) palettes, a 2/4/6/8/12/16/24 px spacing scale, 1 px borders, square corners (radius 0 everywhere), a typography scale, focus ring, hover/pressed/disabled states and contrast minimums, checked by unit tests.
- **Bundled fonts**: Noto Sans (UI) and JetBrains Mono (terminal default), both SIL OFL 1.1, under `assets/fonts/` with license files and `ATTRIBUTION.md` rows, so every Linux desktop renders the same metrics.
- **HiDPI icons**: icons pick the 16/32/48/64/96 px bitmap from the catalog that fits `logical size × scale factor`, so 1.25×, 1.5× and 2× stay sharp.
- **Shell chrome**: classic menu bar (Terminal, Sessions, View, Tools, Settings, Help) whose items are existing or new command ids; compact 24 px-icon toolbar with captions and Exit pinned right; quick connect at the top of the sidebar column; a vertical icon strip for sidebar tabs (Sessions, SFTP); "User sessions" root row with solid selection; add/filter moved to a bottom row and context menus; thin status bar.
- **Tabs**: icon-only pinned Home tab, close button inside each tab, optional tab numbers, "+" new-tab button, edge-to-edge terminal and remote-desktop content.
- **Home page**: logo and title, two wide action buttons, first-run Light/Dark chooser with drawn thumbnails, gear to settings, recent sessions only when there are any.
- **Session editor**: fixed 900×600 dialog, protocol icon row, "Basic <protocol> settings" group with host/user/port on one row, icon sub-tabs (Advanced, Terminal, Network, Bookmark), two-column grids with a large protocol icon, centered OK/Cancel with icons, "choose a session type" empty state; session name/folder/icon move to the Bookmark tab.
- **Settings window**: icon tabs General, Terminal, SSH, Display, Shortcuts, plus the existing Security and Agent control pages; Display holds the Skin dropdown (Light/Dark/System), interface size, toolbar captions toggle and tab options; single centered OK; changes apply live.
- **Tunneling window**: bordered grid with play/stop icon buttons per row, bottom action row (New tunnel, Start all, Stop all … Close), Local/Remote/Dynamic radios at the top of the editor.
- **Interaction polish**: animation budget (none, or ≤ 120 ms), full keyboard navigation, accessibility names on every interactive widget, context menus, drag affordances, tooltips on every icon-only control, empty/loading/error states for every screen, one dialog convention, window state persistence and interface scale 1.0/1.25/1.5/2.0.
- **Verification**: snapshots and `cargo xtask prints` at 1280×800 in both themes; prints also at 2× scale; a local review checklist per screen; a manual review gate before merge.
- **BREAKING** (UI only, no data change): the Home tab loses its text label, "Session name"/"Folder" move to the Bookmark tab of the editor, the settings page becomes a tabbed window. Stored settings and sessions load unchanged.

### Non-goals

Rounded/trapezoid tabs (square corners are a user decision), tab detach (paperclip), split panes, remote-monitoring status chips, Macros/Tools sidebar tabs, X server, Games, Packages, Servers, sidebar on the right, transparency and color-saturation sliders, an original replacement icon set (still required before public release, tracked by `ATTRIBUTION.md`).

## Capabilities

### New Capabilities

- `ui-design-system`: theme tokens (palettes, spacing, borders, radius, typography, states, contrast), bundled UI and terminal fonts, HiDPI icon selection, interface scale, animation budget, focus visibility.

### Modified Capabilities

- `desktop-shell`: main window layout (menu bar, toolbar, quick connect placement, sidebar strip, status bar), tabs (Home, close-inside, numbers, "+"), Home page and first-run theme chooser, settings window, tunneling window, keyboard navigation, context menus, tooltips, empty/loading/error states, dialog convention, window state persistence, review prints and review gate.
- `session-management`: session tree presentation (root row, selection, bottom action row) and session editor layout.
- `terminal-emulation`: edge-to-edge terminal area and bundled default terminal font.
- `file-transfer`: SFTP panel and dual-pane file tab presentation (compact rows, icon toolbars, drag affordances, empty/error states).
- `remote-desktop`: edge-to-edge framebuffer and status overlays for connecting/error states.

## Impact

- Depends on `create-native-rust-linux-terminal` (all screens exist; this change only restyles and rearranges them). Should land before `add-telnet-sessions`, `add-serial-sessions`, `add-rsh-sessions` and `add-xdmcp-sessions` add editor pages, so they inherit the new editor layout.
- Code: `crates/core` (`Tokens` gain spacing/typography/state fields, new `CommandId`s for menu items), `crates/ui` (`theme.rs`, `app.rs`, `tabs.rs`, `sidebar.rs`, `session_editor.rs`, `settings.rs`, `tunnels.rs`, `dialogs.rs`, `desktop.rs`, `file_tab.rs`, `sftp_panel.rs`, `icons.rs`, `build.rs`, `tests.rs`), `xtask` (`prints` sizes, 2× pass and review checklist, `guard` covers fonts).
- Assets: `assets/fonts/` (Noto Sans Regular/Bold, JetBrains Mono Regular/Bold, OFL texts); `ATTRIBUTION.md` rows; icons now also embed the 48/64/96 px catalog sizes (larger binary, about +2–4 MiB).
- Snapshots: every kittest snapshot is regenerated (new size 1280×800 and new look).
- No new runtime crates beyond what egui/eframe already provide; no protocol, storage or engine behavior changes.
