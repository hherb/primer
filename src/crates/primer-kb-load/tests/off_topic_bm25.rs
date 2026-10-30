//! Off-topic small talk on the BM25-only path (issue #386): the path
//! `--embedder-backend none` (the Android guidance) and the dialogue
//! manager's hybrid-error fallback take, where no cosine gate exists.
//!
//! The KB drops per-locale stopwords before the FTS5 OR-join, so filler
//! words ("can **we** stop **now**") no longer match passages. Every
//! utterance in `common::off_topic` retrieves nothing at the production
//! defaults, except the documented `KNOWN_LEAKING_OFF_TOPIC_BM25` entries,
//! which must still leak (so the list can't go stale). BM25-only recall
//! is pinned by `retrieval_quality{,_de}.rs`.

mod common;

use common::off_topic::{KNOWN_LEAKING_OFF_TOPIC_BM25, OFF_TOPIC_DE, OFF_TOPIC_EN};
use primer_core::consts::retrieval as r;
use primer_core::i18n::Locale;
use primer_core::knowledge::{KnowledgeBase, RetrievalParams};
use primer_kb_load::auto_seed_if_empty;
use primer_knowledge::SqliteKnowledgeBase;

#[tokio::test]
async fn off_topic_queries_retrieve_nothing_on_the_bm25_only_path() {
    let params = RetrievalParams {
        top_k: r::KB_FINAL_TOP_K,
        min_score: r::KB_BM25_ONLY_MIN_SCORE,
        source_filter: vec![],
    };
    let mut failures = Vec::new();
    for (locale, queries) in [
        (Locale::English, OFF_TOPIC_EN),
        (Locale::German, OFF_TOPIC_DE),
    ] {
        let db = tempfile::NamedTempFile::new().unwrap();
        let kb = SqliteKnowledgeBase::open_for_locale(db.path(), locale).unwrap();
        auto_seed_if_empty(&kb, locale)
            .await
            .unwrap()
            .expect("seed dir must contain the locale's *.jsonl");
        for q in queries {
            let hits = kb.retrieve(q, &params).await.unwrap();
            let leaks = KNOWN_LEAKING_OFF_TOPIC_BM25.contains(q);
            if leaks == hits.is_empty() {
                failures.push(format!(
                    "{q:?}: {} hits, known-leaking = {leaks}",
                    hits.len()
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "BM25-only off-topic drift (update KNOWN_LEAKING_OFF_TOPIC_BM25):\n  {}",
        failures.join("\n  ")
    );
}
