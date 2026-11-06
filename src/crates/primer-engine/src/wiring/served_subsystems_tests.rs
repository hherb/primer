//! `build_served_subsystems` under `BackendNotice::PrimaryUnavailable`
//! (issue #364): the one helper both binaries call must build every
//! subsystem on the *served* fallback, never the unavailable primary.

use super::*;
use primer_classifier::ClassifierSettings;
use primer_comprehension::ComprehensionSettings;
use primer_core::router::RouterMode;
use primer_extractor::ExtractorSettings;

const FALLBACK_MODEL: &str = "fallback-model";
const OVERRIDE_MODEL: &str = "classifier-override";

fn params() -> BackendParams {
    BackendParams {
        api_key: None,
        ollama_url: "http://localhost:11434".into(),
        openai_compat_url: "http://localhost:8000".into(),
        openai_compat_api_key: None,
        classifier_backend: None,
        classifier_model: Some(OVERRIDE_MODEL.into()),
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
        fallback_backend: Some("ollama".into()),
        fallback_model: Some(FALLBACK_MODEL.into()),
        router_mode: RouterMode::LocalOnly,
        primary_ttft_budget_ms: None,
    }
}

fn settings() -> SubsystemSettings {
    SubsystemSettings {
        classifier: ClassifierSettings::default(),
        extractor: ExtractorSettings::default(),
        comprehension: ComprehensionSettings::default(),
    }
}

fn primary_unavailable() -> BackendNotice {
    BackendNotice::PrimaryUnavailable {
        primary: "qnn".into(),
        fallback: "ollama".into(),
        fallback_model: Some(FALLBACK_MODEL.into()),
        reason: "no NPU on this host".into(),
    }
}

/// `qnn` cannot build on a host, so a helper that dispatched on the
/// configured primary would fail the classifier's model-only override
/// (#351). Built on the served `ollama` fallback it constructs, and the
/// inherited subsystems are labelled with the fallback's model.
#[tokio::test]
async fn primary_unavailable_builds_every_subsystem_on_the_fallback() {
    let params = params();
    // The Arc is a stub; only the kind/model strings drive dispatch.
    let backend = build_backend("stub", "served".into(), &params)
        .await
        .unwrap();
    let notice = primary_unavailable();

    let built = build_served_subsystems(
        &backend,
        "qnn",
        "qnn-pending",
        Some(&notice),
        &params,
        &settings(),
    )
    .await
    .expect("subsystems must build on the served fallback, not the qnn primary");

    assert_eq!(built.served_kind, "ollama");
    assert_eq!(built.served_model, FALLBACK_MODEL);
    assert_eq!(
        built.classifier.identifier(),
        format!("llm:{OVERRIDE_MODEL}")
    );
    for id in [
        built.extractor.identifier(),
        built.comprehension.identifier(),
    ] {
        assert!(
            id.contains(FALLBACK_MODEL),
            "inherited id {id:?} must name the fallback"
        );
        assert!(
            !id.contains("qnn"),
            "inherited id {id:?} must not name the primary"
        );
    }
}

/// Without a notice the configured primary is what serves.
#[tokio::test]
async fn no_notice_builds_on_the_configured_primary() {
    let mut params = params();
    params.classifier_model = None;
    let backend = build_backend("stub", "served".into(), &params)
        .await
        .unwrap();

    let built = build_served_subsystems(&backend, "ollama", "main", None, &params, &settings())
        .await
        .unwrap();

    assert_eq!(
        (built.served_kind.as_str(), built.served_model.as_str()),
        ("ollama", "main")
    );
    assert_eq!(built.classifier.identifier(), "llm:main");
}

/// A failing subsystem names itself in the error, as both binaries used to.
#[tokio::test]
async fn failing_subsystem_is_named_in_the_error() {
    let mut params = params();
    params.extractor_backend = Some("nonexistent".into());
    params.extractor_model = Some("m".into());
    let backend = build_backend("stub", "served".into(), &params)
        .await
        .unwrap();

    let err = build_served_subsystems(&backend, "ollama", "main", None, &params, &settings())
        .await
        .err()
        .expect("unknown extractor backend must fail");

    assert!(
        err.starts_with("constructing concept extractor: "),
        "got {err:?}"
    );
}
