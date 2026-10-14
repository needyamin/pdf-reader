//! The user guide opened from the Help menu.
//!
//! One window, six topics. Each Help entry opens this window on its topic, and
//! the list on the left is the same set, so the menu and the window cannot
//! drift apart.

use egui::{Context, RichText, Ui};
use pdfreader_core::GuideTopic;
use pdfreader_ui::{Palette, card_frame, heading, hint};

/// Open/closed state and the topic on the right.
pub struct GuideWindow {
    /// Whether the window is on screen.
    open: bool,
    /// The page shown on the right.
    topic: GuideTopic,
}

impl GuideWindow {
    /// A closed window, ready to open on Getting Started.
    pub fn new() -> Self {
        Self {
            open: false,
            topic: GuideTopic::Start,
        }
    }

    /// Open the window on `topic`, or switch to it if it is already open.
    pub fn open_on(&mut self, topic: GuideTopic) {
        self.open = true;
        self.topic = topic;
    }

    /// Draw the guide when it is open.
    pub fn draw(&mut self, ctx: &Context, palette: &Palette) {
        if !self.open {
            return;
        }
        let mut open = true;
        let mut topic = self.topic;
        pdfreader_ui::dialog_window("User Guide", palette)
            .open(&mut open)
            .resizable(true)
            .default_size([860.0, 560.0])
            .min_size([720.0, 460.0])
            .show(ctx, |ui| {
                egui::Panel::left("guide-rail")
                    .exact_size(220.0)
                    .resizable(false)
                    .frame(
                        egui::Frame::new()
                            .fill(palette.panel_bg)
                            .inner_margin(egui::Margin::symmetric(14, 16)),
                    )
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new("HELP")
                                .size(10.5)
                                .strong()
                                .color(palette.text_dim),
                        );
                        ui.add_space(10.0);
                        for item in GuideTopic::ALL {
                            if rail_item(ui, palette, item.label(), topic == item) {
                                topic = item;
                            }
                            ui.add_space(6.0);
                        }
                    });

                // The centre panel takes only the space the rail left, so the
                // topic list and the page cannot paint on top of each other.
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::new()
                            .fill(palette.window_bg)
                            .inner_margin(egui::Margin::symmetric(22, 16)),
                    )
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt(("guide-body", topic.label()))
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(heading(palette, topic.label()));
                                ui.add_space(4.0);
                                ui.label(hint(palette, topic_blurb(topic)));
                                ui.add_space(14.0);
                                for (title, body) in topic_steps(topic) {
                                    card_frame(palette).show(ui, |ui| {
                                        ui.label(
                                            RichText::new(*title)
                                                .strong()
                                                .size(13.0)
                                                .color(palette.text),
                                        );
                                        ui.add_space(6.0);
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(*body).size(13.0).color(palette.text),
                                            )
                                            .wrap(),
                                        );
                                    });
                                    ui.add_space(10.0);
                                }
                            });
                    });
            });
        self.open = open;
        self.topic = topic;
    }
}

impl Default for GuideWindow {
    fn default() -> Self {
        Self::new()
    }
}

/// One line under the topic title.
fn topic_blurb(topic: GuideTopic) -> &'static str {
    match topic {
        GuideTopic::Start => "Open a document and find your way around.",
        GuideTopic::Reading => "Zoom, page layout, rotation, and full screen.",
        GuideTopic::Annotate => "Mark the page, then adjust or remove the mark.",
        GuideTopic::Editing => "Change the words that are already in the PDF.",
        GuideTopic::Language => "Look a selection up, hear it, or open a translation.",
        GuideTopic::Share => "Keep a copy, print it, or turn it into images.",
    }
}

/// The steps for one topic. Each pair is a card title and its explanation.
fn topic_steps(topic: GuideTopic) -> &'static [(&'static str, &'static str)] {
    match topic {
        GuideTopic::Start => &[
            (
                "Open a document",
                "File → Open, or Ctrl+O, picks a PDF. Each file gets its own tab along the top. Ctrl+W closes the tab you are reading. Alt+F4 quits.",
            ),
            (
                "Move through the pages",
                "Scroll to move. Home and End jump to the first and last page. Page Up, Page Down, and Space step one page. The page box in the toolbar jumps to a number you type.",
            ),
            (
                "Sidebar",
                "Ctrl+B shows or hides the sidebar. Pages is a thumbnail strip. Outline lists the document's bookmarks, when it has any, and a click jumps there. Comments lists the annotations.",
            ),
            (
                "Find",
                "Edit → Find, or Ctrl+F, focuses the search box. Matches are listed as you type, and choosing one scrolls that page into view.",
            ),
        ],
        GuideTopic::Reading => &[
            (
                "Zoom",
                "Ctrl and + zooms in, Ctrl and − zooms out, and Ctrl+0 returns to actual size. View → Fit Width fills the window across. View → Fit Page fits the whole page. The view stays on the spot you were reading.",
            ),
            (
                "Page display",
                "View → Page Display → Continuous stacks every page in one scroll, which is the usual way to read. Single Page shows one page at a time.",
            ),
            (
                "Rotate and full screen",
                "View → Rotate View turns the pages clockwise or counterclockwise on screen. It does not rewrite the file. F11 fills the screen, and Escape leaves full screen.",
            ),
            (
                "Theme",
                "View → Theme switches the window colours. Settings → Appearance also sets the theme, the interface scale, and how documents open.",
            ),
        ],
        GuideTopic::Annotate => &[
            (
                "Choose a tool",
                "The Annotate bar under the toolbar holds the tools: Select (V), Highlight (H), Underline (U), Strikeout (S), Squiggly (Q), Rectangle (R), Note (N), and Eraser. Drag on the page to draw a markup or a rectangle. Click to place a note, then type it.",
            ),
            (
                "Adjust a mark",
                "A mark you just placed stays selected, with handles. Drag the mark to move it, and drag a handle to resize it. The Select tool does the same for a mark already on the page. Click empty page, or the margin around it, to leave adjustment: the handles go away and the properties window closes. Clicking the mark or its handles keeps adjustment.",
            ),
            (
                "Colour, undo, and comments",
                "Properties, on the Annotate bar, opens the colour window for the selected mark. Undo and Redo are on that bar and in the Edit menu. The Comments tab in the sidebar lists every annotation; click one to jump to it.",
            ),
            (
                "Remove a mark",
                "The Eraser deletes the annotation under the pointer. You can also select a mark and delete it from the Comments list. Undo puts it back.",
            ),
        ],
        GuideTopic::Editing => &[
            (
                "Turn editing on",
                "Edit, at the end of the Annotate bar, switches from marking the page to changing the PDF's own text and images. The bar then reads PDF Editing Mode Is ON, and the button becomes Done. Done leaves editing. A document that does not allow changes keeps the button disabled.",
            ),
            (
                "Select, move, and retype",
                "Click a text run to select it, then drag to move it. Click it again, or double-click, to edit the words. The PDF's own font and point size stay. Arrow keys nudge the selection; hold Shift for a larger step. Delete removes the selected object.",
            ),
            (
                "Style",
                "The text panel sets bold, size, and ink. Reset style puts back the bold, size, and colour the text had in the file. Undo and Redo restore the previous words and the previous style, including the original ink.",
            ),
            (
                "Images",
                "Click an image to select it, then drag to move it or use the handles to resize it. Replace, in the text panel, swaps the picture for another file.",
            ),
        ],
        GuideTopic::Language => &[
            (
                "Look a word up",
                "Select a word or a short phrase on the page. A card opens with the offline English ↔ Bangla meaning, and the pronunciation when the dictionary has one. A long answer wraps inside the card and scrolls, so it is not cut off at the window edge.",
            ),
            (
                "Speak",
                "Speak, at the top of the card, pronounces the selection. While the voice is playing the speaker icon shows sound waves. Click it again to stop. The icon returns to a plain speaker when the voice finishes.",
            ),
            (
                "Google Translate",
                "Google Translate, at the bottom of the card, opens the selection in the browser. Settings → Language tools turns Speak, speak-on-select, and Translate on or off, and chooses the language the translate link uses.",
            ),
        ],
        GuideTopic::Share => &[
            (
                "Save",
                "Ctrl+S writes the document you are reading, including annotations and text edits. Ctrl+Shift+S saves a copy under a new name and leaves the original where it was.",
            ),
            (
                "Print",
                "File → Print, or Ctrl+P, opens the print window. Choose all pages or the current page, then print.",
            ),
            (
                "Export",
                "Tools, or Ctrl+E, opens the export window. You can write pages out as PNG or JPEG, build a PDF from images, or merge several PDFs into one, in the order you arrange.",
            ),
            (
                "Forms",
                "Click a form field and type. Tab moves to the next field. Settings → Text sets the font, size, colour, and weight used for what you type into fields.",
            ),
        ],
    }
}

/// A full-width row in the topic list. Returns whether it was clicked.
fn rail_item(ui: &mut Ui, palette: &Palette, label: &str, selected: bool) -> bool {
    let (fill, text) = if selected {
        (palette.accent_soft, palette.text)
    } else {
        (egui::Color32::TRANSPARENT, palette.text_dim)
    };
    let button = egui::Button::new(RichText::new(label).size(13.0).color(text))
        .fill(fill)
        .corner_radius(palette.radius)
        .min_size(egui::vec2(ui.available_width(), 32.0));
    ui.add(button).clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_help_topic_explains_something() {
        for topic in GuideTopic::ALL {
            assert!(!topic.label().is_empty(), "{topic:?}");
            assert!(!topic_blurb(topic).is_empty(), "{topic:?}");
            let steps = topic_steps(topic);
            assert!(
                steps.len() >= 3,
                "{} should have several steps",
                topic.label()
            );
            for (title, body) in steps {
                assert!(!title.is_empty());
                assert!(body.len() > 40, "{title} is too short to be useful: {body}");
            }
        }
    }
}
