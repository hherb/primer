//! Hybrid (BM25 + dense-vector RRF) retrieval-parameter sweep.
//!
//! Gated on `--features fastembed` so the workspace default build
//! doesn't pull the fastembed-rs dependency. `#[ignore]`'d so even
//! `cargo test --features fastembed` skips it without `--ignored`.
//!
//! Run via:
//!
//! ```text
//! ~/.cargo/bin/cargo test -p primer-kb-load --features fastembed \
//!     --test retrieval_sweep_hybrid \
//!     -- --ignored sweep_retrieval_params_hybrid --nocapture
//! ```
//!
//! First run downloads BGE-M3 (~570 MB) into `~/.cache/primer/models/`.
//! Subsequent runs are warm.
//!
//! This is **infrastructure only** — it does not land hybrid defaults.
//! A follow-up session can run it, eyeball the table, and update
//! `KB_BM25_TOP_K`, `KB_VECTOR_TOP_K`, `RRF_K` if a clear winner emerges.

#![cfg(feature = "fastembed")]

mod common;

use common::{BenchQuery, Cluster, QUERIES};
use primer_core::embedder::Embedder;
use primer_core::i18n::Locale;
use primer_core::knowledge::HybridParams;
use primer_embedding::FastEmbedBackend;
use primer_kb_load::{auto_seed_if_empty, reembed_kb};
use primer_knowledge::SqliteKnowledgeBase;
use std::sync::Arc;

const BM25_TOP_K_GRID: &[usize] = &[10, 20, 30];
const VECTOR_TOP_K_GRID: &[usize] = &[10, 20, 30];
const FINAL_TOP_K_GRID: &[usize] = &[3, 5];
const RRF_K_GRID: &[f64] = &[30.0, 60.0, 90.0];

#[derive(Debug, Clone, Copy)]
struct HybridCellMetrics {
    bm25_top_k: usize,
    vector_top_k: usize,
    final_top_k: usize,
    rrf_k: f64,
    loose_pass: usize,
    loose_total: usize,
    strict_pass: usize,
    strict_total: usize,
}

impl HybridCellMetrics {
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

async fn evaluate_hybrid_cell(
    kb: &SqliteKnowledgeBase,
    embedder: &dyn Embedder,
    queries: &[BenchQuery],
    bm25_top_k: usize,
    vector_top_k: usize,
    final_top_k: usize,
    rrf_k: f64,
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
            min_score: 0.0,
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
        loose_pass,
        loose_total,
        strict_pass,
        strict_total,
    }
}

#[tokio::test]
#[ignore]
async fn sweep_retrieval_params_hybrid() {
    println!("\n=== hybrid retrieval-parameter sweep (BGE-M3) ===");
    println!(">>> first run downloads ~570 MB; subsequent runs are warm\n");

    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    auto_seed_if_empty(&kb, Locale::English)
        .await
        .unwrap()
        .unwrap();

    let embedder: Arc<dyn Embedder> =
        Arc::new(FastEmbedBackend::new().expect("BGE-M3 init failed"));
    // Backfill embeddings for the seed corpus so the vector leg has data.
    reembed_kb(&kb, embedder.as_ref(), false, 32).await.unwrap();

    println!(
        "{:>4} {:>4} {:>4} {:>5} {:>14} {:>15}",
        "BM25", "Vec", "Top", "RRF", "loose_recall", "strict_recall"
    );
    println!("{}", "-".repeat(70));

    let mut cells: Vec<HybridCellMetrics> = Vec::new();
    for &bm25_top_k in BM25_TOP_K_GRID {
        for &vector_top_k in VECTOR_TOP_K_GRID {
            for &final_top_k in FINAL_TOP_K_GRID {
                for &rrf_k in RRF_K_GRID {
                    let m = evaluate_hybrid_cell(
                        &kb,
                        embedder.as_ref(),
                        QUERIES,
                        bm25_top_k,
                        vector_top_k,
                        final_top_k,
                        rrf_k,
                    )
                    .await;
                    println!(
                        "{:>4} {:>4} {:>4} {:>5.0} {:>9}/{:>3}={:>3.0}% {:>9}/{:>3}={:>3.0}%",
                        m.bm25_top_k,
                        m.vector_top_k,
                        m.final_top_k,
                        m.rrf_k,
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

    // Pick the cell with max strict, max loose, min final_top_k, default
    // rrf_k=60 preference. Print, but DO NOT assert against — this is
    // scaffold; defaults don't change this session.
    let winner = cells
        .iter()
        .max_by(|a, b| {
            a.strict_recall()
                .partial_cmp(&b.strict_recall())
                .unwrap()
                .then(a.loose_recall().partial_cmp(&b.loose_recall()).unwrap())
                .then(b.final_top_k.cmp(&a.final_top_k))
                .then(
                    (a.rrf_k - 60.0)
                        .abs()
                        .partial_cmp(&(b.rrf_k - 60.0).abs())
                        .unwrap()
                        .reverse(),
                )
        })
        .unwrap();
    println!(
        "\n>>> NOTIONAL HYBRID WINNER (not landed): bm25_top_k={}, vector_top_k={}, final_top_k={}, rrf_k={:.0}",
        winner.bm25_top_k, winner.vector_top_k, winner.final_top_k, winner.rrf_k
    );
    println!(
        ">>> strict_recall={:.0}%, loose_recall={:.0}%",
        winner.strict_recall() * 100.0,
        winner.loose_recall() * 100.0,
    );
    println!(">>> Hybrid defaults remain unchanged this session — see plan Task 10.");

    // Avoid suppressing unused-arg warnings for Cluster.
    let _ = Cluster::Space;
}
