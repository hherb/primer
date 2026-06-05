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
/// - `PrimaryAlone` ⇒ primary alone (no fallback, or fallback unbuildable);
/// - `Fail` ⇒ the primary's construction error.
pub async fn build_main_backend(
    primary_name: &str,
    primary_model: String,
    params: &BackendParams,
) -> Result<Arc<dyn InferenceBackend>> {
    use primer_inference::FallbackBackend;

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

    // A fallback IS configured. Resolve the secondary model (may error for a
    // required-model backend) and try to build it.
    let fb_model = resolve_fallback_model(&fb_name, params.fallback_model.clone())
        .map_err(|m| PrimerError::Inference(m.into()))?;
    let secondary = build_backend(&fb_name, fb_model.unwrap_or_default(), params).await;

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
            // Reached only when primary built but secondary did not.
            if let Err(ref e) = secondary {
                tracing::warn!(
                    secondary = %fb_name,
                    error = %e,
                    "fallback backend failed to construct; using primary alone"
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
    let qairt_lib_dir = params
        .qnn_qairt_lib_dir
        .clone()
        .unwrap_or_else(|| default_qairt_lib_dir(bundle_dir));
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
mod classifier_construction_tests {
    use super::*;

    /// Build a minimal `BackendParams` for testing.
    fn params_with(
        classifier_backend: Option<&str>,
        classifier_model: Option<&str>,
    ) -> BackendParams {
        BackendParams {
            api_key: None,
            ollama_url: "http://localhost:11434".into(),
            openai_compat_url: "http://localhost:8000".into(),
            openai_compat_api_key: None,
            classifier_backend: classifier_backend.map(String::from),
            classifier_model: classifier_model.map(String::from),
            extractor_backend: None,
            extractor_model: None,
            comprehension_backend: None,
            comprehension_model: None,
            qnn_bundle_dir: None,
            qnn_qairt_lib_dir: None,
            gguf_path: None,
            llamacpp_gpu_layers: None,
            llamacpp_n_ctx: None,
            reasoning_markers: Vec::new(),
            fallback_backend: None,
            fallback_model: None,
        }
    }

    /// With main=stub and no classifier flags, we get a stub classifier.
    #[tokio::test]
    async fn stub_main_no_flags_gives_stub_classifier() {
        let params = params_with(None, None);
        let main = build_backend("stub", "main".into(), &params).await.unwrap();
        let c = build_classifier(main, "stub", "main", &params, ClassifierSettings::default())
            .await
            .unwrap();
        assert_eq!(c.identifier(), "stub");
    }

    /// `--classifier-backend stub` wins regardless of the main backend.
    #[tokio::test]
    async fn explicit_stub_override_wins_over_any_main() {
        // Main is stub (would default to stub anyway), but the explicit flag
        // should also work when the main is declared as a different type.
        // We use stub here to avoid needing a real cloud backend in tests.
        let params_main = params_with(None, None);
        let main = build_backend("stub", "main-model".into(), &params_main)
            .await
            .unwrap();
        let params = params_with(Some("stub"), None);
        let c = build_classifier(
            main,
            "cloud",
            "main-model",
            &params,
            ClassifierSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(c.identifier(), "stub");
    }

    /// No classifier flags on a non-stub main → LlmEngagementClassifier wrapping
    /// Arc::clone of main (identifier is "llm:<main-model>").
    #[tokio::test]
    async fn non_stub_main_no_flags_gives_llm_with_main_model() {
        // We simulate a non-stub main by using "stub" type but treating it as
        // cloud in the dispatch (the type doesn't matter for the Arc::clone path;
        // what matters is that main_backend_name is NOT "stub").
        // Since we can't construct a real cloud backend without an API key,
        // we build a stub backend but pass "cloud" as the name to exercise
        // the dispatch arm that reuses main via Arc::clone.
        let params = params_with(None, None);
        let main = build_backend("stub", "claude-sonnet-4-6".into(), &params)
            .await
            .unwrap();
        let c = build_classifier(
            main,
            "cloud", // use "cloud" as name to exercise the non-stub arm
            "claude-sonnet-4-6",
            &params,
            ClassifierSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(c.identifier(), "llm:claude-sonnet-4-6");
    }

    /// `--classifier-backend ollama` without `--classifier-model` is an error.
    #[tokio::test]
    async fn classifier_backend_without_model_is_error() {
        let params_main = params_with(None, None);
        let main = build_backend("stub", "main".into(), &params_main)
            .await
            .unwrap();
        let params = params_with(Some("ollama"), None /* no model */);
        let result =
            build_classifier(main, "stub", "main", &params, ClassifierSettings::default()).await;
        assert!(
            result.is_err(),
            "should error when --classifier-backend is non-stub and --classifier-model is missing"
        );
    }

    /// `--classifier-model` override on a stub main still yields stub classifier
    /// (the stub arm fires before we reach the model-override arm).
    #[tokio::test]
    async fn classifier_model_override_on_stub_main_still_yields_stub() {
        let params = params_with(None, Some("override-model"));
        let main = build_backend("stub", "main".into(), &params).await.unwrap();
        let c = build_classifier(main, "stub", "main", &params, ClassifierSettings::default())
            .await
            .unwrap();
        // Main is stub → classifier defaults to stub regardless of model override.
        assert_eq!(c.identifier(), "stub");
    }

    /// `--classifier-model` override on a NON-stub main constructs a fresh
    /// backend of the same type with the override model (case 5 in the dispatch
    /// matrix).
    ///
    /// The `main_backend` Arc is a stub (we cannot unit-test real cloud/ollama
    /// without live infrastructure), but `main_backend_name` is "ollama" so the
    /// dispatch falls through to the model-override branch and calls
    /// `build_backend("ollama", "override-model", params)`.  `OllamaBackend::new`
    /// is purely constructive (no I/O), so this succeeds in unit tests.
    #[tokio::test]
    async fn classifier_model_override_on_non_stub_main_constructs_fresh_backend() {
        let params = params_with(None, Some("override-model"));
        // The Arc itself is a stub, but the *name* string "ollama" drives dispatch.
        let main = build_backend("stub", "main".into(), &params).await.unwrap();
        let c = build_classifier(
            main,
            "ollama", // non-stub name → falls through to model-override arm
            "main-model",
            &params,
            ClassifierSettings::default(),
        )
        .await
        .unwrap();
        // A fresh OllamaBackend is constructed with "override-model"; the
        // classifier identifier must reflect the override, not the main model.
        assert_eq!(c.identifier(), "llm:override-model");
    }

    /// An unknown `--classifier-backend` value must return an error (the
    /// explicit-backend arm calls `build_backend`, which errors on unknown names).
    #[tokio::test]
    async fn unknown_classifier_backend_returns_error() {
        let params = params_with(Some("nonexistent"), Some("any-model"));
        let main = build_backend("stub", "main".into(), &params).await.unwrap();
        let result =
            build_classifier(main, "stub", "main", &params, ClassifierSettings::default()).await;
        assert!(
            result.is_err(),
            "should error when --classifier-backend names an unknown backend"
        );
    }
}

#[cfg(test)]
mod extractor_construction_tests {
    use super::*;

    fn params(extractor_backend: Option<&str>, extractor_model: Option<&str>) -> BackendParams {
        BackendParams {
            api_key: Some("k".into()),
            ollama_url: "http://localhost:11434".into(),
            openai_compat_url: "http://localhost:8000".into(),
            openai_compat_api_key: None,
            classifier_backend: None,
            classifier_model: None,
            extractor_backend: extractor_backend.map(String::from),
            extractor_model: extractor_model.map(String::from),
            comprehension_backend: None,
            comprehension_model: None,
            qnn_bundle_dir: None,
            qnn_qairt_lib_dir: None,
            gguf_path: None,
            llamacpp_gpu_layers: None,
            llamacpp_n_ctx: None,
            reasoning_markers: Vec::new(),
            fallback_backend: None,
            fallback_model: None,
        }
    }

    fn stub_main_backend() -> Arc<dyn InferenceBackend> {
        Arc::new(primer_inference::stub::StubBackend)
    }

    #[tokio::test]
    async fn stub_main_no_flags_gives_stub_extractor() {
        let e = build_extractor(
            stub_main_backend(),
            "stub",
            "stub",
            &params(None, None),
            ExtractorSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(e.identifier(), "stub");
    }

    #[tokio::test]
    async fn explicit_stub_override_wins() {
        let e = build_extractor(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(Some("stub"), None),
            ExtractorSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(e.identifier(), "stub");
    }

    #[tokio::test]
    async fn nonstub_backend_without_model_errors() {
        let r = build_extractor(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(Some("ollama"), None),
            ExtractorSettings::default(),
        )
        .await;
        assert!(r.is_err());
    }

    // Note: `stub_main_backend()` always returns a StubBackend (whose
    // `name()` is "stub"), even when `main_backend_name` says otherwise.
    // The identifier therefore reads as `llm:stub:<model>` in these
    // tests — what they actually verify is that the *model* name flows
    // through correctly. In production the main_backend_name and the
    // backend's own `name()` agree by construction.
    #[tokio::test]
    async fn no_flags_reuses_main_model_id() {
        let e = build_extractor(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(None, None),
            ExtractorSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(e.identifier(), "llm:stub:llama3.2");
    }

    #[tokio::test]
    async fn model_only_override_uses_main_backend_type() {
        // With `--extractor-model haiku` and main backend `ollama`,
        // `build_backend("ollama", "haiku", ...)` constructs a real
        // OllamaBackend, so the identifier carries the real backend
        // name (unlike the `_reuses_main_model_id` test above which
        // Arc::clones the stub harness).
        let e = build_extractor(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(None, Some("haiku")),
            ExtractorSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(e.identifier(), "llm:ollama:haiku");
    }
}

#[cfg(test)]
mod comprehension_construction_tests {
    use super::*;

    fn params(
        comprehension_backend: Option<&str>,
        comprehension_model: Option<&str>,
    ) -> BackendParams {
        BackendParams {
            api_key: Some("k".into()),
            ollama_url: "http://localhost:11434".into(),
            openai_compat_url: "http://localhost:8000".into(),
            openai_compat_api_key: None,
            classifier_backend: None,
            classifier_model: None,
            extractor_backend: None,
            extractor_model: None,
            comprehension_backend: comprehension_backend.map(String::from),
            comprehension_model: comprehension_model.map(String::from),
            qnn_bundle_dir: None,
            qnn_qairt_lib_dir: None,
            gguf_path: None,
            llamacpp_gpu_layers: None,
            llamacpp_n_ctx: None,
            reasoning_markers: Vec::new(),
            fallback_backend: None,
            fallback_model: None,
        }
    }

    fn stub_main_backend() -> Arc<dyn InferenceBackend> {
        Arc::new(primer_inference::stub::StubBackend)
    }

    #[tokio::test]
    async fn stub_main_no_flags_gives_stub_comprehension() {
        let c = build_comprehension(
            stub_main_backend(),
            "stub",
            "stub",
            &params(None, None),
            primer_comprehension::ComprehensionSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(c.identifier(), "stub");
    }

    #[tokio::test]
    async fn explicit_stub_override_wins() {
        let c = build_comprehension(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(Some("stub"), None),
            primer_comprehension::ComprehensionSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(c.identifier(), "stub");
    }

    #[tokio::test]
    async fn nonstub_backend_without_model_errors() {
        let r = build_comprehension(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(Some("ollama"), None),
            primer_comprehension::ComprehensionSettings::default(),
        )
        .await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn no_flags_reuses_main_model_id() {
        let c = build_comprehension(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(None, None),
            primer_comprehension::ComprehensionSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(c.identifier(), "llm:stub:llama3.2");
    }

    #[tokio::test]
    async fn model_only_override_uses_main_backend_type() {
        let c = build_comprehension(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(None, Some("haiku")),
            primer_comprehension::ComprehensionSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(c.identifier(), "llm:ollama:haiku");
    }
}

#[cfg(test)]
mod qnn_dispatch_tests {
    //! Pin the `--backend qnn` dispatch path of [`build_backend`].
    //!
    //! The QNN backend's *positive* construction path is unit-tested
    //! at the [`primer_inference::qnn`] module (via the mock
    //! `GenieLibrary` trait split). Here we only need to pin
    //! [`build_backend`]'s dispatch:
    //!
    //! - With the feature compiled in, missing `qnn_bundle_dir` is a
    //!   clear error from `build_backend` itself (before any FFI).
    //! - With the feature compiled in and `qnn_bundle_dir` set,
    //!   dispatch reaches `QnnBackend::new`, which on every non-Android
    //!   host returns the typed `PlatformUnsupported` error — proving
    //!   that the qnn arm fired (any other arm would have produced
    //!   either Ok or a different error message).
    //! - With the feature *not* compiled in, the user gets a
    //!   "rebuild with --features qnn" hint, NOT the generic
    //!   "unknown backend" message — that distinction is the
    //!   load-bearing UX win of the per-feature dispatch.
    use super::*;

    /// Build a `BackendParams` skeleton for the qnn dispatch tests.
    fn params() -> BackendParams {
        BackendParams {
            api_key: None,
            ollama_url: "http://localhost:11434".into(),
            openai_compat_url: "http://localhost:8000".into(),
            openai_compat_api_key: None,
            classifier_backend: None,
            classifier_model: None,
            extractor_backend: None,
            extractor_model: None,
            comprehension_backend: None,
            comprehension_model: None,
            qnn_bundle_dir: None,
            qnn_qairt_lib_dir: None,
            gguf_path: None,
            llamacpp_gpu_layers: None,
            llamacpp_n_ctx: None,
            reasoning_markers: Vec::new(),
            fallback_backend: None,
            fallback_model: None,
        }
    }

    #[test]
    fn default_qairt_lib_dir_lives_one_dir_up_alongside_qairt_lib() {
        // The conventional QAIRT layout from AI Hub apps puts the
        // bundle dir alongside `qairt/` at the same parent level:
        //   ~/primer-bundles/qwen3-4b/{genie_config.json, ...}
        //   ~/primer-bundles/qairt/lib/aarch64-android/libGenie.so
        let bundle = PathBuf::from("/home/user/primer-bundles/qwen3-4b");
        let lib = default_qairt_lib_dir(&bundle);
        assert_eq!(
            lib,
            PathBuf::from("/home/user/primer-bundles/qairt/lib/aarch64-android")
        );
    }

    #[test]
    fn default_qairt_lib_dir_tolerates_root_bundle_path() {
        // A bundle at the filesystem root (no parent) is unusual but
        // should not panic. We fall back to a same-directory relative
        // path — downstream `RealGenieLibrary::open` will report a
        // useful `LibraryLoad` error rather than us deciding here.
        let bundle = PathBuf::from("/");
        let lib = default_qairt_lib_dir(&bundle);
        // On `/`, `.parent()` is `None`, so we fall back to the bare
        // relative path. Exact form is documented in the helper.
        assert_eq!(lib, PathBuf::from("qairt/lib/aarch64-android"));
    }

    /// Without the `qnn` feature, the dispatch arm hands back a build
    /// hint, NOT the generic "unknown backend: qnn" string. The
    /// distinction matters because users who haven't compiled in qnn
    /// need a different action than users who typo'd the backend name.
    #[cfg(not(feature = "qnn"))]
    #[tokio::test]
    async fn qnn_without_feature_returns_build_hint() {
        let p = params();
        let result = build_backend("qnn", "qnn-placeholder".into(), &p).await;
        let err = match result {
            Ok(_) => panic!("expected qnn-without-feature to error, got Ok"),
            Err(e) => e,
        };
        let msg = format!("{err}");
        assert!(
            msg.contains("qnn") && msg.contains("feature"),
            "expected build hint mentioning qnn + feature; got: {msg}"
        );
        // And NOT the generic unknown-backend phrasing:
        assert!(
            !msg.contains("unknown backend"),
            "qnn-without-feature should be distinct from unknown-backend; got: {msg}"
        );
    }

    /// With the `qnn` feature compiled in but no `qnn_bundle_dir`
    /// set in params, the dispatch arm reports the missing required
    /// input BEFORE any FFI is attempted — exactly the "fast clap-style
    /// rejection" UX the plan calls for.
    #[cfg(feature = "qnn")]
    #[tokio::test]
    async fn qnn_with_feature_missing_bundle_dir_errors_pre_ffi() {
        let p = params();
        let result = build_backend("qnn", "qnn-placeholder".into(), &p).await;
        let err = match result {
            Ok(_) => panic!("expected qnn-with-no-bundle to error, got Ok"),
            Err(e) => e,
        };
        let msg = format!("{err}");
        assert!(
            msg.contains("qnn-bundle-dir") || msg.contains("bundle"),
            "expected missing-bundle-dir hint; got: {msg}"
        );
    }

    /// With the `qnn` feature compiled in and a `qnn_bundle_dir`
    /// set, dispatch reaches `QnnBackend::new`. On every host the
    /// repo's CI runs on (Linux + macOS), this surfaces the typed
    /// `PlatformUnsupported` error from `primer-qnn-sys`. That proves
    /// the qnn arm fired — neither the "unknown backend" arm nor the
    /// "missing bundle dir" guard could have produced this string.
    #[cfg(all(feature = "qnn", not(target_os = "android")))]
    #[tokio::test]
    async fn qnn_with_feature_and_bundle_dir_dispatches_to_real_lib_on_host() {
        // Build params with a fake (nonexistent) bundle dir — the
        // dispatch arm hands these straight to `QnnBackend::new`, which
        // tries to dlopen `libGenie.so` from the qairt lib dir FIRST.
        // On a non-Android host, that returns `PlatformUnsupported`
        // before the bundle's existence is checked.
        let p = BackendParams {
            qnn_bundle_dir: Some(PathBuf::from("/nonexistent/bundle")),
            ..params()
        };
        let result = build_backend("qnn", "qnn-placeholder".into(), &p).await;
        let err = match result {
            Ok(_) => panic!("expected PlatformUnsupported on non-Android host, got Ok"),
            Err(e) => e,
        };
        let msg = format!("{err}");
        // The dev-facing error string from
        // `GenieCallError::PlatformUnsupported` carries the platform
        // name. On macOS this is `"macos"`, on Linux it's `"linux"`.
        assert!(
            msg.to_lowercase().contains("android")
                || msg.to_lowercase().contains("platform")
                || msg.to_lowercase().contains("only supported"),
            "expected PlatformUnsupported-flavoured error; got: {msg}"
        );
    }

    /// Without the `llamacpp` feature (the default test build), the
    /// dispatch arm hands back a build hint mentioning `llamacpp` and
    /// `feature` — not the generic "unknown backend" string.
    #[tokio::test]
    async fn llamacpp_without_feature_returns_build_hint() {
        let params = BackendParams {
            gguf_path: Some(std::path::PathBuf::from("/tmp/model.gguf")),
            ..params()
        };
        let err = match build_backend("llamacpp", "ignored".into(), &params).await {
            Ok(_) => panic!("expected llamacpp-without-feature to error, got Ok"),
            Err(e) => e,
        };
        let msg = format!("{err}");
        assert!(msg.contains("llamacpp"), "got: {msg}");
        assert!(msg.contains("feature"), "got: {msg}");
    }

    /// The `"llamacpp"` string reaches its own dispatch arm (the error
    /// mentions llamacpp, proving it didn't fall through to the generic
    /// unknown-backend arm).
    #[tokio::test]
    async fn llamacpp_dispatch_reaches_arm() {
        let params = params();
        let err = match build_backend("llamacpp", "ignored".into(), &params).await {
            Ok(_) => panic!("expected llamacpp dispatch to error, got Ok"),
            Err(e) => e,
        };
        assert!(format!("{err}").to_lowercase().contains("llamacpp"));
    }
}

#[cfg(test)]
mod main_backend_plan_tests {
    use super::{MainBackendPlan, plan_main_backend};

    #[test]
    fn primary_ok_no_fallback_is_primary_alone() {
        assert_eq!(
            plan_main_backend(true, false, false),
            MainBackendPlan::PrimaryAlone
        );
        assert_eq!(
            plan_main_backend(true, false, true),
            MainBackendPlan::PrimaryAlone
        );
    }

    #[test]
    fn primary_ok_fallback_built_is_wrapped() {
        assert_eq!(
            plan_main_backend(true, true, true),
            MainBackendPlan::Wrapped
        );
    }

    #[test]
    fn primary_ok_fallback_failed_is_primary_alone() {
        assert_eq!(
            plan_main_backend(true, true, false),
            MainBackendPlan::PrimaryAlone
        );
    }

    #[test]
    fn primary_failed_secondary_built_is_secondary_alone() {
        assert_eq!(
            plan_main_backend(false, true, true),
            MainBackendPlan::SecondaryAlone
        );
    }

    #[test]
    fn primary_failed_secondary_failed_is_fail() {
        assert_eq!(plan_main_backend(false, true, false), MainBackendPlan::Fail);
    }

    #[test]
    fn primary_failed_no_fallback_is_fail() {
        assert_eq!(
            plan_main_backend(false, false, false),
            MainBackendPlan::Fail
        );
        assert_eq!(plan_main_backend(false, false, true), MainBackendPlan::Fail);
    }
}

#[cfg(test)]
mod build_main_backend_tests {
    use super::*;

    fn params(fallback_backend: Option<&str>, fallback_model: Option<&str>) -> BackendParams {
        BackendParams {
            api_key: None,
            ollama_url: "http://localhost:11434".into(),
            openai_compat_url: "http://localhost:8000".into(),
            openai_compat_api_key: None,
            classifier_backend: None,
            classifier_model: None,
            extractor_backend: None,
            extractor_model: None,
            comprehension_backend: None,
            comprehension_model: None,
            qnn_bundle_dir: None,
            qnn_qairt_lib_dir: None,
            gguf_path: None,
            llamacpp_gpu_layers: None,
            llamacpp_n_ctx: None,
            reasoning_markers: Vec::new(),
            fallback_backend: fallback_backend.map(String::from),
            fallback_model: fallback_model.map(String::from),
        }
    }

    /// No fallback configured ⇒ primary alone (unchanged behavior).
    #[tokio::test]
    async fn no_fallback_returns_primary() {
        let p = params(None, None);
        let b = build_main_backend("stub", "m".into(), &p).await.unwrap();
        assert_eq!(b.name(), "stub");
    }

    /// Primary fails to build, fallback (stub) builds ⇒ secondary alone.
    /// `unknown-backend` is an unbuildable backend name, so the primary leg errors.
    #[tokio::test]
    async fn primary_unbuildable_falls_back_to_secondary() {
        let p = params(Some("stub"), None);
        let b = build_main_backend("unknown-backend", "m".into(), &p)
            .await
            .unwrap();
        // Secondary stub served ⇒ its name surfaces.
        assert_eq!(b.name(), "stub");
    }

    /// Primary builds, fallback fails to build ⇒ primary alone, no error.
    #[tokio::test]
    async fn fallback_unbuildable_keeps_primary() {
        let p = params(Some("unknown-backend"), Some("m"));
        let b = build_main_backend("stub", "m".into(), &p).await.unwrap();
        assert_eq!(b.name(), "stub");
    }

    /// Both unbuildable ⇒ error (the primary's construction error).
    #[tokio::test]
    async fn both_unbuildable_errors() {
        let p = params(Some("unknown-fallback"), Some("m"));
        let r = build_main_backend("unknown-primary", "m".into(), &p).await;
        assert!(r.is_err());
    }

    /// Fallback configured as ollama without a model ⇒ resolve error surfaces.
    #[tokio::test]
    async fn fallback_ollama_without_model_errors() {
        let p = params(Some("ollama"), None);
        let r = build_main_backend("stub", "m".into(), &p).await;
        assert!(r.is_err(), "ollama fallback needs a model");
    }
}

#[cfg(test)]
mod resolve_fallback_model_tests {
    use super::resolve_fallback_model;
    use primer_core::consts::inference::DEFAULT_CLOUD_MODEL;

    #[test]
    fn stub_ignores_model() {
        assert_eq!(resolve_fallback_model("stub", None).unwrap(), None);
        assert_eq!(
            resolve_fallback_model("stub", Some("x".into())).unwrap(),
            None
        );
    }

    #[test]
    fn cloud_defaults_when_unset() {
        assert_eq!(
            resolve_fallback_model("cloud", None).unwrap(),
            Some(DEFAULT_CLOUD_MODEL.to_string())
        );
    }

    #[test]
    fn cloud_uses_explicit_model() {
        assert_eq!(
            resolve_fallback_model("cloud", Some("claude-opus-4-7".into())).unwrap(),
            Some("claude-opus-4-7".to_string())
        );
    }

    #[test]
    fn ollama_requires_model() {
        assert!(resolve_fallback_model("ollama", None).is_err());
        assert_eq!(
            resolve_fallback_model("ollama", Some("llama3.2".into())).unwrap(),
            Some("llama3.2".to_string())
        );
    }

    #[test]
    fn openai_compat_requires_model() {
        assert!(resolve_fallback_model("openai-compat", None).is_err());
        assert_eq!(
            resolve_fallback_model("openai-compat", Some("Qwen3-8B".into())).unwrap(),
            Some("Qwen3-8B".to_string())
        );
    }
}
