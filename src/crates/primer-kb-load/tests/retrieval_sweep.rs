//! Retrieval-parameter sweep diagnostic.
//!
//! `#[ignore]`'d so it doesn't run in default CI. Run via:
//!
//! ```text
//! ~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_sweep \
//!     -- --ignored sweep_retrieval_params --nocapture
//! ```
//!
//! Iterates the (top_k × min_score) grid against the in-repo English
//! seed corpus and the shared `tests/common` benchmark, prints a table
//! of loose recall, strict recall, and per-cluster recall per cell.
//! Identifies the winning cell algorithmically per the lex selection
//! rule (see the spec at
//! `docs/superpowers/specs/2026-05-06-retrieval-tuning-design.md`).

mod common;

use common::{BenchQuery, Cluster, QUERIES};
use primer_core::i18n::Locale;
use primer_core::knowledge::{KnowledgeBase, RetrievalParams};
use primer_kb_load::auto_seed_if_empty;
use primer_knowledge::SqliteKnowledgeBase;

const TOP_K_GRID: &[usize] = &[3, 5, 7, 10];
const MIN_SCORE_GRID: &[f64] = &[0.0, 0.25, 0.5, 0.75, 1.0, 1.5];

/// EN benchmark covers all six clusters (the five topical clusters plus
/// `Cluster::Wiki` for the Simple English Wikipedia layer). The
/// parallel `retrieval_sweep_de.rs` defines its own `DE_CLUSTERS = 5`
/// because Klexikon collapses into the topical clusters.
const EN_CLUSTERS: usize = 6;

#[derive(Debug, Clone, Copy)]
struct CellMetrics {
    top_k: usize,
    min_score: f64,
    loose_pass: usize,
    loose_total: usize,
    strict_pass: usize,
    strict_total: usize,
    per_cluster: [(Cluster, usize, usize); EN_CLUSTERS], // (cluster, pass, total)
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
    let mut cluster_counts: [(Cluster, usize, usize); EN_CLUSTERS] = [
        (Cluster::Space, 0, 0),
        (Cluster::Body, 0, 0),
        (Cluster::HowThingsWork, 0, 0),
        (Cluster::Life, 0, 0),
        (Cluster::EarthWeather, 0, 0),
        (Cluster::Wiki, 0, 0),
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
/// then minimise top_k, then maximise min_score.
///
/// Note: the algorithmic winner of this rule on the current 90-passage
/// corpus is `top_k=5, min_score=1.50`. Production deliberately ships
/// `min_score=0.5` because every cell at fixed `top_k` produced
/// identical recall — landing 1.50 would be brittle against a future
/// corpus expansion that dilutes BM25 scores. See the override
/// rationale in `consts::retrieval::KB_BM25_ONLY_MIN_SCORE` and the
/// design spec at
/// `docs/superpowers/specs/2026-05-06-retrieval-tuning-design.md`.
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
    println!("\n=== retrieval-parameter sweep ===\n");
    println!(
        "{:>5} {:>10} {:>14} {:>15} | {:>5} {:>5} {:>5} {:>5} {:>5} {:>5}",
        "top_k",
        "min_score",
        "loose_recall",
        "strict_recall",
        "Spc",
        "Bod",
        "How",
        "Lif",
        "Erw",
        "Wik"
    );
    println!("{}", "-".repeat(96));
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
        "{:>5} {:>10.2} {:>9}/{:>3}={:>3.0}% {:>9}/{:>3}={:>3.0}% | {:>5} {:>5} {:>5} {:>5} {:>5} {:>5}",
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
        cluster_cell(Cluster::Wiki),
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
async fn sweep_retrieval_params() {
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    auto_seed_if_empty(&kb, Locale::English)
        .await
        .unwrap()
        .unwrap();

    let mut cells: Vec<CellMetrics> = Vec::with_capacity(TOP_K_GRID.len() * MIN_SCORE_GRID.len());
    for &top_k in TOP_K_GRID {
        for &min_score in MIN_SCORE_GRID {
            cells.push(evaluate_cell(&kb, QUERIES, top_k, min_score).await);
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
    let mut per_q: Vec<(BenchQuery, bool, Option<bool>)> = Vec::with_capacity(QUERIES.len());
    for q in QUERIES {
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
    // measured. (Defends against accidental drift in the printing or
    // selection code.)
    let recomputed = evaluate_cell(&kb, QUERIES, winner.top_k, winner.min_score).await;
    assert_eq!(recomputed.loose_pass, winner.loose_pass);
    assert_eq!(recomputed.strict_pass, winner.strict_pass);
}
