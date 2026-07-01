//! Embedder-model identity registration + validation against the per-DB
//! `embedding_models` lookup table.
//!
//! The load-bearing invariant: a `model_id` may only ever be associated
//! with a single vector dimensionality within one DB. Mismatched re-use is
//! a hard error at both the write (`upsert_embedding_model`) and read
//! (`validate_embedder_against_db`) boundary — silent fallback would
//! invisibly degrade retrieval, exactly the bug class this guards against.

use primer_core::error::{PrimerError, Result};
use rusqlite::Connection;

/// Read-side embedder validation. Symmetric to `upsert_embedding_model`
/// but read-only:
/// - registered name + matching dim → Ok(())
/// - registered name + mismatching dim → hard error (would silently
///   return zero hits otherwise because cosine on different-dim vectors
///   is meaningless and `knn_scan` skips dim mismatches)
/// - unregistered name + zero stored embeddings → Ok(()) (fresh DB or
///   pure BM25 corpus; vec leg will simply be empty)
/// - unregistered name + non-zero stored embeddings → tracing::warn but
///   Ok(()) (caller will get an empty vec leg, effective BM25-only;
///   warning surfaces the silent-degradation risk to the user)
pub(crate) fn validate_embedder_against_db(
    conn: &Connection,
    name: &str,
    dim: usize,
) -> Result<()> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT dim FROM embedding_models WHERE name = ?1",
            rusqlite::params![name],
            |r| r.get(0),
        )
        .ok();
    if let Some(existing_dim) = existing {
        if existing_dim as usize != dim {
            return Err(PrimerError::Knowledge(format!(
                "embedder model {name} registered with dim {existing_dim}, now reports dim {dim}; \
                 re-embed with `primer-kb-load --reembed --force` or restore the original model"
            )));
        }
        return Ok(());
    }
    let other_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM embedding_models WHERE name != ?1",
            rusqlite::params![name],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if other_count > 0 {
        tracing::warn!(
            "embedder {name} (dim {dim}) is not registered in this knowledge DB but \
             {other_count} other model(s) are; vector leg will return empty until \
             you re-embed (`primer-kb-load --reembed`)"
        );
    }
    Ok(())
}

/// Register `name` with `dim` if absent, returning the row id. If a row with
/// this name exists, validate the dim matches and return its id; a mismatch
/// is a hard error.
pub(crate) fn upsert_embedding_model(conn: &Connection, name: &str, dim: usize) -> Result<i64> {
    let existing: Option<(i64, i64)> = conn
        .query_row(
            "SELECT id, dim FROM embedding_models WHERE name = ?1",
            rusqlite::params![name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    if let Some((id, existing_dim)) = existing {
        if existing_dim as usize != dim {
            return Err(PrimerError::Knowledge(format!(
                "embedding model {name} previously registered with dim {existing_dim}, now reports dim {dim}; \
                 re-embed with `--reembed --force` or restore the original model"
            )));
        }
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO embedding_models(name, dim) VALUES (?1, ?2)",
        rusqlite::params![name, dim as i64],
    )
    .map_err(|e| PrimerError::Knowledge(format!("insert embedding_model: {e}")))?;
    Ok(conn.last_insert_rowid())
}
