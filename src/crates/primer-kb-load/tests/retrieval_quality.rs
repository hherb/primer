//! Retrieval-quality integration test.
//!
//! Loads the in-repo English seed corpus into a temp DB and asserts
//! that canonical child queries surface passages mentioning the
//! expected key terms AND that strict-subset queries place their
//! canonical_id in the returned hits. The benchmark dataset lives in
//! `tests/common/mod.rs` so the diagnostic sweep
//! (`tests/retrieval_sweep.rs`) shares it.
//!
//! Pins the production retrieval defaults
//! (`KB_FINAL_TOP_K` / `KB_BM25_ONLY_MIN_SCORE`) so a regression in
//! tuned retrieval breaks CI. Queries the chosen defaults cannot
//! satisfy live in `common::KNOWN_FAILING_QUERIES` (tracked in
//! GitHub issue) and are skipped here.

mod common;

use common::{KNOWN_FAILING_QUERIES, QUERIES};
use primer_core::consts::retrieval as r;
use primer_core::i18n::Locale;
use primer_core::knowledge::{KnowledgeBase, RetrievalParams};
use primer_kb_load::auto_seed_if_empty;
use primer_knowledge::SqliteKnowledgeBase;

#[tokio::test]
async fn seed_corpus_satisfies_canonical_queries() {
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    let stats = auto_seed_if_empty(&kb, Locale::English)
        .await
        .unwrap()
        .expect("seed dir must contain at least one *.en.jsonl");
    assert!(
        stats.inserted >= 10,
        "seed corpus must contain at least 10 passages, got {}",
        stats.inserted
    );

    let mut failures = Vec::new();
    for q in QUERIES {
        if KNOWN_FAILING_QUERIES.contains(&q.query) {
            continue;
        }
        let params = RetrievalParams {
            top_k: r::KB_FINAL_TOP_K,
            min_score: r::KB_BM25_ONLY_MIN_SCORE,
            source_filter: vec![],
        };
        let hits = kb.retrieve(q.query, &params).await.unwrap();
        let combined = hits
            .iter()
            .map(|p| p.text.to_lowercase())
            .collect::<Vec<_>>()
            .join(" \n ");

        // Loose: required terms appear somewhere in top-k combined.
        let missing_loose: Vec<_> = q
            .required
            .iter()
            .filter(|term| !combined.contains(&term.to_lowercase()))
            .collect();
        if !missing_loose.is_empty() {
            failures.push(format!(
                "[loose] query {:?}: top {} did not contain required term(s) {:?}; got ids {:?}",
                q.query,
                r::KB_FINAL_TOP_K,
                missing_loose,
                hits.iter().map(|p| &p.id).collect::<Vec<_>>(),
            ));
        }

        // Strict: canonical_id must be in top-k for entries that have one.
        if let Some(canonical) = q.canonical_id {
            if !hits.iter().any(|p| p.id == canonical) {
                failures.push(format!(
                    "[strict] query {:?}: canonical_id {:?} not in top {}; got ids {:?}",
                    q.query,
                    canonical,
                    r::KB_FINAL_TOP_K,
                    hits.iter().map(|p| &p.id).collect::<Vec<_>>(),
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "retrieval-quality regressions at production defaults (top_k={}, min_score={:.2}):\n  - {}",
        r::KB_FINAL_TOP_K,
        r::KB_BM25_ONLY_MIN_SCORE,
        failures.join("\n  - ")
    );
}

#[tokio::test]
async fn seed_corpus_registers_sources_for_attribution() {
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    let stats = auto_seed_if_empty(&kb, Locale::English)
        .await
        .unwrap()
        .expect("seed dir must contain at least one *.en.jsonl");
    assert!(
        stats.sources_seen >= 2,
        "expected sources from at least two seed files, got {}",
        stats.sources_seen
    );

    let sources = kb.list_sources().await.unwrap();
    assert!(
        !sources.is_empty(),
        "every passage must register its source"
    );
    let mut seen_cc0 = false;
    let mut seen_cc_by_sa = false;
    for src in &sources {
        assert!(!src.license.is_empty(), "source {} missing license", src.id);
        assert!(
            !src.attribution.is_empty(),
            "source {} missing attribution",
            src.id
        );
        if src.license == "CC0-1.0" {
            seen_cc0 = true;
        }
        if src.license == "CC-BY-SA-3.0" {
            seen_cc_by_sa = true;
        }
    }
    assert!(seen_cc0, "expected at least one CC0-1.0 source (seed corpus)");
    assert!(seen_cc_by_sa, "expected at least one CC-BY-SA-3.0 source (wiki layer)");
}
