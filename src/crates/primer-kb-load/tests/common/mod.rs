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
