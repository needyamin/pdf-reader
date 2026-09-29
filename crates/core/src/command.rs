//! Every state change in the application goes through a [`Command`].
//!
//! The UI never mutates state directly: it dispatches a command, the reducer
//! produces a new state plus a list of [`Effect`]s describing side effects the
//! shell must perform (open a file, request tiles, persist the session).
//!
//! This is what makes session restore, undo and testing cheap — the reducer is
//! a pure function of `(state, command)`.

use std::path::PathBuf;

use crate::document::Document;
use crate::view::{ThemeId, ViewMode, ZoomMode};

/// Identity of an open tab.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TabId(u64);

impl TabId {
    /// Wrap a raw counter value into an id.
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Return the raw counter value.
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Which panel the sidebar is showing.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum SidebarTab {
    /// Page thumbnails.
    #[default]
    Thumbnails,
    /// User bookmarks for this file.
    Bookmarks,
    /// The document outline.
    Outline,
    /// Full-text search results.
    Search,
    /// Annotation comments.
    Comments,
    /// AcroForm fields.
    Forms,
}

/// A user or system intent. Handling one produces new state and effects.
#[derive(Clone, PartialEq, Debug)]
pub enum Command {
    /// Ask the shell to show an open-file dialog, then open whatever is picked.
    ///
    /// Separate from `OpenPath` because the shell must do async work (a native
    /// modal dialog on another thread) before the real `OpenPath` is known.
    ShowOpenDialog,

    /// Ask the shell to open a file in a new tab.
    OpenPath(PathBuf),

    /// The engine finished opening a document.
    DocumentOpened {
        /// Tab the document belongs to.
        tab: TabId,
        /// The parsed document.
        document: Document,
    },

    /// The engine could not open a document.
    DocumentFailed {
        /// Tab that was loading.
        tab: TabId,
        /// Human-readable failure reason.
        reason: String,
    },

    /// Close a tab and release its document.
    CloseTab(TabId),

    /// Make a tab the active one.
    ActivateTab(TabId),

    /// Switch between single-page and continuous view.
    SetViewMode(ViewMode),

    /// Set the zoom intent (explicit factor, fit width or fit page).
    SetZoom(ZoomMode),

    /// The shell resolved a viewport-dependent zoom mode to a concrete factor.
    ///
    /// `FitWidth`/`FitPage` cannot be evaluated in the reducer because they
    /// depend on the canvas size, which only the shell knows. The shell computes
    /// the factor and feeds it back here so `view.zoom` stays the single source
    /// of truth for the toolbar, status bar and renderer.
    ResolvedZoom(f32),

    /// Zoom in one step.
    ZoomIn,

    /// Zoom out one step.
    ZoomOut,

    /// Return to 100% zoom.
    ZoomReset,

    /// The viewport moved.
    ///
    /// Carries both the offset and the page under the viewport centre because
    /// the two are reported together by the scroll handler, and splitting them
    /// would mean emitting two commands per pointer move.
    ViewportChanged {
        /// New document-space horizontal offset, in points.
        scroll_x: f32,
        /// New document-space vertical offset, in points.
        scroll_y: f32,
        /// Zero-based page under the viewport centre.
        current_page: u32,
    },

    /// Move the viewport by a small keyboard-navigation increment.
    ScrollBy {
        /// Horizontal delta in content coordinates.
        dx: f32,
        /// Vertical delta in content coordinates.
        dy: f32,
    },

    /// Rotate every page clockwise.
    RotateCw,

    /// Rotate every page counter-clockwise.
    RotateCcw,

    /// Navigate to a zero-based page index.
    GoToPage(u32),

    /// Go to the next page.
    NextPage,

    /// Go to the previous page.
    PrevPage,

    /// Show or hide the sidebar.
    ToggleSidebar,

    /// Select which sidebar panel to show.
    SetSidebarTab(SidebarTab),

    /// Switch colour theme.
    SetTheme(ThemeId),

    /// Enter or leave fullscreen.
    ToggleFullscreen,

    /// Clear an in-flight document load after the user dismisses the error.
    DismissError,

    /// Search the active document for a query. The shell performs the
    /// asynchronous engine work; the reducer keeps this command side-effect
    /// free so it remains easy to test.
    Search(String),

    /// The engine finished reading a document's interactive form.
    ///
    /// Keyed by document id rather than tab id because the engine answers long
    /// after the request, by which time the tab may have been closed.
    FormFieldsLoaded {
        /// Document the form belongs to.
        doc: crate::document::DocumentId,
        /// The form, empty when the document has no fields.
        form: crate::form::FormInfo,
    },

    /// Select a form field, or clear the selection with `None`.
    SelectFormField(Option<crate::form::FieldId>),
}

/// Work the shell must perform after a state transition.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Effect {
    /// Load a document from disk on the engine thread.
    OpenDocument {
        /// Destination tab.
        tab: TabId,
        /// File to open.
        path: PathBuf,
    },

    /// Release engine resources for a document.
    CloseDocument {
        /// Document being released.
        doc: crate::document::DocumentId,
    },

    /// The viewport changed, so outstanding tile requests are stale.
    InvalidateTiles,

    /// Scroll so that the given page is at the top of the viewport.
    ///
    /// Emitted rather than applied because computing a page offset needs the
    /// page layout, which lives in the render layer, not in core.
    ScrollToPage {
        /// Tab to scroll.
        tab: TabId,
        /// Zero-based page index.
        page: u32,
    },

    /// Durable state changed and should be written to disk.
    PersistSession,
}
