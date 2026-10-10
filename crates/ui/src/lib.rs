//! egui widgets and theming for the PDF reader shell.
//!
//! This crate is presentation only: it draws the chrome and reports what the
//! user did as [`pdfreader_core::Command`]s. The application crate owns the
//! panels, the canvas and the engine, so it can inject live content (rendered
//! pages, thumbnails) into the layout.

pub mod chrome;
pub mod theme;
pub mod window;

pub use chrome::{
    annotation_properties_window,
    ANNOTATION_BAR_HEIGHT, FORM_FONTS, MENUBAR_HEIGHT, ROW_PAD_X, ROW_PAD_Y, SIDEBAR_GAP,
    SIDEBAR_PAD_X, SIDEBAR_PAD_Y, SIDEBAR_TABS, STATUSBAR_HEIGHT, SearchFieldOutput, TABBAR_HEIGHT,
    TOOLBAR_HEIGHT, TOOLS_RAIL_WIDTH, TextStyle, annotation_bar, available_sidebar_tabs,
    comments_panel, forms_panel, menu_bar, outline_tree, search_field, sidebar_list_margin,
    sidebar_margin, sidebar_note_margin, sidebar_tabs, status_bar, tab_bar, toolbar, tools_rail,
};
pub use theme::{Palette, Theme, apply as apply_theme};
pub use window::{
    BUTTON_HEIGHT, CARD_MARGIN, DIALOG_MIN_WIDTH, FOOTER_GAP, WINDOW_MARGIN, card_frame,
    card_radius, chip_frame, chip_radius, dialog_window, disabled_button, divider, field_frame,
    heading, hint, image_frame, primary_button, secondary_button, section, toast_frame,
    window_frame, window_radius,
};
