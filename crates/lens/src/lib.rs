//! Offline word lookup: English→Bangla meanings, IPA pronunciation, and a
//! link out to Google Translate.
//!
//! This is a port of the `LingoLearn` Lens offline engine. It answers every
//! lookup from three bundled dictionary files, so a lookup never needs the
//! network, an API key, or a round trip. Google Translate is only ever a *link*
//! the shell opens in the browser; nothing is fetched here.
//!
//! The crate is deliberately split the same way the engine was:
//!
//! * [`text`] helpers — whitespace and part-of-speech shaping.
//! * [`Dictionaries`] — the parsed dictionaries, built once by [`load`].
//! * [`Dictionaries::lookup`] — one query in, one [`Lookup`] out.
//! * [`translate_link`] — the Google Translate URL for a piece of text.
//!
//! Only [`load`] touches the filesystem; everything else is a pure function of
//! its arguments, which is why the interesting behaviour is testable without
//! any of the real 12 MB dictionaries present.

mod dict;
mod link;
pub mod load;
mod lookup;
mod text;

pub use dict::{Dictionaries, ParseError};
pub use link::{
    DEFAULT_TARGET, TRANSLATE_BASE, TRANSLATE_TEXT_MAX, is_valid_target, translate_link,
};
pub use lookup::{
    Group, Lang, Lookup, LookupError, MAX_GROUPS, MAX_PHRASE_WORDS, MAX_TERMS_PER_GROUP,
};
