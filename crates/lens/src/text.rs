//! Shaping helpers shared by the dictionary parsers and the lookup.
//!
//! Selected text arrives with whatever punctuation the page happened to have
//! next to it, and dictionary senses arrive with part-of-speech tags inline.
//! These helpers strip the first and split out the second.

/// Whether a character is punctuation or a symbol.
///
/// Rust's `char` exposes the Unicode `Alphabetic` and `Numeric` properties but
/// not `Punctuation`/`Symbol`, so the ranges a PDF is likely to contain are
/// listed here: Latin-1, General Punctuation, currency, letterlike, the big
/// symbol/arrow/dingbat stretch, CJK punctuation, and fullwidth forms.
fn is_punct_or_symbol(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(
            c,
            '\u{00A1}'..='\u{00BF}'
                | '\u{2000}'..='\u{206F}'
                | '\u{20A0}'..='\u{20CF}'
                | '\u{2100}'..='\u{214F}'
                | '\u{2190}'..='\u{2BFF}'
                | '\u{3000}'..='\u{303F}'
                | '\u{FE30}'..='\u{FE4F}'
                | '\u{FF01}'..='\u{FF65}'
        )
}

/// Whether a character is the kind that gets trimmed off a selection's edge.
fn is_edge(c: char) -> bool {
    c.is_whitespace() || is_punct_or_symbol(c)
}

/// Collapse every run of whitespace to one space and trim the ends.
pub(crate) fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut gap = false;
    for c in text.chars() {
        if c.is_whitespace() {
            gap = true;
        } else {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(c);
        }
    }
    out.trim_start().to_owned()
}

/// Trim punctuation, symbols and whitespace from both ends, then collapse the
/// whitespace that is left.
///
/// A word picked out of a PDF is usually "word." or "(word" — this is what
/// turns those into `word` before the dictionaries are asked.
pub(crate) fn strip_edges(value: &str) -> String {
    collapse_whitespace(value.trim_matches(is_edge))
}

/// Split a dictionary sense into its part-of-speech tags and the bare term.
///
/// `"সত্য (Adj.), প্রকৃত (Adj.)"` is one sense string; the tags inside
/// parentheses become the returned part of speech and what is left is the term
/// a reader should see.
pub(crate) fn split_pos(sense: &str) -> (String, String) {
    let mut tags: Vec<&str> = Vec::new();
    let mut bare = String::with_capacity(sense.len());
    let mut rest = sense;

    while let Some(open) = rest.find('(') {
        let (before, after) = rest.split_at(open);
        bare.push_str(before);
        let after = &after[1..];
        if let Some(close) = after.find(')') {
            let tag = after[..close].trim();
            if !tag.is_empty() {
                tags.push(tag);
            }
            rest = &after[close + 1..];
        } else {
            // An unmatched parenthesis is not a tag; keep it literally.
            bare.push('(');
            rest = after;
            break;
        }
        bare.push(' ');
    }
    bare.push_str(rest);

    (tags.join(" "), collapse_whitespace(&bare))
}

/// Split a raw comma-separated meaning string into individual senses.
pub(crate) fn senses_from_raw(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|sense| !sense.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Whether the text contains any Bengali character.
pub(crate) fn contains_bangla(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(c, '\u{0980}'..='\u{09FF}'))
}

/// Whether the text holds more than one word.
pub(crate) fn is_phrase(text: &str) -> bool {
    text.chars().any(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_are_stripped_but_the_word_survives() {
        assert_eq!(strip_edges("  hello "), "hello");
        assert_eq!(strip_edges("\"hello,\""), "hello");
        assert_eq!(strip_edges("(word)"), "word");
        assert_eq!(strip_edges("..."), "");
        // An apostrophe inside a word is punctuation, but it is not an edge.
        assert_eq!(strip_edges("don't"), "don't");
    }

    #[test]
    fn inner_whitespace_collapses_to_one_space() {
        assert_eq!(strip_edges("a   b\t\nc"), "a b c");
    }

    #[test]
    fn pos_tags_come_out_of_parens() {
        let (pos, bare) = split_pos("সত্য (Adj.)");
        assert_eq!(pos, "Adj.");
        assert_eq!(bare, "সত্য");
    }

    #[test]
    fn several_tags_join_into_one_pos() {
        let (pos, bare) = split_pos("ঠিক (Adj.) (Adv.)");
        assert_eq!(pos, "Adj. Adv.");
        assert_eq!(bare, "ঠিক");
    }

    #[test]
    fn an_unmatched_paren_is_kept_literally() {
        let (pos, bare) = split_pos("half (open");
        assert_eq!(pos, "");
        assert_eq!(bare, "half (open");
    }

    #[test]
    fn senses_split_on_commas_and_drop_empties() {
        assert_eq!(senses_from_raw("ভুল, মিথ্যা, , নকল"), ["ভুল", "মিথ্যা", "নকল"]);
    }

    #[test]
    fn bangla_is_detected_by_script() {
        assert!(contains_bangla("সত্য"));
        assert!(!contains_bangla("truth"));
    }

    #[test]
    fn a_phrase_is_anything_with_a_space() {
        assert!(is_phrase("two words"));
        assert!(!is_phrase("word"));
    }
}
