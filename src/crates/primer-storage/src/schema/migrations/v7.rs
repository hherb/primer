//! Schema v6 → v7: Leitner-box level for spaced repetition.
//!
//! v7 adds `learner_concepts.box_level` for the Leitner-box spaced-repetition
//! schedule. INTEGER NOT NULL DEFAULT 0 means existing rows upgrade cleanly:
//! their box_level becomes 0, which (combined with their `last_encountered`)
//! schedules them for review 1 day after their old `last_encountered` —
//! effectively treating pre-v7 data as freshly-encountered. Acceptable for
//! Phase 0.3 with no field-deployed users.

use primer_core::error::{PrimerError, Result};
use rusqlite::Connection;

use super::column_exists;

/// Apply v7 migrations idempotently. Safe to run on a fresh DB (after v6
/// objects exist), on a v6 DB being upgraded, and on a v7 DB being
/// re-opened.
///
/// All steps run inside a single transaction so a partial failure rolls
/// back to the pre-migration state.
pub(crate) fn apply_v7_migrations(conn: &Connection) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| PrimerError::Storage(format!("v7 migration: failed to begin tx: {e}")))?;

    if !column_exists(&tx, "learner_concepts", "box_level")? {
        tx.execute_batch(
            "ALTER TABLE learner_concepts \
             ADD COLUMN box_level INTEGER NOT NULL DEFAULT 0;",
        )
        .map_err(|e| {
            PrimerError::Storage(format!(
                "v7 migration: ALTER learner_concepts ADD box_level: {e}"
            ))
        })?;
    }

    tx.commit()
        .map_err(|e| PrimerError::Storage(format!("v7 migration: commit: {e}")))?;
    Ok(())
}
