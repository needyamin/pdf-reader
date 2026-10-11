//! Shared chrome for every floating surface in the application.
//!
//! The reader has a lot of windows that are not panels: Settings, Keyboard
//! Shortcuts, About Us, the export pane, the unsaved-changes and password
//! prompts, the status toast, the lookup card. Each of them used to invent its
//! own frame — its own fill, stroke, corner radius, margin and buttons — which
//! is how a dialog ends up looking like it belongs to a different program than
//! the toolbar behind it.
//!
//! Everything in this module is the one answer to "what does a window look
//! like", derived from the [`Palette`] alone. A window that uses
//! [`dialog_window`] and [`card_frame`] cannot drift from the main chrome,
//! because both come from the same palette the menu bar and toolbar use.
//!
//! The rule the whole module exists to enforce: **a window is a surface of
//! `panel_bg`, bordered by `border`, rounded a few points more than the
//! controls inside it.** Nothing here picks a colour the palette does not have
//! and nothing here picks a radius the palette did not choose.

use egui::{Align2, Color32, Frame, Margin, RichText, Stroke, Vec2, epaint::Shadow};

use crate::theme::Palette;

/// Padding inside a window or dialog, on every side.
///
/// `i8` because egui's `Margin` counts in whole points.
pub const WINDOW_MARGIN: i8 = 18;
/// Padding inside a nested card: a list, a preview, a grouped block.
pub const CARD_MARGIN: i8 = 10;
/// Narrowest a modal prompt is ever drawn. Below this the buttons start
/// wrapping, which is the point at which a prompt stops reading as a prompt.
pub const DIALOG_MIN_WIDTH: f32 = 360.0;
/// Gap between the controls and the message in a footer row.
pub const FOOTER_GAP: f32 = 10.0;
/// Height of the standard dialog action button.
pub const BUTTON_HEIGHT: f32 = 30.0;

/// Radius of a floating window or dialog.
///
/// A few points rounder than the controls inside it (see
/// [`Palette::radius`]), so a window reads as the container and a button reads
/// as the thing contained.
pub fn window_radius(palette: &Palette) -> f32 {
    palette.radius + 3.0
}

/// Radius of a card, list or field nested inside a window: the same radius as
/// any other control, because that is what it is.
pub const fn card_radius(palette: &Palette) -> f32 {
    palette.radius
}

/// Radius of a small chip or badge. One point tighter than a control so a row
/// of chips does not read as a row of buttons.
pub fn chip_radius(palette: &Palette) -> f32 {
    (palette.radius - 1.0).max(2.0)
}

/// The frame of a window, dialog or modal.
///
/// `panel_bg` because a window is chrome, not paper — the canvas behind it is
/// `window_bg` and has to stay distinguishable from it.
pub fn window_frame(palette: &Palette) -> Frame {
    Frame::new()
        .fill(palette.panel_bg)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(window_radius(palette))
        .inner_margin(Margin::same(WINDOW_MARGIN))
        .shadow(Shadow {
            offset: [0, 8],
            blur: 28,
            spread: 0,
            color: Color32::from_black_alpha(72),
        })
}

/// The frame of a window's title bar.
///
/// egui's default paints the title bar from `widgets.open`, which is the same
/// fill a pressed button uses; on several themes that made the title read as
/// active when it was not, and it never matched the body. This pins it to the
/// body's own fill with a hairline under it, so the bar belongs to the window
/// rather than looking bolted on.
pub fn title_frame(palette: &Palette) -> Frame {
    Frame::new()
        .fill(palette.panel_bg)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(window_radius(palette))
        .inner_margin(Margin::symmetric(12, 8))
}

/// A block inside a window: a list, a preview, a grouped set of controls.
pub fn card_frame(palette: &Palette) -> Frame {
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(card_radius(palette))
        .inner_margin(Margin::same(CARD_MARGIN))
}

/// A read-only field: a path, a value, anything shown but not edited.
pub fn field_frame(palette: &Palette) -> Frame {
    Frame::new()
        .fill(palette.window_bg)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(card_radius(palette))
        .inner_margin(Margin::symmetric(9, 6))
}

/// A small chip: a page badge, a count, a tag.
pub fn chip_frame(palette: &Palette) -> Frame {
    Frame::new()
        .fill(palette.accent_soft)
        .corner_radius(chip_radius(palette))
        .inner_margin(Margin::symmetric(8, 2))
}

/// A toast or overlay surface, tinted by the state it reports.
pub fn toast_frame(palette: &Palette, accent: Color32) -> Frame {
    Frame::new()
        .fill(palette.panel_bg)
        .stroke(Stroke::new(1.0, accent))
        .corner_radius(card_radius(palette))
        .inner_margin(Margin::same(10))
        .shadow(Shadow {
            offset: [0, 4],
            blur: 18,
            spread: 0,
            color: Color32::from_black_alpha(64),
        })
}

/// Start a window that looks like every other window in the app.
///
/// Centred, non-collapsible, and framed by [`window_frame`]. Centred because
/// every window here is a response to a menu command — it has no natural
/// position, and one that appeared wherever it happened to fit would look like
/// a different app each time. Callers add their own sizing and `.open(..)`.
///
/// Use [`floating_window`] instead for anything the user is meant to drag:
/// anchoring re-pins a window to the same spot on every frame, which makes it
/// immovable however hard the title bar is pulled.
pub fn dialog_window(title: &str, palette: &Palette) -> egui::Window<'static> {
    floating_window(title, palette).anchor(Align2::CENTER_CENTER, Vec2::ZERO)
}

/// [`dialog_window`] without the centre anchor: the same frame, title bar and
/// non-collapsible behaviour, but a window the user can put wherever it is out
/// of the way. The caller supplies a `default_pos` (and, to make the placement
/// stick, a stable id — see [`egui::Window::id`]).
pub fn floating_window(title: &str, palette: &Palette) -> egui::Window<'static> {
    egui::Window::new(RichText::new(title).strong().size(13.0).color(palette.text))
        .frame(window_frame(palette))
        .title_frame(title_frame(palette))
        .collapsible(false)
}

/// The committed action of a window: Export, Save and close, Unlock.
///
/// `accent_soft` rather than a solid accent, because a fully saturated button
/// in a window that is otherwise quiet shouts; this is a firm suggestion of
/// the accent colour, which is what the toolbar's primary buttons do too.
pub fn primary_button(palette: &Palette, label: &str) -> egui::Button<'static> {
    egui::Button::new(
        RichText::new(label.to_owned())
            .color(palette.text)
            .size(12.5),
    )
    .fill(palette.accent_soft)
    .corner_radius(palette.radius)
    .min_size(Vec2::new(0.0, BUTTON_HEIGHT))
}

/// The neutral action: Cancel, Close, Browse, Clear.
pub fn secondary_button(palette: &Palette, label: &str) -> egui::Button<'static> {
    egui::Button::new(
        RichText::new(label.to_owned())
            .color(palette.text)
            .size(12.5),
    )
    .fill(palette.surface)
    .corner_radius(palette.radius)
    .min_size(Vec2::new(0.0, BUTTON_HEIGHT))
}

/// Same shape as [`primary_button`], drawn for a disabled state.
///
/// Kept separate so an off button still occupies the right footprint: an
/// action that vanishes when it cannot run moves the row around under the
/// pointer.
pub fn disabled_button(palette: &Palette, label: &str) -> egui::Button<'static> {
    egui::Button::new(
        RichText::new(label.to_owned())
            .color(palette.text_dim)
            .size(12.5),
    )
    .fill(palette.surface)
    .corner_radius(palette.radius)
    .min_size(Vec2::new(0.0, BUTTON_HEIGHT))
}

/// Section heading inside a window: a strong label with a hairline under it.
///
/// Used by Settings, Keyboard Shortcuts and the export pane alike, which is
/// the whole point — a section in one window looks exactly like a section in
/// another.
pub fn section(ui: &mut egui::Ui, palette: &Palette, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).strong().size(13.0).color(palette.text));
    ui.add_space(3.0);
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, palette.border);
    ui.add_space(8.0);
}

/// The dim explanatory line under a control or heading.
pub fn hint(palette: &Palette, text: &str) -> RichText {
    RichText::new(text).size(11.5).color(palette.text_dim)
}

/// The title at the top of a window's content area.
pub fn heading(palette: &Palette, text: &str) -> RichText {
    RichText::new(text).strong().size(18.0).color(palette.text)
}

/// A hairline separator drawn in the palette's own border colour.
pub fn divider(ui: &mut egui::Ui, palette: &Palette) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, palette.border);
}

/// A rounded image holder, so a photo or preview cannot be the one sharp
/// corner in an otherwise rounded window.
pub fn image_frame(palette: &Palette) -> Frame {
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(card_radius(palette))
        .inner_margin(Margin::same(4))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use pdfreader_core::ThemeId;

    #[test]
    fn a_window_is_rounder_than_the_controls_inside_it() {
        for id in ThemeId::ALL {
            let palette = Theme::from_id(id).palette;
            assert!(
                window_radius(&palette) > card_radius(&palette),
                "{id:?}: a window must read as a container, not as one more control"
            );
            assert!(
                chip_radius(&palette) >= 2.0,
                "{id:?}: chip radius collapsed"
            );
        }
    }

    /// The flat theme must stay flat everywhere, including the derived radii —
    /// a rounded window on a squared theme is exactly the mismatch this module
    /// exists to prevent.
    #[test]
    fn the_flat_theme_stays_flat_all_the_way_down() {
        let flat = Theme::from_id(ThemeId::AnsNew).palette;
        let rounded = Theme::from_id(ThemeId::Dark).palette;
        assert!(flat.radius < rounded.radius);
        assert!(window_radius(&flat) < window_radius(&rounded));
    }

    #[test]
    fn every_frame_uses_only_palette_colours() {
        // A window that painted a colour of its own would show up here as a
        // fill no palette contains.
        for id in ThemeId::ALL {
            let palette = Theme::from_id(id).palette;
            let known = [
                palette.window_bg,
                palette.panel_bg,
                palette.border,
                palette.surface,
                palette.accent_soft,
                palette.window_bg,
            ];
            for frame in [
                window_frame(&palette),
                title_frame(&palette),
                card_frame(&palette),
                field_frame(&palette),
                chip_frame(&palette),
                image_frame(&palette),
            ] {
                assert!(
                    known.contains(&frame.fill),
                    "{id:?}: a frame filled with {:?}, which is not in the palette",
                    frame.fill
                );
            }
        }
    }
}
