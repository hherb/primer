//! Retrieval-parameter sweep diagnostic (English, BM25-only).
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
//!
//! Production deliberately ships `min_score=0.5` (not the algorithmic
//! winner) because every cell at fixed `top_k` produced identical
//! recall — landing 1.50 would be brittle against a future corpus
//! expansion that dilutes BM25 scores. See the override rationale in
//! `consts::retrieval::KB_BM25_ONLY_MIN_SCORE`.
//!
//! Implementation lives in `tests/common/sweep.rs` so the EN and DE
//! harnesses share one source of truth.

mod common;

use common::Cluster;
use common::QUERIES;
use common::sweep::{Bm25SweepConfig, run_bm25_sweep};
use primer_core::i18n::Locale;

/// Dashed-separator width tuned to the visible EN table footprint
/// (six cluster columns: Spc Bod How Lif Erw Wik). Kept as an explicit
/// const so the printed output is byte-identical to the pre-refactor
/// harness on the same corpus.
const EN_DASH_WIDTH: usize = 96;

const EN_CLUSTERS: [Cluster; 6] = [
    Cluster::Space,
    Cluster::Body,
    Cluster::HowThingsWork,
    Cluster::Life,
    Cluster::EarthWeather,
    Cluster::Wiki,
];

#[tokio::test]
#[ignore]
async fn sweep_retrieval_params() {
    run_bm25_sweep(Bm25SweepConfig {
        locale: Locale::English,
        queries: QUERIES,
        clusters: &EN_CLUSTERS,
        table_title: "retrieval-parameter sweep",
        dash_width: EN_DASH_WIDTH,
    })
    .await;
}
