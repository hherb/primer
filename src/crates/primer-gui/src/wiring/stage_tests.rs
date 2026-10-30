//! Unit tests for the per-stage helpers that `build_with_strategy`
//! composes (`backend_params`, `stores`, `subsystems`).
//!
//! The end-to-end pipeline tests live in `tests.rs`; these pin each stage
//! in isolation so a regression points at the stage that broke rather than
//! at "building an `ActiveSession` failed somewhere".

use std::cell::RefCell;
use std::time::Duration;

use primer_core::inference::InferenceBackend;
use primer_core::router::RouterMode;
use primer_engine::BackendParams;
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

/// An env lookup that records every variable name it is asked for and
/// answers from `vars`. Lets the key-gating tests run without touching the
/// process environment (`set_var` is unsafe and racy under the parallel
/// test runner).
fn recording_env<'a>(
    vars: &'a [(&'a str, &'a str)],
    asked: &'a RefCell<Vec<String>>,
) -> impl Fn(&str) -> Option<String> + 'a {
    move |name| {
        asked.borrow_mut().push(name.to_string());
        vars.iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.to_string())
    }
}

/// The cloud key is read when cloud is the primary OR (issue #206) only the
/// fallback: a local primary with a cloud fallback still needs
/// `ANTHROPIC_API_KEY`, or the cloud secondary fails with Auth.
#[test]
fn backend_params_env_cloud_key_resolved_when_cloud_primary_or_fallback() {
    for (kind, fallback) in [("cloud", None), ("ollama", Some("cloud"))] {
        let mut cfg = stub_config();
        cfg.backend.kind = kind.to_string();
        cfg.backend.fallback_backend = fallback.map(String::from);
        cfg.backend.api_key_source = ApiKeySource::Env;
        let asked = RefCell::new(Vec::new());

        let p = build_backend_params_with(
            &cfg,
            recording_env(&[("ANTHROPIC_API_KEY", "sk-env")], &asked),
        );

        assert_eq!(p.api_key.as_deref(), Some("sk-env"), "kind {kind}");
        assert!(
            asked.borrow().iter().any(|k| k == "ANTHROPIC_API_KEY"),
            "kind {kind}, asked: {:?}",
            asked.borrow()
        );
    }
}

/// With cloud neither primary nor fallback the cloud key is never read.
#[test]
fn backend_params_env_cloud_key_never_read_when_cloud_unreachable() {
    let mut cfg = stub_config();
    cfg.backend.api_key_source = ApiKeySource::Env;
    cfg.backend.fallback_backend = None;
    let asked = RefCell::new(Vec::new());

    let p = build_backend_params_with(
        &cfg,
        recording_env(&[("ANTHROPIC_API_KEY", "sk-env")], &asked),
    );

    assert_eq!(p.api_key, None);
    assert!(
        !asked.borrow().iter().any(|k| k == "ANTHROPIC_API_KEY"),
        "asked: {:?}",
        asked.borrow()
    );
}

/// The openai-compat `Env` source reads exactly the CLI's variable name.
#[test]
fn backend_params_env_openai_compat_key_reads_its_own_var() {
    let mut cfg = stub_config();
    cfg.backend.openai_compat_api_key_source = ApiKeySource::Env;
    let asked = RefCell::new(Vec::new());

    let p = build_backend_params_with(
        &cfg,
        recording_env(&[("OPENAI_COMPAT_API_KEY", "oc-env")], &asked),
    );

    assert_eq!(p.openai_compat_api_key.as_deref(), Some("oc-env"));
    assert_eq!(*asked.borrow(), ["OPENAI_COMPAT_API_KEY"]);
}

/// The production wrapper must hand `build_backend_params_with` the real
/// process env. Always passes when correct; catches a `|_| None` regression
/// on any machine where `OPENAI_COMPAT_API_KEY` happens to be set (pinning
/// it unconditionally would need `set_var`, which is racy under the
/// parallel test runner).
#[test]
fn backend_params_wrapper_reads_process_env() {
    let mut cfg = stub_config();
    cfg.backend.openai_compat_api_key_source = ApiKeySource::Env;

    let p = build_backend_params(&cfg);

    assert_eq!(
        p.openai_compat_api_key,
        std::env::var("OPENAI_COMPAT_API_KEY").ok()
    );
}

// ─── main-model rebind ───────────────────────────────────────────────

/// qnn / llamacpp only know their model id after construction; dropping
/// the rebind would persist `llm:qnn-pending` as the classifier id.
#[test]
fn rebind_main_model_takes_backend_name_for_qnn_and_llamacpp() {
    assert_eq!(
        rebind_main_model("qnn", "qnn-pending".into(), "qnn:Qwen3-4B"),
        "qnn:Qwen3-4B"
    );
    assert_eq!(
        rebind_main_model(
            "llamacpp",
            "llamacpp-pending".into(),
            "llamacpp:qwen3-4b-q4"
        ),
        "llamacpp:qwen3-4b-q4"
    );
}

#[test]
fn rebind_main_model_passes_other_kinds_through() {
    for kind in ["stub", "cloud", "ollama", "openai-compat"] {
        assert_eq!(
            rebind_main_model(kind, "configured".into(), "backend-name"),
            "configured",
            "kind {kind}"
        );
    }
}

// ─── stores ──────────────────────────────────────────────────────────

/// With no explicit `session_db` the DB lands at `<home>/.primer/<slug>.db`
/// under the injected home, creating the directory.
#[tokio::test]
async fn default_session_db_is_created_under_injected_home() {
    let home = TempDir::new().unwrap();
    let mut cfg = stub_config();
    cfg.persistence.no_persist = false;
    cfg.persistence.session_db = None;
    cfg.learner.name = "José".to_string();

    open_session_store_and_learner(home.path(), &cfg, Locale::English, LocaleStrategy::UseCfg)
        .await
        .unwrap();

    assert!(home.path().join(".primer").join("josé.db").is_file());
}

/// Resume must open the KB under the persisted learner's locale, not the
/// cfg's: a German learner resumed under an English cfg gets the German KB.
#[tokio::test]
async fn resume_opens_knowledge_base_under_inherited_locale() {
    let home = TempDir::new().unwrap();
    let mut cfg = stub_config();
    cfg.persistence.no_persist = false;
    cfg.persistence.session_db = Some(home.path().join("learner.db"));
    let (first, _, _) =
        open_session_store_and_learner(home.path(), &cfg, Locale::German, LocaleStrategy::UseCfg)
            .await
            .unwrap();
    drop(first);

    let stores = open_stores(
        home.path(),
        &cfg,
        Locale::English,
        LocaleStrategy::InheritFromPersistedLearner,
        None,
    )
    .await
    .unwrap();

    assert_eq!(stores.locale, Locale::German);
    assert_eq!(stores.knowledge.locale(), Locale::German);
}

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
async fn knowledge_base_without_embedder_is_in_memory_and_auto_seeds() {
    let home = TempDir::new().unwrap();
    let cfg = stub_config();
    assert!(cfg.persistence.knowledge_db.is_none());

    let kb = open_knowledge_base(home.path(), &cfg, Locale::English, None)
        .await
        .unwrap();

    assert!(
        kb.passage_count().unwrap() > 0,
        "an empty in-memory KB must be auto-seeded from the bundled corpus"
    );
    assert!(
        !home.path().join(".primer").exists(),
        "no embedder ⇒ nothing to cache on disk"
    );
}

/// Issue #388: with an embedder wired, the shipped wiring embeds the whole
/// bundled corpus into the per-model cache, so the hybrid vector leg (and
/// the #307 gate) works on a default run. The next session build reuses
/// the file and embeds nothing.
#[tokio::test]
async fn knowledge_base_with_embedder_is_an_embedded_per_model_cache() {
    let home = TempDir::new().unwrap();
    let mut cfg = stub_config();
    cfg.persistence.no_persist = false;
    let stub = primer_embedding::StubEmbedder::new();
    let cache = primer_engine::resolve_knowledge_db(
        None,
        Some(home.path()),
        Some(primer_core::embedder::Embedder::model_id(&stub)),
        false,
    )
    .path();

    let kb = open_knowledge_base(home.path(), &cfg, Locale::English, Some(&stub))
        .await
        .unwrap();
    let passages = kb.passage_count().unwrap();
    assert!(passages > 0);
    assert_eq!(kb.embedding_count().unwrap(), passages);
    assert!(cache.is_file(), "expected the cache at {}", cache.display());
    drop(kb);

    let again = open_knowledge_base(home.path(), &cfg, Locale::English, Some(&stub))
        .await
        .unwrap();
    assert_eq!(
        again.passage_count().unwrap(),
        passages,
        "top-up adds no duplicates"
    );
    assert!(again.passages_missing_embedding().unwrap().is_empty());

    // A gate above any cosine fires on every query, and with every passage
    // embedded nothing escapes it: the #307 gate is live on this path.
    let always_gated = primer_core::knowledge::HybridParams {
        min_top_cosine: Some(GATE_ABOVE_ANY_COSINE),
        ..Default::default()
    };
    let hits = primer_core::knowledge::KnowledgeBase::retrieve_hybrid(
        &again,
        "why is the sky blue",
        &stub,
        &always_gated,
    )
    .await
    .unwrap();
    assert!(hits.is_empty(), "{hits:?}");
}

/// Cosine similarity never exceeds 1.0, so this gate always fires.
const GATE_ABOVE_ANY_COSINE: f64 = 2.0;

/// `no_persist` writes nothing, but the in-memory KB is still embedded.
#[tokio::test]
async fn knowledge_base_no_persist_embeds_in_memory() {
    let home = TempDir::new().unwrap();
    let cfg = stub_config();
    assert!(cfg.persistence.no_persist);
    let stub = primer_embedding::StubEmbedder::new();

    let kb = open_knowledge_base(home.path(), &cfg, Locale::English, Some(&stub))
        .await
        .unwrap();

    assert_eq!(kb.embedding_count().unwrap(), kb.passage_count().unwrap());
    assert!(!home.path().join(".primer").exists());
}

/// An explicit Settings path is the user's to manage: seeded when empty,
/// never embedded behind their back.
#[tokio::test]
async fn knowledge_base_explicit_path_is_seeded_but_not_embedded() {
    let home = TempDir::new().unwrap();
    let mut cfg = stub_config();
    cfg.persistence.knowledge_db = Some(home.path().join("my-kb.db"));
    let stub = primer_embedding::StubEmbedder::new();

    let kb = open_knowledge_base(home.path(), &cfg, Locale::English, Some(&stub))
        .await
        .unwrap();

    assert!(kb.passage_count().unwrap() > 0);
    assert_eq!(kb.embedding_count().unwrap(), 0);
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
/// plumbing, not `Arc::clone` inheritance (that is
/// `subsystems_inherit_main_backend_via_arc_clone`).
#[tokio::test]
async fn subsystems_match_main_stub_and_apply_gui_settings() {
    let mut cfg = stub_config();
    cfg.classifier.timeout_ms = CUSTOM_CLASSIFIER_TIMEOUT_MS;
    cfg.extractor.timeout_ms = CUSTOM_EXTRACTOR_TIMEOUT_MS;
    cfg.comprehension.timeout_ms = CUSTOM_COMPREHENSION_TIMEOUT_MS;
    cfg.vocab.max_per_prompt = Some(CUSTOM_VOCAB_MAX_PER_PROMPT);
    let (backend, params) = stub_backend(&cfg).await;

    let s = build_subsystems(&cfg, &backend, &cfg.backend.kind, "stub", &params)
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

/// A non-stub main kind with no subsystem override reuses the main backend
/// via `Arc::clone`. The main backend handed in is a `StubBackend` (named
/// "stub") under an `openai-compat` config, so the "stub" in the extractor /
/// comprehension ids proves reuse: a freshly built backend would be named
/// "openai-compat". The classifier id (`llm:<model>`) carries no backend
/// name, so its assertion pins only the model id, not reuse (#362).
#[tokio::test]
async fn subsystems_inherit_main_backend_via_arc_clone() {
    const MAIN_MODEL: &str = "inherited-model";
    let mut cfg = stub_config();
    cfg.backend.kind = "openai-compat".to_string();
    cfg.backend.model = Some(MAIN_MODEL.to_string());
    let params = build_backend_params(&cfg);
    let main: Arc<dyn InferenceBackend> = Arc::new(primer_inference::stub::StubBackend);

    let s = build_subsystems(&cfg, &main, &cfg.backend.kind, MAIN_MODEL, &params)
        .await
        .unwrap();

    // Classifier and extractor/comprehension id formats differ (#316).
    assert_eq!(s.classifier.identifier(), format!("llm:{MAIN_MODEL}"));
    assert_eq!(s.extractor.identifier(), format!("llm:stub:{MAIN_MODEL}"));
    assert_eq!(
        s.comprehension.identifier(),
        format!("llm:stub:{MAIN_MODEL}")
    );
}

#[tokio::test]
async fn subsystems_vocab_cap_defaults_to_const_when_unset() {
    let mut cfg = stub_config();
    cfg.vocab.max_per_prompt = None;
    let (backend, params) = stub_backend(&cfg).await;

    let s = build_subsystems(&cfg, &backend, &cfg.backend.kind, "stub", &params)
        .await
        .unwrap();

    assert_eq!(
        s.vocab_settings.max_per_prompt,
        primer_core::consts::vocab::DEFAULT_VOCAB_MAX_PER_PROMPT
    );
}

// ─── retrieval notice (issue #353) ───────────────────────────────────

/// A configured `none` is intentional BM25-only, and a built embedder means
/// hybrid retrieval is on: neither gets a notice.
#[test]
fn retrieval_notice_absent_when_intentional_or_built() {
    assert_eq!(retrieval_notice("none", false), None);
    for kind in ["stub", "fastembed", "ollama", "openai-compat"] {
        assert_eq!(retrieval_notice(kind, true), None, "kind {kind}");
    }
}

/// A real embedder that failed to start (e.g. the BGE-M3 download offline)
/// silently degrades to BM25-only; the notice names the embedder.
#[test]
fn retrieval_notice_reports_failed_embedder() {
    let notice = retrieval_notice("fastembed", false).expect("failed embedder must be reported");
    assert!(notice.contains("fastembed"), "notice: {notice}");
    assert!(notice.contains("BM25"), "notice: {notice}");
}
