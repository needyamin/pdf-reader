//! Modern desktop chrome for the PDF reader.
//!
//! The widgets in this module are intentionally presentation-only. They emit
//! domain commands and leave document state, rendering, and persistence to the
//! application shell.

use std::sync::Arc;

use egui::{
    Align2, Color32, FontId, Galley, Id, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2,
    ViewportCommand, containers::menu::SubMenuButton,
};
use pdfreader_core::{
    AnnotationId, AnnotationInfo, AnnotationKind, AppState, Command, DocumentId, ExportTask,
    FieldId, FieldValue, FormFieldInfo, FormFieldType, FormInfo, Outline, OutlineNode, SidebarTab,
    Tab, ThemeId, Tool, ViewMode, ZoomMode,
};

use crate::theme::Palette;
use crate::window::dialog_window;

/// Height of the compact application menu.
pub const MENUBAR_HEIGHT: f32 = 28.0;
/// Height of the main command toolbar. Kept tight: 40px fits the icon rows
/// without wasted chrome.
pub const TOOLBAR_HEIGHT: f32 = 40.0;
/// Height of the document tab strip.
pub const TABBAR_HEIGHT: f32 = 34.0;
/// Height of the quiet status bar.
pub const STATUSBAR_HEIGHT: f32 = 28.0;
/// Width of the right-hand tools rail.
pub const TOOLS_RAIL_WIDTH: f32 = 44.0;

// --- Sidebar spacing ------------------------------------------------------
//
// One scale for every sidebar surface, so the panels line up with each other
// and no label ends up flush against an edge. Before this each panel picked its
// own numbers — margins of 4, 6, 8 and 10, and gaps of 2, 3 and 6 — and the
// search panel's text had no horizontal padding at all while its query box had
// ten.

/// Padding left and right of a sidebar panel's content.
pub const SIDEBAR_PAD_X: f32 = 12.0;
/// Padding above and below a sidebar panel's content.
pub const SIDEBAR_PAD_Y: f32 = 8.0;
/// Gap between stacked items in a sidebar panel.
pub const SIDEBAR_GAP: f32 = 4.0;
/// Padding between a clickable row's edge and its text.
pub const ROW_PAD_X: f32 = 10.0;
/// Padding above a row's first line and below its last.
pub const ROW_PAD_Y: f32 = 7.0;
/// Gap between the two lines of a row.
pub const ROW_LINE_GAP: f32 = 4.0;

/// Font size of a sidebar row's title line.
const ROW_TITLE_PT: f32 = 11.5;
/// Font size of a sidebar row's detail line.
const ROW_DETAIL_PT: f32 = 10.5;

/// A sidebar panel's content margin.
pub const fn sidebar_margin() -> egui::Margin {
    egui::Margin::symmetric(SIDEBAR_PAD_X as i8, SIDEBAR_PAD_Y as i8)
}

/// A sidebar list's margin.
///
/// The panel padding less the row's own inset, so a row's highlight sits the
/// same distance from the panel edge as the panel's own text does.
pub const fn sidebar_list_margin() -> egui::Margin {
    egui::Margin::symmetric((SIDEBAR_PAD_X - SIDEBAR_GAP) as i8, SIDEBAR_GAP as i8)
}

/// A sidebar note's margin: full horizontal padding, a gap above and below.
pub const fn sidebar_note_margin() -> egui::Margin {
    egui::Margin::symmetric(SIDEBAR_PAD_X as i8, SIDEBAR_GAP as i8)
}

/// Lay out the two lines of a sidebar row and report the height the row needs.
///
/// The height is *measured* from the laid-out galleys rather than fixed. A
/// hard-coded height with hand-placed baselines left no room below the second
/// line: at 34 px for a 11.5 pt title and a 10.5 pt detail, the detail ran three
/// tenths of a point past the row and its descenders were clipped by the row's
/// own clip rectangle. Measuring means the padding is what is asked for, not
/// whatever is left over.
fn two_line_row(
    ui: &Ui,
    title: &str,
    detail: &str,
    title_color: Color32,
    detail_color: Color32,
) -> (Arc<Galley>, Arc<Galley>, f32) {
    let title = ui.painter().layout_no_wrap(
        title.to_owned(),
        FontId::proportional(ROW_TITLE_PT),
        title_color,
    );
    let detail = ui.painter().layout_no_wrap(
        detail.to_owned(),
        FontId::proportional(ROW_DETAIL_PT),
        detail_color,
    );
    let height = ROW_PAD_Y * 2.0 + title.size().y + ROW_LINE_GAP + detail.size().y;
    (title, detail, height)
}

/// Paint the two lines of a sidebar row inside `rect`, at the row's padding.
///
/// The colours are the ones the galleys were laid out with; `Painter::galley`
/// wants a fallback for a galley that carries none, which these never do.
fn paint_two_line_row(
    ui: &Ui,
    rect: Rect,
    title: Arc<Galley>,
    detail: Arc<Galley>,
    title_color: Color32,
    detail_color: Color32,
) {
    let painter = ui.painter().with_clip_rect(rect);
    let left = rect.min.x + ROW_PAD_X;
    let detail_top = rect.min.y + ROW_PAD_Y + title.size().y + ROW_LINE_GAP;
    painter.galley(egui::pos2(left, rect.min.y + ROW_PAD_Y), title, title_color);
    painter.galley(egui::pos2(left, detail_top), detail, detail_color);
}

/// Font families offered for form text (Settings → Text), as `(family, label)`.
///
/// The empty family means egui's default proportional face. The names match the
/// bundled Noto files, which the app registers as named egui families, so
/// choosing one gives form text a face that covers that script (Bengali, and so
/// on) at the size the user wants.
pub const FORM_FONTS: &[(&str, &str)] = &[
    ("", "Default"),
    ("NotoSans-Regular", "Noto Sans (Latin)"),
    ("NotoSansBengali-Regular", "Bengali"),
    ("NotoSansDevanagari-Regular", "Devanagari"),
    ("NotoSansArabic-Regular", "Arabic"),
    ("NotoSansHebrew-Regular", "Hebrew"),
    ("NotoSansTamil-Regular", "Tamil"),
    ("NotoSansThai-Regular", "Thai"),
];

/// The font used for text typed into form fields.
///
/// Bundled because it travels together: the Forms panel and the quick bar above
/// the page both edit it, and two copies of four loose arguments were easy to
/// get out of step.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TextStyle<'a> {
    /// Named font family, or empty for the default proportional face.
    pub family: &'a str,
    /// Point size, or `0.0` to fit the field's own height.
    pub size: f32,
    /// Whether the text is drawn heavier.
    pub bold: bool,
    /// RGB ink.
    pub color: [u8; 3],
}

const BUTTON_RADIUS: u8 = 7;
const RAIL_RADIUS: u8 = 2;
const ICON_SIZE: f32 = 30.0;

/// Height of the find field. It is sized to *fit* the toolbar row rather than
/// to match the 30 px icon buttons: the toolbar reserves [`TOOLBAR_HEIGHT`]
/// minus [`TOOLBAR_PAD_Y`] on each side, and a taller box pushes the row past
/// that and lets the field spill under the strip below it.
const SEARCH_FIELD_HEIGHT: f32 = 26.0;
/// Vertical padding the toolbar puts above and below its controls. It is what
/// is left of [`TOOLBAR_HEIGHT`] once the tallest control has its 32 px, so
/// the row fits the strip instead of spilling under the section below it.
const TOOLBAR_PAD_Y: i8 = 4;
/// Horizontal padding inside the find field, around the addon and the text.
/// Whole pixels: this is the `Margin` around the text as well as the geometry
/// of the addon, and `Margin` counts in integers.
const SEARCH_FIELD_PAD: i8 = 8;
/// Width reserved for the magnifier glyph in the addon.
const SEARCH_ICON_WIDTH: f32 = 13.0;
/// Gap between the magnifier and the "Find" label.
const SEARCH_ADDON_GAP: f32 = 5.0;
/// Width of the clear button at the right edge of the field.
const SEARCH_CLEAR_WIDTH: f32 = 22.0;

/// A small internally drawn icon set. Keeping the icons in one painter-based
/// system avoids mixing unrelated glyph fonts and keeps stroke weight stable
/// across Windows, macOS, Linux, and high-DPI displays.
#[derive(Clone, Copy)]
#[allow(missing_docs)]
pub enum Icon {
    Open,
    Previous,
    Next,
    ZoomOut,
    ZoomIn,
    RotateLeft,
    RotateRight,
    Sidebar,
    Search,
    Document,
    Pages,
    Outline,
    Close,
    Pan,
    More,
    Undo,
    Redo,
    /// A sticky-note / comment glyph, used to headline the annotate toolbar.
    Annotation,
    /// The annotation-tool glyphs, one per [`pdfreader_core::Tool`]. Each is a
    /// miniature of the mark the tool makes — the same shape the canvas paints
    /// beside the pointer — so the toolbar button and the cursor agree.
    SelectTool,
    HighlightTool,
    UnderlineTool,
    StrikeoutTool,
    SquigglyTool,
    RectangleTool,
    NoteTool,
    EraserTool,
}

/// The Edit menu: the annotation history, enabled only when there is history.
///
/// Kept out of [`menu_bar`] so that function stays readable; every other menu
/// follows the same shape.
fn edit_menu(ui: &mut Ui, state: &AppState) -> Vec<Command> {
    let mut commands = Vec::new();
    let undoable = state.active().is_some_and(|tab| !tab.undo_stack.is_empty());
    let redoable = state.active().is_some_and(|tab| !tab.redo_stack.is_empty());
    if ui
        .add_enabled(undoable, menu_item_button("Undo", "Ctrl+Z"))
        .clicked()
    {
        commands.push(Command::Undo);
        ui.close();
    }
    if ui
        .add_enabled(redoable, menu_item_button("Redo", "Ctrl+Shift+Z"))
        .clicked()
    {
        commands.push(Command::Redo);
        ui.close();
    }

    ui.separator();
    if menu_item(ui, "Find…", "Ctrl+F").clicked() {
        commands.push(Command::FocusSearch);
        ui.close();
    }
    commands
}

/// The File menu: documents in and out, and everything that turns what is open
/// into a new file.
///
/// Kept out of [`menu_bar`] for the same reason as [`edit_menu`]: the export
/// and print entries are longer than the whole rest of the menu bar.
fn file_menu(ui: &mut Ui, state: &AppState) -> Vec<Command> {
    let mut commands = Vec::new();
    let has_doc = state.active().is_some_and(|t| t.document.is_some());
    let has_tab = state.active_tab.is_some();

    if menu_item(ui, "Open…", "Ctrl+O").clicked() {
        commands.push(Command::ShowOpenDialog);
        ui.close();
    }

    ui.separator();
    ui.add_enabled_ui(has_doc, |ui| {
        if menu_item(ui, "Save", "Ctrl+S").clicked() {
            commands.push(Command::SaveDocument);
            ui.close();
        }
        if menu_item(ui, "Save As…", "Ctrl+Shift+S").clicked() {
            commands.push(Command::SaveDocumentAs);
            ui.close();
        }
    });

    ui.separator();
    ui.add_enabled_ui(has_doc, |ui| {
        if menu_item(ui, "Flatten Document", "").clicked() {
            commands.push(Command::FlattenDocument);
            ui.close();
        }
    });

    // Export sits outside the enabled blocks: two of its five tasks work from
    // files the user picks and need no open document, which is exactly when
    // someone wants to merge two PDFs they have not opened. It carries the
    // Ctrl+E hint on the submenu button itself, since the tasks inside have no
    // single shortcut of their own.
    ui.separator();
    SubMenuButton::from_button(menu_item_button("Export", "Ctrl+E")).ui(ui, |ui| {
        commands.extend(export_menu(ui, has_doc));
    });

    ui.separator();
    ui.add_enabled_ui(has_doc, |ui| {
        if menu_item(ui, "Print…", "Ctrl+P").clicked() {
            commands.push(Command::Print);
            ui.close();
        }
        if menu_item(ui, "Print Current Page", "").clicked() {
            commands.push(Command::PrintCurrentPage);
            ui.close();
        }
    });

    ui.separator();
    ui.add_enabled_ui(has_tab, |ui| {
        if menu_item(ui, "Close Tab", "Ctrl+W").clicked() {
            if let Some(id) = state.active_tab {
                commands.push(Command::RequestCloseTab(id));
            }
            ui.close();
        }
    });

    ui.separator();
    if menu_item(ui, "Exit", "Alt+F4").clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
        ui.close();
    }

    commands
}

/// The Export submenu: one entry per task the export window can be opened on.
///
/// All five are always listed, even when the window has no document to read.
/// Hiding the ones that need a document would make the menu change shape as
/// tabs open and close, and a disabled row that stays put is easier to learn
/// than one that comes and goes.
///
/// The entries come from [`ExportTask::ALL`], so a new task in the domain model
/// appears here without a change to the menu.
fn export_menu(ui: &mut Ui, has_doc: bool) -> Vec<Command> {
    let mut commands = Vec::new();

    for task in ExportTask::ALL {
        let enabled = !task.needs_document() || has_doc;
        ui.add_enabled_ui(enabled, |ui| {
            if menu_item(ui, task.label(), "").clicked() {
                commands.push(Command::ShowExportWindow(task));
                ui.close();
            }
        });
    }

    commands
}

/// Top-level application menu.
pub fn menu_bar(ui: &mut Ui, state: &AppState, palette: &Palette) -> Vec<Command> {
    let mut commands = Vec::new();
    let has_doc = state.active().is_some_and(|t| t.document.is_some());
    let zoom_mode = state.active().map(|t| t.view.zoom_mode);
    let view_mode = state.active().map(|t| t.view.mode);
    let (page, page_count) = state
        .active()
        .map_or((0, 0u32), |t| (t.view.current_page, t.page_count()));

    egui::MenuBar::new().ui(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(
                RichText::new("AnsNew PDF Reader")
                    .strong()
                    .color(palette.text)
                    .size(12.5),
            );
            ui.add_space(8.0);
            menu_button(ui, palette, "File", |ui| {
                commands.extend(file_menu(ui, state));
            });
            menu_button(ui, palette, "Edit", |ui| {
                commands.extend(edit_menu(ui, state));
            });
            menu_button(ui, palette, "View", |ui| {
                ui.add_enabled_ui(has_doc, |ui| {
                    if menu_item(ui, "Zoom In", "Ctrl+=").clicked() {
                        commands.push(Command::ZoomIn);
                        ui.close();
                    }
                    if menu_item(ui, "Zoom Out", "Ctrl+-").clicked() {
                        commands.push(Command::ZoomOut);
                        ui.close();
                    }
                    if menu_item(ui, "Actual Size", "Ctrl+0").clicked() {
                        commands.push(Command::ZoomReset);
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .radio(matches!(zoom_mode, Some(ZoomMode::FitWidth)), "Fit Width")
                        .clicked()
                    {
                        commands.push(Command::SetZoom(ZoomMode::FitWidth));
                        ui.close();
                    }
                    if ui
                        .radio(matches!(zoom_mode, Some(ZoomMode::FitPage)), "Fit Page")
                        .clicked()
                    {
                        commands.push(Command::SetZoom(ZoomMode::FitPage));
                        ui.close();
                    }
                    ui.separator();
                    ui.menu_button("Page Display", |ui| {
                        if ui
                            .radio(matches!(view_mode, Some(ViewMode::Single)), "Single Page")
                            .clicked()
                        {
                            commands.push(Command::SetViewMode(ViewMode::Single));
                            ui.close();
                        }
                        if ui
                            .radio(
                                matches!(view_mode, Some(ViewMode::Continuous)),
                                "Continuous",
                            )
                            .clicked()
                        {
                            commands.push(Command::SetViewMode(ViewMode::Continuous));
                            ui.close();
                        }
                    });
                    ui.menu_button("Rotate View", |ui| {
                        if menu_item(ui, "Clockwise", "").clicked() {
                            commands.push(Command::RotateCw);
                            ui.close();
                        }
                        if menu_item(ui, "Counterclockwise", "").clicked() {
                            commands.push(Command::RotateCcw);
                            ui.close();
                        }
                    });
                });
                ui.separator();
                let label = if state.sidebar_visible {
                    "Hide Sidebar"
                } else {
                    "Show Sidebar"
                };
                if menu_item(ui, label, "Ctrl+B").clicked() {
                    commands.push(Command::ToggleSidebar);
                    ui.close();
                }
                if menu_item(ui, fullscreen_label(state), "F11").clicked() {
                    commands.push(Command::ToggleFullscreen);
                    ui.close();
                }
                ui.separator();
                // Theme submenu: the toolbar's theme picker is hidden in
                // compact windows, so the menu is the always-available entry.
                ui.menu_button("Theme", |ui| {
                    for id in ThemeId::ALL {
                        if ui.radio(state.theme == id, theme_label(id)).clicked() {
                            commands.push(Command::SetTheme(id));
                            ui.close();
                        }
                    }
                });
            });
            menu_button(ui, palette, "Go", |ui| {
                ui.add_enabled_ui(has_doc, |ui| {
                    if menu_item(ui, "First Page", "Home").clicked() {
                        commands.push(Command::GoToPage(0));
                        ui.close();
                    }
                    if ui
                        .add_enabled(page > 0, menu_item_button("Previous Page", "Page Up"))
                        .clicked()
                    {
                        commands.push(Command::PrevPage);
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            page_count > 0 && page + 1 < page_count,
                            menu_item_button("Next Page", "Page Down"),
                        )
                        .clicked()
                    {
                        commands.push(Command::NextPage);
                        ui.close();
                    }
                    if menu_item(ui, "Last Page", "End").clicked() {
                        commands.push(Command::GoToPage(u32::MAX));
                        ui.close();
                    }
                });
            });
            // Settings is a direct action, not a submenu: clicking it opens the
            // full Settings window.
            if menu_action(ui, palette, "Settings").clicked() {
                commands.push(Command::ShowSettings);
            }
            menu_button(ui, palette, "Help", |ui| {
                if menu_item(ui, "Keyboard Shortcuts", "").clicked() {
                    commands.push(Command::ShowShortcuts);
                    ui.close();
                }
                ui.separator();
                if menu_item(ui, "About AnsNew PDF Reader", "").clicked() {
                    commands.push(Command::ShowAbout);
                    ui.close();
                }
            });
        });
    });

    commands
}

/// Main toolbar with logically grouped reading controls.
pub fn toolbar(
    ui: &mut Ui,
    state: &AppState,
    palette: &Palette,
    search_query: &mut String,
    pan_tool: &mut bool,
    focus_search: bool,
) -> Vec<Command> {
    let mut commands = Vec::new();
    let tab = state.active();
    let has_doc = tab.is_some_and(|t| t.document.is_some());
    let page_count = tab.map_or(0, Tab::page_count);
    let compact = ui.available_width() < 1040.0;

    egui::Frame::new()
        .fill(palette.panel_bg)
        .inner_margin(egui::Margin::symmetric(10, TOOLBAR_PAD_Y))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if primary_button(ui, palette, Icon::Open, "Open", "Open a PDF (Ctrl+O)") {
                    commands.push(Command::ShowOpenDialog);
                }
                separator(ui, palette);

                let current = tab.map_or(0, |t| t.view.current_page);
                let at_start = current == 0;
                let at_end = page_count == 0 || current + 1 >= page_count;
                toolbar_group(ui, palette, |ui| {
                    if icon_button(
                        ui,
                        palette,
                        Icon::Previous,
                        "Previous page",
                        "Previous page (Page Up)",
                        false,
                        has_doc && !at_start,
                    ) {
                        commands.push(Command::PrevPage);
                    }
                    let mut page = current + 1;
                    let max = page_count.max(1);
                    let page_response = ui.add_sized(
                        Vec2::new(42.0, 30.0),
                        egui::DragValue::new(&mut page)
                            .range(1..=max)
                            .speed(0.25)
                            .fixed_decimals(0),
                    );
                    if page_response.changed() {
                        commands.push(Command::GoToPage(page.saturating_sub(1)));
                    }
                    page_response.on_hover_text("Go to page");
                    ui.label(
                        RichText::new(format!("/ {max}"))
                            .color(palette.text_dim)
                            .size(12.0),
                    );
                    if icon_button(
                        ui,
                        palette,
                        Icon::Next,
                        "Next page",
                        "Next page (Page Down)",
                        false,
                        has_doc && !at_end,
                    ) {
                        commands.push(Command::NextPage);
                    }
                });
                separator(ui, palette);

                ui.add_enabled_ui(has_doc, |ui| {
                    toolbar_group(ui, palette, |ui| {
                        if icon_button(
                            ui,
                            palette,
                            Icon::ZoomOut,
                            "Zoom out",
                            "Zoom out (Ctrl+-)",
                            false,
                            true,
                        ) {
                            commands.push(Command::ZoomOut);
                        }
                        zoom_menu(ui, palette, tab, &mut commands);
                        if icon_button(
                            ui,
                            palette,
                            Icon::ZoomIn,
                            "Zoom in",
                            "Zoom in (Ctrl+=)",
                            false,
                            true,
                        ) {
                            commands.push(Command::ZoomIn);
                        }
                        // Hand tool: drag the page around. Only useful once the
                        // page is bigger than the viewport, which is exactly
                        // when a reader needs it.
                        if icon_button(
                            ui,
                            palette,
                            Icon::Pan,
                            "Pan",
                            "Hand tool: drag to move the page (or hold the middle mouse button)",
                            *pan_tool,
                            true,
                        ) {
                            *pan_tool = !*pan_tool;
                        }
                    });
                    if !compact {
                        separator(ui, palette);
                        toolbar_group(ui, palette, |ui| {
                            if icon_button(
                                ui,
                                palette,
                                Icon::RotateLeft,
                                "Rotate left",
                                "Rotate counter-clockwise",
                                false,
                                true,
                            ) {
                                commands.push(Command::RotateCcw);
                            }
                            if icon_button(
                                ui,
                                palette,
                                Icon::RotateRight,
                                "Rotate right",
                                "Rotate clockwise",
                                false,
                                true,
                            ) {
                                commands.push(Command::RotateCw);
                            }
                        });
                        separator(ui, palette);
                        let continuous =
                            matches!(tab.map(|t| t.view.mode), Some(ViewMode::Continuous));
                        if text_button(ui, palette, "Continuous", "Continuous reading", continuous)
                        {
                            commands.push(Command::SetViewMode(ViewMode::Continuous));
                        }
                        if text_button(ui, palette, "Single", "Single page", !continuous) {
                            commands.push(Command::SetViewMode(ViewMode::Single));
                        }
                    }
                });

                separator(ui, palette);
                ui.add_enabled_ui(has_doc, |ui| {
                    // Bootstrap-style input-group: the label lives inside the
                    // box as its leading addon, so the field carries its own
                    // caption and the toolbar keeps the space a separate
                    // "Find" label used to take.
                    let width = if compact { 186.0 } else { 240.0 };
                    let outcome = search_field(
                        ui,
                        palette,
                        search_query,
                        width,
                        "Search document",
                        focus_search,
                    );
                    if outcome.submitted {
                        commands.push(Command::Search(search_query.trim().to_owned()));
                    }
                    if outcome.cleared {
                        commands.push(Command::Search(String::new()));
                    }
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(4.0);
                    if icon_button(
                        ui,
                        palette,
                        Icon::Sidebar,
                        "Sidebar",
                        "Toggle sidebar (Ctrl+B)",
                        state.sidebar_visible,
                        true,
                    ) {
                        commands.push(Command::ToggleSidebar);
                    }
                    if !compact {
                        let current_theme = state.theme;
                        egui::ComboBox::from_id_salt("theme-picker")
                            .selected_text(
                                RichText::new(theme_label(current_theme))
                                    .color(palette.text)
                                    .size(11.5),
                            )
                            .width(92.0)
                            .show_ui(ui, |ui| {
                                for id in ThemeId::ALL {
                                    if ui
                                        .selectable_label(id == current_theme, theme_label(id))
                                        .clicked()
                                    {
                                        commands.push(Command::SetTheme(id));
                                    }
                                }
                            });
                    }
                });
            });
        });

    commands
}

/// What the reader did inside a [`search_field`] this frame.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct SearchFieldOutput {
    /// Enter was pressed while the field had focus: run the query.
    pub submitted: bool,
    /// The clear button was pressed. The query has already been emptied, so the
    /// caller only has to drop the results the old query produced.
    pub cleared: bool,
}

/// The find box: a Bootstrap-style `input-group`, so one bordered control holds
/// the leading addon (a magnifier and the word "Find"), the query itself, and a
/// clear button that appears only once there is something to clear.
///
/// Both the toolbar and the Search sidebar call this. That is deliberate: two
/// find boxes that are the same call cannot drift apart the way two hand-built
/// copies do.
pub fn search_field(
    ui: &mut Ui,
    palette: &Palette,
    query: &mut String,
    width: f32,
    hint: &str,
    focus: bool,
) -> SearchFieldOutput {
    let mut output = SearchFieldOutput::default();
    let addon_width = search_addon_width(ui, palette);
    let clear_width = if query.is_empty() {
        0.0
    } else {
        SEARCH_CLEAR_WIDTH
    };
    let text_width = (width - addon_width - clear_width).max(64.0);

    // Focus is only known once the field has been laid out, but the border
    // that shows it belongs to the group, so it is remembered and stroked
    // after the frame.
    let mut focused = false;
    let frame = egui::Frame::new()
        .fill(palette.surface)
        .corner_radius(BUTTON_RADIUS)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.horizontal(|ui| {
                let (addon, _) = ui.allocate_exact_size(
                    Vec2::new(addon_width, SEARCH_FIELD_HEIGHT),
                    Sense::hover(),
                );
                // Bootstrap rounds the addon on the outer edge only; its right
                // side meets the divider at a square corner.
                ui.painter().rect_filled(
                    addon,
                    egui::CornerRadius {
                        nw: BUTTON_RADIUS,
                        sw: BUTTON_RADIUS,
                        ne: 0,
                        se: 0,
                    },
                    palette.surface_hover,
                );
                let pad = f32::from(SEARCH_FIELD_PAD);
                draw_icon(
                    ui.painter(),
                    addon.left_center() + Vec2::new(pad + SEARCH_ICON_WIDTH / 2.0, 0.0),
                    Icon::Search,
                    palette.text_dim,
                );
                ui.painter().text(
                    Pos2::new(
                        addon.left() + pad + SEARCH_ICON_WIDTH + SEARCH_ADDON_GAP,
                        addon.center().y,
                    ),
                    Align2::LEFT_CENTER,
                    "Find",
                    FontId::new(11.0, egui::FontFamily::Proportional),
                    palette.text_dim,
                );
                ui.painter().line_segment(
                    [addon.right_top(), addon.right_bottom()],
                    Stroke::new(1.0, palette.border),
                );

                let response = ui.add_sized(
                    Vec2::new(text_width, SEARCH_FIELD_HEIGHT),
                    egui::TextEdit::singleline(query)
                        // The group is the frame. A second one would draw a box
                        // inside the box.
                        .frame(
                            egui::Frame::new()
                                .inner_margin(egui::Margin::symmetric(SEARCH_FIELD_PAD, 0)),
                        )
                        .vertical_align(egui::Align::Center)
                        .font(FontId::new(12.0, egui::FontFamily::Proportional))
                        .hint_text(hint),
                );
                focused = response.has_focus();
                // Edit → Find… (Ctrl+F) asks the field to take focus this frame.
                if focus {
                    response.request_focus();
                }
                // A single-line field surrenders focus when Enter is pressed,
                // so by the time the key is seen it no longer *has* focus —
                // losing it this frame is what marks the submit. This is the
                // pairing egui documents for `TextEdit`.
                if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                    output.submitted = true;
                }

                if !query.is_empty() {
                    let clear = ui.add_sized(
                        Vec2::new(SEARCH_CLEAR_WIDTH, SEARCH_FIELD_HEIGHT),
                        egui::Button::new(RichText::new("×").size(14.0).color(palette.text_dim))
                            .fill(Color32::TRANSPARENT)
                            .corner_radius(0.0),
                    );
                    if clear.on_hover_text("Clear search").clicked() {
                        query.clear();
                        output.cleared = true;
                    }
                }
            });
        });

    ui.painter().rect_stroke(
        frame.response.rect,
        BUTTON_RADIUS,
        Stroke::new(
            1.0,
            if focused {
                palette.accent
            } else {
                palette.border
            },
        ),
        StrokeKind::Inside,
    );

    output
}

/// Width of the "Find" addon: pad, magnifier, gap, label, pad. The label is
/// measured against the real font, so it cannot clip at another UI scale.
fn search_addon_width(ui: &Ui, palette: &Palette) -> f32 {
    let label = ui
        .painter()
        .layout_no_wrap(
            "Find".to_owned(),
            FontId::new(11.0, egui::FontFamily::Proportional),
            palette.text_dim,
        )
        .size()
        .x;
    f32::from(SEARCH_FIELD_PAD) * 2.0 + SEARCH_ICON_WIDTH + SEARCH_ADDON_GAP + label
}

/// Zoom control with reading-friendly presets.
fn zoom_menu(ui: &mut Ui, palette: &Palette, tab: Option<&Tab>, commands: &mut Vec<Command>) {
    let current = tab.map_or(ZoomMode::FitPage, |t| t.view.zoom_mode);
    let label = match current {
        ZoomMode::FitPage => "Fit page".to_string(),
        ZoomMode::FitWidth => "Fit width".to_string(),
        ZoomMode::Fixed(factor) => format!("{factor:.0}%"),
    };
    egui::ComboBox::from_id_salt("zoom-menu")
        .selected_text(RichText::new(label).color(palette.text).size(11.5))
        .width(84.0)
        .show_ui(ui, |ui| {
            for (mode, label) in [
                (ZoomMode::FitWidth, "Fit width"),
                (ZoomMode::FitPage, "Fit page"),
            ] {
                if ui.selectable_label(current == mode, label).clicked() {
                    commands.push(Command::SetZoom(mode));
                }
            }
            ui.separator();
            for percent in [50, 75, 100, 125, 150, 200, 400] {
                let factor = percent as f32 / 100.0;
                if ui
                    .selectable_label(
                        matches!(current, ZoomMode::Fixed(v) if (v - factor).abs() < 1e-3),
                        format!("{percent}%"),
                    )
                    .clicked()
                {
                    commands.push(Command::SetZoom(ZoomMode::Fixed(factor)));
                }
            }
        });
}

/// Compact status information that stays visually quiet while reading.
pub fn status_bar(ui: &mut Ui, state: &AppState, palette: &Palette) {
    egui::Frame::new()
        .fill(palette.panel_bg)
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (label, state_color) = match state.active() {
                    None => ("Ready to open a document".to_string(), palette.text_dim),
                    Some(tab) if tab.loading => ("Opening document…".to_string(), palette.accent),
                    Some(tab) if tab.error.is_some() => {
                        ("Document could not be opened".to_string(), palette.danger)
                    }
                    Some(tab) => (
                        tab.document
                            .as_ref()
                            .map_or_else(|| "Document".to_string(), |d| d.title.clone()),
                        palette.accent,
                    ),
                };
                ui.painter()
                    .circle_filled(ui.cursor().min + Vec2::new(4.0, 8.0), 3.0, state_color);
                ui.add_space(12.0);
                ui.label(RichText::new(label).color(palette.text_dim).size(11.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let Some(tab) = state.active() else { return };
                    if tab.loading || tab.error.is_some() {
                        return;
                    }
                    let rotation = tab.view.rotation.degrees();
                    let detail = if rotation == 0 {
                        format!(
                            "Page {} of {}  ·  {}  ·  {:.0}%",
                            tab.view.current_page + 1,
                            tab.page_count().max(1),
                            view_mode_label(tab.view.mode),
                            tab.view.zoom * 100.0
                        )
                    } else {
                        format!(
                            "Page {} of {}  ·  {}  ·  {:.0}%  ·  {rotation}°",
                            tab.view.current_page + 1,
                            tab.page_count().max(1),
                            view_mode_label(tab.view.mode),
                            tab.view.zoom * 100.0
                        )
                    };
                    ui.label(RichText::new(detail).color(palette.text_dim).size(11.0));
                });
            });
        });
}

/// Document tabs with compact identity, active indicator, and middle-click close.
pub fn tab_bar(ui: &mut Ui, state: &AppState, palette: &Palette) -> Vec<Command> {
    let mut commands = Vec::new();
    egui::Frame::new()
        .fill(palette.panel_bg)
        .inner_margin(egui::Margin::symmetric(10, 3))
        .show(ui, |ui| {
            egui::ScrollArea::horizontal()
                .id_salt("doc-tabs")
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        for tab in &state.tabs {
                            let active = state.active_tab == Some(tab.id);
                            let title = if let Some(doc) = &tab.document {
                                doc.title.clone()
                            } else if tab.loading {
                                "Opening…".to_string()
                            } else {
                                tab.path.file_stem().map_or_else(
                                    || "Untitled".to_string(),
                                    |s| s.to_string_lossy().to_string(),
                                )
                            };
                            let display = truncate(&title, 24);
                            let fill = if active {
                                palette.accent_soft
                            } else {
                                Color32::TRANSPARENT
                            };
                            egui::Frame::new()
                                .fill(fill)
                                .corner_radius(7.0)
                                .inner_margin(egui::Margin::symmetric(7, 2))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        draw_icon(
                                            ui.painter(),
                                            ui.cursor().min + Vec2::new(8.0, 14.0),
                                            Icon::Document,
                                            if active {
                                                palette.accent
                                            } else {
                                                palette.text_dim
                                            },
                                        );
                                        ui.add_space(16.0);
                                        let tab_response = ui
                                            .add_sized(
                                                Vec2::new(118.0, 26.0),
                                                egui::Button::new(
                                                    RichText::new(display.as_str())
                                                        .color(palette.text)
                                                        .size(11.5),
                                                )
                                                .fill(Color32::TRANSPARENT)
                                                .stroke(Stroke::NONE),
                                            )
                                            .on_hover_text(title.as_str());
                                        if tab_response.clicked() {
                                            commands.push(Command::ActivateTab(tab.id));
                                        }
                                        if icon_button(
                                            ui,
                                            palette,
                                            Icon::Close,
                                            "Close",
                                            "Close tab (Ctrl+W)",
                                            false,
                                            true,
                                        ) {
                                            commands.push(Command::RequestCloseTab(tab.id));
                                        }
                                        if tab_response.middle_clicked() {
                                            commands.push(Command::RequestCloseTab(tab.id));
                                        }
                                    });
                                });
                        }
                    });
                });
        });
    commands
}

/// Sidebar navigation. Labels remain visible because the panel is wide enough
/// to be a useful navigation rail rather than an ambiguous icon strip.
pub fn sidebar_tabs(ui: &mut Ui, state: &AppState, palette: &Palette) -> Vec<Command> {
    let mut commands = Vec::new();
    egui::Frame::new()
        .fill(palette.panel_bg)
        .inner_margin(sidebar_margin())
        .show(ui, |ui| {
            // One row, as before: the buttons size to their content and the
            // strip scrolls rather than wraps when the sidebar is dragged very
            // narrow.
            ui.horizontal(|ui| {
                for (id, label, icon) in available_sidebar_tabs(state) {
                    if sidebar_tab_button(ui, palette, label, icon, state.sidebar_tab == id) {
                        commands.push(Command::SetSidebarTab(id));
                    }
                }
            });
        });
    commands
}

/// Recursively draw the outline and return navigation commands.
pub fn outline_tree(ui: &mut Ui, palette: &Palette, outline: &Outline) -> Vec<Command> {
    let mut commands = Vec::new();
    if outline.is_empty() {
        empty_sidebar(
            ui,
            palette,
            Icon::Outline,
            "No table of contents",
            "This document does not contain an outline.",
        );
        return commands;
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(sidebar_margin())
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = SIDEBAR_GAP;
                    let mut next_id = 0u64;
                    for node in &outline.root {
                        show_outline_node(ui, palette, node, &mut next_id, &mut commands, 0);
                    }
                });
        });
    commands
}

/// The always-available sidebar sections.
pub const SIDEBAR_TABS: [(SidebarTab, &str, Icon); 3] = [
    (SidebarTab::Thumbnails, "Pages", Icon::Pages),
    (SidebarTab::Outline, "Outline", Icon::Outline),
    (SidebarTab::Search, "Search", Icon::Search),
];

/// The sidebar sections to show for the current state.
///
/// The Forms tab is always present: hiding it for documents without fields
/// made the feature impossible to discover, and an empty panel that says so is
/// more honest than a tab that appears and vanishes between documents.
pub fn available_sidebar_tabs(_state: &AppState) -> Vec<(SidebarTab, &'static str, Icon)> {
    let mut tabs = SIDEBAR_TABS.to_vec();
    tabs.push((SidebarTab::Comments, "Comments", Icon::Document));
    tabs.push((SidebarTab::Forms, "Forms", Icon::Document));
    tabs
}

/// Height of the annotation tool strip above the canvas.
pub const ANNOTATION_BAR_HEIGHT: f32 = 30.0;

/// The annotation tool strip: one button per tool, the active one lit.
///
/// Rendered as a thin row above the canvas. Text labels rather than icons —
/// eight hand-drawn glyphs for tools this specialised would cost more than the
/// words, and the strip only exists while a document is open.
pub fn annotation_bar(
    ui: &mut Ui,
    palette: &Palette,
    state: &AppState,
    style: TextStyle<'_>,
) -> Vec<Command> {
    let mut commands = Vec::new();
    egui::Frame::new()
        .fill(palette.panel_bg)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(ANNOTATION_BAR_HEIGHT - 4.0);
                // Boxed "Annotate" header chip: the label inside a
                // palette-coloured frame, matching the shared chrome style.
                let chip = egui::Frame::new()
                    .fill(palette.surface)
                    .stroke(egui::Stroke::new(1.0, palette.border))
                    .corner_radius(palette.radius.max(4.0))
                    .inner_margin(egui::Margin::symmetric(7, 3));
                chip.show(ui, |ui| {
                    ui.label(
                        RichText::new("Annotate")
                            .color(palette.text)
                            .size(11.0)
                            .strong(),
                    );
                });
                // The tools as compact icon buttons, split into groups by
                // function (select · markup · shape · note · eraser) so the row
                // scans the way a professional editor's does. Each carries a
                // tooltip with its name and single-key shortcut.
                let mut previous: Option<Tool> = None;
                for tool in Tool::ALL {
                    if previous.is_some_and(|prev| tool_group(prev) != tool_group(tool)) {
                        ui.separator();
                    }
                    if tool_button(ui, palette, tool, state.tool == tool).clicked() {
                        commands.push(Command::SetTool(tool));
                    }
                    previous = Some(tool);
                }
                // Quick controls for the text being typed, sitting right after
                // the text tools. Shown only while a field is open, so the row
                // does not carry controls that do nothing the rest of the time.
                if editing_text_field(state) {
                    ui.separator();
                    quick_text_style(ui, palette, style, &mut commands);
                }

                ui.separator();
                let undoable = state.active().is_some_and(|tab| !tab.undo_stack.is_empty());
                let redoable = state.active().is_some_and(|tab| !tab.redo_stack.is_empty());
                for (enabled, icon, command, tooltip) in [
                    (undoable, Icon::Undo, Command::Undo, "Undo (Ctrl+Z)"),
                    (redoable, Icon::Redo, Command::Redo, "Redo (Ctrl+Shift+Z)"),
                ] {
                    let response = ui
                        .add_enabled(
                            enabled,
                            egui::Button::new("")
                                .min_size(Vec2::new(28.0, 22.0))
                                .fill(if enabled {
                                    palette.accent_soft
                                } else {
                                    Color32::TRANSPARENT
                                })
                                .corner_radius(4.0),
                        )
                        .on_hover_text(tooltip);
                    if response.clicked() && enabled {
                        commands.push(command);
                    }
                    draw_icon(
                        ui.painter(),
                        response.rect.center(),
                        icon,
                        if enabled {
                            palette.text
                        } else {
                            palette.text_dim
                        },
                    );
                }
                // "Properties" toggles the floating colour/opacity/geometry
                // window for the selected annotation. Off by default, so the
                // panel never pops up on its own — selecting an annotation only
                // shows its move/resize handles. This is the on-demand open.
                {
                    let selected = state.active().and_then(|tab| tab.view.selected_annotation);
                    let open = state.active().is_some_and(|tab| tab.view.properties_panel_open);
                    let label = RichText::new("Properties")
                        .size(11.5)
                        .color(if open { palette.accent } else { palette.text });
                    let button = egui::Button::new(label)
                        .fill(if open { palette.accent_soft } else { Color32::TRANSPARENT })
                        .corner_radius(4.0);
                    let response = ui.add_enabled(selected.is_some(), button);
                    if response.clicked() && selected.is_some() {
                        commands.push(Command::SetPropertiesPanel { open: !open });
                    }
                }
            });
        });
    commands
}

/// The icon for one annotation tool.
fn tool_icon(tool: Tool) -> Icon {
    match tool {
        Tool::Select => Icon::SelectTool,
        Tool::Highlight => Icon::HighlightTool,
        Tool::Underline => Icon::UnderlineTool,
        Tool::StrikeOut => Icon::StrikeoutTool,
        Tool::Squiggly => Icon::SquigglyTool,
        Tool::Square => Icon::RectangleTool,
        Tool::StickyNote => Icon::NoteTool,
        Tool::Eraser => Icon::EraserTool,
    }
}

/// Which visual group a tool belongs to in the bar; a change of group draws a
/// divider, so the row reads as select · markup · shape · note · eraser.
fn tool_group(tool: Tool) -> u8 {
    match tool {
        Tool::Select => 0,
        Tool::Highlight | Tool::Underline | Tool::StrikeOut | Tool::Squiggly => 1,
        Tool::Square => 2,
        Tool::StickyNote => 3,
        Tool::Eraser => 4,
    }
}

/// A compact square icon button for one annotation tool.
///
/// The icon is the mark the tool makes (matching the cursor badge), filled in
/// `accent` while armed and `text` otherwise, with a hover fill and a tooltip
/// carrying the tool name and its shortcut.
fn tool_button(ui: &mut Ui, palette: &Palette, tool: Tool, active: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(28.0, 22.0), Sense::click());
    let fill = if active {
        palette.accent_soft
    } else if response.hovered() {
        palette.surface_hover
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 4.0, fill);
    }
    let color = if active { palette.accent } else { palette.text };
    draw_icon(ui.painter(), rect.center(), tool_icon(tool), color);
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!("{} ({})", tool.label(), tool.shortcut()))
}

/// Whether a text field is open for typing.
///
/// The quick text controls belong on screen exactly when there is text to
/// affect: a field the user can type into is selected.
fn editing_text_field(state: &AppState) -> bool {
    let Some(tab) = state.active() else {
        return false;
    };
    let Some(id) = tab.view.selected_field else {
        return false;
    };
    tab.form
        .as_ref()
        .and_then(|form| form.field(id))
        .is_some_and(|field| field.kind == FormFieldType::Text && field.is_editable())
}

/// Face, size, weight and ink for the text being typed.
///
/// The same settings the Forms panel edits, placed next to the page so a face
/// that cannot show the script being typed, or a size too small to read, is one
/// click away rather than a trip to the sidebar.
fn quick_text_style(
    ui: &mut Ui,
    palette: &Palette,
    style: TextStyle<'_>,
    commands: &mut Vec<Command>,
) {
    let chip = egui::Frame::new()
        .fill(palette.surface)
        .stroke(egui::Stroke::new(1.0, palette.border))
        .corner_radius(palette.radius.max(4.0))
        .inner_margin(egui::Margin::symmetric(7, 3));
    chip.show(ui, |ui| {
        ui.label(
            RichText::new("Text")
                .color(palette.text)
                .size(11.0)
                .strong(),
        );
    });

    // The face picker is the widest control here, so it is the one that gives
    // way first when the window is too narrow for the whole group.
    if ui.available_width() > 300.0 {
        let label = FORM_FONTS
            .iter()
            .find(|(name, _)| *name == style.family)
            .map_or("Default", |(_, label)| *label);
        egui::ComboBox::from_id_salt("quick-form-font")
            .selected_text(RichText::new(label).size(11.5).color(palette.text))
            .width(116.0)
            .show_ui(ui, |ui| {
                for &(name, text) in FORM_FONTS {
                    if ui.selectable_label(style.family == name, text).clicked() {
                        commands.push(Command::SetFormFont(name.to_owned()));
                    }
                }
            });
    }

    let mut size = style.size;
    let response = ui
        .add(
            egui::DragValue::new(&mut size)
                .range(0.0..=48.0)
                .speed(0.25)
                .fixed_decimals(1)
                .custom_formatter(|value, _| {
                    if value <= 0.0 {
                        "Auto".to_owned()
                    } else {
                        format!("{value:.1}")
                    }
                }),
        )
        .on_hover_text("Size for typed text — Auto fits each field's own height");
    if response.changed() {
        commands.push(Command::SetFormFontSize(size));
    }

    if ui
        .selectable_label(style.bold, RichText::new("B").strong())
        .on_hover_text("Bold")
        .clicked()
    {
        commands.push(Command::SetFormFontBold(!style.bold));
    }

    let mut rgb = style.color;
    if ui
        .color_edit_button_srgb(&mut rgb)
        .on_hover_text("Ink for typed text")
        .changed()
    {
        commands.push(Command::SetFormFontColor(rgb[0], rgb[1], rgb[2]));
    }
}

/// The Comments panel: every annotation, clickable to jump, editable, deletable.
///
/// Selecting a FreeText or sticky note also opens its text editor, because
/// those are the two kinds whose whole purpose is their text.
pub fn comments_panel(
    ui: &mut Ui,
    palette: &Palette,
    doc: DocumentId,
    annotations: &[AnnotationInfo],
    selected: Option<AnnotationId>,
) -> Vec<Command> {
    let mut commands = Vec::new();

    // The panel is for comments the user can read and edit. Hyperlinks are
    // annotations in the file but not comments — and this reader does not open
    // them — so one row per link just buries the real comments. Drop them here;
    // the engine still returns them for anything that needs the full list.
    let comments: Vec<&AnnotationInfo> = annotations
        .iter()
        .filter(|a| a.kind != AnnotationKind::Link)
        .collect();

    if comments.is_empty() {
        empty_sidebar(
            ui,
            palette,
            Icon::Document,
            "No annotations",
            "Pick a tool above the page, then drag to draw or click to place a note.",
        );
        return commands;
    }

    egui::Frame::new()
        .inner_margin(sidebar_margin())
        .show(ui, |ui| {
            let label = if comments.len() == 1 {
                "1 annotation".to_string()
            } else {
                format!("{} annotations", comments.len())
            };
            ui.label(RichText::new(label).color(palette.text_dim).size(11.0));
        });
    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(sidebar_list_margin())
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = SIDEBAR_GAP;
                    for annotation in comments.iter().copied() {
                        let is_selected = selected == Some(annotation.id);
                        let response = show_comment_row(ui, palette, annotation, is_selected);
                        if response.clicked() && ui.input(|i| i.modifiers.shift) {
                            // Shift-click deletes: one interaction for the
                            // most common follow-up, without a context menu.
                            // Selection is skipped — deleting then selecting
                            // the same id just clears the selection anyway.
                            commands.push(Command::DeleteAnnotation(annotation.id));
                        } else if response.clicked() {
                            commands.push(Command::SelectAnnotation(Some(annotation.id)));
                        }
                        if is_selected {
                            draw_comment_editor(ui, palette, doc, annotation, &mut commands);
                        }
                    }
                });
        });

    commands
}

/// Draw one annotation row: kind and page, with a contents preview.
fn show_comment_row(
    ui: &mut Ui,
    palette: &Palette,
    annotation: &AnnotationInfo,
    is_selected: bool,
) -> egui::Response {
    let title = format!("{} · p.{}", annotation.kind.label(), annotation.id.page + 1);
    let preview = if annotation.display_contents().is_empty() {
        "—".to_string()
    } else {
        truncate(annotation.display_contents(), 30)
    };
    let (title, detail, height) =
        two_line_row(ui, &title, &preview, palette.text, palette.text_dim);

    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());

    if !ui.is_rect_visible(rect) {
        return response;
    }

    let fill = if is_selected {
        palette.accent_soft
    } else if response.hovered() {
        palette.surface_hover
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 4.0, fill);
    paint_two_line_row(ui, rect, title, detail, palette.text, palette.text_dim);
    response
}

/// The contents editor for the selected annotation, plus its delete button.
///
/// Only FreeText and sticky notes get the editor — the markup kinds have no
/// text to edit, and their geometry is the drag that created them.
fn draw_comment_editor(
    ui: &mut Ui,
    palette: &Palette,
    doc: DocumentId,
    annotation: &AnnotationInfo,
    commands: &mut Vec<Command>,
) {
    egui::Frame::new()
        .inner_margin(sidebar_list_margin())
        .show(ui, |ui| {
            let editable = matches!(
                annotation.kind,
                AnnotationKind::FreeText | AnnotationKind::StickyNote
            );
            if editable {
                let key = Id::new((
                    "comment-draft",
                    doc.raw(),
                    annotation.id.page,
                    annotation.id.annot_index,
                ));
                let current = annotation.display_contents().to_string();
                let mut draft = ui.memory_mut(|mem| {
                    mem.data
                        .get_temp_mut_or_insert_with::<String>(key, || current.clone())
                        .clone()
                });

                let response = ui.add(
                    egui::TextEdit::singleline(&mut draft).desired_width(ui.available_width()),
                );
                let changed = draft != current;
                let submit = response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if changed && (response.lost_focus() || submit) {
                    commands.push(Command::SetAnnotationContents {
                        id: annotation.id,
                        contents: draft.clone(),
                    });
                }
                if changed {
                    ui.memory_mut(|mem| mem.data.insert_temp(key, draft));
                }
                ui.add_space(4.0);
            }

            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new(
                        RichText::new("Delete").size(11.0).color(palette.danger),
                    ))
                    .clicked()
                {
                    commands.push(Command::DeleteAnnotation(annotation.id));
                }
            });
        });
}

/// egui memory key for one field's in-progress text draft.
fn draft_key(doc: DocumentId, id: FieldId) -> Id {
    Id::new(("form-draft", doc.raw(), id.page, id.annot_index))
}

/// Draw the editor for the selected field, if its type has one.
fn draw_field_editor(
    ui: &mut Ui,
    palette: &Palette,
    doc: DocumentId,
    field: &FormFieldInfo,
    commands: &mut Vec<Command>,
) {
    if !field.is_editable() {
        return;
    }

    egui::Frame::new()
        .inner_margin(sidebar_list_margin())
        .show(ui, |ui| {
            match field.kind {
                FormFieldType::Text => {
                    draw_text_editor(ui, doc, field, commands);
                }
                // A checkbox has no separate editor surface: a real checkbox
                // right under the name is the whole interaction.
                FormFieldType::CheckBox => {
                    let mut checked = field.value == FieldValue::Checked(true);
                    if ui.checkbox(&mut checked, "").changed() {
                        commands.push(Command::SetFormFieldValue {
                            id: field.id,
                            value: FieldValue::Checked(checked),
                        });
                    }
                }
                FormFieldType::ComboBox | FormFieldType::ListBox => {
                    draw_choice_editor(ui, doc, field, commands);
                }
                // Radio buttons are chosen by clicking their rows; push
                // buttons and signature fields have nothing to edit here.
                FormFieldType::RadioButton
                | FormFieldType::PushButton
                | FormFieldType::Signature
                | FormFieldType::Unknown => {}
            }
        });
    let _ = palette;
}

/// A single-line text editor that commits on Enter or blur.
///
/// The draft lives in egui memory so it survives the panel being scrolled or
/// the sidebar being toggled mid-edit, without committing half-typed text.
fn draw_text_editor(
    ui: &mut Ui,
    doc: DocumentId,
    field: &FormFieldInfo,
    commands: &mut Vec<Command>,
) {
    let key = draft_key(doc, field.id);
    let current = field.value.as_text();

    let mut draft = ui.memory_mut(|mem| {
        mem.data
            .get_temp_mut_or_insert_with::<String>(key, || current.clone())
            .clone()
    });

    let response =
        ui.add(egui::TextEdit::singleline(&mut draft).desired_width(ui.available_width()));
    let changed = draft != current;
    let submit = response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

    if changed && (response.lost_focus() || submit) {
        commands.push(Command::SetFormFieldValue {
            id: field.id,
            value: FieldValue::Text(draft.clone()),
        });
    }
    if changed {
        // Keep the stored draft in step whether or not the edit committed, so
        // the next frame does not see already-typed text as new.
        ui.memory_mut(|mem| mem.data.insert_temp(key, draft));
    }

    // Enter commits without blurring in a single-line field, so keep the
    // caret where the user expects it for the next field.
    if submit {
        response.request_focus();
    }
}

/// A dropdown for combo and list box fields.
fn draw_choice_editor(
    ui: &mut Ui,
    doc: DocumentId,
    field: &FormFieldInfo,
    commands: &mut Vec<Command>,
) {
    let current = field.value.as_text();
    let id = Id::new((
        "form-choice",
        doc.raw(),
        field.id.page,
        field.id.annot_index,
    ));
    let selection = if current.is_empty() {
        "—"
    } else {
        current.as_str()
    };

    egui::ComboBox::new(id, "")
        .selected_text(RichText::new(selection).size(11.5))
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for option in &field.options {
                let label = if option.label.is_empty() {
                    "—"
                } else {
                    option.label.as_str()
                };
                if ui.selectable_label(option.selected, label).clicked() {
                    commands.push(Command::SetFormFieldValue {
                        id: field.id,
                        value: FieldValue::Choice(Some(option.label.clone())),
                    });
                }
            }
        });
}

/// The Forms panel: every field in the document, editable in place.
///
/// This is the list half of form filling. The other half is the overlay drawn
/// on the page itself; both read and write `ViewState::selected_field` so that
/// clicking a row here highlights the widget out there, and vice versa.
///
/// Text fields commit on Enter or when the input loses focus, so typing does
/// not trigger a PDFium write per keystroke; checkbox, radio and dropdown
/// changes are discrete and dispatch immediately.
pub fn forms_panel(
    ui: &mut Ui,
    palette: &Palette,
    doc: DocumentId,
    form: &FormInfo,
    selected: Option<FieldId>,
    style: TextStyle<'_>,
) -> Vec<Command> {
    let mut commands = Vec::new();

    if form.is_empty() {
        empty_sidebar(
            ui,
            palette,
            Icon::Document,
            "No form fields",
            "This document has no fillable form.",
        );
        return commands;
    }

    egui::Frame::new()
        .inner_margin(sidebar_margin())
        .show(ui, |ui| {
            // The total is every widget; "to fill" counts only the ones the
            // reader can actually write, so the note below explains the gap.
            let remaining = form.remaining_count();
            let summary = if remaining == 0 {
                format!("{} fields · complete", form.fields.len())
            } else {
                format!("{} fields · {} to fill", form.fields.len(), remaining)
            };
            ui.label(RichText::new(summary).color(palette.text_dim).size(11.0));
        });

    // Dropdowns and lists have no write path in the PDF binding at all. Saying
    // so up front beats a click that does nothing, or an error after one.
    let unwritable = form
        .fields
        .iter()
        .any(|field| !field.read_only && field.kind.has_options());
    if unwritable {
        egui::Frame::new()
            .inner_margin(sidebar_note_margin())
            .show(ui, |ui| {
                ui.label(
                    RichText::new(
                        "Dropdowns and lists cannot be filled here — the PDF library carries no \
                         setter for them, so those fields are not offered. Fill them in another \
                         viewer; text, checkbox and radio fields work here.",
                    )
                    .color(palette.text_dim)
                    .size(10.5),
                );
            });
    }

    // A radio group may keep its value on a parent dictionary the binding
    // cannot reach. Selecting one still works: the reader draws the choice into
    // the page, so it shows on screen, prints, and is saved with the document.
    let radios = form
        .fields
        .iter()
        .any(|field| !field.read_only && field.kind == FormFieldType::RadioButton);
    if radios {
        egui::Frame::new()
            .inner_margin(sidebar_note_margin())
            .show(ui, |ui| {
                ui.label(
                    RichText::new("Click a radio to select it.")
                        .color(palette.text_dim)
                        .size(10.5),
                );
            });
    }

    // Font controls for filling: the face and size used for typed values, so a
    // script the form's own font lacks (Bengali) and a hard-coded size that is
    // too small are both fixable without leaving the panel. Titled and explained
    // because it is the one control here that changes every field at once.
    egui::Frame::new()
        .inner_margin(sidebar_margin())
        .show(ui, |ui| {
            ui.label(
                RichText::new("Typing font")
                    .strong()
                    .color(palette.text)
                    .size(11.5),
            );
            ui.label(
                RichText::new("Face and size for text typed into fields.")
                    .color(palette.text_dim)
                    .size(10.5),
            );
            ui.add_space(SIDEBAR_GAP + 2.0);
            let label = FORM_FONTS
                .iter()
                .find(|(name, _)| *name == style.family)
                .map_or("Default", |(_, label)| *label);
            egui::ComboBox::from_id_salt("sidebar-form-font")
                .selected_text(label)
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for &(name, text) in FORM_FONTS {
                        if ui.selectable_label(style.family == name, text).clicked() {
                            commands.push(Command::SetFormFont(name.to_owned()));
                        }
                    }
                });
            ui.add_space(SIDEBAR_GAP);
            let mut size = style.size;
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
            if (size - style.size).abs() > f32::EPSILON {
                commands.push(Command::SetFormFontSize(size));
            }

            // Ink and weight for the typed values. A form's own colour can be
            // too pale to read once the value is drawn at a legible size.
            ui.add_space(SIDEBAR_GAP);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Colour").color(palette.text_dim).size(10.5));
                let mut rgb = style.color;
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    commands.push(Command::SetFormFontColor(rgb[0], rgb[1], rgb[2]));
                }
                ui.add_space(SIDEBAR_GAP + 2.0);
                if ui
                    .selectable_label(style.bold, RichText::new("B").strong())
                    .on_hover_text("Bold")
                    .clicked()
                {
                    commands.push(Command::SetFormFontBold(!style.bold));
                }
            });
        });
    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(sidebar_list_margin())
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = SIDEBAR_GAP;
                    for field in &form.fields {
                        let is_selected = selected == Some(field.id);
                        let response = show_form_row(ui, palette, field, is_selected);
                        if response.clicked() {
                            commands.push(Command::SelectFormField(Some(field.id)));
                            // Clicking a radio row is how that option gets
                            // chosen, matching how radio widgets behave on the
                            // page. Unchecking is intentionally not offered:
                            // the engine has no way to write it.
                            if field.kind == FormFieldType::RadioButton
                                && field.is_editable()
                                && field.value != FieldValue::Checked(true)
                            {
                                commands.push(Command::SetFormFieldValue {
                                    id: field.id,
                                    value: FieldValue::Checked(true),
                                });
                            }
                        }

                        if is_selected {
                            draw_field_editor(ui, palette, doc, field, &mut commands);
                        }
                    }
                });
        });

    commands
}

/// Draw one field row and return its interaction response.
fn show_form_row(
    ui: &mut Ui,
    palette: &Palette,
    field: &FormFieldInfo,
    is_selected: bool,
) -> egui::Response {
    // Second line: what kind of field it is, and what it currently holds.
    // A blank field shows a dash rather than nothing, so the column does not
    // collapse and look like a rendering bug.
    let detail = if field.value.is_empty() {
        format!("{} · —", field.kind.label())
    } else {
        format!(
            "{} · {}",
            field.kind.label(),
            truncate(field.value.as_text().trim(), 24)
        )
    };
    // Fields that cannot be filled are marked, because a user who cannot type
    // into one needs to be told why rather than guessing the app is broken.
    // Read-only is the form's own doing; "not fillable" means the reader has no
    // write path for that kind at all (radio groups, dropdowns).
    let detail = if field.read_only {
        format!("{detail} · locked")
    } else if !field.is_editable() {
        format!("{detail} · not fillable")
    } else {
        detail
    };

    let title = short_field_name(field.display_name());
    let (title, detail, height) = two_line_row(ui, &title, &detail, palette.text, palette.text_dim);

    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());

    if !ui.is_rect_visible(rect) {
        return response;
    }

    let fill = if is_selected {
        palette.accent_soft
    } else if response.hovered() {
        palette.surface_hover
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 4.0, fill);
    paint_two_line_row(ui, rect, title, detail, palette.text, palette.text_dim);
    response
}

/// The right-hand tools rail, in the spirit of a classic tool sidebar.
///
/// Only tools that exist and work are listed — the project has a no-dead-
/// controls policy. Each button maps to a command the app already handles.
pub fn tools_rail(ui: &mut Ui, state: &AppState, palette: &Palette) -> Vec<Command> {
    let mut commands = Vec::new();
    let has_doc = state.active().is_some_and(|t| t.document.is_some());

    egui::Frame::new()
        .fill(palette.panel_bg)
        .inner_margin(egui::Margin::symmetric(4, 8))
        .show(ui, |ui| {
            ui.set_min_width(TOOLS_RAIL_WIDTH - 8.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                if rail_button(
                    ui,
                    palette,
                    Icon::Sidebar,
                    "Sidebar",
                    "Toggle sidebar (Ctrl+B)",
                    state.sidebar_visible,
                ) {
                    commands.push(Command::ToggleSidebar);
                }
                ui.add_space(4.0);
                if rail_button(
                    ui,
                    palette,
                    Icon::Pages,
                    "Pages",
                    "Page thumbnails",
                    state.sidebar_tab == SidebarTab::Thumbnails && state.sidebar_visible,
                ) {
                    commands.push(Command::SetSidebarTab(SidebarTab::Thumbnails));
                }
                if rail_button(
                    ui,
                    palette,
                    Icon::Outline,
                    "Outline",
                    "Document outline",
                    state.sidebar_tab == SidebarTab::Outline && state.sidebar_visible,
                ) {
                    commands.push(Command::SetSidebarTab(SidebarTab::Outline));
                }
                if rail_button(
                    ui,
                    palette,
                    Icon::Search,
                    "Search",
                    "Search document",
                    state.sidebar_tab == SidebarTab::Search && state.sidebar_visible,
                ) {
                    commands.push(Command::SetSidebarTab(SidebarTab::Search));
                }
                ui.add_space(4.0);
                if rail_button(
                    ui,
                    palette,
                    Icon::RotateLeft,
                    "Rotate left",
                    "Rotate counter-clockwise",
                    false,
                ) {
                    commands.push(Command::RotateCcw);
                }
                if rail_button(
                    ui,
                    palette,
                    Icon::RotateRight,
                    "Rotate right",
                    "Rotate clockwise",
                    false,
                ) {
                    commands.push(Command::RotateCw);
                }
                let _ = has_doc;
            });
        });
    commands
}

/// A squared-off icon button for the tools rail.
fn rail_button(
    ui: &mut Ui,
    palette: &Palette,
    icon: Icon,
    label: &str,
    tooltip: &str,
    active: bool,
) -> bool {
    let inner = ui
        .add_enabled_ui(true, |ui| {
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(TOOLS_RAIL_WIDTH - 12.0, 32.0), Sense::click());
            let fill = if active {
                palette.accent_soft
            } else if response.hovered() {
                palette.surface_hover
            } else {
                egui::Color32::TRANSPARENT
            };
            if fill != egui::Color32::TRANSPARENT {
                ui.painter().rect_filled(rect, RAIL_RADIUS, fill);
            }
            draw_icon(
                ui.painter(),
                rect.center(),
                icon,
                if active { palette.accent } else { palette.text },
            );
            response
        })
        .inner;
    inner
        .on_hover_text(format!("{label} — {tooltip}"))
        .clicked()
}

fn menu_button(ui: &mut Ui, palette: &Palette, label: &str, add_contents: impl FnOnce(&mut Ui)) {
    ui.menu_button(
        RichText::new(label).color(palette.text_dim).size(11.5),
        add_contents,
    );
}

/// A top-level menu-bar item that performs an action directly, styled like the
/// dropdown labels beside it (dim text, no frame until hover).
fn menu_action(ui: &mut Ui, palette: &Palette, label: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(label).color(palette.text_dim).size(11.5)).frame(false))
}

fn menu_item(ui: &mut Ui, label: &str, shortcut: &str) -> egui::Response {
    let mut button = egui::Button::new(label);
    if !shortcut.is_empty() {
        button = button.shortcut_text(RichText::new(shortcut).weak().size(11.0));
    }
    ui.add(button)
}

fn menu_item_button<'a>(label: &'a str, shortcut: &'a str) -> egui::Button<'a> {
    let mut button = egui::Button::new(label);
    if !shortcut.is_empty() {
        button = button.shortcut_text(RichText::new(shortcut).weak().size(11.0));
    }
    button
}

fn toolbar_group(ui: &mut Ui, palette: &Palette, add_contents: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(palette.surface.gamma_multiply(0.55))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(3, 1))
        .show(ui, add_contents);
}

fn separator(ui: &mut Ui, palette: &Palette) {
    ui.add_space(3.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(1.0, 24.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, palette.border);
    ui.add_space(3.0);
}

fn primary_button(ui: &mut Ui, palette: &Palette, icon: Icon, label: &str, tooltip: &str) -> bool {
    let response = ui.add(
        egui::Button::new(
            RichText::new(format!("  {label}"))
                .color(palette.text)
                .size(12.0),
        )
        .fill(palette.accent_soft)
        .corner_radius(BUTTON_RADIUS)
        .min_size(Vec2::new(76.0, 30.0)),
    );
    if response.hovered() {
        ui.painter().rect_stroke(
            response.rect,
            BUTTON_RADIUS,
            Stroke::new(1.0, palette.accent),
            StrokeKind::Inside,
        );
    }
    draw_icon(
        ui.painter(),
        response.rect.left_center() + Vec2::new(13.0, 0.0),
        icon,
        palette.accent,
    );
    response.on_hover_text(tooltip).clicked()
}

fn icon_button(
    ui: &mut Ui,
    palette: &Palette,
    icon: Icon,
    _label: &str,
    tooltip: &str,
    active: bool,
    enabled: bool,
) -> bool {
    let inner = ui.add_enabled_ui(enabled, |ui| {
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(ICON_SIZE), Sense::click());
        let visuals = ui.style().interact(&response);
        let fill = if active {
            palette.accent_soft
        } else if response.hovered() {
            palette.surface_hover
        } else {
            visuals.bg_fill
        };
        if fill != Color32::TRANSPARENT {
            ui.painter().rect_filled(rect, BUTTON_RADIUS, fill);
        }
        let color = if ui.is_enabled() {
            palette.text
        } else {
            palette.text_dim.gamma_multiply(0.45)
        };
        draw_icon(ui.painter(), rect.center(), icon, color);
        response
    });
    inner.inner.on_hover_text(tooltip).clicked()
}

fn sidebar_tab_button(
    ui: &mut Ui,
    palette: &Palette,
    label: &str,
    icon: Icon,
    active: bool,
) -> bool {
    // Build the button from a measured icon+label layout rather than the old
    // trick of drawing the icon at a fixed offset and padding the label with
    // spaces. A measured width gives every tab the same icon-to-text gap and a
    // consistent padding, and the button sizes to its content so the strip can
    // wrap cleanly when the sidebar is narrowed.
    let icon_slot = 16.0;
    let gap = 5.0;
    let hpad = 8.0;
    let height = 28.0;

    let font_id = FontId::proportional(11.0);
    let text_w = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font_id.clone(), Color32::WHITE)
        .rect
        .width();
    // Never ask for more than the strip can give; the label is short enough
    // that this only kicks in on a deliberately crushed sidebar.
    let width = (hpad + icon_slot + gap + text_w + hpad).min(ui.available_width());

    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let painter = ui.painter();
    let fill = if active {
        palette.accent_soft
    } else if response.hovered() {
        palette.surface_hover
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        painter.rect_filled(rect, 6.0, fill);
    }
    draw_icon(
        painter,
        rect.min + Vec2::new(hpad + icon_slot / 2.0, height / 2.0),
        icon,
        if active {
            palette.accent
        } else {
            palette.text_dim
        },
    );
    painter.text(
        rect.min + Vec2::new(hpad + icon_slot + gap, height / 2.0),
        Align2::LEFT_CENTER,
        label,
        font_id,
        if active { palette.accent } else { palette.text },
    );
    response.on_hover_text(label).clicked()
}

fn text_button(ui: &mut Ui, palette: &Palette, label: &str, tooltip: &str, active: bool) -> bool {
    ui.add(
        egui::Button::new(RichText::new(label).color(palette.text).size(11.0))
            .fill(if active {
                palette.accent_soft
            } else {
                Color32::TRANSPARENT
            })
            .corner_radius(BUTTON_RADIUS)
            .min_size(Vec2::new(52.0, 30.0)),
    )
    .on_hover_text(tooltip)
    .clicked()
}

fn empty_sidebar(ui: &mut Ui, palette: &Palette, icon: Icon, title: &str, body: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(42.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(42.0), Sense::hover());
        ui.painter()
            .circle_filled(rect.center(), 21.0, palette.surface);
        draw_icon(ui.painter(), rect.center(), icon, palette.accent);
        ui.add_space(SIDEBAR_PAD_Y + SIDEBAR_GAP);
        ui.label(RichText::new(title).strong().color(palette.text).size(12.0));
        ui.add_space(SIDEBAR_GAP);
        ui.label(RichText::new(body).color(palette.text_dim).size(11.0));
    });
}

/// Deepest outline level rendered in the sidebar. The engine already caps
/// ingestion at 32 levels; this is defense in depth so a hand-built `Outline`
/// can never overflow the UI stack either.
const MAX_OUTLINE_RENDER_DEPTH: usize = 64;

fn show_outline_node(
    ui: &mut Ui,
    palette: &Palette,
    node: &OutlineNode,
    next_id: &mut u64,
    commands: &mut Vec<Command>,
    depth: usize,
) {
    let id = *next_id;
    *next_id += 1;
    let label = RichText::new(node.title.as_str())
        .color(palette.text)
        .size(11.5);
    if let Some(page) = node.page {
        if ui.selectable_label(false, label).clicked() {
            commands.push(Command::GoToPage(page));
        }
    } else {
        ui.label(label);
    }
    if !node.children.is_empty() && depth < MAX_OUTLINE_RENDER_DEPTH {
        ui.indent(Id::new(("outline-node", id)), |ui| {
            for child in &node.children {
                show_outline_node(ui, palette, child, next_id, commands, depth + 1);
            }
        });
    }
}

/// The View menu's fullscreen entry reflects the current state, so the user is
/// never left guessing whether F11 will enter or leave.
const fn fullscreen_label(state: &AppState) -> &'static str {
    match state.fullscreen {
        true => "Exit fullscreen",
        false => "Fullscreen",
    }
}

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

const fn view_mode_label(mode: ViewMode) -> &'static str {
    match mode {
        ViewMode::Single => "single page",
        ViewMode::Continuous => "continuous",
    }
}

/// Shorten a field's fully-qualified name for the Forms list.
///
/// XFA names are long paths that share a prefix — every field on page 1 of the
/// Japanese visa form starts `topmostSubform[0].Page1[0].` — so truncating the
/// *end* made every row read the same and the list was unusable. The last two
/// path segments are what actually tell the fields apart.
fn short_field_name(name: &str) -> String {
    let parts: Vec<&str> = name.split('.').collect();
    let start = parts.len().saturating_sub(2);
    elide_middle(&parts[start..].join("."), 30)
}

/// Keep the start and the end of `text`, eliding the middle.
///
/// For a path-like name both ends carry information — the container on the left
/// and the field on the right — so cutting one of them loses more than cutting
/// the middle does.
fn elide_middle(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_owned();
    }
    let keep = max_chars.saturating_sub(1) / 2;
    let head: String = text.chars().take(keep).collect();
    let tail: String = text.chars().skip(count - keep).collect();
    format!("{head}…{tail}")
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn draw_icon(painter: &egui::Painter, center: Pos2, icon: Icon, color: Color32) {
    let stroke = Stroke::new(1.6, color);
    let c = center;
    match icon {
        Icon::Open => {
            painter.rect_stroke(
                Rect::from_center_size(c + Vec2::new(1.0, 1.0), Vec2::new(13.0, 10.0)),
                2.0,
                stroke,
                StrokeKind::Inside,
            );
            painter.line(
                vec![
                    c + Vec2::new(-6.0, -4.0),
                    c + Vec2::new(-2.0, -4.0),
                    c + Vec2::new(0.0, -1.0),
                ],
                stroke,
            );
        }
        Icon::Previous => {
            painter.line(
                vec![
                    c + Vec2::new(4.0, -6.0),
                    c + Vec2::new(-3.0, 0.0),
                    c + Vec2::new(4.0, 6.0),
                ],
                stroke,
            );
        }
        Icon::Next => {
            painter.line(
                vec![
                    c + Vec2::new(-4.0, -6.0),
                    c + Vec2::new(3.0, 0.0),
                    c + Vec2::new(-4.0, 6.0),
                ],
                stroke,
            );
        }
        Icon::ZoomOut => {
            painter.circle_stroke(c + Vec2::new(-1.0, -1.0), 5.0, stroke);
            painter.line_segment([c + Vec2::new(3.0, 3.0), c + Vec2::new(7.0, 7.0)], stroke);
            painter.line_segment(
                [c + Vec2::new(-4.0, -1.0), c + Vec2::new(2.0, -1.0)],
                stroke,
            );
        }
        Icon::ZoomIn => {
            painter.circle_stroke(c + Vec2::new(-1.0, -1.0), 5.0, stroke);
            painter.line_segment([c + Vec2::new(3.0, 3.0), c + Vec2::new(7.0, 7.0)], stroke);
            painter.line_segment(
                [c + Vec2::new(-4.0, -1.0), c + Vec2::new(2.0, -1.0)],
                stroke,
            );
            painter.line_segment(
                [c + Vec2::new(-1.0, -4.0), c + Vec2::new(-1.0, 2.0)],
                stroke,
            );
        }
        Icon::RotateLeft => {
            painter.circle_stroke(c, 6.0, stroke);
            painter.line(
                vec![
                    c + Vec2::new(-5.0, -4.0),
                    c + Vec2::new(-6.0, 2.0),
                    c + Vec2::new(-1.0, 1.0),
                ],
                stroke,
            );
        }
        Icon::RotateRight => {
            painter.circle_stroke(c, 6.0, stroke);
            painter.line(
                vec![
                    c + Vec2::new(5.0, -4.0),
                    c + Vec2::new(6.0, 2.0),
                    c + Vec2::new(1.0, 1.0),
                ],
                stroke,
            );
        }
        Icon::Sidebar => {
            painter.rect_stroke(
                Rect::from_center_size(c, Vec2::new(14.0, 12.0)),
                2.0,
                stroke,
                StrokeKind::Inside,
            );
            painter.line_segment(
                [c + Vec2::new(-2.0, -5.0), c + Vec2::new(-2.0, 5.0)],
                stroke,
            );
        }
        Icon::Search => {
            painter.circle_stroke(c + Vec2::new(-2.0, -2.0), 5.0, stroke);
            painter.line_segment([c + Vec2::new(2.0, 2.0), c + Vec2::new(7.0, 7.0)], stroke);
        }
        Icon::Document => {
            painter.rect_stroke(
                Rect::from_center_size(c, Vec2::new(11.0, 14.0)),
                2.0,
                stroke,
                StrokeKind::Inside,
            );
            painter.line(
                vec![
                    c + Vec2::new(1.0, -7.0),
                    c + Vec2::new(5.0, -3.0),
                    c + Vec2::new(1.0, -3.0),
                ],
                stroke,
            );
        }
        Icon::Pages => {
            painter.rect_stroke(
                Rect::from_center_size(c + Vec2::new(-2.0, 1.0), Vec2::new(10.0, 12.0)),
                1.5,
                stroke,
                StrokeKind::Inside,
            );
            painter.rect_stroke(
                Rect::from_center_size(c + Vec2::new(2.0, -1.0), Vec2::new(10.0, 12.0)),
                1.5,
                stroke,
                StrokeKind::Inside,
            );
        }
        Icon::Outline => {
            for y in [-4.0, 0.0, 4.0] {
                painter.circle_filled(c + Vec2::new(-5.0, y), 1.0, color);
                painter.line_segment([c + Vec2::new(-1.0, y), c + Vec2::new(6.0, y)], stroke);
            }
        }
        Icon::Close => {
            painter.line_segment([c + Vec2::new(-4.0, -4.0), c + Vec2::new(4.0, 4.0)], stroke);
            painter.line_segment([c + Vec2::new(4.0, -4.0), c + Vec2::new(-4.0, 4.0)], stroke);
        }
        // The universal "move" glyph: four arrows pointing out from a centre
        // point. Reads as grab-and-drag at 16 px, unlike a literal hand which
        // turns to mush at icon size.
        Icon::Undo => {
            // Open circle (the sweep) with an arrowhead closing it, the
            // conventional "step back" glyph.
            let radius = 5.5;
            let points: Vec<Pos2> = (0..=10)
                .map(|i| {
                    let angle = -0.6 + (i as f32 / 10.0) * 4.4; // radians, CCW
                    c + Vec2::new(angle.cos() * radius, -angle.sin() * radius)
                })
                .collect();
            painter.line(points, stroke);
            // Arrowhead at the sweep's start (top-left), pointing left-down.
            painter.line(
                vec![
                    c + Vec2::new(-6.5, -6.5),
                    c + Vec2::new(-6.0, -1.5),
                    c + Vec2::new(-1.5, -2.0),
                ],
                stroke,
            );
        }
        // The same glyph mirrored: the sweep runs the other way and the
        // arrowhead sits top-right, which is what "step forward" looks like.
        Icon::Redo => {
            let radius = 5.5;
            let points: Vec<Pos2> = (0..=10)
                .map(|i| {
                    let angle = -0.6 + (i as f32 / 10.0) * 4.4; // radians, CCW
                    c + Vec2::new(-(angle.cos() * radius), -angle.sin() * radius)
                })
                .collect();
            painter.line(points, stroke);
            // Arrowhead at the sweep's start (top-right), pointing right-down.
            painter.line(
                vec![
                    c + Vec2::new(6.5, -6.5),
                    c + Vec2::new(6.0, -1.5),
                    c + Vec2::new(1.5, -2.0),
                ],
                stroke,
            );
        }
        Icon::Pan => {
            let arm = 6.0;
            let head = 2.2;
            for dir in [
                Vec2::new(0.0, -1.0),
                Vec2::new(0.0, 1.0),
                Vec2::new(-1.0, 0.0),
                Vec2::new(1.0, 0.0),
            ] {
                let tip = c + dir * arm;
                painter.line_segment([c + dir * 1.5, tip], stroke);
                // Arrow head: two short strokes angled back from the tip.
                let perp = Vec2::new(-dir.y, dir.x);
                painter.line_segment([tip, tip - dir * head + perp * head], stroke);
                painter.line_segment([tip, tip - dir * head - perp * head], stroke);
            }
            painter.circle_filled(c, 1.3, color);
        }
        Icon::More => {
            for x in [-5.0, 0.0, 5.0] {
                painter.circle_filled(c + Vec2::new(x, 0.0), 1.5, color);
            }
        }
        Icon::Annotation => {
            // A sticky-note / speech-bubble glyph: rounded square with a small
            // tail at the bottom-left, the conventional "note" mark.
            let r = Rect::from_center_size(c + Vec2::new(0.0, -1.0), Vec2::new(11.0, 10.0));
            painter.rect_stroke(r, 2.6, stroke, StrokeKind::Inside);
            painter.line(
                vec![
                    c + Vec2::new(-5.5, 4.0),
                    c + Vec2::new(-5.5, 7.5),
                    c + Vec2::new(-2.0, 4.0),
                ],
                stroke,
            );
        }
        // ---- Annotation tools: a miniature of the mark each one makes ----
        Icon::SelectTool => {
            // The classic pointer arrow: a triangular head plus a tail. Both
            // are convex so they can be filled (egui only fills convex paths).
            painter.add(egui::Shape::convex_polygon(
                vec![
                    c + Vec2::new(-4.5, -7.0),
                    c + Vec2::new(-4.5, 5.0),
                    c + Vec2::new(4.5, -0.5),
                ],
                color,
                Stroke::NONE,
            ));
            painter.add(egui::Shape::convex_polygon(
                vec![
                    c + Vec2::new(0.5, -0.5),
                    c + Vec2::new(3.5, 0.8),
                    c + Vec2::new(1.0, 5.5),
                    c + Vec2::new(-1.5, 3.0),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::HighlightTool => {
            // A solid band — the highlight covers the text it marks.
            painter.rect_filled(
                Rect::from_center_size(c, Vec2::new(13.0, 7.0)),
                2.5,
                color,
            );
        }
        Icon::UnderlineTool => {
            // A line of text with the rule drawn beneath it.
            painter.rect_stroke(
                Rect::from_center_size(c + Vec2::new(0.0, -3.0), Vec2::new(12.0, 6.0)),
                1.0,
                stroke,
                StrokeKind::Inside,
            );
            painter.line_segment(
                [c + Vec2::new(-6.0, 5.0), c + Vec2::new(6.0, 5.0)],
                Stroke::new(2.0, color),
            );
        }
        Icon::StrikeoutTool => {
            // A line of text with the rule struck through the middle.
            painter.rect_stroke(
                Rect::from_center_size(c, Vec2::new(12.0, 7.0)),
                1.0,
                stroke,
                StrokeKind::Inside,
            );
            painter.line_segment(
                [c + Vec2::new(-6.5, 0.0), c + Vec2::new(6.5, 0.0)],
                Stroke::new(2.0, color),
            );
        }
        Icon::SquigglyTool => {
            // A line of text with the zig-zag rule beneath it.
            painter.rect_stroke(
                Rect::from_center_size(c + Vec2::new(0.0, -3.5), Vec2::new(12.0, 6.0)),
                1.0,
                stroke,
                StrokeKind::Inside,
            );
            let zig: Vec<Pos2> = (0..=6)
                .map(|i| {
                    let x = -6.0 + i as f32 * 2.0;
                    let y = if i % 2 == 0 { 4.0 } else { 6.5 };
                    c + Vec2::new(x, y)
                })
                .collect();
            painter.line(zig, Stroke::new(1.6, color));
        }
        Icon::RectangleTool => {
            painter.rect_stroke(
                Rect::from_center_size(c, Vec2::new(13.0, 9.0)),
                1.5,
                stroke,
                StrokeKind::Inside,
            );
        }
        Icon::NoteTool => {
            // The folded-corner sticky note.
            let r = Rect::from_center_size(c, Vec2::new(12.0, 12.0));
            painter.rect_stroke(r, 1.5, stroke, StrokeKind::Inside);
            painter.line(
                vec![
                    r.right_top() + Vec2::new(-4.5, 0.0),
                    r.right_top() + Vec2::new(0.0, 4.5),
                    r.right_top() + Vec2::new(-4.5, 4.5),
                ],
                stroke,
            );
        }
        Icon::EraserTool => {
            // A slanted eraser block lifted off the surface line.
            painter.add(egui::Shape::convex_polygon(
                vec![
                    c + Vec2::new(-5.0, 0.0),
                    c + Vec2::new(-0.5, -6.5),
                    c + Vec2::new(5.5, -1.0),
                    c + Vec2::new(1.0, 5.0),
                ],
                color,
                Stroke::NONE,
            ));
            painter.line_segment(
                [c + Vec2::new(-6.5, 6.0), c + Vec2::new(6.5, 6.0)],
                Stroke::new(1.6, color),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Palette, SEARCH_FIELD_HEIGHT, SearchFieldOutput, TOOLBAR_HEIGHT, TOOLBAR_PAD_Y,
        annotation_properties_window, comments_panel, search_field, status_bar, toolbar,
    };
    use crate::theme::Theme;
    use egui::{Context, RawInput, Rect};
    use pdfreader_core::{
        Command, Document, DocumentId, Effect, Outline, PageGeometry, Rotation, Store, ThemeId,
    };
    use std::path::PathBuf;

    /// A sidebar row must fit both of its lines with the padding it asks for.
    ///
    /// The row height is measured from the galleys, so the contract is that
    /// padding plus both lines plus the gap adds up to exactly the height — and
    /// that the second line therefore ends above the row's bottom edge rather
    /// than being clipped by it.
    #[test]
    fn a_sidebar_row_fits_its_lines_and_its_padding() {
        use super::{ROW_LINE_GAP, ROW_PAD_Y, two_line_row};

        let ctx = Context::default();
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(280.0, 120.0),
            )),
            ..Default::default()
        };
        let mut measured = None;
        let mut output = ctx.run_ui(raw, |ui| {
            let (title, detail, height) = two_line_row(
                ui,
                "FullName",
                "Text · Jane Doe",
                egui::Color32::WHITE,
                egui::Color32::GRAY,
            );
            measured = Some((title.size().y, detail.size().y, height));
        });
        output.textures_delta.clear();

        let (title, detail, height) = measured.expect("a frame ran");
        assert!(
            title > 0.0 && detail > 0.0,
            "both lines must have been laid out"
        );
        let expected = ROW_PAD_Y * 2.0 + title + ROW_LINE_GAP + detail;
        assert!(
            (height - expected).abs() < 1e-3,
            "the row must be its padding plus its content: {height} vs {expected}"
        );
        // The bottom edge has to clear the detail line, which is what a fixed
        // row height got wrong.
        let detail_bottom = ROW_PAD_Y + title + ROW_LINE_GAP + detail;
        assert!(
            detail_bottom <= height - ROW_PAD_Y + 1e-3,
            "the second line must end inside the row's padding"
        );
    }

    /// Draw the floating annotation properties window for one frame and
    /// return the commands it produced.
    fn draw_properties_window(
        ctx: &egui::Context,
        palette: &Palette,
        annotations: &[pdfreader_core::AnnotationInfo],
        selected: Option<pdfreader_core::AnnotationId>,
    ) -> Vec<Command> {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        };
        let mut commands = Vec::new();
        let mut out = ctx.run_ui(raw, |ui| {
            commands = annotation_properties_window(ui.ctx(), palette, annotations, selected);
        });
        out.textures_delta.clear();
        commands
    }

    /// Draw the properties window with the pointer pressed (`pressed = true`)
    /// or released (`pressed = false`) at `pos`, returning the commands.
    fn draw_properties_window_pointer(
        ctx: &egui::Context,
        palette: &Palette,
        annotations: &[pdfreader_core::AnnotationInfo],
        selected: Option<pdfreader_core::AnnotationId>,
        pos: egui::Pos2,
        pressed: bool,
    ) -> Vec<Command> {
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        };
        raw.events.push(egui::Event::PointerMoved(pos));
        raw.events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        });
        let mut commands = Vec::new();
        let mut out = ctx.run_ui(raw, |ui| {
            commands = annotation_properties_window(ui.ctx(), palette, annotations, selected);
        });
        out.textures_delta.clear();
        commands
    }

    /// The properties window must never emit edits for an annotation the user
    /// cannot edit (links, stamps, …): opening it on one yields an empty command
    /// list instead of a colour or bounds write.
    #[test]
    fn properties_window_emits_nothing_for_a_non_creatable_annotation() {
        let ctx = Context::default();
        let palette = Theme::from_id(ThemeId::default()).palette;
        let annotations = [pdfreader_core::AnnotationInfo {
            id: pdfreader_core::AnnotationId::new(0, 0),
            kind: pdfreader_core::AnnotationKind::Link,
            rect: pdfreader_core::Rect::from_xywh(0.0, 0.0, 10.0, 10.0),
            contents: None,
            color: None,
        }];
        let commands = draw_properties_window(
            &ctx,
            &palette,
            &annotations,
            Some(pdfreader_core::AnnotationId::new(0, 0)),
        );
        assert!(
            commands.is_empty(),
            "a non-creatable annotation yields no panel commands: {commands:?}"
        );
    }

    /// Headless stand-in for "click the red swatch, the annotation turns red":
    /// sweep the window's swatch row with press+release clicks until one lands
    /// on a swatch and the panel returns a `SetAnnotationColor`. Covers the
    /// properties-panel field commit the manual click-through could not verify.
    #[test]
    fn properties_window_swatch_click_emits_set_annotation_color() {
        let ctx = Context::default();
        let palette = Theme::from_id(ThemeId::default()).palette;
        let id = pdfreader_core::AnnotationId::new(0, 0);
        let annotations = [pdfreader_core::AnnotationInfo {
            id,
            kind: pdfreader_core::AnnotationKind::Square,
            rect: pdfreader_core::Rect::from_xywh(10.0, 20.0, 40.0, 10.0),
            contents: None,
            color: Some([255, 0, 0, 200]),
        }];

        // The window anchors RIGHT_CENTER at (-24, 0) on an 800×600 screen, so
        // it occupies the right-hand strip. Sweep that strip with press+release
        // clicks until one lands on a swatch.
        let mut hit = None;
        'outer: for y in (200..=400).step_by(8) {
            for x in (510..=770).step_by(10) {
                let p = egui::pos2(x as f32, y as f32);
                draw_properties_window_pointer(&ctx, &palette, &annotations, Some(id), p, true);
                let released =
                    draw_properties_window_pointer(&ctx, &palette, &annotations, Some(id), p, false);
                if let Some(cmd) = released
                    .into_iter()
                    .find(|c| matches!(c, Command::SetAnnotationColor { .. }))
                {
                    hit = Some(cmd);
                    break 'outer;
                }
            }
        }
        assert!(
            hit.is_some_and(|c| match c {
                Command::SetAnnotationColor { id: ref cid, .. } => *cid == id,
                _ => false,
            }),
            "clicking a swatch must emit SetAnnotationColor for the selected annotation"
        );
    }

    /// Draw the Comments panel for one frame with the pointer pressed
    /// (`pressed = true`) or released (`pressed = false`) at `pos`, returning
    /// the commands it produced.
    fn draw_comments_panel_pointer(
        ctx: &Context,
        palette: &Palette,
        doc: DocumentId,
        annotations: &[pdfreader_core::AnnotationInfo],
        selected: Option<pdfreader_core::AnnotationId>,
        pos: egui::Pos2,
        pressed: bool,
    ) -> Vec<Command> {
        let mut raw = RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(320.0, 600.0),
            )),
            ..Default::default()
        };
        raw.events.push(egui::Event::PointerMoved(pos));
        raw.events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        });
        let mut commands = Vec::new();
        let mut out = ctx.run_ui(raw, |ui| {
            commands = comments_panel(ui, palette, doc, annotations, selected);
        });
        out.textures_delta.clear();
        commands
    }

    /// Hyperlinks are annotations but not comments: the Comments panel must not
    /// list them (one row per link buried the real comments in link-heavy
    /// PDFs), while genuine markup stays listed and selectable.
    #[test]
    fn comments_panel_hides_hyperlinks() {
        let ctx = Context::default();
        let palette = Theme::from_id(ThemeId::default()).palette;
        let doc = DocumentId::from_raw(1);
        let link = pdfreader_core::AnnotationId::new(0, 0);
        let square = pdfreader_core::AnnotationId::new(0, 1);
        let annotations = [
            pdfreader_core::AnnotationInfo {
                id: link,
                kind: pdfreader_core::AnnotationKind::Link,
                rect: pdfreader_core::Rect::from_xywh(0.0, 0.0, 10.0, 10.0),
                contents: Some("https://example.com".to_string()),
                color: None,
            },
            pdfreader_core::AnnotationInfo {
                id: square,
                kind: pdfreader_core::AnnotationKind::Square,
                rect: pdfreader_core::Rect::from_xywh(0.0, 0.0, 10.0, 10.0),
                contents: None,
                color: Some([255, 0, 0, 255]),
            },
        ];

        // Sweep the panel with press+release clicks and collect every id the
        // panel offers to select.
        let mut selected_ids = Vec::new();
        for y in (0..=600).step_by(6) {
            let p = egui::pos2(120.0, y as f32);
            draw_comments_panel_pointer(&ctx, &palette, doc, &annotations, None, p, true);
            for cmd in
                draw_comments_panel_pointer(&ctx, &palette, doc, &annotations, None, p, false)
            {
                if let Command::SelectAnnotation(Some(id)) = cmd {
                    selected_ids.push(id);
                }
            }
        }
        assert!(
            !selected_ids.contains(&link),
            "the hyperlink must not be listed in the Comments panel: {selected_ids:?}"
        );
        assert!(
            selected_ids.contains(&square),
            "the real annotation must still be listed and selectable: {selected_ids:?}"
        );
    }

    /// Width the find field is drawn at in these tests.
    const TEST_FIELD_WIDTH: f32 = 240.0;

    /// One frame of the find field: what the widget reported, and how many
    /// primitives it painted. `pointer` is a position plus whether the primary
    /// button is down — egui reports a click on release, so a test presses one
    /// frame and releases the next.
    fn find_frame(
        ctx: &Context,
        time: f64,
        query: &mut String,
        pointer: Option<(egui::Pos2, bool)>,
        enter: bool,
    ) -> (SearchFieldOutput, usize) {
        let mut raw = RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(TEST_FIELD_WIDTH + 40.0, 120.0),
            )),
            time: Some(time),
            ..Default::default()
        };
        if let Some((pos, pressed)) = pointer {
            raw.events.push(egui::Event::PointerMoved(pos));
            raw.events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            });
        }
        if enter {
            raw.events.push(egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            });
        }

        let palette = Theme::from_id(ThemeId::default()).palette;
        let mut outcome = SearchFieldOutput::default();
        let mut output = ctx.run_ui(raw, |ui| {
            outcome = search_field(
                ui,
                &palette,
                query,
                TEST_FIELD_WIDTH,
                "Search document",
                false,
            );
        });
        let shapes = output.shapes.len();
        output.textures_delta.clear();
        (outcome, shapes)
    }

    /// Regression: the toolbar is a `Panel::top` with an exact height of
    /// [`TOOLBAR_HEIGHT`]. Anything taller is painted over the strip below it,
    /// which is what made the find box look like it was slipping under the next
    /// section. The row has to fit the strip it is given.
    #[test]
    fn toolbar_row_fits_its_strip() {
        let ctx = Context::default();
        let palette = Theme::from_id(ThemeId::default()).palette;
        let store = store_rotated_to(Rotation::None);
        let mut query = String::new();
        let mut pan = false;

        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1200.0, 80.0),
            )),
            time: Some(0.0),
            ..Default::default()
        };
        let mut output = ctx.run_ui(raw, |ui| {
            toolbar(ui, store.state(), &palette, &mut query, &mut pan, false);
        });
        let bottom = output
            .shapes
            .iter()
            .map(|s| s.shape.visual_bounding_rect())
            .filter(egui::Rect::is_finite)
            .map(|r| r.max.y)
            .fold(0.0, f32::max);
        output.textures_delta.clear();

        assert!(
            bottom <= TOOLBAR_HEIGHT + 1.0,
            "the toolbar row paints to {bottom:.1} px but only {TOOLBAR_HEIGHT} px are reserved"
        );
    }

    /// The find box has to sit inside the toolbar's own padding, which is why
    /// it is shorter than the 30 px icon buttons rather than the same height.
    #[test]
    fn find_field_fits_between_the_toolbar_padding() {
        let available = TOOLBAR_HEIGHT - f32::from(TOOLBAR_PAD_Y) * 2.0;
        assert!(
            SEARCH_FIELD_HEIGHT <= available,
            "the find box is {SEARCH_FIELD_HEIGHT} px but the toolbar only has {available} px"
        );
    }

    /// Presses and releases the primary button at `pos`, which is what egui
    /// counts as one click.
    fn click(ctx: &Context, query: &mut String, pos: egui::Pos2) -> (SearchFieldOutput, usize) {
        // egui ignores the pointer until it has been somewhere for a frame, so
        // the first press of a run is not a click: warm up, then press, release.
        find_frame(ctx, 0.0, query, None, false);
        find_frame(ctx, 0.1, query, Some((pos, true)), false);
        find_frame(ctx, 0.2, query, Some((pos, false)), false)
    }

    /// The field is one bordered group: an addon with the magnifier and the
    /// word "Find", the text, and a clear button that only exists once there is
    /// a query to clear.
    #[test]
    fn find_field_paints_the_group_and_the_clear_button_only_with_a_query() {
        let ctx = Context::default();
        let mut empty = String::new();
        let mut filled = String::from("pdf");

        let (_, empty_shapes) = find_frame(&ctx, 0.0, &mut empty, None, false);
        let (_, filled_shapes) = find_frame(&ctx, 0.1, &mut filled, None, false);

        assert!(
            empty_shapes >= 8,
            "an empty field still paints the group, the addon and the hint: {empty_shapes}"
        );
        assert!(
            filled_shapes > empty_shapes,
            "a query adds the text and the clear button: {filled_shapes} vs {empty_shapes}"
        );
    }

    /// Enter runs the search — the field has no submit button, so the key is
    /// the only way to run a query.
    #[test]
    fn enter_in_the_find_field_submits() {
        let ctx = Context::default();
        let mut query = String::from("pdf");

        // Click past the addon, inside the text half, to take focus.
        click(&ctx, &mut query, egui::pos2(140.0, 15.0));
        let (outcome, _) = find_frame(&ctx, 0.3, &mut query, None, true);

        assert!(outcome.submitted, "Enter while the field has focus submits");
        assert!(!outcome.cleared, "submitting is not clearing");
        assert_eq!(query, "pdf", "submitting leaves the query alone");
    }

    /// The × lives inside the group's right edge, not beside it as its own
    /// toolbar button.
    #[test]
    fn clicking_the_clear_button_empties_the_query() {
        let ctx = Context::default();
        let mut query = String::from("pdf");

        let (outcome, _) = click(&ctx, &mut query, egui::pos2(TEST_FIELD_WIDTH - 11.0, 15.0));

        assert!(outcome.cleared, "the × sits inside the group's right edge");
        assert!(query.is_empty(), "clearing empties the query itself");
        assert!(!outcome.submitted, "clearing is not submitting");
    }

    /// A store with one opened three-page document rotated to `rotation`.
    fn store_rotated_to(rotation: Rotation) -> Store {
        let mut store = Store::new();
        let effects = store.dispatch(Command::OpenPath(PathBuf::from("doc.pdf")));
        let Effect::OpenDocument { tab, .. } = effects[0].clone() else {
            panic!("opening a path must ask the shell to load it");
        };
        store.dispatch(Command::DocumentOpened {
            tab,
            document: Document {
                id: DocumentId::from_raw(1),
                path: PathBuf::from("doc.pdf"),
                title: "doc".into(),
                pages: vec![PageGeometry::A4; 3],
                encrypted: false,
                outline: Outline::default(),
            },
        });
        for _ in 0..rotation.quarter_turns() {
            store.dispatch(Command::RotateCw);
        }
        store
    }

    /// Regression: the status bar derived its rotation label from
    /// `quarter_turns() * 90`, and `quarter_turns()` is a `u8`, so three turns
    /// (270 degrees) overflowed and panicked in debug builds. Rendering the bar
    /// must survive every rotation.
    #[test]
    fn status_bar_renders_at_every_rotation() {
        let ctx = Context::default();
        let palette = Theme::from_id(ThemeId::default()).palette;

        for rotation in [
            Rotation::None,
            Rotation::Cw90,
            Rotation::Cw180,
            Rotation::Cw270,
        ] {
            let store = store_rotated_to(rotation);
            assert_eq!(
                store.state().active().expect("a tab").view.rotation,
                rotation,
                "the fixture must actually reach {rotation:?}"
            );

            let raw = RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(900.0, 40.0),
                )),
                ..Default::default()
            };
            let mut output = ctx.run_ui(raw, |ui| {
                status_bar(ui, store.state(), &palette);
            });
            // The GPU backend would normally apply these; clear them so dropping
            // the frame output does not trip epaint's "unapplied deltas" assert.
            output.textures_delta.clear();
        }
    }
}

// --- Annotation properties -------------------------------------------------
//
// The panel edits the annotation that is selected on the canvas: colour
// swatches, an opacity slider, and the rectangle as four numeric fields. Every
// control emits a domain command; the reducer owns the state, the engine owns
// the pixels, and undo records every change.

/// Colour presets the properties panel offers, as RGBA bytes. The alpha byte
/// is the annotation's rendered opacity, so these swatches are fully opaque.
const ANNOTATION_SWATCHES: [[u8; 4]; 8] = [
    [255, 202, 40, 255],  // yellow — the highlight default
    [76, 175, 80, 255],   // green
    [33, 150, 243, 255],  // blue
    [244, 67, 54, 255],   // red — the rectangle default
    [255, 152, 0, 255],   // orange
    [156, 39, 176, 255],  // purple
    [96, 125, 139, 255],  // grey
    [233, 30, 99, 255],   // pink
];

/// Draft state for the properties window.
///
/// The engine is asynchronous: after a control is released, the PDFium list
/// may still contain the old values for a few frames. Keeping the draft in
/// egui's temporary memory prevents the slider and numeric fields from snapping
/// back to those old values while the request is in flight.
#[derive(Clone, PartialEq)]
struct AnnotationPropertiesDraft {
    color: [u8; 4],
    rect: pdfreader_core::Rect,
    pending_color: bool,
    pending_rect: bool,
}

impl AnnotationPropertiesDraft {
    fn from_annotation(annotation: &AnnotationInfo) -> Self {
        Self {
            color: annotation.color.unwrap_or([0, 0, 0, 255]),
            rect: annotation.rect,
            pending_color: false,
            pending_rect: false,
        }
    }
}

/// The floating properties window for the selected annotation.
///
/// Every control keeps its value in an annotation-specific draft and commits
/// once per completed gesture. This is deliberately not a local `let mut`
/// value: local widget values are recreated from the stale PDFium list every
/// frame, which made opacity and geometry appear not to apply.
pub fn annotation_properties_window(
    ctx: &egui::Context,
    palette: &Palette,
    annotations: &[AnnotationInfo],
    selected: Option<AnnotationId>,
) -> Vec<Command> {
    let mut commands = Vec::new();
    let Some(selected) = selected else {
        return commands;
    };
    let Some(annotation) = annotations.iter().find(|a| a.id == selected) else {
        return commands;
    };
    if !annotation.kind.is_creatable() {
        return commands;
    }

    let state_id = Id::new(("annotation-properties", annotation.id));
    let mut draft = ctx.data(|data| data.get_temp::<AnnotationPropertiesDraft>(state_id))
        .unwrap_or_else(|| AnnotationPropertiesDraft::from_annotation(annotation));
    let authoritative_color = annotation.color.unwrap_or(draft.color);

    // A successful optimistic reducer update confirms the in-flight value.
    // Do not overwrite an unfinished draft with an older engine response.
    if draft.pending_color && authoritative_color == draft.color {
        draft.pending_color = false;
    }
    if !draft.pending_color {
        draft.color = authoritative_color;
    }
    if draft.pending_rect && annotation.rect == draft.rect {
        draft.pending_rect = false;
    }
    if !draft.pending_rect {
        draft.rect = annotation.rect;
    }

    let title = format!("{} · page {}", annotation.kind.label(), annotation.id.page + 1);
    dialog_window(&title, palette)
        .anchor(Align2::RIGHT_CENTER, Vec2::new(-24.0, 0.0))
        .default_width(270.0)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.label(RichText::new("Changes apply automatically").size(11.0).color(palette.text_dim));

            ui.label(RichText::new("Colour").size(11.0).color(palette.text_dim));
            egui::Grid::new(state_id.with("swatches"))
                .num_columns(8)
                .spacing([6.0, 6.0])
                .show(ui, |ui| {
                    for swatch in ANNOTATION_SWATCHES {
                        let fill = Color32::from_rgba_unmultiplied(
                            swatch[0], swatch[1], swatch[2], swatch[3],
                        );
                        let is_current = swatch[0..3] == draft.color[0..3];
                        let (rect, response) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
                        let painter = ui.painter();
                        painter.rect_filled(rect, 4.0, fill);
                        painter.rect_stroke(
                            rect,
                            4.0,
                            Stroke::new(if is_current { 2.0 } else { 1.0 }, if is_current { palette.text } else { palette.text_dim }),
                            StrokeKind::Inside,
                        );
                        if response.clicked() {
                            draft.color = [swatch[0], swatch[1], swatch[2], draft.color[3]];
                            draft.pending_color = true;
                            commands.push(Command::SetAnnotationColor {
                                id: annotation.id,
                                color: draft.color,
                            });
                        }
                    }
                    ui.end_row();
                });

            ui.horizontal(|ui| {
                ui.label(RichText::new("Opacity").size(11.0).color(palette.text_dim));
                let mut percent = (draft.color[3] as f32 / 255.0 * 100.0).round();
                let response = ui.add(
                    egui::Slider::new(&mut percent, 10.0..=100.0)
                        .suffix("%")
                        .show_value(true),
                );
                if response.changed() {
                    draft.color[3] = (percent / 100.0 * 255.0).round().clamp(26.0, 255.0) as u8;
                }
                if response.drag_stopped() || (response.changed() && !ui.input(|i| i.pointer.primary_down())) {
                    draft.pending_color = true;
                    commands.push(Command::SetAnnotationColor {
                        id: annotation.id,
                        color: draft.color,
                    });
                }
            });

            ui.label(RichText::new("Position and size (PDF points)").size(11.0).color(palette.text_dim));
            let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
            let mut commit_rect = false;
            egui::Grid::new(state_id.with("geometry"))
                .num_columns(4)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    let values = [
                        ("X", draft.rect.min_x, -1_000_000.0),
                        ("Y", draft.rect.min_y, -1_000_000.0),
                        ("W", draft.rect.width(), 1.0),
                        ("H", draft.rect.height(), 1.0),
                    ];
                    for (label, value, min) in values {
                        ui.label(RichText::new(label).size(11.0).color(palette.text_dim));
                        let mut edited = value;
                        let response = ui.add(
                            egui::DragValue::new(&mut edited)
                                .speed(0.5)
                                .range(min..=1_000_000.0),
                        );
                        if response.changed() {
                            draft.rect = match label {
                                "X" => pdf_rect_moved_x(draft.rect, edited),
                                "Y" => pdf_rect_moved_y(draft.rect, edited),
                                "W" => pdf_rect_with_width(draft.rect, edited),
                                _ => pdf_rect_with_height(draft.rect, edited),
                            };
                        }
                        // Commit on mouse release, Enter, or blur. The draft
                        // may have changed one frame earlier while typing, so
                        // do not require `lost_focus && changed` on the same
                        // frame.
                        commit_rect |= response.drag_stopped()
                            || response.lost_focus()
                            || enter_pressed;
                    }
                    ui.end_row();
                });
            if commit_rect && draft.rect != annotation.rect {
                draft.pending_rect = true;
                commands.push(Command::UpdateAnnotationBounds {
                    id: annotation.id,
                    rect: draft.rect,
                });
            }

            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new("Changes are undoable with Ctrl+Z").size(10.0).color(palette.text_dim));
                if ui.button("Delete").clicked() {
                    commands.push(Command::DeleteAnnotation(annotation.id));
                }
            });
        });

    ctx.data_mut(|data| {
        let saved = draft.clone();
        let initial = saved.clone();
        *data.get_temp_mut_or_insert_with(state_id, || initial) = saved;
    });
    commands
}

/// The rectangle with `min_x` moved to `x`, size preserved.
fn pdf_rect_moved_x(rect: pdfreader_core::Rect, x: f32) -> pdfreader_core::Rect {
    pdfreader_core::Rect::from_xywh(
        x,
        rect.min_y,
        rect.width(),
        rect.height(),
    )
}

/// The rectangle with `min_y` moved to `y`, size preserved.
fn pdf_rect_moved_y(rect: pdfreader_core::Rect, y: f32) -> pdfreader_core::Rect {
    pdfreader_core::Rect::from_xywh(
        rect.min_x,
        y,
        rect.width(),
        rect.height(),
    )
}

/// The rectangle with its width set to `w`, corner fixed.
fn pdf_rect_with_width(rect: pdfreader_core::Rect, w: f32) -> pdfreader_core::Rect {
    pdfreader_core::Rect::from_xywh(rect.min_x, rect.min_y, w.max(1.0), rect.height())
}

/// The rectangle with its height set to `h`, corner fixed.
fn pdf_rect_with_height(rect: pdfreader_core::Rect, h: f32) -> pdfreader_core::Rect {
    pdfreader_core::Rect::from_xywh(rect.min_x, rect.min_y, rect.width(), h.max(1.0))
}
