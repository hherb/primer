//! Regression guard for the shipped benchmark corpus.
//!
//! The benchmark examples are device/owner-gated, but the prompt corpus they
//! consume is plain data we validate on any host: it must parse, meet the
//! 30-prompt floor, and carry unique labels. The corpus is shared by the QNN
//! and llama.cpp benches, so this guard rides the DEFAULT `cargo test` via the
//! always-compiled [`primer_inference::bench`] loader.

use std::collections::HashSet;
use std::path::PathBuf;

use primer_inference::bench::{DEFAULT_BENCH_SYSTEM_PROMPT, load_bench_prompts};

/// 30 representative dialogue-continuation prompts (Phase 1.2 plan step 1.2.6
/// task 1). The corpus may grow but must not shrink below this floor.
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
