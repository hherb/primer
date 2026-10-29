use super::*;

const ALL_LOCALES: [Locale; 3] = [Locale::English, Locale::German, Locale::Hindi];

#[test]
fn english_function_words_are_stopwords() {
    for w in ["the", "is", "we", "now", "your", "what", "dont", "hello"] {
        assert!(is_stopword(Locale::English, w), "{w:?}");
    }
}

#[test]
fn english_content_words_are_kept() {
    // "near" / "not" stay terms (#309: children use them as content words).
    for w in ["sky", "blue", "near", "not", "light", "volcano", "stomach"] {
        assert!(!is_stopword(Locale::English, w), "{w:?}");
    }
}

#[test]
fn german_function_words_are_stopwords() {
    for w in ["ich", "wir", "können", "keine", "mehr", "bis", "tschüss"] {
        assert!(is_stopword(Locale::German, w), "{w:?}");
    }
}

#[test]
fn german_content_words_are_kept() {
    // "weiß" is also the colour white.
    for w in ["himmel", "blau", "weiß", "nicht", "vulkan", "spiel"] {
        assert!(!is_stopword(Locale::German, w), "{w:?}");
    }
}

#[test]
fn matching_is_case_insensitive() {
    assert!(is_stopword(Locale::English, "The"));
    assert!(is_stopword(Locale::German, "Können"));
}

#[test]
fn lists_are_per_locale() {
    // "die" is a German article but an English content word.
    assert!(is_stopword(Locale::German, "die"));
    assert!(!is_stopword(Locale::English, "die"));
}

#[test]
fn hindi_list_is_an_empty_no_op() {
    assert!(set(Locale::Hindi).is_empty());
    assert!(!is_stopword(Locale::Hindi, "the"));
}

#[test]
fn parse_skips_comments_and_blank_lines() {
    let got: Vec<_> = parse("# header\n\n  the \n# x\nis\n").collect();
    assert_eq!(got, ["the", "is"]);
}

#[test]
fn list_entries_are_in_sanitized_lowercase_form_without_duplicates() {
    // An entry with punctuation or uppercase could never match a sanitized
    // token, so it would be silently dead data.
    for locale in ALL_LOCALES {
        let mut seen = HashSet::new();
        for w in parse(list_source(locale)) {
            assert!(w.chars().all(char::is_alphanumeric), "{locale:?} {w:?}");
            assert_eq!(w, w.to_lowercase(), "{locale:?} {w:?}");
            assert!(seen.insert(w), "{locale:?} duplicate {w:?}");
        }
    }
}
