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
use primer_kb_load::auto_seed_if_empty;
use primer_knowledge::SqliteKnowledgeBase;

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
    // Life cluster
    (
        "how do plants make food",
        &["photosynthesis", "chlorophyll"],
        5,
    ),
    ("how do seeds sprout germinate", &["germinat", "root"], 5),
    ("what is a food chain", &["producer", "consumer"], 5),
    ("what are cells", &["cell", "nucleus"], 5),
    ("how does evolution work", &["natural selection", "dna"], 5),
    (
        "what happened to the dinosaurs",
        &["asteroid", "extinct"],
        5,
    ),
    ("what makes a mammal a mammal", &["fur", "milk"], 5),
    ("how do birds fly", &["feather", "wing"], 5),
    ("what is a frog", &["amphibian", "tadpole"], 5),
    ("how many legs does an insect have", &["six", "thorax"], 5),
    ("where do babies come from", &["egg", "sperm"], 5),
    // Earth and weather cluster
    (
        "what makes rain fall from clouds",
        &["droplet", "condens"],
        5,
    ),
    (
        "how do snowflakes form six arms",
        &["hexagon", "crystal"],
        5,
    ),
    ("why are clouds white or grey", &["scatter", "droplet"], 5),
    ("what causes wind to blow", &["pressure", "air"], 5),
    ("why does lightning make thunder", &["spark", "shock"], 5),
    (
        "why do rainbows have colours",
        &["refract", "wavelength"],
        5,
    ),
    (
        "water cycle evaporation precipitation",
        &["evapora", "condens"],
        5,
    ),
    ("how do volcanoes erupt magma", &["magma", "tectonic"], 5),
    ("what causes earthquakes faults", &["fault", "tectonic"], 5),
    ("how do mountains form collide", &["plate", "collid"], 5),
    (
        "difference between climate and weather",
        &["climate", "average"],
        5,
    ),
    // ----- Wikipedia layer (Phase 0.2 MVP) -----
    // These queries target concepts the hand-drafted seed corpus does
    // NOT cover, so they exercise the wiki_passages.en.jsonl layer
    // specifically. Required terms are case-insensitive substrings
    // chosen from the actual lead text of each wiki article.
    ("what is gravity", &["gravity", "attraction"], 5),
    ("what is an atom", &["atom", "matter"], 5),
    ("what is a virus", &["virus", "parasite"], 5),
    ("what is climate change", &["climate", "temperature"], 5),
    ("what is DNA", &["dna", "genetic"], 5),
    ("how do vaccines work", &["vaccine"], 5),
    ("what is friction", &["friction", "force"], 5),
    ("what makes a plant a plant", &["plant", "autotroph"], 5),
];

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
    // Use auto_seed_if_empty so every shipped *.en.jsonl layer is
    // attribution-validated (CC0 hand-drafted seed AND the Wikipedia
    // CC-BY-SA-3.0 layer). The earlier load_jsonl(seed_path()) shape only
    // covered the first file and would silently pass a wiki-layer
    // regression where to_passage forgot to set license/attribution.
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
    assert!(
        seen_cc0,
        "expected at least one CC0-1.0 source (seed corpus)"
    );
    assert!(
        seen_cc_by_sa,
        "expected at least one CC-BY-SA-3.0 source (wiki layer)"
    );
}
