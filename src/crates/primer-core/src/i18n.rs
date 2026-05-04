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
}

impl Locale {
    /// Every variant, in declaration order. Used by tests and by any
    /// startup path that needs to enumerate available locales.
    pub const ALL: &'static [Self] = &[Self::English];

    /// Canonical machine-readable name. Stable identifier — do not
    /// rename. Mirrors the `name()` pattern on `EngagementState` and
    /// `UnderstandingDepth`.
    pub fn name(self) -> &'static str {
        match self {
            Self::English => "English",
        }
    }

    /// BCP 47 language tag for downstream services (Whisper, Piper,
    /// future TTS backends). Region-suffixed because most speech
    /// models accept region codes for accent selection.
    pub fn bcp47(self) -> &'static str {
        match self {
            Self::English => "en-US",
        }
    }

    /// Short pack id used for prompt-pack lookup and as the SQLite
    /// `learners.locale` / `concepts.concept_language_tag` column value.
    /// Stable identifier — never renamed, never reused for a different
    /// language.
    pub fn pack_id(self) -> &'static str {
        match self {
            Self::English => "en",
        }
    }

    /// Parse a short pack id back to a `Locale`. Returns `None` for
    /// unknown ids (caller decides whether that's a hard error or a
    /// fall-back to `Locale::default()`).
    pub fn from_pack_id(s: &str) -> Option<Self> {
        match s {
            "en" => Some(Self::English),
            _ => None,
        }
    }
}

/// Translate an `InferenceError` into a user-facing message in the
/// requested `Locale`. See module docs for the i18n contract.
pub fn render_inference_error(err: &InferenceError, locale: &Locale) -> String {
    match locale {
        Locale::English => render_english(err),
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
        Other(_) => {
            "Something unexpected went wrong. Please try again. (See logs for details.)".into()
        }
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
    fn locale_all_lists_every_variant() {
        assert_eq!(Locale::ALL.len(), 1);
        assert!(Locale::ALL.contains(&Locale::English));
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
