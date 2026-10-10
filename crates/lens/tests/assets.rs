//! Checks the shipped dictionaries load and answer real queries.
//!
//! Skipped when `assets/lens` is absent, so a clone without the 12 MB
//! dictionary files still runs green.

use std::path::PathBuf;

use pdfreader_lens::load;
use pdfreader_lens::{Dictionaries, LookupError};

fn asset_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/lens")
}

/// Load the real dictionaries, or `None` when the assets are not present.
fn dictionaries() -> Option<Dictionaries> {
    let dir = asset_dir();
    if !dir.join("E2Bdatabase.json").is_file() {
        eprintln!("skipping: {} has no dictionaries", dir.display());
        return None;
    }
    Some(load::load(&dir).expect("the shipped dictionaries load"))
}

#[test]
fn shipped_dictionaries_load() {
    let Some(dicts) = dictionaries() else {
        return;
    };
    assert!(
        dicts.entry_count() > 100_000,
        "expected the full E2B database, got {} entries",
        dicts.entry_count()
    );
    assert!(!dicts.is_empty());
}

#[test]
fn a_known_word_has_a_bangla_meaning() {
    let Some(dicts) = dictionaries() else {
        return;
    };
    let lookup = dicts.lookup("abandon").expect("abandon is a common word");
    assert!(!lookup.translation.is_empty());
    assert!(
        lookup
            .translation
            .chars()
            .any(|c| matches!(c, '\u{0980}'..='\u{09FF}'))
    );
    assert!(!lookup.groups.is_empty());
}

#[test]
fn ipa_comes_from_cmudict() {
    let Some(dicts) = dictionaries() else {
        return;
    };
    assert_eq!(dicts.ipa("computer"), Some("kəmˈpjuːtɝ"));
    assert_eq!(dicts.ipa("abbey"), Some("ˈæbi"));
}

#[test]
fn a_phrase_is_translated_word_by_word() {
    let Some(dicts) = dictionaries() else {
        return;
    };
    let lookup = dicts
        .lookup("abandon the abbey")
        .expect("both words are known");
    assert!(lookup.partial);
    assert!(
        lookup
            .translation
            .chars()
            .any(|c| matches!(c, '\u{0980}'..='\u{09FF}'))
    );
}

#[test]
fn an_unknown_word_is_reported() {
    let Some(dicts) = dictionaries() else {
        return;
    };
    assert!(matches!(
        dicts.lookup("qqzzxx"),
        Err(LookupError::NoMeaning(_))
    ));
}

#[test]
fn bangla_reverses_to_english() {
    let Some(dicts) = dictionaries() else {
        return;
    };
    let lookup = dicts.lookup("মঠ").expect("মঠ is in the database");
    assert_eq!(lookup.detected, pdfreader_lens::Lang::Bn);
    assert!(!lookup.translation.is_empty());
}
