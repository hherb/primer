//! User-facing message rendering.
//!
//! This module is the single i18n boundary for the inference-error
//! surface. Variant authors keep `InferenceError` fields locale-neutral
//! (durations, model-name strings, status data); this function
//! translates them into a human-readable message for the configured
//! `Locale`.
//!
//! Phase 0.1 ships only `Locale::English`. Adding locales is a future
//! PR: append variants here and add corresponding match arms in
//! `render_inference_error` (or replace the body with a fluent / gettext
//! lookup). The function signature is the stable contract.
//!
//! **i18n contract (load-bearing — do not violate):**
//!   - This is the ONLY place user-facing English appears in the
//!     inference error path. Producers (`classify_*`) emit data; this
//!     function translates data to text.
//!   - The `Other` variant is dev-facing — its inner string is NEVER
//!     rendered to users. We surface a generic localized message and
//!     route the inner string to `tracing::warn!` at the call site.
//!
//! **Producer contract:**
//!   - Each variant of `InferenceError` has a documented producer (cloud
//!     backend, Ollama backend, transport layer, etc.) — see the variant
//!     doc-comments in `error.rs`. The English messages here are tuned
//!     to those producers (e.g. `ModelNotFound`'s message recommends
//!     `ollama pull X` because Ollama is the only producer today). If a
//!     future change adds a NEW producer for an existing variant,
//!     update the corresponding message here so the suggestion still
//!     makes sense.

use crate::error::InferenceError;
use serde::{Deserialize, Serialize};

/// User-facing locale.
///
/// Closed enum: a new variant is the single source of truth for that
/// language, and corresponding additions cascade to the prompt-pack
/// dispatch (`primer-pedagogy::prompt_pack`), the `render_inference_error`
/// match below, and `Locale::ALL` / `from_pack_id`. The lookup tables in
/// SQLite (`learners.locale` etc.) round-trip via `pack_id()` —
/// retired pack ids never get reused.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Locale {
    #[default]
    English,
    German,
    /// Preview locale — present in the enum, accessible via `from_pack_id("hi")`,
    /// but deliberately excluded from `Locale::ALL` until a native speaker
    /// reviews the machine-translated prompt pack. CLI/GUI pickers iterate
    /// `Locale::ALL`, so end users never reach this until that review lands.
    /// See `docs/localisation/hi/README.md`.
    Hindi,
}

impl Locale {
    /// Every variant, in declaration order. Used by tests and by any
    /// startup path that needs to enumerate available locales.
    pub const ALL: &'static [Self] = &[Self::English, Self::German];

    /// Canonical machine-readable name. Stable identifier — do not
    /// rename. Mirrors the `name()` pattern on `EngagementState` and
    /// `UnderstandingDepth`.
    pub fn name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::German => "German",
            Self::Hindi => "Hindi",
        }
    }

    /// BCP 47 language tag for downstream services (Whisper, Piper,
    /// future TTS backends). Region-suffixed because most speech
    /// models accept region codes for accent selection.
    pub fn bcp47(self) -> &'static str {
        match self {
            Self::English => "en-US",
            Self::German => "de-DE",
            Self::Hindi => "hi-IN",
        }
    }

    /// Short pack id used for prompt-pack lookup and as the SQLite
    /// `learners.locale` / `concepts.concept_language_tag` column value.
    /// Stable identifier — never renamed, never reused for a different
    /// language.
    pub fn pack_id(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::German => "de",
            Self::Hindi => "hi",
        }
    }

    /// The locale's name written in its own language (an "endonym" or
    /// "autonym"). Used as the user-facing label in a locale picker so a
    /// German child sees "Deutsch", not "German". Distinct from
    /// [`Self::name`], which is the English exonym (stable machine id).
    pub fn endonym(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::German => "Deutsch",
            Self::Hindi => "हिन्दी",
        }
    }

    /// Parse a short pack id back to a `Locale`. Returns `None` for
    /// unknown ids (caller decides whether that's a hard error or a
    /// fall-back to `Locale::default()`).
    pub fn from_pack_id(s: &str) -> Option<Self> {
        match s {
            "en" => Some(Self::English),
            "de" => Some(Self::German),
            "hi" => Some(Self::Hindi),
            _ => None,
        }
    }
}

/// Translate an `InferenceError` into a user-facing message in the
/// requested `Locale`. See module docs for the i18n contract.
pub fn render_inference_error(err: &InferenceError, locale: &Locale) -> String {
    match locale {
        Locale::English => render_english(err),
        Locale::German => render_german(err),
        Locale::Hindi => render_hindi(err),
    }
}

fn render_english(err: &InferenceError) -> String {
    use InferenceError::*;
    match err {
        Auth => {
            "Authentication failed. Please check your ANTHROPIC_API_KEY in .env or ~/.primer_env."
                .into()
        }
        RateLimited {
            retry_after: Some(d),
        } => {
            let secs = d.as_secs();
            if secs == 1 {
                "The service is busy right now. Please try again in 1 second.".into()
            } else {
                format!("The service is busy right now. Please try again in {secs} seconds.")
            }
        }
        RateLimited { retry_after: None } => {
            "The service is busy right now. Please try again in a moment.".into()
        }
        ServiceUnavailable => {
            "The service is temporarily unavailable. Please try again in a moment.".into()
        }
        NetworkUnavailable => {
            "Cannot reach the service. Please check your network connection.".into()
        }
        ModelNotFound { model } => {
            format!("Model '{model}' is not available. For Ollama, run `ollama pull {model}`.")
        }
        ReasoningWithoutAnswer => {
            "Oops — I'm having a thinking problem right now. Could you ask me that again?".into()
        }
        Other(_) => {
            "Something unexpected went wrong. Please try again. (See logs for details.)".into()
        }
    }
}

/// German rendering. Uses informal "du" — the Primer is a children's
/// product and German children are universally addressed as "du" by
/// adults outside formal institutional contexts. Singular/plural
/// agreement is handled explicitly for the rate-limited case
/// (1 Sekunde vs N Sekunden).
fn render_german(err: &InferenceError) -> String {
    use InferenceError::*;
    match err {
        Auth => {
            "Authentifizierung fehlgeschlagen. Bitte überprüfe deinen ANTHROPIC_API_KEY in .env oder ~/.primer_env."
                .into()
        }
        RateLimited {
            retry_after: Some(d),
        } => {
            let secs = d.as_secs();
            if secs == 1 {
                "Der Dienst ist gerade beschäftigt. Bitte versuche es in 1 Sekunde wieder.".into()
            } else {
                format!(
                    "Der Dienst ist gerade beschäftigt. Bitte versuche es in {secs} Sekunden wieder."
                )
            }
        }
        RateLimited { retry_after: None } => {
            "Der Dienst ist gerade beschäftigt. Bitte versuche es gleich wieder.".into()
        }
        ServiceUnavailable => {
            "Der Dienst ist vorübergehend nicht verfügbar. Bitte versuche es gleich wieder.".into()
        }
        NetworkUnavailable => {
            "Der Dienst ist nicht erreichbar. Bitte überprüfe deine Netzwerkverbindung.".into()
        }
        ModelNotFound { model } => {
            format!("Modell '{model}' ist nicht verfügbar. Für Ollama führe `ollama pull {model}` aus.")
        }
        ReasoningWithoutAnswer => {
            "Hoppla — ich komme gerade beim Nachdenken durcheinander. Kannst du mich das \
             noch einmal fragen?"
                .into()
        }
        Other(_) => "Etwas Unerwartetes ist schiefgelaufen. Bitte versuche es erneut. (Details in den Logs.)".into(),
    }
}

/// Hindi rendering. Uses the informal `तुम` register — consistent with
/// the hi.toml prompt-pack precedent and the broader children's-product
/// convention of avoiding the formal `आप`. Marked preview-quality: this
/// content is machine-translated, awaiting native-speaker review.
fn render_hindi(err: &InferenceError) -> String {
    use InferenceError::*;
    match err {
        Auth => {
            "प्रमाणीकरण विफल। कृपया अपने .env या ~/.primer_env में ANTHROPIC_API_KEY की जाँच करो।".into()
        }
        RateLimited {
            retry_after: Some(d),
        } => {
            let secs = d.as_secs();
            if secs == 1 {
                "सेवा अभी व्यस्त है। कृपया 1 सेकंड बाद दोबारा कोशिश करो।".into()
            } else {
                format!("सेवा अभी व्यस्त है। कृपया {secs} सेकंड बाद दोबारा कोशिश करो।")
            }
        }
        RateLimited { retry_after: None } => {
            "सेवा अभी व्यस्त है। कृपया थोड़ी देर बाद दोबारा कोशिश करो।".into()
        }
        ServiceUnavailable => {
            "सेवा अस्थायी रूप से उपलब्ध नहीं है। कृपया थोड़ी देर बाद दोबारा कोशिश करो।".into()
        }
        NetworkUnavailable => "सेवा तक पहुँचा नहीं जा सका। कृपया अपना नेटवर्क कनेक्शन जाँचो।".into(),
        ModelNotFound { model } => {
            format!("मॉडल '{model}' उपलब्ध नहीं है। Ollama के लिए `ollama pull {model}` चलाओ।")
        }
        ReasoningWithoutAnswer => {
            "अरे — मुझे अभी सोचने में थोड़ी दिक्कत हो रही है। क्या तुम मुझसे यह दोबारा पूछ सकते हो?".into()
        }
        Other(_) => "कुछ अप्रत्याशित हुआ। कृपया फिर से कोशिश करो। (विवरण लॉग में हैं।)".into(),
    }
}

#[cfg(test)]
mod tests {
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
}
