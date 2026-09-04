//! Schema v4 → v5: per-concept comprehension assessments.
//!
//! v5 adds per-concept comprehension assessments alongside the existing
//! per-turn engagement classifications (v3). One row per (turn, concept,
//! classifier_id) — re-classification by a different classifier id lands
//! as a parallel row, preserving historical labels.

use primer_core::error::{PrimerError, Result};
use rusqlite::Connection;

const CREATE_COMPREHENSION_CLASSIFIERS_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS comprehension_classifiers (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        identifier TEXT NOT NULL UNIQUE
    )
";

/// One row per (turn, concept, classifier_id) — re-classification by a
/// different `classifier_id` lands as a parallel row, preserving historical
/// labels.
///
/// Cascade design — why `session_id` is duplicated alongside `turn_id`:
/// `session_id` carries `ON DELETE CASCADE` while `turn_id` does not. The
/// session-id cascade fires first on session deletion, so by the time SQLite
/// tries to cascade through `turns.session_id ON DELETE CASCADE` the
/// dependent comprehension rows are already gone. The duplicate `session_id`
/// column exists *because* relying on transitive cascade through `turns` was
/// not possible without one of the two FKs cascading directly. This is a
/// deliberate divergence from v3's `turn_classifications`, which omitted
/// `session_id` and consequently cannot cascade-delete a session whose turns
/// still hold classifications. Future Phase 0.3 work may bring v3 in line
/// via a v6 migration; v5 is correct as-is.
const CREATE_TURN_COMPREHENSIONS_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS turn_comprehensions (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id      TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        turn_id         INTEGER NOT NULL REFERENCES turns(id),
        concept_id      INTEGER NOT NULL REFERENCES concepts(id),
        depth_id        INTEGER NOT NULL REFERENCES understanding_depths(id),
        confidence      REAL NOT NULL,
        classifier_id   INTEGER NOT NULL REFERENCES comprehension_classifiers(id),
        evidence        TEXT,
        created_at      TIMESTAMP NOT NULL,
        UNIQUE(turn_id, concept_id, classifier_id)
    )
";

const CREATE_TURN_COMPREHENSIONS_TURN_INDEX: &str = "
    CREATE INDEX IF NOT EXISTS idx_turn_comprehensions_turn
        ON turn_comprehensions(turn_id)
";

const CREATE_TURN_COMPREHENSIONS_CONCEPT_INDEX: &str = "
    CREATE INDEX IF NOT EXISTS idx_turn_comprehensions_concept
        ON turn_comprehensions(concept_id)
";

/// Apply v5 migrations idempotently. Safe to run on a fresh DB (after
/// v4 objects exist), on a v4 DB being upgraded, and on a v5 DB being
/// re-opened.
///
/// All steps run inside a single transaction so a partial failure rolls
/// back to the pre-migration state.
///
/// v5 adds:
/// - `comprehension_classifiers` lookup table (lazy population, mirrors
///   the v3 `classifiers` table).
/// - `turn_comprehensions` table — one row per (turn, concept, classifier)
///   recording an `UnderstandingDepth` label with confidence and optional
///   evidence text. FKs into `turns`, `concepts`, `understanding_depths`,
///   and `comprehension_classifiers`.
/// - Two helper indices: by turn (to load all assessments for a turn) and
///   by concept (to trace a concept's depth trajectory across sessions).
///
/// See the doc-comment on `CREATE_TURN_COMPREHENSIONS_TABLE` for the
/// rationale behind the duplicate `session_id` column.
pub(crate) fn apply_v5_migrations(conn: &Connection) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| PrimerError::Storage(format!("v5 migration: failed to begin tx: {e}")))?;
    tx.execute(CREATE_COMPREHENSION_CLASSIFIERS_TABLE, [])
        .map_err(|e| {
            PrimerError::Storage(format!("v5 migration: comprehension_classifiers: {e}"))
        })?;
    tx.execute(CREATE_TURN_COMPREHENSIONS_TABLE, [])
        .map_err(|e| PrimerError::Storage(format!("v5 migration: turn_comprehensions: {e}")))?;
    tx.execute(CREATE_TURN_COMPREHENSIONS_TURN_INDEX, [])
        .map_err(|e| PrimerError::Storage(format!("v5 migration: turn-index: {e}")))?;
    tx.execute(CREATE_TURN_COMPREHENSIONS_CONCEPT_INDEX, [])
        .map_err(|e| PrimerError::Storage(format!("v5 migration: concept-index: {e}")))?;
    tx.commit()
        .map_err(|e| PrimerError::Storage(format!("v5 migration: commit: {e}")))?;
    Ok(())
}
