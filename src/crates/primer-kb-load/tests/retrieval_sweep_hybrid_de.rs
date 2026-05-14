//! German hybrid (BM25 + dense-vector RRF) retrieval-parameter sweep.
//!
//! Parallels `retrieval_sweep_hybrid.rs` (English). Gated on
//! `--features fastembed` so the workspace default build doesn't pull
//! the fastembed-rs dependency. `#[ignore]`'d so even
//! `cargo test --features fastembed` skips it without `--ignored`.
//!
//! Run via:
//!
//! ```text
//! ~/.cargo/bin/cargo test -p primer-kb-load --features fastembed \
//!     --test retrieval_sweep_hybrid_de \
//!     -- --ignored sweep_retrieval_params_hybrid_de --nocapture
//! ```
//!
//! First run downloads BGE-M3 (~570 MB) into `~/.cache/primer/models/`.
//! Subsequent runs are warm. BGE-M3 is multilingual; the German queries
//! hit the dense leg with the same model the EN benchmark uses, so no
//! separate embedder configuration is needed.
//!
//! This is **infrastructure only** — it does not land hybrid defaults.
//! Today the production defaults are EN-tuned consts imported across
//! both locales.
//!
//! Implementation lives in `tests/common/sweep.rs` so the EN and DE
//! harnesses share one source of truth.

#![cfg(feature = "fastembed")]

mod common;

use common::de::QUERIES_DE;
use common::sweep::{HybridSweepConfig, run_hybrid_sweep};
use primer_core::i18n::Locale;

#[tokio::test]
#[ignore]
async fn sweep_retrieval_params_hybrid_de() {
    run_hybrid_sweep(HybridSweepConfig {
        locale: Locale::German,
        queries: QUERIES_DE,
        table_title: "DE hybrid retrieval-parameter sweep (Klexikon, BGE-M3)",
        winner_label: "NOTIONAL DE HYBRID WINNER (not landed)",
        closing_note: "Hybrid defaults remain unchanged — diagnostic only.",
    })
    .await;
}
