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

/// User-facing locale.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Locale {
    #[default]
    English,
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
