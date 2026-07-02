//! Hybrid (BM25 + vector cosine) retrieval read path.
//!
//! `retrieve_hybrid_inner` fuses the FTS5 BM25 leg (via the
//! `KnowledgeBase::retrieve` trait method) with an in-Rust cosine k-NN leg
//! (`knn_scan`) through Reciprocal Rank Fusion. The BM25-only trait impl and
//! the `KnowledgeBase::retrieve_hybrid` override that forwards here live with
//! the struct in `lib.rs`; this module is the hybrid extension on top of it.
//! The `_inner` name (mirroring `primer-storage`'s
//! `retrieve_session_turns_hybrid_inner`) keeps the inherent method from
//! shadowing the trait method — a same-name inherent silently masked the
//! trait's BM25-only default for every `dyn KnowledgeBase` caller once, and
//! would turn into infinite recursion if the forwarding impl ever re-resolved
//! to the trait.

use crate::SqliteKnowledgeBase;
use crate::embedding_model::validate_embedder_against_db;
use crate::vector::{blob_to_vec, cosine};
use primer_core::embedder::Embedder;
use primer_core::error::{PrimerError, Result};
use primer_core::knowledge::*;
use primer_core::rrf;

impl SqliteKnowledgeBase {
    /// Hybrid (BM25 + vector cosine) retrieval. Runs the existing FTS5
    /// path with `top_k = bm25_top_k`, runs an in-Rust cosine scan over
    /// every stored vector keeping the top `vector_top_k`, fuses the
    /// two ranked lists via Reciprocal Rank Fusion, and returns the top
    /// `final_top_k` passages by combined score.
    ///
    /// Embedder model identity is validated up-front: a registered
    /// `model_id` with a mismatching `dim` is a hard error (matches the
    /// insert-side invariant). A `model_id` not registered in this DB
    /// but with stored vectors under a different model logs a warning
    /// and returns an empty vec leg (effective BM25-only fallback) —
    /// silent degradation would invisibly hurt retrieval. A `min_score`
    /// floor and a `source_filter` allow-list apply post-fusion.
    pub(crate) async fn retrieve_hybrid_inner(
        &self,
        query: &str,
        embedder: &dyn Embedder,
        params: &HybridParams,
    ) -> Result<Vec<Passage>> {
        // 0. Validate embedder identity against what's already in the DB.
        validate_embedder_against_db(
            &self.conn.lock().unwrap(),
            embedder.model_id(),
            embedder.dim(),
        )?;

        // 1. BM25 leg via the existing path.
        let bm25 = self
            .retrieve(
                query,
                &RetrievalParams {
                    top_k: params.bm25_top_k,
                    min_score: f64::NEG_INFINITY,
                    source_filter: vec![],
                },
            )
            .await?;

        // 2. Vector leg.
        let q_vec = embedder
            .embed(&[query])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| PrimerError::Knowledge("embedder returned empty for query".into()))?;
        if q_vec.len() != embedder.dim() {
            return Err(PrimerError::Knowledge(format!(
                "embedder returned vector of len {} but reported dim {}",
                q_vec.len(),
                embedder.dim()
            )));
        }
        let vec_hits = self.knn_scan(&q_vec, embedder.model_id(), params.vector_top_k)?;

        // 3. RRF fusion. We fuse over passage `id`s (string-stable across
        //    legs) since BM25 and the vector path both produce `Passage`s.
        let bm25_ids: Vec<String> = bm25.iter().map(|p| p.id.clone()).collect();
        let vec_ids: Vec<String> = vec_hits.iter().map(|p| p.id.clone()).collect();
        let fused = rrf::fuse(&[bm25_ids.as_slice(), vec_ids.as_slice()], params.rrf_k);

        // 4. Reassemble Passage objects with fused scores. Prefer text
        //    from whichever leg produced the hit (both have the same
        //    text for a given id).
        let mut by_id: std::collections::HashMap<String, Passage> =
            std::collections::HashMap::new();
        for p in bm25.into_iter().chain(vec_hits.into_iter()) {
            by_id.entry(p.id.clone()).or_insert(p);
        }
        let mut out = Vec::with_capacity(fused.len());
        for (id, score) in fused {
            if let Some(mut p) = by_id.remove(&id) {
                p.score = score;
                if score < params.min_score {
                    continue;
                }
                if !params.source_filter.is_empty()
                    && !params.source_filter.iter().any(|s| p.source.starts_with(s))
                {
                    continue;
                }
                out.push(p);
                if out.len() >= params.final_top_k {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// In-Rust cosine top-K over every stored vector that matches the
    /// given model id. Returns `Passage`s ranked by similarity. At <=
    /// 50 K rows this scan is ~30 ms single-threaded; we revisit when
    /// corpus size justifies a `sqlite-vec` index.
    fn knn_scan(&self, query: &[f32], model_id: &str, top_k: usize) -> Result<Vec<Passage>> {
        let conn = self.conn.lock().unwrap();
        // The model_id filter on the JOIN ensures we never compare
        // vectors from incompatible models — open-time validation
        // already errors on mismatch, but this is belt-and-braces.
        let mut stmt = conn
            .prepare_cached(&self.sql.knn_scan)
            .map_err(|e| PrimerError::Knowledge(format!("prepare knn scan: {e}")))?;
        let rows = stmt
            .query_map(rusqlite::params![model_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            })
            .map_err(|e| PrimerError::Knowledge(format!("knn scan: {e}")))?;

        let mut scored: Vec<(f64, Passage)> = Vec::new();
        for r in rows {
            let (id, source, text, blob) =
                r.map_err(|e| PrimerError::Knowledge(format!("read knn row: {e}")))?;
            let v = blob_to_vec(&blob)?;
            if v.len() != query.len() {
                // Different dim; skip rather than panic. Open-time
                // validation should have caught this, so a mismatch
                // here means a hand-edited DB.
                continue;
            }
            let sim = cosine(query, &v) as f64;
            scored.push((
                sim,
                Passage {
                    id,
                    source,
                    text,
                    score: sim,
                },
            ));
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        Ok(scored.into_iter().map(|(_, p)| p).collect())
    }
}
