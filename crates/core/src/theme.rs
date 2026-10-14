//! Theme tokens shared by the UI and terminal defaults (design D7).

use serde::{Deserialize, Serialize};

/// UI theme selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Light,
    Dark,
    /// Follow the desktop color-scheme; Light when unknown.
    #[default]
    System,
}

impl ThemeMode {
    /// Resolve to a concrete dark flag given the platform preference (`None` = unknown).
    pub fn is_dark(self, system_dark: Option<bool>) -> bool {
        match self {
            Self::Light => false,
            Self::Dark => true,
            Self::System => system_dark.unwrap_or(false),
        }
    }
}

/// sRGB color with alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgba(pub u8, pub u8, pub u8, pub u8);

impl Rgba {
    /// Opaque color from a `0xRRGGBB` literal.
    pub const fn hex(v: u32) -> Self {
        Self((v >> 16) as u8, (v >> 8) as u8, v as u8, 0xff)
    }
}

/// Semantic UI colors (`ui-design-system` palette table). Widgets never hardcode colors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    pub dark: bool,
    /// Content area, tree, inputs background in Light.
    pub surface: Rgba,
    /// Dialogs, sidebar strip, status bar, group interiors.
    pub panel: Rgba,
    /// Menu bar, toolbar, tab strip background.
    pub toolbar: Rgba,
    /// Text fields, combos, spin boxes.
    pub input: Rgba,
    pub text: Rgba,
    pub text_muted: Rgba,
    pub text_disabled: Rgba,
    /// Links, active-tab marker, primary emphasis.
    pub accent: Rgba,
    pub border: Rgba,
    /// Grid lines and separators.
    pub border_subtle: Rgba,
    /// Selected row, chosen tile.
    pub selection: Rgba,
    pub selection_text: Rgba,
    pub hover: Rgba,
    pub pressed: Rgba,
    /// Keyboard focus ring.
    pub focus: Rgba,
    pub danger: Rgba,
    pub warning: Rgba,
    pub success: Rgba,
    pub tab_active: Rgba,
    pub tab_inactive: Rgba,
}

impl Tokens {
    /// Windows 10 classic look: white chrome, `#f0f0f0` dialogs, `#0078d4` selection.
    pub fn light() -> Self {
        Self {
            dark: false,
            surface: Rgba::hex(0xffffff),
            panel: Rgba::hex(0xf0f0f0),
            toolbar: Rgba::hex(0xffffff),
            input: Rgba::hex(0xffffff),
            text: Rgba::hex(0x000000),
            text_muted: Rgba::hex(0x5a5a5a),
            text_disabled: Rgba::hex(0xa0a0a0),
            accent: Rgba::hex(0x0078d4),
            border: Rgba::hex(0xadadad),
            border_subtle: Rgba::hex(0xdadada),
            selection: Rgba::hex(0x0078d4),
            selection_text: Rgba::hex(0xffffff),
            hover: Rgba::hex(0xe5f1fb),
            pressed: Rgba::hex(0xcce4f7),
            focus: Rgba::hex(0x0078d4),
            danger: Rgba::hex(0xc42b1c),
            warning: Rgba::hex(0x9d5d00),
            success: Rgba::hex(0x107c10),
            tab_active: Rgba::hex(0xffffff),
            tab_inactive: Rgba::hex(0xf0f0f0),
        }
    }

    /// Neutral grey, no blue tint; the accent only marks focus and links.
    pub fn dark() -> Self {
        Self {
            dark: true,
            surface: Rgba::hex(0x1e1e1e),
            panel: Rgba::hex(0x2b2b2b),
            toolbar: Rgba::hex(0x1e1e1e),
            input: Rgba::hex(0x121212),
            text: Rgba::hex(0xf0f0f0),
            text_muted: Rgba::hex(0xa0a0a0),
            text_disabled: Rgba::hex(0x6b6b6b),
            accent: Rgba::hex(0x3794ff),
            border: Rgba::hex(0x5a5a5a),
            border_subtle: Rgba::hex(0x3c3c3c),
            selection: Rgba::hex(0x4a4a4a),
            selection_text: Rgba::hex(0xffffff),
            hover: Rgba::hex(0x3a3a3a),
            pressed: Rgba::hex(0x505050),
            focus: Rgba::hex(0x3794ff),
            danger: Rgba::hex(0xf1707a),
            warning: Rgba::hex(0xfce100),
            success: Rgba::hex(0x6ccb5f),
            tab_active: Rgba::hex(0x1e1e1e),
            tab_inactive: Rgba::hex(0x2b2b2b),
        }
    }

    pub fn for_dark(dark: bool) -> Self {
        if dark { Self::dark() } else { Self::light() }
    }
}

/// Theme-independent sizes in logical pixels (`ui-design-system` spacing and desktop-shell bands).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    /// Spacing scale; layouts use only these values.
    pub space: [f32; 7],
    /// Text field, combo box, checkbox row.
    pub control_height: f32,
    /// Dialog button minimum (width, height).
    pub dialog_button: (f32, f32),
    pub menu_height: f32,
    pub toolbar_height: f32,
    /// Toolbar without captions.
    pub toolbar_compact_height: f32,
    /// Quick connect + tab strip row.
    pub row_height: f32,
    pub status_height: f32,
    pub sidebar_width: f32,
    /// Vertical icon strip of the sidebar.
    pub strip_width: f32,
    pub stroke: f32,
    /// Tree, menus, tabs, small buttons.
    pub icon_small: f32,
    /// Toolbar, sidebar strip, settings tabs.
    pub icon_toolbar: f32,
    /// Session-type row.
    pub icon_tile: f32,
    /// Large protocol icon.
    pub icon_large: f32,
}

impl Metrics {
    pub const DEFAULT: Self = Self {
        space: [2.0, 4.0, 6.0, 8.0, 12.0, 16.0, 24.0],
        control_height: 22.0,
        dialog_button: (80.0, 30.0),
        menu_height: 20.0,
        toolbar_height: 52.0,
        toolbar_compact_height: 34.0,
        row_height: 28.0,
        status_height: 18.0,
        sidebar_width: 240.0,
        strip_width: 34.0,
        stroke: 1.0,
        icon_small: 16.0,
        icon_toolbar: 24.0,
        icon_tile: 32.0,
        icon_large: 48.0,
    };
}

impl Default for Metrics {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// UI font sizes in logical pixels at scale 1.0 (bundled Noto Sans).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TypeScale {
    /// Toolbar captions, status bar, tooltips.
    pub caption: f32,
    /// Labels, fields, menus, tree, tabs.
    pub body: f32,
    /// Bold body: group box titles, focused menu bar items.
    pub strong: f32,
    /// Dialog headings.
    pub heading: f32,
    /// Home title.
    pub display: f32,
    /// Line height as a multiple of the size.
    pub line_height: f32,
}

impl TypeScale {
    pub const DEFAULT: Self =
        Self { caption: 11.0, body: 12.0, strong: 12.0, heading: 15.0, display: 32.0, line_height: 1.25 };
}

impl Default for TypeScale {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Interface size chosen in Settings › Display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum UiScale {
    /// Scale factor reported by the windowing system.
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "100")]
    P100,
    #[serde(rename = "125")]
    P125,
    #[serde(rename = "150")]
    P150,
    #[serde(rename = "200")]
    P200,
}

impl UiScale {
    pub const ALL: [Self; 5] = [Self::Auto, Self::P100, Self::P125, Self::P150, Self::P200];

    /// Absolute scale factor; `None` for Auto.
    pub fn factor(self) -> Option<f32> {
        match self {
            Self::Auto => None,
            Self::P100 => Some(1.0),
            Self::P125 => Some(1.25),
            Self::P150 => Some(1.5),
            Self::P200 => Some(2.0),
        }
    }

    /// Zoom factor on top of the native scale (`None` = unknown, taken as 1.0) giving this size.
    pub fn zoom_factor(self, native: Option<f32>) -> f32 {
        let native = native.filter(|n| n.is_finite() && *n > 0.0).unwrap_or(1.0);
        self.factor().map_or(1.0, |f| f / native)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::P100 => "100 %",
            Self::P125 => "125 %",
            Self::P150 => "150 %",
            Self::P200 => "200 %",
        }
    }
}

/// Terminal color scheme: 16 ANSI colors plus defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalScheme {
    pub name: String,
    pub foreground: Rgba,
    pub background: Rgba,
    pub cursor: Rgba,
    pub selection: Rgba,
    pub ansi: [Rgba; 16],
}

impl TerminalScheme {
    pub fn light() -> Self {
        Self {
            name: "rustxterm-light".into(),
            foreground: Rgba::hex(0x1d2330),
            background: Rgba::hex(0xffffff),
            cursor: Rgba::hex(0x1d2330),
            selection: Rgba::hex(0xcfe2fb),
            ansi: [
                Rgba::hex(0x000000),
                Rgba::hex(0xc62828),
                Rgba::hex(0x2e7d32),
                Rgba::hex(0x9a6700),
                Rgba::hex(0x1f6fd1),
                Rgba::hex(0x8e24aa),
                Rgba::hex(0x00838f),
                Rgba::hex(0xbfbfbf),
                Rgba::hex(0x666666),
                Rgba::hex(0xe53935),
                Rgba::hex(0x43a047),
                Rgba::hex(0xc79100),
                Rgba::hex(0x4c9bf0),
                Rgba::hex(0xab47bc),
                Rgba::hex(0x00acc1),
                Rgba::hex(0xffffff),
            ],
        }
    }

    pub fn dark() -> Self {
        Self {
            name: "rustxterm-dark".into(),
            foreground: Rgba::hex(0xdfe3ea),
            background: Rgba::hex(0x14161a),
            cursor: Rgba::hex(0xdfe3ea),
            selection: Rgba::hex(0x2f4d73),
            ansi: [
                Rgba::hex(0x1e2127),
                Rgba::hex(0xe06c75),
                Rgba::hex(0x98c379),
                Rgba::hex(0xd19a66),
                Rgba::hex(0x61afef),
                Rgba::hex(0xc678dd),
                Rgba::hex(0x56b6c2),
                Rgba::hex(0xabb2bf),
                Rgba::hex(0x5c6370),
                Rgba::hex(0xe88388),
                Rgba::hex(0xa5d68a),
                Rgba::hex(0xe5c07b),
                Rgba::hex(0x80c0ff),
                Rgba::hex(0xd7a1e8),
                Rgba::hex(0x7fd3dc),
                Rgba::hex(0xffffff),
            ],
        }
    }

    /// Default scheme for a UI theme.
    pub fn for_dark(dark: bool) -> Self {
        if dark { Self::dark() } else { Self::light() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_falls_back_to_light() {
        assert!(!ThemeMode::System.is_dark(None));
        assert!(ThemeMode::System.is_dark(Some(true)));
        assert!(ThemeMode::Dark.is_dark(Some(false)));
    }

    fn all_colors(t: &Tokens) -> Vec<(&'static str, Rgba)> {
        vec![
            ("surface", t.surface),
            ("panel", t.panel),
            ("toolbar", t.toolbar),
            ("input", t.input),
            ("text", t.text),
            ("text_muted", t.text_muted),
            ("text_disabled", t.text_disabled),
            ("accent", t.accent),
            ("border", t.border),
            ("border_subtle", t.border_subtle),
            ("selection", t.selection),
            ("selection_text", t.selection_text),
            ("hover", t.hover),
            ("pressed", t.pressed),
            ("focus", t.focus),
            ("danger", t.danger),
            ("warning", t.warning),
            ("success", t.success),
            ("tab_active", t.tab_active),
            ("tab_inactive", t.tab_inactive),
        ]
    }

    /// WCAG 2.x relative luminance.
    fn luminance(c: Rgba) -> f64 {
        let lin = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(c.0) + 0.7152 * lin(c.1) + 0.0722 * lin(c.2)
    }

    fn contrast(a: Rgba, b: Rgba) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn neutral_tokens_are_grey_and_no_blue_slate() {
        for t in [Tokens::light(), Tokens::dark()] {
            // Chromatic by design: accent, focus, status colors, and selection/hover/pressed in Light.
            let mut chromatic = vec!["accent", "focus", "danger", "warning", "success"];
            if !t.dark {
                chromatic.extend(["selection", "hover", "pressed"]);
            }
            for (name, c) in all_colors(&t) {
                assert_ne!(c, Rgba::hex(0x232833), "{name} uses the old blue-slate");
                if !chromatic.contains(&name) {
                    let spread = c.0.max(c.1).max(c.2) - c.0.min(c.1).min(c.2);
                    assert!(spread <= 4, "{name} {c:?} is not grey (dark={})", t.dark);
                }
            }
        }
    }

    #[test]
    fn wcag_contrast_pairs() {
        let min = |ratio: f64, want: f64| ratio >= want;
        for t in [Tokens::light(), Tokens::dark()] {
            for (bg_name, bg) in
                [("surface", t.surface), ("panel", t.panel), ("toolbar", t.toolbar), ("input", t.input)]
            {
                assert!(min(contrast(t.text, bg), 7.0), "text on {bg_name} (dark={})", t.dark);
                assert!(min(contrast(t.text_muted, bg), 4.5), "text_muted on {bg_name} (dark={})", t.dark);
            }
            assert!(min(contrast(t.selection_text, t.selection), 4.5), "selection_text (dark={})", t.dark);
            assert!(min(contrast(t.focus, t.surface), 3.0), "focus on surface (dark={})", t.dark);
            assert!(min(contrast(t.focus, t.panel), 3.0), "focus on panel (dark={})", t.dark);
        }
    }

    #[test]
    fn metrics_and_type_scale_values() {
        let m = Metrics::default();
        assert_eq!(m.space, [2.0, 4.0, 6.0, 8.0, 12.0, 16.0, 24.0]);
        assert_eq!((m.control_height, m.dialog_button), (22.0, (80.0, 30.0)));
        let bands = [m.menu_height, m.toolbar_height, m.toolbar_compact_height, m.row_height, m.status_height];
        assert_eq!(bands, [20.0, 52.0, 34.0, 28.0, 18.0]);
        assert_eq!((m.sidebar_width, m.strip_width), (240.0, 34.0));
        assert_eq!([m.icon_small, m.icon_toolbar, m.icon_tile, m.icon_large], [16.0, 24.0, 32.0, 48.0]);
        let ts = TypeScale::default();
        assert_eq!([ts.caption, ts.body, ts.strong, ts.heading, ts.display], [11.0, 12.0, 12.0, 15.0, 32.0]);
    }

    #[test]
    fn ui_scale_zoom() {
        assert_eq!(UiScale::Auto.zoom_factor(Some(2.0)), 1.0);
        assert_eq!(UiScale::P150.zoom_factor(None), 1.5);
        assert_eq!(UiScale::P100.zoom_factor(Some(2.0)), 0.5);
        assert_eq!(serde_json::to_string(&UiScale::P125).unwrap(), "\"125\"");
        assert_eq!(serde_json::from_str::<UiScale>("\"auto\"").unwrap(), UiScale::Auto);
    }

    #[test]
    fn serde_round_trip() {
        for t in [Tokens::light(), Tokens::dark()] {
            assert_eq!(serde_json::from_str::<Tokens>(&serde_json::to_string(&t).unwrap()).unwrap(), t);
        }
        let s = TerminalScheme::dark();
        assert_eq!(serde_json::from_str::<TerminalScheme>(&serde_json::to_string(&s).unwrap()).unwrap(), s);
        assert_eq!(serde_json::to_string(&ThemeMode::System).unwrap(), "\"system\"");
    }
}
