//! Hybrid (BM25 + dense-vector RRF) retrieval-parameter sweep grid.
//!
//! Gated on the `fastembed` feature (the module is only declared in
//! [`super`] under `#[cfg(feature = "fastembed")]`). See the
//! [module-level docs](super) for the shared lex-selection rule and the
//! byte-identical-output contract.

use std::cmp::Ordering;
use std::sync::Arc;

use crate::common::BenchQuery;
use primer_core::embedder::Embedder;
use primer_core::i18n::Locale;
use primer_core::knowledge::HybridParams;
use primer_embedding::FastEmbedBackend;
use primer_kb_load::{auto_seed_if_empty, reembed_kb};
use primer_knowledge::SqliteKnowledgeBase;

/// BM25 leg top-K explored by the hybrid sweep grid.
pub const HYBRID_BM25_TOP_K_GRID: &[usize] = &[10, 20, 30];
/// Vector leg top-K explored by the hybrid sweep grid.
pub const HYBRID_VECTOR_TOP_K_GRID: &[usize] = &[10, 20, 30];
/// Final (post-RRF-fusion) top-K explored.
pub const HYBRID_FINAL_TOP_K_GRID: &[usize] = &[3, 5];
/// RRF k constant explored. 60.0 is the production default; smaller
/// weights the very top of each leg more, larger flattens.
pub const HYBRID_RRF_K_GRID: &[f64] = &[30.0, 60.0, 90.0];
/// Post-fusion score floor explored. RRF scores are bounded by ~`1/k`
/// per leg (≈0.0164 at `rrf_k=60` for the top-ranked hit in one leg),
/// so a meaningful floor lives near `0.0..=0.02`. 0.0 is the
/// production default (no filtering); non-zero floors drop low-
/// confidence fused passages and can either trim noise or regress
/// recall on marginal queries. See issue #46.
pub const HYBRID_MIN_SCORE_GRID: &[f64] = &[0.0, 0.005, 0.01, 0.02];
/// RRF k value used as the tie-break preference. Coupled at compile
/// time to the production default so a future change to
/// [`primer_core::consts::retrieval::RRF_K`] doesn't silently drift
/// from the sweep's tie-break preference.
pub const HYBRID_PREFERRED_RRF_K: f64 = primer_core::consts::retrieval::RRF_K;
/// `min_score` value used as the tie-break preference. Coupled to the
/// production default so flipping `KB_MIN_SCORE` (today `0.0` — no
/// floor) automatically realigns the sweep's tie-break preference.
pub const HYBRID_PREFERRED_MIN_SCORE: f64 = primer_core::consts::retrieval::KB_MIN_SCORE;
/// Width of the dashed separator line under the hybrid table header.
/// Widened from 70 to 76 columns to accommodate the new `Floor`
/// column added for the `min_score` axis (issue #46).
pub const HYBRID_DASH_WIDTH: usize = 76;

pub struct HybridSweepConfig<'a> {
    pub locale: Locale,
    pub queries: &'a [BenchQuery],
    pub table_title: &'a str,
    pub winner_label: &'a str,
    pub closing_note: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct HybridCellMetrics {
    pub bm25_top_k: usize,
    pub vector_top_k: usize,
    pub final_top_k: usize,
    pub rrf_k: f64,
    pub min_score: f64,
    pub loose_pass: usize,
    pub loose_total: usize,
    pub strict_pass: usize,
    pub strict_total: usize,
}

impl HybridCellMetrics {
    pub fn loose_recall(&self) -> f64 {
        if self.loose_total == 0 {
            0.0
        } else {
            self.loose_pass as f64 / self.loose_total as f64
        }
    }
    pub fn strict_recall(&self) -> f64 {
        if self.strict_total == 0 {
            0.0
        } else {
            self.strict_pass as f64 / self.strict_total as f64
        }
    }
}

// The args mirror the five hybrid grid axes one-to-one; bundling them
// into a struct here would just duplicate `HybridParams` for no clarity
// gain in this diagnostic-only harness.
#[allow(clippy::too_many_arguments)]
async fn evaluate_hybrid_cell(
    kb: &SqliteKnowledgeBase,
    embedder: &dyn Embedder,
    queries: &[BenchQuery],
    bm25_top_k: usize,
    vector_top_k: usize,
    final_top_k: usize,
    rrf_k: f64,
    min_score: f64,
) -> HybridCellMetrics {
    let mut loose_pass = 0;
    let mut loose_total = 0;
    let mut strict_pass = 0;
    let mut strict_total = 0;

    for q in queries {
        let params = HybridParams {
            bm25_top_k,
            vector_top_k,
            final_top_k,
            rrf_k,
            min_score,
            source_filter: vec![],
        };
        let hits = kb
            .retrieve_hybrid(q.query, embedder, &params)
            .await
            .unwrap();
        let combined = hits
            .iter()
            .map(|p| p.text.to_lowercase())
            .collect::<Vec<_>>()
            .join(" \n ");
        let loose_ok = q
            .required
            .iter()
            .all(|term| combined.contains(&term.to_lowercase()));
        loose_total += 1;
        if loose_ok {
            loose_pass += 1;
        }
        if let Some(canonical) = q.canonical_id {
            strict_total += 1;
            if hits.iter().any(|p| p.id == canonical) {
                strict_pass += 1;
            }
        }
    }

    HybridCellMetrics {
        bm25_top_k,
        vector_top_k,
        final_top_k,
        rrf_k,
        min_score,
        loose_pass,
        loose_total,
        strict_pass,
        strict_total,
    }
}

/// Lex-selection: max strict, max loose, min final_top_k, RRF k
/// closest to the production default `HYBRID_PREFERRED_RRF_K`,
/// `min_score` closest to the production default
/// `HYBRID_PREFERRED_MIN_SCORE`. Defensive
/// `.unwrap_or(Ordering::Equal)` on every partial_cmp (closes #67).
///
/// The `min_score` tie-break is last because RRF rank-fusion already
/// quasi-normalises the score scale (max contribution per leg is
/// `1/(rrf_k+1)`), so the floor's practical effect at production
/// `final_top_k` is small. Putting it after RRF k means a sweep that
/// finds an outperforming non-zero floor will surface in the printed
/// table; the tie-break preference for the production default keeps
/// "no change" the algorithmic winner when the new axis is neutral.
fn pick_hybrid_winner(cells: &[HybridCellMetrics]) -> &HybridCellMetrics {
    cells
        .iter()
        .max_by(|a, b| {
            a.strict_recall()
                .partial_cmp(&b.strict_recall())
                .unwrap_or(Ordering::Equal)
                .then(
                    a.loose_recall()
                        .partial_cmp(&b.loose_recall())
                        .unwrap_or(Ordering::Equal),
                )
                .then(b.final_top_k.cmp(&a.final_top_k))
                .then(
                    (a.rrf_k - HYBRID_PREFERRED_RRF_K)
                        .abs()
                        .partial_cmp(&(b.rrf_k - HYBRID_PREFERRED_RRF_K).abs())
                        .unwrap_or(Ordering::Equal)
                        .reverse(),
                )
                .then(
                    (a.min_score - HYBRID_PREFERRED_MIN_SCORE)
                        .abs()
                        .partial_cmp(&(b.min_score - HYBRID_PREFERRED_MIN_SCORE).abs())
                        .unwrap_or(Ordering::Equal)
                        .reverse(),
                )
        })
        .expect("non-empty grid")
}

/// Run the hybrid sweep end-to-end: seed the KB, backfill embeddings
/// with BGE-M3, evaluate every cell on the 4-axis grid, print the
/// table, identify the notional winner. Defaults remain unchanged —
/// this is diagnostic-only infrastructure.
pub async fn run_hybrid_sweep(cfg: HybridSweepConfig<'_>) {
    println!("\n=== {} ===", cfg.table_title);
    println!(">>> first run downloads ~570 MB; subsequent runs are warm\n");

    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), cfg.locale).unwrap();
    auto_seed_if_empty(&kb, cfg.locale).await.unwrap().unwrap();

    let embedder: Arc<dyn Embedder> =
        Arc::new(FastEmbedBackend::new().expect("BGE-M3 init failed"));
    reembed_kb(&kb, embedder.as_ref(), false, 32).await.unwrap();

    println!(
        "{:>4} {:>4} {:>4} {:>5} {:>5} {:>14} {:>15}",
        "BM25", "Vec", "Top", "RRF", "Floor", "loose_recall", "strict_recall"
    );
    println!("{}", "-".repeat(HYBRID_DASH_WIDTH));

    let mut cells: Vec<HybridCellMetrics> = Vec::new();
    for &bm25_top_k in HYBRID_BM25_TOP_K_GRID {
        for &vector_top_k in HYBRID_VECTOR_TOP_K_GRID {
            for &final_top_k in HYBRID_FINAL_TOP_K_GRID {
                for &rrf_k in HYBRID_RRF_K_GRID {
                    for &min_score in HYBRID_MIN_SCORE_GRID {
                        let m = evaluate_hybrid_cell(
                            &kb,
                            embedder.as_ref(),
                            cfg.queries,
                            bm25_top_k,
                            vector_top_k,
                            final_top_k,
                            rrf_k,
                            min_score,
                        )
                        .await;
                        println!(
                            "{:>4} {:>4} {:>4} {:>5.0} {:>5.3} {:>9}/{:>3}={:>3.0}% {:>9}/{:>3}={:>3.0}%",
                            m.bm25_top_k,
                            m.vector_top_k,
                            m.final_top_k,
                            m.rrf_k,
                            m.min_score,
                            m.loose_pass,
                            m.loose_total,
                            m.loose_recall() * 100.0,
                            m.strict_pass,
                            m.strict_total,
                            m.strict_recall() * 100.0,
                        );
                        cells.push(m);
                    }
                }
            }
        }
    }

    let winner = pick_hybrid_winner(&cells);
    println!(
        "\n>>> {}: bm25_top_k={}, vector_top_k={}, final_top_k={}, rrf_k={:.0}, min_score={:.3}",
        cfg.winner_label,
        winner.bm25_top_k,
        winner.vector_top_k,
        winner.final_top_k,
        winner.rrf_k,
        winner.min_score,
    );
    println!(
        ">>> strict_recall={:.0}%, loose_recall={:.0}%",
        winner.strict_recall() * 100.0,
        winner.loose_recall() * 100.0,
    );
    println!(">>> {}", cfg.closing_note);
}

#[cfg(test)]
mod selection_tests {
    //! Characterisation tests for [`pick_hybrid_winner`].
    //!
    //! Pins the lex-selection rule: max strict_recall → max loose_recall
    //! → min final_top_k → RRF k closest to [`HYBRID_PREFERRED_RRF_K`]
    //! → `min_score` closest to [`HYBRID_PREFERRED_MIN_SCORE`].
    //! Each test isolates one axis so a future hand-edit that inverts
    //! a `.then(...)` clause fails loudly here.
    use super::*;

    fn cell(
        final_top_k: usize,
        rrf_k: f64,
        min_score: f64,
        loose_pass: usize,
        strict_pass: usize,
    ) -> HybridCellMetrics {
        HybridCellMetrics {
            bm25_top_k: 30,
            vector_top_k: 30,
            final_top_k,
            rrf_k,
            min_score,
            loose_pass,
            loose_total: 10,
            strict_pass,
            strict_total: 5,
        }
    }

    #[test]
    fn strict_recall_wins_over_loose_recall() {
        let cells = vec![cell(5, 60.0, 0.0, 10, 1), cell(5, 60.0, 0.0, 0, 2)];
        let winner = pick_hybrid_winner(&cells);
        assert_eq!(winner.strict_pass, 2);
    }

    #[test]
    fn loose_recall_breaks_strict_tie() {
        let cells = vec![cell(5, 60.0, 0.0, 3, 1), cell(5, 60.0, 0.0, 5, 1)];
        let winner = pick_hybrid_winner(&cells);
        assert_eq!(winner.loose_pass, 5);
    }

    #[test]
    fn smaller_final_top_k_breaks_recall_tie() {
        let cells = vec![cell(5, 60.0, 0.0, 5, 1), cell(3, 60.0, 0.0, 5, 1)];
        let winner = pick_hybrid_winner(&cells);
        assert_eq!(winner.final_top_k, 3);
    }

    #[test]
    fn preferred_rrf_k_wins_over_distant_rrf_k() {
        // strict + loose + final_top_k equal → rrf_k closest to
        // HYBRID_PREFERRED_RRF_K wins. Production default is 60.
        let cells = vec![
            cell(5, 30.0, 0.0, 5, 1),
            cell(5, 60.0, 0.0, 5, 1),
            cell(5, 90.0, 0.0, 5, 1),
        ];
        let winner = pick_hybrid_winner(&cells);
        assert!(
            (winner.rrf_k - HYBRID_PREFERRED_RRF_K).abs() < f64::EPSILON,
            "winner rrf_k={} should match HYBRID_PREFERRED_RRF_K={}",
            winner.rrf_k,
            HYBRID_PREFERRED_RRF_K,
        );
    }

    #[test]
    fn preferred_min_score_wins_over_distant_min_score() {
        // strict + loose + final_top_k + rrf_k equal → min_score
        // closest to HYBRID_PREFERRED_MIN_SCORE wins. Production
        // default is 0.0 (no floor); the grid offers {0.0, 0.005,
        // 0.01, 0.02} so the production default must lex-win on a
        // four-way recall tie.
        let cells = vec![
            cell(5, 60.0, 0.0, 5, 1),
            cell(5, 60.0, 0.005, 5, 1),
            cell(5, 60.0, 0.01, 5, 1),
            cell(5, 60.0, 0.02, 5, 1),
        ];
        let winner = pick_hybrid_winner(&cells);
        assert!(
            (winner.min_score - HYBRID_PREFERRED_MIN_SCORE).abs() < f64::EPSILON,
            "winner min_score={} should match HYBRID_PREFERRED_MIN_SCORE={}",
            winner.min_score,
            HYBRID_PREFERRED_MIN_SCORE,
        );
    }

    #[test]
    fn nontrivial_floor_wins_when_recall_strictly_better() {
        // If a non-zero floor produces a strictly better strict_recall,
        // it must beat the production default — the new axis's whole
        // purpose is to surface such an improvement. Pins that the
        // tie-break preference does not mask a real recall win.
        let cells = vec![cell(5, 60.0, 0.0, 5, 1), cell(5, 60.0, 0.01, 5, 2)];
        let winner = pick_hybrid_winner(&cells);
        assert_eq!(winner.strict_pass, 2);
        assert!((winner.min_score - 0.01).abs() < f64::EPSILON);
    }
}
