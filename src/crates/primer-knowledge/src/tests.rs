use super::*;
// These helpers moved into responsibility submodules during the split;
// `use super::*` (crate root) no longer carries them, so import explicitly.
// `LocaleSql`, `USER_VERSION`, `Locale`, `Connection`, and the
// `primer_core::knowledge::*` types still arrive via `super::*`.
use crate::schema::table_exists;
use crate::vector::{blob_to_vec, vec_to_blob};
// `Embedder` trait methods (`dim`, `model_id`, `embed`) are called on the
// `StubEmbedder` in the embedding tests; the trait must be in scope.
use primer_core::embedder::Embedder;
use tempfile::NamedTempFile;

/// Allocate a fresh sqlite path inside a `tempfile::NamedTempFile`. The
/// returned guard owns the file and removes it on drop — including the
/// drop that runs when a test panics — so test runs cannot leak files
/// or collide on path even under aggressive `--test-threads`. SQLite is
/// happy to open the 0-byte file as a fresh DB.
fn tmp_db() -> NamedTempFile {
    NamedTempFile::new().expect("create temp DB file")
}

/// True if `table` has a column named `column` (via `pragma_table_info`).
fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("prepare pragma");
    let names: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .expect("query pragma")
        .map(|r| r.expect("read column name"))
        .collect();
    names.iter().any(|n| n == column)
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
        parent_source_id: None,
    };
    let s2 = SourceMeta {
        id: "wikipedia:en:Rayleigh_scattering".to_string(),
        license: "CC-BY-SA-4.0".to_string(),
        attribution: "Wikipedia contributors".to_string(),
        source_url: Some("https://en.wikipedia.org/wiki/Rayleigh_scattering".to_string()),
        retrieved_at: 1_700_001_000,
        parent_source_id: None,
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
async fn fresh_db_sources_table_has_parent_source_id_column() {
    // Schema v4 (issue #40): `sources` carries a nullable self-FK
    // `parent_source_id` so per-article rows can point at one shared
    // umbrella source row.
    let db = tmp_db();
    let _kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    let conn = Connection::open(db.path()).unwrap();
    assert!(
        column_exists(&conn, "sources", "parent_source_id"),
        "fresh sources table must have parent_source_id (schema v4)"
    );
}

#[tokio::test]
async fn v3_sources_table_migrates_to_v4_non_destructively() {
    // A v3 DB has a `sources` table WITHOUT `parent_source_id`. Opening
    // it must add the column without dropping existing rows.
    let db = tmp_db();
    let path = db.path();
    {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sources(
                    id            TEXT PRIMARY KEY,
                    license       TEXT NOT NULL,
                    attribution   TEXT NOT NULL,
                    source_url    TEXT,
                    retrieved_at  INTEGER NOT NULL
                );
                INSERT INTO sources(id, license, attribution, source_url, retrieved_at)
                    VALUES ('seed:en:x', 'CC0-1.0', 'old row', NULL, 1700000000);",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 3).unwrap();
    }

    let kb = SqliteKnowledgeBase::open_for_locale(path, Locale::English).unwrap();
    let conn = Connection::open(path).unwrap();
    assert!(
        column_exists(&conn, "sources", "parent_source_id"),
        "v3 → v4 migration must add parent_source_id"
    );
    let v: i32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, USER_VERSION);

    // Pre-existing row survives, with NULL parent.
    let listed = kb.list_sources().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "seed:en:x");
    assert_eq!(listed[0].parent_source_id, None);
}

#[tokio::test]
async fn upsert_and_list_sources_round_trip_parent_source_id() {
    let db = tmp_db();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    let umbrella = SourceMeta {
        id: "wiki-simple:en".to_string(),
        license: "CC-BY-SA-3.0".to_string(),
        attribution: "Corpus from Simple English Wikipedia".to_string(),
        source_url: Some("https://simple.wikipedia.org/".to_string()),
        retrieved_at: 1_700_000_000,
        parent_source_id: None,
    };
    let child = SourceMeta {
        id: "wiki-simple:en:mercury".to_string(),
        license: "CC-BY-SA-3.0".to_string(),
        attribution: "'Mercury' from Simple English Wikipedia".to_string(),
        source_url: Some("https://simple.wikipedia.org/wiki/Mercury".to_string()),
        retrieved_at: 1_700_000_500,
        parent_source_id: Some("wiki-simple:en".to_string()),
    };
    kb.upsert_source(&umbrella).await.unwrap();
    kb.upsert_source(&child).await.unwrap();

    let listed = kb.list_sources().await.unwrap();
    assert_eq!(listed.len(), 2);
    // ORDER BY id: "wiki-simple:en" < "wiki-simple:en:mercury"
    assert_eq!(listed[0], umbrella);
    assert_eq!(listed[1], child);
    assert_eq!(
        listed[1].parent_source_id.as_deref(),
        Some("wiki-simple:en")
    );
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
async fn fresh_db_lands_at_v3_with_embedding_tables() {
    let db = tmp_db();
    let _kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    let conn = Connection::open(db.path()).unwrap();
    let v: i32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, USER_VERSION);
    assert!(table_exists(&conn, "embedding_models").unwrap());
    assert!(table_exists(&conn, "embeddings_en").unwrap());
}

#[tokio::test]
async fn insert_passage_with_embedding_round_trip() {
    use primer_embedding::StubEmbedder;
    let db = tmp_db();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    let stub = StubEmbedder::with_dim(16);
    kb.insert_passage_with_embedding("p1", "src", "hello world", &stub)
        .await
        .unwrap();
    assert_eq!(kb.passage_count().unwrap(), 1);
    assert_eq!(kb.embedding_count().unwrap(), 1);
}

#[tokio::test]
async fn retrieve_hybrid_returns_relevant_passage() {
    use primer_embedding::StubEmbedder;
    let db = tmp_db();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    let stub = StubEmbedder::with_dim(64);
    kb.insert_passage_with_embedding(
        "p1",
        "src",
        "the sky is blue because of rayleigh scattering",
        &stub,
    )
    .await
    .unwrap();
    kb.insert_passage_with_embedding(
        "p2",
        "src",
        "plants make food via photosynthesis using chlorophyll",
        &stub,
    )
    .await
    .unwrap();

    let hits = kb
        .retrieve_hybrid(
            "rayleigh",
            &stub,
            &HybridParams {
                bm25_top_k: 5,
                vector_top_k: 5,
                final_top_k: 3,
                rrf_k: 60.0,
                min_score: f64::NEG_INFINITY,
                source_filter: vec![],
            },
        )
        .await
        .unwrap();
    // BM25 leg matches "rayleigh" → p1 should be top.
    assert!(!hits.is_empty());
    assert_eq!(hits[0].id, "p1");
}

#[tokio::test]
async fn retrieve_hybrid_falls_back_when_only_one_leg_hits() {
    // The vector leg always returns up-to-K candidates regardless
    // of relevance; the BM25 leg may return zero. Hybrid should
    // still surface something useful.
    use primer_embedding::StubEmbedder;
    let db = tmp_db();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    let stub = StubEmbedder::with_dim(32);
    kb.insert_passage_with_embedding("p1", "src", "alpha beta gamma", &stub)
        .await
        .unwrap();
    kb.insert_passage_with_embedding("p2", "src", "delta epsilon zeta", &stub)
        .await
        .unwrap();

    // Query has no BM25 overlap with either passage.
    let hits = kb
        .retrieve_hybrid(
            "completely unrelated query",
            &stub,
            &HybridParams::default(),
        )
        .await
        .unwrap();
    // Vector leg always returns its top-K, so we should see hits.
    assert!(!hits.is_empty(), "vector leg should surface candidates");
}

#[tokio::test]
async fn upsert_embedding_backfills_existing_passage() {
    use primer_embedding::StubEmbedder;
    let db = tmp_db();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    // Insert without embedding (legacy / pre-embedding ingest).
    kb.insert_passage("p1", "src", "hello world").unwrap();
    assert_eq!(kb.embedding_count().unwrap(), 0);
    assert_eq!(kb.passages_missing_embedding().unwrap().len(), 1);

    let stub = StubEmbedder::with_dim(16);
    let v = stub.embed(&["hello world"]).await.unwrap().remove(0);
    kb.upsert_embedding("p1", stub.model_id(), stub.dim(), &v)
        .unwrap();
    assert_eq!(kb.embedding_count().unwrap(), 1);
    assert_eq!(kb.passages_missing_embedding().unwrap().len(), 0);

    // Idempotent upsert.
    kb.upsert_embedding("p1", stub.model_id(), stub.dim(), &v)
        .unwrap();
    assert_eq!(kb.embedding_count().unwrap(), 1);
}

#[tokio::test]
async fn embedding_model_dim_mismatch_is_a_hard_error() {
    use primer_embedding::StubEmbedder;
    let db = tmp_db();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    let stub_a = StubEmbedder::with_dim(16);
    kb.insert_passage_with_embedding("p1", "src", "first", &stub_a)
        .await
        .unwrap();

    // Same model_id reported by stub but the dim recorded above is 16;
    // any subsequent call with a different dim under the same name
    // must error. (The default StubEmbedder reports dim 384 — its
    // model_id is the same constant — so calling it now should error.)
    let stub_b = StubEmbedder::with_dim(384);
    let result = kb
        .insert_passage_with_embedding("p2", "src", "second", &stub_b)
        .await;
    assert!(
        matches!(result, Err(PrimerError::Knowledge(_))),
        "dim mismatch must be a hard error, got {result:?}"
    );
}

#[tokio::test]
async fn retrieve_hybrid_dim_mismatch_under_registered_name_is_hard_error() {
    // Set up a DB with an embedding under model "stub-fxhash-v1" / dim 16.
    use primer_embedding::StubEmbedder;
    let db = tmp_db();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    let stub_a = StubEmbedder::with_dim(16);
    kb.insert_passage_with_embedding("p1", "src", "first", &stub_a)
        .await
        .unwrap();

    // Now ask retrieve_hybrid with the same name but a different dim.
    // Validation must fire before the embed call so this is a hard
    // error rather than a silent BM25 fallback.
    let stub_b = StubEmbedder::with_dim(384);
    let result = kb
        .retrieve_hybrid("first", &stub_b, &HybridParams::default())
        .await;
    assert!(
        matches!(result, Err(PrimerError::Knowledge(_))),
        "hybrid retrieve dim mismatch must hard-error, got {result:?}"
    );
}

#[tokio::test]
async fn vec_blob_round_trip() {
    let v = vec![0.0_f32, 1.5, -2.25, std::f32::consts::PI];
    let blob = vec_to_blob(&v);
    let back = blob_to_vec(&blob).unwrap();
    assert_eq!(v, back);
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

#[test]
fn locale_sql_interpolates_pack_into_every_statement() {
    // `LocaleSql::new` is the single source of truth for every
    // per-locale, per-row/per-query SQL string the hot paths cache.
    // Each statement must carry the locale-qualified table name so a
    // `de` knowledge base never touches `en` tables.
    let sql = LocaleSql::new("de");
    for stmt in [
        &sql.insert_content,
        &sql.insert_fts,
        &sql.insert_embedding,
        &sql.upsert_embedding,
        &sql.select_rowid_by_id,
        &sql.retrieve,
        &sql.knn_scan,
    ] {
        assert!(
            stmt.contains("_de"),
            "every per-locale statement must carry the pack id; missing in: {stmt}"
        );
    }
    // No statement should leak the bare (locale-less) table names that
    // the legacy single-table layout used.
    assert!(!sql.insert_content.contains("passages_content("));
    // A half-interpolated `FROM passages` (bare, locale-less table)
    // would be followed immediately by the newline; the real one is
    // `FROM passages_de`, so this fires only on the legacy bare name.
    assert!(!sql.retrieve.contains("FROM passages\n"));
}

#[test]
fn locale_sql_fts_insert_binds_rowid_as_param_four() {
    // The FTS insert writes `rowid` (the last-insert id from the
    // content table) into `?4`, with id/source/text in `?1..?3` —
    // the order callers bind in `insert_passage_inner`. Pinning this
    // guards against a silent column/parameter reshuffle when the
    // statement moves into `LocaleSql`.
    let sql = LocaleSql::new("en");
    assert!(
        sql.insert_fts.contains("(?4, ?1, ?2, ?3)"),
        "FTS insert must bind rowid as ?4: {}",
        sql.insert_fts
    );
    assert!(
        sql.insert_fts
            .contains("passages_en(rowid, id, source, text)")
    );
}
