//! Schema v1 → v2: rolling session summary + FTS5 index over turn text.
//!
//! Adds the two `sessions` summary columns and the `turn_text_fts`
//! virtual table (plus its sync triggers) that back searchable
//! long-term session memory.

use primer_core::error::{PrimerError, Result};
use rusqlite::Connection;

use super::{column_exists, table_exists};

/// Apply v2 migrations idempotently. Safe to run on a fresh DB (after
/// `SCHEMA_SQL` has created the v1 tables), on a v1 DB being upgraded,
/// and on a v2 DB being re-opened.
///
/// All steps run inside a single transaction so a partial failure (e.g.
/// disk full between the FTS create and a trigger create) rolls back to
/// the pre-migration state instead of leaving an inconsistent half-v2
/// database that subsequent saves would silently miswrite to.
///
/// v2 adds:
/// - `sessions.summary` and `sessions.summary_through_turn_index` —
///   rolling LLM-generated summary of pre-window turns.
/// - `turn_text_fts` virtual table for FTS5 retrieval over `turns.text`.
/// - Triggers to keep `turn_text_fts` in sync with `turns`.
pub fn apply_v2_migrations(conn: &Connection) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| PrimerError::Storage(format!("begin v2 migration tx: {e}")))?;

    if !column_exists(&tx, "sessions", "summary")? {
        tx.execute_batch("ALTER TABLE sessions ADD COLUMN summary TEXT NOT NULL DEFAULT '';")
            .map_err(|e| PrimerError::Storage(format!("ALTER sessions ADD summary: {e}")))?;
    }
    if !column_exists(&tx, "sessions", "summary_through_turn_index")? {
        tx.execute_batch(
            "ALTER TABLE sessions ADD COLUMN summary_through_turn_index INTEGER NOT NULL DEFAULT 0;",
        )
        .map_err(|e| {
            PrimerError::Storage(format!(
                "ALTER sessions ADD summary_through_turn_index: {e}"
            ))
        })?;
    }

    // Detect whether the FTS index existed BEFORE we attempt the CREATE.
    // If we are creating it for the first time, backfill from `turns`;
    // otherwise the existing index is already kept in sync by the
    // triggers and a backfill would just duplicate rows.
    let fts_existed = table_exists(&tx, "turn_text_fts")?;

    tx.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS turn_text_fts USING fts5(\
            text, content='turns', content_rowid='id', tokenize='porter unicode61');",
    )
    .map_err(|e| PrimerError::Storage(format!("create turn_text_fts: {e}")))?;

    if !fts_existed {
        tx.execute_batch("INSERT INTO turn_text_fts(rowid, text) SELECT id, text FROM turns;")
            .map_err(|e| PrimerError::Storage(format!("backfill turn_text_fts: {e}")))?;
    }

    // Triggers keep the FTS index in sync as turns are inserted, deleted,
    // or updated. `IF NOT EXISTS` makes them idempotent across re-opens.
    tx.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS turns_ai AFTER INSERT ON turns BEGIN
             INSERT INTO turn_text_fts(rowid, text) VALUES (new.id, new.text);
         END;
         CREATE TRIGGER IF NOT EXISTS turns_ad AFTER DELETE ON turns BEGIN
             INSERT INTO turn_text_fts(turn_text_fts, rowid, text)
                 VALUES ('delete', old.id, old.text);
         END;
         CREATE TRIGGER IF NOT EXISTS turns_au AFTER UPDATE ON turns BEGIN
             INSERT INTO turn_text_fts(turn_text_fts, rowid, text)
                 VALUES ('delete', old.id, old.text);
             INSERT INTO turn_text_fts(rowid, text) VALUES (new.id, new.text);
         END;",
    )
    .map_err(|e| PrimerError::Storage(format!("create FTS triggers: {e}")))?;

    tx.commit()
        .map_err(|e| PrimerError::Storage(format!("commit v2 migration: {e}")))?;
    Ok(())
}
