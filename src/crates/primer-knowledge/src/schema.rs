//! Schema management: per-locale table creation, the cross-locale
//! `sources` + `embedding_models` tables, `user_version` bookkeeping, and
//! the idempotent open-time migrate-or-create entry point.

use primer_core::error::{PrimerError, Result};
use rusqlite::Connection;

/// Current schema version. Bumped whenever a new `apply_vN_migrations`
/// is added. v1 = legacy / per-locale tables only; v2 = `sources` table;
/// v3 = `embedding_models` lookup + per-locale `embeddings_<pack>` table
/// holding one float-vector per passage for hybrid retrieval;
/// v4 = `sources.parent_source_id` self-FK for umbrella attribution (#40).
pub const USER_VERSION: i32 = 4;

pub(crate) fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
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
pub(crate) fn migrate_or_create(conn: &Connection, pack: &str) -> Result<()> {
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
