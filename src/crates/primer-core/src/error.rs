//! Unified error types for the Primer system.

use std::time::Duration;
use thiserror::Error;

/// Categorized inference failure. Variant fields are language-neutral
/// data (durations, model names, status data) — never embed user-facing
/// English here. The `Display` impl produces *dev-facing* strings used
/// by `tracing::warn!`, panic messages, and test output. End-user
/// rendering will route through a separate i18n module added in a
/// follow-up commit.
// `Clone` is required by the retry helper (lands in a follow-up commit)
// for tracing event payloads that outlive the original Result.
#[derive(Debug, Clone, Error)]
pub enum InferenceError {
    /// 401 / 403 from a cloud API or other auth-shape failure.
    /// User remedy: check the API key.
    #[error("authentication failed")]
    Auth,

    /// 429 with optional Retry-After. The retry helper respects
    /// `retry_after` up to `RetrySettings.retry_after_budget`; longer
    /// waits are surfaced unchanged for the user to retry manually.
    #[error("rate limited (retry_after={retry_after:?})")]
    RateLimited { retry_after: Option<Duration> },

    /// 5xx from cloud, or daemon-down from Ollama.
    #[error("service unavailable")]
    ServiceUnavailable,

    /// Connect / timeout / DNS / TLS failure at the transport layer.
    #[error("network unavailable")]
    NetworkUnavailable,

    /// Ollama returned 404 for a configured model name. Not produced
    /// by the cloud backend (its missing-model surface differs).
    #[error("model not found: {model}")]
    ModelNotFound { model: String },

    /// Catch-all for unmapped conditions. Dev-facing only — the user
    /// never sees the inner string. Goes to `tracing::warn!` instead.
    #[error("{0}")]
    Other(String),
}

impl InferenceError {
    /// Whether the retry helper should attempt to recover this
    /// condition. `Auth`, `ModelNotFound`, and `Other` are surfaced
    /// immediately; the rest are transient by definition or by
    /// observation (network flap).
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. } | Self::ServiceUnavailable | Self::NetworkUnavailable,
        )
    }
}

impl From<&str> for InferenceError {
    fn from(s: &str) -> Self {
        Self::Other(s.to_string())
    }
}

impl From<String> for InferenceError {
    fn from(s: String) -> Self {
        Self::Other(s)
    }
}

#[derive(Error, Debug)]
pub enum PrimerError {
    #[error("Inference error: {0}")]
    Inference(InferenceError),

    #[error("Speech error: {0}")]
    Speech(String),

    #[error("Knowledge base error: {0}")]
    Knowledge(String),

    #[error("Learner model error: {0}")]
    LearnerModel(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Classification error: {0}")]
    Classification(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialisation error: {0}")]
    Serde(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, PrimerError>;

#[cfg(test)]
mod inference_error_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn is_retryable_truth_table() {
        assert!(!InferenceError::Auth.is_retryable());
        assert!(InferenceError::RateLimited { retry_after: None }.is_retryable());
        assert!(
            InferenceError::RateLimited {
                retry_after: Some(Duration::from_secs(1))
            }
            .is_retryable()
        );
        assert!(InferenceError::ServiceUnavailable.is_retryable());
        assert!(InferenceError::NetworkUnavailable.is_retryable());
        assert!(!InferenceError::ModelNotFound { model: "x".into() }.is_retryable());
        assert!(!InferenceError::Other("anything".into()).is_retryable());
    }

    #[test]
    fn from_string_lands_in_other() {
        let s: String = "boom".into();
        let e: InferenceError = s.into();
        assert!(matches!(e, InferenceError::Other(ref msg) if msg == "boom"));
    }

    #[test]
    fn from_str_lands_in_other() {
        let e: InferenceError = "boom".into();
        assert!(matches!(e, InferenceError::Other(ref msg) if msg == "boom"));
    }

    #[test]
    fn display_produces_dev_facing_english() {
        assert_eq!(format!("{}", InferenceError::Auth), "authentication failed");
        assert_eq!(
            format!("{}", InferenceError::ServiceUnavailable),
            "service unavailable"
        );
        assert_eq!(
            format!("{}", InferenceError::NetworkUnavailable),
            "network unavailable"
        );
        assert_eq!(
            format!(
                "{}",
                InferenceError::ModelNotFound {
                    model: "llama3.2".into()
                }
            ),
            "model not found: llama3.2"
        );
        assert_eq!(format!("{}", InferenceError::Other("raw".into())), "raw");
        assert_eq!(
            format!("{}", InferenceError::RateLimited { retry_after: None }),
            "rate limited (retry_after=None)"
        );
        assert_eq!(
            format!(
                "{}",
                InferenceError::RateLimited {
                    retry_after: Some(Duration::from_secs(2))
                }
            ),
            "rate limited (retry_after=Some(2s))"
        );
    }
}
