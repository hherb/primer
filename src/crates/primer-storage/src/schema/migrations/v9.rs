//! Schema v8 → v9: let a session delete cascade through
//! `turn_classifications` (issue #331).
//!
//! v3 created `turn_classifications.turn_id REFERENCES turns(id)` with no
//! `ON DELETE CASCADE`, so deleting a session whose turns had been
//! classified failed with `FOREIGN KEY constraint failed`: the cascade from
//! `sessions` into `turns` hit the plain FK. SQLite cannot add a cascade to
//! an existing FK in place, so v9 rebuilds the table (create / copy / drop /
//! rename). `turns.session_id` already cascades from `sessions`, so a
//! cascading `turn_id` is enough; no `session_id` column is needed.

use primer_core::error::{PrimerError, Result};
use rusqlite::Connection;

/// The v3 shape plus `ON DELETE CASCADE` on `turn_id`. Same column order,
/// so the copy below is a plain `SELECT *`.
const CREATE_CASCADING_TURN_CLASSIFICATIONS: &str = "
    CREATE TABLE turn_classifications_v9 (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        turn_id             INTEGER NOT NULL REFERENCES turns(id) ON DELETE CASCADE,
        engagement_state_id INTEGER NOT NULL REFERENCES engagement_states(id),
        classifier_id       INTEGER NOT NULL REFERENCES classifiers(id),
        confidence          REAL NOT NULL,
        reasoning           TEXT,
        classified_at       TIMESTAMP NOT NULL,
        UNIQUE(turn_id, classifier_id)
    )
";

/// Whether `turn_classifications.turn_id` already cascades — the
/// idempotency guard, so the rebuild runs once per database.
fn turn_id_cascades(conn: &Connection) -> Result<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_foreign_key_list('turn_classifications')
             WHERE \"from\" = 'turn_id' AND on_delete = 'CASCADE'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| PrimerError::Storage(format!("v9 migration: read foreign keys: {e}")))?;
    Ok(count > 0)
}

/// Schema v8 → v9: rebuild `turn_classifications` with a cascading
/// `turn_id`. Idempotent (a no-op once the FK cascades) and wrapped in one
/// transaction, so a failure mid-rebuild leaves the v8 table intact. Rows,
/// ids, the `UNIQUE(turn_id, classifier_id)` constraint and the `turn_id`
/// index all carry over. Nothing references `turn_classifications`, so the
/// drop / rename is safe with `foreign_keys = ON`.
pub(crate) fn apply_v9_migrations(conn: &Connection) -> Result<()> {
    if turn_id_cascades(conn)? {
        return Ok(());
    }
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| PrimerError::Storage(format!("v9 migration: failed to begin tx: {e}")))?;

    tx.execute_batch(&format!(
        "{CREATE_CASCADING_TURN_CLASSIFICATIONS};
         INSERT INTO turn_classifications_v9 SELECT * FROM turn_classifications;
         DROP TABLE turn_classifications;
         ALTER TABLE turn_classifications_v9 RENAME TO turn_classifications;
         CREATE INDEX IF NOT EXISTS idx_turn_classifications_turn_id
             ON turn_classifications(turn_id);"
    ))
    .map_err(|e| {
        PrimerError::Storage(format!("v9 migration: rebuild turn_classifications: {e}"))
    })?;

    tx.commit()
        .map_err(|e| PrimerError::Storage(format!("v9 migration: commit: {e}")))?;
    Ok(())
}
