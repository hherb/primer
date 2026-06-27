use super::*;
use primer_core::i18n::Locale;
use primer_core::knowledge::RetrievalParams;
use std::io::Write;

fn write_jsonl(lines: &[&str]) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
    f.flush().unwrap();
    f
}

#[tokio::test]
async fn load_two_passages_round_trip() {
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    let jsonl = write_jsonl(&[
        r#"{"id":"p1","source":"seed:en:p1","license":"CC0-1.0","attribution":"x","text":"the sky is blue because of rayleigh scattering"}"#,
        r#"{"id":"p2","source":"seed:en:p2","license":"CC0-1.0","attribution":"x","text":"plants make food via photosynthesis"}"#,
    ]);

    let stats = load_jsonl(&kb, jsonl.path()).await.unwrap();
    assert_eq!(stats.inserted, 2);
    assert_eq!(stats.skipped_existing, 0);
    assert_eq!(stats.sources_seen, 2);

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
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].id, "p2");

    let sources = kb.list_sources().await.unwrap();
    assert_eq!(sources.len(), 2);
}

#[tokio::test]
async fn parent_source_creates_umbrella_and_links_child() {
    // A passage carrying a nested `parent_source` must (a) link its own
    // source row to the umbrella via parent_source_id, and (b) cause the
    // umbrella row itself to be registered. Issue #40.
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    let jsonl = write_jsonl(&[
        r#"{"id":"wiki-simple:en:mercury","source":"wiki-simple:en:mercury","license":"CC-BY-SA-3.0","attribution":"'Mercury' from Simple English Wikipedia","source_url":"https://simple.wikipedia.org/wiki/Mercury","parent_source":{"id":"wiki-simple:en","license":"CC-BY-SA-3.0","attribution":"Corpus from Simple English Wikipedia","source_url":"https://simple.wikipedia.org/"},"text":"mercury is the smallest planet"}"#,
    ]);
    let stats = load_jsonl(&kb, jsonl.path()).await.unwrap();
    assert_eq!(stats.inserted, 1);
    // One child source + one umbrella source.
    assert_eq!(stats.sources_seen, 2);

    let sources = kb.list_sources().await.unwrap();
    let child = sources
        .iter()
        .find(|s| s.id == "wiki-simple:en:mercury")
        .expect("child source registered");
    assert_eq!(child.parent_source_id.as_deref(), Some("wiki-simple:en"));

    let umbrella = sources
        .iter()
        .find(|s| s.id == "wiki-simple:en")
        .expect("umbrella source registered");
    assert_eq!(umbrella.parent_source_id, None);
    assert_eq!(umbrella.attribution, "Corpus from Simple English Wikipedia");
    assert_eq!(
        umbrella.source_url.as_deref(),
        Some("https://simple.wikipedia.org/")
    );
}

#[tokio::test]
async fn no_parent_source_stays_flat() {
    // A passage without `parent_source` (the hand-drafted seed shape)
    // registers a source row with a NULL parent_source_id.
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    let jsonl = write_jsonl(&[
        r#"{"id":"seed:en:p1","source":"seed:en:p1","license":"CC0-1.0","attribution":"The Primer seed corpus","text":"the sky is blue"}"#,
    ]);
    load_jsonl(&kb, jsonl.path()).await.unwrap();
    let sources = kb.list_sources().await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].parent_source_id, None);
}

#[tokio::test]
async fn two_children_share_one_umbrella() {
    // Two passages pointing at the same parent_source must yield exactly
    // one umbrella row (de-duped), plus the two child rows.
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    let jsonl = write_jsonl(&[
        r#"{"id":"wiki-simple:en:mercury","source":"wiki-simple:en:mercury","license":"CC-BY-SA-3.0","attribution":"'Mercury' …","parent_source":{"id":"wiki-simple:en","license":"CC-BY-SA-3.0","attribution":"Corpus from Simple English Wikipedia","source_url":"https://simple.wikipedia.org/"},"text":"mercury is a planet"}"#,
        r#"{"id":"wiki-simple:en:atom","source":"wiki-simple:en:atom","license":"CC-BY-SA-3.0","attribution":"'Atom' …","parent_source":{"id":"wiki-simple:en","license":"CC-BY-SA-3.0","attribution":"Corpus from Simple English Wikipedia","source_url":"https://simple.wikipedia.org/"},"text":"an atom is tiny"}"#,
    ]);
    let stats = load_jsonl(&kb, jsonl.path()).await.unwrap();
    assert_eq!(stats.inserted, 2);
    // 2 children + 1 shared umbrella.
    assert_eq!(stats.sources_seen, 3);

    let sources = kb.list_sources().await.unwrap();
    let umbrellas: Vec<_> = sources
        .iter()
        .filter(|s| s.id == "wiki-simple:en")
        .collect();
    assert_eq!(umbrellas.len(), 1, "umbrella must be de-duped to one row");
}

#[tokio::test]
async fn rerun_is_idempotent() {
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    let jsonl = write_jsonl(&[
        r#"{"id":"p1","source":"seed","license":"CC0-1.0","attribution":"x","text":"hello world"}"#,
    ]);
    let s1 = load_jsonl(&kb, jsonl.path()).await.unwrap();
    let s2 = load_jsonl(&kb, jsonl.path()).await.unwrap();
    assert_eq!(s1.inserted, 1);
    assert_eq!(s2.inserted, 0);
    assert_eq!(s2.skipped_existing, 1);
    assert_eq!(kb.passage_count().unwrap(), 1);
}

#[tokio::test]
async fn blank_lines_and_comments_are_skipped() {
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    let jsonl = write_jsonl(&[
        "# this is a comment, skip me",
        "",
        r#"{"id":"p1","source":"seed","license":"CC0-1.0","attribution":"x","text":"only row"}"#,
        "   ",
    ]);
    let stats = load_jsonl(&kb, jsonl.path()).await.unwrap();
    assert_eq!(stats.inserted, 1);
}

#[tokio::test]
async fn auto_seed_via_explicit_jsonl_path_round_trip() {
    // Direct exercise of the `load_jsonl` half of `auto_seed_if_empty`
    // without touching process env. The discovery path is covered by
    // a dedicated unit test below; this asserts the "load + dedup"
    // contract that production CLI flow depends on.
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    assert_eq!(kb.passage_count().unwrap(), 0);

    let jsonl = write_jsonl(&[
        r#"{"id":"p1","source":"seed","license":"CC0-1.0","attribution":"x","text":"auto-seeded row"}"#,
    ]);
    let stats = load_jsonl(&kb, jsonl.path()).await.unwrap();
    assert_eq!(stats.inserted, 1);
    assert_eq!(kb.passage_count().unwrap(), 1);

    // The auto-seed contract: when the KB is non-empty, return None
    // with no I/O attempt — true regardless of discovery state.
    let result = auto_seed_if_empty(&kb, Locale::English).await.unwrap();
    assert!(
        result.is_none(),
        "auto-seed must skip when KB already has passages"
    );
}

#[tokio::test]
async fn reembed_backfills_passages_missing_embeddings() {
    use primer_embedding::StubEmbedder;
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    // Insert two passages WITHOUT embeddings via the legacy path.
    kb.insert_passage("p1", "src", "first text").unwrap();
    kb.insert_passage("p2", "src", "second text").unwrap();
    assert_eq!(kb.embedding_count().unwrap(), 0);

    let stub = StubEmbedder::with_dim(16);
    let n = reembed_kb(&kb, &stub, false, 8).await.unwrap();
    assert_eq!(n, 2);
    assert_eq!(kb.embedding_count().unwrap(), 2);

    // Idempotent: a second pass touches nothing.
    let n2 = reembed_kb(&kb, &stub, false, 8).await.unwrap();
    assert_eq!(n2, 0);
}

/// Serialise the three tests that mutate `PRIMER_SEED_DIR`. Cargo's
/// default test harness runs `#[test]` functions in parallel; without
/// this guard, two tests racing on the same env var would produce
/// flaky failures. `#[tokio::test]` defaults to a single-threaded
/// runtime, so holding this guard across `.await` is safe — the
/// runtime cannot suspend onto a different thread mid-test.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn discover_seed_jsonl_finds_file_under_env_dir() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let seed_dir = tempfile::tempdir().unwrap();
    let path = seed_dir.path().join("seed_passages.en.jsonl");
    std::fs::write(&path, "{}").unwrap();

    unsafe {
        std::env::set_var("PRIMER_SEED_DIR", seed_dir.path());
    }
    let found = discover_seed_jsonl(Locale::English);
    unsafe {
        std::env::remove_var("PRIMER_SEED_DIR");
    }
    assert_eq!(found.as_deref(), Some(path.as_path()));
}

// The std Mutex is held across .await, which clippy normally flags.
// It is safe here: #[tokio::test] defaults to current_thread, so the
// runtime cannot suspend onto another thread mid-test, and the lock
// serialises env-var-touching tests against each other.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn auto_seed_loads_all_matching_jsonl_files_in_dir() {
    // Two seed files in the same dir → both load.
    let seed_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        seed_dir.path().join("seed_passages.en.jsonl"),
        r#"{"id":"hand-1","source":"seed:en:hand-1","license":"CC0-1.0","attribution":"hand","text":"hand-drafted passage one"}"#,
    )
    .unwrap();
    std::fs::write(
        seed_dir.path().join("wiki_passages.en.jsonl"),
        r#"{"id":"wiki-1","source":"wiki-simple:en:wiki-1","license":"CC-BY-SA-3.0","attribution":"wiki","text":"wikipedia passage one"}"#,
    )
    .unwrap();
    // Distractor: a different-locale file must NOT be loaded.
    std::fs::write(
        seed_dir.path().join("wiki_passages.de.jsonl"),
        r#"{"id":"de-1","source":"wiki-simple:de:de-1","license":"CC-BY-SA-3.0","attribution":"de","text":"deutsche passage"}"#,
    )
    .unwrap();
    // Distractor: a non-jsonl file must NOT be loaded.
    std::fs::write(seed_dir.path().join("README.md"), "not jsonl").unwrap();

    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();

    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    unsafe {
        std::env::set_var("PRIMER_SEED_DIR", seed_dir.path());
    }
    let result = auto_seed_if_empty(&kb, Locale::English).await.unwrap();
    unsafe {
        std::env::remove_var("PRIMER_SEED_DIR");
    }

    let stats = result.expect("auto-seed should have loaded files");
    assert_eq!(stats.inserted, 2, "expected both en files to load");
    assert_eq!(kb.passage_count().unwrap(), 2);
}

#[test]
fn discover_seed_files_returns_only_matching_locale() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let seed_dir = tempfile::tempdir().unwrap();
    std::fs::write(seed_dir.path().join("seed_passages.en.jsonl"), "{}").unwrap();
    std::fs::write(seed_dir.path().join("wiki_passages.en.jsonl"), "{}").unwrap();
    std::fs::write(seed_dir.path().join("wiki_passages.de.jsonl"), "{}").unwrap();
    std::fs::write(seed_dir.path().join("README.md"), "x").unwrap();

    unsafe {
        std::env::set_var("PRIMER_SEED_DIR", seed_dir.path());
    }
    let mut found = discover_seed_files(Locale::English);
    unsafe {
        std::env::remove_var("PRIMER_SEED_DIR");
    }
    found.sort();
    assert_eq!(found.len(), 2);
    assert!(
        found[0].file_name().unwrap() == "seed_passages.en.jsonl",
        "expected first match to be seed_passages.en.jsonl, got {:?}",
        found[0]
    );
    assert!(
        found[1].file_name().unwrap() == "wiki_passages.en.jsonl",
        "expected second match to be wiki_passages.en.jsonl, got {:?}",
        found[1]
    );
}

#[tokio::test]
async fn malformed_json_reports_line_number() {
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    let jsonl = write_jsonl(&[
        r#"{"id":"p1","source":"seed","license":"CC0-1.0","attribution":"x","text":"ok"}"#,
        r#"{not valid json"#,
    ]);
    let err = load_jsonl(&kb, jsonl.path()).await.unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("line 2"), "expected line number, got: {msg}");
}
