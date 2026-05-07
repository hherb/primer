//! Retrieval-quality integration test.
//!
//! Loads the in-repo English seed corpus into a temp DB and asserts
//! that canonical child queries surface passages mentioning the
//! expected key terms. The benchmark dataset lives in
//! `tests/common/mod.rs` so the diagnostic sweep
//! (`tests/retrieval_sweep.rs`) shares it.
//!
//! This is the truth signal for the production
//! `KB_FINAL_TOP_K` / `KB_BM25_ONLY_MIN_SCORE` defaults — if a query
//! needs a higher `top_k` to hit, that's a tuning signal surfaced by
//! the sweep.

mod common;

use common::QUERIES;
use primer_core::i18n::Locale;
use primer_core::knowledge::{KnowledgeBase, RetrievalParams};
use primer_kb_load::auto_seed_if_empty;
use primer_knowledge::SqliteKnowledgeBase;

/// Top-K used by the regression test. Mirrors the dialogue manager's
/// realistic retrieval depth once `RetrievalParams::default()` is
/// tuned (Task 6 of the retrieval-tuning plan replaces this constant
/// with `primer_core::consts::retrieval::KB_FINAL_TOP_K`).
const REGRESSION_TOP_K: usize = 5;

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
        let params = RetrievalParams {
            top_k: REGRESSION_TOP_K,
            min_score: f64::NEG_INFINITY,
            source_filter: vec![],
        };
        let hits = kb.retrieve(q.query, &params).await.unwrap();
        let combined = hits
            .iter()
            .map(|p| p.text.to_lowercase())
            .collect::<Vec<_>>()
            .join(" \n ");
        let missing: Vec<_> = q
            .required
            .iter()
            .filter(|term| !combined.contains(&term.to_lowercase()))
            .collect();
        if !missing.is_empty() {
            failures.push(format!(
                "query {:?}: top {} did not contain required term(s) {:?}; got ids {:?}",
                q.query,
                REGRESSION_TOP_K,
                missing,
                hits.iter().map(|p| &p.id).collect::<Vec<_>>(),
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "retrieval-quality regressions:\n  - {}",
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
