use super::*;

fn params(comprehension_backend: Option<&str>, comprehension_model: Option<&str>) -> BackendParams {
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
        router_mode: primer_core::router::RouterMode::LocalOnly,
        primary_ttft_budget_ms: None,
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
