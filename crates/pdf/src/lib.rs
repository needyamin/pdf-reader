//! PDF engine abstraction and its PDFium backend.
//!
//! The `PdfEngine` trait exists so the rest of the application never talks to
//! PDFium directly. That keeps the render and UI layers portable if the engine
//! is ever swapped, and it keeps every piece of FFI behind one audited seam.

pub mod annot;
pub mod engine;
pub mod error;
pub mod export;
pub mod form;
pub mod formmark;
pub mod geometry;
mod pageobj;
pub mod text;

pub use engine::{DocumentHandle, FieldWrite, PdfEngine, TilePixels, TileRequest};
pub use error::EngineError;
pub use export::{JobProgress, MAX_EXPORT_MEGAPIXELS};
// The annotation and form domain types live in `core` so the UI can use them
// without PDFium; this crate only converts PDFium's objects into them.
pub use geometry::Rect;
pub use pdfreader_core::{AnnotationId, AnnotationInfo, AnnotationKind, NewAnnotation};
pub use text::{CharBox, CharRange, Line, Span, TextPage};

/// Result type used throughout this crate.
pub type Result<T> = std::result::Result<T, EngineError>;

/// Metadata produced when a document is opened successfully.
#[derive(Clone, PartialEq, Debug)]
pub struct DocumentInfo {
    /// Page sizes in order.
    pub pages: Vec<PageGeometry>,
    /// Bookmarks tree.
    pub outline: Outline,
    /// Whether a password was required to open the file.
    pub encrypted: bool,
    /// Whether the document's permissions allow its content to be changed.
    ///
    /// Deliberately *not* the same as `encrypted`. A PDF may carry a security
    /// handler purely to restrict permissions, with no password needed to
    /// open it — which is common in files from publishers, banks and print
    /// workflows. Those open without a passphrase and allow editing, so
    /// gating on `encrypted` would block the content editor on documents that
    /// are perfectly writable.
    pub can_edit_content: bool,
}

use pdfreader_core::{Outline, PageGeometry};

impl DocumentInfo {
    /// Number of pages.
    pub fn page_count(&self) -> u32 {
        self.pages.len() as u32
    }
}
