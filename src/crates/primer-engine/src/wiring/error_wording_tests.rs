//! Issue #341: engine construction errors reach both frontends verbatim
//! (CLI stderr and the desktop GUI's inline error / notice banner), so
//! every actionable hint must name BOTH the CLI flag and the GUI Settings
//! field — a GUI user has no flags, a CLI user has no Settings modal.
use super::*;
use primer_classifier::ClassifierSettings;
use primer_comprehension::ComprehensionSettings;
use primer_core::error::Result;
use primer_extractor::ExtractorSettings;
use std::sync::Arc;

/// `BackendParams` with only the subsystem overrides varying.
fn params(subsystem_backend: Option<&str>) -> BackendParams {
    BackendParams {
        api_key: None,
        ollama_url: "http://localhost:11434".into(),
        openai_compat_url: "http://localhost:8000".into(),
        openai_compat_api_key: None,
        classifier_backend: subsystem_backend.map(String::from),
        classifier_model: None,
        extractor_backend: subsystem_backend.map(String::from),
        extractor_model: None,
        comprehension_backend: subsystem_backend.map(String::from),
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

/// The error text of a `Result` that must have failed.
fn err_text<T>(r: Result<T>) -> String {
    match r {
        Ok(_) => panic!("expected an error"),
        Err(e) => e.to_string(),
    }
}

/// Pins the dual rendering: the CLI flag AND the GUI Settings path.
fn assert_names_both(msg: &str, flag: &str, settings_path: &str) {
    assert!(msg.contains(flag), "CLI flag {flag:?} missing: {msg}");
    assert!(
        msg.contains(&format!("Settings → {settings_path}")),
        "GUI Settings path {settings_path:?} missing: {msg}"
    );
}

/// A feature-gated hint must say how to rebuild BOTH binaries.
fn assert_names_both_crates(msg: &str, feature: &str) {
    assert!(msg.contains(&format!("primer-cli/{feature}")), "{msg}");
    assert!(msg.contains(&format!("primer-gui/{feature}")), "{msg}");
}

#[test]
fn fallback_model_required_names_flag_and_setting() {
    let msg = resolve_fallback_model("ollama", None).unwrap_err();
    assert_names_both(
        &msg,
        "--fallback-model",
        "Inference backend → Fallback model",
    );
}

#[tokio::test]
async fn cloud_key_required_names_flag_env_and_setting() {
    let msg = err_text(build_backend("cloud", "m".into(), &params(None)).await);
    assert_names_both(&msg, "--api-key", "Inference backend → API key");
    assert!(msg.contains("ANTHROPIC_API_KEY"), "{msg}");
}

#[tokio::test]
async fn subsystem_model_required_names_flag_and_setting() {
    let p = params(Some("ollama"));
    let main = build_backend("stub", "m".into(), &p).await.unwrap();

    let c = build_classifier(
        Arc::clone(&main),
        "stub",
        "m",
        &p,
        ClassifierSettings::default(),
    );
    assert_names_both(
        &err_text(c.await),
        "--classifier-model",
        "Classifier subsystem → Model",
    );

    let e = build_extractor(
        Arc::clone(&main),
        "stub",
        "m",
        &p,
        ExtractorSettings::default(),
    );
    assert_names_both(
        &err_text(e.await),
        "--extractor-model",
        "Extractor subsystem → Model",
    );

    let k = build_comprehension(main, "stub", "m", &p, ComprehensionSettings::default());
    assert_names_both(
        &err_text(k.await),
        "--comprehension-model",
        "Comprehension subsystem → Model",
    );
}

#[cfg(not(feature = "qnn"))]
#[tokio::test]
async fn qnn_feature_hint_names_both_crates() {
    let msg = err_text(build_backend("qnn", "m".into(), &params(None)).await);
    assert_names_both_crates(&msg, "qnn");
}

#[cfg(not(feature = "llamacpp"))]
#[tokio::test]
async fn llamacpp_feature_hint_names_both_crates() {
    let msg = err_text(build_backend("llamacpp", "m".into(), &params(None)).await);
    assert_names_both_crates(&msg, "llamacpp");
}

#[cfg(not(feature = "embedding"))]
#[test]
fn fastembed_feature_hint_names_both_crates() {
    let msg = build_fastembed_embedder(None).err().unwrap();
    assert_names_both_crates(&msg, "embedding");
}

#[cfg(not(feature = "ollama-embedding"))]
#[tokio::test]
async fn ollama_embedder_feature_hint_names_both_crates() {
    let msg = build_ollama_embedder(None, None).await.err().unwrap();
    assert_names_both_crates(&msg, "ollama-embedding");
}

#[cfg(not(feature = "openai-compat-embedding"))]
#[tokio::test]
async fn openai_compat_embedder_feature_hint_names_both_crates() {
    let msg = build_openai_compat_embedder(None, None, None)
        .await
        .err()
        .unwrap();
    assert_names_both_crates(&msg, "openai-compat-embedding");
}

#[cfg(feature = "openai-compat-embedding")]
#[tokio::test]
async fn openai_compat_embedder_model_required_names_flag_and_setting() {
    let msg = build_openai_compat_embedder(None, None, None)
        .await
        .err()
        .unwrap();
    assert_names_both(&msg, "--embedder-openai-compat-model", "Embedder → Model");
}
