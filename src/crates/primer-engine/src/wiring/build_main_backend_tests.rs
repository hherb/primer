use super::*;
use primer_core::router::RouterMode;

fn params(
    fallback_backend: Option<&str>,
    fallback_model: Option<&str>,
    router_mode: primer_core::router::RouterMode,
) -> BackendParams {
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
        router_mode,
        primary_ttft_budget_ms: None,
    }
}

/// No fallback configured ⇒ primary alone (unchanged behavior).
#[tokio::test]
async fn no_fallback_returns_primary() {
    let p = params(None, None, RouterMode::LocalOnly);
    let b = build_main_backend("stub", "m".into(), &p).await.unwrap();
    assert_eq!(b.name(), "stub");
}

/// Primary fails to build, fallback (stub) builds ⇒ secondary alone.
/// `unknown-backend` is an unbuildable backend name, so the primary leg errors.
#[tokio::test]
async fn primary_unbuildable_falls_back_to_secondary() {
    let p = params(Some("stub"), None, RouterMode::LocalOnly);
    let b = build_main_backend("unknown-backend", "m".into(), &p)
        .await
        .unwrap();
    // Secondary stub served ⇒ its name surfaces.
    assert_eq!(b.name(), "stub");
}

/// Primary builds, fallback fails to build ⇒ primary alone, no error.
#[tokio::test]
async fn fallback_unbuildable_keeps_primary() {
    let p = params(Some("unknown-backend"), Some("m"), RouterMode::LocalOnly);
    let b = build_main_backend("stub", "m".into(), &p).await.unwrap();
    assert_eq!(b.name(), "stub");
}

/// Both unbuildable ⇒ error (the primary's construction error).
#[tokio::test]
async fn both_unbuildable_errors() {
    let p = params(Some("unknown-fallback"), Some("m"), RouterMode::LocalOnly);
    let r = build_main_backend("unknown-primary", "m".into(), &p).await;
    // `Result::err` drops the `Ok` value (`Arc<dyn InferenceBackend>` is
    // not `Debug`, so `expect_err` won't compile here).
    let err = r.err().expect("both legs unbuildable must error");
    // Spec invariant: the `Fail` arm surfaces the PRIMARY's (most
    // informative) error, not the secondary's. Distinct backend names let
    // us prove which one propagated.
    let msg = format!("{err}");
    assert!(
        msg.contains("unknown-primary"),
        "expected the primary's error to surface; got: {msg}"
    );
    assert!(
        !msg.contains("unknown-fallback"),
        "must not surface the secondary's error; got: {msg}"
    );
}

/// Fallback misconfigured (ollama without a model) but the primary is
/// healthy ⇒ the broken opt-in fallback is dropped and the primary serves
/// alone. A misconfigured fallback must NEVER abort startup when the
/// primary built fine.
#[tokio::test]
async fn fallback_misconfigured_keeps_primary() {
    let p = params(Some("ollama"), None, RouterMode::LocalOnly);
    let b = build_main_backend("stub", "m".into(), &p).await.unwrap();
    assert_eq!(b.name(), "stub");
}

/// Primary unbuildable AND fallback misconfigured ⇒ error. The `Fail` arm
/// surfaces the PRIMARY's (more actionable) error, not the fallback's
/// resolve message.
#[tokio::test]
async fn primary_unbuildable_and_fallback_misconfigured_errors() {
    // `ollama` without a model is a resolve error (misconfigured fallback).
    let p = params(Some("ollama"), None, RouterMode::LocalOnly);
    let r = build_main_backend("unknown-primary", "m".into(), &p).await;
    let err = r.err().expect("both legs unusable must error");
    let msg = format!("{err}");
    assert!(
        msg.contains("unknown-primary"),
        "expected the primary's error to surface; got: {msg}"
    );
}

/// Hybrid mode + a buildable secondary ⇒ a RouterBackend whose name() is
/// the primary's (load-bearing for the small-context budget).
#[tokio::test]
async fn hybrid_with_secondary_builds_router_named_after_primary() {
    let p = params(Some("stub"), None, RouterMode::Hybrid);
    let b = build_main_backend("stub", "m".into(), &p).await.unwrap();
    assert_eq!(b.name(), "stub");
}

/// Routing mode with NO secondary configured ⇒ a clear error.
#[tokio::test]
async fn hybrid_without_secondary_errors() {
    let p = params(None, None, RouterMode::Hybrid);
    let r = build_main_backend("stub", "m".into(), &p).await;
    let err = r.err().expect("routing without a secondary must error");
    let msg = format!("{err}").to_lowercase();
    assert!(
        msg.contains("secondary") || msg.contains("fallback"),
        "expected a 'needs a secondary leg' error; got: {err}"
    );
}

/// local-only mode is byte-for-byte the existing fallback behavior.
#[tokio::test]
async fn local_only_with_fallback_is_unchanged_fallback() {
    let p = params(Some("stub"), None, RouterMode::LocalOnly);
    let b = build_main_backend("stub", "m".into(), &p).await.unwrap();
    assert_eq!(b.name(), "stub");
}
