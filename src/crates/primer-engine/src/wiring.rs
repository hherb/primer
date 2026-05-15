//! Backend / classifier / extractor / comprehension / embedder
//! construction matrix shared between binaries.
//!
//! The dispatch logic for "main backend + per-subsystem override"
//! lives here so `primer-cli` and `primer-gui` produce identical
//! wiring from identical inputs.

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
        other => Err(PrimerError::Inference(
            format!("unknown backend: {other}").into(),
        )),
    }
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
