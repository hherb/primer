//! Per-subsystem construction: engagement classifier, concept extractor,
//! and comprehension classifier. Each follows the identical
//! "main backend + optional per-subsystem `--*-backend` / `--*-model`
//! override" dispatch matrix documented per function.
//!
//! Split out of the flat `wiring` module (behaviour-preserving). The
//! shared `BackendParams` and the `build_backend` dispatch live in the
//! parent `wiring` module and `wiring::backend`; this file borrows them
//! via `super::`.

use std::sync::Arc;

use primer_classifier::{
    ClassifierSettings, EngagementClassifier, LlmEngagementClassifier, StubEngagementClassifier,
};
use primer_core::error::{PrimerError, Result};
use primer_core::inference::InferenceBackend;
use primer_extractor::{
    ConceptExtractor, ExtractorSettings, LlmConceptExtractor, StubConceptExtractor,
};

use super::BackendParams;
use super::backend::build_backend;

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
