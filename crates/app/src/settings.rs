//! The full Settings window.
//!
//! A fixed category rail on the left, a fixed action bar along the bottom, and
//! the selected category's options in a scrollable centre. Preferences live on
//! [`App`] — the shell owns them and writes them to `settings.json` — so this
//! module only draws controls and turns a change into a [`Command`] or a direct
//! field update. It is presentation, exactly like the menu bar, but it needs
//! the app's own state, which is why it lives beside the shell rather than in
//! the `ui` crate.

use egui::{Align, Context, Layout, RichText};
use pdfreader_core::{Command, ThemeId, ViewMode, ZoomMode};
use pdfreader_ui::Palette;

use crate::App;

/// One page of the Settings window.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// Theme, interface scale and layout.
    Appearance,
    /// How documents open.
    Reading,
    /// The word-lookup helpers and the translate target.
    Language,
    /// Prefetching and the diagnostics overlay.
    Performance,
    /// Version and where settings live.
    About,
}

impl Category {
    /// Every category, in rail order.
    const ALL: [Category; 5] = [
        Category::Appearance,
        Category::Reading,
        Category::Language,
        Category::Performance,
        Category::About,
    ];

    /// Rail label.
    const fn label(self) -> &'static str {
        match self {
            Category::Appearance => "Appearance",
            Category::Reading => "Reading",
            Category::Language => "Language tools",
            Category::Performance => "Performance",
            Category::About => "About",
        }
    }

    /// One-line description shown under the page title.
    const fn blurb(self) -> &'static str {
        match self {
            Category::Appearance => "Theme, interface scale and layout.",
            Category::Reading => "How documents open.",
            Category::Language => "The lookup helpers and the translate target.",
            Category::Performance => "Prefetching and the diagnostics overlay.",
            Category::About => "Version and where your settings are stored.",
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
            "performance" => Category::Performance,
            "about" => Category::About,
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
        ThemeId::Acrobat => "Acrobat",
        ThemeId::Dark => "Dark",
        ThemeId::Light => "Light",
        ThemeId::Midnight => "Midnight",
        ThemeId::Rose => "Rose",
        ThemeId::Forest => "Forest",
        ThemeId::Sunset => "Sunset",
    }
}

/// A dimmed explanatory line under a control.
fn hint(palette: &Palette, text: &str) -> RichText {
    RichText::new(text).size(11.5).color(palette.text_dim)
}

/// Draw a section title with a hairline rule beneath it.
fn section(ui: &mut egui::Ui, palette: &Palette, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).strong().size(13.0).color(palette.text));
    ui.add_space(3.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, palette.border);
    ui.add_space(8.0);
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

        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
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
                            if ui.button("Reset to defaults").clicked() {
                                reset = true;
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Done").clicked() {
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
                                ui.label(
                                    RichText::new(category.label())
                                        .strong()
                                        .size(18.0)
                                        .color(palette.text),
                                );
                                ui.add_space(2.0);
                                ui.label(hint(palette, category.blurb()));
                                ui.add_space(6.0);

                                match category {
                                    Category::Appearance => {
                                        self.page_appearance(ui, palette, &mut commands, &mut changed);
                                    }
                                    Category::Reading => {
                                        self.page_reading(ui, palette, &mut commands, &mut changed);
                                    }
                                    Category::Language => {
                                        self.page_language(ui, palette, &mut commands);
                                    }
                                    Category::Performance => {
                                        self.page_performance(ui, palette, &mut changed);
                                    }
                                    Category::About => about_page(ui, palette),
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
        if ui
            .checkbox(&mut speak, "Speak (text to speech)")
            .changed()
        {
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
                    if ui.selectable_label(self.lens_target == code, label).clicked() {
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
        if ui
            .checkbox(&mut prefetch, "Preload nearby pages")
            .changed()
        {
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
                egui::Frame::new()
                    .fill(palette.panel_bg)
                    .stroke(egui::Stroke::new(1.0, palette.border))
                    .corner_radius(6.0)
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
        egui::Window::new("Keyboard Shortcuts")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([470.0, 540.0])
            .min_size([380.0, 360.0])
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (group, rows) in SHORTCUTS {
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(*group).strong().size(13.0).color(palette.text),
                            );
                            ui.add_space(4.0);
                            egui::Grid::new(*group)
                                .num_columns(2)
                                .spacing([18.0, 7.0])
                                .show(ui, |ui| {
                                    for (keys, action) in *rows {
                                        ui.label(
                                            RichText::new(*keys)
                                                .monospace()
                                                .size(11.5)
                                                .color(palette.accent),
                                        );
                                        ui.label(
                                            RichText::new(*action).size(12.0).color(palette.text),
                                        );
                                        ui.end_row();
                                    }
                                });
                            ui.add_space(10.0);
                        }
                    });
            });
        self.shortcuts_open = open;
    }
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
        .corner_radius(6.0)
        .min_size(egui::vec2(ui.available_width(), 32.0));
    ui.add(button).clicked()
}

/// About: version and where settings live.
fn about_page(ui: &mut egui::Ui, palette: &Palette) {
    section(ui, palette, "PDF Reader");
    ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).size(12.0));

    section(ui, palette, "Settings file");
    match crate::config_file() {
        Some(path) => ui.label(RichText::new(path.display().to_string()).monospace().size(11.5)),
        None => ui.label(hint(palette, "No configuration directory could be found.")),
    };
    ui.label(hint(
        palette,
        "Preferences are written here as JSON and loaded on the next launch.",
    ));

    section(ui, palette, "Dictionaries");
    ui.label(hint(
        palette,
        "The offline word-lookup dictionaries live in the lens folder next to the executable (or assets/lens when run from a checkout).",
    ));
}
