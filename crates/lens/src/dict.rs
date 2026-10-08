//! The bundled dictionaries, parsed once and then read-only.
//!
//! Three files make up every answer:
//!
//! * `E2Bdatabase.json` — a JSON array of `{"en": ..., "bn": ...}` rows, where
//!   `bn` is a comma-separated list of senses that may carry `(Adj.)` tags.
//! * `bangla_dictionary.txt` — `|word|meaning` lines, an extra source of
//!   meanings for words the JSON knows less about.
//! * `cmudict-0.7b-ipa.txt` — `WORD<TAB>ipa` lines, keyed upper-case.
//!
//! Parsers take `&str` so they can be tested without the real files;
//! [`crate::load`] is the only thing that reads them off disk.

use std::cell::OnceCell;
use std::collections::HashMap;

use serde::Deserialize;

use crate::text::{senses_from_raw, split_pos};

/// One row of `E2Bdatabase.json`.
///
/// Both fields default so a row missing either half is skipped rather than
/// failing the whole 100k-entry file.
#[derive(Clone, PartialEq, Eq, Debug, Deserialize)]
struct E2BRow {
    /// The English headword.
    #[serde(default)]
    en: String,
    /// Comma-separated Bangla senses.
    #[serde(default)]
    bn: String,
}

/// Why a dictionary file could not be turned into usable entries.
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
pub enum ParseError {
    /// `E2Bdatabase.json` was present but not a JSON array of rows.
    #[error("E2Bdatabase.json could not be parsed: {0}")]
    Json(String),
    /// The file parsed but held no row with both a word and a meaning.
    #[error("E2Bdatabase.json contains no usable entries")]
    NoEntries,
}

/// The three dictionaries a lookup answers from.
///
/// Built once at startup (or on first use) and then shared immutably. The
/// reverse map is filled in on first use because only Bangla input needs it and
/// building it costs a pass over every English→Bangla row.
#[derive(Clone, PartialEq, Debug)]
pub struct Dictionaries {
    /// Lower-cased English word to its raw, comma-separated Bangla senses.
    e2b: HashMap<String, String>,
    /// Lower-cased English word to extra meanings from the plain-text file.
    bangla: HashMap<String, Vec<String>>,
    /// Upper-cased word to its IPA pronunciation.
    ipa: HashMap<String, String>,
    /// Bare Bangla sense to the English words that produced it.
    reverse: OnceCell<HashMap<String, Vec<String>>>,
}

impl Dictionaries {
    /// Build from already-parsed maps.
    pub fn new(
        e2b: HashMap<String, String>,
        bangla: HashMap<String, Vec<String>>,
        ipa: HashMap<String, String>,
    ) -> Self {
        Self {
            e2b,
            bangla,
            ipa,
            reverse: OnceCell::new(),
        }
    }

    /// The raw Bangla meaning string for an English word, lower-cased.
    pub fn english_to_bangla(&self, word: &str) -> Option<&str> {
        self.e2b.get(word).map(String::as_str)
    }

    /// Extra meanings for an English word, lower-cased.
    pub fn extra_meanings(&self, word: &str) -> Option<&[String]> {
        self.bangla.get(word).map(Vec::as_slice)
    }

    /// The IPA pronunciation of a word.
    ///
    /// `CMUdict` is keyed upper-case, so the lookup upper-cases for the caller.
    pub fn ipa(&self, word: &str) -> Option<&str> {
        self.ipa.get(&word.to_uppercase()).map(String::as_str)
    }

    /// English words a Bangla term translates back to.
    ///
    /// Returns `None` until the reverse map has been built, which happens on
    /// the first call.
    pub fn reverse(&self, bangla: &str) -> Option<&[String]> {
        let map = self.reverse.get_or_init(|| {
            let mut map: HashMap<String, Vec<String>> = HashMap::new();
            for (en, raw) in &self.e2b {
                for sense in senses_from_raw(raw) {
                    let (_, bare) = split_pos(&sense);
                    if bare.is_empty() {
                        continue;
                    }
                    let words = map.entry(bare).or_default();
                    if !words.contains(en) {
                        words.push(en.clone());
                    }
                }
            }
            map
        });
        map.get(bangla).map(Vec::as_slice)
    }

    /// How many English headwords are known.
    pub fn entry_count(&self) -> usize {
        self.e2b.len()
    }

    /// Whether the dictionaries are empty, which means nothing can be answered.
    pub fn is_empty(&self) -> bool {
        self.e2b.is_empty()
    }
}

/// Parse `E2Bdatabase.json` into a lower-cased English→Bangla map.
///
/// Accepts the JSON array the file actually uses, and falls back to
/// `|en|bn` lines so a re-exported copy in the older format still loads.
pub fn parse_e2b(json: &str) -> Result<HashMap<String, String>, ParseError> {
    let trimmed = json.trim();
    let rows: Vec<E2BRow> = if trimmed.is_empty() {
        Vec::new()
    } else if trimmed.starts_with('[') {
        serde_json::from_str(trimmed).map_err(|err| ParseError::Json(err.to_string()))?
    } else {
        trimmed
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.trim().split('|').collect();
                (parts.len() >= 3).then(|| E2BRow {
                    en: parts[1].to_owned(),
                    bn: parts[2..].join("|"),
                })
            })
            .collect()
    };

    let mut map = HashMap::new();
    for row in rows {
        let en = row.en.trim().to_lowercase();
        let bn = row.bn.trim();
        if en.is_empty() || bn.is_empty() {
            continue;
        }
        // First definition wins; the file repeats some headwords.
        map.entry(en).or_insert_with(|| bn.to_owned());
    }

    if map.is_empty() {
        return Err(ParseError::NoEntries);
    }
    Ok(map)
}

/// Parse `bangla_dictionary.txt`: `|word|meaning` lines.
pub fn parse_bangla(text: &str) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() != 3 || !parts[0].trim().is_empty() {
            continue;
        }
        let en = parts[1].trim().to_lowercase();
        let bn = parts[2].trim();
        if en.is_empty() || bn.is_empty() {
            continue;
        }
        let meanings = map.entry(en).or_default();
        if !meanings.iter().any(|known| known == bn) {
            meanings.push(bn.to_owned());
        }
    }
    map
}

/// Parse `cmudict-0.7b-ipa.txt`: `WORD<TAB>ipa` lines, keyed upper-case.
pub fn parse_ipa(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with(";;;") {
            continue;
        }
        let Some(tab) = line.find('\t') else {
            continue;
        };
        if tab == 0 {
            continue;
        }
        let word = line[..tab].trim().to_uppercase();
        let ipa = line[tab + 1..].trim();
        if word.is_empty() || ipa.is_empty() {
            continue;
        }
        map.entry(word).or_insert_with(|| ipa.to_owned());
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dictionaries() -> Dictionaries {
        let json = r#"[{"en":"abandon","bn":"ত্যাগ করা, পরিত্যাগ"},{"en":"abandon","bn":"duplicate"},{"en":"","bn":"nothing"},{"en":"abbey","bn":"মঠ"}]"#;
        let bangla = "|abandon|ছেড়ে যাওয়া\n|abbey|গির্জা\n|broken|line\n";
        let ipa = ";;; a comment\nABANDON\təˈbændən\nABBEY\tˈæbi\n";
        Dictionaries::new(
            parse_e2b(json).expect("fixture parses"),
            parse_bangla(bangla),
            parse_ipa(ipa),
        )
    }

    #[test]
    fn json_rows_become_a_lowercased_map() {
        let dicts = dictionaries();
        assert_eq!(dicts.entry_count(), 2);
        assert_eq!(dicts.english_to_bangla("abandon"), Some("ত্যাগ করা, পরিত্যাগ"));
        // Duplicate headwords keep the first definition.
        assert_ne!(dicts.english_to_bangla("abandon"), Some("duplicate"));
        // Rows missing either half are dropped.
        assert_eq!(dicts.english_to_bangla(""), None);
    }

    #[test]
    fn pipe_formatted_json_still_loads() {
        let map = parse_e2b("|abbey|মঠ\n|abandon|ত্যাগ করা").expect("pipe fixture parses");
        assert_eq!(map.get("abbey").map(String::as_str), Some("মঠ"));
    }

    #[test]
    fn an_empty_dictionary_file_is_an_error_not_a_silence() {
        assert!(matches!(parse_e2b("[]"), Err(ParseError::NoEntries)));
        // Content that is not a JSON array is read as the older pipe format,
        // which yields nothing here rather than a parse failure.
        assert!(matches!(parse_e2b("not a dictionary"), Err(ParseError::NoEntries)));
        // A file that looks like JSON but is broken reports the parse failure.
        assert!(matches!(parse_e2b("[{\"en\":1}]"), Err(ParseError::Json(_))));
    }

    #[test]
    fn bangla_text_file_collects_meanings_without_duplicates() {
        let map = parse_bangla("|abbey|গির্জা\n|abbey|গির্জা\n|abbey|মঠ\n");
        assert_eq!(map.get("abbey").map(Vec::len), Some(2));
    }

    #[test]
    fn malformed_bangla_lines_are_skipped() {
        assert!(parse_bangla("abbey|মঠ\n|abbey\n\n|abbey|\n").is_empty());
    }

    #[test]
    fn ipa_is_keyed_uppercase_and_comments_are_skipped() {
        let map = parse_ipa(";;; comment\nABBEY\tˈæbi\nBEACH\tbiːtʃ\n\n");
        assert_eq!(map.get("ABBEY").map(String::as_str), Some("ˈæbi"));
        // Lookups go through the dictionary, which upper-cases for the caller.
        let dicts = Dictionaries::new(parse_e2b("[{\"en\":\"abbey\",\"bn\":\"মঠ\"}]").unwrap(), HashMap::new(), map);
        assert_eq!(dicts.ipa("abbey"), Some("ˈæbi"));
    }

    #[test]
    fn reverse_lookup_finds_english_from_a_bangla_term() {
        let dicts = dictionaries();
        assert_eq!(dicts.reverse("মঠ"), Some(["abbey".to_owned()].as_slice()));
        assert_eq!(dicts.reverse("অজানা"), None);
    }
}
