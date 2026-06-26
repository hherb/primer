use super::*;
use tempfile::TempDir;

/// Build the smallest config that triggers the full stub pipeline.
fn stub_config() -> GuiConfig {
    let mut cfg = GuiConfig::default();
    cfg.persistence.no_persist = true;
    cfg
}

#[tokio::test]
async fn builds_active_session_with_stub_backend() {
    let home = TempDir::new().unwrap();
    let cfg = stub_config();
    let s = build_active_session(home.path(), &cfg).await.unwrap();

    assert_eq!(s.backend_name, "stub");
    assert_eq!(s.main_model, "stub");
    assert_eq!(s.locale, Locale::English);
    // The DM is constructed but no turn has run yet, so its
    // session is empty (no on-disk row yet).
    let dm = s.dialogue_manager.lock().await;
    assert!(
        dm.session.turns.is_empty(),
        "no turns until first send_message"
    );
    // Subsystems all default to stub when the main backend is stub.
    assert_eq!(dm.classifier_identifier(), "stub");
    assert_eq!(dm.extractor_identifier(), "stub");
    assert_eq!(dm.comprehension_identifier(), "stub");
    // The learner row is freshly minted; name matches config.
    assert_eq!(dm.learner.profile.name, "Explorer");
    assert_eq!(dm.learner.profile.age, 8);
}

#[tokio::test]
async fn unknown_locale_errors() {
    let home = TempDir::new().unwrap();
    let mut cfg = stub_config();
    cfg.learner.locale = "klingon".to_string();
    let err = build_active_session(home.path(), &cfg).await.unwrap_err();
    assert!(
        err.contains("klingon"),
        "error must name the offending locale: {err}"
    );
}

#[tokio::test]
async fn ollama_without_model_errors() {
    let home = TempDir::new().unwrap();
    let mut cfg = stub_config();
    cfg.backend.kind = "ollama".to_string();
    cfg.backend.model = None;
    let err = build_active_session(home.path(), &cfg).await.unwrap_err();
    assert!(
        err.to_lowercase().contains("ollama"),
        "error must mention ollama: {err}"
    );
}

#[tokio::test]
async fn unknown_backend_errors() {
    let home = TempDir::new().unwrap();
    let mut cfg = stub_config();
    cfg.backend.kind = "magic".to_string();
    let err = build_active_session(home.path(), &cfg).await.unwrap_err();
    assert!(
        err.contains("magic"),
        "error must name the offending backend: {err}"
    );
}

#[tokio::test]
async fn openai_compat_without_model_errors() {
    let home = TempDir::new().unwrap();
    let mut cfg = stub_config();
    cfg.backend.kind = "openai-compat".to_string();
    cfg.backend.model = None;
    let err = build_active_session(home.path(), &cfg).await.unwrap_err();
    assert!(
        err.to_lowercase().contains("openai-compat") && err.to_lowercase().contains("model"),
        "error must mention openai-compat and the missing model: {err}"
    );
}

#[tokio::test]
async fn openai_compat_with_model_constructs() {
    // The openai-compat backend constructs without a network call
    // (it's just an HTTP client + model id). Selecting it with a
    // model id and the default localhost:8000 URL must succeed.
    let home = TempDir::new().unwrap();
    let mut cfg = stub_config();
    cfg.backend.kind = "openai-compat".to_string();
    cfg.backend.model = Some("mlx-community/Qwen3-8B-4bit".to_string());
    let s = build_active_session(home.path(), &cfg).await.unwrap();
    assert_eq!(s.backend_name, "openai-compat");
    assert_eq!(s.main_model, "mlx-community/Qwen3-8B-4bit");
}

#[tokio::test]
async fn openai_compat_embedder_without_model_errors() {
    // Independent of the `openai-compat-embedding` cargo feature: with
    // no embedder model the build fails (feature-absent → feature
    // error; feature-present → model-required error). Either way the
    // GUI surfaces an error instead of silently degrading.
    let home = TempDir::new().unwrap();
    let mut cfg = stub_config();
    cfg.embedder.kind = "openai-compat".to_string();
    cfg.embedder.model = None;
    let err = build_active_session(home.path(), &cfg).await.unwrap_err();
    assert!(
        err.to_lowercase().contains("openai-compat"),
        "error must mention the openai-compat embedder: {err}"
    );
}

#[tokio::test]
async fn unknown_embedder_errors() {
    let home = TempDir::new().unwrap();
    let mut cfg = stub_config();
    cfg.embedder.kind = "secret-sauce".to_string();
    let err = build_active_session(home.path(), &cfg).await.unwrap_err();
    assert!(
        err.contains("secret-sauce"),
        "error must name the offending embedder: {err}"
    );
}

#[test]
fn cloud_key_needed_when_cloud_is_primary() {
    assert!(cloud_backend_in_use("cloud", None));
    assert!(cloud_backend_in_use("cloud", Some("ollama")));
}

#[test]
fn cloud_key_needed_when_cloud_is_fallback_of_local_primary() {
    // The supported fallback direction is local-primary → cloud-fallback
    // (issue #205 follow-up). The cloud key must resolve even though the
    // primary `kind` is not "cloud", or the cloud secondary fails to build
    // with an Auth error and the fallback silently degrades to PrimaryAlone.
    assert!(cloud_backend_in_use("llamacpp", Some("cloud")));
    assert!(cloud_backend_in_use("ollama", Some("cloud")));
}

#[test]
fn cloud_key_not_needed_when_cloud_absent() {
    assert!(!cloud_backend_in_use("ollama", None));
    assert!(!cloud_backend_in_use("llamacpp", Some("openai-compat")));
    assert!(!cloud_backend_in_use("stub", Some("ollama")));
}

#[test]
fn resolve_main_model_qnn_returns_placeholder() {
    // The qnn model id is read from the bundle at construction, so the
    // override is ignored and a placeholder is returned (rebound to
    // `backend.name()` after the backend constructs).
    assert_eq!(resolve_main_model("qnn", None).unwrap(), "qnn-pending");
    assert_eq!(
        resolve_main_model("qnn", Some("ignored-model")).unwrap(),
        "qnn-pending",
        "the model override is ignored for qnn"
    );
}

#[cfg(not(feature = "qnn"))]
#[tokio::test]
async fn qnn_without_feature_surfaces_build_hint() {
    // On a default (non-`qnn`-feature) GUI build, selecting the qnn
    // backend — even with a bundle dir set — must surface the
    // "rebuild with the qnn cargo feature" hint inline rather than
    // panicking or silently falling back. This is the "error inline"
    // contract for the always-show QNN option.
    let home = TempDir::new().unwrap();
    let mut cfg = stub_config();
    cfg.backend.kind = "qnn".to_string();
    cfg.backend.qnn_bundle_dir = Some("/some/bundle".into());
    let err = build_active_session(home.path(), &cfg).await.unwrap_err();
    let lower = err.to_lowercase();
    assert!(
        lower.contains("qnn") && lower.contains("feature"),
        "error must mention qnn and the missing cargo feature: {err}"
    );
}

#[tokio::test]
async fn cloud_with_inline_api_key_constructs() {
    // Inline key bypasses the ANTHROPIC_API_KEY env var entirely —
    // exercise the wiring branch that resolves Inline → Some(key).
    let home = TempDir::new().unwrap();
    let mut cfg = stub_config();
    cfg.backend.kind = "cloud".to_string();
    cfg.backend.api_key_source = crate::config::ApiKeySource::Inline {
        key: "sk-test-not-real".to_string(),
    };
    let s = build_active_session(home.path(), &cfg).await.unwrap();
    assert_eq!(s.backend_name, "cloud");
    assert_eq!(s.main_model, "claude-sonnet-4-6");
}

#[tokio::test]
async fn stub_embedder_kind_succeeds() {
    let home = TempDir::new().unwrap();
    let mut cfg = stub_config();
    cfg.embedder.kind = "stub".to_string();
    let s = build_active_session(home.path(), &cfg).await.unwrap();
    let dm = s.dialogue_manager.lock().await;
    assert_eq!(
        dm.embedder_identifier(),
        Some(primer_embedding::STUB_MODEL_ID),
        "stub embedder must be wired into the DM"
    );
}

#[tokio::test]
async fn second_build_after_first_drops_persists_learner_growth() {
    // Models the GUI's start_session → close_session → start_session
    // round-trip at the wiring layer: each build_active_session
    // call independently re-opens the on-disk learner. Validates
    // that the second open sees a stable UUID (no orphaned learner
    // rows) — the most important invariant of the close+restart
    // flow.
    let home = TempDir::new().unwrap();
    let session_db = home.path().join("roundtrip.db");

    let mut cfg = GuiConfig::default();
    cfg.learner.name = "Roundtrip".to_string();
    cfg.persistence.session_db = Some(session_db.clone());

    // First build; the `ActiveSession` drops at the end of the
    // block, mirroring the session-state-take inside
    // close_session_inner.
    let first_id = {
        let s = build_active_session(home.path(), &cfg).await.unwrap();
        let dm = s.dialogue_manager.lock().await;
        dm.learner.profile.id
    };

    let s2 = build_active_session(home.path(), &cfg).await.unwrap();
    let id2 = s2.dialogue_manager.lock().await.learner.profile.id;
    assert_eq!(id2, first_id, "learner UUID stable across reopens");
}

#[tokio::test]
async fn locale_mismatch_on_existing_learner_returns_error() {
    // First open creates a learner persisted under locale "en".
    // Second open with the same session DB but a different
    // `cfg.learner.locale` must error rather than silently inheriting
    // the persisted locale — otherwise KB/STT/TTS would run under the
    // new locale while the LLM's prompt pack stays English, producing
    // the bug where German speech round-trips through an English LLM
    // response (manual smoke test, PR #101).
    let home = TempDir::new().unwrap();
    let session_db = home.path().join("locale_mismatch.db");

    let mut cfg = GuiConfig::default();
    cfg.learner.name = "Binti".to_string();
    cfg.learner.locale = "en".to_string();
    cfg.persistence.session_db = Some(session_db.clone());

    let _ = build_active_session(home.path(), &cfg)
        .await
        .expect("first open succeeds");

    // Second open: same name (same on-disk DB), different locale.
    let mut cfg2 = cfg.clone();
    cfg2.learner.locale = "de".to_string();
    let err = build_active_session(home.path(), &cfg2)
        .await
        .expect_err("expected locale-mismatch error");
    assert!(
        err.contains("\"de\"") && err.contains("\"en\""),
        "error must name both locales: {err}"
    );
    assert!(
        err.contains("revert") || err.contains("remove"),
        "error must point at the two resolutions: {err}"
    );
}

#[tokio::test]
async fn locale_matches_after_existing_open() {
    // Symmetric green-path: re-opening with the SAME locale must
    // succeed (i.e. the mismatch guard isn't a regression for the
    // ordinary "open the same learner twice" flow).
    let home = TempDir::new().unwrap();
    let session_db = home.path().join("locale_match.db");

    let mut cfg = GuiConfig::default();
    cfg.learner.name = "Binti".to_string();
    cfg.learner.locale = "de".to_string();
    cfg.persistence.session_db = Some(session_db.clone());

    let _ = build_active_session(home.path(), &cfg)
        .await
        .expect("first open succeeds");
    let s2 = build_active_session(home.path(), &cfg)
        .await
        .expect("second open with same locale succeeds");
    let dm = s2.dialogue_manager.lock().await;
    assert_eq!(dm.learner.profile.locale.pack_id(), "de");
}

#[tokio::test]
async fn build_active_session_for_resume_opens_session_db_once() {
    // Acceptance criterion for issue #86: the resume path must open
    // the session DB exactly once, not twice (probe + build). Sets
    // up a learner persisted under English, then drives the resume
    // helper with a German cfg — the helper must inherit English
    // from the persisted learner without a second `open_for_locale`
    // call.
    //
    // IMPORTANT: this assertion uses a thread-local counter exposed
    // by `primer_storage::__session_store_open_count_for_tests`. `#[tokio::test]`
    // defaults to a `current_thread` runtime, so every `await` in
    // this test resumes on the same OS thread as the `before`/
    // `after` snapshots — counter deltas are exact. Do NOT switch to
    // `#[tokio::test(flavor = "multi_thread")]` here: tokio workers
    // would observe the `open_for_locale` increment on a different
    // OS thread and this test would silently always read `0`. See
    // the `__session_store_open_count_for_tests` doc for the full
    // rationale.
    let home = TempDir::new().unwrap();
    let session_db = home.path().join("resume_open_count.db");

    let mut cfg_en = GuiConfig::default();
    cfg_en.learner.name = "Binti".to_string();
    cfg_en.learner.locale = "en".to_string();
    cfg_en.persistence.session_db = Some(session_db.clone());

    // First build under English persists the learner row.
    let _ = build_active_session(home.path(), &cfg_en)
        .await
        .expect("seed open succeeds");

    // Resume request asks for German; the helper must inherit
    // English silently and open only once.
    let mut cfg_de = cfg_en.clone();
    cfg_de.learner.locale = "de".to_string();
    let before = primer_storage::__session_store_open_count_for_tests();
    let active = build_active_session_for_resume(home.path(), &cfg_de)
        .await
        .expect("resume build succeeds despite cfg/persisted locale mismatch");
    let after = primer_storage::__session_store_open_count_for_tests();

    assert_eq!(
        after - before,
        1,
        "resume must open the session DB exactly once (was 2 with probe_learner_locale)"
    );
    assert_eq!(
        active.locale.pack_id(),
        "en",
        "inherited locale wins for the resumed ActiveSession"
    );
    let dm = active.dialogue_manager.lock().await;
    assert_eq!(
        dm.learner.profile.locale.pack_id(),
        "en",
        "learner profile carries the inherited locale"
    );
}

#[tokio::test]
async fn build_active_session_for_resume_uses_cfg_locale_when_no_persisted_learner() {
    // Fresh DB, no learner row yet: the resume helper must fall
    // through to cfg's locale (no inheritance source available).
    let home = TempDir::new().unwrap();
    let mut cfg = GuiConfig::default();
    cfg.learner.name = "Fresh".to_string();
    cfg.learner.locale = "de".to_string();
    cfg.persistence.session_db = Some(home.path().join("fresh.db"));

    let active = build_active_session_for_resume(home.path(), &cfg)
        .await
        .expect("resume build succeeds on a fresh DB");
    assert_eq!(active.locale.pack_id(), "de", "cfg's locale wins");
}

#[tokio::test]
async fn build_active_session_for_resume_matches_cfg_when_locales_agree() {
    // No mismatch, no inheritance to do — should behave identically
    // to start_session and not log an inheritance warning.
    let home = TempDir::new().unwrap();
    let session_db = home.path().join("agree.db");
    let mut cfg = GuiConfig::default();
    cfg.learner.name = "Agree".to_string();
    cfg.learner.locale = "de".to_string();
    cfg.persistence.session_db = Some(session_db);

    let _ = build_active_session(home.path(), &cfg)
        .await
        .expect("seed open under de succeeds");
    let active = build_active_session_for_resume(home.path(), &cfg)
        .await
        .expect("resume under matching de succeeds");
    assert_eq!(active.locale.pack_id(), "de");
}

#[tokio::test]
async fn name_reconciles_on_second_open() {
    // Two start_sessions against the same on-disk file: the persisted
    // name wins; the GUI's --name flag never overwrites it.
    let home = TempDir::new().unwrap();
    let session_db = home.path().join("test.db");

    let mut cfg = GuiConfig::default();
    cfg.learner.name = "Binti".to_string();
    cfg.persistence.session_db = Some(session_db.clone());

    let s1 = build_active_session(home.path(), &cfg).await.unwrap();
    let id1 = s1.dialogue_manager.lock().await.learner.profile.id;

    // Second open with a different CLI-level name.
    let mut cfg2 = cfg.clone();
    cfg2.learner.name = "Other".to_string();
    let s2 = build_active_session(home.path(), &cfg2).await.unwrap();
    let dm2 = s2.dialogue_manager.lock().await;
    assert_eq!(dm2.learner.profile.id, id1, "UUID stable across opens");
    assert_eq!(
        dm2.learner.profile.name, "Binti",
        "persisted name wins over GUI override"
    );
}
