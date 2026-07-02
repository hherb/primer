use std::sync::Arc;

use super::*;
use primer_core::inference::InferenceBackend;
use primer_extractor::ExtractorSettings;

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
        router_mode: primer_core::router::RouterMode::LocalOnly,
        primary_ttft_budget_ms: None,
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
