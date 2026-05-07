# Retrieval-Parameter Tuning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Tune the BM25-only KB retrieval defaults (`KB_FINAL_TOP_K`, `KB_BM25_ONLY_MIN_SCORE`) against the now-90-passage seed corpus, ship a `#[ignore]`'d sweep harness, scaffold a hybrid-leg sweep behind `--features fastembed`, fold in issue #37 (Wikipedia ingest User-Agent contact), and close Phase 0.2.

**Architecture:** Three integration tests in `src/crates/primer-kb-load/tests/` share a `tests/common/mod.rs` module that holds `BenchQuery` + `Cluster` + the expanded `QUERIES` const. The regression test (`retrieval_quality.rs`) runs in CI against the production defaults. The sweep test (`retrieval_sweep.rs`) is `#[ignore]`'d, prints a 24-cell recall table, and computes the winning cell algorithmically per the lex selection rule. The hybrid sweep (`retrieval_sweep_hybrid.rs`) is gated on `--features fastembed --ignored` and serves as scaffold only — no hybrid defaults change this session. Consts in `primer_core::consts::retrieval` and `RetrievalParams::default()` move together to the chosen cell.

**Tech Stack:** Rust workspace edition 2024; cargo from `~/.cargo/bin/cargo` (rustup, not Homebrew rust); `tokio` async runtime; `tempfile` + `SqliteKnowledgeBase` for test KBs.

**Pre-flight:** All commands run from `/Users/hherb/src/primer/src` unless stated otherwise.

---

## Task 1: Set up `tests/common/mod.rs` with `BenchQuery` and `Cluster` (no expansion yet)

**Files:**
- Create: `src/crates/primer-kb-load/tests/common/mod.rs`
- Modify: `src/crates/primer-kb-load/tests/retrieval_quality.rs`

**Background:** The current `QUERIES` const is a tuple `(query, &[required], top_k)`. We're moving it into a `BenchQuery` struct in a shared `tests/common/mod.rs` so both the regression test and the new sweep test can import the same data. Cargo's pattern: `tests/common/mod.rs` is treated as a module (not a separate test binary) and accessed via `mod common;` in each consuming test file.

**No expansion or canonical_id annotation in this task** — pure data migration to keep the diff focused.

- [ ] **Step 1.1: Run baseline test, confirm green**

Run: `~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality`
Expected: 2 passed (the existing `seed_corpus_satisfies_canonical_queries` and `seed_corpus_registers_sources_for_attribution` tests).

- [ ] **Step 1.2: Create `tests/common/mod.rs` with the new shape**

Path: `src/crates/primer-kb-load/tests/common/mod.rs`

```rust
//! Shared benchmark dataset for retrieval-quality and retrieval-sweep
//! integration tests.
//!
//! `tests/common/mod.rs` is the standard Cargo pattern for test-shared
//! code: each test file uses `mod common;` to import these types and
//! data. This file is NOT a separate test binary.

#![allow(dead_code)] // not every BenchQuery field is read by every consumer

/// Cluster a benchmark query targets. Used for per-cluster recall
/// breakdown in the sweep diagnostic.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Cluster {
    Space,
    Body,
    HowThingsWork,
    Life,
    EarthWeather,
    Wiki,
}

/// One canonical child-question benchmark entry.
#[derive(Debug)]
pub struct BenchQuery {
    /// What a child might ask.
    pub query: &'static str,
    /// Loose-substring set: at least one passage in top-k must contain
    /// every term as a (case-insensitive) substring of its text.
    /// Fragments like "fus" match both "fusion" and "fusing".
    pub required: &'static [&'static str],
    /// If `Some`, this query is in the strict subset: the named passage
    /// id must appear in the returned hits' ids. Used by both the
    /// regression test and the sweep's strict-recall metric.
    pub canonical_id: Option<&'static str>,
    /// Cluster this query targets, for per-cluster diagnostics.
    pub cluster: Cluster,
}

/// Canonical benchmark queries. Migrated from the inline `QUERIES`
/// const that previously lived in `retrieval_quality.rs`.
///
/// Expanded in Task 2 with paraphrases, cross-cluster questions, and
/// additional wiki queries. Annotated with `canonical_id` in Task 3.
pub const QUERIES: &[BenchQuery] = &[
    // Space cluster
    BenchQuery { query: "how does the sun shine", required: &["fus", "hydrogen"], canonical_id: None, cluster: Cluster::Space },
    BenchQuery { query: "why does the moon change shape", required: &["phase", "sunlight"], canonical_id: None, cluster: Cluster::Space },
    BenchQuery { query: "why is it day and night", required: &["spin", "axis"], canonical_id: None, cluster: Cluster::Space },
    BenchQuery { query: "how big is jupiter", required: &["jupiter"], canonical_id: None, cluster: Cluster::Space },
    BenchQuery { query: "what is a star made of", required: &["hydrogen", "helium"], canonical_id: None, cluster: Cluster::Space },
    BenchQuery { query: "what is the milky way", required: &["galax", "spiral"], canonical_id: None, cluster: Cluster::Space },
    BenchQuery { query: "what happens inside a black hole", required: &["gravity", "event horizon"], canonical_id: None, cluster: Cluster::Space },
    BenchQuery { query: "what is a shooting star", required: &["meteor", "atmosphere"], canonical_id: None, cluster: Cluster::Space },
    BenchQuery { query: "why do eclipses happen", required: &["shadow", "moon"], canonical_id: None, cluster: Cluster::Space },
    BenchQuery { query: "what causes ocean tides", required: &["moon", "gravity"], canonical_id: None, cluster: Cluster::Space },
    BenchQuery { query: "how did the universe begin", required: &["big bang", "expand"], canonical_id: None, cluster: Cluster::Space },
    BenchQuery { query: "why are there seasons", required: &["tilt", "axis"], canonical_id: None, cluster: Cluster::Space },
    // Body cluster
    BenchQuery { query: "how does the heart pump blood", required: &["heart", "ventric"], canonical_id: None, cluster: Cluster::Body },
    BenchQuery { query: "why do we breathe", required: &["oxygen", "alveoli"], canonical_id: None, cluster: Cluster::Body },
    BenchQuery { query: "what does the brain do", required: &["neuron", "cortex"], canonical_id: None, cluster: Cluster::Body },
    BenchQuery { query: "how does food get digested", required: &["stomach", "intestine"], canonical_id: None, cluster: Cluster::Body },
    BenchQuery { query: "how many bones are in the body", required: &["skeleton", "206"], canonical_id: None, cluster: Cluster::Body },
    BenchQuery { query: "how do muscles move", required: &["contract", "fibre"], canonical_id: None, cluster: Cluster::Body },
    BenchQuery { query: "what is skin made of", required: &["epidermis", "dermis"], canonical_id: None, cluster: Cluster::Body },
    BenchQuery { query: "how does the body fight germs", required: &["immune", "antibod"], canonical_id: None, cluster: Cluster::Body },
    BenchQuery { query: "how do eyes see", required: &["retina", "lens"], canonical_id: None, cluster: Cluster::Body },
    BenchQuery { query: "how do ears hear", required: &["cochlea", "vibrat"], canonical_id: None, cluster: Cluster::Body },
    BenchQuery { query: "why do we sleep and dream", required: &["rem", "memor"], canonical_id: None, cluster: Cluster::Body },
    // How-things-work cluster
    BenchQuery { query: "how do magnets work", required: &["magnetic field", "pole"], canonical_id: None, cluster: Cluster::HowThingsWork },
    BenchQuery { query: "what is electricity", required: &["electron", "circuit"], canonical_id: None, cluster: Cluster::HowThingsWork },
    BenchQuery { query: "how does a battery store energy", required: &["chemical", "anode"], canonical_id: None, cluster: Cluster::HowThingsWork },
    BenchQuery { query: "how does a lever work", required: &["fulcrum", "force"], canonical_id: None, cluster: Cluster::HowThingsWork },
    BenchQuery { query: "what makes sound", required: &["vibrat", "wave"], canonical_id: None, cluster: Cluster::HowThingsWork },
    BenchQuery { query: "speed of light photons", required: &["photon", "wavelength"], canonical_id: None, cluster: Cluster::HowThingsWork },
    BenchQuery { query: "how does a mirror work", required: &["reflect", "angle"], canonical_id: None, cluster: Cluster::HowThingsWork },
    BenchQuery { query: "how does fire burn", required: &["oxygen", "fuel"], canonical_id: None, cluster: Cluster::HowThingsWork },
    BenchQuery { query: "why does ice float", required: &["density", "molecule"], canonical_id: None, cluster: Cluster::HowThingsWork },
    BenchQuery { query: "why does salt melt ice", required: &["freezing", "sodium"], canonical_id: None, cluster: Cluster::HowThingsWork },
    BenchQuery { query: "how does soap clean", required: &["oil", "molecule"], canonical_id: None, cluster: Cluster::HowThingsWork },
    // Life cluster
    BenchQuery { query: "how do plants make food", required: &["photosynthesis", "chlorophyll"], canonical_id: None, cluster: Cluster::Life },
    BenchQuery { query: "how do seeds sprout germinate", required: &["germinat", "root"], canonical_id: None, cluster: Cluster::Life },
    BenchQuery { query: "what is a food chain", required: &["producer", "consumer"], canonical_id: None, cluster: Cluster::Life },
    BenchQuery { query: "what are cells", required: &["cell", "nucleus"], canonical_id: None, cluster: Cluster::Life },
    BenchQuery { query: "how does evolution work", required: &["natural selection", "dna"], canonical_id: None, cluster: Cluster::Life },
    BenchQuery { query: "what happened to the dinosaurs", required: &["asteroid", "extinct"], canonical_id: None, cluster: Cluster::Life },
    BenchQuery { query: "what makes a mammal a mammal", required: &["fur", "milk"], canonical_id: None, cluster: Cluster::Life },
    BenchQuery { query: "how do birds fly", required: &["feather", "wing"], canonical_id: None, cluster: Cluster::Life },
    BenchQuery { query: "what is a frog", required: &["amphibian", "tadpole"], canonical_id: None, cluster: Cluster::Life },
    BenchQuery { query: "how many legs does an insect have", required: &["six", "thorax"], canonical_id: None, cluster: Cluster::Life },
    BenchQuery { query: "where do babies come from", required: &["egg", "sperm"], canonical_id: None, cluster: Cluster::Life },
    // Earth and weather cluster
    BenchQuery { query: "what makes rain fall from clouds", required: &["droplet", "condens"], canonical_id: None, cluster: Cluster::EarthWeather },
    BenchQuery { query: "how do snowflakes form six arms", required: &["hexagon", "crystal"], canonical_id: None, cluster: Cluster::EarthWeather },
    BenchQuery { query: "why are clouds white or grey", required: &["scatter", "droplet"], canonical_id: None, cluster: Cluster::EarthWeather },
    BenchQuery { query: "what causes wind to blow", required: &["pressure", "air"], canonical_id: None, cluster: Cluster::EarthWeather },
    BenchQuery { query: "why does lightning make thunder", required: &["spark", "shock"], canonical_id: None, cluster: Cluster::EarthWeather },
    BenchQuery { query: "why do rainbows have colours", required: &["refract", "wavelength"], canonical_id: None, cluster: Cluster::EarthWeather },
    BenchQuery { query: "water cycle evaporation precipitation", required: &["evapora", "condens"], canonical_id: None, cluster: Cluster::EarthWeather },
    BenchQuery { query: "how do volcanoes erupt magma", required: &["magma", "tectonic"], canonical_id: None, cluster: Cluster::EarthWeather },
    BenchQuery { query: "what causes earthquakes faults", required: &["fault", "tectonic"], canonical_id: None, cluster: Cluster::EarthWeather },
    BenchQuery { query: "how do mountains form collide", required: &["plate", "collid"], canonical_id: None, cluster: Cluster::EarthWeather },
    BenchQuery { query: "difference between climate and weather", required: &["climate", "average"], canonical_id: None, cluster: Cluster::EarthWeather },
    // Wikipedia layer (Phase 0.2 MVP)
    BenchQuery { query: "what is gravity", required: &["gravity", "attraction"], canonical_id: None, cluster: Cluster::Wiki },
    BenchQuery { query: "what is an atom", required: &["atom", "matter"], canonical_id: None, cluster: Cluster::Wiki },
    BenchQuery { query: "what is a virus", required: &["virus", "parasite"], canonical_id: None, cluster: Cluster::Wiki },
    BenchQuery { query: "what is climate change", required: &["climate", "temperature"], canonical_id: None, cluster: Cluster::Wiki },
    BenchQuery { query: "what is DNA", required: &["dna", "genetic"], canonical_id: None, cluster: Cluster::Wiki },
    BenchQuery { query: "how do vaccines work", required: &["vaccine"], canonical_id: None, cluster: Cluster::Wiki },
    BenchQuery { query: "what is friction", required: &["friction", "force"], canonical_id: None, cluster: Cluster::Wiki },
    BenchQuery { query: "what makes a plant a plant", required: &["plant", "autotroph"], canonical_id: None, cluster: Cluster::Wiki },
];
```

- [ ] **Step 1.3: Update `retrieval_quality.rs` to consume the shared module**

Path: `src/crates/primer-kb-load/tests/retrieval_quality.rs`

Replace the entire file contents with:

```rust
//! Retrieval-quality integration test.
//!
//! Loads the in-repo English seed corpus into a temp DB and asserts
//! that canonical child queries surface passages mentioning the
//! expected key terms. The benchmark dataset lives in
//! `tests/common/mod.rs` so the diagnostic sweep
//! (`tests/retrieval_sweep.rs`) shares it.
//!
//! This is the truth signal for the production
//! `KB_FINAL_TOP_K` / `KB_BM25_ONLY_MIN_SCORE` defaults — if a query
//! needs a higher `top_k` to hit, that's a tuning signal surfaced by
//! the sweep.

mod common;

use common::QUERIES;
use primer_core::i18n::Locale;
use primer_core::knowledge::{KnowledgeBase, RetrievalParams};
use primer_kb_load::auto_seed_if_empty;
use primer_knowledge::SqliteKnowledgeBase;

/// Top-K used by the regression test. Mirrors the dialogue manager's
/// realistic retrieval depth once `RetrievalParams::default()` is
/// tuned (Task 6 of the retrieval-tuning plan replaces this constant
/// with `primer_core::consts::retrieval::KB_FINAL_TOP_K`).
const REGRESSION_TOP_K: usize = 5;

#[tokio::test]
async fn seed_corpus_satisfies_canonical_queries() {
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    let stats = auto_seed_if_empty(&kb, Locale::English)
        .await
        .unwrap()
        .expect("seed dir must contain at least one *.en.jsonl");
    assert!(
        stats.inserted >= 10,
        "seed corpus must contain at least 10 passages, got {}",
        stats.inserted
    );

    let mut failures = Vec::new();
    for q in QUERIES {
        let params = RetrievalParams {
            top_k: REGRESSION_TOP_K,
            min_score: f64::NEG_INFINITY,
            source_filter: vec![],
        };
        let hits = kb.retrieve(q.query, &params).await.unwrap();
        let combined = hits
            .iter()
            .map(|p| p.text.to_lowercase())
            .collect::<Vec<_>>()
            .join(" \n ");
        let missing: Vec<_> = q
            .required
            .iter()
            .filter(|term| !combined.contains(&term.to_lowercase()))
            .collect();
        if !missing.is_empty() {
            failures.push(format!(
                "query {:?}: top {} did not contain required term(s) {:?}; got ids {:?}",
                q.query,
                REGRESSION_TOP_K,
                missing,
                hits.iter().map(|p| &p.id).collect::<Vec<_>>(),
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "retrieval-quality regressions:\n  - {}",
        failures.join("\n  - ")
    );
}

#[tokio::test]
async fn seed_corpus_registers_sources_for_attribution() {
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    let stats = auto_seed_if_empty(&kb, Locale::English)
        .await
        .unwrap()
        .expect("seed dir must contain at least one *.en.jsonl");
    assert!(
        stats.sources_seen >= 2,
        "expected sources from at least two seed files, got {}",
        stats.sources_seen
    );

    let sources = kb.list_sources().await.unwrap();
    assert!(
        !sources.is_empty(),
        "every passage must register its source"
    );
    let mut seen_cc0 = false;
    let mut seen_cc_by_sa = false;
    for src in &sources {
        assert!(!src.license.is_empty(), "source {} missing license", src.id);
        assert!(
            !src.attribution.is_empty(),
            "source {} missing attribution",
            src.id
        );
        if src.license == "CC0-1.0" {
            seen_cc0 = true;
        }
        if src.license == "CC-BY-SA-3.0" {
            seen_cc_by_sa = true;
        }
    }
    assert!(seen_cc0, "expected at least one CC0-1.0 source (seed corpus)");
    assert!(seen_cc_by_sa, "expected at least one CC-BY-SA-3.0 source (wiki layer)");
}
```

- [ ] **Step 1.4: Run the regression test, expect green**

Run: `~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality`
Expected: `2 passed; 0 failed`

If a query that previously passed now fails, it means the migration introduced a bug — likely a typo in a `BenchQuery` field. Fix and re-run.

- [ ] **Step 1.5: Run the full workspace tests, expect 550 passed**

Run: `~/.cargo/bin/cargo test --workspace 2>&1 | grep -E "^test result" | awk '{n+=$4; f+=$6} END {printf "Total: %d passed, %d failed\n", n, f}'`
Expected: `Total: 550 passed, 0 failed`

- [ ] **Step 1.6: Commit**

```bash
git add src/crates/primer-kb-load/tests/common/mod.rs src/crates/primer-kb-load/tests/retrieval_quality.rs
git commit -m "$(cat <<'EOF'
test(retrieval): migrate QUERIES to BenchQuery struct in shared common module

Pure data migration: the inline tuple-shape QUERIES const in
retrieval_quality.rs becomes a `&[BenchQuery]` const in
tests/common/mod.rs so the upcoming retrieval_sweep.rs (#[ignore]'d
diagnostic) and retrieval_sweep_hybrid.rs (--features fastembed)
can share the same dataset. No expansion or canonical_id annotation
yet — those land in subsequent tasks.

The regression test now reads queries from common::QUERIES and
asserts the same loose-substring contract as before, against the
same top_k=5 / min_score=NEG_INFINITY parameters. Behaviour is
preserved by construction.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Expand benchmark from ~58 to ~85 queries

**Files:**
- Modify: `src/crates/primer-kb-load/tests/common/mod.rs`

**Background:** Add three categories of new queries to reduce overfit risk: child-language paraphrases (vocabulary drift), cross-cluster questions (topic blending), and additional wiki-only queries (broader coverage). All new queries follow the same `BenchQuery` shape; `canonical_id` stays `None` (set in Task 3).

- [ ] **Step 2.1: Append the new queries to `QUERIES` in `tests/common/mod.rs`**

Append the following block immediately before the closing `];` of `QUERIES`:

```rust
    // ----- Child-language paraphrases (Task 2 expansion) -----
    BenchQuery { query: "why does my heart go thump thump", required: &["heart", "ventric"], canonical_id: None, cluster: Cluster::Body },
    BenchQuery { query: "what makes my tummy growl when I am hungry", required: &["stomach", "intestine"], canonical_id: None, cluster: Cluster::Body },
    BenchQuery { query: "why are bones hard but bendy", required: &["skeleton", "206"], canonical_id: None, cluster: Cluster::Body },
    BenchQuery { query: "where does the rain come from", required: &["droplet", "condens"], canonical_id: None, cluster: Cluster::EarthWeather },
    BenchQuery { query: "why is the sky blue and not green", required: &["scatter", "wavelength"], canonical_id: None, cluster: Cluster::HowThingsWork },
    BenchQuery { query: "why do volcanoes get angry and erupt", required: &["magma", "tectonic"], canonical_id: None, cluster: Cluster::EarthWeather },
    BenchQuery { query: "how does my dog see in the dark", required: &["retina", "lens"], canonical_id: None, cluster: Cluster::Body },
    BenchQuery { query: "what is inside a tiny bug", required: &["six", "thorax"], canonical_id: None, cluster: Cluster::Life },
    BenchQuery { query: "why do flowers smell nice", required: &["plant", "autotroph"], canonical_id: None, cluster: Cluster::Wiki },
    BenchQuery { query: "what makes lightning so bright", required: &["spark", "shock"], canonical_id: None, cluster: Cluster::EarthWeather },
    // ----- Cross-cluster questions (Task 2 expansion) -----
    BenchQuery { query: "why do stars twinkle but planets do not", required: &["atmosphere"], canonical_id: None, cluster: Cluster::Space },
    BenchQuery { query: "how does the moon make the sea move", required: &["moon", "gravity"], canonical_id: None, cluster: Cluster::Space },
    BenchQuery { query: "why does the sun help plants grow", required: &["photosynthesis", "chlorophyll"], canonical_id: None, cluster: Cluster::Life },
    BenchQuery { query: "how do volcanoes change the weather", required: &["magma"], canonical_id: None, cluster: Cluster::EarthWeather },
    BenchQuery { query: "why does the brain need oxygen from the lungs", required: &["neuron"], canonical_id: None, cluster: Cluster::Body },
    BenchQuery { query: "what makes a baby grow inside its mum", required: &["egg", "sperm"], canonical_id: None, cluster: Cluster::Life },
    BenchQuery { query: "how does electricity travel through wires made of metal", required: &["electron"], canonical_id: None, cluster: Cluster::HowThingsWork },
    BenchQuery { query: "why does ice cool a drink", required: &["density", "molecule"], canonical_id: None, cluster: Cluster::HowThingsWork },
    BenchQuery { query: "how do mountains and earthquakes both come from plates moving", required: &["plate"], canonical_id: None, cluster: Cluster::EarthWeather },
    BenchQuery { query: "why do mammals breathe air but fish breathe in water", required: &["oxygen"], canonical_id: None, cluster: Cluster::Body },
    // ----- Additional wiki queries (Task 2 expansion) -----
    BenchQuery { query: "what is a chemical element", required: &["element", "atom"], canonical_id: None, cluster: Cluster::Wiki },
    BenchQuery { query: "what is a molecule", required: &["molecule", "atom"], canonical_id: None, cluster: Cluster::Wiki },
    BenchQuery { query: "what is a gene", required: &["gene"], canonical_id: None, cluster: Cluster::Wiki },
    BenchQuery { query: "what is bacteria", required: &["bacteria"], canonical_id: None, cluster: Cluster::Wiki },
    BenchQuery { query: "what is the ocean", required: &["ocean"], canonical_id: None, cluster: Cluster::Wiki },
    BenchQuery { query: "what is an ecosystem", required: &["ecosystem"], canonical_id: None, cluster: Cluster::Wiki },
    BenchQuery { query: "what is soil", required: &["soil"], canonical_id: None, cluster: Cluster::Wiki },
```

- [ ] **Step 2.2: Add a sanity test in `tests/common/mod.rs`**

Append at the bottom of `tests/common/mod.rs`:

```rust
#[cfg(test)]
mod sanity_tests {
    use super::*;

    #[test]
    fn queries_have_expected_size_floor() {
        // Defensive lower bound — catches accidental truncation.
        assert!(
            QUERIES.len() >= 80,
            "expected at least 80 benchmark queries, got {}",
            QUERIES.len()
        );
    }

    #[test]
    fn every_cluster_has_at_least_three_queries() {
        for cluster in [
            Cluster::Space,
            Cluster::Body,
            Cluster::HowThingsWork,
            Cluster::Life,
            Cluster::EarthWeather,
            Cluster::Wiki,
        ] {
            let count = QUERIES.iter().filter(|q| q.cluster == cluster).count();
            assert!(
                count >= 3,
                "cluster {:?} has only {} queries; need at least 3 for per-cluster recall to be meaningful",
                cluster, count
            );
        }
    }
}
```

Note: `tests/common/mod.rs` is included as a module by each test file via `mod common;`, so `#[cfg(test)]` blocks inside it run as part of every test file that imports the module. The sanity tests above will appear under both `retrieval_quality` and (later) `retrieval_sweep` test binaries — that's fine; they're cheap and the duplication makes sanity always run.

- [ ] **Step 2.3: Run regression + sanity tests, expect green**

Run: `~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality`
Expected: 4 passed (the original 2 plus the 2 new sanity tests, executed inside the `retrieval_quality` test binary).

If any *expanded* query fails the regression assertion, that means the corpus doesn't cover it well enough at `top_k=5, min_score=NEG_INFINITY`. Don't tighten the regression yet — note the failures; the sweep in Task 4 will quantify what's reachable.

If failures look like genuine corpus gaps (no passage in either seed file mentions the required terms even fuzzily), reword the query's `required` to terms that DO appear, OR remove the query. Document in the commit message which queries you adjusted and why.

- [ ] **Step 2.4: Commit**

```bash
git add src/crates/primer-kb-load/tests/common/mod.rs
git commit -m "$(cat <<'EOF'
test(retrieval): expand benchmark to ~85 queries with paraphrases and cross-cluster coverage

Adds three categories of new queries to reduce overfitting risk in
the upcoming default-tuning sweep:

- ~10 child-language paraphrases of existing concepts (vocabulary
  drift: "thump thump", "tummy growl", "twinkle")
- ~10 cross-cluster questions (topic blending: stars twinkle vs
  atmosphere, moon and tides, plants and sun)
- ~7 additional wiki-only queries beyond the original 8 (drilling
  into already-ingested wiki articles not yet exercised)

Two sanity tests guard size + per-cluster coverage so future
expansion can't accidentally drop coverage. canonical_id stays None
across new queries; the strict subset is annotated in the next
task.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Annotate strict-subset queries with `canonical_id`

**Files:**
- Modify: `src/crates/primer-kb-load/tests/common/mod.rs`

**Background:** The strict-subset is the set of queries with an unambiguous "right" passage. We hand-label ~15-20 entries. These power the strict-recall metric in the sweep, which is the primary signal for choosing defaults. All ids below come from `data/seed/seed_passages.en.jsonl` and `data/seed/wiki_passages.en.jsonl` (verified via `jq -r '.id'` — see plan pre-flight notes).

- [ ] **Step 3.1: Update strict-subset entries in `QUERIES`**

For each `BenchQuery` listed below, change `canonical_id: None` to `canonical_id: Some("<id>")`. Match by the `query:` field.

**Wiki-layer (8 entries, all from the existing wiki block):**

```text
"what is gravity"                    → Some("wiki-simple:en:gravity")
"what is an atom"                    → Some("wiki-simple:en:atom")
"what is a virus"                    → Some("wiki-simple:en:virus")
"what is climate change"             → Some("wiki-simple:en:climate-change")
"what is DNA"                        → Some("wiki-simple:en:dna")
"how do vaccines work"               → Some("wiki-simple:en:vaccine")
"what is friction"                   → Some("wiki-simple:en:friction")
"what makes a plant a plant"         → Some("wiki-simple:en:plant")
```

**Wiki-layer (additional, from Task 2 expansion):**

```text
"what is a chemical element"         → Some("wiki-simple:en:chemical-element")
"what is a molecule"                 → Some("wiki-simple:en:molecule")
"what is a gene"                     → Some("wiki-simple:en:gene")
"what is bacteria"                   → Some("wiki-simple:en:bacteria")
"what is the ocean"                  → Some("wiki-simple:en:ocean")
"what is an ecosystem"               → Some("wiki-simple:en:ecosystem")
"what is soil"                       → Some("wiki-simple:en:soil")
```

**Hand-drafted seed (5 entries, picked for unambiguity):**

```text
"how does the sun shine"             → Some("seed:en:sun")
"how does the heart pump blood"      → Some("seed:en:heart")
"how do plants make food"            → Some("seed:en:photosynthesis")
"why are there seasons"              → Some("seed:en:earth-orbit-day-night")
"how do magnets work"                → Some("seed:en:magnets")
```

Total strict subset: 20 entries.

- [ ] **Step 3.2: Add a sanity test asserting strict-subset shape**

Append to the `sanity_tests` mod in `tests/common/mod.rs`:

```rust
    #[test]
    fn strict_subset_meets_floor() {
        let strict_count = QUERIES.iter().filter(|q| q.canonical_id.is_some()).count();
        assert!(
            strict_count >= 15,
            "strict subset has only {} entries; need at least 15 for the sweep's strict-recall signal",
            strict_count
        );
    }

    #[tokio::test]
    async fn strict_subset_canonical_ids_exist_in_corpus() {
        // Defensive: every canonical_id we declare must exist in the
        // shipped seed corpus, otherwise the strict-recall metric is
        // measuring against ghost ids. We open a temp KB, seed it, and
        // probe each canonical_id.
        use primer_core::i18n::Locale;
        use primer_core::knowledge::{KnowledgeBase, RetrievalParams};
        use primer_kb_load::auto_seed_if_empty;
        use primer_knowledge::SqliteKnowledgeBase;

        let db = tempfile::NamedTempFile::new().unwrap();
        let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
        auto_seed_if_empty(&kb, Locale::English).await.unwrap().unwrap();

        for q in QUERIES.iter().filter_map(|q| q.canonical_id.map(|id| (q.query, id))) {
            // Probe with the id as the query — if it exists, BM25 will
            // return it at top of a wide pull. Use a generous top_k.
            let params = RetrievalParams { top_k: 50, min_score: f64::NEG_INFINITY, source_filter: vec![] };
            let hits = kb.retrieve(q.1, &params).await.unwrap();
            let found = hits.iter().any(|p| p.id == q.1);
            assert!(found, "canonical_id {:?} for query {:?} not found in seed corpus", q.1, q.0);
        }
    }
```

You'll need `primer-kb-load`, `primer-knowledge`, `tempfile` accessible — they're already in `[dev-dependencies]` of `primer-kb-load` so the imports compile.

- [ ] **Step 3.3: Run, expect green**

Run: `~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality`
Expected: All sanity tests pass, including the new `strict_subset_canonical_ids_exist_in_corpus`.

If a canonical_id assertion fails, the id was typoed — recheck against `jq -r '.id' data/seed/*.jsonl`.

- [ ] **Step 3.4: Commit**

```bash
git add src/crates/primer-kb-load/tests/common/mod.rs
git commit -m "$(cat <<'EOF'
test(retrieval): annotate 20 strict-subset queries with canonical_id

Each strict-subset BenchQuery names the passage id that retrieval
must place in top-k for the strict-recall metric to credit it. Mix:
15 wiki-layer queries (each maps cleanly to a single Simple
Wikipedia article) + 5 hand-drafted seed queries with unambiguous
right answers (sun, heart, photosynthesis, seasons, magnets).

A new defensive sanity test seeds a temp KB and asserts every
declared canonical_id exists in the shipped corpus, so a future id
rename in seed_passages.en.jsonl or wiki_passages.en.jsonl that
forgets to update this list fails CI loudly.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Write `retrieval_sweep.rs` — the diagnostic sweep

**Files:**
- Create: `src/crates/primer-kb-load/tests/retrieval_sweep.rs`

**Background:** The sweep iterates the `(top_k × min_score)` grid (24 cells), evaluates loose + strict + per-cluster recall per cell, prints a table, and identifies the winning cell per the lex selection rule. `#[ignore]`'d so it doesn't run in default CI. Run via `cargo test -- --ignored sweep_retrieval_params --nocapture`.

The sweep test self-validates: it asserts that the winning cell achieves the recall it claims. Other than that it has no production-truth assertion — its purpose is to print numbers for tuning.

- [ ] **Step 4.1: Create the sweep test file**

Path: `src/crates/primer-kb-load/tests/retrieval_sweep.rs`

```rust
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

#[derive(Debug, Clone, Copy)]
struct CellMetrics {
    top_k: usize,
    min_score: f64,
    loose_pass: usize,
    loose_total: usize,
    strict_pass: usize,
    strict_total: usize,
    per_cluster: [(Cluster, usize, usize); 6], // (cluster, pass, total)
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
    let mut cluster_counts: [(Cluster, usize, usize); 6] = [
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
fn pick_winner(cells: &[CellMetrics]) -> &CellMetrics {
    cells
        .iter()
        .max_by(|a, b| {
            a.strict_recall()
                .partial_cmp(&b.strict_recall())
                .unwrap()
                .then(a.loose_recall().partial_cmp(&b.loose_recall()).unwrap())
                .then(b.top_k.cmp(&a.top_k)) // smaller top_k wins → reverse
                .then(a.min_score.partial_cmp(&b.min_score).unwrap())
        })
        .expect("non-empty grid")
}

fn print_header() {
    println!("\n=== retrieval-parameter sweep ===\n");
    println!(
        "{:>5} {:>10} {:>14} {:>15} | {:>5} {:>5} {:>5} {:>5} {:>5} {:>5}",
        "top_k", "min_score", "loose_recall", "strict_recall",
        "Spc", "Bod", "How", "Lif", "Erw", "Wik"
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
    println!("\n=== Per-query results at winning cell (top_k={}, min_score={:.2}) ===", m.top_k, m.min_score);
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
    auto_seed_if_empty(&kb, Locale::English).await.unwrap().unwrap();

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
        let strict = q
            .canonical_id
            .map(|id| hits.iter().any(|p| p.id == id));
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
```

- [ ] **Step 4.2: Run the sweep, capture the table**

Run: `~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_sweep -- --ignored sweep_retrieval_params --nocapture`
Expected: a 24-row table prints, followed by `>>> WINNING CELL: top_k=X, min_score=Y` and a per-query breakdown.

**Capture the full output to a file** for the next task:

```bash
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_sweep -- --ignored sweep_retrieval_params --nocapture 2>&1 | tee /tmp/sweep_output.txt
```

The expected runtime is 1-3 minutes (24 cells × 85 queries × BM25 lookup against ~90 passages).

- [ ] **Step 4.3: Verify the cargo test exit code is 0**

Run: `echo $?` (immediately after the previous command)
Expected: `0`. If non-zero, the self-validation assertion at the bottom of the test failed — investigate before proceeding.

- [ ] **Step 4.4: Commit**

```bash
git add src/crates/primer-kb-load/tests/retrieval_sweep.rs
git commit -m "$(cat <<'EOF'
test(retrieval): add #[ignore]'d sweep diagnostic over (top_k × min_score) grid

Iterates the 24-cell parameter grid against the in-repo English
seed corpus and the shared common::QUERIES benchmark. Per cell:
loose recall, strict recall (via canonical_id), per-cluster
recall. Prints a table to stdout (capture via --nocapture) and
identifies the winning cell algorithmically per the lex selection
rule (max strict, max loose, min top_k, max min_score).

The test self-validates by re-evaluating the winning cell at the
end and asserting the metrics match what was printed. Default CI
skips it via #[ignore]; run with:

  cargo test -p primer-kb-load --test retrieval_sweep \
    -- --ignored sweep_retrieval_params --nocapture

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Inspect sweep output, verify the winning cell is sensible

**Files:** None — this is a judgment step. The sweep already algorithmically picks a winner; we sanity-check it.

**Background:** Read `/tmp/sweep_output.txt` from Task 4. Confirm the winning cell is a sensible production default (not, e.g., `top_k=10, min_score=0.0` which means "no filtering at all"). Decide whether to land it.

- [ ] **Step 5.1: Read `/tmp/sweep_output.txt` end to end**

```bash
cat /tmp/sweep_output.txt
```

Look for:
- The 24-row table — every cell should have a sensible loose/strict pair (not all 0%, not all 100%; expect a curve as `min_score` rises).
- The `>>> WINNING CELL` line — extract `top_k=X, min_score=Y`.
- The per-query failure list — note any queries that fail at the winning cell.

- [ ] **Step 5.2: Sanity-check the winner**

Apply these heuristics:
1. **`top_k` should be ≤ 7.** Higher means the prompt budget gets a lot of noise. If the winner picks 10, prefer 7 even at a small recall cost; document the override.
2. **`min_score` should be > 0 if any cell with `min_score > 0` ties strict recall.** A `min_score=0.0` filter is effectively no filter; the lex rule's `then a.min_score.partial_cmp(&b.min_score).unwrap()` already prefers higher scores at ties, so this should auto-resolve.
3. **`strict_recall` should be ≥ 80%.** Below that, the corpus has gaps that tuning can't fix; file an issue per Task 9 and consider whether to land the chosen defaults at all (probably yes — better than today's untuned defaults — but flag it).

- [ ] **Step 5.3: Record the chosen winner**

Write down for Task 6:
- Chosen `KB_FINAL_TOP_K`: \_\_\_
- Chosen `KB_BM25_ONLY_MIN_SCORE`: \_\_\_
- Chosen-cell strict recall: \_\_\_%
- Chosen-cell loose recall: \_\_\_%
- Queries that fail at the chosen cell (loose OR strict): \_\_\_

**No commit in this step.** This is decision-recording for the next task's commit message.

---

## Task 6: Update consts to the chosen winner

**Files:**
- Modify: `src/crates/primer-core/src/consts.rs`

**Background:** The chosen `top_k` and `min_score` from Task 5 land as the new production defaults. The chosen values replace the literals in `KB_FINAL_TOP_K` and `KB_BM25_ONLY_MIN_SCORE`.

- [ ] **Step 6.1: Update `KB_FINAL_TOP_K`**

In `src/crates/primer-core/src/consts.rs`, find:

```rust
    /// Number of fused passages handed to the prompt builder per turn.
    /// Matches the BM25-only fallback path's top-K so the system prompt
    /// stays the same shape regardless of which retrieval mode is live.
    pub const KB_FINAL_TOP_K: usize = 3;
```

Replace `3` with the chosen `top_k` from Task 5. Update the doc comment to reflect that the value comes from the sweep:

```rust
    /// Number of fused passages handed to the prompt builder per turn.
    /// Matches the BM25-only fallback path's top-K so the system prompt
    /// stays the same shape regardless of which retrieval mode is live.
    /// Tuned against the 90-passage seed corpus via the sweep at
    /// `tests/retrieval_sweep.rs` — see
    /// `docs/superpowers/specs/2026-05-06-retrieval-tuning-design.md`.
    pub const KB_FINAL_TOP_K: usize = <CHOSEN_TOP_K>;
```

- [ ] **Step 6.2: Update `KB_BM25_ONLY_MIN_SCORE`**

Find:

```rust
    /// Minimum BM25 score for the BM25-only knowledge-base path
    /// (the fallback when no embedder is wired). Higher = stricter,
    /// fewer noisy hits.
    pub const KB_BM25_ONLY_MIN_SCORE: f64 = 0.5;
```

Replace `0.5` with the chosen `min_score`. Update the doc:

```rust
    /// Minimum BM25 score for the BM25-only knowledge-base path
    /// (the fallback when no embedder is wired). Higher = stricter,
    /// fewer noisy hits. Tuned against the 90-passage seed corpus
    /// via the sweep at `tests/retrieval_sweep.rs`.
    pub const KB_BM25_ONLY_MIN_SCORE: f64 = <CHOSEN_MIN_SCORE>;
```

- [ ] **Step 6.3: Build & test the workspace**

Run: `~/.cargo/bin/cargo test --workspace 2>&1 | grep -E "^test result" | awk '{n+=$4; f+=$6} END {printf "%d passed, %d failed\n", n, f}'`
Expected: `552 passed, 0 failed` (550 baseline + 2 sanity tests added in Task 2 — note the sanity tests appear in every test binary that imports `mod common;`, so the count grows by however many test binaries × sanity tests).

If there's a test failure, it's likely the regression test in `retrieval_quality.rs` — investigate. The regression test still uses `REGRESSION_TOP_K = 5, min_score = NEG_INFINITY` (Task 8 will switch it to consts), so this should not yet fail. If it does fail, the issue is upstream of the consts change.

- [ ] **Step 6.4: Commit**

```bash
git add src/crates/primer-core/src/consts.rs
git commit -m "$(cat <<'EOF'
refactor(retrieval): land tuned BM25 defaults from 24-cell sweep

KB_FINAL_TOP_K: 3 → <CHOSEN_TOP_K>
KB_BM25_ONLY_MIN_SCORE: 0.5 → <CHOSEN_MIN_SCORE>

Chosen by lexicographic selection from the sweep at
tests/retrieval_sweep.rs against the 90-passage seed corpus and
the ~85-query benchmark in tests/common/mod.rs:

  strict recall: <X>% (<P>/<T> strict-subset queries place their
                        canonical id in top-k)
  loose recall:  <Y>% (<P>/<T> queries surface required terms in
                        top-k combined text)

The selection rule: max strict_recall, then max loose_recall,
then min top_k (less prompt noise), then max min_score (stronger
noise floor). See the spec for the full methodology.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

(Replace `<CHOSEN_TOP_K>`, `<CHOSEN_MIN_SCORE>`, `<X>`, `<Y>`, etc. with the recorded values from Task 5.)

---

## Task 7: Align `RetrievalParams::default()` with consts

**Files:**
- Modify: `src/crates/primer-core/src/knowledge.rs`

**Background:** `RetrievalParams::default()` currently returns `top_k: 5, min_score: 0.0` — drifted from consts. Land a single source of truth: `Default` reads from `consts::retrieval`.

- [ ] **Step 7.1: Update `RetrievalParams::default()` to read from consts**

In `src/crates/primer-core/src/knowledge.rs`, find:

```rust
impl Default for RetrievalParams {
    fn default() -> Self {
        Self {
            top_k: 5,
            min_score: 0.0,
            source_filter: vec![],
        }
    }
}
```

Replace with:

```rust
impl Default for RetrievalParams {
    fn default() -> Self {
        Self {
            top_k: crate::consts::retrieval::KB_FINAL_TOP_K,
            min_score: crate::consts::retrieval::KB_BM25_ONLY_MIN_SCORE,
            source_filter: vec![],
        }
    }
}
```

- [ ] **Step 7.2: Add a unit test for the alignment**

In the same file (`src/crates/primer-core/src/knowledge.rs`), find an existing `#[cfg(test)] mod tests { ... }` block, or add one at the bottom. Add the following test:

```rust
#[cfg(test)]
mod retrieval_params_tests {
    use super::*;

    #[test]
    fn default_mirrors_consts() {
        let d = RetrievalParams::default();
        assert_eq!(
            d.top_k,
            crate::consts::retrieval::KB_FINAL_TOP_K,
            "RetrievalParams::default().top_k must equal KB_FINAL_TOP_K — drift means production callers using `Default` get untuned values"
        );
        assert!(
            (d.min_score - crate::consts::retrieval::KB_BM25_ONLY_MIN_SCORE).abs() < f64::EPSILON,
            "RetrievalParams::default().min_score must equal KB_BM25_ONLY_MIN_SCORE"
        );
        assert!(
            d.source_filter.is_empty(),
            "default source_filter must be empty (no source restriction)"
        );
    }
}
```

- [ ] **Step 7.3: Run primer-core tests**

Run: `~/.cargo/bin/cargo test -p primer-core retrieval_params_tests`
Expected: `1 passed`.

- [ ] **Step 7.4: Run the full workspace tests**

Run: `~/.cargo/bin/cargo test --workspace 2>&1 | grep -E "^test result" | awk '{n+=$4; f+=$6} END {printf "%d passed, %d failed\n", n, f}'`
Expected: 1 more test than the previous total (the new alignment test). 0 failed.

- [ ] **Step 7.5: Commit**

```bash
git add src/crates/primer-core/src/knowledge.rs
git commit -m "$(cat <<'EOF'
refactor(retrieval): align RetrievalParams::default() with consts

Production code today reads consts::retrieval::* directly when
constructing RetrievalParams literals; nothing in production calls
RetrievalParams::default(). But the struct's Default impl had
drifted to top_k=5, min_score=0.0 — the wrong defaults if a future
caller ever does `RetrievalParams::default()`.

Default now reads from consts so the two cannot drift again. A new
unit test asserts the alignment so accidental future drift breaks
CI.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Update `retrieval_quality.rs` to assert against the tuned defaults

**Files:**
- Modify: `src/crates/primer-kb-load/tests/retrieval_quality.rs`

**Background:** The regression test currently uses `REGRESSION_TOP_K = 5, min_score = NEG_INFINITY`. Switch to the production consts, AND segregate any queries that don't pass at the tuned defaults into a `KNOWN_FAILING` list (kept visible but excluded from the regression assertion).

The question of which queries are "known failing" is informed by Task 5's per-query breakdown.

- [ ] **Step 8.1: Determine the known-failing query list**

From Task 5's recorded "Queries that fail at the chosen cell": gather the `query` strings of any benchmark query whose:
- loose recall failed (required terms not in top-k combined text), OR
- strict recall failed (canonical_id not in top-k by id, when `canonical_id.is_some()`)

at the chosen `(top_k, min_score)` cell.

If the list is empty: skip Step 8.2 and just update the params in Step 8.3.

- [ ] **Step 8.2: Add a `KNOWN_FAILING_QUERIES` const to `tests/common/mod.rs`**

Append to `tests/common/mod.rs` after `QUERIES`:

```rust
/// Benchmark queries that the production retrieval defaults
/// (`KB_FINAL_TOP_K`, `KB_BM25_ONLY_MIN_SCORE`) cannot satisfy. Kept
/// in the dataset (rather than deleted) so the sweep still measures
/// against them. The regression test in `retrieval_quality.rs`
/// excludes these from its assertion.
///
/// **Tracked in:** GitHub issue #<ISSUE_NUMBER> (see Task 9 of the
/// retrieval-tuning plan).
pub const KNOWN_FAILING_QUERIES: &[&str] = &[
    // Insert each failing query string from Task 5's per-query
    // breakdown, one per line. Example:
    //   "why do flowers smell nice",
];
```

If Task 5's list is empty, set `KNOWN_FAILING_QUERIES: &[&str] = &[];` and skip the issue link in the doc comment.

- [ ] **Step 8.3: Update `retrieval_quality.rs` to use the tuned defaults**

In `src/crates/primer-kb-load/tests/retrieval_quality.rs`:

Replace the `REGRESSION_TOP_K` const + the `seed_corpus_satisfies_canonical_queries` test body. New form:

```rust
mod common;

use common::{KNOWN_FAILING_QUERIES, QUERIES};
use primer_core::consts::retrieval as r;
use primer_core::i18n::Locale;
use primer_core::knowledge::{KnowledgeBase, RetrievalParams};
use primer_kb_load::auto_seed_if_empty;
use primer_knowledge::SqliteKnowledgeBase;

#[tokio::test]
async fn seed_corpus_satisfies_canonical_queries() {
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    let stats = auto_seed_if_empty(&kb, Locale::English)
        .await
        .unwrap()
        .expect("seed dir must contain at least one *.en.jsonl");
    assert!(
        stats.inserted >= 10,
        "seed corpus must contain at least 10 passages, got {}",
        stats.inserted
    );

    let mut failures = Vec::new();
    for q in QUERIES {
        if KNOWN_FAILING_QUERIES.contains(&q.query) {
            continue;
        }
        let params = RetrievalParams {
            top_k: r::KB_FINAL_TOP_K,
            min_score: r::KB_BM25_ONLY_MIN_SCORE,
            source_filter: vec![],
        };
        let hits = kb.retrieve(q.query, &params).await.unwrap();
        let combined = hits
            .iter()
            .map(|p| p.text.to_lowercase())
            .collect::<Vec<_>>()
            .join(" \n ");

        // Loose: required terms appear somewhere in top-k combined.
        let missing_loose: Vec<_> = q
            .required
            .iter()
            .filter(|term| !combined.contains(&term.to_lowercase()))
            .collect();
        if !missing_loose.is_empty() {
            failures.push(format!(
                "[loose] query {:?}: top {} did not contain required term(s) {:?}; got ids {:?}",
                q.query,
                r::KB_FINAL_TOP_K,
                missing_loose,
                hits.iter().map(|p| &p.id).collect::<Vec<_>>(),
            ));
        }

        // Strict: canonical_id must be in top-k for entries that have one.
        if let Some(canonical) = q.canonical_id {
            if !hits.iter().any(|p| p.id == canonical) {
                failures.push(format!(
                    "[strict] query {:?}: canonical_id {:?} not in top {}; got ids {:?}",
                    q.query,
                    canonical,
                    r::KB_FINAL_TOP_K,
                    hits.iter().map(|p| &p.id).collect::<Vec<_>>(),
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "retrieval-quality regressions at production defaults (top_k={}, min_score={:.2}):\n  - {}",
        r::KB_FINAL_TOP_K,
        r::KB_BM25_ONLY_MIN_SCORE,
        failures.join("\n  - ")
    );
}

// (the existing `seed_corpus_registers_sources_for_attribution` test
// below is unchanged — keep it as-is from Task 1.)
```

The second test (`seed_corpus_registers_sources_for_attribution`) is unchanged; copy its body verbatim from the file's current state.

- [ ] **Step 8.4: Run regression, expect green**

Run: `~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality`
Expected: All passing. The list of `KNOWN_FAILING_QUERIES` from Step 8.2 is the load-bearing part — if a query you forgot to mark as known-failing fails, this will surface the gap loudly.

If queries fail that you didn't expect: re-read `/tmp/sweep_output.txt` from Task 4. The regression must match what the sweep showed at the chosen cell.

- [ ] **Step 8.5: Run the full workspace tests**

Run: `~/.cargo/bin/cargo test --workspace`
Expected: All passing.

- [ ] **Step 8.6: Commit**

```bash
git add src/crates/primer-kb-load/tests/retrieval_quality.rs src/crates/primer-kb-load/tests/common/mod.rs
git commit -m "$(cat <<'EOF'
test(retrieval): pin regression test to tuned production defaults

Switches retrieval_quality.rs from the legacy permissive params
(top_k=5, min_score=NEG_INFINITY) to read consts::retrieval::*
directly — KB_FINAL_TOP_K and KB_BM25_ONLY_MIN_SCORE — so a
regression in production retrieval breaks CI.

Adds an explicit strict assertion: queries with canonical_id must
place that id in top-k. The previous test only enforced the
substring contract.

KNOWN_FAILING_QUERIES (in tests/common/mod.rs) lists queries that
the chosen defaults can't satisfy; tracked in issue #<ISSUE_NUMBER>
for follow-up corpus expansion. Empty if the chosen cell hits 100%
recall on every query.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: File the corpus-gap issue (conditional on KNOWN_FAILING_QUERIES being non-empty)

**Files:** None — runs `gh issue create`.

**Background:** Per the failure-mode policy, queries the chosen defaults can't satisfy point at corpus gaps, not tuning gaps. File one issue listing them so a future session can fix them via corpus expansion.

**Skip this task entirely if `KNOWN_FAILING_QUERIES` from Task 8 is empty.**

- [ ] **Step 9.1: File the issue via `gh issue create`**

Run from the repo root:

```bash
cd /Users/hherb/src/primer
gh issue create --title "Corpus gap: retrieval defaults cannot satisfy <N> benchmark queries" --body "$(cat <<'EOF'
The retrieval-tuning sweep at `src/crates/primer-kb-load/tests/retrieval_sweep.rs` chose production defaults `KB_FINAL_TOP_K=<CHOSEN_TOP_K>, KB_BM25_ONLY_MIN_SCORE=<CHOSEN_MIN_SCORE>` per the lex selection rule (max strict_recall, max loose_recall, min top_k, max min_score). At those defaults, the following benchmark queries fail (loose-substring or strict-canonical-id):

- "<query 1>" — loose|strict — canonical_id: <id or "n/a">
- "<query 2>" — ...
- ...

These represent corpus gaps, not tuning gaps: tuning the BM25 path further would only trade off other queries' recall. The fix is corpus expansion (more passages on the gap topics, or refinement of `required` terms in `tests/common/mod.rs` if the gap is a brittle assertion rather than a real coverage hole).

Tracked from `KNOWN_FAILING_QUERIES` in `src/crates/primer-kb-load/tests/common/mod.rs`. Each entry there carries this issue number in its surrounding doc comment.

Sweep output that informed the choice:

\`\`\`
<paste relevant rows from /tmp/sweep_output.txt>
\`\`\`

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

(Replace `<N>`, `<CHOSEN_TOP_K>`, `<CHOSEN_MIN_SCORE>`, the query list, and the sweep table fragment with concrete values.)

- [ ] **Step 9.2: Update the placeholder issue number in `tests/common/mod.rs`**

Replace `#<ISSUE_NUMBER>` in the `KNOWN_FAILING_QUERIES` doc comment (Task 8.2) with the actual issue number returned by `gh issue create`.

Also replace `#<ISSUE_NUMBER>` in the Task 8 commit message (which already landed) — no, actually, that commit is already in history; don't try to amend. The issue number lives in the source file going forward, which is fine.

- [ ] **Step 9.3: Commit the issue-number update**

```bash
git add src/crates/primer-kb-load/tests/common/mod.rs
git commit -m "$(cat <<'EOF'
docs(retrieval): pin corpus-gap issue number in KNOWN_FAILING_QUERIES

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Hybrid sweep scaffold behind `--features fastembed`

**Files:**
- Create: `src/crates/primer-kb-load/tests/retrieval_sweep_hybrid.rs`

**Background:** Infrastructure-only — runs end-to-end against the real BGE-M3 model, prints a hybrid recall table. Doesn't change any consts. Compiles and runs only when `--features fastembed` is passed. No CI gating.

- [ ] **Step 10.1: Create the hybrid sweep file**

Path: `src/crates/primer-kb-load/tests/retrieval_sweep_hybrid.rs`

```rust
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
use primer_core::knowledge::{HybridParams, KnowledgeBase};
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
        if self.loose_total == 0 { 0.0 } else { self.loose_pass as f64 / self.loose_total as f64 }
    }
    fn strict_recall(&self) -> f64 {
        if self.strict_total == 0 { 0.0 } else { self.strict_pass as f64 / self.strict_total as f64 }
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
    auto_seed_if_empty(&kb, Locale::English).await.unwrap().unwrap();

    let embedder: Arc<dyn Embedder> = Arc::new(FastEmbedBackend::new().expect("BGE-M3 init failed"));
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
                .then((a.rrf_k - 60.0).abs().partial_cmp(&(b.rrf_k - 60.0).abs()).unwrap().reverse())
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
```

- [ ] **Step 10.2: Verify the file compiles under `--features fastembed`**

Run: `~/.cargo/bin/cargo build -p primer-kb-load --features fastembed --tests`
Expected: clean build. The first build pulls fastembed-rs and is slow (~3-5 min).

If a compile error mentions `reembed_kb` not found: it's defined in `primer-kb-load`. Confirm the import path in the new file matches the public API; consult `src/crates/primer-kb-load/src/lib.rs` for the exact name.

- [ ] **Step 10.3: (Optional, network required) Run the hybrid sweep end-to-end**

This step is optional — the user is busy and the model download is ~570 MB. Skip if no network or no time. The scaffold compiling is sufficient acceptance for this task.

If running:

```bash
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test retrieval_sweep_hybrid -- --ignored sweep_retrieval_params_hybrid --nocapture 2>&1 | tee /tmp/sweep_hybrid_output.txt
```

Expected: First run downloads BGE-M3, then prints a 54-cell table (3×3×2×3 = 54 hybrid cells × 85 queries; runtime 5-15 minutes). The "NOTIONAL HYBRID WINNER" line at the bottom identifies the best hybrid cell.

If the run completes: save the output file. Future tuning starts from there.

- [ ] **Step 10.4: Verify default-feature build is not affected**

Run: `~/.cargo/bin/cargo build -p primer-kb-load --tests`
Expected: clean build, no fastembed-rs in the dep tree. The `#![cfg(feature = "fastembed")]` at the top of `retrieval_sweep_hybrid.rs` makes the file a no-op when the feature is off.

Run: `~/.cargo/bin/cargo test --workspace 2>&1 | grep -E "^test result" | awk '{n+=$4; f+=$6} END {printf "%d passed, %d failed\n", n, f}'`
Expected: same count as Task 7.5 (no new tests appear in default-feature build).

- [ ] **Step 10.5: Commit**

```bash
git add src/crates/primer-kb-load/tests/retrieval_sweep_hybrid.rs
git commit -m "$(cat <<'EOF'
test(retrieval): scaffold hybrid sweep behind --features fastembed

Adds tests/retrieval_sweep_hybrid.rs as #[ignore]'d, file-level
#![cfg(feature = "fastembed")]'d infrastructure. Iterates a 54-cell
grid over (bm25_top_k × vector_top_k × final_top_k × rrf_k)
against the real BGE-M3 model and prints a recall table to
stdout. No hybrid defaults change.

Run via:

  cargo test -p primer-kb-load --features fastembed \
    --test retrieval_sweep_hybrid \
    -- --ignored sweep_retrieval_params_hybrid --nocapture

First run downloads BGE-M3 (~570 MB). Default-feature workspace
build is unaffected: the #![cfg] at the top of the file makes it
a no-op when the feature is off.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Issue #37 — User-Agent contact in `simple_wikipedia.py`

**Files:**
- Modify: `data/ingest/simple_wikipedia.py`
- Possibly modify: `data/ingest/tests/` (if any test pins the User-Agent string)

**Background:** Replace the placeholder contact in the User-Agent header with `my.list.subscriptions@gmail.com`.

- [ ] **Step 11.1: Find the User-Agent string in `simple_wikipedia.py`**

Run: `grep -n "PrimerSeedBuilder" data/ingest/simple_wikipedia.py`
Expected: at least one match showing the current `(contact: see-repo-readme)` form.

- [ ] **Step 11.2: Replace the contact**

In `data/ingest/simple_wikipedia.py`, replace:

```python
"PrimerSeedBuilder/0.1 (contact: see-repo-readme)"
```

with:

```python
"PrimerSeedBuilder/0.1 (contact: my.list.subscriptions@gmail.com)"
```

(Use the Edit tool with the exact match. There should be exactly one occurrence; if there are more, replace all of them — they should all be the same constant.)

- [ ] **Step 11.3: Check whether any Python test pins the User-Agent string**

Run: `grep -rn "PrimerSeedBuilder\|see-repo-readme" data/ingest/tests/`
Expected: probably matches in at least one test file (the contract tests for `fetch_leads`).

For each match, update to the new contact form. The exact text replacement is the same as Step 11.2.

- [ ] **Step 11.4: Run the Python tests**

Run from `/Users/hherb/src/primer/data/ingest`:

```bash
.venv/bin/pytest tests/
```

Expected: 43 passed (the same count NEXT_SESSION.md mentions). If the venv doesn't exist, set it up per `data/ingest/README.md`:

```bash
python3 -m venv .venv
.venv/bin/pip install -r requirements.txt pytest
.venv/bin/pytest tests/
```

- [ ] **Step 11.5: Commit**

```bash
cd /Users/hherb/src/primer
git add data/ingest/simple_wikipedia.py data/ingest/tests/
git commit -m "$(cat <<'EOF'
fix(ingest): replace placeholder User-Agent contact (closes #37)

The Wikipedia ingest pipeline's User-Agent string carried
"see-repo-readme" as a placeholder. Replaces it with
my.list.subscriptions@gmail.com — Horst's spam-catching mailbox —
so future live runs identify the developer contact correctly per
Wikimedia's API etiquette guidance.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Final verification suite

**Files:** None — runs the full verification commands.

**Background:** Confirm the workspace is green end-to-end after all changes.

- [ ] **Step 12.1: Run workspace tests**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test --workspace 2>&1 | grep -E "^test result" | awk '{n+=$4; f+=$6} END {printf "%d passed, %d failed\n", n, f}'
```

Expected: 0 failed. Total count should be ~553-560 depending on how many sanity tests appear under each test binary.

- [ ] **Step 12.2: Run clippy**

```bash
~/.cargo/bin/cargo clippy --workspace --all-targets 2>&1 | tail -30
```

Expected: no warnings, "0 warnings emitted" or clean exit. If new warnings appear in code touched this session, fix them.

- [ ] **Step 12.3: Run rustfmt check**

```bash
~/.cargo/bin/cargo fmt --all -- --check
```

Expected: silent success (exit 0). If output appears, run `~/.cargo/bin/cargo fmt --all` to fix and re-run the check.

- [ ] **Step 12.4: Run Python tests**

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/pytest tests/
```

Expected: 43 passed.

- [ ] **Step 12.5: Run the regression test under `--release` to ensure no debug-only behaviour**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality --release
```

Expected: pass.

If anything fails: stop and investigate before claiming completion.

- [ ] **Step 12.6: No commit needed** — verification only.

---

## Task 13: Update README, ROADMAP, CLAUDE.md to reflect Phase 0.2 closure

**Files:**
- Modify: `README.md`
- Modify: `ROADMAP.md`
- Modify: `CLAUDE.md`

**Background:** Phase 0.2 closes with this work. Bring the docs in line.

- [ ] **Step 13.1: Update `README.md`**

Find the section that describes phase status (likely a "Status" or "Working today" block). Update language so Phase 0.2 is described as **fully done** (not just MVP done) and retrieval defaults are described as **tuned**. Concrete edits:

- If the README mentions "tune `RetrievalParams` defaults against the broader corpus" as still-pending, remove or strike that bullet.
- Add a line under the working-today list: "tuned BM25 retrieval defaults via diagnostic sweep over the 90-passage corpus".

Read the relevant section first, then apply the targeted edit. Keep the diff minimal.

- [ ] **Step 13.2: Update `ROADMAP.md`**

Find the "Phase 0.2" section. Mark the retrieval-tuning task complete. If the ROADMAP has a "next up" pointer that referenced this task, advance it to the next Phase milestone (consult ROADMAP for what's next — likely Phase 1.1 local llama.cpp inference or Phase 0.3 follow-ups).

- [ ] **Step 13.3: Update `CLAUDE.md`**

Find the "Phase 0.1 is done; Phase 0.2 (MVP) is done; Phase 0.3 is done" line near the top. Update to reflect Phase 0.2 fully closed. Concretely:

- If the line says "Phase 0.2 (MVP) is done", change to "Phase 0.2 is done".
- In the "tuned defaults" or hybrid-retrieval gotcha section, mention that BM25-only defaults (`KB_FINAL_TOP_K`, `KB_BM25_ONLY_MIN_SCORE`) are now tuned against the 90-passage corpus and the sweep harness lives at `tests/retrieval_sweep.rs`.

Read each section first; keep edits surgical.

- [ ] **Step 13.4: Run a final test pass after doc edits**

```bash
~/.cargo/bin/cargo test --workspace 2>&1 | grep -E "^test result" | awk '{n+=$4; f+=$6} END {printf "%d passed, %d failed\n", n, f}'
```

Expected: same as Task 12.1 (docs don't change tests, but defensive).

- [ ] **Step 13.5: Commit**

```bash
git add README.md ROADMAP.md CLAUDE.md
git commit -m "$(cat <<'EOF'
docs: Phase 0.2 closeout (retrieval-parameter tuning landed)

README: tuned retrieval defaults listed under working-today.
ROADMAP: Phase 0.2 marked complete; pointer advanced.
CLAUDE.md: Phase 0.2 phrasing updated; sweep harness location
documented in the retrieval gotchas.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

Spec coverage check:

- ✅ Expanded benchmark (~85 queries with cluster + canonical_id) — Tasks 1-3
- ✅ Sweep test with grid + lex selection rule + per-cluster recall — Task 4
- ✅ Tune `KB_FINAL_TOP_K` and `KB_BM25_ONLY_MIN_SCORE` — Task 6
- ✅ `RetrievalParams::default()` aligned with consts + unit test — Task 7
- ✅ Regression test pinned to tuned defaults with strict-id assertion — Task 8
- ✅ Issue filed if strict recall < 100% — Task 9
- ✅ Hybrid sweep scaffold behind `--features fastembed --ignored` — Task 10
- ✅ Issue #37 User-Agent fix — Task 11
- ✅ Workspace verification (test + clippy + fmt + Python) — Task 12
- ✅ README + ROADMAP + CLAUDE.md updates — Task 13

Type consistency check:

- `BenchQuery` fields: `query: &'static str`, `required: &'static [&'static str]`, `canonical_id: Option<&'static str>`, `cluster: Cluster` — used identically in Tasks 1, 4, 8, 10. ✓
- `Cluster` enum variants: `Space`, `Body`, `HowThingsWork`, `Life`, `EarthWeather`, `Wiki` — used identically in Tasks 1, 4. ✓
- `RetrievalParams` fields: `top_k: usize`, `min_score: f64`, `source_filter: Vec<String>` — used identically in Tasks 4, 8. ✓
- `HybridParams` fields: `bm25_top_k`, `vector_top_k`, `final_top_k`, `rrf_k`, `min_score`, `source_filter` — used in Task 10. ✓
- Const names: `KB_FINAL_TOP_K`, `KB_BM25_ONLY_MIN_SCORE` — match the existing `consts.rs` exactly. ✓
- Function names: `auto_seed_if_empty`, `reembed_kb`, `FastEmbedBackend::new()` — verified against actual API. ✓

Placeholder scan: Searched for "TBD", "TODO", "implement later", "fill in details", "appropriate", "as needed". Only legitimate uses remain (e.g., `<CHOSEN_TOP_K>` placeholders that the engineer fills in from sweep output in Tasks 5-6, and `<ISSUE_NUMBER>` that gets pinned in Task 9). These are intentional — the values come from running the sweep, not from designing the plan.

Plan complete.
