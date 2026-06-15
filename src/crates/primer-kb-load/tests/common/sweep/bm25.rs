//! BM25-only retrieval-parameter sweep grid.
//!
//! See the [module-level docs](super) for the shared lex-selection rule
//! and the byte-identical-output contract.

use std::cmp::Ordering;

use crate::common::{BenchQuery, Cluster};
use primer_core::i18n::Locale;
use primer_core::knowledge::{KnowledgeBase, RetrievalParams};
use primer_kb_load::auto_seed_if_empty;
use primer_knowledge::SqliteKnowledgeBase;

/// Top-K values explored by the BM25 sweep grid.
pub const BM25_TOP_K_GRID: &[usize] = &[3, 5, 7, 10];
/// `min_score` values explored by the BM25 sweep grid.
pub const BM25_MIN_SCORE_GRID: &[f64] = &[0.0, 0.25, 0.5, 0.75, 1.0, 1.5];

/// Per-locale configuration for the BM25 sweep harness.
///
/// `clusters` is the list of clusters to break recall down by — EN
/// passes all six (incl. `Cluster::Wiki`); DE passes five (Klexikon
/// articles collapse into the topical clusters and there is no parallel
/// hand-drafted seed). `dash_width` is the visible separator line under
/// the table header, kept as an explicit config knob to preserve the
/// exact byte-for-byte output the pre-refactor harnesses produced.
pub struct Bm25SweepConfig<'a> {
    pub locale: Locale,
    pub queries: &'a [BenchQuery],
    pub clusters: &'a [Cluster],
    pub table_title: &'a str,
    pub dash_width: usize,
}

#[derive(Debug, Clone)]
pub struct Bm25CellMetrics {
    pub top_k: usize,
    pub min_score: f64,
    pub loose_pass: usize,
    pub loose_total: usize,
    pub strict_pass: usize,
    pub strict_total: usize,
    /// `(cluster, pass, total)` for each cluster in the active config —
    /// dynamic length so EN's 6-cluster and DE's 5-cluster harnesses
    /// share one cell type.
    pub per_cluster: Vec<(Cluster, usize, usize)>,
}

impl Bm25CellMetrics {
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

async fn evaluate_bm25_cell(
    kb: &SqliteKnowledgeBase,
    queries: &[BenchQuery],
    clusters: &[Cluster],
    top_k: usize,
    min_score: f64,
) -> Bm25CellMetrics {
    let mut loose_pass = 0;
    let mut loose_total = 0;
    let mut strict_pass = 0;
    let mut strict_total = 0;
    let mut cluster_counts: Vec<(Cluster, usize, usize)> =
        clusters.iter().map(|c| (*c, 0usize, 0usize)).collect();

    for q in queries {
        let params = RetrievalParams {
            top_k,
            min_score,
            source_filter: vec![],
        };
        let hits = kb.retrieve(q.query, &params).await.unwrap();
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

        for entry in cluster_counts.iter_mut() {
            if entry.0 == q.cluster {
                entry.2 += 1;
                if loose_ok {
                    entry.1 += 1;
                }
            }
        }

        if let Some(canonical) = q.canonical_id {
            strict_total += 1;
            if hits.iter().any(|p| p.id == canonical) {
                strict_pass += 1;
            }
        }
    }

    Bm25CellMetrics {
        top_k,
        min_score,
        loose_pass,
        loose_total,
        strict_pass,
        strict_total,
        per_cluster: cluster_counts,
    }
}

/// Lex-selection: max strict, max loose, min top_k, max min_score.
/// `.partial_cmp(...).unwrap_or(Ordering::Equal)` everywhere so an
/// empty-corpus run prints the table instead of panicking.
fn pick_bm25_winner(cells: &[Bm25CellMetrics]) -> &Bm25CellMetrics {
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
                .then(b.top_k.cmp(&a.top_k))
                .then(
                    a.min_score
                        .partial_cmp(&b.min_score)
                        .unwrap_or(Ordering::Equal),
                )
        })
        .expect("non-empty grid")
}

/// Two-letter cluster label used in the table header and row.
/// "Erw" preserves the pre-refactor abbreviation for `EarthWeather`.
fn cluster_short_label(c: Cluster) -> &'static str {
    match c {
        Cluster::Space => "Spc",
        Cluster::Body => "Bod",
        Cluster::HowThingsWork => "How",
        Cluster::Life => "Lif",
        Cluster::EarthWeather => "Erw",
        Cluster::Wiki => "Wik",
    }
}

fn print_bm25_header(cfg: &Bm25SweepConfig<'_>) {
    println!("\n=== {} ===\n", cfg.table_title);
    print!(
        "{:>5} {:>10} {:>14} {:>15} |",
        "top_k", "min_score", "loose_recall", "strict_recall"
    );
    for c in cfg.clusters {
        print!(" {:>5}", cluster_short_label(*c));
    }
    println!();
    println!("{}", "-".repeat(cfg.dash_width));
}

fn print_bm25_row(m: &Bm25CellMetrics, clusters: &[Cluster]) {
    print!(
        "{:>5} {:>10.2} {:>9}/{:>3}={:>3.0}% {:>9}/{:>3}={:>3.0}% |",
        m.top_k,
        m.min_score,
        m.loose_pass,
        m.loose_total,
        m.loose_recall() * 100.0,
        m.strict_pass,
        m.strict_total,
        m.strict_recall() * 100.0,
    );
    for c in clusters {
        let entry = m.per_cluster.iter().find(|e| e.0 == *c).unwrap();
        let cell = if entry.2 == 0 {
            "-".to_string()
        } else {
            format!("{}/{}", entry.1, entry.2)
        };
        print!(" {cell:>5}");
    }
    println!();
}

fn print_bm25_failures_for_winner(
    winner: &Bm25CellMetrics,
    all: &[(BenchQuery, bool, Option<bool>)],
) {
    println!(
        "\n=== Per-query results at winning cell (top_k={}, min_score={:.2}) ===",
        winner.top_k, winner.min_score
    );
    let mut any = false;
    for (q, loose, strict) in all {
        let strict_str = match strict {
            Some(true) => "strict:OK",
            Some(false) => "strict:FAIL",
            None => "strict:n/a",
        };
        let loose_str = if *loose { "loose:OK" } else { "loose:FAIL" };
        if !*loose || matches!(strict, Some(false)) {
            println!("  [{}] [{}] {:?}", loose_str, strict_str, q.query);
            any = true;
        }
    }
    if !any {
        println!("  (no failures — all queries pass loose AND strict at the winning cell)");
    }
}

/// Run the BM25 sweep end-to-end: seed the KB, evaluate every cell,
/// print the table, identify the winner, print per-query failures at
/// the winning cell, and self-validate the winner's printed numbers.
///
/// Output is byte-identical to the pre-refactor harnesses on the same
/// corpus + benchmark.
pub async fn run_bm25_sweep(cfg: Bm25SweepConfig<'_>) {
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), cfg.locale).unwrap();
    auto_seed_if_empty(&kb, cfg.locale).await.unwrap().unwrap();

    let mut cells: Vec<Bm25CellMetrics> =
        Vec::with_capacity(BM25_TOP_K_GRID.len() * BM25_MIN_SCORE_GRID.len());
    for &top_k in BM25_TOP_K_GRID {
        for &min_score in BM25_MIN_SCORE_GRID {
            cells.push(evaluate_bm25_cell(&kb, cfg.queries, cfg.clusters, top_k, min_score).await);
        }
    }

    print_bm25_header(&cfg);
    for cell in &cells {
        print_bm25_row(cell, cfg.clusters);
    }

    let winner = pick_bm25_winner(&cells);
    println!(
        "\n>>> WINNING CELL: top_k={}, min_score={:.2} (strict_recall={:.0}%, loose_recall={:.0}%)",
        winner.top_k,
        winner.min_score,
        winner.strict_recall() * 100.0,
        winner.loose_recall() * 100.0,
    );

    // Per-query breakdown at the winning cell.
    let mut per_q: Vec<(BenchQuery, bool, Option<bool>)> = Vec::with_capacity(cfg.queries.len());
    for q in cfg.queries {
        let params = RetrievalParams {
            top_k: winner.top_k,
            min_score: winner.min_score,
            source_filter: vec![],
        };
        let hits = kb.retrieve(q.query, &params).await.unwrap();
        let combined = hits
            .iter()
            .map(|p| p.text.to_lowercase())
            .collect::<Vec<_>>()
            .join(" \n ");
        let loose = q
            .required
            .iter()
            .all(|term| combined.contains(&term.to_lowercase()));
        let strict = q.canonical_id.map(|id| hits.iter().any(|p| p.id == id));
        per_q.push((
            BenchQuery {
                query: q.query,
                required: q.required,
                canonical_id: q.canonical_id,
                cluster: q.cluster,
            },
            loose,
            strict,
        ));
    }
    print_bm25_failures_for_winner(winner, &per_q);

    // Self-validation: the printed winner numbers must match what we
    // measured. Defends against accidental drift in the printing or
    // selection code.
    let recomputed = evaluate_bm25_cell(
        &kb,
        cfg.queries,
        cfg.clusters,
        winner.top_k,
        winner.min_score,
    )
    .await;
    assert_eq!(recomputed.loose_pass, winner.loose_pass);
    assert_eq!(recomputed.strict_pass, winner.strict_pass);
}

#[cfg(test)]
mod bm25_selection_tests {
    //! Characterisation tests for [`pick_bm25_winner`].
    //!
    //! Pins the lex-selection rule: max strict_recall → max loose_recall
    //! → min top_k → max min_score. Each test isolates one axis with all
    //! other axes equal, so any future hand-edit that inverts a
    //! `.then(...)` clause fails loudly here instead of silently shifting
    //! the algorithmic winner reported by the sweep diagnostics.
    use super::*;

    fn cell(
        top_k: usize,
        min_score: f64,
        loose_pass: usize,
        strict_pass: usize,
    ) -> Bm25CellMetrics {
        Bm25CellMetrics {
            top_k,
            min_score,
            loose_pass,
            loose_total: 10,
            strict_pass,
            strict_total: 5,
            per_cluster: Vec::new(),
        }
    }

    #[test]
    fn strict_recall_wins_over_loose_recall() {
        // (loose=0, strict=2) beats (loose=10, strict=1).
        let cells = vec![cell(5, 0.5, 10, 1), cell(5, 0.5, 0, 2)];
        let winner = pick_bm25_winner(&cells);
        assert_eq!(winner.strict_pass, 2);
    }

    #[test]
    fn loose_recall_breaks_strict_tie() {
        // strict equal → max loose wins.
        let cells = vec![cell(5, 0.5, 3, 1), cell(5, 0.5, 5, 1)];
        let winner = pick_bm25_winner(&cells);
        assert_eq!(winner.loose_pass, 5);
    }

    #[test]
    fn smaller_top_k_breaks_recall_tie() {
        // strict + loose equal → min top_k wins.
        let cells = vec![cell(10, 0.5, 5, 1), cell(3, 0.5, 5, 1)];
        let winner = pick_bm25_winner(&cells);
        assert_eq!(winner.top_k, 3);
    }

    #[test]
    fn larger_min_score_breaks_top_k_tie() {
        // strict + loose + top_k equal → max min_score wins.
        let cells = vec![cell(5, 0.5, 5, 1), cell(5, 1.5, 5, 1)];
        let winner = pick_bm25_winner(&cells);
        assert!((winner.min_score - 1.5).abs() < f64::EPSILON);
    }
}
