//! Colour themes.
//!
//! The previous version shipped six named themes; these are the same six,
//! expressed as an egui `Visuals` plus a small palette for parts of the chrome
//! that paint themselves.

use egui::{Color32, Context, FontFamily, FontId, Margin, Stroke, TextStyle, Visuals};
use pdfreader_core::ThemeId;

/// Colours the chrome paints directly, beyond what `Visuals` covers.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Palette {
    /// Window and canvas background.
    pub window_bg: Color32,
    /// Toolbar, sidebar and panel background.
    pub panel_bg: Color32,
    /// One-pixel separators.
    pub border: Color32,
    /// Primary text.
    pub text: Color32,
    /// Secondary text.
    pub text_dim: Color32,
    /// Selection and focus colour.
    pub accent: Color32,
    /// Area behind a page, distinct from the canvas background.
    pub page_shadow: Color32,
    /// Subtle surface used for selected and elevated controls.
    pub surface: Color32,
    /// Hover fill for compact controls.
    pub surface_hover: Color32,
    /// Low-emphasis accent fill for active controls.
    pub accent_soft: Color32,
    /// Error state color.
    pub danger: Color32,
}

/// A named theme: egui visuals plus the extra palette.
#[derive(Clone, Debug)]
pub struct Theme {
    /// egui widget styling.
    pub visuals: Visuals,
    /// Extra colours used by hand-painted chrome.
    pub palette: Palette,
}

impl Theme {
    /// Build the theme for a given id.
    pub fn from_id(id: ThemeId) -> Self {
        match id {
            ThemeId::Acrobat => Self::build_flat(false, acrobat_palette()),
            ThemeId::Dark => Self::build(false, dark_palette()),
            ThemeId::Light => Self::build(true, light_palette()),
            ThemeId::Midnight => Self::build(false, midnight_palette()),
            ThemeId::Rose => Self::build(false, rose_palette()),
            ThemeId::Forest => Self::build(false, forest_palette()),
            ThemeId::Sunset => Self::build(false, sunset_palette()),
        }
    }

    /// Build visuals from a light flag and a palette.
    fn build(light: bool, palette: Palette) -> Self {
        Self::build_with_radius(light, palette, 7)
    }

    /// Build a flat theme: near-zero corner radii, in the spirit of Adobe
    /// Acrobat Pro DC's squared chrome.
    fn build_flat(light: bool, palette: Palette) -> Self {
        Self::build_with_radius(light, palette, 2)
    }

    /// Build visuals from a light flag, a palette and a widget corner radius.
    fn build_with_radius(light: bool, palette: Palette, radius: u8) -> Self {
        let mut visuals = if light {
            Visuals::light()
        } else {
            Visuals::dark()
        };

        visuals.window_fill = palette.window_bg;
        visuals.panel_fill = palette.panel_bg;
        visuals.extreme_bg_color = palette.window_bg;
        visuals.faint_bg_color = palette.panel_bg;
        visuals.window_stroke = egui::Stroke::new(1.0, palette.border);
        visuals.override_text_color = Some(palette.text);
        visuals.selection.bg_fill = palette.accent;
        visuals.selection.stroke = Stroke::new(1.0, palette.accent);
        visuals.hyperlink_color = palette.accent;
        visuals.widgets.noninteractive.bg_fill = palette.panel_bg;
        visuals.widgets.inactive.bg_fill = palette.surface;
        visuals.widgets.inactive.weak_bg_fill = palette.surface;
        visuals.widgets.hovered.bg_fill = palette.surface_hover;
        visuals.widgets.active.bg_fill = palette.accent_soft;
        visuals.widgets.open.bg_fill = palette.accent_soft;
        visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(radius);
        visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(radius);
        visuals.widgets.active.corner_radius = egui::CornerRadius::same(radius);
        visuals.widgets.open.corner_radius = egui::CornerRadius::same(radius);

        Self { visuals, palette }
    }

    /// Apply this theme to a context.
    pub fn apply(&self, ctx: &Context) {
        ctx.set_visuals(self.visuals.clone());
        ctx.all_styles_mut(|style| {
            style.override_font_id = Some(FontId::new(13.0, FontFamily::Proportional));
            style
                .text_styles
                .insert(TextStyle::Body, FontId::new(13.0, FontFamily::Proportional));
            style.text_styles.insert(
                TextStyle::Button,
                FontId::new(12.5, FontFamily::Proportional),
            );
            style.text_styles.insert(
                TextStyle::Small,
                FontId::new(11.0, FontFamily::Proportional),
            );
            style.text_styles.insert(
                TextStyle::Heading,
                FontId::new(20.0, FontFamily::Proportional),
            );
            style.spacing.item_spacing = egui::vec2(6.0, 5.0);
            style.spacing.window_margin = Margin::same(18);
            style.spacing.menu_margin = Margin::symmetric(8, 6);
            style.spacing.button_padding = egui::vec2(10.0, 5.0);
            style.spacing.interact_size = egui::vec2(28.0, 28.0);
            style.spacing.combo_width = 96.0;
            style.animation_time = 0.12;
            style.compact_menu_style = true;
            style.spacing.scroll = egui::style::ScrollStyle::thin();
        });
    }
}

/// Apply a theme id to a context.
pub fn apply(ctx: &Context, id: ThemeId) {
    Theme::from_id(id).apply(ctx);
}

/// Adobe Acrobat Pro DC–style palette: dark gray chrome, near-black canvas,
/// and the signature red accent.
const fn acrobat_palette() -> Palette {
    Palette {
        window_bg: Color32::from_rgb(0x25, 0x25, 0x26),
        panel_bg: Color32::from_rgb(0x2e, 0x2e, 0x30),
        border: Color32::from_rgb(0x45, 0x45, 0x48),
        text: Color32::from_rgb(0xe8, 0xe8, 0xea),
        text_dim: Color32::from_rgb(0x9a, 0x9a, 0x9e),
        accent: Color32::from_rgb(0xec, 0x1c, 0x24),
        page_shadow: Color32::from_rgb(0x10, 0x10, 0x11),
        surface: Color32::from_rgb(0x3a, 0x3a, 0x3d),
        surface_hover: Color32::from_rgb(0x46, 0x46, 0x4a),
        accent_soft: Color32::from_rgb(0x59, 0x15, 0x18),
        danger: Color32::from_rgb(0xff, 0x6b, 0x6f),
    }
}

const fn dark_palette() -> Palette {
    Palette {
        window_bg: Color32::from_rgb(0x0a, 0x0a, 0x0f),
        panel_bg: Color32::from_rgb(0x16, 0x16, 0x1d),
        border: Color32::from_rgb(0x2a, 0x2a, 0x35),
        text: Color32::from_rgb(0xe6, 0xe6, 0xf0),
        text_dim: Color32::from_rgb(0x9a, 0x9a, 0xad),
        accent: Color32::from_rgb(0x72, 0x9b, 0xff),
        page_shadow: Color32::from_rgb(0x00, 0x00, 0x00),
        surface: Color32::from_rgb(0x20, 0x22, 0x2b),
        surface_hover: Color32::from_rgb(0x2a, 0x2d, 0x38),
        accent_soft: Color32::from_rgb(0x2d, 0x3f, 0x66),
        danger: Color32::from_rgb(0xff, 0x7d, 0x86),
    }
}

const fn light_palette() -> Palette {
    Palette {
        window_bg: Color32::from_rgb(0xf0, 0xf3, 0xf7),
        panel_bg: Color32::from_rgb(0xfb, 0xfc, 0xfe),
        border: Color32::from_rgb(0xd9, 0xdf, 0xe8),
        text: Color32::from_rgb(0x1d, 0x24, 0x32),
        text_dim: Color32::from_rgb(0x6c, 0x76, 0x87),
        accent: Color32::from_rgb(0x3b, 0x6f, 0xd9),
        page_shadow: Color32::from_rgb(0xa8, 0xb2, 0xc0),
        surface: Color32::from_rgb(0xf1, 0xf4, 0xf8),
        surface_hover: Color32::from_rgb(0xe8, 0xed, 0xf5),
        accent_soft: Color32::from_rgb(0xd9, 0xe5, 0xff),
        danger: Color32::from_rgb(0xc9, 0x4b, 0x5d),
    }
}

const fn midnight_palette() -> Palette {
    Palette {
        window_bg: Color32::from_rgb(0x0b, 0x10, 0x2a),
        panel_bg: Color32::from_rgb(0x13, 0x1a, 0x3a),
        border: Color32::from_rgb(0x24, 0x2f, 0x5c),
        text: Color32::from_rgb(0xd8, 0xde, 0xf5),
        text_dim: Color32::from_rgb(0x7d, 0x88, 0xb4),
        accent: Color32::from_rgb(0x8a, 0x7b, 0xff),
        page_shadow: Color32::from_rgb(0x00, 0x00, 0x00),
        surface: Color32::from_rgb(0x1b, 0x24, 0x4a),
        surface_hover: Color32::from_rgb(0x23, 0x2f, 0x5d),
        accent_soft: Color32::from_rgb(0x36, 0x3b, 0x70),
        danger: Color32::from_rgb(0xff, 0x8e, 0x9b),
    }
}

const fn rose_palette() -> Palette {
    Palette {
        window_bg: Color32::from_rgb(0x1a, 0x0f, 0x16),
        panel_bg: Color32::from_rgb(0x2a, 0x17, 0x21),
        border: Color32::from_rgb(0x45, 0x26, 0x35),
        text: Color32::from_rgb(0xf5, 0xdc, 0xe6),
        text_dim: Color32::from_rgb(0xb0, 0x84, 0x97),
        accent: Color32::from_rgb(0xff, 0x78, 0xb0),
        page_shadow: Color32::from_rgb(0x00, 0x00, 0x00),
        surface: Color32::from_rgb(0x36, 0x1e, 0x2b),
        surface_hover: Color32::from_rgb(0x45, 0x26, 0x36),
        accent_soft: Color32::from_rgb(0x66, 0x2e, 0x4a),
        danger: Color32::from_rgb(0xff, 0x9a, 0xa8),
    }
}

const fn forest_palette() -> Palette {
    Palette {
        window_bg: Color32::from_rgb(0x0d, 0x16, 0x10),
        panel_bg: Color32::from_rgb(0x14, 0x24, 0x1c),
        border: Color32::from_rgb(0x24, 0x3d, 0x30),
        text: Color32::from_rgb(0xd9, 0xef, 0xe1),
        text_dim: Color32::from_rgb(0x7f, 0xa3, 0x90),
        accent: Color32::from_rgb(0x4c, 0xd7, 0x91),
        page_shadow: Color32::from_rgb(0x00, 0x00, 0x00),
        surface: Color32::from_rgb(0x1c, 0x31, 0x27),
        surface_hover: Color32::from_rgb(0x24, 0x40, 0x32),
        accent_soft: Color32::from_rgb(0x2c, 0x5a, 0x45),
        danger: Color32::from_rgb(0xff, 0x93, 0x91),
    }
}

const fn sunset_palette() -> Palette {
    Palette {
        window_bg: Color32::from_rgb(0x1d, 0x12, 0x0d),
        panel_bg: Color32::from_rgb(0x2e, 0x1c, 0x14),
        border: Color32::from_rgb(0x4d, 0x2e, 0x20),
        text: Color32::from_rgb(0xf7, 0xe3, 0xd5),
        text_dim: Color32::from_rgb(0xb5, 0x90, 0x78),
        accent: Color32::from_rgb(0xff, 0x9a, 0x52),
        page_shadow: Color32::from_rgb(0x00, 0x00, 0x00),
        surface: Color32::from_rgb(0x3d, 0x25, 0x19),
        surface_hover: Color32::from_rgb(0x4d, 0x2f, 0x20),
        accent_soft: Color32::from_rgb(0x6a, 0x3c, 0x20),
        danger: Color32::from_rgb(0xff, 0x9a, 0x87),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_produces_a_palette() {
        for id in ThemeId::ALL {
            let theme = Theme::from_id(id);
            assert_ne!(theme.palette.window_bg, theme.palette.text);
        }
    }

    #[test]
    fn light_theme_is_light_and_dark_is_dark() {
        assert!(Theme::from_id(ThemeId::Dark).visuals.dark_mode);
        assert!(!Theme::from_id(ThemeId::Light).visuals.dark_mode);
        // Every dark theme must report dark mode; only Light is light.
        for id in ThemeId::ALL {
            let expected_dark = id != ThemeId::Light;
            assert_eq!(
                Theme::from_id(id).visuals.dark_mode,
                expected_dark,
                "{id:?} dark_mode mismatch"
            );
        }
    }
}
