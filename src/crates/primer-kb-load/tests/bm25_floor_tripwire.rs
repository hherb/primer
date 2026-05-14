//! Tripwire diagnostic for `KB_BM25_ONLY_MIN_SCORE`.
//!
//! `#[ignore]`'d so it doesn't run in default CI. Run via:
//!
//! ```text
//! ~/.cargo/bin/cargo test -p primer-kb-load --test bm25_floor_tripwire \
//!     -- --ignored bm25_score_floor_tripwire --nocapture
//! ```
//!
//! Probes the seed corpus with the shared benchmark queries and asserts
//! that every BM25 score in the production top-K window comfortably
//! exceeds `KB_BM25_ONLY_MIN_SCORE`. The floor is intentionally retained
//! as defensive insurance against a future corpus expansion that dilutes
//! term frequencies; this diagnostic fires loudly when that dilution
//! starts to bite, with a hint to revisit the const.
//!
//! Why a tripwire? On the current 90-passage corpus, the 24-cell sweep
//! at `tests/retrieval_sweep.rs` showed every BM25 score sits above ~1.5
//! and the 0.5 floor is a no-op. Without an automated probe, a future
//! corpus expansion that drops scores below 0.5 would silently degrade
//! retrieval — affected queries would lose top-K passages with no
//! existing test catching the change.
//!
//! Why `#[ignore]`'d? Per the design intent in GitHub issue #44:
//! the diagnostic is meant to be run alongside the sweep when the
//! corpus changes, not on every CI build. Promoting it to default-CI
//! is a deliberate future call; at that point it would also make sense
//! to lift the helpers into a shared module.
//!
//! Pure helpers (`sorted_ascending`, `min_score`, `median_score`) carry
//! their own unit tests so the diagnostic's arithmetic is verified
//! without paying the cost of seeding a KB.
//!
//! See also: `tests/retrieval_sweep.rs` (parameter sweep that originally
//! identified the floor as a no-op) and the design spec at
//! `docs/superpowers/specs/2026-05-06-retrieval-tuning-design.md`.

mod common;

use common::QUERIES;
use primer_core::consts::retrieval::{KB_BM25_ONLY_MIN_SCORE, KB_FINAL_TOP_K};
use primer_core::i18n::Locale;
use primer_core::knowledge::{KnowledgeBase, RetrievalParams};
use primer_kb_load::auto_seed_if_empty;
use primer_knowledge::SqliteKnowledgeBase;

/// Multiplier applied to `KB_BM25_ONLY_MIN_SCORE` when checking the
/// median score across the benchmark. Acts as an *early-warning*
/// threshold: if the median drops below `floor × margin` the test trips
/// well before the minimum-check (which would only trip after the
/// floor is *already* filtering hits).
///
/// 2.0 is conservative against the current corpus where the median sits
/// well above 5x the floor — a 4x compression of typical scores would
/// be needed to trip this. Tighten to a smaller margin only after the
/// corpus has stabilised at a larger size.
const TRIPWIRE_MEDIAN_MARGIN: f64 = 2.0;

/// Sort a slice of `f64` ascending. NaN is treated as equal so the
/// comparator is total — the benchmark scores are never NaN, but
/// being defensive keeps a future corpus regression from surfacing
/// as a panic instead of a clear assertion failure.
fn sorted_ascending(scores: &[f64]) -> Vec<f64> {
    let mut v = scores.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v
}

/// Minimum of a non-empty slice. Panics on empty input — every caller
/// in this file guarantees at least one score from a non-empty corpus.
fn min_score(scores: &[f64]) -> f64 {
    *sorted_ascending(scores)
        .first()
        .expect("min of empty slice is undefined")
}

/// Median of a non-empty slice. Mid-element for odd `n`; mean of the
/// two centre elements for even `n`. Panics on empty input.
fn median_score(scores: &[f64]) -> f64 {
    let sorted = sorted_ascending(scores);
    let n = sorted.len();
    assert!(n > 0, "median of empty slice is undefined");
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

#[tokio::test]
#[ignore]
async fn bm25_score_floor_tripwire() {
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    auto_seed_if_empty(&kb, Locale::English)
        .await
        .unwrap()
        .unwrap();

    // Probe with min_score = NEG_INFINITY so the production floor isn't
    // applied — we want every top-K score visible, including ones that
    // would *currently* be filtered. Otherwise the test could only
    // observe scores already above the floor and the tripwire would be
    // measuring the wrong thing.
    let probe_params = RetrievalParams {
        top_k: KB_FINAL_TOP_K,
        min_score: f64::NEG_INFINITY,
        source_filter: vec![],
    };
    let mut all_scores: Vec<f64> = Vec::new();
    let mut top1_scores: Vec<f64> = Vec::new();
    for q in QUERIES {
        let hits = kb.retrieve(q.query, &probe_params).await.unwrap();
        if let Some(top1) = hits.first() {
            top1_scores.push(top1.score);
        }
        for hit in &hits {
            all_scores.push(hit.score);
        }
    }

    let total = all_scores.len();
    assert!(
        total > 0,
        "no scores collected — every query returned zero hits, which means the seed corpus is empty"
    );

    let min_all = min_score(&all_scores);
    let median_all = median_score(&all_scores);
    let min_top1 = min_score(&top1_scores);
    let median_floor = KB_BM25_ONLY_MIN_SCORE * TRIPWIRE_MEDIAN_MARGIN;

    println!("\n=== BM25 score floor tripwire ===\n");
    println!("floor (KB_BM25_ONLY_MIN_SCORE):       {KB_BM25_ONLY_MIN_SCORE:.2}");
    println!(
        "margin (TRIPWIRE_MEDIAN_MARGIN):      {TRIPWIRE_MEDIAN_MARGIN:.1}x  →  required median > {median_floor:.2}"
    );
    println!(
        "scores collected:                     {} queries × top_k={} = {} top-K scores",
        QUERIES.len(),
        KB_FINAL_TOP_K,
        total
    );
    println!();
    println!(
        "min  (over all top-K scores):         {:.2}  [{}]",
        min_all,
        pass_or_fail(min_all > KB_BM25_ONLY_MIN_SCORE)
    );
    println!(
        "median (over all top-K scores):       {:.2}  [{}]",
        median_all,
        pass_or_fail(median_all > median_floor)
    );
    println!("min top-1 (lowest top-1 across queries): {min_top1:.2}");
    println!();

    assert!(
        min_all > KB_BM25_ONLY_MIN_SCORE,
        "minimum BM25 score across {total} top-K hits ({min_all:.4}) does not exceed \
         KB_BM25_ONLY_MIN_SCORE ({KB_BM25_ONLY_MIN_SCORE:.2}). The defensive floor in \
         primer-core/src/consts.rs::KB_BM25_ONLY_MIN_SCORE is now \
         filtering genuine top-K hits — affected queries silently lose \
         a top-K passage. Either lower the floor (and rerun the sweep \
         at tests/retrieval_sweep.rs to pick a new defensive value) or \
         document the corpus expansion that motivated keeping it.",
    );
    assert!(
        median_all > median_floor,
        "median BM25 score across {total} top-K hits ({median_all:.4}) does not exceed \
         {TRIPWIRE_MEDIAN_MARGIN:.1}x the floor ({median_floor:.2} required). Corpus expansion has \
         narrowed the safety margin between typical BM25 scores and \
         KB_BM25_ONLY_MIN_SCORE. Revisit the const before the \
         minimum-check trips: rerun tests/retrieval_sweep.rs to confirm \
         the floor is still a no-op, then either lower the floor or \
         tighten TRIPWIRE_MEDIAN_MARGIN if the new corpus baseline is \
         intentionally lower.",
    );
}

/// Compact PASS/FAIL string for the diagnostic output. Pure helper so
/// the formatting in `bm25_score_floor_tripwire` stays uncluttered.
fn pass_or_fail(condition: bool) -> &'static str {
    if condition { "PASS" } else { "FAIL" }
}

#[cfg(test)]
mod helper_tests {
    use super::*;

    #[test]
    fn min_score_picks_smallest() {
        assert_eq!(min_score(&[3.0, 1.0, 2.0]), 1.0);
    }

    #[test]
    fn min_score_handles_negatives() {
        assert_eq!(min_score(&[-1.0, -3.0, -2.0]), -3.0);
    }

    #[test]
    fn median_odd_length() {
        assert_eq!(median_score(&[1.0, 3.0, 2.0]), 2.0);
    }

    #[test]
    fn median_even_length_averages_two_centres() {
        assert_eq!(median_score(&[1.0, 2.0, 3.0, 4.0]), 2.5);
    }

    #[test]
    fn median_single_element() {
        assert_eq!(median_score(&[7.5]), 7.5);
    }

    #[test]
    fn sorted_ascending_does_not_mutate_input() {
        let input = [3.0, 1.0, 2.0];
        let _ = sorted_ascending(&input);
        // Input slice is by reference; we sort a clone. This is
        // expressed at the type level (`&[f64]` is immutable), so the
        // assertion below is documentary — if a future refactor took
        // `&mut [f64]` it would no longer compile here.
        assert_eq!(input, [3.0, 1.0, 2.0]);
    }

    #[test]
    fn pass_or_fail_strings() {
        assert_eq!(pass_or_fail(true), "PASS");
        assert_eq!(pass_or_fail(false), "FAIL");
    }
}
