//! Build an [`ActiveSession`] from a [`GuiConfig`].
//!
//! Mirrors the wiring code in `primer-cli`'s `async_main` — same call
//! sequence, same defaults, same error semantics — but returns a
//! constructed `ActiveSession` instead of a `DialogueManager` so the
//! GUI can manage the lifetime itself (DM is built lazily per command;
//! see [`crate::state`] for the rationale).
//!
//! Errors surface as `String` so the Tauri command can forward them
//! to the frontend directly. Per-field validation belongs to the
//! settings modal (step 8); here we fail fast on construction errors.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use primer_classifier::ClassifierSettings;
use primer_comprehension::ComprehensionSettings;
use primer_core::config::PedagogyConfig;
use primer_core::i18n::Locale;
use primer_core::storage::{LearnerStore, SessionStore};
use primer_engine::{
    BackendParams, IN_MEMORY, build_backend, build_classifier, build_comprehension,
    build_extractor, build_fastembed_embedder, build_ollama_embedder, create_learner_with_id,
    reconcile_persisted_learner, resolve_session_db_path,
};
use primer_extractor::ExtractorSettings;
use primer_knowledge::SqliteKnowledgeBase;
use primer_pedagogy::vocab::VocabSettings;
use primer_storage::SqliteSessionStore;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::config::{ApiKeySource, GuiConfig};
use crate::state::ActiveSession;

/// Construct everything `DialogueManager::new` would need from a single
/// `GuiConfig`.
///
/// Side effects mirror `primer-cli`:
/// - Opens or creates the per-locale knowledge DB and auto-seeds it
///   from the bundled JSONL if empty.
/// - Opens (or creates the parent dir for) the per-learner session DB
///   under `<home>/.primer/<slug>.db` unless an explicit path is given.
/// - Loads or mints a `LearnerModel`; reconciles with the GUI settings
///   (age refresh + name-mismatch warning) when one is already on disk.
///
/// `home` is the user's home directory. Tests can pass a synthetic dir
/// from `tempfile::TempDir`. The function never reads `$HOME` directly.
pub async fn build_active_session(
    home: &Path,
    config: &GuiConfig,
) -> Result<ActiveSession, String> {
    let learner_config = &config.learner;
    let backend_config = &config.backend;

    // ─── Locale ──────────────────────────────────────────────────────
    let locale = Locale::from_pack_id(&learner_config.locale).ok_or_else(|| {
        let known: Vec<&str> = Locale::ALL.iter().map(|l| l.pack_id()).collect();
        format!(
            "language {:?} is not a supported locale pack. Known: {:?}",
            learner_config.locale, known
        )
    })?;

    // ─── Main model resolution ───────────────────────────────────────
    let main_model = resolve_main_model(&backend_config.kind, backend_config.model.as_deref())?;

    // ─── BackendParams ───────────────────────────────────────────────
    // The cloud `api_key` resolves Env vs Inline here so the wiring
    // crate doesn't need to touch env vars on every helper call.
    let api_key = match (&backend_config.api_key_source, backend_config.kind.as_str()) {
        (ApiKeySource::Inline { key }, _) => Some(key.clone()),
        (ApiKeySource::Env, "cloud") => std::env::var("ANTHROPIC_API_KEY").ok(),
        (ApiKeySource::Env, _) => None,
    };

    let params = BackendParams {
        api_key,
        ollama_url: backend_config.ollama_url.clone(),
        classifier_backend: subsystem_kind(&config.classifier),
        classifier_model: config.classifier.model.clone(),
        extractor_backend: subsystem_kind(&config.extractor),
        extractor_model: config.extractor.model.clone(),
        comprehension_backend: subsystem_kind(&config.comprehension),
        comprehension_model: config.comprehension.model.clone(),
    };

    // ─── Main backend ────────────────────────────────────────────────
    let backend = build_backend(&backend_config.kind, main_model.clone(), &params)
        .await
        .map_err(|e| format!("constructing inference backend: {e}"))?;

    // ─── Knowledge base + auto-seed ──────────────────────────────────
    let knowledge_path = config
        .persistence
        .knowledge_db
        .clone()
        .unwrap_or_else(|| PathBuf::from(IN_MEMORY));
    let knowledge = SqliteKnowledgeBase::open_for_locale(&knowledge_path, locale)
        .map_err(|e| format!("opening knowledge base {}: {e}", knowledge_path.display()))?;
    if let Some(stats) = primer_kb_load::auto_seed_if_empty(&knowledge, locale)
        .await
        .map_err(|e| format!("auto-seeding knowledge base: {e}"))?
    {
        tracing::info!(
            target = "primer-gui::startup",
            inserted = stats.inserted,
            sources = stats.sources_seen,
            "auto-seeded knowledge base for locale {}",
            locale.pack_id()
        );
    }
    let knowledge = Arc::new(knowledge);

    // ─── Session store ───────────────────────────────────────────────
    let session_path = resolve_session_db_path(
        config.persistence.session_db.clone(),
        home,
        &learner_config.name,
        config.persistence.no_persist,
    );
    if !config.persistence.no_persist {
        if let Some(parent) = session_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    format!(
                        "creating session-db directory {}: {e}",
                        parent.display()
                    )
                })?;
            }
        }
    }
    let session_store = Arc::new(
        SqliteSessionStore::open_for_locale(&session_path, locale)
            .map_err(|e| format!("opening session-db {}: {e}", session_path.display()))?,
    );

    // ─── Learner model ───────────────────────────────────────────────
    let learner = match session_store.load_learner().await {
        Ok(Some(existing)) => {
            let reconciled =
                reconcile_persisted_learner(existing, &learner_config.name, learner_config.age);
            if let Err(e) = session_store.save_learner(&reconciled).await {
                tracing::warn!("save_learner on session-start failed: {e}");
            }
            reconciled
        }
        Ok(None) => {
            // Truly fresh DB OR v3 DB with sessions but no learners row.
            // Adopt the most-recent session's learner_id so existing
            // sessions are not orphaned.
            let id = match session_store.most_recent_session_learner_id().await {
                Ok(Some(uuid)) => {
                    tracing::info!("adopted learner_id {uuid} from existing sessions");
                    uuid
                }
                Ok(None) => Uuid::new_v4(),
                Err(e) => {
                    tracing::warn!(
                        "most_recent_session_learner_id failed: {e}; minting fresh UUID"
                    );
                    Uuid::new_v4()
                }
            };
            let fresh =
                create_learner_with_id(id, &learner_config.name, learner_config.age, locale);
            if let Err(e) = session_store.save_learner(&fresh).await {
                tracing::warn!("save_learner on session-start failed: {e}");
            }
            fresh
        }
        Err(e) => return Err(format!("load_learner failed: {e}")),
    };

    // ─── Subsystems ──────────────────────────────────────────────────
    let classifier_settings = ClassifierSettings {
        blocking_timeout: Duration::from_millis(config.classifier.timeout_ms),
        ..ClassifierSettings::default()
    };
    let classifier = build_classifier(
        Arc::clone(&backend),
        &backend_config.kind,
        &main_model,
        &params,
        classifier_settings.clone(),
    )
    .await
    .map_err(|e| format!("constructing engagement classifier: {e}"))?;

    let extractor_settings = ExtractorSettings {
        blocking_timeout: Duration::from_millis(config.extractor.timeout_ms),
        ..ExtractorSettings::default()
    };
    let extractor = build_extractor(
        Arc::clone(&backend),
        &backend_config.kind,
        &main_model,
        &params,
        extractor_settings.clone(),
    )
    .await
    .map_err(|e| format!("constructing concept extractor: {e}"))?;

    let comprehension_settings = ComprehensionSettings {
        blocking_timeout: Duration::from_millis(config.comprehension.timeout_ms),
        ..ComprehensionSettings::default()
    };
    let comprehension = build_comprehension(
        Arc::clone(&backend),
        &backend_config.kind,
        &main_model,
        &params,
        comprehension_settings.clone(),
    )
    .await
    .map_err(|e| format!("constructing comprehension classifier: {e}"))?;

    // ─── Embedder ────────────────────────────────────────────────────
    let embedder = build_embedder(&config.embedder).await?;

    // ─── Pedagogy + vocab settings ───────────────────────────────────
    let pedagogy_config = PedagogyConfig {
        break_suggest_after_minutes: config.breaks.after_mins,
        ..PedagogyConfig::default()
    };
    let vocab_settings = VocabSettings {
        max_per_prompt: config
            .vocab
            .max_per_prompt
            .map(|n| n as usize)
            .unwrap_or(primer_core::consts::vocab::DEFAULT_VOCAB_MAX_PER_PROMPT),
    };

    // ─── ActiveSession ───────────────────────────────────────────────
    // The session_id is provisional — step 4's `send_message` will
    // call `DialogueManager::open_session` and overwrite it with the
    // real Session UUID. Stored here so commands have something to
    // reference before the first turn lands.
    let provisional_session_id = Uuid::new_v4();

    let learner_store: Arc<dyn LearnerStore> = Arc::clone(&session_store) as _;
    let session_store_dyn: Arc<dyn primer_core::storage::SessionStore> =
        Arc::clone(&session_store) as _;

    Ok(ActiveSession {
        session_id: provisional_session_id,
        locale,
        learner: Mutex::new(learner),
        backend,
        backend_name: backend_config.kind.clone(),
        main_model,
        knowledge,
        session_store: session_store_dyn,
        learner_store,
        classifier,
        classifier_settings,
        extractor,
        extractor_settings,
        comprehension,
        comprehension_settings,
        vocab_settings,
        embedder,
        pedagogy_config,
    })
}

/// Resolve the main backend's model id from the GUI's optional override.
/// Mirrors `primer-cli`: cloud defaults to claude-sonnet-4-6, ollama
/// requires an explicit value, stub falls back to the literal "stub".
fn resolve_main_model(kind: &str, model: Option<&str>) -> Result<String, String> {
    match kind {
        "cloud" => Ok(model
            .map(String::from)
            .unwrap_or_else(|| "claude-sonnet-4-6".to_string())),
        "ollama" => model.map(String::from).ok_or_else(|| {
            "ollama backend requires a model name (e.g. \"llama3.2\") in settings".to_string()
        }),
        "stub" => Ok(model.map(String::from).unwrap_or_else(|| "stub".to_string())),
        other => Err(format!(
            "unknown backend kind {other:?}: expected one of stub, cloud, ollama"
        )),
    }
}

/// For a subsystem set to "match the main backend" we leave the
/// override fields empty — the wiring helpers in `primer-engine` then
/// reuse the main backend via `Arc::clone`. Only an explicit override
/// produces a non-None kind here.
fn subsystem_kind(s: &crate::config::SubsystemConfig) -> Option<String> {
    if s.match_main { None } else { s.kind.clone() }
}

/// Embedder construction, mirroring `primer-cli`'s dispatch matrix.
///
/// `fastembed` / `ollama` require their respective cargo features; the
/// `primer-engine` helpers exit the process with a clear error when the
/// feature is missing, which is the same behaviour the CLI provides.
async fn build_embedder(
    config: &crate::config::EmbedderConfig,
) -> Result<Option<Arc<dyn primer_core::embedder::Embedder>>, String> {
    match config.kind.as_str() {
        "none" => Ok(None),
        "stub" => Ok(Some(Arc::new(primer_embedding::StubEmbedder::new()) as _)),
        "fastembed" => Ok(build_fastembed_embedder(config.model.as_deref())),
        "ollama" => Ok(
            build_ollama_embedder(config.ollama_url.as_deref(), config.model.as_deref()).await,
        ),
        other => Err(format!(
            "unknown embedder backend {other:?}: expected one of none, stub, fastembed, ollama"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Build the smallest config that triggers the full stub pipeline.
    fn stub_config() -> GuiConfig {
        let mut cfg = GuiConfig::default();
        cfg.persistence.no_persist = true;
        cfg
    }

    /// Extract the error string from `build_active_session`'s result.
    /// `ActiveSession` deliberately doesn't implement `Debug` (it holds
    /// non-Debug trait objects), so the standard `.unwrap_err()` would
    /// need it to be `Debug` to panic-print the Ok variant.
    fn expect_err<T>(r: Result<T, String>) -> String {
        match r {
            Ok(_) => panic!("expected Err"),
            Err(e) => e,
        }
    }

    #[tokio::test]
    async fn builds_active_session_with_stub_backend() {
        let home = TempDir::new().unwrap();
        let cfg = stub_config();
        let s = build_active_session(home.path(), &cfg).await.unwrap();

        assert_eq!(s.backend_name, "stub");
        assert_eq!(s.main_model, "stub");
        assert_eq!(s.locale, Locale::English);
        // Subsystems all default to stub when the main backend is stub.
        assert_eq!(s.classifier.identifier(), "stub");
        assert_eq!(s.extractor.identifier(), "stub");
        assert_eq!(s.comprehension.identifier(), "stub");
        assert!(s.embedder.is_none(), "default embedder is none");
        // The learner row is freshly minted; name matches config.
        let learner = s.learner.lock().await;
        assert_eq!(learner.profile.name, "Explorer");
        assert_eq!(learner.profile.age, 8);
    }

    #[tokio::test]
    async fn unknown_locale_errors() {
        let home = TempDir::new().unwrap();
        let mut cfg = stub_config();
        cfg.learner.locale = "klingon".to_string();
        let err = expect_err(build_active_session(home.path(), &cfg).await);
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
        let err = expect_err(build_active_session(home.path(), &cfg).await);
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
        let err = expect_err(build_active_session(home.path(), &cfg).await);
        assert!(
            err.contains("magic"),
            "error must name the offending backend: {err}"
        );
    }

    #[tokio::test]
    async fn unknown_embedder_errors() {
        let home = TempDir::new().unwrap();
        let mut cfg = stub_config();
        cfg.embedder.kind = "secret-sauce".to_string();
        let err = expect_err(build_active_session(home.path(), &cfg).await);
        assert!(
            err.contains("secret-sauce"),
            "error must name the offending embedder: {err}"
        );
    }

    #[tokio::test]
    async fn stub_embedder_kind_succeeds() {
        let home = TempDir::new().unwrap();
        let mut cfg = stub_config();
        cfg.embedder.kind = "stub".to_string();
        let s = build_active_session(home.path(), &cfg).await.unwrap();
        assert!(s.embedder.is_some());
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
        let id1 = s1.learner.lock().await.profile.id;

        // Second open with a different CLI-level name.
        let mut cfg2 = cfg.clone();
        cfg2.learner.name = "Other".to_string();
        let s2 = build_active_session(home.path(), &cfg2).await.unwrap();
        let learner2 = s2.learner.lock().await;
        assert_eq!(learner2.profile.id, id1, "UUID stable across opens");
        assert_eq!(
            learner2.profile.name, "Binti",
            "persisted name wins over GUI override"
        );
    }
}
