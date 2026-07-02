use super::*;
use chrono::Utc;
use futures::stream;
use primer_core::conversation::{Speaker, Turn};
use primer_core::inference::{GenerationParams, Prompt, TokenChunk, TokenStream};

/// Test backend that returns canned text from `generate_stream` and stubs
/// `name` / `is_available`.
struct CannedBackend {
    name: &'static str,
    text: String,
}

#[async_trait]
impl InferenceBackend for CannedBackend {
    fn name(&self) -> &str {
        self.name
    }
    async fn is_available(&self) -> bool {
        true
    }
    async fn generate_stream(
        &self,
        _prompt: &Prompt,
        _params: &GenerationParams,
    ) -> Result<TokenStream> {
        let chunk = TokenChunk {
            text: self.text.clone(),
            done: true,
            ..Default::default()
        };
        Ok(Box::pin(stream::once(async move { Ok(chunk) })))
    }
}

fn turn(speaker: Speaker, text: &str) -> Turn {
    Turn {
        speaker,
        text: text.into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    }
}

fn ctx<'a>(
    child: &'a Turn,
    primer: &'a Turn,
    candidates: &'a [String],
) -> ComprehensionContext<'a> {
    ComprehensionContext {
        child_turn: child,
        primer_turn: primer,
        recent_turns: &[],
        candidate_concepts: candidates,
    }
}

#[test]
fn identifier_includes_backend_and_model() {
    let backend = Arc::new(CannedBackend {
        name: "ollama",
        text: "{}".into(),
    }) as Arc<dyn InferenceBackend>;
    let c =
        LlmComprehensionClassifier::new(backend, "haiku".into(), ComprehensionSettings::default());
    assert_eq!(c.identifier(), "llm:ollama:haiku");
}

#[tokio::test]
async fn classify_parses_well_formed_json() {
    let backend = Arc::new(CannedBackend {
        name: "x",
        text: r#"{"assessments": [{"concept": "photosynthesis", "depth": "Comprehension", "confidence": 0.85, "evidence": "explained well"}]}"#.into(),
    }) as Arc<dyn InferenceBackend>;
    let c = LlmComprehensionClassifier::new(
        backend,
        "test-model".into(),
        ComprehensionSettings::default(),
    );
    let child = turn(Speaker::Child, "Plants make food from sunlight.");
    let primer = turn(Speaker::Primer, "What do you think they need?");
    let candidates: Vec<String> = vec!["photosynthesis".into()];
    let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
    assert_eq!(r.assessments.len(), 1);
    assert_eq!(r.assessments[0].concept, "photosynthesis");
    assert_eq!(r.assessments[0].depth, UnderstandingDepth::Comprehension);
    assert!((r.assessments[0].confidence - 0.85).abs() < 1e-6);
    assert_eq!(r.assessments[0].evidence.as_deref(), Some("explained well"));
}

#[tokio::test]
async fn classify_handles_wrapper_text() {
    let backend = Arc::new(CannedBackend {
        name: "x",
        text: r#"sure: {"assessments": [{"concept": "gravity", "depth": "Aware", "confidence": 0.6}]} ok?"#.into(),
    }) as Arc<dyn InferenceBackend>;
    let c =
        LlmComprehensionClassifier::new(backend, "test".into(), ComprehensionSettings::default());
    let child = turn(Speaker::Child, "?");
    let primer = turn(Speaker::Primer, "?");
    let candidates: Vec<String> = vec!["gravity".into()];
    let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
    assert_eq!(r.assessments.len(), 1);
    assert_eq!(r.assessments[0].depth, UnderstandingDepth::Aware);
}

#[tokio::test]
async fn classify_skips_when_candidates_empty() {
    // No backend call should be made; canned text is irrelevant.
    let backend = Arc::new(CannedBackend {
        name: "x",
        text: "{}".into(),
    }) as Arc<dyn InferenceBackend>;
    let c =
        LlmComprehensionClassifier::new(backend, "test".into(), ComprehensionSettings::default());
    let child = turn(Speaker::Child, "?");
    let primer = turn(Speaker::Primer, "?");
    let candidates: Vec<String> = vec![];
    let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
    assert!(r.assessments.is_empty());
}

#[tokio::test]
async fn classify_drops_assessment_with_concept_not_in_candidates() {
    let backend = Arc::new(CannedBackend {
        name: "x",
        text: r#"{"assessments": [{"concept": "invented", "depth": "Aware", "confidence": 0.9}]}"#
            .into(),
    }) as Arc<dyn InferenceBackend>;
    let c =
        LlmComprehensionClassifier::new(backend, "test".into(), ComprehensionSettings::default());
    let child = turn(Speaker::Child, "?");
    let primer = turn(Speaker::Primer, "?");
    let candidates: Vec<String> = vec!["other".into()];
    let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
    assert!(r.assessments.is_empty());
}

#[tokio::test]
async fn classify_matches_candidates_case_insensitively_and_canonicalizes() {
    // Small local models routinely echo "Photosynthesis" for the
    // candidate "photosynthesis". The match is case-insensitive and
    // the *canonical* candidate string is what lands in the
    // assessment, keeping concept ids consistent downstream.
    let backend = Arc::new(CannedBackend {
        name: "x",
        text: r#"{"assessments": [{"concept": " Photosynthesis ", "depth": "Recall", "confidence": 0.7}]}"#.into(),
    }) as Arc<dyn InferenceBackend>;
    let c =
        LlmComprehensionClassifier::new(backend, "test".into(), ComprehensionSettings::default());
    let child = turn(Speaker::Child, "?");
    let primer = turn(Speaker::Primer, "?");
    let candidates: Vec<String> = vec!["photosynthesis".into()];
    let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
    assert_eq!(r.assessments.len(), 1);
    assert_eq!(r.assessments[0].concept, "photosynthesis");
    assert_eq!(r.assessments[0].depth, UnderstandingDepth::Recall);
}

#[tokio::test]
async fn classify_ignores_echoed_prompt_example_before_real_answer() {
    // Small local models sometimes restate the prompt's example
    // before answering. First-object-wins parsing must not adopt
    // the echoed example as a real assessment — here "gravity" IS a
    // candidate, so without excision the example's Comprehension@0.8
    // row would win over the model's actual Aware@0.9 reading.
    let backend = Arc::new(CannedBackend {
        name: "x",
        text: format!(
            "{EXAMPLE_OUTPUT}\n{{\"assessments\": [{{\"concept\": \"gravity\", \"depth\": \"Aware\", \"confidence\": 0.9}}]}}"
        ),
    }) as Arc<dyn InferenceBackend>;
    let c =
        LlmComprehensionClassifier::new(backend, "test".into(), ComprehensionSettings::default());
    let child = turn(Speaker::Child, "?");
    let primer = turn(Speaker::Primer, "?");
    let candidates: Vec<String> = vec!["gravity".into()];
    let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
    assert_eq!(r.assessments.len(), 1);
    assert_eq!(r.assessments[0].depth, UnderstandingDepth::Aware);
    assert!((r.assessments[0].confidence - 0.9).abs() < 1e-6);
}

#[tokio::test]
async fn classify_collapses_duplicate_rows_to_strongest_reading() {
    // Case-insensitive matching means "Gravity" and "gravity" both
    // canonicalise to the candidate "gravity". Two surviving rows
    // would each run the Leitner-box transition downstream (a
    // double advance off one exchange), so duplicates collapse to
    // one assessment keeping the deepest depth (then highest
    // confidence).
    let backend = Arc::new(CannedBackend {
        name: "x",
        text: r#"{"assessments": [
            {"concept": "Gravity", "depth": "Recall", "confidence": 0.9},
            {"concept": "gravity", "depth": "Comprehension", "confidence": 0.7}
        ]}"#
        .into(),
    }) as Arc<dyn InferenceBackend>;
    let c =
        LlmComprehensionClassifier::new(backend, "test".into(), ComprehensionSettings::default());
    let child = turn(Speaker::Child, "?");
    let primer = turn(Speaker::Primer, "?");
    let candidates: Vec<String> = vec!["gravity".into()];
    let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
    assert_eq!(r.assessments.len(), 1);
    assert_eq!(r.assessments[0].concept, "gravity");
    assert_eq!(r.assessments[0].depth, UnderstandingDepth::Comprehension);
    assert!((r.assessments[0].confidence - 0.7).abs() < 1e-6);
}

#[tokio::test]
async fn classify_duplicate_dedup_prefers_above_threshold_row_over_deeper_subthreshold_row() {
    // The dedup must be threshold-aware: a sub-threshold Analysis
    // row must not displace an above-threshold Recall row, or the
    // turn's learner update (depth promotion + box advance) is
    // silently dropped in apply_comprehension's confidence gate.
    let backend = Arc::new(CannedBackend {
        name: "x",
        text: r#"{"assessments": [
            {"concept": "gravity", "depth": "Analysis", "confidence": 0.4},
            {"concept": "Gravity", "depth": "Recall", "confidence": 0.9}
        ]}"#
        .into(),
    }) as Arc<dyn InferenceBackend>;
    let c = LlmComprehensionClassifier::new(
        backend,
        "test".into(),
        ComprehensionSettings::default(), // confidence_threshold 0.6
    );
    let child = turn(Speaker::Child, "?");
    let primer = turn(Speaker::Primer, "?");
    let candidates: Vec<String> = vec!["gravity".into()];
    let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
    assert_eq!(r.assessments.len(), 1);
    assert_eq!(r.assessments[0].depth, UnderstandingDepth::Recall);
    assert!((r.assessments[0].confidence - 0.9).abs() < 1e-6);
}

#[tokio::test]
async fn classify_accepts_output_that_is_exactly_the_example() {
    // A model whose entire output is byte-identical to the prompt's
    // example is indistinguishable from a genuine identical answer.
    // The excision must fall back to the raw output instead of
    // soft-failing and dropping a possibly-correct assessment.
    let backend = Arc::new(CannedBackend {
        name: "x",
        text: EXAMPLE_OUTPUT.to_string(),
    }) as Arc<dyn InferenceBackend>;
    let c =
        LlmComprehensionClassifier::new(backend, "test".into(), ComprehensionSettings::default());
    let child = turn(Speaker::Child, "?");
    let primer = turn(Speaker::Primer, "?");
    let candidates: Vec<String> = vec!["gravity".into()];
    let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
    assert_eq!(r.assessments.len(), 1);
    assert_eq!(r.assessments[0].depth, UnderstandingDepth::Comprehension);
}

#[tokio::test]
async fn classify_drops_assessment_with_unknown_depth_name() {
    let backend = Arc::new(CannedBackend {
        name: "x",
        text: r#"{"assessments": [{"concept": "x", "depth": "MasteryGod", "confidence": 0.9}]}"#
            .into(),
    }) as Arc<dyn InferenceBackend>;
    let c =
        LlmComprehensionClassifier::new(backend, "test".into(), ComprehensionSettings::default());
    let child = turn(Speaker::Child, "?");
    let primer = turn(Speaker::Primer, "?");
    let candidates: Vec<String> = vec!["x".into()];
    let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
    assert!(r.assessments.is_empty());
}

#[tokio::test]
async fn classify_drops_assessment_with_out_of_range_confidence() {
    let backend = Arc::new(CannedBackend {
        name: "x",
        text: r#"{"assessments": [{"concept": "x", "depth": "Aware", "confidence": 1.5}]}"#.into(),
    }) as Arc<dyn InferenceBackend>;
    let c =
        LlmComprehensionClassifier::new(backend, "test".into(), ComprehensionSettings::default());
    let child = turn(Speaker::Child, "?");
    let primer = turn(Speaker::Primer, "?");
    let candidates: Vec<String> = vec!["x".into()];
    let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
    assert!(r.assessments.is_empty());
}

#[tokio::test]
async fn classify_truncates_evidence_to_cap() {
    let long_evidence = "z".repeat(500);
    let payload = serde_json::json!({
        "assessments": [{"concept": "x", "depth": "Aware", "confidence": 0.5, "evidence": long_evidence}]
    });
    let backend = Arc::new(CannedBackend {
        name: "x",
        text: payload.to_string(),
    }) as Arc<dyn InferenceBackend>;
    let settings = ComprehensionSettings {
        evidence_max_chars: 50,
        ..ComprehensionSettings::default()
    };
    let c = LlmComprehensionClassifier::new(backend, "test".into(), settings);
    let child = turn(Speaker::Child, "?");
    let primer = turn(Speaker::Primer, "?");
    let candidates: Vec<String> = vec!["x".into()];
    let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
    assert_eq!(r.assessments.len(), 1);
    let ev = r.assessments[0].evidence.as_deref().unwrap();
    assert_eq!(ev.chars().count(), 50);
}

#[tokio::test]
async fn classify_returns_empty_on_unparseable_output() {
    let backend = Arc::new(CannedBackend {
        name: "x",
        text: "not json".into(),
    }) as Arc<dyn InferenceBackend>;
    let c =
        LlmComprehensionClassifier::new(backend, "test".into(), ComprehensionSettings::default());
    let child = turn(Speaker::Child, "?");
    let primer = turn(Speaker::Primer, "?");
    let candidates: Vec<String> = vec!["x".into()];
    let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
    assert!(r.assessments.is_empty());
}

#[tokio::test]
async fn classify_returns_empty_on_backend_error() {
    struct ErrorBackend;
    #[async_trait]
    impl InferenceBackend for ErrorBackend {
        fn name(&self) -> &str {
            "error"
        }
        async fn is_available(&self) -> bool {
            true
        }
        async fn generate_stream(
            &self,
            _prompt: &Prompt,
            _params: &GenerationParams,
        ) -> Result<TokenStream> {
            Err(primer_core::error::PrimerError::Inference(
                "simulated".into(),
            ))
        }
    }
    let backend = Arc::new(ErrorBackend) as Arc<dyn InferenceBackend>;
    let c =
        LlmComprehensionClassifier::new(backend, "test".into(), ComprehensionSettings::default());
    let child = turn(Speaker::Child, "?");
    let primer = turn(Speaker::Primer, "?");
    let candidates: Vec<String> = vec!["x".into()];
    let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
    assert!(r.assessments.is_empty());
}
