//! Schema v7 → v8: per-turn embedding storage for hybrid retrieval.
//!
//! Adds the `embedding_models` registry (mirroring `primer-knowledge`'s,
//! so cross-model mixing is detectable) and the `embeddings_turns` table
//! holding one little-endian f32 BLOB per turn.

use primer_core::error::{PrimerError, Result};
use rusqlite::Connection;

/// Schema v7 → v8: add the `embedding_models` lookup table and the
/// `embeddings_turns` per-turn vector storage table for hybrid
/// long-term-memory retrieval. Idempotent CREATE IF NOT EXISTS shape;
/// `embedding_models` mirrors the registry in `primer-knowledge` so
/// cross-model mixing is detectable. Vectors are stored as little-endian
/// f32 BLOBs, one row per turn, joined back via `turn_id`. ON DELETE
/// CASCADE so a session deletion sweeps embeddings with it.
pub(crate) fn apply_v8_migrations(conn: &Connection) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| PrimerError::Storage(format!("v8 migration: failed to begin tx: {e}")))?;

    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS embedding_models(
            id   INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            dim  INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS embeddings_turns(
            turn_id   INTEGER PRIMARY KEY REFERENCES turns(id) ON DELETE CASCADE,
            model_id  INTEGER NOT NULL REFERENCES embedding_models(id),
            vec       BLOB NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_embeddings_turns_model
            ON embeddings_turns(model_id);",
    )
    .map_err(|e| PrimerError::Storage(format!("v8 migration: create tables: {e}")))?;

    tx.commit()
        .map_err(|e| PrimerError::Storage(format!("v8 migration: commit: {e}")))?;
    Ok(())
}
