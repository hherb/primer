//! German retrieval-parameter sweep diagnostic (BM25-only).
//!
//! Parallels `retrieval_sweep.rs` (English). `#[ignore]`'d so it doesn't
//! run in default CI. Run via:
//!
//! ```text
//! ~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_sweep_de \
//!     -- --ignored sweep_retrieval_params_de --nocapture
//! ```
//!
//! Iterates the (top_k × min_score) grid against the in-repo Klexikon
//! corpus (~66 articles; CC-BY-SA-4.0) and the shared
//! `tests/common/de.rs` benchmark, prints a table of loose recall,
//! strict recall, and per-cluster recall per cell. Identifies the
//! winning cell algorithmically per the lex selection rule documented
//! in the EN sweep.
//!
//! The `Wiki` cluster is omitted: Klexikon IS the German wiki, so every
//! Klexikon passage collapses into its topical cluster (no parallel
//! hand-drafted seed sitting alongside it). This matches the
//! cluster-floor sanity test in `tests/common/de.rs`.
//!
//! Empirical shape against today's corpus: strict recall is 95% (one
//! miss in the space cluster, `"warum scheint die sonne so hell"`) at
//! `top_k=3` regardless of `min_score`, then flatlines at 100% from
//! `top_k=5` onward. Production `(top_k=5, min_score=0.5)` is a
//! comfortable choice — the EN-tuned defaults clear the German
//! benchmark unchanged.

mod common;

use common::de::QUERIES_DE;
use common::{BenchQuery, Cluster};
use primer_core::i18n::Locale;
use primer_core::knowledge::{KnowledgeBase, RetrievalParams};
use primer_kb_load::auto_seed_if_empty;
use primer_knowledge::SqliteKnowledgeBase;

const TOP_K_GRID: &[usize] = &[3, 5, 7, 10];
const MIN_SCORE_GRID: &[f64] = &[0.0, 0.25, 0.5, 0.75, 1.0, 1.5];

/// DE benchmark covers five non-Wiki clusters. The shared `Cluster`
/// enum's `Wiki` variant is unused for `de` (Klexikon IS the wiki).
const DE_CLUSTERS: usize = 5;

#[derive(Debug, Clone, Copy)]
struct CellMetrics {
    top_k: usize,
    min_score: f64,
    loose_pass: usize,
    loose_total: usize,
    strict_pass: usize,
    strict_total: usize,
    per_cluster: [(Cluster, usize, usize); DE_CLUSTERS], // (cluster, pass, total)
}

impl CellMetrics {
    fn loose_recall(&self) -> f64 {
        if self.loose_total == 0 {
            0.0
        } else {
            self.loose_pass as f64 / self.loose_total as f64
        }
    }
    fn strict_recall(&self) -> f64 {
        if self.strict_total == 0 {
            0.0
        } else {
            self.strict_pass as f64 / self.strict_total as f64
        }
    }
}

async fn evaluate_cell(
    kb: &SqliteKnowledgeBase,
    queries: &[BenchQuery],
    top_k: usize,
    min_score: f64,
) -> CellMetrics {
    let mut loose_pass = 0;
    let mut loose_total = 0;
    let mut strict_pass = 0;
    let mut strict_total = 0;
    let mut cluster_counts: [(Cluster, usize, usize); DE_CLUSTERS] = [
        (Cluster::Space, 0, 0),
        (Cluster::Body, 0, 0),
        (Cluster::HowThingsWork, 0, 0),
        (Cluster::Life, 0, 0),
        (Cluster::EarthWeather, 0, 0),
    ];

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
            let canonical_in_topk = hits.iter().any(|p| p.id == canonical);
            if canonical_in_topk {
                strict_pass += 1;
            }
        }
    }

    CellMetrics {
        top_k,
        min_score,
        loose_pass,
        loose_total,
        strict_pass,
        strict_total,
        per_cluster: cluster_counts,
    }
}

/// Lexicographic selection: maximise strict_recall, then loose_recall,
/// then minimise top_k, then maximise min_score. Same rule as the EN
/// sweep — production defaults are imported across both locales today.
/// See [docs/superpowers/specs/2026-05-06-retrieval-tuning-design.md].
fn pick_winner(cells: &[CellMetrics]) -> &CellMetrics {
    use std::cmp::Ordering;
    cells
        .iter()
        .max_by(|a, b| {
            // Defensive: NaN is unreachable on this grid (every total > 0)
            // but treat it as Equal rather than panicking, so a future
            // empty-corpus run prints the table instead of crashing.
            a.strict_recall()
                .partial_cmp(&b.strict_recall())
                .unwrap_or(Ordering::Equal)
                .then(
                    a.loose_recall()
                        .partial_cmp(&b.loose_recall())
                        .unwrap_or(Ordering::Equal),
                )
                .then(b.top_k.cmp(&a.top_k)) // smaller top_k wins → reverse
                .then(
                    a.min_score
                        .partial_cmp(&b.min_score)
                        .unwrap_or(Ordering::Equal),
                )
        })
        .expect("non-empty grid")
}

fn print_header() {
    println!("\n=== DE retrieval-parameter sweep (Klexikon, BM25-only) ===\n");
    println!(
        "{:>5} {:>10} {:>14} {:>15} | {:>5} {:>5} {:>5} {:>5} {:>5}",
        "top_k", "min_score", "loose_recall", "strict_recall", "Spc", "Bod", "How", "Lif", "Erw"
    );
    println!("{}", "-".repeat(90));
}

fn print_row(m: &CellMetrics) {
    let cluster_cell = |c: Cluster| -> String {
        let entry = m.per_cluster.iter().find(|e| e.0 == c).unwrap();
        if entry.2 == 0 {
            "-".to_string()
        } else {
            format!("{}/{}", entry.1, entry.2)
        }
    };
    println!(
        "{:>5} {:>10.2} {:>9}/{:>3}={:>3.0}% {:>9}/{:>3}={:>3.0}% | {:>5} {:>5} {:>5} {:>5} {:>5}",
        m.top_k,
        m.min_score,
        m.loose_pass,
        m.loose_total,
        m.loose_recall() * 100.0,
        m.strict_pass,
        m.strict_total,
        m.strict_recall() * 100.0,
        cluster_cell(Cluster::Space),
        cluster_cell(Cluster::Body),
        cluster_cell(Cluster::HowThingsWork),
        cluster_cell(Cluster::Life),
        cluster_cell(Cluster::EarthWeather),
    );
}

fn print_failures_for_winner(m: &CellMetrics, all: &[(BenchQuery, bool, Option<bool>)]) {
    println!(
        "\n=== Per-query results at winning cell (top_k={}, min_score={:.2}) ===",
        m.top_k, m.min_score
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

#[tokio::test]
#[ignore]
async fn sweep_retrieval_params_de() {
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::German).unwrap();
    auto_seed_if_empty(&kb, Locale::German)
        .await
        .unwrap()
        .unwrap();

    let mut cells: Vec<CellMetrics> = Vec::with_capacity(TOP_K_GRID.len() * MIN_SCORE_GRID.len());
    for &top_k in TOP_K_GRID {
        for &min_score in MIN_SCORE_GRID {
            cells.push(evaluate_cell(&kb, QUERIES_DE, top_k, min_score).await);
        }
    }

    print_header();
    for cell in &cells {
        print_row(cell);
    }

    let winner = pick_winner(&cells);
    println!(
        "\n>>> WINNING CELL: top_k={}, min_score={:.2} (strict_recall={:.0}%, loose_recall={:.0}%)",
        winner.top_k,
        winner.min_score,
        winner.strict_recall() * 100.0,
        winner.loose_recall() * 100.0,
    );

    // Per-query breakdown at the winning cell, so corpus gaps are obvious.
    let mut per_q: Vec<(BenchQuery, bool, Option<bool>)> = Vec::with_capacity(QUERIES_DE.len());
    for q in QUERIES_DE {
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
    print_failures_for_winner(winner, &per_q);

    // Self-validation: the printed winner numbers must match what we
    // measured. Defends against accidental drift in the printing or
    // selection code.
    let recomputed = evaluate_cell(&kb, QUERIES_DE, winner.top_k, winner.min_score).await;
    assert_eq!(recomputed.loose_pass, winner.loose_pass);
    assert_eq!(recomputed.strict_pass, winner.strict_pass);
}
