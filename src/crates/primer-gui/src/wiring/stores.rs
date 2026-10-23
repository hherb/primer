//! Open the per-learner session store, resolve the learner model (and the
//! session's effective locale), and open + auto-seed the knowledge base.
//!
//! The session store is opened BEFORE the knowledge base so the persisted
//! learner's locale can be probed without a second open; the knowledge
//! base is then opened under the resolved effective locale.

use std::path::{Path, PathBuf};

use primer_core::i18n::Locale;
use primer_core::learner::LearnerModel;
use primer_core::storage::{LearnerStore, SessionStore};
use primer_engine::{
    IN_MEMORY, adopt_or_mint_learner_id, create_learner_with_id, ensure_session_db_dir,
    parse_languages, reconcile_persisted_learner, resolve_session_db_path,
};
use primer_knowledge::SqliteKnowledgeBase;
use primer_storage::SqliteSessionStore;

use super::LocaleStrategy;
use crate::config::GuiConfig;

/// Open (creating the parent dir if needed) the session DB under
/// `cfg_locale`, then load-or-mint the learner and resolve the session's
/// effective locale according to `strategy`.
///
/// Returns the opened store (not yet wrapped in an `Arc`), the effective
/// locale, and the reconciled or freshly minted learner. The learner row
/// is saved back before returning; a save failure only warns.
pub(super) async fn open_session_store_and_learner(
    home: &Path,
    config: &GuiConfig,
    cfg_locale: Locale,
    strategy: LocaleStrategy,
) -> Result<(SqliteSessionStore, Locale, LearnerModel), String> {
    let learner_config = &config.learner;

    // Opens under cfg_locale; under the resume strategy the in-place
    // set_locale call below switches the tag field if the persisted learner
    // has a different locale (`UseCfg` errors on a mismatch instead).
    let session_path = resolve_session_db_path(
        config.persistence.session_db.clone(),
        home,
        &learner_config.name,
        config.persistence.no_persist,
    );
    if !config.persistence.no_persist {
        ensure_session_db_dir(&session_path, home).map_err(|e| {
            format!(
                "creating session-db directory for {}: {e}",
                session_path.display()
            )
        })?;
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
            let id = adopt_or_mint_learner_id(
                session_store_inner.most_recent_session_learner_id().await,
            )
            .map_err(|e| {
                format!(
                    "reading existing sessions' learner_id from {}: {e}",
                    session_path.display()
                )
            })?;
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

    Ok((session_store_inner, effective_locale, learner))
}

/// Open the knowledge base under `locale` (`:memory:` unless Settings
/// names an explicit path) and auto-seed it from the bundled JSONL corpus
/// when empty.
pub(super) async fn open_knowledge_base(
    config: &GuiConfig,
    locale: Locale,
) -> Result<SqliteKnowledgeBase, String> {
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
            target: "primer-gui::startup",
            inserted = stats.inserted,
            sources = stats.sources_seen,
            "auto-seeded knowledge base for locale {}",
            locale.pack_id()
        );
    }
    Ok(knowledge)
}
