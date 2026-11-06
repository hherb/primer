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
