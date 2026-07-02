//! LLM-backed engagement classifier. Wraps any `InferenceBackend`
//! (via Arc<dyn ...>) so memory-constrained devices can reuse the chat
//! backend, while bigger machines can use a separate small model.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::warn;

use primer_core::classifier::{EngagementAssessment, EngagementContext};
use primer_core::error::Result;
use primer_core::inference::InferenceBackend;
use primer_core::llm_util::{extract_first_json_object, truncate_to_chars};

use crate::EngagementClassifier;
use crate::settings::ClassifierSettings;

pub struct LlmEngagementClassifier {
    backend: Arc<dyn InferenceBackend>,
    settings: ClassifierSettings,
    identifier: String,
}

impl LlmEngagementClassifier {
    pub fn new(
        backend: Arc<dyn InferenceBackend>,
        model: String,
        settings: ClassifierSettings,
    ) -> Self {
        let identifier = format!("llm:{model}");
        Self {
            backend,
            settings,
            identifier,
        }
    }
}

#[async_trait]
impl EngagementClassifier for LlmEngagementClassifier {
    fn identifier(&self) -> &str {
        &self.identifier
    }

    async fn classify(&self, ctx: EngagementContext<'_>) -> Result<EngagementAssessment> {
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
                warn!(classifier = %self.identifier, error = ?e, "classifier backend call failed");
                return Ok(EngagementAssessment::unknown_low_confidence(format!(
                    "backend error: {e}"
                )));
            }
        };

        // Truncate to settings.max_output_chars (char-boundary-safe) before parsing.
        let truncated = truncate_to_chars(&raw, self.settings.max_output_chars);

        match parse_classification_output(truncated) {
            Ok(a) => Ok(a),
            Err(reason) => {
                let snippet: String = truncated.chars().take(200).collect();
                warn!(classifier = %self.identifier, raw = %snippet, %reason, "classifier output unparseable");
                Ok(EngagementAssessment::unknown_low_confidence(reason))
            }
        }
    }
}

// ── Prompt builder ────────────────────────────────────────────────────────────

use primer_core::conversation::Speaker;
use primer_core::inference::{Message, Prompt, Role};
use primer_core::learner::EngagementState;

/// Literal example JSON embedded in the classification prompt. Kept as
/// a const so the parser can excise a verbatim echo of it from the
/// model output before extracting the first JSON object — small local
/// models sometimes restate the example before answering, and
/// first-object-wins parsing would otherwise adopt the example (an
/// above-threshold, intent-changing value) as the real classification.
const EXAMPLE_OUTPUT: &str = r#"{"state": "FrustratedTrying", "confidence": 0.7, "reasoning": "Says it is hard but keeps offering new guesses."}"#;

fn build_classification_prompt(ctx: &EngagementContext) -> Prompt {
    let trajectory = if ctx.prior_assessments.is_empty() {
        "(no prior trajectory)".to_string()
    } else {
        ctx.prior_assessments
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let turns_ago = ctx.prior_assessments.len() - i;
                let r = a.reasoning.as_deref().unwrap_or("");
                format!(
                    "{turns_ago} turn(s) ago: {} ({:.2}) — {r}",
                    a.state, a.confidence
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let recent = {
        let joined = ctx
            .recent_child_turns
            .iter()
            .filter(|t| t.speaker == Speaker::Child)
            .map(|t| t.text.as_str())
            .collect::<Vec<_>>()
            .join("\n---\n");
        if joined.is_empty() {
            "(none)".to_string()
        } else {
            joined
        }
    };

    // Written for small local models: every allowed state is defined in
    // one line, a literal example output is shown, and the JSON-only
    // instruction is repeated at the end of the user message where
    // recency keeps it salient.
    let system = format!(
        "You classify a child's engagement state in a learning conversation.\n\n\
        The states:\n\
        - Engaged: actively participating — asking, answering, building on ideas.\n\
        - Reflecting: thinking something over — slower, tentative, but still working on the idea.\n\
        - FrustratedStuck: upset or giving up (\"this is too hard\", \"I can't\") and no longer trying.\n\
        - FrustratedTrying: finding it hard or annoying but still making attempts.\n\
        - Disengaging: drifting away — flat one-word replies, changing the subject, boredom, asking to stop.\n\
        - Unknown: not enough signal to tell.\n\n\
        Rules:\n\
        - Pick exactly one state from the list above, spelled exactly as shown.\n\
        - \"confidence\" is a number between 0.0 and 1.0.\n\
        - \"reasoning\" is one short sentence.\n\
        - Output ONLY one JSON object. No other text, no markdown fences.\n\n\
        Example output:\n\
        {EXAMPLE_OUTPUT}"
    );

    let user = format!(
        "Recent trajectory (oldest first):\n{trajectory}\n\nMost recent child responses (newest last):\n{recent}\n\nClassify the CURRENT engagement state. Answer with the JSON object only."
    );

    Prompt {
        system,
        messages: vec![Message {
            role: Role::User,
            content: user,
        }],
    }
}

// ── Output parser ─────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct ClassifierOutput {
    state: String,
    confidence: f32,
    #[serde(default)]
    reasoning: Option<String>,
}

fn parse_classification_output(raw: &str) -> std::result::Result<EngagementAssessment, String> {
    // Excise any verbatim echo of the prompt's example before extracting:
    // with first-object-wins parsing, an echoed example would otherwise be
    // adopted as the real classification. If nothing parseable remains,
    // the model's entire output WAS the example — indistinguishable from
    // a genuine identical answer — so fall back to the raw output rather
    // than soft-failing.
    let cleaned = raw.replace(EXAMPLE_OUTPUT, "");
    let json_str = match extract_first_json_object(&cleaned) {
        Some(s) => s,
        None => extract_first_json_object(raw)
            .ok_or_else(|| "no JSON object found in output".to_string())?,
    };
    let parsed: ClassifierOutput =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {e}"))?;

    if !(0.0..=1.0).contains(&parsed.confidence) {
        return Err(format!(
            "confidence {} out of range [0.0, 1.0]",
            parsed.confidence
        ));
    }

    let state = match parsed.state.as_str() {
        "Engaged" => EngagementState::Engaged,
        "Reflecting" => EngagementState::Reflecting,
        "FrustratedStuck" => EngagementState::FrustratedStuck,
        "FrustratedTrying" => EngagementState::FrustratedTrying,
        "Disengaging" => EngagementState::Disengaging,
        "Unknown" => EngagementState::Unknown,
        other => return Err(format!("unknown state name: {other:?}")),
    };

    Ok(EngagementAssessment {
        state,
        confidence: parsed.confidence,
        reasoning: parsed.reasoning,
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use primer_core::inference::{GenerationParams, Prompt, TokenChunk, TokenStream};

    /// Test backend that returns canned text from `generate_stream` and stubs
    /// `name` / `is_available`. `generate` and `summarize` use the default
    /// impls that delegate to `generate_stream`.
    struct CannedBackend(String);

    #[async_trait]
    impl InferenceBackend for CannedBackend {
        fn name(&self) -> &str {
            "canned"
        }

        async fn is_available(&self) -> bool {
            true
        }

        async fn generate_stream(
            &self,
            _prompt: &Prompt,
            _params: &GenerationParams,
        ) -> Result<TokenStream> {
            let text = self.0.clone();
            let chunk = TokenChunk {
                text,
                done: true,
                ..Default::default()
            };
            Ok(Box::pin(stream::once(async move { Ok(chunk) })))
        }
    }

    #[test]
    fn identifier_includes_model_name() {
        let backend = Arc::new(CannedBackend("{}".into())) as Arc<dyn InferenceBackend>;
        let c = LlmEngagementClassifier::new(
            backend,
            "claude-haiku-4-5".into(),
            ClassifierSettings::default(),
        );
        assert_eq!(c.identifier(), "llm:claude-haiku-4-5");
    }

    #[tokio::test]
    async fn classify_parses_well_formed_json() {
        let backend = Arc::new(CannedBackend(
            r#"{"state": "FrustratedStuck", "confidence": 0.85, "reasoning": "child says i don't know"}"#.into()
        )) as Arc<dyn InferenceBackend>;
        let c = LlmEngagementClassifier::new(
            backend,
            "test-model".into(),
            ClassifierSettings::default(),
        );
        let ctx = EngagementContext {
            recent_child_turns: &[],
            prior_assessments: &[],
        };
        let a = c.classify(ctx).await.unwrap();
        assert_eq!(a.state, EngagementState::FrustratedStuck);
        assert!((a.confidence - 0.85).abs() < 1e-6);
        assert_eq!(a.reasoning.as_deref(), Some("child says i don't know"));
    }

    #[tokio::test]
    async fn classify_extracts_json_from_wrapper_text() {
        let backend = Arc::new(CannedBackend(
            r#"Here is my analysis: {"state": "Engaged", "confidence": 0.9, "reasoning": "good"} — hope this helps!"#.into()
        )) as Arc<dyn InferenceBackend>;
        let c = LlmEngagementClassifier::new(
            backend,
            "test-model".into(),
            ClassifierSettings::default(),
        );
        let ctx = EngagementContext {
            recent_child_turns: &[],
            prior_assessments: &[],
        };
        let a = c.classify(ctx).await.unwrap();
        assert_eq!(a.state, EngagementState::Engaged);
    }

    #[tokio::test]
    async fn classify_ignores_echoed_prompt_example_before_real_answer() {
        // Small local models sometimes restate the prompt's example
        // before answering. First-object-wins parsing must not adopt
        // the echoed example (FrustratedTrying at 0.7 — an
        // above-threshold, intent-changing value) as the classification.
        let backend = Arc::new(CannedBackend(format!(
            "{EXAMPLE_OUTPUT}\n{{\"state\": \"Engaged\", \"confidence\": 0.9, \"reasoning\": \"asks a follow-up\"}}"
        ))) as Arc<dyn InferenceBackend>;
        let c = LlmEngagementClassifier::new(
            backend,
            "test-model".into(),
            ClassifierSettings::default(),
        );
        let ctx = EngagementContext {
            recent_child_turns: &[],
            prior_assessments: &[],
        };
        let a = c.classify(ctx).await.unwrap();
        assert_eq!(a.state, EngagementState::Engaged);
        assert!((a.confidence - 0.9).abs() < 1e-6);
    }

    #[tokio::test]
    async fn classify_accepts_output_that_is_exactly_the_example() {
        // A model whose entire output is byte-identical to the prompt's
        // example is indistinguishable from a genuine identical answer.
        // The excision must fall back to the raw output instead of
        // soft-failing to Unknown.
        let backend =
            Arc::new(CannedBackend(EXAMPLE_OUTPUT.to_string())) as Arc<dyn InferenceBackend>;
        let c = LlmEngagementClassifier::new(
            backend,
            "test-model".into(),
            ClassifierSettings::default(),
        );
        let ctx = EngagementContext {
            recent_child_turns: &[],
            prior_assessments: &[],
        };
        let a = c.classify(ctx).await.unwrap();
        assert_eq!(a.state, EngagementState::FrustratedTrying);
    }

    #[tokio::test]
    async fn classify_returns_unknown_on_unparseable_output() {
        let backend =
            Arc::new(CannedBackend("not json at all".into())) as Arc<dyn InferenceBackend>;
        let c = LlmEngagementClassifier::new(
            backend,
            "test-model".into(),
            ClassifierSettings::default(),
        );
        let ctx = EngagementContext {
            recent_child_turns: &[],
            prior_assessments: &[],
        };
        let a = c.classify(ctx).await.unwrap();
        assert_eq!(a.state, EngagementState::Unknown);
        assert_eq!(a.confidence, 0.0);
        assert!(a.reasoning.is_some());
    }

    #[tokio::test]
    async fn classify_returns_unknown_on_invalid_state_name() {
        let backend = Arc::new(CannedBackend(
            r#"{"state": "EnthusiastiCallyEngaged", "confidence": 1.0, "reasoning": ""}"#.into(),
        )) as Arc<dyn InferenceBackend>;
        let c = LlmEngagementClassifier::new(
            backend,
            "test-model".into(),
            ClassifierSettings::default(),
        );
        let ctx = EngagementContext {
            recent_child_turns: &[],
            prior_assessments: &[],
        };
        let a = c.classify(ctx).await.unwrap();
        assert_eq!(a.state, EngagementState::Unknown);
    }

    #[tokio::test]
    async fn classify_returns_unknown_on_out_of_range_confidence() {
        let backend = Arc::new(CannedBackend(
            r#"{"state": "Engaged", "confidence": 2.5, "reasoning": ""}"#.into(),
        )) as Arc<dyn InferenceBackend>;
        let c = LlmEngagementClassifier::new(
            backend,
            "test-model".into(),
            ClassifierSettings::default(),
        );
        let ctx = EngagementContext {
            recent_child_turns: &[],
            prior_assessments: &[],
        };
        let a = c.classify(ctx).await.unwrap();
        assert_eq!(a.state, EngagementState::Unknown);
    }

    #[tokio::test]
    async fn classify_returns_unknown_on_backend_error() {
        struct ErrorBackend;

        #[async_trait]
        impl InferenceBackend for ErrorBackend {
            fn name(&self) -> &str {
                "error-test"
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
                    "simulated network error".into(),
                ))
            }
        }

        let backend = Arc::new(ErrorBackend) as Arc<dyn InferenceBackend>;
        let c = LlmEngagementClassifier::new(
            backend,
            "test-model".into(),
            ClassifierSettings::default(),
        );
        let ctx = EngagementContext {
            recent_child_turns: &[],
            prior_assessments: &[],
        };
        let a = c.classify(ctx).await.unwrap();
        assert_eq!(a.state, EngagementState::Unknown);
        assert_eq!(a.confidence, 0.0);
        assert!(a.reasoning.is_some());
        assert!(
            a.reasoning.unwrap().contains("backend error"),
            "reasoning should mention backend error origin"
        );
    }
}
