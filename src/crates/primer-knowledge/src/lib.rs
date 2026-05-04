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
use primer_core::error::{PrimerError, Result};
use primer_core::i18n::Locale;
use primer_core::knowledge::*;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

/// SQLite FTS5-backed knowledge base, scoped to one `Locale`.
pub struct SqliteKnowledgeBase {
    conn: Mutex<Connection>,
    locale: Locale,
}

impl SqliteKnowledgeBase {
    /// Open the knowledge base for the default locale (English).
    /// Back-compat shim for callers that pre-date the locale-aware API.
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

        migrate_or_create(&conn, locale.pack_id())?;

        Ok(Self {
            conn: Mutex::new(conn),
            locale,
        })
    }

    /// Locale this knowledge base is scoped to. Useful for diagnostics
    /// and for asserting that a `DialogueManager` and its knowledge base
    /// agree on locale.
    pub fn locale(&self) -> Locale {
        self.locale
    }

    fn passages_table(&self) -> String {
        format!("passages_{}", self.locale.pack_id())
    }

    fn content_table(&self) -> String {
        format!("passages_{}_content", self.locale.pack_id())
    }

    /// Insert a passage into the knowledge base.
    pub fn insert_passage(&self, id: &str, source: &str, text: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let content_table = self.content_table();
        let passages_table = self.passages_table();

        conn.execute(
            &format!("INSERT INTO {content_table}(id, source, text) VALUES (?1, ?2, ?3)"),
            rusqlite::params![id, source, text],
        )
        .map_err(|e| PrimerError::Knowledge(format!("Insert failed: {e}")))?;

        conn.execute(
            &format!(
                "INSERT INTO {passages_table}(rowid, id, source, text) \
                 VALUES (last_insert_rowid(), ?1, ?2, ?3)"
            ),
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
        let passages_table = self.passages_table();

        // FTS5 match query with BM25 ranking. Table name is interpolated
        // (NOT user input — comes from `Locale::pack_id()`, a closed
        // enum projection), parameters are bound positionally — the
        // user query and limit go through `?1` and `?2`.
        let sql = format!(
            "SELECT id, source, text, rank
             FROM {passages_table}
             WHERE {passages_table} MATCH ?1
             ORDER BY rank
             LIMIT ?2"
        );
        let mut stmt = conn
            .prepare(&sql)
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

// ─── Schema management ─────────────────────────────────────────────────────

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
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

/// Idempotent open-time setup. Four cases:
///
/// 1. **Legacy-only**: copy rows from `passages_content` into
///    `passages_<pack>_content`, rebuild the FTS index, drop the
///    legacy tables. All in one transaction so a partial failure rolls
///    back to the pre-migration state.
/// 2. **Locale-only**: standard re-open, no-op (the
///    `CREATE ... IF NOT EXISTS` calls all short-circuit).
/// 3. **Fresh DB**: create the locale tables.
/// 4. **Both legacy and locale tables present**: ambiguous state — most
///    likely a manual user intervention or a partially-completed
///    pre-PR-2 migration that was hand-fixed. We don't auto-merge
///    (no sound rule for which side wins on row-id collision) and we
///    don't drop legacy data. We log a `tracing::warn!` so the orphan
///    is visible in logs, and proceed with the locale tables intact.
///
/// Migration is locale-specific: we adopt the legacy corpus into the
/// locale being opened. That's the only sound assumption when the
/// locale wasn't tracked at write time.
fn migrate_or_create(conn: &Connection, pack: &str) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| PrimerError::Knowledge(format!("begin migration tx: {e}")))?;

    let legacy_exists = table_exists(&tx, "passages")?;
    let new_exists = table_exists(&tx, &format!("passages_{pack}"))?;

    if legacy_exists && !new_exists {
        tracing::info!(
            target: "primer-knowledge",
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
        if legacy_exists && new_exists {
            tracing::warn!(
                target: "primer-knowledge",
                "knowledge DB has both legacy `passages` and locale-scoped \
                 `passages_{pack}` tables; legacy rows are orphaned and not \
                 readable through the locale-aware API. Inspect the file or \
                 drop the legacy tables manually if you want to recover them."
            );
        }
        create_per_locale_tables(&tx, pack)?;
    }

    tx.commit()
        .map_err(|e| PrimerError::Knowledge(format!("commit migration: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    /// Create a fresh temp file path for a test DB. The `NamedTempFile`
    /// guard auto-cleans on drop (covers the panic path too) and the
    /// path is unique even under `cargo test --test-threads=N`.
    fn tmp_db() -> NamedTempFile {
        NamedTempFile::with_suffix(".sqlite").expect("temp file")
    }

    #[tokio::test]
    async fn open_for_english_creates_per_locale_tables() {
        let f = tmp_db();
        let kb = SqliteKnowledgeBase::open_for_locale(f.path(), Locale::English).unwrap();
        assert_eq!(kb.locale(), Locale::English);
        // Insert + retrieve round-trip.
        kb.insert_passage("p1", "test:source", "the quick brown fox jumps")
            .unwrap();
        let got = kb
            .retrieve(
                "fox",
                &RetrievalParams {
                    top_k: 5,
                    min_score: f64::NEG_INFINITY,
                    source_filter: vec![],
                },
            )
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "p1");
    }

    #[tokio::test]
    async fn open_back_compat_shim_uses_default_locale() {
        let f = tmp_db();
        let kb = SqliteKnowledgeBase::open(f.path()).unwrap();
        assert_eq!(kb.locale(), Locale::default());
    }

    #[tokio::test]
    async fn reopen_existing_db_is_a_no_op() {
        let f = tmp_db();
        {
            let kb = SqliteKnowledgeBase::open_for_locale(f.path(), Locale::English).unwrap();
            kb.insert_passage("p1", "src", "hello world").unwrap();
        }
        let kb = SqliteKnowledgeBase::open_for_locale(f.path(), Locale::English).unwrap();
        let got = kb
            .retrieve(
                "hello",
                &RetrievalParams {
                    top_k: 5,
                    min_score: f64::NEG_INFINITY,
                    source_filter: vec![],
                },
            )
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
    }

    #[tokio::test]
    async fn legacy_db_migrates_into_per_locale_tables_with_data_preserved() {
        let f = tmp_db();
        // Hand-roll a legacy schema (the pre-PR-2 layout) and pre-seed
        // a row, then open under the new API and verify migration.
        {
            let conn = Connection::open(f.path()).unwrap();
            conn.execute_batch(
                "CREATE VIRTUAL TABLE passages USING fts5(
                    id, source, text,
                    content='passages_content', content_rowid='rowid'
                );
                CREATE TABLE passages_content(
                    rowid INTEGER PRIMARY KEY,
                    id TEXT NOT NULL,
                    source TEXT NOT NULL,
                    text TEXT NOT NULL
                );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO passages_content(rowid, id, source, text)
                 VALUES (1, 'legacy-1', 'legacy:source', 'photosynthesis is amazing')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO passages(rowid, id, source, text)
                 VALUES (1, 'legacy-1', 'legacy:source', 'photosynthesis is amazing')",
                [],
            )
            .unwrap();
        }

        let kb = SqliteKnowledgeBase::open_for_locale(f.path(), Locale::English).unwrap();
        let got = kb
            .retrieve(
                "photosynthesis",
                &RetrievalParams {
                    top_k: 5,
                    min_score: f64::NEG_INFINITY,
                    source_filter: vec![],
                },
            )
            .await
            .unwrap();
        assert_eq!(got.len(), 1, "legacy row should migrate into passages_en");
        assert_eq!(got[0].id, "legacy-1");

        // Legacy tables should be gone, locale-suffixed tables present.
        let conn = kb.conn.lock().unwrap();
        assert!(!table_exists(&conn, "passages").unwrap());
        assert!(!table_exists(&conn, "passages_content").unwrap());
        assert!(table_exists(&conn, "passages_en").unwrap());
        assert!(table_exists(&conn, "passages_en_content").unwrap());
    }

    #[tokio::test]
    async fn legacy_migration_rolls_back_when_insert_fails() {
        // Force a failure inside the migration transaction: pre-seed
        // `passages_en_content` (just the content table — `passages_en`
        // virtual table is NOT created, so `migrate_or_create` still
        // takes the migrate branch) with a row whose rowid collides
        // with the legacy data we're about to copy. The INSERT-SELECT
        // in the migration body then fails with a PRIMARY KEY conflict,
        // the `?` propagates, and dropping the `Transaction` rolls back
        // every DDL/DML statement made inside it.
        //
        // Post-condition: legacy tables intact, `passages_en` virtual
        // table NOT present (it was created inside the failed tx).
        let f = tmp_db();
        {
            let conn = Connection::open(f.path()).unwrap();
            conn.execute_batch(
                "CREATE VIRTUAL TABLE passages USING fts5(
                    id, source, text,
                    content='passages_content', content_rowid='rowid'
                );
                CREATE TABLE passages_content(
                    rowid INTEGER PRIMARY KEY,
                    id TEXT NOT NULL,
                    source TEXT NOT NULL,
                    text TEXT NOT NULL
                );
                CREATE TABLE passages_en_content(
                    rowid INTEGER PRIMARY KEY,
                    id TEXT NOT NULL,
                    source TEXT NOT NULL,
                    text TEXT NOT NULL
                );
                INSERT INTO passages_content(rowid, id, source, text)
                    VALUES (1, 'legacy-1', 'legacy:source', 'photosynthesis is amazing');
                INSERT INTO passages_en_content(rowid, id, source, text)
                    VALUES (1, 'pre-existing', 's', 't');",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO passages(rowid, id, source, text)
                 VALUES (1, 'legacy-1', 'legacy:source', 'photosynthesis is amazing')",
                [],
            )
            .unwrap();
        }

        let result = SqliteKnowledgeBase::open_for_locale(f.path(), Locale::English);
        assert!(
            result.is_err(),
            "migration should fail loudly on rowid conflict"
        );

        // Verify the rollback: legacy tables unchanged, `passages_en`
        // virtual table not created. The pre-seeded `passages_en_content`
        // existed before the tx and survives.
        let conn = Connection::open(f.path()).unwrap();
        assert!(
            table_exists(&conn, "passages").unwrap(),
            "legacy `passages` should still exist after rollback"
        );
        assert!(
            table_exists(&conn, "passages_content").unwrap(),
            "legacy `passages_content` should still exist after rollback"
        );
        assert!(
            !table_exists(&conn, "passages_en").unwrap(),
            "`passages_en` was created inside the failed tx and should be rolled back"
        );
        // The pre-existing pre-seed survives.
        let pre_seed_id: String = conn
            .query_row(
                "SELECT id FROM passages_en_content WHERE rowid=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre_seed_id, "pre-existing");
    }

    #[tokio::test]
    async fn legacy_and_new_both_present_skips_migration_and_warns() {
        // Ambiguous state (manual user intervention, hand-fixed prior
        // crash, etc.): both `passages` and `passages_en` exist. We
        // don't auto-merge, we don't drop legacy data — we log a
        // `tracing::warn!` and leave the locale tables intact so the
        // app keeps working. Legacy rows are orphaned but recoverable
        // by manual inspection.
        let f = tmp_db();
        {
            let conn = Connection::open(f.path()).unwrap();
            conn.execute_batch(
                "CREATE VIRTUAL TABLE passages USING fts5(
                    id, source, text,
                    content='passages_content', content_rowid='rowid'
                );
                CREATE TABLE passages_content(
                    rowid INTEGER PRIMARY KEY,
                    id TEXT NOT NULL,
                    source TEXT NOT NULL,
                    text TEXT NOT NULL
                );
                INSERT INTO passages_content(rowid, id, source, text)
                    VALUES (1, 'legacy-orphan', 's', 't');
                CREATE VIRTUAL TABLE passages_en USING fts5(
                    id, source, text,
                    content='passages_en_content', content_rowid='rowid'
                );
                CREATE TABLE passages_en_content(
                    rowid INTEGER PRIMARY KEY,
                    id TEXT NOT NULL,
                    source TEXT NOT NULL,
                    text TEXT NOT NULL
                );",
            )
            .unwrap();
        }

        let kb = SqliteKnowledgeBase::open_for_locale(f.path(), Locale::English).unwrap();

        // Both tables still present after open — no destructive action.
        let conn = kb.conn.lock().unwrap();
        assert!(table_exists(&conn, "passages").unwrap());
        assert!(table_exists(&conn, "passages_en").unwrap());
    }
}
