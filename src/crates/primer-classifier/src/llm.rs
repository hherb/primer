//! LLM-backed engagement classifier. Wraps any `InferenceBackend`
//! (via Arc<dyn ...>) so memory-constrained devices can reuse the chat
//! backend, while bigger machines can use a separate small model.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::warn;

use primer_core::classifier::{EngagementAssessment, EngagementContext};
use primer_core::error::Result;
use primer_core::inference::InferenceBackend;

use crate::settings::ClassifierSettings;
use crate::EngagementClassifier;

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
        Self { backend, settings, identifier }
    }
}

#[async_trait]
impl EngagementClassifier for LlmEngagementClassifier {
    fn identifier(&self) -> &str { &self.identifier }

    async fn classify(&self, ctx: EngagementContext<'_>) -> Result<EngagementAssessment> {
        let prompt = build_classification_prompt(&ctx);
        let params = primer_core::inference::GenerationParams {
            max_tokens: self.settings.generation_max_tokens,
            temperature: self.settings.generation_temperature,
            top_p: self.settings.generation_top_p,
            stop_sequences: vec![],
        };
        let raw = match self.backend.generate(&prompt, &params).await {
            Ok(r) => r,
            Err(e) => {
                warn!(classifier = %self.identifier, error = ?e, "classifier backend call failed");
                return Ok(EngagementAssessment::unknown_low_confidence(
                    format!("backend error: {e}")
                ));
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

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Return a prefix of `s` that is at most `max_chars` Unicode scalar values
/// long. Unlike a raw byte-index slice, this never panics on multi-byte
/// character boundaries.
fn truncate_to_chars(s: &str, max_chars: usize) -> &str {
    if s.chars().count() <= max_chars {
        return s;
    }
    let end = s.char_indices().nth(max_chars).map(|(i, _)| i).unwrap_or(s.len());
    &s[..end]
}

// ── Prompt builder ────────────────────────────────────────────────────────────

use primer_core::conversation::Speaker;
use primer_core::inference::{Message, Prompt, Role};
use primer_core::learner::EngagementState;

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
                format!("{turns_ago} turn(s) ago: {:?} ({:.2}) — {r}", a.state, a.confidence)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let recent = ctx.recent_child_turns
        .iter()
        .filter(|t| t.speaker == Speaker::Child)
        .map(|t| t.text.as_str())
        .collect::<Vec<_>>()
        .join("\n---\n");

    let system = "You classify a child's engagement state in a Socratic learning \
        conversation. Output ONLY valid JSON of the form: \
        {\"state\": \"<one of: Engaged, Reflecting, FrustratedStuck, FrustratedTrying, \
        Disengaging, Unknown>\", \"confidence\": 0.0-1.0, \"reasoning\": \
        \"<one short sentence>\"} — no other text.".to_string();

    let user = format!(
        "Recent trajectory (oldest first):\n{trajectory}\n\nMost recent child responses:\n{recent}\n\nClassify the CURRENT engagement state."
    );

    Prompt {
        system,
        messages: vec![Message { role: Role::User, content: user }],
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
    let json_str = extract_first_json_object(raw)
        .ok_or_else(|| "no JSON object found in output".to_string())?;
    let parsed: ClassifierOutput = serde_json::from_str(json_str)
        .map_err(|e| format!("JSON parse error: {e}"))?;

    if !(0.0..=1.0).contains(&parsed.confidence) {
        return Err(format!("confidence {} out of range [0.0, 1.0]", parsed.confidence));
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

/// Extract the first balanced JSON object from `raw`. Tolerates wrapper
/// text before/after, and JSON containing nested braces. Returns the
/// substring including the outer braces, or None if not found.
fn extract_first_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let bytes = raw.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if escape { escape = false; continue; }
        if in_string {
            match b {
                b'\\' => escape = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&raw[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
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
        fn name(&self) -> &str { "canned" }

        async fn is_available(&self) -> bool { true }

        async fn generate_stream(
            &self,
            _prompt: &Prompt,
            _params: &GenerationParams,
        ) -> Result<TokenStream> {
            let text = self.0.clone();
            let chunk = TokenChunk { text, done: true };
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
            backend, "test-model".into(), ClassifierSettings::default()
        );
        let ctx = EngagementContext { recent_child_turns: &[], prior_assessments: &[] };
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
            backend, "test-model".into(), ClassifierSettings::default()
        );
        let ctx = EngagementContext { recent_child_turns: &[], prior_assessments: &[] };
        let a = c.classify(ctx).await.unwrap();
        assert_eq!(a.state, EngagementState::Engaged);
    }

    #[tokio::test]
    async fn classify_returns_unknown_on_unparseable_output() {
        let backend = Arc::new(CannedBackend("not json at all".into()))
            as Arc<dyn InferenceBackend>;
        let c = LlmEngagementClassifier::new(
            backend, "test-model".into(), ClassifierSettings::default()
        );
        let ctx = EngagementContext { recent_child_turns: &[], prior_assessments: &[] };
        let a = c.classify(ctx).await.unwrap();
        assert_eq!(a.state, EngagementState::Unknown);
        assert_eq!(a.confidence, 0.0);
        assert!(a.reasoning.is_some());
    }

    #[tokio::test]
    async fn classify_returns_unknown_on_invalid_state_name() {
        let backend = Arc::new(CannedBackend(
            r#"{"state": "EnthusiastiCallyEngaged", "confidence": 1.0, "reasoning": ""}"#.into()
        )) as Arc<dyn InferenceBackend>;
        let c = LlmEngagementClassifier::new(
            backend, "test-model".into(), ClassifierSettings::default()
        );
        let ctx = EngagementContext { recent_child_turns: &[], prior_assessments: &[] };
        let a = c.classify(ctx).await.unwrap();
        assert_eq!(a.state, EngagementState::Unknown);
    }

    #[tokio::test]
    async fn classify_returns_unknown_on_out_of_range_confidence() {
        let backend = Arc::new(CannedBackend(
            r#"{"state": "Engaged", "confidence": 2.5, "reasoning": ""}"#.into()
        )) as Arc<dyn InferenceBackend>;
        let c = LlmEngagementClassifier::new(
            backend, "test-model".into(), ClassifierSettings::default()
        );
        let ctx = EngagementContext { recent_child_turns: &[], prior_assessments: &[] };
        let a = c.classify(ctx).await.unwrap();
        assert_eq!(a.state, EngagementState::Unknown);
    }

    #[tokio::test]
    async fn classify_returns_unknown_on_backend_error() {
        struct ErrorBackend;

        #[async_trait]
        impl InferenceBackend for ErrorBackend {
            fn name(&self) -> &str { "error-test" }

            async fn is_available(&self) -> bool { true }

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
        let ctx = EngagementContext { recent_child_turns: &[], prior_assessments: &[] };
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
