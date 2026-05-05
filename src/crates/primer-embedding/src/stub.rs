//! Deterministic in-process [`Embedder`] for tests and the offline
//! escape hatch.
//!
//! The stub produces a fixed-dimension `f32` vector from each input
//! string by feeding the raw bytes through a small splittable hash and
//! L2-normalising the result. Two identical strings produce identical
//! vectors across processes; different strings almost always produce
//! different vectors. The stub embeds in microseconds and never
//! touches the network, so it's the right default for the workspace
//! test suite, for CI, and as a runtime fallback when a real-model
//! download fails.
//!
//! The stub is **not** a substitute for semantic similarity. Two texts
//! that mean the same thing but use different words will produce
//! orthogonal vectors. Tests that assert hybrid retrieval semantics
//! against the stub should therefore check structural properties
//! (e.g. "the hybrid output contains the BM25 top-1") rather than
//! semantic ones (e.g. "synonyms cluster"). The real-model semantic
//! tests live behind the `fastembed` feature flag and are
//! `#[ignore]`-by-default.
//!
//! [`Embedder`]: primer_core::embedder::Embedder

use async_trait::async_trait;
use primer_core::embedder::Embedder;
use primer_core::error::Result;
use primer_core::speech::Named;

/// Stable identifier persisted in the per-DB `embedding_models` lookup
/// table. The trailing version suffix means we can change the hashing
/// scheme later by bumping the suffix; existing DBs will then fail
/// the open-time model-mismatch check and the user gets a clear
/// `--reembed --force` instruction.
pub const STUB_MODEL_ID: &str = "stub-fxhash-v1";

/// Default vector dimensionality. 384 mirrors `bge-small-en-v1.5` so
/// users moving from stub to a real small model don't have to migrate
/// their schema. (BGE-M3, the project default real model, is 1024;
/// switching between stub and BGE-M3 will trigger a `--reembed --force`
/// regardless, so picking 384 vs 1024 here is just convention.)
pub const STUB_DEFAULT_DIM: usize = 384;

/// Deterministic hash-based [`Embedder`].
#[derive(Debug, Clone)]
pub struct StubEmbedder {
    dim: usize,
}

impl StubEmbedder {
    /// Construct a stub embedder with [`STUB_DEFAULT_DIM`] dimensions.
    pub fn new() -> Self {
        Self {
            dim: STUB_DEFAULT_DIM,
        }
    }

    /// Construct a stub embedder with a custom dimensionality. Useful
    /// for unit tests that want to assert dim handling without
    /// allocating 384-element vectors per call.
    pub fn with_dim(dim: usize) -> Self {
        assert!(dim > 0, "stub embedder dim must be > 0");
        Self { dim }
    }
}

impl Default for StubEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

impl Named for StubEmbedder {
    fn name(&self) -> &str {
        STUB_MODEL_ID
    }
}

#[async_trait]
impl Embedder for StubEmbedder {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| hash_to_vector(t, self.dim)).collect())
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn model_id(&self) -> &str {
        STUB_MODEL_ID
    }
}

/// Splittable, deterministic hash → fixed-dim L2-normalised vector.
///
/// The construction:
/// 1. Seed an FNV-style 64-bit state from the input bytes (one pass).
/// 2. Stretch the state into `dim` `f32`s by repeated multiply-mix
///    (xorshift-style). This is not cryptographic; it is just a cheap
///    deterministic stream.
/// 3. Map each chunk into the range `[-1.0, 1.0]` so vectors sit on
///    both sides of the origin. Plain unsigned-to-float mapping was
///    rejected because it would put every stub vector in the same
///    octant of the embedding space, making cosine similarity
///    artificially high on unrelated inputs.
/// 4. L2-normalise. Cosine similarity is then just a dot product, the
///    common contract that real embedding models also satisfy.
fn hash_to_vector(text: &str, dim: usize) -> Vec<f32> {
    // FNV-1a-ish 64-bit hash of the input.
    let mut state: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in text.as_bytes() {
        state ^= b as u64;
        state = state.wrapping_mul(0x100_0000_01b3);
    }

    // Stretch into a deterministic stream of f32s. xorshift-multiply
    // is overkill for this use case but keeps the per-element correlation
    // low enough that distinct inputs reliably produce distinct vectors.
    let mut out = Vec::with_capacity(dim);
    for _ in 0..dim {
        state ^= state.wrapping_shl(13);
        state ^= state.wrapping_shr(7);
        state ^= state.wrapping_shl(17);
        // Pull the top 24 bits as the float payload; this gives ~7 bits
        // of mantissa precision per element, plenty for stub purposes.
        let bits = (state >> 40) as u32;
        let v = (bits as f32 / ((1u32 << 24) as f32 - 1.0)) * 2.0 - 1.0;
        out.push(v);
    }

    // L2-normalise.
    let norm: f32 = out.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut out {
            *x /= norm;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
    }

    fn norm(a: &[f32]) -> f32 {
        a.iter().map(|x| x * x).sum::<f32>().sqrt()
    }

    #[tokio::test]
    async fn embed_round_trip_returns_one_vec_per_input() {
        let s = StubEmbedder::new();
        let v = s.embed(&["hello", "world", "primer"]).await.unwrap();
        assert_eq!(v.len(), 3);
        assert!(v.iter().all(|x| x.len() == STUB_DEFAULT_DIM));
    }

    #[tokio::test]
    async fn embed_empty_input_returns_empty_output() {
        let s = StubEmbedder::new();
        let v = s.embed(&[]).await.unwrap();
        assert!(v.is_empty());
    }

    #[tokio::test]
    async fn embed_is_deterministic_across_calls() {
        let s = StubEmbedder::new();
        let a = s.embed(&["the quick brown fox"]).await.unwrap();
        let b = s.embed(&["the quick brown fox"]).await.unwrap();
        assert_eq!(a, b, "stub embedder must be deterministic");
    }

    #[tokio::test]
    async fn embed_distinct_inputs_yield_distinct_vectors() {
        let s = StubEmbedder::new();
        let v = s
            .embed(&["alpha", "beta", "gamma", "delta", ""])
            .await
            .unwrap();
        // Pairwise inequality. (Five inputs is plenty to guarantee
        // distinguishability under the FNV+xorshift mix; if any two
        // produce identical vectors, the construction is broken.)
        for i in 0..v.len() {
            for j in (i + 1)..v.len() {
                assert_ne!(
                    v[i], v[j],
                    "distinct inputs {i} and {j} produced identical vectors"
                );
            }
        }
    }

    #[tokio::test]
    async fn embed_outputs_are_l2_normalised() {
        let s = StubEmbedder::new();
        let v = s.embed(&["hello", "primer", "x"]).await.unwrap();
        for vec in &v {
            let n = norm(vec);
            assert!((n - 1.0).abs() < 1e-5, "expected L2 norm ~= 1.0, got {n}");
        }
    }

    #[tokio::test]
    async fn cosine_with_self_is_one() {
        let s = StubEmbedder::new();
        let v = s.embed(&["any old text"]).await.unwrap();
        let sim = cosine(&v[0], &v[0]);
        assert!((sim - 1.0).abs() < 1e-5);
    }

    #[tokio::test]
    async fn with_dim_overrides_dimensionality() {
        let s = StubEmbedder::with_dim(16);
        let v = s.embed(&["a", "b"]).await.unwrap();
        assert_eq!(v.len(), 2);
        assert!(v.iter().all(|x| x.len() == 16));
        assert_eq!(s.dim(), 16);
    }

    #[test]
    fn model_id_and_name_match_constant() {
        let s = StubEmbedder::new();
        assert_eq!(s.model_id(), STUB_MODEL_ID);
        assert_eq!(<StubEmbedder as Named>::name(&s), STUB_MODEL_ID);
    }

    #[test]
    #[should_panic(expected = "stub embedder dim must be > 0")]
    fn with_dim_zero_panics() {
        let _ = StubEmbedder::with_dim(0);
    }
}
