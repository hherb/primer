//! Backend / classifier / extractor / comprehension / embedder
//! construction matrix shared between binaries.
//!
//! The dispatch logic for "main backend + per-subsystem override"
//! lives here so `primer-cli` and `primer-gui` produce identical
//! wiring from identical inputs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use primer_classifier::{
    ClassifierSettings, EngagementClassifier, LlmEngagementClassifier, StubEngagementClassifier,
};
use primer_core::error::{PrimerError, Result};
use primer_core::inference::InferenceBackend;
use primer_extractor::{
    ConceptExtractor, ExtractorSettings, LlmConceptExtractor, StubConceptExtractor,
};
use primer_inference::stub::StubBackend;

/// Parameters needed by `build_backend` and the per-subsystem builders
/// that would otherwise require borrowing from the binary's `Cli` /
/// settings struct. Extracted so the helpers can be called after
/// partial moves of the source-of-truth fields.
///
/// **Invariant:** every backend-affecting CLI flag in `primer-cli` (and
/// the eventual `primer-gui` equivalent) must round-trip through this
/// struct. Adding a new flag means adding a field here AND threading it
/// in at the construction site — silent omission would let the binary
/// see a flag the wiring helpers ignore.
pub struct BackendParams {
    pub api_key: Option<String>,
    pub ollama_url: String,
    pub openai_compat_url: String,
    pub openai_compat_api_key: Option<String>,
    pub classifier_backend: Option<String>,
    pub classifier_model: Option<String>,
    pub extractor_backend: Option<String>,
    pub extractor_model: Option<String>,
    pub comprehension_backend: Option<String>,
    pub comprehension_model: Option<String>,
    /// QNN bundle directory (Phase 1.2 step 1.2.4). Contains
    /// `genie_config.json`, `primer-meta.json`, and the per-shard
    /// context binaries. Consumed by the `"qnn"` arm of
    /// [`build_backend`]; ignored by every other arm. Always present
    /// in the struct (not `#[cfg(feature = "qnn")]`-gated) so the
    /// struct shape stays identical across feature combinations —
    /// the qnn arm itself is the only feature-gated piece.
    pub qnn_bundle_dir: Option<PathBuf>,
    /// QNN QAIRT library directory (containing `libGenie.so` +
    /// dependencies). Consumed by the `"qnn"` arm only. Callers can
    /// resolve a sensible default with [`default_qairt_lib_dir`] from
    /// a known bundle dir; the CLI surfaces this as the optional
    /// `--qnn-qairt-lib-dir` flag with the same default applied when
    /// unset.
    pub qnn_qairt_lib_dir: Option<PathBuf>,
    /// Path to the GGUF model file. Consumed by the `"llamacpp"` arm of
    /// [`build_backend`] only; ignored by every other arm. Always present
    /// in the struct (qnn-style) so the shape is identical across feature
    /// combinations. The CLI surfaces this by reusing `--model`.
    pub gguf_path: Option<PathBuf>,
    /// Explicit `n_gpu_layers` override for the llama.cpp backend
    /// (`--llamacpp-gpu-layers`). `None` ⇒ resolved by feature
    /// (`primer_inference::llamacpp::params::resolve_gpu_layers`).
    pub llamacpp_gpu_layers: Option<i32>,
    /// Explicit `n_ctx` override (`--llamacpp-n-ctx`). `None` ⇒ the model's
    /// trained default.
    pub llamacpp_n_ctx: Option<u32>,
    /// Extra `(open, close)` reasoning-marker pairs appended to the built-in
    /// defaults for the Ollama / openai-compat backends. Empty ⇒ defaults
    /// only. Ignored by every other backend arm.
    ///
    /// These markers propagate to any subsystem backend (classifier,
    /// extractor, comprehension) constructed through the same `params` via
    /// [`build_backend`]. That is intentional: the built-in defaults SHOULD
    /// strip reasoning from subsystem responses too (it keeps their JSON
    /// output clean for parsing), and a user-supplied custom pair applies
    /// everywhere rather than to the chat backend alone.
    pub reasoning_markers: Vec<(String, String)>,
    /// Opt-in fallback secondary backend name (`stub`/`cloud`/`ollama`/
    /// `openai-compat`). `None` ⇒ no fallback ⇒ local-only (the privacy
    /// default). Consumed only by [`build_main_backend`]; the flag's
    /// presence is the explicit cloud-fallback consent.
    pub fallback_backend: Option<String>,
    /// Model for the fallback secondary. Resolution rules live in
    /// [`resolve_fallback_model`]. `None` is valid (cloud defaults; stub
    /// ignores it; ollama/openai-compat error).
    pub fallback_model: Option<String>,
    /// Phase 1.3 inference-router mode. `LocalOnly` (the default) ⇒ today's
    /// behavior (primary or `FallbackBackend`). `Hybrid`/`CloudPreferred` ⇒
    /// `build_main_backend` produces a `RouterBackend` over the primary +
    /// secondary legs. Consumed only by [`build_main_backend`].
    pub router_mode: primer_core::router::RouterMode,
    /// Phase 1.3 latency-aware routing budget (ms). `None` ⇒ latency routing
    /// OFF (the default). When set AND `router_mode == Hybrid`, the
    /// `RouterBackend` nudges a turn toward the secondary when its rolling
    /// primary-leg TTFT EMA exceeds this budget. Consumed only by
    /// [`build_main_backend`]'s router path.
    pub primary_ttft_budget_ms: Option<u64>,
}

/// Conventional default location of the QAIRT runtime libraries
/// relative to a QNN bundle directory.
///
/// Pure helper — no filesystem access, no library load. Returns the
/// path `<bundle>/../qairt/lib/aarch64-android/` exactly as the QAIRT
/// SDK layout documents under "AI Hub apps" assets. Callers may pass
/// this directly to [`build_backend`] via
/// [`BackendParams::qnn_qairt_lib_dir`] when the user did not provide
/// an explicit override.
pub fn default_qairt_lib_dir(bundle_dir: &Path) -> PathBuf {
    bundle_dir
        .parent()
        .map(|parent| parent.join("qairt/lib/aarch64-android"))
        // Bundle paths are always absolute in practice (CLI clamps
        // to `PathBuf`), but tolerate the missing-parent case by
        // returning a same-directory-relative fallback rather than
        // panicking — the downstream `RealGenieLibrary::open` will
        // produce a clear `LibraryLoad` error.
        .unwrap_or_else(|| PathBuf::from("qairt/lib/aarch64-android"))
}

/// Resolve the QAIRT runtime-library directory to hand the QNN backend,
/// given the user's optional override and the bundle dir.
///
/// Platform-aware (delegates to the pure [`resolve_qairt_lib_dir_for`]):
///
/// - **Android:** the 9 QAIRT `.so`s ship inside the APK's
///   `lib/arm64-v8a/` and are extracted to the app's `nativeLibraryDir`,
///   which the system linker already searches. An absent override
///   therefore resolves to an **empty** path, signalling
///   [`primer_inference::QnnBackend`] to dlopen `libGenie.so` by
///   *basename* (so the linker finds it and its DT_NEEDED deps in
///   nativeLibraryDir). `qnn_qairt_lib_dir` is thus unnecessary on-device.
/// - **Desktop / non-Android:** an absent override falls back to the
///   conventional bundle-relative [`default_qairt_lib_dir`] layout.
///
/// An explicit override always wins on every platform.
pub fn resolve_qairt_lib_dir(explicit: Option<PathBuf>, bundle_dir: &Path) -> PathBuf {
    resolve_qairt_lib_dir_for(explicit, bundle_dir, cfg!(target_os = "android"))
}

/// Pure core of [`resolve_qairt_lib_dir`] with the platform decision
/// lifted to an explicit `is_android` parameter so both branches are
/// host-testable. See [`resolve_qairt_lib_dir`] for the rationale.
fn resolve_qairt_lib_dir_for(
    explicit: Option<PathBuf>,
    bundle_dir: &Path,
    is_android: bool,
) -> PathBuf {
    match explicit {
        Some(dir) => dir,
        None if is_android => PathBuf::new(),
        None => default_qairt_lib_dir(bundle_dir),
    }
}

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
pub async fn build_main_backend(
    primary_name: &str,
    primary_model: String,
    params: &BackendParams,
) -> Result<Arc<dyn InferenceBackend>> {
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
        return primary;
    };

    // A fallback IS configured. Resolve the secondary model (may fail for a
    // required-model backend like ollama/openai-compat) and try to build it.
    // A misconfigured or unbuildable *opt-in* fallback must NEVER abort startup
    // when the primary is healthy — it degrades to `PrimaryAlone` in the match
    // below — so a resolve error is folded into the `secondary` Result rather
    // than `?`-propagated. When the primary ALSO failed, `plan_main_backend`
    // returns `Fail` and the primary's (more actionable) error surfaces.
    let secondary = match resolve_fallback_model(&fb_name, params.fallback_model.clone()) {
        Ok(fb_model) => build_backend(&fb_name, fb_model.unwrap_or_default(), params).await,
        Err(msg) => Err(PrimerError::Inference(msg.into())),
    };

    match plan_main_backend(primary.is_ok(), true, secondary.is_ok()) {
        MainBackendPlan::Wrapped => {
            // Both built — wrap. expect() is safe: plan returned Wrapped only
            // because both is_ok() were true.
            let primary = primary.expect("Wrapped implies primary built");
            let secondary = secondary.expect("Wrapped implies secondary built");
            Ok(Arc::new(FallbackBackend::new(primary, secondary)))
        }
        MainBackendPlan::SecondaryAlone => {
            tracing::warn!(
                primary = primary_name,
                secondary = %fb_name,
                "primary backend unavailable at startup; using fallback backend alone"
            );
            secondary
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
            primary
        }
        MainBackendPlan::Fail => {
            // Both failed: surface the primary's (most informative) error.
            primary
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
) -> Result<Arc<dyn InferenceBackend>> {
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
    let secondary = match resolve_fallback_model(&fb_name, params.fallback_model.clone()) {
        Ok(fb_model) => build_backend(&fb_name, fb_model.unwrap_or_default(), params).await,
        Err(msg) => Err(PrimerError::Inference(msg.into())),
    };

    match plan_main_backend(primary.is_ok(), true, secondary.is_ok()) {
        MainBackendPlan::Wrapped => {
            let primary = primary.expect("Wrapped implies primary built");
            let secondary = secondary.expect("Wrapped implies secondary built");
            Ok(Arc::new(RouterBackend::with_ttft_budget(
                primary,
                secondary,
                params.router_mode,
                params.primary_ttft_budget_ms,
            )))
        }
        MainBackendPlan::SecondaryAlone => {
            tracing::warn!(
                primary = primary_name,
                secondary = %fb_name,
                "router: primary unavailable at startup; using secondary alone (no routing)"
            );
            secondary
        }
        MainBackendPlan::PrimaryAlone => {
            if let Err(ref e) = secondary {
                tracing::warn!(
                    secondary = %fb_name,
                    error = %e,
                    "router: secondary unusable; using primary alone (routing disabled)"
                );
            }
            primary
        }
        // Both legs failed: surface the primary's (most informative) error.
        MainBackendPlan::Fail => primary,
    }
}

/// Construct the QNN backend from [`BackendParams`].
///
/// Behind the `qnn` cargo feature this validates the required
/// `qnn_bundle_dir`, falls back to [`default_qairt_lib_dir`] when
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
    let qairt_lib_dir = resolve_qairt_lib_dir(params.qnn_qairt_lib_dir.clone(), bundle_dir);
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

/// Construct the engagement classifier according to the dispatch matrix:
///
/// | main backend | --classifier-backend | --classifier-model | outcome                               |
/// |--------------|----------------------|--------------------|---------------------------------------|
/// | stub         | (unset)              | (any)              | StubEngagementClassifier              |
/// | *            | "stub"               | (any)              | StubEngagementClassifier              |
/// | *            | some(X)              | None               | error (model required for non-stub)   |
/// | *            | some(X)              | some(M)            | LlmEngagementClassifier(new backend X, model M) |
/// | non-stub     | (unset)              | None               | LlmEngagementClassifier(Arc::clone main) |
/// | non-stub     | (unset)              | some(M)            | LlmEngagementClassifier(new backend same type, model M) |
pub async fn build_classifier(
    main_backend: Arc<dyn InferenceBackend>,
    main_backend_name: &str,
    main_model: &str,
    params: &BackendParams,
    settings: ClassifierSettings,
) -> Result<Arc<dyn EngagementClassifier>> {
    // Warn on a flag combination the user probably didn't mean: passing
    // --classifier-model alongside an explicit --classifier-backend stub
    // looks like the model name will take effect, but stub ignores it.
    if matches!(params.classifier_backend.as_deref(), Some("stub"))
        && params.classifier_model.is_some()
    {
        tracing::warn!(
            model = ?params.classifier_model,
            "--classifier-model ignored: --classifier-backend is stub (deterministic, no model)"
        );
    }

    match (main_backend_name, params.classifier_backend.as_deref()) {
        // Explicit stub override always wins regardless of main backend.
        (_, Some("stub")) => Ok(Arc::new(StubEngagementClassifier::new())),

        // Main backend is stub and no classifier backend specified → default to stub.
        // (LlmEngagementClassifier wrapping a stub backend would silently return
        // Unknown on every classify call — that's worse than a deterministic stub.)
        ("stub", None) => Ok(Arc::new(StubEngagementClassifier::new())),

        // Explicit non-stub classifier backend → need a model too.
        (_, Some(cls_backend_name)) => {
            let model = params.classifier_model.clone().ok_or_else(|| {
                PrimerError::Inference(
                    "--classifier-model is required when --classifier-backend is set to a non-stub backend".into(),
                )
            })?;
            let cls_backend = build_backend(cls_backend_name, model.clone(), params).await?;
            Ok(Arc::new(LlmEngagementClassifier::new(
                cls_backend,
                model,
                settings,
            )))
        }

        // No --classifier-backend specified, main is not stub.
        (_, None) => {
            match params.classifier_model.as_deref() {
                // No model override → reuse the main backend via Arc::clone.
                // The classifier identifier becomes "llm:<main-model>".
                None => Ok(Arc::new(LlmEngagementClassifier::new(
                    Arc::clone(&main_backend),
                    main_model.to_string(),
                    settings,
                ))),
                // Model override only → construct a fresh backend of the same TYPE
                // with the override model, because InferenceBackend::generate() does
                // not accept a model argument — model is baked in at construction.
                Some(override_model) => {
                    let cls_backend =
                        build_backend(main_backend_name, override_model.to_string(), params)
                            .await?;
                    Ok(Arc::new(LlmEngagementClassifier::new(
                        cls_backend,
                        override_model.to_string(),
                        settings,
                    )))
                }
            }
        }
    }
}

/// Construct the concept extractor according to the same dispatch matrix
/// as `build_classifier`:
///
/// | main backend | --extractor-backend | --extractor-model | outcome                                |
/// |--------------|---------------------|-------------------|----------------------------------------|
/// | stub         | (unset)             | (any)             | StubConceptExtractor                   |
/// | *            | "stub"              | (any)             | StubConceptExtractor                   |
/// | *            | some(X)             | None              | error (model required for non-stub)    |
/// | *            | some(X)             | some(M)           | LlmConceptExtractor(new backend X, model M) |
/// | non-stub     | (unset)             | None              | LlmConceptExtractor(Arc::clone main)   |
/// | non-stub     | (unset)             | some(M)           | LlmConceptExtractor(new backend same type, model M) |
pub async fn build_extractor(
    main_backend: Arc<dyn InferenceBackend>,
    main_backend_name: &str,
    main_model: &str,
    params: &BackendParams,
    settings: ExtractorSettings,
) -> Result<Arc<dyn ConceptExtractor>> {
    // Warn on a flag combination the user probably didn't mean: passing
    // --extractor-model alongside an explicit --extractor-backend stub
    // looks like the model name will take effect, but stub ignores it.
    if matches!(params.extractor_backend.as_deref(), Some("stub"))
        && params.extractor_model.is_some()
    {
        tracing::warn!(
            model = ?params.extractor_model,
            "--extractor-model ignored: --extractor-backend is stub (deterministic, no model)"
        );
    }

    match (main_backend_name, params.extractor_backend.as_deref()) {
        (_, Some("stub")) => Ok(Arc::new(StubConceptExtractor::new())),
        ("stub", None) => Ok(Arc::new(StubConceptExtractor::new())),
        (_, Some(ext_backend_name)) => {
            let model = params.extractor_model.clone().ok_or_else(|| {
                PrimerError::Inference(
                    "--extractor-model is required when --extractor-backend is set to a non-stub backend".into(),
                )
            })?;
            let ext_backend = build_backend(ext_backend_name, model.clone(), params).await?;
            Ok(Arc::new(LlmConceptExtractor::new(
                ext_backend,
                model,
                settings,
            )))
        }
        (_, None) => match params.extractor_model.as_deref() {
            None => Ok(Arc::new(LlmConceptExtractor::new(
                Arc::clone(&main_backend),
                main_model.to_string(),
                settings,
            ))),
            Some(override_model) => {
                let ext_backend =
                    build_backend(main_backend_name, override_model.to_string(), params).await?;
                Ok(Arc::new(LlmConceptExtractor::new(
                    ext_backend,
                    override_model.to_string(),
                    settings,
                )))
            }
        },
    }
}

/// Construct the comprehension classifier according to the same dispatch matrix
/// as `build_extractor`:
///
/// | main backend | --comprehension-backend | --comprehension-model | outcome                                    |
/// |--------------|-------------------------|-----------------------|--------------------------------------------|
/// | stub         | (unset)                 | (any)                 | StubComprehensionClassifier                |
/// | *            | "stub"                  | (any)                 | StubComprehensionClassifier                |
/// | *            | some(X)                 | None                  | error (model required for non-stub)        |
/// | *            | some(X)                 | some(M)               | LlmComprehensionClassifier(new backend X, model M) |
/// | non-stub     | (unset)                 | None                  | LlmComprehensionClassifier(Arc::clone main) |
/// | non-stub     | (unset)                 | some(M)               | LlmComprehensionClassifier(new backend same type, model M) |
pub async fn build_comprehension(
    main_backend: Arc<dyn InferenceBackend>,
    main_backend_name: &str,
    main_model: &str,
    params: &BackendParams,
    settings: primer_comprehension::ComprehensionSettings,
) -> Result<Arc<dyn primer_comprehension::ComprehensionClassifier>> {
    if matches!(params.comprehension_backend.as_deref(), Some("stub"))
        && params.comprehension_model.is_some()
    {
        tracing::warn!(
            model = ?params.comprehension_model,
            "--comprehension-model ignored: --comprehension-backend is stub (deterministic, no model)"
        );
    }

    match (main_backend_name, params.comprehension_backend.as_deref()) {
        (_, Some("stub")) => Ok(Arc::new(
            primer_comprehension::StubComprehensionClassifier::new(),
        )),
        ("stub", None) => Ok(Arc::new(
            primer_comprehension::StubComprehensionClassifier::new(),
        )),
        (_, Some(comp_backend_name)) => {
            let model = params.comprehension_model.clone().ok_or_else(|| {
                PrimerError::Inference(
                    "--comprehension-model is required when --comprehension-backend is set to a non-stub backend".into(),
                )
            })?;
            let comp_backend = build_backend(comp_backend_name, model.clone(), params).await?;
            Ok(Arc::new(
                primer_comprehension::LlmComprehensionClassifier::new(
                    comp_backend,
                    model,
                    settings,
                ),
            ))
        }
        (_, None) => match params.comprehension_model.as_deref() {
            None => Ok(Arc::new(
                primer_comprehension::LlmComprehensionClassifier::new(
                    Arc::clone(&main_backend),
                    main_model.to_string(),
                    settings,
                ),
            )),
            Some(override_model) => {
                let comp_backend =
                    build_backend(main_backend_name, override_model.to_string(), params).await?;
                Ok(Arc::new(
                    primer_comprehension::LlmComprehensionClassifier::new(
                        comp_backend,
                        override_model.to_string(),
                        settings,
                    ),
                ))
            }
        },
    }
}

/// Construct a fastembed-rs-backed `Embedder`.
///
/// Return-value semantics:
/// - `Ok(Some(arc))` — feature compiled in AND init succeeded.
/// - `Ok(None)` — feature compiled in but init failed (e.g. model
///   download failed). Caller falls back to BM25-only retrieval; a
///   warning is emitted to stderr so a CLI user sees it.
/// - `Err(msg)` — feature not compiled in. The user explicitly asked
///   for fastembed; surfacing this as an error lets the binary decide
///   whether to exit (CLI) or render it inline (GUI). Earlier
///   versions called `std::process::exit(1)` here which is hostile to
///   any caller that isn't a CLI — never re-introduce that.
#[cfg(feature = "embedding")]
pub fn build_fastembed_embedder(
    model: Option<&str>,
) -> std::result::Result<Option<Arc<dyn primer_core::embedder::Embedder>>, String> {
    use primer_embedding::FastEmbedBackend;
    let m = model.unwrap_or(primer_embedding::BGE_M3_MODEL_ID);
    if m != primer_embedding::BGE_M3_MODEL_ID {
        eprintln!(
            "Note: --embedder-model {m} not yet supported by the CLI dispatch; using bge-m3."
        );
    }
    eprintln!(
        "Loading fastembed model {m}; first run downloads ~570 MB into ~/.cache/primer/models/."
    );
    match FastEmbedBackend::new() {
        Ok(b) => Ok(Some(Arc::new(b) as _)),
        Err(e) => {
            eprintln!("fastembed init failed ({e}); falling back to BM25-only retrieval.");
            Ok(None)
        }
    }
}

#[cfg(not(feature = "embedding"))]
pub fn build_fastembed_embedder(
    _model: Option<&str>,
) -> std::result::Result<Option<Arc<dyn primer_core::embedder::Embedder>>, String> {
    Err(
        "fastembed embedder requires the `embedding` cargo feature. \
         Build with `cargo run --features primer-cli/embedding -- ...` (or use embedder = none)."
            .to_string(),
    )
}

#[cfg(feature = "ollama-embedding")]
pub async fn build_ollama_embedder(
    url: Option<&str>,
    model: Option<&str>,
) -> std::result::Result<Option<Arc<dyn primer_core::embedder::Embedder>>, String> {
    use primer_embedding::{DEFAULT_OLLAMA_MODEL, DEFAULT_OLLAMA_URL, OllamaEmbedder};
    let url = url.unwrap_or(DEFAULT_OLLAMA_URL);
    let model = model.unwrap_or(DEFAULT_OLLAMA_MODEL);
    match OllamaEmbedder::with_endpoint(url, model).await {
        Ok(b) => {
            eprintln!("Embedder: ollama {model} at {url}");
            Ok(Some(Arc::new(b) as _))
        }
        Err(e) => {
            eprintln!("ollama embedder init failed ({e}); falling back to BM25-only retrieval.");
            Ok(None)
        }
    }
}

#[cfg(not(feature = "ollama-embedding"))]
pub async fn build_ollama_embedder(
    _url: Option<&str>,
    _model: Option<&str>,
) -> std::result::Result<Option<Arc<dyn primer_core::embedder::Embedder>>, String> {
    Err(
        "ollama embedder requires the `ollama-embedding` cargo feature. \
         Build with `cargo run --features primer-cli/ollama-embedding -- ...`"
            .to_string(),
    )
}

#[cfg(feature = "openai-compat-embedding")]
pub async fn build_openai_compat_embedder(
    url: Option<&str>,
    model: Option<&str>,
    api_key: Option<String>,
) -> std::result::Result<Option<Arc<dyn primer_core::embedder::Embedder>>, String> {
    use primer_embedding::{DEFAULT_OPENAI_COMPAT_URL, OpenAiCompatEmbedder};
    let url = url.unwrap_or(DEFAULT_OPENAI_COMPAT_URL);
    let model = model.ok_or_else(|| {
        "--embedder-openai-compat-model is required when --embedder-backend is openai-compat"
            .to_string()
    })?;
    match OpenAiCompatEmbedder::new(url, model, api_key).await {
        Ok(b) => {
            eprintln!("Embedder: openai-compat {model} at {url}");
            Ok(Some(Arc::new(b) as _))
        }
        Err(e) => {
            eprintln!(
                "openai-compat embedder init failed ({e}); falling back to BM25-only retrieval."
            );
            Ok(None)
        }
    }
}

#[cfg(not(feature = "openai-compat-embedding"))]
pub async fn build_openai_compat_embedder(
    _url: Option<&str>,
    _model: Option<&str>,
    _api_key: Option<String>,
) -> std::result::Result<Option<Arc<dyn primer_core::embedder::Embedder>>, String> {
    Err(
        "openai-compat embedder requires the `openai-compat-embedding` cargo feature. \
         Build with `cargo run --features primer-cli/openai-compat-embedding -- ...`"
            .to_string(),
    )
}

#[cfg(test)]
mod build_main_backend_tests;
#[cfg(test)]
mod classifier_construction_tests;
#[cfg(test)]
mod comprehension_construction_tests;
#[cfg(test)]
mod extractor_construction_tests;
#[cfg(test)]
mod main_backend_plan_tests;
#[cfg(test)]
mod qnn_dispatch_tests;
#[cfg(test)]
mod resolve_fallback_model_tests;
