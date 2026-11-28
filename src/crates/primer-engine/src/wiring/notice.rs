//! The [`BackendNotice`] a main-backend build reports, and the
//! [`served_identity`] it implies. Split out of `backend.rs` to keep that
//! file under the size guideline (behaviour-preserving; #352 item 2 added
//! the `FallbackIdentical` variant).

/// A non-fatal degradation of the configured fallback / router legs,
/// returned by [`build_main_backend`](super::build_main_backend) so a UI can tell the user instead of
/// leaving it in a `tracing::warn!` nobody sees (issues #347, #338).
/// `reason` is the failed leg's construction error, rendered verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendNotice {
    /// The configured fallback could not be built; the primary serves alone,
    /// so there is no fallback if it fails mid-session (plan `PrimaryAlone`).
    FallbackDropped { fallback: String, reason: String },
    /// The primary could not be built; every turn goes to the fallback
    /// (plan `SecondaryAlone`). `fallback_model` is its resolved model.
    PrimaryUnavailable {
        primary: String,
        fallback: String,
        fallback_model: Option<String>,
        reason: String,
    },
    /// Both legs built, but the fallback is the same backend and model as
    /// the primary, so it adds no protection: whatever takes the primary
    /// down takes the fallback down too (#352 item 2). The session runs
    /// unprotected. `fallback_model` is the resolved model (`None` for a
    /// backend that ignores it).
    FallbackIdentical {
        fallback: String,
        fallback_model: Option<String>,
    },
}

impl std::fmt::Display for BackendNotice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FallbackDropped { fallback, reason } => write!(
                f,
                "Fallback backend '{fallback}' is disabled for this session: {reason}"
            ),
            Self::PrimaryUnavailable {
                primary,
                fallback,
                reason,
                ..
            } => write!(
                f,
                "Backend '{primary}' is unavailable, so this session uses the \
                 fallback backend '{fallback}': {reason}"
            ),
            Self::FallbackIdentical {
                fallback,
                fallback_model,
            } => {
                let model = fallback_model
                    .as_deref()
                    .map(|m| format!(" with model '{m}'"))
                    .unwrap_or_default();
                write!(
                    f,
                    "Fallback backend '{fallback}'{model} is the same as the main backend, \
                     so it gives no protection if that fails. Choose a different fallback \
                     backend or model (`--fallback-backend` / `--fallback-model`, or \
                     Settings → Inference backend → Fallback backend / Fallback model)."
                )
            }
        }
    }
}

/// The `(backend kind, model)` that actually serves turns. Normally the
/// configured primary; but under [`BackendNotice::PrimaryUnavailable`] every
/// turn goes to the fallback, so return that instead. Both binaries feed it
/// to the subsystem builders (whose ids are provenance records, issue #351)
/// and the GUI sidebar shows it (issue #338). `served_name` is the
/// constructed backend's `name()`, used when the fallback has no model.
pub fn served_identity(
    kind: &str,
    main_model: &str,
    notice: Option<&BackendNotice>,
    served_name: &str,
) -> (String, String) {
    match notice {
        Some(BackendNotice::PrimaryUnavailable {
            fallback,
            fallback_model,
            ..
        }) => (
            fallback.clone(),
            fallback_model
                .clone()
                .unwrap_or_else(|| served_name.to_string()),
        ),
        _ => (kind.to_string(), main_model.to_string()),
    }
}
