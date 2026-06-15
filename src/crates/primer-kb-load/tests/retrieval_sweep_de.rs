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
//! `tests/common/de.rs` benchmark. Same lex-selection rule as the EN
//! sweep — production defaults are imported across both locales today.
//!
//! The `Wiki` cluster is omitted: Klexikon IS the German wiki, so every
//! Klexikon passage collapses into its topical cluster (no parallel
//! hand-drafted seed sitting alongside it). This matches the
//! cluster-floor sanity test in `tests/common/de.rs`.
//!
//! Implementation lives in `tests/common/sweep/bm25.rs` so the EN and
//! DE harnesses share one source of truth.

mod common;

use common::Cluster;
use common::de::QUERIES_DE;
use common::sweep::bm25::{Bm25SweepConfig, run_bm25_sweep};
use primer_core::i18n::Locale;

/// Dashed-separator width tuned to the visible DE table footprint
/// (five cluster columns: Spc Bod How Lif Erw — no Wiki column).
/// Kept as an explicit const so the printed output is byte-identical
/// to the pre-refactor harness.
const DE_DASH_WIDTH: usize = 90;

const DE_CLUSTERS: [Cluster; 5] = [
    Cluster::Space,
    Cluster::Body,
    Cluster::HowThingsWork,
    Cluster::Life,
    Cluster::EarthWeather,
];

#[tokio::test]
#[ignore]
async fn sweep_retrieval_params_de() {
    run_bm25_sweep(Bm25SweepConfig {
        locale: Locale::German,
        queries: QUERIES_DE,
        clusters: &DE_CLUSTERS,
        table_title: "DE retrieval-parameter sweep (Klexikon, BM25-only)",
        dash_width: DE_DASH_WIDTH,
    })
    .await;
}
