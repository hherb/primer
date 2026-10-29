//! # primer-knowledge
//!
//! Knowledge base implementation backed by SQLite FTS5.
//!
//! The knowledge base stores passages from Wikipedia, curated encyclopedias,
//! and curriculum materials. It supports full-text search with BM25 ranking,
//! and can be extended with embedding-based semantic search.
//!
//! ## Module layout
//!
//! The crate is split by responsibility behind this crate root:
//! - [`locale_sql`] — precomputed, `prepare_cached`-friendly hot-path SQL.
//! - [`schema`] — table creation, migrations, `user_version` bookkeeping
//!   (and the public [`USER_VERSION`] constant).
//! - [`ingest`] — the passage write path (plain / embedded / `--reembed`).
//! - [`retrieval`] — the hybrid (BM25 + vector cosine) read path.
//! - [`vector`] / [`embedding_model`] — pure leaf helpers (FTS5 query
//!   sanitisation is shared via [`primer_core::fts_util`]).
//!
//! The BM25-only [`KnowledgeBase`] trait impl and the struct's lifecycle +
//! diagnostic accessors live here in the crate root.
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

// The `//!` docs above link this crate's private implementation submodules
// (each adds `impl` blocks to `SqliteKnowledgeBase`). They resolve under
// `cargo doc --document-private-items` (the contributor view), so keep them
// as links rather than escaping them (issue #330). Making them `pub` would
// leak internal impl modules into the public API. Crate-wide by placement.
#![allow(rustdoc::private_intra_doc_links)]

mod embedding_model;
mod ingest;
mod locale_sql;
mod retrieval;
mod schema;
mod vector;

use crate::locale_sql::LocaleSql;
use crate::schema::migrate_or_create;
use async_trait::async_trait;
use primer_core::error::{PrimerError, Result};
use primer_core::i18n::Locale;
use primer_core::knowledge::*;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

pub use schema::USER_VERSION;

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

        // SQLite defaults foreign_keys to OFF per-connection. The schema
        // declares FKs (`sources.parent_source_id`, the embeddings
        // ON DELETE CASCADE) whose enforcement the loader's
        // umbrella-before-child upsert ordering exists to satisfy —
        // without this pragma they were silently unenforced. Mirrors
        // `primer-storage`'s open path.
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| PrimerError::Knowledge(format!("enable foreign_keys: {e}")))?;

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
}

#[async_trait]
impl KnowledgeBase for SqliteKnowledgeBase {
    async fn upsert_source(&self, source: &SourceMeta) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        ingest::upsert_source_row(&conn, source)
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

    /// BM25-only retrieval drops the locale's stopwords (#386): with no
    /// cosine gate on this path, small talk ("can we stop now") would
    /// otherwise match passages through its filler words.
    async fn retrieve(&self, query: &str, params: &RetrievalParams) -> Result<Vec<Passage>> {
        self.bm25_search(query, params, true)
    }

    /// Trait-object dispatch for the hybrid path. Without this override,
    /// callers holding `&dyn KnowledgeBase` (the dialogue manager holds
    /// `Arc<dyn KnowledgeBase>`) fell through to the trait's DEFAULT
    /// impl, which ignores the embedder and runs BM25-only — silently
    /// disabling hybrid retrieval in production even with an embedder
    /// wired. Delegates to `retrieve_hybrid_inner` in `retrieval.rs`;
    /// the distinct `_inner` name (the `primer-storage` convention)
    /// makes the delegation unambiguous — a same-name inherent method
    /// is exactly what masked the trait default originally.
    async fn retrieve_hybrid(
        &self,
        query: &str,
        embedder: &dyn primer_core::embedder::Embedder,
        params: &HybridParams,
    ) -> Result<Vec<Passage>> {
        self.retrieve_hybrid_inner(query, embedder, params).await
    }
}

#[cfg(test)]
mod tests;
