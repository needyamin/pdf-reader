//! Small, allocation-conscious text search primitives.
//!
//! The PDF engine owns extraction and the application owns scheduling. This
//! crate only turns page text into deterministic, case-insensitive results so
//! it can be tested without PDFium or a GUI.

/// Characters of context kept either side of the first match when building
/// [`SearchMatch::snippet`].
pub const SNIPPET_RADIUS: usize = 48;

/// A page containing at least one match for a query.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SearchMatch {
    /// Zero-based page index.
    pub page: u32,
    /// Number of non-overlapping matches on this page.
    pub count: usize,
    /// Excerpt of page text around the first match, with leading and trailing
    /// ellipses when the text was cut. Whitespace is collapsed so a single
    /// excerpt line fits a narrow sidebar. Empty when no excerpt could be
    /// taken, which the UI tolerates by falling back to the match count.
    pub snippet: String,
}

/// Search one string with a case-insensitive query.
///
/// Empty or whitespace-only queries intentionally produce no matches. Unicode
/// lowercasing is used rather than ASCII-only matching because PDF text often
/// contains accented or non-Latin characters.
pub fn match_count(text: &str, query: &str) -> usize {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return 0;
    }

    let text = text.to_lowercase();
    let mut count = 0;
    let mut start = 0;
    while let Some(relative) = text[start..].find(&query) {
        count += 1;
        start += relative + query.len();
        if start >= text.len() {
            break;
        }
    }
    count
}

/// Search `(page, text)` pairs and return only pages with matches.
pub fn search_pages<'a, I>(pages: I, query: &str) -> Vec<SearchMatch>
where
    I: IntoIterator<Item = (u32, &'a str)>,
{
    pages
        .into_iter()
        .filter_map(|(page, text)| {
            let count = match_count(text, query);
            (count > 0).then_some(SearchMatch {
                page,
                count,
                snippet: snippet_around(text, query, SNIPPET_RADIUS),
            })
        })
        .collect()
}

/// Build a one-line excerpt of `text` centred on the first match of `query`.
///
/// Returns an empty string for an empty query or when there is no match, so
/// callers can treat "no snippet" as "no match" without a second scan.
pub fn snippet_around(text: &str, query: &str, radius: usize) -> String {
    let needle: Vec<char> = query.trim().to_lowercase().chars().collect();
    if needle.is_empty() {
        return String::new();
    }

    let chars: Vec<char> = text.chars().collect();
    // `to_lowercase` changes the character count for a handful of code points
    // (a few Georgian and Cherokee letters expand). If that happened the two
    // vectors are no longer index-compatible, so take a plain leading excerpt
    // rather than slice at a wrong offset.
    let lower: Vec<char> = text.to_lowercase().chars().collect();
    if lower.len() != chars.len() {
        return collapse_whitespace(&chars[..chars.len().min(radius * 2)]);
    }

    let Some(start) = (0..=lower.len().saturating_sub(needle.len()))
        .find(|&i| lower[i..i + needle.len()] == needle[..])
    else {
        return String::new();
    };

    let end = (start + needle.len()).min(chars.len());
    let from = start.saturating_sub(radius);
    let to = (end + radius).min(chars.len());

    let mut excerpt = collapse_whitespace(&chars[from..to]);
    if from > 0 {
        excerpt.insert(0, '…');
    }
    if to < chars.len() {
        excerpt.push('…');
    }
    excerpt
}

/// Collapse runs of whitespace (PDF text is full of newlines) into single
/// spaces so an excerpt renders as one line.
fn collapse_whitespace(chars: &[char]) -> String {
    let mut out = String::with_capacity(chars.len());
    let mut pending_space = false;
    for &c in chars {
        if c.is_whitespace() {
            // Keep the first space of a run, drop the rest, and never lead.
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_is_case_insensitive_and_non_overlapping() {
        assert_eq!(match_count("PDF pdf Pdfium", "pdf"), 3);
        assert_eq!(match_count("aaaa", "aa"), 2);
    }

    #[test]
    fn whitespace_query_is_empty() {
        assert_eq!(match_count("anything", "  \t"), 0);
    }

    #[test]
    fn page_search_keeps_page_order_and_skips_empty_pages() {
        let results = search_pages([(0, "hello"), (1, "PDF pdf"), (2, "none")], "pdf");
        assert_eq!(
            results,
            vec![SearchMatch {
                page: 1,
                count: 2,
                snippet: "PDF pdf".into(),
            }]
        );
    }

    #[test]
    fn snippet_is_centred_on_the_match_and_marked_when_cut() {
        let text = "one two three FOUR five six seven eight nine ten eleven twelve";
        let snippet = snippet_around(text, "four", 8);
        assert!(
            snippet.starts_with('…') && snippet.ends_with('…'),
            "both ends were cut so both need an ellipsis: {snippet:?}"
        );
        assert!(snippet.contains("FOUR"), "must keep the original casing");

        // Nothing to cut on the left: no leading ellipsis.
        let at_start = snippet_around("needle at the very start of a long line", "needle", 4);
        assert!(!at_start.starts_with('…'), "{at_start:?}");
        assert!(at_start.starts_with("needle"));
    }

    #[test]
    fn snippet_collapses_newlines_into_single_spaces() {
        // A radius wide enough that nothing is cut, so the assertion is purely
        // about whitespace: PDF text is full of newlines and runs of spaces.
        let snippet = snippet_around("alpha\n\nbeta\n   gamma  delta", "gamma", 20);
        assert_eq!(snippet, "alpha beta gamma delta");
        assert!(!snippet.contains('\n'));
    }

    #[test]
    fn snippet_is_empty_when_there_is_no_match_or_no_query() {
        assert_eq!(snippet_around("nothing here", "absent", 8), "");
        assert_eq!(snippet_around("anything", "   ", 8), "");
    }
}
