//! Regression guard for the shipped QNN benchmark corpus.
//!
//! The benchmark example (`examples/qnn_bench.rs`) is a device-only test,
//! but the prompt corpus it consumes is plain data we can validate on any
//! host: it must parse, meet the Phase 1.2 plan's 30-prompt floor, and
//! carry unique labels. Gated on the `qnn` feature so it rides the same
//! `cargo test -p primer-inference --features qnn` invocation as the rest
//! of the QNN module.
#![cfg(feature = "qnn")]

use std::collections::HashSet;
use std::path::PathBuf;

use primer_inference::bench::{DEFAULT_BENCH_SYSTEM_PROMPT, load_bench_prompts};

/// Phase 1.2 plan step 1.2.6 task 1: "30 representative dialogue-continuation
/// prompts". The corpus may grow but must not shrink below this floor.
const MIN_CORPUS_PROMPTS: usize = 30;

fn corpus_path() -> PathBuf {
    // CARGO_MANIFEST_DIR = <repo>/src/crates/primer-inference.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../data/bench/socratic_prompts.jsonl")
}

#[test]
fn shipped_corpus_parses_and_meets_floor() {
    let path = corpus_path();
    let prompts = load_bench_prompts(&path, DEFAULT_BENCH_SYSTEM_PROMPT)
        .unwrap_or_else(|e| panic!("shipped corpus at {} failed to load: {e}", path.display()));
    assert!(
        prompts.len() >= MIN_CORPUS_PROMPTS,
        "expected >= {MIN_CORPUS_PROMPTS} prompts, got {}",
        prompts.len()
    );
}

#[test]
fn shipped_corpus_has_unique_labels() {
    let prompts = load_bench_prompts(&corpus_path(), DEFAULT_BENCH_SYSTEM_PROMPT).unwrap();
    let mut seen = HashSet::new();
    for p in &prompts {
        assert!(seen.insert(p.label.clone()), "duplicate label: {}", p.label);
    }
}
