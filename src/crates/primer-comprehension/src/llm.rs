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

    // The depth ladder offered to the model deliberately excludes
    // `Unknown`: a small model told "use Unknown when unsure" emits
    // noise rows instead of omitting. The parser still accepts
    // "Unknown" defensively (monotonic-max apply means it can never
    // demote), but the prompt only ever asks for the five real levels.
    let depth_levels = UnderstandingDepth::ALL
        .iter()
        .filter(|d| **d != UnderstandingDepth::Unknown)
        .map(|d| d.name())
        .collect::<Vec<_>>()
        .join(", ");

    // Written for small local models: one-line rules, a literal example
    // output, and the JSON-only instruction repeated at the end of the
    // user message where recency keeps it salient.
    let system = format!(
        "You assess how deeply a child understands specific concepts, based on what the \
        child actually said in a learning conversation.\n\n\
        Depth levels (shallowest to deepest):\n\
        - Aware: the concept registered — the child named it back or asked about it.\n\
        - Recall: the child stated a fact about it.\n\
        - Comprehension: the child explained it in their own words.\n\
        - Application: the child applied it to a new situation.\n\
        - Analysis: the child compared, contrasted, or reasoned about it.\n\n\
        Rules:\n\
        - Judge ONLY from the child's own words. What the Primer said is not evidence.\n\
        - Copy each concept name EXACTLY as it appears in the candidate list.\n\
        - \"depth\" is exactly one of: {depth_levels}.\n\
        - \"confidence\" is a number between 0.0 and 1.0.\n\
        - \"evidence\" is one short sentence quoting or paraphrasing what the child said.\n\
        - If the child did not engage with a concept, or only said \"yeah\"/\"ok\"/\"I see\", \
        leave that concept out of the list. When unsure, leave it out.\n\
        - It is fine to return {{\"assessments\": []}}.\n\
        - Output ONLY one JSON object. No other text, no markdown fences.\n\n\
        Example output:\n\
        {{\"assessments\": [{{\"concept\": \"gravity\", \"depth\": \"Comprehension\", \
        \"confidence\": 0.8, \"evidence\": \"Explained in their own words that heavier things do not fall faster.\"}}]}}"
    );

    let user = format!(
        "Prior context (oldest first):\n{context_section}\n\nExchange to analyse:\n{}: {}\n{}: {}\n\nCandidate concepts to assess (only these — do not invent others):\n{candidates_section}\n\nAssess now. Answer with the JSON object only.",
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
        // Match the concept against the candidate list (defends against
        // the LLM inventing concepts). Matching is trimmed and
        // case-insensitive — small local models routinely echo
        // "Photosynthesis" for the candidate "photosynthesis" — but the
        // *canonical* candidate string is what lands in the assessment,
        // so downstream concept ids stay consistent with the extractor's
        // normalized form.
        let concept_returned = a.concept.trim().to_lowercase();
        let canonical = match candidates
            .iter()
            .find(|c| c.trim().to_lowercase() == concept_returned)
        {
            Some(c) => c.clone(),
            None => continue,
        };
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
            concept: canonical,
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
    async fn classify_matches_candidates_case_insensitively_and_canonicalizes() {
        // Small local models routinely echo "Photosynthesis" for the
        // candidate "photosynthesis". The match is case-insensitive and
        // the *canonical* candidate string is what lands in the
        // assessment, keeping concept ids consistent downstream.
        let backend = Arc::new(CannedBackend {
            name: "x",
            text: r#"{"assessments": [{"concept": " Photosynthesis ", "depth": "Recall", "confidence": 0.7}]}"#.into(),
        }) as Arc<dyn InferenceBackend>;
        let c = LlmComprehensionClassifier::new(
            backend,
            "test".into(),
            ComprehensionSettings::default(),
        );
        let child = turn(Speaker::Child, "?");
        let primer = turn(Speaker::Primer, "?");
        let candidates: Vec<String> = vec!["photosynthesis".into()];
        let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert_eq!(r.assessments.len(), 1);
        assert_eq!(r.assessments[0].concept, "photosynthesis");
        assert_eq!(r.assessments[0].depth, UnderstandingDepth::Recall);
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
