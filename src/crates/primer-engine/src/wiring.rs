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
        "ollama" => Ok(Arc::new(primer_inference::ollama::OllamaBackend::new(
            params.ollama_url.clone(),
            model,
        ))),
        "openai-compat" => Ok(Arc::new(
            primer_inference::openai_compat::OpenAiCompatBackend::new(
                params.openai_compat_url.clone(),
                model,
                params.openai_compat_api_key.clone(),
            ),
        )),
        "qnn" => build_qnn_backend(params).await,
        other => Err(PrimerError::Inference(
            format!("unknown backend: {other}").into(),
        )),
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
}
