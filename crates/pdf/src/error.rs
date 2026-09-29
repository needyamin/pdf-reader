//! Engine error taxonomy.

use std::path::PathBuf;
use thiserror::Error;

/// Everything that can go wrong inside the PDF engine.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum EngineError {
    /// The PDFium shared library could not be located or loaded.
    #[error("could not load the PDFium library: {0}")]
    Library(String),

    /// The file does not exist or cannot be read.
    #[error("cannot read {path}: {reason}")]
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying reason.
        reason: String,
    },

    /// The document is encrypted and no password was supplied.
    #[error("password required")]
    PasswordRequired,

    /// The password supplied was rejected.
    #[error("incorrect password")]
    BadPassword,

    /// The file is not a PDF, or is damaged beyond repair.
    ///
    /// The message is written for the user, not for a log.
    #[error("{0}")]
    Malformed(String),

    /// A page index outside the document was requested.
    #[error("page {index} is out of range (document has {count} pages)")]
    PageOutOfRange {
        /// Requested index.
        index: u32,
        /// Actual page count.
        count: u32,
    },

    /// The requested document is not open in this engine.
    #[error("document {0} is not open")]
    NotOpen(u64),

    /// PDFium itself rejected the operation.
    #[error("pdfium error: {0}")]
    Pdfium(String),
}
