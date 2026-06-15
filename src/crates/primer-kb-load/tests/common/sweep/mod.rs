//! Shared retrieval-parameter sweep harness.
//!
//! One implementation of the BM25 and hybrid sweep grids, parameterised
//! by locale, query dataset, and cluster set so each per-locale test
//! file (`retrieval_sweep{,_de,_hybrid,_hybrid_de}.rs`) shrinks to a thin
//! `#[ignore]` shim. Replaces four near-duplicate harnesses (~1000
//! lines) with one source of truth — adding a new locale (e.g. Hindi
//! `hi`) is now a data-only change.
//!
//! Split across two submodules to stay under the project's 500-line
//! file guideline (issue #98):
//! - [`bm25`] — BM25-only grid (`Bm25SweepConfig`, `run_bm25_sweep`).
//! - [`hybrid`] — BM25 + dense-vector RRF grid, gated on the `fastembed`
//!   feature (`HybridSweepConfig`, `run_hybrid_sweep`).
//!
//! Shims import the config + entry point from the relevant submodule
//! directly (`common::sweep::bm25::…` / `common::sweep::hybrid::…`) — no
//! re-exports, so a test binary that pulls in `mod common;` but uses
//! neither submodule never trips an `unused_imports` warning.
//!
//! Output is byte-identical to the pre-refactor harnesses on the same
//! corpus and benchmark; the printed table is the public contract.
//!
//! Lex-selection rule (both grids): maximise strict_recall, then
//! loose_recall, then minimise the final `top_k`, then prefer the
//! default tie-breaker (BM25: max `min_score`; hybrid: closest to
//! `rrf_k=60`). Every `.partial_cmp(...)` is `.unwrap_or(Ordering::Equal)`
//! so a future empty-corpus run prints the table instead of crashing.
//! See [docs/superpowers/specs/2026-05-06-retrieval-tuning-design.md].

pub mod bm25;

#[cfg(feature = "fastembed")]
pub mod hybrid;
