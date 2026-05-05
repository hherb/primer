//! Retrieval-quality integration test.
//!
//! Loads the in-repo English seed corpus into a temp DB and asserts that
//! canonical child queries surface passages mentioning the expected key
//! terms. This is the truth signal for `RetrievalParams` defaults — if a
//! query needs a higher `top_k` to hit, that's a tuning signal.
//!
//! The test runs in CI on every push, so corpus edits that regress
//! retrieval get caught early.

use primer_core::i18n::Locale;
use primer_core::knowledge::{KnowledgeBase, RetrievalParams};
use primer_kb_load::load_jsonl;
use primer_knowledge::SqliteKnowledgeBase;
use std::path::PathBuf;

fn seed_path() -> PathBuf {
    // CARGO_MANIFEST_DIR = src/crates/primer-kb-load. Walk up to repo root.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .find_map(|d| {
            let p = d.join("data/seed/seed_passages.en.jsonl");
            p.is_file().then_some(p)
        })
        .expect("could not locate data/seed/seed_passages.en.jsonl from CARGO_MANIFEST_DIR")
}

/// Each entry: (query, required_substring_terms_in_top_k, top_k).
/// The query is what a child might actually ask. The required terms are
/// case-insensitive substrings that at least one of the top-k passages
/// must contain — fragments like "fus" match both "fusion" and "fusing"
/// so the test asserts content presence without locking morphology.
/// `top_k = 5` mirrors the dialogue manager's realistic retrieval depth
/// once `RetrievalParams::default()` is tuned (currently 3, may rise).
const QUERIES: &[(&str, &[&str], usize)] = &[
    // Space cluster
    ("how does the sun shine", &["fus", "hydrogen"], 5),
    ("why does the moon change shape", &["phase", "sunlight"], 5),
    ("why is it day and night", &["spin", "axis"], 5),
    ("how big is jupiter", &["jupiter"], 5),
    ("what is a star made of", &["hydrogen", "helium"], 5),
    ("what is the milky way", &["galax", "spiral"], 5),
    (
        "what happens inside a black hole",
        &["gravity", "event horizon"],
        5,
    ),
    ("what is a shooting star", &["meteor", "atmosphere"], 5),
    ("why do eclipses happen", &["shadow", "moon"], 5),
    ("what causes ocean tides", &["moon", "gravity"], 5),
    ("how did the universe begin", &["big bang", "expand"], 5),
    ("why are there seasons", &["tilt", "axis"], 5),
    // Body cluster
    ("how does the heart pump blood", &["heart", "ventric"], 5),
    ("why do we breathe", &["oxygen", "alveoli"], 5),
    ("what does the brain do", &["neuron", "cortex"], 5),
    ("how does food get digested", &["stomach", "intestine"], 5),
    ("how many bones are in the body", &["skeleton", "206"], 5),
    ("how do muscles move", &["contract", "fibre"], 5),
    ("what is skin made of", &["epidermis", "dermis"], 5),
    ("how does the body fight germs", &["immune", "antibod"], 5),
    ("how do eyes see", &["retina", "lens"], 5),
    ("how do ears hear", &["cochlea", "vibrat"], 5),
    ("why do we sleep and dream", &["rem", "memor"], 5),
    // How-things-work cluster
    ("how do magnets work", &["magnetic field", "pole"], 5),
    ("what is electricity", &["electron", "circuit"], 5),
    ("how does a battery store energy", &["chemical", "anode"], 5),
    ("how does a lever work", &["fulcrum", "force"], 5),
    ("what makes sound", &["vibrat", "wave"], 5),
    ("speed of light photons", &["photon", "wavelength"], 5),
    ("how does a mirror work", &["reflect", "angle"], 5),
    ("how does fire burn", &["oxygen", "fuel"], 5),
    ("why does ice float", &["density", "molecule"], 5),
    ("why does salt melt ice", &["freezing", "sodium"], 5),
    ("how does soap clean", &["oil", "molecule"], 5),
];

#[tokio::test]
async fn seed_corpus_satisfies_canonical_queries() {
    let db = tempfile::NamedTempFile::new().unwrap();
    let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
    let stats = load_jsonl(&kb, &seed_path()).await.unwrap();
    assert!(
        stats.inserted >= 10,
        "seed corpus must contain at least 10 passages, got {}",
        stats.inserted
    );

    let mut failures = Vec::new();
    for (query, required, top_k) in QUERIES {
        let params = RetrievalParams {
            top_k: *top_k,
            min_score: f64::NEG_INFINITY,
            source_filter: vec![],
        };
        let hits = kb.retrieve(query, &params).await.unwrap();
        let combined = hits
            .iter()
            .map(|p| p.text.to_lowercase())
            .collect::<Vec<_>>()
            .join(" \n ");
        let missing: Vec<_> = required
            .iter()
            .filter(|term| !combined.contains(&term.to_lowercase()))
            .collect();
        if !missing.is_empty() {
            failures.push(format!(
                "query {:?}: top {} did not contain required term(s) {:?}; got ids {:?}",
                query,
                top_k,
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
    load_jsonl(&kb, &seed_path()).await.unwrap();
    let sources = kb.list_sources().await.unwrap();
    assert!(
        !sources.is_empty(),
        "every passage must register its source"
    );
    for src in &sources {
        assert!(!src.license.is_empty(), "source {} missing license", src.id);
        assert!(
            !src.attribution.is_empty(),
            "source {} missing attribution",
            src.id
        );
    }
}
