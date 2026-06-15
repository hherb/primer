//! LLM-backed comprehension classifier. Wraps any `InferenceBackend`
//! (via `Arc<dyn ...>`) so the chat backend can be reused or a cheaper
//! comprehension-specific model can be swapped in independently.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::warn;

use primer_core::comprehension::{
    ComprehensionAssessment, ComprehensionContext, ComprehensionResult,
};
use primer_core::conversation::Speaker;
use primer_core::error::Result;
use primer_core::inference::{InferenceBackend, Message, Prompt, Role};
use primer_core::learner::UnderstandingDepth;
use primer_core::llm_util::{extract_first_json_object, truncate_to_chars};

use crate::ComprehensionClassifier;
use crate::consts;
use crate::settings::ComprehensionSettings;

pub struct LlmComprehensionClassifier {
    backend: Arc<dyn InferenceBackend>,
    settings: ComprehensionSettings,
    identifier: String,
}

impl LlmComprehensionClassifier {
    /// Build a new LLM-backed comprehension classifier.
    ///
    /// The stable `identifier` is composed as `llm:{backend}:{model}`,
    /// matching the format used by `LlmConceptExtractor`.
    pub fn new(
        backend: Arc<dyn InferenceBackend>,
        model: String,
        settings: ComprehensionSettings,
    ) -> Self {
        let identifier = format!("llm:{}:{}", backend.name(), model);
        Self {
            backend,
            settings,
            identifier,
        }
    }
}

#[async_trait]
impl ComprehensionClassifier for LlmComprehensionClassifier {
    fn identifier(&self) -> &str {
        &self.identifier
    }

    async fn classify(&self, ctx: ComprehensionContext<'_>) -> Result<ComprehensionResult> {
        if ctx.candidate_concepts.is_empty() {
            return Ok(ComprehensionResult::empty());
        }

        let prompt = build_classification_prompt(&ctx);
        let params = primer_core::inference::GenerationParams {
            max_tokens: self.settings.generation_max_tokens,
            temperature: self.settings.generation_temperature,
            top_p: self.settings.generation_top_p,
            stop_sequences: vec![],
            routing: None,
        };
        let raw = match self.backend.generate(&prompt, &params).await {
            Ok(r) => r,
            Err(e) => {
                warn!(classifier = %self.identifier, error = ?e, "comprehension backend call failed");
                return Ok(ComprehensionResult::empty());
            }
        };

        let truncated = truncate_to_chars(&raw, self.settings.max_output_chars);
        match parse_classification_output(
            truncated,
            ctx.candidate_concepts,
            self.settings.evidence_max_chars,
        ) {
            Ok(r) => Ok(r),
            Err(reason) => {
                let snippet: String = truncated
                    .chars()
                    .take(consts::LLM_DEBUG_SNIPPET_CHARS)
                    .collect();
                warn!(classifier = %self.identifier, raw = %snippet, %reason, "comprehension output unparseable");
                Ok(ComprehensionResult::empty())
            }
        }
    }
}

// ─── Prompt builder ───────────────────────────────────────────────────────────

fn build_classification_prompt(ctx: &ComprehensionContext) -> Prompt {
    let context_section = if ctx.recent_turns.is_empty() {
        "(no prior context)".to_string()
    } else {
        ctx.recent_turns
            .iter()
            .map(|t| format!("{}: {}", speaker_label(t.speaker), t.text))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let candidates_section = ctx
        .candidate_concepts
        .iter()
        .map(|c| format!("- {c}"))
        .collect::<Vec<_>>()
        .join("\n");

    let depth_levels = UnderstandingDepth::ALL
        .iter()
        .map(|d| d.name())
        .collect::<Vec<_>>()
        .join(", ");

    let system = format!(
        "You assess a child's depth of understanding for specific concepts in a Socratic \
        learning conversation. Output ONLY valid JSON of the form: \
        {{\"assessments\": [{{\"concept\": \"<from candidates>\", \"depth\": \"<one of: {depth_levels}>\", \
        \"confidence\": 0.0-1.0, \"evidence\": \"<one short sentence>\"}}]}} — no other text. \
        \
        Only emit assessments for concepts where the CHILD's turn shows actual demonstration. \
        Concepts the Primer introduced but the child did not engage with should be OMITTED — not \
        assessed at the lowest level. A bare acknowledgement (\"yeah\", \"ok\", \"I see\") is not \
        evidence; omit those concepts too. \
        \
        Depth meanings: \
        - Aware: child shows the concept registered (named it back, asked about it). \
        - Recall: child stated a fact about it. \
        - Comprehension: child explained it in their own words. \
        - Application: child transferred it to a new context they hadn't seen. \
        - Analysis: child compared, contrasted, or reasoned about it. \
        \
        Use Unknown only if you have no evidence at all (you should normally omit instead)."
    );

    let user = format!(
        "Prior context (oldest first):\n{context_section}\n\nExchange to analyse:\n{}: {}\n{}: {}\n\nCandidate concepts to assess (only these — do not invent others):\n{candidates_section}\n\nAssess now.",
        speaker_label(ctx.child_turn.speaker),
        ctx.child_turn.text,
        speaker_label(ctx.primer_turn.speaker),
        ctx.primer_turn.text,
    );

    Prompt {
        system,
        messages: vec![Message {
            role: Role::User,
            content: user,
        }],
    }
}

fn speaker_label(s: Speaker) -> &'static str {
    match s {
        Speaker::Child => "Child",
        Speaker::Primer => "Primer",
    }
}

// ─── Output parser ─────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct ClassifierOutput {
    #[serde(default)]
    assessments: Vec<RawAssessment>,
}

#[derive(serde::Deserialize)]
struct RawAssessment {
    concept: String,
    depth: String,
    confidence: f32,
    #[serde(default)]
    evidence: Option<String>,
}

fn parse_classification_output(
    raw: &str,
    candidates: &[String],
    evidence_max_chars: usize,
) -> std::result::Result<ComprehensionResult, String> {
    let json_str = extract_first_json_object(raw)
        .ok_or_else(|| "no JSON object found in output".to_string())?;
    let parsed: ClassifierOutput =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {e}"))?;

    let mut out: Vec<ComprehensionAssessment> = Vec::with_capacity(parsed.assessments.len());
    for a in parsed.assessments {
        // Drop assessments whose concept name isn't in the candidate
        // list (defends against the LLM inventing concepts).
        if !candidates.iter().any(|c| c == &a.concept) {
            continue;
        }
        // Drop assessments with out-of-range confidence.
        if !(0.0..=1.0).contains(&a.confidence) {
            continue;
        }
        // Drop assessments with unknown depth name.
        let depth = match depth_from_name(&a.depth) {
            Some(d) => d,
            None => continue,
        };
        let evidence = a.evidence.map(|e| {
            let trimmed = truncate_to_chars(&e, evidence_max_chars);
            trimmed.to_string()
        });
        out.push(ComprehensionAssessment {
            concept: a.concept,
            depth,
            confidence: a.confidence,
            evidence,
        });
    }

    Ok(ComprehensionResult { assessments: out })
}

fn depth_from_name(name: &str) -> Option<UnderstandingDepth> {
    UnderstandingDepth::ALL
        .iter()
        .copied()
        .find(|d| d.name() == name)
}

// ─── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
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
        let c = LlmComprehensionClassifier::new(
            backend,
            "haiku".into(),
            ComprehensionSettings::default(),
        );
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
        let c = LlmComprehensionClassifier::new(
            backend,
            "test".into(),
            ComprehensionSettings::default(),
        );
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
        let c = LlmComprehensionClassifier::new(
            backend,
            "test".into(),
            ComprehensionSettings::default(),
        );
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
            text:
                r#"{"assessments": [{"concept": "invented", "depth": "Aware", "confidence": 0.9}]}"#
                    .into(),
        }) as Arc<dyn InferenceBackend>;
        let c = LlmComprehensionClassifier::new(
            backend,
            "test".into(),
            ComprehensionSettings::default(),
        );
        let child = turn(Speaker::Child, "?");
        let primer = turn(Speaker::Primer, "?");
        let candidates: Vec<String> = vec!["other".into()];
        let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert!(r.assessments.is_empty());
    }

    #[tokio::test]
    async fn classify_drops_assessment_with_unknown_depth_name() {
        let backend = Arc::new(CannedBackend {
            name: "x",
            text:
                r#"{"assessments": [{"concept": "x", "depth": "MasteryGod", "confidence": 0.9}]}"#
                    .into(),
        }) as Arc<dyn InferenceBackend>;
        let c = LlmComprehensionClassifier::new(
            backend,
            "test".into(),
            ComprehensionSettings::default(),
        );
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
            text: r#"{"assessments": [{"concept": "x", "depth": "Aware", "confidence": 1.5}]}"#
                .into(),
        }) as Arc<dyn InferenceBackend>;
        let c = LlmComprehensionClassifier::new(
            backend,
            "test".into(),
            ComprehensionSettings::default(),
        );
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
        let c = LlmComprehensionClassifier::new(
            backend,
            "test".into(),
            ComprehensionSettings::default(),
        );
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
        let c = LlmComprehensionClassifier::new(
            backend,
            "test".into(),
            ComprehensionSettings::default(),
        );
        let child = turn(Speaker::Child, "?");
        let primer = turn(Speaker::Primer, "?");
        let candidates: Vec<String> = vec!["x".into()];
        let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert!(r.assessments.is_empty());
    }
}
