//! Map `core` theme tokens to egui style and fonts (design D7, R1, R2, R9).

use std::sync::Arc;

use egui::epaint::Shadow;
use egui::{
    Color32, Context, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Id, LayerId, Margin, Order, Stroke,
    Style, TextStyle, Visuals, vec2,
};
use rxt_core::{Metrics, Rgba, TerminalScheme, Tokens, TypeScale};

/// Bundled fonts (SIL OFL 1.1, see ATTRIBUTION.md): (egui font name, bytes).
const FONTS: [(&str, &[u8]); 7] = [
    ("Noto Sans", include_bytes!("../../../assets/fonts/noto-sans/NotoSans-Regular.ttf")),
    ("Noto Sans Bold", include_bytes!("../../../assets/fonts/noto-sans/NotoSans-Bold.ttf")),
    ("JetBrains Mono", include_bytes!("../../../assets/fonts/jetbrains-mono/JetBrainsMono-Regular.ttf")),
    ("JetBrains Mono Bold", include_bytes!("../../../assets/fonts/jetbrains-mono/JetBrainsMono-Bold.ttf")),
    ("Cascadia Mono", include_bytes!("../../../assets/fonts/cascadia-mono/CascadiaMono-Regular.ttf")),
    ("Hack", include_bytes!("../../../assets/fonts/hack/Hack-Regular.ttf")),
    ("DejaVu Sans Mono", include_bytes!("../../../assets/fonts/dejavu-sans-mono/DejaVuSansMono.ttf")),
];

/// Bold UI family (strong role).
pub(crate) const BOLD: &str = "Noto Sans Bold";
/// Text style of the strong role (12 px bold).
pub(crate) const STRONG: &str = "strong";
/// Text style of the display role (32 px, Home title).
pub(crate) const DISPLAY: &str = "display";

/// Token color as an egui color.
pub(crate) fn color(c: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(c.0, c.1, c.2, c.3)
}

/// Bundled monospace faces offered for the terminal; the first is the default (crisp at 13 px with
/// egui's rasterizer, like Windows Terminal's default).
pub(crate) const MONO_FONTS: [&str; 4] = ["Cascadia Mono", "JetBrains Mono", "Hack", FULL_MONO];

/// Bundled monospace face with every symbol the terminal UI draws; first fallback of the others.
const FULL_MONO: &str = "DejaVu Sans Mono";

/// egui's default fonts with the bundled ones first: Noto Sans for proportional text, Cascadia
/// Mono for monospace; each bundled face is also a named family. egui's fonts stay as fallbacks.
pub(crate) fn fonts() -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    let defaults = |defs: &FontDefinitions, f: FontFamily| defs.families.get(&f).cloned().unwrap_or_default();
    let prop = defaults(&defs, FontFamily::Proportional);
    // DejaVu Sans Mono covers the symbols the others lack (Cascadia Mono has no ✗, Hack no ✓✗).
    let mono: Vec<String> =
        std::iter::once(FULL_MONO.to_owned()).chain(defaults(&defs, FontFamily::Monospace)).collect();
    for (name, bytes) in FONTS {
        defs.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
        let fallback = if MONO_FONTS.iter().any(|m| name.starts_with(m)) { &mono } else { &prop };
        let chain = std::iter::once(name.to_owned()).chain(fallback.iter().filter(|f| *f != name).cloned()).collect();
        defs.families.insert(FontFamily::Name(name.into()), chain);
    }
    defs.families.entry(FontFamily::Proportional).or_default().insert(0, "Noto Sans".to_owned());
    defs.families.insert(FontFamily::Monospace, std::iter::once(MONO_FONTS[0].to_owned()).chain(mono).collect());
    defs
}

/// Full egui style from tokens: type scale, spacing, interaction and visuals.
pub(crate) fn style(t: &Tokens) -> Style {
    let (m, ts) = (Metrics::DEFAULT, TypeScale::DEFAULT);
    let mut s = Style { visuals: visuals(t), ..Style::default() };
    s.text_styles = [
        (TextStyle::Small, FontId::proportional(ts.caption)),
        (TextStyle::Body, FontId::proportional(ts.body)),
        (TextStyle::Button, FontId::proportional(ts.body)),
        (TextStyle::Monospace, FontId::monospace(ts.body)),
        (TextStyle::Heading, FontId::proportional(ts.heading)),
        (TextStyle::Name(STRONG.into()), FontId::new(ts.strong, FontFamily::Name(BOLD.into()))),
        (TextStyle::Name(DISPLAY.into()), FontId::proportional(ts.display)),
    ]
    .into();
    let [s2, s4, s6, s8, _, s16, _] = m.space;
    let sp = &mut s.spacing;
    sp.item_spacing = vec2(s8, s4);
    sp.button_padding = vec2(s6, s2);
    sp.window_margin = Margin::same(s8 as i8);
    sp.menu_margin = Margin::same(s2 as i8);
    sp.indent = s16;
    sp.interact_size = vec2(m.dialog_button.0 / 2.0, m.control_height);
    sp.combo_height = 300.0;
    s.interaction.tooltip_delay = 0.5;
    // Animations off in snapshots (tests), short otherwise.
    s.animation_time = if cfg!(test) { 0.0 } else { 0.1 };
    // A blinking caret makes snapshots depend on wall-clock time.
    s.visuals.text_cursor.blink = !cfg!(test);
    s
}

/// egui visuals built from theme tokens: square corners, 1 px strokes, no shadows.
pub(crate) fn visuals(t: &Tokens) -> Visuals {
    let mut v = if t.dark { Visuals::dark() } else { Visuals::light() };
    let text = Stroke::new(1.0, color(t.text));
    let border = Stroke::new(1.0, color(t.border));
    // Hover and pressed outline: accent in Light, border in Dark.
    let outline = Stroke::new(1.0, color(if t.dark { t.border } else { t.accent }));
    v.panel_fill = color(t.panel);
    v.window_fill = color(t.panel);
    v.window_stroke = border;
    v.window_shadow = Shadow::NONE;
    v.popup_shadow = Shadow::NONE;
    v.extreme_bg_color = color(t.input);
    v.text_edit_bg_color = Some(color(t.input));
    v.faint_bg_color = color(t.panel);
    v.code_bg_color = color(t.panel);
    v.hyperlink_color = color(t.accent);
    v.error_fg_color = color(t.danger);
    v.warn_fg_color = color(t.warning);
    v.weak_text_color = Some(color(t.text_muted));
    v.selection.bg_fill = color(t.selection);
    v.selection.stroke = Stroke::new(1.0, color(t.selection_text));
    // egui fades disabled widgets; pick the opacity that turns text on panel into text_disabled.
    let channel = |c: Rgba| f32::from(c.0);
    v.disabled_alpha = ((channel(t.panel) - channel(t.text_disabled)) / (channel(t.panel) - channel(t.text))).abs();
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = color(t.panel);
    w.noninteractive.weak_bg_fill = color(t.panel);
    w.noninteractive.bg_stroke = Stroke::new(1.0, color(t.border_subtle));
    w.noninteractive.fg_stroke = text;
    w.inactive.weak_bg_fill = color(t.panel);
    w.inactive.bg_fill = color(t.input);
    w.inactive.bg_stroke = border;
    w.inactive.fg_stroke = text;
    w.hovered.weak_bg_fill = color(t.hover);
    w.hovered.bg_fill = color(t.hover);
    w.hovered.bg_stroke = outline;
    w.hovered.fg_stroke = text;
    w.active.weak_bg_fill = color(t.pressed);
    w.active.bg_fill = color(t.pressed);
    w.active.bg_stroke = outline;
    w.active.fg_stroke = text;
    w.open.weak_bg_fill = color(t.pressed);
    w.open.bg_fill = color(t.pressed);
    w.open.bg_stroke = border;
    w.open.fg_stroke = text;
    // Flat, square look everywhere: windows, menus, tooltips, widgets, scroll bars.
    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
    for w in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        w.corner_radius = CornerRadius::ZERO;
        w.expansion = 0.0;
    }
    v
}

/// Keyboard-focus tracking kept in egui memory between frames.
#[derive(Clone, Copy, Default)]
struct FocusState {
    /// Focus owner at the end of the previous frame.
    last: Option<Id>,
    /// Tab was pressed last frame; egui moves focus at the end of that frame.
    tab_pending: bool,
    /// Focus last moved by keyboard (cleared by any pointer press).
    keyboard: bool,
}

/// Paint the 1 px focus ring 1 px outside the focused widget when focus came from the keyboard.
/// Drawn on a foreground layer so parents never clip it. Call once per frame after the UI.
pub(crate) fn focus_ring(ctx: &Context, t: &Tokens) {
    let id = Id::new("rxt-focus-ring");
    let mut st: FocusState = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let focused = ctx.memory(|m| m.focused());
    let (tab, pointer) = ctx.input(|i| (i.key_pressed(egui::Key::Tab), i.pointer.any_pressed()));
    if pointer {
        st.keyboard = false;
    } else if focused != st.last && (tab || st.tab_pending) {
        st.keyboard = true;
    }
    st.tab_pending = tab;
    st.last = focused;
    ctx.data_mut(|d| d.insert_temp(id, st));
    let Some(rect) = focused.filter(|_| st.keyboard).and_then(|f| ctx.read_response(f)).map(|r| r.rect) else {
        return;
    };
    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, id));
    painter.rect_stroke(
        rect.expand(1.5),
        CornerRadius::ZERO,
        Stroke::new(1.0, color(t.focus)),
        egui::StrokeKind::Middle,
    );
}

/// Terminal scheme by name; unknown or unset names follow the UI theme.
pub(crate) fn scheme(name: Option<&str>, dark: bool) -> TerminalScheme {
    match name {
        Some(n) if n == TerminalScheme::light().name => TerminalScheme::light(),
        Some(n) if n == TerminalScheme::dark().name => TerminalScheme::dark(),
        _ => TerminalScheme::for_dark(dark),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visuals_square_one_px_and_token_colors() {
        for t in [Tokens::light(), Tokens::dark()] {
            let v = visuals(&t);
            assert_eq!(v.dark_mode, t.dark);
            assert_eq!(v.panel_fill, color(t.panel));
            assert_eq!(v.window_fill, color(t.panel));
            assert_eq!(v.extreme_bg_color, color(t.input));
            assert_eq!(v.selection.bg_fill, color(t.selection));
            assert_eq!((v.window_corner_radius, v.menu_corner_radius), (CornerRadius::ZERO, CornerRadius::ZERO));
            assert_eq!((v.window_shadow, v.popup_shadow), (Shadow::NONE, Shadow::NONE));
            assert_eq!((v.window_stroke.width, v.selection.stroke.width), (1.0, 1.0));
            let w = &v.widgets;
            for (name, s) in [
                ("idle", w.noninteractive),
                ("inactive", w.inactive),
                ("hover", w.hovered),
                ("active", w.active),
                ("open", w.open),
            ] {
                assert_eq!(s.corner_radius, CornerRadius::ZERO, "{name}");
                assert_eq!((s.bg_stroke.width, s.fg_stroke.width), (1.0, 1.0), "{name}");
                assert_eq!(s.expansion, 0.0, "{name}");
                assert_eq!(s.fg_stroke.color, color(t.text), "{name}");
            }
            assert_eq!(w.hovered.weak_bg_fill, color(t.hover));
            assert_eq!(w.hovered.bg_stroke.color, color(if t.dark { t.border } else { t.accent }));
            assert_eq!(w.active.weak_bg_fill, color(t.pressed));
            assert_eq!(w.noninteractive.bg_stroke.color, color(t.border_subtle));
            // Faded text lands on text_disabled (±2 per channel).
            let faded = f32::from(t.panel.0) + (f32::from(t.text.0) - f32::from(t.panel.0)) * v.disabled_alpha;
            assert!((faded - f32::from(t.text_disabled.0)).abs() <= 2.0, "disabled {faded}");
        }
    }

    #[test]
    fn style_type_scale_and_timing() {
        let s = style(&Tokens::dark());
        let size = |ts: TextStyle| s.text_styles[&ts].size;
        assert_eq!([size(TextStyle::Small), size(TextStyle::Body), size(TextStyle::Heading)], [11.0, 12.0, 15.0]);
        assert_eq!(s.text_styles[&TextStyle::Name(STRONG.into())].family, FontFamily::Name(BOLD.into()));
        assert_eq!(size(TextStyle::Name(DISPLAY.into())), 32.0);
        assert_eq!(s.spacing.interact_size.y, 22.0);
        assert_eq!(s.interaction.tooltip_delay, 0.5);
        assert_eq!(s.animation_time, 0.0, "animations are off in tests");
    }

    #[test]
    fn bundled_fonts_come_first_with_egui_fallbacks() {
        let defs = fonts();
        let prop = &defs.families[&FontFamily::Proportional];
        let mono = &defs.families[&FontFamily::Monospace];
        assert_eq!(
            (prop[0].as_str(), mono[0].as_str(), mono[1].as_str()),
            ("Noto Sans", "Cascadia Mono", "DejaVu Sans Mono")
        );
        assert!(prop.len() > 1 && mono.len() > 1, "egui defaults stay as fallbacks");
        assert_eq!(defs.families[&FontFamily::Name(BOLD.into())][0], BOLD);
    }

    #[test]
    fn scheme_defaults_follow_theme() {
        assert_eq!(scheme(None, true).name, "rustxterm-dark");
        assert_eq!(scheme(Some("rustxterm-light"), true).name, "rustxterm-light");
        assert_eq!(scheme(Some("nope"), false).name, "rustxterm-light");
    }
}
