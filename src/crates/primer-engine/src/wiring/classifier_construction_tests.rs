use super::*;
use primer_classifier::ClassifierSettings;
use primer_core::inference::InferenceBackend;
use std::sync::Arc;

/// Build a minimal `BackendParams` for testing.
fn params_with(classifier_backend: Option<&str>, classifier_model: Option<&str>) -> BackendParams {
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
        router_mode: primer_core::router::RouterMode::LocalOnly,
        primary_ttft_budget_ms: None,
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

// ── Arc::clone inheritance, pinned behaviourally (issue #362) ──────────────

/// Main-backend double whose every reply is one canned engagement-assessment
/// JSON. A freshly built backend could never produce it, so a classifier that
/// returns it must be talking to this exact instance.
struct CannedAssessmentBackend;

const CANNED_ASSESSMENT: &str =
    r#"{"state": "FrustratedStuck", "confidence": 0.85, "reasoning": "canned"}"#;

#[async_trait::async_trait]
impl InferenceBackend for CannedAssessmentBackend {
    fn name(&self) -> &str {
        "canned"
    }

    async fn is_available(&self) -> bool {
        true
    }

    async fn generate_stream(
        &self,
        _prompt: &primer_core::inference::Prompt,
        _params: &primer_core::inference::GenerationParams,
    ) -> primer_core::error::Result<primer_core::inference::TokenStream> {
        let chunk = primer_core::inference::TokenChunk {
            text: CANNED_ASSESSMENT.into(),
            done: true,
            ..Default::default()
        };
        Ok(Box::pin(futures::stream::once(async move { Ok(chunk) })))
    }
}

/// No `--classifier-*` override on a non-stub served kind: the classifier
/// must reuse the main backend via `Arc::clone` (that is how it inherits a
/// `FallbackBackend` / `RouterBackend`). The id (`llm:<model>`) carries no
/// backend name, so only a real `classify` call can tell a clone from a
/// fresh `build_backend("openai-compat", …)`, which would get no canned reply.
#[tokio::test]
async fn classifier_without_override_classifies_through_main_backend() {
    use primer_core::classifier::EngagementContext;
    use primer_core::learner::EngagementState;

    let params = params_with(None, None);
    let main: Arc<dyn InferenceBackend> = Arc::new(CannedAssessmentBackend);
    let c = build_classifier(
        main,
        "openai-compat",
        "inherited-model",
        &params,
        ClassifierSettings::default(),
    )
    .await
    .unwrap();
    let a = c
        .classify(EngagementContext {
            recent_child_turns: &[],
            prior_assessments: &[],
        })
        .await
        .unwrap();
    assert_eq!(a.state, EngagementState::FrustratedStuck);
}

// ── stub_ignores_model (issue #365) ─────────────────────────────────────────
// Shared by all three subsystem builders: says why a `--*-model` override has
// no effect, so the builder can warn instead of dropping it silently.

use super::subsystems::stub_ignores_model;

/// Explicit `--*-backend stub` + a model: ignored (the pre-#365 warning).
#[test]
fn stub_ignores_model_explicit_stub_override() {
    assert!(stub_ignores_model("ollama", Some("stub"), Some("m")).is_some());
}

/// Served backend is stub, no subsystem backend, a model: ignored. This is the
/// case #365 found silent (stub primary, or a stub fallback serving alone).
#[test]
fn stub_ignores_model_stub_served_backend() {
    assert!(stub_ignores_model("stub", None, Some("m")).is_some());
}

/// No model override: nothing to ignore, whatever the backends are.
#[test]
fn stub_ignores_model_no_model_is_none() {
    assert_eq!(stub_ignores_model("stub", None, None), None);
    assert_eq!(stub_ignores_model("ollama", Some("stub"), None), None);
}

/// A model that takes effect (non-stub served kind or explicit non-stub
/// subsystem backend) is not reported — even when the served kind is stub.
#[test]
fn stub_ignores_model_effective_model_is_none() {
    assert_eq!(stub_ignores_model("ollama", None, Some("m")), None);
    assert_eq!(stub_ignores_model("stub", Some("ollama"), Some("m")), None);
}
