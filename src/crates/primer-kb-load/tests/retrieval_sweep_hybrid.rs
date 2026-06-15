//! Hybrid (BM25 + dense-vector RRF) retrieval-parameter sweep (English).
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
//!
//! Implementation lives in `tests/common/sweep/hybrid.rs` so the EN and
//! DE harnesses share one source of truth.

#![cfg(feature = "fastembed")]

mod common;

use common::QUERIES;
use common::sweep::hybrid::{HybridSweepConfig, run_hybrid_sweep};
use primer_core::i18n::Locale;

#[tokio::test]
#[ignore]
async fn sweep_retrieval_params_hybrid() {
    run_hybrid_sweep(HybridSweepConfig {
        locale: Locale::English,
        queries: QUERIES,
        table_title: "hybrid retrieval-parameter sweep (BGE-M3)",
        winner_label: "NOTIONAL HYBRID WINNER (not landed)",
        closing_note: "Hybrid defaults remain unchanged this session — see plan Task 10.",
    })
    .await;
}
