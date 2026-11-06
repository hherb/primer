//! Per-locale stopword lists for knowledge-base BM25 retrieval (issue #386).
//!
//! `sanitize_fts_phrase` OR-joins every token, so filler words in small
//! talk ("can **we** stop **now**") match passages. The KB drops these
//! words before the OR-join (`sanitize_fts_phrase_excluding`); long-term
//! memory retrieval does not, because a child's own turns are the corpus
//! there.
//!
//! The lists are data: one file per locale under `primer-core/stopwords/`,
//! embedded with `include_str!` the same way prompt packs are. Adding a
//! locale is a new `<pack_id>.txt` plus one arm each in `list_source` and
//! `set`.

use crate::i18n::Locale;
use std::collections::HashSet;
use std::sync::OnceLock;
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

const EN: &str = include_str!("../stopwords/en.txt");
const DE: &str = include_str!("../stopwords/de.txt");
const HI: &str = include_str!("../stopwords/hi.txt");

/// The raw embedded list for `locale`.
fn list_source(locale: Locale) -> &'static str {
    match locale {
        Locale::English => EN,
        Locale::German => DE,
        Locale::Hindi => HI,
    }
}

/// Parse a list: one token per line, blank lines and `#` comment lines
/// skipped, surrounding whitespace trimmed.
fn parse(source: &str) -> impl Iterator<Item = &str> {
    source
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
}

/// Lookup key: lowercase with diacritics removed, the way the FTS5
/// `unicode61` tokenizer folds terms. So "für", NFD "für" (which the
/// sanitizer leaves as "fur") and a typed "fur" all match one entry (#391).
fn fold(token: &str) -> String {
    token
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .collect()
}

/// The parsed, folded set for `locale`, built once per process.
fn set(locale: Locale) -> &'static HashSet<String> {
    static EN_SET: OnceLock<HashSet<String>> = OnceLock::new();
    static DE_SET: OnceLock<HashSet<String>> = OnceLock::new();
    static HI_SET: OnceLock<HashSet<String>> = OnceLock::new();
    let cell = match locale {
        Locale::English => &EN_SET,
        Locale::German => &DE_SET,
        Locale::Hindi => &HI_SET,
    };
    cell.get_or_init(|| parse(list_source(locale)).map(fold).collect())
}

/// Whether `token` is a stopword in `locale`. Case- and
/// diacritic-insensitive (see `fold`); `token` is expected in the
/// punctuation-stripped form the FTS sanitizer yields.
pub fn is_stopword(locale: Locale, token: &str) -> bool {
    set(locale).contains(&fold(token))
}

#[cfg(test)]
mod tests;
