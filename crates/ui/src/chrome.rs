//! Modern desktop chrome for the PDF reader.
//!
//! The widgets in this module are intentionally presentation-only. They emit
//! domain commands and leave document state, rendering, and persistence to the
//! application shell.

use egui::{
    Align2, Color32, FontId, Id, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2,
    ViewportCommand,
};
use pdfreader_core::{
    AnnotationId, AnnotationInfo, AnnotationKind, AppState, Command, DocumentId, FieldId,
    FieldValue, FormFieldInfo, FormFieldType, FormInfo, ImageFormat, Outline, OutlineNode,
    SidebarTab, Tab, ThemeId, Tool, ViewMode, ZoomMode,
};

use crate::theme::Palette;

/// Height of the compact application menu.
pub const MENUBAR_HEIGHT: f32 = 28.0;
/// Height of the main command toolbar. Acrobat Pro DC keeps its toolbar tight;
/// 40px fits the icon rows without wasted chrome.
pub const TOOLBAR_HEIGHT: f32 = 40.0;
/// Height of the document tab strip.
pub const TABBAR_HEIGHT: f32 = 34.0;
/// Height of the quiet status bar.
pub const STATUSBAR_HEIGHT: f32 = 28.0;
/// Width of the right-hand tools rail.
pub const TOOLS_RAIL_WIDTH: f32 = 44.0;

const BUTTON_RADIUS: u8 = 7;
const RAIL_RADIUS: u8 = 2;
const ICON_SIZE: f32 = 30.0;

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

    if menu_item(ui, "Open document…", "Ctrl+O").clicked() {
        commands.push(Command::ShowOpenDialog);
        ui.close();
    }

    ui.add_enabled_ui(has_doc, |ui| {
        if menu_item(ui, "Save", "Ctrl+S").clicked() {
            commands.push(Command::SaveDocument);
            ui.close();
        }
        if menu_item(ui, "Save as…", "Ctrl+Shift+S").clicked() {
            commands.push(Command::SaveDocumentAs);
            ui.close();
        }
        if menu_item(ui, "Flatten form and annotations", "").clicked() {
            commands.push(Command::FlattenDocument);
            ui.close();
        }

        ui.separator();
        ui.menu_button("Export", |ui| {
            export_menu(ui, &mut commands);
        });

        ui.separator();
        if menu_item(ui, "Print…", "Ctrl+P").clicked() {
            commands.push(Command::Print);
            ui.close();
        }
        if menu_item(ui, "Print current page", "").clicked() {
            commands.push(Command::PrintCurrentPage);
            ui.close();
        }
    });

    // Composition needs no open document: the inputs are files the user picks
    // and the result is a new file, so this is available from an empty window.
    ui.separator();
    if menu_item(ui, "Create PDF from images…", "").clicked() {
        commands.push(Command::ShowImagesToPdfDialog);
        ui.close();
    }
    if menu_item(ui, "Merge PDFs…", "").clicked() {
        commands.push(Command::ShowMergeDialog);
        ui.close();
    }

    ui.separator();
    ui.add_enabled_ui(has_tab, |ui| {
        if menu_item(ui, "Close tab", "Ctrl+W").clicked() {
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

/// The Export submenu. Everything here writes a *new* file rather than
/// updating the open one, which is what separates it from Save.
///
/// The image entries are generated from [`ImageFormat::ALL`] so that adding a
/// format in the domain model adds its menu entries here.
fn export_menu(ui: &mut Ui, commands: &mut Vec<Command>) {
    for format in ImageFormat::ALL {
        if menu_item(ui, &format!("Page as {}…", format.label()), "").clicked() {
            commands.push(Command::ShowExportImageDialog(format));
            ui.close();
        }
    }

    ui.separator();
    for format in ImageFormat::ALL {
        if menu_item(ui, &format!("All pages as {}…", format.label()), "").clicked() {
            commands.push(Command::ShowExportAllPagesDialog(format));
            ui.close();
        }
    }

    ui.separator();
    if menu_item(ui, "Current page as PDF…", "").clicked() {
        commands.push(Command::ShowExportPagesPdfDialog);
        ui.close();
    }
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
                RichText::new("PDF Reader")
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
            menu_button(ui, palette, "Go", |ui| {
                ui.add_enabled_ui(has_doc, |ui| {
                    if menu_item(ui, "First page", "Home").clicked() {
                        commands.push(Command::GoToPage(0));
                        ui.close();
                    }
                    if ui
                        .add_enabled(page > 0, menu_item_button("Previous page", "Page Up"))
                        .clicked()
                    {
                        commands.push(Command::PrevPage);
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            page_count > 0 && page + 1 < page_count,
                            menu_item_button("Next page", "Page Down"),
                        )
                        .clicked()
                    {
                        commands.push(Command::NextPage);
                        ui.close();
                    }
                    if menu_item(ui, "Last page", "End").clicked() {
                        commands.push(Command::GoToPage(u32::MAX));
                        ui.close();
                    }
                });
            });
            menu_button(ui, palette, "View", |ui| {
                ui.add_enabled_ui(has_doc, |ui| {
                    if menu_item(ui, "Zoom in", "Ctrl+=").clicked() {
                        commands.push(Command::ZoomIn);
                        ui.close();
                    }
                    if menu_item(ui, "Zoom out", "Ctrl+-").clicked() {
                        commands.push(Command::ZoomOut);
                        ui.close();
                    }
                    if menu_item(ui, "Actual size", "Ctrl+0").clicked() {
                        commands.push(Command::ZoomReset);
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .radio(matches!(zoom_mode, Some(ZoomMode::FitWidth)), "Fit width")
                        .clicked()
                    {
                        commands.push(Command::SetZoom(ZoomMode::FitWidth));
                        ui.close();
                    }
                    if ui
                        .radio(matches!(zoom_mode, Some(ZoomMode::FitPage)), "Fit page")
                        .clicked()
                    {
                        commands.push(Command::SetZoom(ZoomMode::FitPage));
                        ui.close();
                    }
                    ui.separator();
                    if menu_item(ui, "Rotate clockwise", "").clicked() {
                        commands.push(Command::RotateCw);
                        ui.close();
                    }
                    if menu_item(ui, "Rotate counter-clockwise", "").clicked() {
                        commands.push(Command::RotateCcw);
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .radio(
                            matches!(view_mode, Some(ViewMode::Continuous)),
                            "Continuous reading",
                        )
                        .clicked()
                    {
                        commands.push(Command::SetViewMode(ViewMode::Continuous));
                        ui.close();
                    }
                    if ui
                        .radio(matches!(view_mode, Some(ViewMode::Single)), "Single page")
                        .clicked()
                    {
                        commands.push(Command::SetViewMode(ViewMode::Single));
                        ui.close();
                    }
                });
                ui.separator();
                let label = if state.sidebar_visible {
                    "Hide sidebar"
                } else {
                    "Show sidebar"
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
) -> Vec<Command> {
    let mut commands = Vec::new();
    let tab = state.active();
    let has_doc = tab.is_some_and(|t| t.document.is_some());
    let page_count = tab.map_or(0, Tab::page_count);
    let compact = ui.available_width() < 1040.0;

    egui::Frame::new()
        .fill(palette.panel_bg)
        .inner_margin(egui::Margin::symmetric(10, 7))
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
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Find").color(palette.text_dim).size(11.0));
                        let response = ui.add_sized(
                            Vec2::new(if compact { 112.0 } else { 168.0 }, 30.0),
                            egui::TextEdit::singleline(search_query).hint_text("Search document"),
                        );
                        if response.has_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter))
                        {
                            commands.push(Command::Search(search_query.trim().to_owned()));
                        }
                        if !search_query.is_empty()
                            && icon_button(
                                ui,
                                palette,
                                Icon::Close,
                                "Clear",
                                "Clear search",
                                false,
                                true,
                            )
                        {
                            search_query.clear();
                            commands.push(Command::Search(String::new()));
                        }
                    });
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
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
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
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    // A little vertical rhythm between outline entries.
                    ui.spacing_mut().item_spacing.y = 3.0;
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
pub fn annotation_bar(ui: &mut Ui, palette: &Palette, state: &AppState) -> Vec<Command> {
    let mut commands = Vec::new();
    egui::Frame::new()
        .fill(palette.panel_bg)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(ANNOTATION_BAR_HEIGHT - 4.0);
                ui.label(RichText::new("Annotate").color(palette.text_dim).size(11.0));
                for tool in Tool::ALL {
                    let active = state.tool == tool;
                    let label = RichText::new(tool.label()).size(11.5).color(if active {
                        palette.accent
                    } else {
                        palette.text
                    });
                    let button = egui::Button::new(label)
                        .fill(if active { palette.accent_soft } else { Color32::TRANSPARENT })
                        .corner_radius(4.0);
                    if ui.add(button).clicked() {
                        commands.push(Command::SetTool(tool));
                    }
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
                        if enabled { palette.text } else { palette.text_dim },
                    );
                }
            });
        });
    commands
}

/// Height of one row in the Comments panel.
const COMMENT_ROW_HEIGHT: f32 = 34.0;

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

    if annotations.is_empty() {
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
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            let label = if annotations.len() == 1 {
                "1 annotation".to_string()
            } else {
                format!("{} annotations", annotations.len())
            };
            ui.label(RichText::new(label).color(palette.text_dim).size(11.0));
        });
    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(6, 4))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for annotation in annotations {
                        let is_selected = selected == Some(annotation.id);
                        let response =
                            show_comment_row(ui, palette, annotation, is_selected);
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
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, COMMENT_ROW_HEIGHT), Sense::click());

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
    ui.painter().rect_filled(rect.expand(1.0), 4.0, fill);

    let painter = ui.painter().with_clip_rect(rect);
    let left = rect.min.x + 8.0;
    painter.text(
        egui::pos2(left, rect.min.y + 4.0),
        Align2::LEFT_TOP,
        format!("{} · p.{}", annotation.kind.label(), annotation.id.page + 1),
        FontId::proportional(11.5),
        palette.text,
    );
    let preview = if annotation.display_contents().is_empty() {
        "—".to_string()
    } else {
        truncate(annotation.display_contents(), 30)
    };
    painter.text(
        egui::pos2(left, rect.min.y + 20.0),
        Align2::LEFT_TOP,
        preview,
        FontId::proportional(10.5),
        palette.text_dim,
    );
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
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            let editable = matches!(
                annotation.kind,
                AnnotationKind::FreeText | AnnotationKind::StickyNote
            );
            if editable {
                let key = Id::new(("comment-draft", doc.raw(), annotation.id.page, annotation.id.annot_index));
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
                let submit =
                    response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
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

/// Height of one row in the Forms panel: field name over type and value.
const FORM_ROW_HEIGHT: f32 = 36.0;

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
        .inner_margin(egui::Margin::symmetric(8, 4))
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

    let response = ui.add(
        egui::TextEdit::singleline(&mut draft).desired_width(ui.available_width()),
    );
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
    let id = Id::new(("form-choice", doc.raw(), field.id.page, field.id.annot_index));
    let selection = if current.is_empty() { "—" } else { current.as_str() };

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
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            let remaining = form.remaining_count();
            let summary = if remaining == 0 {
                format!("{} fields · complete", form.fillable_count())
            } else {
                format!("{} fields · {} to fill", form.fillable_count(), remaining)
            };
            ui.label(RichText::new(summary).color(palette.text_dim).size(11.0));
        });
    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(6, 4))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
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
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, FORM_ROW_HEIGHT), Sense::click());

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
    ui.painter().rect_filled(rect.expand(1.0), 4.0, fill);

    let painter = ui.painter().with_clip_rect(rect);
    let left = rect.min.x + 8.0;

    painter.text(
        egui::pos2(left, rect.min.y + 4.0),
        Align2::LEFT_TOP,
        truncate(field.display_name(), 28),
        FontId::proportional(11.5),
        palette.text,
    );

    // Second line: what kind of field it is, and what it currently holds.
    // A blank field shows a dash rather than nothing, so the column does not
    // collapse and look like a rendering bug.
    let detail = if field.value.is_empty() {
        format!("{} · —", field.kind.label())
    } else {
        format!("{} · {}", field.kind.label(), truncate(&field.value.as_text(), 24))
    };
    // Read-only fields are marked, because a user who cannot type into a field
    // needs to be told why rather than guessing the app is broken.
    let detail = if field.read_only {
        format!("{detail} · locked")
    } else {
        detail
    };

    painter.text(
        egui::pos2(left, rect.min.y + 20.0),
        Align2::LEFT_TOP,
        detail,
        FontId::proportional(10.5),
        palette.text_dim,
    );

    response
}

/// The right-hand tools rail, in the spirit of Acrobat Pro DC's tool sidebar.
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
        .inner_margin(egui::Margin::symmetric(3, 2))
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
        .min_size(Vec2::new(76.0, 32.0)),
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
    let response = ui.add(
        egui::Button::new(
            RichText::new(format!("  {label}"))
                .size(11.0)
                .color(palette.text),
        )
        .fill(if active {
            palette.accent_soft
        } else {
            Color32::TRANSPARENT
        })
        .corner_radius(6.0)
        .min_size(Vec2::new(66.0, 28.0)),
    );
    draw_icon(
        ui.painter(),
        response.rect.left_center() + Vec2::new(12.0, 0.0),
        icon,
        if active {
            palette.accent
        } else {
            palette.text_dim
        },
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
        ui.add_space(10.0);
        ui.label(RichText::new(title).strong().color(palette.text).size(12.0));
        ui.add_space(4.0);
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
        ThemeId::Acrobat => "Acrobat",
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
    }
}

#[cfg(test)]
mod tests {
    use super::status_bar;
    use crate::theme::Theme;
    use egui::{Context, RawInput, Rect};
    use pdfreader_core::{
        Command, Document, DocumentId, Effect, Outline, PageGeometry, Rotation, Store, ThemeId,
    };
    use std::path::PathBuf;

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
