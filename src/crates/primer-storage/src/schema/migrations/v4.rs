//! Schema v3 → v4: learner-model persistence.
//!
//! Adds the `understanding_depths` lookup table plus the `learners` and
//! `learner_concepts` tables backing the longitudinal concept-mastery
//! record.
//!
//! Note on free-text `Vec<String>` columns (`learners.languages`,
//! `learners.high_engagement_topics`, `learner_concepts.notes`): these are
//! stored as JSON-encoded TEXT, not normalised into a lookup table.
//!
//! The "categorical text → lookup table" rule (CLAUDE.md) targets *closed*
//! vocabularies where a Rust enum is the source of truth (`Speaker`,
//! `PedagogicalIntent`, `EngagementState`, `UnderstandingDepth`). The three
//! columns above are open-vocabulary, free-text lists owned by the learner
//! (preferred languages, high-engagement topic phrases, free-form per-concept
//! notes from the dialogue manager). Normalising them would buy nothing the
//! `concepts` table doesn't already prove — they aren't FK targets, aren't
//! queried by exact match, aren't shared across rows, and aren't bounded.
//! JSON-in-TEXT keeps the schema flat and the round-trip lossless.

use primer_core::error::{PrimerError, Result};
use rusqlite::Connection;

const CREATE_UNDERSTANDING_DEPTHS_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS understanding_depths (
        id   INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE
    )
";

const CREATE_LEARNERS_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS learners (
        id                          TEXT PRIMARY KEY,
        name                        TEXT NOT NULL,
        age                         INTEGER NOT NULL,
        languages                   TEXT NOT NULL,
        created_at                  TEXT NOT NULL,
        last_active                 TEXT NOT NULL,
        pref_narrative              REAL NOT NULL,
        pref_socratic               REAL NOT NULL,
        pref_visual                 REAL NOT NULL,
        pref_kinesthetic            REAL NOT NULL,
        typical_session_minutes     REAL NOT NULL,
        high_engagement_topics      TEXT NOT NULL,
        early_disengagement_secs    INTEGER NOT NULL,
        current_engagement_state_id INTEGER NOT NULL REFERENCES engagement_states(id)
    )
";

const CREATE_LEARNER_CONCEPTS_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS learner_concepts (
        learner_id        TEXT NOT NULL REFERENCES learners(id) ON DELETE CASCADE,
        concept_id        INTEGER NOT NULL REFERENCES concepts(id),
        depth_id          INTEGER NOT NULL REFERENCES understanding_depths(id),
        confidence        REAL NOT NULL,
        encounter_count   INTEGER NOT NULL,
        last_encountered  TEXT,
        notes             TEXT NOT NULL DEFAULT '[]',
        PRIMARY KEY (learner_id, concept_id)
    )
";

const CREATE_LEARNER_CONCEPTS_INDEX: &str = "
    CREATE INDEX IF NOT EXISTS idx_learner_concepts_learner
        ON learner_concepts(learner_id)
";

/// Apply v4 migrations idempotently. Safe to run on a fresh DB (after
/// v3 objects exist), on a v3 DB being upgraded, and on a v4 DB being
/// re-opened.
///
/// All steps run inside a single transaction so a partial failure rolls
/// back to the pre-migration state.
///
/// v4 adds:
/// - `understanding_depths` lookup table (seeded by the validate pass
///   in `open()` after this migration runs).
/// - `learners` table — one row per learner DB file (application-level
///   invariant), holds profile + preferences + engagement snapshot.
/// - `learner_concepts` junction table for per-learner concept-mastery
///   state, FK'd into `learners`, `concepts`, and `understanding_depths`.
/// - `idx_learner_concepts_learner` index on the junction table.
///
/// Schema-only. Adopting an existing session's `learner_id` for the new
/// learners row is the CLI's responsibility — `apply_v4_migrations` runs
/// without CLI flag access and cannot populate `name` / `age`.
pub(crate) fn apply_v4_migrations(conn: &Connection) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| PrimerError::Storage(format!("v4 migration: failed to begin tx: {e}")))?;
    tx.execute(CREATE_UNDERSTANDING_DEPTHS_TABLE, [])
        .map_err(|e| PrimerError::Storage(format!("v4 migration: understanding_depths: {e}")))?;
    tx.execute(CREATE_LEARNERS_TABLE, [])
        .map_err(|e| PrimerError::Storage(format!("v4 migration: learners: {e}")))?;
    tx.execute(CREATE_LEARNER_CONCEPTS_TABLE, [])
        .map_err(|e| PrimerError::Storage(format!("v4 migration: learner_concepts: {e}")))?;
    tx.execute(CREATE_LEARNER_CONCEPTS_INDEX, [])
        .map_err(|e| PrimerError::Storage(format!("v4 migration: index: {e}")))?;
    tx.commit()
        .map_err(|e| PrimerError::Storage(format!("v4 migration: commit: {e}")))?;
    Ok(())
}
