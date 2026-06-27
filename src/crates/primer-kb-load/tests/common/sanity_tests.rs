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
            "cluster {cluster:?} has only {count} queries; need at least 3 for per-cluster recall to be meaningful"
        );
    }
}

#[test]
fn strict_subset_meets_floor() {
    let strict_count = QUERIES.iter().filter(|q| q.canonical_id.is_some()).count();
    assert!(
        strict_count >= 15,
        "strict subset has only {strict_count} entries; need at least 15 for the sweep's strict-recall signal"
    );
}

#[test]
fn known_failing_queries_all_appear_in_queries() {
    // Defensive: a typo in KNOWN_FAILING_QUERIES would silently
    // disable nothing — the regression test would still iterate
    // every BenchQuery in QUERIES because no string would match.
    // This test fails loudly if a known-failing entry doesn't
    // correspond to a real benchmark query (e.g. after a query
    // is reworded in QUERIES without updating KNOWN_FAILING).
    for failing in KNOWN_FAILING_QUERIES {
        let found = QUERIES.iter().any(|q| q.query == *failing);
        assert!(
            found,
            "KNOWN_FAILING_QUERIES entry {failing:?} does not appear in QUERIES — typo or stale entry"
        );
    }
    for failing in KNOWN_FAILING_QUERIES_HYBRID {
        let found = QUERIES.iter().any(|q| q.query == *failing);
        assert!(
            found,
            "KNOWN_FAILING_QUERIES_HYBRID entry {failing:?} does not appear in QUERIES — typo or stale entry"
        );
    }
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
        // "earthorbitdaynight" that matches nothing. With
        // `CANONICAL_PROBE_TOP_K` (well above corpus size) the
        // canonical row is guaranteed to surface when it exists.
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
            "canonical_id {:?} for query {:?} not found in seed corpus (probed with {:?})",
            q.1, q.0, probe
        );
    }
}
