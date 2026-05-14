//! German benchmark dataset for retrieval-quality integration tests.
//!
//! Mirrors `tests/common/mod.rs::QUERIES` for the `de` locale. The
//! Klexikon corpus (66 articles at the time of authoring; CC-BY-SA-4.0
//! children's wiki — the German analogue of Simple English Wikipedia)
//! is the only seed layer for `de`, so every `canonical_id` here points
//! at a `wiki-klexikon:de:*` passage. There is no separate hand-drafted
//! seed layer for German.
//!
//! Queries are child-style German phrasings. `required` substrings were
//! chosen against the lead text of each targeted passage so the loose
//! check measures retrieval recall and not just vocabulary alignment.
//! Substring matching runs on `text.to_lowercase()`, so German umlauts
//! flow through unchanged.
//!
//! The shared `Cluster` enum (`tests/common/mod.rs`) is reused. The
//! `Wiki` variant does not appear here because the entire German layer
//! IS the wiki — there is no parallel hand-drafted seed cluster.

use super::{BenchQuery, CANONICAL_PROBE_TOP_K, Cluster};

/// Canonical benchmark queries for the `de` locale. Targets the
/// 66-passage Klexikon corpus shipped at `data/seed/wiki_passages.de.jsonl`.
pub const QUERIES_DE: &[BenchQuery] = &[
    // ── Space cluster ─────────────────────────────────────────────
    BenchQuery {
        query: "warum scheint die sonne so hell",
        required: &["stern", "hitze"],
        canonical_id: Some("wiki-klexikon:de:sonne"),
        cluster: Cluster::Space,
    },
    BenchQuery {
        query: "was sind sterne",
        required: &["wasserstoff", "helium"],
        canonical_id: Some("wiki-klexikon:de:stern"),
        cluster: Cluster::Space,
    },
    BenchQuery {
        query: "warum wechselt der mond seine form",
        required: &["sonne", "beleuchtet"],
        canonical_id: Some("wiki-klexikon:de:mond"),
        cluster: Cluster::Space,
    },
    BenchQuery {
        query: "was ist eine galaxie",
        required: &["sterne", "weltall"],
        canonical_id: Some("wiki-klexikon:de:galaxie"),
        cluster: Cluster::Space,
    },
    BenchQuery {
        query: "warum gibt es jahreszeiten",
        required: &["achse", "schief"],
        canonical_id: Some("wiki-klexikon:de:jahreszeiten"),
        cluster: Cluster::Space,
    },
    // ── Body cluster ──────────────────────────────────────────────
    BenchQuery {
        query: "wie pumpt das herz das blut",
        required: &["muskel", "blut"],
        canonical_id: Some("wiki-klexikon:de:herz"),
        cluster: Cluster::Body,
    },
    BenchQuery {
        query: "wozu brauchen wir die lunge",
        required: &["sauerstoff", "kohlendioxid"],
        canonical_id: Some("wiki-klexikon:de:lunge"),
        cluster: Cluster::Body,
    },
    BenchQuery {
        query: "was macht das gehirn in unserem kopf",
        required: &["organ", "informationen"],
        canonical_id: Some("wiki-klexikon:de:gehirn"),
        cluster: Cluster::Body,
    },
    BenchQuery {
        query: "wie verdaut der körper das essen",
        required: &["nahrung", "zerlegen"],
        canonical_id: Some("wiki-klexikon:de:verdauung"),
        cluster: Cluster::Body,
    },
    BenchQuery {
        query: "wie viele knochen hat der mensch",
        required: &["skelett", "knochen"],
        canonical_id: None,
        cluster: Cluster::Body,
    },
    BenchQuery {
        query: "wie hören wir mit den ohren",
        required: &["geräusch"],
        canonical_id: None,
        cluster: Cluster::Body,
    },
    // ── How-things-work cluster ───────────────────────────────────
    BenchQuery {
        query: "wie funktioniert ein magnet",
        required: &["nordpol", "südpol"],
        canonical_id: Some("wiki-klexikon:de:magnet"),
        cluster: Cluster::HowThingsWork,
    },
    BenchQuery {
        query: "was ist elektrizität",
        required: &["strom", "kraft"],
        canonical_id: Some("wiki-klexikon:de:elektrizitat"),
        cluster: Cluster::HowThingsWork,
    },
    BenchQuery {
        query: "was ist schwerkraft",
        required: &["kraft", "boden"],
        canonical_id: Some("wiki-klexikon:de:schwerkraft"),
        cluster: Cluster::HowThingsWork,
    },
    BenchQuery {
        query: "wie entsteht ein feuer",
        required: &["sauerstoff", "verbrenn"],
        canonical_id: None,
        cluster: Cluster::HowThingsWork,
    },
    BenchQuery {
        query: "was ist schall",
        required: &["geräusch", "hören"],
        canonical_id: Some("wiki-klexikon:de:schall"),
        cluster: Cluster::HowThingsWork,
    },
    // ── Life cluster ──────────────────────────────────────────────
    BenchQuery {
        query: "wie wachsen pflanzen",
        required: &["wurzeln", "blätter"],
        canonical_id: Some("wiki-klexikon:de:pflanzen"),
        cluster: Cluster::Life,
    },
    BenchQuery {
        query: "wann sind die dinosaurier ausgestorben",
        required: &["millionen", "ausgestorben"],
        canonical_id: Some("wiki-klexikon:de:dinosaurier"),
        cluster: Cluster::Life,
    },
    // The earlier draft of this slot asked "wie viele beine haben
    // insekten" with `required: &["sechs"]`. That passed for the wrong
    // reason — the `wiki-klexikon:de:insekten` article never names the
    // leg count; "sechs Beine" only appears in `wiki-klexikon:de:bienen`.
    // Code review (PR #63) flagged the fragility: top-5 retrieval
    // happened to surface `bienen` (it mentions both "Insekten" AND
    // "sechs"), so the loose check was measuring "some passage in
    // top-K says sechs" rather than "the right passage answered the
    // question". Replaced with a question the insekten article DOES
    // answer directly, with a strict canonical_id pin.
    BenchQuery {
        query: "wie viele arten von insekten gibt es",
        required: &["million", "arten"],
        canonical_id: Some("wiki-klexikon:de:insekten"),
        cluster: Cluster::Life,
    },
    BenchQuery {
        query: "wie atmen fische unter wasser",
        required: &["kiemen", "wasser"],
        canonical_id: Some("wiki-klexikon:de:fische"),
        cluster: Cluster::Life,
    },
    BenchQuery {
        query: "wie fliegen vögel",
        required: &["flügel", "federn"],
        canonical_id: Some("wiki-klexikon:de:vogel"),
        cluster: Cluster::Life,
    },
    // ── Earth and weather cluster ─────────────────────────────────
    BenchQuery {
        query: "wie entsteht regen",
        required: &["tropfen", "wolken"],
        canonical_id: None,
        cluster: Cluster::EarthWeather,
    },
    BenchQuery {
        query: "wie entsteht ein vulkan",
        required: &["gestein", "krater"],
        canonical_id: Some("wiki-klexikon:de:vulkan"),
        cluster: Cluster::EarthWeather,
    },
    BenchQuery {
        query: "warum gibt es erdbeben",
        required: &["bewegung"],
        canonical_id: None,
        cluster: Cluster::EarthWeather,
    },
    BenchQuery {
        query: "wie entstehen wolken",
        required: &["wasser", "kondens"],
        canonical_id: Some("wiki-klexikon:de:wolke"),
        cluster: Cluster::EarthWeather,
    },
    BenchQuery {
        query: "wie entsteht wind",
        required: &["luft", "luftdruck"],
        canonical_id: None,
        cluster: Cluster::EarthWeather,
    },
    // ── Child-language stress paraphrases (issue #64) ─────────────
    // Authored to use child-style vocabulary that deliberately does
    // NOT appear in the canonical Klexikon article's lead text. The
    // BM25 leg is therefore measured against vocabulary it was not
    // given, motivating the hybrid lift. Mirrors the EN issue #45
    // paraphrase shape ("what makes my tummy growl…", etc.). Required
    // substrings are drawn from the canonical article's lead so the
    // loose check measures "retrieval surfaced the right article" not
    // "the query happened to share vocabulary with the corpus".
    BenchQuery {
        query: "warum macht mein bauch komische geräusche wenn ich hunger habe",
        required: &["nahrung", "darm"],
        canonical_id: Some("wiki-klexikon:de:verdauung"),
        cluster: Cluster::Body,
    },
    BenchQuery {
        query: "wieso wird es im sommer länger hell als im winter",
        required: &["achse", "schief"],
        canonical_id: Some("wiki-klexikon:de:jahreszeiten"),
        cluster: Cluster::Space,
    },
    BenchQuery {
        query: "warum bekomme ich gänsehaut wenn mir kalt ist",
        required: &["organ", "pigment"],
        canonical_id: Some("wiki-klexikon:de:haut"),
        cluster: Cluster::Body,
    },
    BenchQuery {
        query: "warum gibt es ebbe und flut am meer",
        required: &["satellit", "kreist"],
        canonical_id: Some("wiki-klexikon:de:mond"),
        cluster: Cluster::Space,
    },
    BenchQuery {
        query: "warum sind blätter im herbst bunt",
        required: &["laub", "herbst"],
        canonical_id: Some("wiki-klexikon:de:baum"),
        cluster: Cluster::Life,
    },
];

/// German queries the production BM25-only retrieval defaults
/// (`KB_FINAL_TOP_K`, `KB_BM25_ONLY_MIN_SCORE`) cannot satisfy. Kept in
/// `QUERIES_DE` rather than deleted so the sweep diagnostic still
/// measures against them. The BM25-only regression test in
/// `retrieval_quality_de.rs` excludes these from its assertion.
///
/// Populated by running the BM25-only test against the dataset; each
/// entry carries a one-line rationale pointing at the corpus gap or
/// query-vocabulary mismatch.
pub const KNOWN_FAILING_QUERIES_DE: &[&str] = &[
    // Issue #64 stress paraphrases. BM25 at production defaults
    // (top_k=5, min_score=0.5) surfaces lexically adjacent passages
    // rather than the semantically correct one. The hybrid path lifts
    // them via the dense leg — see retrieval_quality_hybrid_de.rs and
    // the empty KNOWN_FAILING_QUERIES_DE_HYBRID list below.
    //
    // Lexical near-misses observed when run on the 66-passage corpus:
    //   bauch-geräusche  → energie/schall/saugetiere/planet/ohr
    //   gänsehaut-kalt   → energie/planet/klima/temperatur/schnee
    //   ebbe-flut-meer   → meer/fluss/wetter/insekten/fische
    "warum macht mein bauch komische geräusche wenn ich hunger habe",
    "warum bekomme ich gänsehaut wenn mir kalt ist",
    "warum gibt es ebbe und flut am meer",
];

/// German queries the hybrid (BM25 + dense-vector RRF) retrieval path
/// with production `HybridParams::default()` cannot satisfy. Every entry
/// represents either a real semantic regression or a corpus-coverage gap
/// the dense leg cannot bridge — investigate before adding.
pub const KNOWN_FAILING_QUERIES_DE_HYBRID: &[&str] = &[
    // Issue #64 corpus-coverage gaps. The hybrid path correctly
    // matches the *semantic context* (kalt/Temperatur for gänsehaut;
    // Meer/Wasser for ebbe-flut) but the canonical Klexikon article
    // never discusses the targeted reflex/phenomenon, so no embedding
    // can bridge to it:
    //
    //   • `wiki-klexikon:de:haut` does not describe the goosebumps
    //     reflex — only skin layers, pigments, and Sonnenbrand. The
    //     hybrid path returns temperatur/klima/schnee/wüste/feuer.
    //
    //   • `wiki-klexikon:de:mond` does not discuss tides — only
    //     orbit, phases, eclipses, and other planets' moons. The
    //     hybrid path returns fluss/meer/wetter/regen/fische.
    //
    // Mirrors the EN-side precedent: pre-issue-#45, the EN benchmark
    // had similar corpus gaps that motivated hand-drafted seed
    // expansion. Resolution for DE is option (b) from issue #64:
    // leave with rationale until/unless a hand-drafted German seed
    // layer ships alongside Klexikon — there is no current plan to
    // do so (Klexikon IS the children's wiki for German).
    "warum bekomme ich gänsehaut wenn mir kalt ist",
    "warum gibt es ebbe und flut am meer",
];

#[cfg(test)]
mod sanity_tests {
    use super::*;

    #[test]
    fn queries_de_have_expected_size_floor() {
        // Brief specified ~25 queries as the target. Set a defensive
        // lower bound of 20 to catch accidental truncation.
        assert!(
            QUERIES_DE.len() >= 20,
            "expected at least 20 DE benchmark queries, got {}",
            QUERIES_DE.len()
        );
    }

    #[test]
    fn every_de_cluster_has_at_least_three_queries() {
        // Wiki cluster is excluded — the Klexikon layer IS the
        // German wiki, so it does not have a separate hand-drafted
        // seed cluster sitting alongside it. **If a German hand-drafted
        // seed layer is ever added (mirroring the EN seed_passages /
        // wiki_passages split), add `Cluster::Wiki` to this list so
        // wiki-targeted DE queries get the same per-cluster floor
        // guarantee.**
        for cluster in [
            Cluster::Space,
            Cluster::Body,
            Cluster::HowThingsWork,
            Cluster::Life,
            Cluster::EarthWeather,
        ] {
            let count = QUERIES_DE.iter().filter(|q| q.cluster == cluster).count();
            assert!(
                count >= 3,
                "DE cluster {cluster:?} has only {count} queries; need at least 3 for per-cluster recall to be meaningful"
            );
        }
    }

    #[test]
    fn de_strict_subset_meets_floor() {
        // 20 strict canonical-id mappings ship today (PR #63 + the
        // insekten-query fix); the EN-side floor is ≥15 against ~24
        // mappings (a ~60% gap), so match that tightness ratio rather
        // than the looser 50% gap from the initial PR. Tuning could
        // remove up to 5 strict mappings before this trips — meaningful
        // signal preserved without forcing every tuning experiment to
        // edit this floor.
        let strict_count = QUERIES_DE
            .iter()
            .filter(|q| q.canonical_id.is_some())
            .count();
        assert!(
            strict_count >= 15,
            "DE strict subset has only {strict_count} entries; need at least 15 for the recall signal"
        );
    }

    #[test]
    fn de_known_failing_queries_all_appear_in_queries() {
        // Mirrors the EN equivalent: a typo in KNOWN_FAILING_QUERIES_DE
        // would silently disable nothing. Fails loudly if an entry
        // doesn't match a real benchmark query.
        for failing in KNOWN_FAILING_QUERIES_DE {
            let found = QUERIES_DE.iter().any(|q| q.query == *failing);
            assert!(
                found,
                "KNOWN_FAILING_QUERIES_DE entry {failing:?} does not appear in QUERIES_DE — typo or stale entry"
            );
        }
        for failing in KNOWN_FAILING_QUERIES_DE_HYBRID {
            let found = QUERIES_DE.iter().any(|q| q.query == *failing);
            assert!(
                found,
                "KNOWN_FAILING_QUERIES_DE_HYBRID entry {failing:?} does not appear in QUERIES_DE — typo or stale entry"
            );
        }
    }

    #[tokio::test]
    async fn de_strict_subset_canonical_ids_exist_in_corpus() {
        // Mirrors the EN equivalent: every declared canonical_id must
        // exist in the shipped Klexikon corpus, otherwise strict
        // recall measures against ghost ids. Open a temp `de`-locale
        // KB, auto-seed, then probe each id with the same word-parts
        // workaround the EN test uses.
        use primer_core::i18n::Locale;
        use primer_core::knowledge::{KnowledgeBase, RetrievalParams};
        use primer_kb_load::auto_seed_if_empty;
        use primer_knowledge::SqliteKnowledgeBase;

        let db = tempfile::NamedTempFile::new().unwrap();
        let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::German).unwrap();
        auto_seed_if_empty(&kb, Locale::German)
            .await
            .unwrap()
            .unwrap();

        for q in QUERIES_DE
            .iter()
            .filter_map(|q| q.canonical_id.map(|id| (q.query, id)))
        {
            let probe: String =
                q.1.chars()
                    .map(|c| if c.is_alphanumeric() { c } else { ' ' })
                    .collect();
            let params = RetrievalParams {
                top_k: CANONICAL_PROBE_TOP_K,
                min_score: f64::NEG_INFINITY,
                source_filter: vec![],
            };
            let hits = kb.retrieve(&probe, &params).await.unwrap();
            let found = hits.iter().any(|p| p.id == q.1);
            assert!(
                found,
                "canonical_id {:?} for query {:?} not found in DE seed corpus (probed with {:?})",
                q.1, q.0, probe
            );
        }
    }
}
