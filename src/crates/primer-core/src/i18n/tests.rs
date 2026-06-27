use super::*;
use crate::error::InferenceError;
use std::time::Duration;

#[test]
fn locale_default_is_english() {
    assert_eq!(Locale::default(), Locale::English);
}

#[test]
fn locale_german_pack_id_and_bcp47() {
    assert_eq!(Locale::German.pack_id(), "de");
    assert_eq!(Locale::German.bcp47(), "de-DE");
    assert_eq!(Locale::German.name(), "German");
    assert_eq!(Locale::from_pack_id("de"), Some(Locale::German));
}

#[test]
fn render_inference_error_dispatches_on_locale() {
    let auth_en = render_inference_error(&InferenceError::Auth, &Locale::English);
    let auth_de = render_inference_error(&InferenceError::Auth, &Locale::German);
    assert_ne!(
        auth_en, auth_de,
        "different locales must produce different text"
    );
    assert!(auth_de.contains("ANTHROPIC_API_KEY"));
    assert!(auth_de.contains("fehlgeschlagen"));
}

#[test]
fn german_rate_limited_singular_vs_plural() {
    let one = render_inference_error(
        &InferenceError::RateLimited {
            retry_after: Some(Duration::from_secs(1)),
        },
        &Locale::German,
    );
    let many = render_inference_error(
        &InferenceError::RateLimited {
            retry_after: Some(Duration::from_secs(7)),
        },
        &Locale::German,
    );
    assert!(
        one.contains("1 Sekunde") && !one.contains("1 Sekunden"),
        "singular form expected for 1s: {one}"
    );
    assert!(many.contains("7 Sekunden"), "plural form expected: {many}");
}

#[test]
fn german_model_not_found_includes_model_name_and_pull_hint() {
    let s = render_inference_error(
        &InferenceError::ModelNotFound {
            model: "llama3.2".into(),
        },
        &Locale::German,
    );
    assert!(s.contains("llama3.2"), "got: {s}");
    assert!(s.to_lowercase().contains("pull"), "got: {s}");
}

#[test]
fn german_other_does_not_leak_inner_dev_string() {
    let s = render_inference_error(
        &InferenceError::Other("RAW_DEV_STRING_FOO".into()),
        &Locale::German,
    );
    assert!(
        !s.contains("RAW_DEV_STRING_FOO"),
        "Other's inner string must not reach users; got: {s}"
    );
}

#[test]
fn reasoning_without_answer_is_not_retryable() {
    assert!(!InferenceError::ReasoningWithoutAnswer.is_retryable());
}

#[test]
fn reasoning_without_answer_renders_nonempty_per_locale() {
    for &l in &[Locale::English, Locale::German, Locale::Hindi] {
        let s = render_inference_error(&InferenceError::ReasoningWithoutAnswer, &l);
        assert!(!s.trim().is_empty(), "empty render for {l:?}");
    }
}

#[test]
fn reasoning_without_answer_hindi_has_devanagari() {
    let s = render_inference_error(&InferenceError::ReasoningWithoutAnswer, &Locale::Hindi);
    assert!(
        s.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c)),
        "expected Devanagari, got: {s}"
    );
}

#[test]
fn locale_pack_id_round_trips_via_from_pack_id() {
    for &l in Locale::ALL {
        assert_eq!(Locale::from_pack_id(l.pack_id()), Some(l));
    }
}

#[test]
fn locale_from_pack_id_unknown_returns_none() {
    assert_eq!(Locale::from_pack_id("zz"), None);
    assert_eq!(Locale::from_pack_id(""), None);
}

#[test]
fn locale_hindi_pack_id_and_bcp47() {
    assert_eq!(Locale::Hindi.pack_id(), "hi");
    assert_eq!(Locale::Hindi.bcp47(), "hi-IN");
    assert_eq!(Locale::Hindi.name(), "Hindi");
    assert_eq!(Locale::from_pack_id("hi"), Some(Locale::Hindi));
}

/// `endonym()` returns each locale's name in its own language —
/// the label shown in a locale-picker UI so a German child sees
/// "Deutsch" rather than "German". Pinned for every variant so a
/// future locale addition can't silently fall through to a default.
#[test]
fn locale_endonym_matches_locale_native_spelling() {
    assert_eq!(Locale::English.endonym(), "English");
    assert_eq!(Locale::German.endonym(), "Deutsch");
    assert_eq!(Locale::Hindi.endonym(), "हिन्दी");
}

/// Hindi is gated as a preview locale: present in the enum, available
/// via --language hi for developers, but excluded from Locale::ALL so
/// CLI/GUI pickers don't surface it to end users. Flipping ALL is the
/// native-speaker-review PR. Pins both the length (no accidental
/// addition) and Hindi absence (the specific gate) so this is the
/// single source of truth for `ALL`'s composition.
#[test]
fn locale_all_excludes_hindi_until_translation_reviewed() {
    assert_eq!(Locale::ALL.len(), 2);
    assert!(Locale::ALL.contains(&Locale::English));
    assert!(Locale::ALL.contains(&Locale::German));
    assert!(!Locale::ALL.contains(&Locale::Hindi));
}

/// Each Hindi inference-error variant returns a non-empty string with
/// at least one Devanagari character. Guards against accidental
/// English fall-through.
#[test]
fn hindi_inference_errors_contain_devanagari() {
    use std::time::Duration;
    let cases: Vec<InferenceError> = vec![
        InferenceError::Auth,
        InferenceError::RateLimited { retry_after: None },
        InferenceError::RateLimited {
            retry_after: Some(Duration::from_secs(1)),
        },
        InferenceError::RateLimited {
            retry_after: Some(Duration::from_secs(5)),
        },
        InferenceError::ServiceUnavailable,
        InferenceError::NetworkUnavailable,
        InferenceError::ModelNotFound {
            model: "llama3.2".into(),
        },
        InferenceError::ReasoningWithoutAnswer,
        InferenceError::Other("RAW_DEV_STRING".into()),
    ];
    for err in cases {
        let s = render_inference_error(&err, &Locale::Hindi);
        assert!(!s.is_empty(), "empty render_hindi for {err:?}");
        assert!(
            s.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c)),
            "no Devanagari in render_hindi for {err:?}: {s}"
        );
    }
}

#[test]
fn hindi_model_not_found_includes_model_name() {
    let s = render_inference_error(
        &InferenceError::ModelNotFound {
            model: "llama3.2".into(),
        },
        &Locale::Hindi,
    );
    assert!(s.contains("llama3.2"), "got: {s}");
}

#[test]
fn hindi_other_does_not_leak_inner_dev_string() {
    let s = render_inference_error(
        &InferenceError::Other("RAW_DEV_STRING_FOO".into()),
        &Locale::Hindi,
    );
    assert!(
        !s.contains("RAW_DEV_STRING_FOO"),
        "Other's inner string must not reach users; got: {s}"
    );
}

#[test]
fn locale_bcp47_includes_region() {
    assert_eq!(Locale::English.bcp47(), "en-US");
}

#[test]
fn auth_message_mentions_api_key_env_var() {
    let s = render_inference_error(&InferenceError::Auth, &Locale::English);
    assert!(s.contains("ANTHROPIC_API_KEY"), "got: {s}");
}

#[test]
fn rate_limited_with_retry_after_includes_seconds() {
    let s = render_inference_error(
        &InferenceError::RateLimited {
            retry_after: Some(Duration::from_secs(7)),
        },
        &Locale::English,
    );
    assert!(
        s.contains("7"),
        "rendered string should mention the seconds: {s}"
    );
}

#[test]
fn rate_limited_with_one_second_retry_after_uses_singular() {
    let s = render_inference_error(
        &InferenceError::RateLimited {
            retry_after: Some(Duration::from_secs(1)),
        },
        &Locale::English,
    );
    assert!(
        s.contains("1 second.") && !s.contains("1 seconds"),
        "expected singular 'second' for 1-second retry-after; got: {s}"
    );
}

#[test]
fn rate_limited_without_retry_after_uses_generic_phrasing() {
    let s = render_inference_error(
        &InferenceError::RateLimited { retry_after: None },
        &Locale::English,
    );
    assert!(
        s.to_lowercase().contains("busy") || s.to_lowercase().contains("moment"),
        "got: {s}"
    );
}

#[test]
fn service_unavailable_message_suggests_try_again() {
    let s = render_inference_error(&InferenceError::ServiceUnavailable, &Locale::English);
    assert!(s.to_lowercase().contains("try again"), "got: {s}");
}

#[test]
fn network_unavailable_message_mentions_network() {
    let s = render_inference_error(&InferenceError::NetworkUnavailable, &Locale::English);
    assert!(
        s.to_lowercase().contains("network") || s.to_lowercase().contains("connect"),
        "got: {s}"
    );
}

#[test]
fn model_not_found_includes_model_name_and_pull_hint() {
    let s = render_inference_error(
        &InferenceError::ModelNotFound {
            model: "llama3.2".into(),
        },
        &Locale::English,
    );
    assert!(s.contains("llama3.2"), "got: {s}");
    assert!(s.to_lowercase().contains("pull"), "got: {s}");
}

#[test]
fn other_does_not_leak_inner_dev_string() {
    let s = render_inference_error(
        &InferenceError::Other("RAW_DEV_STRING_FOO".into()),
        &Locale::English,
    );
    assert!(
        !s.contains("RAW_DEV_STRING_FOO"),
        "Other's inner string must not reach users; got: {s}"
    );
}
