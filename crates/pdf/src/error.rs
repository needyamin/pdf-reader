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

    /// The file could not be written.
    ///
    /// Kept separate from [`EngineError::Io`] so a failed export or print spool
    /// does not claim it could not *read* the very file it was creating.
    #[error("cannot write {path}: {reason}")]
    Write {
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

    /// The job was cancelled by the user before it finished.
    ///
    /// Not a failure: the caller asked for this. It travels as an error only
    /// because that is what unwinds a loop cleanly, and the shell treats it as
    /// a quiet stop rather than something to report.
    #[error("cancelled")]
    Cancelled,

    /// Building an output file failed for a reason that is not `PDFium`'s fault.
    ///
    /// Encoding a rendered page and rasterizing it are different operations;
    /// conflating the second's failures with the first's would send the reader
    /// looking in the wrong place.
    #[error("export failed: {0}")]
    Export(String),

    /// One input of a multi-file job could not be used.
    ///
    /// Multi-file operations need to name the file that failed: "the document
    /// is encrypted" is useless when the job was handed a dozen of them.
    #[error("{path} could not be used: {reason}")]
    Source {
        /// File that failed.
        path: PathBuf,
        /// Why, in user-facing terms.
        reason: String,
    },

    /// The operation exists in the PDF format but the engine cannot perform it.
    ///
    /// Used where PDFium's safe binding exposes no way to write a value —
    /// currently combo and list box selection, which only have immutable
    /// accessors in `pdfium-render`.
    #[error("{0} is not supported by the PDF engine")]
    Unsupported(String),

    /// Sending the document to a printer failed.
    #[error("print failed: {0}")]
    Print(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A failed write must not claim it could not *read* the file — the message
    /// is user-facing and the two are different problems. A failed print spool
    /// used to surface as "cannot read <spool>", sending the reader looking for
    /// a read bug that was not there.
    #[test]
    fn read_and_write_failures_read_differently() {
        let read = EngineError::Io {
            path: PathBuf::from("a.pdf"),
            reason: "not found".into(),
        };
        assert!(read.to_string().contains("cannot read"), "{read}");

        let write = EngineError::Write {
            path: PathBuf::from("a.pdf"),
            reason: "disk full".into(),
        };
        assert!(write.to_string().contains("cannot write"), "{write}");
    }
}
