//! Schema v5 → v6: locale tagging for learners and concepts.
//!
//! v6 wires the `Locale` enum into persistence. Two narrowly-scoped
//! column adds:
//!   - `learners.locale` — BCP-47 short pack id (e.g. `'en'`, `'de'`).
//!     Bound dispatch key for the prompt pack and the speech pipeline.
//!   - `concepts.concept_language_tag` — language the concept was
//!     extracted in. Schema-only landing in v6; per-concept linkage
//!     across locales is a follow-up.
//!
//! Both columns default to `'en'` so pre-v6 rows upgrade cleanly without
//! a backfill pass. The application maps the short id back to a `Locale`
//! variant via `Locale::from_pack_id` at the boundary.

use primer_core::error::{PrimerError, Result};
use rusqlite::Connection;

use super::column_exists;

/// Apply v6 migrations idempotently. Safe to run on a fresh DB (after
/// v5 objects exist), on a v5 DB being upgraded, and on a v6 DB being
/// re-opened.
///
/// All steps run inside a single transaction so a partial failure rolls
/// back to the pre-migration state.
pub(crate) fn apply_v6_migrations(conn: &Connection) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| PrimerError::Storage(format!("v6 migration: failed to begin tx: {e}")))?;

    if !column_exists(&tx, "learners", "locale")? {
        tx.execute_batch("ALTER TABLE learners ADD COLUMN locale TEXT NOT NULL DEFAULT 'en';")
            .map_err(|e| {
                PrimerError::Storage(format!("v6 migration: ALTER learners ADD locale: {e}"))
            })?;
    }

    if !column_exists(&tx, "concepts", "concept_language_tag")? {
        tx.execute_batch(
            "ALTER TABLE concepts ADD COLUMN concept_language_tag TEXT NOT NULL DEFAULT 'en';",
        )
        .map_err(|e| {
            PrimerError::Storage(format!(
                "v6 migration: ALTER concepts ADD concept_language_tag: {e}"
            ))
        })?;
    }

    tx.commit()
        .map_err(|e| PrimerError::Storage(format!("v6 migration: commit: {e}")))?;
    Ok(())
}
