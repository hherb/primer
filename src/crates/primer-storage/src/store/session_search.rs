//! `SessionStore` search side: BM25 retrieve, hybrid (BM25 + vector cosine RRF) retrieve, plus the knn_scan_turns helper.
//!
//! Inherent `pub(super) async fn *_inner` methods on `SqliteSessionStore`.
//! The trait dispatch lives in `super::session`; each trait method is a
//! one-line delegation to the matching `_inner`. Keeps the trait surface
//! tiny and the heavy bodies grouped by responsibility.

use primer_core::error::{PrimerError, Result};
use uuid::Uuid;

use super::SqliteSessionStore;
use super::conv::parse_rfc3339;
use super::embeddings::{blob_to_vec, cosine, validate_storage_embedder};
use super::fts::sanitize_fts_phrase;

impl SqliteSessionStore {
    pub(super) async fn retrieve_session_turns_inner(
        &self,
        session_id: Uuid,
        query: &str,
        k: usize,
        exclude_indices_at_or_after: usize,
    ) -> Result<Vec<primer_core::conversation::Turn>> {
        use primer_core::conversation::Turn;

        let phrase = sanitize_fts_phrase(query);
        // Empty input → nothing to match. Avoids issuing a `MATCH '""'`
        // which FTS5 rejects as a syntax error.
        if phrase.is_empty() {
            return Ok(vec![]);
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT t.speaker_id, t.text, t.timestamp, t.intent_id
                 FROM turn_text_fts f
                 JOIN turns t ON t.id = f.rowid
                 WHERE f.text MATCH ?1
                   AND t.session_id = ?2
                   AND t.turn_index < ?3
                 ORDER BY bm25(turn_text_fts)
                 LIMIT ?4",
            )
            .map_err(|e| PrimerError::Storage(format!("prepare retrieve: {e}")))?;

        let rows = stmt
            .query_map(
                rusqlite::params![
                    phrase,
                    session_id.to_string(),
                    exclude_indices_at_or_after as i64,
                    k as i64,
                ],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                    ))
                },
            )
            .map_err(|e| PrimerError::Storage(format!("query retrieve: {e}")))?;

        let mut out = Vec::with_capacity(k);
        for row in rows {
            let (speaker_id, text, ts_str, intent_id) =
                row.map_err(|e| PrimerError::Storage(format!("read retrieve row: {e}")))?;
            let speaker = crate::catalog::speaker_from_id(speaker_id)
                .ok_or_else(|| PrimerError::Storage(format!("unknown speaker_id {speaker_id}")))?;
            let intent = match intent_id {
                None => None,
                Some(id) => Some(
                    crate::catalog::intent_from_id(id)
                        .ok_or_else(|| PrimerError::Storage(format!("unknown intent_id {id}")))?,
                ),
            };
            let timestamp = parse_rfc3339(&ts_str, "turn timestamp")?;
            out.push(Turn {
                speaker,
                text,
                timestamp,
                intent,
                concepts: vec![],
            });
        }
        Ok(out)
    }

    pub(super) async fn retrieve_session_turns_hybrid_inner(
        &self,
        session_id: Uuid,
        query: &str,
        embedder: &dyn primer_core::embedder::Embedder,
        params: &primer_core::knowledge::HybridParams,
        exclude_indices_at_or_after: usize,
    ) -> Result<Vec<primer_core::conversation::Turn>> {
        // 0. Validate embedder identity against the per-DB registry.
        validate_storage_embedder(
            &self.conn.lock().unwrap(),
            embedder.model_id(),
            embedder.dim(),
        )?;

        // 1. BM25 leg via the existing path.
        let bm25 = self
            .retrieve_session_turns_inner(
                session_id,
                query,
                params.bm25_top_k,
                exclude_indices_at_or_after,
            )
            .await?;

        // 2. Vector leg.
        let q_vec = embedder
            .embed(&[query])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                PrimerError::Storage("retrieve_session_turns_hybrid: empty embed".into())
            })?;
        if q_vec.len() != embedder.dim() {
            return Err(PrimerError::Storage(format!(
                "retrieve_session_turns_hybrid: vec len {} != dim {}",
                q_vec.len(),
                embedder.dim()
            )));
        }
        let vec_hits = self.knn_scan_turns(
            session_id,
            &q_vec,
            embedder.model_id(),
            params.vector_top_k,
            exclude_indices_at_or_after,
        )?;

        // 3. RRF over (speaker, text, timestamp) signature — turns
        //    don't have a natural string id, so we construct a stable
        //    composite key using `(turn_index_string, text-hash)`. The
        //    underlying retrieval methods both go through the same
        //    `(session_id, turn_index)` PK, but we don't have access
        //    to turn_index from the BM25 path's `Turn` shape. Easiest
        //    fix: use turn `text` as the fusion key — guaranteed unique
        //    within a single session because turn texts are user input
        //    and primer responses, never empty, and the chance of a
        //    collision in practice is negligible. Belt-and-braces: also
        //    include the timestamp.
        use primer_core::conversation::Turn;
        let key = |t: &Turn| format!("{}::{}", t.timestamp.timestamp_micros(), t.text);
        let bm25_keys: Vec<String> = bm25.iter().map(key).collect();
        let vec_keys: Vec<String> = vec_hits.iter().map(key).collect();
        let fused =
            primer_core::rrf::fuse(&[bm25_keys.as_slice(), vec_keys.as_slice()], params.rrf_k);

        let mut by_key: std::collections::HashMap<String, Turn> = std::collections::HashMap::new();
        for t in bm25.into_iter().chain(vec_hits.into_iter()) {
            by_key.entry(key(&t)).or_insert(t);
        }
        let mut out = Vec::with_capacity(params.final_top_k);
        for (k, _score) in fused {
            if let Some(t) = by_key.remove(&k) {
                out.push(t);
                if out.len() >= params.final_top_k {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// In-Rust cosine top-K over embeddings stored for this session,
    /// filtered by the embedder's model id and the same
    /// `exclude_indices_at_or_after` boundary the BM25 path uses.
    fn knn_scan_turns(
        &self,
        session_id: Uuid,
        query: &[f32],
        model_id: &str,
        top_k: usize,
        exclude_indices_at_or_after: usize,
    ) -> Result<Vec<primer_core::conversation::Turn>> {
        use primer_core::conversation::Turn;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT t.speaker_id, t.text, t.timestamp, t.intent_id, e.vec
                 FROM embeddings_turns e
                 JOIN turns t ON t.id = e.turn_id
                 JOIN embedding_models m ON m.id = e.model_id
                 WHERE m.name = ?1
                   AND t.session_id = ?2
                   AND t.turn_index < ?3",
            )
            .map_err(|e| PrimerError::Storage(format!("prepare knn_scan_turns: {e}")))?;
        let rows = stmt
            .query_map(
                rusqlite::params![
                    model_id,
                    session_id.to_string(),
                    exclude_indices_at_or_after as i64,
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                    ))
                },
            )
            .map_err(|e| PrimerError::Storage(format!("query knn_scan_turns: {e}")))?;

        let mut scored: Vec<(f64, Turn)> = Vec::new();
        for r in rows {
            let (speaker_id, text, ts, intent_id, blob) =
                r.map_err(|e| PrimerError::Storage(format!("read knn row: {e}")))?;
            let v = blob_to_vec(&blob)?;
            if v.len() != query.len() {
                continue;
            }
            let sim = cosine(query, &v) as f64;
            let speaker = crate::catalog::speaker_from_id(speaker_id)
                .ok_or_else(|| PrimerError::Storage(format!("unknown speaker_id {speaker_id}")))?;
            let intent = match intent_id {
                None => None,
                Some(id) => Some(
                    crate::catalog::intent_from_id(id)
                        .ok_or_else(|| PrimerError::Storage(format!("unknown intent_id {id}")))?,
                ),
            };
            let timestamp = parse_rfc3339(&ts, "turn timestamp")?;
            scored.push((
                sim,
                Turn {
                    speaker,
                    text,
                    timestamp,
                    intent,
                    concepts: vec![],
                },
            ));
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        Ok(scored.into_iter().map(|(_, t)| t).collect())
    }
}
