//! Embedding model registry + f32-vector serialisation helpers.
//!
//! Two halves: connection-bound functions (`validate_storage_embedder`,
//! `upsert_storage_embedding_model`) that interact with the
//! `embedding_models` lookup table, and pure functions
//! (`vec_to_blob`, `blob_to_vec`, `dot_product`) that operate on byte
//! buffers and float slices only.
//!
//! TODO: lift `vec_to_blob`, `blob_to_vec`, and `dot_product` to
//! `primer-core::vector` — the two former are duplicated verbatim in
//! `primer-knowledge/src/lib.rs` (~lines 551-577); the latter lives there
//! under the legacy name `cosine` and should be unified at the same time.

use primer_core::error::{PrimerError, Result};
use rusqlite::Connection;

/// Read-side counterpart to `upsert_storage_embedding_model`. Same
/// invariant on dim mismatch (hard error), and surfaces a tracing
/// warning when the embedder isn't registered yet but other models
/// already wrote vectors to this DB — that combination silently
/// degrades the vector leg to empty, and we want the user to see why.
pub(super) fn validate_storage_embedder(conn: &Connection, name: &str, dim: usize) -> Result<()> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT dim FROM embedding_models WHERE name = ?1",
            rusqlite::params![name],
            |r| r.get(0),
        )
        .ok();
    if let Some(existing_dim) = existing {
        if existing_dim as usize != dim {
            return Err(PrimerError::Storage(format!(
                "embedder model {name} registered with dim {existing_dim}, now reports dim {dim}; \
                 the original embedder is required to read existing vectors"
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
            "embedder {name} (dim {dim}) is not registered in this session DB but \
             {other_count} other model(s) are; vector leg will return empty until \
             turns are re-embedded"
        );
    }
    Ok(())
}

pub(super) fn upsert_storage_embedding_model(
    conn: &Connection,
    name: &str,
    dim: usize,
) -> Result<i64> {
    let existing: Option<(i64, i64)> = conn
        .query_row(
            "SELECT id, dim FROM embedding_models WHERE name = ?1",
            rusqlite::params![name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    if let Some((id, existing_dim)) = existing {
        if existing_dim as usize != dim {
            return Err(PrimerError::Storage(format!(
                "embedding model {name} previously registered with dim {existing_dim}, now reports dim {dim}"
            )));
        }
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO embedding_models(name, dim) VALUES (?1, ?2)",
        rusqlite::params![name, dim as i64],
    )
    .map_err(|e| PrimerError::Storage(format!("insert embedding_model: {e}")))?;
    Ok(conn.last_insert_rowid())
}

pub(super) fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

pub(super) fn blob_to_vec(blob: &[u8]) -> Result<Vec<f32>> {
    if blob.len() % 4 != 0 {
        return Err(PrimerError::Storage(format!(
            "embedding blob length {} not a multiple of 4",
            blob.len()
        )));
    }
    let mut out = Vec::with_capacity(blob.len() / 4);
    for chunk in blob.chunks_exact(4) {
        let arr: [u8; 4] = chunk.try_into().unwrap();
        out.push(f32::from_le_bytes(arr));
    }
    Ok(out)
}

/// Inner product of two equal-length f32 slices. Equivalent to cosine
/// similarity **only when both inputs are L2-normalized** — the function
/// does not normalize. The hybrid retrieval path relies on this: every
/// `Embedder` in `primer-embedding` (stub, fastembed/BGE-M3, ollama)
/// returns L2-normalized vectors, so dot product = cosine similarity for
/// the on-disk + query-side combination. If a future embedder ever
/// returns non-normalized vectors, the caller must normalize before
/// invoking this function (or switch to a true cosine helper).
///
/// Slices of unequal length: `iter().zip()` stops at the shorter slice,
/// matching the historical behaviour. Pinned by the `dot_product_zips_to_shorter_vector`
/// test below.
pub(super) fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vec_round_trip_through_blob() {
        let v = vec![1.0_f32, -2.5, 0.0, std::f32::consts::PI];
        let blob = vec_to_blob(&v);
        assert_eq!(blob.len(), v.len() * 4);
        let back = blob_to_vec(&blob).expect("decode");
        assert_eq!(back, v);
    }

    #[test]
    fn blob_to_vec_rejects_misaligned_length() {
        let err = blob_to_vec(&[0u8; 7]).unwrap_err().to_string();
        assert!(err.contains("not a multiple of 4"), "{err}");
    }

    #[test]
    fn vec_to_blob_handles_empty() {
        assert_eq!(vec_to_blob(&[]), Vec::<u8>::new());
        assert_eq!(blob_to_vec(&[]).unwrap(), Vec::<f32>::new());
    }

    #[test]
    fn dot_product_of_orthogonal_vectors_is_zero() {
        assert_eq!(dot_product(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
    }

    #[test]
    fn dot_product_of_aligned_unit_vectors_is_one() {
        // For L2-normalized inputs, dot product equals cosine similarity.
        let v = [0.6_f32, 0.8];
        assert!((dot_product(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn dot_product_zips_to_shorter_vector() {
        // Mirrors the production behaviour: zip stops at the shorter
        // slice. Documented here so future changes don't silently
        // diverge.
        assert_eq!(dot_product(&[1.0, 1.0, 1.0], &[2.0, 0.5]), 2.0 + 0.5);
    }

    #[test]
    fn validate_storage_embedder_accepts_registered() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE embedding_models(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, dim INTEGER NOT NULL);",
        ).unwrap();
        upsert_storage_embedding_model(&conn, "stub", 8).unwrap();
        assert!(validate_storage_embedder(&conn, "stub", 8).is_ok());
    }

    #[test]
    fn validate_storage_embedder_rejects_dim_mismatch() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE embedding_models(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, dim INTEGER NOT NULL);",
        ).unwrap();
        upsert_storage_embedding_model(&conn, "stub", 8).unwrap();
        let err = validate_storage_embedder(&conn, "stub", 16)
            .unwrap_err()
            .to_string();
        assert!(err.contains("dim 8") && err.contains("dim 16"), "{err}");
    }

    #[test]
    fn upsert_is_idempotent_on_same_dim() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE embedding_models(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, dim INTEGER NOT NULL);",
        ).unwrap();
        let id1 = upsert_storage_embedding_model(&conn, "stub", 8).unwrap();
        let id2 = upsert_storage_embedding_model(&conn, "stub", 8).unwrap();
        assert_eq!(id1, id2);
    }
}
