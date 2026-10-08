//! The "open in Google Translate" link.
//!
//! Nothing is fetched here. The offline dictionaries answer the popup; this
//! only builds the URL the shell hands to the browser, which is how the reader
//! gets a real sentence translation without this crate needing the network.

use std::fmt::Write;

/// Where the link points.
pub const TRANSLATE_BASE: &str = "https://translate.google.com/";

/// How much text the link carries. `GoogleTranslate` truncates anyway, and a
/// URL longer than this stops being paste-able.
pub const TRANSLATE_TEXT_MAX: usize = 1500;

/// The target used when the requested one is not a language code.
pub const DEFAULT_TARGET: &str = "bn";

/// Whether a string is a usable target language code.
///
/// Accepts `bn` and region-tagged forms like `zh-CN`, matching the shape the
/// Google Translate URL expects.
pub fn is_valid_target(code: &str) -> bool {
    let bytes = code.as_bytes();
    if bytes.len() < 2 || !bytes[..2].iter().all(u8::is_ascii_alphabetic) {
        return false;
    }
    match bytes.get(2) {
        None => true,
        Some(b'-') => {
            let region = &bytes[3..];
            (2..=4).contains(&region.len()) && region.iter().all(u8::is_ascii_alphabetic)
        }
        Some(_) => false,
    }
}

/// Build the Google Translate URL for a piece of text.
///
/// An unrecognised target falls back to [`DEFAULT_TARGET`] so the link is
/// always usable.
pub fn translate_link(text: &str, target: &str) -> String {
    let code = if is_valid_target(target) {
        target.to_ascii_lowercase()
    } else {
        DEFAULT_TARGET.to_owned()
    };
    let text: String = text.chars().take(TRANSLATE_TEXT_MAX).collect();

    format!(
        "{TRANSLATE_BASE}?sl=auto&tl={}&text={}&op=translate",
        encode(&code),
        encode(&text)
    )
}

/// Percent-encode a query value, with spaces as `+`.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            b' ' => out.push('+'),
            other => {
                let _ = write!(out, "%{other:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_carries_the_text_and_the_target() {
        assert_eq!(
            translate_link("abbey", "bn"),
            "https://translate.google.com/?sl=auto&tl=bn&text=abbey&op=translate"
        );
    }

    #[test]
    fn unrecognised_targets_fall_back_to_bangla() {
        assert!(translate_link("a", "").contains("tl=bn"));
        assert!(translate_link("a", "not a code").contains("tl=bn"));
        assert!(translate_link("a", "zh-CN").contains("tl=zh-cn"));
    }

    #[test]
    fn spaces_and_unicode_are_encoded() {
        let link = translate_link("two words", "bn");
        assert!(link.contains("two+words"));
        assert!(translate_link("মঠ", "bn").contains("%E0%A6"));
    }

    #[test]
    fn very_long_text_is_truncated() {
        let link = translate_link(&"a".repeat(TRANSLATE_TEXT_MAX + 500), "bn");
        assert!(link.len() < TRANSLATE_TEXT_MAX * 4);
    }

    #[test]
    fn target_codes_are_validated() {
        assert!(is_valid_target("bn"));
        assert!(is_valid_target("BN"));
        assert!(is_valid_target("zh-CN"));
        assert!(!is_valid_target("b"));
        assert!(!is_valid_target("bnn"));
        assert!(!is_valid_target("bn-"));
        assert!(!is_valid_target(""));
    }
}
