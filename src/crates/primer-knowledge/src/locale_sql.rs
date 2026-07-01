//! Precomputed, locale-qualified SQL for the per-row / per-query hot paths.

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
pub(crate) struct LocaleSql {
    /// Content-table row insert; binds `id`, `source`, `text` as `?1..?3`.
    pub(crate) insert_content: String,
    /// FTS5 index row insert; binds `rowid` as `?4` and `id`/`source`/`text`
    /// as `?1..?3` (the order `insert_passage_inner` passes them).
    pub(crate) insert_fts: String,
    /// Per-passage embedding insert; binds `content_rowid`, `model_id`,
    /// `vec` as `?1..?3`. Hot during the embedded bulk-ingest path.
    pub(crate) insert_embedding: String,
    /// Embedding backfill upsert (the `--reembed` per-row path); binds
    /// `content_rowid`, `model_id`, `vec` as `?1..?3`.
    pub(crate) upsert_embedding: String,
    /// Resolve a content rowid from a passage id (`?1`); the per-row lookup
    /// inside `upsert_embedding`.
    pub(crate) select_rowid_by_id: String,
    /// BM25 retrieval; binds the sanitized phrase as `?1` and the limit as
    /// `?2`.
    pub(crate) retrieve: String,
    /// Vector-leg k-NN scan join; binds the embedder model name as `?1`.
    pub(crate) knn_scan: String,
}

impl LocaleSql {
    /// Build every hot-path statement for one locale pack id. Pure: the
    /// only input is the pack id (a `Locale::pack_id()` projection of a
    /// closed enum, never user input), so the interpolated table names are
    /// safe and deterministic.
    pub(crate) fn new(pack: &str) -> Self {
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
