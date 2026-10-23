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
    let (b, notice) = build_main_backend("stub", "m".into(), &p).await.unwrap();
    assert_eq!(b.name(), "stub");
    assert_eq!(notice, None);
}

/// Primary fails to build, fallback (stub) builds ⇒ secondary alone.
/// `unknown-backend` is an unbuildable backend name, so the primary leg errors.
#[tokio::test]
async fn primary_unbuildable_falls_back_to_secondary() {
    let p = params(Some("stub"), None, RouterMode::LocalOnly);
    let (b, _) = build_main_backend("unknown-backend", "m".into(), &p)
        .await
        .unwrap();
    // Secondary stub served ⇒ its name surfaces.
    assert_eq!(b.name(), "stub");
}

/// Primary builds, fallback fails to build ⇒ primary alone, no error.
#[tokio::test]
async fn fallback_unbuildable_keeps_primary() {
    let p = params(Some("unknown-backend"), Some("m"), RouterMode::LocalOnly);
    let (b, _) = build_main_backend("stub", "m".into(), &p).await.unwrap();
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
    let (b, _) = build_main_backend("stub", "m".into(), &p).await.unwrap();
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
    let (b, _) = build_main_backend("stub", "m".into(), &p).await.unwrap();
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
    let (b, _) = build_main_backend("stub", "m".into(), &p).await.unwrap();
    assert_eq!(b.name(), "stub");
}

/// Primary down, fallback built ⇒ the notice names both legs so the GUI can
/// show which backend actually serves turns (issue #338).
#[tokio::test]
async fn primary_unbuildable_reports_primary_unavailable_notice() {
    let p = params(Some("stub"), None, RouterMode::LocalOnly);
    let (_, notice) = build_main_backend("unknown-primary", "m".into(), &p)
        .await
        .unwrap();
    match notice {
        Some(BackendNotice::PrimaryUnavailable {
            primary,
            fallback,
            fallback_model,
            reason,
        }) => {
            assert_eq!(primary, "unknown-primary");
            assert_eq!(fallback, "stub");
            assert_eq!(fallback_model, None);
            assert!(reason.contains("unknown-primary"), "reason: {reason}");
        }
        other => panic!("expected PrimaryUnavailable, got {other:?}"),
    }
}

/// Healthy primary, misconfigured fallback ⇒ a FallbackDropped notice carrying
/// the resolve error, instead of a warn-only log line (issue #347).
#[tokio::test]
async fn misconfigured_fallback_reports_fallback_dropped_notice() {
    let p = params(Some("ollama"), None, RouterMode::LocalOnly);
    let (_, notice) = build_main_backend("stub", "m".into(), &p).await.unwrap();
    match notice {
        Some(BackendNotice::FallbackDropped { fallback, reason }) => {
            assert_eq!(fallback, "ollama");
            assert!(reason.contains("--fallback-model"), "reason: {reason}");
        }
        other => panic!("expected FallbackDropped, got {other:?}"),
    }
}

/// Both legs built ⇒ no notice, for the fallback wrap and the router alike.
#[tokio::test]
async fn wrapped_legs_report_no_notice() {
    for mode in [RouterMode::LocalOnly, RouterMode::Hybrid] {
        let p = params(Some("stub"), None, mode);
        let (_, notice) = build_main_backend("stub", "m".into(), &p).await.unwrap();
        assert_eq!(notice, None, "mode {mode}");
    }
}

/// The router path reports the same degradations as the fallback path.
#[tokio::test]
async fn router_degradations_report_notices() {
    let p = params(Some("unknown-fallback"), Some("m"), RouterMode::Hybrid);
    let (_, notice) = build_main_backend("stub", "m".into(), &p).await.unwrap();
    assert!(
        matches!(&notice, Some(BackendNotice::FallbackDropped { fallback, .. }) if fallback == "unknown-fallback"),
        "got {notice:?}"
    );

    let p = params(Some("stub"), None, RouterMode::Hybrid);
    let (_, notice) = build_main_backend("unknown-primary", "m".into(), &p)
        .await
        .unwrap();
    assert!(
        matches!(notice, Some(BackendNotice::PrimaryUnavailable { .. })),
        "got {notice:?}"
    );
}

/// The user-facing text names the dropped leg and the reason.
#[test]
fn notice_messages_name_legs_and_reason() {
    let dropped = BackendNotice::FallbackDropped {
        fallback: "cloud".into(),
        reason: "no API key".into(),
    };
    let msg = dropped.to_string();
    assert!(msg.contains("cloud") && msg.contains("no API key"), "{msg}");

    let down = BackendNotice::PrimaryUnavailable {
        primary: "ollama".into(),
        fallback: "cloud".into(),
        fallback_model: Some("claude-sonnet-4-6".into()),
        reason: "connection refused".into(),
    };
    let msg = down.to_string();
    assert!(
        msg.contains("ollama") && msg.contains("cloud") && msg.contains("connection refused"),
        "{msg}"
    );
}

/// A cloud fallback carries its *resolved* model in the notice (the default
/// when unset, an explicit one when given), for both paths — this is what the
/// GUI sidebar shows instead of the backend's `name()` (#338).
#[tokio::test]
async fn primary_unavailable_notice_carries_resolved_fallback_model() {
    use primer_core::consts::inference::DEFAULT_CLOUD_MODEL;
    for mode in [RouterMode::LocalOnly, RouterMode::Hybrid] {
        for (configured, expected) in [
            (None, DEFAULT_CLOUD_MODEL),
            (Some("claude-opus-4-7"), "claude-opus-4-7"),
        ] {
            let mut p = params(Some("cloud"), configured, mode);
            p.api_key = Some("test-key".into());
            let (_, notice) = build_main_backend("unknown-primary", "m".into(), &p)
                .await
                .unwrap();
            assert!(
                matches!(
                    &notice,
                    Some(BackendNotice::PrimaryUnavailable { fallback, fallback_model: Some(m), .. })
                        if fallback == "cloud" && m == expected
                ),
                "mode {mode}, configured {configured:?}: got {notice:?}"
            );
        }
    }
}

// ─── served identity (issues #338, #351) ─────────────────────────────

/// With no notice, or a dropped fallback, the primary is what serves turns,
/// so the sidebar keeps showing the configured kind + model.
#[test]
fn served_identity_is_primary_unless_primary_unavailable() {
    let dropped = BackendNotice::FallbackDropped {
        fallback: "cloud".into(),
        reason: "no key".into(),
    };
    for notice in [None, Some(&dropped)] {
        assert_eq!(
            served_identity("ollama", "llama3.2", notice, "ollama"),
            ("ollama".to_string(), "llama3.2".to_string())
        );
    }
}

/// Primary down ⇒ the sidebar shows the fallback that actually serves turns.
#[test]
fn served_identity_shows_fallback_when_primary_unavailable() {
    let notice = BackendNotice::PrimaryUnavailable {
        primary: "ollama".into(),
        fallback: "cloud".into(),
        fallback_model: Some("claude-sonnet-4-6".into()),
        reason: "connection refused".into(),
    };
    assert_eq!(
        served_identity("ollama", "llama3.2", Some(&notice), "claude-sonnet-4-6"),
        ("cloud".to_string(), "claude-sonnet-4-6".to_string())
    );

    // A model-less fallback (stub) falls back to the served backend's name.
    let notice = BackendNotice::PrimaryUnavailable {
        primary: "ollama".into(),
        fallback: "stub".into(),
        fallback_model: None,
        reason: "x".into(),
    };
    assert_eq!(
        served_identity("ollama", "llama3.2", Some(&notice), "stub"),
        ("stub".to_string(), "stub".to_string())
    );
}
