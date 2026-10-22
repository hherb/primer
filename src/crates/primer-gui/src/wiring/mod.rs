//! Build an [`ActiveSession`] from a [`GuiConfig`].
//!
//! Mirrors the wiring code in `primer-cli`'s `async_main` — same defaults,
//! same error semantics — except that the session store opens *before*
//! the knowledge base, so the persisted learner's locale is known before
//! the KB is opened under it (issue #86). Returns an `ActiveSession` that
//! owns the long-lived `DialogueManager` (see [`crate::state`] for why it
//! is long-lived).
//!
//! Errors surface as `String` so the Tauri command can forward them
//! to the frontend directly. Per-field validation lives in
//! [`crate::validation`] and the settings modal; here we fail fast on
//! construction errors.
//!
//! Layout: this file holds the two public entry points and the
//! orchestrating `build_with_strategy`; the steps live in
//! `backend_params` (GUI config → `BackendParams` + model/key resolvers),
//! `stores` (session store + learner/locale resolution + knowledge base),
//! and `subsystems` (classifier / extractor / comprehension / embedder).

mod backend_params;
mod stores;
mod subsystems;

use std::path::Path;
use std::sync::Arc;

use primer_core::config::PedagogyConfig;
use primer_core::i18n::Locale;
use primer_core::storage::{LearnerStore, SessionStore};
use primer_engine::build_main_backend;
use tokio::sync::Mutex;

use crate::config::GuiConfig;
use crate::state::{ActiveSession, SessionSnapshot};

use backend_params::{build_backend_params, resolve_main_model, served_identity};
use stores::{open_knowledge_base, open_session_store_and_learner};
use subsystems::{build_subsystems, retrieval_notice};

// `tests.rs` calls `cloud_backend_in_use` by bare name via `use super::*`;
// production code in this file doesn't call it, so the import is test-only.
#[cfg(test)]
use backend_params::cloud_backend_in_use;

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
/// Side effects, in order:
/// - Opens (or creates the parent dir for) the per-learner session DB
///   under `<home>/.primer/<slug>.db` unless an explicit path is given.
/// - Loads or mints a `LearnerModel`; reconciles with the GUI settings
///   (age refresh + name-mismatch warning) when one is already on disk.
/// - Opens or creates the per-locale knowledge DB and auto-seeds it
///   from the bundled JSONL if empty.
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

    // ─── Main model resolution + BackendParams ───────────────────────
    let main_model = resolve_main_model(&backend_config.kind, backend_config.model.as_deref())?;
    let params = build_backend_params(config);

    // ─── Main backend (locale-independent) ───────────────────────────
    // `build_main_backend` is `build_backend` plus the opt-in fallback /
    // router wrap; with no fallback configured and `router_mode =
    // LocalOnly` it is equivalent to `build_backend`.
    let (backend, notice) = build_main_backend(&backend_config.kind, main_model.clone(), &params)
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

    // ─── Session store + learner (open BEFORE KB so the learner's
    //     locale is probed without a second open) ─────────────────────
    let (session_store_inner, effective_locale, learner) =
        open_session_store_and_learner(home, config, cfg_locale, strategy).await?;
    let session_store = Arc::new(session_store_inner);

    // ─── Knowledge base + auto-seed (under effective_locale) ─────────
    let knowledge = Arc::new(open_knowledge_base(config, effective_locale).await?);

    // ─── Subsystems (classifier, extractor, comprehension, embedder) ─
    let subsystems = build_subsystems(config, &backend, &main_model, &params).await?;
    let retrieval_notice = retrieval_notice(&config.embedder.kind, subsystems.embedder.is_some());

    // ─── Pedagogy settings ───────────────────────────────────────────
    let pedagogy_config = PedagogyConfig {
        break_suggest_after_minutes: config.breaks.after_mins,
        ..PedagogyConfig::default()
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
    // Snapshot must be built BEFORE `learner` moves into `DialogueManager::new`.
    let initial_snapshot = SessionSnapshot {
        session_id: None,
        learner_id: learner.profile.id,
        learner_name: learner.profile.name.clone(),
        learner_age: learner.profile.age,
        concept_count: learner.concepts.len(),
    };

    // What the sidebar shows; differs from the config when the fallback
    // serves alone (#338). The subsystems above still get the primary's
    // `main_model` as their identifier even then — a known mislabel (#351).
    let (backend_name, display_model) = served_identity(
        &backend_config.kind,
        &main_model,
        notice.as_ref(),
        backend.name(),
    );

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
        backend_name,
        main_model: display_model,
        backend_notice: notice.map(|n| n.to_string()),
        retrieval_notice,
        session_store: Arc::clone(&session_store) as _,
        current_turn_abort: Mutex::new(None),
    })
}

#[cfg(test)]
mod stage_tests;
#[cfg(test)]
mod tests;
