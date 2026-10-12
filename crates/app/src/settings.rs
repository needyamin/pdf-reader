//! The full Settings window.
//!
//! A fixed category rail on the left, a fixed action bar along the bottom, and
//! the selected category's options in a scrollable centre. Preferences live on
//! [`App`] — the shell owns them and writes them to `settings.json` — so this
//! module only draws controls and turns a change into a [`Command`] or a direct
//! field update. It is presentation, exactly like the menu bar, but it needs
//! the app's own state, which is why it lives beside the shell rather than in
//! the `ui` crate.

use egui::{Align, Context, Frame, Layout, Margin, RichText, Sense, Stroke, Ui, Vec2};
use pdfreader_core::{Command, ThemeId, ViewMode, ZoomMode};
use pdfreader_ui::{
    Palette, card_frame, chip_frame, chip_radius, divider, heading, hint, image_frame,
    primary_button, secondary_button, section,
};

use crate::App;
use std::sync::OnceLock;

/// One page of the Settings window.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// Theme, interface scale and layout.
    Appearance,
    /// How documents open.
    Reading,
    /// The word-lookup helpers and the translate target.
    Language,
    /// Font family and size for form text.
    Text,
    /// Prefetching and the diagnostics overlay.
    Performance,
}

impl Category {
    /// Every category, in rail order.
    const ALL: [Category; 5] = [
        Category::Appearance,
        Category::Reading,
        Category::Language,
        Category::Text,
        Category::Performance,
    ];

    /// Rail label.
    const fn label(self) -> &'static str {
        match self {
            Category::Appearance => "Appearance",
            Category::Reading => "Reading",
            Category::Language => "Language tools",
            Category::Text => "Text",
            Category::Performance => "Performance",
        }
    }

    /// One-line description shown under the page title.
    const fn blurb(self) -> &'static str {
        match self {
            Category::Appearance => "Theme, interface scale and layout.",
            Category::Reading => "How documents open.",
            Category::Language => "The lookup helpers and the translate target.",
            Category::Text => "Font family and size for text typed into forms.",
            Category::Performance => "Prefetching and the diagnostics overlay.",
        }
    }
}

/// Open/closed state and selected category of the Settings window.
pub struct SettingsWindow {
    /// Whether the window is on screen.
    pub open: bool,
    /// The page shown on the right.
    category: Category,
}

impl SettingsWindow {
    /// A closed window showing Appearance.
    pub fn new() -> Self {
        Self {
            open: false,
            category: Category::Appearance,
        }
    }
}

impl Default for SettingsWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsWindow {
    /// Select a page by name (case-insensitive), for the `PDFREADER_SETTINGS_PAGE`
    /// screenshot helper. Unknown names fall back to Appearance.
    pub fn select_by_name(&mut self, name: &str) {
        self.category = match name.to_ascii_lowercase().as_str() {
            "reading" => Category::Reading,
            "language" => Category::Language,
            "text" => Category::Text,
            "performance" => Category::Performance,
            _ => Category::Appearance,
        };
    }
}

/// Rail width. Wide enough for the longest label ("Language tools").
const RAIL_WIDTH: f32 = 190.0;
/// Action-bar height.
const FOOTER_HEIGHT: f32 = 52.0;

/// Google Translate targets offered in the Language page.
const TARGETS: &[(&str, &str)] = &[
    ("bn", "Bangla"),
    ("en", "English"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("de", "German"),
    ("ar", "Arabic"),
    ("hi", "Hindi"),
    ("zh-CN", "Chinese (Simplified)"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("ru", "Russian"),
    ("pt", "Portuguese"),
];

/// The keyboard-shortcuts reference (Help → Keyboard Shortcuts), grouped the
/// way the menu bar is.
const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    (
        "File",
        &[
            ("Ctrl+O", "Open a document"),
            ("Ctrl+S", "Save"),
            ("Ctrl+Shift+S", "Save As"),
            ("Ctrl+P", "Print"),
            ("Ctrl+E", "Open the export window"),
            ("Ctrl+W", "Close the current tab"),
            ("Alt+F4", "Exit"),
        ],
    ),
    (
        "Edit",
        &[
            ("Ctrl+Z", "Undo"),
            ("Ctrl+Shift+Z  /  Ctrl+Y", "Redo"),
            ("Ctrl+F", "Find"),
        ],
    ),
    (
        "View",
        &[
            ("Ctrl+=", "Zoom in"),
            ("Ctrl+-", "Zoom out"),
            ("Ctrl+0", "Actual size"),
            ("Ctrl+B", "Show or hide the sidebar"),
            ("F11", "Full screen"),
        ],
    ),
    (
        "Go",
        &[
            ("Home", "First page"),
            ("End", "Last page"),
            ("Page Up", "Previous page"),
            ("Page Down", "Next page"),
            ("Space", "Next page"),
            ("Shift+Space", "Previous page"),
        ],
    ),
    (
        "General",
        &[("Escape", "Cancel the current tool, or leave full screen")],
    ),
];

/// Display name for a theme.
const fn theme_label(id: ThemeId) -> &'static str {
    match id {
        ThemeId::AnsNew => "AnsNew",
        ThemeId::Dark => "Dark",
        ThemeId::Light => "Light",
        ThemeId::Midnight => "Midnight",
        ThemeId::Rose => "Rose",
        ThemeId::Forest => "Forest",
        ThemeId::Sunset => "Sunset",
    }
}

impl App {
    /// Draw the Settings window when it is open.
    ///
    /// Commands the controls produce are applied once, after the window closes,
    /// so the store is not mutated while it is being read for layout.
    ///
    /// The function is long because it lays out three nested panels (rail,
    /// footer, scrolling centre) in one place; splitting it would scatter the
    /// window's structure across helpers for no gain.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn draw_settings_window(&mut self, ctx: &Context, palette: &Palette) {
        if !self.settings_window.open {
            return;
        }

        let mut open = true;
        let mut close_requested = false;
        let mut reset = false;
        let mut category = self.settings_window.category;
        let mut commands: Vec<Command> = Vec::new();
        let mut changed = false;

        pdfreader_ui::dialog_window("Settings", palette)
            .open(&mut open)
            .resizable(true)
            .default_size([900.0, 580.0])
            .min_size([780.0, 500.0])
            .show(ctx, |ui| {
                // Left rail: the category list.
                egui::Panel::left("settings-rail")
                    .exact_size(RAIL_WIDTH)
                    .resizable(false)
                    .frame(
                        egui::Frame::new()
                            .fill(palette.panel_bg)
                            .inner_margin(egui::Margin::symmetric(10, 12)),
                    )
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new("SETTINGS")
                                .size(10.5)
                                .strong()
                                .color(palette.text_dim),
                        );
                        ui.add_space(8.0);
                        for cat in Category::ALL {
                            if rail_item(ui, palette, cat.label(), category == cat) {
                                category = cat;
                            }
                            ui.add_space(3.0);
                        }
                    });

                // Bottom action bar: reset on the left, done on the right.
                egui::Panel::bottom("settings-footer")
                    .exact_size(FOOTER_HEIGHT)
                    .resizable(false)
                    .frame(
                        egui::Frame::new()
                            .fill(palette.panel_bg)
                            .inner_margin(egui::Margin::symmetric(14, 9)),
                    )
                    .show(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            if ui
                                .add(secondary_button(palette, "Reset to defaults"))
                                .clicked()
                            {
                                reset = true;
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.add(primary_button(palette, "Done")).clicked() {
                                    close_requested = true;
                                }
                            });
                        });
                    });

                // Centre: the selected page, scrolling if it is tall.
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::new()
                            .fill(palette.window_bg)
                            .inner_margin(egui::Margin::symmetric(22, 16)),
                    )
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(heading(palette, category.label()));
                                ui.add_space(2.0);
                                ui.label(hint(palette, category.blurb()));
                                ui.add_space(6.0);

                                match category {
                                    Category::Appearance => {
                                        self.page_appearance(
                                            ui,
                                            palette,
                                            &mut commands,
                                            &mut changed,
                                        );
                                    }
                                    Category::Reading => {
                                        self.page_reading(ui, palette, &mut commands, &mut changed);
                                    }
                                    Category::Language => {
                                        self.page_language(ui, palette, &mut commands);
                                    }
                                    Category::Text => {
                                        self.page_text(ui, palette, &mut commands);
                                    }
                                    Category::Performance => {
                                        self.page_performance(ui, palette, &mut changed);
                                    }
                                }
                                ui.add_space(8.0);
                            });
                    });
            });

        if close_requested {
            open = false;
        }
        self.settings_window.open = open;
        self.settings_window.category = category;

        if reset {
            self.reset_settings(ctx);
            return;
        }
        if !commands.is_empty() {
            self.apply(ctx, commands);
            changed = true;
        }
        if changed {
            if (ctx.zoom_factor() - self.ui_scale).abs() > 1e-4 {
                ctx.set_zoom_factor(self.ui_scale);
            }
            self.persist();
        }
    }

    /// Appearance: theme, interface scale, layout.
    fn page_appearance(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        commands: &mut Vec<Command>,
        changed: &mut bool,
    ) {
        section(ui, palette, "Theme");
        let current = self.store.state().theme;
        ui.horizontal_wrapped(|ui| {
            for id in ThemeId::ALL {
                if ui.radio(current == id, theme_label(id)).clicked() && current != id {
                    commands.push(Command::SetTheme(id));
                }
            }
        });
        ui.label(hint(
            palette,
            "Applies to the whole window, including the pages you are reading.",
        ));
        ui.add_space(10.0);

        section(ui, palette, "Interface scale");
        let before = self.ui_scale;
        ui.add(
            egui::Slider::new(&mut self.ui_scale, 0.75..=2.0)
                .step_by(0.05)
                .suffix("×"),
        );
        if (self.ui_scale - before).abs() > 1e-4 {
            *changed = true;
        }
        ui.label(hint(
            palette,
            "Scales the whole interface. 1.00× is the default; raise it on a small or high-DPI screen.",
        ));
        ui.add_space(10.0);

        section(ui, palette, "Layout");
        let mut sidebar = self.store.state().sidebar_visible;
        if ui
            .checkbox(&mut sidebar, "Show the sidebar on startup")
            .changed()
        {
            commands.push(Command::ToggleSidebar);
        }
        ui.label(hint(
            palette,
            "The sidebar holds thumbnails, the outline, search results and comments.",
        ));
    }

    /// Text: the font used for form field text.
    fn page_text(&mut self, ui: &mut egui::Ui, palette: &Palette, commands: &mut Vec<Command>) {
        section(ui, palette, "Font family");
        let family_label = pdfreader_ui::FORM_FONTS
            .iter()
            .find(|(name, _)| *name == self.form_font)
            .map_or("Default", |(_, label)| *label);
        egui::ComboBox::from_id_salt("form-font")
            .width(240.0)
            .selected_text(family_label)
            .show_ui(ui, |ui| {
                for &(name, label) in pdfreader_ui::FORM_FONTS {
                    if ui.selectable_label(self.form_font == name, label).clicked() {
                        commands.push(Command::SetFormFont(name.to_owned()));
                    }
                }
            });
        ui.label(hint(
            palette,
            "Face used for text typed into form fields. Pick one that covers the script you fill in — Bengali, Arabic and the rest are bundled.",
        ));
        ui.add_space(14.0);

        section(ui, palette, "Font size");
        let mut size = self.form_font_size;
        ui.add(
            egui::Slider::new(&mut size, 0.0..=48.0)
                .step_by(0.5)
                .custom_formatter(|value, _| {
                    if value <= 0.0 {
                        "Auto".to_owned()
                    } else {
                        format!("{value:.1} pt")
                    }
                }),
        );
        if (size - self.form_font_size).abs() > f32::EPSILON {
            commands.push(Command::SetFormFontSize(size));
        }
        ui.label(hint(
            palette,
            "Size for filled field text. Auto fits each field's own height, which is often only 10–14 pt.",
        ));
        ui.add_space(14.0);

        section(ui, palette, "Colour");
        ui.horizontal(|ui| {
            let mut rgb = self.form_font_color;
            if ui.color_edit_button_srgb(&mut rgb).changed() {
                commands.push(Command::SetFormFontColor(rgb[0], rgb[1], rgb[2]));
            }
            ui.label(hint(palette, "Ink for filled field text."));
        });
        ui.add_space(14.0);

        section(ui, palette, "Weight");
        let mut bold = self.form_font_bold;
        if ui
            .checkbox(&mut bold, "Bold")
            .on_hover_text("Draw typed text heavier")
            .changed()
        {
            commands.push(Command::SetFormFontBold(bold));
        }
        ui.label(hint(
            palette,
            "No bold face is bundled, so this thickens the letters rather than switching typeface.",
        ));
        ui.add_space(14.0);

        section(ui, palette, "Preview");
        let preview = if self.form_font_size > 0.0 {
            self.form_font_size
        } else {
            14.0
        };
        let family = if self.form_font.is_empty() {
            egui::FontFamily::Proportional
        } else {
            egui::FontFamily::Name(self.form_font.clone().into())
        };
        let ink = egui::Color32::from_rgb(
            self.form_font_color[0],
            self.form_font_color[1],
            self.form_font_color[2],
        );
        card_frame(palette).show(ui, |ui| {
            // The preview shows the same synthetic bold the canvas uses, so the
            // choice can be judged here rather than in a field.
            let mut text = RichText::new("AnsNew PDF Reader  ·  বাংলা  ·  English")
                .font(egui::FontId::new(preview, family.clone()))
                .color(ink);
            if self.form_font_bold {
                text = text.strong();
            }
            ui.label(text);
        });
    }

    /// Reading: how documents open.
    fn page_reading(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        commands: &mut Vec<Command>,
        changed: &mut bool,
    ) {
        section(ui, palette, "Default view mode");
        let mut mode = self.default_view_mode;
        ui.horizontal(|ui| {
            ui.radio_value(&mut mode, ViewMode::Continuous, "Continuous scroll");
            ui.radio_value(&mut mode, ViewMode::Single, "Single page");
        });
        if mode != self.default_view_mode {
            self.default_view_mode = mode;
            commands.push(Command::SetViewMode(mode));
            *changed = true;
        }
        ui.label(hint(
            palette,
            "Applied to every document you open. The current document changes immediately.",
        ));
        ui.add_space(10.0);

        section(ui, palette, "Default zoom");
        let mut zoom = self.default_zoom;
        ui.horizontal(|ui| {
            ui.radio_value(&mut zoom, ZoomMode::FitWidth, "Fit width");
            ui.radio_value(&mut zoom, ZoomMode::FitPage, "Fit page");
            ui.radio_value(&mut zoom, ZoomMode::Fixed(1.0), "100%");
        });
        if zoom != self.default_zoom {
            self.default_zoom = zoom;
            commands.push(Command::SetZoom(zoom));
            *changed = true;
        }
        ui.label(hint(
            palette,
            "Fit modes re-scale as the window resizes; 100% shows the page at its true size.",
        ));
        ui.add_space(10.0);

        section(ui, palette, "Window");
        let mut maximized = self.open_maximized;
        if ui.checkbox(&mut maximized, "Open maximized").changed() {
            self.open_maximized = maximized;
            *changed = true;
        }
        ui.label(hint(
            palette,
            "Open filling the screen with the title bar and its window buttons still visible — on by default. F11 toggles true full screen.",
        ));
    }

    /// Language tools: the lookup helpers and the translate target.
    ///
    /// Every control here emits a command, so the caller's `commands` vec is
    /// what signals that something changed.
    fn page_language(&mut self, ui: &mut egui::Ui, palette: &Palette, commands: &mut Vec<Command>) {
        section(ui, palette, "Word lookup");
        let mut lookup = self.lookup_enabled;
        if ui
            .checkbox(&mut lookup, "Word lookup (offline dictionary)")
            .changed()
        {
            commands.push(Command::ToggleLookup);
        }
        ui.label(hint(
            palette,
            "Click a word or drag to select a phrase to open the meaning card.",
        ));
        ui.add_space(6.0);

        let mut speak = self.speak_enabled;
        if ui.checkbox(&mut speak, "Speak (text to speech)").changed() {
            commands.push(Command::ToggleSpeak);
        }
        ui.label(hint(
            palette,
            "Adds a Speak button that reads the selection with the system voice.",
        ));
        ui.add_space(4.0);

        let mut auto_speak = self.auto_speak;
        if ui
            .add_enabled(
                self.speak_enabled,
                egui::Checkbox::new(&mut auto_speak, "Speak automatically on selection"),
            )
            .changed()
        {
            commands.push(Command::ToggleAutoSpeak);
        }
        ui.label(hint(
            palette,
            "Reads a selected word or phrase aloud the moment the card opens. Needs Speak to be on.",
        ));
        ui.add_space(10.0);

        let mut translate = self.translate_enabled;
        if ui
            .checkbox(&mut translate, "Google Translate link")
            .changed()
        {
            commands.push(Command::ToggleTranslate);
        }
        ui.label(hint(
            palette,
            "Adds a link that opens the exact selected text in your browser.",
        ));
        ui.add_space(10.0);

        section(ui, palette, "Google Translate target language");
        let target = self.lens_target.clone();
        let selected_text = TARGETS
            .iter()
            .find(|&&(code, _)| code == target.as_str())
            .map_or(target.as_str(), |&(_, label)| label)
            .to_owned();
        egui::ComboBox::from_id_salt("lens-target")
            .width(220.0)
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                for &(code, label) in TARGETS {
                    if ui
                        .selectable_label(self.lens_target == code, label)
                        .clicked()
                    {
                        commands.push(Command::SetLensTarget(code.to_owned()));
                    }
                }
            });
        ui.label(hint(
            palette,
            "The offline dictionary is always English↔Bangla; this only sets where the translate link sends the text.",
        ));
    }

    /// Performance: prefetching and the diagnostics overlay.
    fn page_performance(&mut self, ui: &mut egui::Ui, palette: &Palette, changed: &mut bool) {
        section(ui, palette, "Rendering");
        let mut prefetch = self.prefetch;
        if ui.checkbox(&mut prefetch, "Preload nearby pages").changed() {
            self.prefetch = prefetch;
            self.canvas.set_prefetch(prefetch);
            *changed = true;
        }
        ui.label(hint(
            palette,
            "Requests tiles just outside the viewport so scrolling stays smooth. Off saves memory and bandwidth.",
        ));
        ui.add_space(10.0);

        section(ui, palette, "Diagnostics");
        let mut stats = self.show_stats;
        if ui
            .checkbox(&mut stats, "Show the performance overlay")
            .changed()
        {
            self.show_stats = stats;
            *changed = true;
        }
        ui.label(hint(
            palette,
            "Draws frames per second, cached tiles and zoom in the corner of the view.",
        ));
    }

    /// Restore every preference to its default and persist.
    fn reset_settings(&mut self, ctx: &Context) {
        self.ui_scale = 1.0;
        self.default_view_mode = ViewMode::Continuous;
        self.default_zoom = ZoomMode::FitWidth;
        self.show_stats = false;
        self.prefetch = true;
        self.form_font.clear();
        self.form_font_size = 0.0;
        self.form_font_bold = false;
        self.form_font_color = crate::canvas::DEFAULT_FORM_INK;
        self.sync_form_font();
        self.open_maximized = true;
        self.lookup_enabled = true;
        self.speak_enabled = true;
        self.auto_speak = true;
        self.translate_enabled = true;
        pdfreader_lens::DEFAULT_TARGET.clone_into(&mut self.lens_target);
        self.canvas.set_prefetch(true);
        ctx.set_zoom_factor(1.0);

        let mut commands = vec![Command::SetTheme(ThemeId::default())];
        // The default layout has the sidebar hidden; hide it if it is showing.
        if self.store.state().sidebar_visible {
            commands.push(Command::ToggleSidebar);
        }
        self.apply(ctx, commands);
        self.persist();
    }

    /// Draw the optional performance overlay in the top-right of the view.
    pub(crate) fn draw_stats_overlay(&self, ctx: &Context, palette: &Palette) {
        let (zoom, page) = self
            .store
            .state()
            .active()
            .map_or((0.0, 0), |t| (t.view.zoom, t.view.current_page));
        let text = format!(
            "{} fps\n{} tiles cached · {} in flight\nzoom {:.0}% · page {}\nUI {:.0}%",
            self.frame_count,
            self.canvas.cached_tiles(),
            self.canvas.inflight_count(),
            zoom * 100.0,
            page + 1,
            self.ui_scale * 100.0,
        );

        egui::Area::new(egui::Id::new("stats-overlay"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_TOP, [-12.0, 108.0])
            .interactable(false)
            .show(ctx, |ui| {
                pdfreader_ui::toast_frame(palette, palette.border)
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(text)
                                .size(11.0)
                                .monospace()
                                .color(palette.text),
                        );
                    });
            });
    }

    /// Draw the keyboard-shortcuts reference (Help → Keyboard Shortcuts).
    pub(crate) fn draw_shortcuts_window(&mut self, ctx: &Context, palette: &Palette) {
        if !self.shortcuts_open {
            return;
        }
        let mut open = true;
        pdfreader_ui::dialog_window("Keyboard Shortcuts", palette)
            .open(&mut open)
            .resizable(true)
            .default_size([880.0, 700.0])
            .min_size([640.0, 480.0])
            .show(ctx, |ui| {
                draw_shortcuts_content(ui, palette);
            });
        self.shortcuts_open = open;
    }
}

/// The body of the Keyboard Shortcuts window, also exercised by tests.
///
/// The five shortcut groups are laid out as a uniform 3×2 tile grid: every card
/// is the same width (equal `ui.columns`) and the same height (forced to the
/// tallest card), so the reference reads as a clean, evenly sized grid that
/// fills the window width edge to edge — no mismatched widths, no ragged edges.
pub(crate) fn draw_shortcuts_content(ui: &mut Ui, palette: &Palette) {
    ui.label(hint(
        palette,
        "Every shortcut the reader answers, grouped the way the menu bar is.",
    ));
    ui.add_space(12.0);

    // Fixed 3-column grid: the user picked a 3×2 layout, so the cards are a
    // comfortable width and the grid is perfectly even.
    const COLUMNS: usize = 3;

    // Force every card to the same height so the row of tiles is level.
    let card_h = uniform_card_height(palette);

    for chunk in SHORTCUTS.chunks(COLUMNS) {
        ui.columns(COLUMNS, |cols| {
            for (j, (group, rows)) in chunk.iter().enumerate() {
                cols[j].vertical(|ui| {
                    card_frame(palette).show(ui, |ui| {
                        // Pin the inner height so each card is exactly the same
                        // size; shorter groups just get a little empty space at
                        // the bottom rather than a shorter card.
                        ui.set_min_height(card_h);
                        render_shortcut_card(ui, palette, *group, *rows);
                    });
                });
            }
        });
    }
}

/// Draw a single shortcut group as a card body: a branded header (accent rule
/// + group name) followed by one row per shortcut. Extracted from the window
/// layout so the uniform-height measurement reuses exactly the same drawing
/// code the window uses — there is only one source of truth for a card's size.
fn render_shortcut_card(ui: &mut Ui, palette: &Palette, group: &str, rows: &[(&str, &str)]) {
    ui.horizontal(|ui| {
        let (bar, _) = ui.allocate_exact_size(Vec2::new(3.0, 13.0), Sense::hover());
        ui.painter().rect_filled(bar, 1.5, palette.accent);
        ui.add_space(7.0);
        ui.label(
            RichText::new(group)
                .strong()
                .size(13.0)
                .color(palette.text),
        );
    });
    ui.add_space(2.0);
    divider(ui, palette);
    ui.add_space(6.0);

    for (keys, action) in rows {
        ui.horizontal(|ui| {
            draw_keycombo(ui, palette, keys);
            ui.add(
                egui::Label::new(RichText::new(*action).size(12.0).color(palette.text)).wrap(),
            );
        });
        ui.add_space(3.0);
    }
}

/// The height every shortcut card is forced to, so the grid is perfectly level.
///
/// It is the tallest group's natural content height, measured once with the
/// same `render_shortcut_card` the window draws. The height depends only on the
/// (static) shortcut list and the fixed font sizes, not on the theme, so the
/// result is cached. A small buffer absorbs a description that wraps to two
/// lines at a given width, so no card ever overflows its uniform height.
fn uniform_card_height(palette: &Palette) -> f32 {
    static CACHE: OnceLock<f32> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let ctx = Context::default();
        let mut tallest = 0.0_f32;
        for (group, rows) in SHORTCUTS {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(300.0, 4000.0),
                )),
                ..Default::default()
            };
            let mut measured = 0.0_f32;
            let mut out = ctx.run_ui(raw, |ui| {
                measured = ui
                    .allocate_ui(egui::vec2(300.0, 4000.0), |ui| {
                        render_shortcut_card(ui, palette, *group, *rows);
                        ui.min_rect().height()
                    })
                    .inner;
            });
            // Egui hands back texture deltas the caller must consume.
            out.textures_delta.clear();
            tallest = tallest.max(measured);
        }
        tallest + 14.0
    })
}

/// One key on a keycap: a small rounded chip. Modifier keys (Ctrl, Shift, Alt…)
/// take the soft accent fill so a combination reads as "modifiers + key", the
/// way professional shortcut tables do.
fn keycap(ui: &mut Ui, palette: &Palette, label: &str) {
    let modifier = matches!(
        label,
        "Ctrl" | "Shift" | "Alt" | "Win" | "Cmd" | "Meta" | "Opt" | "Super"
    );
    let fill = if modifier {
        palette.accent_soft
    } else {
        palette.panel_bg
    };
    Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(chip_radius(palette))
        .inner_margin(Margin::symmetric(4, 1))
        .show(ui, |ui| {
            ui.label(
                RichText::new(label)
                    .monospace()
                    .size(10.5)
                    .color(palette.text),
            );
        });
}

/// Render a shortcut string such as `"Ctrl+Shift+S"` or
/// `"Ctrl+Shift+Z  /  Ctrl+Y"` as a row of keycap chips, with faint `+` between
/// the keys of one combination and a faint `/` between alternatives.
fn draw_keycombo(ui: &mut Ui, palette: &Palette, combo: &str) {
    let alternatives: Vec<&str> = combo.split("  /  ").collect();
    ui.horizontal(|ui| {
        for (ai, alt) in alternatives.iter().enumerate() {
            if ai > 0 {
                ui.label(RichText::new("/").weak().size(11.0).color(palette.text_dim));
                ui.add_space(4.0);
            }
            let keys: Vec<&str> = alt.split('+').map(str::trim).collect();
            for (ki, key) in keys.iter().enumerate() {
                if ki > 0 {
                    ui.label(RichText::new("+").weak().size(11.0).color(palette.text_dim));
                    ui.add_space(1.0);
                }
                keycap(ui, palette, key);
            }
        }
    });
}

/// A full-width selectable row in the category rail. Returns whether it was
/// clicked this frame.
fn rail_item(ui: &mut egui::Ui, palette: &Palette, label: &str, selected: bool) -> bool {
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

// --- About Us window ---------------------------------------------------------
//
// The same window chrome, palette and buttons as every other window in the
// reader. It used to pin its own light visuals and a set of hardcoded hex
// colours copied from another product, which left it the one surface that did
// not follow the theme the user picked: switch to Sunset and the About window
// stayed stubbornly blue-on-white. Everything here now comes from `palette`, so
// it is the reader's theme or nothing.

/// The profile photo, embedded so the window works in the packaged build.
const PROFILE_JPG: &[u8] = include_bytes!("../../../assets/profile.jpg");

/// Links shown in the "Profile Links" block.
const PROFILE_LINKS: &[(&str, &str)] = &[
    ("GitHub", "https://github.com/needyamin"),
    ("Website", "https://needyamin.github.io"),
    ("ORCID", "https://orcid.org/0009-0009-1184-6005"),
    ("Facebook", "https://facebook.com/needyaminofficial"),
];

/// The GitHub profile the "Visit GitHub" button opens.
const PROFILE_GITHUB: &str = "https://github.com/needyamin";

/// Who makes the reader and where to find them. Written in the "About this app"
/// card as a sentence with the company name as its hyperlink.
const COMPANY_NAME: &str = "AnsNew Tech";
const COMPANY_URL: &str = "https://inside.ansnew.com/";

/// Size the profile photo is drawn at.
const PHOTO_SIZE: f32 = 76.0;

/// Decode the embedded profile photo once.
fn load_profile_texture(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let image = image::load_from_memory(PROFILE_JPG).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Some(ctx.load_texture("profile-photo", color, egui::TextureOptions::LINEAR))
}

impl App {
    /// Draw the "About Us" window.
    ///
    /// Long because it is one continuous layout — header, links, blurb, action
    /// bar — and splitting it would scatter that for no gain.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn draw_about_window(&mut self, ctx: &egui::Context, palette: &Palette) {
        if !self.about_open {
            return;
        }
        if self.profile_texture.is_none() {
            self.profile_texture = load_profile_texture(ctx);
        }
        let photo = self.profile_texture.clone();
        let version = env!("CARGO_PKG_VERSION");

        let mut open = self.about_open;
        let mut close = false;
        let mut visit: Option<&str> = None;

        pdfreader_ui::dialog_window("About Us", palette)
            .open(&mut open)
            // A fixed, comfortable size: the profile is static, so resizing
            // gains nothing, and the body scrolls if the screen is short.
            .fixed_size([680.0, 610.0])
            .show(ctx, |ui| {
                // The actions stay pinned to the bottom of the window; only the
                // profile scrolls, so a small window never hides the buttons.
                egui::Panel::bottom("about-footer")
                    .resizable(false)
                    .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(0, 8)))
                    .show(ui, |ui| {
                        divider(ui, palette);
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.add(primary_button(palette, "Visit GitHub")).clicked() {
                                visit = Some(PROFILE_GITHUB);
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.add(secondary_button(palette, "Close")).clicked() {
                                        close = true;
                                    }
                                },
                            );
                        });
                    });

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());

                        // --- the profile card ------------------------------
                        card_frame(palette).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal_top(|ui| {
                                if let Some(texture) = &photo {
                                    image_frame(palette).show(ui, |ui| {
                                        ui.add(
                                            egui::Image::new((
                                                texture.id(),
                                                egui::vec2(PHOTO_SIZE, PHOTO_SIZE),
                                            ))
                                            .corner_radius(palette.radius),
                                        );
                                    });
                                    ui.add_space(16.0);
                                }
                                ui.vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(heading(palette, "Md. Yamin Hossain"));
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                chip_frame(palette).show(ui, |ui| {
                                                    ui.label(
                                                        RichText::new(format!("v{version}"))
                                                            .size(11.0)
                                                            .color(palette.text),
                                                    );
                                                });
                                            },
                                        );
                                    });
                                    ui.add_space(3.0);
                                    ui.label(
                                        RichText::new("Software Engineer")
                                            .size(13.5)
                                            .strong()
                                            .color(palette.accent),
                                    );
                                    ui.add_space(2.0);
                                    ui.label(
                                        RichText::new("Dhaka, Bangladesh")
                                            .size(12.0)
                                            .color(palette.text_dim),
                                    );
                                    ui.add_space(10.0);
                                    ui.label(
                                        RichText::new(
                                            "Scalable systems, infrastructure automation, \
                                             and intuitive user experiences.",
                                        )
                                        .size(12.5)
                                        .color(palette.text),
                                    );
                                });
                            });
                        });

                        // --- profile links --------------------------------
                        ui.add_space(12.0);
                        section(ui, palette, "Profile Links");
                        card_frame(palette).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            egui::Grid::new("about-links")
                                .num_columns(2)
                                .spacing([20.0, 7.0])
                                .show(ui, |ui| {
                                    for &(label, url) in PROFILE_LINKS {
                                        ui.label(
                                            RichText::new(label)
                                                .strong()
                                                .size(12.0)
                                                .color(palette.text_dim),
                                        );
                                        if ui
                                            .link(
                                                RichText::new(url).size(12.5).color(palette.accent),
                                            )
                                            .clicked()
                                        {
                                            visit = Some(url);
                                        }
                                        ui.end_row();
                                    }
                                });
                        });

                        // --- about this app -------------------------------
                        ui.add_space(12.0);
                        section(ui, palette, "About this app");
                        card_frame(palette).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new(
                                    "AnsNew PDF Reader is created and maintained by \
                                     Md. Yamin Hossain. This open-source desktop app focuses on \
                                     fast, offline PDF reading with a clean user experience.",
                                )
                                .size(12.5)
                                .color(palette.text),
                            );
                            ui.add_space(6.0);
                            // The company credit, with the company name as its
                            // own hyperlink — the same `visit` route every other
                            // link in this window takes, so it opens the system
                            // browser and logs a warning instead of failing
                            // silently if that is blocked.
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    RichText::new("This product is made by ")
                                        .size(12.5)
                                        .color(palette.text),
                                );
                                if ui
                                    .link(
                                        RichText::new(COMPANY_NAME)
                                            .size(12.5)
                                            .strong()
                                            .color(palette.accent),
                                    )
                                    .on_hover_text(COMPANY_URL)
                                    .clicked()
                                {
                                    visit = Some(COMPANY_URL);
                                }
                                ui.label(
                                    RichText::new(".")
                                        .size(12.5)
                                        .color(palette.text),
                                );
                            });
                        });

                        ui.add_space(8.0);
                    });
            });

        if let Some(url) = visit
            && let Err(error) = pdfreader_platform::url::open(url)
        {
            tracing::warn!("could not open {url} ({error})");
        }
        if !open || close {
            self.about_open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, RawInput, Rect};

    /// The shortcuts reference must fit inside the default window in one view —
    /// no scrolling, no overflow. The real content is laid out inside a generous
    /// area and the space it actually uses is read back from `min_rect`.
    #[test]
    fn shortcuts_reference_fits_default_window() {
        let ctx = Context::default();
        let palette = pdfreader_ui::Theme::from_id(ThemeId::AnsNew).palette;

        // Default window is 880x700; the content area is that minus the 18px
        // frame margin on every side and the ~30px title bar.
        let content_w = 880.0 - 36.0;
        let content_h = 700.0 - 36.0 - 30.0;

        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1000.0, 3000.0))),
            ..Default::default()
        };
        let mut measured: (f32, f32) = (0.0, 0.0);
        let mut out = ctx.run_ui(raw, |ui| {
            measured = ui
                .allocate_ui(egui::vec2(content_w, 3000.0), |ui| {
                    draw_shortcuts_content(ui, &palette);
                    (ui.min_rect().width(), ui.min_rect().height())
                })
                .inner;
        });
        // Egui hands back texture deltas that the test harness must consume.
        out.textures_delta.clear();
        let (used_w, used_h) = measured;

        assert!(
            used_w <= content_w + 1.0,
            "shortcuts overflow width: used {used_w:.0} > {content_w:.0}"
        );
        // The grid must also fill the width edge to edge — "fully cover the GUI"
        // means no side margin gap. Equal `ui.columns` should span the whole
        // content area, so the used width must be essentially the full width.
        assert!(
            used_w >= content_w - 2.0,
            "shortcuts leave a side gap: used {used_w:.0} < {content_w:.0}"
        );
        assert!(
            used_h <= content_h + 1.0,
            "shortcuts overflow height: used {used_h:.0} > {content_h:.0}"
        );
    }
}
