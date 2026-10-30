//! Retrieval read path: the FTS5 BM25 query (`bm25_search`) and hybrid
//! (BM25 + vector cosine) retrieval.
//!
//! `bm25_search` backs both the BM25-only `KnowledgeBase::retrieve` and
//! the hybrid BM25 leg; stopwords are dropped unless the hybrid vector
//! leg has hits (#386). `retrieve_hybrid_inner` fuses that leg with an
//! in-Rust cosine k-NN leg (`knn_scan`) through Reciprocal Rank Fusion.
//! The BM25-only trait impl and the `KnowledgeBase::retrieve_hybrid`
//! override that forwards here live with the struct in `lib.rs`.
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
    /// Hybrid (BM25 + vector cosine) retrieval. Runs an in-Rust cosine
    /// scan over every stored vector keeping the top `vector_top_k`,
    /// applies the off-topic gate, then runs the FTS5 path with
    /// `top_k = bm25_top_k` (dropping stopwords only when the vector leg
    /// is empty), fuses the two ranked lists via Reciprocal Rank Fusion,
    /// and returns the top `final_top_k` passages by combined score.
    ///
    /// Embedder model identity is validated up-front: a registered
    /// `model_id` with a mismatching `dim` is a hard error (matches the
    /// insert-side invariant). A `model_id` not registered in this DB
    /// but with stored vectors under a different model logs a warning
    /// and returns an empty vec leg (effective BM25-only fallback) —
    /// silent degradation would invisibly hurt retrieval. A `min_score`
    /// floor and a `source_filter` allow-list apply post-fusion; the
    /// `min_top_cosine` off-topic gate applies before fusion and empties
    /// the whole result.
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

        // 1. Vector leg.
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

        // Off-topic gate (issue #307): `knn_scan` sorts by cosine, so the
        // first hit is the best match. An empty vector leg has nothing to
        // judge, so it falls through to BM25 alone.
        if let (Some(floor), Some(best)) = (params.min_top_cosine, vec_hits.first())
            && best.score < floor
        {
            tracing::debug!(
                target: "primer::retrieval",
                best_cosine = best.score,
                floor,
                "off-topic gate: hybrid KB retrieval emptied"
            );
            return Ok(vec![]);
        }

        // 2. BM25 leg. Stopwords are kept while the vector leg has hits
        //    (see `bm25_search`); with an empty vector leg the result is
        //    BM25 alone, so it drops them like BM25-only `retrieve` (#386).
        let bm25 = self.bm25_search(
            query,
            &RetrievalParams {
                top_k: params.bm25_top_k,
                min_score: f64::NEG_INFINITY,
                source_filter: vec![],
            },
            vec_hits.is_empty(),
        )?;

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
            if !sim.is_finite() {
                // A NaN breaks the sort's ordering, so `first()` would no
                // longer be the best hit the off-topic gate judges.
                tracing::warn!(target: "primer::retrieval", passage = %id, "non-finite cosine; skipping");
                continue;
            }
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

impl SqliteKnowledgeBase {
    /// The FTS5 BM25 query behind both retrieval paths.
    /// `drop_stopwords` is `true` for BM25-only [`KnowledgeBase::retrieve`]
    /// and for a hybrid query whose vector leg is empty (e.g. no stored
    /// vectors, or none under this embedder's model: the result is BM25
    /// alone). It is `false` for the hybrid BM25 leg
    /// beside a non-empty vector leg, whose off-topic defence is the cosine
    /// gate and whose 100% benchmark recall was measured with stopwords
    /// kept (dropping them cost "what is inside a tiny bug" its
    /// `seed:en:insects` hit).
    pub(crate) fn bm25_search(
        &self,
        query: &str,
        params: &RetrievalParams,
        drop_stopwords: bool,
    ) -> Result<Vec<Passage>> {
        // Sanitize the input into FTS5-friendly form: strip
        // non-alphanumerics, quote and OR-join the surviving tokens.
        // Without this, a natural-language query like "how does the sun
        // shine" would AND every token together (FTS5's default) and
        // miss any passage that uses different filler words.
        let phrase = primer_core::fts_util::sanitize_fts_phrase_excluding(query, |t| {
            drop_stopwords && primer_core::stopwords::is_stopword(self.locale, t)
        });
        if phrase.is_empty() {
            return Ok(vec![]);
        }

        let conn = self.conn.lock().unwrap();

        // FTS5 match query with BM25 ranking. The statement string is
        // precomputed in `LocaleSql` (table name comes from
        // `Locale::pack_id()`, a closed-enum projection, never user
        // input); parameters are bound positionally — the sanitized
        // phrase and limit go through `?1` and `?2`.
        let mut stmt = conn
            .prepare_cached(&self.sql.retrieve)
            .map_err(|e| PrimerError::Knowledge(format!("Query prepare failed: {e}")))?;

        let passages = stmt
            .query_map(rusqlite::params![phrase, params.top_k as i64], |row| {
                Ok(Passage {
                    id: row.get(0)?,
                    source: row.get(1)?,
                    text: row.get(2)?,
                    // FTS5 rank is negative (more negative = more relevant).
                    // We negate it so higher = more relevant.
                    score: -row.get::<_, f64>(3)?,
                })
            })
            .map_err(|e| PrimerError::Knowledge(format!("Query failed: {e}")))?
            .filter_map(|r| r.ok())
            .filter(|p| p.score >= params.min_score)
            .filter(|p| {
                params.source_filter.is_empty()
                    || params.source_filter.iter().any(|s| p.source.starts_with(s))
            })
            .collect();

        Ok(passages)
    }
}
