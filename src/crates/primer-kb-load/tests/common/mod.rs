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
    BenchQuery {
        query: "how does the sun shine",
        required: &["fus", "hydrogen"],
        canonical_id: Some("seed:en:sun"),
        cluster: Cluster::Space,
    },
    BenchQuery {
        query: "why does the moon change shape",
        required: &["phase", "sunlight"],
        canonical_id: None,
        cluster: Cluster::Space,
    },
    BenchQuery {
        query: "why is it day and night",
        required: &["spin", "axis"],
        canonical_id: None,
        cluster: Cluster::Space,
    },
    BenchQuery {
        query: "how big is jupiter",
        required: &["jupiter"],
        canonical_id: None,
        cluster: Cluster::Space,
    },
    BenchQuery {
        query: "what is a star made of",
        required: &["hydrogen", "helium"],
        canonical_id: None,
        cluster: Cluster::Space,
    },
    BenchQuery {
        query: "what is the milky way",
        required: &["galax", "spiral"],
        canonical_id: None,
        cluster: Cluster::Space,
    },
    BenchQuery {
        query: "what happens inside a black hole",
        required: &["gravity", "event horizon"],
        canonical_id: None,
        cluster: Cluster::Space,
    },
    BenchQuery {
        query: "what is a shooting star",
        required: &["meteor", "atmosphere"],
        canonical_id: None,
        cluster: Cluster::Space,
    },
    BenchQuery {
        query: "why do eclipses happen",
        required: &["shadow", "moon"],
        canonical_id: None,
        cluster: Cluster::Space,
    },
    BenchQuery {
        query: "what causes ocean tides",
        required: &["moon", "gravity"],
        canonical_id: None,
        cluster: Cluster::Space,
    },
    BenchQuery {
        query: "how did the universe begin",
        required: &["big bang", "expand"],
        canonical_id: None,
        cluster: Cluster::Space,
    },
    BenchQuery {
        query: "why are there seasons",
        required: &["tilt", "axis"],
        canonical_id: Some("seed:en:earth-orbit-day-night"),
        cluster: Cluster::Space,
    },
    // Body cluster
    BenchQuery {
        query: "how does the heart pump blood",
        required: &["heart", "ventric"],
        canonical_id: Some("seed:en:heart"),
        cluster: Cluster::Body,
    },
    BenchQuery {
        query: "why do we breathe",
        required: &["oxygen", "alveoli"],
        canonical_id: None,
        cluster: Cluster::Body,
    },
    BenchQuery {
        query: "what does the brain do",
        required: &["neuron", "cortex"],
        canonical_id: None,
        cluster: Cluster::Body,
    },
    BenchQuery {
        query: "how does food get digested",
        required: &["stomach", "intestine"],
        canonical_id: None,
        cluster: Cluster::Body,
    },
    BenchQuery {
        query: "how many bones are in the body",
        required: &["skeleton", "206"],
        canonical_id: None,
        cluster: Cluster::Body,
    },
    BenchQuery {
        query: "how do muscles move",
        required: &["contract", "fibre"],
        canonical_id: None,
        cluster: Cluster::Body,
    },
    BenchQuery {
        query: "what is skin made of",
        required: &["epidermis", "dermis"],
        canonical_id: None,
        cluster: Cluster::Body,
    },
    BenchQuery {
        query: "how does the body fight germs",
        required: &["immune", "antibod"],
        canonical_id: None,
        cluster: Cluster::Body,
    },
    BenchQuery {
        query: "how do eyes see",
        required: &["retina", "lens"],
        canonical_id: None,
        cluster: Cluster::Body,
    },
    BenchQuery {
        query: "how do ears hear",
        required: &["cochlea", "vibrat"],
        canonical_id: None,
        cluster: Cluster::Body,
    },
    BenchQuery {
        query: "why do we sleep and dream",
        required: &["rem", "memor"],
        canonical_id: None,
        cluster: Cluster::Body,
    },
    // How-things-work cluster
    BenchQuery {
        query: "how do magnets work",
        required: &["magnetic field", "pole"],
        canonical_id: Some("seed:en:magnets"),
        cluster: Cluster::HowThingsWork,
    },
    BenchQuery {
        query: "what is electricity",
        required: &["electron", "circuit"],
        canonical_id: None,
        cluster: Cluster::HowThingsWork,
    },
    BenchQuery {
        query: "how does a battery store energy",
        required: &["chemical", "anode"],
        canonical_id: None,
        cluster: Cluster::HowThingsWork,
    },
    BenchQuery {
        query: "how does a lever work",
        required: &["fulcrum", "force"],
        canonical_id: None,
        cluster: Cluster::HowThingsWork,
    },
    BenchQuery {
        query: "what makes sound",
        required: &["vibrat", "wave"],
        canonical_id: None,
        cluster: Cluster::HowThingsWork,
    },
    BenchQuery {
        query: "speed of light photons",
        required: &["photon", "wavelength"],
        canonical_id: None,
        cluster: Cluster::HowThingsWork,
    },
    BenchQuery {
        query: "how does a mirror work",
        required: &["reflect", "angle"],
        canonical_id: None,
        cluster: Cluster::HowThingsWork,
    },
    BenchQuery {
        query: "how does fire burn",
        required: &["oxygen", "fuel"],
        canonical_id: None,
        cluster: Cluster::HowThingsWork,
    },
    BenchQuery {
        query: "why does ice float",
        required: &["density", "molecule"],
        canonical_id: None,
        cluster: Cluster::HowThingsWork,
    },
    BenchQuery {
        query: "why does salt melt ice",
        required: &["freezing", "sodium"],
        canonical_id: None,
        cluster: Cluster::HowThingsWork,
    },
    BenchQuery {
        query: "how does soap clean",
        required: &["oil", "molecule"],
        canonical_id: None,
        cluster: Cluster::HowThingsWork,
    },
    // Life cluster
    BenchQuery {
        query: "how do plants make food",
        required: &["photosynthesis", "chlorophyll"],
        canonical_id: Some("seed:en:photosynthesis"),
        cluster: Cluster::Life,
    },
    BenchQuery {
        query: "how do seeds sprout germinate",
        required: &["germinat", "root"],
        canonical_id: None,
        cluster: Cluster::Life,
    },
    BenchQuery {
        query: "what is a food chain",
        required: &["producer", "consumer"],
        canonical_id: None,
        cluster: Cluster::Life,
    },
    BenchQuery {
        query: "what are cells",
        required: &["cell", "nucleus"],
        canonical_id: None,
        cluster: Cluster::Life,
    },
    BenchQuery {
        query: "how does evolution work",
        required: &["natural selection", "dna"],
        canonical_id: None,
        cluster: Cluster::Life,
    },
    BenchQuery {
        query: "what happened to the dinosaurs",
        required: &["asteroid", "extinct"],
        canonical_id: None,
        cluster: Cluster::Life,
    },
    BenchQuery {
        query: "what makes a mammal a mammal",
        required: &["fur", "milk"],
        canonical_id: None,
        cluster: Cluster::Life,
    },
    BenchQuery {
        query: "how do birds fly",
        required: &["feather", "wing"],
        canonical_id: None,
        cluster: Cluster::Life,
    },
    BenchQuery {
        query: "what is a frog",
        required: &["amphibian", "tadpole"],
        canonical_id: None,
        cluster: Cluster::Life,
    },
    BenchQuery {
        query: "how many legs does an insect have",
        required: &["six", "thorax"],
        canonical_id: None,
        cluster: Cluster::Life,
    },
    BenchQuery {
        query: "where do babies come from",
        required: &["egg", "sperm"],
        canonical_id: None,
        cluster: Cluster::Life,
    },
    // Earth and weather cluster
    BenchQuery {
        query: "what makes rain fall from clouds",
        required: &["droplet", "condens"],
        canonical_id: None,
        cluster: Cluster::EarthWeather,
    },
    BenchQuery {
        query: "how do snowflakes form six arms",
        required: &["hexagon", "crystal"],
        canonical_id: None,
        cluster: Cluster::EarthWeather,
    },
    BenchQuery {
        query: "why are clouds white or grey",
        required: &["scatter", "droplet"],
        canonical_id: None,
        cluster: Cluster::EarthWeather,
    },
    BenchQuery {
        query: "what causes wind to blow",
        required: &["pressure", "air"],
        canonical_id: None,
        cluster: Cluster::EarthWeather,
    },
    BenchQuery {
        query: "why does lightning make thunder",
        required: &["spark", "shock"],
        canonical_id: None,
        cluster: Cluster::EarthWeather,
    },
    BenchQuery {
        query: "why do rainbows have colours",
        required: &["refract", "wavelength"],
        canonical_id: None,
        cluster: Cluster::EarthWeather,
    },
    BenchQuery {
        query: "water cycle evaporation precipitation",
        required: &["evapora", "condens"],
        canonical_id: None,
        cluster: Cluster::EarthWeather,
    },
    BenchQuery {
        query: "how do volcanoes erupt magma",
        required: &["magma", "tectonic"],
        canonical_id: None,
        cluster: Cluster::EarthWeather,
    },
    BenchQuery {
        query: "what causes earthquakes faults",
        required: &["fault", "tectonic"],
        canonical_id: None,
        cluster: Cluster::EarthWeather,
    },
    BenchQuery {
        query: "how do mountains form collide",
        required: &["plate", "collid"],
        canonical_id: None,
        cluster: Cluster::EarthWeather,
    },
    BenchQuery {
        query: "difference between climate and weather",
        required: &["climate", "average"],
        canonical_id: None,
        cluster: Cluster::EarthWeather,
    },
    // Wikipedia layer (Phase 0.2 MVP)
    BenchQuery {
        query: "what is gravity",
        required: &["gravity", "attraction"],
        canonical_id: Some("wiki-simple:en:gravity"),
        cluster: Cluster::Wiki,
    },
    BenchQuery {
        query: "what is an atom",
        required: &["atom", "matter"],
        canonical_id: Some("wiki-simple:en:atom"),
        cluster: Cluster::Wiki,
    },
    BenchQuery {
        query: "what is a virus",
        required: &["virus", "parasite"],
        canonical_id: Some("wiki-simple:en:virus"),
        cluster: Cluster::Wiki,
    },
    BenchQuery {
        query: "what is climate change",
        required: &["climate", "temperature"],
        canonical_id: Some("wiki-simple:en:climate-change"),
        cluster: Cluster::Wiki,
    },
    BenchQuery {
        query: "what is DNA",
        required: &["dna", "genetic"],
        canonical_id: Some("wiki-simple:en:dna"),
        cluster: Cluster::Wiki,
    },
    BenchQuery {
        query: "how do vaccines work",
        required: &["vaccine"],
        canonical_id: Some("wiki-simple:en:vaccine"),
        cluster: Cluster::Wiki,
    },
    BenchQuery {
        query: "what is friction",
        required: &["friction", "force"],
        canonical_id: Some("wiki-simple:en:friction"),
        cluster: Cluster::Wiki,
    },
    BenchQuery {
        query: "what makes a plant a plant",
        required: &["plant", "autotroph"],
        canonical_id: Some("wiki-simple:en:plant"),
        cluster: Cluster::Wiki,
    },
    // ----- Child-language paraphrases (Task 2 expansion) -----
    BenchQuery {
        query: "why does my heart go thump thump",
        required: &["heart", "ventric"],
        canonical_id: None,
        cluster: Cluster::Body,
    },
    BenchQuery {
        query: "why are bones hard but bendy",
        required: &["skeleton", "206"],
        canonical_id: None,
        cluster: Cluster::Body,
    },
    BenchQuery {
        query: "where does the rain come from",
        required: &["droplet", "condens"],
        canonical_id: None,
        cluster: Cluster::EarthWeather,
    },
    BenchQuery {
        query: "why is the sky blue and not green",
        required: &["scatter", "wavelength"],
        canonical_id: None,
        cluster: Cluster::HowThingsWork,
    },
    BenchQuery {
        query: "why do volcanoes get angry and erupt",
        required: &["magma", "tectonic"],
        canonical_id: None,
        cluster: Cluster::EarthWeather,
    },
    BenchQuery {
        query: "how does my dog see in the dark",
        required: &["retina", "lens"],
        canonical_id: None,
        cluster: Cluster::Body,
    },
    BenchQuery {
        query: "what makes lightning so bright",
        required: &["spark", "shock"],
        canonical_id: None,
        cluster: Cluster::EarthWeather,
    },
    // ----- Cross-cluster questions (Task 2 expansion) -----
    BenchQuery {
        query: "why do stars twinkle but planets do not",
        required: &["atmosphere"],
        canonical_id: None,
        cluster: Cluster::Space,
    },
    BenchQuery {
        query: "how does the moon make the sea move",
        required: &["moon", "gravity"],
        canonical_id: None,
        cluster: Cluster::Space,
    },
    BenchQuery {
        query: "why does the sun help plants grow",
        required: &["photosynthesis", "chlorophyll"],
        canonical_id: None,
        cluster: Cluster::Life,
    },
    BenchQuery {
        query: "how do volcanoes change the weather",
        required: &["magma"],
        canonical_id: None,
        cluster: Cluster::EarthWeather,
    },
    BenchQuery {
        query: "what makes a baby grow inside its mum",
        required: &["egg", "sperm"],
        canonical_id: None,
        cluster: Cluster::Life,
    },
    BenchQuery {
        query: "how does electricity travel through wires made of metal",
        required: &["electron"],
        canonical_id: None,
        cluster: Cluster::HowThingsWork,
    },
    BenchQuery {
        query: "why does ice cool a drink",
        required: &["density", "molecule"],
        canonical_id: None,
        cluster: Cluster::HowThingsWork,
    },
    BenchQuery {
        query: "how do mountains and earthquakes both come from plates moving",
        required: &["plate"],
        canonical_id: None,
        cluster: Cluster::EarthWeather,
    },
    BenchQuery {
        query: "why do mammals breathe air but fish breathe in water",
        required: &["oxygen"],
        canonical_id: None,
        cluster: Cluster::Body,
    },
    // ----- Additional wiki queries (Task 2 expansion) -----
    BenchQuery {
        query: "what is a chemical element",
        required: &["element", "atom"],
        canonical_id: Some("wiki-simple:en:chemical-element"),
        cluster: Cluster::Wiki,
    },
    BenchQuery {
        query: "what is a molecule",
        required: &["molecule", "atom"],
        canonical_id: Some("wiki-simple:en:molecule"),
        cluster: Cluster::Wiki,
    },
    BenchQuery {
        query: "what is a gene",
        required: &["gene"],
        canonical_id: Some("wiki-simple:en:gene"),
        cluster: Cluster::Wiki,
    },
    BenchQuery {
        query: "what is bacteria",
        required: &["bacteria"],
        canonical_id: Some("wiki-simple:en:bacteria"),
        cluster: Cluster::Wiki,
    },
    BenchQuery {
        query: "what is the ocean",
        required: &["ocean"],
        canonical_id: Some("wiki-simple:en:ocean"),
        cluster: Cluster::Wiki,
    },
    BenchQuery {
        query: "what is an ecosystem",
        required: &["ecosystem"],
        canonical_id: Some("wiki-simple:en:ecosystem"),
        cluster: Cluster::Wiki,
    },
    BenchQuery {
        query: "what is soil",
        required: &["soil"],
        canonical_id: Some("wiki-simple:en:soil"),
        cluster: Cluster::Wiki,
    },
];

/// Benchmark queries that the production retrieval defaults
/// (`KB_FINAL_TOP_K`, `KB_BM25_ONLY_MIN_SCORE`) cannot satisfy. Kept
/// in the dataset (rather than deleted) so the sweep still measures
/// against them. The regression test in `retrieval_quality.rs`
/// excludes these from its assertion.
///
/// **Tracked in:** GitHub issue #42.
pub const KNOWN_FAILING_QUERIES: &[&str] = &[
    // strict:FAIL at top_k=5, min_score=0.5 — canonical seed:en:sun
    // is not in top-5 even though loose terms (fus*/hydrogen) appear
    // in another top-5 passage. Tracked as a corpus gap.
    "how does the sun shine",
];

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
                cluster,
                count
            );
        }
    }

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
        auto_seed_if_empty(&kb, Locale::English)
            .await
            .unwrap()
            .unwrap();

        for q in QUERIES
            .iter()
            .filter_map(|q| q.canonical_id.map(|id| (q.query, id)))
        {
            // Probe by replacing every non-alphanumeric in the id with
            // whitespace, so the FTS5 sanitizer (which splits on
            // whitespace and then strips non-alphanumerics from each
            // token) ends up with the id's word-parts as separate
            // OR-joined tokens. Querying the raw id directly fails
            // because `:` and `-` are not whitespace, leaving the
            // sanitizer with a single blob like "seedensun" or
            // "earthorbitdaynight" that matches nothing. With top_k=200
            // (well above the ~90-passage corpus) the canonical row is
            // guaranteed to surface when it exists.
            let probe: String =
                q.1.chars()
                    .map(|c| if c.is_alphanumeric() { c } else { ' ' })
                    .collect();
            let params = RetrievalParams {
                top_k: 200,
                min_score: f64::NEG_INFINITY,
                source_filter: vec![],
            };
            let hits = kb.retrieve(&probe, &params).await.unwrap();
            let found = hits.iter().any(|p| p.id == q.1);
            assert!(
                found,
                "canonical_id {:?} for query {:?} not found in seed corpus (probed with {:?})",
                q.1, q.0, probe
            );
        }
    }
}
