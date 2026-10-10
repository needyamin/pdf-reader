//! Turning a piece of selected text into an answer.
//!
//! The lookup tries four things in order, cheapest first:
//!
//! 1. A Bangla selection is reverse-looked-up to English.
//! 2. An exact English→Bangla hit, which also carries the group list and IPA.
//! 3. A single unknown word falls back to the plain-text meanings file.
//! 4. Anything longer is treated as a phrase and translated word by word,
//!    which is why such a result is marked [`Lookup::partial`].

use std::collections::HashMap;

use crate::dict::Dictionaries;
use crate::text::{contains_bangla, is_phrase, senses_from_raw, split_pos, strip_edges};

/// How many words of a phrase are translated before the rest is passed through.
pub const MAX_PHRASE_WORDS: usize = 12;

/// How many part-of-speech groups a single lookup shows.
pub const MAX_GROUPS: usize = 3;

/// How many terms one part-of-speech group holds.
pub const MAX_TERMS_PER_GROUP: usize = 4;

/// How many Bangla terms a reverse lookup joins into the translation line.
const MAX_GLOSSES_IN_TRANSLATION: usize = 3;

/// Which side of the pair the selected text was on.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Lang {
    /// The selection was English, so the answer is Bangla.
    #[default]
    En,
    /// The selection was Bangla, so the answer is English.
    Bn,
}

impl Lang {
    /// The BCP-47 code used when speaking or linking out.
    pub const fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Bn => "bn",
        }
    }

    /// The language a lookup *answers* in.
    pub const fn target(self) -> &'static str {
        match self {
            Self::En => "bn",
            Self::Bn => "en",
        }
    }
}

/// One part of speech and the terms under it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Group {
    /// Part-of-speech tags taken from the parentheses in a sense, joined.
    /// Empty for senses the dictionary left untagged.
    pub pos: String,
    /// The terms, de-duplicated and capped at [`MAX_TERMS_PER_GROUP`].
    pub terms: Vec<String>,
}

/// A successful lookup.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Lookup {
    /// The meaning, shown as the headline of the popup. Empty when a Bangla
    /// word was selected and nothing in the dictionary maps back to it.
    pub translation: String,
    /// IPA pronunciation without slashes, or empty when the word is not in
    /// `CMUdict`. The popup adds the slashes so the field stays plain data.
    pub ipa: String,
    /// Meanings grouped by part of speech.
    pub groups: Vec<Group>,
    /// Which language the selection was detected as.
    pub detected: Lang,
    /// True when the answer came from translating a phrase word by word, so
    /// the UI can say the result is approximate.
    pub partial: bool,
}

impl Lookup {
    /// Whether there is anything worth drawing beyond the selected text.
    pub fn has_content(&self) -> bool {
        !self.translation.is_empty() || !self.groups.is_empty() || !self.ipa.is_empty()
    }
}

/// Why no answer could be given.
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
pub enum LookupError {
    /// The selection was empty once punctuation was trimmed.
    #[error("select a word to look up")]
    EmptySelection,
    /// Nothing in the bundled dictionaries knows this text.
    #[error("no offline meaning found for \"{0}\"")]
    NoMeaning(String),
}

/// Group senses by part of speech, capped at [`MAX_GROUPS`] groups of
/// [`MAX_TERMS_PER_GROUP`] terms.
fn build_groups(senses: &[String]) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    let mut by_pos: HashMap<String, usize> = HashMap::new();

    for sense in senses {
        let (pos, bare) = split_pos(sense);
        if bare.is_empty() {
            continue;
        }
        let index = if let Some(&index) = by_pos.get(&pos) {
            index
        } else {
            if groups.len() >= MAX_GROUPS {
                continue;
            }
            groups.push(Group {
                pos: pos.clone(),
                terms: Vec::new(),
            });
            let index = groups.len() - 1;
            by_pos.insert(pos, index);
            index
        };
        let group = &mut groups[index];
        if !group.terms.contains(&bare) && group.terms.len() < MAX_TERMS_PER_GROUP {
            group.terms.push(bare);
        }
    }

    groups
}

impl Dictionaries {
    /// Look up a word or phrase.
    ///
    /// The text is trimmed of surrounding punctuation first, so a word picked
    /// out of a PDF with its full stop attached still resolves.
    pub fn lookup(&self, text: &str) -> Result<Lookup, LookupError> {
        let query = strip_edges(text);
        if query.is_empty() {
            return Err(LookupError::EmptySelection);
        }

        if contains_bangla(&query) {
            return Ok(self.lookup_bangla(&query));
        }

        let lower = query.to_lowercase();
        let extra = self.extra_meanings(&lower);

        if let Some(raw) = self.english_to_bangla(&lower) {
            return Ok(Lookup {
                translation: raw.to_owned(),
                ipa: self.ipa(&query).unwrap_or_default().to_owned(),
                groups: Self::groups_for(raw, extra),
                detected: Lang::En,
                partial: false,
            });
        }

        if !is_phrase(&query) {
            let Some(extra) = extra else {
                return Err(LookupError::NoMeaning(query));
            };
            return Ok(Lookup {
                translation: extra.first().cloned().unwrap_or_default(),
                ipa: self.ipa(&query).unwrap_or_default().to_owned(),
                groups: vec![Group {
                    pos: String::new(),
                    terms: extra.iter().take(MAX_TERMS_PER_GROUP).cloned().collect(),
                }],
                detected: Lang::En,
                partial: false,
            });
        }

        self.lookup_phrase(&query)
    }

    /// Reverse lookup: a Bangla term back to the English words behind it.
    fn lookup_bangla(&self, query: &str) -> Lookup {
        if !is_phrase(query) {
            let (_, bare) = split_pos(query);
            if let Some(glosses) = self.reverse(&bare) {
                let translation = glosses
                    .iter()
                    .take(MAX_GLOSSES_IN_TRANSLATION)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                let terms = glosses
                    .iter()
                    .take(MAX_TERMS_PER_GROUP)
                    .cloned()
                    .collect::<Vec<_>>();
                return Lookup {
                    translation,
                    groups: vec![Group {
                        pos: "English".to_owned(),
                        terms,
                    }],
                    detected: Lang::Bn,
                    partial: false,
                    ..Default::default()
                };
            }
        }
        // Already Bangla and unmapped: there is nothing to translate, but the
        // popup still opens so the reader can send it to Google Translate.
        Lookup {
            detected: Lang::Bn,
            ..Default::default()
        }
    }

    /// Build the part-of-speech groups for a raw meaning string, merging in
    /// the extra meanings the plain-text file knows about.
    fn groups_for(raw: &str, extra: Option<&[String]>) -> Vec<Group> {
        let mut groups = build_groups(&senses_from_raw(raw));
        let Some(extra) = extra else {
            return groups;
        };

        let plain = if let Some(index) = groups.iter().position(|group| group.pos.is_empty()) {
            index
        } else {
            groups.push(Group::default());
            groups.len() - 1
        };

        for meaning in extra {
            let known = groups
                .iter()
                .any(|group| group.terms.iter().any(|term| term == meaning));
            if known {
                continue;
            }
            let group = &mut groups[plain];
            if group.terms.len() < MAX_TERMS_PER_GROUP {
                group.terms.push(meaning.clone());
            }
        }

        groups
    }

    /// Translate a phrase word by word, leaving unknown words as they are.
    fn lookup_phrase(&self, query: &str) -> Result<Lookup, LookupError> {
        let mut out: Vec<String> = Vec::new();
        let mut mapped = false;

        for word in query.split(' ').take(MAX_PHRASE_WORDS) {
            let token = strip_edges(word);
            let lower = token.to_lowercase();
            let replacement = self
                .english_to_bangla(&lower)
                .and_then(|raw| senses_from_raw(raw).into_iter().next())
                .map(|first| {
                    let (_, bare) = split_pos(&first);
                    if bare.is_empty() { first } else { bare }
                })
                .or_else(|| {
                    self.extra_meanings(&lower)
                        .and_then(|meanings| meanings.first().cloned())
                });

            match replacement {
                Some(meaning) => {
                    out.push(meaning);
                    mapped = true;
                }
                None => out.push(if token.is_empty() {
                    word.to_owned()
                } else {
                    token
                }),
            }
        }

        if !mapped {
            return Err(LookupError::NoMeaning(query.to_owned()));
        }

        Ok(Lookup {
            translation: out.join(" "),
            detected: Lang::En,
            partial: true,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict::{parse_bangla, parse_e2b, parse_ipa};

    fn dictionaries() -> Dictionaries {
        let json = r#"[
            {"en":"abandon","bn":"ত্যাগ করা (v.), পরিত্যাগ (n.)"},
            {"en":"abbey","bn":"মঠ"},
            {"en":"true","bn":"সত্য (Adj.), প্রকৃত (Adj.)"}
        ]"#;
        let bangla = "|abandon|ছেড়ে যাওয়া\n|bright|উজ্জ্বল\n";
        let ipa = "ABBEY\tˈæbi\nABANDON\təˈbændən\n";
        Dictionaries::new(
            parse_e2b(json).expect("fixture parses"),
            parse_bangla(bangla),
            parse_ipa(ipa),
        )
    }

    #[test]
    fn an_exact_hit_carries_groups_and_ipa() {
        let lookup = dictionaries().lookup("abbey").expect("abbey is known");
        assert_eq!(lookup.translation, "মঠ");
        assert_eq!(lookup.ipa, "ˈæbi");
        assert_eq!(lookup.detected, Lang::En);
        assert!(!lookup.partial);
    }

    #[test]
    fn punctuation_around_a_word_is_ignored() {
        let lookup = dictionaries().lookup("  abbey,  ").expect("abbey is known");
        assert_eq!(lookup.translation, "মঠ");
    }

    #[test]
    fn part_of_speech_tags_become_groups() {
        let lookup = dictionaries().lookup("abandon").expect("abandon is known");
        assert_eq!(
            lookup.groups,
            vec![
                Group {
                    pos: "v.".to_owned(),
                    terms: vec!["ত্যাগ করা".to_owned()],
                },
                Group {
                    pos: "n.".to_owned(),
                    terms: vec!["পরিত্যাগ".to_owned()],
                },
                Group {
                    pos: String::new(),
                    // The plain-text file's extra meaning lands in the untagged group.
                    terms: vec!["ছেড়ে যাওয়া".to_owned()],
                },
            ]
        );
    }

    #[test]
    fn an_unknown_word_falls_back_to_the_text_dictionary() {
        let lookup = dictionaries()
            .lookup("bright")
            .expect("bright has a text meaning");
        assert_eq!(lookup.translation, "উজ্জ্বল");
        assert_eq!(lookup.groups.len(), 1);
    }

    #[test]
    fn an_unknown_word_with_no_meaning_is_reported() {
        let error = dictionaries().lookup("zzzz").expect_err("unknown word");
        assert_eq!(error, LookupError::NoMeaning("zzzz".to_owned()));
    }

    #[test]
    fn an_empty_selection_says_so() {
        assert_eq!(
            dictionaries()
                .lookup("  ...  ")
                .expect_err("nothing selected"),
            LookupError::EmptySelection
        );
    }

    #[test]
    fn a_phrase_is_translated_word_by_word() {
        let lookup = dictionaries()
            .lookup("abbey bright zzzz")
            .expect("phrase maps at least one word");
        assert_eq!(lookup.translation, "মঠ উজ্জ্বল zzzz");
        assert!(lookup.partial, "a word-by-word answer is approximate");
    }

    #[test]
    fn a_phrase_nobody_knows_still_fails() {
        assert!(dictionaries().lookup("zzzz yyyy").is_err());
    }

    #[test]
    fn bangla_reverses_to_english() {
        let lookup = dictionaries().lookup("মঠ").expect("মঠ maps back");
        assert_eq!(lookup.detected, Lang::Bn);
        assert_eq!(lookup.translation, "abbey");
        assert_eq!(lookup.groups[0].pos, "English");
    }

    #[test]
    fn bangla_with_no_reverse_entry_opens_an_empty_popup() {
        let lookup = dictionaries().lookup("অজানা").expect("still opens");
        assert_eq!(lookup.detected, Lang::Bn);
        assert!(lookup.translation.is_empty());
    }

    #[test]
    fn groups_are_capped_at_three() {
        let senses: Vec<String> = ["a (n.)", "b (v.)", "c (adj.)", "d (adv.)"]
            .iter()
            .map(|sense| (*sense).to_owned())
            .collect();
        let groups = build_groups(&senses);
        assert_eq!(groups.len(), MAX_GROUPS);
    }
}
