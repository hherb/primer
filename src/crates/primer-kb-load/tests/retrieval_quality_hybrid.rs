//! Hybrid (BM25 + dense-vector RRF) retrieval-quality integration test.
//!
//! Mirrors the BM25-only regression test in `retrieval_quality.rs` but
//! exercises the hybrid path via `KnowledgeBase::retrieve_hybrid` and
//! the production `HybridParams::default()`. Two flavours:
//!
//! * `hybrid_retrieval_structural_with_stub` — always built. Uses
//!   `StubEmbedder` (deterministic hash vectors, no model download).
//!   Asserts structural sanity only — that the hybrid path doesn't
//!   crash, returns ≤ `final_top_k` hits, and is idempotent. Does NOT
//!   assert recall: the stub embedder produces orthogonal vectors for
//!   synonyms and noticeably degrades the vector leg, so an assertion
//!   would be measuring stub-noise, not retrieval quality.
//! * `hybrid_retrieval_recall_with_fastembed` — gated on
//!   `--features fastembed`. Uses the real BGE-M3 model (~570 MB
//!   download on first run, cached afterwards). Pins the
//!   loose / strict recall floor measured by the 54-cell hybrid
//!   sweep at the production `(bm25_top_k=30, vector_top_k=30,
//!   final_top_k=5, rrf_k=60)` cell.
//!
//! Both tests load the same shared benchmark dataset
//! (`tests/common/mod.rs`) so the BM25 and hybrid paths converge on a
//! single source of truth for queries, canonical ids, and known-failing
//! entries.

mod common;

use common::QUERIES;
use primer_core::i18n::Locale;
use primer_core::knowledge::HybridParams;
use primer_embedding::StubEmbedder;
use primer_kb_load::{auto_seed_if_empty, reembed_kb};
use primer_knowledge::SqliteKnowledgeBase;

#[tokio::test]
async fn hybrid_retrieval_structural_with_stub() {
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    auto_seed_if_empty(&kb, Locale::English)
        .await
        .unwrap()
        .expect("seed dir must contain at least one *.en.jsonl");

    let embedder = StubEmbedder::new();
    reembed_kb(&kb, &embedder, false, 32)
        .await
        .expect("stub re-embed must succeed");

    let params = HybridParams::default();

    // Iterate over the full benchmark — structural checks are cheap
    // (no model download, all in-process). Idempotency is verified
    // per-query because tie-ordering nondeterminism in the fusion path
    // would only surface on queries whose candidate sets contain ties.
    for q in QUERIES {
        let h1 = kb
            .retrieve_hybrid(q.query, &embedder, &params)
            .await
            .unwrap_or_else(|e| panic!("hybrid retrieval errored for {:?}: {e}", q.query));
        assert!(
            !h1.is_empty(),
            "hybrid retrieval returned no hits for {:?}; BM25 leg must always surface something",
            q.query
        );
        assert!(
            h1.len() <= params.final_top_k,
            "hybrid retrieval returned {} hits for {:?}; final_top_k = {}",
            h1.len(),
            q.query,
            params.final_top_k
        );

        let h2 = kb
            .retrieve_hybrid(q.query, &embedder, &params)
            .await
            .unwrap();
        let ids1: Vec<&str> = h1.iter().map(|p| p.id.as_str()).collect();
        let ids2: Vec<&str> = h2.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids1, ids2,
            "hybrid retrieval not idempotent for {:?}: {:?} vs {:?}",
            q.query, ids1, ids2
        );
    }
}

#[cfg(feature = "fastembed")]
#[tokio::test]
async fn hybrid_retrieval_recall_with_fastembed() {
    use common::KNOWN_FAILING_QUERIES_HYBRID;
    use primer_embedding::FastEmbedBackend;

    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    auto_seed_if_empty(&kb, Locale::English)
        .await
        .unwrap()
        .expect("seed dir must contain at least one *.en.jsonl");

    let embedder = FastEmbedBackend::new().expect("BGE-M3 init failed (network or ort?)");
    reembed_kb(&kb, &embedder, false, 32)
        .await
        .expect("BGE-M3 re-embed must succeed");

    let params = HybridParams::default();

    let mut loose_failures = Vec::new();
    let mut strict_failures = Vec::new();
    let mut strict_total = 0usize;

    for q in QUERIES {
        if KNOWN_FAILING_QUERIES_HYBRID.contains(&q.query) {
            continue;
        }
        let hits = kb
            .retrieve_hybrid(q.query, &embedder, &params)
            .await
            .unwrap();
        let combined = hits
            .iter()
            .map(|p| p.text.to_lowercase())
            .collect::<Vec<_>>()
            .join(" \n ");

        let missing_loose: Vec<_> = q
            .required
            .iter()
            .filter(|term| !combined.contains(&term.to_lowercase()))
            .collect();
        if !missing_loose.is_empty() {
            loose_failures.push(format!(
                "[loose] query {:?}: top {} did not contain required term(s) {:?}; got ids {:?}",
                q.query,
                params.final_top_k,
                missing_loose,
                hits.iter().map(|p| &p.id).collect::<Vec<_>>(),
            ));
        }

        if let Some(canonical) = q.canonical_id {
            strict_total += 1;
            if !hits.iter().any(|p| p.id == canonical) {
                strict_failures.push(format!(
                    "[strict] query {:?}: canonical_id {:?} not in top {}; got ids {:?}",
                    q.query,
                    canonical,
                    params.final_top_k,
                    hits.iter().map(|p| &p.id).collect::<Vec<_>>(),
                ));
            }
        }
    }

    // The sweep at HybridParams::default() recorded 100% loose / 100%
    // strict on the 87-query / 20-canonical benchmark. Pin both as
    // hard floors — any drop is a regression worth investigating.
    let loose_failed = !loose_failures.is_empty();
    let strict_failed = !strict_failures.is_empty();
    let mut msg = String::new();
    if loose_failed {
        msg.push_str("Loose-recall regressions:\n  - ");
        msg.push_str(&loose_failures.join("\n  - "));
        msg.push('\n');
    }
    if strict_failed {
        msg.push_str(&format!(
            "Strict-recall regressions ({} of {} canonical queries failed):\n  - ",
            strict_failures.len(),
            strict_total
        ));
        msg.push_str(&strict_failures.join("\n  - "));
    }
    assert!(
        !loose_failed && !strict_failed,
        "hybrid retrieval-quality regressions at HybridParams::default():\n{msg}"
    );
}
