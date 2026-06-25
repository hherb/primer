//! # primer-knowledge
//!
//! Knowledge base implementation backed by SQLite FTS5.
//!
//! The knowledge base stores passages from Wikipedia, curated encyclopedias,
//! and curriculum materials. It supports full-text search with BM25 ranking,
//! and can be extended with embedding-based semantic search.
//!
//! ## Per-locale tables
//!
//! Each locale gets its own pair of tables: `passages_<pack_id>` (FTS5
//! virtual table) and `passages_<pack_id>_content` (external-content
//! storage). This isolation matters because BM25 statistics are
//! computed per-FTS5-index — mixing English and (e.g.) German passages
//! in a single index degrades retrieval quality for both. A future
//! locale that benefits from a different FTS5 tokenizer
//! (e.g. `porter` for English-only stemming) can configure it without
//! affecting other locales.
//!
//! ## Migration from the legacy single-table layout
//!
//! Builds prior to PR 2 used a single `passages` / `passages_content`
//! pair without locale qualification. On the first open under
//! `open_for_locale`, if the legacy `passages` table exists and the
//! locale-qualified table does not, this module migrates the legacy
//! rows into the requested locale's tables in a single transaction
//! and drops the legacy tables. The migration assumes the legacy
//! corpus was in the locale being opened — which is the only sound
//! assumption when the locale wasn't tracked at write time.

use async_trait::async_trait;
use primer_core::embedder::Embedder;
use primer_core::error::{PrimerError, Result};
use primer_core::i18n::Locale;
use primer_core::knowledge::*;
use primer_core::rrf;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

/// Current schema version. Bumped whenever a new `apply_vN_migrations`
/// is added. v1 = legacy / per-locale tables only; v2 = `sources` table;
/// v3 = `embedding_models` lookup + per-locale `embeddings_<pack>` table
/// holding one float-vector per passage for hybrid retrieval;
/// v4 = `sources.parent_source_id` self-FK for umbrella attribution (#40).
pub const USER_VERSION: i32 = 4;

/// Precomputed, locale-qualified SQL for every per-row / per-query hot
/// path. Built once per `SqliteKnowledgeBase` (in `open_for_locale`) so
/// that the bootstrap-ingest loop pays neither a `format!` allocation nor
/// an FTS5/SQL re-parse per row: each statement string is stable, so
/// `Connection::prepare_cached` resolves it to a cached `sqlite3_stmt`
/// after the first call (a hash lookup, not a parse).
///
/// This is the single source of truth for the hot-path SQL — keep the
/// schema-creation SQL (`create_per_locale_tables` etc.) out of here; that
/// runs once at open and reads more clearly inline next to the migration
/// logic it belongs to.
struct LocaleSql {
    /// Content-table row insert; binds `id`, `source`, `text` as `?1..?3`.
    insert_content: String,
    /// FTS5 index row insert; binds `rowid` as `?4` and `id`/`source`/`text`
    /// as `?1..?3` (the order `insert_passage_inner` passes them).
    insert_fts: String,
    /// Per-passage embedding insert; binds `content_rowid`, `model_id`,
    /// `vec` as `?1..?3`. Hot during the embedded bulk-ingest path.
    insert_embedding: String,
    /// Embedding backfill upsert (the `--reembed` per-row path); binds
    /// `content_rowid`, `model_id`, `vec` as `?1..?3`.
    upsert_embedding: String,
    /// Resolve a content rowid from a passage id (`?1`); the per-row lookup
    /// inside `upsert_embedding`.
    select_rowid_by_id: String,
    /// BM25 retrieval; binds the sanitized phrase as `?1` and the limit as
    /// `?2`.
    retrieve: String,
    /// Vector-leg k-NN scan join; binds the embedder model name as `?1`.
    knn_scan: String,
}

impl LocaleSql {
    /// Build every hot-path statement for one locale pack id. Pure: the
    /// only input is the pack id (a `Locale::pack_id()` projection of a
    /// closed enum, never user input), so the interpolated table names are
    /// safe and deterministic.
    fn new(pack: &str) -> Self {
        let content_table = format!("passages_{pack}_content");
        let passages_table = format!("passages_{pack}");
        let embeddings_table = format!("embeddings_{pack}");
        Self {
            insert_content: format!(
                "INSERT INTO {content_table}(id, source, text) VALUES (?1, ?2, ?3)"
            ),
            insert_fts: format!(
                "INSERT INTO {passages_table}(rowid, id, source, text) VALUES (?4, ?1, ?2, ?3)"
            ),
            insert_embedding: format!(
                "INSERT INTO {embeddings_table}(content_rowid, model_id, vec) VALUES (?1, ?2, ?3)"
            ),
            upsert_embedding: format!(
                "INSERT INTO {embeddings_table}(content_rowid, model_id, vec) VALUES (?1, ?2, ?3)
                 ON CONFLICT(content_rowid) DO UPDATE SET
                    model_id = excluded.model_id,
                    vec      = excluded.vec"
            ),
            select_rowid_by_id: format!("SELECT rowid FROM {content_table} WHERE id = ?1"),
            retrieve: format!(
                "SELECT id, source, text, rank
                 FROM {passages_table}
                 WHERE {passages_table} MATCH ?1
                 ORDER BY rank
                 LIMIT ?2"
            ),
            knn_scan: format!(
                "SELECT c.id, c.source, c.text, e.vec
                 FROM {embeddings_table} e
                 JOIN {content_table} c ON c.rowid = e.content_rowid
                 JOIN embedding_models m ON m.id = e.model_id
                 WHERE m.name = ?1"
            ),
        }
    }
}

/// SQLite FTS5-backed knowledge base, scoped to one `Locale`.
pub struct SqliteKnowledgeBase {
    conn: Mutex<Connection>,
    locale: Locale,
    /// `passages_<pack_id>_content` — the external-content storage table
    /// name. Cached at `open_for_locale` for the one-shot diagnostic
    /// queries (`passage_count`, `list_passage_ids`,
    /// `passages_missing_embedding`) that build their SQL inline; the
    /// per-row hot paths use [`LocaleSql`] instead.
    content_table: String,
    /// `embeddings_<pack_id>` — the schema-v3 vector storage table. Same
    /// one-shot-diagnostic rationale as `content_table`.
    embeddings_table: String,
    /// Precomputed hot-path SQL, fed to `prepare_cached` so bulk ingest
    /// re-parses nothing per row. See [`LocaleSql`].
    sql: LocaleSql,
}

impl SqliteKnowledgeBase {
    /// Open the knowledge base for the default locale (English).
    /// Back-compat shim for callers that pre-date the locale-aware API.
    #[deprecated(
        since = "0.1.0",
        note = "use open_for_locale to make the locale explicit; the shim defaults to English and would silently route a non-English corpus through the English-default migration path"
    )]
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_for_locale(path, Locale::default())
    }

    /// Open or create a knowledge base scoped to `locale`. The on-disk
    /// schema uses per-locale tables (`passages_<pack_id>` and
    /// `passages_<pack_id>_content`); a legacy single-table file is
    /// migrated into `locale`'s tables on first open. New empty files
    /// just create the locale-scoped tables.
    pub fn open_for_locale(path: &Path, locale: Locale) -> Result<Self> {
        let conn = Connection::open(path)
            .map_err(|e| PrimerError::Knowledge(format!("Failed to open DB: {e}")))?;

        let pack = locale.pack_id();
        migrate_or_create(&conn, pack)?;

        Ok(Self {
            conn: Mutex::new(conn),
            locale,
            content_table: format!("passages_{pack}_content"),
            embeddings_table: format!("embeddings_{pack}"),
            sql: LocaleSql::new(pack),
        })
    }

    /// Locale this knowledge base is scoped to. Useful for diagnostics
    /// and for asserting that a `DialogueManager` and its knowledge base
    /// agree on locale.
    pub fn locale(&self) -> Locale {
        self.locale
    }

    /// All passage ids currently in this locale's table. Used by the
    /// loader to dedup against existing rows in O(N+M).
    pub fn list_passage_ids(&self) -> Result<std::collections::HashSet<String>> {
        let conn = self.conn.lock().unwrap();
        let content_table = &self.content_table;
        let mut stmt = conn
            .prepare(&format!("SELECT id FROM {content_table}"))
            .map_err(|e| PrimerError::Knowledge(format!("prepare list ids: {e}")))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| PrimerError::Knowledge(format!("query ids: {e}")))?;
        let mut out = std::collections::HashSet::new();
        for r in rows {
            out.insert(r.map_err(|e| PrimerError::Knowledge(format!("read id row: {e}")))?);
        }
        Ok(out)
    }

    /// Number of passages currently in this locale's table. Used by the
    /// auto-seed-on-empty path to decide whether to load the seed corpus.
    pub fn passage_count(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let content_table = &self.content_table;
        let n: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {content_table}"), [], |r| {
                r.get(0)
            })
            .map_err(|e| PrimerError::Knowledge(format!("count passages: {e}")))?;
        Ok(n)
    }

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
    fn insert_passage_inner(
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

    /// All passage ids currently lacking an embedding. Used by the
    /// `--reembed` backfill mode. Returns ids in insertion order.
    pub fn passages_missing_embedding(&self) -> Result<Vec<(String, String)>> {
        let conn = self.conn.lock().unwrap();
        let content_table = &self.content_table;
        let embeddings_table = &self.embeddings_table;
        let sql = format!(
            "SELECT c.id, c.text
             FROM {content_table} c
             LEFT JOIN {embeddings_table} e ON e.content_rowid = c.rowid
             WHERE e.content_rowid IS NULL
             ORDER BY c.rowid"
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| PrimerError::Knowledge(format!("prepare missing query: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| PrimerError::Knowledge(format!("query missing: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| PrimerError::Knowledge(format!("read row: {e}")))?);
        }
        Ok(out)
    }

    /// Number of passages with an embedding row. For diagnostics.
    pub fn embedding_count(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let embeddings_table = &self.embeddings_table;
        conn.query_row(
            &format!("SELECT COUNT(*) FROM {embeddings_table}"),
            [],
            |r| r.get(0),
        )
        .map_err(|e| PrimerError::Knowledge(format!("count embeddings: {e}")))
    }

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
    pub async fn retrieve_hybrid(
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
fn validate_embedder_against_db(conn: &Connection, name: &str, dim: usize) -> Result<()> {
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

fn upsert_embedding_model(conn: &Connection, name: &str, dim: usize) -> Result<i64> {
    // If a row with this name exists, validate dim and return id.
    // Otherwise insert and return the new id.
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

fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

fn blob_to_vec(blob: &[u8]) -> Result<Vec<f32>> {
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

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    // Both legs are produced by an `Embedder` which contractually
    // returns L2-normalised vectors (or close to it for some real
    // models). We do NOT renormalise here; if a backend ships
    // un-normalised vectors, retrieval quality will silently degrade,
    // and that's a backend bug, not a retrieval bug.
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

#[async_trait]
impl KnowledgeBase for SqliteKnowledgeBase {
    async fn upsert_source(&self, source: &SourceMeta) -> Result<()> {
        let conn = self.conn.lock().unwrap();
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
        .map_err(|e| PrimerError::Knowledge(format!("upsert source: {e}")))?;
        Ok(())
    }

    async fn list_sources(&self) -> Result<Vec<SourceMeta>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, license, attribution, source_url, retrieved_at, parent_source_id
                 FROM sources ORDER BY id",
            )
            .map_err(|e| PrimerError::Knowledge(format!("prepare list sources: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(SourceMeta {
                    id: row.get(0)?,
                    license: row.get(1)?,
                    attribution: row.get(2)?,
                    source_url: row.get(3)?,
                    retrieved_at: row.get(4)?,
                    parent_source_id: row.get(5)?,
                })
            })
            .map_err(|e| PrimerError::Knowledge(format!("query sources: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| PrimerError::Knowledge(format!("read source row: {e}")))?);
        }
        Ok(out)
    }

    async fn retrieve(&self, query: &str, params: &RetrievalParams) -> Result<Vec<Passage>> {
        // Sanitize the input into FTS5-friendly form: strip reserved
        // keywords and non-alphanumerics, OR-join the surviving tokens.
        // Without this, a natural-language query like "how does the sun
        // shine" would AND every token together (FTS5's default) and
        // miss any passage that uses different filler words. Mirrors the
        // sanitizer in `primer-storage`; consolidating both into
        // `primer-core::fts_util` is a separate cleanup.
        let phrase = sanitize_fts_phrase(query);
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

// ─── FTS5 query sanitization ───────────────────────────────────────────────

/// Coerce free-form input into an FTS5-safe phrase. Strips non-alphanumerics
/// (so any FTS5-special character is inert), drops reserved keywords
/// (`AND`, `OR`, `NOT`, `NEAR`), quotes each surviving token, and joins
/// them with explicit `OR`. An empty result means "no useful tokens";
/// the caller short-circuits to empty results rather than issuing
/// `MATCH ''` which FTS5 rejects.
///
/// `OR` is chosen over implicit-AND so a natural-language query like
/// "how does the sun shine" surfaces passages that mention any of
/// `sun`/`shine` rather than requiring all five words. BM25 ranking +
/// the caller's `LIMIT k` keep the list focused on the most relevant.
///
/// Mirrors the same-named function in `primer-storage`. A future cleanup
/// should hoist both into `primer-core::fts_util`.
fn sanitize_fts_phrase(query: &str) -> String {
    const RESERVED: &[&str] = &["AND", "OR", "NOT", "NEAR"];
    let tokens: Vec<String> = query
        .split_whitespace()
        .map(|tok| {
            tok.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
        })
        .filter(|tok| !tok.is_empty())
        .filter(|tok| !RESERVED.iter().any(|r| r.eq_ignore_ascii_case(tok)))
        .collect();
    if tokens.is_empty() {
        return String::new();
    }
    tokens
        .iter()
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(" OR ")
}

// ─── Schema management ─────────────────────────────────────────────────────

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','view') AND name=?1",
            rusqlite::params![name],
            |r| r.get(0),
        )
        .map_err(|e| PrimerError::Knowledge(format!("check table {name}: {e}")))?;
    Ok(count > 0)
}

/// Create the per-locale tables idempotently. The FTS5 vtable uses
/// SQLite's default tokenizer (`unicode61`) — fine for both English
/// (case-folding, basic punctuation handling) and most other Latin /
/// Cyrillic scripts. Locales with morphologically richer needs
/// (heavy stemming, compound splitting) can override the tokenizer
/// here per-pack-id when we add them; today every locale shares the
/// same tokenizer.
fn create_per_locale_tables(conn: &Connection, pack: &str) -> Result<()> {
    let sql = format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS passages_{pack} USING fts5(
            id,
            source,
            text,
            content='passages_{pack}_content',
            content_rowid='rowid'
        );
        CREATE TABLE IF NOT EXISTS passages_{pack}_content(
            rowid INTEGER PRIMARY KEY,
            id TEXT NOT NULL,
            source TEXT NOT NULL,
            text TEXT NOT NULL
        );"
    );
    conn.execute_batch(&sql)
        .map_err(|e| PrimerError::Knowledge(format!("Failed to create tables: {e}")))?;
    Ok(())
}

/// Create the v3 hybrid-retrieval tables idempotently. Two tables:
/// - `embedding_models` (cross-locale): registry of every embedder
///   that has ever written into this DB. `dim` is the dimensionality
///   reported by the embedder at registration time; mismatched re-use
///   is a hard error at the call site.
/// - `embeddings_<pack>` (per-locale): one row per passage holding the
///   float vector as a `BLOB` (little-endian f32, `dim` floats). Foreign
///   keys cascade so deleting a passage drops its embedding.
fn create_embedding_tables(conn: &Connection, pack: &str) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS embedding_models(
            id   INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            dim  INTEGER NOT NULL
        );",
    )
    .map_err(|e| PrimerError::Knowledge(format!("create embedding_models: {e}")))?;
    let sql = format!(
        "CREATE TABLE IF NOT EXISTS embeddings_{pack}(
            content_rowid INTEGER PRIMARY KEY
                REFERENCES passages_{pack}_content(rowid) ON DELETE CASCADE,
            model_id      INTEGER NOT NULL REFERENCES embedding_models(id),
            vec           BLOB NOT NULL
        );
        CREATE INDEX IF NOT EXISTS ix_embeddings_{pack}_model
            ON embeddings_{pack}(model_id);"
    );
    conn.execute_batch(&sql)
        .map_err(|e| PrimerError::Knowledge(format!("create embeddings_{pack}: {e}")))?;
    Ok(())
}

/// Create the cross-locale `sources` table idempotently. Stores per-source
/// licence + attribution metadata so credits travel with the data; one
/// table for the whole DB (no locale partition) since `Passage.source`
/// values are already globally unique strings.
fn create_sources_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sources(
            id                TEXT PRIMARY KEY,
            license           TEXT NOT NULL,
            attribution       TEXT NOT NULL,
            source_url        TEXT,
            retrieved_at      INTEGER NOT NULL,
            parent_source_id  TEXT REFERENCES sources(id)
        );",
    )
    .map_err(|e| PrimerError::Knowledge(format!("create sources table: {e}")))?;
    Ok(())
}

/// v4 (#40): add the nullable self-referential `parent_source_id` column to
/// an existing `sources` table. Idempotent — guarded by a `pragma_table_info`
/// check so re-running on a DB that already has the column is a no-op. A fresh
/// DB gets the column directly from `create_sources_table` and skips this.
fn add_parent_source_id_column(conn: &Connection) -> Result<()> {
    if sources_has_parent_source_id(conn)? {
        return Ok(());
    }
    conn.execute_batch(
        "ALTER TABLE sources ADD COLUMN parent_source_id TEXT REFERENCES sources(id);",
    )
    .map_err(|e| PrimerError::Knowledge(format!("add sources.parent_source_id: {e}")))?;
    Ok(())
}

/// True if the `sources` table already has a `parent_source_id` column.
fn sources_has_parent_source_id(conn: &Connection) -> Result<bool> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(sources)")
        .map_err(|e| PrimerError::Knowledge(format!("pragma table_info sources: {e}")))?;
    let has = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| PrimerError::Knowledge(format!("read sources columns: {e}")))?
        .filter_map(|r| r.ok())
        .any(|name| name == "parent_source_id");
    Ok(has)
}

/// Read SQLite's `PRAGMA user_version`. Defaults to 0 on a fresh DB.
fn read_user_version(conn: &Connection) -> Result<i32> {
    let v: i32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(|e| PrimerError::Knowledge(format!("read user_version: {e}")))?;
    Ok(v)
}

fn write_user_version(conn: &Connection, v: i32) -> Result<()> {
    conn.pragma_update(None, "user_version", v)
        .map_err(|e| PrimerError::Knowledge(format!("write user_version: {e}")))?;
    Ok(())
}

/// Idempotent open-time setup. Cases handled:
///
/// 1. **Legacy DB without locale tables** (any version): copy rows from
///    `passages_content` into `passages_<pack>_content`, rebuild the
///    FTS index, drop the legacy tables. All in one transaction so a
///    partial failure rolls back to the pre-migration state.
/// 2. **Fresh or already-migrated DB**: create the per-locale tables
///    if they don't already exist; otherwise no-op for that step.
/// 3. **v1 → v2 schema bump**: add the cross-locale `sources` table.
///    Idempotent and additive, like every other migration in this codebase.
///
/// Migration is locale-specific: we adopt the legacy corpus into the
/// locale being opened. That's the only sound assumption when the
/// locale wasn't tracked at write time. A `user_version` newer than
/// `USER_VERSION` is rejected — same policy as `primer-storage`.
fn migrate_or_create(conn: &Connection, pack: &str) -> Result<()> {
    let existing_version = read_user_version(conn)?;
    if existing_version > USER_VERSION {
        return Err(PrimerError::Knowledge(format!(
            "knowledge DB user_version {existing_version} is newer than supported {USER_VERSION}"
        )));
    }

    let tx = conn
        .unchecked_transaction()
        .map_err(|e| PrimerError::Knowledge(format!("begin migration tx: {e}")))?;

    let legacy_exists = table_exists(&tx, "passages")?;
    let new_exists = table_exists(&tx, &format!("passages_{pack}"))?;

    if legacy_exists && !new_exists {
        tracing::info!(
            target = "primer-knowledge",
            "migrating legacy `passages` table into `passages_{pack}`"
        );
        create_per_locale_tables(&tx, pack)?;
        // Copy content first, then rebuild the FTS index from it.
        tx.execute(
            &format!(
                "INSERT INTO passages_{pack}_content(rowid, id, source, text) \
                 SELECT rowid, id, source, text FROM passages_content"
            ),
            [],
        )
        .map_err(|e| PrimerError::Knowledge(format!("copy legacy content: {e}")))?;
        tx.execute(
            &format!(
                "INSERT INTO passages_{pack}(rowid, id, source, text) \
                 SELECT rowid, id, source, text FROM passages_{pack}_content"
            ),
            [],
        )
        .map_err(|e| PrimerError::Knowledge(format!("rebuild FTS index: {e}")))?;
        tx.execute_batch("DROP TABLE passages; DROP TABLE passages_content;")
            .map_err(|e| PrimerError::Knowledge(format!("drop legacy tables: {e}")))?;
    } else {
        create_per_locale_tables(&tx, pack)?;
    }

    // v2: cross-locale sources table. Idempotent CREATE IF NOT EXISTS,
    // safe to run on every open regardless of `existing_version`.
    create_sources_table(&tx)?;

    // v4: add `sources.parent_source_id` to DBs created before v4. A fresh
    // DB already has it from `create_sources_table`; this no-ops there.
    add_parent_source_id_column(&tx)?;

    // v3: cross-locale `embedding_models` lookup + per-locale
    // `embeddings_<pack>` table. Same idempotent CREATE IF NOT EXISTS
    // shape; safe to re-run.
    create_embedding_tables(&tx, pack)?;

    tx.commit()
        .map_err(|e| PrimerError::Knowledge(format!("commit migration: {e}")))?;

    if existing_version != USER_VERSION {
        write_user_version(conn, USER_VERSION)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
