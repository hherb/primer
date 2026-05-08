//! `SessionStore` search side: BM25 retrieve, hybrid (BM25 + vector cosine RRF) retrieve, plus the knn_scan_turns helper.
//!
//! Inherent `pub(super) async fn *_inner` methods on `SqliteSessionStore`.
//! The trait dispatch lives in `super::session`; each trait method is a
//! one-line delegation to the matching `_inner`. Keeps the trait surface
//! tiny and the heavy bodies grouped by responsibility.

use primer_core::error::{PrimerError, Result};
use rusqlite::Connection;
use uuid::Uuid;

use super::SqliteSessionStore;
use super::conv::parse_rfc3339;
use super::embeddings::{blob_to_vec, dot_product, validate_storage_embedder};
use super::fts::sanitize_fts_phrase;

impl SqliteSessionStore {
    pub(super) async fn retrieve_session_turns_inner(
        &self,
        session_id: Uuid,
        query: &str,
        k: usize,
        exclude_indices_at_or_after: usize,
    ) -> Result<Vec<primer_core::conversation::Turn>> {
        // Public BM25 path drops the rowid the with-ids helper exposes
        // — only the hybrid path needs the id (as a stable RRF fusion
        // key); BM25-only consumers see the trait shape.
        Ok(self
            .retrieve_session_turns_with_ids(session_id, query, k, exclude_indices_at_or_after)
            .await?
            .into_iter()
            .map(|(_, t)| t)
            .collect())
    }

    /// BM25 retrieval that also returns each turn's `turns.id` rowid.
    /// Used as a stable, collision-free RRF fusion key in the hybrid
    /// path. Both retrieval legs go through `turns.id` so keys match
    /// exactly across legs without depending on text equality.
    async fn retrieve_session_turns_with_ids(
        &self,
        session_id: Uuid,
        query: &str,
        k: usize,
        exclude_indices_at_or_after: usize,
    ) -> Result<Vec<(i64, primer_core::conversation::Turn)>> {
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
                "SELECT t.id, t.speaker_id, t.text, t.timestamp, t.intent_id
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
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Option<i64>>(4)?,
                    ))
                },
            )
            .map_err(|e| PrimerError::Storage(format!("query retrieve: {e}")))?;

        let mut out = Vec::with_capacity(k);
        for row in rows {
            let (turn_id, speaker_id, text, ts_str, intent_id) =
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
            out.push((
                turn_id,
                Turn {
                    speaker,
                    text,
                    timestamp,
                    intent,
                    concepts: vec![],
                },
            ));
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
        // The connection is locked, used, and dropped multiple times in
        // this function — once per BM25 query, once per validate, once
        // per knn scan. The `embedder.embed(...).await` between BM25 and
        // the kNN scan would otherwise hold the lock across an `.await`
        // (which would force `tokio::sync::Mutex`); instead we re-take
        // the std::sync::Mutex briefly at each step. Lock contention is
        // none in practice (single writer per DB).

        // 0. Validate embedder identity against the per-DB registry.
        {
            let conn = self.conn.lock().unwrap();
            validate_storage_embedder(&conn, embedder.model_id(), embedder.dim())?;
        }

        // 1. BM25 leg via the with-ids helper so we have stable rowid
        //    keys for RRF (locks internally + drops before returning).
        let bm25 = self
            .retrieve_session_turns_with_ids(
                session_id,
                query,
                params.bm25_top_k,
                exclude_indices_at_or_after,
            )
            .await?;

        // 2. Vector leg. Embed off-lock — this is the only `.await` in
        //    the function and the reason the lock pattern above is
        //    "lock-drop-await-relock" rather than "lock once".
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
        let vec_hits = {
            let conn = self.conn.lock().unwrap();
            Self::knn_scan_turns(
                &conn,
                session_id,
                &q_vec,
                embedder.model_id(),
                params.vector_top_k,
                exclude_indices_at_or_after,
            )?
        };

        // 3. RRF fusion keyed on `turns.id` (the SQLite rowid). Both
        //    legs SELECT the same rowid, so keys match exactly across
        //    legs — no string-formatting cost, no collision risk.
        use primer_core::conversation::Turn;
        let bm25_keys: Vec<String> = bm25.iter().map(|(id, _)| id.to_string()).collect();
        let vec_keys: Vec<String> = vec_hits.iter().map(|(id, _)| id.to_string()).collect();
        let fused =
            primer_core::rrf::fuse(&[bm25_keys.as_slice(), vec_keys.as_slice()], params.rrf_k);

        let mut by_key: std::collections::HashMap<String, Turn> = std::collections::HashMap::new();
        for (id, t) in bm25.into_iter().chain(vec_hits.into_iter()) {
            by_key.entry(id.to_string()).or_insert(t);
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

    /// In-Rust dot-product top-K over embeddings stored for this
    /// session, filtered by the embedder's model id and the same
    /// `exclude_indices_at_or_after` boundary the BM25 path uses.
    /// Equivalent to cosine similarity for L2-normalized inputs (every
    /// `Embedder` impl in this workspace returns normalized vectors).
    /// Returns `(turns.id, Turn)` pairs so the hybrid path can use the
    /// rowid as a stable RRF fusion key.
    ///
    /// Takes `&Connection` rather than `&self` so the caller controls
    /// the lock acquisition (the hybrid orchestrator drops and re-takes
    /// the lock around the embedder's `.await`).
    fn knn_scan_turns(
        conn: &Connection,
        session_id: Uuid,
        query: &[f32],
        model_id: &str,
        top_k: usize,
        exclude_indices_at_or_after: usize,
    ) -> Result<Vec<(i64, primer_core::conversation::Turn)>> {
        use primer_core::conversation::Turn;
        let mut stmt = conn
            .prepare(
                "SELECT t.id, t.speaker_id, t.text, t.timestamp, t.intent_id, e.vec
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
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                        row.get::<_, Vec<u8>>(5)?,
                    ))
                },
            )
            .map_err(|e| PrimerError::Storage(format!("query knn_scan_turns: {e}")))?;

        let mut scored: Vec<(f64, i64, Turn)> = Vec::new();
        for r in rows {
            let (turn_id, speaker_id, text, ts, intent_id, blob) =
                r.map_err(|e| PrimerError::Storage(format!("read knn row: {e}")))?;
            let v = blob_to_vec(&blob)?;
            if v.len() != query.len() {
                continue;
            }
            let sim = dot_product(query, &v) as f64;
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
                turn_id,
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
        Ok(scored.into_iter().map(|(_, id, t)| (id, t)).collect())
    }
}
