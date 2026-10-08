//! OS integration that has no business being in the domain or UI layers.
//!
//! Two things live here: speaking text through the system voice, and handing a
//! URL to the default browser. Both are thin wrappers over an OS command, which
//! is why they are isolated in this crate rather than threaded through the
//! application — everything else stays portable and testable.

pub mod speech;
pub mod url;

pub use speech::{SpeechError, Speaker};
pub use url::{UrlError, open};
