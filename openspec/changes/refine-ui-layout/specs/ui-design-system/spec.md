## Purpose

Shared visual system of every rustXterm screen: color palettes, spacing, borders, typography, interaction states, fonts, HiDPI icons and interface scale, so all screens share one consistent look in Light and Dark with measurable values.

## ADDED Requirements

### Requirement: Neutral color palettes
The application SHALL define Light and Dark palettes with the token values below (hex sRGB) and SHALL draw every screen only from these tokens. Neutral tokens (all except accent, focus, selection in Light, hover in Light, pressed in Light, danger, warning, success) SHALL be grey: for each, the largest difference between its R, G and B channels SHALL be at most 4.

| Token | Light | Dark |
|---|---|---|
| surface (content area, tree, inputs background in Light) | `#ffffff` | `#1e1e1e` |
| panel (dialogs, sidebar strip, status bar, group interiors) | `#f0f0f0` | `#2b2b2b` |
| toolbar (menu bar, toolbar, tab strip background) | `#ffffff` | `#1e1e1e` |
| input (text fields, combos, spin boxes) | `#ffffff` | `#121212` |
| text | `#000000` | `#f0f0f0` |
| text_muted | `#5a5a5a` | `#a0a0a0` |
| text_disabled | `#a0a0a0` | `#6b6b6b` |
| border | `#adadad` | `#5a5a5a` |
| border_subtle (grid lines, separators) | `#dadada` | `#3c3c3c` |
| hover | `#e5f1fb` | `#3a3a3a` |
| pressed | `#cce4f7` | `#505050` |
| selection (selected row, chosen tile) | `#0078d4` | `#4a4a4a` |
| selection_text | `#ffffff` | `#ffffff` |
| accent (links, active-tab marker, primary emphasis) | `#0078d4` | `#3794ff` |
| focus (keyboard focus ring) | `#0078d4` | `#3794ff` |
| tab_active | `#ffffff` | `#1e1e1e` |
| tab_inactive | `#f0f0f0` | `#2b2b2b` |
| danger | `#c42b1c` | `#f1707a` |
| warning | `#9d5d00` | `#fce100` |
| success | `#107c10` | `#6ccb5f` |

#### Scenario: Dark palette has no blue tint
- **WHEN** the Dark palette is checked by the token unit test
- **THEN** every neutral token has a max channel difference of at most 4 and no token equals the previous blue-slate `#232833`

#### Scenario: Rendered screen uses tokens
- **WHEN** the main window is rendered empty at 1280×800 in Dark
- **THEN** a pixel sampled in the middle of the content area is `#1e1e1e` and a pixel in the middle of the status bar is `#2b2b2b`, each within ±2 per channel

### Requirement: Contrast minimums
Text and focus colors SHALL meet WCAG 2.x contrast ratios in both palettes: text on surface, panel, toolbar and input at least 7:1; text_muted on the same backgrounds at least 4.5:1; selection_text on selection at least 4.5:1; focus against surface and panel at least 3:1. text_disabled is exempt.

#### Scenario: Contrast test
- **WHEN** the token unit test computes the ratios for both palettes
- **THEN** every listed pair meets its minimum

### Requirement: Spacing, borders and corners
Layout SHALL use only the spacing scale 2, 4, 6, 8, 12, 16 and 24 logical pixels. Borders and separators SHALL be 1 logical pixel. Every corner radius (windows, dialogs, menus, tooltips, buttons, inputs, tabs, group boxes, scroll bars) SHALL be 0. Windows, menus and popups SHALL have no drop shadow larger than 4 logical pixels of blur and no shadow at all in snapshots. Standard control height SHALL be 22 logical pixels; dialog buttons SHALL be 30 high and at least 80 wide.

#### Scenario: Square corners
- **WHEN** the visuals built from either palette are inspected by unit test
- **THEN** every widget state, window and menu corner radius is 0 and every stroke width is 1

#### Scenario: Control height
- **WHEN** a text field, combo box and checkbox row are rendered in a dialog at scale 1.0
- **THEN** each measures 22 ± 1 logical pixels high

### Requirement: Typography scale
The UI SHALL use the bundled UI font (Noto Sans) in five roles: caption 11 px (toolbar captions, status bar, tooltips), body 12 px (labels, fields, menus, tree, tabs), strong 12 px bold (group box titles, menu bar items in focus), heading 15 px (dialog headings) and display 32 px (Home title), all in logical pixels at scale 1.0, with line height 1.25× the size. No other sizes SHALL appear outside the terminal and remote views.

#### Scenario: Roles only
- **WHEN** the kittest harness renders every screen and the test collects the font sizes of all laid-out UI text
- **THEN** the set is a subset of {11, 12, 15, 32} logical pixels

### Requirement: Bundled fonts
The application SHALL embed Noto Sans (Regular and Bold) as the UI font and Cascadia Mono as the default terminal font, both under the SIL Open Font License 1.1, with their license texts in the repository and one `ATTRIBUTION.md` row each. The UI SHALL render identically on hosts with no fonts installed. Characters missing from a bundled font SHALL fall back to the toolkit's built-in fallback fonts, never to a blank box when the fallback covers them.

#### Scenario: No system fonts
- **WHEN** the application runs in a container with no font packages installed
- **THEN** menus, dialogs and the terminal render with the bundled fonts

#### Scenario: Attribution guard
- **WHEN** a font file under `assets/fonts/` has no `ATTRIBUTION.md` row
- **THEN** `cargo xtask guard` fails naming the file

### Requirement: Interaction states and focus ring
Every interactive widget SHALL show distinct idle, hover, pressed, focused and disabled states drawn from the tokens: hover fills with hover and a 1 px border (accent in Light, border in Dark), pressed fills with pressed, disabled draws text_disabled with no hover response, and keyboard focus draws a 1 px focus-colored rectangle 1 px outside the widget bounds. The focus ring SHALL appear only after keyboard navigation and SHALL never be clipped by its parent.

#### Scenario: Keyboard focus visible
- **WHEN** the user presses Tab in the session editor until the Port field is focused
- **THEN** a 1 px focus-colored rectangle surrounds the Port field and no other widget shows one

#### Scenario: Disabled button
- **WHEN** a dialog's OK button is disabled and the pointer hovers it
- **THEN** its label uses text_disabled and its fill does not change

### Requirement: HiDPI icons
Icons SHALL be drawn at logical sizes 16 (tree, menus, tabs, small buttons), 24 (toolbar, sidebar strip, settings tabs), 32 (session-type row) and 48 (large protocol icon). For each draw, the bitmap used SHALL be the smallest size available for that icon in {16, 32, 48, 64, 96} that is at least logical size × scale factor, or the largest available size if none is; icon override directories SHALL be searched with the same size first.

#### Scenario: 2x toolbar
- **WHEN** the scale factor is 2.0 and the toolbar draws a 24 px icon
- **THEN** the 48 px bitmap is used

#### Scenario: 1.25x session row
- **WHEN** the scale factor is 1.25 and the session-type row draws a 32 px icon
- **THEN** the 48 px bitmap is used

### Requirement: Interface scale
The user SHALL choose the interface size Auto, 100 %, 125 %, 150 % or 200 % in Settings; Auto SHALL use the scale factor reported by the windowing system. The change SHALL apply immediately without restart, and layouts SHALL have no clipped or overlapping controls at any of these scales in a 1280×800 logical window at 100 %, or the equivalent physical size at higher scales.

#### Scenario: Switch to 150 %
- **WHEN** the user selects 150 % in Settings › Display
- **THEN** all text and icons grow by 1.5× within the same frame cycle, and icons use the 48 px bitmaps for 24 px icons

### Requirement: Animation budget
UI animations SHALL be off or last at most 120 ms, SHALL never delay input handling, and SHALL be disabled entirely in snapshot and print rendering. Terminal cursor blink is not an animation for this rule.

#### Scenario: Collapse animation
- **WHEN** the user collapses a folder in the session tree
- **THEN** the collapse completes in at most 120 ms

#### Scenario: Deterministic snapshots
- **WHEN** a snapshot test renders a screen right after a state change
- **THEN** the image shows the final state, not an intermediate frame
