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
use crate::view::{ThemeId, ViewMode, ViewState, ZoomMode};

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

/// One page of the Help user guide.
///
/// The shell draws the page. The command only names which one to open, so a
/// Help menu entry and the guide's own list stay the same set of topics.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GuideTopic {
    /// Open a file, move around, and use the sidebar.
    Start,
    /// Zoom, page display, rotation, and full screen.
    Reading,
    /// Highlight, notes, shapes, and leaving adjustment.
    Annotate,
    /// Retype and move the PDF's own text.
    Editing,
    /// Offline lookup, speak, and Google Translate.
    Language,
    /// Save, print, and export.
    Share,
}

impl GuideTopic {
    /// Every topic, in Help-menu order.
    pub const ALL: [GuideTopic; 6] = [
        GuideTopic::Start,
        GuideTopic::Reading,
        GuideTopic::Annotate,
        GuideTopic::Editing,
        GuideTopic::Language,
        GuideTopic::Share,
    ];

    /// The label used in the Help menu and in the guide's own list.
    pub const fn label(self) -> &'static str {
        match self {
            GuideTopic::Start => "Getting Started",
            GuideTopic::Reading => "Reading",
            GuideTopic::Annotate => "Annotating",
            GuideTopic::Editing => "Editing Text",
            GuideTopic::Language => "Translate & Speak",
            GuideTopic::Share => "Save, Print & Export",
        }
    }
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

    /// Reopen the tabs from the previous session, at the state they were left.
    ///
    /// One command rather than a run of `OpenPath`s so the active tab is
    /// restored in the same step, and the shell can open every file it names.
    RestoreTabs {
        /// Tabs to reopen, in display order: the file and where it was left.
        tabs: Vec<(PathBuf, ViewState)>,
        /// Index into `tabs` of the tab that was active, if any.
        active: Option<usize>,
    },

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

    /// Select an annotation without scrolling to it.
    ///
    /// The canvas uses this when the user clicks an annotation that is
    /// already on screen: centring the view on every click would yank the
    /// page out from under the pointer, and a drag that began at the click
    /// would compute its delta against a moving viewport.
    SelectAnnotationInPlace(Option<crate::annotation::AnnotationId>),

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

    /// Move / resize an annotation to a new rectangle.
    ///
    /// `rect` is in unrotated PDF page space, exactly like
    /// [`crate::annotation::AnnotationInfo::rect`]. The reducer updates the
    /// listed annotation immediately (the engine call only repaints) and
    /// records the geometry change for undo.
    UpdateAnnotationBounds {
        /// Which annotation.
        id: crate::annotation::AnnotationId,
        /// The new bounds, in PDF page space.
        rect: crate::rect::Rect,
    },

    /// Recolour an annotation. RGBA bytes; the alpha byte is the rendered
    /// opacity, so one command serves both the colour swatches and the
    /// opacity slider.
    SetAnnotationColor {
        /// Which annotation.
        id: crate::annotation::AnnotationId,
        /// The new colour.
        color: [u8; 4],
    },

    /// Change an annotation's stroke weight, in PDF points.
    ///
    /// Applies to line markups and rectangles. The reducer grows a line
    /// markup's box when the stroke would otherwise be clipped, and records
    /// both the weight and the box so undo restores the mark that was there.
    SetAnnotationThickness {
        /// Which annotation.
        id: crate::annotation::AnnotationId,
        /// The new stroke weight, in PDF points.
        thickness: f32,
    },

    /// Open or close the floating properties window for the selected
    /// annotation, without changing the selection itself.
    ///
    /// The window is off by default so selecting an annotation shows only its
    /// move/resize handles; this command lets the toolbar toggle it on demand.
    SetPropertiesPanel {
        /// Whether the window should be open.
        open: bool,
    },

    /// Enter or leave content-edit mode.
    ///
    /// Entering is refused for a document that has no pages or is encrypted,
    /// and forces the tool back to Select so no half-drawn mark is armed while
    /// the pointer is editing the page itself.
    SetEditingContent {
        /// Whether editing should be on.
        on: bool,
    },

    /// Select a page-content object, or clear the selection with `None`.
    SelectPageObject(Option<crate::pageobj::PageObjectId>),

    /// Open the inline editor on a text object that is already selected.
    ///
    /// The shell owns the editor widget, so the reducer has nothing to apply.
    /// A click only selects; this is the second click, the Edit button, or a
    /// double-click asking to type.
    BeginObjectTextEdit {
        /// The text object to edit.
        info: crate::pageobj::PageObjectInfo,
    },

    /// Add a copy of a text object, offset slightly so it does not sit on the
    /// original. Images are not duplicated: their pixels are not kept.
    DuplicatePageObject {
        /// The object to copy.
        info: crate::pageobj::PageObjectInfo,
    },

    /// Move or resize a page-content object.
    ///
    /// `previous` is carried rather than looked up: the object list lives in
    /// the engine, not in the store, so the reducer has no other way to know
    /// what the bounds were before the edit — and undo needs both.
    SetPageObjectBounds {
        /// Which object.
        id: crate::pageobj::PageObjectId,
        /// The bounds before the edit; undo restores this.
        previous: crate::rect::Rect,
        /// The bounds after the edit.
        rect: crate::rect::Rect,
    },

    /// Delete a page-content object.
    ///
    /// Carries the whole record so a deleted **text** object can be rebuilt on
    /// undo. Deleting an image is not undoable and pushes no history — see
    /// [`crate::store::UndoAction`].
    DeletePageObject {
        /// What to remove.
        info: crate::pageobj::PageObjectInfo,
    },

    /// Move a page-content object within the page's paint order — the panel's
    /// "bring to front" / "send to back". The record (not the id) addresses the
    /// object, because removing it renumbers the page; undo restores the exact
    /// index the object occupied before.
    RestackPageObject {
        /// What to move.
        info: crate::pageobj::PageObjectInfo,
        /// Where it lands in the paint order.
        target: crate::pageobj::RestackTarget,
    },

    /// Replace the text a page-content text object holds.
    SetPageObjectText {
        /// Which object.
        id: crate::pageobj::PageObjectId,
        /// The text before the edit; undo restores this.
        previous: String,
        /// The text after the edit.
        text: String,
        /// The original text area. The engine tries to fit the edited string
        /// inside it and lets it overflow when that is not possible at a
        /// legible size — an edit is never refused.
        layout_bounds: crate::rect::Rect,
        /// The effective size the original line was set at. Undo restores this
        /// size; longer text may be shrunk to keep the page layout intact.
        base_font_size: f32,
        /// The styling the object had before the edit; undo restores this.
        previous_format: crate::pageobj::TextFormat,
        /// The styling to apply to the result.
        format: crate::pageobj::TextFormat,
    },

    /// Restyle a page-content text object without changing its string.
    ///
    /// Bold, colour and size are synthetic (PDFium has no style property on a
    /// text run), so this is what the formatting panel toggles. The reducer
    /// emits the effect and records an undo entry carrying both the old and new
    /// format, so a toggle can be undone.
    SetPageObjectFormat {
        /// Which object.
        id: crate::pageobj::PageObjectId,
        /// The object's text area, kept for undo symmetry; a format change
        /// applies the size as asked and does not fit it into this area.
        layout_bounds: crate::rect::Rect,
        /// The effective size the line was set at; the shear scales with it.
        base_font_size: f32,
        /// The styling the object had before; undo restores this.
        previous_format: crate::pageobj::TextFormat,
        /// The styling to apply.
        format: crate::pageobj::TextFormat,
    },

    /// Ask the user for a picture to put in an image object.
    ///
    /// Shell-only, like `ShowOpenDialog`: the picker is native and the answer
    /// comes back as [`Command::ReplacePageObjectImage`].
    PickReplacementImage(crate::pageobj::PageObjectId),

    /// Swap the pixels of an image object for a file on disk.
    ///
    /// Not undoable: the previous pixels would have to be kept, and a page of
    /// images would cost more memory than the undo stack is worth.
    ReplacePageObjectImage {
        /// Which object.
        id: crate::pageobj::PageObjectId,
        /// Picture to put in it.
        path: PathBuf,
    },

    /// Add a new text object to a page.
    InsertPageObjectText {
        /// Page to add it to.
        page: u32,
        /// Where the click landed, in PDF page space.
        point: (f32, f32),
        /// What it says.
        text: String,
    },

    /// The engine finished listing a page's objects.
    ///
    /// The list itself is cached by the shell; the reducer only needs it to
    /// resolve the positional ids recorded by undo and to drop a selection that
    /// no longer exists.
    PageObjectsLoaded {
        /// Document the page belongs to.
        doc: crate::document::DocumentId,
        /// Which page.
        page: u32,
        /// Every editable object on it, in object order.
        objects: Vec<crate::pageobj::PageObjectInfo>,
    },

    /// The engine created a page object, so its id is now known.
    ///
    /// The positional analogue of [`Command::AnnotationsLoaded`]'s `created`:
    /// an insert records its undo entry before the index exists.
    PageObjectInserted {
        /// Document it was added to.
        doc: crate::document::DocumentId,
        /// Which page.
        page: u32,
        /// Where it landed.
        id: crate::pageobj::PageObjectId,
        /// The refreshed object list.
        objects: Vec<crate::pageobj::PageObjectInfo>,
    },

    /// Undo the last annotation operation on the active document.
    Undo,

    /// Re-apply the annotation operation that was just undone.
    ///
    /// Only meaningful straight after an undo: any new edit clears the redo
    /// history, exactly like every other editor.
    Redo,

    /// An annotation operation failed on the engine thread.
    ///
    /// If the list was never read, this un-sticks the Comments panel from its
    /// "reading" state; an existing list is left alone.
    AnnotationsLoadFailed {
        /// Document the failed operation belonged to.
        doc: crate::document::DocumentId,
    },

    /// Open the export window, set up for one kind of export.
    ///
    /// One command for all five tasks: the window is where the options live,
    /// and the reducer has nothing to decide about a window that is only
    /// collecting settings. The shell intercepts it, exactly like
    /// `ShowOpenDialog`.
    ShowExportWindow(crate::export::ExportTask),

    /// Write the active document's current page out as an image file.
    ///
    /// The page and rotation come from the reducer rather than the caller: the
    /// window is open for as long as the user is choosing options, and the page
    /// it exports must be the one they were looking at, not whichever page the
    /// view drifted to while the window was up. The format and resolution are
    /// the user's choices, so they are carried through.
    ExportPageImage {
        /// Destination file, chosen by the user.
        path: PathBuf,
        /// Format to write.
        format: crate::export::ImageFormat,
        /// Device pixels per PDF point.
        scale: f32,
    },

    /// Write every page of the active document out as image files.
    ExportAllPages {
        /// Destination directory, chosen by the user.
        dir: PathBuf,
        /// Format to write.
        format: crate::export::ImageFormat,
        /// Device pixels per PDF point.
        scale: f32,
    },

    /// Write a page range of the active document out as a standalone PDF.
    ExportPagesPdf {
        /// Destination file, chosen by the user.
        path: PathBuf,
        /// Pages to include.
        range: crate::export::PageRange,
    },

    /// Print the active document. The operating system's dialog chooses the
    /// printer and, if the user wants one, the page range.
    Print,

    /// Print only the page currently in view.
    PrintCurrentPage,

    /// Build a PDF with one page per image.
    ImagesToPdf {
        /// Images to place, one page each.
        images: Vec<PathBuf>,
        /// Destination file, chosen by the user.
        output: PathBuf,
        /// How each page is sized.
        size: crate::export::ImagePageSize,
    },

    /// Concatenate documents into one, in the order given.
    MergePdfs {
        /// Documents to merge.
        sources: Vec<PathBuf>,
        /// Destination file, chosen by the user.
        output: PathBuf,
    },

    /// Turn the offline word-lookup popup on or off. App-local preference,
    /// intercepted by the shell (like `ShowExportWindow`): the reducer has no
    /// say in a feature that lives entirely in the shell.
    ToggleLookup,

    /// Turn the popup's "Speak" (text-to-speech) button on or off.
    ToggleSpeak,

    /// Turn reading the selection aloud automatically on or off.
    ToggleAutoSpeak,

    /// Set the font family used for form field text (empty = default).
    SetFormFont(String),

    /// Set the point size used for form field text (0 = fit the field).
    SetFormFontSize(f32),

    /// Set the RGB ink used for form field text.
    SetFormFontColor(u8, u8, u8),

    /// Draw form field text heavier, or stop doing so.
    ///
    /// No bold face is bundled and `egui` has no weight axis, so this is a
    /// synthetic embolden: the glyphs are stamped twice a hair apart. It is
    /// applied identically to the field being edited and to the committed
    /// value, so text does not change weight when a field is finished.
    SetFormFontBold(bool),

    /// Turn the popup's "Google Translate" link on or off.
    ToggleTranslate,

    /// Set the language the Google Translate button links to (an ISO code such
    /// as `bn` or `zh-CN`). Invalid codes fall back to Bangla in the link
    /// builder; the shell stores the raw value and validates on use.
    SetLensTarget(String),

    /// Open the full Settings window. Shell-only, like the toggles above.
    ShowSettings,

    /// Open the Settings window on its About page (Help → About). Shell-only.
    ShowAbout,

    /// Open the keyboard-shortcuts reference (Help → Keyboard Shortcuts).
    ShowShortcuts,

    /// Open the user guide on one topic (Help → Getting Started, and the
    /// other guide entries). Shell-only.
    ShowGuide(GuideTopic),

    /// Move focus to the find box. Shell-only: focus is a view concern, so the
    /// reducer has nothing to say about it.
    FocusSearch,
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

    /// Bring a form field into view, moving the view as little as possible.
    ///
    /// Distinct from [`Self::ScrollToPage`]: selecting a field is what the user
    /// does immediately before typing, so a field that is already visible must
    /// not move at all. Only the field's own rectangle is carried — the shell
    /// turns it into a scroll delta, because that needs the page layout.
    RevealField {
        /// Tab the field belongs to.
        tab: TabId,
        /// Zero-based page the field sits on.
        page: u32,
        /// Field rectangle in unrotated PDF page space.
        rect: crate::rect::Rect,
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

    /// Move / resize an annotation on the engine thread.
    UpdateAnnotationBounds {
        /// Document to edit.
        doc: crate::document::DocumentId,
        /// Which annotation.
        id: crate::annotation::AnnotationId,
        /// The new bounds, in PDF page space.
        rect: crate::rect::Rect,
    },

    /// Recolour an annotation on the engine thread.
    SetAnnotationColor {
        /// Document to edit.
        doc: crate::document::DocumentId,
        /// Which annotation.
        id: crate::annotation::AnnotationId,
        /// RGBA bytes; the alpha byte is the rendered opacity.
        color: [u8; 4],
    },

    /// Change an annotation's stroke weight on the engine thread.
    ///
    /// `rect` is the box the stroke needs. For a line markup that is the box
    /// grown to fit `thickness`; for a rectangle it is the box unchanged.
    SetAnnotationThickness {
        /// Document to edit.
        doc: crate::document::DocumentId,
        /// Which annotation.
        id: crate::annotation::AnnotationId,
        /// The new stroke weight, in PDF points.
        thickness: f32,
        /// The bounds the stroke should occupy, in PDF page space.
        rect: crate::rect::Rect,
    },

    /// Move or resize a page-content object on the engine thread.
    SetPageObjectBounds {
        /// Document to edit.
        doc: crate::document::DocumentId,
        /// Which object.
        id: crate::pageobj::PageObjectId,
        /// The new bounds, in PDF page space.
        rect: crate::rect::Rect,
    },

    /// Delete a page-content object on the engine thread.
    DeletePageObject {
        /// Document to edit.
        doc: crate::document::DocumentId,
        /// Which object.
        id: crate::pageobj::PageObjectId,
    },

    /// Delete the page-content object that matches a record.
    ///
    /// Redo of a delete cannot trust the positional id it recorded: undoing
    /// re-created the object, which renumbered the page. The engine re-matches
    /// on the parts of the record a renumbering cannot change.
    DeletePageObjectMatching {
        /// Document to edit.
        doc: crate::document::DocumentId,
        /// The record to match.
        info: crate::pageobj::PageObjectInfo,
    },

    /// Move a page-content object within the page's paint order.
    ///
    /// The record addresses the object: removing it to re-insert renumbers the
    /// page, so undo's restore-to-index re-matches the same way.
    RestackPageObject {
        /// Document to edit.
        doc: crate::document::DocumentId,
        /// The record to match.
        info: crate::pageobj::PageObjectInfo,
        /// Where the object lands in the paint order.
        target: crate::pageobj::RestackTarget,
    },

    /// Rewrite a page-content text object on the engine thread.
    SetPageObjectText {
        /// Document to edit.
        doc: crate::document::DocumentId,
        /// Which object.
        id: crate::pageobj::PageObjectId,
        /// The text undo writes back.
        previous: String,
        /// The new text.
        text: String,
        /// The run's ink box when the edit was made. The font and size are kept.
        layout_bounds: crate::rect::Rect,
        /// The original point size. A retype keeps it unless the user set another.
        base_font_size: f32,
        /// The styling the object had before the edit; restored if the new text will not fit.
        previous_format: crate::pageobj::TextFormat,
        /// The styling to apply to the result.
        format: crate::pageobj::TextFormat,
    },

    /// Restyle a page-content text object on the engine thread.
    SetPageObjectFormat {
        /// Document to edit.
        doc: crate::document::DocumentId,
        /// Which object.
        id: crate::pageobj::PageObjectId,
        /// The original text area; a style that overflows it is refused.
        layout_bounds: crate::rect::Rect,
        /// The effective size the line was set at; the shear scales with it.
        base_font_size: f32,
        /// The styling the object had before; undo restores this.
        previous_format: crate::pageobj::TextFormat,
        /// The styling to apply.
        format: crate::pageobj::TextFormat,
    },

    /// Swap an image object's pixels for a file on disk.
    ReplacePageObjectImage {
        /// Document to edit.
        doc: crate::document::DocumentId,
        /// Which object.
        id: crate::pageobj::PageObjectId,
        /// Picture to put in it.
        path: PathBuf,
    },

    /// Add a new text object to a page on the engine thread.
    InsertPageObjectText {
        /// Document to edit.
        doc: crate::document::DocumentId,
        /// Page to add it to.
        page: u32,
        /// Where the click landed, in PDF page space.
        point: (f32, f32),
        /// What it says.
        text: String,
        /// Size to draw it at, in points.
        font_size: f32,
    },

    /// Rasterize one page and write it out as an image file.
    ExportPageImage {
        /// Document to read.
        doc: crate::document::DocumentId,
        /// Zero-based page index.
        page: u32,
        /// Page rotation to bake into the export.
        rotation: crate::document::Rotation,
        /// Device pixels per PDF point.
        scale: f32,
        /// Format to write.
        format: crate::export::ImageFormat,
        /// Destination file.
        path: PathBuf,
    },

    /// Rasterize every page into `dir`, one image file per page.
    ExportAllPages {
        /// Document to read.
        doc: crate::document::DocumentId,
        /// Directory to write into.
        dir: PathBuf,
        /// Name stem for the files, e.g. `report` → `report-0001.png`.
        stem: String,
        /// Page rotation to bake into the exports.
        rotation: crate::document::Rotation,
        /// Device pixels per PDF point.
        scale: f32,
        /// Format to write.
        format: crate::export::ImageFormat,
    },

    /// Serialise a page range of a document as a standalone PDF.
    ExportPages {
        /// Document to read.
        doc: crate::document::DocumentId,
        /// Pages to include; `None` means the whole document.
        range: Option<crate::export::PageRange>,
        /// Destination file.
        path: PathBuf,
    },

    /// Write a page range to a temporary PDF and hand it to the operating
    /// system's print handler.
    ///
    /// The shell invents the spool path, because it owns the temp-file
    /// lifecycle: the file cannot be deleted when the job finishes, since the
    /// handler reads it afterwards.
    Print {
        /// Document to print.
        doc: crate::document::DocumentId,
        /// Pages to print; `None` means the whole document.
        range: Option<crate::export::PageRange>,
    },

    /// Build a PDF with one page per image.
    ImagesToPdf {
        /// Images to place, one page each.
        images: Vec<PathBuf>,
        /// Destination file.
        output: PathBuf,
        /// How each page is sized.
        size: crate::export::ImagePageSize,
    },

    /// Concatenate documents into one, in the order given.
    MergePdfs {
        /// Documents to merge.
        sources: Vec<PathBuf>,
        /// Destination file.
        output: PathBuf,
    },
}
