//! Off-topic gate for hybrid retrieval (issue #307), under the real
//! BGE-M3 model. Everything here is gated on `--features fastembed`
//! (~570 MB model download on first run).
//!
//! * `off_topic_queries_are_gated_with_fastembed` — every off-topic
//!   utterance in `common::off_topic` retrieves nothing at the production
//!   gate, except the documented `KNOWN_LEAKING_OFF_TOPIC` entries, which
//!   must still leak (so the list can't go stale). Recall under the gate
//!   is pinned by `retrieval_quality_hybrid{,_de}.rs`.
//! * `off_topic_gate_probe` (`#[ignore]`) — prints every benchmark and
//!   off-topic query's best passage cosine, the numbers behind
//!   `KB_QUERY_GATE_MIN_COSINE_BGE_M3`. Re-run when the corpus or the
//!   model changes:
//!
//! ```text
//! ~/.cargo/bin/cargo test -p primer-kb-load --features fastembed \
//!     --test off_topic_gate -- --ignored off_topic_gate_probe --nocapture
//! ```

mod common;

#[cfg(feature = "fastembed")]
mod fastembed_tests {
    use crate::common::QUERIES;
    use crate::common::de::QUERIES_DE;
    use crate::common::off_topic::{KNOWN_LEAKING_OFF_TOPIC, OFF_TOPIC_DE, OFF_TOPIC_EN};
    use primer_core::embedder::Embedder;
    use primer_core::i18n::Locale;
    use primer_core::knowledge::{HybridParams, KnowledgeBase, kb_query_gate_min_cosine};
    use primer_embedding::FastEmbedBackend;
    use primer_kb_load::{auto_seed_if_empty, reembed_kb};
    use primer_knowledge::SqliteKnowledgeBase;

    /// A seeded + embedded KB; the temp file guard must outlive the KB.
    async fn embedded_kb(
        locale: Locale,
        embedder: &FastEmbedBackend,
    ) -> (tempfile::NamedTempFile, SqliteKnowledgeBase) {
        let db = tempfile::NamedTempFile::new().unwrap();
        let kb = SqliteKnowledgeBase::open_for_locale(db.path(), locale).unwrap();
        auto_seed_if_empty(&kb, locale)
            .await
            .unwrap()
            .expect("seed dir must contain the locale's *.jsonl");
        reembed_kb(&kb, embedder, false, 32).await.unwrap();
        (db, kb)
    }

    #[tokio::test]
    async fn off_topic_queries_are_gated_with_fastembed() {
        let embedder = FastEmbedBackend::new().expect("BGE-M3 init failed (network or ort?)");
        let params = HybridParams {
            min_top_cosine: kb_query_gate_min_cosine(embedder.model_id()),
            ..HybridParams::default()
        };
        assert!(params.min_top_cosine.is_some(), "BGE-M3 must be calibrated");

        let mut failures = Vec::new();
        for (locale, queries) in [
            (Locale::English, OFF_TOPIC_EN),
            (Locale::German, OFF_TOPIC_DE),
        ] {
            let (_db, kb) = embedded_kb(locale, &embedder).await;
            for q in queries {
                let hits = kb.retrieve_hybrid(q, &embedder, &params).await.unwrap();
                let leaks = KNOWN_LEAKING_OFF_TOPIC.contains(q);
                if leaks == hits.is_empty() {
                    failures.push(format!(
                        "{q:?}: {} hits, known-leaking = {leaks}",
                        hits.len()
                    ));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "off-topic gate drift (update KNOWN_LEAKING_OFF_TOPIC or re-run off_topic_gate_probe):\n  {}",
            failures.join("\n  ")
        );
    }

    /// Best cosine of `q` against `passages` (the value the gate compares).
    fn best_cosine(q: &[f32], passages: &[Vec<f32>]) -> f32 {
        let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
        passages
            .iter()
            .map(|p| q.iter().zip(p).map(|(a, b)| a * b).sum::<f32>() / (norm(q) * norm(p)))
            .fold(f32::MIN, f32::max)
    }

    #[tokio::test]
    #[ignore]
    async fn off_topic_gate_probe() {
        let embedder = FastEmbedBackend::new().expect("BGE-M3 init failed (network or ort?)");
        let en: Vec<&str> = QUERIES.iter().map(|q| q.query).collect();
        let de: Vec<&str> = QUERIES_DE.iter().map(|q| q.query).collect();
        for (locale, on, off) in [
            (Locale::English, en, OFF_TOPIC_EN),
            (Locale::German, de, OFF_TOPIC_DE),
        ] {
            let texts: Vec<String> = primer_kb_load::discover_seed_files(locale)
                .iter()
                .flat_map(|f| {
                    std::fs::read_to_string(f)
                        .unwrap()
                        .lines()
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .filter(|l| l.trim_start().starts_with('{'))
                .map(|l| {
                    serde_json::from_str::<serde_json::Value>(&l).unwrap()["text"]
                        .as_str()
                        .unwrap()
                        .to_owned()
                })
                .collect();
            let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
            let passages = embedder.embed(&refs).await.unwrap();
            for (tag, queries) in [("on-topic", on.as_slice()), ("off-topic", off)] {
                let vecs = embedder.embed(queries).await.unwrap();
                let mut scored: Vec<(f32, &str)> = vecs
                    .iter()
                    .map(|v| best_cosine(v, &passages))
                    .zip(queries.iter().copied())
                    .collect();
                scored.sort_by(|a, b| a.0.total_cmp(&b.0));
                println!("== {locale:?} {tag}: {} queries", scored.len());
                for (c, q) in scored {
                    println!("  {c:.3}  {q}");
                }
            }
        }
    }
}
