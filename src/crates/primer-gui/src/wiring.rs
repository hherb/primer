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
    BackendParams, IN_MEMORY, build_classifier, build_comprehension, build_extractor,
    build_fastembed_embedder, build_main_backend, build_ollama_embedder,
    build_openai_compat_embedder, create_learner_with_id, parse_languages,
    reconcile_persisted_learner, resolve_session_db_path,
};
use primer_extractor::ExtractorSettings;
use primer_knowledge::SqliteKnowledgeBase;
use primer_pedagogy::vocab::VocabSettings;
use primer_storage::SqliteSessionStore;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::config::{ApiKeySource, GuiConfig};
use crate::state::{ActiveSession, SessionSnapshot};

/// How `build_active_session*` should treat a locale mismatch between
/// `cfg.learner.locale` and a learner already persisted on disk.
///
/// `start_session` uses [`LocaleStrategy::UseCfg`] (hard-fail on mismatch
/// — the persisted longitudinal record must not silently accept a new
/// locale tag). `resume_session` uses
/// [`LocaleStrategy::InheritFromPersistedLearner`] (silently inherit
/// the persisted locale; the cfg value is only relevant for new
/// sessions, never for continuing an existing one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocaleStrategy {
    UseCfg,
    InheritFromPersistedLearner,
}

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
    build_with_strategy(home, config, LocaleStrategy::UseCfg).await
}

/// Variant of [`build_active_session`] that silently inherits the
/// persisted learner's locale on mismatch instead of erroring.
///
/// Used by the GUI's `resume_session` Tauri command: the user has chosen
/// a saved session whose locale was set when it was originally created;
/// the `cfg.learner.locale` value reflects what the picker would use for
/// a NEW session, not what this resumed session was tagged under. Issue
/// #86: the previous code path called a separate `probe_learner_locale`
/// helper that opened the session DB just to read the learner row, then
/// `build_active_session` opened it again. This helper folds both into
/// a single open by reading the learner immediately after the first
/// open and (when needed) re-tagging the store's locale field in place
/// — the SQLite-file schema is locale-neutral on the session side, so
/// no re-open is required (see `SqliteSessionStore::set_locale` for the
/// rationale).
pub async fn build_active_session_for_resume(
    home: &Path,
    config: &GuiConfig,
) -> Result<ActiveSession, String> {
    build_with_strategy(home, config, LocaleStrategy::InheritFromPersistedLearner).await
}

/// Shared body of `build_active_session` and
/// `build_active_session_for_resume`. The two callers differ only in
/// how they treat a locale mismatch between `cfg.learner.locale` and
/// the persisted learner row (see [`LocaleStrategy`]).
async fn build_with_strategy(
    home: &Path,
    config: &GuiConfig,
    strategy: LocaleStrategy,
) -> Result<ActiveSession, String> {
    let learner_config = &config.learner;
    let backend_config = &config.backend;

    // ─── Locale (from cfg; may be overridden by inherit-on-mismatch) ─
    let cfg_locale = Locale::from_pack_id(&learner_config.locale).ok_or_else(|| {
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
    // crate doesn't need to touch env vars on every helper call. The
    // `ANTHROPIC_API_KEY` env var is consulted whenever the cloud backend
    // is reachable — as the primary OR as the opt-in fallback secondary
    // (issue #205 follow-up). A local-primary → cloud-fallback setup (the
    // supported fallback direction) needs the key even though the primary
    // `kind` is not "cloud"; without this the cloud secondary fails to
    // build with an Auth error and the fallback silently degrades to
    // PrimaryAlone. An Inline key is kind-agnostic (it may be entered for
    // a cloud fallback even when the primary is local).
    let api_key = match &backend_config.api_key_source {
        ApiKeySource::Inline { key } => Some(key.clone()),
        ApiKeySource::Env
            if cloud_backend_in_use(
                &backend_config.kind,
                backend_config.fallback_backend.as_deref(),
            ) =>
        {
            std::env::var("ANTHROPIC_API_KEY").ok()
        }
        ApiKeySource::Env => None,
    };

    // OpenAI-compat key resolves independently of the cloud key. `Env`
    // reads `OPENAI_COMPAT_API_KEY` (the CLI's env-var name); the same
    // resolved value feeds both the main backend (when kind ==
    // "openai-compat") and the openai-compat embedder, mirroring the
    // CLI's reuse of `--openai-compat-api-key` across both.
    let openai_compat_api_key = match &backend_config.openai_compat_api_key_source {
        ApiKeySource::Inline { key } => Some(key.clone()),
        ApiKeySource::Env => std::env::var("OPENAI_COMPAT_API_KEY").ok(),
    };

    let params = BackendParams {
        api_key,
        ollama_url: backend_config.ollama_url.clone(),
        openai_compat_url: backend_config.openai_compat_url.clone(),
        openai_compat_api_key: openai_compat_api_key.clone(),
        classifier_backend: subsystem_kind(&config.classifier),
        classifier_model: config.classifier.model.clone(),
        extractor_backend: subsystem_kind(&config.extractor),
        extractor_model: config.extractor.model.clone(),
        comprehension_backend: subsystem_kind(&config.comprehension),
        comprehension_model: config.comprehension.model.clone(),
        // QNN backend (Phase 1.2 step 1.2.4): the bundle / QAIRT-lib
        // paths come straight from Settings → Inference backend. On a
        // default (non-`qnn`-feature) GUI build, `build_qnn_backend`'s
        // `not(feature = "qnn")` arm still returns the "rebuild with
        // --features qnn" hint regardless of these values, so selecting
        // qnn surfaces a clear build-time error inline rather than
        // killing the GUI (mirrors the openai-compat-embedder pattern).
        qnn_bundle_dir: backend_config.qnn_bundle_dir.clone(),
        qnn_qairt_lib_dir: backend_config.qnn_qairt_lib_dir.clone(),
        // llama.cpp backend (Phase 1.1): the GGUF path / gpu-layers /
        // n_ctx come straight from Settings → Inference backend. The GUI
        // carries a dedicated `gguf_path` field (unlike the CLI, which
        // reuses `--model`). On a default (non-`llamacpp`-feature) build,
        // `build_llamacpp_backend`'s `not(feature = "llamacpp")` arm
        // returns the "rebuild with --features llamacpp" hint regardless
        // of these values, so selecting llamacpp surfaces a clear
        // build-time error inline rather than killing the GUI (mirrors
        // the qnn pattern).
        gguf_path: backend_config.gguf_path.clone(),
        llamacpp_gpu_layers: backend_config.llamacpp_gpu_layers,
        llamacpp_n_ctx: backend_config.llamacpp_n_ctx,
        // Custom reasoning markers from Settings → Inference backend.
        // Parsed from the raw textarea text into `(open, close)` pairs and
        // appended to the built-in defaults by the ollama / openai-compat
        // backends. Empty string ⇒ empty Vec ⇒ defaults only.
        reasoning_markers: crate::reasoning_markers::parse_reasoning_markers(
            &backend_config.reasoning_markers,
        ),
        // Opt-in local→cloud fallback (issue #205): the fallback backend /
        // model come straight from Settings → Inference backend. `None` ⇒ no
        // fallback ⇒ today's single-backend behavior (the privacy default —
        // a local-only setup never silently reaches the cloud). When set,
        // `build_main_backend` wraps the primary in a `FallbackBackend`
        // decorator that serves the turn from the secondary if the primary is
        // unavailable at startup or fails *before any token streams*. Mirrors
        // the CLI's `--fallback-backend` / `--fallback-model`.
        fallback_backend: backend_config.fallback_backend.clone(),
        fallback_model: backend_config.fallback_model.clone(),
        // Phase 1.3 inference-router mode from Settings → Inference backend.
        // `RouterMode` is `Copy`, so no `.clone()`. `LocalOnly` (the default)
        // ⇒ today's single-backend behaviour; the other modes route via the
        // configured fallback secondary in `build_main_backend`. Mirrors the
        // CLI's `--router-mode`.
        router_mode: backend_config.router_mode,
        // Phase 1.3 latency-aware routing budget from Settings → Inference
        // backend. `None` ⇒ latency routing OFF. Mirrors the CLI's
        // `--primary-ttft-budget-ms`.
        primary_ttft_budget_ms: backend_config.primary_ttft_budget_ms,
    };

    // ─── Main backend (locale-independent) ───────────────────────────
    // `build_main_backend` is `build_backend` plus the opt-in fallback wrap;
    // with no fallback configured it is byte-for-byte today's behavior.
    let backend = build_main_backend(&backend_config.kind, main_model.clone(), &params)
        .await
        .map_err(|e| format!("constructing inference backend: {e}"))?;

    // For QNN the real model id comes from `primer-meta.json` inside the
    // bundle; for llamacpp it comes from the GGUF stem. In both cases
    // rebind `main_model` to `backend.name()` (e.g. "qnn:Qwen3-4B" or
    // "llamacpp:qwen3-4b-q4") so the downstream subsystem identifiers
    // carry the real model id instead of the "*-pending" placeholder.
    // Mirrors the CLI's post-construction rebind.
    let main_model = if backend_config.kind == "qnn" || backend_config.kind == "llamacpp" {
        backend.name().to_string()
    } else {
        main_model
    };

    // ─── Session store (open BEFORE KB so we can probe the learner's
    //     locale and avoid a second open). Opens under cfg_locale; the
    //     in-place set_locale call below switches the tag field if the
    //     persisted learner has a different locale.  ──────────────────
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
                    format!("creating session-db directory {}: {e}", parent.display())
                })?;
            }
        }
    }
    let mut session_store_inner = SqliteSessionStore::open_for_locale(&session_path, cfg_locale)
        .map_err(|e| format!("opening session-db {}: {e}", session_path.display()))?;

    // ─── Learner model + effective locale resolution ─────────────────
    let persisted = session_store_inner
        .load_learner()
        .await
        .map_err(|e| format!("load_learner failed: {e}"))?;

    let (effective_locale, learner) = match (persisted, strategy) {
        (Some(existing), LocaleStrategy::UseCfg) => {
            // Hard-fail on locale mismatch. The persisted learner already
            // carries `concept_language_tag` rows under its stored locale;
            // silently adopting a new locale would tag every new concept
            // under a different language, corrupting the longitudinal
            // vocabulary record (see the locale-is-per-learner gotcha in
            // CLAUDE.md). Mirrors the CLI's `verify_resume_locale_match`
            // discipline, but adapted for the GUI's start-new-session path
            // where the user-actionable resolutions are "revert Settings"
            // or "remove the persisted learner DB file".
            if existing.profile.locale != cfg_locale {
                return Err(format!(
                    "Settings → Locale is {:?} but the persisted learner at {} \
                     was created under locale {:?}. Either revert Settings → Locale \
                     to {:?}, or remove that DB file to start a fresh learner under \
                     {:?}. Silent re-tagging is refused because it would corrupt the \
                     longitudinal concept-language record.",
                    cfg_locale.pack_id(),
                    session_path.display(),
                    existing.profile.locale.pack_id(),
                    existing.profile.locale.pack_id(),
                    cfg_locale.pack_id(),
                ));
            }
            let reconciled =
                reconcile_persisted_learner(existing, &learner_config.name, learner_config.age);
            if let Err(e) = session_store_inner.save_learner(&reconciled).await {
                tracing::warn!("save_learner on session-start failed: {e}");
            }
            (cfg_locale, reconciled)
        }
        (Some(existing), LocaleStrategy::InheritFromPersistedLearner) => {
            // Resume path: silently inherit the persisted locale. The
            // store was opened under cfg_locale; if they differ, re-tag
            // the store in place so any newly written concepts in the
            // resumed session land with the correct
            // `concept_language_tag`. No second `open_for_locale` call.
            let persisted_locale = existing.profile.locale;
            if persisted_locale != cfg_locale {
                tracing::info!(
                    target: "primer_gui::resume",
                    cfg_locale = %cfg_locale.pack_id(),
                    session_locale = %persisted_locale.pack_id(),
                    "resume: inheriting persisted learner's locale (cfg differed)"
                );
                session_store_inner.set_locale(persisted_locale);
            }
            let reconciled =
                reconcile_persisted_learner(existing, &learner_config.name, learner_config.age);
            if let Err(e) = session_store_inner.save_learner(&reconciled).await {
                tracing::warn!("save_learner on session-start failed: {e}");
            }
            (persisted_locale, reconciled)
        }
        (None, _) => {
            // Truly fresh DB OR v3 DB with sessions but no learners row.
            // Adopt the most-recent session's learner_id so existing
            // sessions are not orphaned. No inheritance possible — cfg
            // wins regardless of strategy.
            let id = match session_store_inner.most_recent_session_learner_id().await {
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
            // The GUI has no `--languages` field yet (out of scope for the
            // CLI-titled issue #21); pass the locale-derived default so the
            // preference list matches the historical behaviour exactly.
            let fresh = create_learner_with_id(
                id,
                &learner_config.name,
                learner_config.age,
                cfg_locale,
                parse_languages(None, cfg_locale),
            );
            if let Err(e) = session_store_inner.save_learner(&fresh).await {
                tracing::warn!("save_learner on session-start failed: {e}");
            }
            (cfg_locale, fresh)
        }
    };

    let session_store = Arc::new(session_store_inner);

    // ─── Knowledge base + auto-seed (under effective_locale) ─────────
    let knowledge_path = config
        .persistence
        .knowledge_db
        .clone()
        .unwrap_or_else(|| PathBuf::from(IN_MEMORY));
    let knowledge = SqliteKnowledgeBase::open_for_locale(&knowledge_path, effective_locale)
        .map_err(|e| format!("opening knowledge base {}: {e}", knowledge_path.display()))?;
    if let Some(stats) = primer_kb_load::auto_seed_if_empty(&knowledge, effective_locale)
        .await
        .map_err(|e| format!("auto-seeding knowledge base: {e}"))?
    {
        tracing::info!(
            target = "primer-gui::startup",
            inserted = stats.inserted,
            sources = stats.sources_seen,
            "auto-seeded knowledge base for locale {}",
            effective_locale.pack_id()
        );
    }
    let knowledge = Arc::new(knowledge);

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
    let embedder = build_embedder(
        &config.embedder,
        &backend_config.openai_compat_url,
        openai_compat_api_key,
    )
    .await?;

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

    // ─── DialogueManager construction ────────────────────────────────
    // Build the long-lived DM here. `DialogueManager::new` mints a fresh
    // `Session` automatically (with a brand-new UUID), so no extra
    // `open_session` call is needed — the first `send_message` lands
    // the first child turn and primer response into that session.
    //
    // The `as _` casts upcast concrete `Arc<T>` to `Arc<dyn Trait>` —
    // `Arc::clone` alone can't bridge the unsize coercion across the
    // generic boundary, so the explicit cast is the standard idiom.
    let learner_store: Arc<dyn LearnerStore> = Arc::clone(&session_store) as _;
    let session_store_dyn: Arc<dyn SessionStore> = Arc::clone(&session_store) as _;
    let knowledge_dyn: Arc<dyn primer_core::knowledge::KnowledgeBase> = knowledge as _;

    let stores = primer_pedagogy::DialogueManagerStores {
        session: Some(session_store_dyn),
        learner: Some(learner_store),
    };
    let subsystems = primer_pedagogy::DialogueManagerSubsystems {
        classifier,
        classifier_settings,
        extractor,
        extractor_settings,
        comprehension,
        comprehension_settings,
        vocab_settings,
        embedder,
    };
    // Snapshot must be built BEFORE `learner` moves into `DialogueManager::new`.
    let initial_snapshot = SessionSnapshot {
        session_id: None,
        learner_id: learner.profile.id,
        learner_name: learner.profile.name.clone(),
        learner_age: learner.profile.age,
        concept_count: learner.concepts.len(),
    };

    let dm = primer_pedagogy::DialogueManager::new(
        learner,
        Arc::clone(&backend),
        knowledge_dyn,
        stores,
        subsystems,
        pedagogy_config,
    );

    Ok(ActiveSession {
        dialogue_manager: Arc::new(Mutex::new(dm)),
        snapshot: Arc::new(Mutex::new(initial_snapshot)),
        locale: effective_locale,
        backend_name: backend_config.kind.clone(),
        main_model,
        session_store: Arc::clone(&session_store) as _,
        current_turn_abort: Mutex::new(None),
    })
}

/// Is the Anthropic cloud backend reachable in this config — either as the
/// primary backend or as the opt-in fallback secondary? Determines whether
/// the `ANTHROPIC_API_KEY` env var is worth resolving for `Env` key mode.
/// A local-primary → cloud-fallback setup (the supported fallback direction,
/// issue #205) needs the key even though the primary `kind` is not `"cloud"`.
fn cloud_backend_in_use(primary_kind: &str, fallback_backend: Option<&str>) -> bool {
    primary_kind == "cloud" || fallback_backend == Some("cloud")
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
        "openai-compat" => model.map(String::from).ok_or_else(|| {
            "openai-compat backend requires a model name (the server's model id) in settings"
                .to_string()
        }),
        "stub" => Ok(model
            .map(String::from)
            .unwrap_or_else(|| "stub".to_string())),
        "qnn" => {
            // The model id is read from the bundle's `primer-meta.json`
            // at construction time, so the `model` override is ignored.
            // Return a placeholder that `build_with_strategy` rebinds to
            // `backend.name()` once the QNN backend constructs (mirrors
            // the CLI's "qnn-pending" placeholder).
            Ok("qnn-pending".to_string())
        }
        "llamacpp" => {
            // The real model id comes from the GGUF stem at construction
            // time (`backend.name()` == "llamacpp:<stem>"). The `model`
            // override is ignored — the GGUF path lives in its own config
            // field. Return a placeholder that `build_active_session`
            // rebinds to `backend.name()` once the backend constructs
            // (mirrors the CLI's "llamacpp" rebind).
            Ok("llamacpp-pending".to_string())
        }
        other => Err(format!(
            "unknown backend kind {other:?}: expected one of stub, cloud, ollama, openai-compat, qnn, llamacpp"
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
/// `primer-engine` helpers return an `Err(String)` when the feature is
/// missing so this command path can surface it to the frontend instead
/// of killing the GUI process (the CLI maps the same Err to a clean
/// stderr line + exit).
async fn build_embedder(
    config: &crate::config::EmbedderConfig,
    main_openai_compat_url: &str,
    openai_compat_api_key: Option<String>,
) -> Result<Option<Arc<dyn primer_core::embedder::Embedder>>, String> {
    match config.kind.as_str() {
        "none" => Ok(None),
        "stub" => Ok(Some(Arc::new(primer_embedding::StubEmbedder::new()) as _)),
        "fastembed" => build_fastembed_embedder(config.model.as_deref()),
        "ollama" => {
            build_ollama_embedder(config.ollama_url.as_deref(), config.model.as_deref()).await
        }
        "openai-compat" => {
            // The embedder URL falls back to the main backend's
            // openai-compat URL when no embedder-specific override is set
            // (mirrors the CLI's `--embedder-openai-compat-url` →
            // `--openai-compat-url` fallback). The API key is the same
            // resolved value the main backend uses.
            let url = config
                .openai_compat_url
                .as_deref()
                .or(Some(main_openai_compat_url));
            build_openai_compat_embedder(url, config.model.as_deref(), openai_compat_api_key).await
        }
        other => Err(format!(
            "unknown embedder backend {other:?}: expected one of none, stub, fastembed, ollama, openai-compat"
        )),
    }
}

#[cfg(test)]
mod tests;
