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

/// Current schema version. Bumped whenever a new `apply_vN_migrations`
/// is added. v1 = legacy / per-locale tables only; v2 = `sources` table.
pub const USER_VERSION: i32 = 2;

/// SQLite FTS5-backed knowledge base, scoped to one `Locale`.
pub struct SqliteKnowledgeBase {
    conn: Mutex<Connection>,
    locale: Locale,
    /// `passages_<pack_id>` — the FTS5 virtual table name. Cached at
    /// `open_for_locale` so the per-call `format!` allocation in the
    /// insert/retrieve hot paths is gone.
    passages_table: String,
    /// `passages_<pack_id>_content` — the external-content storage
    /// table name. Same caching rationale as `passages_table`.
    content_table: String,
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
            passages_table: format!("passages_{pack}"),
            content_table: format!("passages_{pack}_content"),
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
        let content_table = &self.content_table;
        let passages_table = &self.passages_table;

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
    async fn upsert_source(&self, source: &SourceMeta) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sources(id, license, attribution, source_url, retrieved_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET
                    license       = excluded.license,
                    attribution   = excluded.attribution,
                    source_url    = excluded.source_url,
                    retrieved_at  = excluded.retrieved_at",
            rusqlite::params![
                source.id,
                source.license,
                source.attribution,
                source.source_url,
                source.retrieved_at,
            ],
        )
        .map_err(|e| PrimerError::Knowledge(format!("upsert source: {e}")))?;
        Ok(())
    }

    async fn list_sources(&self) -> Result<Vec<SourceMeta>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, license, attribution, source_url, retrieved_at
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
        let passages_table = &self.passages_table;

        // FTS5 match query with BM25 ranking. Table name is interpolated
        // (NOT user input — comes from `Locale::pack_id()`, a closed
        // enum projection), parameters are bound positionally — the
        // sanitized phrase and limit go through `?1` and `?2`.
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

/// Create the cross-locale `sources` table idempotently. Stores per-source
/// licence + attribution metadata so credits travel with the data; one
/// table for the whole DB (no locale partition) since `Passage.source`
/// values are already globally unique strings.
fn create_sources_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sources(
            id            TEXT PRIMARY KEY,
            license       TEXT NOT NULL,
            attribution   TEXT NOT NULL,
            source_url    TEXT,
            retrieved_at  INTEGER NOT NULL
        );",
    )
    .map_err(|e| PrimerError::Knowledge(format!("create sources table: {e}")))?;
    Ok(())
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

    tx.commit()
        .map_err(|e| PrimerError::Knowledge(format!("commit migration: {e}")))?;

    if existing_version != USER_VERSION {
        write_user_version(conn, USER_VERSION)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    /// Allocate a fresh sqlite path inside a `tempfile::NamedTempFile`. The
    /// returned guard owns the file and removes it on drop — including the
    /// drop that runs when a test panics — so test runs cannot leak files
    /// or collide on path even under aggressive `--test-threads`. SQLite is
    /// happy to open the 0-byte file as a fresh DB.
    fn tmp_db() -> NamedTempFile {
        NamedTempFile::new().expect("create temp DB file")
    }

    #[tokio::test]
    async fn open_for_english_creates_per_locale_tables() {
        let db = tmp_db();
        let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
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
    #[allow(deprecated)] // intentionally exercises the back-compat shim
    async fn open_back_compat_shim_uses_default_locale() {
        let db = tmp_db();
        let kb = SqliteKnowledgeBase::open(db.path()).unwrap();
        assert_eq!(kb.locale(), Locale::default());
    }

    #[tokio::test]
    async fn reopen_existing_db_is_a_no_op() {
        let db = tmp_db();
        {
            let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
            kb.insert_passage("p1", "src", "hello world").unwrap();
        }
        let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
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
        let db = tmp_db();
        let path = db.path();
        // Hand-roll a legacy schema (the pre-PR-2 layout) and pre-seed
        // a row, then open under the new API and verify migration.
        {
            let conn = Connection::open(path).unwrap();
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

        let kb = SqliteKnowledgeBase::open_for_locale(path, Locale::English).unwrap();
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
        {
            let conn = kb.conn.lock().unwrap();
            assert!(!table_exists(&conn, "passages").unwrap());
            assert!(!table_exists(&conn, "passages_content").unwrap());
            assert!(table_exists(&conn, "passages_en").unwrap());
            assert!(table_exists(&conn, "passages_en_content").unwrap());
        }
    }

    #[tokio::test]
    async fn cross_locale_isolation_passages_inserted_in_de_do_not_appear_in_en() {
        // Open the same DB file under two different locales and verify
        // their FTS5 indices stay isolated. This is the structural
        // guarantee that BM25 stays locale-pure: each locale only ever
        // queries its own `passages_<pack_id>` table.
        let db = tmp_db();
        let path = db.path();
        // Insert a German passage.
        {
            let kb = SqliteKnowledgeBase::open_for_locale(path, Locale::German).unwrap();
            kb.insert_passage(
                "de-1",
                "wikipedia:de:Photosynthese",
                "Photosynthese ist der Vorgang, bei dem Pflanzen Licht aufnehmen.",
            )
            .unwrap();
        }
        // Insert an English passage.
        {
            let kb = SqliteKnowledgeBase::open_for_locale(path, Locale::English).unwrap();
            kb.insert_passage(
                "en-1",
                "wikipedia:en:Photosynthesis",
                "Photosynthesis is the process plants use to capture light.",
            )
            .unwrap();
        }
        // Query under English — only English row should come back.
        {
            let kb = SqliteKnowledgeBase::open_for_locale(path, Locale::English).unwrap();
            let got = kb
                .retrieve(
                    "photosynthesis",
                    &RetrievalParams {
                        top_k: 10,
                        min_score: f64::NEG_INFINITY,
                        source_filter: vec![],
                    },
                )
                .await
                .unwrap();
            assert_eq!(got.len(), 1, "English KB must not return German rows");
            assert_eq!(got[0].id, "en-1");
        }
        // Query under German — only German row should come back. Use
        // the German term so the query is locale-appropriate; the
        // index isolation is what's being verified here, not query
        // tokenisation.
        {
            let kb = SqliteKnowledgeBase::open_for_locale(path, Locale::German).unwrap();
            let got = kb
                .retrieve(
                    "photosynthese",
                    &RetrievalParams {
                        top_k: 10,
                        min_score: f64::NEG_INFINITY,
                        source_filter: vec![],
                    },
                )
                .await
                .unwrap();
            assert_eq!(got.len(), 1, "German KB must not return English rows");
            assert_eq!(got[0].id, "de-1");
        }
    }

    #[tokio::test]
    async fn legacy_migration_skipped_when_target_already_exists() {
        // If both `passages` and `passages_<pack>` exist, the
        // `legacy_exists && !new_exists` guard in `migrate_or_create`
        // skips the migration branch entirely. Documented behaviour:
        // no-op, no data movement, both table sets stay intact. This
        // covers DBs that were previously (partially or fully) opened
        // under the new locale-aware API and still carry the legacy
        // tables from a pre-PR-2 build.
        let db = tmp_db();
        let path = db.path();
        {
            let conn = Connection::open(path).unwrap();
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

        let _kb = SqliteKnowledgeBase::open_for_locale(path, Locale::English).unwrap();
        let conn = Connection::open(path).unwrap();
        // Both legacy and locale-suffixed tables still present.
        assert!(table_exists(&conn, "passages").unwrap());
        assert!(table_exists(&conn, "passages_content").unwrap());
        assert!(table_exists(&conn, "passages_en").unwrap());
        assert!(table_exists(&conn, "passages_en_content").unwrap());
    }

    #[tokio::test]
    async fn legacy_migration_rolls_back_on_partial_failure() {
        // Real rollback test: hand-roll a malformed legacy schema
        // missing the `text` column. The migration's INSERT-SELECT
        // (which references `text`) fails at SQL-prepare time, and
        // since `migrate_or_create` runs the whole sequence inside a
        // single `unchecked_transaction()`, the locale-suffixed tables
        // it created earlier in the same transaction must also roll
        // back — leaving the legacy tables intact and `passages_en`
        // absent.
        let db = tmp_db();
        let path = db.path();
        {
            let conn = Connection::open(path).unwrap();
            conn.execute_batch(
                "CREATE VIRTUAL TABLE passages USING fts5(
                    id, source,
                    content='passages_content', content_rowid='rowid'
                );
                CREATE TABLE passages_content(
                    rowid INTEGER PRIMARY KEY,
                    id TEXT NOT NULL,
                    source TEXT NOT NULL
                    -- no `text` column: forces migration INSERT-SELECT
                    -- to fail with `no such column: text`.
                );
                INSERT INTO passages_content(rowid, id, source)
                    VALUES (1, 'a', 's');",
            )
            .unwrap();
        }

        let result = SqliteKnowledgeBase::open_for_locale(path, Locale::English);
        let err = match result {
            Ok(_) => panic!("malformed legacy schema must surface as an error"),
            Err(e) => e,
        };
        assert!(
            matches!(err, PrimerError::Knowledge(_)),
            "expected Knowledge error, got: {err:?}"
        );

        let conn = Connection::open(path).unwrap();
        // Legacy tables intact.
        assert!(table_exists(&conn, "passages").unwrap());
        assert!(table_exists(&conn, "passages_content").unwrap());
        // Locale-suffixed tables rolled back.
        assert!(
            !table_exists(&conn, "passages_en").unwrap(),
            "passages_en must roll back when migration fails"
        );
        assert!(
            !table_exists(&conn, "passages_en_content").unwrap(),
            "passages_en_content must roll back when migration fails"
        );
    }

    #[tokio::test]
    async fn fresh_db_lands_at_current_user_version_with_sources_table() {
        let db = tmp_db();
        let _kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
        let conn = Connection::open(db.path()).unwrap();
        let v: i32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, USER_VERSION);
        assert!(table_exists(&conn, "sources").unwrap());
    }

    #[tokio::test]
    async fn upsert_and_list_sources_round_trip() {
        let db = tmp_db();
        let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
        let s1 = SourceMeta {
            id: "seed:en:photosynthesis".to_string(),
            license: "CC0-1.0".to_string(),
            attribution: "The Primer seed corpus".to_string(),
            source_url: None,
            retrieved_at: 1_700_000_000,
        };
        let s2 = SourceMeta {
            id: "wikipedia:en:Rayleigh_scattering".to_string(),
            license: "CC-BY-SA-4.0".to_string(),
            attribution: "Wikipedia contributors".to_string(),
            source_url: Some("https://en.wikipedia.org/wiki/Rayleigh_scattering".to_string()),
            retrieved_at: 1_700_001_000,
        };
        kb.upsert_source(&s1).await.unwrap();
        kb.upsert_source(&s2).await.unwrap();

        let listed = kb.list_sources().await.unwrap();
        assert_eq!(listed.len(), 2);
        // ORDER BY id: seed: < wikipedia:
        assert_eq!(listed[0], s1);
        assert_eq!(listed[1], s2);

        // Upsert is idempotent and updates the row.
        let s1_updated = SourceMeta {
            attribution: "The Primer seed corpus, v2".to_string(),
            retrieved_at: 1_700_005_000,
            ..s1.clone()
        };
        kb.upsert_source(&s1_updated).await.unwrap();
        let listed = kb.list_sources().await.unwrap();
        assert_eq!(listed.len(), 2, "upsert must not create a duplicate");
        assert_eq!(listed[0], s1_updated);
    }

    #[tokio::test]
    async fn migration_from_v0_legacy_db_lands_at_v2() {
        // A pre-PR-2 DB had no `user_version` set (defaults to 0) and a
        // single legacy `passages` table. Opening under the new API
        // should run the legacy → per-locale migration AND create the
        // sources table AND bump user_version to USER_VERSION.
        let db = tmp_db();
        let path = db.path();
        {
            let conn = Connection::open(path).unwrap();
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
        }

        let _kb = SqliteKnowledgeBase::open_for_locale(path, Locale::English).unwrap();
        let conn = Connection::open(path).unwrap();
        let v: i32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, USER_VERSION);
        assert!(table_exists(&conn, "passages_en").unwrap());
        assert!(table_exists(&conn, "sources").unwrap());
        assert!(!table_exists(&conn, "passages").unwrap());
    }

    #[tokio::test]
    async fn newer_user_version_is_rejected() {
        let db = tmp_db();
        let path = db.path();
        {
            let conn = Connection::open(path).unwrap();
            conn.pragma_update(None, "user_version", USER_VERSION + 1)
                .unwrap();
        }
        let result = SqliteKnowledgeBase::open_for_locale(path, Locale::English);
        assert!(
            matches!(result, Err(PrimerError::Knowledge(_))),
            "newer user_version must be rejected"
        );
    }

    #[tokio::test]
    async fn passage_count_reflects_inserts() {
        let db = tmp_db();
        let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
        assert_eq!(kb.passage_count().unwrap(), 0);
        kb.insert_passage("p1", "src", "hello world").unwrap();
        kb.insert_passage("p2", "src", "another row").unwrap();
        assert_eq!(kb.passage_count().unwrap(), 2);
    }
}
