//! Main inference-backend construction: the `--backend` dispatch matrix,
//! the opt-in local→cloud fallback, the Phase 1.3 router, and the
//! feature-gated QNN / llama.cpp constructors.
//!
//! Split out of the flat `wiring` module (behaviour-preserving) so each
//! responsibility lives in its own file. `BackendParams` and the QAIRT
//! path helpers stay in the parent `wiring` module; this file borrows
//! them via `super::`.

use std::sync::Arc;

use primer_core::error::{PrimerError, Result};
use primer_core::inference::InferenceBackend;
use primer_inference::stub::StubBackend;

use super::BackendParams;

/// Outcome of the main-backend construction decision. See the truth table
/// in docs/superpowers/specs/2026-06-05-local-cloud-fallback-design.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainBackendPlan {
    /// Use the primary backend alone (no fallback, or fallback unbuildable).
    PrimaryAlone,
    /// Primary failed to build; use the secondary alone (startup fallback).
    SecondaryAlone,
    /// Both built and a fallback is configured; wrap them in `FallbackBackend`.
    Wrapped,
    /// Nothing usable; surface the primary's construction error.
    Fail,
}

/// A non-fatal degradation of the configured fallback / router legs,
/// returned by [`build_main_backend`] so a UI can tell the user instead of
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
        }
    }
}

/// Pure decision: given whether each leg built and whether a fallback was
/// configured, choose how to assemble the main backend. No I/O.
pub fn plan_main_backend(
    primary_built: bool,
    fallback_configured: bool,
    secondary_built: bool,
) -> MainBackendPlan {
    match (primary_built, fallback_configured, secondary_built) {
        (true, false, _) => MainBackendPlan::PrimaryAlone,
        (true, true, true) => MainBackendPlan::Wrapped,
        (true, true, false) => MainBackendPlan::PrimaryAlone,
        (false, true, true) => MainBackendPlan::SecondaryAlone,
        (false, true, false) => MainBackendPlan::Fail,
        (false, false, _) => MainBackendPlan::Fail,
    }
}

/// Resolve the fallback secondary's model from its backend name + optional
/// explicit `--fallback-model`. Pure — no I/O.
///
/// - `stub` ⇒ `Ok(None)` (model ignored by the stub backend).
/// - `cloud` ⇒ `Ok(Some(model | DEFAULT_CLOUD_MODEL))`.
/// - `ollama` / `openai-compat` ⇒ model required ⇒ `Err` when `None`.
/// - any other name ⇒ `Ok(model)` passthrough (`build_backend` rejects the
///   unknown name later with its own clear error).
pub fn resolve_fallback_model(
    backend: &str,
    model: Option<String>,
) -> std::result::Result<Option<String>, String> {
    match backend {
        "stub" => Ok(None),
        "cloud" => Ok(Some(model.unwrap_or_else(|| {
            primer_core::consts::inference::DEFAULT_CLOUD_MODEL.to_string()
        }))),
        "ollama" | "openai-compat" => model.map(Some).ok_or_else(|| {
            format!("--fallback-model is required when --fallback-backend is {backend}")
        }),
        _ => Ok(model),
    }
}

/// Construct an `InferenceBackend` of the named type with the given model.
///
/// All three backend variants are synchronous at construction time; the
/// function signature is `async` only for uniformity with `build_classifier`.
pub async fn build_backend(
    backend_name: &str,
    model: String,
    params: &BackendParams,
) -> Result<Arc<dyn InferenceBackend>> {
    match backend_name {
        "stub" => Ok(Arc::new(StubBackend)),
        "cloud" => {
            let api_key = params.api_key.clone().ok_or_else(|| {
                PrimerError::Inference(
                    "--api-key or ANTHROPIC_API_KEY required for cloud backend".into(),
                )
            })?;
            Ok(Arc::new(primer_inference::cloud::CloudBackend::new(
                "https://api.anthropic.com".to_string(),
                api_key,
                model,
            )))
        }
        "ollama" => Ok(Arc::new(
            primer_inference::ollama::OllamaBackend::new(params.ollama_url.clone(), model)
                .with_extra_markers(params.reasoning_markers.clone()),
        )),
        "openai-compat" => Ok(Arc::new(
            primer_inference::openai_compat::OpenAiCompatBackend::new(
                params.openai_compat_url.clone(),
                model,
                params.openai_compat_api_key.clone(),
            )
            .with_extra_markers(params.reasoning_markers.clone()),
        )),
        "qnn" => build_qnn_backend(params).await,
        "llamacpp" => build_llamacpp_backend(params).await,
        other => Err(PrimerError::Inference(
            format!("unknown backend: {other}").into(),
        )),
    }
}

/// Build the secondary leg named `fb_name`. A model-resolve error becomes the
/// leg's `Err` (never `?`-propagated: a broken opt-in fallback must not abort
/// a healthy primary). Also returns the resolved model for [`BackendNotice`].
async fn build_secondary_leg(
    fb_name: &str,
    params: &BackendParams,
) -> (Result<Arc<dyn InferenceBackend>>, Option<String>) {
    match resolve_fallback_model(fb_name, params.fallback_model.clone()) {
        Ok(fb_model) => (
            build_backend(fb_name, fb_model.clone().unwrap_or_default(), params).await,
            fb_model,
        ),
        Err(msg) => (Err(PrimerError::Inference(msg.into())), None),
    }
}

/// The notice for a degraded plan; `None` for `Wrapped` / `Fail`. `failed`
/// is the error of whichever leg did not build.
fn degraded_notice(
    plan: MainBackendPlan,
    primary_name: &str,
    fb_name: &str,
    fb_model: Option<String>,
    failed: Option<&PrimerError>,
) -> Option<BackendNotice> {
    let reason = failed.map(ToString::to_string).unwrap_or_default();
    match plan {
        MainBackendPlan::PrimaryAlone => Some(BackendNotice::FallbackDropped {
            fallback: fb_name.to_string(),
            reason,
        }),
        MainBackendPlan::SecondaryAlone => Some(BackendNotice::PrimaryUnavailable {
            primary: primary_name.to_string(),
            fallback: fb_name.to_string(),
            fallback_model: fb_model,
            reason,
        }),
        MainBackendPlan::Wrapped | MainBackendPlan::Fail => None,
    }
}

/// Construct the main inference backend, applying the opt-in construction
/// fallback. When `params.fallback_backend` is `None`, this is exactly
/// `build_backend(primary_name, primary_model, params)`.
///
/// Otherwise it builds both legs and applies [`plan_main_backend`]:
/// - `Wrapped` ⇒ `FallbackBackend { primary, secondary }`;
/// - `SecondaryAlone` ⇒ secondary alone (primary was unavailable at startup);
/// - `PrimaryAlone` ⇒ primary alone (no fallback, or the fallback was
///   unbuildable/misconfigured — a broken opt-in fallback never aborts a
///   healthy primary);
/// - `Fail` ⇒ the primary's construction error.
///
/// The second tuple element reports a `PrimaryAlone` / `SecondaryAlone`
/// degradation of a configured fallback (or router) as a [`BackendNotice`];
/// it is `None` when nothing was configured or both legs built.
pub async fn build_main_backend(
    primary_name: &str,
    primary_model: String,
    params: &BackendParams,
) -> Result<(Arc<dyn InferenceBackend>, Option<BackendNotice>)> {
    use primer_inference::FallbackBackend;

    // Routing modes build a RouterBackend over the same two legs the fallback
    // uses; LocalOnly falls through to today's primary/fallback logic verbatim.
    if params.router_mode.uses_secondary() {
        return build_router_backend(primary_name, primary_model, params).await;
    }

    // Warn on a pointless same-backend fallback (still works, just no resilience).
    if let Some(fb) = params.fallback_backend.as_deref() {
        if fb == primary_name {
            tracing::warn!(
                backend = primary_name,
                "--fallback-backend equals --backend; no resilience gain"
            );
        }
    }

    let primary = build_backend(primary_name, primary_model, params).await;

    let Some(fb_name) = params.fallback_backend.clone() else {
        // No fallback configured: today's behavior verbatim.
        return primary.map(|b| (b, None));
    };

    // A fallback IS configured. When the primary ALSO failed,
    // `plan_main_backend` returns `Fail` and the primary's (more actionable)
    // error surfaces.
    let (secondary, fb_model) = build_secondary_leg(&fb_name, params).await;

    let plan = plan_main_backend(primary.is_ok(), true, secondary.is_ok());
    match plan {
        MainBackendPlan::Wrapped => {
            // Both built — wrap. expect() is safe: plan returned Wrapped only
            // because both is_ok() were true.
            let primary = primary.expect("Wrapped implies primary built");
            let secondary = secondary.expect("Wrapped implies secondary built");
            Ok((Arc::new(FallbackBackend::new(primary, secondary)), None))
        }
        MainBackendPlan::SecondaryAlone => {
            tracing::warn!(
                primary = primary_name,
                secondary = %fb_name,
                "primary backend unavailable at startup; using fallback backend alone"
            );
            let notice = degraded_notice(
                plan,
                primary_name,
                &fb_name,
                fb_model,
                primary.err().as_ref(),
            );
            secondary.map(|b| (b, notice))
        }
        MainBackendPlan::PrimaryAlone => {
            // Reached only when primary built but the secondary did not — either
            // unbuildable or misconfigured (e.g. ollama fallback without a
            // model). The broken opt-in fallback is dropped, not fatal.
            if let Err(ref e) = secondary {
                tracing::warn!(
                    secondary = %fb_name,
                    error = %e,
                    "fallback backend unusable (misconfigured or failed to construct); using primary alone"
                );
            }
            let notice = degraded_notice(
                plan,
                primary_name,
                &fb_name,
                fb_model,
                secondary.err().as_ref(),
            );
            primary.map(|b| (b, notice))
        }
        MainBackendPlan::Fail => {
            // Both failed: surface the primary's (most informative) error.
            primary.map(|b| (b, None))
        }
    }
}

/// Construct a `RouterBackend` for a non-`LocalOnly` mode. Requires a
/// configured secondary leg (`--fallback-backend`); both legs are built and
/// wrapped. Degrades gracefully if exactly one leg fails to build (mirrors
/// `plan_main_backend`): primary-only (routing disabled, warn) or
/// secondary-only (warn); both-fail surfaces the primary's error.
async fn build_router_backend(
    primary_name: &str,
    primary_model: String,
    params: &BackendParams,
) -> Result<(Arc<dyn InferenceBackend>, Option<BackendNotice>)> {
    use primer_inference::RouterBackend;

    let Some(fb_name) = params.fallback_backend.clone() else {
        return Err(PrimerError::Inference(
            format!(
                "router mode '{}' requires a secondary leg; set --fallback-backend \
                 (and --fallback-model where required)",
                params.router_mode
            )
            .into(),
        ));
    };

    let primary = build_backend(primary_name, primary_model, params).await;
    let (secondary, fb_model) = build_secondary_leg(&fb_name, params).await;

    let plan = plan_main_backend(primary.is_ok(), true, secondary.is_ok());
    match plan {
        MainBackendPlan::Wrapped => {
            let primary = primary.expect("Wrapped implies primary built");
            let secondary = secondary.expect("Wrapped implies secondary built");
            let router = RouterBackend::with_ttft_budget(
                primary,
                secondary,
                params.router_mode,
                params.primary_ttft_budget_ms,
            );
            Ok((Arc::new(router), None))
        }
        MainBackendPlan::SecondaryAlone => {
            tracing::warn!(
                primary = primary_name,
                secondary = %fb_name,
                "router: primary unavailable at startup; using secondary alone (no routing)"
            );
            let notice = degraded_notice(
                plan,
                primary_name,
                &fb_name,
                fb_model,
                primary.err().as_ref(),
            );
            secondary.map(|b| (b, notice))
        }
        MainBackendPlan::PrimaryAlone => {
            if let Err(ref e) = secondary {
                tracing::warn!(
                    secondary = %fb_name,
                    error = %e,
                    "router: secondary unusable; using primary alone (routing disabled)"
                );
            }
            let notice = degraded_notice(
                plan,
                primary_name,
                &fb_name,
                fb_model,
                secondary.err().as_ref(),
            );
            primary.map(|b| (b, notice))
        }
        // Both legs failed: surface the primary's (most informative) error.
        MainBackendPlan::Fail => primary.map(|b| (b, None)),
    }
}

/// Construct the QNN backend from [`BackendParams`].
///
/// Behind the `qnn` cargo feature this validates the required
/// `qnn_bundle_dir`, falls back to [`super::default_qairt_lib_dir`] when
/// `qnn_qairt_lib_dir` is unset, and delegates to
/// [`primer_inference::QnnBackend::new`] — which itself returns
/// `InferenceError::Other("...PlatformUnsupported...")` on non-Android
/// hosts, so the dispatch arm fires cleanly even when running on a
/// developer laptop.
///
/// Without the feature, returns a clear "rebuild with `--features qnn`"
/// error so the CLI's "unknown backend"-style hint stays one diagnostic
/// per audience: build-time vs. runtime.
///
/// Kept as a free function (not inlined into the match arm) so the
/// cfg-gating is one shape, not two; the no-feature branch becomes a
/// dead-simple one-liner and the qnn-feature branch carries all the
/// validation logic.
#[cfg(feature = "qnn")]
async fn build_qnn_backend(params: &BackendParams) -> Result<Arc<dyn InferenceBackend>> {
    let bundle_dir = params.qnn_bundle_dir.as_ref().ok_or_else(|| {
        PrimerError::Inference(
            "--qnn-bundle-dir is required for --backend qnn \
             (or set PRIMER_QNN_BUNDLE_DIR)"
                .into(),
        )
    })?;
    let qairt_lib_dir = super::resolve_qairt_lib_dir(params.qnn_qairt_lib_dir.clone(), bundle_dir);
    let backend = primer_inference::QnnBackend::new(bundle_dir.clone(), qairt_lib_dir).await?;
    Ok(Arc::new(backend))
}

#[cfg(not(feature = "qnn"))]
async fn build_qnn_backend(_params: &BackendParams) -> Result<Arc<dyn InferenceBackend>> {
    Err(PrimerError::Inference(
        "qnn backend requires the `qnn` cargo feature. \
         Build with `cargo build --features primer-cli/qnn` (Android target only)."
            .into(),
    ))
}

/// Construct the llama.cpp backend. Mirrors [`build_qnn_backend`]'s two-cfg
/// shape: feature-on validates the GGUF path and constructs the real engine;
/// feature-off returns a distinct build-time hint.
#[cfg(feature = "llamacpp")]
async fn build_llamacpp_backend(params: &BackendParams) -> Result<Arc<dyn InferenceBackend>> {
    use primer_inference::llamacpp::params::resolve_gpu_layers;
    let gguf_path = params.gguf_path.as_ref().ok_or_else(|| {
        PrimerError::Inference("--model <path-to.gguf> is required for --backend llamacpp".into())
    })?;
    let n_gpu_layers = resolve_gpu_layers(params.llamacpp_gpu_layers);
    let engine = primer_inference::llamacpp::engine::RealLlamaEngine::new(
        gguf_path,
        n_gpu_layers,
        params.llamacpp_n_ctx,
    )?;
    let backend = primer_inference::LlamaCppBackend::new(Arc::new(engine))
        .with_extra_markers(params.reasoning_markers.clone());
    Ok(Arc::new(backend))
}

#[cfg(not(feature = "llamacpp"))]
async fn build_llamacpp_backend(_params: &BackendParams) -> Result<Arc<dyn InferenceBackend>> {
    Err(PrimerError::Inference(
        "llamacpp backend requires the `llamacpp` cargo feature. \
         Build with `cargo build --features primer-cli/llamacpp` \
         (or llamacpp-metal / llamacpp-cuda / llamacpp-vulkan for GPU)."
            .into(),
    ))
}
