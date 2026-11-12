use std::sync::Arc;

use eframe::egui::{
    self, Color32 as C, Context, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke, TextStyle,
    Theme, ThemePreference, Visuals, vec2,
};
use serde::{Deserialize, Serialize};

pub const MEDIUM: &str = "medium";
pub const SEMIBOLD: &str = "semibold";
/// Phosphor alone: Inter and Segoe UI Symbol carry other glyphs on some of its codepoints.
pub const ICONS: &str = "icons";

pub fn icon_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(ICONS.into()))
}

/// Colour roles of one theme; look them up with [`palette`].
pub struct Palette {
    pub gallery_bg: C,
    /// Title and tab bar.
    pub chrome_bg: C,
    /// Toolbar and active tab.
    pub toolbar_bg: C,
    pub status_bg: C,
    /// Popups, combo lists, tooltips.
    pub popup_bg: C,
    /// Text field and segmented rail.
    pub field_bg: C,
    pub button_fill: C,
    pub border: C,
    pub hover_bg: C,
    /// Slider rail and checkbox body.
    pub control_bg: C,
    pub text: C,
    pub text_muted: C,
    pub text_weak: C,
    pub accent: C,
    pub active_bg: C,
    pub active_text: C,
    /// New arrival and live indicator.
    pub arrival: C,
    pub error: C,
    pub tile_placeholder: C,
    pub preview_bg: C,
    /// Format badge and pin backdrop (white text on top).
    pub badge_bg: C,
}

const fn rgb(r: u8, g: u8, b: u8) -> C {
    C::from_rgb(r, g, b)
}

pub const DARK: Palette = Palette {
    gallery_bg: rgb(17, 17, 19),
    chrome_bg: rgb(20, 20, 22),
    toolbar_bg: rgb(30, 30, 34),
    status_bg: rgb(24, 24, 27),
    popup_bg: rgb(38, 38, 43),
    field_bg: rgb(20, 20, 22),
    button_fill: rgb(42, 42, 48),
    border: rgb(42, 42, 48),
    hover_bg: rgb(42, 42, 48),
    control_bg: rgb(50, 50, 56),
    text: rgb(232, 232, 236),
    text_muted: rgb(200, 200, 208),
    text_weak: rgb(150, 150, 160),
    accent: rgb(77, 148, 255),
    active_bg: rgb(34, 50, 77),
    active_text: rgb(184, 210, 255),
    arrival: rgb(255, 184, 48),
    error: rgb(255, 138, 92),
    tile_placeholder: rgb(28, 28, 32),
    preview_bg: rgb(18, 18, 18),
    badge_bg: C::from_black_alpha(160),
};

pub const LIGHT: Palette = Palette {
    gallery_bg: rgb(228, 228, 231),
    chrome_bg: rgb(235, 235, 238),
    toolbar_bg: rgb(250, 250, 251),
    status_bg: rgb(243, 243, 245),
    popup_bg: rgb(255, 255, 255),
    field_bg: rgb(235, 235, 238),
    button_fill: rgb(255, 255, 255),
    border: rgb(220, 220, 225),
    hover_bg: rgb(235, 235, 238),
    control_bg: rgb(220, 220, 225),
    text: rgb(24, 24, 27),
    text_muted: rgb(63, 63, 70),
    text_weak: rgb(95, 95, 107),
    accent: rgb(31, 111, 235),
    active_bg: rgb(221, 233, 253),
    active_text: rgb(23, 78, 166),
    arrival: rgb(224, 150, 0),
    error: rgb(212, 68, 47),
    tile_placeholder: rgb(212, 212, 216),
    preview_bg: rgb(18, 18, 18),
    badge_bg: C::from_black_alpha(160),
};

/// The user's theme setting; `System` follows the OS.
#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeChoice {
    #[default]
    Light,
    Dark,
    System,
}

impl ThemeChoice {
    pub fn next(self) -> Self {
        match self {
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
            Self::System => Self::Light,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "Clair",
            Self::Dark => "Sombre",
            Self::System => "Système",
        }
    }

    fn preference(self) -> ThemePreference {
        match self {
            Self::Light => ThemePreference::Light,
            Self::Dark => ThemePreference::Dark,
            Self::System => ThemePreference::System,
        }
    }

    pub fn native(self) -> egui::SystemTheme {
        match self {
            Self::Light => egui::SystemTheme::Light,
            Self::Dark => egui::SystemTheme::Dark,
            Self::System => egui::SystemTheme::SystemDefault,
        }
    }
}

/// The palette of the theme currently in effect for `ctx`, system choice resolved.
pub fn palette(ctx: &Context) -> &'static Palette {
    match ctx.theme() {
        Theme::Dark => &DARK,
        Theme::Light => &LIGHT,
    }
}

fn visuals(p: &Palette, mut v: Visuals) -> Visuals {
    v.panel_fill = p.toolbar_bg;
    v.window_fill = p.popup_bg;
    v.extreme_bg_color = p.field_bg;
    v.faint_bg_color = p.status_bg;
    v.hyperlink_color = if v.dark_mode { p.active_text } else { p.accent };
    v.selection.bg_fill = p.active_bg;
    v.selection.stroke = Stroke::new(1.0, p.active_text);
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(8);
    v.warn_fg_color = p.arrival;
    v.error_fg_color = p.error;
    v.popup_shadow = Shadow { offset: [0, 8], blur: 20, spread: 0, color: C::from_black_alpha(128) };
    v.window_shadow = v.popup_shadow;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.status_bg;
    w.noninteractive.weak_bg_fill = p.status_bg;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.weak_bg_fill = C::TRANSPARENT;
    w.inactive.bg_fill = p.control_bg;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, p.text_muted);
    w.hovered.weak_bg_fill = p.hover_bg;
    w.hovered.bg_fill = p.control_bg;
    w.hovered.bg_stroke = Stroke::NONE;
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.active.weak_bg_fill = p.control_bg;
    w.active.bg_fill = p.control_bg;
    w.active.bg_stroke = Stroke::NONE;
    w.active.fg_stroke = Stroke::new(1.0, p.text);
    w.open.weak_bg_fill = p.hover_bg;
    w.open.bg_fill = p.hover_bg;
    w.open.bg_stroke = Stroke::NONE;
    w.open.fg_stroke = Stroke::new(1.0, p.text);
    for s in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        s.corner_radius = CornerRadius::same(6);
        s.expansion = 0.0;
    }
    v.weak_text_color = Some(p.text_weak);
    v
}

pub fn dark_visuals() -> Visuals {
    visuals(&DARK, Visuals::dark())
}

pub fn light_visuals() -> Visuals {
    visuals(&LIGHT, Visuals::light())
}

/// Text styles and spacing, for both themes.
pub fn apply_style(ctx: &Context) {
    ctx.all_styles_mut(|s| {
        let p = FontFamily::Proportional;
        s.text_styles = [
            (TextStyle::Small, FontId::new(11.0, p.clone())),
            (TextStyle::Body, FontId::new(13.0, p.clone())),
            (TextStyle::Button, FontId::new(13.0, FontFamily::Name(MEDIUM.into()))),
            (TextStyle::Heading, FontId::new(16.0, FontFamily::Name(SEMIBOLD.into()))),
            (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
        ]
        .into();
        s.spacing.item_spacing = vec2(8.0, 6.0);
        s.spacing.button_padding = vec2(10.0, 5.0);
        s.spacing.interact_size = vec2(28.0, 28.0);
        s.spacing.slider_width = 120.0;
    });
}

/// Installs both visuals sets and the preference; the native title bar is handled by [`sync_native`].
pub fn apply(ctx: &Context, choice: ThemeChoice) {
    ctx.set_visuals_of(Theme::Dark, dark_visuals());
    ctx.set_visuals_of(Theme::Light, light_visuals());
    apply_style(ctx);
    ctx.set_theme(choice.preference());
}

/// Makes `viewport`'s native title bar follow the choice.
pub fn sync_native(ctx: &Context, viewport: egui::ViewportId, choice: ThemeChoice) {
    ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::SetTheme(choice.native()));
}

/// Inter (400/500/600), JetBrains Mono, Phosphor, then Segoe UI Symbol for the arrows the others lack.
pub fn install_fonts(ctx: &Context) {
    let mut fonts = FontDefinitions::default();
    let embedded: [(&str, &'static [u8]); 5] = [
        ("inter", include_bytes!("../../assets/fonts/Inter-Regular.ttf")),
        ("inter-500", include_bytes!("../../assets/fonts/Inter-Medium.ttf")),
        ("inter-600", include_bytes!("../../assets/fonts/Inter-SemiBold.ttf")),
        ("mono", include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf")),
        ("phosphor", include_bytes!("../../assets/fonts/Phosphor.ttf")),
    ];
    for (name, bytes) in embedded {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into());
    let symbols = std::path::PathBuf::from(windir).join("Fonts").join("seguisym.ttf");
    match std::fs::read(&symbols) {
        Ok(bytes) => {
            fonts.font_data.insert("symbols".into(), Arc::new(FontData::from_owned(bytes)));
        }
        Err(_) => log::warn!("symbol font not found at {}", symbols.display()),
    }
    let has_symbols = fonts.font_data.contains_key("symbols");

    let tail = |fonts: &mut FontDefinitions, family: FontFamily| {
        let list = fonts.families.entry(family).or_default();
        list.push("phosphor".into());
        if has_symbols {
            list.push("symbols".into());
        }
    };
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "inter".into());
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "mono".into());
    tail(&mut fonts, FontFamily::Proportional);
    tail(&mut fonts, FontFamily::Monospace);
    for (name, face) in [(MEDIUM, "inter-500"), (SEMIBOLD, "inter-600")] {
        let family = FontFamily::Name(name.into());
        fonts.families.insert(family.clone(), vec![face.into(), "inter".into()]);
        tail(&mut fonts, family);
    }
    fonts.families.insert(FontFamily::Name(ICONS.into()), vec!["phosphor".into()]);
    ctx.set_fonts(fonts);
}

/// Phosphor Regular codepoints; render with any family, Phosphor is in every fallback chain.
pub mod icons {
    pub const FOLDER_OPEN: &str = "\u{e256}";
    pub const TREE_STRUCTURE: &str = "\u{e67c}";
    pub const LIST: &str = "\u{e2f0}";
    pub const GRID_FOUR: &str = "\u{e296}";
    pub const ROWS: &str = "\u{e5a2}";
    pub const LAYOUT: &str = "\u{e6d6}";
    pub const SORT_ASCENDING: &str = "\u{e444}";
    pub const SORT_DESCENDING: &str = "\u{e446}";
    pub const ARROWS_DOWN_UP: &str = "\u{e098}";
    pub const MAGNIFYING_GLASS: &str = "\u{e30c}";
    pub const X: &str = "\u{e4f6}";
    pub const PLUS: &str = "\u{e3d4}";
    pub const PUSH_PIN: &str = "\u{e3e2}";
    pub const CUBE: &str = "\u{e1da}";
    pub const WARNING: &str = "\u{e4e0}";
    pub const CHECK: &str = "\u{e182}";
    pub const CARET_DOWN: &str = "\u{e136}";
    pub const CARET_RIGHT: &str = "\u{e13a}";
    pub const CARET_LEFT: &str = "\u{e138}";
    pub const ARROWS_OUT: &str = "\u{e0a2}";
    pub const ARROWS_IN: &str = "\u{e09a}";
    pub const MINUS: &str = "\u{e32a}";
    pub const IMAGE: &str = "\u{e2ca}";
    pub const DOTS_THREE: &str = "\u{e1fe}";
    pub const ARROW_CLOCKWISE: &str = "\u{e036}";
    pub const SUN: &str = "\u{e472}";
    pub const MOON: &str = "\u{e330}";
    pub const MONITOR: &str = "\u{e32e}";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_cycles_through_all_three() {
        assert_eq!(ThemeChoice::Light.next().next().next(), ThemeChoice::Light);
        assert_eq!(ThemeChoice::default(), ThemeChoice::Light);
    }

    #[test]
    fn palette_follows_the_context_theme() {
        let ctx = Context::default();
        apply(&ctx, ThemeChoice::Dark);
        assert_eq!(palette(&ctx).gallery_bg, DARK.gallery_bg);
        apply(&ctx, ThemeChoice::Light);
        assert_eq!(palette(&ctx).gallery_bg, LIGHT.gallery_bg);
    }
}
