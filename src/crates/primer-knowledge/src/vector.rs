//! Pure f32-vector (de)serialisation + cosine similarity helpers shared by
//! the ingest write path and the hybrid-retrieval read path.
//!
//! Vectors are stored on disk as little-endian `f32` BLOBs (one blob per
//! passage). These functions are the single source of truth for that
//! encoding; keeping them pure and in one module makes them trivially
//! unit-testable and keeps the encoding decision in exactly one place.

use primer_core::error::{PrimerError, Result};

/// Encode an `f32` slice as a little-endian byte BLOB (4 bytes per float).
pub(crate) fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

/// Decode a little-endian `f32` BLOB back into a vector. Errors if the blob
/// length is not a multiple of 4 (a corrupt or hand-edited row).
pub(crate) fn blob_to_vec(blob: &[u8]) -> Result<Vec<f32>> {
    if blob.len() % 4 != 0 {
        return Err(PrimerError::Knowledge(format!(
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

/// Dot-product cosine similarity between two vectors.
///
/// Both legs are produced by an `Embedder` which contractually returns
/// L2-normalised vectors (or close to it for some real models). We do NOT
/// renormalise here; if a backend ships un-normalised vectors, retrieval
/// quality will silently degrade, and that's a backend bug, not a
/// retrieval bug.
pub(crate) fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}
