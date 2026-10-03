//! egui widgets and theming for the PDF reader shell.
//!
//! This crate is presentation only: it draws the chrome and reports what the
//! user did as [`pdfreader_core::Command`]s. The application crate owns the
//! panels, the canvas and the engine, so it can inject live content (rendered
//! pages, thumbnails) into the layout.

pub mod chrome;
pub mod theme;

pub use chrome::{
    ANNOTATION_BAR_HEIGHT, MENUBAR_HEIGHT, SIDEBAR_TABS, STATUSBAR_HEIGHT, SearchFieldOutput,
    TABBAR_HEIGHT, TOOLBAR_HEIGHT, TOOLS_RAIL_WIDTH, annotation_bar, available_sidebar_tabs,
    comments_panel, forms_panel, menu_bar, outline_tree, search_field, sidebar_tabs, status_bar,
    tab_bar, toolbar, tools_rail,
};
pub use theme::{Palette, Theme, apply as apply_theme};
