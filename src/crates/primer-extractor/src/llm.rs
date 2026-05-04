//! LLM-backed concept extractor. Wraps any `InferenceBackend`
//! (via `Arc<dyn ...>`) so the chat backend can be reused or a
//! cheaper extraction-specific model can be swapped in independently.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::warn;

use primer_core::conversation::Speaker;
use primer_core::error::Result;
use primer_core::extractor::{ConceptExtraction, ExtractionContext};
use primer_core::inference::{InferenceBackend, Message, Prompt, Role};
use primer_core::llm_util::{extract_first_json_object, truncate_to_chars};

use crate::ConceptExtractor;
use crate::consts;
use crate::settings::ExtractorSettings;

pub struct LlmConceptExtractor {
    backend: Arc<dyn InferenceBackend>,
    settings: ExtractorSettings,
    identifier: String,
}

impl LlmConceptExtractor {
    /// Build a new LLM-backed extractor.
    ///
    /// The stable `identifier` is composed as `llm:{backend}:{model}`
    /// — e.g. `llm:cloud-anthropic:claude-haiku-4-5`. Including the
    /// backend keeps two extractors using the same model name across
    /// different backends (cloud vs ollama) distinguishable in any
    /// future analysis or filtering, mirroring how `turn_classifications`
    /// filters by classifier identifier.
    pub fn new(
        backend: Arc<dyn InferenceBackend>,
        model: String,
        settings: ExtractorSettings,
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
impl ConceptExtractor for LlmConceptExtractor {
    fn identifier(&self) -> &str {
        &self.identifier
    }

    async fn extract(&self, ctx: ExtractionContext<'_>) -> Result<ConceptExtraction> {
        let prompt = build_extraction_prompt(&ctx);
        let params = primer_core::inference::GenerationParams {
            max_tokens: self.settings.generation_max_tokens,
            temperature: self.settings.generation_temperature,
            top_p: self.settings.generation_top_p,
            stop_sequences: vec![],
        };
        let raw = match self.backend.generate(&prompt, &params).await {
            Ok(r) => r,
            Err(e) => {
                warn!(extractor = %self.identifier, error = ?e, "extractor backend call failed");
                return Ok(ConceptExtraction::empty());
            }
        };

        let truncated = truncate_to_chars(&raw, self.settings.max_output_chars);
        match parse_extraction_output(truncated, self.settings.per_concept_chars) {
            Ok(mut e) => {
                truncate_concept_lists(&mut e, self.settings.max_concepts_per_speaker);
                Ok(e)
            }
            Err(reason) => {
                let snippet: String = truncated
                    .chars()
                    .take(consts::LLM_DEBUG_SNIPPET_CHARS)
                    .collect();
                warn!(extractor = %self.identifier, raw = %snippet, %reason, "extractor output unparseable");
                Ok(ConceptExtraction::empty())
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn truncate_concept_lists(e: &mut ConceptExtraction, max_per_speaker: usize) {
    if e.child_concepts.len() > max_per_speaker {
        e.child_concepts.truncate(max_per_speaker);
    }
    if e.primer_concepts.len() > max_per_speaker {
        e.primer_concepts.truncate(max_per_speaker);
    }
}

// ── Prompt builder ────────────────────────────────────────────────────────────

fn build_extraction_prompt(ctx: &ExtractionContext) -> Prompt {
    let context_section = if ctx.recent_turns.is_empty() {
        "(no prior context)".to_string()
    } else {
        ctx.recent_turns
            .iter()
            .map(|t| format!("{}: {}", speaker_label(t.speaker), t.text))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let system = "You extract conceptual topics from a Socratic learning \
        conversation between a child and a teaching companion (the Primer). \
        Output ONLY valid JSON of the form: \
        {\"child_concepts\": [\"<topic>\", ...], \"primer_concepts\": [\"<topic>\", ...]} — \
        no other text. Each topic is a short noun phrase (1–3 words, lowercase). \
        For child_concepts, list topics the child surfaced or engaged with — \
        what they brought up, asked about, or attempted to reason about. \
        For primer_concepts, list topics the Primer introduced — what the \
        child was exposed to via the Primer's response. A topic the Primer \
        asks about counts as Primer-introduced even if the child only \
        acknowledged it. Use [] when a speaker introduced no clear topics. \
        Do not invent topics absent from the text."
        .to_string();

    let user = format!(
        "Prior context (oldest first):\n{context_section}\n\nTurn to analyse:\n{}: {}\n{}: {}\n\nExtract concepts now.",
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

// ── Output parser ─────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct ExtractorOutput {
    #[serde(default)]
    child_concepts: Vec<String>,
    #[serde(default)]
    primer_concepts: Vec<String>,
}

fn parse_extraction_output(
    raw: &str,
    per_concept_chars: usize,
) -> std::result::Result<ConceptExtraction, String> {
    let json_str = extract_first_json_object(raw)
        .ok_or_else(|| "no JSON object found in output".to_string())?;
    let parsed: ExtractorOutput =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {e}"))?;

    Ok(ConceptExtraction {
        child_concepts: normalize_concepts(parsed.child_concepts, per_concept_chars),
        primer_concepts: normalize_concepts(parsed.primer_concepts, per_concept_chars),
    })
}

/// Lower-case, trim, drop empty/over-cap entries, dedupe (preserving
/// first occurrence). The per-concept char cap defends against
/// pathological "concept = entire sentence" outputs from a
/// misbehaving LLM. The cap is configurable via `ExtractorSettings`
/// and defaults to a generous 128 chars (see
/// `consts::DEFAULT_PER_CONCEPT_CHARS`).
fn normalize_concepts(input: Vec<String>, per_concept_chars: usize) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(input.len());
    for c in input {
        let trimmed = c.trim().to_lowercase();
        if trimmed.is_empty() || trimmed.chars().count() > per_concept_chars {
            continue;
        }
        if seen.insert(trimmed.clone()) {
            out.push(trimmed);
        }
    }
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use futures::stream;
    use primer_core::conversation::{Speaker, Turn};
    use primer_core::inference::{GenerationParams, Prompt, TokenChunk, TokenStream};

    /// Test backend that returns canned text from `generate_stream` and stubs
    /// `name` / `is_available`.
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
            let chunk = TokenChunk { text, done: true };
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

    fn ctx<'a>(child: &'a Turn, primer: &'a Turn) -> ExtractionContext<'a> {
        ExtractionContext {
            child_turn: child,
            primer_turn: primer,
            recent_turns: &[],
        }
    }

    #[test]
    fn identifier_includes_backend_and_model() {
        let backend = Arc::new(CannedBackend("{}".into())) as Arc<dyn InferenceBackend>;
        let e = LlmConceptExtractor::new(
            backend,
            "claude-haiku-4-5".into(),
            ExtractorSettings::default(),
        );
        // CannedBackend.name() returns "canned", so the identifier
        // composes as `llm:{backend}:{model}`. This format keeps two
        // extractors using the same model across different backends
        // distinguishable.
        assert_eq!(e.identifier(), "llm:canned:claude-haiku-4-5");
    }

    #[tokio::test]
    async fn extract_parses_well_formed_json() {
        let backend = Arc::new(CannedBackend(
            r#"{"child_concepts": ["photosynthesis", "Plants"], "primer_concepts": ["chlorophyll"]}"#.into(),
        )) as Arc<dyn InferenceBackend>;
        let e =
            LlmConceptExtractor::new(backend, "test-model".into(), ExtractorSettings::default());
        let c = turn(Speaker::Child, "what's photosynthesis?");
        let p = turn(Speaker::Primer, "great question!");
        let r = e.extract(ctx(&c, &p)).await.unwrap();
        assert_eq!(r.child_concepts, vec!["photosynthesis", "plants"]);
        assert_eq!(r.primer_concepts, vec!["chlorophyll"]);
    }

    #[tokio::test]
    async fn extract_handles_wrapper_text() {
        let backend = Arc::new(CannedBackend(
            r#"Here you go: {"child_concepts": ["gravity"], "primer_concepts": []} done"#.into(),
        )) as Arc<dyn InferenceBackend>;
        let e =
            LlmConceptExtractor::new(backend, "test-model".into(), ExtractorSettings::default());
        let c = turn(Speaker::Child, "?");
        let p = turn(Speaker::Primer, "?");
        let r = e.extract(ctx(&c, &p)).await.unwrap();
        assert_eq!(r.child_concepts, vec!["gravity"]);
        assert!(r.primer_concepts.is_empty());
    }

    #[tokio::test]
    async fn extract_returns_empty_on_unparseable_output() {
        let backend =
            Arc::new(CannedBackend("not json at all".into())) as Arc<dyn InferenceBackend>;
        let e =
            LlmConceptExtractor::new(backend, "test-model".into(), ExtractorSettings::default());
        let c = turn(Speaker::Child, "?");
        let p = turn(Speaker::Primer, "?");
        let r = e.extract(ctx(&c, &p)).await.unwrap();
        assert!(r.child_concepts.is_empty());
        assert!(r.primer_concepts.is_empty());
    }

    #[tokio::test]
    async fn extract_returns_empty_on_backend_error() {
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
        let e =
            LlmConceptExtractor::new(backend, "test-model".into(), ExtractorSettings::default());
        let c = turn(Speaker::Child, "?");
        let p = turn(Speaker::Primer, "?");
        let r = e.extract(ctx(&c, &p)).await.unwrap();
        assert!(r.child_concepts.is_empty());
        assert!(r.primer_concepts.is_empty());
    }

    #[tokio::test]
    async fn extract_truncates_concept_lists_per_speaker_cap() {
        let mut payload_child: Vec<String> = (0..20).map(|i| format!("child-{i}")).collect();
        let payload_primer: Vec<String> = (0..20).map(|i| format!("primer-{i}")).collect();
        payload_child.dedup();
        let payload = serde_json::json!({
            "child_concepts": payload_child,
            "primer_concepts": payload_primer,
        });
        let backend = Arc::new(CannedBackend(payload.to_string())) as Arc<dyn InferenceBackend>;
        let settings = ExtractorSettings {
            max_concepts_per_speaker: 5,
            ..ExtractorSettings::default()
        };
        let e = LlmConceptExtractor::new(backend, "test-model".into(), settings);
        let c = turn(Speaker::Child, "?");
        let p = turn(Speaker::Primer, "?");
        let r = e.extract(ctx(&c, &p)).await.unwrap();
        assert_eq!(r.child_concepts.len(), 5);
        assert_eq!(r.primer_concepts.len(), 5);
    }

    #[tokio::test]
    async fn extract_drops_concepts_over_per_concept_cap() {
        // With a tight per-concept cap of 5, "gravity" (7) is dropped;
        // "mass" (4) survives. Confirms the cap is read from settings.
        let backend = Arc::new(CannedBackend(
            r#"{"child_concepts": ["gravity", "mass"], "primer_concepts": []}"#.into(),
        )) as Arc<dyn InferenceBackend>;
        let settings = ExtractorSettings {
            per_concept_chars: 5,
            ..ExtractorSettings::default()
        };
        let e = LlmConceptExtractor::new(backend, "test-model".into(), settings);
        let c = turn(Speaker::Child, "?");
        let p = turn(Speaker::Primer, "?");
        let r = e.extract(ctx(&c, &p)).await.unwrap();
        assert_eq!(r.child_concepts, vec!["mass"]);
    }

    #[tokio::test]
    async fn extract_normalizes_lowercase_and_dedupes() {
        let backend = Arc::new(CannedBackend(
            r#"{"child_concepts": ["Gravity", "gravity", "  "], "primer_concepts": []}"#.into(),
        )) as Arc<dyn InferenceBackend>;
        let e =
            LlmConceptExtractor::new(backend, "test-model".into(), ExtractorSettings::default());
        let c = turn(Speaker::Child, "?");
        let p = turn(Speaker::Primer, "?");
        let r = e.extract(ctx(&c, &p)).await.unwrap();
        assert_eq!(r.child_concepts, vec!["gravity"]);
    }
}
