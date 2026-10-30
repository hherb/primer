//! Passage ingest (write path): plain and seed inserts (with their source
//! rows), embedded inserts, and the `--reembed` backfill upsert and its
//! `--force` reset. All go through the cached-statement
//! `insert_passage_inner` core so the bootstrap-ingest loop stays cheap
//! per row.

use crate::SqliteKnowledgeBase;
use crate::embedding_model::upsert_embedding_model;
use crate::locale_sql::LocaleSql;
use crate::vector::vec_to_blob;
use primer_core::embedder::Embedder;
use primer_core::error::{PrimerError, Result};
use primer_core::knowledge::SourceMeta;
use rusqlite::{Connection, Transaction, TransactionBehavior};
use std::collections::HashSet;

/// Upsert one `sources` row, refreshing its metadata on conflict. Takes a
/// `Connection` so it runs both standalone (`KnowledgeBase::upsert_source`)
/// and inside `insert_seed`'s transaction.
pub(crate) fn upsert_source_row(conn: &Connection, source: &SourceMeta) -> Result<()> {
    conn.execute(
        "INSERT INTO sources(id, license, attribution, source_url, retrieved_at, parent_source_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                license          = excluded.license,
                attribution      = excluded.attribution,
                source_url       = excluded.source_url,
                retrieved_at     = excluded.retrieved_at,
                parent_source_id = excluded.parent_source_id",
        rusqlite::params![
            source.id,
            source.license,
            source.attribution,
            source.source_url,
            source.retrieved_at,
            source.parent_source_id,
        ],
    )
    .map_err(|e| PrimerError::Knowledge(format!("upsert source {}: {e}", source.id)))?;
    Ok(())
}

impl SqliteKnowledgeBase {
    /// Insert a passage into the knowledge base. Its content row and FTS
    /// row commit together, so BM25 can always see a stored passage (#308).
    pub fn insert_passage(&self, id: &str, source: &str, text: &str) -> Result<()> {
        self.insert_passages(&[(id, source, text)])
    }

    /// Insert `(id, source, text)` passages in one transaction: either all
    /// land or none do.
    pub fn insert_passages(&self, passages: &[(&str, &str, &str)]) -> Result<()> {
        self.insert_seed(&[], passages)
    }

    /// Upsert `sources` and insert `(id, source, text)` passages in one
    /// transaction: either everything lands or nothing does. The seed
    /// loader writes a whole seed corpus through this, so a failed load
    /// leaves neither passages nor stray source rows behind (#308).
    ///
    /// Umbrella (parent-less) sources are written before their children:
    /// `sources.parent_source_id` is an enforced FK, so a child written
    /// first would fail the constraint.
    pub fn insert_seed(
        &self,
        sources: &[SourceMeta],
        passages: &[(&str, &str, &str)],
    ) -> Result<()> {
        self.insert_seed_tx(sources, passages, false).map(|_| ())
    }

    /// [`Self::insert_seed`], skipping every passage whose id is already
    /// stored. The check runs inside the write transaction, taken
    /// `IMMEDIATE`, so two processes topping up one shared KB file at once
    /// can't both insert a passage (`id` has no UNIQUE constraint). Returns
    /// how many passages were inserted.
    pub fn insert_seed_new(
        &self,
        sources: &[SourceMeta],
        passages: &[(&str, &str, &str)],
    ) -> Result<usize> {
        self.insert_seed_tx(sources, passages, true)
    }

    fn insert_seed_tx(
        &self,
        sources: &[SourceMeta],
        passages: &[(&str, &str, &str)],
        skip_existing: bool,
    ) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let behavior = if skip_existing {
            TransactionBehavior::Immediate
        } else {
            TransactionBehavior::Deferred
        };
        let tx = Transaction::new_unchecked(&conn, behavior)
            .map_err(|e| PrimerError::Knowledge(format!("begin insert tx: {e}")))?;
        let existing = if skip_existing {
            Self::passage_ids_in(&tx, &self.content_table)?
        } else {
            HashSet::new()
        };
        let (umbrellas, children): (Vec<_>, Vec<_>) =
            sources.iter().partition(|s| s.parent_source_id.is_none());
        for src in umbrellas.into_iter().chain(children) {
            upsert_source_row(&tx, src)?;
        }
        let mut inserted = 0;
        for (id, source, text) in passages {
            if existing.contains(*id) {
                continue;
            }
            Self::insert_passage_inner(&tx, &self.sql, id, source, text)?;
            inserted += 1;
        }
        tx.commit()
            .map_err(|e| PrimerError::Knowledge(format!("commit insert: {e}")))?;
        Ok(inserted)
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
            .map_err(|e| PrimerError::Knowledge(format!("insert passage {id}: {e}")))?;

        let rowid = conn.last_insert_rowid();

        conn.prepare_cached(&sql.insert_fts)
            .and_then(|mut s| s.execute(rusqlite::params![id, source, text, rowid]))
            .map_err(|e| PrimerError::Knowledge(format!("FTS insert passage {id}: {e}")))?;

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

    /// The `--reembed --force` reset (#377): delete every vector in this
    /// locale, then unregister each `embedding_models` row that no locale's
    /// vectors still use, so a model may come back under the same name at a
    /// new dim. One transaction. Returns the number of vectors deleted.
    pub fn clear_embeddings(&self) -> Result<usize> {
        let err = |e: rusqlite::Error| PrimerError::Knowledge(format!("clear embeddings: {e}"));
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction().map_err(err)?;
        let deleted = tx
            .execute(&format!("DELETE FROM {}", self.embeddings_table), [])
            .map_err(err)?;
        // Every locale's vectors, not just ours: another locale may still use a model.
        let tables: Vec<String> = tx
            .prepare(r"SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'embeddings\_%' ESCAPE '\'")
            .and_then(|mut s| s.query_map([], |r| r.get(0))?.collect())
            .map_err(err)?;
        let in_use = tables
            .iter()
            .map(|t| format!("SELECT model_id FROM {t}"))
            .collect::<Vec<_>>()
            .join(" UNION ");
        tx.execute(
            &format!("DELETE FROM embedding_models WHERE id NOT IN ({in_use})"),
            [],
        )
        .map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(deleted)
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
