//! The print window: a preview of the pages that will print, and which pages.
//!
//! Printing used to go straight to the system dialog, which shows no preview and
//! whose own page range was ignored. This window is where the user decides what
//! goes to the printer — every page, just the one in view, or a typed range —
//! and sees the pages before committing. The single [`Action`] it produces
//! becomes the list of pages handed to the engine.
//!
//! It is built from the shared chrome helpers, so it reads as the same surface
//! as Settings, About Us and the export pane: one header, a two-pane body, a
//! footer with the committed action on the right.

use egui::{Context, RichText, Vec2};
use pdfreader_ui::{
    Palette, card_frame, chip_frame, divider, heading, hint, primary_button, secondary_button,
    section, window_frame,
};

/// Width of the whole window, in points.
const WINDOW_WIDTH: f32 = 900.0;

/// Width of the settings column on the left.
const SETTINGS_WIDTH: f32 = 300.0;

/// Height the preview pane is allowed to grow to before it scrolls.
const PREVIEW_HEIGHT: f32 = 430.0;

/// Least height the body holds, so the window does not collapse to a strip
/// while the first previews are still rendering.
const BODY_MIN_HEIGHT: f32 = 210.0;

/// Width one page preview is drawn at.
pub const PREVIEW_WIDTH: f32 = 132.0;

/// Most pages previewed. A five-hundred page job does not need five hundred
/// thumbnails; the count beside the range still tells the truth.
const PREVIEW_CAP: usize = 30;

/// Preview columns. Fixed rather than measured: inside the scroll area the
/// available width is not bounded, so a measured row would never wrap and would
/// stretch the window to the width of every page at once.
const PREVIEW_COLUMNS: usize = 3;

/// Which pages to print.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Range {
    /// Every page in the document.
    #[default]
    All,
    /// Only the page that was in view.
    Current,
    /// A typed from/to range, one-based and inclusive.
    Custom,
}

/// What the user asked the shell to do.
#[derive(Clone, PartialEq, Debug)]
pub enum Action {
    /// Nothing yet.
    None,
    /// Dismissed without printing.
    Cancel,
    /// Print these zero-based pages, in order.
    Print(Vec<u32>),
}

/// The print window's state.
pub struct PrintWindow {
    /// Whether the window is on screen.
    open: bool,
    /// Which pages the user picked.
    range: Range,
    /// First page of a custom range, one-based, as typed.
    from: String,
    /// Last page of a custom range, one-based, as typed.
    to: String,
}

impl PrintWindow {
    /// A window opened on the whole document.
    pub fn new(page_count: u32) -> Self {
        Self {
            open: true,
            range: Range::All,
            from: "1".to_string(),
            to: page_count.max(1).to_string(),
        }
    }

    /// A window opened with the page in view already selected.
    pub fn current_page(page_count: u32) -> Self {
        let mut window = Self::new(page_count);
        window.range = Range::Current;
        window
    }

    /// The zero-based pages the selection resolves to, or why it cannot.
    pub fn pages(&self, page_count: u32, current_page: u32) -> Result<Vec<u32>, String> {
        if page_count == 0 {
            return Err("This document has no pages.".to_string());
        }
        match self.range {
            Range::All => Ok((0..page_count).collect()),
            Range::Current => Ok(vec![current_page.min(page_count - 1)]),
            Range::Custom => {
                let first: u32 = self
                    .from
                    .trim()
                    .parse()
                    .map_err(|_| "Enter the first page number.".to_string())?;
                let last: u32 = self
                    .to
                    .trim()
                    .parse()
                    .map_err(|_| "Enter the last page number.".to_string())?;
                if first == 0 || last == 0 || first > last {
                    return Err(format!(
                        "Enter a range from 1 to {page_count}, first to last."
                    ));
                }
                if last > page_count {
                    return Err(format!("This document has {page_count} pages."));
                }
                Ok((first - 1..=last - 1).collect())
            }
        }
    }

    /// The pages the preview shows: the selection, capped so a huge job does
    /// not ask for hundreds of thumbnails.
    pub fn preview_pages(&self, page_count: u32, current_page: u32) -> Vec<u32> {
        self.pages(page_count, current_page)
            .map(|pages| pages.into_iter().take(PREVIEW_CAP).collect())
            .unwrap_or_default()
    }

    /// Draw the window. `previews` pairs each preview page with its texture,
    /// which the shell has already asked the engine for.
    pub fn show(
        &mut self,
        ctx: &Context,
        palette: &Palette,
        title: &str,
        page_count: u32,
        current_page: u32,
        previews: &[(u32, Option<egui::TextureHandle>)],
    ) -> Action {
        if !self.open {
            return Action::None;
        }

        let mut action = Action::None;
        let frame = window_frame(palette);

        let modal = egui::Modal::new(egui::Id::new("print-window"))
            .frame(frame)
            .backdrop_color(egui::Color32::from_black_alpha(130))
            .show(ctx, |ui| {
                ui.set_width(WINDOW_WIDTH);

                // Resolved once, so the summary, the button state and the
                // keyboard shortcut all agree.
                let resolved = self.pages(page_count, current_page);

                self.header(ui, palette, title, page_count);
                ui.add_space(14.0);
                divider(ui, palette);
                ui.add_space(14.0);

                ui.horizontal_top(|ui| {
                    ui.set_min_height(BODY_MIN_HEIGHT);
                    ui.vertical(|ui| {
                        ui.set_width(SETTINGS_WIDTH);
                        self.settings(ui, palette, current_page, &resolved);
                    });
                    ui.add_space(22.0);
                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width());
                        preview_pane(ui, palette, previews, page_count, current_page, self);
                    });
                });

                ui.add_space(16.0);
                divider(ui, palette);
                ui.add_space(12.0);
                self.footer(ui, palette, page_count, &resolved, &mut action);
            });

        // Escape and a click on the backdrop both mean "never mind".
        if modal.should_close() {
            return Action::Cancel;
        }
        action
    }

    /// Title and subtitle at the top of the window.
    fn header(&self, ui: &mut egui::Ui, palette: &Palette, title: &str, page_count: u32) {
        ui.horizontal(|ui| {
            ui.label(heading(palette, "Print"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                chip_frame(palette).show(ui, |ui| {
                    ui.label(
                        RichText::new(format!("{page_count} page(s)"))
                            .size(11.0)
                            .color(palette.text),
                    );
                });
            });
        });
        ui.add_space(3.0);
        ui.label(hint(palette, title));
    }

    /// The left column: which pages, and how many that comes to.
    fn settings(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        current_page: u32,
        resolved: &Result<Vec<u32>, String>,
    ) {
        section(ui, palette, "Pages");
        ui.radio_value(&mut self.range, Range::All, "All pages");
        ui.add_space(2.0);
        ui.radio_value(
            &mut self.range,
            Range::Current,
            &format!("Current page ({})", current_page + 1),
        );
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.range, Range::Custom, "Pages");
            ui.add_enabled_ui(self.range == Range::Custom, |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.from).desired_width(46.0));
                ui.label(hint(palette, "to"));
                ui.add(egui::TextEdit::singleline(&mut self.to).desired_width(46.0));
            });
        });

        ui.add_space(12.0);
        match resolved {
            Ok(pages) => {
                chip_frame(palette).show(ui, |ui| {
                    ui.label(
                        RichText::new(format!("{} page(s) will print", pages.len()))
                            .size(11.5)
                            .color(palette.text),
                    );
                });
            }
            Err(reason) => {
                ui.label(RichText::new(reason).size(11.5).color(palette.danger));
            }
        }
    }

    /// The footer: the count on the left, the actions on the right.
    fn footer(
        &self,
        ui: &mut egui::Ui,
        palette: &Palette,
        page_count: u32,
        resolved: &Result<Vec<u32>, String>,
        action: &mut Action,
    ) {
        ui.horizontal(|ui| {
            let summary = match resolved {
                Ok(pages) => format!("{} of {page_count} page(s) selected", pages.len()),
                Err(_) => "Choose a valid page range".to_string(),
            };
            ui.label(hint(palette, &summary));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let valid = resolved.is_ok();
                if ui
                    .add_enabled(valid, primary_button(palette, "Print"))
                    .clicked()
                    && let Ok(pages) = resolved
                {
                    *action = Action::Print(pages.clone());
                }
                ui.add_space(8.0);
                if ui.add(secondary_button(palette, "Cancel")).clicked() {
                    *action = Action::Cancel;
                }
            });
        });

        // Enter prints, unless the caret is in the range boxes.
        if let Ok(pages) = resolved {
            let typing = ui.memory(|memory| memory.focused().is_some());
            if !typing && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                *action = Action::Print(pages.clone());
            }
        }
    }
}

/// The preview column: one card per selected page, wrapping.
fn preview_pane(
    ui: &mut egui::Ui,
    palette: &Palette,
    previews: &[(u32, Option<egui::TextureHandle>)],
    page_count: u32,
    current_page: u32,
    window: &PrintWindow,
) {
    section(ui, palette, "Preview");
    egui::ScrollArea::vertical()
        .max_height(PREVIEW_HEIGHT)
        // Full width, but only as tall as the pages it holds: a three-page
        // document gets a compact window rather than one padded with empty
        // space.
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if previews.is_empty() {
                ui.add_space(4.0);
                ui.label(hint(palette, "No pages selected."));
                return;
            }

            egui::Grid::new("print-preview")
                .num_columns(PREVIEW_COLUMNS)
                .spacing(Vec2::new(12.0, 12.0))
                .show(ui, |ui| {
                    for (index, (page, texture)) in previews.iter().enumerate() {
                        preview_card(ui, palette, *page, texture.as_ref());
                        if (index + 1) % PREVIEW_COLUMNS == 0 {
                            ui.end_row();
                        }
                    }
                });

            if let Ok(pages) = window.pages(page_count, current_page)
                && pages.len() > PREVIEW_CAP
            {
                ui.add_space(6.0);
                ui.label(hint(
                    palette,
                    &format!("…and {} more page(s).", pages.len() - PREVIEW_CAP),
                ));
            }
        });
}

/// One page preview: the page image in a card, its number underneath.
fn preview_card(
    ui: &mut egui::Ui,
    palette: &Palette,
    page: u32,
    texture: Option<&egui::TextureHandle>,
) {
    card_frame(palette).show(ui, |ui| {
        ui.set_width(PREVIEW_WIDTH);
        ui.vertical_centered(|ui| {
            match texture {
                Some(texture) => {
                    let size = fit_into(texture.size_vec2(), PREVIEW_WIDTH);
                    ui.add(egui::Image::new(texture).fit_to_exact_size(size));
                }
                None => {
                    // Hold the page's space so the grid does not jump when the
                    // texture arrives.
                    ui.allocate_ui(Vec2::new(PREVIEW_WIDTH, PREVIEW_WIDTH * 1.3), |ui| {
                        ui.centered_and_justified(|ui| {
                            ui.spinner();
                        });
                    });
                }
            }
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("Page {}", page + 1))
                    .size(11.5)
                    .color(palette.text),
            );
        });
    });
}

/// Scale `size` to fit inside `max_width`, keeping its aspect ratio.
fn fit_into(size: Vec2, max_width: f32) -> Vec2 {
    if size.x <= 0.0 {
        return Vec2::new(max_width, max_width);
    }
    let scale = (max_width / size.x).min(1.0);
    size * scale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_pages_is_every_page() {
        let window = PrintWindow::new(5);
        assert_eq!(window.pages(5, 2).unwrap(), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn current_page_is_the_page_in_view() {
        let window = PrintWindow::current_page(10);
        assert_eq!(window.pages(10, 3).unwrap(), vec![3]);
        // A stale current page past the end still resolves to a real page.
        assert_eq!(window.pages(10, 99).unwrap(), vec![9]);
    }

    #[test]
    fn a_custom_range_is_one_based_and_inclusive() {
        let mut window = PrintWindow::new(10);
        window.range = Range::Custom;
        window.from = "3".to_string();
        window.to = "5".to_string();
        assert_eq!(window.pages(10, 0).unwrap(), vec![2, 3, 4]);
    }

    #[test]
    fn a_bad_range_is_explained() {
        let mut window = PrintWindow::new(10);
        window.range = Range::Custom;

        window.from = "x".to_string();
        window.to = "5".to_string();
        assert!(window.pages(10, 0).is_err());

        window.from = "5".to_string();
        window.to = "3".to_string();
        assert!(window.pages(10, 0).is_err(), "reversed range");

        window.from = "0".to_string();
        window.to = "3".to_string();
        assert!(window.pages(10, 0).is_err(), "pages are one-based");

        window.from = "8".to_string();
        window.to = "20".to_string();
        assert!(window.pages(10, 0).is_err(), "past the end");
    }

    #[test]
    fn an_empty_document_prints_nothing() {
        let window = PrintWindow::new(0);
        assert!(window.pages(0, 0).is_err());
        assert!(window.preview_pages(0, 0).is_empty());
    }

    /// The preview is capped, but the count the user sees is not.
    #[test]
    fn the_preview_is_capped_but_the_selection_is_not() {
        let window = PrintWindow::new(200);
        assert_eq!(window.pages(200, 0).unwrap().len(), 200);
        assert_eq!(window.preview_pages(200, 0).len(), PREVIEW_CAP);
    }
}
