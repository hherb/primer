//! Schema v2 → v3: per-turn engagement classification.
//!
//! Adds the `engagement_states` lookup table, the `classifiers` registry,
//! and the `turn_classifications` junction table that records one
//! engagement label per (turn, classifier).

use primer_core::error::{PrimerError, Result};
use rusqlite::Connection;

const CREATE_ENGAGEMENT_STATES_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS engagement_states (
        id   INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE
    )
";

const CREATE_CLASSIFIERS_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS classifiers (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        identifier TEXT NOT NULL UNIQUE
    )
";

const CREATE_TURN_CLASSIFICATIONS_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS turn_classifications (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        turn_id             INTEGER NOT NULL REFERENCES turns(id),
        engagement_state_id INTEGER NOT NULL REFERENCES engagement_states(id),
        classifier_id       INTEGER NOT NULL REFERENCES classifiers(id),
        confidence          REAL NOT NULL,
        reasoning           TEXT,
        classified_at       TIMESTAMP NOT NULL,
        UNIQUE(turn_id, classifier_id)
    )
";

const CREATE_TURN_CLASSIFICATIONS_INDEX: &str = "
    CREATE INDEX IF NOT EXISTS idx_turn_classifications_turn_id
        ON turn_classifications(turn_id)
";

/// Apply v3 migrations idempotently. Safe to run on a fresh DB (after
/// v2 objects exist), on a v2 DB being upgraded, and on a v3 DB being
/// re-opened.
///
/// All steps run inside a single transaction so a partial failure rolls
/// back to the pre-migration state.
///
/// v3 adds:
/// - `engagement_states` lookup table (seeded by the validate pass in
///   `open()` after this migration runs).
/// - `classifiers` table for named classifier identifiers.
/// - `turn_classifications` junction table recording per-turn engagement
///   state labels (FK into `turns`, `engagement_states`, `classifiers`).
/// - `idx_turn_classifications_turn_id` index on the junction table.
pub(crate) fn apply_v3_migrations(conn: &Connection) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| PrimerError::Storage(format!("v3 migration: failed to begin tx: {e}")))?;
    tx.execute(CREATE_ENGAGEMENT_STATES_TABLE, [])
        .map_err(|e| PrimerError::Storage(format!("v3 migration: engagement_states: {e}")))?;
    tx.execute(CREATE_CLASSIFIERS_TABLE, [])
        .map_err(|e| PrimerError::Storage(format!("v3 migration: classifiers: {e}")))?;
    tx.execute(CREATE_TURN_CLASSIFICATIONS_TABLE, [])
        .map_err(|e| PrimerError::Storage(format!("v3 migration: turn_classifications: {e}")))?;
    tx.execute(CREATE_TURN_CLASSIFICATIONS_INDEX, [])
        .map_err(|e| PrimerError::Storage(format!("v3 migration: index: {e}")))?;
    tx.commit()
        .map_err(|e| PrimerError::Storage(format!("v3 migration: commit: {e}")))?;
    Ok(())
}
