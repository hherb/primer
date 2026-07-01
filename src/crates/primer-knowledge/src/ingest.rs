//! Passage ingest (write path): plain inserts, embedded inserts, and the
//! `--reembed` backfill upsert. All go through the cached-statement
//! `insert_passage_inner` core so the bootstrap-ingest loop stays cheap
//! per row.

use crate::SqliteKnowledgeBase;
use crate::embedding_model::upsert_embedding_model;
use crate::locale_sql::LocaleSql;
use crate::vector::vec_to_blob;
use primer_core::embedder::Embedder;
use primer_core::error::{PrimerError, Result};
use rusqlite::Connection;

impl SqliteKnowledgeBase {
    /// Insert a passage into the knowledge base.
    pub fn insert_passage(&self, id: &str, source: &str, text: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        Self::insert_passage_inner(&conn, &self.sql, id, source, text)?;
        Ok(())
    }

    /// Core two-statement insert (content row + FTS index row), shared by
    /// the plain and embedded insert paths. Takes `&LocaleSql` rather than
    /// `&self` so it can run against either a `Connection` or a
    /// `Transaction` (both expose `prepare_cached`). The cached statements
    /// are what make the bootstrap-ingest loop cheap per row.
    pub(crate) fn insert_passage_inner(
        conn: &Connection,
        sql: &LocaleSql,
        id: &str,
        source: &str,
        text: &str,
    ) -> Result<i64> {
        conn.prepare_cached(&sql.insert_content)
            .and_then(|mut s| s.execute(rusqlite::params![id, source, text]))
            .map_err(|e| PrimerError::Knowledge(format!("Insert failed: {e}")))?;

        let rowid = conn.last_insert_rowid();

        conn.prepare_cached(&sql.insert_fts)
            .and_then(|mut s| s.execute(rusqlite::params![id, source, text, rowid]))
            .map_err(|e| PrimerError::Knowledge(format!("FTS insert failed: {e}")))?;

        Ok(rowid)
    }

    /// Insert a passage and embed its text in one transaction. The
    /// embedder is called synchronously — KB ingestion is offline by
    /// nature, and a partial state (passage without embedding) would
    /// silently degrade hybrid retrieval until backfilled. If you need
    /// async behaviour (e.g. spawning a background task), embed the
    /// text first and call `insert_passage_with_vector`.
    ///
    /// On open-time the embedder's `model_id` is registered in
    /// `embedding_models` if not already present; if present with a
    /// different `dim` the call errors (mismatched vector
    /// dimensionality cannot share a column).
    pub async fn insert_passage_with_embedding(
        &self,
        id: &str,
        source: &str,
        text: &str,
        embedder: &dyn Embedder,
    ) -> Result<()> {
        let vecs = embedder.embed(&[text]).await?;
        let vec = vecs.into_iter().next().ok_or_else(|| {
            PrimerError::Knowledge("embedder returned empty result for one input".into())
        })?;
        if vec.len() != embedder.dim() {
            return Err(PrimerError::Knowledge(format!(
                "embedder returned vector of len {} but reported dim {}",
                vec.len(),
                embedder.dim()
            )));
        }
        self.insert_passage_with_vector(id, source, text, embedder.model_id(), embedder.dim(), &vec)
    }

    /// Pre-embedded variant of `insert_passage_with_embedding`. Useful
    /// when the caller has already embedded a batch of passages and
    /// wants to amortise the embed call across many inserts.
    pub fn insert_passage_with_vector(
        &self,
        id: &str,
        source: &str,
        text: &str,
        model_id: &str,
        dim: usize,
        vec: &[f32],
    ) -> Result<()> {
        if vec.len() != dim {
            return Err(PrimerError::Knowledge(format!(
                "vector len {} != declared dim {dim}",
                vec.len()
            )));
        }
        let conn = self.conn.lock().unwrap();
        let model_row = upsert_embedding_model(&conn, model_id, dim)?;

        let tx = conn
            .unchecked_transaction()
            .map_err(|e| PrimerError::Knowledge(format!("begin insert tx: {e}")))?;
        let rowid = Self::insert_passage_inner(&tx, &self.sql, id, source, text)?;
        let blob = vec_to_blob(vec);
        tx.prepare_cached(&self.sql.insert_embedding)
            .and_then(|mut s| s.execute(rusqlite::params![rowid, model_row, blob]))
            .map_err(|e| PrimerError::Knowledge(format!("embedding insert: {e}")))?;
        tx.commit()
            .map_err(|e| PrimerError::Knowledge(format!("commit insert: {e}")))?;
        Ok(())
    }

    /// Backfill an embedding for an already-inserted passage. Used by
    /// `primer-kb-load --reembed` and by tests. Errors if the passage
    /// id is not present.
    pub fn upsert_embedding(
        &self,
        passage_id: &str,
        model_id: &str,
        dim: usize,
        vec: &[f32],
    ) -> Result<()> {
        if vec.len() != dim {
            return Err(PrimerError::Knowledge(format!(
                "vector len {} != declared dim {dim}",
                vec.len()
            )));
        }
        let conn = self.conn.lock().unwrap();
        let model_row = upsert_embedding_model(&conn, model_id, dim)?;
        let rowid: i64 = conn
            .prepare_cached(&self.sql.select_rowid_by_id)
            .and_then(|mut s| s.query_row(rusqlite::params![passage_id], |r| r.get(0)))
            .map_err(|e| PrimerError::Knowledge(format!("lookup passage {passage_id}: {e}")))?;
        let blob = vec_to_blob(vec);
        conn.prepare_cached(&self.sql.upsert_embedding)
            .and_then(|mut s| s.execute(rusqlite::params![rowid, model_row, blob]))
            .map_err(|e| PrimerError::Knowledge(format!("upsert embedding: {e}")))?;
        Ok(())
    }
}
