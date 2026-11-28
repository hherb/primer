use std::path::{Path, PathBuf};

use primer_core::i18n::Locale;
use primer_embedding::StubEmbedder;

use super::*;

const P1: &str = r#"{"id":"seed:en:p1","source":"seed:en:p1","license":"CC0-1.0","attribution":"x","text":"the sky is blue because of rayleigh scattering"}"#;
const P2: &str = r#"{"id":"seed:en:p2","source":"seed:en:p2","license":"CC0-1.0","attribution":"x","text":"plants make food via photosynthesis"}"#;

fn write_corpus(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, lines.join("\n")).unwrap();
    p
}

async fn open(
    db: &KnowledgeDb,
    seed: &[PathBuf],
    embedder: Option<&dyn Embedder>,
) -> (SqliteKnowledgeBase, PrepareStats, Vec<(usize, usize)>) {
    let mut progress = Vec::new();
    let (kb, stats) = open_prepared_kb(db, Locale::English, seed, embedder, &mut |d, t| {
        progress.push((d, t))
    })
    .await
    .unwrap();
    (kb, stats, progress)
}

#[tokio::test]
async fn cache_creates_its_dir_tops_up_and_embeds() {
    let tmp = tempfile::tempdir().unwrap();
    let db = KnowledgeDb::Cache(tmp.path().join("a/b/kb/stub.db"));
    let embedder = StubEmbedder::new();
    let one = vec![write_corpus(tmp.path(), "one.en.jsonl", &[P1])];
    let (kb, stats, progress) = open(&db, &one, Some(&embedder)).await;
    assert_eq!(kb.passage_count().unwrap(), 1);
    assert_eq!(stats.embedded, 1);
    assert!(stats.embed_error.is_none());
    assert_eq!(progress.first(), Some(&(0, 1)));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(tmp.path().join("a/b/kb"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "the cache dir is owner-only");
    }
    drop(kb);

    // A grown corpus reaches the existing cache: top-up, not seed-when-empty.
    let two = vec![write_corpus(tmp.path(), "two.en.jsonl", &[P1, P2])];
    let (kb, stats, _) = open(&db, &two, Some(&embedder)).await;
    assert_eq!(kb.passage_count().unwrap(), 2);
    assert_eq!(stats.embedded, 1, "only the new passage needs a vector");
    assert!(kb.passages_missing_embedding().unwrap().is_empty());
}

#[tokio::test]
async fn explicit_is_seeded_once_and_never_embedded() {
    let tmp = tempfile::tempdir().unwrap();
    let db = KnowledgeDb::Explicit(tmp.path().join("mine.db"));
    let embedder = StubEmbedder::new();
    let one = vec![write_corpus(tmp.path(), "one.en.jsonl", &[P1])];
    let (kb, stats, progress) = open(&db, &one, Some(&embedder)).await;
    assert_eq!(kb.passage_count().unwrap(), 1);
    assert_eq!(stats.embedded, 0);
    assert!(progress.is_empty(), "no embed ran, so no progress");
    assert_eq!(kb.passages_missing_embedding().unwrap().len(), 1);
    drop(kb);

    // Non-empty: a grown corpus is not loaded into the user's KB.
    let two = vec![write_corpus(tmp.path(), "two.en.jsonl", &[P1, P2])];
    let (kb, stats, _) = open(&db, &two, Some(&embedder)).await;
    assert_eq!(kb.passage_count().unwrap(), 1);
    assert!(stats.seeded.is_none());
}

#[tokio::test]
async fn in_memory_seeds_and_embeds_when_an_embedder_is_given() {
    let tmp = tempfile::tempdir().unwrap();
    let seed = vec![write_corpus(tmp.path(), "s.en.jsonl", &[P1, P2])];
    let embedder = StubEmbedder::new();
    let (kb, stats, _) = open(&KnowledgeDb::InMemory, &seed, Some(&embedder)).await;
    assert_eq!(kb.passage_count().unwrap(), 2);
    assert_eq!(stats.embedded, 2);

    let (kb, stats, _) = open(&KnowledgeDb::InMemory, &seed, None).await;
    assert_eq!(kb.passage_count().unwrap(), 2);
    assert_eq!(stats.embedded, 0);
}

#[tokio::test]
async fn a_failed_open_names_the_path() {
    let tmp = tempfile::tempdir().unwrap();
    // A directory is not a SQLite file.
    let db = KnowledgeDb::Explicit(tmp.path().to_path_buf());
    let err = open_prepared_kb(&db, Locale::English, &[], None, &mut |_, _| {})
        .await
        .err()
        .expect("opening a directory must fail");
    let err = err.to_string();
    assert!(err.contains(&tmp.path().display().to_string()), "{err}");
}

#[tokio::test]
async fn a_failed_seed_names_the_path() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("mine.db");
    let bad = vec![write_corpus(tmp.path(), "bad.en.jsonl", &[P1, "{not json"])];
    let err = open_prepared_kb(
        &KnowledgeDb::Explicit(path.clone()),
        Locale::English,
        &bad,
        None,
        &mut |_, _| {},
    )
    .await
    .err()
    .expect("a malformed seed line must fail")
    .to_string();
    assert!(err.contains("seeding"), "{err}");
    assert!(err.contains(&path.display().to_string()), "{err}");
}

/// Issue #401 end to end: two corpus folders on one home and one model
/// resolve to two caches, so opening B never prunes A's passages and
/// reopening A needs no re-embed.
#[tokio::test]
async fn two_seed_folders_keep_separate_caches() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let embedder = StubEmbedder::new();
    let corpus = |dir: &str, line: &str| {
        let d = tmp.path().join(dir);
        std::fs::create_dir_all(&d).unwrap();
        vec![write_corpus(&d, "seed.en.jsonl", &[line])]
    };
    let (a, b) = (corpus("a", P1), corpus("b", P2));
    let db_for = |files: &[PathBuf]| {
        crate::resolve_knowledge_db(
            None,
            Some(&home),
            Some(embedder.model_id()),
            crate::seed_corpus_dir(files).as_deref(),
            false,
        )
    };
    assert_ne!(db_for(&a), db_for(&b));

    drop(open(&db_for(&a), &a, Some(&embedder)).await);
    drop(open(&db_for(&b), &b, Some(&embedder)).await);
    let (kb, stats, progress) = open(&db_for(&a), &a, Some(&embedder)).await;
    assert_eq!(kb.passage_count().unwrap(), 1, "B's top-up never touched A");
    assert_eq!(stats.embedded, 0);
    assert!(progress.is_empty(), "A's vectors survived: no re-embed");
}

const P3: &str = r#"{"id":"seed:en:p3","source":"seed:en:p3","license":"CC0-1.0","attribution":"x","text":"the moon pulls on the sea and makes the tides"}"#;

/// Issue #406: a new corpus folder's cache starts as a copy of the newest
/// cache for the same model, so its first open embeds only what differs.
#[tokio::test]
async fn a_new_cache_starts_from_the_newest_same_model_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let kb_dir = tmp.path().join("kb");
    let embedder = StubEmbedder::new();
    let a = KnowledgeDb::Cache(kb_dir.join("stub-fxhash-v1-00000000000000aa.db"));
    let b = KnowledgeDb::Cache(kb_dir.join("stub-fxhash-v1-00000000000000bb.db"));
    // Another model's cache is newer, but must never be the source.
    let other = KnowledgeDb::Cache(kb_dir.join("other-model-00000000000000cc.db"));

    let corpus_a = vec![write_corpus(tmp.path(), "a.en.jsonl", &[P1, P2])];
    drop(open(&a, &corpus_a, Some(&embedder)).await);
    let corpus_other = vec![write_corpus(tmp.path(), "o.en.jsonl", &[P3])];
    drop(open(&other, &corpus_other, Some(&embedder)).await);

    let corpus_b = vec![write_corpus(tmp.path(), "b.en.jsonl", &[P1, P3])];
    let (kb, stats, _) = open(&b, &corpus_b, Some(&embedder)).await;
    assert_eq!(stats.embedded, 1, "only P3 is new; P1's vector was copied");
    assert_eq!(kb.passage_count().unwrap(), 2, "P2 pruned from the copy");
    drop(kb);

    let (kb, stats, _) = open(&a, &corpus_a, Some(&embedder)).await;
    assert_eq!(
        kb.passage_count().unwrap(),
        2,
        "the source cache is untouched"
    );
    assert_eq!(stats.embedded, 0);
}

/// Issue #406: once a hashed cache is ready, the pre-#401 `<slug>.db` is
/// deleted, after it has seeded the new cache (no upgrade re-embed).
#[tokio::test]
async fn a_ready_hashed_cache_replaces_the_legacy_file() {
    let tmp = tempfile::tempdir().unwrap();
    let kb_dir = tmp.path().join("kb");
    let embedder = StubEmbedder::new();
    let corpus = vec![write_corpus(tmp.path(), "s.en.jsonl", &[P1, P2])];
    let legacy = kb_dir.join("stub-fxhash-v1.db");
    drop(
        open(
            &KnowledgeDb::Cache(legacy.clone()),
            &corpus,
            Some(&embedder),
        )
        .await,
    );
    assert!(legacy.exists());

    let hashed = KnowledgeDb::Cache(kb_dir.join("stub-fxhash-v1-0123456789abcdef.db"));
    let (kb, stats, progress) = open(&hashed, &corpus, Some(&embedder)).await;
    assert_eq!(kb.passage_count().unwrap(), 2);
    assert_eq!(stats.embedded, 0, "the legacy vectors were copied");
    assert!(progress.is_empty());
    assert!(!legacy.exists(), "the legacy cache is removed");
}

/// The legacy name is still the cache when no seed file is found; it must
/// never delete itself.
#[tokio::test]
async fn an_unhashed_cache_keeps_its_own_file() {
    let tmp = tempfile::tempdir().unwrap();
    let legacy = tmp.path().join("kb/stub-fxhash-v1.db");
    let embedder = StubEmbedder::new();
    drop(open(&KnowledgeDb::Cache(legacy.clone()), &[], Some(&embedder)).await);
    assert!(legacy.exists());
}
