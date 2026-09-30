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

    /// Ask to close a tab, prompting first when it has unsaved changes.
    RequestCloseTab(TabId),

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

    /// Write a value into a form field.
    ///
    /// The reducer applies the value optimistically so the UI updates on the
    /// same frame, and emits an effect for the engine to write it into the
    /// open document. A failed write is logged by the shell; the in-memory
    /// document is the authority when saving.
    SetFormFieldValue {
        /// Which widget to change.
        id: crate::form::FieldId,
        /// The new value.
        value: crate::form::FieldValue,
    },

    /// Save the active document to the path it was opened from.
    SaveDocument,

    /// Ask the shell for a destination path, then save there.
    ///
    /// The dialog is shell work, so this command only flips a flag; the shell
    /// dispatches `SaveDocumentTo` with the chosen path.
    SaveDocumentAs,

    /// Save the active document to an explicit path.
    SaveDocumentTo(PathBuf),

    /// The engine finished writing a document; clear its dirty flag.
    DocumentSaved {
        /// Document that was written.
        doc: crate::document::DocumentId,
    },

    /// Flatten the active document: bake field values and annotations into
    /// page content. Marks the document dirty; saving afterwards is expected.
    FlattenDocument,

    /// Close a tab unconditionally, after the user confirmed discarding.
    ConfirmCloseTab(TabId),

    /// Pick the active annotation tool.
    SetTool(crate::annotation::Tool),

    /// The engine finished listing a document's annotations.
    AnnotationsLoaded {
        /// Document the annotations belong to.
        doc: crate::document::DocumentId,
        /// Every non-widget annotation, in page order.
        annotations: Vec<crate::annotation::AnnotationInfo>,
        /// The annotation that was just created, when this load follows an
        /// add — used to resolve the undo entry's positional id.
        created: Option<crate::annotation::AnnotationId>,
    },

    /// Select an annotation, or clear the selection with `None`.
    SelectAnnotation(Option<crate::annotation::AnnotationId>),

    /// Create an annotation with the current tool.
    ///
    /// The reducer marks the document dirty and emits an effect; the engine
    /// answers with a fresh [`Command::AnnotationsLoaded`], which is how the
    /// new annotation's id becomes known.
    AddAnnotation {
        /// Page to draw on.
        page: u32,
        /// What to create and where.
        new: crate::annotation::NewAnnotation,
    },

    /// Delete an annotation.
    DeleteAnnotation(crate::annotation::AnnotationId),

    /// Replace the text contents of an annotation.
    SetAnnotationContents {
        /// Which annotation.
        id: crate::annotation::AnnotationId,
        /// The new text.
        contents: String,
    },

    /// Undo the last annotation operation on the active document.
    Undo,

    /// An annotation operation failed on the engine thread.
    ///
    /// If the list was never read, this un-sticks the Comments panel from its
    /// "reading" state; an existing list is left alone.
    AnnotationsLoadFailed {
        /// Document the failed operation belonged to.
        doc: crate::document::DocumentId,
    },
}

/// Work the shell must perform after a state transition.
///
/// Not `Eq`: annotation effects carry rectangles built from `f32`, which has
/// no total ordering.
#[derive(Clone, PartialEq, Debug)]
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

    /// Write a form value into the open document on the engine thread.
    SetField {
        /// Document holding the field.
        doc: crate::document::DocumentId,
        /// Which widget to change.
        id: crate::form::FieldId,
        /// The new value.
        value: crate::form::FieldValue,
    },

    /// Serialise a document and write it to `path`.
    ///
    /// `flatten` bakes annotations and field values into page content first.
    /// The path may differ from the one the document was opened from (Save As);
    /// the shell writes bytes itself so an in-place save can be atomic.
    SaveDocument {
        /// Document to write.
        doc: crate::document::DocumentId,
        /// Destination file.
        path: PathBuf,
        /// Whether to flatten before serialising.
        flatten: bool,
    },

    /// Show the unsaved-changes prompt for a tab.
    ///
    /// Emitted instead of closing so the user can cancel; the shell must not
    /// close the tab until it dispatches `ConfirmCloseTab`.
    ConfirmClose {
        /// Tab the user asked to close.
        tab: TabId,
    },

    /// Scroll a tab so the given PDF-space point sits at the viewport centre.
    ///
    /// Used for annotation jumps: clicking a comment must land on the exact
    /// noted area, not just the top of its page.
    ScrollToPoint {
        /// Tab to scroll.
        tab: TabId,
        /// Page the point is on.
        page: u32,
        /// PDF-space point to centre on.
        point: (f32, f32),
    },

    /// Enumerate a document's annotations on the engine thread.
    LoadAnnotations {
        /// Document to read.
        doc: crate::document::DocumentId,
    },

    /// Create an annotation on the engine thread.
    AddAnnotation {
        /// Document to edit.
        doc: crate::document::DocumentId,
        /// Page to draw on.
        page: u32,
        /// What to create and where.
        new: crate::annotation::NewAnnotation,
    },

    /// Delete an annotation on the engine thread.
    DeleteAnnotation {
        /// Document to edit.
        doc: crate::document::DocumentId,
        /// Which annotation.
        id: crate::annotation::AnnotationId,
    },

    /// Rewrite an annotation's contents on the engine thread.
    SetAnnotationContents {
        /// Document to edit.
        doc: crate::document::DocumentId,
        /// Which annotation.
        id: crate::annotation::AnnotationId,
        /// The new text.
        contents: String,
    },
}
