//! Unit tests for the per-stage helpers that `build_with_strategy`
//! composes (`backend_params`, `stores`, `subsystems`).
//!
//! The end-to-end pipeline tests live in `tests.rs`; these pin each stage
//! in isolation so a regression points at the stage that broke rather than
//! at "building an `ActiveSession` failed somewhere".

use std::time::Duration;

use primer_core::inference::InferenceBackend;
use primer_core::router::RouterMode;
use primer_engine::{BackendNotice, BackendParams};
use tempfile::TempDir;

use super::*;
use crate::config::{ApiKeySource, SubsystemConfig};

/// Per-subsystem timeouts, distinct from each other and from the
/// `*Settings::default().blocking_timeout` values, so the test proves each
/// GUI value (not a default, not a sibling's) reached its settings.
const CUSTOM_CLASSIFIER_TIMEOUT_MS: u64 = 1234;
const CUSTOM_EXTRACTOR_TIMEOUT_MS: u64 = 2345;
const CUSTOM_COMPREHENSION_TIMEOUT_MS: u64 = 3456;
/// Vocab cap distinct from `DEFAULT_VOCAB_MAX_PER_PROMPT`.
const CUSTOM_VOCAB_MAX_PER_PROMPT: u64 = 2;
/// An arbitrary non-`None` latency budget for the pass-through check.
const CUSTOM_TTFT_BUDGET_MS: u64 = 1500;

/// Smallest config that runs the whole stub pipeline with nothing on disk
/// and no embedder download (the default build's embedder is fastembed).
fn stub_config() -> GuiConfig {
    let mut cfg = GuiConfig::default();
    cfg.persistence.no_persist = true;
    cfg.embedder.kind = "none".to_string();
    cfg
}

// ─── backend_params ──────────────────────────────────────────────────

#[test]
fn backend_params_match_main_subsystems_carry_no_kind_override() {
    let mut cfg = stub_config();
    // A stale `kind` or `model` left behind while "match main" is ticked
    // must be ignored — otherwise the subsystem silently stops inheriting
    // (a model override alone builds a separate backend; issue #335).
    cfg.extractor.kind = Some("cloud".to_string());
    cfg.classifier.model = Some("stale-model".to_string());
    cfg.comprehension.model = Some("stale-model".to_string());
    assert!(cfg.extractor.match_main);
    assert!(cfg.classifier.match_main);
    assert!(cfg.comprehension.match_main);

    let p = build_backend_params(&cfg);

    assert_eq!(p.classifier_backend, None);
    assert_eq!(p.extractor_backend, None);
    assert_eq!(p.comprehension_backend, None);
    assert_eq!(p.classifier_model, None);
    assert_eq!(p.extractor_model, None);
    assert_eq!(p.comprehension_model, None);
}

#[test]
fn backend_params_explicit_subsystem_override_passes_through() {
    let mut cfg = stub_config();
    cfg.classifier = SubsystemConfig {
        match_main: false,
        kind: Some("ollama".to_string()),
        model: Some("llama3.2".to_string()),
        timeout_ms: cfg.classifier.timeout_ms,
    };

    let p = build_backend_params(&cfg);

    assert_eq!(p.classifier_backend.as_deref(), Some("ollama"));
    assert_eq!(p.classifier_model.as_deref(), Some("llama3.2"));
}

#[test]
fn backend_params_inline_keys_are_used_regardless_of_backend_kind() {
    let mut cfg = stub_config();
    assert_eq!(cfg.backend.kind, "stub");
    cfg.backend.api_key_source = ApiKeySource::Inline {
        key: "sk-inline".to_string(),
    };
    cfg.backend.openai_compat_api_key_source = ApiKeySource::Inline {
        key: "oc-inline".to_string(),
    };

    let p = build_backend_params(&cfg);

    assert_eq!(p.api_key.as_deref(), Some("sk-inline"));
    assert_eq!(p.openai_compat_api_key.as_deref(), Some("oc-inline"));
}

#[test]
fn backend_params_env_cloud_key_skipped_when_cloud_unreachable() {
    // Deterministic regardless of the test process's environment: with no
    // cloud primary and no cloud fallback the env var is never consulted.
    let mut cfg = stub_config();
    cfg.backend.api_key_source = ApiKeySource::Env;
    cfg.backend.fallback_backend = None;

    let p = build_backend_params(&cfg);

    assert_eq!(p.api_key, None);
}

#[test]
fn backend_params_parses_reasoning_markers_and_passes_routing_through() {
    let mut cfg = stub_config();
    cfg.backend.reasoning_markers = "<r> </r>\n".to_string();
    cfg.backend.fallback_backend = Some("cloud".to_string());
    cfg.backend.fallback_model = Some("claude-sonnet-4-6".to_string());
    cfg.backend.router_mode = RouterMode::Hybrid;
    cfg.backend.primary_ttft_budget_ms = Some(CUSTOM_TTFT_BUDGET_MS);

    let p = build_backend_params(&cfg);

    assert_eq!(
        p.reasoning_markers,
        vec![("<r>".to_string(), "</r>".to_string())]
    );
    assert_eq!(p.fallback_backend.as_deref(), Some("cloud"));
    assert_eq!(p.fallback_model.as_deref(), Some("claude-sonnet-4-6"));
    assert_eq!(p.router_mode, RouterMode::Hybrid);
    assert_eq!(p.primary_ttft_budget_ms, Some(CUSTOM_TTFT_BUDGET_MS));
}

// ─── stores ──────────────────────────────────────────────────────────

#[tokio::test]
async fn fresh_store_mints_learner_under_cfg_locale() {
    let home = TempDir::new().unwrap();
    let mut cfg = stub_config();
    cfg.learner.name = "Ada".to_string();
    cfg.learner.age = 9;

    let (store, locale, learner) = open_session_store_and_learner(
        home.path(),
        &cfg,
        Locale::German,
        LocaleStrategy::InheritFromPersistedLearner,
    )
    .await
    .unwrap();

    // No persisted learner ⇒ cfg wins regardless of strategy.
    assert_eq!(locale, Locale::German);
    assert_eq!(store.locale(), Locale::German);
    assert_eq!(learner.profile.locale, Locale::German);
    assert_eq!(learner.profile.name, "Ada");
    assert_eq!(learner.profile.age, 9);
}

#[tokio::test]
async fn inherit_strategy_retags_store_to_persisted_learner_locale() {
    let home = TempDir::new().unwrap();
    let mut cfg = stub_config();
    cfg.persistence.no_persist = false;
    cfg.persistence.session_db = Some(home.path().join("learner.db"));

    // First open mints and persists a German learner.
    let (first, _, _) =
        open_session_store_and_learner(home.path(), &cfg, Locale::German, LocaleStrategy::UseCfg)
            .await
            .unwrap();
    drop(first);

    // Resume under an English cfg: the persisted German locale wins and
    // the store is re-tagged in place.
    let (store, locale, learner) = open_session_store_and_learner(
        home.path(),
        &cfg,
        Locale::English,
        LocaleStrategy::InheritFromPersistedLearner,
    )
    .await
    .unwrap();

    assert_eq!(locale, Locale::German);
    assert_eq!(store.locale(), Locale::German);
    assert_eq!(learner.profile.locale, Locale::German);
}

#[tokio::test]
async fn knowledge_base_defaults_to_in_memory_and_auto_seeds() {
    let cfg = stub_config();
    assert!(cfg.persistence.knowledge_db.is_none());

    let kb = open_knowledge_base(&cfg, Locale::English).await.unwrap();

    assert!(
        kb.passage_count().unwrap() > 0,
        "an empty in-memory KB must be auto-seeded from the bundled corpus"
    );
}

// ─── subsystems ──────────────────────────────────────────────────────

/// Build the stub main backend plus the params it was built from.
async fn stub_backend(cfg: &GuiConfig) -> (Arc<dyn InferenceBackend>, BackendParams) {
    let params = build_backend_params(cfg);
    let (backend, _) = build_main_backend("stub", "stub".to_string(), &params)
        .await
        .unwrap();
    (backend, params)
}

/// With a stub main backend `primer-engine` takes its `("stub", None)` arm
/// and builds a fresh stub per subsystem, so this pins the GUI-settings
/// plumbing, not `Arc::clone` inheritance (that gap is tracked in #340).
#[tokio::test]
async fn subsystems_match_main_stub_and_apply_gui_settings() {
    let mut cfg = stub_config();
    cfg.classifier.timeout_ms = CUSTOM_CLASSIFIER_TIMEOUT_MS;
    cfg.extractor.timeout_ms = CUSTOM_EXTRACTOR_TIMEOUT_MS;
    cfg.comprehension.timeout_ms = CUSTOM_COMPREHENSION_TIMEOUT_MS;
    cfg.vocab.max_per_prompt = Some(CUSTOM_VOCAB_MAX_PER_PROMPT);
    let (backend, params) = stub_backend(&cfg).await;

    let s = build_subsystems(&cfg, &backend, "stub", &params)
        .await
        .unwrap();

    assert_eq!(s.classifier.identifier(), "stub");
    assert_eq!(
        s.classifier_settings.blocking_timeout,
        Duration::from_millis(CUSTOM_CLASSIFIER_TIMEOUT_MS)
    );
    assert_eq!(
        s.extractor_settings.blocking_timeout,
        Duration::from_millis(CUSTOM_EXTRACTOR_TIMEOUT_MS)
    );
    assert_eq!(
        s.comprehension_settings.blocking_timeout,
        Duration::from_millis(CUSTOM_COMPREHENSION_TIMEOUT_MS)
    );
    assert_eq!(
        s.vocab_settings.max_per_prompt,
        CUSTOM_VOCAB_MAX_PER_PROMPT as usize
    );
    assert!(s.embedder.is_none(), "embedder kind \"none\" ⇒ BM25-only");
}

#[tokio::test]
async fn subsystems_vocab_cap_defaults_to_const_when_unset() {
    let mut cfg = stub_config();
    cfg.vocab.max_per_prompt = None;
    let (backend, params) = stub_backend(&cfg).await;

    let s = build_subsystems(&cfg, &backend, "stub", &params)
        .await
        .unwrap();

    assert_eq!(
        s.vocab_settings.max_per_prompt,
        primer_core::consts::vocab::DEFAULT_VOCAB_MAX_PER_PROMPT
    );
}

// ─── served identity (issue #338) ────────────────────────────────────

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
