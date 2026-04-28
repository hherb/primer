//! # primer-knowledge
//!
//! Knowledge base implementation backed by SQLite FTS5.
//!
//! The knowledge base stores passages from Wikipedia, curated encyclopedias,
//! and curriculum materials. It supports full-text search with BM25 ranking,
//! and can be extended with embedding-based semantic search.

use async_trait::async_trait;
use primer_core::error::{PrimerError, Result};
use primer_core::knowledge::*;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

/// SQLite FTS5-backed knowledge base.
pub struct SqliteKnowledgeBase {
    conn: Mutex<Connection>,
}

impl SqliteKnowledgeBase {
    /// Open an existing knowledge base, or create a new empty one.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .map_err(|e| PrimerError::Knowledge(format!("Failed to open DB: {e}")))?;

        // Create the FTS5 table if it doesn't exist.
        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS passages USING fts5(
                id,
                source,
                text,
                content='passages_content',
                content_rowid='rowid'
            );
            CREATE TABLE IF NOT EXISTS passages_content(
                rowid INTEGER PRIMARY KEY,
                id TEXT NOT NULL,
                source TEXT NOT NULL,
                text TEXT NOT NULL
            );",
        )
        .map_err(|e| PrimerError::Knowledge(format!("Failed to create tables: {e}")))?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Insert a passage into the knowledge base.
    pub fn insert_passage(&self, id: &str, source: &str, text: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO passages_content(id, source, text) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, source, text],
        )
        .map_err(|e| PrimerError::Knowledge(format!("Insert failed: {e}")))?;

        conn.execute(
            "INSERT INTO passages(rowid, id, source, text) VALUES (last_insert_rowid(), ?1, ?2, ?3)",
            rusqlite::params![id, source, text],
        )
        .map_err(|e| PrimerError::Knowledge(format!("FTS insert failed: {e}")))?;

        Ok(())
    }
}

#[async_trait]
impl KnowledgeBase for SqliteKnowledgeBase {
    async fn retrieve(&self, query: &str, params: &RetrievalParams) -> Result<Vec<Passage>> {
        let conn = self.conn.lock().unwrap();

        // FTS5 match query with BM25 ranking.
        let mut stmt = conn
            .prepare(
                "SELECT id, source, text, rank
                 FROM passages
                 WHERE passages MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )
            .map_err(|e| PrimerError::Knowledge(format!("Query prepare failed: {e}")))?;

        let passages = stmt
            .query_map(rusqlite::params![query, params.top_k as i64], |row| {
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
