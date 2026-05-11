//! BM25-only retrieval-quality integration test for the `de` locale.
//!
//! Mirrors `retrieval_quality.rs` for the German Klexikon corpus
//! (CC-BY-SA-4.0; 66 articles at the time of authoring). Loads the
//! seed JSONL into a temp `de`-locale KB, runs each `QUERIES_DE` entry
//! through `KnowledgeBase::retrieve` with the production defaults
//! (`KB_FINAL_TOP_K`, `KB_BM25_ONLY_MIN_SCORE`), and asserts:
//!
//! * **Loose:** every `required` substring appears in the combined
//!   top-k text (case-insensitive).
//! * **Strict:** when `canonical_id` is set, the named passage id
//!   must appear in the returned hits.
//!
//! Queries the BM25-only defaults cannot satisfy live in
//! `common::de::KNOWN_FAILING_QUERIES_DE` and are skipped here. The
//! hybrid path (`retrieval_quality_hybrid_de.rs`) typically lifts the
//! BM25 misses thanks to the dense leg.

mod common;

use common::de::{KNOWN_FAILING_QUERIES_DE, QUERIES_DE};
use primer_core::consts::retrieval as r;
use primer_core::i18n::Locale;
use primer_core::knowledge::{KnowledgeBase, RetrievalParams};
use primer_kb_load::auto_seed_if_empty;
use primer_knowledge::SqliteKnowledgeBase;

#[tokio::test]
async fn de_seed_corpus_satisfies_canonical_queries() {
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::German).unwrap();
    let stats = auto_seed_if_empty(&kb, Locale::German)
        .await
        .unwrap()
        .expect("seed dir must contain at least one *.de.jsonl");
    assert!(
        stats.inserted >= 10,
        "DE seed corpus must contain at least 10 passages, got {}",
        stats.inserted
    );

    let mut failures = Vec::new();
    for q in QUERIES_DE {
        if KNOWN_FAILING_QUERIES_DE.contains(&q.query) {
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
        "DE retrieval-quality regressions at production BM25-only defaults (top_k={}, min_score={:.2}):\n  - {}",
        r::KB_FINAL_TOP_K,
        r::KB_BM25_ONLY_MIN_SCORE,
        failures.join("\n  - ")
    );
}

#[tokio::test]
async fn de_seed_corpus_registers_klexikon_source() {
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::German).unwrap();
    let stats = auto_seed_if_empty(&kb, Locale::German)
        .await
        .unwrap()
        .expect("seed dir must contain at least one *.de.jsonl");
    assert!(
        stats.sources_seen >= 1,
        "expected at least one DE source row, got {}",
        stats.sources_seen
    );

    let sources = kb.list_sources().await.unwrap();
    assert!(
        !sources.is_empty(),
        "every passage must register its source"
    );
    let mut seen_klexikon = false;
    for src in &sources {
        assert!(!src.license.is_empty(), "source {} missing license", src.id);
        assert!(
            !src.attribution.is_empty(),
            "source {} missing attribution",
            src.id
        );
        // The literal `"CC-BY-SA-4.0"` must match `WikiSource::license`
        // for the `KLEXIKON` preset in
        // `data/ingest/wiki/source.py` exactly. If the ingest layer is
        // ever rewritten to a different SPDX-style spelling
        // (`cc-by-sa-4.0`, `CC-BY-SA-4.0-DE`, ...), update both sides
        // in lockstep. The whole-string match is intentional: a
        // `starts_with("CC-BY-SA-4")` would silently accept a wrong
        // jurisdiction marker.
        if src.license == "CC-BY-SA-4.0" {
            seen_klexikon = true;
        }
    }
    assert!(
        seen_klexikon,
        "expected at least one CC-BY-SA-4.0 source (Klexikon layer)"
    );
}
